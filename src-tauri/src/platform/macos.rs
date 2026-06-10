//! macOS 特定：两件事
//!
//!   1. 把浮窗的 NSWindow level 直接设到非 0 位置，实现"始终置底/置顶"。
//!   2. **全局鼠标位置轮询，把 hover 状态广播给前端**
//!      —— 因为 macOS 上非 key window 不分发 mouseMoved 事件，WKWebView 的
//!      CSS `:hover` 在浮窗未聚焦时不会激活，会导致"必须先点一下窗口 hover 才生效"
//!      的体验坑。用 `NSEvent.mouseLocation` + 窗口 frame 做 point-in-rect 判断，
//!      完全绕过 WebKit 的事件流依赖。
//!
//! ## Hover tracker 生命周期
//!
//! - **始终运行**：lib.rs setup 时调一次 [`start_hover_emitter`]，整个 app 生命
//!   周期不停。idempotent，第二次调用立即返回。
//! - 每 50ms 调 `NSEvent.mouseLocation` + main thread dispatch 拿窗口 frame
//!   做 point-in-rect。开销 ~20Hz 的轻量轮询。
//! - 状态变化时：
//!   - 永远 `app.emit("musage://floating-hover", inside)` 给前端
//!     （前端拿来切 `body[data-hover]` 属性，驱动 CSS）
//!   - 当 [`LEVEL_SWITCHING_ACTIVE`] 为 true（PinBottom 模式）时**额外**切 NSWindow level
//!     —— 这是 PinBottom 模式"hover 临时置顶"的实现路径
//!
//! ## 三个 level 常量
//!
//! - `LEVEL_BELOW_NORMAL = -1` ：在 `kCGNormalWindowLevel` 之下 1 格，所有普通 app
//!   窗口都在我们之上，但我们在桌面背景之上。PinBottom 模式用它。
//! - `LEVEL_FLOATING = 3` ：就是 `kCGFloatingWindowLevel`，相当于 Tauri 的
//!   `set_always_on_top(true)`。PinTop 模式用它，hover 临时置顶也用它。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use objc2_app_kit::{NSEvent, NSWindow};
use objc2_core_graphics::{kCGFloatingWindowLevel, kCGNormalWindowLevel, CGWindowLevel};
use objc2_foundation::NSRect;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// 始终在底部：在 kCGNormalWindowLevel 之下 1 格。
/// 比桌面背景高，比所有普通 app 窗口低 → macOS 调度会一直把我们压在最底。
pub const LEVEL_BELOW_NORMAL: CGWindowLevel = kCGNormalWindowLevel - 1;

/// 始终在顶部：等于 kCGFloatingWindowLevel。
pub const LEVEL_FLOATING: CGWindowLevel = kCGFloatingWindowLevel;

/// hover emitter thread 是否已启动（idempotent 防重入）。
/// 启动后整个 app 生命周期不停，所以这里只是 "thread spawned?" 的标志，
/// 不参与运行时控制 —— 真正想动行为请改 [`LEVEL_SWITCHING_ACTIVE`]。
static TRACKER_RUNNING: AtomicBool = AtomicBool::new(false);

/// 鼠标 hover 时是否同步切 NSWindow level：仅 PinBottom 模式置 true。
/// 这个开关只影响 level 切换；hover 事件 emit 不受影响（**永远 emit**），
/// 因为前端的 iOS 26 玻璃 hover 效果需要它，不分 pin mode。
static LEVEL_SWITCHING_ACTIVE: AtomicBool = AtomicBool::new(false);

// ── 公开 API ──

/// PinBottom 模式启动时调：把 level 切到 below-normal，并开启 hover 切 level。
/// tracker 已由 [`start_hover_emitter`] 在 app 启动时拉起，这里只翻开关。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    set_window_level(app, LEVEL_BELOW_NORMAL);
    LEVEL_SWITCHING_ACTIVE.store(true, Ordering::SeqCst);
    // 防御：如果 lib.rs setup 之外的路径走到这（理论上不会），保底拉起 tracker
    start_hover_emitter(app.clone());
}

/// PinTop 模式：level 切到 floating，关闭 hover 切 level（窗口已经始终置顶）。
/// hover 事件 emit 不变，前端的玻璃效果继续受惠。
pub fn set_window_pin_top<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    set_window_level(app, LEVEL_FLOATING);
}

/// Normal 模式：level 切回 0，关闭 hover 切 level。
pub fn set_window_normal<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    set_window_level(app, kCGNormalWindowLevel);
}

/// hover 切 level 的"前端兜底信号"：macOS 上 tracker 已自行处理，此处 no-op。
/// 保留是为了让 commands.rs 在跨平台调用时不必 `#[cfg]`。
/// （Win/Linux 的 stub 会真正执行 `set_always_on_top`。）
pub fn set_window_hover_raise<R: Runtime>(_app: &AppHandle<R>, _hovering: bool) {
    // no-op —— tracker 自己处理 level 切换
}

/// 启动 hover emitter 线程。idempotent —— 第二次调用立即返回。
/// 由 lib.rs setup() 调一次即可。
///
/// 启动后整个 app 生命周期不停。20Hz 轮询，单次 ~微秒级开销。
pub fn start_hover_emitter<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }
    thread::Builder::new()
        .name("musage-hover-emitter".into())
        .spawn(move || {
            tracing::debug!("hover emitter 启动");
            let mut last_inside = false;
            loop {
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

                if inside != last_inside {
                    last_inside = inside;

                    // (1) 永远 emit —— 驱动前端 body[data-hover]，让 CSS hover 生效
                    //     不依赖 WKWebView 的 mouseMoved 事件流（macOS 非 key window 不分发）
                    if let Err(e) = app.emit("musage://floating-hover", inside) {
                        tracing::trace!(error = %e, "emit hover 失败");
                    }

                    // (2) PinBottom 模式：同步切 NSWindow level
                    if LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                        let level = if inside { LEVEL_FLOATING } else { LEVEL_BELOW_NORMAL };
                        tracing::trace!(?level, inside, "PinBottom hover 切 level");
                        set_window_level(&app, level);
                    }
                }
            }
        })
        .expect("spawn hover emitter thread");
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
