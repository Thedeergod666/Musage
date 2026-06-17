//! Musage - 多 Provider 实时用量监控
//!
//! 架构：
//! - `providers` : 多 provider 抽象（trait + minimax/deepseek 实现）
//! - `config`    : 多 provider 配置 + 本地 keys.json 存取
//! - `poller`    : tokio 后台定时拉取
//! - `commands`  : tauri::command 暴露给前端
//! - `tray`      : 系统托盘 + 动态图标
//!
//! CLI 子命令：
//! - (无)           : 启动 GUI（Tauri 默认行为）
//! - `dump`         : 拉一次全部 provider 并打印原始 JSON + 解析结果
//! - `dump <id>`    : 只拉某个 provider（`minimax` / `deepseek`）

mod commands;
mod config;
mod logstore;
mod platform;
mod poller;
mod poller_backoff;
mod providers;
mod tray;
mod xiaomi_login;

// P0 国际化：编译期展开 tr!() / t!() macro 时需要知道 locale 文件路径。
// 必须在 `mod` 声明之后、其他文件 `use rust_i18n` 之前。
// `fallback = "en"`：找不到 key 时退到英文（开发期抓漏翻的护栏）。
rust_i18n::i18n!("locales", fallback = "en");
// 把 t! / tr! macro 重新导出到 crate 根，让所有模块不用各自 import。
pub use rust_i18n::t;

use std::sync::Arc;
use tauri::{Listener, Manager};
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::logstore::LogStore;
use crate::poller_backoff::BackoffState;
use crate::providers::{builtin_sources, CustomSourceSpec, QuotaSnapshot};
use crate::commands::apply_pin_mode_to_window;

pub struct AppState {
    pub snapshot: Arc<RwLock<QuotaSnapshot>>,
    pub config: Arc<RwLock<AppConfig>>,
    /// 应用运行日志（错误/警告/信息），详见 [`crate::logstore`]
    pub log: Arc<LogStore>,
    /// Poller per-provider 指数退避状态。
    /// - 写：每次 fetch 完（`refresh_inner` / `refresh_single_inner`）
    /// - 读：poller 调度 tick 时算下次间隔
    /// 详见 [`crate::poller_backoff`]
    pub backoff: Arc<RwLock<BackoffState>>,
    /// PR 3：用户自定义 New API sources。启动时从 `custom_sources.json` load。
    /// 写：add/update/delete_custom_source IPC 命令（会 persist）。
    /// 读：providers::all_sources / find_source 拼 customs 进去。
    pub custom_sources: Arc<RwLock<Vec<CustomSourceSpec>>>,
}

