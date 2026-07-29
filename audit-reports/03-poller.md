# Musage 后台轮询 / 退避 / Task 生命周期 审查报告

## 总览

| 等级 | 数量 |
|---|---|
| CRITICAL | 0 |
| HIGH | 5 |
| MEDIUM | 6 |
| LOW | 7 |

**架构优点**:
- `JoinSet<()>` 替换原 fire-and-forget(`poller.rs:21-25`)+ `try_join_next` 主动回收 panic(`poller.rs:127-141`)
- `TickGuard` RAII + CAS 保证「手动刷新」与「poller 全量 tick」互斥(`poller.rs:30-62`)
- `refresh_single_from_poller` 二次检查 `tick_is_running` 堵并发(`commands/mod.rs:1708-1720`)
- `BackoffState::record` 三档分流(用户配置类不动 / 成功 reset / 服务端压力类翻倍 + cap),`max(base)` 守卫防"退避变加速"
- 退避单调不递减(L-b1 单测已锁)

## HIGH 级别

### H1 — 主循环永不退出 + 全工程零 graceful shutdown 路径
- **位置**:`src-tauri/src/poller.rs:111-262`(主 loop),`lib.rs` 没注册 shutdown,`commands/mod.rs:1078-1080`(`quit_app` 直接 `app.exit(0)`)
- **类型**:Task 生命周期 / 资源泄漏
- **描述**:主循环是 `loop { tokio::time::sleep(Duration::from_secs(1)).await; ... }` 严格 1s tick,**没有任何 cancellation 触发点**。`quit_app` 直接 `app.exit(0)` → tokio runtime drop → 所有正在跑的 task 立即被 abort。后果:半连接残留、半截文件写入(虽然有 atomic write)、半截 snapshot emit、`set_provider_enabled` placeholder 已发但 spawn fetch 被 abort → 浮窗长期显示未配置态。
- **影响**:用户主动退出时网络/文件/UI 三方都有微小概率的脏状态残留。
- **修法**:新增 `src-tauri/src/shutdown.rs` 用 `tokio::sync::Notify`,主循环改 `tokio::select!`,`quit_app` 改两步:先 `notify_waiters` + drain JoinSet,再 `app.exit(0)`。

### H2 — 12+ provider 主循环零 jitter → thundering herd
- **位置**:`src-tauri/src/poller.rs:89-107`(next_fetch 初始化)+ `poller.rs:111`(1s tick 唤醒)
- **类型**:轮询频率 / 退避策略
- **描述**:启动时所有 provider 的 next_fetch = now + interval(60s)。主循环 sleep 1s 后,**当 tick 醒来踩到 t=60s** 那一秒,for 循环检测 `now >= entry` 全 true → 单次 main loop body **spawn 12 个并发 HTTP fetch**。所有用户的 musage 启动时间对齐 → 中转站观察到的并发 = N 用户 × 12 fetch/IP → 触发 429 → 全部退避 → 用户看到的不是数据而是错误。
- **修法**:初始 deadline 加 0..interval_secs 均匀抖动,主循环的 `sleep(1s)` 也加 0-100ms 抖动。

### H3 — `refresh_inner` 内部 12 个 `tokio::spawn` 同步触发,fan-out 无界
- **位置**:`src-tauri/src/commands/mod.rs:1340-1377`
- **类型**:轮询频率 / 资源
- **描述**:全部 enabled provider 一口气 spawn,没有 `buffer_unordered` 限并发,没有 jitter 控制。如果用户配了 5 个走同一中转站的 New API 转发,瞬间 5 req → 触发中转 429 → 5 个全部退避 → 用户看到一片错误。
- **修法**:`futures::stream::iter(sources).map(...).buffer_unordered(4)` 限制并发为 4。

### H4 — `BackoffState` 内存态,App 重启退避历史全丢
- **位置**:`src-tauri/src/poller_backoff.rs` 整个文件 + `lib.rs:81-89`
- **类型**:退避状态 / 持久化
- **描述**:`BackoffState` 纯内存,无 `serde::Serialize` / `Deserialize`,无 `save()` / `load()`。用户某 provider 因为持续 server 挂被推到 30min cap;晚上 OS auto-update 触发 App 重启;**重启后 next_fetch 用 cfg 默认 60s** → 立刻又打一次中转站,新一轮 429 → 进入新一轮退避计数。中转站侧看就是"同一个 client 不规律地打过来",加重风控判定。**v0.3 待做项**(AGENTS.md 第 138 行明示)。
- **修法**:`PersistedBackoff { entries, saved_at_unix }` + `save_to_disk` / `load_from_disk`,`record()` 末尾 spawn debounce task。

### H5 — 手动「立即刷新」失败时,backoff streak 被多算
- **位置**:`src-tauri/src/commands/mod.rs:1430-1450`(`refresh_inner` Err 分支)
- **类型**:退避策略 / 错误传播
- **描述**:`refresh_inner` 收集 task 结果时,无论 spawn 是 poller 触发还是手动「立即刷新」触发,**失败路径都会调用 `backoff.record()`**。用户多次手动刷新之后,poller 频率变成 30min cap,看起来像"软件坏了没自动刷新"。
- **修法**:`BackoffState::record` 加 caller 区分(Poller / ManualOverride),手动路径的失败 no-op,只 reset(成功)。`refresh_inner` / `refresh_single_inner` 加 `caller: RefreshSource` 参数。

