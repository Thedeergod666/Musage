//! ZenMux Platform API 监控
//!
//! 支持两种模式（设置面板里切换）：
//! - **payg**（默认）：PAYG 余额。1 行「余额 $X.XX USD」+ 2 行细分（Top up / Bonus）
//!   - 端点：`GET https://zenmux.ai/api/v1/management/payg/balance`
//! - **subscription**：订阅用量窗口。2 行「5h / 周」+ 倒计时
//!   - 端点：`GET https://zenmux.ai/api/v1/management/subscription/detail`
//!
//! 两种模式都用 **Management API Key**（prefix `sk-mg-v1-`），在
//! [zenmux.ai/platform/management](https://zenmux.ai/platform/management) 创建。
//! 普通 `sk-...` 调模型的 key 不能用于平台 API。
//!
//! ## PAYG 响应 schema（[docs](https://zenmux.ai/docs/zh/api/platform/payg-balance.html)）
//!
//! ```json
//! {
//!   "success": true,
//!   "data": {
//!     "currency": "usd",
//!     "total_credits": 482.74,
//!     "top_up_credits": 35.0,
//!     "bonus_credits": 447.74
//!   }
//! }
//! ```
//!
//! `total_credits = top_up_credits + bonus_credits`（浮点精度可能有微小误差）
//!
//! ## Subscription 响应 schema（[docs](https://zenmux.ai/docs/zh/api/platform/subscription-detail.html)）
//!
//! ```json
//! {
//!   "success": true,
//!   "data": {
//!     "plan": { "tier": "ultra", "amount_usd": 200, "interval": "month",
//!               "expires_at": "2026-04-12T08:26:56.000Z" },
//!     "account_status": "healthy",
//!     "quota_5_hour": { "usage_percentage": 0.0715,
//!                       "resets_at": "2026-03-24T08:35:09.000Z",
//!                       "max_flows": 800, "used_flows": 57.2, "remaining_flows": 742.8 },
//!     "quota_7_day": { /* 同上结构 */ },
//!     "quota_monthly": { "max_flows": 34560 }   // 只有 max，无实时 usage
//!   }
//! }
//! ```

use std::pin::Pin;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};

const URL_PAYG: &str = "https://zenmux.ai/api/v1/management/payg/balance";
const URL_SUBSCRIPTION: &str = "https://zenmux.ai/api/v1/management/subscription/detail";

/// 监控模式：钱包余额（PAYG）or 订阅用量窗口。
/// Phase 2 之前是字符串序列化的"内部 API"，settings.ts 直接传字符串。
/// 加新 mode 只需在 `set_state` + `fetch` 里加分支 + 改 [`parse_*`]。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZenmuxMode {
    /// Pay As You Go 钱包余额（默认 —— 大多数用户都是 PAYG-only）
    #[default]
    Payg,
    /// 订阅用量窗口（5h / 7d），适合订阅 ZenMux 套餐的用户
    Subscription,
}

impl ZenmuxMode {
    fn default_url(&self) -> &'static str {
        match self {
            ZenmuxMode::Payg => URL_PAYG,
            ZenmuxMode::Subscription => URL_SUBSCRIPTION,
        }
    }
}

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct ZenmuxSource {
    /// 用户在设置面板里选的 mode（默认 Payg）
    mode: OnceLock<ZenmuxMode>,
    /// 用户可选填的自定义 endpoint（覆盖 mode 默认 URL）
    base_url: OnceLock<String>,
}

impl Default for ZenmuxSource {
    fn default() -> Self {
        Self {
            mode: OnceLock::new(),
            base_url: OnceLock::new(),
        }
    }
}

impl QuotaSource for ZenmuxSource {
    fn id(&self) -> &'static str { "zenmux" }
    fn display_name(&self) -> &'static str { "ZenMux" }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // mode：先看 `providers.zenmux.mode`（如果其他 CC 加了 ProviderConfig.mode），
            // 再看顶层 `zenmux_mode`（settings.ts 实际写入的位置）；都没有 = Payg。
            let mode_str = cfg
                .pointer("/providers/zenmux/mode")
                .and_then(|v| v.as_str())
                .or_else(|| cfg.get("zenmux_mode").and_then(|v| v.as_str()));
            let mode = mode_str.and_then(parse_mode).unwrap_or(ZenmuxMode::Payg);
            let _ = self.mode.set(mode);

