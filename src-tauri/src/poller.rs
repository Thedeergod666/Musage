//! 后台轮询：tokio interval，定期拉取并广播到前端 + 刷新托盘

use std::time::Duration;
use tauri::{AppHandle, Manager};

use crate::commands::refresh_inner;
use crate::AppState;

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动后立即拉一次
        if let Err(e) = tick(&app).await {
            tracing::warn!(error = %e, "初次拉取失败");
        }

        loop {
            // sleep 优先于 interval：interval 的首次 tick 立即 fire，
            // tick() 失败时循环会空转刷日志（实测 ~15ms 一次）。
            let secs = {
                let state = app.state::<AppState>();
                let cfg = state.config.read().await;
                cfg.refresh_interval_secs.max(10)
            };
            tokio::time::sleep(Duration::from_secs(secs)).await;
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
