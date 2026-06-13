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

use tauri::{AppHandle, Emitter, Manager, Runtime, WindowEvent};
use windows_sys::Win32::Foundation::{HWND as WIN_HWND, POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetWindowLongW, GetWindowRect, SetWindowLongW, SetWindowPos,
    WindowFromPoint, GA_ROOT, GWL_EXSTYLE, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_EX_NOACTIVATE, WS_EX_TOPMOST,
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

/// 把浮窗的 z-order 设到指定模式。直接走 `SetWindowPos` + 直接设
/// `WS_EX_TOPMOST` style bit，绕开 Tauri/tao 的 `set_always_on_top`
/// 抽象层 —— 后者只能 toggle topmost 标志位，不能塞到 HWND_BOTTOM。
///
/// **两路并发 re-assert**：
/// - 路 A：`SetWindowPos(HWND_TOPMOST, ...)` —— 标准 z-order 操纵，
///   内部会设 `WS_EX_TOPMOST` style bit 但走 SetWindowPos 的 cache
///   flush 路径
/// - 路 B：`SetWindowLongW(GWL_EXSTYLE, ex_style | WS_EX_TOPMOST)` + 紧跟
///   `SetWindowPos` 强制 flush cache —— 直接改 style bit，**绕开
///   SetWindowPos 的内部优化**
///
/// WebView2（怀疑的 demote 源）走的是修改 extended style 的路径，
/// `SetWindowPos` 走的是 z-order API 路径，两者可能独立。如果
/// WebView2 清 bit 但 SetWindowPos 的 cache 没及时刷新，单纯调
/// `SetWindowPos` 拿不回 bit —— `SetWindowLongPtr` 直接改 style 是
/// 最暴力的兜底。两条路并发，至少一条能赢。
///
/// **thread-safety**：`SetWindowPos` + `SetWindowLongW` 都是 Win32
/// kernel call，文档明确 thread-safe。
///
/// **flags**：
/// - `SWP_NOMOVE` / `SWP_NOSIZE` —— 不动 rect，只换 z-order
/// - `SWP_NOACTIVATE` —— 不抢焦点
/// - **不**带 `SWP_ASYNCWINDOWPOS` —— 同步处理拿确定时序
unsafe fn apply_z_order(hwnd: *mut core::ffi::c_void, z: ZOrder) {
    let insert_after = match z {
        ZOrder::TopMost => HWND_TOPMOST,
        ZOrder::Bottom => HWND_BOTTOM,
        ZOrder::NotTopMost => HWND_NOTOPMOST,
    };

    // 路 B：先直接设 style bit（盖住"有人清了 style 但 SetWindowPos
    // 还没察觉"的情况）
    if matches!(z, ZOrder::TopMost) {
        // **必须 OR 不能替换** —— `WS_EX_TOPMOST = 0x0008` 只是 exstyle
        // 32 位里的一个 bit。窗口正常 exstyle 还含 `WS_EX_LAYERED`（半透明）、
        // `WS_EX_NOREDIRECTIONBITMAP`、WebView2 内部加的 bit 等。直接
        // `SetWindowLongW(GWL_EXSTYLE, 0x0008)` 会**全 wipe 掉其它 bit**，
        // 触发 Tauri 窗口恢复代码 re-assert 那些 bit 时隐式清掉
        // `WS_EX_TOPMOST` —— 反而让 dual-path 自己的路 B 起反效果。
        //
        // 正确做法：先 GetWindowLongW 拿当前 exstyle，OR 上 `WS_EX_TOPMOST`，
        // SetWindowLongW 写回 —— 保留所有 bit，只 flip 一个。
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let new_style: i32 = ex_style | (WS_EX_TOPMOST as i32);
        let prev = SetWindowLongW(hwnd, GWL_EXSTYLE, new_style);
        if prev == 0 {
            eprintln!(
                "[musage-zorder-warn] SetWindowLongW(ex|WS_EX_TOPMOST) returned 0 (hwnd={:?})",
                hwnd
            );
        }
    }

    // 路 A：SetWindowPos 走标准 z-order 操纵 + flush 路 B 的 cache
    let ret = SetWindowPos(
        hwnd,
        insert_after,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    );
    if ret == 0 {
        eprintln!(
            "[musage-zorder-warn] SetWindowPos({:?}) returned 0 (hwnd={:?})",
            z, hwnd
        );
    }
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

/// 启动 hover emitter 线程 + 焦点事件 hook。idempotent —— 第二次调用立即返回。
/// 由 lib.rs setup() 调一次即可。启动后整个 app 生命周期不停。
///
/// **两路并进切 z-order**：
/// - **路 1**（`start_hover_emitter` 主线程）：16ms tick，每 tick 重新
///   `SetWindowPos(TOPMOST)`，盖住 OS 持续 demote。
/// - **路 2**（焦点事件 hook，主线程）：`WindowEvent::Focused(false)`
///   一帧就 re-assert，赶在 OS demote 把状态沉淀前抢回来。
///
/// 仅路 1 不够（16ms 已经验证 H1 证伪），加路 2 是 H3 修法。
pub fn start_hover_emitter<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }

    // ── 路 2：焦点事件 hook（H3 修法） ──
    //
    // Codex 报告 H3：WebView2 / OS 在 focus loss 时把 `WS_EX_TOPMOST`
    // 标志位清掉。SetWindowPos 调用本身被 OS 接受（ret != 0），但
    // style bit 在几 ms 内被覆盖回去 —— 这跟 H1（OS demote 50ms 内）
    // 是不一样的 race。
    //
    // 16ms tick **盖不住**这个 race：focus event 触发后 OS 同步清
    // bit，下一个 16ms tick 才 re-assert → 16ms 期间窗口是 BOTTOM。
    // 修：listen 焦点变化事件，**同一帧**就 re-assert。这条路径
    // 走主线程（window event 派发在主线程），可以保证 SetWindowPos
    // 在 focus 事件处理栈内同步完成。
    if let Some(win) = app.get_webview_window("floating") {
        let app2 = app.clone();
        win.on_window_event(move |event| {
            if let WindowEvent::Focused(false) = event {
                if !LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                    return; // PinTop / Normal 模式不需要 hover re-assert
                }
                if let Some(win) = app2.get_webview_window("floating") {
                    if let Ok(hwnd) = win.hwnd() {
                        // 看当前 cursor 位置：in rect → TopMost，out → Bottom
                        let cursor_in_rect = unsafe {
                            let mut pt: POINT = std::mem::zeroed();
                            let mut rect: RECT = std::mem::zeroed();
                            if GetCursorPos(&mut pt) != 0
                                && GetWindowRect(hwnd.0, &mut rect) != 0
                            {
                                point_in_rect(pt, &rect)
                            } else {
                                false
                            }
                        };
                        let z = if cursor_in_rect {
                            ZOrder::TopMost
                        } else {
                            ZOrder::Bottom
                        };
                        tracing::trace!(?z, "focus loss → re-assert z-order");
                        unsafe { apply_z_order(hwnd.0, z) };
                    }
                }
            }
        });
    }

    // ── 路 1：hover tracker 线程（H1 + 安全网） ──
    std::thread::Builder::new()
        .name("musage-hover-emitter".into())
        .spawn(move || {
            tracing::debug!("hover emitter 启动");
            // **H1 验证 tick 频率**：之前 50ms（20Hz）现场数据 14 个
            // inside=true 里有 10 个 topmost=false。Codex 报告假设
            // 50ms tick 太稀疏，OS 在 re-assert 间隔里"塌"一次。
            // 缩到 16ms（≈ 1 帧 @ 60Hz）做对比实验 —— 抖动消失 → H1
            // 命中，把这行 freeze 当成 fix 留着；抖动还在 → H2 / H3，
            // 下一步看 focus 事件。
            const TICK: Duration = Duration::from_millis(16);

            // tracker 心跳（H4 排除）：每 50s 一次，证明线程没 panic 死。
            // 如果某天 stderr 看到 N 秒后没新心跳，说明 spawn 之后的
            // closure panic 掉了 tracker —— 这种 silent death 之前完全
            // 没有任何信号。
            let mut tick_count: u64 = 0;
            let mut last_inside = false;
            loop {
                std::thread::sleep(TICK);
                tick_count += 1;

                if tick_count % 3125 == 0 {
                    // ≈ 16ms × 3125 = 50s
                    eprintln!(
                        "[musage-tracker-heartbeat] alive, ticks={}, last_inside={}",
                        tick_count, last_inside
                    );
                }

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
                    if inside {
                        // **关键：inside 时每个 poll cycle 都断言 TopMost**。
                        // 之前用 last_inside != inside 做 edge trigger，
                        // 行为是「只有鼠标进出窗口那一帧切 z-order」——
                        // Win 切完后，OS 调度（用户点其它 app → 其它窗口
                        // HWND_TOP）会把浮窗 z-order 重新打到 BOTTOM 一带，
                        // 我们的 tracker 不会 re-assert，浮窗就一直沉底。
                        // 改成 level trigger：每 50ms 断言一次。
                        //
                        // **直接调 SetWindowPos，绕过 run_on_main_thread**：
                        // 之前 fire-and-forget dispatch 在 50ms tick × 持续 inside
                        // 几秒 = 几十上百个 closure 排队，主线程还要处理 WebView2
                        // IPC / focus 事件 / 自己的 paint，dispatch 队列被挤，
                        // SetWindowPos 实际生效的概率下降 —— 现场 diag 抓到
                        // 10 个 inside=true 里有 6 个 topmost=false 案例就是
                        // 证据。SetWindowPos 本身是 Win32 kernel call，文档
                        // 明确 thread-safe，可以从任意线程调。
                        if let Some(win) = app.get_webview_window("floating") {
                            if let Ok(hwnd) = win.hwnd() {
                                unsafe { apply_z_order(hwnd.0, ZOrder::TopMost) };
                            }
                        }
                    } else if last_inside {
                        // 鼠标刚离开：edge-trigger 切到 BOTTOM（真正的"置底"）。
                        // 离开后不再断言，让 OS 正常 z-order 接管。
                        // 同样直接调 SetWindowPos（不通过 main thread dispatch）。
                        tracing::trace!("PinBottom: 鼠标离开浮窗 → z-order Bottom");
                        if let Some(win) = app.get_webview_window("floating") {
                            if let Ok(hwnd) = win.hwnd() {
                                unsafe { apply_z_order(hwnd.0, ZOrder::Bottom) };
                            }
                        }
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
///
/// **诊断 hook**：`MUSAGE_HOVER_DIAG=1` 环境变量打开时，每秒一次
/// eprintln 鼠标位置 + 浮窗 rect + hit test 结果，方便现场抓"浮窗
/// focus 丢失后为什么 inside 还是 false"。生产环境零开销（一个 env
/// check + 1 个 atomic load）。
fn is_cursor_inside_floating<R: Runtime>(app: &AppHandle<R>) -> Option<bool> {
    let win = app.get_webview_window("floating")?;
    let hwnd_t = win.hwnd().ok()?;
    if hwnd_t.0.is_null() {
        return None;
    }
    let our_hwnd: *mut core::ffi::c_void = hwnd_t.0;

    // SAFETY: GetCursorPos / GetWindowRect / WindowFromPoint / GetAncestor
    // 都是 Win32 kernel call，文档明确 thread-safe，可从任意线程调。
    // POINT/RECT 是值类型，零初始化即合法。
    unsafe {
        let mut pt: POINT = std::mem::zeroed();
        if GetCursorPos(&mut pt) == 0 {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(our_hwnd, &mut rect) == 0 {
            return None;
        }
        if !point_in_rect(pt, &rect) {
            diag_dump(our_hwnd, pt, rect, false);
            return Some(false);
        }
        // macOS-parity "未被遮挡" 判定：rect 内 + 该点 topmost window
        // 沿 parent 链爬到顶层根后等于浮窗自己 → 算 "inside"。
        //
        // **为什么现在能加回来（之前 commit e9e7f87 拿掉过这条）**：
        // 之前"focus 切走后浮窗一直置底"bug 的真凶是 WebView2 / OS 持续
        // 清 `WS_EX_TOPMOST` style bit（commit 79dbdbc 双路 fix 修好了）。
        // 当时拿掉 `WindowFromPoint` 检查是因为"浮窗根本没 topmost"导致
        // `WindowFromPoint` 返回别的窗口的 hwnd，check 永远 false。**根
        // 因是 `WS_EX_TOPMOST` 被清**，不是 `WindowFromPoint` 本身错。
        //
        // 双路 fix 之后浮窗持续 topmost，`WindowFromPoint` 在浮窗可见
        // 区域正确返回 WebView2（浮窗的子窗口），`GetAncestor(_, GA_ROOT)`
        // 爬到浮窗根 = match。被其它 app 完全覆盖的区域 `WindowFromPoint`
        // 返回那个 app 的 hwnd（不是我们浮窗）→ `GetAncestor` 爬根是别
        // app → 不 match → false → 不 raise，浮窗保持被覆盖的常态。
        let topmost: WIN_HWND = WindowFromPoint(pt);
        if topmost.is_null() {
            diag_dump(our_hwnd, pt, rect, false);
            return Some(false);
        }
        let root = GetAncestor(topmost, GA_ROOT);
        let inside = if root.is_null() {
            // 兜底：取不到根就退到裸比（虽然不太可能）
            topmost == our_hwnd
        } else {
            root == our_hwnd
        };
        diag_dump(our_hwnd, pt, rect, inside);
        Some(inside)
    }
}

static DIAG_LAST_DUMP: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// `MUSAGE_HOVER_DIAG=1` 开启后每秒 eprintln 一次 hit test + z-order 现场。
/// 设计成单次调用不阻塞（env check + atomic load），生产路径零开销。
///
/// z-order 字段（`topmost=`）：
/// - `true`  = `GetWindowLong(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST` 非零，
///             浮窗当前是 OS topmost
/// - `false` = 同上为零，浮窗没 topmost（处于普通 z-order，可能是
///             刚 set_always_on_top(false) 或 BOTTOM）
/// - `?`     = `GetWindowLong` 失败（窗口已销毁/无效 hwnd）
fn diag_dump(hwnd: *mut core::ffi::c_void, pt: POINT, rect: RECT, inside: bool) {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GWL_EXSTYLE};

    if std::env::var_os("MUSAGE_HOVER_DIAG").is_none() {
        return;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let last = DIAG_LAST_DUMP.load(Ordering::Relaxed);
    if now_ms - last < 1000 {
        return;
    }
    DIAG_LAST_DUMP.store(now_ms, Ordering::Relaxed);

    // WS_EX_TOPMOST = 0x0008，是 Win 标记 topmost 窗口的 extended-style
    // bit。GetWindowLongW(GWL_EXSTYLE) 返回扩展样式位，0x0008 bit 置位即
    // 当前 topmost。
    let topmost: String = unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if ex_style == 0 {
            "?".to_string()
        } else {
            (ex_style & 0x0008 != 0).to_string()
        }
    };

    eprintln!(
        "[musage-hover-diag] cursor=({}, {}) rect=[{}, {}, {}, {}] inside={} topmost={}",
        pt.x, pt.y,
        rect.left, rect.top, rect.right, rect.bottom,
        inside,
        topmost,
    );
}

#[inline]
fn point_in_rect(pt: POINT, rect: &RECT) -> bool {
    pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
}
