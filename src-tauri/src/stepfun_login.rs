//! StepFun Oasis-Token 一键提取 —— 应用内 WebView 登录
//!
//! 用户在设置面板点 "🔑 登录 StepFun" → 弹一个 webview 窗口加载
//! `platform.stepfun.com` 登录页 → 用户在 webview 里正常走"邮箱 +
//! 密码"登录 → 后端监听 `on_page_load` 检测到 dashboard URL → 提取
//! `Oasis-Token` cookie → 写进 keys.json → 关 webview → emit
//! `musage://stepfun-login-success` 事件。
//!
//! ## 设计要点
//!
//! - **不走 DevTools**：cookie 始终在 webview 自己的 cookie jar 里（加密
//!   内存），不需要复制到剪贴板
//! - **不依赖外部扩展**：复用现有 Tauri 2 webview 能力，0 新增依赖
//! - **跨平台同代码**：Mac/Win/Linux 都是同一套（Tauri runtime 适配）
//!
//! ## 登录完成启发式
//!
//! StepFun 登录后跳到 `platform.stepfun.com/<dashboard path>`。判定
//! "登录完成"的规则（仿 xiaomi_login.rs 的 `is_dashboard_url`）：
//! - host **完全等于** `platform.stepfun.com`
//! - scheme 必须是 `https`
//! - 不在 `passport` / `/login` / `/signin` / `/signup` 路径上
//!
//! 这是 heuristic —— 如果 StepFun 改了登录流程，要改这里或加新关键字。
//!
//! ## 并发控制
//!
//! `on_page_load` 在 macOS WKWebView 上会多次触发（登录后 SPA 跳
//! 转 + 页面内导航），每次触发都会 spawn 异步任务。用 `AtomicBool`
//! 保证同一时间只有一个提取任务在运行。H3 fix: 用 RAII `ExtractingGuard`
//! 兜底 panic。
//!
//! ## Cookie 白名单
//!
//! 登录后 dashboard 实际依赖的就 1 个 cookie（Oasis-Token）。其他
//! 分析 cookie（_ga / _fbp）一律丢弃（最小权限）。
//! platform 改名 → 改这里就行。
//!
//! ## Token 形态
//!
//! CodexBar 实测 Oasis-Token 是 `<access>...<refresh>` 形式（用
//! `...` 哨兵分隔，跟 AnySearch 一样）。但 webview 拿到的可能只
//! 有 `Oasis-Token` 单段，Oasis-Refresh-Token 是另一段 cookie。
//! 我们只存 `Oasis-Token`（含两段就两段，含一段就一段），让 stepfun.rs
//! 现有的 `normalize_oasis_token` + `device_id_for_token` 处理 split。
//!
//! ## 重新登录 / 残留清理
//!
//! webview profile 可能持久化上一次的 Oasis-Token（如果之前已登录）。
//! 第一次打开 webview 时 `cookies_for_url` 立刻能拿到旧 token ——
//! init script 在 document_start 清空 `Oasis-Token` / `Oasis-Webid`
//! 旧值并置 `MUSAGE_READY` 标记（仿 anysearch_login.rs 的握手机制）。
//! 这样保证抓到的一定是用户新登录后的 token。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::webview::Cookie;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::time::sleep;

use crate::config;
use crate::providers::Credentials;
use crate::t;

/// 全局提取锁：防止多个 on_page_load 回调同时运行提取任务。
static EXTRACTING: AtomicBool = AtomicBool::new(false);

/// 全局完成标记：提取成功后置 true，后续 on_page_load 回调全部跳过。
static DONE: AtomicBool = AtomicBool::new(false);

/// RAII guard: Drop 时无条件 reset `EXTRACTING`。
///
/// H3 fix: tokio spawn 的 task panic 时，guard 在任意路径退出(正常
/// 返回/Err/panic)都会被 Drop,强制 reset EXTRACTING,保证下次用户点
/// 登录能 compare_exchange 成功。
struct ExtractingGuard;

impl Drop for ExtractingGuard {
    fn drop(&mut self) {
        EXTRACTING.store(false, Ordering::SeqCst);
    }
}

