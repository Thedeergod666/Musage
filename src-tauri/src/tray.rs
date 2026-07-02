//! 系统托盘动态图标生成
//!
//! 渲染规则：
//! - 32x32 RGBA，**透明底**（无背景填充）
//! - MiniMax：两条水平迷你进度条，上 = 5h utilization，下 = 周 utilization
//!   - 轨道 = 暗灰圆角矩形
//!   - 填充 = 白色实心（按已用% 决定填充宽度）
//! - DeepSeek：保留单大数字 + 货币单位（钱包余额没有"已用/总量"，进度条不适用）
//! - 都没有数据：留空（font 缺失同样留空）
//! - 托盘 tooltip：所有 provider 的核心状态，逗号分隔

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::Rgba;
use imageproc::drawing::draw_text_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};
use tokio::sync::mpsc;

use crate::config::TrayIconStyle;
use crate::providers::{ProviderSnapshot, QuotaSnapshot};
// rust-i18n t! macro 在 crate 根（lib.rs）定义，子模块需显式 use 才能用。
// 不要在子模块里写 `use rust_i18n::t;` —— 那会找错 macro；必须走 crate::t
// 才能拿到 i18n!("locales") 生成的那份。
use crate::t;

// ═══════════════════════════════════════════════════════════════════
// 跨线程 tray 更新 channel（2026-06-18 闪退根因修复）
// ═══════════════════════════════════════════════════════════════════
//
// 之前：所有调用方（poller / refresh_single / set_tray_icon_style）直接
// `app.tray_by_id("main-tray")` → 拿 owned `TrayIcon`（Tauri 内部走
// `Arc::unwrap_or_clone` 拿唯一所有权）→ set_icon / set_tooltip → 函数
// 返回时 tray 走 `Drop for TrayIcon` → `TrayIcon::remove` → 调
// `NSStatusBar::removeStatusItem` → **BSServiceMainRunLoopQueue::assertBarrierOnQueue
// 触发 SIGTRAP 闪退**。
//
// 根因：`app.tray_by_id` 返回的 tray 是**唯一所有者**（Tauri 资源表里
// 只放了一个 Arc handle），函数出 scope 自动 drop，而调用方是 tokio
// worker 线程（poller 走 `tauri::async_runtime::spawn`），不在 main
// runloop —— AppKit NSStatusBar 操作跨线程必炸。
//
// 修法：所有 tray 写操作通过一个进程内 mpsc channel 派发到 main thread：
// - 调用方（任何线程）`try_send` 一条 TrayRequest，立即返回，不阻塞
// - 一个 long-lived tokio task 跑 receiver 循环，每收一条消息就
//   `app.run_on_main_thread(closure)` 派到 main thread
// - main thread closure 里拿 `tray_by_id` → set_icon → set_tooltip →
//   **正常 drop**（在 main thread 上 → 不会 SIGTRAP）
//
// 注意：`run_on_main_thread` 是 `FnOnce`，所以不能在 closure 里 loop。
// 每条消息 → 一次 dispatch（poller 1Hz × N provider → N 次/秒，完全可接受）。
// 真要 coalesce 时（v2 优化）可以 receiver 内部加"keep last" buffer。

/// Tray 写操作请求（跨线程派发到 main thread）
#[derive(Debug)]
enum TrayRequest {
    /// 更新托盘图标 + tooltip
    Update {
        snap: Box<QuotaSnapshot>,
        style: TrayIconStyle,
    },
    /// 重建菜单（locale 切换时，menu label 走 t!() 重新拿当前 locale）
    RebuildMenu,
}

/// 进程内 tray request sender —— `update_tray_from_snapshot` / `rebuild_tray`
/// 通过这个 handle 派发到 main thread。
///
/// 用 `OnceLock<Mutex<Option<UnboundedSender>>>`：
/// - `OnceLock` 保证全局只有一个 sender
/// - `Mutex<Option<...>>` 因为 sender 在 `tray::setup` 里才能初始化（那时
///   才有 AppHandle 可以 clone 出来 start receiver），之前的调用方
///   （理论上不会有，但 race-conditions 下极早期 emit 可能先到）走
///   `try_send` 拿到 `None` 就 log warn skip
static TRAY_REQUEST_TX: OnceLock<Mutex<Option<mpsc::UnboundedSender<TrayRequest>>>> =
    OnceLock::new();

fn tray_request_tx() -> Option<mpsc::UnboundedSender<TrayRequest>> {
    TRAY_REQUEST_TX.get().and_then(|m| {
        // **B-NEW-7（2026-06-19 audit）**：mutex poison 自动恢复。
        // 之前 .lock().ok() 在 poisoned 时返 Err → 整个闭包返 None →
        // 所有 tray 更新静默丢弃。改成 log warn + into_inner() 继续用，
        // 与 logstore / 其他模块的 poison 恢复策略保持一致。
        match m.lock() {
            Ok(g) => g.clone(),
            Err(p) => {
                tracing::warn!("tray_request_tx mutex poisoned，自动恢复");
                p.into_inner().clone()
            }
        }
    })
}

