# Musage 5/8 轮审计报告：poller / refresh 调用链 / JoinSet / 事件 / 启动退出

审计范围：`poller.rs` / `poller_backoff.rs` / `refresh_inner` / `refresh_single_inner` / `refresh_now` / `tick` / `tick_now` / `quit_app` / 启动顺序 / 平台层后台线程 / 浮窗几何 persister。

调用图与锁顺序均已对照源码逐行确认（HEAD `9cdbb1c`）。本轮未触碰文件，未跑旧 audit 的结论。

---

## P1 — 高

### D5-084 `refresh_inner` 全量刷新走 `Poller` 退避，`H5 fix` 半成品

- 置信度：高
- 文件:行号
  - [`src-tauri/src/commands/mod.rs:1427`](src-tauri/src/commands/mod.rs:1427)
  - [`src-tauri/src/commands/mod.rs:1448`](src-tauri/src/commands/mod.rs:1448)
  - [`src-tauri/src/commands/mod.rs:1471`](src-tauri/src/commands/mod.rs:1471)
  - 调用方：[`src-tauri/src/poller.rs:352`](src-tauri/src/poller.rs:352)（`tick`），[`src-tauri/src/commands/mod.rs:546`](src-tauri/src/commands/mod.rs:546)（`refresh_now`）
- 触发时序：用户点托盘菜单「立即刷新」/前端 `refresh_now` IPC → `tick_now` → `tick` → `refresh_inner`；或前端 `refresh_now` → `refresh_inner`。两条全量刷新路径均走到硬编码的 `RefreshSource::Poller`。
- 根因：commit `331c59b`（H5 fix）给 `BackoffState::record` 和 `refresh_single_inner` 加上 `caller: RefreshSource` 参数，意图是「手动失败 no-op、自动失败退避」。但 `refresh_inner` 三处 `record` 调用（1427/1448/1471）仍硬编码 `Poller`，没把 caller 透传进去。同文件 `refresh_now`（546）和 `poller::tick`（352）也没给 `refresh_inner` 传 caller。
- 用户影响：用户主动「立即刷新」遇到 `RateLimited` / `ServerError` / `Network` 时，本应 no-op 的失败把 `current_interval_secs` 翻倍。反复手动失败 3-4 次后，`BackoffState` 撞 `MAX_BACKOFF_SECS = 1800s`，poller 下一次自动拉取要等 30 分钟。视觉表现：「手动刷了几次都没成功，软件看起来坏了，浮窗 30 分钟不更新」。
- 证据链：
  - `tick_now` 在 [`poller.rs:334`](src-tauri/src/poller.rs:334) 注释明确写「供 tray 菜单和 commands::refresh_now 调用」，tray 菜单路径在 [`tray.rs:253`](src-tauri/src/tray.rs:253)。
  - `refresh_now` 在 [`commands/mod.rs:536-544`](src-tauri/src/commands/mod.rs:536) 的注释明确写「跟 poller::tick 共用全量刷新互斥位」，是用户主动 IPC 入口。
  - `poller_backoff.rs:103-105` 的 `Manual` 短路逻辑只在 `refresh_single_inner` 路径生效，全量路径绕过。
- 最小修复：
  1. `refresh_inner` 签名加 `caller: RefreshSource`，三处 `record` 改用传入值。
  2. `tick` 加 `caller` 参数，`poller::start` 初始 tick 传 `Poller`（或保留默认），`tick_now` 传 `Manual`。
  3. `refresh_now` 调 `refresh_inner` 时传 `Manual`。
  4. 加单测：`refresh_now` 失败后 `BackoffState.next_interval_secs` 不变。

---

## P2 — 中

### D5-007 `refresh_inner` 每轮 12 次顺序拿 `backoff.write` 锁

- 置信度：高
- 文件:行号
  - [`src-tauri/src/commands/mod.rs:1424-1428`](src-tauri/src/commands/mod.rs:1424)（成功分支）
  - [`src-tauri/src/commands/mod.rs:1446-1449`](src-tauri/src/commands/mod.rs:1446)（业务错误分支）
  - [`src-tauri/src/commands/mod.rs:1469-1472`](src-tauri/src/commands/mod.rs:1469)（join 错误分支）
