# Musage 项目全量代码审查 — 总报告

**审查日期**: 2026-07-29
**项目规模**: ~22,675 行 Rust + ~8,584 行 TypeScript
**审查方式**: 8 个并行审查 agent,每域独立通读 + 写报告
**审查员覆盖**:
- 域 1: 13 个 provider API 实现 (Darwin)
- 域 2: 配置 / 凭证持久化 + IPC commands (Bohr)
- 域 3: 后台轮询 / 退避 / Task 生命周期 (Wegener)
- 域 4: 3 个一键登录模块 (Sartre)
- 域 5: 平台特定代码 (Beauvoir)
- 域 6: 托盘 + 动态图标渲染 (Newton)
- 域 7: 前端核心 main.ts + settings/ (Epicurus A)
- 域 8: i18n / 日志 / parse / 安全 / Tauri 配置 (Epicurus B)

---

## 1. 全局概览

| 域 | CRITICAL | HIGH | MEDIUM | LOW | 合计 | 健康度 |
|---|---|---|---|---|---|---|
| 1. Provider API | 0 | 3 | 3 | 0 | 6 | 7.5/10 |
| 2. Config + IPC | 0 | 4 | 15 | 10 | 29 | 7/10 |
| 3. Poller / 退避 | 0 | 5 | 6 | 7 | 18 | B+ (缺生命周期收尾 + 调度策略) |
| 4. 一键登录 | 0 | 2 | 4 | 6 | 12 | 7.5/10 (三轮实测较扎实) |
| 5. Platform (Win/macOS) | 0 | 4 | 8 | 9 | 21 | 7/10 (多线程 + 跨平台 + 资源生命周期) |
| 6. 托盘 / 图标 | 0 | 2 | 5 | 6 | 13 | 6.5/10 (Percent 文本溢出 + 亮色主题) |
| 7. 前端核心 | **1** | 5 | 4 | 2 | 12 | 7/10 (XSS 防御成熟, 事件/资源生命周期是短板) |
| 8. 安全 + 配置 | 0 | 4 | 3 | 2 | 9 | 8/10 (Rust 端辅助层较扎实, capability / 配置层是软肋) |
| **总计** | **1** | **29** | **48** | **42** | **120** | **综合 7/10** |

### 关键观察

- **CRITICAL 仅 1 个,且都集中在前端** (浮窗 init 错误回退点击死锁)。后端 Rust 端 0 CRITICAL,与最近 4 轮审查 (2026-06-20 / 2026-07-02 / 2026-07-06 / 2026-07-28) 已修掉 critical 级硬伤的演进趋势一致
- **HIGH 29 个分布广**:登录模块并发竞态、provider 鉴权分类、poller 零 jitter、托盘文本裁切、Windows 线程 race、logstore 敏感数据、前端 capabilities URL 白名单、IPC 校验缺失、JS init script 无单测等
- **趋势**:**事件 / 资源生命周期**和**调度策略**是新一批问题的主要来源,不是 panic / 死锁 / schema 失效这种硬伤

---

## 2. CRITICAL 级 (1 个)

### 🔴 A-C1 — 浮窗 init 失败时 `app.innerHTML` 注入 + 事件委托残留导致 IPC 不可达
- **域**:7 前端核心
- **位置**:`src/main.ts:1469`、`:1485`
- **类型**:前端可用性
- **问题**:`get_snapshot` / `refresh_now` IPC 抛出时,`app.innerHTML` 把 `<button class="err-btn open-settings">` 注入。但 click handler 在 init() 同步 try/catch 之后注册 → catch 早 return → handler 没注册,open-settings 按钮**死锁,用户点不开设置面板**。
- **触发**:首启 / 密钥损坏 / Rust 端 emit 还没建立 → get_snapshot 抛 → catch 执行 innerHTML 注入 → 用户看红卡 + open-settings 按钮 → 点击没反应。
- **影响**:**首启可用性**,易复现。
- **修法**:`init()` 改成 `try { 注册 listener; await invoke; render } catch (放在 listener 注册之后)`。

---

