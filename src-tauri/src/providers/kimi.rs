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
//! - 第一行（5h 滚动窗口）：`body.limits[].detail.{limit, remaining}`，kind = FiveHour
//! - 第二行（7 天滚动窗口）：`body.usage.{limit, remaining}`，kind = Weekly
//! - 浮窗左侧标签按 resets_at 动态显示窗口剩余（"5h" / "7d"），不显示 used/total
//! - `resetTime` 容错：字符串（ISO 8601）+ 数字（epoch 秒/毫秒自动识别）
//!
//! 字段名 / schema 参照 ccswitch；老套餐只回 `usage` 时只显示 1 行（自然降级）。

use std::borrow::Cow;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource, RowKind,
};
use crate::t;

const URL: &str = "https://api.kimi.com/coding/v1/usages";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct KimiSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for KimiSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl KimiSource {
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

impl QuotaSource for KimiSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("kimi")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "kimi".to_string()
        } else {
            format!("kimi#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.kimi").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.kimi").as_ref(),
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
        // Kimi 无 region/模式/overrides 概念，忽略
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "Kimi").into_owned(),
                ));
            }
            do_fetch(api_key, &self.unique_id(), &self.display_name().to_string()).await
        })
    }
}

async fn do_fetch(
    api_key: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    let resp = client
        .get(URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = URL, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    // H6 fix + L1 fix：429 显式 → RateLimited
    // helper 复用(L1):其它 provider 的 HTTP status → ErrorKind 分类统一走
    // `classify_http_status`,本文件保留对它的快速短路以便让具体 message
    // 走 rate_limited 模板(其它 status 走 http_error 模板),但 kind 计算
    // 复用 helper —— 加 402 Payment Required 时 11 处自动跟上。
    let kind = crate::providers::classify_http_status(status);
    if kind == ErrorKind::RateLimited {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "Kimi").into_owned(),
        ));
    }
    if kind == ErrorKind::AuthFailed {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "Kimi").into_owned(),
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
            )
            .into_owned(),
        ));
    }

    let raw: Value = resp.json().await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    parse(&raw, source_id, display_name)
}

