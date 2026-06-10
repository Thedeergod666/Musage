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
mod platform;
mod poller;
mod providers;
mod tray;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::providers::QuotaSnapshot;
use crate::commands::apply_pin_mode_to_window;

pub struct AppState {
    pub snapshot: Arc<RwLock<QuotaSnapshot>>,
    pub config: Arc<RwLock<AppConfig>>,
}

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
        .setup(|app| {
            // 读取配置
            let config = AppConfig::load(&app.handle()).unwrap_or_default();
            let cfg_handle = app.state::<AppState>();

            // 写入初始状态
            {
                let mut guard = cfg_handle.config.blocking_write();
                *guard = config;
            }

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
                if let (Some(x), Some(y)) = (cfg.floating_x, cfg.floating_y) {
                    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                }
                if let (Some(w), Some(h)) = (cfg.floating_w, cfg.floating_h) {
                    // 尊重 tauri.conf.json 里的 minWidth/minHeight（保持同步）
                    let scale = win.scale_factor().unwrap_or(1.0);
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

                // 监听移动 / 缩放 → 持久化（spawn 异步任务避免阻塞 UI 线程）
                let app_for_event = app.handle().clone();
                win.on_window_event(move |event| match event {
                    tauri::WindowEvent::Moved(pos) => {
                        persist_floating_geom(&app_for_event, Some(pos.x), Some(pos.y), None, None);
                    }
                    tauri::WindowEvent::Resized(size) => {
                        persist_floating_geom(&app_for_event, None, None, Some(size.width as i32), Some(size.height as i32));
                    }
                    _ => {}
                });
            }

            // 默认显示悬浮窗
            if let Some(win) = app.get_webview_window("floating") {
                let _ = win.show();
                let _ = win.set_focus();
            }

            // 首次启动引导：所有 provider 都没配 key → 自动弹设置窗口
            let any_key = providers::Provider::all().iter().any(|p| {
                config::load_api_key_for(*p)
                    .ok()
                    .flatten()
                    .is_some()
            });
            if !any_key {
                let _ = tauri::WebviewWindowBuilder::new(
                    app.handle(),
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
                .build();
            }

            Ok(())
        })
        .manage(AppState {
            snapshot: Arc::new(RwLock::new(QuotaSnapshot::default())),
            config: Arc::new(RwLock::new(AppConfig::default())),
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh_now,
            commands::get_config,
            commands::save_config,
            commands::has_api_key_for,
            commands::set_api_key_for,
            commands::delete_api_key_for,
            commands::get_api_key_for,
            commands::open_settings_window,
            commands::hide_floating_window,
            commands::show_floating_window,
            commands::hide_settings_window,
            commands::reset_floating_window,
            commands::set_floating_pin_mode,
            commands::set_floating_hover_raise,
            commands::set_low_power_mode,
            commands::set_auto_hide_in_fullscreen,
            commands::quit_app,
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

/// 把浮窗位置/大小变化持久化到 config.json。None 表示该字段保持原值。
///
/// 在 tokio 任务里跑，避免阻塞 UI 线程（on_window_event 在主线程触发）。
fn persist_floating_geom(
    app: &tauri::AppHandle,
    x: Option<i32>,
    y: Option<i32>,
    w: Option<i32>,
    h: Option<i32>,
) {
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app2.state::<AppState>();
        let mut cfg = state.config.write().await;
        let mut dirty = false;
        if let Some(x) = x {
            if cfg.floating_x != Some(x) { cfg.floating_x = Some(x); dirty = true; }
        }
        if let Some(y) = y {
            if cfg.floating_y != Some(y) { cfg.floating_y = Some(y); dirty = true; }
        }
        if let Some(w) = w {
            if cfg.floating_w != Some(w) { cfg.floating_w = Some(w); dirty = true; }
        }
        if let Some(h) = h {
            if cfg.floating_h != Some(h) { cfg.floating_h = Some(h); dirty = true; }
        }
        if dirty {
            if let Err(e) = cfg.save() {
                tracing::warn!(error = %e, "保存浮窗几何失败");
            }
        }
    });
}

/// `musage dump [provider]` 子命令：拉一次用量并打印
///
/// `provider`：可选，`minimax` / `deepseek`，不传则跑全部。
fn run_dump_subcommand(provider_filter: Option<&str>) -> i32 {
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    rt.block_on(async {
        let cfg = AppConfig::load_from_disk().unwrap_or_default();

        // 决定要 dump 哪些 provider
        let targets: Vec<providers::Provider> = match provider_filter {
            None => cfg.enabled_providers(),
            Some("minimax") => vec![providers::Provider::Minimax],
            Some("deepseek") => vec![providers::Provider::Deepseek],
            Some("xiaomi") | Some("xiaomimimo") => vec![providers::Provider::Xiaomimimo],
            Some(other) => {
                eprintln!("[dump] 未知 provider: {other}（可用: minimax / deepseek / xiaomi）");
                return 2;
            }
        };

        if targets.is_empty() {
            eprintln!("[dump] 没有启用的 provider");
            return 2;
        }

        for provider in targets {
            println!("\n========== {} ==========", provider.display_name());

            let key = match config::load_api_key_for(provider) {
                Ok(Some(k)) => k,
                Ok(None) => {
                    eprintln!("[dump] keys.json 里没找到 {} 的 key。请先在 GUI 设置面板配置。", provider.display_name());
                    continue;
                }
                Err(e) => {
                    eprintln!("[dump] 读 keys.json 失败: {e}");
                    continue;
                }
            };

            let prefix = if key.len() >= 8 { &key[..8] } else { &key };
            println!("[dump] api key 前缀: {prefix}…");

            // 直接调对应的 provider 实现（不经过 poller）
            let result: Result<providers::ProviderSnapshot, String> = match provider {
                providers::Provider::Minimax => {
                    let region = cfg.region();
                    let ov = cfg.schema_overrides
                        .get(provider.id_str())
                        .cloned()
                        .unwrap_or_default();
                    let r = providers::minimax::Minimax::do_fetch(&key, region, &ov).await;
                    r.map(|(_, snap)| snap)
                }
                providers::Provider::Deepseek => {
                    let p = providers::deepseek::Deepseek;
                    <providers::deepseek::Deepseek as providers::ProviderImpl>::fetch(&p, &key)
                        .await
                }
                providers::Provider::Xiaomimimo => {
                    let region = cfg.xiaomi_region();
                    let ov = cfg.schema_overrides
                        .get(provider.id_str())
                        .cloned()
                        .unwrap_or_default();
                    let r = providers::xiaomi::Xiaomimimo::do_fetch(&key, region, &ov).await;
                    r.map(|(_, snap)| snap)
                }
            };

            match result {
                Ok(snap) => {
                    println!("\n--- 原始响应 ---");
                    if let Some(raw) = &snap.raw {
                        println!("{}", serde_json::to_string_pretty(raw).unwrap_or_default());
                    }
                    println!("\n--- 解析结果 ---");
                    println!("{}", serde_json::to_string_pretty(&snap).unwrap_or_default());
                }
                Err(e) => {
                    eprintln!("[dump] 拉取失败: {e}");
                }
            }
        }

        0
    })
}