- 触发时序：全量 tick 收集完 12 个 `JoinHandle`，for 循环逐 await 后逐条拿 `state.backoff.write().await`。`tokio::sync::RwLock` 默认 write-prefer，单次 record 期间排他，循环里 12 次 write 串行排队。
- 根因：`refresh_inner` 把 record 调用写在结果收集的 for 循环里，每条 provider 一锁。`fill_next_fetch_at` 在 record 之后只拿 read 锁、不嵌套，所以本身没死锁，但 12 次 write 串行。
- 用户影响：常态不可见（record 本身 O(1)），但 `fill_next_fetch_at` 拿 read 锁和 per-provider 路径同时抢时（`refresh_single_inner:1669`），poller 主循环 `backoff_snapshot` 的 read 也可能插队，造成单次全量刷新多花几毫秒—几十毫秒。无功能 bug，但和 `M5 fix`（L172-180 注释「spawn 抢 write 锁 1s+」）的设计意图冲突。
- 证据链：`poller.rs:172-180` 注释明确说「backoff write 锁不能跨 for 持有」，但 `refresh_inner` 的 for 循环里是「每条一锁」而非「整批一锁」。
- 最小修复：在 for 循环里 `Vec<(String, ProviderSnapshot, u64)>` 收集 `(id, snap, default_interval)`，循环结束后 `let mut backoff = state.backoff.write().await;` 一次、`for (id, s, di) in batch { backoff.record(...); }` 批量写。

### D5-033 `last_intervals` HashMap 不跟 `next_fetch` / `backoff` 一起 retain

- 置信度：高
- 文件:行号
  - 初始化：[`src-tauri/src/poller.rs:119`](src-tauri/src/poller.rs:119)
  - 写入：[`src-tauri/src/poller.rs:134`](src-tauri/src/poller.rs:134)、[`src-tauri/src/poller.rs:273-274`](src-tauri/src/poller.rs:273)
  - **缺失**清理：对比 [`src-tauri/src/poller.rs:229-231`](src-tauri/src/poller.rs:229)（`next_fetch.retain`）和 [`src-tauri/src/poller.rs:234-237`](src-tauri/src/poller.rs:234)（`backoff.retain_live`），`last_intervals` 没有对应的 retain。
- 触发时序：用户 `add_extra_instance` 创建一个「minimax#2」→ `delete_extra_instance` 删它。`next_fetch` 在下一 tick 自动清掉（229），`backoff` 也清掉（236），但 `last_intervals["minimax#2"]` 永久残留。
- 根因：refactor 时加了 `next_fetch` 和 `backoff` 的清理，但 `last_intervals`（L119 注释「P8 fix」）漏了。M22 那一轮审计只盯了 `in_flight` Mutex，没碰 poller 主循环的 HashMap 集合。
- 用户影响：单进程内存慢泄漏。`add+delete` 循环 1000 次后 `last_intervals` 涨到 ~16KB（每条 16 字节 String + 8 字节 u64 + HashMap overhead）。bounded by 13+N builtin，N 上限 256（`PROVIDERS_MAP_MAX`），所以最坏也就几十 KB，不会 OOM。但属于明显「refactor 半完成」的可证 bug。
- 最小修复：在 `poller.rs:229-231` 的 `next_fetch.retain` 旁边加一行 `last_intervals.retain(|k, _| live_sources.contains(k));`，跟 `backoff.retain_live` 对称。

---

## P3 — 低

### D5-038 `tick` 与 `refresh_now` 重复的「合并 snapshot」逻辑

- 置信度：高
- 文件:行号
  - [`src-tauri/src/poller.rs:362-385`](src-tauri/src/poller.rs:362)（`tick` 的合并段）
  - [`src-tauri/src/commands/mod.rs:550-568`](src-tauri/src/commands/mod.rs:550)（`refresh_now` 的合并段）
- 触发时序：两处都做「按 `snapshot_key` 找匹配 → 替换或 push → 同步顶层字段 → 二次 emit」。`P3 fix` 注释（367-369、553-555）已经强调「合并键统一为 `snapshot_key`」，但合并代码本身是复制粘贴。
- 根因：`refresh_inner` 把 emit 提前到了内部（1516），调用方拿不到 emit 前的合并窗口，所以两处都重复「再 emit 一次合并后版本」。要么 `refresh_inner` 不 emit 让调用方 emit，要么抽个 `merge_snapshots(state, new_snap) -> QuotaSnapshot` 共享函数。
- 用户影响：无功能 bug，但下一次「合并规则改动」必须改两处，已有先例（`P3 fix` 注释说「之前只按 source_id 匹配」漏改一处）。
- 最小修复：抽 `fn merge_into_state(state, new_snap)` 到 `commands/mod.rs`，tick 和 refresh_now 都调它。

