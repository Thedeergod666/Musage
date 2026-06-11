//! Windows 特定：PinBottom 模式的"hover 临时置顶"靠 Rust 后台线程
//! 全局轮询鼠标位置 + 窗口 rect 实现，不依赖前端 JS 的 mouseenter/leave。
//!
//! ## 为什么不在 Win 走 JS 路径？
//!
//! 早期版本跟 macOS stub 一样，让前端 JS 在 `document.body` 上挂
//! `mouseenter` / `mouseleave` 然后 `set_always_on_top` 切换。Win + WebView2
//! + 透明窗上有两个坑：
//!
//! 1. **mouseleave 在 transparent window 不可靠** —— body 是 `background:
//!    transparent`（来自 styles.css），Chromium 内部"鼠标在窗口内"的命中
//!    测试对透明区域有时不记事件，鼠标快速移出 + 切焦点会丢 leave。CSS
//!    玻璃 hover 用 `data-hover` attribute 不在意（Rust emit 兜底），但
//!    IPC 链路靠 mouseleave 触发 → 状态机卡死。
//!
//! 2. **WS_EX_TOPMOST 出生残留** —— tauri.conf.json 浮窗 `alwaysOnTop:
//!    true` 让窗口**创建时**就带 WS_EX_TOPMOST。后续 `SetWindowPos(
//!    HWND_NOTOPMOST)` 取消 topmost 在 Win API 里有文档但实现不一致，
//!    部分 Win10/11 版本会保留 topmost 行为，"鼠标移开浮窗后一直置顶"
//!    就是这个症状。
//!
//! 修这两坑靠：
//!  - **Rust 端 50ms 轮询** GetCursorPos + GetWindowRect 做 point-in-rect，
//!    跟 macOS 那套 `NSEvent.mouseLocation` 走线对称。
//!  - tauri.conf.json 把浮窗的初始 `alwaysOnTop` 去掉，让 pin 模式
//!    (`set_window_pin_bottom/top/normal`) 成为 topmost 状态的**唯一**
//!    真值源。
//!
//! ## Hover tracker 生命周期
//!
//! 跟 macos.rs 一致：
//! - 始终运行，由 lib.rs `start_hover_emitter` 拉起一次。
//! - 50ms 一次，~20Hz，单次 ~微秒级开销（GetCursorPos / GetWindowRect 都是
//!   Win32 kernel 路径，不走 user32 消息泵）。
//! - 状态变化时：
//!   - 永远 `app.emit("musage://floating-hover", inside)` 给前端
//!     （驱动 CSS `body[data-hover]` 玻璃效果）
//!   - 当 `LEVEL_SWITCHING_ACTIVE` 为 true（PinBottom 模式）时**额外**切
//!     `set_always_on_top(inside)`
//!
//! ## hit test —— 严格"未遮挡才算"
//!
//! 单纯 `point-in-rect` 太宽松：PinBottom 模式下浮窗 frame 经常被其它 app
//! 部分盖住，鼠标移到被盖区域（用户其实在跟那个 app 交互）按矩形判定会
//! 误触发 hover 置顶。Win 端用 `WindowFromPoint(pt)` 拿 topmost window 的
//! HWND，跟浮窗自己的 hwnd 比对 —— 严格只算"浮窗是最上层"的那一格。
//! 跟 macOS `+[NSWindow windowNumberAtPoint:...]` 同思路。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Runtime};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, WindowFromPoint};

/// Hover tracker thread 是否已启动（idempotent 防重入）。
static TRACKER_RUNNING: AtomicBool = AtomicBool::new(false);

/// 鼠标 hover 时是否同步切 always-on-top：仅 PinBottom 模式置 true。
/// 这个开关只影响 `set_always_on_top` 切换；hover 事件 emit 不受影响
/// （**永远 emit**），因为前端 iOS 26 玻璃 hover 效果需要它，不分 pin mode。
static LEVEL_SWITCHING_ACTIVE: AtomicBool = AtomicBool::new(false);

// ── 公开 API ──

/// PinBottom 模式启动时调：把 always-on-top 关掉，并开启 hover 切
/// always-on-top。tracker 已由 `start_hover_emitter` 在 app 启动时拉起，
/// 这里只翻开关。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(false);
    }
    LEVEL_SWITCHING_ACTIVE.store(true, Ordering::SeqCst);
    // 防御：lib.rs setup 之外的路径走到这（理论上不会），保底拉起 tracker
    start_hover_emitter(app.clone());
}

/// PinTop 模式：always-on-top 开，关闭 hover 切换（窗口已经始终置顶）。
/// hover 事件 emit 不变，前端玻璃效果继续受惠。
pub fn set_window_pin_top<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(true);
    }
}

/// Normal 模式：always-on-top 关，关闭 hover 切换。
pub fn set_window_normal<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    if let Some(win) = app.get_webview_window("floating") {
        let _ = win.set_always_on_top(false);
    }
}

