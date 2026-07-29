# 平台特定代码审查报告

**审查范围**: `src-tauri/src/platform/{mod,windows,macos}.rs` + `src-tauri/src/logstore.rs` + `src-tauri/src/lib.rs` 中平台调用上下文

## Critical (0)
未发现会造成数据丢失、UAF panic 或安全漏洞的 critical 级问题。

## High (4)

### H1. Windows: `apply_z_order` 在 worker 线程修改窗口 style, 与 main thread WndProc 存在 TOCTOU 竞态
- **位置**: `src-tauri/src/platform/windows.rs:171-220`
- **类型**: 多线程 / 消息循环
- **描述**: `SetWindowLongW` + `SetWindowPos` 文档 thread-safe,但窗口的 WndProc 在 main thread。背景 hover-emitter 线程和 main thread (`run_on_main_thread` 派来的 pin 模式切换) 并发调用 `apply_z_order`,读到-改-写不是原子的。
- **影响**: PinBottom → PinTop 切换时,主线程 dispatch 跟 emitter 50ms tick 撞车,理论概率 1/40。v0.2.4 已知 3/7 命中率可能跟这条有关。
- **修法**: 把 `apply_z_order` 整个 dispatch 到 main thread,或在 `apply_z_order` 顶部加 `std::sync::Mutex<()>` 串行化两个 write path。

### H2. macOS: NSWindow 裸指针在 raw closure 中 dereference, 存在窗口销毁竞态 UAF
- **位置**: `src-tauri/src/platform/macos.rs:248-270`
- **类型**: 资源生命周期 / UAF
- **描述**: `run_on_main_thread` 是 async 派发,closure 不立即执行。期间窗口可能被 user 关闭或 app quit 触发 destroy。**closure 真正跑时** `ptr` 可能指向已 destroy 的 NSWindow → `setLevel` 是 msg_send 到 freed object → UB / 进程 crash。`is_null()` 检查只能挡已完全销毁的情况,挡不住 half-destroyed。
- **修法**: 用 `objc2::rc::Retained<NSWindow>` 包一层 retain,或者在 closure 顶上加 `if window.windowNumber() == 0 { return; }` 守卫。

### H3. logstore: 错误消息明文落盘, 可能写入 API key / Cookie
- **位置**: `src-tauri/src/logstore.rs:230-252`
- **类型**: 敏感数据 / 权限
- **描述**: `LogEntry::message` 是 `String`,调用者传任意文本。已知源头:provider 失败时 `format!("...token: {}", token)`; stepfun/anysearch 401 错误回吐部分 Bearer;commands::save_cookie 失败 message。如果用户把 `~/Library/Application Support/com.musage.app/app_log.jsonl` 上传到 GitHub issue / 截图分享支持 → 密钥泄露;macOS Spotlight / Windows Search 可能索引这文件。
- **修法**: 在 `LogEntry::error/warn/info` 构造器里加 `redact()` 步骤,匹配 `Bearer ` / `sk-` / `eyJ` (JWT 头) / `Oasis-Token=` / `MUSAGE_TOKEN=` 等模式做正则替换。

### H4. logstore: tmp 文件 rename 失败 → 残留孤儿文件
- **位置**: `src-tauri/src/logstore.rs:329-343`
- **类型**: 资源泄漏 / 跨平台
- **描述**: 进程在 rename 进行中被 kill(`SIGKILL` / 断电),`tmp` 文件没被 rename 也没被删 → 每次启动**不**会清理 `app_log.jsonl.tmp`。实测一年下来可能堆几十个 `.tmp` 残留。
- **修法**: 在 `logstore::load_from_disk` 启动时 `std::fs::remove_file(<log_path>.with_extension("jsonl.tmp"))` 一下,扫掉上次 crash 残留。

## Medium (8)

### M1. macOS: `set_window_level` 的 `is_pin_bottom: bool` 参数完全 dead code
- **位置**: `src-tauri/src/platform/macos.rs:244-263`
- **修法**: 删参数,所有 caller 改 `set_window_level(app, level)`。

