//! 后台轮询：tokio interval，定期拉取并广播到前端 + 刷新托盘
//!
//! Phase 2 (H9) 起改为 per-provider 调度 —— 每个 provider 拿自己的
//! `cfg.providers[id].refresh_interval_secs`（None 时 fallback 到
//! 全局 `cfg.refresh_interval_secs`），独立 sleep + 独立 fetch。
//! 用户可以给不常变动的 provider 设长间隔节流。

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

/// H1 fix (2026-07-30 audit): 全工程 graceful shutdown 信号源。
///
/// 之前主循环永远退出不了、per-provider task 永不 drain → App 退出时丢
/// 部分 in-flight fetch + JoinSet 残留 panic 提示。quit_app 调
/// `SHUTDOWN.notify_waiters()` 唤醒主循环, 主循环会立刻 break 跳出、
/// JoinSet abort + drain,然后 app.exit(0)。
pub(crate) static SHUTDOWN: Notify = Notify::const_new();

/// D5-102 fix (2026-07-30 audit): tokio::sync::Notify 不能跨 std::thread
/// 用 (Windows / macOS 的 hover emitter 是 std::thread::spawn, 等不了
/// notified().await)。改用 std::sync::atomic::AtomicBool, quit_app 同步
/// store(true), OS 线程每个 50ms tick 检查一次, 检测到就 break 退出。
///
/// SHUTDOWN (Notify) 仍保留给 tokio task (poller 主循环 + geom_persister),
/// 二者并存, 各自用途。
pub(crate) static SHUTDOWN_NATIVE_THREADS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 2026-08-05 审查交叉验证修复: tokio 主循环 shutdown 兜底标志.
///
/// `SHUTDOWN` (Notify) 的 `notify_waiters()` 只唤醒**当前已注册**的
/// `notified()` future -- 若信号在主循环 loop body 执行期间触发 (此时
/// 没有 `notified()` future 注册), 通知永久丢失, 主循环漏掉 shutdown.
/// 此 AtomicBool 由 `quit_app` 在 `notify_waiters()` 之前置位, 主循环
/// `select!` 退出后必查, 不依赖 notify 时序, 保证不漏.
///
/// 跟 `SHUTDOWN_NATIVE_THREADS` 区分: 那个给 std::thread (hover emitter),
/// 这个给 tokio 主循环. 两者并存.
pub(crate) static SHUTDOWN_REQUESTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

use crate::commands::refresh_inner;
use crate::providers::all_sources;
use crate::AppState;

/// H2 fix (2026-07-29 审查): 给定 provider id + interval,返 0..+10% 范围的
/// 确定性 jitter ms。确定性 (基于 provider id hash) → 同一 provider
/// 每次拉取 jitter 都一样 (避免运行时漂移),不同 provider 散开。
///
/// 不引 rand 依赖 —— FNV-1a 64-bit hash 输出分散到 u64 全空间,再 map
/// 到 0..+10% interval 范围 (ms 范围 ~6s @ interval=60s)。21 bucket (旧版)
/// 散 12 个 provider 只能 7-8 个不同 (实测撞概率高),后端看到的是
/// 绝对 jitter ms,不是 percent,只要"不同 provider 不同毫秒偏移"即可,
/// u64 空间足够。
fn jitter_for(provider_id: &str, interval_secs: u64) -> u64 {
    // FNV-1a 64-bit hash
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in provider_id.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let max_ms = (interval_secs as i64 * 1000) / 10;
    let range = (max_ms * 2 + 1) as u64;
    let offset = (hash as i64).rem_euclid(range as i64) - max_ms;
    offset.unsigned_abs()
}

fn retain_live_schedules(
    next_fetch: &mut HashMap<String, Instant>,
    last_intervals: &mut HashMap<String, u64>,
    live_sources: &std::collections::HashSet<String>,
) {
    next_fetch.retain(|key, _| live_sources.contains(key));
    last_intervals.retain(|key, _| live_sources.contains(key));
}

/// P1 audit fix (2026-08-13): interval 直接来自用户可改的 config.json,
/// 只做了 .max(10) 下限。`Instant + Duration::from_secs` 内部是
/// checked_add().expect(), 超大值(如 u64::MAX)会 panic 杀死 poller 主循环
/// 且无人重启 —— 轮询永久停摆。统一 clamp 到 save_config 同款上界 86400s。
const MIN_INTERVAL_SECS: u64 = 10;
const MAX_INTERVAL_SECS: u64 = 86400;

