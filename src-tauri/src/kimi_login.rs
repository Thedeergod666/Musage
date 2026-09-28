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
//!   （结果正确 —— 同账号有效 token）。**换账号的可达路径（2026-09-28 补）**：
//!   ① 在登录窗内登出 kimi.com 再登录，或 ② 设置面板点「清除」走
//!   [`clear_kimi_session`] —— 它会连 webview cookie jar 里 kimi 域的
//!   session 一起清掉，下次登录必然是真登录。
//!   ⚠️ 旧文档写的「在系统浏览器里登出」是**不可达的**：登录 webview 用的是
//!   app 自己的持久化 WKWebView store，跟系统浏览器的 cookie jar 完全隔离，
//!   在系统浏览器登出对登录窗没有任何影响。
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

/// webview cookie jar 里属于 kimi 的域名（清 jar 时按域白名单删，
/// 不碰别的 provider 的会话）。
const KIMI_COOKIE_DOMAINS: &[&str] = &["kimi.com"];

/// 解析本次登录要写入的凭据槽（base 或副本 unique_id）。
///
/// 2026-09-28 fix（契约 1/2）：base 禁用 + 副本启用时，登录写 base 槽、
/// 浮窗刷副本槽 → 「登录成功但卡片还是红的」，且副本凭据成了 UI 删不掉的
/// 孤儿。优先级：显式 `instance_id` > base 启用 > 升序第一个启用副本 >
/// `instance_id` 原样（用户明确点了某张卡，禁用也该写进去） > base。
async fn resolve_target(app: &AppHandle, base: &str, instance_id: Option<&str>) -> String {
    match crate::commands::resolve_login_refresh_target(&app.state(), base, instance_id).await {
        Some(t) => t,
        None => instance_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(base)
            .to_string(),
    }
}

