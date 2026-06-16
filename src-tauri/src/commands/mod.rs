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
//!
//! PR 3：custom_sources 子模块装 5 个用户自定义 New API source 的 IPC。
//! 拆出子模块是因为 `commands/mod.rs` 本身已经 1200+ 行。

pub mod custom_sources;
pub mod i18n;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::config::{self, AppConfig, FloatingPinMode, ProviderConfig, TrayIconStyle, UserRegion};
use crate::providers::{
    builtin_sources, find_source, AuthKind, Credentials, ErrorKind, FetchError, Provider, ProviderSnapshot, QuotaSnapshot, QuotaSource,
};
use crate::providers::minimax::Region as MinimaxRegion;
use crate::providers::xiaomi::XiaomiDisplayMode;
use crate::AppState;
use crate::t;

/// 立即更新 provider 顺序 + 落盘 + emit config-changed（前端调，无需走
/// save_config 全量保存）。前端用这个实现「↑↓ 按钮即时生效」。
#[tauri::command]
pub async fn set_provider_order(
    state: State<'_, AppState>,
    app: AppHandle,
    order: Vec<String>,
) -> Result<(), String> {
    // 先保存 config（释放 write lock），再读 + 重排 snapshot。
    // 如果先持有 config.write 再拿 snapshot.write，会和
    // refresh_single_inner（先拿 snapshot.write 再拿 config.read）死锁。
    {
        let mut cfg = state.config.write().await;
        cfg.provider_order = order;
        cfg.save()?;
    }
    // 重排 in-memory snapshot 并 emit 给浮窗，让浮窗立刻按新顺序渲染。
    //
    // ⚠️ 关键：必须先 drop cfg_snap 和 snap 两个锁，再 emit。
    // 如果持有锁期间 emit，refresh_single_inner 同时拿 snapshot.write
    // 会死锁 → emit 永远发不出 → 浮窗永远不刷新。
    {
        let cfg_snap = state.config.read().await;
        let mut snap = state.snapshot.write().await;
        apply_provider_order(&mut snap, &cfg_snap);
        let s = snap.clone();
        drop(snap);
        drop(cfg_snap);
        let _ = app.emit("musage://snapshot", &s);
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 立即更新单个 provider 的 enabled 标志 + 落盘 + emit。供设置面板
/// 「在浮窗显示 X」复选框 onchange 即时调用。
#[tauri::command]
pub async fn set_provider_enabled(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        // 缺 key 时插一份默认配置（保持 BTreeMap key 顺序 + 默认值）
        let entry = cfg
            .providers
            .entry(id.clone())
            .or_insert(crate::config::ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            });
        entry.enabled = enabled;
        cfg.save()?;
    }
    // 如果用户关掉了某个 provider，立刻清掉它在 in-memory snapshot 里
    // 的条目（不然浮窗下次刷新前还会显示旧数据）。
    if !enabled {
        let state_arc = app.state::<AppState>();
        let mut snap = state_arc.snapshot.write().await;
        snap.providers.retain(|p| {
            p.source_id.as_deref().unwrap_or(p.provider.id_str()) != id
        });
        let emit_snap = snap.clone();
        drop(snap);
        // 排序 + emit
        let cfg2 = state_arc.config.read().await;
        let mut emit = emit_snap;
        apply_provider_order(&mut emit, &cfg2);
        drop(cfg2);
        let _ = app.emit("musage://snapshot", &emit);
    } else {
        // 重新拉一次这个 provider（用户刚开就显示数据）
        let _ = refresh_single_inner(&app, &id).await;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 即时切换 Xiaomi MiMo 浮窗显示模式：完整 / 只套餐 / 只总额度。
///
/// 走单字段 command 路径（参考 `set_provider_enabled`），不走 `save_config` 全量保存。
/// 保存后立即 refresh 一次（poller 下一分钟才 fire，user 等不了）。
#[tauri::command]
pub async fn set_xiaomi_display_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    mode: String,
) -> Result<(), String> {
    let parsed = match mode.as_str() {
        "all" => XiaomiDisplayMode::All,
        "plan_only" => XiaomiDisplayMode::PlanOnly,
        "total_only" => XiaomiDisplayMode::TotalOnly,
        other => return Err(t!("commands.display_mode_unknown_xiaomi", other = other).into_owned()),
    };
    {
        let mut cfg = state.config.write().await;
        let entry = cfg
            .providers
            .entry("xiaomimimo".to_string())
            .or_insert(ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            });
        entry.xiaomi_display_mode = Some(parsed);
        cfg.save()?;
    }
    // 立即刷新（让浮窗按新模式显示）
    let _ = refresh_single_inner(&app, "xiaomimimo").await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 读 Xiaomi 当前显示模式（给设置面板初始化用）。
#[tauri::command]
pub async fn get_xiaomi_display_mode(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = state.config.read().await;
    let mode = cfg
        .providers
        .get("xiaomimimo")
        .and_then(|p| p.xiaomi_display_mode)
        .unwrap_or_default();
    Ok(match mode {
        XiaomiDisplayMode::All => "all".to_string(),
        XiaomiDisplayMode::PlanOnly => "plan_only".to_string(),
        XiaomiDisplayMode::TotalOnly => "total_only".to_string(),
    })
}

#[tauri::command]
pub async fn get_snapshot(state: State<'_, AppState>) -> Result<QuotaSnapshot, String> {
    let snap = state.snapshot.read().await.clone();
    let cfg = state.config.read().await;
    // 过滤被关掉的 provider —— 设置面板的「在浮窗显示 X」开关关闭后，
    // 浮窗不应该再看到这张卡。poller 自己也会跳过 disabled，但旧的成功
    // 数据还留在 vecdeque 里，所以需要在这里也过滤一次。
    let mut filtered = snap;
    filtered.providers.retain(|p| {
        let id = p.source_id.as_deref().unwrap_or(p.provider.id_str());
        cfg.is_enabled_id(id)
    });
    // 按用户配置的 provider_order 排序（空 = 用 builtin_sources() 顺序）
    apply_provider_order(&mut filtered, &cfg);
    Ok(filtered)
}

#[tauri::command]
pub async fn refresh_now(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<QuotaSnapshot, String> {
    let cfg = state.config.read().await.clone();
    let snap = refresh_inner(&app, &cfg).await?;
    // 合并写回 state（而不是整块覆写）—— 跟 tick() 同理：
    // refresh_inner 并发拉所有 provider 的过程中，per-provider poller 可能已经
    // 把某个 provider 更新到 state.snapshot 里了；整块覆写会把那份新数据回滚。
    {
        let mut guard = state.snapshot.write().await;
        for new_p in &snap.providers {
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
        guard.fetched_at = snap.fetched_at;
    }
    // refresh_inner 内部已经 emit 过一次，这里再 emit 合并后的完整快照
    // （refresh_inner emit 的是它自己收集的版本，不含 per-provider 的中间更新）
    let state2 = app.state::<AppState>();
    let final_snap = state2.snapshot.read().await.clone();
    let _ = app.emit("musage://snapshot", &final_snap);
    let tray_style = cfg.tray_icon_style;
    if let Err(e) = crate::tray::update_tray_from_snapshot(&app, &final_snap, tray_style) {
        tracing::warn!(error = %e, "刷新托盘失败");
    }
    Ok(final_snap)
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
    let cfg = cfg;
    if cfg.refresh_interval_secs < 10 {
        return Err(t!("commands.interval_too_small").into_owned());
    }
    // 校验色阈值（settings 面板的保存路径也要兜底 —— 即使用户绕过 set_display_thresholds
    // 直接调 save_config 也会在这里被挡）
    let [t0, t1, t2] = cfg.color_thresholds;
    if !(0 < t0 && t0 < t1 && t1 < t2 && t2 < 100) {
        return Err(t!("commands.threshold_invalid", t0 = t0, t1 = t1, t2 = t2).into_owned());
    }
    if let Some(n) = cfg.wallet_alert_threshold {
        if !(n.is_finite() && n >= 0.0) {
            return Err(t!("commands.wallet_threshold_negative", n = n).into_owned());
        }
    }
    // 校验自定义色（同 set_display_thresholds 路径）
    for (k, v) in &cfg.color_overrides {
        match k.as_str() {
            "ok" | "cyan" | "warn" | "alert" => {}
            other => {
                return Err(t!("commands.color_key_unknown", other = other).into_owned());
            }
        }
        if !is_valid_hex_color(v) {
            return Err(t!("commands.color_value_invalid", k = k.as_str(), v = v.as_str()).into_owned());
        }
    }
    cfg.save()?;

    // 同步 autostart
    let mgr = app.autolaunch();
    if cfg.autostart {
        mgr.enable().map_err(|e| t!("commands.autostart_enable", err = e.to_string()).into_owned())?;
    } else {
        mgr.disable().map_err(|e| t!("commands.autostart_disable", err = e.to_string()).into_owned())?;
    }

    // 同步「全屏自动隐藏」开关到平台层（watcher 始终运行，这里翻原子开关）
    crate::platform::set_auto_hide_in_fullscreen(&app, cfg.auto_hide_in_fullscreen);

    // 广播省电模式给浮窗，让前端 toggle body[data-low-power]
    let _ = app.emit("musage://low-power-mode-changed", cfg.low_power_mode);

    {
        let mut guard = state.config.write().await;
        *guard = cfg;
    }
    // 广播「配置变了」给浮窗，让浮窗按需 re-fetch（比如 Tavily 简洁模式开关）
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

// ── 新 API：按字符串 id 操作（推荐） ──────────────────────────────

/// 注册表元信息：前端拿到后能动态渲染设置面板（避免硬编码 3 个 provider）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMeta {
    pub id: String,
    pub display_name: String,
    /// "api_key" | "cookie" | "api_key_or_cookie"
    pub auth_kind: &'static str,
    pub enabled: bool,
    /// true = 主面板不渲染凭据字段（移至"高级"tab）。Xiaomi 用：
    /// API key 对 Bearer 永远 401，手动 cookie 是兜底，都放高级 tab。
    #[serde(default)]
    pub hide_credentials: bool,
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
                AuthKind::ApiKeyOrCookie => "api_key_or_cookie",
            },
            enabled: cfg.is_enabled_id(s.id().as_ref()),
            // Xiaomi: API key (Bearer) 永远 401，手动 cookie 是兜底 → 都放高级 tab
            hide_credentials: s.id() == "xiaomimimo",
        })
        .collect())
}