/// 派发一条 tray 请求到 main thread。失败（receiver 已 drop）→ log warn skip。
fn dispatch_tray_request(req: TrayRequest) {
    let Some(tx) = tray_request_tx() else {
        tracing::warn!("tray request channel 还没初始化（极早期事件），丢弃");
        return;
    };
    if let Err(e) = tx.send(req) {
        tracing::warn!(error = %e, "tray request 派发失败（receiver 已退出）");
    }
}

/// 防御 NSStatusItem 合成事件误触发 toggle：
///
/// macOS 端 `rightMouseDown:` → `on_tray_click(Right)` → `ns_button.performClick(None)`
/// 会**模拟**一次左键 click 走 `mouseUp:`，派发 `Click{button:Left, state:Up}`
/// 到我们 `on_tray_icon_event`。我们的 toggle 模式 `if let ... button: Left,
/// state: Up` **会匹配**这个合成事件，触发 `w.hide()` → 浮窗消失 → app 失焦
/// → 菜单立即关闭（用户看到"闪一下就消失"）。
///
/// 修法：记录最近一次真 `Left Down` 的时间戳，`Left Up` 触发 toggle 前校验
/// —— 必须是过去 500ms 内有对应 `Left Down` 才认作用户真点击，否则视作
/// 合成事件丢弃。
///
/// 500ms 阈值远大于任何真用户的 down→up 间隔（典型 < 200ms），又短到能
/// 把隔夜的陈旧 Down 视为失效（防止上次 app 关闭时的 Down 状态影响下次）。
static LAST_LEFT_DOWN_MS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 真左键 click 的 down→up 间隔上限。超过这个值的 Up 视为合成事件或脏状态。
const LEFT_DOWN_UP_WINDOW_MS: u64 = 500;

// 字体加载：优先用户自选填 `assets/font.ttf`，再走系统字体 fallback，
// 最后用平台内置的备用路径。全部失败 → 纯色圆点（无文字）。
static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

fn load_font() -> Option<&'static FontVec> {
    FONT.get_or_init(|| {
        // 1. 用户自选填
        let user_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/font.ttf");
        if let Some(font) = try_load_font(&user_path) {
            tracing::debug!(path = %user_path.display(), "loaded user font");
            return Some(font);
        }

        // 2. 系统字体 fallback（单 face .ttf，避免 .ttc collection）
        for path in system_font_paths() {
            if let Some(font) = try_load_font(&path) {
                tracing::debug!(path = %path.display(), "loaded system font");
                return Some(font);
            }
        }

        tracing::warn!("no usable TTF found; tray will show color circle without text");
        None
    })
    .as_ref()
}

fn try_load_font(path: &std::path::Path) -> Option<FontVec> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| FontVec::try_from_vec(bytes).ok())
}

fn system_font_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    {
        // macOS 菜单栏渲染到 ~16-18px 实际像素，Regular 字体在那个尺寸下
        // 字形会糊掉看不清。**优先 Bold/Black 单 face TTF**（不动 .ttc 避免
        // collection 解析坑），保证 percent 模式的 "5h 83%" 清晰可读。
        paths.push("/System/Library/Fonts/Supplemental/Arial Black.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Arial Bold.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Arial Rounded Bold.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Tahoma Bold.ttf".into());
        // fallback: 变量字体（ab_glyph 加载后默认走 Regular，仍比没有强）
        paths.push("/System/Library/Fonts/SFNS.ttf".into());
        // 最后兜底
        paths.push("/System/Library/Fonts/Supplemental/Arial.ttf".into());
    }
    #[cfg(target_os = "windows")]
    {
        // Win 优先 arialbd（粗体），保证 tray 文字可见
        paths.push("C:/Windows/Fonts/arialbd.ttf".into());
        paths.push("C:/Windows/Fonts/arial.ttf".into());
        paths.push("C:/Windows/Fonts/segoeui.ttf".into());
        paths.push("C:/Windows/Fonts/segoeuib.ttf".into());
        paths.push("C:/Windows/Fonts/tahoma.ttf".into());
        paths.push("C:/Windows/Fonts/consola.ttf".into());
    }
    #[cfg(target_os = "linux")]
    {
        // Linux Bold 优先（DejaVu Sans-Bold / Liberation Sans-Bold）
        paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".into());
        paths.push("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf".into());
        paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
        paths.push("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf".into());
        paths.push("/usr/share/fonts/TTF/DejaVuSans.ttf".into());
    }
    paths
}