### M2. Windows: `SetWindowLongW` 对 `ZOrder::TopMost` / `ZOrder::NotTopMost` 是冗余
- **位置**: `src-tauri/src/platform/windows.rs:201-220`
- **描述**: Win32 文档:`SetWindowPos(HWND_TOPMOST, ...)` 自动 SET `WS_EX_TOPMOST`;`SetWindowPos(HWND_NOTOPMOST, ...)` 自动 CLEAR。每 tick (~50ms) 写 style 是 2 次 user→kernel transition,20Hz × 1 次 = 20 次/秒 = 172 万次/天,纯浪费。
- **修法**: 只对 `ZOrder::Bottom` 保留 `SetWindowLongW`;其余直接 `SetWindowPos` 就行。

### M3. Windows: `ensure_per_monitor_v2_dpi` 失败时静默降级, 跨 DPI 屏 hit test 永久错位
- **位置**: `src-tauri/src/platform/windows.rs:113-121`
- **描述**: 多屏 + 混合 DPI 缩放(150% 主屏 + 100% 副屏)的用户, 如果 release manifest 写错,V2 上不去,hover-raise 永久失效,而且**没有任何 log**。
- **修法**: 失败时 `tracing::warn!` 一次,或加一个对外的 `dpi_mode` 状态让 settings 面板 help 文字能引用。

### M4. macOS: fullscreen watcher 用 `NSMenu::menuBarVisible` 启发式, 误判率高
- **位置**: `src-tauri/src/platform/macos.rs:465-475` + `521-562`
- **描述**: macOS 上"菜单栏隐藏 → 用户在全屏"是经验性推断。1) "系统设置 → 桌面与程序坞 → 在桌面上自动隐藏并显示菜单栏"开启时,即使不在全屏,菜单栏也隐藏 → watcher 误判全屏。2) 副屏进入全屏但主屏菜单栏未隐藏。
- **修法**: 改用 `[NSApp applicationIsFullscreen]` 或 `[(window.screen ?: NSScreen.mainScreen) isFullscreen]`。

### M5. macOS: `set_window_level` 锁外发 dispatch + 强转 `CGWindowLevel → NSWindowLevel`
- **位置**: `src-tauri/src/platform/macos.rs:263`
- **修法**: 显式 `let ns_level: i64 = level as i64;` 让 API 边界可见。

### M6. Windows: hit test `getwindowrect` 与 `getcursorpos` 不是 atomic
- **位置**: `src-tauri/src/platform/windows.rs:489-505`
- **修法**: 维持现状即可。仅在 H1 fix 时顺手处理。

### M7. macOS + Windows: `LEVEL_SWITCHING_ACTIVE` / `HOVER_STATE_RESET` 是 `static AtomicBool`, 多 Tauri runtime 共享
- **位置**: `src-tauri/src/platform/macos.rs:55,62` + `src-tauri/src/platform/windows.rs:130,138`
- **影响**: 测试 isolation 差,未来扩展风险。维持现状。

### M8. macOS: `set_window_level` 异步 dispatch, 顺序非严格
- **位置**: `src-tauri/src/platform/macos.rs:67-92`
- **影响**: 极低概率。维持现状。真正 fix 是 H1 的"主线程化 apply_z_order"。

## Low (9)

### L1. macOS: `chrono` 在 logstore.rs 仅用于 `timestamp_millis()`, 不必要的大依赖
- **位置**: `src-tauri/src/logstore.rs:359-380`
- **修法**: v0.3 tech debt 顺手。

### L2. macOS: `MainThreadMarker unavailable` log level 不一致 (warn vs trace)
- **位置**: `src-tauri/src/platform/macos.rs:506` vs `:330`

### L3. Windows: `debug_assert!(!hwnd.is_null())` 跟后面的 `if hwnd.is_null()` 重复
- **位置**: `src-tauri/src/platform/windows.rs:175-178`
- **修法**: 删 `debug_assert!`, 保留 runtime `if`。

