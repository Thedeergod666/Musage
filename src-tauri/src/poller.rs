//! 后台轮询：tokio interval，定期拉取并广播到前端 + 刷新托盘

use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::{interval, MissedTickBehavior};

use crate::api::fetch_quota;
use crate::config;
use crate::tray;
use crate::AppState;

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动后立即拉一次
        if let Err(e) = tick(&app).await {
            tracing::warn!(error = %e, "初次拉取失败");
        }

        loop {
            let secs = {
                let state = app.state::<AppState>();
                let cfg = state.config.read().await;
                cfg.refresh_interval_secs.max(10)
            };
            let mut ticker = interval(Duration::from_secs(secs));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker.tick().await;
            if let Err(e) = tick(&app).await {
                tracing::warn!(error = %e, "轮询拉取失败");
            }
        }
    });
}

/// 手动触发一次（供 tray 菜单和 commands::refresh_now 调用）
pub async fn tick_now(app: &AppHandle) -> Result<(), String> {
    tick(app).await
}

pub async fn tick(app: &AppHandle) -> Result<(), String> {
    let (api_key, region) = {
        let state = app.state::<AppState>();
        let cfg = state.config.read().await;
        let key = config::load_api_key_from_keyring()
            .map_err(|e| format!("keyring: {e}"))?
            .ok_or_else(|| "未配置 API key".to_string())?;
        (key, cfg.region)
    };

    let (_, snap) = fetch_quota(&api_key, region).await?;
    // 写回 state
    {
        let state = app.state::<AppState>();
        let mut guard = state.snapshot.blocking_write();
        *guard = snap.clone();
    }

    // 通知前端
    let _ = app.emit("musage://snapshot", &snap);

    // 刷新托盘
    if let Err(e) = tray::update_tray_from_snapshot(app, &snap) {
        tracing::warn!(error = %e, "刷新托盘失败");
    }

    Ok(())
}
