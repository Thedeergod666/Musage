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
//! ## 并发控制
//!
//! `on_page_load` 在 macOS WKWebView 上会多次触发（SSO 回调链 + 页面内
//! 导航），每次触发都会 spawn 异步任务。用 `AtomicBool` 保证同一时间只有
//! 一个提取任务在运行，后续触发直接跳过，避免多任务竞争同一个 webview
//! 窗口导致 "failed to receive message from webview" 错误。
//!
//! H3 fix: 任务 panic 时 EXTRACTING 永久留在 true → 用户再点登录按钮后
//! compare_exchange 永远失败 → 登录永远卡住。修法:用 RAII `ExtractingGuard`
//! 在 future 末尾 Drop 时无条件 reset EXTRACTING。tokio task panic 时 local
//! variables 仍然被 Drop(run by panic unwinding) → guard 兜底。
//!
//! ## Cookie 白名单
//!
//! 不在白名单里的 cookie 一律丢弃（最小权限）。
//! dashboard API 实际依赖的就这 4 个（参考 `providers/xiaomi.rs` 的注释）。
//! 平台如果改名 → 改这里就行，UI 不变。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::webview::Cookie;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::time::sleep;

use crate::config;
use crate::providers::Credentials;
use crate::t;

/// 全局提取锁：防止多个 on_page_load 回调同时运行提取任务。
/// 一旦有任务在提取/等待中，后续回调直接跳过。
static EXTRACTING: AtomicBool = AtomicBool::new(false);

/// 全局完成标记：提取成功后置 true，后续 on_page_load 回调全部跳过。
/// 解决 macOS WKWebView 上 on_page_load 多次触发导致的
/// "failed to receive message from webview" 错误——窗口被第一个成功
/// 任务关闭后，后续回调不再尝试操作已销毁的 webview。
static DONE: AtomicBool = AtomicBool::new(false);

/// RAII guard: Drop 时无条件 reset `EXTRACTING`。
///
/// H3 fix: tokio spawn 的 task panic 时,虽然 tokio 自己会打印 panic 信息 +
/// propagate 给 spawn handle,但我们 spawn 时没 await handle → panic 后
/// task 内部的局部变量仍被 Drop (Rust unwinding 时跑 Drop glue)。把
/// EXTRACTING reset 放在 Drop 里 —— 任意路径退出(正常返回/Err/panic)
/// 都会清掉锁,保证下次用户点登录能 compare_exchange 成功。
struct ExtractingGuard;

impl Drop for ExtractingGuard {
    fn drop(&mut self) {
        EXTRACTING.store(false, Ordering::SeqCst);
    }
}

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
    let not_login =
        !s.contains("account.xiaomi.com") && !s.contains("serviceLogin") && !s.contains("passport");
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
/// 3. 监听 `on_page_load`：URL 命中 dashboard 启发式 → 等待 + 重试提取
///    cookie → 保存 → 关闭 → emit 成功事件
///
/// macOS WKWebView 上 `on_page_load` 会多次触发（SSO 重定向链 +
/// 页面内导航），用 `EXTRACTING` 保证只有一个任务在提取。
///
/// 错误通过 `musage://xiaomi-login-failed` 事件返回给前端。
#[tauri::command]
pub async fn open_xiaomi_login_window(app: AppHandle) -> Result<(), String> {
    // 重置提取锁 + 完成标记（新窗口 = 全新流程）
    EXTRACTING.store(false, Ordering::SeqCst);
    DONE.store(false, Ordering::SeqCst);

    // 已开过 → 先关（重新登录场景）
    if let Some(existing) = app.get_webview_window("xiaomi-login") {
        let _ = existing.close();
        sleep(Duration::from_millis(100)).await;
    }

    let url: Url = (LOGIN_URL.parse::<Url>())
        .map_err(|e| t!("xiaomi_login.parse_login_url", err = e.to_string()).into_owned())?;

    // 闭包必须 'static + Send + Sync → 克隆 AppHandle（内部 Arc 包装，廉价）
    let app_for_callback = app.clone();

    WebviewWindowBuilder::new(&app, "xiaomi-login", WebviewUrl::External(url))
        .title(t!("window.xiaomi_login").to_string())
        .inner_size(960.0, 720.0)
        .min_inner_size(640.0, 540.0)
        .resizable(true)
        .decorations(true)
        .center()
        .on_page_load(move |window, payload| {
            let url = payload.url();
            tracing::debug!(%url, "xiaomi login webview page load");

            // 提取已完成（或正在运行）→ 全部跳过，不再操作 webview
            if DONE.load(Ordering::SeqCst) {
                return;
            }

            if !is_dashboard_url(url) {
                return;
            }

            // 并发锁：已有任务在跑就跳过
            if EXTRACTING
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                tracing::debug!("on_page_load: 已有提取任务在运行，跳过");
                return;
            }

            tracing::info!(%url, "on_page_load: ✅ 命中 dashboard，启动 cookie 提取");
            let app2 = app_for_callback.clone();
            let window_clone = window.clone();
            tauri::async_runtime::spawn(async move {
                // H3 fix: 用 RAII guard 兜底 —— spawn 的 task panic 时
                // Rust 仍会跑局部变量的 Drop glue (除非 panic = abort 但
                // tokio 默认 unwind)。guard 在任意路径退出(正常返回/Err/panic)
                // 都会被 Drop,强制 reset EXTRACTING,保证下次用户点登录
                // compare_exchange 永远能成功。
                let _extracting_guard = ExtractingGuard;
                let result = extract_with_retry(&window_clone, &app2).await;
                // 显式 store 保留显式锁语义给阅读者,guard 是 panic 兜底。
                EXTRACTING.store(false, Ordering::SeqCst);

                match result {
                    Ok(saved_len) => {
                        DONE.store(true, Ordering::SeqCst);
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
                    Err(e) => {
                        // 只有 DONE 为 false 时才报错（避免关闭后的残留任务触发误报）
                        if !DONE.load(Ordering::SeqCst) {
                            emit_failed(&app2, e);
                        }
                    }
                }
            });
        })
        .build()
        .map_err(|e| t!("xiaomi_login.build_webview", err = e.to_string()).into_owned())?;

    Ok(())
}