/// 打开 Kimi 登录 webview 窗口（设置面板「🔑 登录 Kimi（总套餐）」按钮）。
///
/// `instance_id`（2026-09-28 契约 1/2）：副本行必须传自己的 `unique_id`；
/// 标量参数走 camelCase，前端 `{ instanceId: "kimi#2" }`；不传 → 旧行为。
///
/// 错误（写盘失败 / 超时）通过 `musage://kimi-login-failed` 返回前端；
/// 用户主动关窗 → 静默退出，不弹红条。
#[tauri::command]
pub async fn open_kimi_login_window(
    app: AppHandle,
    instance_id: Option<String>,
) -> Result<(), String> {
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
        Some(p) => b.parent(&p).map_err(|e| {
            // D7-06 (2026-09-04 audit): parent() 消费 builder，Err 后无法降级为无 parent ——
            // 只能把原始错误细节（{e:#} 含 anyhow 链）落 warn 日志并带进IPC 错误，
            // 此前 format!("{e}") 丢 details 导致难定位。
            tracing::warn!(error = ?e, "kimi login parent 设置失败");
            format!("kimi login parent: {e:#}")
        })?,
        None => b,
    };
    let window = b
        .build()
        .map_err(|e| t!("kimi_login.build_webview", err = e.to_string()).into_owned())?;

    let app2 = app.clone();
    let window_clone = window.clone();
    let my_gen = gen;
    // D7-02 fix + 2026-09-28 契约 1：spawn 前 resolve refresh target，
    // 前端显式传的 instance_id 优先（见 resolve_target）。
    let target = resolve_target(&app2, "kimi", instance_id.as_deref()).await;
    tauri::async_runtime::spawn(async move {
        let _close_guard = WindowCloseGuard(window_clone.clone());
        let result = poll_token_from_cookie(&app2, &window_clone, my_gen, &target).await;
        if !is_current_gen(my_gen) {
            tracing::debug!(my_gen, "kimi 老轮询流程被新流程取代,静默退出");
            return;
        }
        match result {
            PollOutcome::Saved(len) => {
                DONE.store(true, Ordering::SeqCst);
                tracing::info!(len, target = %target, "kimi-auth token 提取 + 保存成功");
                // D7-02 fix: target 已 resolve + save 直接写 target 槽,直接刷 target。
                if let Err(e) = crate::commands::refresh_single_inner(
                    &app2,
                    &target,
                    crate::poller_backoff::RefreshSource::Manual,
                )
                .await
                {
                    tracing::warn!(error = %e, target = %target, "kimi 登录后立即拉取失败（不阻塞成功事件）");
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

/// 清除已保存的 `{id}:cookie` Kimi 网页会话（设置面板「清除」按钮）。
///
/// **只清 cookie 槽，不动 kimi API key** —— cookie 是「总套餐」可选增强，
/// API key 才是主凭据。清完立即 refresh：浮窗回到只有 5h + 7d 的形态
/// （若 kimi-desktop 本地会话还在，refresh 后会继续从桌面端读 —— 这是
/// 设计行为，banner 帮助文案里说明）。
///
/// 2026-09-28 fix（2 处）：
/// 1. **实例感知**：旧实现硬编码 `delete_cookie_slot_for_id("kimi")` +
///    无条件刷 `"kimi"`，副本 `kimi#2` 的 cookie 槽永远删不掉（D7-02 只补了
///    写入侧，清除侧没跟上）。改成跟登录同款 resolve：`instance_id` 合法就
///    删它，否则按 base 启用 / 升序第一个启用副本。前端不传该参数时行为不变。
/// 2. **顺带清 webview cookie jar**：清完只删 keys.json 的话，登录窗里
///    `kimi-auth` 的 SSO session 还在 —— 下次点登录会秒抓回同一份 token
///    （「换账号」不可达，见模块头「已知取舍」）。逐个 `delete_cookie`
///    清 kimi 域的 session 后，下一次登录必然是真登录。
#[tauri::command]
pub async fn clear_kimi_session(app: AppHandle, instance_id: Option<String>) -> Result<(), String> {
    let target = resolve_target(&app, "kimi", instance_id.as_deref()).await;
    config::delete_cookie_slot_for_id(&target)?;
    tracing::info!(target = %target, "kimi 已清除凭据槽，开始清理 webview cookie jar");
    if let Err(e) = purge_cookies_for_domains(&app, KIMI_COOKIE_DOMAINS).await {
        tracing::warn!(error = %e, target = %target, "kimi 清 webview cookie jar 失败（下次登录可能仍抓到旧 session）");
    }
    // best-effort refresh：失败只警告（浮窗下一轮 poll 也会自然更新）
    if let Err(e) = crate::commands::refresh_single_inner(
        &app,
        &target,
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await
    {
        tracing::warn!(error = %e, target = %target, "清除 kimi 会话后立即拉取失败（忽略）");
    }
    Ok(())
}

/// 临时窗口 label 计数器（同 label 并发 build 会失败；两次清除动作同时点
/// 不是要支持的场景，用计数器保证互不干扰）。
static PURGE_WINDOW_SEQ: AtomicU64 = AtomicU64::new(0);

/// 清掉 webview cookie jar 里属于 `domains` 的全部 cookie，返回删除条数。
///
/// ## 为什么不用 `WebviewWindow::clear_all_browsing_data()`
///
/// 2026-09-28 实读 wry 0.55.1 源码确认：macOS 上它对
/// `WKWebsiteDataStore` 调 `removeDataOfTypes(allWebsiteDataTypes, since 1970)`，
/// 而 Tauri 的登录窗跟主窗口 / 设置面板**共用同一个默认（非 incognito）data
/// store** → 进程级全清，连 stepfun / anysearch / xiaomi 的登录会话和 app
/// 自己的 webview 存储一起抹掉；Windows 上 `ClearBrowsingDataAll` 作用于
/// 整个 user-data-folder profile，同款连带伤害。用户点「清除 Kimi 会话」
/// 却把别的 provider 登出，是不可接受的。
///
/// 逐个 `delete_cookie` 走的是底层 cookie store
/// （`WKHTTPCookieStore.deleteCookie` / `ICoreWebView2CookieManager::DeleteCookies`），
/// 能删 HttpOnly cookie，且只影响指定域。
///
/// ## 实现注记
///
/// `cookies()` / `delete_cookie()` 是 `WebviewWindow` 的方法，要句柄就得有
/// 个 webview —— 登录窗此刻通常已关，所以临时建一个**不可见**的 `about:blank`
/// 窗（Tauri 文档的标准用法），删完立刻 destroy。它跟登录窗共享同一个 data
/// store，删的就是登录窗的 jar。
async fn purge_cookies_for_domains(app: &AppHandle, domains: &[&str]) -> Result<usize, String> {
    let label = format!(
        "musage-cookie-purge-{}",
        PURGE_WINDOW_SEQ.fetch_add(1, Ordering::SeqCst)
    );
    let url = Url::parse("about:blank").map_err(|e| format!("purge window url: {e}"))?;
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .visible(false)
        .build()
        .map_err(|e| format!("purge window build: {e}"))?;

    // webview 刚建好时 data store 可能还没初始化（stepfun H1 记录过同类暂态
    // Err），先让出一点时间再读。
    sleep(Duration::from_millis(400)).await;

    let outcome = purge_with_window(&window, domains);

    // 无论成败都要回收窗口，否则每次「清除」都漏一个 webview。
    let _ = window.destroy();
    outcome
}

fn purge_with_window(window: &tauri::WebviewWindow, domains: &[&str]) -> Result<usize, String> {
    let all = window
        .cookies()
        .map_err(|e| format!("webview.cookies(): {e}"))?;
    let mut deleted = 0usize;
    for c in all {
        let domain = c.domain().unwrap_or("");
        if !domains.iter().any(|d| cookie_domain_matches(domain, d)) {
            continue;
        }
        match window.delete_cookie(c.clone()) {
            Ok(()) => {
                deleted += 1;
                tracing::debug!(name = c.name(), domain, "已删除登录域 cookie");
            }
            Err(e) => {
                tracing::warn!(name = c.name(), domain, error = %e, "delete_cookie 失败");
            }
        }
    }
    tracing::info!(deleted, "webview cookie jar 清理完成");
    Ok(deleted)
}

/// cookie 的 `domain` 是否落在 `suffix` 域内（含自身）。
///
/// cookie domain 常见形态是前导点（`.kimi.com`）—— 先剥掉再比。
fn cookie_domain_matches(domain: &str, suffix: &str) -> bool {
    let d = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    let s = suffix.trim().trim_start_matches('.').to_ascii_lowercase();
    !s.is_empty() && (d == s || d.ends_with(&format!(".{s}")))
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
    // D7-02 fix (2026-09-07 audit): 接收 caller 在 spawn 前 resolve 的 target。
    target: &str,
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
                // P2 audit fix (2026-08-13): cookies_for_url blocking 期间用户
                // 可能重新登录 → gen bump, 复查避免旧流程 token 覆盖新 token。
                if !is_current_gen(my_gen) {
                    return PollOutcome::Cancelled;
                }
                return match save_token(target, token) {
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
        // P3 audit fix (2026-08-13): 加 60s skew 对齐 kimi_desktop::
        // validate_auth_token (边界 token 拿到即死 -> 拒绝让 provider 自愈,
        // 而非存下立刻 401)。
        Some(secs_ago) => secs_ago + 60 < 0,
        None => true,
    }
}

/// 把抽到的 token 写进 keys.json 的 `kimi:cookie` 槽位（裸 JWT，不带
/// `kimi-auth=` 前缀 —— provider 侧 `resolve_session_token` 直接当
/// Bearer 用；`save_credential_for_id` 对 None 字段跳过不删，API key
/// 槽不受影响）。返回写入字节数。
/// D7-02 fix (2026-09-07 audit): 旧实现硬编码写 "kimi" base 槽,base 禁用 +
/// 副本启用场景 refresh 命中副本空槽 → 401 循环。caller 早期 resolve 传 target。
fn save_token(target: &str, token: &str) -> Result<usize, String> {
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
    config::save_credential_for_id(target, &cred)
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
        let err = save_token("kimi", &big).unwrap_err();
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

    #[test]
    fn cookie_domain_matches_leading_dot_form() {
        // cookie store 里 domain 常见 ".kimi.com" 形态
        assert!(cookie_domain_matches(".kimi.com", "kimi.com"));
        assert!(cookie_domain_matches("www.kimi.com", "kimi.com"));
    }

    #[test]
    fn cookie_domain_rejects_lookalike_suffix() {
        // 防 `notkimi.com` / `kimi.com.evil.tld` 这类后缀钓鱼
        assert!(!cookie_domain_matches("notkimi.com", "kimi.com"));
        assert!(!cookie_domain_matches("kimi.com.evil.tld", "kimi.com"));
    }
}
