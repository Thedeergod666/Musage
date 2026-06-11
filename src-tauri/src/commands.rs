//! 暴露给前端的 tauri commands
//!
//! ## 双轨制（Phase 1 迁移期）
//!
//! 旧 API（`set_api_key_for(provider: Provider, ...)`）继续存在，给老的 3 个
//! provider（MiniMax / DeepSeek / Xiaomi）用。新 API（`set_source_credential(id: String, ...)`）
//! 走字符串 id，给新的 / 未来的 source（含 Tavily）用。前端优先用新 API。
//!
//! ## 关键路径
//!
//! [`refresh_inner`] 用 [`crate::providers::builtin_sources`] 注册表遍历所有启用的
//! source，每个 source 自己负责鉴权 + 拉数据 + 解析。这是 ROADMAP Phase 1 的核心。
//!
//! [`refresh_now`] 和 [`crate::poller::tick`] 共用 refresh_inner。

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::config::{self, AppConfig, FloatingPinMode};
use crate::providers::{
    builtin_sources, find_source, AuthKind, Credentials, ErrorKind, FetchError, Provider, ProviderSnapshot, QuotaSnapshot, QuotaSource,
};
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
        let mut guard = state.snapshot.write().await;
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
        return Err("轮询间隔不能小于 10 秒（避免触发 provider rate limit）".to_string());
    }
    cfg.save()?;

    // 同步 autostart
    let mgr = app.autolaunch();
    if cfg.autostart {
        mgr.enable().map_err(|e| format!("autostart enable: {e}"))?;
    } else {
        mgr.disable().map_err(|e| format!("autostart disable: {e}"))?;
    }

    // 同步「全屏自动隐藏」开关到平台层（watcher 始终运行，这里翻原子开关）
    crate::platform::set_auto_hide_in_fullscreen(&app, cfg.auto_hide_in_fullscreen);

    // 广播省电模式给浮窗，让前端 toggle body[data-low-power]
    let _ = app.emit("musage://low-power-mode-changed", cfg.low_power_mode);

    {
        let mut guard = state.config.write().await;
        *guard = cfg;
    }
    Ok(())
}

// ── 新 API：按字符串 id 操作（推荐） ──────────────────────────────

/// 注册表元信息：前端拿到后能动态渲染设置面板（避免硬编码 3 个 provider）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMeta {
    pub id: String,
    pub display_name: String,
    /// "api_key" | "cookie"
    pub auth_kind: &'static str,
    pub enabled: bool,
}

/// 列出所有内置 source 的元信息 + 当前启用状态。
#[tauri::command]
pub async fn list_sources(state: State<'_, AppState>) -> Result<Vec<SourceMeta>, String> {
    let cfg = state.config.read().await;
    Ok(builtin_sources()
        .iter()
        .map(|s| SourceMeta {
            id: s.id().to_string(),
            display_name: s.display_name().to_string(),
            auth_kind: match s.auth_kind() {
                AuthKind::ApiKey => "api_key",
                AuthKind::Cookie => "cookie",
            },
            enabled: cfg.is_enabled_id(s.id()),
        })
        .collect())
}

#[tauri::command]
pub async fn has_source_credential(id: String) -> Result<bool, String> {
    // 验证 id 存在（防 IPC 注入任意 key 名）
    let _ = find_source(&id).ok_or_else(|| format!("未知的 source id: {id}"))?;
    Ok(config::load_credential_for_id(&id)?.is_some())
}

#[tauri::command]
pub async fn set_source_credential(id: String, value: String) -> Result<(), String> {
    let src = find_source(&id).ok_or_else(|| format!("未知的 source id: {id}"))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("凭据不能为空".to_string());
    }
    // 鉴权方式决定存哪个字段
    let cred = match src.auth_kind() {
        AuthKind::ApiKey => Credentials { api_key: Some(trimmed.to_string()), cookie: None },
        AuthKind::Cookie => Credentials { api_key: None, cookie: Some(trimmed.to_string()) },
    };
    config::save_credential_for_id(&id, &cred)
}

#[tauri::command]
pub async fn delete_source_credential(id: String) -> Result<(), String> {
    let _ = find_source(&id).ok_or_else(|| format!("未知的 source id: {id}"))?;
    config::delete_credential_for_id(&id)
}

/// 用于设置面板"复制到剪贴板"按钮。返回值仅一次 IPC 用，不在前端持久化。
#[tauri::command]
pub async fn get_source_credential(id: String) -> Result<Option<String>, String> {
    let _ = find_source(&id).ok_or_else(|| format!("未知的 source id: {id}"))?;
    let cred = config::load_credential_for_id(&id)?;
    Ok(cred.and_then(|c| c.api_key.or(c.cookie)))
}