/// 登录入口 URL。直接跳到 account.stepfun.com 登录页（携带 redirect
/// 让登录后自动跳回 platform.stepfun.com，OIDC 风格的跨域 cookie 传递）。
///
/// **为什么不在 platform.stepfun.com/ 根路径**：
/// - 根路径未登录时会显示 dashboard 主页内容,用户必须手动点右上角
///   "未登录"按钮才能跳到登录页 → 多一次点击
/// - 根路径打开后我们 `is_dashboard_url` 会**误判**为 dashboard 触发
///   cookie 提取任务(根路径 host 一致),此时 webview 还没登录根本
///   拿不到 Oasis-Token,5 次重试 11s 跑完 emit 错误——**用户根本
///   还没开始登录**
///
/// 直接打 `account.stepfun.com/login` + redirect 参数,登录后 SPA
/// 自动跳回 platform.stepfun.com 域,on_page_load 触发时 Oasis-Token
/// cookie 已经落定。
///
/// 已知:登录页**只有手机号+验证码**默认可见,要"账号密码登录"必须
/// 点"其他登录方式"→"账号密码登录"(2 次点击)。我们**没**找到直接
/// 走密码模式的 URL 参数(`type=password` / `login_type=password` 都
/// 不生效),只能接受这 2 次点击。
const LOGIN_URL: &str = "https://account.stepfun.com/login?redirect=https%3A%2F%2Fplatform.stepfun.com%2F&source_app=platform-cn";

/// webview 窗口 label（capability 按此授权 create-webview-window）。
const WINDOW_LABEL: &str = "stepfun-login";

/// 「清理完成」握手标记 cookie。init script 在 document_start 清掉
/// 旧 Oasis-Token / Oasis-Webid 后写 `MUSAGE_READY=1`；Rust 轮询
/// **见到 READY 才开始接受** `Oasis-Token`。
///
/// 仿 [anysearch_login.rs](crate::anysearch_login) 同款机制：
/// webview profile 持久化 cookie，若轮询在 init script 清理之前就读
/// cookie，会抓到过期 token 存盘 → 浮窗继续 401、登录窗口"弹出即消失"。
/// READY 保证 token 一定是清理之后新登录的。
const READY_COOKIE_NAME: &str = "MUSAGE_READY";

/// 判定 URL 是否已经离开登录域、到达 dashboard。
///
/// 关键约束：**Oasis-Token cookie 是 platform.stepfun.com 域的**
/// (从 chrome-devtools 实测 Network 请求看到)。所以提取任务必须
/// 等待用户从 account.stepfun.com 登录后跳回 platform.stepfun.com
/// 域才触发,不能误判 account.stepfun.com 域为 dashboard。
///
/// 规则：
/// - host **完全等于** `platform.stepfun.com`(不接受 account.stepfun.com
///   或 stepfun.ai 海外站)
/// - scheme 必须是 `https`
/// - 不能在 `passport` / `/login` / `/signin` / `/signup` 路径上
///
/// 简化:根路径(`/`)和 `/account-overview` / `/interface-key` 都接受
/// —— CodexBar docs 也说"登录后回到 dashboard",所以"已经能登到
/// 根路径"就是登录成功的标志,不再加更严格 path 白名单(避免
/// StepFun 改 dashboard 路径我们就漏判)。
fn is_dashboard_url(url: &Url) -> bool {
    let host_ok = url.host_str() == Some("platform.stepfun.com") && url.scheme() == "https";
    let s = url.as_str();
    let not_login = !s.contains("passport")
        && !s.contains("/login")
        && !s.contains("/signin")
        && !s.contains("/signup");
    host_ok && not_login
}

/// dashboard 实际依赖的 cookie name 集合。不在白名单的丢弃（最小权限）。
const WANTED_COOKIES: &[&str] = &["Oasis-Token"];

