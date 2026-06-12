//! Tavily Search API 用量查询
//!
//! 端点：`GET https://api.tavily.com/usage`
//! 鉴权：`Authorization: Bearer <api_key>`
//!
//! ## 用途
//!
//! Tavily 是 AI agent 常用的 search API（不是 LLM token plan），用来验证
//! "通用 quota source 抽象" 能不能承载非 LLM 场景 —— 它的响应展示的是具体数字
//!（"150 / 1000 credits"）而不是百分比，跟 MiniMax 5h/周这种 utilization% 不同。
//!
//! ## 响应 schema
//!
//! 实际格式（参考 [Tavily docs](https://docs.tavily.com/docs/rest-api/api-reference#endpoint-usage)）：
//! ```json
//! {
//!   "account": {
//!     "current_plan": "Researcher",
//!     "current_billing_period": { "start": "2026-06-01", "end": "2026-07-01" },
//!     "plan_usage": { "key": { "usage": 0, "limit": null } }
//!   },
//!   "key": {
//!     "usage": 150,
//!     "limit": 1000,
//!     "search_usage": 80,
//!     "extract_usage": 20,
//!     "crawl_usage": 0,
//!     "map_usage": 0,
//!     "research_usage": 50
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 第一行（主指标）：`"150 / 1000 credits"` —— 显示 used/total 数字
//! - 后续行（细分 endpoint）：每个一行，label 是 endpoint 名，used 是数字
//! - 头部副标题：`plan_name = account.current_plan`

use std::pin::Pin;

use chrono::{NaiveDate, NaiveTime};
use serde_json::Value;

use super::{shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow, QuotaSource};

const URL: &str = "https://api.tavily.com/usage";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct TavilySource;

impl Default for TavilySource {
    fn default() -> Self { Self }
}

impl QuotaSource for TavilySource {
    fn id(&self) -> &'static str { "tavily" }
    fn display_name(&self) -> &'static str { "Tavily" }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Tavily 无 region 概念，忽略
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured("未配置 Tavily API key（设置面板填入）"));
            }
            do_fetch(api_key).await
        })
    }
}

