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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSMenu, NSWindow};
use objc2_core_graphics::{kCGFloatingWindowLevel, kCGNormalWindowLevel, CGWindowLevel};
use objc2_foundation::NSPoint;
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

/// fix (2026-07-28 审查): 请求 hover emitter 复位内部状态（last_inside / 防抖计数器）。
/// `set_window_pin_bottom` 切模式时置位，emitter 每 tick 开头消费（swap(false)）。
/// 根因：PinTop→PinBottom 切换时若鼠标已在浮窗上方，emitter 的 `last_inside`
/// 已是 true，`inside == last_inside` 命中 continue → hover-raise 的采纳 edge
/// 永不触发，浮窗永久卡底部。复位后同 tick 重新走 enter 评估（阈值 1 tick）。
static HOVER_STATE_RESET: AtomicBool = AtomicBool::new(false);
/// D6-04: main thread dispatch 连续失败计数（hover emitter 20Hz）。
/// 跨过 MAIN_DISPATCH_WARN_EVERY 的整数倍时 warn 一次，成功复位。
static MAIN_DISPATCH_CONSECUTIVE_FAILS: AtomicU64 = AtomicU64::new(0);
/// 1000 tick ≈ 20Hz × 50s —— 持续失败超 50s 才升级为 warn。
const MAIN_DISPATCH_WARN_EVERY: u64 = 1000;

// ── 公开 API ──

/// PinBottom 模式启动时调：把 level 切到 below-normal，并开启 hover 切 level。
/// tracker 已由 [`start_hover_emitter`] 在 app 启动时拉起，这里只翻开关。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    // fix (2026-07-28 审查, T8): 对齐 Windows —— 先 store 开关再 dispatch 切 level，
    // 避免 emitter tick 在 dispatch 与 store 之间读到旧 mode。
    LEVEL_SWITCHING_ACTIVE.store(true, Ordering::SeqCst);
    set_window_level(app, LEVEL_BELOW_NORMAL, true); // M3 fix: true = is_pin_bottom
                                                     // fix (2026-07-28 审查, T1): 置位必须放在切 level 的 dispatch **之后** ——
                                                     // 保证 emitter 复位重评后 post 的 raise 在 main thread 队列里排在 demote 之后，
                                                     // 最终 level 由 raise 决定；即使极罕见时序下 raise 先落地被 demote 覆盖，
                                                     // 下一 tick（≤50ms）复位后的重评也会自愈。
    HOVER_STATE_RESET.store(true, Ordering::SeqCst);
    // 防御：如果 lib.rs setup 之外的路径走到这（理论上不会），保底拉起 tracker
    start_hover_emitter(app.clone());
}

/// PinTop 模式：level 切到 floating，关闭 hover 切 level（窗口已经始终置顶）。
/// hover 事件 emit 不变，前端的玻璃效果继续受惠。
pub fn set_window_pin_top<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    set_window_level(app, LEVEL_FLOATING, false); // M3 fix: false = 非 PinBottom
}

