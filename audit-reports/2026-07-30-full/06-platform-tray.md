# Musage 全量代码审查 — Platform + Tray 报告

**范围**:src-tauri/src/platform/{windows,macos}.rs(Win PinBottom hover-raise 重写 + macOS NSWindow setLevel + 命中测试)+ src-tauri/src/tray.rs(1196 行,跨线程 tray dispatch + 动态图标 + 多 instance 后缀 + Win UTF-16 tooltip 截断)。

**未审**:lib.rs 的 `start_hover_emitter` / `start_fullscreen_watcher` 调用方;commands.rs 的 `quit_app` 设置 SHUTDOWN_NATIVE_THREADS 的 150ms 等待(已确认正确);其他 platform 子模块未在本轮 scope。

整体判断:**真 bug 极少且偏防御性**——三个文件均经过 2026-06-20 + 2026-07-02 + 2026-07-06 + 2026-07-28 + 2026-07-29 + 2026-07-30 多轮审查(H1-L17 标号),核心逻辑安全(dwell hysteresis / Mutex 串行 / Retained 防 UAF / cross-thread dispatch / Mutex poison 恢复)。多数发现是 **D5-102(e2af7d2)三线程修复不完整** + **代码一致性缺口**,非可触发生产崩溃。下面按 P1/P2/P3 列出。

---

## P1 (高)

### D6-001 — D5-102(e2af7d2)三线程修复不完整:`start_fullscreen_watcher` OS 线程没检查 `SHUTDOWN_NATIVE_THREADS`,quit_app 后线程泄漏
**置信度**:高(已确认) **文件**:macos.rs:440-488(start_fullscreen_watcher);对照 commands/mod.rs:1168 quit_app 已 store(true) **触发条件**:用户点 tray 菜单 quit,真 macOS 上 *fullscreen watcher* 线程(2 秒一 tick)在下个 tick 时继续跑 `app.run_on_main_thread(...)`,Tauri event loop 已退出 → dispatch 返 Err → `hide_floating` / `show_floating` 静默吞 Err → 线程继续 sleep 2 秒 → 直至进程被 std::thread OS 层强杀。

**根因**:e2af7d2 commit 2026-07-30 16:56 只在 `start_hover_emitter` 里加 SHUTDOWN_NATIVE_THREADS 检查:
```
src-tauri/src/platform/macos.rs:158-169 (hover emitter)
src-tauri/src/platform/windows.rs:354-363 (hover emitter)
```
**macOS `start_fullscreen_watcher`(macos.rs:440-488)整个 `loop` 没加检查**:
```rust
loop {
    thread::sleep(Duration::from_secs(2));   // ← quit_app 后还睡 2s
    if !AUTO_HIDE_IN_FULLSCREEN.load(...) { ... continue; }
    let is_fs = is_menubar_hidden(&app);      // ← 这里调 run_on_main_thread
    ...
}
```
`commands/mod.rs:1156-1176 quit_app` 流程是:`SHUTDOWN_NATIVE_THREADS.store(true)` → `tokio::time::sleep(150ms)` → `app.exit(0)`。150ms 内:
- Win hover emitter 50ms tick → 看到 SHUTDOWN 退出 ✓
- macOS hover emitter 50ms tick → 看到 SHUTDOWN 退出 ✓
- **macOS fullscreen watcher 2000ms tick → 还在 sleep → 进入 `is_menubar_hidden` → Tauri 已 exit → dispatch Err 静默吞 → 再睡 2s → process 被 OS 强杀线程**

**影响**:
1. **线程泄漏**(虽然只持续 ~2s,但 OS 强杀前 std::thread 跑空 tick 是非优雅退出,违反 2026-07-30 e2af7d2 的明确目标)
2. **`is_menubar_hidden` 内 `run_on_main_thread` 失败时记录 warn**(macos.rs:536),shut down 期间该 warn 被静默吞没,误导后续日志分析
3. **dispatch_err 路径** macos.rs:535-541 写 `*g = Some(false)` 后 cvar notify_all,但 hover emitter 已退出,没人 wait —— `wait_timeout` 永远占着 slot 直到 200ms timeout,然后 caller 已经被退出 `app`,无意义空转
4. **进程退出延迟**:`~2s 内 thread::sleep 是 best-effort,destructor 不跑,但 std::thread runtime 不会主动结束该线程,**Tauri 等 OS**;真生产中用户看到的"Quit 之后图标卡几秒才彻底消失"可能就是这条线

**证据/调用链**:
```
quit_app handler
  → commands/mod.rs:1168 store(SHUTDOWN_NATIVE_THREADS, true)
  → 150ms sleep
  → app.exit(0)                     ← Tauri 关 webview,关闭 event loop
                                      ↓
