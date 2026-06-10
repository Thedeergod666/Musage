//! Musage - 多 Provider 实时用量监控
//!
//! 架构：
//! - `providers` : 多 provider 抽象（trait + minimax/deepseek 实现）
//! - `config`    : 多 provider 配置 + keyring 存取
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
mod poller;
mod providers;
mod tray;

use std::sync::Arc;
use tauri::Manager;
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::providers::QuotaSnapshot;

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

            // 初始化托盘
            tray::setup(app.handle())?;

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
            Some(other) => {
                eprintln!("[dump] 未知 provider: {other}（可用: minimax / deepseek）");
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
                    eprintln!("[dump] keyring 里没找到 {} 的 key。请先在 GUI 设置面板配置。", provider.display_name());
                    continue;
                }
                Err(e) => {
                    eprintln!("[dump] keyring 读 key 失败: {e}");
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
