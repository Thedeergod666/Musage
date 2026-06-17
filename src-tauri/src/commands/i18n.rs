//! P0 国际化相关的 Tauri commands。
//!
//! 放独立子模块的原因：`#[tauri::command]` proc macro 在 lib.rs 顶层 + 同一
//! module 里和 `tauri::generate_handler!` 配合时会触发 `__cmd__xxx` macro
//! 重复定义的 bug（macro namespace 冲突）。把命令挪到子模块后，namespace
//! 隔离，bug 消失。其它 commands 也都在子模块里（[`super`]），跟项目约定一致。

use tauri::{AppHandle, Emitter, State};

use crate::AppState;

/// 前端调用的"切换语言"命令。
///
/// 流程：rust_i18n 进程内 locale + cfg 持久化 + emit `musage://locale-changed`
/// 事件（让 tray menu + window title 重建）。
///
/// **命名避坑**：`set_app_locale` 而不是 `set_locale` —— `rust_i18n::i18n!()` 宏
/// 会把 crate 自己的 `set_locale` re-export 到当前 scope，跟我们自己的同名
/// `#[tauri::command]` 撞车（proc macro 生成的 `__cmd__set_locale` 重复定义）。
/// 改成 `set_app_locale` 干净避开。
///
/// 错误：locale 字符串不在白名单内（"zh-CN" / "en"）时返 Err；其它情况
/// （persist 失败等）也返 Err 让前端显示。
#[tauri::command]
pub async fn set_app_locale(
    state: State<'_, AppState>,
    app: AppHandle,
    locale: String,
) -> Result<(), String> {
    // 白名单校验 —— 防止前端注入任意 locale 让 rust_i18n 报 "no such locale"
    if !matches!(locale.as_str(), "zh-CN" | "en") {
        return Err(format!("unsupported locale: {locale}（仅支持 zh-CN / en）"));
    }
    rust_i18n::set_locale(&locale);
    {
        let mut cfg = state.config.write().await;
        cfg.locale = locale.clone();
        cfg.save()?;
    }
    // 广播给前端（让 src/i18n/index.ts 重新 render）+ 给自己（tray rebuild listener）
    // M3 fix: emit 失败 log warn，避免静默丢事件（前端保持旧语言直到下次 reload）
    if let Err(e) = app.emit("musage://locale-changed", &locale) {
        tracing::warn!(error = %e, "emit musage://locale-changed 失败，前端可能未刷新");
    }
    Ok(())
}

/// 读当前 locale（前端启动时调一次决定默认语言）。
#[tauri::command]
pub async fn get_app_locale(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.config.read().await.locale.clone())
}