## 3. HIGH 级 (29 个,按域分组)

### 3.1 域 1 Provider API (3)
- **BUG-001** StepFun / AnySearch 单次轮换 RefreshToken 缺少并发串行化
  - 位置:`providers/stepfun.rs:252`、`providers/anysearch.rs:323`、`commands/mod.rs:1575`、`commands/extra_instances.rs:636`
  - 类型:并发竞态 / 凭据状态损坏。两个任务若同时读到同一旧 pair,都会进入主动续期,后完成的旧分支可能覆盖新 pair 写回 keys.json → 用户随机看到"凭据失效"且 keys.json 最终保存的 pair 不再是服务端当前有效 pair。
  - 修法:建立按 `unique_id` 分片的 `tokio::sync::Mutex`,网络续期和写回必须处于同一个实例级临界区内。
- **BUG-002** MiniMax 将 `status=2/3 + remaining_percent=0` 错当成额度耗尽
  - 位置:`providers/minimax.rs:545`
  - 类型:Schema 语义错误。`status != 1 && remaining_percent == 0` 不应绕过 percent schema 有效门。
  - 修法:严格以 `status == 1` 作为 percent schema 的有效门。
- **BUG-003** Xiaomi 将部分真实 401 错误分类成 ServerError
  - 位置:`providers/xiaomi.rs:500`
  - 类型:鉴权边界。HTTP 401 分类依赖响应正文是否包含三个英文关键词(login/session/token)。真实鉴权失败响应完全可能为空、为中文 → 被归为 ServerError,前端不会展示重新登录入口。
  - 修法:401 默认归为 AuthFailed。

### 3.2 域 2 Config + IPC (4)
- **H1** `cleanup_orphan_tmp_files` 会静默删除用户在 cfg 目录下的 `*.tmp` 文件
  - 位置:`config.rs:935-955`
  - 类型:文件所有权 / 数据丢失。`~/.config/com.musage.app/` 是普通用户目录,用户完全可能自己放 `download.tmp` / `database.tmp`,启动时会被静默删除 — 没有日期过滤。
  - 修法:改成只清理自己产生的 `*.json.tmp` / `*.jsonl.tmp` 或限定为四个固定文件名。
- **H2** `save_config` 接受前端传的 `cfg.providers` map 任意 key,几乎没校验
  - 位置:`commands/mod.rs:586-690`
  - 类型:IPC 边界 / 输入校验缺失。key 可任意字符串、`floating_x/y` 可设成 `i32::MIN`、`provider_order` 无大小校验。
  - 修法:key 白名单过滤 + `provider_order` max_len=128 + `floating_x/y` range check `-32768..=32767`。
- **H3** `delete_source_credential` 级联 disable 副本落盘失败,但 key 已删 → 永久不一致
  - 位置:`commands/mod.rs:862-933`
  - 类型:状态机一致性。cascade 落盘失败 + key 已删 = disk 与 in-memory 永久错位,下次启动 visible bug。
  - 修法:cfg.save 失败时 abort 整个 cascade(保留旧 key)。
- **H4** `set_source_credential` / `save_config` 不限制 value 长度,IPC 边界无 size cap (DoS)
  - 位置:`commands/mod.rs:726-741`、`:586`
  - 修法:handler 入口 `value.chars().take(8 * 1024)`;`provider_order` / `providers` map 加 max entries 截断。

### 3.3 域 3 Poller / 退避 (5)
- **H1** 主循环永不退出 + 全工程零 graceful shutdown 路径
  - 位置:`poller.rs:111-262` + `commands/mod.rs:1078-1080`
  - 修法:新增 `src-tauri/src/shutdown.rs` 用 `tokio::sync::Notify`,主循环改 `tokio::select!`,`quit_app` 改两步:先 `notify_waiters` + drain JoinSet,再 `app.exit(0)`。
- **H2** 12+ provider 主循环零 jitter → thundering herd
  - 位置:`poller.rs:89-107` + `:111`
  - 修法:初始 deadline 加 0..interval_secs 均匀抖动,主循环的 `sleep(1s)` 也加 0-100ms 抖动。