/// hover 切 always-on-top 的"前端兜底信号"：Win 上 tracker 已自行处理，
/// 此处 no-op。保留是为了让 commands.rs 在跨平台调用时不必 `#[cfg]`。
pub fn set_window_hover_raise<R: Runtime>(_app: &AppHandle<R>, _hovering: bool) {
    // no-op —— tracker 自己处理
}

// ── Fullscreen watcher：Win 暂未实现 ──
// Win 全屏检测用 `GetWindowLong(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST` 这类
// 启发式不可靠（很多 app 全屏时不会把窗口设 topmost）；得 hook
// `EVENT_SYSTEM_FOREGROUND` + 几何变化等多信号源。
// 设置项保留可见但 Win/Linux 开了无效，help 文字告诉用户「目前仅 macOS 生效」。
pub fn start_fullscreen_watcher<R: Runtime>(_app: tauri::AppHandle<R>) {}
pub fn set_auto_hide_in_fullscreen<R: Runtime>(
    _app: &tauri::AppHandle<R>,
    _enabled: bool,
) {
}

/// 启动 hover emitter 线程。idempotent —— 第二次调用立即返回。
/// 由 lib.rs setup() 调一次即可。启动后整个 app 生命周期不停。
pub fn start_hover_emitter<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }
    std::thread::Builder::new()
        .name("musage-hover-emitter".into())
        .spawn(move || {
            tracing::debug!("hover emitter 启动");
            let mut last_inside = false;
            loop {
                std::thread::sleep(Duration::from_millis(50));

                let Some(inside) = is_cursor_inside_floating(&app) else {
                    continue;
                };

                if inside == last_inside {
                    continue;
                }
                last_inside = inside;

                // (1) 永远 emit —— 驱动前端 body[data-hover]，让 CSS hover
                //     玻璃效果不依赖 WebView2 鼠标事件
                if let Err(e) = app.emit("musage://floating-hover", inside) {
                    tracing::trace!(error = %e, "emit hover 失败");
                }

                // (2) PinBottom 模式：同步切 always-on-top
                if LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                    tracing::trace!(inside, "PinBottom hover 切 always-on-top");
                    if let Some(win) = app.get_webview_window("floating") {
                        let _ = win.set_always_on_top(inside);
                    }
                }
            }
        })
        .expect("spawn hover emitter thread");
}

// ── 内部 ──

/// Hit test：鼠标位置是否在浮窗**可见**（未被遮挡）区域内。
///
/// - 鼠标先得在窗口 rect 内（`GetWindowRect` → `point_in_rect`）
/// - 然后该点 topmost window 必须是浮窗自己（`WindowFromPoint` 比对 hwnd）
///
/// 返回 `None` 表示本轮无法判定（窗口未上屏 / Win API 失败），caller
/// continue 即可，下一轮再判。
fn is_cursor_inside_floating<R: Runtime>(app: &AppHandle<R>) -> Option<bool> {
    let win = app.get_webview_window("floating")?;
    // Tauri 2 `Window::hwnd()` 返回 `windows::Win32::Foundation::HWND`
    // （`pub struct HWND(pub *mut c_void)`，来自 `windows` crate 0.61）。
    // 我 Cargo.toml 里的 `windows-sys` 0.59 跟它是不同 crate，类型不互通。
    // 透 `.0` 拿 raw pointer 喂 windows-sys 0.59 的 Win32 API 就行 —— 两者
    // 底层都是 `*mut c_void`，只是 Rust 不让跨 crate 隐式转。
    let hwnd_t = win.hwnd().ok()?;
    if hwnd_t.0.is_null() {
        return None;
    }
    let hwnd_ptr: *mut core::ffi::c_void = hwnd_t.0;

    // SAFETY:
    // - GetCursorPos / GetWindowRect / WindowFromPoint 都是 Win32 kernel
    //   call，文档明确 thread-safe，可从任意线程调。
    // - POINT/RECT 是值类型，零初始化即合法。
    // - hwnd 来自 webview_window，整个 app 生命周期有效。
    unsafe {
        let mut pt: POINT = std::mem::zeroed();
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd_ptr, &mut rect) == 0 {
            return None;
        }
        if !point_in_rect(pt, &rect) {
            return Some(false);
        }
        // rect 内 → 检查是否浮窗自己是 topmost。
        // windows-sys 0.59 的 HWND = `*mut c_void`（type alias，不是 struct），
        // 所以 WindowFromPoint 直接返回 raw pointer，可以跟 hwnd_ptr 裸比。
        let topmost = WindowFromPoint(pt);
        Some(topmost == hwnd_ptr)
    }
}

#[inline]
fn point_in_rect(pt: POINT, rect: &RECT) -> bool {
    pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
}