            // base_url：同样支持两路径（顶层 = settings.ts 实际写的位置）
            let url = cfg
                .pointer("/providers/zenmux/base_url")
                .and_then(|v| v.as_str())
                .or_else(|| cfg.get("zenmux_base_url").and_then(|v| v.as_str()))
                .filter(|s| !s.is_empty());
            if let Some(url) = url {
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
                return Err(FetchError::unconfigured("未配置 ZenMux Management API key（设置面板填入）"));
            }
            let mode = self.mode.get().copied().unwrap_or_default();
            let custom_url = self.base_url.get().map(|s| s.as_str());
            do_fetch(api_key, mode, custom_url).await
        })
    }
}

fn parse_mode(s: &str) -> Option<ZenmuxMode> {
    match s {
        "payg" => Some(ZenmuxMode::Payg),
        "subscription" => Some(ZenmuxMode::Subscription),
        _ => None,
    }
}

async fn do_fetch(
    api_key: &str,
    mode: ZenmuxMode,
    custom_url: Option<&str>,
) -> Result<ProviderSnapshot, FetchError> {
    let url = custom_url.unwrap_or_else(|| mode.default_url());
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
        return Err(FetchError::auth(
            "鉴权失败 —— ZenMux 平台 API 需要 Management API Key（在 zenmux.ai/platform/management 创建）",
        ));
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

    match mode {
        ZenmuxMode::Payg => parse_payg(&raw),
        ZenmuxMode::Subscription => parse_subscription(&raw),
    }
}

// ── PAYG 解析 ─────────────────────────────────────────────────────

/// 解析 `Get PAYG Balance` 响应。
///
/// 渲染策略（仿 DeepSeek "余额 + Tavily 细分"）：
/// - 主行：`余额` + `total_credits` + `USD`（balance-row 样式，无 bar）
/// - 细分 1：`充值` + `top_up_credits`（only-used，无 bar）
/// - 细分 2：`奖励` + `bonus_credits`（only-used，无 bar）
fn parse_payg(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    if raw.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
        return Err(FetchError::server(format!("ZenMux API error: {msg}")));
    }

    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse("PAYG 响应缺少 data 字段".to_string())
    })?;

    // 货币：docs 说固定 "usd"，但允许其它值透传
    let currency = data
        .get("currency")
        .and_then(|v| v.as_str())
        .unwrap_or("usd")
        .to_uppercase();

    let total = num_f64(data, "total_credits")
        .ok_or_else(|| FetchError::parse("PAYG 响应缺少 total_credits".to_string()))?;

    let top_up = num_f64(data, "top_up_credits");
    let bonus = num_f64(data, "bonus_credits");

    let mut rows = Vec::new();

    // 主行 —— 余额（DeepSeek 风格，balance-row）
    rows.push(QuotaRow {
        label: "余额".to_string(),
        utilization: None,
        remaining: Some(total),
        used: None,
        total: None,
        resets_at: None,
        unit: Some(currency.clone()),
        extra: None,
    });

    // 细分：充值（Tavily 风格，only-used）
    if let Some(v) = top_up {
        rows.push(QuotaRow {
            label: "充值".to_string(),
            utilization: None,
            remaining: None,
            used: Some(v),
            total: None,
            resets_at: None,
            unit: Some(currency.clone()),
            extra: None,
        });
    }
    // 细分：奖励
    if let Some(v) = bonus {
        rows.push(QuotaRow {
            label: "奖励".to_string(),
            utilization: None,
            remaining: None,
            used: Some(v),
            total: None,
            resets_at: None,
            unit: Some(currency.clone()),
            extra: None,
        });
    }

    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax, // 沿用 Tavily 模式：复用 Minimax 占位
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("zenmux".to_string()),
        source_display_name: Some("ZenMux".to_string()),
        plan_name: Some("PAYG".to_string()),
    })
}

// ── Subscription 解析 ────────────────────────────────────────────

/// 解析 `Get Subscription Detail` 响应。
///
/// 5h / 7d 两个滚动窗口有实时 usage → 渲染成 2 行 quota row；
/// monthly 只有 max 没 usage → 跳过（0% 行没意义）。
fn parse_subscription(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    if raw.get("success").and_then(|v| v.as_bool()) != Some(true) {
        let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
        return Err(FetchError::server(format!("ZenMux API error: {msg}")));
    }

    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse("响应缺少 data 字段".to_string())
    })?;

    let mut rows = Vec::new();

    if let Some(q) = data.get("quota_5_hour") {
        if let Some(row) = parse_subscription_window(q, "5h") {
            rows.push(row);
        }
    }
    if let Some(q) = data.get("quota_7_day") {
        if let Some(row) = parse_subscription_window(q, "周") {
            rows.push(row);
        }
    }
    // monthly 只有 max（无实时 usage）→ 跳过

    if rows.is_empty() {
        return Err(FetchError::parse(
            "响应里没找到 quota_5_hour / quota_7_day 字段".to_string(),
        ));
    }

    let plan_tier = data.pointer("/plan/tier").and_then(|v| v.as_str()).unwrap_or("");
    let account_status = data.get("account_status").and_then(|v| v.as_str()).unwrap_or("");
    let plan_name = if !plan_tier.is_empty() {
        if !account_status.is_empty() {
            Some(format!("{plan_tier} ({account_status})"))
        } else {
            Some(plan_tier.to_string())
        }
    } else {
        None
    };

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