// ── 旧 API：按 Provider enum 操作（保留给现有 UI） ──────────────────

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

#[tauri::command]
pub async fn has_cookie_for(provider: Provider) -> Result<bool, String> {
    Ok(config::load_cookie_for(provider)?.is_some())
}

#[tauri::command]
pub async fn set_cookie_for(provider: Provider, cookie: String) -> Result<(), String> {
    let trimmed = cookie.trim();
    if trimmed.is_empty() {
        return Err("cookie 不能为空".to_string());
    }
    config::save_cookie_for(provider, trimmed)
}

#[tauri::command]
pub async fn delete_cookie_for(provider: Provider) -> Result<(), String> {
    config::delete_cookie_for(provider)
}

/// 从 keys.json 读出明文 key（用于"复制到剪贴板"功能）。
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

/// 浮窗归位到主屏幕正中央，并把位置持久化。
#[tauri::command]
pub async fn reset_floating_window(app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("floating")
        .ok_or_else(|| "找不到浮窗".to_string())?;

    // 优先用 Tauri 内置 center() —— 自己算 monitor 几何的旧实现
    // (commands.rs:209-216 旧版) 有 .max(0) 截断的 bug，多显示器 / 负坐标场景会偏。
    win.center().map_err(|e| format!("center: {e}"))?;

    // 持久化（on_window_event(Moved) 也会触发，但先写一次更稳）
    if let Ok(pos) = win.outer_position() {
        let state = app.state::<crate::AppState>();
        let mut cfg = state.config.write().await;
        cfg.floating_x = Some(pos.x);
        cfg.floating_y = Some(pos.y);
        let _ = cfg.save();
    }
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn set_floating_pin_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    mode: String,
) -> Result<(), String> {
    let parsed = parse_pin_mode(&mode)?;
    apply_pin_mode_to_window(&app, parsed);
    {
        let mut cfg = state.config.write().await;
        if cfg.floating_pin_mode != parsed {
            cfg.floating_pin_mode = parsed;
            let _ = cfg.save();
        }
    }
    let _ = app.emit("musage://pin-mode-changed", &parsed);
    Ok(())
}

#[tauri::command]
pub async fn set_floating_hover_raise(
    state: State<'_, AppState>,
    app: AppHandle,
    hovering: bool,
) -> Result<(), String> {
    let mode = {
        let cfg = state.config.read().await;
        cfg.floating_pin_mode
    };
    if mode != FloatingPinMode::PinBottom {
        return Ok(());
    }
    crate::platform::set_window_hover_raise(&app, hovering);
    Ok(())
}

fn parse_pin_mode(s: &str) -> Result<FloatingPinMode, String> {
    match s {
        "pin_top" | "PinTop" => Ok(FloatingPinMode::PinTop),
        "pin_bottom" | "PinBottom" => Ok(FloatingPinMode::PinBottom),
        "normal" | "Normal" => Ok(FloatingPinMode::Normal),
        other => Err(format!("未知的浮窗置顶模式: {other}")),
    }
}

pub fn apply_pin_mode_to_window(app: &AppHandle, mode: FloatingPinMode) {
    match mode {
        FloatingPinMode::PinTop => crate::platform::set_window_pin_top(app),
        FloatingPinMode::PinBottom => crate::platform::set_window_pin_bottom(app),
        FloatingPinMode::Normal => crate::platform::set_window_normal(app),
    }
}

// ── 核心：refresh_inner ───────────────────────────────────────────

