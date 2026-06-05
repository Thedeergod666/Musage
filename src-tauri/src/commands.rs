//! 暴露给前端的 tauri commands

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::api::{self, QuotaSnapshot};
use crate::config::{self, AppConfig};
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
    let (api_key, region) = {
        let cfg = state.config.read().await;
        let key = config::load_api_key_from_keyring()?
            .ok_or_else(|| "未配置 API key".to_string())?;
        (key, cfg.region)
    };
    let (_, snap) = api::fetch_quota(&api_key, region).await?;
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
    mut cfg: AppConfig,
) -> Result<(), String> {
    // 区域空时用默认
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
pub async fn has_api_key() -> Result<bool, String> {
    Ok(config::load_api_key_from_keyring()?.is_some())
}

#[tauri::command]
pub async fn set_api_key(key: String) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("key 不能为空".to_string());
    }
    config::save_api_key_to_keyring(trimmed)
}

#[tauri::command]
pub async fn delete_api_key() -> Result<(), String> {
    config::delete_api_key_from_keyring()
}

#[tauri::command]
pub async fn open_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        // 第一次打开：创建窗口
        tauri::WebviewWindowBuilder::new(&app, "settings", tauri::WebviewUrl::App("settings.html".into()))
            .title("Musage · 设置")
            .inner_size(480.0, 520.0)
            .min_inner_size(400.0, 400.0)
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
