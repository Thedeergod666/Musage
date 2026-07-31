//! AnySearch Session JWT 一键提取 —— 应用内 WebView 登录
//!
//! 用户在设置面板点 “🔑 登录 AnySearch” → 弹一个 webview 窗口指向
//! `anysearch.com/login` → 用户正常登录 → 后端从 webview 的 **localStorage**
//! 抽出 session JWT → 写进 keys.json → 关 webview → emit
//! `musage://anysearch-login-success` 事件。
//!
//! ## 为什么不走 cookie（跟 Xiaomi 不同）
//!
//! AnySearch 的 console API（`/api/api/user/keys`）鉴权**只**认
//! `Authorization: Bearer <jwt>`，而这个 JWT 存在浏览器 **localStorage**
//! （`search-template-auth-state.state.accessToken`），**不在 cookie jar 里**
//! （cookie 只有 _ga / _fbp 等分析 cookie）。所以 [`xiaomi_login`] 那套
//! `cookies_for_url()` 在这里拿不到东西。
//!
//! ## 提取通道：`MUSAGE_TOKEN` cookie 中转
//!
//! 调研第一版让 init script 写 `document.title`、Rust 调 `window.title()` 读。
//! **实测失败**：`tauri::WebviewWindow::title()` 在 Tauri 2 里读的是 **OS 窗口标题**
//! （被 Rust 侧设为 "登录 AnySearch - Musage"），跟 `document.title` 是两套 API。
//! 轮询永远拿不到 SPA 写的值。
//!
//! 改用 cookie 中转（跟 xiaomi 的 `cookies_for_url()` 同套机制）：
//!
//! 1. init script 起 `setInterval` 监听 localStorage（key=`search-template-auth-state`）
//!    → 一旦 `.state.accessToken` 出现，就写一个**同源 cookie** `MUSAGE_TOKEN`。
//!    cookie 值是 **`<access>...<refresh>`**（有 refreshToken 时）或裸 access（没有时）——
//!    `...` 哨兵分隔符跟 StepFun combined-token 约定一致（`...` 不是 base64url
//!    合法字符，绝不跟 JWT 自身内容冲突）。
//! 2. Rust 端用 [`tauri::WebviewWindow::cookies_for_url`] 读 cookie jar → 按白名单
//!    `MUSAGE_TOKEN` name 拿 value → 校验形态 → 保存
//!
//! ## 为什么要连 refreshToken 一起抓（AnySearch 特有）
//!
//! AnySearch 的 access token 是 **OAuth 短命令牌，寿命仅 30 分钟**（实测 JWT
//! `exp - iat = 1800s`）。只存 access → 用户出门吃个饭回来必掉线（浮窗 401）。
//! auth-state 里同时有一个长效 `refreshToken`，配 `POST /api/ssuser/auth/refresh`
//! 端点（body `{refresh_token}` → 返新的 `{access_token, refresh_token,
//! expires_in_seconds}`）可换新的 access。所以这里把 access + refresh 一起抓下来
//! （combined 存进 cookie 槽位），provider 侧在 access 快过期时主动 refresh
//! （详见 [`crate::providers::anysearch`]）。
//!
//! ⚠️ **refresh token 是单次轮换的（single-use rotation）**：每次 refresh 都换发
//! 一个新的 refresh_token 并作废旧的（实测旧 token 复用返 `40114 revoked`）。
//! 所以 provider refresh 成功后**必须**把新的 refresh_token 原子写回 keys.json，
//! 否则下一轮就废了。
//!
//! 用 `setInterval` 而不是 `on_page_load` 是因为 AnySearch 登录后是 **SPA 客户端
//! 跳转**（pushState，不触发 document load），`on_page_load` 不会再 fire；interval
//! 装在一次 document load 上后，整个 SPA 生命周期都活着，客户端跳转写 localStorage
//! 也能在 500ms 内被捕获。
//!
//! init script 的 `document.cookie` / `Storage.getItem` override 只在
//! `www.anysearch.com` 域安装（L12 fix：非受信域保持原生行为 —— 旧版一律
//! 返 null，会破坏 SSO/OAuth 页面读自己的 storage；跨域偷读本来就有同源
//! 隔离）。
//!
//! ## 并发 / 重入
//!
//! 用户重复点按钮 → gen+1 并先关旧窗口 → 开新窗口 + 新任务。旧轮询任务
//! 每轮检查 generation（`GEN`），不等即静默退出（L1/L-gen fix：窗口同
//! label 复用会让 `get_webview_window().is_none()` 检查命中新窗口，单靠
//! 它旧任务不退出，可能把旧 token 存盘或误报 failed）。`DONE` 标记保证
//! 成功后不再处理残留读。
//!
//! ## 重新登录 / 过期 token
//!
//! webview profile 持久化 localStorage —— 上一次（可能已过期）的 JWT 会残留。
//! 不清理就重开登录窗，interval 会立刻把旧 JWT 写进中转 cookie，Rust 抓到存盘
//! → 浮窗继续 401、窗口「弹出即消失」。fix：init script 在 document_start 删旧
//! auth state + 清旧 cookie，并置 `MUSAGE_READY` 标记；Rust 轮询见到 READY 才
//! 接受 token，保证抓到的一定是清理后新登录的 JWT。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tauri::webview::Cookie;
use tauri::{AppHandle, Emitter, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::time::sleep;

use crate::config;
use crate::providers::Credentials;
use crate::t;

/// 全局完成标记：提取成功后置 true，轮询任务退出。
static DONE: AtomicBool = AtomicBool::new(false);

/// generation 计数器：每次 `open_anysearch_login_window` +1。
///
/// fix (2026-07-28 审查 L1/L-gen)：重新登录时旧窗口关闭、新窗口用**同
/// label** build —— 旧轮询任务的 `get_webview_window(label).is_none()`
/// 检查会命中新窗口（返 Some）→ 旧任务不退出，可能把旧 token 存盘 /
/// 误报 failed / 跟新任务竞争。引入 gen：旧任务每轮轮询 + emit 前检查，
/// 不等即静默退出。
static GEN: AtomicU64 = AtomicU64::new(0);

/// 本次流程是否仍是最新一次开窗（gen 未变）。
fn is_current_gen(my_gen: u64) -> bool {
    GEN.load(Ordering::SeqCst) == my_gen
}

/// fix (2026-07-28 审查 L9)：panic 兜底 guard —— 轮询任务任意退出路径
/// （正常 / Cancelled / Failed / panic unwind）都确保窗口被关闭；panic 时
/// 额外打 error 日志。正常路径各分支已显式 close（幂等），guard 主要在
/// panic / 未来新增提前 return 的路径上兜底。
struct WindowCloseGuard(tauri::WebviewWindow);

impl Drop for WindowCloseGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            tracing::error!("anysearch 登录轮询任务 panic，guard 兜底关窗");
        }
        let _ = self.0.close();
    }
}

