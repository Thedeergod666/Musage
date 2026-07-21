//! StepFun（阶跃星辰）Step Plan 用量查询
//!
//! 端点（[CodexBar docs/stepfun.md](https://github.com/steipete/CodexBar/blob/main/docs/stepfun.md) 参考）：
//! - `POST https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit`
//! - `POST https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus`
//!
//! ## 鉴权
//!
//! Dashboard 端点要求一组浏览器侧 headers（CodexBar reverse-engineered）：
//!
//! - `oasis-appid: 10300`
//! - `oasis-platform: web`
//! - `oasis-webid: <device_id>` — 必须等于 token refresh half 的
//!   JWT `device_id` claim（详见 [`device_id_for_token`]）。
//! - `Cookie: Oasis-Token=<token>; Oasis-Webid=<webid>`
//! - 浏览器 UA（Chrome 147 / macOS）。
//!
//! 缺 `oasis-webid` 会被服务端拒绝（401/403），但响应不区分「token
//! 无效」与「Webid 缺失」——一律表现为 401。本文件提取 device_id 后
//! 一并发送以匹配浏览器侧请求。
//!
//! ## 鉴权模式
//!
//! - **Manual（当前实现）**：用户在设置面板 API Key 框粘贴 Oasis-Token。
//!   token 在登录 `platform.stepfun.com` 后从浏览器 DevTools → Application →
//!   Cookies → `Oasis-Token` 的 value 复制；也支持整段
//!   `Cookie: Oasis-Token=...; ...` 粘贴，程序自动剥离。
//! - **Auto login（TODO 未实现）**：参考 CodexBar 3 步 OAuth 流：
//!   1. `GET https://platform.stepfun.com` → INGRESSCOOKIE
//!   2. `POST …/RegisterDevice` → anonymous token
//!   3. `POST …/SignInByPassword` → authenticated Oasis-Token
//!
//! ## 响应 schema（实测逆向，2026-06-16 参考 CodexBar）
//!
//! QueryStepPlanRateLimit 返回：
//! ```json
//! {
//!   "code": 0,
//!   "data": {
//!     "five_hour_usage_left_rate": 0.99781543,         // 5h 剩余比例 (0-1)
//!     "weekly_usage_left_rate": 0.85,                    // 周剩余比例
//!     "five_hour_usage_reset_time": "2026-06-16T18:30:00Z",  // ISO 8601 或 epoch ms
//!     "weekly_usage_reset_time": "2026-06-19T03:00:00Z",
//!     "plan_family": 2,                                // 2 = credit 套餐 (Mini/Pro)
//!     "plan_credit_rate_limit": {
//!       "subscription_credit_left_rate": 0.96,
//!       "topup_credit_left_rate": 0.5,
//!       "credit_buckets": [
//!         { "credit_total": 100, "credit_residual": 80, "expire_at": "...", "next_reset_at": "..." }
//!       ]
//!     }
//!   }
//! }
//! ```
//!
//! GetStepPlanStatus 返回：
//! ```json
//! {
//!   "code": 0,
//!   "data": {
//!     "subscription": { "name": "Plus" }
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - Rate-window 套餐（`plan_family` 缺失或 ≠ 2）：
//!   - 第一行 `5h`：`(1.0 - five_hour_usage_left_rate) * 100`
//!   - 第二行 `周`：`(1.0 - weekly_usage_left_rate) * 100`
//! - Credit 套餐（`plan_family == 2`）：
//!   - 单行 `额度`：优先 `subscription_credit_left_rate`（缺则用
//!     `topup_credit_left_rate`，再缺则用 `credit_buckets` 加权平均）。
//! - plan_name 来自 GetStepPlanStatus（如 "Plus" / "Mini"）。
//!
//! ## 已知坑
//!
//! 1. **Token 失效**：Oasis-Token 一般 7-30 天过期。本地预检 JWT
//!    `exp` claim，若已过期直接返友好错误（不发请求）；否则服务端
//!    返回 401 时引导用户去 `platform.stepfun.com` 重新登录。
//! 2. **3 步登录流暂未实现**：需要单独 UI 收集 username + password，
//!    加密本地存，目前只支持 manual token paste。Phase X 补 auto-login。
//! 3. **请求是 POST 而非 GET**：Step Plan rate limit 用 POST + JSON body（空
//!    body 也可），不是常规 GET。

