//! Windows 端 PinBottom 模式"hover 临时置顶"实现。
//!
//! ## 设计原则
//!
//! Rust 后台线程轮询全局鼠标位置 + 浮窗屏幕 rect，对照 macOS 那套
//! `NSEvent.mouseLocation` + `NSWindow.windowNumberAtPoint` 的形态。
//! 50ms tick（~20Hz）单次调用 ~微秒级 Win32 API 开销。
//!
//! ## 为什么不在 Win 走 JS 路径
//!
//! 早期版本让前端 JS 在 `document.body` 上挂 `mouseenter` / `mouseleave`
//! 然后 `set_always_on_top` 切换。Win + WebView2 + 透明窗上有两个坑：
//!
//! 1. `mouseleave` 在 transparent window 上不可靠 —— body 是
//!    `background: transparent`（来自 styles.css），Chromium 对透明区域的
//!    鼠标命中测试有时不记事件，鼠标快速移出 + 切焦点会丢 leave。CSS
//!    玻璃 hover 有 Rust emit 兜底，但 IPC 链路靠 mouseleave 触发 → 状态机
//!    卡死。
//!
//! 2. `WS_EX_TOPMOST` 出生残留 —— tauri.conf.json 浮窗 `alwaysOnTop:
//!    true` 让窗口**创建时**就带 WS_EX_TOPMOST。后续 `SetWindowPos(
//!    HWND_NOTOPMOST)` 取消 topmost 在部分 Win10/11 上保留 topmost 行为。
//!    tauri.conf.json 已经把 `alwaysOnTop` 改成 `false`，让 pin 模式
//!    (`set_window_pin_bottom/top/normal`) 成为 topmost 状态的**唯一**真值源。
//!
//! ## hit test —— 两级命中：可见即抬 + 被遮挡 dwell 抬
//!
//! 三态判定（`WindowFromPoint` + `GetAncestor(_, GA_ROOT)`）：
//! 1. 鼠标不在浮窗 rect 内 → `Outside`
//! 2. 在 rect 内、该点 topmost 窗口爬根后是浮窗自己（或其子窗口
//!    WebView2）→ `Visible`
//! 3. 在 rect 内、该点 topmost 是别的 app（浮窗被盖住）→ `Covered`
//!
//! 2026-07-20 之前是严格"未被遮挡才算"（macOS-parity，commit `88affcc`）：
//! `Covered` 一律不 raise，防止"用户其实在跟遮挡它的 app 交互"时误抬。
//! 但 Windows 用户的真实场景是浮窗**长期被最大化窗口盖住大半、只露一条
//! 边** —— 严格语义下鼠标几乎永远落在 `Covered` 区域，hover-raise 等于
//! 不存在（v0.2.4 用户实测反馈）。
//!
//! 改为两级：`Visible` 1 tick 即抬（快路径，v0.1.0 一档响应）；`Covered`
//! 需要**连续** 5 tick（250ms）dwell 才抬（慢路径）—— 鼠标路过浮窗所在
//! 屏幕区域通常 <100ms 不停留，不会误弹；用户"想看浮窗"时自然会停鼠标。
//! 抬升不抢焦点（`SWP_NOACTIVATE`），鼠标移开 150ms 后自动沉回
//! `HWND_BOTTOM`。代价：用户把鼠标停在浮窗被盖区域干别的事（>250ms）时
//! 浮窗会弹出来遮一下，移开即恢复 —— 换来主场景可用，值得。
//!
//! WebView2 是浮窗的子窗口，`WindowFromPoint` 在浮窗可见区域返回的是
//! WebView2 的 hwnd（不是浮窗的）。`GetAncestor(WebView2, GA_ROOT)` 沿
//! parent 链爬到顶层根（就是我们的浮窗），比对通过判 `Visible`。
//!
//! ## Win 端 z-order 的 sticky 性与兜底
//!
//! `HWND_TOPMOST` 是 z-order 里的一个**位置**，但 `WS_EX_TOPMOST` 本身是
//! sticky 的 —— 设置一次后 OS 不会自发清掉，不需要 macOS 那种 window
//! server 级别的持久维持。真正让历史上 hover-raise 失效的是**自己撤销
//! 自己**（见下方"为什么需要 exit hysteresis"），不是 OS demote。
//!
//! 因此新实现改为：enter/exit 各做一次 edge-trigger 切换，稳定 hover
//! 期间只每 1s 低频 re-assert 一次兜底（防止极端情况下被别的窗口管理
//! 操作顶掉），不再 20Hz 反复 `SetWindowPos` —— 那既是无效 churn，也
//! 扩大了跟 tao / WebView2 主线程窗口管理竞争的窗口。
//!
//! 万一用户场景里 raise 仍被系统策略压制（极少数），tray 菜单
//! "强制置顶浮窗" 走更暴力的路径
//! （`AllowSetForegroundWindow(ASFW_ANY) + SetForegroundWindow`），
//! 代价是抢焦点。
//!
//! ## Hover tracker 生命周期
//!
//! - 始终运行，由 `start_hover_emitter` 拉起一次
//! - 50ms tick，三态 hit test（`HitTest`）+ dwell-time hysteresis：
//!   - **`Visible` → 1 tick**：未被遮挡时第一个 tick 就采纳（v0.1.0 一档响应）
//!   - **`Covered` → 5 tick（250ms）**：被遮挡时需连续 dwell 才采纳，
//!     路过不误抬（见上方"两级命中"）
//!   - **`Outside` → 3 tick（150ms）**：退场防抖
//! - 采纳的状态切换时：
//!   1. 永远 `app.emit("musage://floating-hover", inside)` 给前端
//!      （驱动 CSS `body[data-hover]` 玻璃效果）
//!   2. 当 `LEVEL_SWITCHING_ACTIVE` 为 true（PinBottom 模式）：
//!      - 采纳 `true` → raise 到 `HWND_TOPMOST`（edge-trigger，抬一次）
//!      - 采纳 `false` → drop 到 `HWND_BOTTOM`
//! - 稳定 hover 期间每 20 tick（1s）低频 re-assert 一次 TopMost 兜底，
//!   不再每 tick 抬（WS_EX_TOPMOST 本身是 sticky 的）
//!
//! ## 为什么需要 exit hysteresis（2026-07-20 根因定位）
//!
//! 旧实现**没有**防抖：单 tick 的 spurious `inside=false` 立刻触发
//! edge-drop 把窗口塞回 `HWND_BOTTOM`。而 `WindowFromPoint` 在 Win 上
//! 比 macOS `windowNumberAtPoint` 更容易单 tick 抖动 —— DWM 重绘、
//! WebView2 瞬态子窗口、光标压在 rect 边界 1px、光标在"被遮挡/未遮挡"
//! 细条之间摆动，都会让 hit test 偶发返一拍 false。结果是 raise 后
//! 50~100ms 内就被自己的 edge-drop 撤销，肉眼看到"hover 不变置顶"
//! （也是历史上 best-effort 3/7 命中率的真正根因，不是 OS demote）。
//! exit 阈值 3 tick 把这类抖动全部吞掉；离开方向的 100ms 额外延迟
//! 人眼不可感知。

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Runtime};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::{HWND as WIN_HWND, POINT, RECT};
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetWindowLongW, GetWindowRect, SetWindowLongW, SetWindowPos,
    WindowFromPoint, GA_ROOT, GWL_EXSTYLE, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_TOPMOST,
};