- **H3** `refresh_inner` 内部 12 个 `tokio::spawn` 同步触发,fan-out 无界
  - 位置:`commands/mod.rs:1340-1377`
  - 修法:`buffer_unordered(4)` 限制并发为 4。
- **H4** `BackoffState` 内存态,App 重启退避历史全丢(v0.3 待做项)
  - 位置:`poller_backoff.rs` 整个文件 + `lib.rs:81-89`
  - 修法:`PersistedBackoff { entries, saved_at_unix }` + `save_to_disk` / `load_from_disk`,`record()` 末尾 spawn debounce task。
- **H5** 手动「立即刷新」失败时,backoff streak 被多算
  - 位置:`commands/mod.rs:1430-1450`
  - 修法:`BackoffState::record` 加 caller 区分(Poller / ManualOverride)。

### 3.4 域 4 一键登录 (2)
- **H1** `EXTRACTING.store(false)` 在 gen 检查之前无条件清锁 — 重复 emit + 双写
  - 位置:`xiaomi_login.rs:255-264`
  - 类型:并发(race + duplicate emit)。7d21fcb 给 `ExtractingGuard::Drop` 加了 gen 检查,但遗留显式 `EXTRACTING.store(false)` 不带 gen 检查,跑在 guard 之前 — 等于绕过刚加的 guard 兜底。
  - 修法:删除 line 258 的显式 store,靠 guard 的 Drop 清锁。
- **H2** `cookies_for_url` Err 立刻返 Cancelled — 启动期瞬时错误不可恢复
  - 位置:`stepfun_login.rs:255-260`、`anysearch_login.rs:412-417`
  - 修法:Err 不要直接 Cancelled,先连续 N 次重试(5 次 × 700ms = 3.5s),再 fallback。

### 3.5 域 5 Platform (4)
- **H1** Windows: `apply_z_order` 在 worker 线程修改窗口 style, 与 main thread WndProc 存在 TOCTOU 竞态
  - 位置:`platform/windows.rs:171-220`
  - 影响:PinBottom → PinTop 切换时,主线程 dispatch 跟 emitter 50ms tick 撞车,理论概率 1/40。v0.2.4 已知 3/7 命中率可能跟这条有关。
  - 修法:把 `apply_z_order` 整个 dispatch 到 main thread,或加 `std::sync::Mutex<()>` 串行化两个 write path。
- **H2** macOS: NSWindow 裸指针在 raw closure 中 dereference, 存在窗口销毁竞态 UAF
  - 位置:`platform/macos.rs:248-270`
  - 修法:用 `objc2::rc::Retained<NSWindow>` 包一层 retain。
- **H3** logstore: 错误消息明文落盘, 可能写入 API key / Cookie (**跨域问题:5+6+8**)
  - 位置:`logstore.rs:230-252` + 全文件调用者
  - 修法:在 `LogEntry::error/warn/info` 构造器里加 `redact()` 步骤,匹配 `Bearer ` / `sk-` / `eyJ` (JWT 头) / `Oasis-Token=` / `MUSAGE_TOKEN=` 等模式做正则替换。
- **H4** logstore: tmp 文件 rename 失败 → 残留孤儿文件
  - 位置:`logstore.rs:329-343`
  - 修法:在 `logstore::load_from_disk` 启动时清一次 `.jsonl.tmp`。

### 3.6 域 6 托盘 / 图标 (2)
- **H1** 百分比文本在常用数值下必然被裁切
  - 位置:`tray.rs:814`
  - 影响:`99%` 已触碰右边界,`100%` 会裁掉约 12px,通常连 `%` 都显示不完整。100% 上限状态尤其严重。
  - 修法:动态缩小字号直到 `width <= ICON_SIZE - 2 * padding`;两行按真实高度垂直居中。
- **H2** 动态图标不适配亮色主题,默认 Percent 可能近乎不可见
  - 位置:`tray.rs:223`、`:693`、`:820`
  - 修法:macOS 使用 template-compatible alpha mask + `set_icon_with_as_template(..., true)` 原子更新;Windows/Linux 给白色文字和填充加 1-2px 深色描边。