/// Normal 模式：level 切回 0，关闭 hover 切 level。
pub fn set_window_normal<R: Runtime>(app: &AppHandle<R>) {
    LEVEL_SWITCHING_ACTIVE.store(false, Ordering::SeqCst);
    set_window_level(app, kCGNormalWindowLevel, false); // M3 fix: false = 非 PinBottom
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
///
/// **2026-07-03 fix（v0.2.x 闪烁修复 — 后端层）**：挂后台时浮窗毛玻璃效果
/// 偶发闪一下。根因是 macOS 上多个 transparent + always-on-top 窗口
/// （例如同层另一个项目也开了 transparent 浮窗）共存时，光标静止在某像素
/// 边缘，`windowNumberAtPoint` 返回值在两个 window number 之间抖；
/// `is_floating_topmost_at` 20Hz tick 里 inside 持续翻转 → 每次翻转
/// emit 一次 → 前端每次都 toggle body[data-hover] → 0.28s spring 动画
/// 反复起头又被瞬间打断 → 肉眼看到持续 / 偶发闪一下。
///
/// 修复：在 hover emitter 加 dwell-time hysteresis：
/// - **enter**：inside=true 必须连续 ≥3 个 tick（150ms）才采纳，
///   抖动短脉冲被吞。
/// - **exit**：inside=false 必须连续 ≥2 个 tick（100ms）才采纳，略快
///   因为用户离开时希望玻璃及时撤销（vs enter 多 1 tick 防误触发）。
/// - **enter→exit 切换瞬间 reset 计数器**：避免在过渡中误累计。
///
/// 浮窗前端的 `body[data-hover]` CSS spring 动画不会被反复重置，
/// 闪烁消失。
///
/// **2026-07-09 fix（恢复 v0.1.0 响应速度）**：上面那套防抖把 hover 响应
/// 从 ~50ms（v0.1.0 一个 tick）拖到 ~190ms（enter 3 ticks + 前端 40ms
/// debounce），多数用户没遇到"同层多个 transparent 浮窗"的极端场景，但
/// 都付了延迟代价。退一步：
/// - **enter**：`ENTER_THRESHOLD = 1` —— 第一个 inside=true 就采纳，
///   响应回到 v0.1.0 一个 tick（≤50ms）。极端场景（光标卡在 transparent
///   边界 + 同层还有别家 transparent 浮窗）下偶发 enter 抖动回归，
///   但前端的 `lastHoverPayload` 同值去重 + visibility / focus guard
///   还能挡掉大部分重复切。
/// - **exit**：`EXIT_THRESHOLD = 2`（100ms）—— 保留。光标离开的延迟没人
///   在意，但能挡掉退场方向的偶发抖动；而且前端 `mouseleave` 路径会
///   即时 `setHoverAttr(false)` 接管，exit 阈值对观感几乎无影响。
///
/// 权衡：极少数同层 transparent 浮窗共存的用户可能重新看到偶发闪烁，
/// 换来 100% 用户的 hover 响应速度回到 v0.1.0 一档。
pub fn start_hover_emitter<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }
    let builder = thread::Builder::new()
        .name("musage-hover-emitter".into())
        .spawn(move || {
            tracing::debug!("hover emitter 启动");
            let mut last_inside = false;
            // pending_ticks：当前观察到的 inside 与 last_inside 不同的累计 tick 数。
            // 当达到对应阈值才采纳新状态 + emit。
            //   - ENTER_THRESHOLD = 1：第一个 inside=true 就采纳（v0.1.0 一档速度）
            //   - EXIT_THRESHOLD = 2 (100ms)：保留挡退场抖动；前端 mouseleave
            //     路径会即时 setHoverAttr(false) 接管，exit 阈值对观感无影响
            let mut pending_ticks: u8 = 0;
            // pending_value：累计观察到的"候选新值"。
            let mut pending_value = false;
            loop {
                thread::sleep(Duration::from_millis(50));

                // D5-102 fix (2026-07-30 audit): macOS hover emitter OS 线程
                // 同样永久循环, quit_app 同步 SHUTDOWN_NATIVE_THREADS atomic,
                // 此线程每 tick 检查后退出。 50ms 延迟用户无感知。
                if crate::poller::SHUTDOWN_NATIVE_THREADS.load(std::sync::atomic::Ordering::SeqCst)
                {
                    tracing::debug!("macOS hover emitter 收到 SHUTDOWN, 退出");
                    break;
                }

                // fix (2026-07-28 审查): 模式切换（set_window_pin_bottom）请求
                // 复位内部状态 —— 否则鼠标已在浮窗上方时 last_inside 已是 true，
                // `inside == last_inside` 永远命中 continue，hover-raise 不触发。
                if HOVER_STATE_RESET.swap(false, Ordering::SeqCst) {
                    last_inside = false;
                    pending_ticks = 0;
                    pending_value = false;
                }

                // mouseLocation 在 macOS 上是 thread-safe 的，可从任意线程调
                let mouse = NSEvent::mouseLocation();

                // 关键：用 NSWindow.windowNumberAtPoint 做命中测试 ——
                // 不光检查"鼠标在不在浮窗 frame 内"，还要确认浮窗在该点是**最上层**。
                // PinBottom 模式下浮窗经常被其它 app 部分遮挡，单纯 point-in-rect
                // 会在被遮挡区域也误触发置顶（用户其实在操作遮挡它的那个 app）。
                //
                // M-tray-10 fix (2026-09-28 audit)：`None` = 本 tick 无法判定
                // （主线程派发失败 / 50ms 超时 / MainThreadMarker 拿不到 / 只看到
                // 迟到的上一拍闭包），**必须 continue 不采纳**。之前函数把三条
                // 失败路径都塌缩成 `false`，emitter 攒够 EXIT_THRESHOLD=2 个
                // 连续 false 就把 inside 改成 false：鼠标明明还在窗上，玻璃效果
                // 被撤销、PinBottom 下窗口被 setWindowLevel(BELOW_NORMAL) 从光标
                // 底下抽走 —— 用户看到浮窗"闪一下降到底部再抬起"。跟 Windows 端
                // （`hit_test_floating → None → continue`）对齐。
                let Some(inside) = is_floating_topmost_at(&app, mouse) else {
                    continue;
                };

                if inside == last_inside {
                    // D6-002 fix (2026-07-30 audit): 同步 Win 同款, 稳定态 reset
                    // pending_value 防 Visible↔Outside 病态抖动击穿 EXIT_THRESHOLD=2.
                    pending_value = last_inside;
                    pending_ticks = 0;
                    continue;
                }

                // inside 与 last_inside 不同 —— 是真切换还是抖动？
                // 进入累计阶段。每次观察同值则递增；中途翻回 old 值则重置。
                if pending_value != inside {
                    pending_value = inside;
                    pending_ticks = 1;
                } else {
                    pending_ticks = pending_ticks.saturating_add(1);
                }

                // enter 阈值 (1) < exit 阈值 (2) —— 进入方向零延迟回到 v0.1.0，
                // 退场方向留 100ms 挡抖动。注释见上方 doc comment 的
                // "2026-07-09 fix（恢复 v0.1.0 响应速度）" 段。
                const ENTER_THRESHOLD: u8 = 1;
                const EXIT_THRESHOLD: u8 = 2;
                let threshold = if pending_value {
                    ENTER_THRESHOLD
                } else {
                    EXIT_THRESHOLD
                };

                if pending_ticks < threshold {
                    continue;
                }

                // 阈值达成 —— 采纳新状态，emit + 切 level
                last_inside = inside;
                pending_ticks = 0;
                tracing::trace!(inside, "hover 采纳新状态（dwell-time 达阈值）");

                // (1) 永远 emit —— 驱动前端 body[data-hover]，让 CSS hover 生效
                //     不依赖 WKWebView 的 mouseMoved 事件流（macOS 非 key window 不分发）
                if let Err(e) = app.emit("musage://floating-hover", inside) {
                    tracing::trace!(error = %e, "emit hover 失败");
                }

                // (2) PinBottom 模式：同步切 NSWindow level
                if LEVEL_SWITCHING_ACTIVE.load(Ordering::SeqCst) {
                    let level = if inside {
                        LEVEL_FLOATING
                    } else {
                        LEVEL_BELOW_NORMAL
                    };
                    tracing::trace!(?level, inside, "PinBottom hover 切 level");
                    set_window_level(&app, level, true); // M3 fix: true = PinBottom 模式内
                }
            }
        });
    // **2026-06-20 audit**：之前 .expect()，线程数耗尽 / ulimit 触底时整 app
    // 启动 panic。降级：log + 关闭 TRACKER_RUNNING 让下次重启能重试。
    if let Err(e) = builder {
        tracing::error!(error = %e, "spawn hover emitter thread 失败，hover raise / glass 效果将失效");
        TRACKER_RUNNING.store(false, Ordering::SeqCst);
    }
}

