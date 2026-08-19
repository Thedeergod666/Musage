//! 检查 GitHub releases 有无新版本
//!
//! v0.2.0+ 不再做应用内自动更新（详见 [[musage-macos-signing-saga]] +
//! RELEASING 第 6 章）。本模块只做**轻量**版本检测：拉
//! `https://api.github.com/repos/{owner}/{repo}/releases/latest` → 跟
//! `CARGO_PKG_VERSION` semver 比较 → 缓存到模块私有 [`UPDATE_CACHE`]。
//!
//! 前端 ([`check_for_update`])：
//! - 启动 5s 后 [`spawn_startup_check`] 写一次缓存
//! - 设置页打开 about section 时 `force=false`：立刻返缓存，若空则 spawn 后台 fetch
//! - 「检查更新」按钮 `force=true`：强制 await fetch，结果写缓存 + 返回
//!
//! 为什么不用 tauri-plugin-updater：见 [[musage-macos-signing-saga]]，
//! macOS 签名 + notarize 链路没稳定前应用内安装会触发「应用已损坏」。
//! 跳浏览器走 GitHub 是当前的最低摩擦路径。
//!
//! pre-release 处理：GitHub `/releases/latest` 端点本身只返 stable release
//! （drafts 排除，prerelease 被过滤而非"退回"），所以本模块**天然不
//! 提示 pre-release** —— 符合用户决策 "pre 不用算"。

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::providers::{json_body_limited, shared_client};

/// 启动后多久触发首次 GitHub API 探测。
/// 5s 足够托盘 + 浮窗就位后再探测，用户首次打开设置时已经有缓存可用。
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(5);

/// `releases/latest` 不存在时（repo 一个 release 都没打）的特殊状态码。
/// 视为"没有新版本"而不是错误 —— 用户可能装的是 pre-release build。
const HTTP_NOT_FOUND: u16 = 404;

/// GitHub 仓库 owner/repo。前端 settings page 里的"去 GitHub 下载"外链
/// 是 hardcode 的 `https://github.com/Thedeergod666/Musage/releases/latest`，
/// 这里写同一个 owner/repo 即可。改仓库要**同时改两处**。
const GITHUB_REPO: &str = "Thedeergod666/Musage";

/// 模块私有 update 缓存。**不在 AppState 里**——AppState 的字段（snapshot /
/// config / backoff）是 poller 主循环每次 tick 都要读的"热路径"，update_info
/// 只在设置页打开 + 手动按钮时碰，量级差几个数量级。参考 `tray.rs` /
/// `poller.rs` / `logstore.rs` 的 `OnceLock` 模式放模块私有 static。
///
/// 写：do_check 完成。读：check_for_update command。
static UPDATE_CACHE: OnceLock<Arc<RwLock<Option<UpdateInfo>>>> = OnceLock::new();

fn cache() -> &'static Arc<RwLock<Option<UpdateInfo>>> {
    UPDATE_CACHE.get_or_init(|| Arc::new(RwLock::new(None)))
}

/// 前端拿到这条结构就显示 banner。所有字段都来自 GitHub releases JSON。
#[derive(Serialize, Debug, Clone)]
pub struct UpdateInfo {
    /// semver 字符串，不带 "v" 前缀（跟 `CARGO_PKG_VERSION` 格式一致）
    pub latest_version: String,
    /// GitHub release page URL，前端 `<a target="_blank">` 用
    pub html_url: String,
}

/// GitHub `/releases/latest` 响应里我们关心的字段。serde 容忍未知字段。
#[derive(Deserialize, Debug)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

/// 检查 GitHub releases 是否有新版本。
///
/// - `force=false`（设置页打开 about section 时）：读缓存同步返回 + spawn
///   后台 fetch 更新缓存。**总是立刻返回**，不等网络。
/// - `force=true`（「检查更新」按钮）：await fetch 写缓存 + 返回结果。
///
/// 两模式合并到同一个 command 而不是拆两个（`check_for_update` +
/// `get_cached_update`）的原因：单一入口让前端不用做"先读缓存再触发
/// fetch"的两步串联，后端的 force=false 已经 cover 了"立刻拿缓存 + 后台
/// 更新"的语义。参考 `refresh_now` 单命令模式（[`crate::commands::refresh_now`]）。
#[tauri::command]
pub async fn check_for_update(force: bool) -> Result<Option<UpdateInfo>, String> {
    if force {
        do_check().await
    } else {
        // 读缓存同步返回
        let cached = cache().read().await.clone();
        // 若缓存为空（启动 5s 内 + 启动探测还没跑完），spawn 后台 fetch
        // —— 不能阻塞 settings 打开的瞬间。Fire-and-forget。
        if cached.is_none() {
            tokio::spawn(async move {
                if let Err(e) = do_check().await {
                    tracing::debug!(error = %e, "check_for_update(force=false) 后台 fetch 失败");
                }
            });
        }
        Ok(cached)
    }
}

