//! Kimi (Moonshot) For Coding 用量查询
//!
//! 端点：`GET https://api.kimi.com/coding/v1/usages`
//! 鉴权：`Authorization: Bearer <api_key>`
//!
//! ## 用途
//!
//! Kimi Coding Plan 是月之暗面（Moonshot AI）的编程套餐，跟 MiniMax 5h/周
//! 类似的滚动窗口设计。CCSwitch 已有 [同款实现](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/services/coding_plan.rs)
//! 可以参考（query_kimi + extract_reset_time 的容错处理）。
//!
//! ## 响应 schema
//!
//! ```json
//! {
//!   "limits": [
//!     {
//!       "detail": {
//!         "limit": 100,
//!         "remaining": 72,
//!         "resetTime": "2026-06-14T18:30:00.000Z"   // 也可能是 epoch 秒/毫秒
//!       }
//!     }
//!   ],
//!   "usage": {
//!     "limit": 1000,
//!     "remaining": 742,
//!     "resetTime": 1749840000                       // 数值（秒或毫秒）
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 第一行（5h 滚动窗口）：`body.limits[].detail.{limit, remaining}` → "28% used · 72/100"
//! - 第二行（周限额）：`body.usage.{limit, remaining}` → "26% used · 742/1000"
//! - `resetTime` 容错：字符串（ISO 8601）+ 数字（epoch 秒/毫秒自动识别）
//!
//! 字段名 / schema 参照 ccswitch；老套餐只回 `usage` 时只显示 1 行（自然降级）。

use std::borrow::Cow;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow, QuotaSource};
use crate::t;

const URL: &str = "https://api.kimi.com/coding/v1/usages";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct KimiSource;

impl Default for KimiSource {
    fn default() -> Self { Self }
}

impl QuotaSource for KimiSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("kimi") }
    fn display_name(&self) -> Cow<'_, str> { Cow::Owned(t!("provider_name.kimi").into_owned()) }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Kimi 无 region/模式/overrides 概念，忽略
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
                    t!("error.provider.unconfigured_key", provider = "Kimi").into_owned()
                ));
            }
            do_fetch(api_key).await
        })
    }
}

async fn do_fetch(api_key: &str) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    let resp = client
        .get(URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::network(
            t!("error.common.network", url = URL, err = e.to_string()).into_owned()
        ))?;

    let status = resp.status();
    // H6 fix: 429 显式 → RateLimited
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "Kimi").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "Kimi").into_owned()
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "Kimi",
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

    parse(&raw)
}

/// 解析 Kimi Coding usage 响应。
///
/// 解析失败时按 ROADMAP 策略返回 `Err(FetchError::Parse)`。
fn parse(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut rows = Vec::new();

    // ── 5 小时窗口：从 limits[].detail 取 ──
    if let Some(limits) = raw.get("limits").and_then(|v| v.as_array()) {
        for limit_item in limits {
            let Some(detail) = limit_item.get("detail") else { continue };
            let limit = parse_f64(detail.get("limit"));
            let remaining = parse_f64(detail.get("remaining"));
            let resets_at = extract_reset_ms(detail.get("resetTime"));

            if let (Some(l), Some(r)) = (limit, remaining) {
                if l > 0.0 {
                    let used = (l - r).max(0.0);
                    let utilization = (used / l) * 100.0;
                    rows.push(QuotaRow {
                        label: t!("row.five_hour").to_string(),
                        utilization: Some(utilization),
                        remaining: Some(r),
                        used: Some(used),
                        total: Some(l),
                        resets_at,
                        unit: Some("%".to_string()),
                        extra: None,
            kind: None,
                    });
                    break; // 只取第一条 5h 限额
                }
            }
        }
    }

    // ── 周限额：从顶层 usage 取 ──
    if let Some(usage) = raw.get("usage") {
        let limit = parse_f64(usage.get("limit"));
        let remaining = parse_f64(usage.get("remaining"));
        let resets_at = extract_reset_ms(usage.get("resetTime"));

        if let (Some(l), Some(r)) = (limit, remaining) {
            if l > 0.0 {
                let used = (l - r).max(0.0);
                let utilization = (used / l) * 100.0;
                rows.push(QuotaRow {
                    label: t!("row.weekly").to_string(),
                    utilization: Some(utilization),
                    remaining: Some(r),
                    used: Some(used),
                    total: Some(l),
                    resets_at,
                    unit: Some("%".to_string()),
                    extra: None,
            kind: None,
                });
            }
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            "Kimi 响应里没找到任何 usage/limits 字段".to_string(),
        ));
    }

    Ok(ProviderSnapshot {
        // 沿用 Provider::Minimax 是 Kimi 还没有自己的 enum 变体；
        // source_id 才是前端应该用的字段。Phase 2 改成自有变体。
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("kimi".to_string()),
        source_display_name: Some("Kimi".to_string()),
        plan_name: Some("Coding Plan".to_string()),
    })
}