/// 刷新所有启用的 source。**并发**跑，互不拖累。
///
/// 被 [`refresh_now`] 和 [`crate::poller::tick`] 共用。
///
/// Phase 1：每个 source 自己负责鉴权和 fetch，commands.rs 不再 `match provider`。
pub async fn refresh_inner(app: &AppHandle, cfg: &AppConfig) -> Result<QuotaSnapshot, String> {
    // 按 cfg 准备好 sources（避免在 spawn 闭包里 .await 持锁）
    let sources = builtin_sources();
    let mut tasks: Vec<(String, tokio::task::JoinHandle<Result<ProviderSnapshot, String>>)> =
        Vec::new();

    for src in &sources {
        let id = src.id();
        // 跳过未启用的
        if !cfg.is_enabled_id(id) {
            continue;
        }

        // 1. 同步加载凭据（避免在 tokio::spawn 里 await I/O）
        let creds_res = config::load_credential_for_id(id);

        // 2. 让 source 更新自己的 state（region / overrides）
        update_source_state(src, cfg).await;

        match creds_res {
            Ok(Some(creds)) => {
                let id_owned = id.to_string();
                // 注意：每次 fetch 都重新构造 source 实例，但内部 state 是
                // Arc<RwLock> 共享的，所以 region / overrides 不会丢。
                let src_box: Box<dyn QuotaSource> = builtin_sources()
                    .into_iter()
                    .find(|s| s.id() == id)
                    .expect("source still registered");
                let task: tokio::task::JoinHandle<Result<ProviderSnapshot, String>> =
                    tokio::spawn(async move {
                        match src_box.fetch(&creds).await {
                            Ok(snap) => Ok(snap),
                            Err(e) => Err(e.message),  // message 给前端看，kind 由 classify 还原
                        }
                    });
                tasks.push((id_owned, task));
            }
            Ok(None) => {
                let id_owned = id.to_string();
                let task = tokio::spawn(async move {
                    Err("未配置凭据（设置面板填入）".to_string())
                });
                tasks.push((id_owned, task));
            }
            Err(e) => {
                let id_owned = id.to_string();
                let task = tokio::spawn(async move {
                    Err(format!("读 keys.json 失败: {e}"))
                });
                tasks.push((id_owned, task));
            }
        }
    }

    // 收集所有结果（按 builtin_sources 顺序，前端卡顺序稳定）
    let mut snap = QuotaSnapshot::default();
    for (id, task) in tasks {
        match task.await {
            Ok(Ok(s)) => snap.providers.push(s),
            Ok(Err(e)) => {
                let provider = provider_from_id(&id);
                snap.providers.push(ProviderSnapshot::empty_error(provider, classify_error_message(&e), e));
            }
            Err(join_err) => {
                let provider = provider_from_id(&id);
                snap.providers.push(ProviderSnapshot::empty_error(
                    provider,
                    ErrorKind::Other,
                    msg,
                ));
            }
        }
    }

    snap.fetched_at = Some(chrono::Utc::now().timestamp_millis());

    // 刷新托盘 + 推送
    let _ = app.emit("musage://snapshot", &snap);
    if let Err(e) = crate::tray::update_tray_from_snapshot(app, &snap) {
        tracing::warn!(error = %e, "刷新托盘失败");
    }

    Ok(snap)
}

/// 把 provider id 映射到 Provider enum（仅供空错误快照用，UI 仍以 source_id 为准）。
fn provider_from_id(id: &str) -> Provider {
    match id {
        "minimax" => Provider::Minimax,
        "deepseek" => Provider::Deepseek,
        "xiaomimimo" => Provider::Xiaomimimo,
        _ => Provider::Minimax,  // 占位，Phase 2 加 Tavily 变体
    }
}

/// 在 fetch 前把 cfg 里的 region / overrides 推给 source（如果 source 实现了的话）。
///
/// 公开给 [`crate::lib::run_dump_subcommand`] 共享。
pub async fn update_source_state(src: &Box<dyn QuotaSource>, cfg: &AppConfig) {
    // 把整个 cfg 序列化成 JSON，让 source 自己按需取字段
    let cfg_json = match serde_json::to_value(cfg) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "序列化 AppConfig 失败，跳过 set_state");
            return;
        }
    };
    src.set_state(cfg_json).await;
}

/// 把 provider 抛出的中文错误串映射成 [`ErrorKind`]。
///
/// Phase 1 起，理想情况下 provider 返回的是 [`FetchError`]（带 kind），但
/// 为了保持 refresh_inner 的鲁棒性，这里仍然对最终的中文消息做兜底分类。
fn classify_error_message(msg: &str) -> ErrorKind {
    let m = msg;
    if m.contains("API key 为空") || m.contains("未配置") {
        ErrorKind::UnconfiguredKey
    } else if m.contains("鉴权失败") || m.contains("无权限") || m.contains("HTTP 401") || m.contains("HTTP 403") {
        ErrorKind::AuthFailed
    } else if m.contains("频繁") || m.contains("HTTP 429") {
        ErrorKind::RateLimited
    } else if m.starts_with("网络错误") || m.contains("网络错误") {
        ErrorKind::Network
    } else if m.contains("不是 JSON") {
        ErrorKind::Parse
    } else if m.contains("未识别 schema") {
        ErrorKind::SchemaUnknown
    } else if m.contains("服务异常") || m.contains("HTTP 5") {
        ErrorKind::ServerError
    } else {
        ErrorKind::Other
    }
}