/// 等旧窗口真正关闭（`close()` 是异步的，慢机器上固定 sleep 100ms 未必够，
/// 同 label build 可能失败 / 竞态）。50ms × 40 ≈ 2s 上限；超时兜底继续，
/// build 失败会把错误透传给前端，不会卡死。
/// fix (2026-07-28 审查 L7)。
async fn wait_window_closed(app: &AppHandle, label: &str) {
    for _ in 0..40 {
        if app.get_webview_window(label).is_none() {
            return;
        }
        sleep(Duration::from_millis(50)).await;
    }
    // M1 fix (2026-07-30 audit): 超时兜底再走 2s 后,若窗口仍存在则强制
    // destroy(WebView2 异步 close 在 sandbox / DevTools 关掉等场景下可能拖
    // >2s)。不 destroy 会泄露 WebView2 process + profile 目录(每次重新登录
    // 都堆一份),且下次 build 同 label 返 Err → 用户看到红色 toast 不明所以。
    // destroy 是同步 drop,不 await,百毫秒内必回收。
    if let Some(w) = app.get_webview_window(label) {
        tracing::warn!(label = label, "wait_window_closed 超时 2s,强制 destroy 防 webview 泄漏");
        let _ = w.destroy();
        // 重建后极短时间再确认一次,有些平台 destroy 后句柄还没完全 drop
        for _ in 0..10 {
            if app.get_webview_window(label).is_none() {
                return;
            }
            sleep(Duration::from_millis(50)).await;
        }
    }
}

