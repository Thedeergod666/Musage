//! AnySearch 用量查询 —— cookie 路径（登录 webview 自动提取 session JWT）
//!
//! ⚠️ AnySearch 的用量查询**只走 console 内部 API**，不是公开 endpoint，也**不接受**
//! `as_sk_` 形式的 MCP API key（实测 `as_sk_` 调 console 端点返 401
//! “管理员会话或用户访问令牌无效”）：
//! - `GET https://www.anysearch.com/api/api/user/billing/overview`
//! - 鉴权：`Authorization: Bearer <user_session_jwt>` —— 用户在 anysearch.com 登录后
//!   浏览器 localStorage `search-template-auth-state.state.accessToken` 里的那个 JWT。
//!
//! **端点选择**：之前误用 `/api/api/user/keys`（那是 API key 元数据接口，quota_used
//! 是「全 key 累计调用数」、无日配额/剩余额度）；**真正展示给用户看的用量在
//! `/api/api/user/billing/overview`**（overview 页直接调它）。
//!
//! ## ⭐ access token 短命 + 主动续期（refresh token 方案）
//!
//! AnySearch 的 access token 是 **OAuth 短命令牌，寿命仅 30 分钟**（实测 JWT
//! `exp - iat = 1800s`）。若只存 access，用户「出门吃个饭回来必掉线」（浮窗 401）。
//! 解法：登录时连 **refreshToken** 一起抓下来（[`crate::anysearch_login`]），combined
//! 成 `<access>...<refresh>` 存进 cookie 槽位；本 provider 在请求前：
//!
//! 1. 按 `...` 哨兵 split 出 access / refresh 两半（无 `...` = 老格式/手动粘贴的裸
//!    access，退化成只有 access、无法续期）。
//! 2. **本地预检** access 的 `exp` claim：已过期或 `SKEW_SECS`(120s) 内将过期，
//!    且有 refresh token → 先调 `POST /api/ssuser/auth/refresh` 换新的 access+refresh。
//! 3. 用（可能刚换的）access 请求 billing/overview。
//! 4. **兜底**：若请求仍返 401（本地预检没抓到、但服务端已作废），有 refresh 时
//!    再 refresh 一次 + 重试一遍请求。
//!
//! ⚠️ **refresh token 单次轮换（single-use rotation）**：每次 refresh 换发一个新的
//! refresh_token 并作废旧的（实测旧 token 复用返 `40114 revoked`）。所以 refresh
//! **成功后必须**把新的 `<access>...<refresh>` 原子写回 keys.json（`save_credential_for_id`），
//! 否则下一轮 refresh 就废了。写回用 `unique_id`（跟 poller/commands load 的 key 一致，
//! 副本 `anysearch#2` 也对得上）。
//!
//! 用户操作（推荐一键登录，详见 [`crate::anysearch_login`]）：
//! 1. 设置面板点 “🔑 登录 AnySearch” → 弹 webview → 登录 anysearch.com
//! 2. 后端从 webview 的 localStorage 抽出 access+refresh JWT → 写 keys.json
//! 3. 后台轮询用 access 拉数据，到期自动用 refresh 续；refresh 也失效时 (HTTP 401)
//!    错误信息引导重新登录
//!
//! 也支持手动兜底：把 access JWT 整段粘到下面的 “Cookie / Token” 文本框（跟 cookie
//! 字段共用存储槽位，[`AuthKind::Cookie`]）。手动粘贴无 refresh 半段 → 不能续期，
//! 30 分钟后仍需重新粘。
//!
//! ## 响应 schema
//!
//! ```json
//! {
//!   "code": 0,
//!   "message": "Success.",
//!   "data": {
//!     "tier_name": "Free Plan",
//!     "tier_code": "basic",
//!     "total": 1000,                    // 日配额（-1 / null = 无限）
//!     "used": 523,                      // 已用
//!     "remaining": 477,                  // 剩余
//!     "rate_limit_qps": 10,              // QPS（每秒）
//!     "rate_limit_unlimited": false,
//!     "reset_period": "daily",           // daily / monthly
//!     "next_reset_at": "2026-07-23T00:00:00Z",  // ISO 8601 UTC
//!     "upgrade_hint": "verify_developer_student"
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 主指标行：`"523 / 1000 calls"` + 进度条（Free Plan 是 limited，1000/天）
//!   无限额时 `"523 calls"`（无进度条）
//! - 重置时间：从 `next_reset_at` 解析 → 填 `resets_at`（主指标行）；
//!   `reset_period`（"daily" / "monthly"）塞进主行 `extra.reset_period`，
//!   浮窗据此显示「日重置」/「月重置」前缀（Free Plan 是 daily → 日重置）
//! - 头部副标题：`plan_name = tier_name`（如 "Free Plan"）
//!
//! 注：速率限制（QPS）副行按产品要求**不展示**，只留主配额行。