/// H2 fix (2026-07-06 全量审查): 启动期一次性把进程 DPI awareness 声明成
/// Per-Monitor V2。之后 `GetCursorPos` / `GetWindowRect` 在多屏不同 DPI
/// 缩放下都返回同一虚拟坐标系,hover 检测不会再因跨 DPI 屏而"鼠标永久在窗
/// 外"。失败 (老 OS / manifest 冲突) 时静默 —— 非致命,行为退回到系统默
/// 认 DPI awareness。
fn ensure_per_monitor_v2_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Hover tracker thread 是否已启动（idempotent 防重入）。
static TRACKER_RUNNING: AtomicBool = AtomicBool::new(false);

/// 鼠标 hover 时是否同步切 z-order：仅 PinBottom 模式置 true。
/// 这个开关只影响 z-order 切换；hover 事件 emit 不受影响（**永远 emit**），
/// 因为前端 iOS 26 玻璃 hover 效果需要它，不分 pin mode。
static LEVEL_SWITCHING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// 浮窗的 z-order 模式。直接走 `SetWindowPos` 的 3 个目标值之一。
#[derive(Debug, Clone, Copy)]
enum ZOrder {
    /// `HWND_TOPMOST` —— 高于所有其它窗口。PinTop 模式 + PinBottom
    /// hover 进窗口时用。
    TopMost,
    /// `HWND_BOTTOM` —— 低于所有 normal 窗口。PinBottom 模式
    /// + PinBottom hover 出窗口时用。
    ///
    /// **为什么不直接用 `set_always_on_top(false)`（即 HWND_NOTOPMOST）**：
    /// 后者只把 HWND 的 WS_EX_TOPMOST 标志位清掉，**不动 z-order**。
    /// 浮窗之前在 topmost 位置，清掉 topmost 标志后会落回 "top of
    /// normal z-order"，**视觉上还是盖在其它 app 之上**。HWND_BOTTOM
    /// 是显式"塞到正常 z-order 最底"，跟 macOS `LEVEL_BELOW_NORMAL`
    /// 行为对得起来。
    Bottom,
    /// `HWND_NOTOPMOST` —— 清 topmost 标志、保留 z-order。Normal
    /// 模式用：用户没要"始终 topmost"也别强塞 HWND_BOTTOM（那会
    /// 让窗口被其它所有 app 盖住）。
    NotTopMost,
}

