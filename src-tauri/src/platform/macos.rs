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
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

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

// ── 公开 API ──

/// PinBottom 模式启动时调：把 level 切到 below-normal，并开启 hover 切 level。
/// tracker 已由 [`start_hover_emitter`] 在 app 启动时拉起，这里只翻开关。
pub fn set_window_pin_bottom<R: Runtime>(app: &AppHandle<R>) {
    set_window_level(app, LEVEL_BELOW_NORMAL, true); // M3 fix: true = is_pin_bottom
    LEVEL_SWITCHING_ACTIVE.store(true, Ordering::SeqCst);
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
pub fn start_hover_emitter<R: Runtime>(app: AppHandle<R>) {
    if TRACKER_RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在跑
    }
    let builder = thread::Builder::new()
        .name("musage-hover-emitter".into())
        .spawn(move || {
            tracing::debug!("hover emitter 启动");
            let mut last_inside = false;
            loop {
                thread::sleep(Duration::from_millis(50));

                // mouseLocation 在 macOS 上是 thread-safe 的，可从任意线程调
                let mouse = NSEvent::mouseLocation();

                // 关键：用 NSWindow.windowNumberAtPoint 做命中测试 ——
                // 不光检查"鼠标在不在浮窗 frame 内"，还要确认浮窗在该点是**最上层**。
                // PinBottom 模式下浮窗经常被其它 app 部分遮挡，单纯 point-in-rect
                // 会在被遮挡区域也误触发置顶（用户其实在操作遮挡它的那个 app）。
                let inside = is_floating_topmost_at(&app, mouse);

                if inside != last_inside {
                    last_inside = inside;

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

/// 把浮窗的 NSWindow level 切到 `level`。dispatch 到 main thread（AppKit 强制要求）。
/// M3 fix: 加 `is_pin_bottom` 参数。PinBottom 模式设 hidesOnDeactivate(false)
/// (否则鼠标一离开焦点窗口就消失)；PinTop / Normal 走默认值(true)，
/// Normal 模式失焦时窗口应被隐藏(跟普通窗口一致)。
pub fn set_window_level<R: Runtime>(app: &AppHandle<R>, level: CGWindowLevel, is_pin_bottom: bool) {
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(win) = app2.get_webview_window("floating") {
            if let Ok(ptr) = win.ns_window() {
                if !ptr.is_null() {
                    // SAFETY: `ptr` 来自 webview_window 的 NSWindow，整个 app 生命周期有效。
                    let window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
                    window.setLevel(level as _);
                    // M3 fix: 只 PinBottom 模式设 false，PinTop/Normal 走默认(true)。
                    // 之前无条件 false 导致 Normal 模式失焦后窗口仍保持可见，
                    // 行为跟 PinBottom 一致，违反 "Normal = 跟普通窗口一样" 语义。
                    window.setHidesOnDeactivate(!is_pin_bottom);
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
/// 拿不到 / 超时 / 浮窗未上屏 → 保守返回 false。
///
/// **L12 fix（2026-06-19）**：旧实现每调用一次就新建一对 `mpsc::channel::<bool>()`。
/// hover emitter 20Hz × 86,400s ≈ 1.7M 次/24h，allocator churn 严重。改用
/// 全局复用的 `std::sync::Mutex<Option<bool>>` + `Condvar` 单槽位（外层包
/// `OnceLock<Arc<...>>` 复用）。hover emitter 串行调用，单槽位足够。
fn is_floating_topmost_at<R: Runtime>(app: &AppHandle<R>, point: NSPoint) -> bool {
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

    let app2 = app.clone();
    let slot2 = slot.clone();
    // **M2 fix（2026-07-02 audit）**：之前 `let _ = app.run_on_main_thread(...)`
    // 静默吞 Err —— main thread 忙 / Tauri event loop 挂时,run_on_main_thread
    // 返 Err,closure 永远不被调度 → cvar 等 50ms 超时返 false。下一次 poll
    // 又重复同样流程。如果 main thread 长时间忙（极少见但理论可能），hover
    // emitter 持续 20Hz 失败但用户看不到任何 log,浮窗玻璃效果永久失效。
    // 改为: run_on_main_thread 失败时记录 warning (首次 fail 后降级为 trace
    // 避免 log spam),把 slot 填 false 让 cvar 立即 notify 走 timeout 路径。
    let dispatch_result = app.run_on_main_thread(move || {
        let result = (|| -> Option<bool> {
            let win = app2.get_webview_window("floating")?;
            let ptr = win.ns_window().ok()?;
            if ptr.is_null() {
                return None;
            }
            // SAFETY: ptr 来自 webview_window 的 NSWindow，整个 app 生命周期有效。
            let window: &NSWindow = unsafe { &*ptr.cast::<NSWindow>() };
            let our_id = window.windowNumber();
            if our_id == 0 {
                // 窗口还没上屏（极少见，初始化竞态）→ 直接 false
                return Some(false);
            }
            // 传 0 = 不排除任何窗口，返回整个屏幕在该点 topmost window 的 number
            // **2026-06-20 audit**：MainThreadMarker 拿不到时改返 false 兜底，
            // 避免拿不到 → panic → hover emitter 永久停掉。
            let Some(mtm) = MainThreadMarker::new() else {
                tracing::warn!("is_floating_topmost_at: MainThreadMarker 不可用，跳过本 tick");
                return Some(false);
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
            *g = Some(result.unwrap_or(false));
        }
        slot2.cvar.notify_all();
    });
    if let Err(e) = dispatch_result {
        // 主线程无法调度 (临时忙 / 退出中) —— 立即把 slot 填 false 让
        // cvar notify_all 提前返 poll 路径,避免调用方空等 50ms。
        tracing::trace!(
            error = %e,
            "is_floating_topmost_at: dispatch to main thread 失败，立即返 false"
        );
        {
            let mut g = slot.slot.lock().unwrap_or_else(|e| e.into_inner());
            if g.is_none() {
                *g = Some(false);
            }
        }
        slot.cvar.notify_all();
    }

    // 50ms 超时兜底：main thread 卡住时 hover 轮询不至于一起卡住
    let started = std::time::Instant::now();
    let deadline = Duration::from_millis(50);
    // 同样：poison 恢复（mutex 共享，跟上面 write 路径同源）
    let mut guard = slot.slot.lock().unwrap_or_else(|e| e.into_inner());
    while guard.is_none() && started.elapsed() < deadline {
        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        let (g, _wait_timeout) = slot
            .cvar
            .wait_timeout(guard, remaining)
            .unwrap_or_else(|e| e.into_inner());
        guard = g;
    }
    guard.unwrap_or(false)
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
                    // 进入全屏 —— 隐藏浮窗（若我们尚未藏）
                    if !WINDOW_HIDDEN_BY_FULLSCREEN.swap(true, Ordering::SeqCst) {
                        tracing::debug!("检测到全屏 → 隐藏浮窗");
                        hide_floating(&app);
                    }
                } else {
                    // 退出全屏 —— 恢复浮窗（若是我们之前藏的）
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
    let _ = app.run_on_main_thread(move || {
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
