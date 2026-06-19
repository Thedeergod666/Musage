//! 平台特定代码 —— 当前只有 macOS 有非 stub 实现。
//!
//! 设计目标：让上层 (commands.rs / lib.rs) 不用 `#[cfg]`，
//! 直接 `crate::platform::set_window_pin_bottom(&app)` 调，编译器在
//! 非 macOS 平台自动选 stub 版本（stub 内部走 Tauri 原生 `set_always_on_top`）。
//!
//! ## 为什么需要 macOS 特定代码？
//!
//! 在 macOS 上，"置底" 不是仅靠 `set_always_on_top(false)` 能实现的。
//! 那样做的话窗口就只是 `kCGNormalWindowLevel = 0`，macOS 前台调度
//! 会把其它正在激活的 app 窗口叠在我们上面，浮窗就"消失"了。
//!
//! macOS 原生的"始终在底部"做法是：把 NSWindow 的 level 设到
//! `kCGNormalWindowLevel - 1`（即 `-1`），这样：
//! - 高于桌面背景 (`kCGDesktopWindowLevel`)
//! - 低于所有普通应用窗口 (`kCGNormalWindowLevel` = 0) 来自所有 app
//! - 低于所有浮动窗口 (`kCGFloatingWindowLevel` = 3)
//! - 低于状态栏 / 菜单 (`kCGStatusWindowLevel` = 25 等)
//!
//! 配合全局鼠标位置轮询 (因为窗口在 level -1 时被其它 app 盖住，
//! JS mouseenter 事件触发不到)，实现 "hover 临时置顶"。

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

/// 引入 Manager trait，让 `app.get_webview_window(...)` 可用（Tauri 2 需要显式 use）。
use tauri::Manager;

#[cfg(target_os = "macos")]
pub use self::macos::*;
#[cfg(target_os = "windows")]
pub use self::windows::*;

// ── Linux stub：EWMH 不支持原生"置底"，会降级成普通窗口（已知限制）──
// Windows 已经走 windows.rs 自己的 tracker 实现（对称 macOS 那套），
// 这里只留 Linux 作为最后兜底。
#[cfg(target_os = "linux")]
pub fn set_window_pin_bottom<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(false);
    }
}
#[cfg(target_os = "linux")]
pub fn set_window_pin_top<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(true);
    }
}

/// **L11 fix（2026-06-19）**：跨平台 PinTop 语义差异（已知限制，不打算消除）：
///
/// - **macOS** （[`macos::set_window_pin_top`]）：走 `kCGFloatingWindowLevel = 3`。
///   高于所有普通 app 窗口（`kCGNormalWindowLevel = 0`），**但**低于状态菜单栏
///   （`kCGStatusWindowLevel = 25`）。这是 macOS 平台限制 —— 用更高 level 会
///   把系统状态栏（Wi-Fi/电池/时间）盖住，不被允许。
/// - **Linux** （上方 stub）：走 EWMH `_NET_WM_STATE_ABOVE`，行为依赖 WM；
///   大多数桌面环境会把窗口放在所有 normal 层之上，**包括**任务栏/状态栏。
/// - **Windows** （[`windows::set_window_pin_top`]）：走 `HWND_TOPMOST`，高于
///   所有 normal 窗口 + 任务栏（OS 行为）。
///
/// **结论**：macOS 用户看到的状态菜单栏仍会盖在浮窗上；Linux/Windows 不会。
/// 设计层面接受这个差异 —— 主动选择 macOS UX 妥协，避免状态栏被覆盖。
#[cfg(target_os = "linux")]
pub fn set_window_normal<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(false);
    }
}
#[cfg(target_os = "linux")]
pub fn set_window_hover_raise<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    hovering: bool,
) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(hovering);
    }
}

/// 始终运行的全局鼠标位置广播器：macOS / Windows 上必需
/// （非焦点窗口 + WebView2 透明窗的 mouseenter/leave 行为不可靠），
/// tracker 自己处理。Linux WebKitGTK 浏览器层 mouseenter/leave 工作正常，
/// 前端 JS 挂 body listener 就够 —— stub 真 no-op。
#[cfg(target_os = "linux")]
pub fn start_hover_emitter<R: tauri::Runtime>(_app: tauri::AppHandle<R>) {}

// ── Fullscreen watcher：非 macOS 暂未实现 ──
// Win/Linux 全屏检测 API 各家不同（Win32 / X11 / Wayland），需要单独适配。
// 设置项保留可见但开了无效，help 文字告诉用户「目前仅 macOS 生效」。
#[cfg(target_os = "linux")]
pub fn start_fullscreen_watcher<R: tauri::Runtime>(_app: tauri::AppHandle<R>) {}
#[cfg(target_os = "linux")]
pub fn set_auto_hide_in_fullscreen<R: tauri::Runtime>(
    _app: &tauri::AppHandle<R>,
    _enabled: bool,
) {}
