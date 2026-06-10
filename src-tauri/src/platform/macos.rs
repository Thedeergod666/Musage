//! macOS 特定：把浮窗的 NSWindow level 直接设到非 0 位置，实现"始终置底"。
//!
//! 三个 level 概念：
//! - `LEVEL_BELOW_NORMAL = -1`：在 `kCGNormalWindowLevel` 之下 1 格，所有普通 app
//!   窗口都在我们之上，但我们在桌面背景之上。PinBottom 模式用它。
//! - `LEVEL_FLOATING = 3`：就是 `kCGFloatingWindowLevel`，相当于 Tauri 的
//!   `set_always_on_top(true)`。PinTop 模式用它，hover 临时置顶也用它。
//!
//! 同时启动一个 background thread，每 50ms 轮询 `NSEvent.mouseLocation()`，
//! 与窗口的 `frame` 做点-in-rect 判断；命中 → 切到 floating level，移出 → 切回 below。
//! 因为窗口在 level=-1 时被其它 app 盖住，浏览器侧 `mouseenter` 事件触发不到，
//! 必须在 OS 层面做全局鼠标位置监听。
//!
//! 同一时间只能有一个 hover tracker（PinBottom 模式）。用全局 flag `TRACKER_RUNNING`
//! 防重入；用户切到其它模式时调 [`stop_hover_tracker`] 终止。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use objc2_app_kit::{NSEvent, NSWindow};
use objc2_core_graphics::{kCGFloatingWindowLevel, kCGNormalWindowLevel, CGWindowLevel};
use objc2_foundation::NSRect;
use tauri::{AppHandle, Manager, Runtime};

/// 始终在底部：在 kCGNormalWindowLevel 之下 1 格。
/// 比桌面背景高，比所有普通 app 窗口低 → macOS 调度会一直把我们压在最底。
pub const LEVEL_BELOW_NORMAL: CGWindowLevel = kCGNormalWindowLevel - 1;

/// 始终在顶部：等于 kCGFloatingWindowLevel。
pub const LEVEL_FLOATING: CGWindowLevel = kCGFloatingWindowLevel;

static TRACKER_RUNNING: AtomicBool = AtomicBool::new(false);

// ── 公开 API ──

/// PinBottom 模式启动时调：把 level 切到 below-normal，并启动 hover tracker。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    set_window_level(app, LEVEL_BELOW_NORMAL);
    start_hover_tracker(app.clone());
}

/// PinTop 模式：level 切到 floating，hover tracker 不需要（窗口一直置顶）。
pub fn set_window_pin_top<R: Runtime>(app: &AppHandle<R>) {
    stop_hover_tracker();
    set_window_level(app, LEVEL_FLOATING);
}

/// Normal 模式：level 切回 0，hover tracker 不需要。
pub fn set_window_normal<R: Runtime>(app: &AppHandle<R>) {
    stop_hover_tracker();
    set_window_level(app, kCGNormalWindowLevel);
}

/// hover 状态切换：仅当 tracker 判定有效时落到 main thread 改 level。
/// 当前 PinTop / Normal 模式下 tracker 不在跑，但前端可能仍然会发信号，
/// 所以这里做幂等处理：tracker 关闭时一律 no-op。
pub fn set_window_hover_raise<R: Runtime>(app: &AppHandle<R>, hovering: bool) {
    if !TRACKER_RUNNING.load(Ordering::SeqCst) {
        return;
    }
    let level = if hovering { LEVEL_FLOATING } else { LEVEL_BELOW_NORMAL };
    set_window_level(app, level);
}

pub fn stop_hover_tracker() {
    TRACKER_RUNNING.store(false, Ordering::SeqCst);
}

// ── 内部 ──

/// 把浮窗的 NSWindow level 切到 `level`。dispatch 到 main thread（AppKit 强制要求）。
pub fn set_window_level<R: Runtime>(app: &AppHandle<R>, level: CGWindowLevel) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(ptr) = win.ns_window() {
                if !ptr.is_null() {
                    // SAFETY: `ptr` 来自 webview_window 的 NSWindow，整个 app 生命周期有效。
                    let window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
                    window.setLevel(level as _);
                    // 关键：默认 `hidesOnDeactivate=true` 会在 app 失焦时把窗口一起藏起来，
                    // PinBottom 模式下必须设为 false，否则鼠标一离开焦点窗口就消失。
                    window.setHidesOnDeactivate(false);
                }
            }
        }
    });
}

/// 在 main thread 拿窗口的当前 frame（屏幕坐标系，原点左下）。
/// 用 mpsc channel 把值从 main thread 同步送回调用方。
/// 拿不到（窗口已销毁/未建好/ns_window 失败）→ 返回 None，调用方跳过这次轮询。
fn get_window_frame_on_main<R: Runtime>(app: &AppHandle<R>) -> Option<NSRect> {
    let (tx, rx) = mpsc::channel::<Option<NSRect>>();
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        let result = (|| -> Option<NSRect> {
            let win = app2.get_webview_window("floating")?;
            let ptr = win.ns_window().ok()?;
            if ptr.is_null() {
                return None;
            }
            let window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
            Some(window.frame())
        })();
        let _ = tx.send(result);
    });
    let frame = rx.recv().ok().flatten()?;
    if frame.size.width > 0.0 && frame.size.height > 0.0 {
        Some(frame)
    } else {
        None
    }
}

/// 启动 background thread 轮询鼠标位置。已存在则忽略。
pub fn start_hover_tracker<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }
    thread::Builder::new()
        .name("pin-bottom-hover-tracker".into())
        .spawn(move || {
            tracing::debug!("pin-bottom hover tracker 启动");
            let mut last_raised = false;
            while TRACKER_RUNNING.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(50));
                // mouseLocation 在 macOS 上是 thread-safe 的，可从任意线程调
                let mouse = NSEvent::mouseLocation();
                // frame 必须从 main thread 拿
                let Some(frame) = get_window_frame_on_main(&app) else {
                    continue;
                };
                let inside = mouse.x >= frame.origin.x
                    && mouse.x <= frame.origin.x + frame.size.width
                    && mouse.y >= frame.origin.y
                    && mouse.y <= frame.origin.y + frame.size.height;
                if inside != last_raised {
                    last_raised = inside;
                    let level = if inside { LEVEL_FLOATING } else { LEVEL_BELOW_NORMAL };
                    tracing::trace!(?level, inside, "hover 状态变化，切 level");
                    set_window_level(&app, level);
                }
            }
            tracing::debug!("pin-bottom hover tracker 退出");
        })
        .expect("spawn hover tracker thread");
}