use std::borrow::Cow;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};

use super::{
    json_body_limited, shared_client, text_body_limited, AuthKind, Credentials, ErrorKind,
    FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};

use crate::t;

/// BUG-001 fix (2026-07-29 审查): per-unique_id 锁串行化 refresh。
/// 同一 instance 的 refresh 调用必须互斥 —— 并发时两次都拿同一旧
/// refresh_token 去 POST,服务端 revoke 旧的,第二次必败 (40114)。
/// 不同 unique_id 之间不互斥 (instance 独立 refresh 配额)。
static REFRESH_LOCKS: OnceLock<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

/// console 内部用量端点（需要 user session JWT，不接受 `as_sk_` API key）。
/// 必须是 `/api/api/user/billing/overview` —— overview 页直接调它，
/// 返回用户的日/月配额、剩余、QPS、重置时间。`/api/api/user/keys` 只返
/// API key 元数据，不是用户用量。
const URL: &str = "https://www.anysearch.com/api/api/user/billing/overview";

/// access token 刷新端点（逆向自 anysearch.com 前端 bundle）。
/// `POST` body `{"refresh_token": "..."}` → 返
/// `{code:0, data:{access_token, refresh_token, expires_in_seconds}}`。
/// ⚠️ refresh token 单次轮换：每次换发新的、作废旧的。
const REFRESH_URL: &str = "https://www.anysearch.com/api/ssuser/auth/refresh";

/// combined token 的哨兵分隔符：`<access>...<refresh>`。`...` 不是 base64url
/// 合法字符（JWT 只含 `A-Za-z0-9-_` + `.`），拿它当分隔符绝不跟 token 内容冲突。
/// 跟 StepFun combined-token 约定一致。
const TOKEN_SEP: &str = "...";

/// access token 过期缓冲：距 `exp` 不足这个秒数就提前 refresh（避免「刚好卡在
/// 请求发出瞬间过期」的边界 401）。
const SKEW_SECS: i64 = 120;

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct AnysearchSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for AnysearchSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl AnysearchSource {
    /// PR 1b：带 instance_index 的新实例
    pub fn with_instance_index(mut self, idx: u32) -> Self {
        self.instance_index = idx;
        self
    }

    /// PR 1b：in-place 改 instance_index
    #[allow(dead_code)] // 预留 v2 备用（PR 1b 用 with_instance_index 已覆盖当前路径）
    pub fn set_instance_index(&mut self, idx: u32) {
        self.instance_index = idx;
    }
}

impl QuotaSource for AnysearchSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("anysearch")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "anysearch".to_string()
        } else {
            format!("anysearch#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.anysearch").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.anysearch").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    /// JWT 存进 cookie 槽位（跟手动粘贴文本框共用存储）。console 端点不接受
    /// `as_sk_` API key，所以**只**走 cookie 字段，不展示 API key 输入框。
    fn auth_kind(&self) -> AuthKind {
        AuthKind::Cookie
    }

    /// 无 region / overrides / display_mode 概念 → 跳过 update_source_state 的
    /// 整张 AppConfig 序列化（每分钟 11 provider 各一次，纯属浪费）。
    fn needs_state_update(&self) -> bool {
        false
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // AnySearch 无运行时状态，忽略
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            let token = credentials.cookie.as_deref().unwrap_or("").trim();
            if token.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.anysearch.token_empty").into_owned(),
                ));
            }
            do_fetch(token, &self.unique_id(), self.display_name().as_ref()).await
        })
    }
}