### L4. Windows: `apply_z_order` 不检查 `SetWindowLongW` / `SetWindowPos` 返回值
- **位置**: `src-tauri/src/platform/windows.rs:171-220`
- **修法**: `SetWindowPos` 返 0 时 `tracing::trace!` 一次。

### L5. macOS: `set_window_hover_raise` 在 Win/macOS 都是 no-op, 仅 Linux stub 真干活
- **位置**: `src-tauri/src/platform/macos.rs:96-99` + `src-tauri/src/platform/windows.rs:287-290`
- **修法**: 接受现状, 注释写明意图即可。

### L6. macOS: `LEVEL_BELOW_NORMAL = kCGNormalWindowLevel - 1` 依赖常量值, Apple 改的话 silently break
- **位置**: `src-tauri/src/platform/macos.rs:39-40`
- **修法**: unit test: `assert_eq!(LEVEL_BELOW_NORMAL, -1);`

### L7. Windows: `SetWindowPos` 调用包含 `SWP_NOACTIVATE`, 但 Tray 菜单的 `force_top_floating` 不带
- **位置**: `src-tauri/src/tray.rs:306-313`
- **修法**: 写个 doc comment 说明"此处故意抢焦点"。

### L8. logstore: `MAX_ENTRIES = 200` truncate 频率 ~1/200 pushes
- **位置**: `src-tauri/src/logstore.rs:125-138` + `285-318`
- **修法**: 维持。

### L9. macOS: `set_window_level` 的 `app2.emit("musage://backdrop-refresh", ())` 失败时 `let _ = ...` 静默吞
- **位置**: `src-tauri/src/platform/macos.rs:274`

## 试验室补充(已知 trade-off, NOT 修, 仅记录)

1. **Windows: `HitTest::Covered` 5-tick dwell 的 UX 代价**: 鼠标停在浮窗被盖区域 >250ms 浮窗会弹出遮一下, v0.2.4 用户场景修复用。 跟 macOS 严格"未被遮挡"语义**有意分歧**。
2. **Windows hover emitter 工作在非 main thread**: 见 H1, 大改。
3. **macOS: `set_window_hover_raise` 整链路 no-op**: 见 L5, 抽象层必需.
4. **Linux 整 stub**: `mod.rs:42-77` 6 个 fn 全 stub, **v0.3 之前不做**。

## 总览

| 等级 | Windows | macOS | logstore | 跨平台 lib.rs |
|---|---|---|---|---|
| Critical | 0 | 0 | 0 | 0 |
| High | 2 (H1, H3[共用]) | 1 (H2) | 2 (H3, H4) | 0 |
| Medium | 3 (M2, M3, M6) | 4 (M1, M4, M5, M8) | 0 | 1 (M7[共用]) |
| Low | 3 (L3, L4, L7) | 4 (L1[logstore], L2, L5, L6, L9) | 1 (L1, L8) | 0 |

**最值得动手的两个**:
- **H1** (Windows 线程 race) — 影响 v0.2.4 已知 3/7 命中率这个隐性 bug 的根因之一
- **H3** (log 包含密钥) — 安全问题, 不可推迟到 v0.3

**其余可以分批进 v0.3 / 0.4 节奏**:
- H2 / H4: 修一下成本低, 推 v0.2.5 hotfix
- M / L 一档: tech debt, 不阻塞 release

## 文件引用
- `/Users/wyh/Project/Musage/src-tauri/src/platform/windows.rs`
- `/Users/wyh/Project/Musage/src-tauri/src/platform/macos.rs`
- `/Users/wyh/Project/Musage/src-tauri/src/platform/mod.rs`
- `/Users/wyh/Project/Musage/src-tauri/src/logstore.rs`
- `/Users/wyh/Project/Musage/src-tauri/src/lib.rs:208-213, 276-279`
- `/Users/wyh/Project/Musage/src-tauri/src/tray.rs:258-313`
