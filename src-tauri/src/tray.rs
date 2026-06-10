//! 系统托盘动态图标生成
//!
//! 渲染规则：
//! - 32x32 RGBA
//! - 圆形背景：颜色 = 所有 provider 中**最差**的 health（绿/橙/红/灰）
//! - 中心两行文字（font 加载失败时只画圆）：
//!   - 优先 MiniMax：上 `h<5h%>`、下 `w<周%>`（h=hour, w=week）
//!   - 其次 DeepSeek：上 余额数字、下 货币单位
//!   - 都没有：上 `!`、下 `!`
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
    let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));
    let center = ICON_SIZE as i32 / 2;
    let radius = center - 1;
    let r2 = radius * radius;

    // 颜色：所有 provider 中最差
    let health = snap.worst_health();
    let color = match health {
        "ok" => Rgba([76u8, 175, 80, 255]),    // 绿
        "warn" => Rgba([255u8, 152, 0, 255]),  // 橙
        "alert" => Rgba([244u8, 67, 54, 255]), // 红
        _ => Rgba([128u8, 128, 128, 255]),     // 灰
    };

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as i32 - center;
            let dy = y as i32 - center;
            if dx * dx + dy * dy <= r2 {
                img.put_pixel(x, y, color);
            }
        }
    }

    // 中心两行文字
    let (line1, line2) = pick_two_lines(snap);
    draw_two_line_text(&mut img, &line1, &line2, Rgba([255, 255, 255, 255]));

    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

/// 选 primary provider 并返回 (line1, line2)。
///
/// 优先级：MiniMax 成功 > DeepSeek 成功 > 失败态。
/// MiniMax 用 `h<5h%>` / `w<周%>`（h=hour, w=week，方便一眼区分）。
/// DeepSeek 用余额数字 / 货币单位（如 "128" / "CNY"）。
/// 缺失行用 `"—"` 占位。
fn pick_two_lines(snap: &QuotaSnapshot) -> (String, String) {
    if snap.providers.is_empty() {
        return ("…".to_string(), "".to_string());
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
            .map(format_pct_with_h_prefix)
            .unwrap_or_else(|| "h—".to_string());
        let weekly = minimax
            .rows
            .iter()
            .find(|r| r.label == "周")
            .and_then(|r| r.utilization)
            .map(format_pct_with_w_prefix)
            .unwrap_or_else(|| "w—".to_string());
        return (five_h, weekly);
    }

    if let Some(deepseek) = snap
        .providers
        .iter()
        .find(|p| p.provider == Provider::Deepseek && p.success)
    {
        let amount = deepseek
            .rows
            .iter()
            .find(|r| r.remaining.is_some())
            .and_then(|r| r.remaining)
            .map(format_amount_compact)
            .unwrap_or_else(|| "—".to_string());
        let currency = deepseek
            .rows
            .iter()
            .find(|r| r.remaining.is_some())
            .and_then(|r| r.unit.clone())
            .unwrap_or_else(|| "CNY".to_string());
        return (amount, currency);
    }

    ("!".to_string(), "×".to_string())
}

/// "h<已用%>" —— 例如 "h10%"。32x32 装得下 4 字符。
fn format_pct_with_h_prefix(v: f64) -> String {
    let n = v.round().clamp(0.0, 999.0) as i64;
    format!("h{}%", n)
}

/// "w<已用%>" —— 例如 "w5%"。
fn format_pct_with_w_prefix(v: f64) -> String {
    let n = v.round().clamp(0.0, 999.0) as i64;
    format!("w{}%", n)
}

/// 紧凑金额格式：<1000 整数；<10000 1 位小数 k；<1M 整数 k；否则 1 位小数 M。
/// 32x32 限制字符数 ≤ 5。
fn format_amount_compact(v: f64) -> String {
    let abs = v.abs();
    if abs >= 1_000_000.0 {
        // 转 f32 走 f32 的 Display 格式化（f64 没有 {:#.1} 那种 trait）
        format!("{:.1}M", (v / 1_000_000.0) as f32)
    } else if abs >= 10_000.0 {
        format!("{}k", (v / 1000.0).round() as i64)
    } else if abs >= 1000.0 {
        format!("{:.1}k", (v / 1000.0) as f32)
    } else {
        format!("{}", v.round() as i64)
    }
}

/// 在 32x32 上画两行居中文字。font 缺失则 noop。
fn draw_two_line_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    top: &str,
    bottom: &str,
    color: Rgba<u8>,
) {
    let Some(font) = load_font() else { return };
    let scale = PxScale::from(12.0);
    let scaled = font.as_scaled(scale);
    let w = ICON_SIZE as f32;

    // 测量 + 居中
    let top_w: f32 = top
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum();
    let top_x = ((w - top_w) / 2.0).max(1.0) as i32;
    draw_text_mut(img, color, top_x, 13, scale, font, top);

    let bot_w: f32 = bottom
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum();
    let bot_x = ((w - bot_w) / 2.0).max(1.0) as i32;
    draw_text_mut(img, color, bot_x, 27, scale, font, bottom);
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