/// 从 combined `<access>...<refresh>` 拆出两半。无 `...` = 老格式 / 手动粘贴的
/// 裸 access → refresh 半段为 None（不能续期）。
fn split_token(combined: &str) -> (&str, Option<&str>) {
    match combined.split_once(TOKEN_SEP) {
        Some((access, refresh)) => {
            let refresh = refresh.trim();
            (
                access.trim(),
                if refresh.is_empty() {
                    None
                } else {
                    Some(refresh)
                },
            )
        }
        None => (combined.trim(), None),
    }
}

/// 本地预检 access token 的 `exp` claim（不校验签名，参考 stepfun）。
///
/// 返回距过期的秒数：`> 0` = 还有 N 秒有效；`<= 0` = 已过期 |N| 秒。
/// 解析不出 `exp` 时返 `None`（交给服务端 401 兜底路径）。
fn access_expires_in_secs(access: &str) -> Option<i64> {
    let payload_b64 = access.split('.').nth(1)?.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v.get("exp").and_then(|x| x.as_i64())?;
    Some(exp - chrono::Utc::now().timestamp())
}

/// 调 refresh 端点用 refresh_token 换新的 `<access>...<refresh>`。
///
/// 成功后**立即**把新 combined 原子写回 keys.json（`save_credential_for_id(unique_id)`）——
/// refresh token 单次轮换，不写回下一轮就废。写回失败只 warn 不阻塞（本轮拿到的
/// 新 access 仍可用，只是下次得重登）。返回新的 combined token。
async fn refresh_token(refresh: &str, unique_id: &str) -> Result<String, FetchError> {
    // BUG-001 fix: per-unique_id 锁串行化 refresh。lock_recover 风格
    // 处理 poison,保证 panic 后 caller 不永久死锁。
    let _refresh_guard = {
        let locks = REFRESH_LOCKS.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
        let mut g = locks.lock().await;
        let lock = g
            .entry(unique_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        Arc::clone(&lock).lock_owned().await
    };
    let client = shared_client();
    let resp = client
        .post(REFRESH_URL)
        .header("Accept", "application/json")
        .json(&json!({ "refresh_token": refresh }))
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!(
                    "error.common.network",
                    url = REFRESH_URL,
                    err = e.to_string()
                )
                .into_owned(),
            )
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.anysearch.token_invalid_hint").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            super::ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "AnySearch").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = text_body_limited(resp).await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "AnySearch refresh",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw = json_body_limited(resp).await?;

    // 业务级 code（0 = 成功；40114 = refresh token 已作废 → 需重新登录）
    let code = raw.get("code").and_then(json_i64).unwrap_or(0);
    if code != 0 {
        // refresh 失败几乎都是 refresh token 也过期/被作废 → 引导重新登录
        return Err(FetchError::auth(
            t!("error.anysearch.token_invalid_hint").into_owned(),
        ));
    }

    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::auth(t!("error.anysearch.token_invalid_hint").into_owned()))?;
    let new_access = data
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let new_refresh = data
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if new_access.is_empty() || new_refresh.is_empty() {
        return Err(FetchError::auth(
            t!("error.anysearch.token_invalid_hint").into_owned(),
        ));
    }

    let combined = format!("{new_access}{TOKEN_SEP}{new_refresh}");

    // 原子写回（单次轮换硬约束）。失败只 warn —— 本轮新 access 仍可用。
    let cred = Credentials {
        api_key: None,
        cookie: Some(combined.clone()),
        secret_key: None,
    };
    if let Err(e) = crate::config::save_credential_for_id(unique_id, &cred) {
        tracing::warn!(error = %e, unique_id, "anysearch refresh 后写回 keys.json 失败（本轮仍可用，下次可能需重登）");
    } else {
        tracing::info!(
            unique_id,
            "anysearch access token 已通过 refresh 续期并写回"
        );
    }

    Ok(combined)
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|raw| raw.trim().parse().ok()))
}