// ── 内部 ──

/// 把浮窗的 NSWindow level 切到 `level`,dispatch 到 main thread(AppKit 强制要求)。
///
/// **D6-004 fix (2026-07-30 audit)**: doc 跟 H15 fix (2026-07-03, commit d5612ab) 实际
/// 行为对齐 —— 所有模式都强制 `setHidesOnDeactivate(false)`,让浮窗失焦时仍可见
/// (macOS 普通窗口失焦只是被其他 app 遮盖, level 切换已实现该语义, hide() 不属于
/// "始终可见的用量悬浮窗"产品定义)。`is_pin_bottom` 参数保留仅为调用方签名兼容,
/// 不参与运行时分支(见函数体内 `let _ = is_pin_bottom;` 注释)。
pub fn set_window_level<R: Runtime>(app: &AppHandle<R>, level: CGWindowLevel, is_pin_bottom: bool) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(ptr) = win.ns_window() {
                if !ptr.is_null() {
                    // H2 fix (2026-07-30 audit): 用 Retained<NSWindow> 包一层。
                    // 之前裸 `&*ptr.cast::<NSWindow>()` 在 dispatch 任务里 deref
                    // 裸指针;若 webview_window 在 dispatch 排队 → 执行期间并发
                    // 销毁(应用退出 / Tauri 关 webview / 用户手动关浮窗),raw
                    // ptr 指向已释放内存 → segfault。Retained balance +1 retain
                    // count,closure 结束 Drop 时 -1,即使 Tauri 端已 Drop 也能
                    // 保证 NSWindow 在闭包执行期间存活。
                    let window: Retained<NSWindow> = unsafe {
                        match Retained::retain(ptr.cast::<NSWindow>()) {
                            Some(w) => w,
                            None => return,
                        }
                    };
                    window.setLevel(level as _);
                    let _ = is_pin_bottom; // 保留参数兼容现有调用,语义不再依赖
                    window.setHidesOnDeactivate(false);
                    // backdrop-refresh emit:在 setLevel 之后(同 main thread dispatch)
                    let _ = app2.emit("musage://backdrop-refresh", ());
                }
            }
        }
    });
}

