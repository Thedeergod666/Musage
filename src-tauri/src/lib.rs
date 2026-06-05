//! Musage - MiniMax Token Plan 实时用量监控
//!
//! 架构：
//! - `api`    : 拉取用量数据，灵活 schema 解析
//! - `config` : 配置 + keyring 存取
//! - `poller` : tokio 后台定时拉取
//! - `tray`   : 系统托盘 + 动态图标
//! - `commands` : tauri::command 暴露给前端
//!
//! CLI 子命令：
//! - (无)   : 启动 GUI（Tauri 默认行为）
//! - `dump` : 拉一次并打印原始 JSON + 解析结果（用于探查 schema）

mod api;
mod commands;
mod config;
mod poller;
mod tray;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

use crate::api::QuotaSnapshot;
use crate::config::AppConfig;

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
        std::process::exit(run_dump_subcommand());
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

            // 初始化托盘
            tray::setup(app.handle())?;

            // 默认显示悬浮窗
            if let Some(win) = app.get_webview_window("floating") {
                let _ = win.show();
                let _ = win.set_focus();
            }

            // 首次启动引导：未配置 API key → 自动弹设置窗口
            let has_key = config::load_api_key_from_keyring()
                .ok()
                .flatten()
                .is_some();
            if !has_key {
                let _ = tauri::WebviewWindowBuilder::new(
                    app.handle(),
                    "settings",
                    tauri::WebviewUrl::App("settings.html".into()),
                )
                .title("Musage · 设置")
                .inner_size(480.0, 520.0)
                .min_inner_size(400.0, 400.0)
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
            commands::has_api_key,
            commands::set_api_key,
            commands::delete_api_key,
            commands::open_settings_window,
            commands::hide_floating_window,
            commands::show_floating_window,
            commands::hide_settings_window,
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

/// `musage dump` 子命令：拉一次用量并打印
fn run_dump_subcommand() -> i32 {
    // 简易 runtime（不进 tauri）
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    rt.block_on(async {
        // 从 keyring 读 api key
        let api_key = match config::load_api_key_from_keyring() {
            Ok(Some(k)) => k,
            Ok(None) => {
                eprintln!("[dump] keyring 里没找到 api key。请先运行 GUI 配置一次。");
                return 2;
            }
            Err(e) => {
                eprintln!("[dump] keyring 读 key 失败: {e}");
                return 2;
            }
        };

        let cfg = AppConfig::load_from_disk().unwrap_or_default();
        println!("[dump] region: {:?}", cfg.region);
        println!("[dump] api key 前缀: {}…", &api_key[..api_key.len().min(8)]);

        match api::fetch_quota(&api_key, cfg.region).await {
            Ok((raw, snap)) => {
                println!("\n========== 原始响应 ==========");
                println!("{}", serde_json::to_string_pretty(&raw).unwrap_or_default());
                println!("\n========== 解析结果 ==========");
                println!("{}", serde_json::to_string_pretty(&snap).unwrap_or_default());
                0
            }
            Err(e) => {
                eprintln!("[dump] 拉取失败: {e}");
                1
            }
        }
    })
}