/// 启动后 spawn 一次探测，结果写 [`UPDATE_CACHE`]。监听 [`crate::poller::SHUTDOWN`]
/// 在用户 5s 窗口内 quit_app 时立即退出 —— 否则会发出一次浪费的 GitHub
/// request（unauthenticated 60 req/hour/IP 限制下每多一次都是 budget）。
pub fn spawn_startup_check() {
    tauri::async_runtime::spawn(async move {
        tokio::select! {
            _ = tokio::time::sleep(STARTUP_CHECK_DELAY) => {}
            _ = crate::poller::SHUTDOWN.notified() => {
                tracing::debug!("update_check 启动探测被 SHUTDOWN 取消");
                return;
            }
        }
        if let Err(e) = do_check().await {
            // 探测失败是**预期**网络场景（offline / VPN / 限流），用 debug
            // 等级不刷日志。前端 banner 区域会显示「检查失败」给用户。
            tracing::debug!(error = %e, "启动后 update check 失败");
        }
    });
}

/// 干活的实现：`fetch → parse → semver compare → 写缓存 → 返结果`。
///
/// 失败语义：
/// - 404（repo 无 release）→ 写 `None` 进缓存 + 返 `Ok(None)`
/// - 其它非 2xx → 返 `Err(...)`，缓存保持原值（不擦写）
/// - tag_name 非 semver → 返 `Err(parse)`，避免把"非版本"误当成新版本
///
/// **复用** [`crate::providers::shared_client`]：UA / 10s timeout / SSRF-aware
/// redirect policy 全部跟 11+ provider 一致，不重新 hand-roll。复用
/// [`crate::providers::json_body_limited`]：走 D5 防御的 8 MiB body cap，
/// 不让恶意 / 劫持响应 OOM 进程（本次改动前是 codebase 唯一绕过 body
/// 限制的 HTTP 路径）。
async fn do_check() -> Result<Option<UpdateInfo>, String> {
    let resp = shared_client()
        .get(format!(
            "https://api.github.com/repos/{GITHUB_REPO}/releases/latest"
        ))
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?;

    let status = resp.status();
    if status.as_u16() == HTTP_NOT_FOUND {
        // repo 一个 release 都没打 → 视为"没有新版本"，写 None 进缓存
        let mut g = cache().write().await;
        *g = None;
        return Ok(None);
    }
    if !status.is_success() {
        // 403 (rate limit) / 5xx 等 —— 返 Err 让前端显示「检查失败」，
        // 缓存保留**旧值**避免覆盖上一次成功的"有新版本"状态。
        // 如果用户上次看到 v0.2.9 可用，这次网络挂了不能误显示成"已是最新"。
        let reason = status.canonical_reason().unwrap_or("");
        return Err(format!("github api {status} {reason}").trim().to_string());
    }

    // D5 防御：body 超过 8 MiB 由 json_body_limited 内部拒绝。
    let value = json_body_limited(resp)
        .await
        .map_err(|e| format!("read body: {e}"))?;
    let release: GithubRelease =
        serde_json::from_value(value).map_err(|e| format!("parse github release: {e}"))?;

    // tag_name 形如 "v0.2.8" → "0.2.8"。有些仓库用 "0.2.8" 没 "v" 前缀，
    // trim_start_matches 不带 'v' 的字符串是 no-op，安全。
    let latest_str = release.tag_name.trim_start_matches('v');
    let current_str = env!("CARGO_PKG_VERSION");

    // 解析失败（例如 tag 写了 "release-2024"）→ 返 Err 让前端显示「检查失败」，
    // 比静默 None 让用户以为"没有新版本"安全 —— 这种情况是有 bug 要修的信号。
    let latest = semver::Version::parse(latest_str)
        .map_err(|e| format!("parse latest '{}': {e}", release.tag_name))?;
    let current = semver::Version::parse(current_str)
        .map_err(|e| format!("parse current '{current_str}': {e}"))?;

    let info = if latest > current {
        Some(UpdateInfo {
            latest_version: latest.to_string(),
            html_url: release.html_url,
        })
    } else {
        None
    };

    let mut g = cache().write().await;
    *g = info.clone();
    Ok(info)
}
