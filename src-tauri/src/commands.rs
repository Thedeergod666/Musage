//! 暴露给前端的 tauri commands
//!
//! 多 provider 模型：
//! - [`refresh_inner`] 是核心实现，被 `refresh_now` (tauri command) 和后台 poller 共用
//! - key 操作按 provider 命名（`has_api_key_for` / `set_api_key_for` / `delete_api_key_for`）

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::config::{self, AppConfig};
use crate::providers::{deepseek, minimax, Provider, ProviderImpl, ProviderSnapshot, QuotaSnapshot};
use crate::AppState;

#[tauri::command]
pub async fn get_snapshot(state: State<'_, AppState>) -> Result<QuotaSnapshot, String> {
    Ok(state.snapshot.read().await.clone())
}

#[tauri::command]
pub async fn refresh_now(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<QuotaSnapshot, String> {
    let cfg = state.config.read().await.clone();
    let snap = refresh_inner(&app, &cfg).await?;
    {
        let mut guard = state.snapshot.blocking_write();
        *guard = snap.clone();
    }
    let _ = app.emit("musage://snapshot", &snap);
    if let Err(e) = crate::tray::update_tray_from_snapshot(&app, &snap) {
        tracing::warn!(error = %e, "刷新托盘失败");
    }
    Ok(snap)
}

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    Ok(state.config.read().await.clone())
}

#[tauri::command]
pub async fn save_config(
    state: State<'_, AppState>,
    app: AppHandle,
    cfg: AppConfig,
) -> Result<(), String> {
    let mut cfg = cfg;
    if cfg.refresh_interval_secs < 10 {
        cfg.refresh_interval_secs = 10;
    }
    cfg.save()?;

    // 同步 autostart
    let mgr = app.autolaunch();
    if cfg.autostart {
        mgr.enable().map_err(|e| format!("autostart enable: {e}"))?;
    } else {
        mgr.disable().map_err(|e| format!("autostart disable: {e}"))?;
    }

    {
        let mut guard = state.config.blocking_write();
        *guard = cfg;
    }
    Ok(())
}

#[tauri::command]
pub async fn has_api_key_for(provider: Provider) -> Result<bool, String> {
    Ok(config::load_api_key_for(provider)?.is_some())
}

#[tauri::command]
pub async fn set_api_key_for(provider: Provider, key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("key 不能为空".to_string());
    }
    config::save_api_key_for(provider, trimmed)
}

#[tauri::command]
pub async fn delete_api_key_for(provider: Provider) -> Result<(), String> {
    config::delete_api_key_for(provider)
}

/// 从 keyring 读出明文 key（用于"复制到剪贴板"功能）。
/// 前端不会保存返回值，只用一次写剪贴板后丢弃。
#[tauri::command]
pub async fn get_api_key_for(provider: Provider) -> Result<Option<String>, String> {
    config::load_api_key_for(provider)
}

#[tauri::command]
pub async fn open_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        tauri::WebviewWindowBuilder::new(
            &app,
            "settings",
            tauri::WebviewUrl::App("settings.html".into()),
        )
        .title("Musage · 设置")
        .inner_size(540.0, 620.0)
        .min_inner_size(440.0, 500.0)
        .resizable(true)
        .decorations(true)
        .skip_taskbar(true)
        .center()
        .build()
        .map_err(|e| format!("create settings: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn hide_floating_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("floating") {
        let _ = w.hide();
    }
    Ok(())
}

#[tauri::command]
pub async fn show_floating_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("floating") {
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn hide_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.hide();
    }
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ── 核心实现 ──────────────────────────────────────────────

/// 刷新所有 enabled provider。失败不阻断其他。
///
/// 被 [`refresh_now`] 和 [`crate::poller::tick`] 共用。
pub async fn refresh_inner(app: &AppHandle, cfg: &AppConfig) -> Result<QuotaSnapshot, String> {
    let mut snap = QuotaSnapshot::default();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let region = cfg.region();

    for provider in cfg.enabled_providers() {
        let key = match config::load_api_key_for(provider) {
            Ok(Some(k)) => k,
            Ok(None) => {
                snap.providers.push(ProviderSnapshot::empty_error(
                    provider,
                    "未配置 API key（设置面板填入）".to_string(),
                ));
                continue;
            }
            Err(e) => {
                snap.providers.push(ProviderSnapshot::empty_error(
                    provider,
                    format!("读 keyring 失败: {e}"),
                ));
                continue;
            }
        };

        let result: Result<ProviderSnapshot, String> = match provider {
            Provider::Minimax => {
                let r = minimax::Minimax::do_fetch(&key, region).await;
                r.map(|(_, snap)| snap)
            }
            Provider::Deepseek => {
                let p = deepseek::Deepseek;
                <deepseek::Deepseek as ProviderImpl>::fetch(&p, &key).await
            }
        };

        match result {
            Ok(s) => snap.providers.push(s),
            Err(e) => snap.providers.push(ProviderSnapshot::empty_error(provider, e)),
        }
    }

    snap.fetched_at = Some(now_ms);

    // 刷新托盘 + 推送
    let _ = app.emit("musage://snapshot", &snap);
    if let Err(e) = crate::tray::update_tray_from_snapshot(app, &snap) {
        tracing::warn!(error = %e, "刷新托盘失败");
    }

    Ok(snap)
}
