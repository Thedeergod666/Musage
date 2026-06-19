//! StepFun（阶跃星辰）Step Plan 用量查询
//!
//! 端点（[CodexBar docs/stepfun.md](https://github.com/steipete/CodexBar/blob/main/docs/stepfun.md) 参考）：
//! - `POST https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit`
//! - `POST https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus`
//! 鉴权：`Cookie: Oasis-Token=<token>`
//!
//! ## 鉴权模式
//!
//! - **Manual（当前实现）**：用户在设置面板 API Key 框粘贴 Oasis-Token。
//!   token 在登录 stepfun.ai 后从浏览器 DevTools → Application → Cookies
//!   → `Oasis-Token` 的 value 复制。
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
//!     "weekly_usage_reset_time": "2026-06-19T03:00:00Z"
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
//! - 第一行 `5h`：`(1.0 - five_hour_usage_left_rate) * 100` → 已用百分比
//! - 第二行 `周`：`(1.0 - weekly_usage_left_rate) * 100` → 已用百分比
//! - plan_name 来自 GetStepPlanStatus（如 "Plus" / "Mini"）
//!
//! ## 已知坑
//!
//! 1. **Token 失效**：Oasis-Token 一般 7-30 天过期。过期后 fetch 返回 401
//!    → 用户去 stepfun.ai 重新登录 → 重新粘贴新 token。
//! 2. **3 步登录流暂未实现**：需要单独 UI 收集 username + password，
//!    加密本地存，目前只支持 manual token paste。Phase X 补 auto-login。
//! 3. **请求是 POST 而非 GET**：Step Plan rate limit 用 POST + JSON body（空
//!    body 也可），不是常规 GET。

use std::borrow::Cow;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow, QuotaSource};
use crate::t;

const URL_RATE_LIMIT: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit";
const URL_PLAN_STATUS: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct StepfunSource;

impl Default for StepfunSource {
    fn default() -> Self { Self }
}

impl QuotaSource for StepfunSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("stepfun") }
    fn display_name(&self) -> Cow<'_, str> { Cow::Owned(t!("provider_name.stepfun").into_owned()) }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let token = credentials
                .api_key
                .as_deref()
                .or(credentials.cookie.as_deref())
                .unwrap_or("")
                .trim();
            if token.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.stepfun.token_unconfigured_hint").into_owned()
                ));
            }
            do_fetch(token).await
        })
    }
}

async fn do_fetch(oasis_token: &str) -> Result<ProviderSnapshot, FetchError> {
    // 并行拉 rate limit + plan status（互不依赖）
    let rate = fetch_rate_limit(oasis_token).await?;
    let plan = fetch_plan_status(oasis_token).await.ok().flatten();  // 失败不阻塞

    parse(rate, plan)
}

/// POST Step Plan rate limit endpoint。
async fn fetch_rate_limit(token: &str) -> Result<Value, FetchError> {
    let client = shared_client();

    let resp = client
        .post(URL_RATE_LIMIT)
        .header("Cookie", format!("Oasis-Token={token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body("{}")  // 空 body，服务端 schema 不需要参数
        .send()
        .await
        .map_err(|e| FetchError::network(
            t!("error.common.network", url = URL_RATE_LIMIT, err = e.to_string()).into_owned()
        ))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.stepfun.token_invalid_hint").into_owned()
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "StepFun").into_owned()
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
            ).into_owned()
        ));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(
            t!("error.common.parse_json", err = e.to_string()).into_owned()
        ))?;

    // 业务级 code 检查
    let code = raw.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(FetchError::server(
            t!("error.common.business_code", provider = "StepFun", code = code, msg = msg).into_owned()
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

    let resp = client
        .post(URL_PLAN_STATUS)
        .header("Cookie", format!("Oasis-Token={token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body("{}")
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
fn parse(rate_raw: Value, plan_name: Option<String>) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let data = rate_raw
        .get("data")
        .ok_or_else(|| FetchError::parse(
            t!("error.common.missing_data_field", provider = "StepFun").into_owned()
        ))?;

    let mut rows = Vec::new();

    // 5h tier
    if let Some(left) = data.get("five_hour_usage_left_rate").and_then(|v| v.as_f64()) {
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

    if rows.is_empty() {
        return Err(FetchError::parse(
            "StepFun 响应里没找到有效的 *_usage_left_rate 字段".to_string(),
        ));
    }

    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(rate_raw),
        is_healthy: true,
        source_id: Some("stepfun".to_string()),
        source_display_name: Some("StepFun".to_string()),
        plan_name,
    })
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
        let snap = parse(raw.clone(), Some("Plus".to_string())).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("stepfun"));
        assert_eq!(snap.plan_name.as_deref(), Some("Plus"));
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, "5h");
        // 1.0 - 0.72 = 0.28 → 28%
        assert!((five_h.utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(five_h.unit.as_deref(), Some("%"));
        assert!(five_h.resets_at.is_some());

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, "周");
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
        let snap = parse(raw, None).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "5h");
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
        let snap = parse(raw, None).expect("parse");
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
        let snap = parse(raw, None).expect("parse");
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
        let snap = parse(raw, None).expect("parse");
        // 5h 跳过，只剩 weekly
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "周");
    }

    #[test]
    fn parse_no_data_is_error() {
        let raw = json!({ "code": 0 });
        let err = parse(raw, None).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_code_nonzero_is_error() {
        // 业务级 code != 0 应在 fetch_rate_limit 阶段就报错（这里 raw 直接 parse 不会触发）
        // parse 本身只检查 data 字段
        let raw = json!({ "code": 401, "message": "token expired" });
        let err = parse(raw, None).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);  // 缺 data 字段 → parse 错
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
}
