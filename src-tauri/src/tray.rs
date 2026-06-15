//! 系统托盘动态图标生成
//!
//! 渲染规则：
//! - 32x32 RGBA，**透明底**（无背景填充）
//! - MiniMax：两条水平迷你进度条，上 = 5h utilization，下 = 周 utilization
//!   - 轨道 = 暗灰圆角矩形
//!   - 填充 = 白色实心（按已用% 决定填充宽度）
//! - DeepSeek：保留单大数字 + 货币单位（钱包余额没有"已用/总量"，进度条不适用）
//! - 都没有数据：留空（font 缺失同样留空）
//! - 托盘 tooltip：所有 provider 的核心状态，逗号分隔

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::Rgba;
use imageproc::drawing::draw_text_mut;
use std::sync::OnceLock;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager,
};

use crate::config::TrayIconStyle;
use crate::providers::{Provider, ProviderSnapshot, QuotaSnapshot};

// 字体加载：优先用户自选填 `assets/font.ttf`，再走系统字体 fallback，
// 最后用平台内置的备用路径。全部失败 → 纯色圆点（无文字）。
static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

fn load_font() -> Option<&'static FontVec> {
    FONT.get_or_init(|| {
        // 1. 用户自选填
        let user_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/font.ttf");
        if let Some(font) = try_load_font(&user_path) {
            tracing::debug!(path = %user_path.display(), "loaded user font");
            return Some(font);
        }

        // 2. 系统字体 fallback（单 face .ttf，避免 .ttc collection）
        for path in system_font_paths() {
            if let Some(font) = try_load_font(&path) {
                tracing::debug!(path = %path.display(), "loaded system font");
                return Some(font);
            }
        }

        tracing::warn!("no usable TTF found; tray will show color circle without text");
        None
    })
    .as_ref()
}

fn try_load_font(path: &std::path::Path) -> Option<FontVec> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| FontVec::try_from_vec(bytes).ok())
}

fn system_font_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    {
        // macOS 菜单栏渲染到 ~16-18px 实际像素，Regular 字体在那个尺寸下
        // 字形会糊掉看不清。**优先 Bold/Black 单 face TTF**（不动 .ttc 避免
        // collection 解析坑），保证 percent 模式的 "5h 83%" 清晰可读。
        paths.push("/System/Library/Fonts/Supplemental/Arial Black.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Arial Bold.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Arial Rounded Bold.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Tahoma Bold.ttf".into());
        // fallback: 变量字体（ab_glyph 加载后默认走 Regular，仍比没有强）
        paths.push("/System/Library/Fonts/SFNS.ttf".into());
        // 最后兜底
        paths.push("/System/Library/Fonts/Supplemental/Arial.ttf".into());
    }
    #[cfg(target_os = "windows")]
    {
        // Win 优先 arialbd（粗体），保证 tray 文字可见
        paths.push("C:/Windows/Fonts/arialbd.ttf".into());
        paths.push("C:/Windows/Fonts/arial.ttf".into());
        paths.push("C:/Windows/Fonts/segoeui.ttf".into());
        paths.push("C:/Windows/Fonts/segoeuib.ttf".into());
        paths.push("C:/Windows/Fonts/tahoma.ttf".into());
        paths.push("C:/Windows/Fonts/consola.ttf".into());
    }
    #[cfg(target_os = "linux")]
    {
        // Linux Bold 优先（DejaVu Sans-Bold / Liberation Sans-Bold）
        paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".into());
        paths.push("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf".into());
        paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
        paths.push("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf".into());
        paths.push("/usr/share/fonts/TTF/DejaVuSans.ttf".into());
    }
    paths
}