async fn do_fetch(
    combined: &str,
    unique_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    if combined.trim().is_empty() {
        return Err(FetchError::unconfigured(
            t!("error.anysearch.token_empty").into_owned(),
        ));
    }

    let (access, refresh) = split_token(combined);
    let mut access = access.to_string();
    // fix (2026-07-28 审查 D1): refresh 改 owned —— 主动续期成功后服务端已
    // 作废旧 refresh(single-use rotation),后续 401 兜底重试必须拿新 combined
    // 里的新 refresh,否则拿已作废的旧 refresh 调 refresh 端点必失败(40114)。
    let mut refresh: Option<String> = refresh.map(str::to_string);

    // ── 主动续期：本地预检 access exp，快过期且有 refresh → 先换新 ──
    if let Some(r) = refresh.clone() {
        let should_refresh = match access_expires_in_secs(&access) {
            Some(remaining) => remaining <= SKEW_SECS, // 已过期或 SKEW 内将过期
            None => false, // 解析不出 exp → 不主动 refresh，交给 401 兜底
        };
        if should_refresh {
            match refresh_token(&r, unique_id).await {
                Ok(new_combined) => {
                    let (new_access, new_refresh) = split_token(&new_combined);
                    access = new_access.to_string();
                    // 新 refresh 同步给下面的 401 兜底路径用
                    refresh = new_refresh.map(str::to_string);
                }
                // 主动 refresh 失败（refresh 也废了）→ 直接返 auth 错误引导重登
                Err(e) => return Err(e),
            }
        }
    }

    // ── 用 access 请求 billing/overview ──
    match do_fetch_once(&access, unique_id, display_name).await {
        // 兜底：请求仍 401（本地预检没抓到 / access 实际已被服务端作废），
        // 且有 refresh → refresh 一次再重试一遍。
        Err(e) if e.kind == super::ErrorKind::AuthFailed => {
            if let Some(r) = &refresh {
                let new_combined = refresh_token(r, unique_id).await?;
                let (new_access, _) = split_token(&new_combined);
                do_fetch_once(new_access, unique_id, display_name).await
            } else {
                // 无 refresh（手动粘贴的裸 access）→ 原样返回引导重登
                Err(e)
            }
        }
        other => other,
    }
}

/// 单次 billing/overview 请求（不含续期逻辑）。`access` 是纯 access JWT。
async fn do_fetch_once(
    access: &str,
    unique_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let token = access.trim();
    if token.is_empty() {
        return Err(FetchError::unconfigured(
            t!("error.anysearch.token_empty").into_owned(),
        ));
    }

    let client = shared_client();

    let resp = client
        .get(URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = URL, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            super::ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "AnySearch").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // JWT 过期 / 无效 —— 上层 do_fetch 见 AuthFailed 会尝试 refresh；
        // refresh 也失败时这个文案引导用户点浮窗「重新登录」。
        return Err(FetchError::auth(
            t!("error.anysearch.token_invalid_hint").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = text_body_limited(resp).await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "AnySearch",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw = json_body_limited(resp).await?;

    // 业务级 code（0 = 成功）
    if let Some(code) = raw.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
            return Err(FetchError::server(
                t!(
                    "error.common.business_code",
                    provider = "AnySearch",
                    code = code,
                    msg = msg
                )
                .into_owned(),
            ));
        }
    }

    parse(&raw, unique_id, display_name)
}