/// 把浮窗的 z-order 设到指定模式。**双路并发 re-assert**：
/// - **路 A**：`SetWindowPos(HWND_TOPMOST, ...)` —— 标准 z-order 操纵
/// - **路 B**：`SetWindowLongW(GWL_EXSTYLE, ex | WS_EX_TOPMOST)` + 紧跟
///   `SetWindowPos` flush cache —— 直接改 style bit
///
/// WebView2 / OS 走哪条路径 demote 我们的窗口未知，两路并发至少能保证
/// 一路赢。`SetWindowLongW` **必须 OR 不能替换** —— 直接 `0x0008` 会
/// wipe 掉 `WS_EX_LAYERED` / `WS_EX_NOREDIRECTIONBITMAP` 等所有 bit，
/// 触发 Tauri 窗口恢复代码 re-assert 那些 bit 时隐式清掉 `WS_EX_TOPMOST`。
///
/// `SetWindowPos` + `SetWindowLongW` 都是 Win32 kernel call，文档
/// 明确 thread-safe，可从任意线程调。
unsafe fn apply_z_order(hwnd: *mut core::ffi::c_void, z: ZOrder) {
    let insert_after = match z {
        ZOrder::TopMost => HWND_TOPMOST,
        ZOrder::Bottom => HWND_BOTTOM,
        ZOrder::NotTopMost => HWND_NOTOPMOST,
    };

    // 路 B：直接改 style bit（OR 不能 wipe，AND 清除时不能保留其它 bit）
    //
    // TopMost: OR WS_EX_TOPMOST
    // Bottom: AND 清除 WS_EX_TOPMOST(显式清掉,避免 SetWindowPos(HWND_BOTTOM)
    //         之后 bit 残留——HWND_BOTTOM 不会自动清 bit)
    // NotTopMost: 不动 bit(SetWindowPos(HWND_NOTOPMOST) 按 Win32 文档
    //         自身会清 bit,不需要这里重复做)
    match z {
        ZOrder::TopMost => {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            let new_style: i32 = ex_style | (WS_EX_TOPMOST as i32);
            SetWindowLongW(hwnd, GWL_EXSTYLE, new_style);
        }
        ZOrder::Bottom | ZOrder::NotTopMost => {
            // M4 fix: NotTopMost 之前不清 style bit (注释说 "SetWindowPos 按 Win32 文档
            // 自身会清 bit")。但 WebView2 会在自己的 message handler 里 re-assert
            // WS_EX_TOPMOST，导致 Normal 模式在 Win10/11 上不可靠。
            // 改为 Bottom 和 NotTopMost 都显式 AND-out WS_EX_TOPMOST。
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
            let new_style: i32 = ex_style & !(WS_EX_TOPMOST as i32);
            SetWindowLongW(hwnd, GWL_EXSTYLE, new_style);
        }
    }

    // 路 A：z-order API + flush 路 B 的 cache
    SetWindowPos(
        hwnd,
        insert_after,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
}

// ── 公开 API ──

/// PinBottom 模式启动时调：把窗口塞到 `HWND_BOTTOM`，并开启 hover 切 z-order。
/// tracker 已由 `start_hover_emitter` 在 app 启动时拉起，这里只翻开关。
///
/// **L10 fix（2026-06-19）**：先把 `LEVEL_SWITCHING_ACTIVE` 置 true 再 dispatch
/// 闭包。原顺序（先 dispatch 再 store）在极罕见时序下，hover emitter thread 20Hz
/// 轮询可能读到"还在切"的中间态；新顺序保证 observer 看到的 store 永远先于或
/// 与 z-order 切换同时生效。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(true, Ordering::SeqCst);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(hwnd) = win.hwnd() {
                unsafe { apply_z_order(hwnd.0, ZOrder::Bottom) };
            }
        }
    });
    start_hover_emitter(app.clone());
}

