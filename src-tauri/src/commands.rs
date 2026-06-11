//! 暴露给前端的 tauri commands
//!
//! 多 provider 模型：
//! - [`refresh_inner`] 是核心实现，被 `refresh_now` (tauri command) 和后台 poller 共用
//! - key 操作按 provider 命名（`has_api_key_for` / `set_api_key_for` / `delete_api_key_for`）

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::config::{self, AppConfig, FloatingPinMode, ProviderOverrides};
use crate::logstore::{LogEntry, LogStore};
use crate::providers::{deepseek, minimax, xiaomi, ErrorKind, Provider, ProviderImpl, ProviderSnapshot, QuotaSnapshot};
use crate::AppState;

/// 把 provider 抛出的中文错误串映射成 [`ErrorKind`]。
///
/// 借鉴 ccswitch 的 `isValid: false` 思路：除了文案还给出机器可读类型，
/// 前端按类型选样式 + 操作按钮。
fn classify_error_message(msg: &str) -> ErrorKind {
    let m = msg;
    if m.contains("API key 为空") || m.contains("未配置 API key") {
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

/// 把一次 provider 失败封装成日志条目写入 [`LogStore`]。
///
/// **所有路径的错误（不仅是 transient）都打日志** —— 用户事后能看到完整 history，
/// 而不是只看到浮窗上的小红点。
///
/// 去重：同一 provider 连续 60s 内相同 `kind` 视为同一次抖动，只记 1 条
/// （避免网络长时间断网时日志被刷爆成 100 条 "network error"）。
fn log_provider_error(log: &LogStore, provider: Provider, kind: ErrorKind, message: &str) {
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};

    // provider + kind → 上次记录时间
    static LAST: OnceLock<Mutex<std::collections::HashMap<(String, &'static str), Instant>>> =
        OnceLock::new();

    let map = LAST.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let key = (provider.id_str().to_string(), kind_label(kind));
    let now = Instant::now();

    let should_log = {
        let mut g = match map.lock() {
            Ok(g) => g,
            Err(_) => return, // 锁毒了不阻塞主流程
        };
        match g.get(&key) {
            Some(prev) if now.duration_since(*prev) < Duration::from_secs(60) => false,
            _ => {
                g.insert(key, now);
                true
            }
        }
    };

    if should_log {
        log.push(LogEntry::error(provider.id_str(), kind_label(kind), message));
    }
}

/// 跟前端 ERROR_LABELS 对齐的小写英文 enum 名（snake_case）。
fn kind_label(k: ErrorKind) -> &'static str {
    match k {
        ErrorKind::UnconfiguredKey => "unconfigured_key",
        ErrorKind::AuthFailed => "auth_failed",
        ErrorKind::RateLimited => "rate_limited",
        ErrorKind::Network => "network",
        ErrorKind::Parse => "parse",
        ErrorKind::SchemaUnknown => "schema_unknown",
        ErrorKind::ServerError => "server_error",
        ErrorKind::Other => "other",
    }
}

// 轻量 Mutex 别名 —— 上面 dedup 表用 std 而不是 tokio 的（不在 async 热路径）
use std::sync::Mutex;

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
///
/// 用 `primary_monitor()` 拿主屏的工作区尺寸（不是窗口尺寸），
/// 减去浮窗的 outer size 除以 2，得到左上角坐标。
#[tauri::command]
pub async fn reset_floating_window(app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("floating")
        .ok_or_else(|| "找不到浮窗".to_string())?;

    let monitor = app
        .primary_monitor()
        .map_err(|e| format!("primary_monitor: {e}"))?
        .ok_or_else(|| "找不到主显示器".to_string())?;

    let mon_size = monitor.size(); // PhysicalSize<u32>
    let mon_pos = monitor.position(); // PhysicalPosition<i32>
    let win_size = win
        .outer_size()
        .map_err(|e| format!("outer_size: {e}"))?;

    let x = mon_pos.x + ((mon_size.width as i32 - win_size.width as i32) / 2).max(0);
    let y = mon_pos.y + ((mon_size.height as i32 - win_size.height as i32) / 2).max(0);

    win.set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| format!("set_position: {e}"))?;

    // 持久化（on_window_event(Moved) 也会触发，但先写一次更稳）
    {
        let state = app.state::<crate::AppState>();
        let mut cfg = state.config.write().await;
        cfg.floating_x = Some(x);
        cfg.floating_y = Some(y);
        let _ = cfg.save();
    }
    Ok(())
}