/// 命中测试：鼠标在 `point` 处时，浮窗是否是**最上层**窗口。
///
/// 用 `+[NSWindow windowNumberAtPoint:belowWindowWithWindowNumber:]` 传 0
/// （穿透所有 app 检查整个屏幕），返回该点 topmost window 的 ID。
/// 与浮窗自己的 `windowNumber` 比对：
/// - 相等 → 鼠标 hover 在浮窗**可见**部分
/// - 不等 → 别的窗口盖在那里，用户在跟那个窗口交互，不该触发置顶/玻璃显形
///
/// 解决 PinBottom 模式下浮窗被部分遮挡时，鼠标移到被盖的区域也误触发的问题。
///
/// dispatch 到 main thread（NSWindow API 强制要求）。channel 同步等待。
///
/// **返回 `Option<bool>` —— `None` = "本 tick 无法判定"，`Some(v)` = 判定成功。**
///
/// M-tray-10 fix (2026-09-28 audit)：之前这个函数返 `bool`，把"派发失败 /
/// 超时 / MainThreadMarker 拿不到"三条**失败**路径全部塌缩成语义确定的
/// `false`。hover emitter 拿到连续 2 个 false（`EXIT_THRESHOLD = 2`，见
/// [`start_hover_emitter`]）就采纳 `inside = false` → emit
/// `musage://floating-hover=false`（**鼠标还在窗上**，玻璃效果却撤销了），
/// PinBottom 下还会 `set_window_level(LEVEL_BELOW_NORMAL)` 把窗口从光标底
/// 下抽走。触发条件很日常：主线程任何 >=100ms 的阻塞（NSOpenPanel、
/// WKWebView 首次初始化 / 重绘、NSAlert 嵌套循环）恰好落在用户悬停浮窗期
/// 间 → 浮窗**闪一下降到底部再抬起**。这正是 H7 fix 当初要消灭的"应用消
/// 失"症状换了个触发路径。
///
/// Windows 端同类失败一直走的是正确做法（`hit_test_floating → None →
/// continue`，`platform/windows.rs`）—— 两平台语义相反，本次对齐。
///
/// **L12 fix（2026-06-19）**：旧实现每调用一次就新建一对 `mpsc::channel::<bool>()`。
/// hover emitter 20Hz × 86,400s ≈ 1.7M 次/24h，allocator churn 严重。改用
/// 全局复用的 `Mutex` + `Condvar` 单槽位（外层包 `OnceLock<Arc<...>>` 复用）。
/// hover emitter 串行调用，单槽位足够。
fn is_floating_topmost_at<R: Runtime>(app: &AppHandle<R>, point: NSPoint) -> Option<bool> {
    use std::sync::{Arc, Condvar, Mutex};

    /// 单槽位回填状态。`ticket` 是 M-tray-10 fix 追加的**代际标记**（见下方
    /// "迟到闭包"注释）；`filled` 里的值本身是 `Option<bool>`，`None` 表示
    /// "闭包跑完了但没法判定"。
    struct SlotState {
        ticket: u64,
        filled: Option<(u64, Option<bool>)>,
    }
    struct OneSlot {
        slot: Mutex<SlotState>,
        cvar: Condvar,
    }
    static SLOT: OnceLock<Arc<OneSlot>> = OnceLock::new();
    let slot = SLOT.get_or_init(|| {
        Arc::new(OneSlot {
            slot: Mutex::new(SlotState {
                ticket: 0,
                filled: None,
            }),
            cvar: Condvar::new(),
        })
    });

    let app2 = app.clone();
    let slot2 = slot.clone();

    // M-tray-10 fix：**派发前先清槽 + 领一个 ticket**。
    //
    // "迟到闭包"问题：上一次调用超时返回后，它派发的闭包才落到主线程上执行
    // 并写槽。此时本次调用进来若直接等 `filled` 为空，会把**上一拍**的值当
    // 成自己的结果读走 —— hover emitter 的迟滞计数被陈旧值污染，且这个污
    // 染是系统性的（每次主线程拥堵都会发生）。
    //
    // 修法两件套：(1) 派发前 `filled = None`，杜绝读残留；(2) 闭包写回时带上
    // 自己那一拍的 ticket，等待侧只接受 ticket 匹配的写 —— 迟到的旧闭包即
    // 使落地也只会被忽略，本 tick 走超时返 `None`（emitter `continue`，不
    // 采纳）。串行调用 + ticket 让迟滞计数重新可信，且没有恢复 per-call
    // channel 的 allocator churn。
    let ticket = {
        let mut g = slot.slot.lock().unwrap_or_else(|e| e.into_inner());
        g.ticket = g.ticket.wrapping_add(1);
        g.filled = None;
        g.ticket
    };

    // **M2 fix（2026-07-02 audit）**：`run_on_main_thread` 返 Err 时必须记
    // warning（首次 fail 后降级为 trace 避免 log spam），否则 main thread
    // 长时间忙时 hover emitter 持续 20Hz 失败、用户看不到任何 log，浮窗玻
    // 璃效果永久失效且完全无声。
    let dispatch_result = app.run_on_main_thread(move || {
        let result: Option<bool> = (|| {
            // 浮窗不存在 / ns_window 拿不到 / 指针为空 / Retained::retain 失败 /
            // 窗口还没上屏（windowNumber == 0）—— 这些都是"这里没有一个可抬起
            // 的浮窗"，语义**确定**的 false，直接给结论（也省掉本 tick 的 50ms
            // 超时等待，hover 循环不会因为浮窗缺席而退化到 10Hz）。
            let Some(win) = app2.get_webview_window("floating") else {
                return Some(false);
            };
            let Ok(ptr) = win.ns_window() else {
                return Some(false);
            };
            if ptr.is_null() {
                return Some(false);
            }
            // D6-003 fix (2026-07-30 audit): 改用 Retained::retain() 拿 NSWindow,
            // 对齐 H2 fix (set_window_level) 的安全模式, 防止 future footgun.
            // 之前裸 &*ptr.cast 借 raw pointer 引用, 若浮窗在引用期间被 close,
            // 后续 windowNumber() 调 dispatch use-after-free. Retained 增加
            // 一次 retain 引用计数, 闭包内一直 hold 住, 闭包结束自动 release.
            let window: Retained<NSWindow> =
                match unsafe { Retained::retain(ptr.cast::<NSWindow>()) } {
                    Some(w) => w,
                    None => return Some(false),
                };
            let our_id = window.windowNumber();
            if our_id == 0 {
                // 窗口还没上屏（极少见，初始化竞态）→ 直接 false
                return Some(false);
            }
            // 传 0 = 不排除任何窗口，返回整个屏幕在该点 topmost window 的 number
            let Some(mtm) = MainThreadMarker::new() else {
                // fix (2026-07-28 审查): 之前 warn! —— 此分支一旦进入会随
                // hover emitter 20Hz 全天刷屏（≈172 万条/天）。降 trace，
                // 与函数内其它失败路径（dispatch 失败等）的级别对齐。
                // M-tray-10 fix：`return Some(false)` → `return None`。拿不到
                // marker = 这次读不到 AppKit 状态 = **无法判定**，不能当成
                // "鼠标不在窗上"。返 Some(false) 会喂给 hover emitter 的退
                // 场迟滞计数，凑够 2 tick 就把浮窗从光标底下抽走。
                tracing::trace!("is_floating_topmost_at: MainThreadMarker 不可用，跳过本 tick");
                return None;
            };
            let topmost = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, mtm);
            Some(topmost == our_id)
        })();
        // **B-NEW-1 / Fix #5（2026-06-19 audit）**：mutex poison 自动恢复而不是 .expect()。
        // 之前用 .expect("topmost slot mutex poisoned") —— 一旦主线程持锁路径 panic
        // （理论极少见但一旦发生），hover emitter 后续每次 20Hz 调 is_floating_topmost_at
        // 都会跟着 panic，tray 整体停摆。改成 unwrap_or_else(|e| e.into_inner())。
        {
            let mut g = slot2.slot.lock().unwrap_or_else(|e| e.into_inner());
            g.filled = Some((ticket, result));
        }
        slot2.cvar.notify_all();
    });
    if let Err(e) = dispatch_result {
        // D6-04 (2026-09-04 audit): 纯 trace 让「main thread 长期阻塞
        // (modal 面板) → hover 永久失灵」的场景完全无声。加连续失败计数，
        // 跨过阈值（20Hz × 50s = 1000 tick）时 warn 一次——不刷屏，但
        // 诊断有入口；dispatch 成功即复位，下次事故还能再告警。
        // L7 fix (2026-09-05 audit)：fetch_add 返回自增**前**值 —— 原判断
        // 首次 warn 实际发生在第 1001 次连续失败。加 1 后对齐注释语义。
        let fails = MAIN_DISPATCH_CONSECUTIVE_FAILS.fetch_add(1, Ordering::Relaxed) + 1;
        if fails.is_multiple_of(MAIN_DISPATCH_WARN_EVERY) {
            tracing::warn!(
                error = %e,
                consecutive_fails = fails,
                "is_floating_topmost_at: dispatch to main thread 持续失败，hover/玻璃效果可能长期失灵"
            );
        } else {
            tracing::trace!(
                error = %e,
                "is_floating_topmost_at: dispatch to main thread 失败，本 tick 无法判定（不采纳）"
            );
        }
        // M-tray-10 fix：**不再**把 slot 填 `Some(false)` + notify 让 caller
        // 立刻拿到 false。闭包永远不会被调度，等下去没有意义 —— 直接返
        // `None`（无法判定），caller / emitter `continue`。
        return None;
    }
    MAIN_DISPATCH_CONSECUTIVE_FAILS.store(0, Ordering::Relaxed);

    // 50ms 超时兜底：main thread 卡住时 hover 轮询不至于一起卡住。
    // 超时 / 只看到别的 ticket 的迟到写 → `None` = 无法判定，不采纳。
    let started = std::time::Instant::now();
    let deadline = Duration::from_millis(50);
    // 同样：poison 恢复（mutex 共享，跟上面 write 路径同源）
    let mut guard = slot.slot.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        // 只接受本 tick 自己的回填；ticket 不匹配 = 迟到的上一拍闭包，忽略。
        if let Some((t, v)) = guard.filled {
            if t == ticket {
                return v;
            }
        }
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return None;
        }
        let (g, _wait_timeout) = slot
            .cvar
            .wait_timeout(guard, remaining)
            .unwrap_or_else(|e| e.into_inner());
        guard = g;
    }
}

