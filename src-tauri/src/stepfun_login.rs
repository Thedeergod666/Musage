//! StepFun Oasis-Token 一键提取 —— 应用内 WebView 登录
//!
//! 用户在设置面板点 "🔑 登录 StepFun" → 弹一个 webview 窗口加载
//! `account.stepfun.com/login`（带 redirect 参数）→ 用户在 webview 里
//! 正常登录（或已存 SSO session 时自动跳转）→ 登录完成后
//! `platform.stepfun.com` 域落下 `Oasis-Token` cookie → 后端轮询
//! `cookies_for_url(PLATFORM_URL)` 抽到 token → 写进 keys.json →
//! 关 webview → emit `musage://stepfun-login-success` 事件。
//!
//! ## 2026-07-28 重写（修「永远抓不到 token」）
//!
//! 第一版（2026-07-27）仿 xiaomi 用 `on_page_load` + `is_dashboard_url`
//! 触发提取，实测窗口停在 dashboard 却永远抓不到 token。四个叠加 bug：
//!
//! 1. **probe URL 域错了（致命）**：`extract_and_save` 用 `LOGIN_URL`
//!    （`account.stepfun.com`）调 `cookies_for_url`，但 `Oasis-Token`
//!    落在 **platform.stepfun.com** 域。`cookies_for_url` 按域过滤，
//!    account 域的 cookie jar 里永远不会有它 → 100% 提取失败。
//!    （xiaomi 没踩这个坑是因为它的 LOGIN_URL 和 cookie 同域。）
//! 2. **init script 会删掉新 token**：document_start 在 platform 域
//!    无差别 `max-age=0` 清 `Oasis-Token` —— 登录后跳回 platform 的
//!    首次页面加载也会跑这段，把刚 Set-Cookie 的新 token 立刻删掉
//!    （非 HttpOnly 时）。
//! 3. **init script 可能破坏登录本身**：`Storage.getItem` 锁在非
//!    platform 域一律返 null，account 域登录 SPA 若用 localStorage
//!    存 OIDC state，登录流程直接坏掉。
//! 4. **`clear_all_browsing_data` 定时竞态**：开窗后 200ms 全清 cookie
//!    store —— 已登录用户 SSO 秒跳时新 token 可能在 clear 之前落定，
//!    被一起清掉；同时也把「SSO 自动重登」这条零交互路径杀死了。
//!
//! ## 现行设计（对齐 anysearch_login.rs）
//!
//! - **独立轮询任务**（不依赖 `on_page_load` —— WKWebView 上触发时机
//!   不稳定，且 SPA 客户端跳转根本不 fire）：每 700ms 读一次
//!   `cookies_for_url(PLATFORM_URL)`，上限 ~14 分钟（覆盖手动手机号+
//!   验证码登录）。
//! - **JWT exp 新鲜度门**替代 READY 握手 + 清 cookie：重新登录的触发
//!   场景就是旧 token 已过期（浮窗报 `token_expired_hint`），所以
//!   cookie jar 里的残留 token 一定解出过期的 exp → 拒绝、继续等；
//!   用户登录后的新 token exp 在未来 → 接受。复用 provider 侧同一个
//!   [`access_token_exp_seconds_ago`]，保证「登录存下来的 token」和
//!   「provider 预检接受的 token」判定一致。
//! - **不清 browsing data**：保留 webview profile 里的 account 域 SSO
//!   session，token 过期后重新登录可走「点按钮 → SSO 秒跳 → 抓新
//!   token → 关窗」零交互路径。
//! - **combined token**：`Oasis-Token` 值本身是 `<access>...<refresh>`
//!   就直接存；若是单段 access 且存在独立的 `Oasis-Refresh-Token`
//!   cookie，拼成 `<access>...<refresh>` 再存（CodexBar combinedToken
//!   约定；refresh 半段带 `device_id` claim，provider 用它做
//!   `Oasis-Webid` 请求头，缺了 dashboard 端点一律 401）。
//!
//! ## 并发 / 重入
//!
//! 用户重复点按钮 → 先关旧窗口（旧轮询任务因窗口句柄消失 /
//! `cookies_for_url` 报错自然退出）→ 开新窗口 + 新任务。`DONE` 标记
//! 保证成功后残留任务不再写盘。
//!
//! ## 已知取舍
//!
//! - 不做「切换账号」支持：旧 token 仍有效时点重新登录会直接抓走旧
//!   token 并关窗（结果正确 —— 同账号有效 token）。要换账号需在
//!   系统浏览器里先登出 StepFun，或等 token 过期后再重登。
//! - 用户主动关窗 / 超时未登录 → 静默退出，不弹错误条（跟 anysearch
//!   一致；只有写盘失败这类真错误才 emit `-failed`）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::webview::Cookie;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::time::sleep;