/// 解析单个 quota window → QuotaRow。`usage_percentage` 缺失则整行 None。
fn parse_subscription_window(q: &Value, label: &str) -> Option<QuotaRow> {
    let usage_ratio = num_f64(q, "usage_percentage")?;
    let used = num_f64(q, "used_flows");
    let max = num_f64(q, "max_flows");
    let remaining = num_f64(q, "remaining_flows");
    let resets_at = parse_iso8601_ms(q.get("resets_at").and_then(|v| v.as_str()));

    Some(QuotaRow {
        label: label.to_string(),
        utilization: Some(usage_ratio * 100.0),
        remaining,
        used,
        total: max,
        resets_at,
        unit: Some("%".to_string()),
        extra: None,
    })
}

// ── 工具 ────────────────────────────────────────────────────────

/// ISO 8601 字符串 → 毫秒时间戳。失败返 None。
fn parse_iso8601_ms(s: Option<&str>) -> Option<i64> {
    let s = s?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
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

    // ── PAYG 模式 ──

    #[test]
    fn parse_payg_full_response() {
        let raw = json!({
            "success": true,
            "data": {
                "currency": "usd",
                "total_credits": 482.74,
                "top_up_credits": 35.0,
                "bonus_credits": 447.74
            }
        });
        let snap = parse_payg(&raw).expect("parse_payg");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("zenmux"));
        assert_eq!(snap.plan_name.as_deref(), Some("PAYG"));
        // 3 rows: 余额 + 充值 + 奖励
        assert_eq!(snap.rows.len(), 3);
        let main = &snap.rows[0];
        assert_eq!(main.label, "余额");
        assert_eq!(main.remaining, Some(482.74));
        assert_eq!(main.unit.as_deref(), Some("USD"));
        // 充值 + 奖励 走 Tavily only-used 分支
        assert_eq!(snap.rows[1].label, "充值");
        assert_eq!(snap.rows[1].used, Some(35.0));
        assert_eq!(snap.rows[2].label, "奖励");
        assert_eq!(snap.rows[2].used, Some(447.74));
    }

    #[test]
    fn parse_payg_only_total_no_breakdown() {
        let raw = json!({
            "success": true,
            "data": {
                "currency": "usd",
                "total_credits": 10.5
            }
        });
        let snap = parse_payg(&raw).expect("parse_payg");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "余额");
        assert_eq!(snap.rows[0].remaining, Some(10.5));
    }

    #[test]
    fn parse_payg_missing_total_is_error() {
        let raw = json!({
            "success": true,
            "data": { "currency": "usd", "top_up_credits": 1.0 }
        });
        let err = parse_payg(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_payg_success_false_is_error() {
        let raw = json!({ "success": false, "message": "API key invalid" });
        let err = parse_payg(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
    }

    // ── Subscription 模式 ──

    #[test]
    fn parse_subscription_full_response() {
        let raw = json!({
            "success": true,
            "data": {
                "plan": { "tier": "ultra", "amount_usd": 200, "interval": "month",
                          "expires_at": "2026-04-12T08:26:56.000Z" },
                "account_status": "healthy",
                "quota_5_hour": {
                    "usage_percentage": 0.0715,
                    "resets_at": "2026-03-24T08:35:09.000Z",
                    "max_flows": 800, "used_flows": 57.2, "remaining_flows": 742.8
                },
                "quota_7_day": {
                    "usage_percentage": 0.0673,
                    "resets_at": "2026-03-26T02:15:05.000Z",
                    "max_flows": 6182, "used_flows": 416.11, "remaining_flows": 5765.89
                },
                "quota_monthly": { "max_flows": 34560 }
            }
        });
        let snap = parse_subscription(&raw).expect("parse_subscription");
        assert!(snap.success);
        assert_eq!(snap.plan_name.as_deref(), Some("ultra (healthy)"));
        assert_eq!(snap.rows.len(), 2); // monthly 跳过
        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, "5h");
        assert_eq!(five_h.used, Some(57.2));
        assert_eq!(five_h.total, Some(800.0));
        assert_eq!(five_h.remaining, Some(742.8));
        assert!((five_h.utilization.unwrap() - 7.15).abs() < 0.001);
        assert!(five_h.resets_at.is_some());
        let week = &snap.rows[1];
        assert_eq!(week.label, "周");
        assert!((week.utilization.unwrap() - 6.73).abs() < 0.001);
    }

    #[test]
    fn parse_subscription_only_5h() {
        let raw = json!({
            "success": true,
            "data": {
                "plan": { "tier": "pro" },
                "account_status": "monitored",
                "quota_5_hour": { "max_flows": 100, "used_flows": 10,
                                   "remaining_flows": 90, "usage_percentage": 0.1,
                                   "resets_at": "2026-06-12T15:00:00.000Z" }
            }
        });
        let snap = parse_subscription(&raw).expect("parse_subscription");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "5h");
        assert_eq!(snap.plan_name.as_deref(), Some("pro (monitored)"));
    }

    #[test]
    fn parse_subscription_resets_at_null_ok() {
        let raw = json!({
            "success": true,
            "data": {
                "plan": { "tier": "free" },
                "quota_5_hour": { "max_flows": 100, "usage_percentage": 0.0,
                                   "resets_at": null }
            }
        });
        let snap = parse_subscription(&raw).expect("parse_subscription");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].resets_at, None);
    }

    #[test]
    fn parse_subscription_suspended_account_status() {
        let raw = json!({
            "success": true,
            "data": {
                "plan": { "tier": "max" },
                "account_status": "suspended",
                "quota_5_hour": { "usage_percentage": 1.0, "max_flows": 800, "used_flows": 800 }
            }
        });
        let snap = parse_subscription(&raw).expect("parse_subscription");
        assert_eq!(snap.plan_name.as_deref(), Some("max (suspended)"));
    }

    #[test]
    fn parse_subscription_no_quotas_is_error() {
        let raw = json!({
            "success": true,
            "data": { "plan": { "tier": "free" } }
        });
        let err = parse_subscription(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_subscription_window_skipped_when_no_usage_pct() {
        let raw = json!({
            "success": true,
            "data": {
                "plan": { "tier": "ultra" },
                "quota_5_hour": { "max_flows": 800 } // 缺 usage_percentage
            }
        });
        let err = parse_subscription(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // ── 工具 ──

    #[test]
    fn parse_iso8601_works() {
        let ms = parse_iso8601_ms(Some("2026-03-24T08:35:09.000Z")).unwrap();
        assert!(ms > 1_700_000_000_000 && ms < 1_800_000_000_000);
        assert!(parse_iso8601_ms(None).is_none());
        assert!(parse_iso8601_ms(Some("not a date")).is_none());
    }

    #[test]
    fn parse_mode_strings() {
        assert_eq!(parse_mode("payg"), Some(ZenmuxMode::Payg));
        assert_eq!(parse_mode("subscription"), Some(ZenmuxMode::Subscription));
        assert_eq!(parse_mode("PAYG"), None); // 严格小写，frontend 必须传小写
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn default_url_per_mode() {
        assert_eq!(ZenmuxMode::Payg.default_url(), URL_PAYG);
        assert_eq!(ZenmuxMode::Subscription.default_url(), URL_SUBSCRIPTION);
    }

    #[test]
    fn default_mode_is_payg() {
        assert_eq!(ZenmuxMode::default(), ZenmuxMode::Payg);
    }

    #[tokio::test]
    async fn set_state_reads_top_level_mode() {
        // settings.ts 实际写到顶层 `zenmux_mode`（不是 providers.zenmux.mode）
        let src = ZenmuxSource::default();
        let cfg = json!({ "zenmux_mode": "subscription" });
        src.set_state(cfg).await;
        assert_eq!(src.mode.get().copied(), Some(ZenmuxMode::Subscription));
    }

    #[tokio::test]
    async fn set_state_reads_top_level_base_url() {
        let src = ZenmuxSource::default();
        let cfg = json!({ "zenmux_base_url": "https://custom.example/v1/x" });
        src.set_state(cfg).await;
        assert_eq!(src.base_url.get().map(|s| s.as_str()), Some("https://custom.example/v1/x"));
    }

    #[tokio::test]
    async fn set_state_defaults_to_payg_when_missing() {
        let src = ZenmuxSource::default();
        let cfg = json!({}); // 完全没有 zenmux_mode
        src.set_state(cfg).await;
        assert_eq!(src.mode.get().copied(), Some(ZenmuxMode::Payg));
        assert!(src.base_url.get().is_none());
    }

    #[tokio::test]
    async fn set_state_ignores_invalid_mode() {
        let src = ZenmuxSource::default();
        let cfg = json!({ "zenmux_mode": "BOGUS" });
        src.set_state(cfg).await;
        // 非法 mode → fallback 到 Payg（不 panic）
        assert_eq!(src.mode.get().copied(), Some(ZenmuxMode::Payg));
    }
}