### D5-048 每 tick 每 source 一次 `unique.clone()` 给 `entry()`

- 置信度：高
- 文件:行号：[`src-tauri/src/poller.rs:290`](src-tauri/src/poller.rs:290)（`next_fetch.entry(unique.clone()).or_insert(now)`）
- 触发时序：每秒 1 次主循环 × 12+ source = 12+ String 分配/秒，仅为了 `HashMap::entry` 的 key。
- 根因：`unique` 在循环体里被多次借用（`last_intervals.get(unique_str)`、`cfg.providers.get(unique_str)`、`cfg.providers.get(base_str)`、`jitter_for(unique.as_str(), ...)`），要保留到 spawn 后，所以 clone 一次是合理的。但可以改成 lookup_or_insert_with 闭包。
- 用户影响：13 个 source 每秒约 13 × (24 字节 String alloc + 8 字节 ptr) ≈ 400 字节/秒的 alloc churn。完全可忽略。属「未来加大量 custom source 时会放大」的隐患。
- 最小修复：改成 `let key = unique.clone(); next_fetch.entry(key.clone()).or_insert(now);` 然后循环内只借用 `&key`，或直接 `if !next_fetch.contains_key(unique_str) { next_fetch.insert(unique.clone(), now); } let entry = next_fetch.get_mut(unique_str).unwrap();` 省一次 clone。

### D5-066 `BackoffState::record` 成功路径会无意义创建 entry

- 置信度：高
- 文件:行号：[`src-tauri/src/poller_backoff.rs:91`](src-tauri/src/poller_backoff.rs:91)
- 触发时序：12 个 source × 60s 一次 = 每分钟 12 次 `record(success)`。每次都 `entry(id).or_default()` 创建 entry（即便后续不修改），`per_source` HashMap 永久塞 12 条「streak=0, interval=None」的死 entry。
- 根因：line 91 早 create 晚判断，line 95 才检查「streak=0 && interval=None 时不动」。`backoff_idle_success_does_not_touch_state` 测试（330-339 行）只检查「streak/interval 字段」，没检查「entry 是否存在」。
- 用户影响：`BackoffState.per_source` 永久保留 12+N 条死 entry（N=extra instance 数）。每条 24 字节 String + ~16 字节 SourceBackoff = 40 字节 × 256 上限 ≈ 10KB。bounded 内存浪费。
- 最小修复：把 line 91 的 `entry().or_default()` 挪到真正要写的时候（line 95 之前先 `match self.per_source.get(id) { None => { return; } Some(e) => { ... } }`），成功无修改直接 return。

### D5-073 `refresh_inner` 每个 source 重新序列化整个 `AppConfig` 给 `set_state`

- 置信度：中
- 文件:行号：[`src-tauri/src/commands/mod.rs:1364-1371`](src-tauri/src/commands/mod.rs:1364)
- 触发时序：全量 tick 循环里 12 次 `update_source_state(&src_box, cfg)`，每次都 `serde_json::to_value(cfg)`（1764-1770）。`AppConfig` 含 `providers: BTreeMap<String, ProviderConfig>`（12+ 条）和 `color_overrides`、`provider_order` 等。
- 根因：`update_source_state` 设计成「让 source 决定它要不要 state」（`needs_state_update()`），但序列化是 caller 做。deepseek / kimi / claude_official 返回 `false` 跳过，但 Xiaomi / StepFun / Tavily 都要 state。
- 用户影响：每分钟多序列化 12 次 ≈ 1-2KB JSON × 12 ≈ 15KB alloc/min。`needs_state_update` 已经挡掉无状态 source，影响小。
- 最小修复：在循环外 `let cfg_json = serde_json::to_value(cfg).ok();`，循环内 `if src.needs_state_update() { if let Some(j) = &cfg_json { src.set_state(j.clone()).await; } }`。`j.clone()` 一次 alloc，胜过 `to_value` 一次。

### D5-075 `tick` / `refresh_now` / `refresh_inner` 内部各 emit 一次 `musage://snapshot`

- 置信度：高
- 文件:行号
  - `refresh_inner` 内部 emit：[`src-tauri/src/commands/mod.rs:1516`](src-tauri/src/commands/mod.rs:1516)
  - `tick` 二次 emit：[`src-tauri/src/poller.rs:391`](src-tauri/src/poller.rs:391)
  - `refresh_now` 二次 emit：[`src-tauri/src/commands/mod.rs:573`](src-tauri/src/commands/mod.rs:573)