use crate::config;
use crate::providers::stepfun::access_token_exp_seconds_ago;
use crate::providers::Credentials;
use crate::t;

/// 全局完成标记：提取成功后置 true，轮询任务退出。
static DONE: AtomicBool = AtomicBool::new(false);

/// 登录入口 URL。直接跳到 account.stepfun.com 登录页（携带 redirect
/// 让登录后自动跳回 platform.stepfun.com，OIDC 风格的跨域 cookie 传递）。
///
/// **为什么不在 platform.stepfun.com/ 根路径**：根路径未登录时显示的
/// 是公开首页内容，用户必须手动点右上角入口才能跳到登录页 → 多一次
/// 点击。直接打登录页，已登录（SSO session 还在）时也会自动 302 回
/// platform 域，零交互完成重登。
const LOGIN_URL: &str = "https://account.stepfun.com/login?redirect=https%3A%2F%2Fplatform.stepfun.com%2F&source_app=platform-cn";

/// `cookies_for_url` 的探测 URL。**必须是 platform 域** —— `Oasis-Token`
/// cookie 落在 platform.stepfun.com，`cookies_for_url` 按域过滤；用
/// account 域 URL 探测永远拿不到（第一版的致命 bug）。
const PLATFORM_URL: &str = "https://platform.stepfun.com/";

/// webview 窗口 label（capability 按此授权 create-webview-window）。
const WINDOW_LABEL: &str = "stepfun-login";

/// 目标 cookie 名。`Oasis-Token` 是 auth-critical；`Oasis-Refresh-Token`
/// 仅在 token 单段时用于拼 combined token。
const TOKEN_COOKIE: &str = "Oasis-Token";
const REFRESH_COOKIE: &str = "Oasis-Refresh-Token";

/// 打开登录 webview 窗口。
///
/// 行为：
/// 1. 已有 `stepfun-login` 窗口 → 先关（重新登录场景）
/// 2. 开新 webview 指向 `LOGIN_URL`
/// 3. spawn 轮询任务：读 platform 域 cookie jar，见到**新鲜**（exp 未过）
///    `Oasis-Token` → 拼 combined → 写 keys.json → 关窗口 → emit 成功
///
/// 错误（仅“写盘失败”这类真错误）通过 `musage://stepfun-login-failed`
/// 返回前端。用户主动关窗 / 没登录 / 超时 → 静默退出，不弹红条。
#[tauri::command]
pub async fn open_stepfun_login_window(app: AppHandle) -> Result<(), String> {
    DONE.store(false, Ordering::SeqCst);

    // 已开过 → 先关（旧轮询任务会因窗口句柄消失自然退出）
    if let Some(existing) = app.get_webview_window(WINDOW_LABEL) {
        let _ = existing.close();
        sleep(Duration::from_millis(100)).await;
    }

    let url: Url = LOGIN_URL
        .parse::<Url>()
        .map_err(|e| t!("stepfun_login.parse_login_url", err = e.to_string()).into_owned())?;

    let window = WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::External(url))
        .title(t!("window.stepfun_login").to_string())
        .inner_size(960.0, 720.0)
        .min_inner_size(640.0, 540.0)
        .resizable(true)
        .decorations(true)
        .center()
        .build()
        .map_err(|e| t!("stepfun_login.build_webview", err = e.to_string()).into_owned())?;

    // 轮询任务：读 platform 域 cookie jar 抽 Oasis-Token
    let app2 = app.clone();
    let window_clone = window.clone();
    tauri::async_runtime::spawn(async move {
        let result = poll_token_from_cookie(&app2, &window_clone).await;
        match result {
            PollOutcome::Saved(len) => {
                DONE.store(true, Ordering::SeqCst);
                tracing::info!(len, "stepfun cookie 提取 + 保存成功");
                // 立即拉一次（让浮窗立刻看到数据）
                if let Err(e) = crate::commands::refresh_single_inner(&app2, "stepfun").await {
                    tracing::warn!(error = %e, "登录后立即拉取失败（不阻塞成功事件）");
                }
                let _ = window_clone.close();
                let _ = app2.emit("musage://stepfun-login-success", len);
            }
            // 用户关窗 / 没登录 / 超时 —— 静默退出，不弹红条
            PollOutcome::Cancelled => {
                tracing::debug!("stepfun 登录窗口已关闭或超时，未提取到 token");
            }
            PollOutcome::Failed(e) => {
                if !DONE.load(Ordering::SeqCst) {
                    tracing::error!(error = %e, "stepfun login flow failed");
                    let _ = app2.emit("musage://stepfun-login-failed", e);
                }
            }
        }
    });

    Ok(())
}