/// 打开登录 webview 窗口。
///
/// 行为：
/// 1. 如果已有 `stepfun-login` 窗口（用户再次点按钮），先关掉
/// 2. 开新 webview 指向 `LOGIN_URL`
/// 3. 监听 `on_page_load`：URL 命中 dashboard 启发式 → 等待 + 重试提取
///    cookie → 保存 → 关闭 → emit 成功事件
///
/// macOS WKWebView 上 `on_page_load` 会多次触发（登录后 SPA 跳
/// 转 + 页面内导航），用 `EXTRACTING` 保证只有一个任务在提取。
///
/// 错误通过 `musage://stepfun-login-failed` 事件返回给前端。
#[tauri::command]
pub async fn open_stepfun_login_window(app: AppHandle) -> Result<(), String> {
    // 重置提取锁 + 完成标记（新窗口 = 全新流程）
    EXTRACTING.store(false, Ordering::SeqCst);
    DONE.store(false, Ordering::SeqCst);

    // 已开过 → 先关（重新登录场景）
    if let Some(existing) = app.get_webview_window(WINDOW_LABEL) {
        let _ = existing.close();
        sleep(Duration::from_millis(100)).await;
    }

    let url: Url = (LOGIN_URL.parse::<Url>())
        .map_err(|e| t!("stepfun_login.parse_login_url", err = e.to_string()).into_owned())?;

    // 闭包必须 'static + Send + Sync → 克隆 AppHandle（内部 Arc 包装，廉价）
    let app_for_callback = app.clone();

    WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::External(url))
        .title(t!("window.stepfun_login").to_string())
        .inner_size(960.0, 720.0)
        .min_inner_size(640.0, 540.0)
        .resizable(true)
        .decorations(true)
        .center()
        // H8 fix: 在 webview 里注入 init script,挡掉第三方 tracker 在
        // 受信 webview 上下文里跑 JS 偷 session + 清理上一次的残留
        // Oasis-Token / Oasis-Webid cookie（webview profile 持久化）。
        //   - document.cookie getter: 仅当 location.hostname ===
        //     "platform.stepfun.com" 时返回真值,否则返空串。
        //   - 启动即清:删旧 Oasis-Token / Oasis-Webid + 置 MUSAGE_READY
        //     标记,Rust 轮询见到 READY 才接受新 token(避免抓过期 cookie)
        // 配合 capabilities/stepfun-login.json 把 webview create 权限
        // only 给 stepfun-login 窗口。
        .initialization_script(
            r#"
            (function () {
                // ALLOW_HOST 用于限制"document.cookie / Storage 读取"。
                // StepFun 登录流程跨 account.stepfun.com (登录域) →
                // platform.stepfun.com (dashboard 域) 两个域,所以:
                //   - cookie/storage 读取**两个域都允许**(user 输
                //     密码时 account 域要能写入 sessionStorage 走
                //     OIDC 流程)
                //   - 旧 Oasis-Token / Oasis-Webid / Oasis-Refresh-Token
                //     cookie 清理**只在 platform 域**做(避免误清
                //     account 域的 SSO session)
                //   - MUSAGE_READY 握手**所有域都置**(保证 redirect
                //     跳到 platform 时 Rust 端 cookies_for_url 能
                //     见到 READY 标记)
                var ALLOW_HOST = "platform.stepfun.com";
                var DASHBOARD_HOST = "platform.stepfun.com";
                var READY_NAME = "MUSAGE_READY";
                function isDashboard() {
                    try { return location.hostname === DASHBOARD_HOST; } catch (_) { return false; }
                }
                // ── cookie / storage 读取锁到受信 host 集合(account + platform)──
                try {
                    var _origCookie = Object.getOwnPropertyDescriptor(Document.prototype, "cookie");
                    Object.defineProperty(document, "cookie", {
                        get: function () { return isDashboard() ? _origCookie.get.call(this) : ""; },
                        set: function (v) { if (isDashboard()) _origCookie.set.call(this, v); },
                        configurable: false
                    });
                } catch (_) {}
                try {
                    var _origLs = Object.getOwnPropertyDescriptor(Storage.prototype, "getItem");
                    Object.defineProperty(Storage.prototype, "getItem", {
                        value: function (k) { return isDashboard() ? _origLs.value.call(this, k) : null; },
                        configurable: true
                    });
                } catch (_) {}
                // ── 重新登录:清掉上一次残留的 Oasis-Token(关键 fix)──
                // webview profile 持久化 cookie —— 上一次(可能已过期)
                // 的 Oasis-Token 还在 platform 域 cookie jar 里。不清
                // 的话 Rust 端 cookies_for_url 会立刻抓到过期 token
                // 存盘 → 浮窗继续 401,且登录窗口"弹出即消失"。所
                // 以每次打开都从干净状态开始:
                //   1) 删旧 Oasis-Token / Oasis-Webid / Oasis-Refresh-Token
                //      cookie(强制重新登录)
                //   2) 置 MUSAGE_READY 标记 —— Rust 见到 READY 才开始
                //      接受 token,保证不会抓到清理之前残留在 cookie
                //      jar 里的旧 token
                // 只在 dashboard 域上清(isDashboard 守卫),account
                // 域的 SSO session 不动。
                try {
                    if (isDashboard()) {
                        document.cookie = "Oasis-Token=; path=/; max-age=0";
                        document.cookie = "Oasis-Webid=; path=/; max-age=0";
                        document.cookie = "Oasis-Refresh-Token=; path=/; max-age=0";
                        document.cookie = READY_NAME + "=1; path=/; max-age=3600; SameSite=Lax";
                    } else {
                        // account 域:也置 READY(cookies_for_url 在 platform
                        // 域查的 READY 是 platform 域 cookie,这里设的 account
                        // 域 READY 不冲突;主要是要 platform 域自己 document_start
                        // 跑时设上)
                        document.cookie = READY_NAME + "=1; path=/; max-age=3600; SameSite=Lax";
                    }
                } catch (_) {}
            })();
            "#,
        )
        .on_page_load(move |window, payload| {
            let url = payload.url();
            tracing::debug!(%url, "stepfun login webview page load");

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
                // Rust 仍会跑局部变量的 Drop glue。guard 在任意路径退出
                // 都会被 Drop,强制 reset EXTRACTING。
                let _extracting_guard = ExtractingGuard;
                let result = extract_with_retry(&window_clone, &app2).await;
                // 显式 store 保留显式锁语义给阅读者,guard 是 panic 兜底。
                EXTRACTING.store(false, Ordering::SeqCst);

                match result {
                    Ok(saved_len) => {
                        DONE.store(true, Ordering::SeqCst);
                        tracing::info!(saved_len, "stepfun cookie 提取 + 保存成功");
                        // 立即拉一次（让浮窗立刻看到数据）
                        if let Err(e) =
                            crate::commands::refresh_single_inner(&app2, "stepfun").await
                        {
                            tracing::warn!(error = %e, "登录后立即拉取失败（不阻塞成功事件）");
                        }
                        // 关 webview
                        let _ = window_clone.close();
                        // 通知前端
                        let _ = app2.emit("musage://stepfun-login-success", saved_len);
                    }
                    Err(e) => {
                        // 只有 DONE 为 false 时才报错（避免关闭后的残留任务触发误报）
                        if !DONE.load(Ordering::SeqCst) {
                            tracing::error!(error = %e, "stepfun login flow failed, closing webview");
                            // 关 webview(user 看错误条后不必手动关,体验更顺)
                            let _ = window_clone.close();
                            emit_failed(&app2, e);
                        }
                    }
                }
            });
        })
        .build()
        .map_err(|e| t!("stepfun_login.build_webview", err = e.to_string()).into_owned())?;

    Ok(())
}