// ═══════════════════════════════════════════════════════════════════
//  Fullscreen watcher —— 检测全屏并自动隐藏浮窗
// ═══════════════════════════════════════════════════════════════════
//
// 思路：用 `+[NSMenu menuBarVisible]` 探测菜单栏是否可见。macOS 进入全屏
// 时（任何 app 的 fullscreen），菜单栏自动隐藏；退出全屏菜单栏恢复。
// 这是 macOS 默认行为，绝大多数用户没改。
//
// **已知 caveat**：用户在「系统设置 → 桌面与程序坞 → 在桌面上自动隐藏
// 并显示菜单栏」打开后，菜单栏在非全屏也会消失 → 会误触发隐藏浮窗。
// 这是 trade-off 已知局限，写在设置面板 help 文字里告诉用户。
//
// 设计：
// - tracker 始终运行（idempotent），由 lib.rs 启动一次
// - AUTO_HIDE_IN_FULLSCREEN 原子开关由 commands.rs save_config / 启动
//   时同步给 macos.rs（保持 config.json 单源真理）
// - WINDOW_HIDDEN_BY_FULLSCREEN 标志「窗口是被我们隐藏的」，避免用户手动
//   隐藏后又被我们误恢复

static FULLSCREEN_WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);
static AUTO_HIDE_IN_FULLSCREEN: AtomicBool = AtomicBool::new(false);
static WINDOW_HIDDEN_BY_FULLSCREEN: AtomicBool = AtomicBool::new(false);