macos.rs:444-482 spawned thread (fullscreen watcher)
  → sleep 2s 中(下次醒来已是 qu_app 之后)
  → is_menubar_hidden(app) → run_on_main_thread → Err(NSApp 已退)
  → write Some(false) + cvar notify_all (但没人 wait)
  → 循环 / 进 sleep → 2s 后 OS 杀进程
```

**修复建议**:在 macos.rs:449 `loop` 开头加 SHUTDOWN_NATIVE_THREADS 检查(对齐 hover emitter 已有的模式):
```rust
loop {
    thread::sleep(Duration::from_secs(2));
    if crate::poller::SHUTDOWN_NATIVE_THREADS
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        tracing::debug!("macOS fullscreen watcher 收到 SHUTDOWN, 退出");
        break;
    }
    if !AUTO_HIDE_IN_FULLSCREEN.load(...) { ... }
    ...
}
```
150ms sleep 余量内能干净退出,跟另外两个 OS 线程行为对齐。

**待实测**:跑 `pnpm tauri dev` → 在 macOS 全屏切换 / 不切都试试 → tray 菜单点 quit → 看进程是否在 ~200ms 内完全退出(log 看到 fullscreen watcher 的 "收到 SHUTDOWN" 信息)。

---

## P2 (中)

### D6-002 — Win hover emitter 退场 hysteresis 在 "稳定→新退出候选" 转换时 `pending_value` 不重置,hit test 抖动 Visible ↔ Outside 每 tick 可击穿退场阈值
**置信度**:中(理论可触发条件依赖病态 hit test 抖动) **文件**:windows.rs:382-411 **触发条件**:鼠标在 WebView2 transient 子窗口闪烁 / DWM 帧切换 / 用户鼠标压在 rect 边界 1px 且同时 WebView2 子窗口瞬态 1 帧的不同 `WindowFromPoint` 值反复抢 root → hit test 每 50ms 在 Visible ↔ Outside 之间摆。

**根因**:emitter 的退场 accumulating 状态机(`pending_ticks`,`pending_value`)在 "stable"(line 385-401,`inside == last_inside`)分支只 reset `pending_ticks = 0`,**不 reset `pending_value`**:
```rust
if inside == last_inside {
    pending_ticks = 0;
    if inside && raised && LEVEL_SWITCHING_ACTIVE.load(...) {
        steady_ticks = steady_ticks.saturating_add(1);
        ...
    }
    continue;                          // ← pending_value 残留
}
```
退场路径(line 405-410):
```rust
if pending_value != inside {
    pending_value = inside;
    pending_ticks = 1;
} else {
    pending_ticks = pending_ticks.saturating_add(1);
}
```
**病态时序**(last_inside=true,初始 Visible 已被采纳):Outside (pending_value=false, ticks=1) → Outside (ticks=2) → **Visible 抖动**(stable 分支 → pending_ticks=0,但 pending_value 仍是 false) → Outside (pending_value==inside → ticks += 1 = 1,而不是从 0 起步) → Visible(抖动) → ... 永远累不到 EXIT_THRESHOLD=3。用户的鼠标**实际**已离开浮窗(连续 Outside tick 多拍),但 hover-raise 不 demote。

**影响**:
- 实战中要触发,需要**WebView2 + DWM 同步每 tick 抢 hit test** —— 极为罕见(~50ms 间隔精准切换 Visible/Outside),但理论可达
- 主要风险面:鼠标在浮窗边缘反复 `WindowFromPoint` 抢不到根(抖动)同时 WebView2 渲染 sub-window 闪一帧 → 用户视觉上"鼠标已离开",系统上 hover-raise 不动

**证据/调用链**:
```rust
// windows.rs:385-401 stable 分支
if inside == last_inside {
    pending_ticks = 0;                  // ← reset ticks
    // ← pending_value 没动!上轮的"想要切换到的值"残留
    continue;
}
// windows.rs:405-410 非 stable 分支
if pending_value != inside {            // ← pending_value 跟 last entered 的 inside 比,不跟 last_inside 比
    pending_value = inside;             // ← 残留值跟当前 inside 凑巧相等时分支
    pending_ticks = 1;
} else {
    pending_ticks = pending_ticks.saturating_add(1);
}
```

**修复建议**:stable 分支补 `pending_value = last_inside;` 一行 —— "回到稳定时连目标值也清掉",下次 transition 必走 `pending_value != inside` 起步 pending_ticks=1。patch:
```rust
if inside == last_inside {
    pending_value = last_inside;       // ← fix
    pending_ticks = 0;
    ...
    continue;
}
```
注意 macOS 的 `start_hover_emitter`(macos.rs:189-192)有**完全相同的模式**,但 macOS 是 ENTER_THRESHOLD=1 / EXIT_THRESHOLD=2,理论上同样可击穿 —— 但 1 tick ENTER 的语义本来就 fast-path,影响面更小。建议两处一起修。

**待实测**:手动 toggling `WindowFromPoint` mock 不现实;实测条件:macOS `WindowNumberAtPoint` 病态抖动 + 用户长按可见悬窗边界 > 5 秒,DWM 重绘触发 sub-window 闪烁的精确 50ms 时序窗口。该 bug 触发的可见现象:鼠标移出浮窗后,PinBottom 浮窗不 demote(一直盖着别的 app),用户需 mouse jiggle / 多动几次才会 drop。

---

### D6-003 — `is_floating_topmost_at` 在 main thread dispatch 闭包内裸 deref raw NSWindow ptr,跟 H2 fix(set_window_level)的安全策略不一致
**置信度**:低(目前不可触发,代码一致性 / footgun) **文件**:macos.rs:334 vs macos.rs:268-273 **触发条件**:目前 on_app 退出 / 用户手动关浮窗 / Tauri webview 销毁,`is_floating_topmost_at` 闭包在 `run_on_main_thread` 队列中等待 → webview 销毁 → raw ptr 失效 → 下一行 `window.windowNumber()` 触发 segfault。

**根因**:H2 fix(commit 721d016,2026-07-30 14:39)更新了 `set_window_level`,把裸 `&*ptr.cast::<NSWindow>()` 改成 `Retained::retain(ptr.cast::<NSWindow>())`。但**同一个 raw ptr 来源 + 同一个 main-thread dispatch 模式下**的 `is_floating_topmost_at`(macos.rs:329-334)**没被改**:
```rust
// macos.rs:328-334 (未改)
let win = app2.get_webview_window("floating")?;
let ptr = win.ns_window().ok()?;
if ptr.is_null() { return None; }
// SAFETY: ptr 来自 webview_window 的 NSWindow，整个 app 生命周期有效。
let window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
let our_id = window.windowNumber();   // ← ptr 在此失效会 segfault