/// pub(crate): commands 层的 next_fetch_at / backoff 计算同样消费
/// per-provider interval, 必须用同款 clamp (u64::MAX → `(as i64)*1000` 溢出)。
pub(crate) fn clamp_interval_secs(secs: u64) -> u64 {
    secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS)
}

/// P1 audit fix (2026-08-13): per-provider in-flight 去重。之前只按
/// next_fetch deadline 调度: fetch 耗时 > interval (挂起 HTTP + 短间隔)时,
/// 同一 unique_id 每轮循环重复 spawn 并发请求, 失败还重复计入 backoff。
/// spawn 前插标记, task 结束(含 panic / SHUTDOWN abort)由 guard drop 移除;
/// 被跳过的那轮 entry 保持过期, 下个 tick (1s) 重试。
static PER_PROVIDER_IN_FLIGHT: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::OnceLock::new();

fn per_provider_in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    PER_PROVIDER_IN_FLIGHT.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// false = 已有同 unique_id 的 fetch 在跑, 调用方应跳过本次调度。
fn try_mark_provider_in_flight(unique: &str) -> bool {
    per_provider_in_flight()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(unique.to_string())
}

struct ProviderInFlightGuard {
    unique: String,
}

impl Drop for ProviderInFlightGuard {
    fn drop(&mut self) {
        per_provider_in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.unique);
    }
}

/// per-provider 拉取 task 集合。poller 每秒检查时把过期的 provider spawn 进来，
/// task 完成或 panic 后自动从 set 里清理（JoinSet::join_next 移除）。当前
/// 不在 quit_app 时主动 abort —— 浮窗最常见关闭是"窗口关闭"拦截（tray 隐藏），
/// poller 跟 app 同生同死。后续如要 abort-on-quit，给 AppState 加 abort flag。
static IN_FLIGHT: std::sync::OnceLock<std::sync::Mutex<JoinSet<()>>> = std::sync::OnceLock::new();

fn in_flight() -> &'static std::sync::Mutex<JoinSet<()>> {
    IN_FLIGHT.get_or_init(|| std::sync::Mutex::new(JoinSet::new()))
}