/// 设置「全屏时自动隐藏浮窗」开关。
/// - 立即开启：watcher loop 下个 tick (≤2s) 会探测当前状态并执行
/// - 立即关闭：如果浮窗是被我们隐藏的，立刻恢复显示（不等 loop）
pub fn set_auto_hide_in_fullscreen<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let was = AUTO_HIDE_IN_FULLSCREEN.swap(enabled, Ordering::SeqCst);
    if was && !enabled {
        // 刚关闭功能 —— 如果窗口是我们之前自动藏起来的，立刻恢复
        if WINDOW_HIDDEN_BY_FULLSCREEN.swap(false, Ordering::SeqCst) {
            show_floating(app);
        }
    }
}

/// 启动 fullscreen watcher。idempotent，启动后整个 app 生命周期不停。
/// 由 lib.rs setup() 调一次。开销：2s 一次 + 一次主线程 dispatch + 一次
/// `[NSMenu menuBarVisible]` 读取，约 μs 级，可忽略。
pub fn start_fullscreen_watcher<R: Runtime>(app: AppHandle<R>) {
    if FULLSCREEN_WATCHER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }
    let builder = thread::Builder::new()
        .name("musage-fullscreen-watcher".into())
        .spawn(move || {
            tracing::debug!("fullscreen watcher 启动");
            let mut last_fs = false;
            loop {
                thread::sleep(Duration::from_secs(2));

                // D6-001 fix (2026-07-30 audit): fullscreen watcher OS 线程
                // 永久循环, quit_app 同步 SHUTDOWN_NATIVE_THREADS atomic,
                // 此线程每 2s tick 检查后退出。 150ms sleep 余量内能干净退出,
                // 跟 macOS/Win hover emitter 行为对齐, 避免 quit_app 后跑空 tick。
                if crate::poller::SHUTDOWN_NATIVE_THREADS.load(std::sync::atomic::Ordering::SeqCst)
                {
                    tracing::debug!("macOS fullscreen watcher 收到 SHUTDOWN, 退出");
                    break;
                }

                // 功能开关关：还原任何之前的自动隐藏 + 重置状态
                if !AUTO_HIDE_IN_FULLSCREEN.load(Ordering::SeqCst) {
                    if WINDOW_HIDDEN_BY_FULLSCREEN.swap(false, Ordering::SeqCst) {
                        show_floating(&app);
                    }
                    last_fs = false;
                    continue;
                }

                // 功能开关开：探测 + 响应状态变化
                let is_fs = is_menubar_hidden(&app);
                if is_fs == last_fs {
                    continue;
                }
                last_fs = is_fs;

                if is_fs {
                    // 进入全屏 —— 仅在我们**之前未藏过**且**当前可见**时才藏。
                    // H-Tray fix (2026-09-07 audit, merge 2026-09-08 采纳远端
                    // 同域命中版)：模块 doc 自承 flag 语义是"避免用户手动隐藏
                    // 后又被我们误恢复",但旧实现是无条件 swap(true) —— 即使用
                    // 户先手动 hide_floating 再进全屏 → flag 仍被置 true →
                    // 退出全屏时误调 show_floating 把窗口弹回来。加 is_visible
                    // 闸:窗口已经不可见就不动 flag,保留用户自己的隐藏意图。
                    if !WINDOW_HIDDEN_BY_FULLSCREEN.load(Ordering::SeqCst)
                        && is_floating_visible(&app)
                    {
                        WINDOW_HIDDEN_BY_FULLSCREEN.store(true, Ordering::SeqCst);
                        tracing::debug!("检测到全屏 → 隐藏浮窗");
                        hide_floating(&app);
                    }
                } else {
                    // 退出全屏 —— 仅在我们**之前自己藏过**时才恢复(防止恢复用户
                    // 手动隐藏的窗口)。
                    if WINDOW_HIDDEN_BY_FULLSCREEN.swap(false, Ordering::SeqCst) {
                        tracing::debug!("退出全屏 → 恢复浮窗");
                        show_floating(&app);
                    }
                }
            }
        });
    // **2026-06-20 audit**：之前 .expect()，线程数耗尽时整 app panic。降级 log + 翻转 RUNNING 让下次重启能重试。
    if let Err(e) = builder {
        tracing::error!(error = %e, "spawn fullscreen watcher thread 失败，auto-hide-in-fullscreen 将失效");
        FULLSCREEN_WATCHER_RUNNING.store(false, Ordering::SeqCst);
    }
}