// macos.rs:268-273 (H2 已修)
let window: Retained<NSWindow> = unsafe {
    match Retained::retain(ptr.cast::<NSWindow>()) {
        Some(w) => w,
        None => return,
    }
};
window.setLevel(level as _);
```

**影响**:**当前**因为 `is_floating_topmost_at` 整段读 ptr 在连续 3 行内(`let our_id = window.windowNumber();`、`NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(...)`、`Some(topmost == our_id)`),不像 `set_window_level` 那样 `Retained` outlives 多条语句的 `'win' drop`,所以现状安全。但:
1. 这是个**footgun** —— 任何后续维护者在 `let window: &NSWindow = ...` 后插入语句,都会暴露同一 H2 文档明确指出的 UAF 风险
2. H2 commit message 的论据("webview_window 在 dispatch 排队 → 执行期间并发销毁")在 `is_floating_topmost_at` 同样成立

**证据/调用链**:
```rust
// set_window_level (H2 已修)
app.run_on_main_thread(move || {
    if let Some(win) = app2.get_webview_window("floating") {  // ← win 在 if-let 块内
        if let Ok(ptr) = win.ns_window() {
            if !ptr.is_null() {
                let window: Retained<NSWindow> = unsafe { Retained::retain(...) ... };
                window.setLevel(level as _);                  // ← 多语句,win drop 后仍安全
                window.setHidesOnDeactivate(false);
                app2.emit(...);
            }
        }
    }  // ← win drops here,但 Retained 仍持有 NSWindow
});

