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
//! 在 future 末尾 Drop 时 reset EXTRACTING。tokio task panic 时 local
//! variables 仍然被 Drop(run by panic unwinding) → guard 兜底。
//!
//! L-gen fix (2026-07-28 审查): guard 携带本次流程的 generation，仅在 gen
//! 未变时才清锁 —— 用户重复点登录时，老任务的 guard drop / emit 不会清掉
//! 或误报新流程的状态（详见 `GEN` 注释）。
//!
//! ## Cookie 白名单
//!
//! 不在白名单里的 cookie 一律丢弃（最小权限）。
//! dashboard API 实际依赖的就这 4 个（参考 `providers/xiaomi.rs` 的注释）。
//! 平台如果改名 → 改这里就行，UI 不变。
//!
//! ## 「清除 Cookie → 重新登录」死循环（2026-09-28 fix）
//!
//! 本模块是 4 个登录模块里**唯一**没有新鲜度门的 —— stepfun `is_fresh_token`
//! / kimi `is_fresh_token` / anysearch `is_fresh_access` 都能解 token 的 exp，
//! 拒掉「cookie jar 里上一次会话的残留」；xiaomi 的 cookie 是 HttpOnly 会话
//! cookie，JS 读不到 exp，**本地无从判断新旧**，旧实现只查「4 个白名单 cookie
//! 存在且值非空」就写盘。
//!
//! 死循环链路（无需重启即可复现）：
//! 1. cookie 失效（浮窗报 `error.xiaomi.cookie_invalid_hint`）
//! 2. 用户点「清除 Cookie」→ 只删 keys.json 的 `{id}:cookie` 槽，
//!    **从不碰 webview cookie jar**
//! 3. 用户点「🔑 登录小米账号」→ webview 用的是 app 默认的**持久化**
//!    WKWebView data store → jar 里旧 cookie 还在
//! 4. `on_page_load` 命中 dashboard → 首次重试就提取成功 → 关窗 + toast
//!    「登录成功」→ keys.json 里是**同一份已失效的 cookie** → 浮窗依旧 401
//! 5. 用户无路可走（全工程没有任何 UI 能清 cookie jar）
//!
//! 两段修：
//! - **新鲜度门**（[`is_same_as_stored`]）：抓到的 cookie 与已存**逐 pair
//!   相同** → 拒绝写盘（归一化后比集合，绕开 cookie jar 枚举顺序漂移）。
//!   Err 走既有重试通道 —— 用户若在重试窗口内登出重登，新 cookie 立刻放行。
//! - **逃生口**（[`clear_xiaomi_session`]）：删凭据槽 + 逐个
//!   [`purge_cookies_for_domains`] 清 jar 里 xiaomi 域的 cookie。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// fix (2026-07-30 audit M4): panic 兜底 guard —— xiaomi on_page_load spawn 的
/// 提取任务任意退出路径(正常 / Cancelled / Failed / panic unwind)都确保窗口
/// 被关闭。**对齐 stepfun / anysearch 的同款 WindowCloseGuard 模式**(之前
/// anysearch + stepfun 加了,xiaomi 漏了),保证三个登录模块语义一致。
struct WindowCloseGuard(tauri::WebviewWindow);

impl Drop for WindowCloseGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            tracing::error!("xiaomi 登录提取任务 panic,guard 兜底关窗");
        }
        let _ = self.0.close();
    }
}

/// 全局完成标记：提取成功后置 true，后续 on_page_load 回调全部跳过。
/// 解决 macOS WKWebView 上 on_page_load 多次触发导致的
/// "failed to receive message from webview" 错误——窗口被第一个成功
/// 任务关闭后，后续回调不再尝试操作已销毁的 webview。
static DONE: AtomicBool = AtomicBool::new(false);

/// generation 计数器：每次 `open_xiaomi_login_window` +1。
///
/// fix (2026-07-28 审查 L-gen/L8)：用户重复点登录时，旧实现有两个跨流程
/// 竞态 —— ① 老任务的 `ExtractingGuard` drop 无条件清 EXTRACTING，会把
/// 新流程的锁清掉 → 并发提取；② 失败分支只看 DONE，老任务可能在新流程
/// 进行中误报 failed。引入 gen 后：老任务的 guard drop / 轮询 / emit 见
/// 到 gen 不等即静默退出或跳过。
static GEN: AtomicU64 = AtomicU64::new(0);

/// 本次流程是否仍是最新一次开窗（gen 未变）。
fn is_current_gen(my_gen: u64) -> bool {
    GEN.load(Ordering::SeqCst) == my_gen
}