/// 探测 macOS 菜单栏是否被隐藏。隐藏 → 大概率正在全屏。
/// 主线程同步调用（NSMenu 类方法需要 main thread）。
///
/// L17 fix（2026-06-26 audit）: 旧实现每次调用创建新的 mpsc::channel。
/// 改为全局复用 Condvar + Mutex 单槽位，与 is_floating_topmost_at 同款模式。
/// fullscreen watcher 0.5Hz × 86,400s ≈ 43K 次/24h，不如 hover emitter 的
/// 1.7M/24h 严重，但风格一致，避免给后续维护者两种实现去理解。
fn is_menubar_hidden<R: Runtime>(app: &AppHandle<R>) -> bool {
    use std::sync::{Arc, Condvar, Mutex};

    struct OneSlot {
        slot: Mutex<Option<bool>>,
        cvar: Condvar,
    }
    static SLOT: OnceLock<Arc<OneSlot>> = OnceLock::new();
    let slot = SLOT.get_or_init(|| {
        Arc::new(OneSlot {
            slot: Mutex::new(None),
            cvar: Condvar::new(),
        })
    });

    let slot2 = slot.clone();
    // L7 fix (2026-07-06 全量审查): `let _ = app.run_on_main_thread(...)`
    // 静默吞 Err —— main thread 挂 / NSOpenPanel modal / 关闭期间,dispatch
    // closure 永不跑,slot 永远 None,cvar 等到 200ms timeout 进入下一轮,本
    // tick 拿不到正确状态。改为:Err 时主动 fill slot = Some(false) +
    // notify,caller 拿到 unwrapped false 立即返回(假设没全屏),不让循环
    // 永久空转。
    let dispatch_result = app.run_on_main_thread(move || {
        let mtm = match MainThreadMarker::new() {
            Some(m) => m,
            None => {
                tracing::warn!("MainThreadMarker 不可用，is_menubar_hidden 跳过本 tick");
                let mut g = slot2.slot.lock().unwrap_or_else(|e| e.into_inner());
                *g = Some(false);
                slot2.cvar.notify_all();
                return;
            }
        };
        let visible = NSMenu::menuBarVisible(mtm);
        let mut g = slot2.slot.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(!visible);
        slot2.cvar.notify_all();
    });
    if let Err(e) = dispatch_result {
        tracing::warn!(error = ?e, "is_menubar_hidden: run_on_main_thread 失败,fallback false");
        if let Ok(mut g) = slot.slot.lock() {
            *g = Some(false);
            slot.cvar.notify_all();
        }
    }

    let started = std::time::Instant::now();
    loop {
        let mut g = slot.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = g.take() {
            return v;
        }
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_millis(200) {
            return false;
        }
        let remaining = Duration::from_millis(200) - elapsed;
        let _ = slot.cvar.wait_timeout(g, remaining);
    }
}

fn hide_floating<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            let _ = win.hide();
        }
    });
}

fn show_floating<R: Runtime>(app: &AppHandle<R>) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            let _ = win.show();
        }
    });
}

/// 探测浮窗当前是否可见(主线程同步)。给 H-Tray 全屏 watcher 用:仅当可见
/// 才让 watcher 把 WINDOW_HIDDEN_BY_FULLSCREEN 置 true,避免用户手动隐藏后
/// 退出全屏时被我们误恢复。
fn is_floating_visible<R: Runtime>(app: &AppHandle<R>) -> bool {
    // `run_on_main_thread` 闭包要 'static,AppHandle 是 Clone (内部 Arc),clone 一份。
    use std::sync::mpsc;
    let app2 = app.clone();
    let (tx, rx) = mpsc::channel();
    let _ = app.run_on_main_thread(move || {
        let visible = app2
            .get_webview_window("floating")
            .map(|w| w.is_visible().unwrap_or(false))
            .unwrap_or(false);
        let _ = tx.send(visible);
    });
    rx.recv_timeout(std::time::Duration::from_millis(100))
        .unwrap_or(false)
}

