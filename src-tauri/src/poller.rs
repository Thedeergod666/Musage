//! 后台轮询：tokio interval，定期拉取并广播到前端 + 刷新托盘
//!
//! Phase 2 (H9) 起改为 per-provider 调度 —— 每个 provider 拿自己的
//! `cfg.providers[id].refresh_interval_secs`（None 时 fallback 到
//! 全局 `cfg.refresh_interval_secs`），独立 sleep + 独立 fetch。
//! 用户可以给不常变动的 provider 设长间隔节流。

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

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

    let snap = refresh_inner(app, &cfg).await?;

    // 写回 state
    {
        let state = app.state::<AppState>();
        let mut guard = state.snapshot.write().await;
        *guard = snap;
    }

    Ok(())
}
