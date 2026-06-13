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
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowRect, SetWindowPos, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

/// Hover tracker thread 是否已启动（idempotent 防重入）。
static TRACKER_RUNNING: AtomicBool = AtomicBool::new(false);

/// 鼠标 hover 时是否同步切 z-order：仅 PinBottom 模式置 true。
/// 这个开关只影响 z-order 切换；hover 事件 emit 不受影响
/// （**永远 emit**），因为前端 iOS 26 玻璃 hover 效果需要它，不分 pin mode。
static LEVEL_SWITCHING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 浮窗的 z-order 模式。直接走 `SetWindowPos` 的 3 个目标值之一。
#[derive(Debug, Clone, Copy)]
enum ZOrder {
    /// `HWND_TOPMOST` —— 高于所有其它窗口（含 topmost 类别）。PinTop
    /// 模式 + PinBottom hover 进窗口时用。
    TopMost,
    /// `HWND_BOTTOM` —— 低于所有 normal 窗口，对应 macOS 那套的
    /// `kCGNormalWindowLevel - 1`（"below normal"）。PinBottom
    /// 鼠标离开时用。
    ///
    /// **为什么不直接用 `set_always_on_top(false)`（即 HWND_NOTOPMOST）**：
    /// 后者只把 HWND 的 WS_EX_TOPMOST 标志位清掉，**不动 z-order**。
    /// 浮窗之前在 topmost 位置，清掉 topmost 标志后会落回 "top of
    /// normal z-order"，**视觉上还是盖在其它 app 之上**，用户感知
    /// 到的就是"鼠标移开浮窗它没掉下去"。HWND_BOTTOM 是显式"塞到
    /// 正常 z-order 最底"，跟 macOS 那个 -1 行为对得起来。
    Bottom,
    /// `HWND_NOTOPMOST` —— 清 topmost 标志、保留 z-order。Normal
    /// 模式用：用户没要"始终 topmost"也别强塞 HWND_BOTTOM（那会
    /// 让窗口被其它所有 app 盖住，对 Normal 模式过度）。
    NotTopMost,
}

/// 把浮窗的 z-order 设到指定模式。直接走 `SetWindowPos`，绕开
/// Tauri/tao 的 `set_always_on_top` 抽象层 —— 后者只能 toggle topmost
/// 标志位，不能塞到 HWND_BOTTOM。
///
/// 必须在主线程调（`SetWindowPos` 影响窗口 Z 顺序，Win 通常要求）。
/// 调用方（hover tracker / pin mode 设置）都通过 `app.run_on_main_thread`
/// 派发；这里 unsafe 由调用方担保。
unsafe fn apply_z_order(hwnd: *mut core::ffi::c_void, z: ZOrder) {
    let insert_after = match z {
        ZOrder::TopMost => HWND_TOPMOST,
        ZOrder::Bottom => HWND_BOTTOM,
        ZOrder::NotTopMost => HWND_NOTOPMOST,
    };
    // windows-sys 0.59 的 SetWindowPos 第二参是 `HWND`（=`*mut c_void`），
    // 不是 `Option<HWND>`（tao 那种 high-level `windows` crate 才包 Option），
    // 直接传 raw pointer 就行。
    let _ = SetWindowPos(
        hwnd,
        insert_after,
        0,
        0,
        0,
        0,
        SWP_ASYNCWINDOWPOS | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}

// ── 公开 API ──

/// PinBottom 模式启动时调：把窗口塞到 HWND_BOTTOM（z-order 最底，
/// 对应 macOS 那个 `LEVEL_BELOW_NORMAL`），并开启 hover 切 z-order。
/// tracker 已由 `start_hover_emitter` 在 app 启动时拉起，这里只翻开关。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(hwnd) = win.hwnd() {
                unsafe { apply_z_order(hwnd.0, ZOrder::Bottom) };
                tracing::trace!("PinBottom: set z-order Bottom");
            }
        }
    });
    LEVEL_SWITCHING_ACTIVE.store(true, Ordering::SeqCst);
    // 防御：lib.rs setup 之外的路径走到这（理论上不会），保底拉起 tracker
    start_hover_emitter(app.clone());
}

/// PinTop 模式：z-order 切到 TopMost，关闭 hover 切换（窗口已经始终
/// 置顶）。hover 事件 emit 不变，前端玻璃效果继续受惠。
pub fn set_window_pin_top<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(hwnd) = win.hwnd() {
                unsafe { apply_z_order(hwnd.0, ZOrder::TopMost) };
                tracing::trace!("PinTop: set z-order TopMost");
            }
        }
    });
}