/// 登录入口 URL。直接落到登录页；已登录时 SPA 会自行跳去 console。
const LOGIN_URL: &str = "https://www.anysearch.com/login";

/// webview 窗口 label（capability 按此授权 create-webview-window）。
const WINDOW_LABEL: &str = "anysearch-login";

/// localStorage 里存登录态的 key（实测自 anysearch.com console）。
const STORAGE_KEY: &str = "search-template-auth-state";

/// init script 把 JWT 写到这个 cookie，Rust 端 `cookies_for_url` 读。
/// 命名带 MUSAGE_ 前缀避免跟 anysearch 自己设的 cookie 撞名，值就是 JWT。
const COOKIE_NAME: &str = "MUSAGE_TOKEN";

/// 「清理完成」握手标记 cookie。init script 在 document_start 清掉上一次
/// 残留的 auth state 后写 `MUSAGE_READY=1`；Rust 轮询**见到 READY 才开始接受**
/// `MUSAGE_TOKEN`。这样能堵住一个竞态：webview profile 持久化了上一次的
/// `MUSAGE_TOKEN` cookie，若轮询在 init script 清理之前就读 cookie，会抓到
/// 过期 token 存盘 → 浮窗继续 401、登录窗口「弹出即消失」。READY 保证
/// token 一定是清理之后新写入的。
const READY_COOKIE_NAME: &str = "MUSAGE_READY";

/// JWT 形态校验：`eyJ` 开头 + 长度合理（≥ 20，挡 3-char 短串）+ ≤ 4096 +
/// 无空白 / 控制字符。挡掉 interval 还没拿到 token 时的空串、init script
/// 抓脏字符、以及任何明显不是真实 JWT 的极短串。长度下限是**合理性门槛**，
/// 不是 JWT 规范强制的最小值（实测 AnySearch JWT ≈ 572 字符）。
fn is_jwt_like(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with("eyJ")
        && s.len() >= 20
        && s.len() <= 4096
        && s.chars().all(|c| !c.is_whitespace() && !c.is_control())
        // fix (2026-07-28 审查 L11): 至少两段 `.` 分隔（JWT 的
        // header.payload.sig；combined token 的 `...` 哨兵分段更多），
        // 挡掉无分段的脏 base64 串
        && s.matches('.').count() >= 2
}