enum PollOutcome {
    Saved(usize),
    Cancelled,
    Failed(String),
}

/// 轮询 webview cookie jar 直到抽到**新鲜**的 Oasis-Token 或窗口消失 / 超时。
///
/// 用 [`tauri::WebviewWindow::cookies_for_url`] 读 cookie jar（含 HttpOnly，
/// 跟 xiaomi / anysearch 同一套机制，跨平台稳定）。probe URL 固定为
/// [`PLATFORM_URL`]，跟当前页面停在哪个域无关（cookie store 是
/// webview profile 级的，按目标 URL 域过滤）。
///
/// 「新鲜度门」：旧会话残留的 token 一定已过期（用户正是因为它过期
/// 才点重新登录），本地解 JWT exp 直接拒掉、继续轮询；用户登录后
/// Set-Cookie 的新 token exp 在未来才接受。这样不需要 init script
/// 清 cookie / READY 握手 / clear_all_browsing_data 这些带竞态的机制。
async fn poll_token_from_cookie(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
) -> PollOutcome {
    // 安全上限：~14 分钟（1200 × 700ms），覆盖手动手机号 + 验证码登录；
    // 防窗口句柄异常残留时任务永不退出。
    const MAX_ITERS: u32 = 1200;
    let probe_url: Url = PLATFORM_URL
        .parse()
        .unwrap_or_else(|_| Url::parse("https://platform.stepfun.com/").expect("hardcoded URL parses"));

    // 首次读取前让出 1s：等窗口首个导航开始、cookie store 可用。
    sleep(Duration::from_millis(1000)).await;
    if DONE.load(Ordering::SeqCst) || app.get_webview_window(WINDOW_LABEL).is_none() {
        return PollOutcome::Cancelled;
    }

    for _ in 0..MAX_ITERS {
        if DONE.load(Ordering::SeqCst) {
            return PollOutcome::Cancelled;
        }
        // 窗口已被关 → 用户取消
        if app.get_webview_window(WINDOW_LABEL).is_none() {
            return PollOutcome::Cancelled;
        }

        let cookies: Vec<Cookie<'static>> = match window.cookies_for_url(probe_url.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "读 webview cookies_for_url 失败（窗口可能已关闭）");
                return PollOutcome::Cancelled;
            }
        };

        if let Some(tok) = cookies.iter().find(|c| c.name() == TOKEN_COOKIE) {
            // cookie value 可能带引号（macOS WKWebView 习惯），剥掉
            let access = tok.value().trim_matches('"');
            if is_fresh_token(access) {
                let refresh = cookies
                    .iter()
                    .find(|c| c.name() == REFRESH_COOKIE)
                    .map(|c| c.value().trim_matches('"'))
                    .filter(|v| !v.is_empty());
                let combined = combine_token(access, refresh);
                return match save_token(&combined) {
                    Ok(len) => PollOutcome::Saved(len),
                    Err(e) => PollOutcome::Failed(e),
                };
            }
            // token 在但已过期 / 为空 —— 上一次会话的残留，继续等用户登录后的新 token
            tracing::debug!("Oasis-Token 存在但已过期或为空（旧会话残留），继续轮询");
        }
        // 没有 Oasis-Token cookie = 用户还没登录 —— 继续等

        sleep(Duration::from_millis(700)).await;
    }

    tracing::debug!("stepfun 登录轮询达到安全上限，静默退出");
    PollOutcome::Cancelled
}

/// 判断抽到的 Oasis-Token 是否「新鲜可用」。
///
/// - 空串 → 拒绝（等待写入）
/// - JWT 可解且 access 半段 exp 已过期 → 拒绝（旧会话残留，继续等新 token）
/// - JWT 可解且未过期 → 接受
/// - 解不出 exp（非 JWT / 格式变化）→ 放行，交给服务端校验（宁可存一个
///   可能无效的 token 让 provider 报 401，也不让登录流程永远卡死）
fn is_fresh_token(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    match access_token_exp_seconds_ago(value) {
        Some(secs_ago) => secs_ago < 0,
        None => true,
    }
}