use std::borrow::Cow;
use std::pin::Pin;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};
use crate::t;

const URL_RATE_LIMIT: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit";
const URL_PLAN_STATUS: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus";

/// CodexBar 的 login/register 流使用的默认 Webid。
///
/// 仅在 token 中无法解析出 `device_id` 时作为兜底。注意 dashboard 端点
/// 大概率会拒绝这个值（不是当前 token 对应的 device），届时会落到
/// 401/403 错误路径；用户需要重新登录获取带 `device_id` claim 的 token。
const DEFAULT_WEBID: &str = "c8a1002d2c457e758785a9979832217c7c0b884c";

/// CodexBar 的固定 app id。
const OASIS_APPID: &str = "10300";

/// 浏览器 UA — CodexBar 用的 Chrome 147 / macOS，避免被风控识别为
/// 非浏览器客户端。
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct StepfunSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for StepfunSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl StepfunSource {
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

impl QuotaSource for StepfunSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("stepfun")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "stepfun".to_string()
        } else {
            format!("stepfun#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.stepfun").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.stepfun").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKey
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // StepFun 无 region / mode 概念（虽然 URL 有 .com/.ai，但 Oasis-Token 跨域通用）
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            let raw = credentials
                .api_key
                .as_deref()
                .or(credentials.cookie.as_deref())
                .unwrap_or("");

            // 1. 规范化（处理 "Cookie: Oasis-Token=..." / "Oasis-Token=..." 整段粘贴）
            let token = match normalize_oasis_token(raw) {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return Err(FetchError::unconfigured(
                        t!("error.stepfun.token_unconfigured_hint").into_owned(),
                    ));
                }
            };

            // 2. 本地预检 JWT exp：access 已过期就不发请求，给清晰错误
            if let Some(secs_ago) = access_token_exp_seconds_ago(&token) {
                if secs_ago >= 0 {
                    return Err(FetchError::auth(
                        t!(
                            "error.stepfun.token_expired_hint",
                            mins = secs_ago / 60
                        )
                        .into_owned(),
                    ));
                }
            } else {
                // 完全无法识别为 JWT：给"格式无效"提示，避免落入 401 误导
                tracing::warn!(
                    "StepFun Oasis-Token not decodable as JWT; dashboard request will likely 401"
                );
            }

            do_fetch(&token, &self.unique_id(), &self.display_name().to_string()).await
        })
    }
}

async fn do_fetch(
    oasis_token: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // 并行拉 rate limit + plan status（互不依赖）
    let rate = fetch_rate_limit(oasis_token).await?;
    let plan = fetch_plan_status(oasis_token).await.ok().flatten(); // 失败不阻塞

    parse(rate, plan, source_id, display_name)
}

/// 组装 Step Plan dashboard 请求（带 cookie 鉴权 + 浏览器侧 headers）。
///
/// 抽出来让 `fetch_rate_limit` / `fetch_plan_status` 共用同一套鉴权，
/// 也方便单元测试断言 header 是否齐全。
fn build_request(client: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    let webid = device_id_for_token(token).unwrap_or_else(|| {
        tracing::warn!("StepFun token missing device_id claim; falling back to DEFAULT_WEBID");
        DEFAULT_WEBID.to_string()
    });
    let cookie_value = format!("Oasis-Token={token}; Oasis-Webid={webid}");

    client
        .post(url)
        .header("Cookie", cookie_value)
        // CodexBar 同时使用首字母大写和小写两种 header 名,保险都发。
        .header("Oasis-Webid", webid.clone())
        .header("oasis-webid", webid)
        .header("oasis-appid", OASIS_APPID)
        .header("oasis-platform", "web")
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body("{}")
}

/// POST Step Plan rate limit endpoint。
async fn fetch_rate_limit(token: &str) -> Result<Value, FetchError> {
    let client = shared_client();

    let resp = build_request(client, URL_RATE_LIMIT, token)
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!(
                    "error.common.network",
                    url = URL_RATE_LIMIT,
                    err = e.to_string()
                )
                .into_owned(),
            )
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.stepfun.token_invalid_hint").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "StepFun").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "StepFun",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw: Value = resp.json().await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    // 业务级 code 检查
    let code = raw.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(FetchError::server(
            t!(
                "error.common.business_code",
                provider = "StepFun",
                code = code,
                msg = msg
            )
            .into_owned(),
        ));
    }

    Ok(raw)
}