/// PinTop 模式：z-order 切到 `TopMost`，关闭 hover 切换（窗口已经始终置顶）。
pub fn set_window_pin_top<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(hwnd) = win.hwnd() {
                unsafe { apply_z_order(hwnd.0, ZOrder::TopMost) };
            }
        }
    });
}

/// Normal 模式：z-order 切到 `NotTopMost`（清 topmost 标志、保留 z-order），
/// 关闭 hover 切换。
pub fn set_window_normal<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(hwnd) = win.hwnd() {
                unsafe { apply_z_order(hwnd.0, ZOrder::NotTopMost) };
            }
        }
    });
}

/// hover 切 z-order 的"前端兜底信号"：Win 上 tracker 已自行处理，此处 no-op。
/// 保留是为了让 commands.rs 在跨平台调用时不必 `#[cfg]`。
pub fn set_window_hover_raise<R: Runtime>(_app: &AppHandle<R>, _hovering: bool) {
    // no-op —— tracker 自己处理
}

// ── Fullscreen watcher：Win 暂未实现 ──
// Win 全屏检测需要多信号源（focus + 几何 + DWM），未实现。设置项保留可见
// 但 Win/Linux 开了无效，help 文字告诉用户「目前仅 macOS 生效」。
pub fn start_fullscreen_watcher<R: Runtime>(_app: tauri::AppHandle<R>) {}
pub fn set_auto_hide_in_fullscreen<R: Runtime>(_app: &tauri::AppHandle<R>, _enabled: bool) {}