/// 拼 combined token（CodexBar `access...refresh` 约定）。
///
/// `Oasis-Token` 值本身已含 `...` 两段 → 原样用；单段 access 且有独立
/// `Oasis-Refresh-Token` cookie → 拼成 `access...refresh`（refresh 半段
/// 的 `device_id` claim 是 provider 组 `Oasis-Webid` 请求头的来源，
/// 缺了 dashboard 端点一律 401）。
fn combine_token(access: &str, refresh: Option<&str>) -> String {
    if access.contains("...") {
        return access.to_string();
    }
    match refresh {
        Some(r) if !r.is_empty() => format!("{access}...{r}"),
        _ => access.to_string(),
    }
}

/// 把抽到的 token 写进 keys.json 的 `stepfun:cookie` 槽位。返回写入字节数。
///
/// 存盘格式 `Oasis-Token=<combined>`：provider 侧
/// `normalize_oasis_token` 会剥掉前缀，跟手动粘贴整段 cookie 的形态一致。
fn save_token(combined: &str) -> Result<usize, String> {
    let cookie_slot = format!("Oasis-Token={combined}");
    let cred = Credentials {
        api_key: None,
        cookie: Some(cookie_slot.clone()),
        secret_key: None,
    };
    config::save_credential_for_id("stepfun", &cred)
        .map_err(|e| t!("stepfun_login.save_keys_failed", err = e.to_string()).into_owned())?;
    Ok(cookie_slot.len())
}

// ── 单元测试（pure function） ───────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use chrono::Utc;

    fn make_jwt_with_claims(claims: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload = URL_SAFE_NO_PAD.encode(claims.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(b"sig");
        format!("{header}.{payload}.{sig}")
    }

    fn fresh_jwt() -> String {
        let exp = Utc::now().timestamp() + 7 * 86400; // 7 天后
        make_jwt_with_claims(&format!(r#"{{"exp":{exp}}}"#))
    }

    fn expired_jwt() -> String {
        let exp = Utc::now().timestamp() - 3600; // 1 小时前
        make_jwt_with_claims(&format!(r#"{{"exp":{exp}}}"#))
    }

    // ── is_fresh_token ──

    #[test]
    fn fresh_token_accepted() {
        assert!(is_fresh_token(&fresh_jwt()));
    }

    #[test]
    fn expired_token_rejected() {
        // 旧会话残留的过期 token 必须拒掉 —— 这是替代 READY 握手的关键门
        assert!(!is_fresh_token(&expired_jwt()));
    }

    #[test]
    fn empty_token_rejected() {
        assert!(!is_fresh_token(""));
    }

    #[test]
    fn non_jwt_passes_through() {
        // 解不出 exp 的格式放行（交给服务端校验），避免登录流程卡死
        assert!(is_fresh_token("opaque-token-value"));
    }

    #[test]
    fn combined_token_uses_access_half() {
        let fresh = fresh_jwt();
        let expired = expired_jwt();
        assert!(is_fresh_token(&format!("{fresh}...{expired}")));
        assert!(!is_fresh_token(&format!("{expired}...{fresh}")));
    }

    // ── combine_token ──

    #[test]
    fn combine_single_with_refresh() {
        let combined = combine_token("aaa.bbb.ccc", Some("ddd.eee.fff"));
        assert_eq!(combined, "aaa.bbb.ccc...ddd.eee.fff");
    }

    #[test]
    fn combine_already_combined_stays() {
        let combined = combine_token("aaa.bbb.ccc...ddd.eee.fff", Some("xxx.yyy.zzz"));
        assert_eq!(combined, "aaa.bbb.ccc...ddd.eee.fff");
    }

    #[test]
    fn combine_without_refresh_returns_single() {
        assert_eq!(combine_token("aaa.bbb.ccc", None), "aaa.bbb.ccc");
        assert_eq!(combine_token("aaa.bbb.ccc", Some("")), "aaa.bbb.ccc");
    }

    #[test]
    fn window_label_matches_capability() {
        // capability 文件里 windows 数组必须含这个 label,否则 webview 拿不到权限
        assert_eq!(WINDOW_LABEL, "stepfun-login");
    }

    #[test]
    fn probe_url_is_platform_domain() {
        // 回归防御：cookies_for_url 按域过滤，probe 必须在 platform 域
        // （2026-07-27 版用 account 域 LOGIN_URL 探测 → 永远抓不到 token）
        let u: Url = PLATFORM_URL.parse().expect("parse probe url");
        assert_eq!(u.host_str(), Some("platform.stepfun.com"));
        assert_eq!(u.scheme(), "https");
    }
}
