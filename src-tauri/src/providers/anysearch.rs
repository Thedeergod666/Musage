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
//! 用户操作（推荐一键登录，详见 [`crate::anysearch_login`]）：
//! 1. 设置面板点 “🔑 登录 AnySearch” → 弹 webview → 登录 anysearch.com
//! 2. 后端从 webview 的 localStorage 抽出 JWT → 写 keys.json
//! 3. 后台轮询用这个 JWT 拉数据；JWT 过期 (HTTP 401) 时错误信息引导重新登录
//!
//! 也支持手动兜底：把 JWT 整段粘到下面的 “Cookie / Token” 文本框（跟 cookie 字段共用
//! 存储槽位，[`AuthKind::Cookie`]）。
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
//! - 第一行（主指标）：`"523 / 1000 calls"` + 进度条（Free Plan 是 limited，1000/天）
//!   无限额时 `"523 calls"`（无进度条）
//! - 第二行（副指标）：`rate_limit_qps` → `"10 /秒"`（QPS 单位）
//! - 重置时间：从 `next_reset_at` 解析 → 填 `resets_at`（主指标行）
//! - 头部副标题：`plan_name = tier_name`（如 "Free Plan"）

use std::borrow::Cow;
use std::pin::Pin;

use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};

use crate::t;

/// console 内部用量端点（需要 user session JWT，不接受 `as_sk_` API key）。
/// 必须是 `/api/api/user/billing/overview` —— overview 页直接调它，
/// 返回用户的日/月配额、剩余、QPS、重置时间。`/api/api/user/keys` 只返
/// API key 元数据，不是用户用量。
const URL: &str = "https://www.anysearch.com/api/api/user/billing/overview";

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
            do_fetch(token, &self.unique_id(), &self.display_name().to_string()).await
        })
    }
}

async fn do_fetch(
    token: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    if token.trim().is_empty() {
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
        // JWT 过期 / 无效 —— 引导用户点浮窗「重新登录」（前端按 auth_failed 分发按钮）
        return Err(FetchError::auth(
            t!("error.anysearch.token_invalid_hint").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
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

    let raw: Value = resp.json().await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    // 业务级 code（0 = 成功）
    if let Some(code) = raw.get("code").and_then(|v| v.as_i64()) {
        if code != 0 {
            let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
            return Err(FetchError::server(
                t!("error.common.business_code", provider = "AnySearch", code = code, msg = msg)
                    .into_owned(),
            ));
        }
    }

    parse(&raw, source_id, display_name)
}

/// 解析 `/api/api/user/billing/overview` 响应。
///
/// data 必含 `used` + `total` + `remaining` + `rate_limit_qps` + `next_reset_at`
/// + `tier_name`。`next_reset_at` 是 ISO 8601 UTC 字符串（`"2026-07-23T00:00:00Z"`），
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
    let rate_qps = num_f64(data, "rate_limit_qps");
    let rate_unlimited = data
        .get("rate_limit_unlimited")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let is_active = data
        .get("is_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // total=0 / null / -1 = unlimited（schema 没明说，按 Tavily 那边的兜底约定）
    let is_unlimited = total.map(|t| t <= 0.0).unwrap_or(true);

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
            extra: None,
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
                extra: None,
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
                extra: None,
                kind: None,
            });
        }
    }

    // ── 副行：QPS 限速
    // rate_limit_qps=0 也显示（让用户知道"无 QPS 限"）；unlimited 不显示
    if !rate_unlimited {
        if let Some(qps) = rate_qps {
            rows.push(QuotaRow {
                label: t!("row.rate_limit").to_string(),
                utilization: None,
                remaining: None,
                used: Some(qps),
                total: None,
                resets_at: None,
                unit: Some(t!("row.calls_per_sec").to_string()),
                extra: None,
                kind: None,
            });
        }
    }

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

    let success = !rows.is_empty();
    Ok(ProviderSnapshot {
        provider: "anysearch".to_string(),
        success,
        rows,
        error: None,
        error_kind: None,
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
        // 2 行：quota + rate_limit
        assert_eq!(snap.rows.len(), 2);

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

        let rate = &snap.rows[1];
        assert_eq!(rate.label, t!("row.rate_limit"));
        assert_eq!(rate.used, Some(10.0));
        assert_eq!(rate.unit.as_deref(), Some(t!("row.calls_per_sec").as_ref()));
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
    fn parse_rate_limit_unlimited_hides_qps_row() {
        // rate_limit_unlimited=true → 不显示副行
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v["data"]["rate_limit_unlimited"] = json!(true);
        v["data"]["rate_limit_qps"] = json!(99999);
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        assert_eq!(snap.rows.len(), 1, "rate unlimited → 副行消失");
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
    fn parse_next_reset_at_missing_still_works() {
        // next_reset_at 缺失（罕见的 schema 漂移）→ resets_at=None，主行仍渲染
        let mut v: Value = serde_json::from_str(FREE_PLAN_OK).unwrap();
        v.as_object_mut().unwrap()["data"]
            .as_object_mut()
            .unwrap()
            .remove("next_reset_at");
        let snap = parse(&v, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success);
        assert!(snap.rows[0].resets_at.is_none(), "无 next_reset_at → resets_at=None");
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
