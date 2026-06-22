//! OpenRouter 余额监控
//!
//! **策略**：先试 `/api/v1/credits`（账户余额），失败再试 `/api/v1/key`（per-key 限额）。
//!
//! ## 端点 1: `/api/v1/credits`（账户余额，需要 Management key）
//!
//! ```json
//! { "data": { "total_credits": 100.5, "total_usage": 25.75 } }
//! ```
//! 余额 = `total_credits - total_usage`（文档说 Management key required，
//! 但普通 key 也能用——OpenRouter 鉴权宽松）
//!
//! ## 端点 2: `/api/v1/key`（per-key 限额，任何 key 都行）
//!
//! ```json
//! { "data": { "limit": 100.0, "limit_remaining": 74.25,
//!             "is_free_tier": false, ... } }
//! ```
//!
//! **问题**：`limit_remaining` 是 per-key 级别的 credit limit，**不是账户余额**。
//! 账户可能有 $5 余额但 key 的 credit limit 是 $100 → 显示 $100 而不是 $5。
//!
//! 渲染：1 行「余额 $X.XX USD」（DeepSeek-style balance-row）

use std::borrow::Cow;
use std::pin::Pin;

use serde_json::Value;

use super::{shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow, QuotaSource};
use crate::t;

const URL_CREDITS: &str = "https://openrouter.ai/api/v1/credits";
const URL_KEY: &str = "https://openrouter.ai/api/v1/key";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct OpenrouterSource;

// M15 fix: fallback 缓存 —— /credits 和 /key 两个端点都可能被 401 / 5xx 拒绝。
// 之前每次 fetch 都要先试 /credits（失败）再试 /key，浪费 50% 请求。
// 缓存最近 5 分钟内成功的端点；TTL 过后重新探测（应对 endpoint 状态变化）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Credits,
    Key,
}
static LAST_SUCCESSFUL: std::sync::OnceLock<std::sync::Mutex<Option<(std::time::Instant, Endpoint)>>> =
    std::sync::OnceLock::new();

fn last_successful() -> &'static std::sync::Mutex<Option<(std::time::Instant, Endpoint)>> {
    LAST_SUCCESSFUL.get_or_init(|| std::sync::Mutex::new(None))
}

fn remember_endpoint(ep: Endpoint) {
    if let Ok(mut g) = last_successful().lock() {
        *g = Some((std::time::Instant::now(), ep));
    }
}

/// M12 fix: AuthFailed 时清缓存 —— 用户可能换了 key 类型（普通 → Management），
/// 下次 fetch 需要重新探测 /credits 和 /key，不能继续用 5 分钟前的成功记录。
fn clear_endpoint_cache() {
    if let Ok(mut g) = last_successful().lock() {
        *g = None;
    }
}

fn should_skip_endpoint(ep: Endpoint) -> bool {
    // 如果最近 5 分钟内有别的 endpoint 成功，跳过这个
    let Ok(g) = last_successful().lock() else { return false };
    match g.as_ref() {
        Some((ts, last)) if ts.elapsed() < std::time::Duration::from_secs(300) => last != &ep,
        _ => false,
    }
}

impl Default for OpenrouterSource {
    fn default() -> Self { Self }
}

impl QuotaSource for OpenrouterSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("openrouter") }
    fn display_name(&self) -> Cow<'_, str> { Cow::Borrowed("OpenRouter") }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "OpenRouter").into_owned()
                ));
            }
            do_fetch(api_key).await
        })
    }
}

async fn do_fetch(api_key: &str) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    // ── 第一优先：/api/v1/credits（账户余额，准确） ──
    // M15 fix: 最近 5 分钟内 /key 成功过 → 跳过 /credits 探测（避免重复 401 浪费请求）
    let try_credits = !should_skip_endpoint(Endpoint::Credits);
    if try_credits {
        match fetch_credits(client, api_key).await {
            Ok(snap) => {
                remember_endpoint(Endpoint::Credits);
                return Ok(snap);
            }
            Err(e) if matches!(e.kind, ErrorKind::AuthFailed | ErrorKind::ServerError) => {
                // M12 fix: AuthFailed 时清缓存，下次重新探测两个端点
                // (用户可能换了 key 类型：普通 → Management，/credits 应重试)
                if e.kind == ErrorKind::AuthFailed {
                    clear_endpoint_cache();
                }
                // Management key 被拒 / 5xx → fallback 到 /api/v1/key
                tracing::debug!(error = %e, "openrouter /credits 失败，fallback 到 /key");
            }
            Err(e) => return Err(e), // 网络 / 解析错误直接报
        }
    }

    // ── fallback：/api/v1/key（per-key 限额，任何 key 都行） ──
    match fetch_key(client, api_key).await {
        Ok(snap) => {
            remember_endpoint(Endpoint::Key);
            Ok(snap)
        }
        Err(e) => Err(e),
    }
}