/// 轮询 webview cookie jar 直到抽到合法 Oasis-Token 或窗口消失 / 超时。
///
/// 改成跟 [anysearch_login.rs](crate::anysearch_login) 的 `poll_token_from_cookie`
/// 同款轮询模式（旧版固定 5 次重试间隔 11s 太慢：首次成功要等 1s sleep
/// 才检查，用户登录后 cookie 落定到 on_page_load 触发之间可能 <500ms，
/// 但旧版要等 1s 才第一次检查 → 浪费 500ms+；更糟的是如果首次
/// ready_seen 失败，要等 1+2+4+6+12=25s 才退出）。
///
/// 新策略：
/// - 首次 sleep 300ms（给 init script document_start 跑完 + cookie 落定）
/// - 然后每 700ms 轮询一次（跟 anysearch 一致）
/// - 最多 40 次 = 28s 覆盖手机号+验证码+跨域 redirect 链
/// - URL 不在 dashboard 时不消耗轮询次数（continue 不 increment）
async fn extract_with_retry(
    window: &tauri::WebviewWindow,
    _app: &AppHandle,
) -> Result<usize, String> {
    // 首次让出 300ms：等 init script document_start 跑完清旧 cookie +
    // 置 MUSAGE_READY。webview profile 持久化上一次的 Oasis-Token
    // cookie；若一开窗就立刻读，可能先于清理抓到过期 token
    // （「弹出即消失 + 信息不更新」bug 的根因）。
    sleep(Duration::from_millis(300)).await;
    if DONE.load(Ordering::SeqCst) {
        return Err(t!("stepfun_login.another_task_done").into_owned());
    }

    // 安全上限：40 次 × 700ms = 28s，覆盖手机号+验证码+跨域 redirect
    const MAX_ITERS: u32 = 40;

    for iter in 0..MAX_ITERS {
        if DONE.load(Ordering::SeqCst) {
            return Err(t!("stepfun_login.another_task_done").into_owned());
        }

        // 检查 URL 是否还在 dashboard
        let current_url = match window.url() {
            Ok(u) => u,
            Err(e) => {
                tracing::debug!(error = %e, iter, "读 webview URL 失败（窗口可能已关闭）");
                return Err(t!("stepfun_login.read_url_failed", err = e.to_string()).into_owned());
            }
        };

        if !is_dashboard_url(&current_url) {
            // URL 不在 dashboard（还在 account 域登录页）→ 不消耗轮询
            // 次数，等 700ms 后再看
            tracing::debug!(%current_url, iter, "URL 不在 dashboard，等 700ms");
            sleep(Duration::from_millis(700)).await;
            continue;
        }

        // 尝试提取
        match extract_and_save(window).await {
            Ok(saved_len) => {
                tracing::info!(saved_len, iter, "cookie 提取成功");
                return Ok(saved_len);
            }
            Err(e) => {
                tracing::debug!(error = %e, iter, "cookie 提取失败，等 700ms 再试");
            }
        }

        sleep(Duration::from_millis(700)).await;
    }

    // 所有轮询都失败
    Err(t!("stepfun_login.cookie_extraction_failed").into_owned())
}

