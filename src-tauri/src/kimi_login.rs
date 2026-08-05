//! Kimi 网页会话一键提取 —— 应用内 WebView 登录（v0.2.5「总套餐」特性）
//!
//! ## 为什么需要这个
//!
//! Kimi「总套餐」（`FEATURE_OMNI` 月度共享额度池）只通过 `www.kimi.com`
//! 网页会话网关暴露，鉴权要 `kimi-auth` cookie 里的会话 JWT（API key 的
//! scope 锁在 `FEATURE_CODING`，拿不到）。首选获取路径是零交互读
//! kimi-desktop 本地 Cookies 库（[`crate::kimi_desktop`]）；本模块是
//! **兜底路径**：没装 / 没登录 Kimi Desktop（或桌面端 cookie 加密读不出）
//! 的用户，在应用内 WebView 登录一次 kimi.com，把 `kimi-auth` 存进
//! keys.json 的 `kimi:cookie` 槽。
//!
//! ## 流程（对齐 stepfun_login.rs 2026-07-28 重写后的现行设计）
//!
//! 用户在设置面板点「🔑 登录 Kimi（总套餐）」→ 弹 webview 加载
//! kimi.com 会员额度页（未登录会先走登录流程）→ 登录完成后
//! `www.kimi.com` 域落下 `kimi-auth` cookie → 后端**独立轮询任务**每
//! 700ms 读 `cookies_for_url(PROBE_URL)` → 见到**新鲜**（JWT exp 未过）
//! 的 token → 写 keys.json → 关窗 → emit `musage://kimi-login-success`
//! → 立即 refresh 一次让浮窗多出「总套餐」行。
//!
//! 设计要点（从 stepfun 四连 bug 学来的，全部规避）：
//! - **probe URL 与 cookie 同域**：`kimi-auth` 落在 `www.kimi.com` 域
//!   （本机 kimi-desktop Cookies 库实测 host_key = `www.kimi.com`），
//!   PROBE_URL 固定 `https://www.kimi.com/` —— 不踩 stepfun 第一版
//!   「account 域探测 platform 域 cookie 永远为空」的坑。
//! - **无 init script / READY 握手 / clear_all_browsing_data**：不干扰
//!   登录 SPA 的 localStorage / OIDC state，也不杀 SSO 零交互路径。
//! - **JWT exp 新鲜度门**：cookie jar 里的旧残留 token 必已过期 → 拒绝、
//!   继续等；登录后的新 token exp 在未来才接受。复用 provider 侧同一个
//!   [`crate::kimi_desktop::jwt_exp_seconds_ago`]，保证「登录存下来的
//!   token」和「provider 预检接受的 token」判定单一来源。
//! - **不清 browsing data**：保留 webview profile 的 kimi.com session，
//!   token 过期后重登可走「点按钮 → 已登录 → 直接抓 → 关窗」零交互路径。
//!
//! ## 并发 / 重入
//!
//! 跟 stepfun 同款：`GEN` generation 计数（重开窗口旧轮询任务静默退出）
//! + `DONE` 完成标记 + `WindowCloseGuard` panic 兜底关窗。
//!
//! ## 已知取舍
//!
//! - 不做「切换账号」：旧 token 仍有效时点登录会直接抓走旧 token 并关窗
//!   （结果正确 —— 同账号有效 token）。换账号需先在 webview 里登出
//!   kimi.com，或等 token 过期。
//! - 用户主动关窗 / 超时未登录 → 静默退出；超时 / 写盘失败才 emit
//!   `-failed` 让前端弹 toast（D3-002 同款语义）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tauri::webview::Cookie;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::time::sleep;

use crate::config;
use crate::kimi_desktop::jwt_exp_seconds_ago;
use crate::providers::Credentials;
use crate::t;

/// 全局完成标记：提取成功后置 true，轮询任务退出。
static DONE: AtomicBool = AtomicBool::new(false);

/// generation 计数器：每次 `open_kimi_login_window` +1（旧轮询任务见到
/// gen 不等即静默退出 —— 同 label 新窗口会让旧的
/// `get_webview_window().is_none()` 检查失效）。
static GEN: AtomicU64 = AtomicU64::new(0);