### 3.7 域 7+8 前端核心 + 安全 (9)
- **A-H1** order.ts 拖拽监听器在 mouseup 抛错时泄漏(绑 document 级)
  - 位置:`settings/order.ts:261-360`
  - 修法:`try/finally { removeEventListener }`。
- **A-H2** main.ts 浮窗 `app.addEventListener("mousedown"/"dblclick"/"click")` 在 beforeunload 不卸载 → dev HMR / 重渲累积
  - 位置:`main.ts:1266-1287`、`1474-1535`
  - 修法:把 mousedown/dblclick/click 也存到 `unlistenX` 变量(或返回 AbortController)。
- **A-H3** `renderEmptyState()` 用 `app.innerHTML` 全量覆盖,与增量 render 互斥
  - 位置:`main.ts:653-664`
  - 修法:renderEmptyState 也用增量模式。
- **A-H4** `setupHoverRaise()` 在 body 上 addEventListener,初始状态 + 重入不幂等
  - 位置:`main.ts:1608-1624`
  - 修法:加 `if (hoverHandlerInstalled) return;` 守卫。
- **A-H5** `formatResetWithCountdown` 未防御 Date 上限
  - 位置:`main.ts:1112-1116`、`1181-1208`
  - 修法:`if (!Number.isFinite(ms) || ms < 8.64e15)` 双阈值检查。
- **B-H1** `capabilities/{xiaomi,anysearch,stepfun}-login.json` 共享 `core:webview:allow-create-webview-window` 权限,任一被劫持就可能跨域(**跨域问题:4+8**)
  - 位置:`capabilities/{xiaomi,anysearch,stepfun}-login.json:7`
  - 影响:DNS 污染 / hosts 文件被改 / 同网络 MITM → 登录 window 弹的是 attacker 控制页 → Set-Cookie 塞 `api-platform_serviceToken=...` → 写到 keys.json → attacker 用 cookie 登录。
  - 修法:Tauri 2 `core:webview:allow-create-webview-window-with-specific-urls` + URL 白名单。
- **B-H2** `tauri.conf.json` CSP 含 `style-src 'unsafe-inline'` 但代码里有 inline `style="..."` 调用
  - 位置:`tauri.conf.json:35`、`settings/extra-instance-form.ts:104`、`settings/floating.ts:301-345`
  - 修法:CSP 去掉 `'unsafe-inline'`,改用 nonce / hash。
- **B-H3** `logstore.rs::load_from_disk` 不强制 0600,旧用户文件权限泄漏
  - 位置:`logstore.rs:108-130`
  - 修法:`load_from_disk` 开头加 `set_permissions(path, 0o600)`。
- **B-H4** settings.html / index.html CSP meta 缺失 + dev 模式无 CSP 强制
  - 位置:`tauri.conf.json:35`、`index.html`、`settings.html`
  - 修法:加 `<meta http-equiv="Content-Security-Policy">`,或配 `app.security.devCsp` 字段。

---

## 4. 跨域关联(同一根因出现在多个域)

### 4.1 Capabilities URL 白名单缺失(B-7+8 → 4)
- 域 4 一键登录的 `extract_and_save` 不验证 cookie 来源域
- 域 8 capabilities 给 `core:webview:allow-create-webview-window` 但不限定 URL
- 联合影响:任何登录 webview 被劫持都会把恶意 cookie 落 keys.json
- **统一修法**:所有 `*-login.json` 加 `core:webview:allow-create-webview-window-with-specific-urls` + URL 白名单

### 4.2 logstore 敏感字段(B-5 → B-8)
- 域 5 报告 H3:logstore message 字段含 API key / Cookie 明文
- 域 8 报告 B-H3:logstore load_from_disk 不强制 0600,旧用户文件权限泄漏
- **统一修法**:`LogEntry::error/warn/info` 构造器加 `redact()` 正则替换 + `load_from_disk` 开头加 `set_permissions(path, 0o600)`