/// 切换省电模式。即时生效：
/// 1. 把 config.low_power_mode 改掉并落盘
/// 2. emit `musage://low-power-mode-changed` 给浮窗 → 前端切 body[data-low-power]
#[tauri::command]
pub async fn set_low_power_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.low_power_mode != enabled {
            cfg.low_power_mode = enabled;
            let _ = cfg.save();
        }
    }
    let _ = app.emit("musage://low-power-mode-changed", enabled);
    Ok(())
}

/// 切换「全屏时自动隐藏浮窗」。即时生效：
/// 1. 把 config 改掉并落盘
/// 2. 同步给平台层（macOS watcher 用 / 非 macOS no-op）
#[tauri::command]
pub async fn set_auto_hide_in_fullscreen(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.auto_hide_in_fullscreen != enabled {
            cfg.auto_hide_in_fullscreen = enabled;
            let _ = cfg.save();
        }
    }
    crate::platform::set_auto_hide_in_fullscreen(&app, enabled);
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// 返回当前应用版本（来自 tauri.conf.json 的 version 字段）。
///
/// 前端用于：
/// 1. 设置面板底部展示 "Musage v0.1.0"
/// 2. 跟 updater 拉到的远端版本对比时做日志
#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 设置浮窗的置顶/置底模式，同时把模式持久化到 config。
///
/// 模式含义：
/// - `pin_top`   ：浮窗始终在最上层（系统 always-on-top）
/// - `pin_bottom`：默认在底部（不 always-on-top），鼠标 hover 进窗口时由前端
///                 调 [`set_floating_hover_raise`] 临时切到置顶，鼠标离开后还原
/// - `normal`    ：不强制层级，跟普通窗口一样
///
/// 调本命令会**立即**应用效果，并把选择持久化到 config.json（下次启动恢复）。
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
    // 通知浮窗刷新 hover 监听（PinTop/Normal 模式下不应该有 hover 监听）
    let _ = app.emit("musage://pin-mode-changed", &parsed);
    Ok(())
}

/// 浮窗的 hover 状态切换（只在 PinBottom 模式下生效）。
///
/// - `hovering=true`  → 临时把窗口设成 always-on-top
/// - `hovering=false` → 还原成 always-on-top=false（让其它窗口盖住它）
///
/// 平台差异：
/// - 非 macOS：前端 JS 的 `mouseenter`/`mouseleave` 会调本命令，由 platform stub
///   走 Tauri 原生 `set_always_on_top` 切顶层。
/// - macOS：窗口在 `kCGNormalWindowLevel - 1` 时被其它 app 盖住，JS mouseenter
///   触发不到，所以 [`crate::platform::macos`] 启了一个 background thread 轮询
///   `NSEvent.mouseLocation()` + 窗口 `frame`，自己切 level。前端这条信号会被
///   macos stub 忽略（`TRACKER_RUNNING` 才会生效）。
///
/// 在 PinTop / Normal 模式下调用会被忽略（不会破坏其它模式的状态）。
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
        // 非 PinBottom 模式忽略 hover 信号，避免误改置顶状态
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

/// 把 pin 模式应用到浮窗窗口。失败只 warn，不抛错（窗口可能还没建好）。
///
/// macOS 上不能只调 `set_always_on_top(false)` —— 那样的话窗口是
/// `kCGNormalWindowLevel = 0`，前台调度会把其它正在激活的 app 窗口叠上来，
/// 浮窗就"消失"了。所以走 [`crate::platform`] 模块的私有 API 实现：
/// - PinTop   → `kCGFloatingWindowLevel` (3)
/// - PinBottom→ `kCGNormalWindowLevel - 1` (-1) + 启动全局 hover tracker
/// - Normal   → `kCGNormalWindowLevel` (0)
///
/// 非 macOS 平台 (Windows / Linux) stub 走 Tauri 原生 `set_always_on_top`，
/// Windows 上 `set_always_on_top(false)` 配 `HWND_NOTOPMOST` 行为 OK，
/// Linux 上 EWMH 不支持"置底"会降级成普通窗口 —— 已知限制。
pub fn apply_pin_mode_to_window(app: &AppHandle, mode: FloatingPinMode) {
    match mode {
        FloatingPinMode::PinTop => crate::platform::set_window_pin_top(app),
        FloatingPinMode::PinBottom => crate::platform::set_window_pin_bottom(app),
        FloatingPinMode::Normal => crate::platform::set_window_normal(app),
    }
}

// ── 核心实现 ──────────────────────────────────────────────

