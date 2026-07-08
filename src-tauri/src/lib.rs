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

use crate::commands::apply_pin_mode_to_window;
use crate::config::{extra_instances, AppConfig};
use crate::logstore::LogStore;
use crate::poller_backoff::BackoffState;
use crate::providers::{builtin_sources, CustomSource, QuotaSnapshot};

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
    /// PR 1b：用户额外添加的 source 实例（内置 provider 副本 + New API 中转站）。
    /// 启动时从 `extra_instances.json` load（PR 1a 起的迁移 wrapper 会从老
    /// `custom_sources.json` 迁过来，PR 1c 删老文件）。
    /// 写：add/update/delete_extra_instance IPC 命令（会 persist）。
    /// 读：providers::all_sources / find_source 拼 extras 进去。
    pub extra_instances: Arc<RwLock<Vec<extra_instances::ExtraInstance>>>,
}

pub use crate::commands::i18n::get_app_locale;
/// 前端调用的"切换语言"命令。实现见 [`crate::commands::i18n::set_app_locale`]。
/// 这里只 re-export 给 `tauri::generate_handler!` 用。
pub use crate::commands::i18n::set_app_locale;

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
        .plugin(tauri_plugin_process::init())
        // v0.2.0 follow-up commit 7 (P2-B-8): 注册 notification plugin 让 log_provider_error
        // 在 Xiaomi/Claude cookie 失效时弹系统通知。
        .plugin(tauri_plugin_notification::init())
        // M6 fix: .manage() 移到 .setup() 之前。之前 .setup() 里
        // app.state::<AppState>() 在 .manage() 之前调用，Tauri 2 部分版本
        // 会 panic（state 未注册）。更安全的做法:先 manage 注册 state，
        // 再 setup 里读写。
        .manage(AppState {
            snapshot: Arc::new(RwLock::new(QuotaSnapshot::default())),
            config: Arc::new(RwLock::new(AppConfig::default())),
            // 从磁盘 reload 最近 200 条 —— 启动时一次性 IO，不在热路径
            log: Arc::new(LogStore::load_from_disk()),
            backoff: Arc::new(RwLock::new(BackoffState::new())),
            // PR 3 → PR 1a：extra_instances 启动 load。优先读新文件 extra_instances.json，
            // 老 custom_sources.json 自动迁移后 rename 成 .migrated。
            // load 失败时返空 Vec（不阻塞启动）。
            // v0.2.1 commit 2: load_or_migrate 从 config/custom_sources.rs 内联到
            // extra_instances.rs 同模块，wrapper 文件已删。
            extra_instances: Arc::new(RwLock::new(
                extra_instances::load_or_migrate().unwrap_or_default(),
            )),
        })
        .setup(|app| {
            // 启动时清理上次崩溃留下的孤儿 .tmp 文件（F2/M10 修复连带）
            config::cleanup_orphan_tmp_files();
            // 读取配置（load_from_disk 已会在损坏时备份到 .bak.<ts>，不再静默吞掉）
            let config = AppConfig::load(&app.handle()).unwrap_or_default();

            // P0：把 cfg.locale 推到 rust_i18n 的进程内 state，让 tr!() 立刻生效。
            // 必须在 .setup 里最早做（之后 tr!() 才会拿到正确 locale）。
            rust_i18n::set_locale(&config.locale);

            // v0.2.1 fix（Windows NSIS i18n regression guard）：
            // rust-i18n 3.x 把 locale 数据存进一个 `Lazy<Box<dyn Backend>>`，
            // initializer 里塞的是 HashMap 字面量。Windows MSVC + lto + strip
            // 组合会把 HashMap 的 backing 数据段当 unreferenced 丢掉，
            // 导致 release binary 的 backend 是空的 —— t!() 全部回退成
            // "locale.key" 字面量。upstream longbridge/rust-i18n #115 在
            // Windows 上踩过同样的坑。
            //
            // 这里 probe 一下："known good" key 必须能翻成非 key 字符串，
            // 不然直接 panic，让失败模式从"用户看到 provider_name.xiaomimimo"
            // 变成"启动时立刻崩 + 明确错误信息"。
            {
                let probe_en =
                    rust_i18n::t!("provider_name.xiaomimimo", locale = "en").into_owned();
                if probe_en == "provider_name.xiaomimimo" {
                    panic!(
                        "i18n backend returned raw key for `provider_name.xiaomimimo`. \
                         This is the known Windows + LTO + strip interaction with \
                         rust-i18n 3.x HashMap storage — see [profile.release] in \
                         src-tauri/Cargo.toml. Available locales: {:?}",
                        rust_i18n::available_locales!()
                    );
                }
                tracing::info!(
                    locales = ?rust_i18n::available_locales!(),
                    probe_en = %probe_en,
                    "rust_i18n backend ready"
                );
            }

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
                    if let Some(w) = app_for_locale.get_webview_window("floating") {
                        let title = t!("window.floating").to_string();
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

                // "有效位置"判定（v0.2.1 commit 9 升级：跨屏感知）：
                // - saved x/y 都存在
                // - 该位置在当前任何一块 monitor 的物理像素矩形内
                // 老 v0.2.0 实现用 `(x > 50 || y > 50)` 简单判断"非 OS 默认"，
                // 在多屏场景下不靠谱 —— 用户拖到 (300, -200) 也 > 50，但
                // 副屏拔了之后 (300, -200) 不在任何 monitor 内,启动后窗口
                // 飞出去看不见。改成走 `available_monitors()` 几何检查。
                let monitors: Vec<tauri::Monitor> = win.available_monitors().unwrap_or_default();
                let saved_pos_valid = if monitors.is_empty() {
                    // 兜底:拿不到 monitor 列表时,沿用老的 >50 启发式判断。
                    matches!(
                        (cfg.floating_x, cfg.floating_y),
                        (Some(x), Some(y)) if x > 50 || y > 50
                    )
                } else {
                    matches!(
                        (cfg.floating_x, cfg.floating_y),
                        (Some(x), Some(y)) if position_is_visible(x, y, &monitors)
                    )
                };

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
                let _ = win.set_title(&t!("window.floating").to_string());
                let _ = win.show();
                let _ = win.set_focus();
            }

            // 首次启动引导：所有 provider 都没配 key → 自动弹设置窗口
            // H1: 检查范围扩展到 custom sources —— 用户可能只配了 New API 中转站,
            // 没动任何 builtin,这种场景不应该再弹"请配 key"的引导。
            let builtin_has_key = builtin_sources().iter().any(|src| {
                config::load_credential_for_id(src.id().as_ref())
                    .ok()
                    .flatten()
                    .is_some()
            });
            let custom_has_key = extra_instances::load_or_migrate()
                .unwrap_or_default()
                .iter()
                .any(|inst| {
                    config::load_credential_for_id(&inst.api_key_ref)
                        .ok()
                        .flatten()
                        .is_some()
                });
            if !builtin_has_key && !custom_has_key {
                // 首启引导：走和 open_settings_window 同一个 builder，避免
                // 两处配置漂移（窗口大小 / decorations / background_color 等
                // 必须一致，否则两个入口的设置窗看上去会不一样）。
                let _ = commands::build_settings_window(app.handle());
            }

            Ok(())
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
            commands::set_schema_overrides,
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
            // PR 1b：用户额外 source 实例 (6 commands，替换 PR 3 的 5 个 custom_sources)
            commands::extra_instances::list_extra_instances,
            commands::extra_instances::add_extra_instance,
            commands::extra_instances::update_extra_instance,
            commands::extra_instances::delete_extra_instance,
            commands::extra_instances::list_picker_providers,
            commands::extra_instances::test_extra_instance,
            // C3 fix: source-extras 6 个 per-field setter (region / mode / concise / base_url)
            commands::set_minimax_region,
            commands::set_xiaomi_region_field,
            commands::set_tavily_concise_mode,
            commands::set_zenmux_base_url,
            commands::set_zenmux_mode,
            commands::set_zenmux_payg_concise,
            commands::set_zhipu_region,
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
        // L1 fix (2026-07-06 全量审查): 把入口 panic 改成可读 stderr + 非零退出。
        // 之前 .expect() 在 Win 缺 WebView2 / tauri dev 端口被占时静默崩,用户
        // 只看到托盘图标消失。给运维一个明确的失败原因(从 stderr / 日志抓取)。
        .unwrap_or_else(|e| {
            eprintln!("[musage] tauri::Builder::run 失败: {e}");
            tracing::error!(error = %e, "tauri::Builder::run 失败,进程退出");
            std::process::exit(1);
        });
}

/// 启动一个后台任务，把浮窗的"位置/大小"事件 debounce 到 500ms 再写 config.json。
///
/// 修复 H1：原来每个像素的 `WindowEvent::Moved` 都 spawn 一个 tokio 任务并立即
/// `save()` —— 拖 300px 会 spawn 300 个任务 + 写 300 次 config.json。
/// 现在的策略：回调里只更新共享状态，background task 定时（500ms）检查并落盘。
fn spawn_debounced_geom_persister(app: tauri::AppHandle, win: tauri::WebviewWindow) {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let latest: Arc<Mutex<Option<(i32, i32, i32, i32)>>> = Arc::new(Mutex::new(None));
    let latest_for_cb = latest.clone();

    // 回调线程：只更新 latest，不做 I/O
    // **2026-06-20 audit**：之前 `.lock().unwrap()` 在 persister task panic 后
    // 会 poison mutex → 后续 .unwrap() 直接 panic（窗口事件回调拿不到锁），
    // 整个 geometry 持久化静默坏掉。改用项目统一的 `.unwrap_or_else(|e| ...)`
    // poison 恢复模式。
    //
    // **2026-07-08 fix**：Moved 分支显式拒绝 `(0, 0)` —— 治根因层
    // (macOS NSWindow 未指定 x/y 时默认 (0,0),启动 set_position 矫正前
    // WKWebView 可能 emit 合成 Moved(0,0),会被 persister 落进 config.json)。
    // Layer 1 在 `position_is_visible` 兜底读到的脏数据,这里 Layer 2 在源头
    // 阻断脏数据进入 `latest`。两者独立,缺一不可:`position_is_visible` 单修
    // 解决重启位置,但 disk 上还会留 `(0,0)` 等下次清 config 时才消失;
    // 这里单修不解决老用户已经落 `(0,0)` 的 config.json。
    win.on_window_event(move |event| match event {
        tauri::WindowEvent::Moved(pos) => {
            if pos.x == 0 && pos.y == 0 {
                // OS 默认值,不是用户拖动 → 丢弃
                return;
            }
            let mut g = latest_for_cb.lock().unwrap_or_else(|e| {
                tracing::warn!("geom_persister latest_for_cb poisoned (Moved), recovering");
                e.into_inner()
            });
            let cur = g.unwrap_or((pos.x, pos.y, 0, 0));
            *g = Some((pos.x, pos.y, cur.2, cur.3));
        }
        tauri::WindowEvent::Resized(size) => {
            let mut g = latest_for_cb.lock().unwrap_or_else(|e| {
                tracing::warn!("geom_persister latest_for_cb poisoned (Resized), recovering");
                e.into_inner()
            });
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
                let mut g = latest.lock().unwrap_or_else(|e| {
                    tracing::warn!("geom_persister latest poisoned (tick), recovering");
                    e.into_inner()
                });
                g.take()
            };
            if let Some((x, y, w, h)) = pending {
                let state = app.state::<AppState>();
                let mut cfg = state.config.write().await;
                let mut dirty = false;
                if cfg.floating_x != Some(x) {
                    cfg.floating_x = Some(x);
                    dirty = true;
                }
                if cfg.floating_y != Some(y) {
                    cfg.floating_y = Some(y);
                    dirty = true;
                }
                if w > 0 && cfg.floating_w != Some(w) {
                    cfg.floating_w = Some(w);
                    dirty = true;
                }
                if h > 0 && cfg.floating_h != Some(h) {
                    cfg.floating_h = Some(h);
                    dirty = true;
                }
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
/// v0.2.1 commit 9 (P1-1): 浮窗跨屏感知。
///
/// 多屏用户拖到副屏,重启后位置可能"不在任何 monitor 内"(比如副屏拔了、
/// 主屏 DPI 变了、显卡驱动重置了 monitor layout)。返回 false 让 caller
/// 走 top-right fallback 而不是把窗口放到看不见的地方。
///
/// monitors: `WebviewWindow::available_monitors()` 返回的列表。
/// x/y: 物理像素 (跟 `PhysicalPosition` 一致)。
///
/// **2026-07-08 fix**:`(0, 0)` 显式拒绝 —— macOS NSWindow 未指定 x/y 时
/// 默认 (0, 0),启动 set_position(top_right) 矫正前 WKWebView 可能 emit 过
/// `WindowEvent::Moved(0, 0)` 给 persister → config.json 落 `(0, 0)`。
/// 第二次启动读 disk,`position_is_visible` 因为主屏 `pos=(0,0)` 会误判合法
/// → 浮窗回到左上角。`(0, 0)` 不可能是用户主动拖到的位置(贴边 0px 没意义),
/// 一律按"OS 默认值"踢回 fallback。多屏下副屏在主屏左侧时 `pos.x < 0`,
/// 用户拖过去再回来不会留 (0, 0),所以这条规则跨屏安全。
fn position_is_visible(x: i32, y: i32, monitors: &[tauri::Monitor]) -> bool {
    if x == 0 && y == 0 {
        return false;
    }
    monitors.iter().any(|m| {
        let pos = m.position();
        let size = m.size();
        x >= pos.x && x < pos.x + size.width as i32 && y >= pos.y && y < pos.y + size.height as i32
    })
}

/// `provider`：可选，`minimax` / `deepseek` / `xiaomimimo` / `tavily` / `custom_<uuid>`，
/// 不传则跑全部启用的(builtin + custom 都跑)。
fn run_dump_subcommand(provider_filter: Option<&str>) -> i32 {
    let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
    rt.block_on(async {
        let cfg = AppConfig::load_from_disk().unwrap_or_default();
        // 加载 extra instances 供 dump CLI 用——standalone CLI 没有 AppState,
        // 直接从 extra_instances.json 读即可(跟 lib.rs setup() 那条路径同款)。
        let customs: Vec<extra_instances::ExtraInstance> =
            extra_instances::load_or_migrate().unwrap_or_default();

        // 决定要 dump 哪些 source
        let sources: Vec<Box<dyn crate::providers::QuotaSource>> = match provider_filter {
            None => {
                let mut all: Vec<Box<dyn crate::providers::QuotaSource>> = builtin_sources()
                    .into_iter()
                    .filter(|s| cfg.is_enabled_id(s.id().as_ref()))
                    .collect();
                // custom source 单独 append —— builtin 列表里没有
                // PR 1a：extra instance 里只 append provider_id == "custom" 的
                // (内置副本由 builtin_sources() 已含的 11 份 ... 不对, 副本不走 builtin)
                // PR 1a 简化：dump CLI 暂时不展示内置副本(builtin_sources() 只返 11 份
                // 内置第 1 份),全量 dump 时只 append custom 中转站。后续 PR 1b
                // providers::all_sources 改完后再调它。
                for inst in &customs {
                    if inst.provider_id == "custom" {
                        if let Some(spec) = &inst.custom {
                            if cfg.is_enabled_id(&inst.api_key_ref) {
                                all.push(Box::new(CustomSource::new(spec.clone())));
                            }
                        }
                    }
                }
                all
            }
            Some(id) => {
                // builtin 优先,然后 custom
                if let Some(s) = builtin_sources().into_iter().find(|s| s.id() == id) {
                    vec![s]
                } else if let Some(inst) = customs.iter().find(|s| s.api_key_ref == id) {
                    if let Some(spec) = &inst.custom {
                        vec![Box::new(CustomSource::new(spec.clone()))]
                    } else {
                        // 找到了 instance 但不是 custom(理论不会到这里)
                        eprintln!("instance {} found but has no custom spec", inst.api_key_ref);
                        return 2;
                    }
                } else {
                    // 拼"已知 id"列表(builtin + custom),错误消息更友好
                    let mut known: Vec<String> = builtin_sources()
                        .iter()
                        .map(|s| s.id().to_string())
                        .collect();
                    known.extend(customs.iter().map(|s| s.api_key_ref.clone()));
                    eprintln!(
                        "{}",
                        t!(
                            "cli.dump_unknown_source",
                            id = id,
                            known = known.join(" / ")
                        )
                    );
                    return 2;
                }
            }
        };

        if sources.is_empty() {
            eprintln!("{}", t!("cli.dump_no_enabled"));
            return 2;
        }

        for src in sources {
            println!(
                "{}",
                t!(
                    "cli.dump_header",
                    display_name = src.display_name(),
                    id = src.id()
                )
            );

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
            // M5 fix: 包 30s timeout —— shared reqwest client 默认 10s timeout，但
            // 某些 provider (deepseek / stepfun) 偶发挂 30s+ 不返；dump CLI 不应挂死。
            let result =
                match tokio::time::timeout(std::time::Duration::from_secs(30), src.fetch(&creds))
                    .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        eprintln!("[musage dump] {} fetch 超时（30s）", src.id());
                        continue;
                    }
                };

            match result {
                Ok(snap) => {
                    println!("{}", t!("cli.dump_raw_response"));
                    if let Some(raw) = &snap.raw {
                        println!("{}", serde_json::to_string_pretty(raw).unwrap_or_default());
                    }
                    println!("{}", t!("cli.dump_parsed_result"));
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&snap).unwrap_or_default()
                    );
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
async fn update_source_state_for_dump(
    src: &Box<dyn crate::providers::QuotaSource>,
    cfg: &AppConfig,
) {
    crate::commands::update_source_state(src, cfg).await;
}

// ── i18n regression guard ────────────────────────────────────────
//
// 在 `cargo test --locked` 阶段捕获 Windows NSIS LTO+strip 与 rust-i18n 3.x
// HashMap 存储的交互 bug，避免再次出 release 后才发现 locale 全部回退到
// "locale.key" 字面量。runtime 端的 probe 在 setup() 里（启动即崩，给出
// 明确错误信息），test 端在这里（CI 守门，任何平台跑 test 都能发现）。
//
// 跟 main 流程的 t!() 调用同一个 macro path —— 如果哪天有人把
// `rust_i18n::i18n!("locales")` 那行改坏 / locales/*.json 写错格式 /
// release profile 又启用了某种 strip 变体，这些 test 会立刻 panic。
#[cfg(test)]
mod i18n_guard_tests {
    use super::t;

    #[test]
    fn available_locales_has_en_and_zh_cn() {
        let locales: Vec<&'static str> = rust_i18n::available_locales!();
        assert!(
            locales.contains(&"en"),
            "rust-i18n 没有 en locale —— locales/en.json 可能没被 macro 编译进 binary。locales: {locales:?}"
        );
        assert!(
            locales.contains(&"zh-CN"),
            "rust-i18n 没有 zh-CN locale —— locales/zh-CN.json 可能没被 macro 编译进 binary。locales: {locales:?}"
        );
    }

    #[test]
    fn en_provider_names_resolve_to_non_key_strings() {
        rust_i18n::set_locale("en");
        for key in [
            "provider_name.xiaomimimo",
            "provider_name.deepseek",
            "row.five_hour",
        ] {
            let v = t!(key).into_owned();
            assert_ne!(
                v, key,
                "rust-i18n returned raw key for `{key}` in en — locale HashMap not loaded \
                 (likely Cargo.toml [profile.release] strip + lto interaction, \
                 see lib.rs setup probe)"
            );
        }
    }

    #[test]
    fn zh_cn_provider_names_resolve_to_non_key_strings() {
        rust_i18n::set_locale("zh-CN");
        for key in [
            "provider_name.xiaomimimo",
            "provider_name.deepseek",
            "row.five_hour",
        ] {
            let v = t!(key).into_owned();
            assert_ne!(
                v, key,
                "rust-i18n returned raw key for `{key}` in zh-CN — locale HashMap not loaded"
            );
        }
    }
}