### 4.3 RefreshToken 续期并发竞态(域 1)
- stepfun + anysearch 都用单次轮换 refresh token,但没有按 unique_id 加异步锁
- poller 全量刷新 / save_config 触发刷新 / 单 provider 刷新 / extra_instances 验证 4 个独立入口都可并发调用 fetch
- **统一修法**:全局抽 `tokio::sync::Mutex<HashMap<String, Mutex>>`,续期流程持锁

### 4.4 浮窗状态同步(域 3 + 域 7)
- 域 3 M3:用户改 interval 后立即 fire 也可能撞 backoff cap(`last_intervals` 与 `entry` 不同步)
- 域 7 A-H3:`renderEmptyState` 全量 innerHTML 覆盖与增量 render 互斥
- 域 7 A-H5:`formatResetWithCountdown` 未防御 Date 上限
- **统一关注点**:浮窗显示数据的所有路径都得能正确表达 "未配置 / 加载中 / 错误 / 数据" 四种状态,且边界值不出 NaN / 负百分比

---

## 5. 推荐修复优先级

### P0 (立刻修,影响数据完整性 / 用户立即可见 bug)
1. **A-C1** 浮窗 init 错误回退点击死锁 (前端) — 5 行 diff,影响首启可用性
2. **H3** logstore 敏感字段落盘 (域 5) — 安全问题,加 redact 正则
3. **B-H1** capabilities URL 白名单 (域 8) — 真实 attack surface
4. **H1** Windows apply_z_order 线程 race (域 5) — 影响 v0.2.4 已知 3/7 命中率

### P1 (下次 release 修,影响常用场景)
5. **BUG-001** StepFun/AnySearch RefreshToken 并发串行化 (域 1) — 数据完整性
6. **BUG-002** MiniMax status=2/3 错当成额度耗尽 (域 1) — schema 语义
7. **BUG-003** Xiaomi 401 误分类 (域 1) — 鉴权边界
8. **H1** Poller graceful shutdown (域 3) — 数据完整性
9. **H2/H3** Poller jitter / buffer_unordered (域 3) — 中转站风控 / UX
10. **H1** tray.rs Percent 文本裁切 (域 6) — 100% 显示残缺
11. **H2** tray.rs 亮色主题 (域 6) — macOS 亮色菜单栏白字消失
12. **H1/H2** login: EXTRACTING 清锁 + cookies_for_url Err 重试 (域 4) — 立即生效
13. **H1** Config cleanup_orphan_tmp_files (域 2) — 用户文件被删
14. **H3** Config delete_source_credential 级联落盘失败 (域 2) — 永久不一致
15. **H4** Config IPC size cap (域 2) — DoS 面

### P2 (v0.3 hotfix,影响体验 / 性能)
16. **H4** Poller BackoffState 持久化 (v0.3 待做项)
17. **H5** Poller manual vs poller 区分 (域 3)
18. **H2** macOS NSWindow UAF (域 5)
19. **H4** logstore tmp 孤儿文件 (域 5)
20. **A-H1/H2** 前端拖拽监听器 / 浮窗 listener 泄漏 (域 7) — dev HMR 累积
21. **A-H4** setupHoverRaise 不幂等 (域 7) — dev HMR 累积
22. **M1** tray.rs poller 先启动竞态 (域 6)
23. **M2** tray.rs 无界队列携带完整 Snapshot (域 6) — 内存峰值
24. **B-H3** logstore load_from_disk 不强制 0600 (域 8)
25. **B-H4** CSP meta + dev 模式 (域 8)

### P3 (v0.3 tech debt,不影响 release)
- 域 2 M1/M2/M3/M4/M5/M6/M7/M8/M9/M10/M11/M12/M13/M14/M15 (15 项)
- 域 3 M1/M2/M4/M5/M6 + L1/L2/L3/L4/L5/L6/L7/L8 (13 项)
- 域 4 M1/M2/M3/M4 + L1/L2/L3/L4/L5/L6 (10 项)
- 域 5 M1/M2/M3/M4/M5/M6/M7/M8 + L1-L9 (17 项)
- 域 6 M3/M4/M5 + L1-L6 (11 项)
- 域 7 A-M1/M2/M3/M4 + A-L1/L2 (6 项)
- 域 8 B-M1/M2/M3 + B-L1/L2 (5 项)