/// Normal 模式：z-order 切到 NotTopMost（清 topmost 标志、保留 z-order），
/// 关闭 hover 切换。
pub fn set_window_normal<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(hwnd) = win.hwnd() {
                unsafe { apply_z_order(hwnd.0, ZOrder::NotTopMost) };
                tracing::trace!("Normal: set z-order NotTopMost");
            }
        }
    });
}

/// hover 切 z-order 的"前端兜底信号"：Win 上 tracker 已自行处理，
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

                // (1) 永远 emit —— 驱动前端 body[data-hover]，让 CSS hover
                //     玻璃效果不依赖 WebView2 鼠标事件
                if inside != last_inside {
                    if let Err(e) = app.emit("musage://floating-hover", inside) {
                        tracing::trace!(error = %e, "emit hover 失败");
                    }
                }

                // (2) PinBottom 模式：切 z-order
                if LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                    let app2 = app.clone();
                    if inside {
                        // **关键：inside 时每个 poll cycle 都断言 TopMost**。
                        // 之前用 last_inside != inside 做 edge trigger，
                        // 行为是「只有鼠标进出窗口那一帧切 z-order」——
                        // Win 切完后，OS 调度（用户点其它 app → 其它窗口
                        // HWND_TOP）会把浮窗 z-order 重新打到 BOTTOM 一带，
                        // 我们的 tracker 不会 re-assert，浮窗就一直沉底。
                        // 改成 level trigger：每 50ms 断言一次。
                        let _ = app.run_on_main_thread(move || {
                            if let Some(win) = app2.get_webview_window("floating") {
                                if let Ok(hwnd) = win.hwnd() {
                                    unsafe { apply_z_order(hwnd.0, ZOrder::TopMost) };
                                }
                            }
                        });
                    } else if last_inside {
                        // 鼠标刚离开：edge-trigger 切到 BOTTOM（真正的"置底"）。
                        // 离开后不再断言，让 OS 正常 z-order 接管。
                        tracing::trace!("PinBottom: 鼠标离开浮窗 → z-order Bottom");
                        let _ = app.run_on_main_thread(move || {
                            if let Some(win) = app2.get_webview_window("floating") {
                                if let Ok(hwnd) = win.hwnd() {
                                    unsafe { apply_z_order(hwnd.0, ZOrder::Bottom) };
                                }
                            }
                        });
                    }
                }

                last_inside = inside;
            }
        })
        .expect("spawn hover emitter thread");
}

// ── 内部 ──

/// Hit test：鼠标位置是否在浮窗 rect 内。
///
/// **之前**：rect 内 + `WindowFromPoint` + `GetAncestor(GA_ROOT)` 严格
/// 判定 "topmost window 是浮窗自己"，对应 macOS 那套
/// `windowNumberAtPoint` 行为。意图是"被其它 app 窗口盖住的区域不触发"，
/// 但在 Win + 透明浮窗上有隐性 bug —— 用户报"点击别处窗口后浮窗
/// 一直置底"，tracing 不到具体原因，但**症状明确是 WindowFromPoint
/// 在 Win 上对 focus 在其它窗口的状态下行为不可靠**（可能返回某个
/// 临时 topmost 窗口 / 不可见 overlay / 其它 app 的隐藏 rect，判
/// false 后 tracker 永远不 raise）。
///
/// **改成**：只用 `point_in_rect`。判断粒度从"topmost z-order 在
/// 浮窗自己的根"放宽到"鼠标在浮窗屏幕 rect 内"。这是用户心智模型
/// —— "我把鼠标移到浮窗能看见的地方"。代价：被其它窗口完全覆盖的
/// 区域也会 raise（不过那种情况浮窗本来就看不见，用户不会去 hover）。
///
/// 跟 macOS 行为略有偏差（macOS 那套更严），但 Win 上以能 raise 为
/// 第一优先级。返回 `None` 表示本轮无法判定（窗口未上屏 / Win API
/// 失败），caller continue 即可。
fn is_cursor_inside_floating<R: Runtime>(app: &AppHandle<R>) -> Option<bool> {
    let win = app.get_webview_window("floating")?;
    let hwnd_t = win.hwnd().ok()?;
    if hwnd_t.0.is_null() {
        return None;
    }
    let our_hwnd: *mut core::ffi::c_void = hwnd_t.0;

    // SAFETY: GetCursorPos / GetWindowRect 都是 Win32 kernel call，文档
    // 明确 thread-safe，可从任意线程调。POINT/RECT 是值类型。
    unsafe {
        let mut pt: POINT = std::mem::zeroed();
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(our_hwnd, &mut rect) == 0 {
            return None;
        }
        Some(point_in_rect(pt, &rect))
    }
}

#[inline]
fn point_in_rect(pt: POINT, rect: &RECT) -> bool {
    pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
}