/// 前端调用的"切换语言"命令。实现见 [`crate::commands::i18n::set_app_locale`]。
/// 这里只 re-export 给 `tauri::generate_handler!` 用。
pub use crate::commands::i18n::set_app_locale;
pub use crate::commands::i18n::get_app_locale;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,musage=debug")),
        )
        .with_target(false)
        .compact()
        .init();

    // CLI 分流：dump 子命令不进 GUI
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "dump" {
        std::process::exit(run_dump_subcommand(args.get(2).map(|s| s.as_str())));
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // 启动时清理上次崩溃留下的孤儿 .tmp 文件（F2/M10 修复连带）
            config::cleanup_orphan_tmp_files();
            // 读取配置（load_from_disk 已会在损坏时备份到 .bak.<ts>，不再静默吞掉）
            let config = AppConfig::load(&app.handle()).unwrap_or_default();

            // P0：把 cfg.locale 推到 rust_i18n 的进程内 state，让 tr!() 立刻生效。
            // 必须在 .setup 里最早做（之后 tr!() 才会拿到正确 locale）。
            rust_i18n::set_locale(&config.locale);

            let cfg_handle = app.state::<AppState>();

            // 写入初始状态
            {
                let mut guard = cfg_handle.config.blocking_write();
                *guard = config;
            }

            // P0：监听 locale-changed 事件 → 重建 tray menu（用新 locale 的 label）
            // + 同步 settings / xiaomi 窗口 title。
            // 用 cloned AppHandle 在闭包外 spawn 一个长生命周期监听。
            let app_for_locale = app.handle().clone();
            app.listen("musage://locale-changed", move |event| {
                if let Ok(locale) = serde_json::from_str::<String>(event.payload()) {
                    rust_i18n::set_locale(&locale);
                    // 重建 tray menu（label 走 tr!()，新 locale 立刻生效）
                    if let Err(e) = crate::tray::rebuild_tray(&app_for_locale) {
                        tracing::warn!(error = %e, "rebuild_tray 失败");
                    }
                    // 同步 settings 窗口 title
                    if let Some(w) = app_for_locale.get_webview_window("settings") {
                        let title = t!("window.settings").to_string();
                        let _ = w.set_title(&title);
                    }
                    if let Some(w) = app_for_locale.get_webview_window("xiaomi-login") {
                        let title = t!("window.xiaomi_login").to_string();
                        let _ = w.set_title(&title);
                    }
                }
            });

            // 启动后台轮询
            poller::start(app.handle().clone());

            // 启动 hover emitter：始终运行，不管 pin mode 是哪个
            // macOS 上这是绕过 WKWebView "非 key window 不分发 mouseMoved"
            // 的关键 —— Rust 端轮询全局鼠标位置 → emit 给前端 → 切
            // body[data-hover] → CSS 玻璃 hover 效果不依赖窗口焦点。
            // 非 macOS 平台是 no-op stub。
            crate::platform::start_hover_emitter(app.handle().clone());

            // 启动 fullscreen watcher（macOS 探测菜单栏可见性 → 自动隐藏浮窗）。
            // 非 macOS 是 no-op。watcher 自身始终运行，是否真的隐藏看 config
            // 的 auto_hide_in_fullscreen，这个开关由下面 from-config 同步到平台层。
            crate::platform::start_fullscreen_watcher(app.handle().clone());

            // 初始化托盘
            tray::setup(app.handle())?;

            // 恢复浮窗位置 + 大小（用户上次拖/拉过的话）
            // 必须在 show() 之前调用，否则会有 1 帧错位
            if let Some(win) = app.get_webview_window("floating") {
                let cfg = cfg_handle.config.blocking_read().clone();
                let scale = win.scale_factor().unwrap_or(1.0);

                // "有效位置"判定：saved x/y 都存在 + 不在 OS 默认左上角
                // —— 老用户首次跑（升级前）的位置被 OS 放 (0,0) 附近并被
                // Moved 事件持久化下来，留在 config.json 里。如果直接当
                // "有保存位置" 恢复，新行为（top-right）永远触发不到。
                // 把 (<= 50, <= 50) 视作"未设置" + 走 top-right。
                let saved_pos_valid = matches!(
                    (cfg.floating_x, cfg.floating_y),
                    (Some(x), Some(y)) if x > 50 || y > 50
                );

                if saved_pos_valid {
                    if let (Some(x), Some(y)) = (cfg.floating_x, cfg.floating_y) {
                        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                    }
                } else {
                    // 首次启动 / 老用户的 OS 默认位置 → 默认放到主屏幕右上角
                    // （系统托盘浮窗惯例：右上角，距离顶/右各 10px，避开 macOS 菜单栏）
                    if let Ok(Some(monitor)) = app.primary_monitor() {
                        let mon_size = monitor.size();
                        let mon_pos = monitor.position();
                        let cur_w = win
                            .outer_size()
                            .map(|s| s.width as i32)
                            .unwrap_or((300.0 * scale) as i32);
                        let margin = (10.0 * scale) as i32;
                        let x = mon_pos.x + mon_size.width as i32 - cur_w - margin;
                        let y = mon_pos.y + margin;
                        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                    }
                }

                if let (Some(w), Some(h)) = (cfg.floating_w, cfg.floating_h) {
                    // 尊重 tauri.conf.json 里的 minWidth/minHeight（保持同步）
                    let min_w = (180.0 * scale) as u32;
                    let min_h = (100.0 * scale) as u32;
                    let ww = w.max(min_w as i32) as u32;
                    let hh = h.max(min_h as i32) as u32;
                    let _ = win.set_size(tauri::PhysicalSize::new(ww, hh));
                }

                // 恢复浮窗的置顶/置底模式（用户上次选的）
                apply_pin_mode_to_window(app.handle(), cfg.floating_pin_mode);

                // 同步「全屏自动隐藏」开关到平台层（watcher 已经启动，这里只翻开关）
                crate::platform::set_auto_hide_in_fullscreen(
                    app.handle(),
                    cfg.auto_hide_in_fullscreen,
                );

                // 监听移动 / 缩放 → 持久化（**H1 修复**：debounce 到 500ms，
                // 否则拖 300px 会 spawn 300 个 tokio 任务 + 写 config.json 300 次）
                let app_for_event = app.handle().clone();
                spawn_debounced_geom_persister(app_for_event, win.clone());
            }

            // 默认显示悬浮窗
            if let Some(win) = app.get_webview_window("floating") {
                let _ = win.show();
                let _ = win.set_focus();
            }

            // 首次启动引导：所有 provider 都没配 key → 自动弹设置窗口
            let any_key = builtin_sources().iter().any(|src| {
                config::load_credential_for_id(src.id().as_ref())
                    .ok()
                    .flatten()
                    .is_some()
            });
            if !any_key {
                // 首启引导：走和 open_settings_window 同一个 builder，避免
                // 两处配置漂移（窗口大小 / decorations / background_color 等
                // 必须一致，否则两个入口的设置窗看上去会不一样）。
                let _ = commands::build_settings_window(app.handle());
            }

            Ok(())
        })
        .manage(AppState {
            snapshot: Arc::new(RwLock::new(QuotaSnapshot::default())),
            config: Arc::new(RwLock::new(AppConfig::default())),
            // 从磁盘 reload 最近 200 条 —— 启动时一次性 IO，不在热路径
            log: Arc::new(LogStore::load_from_disk()),
            backoff: Arc::new(RwLock::new(BackoffState::new())),
            // PR 3：custom_sources 启动 load。load 失败时返空 Vec（不阻塞启动）。
            custom_sources: Arc::new(RwLock::new(
                config::custom_sources::load_custom_sources().unwrap_or_default(),
            )),
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh_now,
            commands::get_config,
            commands::save_config,
            commands::list_sources,
            commands::has_source_credential,
            commands::set_source_credential,
            commands::delete_source_credential,
            commands::get_source_credential,
            commands::has_api_key_for,
            commands::set_api_key_for,
            commands::delete_api_key_for,
            commands::get_api_key_for,
            commands::has_cookie_for,
            commands::set_cookie_for,
            commands::delete_cookie_for,
            commands::open_settings_window,
            commands::hide_floating_window,
            commands::show_floating_window,
            commands::hide_settings_window,
            commands::reset_floating_window,
            commands::set_floating_pin_mode,
            commands::set_floating_hover_raise,
            commands::resize_floating_window,
            commands::refresh_single,
            commands::set_provider_order,
            commands::set_provider_enabled,
            commands::set_xiaomi_display_mode,
            commands::get_xiaomi_display_mode,
            commands::set_low_power_mode,
            commands::set_auto_hide_in_fullscreen,
            commands::set_show_footer_hint,
            commands::set_tray_icon_style,
            commands::set_display_thresholds,
            commands::quit_app,
            commands::get_app_version,
            commands::get_recent_logs,
            commands::clear_logs,
            // P0 国际化：locale 切换（persistence + 事件 + rust_i18n set_locale）
            set_app_locale,
            get_app_locale,
            // P2 区域向导：用户选 cn/global 后 apply 默认 provider 顺序 + endpoint
            commands::set_region,
            commands::get_region,
            // PR 3: 用户自定义 New API source (5 commands)
            commands::custom_sources::list_custom_sources,
            commands::custom_sources::add_custom_source,
            commands::custom_sources::update_custom_source,
            commands::custom_sources::delete_custom_source,
            commands::custom_sources::test_custom_source,
            xiaomi_login::open_xiaomi_login_window,
        ])
        .on_window_event(|window, event| {
            // 关闭悬浮窗时拦截，避免退出整个 app
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "floating" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 启动一个后台任务，把浮窗的"位置/大小"事件 debounce 到 500ms 再写 config.json。
///
/// 修复 H1：原来每个像素的 `WindowEvent::Moved` 都 spawn 一个 tokio 任务并立即
/// `save()` —— 拖 300px 会 spawn 300 个任务 + 写 300 次 config.json。
/// 现在的策略：回调里只更新共享状态，background task 定时（500ms）检查并落盘。
fn spawn_debounced_geom_persister(
    app: tauri::AppHandle,
    win: tauri::WebviewWindow,
) {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let latest: Arc<Mutex<Option<(i32, i32, i32, i32)>>> = Arc::new(Mutex::new(None));
    let latest_for_cb = latest.clone();

    // 回调线程：只更新 latest，不做 I/O
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(pos) => {
            let mut g = latest_for_cb.lock().unwrap();
            let cur = g.unwrap_or((pos.x, pos.y, 0, 0));
            *g = Some((pos.x, pos.y, cur.2, cur.3));
        }
        tauri::WindowEvent::Resized(size) => {
            let mut g = latest_for_cb.lock().unwrap();
            let cur = g.unwrap_or((0, 0, size.width as i32, size.height as i32));
            *g = Some((cur.0, cur.1, size.width as i32, size.height as i32));
        }
        _ => {}
    });

    // 落盘线程：每 500ms 检查 latest 是否有变化，有就写一次
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let pending = {
                let mut g = latest.lock().unwrap();
                g.take()
            };
            if let Some((x, y, w, h)) = pending {
                let state = app.state::<AppState>();
                let mut cfg = state.config.write().await;
                let mut dirty = false;
                if cfg.floating_x != Some(x) { cfg.floating_x = Some(x); dirty = true; }
                if cfg.floating_y != Some(y) { cfg.floating_y = Some(y); dirty = true; }
                if w > 0 && cfg.floating_w != Some(w) { cfg.floating_w = Some(w); dirty = true; }
                if h > 0 && cfg.floating_h != Some(h) { cfg.floating_h = Some(h); dirty = true; }
                if dirty {
                    if let Err(e) = cfg.save() {
                        tracing::warn!(error = %e, "保存浮窗几何失败 (debounced)");
                    }
                }
            }
        }
    });
}