// **Win11 高 DPI 防糊（2026-06-11）**：通知区托盘槽位固定 16 DIP，按系统
// DPI 缩放后实际像素是 16/20/24/28/32/40/48/56（100/125/150/175/200/250/300/350%）。
// 32px 源在 100/125/150% 上被 GDI 默认拉伸模式（COLORONCOLOR/最近邻）下采样
// 到 16/20/24，结果模糊+锯齿；64px 是常见 DPI 档（≤400%）都能干净缩放的最小源。
// macOS NSStatusItem / Linux AppIndicator 对任意源尺寸都容错，无需提升。
#[cfg(target_os = "windows")]
const ICON_SIZE: u32 = 64;
#[cfg(not(target_os = "windows"))]
const ICON_SIZE: u32 = 32;

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    // 显隐合并：菜单里只留一个 "切换悬浮窗"，内部根据当前可见性自动判断
    // 该 show 还是 hide。跟左键单击同逻辑（on_tray_icon_event 里的 toggle）。
    let toggle_i = MenuItem::with_id(app, "toggle", "切换悬浮窗", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)?;
    let refresh_i = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?;
    // **Win 端 z-order 逃生口**（2026-06-12）：hover-raise 的 16ms tick +
    // dual-path + 焦点事件 hook 多管齐下，OS 还是持续 demote `WS_EX_TOPMOST`。
    // 给用户一个**主动**操作：菜单里点 "强制置顶浮窗" 走
    // `AllowSetForegroundWindow(ASFW_ANY) + SetForegroundWindow`，靠**抢前台**
    // 把浮窗真顶到最上面（**会**抢焦点，但用户点菜单那一瞬间本来就在
    // 操作我们 app，UX 可接受）。
    let force_top_i = MenuItem::with_id(
        app,
        "force_top_floating",
        "置顶一下",
        cfg!(target_os = "windows"),
        None::<&str>,
    )?;
    let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&toggle_i, &settings_i, &refresh_i, &force_top_i, &quit_i],
    )?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .tooltip("Musage - 加载中…")
        .icon(make_placeholder_icon())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => {
                if let Some(w) = app.get_webview_window("floating") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
            "settings" => {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::commands::open_settings_window(app2).await {
                        tracing::warn!(error = %e, "打开设置失败");
                    }
                });
            }
            "refresh" => {
                let app2 = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::poller::tick_now(&app2).await {
                        tracing::warn!(error = %e, "手动刷新失败");
                    }
                });
            }
            "force_top_floating" => {
                // 走 Win 私有路径：先解锁前台锁定（ASFW_ANY 允许任何
                // process 抢前台），再 SetForegroundWindow 抢前台。
                // **会**把焦点抢过来 —— 这是用户**主动**点菜单触发的
                // 操作，UX 上可接受（用户此刻在操作我们 app）。
                if let Some(w) = app.get_webview_window("floating") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    // hwnd() / windows_sys crate 在 Tauri 2 里 cfg-gate 在 windows,
                    // macos / linux 上不存在这两个 symbol。Win32 API 整块 cfg-gate。
                    #[cfg(target_os = "windows")]
                    {
                        if let Ok(hwnd) = w.hwnd() {
                            use windows_sys::Win32::UI::WindowsAndMessaging::{
                                AllowSetForegroundWindow, SetForegroundWindow,
                            };
                            unsafe {
                                let _ = AllowSetForegroundWindow(0x00000001); // ASFW_ANY
                                let _ = SetForegroundWindow(hwnd.0);
                            }
                        }
                    }
                    // 同时把 WS_EX_TOPMOST 持久化（这样即便焦点之后
                    // 丢失，level-trigger 也能在 16ms 内 re-assert）
                    let _ = w.set_always_on_top(true);
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击切换悬浮窗显隐
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("floating") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

pub fn update_tray_from_snapshot(
    app: &AppHandle,
    snap: &QuotaSnapshot,
    style: TrayIconStyle,
) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return Ok(());
    };
    tray.set_icon(Some(render_icon(snap, style)))?;
    tray.set_tooltip(Some(tooltip(snap)))?;
    Ok(())
}

fn make_placeholder_icon() -> Image<'static> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("icons/tray-base.png");
    if let Ok(img) = image::open(&path) {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        return Image::new_owned(rgba.into_raw(), w, h);
    }
    let img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));
    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