/// 注入页面的 init script：
/// - 把 cookie / storage 读取锁在受信 host（挡第三方 tracker 偷 JWT）
/// - **打开即清理**：删 localStorage 里上一次残留的 auth state + 清旧中转
///   cookie，然后置 `MUSAGE_READY` 标记（重新登录不被过期 token 污染）
/// - 起 interval：localStorage 一旦出现 JWT 就写同源 cookie `MUSAGE_TOKEN`；
///   没 token 时把该 cookie 清成空（让 Rust 知道「还在等」）
fn init_script() -> String {
    // 设计：**localStorage → cookie 中转** + Rust 用 `cookies_for_url` 读。
    //
    // 之前的设计让 init script 写 `document.title`，Rust 调 `window.title()` 读。
    // 但 Tauri 2 的 `WebviewWindow::title()` 读的是 **OS 窗口标题**（被 Rust 设为
    // "登录 AnySearch - Musage"），不是页面的 `document.title`。所以轮询
    // 永远拿不到 init script 写的值，整个通道失效。
    //
    // 新方案：init script 检测到 token 后写一个**同名 cookie**
    // (`MUSAGE_TOKEN=<jwt>`)，Rust 走 `window.cookies_for_url()` 读（xiami 那套
    // 已验证可用）。同源 cookie，Rust 端可读（含 HttpOnly——虽然我们设的不是
    // HttpOnly，但 `cookies_for_url` 走的是 webview 的底层 cookie store）。
    //
    // 注意：JS 里有大量字面 `{}`（空 catch 块 / 对象字面量），所以**不能**用
    // format!（单花括号会被当占位符）。改用唯一占位符 + replace() 注入。
    const JS: &str = r#"
        (function () {
            var ALLOW_HOST = "www.anysearch.com";
            var COOKIE_NAME = "__MUSAGE_COOKIE_NAME__";
            var LS_KEY = "__MUSAGE_LS_KEY__";
            var READY_NAME = "__MUSAGE_READY_NAME__";
            function isAllowed() {
                try { return location.hostname === ALLOW_HOST; } catch (_) { return false; }
            }
            // ── 锁 cookie / storage 读取到受信 host（挡第三方 tracker 偷 JWT）──
            try {
                // D3-001 fix (2026-07-30 audit): 锁 Document.prototype.cookie
                // 而非 document.cookie (instance). 之前 instance-level override
                // 可被 'Object.getOwnPropertyDescriptor(Document.prototype, "cookie")
                // .get.call(document)' 绕过 → 同源 XSS 能直接读 MUSAGE_TOKEN
                // (非 HttpOnly). 锁 prototype + configurable: false 防 redef.
                var _origCookie = Object.getOwnPropertyDescriptor(Document.prototype, "cookie");
                Object.defineProperty(Document.prototype, "cookie", {
                    get: function () { return isAllowed() ? _origCookie.get.call(this) : ""; },
                    set: function (v) { if (isAllowed()) _origCookie.set.call(this, v); },
                    configurable: false
                });
            } catch (_) {}
            try {
                var _origGet = Object.getOwnPropertyDescriptor(Storage.prototype, "getItem");
                Object.defineProperty(Storage.prototype, "getItem", {
                    value: function (k) { return isAllowed() ? _origGet.value.call(this, k) : null; },
                    configurable: true
                });
            } catch (_) {}
            // ── 重新登录：清掉上一次残留的登录态（关键 fix）──
            // webview profile 持久化 localStorage —— 上一次（可能已过期）的 JWT
            // 还在 LS_KEY 里。不清的话下面的 interval 会立刻把它写进中转
            // cookie，Rust 抓到过期 token 存盘 → 浮窗继续 401，且登录窗口
            // 「弹出即消失」。所以每次打开都从干净状态开始：
            //   1) 删 localStorage 里的旧 auth state（强制重新登录）
            //   2) 清掉旧的中转 cookie
            //   3) 置 MUSAGE_READY 标记 —— Rust 见到 READY 才开始接受 token，
            //      保证不会抓到清理之前残留在 cookie store 里的旧 MUSAGE_TOKEN
            // 只在受信 host 上清（isAllowed 守卫），不碰第三方数据。
            try {
                if (isAllowed()) {
                    localStorage.removeItem(LS_KEY);
                    document.cookie = COOKIE_NAME + "=; path=/; max-age=0";
                    document.cookie = READY_NAME + "=1; path=/; max-age=3600; SameSite=Lax";
                }
            } catch (_) {}
            // ── 读 token ──
            // 返回 `access...refresh`（有 refreshToken 时）或裸 access（没有时）。
            // 分隔符 `...` 跟 StepFun 的 combined-token 约定一致：`...` 不是
            // base64url 合法字符（JWT 只含 A-Za-z0-9-_ + '.'），所以拿它当哨兵
            // 分隔两段 token 绝不会跟 token 自身内容冲突。Rust 端按 `...` split
            // 出 access / refresh 两半：access 用来请求 + 本地读 exp，refresh 用来
            // 到期时换新（AnySearch access token 仅 30 分钟寿命）。
            function readToken() {
                try {
                    var raw = localStorage.getItem(LS_KEY);
                    if (!raw) return "";
                    var st = JSON.parse(raw);
                    var s = (st && st.state) || {};
                    var access = s.accessToken || "";
                    if (!access) return "";
                    var refresh = s.refreshToken || "";
                    return refresh ? (access + "..." + refresh) : access;
                } catch (_) { return ""; }
            }
            // ── 每 500ms 把 token 写到 cookie（覆盖式，方便 Rust 端轮询读）──
            // 同时清掉上一次写入的过期 cookie 防止积累；token 为空时清 cookie（让
            // Rust 知道"还在等"）。
            // D3-006 fix (2026-07-30 audit): 拿到 interval handle, 首次成功写
            // 非空 token 后 clearInterval. 之前永远 24h × 2 次/秒 = 17 万次
            // cookie store fsync. SPA 跳转 / 新 tab 会叠加新 interval (init script
            // 在每个 document_start 跑), webview 关才停, 资源浪费.
            var _musageIv = setInterval(function () {
                if (!isAllowed()) return;
                var tok = readToken();
                try {
                    if (tok) {
                        document.cookie = COOKIE_NAME + "=" + tok + "; path=/; max-age=3600; SameSite=Lax";
                        // 首次成功写 → 停 interval, Rust 端会读 cookie 后关窗
                        clearInterval(_musageIv);
                    } else {
                        document.cookie = COOKIE_NAME + "=; path=/; max-age=0";
                    }
                } catch (_) {}
            }, 500);
        })();
        "#;
    JS.replace("__MUSAGE_COOKIE_NAME__", COOKIE_NAME)
        .replace("__MUSAGE_LS_KEY__", STORAGE_KEY)
        .replace("__MUSAGE_READY_NAME__", READY_COOKIE_NAME)
}