/// 解析 `/api/api/user/billing/overview` 响应。
///
/// data 必含 `used` + `total` + `remaining` + `rate_limit_qps` + `next_reset_at`
/// 以及 `tier_name`。`next_reset_at` 是 ISO 8601 UTC 字符串（`"2026-07-23T00:00:00Z"`），
/// 直接 RFC 3339 解析即可。
fn parse(raw: &Value, source_id: &str, display_name: &str) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "AnySearch",
                field = "data"
            )
            .into_owned(),
        )
    })?;

    let used = num_f64(data, "used").unwrap_or(0.0);
    let total = num_f64(data, "total");
    let remaining = num_f64(data, "remaining");
    let is_active = data
        .get("is_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // total=0 / null / -1 = unlimited（schema 没明说，按 Tavily 那边的兜底约定）
    let is_unlimited = total.map(|t| t <= 0.0).unwrap_or(true);

    // reset_period（"daily" / "monthly"）→ 塞进主行 extra，浮窗据此选
    // 「日重置」/「月重置」前缀。Free Plan 实测 = "daily"。缺失时 extra=None，
    // 浮窗 fallback 到月重置前缀（跟旧行为一致）。
    let row_extra: Option<Value> = data
        .get("reset_period")
        .and_then(|v| v.as_str())
        .map(|p| json!({ "reset_period": p }));

    // resets_at：`next_reset_at` 是 ISO 8601 UTC（"2026-07-23T00:00:00Z"），
    // DateTime::parse_from_rfc3339 直接吃。
    let resets_at: Option<i64> = data
        .get("next_reset_at")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis())
        });

    // plan_name = tier_name（"Free Plan" / "Pro" / 之类）
    let plan_name = data
        .get("tier_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut rows = Vec::new();

    // ── 主行：已用 / 总量 calls（limited 时用进度条）
    if is_unlimited {
        // 无上限：只显示已用，无进度条
        rows.push(QuotaRow {
            label: t!("row.quota").to_string(),
            utilization: None,
            remaining: None,
            used: Some(used),
            total: None,
            resets_at,
            unit: Some(t!("row.calls").to_string()),
            extra: row_extra.clone(),
            kind: None,
        });
    } else if let Some(l) = total {
        if l > 0.0 {
            rows.push(QuotaRow {
                label: t!("row.quota").to_string(),
                // 超用 clamp 0..100
                utilization: Some((used / l * 100.0).clamp(0.0, 100.0)),
                // 用 API 返的 remaining（跟 used/total 可能略有浮点差，API 自己的值更准）
                remaining,
                used: Some(used),
                total: Some(l),
                resets_at,
                unit: Some(t!("row.calls").to_string()),
                extra: row_extra.clone(),
                kind: None,
            });
        } else {
            rows.push(QuotaRow {
                label: t!("row.quota").to_string(),
                utilization: None,
                remaining: None,
                used: Some(used),
                total: None,
                resets_at,
                unit: Some(t!("row.calls").to_string()),
                extra: row_extra.clone(),
                kind: None,
            });
        }
    }

    // 速率限制（QPS）副行按产品要求不展示 —— 只留主配额行。

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "AnySearch",
                field = "used"
            )
            .into_owned(),
        ));
    }

    // 2026-08-05 审查交叉验证修复: success 必须跟 is_active 联动.
    // 之前 success = !rows.is_empty() -- 账号停用时 API 仍可能返回 billing 行,
    // 导致 success:true + error:Some(AuthFailed) 同时出现, 违反
    // ProviderSnapshot 契约 (error_kind 仅 success==false 时有意义), 前端
    // 浮窗会同时渲染用量条 + 错误卡, 行为矛盾.
    let success = !rows.is_empty() && is_active;
    // 2026-08-03 audit (Raman P2 / McClintock): is_active=false 时
    // 必须返回 AuthFailed error,浮窗才能显示"账号已停用"提示,
    // 并触发一键重登路径 (跟 anysearch_login.rs 的 relogin-anysearch 分支配对)
    let (error, error_kind) = if !is_active {
        (
            Some(t!("error.anysearch.account_inactive").into_owned()),
            Some(ErrorKind::AuthFailed),
        )
    } else {
        (None, None)
    };
    Ok(ProviderSnapshot {
        provider: "anysearch".to_string(),
        success,
        rows,
        error,
        error_kind,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: success && is_active,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
    })
}