/// 带重试的 cookie 提取。最多尝试 5 次，间隔递增。
///
/// macOS WKWebView 的 cookie store 可能延迟写入（SSO 回调链中
/// `on_page_load` 触发时 cookie 还没落定），所以需要多次尝试。
async fn extract_with_retry(
    window: &tauri::WebviewWindow,
    _app: &AppHandle,
) -> Result<usize, String> {
    // 重试策略：1s, 2s, 2s, 3s, 3s（共 11s 覆盖大部分场景）
    let retry_delays = [1u64, 2, 2, 3, 3];

    for (attempt, delay) in retry_delays.iter().enumerate() {
        let attempt_num = attempt + 1;

        // 如果另一个任务已经成功，直接退出
        if DONE.load(Ordering::SeqCst) {
            return Err(t!("xiaomi_login.another_task_done").into_owned());
        }

        sleep(Duration::from_secs(*delay)).await;

        // 检查 URL 是否还在 dashboard
        let current_url = match window.url() {
            Ok(u) => u,
            Err(e) => {
                // webview 可能已被成功的任务关闭（"failed to receive message"），
                // 这是预期行为，直接退出
                tracing::debug!(error = %e, attempt_num, "读 webview URL 失败（窗口可能已关闭）");
                return Err(t!("xiaomi_login.read_url_failed", err = e.to_string()).into_owned());
            }
        };

        if !is_dashboard_url(&current_url) {
            tracing::debug!(%current_url, attempt_num, "URL 不在 dashboard，跳过");
            continue;
        }

        // 尝试提取
        match extract_and_save(window).await {
            Ok(saved_len) => {
                tracing::info!(saved_len, attempt_num, "cookie 提取成功");
                return Ok(saved_len);
            }
            Err(e) => {
                tracing::debug!(error = %e, attempt_num, "cookie 提取失败，继续重试");
            }
        }
    }

    // 所有重试都失败
    Err(t!("xiaomi_login.cookie_extraction_failed").into_owned())
}