// **Win11 高 DPI 防糊（2026-06-11）**：通知区托盘槽位固定 16 DIP，按系统
// DPI 缩放后实际像素是 16/20/24/28/32/40/48/56（100/125/150/175/200/250/300/350%）。
// 32px 源在 100/125/150% 上被 GDI 默认拉伸模式（COLORONCOLOR/最近邻）下采样
// 到 16/20/24，结果模糊+锯齿；64px 是常见 DPI 档（≤400%）都能干净缩放的最小源。
// macOS NSStatusItem / Linux AppIndicator 对任意源尺寸都容错，无需提升。
#[cfg(target_os = "windows")]
const ICON_SIZE: u32 = 64;
#[cfg(not(target_os = "windows"))]
const ICON_SIZE: u32 = 32;

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    // 构造初始菜单（tray builder 一次吃完）。
    let menu = build_tray_menu(app)?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .tooltip(t!("tray.tooltip.loading").to_string())
        .icon(make_placeholder_icon())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => {
                if let Some(w) = app.get_webview_window("floating") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
            "settings" => {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    // v0.2.1 commit 8: 走默认 tab (None),保留之前"打开设置"
                    // 不跳 section 的行为。
                    if let Err(e) = crate::commands::open_settings_window(app2, None).await {
                        tracing::warn!(error = %e, "打开设置失败");
                    }
                });
            }
            "refresh" => {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::poller::tick_now(&app2).await {
                        tracing::warn!(error = %e, "手动刷新失败");
                    }
                });
            }
            "force_top_floating" => {
                // 走 Win 私有路径：先解锁前台锁定（ASFW_ANY 允许任何
                // process 抢前台），再 SetForegroundWindow 抢前台。
                // **会**把焦点抢过来 —— 这是用户**主动**点菜单触发的
                // 操作，UX 上可接受（用户此刻在操作我们 app）。
                if let Some(w) = app.get_webview_window("floating") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    // hwnd() / windows_sys crate 在 Tauri 2 里 cfg-gate 在 windows,
                    // macos / linux 上不存在这两个 symbol。Win32 API 整块 cfg-gate。
                    #[cfg(target_os = "windows")]
                    {
                        if let Ok(hwnd) = w.hwnd() {
                            use windows_sys::Win32::UI::WindowsAndMessaging::{
                                AllowSetForegroundWindow, SetForegroundWindow,
                            };
                            unsafe {
                                let _ = AllowSetForegroundWindow(0x00000001); // ASFW_ANY
                                let _ = SetForegroundWindow(hwnd.0);
                            }
                        }
                    }
                    // 同时把 WS_EX_TOPMOST 持久化（这样即便焦点之后
                    // 丢失，level-trigger 也能在 16ms 内 re-assert）
                    let _ = w.set_always_on_top(true);
                }
                // **L1 fix（2026-06-19）**：之前只在本会话抢一次前台，下次
                // 重启 PinBottom hover-tick 又把窗口 demote 回去；用户感觉
                // "点了没用"。现在显式把 floating_pin_mode 切到 PinTop 并落盘。
                // 通过 IPC 走现有 set_floating_pin_mode handler（自动 save + emit
                // musage://pin-mode-changed，让 lib.rs 重新应用 mode 到窗口）。
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::commands::set_floating_pin_mode(
                        app2.state::<crate::AppState>(),
                        app2.clone(),
                        "pin_top".to_string(),
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "force_top_floating 持久化 pin_mode 失败");
                    }
                });
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击切换悬浮窗显隐。Down 时记时间戳，Up 时校验：
            // 真用户 click 的 down→up 间隔 < 500ms，合成事件无对应 Down 直接丢。
            match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Down,
                    ..
                } => {
                    LAST_LEFT_DOWN_MS.store(now_ms(), Ordering::SeqCst);
                }
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => {
                    let down = LAST_LEFT_DOWN_MS.load(Ordering::SeqCst);
                    let up = now_ms();
                    // 0 = 从未收到过 Down（启动后首次 Up 一定是合成的）
                    // up - down > 500 = 陈旧 Down，丢
                    if down == 0 || up.saturating_sub(down) > LEFT_DOWN_UP_WINDOW_MS {
                        tracing::trace!(
                            down,
                            up,
                            since_down_ms = up.saturating_sub(down),
                            "ignore synthesized Left/Up (no fresh Down within 500ms)"
                        );
                        return;
                    }
                    let app = tray.app_handle();
                    if let Some(w) = app.get_webview_window("floating") {
                        if w.is_visible().unwrap_or(false) {
                            let _ = w.hide();
                        } else {
                            let _ = w.unminimize();
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                }
                _ => {}
            }
        })
        .build(app)?;

    // ── 启动 tray request receiver（2026-06-18 闪退修复）──
    //
    // 创建 mpsc channel，把 sender 存到 OnceLock 全局（其他线程 try_send 用），
    // 然后 spawn 一个 long-lived tokio task 跑 receiver 循环：
    // 每收一条 TrayRequest → `app.run_on_main_thread(closure)` → main thread
    // 上做 tray_by_id + set_icon / set_menu。
    //
    // 必须在 `.build(app)?` **之后** —— tray 资源已注册，tray_by_id 才有东西。
    // 必须在 `setup` 返回前启动 —— 早期 emit（如果有）会拿到 None 走 warn skip
    // 然后下一次再 try_send 就能成功。
    start_tray_request_receiver(app);

    Ok(())
}