/// 启动 hover emitter 线程。idempotent —— 第二次调用立即返回。
/// 由 lib.rs setup() 调一次即可。启动后整个 app 生命周期不停。
///
/// **2026-06-20 audit**：之前 spawn().expect()，线程数耗尽时整 app 启动 panic。
/// 降级 log + 翻转 TRACKER_RUNNING 让下次启动能重试。
pub fn start_hover_emitter<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    // H2 fix: 启动期一次性声明 Per-Monitor V2 DPI awareness。
    ensure_per_monitor_v2_dpi();
    let builder = thread::Builder::new()
        .name("musage-hover-emitter".into())
        .spawn(move || {
            tracing::debug!("hover emitter 启动");
            let mut last_inside = false;
            // pending_ticks / pending_value：dwell-time hysteresis 计数
            // （结构同 macos.rs，但观察值是三态 HitTest —— 计数仍按
            // "inside-ish"（Visible/Covered）vs Outside 的二值方向累计，
            // 阈值按当前观察到的具体态分档）。
            let mut pending_ticks: u8 = 0;
            let mut pending_value = false;
            // raised：当前 TopMost 是我们抬的（用于稳定 hover 期间的低频
            // safety re-assert）。edge-trigger 之后不再每 tick 抬。
            let mut raised = false;
            let mut steady_ticks: u8 = 0;
            loop {
                thread::sleep(Duration::from_millis(50));

                let Some(hit) = hit_test_floating(&app) else {
                    continue;
                };
                // Covered 算 inside 候选（dwell 够了就采纳）；一旦已采纳
                // （窗口已抬起成 topmost），Covered 只可能是"被另一个
                // topmost 窗口压住"，不算离开 —— 不会因此误 drop。
                let inside = hit != HitTest::Outside;

                if inside == last_inside {
                    pending_ticks = 0;
                    // 稳定 hover 中：每 20 tick（1s）safety re-assert TopMost。
                    // WS_EX_TOPMOST 是 sticky 的，这只是兜底"万一被顶掉"；
                    // 20Hz 反复 SetWindowPos 是无效 churn（见模块 doc）。
                    if inside && raised && LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                        steady_ticks = steady_ticks.saturating_add(1);
                        if steady_ticks >= 20 {
                            steady_ticks = 0;
                            if let Some(win) = app.get_webview_window("floating") {
                                if let Ok(hwnd) = win.hwnd() {
                                    unsafe { apply_z_order(hwnd.0, ZOrder::TopMost) };
                                }
                            }
                        }
                    }
                    continue;
                }

                // 观察值与 last_inside 不同 —— 真切换还是抖动？进入累计。
                if pending_value != inside {
                    pending_value = inside;
                    pending_ticks = 1;
                } else {
                    pending_ticks = pending_ticks.saturating_add(1);
                }

                // 阈值按当前观察态分档：
                // - Visible 1 tick（≤50ms，v0.1.0 一档响应）
                // - Covered 5 tick（250ms dwell）：浮窗被盖住时，鼠标停在
                //   浮窗所在屏幕区域 250ms 才算 intentional hover —— 路过
                //   （通常 <100ms）不误抬。这是 v0.2.4 用户场景修复：浮窗
                //   长期被最大化窗口盖住大半、只露一条边，严格"未被遮挡"
                //   语义下 hover-raise 永远不会触发（见模块 doc"两级命中"）。
                // - Outside 3 tick（150ms）：退场防抖。WindowFromPoint 单
                //   tick 抖动（DWM 重绘 / WebView2 瞬态子窗口 / 光标压边界）
                //   不会再让 raise 被自己瞬间撤销（模块 doc 有根因分析）。
                const ENTER_VISIBLE: u8 = 1;
                const ENTER_COVERED: u8 = 5;
                const EXIT_THRESHOLD: u8 = 3;
                let threshold = match hit {
                    HitTest::Visible => ENTER_VISIBLE,
                    HitTest::Covered => ENTER_COVERED,
                    HitTest::Outside => EXIT_THRESHOLD,
                };
                if pending_ticks < threshold {
                    continue;
                }

                // 阈值达成 —— 采纳新状态，emit + 切 z-order
                last_inside = inside;
                pending_ticks = 0;
                steady_ticks = 0;

                // (1) 永远 emit hover 事件（驱动 CSS 玻璃效果）。
                //     采纳后才 emit，前端 spring 不会被抖动反复重置。
                if let Err(e) = app.emit("musage://floating-hover", inside) {
                    tracing::trace!(error = %e, "emit hover 失败");
                }

                // (2) PinBottom 模式：edge-trigger 切 z-order
                if LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                    let z = if inside {
                        ZOrder::TopMost
                    } else {
                        ZOrder::Bottom
                    };
                    tracing::debug!(inside, ?hit, "hover 状态采纳，切换浮窗 z-order");
                    if let Some(win) = app.get_webview_window("floating") {
                        if let Ok(hwnd) = win.hwnd() {
                            unsafe { apply_z_order(hwnd.0, z) };
                        }
                    }
                    raised = inside;
                }
            }
        });
    if let Err(e) = builder {
        tracing::error!(error = %e, "spawn hover emitter thread 失败，hover raise / glass 效果将失效");
        TRACKER_RUNNING.store(false, Ordering::SeqCst);
    }
}

// ── 内部 ──

/// Hit test 三态结果：鼠标相对浮窗的位置。
///
/// 设计见模块 doc"两级命中"：`Visible` 快路径即抬，`Covered` 慢路径
/// dwell 抬，`Outside` 不抬。计数时 `Covered` 算 inside 候选
/// （`hit != HitTest::Outside`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitTest {
    /// 鼠标不在浮窗 rect 内。
    Outside,
    /// 在 rect 内，但该点 topmost 窗口爬根后**不是**浮窗 —— 浮窗被其它
    /// app（或另一个 topmost 窗口）盖住。
    Covered,
    /// 在 rect 内，且该点 topmost 窗口爬根后**是**浮窗（或其子窗口
    /// WebView2）—— 未被遮挡。
    Visible,
}