/// 打开登录 webview 窗口。
///
/// 行为：
/// 1. 已有 `anysearch-login` 窗口 → 先关（重新登录 / 刷新 token 场景）
/// 2. 开新 webview 指向 `LOGIN_URL`，注入 init script（清旧登录态 + 置
///    `MUSAGE_READY` + cookie 中转 + tracker 防护）
/// 3. spawn 一个轮询任务：见到 `MUSAGE_READY` 后读 cookie jar，命中合法的
///    `MUSAGE_TOKEN`（JWT）→ 校验 → 写 keys.json → 关窗口 → emit 成功；
///    窗口被关 / 超时 → 静默退出
///
/// 错误（仅“写盘失败”这类真错误）通过 `musage://anysearch-login-failed` 返回前端。
/// 用户主动关窗 / 没登录不算错误，不弹红条。
#[tauri::command]
pub async fn open_anysearch_login_window(app: AppHandle) -> Result<(), String> {
    // 新一轮流程：gen+1（老轮询任务见 gen 不等即静默退出 —— 同 label 新窗口
    // 会让旧的 `get_webview_window().is_none()` 检查失效）
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    DONE.store(false, Ordering::SeqCst);

    // 已开过 → 先关（旧轮询任务因 gen 不等 / 窗口句柄失效退出）
    if let Some(existing) = app.get_webview_window(WINDOW_LABEL) {
        let _ = existing.close();
        wait_window_closed(&app, WINDOW_LABEL).await;
    }

    let url: Url = LOGIN_URL
        .parse::<Url>()
        .map_err(|e| t!("anysearch_login.parse_login_url", err = e.to_string()).into_owned())?;

    // D3-003 fix (2026-07-30 audit): 同 xiaomi
    let b = WebviewWindowBuilder::new(&app, WINDOW_LABEL, WebviewUrl::External(url))
        .title(t!("window.anysearch_login").to_string())
        .inner_size(960.0, 720.0)
        .min_inner_size(640.0, 540.0)
        .resizable(true)
        .decorations(true)
        .center()
        .skip_taskbar(true);
    let b = match app.get_webview_window("settings") {
        Some(p) => b.parent(&p).map_err(|e| format!("anysearch login parent: {e}"))?,
        None => b,
    };
    let window = b
        .initialization_script(init_script())
        .build()
        .map_err(|e| t!("anysearch_login.build_webview", err = e.to_string()).into_owned())?;

    // 轮询任务：读 cookie jar 抽 JWT
    let app2 = app.clone();
    let window_clone = window.clone();
    tauri::async_runtime::spawn(async move {
        // L9 fix (2026-07-28 审查): panic 兜底 guard —— 任意退出路径(正常 /
        // Cancelled / Failed / panic unwind)都确保窗口被关闭;panic 时额外打
        // error 日志。正常路径各分支已显式 close(幂等),guard 主要在 panic /
        // 未来新增提前 return 的路径上兜底。
        let _close_guard = WindowCloseGuard(window_clone.clone());
        let my_gen = gen;
        let result = poll_token_from_cookie(&app2, &window_clone, my_gen).await;
        // gen 已被新流程取代 → 静默退出,不发任何事件,不要 close(新窗口接管)
        if !is_current_gen(my_gen) {
            tracing::debug!(my_gen, "anysearch 老轮询流程被新流程取代,静默退出");
            return;
        }
        match result {
            PollOutcome::Saved(len) => {
                DONE.store(true, Ordering::SeqCst);
                tracing::info!(len, "anysearch JWT 提取 + 保存成功");
                // 立即拉一次（让浮窗立刻看到数据）
                if let Err(e) = crate::commands::refresh_single_inner(&app2, "anysearch", crate::poller_backoff::RefreshSource::Manual).await {
                    tracing::warn!(error = %e, "登录后立即拉取失败（不阻塞成功事件）");
                }
                let _ = window_clone.close();
                let _ = app2.emit("musage://anysearch-login-success", len);
            }
            // 用户关窗 / 窗口被新流程取代 —— 静默退出
            PollOutcome::Cancelled => {
                tracing::debug!("anysearch 登录窗口已关闭或被取代，未提取到 token");
            }
            // D3-002 fix (2026-07-30 audit): 超时 emit failed 让前端弹 toast
            PollOutcome::Timeout(reason) => {
                if !DONE.load(Ordering::SeqCst) {
                    tracing::warn!(reason = %reason, "anysearch 登录超时");
                    let _ = app2.emit("musage://anysearch-login-failed", reason);
                }
            }
            PollOutcome::Failed(e) => {
                if !DONE.load(Ordering::SeqCst) {
                    tracing::error!(error = %e, "anysearch login flow failed");
                    let _ = app2.emit("musage://anysearch-login-failed", e);
                }
            }
        }
    });

    Ok(())
}