fn num_f64(obj: &Value, field: &str) -> Option<f64> {
    // D-014 fix (2026-07-30 audit): 过滤 NaN/inf 字符串. 对齐 H12 fix.
    obj.get(field).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|i| i as f64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|f| f.is_finite())
    })
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── combined token split + exp 预检 ──

    #[test]
    fn split_token_combined() {
        let (a, r) = split_token("eyJaccess...myrefresh");
        assert_eq!(a, "eyJaccess");
        assert_eq!(r, Some("myrefresh"));
    }

    #[test]
    fn split_token_bare_access_no_refresh() {
        // 老格式 / 手动粘贴的裸 access → refresh 半段 None（不能续期）
        let (a, r) = split_token("eyJbareAccessOnly");
        assert_eq!(a, "eyJbareAccessOnly");
        assert_eq!(r, None);
    }

    #[test]
    fn split_token_trims_and_empty_refresh_is_none() {
        let (a, r) = split_token("  eyJx  ...   ");
        assert_eq!(a, "eyJx");
        assert_eq!(r, None, "空 refresh 半段 → None");
    }

    #[test]
    fn access_expires_in_secs_reads_exp() {
        // 构造一个 exp = now + 1000s 的极简 JWT（header.payload.sig，只 payload 有用）
        let future = chrono::Utc::now().timestamp() + 1000;
        let payload = json!({ "exp": future });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("eyJhbGciOiJFZERTQSJ9.{payload_b64}.sig");
        let remaining = access_expires_in_secs(&jwt).expect("应解析出 exp");
        assert!(
            (900..=1000).contains(&remaining),
            "剩余应 ~1000s，实际 {remaining}"
        );
    }

    #[test]
    fn access_expires_in_secs_expired_is_negative() {
        let past = chrono::Utc::now().timestamp() - 500;
        let payload = json!({ "exp": past });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let jwt = format!("eyJhbGciOiJFZERTQSJ9.{payload_b64}.sig");
        let remaining = access_expires_in_secs(&jwt).expect("应解析出 exp");
        assert!(remaining < 0, "已过期应为负，实际 {remaining}");
    }

    #[test]
    fn access_expires_in_secs_no_exp_or_garbage_is_none() {
        assert!(access_expires_in_secs("not-a-jwt").is_none());
        // 合法结构但 payload 无 exp
        let payload_b64 = URL_SAFE_NO_PAD.encode(b"{\"sub\":\"x\"}");
        let jwt = format!("h.{payload_b64}.s");
        assert!(access_expires_in_secs(&jwt).is_none());
    }

    /// 真实 console overview 抓的 Free Plan 数据
    const FREE_PLAN_OK: &str = r#"{
        "code": 0,
        "message": "Success.",
        "data": {
            "next_reset_at": "2026-07-23T00:00:00Z",
            "rate_limit_qps": 10,
            "rate_limit_unlimited": false,
            "remaining": 477,
            "request_id": "5a4f89ef-0212-48d1-bf01-576329823280",
            "reset_period": "daily",
            "tier_code": "basic",
            "tier_name": "Free Plan",
            "total": 1000,
            "upgrade_hint": "verify_developer_student",
            "used": 523
        }
    }"#;

    #[test]
    fn parse_free_plan_limited_shows_bar_and_resets_at() {
        let raw = serde_json::from_str(FREE_PLAN_OK).unwrap();
        let snap = parse(&raw, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.plan_name.as_deref(), Some("Free Plan"));
        assert_eq!(snap.source_id.as_deref(), Some("anysearch"));
        assert!(snap.is_healthy);
        // 只有 1 行：quota（QPS 副行按产品要求不再展示）
        assert_eq!(snap.rows.len(), 1);

        let main = &snap.rows[0];
        assert_eq!(main.label, t!("row.quota"));
        assert_eq!(main.used, Some(523.0));
        assert_eq!(main.total, Some(1000.0));
        assert_eq!(main.remaining, Some(477.0));
        // 523/1000 = 52.3%
        assert!((main.utilization.unwrap() - 52.3).abs() < 0.001);
        assert_eq!(main.unit.as_deref(), Some(t!("row.calls").as_ref()));
        // next_reset_at → resets_at 必须填上（2026-07-23T00:00:00Z）
        assert!(main.resets_at.is_some());
        let expected = chrono::DateTime::parse_from_rfc3339("2026-07-23T00:00:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(main.resets_at, Some(expected));
        // reset_period=daily → 塞进主行 extra（浮窗据此显示「日重置」）
        assert_eq!(
            main.extra
                .as_ref()
                .and_then(|e| e.get("reset_period"))
                .and_then(|v| v.as_str()),
            Some("daily")
        );
    }

    #[test]
    fn parse_overuse_clamps_remaining_to_zero() {
        // used > total 边角：utilization clamp 100，remaining clamp 0
        // （API 通常不会返这种，但前端要稳）
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v["data"]["used"] = json!(1200);
        v["data"]["remaining"] = json!(0); // API 自己的兜底
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        let main = &snap.rows[0];
        assert!((main.utilization.unwrap() - 100.0).abs() < 0.001);
        assert_eq!(main.remaining, Some(0.0));
    }

    #[test]
    fn parse_total_zero_or_negative_is_unlimited() {
        // total=null / -1 / 0 兜底无限额（Tavily 同款约定）
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v["data"]["total"] = json!(0);
        v["data"]["remaining"] = json!(null);
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        let main = &snap.rows[0];
        assert_eq!(main.used, Some(523.0));
        assert_eq!(main.total, None, "unlimited → total=None");
        assert!(main.utilization.is_none(), "unlimited → 无进度条");
    }

    #[test]
    fn parse_rate_limit_fields_ignored_single_row() {
        // QPS 副行已按产品要求移除：无论 rate_limit_unlimited / rate_limit_qps
        // 取何值，都只产出 1 行主配额（不再读这两个字段）。
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v["data"]["rate_limit_unlimited"] = json!(false);
        v["data"]["rate_limit_qps"] = json!(10);
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        assert_eq!(snap.rows.len(), 1, "只留主配额行，无 QPS 副行");
        assert_eq!(snap.rows[0].label, t!("row.quota"));
    }

    #[test]
    fn parse_missing_data_field_is_parse_error() {
        let raw = json!({ "code": 0, "result": "ok" });
        let err = parse(&raw, "anysearch", "AnySearch").unwrap_err();
        assert_eq!(err.kind, super::super::ErrorKind::Parse);
    }

    #[test]
    fn parse_business_code_non_zero_is_rejected_in_do_fetch() {
        // parse() 不拦 code（do_fetch 已拦）；缺 data 时 parse 报错而非塞脏 rows
        let raw = json!({ "code": 40141, "message": "token invalid" });
        let err = parse(&raw, "anysearch", "AnySearch").unwrap_err();
        assert_eq!(err.kind, super::super::ErrorKind::Parse);
    }

    #[test]
    fn json_i64_accepts_numeric_string_business_codes() {
        assert_eq!(json_i64(&json!("40114")), Some(40114));
        assert_eq!(json_i64(&json!("not-a-code")), None);
    }

    #[test]
    fn parse_next_reset_at_missing_still_works() {
        // next_reset_at 缺失（罕见的 schema 漂移）→ resets_at=None，主行仍渲染
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v.as_object_mut().unwrap()["data"]
            .as_object_mut()
            .unwrap()
            .remove("next_reset_at");
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success);
        assert!(
            snap.rows[0].resets_at.is_none(),
            "无 next_reset_at → resets_at=None"
        );
    }

    #[test]
    fn parse_bad_next_reset_at_falls_back_to_none() {
        // next_reset_at 不是合法 RFC 3339 → 走 fallback，resets_at=None 但 rows 仍渲
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v["data"]["next_reset_at"] = json!("not-a-date");
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success);
        assert!(snap.rows[0].resets_at.is_none());
    }
}