#[tauri::command]
pub async fn has_source_credential(
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    // 验证 id 存在（防 IPC 注入任意 key 名）
    let _ = find_source(&state, &id).await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    Ok(config::load_credential_for_id(&id)?.is_some())
}

#[tauri::command]
pub async fn set_source_credential(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    value: String,
    // 可选：明确指定这个 value 落到哪个字段（"api_key" / "cookie"）。
    // 不传时按 source 的 `auth_kind()` 默认：
    //   ApiKey / ApiKeyOrCookie → api_key
    //   Cookie                   → cookie
    // 多鉴权 source（ApiKeyOrCookie）必须传 field hint，
    // 否则两个输入框都保存到 api_key，cookie 永远落不进去。
    field: Option<String>,
) -> Result<(), String> {
    let src = find_source(&state, &id).await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(t!("commands.credential_empty").into_owned());
    }
    let cred = build_credentials(&src, trimmed, field.as_deref())?;
    config::save_credential_for_id(&id, &cred)?;
    tracing::debug!(provider = %id, field = ?field, "set_source_credential: saved to keys.json");
    // 关键：用户刚配完 key 浮窗应当立刻看到数据。per-provider 调度最早
    // 在下一分钟才 fire（启动时初始化为 now+interval），不手动拉一次用户得
    // 等 1 分钟甚至更久。refresh_single_inner 内部会更新 in-memory
    // snapshot + emit，浮窗自动跟着变。
    let enabled = state.config.read().await.is_enabled_id(&id);
    tracing::debug!(provider = %id, enabled, "set_source_credential: refresh decision");
    if enabled {
        if let Err(e) = refresh_single_inner(&app, &id).await {
            tracing::warn!(error = %e, provider = %id, "set_source_credential 后立即拉取失败（不阻塞保存）");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 把 "value 落到 Credentials 哪个字段" 这条规则集中到一处。
///
/// `field` 取值：
/// - `Some("api_key")` / `Some("cookie")` → 强制指定
/// - `None` → 按 source 的 auth_kind 默认
/// - `Some(其他)` → 报错（避免 typo 默默走错字段）
fn build_credentials(
    src: &Box<dyn QuotaSource>,
    value: &str,
    field: Option<&str>,
) -> Result<Credentials, String> {
    let target = match field {
        Some("api_key") => "api_key",
        Some("cookie") => "cookie",
        Some(other) => return Err(t!("commands.field_unknown", other = other).into_owned()),
        None => match src.auth_kind() {
            AuthKind::ApiKey | AuthKind::ApiKeyOrCookie => "api_key",
            AuthKind::Cookie => "cookie",
        },
    };
    Ok(match target {
        "api_key" => Credentials { api_key: Some(value.to_string()), cookie: None },
        "cookie" => Credentials { api_key: None, cookie: Some(value.to_string()) },
        _ => unreachable!(),
    })
}

#[tauri::command]
pub async fn delete_source_credential(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let _ = find_source(&state, &id).await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    config::delete_credential_for_id(&id)?;
    // 跟 set_source_credential 对称：删了 key 浮窗应该立刻看到 "未配置"
    // 错误态，而不是等下一次 poller 周期。
    let enabled = state.config.read().await.is_enabled_id(&id);
    if enabled {
        if let Err(e) = refresh_single_inner(&app, &id).await {
            tracing::warn!(error = %e, provider = %id, "delete 后立即拉取失败");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 用于设置面板"复制到剪贴板"按钮。返回值仅一次 IPC 用，不在前端持久化。
#[tauri::command]
pub async fn get_source_credential(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<String>, String> {
    let _ = find_source(&state, &id).await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    let cred = config::load_credential_for_id(&id)?;
    Ok(cred.and_then(|c| c.api_key.or(c.cookie)))
}

// ── 旧 API：按 Provider enum 操作（保留给现有 UI） ──────────────────

#[tauri::command]
pub async fn has_api_key_for(provider: Provider) -> Result<bool, String> {
    Ok(config::load_api_key_for(provider)?.is_some())
}

#[tauri::command]
pub async fn set_api_key_for(
    state: State<'_, AppState>,
    app: AppHandle,
    provider: Provider,
    key: String,
) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err(t!("commands.api_key_empty").into_owned());
    }
    config::save_api_key_for(provider, trimmed)?;
    // 跟 set_source_credential 一致：保存后立即拉一次，浮窗即时看到
    let id = provider.id_str();
    let enabled = state.config.read().await.is_enabled_id(id);
    if enabled {
        if let Err(e) = refresh_single_inner(&app, id).await {
            tracing::warn!(error = %e, provider = %id, "set_api_key_for 后立即拉取失败");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_api_key_for(
    state: State<'_, AppState>,
    app: AppHandle,
    provider: Provider,
) -> Result<(), String> {
    config::delete_api_key_for(provider)?;
    let id = provider.id_str();
    let enabled = state.config.read().await.is_enabled_id(id);
    if enabled {
        if let Err(e) = refresh_single_inner(&app, id).await {
            tracing::warn!(error = %e, provider = %id, "delete_api_key_for 后立即拉取失败");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn has_cookie_for(provider: Provider) -> Result<bool, String> {
    Ok(config::load_cookie_for(provider)?.is_some())
}

#[tauri::command]
pub async fn set_cookie_for(
    state: State<'_, AppState>,
    app: AppHandle,
    provider: Provider,
    cookie: String,
) -> Result<(), String> {
    let trimmed = cookie.trim();
    if trimmed.is_empty() {
        return Err(t!("commands.cookie_empty").into_owned());
    }
    config::save_cookie_for(provider, trimmed)?;
    let id = provider.id_str();
    let enabled = state.config.read().await.is_enabled_id(id);
    if enabled {
        if let Err(e) = refresh_single_inner(&app, id).await {
            tracing::warn!(error = %e, provider = %id, "set_cookie_for 后立即拉取失败");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn delete_cookie_for(
    state: State<'_, AppState>,
    app: AppHandle,
    provider: Provider,
) -> Result<(), String> {
    config::delete_cookie_for(provider)?;
    let id = provider.id_str();
    let enabled = state.config.read().await.is_enabled_id(id);
    if enabled {
        if let Err(e) = refresh_single_inner(&app, id).await {
            tracing::warn!(error = %e, provider = %id, "delete_cookie_for 后立即拉取失败");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 从 keys.json 读出明文 key（用于"复制到剪贴板"功能）。
/// 前端不会保存返回值，只用一次写剪贴板后丢弃。
#[tauri::command]
pub async fn get_api_key_for(provider: Provider) -> Result<Option<String>, String> {
    config::load_api_key_for(provider)
}

/// 设置窗 builder —— commands.rs 的 open_settings_window + lib.rs 的首启引导
/// 都走这里，防止两处 builder 配置漂移（之前是 byte-for-byte 复制两份）。
///
/// **Win11 闪白修复（2026-06-11）**：不设 background_color 时，WebView2 surface
/// 在第一帧 HTML/CSS 抵达前是系统默认白色，而 settings.css 的 body 背景是
/// `#1a1c22`，用户看到的是「白窗 → 一帧后变深色 = 闪一下」。`background_color`
/// 在 native 层（窗口 chrome + WebView2 surface）就预先涂成 `#1a1c22`，HTML
/// 还没解析的那几十毫秒里画的就是深色，肉眼无感。注意：Windows 8+ 上
/// `Color` 的 alpha 通道会被 webview 层忽略（见 tauri_utils config 注释），
/// `0xff` 只是给阅读代码的人看的。
pub(crate) fn build_settings_window(
    app: &AppHandle,
) -> tauri::Result<tauri::WebviewWindow> {
    let bg = tauri::webview::Color(0x1a, 0x1c, 0x22, 0xff);
    tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title("Musage · 设置")
    .inner_size(780.0, 680.0)
    .min_inner_size(720.0, 600.0)
    .resizable(true)
    .decorations(true)
    // **任务栏映射**：设置窗才是用户面对的 app 窗口，应该出现在 Win 任务栏
    // （这样 ALT+TAB / 任务栏右键能正常操作，icon 也走 bundle.icon）。
    // 浮窗在 tauri.conf.json 里设了 skipTaskbar:true（小悬浮 overlay 不该
    // 出现在任务栏）—— 两侧必须保持一反一正，否则 Win 用户会看到一个
    // "Musage" 任务栏条目对应错误的窗口。
    .skip_taskbar(false)
    .center()
    .background_color(bg)
    .build()
}

#[tauri::command]
pub async fn open_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        // Win11 已存在窗口的恢复链：unminimize 必须在 show 之前 ——
        // Win 上 show() 对 minimized 窗口是 no-op（不会自动 SW_RESTORE），
        // 不 unminimize 的话用户最小化设置窗后再从托盘点"设置"会以为
        // 命令死了。set_focus 收尾把窗口拉前台 + 抢焦点。
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        build_settings_window(&app).map_err(|e| t!("commands.create_settings", err = e.to_string()).into_owned())?;
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
        // 与 open_settings_window 同样的"先 unminimize 再 show"链 —— 即使
        // 浮窗 decorations:false 没有最小化按钮，WIN+M / 命令行也能最小化。
        let _ = w.unminimize();
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
        .ok_or_else(|| t!("commands.floating_not_found").into_owned())?;

    // 优先用 Tauri 内置 center() —— 自己算 monitor 几何的旧实现
    // (commands.rs:209-216 旧版) 有 .max(0) 截断的 bug，多显示器 / 负坐标场景会偏。
    win.center().map_err(|e| t!("commands.center_failed", err = e.to_string()).into_owned())?;

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
        other => Err(t!("commands.pin_mode_unknown", other = other).into_owned()),
    }
}

/// 调整浮窗高度以适配内容（前端在 render 后调用）。
///
/// 浮窗默认 height=100，多 provider 全堆一起会装不下 —— 用户手动拉能拉
/// 一点但 maxHeight 也会卡。改用这个 command 在每次 render 后把窗口
/// resize 到内容实际需要的高度（限在 tauri.conf.json 的 minHeight=100 /
/// maxHeight=2400 范围内）。auto-resize 跟手拉并存：手拉的尺寸会被 debounced
/// 写盘，但下一次 render 又会贴内容。H5。
///
/// **maxHeight 为什么是 2400**：8+ provider 全开（旧上限 800 装不下 → 用户
/// 反馈底部卡片被截）。2400 logical 像素覆盖到 4K 工作区（2160p ≈ 2000+ 可用）。
/// 真正"别超出屏幕"的兜底由前端 `screen.availHeight - 80` 处理 —— 后端这层只
/// 是 OS 硬上限的镜像，避免 Tauri 把窗口拉到天文数字。
///
/// **`height` 是 logical / CSS 像素**（前端读 `app.scrollHeight` 拿到的就是
/// 这个单位）。Tauri 2 在 macOS / Win / Linux 各自对 `set_size(LogicalSize)`
/// 的处理一致 —— 内部转物理像素，避免前端用 `scale_factor` 手算带来的舍入误差。
/// 之前的 `set_size(PhysicalSize::new(w, height*scale))` 在 Retina 上若 scale
/// 算错就会比预期高 1px，再叠加前端的 +1 就会造成 [H5 静置越长越高] 的反馈环。
#[tauri::command]
pub async fn resize_floating_window(app: AppHandle, height: f64) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("floating") {
        // 保留用户当前的宽度（auto-resize 只调高度，不动宽 —— 宽度由用户拖控制）
        // 用 inner_size 的 logical 版本，绕开 macOS 上 outer/inner 的细微差
        let cur_logical: tauri::LogicalSize<f64> =
            w.inner_size().map_err(|e| t!("commands.size_failed", err = e.to_string()).into_owned())?.to_logical(
                w.scale_factor().unwrap_or(1.0),
            );
        let width = cur_logical.width;
        // 限高 —— 必须与 tauri.conf.json 的 minHeight/maxHeight 同步，否则
        // Tauri 会把后端 set_size 拽回 conf 设的范围 → "前端给 1500 但窗口还是 800"。
        // 真正"别超出 monitor 工作区"由前端 `screen.availHeight` 兜底。
        let height = height.clamp(100.0, 2400.0);
        let _ = w.set_size(tauri::LogicalSize::new(width, height));
    }
    Ok(())
}

pub fn apply_pin_mode_to_window(app: &AppHandle, mode: FloatingPinMode) {
    match mode {
        FloatingPinMode::PinTop => crate::platform::set_window_pin_top(app),
        FloatingPinMode::PinBottom => crate::platform::set_window_pin_bottom(app),
        FloatingPinMode::Normal => crate::platform::set_window_normal(app),
    }
}

/// P2 区域向导：用户选定区域后 apply 该区域的默认 provider 顺序 + 默认
/// endpoint（MiniMax/Zhipu CN/EN），并把 user_region 标为 Custom
/// （之后用户手动改顺序/endpoint 不会触发 wizard 重新弹出）。
#[tauri::command]
pub async fn set_region(
    state: State<'_, AppState>,
    app: AppHandle,
    region: String,
) -> Result<(), String> {
    let parsed = match region.as_str() {
        "cn" => UserRegion::Cn,
        "global" => UserRegion::Global,
        "custom" => UserRegion::Custom,
        other => return Err(t!("commands.region_invalid", other = other).into_owned()),
    };

    let default_order: Vec<String> = parsed.default_provider_order()
        .iter().map(|s| s.to_string()).collect();

    {
        let mut cfg = state.config.write().await;
        // 1. apply 默认 provider 顺序（仅在当前是 default empty 时覆盖）
        if cfg.provider_order.is_empty() {
            cfg.provider_order = default_order;
        }
        // 2. apply 默认 endpoint（MiniMax 跟 region 走；zhipu 当前 schema
        // 缺独立 field，靠 cfg.pointer(\"/providers/zhipu/region\") 读，
        // 不在 ProviderConfig 里 —— TODO v2 加 zhipu_region 字段）
        if parsed == UserRegion::Global {
            if let Some(mm) = cfg.providers.get_mut("minimax") {
                mm.region = Some(MinimaxRegion::En);
            }
        } else {
            // Cn (默认) —— 显式归位 CN
            if let Some(mm) = cfg.providers.get_mut("minimax") {
                mm.region = Some(MinimaxRegion::Cn);
            }
        }
        // 3. 标 user_region 为 Custom（之后用户手动改任何字段都不会触发 wizard）
        cfg.user_region = UserRegion::Custom;
        cfg.save()?;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 取当前 user_region（给前端决定是否显示 wizard）
#[tauri::command]
pub async fn get_region(state: State<'_, AppState>) -> Result<String, String> {
    let region = match state.config.read().await.user_region {
        UserRegion::Cn => "cn",
        UserRegion::Global => "global",
        UserRegion::Custom => "custom",
    };
    Ok(region.to_string())
}

// ── 核心：refresh_inner ───────────────────────────────────────

/// 刷新所有启用的 source。**并发**跑，互不拖累。
///
/// 被 [`refresh_now`] 和 [`crate::poller::tick`] 共用。
///
/// Phase 1：每个 source 自己负责鉴权和 fetch，commands.rs 不再 `match provider`。
pub async fn refresh_inner(app: &AppHandle, cfg: &AppConfig) -> Result<QuotaSnapshot, String> {
    // 按 cfg 准备好 sources（避免在 spawn 闭包里 .await 持锁）
    let sources = builtin_sources();
    // P1 重构：closure 返 FetchError 而不是 String，kind 在 collect 时直接拿。
    let mut tasks: Vec<(String, u64, tokio::task::JoinHandle<Result<ProviderSnapshot, FetchError>>)> =
        Vec::new();

    for src in &sources {
        let id = src.id();
        let id_str = id.as_ref();  // Cow<'_, str> → &str，避免在循环里反复 .as_ref()
        // 跳过未启用的
        if !cfg.is_enabled_id(id_str) {
            continue;
        }

        // 默认间隔（per-provider override 优先）—— backoff 写入时用
        let default_interval_secs = cfg
            .providers
            .get(id_str)
            .and_then(|p| p.refresh_interval_secs)
            .unwrap_or(cfg.refresh_interval_secs)
            .max(10);

        // 1. 同步加载凭据（避免在 tokio::spawn 里 await I/O）
        let creds_res = config::load_credential_for_id(id_str);
        tracing::trace!(provider = %id, has_creds = creds_res.as_ref().ok().and_then(|c| c.as_ref()).is_some(), "refresh_inner load_credential");

        match creds_res {
            Ok(Some(creds)) => {
                let id_owned = id.to_string();
                // 每次 fetch 都重新构造 source 实例 —— builtin_sources() 内部
                // 走 `Box::new(XxxSource::default())`，每次都产生**全新**的
                // `Arc<RwLock<state>>`，跟外层 `src` 的 state 不是同一份。
                // 所以 set_state 必须推给真正用于 fetch 的 `src_box`，而不是
                // 循环变量 `src`（早期代码注释误以为"内部 state 是 Arc<RwLock
                // 共享的"，实际不共享 —— 症状：用户在设置面板切到 Xiaomi
                // 显示模式 "all" 后保存，托盘右键"立即刷新"又把模式拉回默认
                // "total_only"，因为 fetch 用的是新建 src_box 的默认空 state）。
                let src_box: Box<dyn QuotaSource> = builtin_sources()
                    .into_iter()
                    .find(|s| s.id() == id)
                    .expect("source still registered");
                update_source_state(&src_box, cfg).await;
                // P1 重构：返回 FetchError 而不是 String，kind 在 closure 内就
                // 保留住，collect 时直接 e.kind 拿（不再走 classify_error_message）。
                let task: tokio::task::JoinHandle<Result<ProviderSnapshot, FetchError>> =
                    tokio::spawn(async move {
                        src_box.fetch(&creds).await
                    });
                tasks.push((id_owned, default_interval_secs, task));
            }
            Ok(None) => {
                let id_owned = id.to_string();
                let task = tokio::spawn(async move {
                    Err(FetchError::unconfigured("未配置凭据（设置面板填入）"))
                });
                tasks.push((id_owned, default_interval_secs, task));
            }
            Err(e) => {
                let id_owned = id.to_string();
                let task = tokio::spawn(async move {
                    // 读 keys.json 失败归到 Network（IO 错误类），不归到 Other
                    // 让前端能正确分类显示
                    Err(FetchError::network(format!("读 keys.json 失败: {e}")))
                });
                tasks.push((id_owned, default_interval_secs, task));
            }
        }
    }

    // 收集所有结果（按 builtin_sources 顺序，前端卡顺序稳定）
    let mut snap = QuotaSnapshot::default();
    for (id, default_interval_secs, task) in tasks {
        match task.await {
            Ok(Ok(s)) => {
                // 写 backoff：成功 → reset 退避状态
                {
                    let state = app.state::<AppState>();
                    let mut backoff = state.backoff.write().await;
                    backoff.record(&id, &s, default_interval_secs);
                }
                snap.providers.push(s);
            }
            Ok(Err(e)) => {
                // P1 重构：kind 直接从 FetchError 取，不再走 classify_error_message
                // 子串匹配（旧实现 i18n 一动就破）。
                let provider = provider_from_id(&id);
                log_provider_error(app, &id, e.kind, &e.message);
                let err_snap = ProviderSnapshot::empty_error(
                    &app.state::<AppState>(),
                    provider,
                    &id,
                    e.kind,
                    e.message,
                ).await;
                // 写 backoff：失败（如果 kind 属于可退避类）→ 翻倍
                {
                    let state = app.state::<AppState>();
                    let mut backoff = state.backoff.write().await;
                    backoff.record(&id, &err_snap, default_interval_secs);
                }
                snap.providers.push(err_snap);
            }
            Err(join_err) => {
                let provider = provider_from_id(&id);
                let msg = format!("任务调度失败: {join_err}");
                log_provider_error(app, &id, ErrorKind::Other, &msg);
                snap.providers.push(
                    ProviderSnapshot::empty_error(
                        &app.state::<AppState>(),
                        provider,
                        &id,
                        ErrorKind::Other,
                        msg,
                    ).await,
                );
            }
        }
    }

    snap.fetched_at = Some(chrono::Utc::now().timestamp_millis());

    // 过滤 + 排序 + 推送
    let state = app.state::<AppState>();
    let cfg_read = state.config.read().await;
    snap.providers.retain(|p| {
        let id = p.source_id.as_deref().unwrap_or(p.provider.id_str());
        cfg_read.is_enabled_id(id)
    });
    apply_provider_order(&mut snap, &cfg_read);
    let tray_style = cfg_read.tray_icon_style;
    drop(cfg_read);
    // 刷新托盘 + 推送
    let _ = app.emit("musage://snapshot", &snap);
    if let Err(e) = crate::tray::update_tray_from_snapshot(app, &snap, tray_style) {
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

/// 按 AppConfig.provider_order 给 snapshot.providers 排序。
/// - provider_order 为空 → 不动（保留 builtin_sources() 注册表顺序）
/// - 非空 → 按用户在设置面板拖拽/上下按钮指定的顺序排
///   不在 order 里的 provider 沉到末尾（usize::MAX）—— 防止用户
///   删掉一个 provider 后剩下的"消失"。
fn apply_provider_order(snap: &mut QuotaSnapshot, cfg: &AppConfig) {
    if cfg.provider_order.is_empty() {
        return;
    }
    snap.providers.sort_by_key(|p| {
        let source_id = p.source_id.as_deref().unwrap_or(p.provider.id_str());
        cfg.provider_order
            .iter()
            .position(|o| o == source_id)
            .unwrap_or(usize::MAX)
    });
}

/// 拉取单个 provider —— 供 poller 的 per-provider 调度使用（H9）。
///
/// 不重新跑全部 enabled source，只跑指定的一个；fetch 完成后
/// 替换 in-memory snapshot 里对应那条，再 emit + 刷新托盘。
/// 这样每个 provider 可以有自己的轮询间隔。
#[tauri::command]
pub async fn refresh_single(app: AppHandle, id: String) -> Result<(), String> {
    refresh_single_inner(&app, &id).await
}

pub async fn refresh_single_inner(app: &AppHandle, id: &str) -> Result<(), String> {
    let cfg = app.state::<AppState>().config.read().await.clone();
    if !cfg.is_enabled_id(id) {
        return Ok(());  // 已被关掉，跳过
    }
    let src = builtin_sources()
        .into_iter()
        .find(|s| s.id() == id)
        .ok_or_else(|| format!("未知的 source id: {id}"))?;
    let creds = config::load_credential_for_id(id)?;
    update_source_state(&src, &cfg).await;
    let provider_snap = match creds {
        Some(c) => match src.fetch(&c).await {
            Ok(s) => s,
            Err(e) => {
                let provider = provider_from_id(id);
                let kind = e.kind;
                log_provider_error(app, id, kind, &e.message);
                ProviderSnapshot::empty_error(
                    &app.state::<AppState>(),
                    provider,
                    id,
                    kind,
                    e.message,
                ).await
            }
        },
        None => {
            let provider = provider_from_id(id);
            let kind = ErrorKind::UnconfiguredKey;
            let msg = "未配置凭据（设置面板填入）".to_string();
            log_provider_error(app, id, kind, &msg);
            ProviderSnapshot::empty_error(
                &app.state::<AppState>(),
                provider,
                id,
                kind,
                msg,
            ).await
        }
    };

    // 写 backoff：让 poller 下次调度知道这个 provider 是不是该延长间隔
    // (失败 → 翻倍；成功 → reset)。详见 `poller_backoff::BackoffState::record`。
    let default_interval_secs = cfg
        .providers
        .get(id)
        .and_then(|p| p.refresh_interval_secs)
        .unwrap_or(cfg.refresh_interval_secs)
        .max(10);
    {
        let state = app.state::<AppState>();
        let mut backoff = state.backoff.write().await;
        backoff.record(id, &provider_snap, default_interval_secs);
    }

    // 替换 in-memory snapshot 里对应那条
    let state = app.state::<AppState>();
    let mut snap = state.snapshot.write().await;
    let source_id = provider_snap
        .source_id
        .clone()
        .unwrap_or_else(|| id.to_string());
    // H10 修复：只按 source_id 匹配 —— 旧实现 fallback 到 provider.id_str()
    // 会跟 Tavily 这类「复用 Provider::Minimax 占位」的 source 撞：per-provider
    // poller 跟全量 refresh_inner 并发时，Tavily 的 fetch 找到 minimax 位置并
    // 替换，全量后又加一个新 Tavily → 浮窗两个 Tavily 卡片。
    if let Some(idx) = snap
        .providers
        .iter()
        .position(|p| p.source_id.as_deref() == Some(source_id.as_str()))
    {
        snap.providers[idx] = provider_snap;
    } else {
        snap.providers.push(provider_snap);
    }
    snap.fetched_at = Some(chrono::Utc::now().timestamp_millis());
    drop(snap);

    // 重新读最新 config（可能用户在两次 fetch 之间改了 enabled/order），
    // 过滤 + 排序后再 emit
    let state = app.state::<AppState>();
    let cfg2 = state.config.read().await;
    let cfg2_snapshot = cfg2.clone();
    drop(cfg2);
    let mut snap = state.snapshot.write().await;
    snap.providers.retain(|p| {
        let id = p.source_id.as_deref().unwrap_or(p.provider.id_str());
        cfg2_snapshot.is_enabled_id(id)
    });
    apply_provider_order(&mut snap, &cfg2_snapshot);
    let tray_style = cfg2_snapshot.tray_icon_style;
    let emit_snap = snap.clone();
    drop(snap);
    let _ = app.emit("musage://snapshot", &emit_snap);
    if let Err(e) = crate::tray::update_tray_from_snapshot(app, &emit_snap, tray_style) {
        tracing::warn!(error = %e, "刷新托盘失败 (refresh_single)");
    }
    Ok(())
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
/// P1 错误分类重构：删了。
/// 旧实现对中文字符串做子串匹配（鉴权失败 / 网络错误 / ...），i18n 一动
/// （Rust 错误消息改 tr!() 走 en.json）就全破。
/// 现在 refresh_inner closure 直接返回 [`FetchError`]（带 kind），
/// 这里不再需要兜底分类。详见 `refresh_inner` L774 注释。
#[allow(dead_code)]
fn _classify_error_message_removed(_msg: &str) -> ErrorKind {
    // 保留一个占位 stub 防止别处误引用（编译期 dead_code 警告，不影响产物）。
    ErrorKind::Other
}

// ── 日志：错误事件下沉到 LogStore ────────────────────────────────────
//
// 设计要点（commit 3d5ee5d）：
// - refresh_inner 每个失败的 provider 都打一条 LogEntry::error
// - 60s 去重窗口（同 provider + 同 kind）避免长断网刷爆日志
// - 浮窗 UI 此时只翻红点，rowsBox 仍保留最后一次成功的数据
// - 设置面板通过 `get_recent_logs` 拉取查看，`clear_logs` 清空

/// (provider_id, kind_short_label) → 上次写日志的毫秒时间戳。
/// 在 60s 窗口内的同 key 错误被吞掉，不重复写。
fn dedup_cache() -> &'static std::sync::Mutex<std::collections::HashMap<(String, &'static str), i64>> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<(String, &'static str), i64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const LOG_DEDUP_WINDOW_MS: i64 = 60_000;

/// 把一次 provider 拉取失败写进 [`crate::logstore::LogStore`]。
///
/// 同 (provider_id, kind) 在 60s 窗口内只保留第一条，避免长断网刷爆 ring buffer。
/// IO 失败 / mutex 中毒都不阻塞调用方 —— 这是热路径的旁路。
fn log_provider_error(app: &AppHandle, provider_id: &str, kind: ErrorKind, message: &str) {
    let now = chrono::Utc::now().timestamp_millis();
    // H12 宽限期：用户刚点过「清空」日志的 60s 内，所有新错误一律不写
    // —— 让用户真切看到「已清空」状态，不被立刻涌出的新错误淹没。
    if is_in_clear_grace(now) {
        return;
    }
    // P1 重构：用 ErrorKind::as_str()（snake_case）作为 dedup key —— 跟 serde
    // 序列化的形式一致，i18n 切换不会破坏去重窗口。
    let key = (provider_id.to_string(), kind.as_str());

    // 去重判断：拿锁尽量短
    {
        let mut g = match dedup_cache().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),  // 中毒也继续，日志比强一致重要
        };
        if let Some(&last_ts) = g.get(&key) {
            if now - last_ts < LOG_DEDUP_WINDOW_MS {
                return;
            }
        }
        g.insert(key, now);
    }

    let state = app.state::<AppState>();
    state.log.push(crate::logstore::LogEntry::error(
        provider_id,
        kind.as_str(),
        message,
    ));
}

// ── 设置面板"即时生效"command 群 ──────────────────────────────────
//
// 设置面板"勾选即生效 / 切 radio 即生效"那条路不依赖 `save_config` 全量保存。
// 每个 command 自己：写 cfg + 落盘 + 必要时 emit 给浮窗 / 调 platform 层。
//
// 修复原 settings.ts:978-997 调 `set_low_power_mode` / `set_auto_hide_in_fullscreen`
// 但后端没注册 → 死按钮（catch 吞错）的 bug。

/// 即时切换省电模式：写 cfg + emit `musage://low-power-mode-changed` 给浮窗
/// 让它 toggle body[data-low-power]（styles.css 切玻璃材质）。
#[tauri::command]
pub async fn set_low_power_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.low_power_mode == enabled {
            return Ok(());
        }
        cfg.low_power_mode = enabled;
        cfg.save()?;
    }
    let _ = app.emit("musage://low-power-mode-changed", enabled);
    Ok(())
}

/// 即时切换"全屏时自动隐藏浮窗"：写 cfg + 同步给 platform 层的原子开关
/// （watcher 始终运行，仅翻开关）。
#[tauri::command]
pub async fn set_auto_hide_in_fullscreen(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.auto_hide_in_fullscreen == enabled {
            return Ok(());
        }
        cfg.auto_hide_in_fullscreen = enabled;
        cfg.save()?;
    }
    crate::platform::set_auto_hide_in_fullscreen(&app, enabled);
    Ok(())
}

/// 即时切换浮窗底部提示行显隐：写 cfg + emit config-changed 让浮窗重读。
#[tauri::command]
pub async fn set_show_footer_hint(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.show_footer_hint == enabled {
            return Ok(());
        }
        cfg.show_footer_hint = enabled;
        cfg.save()?;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 即时切换托盘图标样式：写 cfg + 立即用新 style 重渲托盘（不等下次 poller）。
#[tauri::command]
pub async fn set_tray_icon_style(
    state: State<'_, AppState>,
    app: AppHandle,
    style: TrayIconStyle,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.tray_icon_style == style {
            return Ok(());
        }
        cfg.tray_icon_style = style;
        cfg.save()?;
    }
    // 立即重渲（不阻塞 cmd 返回）
    let state2 = app.state::<AppState>();
    let snap = state2.snapshot.read().await.clone();
    if let Err(e) = crate::tray::update_tray_from_snapshot(&app, &snap, style) {
        tracing::warn!(error = %e, "切换托盘样式后重渲失败");
    }
    Ok(())
}

/// 即时更新"显示阈值"：色档分界 [ok/cyan/warn/alert] + 钱包余额告警阈值 +
/// 4 档自定义色。
///
/// 走单字段 command 路径（参考 `set_provider_enabled` / `set_tray_icon_style`），
/// 不走 `save_config` 全量保存。写 cfg + 落盘 + emit `config-changed` 让浮窗
/// 重新渲染（颜色立刻反映新阈值/新色）。
///
/// 校验：
/// - `color_thresholds`：3 个 u8，必须 0 < t0 < t1 < t2 < 100
/// - `wallet_alert_threshold`：None 关闭；Some(n) 要求 n >= 0
/// - `color_overrides`：只允许 key ∈ {ok, cyan, warn, alert}，value 必须是
///   `#RGB` / `#RRGGBB` 形式的 hex（与 `<input type="color">` 输出一致）；
///   其他 key 一律 reject（防 typo 默默走默认）
#[tauri::command]
pub async fn set_display_thresholds(
    state: State<'_, AppState>,
    app: AppHandle,
    color_thresholds: [u8; 3],
    wallet_alert_threshold: Option<f64>,
    color_overrides: std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let [t0, t1, t2] = color_thresholds;
    if !(0 < t0 && t0 < t1 && t1 < t2 && t2 < 100) {
        return Err(t!("commands.threshold_invalid", t0 = t0, t1 = t1, t2 = t2).into_owned());
    }
    if let Some(n) = wallet_alert_threshold {
        if !(n.is_finite() && n >= 0.0) {
            return Err(t!("commands.wallet_threshold_negative", n = n).into_owned());
        }
    }
    for (k, v) in &color_overrides {
        match k.as_str() {
            "ok" | "cyan" | "warn" | "alert" => {}
            other => {
                return Err(t!("commands.color_key_unknown", other = other).into_owned());
            }
        }
        if !is_valid_hex_color(v) {
            return Err(t!("commands.color_value_invalid", k = k.as_str(), v = v.as_str()).into_owned());
        }
    }
    {
        let mut cfg = state.config.write().await;
        cfg.color_thresholds = color_thresholds;
        cfg.wallet_alert_threshold = wallet_alert_threshold;
        cfg.color_overrides = color_overrides;
        cfg.save()?;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 校验 CSS 颜色串：`#RGB` / `#RRGGBB` 形式的 hex（区分大小写不敏感）。
/// 与 `<input type="color">` 的输出格式严格对齐。
fn is_valid_hex_color(s: &str) -> bool {
    let s = s.trim();
    if !s.starts_with('#') {
        return false;
    }
    let hex = &s[1..];
    matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// 设置面板「📋 日志」拉取最近 N 条（最新在末尾）。
///
/// `limit` 上限被裁到 [`crate::logstore::max_entries`]，防止前端乱传 100000。
#[tauri::command]
pub fn get_recent_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Vec<crate::logstore::LogEntry> {
    let cap = crate::logstore::max_entries();
    let n = limit.map(|l| l.min(cap));
    state.log.recent(n)
}

/// 设置面板「清空」按钮：清内存 + 删 jsonl 文件。**保留** dedup 缓存 + 加
/// 60s 宽限期：
/// - dedup 保留 → 用户清完 log 1s 后 poller 跑出同 (provider, kind) 错误
///   会被 60s 去重窗口吞掉，不刷出新日志
/// - 宽限期 60s → 期间所有新错误一律不写（即使不同 kind）
/// 两个机制叠加让用户真切看到「已清空」状态（1 分钟内），不被立刻涌出的
/// 新错误淹没。
const LOG_CLEAR_GRACE_MS: i64 = 60_000;
static LAST_CLEAR_TS: std::sync::Mutex<Option<i64>> = std::sync::Mutex::new(None);

pub(crate) fn is_in_clear_grace(now_ms: i64) -> bool {
    let g = match LAST_CLEAR_TS.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    match *g {
        Some(t) if now_ms - t < LOG_CLEAR_GRACE_MS => true,
        _ => false,
    }
}

#[tauri::command]
pub fn clear_logs(state: State<'_, AppState>) {
    state.log.clear();
    if let Ok(mut g) = dedup_cache().lock() {
        g.clear();
    }
    // 记下清空时间戳，宽限期内 log_provider_error 直接 return
    if let Ok(mut g) = LAST_CLEAR_TS.lock() {
        *g = Some(chrono::Utc::now().timestamp_millis());
    }
}