enum PollOutcome {
    Saved(usize),
    /// 用户主动关窗 / 窗口被新流程取代 —— 静默退出
    Cancelled,
    /// D3-002 fix (2026-07-30 audit): 14min 硬上限达到 —— emit failed
    /// 让前端能区分"超时"和"用户主动关",否则用户被静默丢下
    Timeout(String),
    Failed(String),
}

/// 轮询 webview cookie jar 直到抽到合法 JWT 或窗口消失 / 超时。
///
/// init script 每 500ms 把 JWT 写进 `MUSAGE_TOKEN` cookie（同源 `www.anysearch.com`）；
/// 这里用 [`tauri::WebviewWindow::cookies_for_url`] 读 cookie jar（跟 xiaomi
/// 同一套机制，跨平台稳定）。窗口被用户关掉 → `get_webview_window` 返 None
/// / `cookies_for_url` 报错 → Cancelled。
async fn poll_token_from_cookie(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
    my_gen: u64,
) -> PollOutcome {
    // 安全上限：~14 分钟，防窗口句柄异常残留时任务永不退出。
    const MAX_ITERS: u32 = 1200;
    // cookies_for_url 需要一个 URL —— anysearch.com 任何 URL 都返同一 cookie jar
    let probe_url: Url = LOGIN_URL.parse().unwrap_or_else(|_| {
        Url::parse("https://www.anysearch.com/").expect("hardcoded URL parses")
    });

    // 首次读取前先让出 ~1.5s，等导航到 document_start、init script 跑完
    // 「清旧 MUSAGE_TOKEN + 置 MUSAGE_READY」。webview profile 会持久化上一次
    // 的 MUSAGE_TOKEN cookie；若一开窗就立刻读，可能先于清理抓到那个过期 token
    // （「弹出即消失 + 信息不更新」bug）。登录页是轻量 SPA，1.5s 足够到
    // document_start；真正的 token 写入（用户手动登录）远晚于此，不会错过。
    sleep(Duration::from_millis(1500)).await;
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
        // L-gen fix (2026-07-28 审查): gen 已被新流程取代 → 静默退出,
        // 不写盘、不 emit,让 WindowCloseGuard 接管关窗
        if !is_current_gen(my_gen) {
            tracing::debug!(my_gen, "anysearch 轮询 gen 失效,静默退出");
            return PollOutcome::Cancelled;
        }

        // 读 cookie jar。webview.cookies_for_url 是异步的（在 Tauri 2 里是
        // blocking call on platform thread → 我们包在 spawn_blocking 里，或直接
        // 接受其同步语义。参考 xiaomi_login.rs:302 是 .await 调用，是同步阻塞
        // 包装在 async fn 里，Tauri runtime 会处理）。
        // H1 fix (2026-07-29 审查): cookies_for_url 失败不直接 Cancelled,
        // 而是 continue 下一轮重试。Tauri 2 文档明确该调用可能在以下场景
        // 暂态 Err:
        //   - webview 刚创建、cookie store 还没初始化 (前 100ms)
        //   - URL 解析 race (probe_url 在 webview 加载完成前)
        //   - macOS WKWebView 短暂 unreachable (主线程 dispatch 延迟)
        // 之前遇到就 Cancelled,常见「登录按钮点了但网页还没加载就报错」bug。
        // 改成 continue: 下一轮会重新检查窗口是否还存在 (line 268),窗口
        // 真没了再走 Cancelled;窗口还在 + Err 暂态 → sleep 700ms 重试。
        // 注意: 用 sleep 而不是 continue 立即重试,避免错误循环 100% CPU。
        let cookies: Vec<Cookie<'static>> = match window.cookies_for_url(probe_url.clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "读 webview cookies_for_url 暂态失败, 700ms 后重试");
                sleep(Duration::from_millis(700)).await;
                continue;
            }
        };

        // 握手：init script 清完上一次残留的 auth state 后才写 MUSAGE_READY。
        // 没见到 READY 就不读 token —— 否则可能抓到清理之前残留在 cookie store
        // 里的过期 MUSAGE_TOKEN（「弹出即消失 + 信息不更新」bug 的根因）。
        if !cookies.iter().any(|c| c.name() == READY_COOKIE_NAME) {
            sleep(Duration::from_millis(700)).await;
            continue;
        }

        if let Some(tok) = cookies.iter().find(|c| c.name() == COOKIE_NAME) {
            // cookie value 可能带引号（macOS WKWebView 习惯），剥掉
            let raw = tok.value().trim_matches('"');
            if is_jwt_like(raw) {
                return match save_token(raw) {
                    Ok(len) => PollOutcome::Saved(len),
                    Err(e) => PollOutcome::Failed(e),
                };
            }
            // cookie 在但不是 JWT（空 / 脏字符）—— 继续等
            tracing::debug!(
                len = raw.len(),
                "MUSAGE_TOKEN cookie 存在但形态不合法，继续轮询"
            );
        }
        // 没有 MUSAGE_TOKEN cookie = 用户还没登录或 interval 还没写 —— 继续等

        sleep(Duration::from_millis(700)).await;
    }

    // D3-002 fix (2026-07-30 audit): 返 Timeout 而不是 Cancelled, 让上层 emit
    // failed event 给前端。Max iters 14 min 是为了防止窗口句柄异常残留
    // 时任务永不退出; 真超时场景用户应该看到反馈而不是窗口消失无任何提示。
    tracing::warn!("anysearch 登录轮询达到 14min 安全上限, 通知前端");
    PollOutcome::Timeout(anysearch_timeout_reason())
}