/// `GET /api/v1/credits` → 账户余额
async fn fetch_credits(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let resp = client
        .get(URL_CREDITS)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::network(
            t!("error.common.network", url = URL_CREDITS, err = e.to_string()).into_owned()
        ))?;

    let status = resp.status();
    // H6 fix: 429 显式 → RateLimited（之前的 is_success() 兜底会归到 ServerError，
    // 触发 fallback 到 /key 端点，浪费请求）
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "OpenRouter").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "OpenRouter").into_owned()
        ));
    }
    if !status.is_success() {
        return Err(FetchError::server(
            t!("error.common.http_error_simple", provider = "OpenRouter", status = status.as_u16()).into_owned()
        ));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(
            t!("error.common.parse_json", err = e.to_string()).into_owned()
        ))?;

    parse_credits(&raw)
}

/// `GET /api/v1/key` → per-key 限额
async fn fetch_key(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let resp = client
        .get(URL_KEY)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::network(
            t!("error.common.network", url = URL_KEY, err = e.to_string()).into_owned()
        ))?;

    let status = resp.status();
    // 同 fetch_credits：429 显式 → RateLimited
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "OpenRouter").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "OpenRouter").into_owned()
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "OpenRouter",
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

    parse_key(&raw)
}

/// 解析 `/api/v1/credits` 响应 → 1 行「余额 $X.XX USD」
fn parse_credits(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::parse(
            t!("error.common.missing_field", provider = "OpenRouter", field = "data").into_owned()
        ))?;

    let total_credits = num_f64(data, "total_credits")
        .ok_or_else(|| FetchError::parse(
            t!("error.common.missing_field", provider = "OpenRouter", field = "total_credits").into_owned()
        ))?;
    let total_usage = num_f64(data, "total_usage").unwrap_or(0.0);
    let remaining = (total_credits - total_usage).max(0.0);

    let rows = vec![QuotaRow {
        label: t!("row.balance").to_string(),
        utilization: None,
        remaining: Some(remaining),
        used: None,
        total: None,
        resets_at: None,
        unit: Some("USD".to_string()),
        extra: None,
            kind: None,
    }];

    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("openrouter".to_string()),
        source_display_name: Some("OpenRouter".to_string()),
        plan_name: Some("OpenRouter".to_string()),
        transient: None,
    })
}

/// 解析 `/api/v1/key` 响应 → 1 行「余额 $X.XX USD」（per-key fallback）
fn parse_key(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::parse(
            t!("error.common.missing_data_field", provider = "OpenRouter").into_owned()
        ))?;

    let remaining = num_f64(data, "limit_remaining");
    let limit = num_f64(data, "limit");
    let is_free_tier = data.get("is_free_tier").and_then(|v| v.as_bool()).unwrap_or(false);

    let plan_name = if is_free_tier {
        Some(t!("row.free_tier").to_string())
    } else {
        Some("OpenRouter".to_string())
    };

    let mut rows = Vec::new();

    if let Some(r) = remaining {
        rows.push(QuotaRow {
            label: t!("row.balance").to_string(),
            utilization: None,
            remaining: Some(r),
            used: None,
            total: limit,
            resets_at: None,
            unit: Some("USD".to_string()),
            extra: None,
            kind: None,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.common.missing_field_generic", field = "limit_remaining").into_owned()
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
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("openrouter".to_string()),
        source_display_name: Some("OpenRouter".to_string()),
        plan_name,
        transient: None,
    })
}

fn num_f64(obj: &Value, field: &str) -> Option<f64> {
    obj.get(field).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|i| i as f64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── /credits 端点 ──

    #[test]
    fn parse_credits_full() {
        let raw = json!({
            "data": { "total_credits": 100.5, "total_usage": 25.75 }
        });
        let snap = parse_credits(&raw).expect("parse_credits");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        let row = &snap.rows[0];
        assert_eq!(row.label, t!("row.balance"));
        assert!((row.remaining.unwrap() - 74.75).abs() < 0.01);
        assert_eq!(row.unit.as_deref(), Some("USD"));
    }

    #[test]
    fn parse_credits_zero_balance() {
        let raw = json!({ "data": { "total_credits": 10.0, "total_usage": 10.0 } });
        let snap = parse_credits(&raw).expect("parse_credits");
        assert!((snap.rows[0].remaining.unwrap()).abs() < 0.01);
    }

    #[test]
    fn parse_credits_missing_total_credits() {
        let raw = json!({ "data": { "total_usage": 5.0 } });
        let err = parse_credits(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // ── /key 端点 ──

    #[test]
    fn parse_key_full() {
        let raw = json!({
            "data": {
                "label": "Musage 测试",
                "limit": 100.0,
                "limit_remaining": 74.25,
                "is_free_tier": false
            }
        });
        let snap = parse_key(&raw).expect("parse_key");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].remaining, Some(74.25));
        assert_eq!(snap.rows[0].unit.as_deref(), Some("USD"));
    }

    #[test]
    fn parse_key_free_tier_no_limit() {
        let raw = json!({
            "data": {
                "label": "free",
                "limit": null,
                "limit_remaining": null,
                "is_free_tier": true
            }
        });
        let err = parse_key(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_key_missing_data() {
        let raw = json!({ "error": "bad key" });
        let err = parse_key(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }
}
