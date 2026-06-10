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

#[cfg(target_os = "macos")]
pub use self::macos::*;

// ── 非 macOS 平台：stub（走 Tauri 原生 set_always_on_top）──
// Windows: `set_always_on_top(false)` 配 `WS_EX_NOACTIVATE` 行为 OK，
//          `set_always_on_top(true)` 等价于 `HWND_TOPMOST`。
// Linux: EWMH 不支持原生"置底"，会降级成普通窗口 —— 已知限制。
#[cfg(not(target_os = "macos"))]
pub fn set_window_pin_bottom<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(false);
    }
}
#[cfg(not(target_os = "macos"))]
pub fn set_window_pin_top<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(true);
    }
}
#[cfg(not(target_os = "macos"))]
pub fn set_window_normal<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(false);
    }
}
#[cfg(not(target_os = "macos"))]
pub fn set_window_hover_raise<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    hovering: bool,
) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(hovering);
    }
}
#[cfg(not(target_os = "macos"))]
pub fn start_hover_tracker<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {
    // 非 macOS 平台：hover 监听由前端 JS mouseenter/leave 完成，不需要 OS 层面 tracker
}
#[cfg(not(target_os = "macos"))]
pub fn stop_hover_tracker() {}