---

## 6. 整体健康度评分

| 域 | 评分 | 评语 |
|---|---|---|
| 1. Provider API | 7.5/10 | schema 双版本兼容做得好,但 401 分类 + RefreshToken 并发有提升空间 |
| 2. Config + IPC | 7/10 | 2026-06-20 死锁修复后稳定,但 IPC 边界校验缺失是新增问题 |
| 3. Poller | B+ | 核心架构 panic 隔离扎实,缺生命周期收尾 + 调度策略 |
| 4. 一键登录 | 7.5/10 | stepfun 三轮实测整体方向正确,剩余风险在并发骨架 |
| 5. Platform | 7/10 | 多线程 + 跨平台 + 资源生命周期层面 issue,无"代码不动就 crash"那种炸弹 |
| 6. 托盘 | 6.5/10 | Percent 文本溢出 + 亮色主题支持是明显短板 |
| 7. 前端核心 | 7/10 | XSS 防御成熟,事件/资源生命周期是短板 |
| 8. 安全 + 配置 | 8/10 | Rust 端辅助层扎实,capability / 配置层是软肋 |
| **综合** | **7/10** | 无 CRITICAL 级炸弹,但 HIGH 29 个跨域分布广,**事件生命周期**和**调度策略**是下一阶段重点 |

---

## 7. 子报告索引

| 文件 | 域 |
|---|---|
| `audit-reports/01-provider-api.md` | 域 1:13 个 provider API 实现 |
| `audit-reports/02-config-ipc.md` | 域 2:配置 / 凭证持久化 + IPC commands |
| `audit-reports/03-poller.md` | 域 3:后台轮询 / 退避 / Task 生命周期 |
| `audit-reports/04-login-modules.md` | 域 4:3 个一键登录模块 |
| `audit-reports/05-platform.md` | 域 5:平台特定代码 (windows.rs + macos.rs) |
| `audit-reports/06-tray.md` | 域 6:托盘 + 动态图标渲染 |
| `audit-reports/78-frontend-security.md` | 域 7+8:前端核心 + i18n/日志/parse/安全/Tauri 配置 |

---

## 8. 总结

**正面**:
- 4 轮历史审查已把 critical 级硬伤(死锁 / schema 失效 / 升级面板 / CSP / 死代码)修干净
- 后端 Rust 端 0 CRITICAL,核心架构 panic 隔离、双 tick 防重、Backoff 单调不递减、shared_client 防连接泄漏都已落地扎实
- 跨域并行:StepFun / AnySearch / Xiaomi 三个登录模块的差异化设计走通
- Provider 多实例实现整体一致(`instance_index` / `unique_id` / `with_instance_index`)
- 前端 XSS 防御成熟,escapeHtml 5 字符全覆盖

**空白**:
- **任务生命周期完全没 graceful shutdown** (域 3 H1)
- **零 jitter / 无并发上限** (域 3 H2/H3)
- **退避状态无持久化** (域 3 H4)
- **手动刷新跟 poller 退避语义混淆** (域 3 H5)
- **前端事件/资源生命周期管理是软肋** (域 7 A-C1 + A-H1/H2/H4)
- **capability URL 白名单 + CSP meta 缺位** (域 8 B-H1/B-H4)
- **logstore 敏感字段 + 文件权限 + 跨平台资源泄漏** (域 5 H3/H4 + 域 8 B-H3)
- **托盘 Percent 文本溢出 + 亮色主题支持** (域 6 H1/H2)

**建议节奏**:
- v0.2.5 hotfix:15 个 P0/P1 项中低成本高价值的部分(A-C1 + H1 域 5 + B-H1 capabilities URL + H1 tray 文本 + 域 4 H1/H2)
- v0.3:剩下 P1 + P2 + 历史 v0.3 待做项(poller 持久化 / monitor hotplug / 错误卡"忽略"按钮 / 前端单测 4 核心函数等)