/// Hit test：返回鼠标相对浮窗的三态位置。
///
/// 判定两步：
/// 1. 鼠标在浮窗 rect 内（`GetWindowRect` → `point_in_rect`），否则 `Outside`
/// 2. 鼠标该点 topmost window 沿 parent 链爬到顶层根（`GetAncestor(_, GA_ROOT)`）：
///    等于浮窗自己 → `Visible`；不等于 → `Covered`
///
/// `WindowFromPoint` 在浮窗可见区域会返回 WebView2（浮窗的子窗口）的
/// hwnd，`GetAncestor` 爬到顶层根 = 浮窗 = `Visible`。被其它 app 覆盖
/// 的区域 `WindowFromPoint` 返回那个 app 的 hwnd，爬根不 match → `Covered`。
///
/// 返回 `None` 表示本轮无法判定（窗口未上屏 / Win API 失败），caller
/// continue 即可。
///
/// **L5 fix（2026-07-02 audit）**: GetCursorPos / GetWindowRect 返回 0 时
/// 之前直接返 None —— 但 0 也可能是合法坐标(理论:多屏桌面原点位移,
/// (0,0) 可能是合法点)。改为:Win API 失败时调 GetLastError 查具体原因,
/// 把 ERROR_ACCESS_DENIED (5) 等"真有错" 和"恰好 (0,0)" 区分开。
/// 实际生产中 (0,0) 几乎不可能(任务栏/开始菜单抢占),但严格说应区分。
fn hit_test_floating<R: Runtime>(app: &AppHandle<R>) -> Option<HitTest> {
    let win = app.get_webview_window("floating")?;
    let hwnd_t = win.hwnd().ok()?;
    if hwnd_t.0.is_null() {
        return None;
    }
    let our_hwnd: *mut core::ffi::c_void = hwnd_t.0;

    // SAFETY: GetCursorPos / GetWindowRect / WindowFromPoint / GetAncestor /
    // GetLastError 都是 Win32 kernel call,文档明确 thread-safe,可在任意线程调。
    // POINT/RECT 是值类型,零初始化即合法。
    unsafe {
        let mut pt: POINT = std::mem::zeroed();
        if GetCursorPos(&mut pt) == 0 {
            let err = GetLastError();
            tracing::trace!(
                error = err,
                "hit_test_floating: GetCursorPos 失败,跳过本 tick"
            );
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(our_hwnd, &mut rect) == 0 {
            let err = GetLastError();
            tracing::trace!(
                error = err,
                "hit_test_floating: GetWindowRect 失败,跳过本 tick"
            );
            return None;
        }
        // PointInRect 不用 GetLastError —— 它本身就是正确区分命中/不命中,
        // 跟 (0,0) 边界 case 无关。
        if !point_in_rect(pt, &rect) {
            return Some(HitTest::Outside);
        }
        // WindowFromPoint 成功 → topmost non-null = 真实命中窗口。
        // 失败 → null = 未知状态(UAC 同意框 / 锁屏 / 不同 desktop)，
        // M17 fix (2026-07-06 全量审查): 之前 `return Some(false)` 会
        // 经 IPC 发给前端 → CSS glass hover 状态闪烁。改为返 `None` 让
        // 调用方完全跳过这一 tick,无 emit、无 z-order 切换,前端 CSS
        // 状态保持不变,下一 tick 自然恢复。
        let topmost: WIN_HWND = WindowFromPoint(pt);
        if topmost.is_null() {
            return None;
        }
        let root = GetAncestor(topmost, GA_ROOT);
        if root.is_null() {
            // 兜底:取不到根就退到裸比
            return Some(if topmost == our_hwnd {
                HitTest::Visible
            } else {
                HitTest::Covered
            });
        }
        Some(if root == our_hwnd {
            HitTest::Visible
        } else {
            HitTest::Covered
        })
    }
}

#[inline]
fn point_in_rect(pt: POINT, rect: &RECT) -> bool {
    pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
}