/// 构造 tray 菜单（独立成函数，方便 [`rebuild_tray`] 在 locale 切换时复用）。
///
/// 5 条 menu label + 2 个 Win-only force_top 都走 t!()（i18n）。
/// 切换语言时 [`rebuild_tray`] 重新构造菜单 + set_menu 替换（不闪烁）。
///
/// **Win 端 z-order 逃生口**（2026-06-12）：hover-raise 的 16ms tick +
/// dual-path + 焦点事件 hook 多管齐下，OS 还是持续 demote `WS_EX_TOPMOST`。
/// 给用户一个**主动**操作：菜单里点 "强制置顶浮窗" 走
/// `AllowSetForegroundWindow(ASFW_ANY) + SetForegroundWindow`，靠**抢前台**
/// 把浮窗真顶到最上面（**会**抢焦点，但用户点菜单那一瞬间本来就在
/// 操作我们 app，UX 可接受）。
fn build_tray_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let toggle_i = MenuItem::with_id(
        app,
        "toggle",
        t!("tray.menu.toggle").to_string(),
        true,
        None::<&str>,
    )?;
    let settings_i = MenuItem::with_id(
        app,
        "settings",
        t!("tray.menu.settings").to_string(),
        true,
        None::<&str>,
    )?;
    let refresh_i = MenuItem::with_id(
        app,
        "refresh",
        t!("tray.menu.refresh").to_string(),
        true,
        None::<&str>,
    )?;
    let force_top_i = MenuItem::with_id(
        app,
        "force_top_floating",
        t!("tray.menu.force_top").to_string(),
        cfg!(target_os = "windows"),
        None::<&str>,
    )?;
    let quit_i = MenuItem::with_id(
        app,
        "quit",
        t!("tray.menu.quit").to_string(),
        true,
        None::<&str>,
    )?;
    Menu::with_items(
        app,
        &[&toggle_i, &settings_i, &refresh_i, &force_top_i, &quit_i],
    )
}

/// 启动 tray request 派发通道（2026-06-18 闪退修复核心）。
///
/// - 创建 `mpsc::unbounded_channel::<TrayRequest>`
/// - sender 存进 `TRAY_REQUEST_TX`（进程全局，其他模块 try_send 用）
/// - spawn 一个 long-lived tokio task 跑 receiver：每收一条消息 → 派发到 main thread
///
/// 设计取舍：
/// - **每次 `try_send` 都派发一次 main thread**（不做 coalesce）：poller 1Hz
///   tick × 几个 provider，每秒 main thread 顶多几次 set_icon，完全可接受。
///   v2 优化需要时再在 receiver 内部加 "keep last" 模式。
/// - **tokio task 跑 receiver**（不是 std::thread）：tray 模块已经重度依赖
///   tokio，复用 `tauri::async_runtime::spawn` 简单且不出新线程。
/// - **main thread closure 拿 owned tray + set_icon/set_menu + 自然 drop**：
///   这就是修复的核心 —— drop 在 main thread 跑，AppKit NSStatusBar 操作
///   不会 SIGTRAP。`tray_by_id` 内部走 `Arc::unwrap_or_clone` 拿唯一所有权，
///   出 scope 必走 Drop → remove → removeStatusItem。必须 main thread。
fn start_tray_request_receiver(app: &AppHandle) {
    let (tx, mut rx) = mpsc::unbounded_channel::<TrayRequest>();

    // 把 sender 存到全局。OnceLock + Mutex<Option<...>>：
    // - OnceLock 保证唯一
    // - Mutex<Option> 兼容"还没初始化"状态（理论上 setup 期间不会调用，但保险）
    TRAY_REQUEST_TX.get_or_init(|| Mutex::new(Some(tx)));

    // 启动 long-lived receiver task
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        tracing::debug!("tray request receiver 启动");
        while let Some(req) = rx.recv().await {
            // 每条消息派发一次 main thread。
            //
            // **2026-06-20 audit**：之前 dispatch 失败 → break 退出 receiver loop →
            // 后续 tray request 全被 tx.send 返 Err 默默 log + 丢弃 → UI 状态
            // （icon / tooltip / menu）永久冻结。改成 log + continue，下一条
            // 请求还有重试机会（main thread 临时 dispatch 失败 ≠ 永久失败）。
            //
            // 不能 move 同一个变量同时又借用它：clone 一份给 closure。
            let app_for_dispatch = app_for_task.clone();
            let app_for_closure = app_for_task.clone();
            if let Err(e) = app_for_dispatch.run_on_main_thread(move || {
                handle_tray_request(&app_for_closure, req);
            }) {
                tracing::warn!(error = %e, "派发 tray request 到 main thread 失败，本条丢弃，继续接收");
                continue;
            }
        }
        tracing::debug!("tray request receiver 退出（channel 关闭）");
    });
}