## MEDIUM 级别

### M1 — 主循环每秒 `all_sources(&state).await` → 13 个 Box 分配 + RwLock read
- **位置**:`src-tauri/src/poller.rs:152-157`
- **修法**:用 `ArcSwap<Option<Vec<Box<dyn QuotaSource>>>>` 缓存,或把 1s tick 拉到 5s。

### M2 — `IN_FLIGHT` 用 `std::sync::Mutex`,持锁期间不能 await,脆弱
- **位置**:`src-tauri/src/poller.rs:21-25`、`127-141`、`238-258`
- **修法**:改 `tokio::sync::Mutex<JoinSet<()>>`,或注释 `// INVARIANT: no .await while holding this lock` 显式禁止。

### M3 — `last_intervals` 与 `entry` 不同步:用户改 interval 后立即 fire 也可能撞 backoff cap
- **位置**:`src-tauri/src/poller.rs:213-220` + `255-259`
- **描述**:P8 fix 用 `cfg_interval_secs` 重排 `entry`,但主循环 fire 后的推进用的是 `interval_secs`(含 backoff)。用户某 provider 现在 backoff=1800s,在设置面板把 interval 从 60s 改到 10s。期望:下一轮 10s 后立即拉;实际:推进 `entry = now + 1800s`。
- **修法**:backoff 跟 cfg interval 解耦,backoff 只决定「skip 这次轮询」。

### M4 — 1 tick 内 fan-out 无上界
- **位置**:`src-tauri/src/poller.rs:227-260`
- **修法**:主循环末尾 `take(4)` 限并发。

### M5 — `kill / cron` 等场景:low-power mode 没有真正和 poller 联动
- **位置**:`lib.rs`(没有 `set_low_power_mode` 的 hook)
- **修法**:加 `tokio::sync::watch<bool>(low_power)`,主循环 select! 监听。

### M6 — 1s tick + O(N) body → schedule drift,deadline 实际值漂移 1-3s
- **位置**:`src-tauri/src/poller.rs:111-262`
- **修法**:改 `tokio::time::interval(Duration::from_secs(1))` + `MissedTickBehavior::Skip`。

## LOW 级别

### L1 — `IN_FLIGHT` 进程退出时静态析构顺序不可预测(同 H1)

### L2 — `try_join_next` batch=5 上限 → 长跑 JoinSet 内存轻微驻留(可接受)

### L3 — `std::sync::Mutex` 持锁期间 await 是 silent UB(防御性,同 M2)

### L4 — 退避 record 无 timestamp:无法做"1h 内 5 次失败立即 cap"
- **修法**:`SourceBackoff` 加 `last_failure_unix: u64`。

### L5 — 用户配置类失败既不清零也不递增 → 语义隐式依赖"成功一定清零"(行为正确,记录)

### L6 — `shared_client` `pool_idle_timeout=30s` + `pool_max_idle_per_host=2` 太保守
- **修法**:`pool_idle_timeout=120s`,`pool_max_idle_per_host=4`。

### L7 — `max(10)` 魔术数分散在 4 处
- **位置**:`poller.rs:96`、`200`、`245`、`commands/mod.rs:1296`
- **修法**:提到 `pub const MIN_INTERVAL_SECS: u64 = 10;`。

### L8 — provider 切换(ip 区域、base_url)后,backoff 不感知
- **修法**:setter 调 `backoff.reset(unique_id)`。

## 额外发现(不属于该域,但在审查时撞见)

### O1 — `poller.rs:88-107` 用 `cfg0` 作为 60s 内不变的快照,但主循环每 1s 重新 `state.config.read()` 拿新 cfg,没真正用 `cfg0`。dead code 形态,无害。

### O2 — `poller_backoff.rs:99` `reset()` 是 pub,无调用方。
- **修法**:加 `#[cfg(test)]` 锁住或文档化"only test"。

### O3 — `poller.rs:249` 新 source 立刻 fire(`or_insert(now)`)——设计正确,但**没有走 `tick_is_running` 守卫**。实际影响小,但代码 fragile。

## 推荐修复次序

| 优先级 | 项 | 工作量 | 影响 |
|---|---|---|---|
| P0 | H1 graceful shutdown | M | 数据完整性 |
| P0 | H2 + H3 jitter / buffer_unordered | S | 中转站风控 / UX |
| P1 | H4 backoff 持久化 | M | 跨重启稳定 |
| P1 | H5 manual vs poller 区分 | S | UX |
| P2 | M3 退避 vs cfg interval 交互 | S | UX |
| P2 | L6 shared_client pool 调优 | S | 性能 |

## 总结

**正面**:Panic 隔离、双 tick 防重、Backoff 单调不递减、shared_client 防连接泄漏、`task.await` Err 分支处理 join panic 都已落地扎实。

**空白**:**任务生命周期完全没 graceful shutdown**(H1),fire-and-forget `tauri::async_runtime::spawn` 进程退出时无 drain;**零 jitter / 无并发上限**(H2 H3);**退避状态无持久化**(H4);**手动刷新跟 poller 退避语义混淆**(H5)。