/// `musage dump [provider]` 子命令：拉一次用量并打印
///
/// `provider`：可选，`minimax` / `deepseek` / `xiaomimimo` / `tavily`，不传则跑全部启用的。
fn run_dump_subcommand(provider_filter: Option<&str>) -> i32 {
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    rt.block_on(async {
        let cfg = AppConfig::load_from_disk().unwrap_or_default();

        // 决定要 dump 哪些 source
        let sources: Vec<Box<dyn crate::providers::QuotaSource>> = match provider_filter {
            None => builtin_sources()
                .into_iter()
                .filter(|s| cfg.is_enabled_id(s.id().as_ref()))
                .collect(),
            Some(id) => match builtin_sources().into_iter().find(|s| s.id() == id) {
                Some(s) => vec![s],
                None => {
                    let known: Vec<String> = builtin_sources().iter().map(|s| s.id().to_string()).collect();
                    eprintln!("{}", t!("cli.dump_unknown_source", id = id, known = known.join(" / ")));
                    return 2;
                }
            },
        };

        if sources.is_empty() {
            eprintln!("{}", t!("cli.dump_no_enabled"));
            return 2;
        }

        for src in sources {
            println!("{}", t!("cli.dump_header", display_name = src.display_name(), id = src.id()));

            // 加载凭据
            let creds = match config::load_credential_for_id(src.id().as_ref()) {
                Ok(Some(c)) => c,
                Ok(None) => {
                    eprintln!("{}", t!("cli.dump_no_credentials"));
                    continue;
                }
                Err(e) => {
                    eprintln!("{}", t!("cli.dump_read_keys_failed", err = e.to_string()));
                    continue;
                }
            };

            // 给 source 推 state（region / overrides）—— 命令行调试场景用 cfg 默认值
            update_source_state_for_dump(&src, &cfg).await;

            // 走 registry 路径（Phase 1 起的新路径）
            let result = src.fetch(&creds).await;

            match result {
                Ok(snap) => {
                    println!("{}", t!("cli.dump_raw_response"));
                    if let Some(raw) = &snap.raw {
                        println!("{}", serde_json::to_string_pretty(raw).unwrap_or_default());
                    }
                    println!("{}", t!("cli.dump_parsed_result"));
                    println!("{}", serde_json::to_string_pretty(&snap).unwrap_or_default());
                }
                Err(e) => {
                    eprintln!("{}", t!("cli.dump_fetch_failed", err = format!("{e:?}")));
                }
            }
        }

        0
    })
}

/// 给 dump CLI 推 source state（region / overrides）—— 走 commands 模块的共享逻辑。
async fn update_source_state_for_dump(src: &Box<dyn crate::providers::QuotaSource>, cfg: &AppConfig) {
    crate::commands::update_source_state(src, cfg).await;
}
