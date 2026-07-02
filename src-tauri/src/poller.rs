//! 后台轮询：tokio interval，定期拉取并广播到前端 + 刷新托盘
//!
//! Phase 2 (H9) 起改为 per-provider 调度 —— 每个 provider 拿自己的
//! `cfg.providers[id].refresh_interval_secs`（None 时 fallback 到
//! 全局 `cfg.refresh_interval_secs`），独立 sleep + 独立 fetch。
//! 用户可以给不常变动的 provider 设长间隔节流。

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::JoinSet;

use crate::commands::refresh_inner;
use crate::providers::all_sources;
use crate::AppState;

/// per-provider 拉取 task 集合。poller 每秒检查时把过期的 provider spawn 进来，
/// task 完成或 panic 后自动从 set 里清理（JoinSet::join_next 移除）。当前
/// 不在 quit_app 时主动 abort —— 浮窗最常见关闭是"窗口关闭"拦截（tray 隐藏），
/// poller 跟 app 同生同死。后续如要 abort-on-quit，给 AppState 加 abort flag。
static IN_FLIGHT: std::sync::OnceLock<std::sync::Mutex<JoinSet<()>>> = std::sync::OnceLock::new();

fn in_flight() -> &'static std::sync::Mutex<JoinSet<()>> {
    IN_FLIGHT.get_or_init(|| std::sync::Mutex::new(JoinSet::new()))
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
        for src in all_sources(&state).await {
            // PR 1a：用 unique_id() 而不是 id()，否则 minimax #2 跟 minimax
            // 共享 map entry（id() 都返 "minimax"），#2 的 interval 会覆盖
            // 内置那一份。config/enabled/interval/backoff 也用同一套 key。
            let unique = src.unique_id();
            let id_str = unique.as_str();
            let base_id = src.id().into_owned();
            let fallback_interval = cfg0
                .providers
                .get(id_str)
                .or_else(|| cfg0.providers.get(&base_id))
                .and_then(|p| p.refresh_interval_secs)
                .unwrap_or(cfg0.refresh_interval_secs)
                .max(10);
            next_fetch.insert(
                unique,
                Instant::now() + Duration::from_secs(fallback_interval),
            );
        }

        // 每秒检查一次
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
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
            // H2 fix: 清理 next_fetch 里已不存在的 source 条目。
            // delete_extra_instance 后 extras 列表少了条目,poller 不再调度
            // 它,但 next_fetch HashMap 里仍有该 unique_id 的 entry 只增不删,
            // 长时间频繁 add/delete 会泄漏。每次 tick 先算当前所有 unique_id,
            // 把不在 set 里的 entry 从 next_fetch 删掉 (用 retain 一次完成)。
            let live_sources: std::collections::HashSet<String> = {
                let mut set = std::collections::HashSet::new();
                for src in all_sources(&state).await {
                    set.insert(src.unique_id());
                }
                set
            };
            // L2 fix: 跟 try_join_next 的 lock 处理对称 ——
            // 单次 batch 删除所有 stale entries 后立刻 drop 锁,避免保留锁进
            // 长 for 循环(下面 for src 是大批量 source)。
            if next_fetch.len() > live_sources.len() {
                next_fetch.retain(|k, _| live_sources.contains(k));
            }

            for src in all_sources(&state).await {
                // PR 1a：用 unique_id() 做 map key（决策 1：id() 共享 base）
                let unique = src.unique_id();
                let id_owned = src.id().into_owned(); // Cow::Owned 解引用，避开临时 lifetime
                let id_str: &str = &id_owned;
                if !cfg.is_enabled_id(id_str) {
                    continue; // 用户关了，不拉
                }
                // STUB 默认 disabled: 公开 API 无 quota endpoint 的 provider
                // 拉一次就是 30 min 退避。用户没显式
                // 启用 = 跳过,避免浮窗假死。
                if !src.default_enabled() && !cfg.providers.contains_key(id_str) {
                    continue;
                }
                let cfg_interval_secs = cfg
                    .providers
                    .get(id_str)
                    .and_then(|p| p.refresh_interval_secs)
                    .unwrap_or(cfg.refresh_interval_secs)
                    .max(10);
                // 退避后的实际间隔：优先用 backoff 的，没退避用 cfg 默认
                let interval_secs = backoff_snapshot
                    .get(id_str)
                    .copied()
                    .unwrap_or(cfg_interval_secs)
                    .max(10);

                let entry = next_fetch.entry(unique.clone()).or_insert(now);
                if now < *entry {
                    continue; // 还没到点
                }
                // 到点 → 拉这个 provider（独立 task，并发）
                let app_clone = app.clone();
                let unique_owned = unique.clone();
                in_flight()
                    .lock()
                    .unwrap_or_else(|e| {
                        tracing::warn!("poller IN_FLIGHT mutex poisoned (spawn), recovering");
                        e.into_inner()
                    })
                    .spawn(async move {
                        match crate::commands::refresh_single_inner(&app_clone, &unique_owned).await {
                            Ok(()) => {}
                            Err(e) => tracing::warn!(error = %e, provider = %unique_owned, "per-provider 拉取失败"),
                        }
                    });
                *entry = now + Duration::from_secs(interval_secs);
            }
        }
    });
}

/// 手动触发一次（供 tray 菜单和 commands::refresh_now 调用）
pub async fn tick_now(app: &AppHandle) -> Result<(), String> {
    tick(app).await
}

pub async fn tick(app: &AppHandle) -> Result<(), String> {
    let cfg = {
        let state = app.state::<AppState>();
        let cfg = state.config.read().await.clone();
        cfg
    };

    let new_snap = refresh_inner(app, &cfg).await?;

    // 合并写回 state（而不是整块覆写）——
    // refresh_inner 会在内部 emit 一次快照，但那个快照是在 fetch 各 provider
    // 并发期间收集的；如果此时 per-provider poller 的 refresh_single_inner
    // 已经把某个 provider 更新到 state.snapshot 里了，整块覆写会把那份新数据
    // 回滚成 refresh_inner 拿到的旧版本。
    //
    // 正确做法：按 source_id 逐条合并——新数据覆盖旧的，但只动 fetch 到的
    // provider，不碰其他的。
    {
        let state = app.state::<AppState>();
        let mut guard = state.snapshot.write().await;
        for new_p in &new_snap.providers {
            let new_id = new_p.source_id.as_deref().unwrap_or(&new_p.provider);
            if let Some(idx) = guard
                .providers
                .iter()
                .position(|p| p.source_id.as_deref() == Some(new_id))
            {
                guard.providers[idx] = new_p.clone();
            } else {
                guard.providers.push(new_p.clone());
            }
        }
        guard.fetched_at = new_snap.fetched_at;
        // 顶层字段(钱包告警阈值)也要同步——refresh_inner 内部已 populate,
        // 这里只是按 source_id 合并 providers,顶层字段会被忽略,所以手动搬过来。
        guard.wallet_alert_threshold = new_snap.wallet_alert_threshold;
    }

    // 合并后再 emit 一次——refresh_inner 内部 emit 的是它收集的版本，
    // 不含 per-provider poller 在并发期间的中间更新。
    let state = app.state::<AppState>();
    let final_snap = state.snapshot.read().await.clone();
    let _ = app.emit("musage://snapshot", &final_snap);

    Ok(())
}