/// H2 fix (2026-07-29 审查): 检测 macOS 菜单栏当前外观。菜单栏在
/// "浅色"模式下背景接近白色,我们硬编码的白字 Rgba([255,255,255,255])
/// 会变得完全不可见。返回 true 表示"菜单栏是浅色背景",caller 改用
/// 黑色文字。返回 false 表示"菜单栏是深色背景",保持白色文字。
///
/// 实现: NSApp().effectiveAppearance() 拿当前应用 effective 外观,
/// 跟 NSAppearanceNameAqua (light) 比对判断。effectiveAppearance 跟随
/// 系统外观 (系统设置 → 外观 → 浅色/深色/自动),也跟随 NSApp override。
///
/// H-6 fix (2026-09-05 audit)：原实现只认 MainThreadMarker，但全部调用点
/// （publish_snapshot / refresh_now / set_tray_* / locale-changed 监听）
/// 都在 tokio worker / 事件回调线程 —— MainThreadMarker 恒 None，该功能
/// 自引入以来从未真正生效过。改为与 [`is_menubar_hidden`] 同款
/// run_on_main_thread + Condvar 单槽位派发；已在主线程时走 fast path。
/// 派发失败 / 超时保守返 false（保持白字，深色菜单栏是常见默认）。
#[cfg(target_os = "macos")]
pub fn menu_bar_is_light<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    use objc2::MainThreadMarker;

    // fast path：已在主线程（如 locale-changed 监听器走 run_on_main_thread
    // 的场景、托盘菜单回调），直接读，不再派发一轮。
    if let Some(mtm) = MainThreadMarker::new() {
        return native_menu_bar_is_light(mtm);
    }

    use std::sync::{Arc, Condvar, Mutex};

    struct OneSlot {
        slot: Mutex<Option<bool>>,
        cvar: Condvar,
    }
    static SLOT: OnceLock<Arc<OneSlot>> = OnceLock::new();
    let slot = SLOT.get_or_init(|| {
        Arc::new(OneSlot {
            slot: Mutex::new(None),
            cvar: Condvar::new(),
        })
    });

    let slot2 = slot.clone();
    let dispatch_result = app.run_on_main_thread(move || {
        let v = match MainThreadMarker::new() {
            Some(mtm) => native_menu_bar_is_light(mtm),
            None => {
                tracing::warn!("MainThreadMarker 不可用，menu_bar_is_light 派发闭包跳过");
                false
            }
        };
        let mut g = slot2.slot.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(v);
        slot2.cvar.notify_all();
    });
    if dispatch_result.is_err() {
        return false;
    }

    let started = std::time::Instant::now();
    loop {
        let mut g = slot.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = g.take() {
            return v;
        }
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_millis(200) {
            return false;
        }
        let remaining = Duration::from_millis(200) - elapsed;
        let _ = slot.cvar.wait_timeout(g, remaining);
    }
}

/// 主线程上真正读取 NSApp effectiveAppearance 的部分。
#[cfg(target_os = "macos")]
fn native_menu_bar_is_light(mtm: objc2::MainThreadMarker) -> bool {
    use objc2_app_kit::NSApplication;

    let app = NSApplication::sharedApplication(mtm);
    let appearance = app.effectiveAppearance();
    let name = appearance.name();
    // name 是 NSAppearanceName (NSString 包装)。转成 Rust &str 比较内容,
    // 避免 objc2 0.6 的 NSString bridging 差异 (isEqual / isEqualTo /
    // as_ref 行为各异)。
    let name_str = name.to_string();
    if name_str == "NSAppearanceNameAqua" || name_str == "Aqua" {
        return true; // Aqua = 浅色背景
    }
    if name_str.contains("Dark") {
        return false; // DarkAqua / DarkVibrantDark 等 = 深色背景
    }
    // 其他 (VibrantLight / HighContrastVibrantLight 等): 当浅色处理
    true
}

#[cfg(not(target_os = "macos"))]
#[inline]
pub fn menu_bar_is_light() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // H-6 fix (2026-09-05 audit)：menu_bar_is_light 改为收 &AppHandle（主线程
    // 派发），单测环境拿不到 AppHandle / 主线程，原占位测试删除 —— 该函数的
    // 真机验证走手动 QA（托盘图标颜色随系统外观切换）。
    #[allow(unused)]
    fn _menu_bar_is_light_signature_probe() {
        // 编译期签名护栏：确认函数存在且泛型签名未漂移（不实际调用）。
        let _: fn(&tauri::AppHandle<tauri::Wry>) -> bool = menu_bar_is_light::<tauri::Wry>;
    }
}
