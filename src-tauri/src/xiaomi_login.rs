//! Xiaomi MiMo Cookie 一键提取 —— 应用内 WebView 登录
//!
//! 用户在设置面板点 "🔑 登录小米账号" → 弹一个 webview 窗口 → 用户
//! 在 webview 里正常登录小米账号 → 后端监听 URL 变化，登录完成后
//! 调 `webview.cookies_for_url()` 提取 dashboard 相关的 cookie → 拼成
//! `Cookie:` header 字符串 → 写进 keys.json → 关 webview → emit
//! `musage://xiaomi-login-success` 事件。
//!
//! ## 设计要点
//!
//! - **不走 DevTools**：cookie 始终在 webview 自己的 cookie jar 里（加密
//!   内存），不需要复制到剪贴板（剪贴板是公开 API，其他 app 能读）
//! - **不依赖外部扩展**：复用现有 Tauri 2 webview 能力，0 新增依赖
//! - **跨平台同代码**：Mac/Win/Linux 都是同一套（Tauri runtime 适配）
//!
//! ## 登录完成启发式
//!
//! Xiaomi SSO 流程：未登录 → 重定向到 `account.xiaomi.com/.../serviceLogin`
//! → 用户登录 → 重定向回 `platform.xiaomimimo.com/console/...`。
//! 判定"登录完成"的最小规则：URL 命中 `platform.xiaomimimo.com` **且**
//! 不在 `account.xiaomi.com` / `serviceLogin` / `passport` 路径上。
//!
//! ## Cookie 白名单
//!
//! 不在白名单里的 cookie 一律丢弃（最小权限）。
//! dashboard API 实际依赖的就这 4 个（参考 `providers/xiaomi.rs` 的注释）。
//! 平台如果改名 → 改这里就行，UI 不变。

use std::time::Duration;

use tauri::webview::Cookie;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::time::sleep;

use crate::config;
use crate::providers::Credentials;

/// 登录入口 URL。直接定位到 dashboard 的"订阅管理"页。
const LOGIN_URL: &str = "https://platform.xiaomimimo.com/console/plan-manage";

/// 判定 URL 是否已经离开 SSO 重定向链、到达 dashboard。
///
/// 规则（白名单 + 黑名单组合）：
/// - host 必须是 `platform.xiaomimimo.com`
/// - 不能在 `account.xiaomi.com` / `serviceLogin` / `passport` 路径上
///
/// 这是 heuristic，不是绝对 —— 如果 Xiaomi 改了 SSO 流程（比如加一层
/// 验证中间页），要改这里或加新关键字。
fn is_dashboard_url(url: &Url) -> bool {
    let s = url.as_str();
    let host_ok = s.contains("platform.xiaomimimo.com");
    let not_login = !s.contains("account.xiaomi.com")
        && !s.contains("serviceLogin")
        && !s.contains("passport");
    host_ok && not_login
}

/// dashboard API 实际依赖的 cookie name 集合。不在白名单的丢弃（最小权限）。
const WANTED_COOKIES: &[&str] = &[
    "api-platform_serviceToken",
    "userId",
    "api-platform_slh",
    "api-platform_ph",
];

/// 打开登录 webview 窗口。
///
/// 行为：
/// 1. 如果已有 `xiaomi-login` 窗口（用户再次点按钮），先关掉
/// 2. 开新 webview 指向 `LOGIN_URL`
/// 3. 监听 `on_page_load`：URL 命中 dashboard 启发式 → 等 1s 让
///    cookies 落定 → 二次校验 URL → 提取 → 保存 → 关闭 → emit
///
/// 错误通过 `musage://xiaomi-login-failed` 事件返回给前端。
#[tauri::command]
pub async fn open_xiaomi_login_window(app: AppHandle) -> Result<(), String> {
    // 已开过 → 先关（重新登录场景）
    if let Some(existing) = app.get_webview_window("xiaomi-login") {
        let _ = existing.close();
        sleep(Duration::from_millis(100)).await;
    }

    let url: Url = LOGIN_URL
        .parse()
        .map_err(|e| format!("parse LOGIN_URL: {e}"))?;

    // 闭包必须 'static + Send + Sync → 克隆 AppHandle（内部 Arc 包装，廉价）
    let app_for_callback = app.clone();

    WebviewWindowBuilder::new(&app, "xiaomi-login", WebviewUrl::External(url))
        .title("登录小米账号 - Musage")
        .inner_size(960.0, 720.0)
        .min_inner_size(640.0, 540.0)
        .resizable(true)
        .decorations(true)
        .center()
        .on_page_load(move |window, payload| {
            let url = payload.url();
            tracing::debug!(%url, "xiaomi login webview page load");
            if !is_dashboard_url(url) {
                return;
            }
            // 看起来到 dashboard 了 —— 等 1s 让 set-cookie 落定
            let app2 = app_for_callback.clone();
            let window_clone = window.clone();
            tauri::async_runtime::spawn(async move {
                sleep(Duration::from_secs(1)).await;

                // 二次校验：1s 内 URL 跳回 SSO 的话就别提了
                let current_url = match window_clone.url() {
                    Ok(u) => u,
                    Err(e) => {
                        emit_failed(&app2, format!("读 webview URL 失败: {e}"));
                        return;
                    }
                };
                if !is_dashboard_url(&current_url) {
                    tracing::debug!(%current_url, "二次校验：URL 又跳走了，不提取 cookie");
                    return;
                }

                match extract_and_save(&window_clone).await {
                    Ok(saved_len) => {
                        tracing::info!(saved_len, "xiaomi cookie 提取 + 保存成功");
                        // 立即拉一次（让浮窗立刻看到数据）
                        if let Err(e) =
                            crate::commands::refresh_single_inner(&app2, "xiaomimimo").await
                        {
                            tracing::warn!(error = %e, "登录后立即拉取失败（不阻塞成功事件）");
                        }
                        // 关 webview
                        let _ = window_clone.close();
                        // 通知前端
                        let _ = app2.emit("musage://xiaomi-login-success", saved_len);
                    }
                    Err(e) => emit_failed(&app2, e),
                }
            });
        })
        .build()
        .map_err(|e| format!("build webview: {e}"))?;

    Ok(())
}