/// 从 webview 提取 cookie → 过滤白名单 → 写 keys.json。
///
/// 返回写入的字节数（便于前端展示"已保存 N 字节"）。
async fn extract_and_save(window: &tauri::WebviewWindow) -> Result<usize, String> {
    let url: Url = (LOGIN_URL.parse::<Url>())
        .map_err(|e| t!("stepfun_login.parse_url", err = e.to_string()).into_owned())?;

    // cookies_for_url：拿指定 URL 上下文下的 cookies（含 HttpOnly，
    // 这正是我们需要的 —— 普通 document.cookie 读不到 HttpOnly）
    let raw_cookies: Vec<Cookie<'static>> = window
        .cookies_for_url(url)
        .map_err(|e| t!("stepfun_login.cookies_for_url_failed", err = e.to_string()).into_owned())?;

    tracing::debug!(total = raw_cookies.len(), "cookies_for_url 返回");

    // 握手:init script 清完上一次残留的 Oasis-Token 后才写 MUSAGE_READY。
    // 没见到 READY 就不读 token —— 否则可能抓到清理之前残留在 cookie jar
    // 里的过期 Oasis-Token（"弹出即消失 + 信息不更新"bug 的根因）。
    let ready_seen = raw_cookies.iter().any(|c| c.name() == READY_COOKIE_NAME);
    if !ready_seen {
        return Err(t!("stepfun_login.handshake_not_ready").into_owned());
    }

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
            "stepfun_login.cookies_not_found",
            count = raw_cookies.len(),
            expected = WANTED_COOKIES.len(),
            wanted = format!("{WANTED_COOKIES:?}"),
            available = format!("{available:?}")
        )
        .into_owned());
    }

    // macOS WKWebView 的 cookie store 可能 value 外层包双引号
    // （如 `"tokenvalue"`），Cookie: HTTP header 期望 raw value，
    // 需要去掉。
    let cookie_parts: Vec<String> = relevant
        .iter()
        .map(|c| {
            let val = c.value().trim_matches('"');
            format!("{}={}", c.name(), val)
        })
        .collect();

    // F4 fix 风格: 在写入 keys.json 前做完整性校验。Oasis-Token 是
    // 唯一的 auth-critical cookie;必须存在。不在就 return Err,不
    // 覆盖原有的有效 cookie（避免用户被锁在"看似登录了但 API 401"的
    // 状态）。
    let has_oasis_token = cookie_parts
        .iter()
        .any(|p| p.starts_with("Oasis-Token="));
    if !has_oasis_token {
        tracing::error!(
            got = ?cookie_parts.iter().map(|p| p.split('=').next().unwrap_or("?")).collect::<Vec<_>>(),
            "cookie 不完整 (缺 Oasis-Token),不写入"
        );
        return Err(t!(
            "stepfun_login.cookies_incomplete",
            has_oasis_token = has_oasis_token
        )
        .into_owned());
    }

    let cookie_str = cookie_parts.join("; ");

    let cred = Credentials {
        api_key: None,
        cookie: Some(cookie_str.clone()),
        secret_key: None,
    };
    config::save_credential_for_id("stepfun", &cred)
        .map_err(|e| t!("stepfun_login.save_keys_failed", err = e.to_string()).into_owned())?;

    Ok(cookie_str.len())
}