/// D3-002 fix (2026-07-30 audit): 超时原因走 i18n, 复用现有 key + 拼具体秒数
fn anysearch_timeout_reason() -> String {
    t!(
        "login.anysearch.timeout",
        secs = 14 * 60
    )
    .into_owned()
}

/// 把抽到的 JWT 写进 keys.json 的 cookie 槽位。返回写入字节数。
fn save_token(token: &str) -> Result<usize, String> {
    let cred = Credentials {
        api_key: None,
        cookie: Some(token.to_string()),
        secret_key: None,
    };
    config::save_credential_for_id("anysearch", &cred)
        .map_err(|e| t!("anysearch_login.save_keys_failed", err = e.to_string()).into_owned())?;
    Ok(token.len())
}

// ── 单元测试（pure function） ───────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_like_accepts_real_jwt_shape() {
        // 典型 JWT 三段 base64，eyJ 开头
        let jwt = "eyJhbGciOiJFZERTQSIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1c3JfZm9vIn0.sig";
        assert!(is_jwt_like(jwt));
    }

    #[test]
    fn jwt_like_rejects_empty_and_short() {
        assert!(!is_jwt_like(""));
        assert!(!is_jwt_like("eyJ")); // 太短
        assert!(!is_jwt_like("notajwt")); // 不以 eyJ 开头
    }

    #[test]
    fn jwt_like_rejects_whitespace_and_control() {
        assert!(!is_jwt_like("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0\n.sig"));
        assert!(!is_jwt_like("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0 .sig"));
    }
}