async fn do_fetch(api_key: &str) -> Result<ProviderSnapshot, FetchError> {
    if api_key.trim().is_empty() {
        return Err(FetchError::unconfigured("API key 为空"));
    }

    let client = shared_client();

    let resp = client
        .get(URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::network(format!("Tavily 网络错误: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth("鉴权失败，请检查 Tavily API key"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(format!(
            "Tavily 服务异常 (HTTP {status}): {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

    parse(&raw)
}

/// 解析 Tavily usage 响应。
///
/// 解析失败时按 ROADMAP 策略返回 `Err(FetchError::Parse)`，让前端能正确分类。
fn parse(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let key = raw
        .get("key")
        .ok_or_else(|| FetchError::parse("Tavily 响应缺少 key 字段".to_string()))?;

    let used = num_f64(key, "usage");
    let mut limit = num_f64(key, "limit");

    // Tavily API 对 Researcher plan 返回 "limit": null，
    // 但实际有 1000 credits/月上限。按 plan_name 兜底。
    if limit.is_none() {
        let plan = raw
            .pointer("/account/current_plan")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        limit = match plan {
            "Research" | "Researcher" => Some(1000.0),
            _ => None,
        };
    }

    // plan_name 来自 account.current_plan（不在就 None，让前端不显示副标题）
    let plan_name = raw
        .pointer("/account/current_plan")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // ── 套餐重置时间：从 account.current_billing_period.end 提取 ──
    let resets_at: Option<i64> = raw
        .pointer("/account/current_billing_period/end")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .map(|d| {
                    d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
                        .and_utc()
                        .timestamp_millis()
                })
        });

    let mut rows = Vec::new();

    // ── 主行："已用 / 总量 credits"（limit 可能是 null = 无限）
    if let (Some(u), Some(l)) = (used, limit) {
        if l > 0.0 {
            rows.push(QuotaRow {
                label: "Free tier".to_string(),
                utilization: Some((u / l) * 100.0),
                remaining: Some((l - u).max(0.0)),
                used: Some(u),
                total: Some(l),
                resets_at,
                unit: Some("credits".to_string()),
                extra: None,
            });
        } else {
            // limit = 0 → 理论上不该出现，但保险起见也列
            rows.push(QuotaRow {
                label: "Free tier".to_string(),
                utilization: None,
                remaining: None,
                used: Some(u),
                total: None,
                resets_at,
                unit: Some("credits".to_string()),
                extra: None,
            });
        }
    } else if let Some(u) = used {
        // 没有 limit（无限制套餐）：只显示 used
        rows.push(QuotaRow {
            label: "Free tier".to_string(),
            utilization: None,
            remaining: None,
            used: Some(u),
            total: None,
            resets_at,
            unit: Some("credits".to_string()),
            extra: None,
        });
    }

    // ── 细分 endpoint：每个一行（值 = 0 的也显示，让用户看到"没用过"）
    let endpoints: &[(&str, &str)] = &[
        ("search_usage", "search"),
        ("extract_usage", "extract"),
        ("crawl_usage", "crawl"),
        ("map_usage", "map"),
        ("research_usage", "research"),
    ];
    for (key_name, label) in endpoints {
        if let Some(n) = num_f64(key, key_name) {
            rows.push(QuotaRow {
                label: (*label).to_string(),
                utilization: None,
                remaining: None,
                used: Some(n),
                total: None,
                resets_at: None,
                unit: Some("calls".to_string()),
                extra: None,
            });
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse("Tavily 响应里没找到任何 usage 字段".to_string()));
    }

    let success = !rows.is_empty();
    Ok(ProviderSnapshot {
        // 复用 Provider::Minimax 是因为 Tavily 还没单独的 enum 变体；
        // source_id 才是前端应该用的字段。Phase 2 改成自有变体。
        provider: super::Provider::Minimax,
        success,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: success,
        source_id: Some("tavily".to_string()),
        source_display_name: Some("Tavily".to_string()),
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

    #[test]
    fn parse_full_response() {
        let raw = json!({
            "account": {
                "current_plan": "Research",
                "current_billing_period": { "start": "2026-06-01", "end": "2026-07-01" }
            },
            "key": {
                "usage": 150,
                "limit": 1000,
                "search_usage": 80,
                "extract_usage": 20,
                "crawl_usage": 0,
                "map_usage": 0,
                "research_usage": 50
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.plan_name.as_deref(), Some("Research"));
        assert_eq!(snap.source_id.as_deref(), Some("tavily"));
        // 6 rows: free tier + 5 endpoints
        assert_eq!(snap.rows.len(), 6);
        // First row: 150/1000 credits, 15% used
        let main = &snap.rows[0];
        assert_eq!(main.label, "Free tier");
        assert_eq!(main.unit.as_deref(), Some("credits"));
        assert_eq!(main.used, Some(150.0));
        assert_eq!(main.total, Some(1000.0));
        assert!((main.utilization.unwrap() - 15.0).abs() < 0.001);
        // resets_at: 2026-07-01 00:00 UTC → millis
        let expected_reset = NaiveDate::from_ymd_opt(2026, 7, 1)
            .unwrap()
            .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
            .and_utc()
            .timestamp_millis();
        assert_eq!(main.resets_at, Some(expected_reset));
        // Endpoint rows
        assert_eq!(snap.rows[1].label, "search");
        assert_eq!(snap.rows[1].used, Some(80.0));
        assert_eq!(snap.rows[5].label, "research");
        assert_eq!(snap.rows[5].used, Some(50.0));
    }

    #[test]
    fn parse_no_limit() {
        let raw = json!({
            "account": { "current_plan": "Pay-as-you-go" },
            "key": { "usage": 42, "limit": null }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.rows[0].used, Some(42.0));
        assert_eq!(snap.rows[0].total, None);
        assert!(snap.rows[0].utilization.is_none());
    }

    #[test]
    fn parse_research_plan_limit_fallback() {
        // Researcher plan API 返回 limit=null，但实际有 1000 credits 上限
        let raw = json!({
            "account": {
                "current_plan": "Researcher",
                "current_billing_period": { "start": "2026-06-01", "end": "2026-07-01" }
            },
            "key": { "usage": 765, "limit": null }
        });
        let snap = parse(&raw).expect("parse");
        let main = &snap.rows[0];
        assert_eq!(main.used, Some(765.0));
        assert_eq!(main.total, Some(1000.0));
        assert!((main.utilization.unwrap() - 76.5).abs() < 0.001);
        // resets_at 也应被设置
        assert!(main.resets_at.is_some());
    }

    #[test]
    fn parse_no_plan() {
        let raw = json!({
            "key": { "usage": 10, "limit": 100 }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.plan_name.is_none());
        // 无 billing period → resets_at 应为 None
        assert!(snap.rows[0].resets_at.is_none());
    }

    #[test]
    fn parse_missing_key_field_is_error() {
        let raw = json!({ "account": {} });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_empty_key_is_error() {
        let raw = json!({ "key": {} });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }
}
