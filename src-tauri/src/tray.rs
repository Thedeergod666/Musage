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
        // macOS 系统自带的单 face TTF（不动 .ttc，避免 collection 解析坑）
        paths.push("/System/Library/Fonts/Supplemental/Arial.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Arial Bold.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Verdana.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Georgia.ttf".into());
        paths.push("/System/Library/Fonts/Supplemental/Tahoma.ttf".into());
        paths.push("/Library/Fonts/Arial.ttf".into());
    }
    #[cfg(target_os = "windows")]
    {
        paths.push("C:/Windows/Fonts/arial.ttf".into());
        paths.push("C:/Windows/Fonts/arialbd.ttf".into());
        paths.push("C:/Windows/Fonts/segoeui.ttf".into());
        paths.push("C:/Windows/Fonts/tahoma.ttf".into());
        paths.push("C:/Windows/Fonts/consola.ttf".into());
    }
    #[cfg(target_os = "linux")]
    {
        paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
        paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".into());
        paths.push("/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf".into());
        paths.push("/usr/share/fonts/TTF/DejaVuSans.ttf".into());
    }
    paths
}

const ICON_SIZE: u32 = 32;

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "show", "显示悬浮窗", true, None::<&str>)?;
    let hide_i = MenuItem::with_id(app, "hide", "隐藏悬浮窗", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)?;
    let refresh_i = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &hide_i, &settings_i, &refresh_i, &quit_i])?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .tooltip("Musage - 加载中…")
        .icon(make_placeholder_icon())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("floating") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "hide" => {
                if let Some(w) = app.get_webview_window("floating") {
                    let _ = w.hide();
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
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}

pub fn update_tray_from_snapshot(app: &AppHandle, snap: &QuotaSnapshot) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return Ok(());
    };
    tray.set_icon(Some(render_icon(snap)))?;
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

fn render_icon(snap: &QuotaSnapshot) -> Image<'static> {
    // 透明背景 —— MiniMax 走双进度条，DeepSeek 走单大数字
    let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));

    match pick_content(snap) {
        Some(TrayContent::MinimaxBars { five_h, weekly }) => {
            draw_mini_bars(&mut img, five_h, weekly);
        }
        Some(TrayContent::DeepseekText { amount, currency }) => {
            // 单大数字 + 小字货币
            draw_deepseek_text(&mut img, amount, &currency);
        }
        None => {
            // 全失败：留空（font 缺失也留空）
        }
    }

    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

/// Tray 渲染内容选择结果
enum TrayContent {
    /// MiniMax：上 5h utilization（0-100），下 周 utilization（0-100）
    MinimaxBars { five_h: f64, weekly: f64 },
    /// DeepSeek：余额数字 + 货币单位（如 128.5 / "CNY"）
    DeepseekText { amount: f64, currency: String },
}

/// 选 primary provider 并返回对应渲染内容。
///
/// 优先级：MiniMax 成功 > DeepSeek 成功 > None（失败/空）。
fn pick_content(snap: &QuotaSnapshot) -> Option<TrayContent> {
    if snap.providers.is_empty() {
        return None;
    }

    if let Some(minimax) = snap
        .providers
        .iter()
        .find(|p| p.provider == Provider::Minimax && p.success)
    {
        let five_h = minimax
            .rows
            .iter()
            .find(|r| r.label == "5h")
            .and_then(|r| r.utilization)
            .unwrap_or(0.0);
        let weekly = minimax
            .rows
            .iter()
            .find(|r| r.label == "周")
            .and_then(|r| r.utilization)
            .unwrap_or(0.0);
        return Some(TrayContent::MinimaxBars { five_h, weekly });
    }

    if let Some(deepseek) = snap
        .providers
        .iter()
        .find(|p| p.provider == Provider::Deepseek && p.success)
    {
        let amount = deepseek
            .rows
            .iter()
            .find_map(|r| r.remaining)
            .unwrap_or(0.0);
        let currency = deepseek
            .rows
            .iter()
            .find_map(|r| r.unit.clone())
            .unwrap_or_else(|| "CNY".to_string());
        return Some(TrayContent::DeepseekText { amount, currency });
    }

    None
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
    const PAD_X: i32 = 3;
    const BAR_W: i32 = ICON_SIZE as i32 - PAD_X * 2; // 26
    const BAR_H: i32 = 9;
    const GAP: i32 = 2;
    const TOP: i32 = 6;
    const RADIUS: i32 = 2;
    let track = Rgba([60u8, 60, 60, 255]);
    let fill = Rgba([255u8, 255, 255, 255]);

    let pct = |u: f64| -> u32 { (u.clamp(0.0, 100.0)).round() as u32 };

    draw_rounded_bar(
        img,
        PAD_X,
        TOP,
        BAR_W,
        BAR_H,
        pct(util_top),
        track,
        fill,
        RADIUS,
    );
    draw_rounded_bar(
        img,
        PAD_X,
        TOP + BAR_H + GAP,
        BAR_W,
        BAR_H,
        pct(util_bot),
        track,
        fill,
        RADIUS,
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

/// DeepSeek 用：大号余额数字 + 小号货币单位
fn draw_deepseek_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    amount: f64,
    currency: &str,
) {
    let Some(font) = load_font() else { return };

    // 大数字：scale 16，居中放上半部分
    let big_scale = PxScale::from(16.0);
    let big_text = format_amount_tray(amount);
    let big_scaled = font.as_scaled(big_scale);
    let big_w: f32 = big_text
        .chars()
        .map(|c| big_scaled.h_advance(font.glyph_id(c)))
        .sum();
    let big_x = ((ICON_SIZE as f32 - big_w) / 2.0).max(1.0) as i32;
    draw_text_mut(img, Rgba([255, 255, 255, 255]), big_x, 2, big_scale, font, &big_text);

    // 小货币：scale 9，居中放下半部分
    let small_scale = PxScale::from(9.0);
    let small_scaled = font.as_scaled(small_scale);
    let small_w: f32 = currency
        .chars()
        .map(|c| small_scaled.h_advance(font.glyph_id(c)))
        .sum();
    let small_x = ((ICON_SIZE as f32 - small_w) / 2.0).max(1.0) as i32;
    draw_text_mut(
        img,
        Rgba([180, 180, 180, 255]),
        small_x,
        22,
        small_scale,
        font,
        currency,
    );
}

/// 紧凑金额（tray 用）：<100 整数；<1000 1 位小数 k；<1M 整数 k；否则 1 位小数 M。
/// 32x32 限制字符数 ≤ 5。
fn format_amount_tray(v: f64) -> String {
    let abs = v.abs();
    if abs >= 1_000_000.0 {
        format!("{:.1}M", (v / 1_000_000.0) as f32)
    } else if abs >= 10_000.0 {
        format!("{}k", (v / 1000.0).round() as i64)
    } else if abs >= 1000.0 {
        format!("{:.1}k", (v / 1000.0) as f32)
    } else if abs >= 100.0 {
        format!("{}", v.round() as i64)
    } else {
        // 小数余额（<100）：保留 1 位小数
        format!("{:.1}", v)
    }
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
