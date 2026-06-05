//! 系统托盘动态图标生成
//!
//! 渲染规则：
//! - 16x16 RGBA
//! - 圆形背景：颜色随 5h utilization 变化（绿/橙/红）
//! - 中心文字：5h 已用百分比（缩写）
//! - 托盘 tooltip：完整状态

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

// 字体加载（如果 assets/font.ttf 缺失则跳过文字，纯色圆点）
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::Rgba;
use imageproc::drawing::draw_text_mut;
use std::sync::OnceLock;

static FONT: OnceLock<Option<FontVec>> = OnceLock::new();

fn load_font() -> Option<&'static FontVec> {
    FONT.get_or_init(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/font.ttf");
        std::fs::read(&path).ok().and_then(|bytes| FontVec::try_from_vec(bytes).ok())
    }).as_ref()
}

use crate::api::QuotaSnapshot;

const ICON_SIZE: u32 = 32; // tray 标准 32x32，更清晰

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "show", "显示悬浮窗", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)?;
    let refresh_i = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &settings_i, &refresh_i, &quit_i])?;

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
            "settings" => {
                let _ = app.emit_to("settings", "musage://open", ());
                // 后续可在这里显式创建 settings 窗口
            }
            "refresh" => {
                let app2 = app.clone();
                tokio::spawn(async move {
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
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
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
    let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));
    let center = ICON_SIZE as i32 / 2;
    let r2 = (center - 2).pow(2);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as i32 - center;
            let dy = y as i32 - center;
            if dx * dx + dy * dy <= r2 {
                img.put_pixel(x, y, Rgba([128, 128, 128, 255]));
            }
        }
    }
    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

fn render_icon(snap: &QuotaSnapshot) -> Image<'static> {
    let mut img: image::ImageBuffer<Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_fn(ICON_SIZE, ICON_SIZE, |_x, _y| Rgba([0, 0, 0, 0]));
    let center = ICON_SIZE as i32 / 2;
    let radius = center - 1;
    let r2 = radius * radius;

    let color = match snap.to_health_label() {
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

    // 中心文字：百分比
    let text_opt = if snap.success {
        snap.five_hour.as_ref().map(|t| format_pct_short(t.utilization))
    } else {
        Some("?".to_string())
    };
    if let Some(t) = text_opt {
        draw_centered_text(&mut img, &t, Rgba([255, 255, 255, 255]));
    }

    let (w, h) = img.dimensions();
    Image::new_owned(img.into_raw(), w, h)
}

fn format_pct_short(v: f64) -> String {
    let r = v.round() as i64;
    if r >= 100 {
        // 超过 100 简化为 "1+"（如 144% → "1+", 280% → "2+"）
        format!("{}x", (r / 100).max(1))
    } else if r < 0 {
        "0".to_string()
    } else {
        r.to_string()
    }
}

fn draw_centered_text(
    img: &mut image::ImageBuffer<Rgba<u8>, Vec<u8>>,
    text: &str,
    color: Rgba<u8>,
) {
    let Some(font) = load_font() else { return; };
    let scale = PxScale::from(20.0);
    let scaled = font.as_scaled(scale);
    let text_w: f32 = text.chars().map(|c| scaled.h_advance(font.glyph_id(c))).sum();
    let w = ICON_SIZE as f32;
    let h = ICON_SIZE as f32;
    let x = ((w - text_w) / 2.0).max(2.0) as i32;
    let y = (h - scale.y - 2.0).max(2.0) as i32;
    draw_text_mut(img, color, x, y, scale, font, text);
}

fn tooltip(snap: &QuotaSnapshot) -> String {
    if !snap.success {
        return format!("Musage · {}", snap.error.as_deref().unwrap_or("未知错误"));
    }
    let mut parts = vec!["Musage".to_string()];
    if let Some(t) = &snap.five_hour {
        parts.push(format!("5h: {}%", t.utilization.round() as i64));
    }
    if let Some(t) = &snap.weekly {
        parts.push(format!("周: {}%", t.utilization.round() as i64));
    }
    if let Some(ms) = snap.fetched_at {
        let dt = chrono::DateTime::from_timestamp_millis(ms)
            .map(|d| d.format("%H:%M:%S").to_string())
            .unwrap_or_default();
        parts.push(format!("更新于 {dt}"));
    }
    parts.join(" · ")
}