- 触发时序：用户点「立即刷新」→ `refresh_now` → `refresh_inner` emit 一次（1516，部分 snapshot）→ `refresh_now` emit 一次（573，合并后完整 snapshot）。前端 listener 收到两次，渲染两次。
- 根因：`refresh_inner` 设计上「独立可用」（`tick` 和 `refresh_now` 都复用），但它内部已经 emit 了「它收集到的版本」，没考虑调用方还会再 emit 一次。
- 用户影响：前端多渲染一次（一次浮窗卡片重绘），CPU 浪费但不阻塞。`tick` 走 `TickGuard` 时更是「先 emit（refresher 内部）→ 等 5-30s tick 完成 → 再 emit（tick 自己）」，用户在前端看到「先闪一下半数据 → 几秒后变全」。
- 最小修复：把 1516 行的 `app.emit` 移到 `refresh_inner` 之外，由调用方 emit（`tick` 和 `refresh_now` 已经各自 emit 了）。或者把 `refresh_inner` 改成不 emit，签名返回 `Result<QuotaSnapshot, String>` 给调用方决定。

### D5-076 `quit_app` 150ms 不够覆盖初始 `tick()` 慢路径

- 置信度：中
- 文件:行号
  - [`src-tauri/src/commands/mod.rs:1106-1109`](src-tauri/src/commands/mod.rs:1106)
  - 初始 tick：[`src-tauri/src/poller.rs:99`](src-tauri/src/poller.rs:99)
- 触发时序：用户刚启动 app 就立刻「退出」或 panic 重启 → `poller::start` 还在 await 初始 `tick()` → `SHUTDOWN.notify_waiters()` 此时没有 waiter（`select!` 还没起来，poller.rs:147）。150ms 后 `app.exit(0)` 触发 tokio runtime drop，初始 tick 的 12 个 in-flight HTTP 请求被 cancel。
- 根因：`SHUTDOWN.notify_waiters()` 只唤醒**当前已在等**的 future。初始 `tick()` 是 `await tick(&app).await`（99 行），在 `select!` 之前，所以信号丢失。150ms 只能等主循环第一次进入 `select!` 并 drain。慢 provider（30s timeout）下 150ms 不够。
- 用户影响：低概率（启动就退），但偶发「设置面板点完 key 立刻重启 → 第一次拉的数据丢失，UI 显示 stale」。
- 最小修复：把 1108 行的 sleep 换成 `SHUTDOWN_DONE.notified()`（`oneshot::channel` 收 poller 真正 drain 完的 ack），超时 30s fallback。或者把初始 tick 也包进 `select!`。

### D5-074 长暂停后 per-provider 同时 fire，事件风暴

- 置信度：中
- 文件:行号：[`src-tauri/src/poller.rs:239-328`](src-tauri/src/poller.rs:239)
- 触发时序：app 挂起（macOS sleep / 用户长时间不交互）后唤醒，12 个 `next_fetch` entry 全部过期（`now > entry`）。当前 tick 全部 `now >= entry` 命中，循环连续 spawn 12 个 `refresh_single_from_poller` 任务。jitter 只影响「下次 deadline」（325-327），不影响「本次 spawn 时间」。
- 根因：spawn 同步发生（307 行 `JoinSet::spawn`），12 个 future 几乎同时被 tokio scheduler 拉起执行。如果 `tick_is_running() == false`（无全量刷新在跑），12 个并发 HTTP 请求打出去。
- 用户影响：12 个并发 fetch，5-10s 同时返回，浮窗 12 张卡片几乎同时跳数据。jitter 没救本次。后端 provider 如果有 rate limit 可能触发 429。
- 最小修复：spawn 前做 `random_jitter_delay(0..=max_jitter_ms).await`，或每 spawn N 个 source 后 `tokio::task::yield_now().await` 让出给其他任务。或者限制 per-provider 的「最长未拉取时间」超过 N 倍 interval 就降级为 manual backoff。

### D5-101 `spawn_debounced_geom_persister` 无 shutdown 信号

- 置信度：高
- 文件:行号：[`src-tauri/src/lib.rs:484-521`](src-tauri/src/lib.rs:484)
- 触发时序：用户拖完浮窗最后 100ms 内 quit → 500ms 落盘循环还没跑 → tokio runtime drop 取消 task → `Mutex<Option<(x,y,w,h)>>` 里的最新值丢失。
- 根因：`loop { sleep(500ms).await; ... }` 永久循环，不监听 `SHUTDOWN`。
- 用户影响：用户最后一次拖动位置不持久化。下次启动 `cfg.floating_x/y` 是上一次值，浮窗可能回到「拖之前」位置。
- 最小修复：加一个 `static SHUTDOWN_GEOM: Notify = Notify::const_new();`，500ms 循环里 `tokio::select! { _ = sleep() => {}, _ = SHUTDOWN.notified() => break; }`。`quit_app` 触发 `SHUTDOWN.notify_waiters()` 后再 sleep 一次（100ms）保证 last batch 落盘。