// is_floating_topmost_at (H2 漏改)
app.run_on_main_thread(move || {
    let result = (|| -> Option<bool> {
        let win = app2.get_webview_window("floating")?;
        let ptr = win.ns_window().ok()?;
        if ptr.is_null() { return None; }
        let window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
        let our_id = window.windowNumber();                   // ← win 持有期间
        let topmost = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, mtm);
        Some(topmost == our_id)
    })();
    ...
});
```

**修复建议**:对齐 H2 fix,改 macos.rs:334:
```rust
let window: Retained<NSWindow> = unsafe {
    match Retained::retain(ptr.cast::<NSWindow>()) {
        Some(w) => w,
        None => return None,
    }
};
let our_id = window.windowNumber();
if our_id == 0 { return Some(false); }
let topmost = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, mtm);
Some(topmost == our_id)
```
并跟 set_window_level 一样加注释说明 `Retained` 防 UAF 的设计意图。

**待实测**:模拟"dispatch 排队 → webview 销毁"竞态不容易;直接靠 code review 保证。强烈建议跟 H2 走同款 `Retained::retain` 防 footgun。

---

## P3 (低)

### D6-004 — `set_window_level` 函数 doc comment 陈旧,描述与代码不一致
**置信度**:高(已确认) **文件**:macos.rs:252-255(doc comment)+ macos.rs:276(code) **触发条件**:无,纯文档问题。

**根因**:macos.rs:252-255 说:
```rust
/// M3 fix: 加 `is_pin_bottom` 参数。PinBottom 模式设 hidesOnDeactivate(false)
/// (否则鼠标一离开焦点窗口就消失);PinTop / Normal 走默认值(true)，
/// Normal 模式失焦时窗口应被隐藏(跟普通窗口一致)。
```
代码却 hardcode `setHidesOnDeactivate(false)`(line 276)。这是 H15 fix(commit d5612ab, 2026-07-03 17:56)有意的:**所有模式都 false,让浮窗始终可见** —— "macOS 普通窗口失焦只是被其他 app 遮盖(level=0 已实现该语义), 不是 hide()"。

inline 注释 `let _ = is_pin_bottom;` 标注 "保留参数兼容现有调用,语义不再依赖" 也明确说是参数兼容保留。但 `pub fn set_window_level(...is_pin_bottom: bool)` 的 **doc comment 没改**,导致:
- API caller 看到 `is_pin_bottom` 参数,以为有实际作用,可能误用它做条件分支
- 维护者看到 doc 会误以为有 bug

**影响**:无运行期 bug,但属于"代码讲一套、注释讲另一套"的 stale docs,认知负担。

**修复建议**:把 macos.rs:252-254 改成对齐 H15 行为:
```rust
/// 把浮窗的 NSWindow level 切到 `level`,dispatch 到 main thread(AppKit 强制要求)。
///
/// **H15 fix (2026-07-03)**: 所有模式都强制 setHidesOnDeactivate(false),让浮窗
/// 失焦时仍可见(macOS 普通窗口失焦只是被其他 app 遮盖, level 切换已实现该语义,
/// hide() 不属于"始终可见的用量悬浮窗"产品定义)。`is_pin_bottom` 参数保留仅为
/// 调用方签名兼容, 不参与运行时分支。
```

**待实测**:无,纯文档更新。

---

## 审过的路径(显式声明,确认无 issue)

### platform/windows.rs
- **`apply_z_order` 全局 Mutex 串行** (lines 149-152 + 194-199): mutex poison 恢复 (`unwrap_or_else(|e| e.into_inner())`) 跟其他模块对齐;RMW 锁同时覆盖路 A `SetWindowPos` + 路 B `SetWindowLongW`,last writer 决定 z-order + style bit,two-thread race 不再丢写。OK。
- **`GetWindowLongW` 失败/0 区分** (lines 218-223): `SetLastError(0) + GetLastError` 区分 0 返值 vs 真失败,避免 `0 | WS_EX_TOPMOST` wipe 掉 `WS_EX_LAYERED` 等已有 bit。OK。
- **`hwnd.is_null()` 防御** (lines 509-511 + 203-206): 销毁竞态显式 early return。OK。
- **`SetWindowPos(...SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE...)` flag 组合** (lines 248-256): 正确不抢焦点 + 不改几何 + 不改大小,只改 z-order。OK。
- **`ensure_per_monitor_v2_dpi`** (lines 119-123): 启动期一次性声明,失败降级不阻塞。OK。
- **hit_test_floating null check** (lines 547-549): `WindowFromPoint` 返 null → `None`,不返 false(CSS spring flicker bug M17 fix)。OK。
- **`POINT_IN_RECT` 半开区间** (line 569-571): `pt.x < rect.right` 半开,矩形边界 1px 不会重叠临界点。OK。
- **`HOVER_STATE_RESET` 顺序** (lines 268-283): `LEVEL_SWITCHING_ACTIVE.store` 在 `apply_z_order` dispatch 前;`HOVER_STATE_RESET.store` 在 dispatch 后 —— 注释明确"主线程队列里 raise 排在 demote 之后",demo 跟推断一致。OK。
- **dwell-time hysteresis Visible 1 tick / Covered 5 tick / Outside 3 tick** (lines 422-429): 1 档响应(Visible 快路径)vs 250ms dwell(Covered 慢路径)分级阈值正确,符合"两级命中"产品决定。OK。
- **`SHUTDOWN_NATIVE_THREADS` 检查** (lines 358-363): 50ms 内可退出,跟 D5-102 修复一致。OK。

### platform/macos.rs
- **`setHidesOnDeactivate(false)` 强制** (line 276): H15 fix 故意为之,所有模式不 hide 符合"始终可见悬浮窗"产品定义(参见 D6-004)。运行期 OK,只是 doc 陈旧。
- **`LEVEL_BELOW_NORMAL = kCGNormalWindowLevel - 1`** (line 43): 公开常量,只取 const,无运行时行为。OK。
- **`is_floating_topmost_at` slot + condvar 单槽复用** (lines 302-398): OnceLock<Arc<OneSlot>> 复用 1.7M/天调用,L12 fix 起;mutex poison 恢复 + dispatch 失败 fallback + 50ms 超时 + `take()` 消费旧值 全部到位。OK。
- **`is_menubar_hidden` slot + condvar** (lines 497-556): 同款单槽复用模式,fullscreen watcher 0.5Hz × 86,400s = 43K 次/24h,风格一致。OK。
- **`start_hover_emitter` SHUTDOWN 检查** (lines 162-169): D5-102 已加,50ms 内退出。OK。
- **`start_hover_emitter` 线程 spawn panic 降级** (lines 243-246): 不 .expect(),降级 log + reset `TRACKER_RUNNING` 让下次重启重试。OK。
- **`start_fullscreen_watcher` 线程 spawn panic 降级** (lines 484-487): 同款 pattern。OK。
- **`start_fullscreen_watcher` 主 mutex poison 恢复** (line 537): `.lock()` 不 expect(),`.into_inner()` 路径有(虽然是 polling 路径而不是关键路径)。OK。
- **`menu_bar_is_light` Main thread 检查** (lines 594-595): `MainThreadMarker::new()` 失败返 false(保守保留白字)。OK。
- **`menu_bar_is_light` Aqua / DarkAqual 判定** (lines 604-611): Apple 文档规定的 `NSAppearanceNameAqua`(完整字符串) + 后备 `"Aqua"` 简写 + `contains("Dark")` 三路覆盖。OK。
- **`NSEvent::mouseLocation` thread-safe** (line 181): 注释明示可在任意线程调,跟 Apple 文档一致。OK。
- **`set_window_level` 的 `Retained::retain`** (lines 268-273): H2 fix 防 dispatch 间 UAF,Retained drop = +1/-1 平衡。OK。
- **`HOVER_STATE_RESET` 顺序** (lines 70-80): 同 Win 模式(comment 解释 raise 排在 demote 之后)OK。
- **`windowNumber == 0` 兜底** (lines 336-339): 窗口还没上屏(初始化竞态)直接返 false,不 panic。OK。

### tray.rs
- **`mpsc::unbounded_channel + 进程内 sender` 设计** (lines 72-109): OnceLock<Mutex<Option<UnboundedSender>>> + `tray_request_tx()` 函数返回 `Sender`,Mutex poison 恢复(B-NEW-7 fix)对 `log`/`macos` 风格一致。OK。
- **tray 跨线程 SIGTRAP 根因修复** (comment lines 39-58): receiver 走 `tauri::async_runtime::spawn` long-lived task,每条消息 `run_on_main_thread` 派 closure,closure 拿 owned tray + 自然 drop(drop 在 main thread 跑 → 不会 SIGTRAP on AppKit)。OK。
- **`handle_tray_request` 失败的 receiver 路径** (lines 463-469): dispatch Err 不再 `break` 退出接收循环(之前 2026-06-20 audit 的回归),改成 log + continue,下一条请求还有重试机会。OK。
- **`LEFT_DOWN_UP_WINDOW_MS` 防 NSStatusItem 合成 click** (lines 125-136): 500ms 阈值远大于真用户 down→up(< 200ms),又能把陈旧 Down 视为失效。OK。
- **`include_bytes!` 嵌入 tray-base.png** (line 555): dev/prod 都可用,不依赖运行时路径(对比 CARGO_MANIFEST_DIR 已被 H7 fix 弃)。OK。
- **`render_icon` 分发 logic** (lines 587-610): Logo → placeholder;Bars/Percent + 无 MiniMax → placeholder;其余 → draw_mini_bars 或 draw_percent。 OK。
- **`draw_mini_bars` 像素越界防御** (line 761 `if px < 0 || py < 0 || px >= ICON_SIZE || py >= ICON_SIZE`): 边界外 continue,不 panic。OK。
- **`fit_scale` 缩比方向** (lines 870-883): 只缩小不放大(w > max_w 才进 ratio 分支,ratio < 1)。OK。
- **`draw_right_text` draw_text_mut 像素位置** (line 912): x 钳到 ≥ 1,不会负。OK。
- **`truncate_to_utf16_units` emoji surrogate pair 安全** (lines 988-1010): 按 char 边界(unit 边界对齐),`used + units > budget` 处 break,不会切坏 surrogate pair。OK。
- **`d5-001 fix` `instance_suffix` hidden base id 启发** (lines 1018-1029): `rfind('#')` 取最后 `#`(防用户 source 里有 `#`),tail=="1" 或空都视作 #1 不显示(主套餐)。OK。
- **`pick_minimax_rows` base id 匹配** (lines 619-648): `split('#').next() == "minimax"`,extra instance 不被过滤(H1 fix)。OK。
- **`row.kind` 枚举匹配 (M2)** (lines 651-668): 不依赖 label 字符串(locale 切换不破坏 util 查找)。OK。
- **`sanitize_percent` NaN/Infinity/越界归一** (lines 820-826 + 单测 1133-1152): M5 fix 全 audit 推广,统一 percent/bars/draw_percent 三处 sanitization。OK。
- **format_amount_short 大数字 k 简写** (lines 1107-1117): 100k+ 整数显示 `123k`,1k+ 一位小数显示 `1.2k`,其余两位小数;Tray tooltip 空间有限,合理。OK。
- **`is_*_rate_limit` 没有引用** (`is_menubar_hidden` dispatcher_Result 路径 line 535): 命名前缀 OK,无 dead code 残留。
- **Win NOTIFYICONDATAW.szTip 128 UTF-16 unit 截断** (lines 974-983): M4 fix,按 char 边界 + 拼 `…`(U+2026,1 unit)。OK。

---

## 优先级建议

1. **立即修**(回归风险/可触发):D6-001(D5-102 同款三线程修复漏一个)
2. **v0.3 一并修**(代码一致性/footgun):D6-002,D6-003
3. **机会修**:D6-004(纯文档)

本轮未发现 P0(critical)级别 bug。