/// 从 webview 提取 cookie → 过滤白名单 → 拼字符串 → 写 keys.json。
///
/// 返回写入的字节数（便于前端展示"已保存 N 字节"）。
///
/// 注意：返回的 cookie 字符串里 `serviceToken` 的 value 是 raw value
/// （没有 DevTools 显示时加的双引号）。`Cookie:` HTTP header 期望的也是
/// raw value，所以直接拼即可，不需要再加引号。
async fn extract_and_save(window: &tauri::WebviewWindow) -> Result<usize, String> {
    let url: Url = LOGIN_URL
        .parse()
        .map_err(|e| format!("parse url: {e}"))?;
    // cookies_for_url：拿指定 URL 上下文下的 cookies（含 HttpOnly，
    // 这正是我们需要的 —— 普通 document.cookie 读不到 HttpOnly）
    let raw_cookies: Vec<Cookie<'static>> = window
        .cookies_for_url(url)
        .map_err(|e| format!("webview.cookies_for_url 失败: {e}"))?;

    let relevant: Vec<&Cookie<'static>> = raw_cookies
        .iter()
        .filter(|c| WANTED_COOKIES.contains(&c.name()))
        .collect();

    if relevant.is_empty() {
        return Err(format!(
            "没找到 Xiaomi dashboard cookies（共 {} 个 cookie，白名单期望 {} 个：{:?}）。可能：\
            1) dashboard 还没完整加载（等几秒重试）；\
            2) 平台改了 cookie 命名（要改 WANTED_COOKIES 白名单）；\
            3) 你登的不是 platform.xiaomimimo.com 而是子账号。",
            raw_cookies.len(),
            WANTED_COOKIES.len(),
            WANTED_COOKIES
        ));
    }

    let cookie_str: String = relevant
        .iter()
        .map(|c| format!("{}={}", c.name(), c.value()))
        .collect::<Vec<_>>()
        .join("; ");

    let cred = Credentials {
        api_key: None,
        cookie: Some(cookie_str.clone()),
    };
    config::save_credential_for_id("xiaomimimo", &cred)
        .map_err(|e| format!("写 keys.json 失败: {e}"))?;

    Ok(cookie_str.len())
}

fn emit_failed(app: &AppHandle, msg: String) {
    tracing::error!(error = %msg, "xiaomi login flow failed");
    let _ = app.emit("musage://xiaomi-login-failed", msg);
}

// ── 单元测试（pure function） ───────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().expect("parse test url")
    }

    #[test]
    fn dashboard_url_basic() {
        assert!(is_dashboard_url(&url(
            "https://platform.xiaomimimo.com/console/plan-manage"
        )));
        assert!(is_dashboard_url(&url(
            "https://platform.xiaomimimo.com/console/plan-manage?userId=12345"
        )));
        assert!(is_dashboard_url(&url(
            "https://platform.xiaomimimo.com/api/v1/tokenPlan/usage"
        )));
    }

    #[test]
    fn dashboard_url_rejects_sso_redirects() {
        // account.xiaomi.com SSO
        assert!(!is_dashboard_url(&url(
            "https://account.xiaomi.com/passport/login?sid=api-platform"
        )));
        // serviceLogin 关键字
        assert!(!is_dashboard_url(&url(
            "https://account.xiaomi.com/serviceLogin?followup=..."
        )));
        // 第三方 SSO 跳板
        assert!(!is_dashboard_url(&url(
            "https://passport.xiaomi.com/..."
        )));
    }

    #[test]
    fn dashboard_url_rejects_unrelated_hosts() {
        assert!(!is_dashboard_url(&url("https://example.com/console")));
        assert!(!is_dashboard_url(&url("https://mimo.xiaomi.com/")));
        // mimo.xiaomi.com 是品牌主页，**不**是 dashboard（虽然名字相似）
    }

    #[test]
    fn wanted_cookies_list_is_non_empty() {
        // 防御性检查：白名单不能被改空
        assert!(!WANTED_COOKIES.is_empty());
        assert!(WANTED_COOKIES.len() >= 2, "白名单至少 2 项才合理");
    }
}