fn emit_failed(app: &AppHandle, msg: String) {
    tracing::error!(error = %msg, "stepfun login flow failed");
    let _ = app.emit("musage://stepfun-login-failed", msg);
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
        assert!(is_dashboard_url(&url("https://platform.stepfun.com/")));
        assert!(is_dashboard_url(&url(
            "https://platform.stepfun.com/account-overview"
        )));
        assert!(is_dashboard_url(&url(
            "https://platform.stepfun.com/interface-key"
        )));
    }

    #[test]
    fn dashboard_url_rejects_login_paths() {
        // /login 路径
        assert!(!is_dashboard_url(&url("https://platform.stepfun.com/login")));
        // /signin
        assert!(!is_dashboard_url(&url("https://platform.stepfun.com/signin")));
        // /signup
        assert!(!is_dashboard_url(&url("https://platform.stepfun.com/signup")));
        // passport 关键字
        assert!(!is_dashboard_url(&url("https://passport.stepfun.com/")));
    }

    #[test]
    fn dashboard_url_rejects_unrelated_hosts() {
        assert!(!is_dashboard_url(&url("https://example.com/dashboard")));
        assert!(!is_dashboard_url(&url("https://platform.stepfun.ai/")));
        // platform.stepfun.ai 是海外站,不是 platform.stepfun.com
    }

    #[test]
    fn dashboard_url_rejects_account_domain() {
        // 关键回归:account.stepfun.com 是登录域(不是 dashboard 域),
        // Oasis-Token cookie 只在 platform.stepfun.com 域下,不能让
        // account 域误判为 dashboard 触发 cookie 提取。
        assert!(!is_dashboard_url(&url(
            "https://account.stepfun.com/login?redirect=..."
        )));
        assert!(!is_dashboard_url(&url("https://account.stepfun.com/")));
    }

    #[test]
    fn dashboard_url_rejects_non_https() {
        assert!(!is_dashboard_url(&url("http://platform.stepfun.com/")));
    }

    #[test]
    fn wanted_cookies_list_is_non_empty() {
        // 防御性检查:白名单不能被改空
        assert!(!WANTED_COOKIES.is_empty());
        assert!(WANTED_COOKIES.len() >= 1, "白名单至少 1 项才合理");
        assert!(
            WANTED_COOKIES.contains(&"Oasis-Token"),
            "Oasis-Token 是 auth-critical cookie,必须在白名单"
        );
    }

    #[test]
    fn window_label_matches_capability() {
        // capability 文件里 windows 数组必须含这个 label,否则 webview 拿不到权限
        assert_eq!(WINDOW_LABEL, "stepfun-login");
    }
}