// ── 工具函数 ─────────────────────────────────────────────────────

/// 解析 JSON 值为 f64，兼容数字和字符串格式（如 `100` 和 `"100"`）。
fn parse_f64(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_i64().map(|i| i as f64))
            .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// 从 JSON 值提取重置时间（毫秒），兼容字符串和数字格式。
/// - 字符串：直接解析为 ISO 8601 → 毫秒
/// - 数字：自动判断秒/毫秒（< 1e12 当作秒，否则毫秒）→ 毫秒
fn extract_reset_ms(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis());
    }
    if let Some(n) = v.as_i64() {
        let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
        // sanity check：转回 DateTime 避免溢出
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
            "limits": [
                {
                    "detail": {
                        "limit": 100,
                        "remaining": 72,
                        "resetTime": "2026-06-14T18:30:00.000Z"
                    }
                }
            ],
            "usage": {
                "limit": 1000,
                "remaining": 742,
                "resetTime": 1749840000   // epoch 秒
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("kimi"));
        assert_eq!(snap.plan_name.as_deref(), Some("Coding Plan"));
        // 2 rows: 5h + weekly
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, "5h");
        assert!((five_h.utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(five_h.remaining, Some(72.0));
        assert_eq!(five_h.total, Some(100.0));
        assert_eq!(five_h.used, Some(28.0));
        // resetTime ISO 8601 → 2026-06-14T18:30:00.000Z = 1771005000000 ms (approximate)
        assert!(five_h.resets_at.is_some());

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, "周");
        assert!((weekly.utilization.unwrap() - 25.8).abs() < 0.001);
        assert_eq!(weekly.remaining, Some(742.0));
        // epoch 秒 1749840000 → 1749840000000 ms
        assert_eq!(weekly.resets_at, Some(1749840000000));
    }

    #[test]
    fn parse_only_limits_no_usage() {
        // 老套餐只回 limits
        let raw = json!({
            "limits": [
                { "detail": { "limit": 50, "remaining": 50, "resetTime": null } }
            ]
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "5h");
        assert_eq!(snap.rows[0].resets_at, None);
    }

    #[test]
    fn parse_only_usage_no_limits() {
        let raw = json!({
            "usage": { "limit": 500, "remaining": 100, "resetTime": 1749840000000_i64 }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "周");
        assert_eq!(snap.rows[0].resets_at, Some(1749840000000));
    }

    #[test]
    fn parse_zero_limit_is_skipped() {
        // limit = 0 不展示（防御性，正常 schema 不会给）
        let raw = json!({
            "limits": [{ "detail": { "limit": 0, "remaining": 0 } }],
            "usage":  { "limit": 100, "remaining": 50 }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1); // 5h 被跳过
        assert_eq!(snap.rows[0].label, "周");
    }

    #[test]
    fn parse_empty_is_error() {
        let raw = json!({});
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_missing_limit_is_error() {
        let raw = json!({
            "limits": [{ "detail": { "remaining": 50 } }]
        });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn extract_reset_ms_handles_iso_string() {
        let v = json!("2026-06-14T18:30:00.000Z");
        let ms = extract_reset_ms(Some(&v)).expect("iso");
        assert!(ms > 1_700_000_000_000 && ms < 1_800_000_000_000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_seconds() {
        let v = json!(1749840000_i64);
        let ms = extract_reset_ms(Some(&v)).expect("secs");
        assert_eq!(ms, 1749840000000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_millis() {
        let v = json!(1749840000000_i64);
        let ms = extract_reset_ms(Some(&v)).expect("ms");
        assert_eq!(ms, 1749840000000);
    }

    #[test]
    fn extract_reset_ms_invalid_returns_none() {
        assert_eq!(extract_reset_ms(None), None);
        assert_eq!(extract_reset_ms(Some(&json!("not a date"))), None);
        // 远超合理范围的数（from_timestamp_millis 返回 None）→ None
        assert_eq!(extract_reset_ms(Some(&json!(i64::MAX))), None);
    }
}