/// POST Step Plan status endpoint。
/// L8 fix: 之前 HTTP 非 200 时返 Ok(None) 静默吞掉错误，
/// do_fetch 里 .ok().flatten() 也吞。plan_name 显示为 None 时
/// 用户/开发者查不到原因，日志也没有任何记录。
/// 改为非 200 时 log warn 后返 Ok(None)（plan_name 是可选字段，不阻塞主 fetch）。
async fn fetch_plan_status(token: &str) -> Result<Option<String>, FetchError> {
    let client = shared_client();

    let resp = build_request(client, URL_PLAN_STATUS, token)
        .send()
        .await
        .map_err(|e| FetchError::network(format!("StepFun plan status 网络错误: {e}")))?;

    if !resp.status().is_success() {
        // L8 fix: log warn 而不是静默返 Ok(None)
        tracing::warn!(
            status = %resp.status(),
            "StepFun plan status endpoint 非 200，plan_name 将为 None"
        );
        return Ok(None);
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("plan status 响应不是 JSON: {e}")))?;

    let name = raw
        .pointer("/data/subscription/name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(name)
}

/// 解析 rate limit 响应 → QuotaRow 列表。
///
/// `usedPercent = (1.0 - left_rate) * 100`
///
/// 两种 plan 形态：
/// - Rate-window（默认，`plan_family` 缺失或 ≠ 2）：5h + 周双行。
/// - Credit 套餐（`plan_family == 2`，Mini/Pro）：单行，按
///   `subscription_credit_left_rate` → `topup_credit_left_rate` →
///   `credit_buckets` 加权平均 的优先级挑出 left 比例。
fn parse(
    rate_raw: Value,
    plan_name: Option<String>,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let data = rate_raw.get("data").ok_or_else(|| {
        FetchError::parse(t!("error.common.missing_data_field", provider = "StepFun").into_owned())
    })?;

    let mut rows = Vec::new();

    let plan_family = data.get("plan_family").and_then(|v| v.as_i64()).unwrap_or(0);
    if plan_family == 2 {
        // ── Credit 套餐（Mini/Pro）：单行 + bucket 加权平均 ──
        if let Some(left) = credit_plan_left_rate(data) {
            if (0.0..=1.0).contains(&left) {
                let used_pct = (1.0 - left) * 100.0;
                rows.push(QuotaRow {
                    label: t!("row.credit").to_string(),
                    utilization: Some(used_pct),
                    remaining: None,
                    used: None,
                    total: None,
                    resets_at: None, // credit 套餐无统一 reset 时间
                    unit: Some("%".to_string()),
                    extra: None,
                    kind: None,
                });
            }
        }
    } else {
        // ── Rate-window 套餐：5h + 周双行 ──

        // 5h tier
        if let Some(left) = data
            .get("five_hour_usage_left_rate")
            .and_then(|v| v.as_f64())
        {
            if (0.0..=1.0).contains(&left) {
                let used_pct = (1.0 - left) * 100.0;
                let reset = data
                    .get("five_hour_usage_reset_time")
                    .and_then(extract_reset_ms);
                rows.push(QuotaRow {
                    label: t!("row.five_hour").to_string(),
                    utilization: Some(used_pct),
                    remaining: None,
                    used: None,
                    total: None,
                    resets_at: reset,
                    unit: Some("%".to_string()),
                    extra: None,
                    kind: None,
                });
            }
        }

        // 周 tier
        if let Some(left) = data.get("weekly_usage_left_rate").and_then(|v| v.as_f64()) {
            if (0.0..=1.0).contains(&left) {
                let used_pct = (1.0 - left) * 100.0;
                let reset = data
                    .get("weekly_usage_reset_time")
                    .and_then(extract_reset_ms);
                rows.push(QuotaRow {
                    label: t!("row.weekly").to_string(),
                    utilization: Some(used_pct),
                    remaining: None,
                    used: None,
                    total: None,
                    resets_at: reset,
                    unit: Some("%".to_string()),
                    extra: None,
                    kind: None,
                });
            }
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    Ok(ProviderSnapshot {
        // v0.3: 用 source_id ("stepfun") 替代旧 "minimax" 占位
        provider: "stepfun".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(rate_raw),
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
    })
}

/// 解析 credit 套餐的 `left_rate`：subscription > topup > bucket 加权平均。
fn credit_plan_left_rate(data: &Value) -> Option<f64> {
    let credit = data.get("plan_credit_rate_limit")?;

    if let Some(v) = credit
        .get("subscription_credit_left_rate")
        .and_then(|x| x.as_f64())
    {
        return Some(v);
    }
    if let Some(v) = credit
        .get("topup_credit_left_rate")
        .and_then(|x| x.as_f64())
    {
        return Some(v);
    }
    // 兜底：credit_buckets 加权平均 (residual / total)
    if let Some(arr) = credit.get("credit_buckets").and_then(|x| x.as_array()) {
        if !arr.is_empty() {
            let mut sum_r = 0.0_f64;
            let mut sum_t = 0.0_f64;
            for b in arr {
                let r = b.get("credit_residual").and_then(|x| x.as_f64());
                let t = b.get("credit_total").and_then(|x| x.as_f64());
                if let (Some(r), Some(t)) = (r, t) {
                    sum_r += r;
                    sum_t += t;
                }
            }
            if sum_t > 0.0 {
                return Some(sum_r / sum_t);
            }
        }
    }
    None
}

// ── Auth helpers (CodexBar reverse-engineered) ────────────────────
//
// StepFun dashboard 端点要求一组浏览器侧 headers,缺 Oasis-Webid 时
// 服务端会无差别返 401/403。本节把"用户粘的 token 字符串"加工成
// 可正常鉴权的请求,并提供本地 exp 预检 + 友好错误。

/// 把 "sessionKey=xxx" / 纯 "xxx" / 整段 cookie 字符串 / 多行粘贴
/// 统一规整成纯 token value。失败时返 `None`,让调用方走"未配置"路径。
///
/// 防御性：
/// - 用户可能只复制 value（最常见）
/// - 用户可能整段复制 `Oasis-Token=xxx; yyy=zzz`
/// - 用户可能整段复制 `Cookie: Oasis-Token=xxx; yyy=zzz`（DevTools
///   右键 → Copy headers 会带前缀）
/// - 多行粘贴取第一行非空（与 `saveCredentialAction` 行为一致）
fn normalize_oasis_token(raw: &str) -> Option<String> {
    // 多行粘贴：取第一行非空
    let first_line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let s = first_line.trim();
    if s.is_empty() {
        return None;
    }

    // 整段 cookie 形式（带 `;`） → 拆出 Oasis-Token 的 value
    if s.contains(';') {
        for part in s.split(';') {
            // 容忍 "Cookie: Oasis-Token=xxx" 整段带前缀
            let p = part.trim().trim_start_matches("Cookie:").trim();
            if let Some((k, v)) = p.split_once('=') {
                if k.trim().eq_ignore_ascii_case("Oasis-Token") {
                    let v = v.trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }

    // "Oasis-Token=xxx" 无 sibling 段
    for prefix in ["Oasis-Token=", "oasis-token="] {
        if let Some(v) = s.strip_prefix(prefix) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }

    Some(s.to_string())
}

/// 从 token 中提取 `device_id` claim（用作 Oasis-Webid）。
///
/// Token 两种形态（CodexBar `combinedToken`）：
/// - 单 JWT: `header.payload.sig`
/// - 双 JWT 组合: `access_jwt...refresh_jwt`
///
/// 优先 refresh half 的 `device_id`（CodexBar `webID(forToken:)`
/// 倒序遍历）。任一半解析失败就跳过。
fn device_id_for_token(token: &str) -> Option<String> {
    let halves: Vec<&str> = if token.contains("...") {
        // combined access...refresh: 反序,先试 refresh
        let mut h: Vec<&str> = token.split("...").collect();
        h.reverse();
        h
    } else if token.contains('.') {
        // 单 JWT
        vec![token]
    } else {
        return None;
    };

    for half in halves {
        if let Some(id) = jwt_device_id(half) {
            return Some(id);
        }
    }
    None
}

/// 从单个 JWT 字符串中提取 `device_id` claim。不做签名校验（参考
/// CodexBar —— 这是 web 客户端正常做法）。
fn jwt_device_id(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    // 容忍带 padding 的 base64url
    let payload_b64 = payload_b64.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("device_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// 本地预检 access token 的 `exp` claim。
///
/// 返回 `Some(secs)`：
/// - `secs >= 0` → 已过期 X 秒（让 `fetch` 走"已过期"错误路径）
/// - `secs < 0` → 距过期还有 -X 秒（即还有 X 秒有效，本函数不阻止请求，
///   仅在返回时调用方可以据此决定是否加 log）
///
/// 解析不出 exp 时返 `None`（交给服务端校验）。
fn access_token_exp_seconds_ago(token: &str) -> Option<i64> {
    // 只看 access half（combined token 的第一段），因为 exp 是 access 的
    let access = token.split("...").next().unwrap_or(token);
    let payload_b64 = access.split('.').nth(1)?;
    let payload_b64 = payload_b64.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v.get("exp").and_then(|x| x.as_i64())?;
    Some(Utc::now().timestamp() - exp)
}

/// 提取 resets_at 为毫秒。接受 ISO 8601 字符串（首选）或 epoch 数字（兜底）。
fn extract_reset_ms(v: &Value) -> Option<i64> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis());
    }
    if let Some(n) = v.as_i64() {
        let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
        return DateTime::<Utc>::from_timestamp_millis(ms).map(|_| ms);
    }
    None
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_full_response() {
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 0.72,
                "weekly_usage_left_rate": 0.55,
                "five_hour_usage_reset_time": "2026-06-16T18:30:00Z",
                "weekly_usage_reset_time": "2026-06-19T03:00:00Z"
            }
        });
        let snap =
            parse(raw.clone(), Some("Plus".to_string()), "stepfun", "StepFun").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("stepfun"));
        assert_eq!(snap.plan_name.as_deref(), Some("Plus"));
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        // 1.0 - 0.72 = 0.28 → 28%
        assert!((five_h.utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(five_h.unit.as_deref(), Some("%"));
        assert!(five_h.resets_at.is_some());

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, t!("row.weekly"));
        // 1.0 - 0.55 = 0.45 → 45%
        assert!((weekly.utilization.unwrap() - 45.0).abs() < 0.001);
    }

    #[test]
    fn parse_only_five_hour() {
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 0.9,
                "five_hour_usage_reset_time": "2026-06-16T18:30:00Z"
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.five_hour").as_ref());
        assert!((snap.rows[0].utilization.unwrap() - 10.0).abs() < 0.001);
        assert_eq!(snap.plan_name, None);
    }

    #[test]
    fn parse_zero_left_rate_is_full() {
        // 0.0 = 100% used
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 0.0,
                "weekly_usage_left_rate": 0.0
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        for row in &snap.rows {
            assert!((row.utilization.unwrap() - 100.0).abs() < 0.001);
        }
    }

    #[test]
    fn parse_left_rate_one_is_zero_used() {
        // 1.0 = 0% used (clean state)
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 1.0
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert!((snap.rows[0].utilization.unwrap() - 0.0).abs() < 0.001);
    }

    #[test]
    fn parse_out_of_range_left_rate_is_skipped() {
        // -0.5 / 1.5 视为异常 → 跳过
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": -0.5,
                "weekly_usage_left_rate": 0.5
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        // 5h 跳过，只剩 weekly
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.weekly"));
    }

    #[test]
    fn parse_no_data_is_error() {
        let raw = json!({ "code": 0 });
        let err = parse(raw, None, "stepfun", "StepFun").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_code_nonzero_is_error() {
        // 业务级 code != 0 应在 fetch_rate_limit 阶段就报错（这里 raw 直接 parse 不会触发）
        // parse 本身只检查 data 字段
        let raw = json!({ "code": 401, "message": "token expired" });
        let err = parse(raw, None, "stepfun", "StepFun").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind); // 缺 data 字段 → parse 错
    }

    #[test]
    fn extract_reset_ms_handles_iso() {
        let v = json!("2026-06-16T18:30:00Z");
        let ms = extract_reset_ms(&v).expect("iso");
        assert!(ms > 1_780_000_000_000 && ms < 1_800_000_000_000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_seconds() {
        let v = json!(1_750_000_000_i64);
        let ms = extract_reset_ms(&v).expect("secs");
        assert_eq!(ms, 1_750_000_000_000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_millis() {
        let v = json!(1_750_000_000_000_i64);
        let ms = extract_reset_ms(&v).expect("ms");
        assert_eq!(ms, 1_750_000_000_000);
    }

    #[test]
    fn extract_reset_ms_invalid_returns_none() {
        assert_eq!(extract_reset_ms(&json!("not a date")), None);
        assert_eq!(extract_reset_ms(&json!(null)), None);
    }

    // ── normalize_oasis_token ──

    #[test]
    fn normalize_oasis_token_plain() {
        assert_eq!(
            normalize_oasis_token("eyJhbGciOiJIUzI1NiJ9.eyJkZXZpY2VfaWQiOiJhYmMifQ.sig"),
            Some("eyJhbGciOiJIUzI1NiJ9.eyJkZXZpY2VfaWQiOiJhYmMifQ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_with_prefix() {
        assert_eq!(
            normalize_oasis_token("Oasis-Token=eyJ.eyJ.sig"),
            Some("eyJ.eyJ.sig".to_string())
        );
        assert_eq!(
            normalize_oasis_token("oasis-token=eyJ.eyJ.sig"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_full_cookie_string() {
        assert_eq!(
            normalize_oasis_token("Oasis-Token=eyJ.eyJ.sig; other=zzz"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_with_leading_cookie_header() {
        assert_eq!(
            normalize_oasis_token("Cookie: Oasis-Token=eyJ.eyJ.sig; foo=bar"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_multiline_first_line() {
        assert_eq!(
            normalize_oasis_token("\n\n  eyJ.eyJ.sig  \nOasis-Webid=yyy"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_empty_returns_none() {
        assert_eq!(normalize_oasis_token(""), None);
        assert_eq!(normalize_oasis_token("   \n  "), None);
    }

    // ── device_id_for_token / jwt_device_id ──

    fn make_jwt_with_claims(claims: &str) -> String {
        // 任意 header / sig;只关心 payload 解析
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload = URL_SAFE_NO_PAD.encode(claims.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(b"sig");
        format!("{header}.{payload}.{sig}")
    }

    #[test]
    fn device_id_for_single_jwt() {
        let jwt = make_jwt_with_claims(r#"{"device_id":"dev-abc"}"#);
        assert_eq!(device_id_for_token(&jwt), Some("dev-abc".to_string()));
    }

    #[test]
    fn device_id_for_combined_access_refresh() {
        // 模拟 access...refresh：refresh 含 device_id
        let access = make_jwt_with_claims(r#"{"sub":"u1","exp":1}"#);
        let refresh = make_jwt_with_claims(r#"{"device_id":"dev-xyz"}"#);
        let combined = format!("{access}...{refresh}");
        assert_eq!(device_id_for_token(&combined), Some("dev-xyz".to_string()));
    }

    #[test]
    fn device_id_prefers_refresh_when_both_have_it() {
        // 两半都含 device_id,CodexBar 倒序偏好 → refresh 胜
        let access = make_jwt_with_claims(r#"{"device_id":"dev-access"}"#);
        let refresh = make_jwt_with_claims(r#"{"device_id":"dev-refresh"}"#);
        let combined = format!("{access}...{refresh}");
        assert_eq!(
            device_id_for_token(&combined),
            Some("dev-refresh".to_string())
        );
    }

    #[test]
    fn device_id_returns_none_for_non_jwt() {
        assert_eq!(device_id_for_token("not-a-jwt"), None);
        assert_eq!(device_id_for_token(""), None);
    }

    #[test]
    fn device_id_returns_none_for_jwt_without_claim() {
        let jwt = make_jwt_with_claims(r#"{"sub":"u1"}"#);
        assert_eq!(device_id_for_token(&jwt), None);
    }

    // ── build_request headers ──

    #[test]
    fn build_request_includes_required_headers() {
        let client = reqwest::Client::new();
        let jwt = make_jwt_with_claims(r#"{"device_id":"dev-headers"}"#);
        let req = build_request(&client, URL_RATE_LIMIT, &jwt).build().unwrap();
        let headers = req.headers();

        // 必发 headers
        for name in [
            "cookie",
            "oasis-appid",
            "oasis-platform",
            "user-agent",
            "accept",
            "content-type",
        ] {
            assert!(
                headers.get(name).is_some(),
                "missing header: {name}\nheaders: {headers:?}"
            );
        }

        // Oasis-Webid 用首字母大写 + 全小写两种名(CodexBar 兼容)
        assert!(headers.get("Oasis-Webid").is_some());
        assert!(headers.get("oasis-webid").is_some());
        assert_eq!(
            headers.get("oasis-appid").unwrap().to_str().unwrap(),
            "10300"
        );

        // Cookie 头同时含 Oasis-Token= 和 Oasis-Webid=
        let cookie = headers.get("cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains("Oasis-Token="), "cookie: {cookie}");
        assert!(cookie.contains("Oasis-Webid=dev-headers"), "cookie: {cookie}");
    }

    #[test]
    fn build_request_falls_back_to_default_webid() {
        let client = reqwest::Client::new();
        let req = build_request(&client, URL_RATE_LIMIT, "not-a-jwt")
            .build()
            .unwrap();
        let cookie = req.headers().get("cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains(&format!("Oasis-Webid={DEFAULT_WEBID}")));
    }

    // ── access_token_exp_seconds_ago ──

    #[test]
    fn access_exp_already_expired() {
        let now = Utc::now().timestamp();
        let claims = format!(r#"{{"exp":{}}}"#, now - 600); // 10 min ago
        let jwt = make_jwt_with_claims(&claims);
        let secs = access_token_exp_seconds_ago(&jwt).expect("exp");
        assert!(secs >= 590 && secs <= 620, "got {secs}");
    }

    #[test]
    fn access_exp_not_yet_expired_returns_negative() {
        let now = Utc::now().timestamp();
        let claims = format!(r#"{{"exp":{}}}"#, now + 3600); // 1h 之后
        let jwt = make_jwt_with_claims(&claims);
        let secs = access_token_exp_seconds_ago(&jwt).expect("exp");
        assert!(secs < 0, "got {secs}");
    }

    #[test]
    fn access_exp_no_claim_returns_none() {
        let jwt = make_jwt_with_claims(r#"{"sub":"u1"}"#);
        assert_eq!(access_token_exp_seconds_ago(&jwt), None);
    }

    #[test]
    fn access_exp_uses_access_half_when_combined() {
        // access.exp 已过期;refresh.exp 还很远 → 应该用 access 的
        let now = Utc::now().timestamp();
        let access = make_jwt_with_claims(&format!(r#"{{"exp":{}}}"#, now - 60));
        let refresh = make_jwt_with_claims(&format!(r#"{{"exp":{}}}"#, now + 86400));
        let combined = format!("{access}...{refresh}");
        let secs = access_token_exp_seconds_ago(&combined).expect("exp");
        assert!(secs >= 30 && secs <= 90, "got {secs}");
    }

    // ── Credit 套餐（plan_family == 2）──

    #[test]
    fn parse_credit_plan_uses_subscription_rate() {
        let raw = json!({
            "code": 0,
            "data": {
                "plan_family": 2,
                "plan_credit_rate_limit": {
                    "subscription_credit_left_rate": 0.8
                }
            }
        });
        let snap = parse(raw, Some("Mini".into()), "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.credit").as_ref());
        // 1 - 0.8 = 0.2 → 20%
        assert!((snap.rows[0].utilization.unwrap() - 20.0).abs() < 0.001);
        assert_eq!(snap.plan_name.as_deref(), Some("Mini"));
    }

    #[test]
    fn parse_credit_plan_falls_back_to_topup() {
        let raw = json!({
            "code": 0,
            "data": {
                "plan_family": 2,
                "plan_credit_rate_limit": {
                    "topup_credit_left_rate": 0.5
                }
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        // 1 - 0.5 = 0.5 → 50%
        assert!((snap.rows[0].utilization.unwrap() - 50.0).abs() < 0.001);
    }

    #[test]
    fn parse_credit_plan_weighted_buckets() {
        // bucket1: 20/100, bucket2: 30/100 → (20+30)/(100+100) = 0.25 → 75% used
        let raw = json!({
            "code": 0,
            "data": {
                "plan_family": 2,
                "plan_credit_rate_limit": {
                    "credit_buckets": [
                        { "credit_total": 100, "credit_residual": 20 },
                        { "credit_total": 100, "credit_residual": 30 }
                    ]
                }
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert!((snap.rows[0].utilization.unwrap() - 75.0).abs() < 0.001);
    }

    #[test]
    fn parse_credit_plan_no_credit_data_is_error() {
        // plan_family==2 但 plan_credit_rate_limit 缺关键字段 → empty rows → 错误
        let raw = json!({
            "code": 0,
            "data": { "plan_family": 2 }
        });
        let err = parse(raw, None, "stepfun", "StepFun").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }
}