/// v0.6+ 托盘图标渲染：根据 [`TrayIconStyle`] 分发。
///
/// - `Logo`   ：画 icons/tray-base.png（白底 M），不显示实时数据
/// - `Bars`   ：MiniMax 双水平进度条
/// - `Percent`：MiniMax 双行百分比文本（font 缺失时 fallback 到 Bars）
///
/// bars / percent 模式在 MiniMax 没有/失败时**退化到 logo**（不再走旧
/// DeepSeek 大数字 —— v0.6+ 3 选 1 心智模型，深向文本风格作 v2 扩展）。
fn render_icon(snap: &QuotaSnapshot, style: TrayIconStyle) -> Image<'static> {
    if style == TrayIconStyle::Logo {
        return make_placeholder_icon();
    }

    let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));

    match pick_minimax_rows(snap) {
        Some((five_h, weekly)) => match style {
            TrayIconStyle::Bars => draw_mini_bars(&mut img, five_h, weekly),
            TrayIconStyle::Percent => draw_percent(&mut img, five_h, weekly),
            TrayIconStyle::Logo => unreachable!("logo handled above"),
        },
        None => {
            // bars / percent 模式：MiniMax 没有/失败 → 退化为 logo
            // 单独重建一张避免在主 img 上画占位
            return make_placeholder_icon();
        }
    }

    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

/// 取 MiniMax 行的 5h / 周 utilization（缺则 0.0）。
/// MiniMax 不存在或失败时返回 None。
fn pick_minimax_rows(snap: &QuotaSnapshot) -> Option<(f64, f64)> {
    let m = snap
        .providers
        .iter()
        .find(|p| p.provider == Provider::Minimax && p.success)?;
    let five_h = m
        .rows
        .iter()
        .find(|r| r.label == "5h")
        .and_then(|r| r.utilization)
        .unwrap_or(0.0);
    let weekly = m
        .rows
        .iter()
        .find(|r| r.label == "周")
        .and_then(|r| r.utilization)
        .unwrap_or(0.0);
    Some((five_h, weekly))
}

/// 画两条水平迷你进度条。
///
/// 布局（32x32）：
/// ```
/// ┌──────────────────────────┐
/// │                          │  ← 6px 顶 padding
/// │   ████████░░░░░░░░░░░░   │  ← 5h 进度条 (高 9px)
/// │   ██░░░░░░░░░░░░░░░░░░   │  ← 周 进度条 (高 9px)
/// │                          │  ← 6px 底 padding
/// └──────────────────────────┘
///     ↑ 3px                  ↑ 3px
/// ```
fn draw_mini_bars(img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>, util_top: f64, util_bot: f64) {
    // 所有像素常量按 32x32 原设计折算到当前 ICON_SIZE，比例：
    //   PAD_X 3/32, BAR_H 9/32, GAP 2/32, TOP 6/32, RADIUS 2/32
    // 32→64 时这些值整体翻倍（6/12/4/12/4/52/18/4），布局完全等比。
    let s = ICON_SIZE as i32;
    let pad_x = s * 3 / 32;          // 3  →  6
    let bar_w = s - pad_x * 2;       // 26 → 52
    let bar_h = s * 9 / 32;          // 9  → 18
    let gap = s * 2 / 32;            // 2  →  4
    let top = s * 6 / 32;            // 6  → 12
    let radius = s * 2 / 32;         // 2  →  4
    let track = Rgba([60u8, 60, 60, 255]);
    let fill = Rgba([255u8, 255, 255, 255]);

    let pct = |u: f64| -> u32 { (u.clamp(0.0, 100.0)).round() as u32 };

    draw_rounded_bar(
        img,
        pad_x,
        top,
        bar_w,
        bar_h,
        pct(util_top),
        track,
        fill,
        radius,
    );
    draw_rounded_bar(
        img,
        pad_x,
        top + bar_h + gap,
        bar_w,
        bar_h,
        pct(util_bot),
        track,
        fill,
        radius,
    );
}