### D5-102 `start_hover_emitter` / `start_fullscreen_watcher` OS 线程无 shutdown

- 置信度：中
- 文件:行号
  - hover_emitter thread：[`src-tauri/src/platform/macos.rs:141-220`](src-tauri/src/platform/macos.rs:141)
  - fullscreen_watcher thread：[`src-tauri/src/platform/macos.rs:430-478`](src-tauri/src/platform/macos.rs:430)
  - Windows 同款：[`src-tauri/src/platform/windows.rs:330-...`](src-tauri/src/platform/windows.rs:330)
- 触发时序：app 退出时 OS 直接 kill 进程，线程没有 join。`std::thread::Builder::spawn` 不返回 JoinHandle，没法 abort。
- 根因：线程内部 `loop { thread::sleep(...); ... }` 永久循环，没有 AtomicBool 退出条件。注释明确写「idempotent，启动后整个 app 生命周期不停」。
- 用户影响：功能上无 bug（OS 进程退出 = 线程结束）。但 macOS 上 quit_app 后进程残留 zombie 数百 ms（系统回收线程栈），且 `tracing::info!("hover emitter 启动")` 之类的日志可能漏出来。Apple notarization 对 zombie 不在意。
- 最小修复：加 `static SHUTDOWN_THREADS: AtomicBool = AtomicBool::new(false);` 在 `quit_app` 里 `store(true)`，线程循环每 N tick 查一次并 break。或者改用 tokio task + `tokio::select!`（但这俩线程是 macOS NSWindow / Win32 强耦合 main thread 的，tokio 化改动大）。

---

## 不构成 P0 / 不写入本轮

- **`tick_is_running` 与 `TickGuard` 的 CAS 竞态**：D5-044 候选，`compare_exchange` 成功后到 `tick_is_running()` 读 flag 之间有纳秒级窗口，不构成真问题。
- **`IN_FLIGHT` JoinSet 与 drain 路径的并发**：D5-035 已确认无并发（`std::mem::take` 把 JoinSet move 出 static，主循环已 return）。
- **`refresh_single_inner` 的 `Manual` vs `Poller`**：D5-085 已确认正确（`refresh_single_from_poller` 显式传 `Poller`，`refresh_single` 显式传 `Manual`）。
- **`retain_live` 的 O(N) 成本**：每次 tick 都跑一次，但 N ≤ 13+256 = 269，亚微秒，不构成性能问题。
- **`fill_next_fetch_at` 的 `now` 与 read 锁释放的微妙时序**：D5-114 候选，误差 < 1ms，浮窗倒计时显示无可见影响。
- **`jitter_for` 的 `hash as i64` wrap**：D5-052 已确认 `range` 在 17M 以内，wrap 不会改变 `rem_euclid` 结果，jitter 分布仍均匀。

---

## 验证方法（不确定项）

- **D5-084** 复现：在 `RUST_LOG=musage=debug` 下，启动 app → 关网络 → 在浮窗右键「立即刷新」3 次（间隔 5s）→ 看 `BackoffState.next_interval_secs("minimax", 60)` 是否从 60 → 120 → 240。预期：现在会涨（bug），修完后保持 60。
- **D5-007** 验证：临时把 `refresh_inner` 1426 的 `write().await` 改成 `sleep(50ms).await; write().await`，perf benchmark 看 12 provider 全量 tick 的总耗时。
- **D5-033** 验证：单元测试构造 `last_intervals: HashMap` 1000 条 stale entry，断言 `add_extra_instance` + `delete_extra_instance` 后 size 不变（现在会变，证明漏清理）。
- **D5-076** 验证：临时把 `refresh_inner` 里 deepest provider 的 `fetch` 改成 `tokio::time::sleep(Duration::from_secs(5)).await; Err(...)`，启动 app 后立刻 quit，log 看 `poller drain 时 task panic` 是否出现。
- **D5-074** 验证：把 `cfg.refresh_interval_secs = 10`，启动后 macOS `pmset sleepnow` 60s 再唤醒，看 `cargo run` stdout 的 `per-provider 拉取` 时间戳是否全在同一秒内。