/// M7 fix (2026-07-03 audit): tick() 并发去重。用户在 poller 自动 tick 期间
/// 点"立即刷新",两个 tick() 并发跑 → 2N 次网络请求 + backoff 记录竞争。
/// 正在跑时直接返回 Ok,避免重复 fetch。
static TICK_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// P6 fix (2026-07-28 审查): 全量刷新互斥位的 RAII guard —— drop 自动释放,
/// tick() / refresh_now 无论成功失败都不会卡住 flag。
pub(crate) struct TickGuard {
    _priv: (),
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        TICK_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 尝试占有全量刷新互斥位。Some = 本调用方独占(guard drop 自动释放);
/// None = 已有 tick/refresh 在跑,调用方应跳过本次(防双倍 fetch 风暴)。
pub(crate) fn try_acquire_tick() -> Option<TickGuard> {
    TICK_RUNNING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .ok()?;
    Some(TickGuard { _priv: () })
}

/// 全量刷新是否正在进行。P4 fix: poller spawn 的 per-provider 拉取入口
/// 用它跳过与 tick 重叠的本次调度。
pub(crate) fn tick_is_running() -> bool {
    TICK_RUNNING.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动后立即拉一次（全量）
        if let Err(e) = tick(&app).await {
            tracing::warn!(error = %e, "初次拉取失败");
        }

        // per-provider 下次拉取时间。初始化为 "now + interval"（不在启动瞬间
        // 跟 tick() 的全量 fetch 并发抢写 state.snapshot —— 那会跟 tick()
        // 的「全量 push」撞出重复 provider 条目）。第一轮 per-provider 调度
        // 会因为 now < entry 而全部 skip，等到各自 interval 后才开始 fire。
        //
        // H1: builtin_sources() 不含 custom sources。poller 必须用 all_sources
        // 才能让用户添加的 New API 中转站被定时轮询——否则 custom source 唯一能
        // 拿数据的时机是「启动时 tick() 全量拉一次」+「用户手动点立即刷新」
        // （add/update_custom_source 调 refresh_single_inner 那次）。
        let state = app.state::<AppState>();
        let cfg0 = state.config.read().await.clone();
        let mut next_fetch: HashMap<String, Instant> = HashMap::new();
        // P8 fix (2026-07-28 审查): 记录每个 provider 上次调度用的 cfg
        // interval。初始 next_fetch 用启动时 cfg0 快照算 deadline,用户改
        // interval 后第一轮 per-provider 调度仍按旧值到期 —— 主循环里
        // 发现 interval 变化时把 entry 重排到 now + 新值。
        let mut last_intervals: HashMap<String, u64> = HashMap::new();
        for src in all_sources(&state).await {
            // PR 1a：用 unique_id() 而不是 id()，否则 minimax #2 跟 minimax
            // 共享 map entry（id() 都返 "minimax"），#2 的 interval 会覆盖
            // 内置那一份。config/enabled/interval/backoff 也用同一套 key。
            let unique = src.unique_id();
            let id_str = unique.as_str();
            let base_id = src.id().into_owned();
            let fallback_interval = clamp_interval_secs(
                cfg0.providers
                    .get(id_str)
                    .or_else(|| cfg0.providers.get(&base_id))
                    .and_then(|p| p.refresh_interval_secs)
                    .unwrap_or(cfg0.refresh_interval_secs),
            );
            last_intervals.insert(unique.clone(), fallback_interval);
            next_fetch.insert(
                unique,
                Instant::now() + Duration::from_secs(fallback_interval),
            );
        }

        // 每秒检查一次。H1 fix (2026-07-30 audit): 用 tokio::select! 同时
        // 监听 sleep + SHUTDOWN.notify, quit_app 触发 notify_waiters
        // 即同步退,不丢在飞 fetch 让 JoinSet 自然 abort。
        //
        // 2026-08-05 审查交叉验证修复: 之前 drain 逻辑在 select! 的 notified()
        // 分支里 -- 若 quit_app 的 notify_waiters() 在 loop body 执行期间触发
        // (此时无 notified() future 注册), 通知丢失, 主循环漏掉 shutdown.
        // 改为: select! 两个分支都只负责"醒过来", 真正的 drain 判定移到 select!
        // 之后用 SHUTDOWN_REQUESTED AtomicBool 兜底 (quit_app notify 前置位).
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = SHUTDOWN.notified() => {}
            }
            if SHUTDOWN_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("poller 主循环收到 SHUTDOWN,开始 drain");
                // 把全部在飞 task abort + join。std::sync::MutexGuard
                // !Send,所以必须 mem::replace 接管 JoinSet + drop guard 后
                // 再 await,否则 spawn 进来的 future 拿不到 Send bound。
                let taken: JoinSet<()> = {
                    let mut g = in_flight().lock().unwrap_or_else(|e| e.into_inner());
                    std::mem::take(&mut *g)
                };
                // 全部在飞 task 取消
                let mut taken = taken;
                taken.abort_all();
                // 等所有 task 退出,join_set .join_next().await 阻塞到空
                while let Some(res) = taken.join_next().await {
                    if let Err(e) = res {
                        if !e.is_cancelled() {
                            tracing::warn!(error = %e, "poller drain 时 task panic");
                        }
                    }
                }
                tracing::info!("poller 主循环退出");
                return;
            }
            let cfg = app.state::<AppState>().config.read().await.clone();
            // M5 fix: 之前 backoff read guard 持有整个 for 循环（for 循环里 spawn 的
            // refresh_single_inner 要拿 backoff.write → tokio RwLock read-prefer-write
            // 公平锁 → write 全排队 1s+ → 用户保存 key 后 refresh_single_inner 卡 1s+）。
            // 改为：先 clone 一份 interval map，立刻 drop guard，循环里查 clone。
            let state = app.state::<AppState>();
            let backoff_snapshot = {
                let guard = state.backoff.read().await;
                guard.clone_interval_map()
            };
            // 清理已完成/panic 的 task —— JoinSet 拿掉 finished task，panic 也
            // 算 finished（await JoinHandle 会返 Err）。2026-06-20 audit：
            // 之前完全 fire-and-forget 累积 panic task。
            //
            // L2 fix（2026-07-02 audit）：之前用 `while let Some(res) = set.try_join_next()`
            // 连续消费所有完成 task,极端场景(网络恢复后 12 provider 同时完成)
            // 持锁时间随 task 数线性增长。改为:每次循环最多 batch_size 个,
            // drop 锁让其它 spawn 路径(spawn 本身也要拿 in_flight 锁)有机会执行。
            // 5 是一个保守上限:典型 0-3 task/s 完成,大部分 tick 一次 try_join_next
            // 就只清 0-1 task。
            let batch_size: usize = 5;
            {
                let mut set = in_flight().lock().unwrap_or_else(|e| {
                    tracing::warn!("poller IN_FLIGHT mutex poisoned, recovering");
                    e.into_inner()
                });
                for _ in 0..batch_size {
                    match set.try_join_next() {
                        Some(Ok(())) => {}
                        Some(Err(e)) if e.is_panic() => {
                            tracing::error!(
                                panic = ?e.into_panic(),
                                "poller spawned task panic（已清理）"
                            );
                        }
                        Some(Err(_)) => {}
                        None => break,
                    }
                }
            }
            let now = Instant::now();

            // H1: 同上,改用 all_sources(&state)——custom source 必须能被轮询
            //
            // P5 fix (2026-07-28 审查): 之前 live_sources 算一次 all_sources、
            // 下面 for 循环又算一次 —— 每秒 2 × (13+ 个 Box 分配 + RwLock
            // read)。合并成一次调用复用结果。
            let sources = all_sources(&state).await;
            // H2 fix: 清理 next_fetch 里已不存在的 source 条目。
            // delete_extra_instance 后 extras 列表少了条目,poller 不再调度
            // 它,但 next_fetch HashMap 里仍有该 unique_id 的 entry 只增不删,
            // 长时间频繁 add/delete 会泄漏。每次 tick 先算当前所有 unique_id,
            // 把不在 set 里的 entry 从 next_fetch 删掉 (用 retain 一次完成)。
            let live_sources: std::collections::HashSet<String> =
                sources.iter().map(|s| s.unique_id()).collect();
            // L2 fix: 跟 try_join_next 的 lock 处理对称 ——
            // 单次 batch 删除所有 stale entries 后立刻 drop 锁,避免保留锁进
            // 长 for 循环(下面 for src 是大批量 source)。
            retain_live_schedules(&mut next_fetch, &mut last_intervals, &live_sources);
            // M6 fix: 同步清理 backoff 里已删除 source 的 entry,
            // 避免 HashMap 长期膨胀(每删一个 extra instance 留一条永久残留)。
            {
                let mut backoff_guard = state.backoff.write().await;
                backoff_guard.retain_live(&live_sources);
            }

            for src in &sources {
                // PR 1a：用 unique_id() 做 map key（决策 1：id() 共享 base）
                //
                // C1 fix (2026-07-03 audit): 之前 L137/L143/L146/L153 用 id_str
                // (= src.id() = base id "minimax"),而 refresh_single_inner 写 backoff
                // 时用 unique_id ("minimax#2")。读写键不一致 → extra instance 永不退避、
                // per-instance interval 失效、禁用后仍 spawn task。改为 unique_id 优先,
                // base_id fallback(共享 base instance 的配置)。
                let unique = src.unique_id();
                let base_id = src.id().into_owned(); // Cow::Owned 解引用，避开临时 lifetime
                let unique_str: &str = &unique;
                let base_str: &str = &base_id;
                // enabled: 优先查 instance 自己的 entry,没配置则 fallback 到 base
                // (用户关 base 时 extra 也跟着关,除非显式启用 extra)
                let enabled = cfg
                    .providers
                    .get(unique_str)
                    .map(|c| c.enabled)
                    .or_else(|| cfg.providers.get(base_str).map(|c| c.enabled))
                    .unwrap_or(true);
                if !enabled {
                    continue; // 用户关了，不拉
                }
                let cfg_interval_secs = clamp_interval_secs(
                    cfg.providers
                        .get(unique_str)
                        .or_else(|| cfg.providers.get(base_str))
                        .and_then(|p| p.refresh_interval_secs)
                        .unwrap_or(cfg.refresh_interval_secs),
                );
                // P8 fix: 用户改了 interval(全局或 per-provider)后,重排该
                // provider 的 deadline —— 否则第一轮仍按启动时旧值到期,新值
                // 要再等一轮才生效。用 cfg_interval_secs(不含 backoff)做变化
                // 检测:backoff 波动不该触发重排,fire 时仍按 backoff 算下轮。
                if last_intervals.get(unique_str).copied() != Some(cfg_interval_secs) {
                    last_intervals.insert(unique.clone(), cfg_interval_secs);
                    // 已存在的 entry 直接重排;还不存在的(新 source)交给下面
                    // entry().or_insert(now) 立即 fire。
                    if let Some(e) = next_fetch.get_mut(unique_str) {
                        *e = now + Duration::from_secs(cfg_interval_secs);
                    }
                }
                // 退避后的实际间隔：backoff 用 unique_id 写(见 refresh_single_inner),
                // 这里也必须用 unique_id 读。base instance 的 unique_id == base_id,
                // 自动兼容。
                let interval_secs = clamp_interval_secs(
                    backoff_snapshot
                        .get(unique_str)
                        .copied()
                        .unwrap_or(cfg_interval_secs),
                );

                let entry = next_fetch.entry(unique.clone()).or_insert(now);
                if now < *entry {
                    continue; // 还没到点
                }
                // P1 audit fix (2026-08-13): 同一 provider 上次 fetch 还没
                // 结束 (耗时 > interval) 时跳过本次 spawn —— 之前会重复
                // spawn 并发请求 + 失败重复计入 backoff。entry 保持过期,
                // 下个 tick (1s 后) 重试。
                if !try_mark_provider_in_flight(unique_str) {
                    continue;
                }
                // D5-074 fix (2026-07-30 audit): 长暂停 (sleep / 长时间不交互)
                // 后唤醒时,12 个 next_fetch entry 全部过期, 同步连续 spawn 12
                // 个 task → 12 个并发 HTTP 同时打出去。 之前的 jitter (H2 fix)
                // 只影响 next deadline, 不影响本次 spawn 时间。修复: spawn 前
                // await jitter 延迟,让本次 fire 也散开,后端 / 中转站不会瞬时
                // 12 倍并发压力。
                //
                // cap jitter 到 interval 的 10% (跟 jitter_for 一致), wake-up
                // 路径下 spawn 总等待时长 ≤ 1.2 * interval,用户体感仍是"立刻刷新"。
                //
                // H6 fix (2026-08-03 audit): jitter sleep **必须**移进 spawn task,
                // 主循环只推进 entry。原来主循环里 await sleep (1.2 * 12 providers
                // 顺序串行 ≈ 36-72s),quit_app 500ms drain 窗口内主循环卡 sleep,
                // app.exit(0) 强杀 → in-flight fetch 被强杀 + JoinSet 残留 panic
                // 日志。移进去后主循环立即可响应 SHUTDOWN,spawn task 自己用
                // select 守 SHUTDOWN 提前 return。
                let jitter_ms = jitter_for(unique.as_str(), interval_secs);
                // 到点 → 拉这个 provider（独立 task，并发）
                let app_clone = app.clone();
                let unique_owned = unique.clone();
                in_flight()
                    .lock()
                    .unwrap_or_else(|e| {
                        // M22 fix (2026-07-06 全量审查): 显式 log warn 等级 +
                        // 注释解释 into_inner 的边缘场景(JoinSet::spawn 内部
                        // allocation 期间 mutex 持有,关闭路径上若别的 thread
                        // 已锁,本 tick 漏一次拉取,下个 60s 周期自动恢复)。
                        tracing::warn!("poller IN_FLIGHT mutex poisoned (spawn), recovering");
                        e.into_inner()
                    })
                    .spawn(async move {
                        // P1 audit fix: guard 必须在最前 —— jitter 期间
                        // SHUTDOWN abort / refresh panic / 正常完成, 全部靠
                        // drop 移除 in-flight 标记, 释放该 provider 的调度。
                        let _inflight_guard = ProviderInFlightGuard {
                            unique: unique_owned.clone(),
                        };
                        // H6 fix: jitter sleep 在 spawn task 里,且 select 守 SHUTDOWN
                        // —— quit_app 期间 task 立即退出,不阻塞 drain。
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(jitter_ms)) => {}
                            _ = SHUTDOWN.notified() => {
                                tracing::debug!(provider = %unique_owned, "spawn task aborted by SHUTDOWN during jitter");
                                return;
                            }
                        }
                        // P4 fix (2026-07-28 审查): 走 poller 专用入口 ——
                        // tick()/refresh_now 全量刷新在跑时跳过本次(之前
                        // TICK_RUNNING 只防 tick vs tick,这里 spawn 的
                        // refresh_single_inner 跟 tick 并发 → backoff.record
                        // 双倍计数 + fetch 量翻倍)。
                        match crate::commands::refresh_single_from_poller(&app_clone, &unique_owned).await {
                            Ok(()) => {}
                            Err(e) => tracing::warn!(error = %e, provider = %unique_owned, "per-provider 拉取失败"),
                        }
                    });
                // H2 fix (2026-07-29 审查): jitter 已用于 spawn 前 sleep,next
                // entry 仍按 interval + jitter 排,但本次 fire 已在 sleep 时分散。
                // jitter 已在 spawn 前消费,此处不再重复计算。
                *entry =
                    now + Duration::from_secs(interval_secs) + Duration::from_millis(jitter_ms);
            }
        }
    });
}

