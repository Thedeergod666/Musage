//! 后台轮询：tokio interval，定期拉取并广播到前端 + 刷新托盘
//!
//! Phase 2 (H9) 起改为 per-provider 调度 —— 每个 provider 拿自己的
//! `cfg.providers[id].refresh_interval_secs`（None 时 fallback 到
//! 全局 `cfg.refresh_interval_secs`），独立 sleep + 独立 fetch。
//! 用户可以给不常变动的 provider 设长间隔节流。

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{refresh_inner, refresh_single};
use crate::providers::builtin_sources;
use crate::AppState;

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
        let cfg0 = app.state::<AppState>().config.read().await.clone();
        let mut next_fetch: HashMap<String, Instant> = HashMap::new();
        for src in builtin_sources() {
            let interval_secs = cfg0
                .providers
                .get(src.id().as_ref())
                .and_then(|p| p.refresh_interval_secs)
                .unwrap_or(cfg0.refresh_interval_secs)
                .max(10);
            next_fetch.insert(
                src.id().to_string(),
                Instant::now() + Duration::from_secs(interval_secs),
            );
        }

        // 每秒检查一次
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let cfg = app.state::<AppState>().config.read().await.clone();
            // 拿 backoff state 的 read guard（在下面循环结束前不释放），
            // `next_interval_secs` 是 &self 方法，guard deref 即可。
            // 注意：先 let-bind state，避开"app.state() 是临时值"的借用问题
            // (State<'_> 在 .await 之后被 drop，guard 借用会失效)
            let state = app.state::<AppState>();
            let backoff_guard = state.backoff.read().await;
            let now = Instant::now();

            for src in builtin_sources() {
                let id = src.id();
                let id_str = id.as_ref();  // Cow → &str，给 is_enabled_id / map.get 用
                if !cfg.is_enabled_id(id_str) {
                    continue;  // 用户关了，不拉
                }
                // STUB 默认 disabled: 公开 API 无 quota endpoint 的 provider
                // (如 novita / qwen) 拉一次就是 30 min 退避。用户没显式
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
                let interval_secs = backoff_guard.next_interval_secs(id_str, cfg_interval_secs);

                let entry = next_fetch.entry(id.to_string()).or_insert(now);
                if now < *entry {
                    continue;  // 还没到点
                }
                // 到点 → 拉这个 provider（独立 task，并发）
                let app_clone = app.clone();
                let id_owned = id.to_string();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::commands::refresh_single_inner(&app_clone, &id_owned).await {
                        tracing::warn!(error = %e, provider = %id_owned, "per-provider 拉取失败");
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
            let new_id = new_p.source_id.as_deref().unwrap_or(new_p.provider.id_str());
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
    }

    // 合并后再 emit 一次——refresh_inner 内部 emit 的是它收集的版本，
    // 不含 per-provider poller 在并发期间的中间更新。
    let state = app.state::<AppState>();
    let final_snap = state.snapshot.read().await.clone();
    let _ = app.emit("musage://snapshot", &final_snap);

    Ok(())
}