/// 单条圆角水平进度条。先画整个轨道，再叠加按 % 裁宽的填充。
fn draw_rounded_bar(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    fill_pct: u32,
    track: Rgba<u8>,
    fill: Rgba<u8>,
    radius: i32,
) {
    // 轨道
    fill_rounded_rect(img, x, y, w, h, radius, track);
    // 填充（至少 1px 让 0% 和 "没数据" 视觉上区分开 —— 0% 也是有一截小白条）
    let fill_w = ((w as u32).saturating_mul(fill_pct.min(100)) / 100).max(1) as i32;
    fill_rounded_rect(img, x, y, fill_w, h, radius, fill);
}

/// 填充一个圆角矩形。逐像素判断是否在圆角内。
fn fill_rounded_rect(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: Rgba<u8>,
) {
    let r = radius.min(w / 2).min(h / 2).max(0);
    for py in y..(y + h) {
        for px in x..(x + w) {
            if px < 0 || py < 0 || px >= ICON_SIZE as i32 || py >= ICON_SIZE as i32 {
                continue;
            }
            // 4 个圆角：若 (px, py) 落在任一圆角外接矩形内，要算到圆心距离
            let in_left = (x..x + r).contains(&px);
            let in_right = (x + w - r..x + w).contains(&px);
            let in_top = (y..y + r).contains(&py);
            let in_bot = (y + h - r..y + h).contains(&py);

            let in_corner_zone = (in_left || in_right) && (in_top || in_bot);

            if !in_corner_zone {
                img.put_pixel(px as u32, py as u32, color);
            } else {
                // 计算到对应角圆心的距离
                let (cx, cy) = if in_left && in_top {
                    (x + r, y + r)
                } else if in_right && in_top {
                    (x + w - r - 1, y + r)
                } else if in_left && in_bot {
                    (x + r, y + h - r - 1)
                } else {
                    (x + w - r - 1, y + h - r - 1)
                };
                let dx = px - cx;
                let dy = py - cy;
                if dx * dx + dy * dy <= r * r {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
}

/// v0.6+ MiniMax 用：上行 5h 利用率，下行 周利用率，**右对齐纯白数字，无背景**。
///
/// 设计要点（用户 2026-06-12 反馈）：
/// - **无标签**：5h / 周这种语义已经在固定位置被认知（上行 = 5h，下行 = 周），
///   加 "5h" 字符浪费像素、稀释对比度
/// - **右对齐**：跟系统 / 企业微信 / 微信那种徽章风格保持一致，
///   "33%" / "100%" / "0%" 各种长度都对齐到右边
/// - **无背板**：纯白文字 + Bold 字体（macOS 优先 Arial Black）字形本身
///   足够粗，菜单栏透明背景上自然清晰
/// - **scale 14**：比 v1 的 11 大一档，菜单栏渲染到 ~16px 时字形不糊
///
/// font 缺失时 fallback 到 `draw_mini_bars`（保持信息密度，不留空让用户
/// 困惑 "是不是没数据"）。
fn draw_percent(img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>, util_top: f64, util_bot: f64) {
    let Some(font) = load_font() else {
        return draw_mini_bars(img, util_top, util_bot);
    };

    let s = ICON_SIZE as i32;
    let scale = PxScale::from(s as f32 * 14.0 / 32.0); // 14 → 28
    // 两行基线：上行顶部 y=2，下行顶部 y=18。两者之间留 2px 空隙避免粘连。
    let y_top = s * 2 / 32;  //  2 →  4
    let y_bot = s * 18 / 32; // 18 → 36
    let pad_right = s * 2 / 32; // 右边留 2px 内边距
    let color = Rgba([255, 255, 255, 255]);

    let top = format!("{}%", util_top.round() as i64);
    let bot = format!("{}%", util_bot.round() as i64);

    draw_right_text(img, &top, scale, y_top, pad_right, font, color);
    draw_right_text(img, &bot, scale, y_bot, pad_right, font, color);
}

/// 在 ICON_SIZE 宽画布上**右对齐**画一行文字，距右边 `pad_right` 像素。
fn draw_right_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    text: &str,
    scale: PxScale,
    y: i32,
    pad_right: i32,
    font: &FontVec,
    color: Rgba<u8>,
) {
    let scaled = font.as_scaled(scale);
    let w: f32 = text
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum();
    // 右对齐 x = ICON_SIZE - text_width - pad_right
    let x = (ICON_SIZE as f32 - w - pad_right as f32).max(1.0) as i32;
    draw_text_mut(img, color, x, y, scale, font, text);
}

/// 在 ICON_SIZE 宽画布上居中画一行文字。**已废弃**（percent 模式改右对齐），
/// 保留供未来 v2 "logo + 居中文字" 等变体。
#[allow(dead_code)]
fn draw_centered_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    text: &str,
    scale: PxScale,
    y: i32,
    font: &FontVec,
    color: Rgba<u8>,
) {
    let scaled = font.as_scaled(scale);
    let w: f32 = text
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum();
    let x = ((ICON_SIZE as f32 - w) / 2.0).max(1.0) as i32;
    draw_text_mut(img, color, x, y, scale, font, text);
}

fn tooltip(snap: &QuotaSnapshot) -> String {
    if snap.providers.is_empty() {
        return "Musage · 加载中…".to_string();
    }
    let mut parts = vec!["Musage".to_string()];
    for p in &snap.providers {
        let dot = match p.health_label() {
            "ok" => "🟢",
            "warn" => "🟡",
            "alert" => "🔴",
            _ => "⚪",
        };
        let body = provider_short_body(p);
        parts.push(format!("{dot} {body}"));
    }
    if let Some(ms) = snap.fetched_at {
        let dt = chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_default();
        parts.push(format!("更新于 {dt}"));
    }
    parts.join(" · ")
}

fn provider_short_body(p: &ProviderSnapshot) -> String {
    if !p.success {
        let err = p.error.as_deref().unwrap_or("未知错误");
        // 截短避免 tooltip 太长
        return format!("{}: {}", p.provider.display_name(), truncate(err, 30));
    }
    match p.provider {
        Provider::Minimax => {
            // "5h 45% / 周 72%"
            let mut parts = Vec::new();
            for r in &p.rows {
                if let Some(u) = r.utilization {
                    parts.push(format!("{} {}%", r.label, u.round() as i64));
                }
            }
            if parts.is_empty() {
                p.provider.display_name().to_string()
            } else {
                format!("{} {}", p.provider.display_name(), parts.join(" / "))
            }
        }
        Provider::Deepseek => {
            // "DeepSeek ¥128.50"
            if let Some(r) = p.rows.iter().find(|r| r.remaining.is_some()) {
                let amount = r
                    .remaining
                    .map(format_amount_short)
                    .unwrap_or_else(|| "?".to_string());
                let unit = r.unit.as_deref().unwrap_or("");
                format!("{} {}{}", p.provider.display_name(), amount, unit)
            } else {
                p.provider.display_name().to_string()
            }
        }
        Provider::Xiaomimimo => {
            // 跟 MiniMax 一样："Xiaomi MiMo 月度 5% / 补偿 100%"
            let mut parts = Vec::new();
            for r in &p.rows {
                if let Some(u) = r.utilization {
                    parts.push(format!("{} {}%", r.label, u.round() as i64));
                }
            }
            if parts.is_empty() {
                p.provider.display_name().to_string()
            } else {
                format!("{} {}", p.provider.display_name(), parts.join(" / "))
            }
        }
    }
}

fn format_amount_short(v: f64) -> String {
    let r = v.round() as i64;
    if r >= 100_000 {
        // 大数字用 k 简写
        format!("{}k", r / 1000)
    } else if v >= 1000.0 {
        format!("{:.1}k", v / 1000.0)
    } else {
        format!("{:.2}", v)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}