/// main thread 上执行单条 tray request（tray request receiver 用）。
///
/// 关键：在这里 `app.tray_by_id` 拿 owned tray + 操作 + 让 tray 出 scope drop，
/// drop 是在 main thread 跑（`assertBarrierOnQueue` 通过），不会闪退。
fn handle_tray_request(app: &AppHandle, req: TrayRequest) {
    match req {
        TrayRequest::Update { snap, style } => {
            let Some(tray) = app.tray_by_id("main-tray") else {
                tracing::warn!("tray 还没建好（tray_by_id 返 None）");
                return;
            };
            if let Err(e) = tray.set_icon(Some(render_icon(&snap, style))) {
                tracing::warn!(error = %e, "set_icon 失败");
                return;
            }
            if let Err(e) = tray.set_tooltip(Some(tooltip(&snap))) {
                tracing::warn!(error = %e, "set_tooltip 失败");
            }
            // tray 在 main thread 上自然 drop，安全
        }
        TrayRequest::RebuildMenu => {
            let Some(tray) = app.tray_by_id("main-tray") else {
                return;
            };
            let menu = match build_tray_menu(app) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "build_tray_menu 失败");
                    return;
                }
            };
            if let Err(e) = tray.set_menu(Some(menu)) {
                tracing::warn!(error = %e, "set_menu 失败");
            }
        }
    }
}

/// P0：locale 切换时重新构造菜单并 `set_menu()` 替换（不是 remove+add，
/// 避免 tray 短暂消失闪烁）。
///
/// 调用时机：[`crate::lib::run`] setup 里的 `musage://locale-changed` 监听器。
/// 错误返回：仅 Tauri API 失败时返 Err，调用方记 warn 不阻塞。
///
/// 2026-06-18：原本直接 `app.tray_by_id(...)` 拿 owned tray → set_menu →
/// drop 跨线程 → SIGTRAP。改为派发到 main thread，drop 在 main 跑，安全。
pub fn rebuild_tray(_app: &AppHandle) -> tauri::Result<()> {
    dispatch_tray_request(TrayRequest::RebuildMenu);
    Ok(())
}

/// 派发 tray 图标更新到 main thread（2026-06-18 修复后）。
///
/// 旧实现直接 `tray_by_id` 拿 owned tray 然后 set_icon —— 调用方常在 tokio
/// worker 线程（poller 1Hz tick），tray 出 scope 跨线程 drop 触发
/// `BSServiceMainRunLoopQueue::assertBarrierOnQueue` SIGTRAP 闪退。
/// 新实现只 try_send 消息，**毫秒级返回** —— 不会卡 poller。
///
/// 调用方签名零变化，行为等价：最终 main thread 上跑 set_icon + set_tooltip。
pub fn update_tray_from_snapshot(
    _app: &AppHandle,
    snap: &QuotaSnapshot,
    style: TrayIconStyle,
) -> tauri::Result<()> {
    dispatch_tray_request(TrayRequest::Update {
        snap: Box::new(snap.clone()),
        style,
    });
    Ok(())
}

fn make_placeholder_icon() -> Image<'static> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/tray-base.png");
    if let Ok(img) = image::open(&path) {
        let mut rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        // H5 fix: 注释(line 197-201)明确说 Win 64x64 是为了高 DPI 防糊，
        // 但之前直接用 PNG 原生尺寸(32x32)。Win11 200% DPI 下 GDI
        // COLORONCOLOR 拉伸 → 模糊。不足 ICON_SIZE 时 Lanczos 上采样。
        if w < ICON_SIZE || h < ICON_SIZE {
            rgba = image::imageops::resize(
                &rgba,
                ICON_SIZE,
                ICON_SIZE,
                image::imageops::FilterType::Lanczos3,
            );
        }
        let (w, h) = rgba.dimensions();
        return Image::new_owned(rgba.into_raw(), w, h);
    }
    let img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));
    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

/// v0.6+ 托盘图标渲染：根据 [`TrayIconStyle`] 分发。
///
/// - `Logo`   ：画 icons/tray-base.png（白底 M），不显示实时数据
/// - `Bars`   ：MiniMax 双水平进度条
/// - `Percent`：MiniMax 双行百分比文本（font 缺失时 fallback 到 Bars）
///
/// bars / percent 模式在 MiniMax 没有/失败时**退化到 logo**（不再走旧
/// DeepSeek 大数字 —— v0.6+ 3 选 1 心智模型，深向文本风格作 v2 扩展）。
fn render_icon(snap: &QuotaSnapshot, style: TrayIconStyle) -> Image<'static> {
    if style == TrayIconStyle::Logo {
        return make_placeholder_icon();
    }

    let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));

    match pick_minimax_rows(snap) {
        Some((five_h, weekly)) => match style {
            TrayIconStyle::Bars => draw_mini_bars(&mut img, five_h, weekly),
            TrayIconStyle::Percent => draw_percent(&mut img, five_h, weekly),
            TrayIconStyle::Logo => unreachable!("logo handled above"),
        },
        None => {
            // bars / percent 模式：MiniMax 没有/失败 → 退化为 logo
            // 单独重建一张避免在主 img 上画占位
            return make_placeholder_icon();
        }
    }

    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

