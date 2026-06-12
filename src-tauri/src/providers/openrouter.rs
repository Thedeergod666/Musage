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

use std::pin::Pin;

use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};

const URL_CREDITS: &str = "https://openrouter.ai/api/v1/credits";
const URL_KEY: &str = "https://openrouter.ai/api/v1/key";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct OpenrouterSource;

impl Default for OpenrouterSource {
    fn default() -> Self { Self }
}

impl QuotaSource for OpenrouterSource {
    fn id(&self) -> &'static str { "openrouter" }
    fn display_name(&self) -> &'static str { "OpenRouter" }
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
                return Err(FetchError::unconfigured("未配置 OpenRouter API key（设置面板填入）"));
            }
            do_fetch(api_key).await
        })
    }
}

async fn do_fetch(api_key: &str) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    // ── 第一优先：/api/v1/credits（账户余额，准确） ──
    match fetch_credits(client, api_key).await {
        Ok(snap) => return Ok(snap),
        Err(e) if matches!(e.kind, ErrorKind::AuthFailed | ErrorKind::ServerError) => {
            // Management key 被拒 / 5xx → fallback 到 /api/v1/key
            tracing::debug!(error = %e, "openrouter /credits 失败，fallback 到 /key");
        }
        Err(e) => return Err(e), // 网络 / 解析错误直接报
    }

    // ── fallback：/api/v1/key（per-key 限额，任何 key 都行） ──
    fetch_key(client, api_key).await
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
        .map_err(|e| FetchError::network(format!("OpenRouter 网络错误: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth("鉴权失败"));
    }
    if !status.is_success() {
        return Err(FetchError::server(format!("HTTP {status}")));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

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
        .map_err(|e| FetchError::network(format!("OpenRouter 网络错误: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth("鉴权失败，请检查 OpenRouter API key"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(format!(
            "OpenRouter 服务异常 (HTTP {status}): {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

    parse_key(&raw)
}

/// 解析 `/api/v1/credits` 响应 → 1 行「余额 $X.XX USD」
fn parse_credits(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::parse("credits 响应缺少 data 字段".to_string()))?;

    let total_credits = num_f64(data, "total_credits")
        .ok_or_else(|| FetchError::parse("credits 响应缺少 total_credits".to_string()))?;
    let total_usage = num_f64(data, "total_usage").unwrap_or(0.0);
    let remaining = (total_credits - total_usage).max(0.0);

    let rows = vec![QuotaRow {
        label: "余额".to_string(),
        utilization: None,
        remaining: Some(remaining),
        used: None,
        total: None,
        resets_at: None,
        unit: Some("USD".to_string()),
        extra: None,
    }];

    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("openrouter".to_string()),
        source_display_name: Some("OpenRouter".to_string()),
        plan_name: Some("OpenRouter".to_string()),
    })
}

/// 解析 `/api/v1/key` 响应 → 1 行「余额 $X.XX USD」（per-key fallback）
fn parse_key(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::parse("key 响应缺少 data 字段".to_string()))?;

    let remaining = num_f64(data, "limit_remaining");
    let limit = num_f64(data, "limit");
    let is_free_tier = data.get("is_free_tier").and_then(|v| v.as_bool()).unwrap_or(false);

    let plan_name = if is_free_tier {
        Some("Free tier".to_string())
    } else {
        Some("OpenRouter".to_string())
    };

    let mut rows = Vec::new();

    if let Some(r) = remaining {
        rows.push(QuotaRow {
            label: "余额".to_string(),
            utilization: None,
            remaining: Some(r),
            used: None,
            total: limit,
            resets_at: None,
            unit: Some("USD".to_string()),
            extra: None,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse("key 响应缺少 limit_remaining 字段".to_string()));
    }

    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("openrouter".to_string()),
        source_display_name: Some("OpenRouter".to_string()),
        plan_name,
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
        assert_eq!(row.label, "余额");
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