/// 刷新所有 enabled provider。**并发**跑，互不拖累。
///
/// 被 [`refresh_now`] 和 [`crate::poller::tick`] 共用。
pub async fn refresh_inner(app: &AppHandle, cfg: &AppConfig) -> Result<QuotaSnapshot, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let region = cfg.region();
    let xiaomi_region = cfg.xiaomi_region();
    let enabled = cfg.enabled_providers();

    // 每个 provider 自己的 overrides（用户从设置面板加的字段名候选）
    // 必须 clone 出来再 move 进 spawn（cfg 是引用，spawn 要求 'static）
    let overrides_for = |p: Provider| -> ProviderOverrides {
        cfg.schema_overrides
            .get(p.id_str())
            .cloned()
            .unwrap_or_default()
    };

    // 准备每个 provider 的 fetch 任务（keys.json 读 key 同步，main 里完成避免 spawn 阻塞）
    let mut tasks: Vec<(Provider, tokio::task::JoinHandle<Result<ProviderSnapshot, String>>)> =
        Vec::new();
    for provider in enabled {
        let key_res = config::load_api_key_for(provider);
        match key_res {
            Ok(Some(k)) => {
                let ov = overrides_for(provider);
                let task: tokio::task::JoinHandle<Result<ProviderSnapshot, String>> =
                    tokio::spawn(async move {
                        match provider {
                            Provider::Minimax => {
                                minimax::Minimax::do_fetch(&k, region, &ov)
                                    .await
                                    .map(|(_, snap)| snap)
                            }
                            Provider::Deepseek => {
                                let p = deepseek::Deepseek;
                                <deepseek::Deepseek as ProviderImpl>::fetch(&p, &k).await
                            }
                            Provider::Xiaomimimo => {
                                // Xiaomi 走 dashboard cookie（不是 Bearer/api-key header）
                                let cookie_res = config::load_cookie_for(provider);
                                let snap_res: Result<ProviderSnapshot, String> = match cookie_res {
                                    Ok(Some(cookie)) => {
                                        xiaomi::Xiaomimimo::do_fetch(&cookie, xiaomi_region, &ov)
                                            .await
                                            .map(|(_, snap)| snap)
                                    }
                                    Ok(None) => Err("未配置 Dashboard cookie".to_string()),
                                    Err(e) => Err(e),
                                };
                                snap_res
                            }
                        }
                    });
                tasks.push((provider, task));
            }
            Ok(None) => {
                // key 没配 → 直接给错误快照，不入 task
                tasks.push((
                    provider,
                    tokio::spawn(async move {
                        Err("未配置 API key（设置面板填入）".to_string())
                    }),
                ));
            }
            Err(e) => {
                tasks.push((
                    provider,
                    tokio::spawn(async move { Err(format!("读 keys.json 失败: {e}")) }),
                ));
            }
        }
    }

    // 收集所有结果（保持按 cfg.enabled_providers() 顺序）
    let mut snap = QuotaSnapshot::default();
    let log = app.state::<AppState>().log.clone();
    for (provider, task) in tasks {
        match task.await {
            Ok(Ok(s)) => snap.providers.push(s),
            Ok(Err(e)) => {
                let kind = classify_error_message(&e);
                // 写日志（去重，60s 窗口）。**所有**错误都打，不只是 transient。
                log_provider_error(&log, provider, kind, &e);
                snap.providers.push(ProviderSnapshot::empty_error(provider, kind, e));
            }
            Err(join_err) => {
                let msg = format!("task join 失败: {join_err}");
                log_provider_error(&log, provider, ErrorKind::Other, &msg);
                snap.providers.push(ProviderSnapshot::empty_error(
                    provider,
                    ErrorKind::Other,
                    msg,
                ));
            }
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

// ── 日志模块：暴露给设置面板 ───────────────────────────────

/// 返回最近 n 条日志（默认 100，最多 200）。按时间正序。
///
/// 设置面板打开时一次性拉取 + "刷新" 按钮触发；新产生的事件不主动 push
/// （避免 IPC 噪音 + 双源同步问题）。用户主动 refresh 即可。
#[tauri::command]
pub async fn get_recent_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<LogEntry>, String> {
    let n = limit.unwrap_or(100).min(crate::logstore::max_entries());
    Ok(state.log.recent(Some(n)))
}

/// 清空内存 + 删磁盘文件。设置面板的"清空日志"按钮触发。
#[tauri::command]
pub async fn clear_logs(state: State<'_, AppState>) -> Result<(), String> {
    state.log.clear();
    Ok(())
}