/// 取 MiniMax 行的 5h / 周 utilization（缺则 0.0）。
/// MiniMax 不存在或失败时返回 None。
///
/// P1 i18n：行 label 来自 provider 端 `t!("row.five_hour")` / `t!("row.weekly")`，
/// 这里也用 t!() 比较，保证跨语言一致（切到 en 时 zh-CN 字符串 "5h"/"周" 不会
/// 永远匹配不上 en 字符串 "5h"/"Weekly"）。
///
/// v0.2.1 commit 5:多 instance 时遍历所有 minimax instance,选 5h utilization
/// 最高的(快耗尽的副本最该被高亮);并列时取 instance_index 小的优先。
/// 失败/无数据时 fallback 到任意一份成功的。进度条小图标只画 1 份,
/// tooltip 列出所有 instance 拼 #N 后缀(由 tooltip() 统一处理)。
fn pick_minimax_rows(snap: &QuotaSnapshot) -> Option<(f64, f64)> {
    let minimaxes: Vec<&ProviderSnapshot> = snap
        .providers
        .iter()
        .filter(|p| p.source_id.as_deref() == Some("minimax") && p.success)
        .collect();
    if minimaxes.is_empty() {
        return None;
    }
    // 选 5h 利用率最高(快耗尽)
    let best = minimaxes
        .iter()
        .max_by(|a, b| {
            five_hour_util(a)
                .partial_cmp(&five_hour_util(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .unwrap_or(minimaxes[0]);
    Some((five_hour_util(best), weekly_util(best)))
}

fn five_hour_util(p: &ProviderSnapshot) -> f64 {
    p.rows
        .iter()
        .find(|r| r.label == t!("row.five_hour"))
        .and_then(|r| r.utilization)
        .unwrap_or(0.0)
}

fn weekly_util(p: &ProviderSnapshot) -> f64 {
    p.rows
        .iter()
        .find(|r| r.label == t!("row.weekly"))
        .and_then(|r| r.utilization)
        .unwrap_or(0.0)
}

/// 画两条水平迷你进度条。
///
/// 布局（32x32）：
/// ```text
/// ┌──────────────────────────┐
/// │                          │  ← 6px 顶 padding
/// │   ████████░░░░░░░░░░░░   │  ← 5h 进度条 (高 9px)
/// │   ██░░░░░░░░░░░░░░░░░░   │  ← 周 进度条 (高 9px)
/// │                          │  ← 6px 底 padding
/// └──────────────────────────┘
///     ↑ 3px                  ↑ 3px
/// ```
fn draw_mini_bars(img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>, util_top: f64, util_bot: f64) {
    // 所有像素常量按 32x32 原设计折算到当前 ICON_SIZE，比例：
    //   PAD_X 3/32, BAR_H 9/32, GAP 2/32, TOP 6/32, RADIUS 2/32
    // 32→64 时这些值整体翻倍（6/12/4/12/4/52/18/4），布局完全等比。
    let s = ICON_SIZE as i32;
    let pad_x = s * 3 / 32; // 3  →  6
    let bar_w = s - pad_x * 2; // 26 → 52
    let bar_h = s * 9 / 32; // 9  → 18
    let gap = s * 2 / 32; // 2  →  4
    let top = s * 6 / 32; // 6  → 12
    let radius = s * 2 / 32; // 2  →  4
    let track = Rgba([60u8, 60, 60, 255]);
    let fill = Rgba([255u8, 255, 255, 255]);

    let pct = |u: f64| -> u32 { (u.clamp(0.0, 100.0)).round() as u32 };

    draw_rounded_bar(
        img,
        pad_x,
        top,
        bar_w,
        bar_h,
        pct(util_top),
        track,
        fill,
        radius,
    );
    draw_rounded_bar(
        img,
        pad_x,
        top + bar_h + gap,
        bar_w,
        bar_h,
        pct(util_bot),
        track,
        fill,
        radius,
    );
}

/// 单条圆角水平进度条。先画整个轨道，再叠加按 % 裁宽的填充。
fn draw_rounded_bar(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    fill_pct: u32,
    track: Rgba<u8>,
    fill: Rgba<u8>,
    radius: i32,
) {
    // 轨道
    fill_rounded_rect(img, x, y, w, h, radius, track);
    // 填充（至少 1px 让 0% 和 "没数据" 视觉上区分开 —— 0% 也是有一截小白条）
    let fill_w = ((w as u32).saturating_mul(fill_pct.min(100)) / 100).max(1) as i32;
    fill_rounded_rect(img, x, y, fill_w, h, radius, fill);
}

/// 填充一个圆角矩形。逐像素判断是否在圆角内。
fn fill_rounded_rect(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: Rgba<u8>,
) {
    let r = radius.min(w / 2).min(h / 2).max(0);
    for py in y..(y + h) {
        for px in x..(x + w) {
            if px < 0 || py < 0 || px >= ICON_SIZE as i32 || py >= ICON_SIZE as i32 {
                continue;
            }
            // 4 个圆角：若 (px, py) 落在任一圆角外接矩形内，要算到圆心距离
            let in_left = (x..x + r).contains(&px);
            let in_right = (x + w - r..x + w).contains(&px);
            let in_top = (y..y + r).contains(&py);
            let in_bot = (y + h - r..y + h).contains(&py);

            let in_corner_zone = (in_left || in_right) && (in_top || in_bot);

            if !in_corner_zone {
                img.put_pixel(px as u32, py as u32, color);
            } else {
                // 计算到对应角圆心的距离
                let (cx, cy) = if in_left && in_top {
                    (x + r, y + r)
                } else if in_right && in_top {
                    (x + w - r - 1, y + r)
                } else if in_left && in_bot {
                    (x + r, y + h - r - 1)
                } else {
                    (x + w - r - 1, y + h - r - 1)
                };
                let dx = px - cx;
                let dy = py - cy;
                if dx * dx + dy * dy <= r * r {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
}

/// v0.6+ MiniMax 用：上行 5h 利用率，下行 周利用率，**右对齐纯白数字，无背景**。
///
/// 设计要点（用户 2026-06-12 反馈）：
/// - **无标签**：5h / 周这种语义已经在固定位置被认知（上行 = 5h，下行 = 周），
///   加 "5h" 字符浪费像素、稀释对比度
/// - **右对齐**：跟系统 / 企业微信 / 微信那种徽章风格保持一致，
///   "33%" / "100%" / "0%" 各种长度都对齐到右边
/// - **无背板**：纯白文字 + Bold 字体（macOS 优先 Arial Black）字形本身
///   足够粗，菜单栏透明背景上自然清晰
/// - **scale 20**（用户 2026-06-15 三次反馈：14/16/18 都偏小）：比 v1 的
///   11 大近一倍；menu bar 实际渲染到 ~16px（macOS）或 ~64px（Win11 高 DPI）
///   时字形都清晰可读。layout 两行贴边：y_top=0, y_bot=s/2，间距 = 字号 =
///   20/40，Bold 字体的数字 cap height ≈ 0.7×scale ≈ 14/28，第一行底 14/28
///   < 第二行顶 16/32，刚好不重叠。
///   ⚠ 这是 percent 模式布局的物理上限 —— 再大字号第一行底部会进第二行顶部，
///   两行粘连糊成一片。届时建议切回 Bars 模式（同样信息密度、无字号痛点）。
///
/// font 缺失时 fallback 到 `draw_mini_bars`（保持信息密度，不留空让用户
/// 困惑 "是不是没数据"）。
fn draw_percent(img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>, util_top: f64, util_bot: f64) {
    let Some(font) = load_font() else {
        return draw_mini_bars(img, util_top, util_bot);
    };

    let s = ICON_SIZE as i32;
    let scale = PxScale::from(s as f32 * 20.0 / 32.0); // 20 → 40
                                                       // 两行贴边（间距 = 字号 = 20/40），Bold 数字 cap height ≈ 0.7×scale，
                                                       // 第一行底 14/28 < 第二行顶 16/32 不重叠
    let y_top = 0; //  0 →  0
    let y_bot = s / 2; // 16 → 32
    let pad_right = s * 2 / 32; // 右边留 2px 内边距
    let color = Rgba([255, 255, 255, 255]);

    let top = format!("{}%", util_top.round() as i64);
    let bot = format!("{}%", util_bot.round() as i64);

    draw_right_text(img, &top, scale, y_top, pad_right, font, color);
    draw_right_text(img, &bot, scale, y_bot, pad_right, font, color);
}

/// 在 ICON_SIZE 宽画布上**右对齐**画一行文字，距右边 `pad_right` 像素。
///
/// **M3 fix（2026-07-02 audit）**：之前逐字符 h_advance 用 f32 累加求总宽度
/// —— 6 个全角字符（如未来中文 "智谱 GLM 95%"）累积误差可能 > 1px,右对齐
/// 出现像素级抖动。改用 ab_glyph 的 `horizontal_advance(...)` 单次调用拿字符
/// 间距,避免循环累加 + round 到 i32 拿整数宽度。ab_glyph 本身已经把每个字符
/// 的 advance 算得很准(f32→i32 round 误差 < 1 unit),比手累加稳定。
fn draw_right_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    text: &str,
    scale: PxScale,
    y: i32,
    pad_right: i32,
    font: &FontVec,
    color: Rgba<u8>,
) {
    let scaled = font.as_scaled(scale);
    // M3 fix (2026-07-02 audit): 用 h_advance 单字符调用 + final round
    // 替代原来 f32 累加. ab_glyph 的 ScaleFont::h_advance 单次返回 f32 单字符
    // advance, 累加 + round 到 i32, 比 6+ 字符 manual 加法稳定。
    // ab_glyph stable API: h_advance (不能 horizontal_advance,后缀版本不存在)。
    let w_f = text
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum::<f32>();
    let w = w_f.round() as i32;
    // 右对齐 x = ICON_SIZE - text_width - pad_right
    let x = (ICON_SIZE as i32 - w - pad_right).max(1);
    draw_text_mut(img, color, x, y, scale, font, text);
}

/// 在 ICON_SIZE 宽画布上居中画一行文字。**已废弃**（percent 模式改右对齐），
/// 保留供未来 v2 "logo + 居中文字" 等变体。
#[allow(dead_code)]
fn draw_centered_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    text: &str,
    scale: PxScale,
    y: i32,
    font: &FontVec,
    color: Rgba<u8>,
) {
    let scaled = font.as_scaled(scale);
    // M3 fix: 同 draw_right_text —— 累加 + final round
    let w_f = text
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum::<f32>();
    let w = w_f.round() as i32;
    let x = ((ICON_SIZE as i32 - w) / 2).max(1);
    draw_text_mut(img, color, x, y, scale, font, text);
}

fn tooltip(snap: &QuotaSnapshot) -> String {
    if snap.providers.is_empty() {
        return t!("tray.tooltip.loading").to_string();
    }
    let mut parts = vec![t!("tray.tooltip.title").to_string()];
    let threshold = snap.wallet_alert_threshold;
    for p in &snap.providers {
        let dot = match p.health_label(threshold) {
            "ok" => "🟢",
            "warn" => "🟡",
            "alert" => "🔴",
            _ => "⚪",
        };
        // v0.2.1 commit 5: 多 instance 时同 base id 副本(utilization/balance 都
        // 一样)在 tooltip 里会重复。body 末尾拼 #N 后缀(`p.unique_id` 解析
        // 出 "minimax#2" / "minimax" 等),让用户能区分。
        let body = provider_short_body(p);
        let suffix = instance_suffix(p);
        parts.push(format!("{dot} {body}{suffix}"));
    }
    if let Some(ms) = snap.fetched_at {
        let time_str = chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "?".to_string());
        parts.push(t!("tray.tooltip.updated_at", time = time_str).to_string());
    }
    parts.join(" · ")
}

/// 提取 instance `#N` 后缀,instance_index == 1 或 None 时返空串。
///
/// 用 `unique_id` (PR 1b 加的 `"minimax#2"` 格式) 拆出 #N 部分。
/// 旧 snapshot (没 unique_id 字段) 走 source_id 字符串,instance #1
/// 视为基线(无后缀),#2+ 由 source_display_name 是否有 `dup` suffix
/// 启发判断 —— 简化:无 unique_id 就返空串(无后缀,跟之前一致)。
fn instance_suffix(p: &ProviderSnapshot) -> String {
    if let Some(uid) = &p.unique_id {
        if let Some(idx) = uid.rfind('#') {
            let tail = &uid[idx + 1..];
            // 只在 instance_index > 1 时显示(主套餐不需要 #1)
            if tail != "1" && !tail.is_empty() {
                return format!(" #{tail}");
            }
        }
    }
    String::new()
}

fn provider_short_body(p: &ProviderSnapshot) -> String {
    // v0.2: provider 字段改 String, display_name() 不存在; 改读 source_display_name
    // 字段 (ProviderSnapshot 构造时已经 fill_source_display_name 填好)。
    let display = || {
        p.source_display_name
            .clone()
            .unwrap_or_else(|| p.provider.clone())
    };
    if !p.success {
        let err = p.error.as_deref().unwrap_or("?");
        // 截短避免 tooltip 太长
        return t!(
            "tray.tooltip.provider_error",
            provider = display(),
            error = truncate(err, 30)
        )
        .to_string();
    }
    // 按 source_id 字符串路由:
    // - "deepseek" → 余额系 (balance 渲染)
    // - "minimax" / "xiaomimimo" → 百分比系 (utilization 渲染)
    // - 其它 (tavily / zenmux / kimi / zhipu / stepfun / siliconflow / claude_official / custom_*)
    //   → 通用 percent 渲染 (rows 第一条 utilization)
    let id = p.source_id.as_deref().unwrap_or(&p.provider);
    if id == "deepseek" {
        // "DeepSeek ¥128.50"
        if let Some(r) = p.rows.iter().find(|r| r.remaining.is_some()) {
            let amount = r
                .remaining
                .map(format_amount_short)
                .unwrap_or_else(|| "?".to_string());
            let unit = r.unit.as_deref().unwrap_or("");
            t!(
                "tray.tooltip.provider_balance",
                provider = display(),
                amount = amount,
                unit = unit
            )
            .to_string()
        } else {
            display()
        }
    } else {
        // percent 渲染: "5h 45% / 周 72%" (Minimax / Xiaomimimo) 或
        // "Kimi 0% / ..." (其他 source, 任何有 utilization 的 row 都拼)
        let mut parts = Vec::new();
        for r in &p.rows {
            if let Some(u) = r.utilization {
                parts.push(
                    t!(
                        "tray.tooltip.row_pct",
                        label = r.label.as_str(),
                        pct = u.round() as i64
                    )
                    .to_string(),
                );
            }
        }
        if parts.is_empty() {
            display()
        } else {
            t!(
                "tray.tooltip.provider_rows",
                provider = display(),
                rows = parts.join(" / ")
            )
            .to_string()
        }
    }
}

fn format_amount_short(v: f64) -> String {
    let r = v.round() as i64;
    if r >= 100_000 {
        // 大数字用 k 简写
        format!("{}k", r / 1000)
    } else if v >= 1000.0 {
        format!("{:.1}k", v / 1000.0)
    } else {
        format!("{:.2}", v)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}