/// 解析 Kimi Coding usage 响应。
///
/// 解析失败时按 ROADMAP 策略返回 `Err(FetchError::Parse)`。
fn parse(raw: &Value, source_id: &str, display_name: &str) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut rows = Vec::new();

    // ── 5 小时窗口：从 limits[].detail 取 ──
    if let Some(limits) = raw.get("limits").and_then(|v| v.as_array()) {
        for limit_item in limits {
            let Some(detail) = limit_item.get("detail") else {
                continue;
            };
            let resets_at = extract_reset_ms(detail.get("resetTime"));
            if let Some(row) = build_window_row(
                detail,
                t!("row.five_hour").to_string(),
                RowKind::FiveHour,
                resets_at,
            ) {
                rows.push(row);
                break; // 只取第一条 5h 限额
            }
        }
    }

    // ── 周限额：从顶层 usage 取 ──
    if let Some(usage) = raw.get("usage") {
        let resets_at = extract_reset_ms(usage.get("resetTime"));
        if let Some(row) = build_window_row(
            usage,
            t!("row.weekly").to_string(),
            RowKind::Weekly,
            resets_at,
        ) {
            rows.push(row);
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    Ok(ProviderSnapshot {
        // provider 字段: v0.2 删 enum 后从 Provider::Minimax 改成
        // "minimax" string 占位。前端走 source_id ("kimi") 路由,
        // 这个字段仅给老 JSON 反序列化兜底 (#[serde(default)] 让空 / 缺失
        // 字段不报错)
        provider: "kimi".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name: Some("Coding Plan".to_string()),
        transient: None,
    })
}

// ── 工具函数 ─────────────────────────────────────────────────────

/// 从一个 `{limit, remaining, used}` 对象构造窗口行（5h / 周）。
///
/// **回归修复（2026-07-17）**：5h 窗口达到 100% 上限时，Kimi API 会把
/// `remaining` 字段翻成 `0`、或干脆**省略**该字段（只回 `limit` / `used`）。
/// 旧逻辑严格要求 `limit` 与 `remaining` 同时 `Some` 才建行，导致 5h 达上限后
/// 整行 drop —— 浮窗里 5h 行消失（周限还在，因为周还没满）。跟 MiniMax 之前
/// 的 `status` 门控 bug（commit 7af0755）同源。
///
/// 新策略（对齐 ccswitch `query_kimi` 的 `unwrap_or` 容错）：
/// - `limit` 缺失 / <= 0 → 无法算百分比，返回 None（自然降级，非上限态）
/// - `remaining` 缺失 → 优先用显式 `used` 字段；再退化为 `0`（= 已用满 100%）
/// - `used` 优先取显式字段，否则用 `limit - remaining`
///
/// 这样只要拿到合法 `limit`，行就一定存在，哪怕 remaining/used 在上限态被省略。
fn build_window_row(
    obj: &Value,
    label: String,
    kind: RowKind,
    resets_at: Option<i64>,
) -> Option<QuotaRow> {
    let limit = parse_f64(obj.get("limit"))?;
    if limit <= 0.0 {
        return None;
    }
    // remaining 缺失时：先看显式 used，能反推就反推；否则视为已用满（0 剩余）。
    let explicit_used = parse_f64(obj.get("used"));
    let remaining = parse_f64(obj.get("remaining"))
        .unwrap_or_else(|| explicit_used.map(|u| (limit - u).max(0.0)).unwrap_or(0.0));
    let used = explicit_used.unwrap_or_else(|| (limit - remaining).max(0.0));
    // clamp：防御 used > limit 的异常上限态渲染出 >100% 的 bar
    let utilization = ((used / limit) * 100.0).clamp(0.0, 100.0);
    Some(QuotaRow {
        label,
        utilization: Some(utilization),
        remaining: Some(remaining),
        used: Some(used),
        total: Some(limit),
        resets_at,
        unit: Some("%".to_string()),
        extra: None,
        kind: Some(kind),
    })
}

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
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("kimi"));
        assert_eq!(snap.plan_name.as_deref(), Some("Coding Plan"));
        // 2 rows: 5h + weekly
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        assert_eq!(five_h.kind, Some(RowKind::FiveHour));
        assert!((five_h.utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(five_h.remaining, Some(72.0));
        assert_eq!(five_h.total, Some(100.0));
        assert_eq!(five_h.used, Some(28.0));
        // resetTime ISO 8601 → 2026-06-14T18:30:00.000Z = 1771005000000 ms (approximate)
        assert!(five_h.resets_at.is_some());

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, t!("row.weekly"));
        assert_eq!(weekly.kind, Some(RowKind::Weekly));
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
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.five_hour").as_ref());
        assert_eq!(snap.rows[0].resets_at, None);
    }

    #[test]
    fn parse_only_usage_no_limits() {
        let raw = json!({
            "usage": { "limit": 500, "remaining": 100, "resetTime": 1749840000000_i64 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.weekly"));
        assert_eq!(snap.rows[0].resets_at, Some(1749840000000));
    }

    #[test]
    fn parse_zero_limit_is_skipped() {
        // limit = 0 不展示（防御性，正常 schema 不会给）
        let raw = json!({
            "limits": [{ "detail": { "limit": 0, "remaining": 0 } }],
            "usage":  { "limit": 100, "remaining": 50 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1); // 5h 被跳过
        assert_eq!(snap.rows[0].label, t!("row.weekly"));
    }

    #[test]
    fn parse_empty_is_error() {
        let raw = json!({});
        let err = parse(&raw, "kimi", "Kimi").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_missing_limit_is_error() {
        let raw = json!({
            "limits": [{ "detail": { "remaining": 50 } }]
        });
        let err = parse(&raw, "kimi", "Kimi").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    // ── 回归：5h 达 100% 上限后行不消失（2026-07-17）───────────────

    #[test]
    fn parse_5h_exhausted_remaining_zero_keeps_row() {
        // 5h 达上限：remaining=0。旧逻辑 (Some,Some) 门控其实能过,但确认
        // 100% utilization 正常建行(不被后续 clamp / 空判 drop)。
        let raw = json!({
            "limits": [{ "detail": { "limit": 100, "remaining": 0, "resetTime": 1749840000 } }],
            "usage": { "limit": 1000, "remaining": 530 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 2, "5h + weekly 都要在");
        let five_h = &snap.rows[0];
        assert_eq!(five_h.kind, Some(RowKind::FiveHour));
        assert!((five_h.utilization.unwrap() - 100.0).abs() < 0.001);
        assert!(five_h.resets_at.is_some());
    }

    #[test]
    fn parse_5h_exhausted_remaining_omitted_keeps_row() {
        // **核心回归**：5h 达上限时 API 省略 remaining 字段,只回 limit(+used)。
        // 旧逻辑 `(Some(l), Some(r))` 门控 → r=None → 整行 drop → 浮窗 5h 消失。
        // 新逻辑：remaining 缺失退化为已用满 → 100% 行仍在。
        let raw = json!({
            "limits": [{ "detail": { "limit": 100, "resetTime": 1749840000 } }],
            "usage": { "limit": 1000, "remaining": 742 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 2, "remaining 省略时 5h 行不能消失");
        let five_h = &snap.rows[0];
        assert_eq!(five_h.kind, Some(RowKind::FiveHour));
        assert!((five_h.utilization.unwrap() - 100.0).abs() < 0.001);
        assert_eq!(five_h.total, Some(100.0));
        assert_eq!(five_h.remaining, Some(0.0));
    }

    #[test]
    fn parse_window_row_prefers_explicit_used() {
        // 某些 schema 回 used 而不回 remaining（codexbar/usagebar 观测形态）。
        // used=139, limit=200 → utilization=69.5%, remaining 反推=61。
        let raw = json!({
            "limits": [{ "detail": { "limit": 200, "used": 139, "resetTime": 1749840000 } }]
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        let five_h = &snap.rows[0];
        assert!((five_h.utilization.unwrap() - 69.5).abs() < 0.001);
        assert_eq!(five_h.used, Some(139.0));
        assert_eq!(five_h.remaining, Some(61.0));
    }

    #[test]
    fn parse_window_row_clamps_over_limit() {
        // 防御：used > limit（异常上限态）不渲染 >100% 的 bar。
        let raw = json!({
            "usage": { "limit": 100, "used": 130 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert!((snap.rows[0].utilization.unwrap() - 100.0).abs() < 0.001);
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