/// RAII guard: Drop 时 reset `EXTRACTING`（仅当 generation 未变）。
///
/// H3 fix: tokio spawn 的 task panic 时,虽然 tokio 自己会打印 panic 信息 +
/// propagate 给 spawn handle,但我们 spawn 时没 await handle → panic 后
/// task 内部的局部变量仍被 Drop (Rust unwinding 时跑 Drop glue)。把
/// EXTRACTING reset 放在 Drop 里 —— 任意路径退出(正常返回/Err/panic)
/// 都会清掉锁,保证下次用户点登录能 compare_exchange 成功。
///
/// L-gen fix (2026-07-28 审查): guard 携带本次流程的 generation —— 老流程
/// 的 guard drop 不能清新流程的锁（否则用户重复点登录会出现并发提取任务）。
struct ExtractingGuard(u64);

impl ExtractingGuard {
    fn new(gen: u64) -> Self {
        Self(gen)
    }
}

impl Drop for ExtractingGuard {
    fn drop(&mut self) {
        if is_current_gen(self.0) {
            EXTRACTING.store(false, Ordering::SeqCst);
        }
    }
}

/// 登录入口 URL。直接定位到 dashboard 的"订阅管理"页。
const LOGIN_URL: &str = "https://platform.xiaomimimo.com/console/plan-manage";

/// 判定 URL 是否已经离开 SSO 重定向链、到达 dashboard。
///
/// 规则（白名单 + 黑名单组合）：
/// - host 必须**完全等于** `platform.xiaomimimo.com`（不接受子串匹配；
///   否则 `platform.xiaomimimo.com.attacker.tld` DNS rebinding 可绕过）
/// - scheme 必须是 `https`（防明文 / 钓鱼）
/// - 不能在 `account.xiaomi.com` / `serviceLogin` / `passport` 路径上
///
/// 这是 heuristic，不是绝对 —— 如果 Xiaomi 改了 SSO 流程（比如加一层
/// 验证中间页），要改这里或加新关键字。
fn is_dashboard_url(url: &Url) -> bool {
    let host_ok = url.host_str() == Some("platform.xiaomimimo.com") && url.scheme() == "https";
    let s = url.as_str();
    let not_login =
        !s.contains("account.xiaomi.com") && !s.contains("serviceLogin") && !s.contains("passport");
    host_ok && not_login
}