fn is_current_gen(my_gen: u64) -> bool {
    GEN.load(Ordering::SeqCst) == my_gen
}

/// panic 兜底 guard：轮询任务任意退出路径都确保窗口被关闭（stepfun L9 同款）。
struct WindowCloseGuard(tauri::WebviewWindow);

impl Drop for WindowCloseGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            tracing::error!("kimi 登录轮询任务 panic，guard 兜底关窗");
        }
        let _ = self.0.close();
    }
}

/// 等旧窗口真正关闭（50ms × 40 ≈ 2s 上限；超时强制 destroy 防 webview
/// 泄漏，stepfun L7/M1 同款）。
async fn wait_window_closed(app: &AppHandle, label: &str) {
    for _ in 0..40 {
        if app.get_webview_window(label).is_none() {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    if let Some(w) = app.get_webview_window(label) {
        tracing::warn!(
            label = label,
            "wait_window_closed 超时 2s,强制 destroy 防 webview 泄漏"
        );
        let _ = w.destroy();
        for _ in 0..10 {
            if app.get_webview_window(label).is_none() {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    }
}

/// 登录入口 URL：kimi.com 会员「我的额度」页。未登录时 kimi.com 会先走
/// 登录流程（扫码 / 手机号），登录后落回该页 —— 用户能直接看到官方
/// 「总使用量」条，跟浮窗即将新增的「总套餐」行语义对应。
const LOGIN_URL: &str = "https://www.kimi.com/membership/subscription?tab=quota";

/// `cookies_for_url` 的探测 URL。`kimi-auth` 落在 `www.kimi.com` 域
/// （本机 kimi-desktop Cookies 库实测 host_key = `www.kimi.com`），
/// 探测 URL 与 cookie 同域 —— 不踩 stepfun 跨域探测的坑。
const PROBE_URL: &str = "https://www.kimi.com/";

/// webview 窗口 label（capability 按此授权）。
const WINDOW_LABEL: &str = "kimi-login";

/// 目标 cookie 名。
const TOKEN_COOKIE: &str = "kimi-auth";

/// 打开 Kimi 登录 webview 窗口（设置面板「🔑 登录 Kimi（总套餐）」按钮）。
///
/// 错误（写盘失败 / 超时）通过 `musage://kimi-login-failed` 返回前端；
/// 用户主动关窗 → 静默退出，不弹红条。
#[tauri::command]
pub async fn open_kimi_login_window(app: AppHandle) -> Result<(), String> {
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    DONE.store(false, Ordering::SeqCst);

    if let Some(existing) = app.get_webview_window(WINDOW_LABEL) {
        let _ = existing.close();
        wait_window_closed(&app, WINDOW_LABEL).await;
    }

    let url: Url = LOGIN_URL
        .parse::<Url>()
        .map_err(|e| t!("kimi_login.parse_login_url", err = e.to_string()).into_owned())?;

    let b = WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::External(url))
        .title(t!("window.kimi_login").to_string())
        .inner_size(960.0, 720.0)
        .min_inner_size(640.0, 540.0)
        .resizable(true)
        .decorations(true)
        .center()
        .skip_taskbar(true);
    let b = match app.get_webview_window("settings") {
        Some(p) => b
            .parent(&p)
            .map_err(|e| format!("kimi login parent: {e}"))?,
        None => b,
    };
    let window = b
        .build()
        .map_err(|e| t!("kimi_login.build_webview", err = e.to_string()).into_owned())?;

    let app2 = app.clone();
    let window_clone = window.clone();
    let my_gen = gen;
    tauri::async_runtime::spawn(async move {
        let _close_guard = WindowCloseGuard(window_clone.clone());
        let result = poll_token_from_cookie(&app2, &window_clone, my_gen).await;
        if !is_current_gen(my_gen) {
            tracing::debug!(my_gen, "kimi 老轮询流程被新流程取代,静默退出");
            return;
        }
        match result {
            PollOutcome::Saved(len) => {
                DONE.store(true, Ordering::SeqCst);
                tracing::info!(len, "kimi-auth token 提取 + 保存成功");
                // 立即拉一次（让浮窗立刻多出「总套餐」行；未配 API key 时
                // refresh 报 unconfigured 仅警告，不阻塞成功事件）
                if let Err(e) = crate::commands::refresh_single_inner(
                    &app2,
                    "kimi",
                    crate::poller_backoff::RefreshSource::Manual,
                )
                .await
                {
                    tracing::warn!(error = %e, "kimi 登录后立即拉取失败（不阻塞成功事件）");
                }
                let _ = window_clone.close();
                let _ = app2.emit("musage://kimi-login-success", len);
            }
            PollOutcome::Timeout(reason) => {
                if !DONE.load(Ordering::SeqCst) {
                    tracing::warn!(reason = %reason, "kimi 登录超时");
                    let _ = app2.emit("musage://kimi-login-failed", reason);
                }
            }
            PollOutcome::Cancelled => {
                tracing::debug!("kimi 登录窗口已关闭或超时，未提取到 token");
            }
            PollOutcome::Failed(e) => {
                if !DONE.load(Ordering::SeqCst) {
                    tracing::error!(error = %e, "kimi login flow failed");
                    let _ = app2.emit("musage://kimi-login-failed", e);
                }
            }
        }
    });

    Ok(())
}

/// 清除已保存的 `kimi:cookie` 会话（设置面板「清除」按钮）。
///
/// **只清 cookie 槽，不动 kimi API key** —— cookie 是「总套餐」可选增强，
/// API key 才是主凭据。清完立即 refresh：浮窗回到只有 5h + 7d 的形态
/// （若 kimi-desktop 本地会话还在，refresh 后会继续从桌面端读 —— 这是
/// 设计行为，banner 帮助文案里说明）。
#[tauri::command]
pub async fn clear_kimi_session(app: AppHandle) -> Result<(), String> {
    config::delete_cookie_slot_for_id("kimi")?;
    // best-effort refresh：失败只警告（浮窗下一轮 poll 也会自然更新）
    if let Err(e) = crate::commands::refresh_single_inner(
        &app,
        "kimi",
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await
    {
        tracing::warn!(error = %e, "清除 kimi 会话后立即拉取失败（忽略）");
    }
    Ok(())
}

enum PollOutcome {
    Saved(usize),
    Cancelled,
    Timeout(String),
    Failed(String),
}

/// 轮询 webview cookie jar 直到抽到**新鲜**的 kimi-auth 或窗口消失 / 超时。
///
/// `cookies_for_url` 能读到 HttpOnly cookie（跟 xiaomi / anysearch /
/// stepfun 同一套机制）。暂态 Err → sleep 700ms 重试，不直接 Cancelled
///（stepfun H1 fix 同款）。
async fn poll_token_from_cookie(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    my_gen: u64,
) -> PollOutcome {
    // 安全上限：~14 分钟（覆盖手动扫码 / 手机号 + 验证码登录）；
    // wall-clock deadline 为主，MAX_ITERS 兜底防 runaway。
    const MAX_ITERS: u32 = 1200;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(14 * 60);
    let probe_url: Url = PROBE_URL
        .parse()
        .unwrap_or_else(|_| Url::parse("https://www.kimi.com/").expect("hardcoded URL parses"));

    // 首次读取前让出 1s：等窗口首个导航开始、cookie store 可用。
    sleep(Duration::from_millis(1000)).await;
    if DONE.load(Ordering::SeqCst) || app.get_webview_window(WINDOW_LABEL).is_none() {
        return PollOutcome::Cancelled;
    }

    for _ in 0..MAX_ITERS {
        if DONE.load(Ordering::SeqCst) {
            return PollOutcome::Cancelled;
        }
        if crate::poller::SHUTDOWN_NATIVE_THREADS.load(std::sync::atomic::Ordering::SeqCst) {
            tracing::debug!("kimi 轮询收到 SHUTDOWN, 退出");
            return PollOutcome::Cancelled;
        }
        if std::time::Instant::now() >= deadline {
            tracing::warn!("kimi 登录轮询达到 14min 硬上限 deadline, 通知前端");
            return PollOutcome::Timeout(kimi_timeout_reason());
        }
        if app.get_webview_window(WINDOW_LABEL).is_none() {
            return PollOutcome::Cancelled;
        }
        if !is_current_gen(my_gen) {
            tracing::debug!(my_gen, "kimi 轮询 gen 失效,静默退出");
            return PollOutcome::Cancelled;
        }

        let cookies: Vec<Cookie<'static>> = match window.cookies_for_url(probe_url.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "读 webview cookies_for_url 暂态失败, 700ms 后重试");
                sleep(Duration::from_millis(700)).await;
                continue;
            }
        };

        if let Some(tok) = cookies.iter().find(|c| c.name() == TOKEN_COOKIE) {
            // cookie value 可能带引号（macOS WKWebView 习惯），剥掉
            let token = tok.value().trim_matches('"');
            if is_fresh_token(token) {
                return match save_token(token) {
                    Ok(len) => PollOutcome::Saved(len),
                    Err(e) => PollOutcome::Failed(e),
                };
            }
            // token 在但已过期 / 为空 —— 上一次会话的残留,继续等用户登录后的新 token
            tracing::debug!("kimi-auth 存在但已过期或为空(旧会话残留),继续轮询");
        }
        // 没有 kimi-auth cookie = 用户还没登录 —— 继续等

        sleep(Duration::from_millis(700)).await;
    }

    tracing::warn!("kimi 登录轮询达到 14min 安全上限, 通知前端");
    PollOutcome::Timeout(kimi_timeout_reason())
}

/// 超时原因走 i18n（D3-002 同款语义：区分「超时」和「用户主动关」）。
fn kimi_timeout_reason() -> String {
    t!("login.kimi.timeout", secs = 14 * 60).into_owned()
}

/// 判断抽到的 kimi-auth 是否「新鲜可用」。
///
/// - 空串 → 拒绝（等待写入）
/// - JWT 可解且 exp 已过期 → 拒绝（旧会话残留，继续等新 token）
/// - JWT 可解且未过期 → 接受
/// - 解不出 exp（非 JWT / 格式变化）→ 放行，交给服务端校验（宁可存一个
///   可能无效的 token 让 provider 跳过增强，也不让登录流程永远卡死；
///   stepfun `is_fresh_token` 同款取舍）
fn is_fresh_token(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    match jwt_exp_seconds_ago(value) {
        Some(secs_ago) => secs_ago < 0,
        None => true,
    }
}

/// 把抽到的 token 写进 keys.json 的 `kimi:cookie` 槽位（裸 JWT，不带
/// `kimi-auth=` 前缀 —— provider 侧 `resolve_session_token` 直接当
/// Bearer 用；`save_credential_for_id` 对 None 字段跳过不删，API key
/// 槽不受影响）。返回写入字节数。
fn save_token(token: &str) -> Result<usize, String> {
    // 12 KB 上限（stepfun M2 同款：RFC 6265 § 6.1 cookie 4 KB 推荐上限的
    // 3x 冗余；实测 kimi-auth JWT ~555 字符，未来扩 claim 也远够）
    if token.len() > 12 * 1024 {
        return Err(t!("kimi_login.token_too_large", bytes = token.len()).into_owned());
    }
    let cred = Credentials {
        api_key: None,
        cookie: Some(token.to_string()),
        secret_key: None,
    };
    config::save_credential_for_id("kimi", &cred)
        .map_err(|e| t!("kimi_login.save_keys_failed", err = e.to_string()).into_owned())?;
    Ok(token.len())
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

    #[test]
    fn save_token_rejects_over_12kb() {
        // 12 KB length gate（不走实际 save，只验证 size 短路）
        let big = "a".repeat(13 * 1024);
        let err = save_token(&big).unwrap_err();
        assert!(
            err.contains("13312") || err.contains("12"),
            "expected size-cap error, got: {err}"
        );
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
    fn window_label_matches_capability() {
        // capability 文件里 windows 数组必须含这个 label,否则 webview 拿不到权限
        assert_eq!(WINDOW_LABEL, "kimi-login");
    }

    #[test]
    fn probe_url_matches_cookie_domain() {
        // 回归防御：cookies_for_url 按域过滤，probe 必须在 kimi-auth 实际
        // 落域（www.kimi.com）—— stepfun 2026-07-27 版用错域导致永远抓不到
        let u: Url = PROBE_URL.parse().expect("parse probe url");
        assert_eq!(u.host_str(), Some("www.kimi.com"));
        assert_eq!(u.scheme(), "https");
    }
}
