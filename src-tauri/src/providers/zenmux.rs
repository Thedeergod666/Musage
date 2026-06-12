//! ZenMux Platform API 余额监控
//!
//! 照 ccswitch `coding_plan.rs::query_zenmux` 实现：Bearer 鉴权、GET 用量 endpoint、
//! 解析 `quota_5_hour` + `quota_7_day` + `plan.tier` + `account_status`。
//!
//! ## Endpoint
//!
//! - `GET https://api.zenmux.ai/api/v1/usage` （猜的，ZenMux docs 没明说）
//! - Header: `Authorization: Bearer <api_key>`
//! - 鉴权可用 **Management API Key**（专门给 platform API 用），或普通 API key
//!
//! ## 响应 schema（来自 ZenMux QuickStart 例子的 ccswitch 翻译）
//!
//! ```json
//! {
//!   "success": true,
//!   "data": {
//!     "plan": { "tier": "ultra", "amount_usd": 200, "interval": "month" },
//!     "account_status": "healthy",
//!     "quota_5_hour": {
//!       "max_flows": 800, "used_flows": 57.2, "remaining_flows": 742.8,
//!       "usage_percentage": 0.0715
//!     },
//!     "quota_7_day": {
//!       "max_flows": 6182, "used_flows": 416.11, "remaining_flows": 5765.89
//!     }
//!   }
//! }
//! ```
//!
//! - `usage_percentage` 是 0-1 的小数（要 ×100）
//! - `resets_at` 在 ccswitch 解析里是字符串 ISO，但 ZenMux 公开 docs 没列；
//!   缺则 resets_at 留 None（前端不显示倒计时）
//!
//! ## 套餐名取自 `data.plan.tier`

use std::pin::Pin;
use std::sync::OnceLock;

use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};

const URL: &str = "https://api.zenmux.ai/api/v1/usage";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct ZenmuxSource {
    base_url: OnceLock<String>,
}

impl Default for ZenmuxSource {
    fn default() -> Self {
        Self {
            base_url: OnceLock::new(),
        }
    }
}

impl QuotaSource for ZenmuxSource {
    fn id(&self) -> &'static str { "zenmux" }
    fn display_name(&self) -> &'static str { "ZenMux" }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    /// 从 [`crate::config::AppConfig`] 拿用户配的 base_url（如果有），覆盖默认。
    /// Phase 2 之前先支持默认 URL；用户改 URL 走 ccswitch 风格的 base_url 字段。
    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // 优先从 providers.zenmux.base_url 读；缺失就保留默认
            if let Some(url) = cfg
                .pointer("/providers/zenmux/base_url")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                let _ = self.base_url.set(url.to_string());
            }
        })
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured("未配置 ZenMux API key（设置面板填入）"));
            }
            do_fetch(api_key, self.base_url.get().map(|s| s.as_str())).await
        })
    }
}

async fn do_fetch(
    api_key: &str,
    custom_url: Option<&str>,
) -> Result<ProviderSnapshot, FetchError> {
    let url = custom_url.unwrap_or(URL);
    let client = shared_client();

    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::network(format!("ZenMux 网络错误: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth("鉴权失败，请检查 ZenMux API key"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(format!(
            "ZenMux 服务异常 (HTTP {status}): {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

    parse(&raw)
}

/// 解析 ZenMux subscription 响应（照 ccswitch 翻译）
fn parse(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    if raw.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let msg = raw
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown error");
        return Err(FetchError::server(format!("ZenMux API error: {msg}")));
    }

    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::parse("响应缺少 data 字段".to_string()))?;

    let mut rows = Vec::new();

    // ── 5 小时窗口
    if let Some(q5h) = data.get("quota_5_hour") {
        let usage_ratio = num_f64(q5h, "usage_percentage").unwrap_or(0.0);
        let used = num_f64(q5h, "used_flows");
        let max = num_f64(q5h, "max_flows");
        rows.push(QuotaRow {
            label: "5h".to_string(),
            utilization: Some(usage_ratio * 100.0),
            remaining: num_f64(q5h, "remaining_flows"),
            used,
            total: max,
            resets_at: None, // ccswitch 解析 `resets_at` 字符串但 ZenMux docs 没列
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    // ── 周窗口
    if let Some(q7d) = data.get("quota_7_day") {
        let usage_ratio = num_f64(q7d, "usage_percentage").unwrap_or(0.0);
        let used = num_f64(q7d, "used_flows");
        let max = num_f64(q7d, "max_flows");
        rows.push(QuotaRow {
            label: "周".to_string(),
            utilization: Some(usage_ratio * 100.0),
            remaining: num_f64(q7d, "remaining_flows"),
            used,
            total: max,
            resets_at: None,
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse("响应里没找到 quota_5_hour / quota_7_day 字段".to_string()));
    }

    // ── 套餐名 + 账户状态
    let plan_tier = data
        .pointer("/plan/tier")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let account_status = data
        .get("account_status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let plan_name = if !plan_tier.is_empty() {
        if !account_status.is_empty() {
            Some(format!("{plan_tier} ({account_status})"))
        } else {
            Some(plan_tier.to_string())
        }
    } else {
        None
    };

    // 复用 Provider::Minimax 是因为 ZenMux 还没单独的 enum 变体（沿用 Tavily 模式）。
    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("zenmux".to_string()),
        source_display_name: Some("ZenMux".to_string()),
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
            "success": true,
            "data": {
                "plan": { "tier": "ultra", "amount_usd": 200, "interval": "month" },
                "account_status": "healthy",
                "quota_5_hour": {
                    "max_flows": 800, "used_flows": 57.2, "remaining_flows": 742.8,
                    "usage_percentage": 0.0715
                },
                "quota_7_day": {
                    "max_flows": 6182, "used_flows": 416.11, "remaining_flows": 5765.89,
                    "usage_percentage": 0.0673
                }
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.plan_name.as_deref(), Some("ultra (healthy)"));
        assert_eq!(snap.source_id.as_deref(), Some("zenmux"));
        // 2 rows: 5h + weekly
        assert_eq!(snap.rows.len(), 2);
        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, "5h");
        assert_eq!(five_h.used, Some(57.2));
        assert_eq!(five_h.total, Some(800.0));
        assert_eq!(five_h.remaining, Some(742.8));
        assert!((five_h.utilization.unwrap() - 7.15).abs() < 0.001);
        let week = &snap.rows[1];
        assert_eq!(week.label, "周");
        assert_eq!(week.used, Some(416.11));
        assert!((week.utilization.unwrap() - 6.73).abs() < 0.001);
    }

    #[test]
    fn parse_only_5h_no_7d() {
        let raw = json!({
            "success": true,
            "data": {
                "plan": { "tier": "pro" },
                "quota_5_hour": { "max_flows": 100, "used_flows": 10, "usage_percentage": 0.1 }
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "5h");
        assert_eq!(snap.plan_name.as_deref(), Some("pro"));
    }

    #[test]
    fn parse_success_false_is_error() {
        let raw = json!({
            "success": false,
            "message": "API key invalid"
        });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
        assert!(err.message.contains("API key invalid"));
    }

    #[test]
    fn parse_missing_data_is_error() {
        let raw = json!({ "success": true });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_no_quotas_is_error() {
        let raw = json!({
            "success": true,
            "data": { "plan": { "tier": "free" } }
        });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_no_plan_name() {
        let raw = json!({
            "success": true,
            "data": {
                "quota_5_hour": { "usage_percentage": 0.5, "used_flows": 5, "max_flows": 10 }
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.plan_name.is_none());
    }
}