/// P3 audit fix (2026-08-13): SSO 回调 / dashboard URL 的 query 可能带
/// 一次性 ticket / code 等凭据, 落盘 app_log.jsonl 会随 bug report 外泄。
/// 日志只记 scheme://host/path, query/fragment 用 `<redacted>` 占位。
fn redact_url_for_log(url: &Url) -> String {
    let mut s = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    let p = url.path();
    if !p.is_empty() {
        s.push_str(p);
    }
    if url.query().is_some() || url.fragment().is_some() {
        s.push_str("?<redacted>");
    }
    s
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
/// 错误通过 `musage://xiaomi-login-failed` 事件返回给前端；用户主动关窗 /
/// 被新一轮登录流程取代 → 静默退出（L2/L-gen fix），不弹错误条。
///
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
        tracing::warn!(
            label = label,
            "wait_window_closed 超时 2s,强制 destroy 防 webview 泄漏"
        );
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

/// 解析本次登录要写入的凭据槽（base 或副本 unique_id）。
///
/// 2026-09-28 fix（契约 1/2：登录命令不带 instance id → 多账号凭据成为
/// UI 删不掉的孤儿）：base 被禁用 + 副本启用时，登录写进 base 槽而浮窗刷的是
/// 副本槽 → 用户看到「登录成功但卡片还是红的」，且副本槽永远没凭据可清。
/// 前端登录卡片手里就有这张卡的 unique_id，后端没理由不用。
///
/// 优先级：
/// 1. `instance_id` 合法 → 它（base 前缀校验在 resolve 内部做）
/// 2. base 启用 → base
/// 3. 按 index 升序第一个启用副本
/// 4. 全禁用 → `instance_id` 原样（用户明确点了某张卡，即使禁用也该写进去）
///    再兜底 base（保持旧行为）
async fn resolve_target(app: &AppHandle, base: &str, instance_id: Option<&str>) -> String {
    match crate::commands::resolve_login_refresh_target(&app.state(), base, instance_id).await {
        Some(t) => t,
        None => instance_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(base)
            .to_string(),
    }
}

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
/// 错误通过 `musage://xiaomi-login-failed` 事件返回前端；用户主动关窗 /
/// 被新一轮登录流程取代 → 静默退出（L2/L-gen fix），不弹错误条。
///
/// `instance_id`（2026-09-28 契约 1/2）：前端登录卡片传自己的 `unique_id`，
/// 副本行（`xiaomimimo#2`）必须带上，否则凭据落 base 槽、浮窗刷副本槽。
/// 标量参数走 camelCase（tauri-macros 的 `ArgumentCase::Camel`），
/// 前端 `{ instanceId: "xiaomimimo#2" }` 即可；不传 → None → 行为同旧版。
#[tauri::command]
pub async fn open_xiaomi_login_window(
    app: AppHandle,
    instance_id: Option<String>,
) -> Result<(), String> {
    // 新一轮流程：gen+1（老任务的 guard / 轮询 / emit 见到不一致即失效）
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    // 重置提取锁 + 完成标记（新窗口 = 全新流程）
    EXTRACTING.store(false, Ordering::SeqCst);
    DONE.store(false, Ordering::SeqCst);

    // 已开过 → 先关（重新登录场景）
    if let Some(existing) = app.get_webview_window("xiaomi-login") {
        let _ = existing.close();
        wait_window_closed(&app, "xiaomi-login").await;
    }

    let url: Url = (LOGIN_URL.parse::<Url>())
        .map_err(|e| t!("xiaomi_login.parse_login_url", err = e.to_string()).into_owned())?;

    // 2026-09-28: target 提前 resolve（on_page_load 是 Fn 非 FnOnce 闭包，
    // 只能按引用捕获 target，进 task 前必须 clone）。
    let target = resolve_target(&app, "xiaomimimo", instance_id.as_deref()).await;

    // 闭包必须 'static + Send + Sync → 克隆 AppHandle（内部 Arc 包装，廉价）
    let app_for_callback = app.clone();

    // D3-003 fix (2026-07-30 audit): parent 让登录窗附属设置窗(关设置
    // 窗 → 关登录窗 + 避免被设置窗完全遮挡), skip_taskbar(true) 因为
    // 登录是 transient dialog. settings 窗可能还没建(用户从系统托盘
    // 直接进登录), 这种情况降级到无 parent.
    let b = WebviewWindowBuilder::new(&app, "xiaomi-login", WebviewUrl::External(url))
        .title(t!("window.xiaomi_login").to_string())
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
            tracing::warn!(error = ?e, "xiaomi login parent 设置失败");
            format!("xiaomi login parent: {e:#}")
        })?,
        None => b,
    };
    b
        // 2026-09-28 fix（H8 / H-7 的净效果为负，整段删除 init script）：
        // 这段 override 现在只剩「把 `Document.prototype.cookie` 的
        // `configurable` 从 true（WebIDL 原生）改成 false，getter/setter
        // 逐字 passthrough」一件事 —— 见 4f54ee7：门控被改成 `if (!isAllowed())
        // return` 早返（这一步是对的，跨域 SSO 中间页不再被锁），但 override
        // 本身没恢复成真实现。
        //
        // 净效果全是负的：
        // - **防护价值为零**：xiaomi 抓的是 HttpOnly 会话 cookie，JS 本来就
        //   读不到；getter 又是 passthrough，页面脚本 `document.cookie` 照常
        //   拿到全部 cookie（真要做域隔离，这里必须返回过滤后的串）。
        // - **净破坏**：`platform.xiaomimimo.com` 上的页面脚本或第三方 widget
        //   调 `Object.defineProperty(Document.prototype, 'cookie', ...)`
        //   （严格模式 / ESM）会因 `configurable: false` 抛 TypeError，
        //   页面脚本中断。
        // 真正的边界防护在 WebView 实例本身（独立 webview + capabilities
        // 只授权这一个 label），不在 JS 层。anysearch_login.rs 的同款
        // override 一并删除（同一份 passthrough 论证）。
        .on_page_load(move |window, payload| {
            let url = payload.url();
            tracing::debug!(url = %redact_url_for_log(&url), "xiaomi login webview page load");

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

            tracing::info!(url = %redact_url_for_log(&url), "on_page_load: ✅ 命中 dashboard，启动 cookie 提取");
            let app2 = app_for_callback.clone();
            let window_clone = window.clone();
            let my_gen = gen;
            // on_page_load 是 `Fn`（WKWebView 上多次触发）→ 不能把 `target`
            // move 进 task，只能 clone 一份（String clone 很便宜）。
            let task_target = target.clone();
            tauri::async_runtime::spawn(async move {
                // M4 fix: panic 兜底关窗 —— 任意路径退出都强制关 webview
                let _close_guard = WindowCloseGuard(window_clone.clone());
                // H3 fix: 用 RAII guard 兜底 —— spawn 的 task panic 时
                // Rust 仍会跑局部变量的 Drop glue (除非 panic = abort 但
                // tokio 默认 unwind)。guard 在任意路径退出(正常返回/Err/panic)
                // 都会被 Drop,强制 reset EXTRACTING,保证下次用户点登录
                // compare_exchange 永远能成功。
                //
                // L-gen fix (2026-07-28 审查): guard 携带本次 gen —— 老流程
                // 的 drop 不能清新流程的锁(否则用户重复点登录会出现并发提取)。
                let _extracting_guard = ExtractingGuard::new(my_gen);
                // D7-02 fix (2026-09-07 audit) 的 resolve 已上移到
                // `open_xiaomi_login_window`（见 resolve_target）：on_page_load
                // 是 `Fn` 闭包，无法 await，且 4 个登录模块的 resolve 位置
                // 现在完全一致（都在开窗前一次）。
                let target = task_target;
                let result = extract_with_retry(&window_clone, &app2, my_gen, &target).await;
                // 注意: 不显式 EXTRACTING.store(false) —— ExtractingGuard
                // 的 Drop 已经做这件事,而且带 gen 检查 (is_current_gen)。
                // 显式 store 会无视 gen,老流程可能在新流程拿锁之后才跑到
                // 这里 → 把新流程的锁清掉 → 两个流程并发提取 (audit H1
                // 2026-07-29)。guard 在任意路径退出(正常/Err/panic)都跑
                // Drop,这里不再需要额外兜底。

                // gen 已被新流程取代 → 静默退出,不发任何事件
                if !is_current_gen(my_gen) {
                    tracing::debug!(my_gen, "xiaomi 老轮询/提取流程被新流程取代，静默退出");
                    return;
                }

                match result {
                    Ok(saved_len) => {
                        DONE.store(true, Ordering::SeqCst);
                        tracing::info!(saved_len, target = %target, "xiaomi cookie 提取 + 保存成功");
                        // D7-02 fix (2026-09-07 audit): target 已在 spawn 前 resolve,
                        // extract 已直接写到 target 槽,直接刷 target 即可。
                        if let Err(e) = crate::commands::refresh_single_inner(
                            &app2,
                            &target,
                            crate::poller_backoff::RefreshSource::Manual,
                        )
                        .await
                        {
                            tracing::warn!(error = %e, target = %target, "登录后立即拉取失败（不阻塞成功事件）");
                        }
                        // 关 webview
                        let _ = window_clone.close();
                        // 通知前端
                        let _ = app2.emit("musage://xiaomi-login-success", saved_len);
                    }
                    Err(e) => {
                        // 只有 DONE 为 false 时才报错（避免关闭后的残留任务触发误报）
                        if !DONE.load(Ordering::SeqCst) {
                            // D7-07 (2026-09-04 audit): 窗口已不存在（url() 读失败
                            // 的常见根因 = 用户主动关窗）→ 静默退出不弹红条，
                            // 对齐 stepfun/kimi/anysearch 的 Cancelled 语义；
                            // 窗口还在的错误（cookie 解析 / 写盘失败）照常报。
                            if window_clone.url().is_err() {
                                tracing::debug!("xiaomi 登录窗口已被用户关闭，静默取消");
                            } else {
                                emit_failed(&app2, e);
                            }
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
///
/// `my_gen` 透传本次开窗 generation —— 每轮重试前检查 `is_current_gen`，
/// 已被新流程取代就静默退出，不写盘、不 emit。
async fn extract_with_retry(
    window: &tauri::WebviewWindow,
    _app: &AppHandle,
    my_gen: u64,
    // D7-02 fix (2026-09-07 audit): 接收 caller resolve 的 refresh target。
    target: &str,
) -> Result<usize, String> {
    // 重试策略：1s, 2s, 2s, 3s, 3s（共 11s 覆盖大部分场景）
    let retry_delays = [1u64, 2, 2, 3, 3];

    for (attempt, delay) in retry_delays.iter().enumerate() {
        let attempt_num = attempt + 1;

        // 如果另一个任务已经成功，直接退出
        if DONE.load(Ordering::SeqCst) {
            return Err(t!("xiaomi_login.another_task_done").into_owned());
        }

        // 2026-08-03 audit (Darwin B7): 跟 hover emitter 同款 SHUTDOWN 检查
        if crate::poller::SHUTDOWN_NATIVE_THREADS.load(std::sync::atomic::Ordering::SeqCst) {
            tracing::debug!("xiaomi 提取流程收到 SHUTDOWN, 退出");
            return Err(t!("xiaomi_login.another_task_done").into_owned());
        }

        // gen 已被新流程取代 → 静默退出
        if !is_current_gen(my_gen) {
            tracing::debug!(my_gen, attempt_num, "xiaomi 提取流程 gen 失效，静默退出");
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
            // L1 fix (2026-09-05 audit)：URL 带 query（ticket/code/userId），
            // 走 redact，不裸打完整 URL（与同文件 290 行 P3 修复一致）。
            tracing::debug!(
                url = %redact_url_for_log(&current_url),
                attempt_num,
                "URL 不在 dashboard，跳过"
            );
            continue;
        }

        // 尝试提取
        match extract_and_save(window, my_gen, target).await {
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
///
/// M-20 fix (2026-09-05 audit)：携带 `my_gen` 用于写盘前最终 gen 复查。
/// D7-02 fix (2026-09-07 audit, 2 域独立命中, merge 2026-09-08 合并双修)：
/// 旧实现硬编码写 "xiaomimimo" base 槽,base 禁用 + 副本启用场景 refresh
/// 命中副本空槽 → 401 循环。caller 早期 resolve 传 target,save 直接写
/// target 槽让 refresh 命中（取代本地 H-8 mirror 双写方案）。
async fn extract_and_save(
    window: &tauri::WebviewWindow,
    my_gen: u64,
    target: &str,
) -> Result<usize, String> {
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
                // L2 fix (2026-09-05 audit)：userId 是账号标识，不再明文进日志
                //（只记长度），与 redact 策略一致。
                tracing::debug!(user_id_len = uid.len(), "从 URL 参数补充 userId 到 cookie");
                cookie_parts.push(format!("userId={uid}"));
            }
        }
    }

    // F4 fix: 在写入 keys.json 前做完整性校验。两个 auth-critical cookie 必须同时存在：
    // - api-platform_serviceToken：真正的认证 token
    // - userId：dashboard API 路由参数
    // 任何一个缺失就 return Err，不覆盖原有的有效 cookie（避免用户被锁在"看似登录了但 API 401"的状态）。
    //
    // L4 fix (2026-09-05 audit)：presence 检查之外再查 **value 非空** ——
    // `api-platform_serviceToken=`（空值）也能通过 starts_with，空值拼串覆盖
    // 有效旧凭据后浮窗 401。
    let service_token_value = cookie_parts
        .iter()
        .find(|p| p.starts_with("api-platform_serviceToken="))
        .and_then(|p| p.split_once('='))
        .map(|(_, v)| v)
        .unwrap_or("");
    let has_service_token = !service_token_value.is_empty();
    let user_id_value = cookie_parts
        .iter()
        .find(|p| p.starts_with("userId="))
        .and_then(|p| p.split_once('='))
        .map(|(_, v)| v)
        .unwrap_or("");
    let has_user_id = !user_id_value.is_empty();
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

    // ── 新鲜度门（2026-09-28 fix，见模块头「清除 Cookie → 重新登录」死循环）──
    // xiaomi 是 4 个登录模块里唯一解不出 token exp 的（HttpOnly 会话 cookie，
    // JS 读不到），所以唯一能判「这是不是新东西」的手段就是**跟已存的比**。
    // 抓到的 pair 集合与 `{target}:cookie` 完全相同 → 没有任何新信息，
    // 写盘 + emit「登录成功」只会把同一份已失效 cookie 再确认一遍，
    // 让用户陷在「登录成功 → 浮窗依旧 401 → 再登录」的循环里。
    //
    // 这里返 Err 而不是直接失败退出：Err 会进 `extract_with_retry` 的重试
    // 通道（1s/2s/2s/3s/3s）。用户在重试窗口里**在登录窗内登出再登入**，
    // 下一轮 cookie 变化 → 正常写盘。这是本模块唯一可用的「让用户自己
    // 打破僵局」路径，不能一上来就把窗口关了。
    let stored = config::load_credential_for_id(target)
        .ok()
        .flatten()
        .and_then(|c| c.cookie);
    if is_same_as_stored(&cookie_str, stored.as_deref()) {
        tracing::warn!(
            target = %target,
            len = cookie_str.len(),
            "抓到的 cookie 与 keys.json 已存内容逐 pair 相同 —— 判定为旧会话残留，拒绝写盘"
        );
        // 复用现有 key（本次不新增 i18n key）：语义上「没提取到新东西」与
        // 「没提取到预期 cookie」对用户是同一件事 —— 都是「这次登录白登了，
        // 请确认是否真的登录成功、在不在 dashboard 页」。
        return Err(t!("xiaomi_login.cookie_extraction_failed").into_owned());
    }

    let cred = Credentials {
        api_key: None,
        cookie: Some(cookie_str.clone()),
        secret_key: None,
    };
    // M-20 fix (2026-09-05 audit)：写盘前做最终 gen 复查（anysearch/stepfun
    // 已有同款）—— cookies_for_url / 文件 IO 期间用户重新点登录 gen bump，
    // 旧流程的写盘会覆盖新流程刚存的 token。
    if !is_current_gen(my_gen) {
        tracing::debug!("extract_and_save: gen 已被新流程取代，放弃写盘");
        return Err(t!("xiaomi_login.cancelled").into_owned());
    }
    // D7-02：直写 target 槽（base 或副本 unique_id），refresh 必命中。
    config::save_credential_for_id(target, &cred)
        .map_err(|e| t!("xiaomi_login.save_keys_failed", err = e.to_string()).into_owned())?;

    Ok(cookie_str.len())
}

fn emit_failed(app: &AppHandle, msg: String) {
    tracing::error!(error = %msg, "xiaomi login flow failed");
    let _ = app.emit("musage://xiaomi-login-failed", msg);
}

/// 把 `Cookie: a=1; b=2` 头字符串拆成**排序去重**后的 pair 集合。
///
/// 2026-09-28：新鲜度门必须比集合而不是比字符串 —— `cookies_for_url` 的
/// 枚举顺序在 WKWebView / WebView2 上都不保证跨调用稳定（同一份 cookie 两次
/// 拿到不同顺序），直接 `==` 比字符串会把「同一份 cookie」误判成「新值」，
/// 死循环照旧。
///
/// 同时剥掉 macOS WKWebView 习惯包的外层双引号（与 extract 时同款处理）。
fn normalized_cookie_pairs(cookie_header: &str) -> BTreeSet<String> {
    cookie_header
        .split(';')
        .map(|p| p.trim().trim_matches('"'))
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// 新鲜度门：抓到的 cookie 与 keys.json 已存的是**同一份**内容 → 拒收。
///
/// xiaomi 的 cookie 是 HttpOnly 会话 cookie，本地解不出 exp（stepfun / kimi /
/// anysearch 都有 JWT exp 可解），所以「跟已存的比」是唯一可用的判据。
/// `stored` 为 None / 空串（从没配过凭据）→ 一定放行。
fn is_same_as_stored(fresh: &str, stored: Option<&str>) -> bool {
    let fresh_pairs = normalized_cookie_pairs(fresh);
    if fresh_pairs.is_empty() {
        return false;
    }
    match stored.map(normalized_cookie_pairs) {
        Some(stored_pairs) if !stored_pairs.is_empty() => fresh_pairs == stored_pairs,
        _ => false,
    }
}

/// webview cookie jar 里属于 xiaomi 系（登录相关）的域名。
///
/// 登录窗会经过 `platform.xiaomimimo.com` + `account.xiaomi.com`（SSO）
/// + `xiaomi.com`（品牌 / passport），死循环里被原样抓回来的是
/// `api-platform_*` 那几个。清 jar 时按域白名单删，别的 provider 的会话不动。
const XIAOMI_COOKIE_DOMAINS: &[&str] = &["xiaomi.com", "xiaomimimo.com"];

/// 清除已保存的 Xiaomi dashboard cookie（设置面板 / 浮窗「清除 Cookie」按钮）。
///
/// 2026-09-28 fix：旧路径走通用 `delete_source_credential("xiaomimimo")`，
/// 只删 keys.json 槽位，**从不碰 webview cookie jar** → 用户被
/// 「清除 → 重登 → 抓到同一份失效 cookie → 401」死循环锁死（全工程无任何
/// UI 能清 jar，`clear_all_browsing_data` 只在注释里出现过）。本命令在删槽
/// 位之外补上 jar 清理，这才是真正的逃生口。
///
/// `instance_id`：副本行（`xiaomimimo#2`）必须带上，否则副本凭据永远删不掉。
#[tauri::command]
pub async fn clear_xiaomi_session(
    app: AppHandle,
    instance_id: Option<String>,
) -> Result<(), String> {
    let target = resolve_target(&app, "xiaomimimo", instance_id.as_deref()).await;
    // 只删 cookie 槽，不动 api_key（xiaomi 的 API key 是独立凭据，
    // 「清除 Cookie」按钮的语义就是只清 cookie —— 同 kimi 的取舍）。
    config::delete_cookie_slot_for_id(&target)?;
    tracing::info!(target = %target, "xiaomi 已清除凭据槽，开始清理 webview cookie jar");
    if let Err(e) = purge_cookies_for_domains(&app, XIAOMI_COOKIE_DOMAINS).await {
        // 清 jar 失败不吞掉主操作的成功（槽位已删），但必须 warn：用户下一次
        // 登录仍可能抓回旧 cookie。
        tracing::warn!(error = %e, target = %target, "xiaomi 清 webview cookie jar 失败（下次登录可能仍抓到旧 cookie）");
    }
    // best-effort refresh：失败只警告（浮窗下一轮 poll 也会自然更新）
    if let Err(e) = crate::commands::refresh_single_inner(
        &app,
        &target,
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await
    {
        tracing::warn!(error = %e, target = %target, "清除 xiaomi 会话后立即拉取失败（忽略）");
    }
    Ok(())
}

/// 清掉 webview cookie jar 里属于 `domains` 的全部 cookie，返回删除条数。
///
/// ## 为什么不用 `WebviewWindow::clear_all_browsing_data()`
///
/// 2026-09-28 实读 wry 0.55.1 源码确认：
/// - macOS：`WKWebsiteDataStore` 取自 webview 的 `configuration()`，而 Tauri
///   的登录窗跟主窗口 / 设置面板**共用同一个默认（非 incognito）data store**
///   → `removeDataOfTypes(allWebsiteDataTypes, since 1970)` 是**进程级**全清：
///   连 kimi / stepfun / anysearch 的登录会话和 app 自己的 webview 存储一起抹。
/// - Windows：`ICoreWebView2Profile2::ClearBrowsingDataAll` 作用于整个
///   user-data-folder 的 profile，同款连带伤害。
/// - Linux(webkitgtk)：同理是全局 `WebKitWebsiteDataManager`。
///
/// 用户点「清除小米 Cookie」却把 Kimi / StepFun 的登录一起清掉，是不可接受的
/// 连带伤害，所以这里逐个 `delete_cookie`（`WKHTTPCookieStore.deleteCookie` /
/// `ICoreWebView2CookieManager::DeleteCookies` —— 都是**底层 cookie store**
/// API，能删 HttpOnly cookie）。
///
/// ## 实现注记
///
/// `cookies()` / `delete_cookie()` 是 `WebviewWindow` 的方法，要拿句柄就得有
/// 个 webview。登录窗此刻通常已经关掉了，所以临时建一个**不可见**的
/// `about:blank` 窗（Tauri 文档里 `WebviewUrl::External("about:blank")` 就是
/// 标准用法），拿到句柄删完立刻 destroy。它跟登录窗共享同一个 data store，
/// 删的就是登录窗的 jar。
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

    // webview 刚建好时 data store 可能还没初始化（stepfun H1 记录过同类
    // 暂态 Err），先让出一点时间再读。
    sleep(Duration::from_millis(400)).await;

    let outcome = purge_with_window(&window, domains);

    // 无论成败都要回收窗口，否则每次「清除」都漏一个 webview。
    let _ = window.destroy();
    outcome
}

/// 临时窗口 label 计数器（同 label 并发 build 会失败，且"两个清除动作同时点"
/// 不是要支持的场景 —— 用计数器保证互不干扰）。
static PURGE_WINDOW_SEQ: AtomicU64 = AtomicU64::new(0);

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
/// cookie domain 常见形态是前导点（`.xiaomi.com`）—— 先剥掉再比，
/// 否则 `.xiaomi.com` 永远匹配不上 `xiaomi.com`。
fn cookie_domain_matches(domain: &str, suffix: &str) -> bool {
    let d = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    let s = suffix.trim().trim_start_matches('.').to_ascii_lowercase();
    !s.is_empty() && (d == s || d.ends_with(&format!(".{s}")))
}

/// 从 URL 查询参数中提取 `userId`。
/// dashboard URL 格式：`...?userId=12345&...` 或 `...?...&userId=12345`
///
/// 安全校验：只接受纯数字（≤32 位）。任何非数字字符（攻击者注入
/// `<script>` / 控制字符 / 过长串）都返回 None，不写入 cookie。
// D3-005 fix (2026-07-30 audit): 加 host 白名单 + path 前缀校验,
// 防止 XSS / MITM 在受信任域外构造 ?userId=... URL 注入错误的 userId.
// dashboard host 是 platform.xiaomimimo.com, SSO 回调可能走 *.xiaomimimo.com
// 域, path 应在 /dashboard 或 /oauth/ 等受信 prefix 下.
fn extract_user_id_from_url(url: &Url) -> Option<String> {
    // 域白名单 + path 前缀白名单
    let host_ok = matches!(
        url.host_str(),
        Some("platform.xiaomimimo.com") | Some("xiaomimimo.com")
    );
    // M-8 fix (2026-08-27 audit): macOS 上 cookie 在 SSO 跳转中可能丢, 兜底
    // 从 URL 抓 userId。原本白名单只有 /dashboard /oauth /, 但 dashboard_url =
    // https://platform.xiaomimimo.com/console/plan-manage (LOGIN_URL, line 122)
    // 走 SSO 回调后落地页就是 /console/plan-manage?userId=N, 在 /console 之外
    // 全部走 None → 兜底实际不可达, 用户反复登录仍报 "cookie 不完整"。
    let path_ok = url.path().starts_with("/dashboard")
        || url.path().starts_with("/oauth")
        || url.path().starts_with("/console")
        || url.path() == "/";
    if !host_ok || !path_ok {
        return None;
    }
    for (key, value) in url.query_pairs() {
        if key == "userId" {
            let v = value.into_owned();
            if !v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_ascii_digit()) {
                return Some(v);
            }
            return None;
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

    #[test]
    fn extract_user_id_accepts_dashboard_host() {
        // D3-005 fix: 受信任 host + dashboard path
        let url = url("https://platform.xiaomimimo.com/dashboard?userId=12345");
        assert_eq!(extract_user_id_from_url(&url), Some("12345".to_string()));
    }

    #[test]
    fn extract_user_id_accepts_console_path() {
        // M-8 fix (2026-08-27 audit): LOGIN_URL = /console/plan-manage, SSO 回调
        // 落地就是这条路 (line 122)。macOS 丢 cookie 时必须能从 URL 兜底拿到
        // userId, 之前白名单没 /console 前缀 → 兜底实际不可达。
        let parsed =
            url("https://platform.xiaomimimo.com/console/plan-manage?userId=12345&other=foo");
        assert_eq!(extract_user_id_from_url(&parsed), Some("12345".to_string()));

        // 嵌套 /console 也算
        let parsed = url("https://platform.xiaomimimo.com/console/sub/page?userId=99999");
        assert_eq!(extract_user_id_from_url(&parsed), Some("99999".to_string()));
    }

    #[test]
    fn extract_user_id_rejects_untrusted_host() {
        // D3-005 fix: 攻击者构造的 evil.com URL 即使有合法 userId 也被拒
        let url = url("https://evil.com/dashboard?userId=12345");
        assert_eq!(extract_user_id_from_url(&url), None);
    }

    #[test]
    fn extract_user_id_rejects_untrusted_path() {
        // D3-005 fix: 受信任 host 但 path 不在白名单 → 拒
        let url = url("https://platform.xiaomimimo.com/some/random/path?userId=12345");
        assert_eq!(extract_user_id_from_url(&url), None);
    }

    // ── 新鲜度门（2026-09-28：清除 Cookie → 重新登录 死循环）──

    #[test]
    fn same_cookie_as_stored_is_rejected() {
        // 死循环的核心场景：webview jar 里还是上一份已失效 cookie，
        // 抓出来跟 keys.json 逐 pair 相同 → 必须拒收，否则又「登录成功」一次。
        let stored = "api-platform_serviceToken=deadbeef; userId=12345";
        assert!(is_same_as_stored(stored, Some(stored)));
    }

    #[test]
    fn same_cookie_different_pair_order_is_rejected() {
        // cookies_for_url 的枚举顺序跨调用不稳定（WKWebView / WebView2 都不保证）
        // —— 顺序不同但内容相同，必须照样拒收，否则死循环绕过去。
        let stored = "api-platform_serviceToken=deadbeef; userId=12345";
        let fresh = "userId=12345; api-platform_serviceToken=deadbeef";
        assert!(is_same_as_stored(fresh, Some(stored)));
    }

    #[test]
    fn same_cookie_ignores_pair_order_and_whitespace() {
        // 归一化 = trim + 去重 + 排序：cookie jar 的枚举顺序跨调用不稳定
        // （WKWebView / WebView2 都不保证），手写 cookie 时分隔符两侧空格也不定。
        let stored = "api-platform_serviceToken=deadbeef; userId=12345";
        let fresh = " api-platform_serviceToken=deadbeef;userId=12345 ";
        assert!(is_same_as_stored(fresh, Some(stored)));
    }

    #[test]
    fn same_cookie_tolerates_whole_pair_quote_wrapping() {
        // 防御性：整对被 WKWebView 包一层引号时归一化后仍判同。
        // （value 级引号在 extract 阶段就 `trim_matches('"')` 剥掉了，
        //   所以存盘串里本来就不该有引号 —— 这里只锁归一化的健壮性。）
        let stored = "api-platform_serviceToken=deadbeef";
        assert!(is_same_as_stored(
            "\"api-platform_serviceToken=deadbeef\"",
            Some(stored)
        ));
    }

    #[test]
    fn new_token_passes_freshness_gate() {
        let stored = "api-platform_serviceToken=deadbeef; userId=12345";
        let fresh = "api-platform_serviceToken=freshcafe; userId=12345";
        assert!(!is_same_as_stored(fresh, Some(stored)));
    }

    #[test]
    fn no_stored_cookie_always_passes() {
        // 从没配过凭据（首次登录 / 刚点过「清除 Cookie」）→ 无从比较，放行
        let fresh = "api-platform_serviceToken=freshcafe; userId=12345";
        assert!(!is_same_as_stored(fresh, None));
        assert!(!is_same_as_stored(fresh, Some("")));
        assert!(!is_same_as_stored(fresh, Some("   ")));
    }

    #[test]
    fn empty_fresh_cookie_is_never_same_as_stored() {
        // 空串（一次都没提到 cookie）不该被当成「跟已存相同」—— 那会让
        // 重试循环白白烧掉 11s。
        assert!(!is_same_as_stored("", Some("api-platform_serviceToken=x")));
    }

    // ── cookie domain 匹配 ──

    #[test]
    fn cookie_domain_matches_leading_dot_form() {
        // cookie store 里 domain 常见 ".xiaomi.com" 形态
        assert!(cookie_domain_matches(".xiaomi.com", "xiaomi.com"));
        assert!(cookie_domain_matches("account.xiaomi.com", "xiaomi.com"));
        assert!(cookie_domain_matches(
            "platform.xiaomimimo.com",
            "xiaomimimo.com"
        ));
    }

    #[test]
    fn cookie_domain_rejects_lookalike_suffix() {
        // 防 `notxiaomi.com` / `xiaomi.com.evil.tld` 这类后缀钓鱼
        assert!(!cookie_domain_matches("notxiaomi.com", "xiaomi.com"));
        assert!(!cookie_domain_matches("xiaomi.com.evil.tld", "xiaomi.com"));
        assert!(!cookie_domain_matches("xiaomimimo.com", "xiaomi.com"));
    }
}