/// 从 webview 提取 cookie → 过滤白名单 → 拼字符串 → 写 keys.json。
///
/// 返回写入的字节数（便于前端展示"已保存 N 字节"）。
async fn extract_and_save(window: &tauri::WebviewWindow) -> Result<usize, String> {
    let url: Url = (LOGIN_URL.parse::<Url>())
        .map_err(|e| t!("xiaomi_login.parse_url", err = e.to_string()).into_owned())?;

    // cookies_for_url：拿指定 URL 上下文下的 cookies（含 HttpOnly，
    // 这正是我们需要的 —— 普通 document.cookie 读不到 HttpOnly）
    let raw_cookies: Vec<Cookie<'static>> = window
        .cookies_for_url(url)
        .map_err(|e| t!("xiaomi_login.cookies_for_url_failed", err = e.to_string()).into_owned())?;

    tracing::debug!(total = raw_cookies.len(), "cookies_for_url 返回");

    let relevant: Vec<&Cookie<'static>> = raw_cookies
        .iter()
        .filter(|c| WANTED_COOKIES.contains(&c.name()))
        .collect();

    if relevant.is_empty() {
        let available: Vec<String> = raw_cookies
            .iter()
            .map(|c| {
                format!(
                    "{} (domain={}, secure={}, httpOnly={})",
                    c.name(),
                    c.domain().unwrap_or("?"),
                    c.secure().map_or("?".to_string(), |b| b.to_string()),
                    c.http_only().map_or("?".to_string(), |b| b.to_string()),
                )
            })
            .collect();
        return Err(t!(
            "xiaomi_login.cookies_not_found",
            count = raw_cookies.len(),
            expected = WANTED_COOKIES.len(),
            wanted = format!("{WANTED_COOKIES:?}"),
            available = format!("{available:?}")
        )
        .into_owned());
    }

    // F4 fix: require at least `api-platform_serviceToken` AND `userId` to be present
    // (these are the auth-critical ones per providers/xiaomi.rs comment). Before this,
    // extraction accepted any non-empty subset, so a stale extraction that only got
    // `userId` (1/4 cookies) would silently overwrite a valid saved cookie with junk.
    let mut cookie_parts: Vec<String> = relevant
        .iter()
        .map(|c| {
            // macOS WKWebView 的 cookie store 可能在 value 外层包双引号
            // （如 `"tokenvalue"`），Cookie: HTTP header 期望 raw value，
            // 需要去掉。
            let val = c.value().trim_matches('"');
            format!("{}={}", c.name(), val)
        })
        .collect();

    // macOS WKWebView 的 cookie store 可能不包含 `userId` cookie（它可能
    // 是由 JS 设置的或域名不同）。但 userId 会出现在 dashboard URL 的
    // 查询参数里（`?userId=12345`），从中提取并补充到 cookie 字符串。
    // API 需要 userId 才能返回 200。
    let has_user_id = cookie_parts.iter().any(|p| p.starts_with("userId="));
    if !has_user_id {
        if let Ok(current_url) = window.url() {
            if let Some(uid) = extract_user_id_from_url(&current_url) {
                tracing::info!(userId = %uid, "从 URL 参数补充 userId 到 cookie");
                cookie_parts.push(format!("userId={uid}"));
            }
        }
    }

    // F4 fix: 在写入 keys.json 前做完整性校验。两个 auth-critical cookie 必须同时存在：
    // - api-platform_serviceToken：真正的认证 token
    // - userId：dashboard API 路由参数
    // 任何一个缺失就 return Err，不覆盖原有的有效 cookie（避免用户被锁在"看似登录了但 API 401"的状态）。
    let has_service_token = cookie_parts
        .iter()
        .any(|p| p.starts_with("api-platform_serviceToken="));
    let has_user_id = cookie_parts.iter().any(|p| p.starts_with("userId="));
    if !(has_service_token && has_user_id) {
        tracing::error!(
            has_service_token,
            has_user_id,
            got = ?cookie_parts.iter().map(|p| p.split('=').next().unwrap_or("?")).collect::<Vec<_>>(),
            "cookie 不完整 (缺 api-platform_serviceToken 或 userId)，不写入"
        );
        return Err(t!(
            "xiaomi_login.cookies_incomplete",
            has_service_token = has_service_token,
            has_user_id = has_user_id
        )
        .into_owned());
    }

    let cookie_str = cookie_parts.join("; ");

    let cred = Credentials {
        api_key: None,
        cookie: Some(cookie_str.clone()),
    };
    config::save_credential_for_id("xiaomimimo", &cred)
        .map_err(|e| t!("xiaomi_login.save_keys_failed", err = e.to_string()).into_owned())?;

    Ok(cookie_str.len())
}

fn emit_failed(app: &AppHandle, msg: String) {
    tracing::error!(error = %msg, "xiaomi login flow failed");
    let _ = app.emit("musage://xiaomi-login-failed", msg);
}

/// 从 URL 查询参数中提取 `userId`。
/// dashboard URL 格式：`...?userId=12345&...` 或 `...?...&userId=12345`
fn extract_user_id_from_url(url: &Url) -> Option<String> {
    for (key, value) in url.query_pairs() {
        if key == "userId" {
            return Some(value.into_owned());
        }
    }
    None
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
        assert!(!is_dashboard_url(&url("https://passport.xiaomi.com/...")));
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