/// 手动触发一次（供 tray 菜单和 commands::refresh_now 调用）
pub async fn tick_now(app: &AppHandle) -> Result<(), String> {
    tick_with_source(app, crate::poller_backoff::RefreshSource::Manual).await
}

pub async fn tick(app: &AppHandle) -> Result<(), String> {
    tick_with_source(app, crate::poller_backoff::RefreshSource::Poller).await
}

async fn tick_with_source(
    app: &AppHandle,
    caller: crate::poller_backoff::RefreshSource,
) -> Result<(), String> {
    // M7 fix: 并发去重。CAS 已封装进 try_acquire_tick(P6 fix 抽出,
    // refresh_now 复用同一位)。已有实例在跑时直接返回。
    let Some(_guard) = try_acquire_tick() else {
        tracing::debug!("tick() 已有实例在跑,跳过本次并发触发");
        return Ok(());
    };

    let cfg = {
        let state = app.state::<AppState>();
        let cfg = state.config.read().await.clone();
        cfg
    };

    let new_snap = refresh_inner(app, &cfg, caller).await?;

    // 合并写回 state（而不是整块覆写）——
    // refresh_inner 会在内部 emit 一次快照，但那个快照是在 fetch 各 provider
    // 并发期间收集的；如果此时 per-provider poller 的 refresh_single_inner
    // 已经把某个 provider 更新到 state.snapshot 里了，整块覆写会把那份新数据
    // 回滚成 refresh_inner 拿到的旧版本。
    //
    // 正确做法：按 snapshot_key 逐条合并——新数据覆盖旧的，但只动 fetch 到的
    // provider，不碰其他的。
    {
        let state = app.state::<AppState>();
        let mut guard = state.snapshot.write().await;
        for new_p in &new_snap.providers {
            // P3 fix (2026-07-28 审查): 合并键统一为 snapshot_key(unique_id
            // 优先)。之前只按 source_id 匹配 —— 副本 fetch 成功时 source_id
            // 是 provider 侧硬编码的 base id("minimax"),unique_id 才是
            // "minimax#2",按 source_id 合并会把副本数据覆盖基础实例条目。
            let new_id = crate::commands::snapshot_key(new_p);
            if let Some(idx) = guard
                .providers
                .iter()
                .position(|p| crate::commands::snapshot_key(p) == new_id)
            {
                // 2026-08-05 审查交叉验证修复: tick 全量刷新的 new_snap 是
                // refresh_inner fetch 期间收集的, 可能比 per-provider poller
                // 并发写入的更新数据更旧 (tick_is_running 只挡 tick 开始后的
                // 新 task, 挡不住已 in-flight 的 per-provider fetch). 比较
                // fetched_at, 仅在新数据 >= 已有数据时覆盖, 避免 tick 回滚
                // per-provider 的并发更新. 漏掉时下个周期会自愈.
                let old_ts = guard.providers[idx].fetched_at.unwrap_or(0);
                let new_ts = new_p.fetched_at.unwrap_or(0);
                if new_ts >= old_ts {
                    guard.providers[idx] = new_p.clone();
                }
            } else {
                guard.providers.push(new_p.clone());
            }
        }
        // P3 audit fix (2026-08-13): 之前顶层 fetched_at 无条件覆盖 --
        // per-provider fetch 若在 tick snapshot 收集后写入更旧的时间戳,
        // 顶层"更新于 HH:MM:SS"会倒退。跟 per-provider 合并同款比较。
        let new_top_ts = new_snap.fetched_at.unwrap_or(0);
        let old_top_ts = guard.fetched_at.unwrap_or(0);
        if new_top_ts >= old_top_ts {
            guard.fetched_at = new_snap.fetched_at;
        }
        // 顶层字段(钱包告警阈值)也要同步——refresh_inner 内部已 populate,
        // 这里只是按 snapshot_key 合并 providers,顶层字段会被忽略,所以手动搬过来。
        guard.wallet_alert_threshold = new_snap.wallet_alert_threshold;
    }

    // 合并后再 emit 一次——refresh_inner 内部 emit 的是它收集的版本，
    // 不含 per-provider poller 在并发期间的中间更新。
    let state = app.state::<AppState>();
    let final_snap = state.snapshot.read().await.clone();
    let _ = app.emit("musage://snapshot", &final_snap);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_for_is_deterministic() {
        let a = jitter_for("minimax", 60);
        let b = jitter_for("minimax", 60);
        assert_eq!(a, b, "jitter 必须确定性");
    }

    #[test]
    fn jitter_for_different_ids_scatter() {
        let providers = [
            "minimax",
            "deepseek",
            "xiaomimimo",
            "tavily",
            "zenmux",
            "openrouter",
            "kimi",
            "zhipu",
            "claude_official",
            "siliconflow",
            "stepfun",
            "anysearch",
            "tokendance",
        ];
        let mut seen = std::collections::HashSet::new();
        for id in &providers {
            seen.insert(jitter_for(id, 60));
        }
        assert_eq!(
            seen.len(),
            providers.len(),
            "12 个 provider jitter 应全部不同 (u64 空间), 实际 {} / {}",
            seen.len(),
            providers.len()
        );
    }

    #[test]
    fn jitter_for_within_10_percent() {
        let interval = 60u64;
        let max_ms = (interval * 1000) / 10;
        for id in ["minimax", "deepseek", "kimi", "zhipu", "stepfun"] {
            let j = jitter_for(id, interval);
            assert!(
                j <= max_ms as u64,
                "jitter={j} > 0..+10% ({max_ms}ms) for id={id}"
            );
        }
    }

    #[test]
    fn jitter_for_scales_with_interval() {
        let j60 = jitter_for("minimax", 60);
        let j120 = jitter_for("minimax", 120);
        let max60 = (60 * 1000) / 10;
        let max120 = (120 * 1000) / 10;
        assert!(j60 <= max60 as u64);
        assert!(j120 <= max120 as u64);
    }

    #[test]
    fn retain_live_schedules_removes_replaced_source_at_equal_size() {
        let now = Instant::now();
        let mut next_fetch =
            HashMap::from([("minimax".to_string(), now), ("minimax#2".to_string(), now)]);
        let mut last_intervals =
            HashMap::from([("minimax".to_string(), 60), ("minimax#2".to_string(), 120)]);
        let live_sources =
            std::collections::HashSet::from(["minimax".to_string(), "deepseek#2".to_string()]);

        retain_live_schedules(&mut next_fetch, &mut last_intervals, &live_sources);

        assert_eq!(next_fetch.len(), 1);
        assert!(next_fetch.contains_key("minimax"));
        assert_eq!(last_intervals, HashMap::from([("minimax".to_string(), 60)]));
    }

    /// H6 fix 主用例:SHUTDOWN 在 sleep 中途通知,task 立即退出。
    #[tokio::test]
    async fn shutdown_during_long_sleep_aborts_spawn_task() {
        let jitter_ms = 5_000u64; // 5s sleep
                                  // notify_one 安排 permit 给下一次 notified() —— 跟 wake-up 期间
                                  // 主循环处理 SHUTDOWN 的语义一致(notify_waiters 只唤醒当前等待的)。
        SHUTDOWN.notify_one();
        let aborted = tokio::time::timeout(Duration::from_millis(500), async {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(jitter_ms)) => {
                    false
                }
                _ = SHUTDOWN.notified() => {
                    true
                }
            }
        })
        .await
        .expect("select 应在 500ms 内被 SHUTDOWN permit 唤醒");
        assert!(
            aborted,
            "spawn task 在 SHUTDOWN 后必须 abort,不能等满 5s sleep"
        );
    }
}
