//! AnySearch 用量查询 —— cookie 路径（登录 webview 自动提取 session JWT）
//!
//! ⚠️ AnySearch 的用量查询**只走 console 内部 API**，不是公开 endpoint，也**不接受**
//! `as_sk_` 形式的 MCP API key（实测 `as_sk_` 调 console 端点返 401
//! “管理员会话或用户访问令牌无效”）：
//! - `GET https://www.anysearch.com/api/api/user/keys`
//! - 鉴权：`Authorization: Bearer <user_session_jwt>` —— 用户在 anysearch.com 登录后
//!   浏览器 localStorage `search-template-auth-state.state.accessToken` 里的那个 JWT。
//!
//! 用户操作（推荐一键登录，详见 [`crate::anysearch_login`]）：
//! 1. 设置面板点 “🔑 登录 AnySearch” → 弹 webview → 登录 anysearch.com
//! 2. 后端用 `document.title` 通道从 webview 的 localStorage 抽出 JWT → 写 keys.json
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
//!     "count": 1,
//!     "keys": [{
//!       "id": "key_...",
//!       "name": "default",
//!       "key_prefix": "as_sk_1b3a66...",
//!       "is_active": true,
//!       "is_unlimited": false,
//!       "quota_is_unlimited": true,
//!       "quota_limit": 0,        // 0 / quota_is_unlimited=true → 无上限
//!       "quota_used": 2175,      // ← 已用次数（主指标）
//!       "rate_limit": 20,        // ← calls/min（副指标）
//!       "last_used_at": "2026-07-22T06:34:27Z",
//!       "created_at": "2026-07-14T09:26:23Z"
//!     }]
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 第一行（主指标）：无上限 → `"已用 N calls"`（无进度条）；有上限 → `"N / M calls"` + 进度条
//! - 第二行（副指标）：`rate_limit` → `"20 calls/min"`
//! - 头部副标题：`plan_name = keys[0].name`（如 "default"）

use std::borrow::Cow;
use std::pin::Pin;

use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};

use crate::t;

/// console 内部用量端点（需要 user session JWT，不接受 `as_sk_` API key）。
const URL: &str = "https://www.anysearch.com/api/api/user/keys";

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

/// 解析 `/api/api/user/keys` 响应。
///
/// 取 `data.keys[0]`（用户当前账号通常就 1 把 key；多把时取第一把，跟 console
/// 列表顺序一致）。`keys` 为空 / 缺 `data` 走 schema/parse 错误，让前端正确分类。
fn parse(raw: &Value, source_id: &str, display_name: &str) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let keys = raw
        .pointer("/data/keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            FetchError::parse(
                t!(
                    "error.common.missing_field",
                    provider = "AnySearch",
                    field = "data.keys"
                )
                .into_owned(),
            )
        })?;

    let key = keys.first().ok_or_else(|| {
        // 账号存在但一把 key 都没有 —— 引导去 console 建 key（schema 没坏，是空数据）
        FetchError::new(
            super::ErrorKind::SchemaUnknown,
            t!("error.anysearch.no_keys").into_owned(),
        )
    })?;

    let used = num_f64(key, "quota_used").unwrap_or(0.0);
    let limit = num_f64(key, "quota_limit");
    let is_unlimited = key
        .get("quota_is_unlimited")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || limit.map(|l| l <= 0.0).unwrap_or(true);
    let rate_limit = num_f64(key, "rate_limit");
    let is_active = key
        .get("is_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let plan_name = key.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());

    let mut rows = Vec::new();

    // ── 主行：已用 / 总量 calls
    if is_unlimited {
        // 无上限：只显示已用数字，无进度条（utilization=None → health 走 ok 分支）
        rows.push(QuotaRow {
            label: t!("row.quota").to_string(),
            utilization: None,
            remaining: None,
            used: Some(used),
            total: None,
            resets_at: None,
            unit: Some(t!("row.calls").to_string()),
            extra: None,
            kind: None,
        });
    } else if let Some(l) = limit {
        if l > 0.0 {
            rows.push(QuotaRow {
                label: t!("row.quota").to_string(),
                // H4-style clamp：超用时不越界
                utilization: Some((used / l * 100.0).clamp(0.0, 100.0)),
                remaining: Some((l - used).max(0.0)),
                used: Some(used),
                total: Some(l),
                resets_at: None,
                unit: Some(t!("row.calls").to_string()),
                extra: None,
                kind: None,
            });
        } else {
            // limit=0 但 is_unlimited=false 的边角：只显示已用
            rows.push(QuotaRow {
                label: t!("row.quota").to_string(),
                utilization: None,
                remaining: None,
                used: Some(used),
                total: None,
                resets_at: None,
                unit: Some(t!("row.calls").to_string()),
                extra: None,
                kind: None,
            });
        }
    }

    // ── 副行：rate limit (calls/min) —— 0 也显示，让用户知道节流上限
    if let Some(rl) = rate_limit {
        rows.push(QuotaRow {
            label: t!("row.rate_limit").to_string(),
            utilization: None,
            remaining: None,
            used: Some(rl),
            total: None,
            resets_at: None,
            unit: Some(t!("row.calls_per_min").to_string()),
            extra: None,
            kind: None,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "AnySearch",
                field = "quota_used"
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
        // key 被禁用 = 不健康（浮窗翻红，提示去 console 看）
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

    #[test]
    fn parse_unlimited_shows_used_no_bar() {
        let raw = json!({
            "code": 0,
            "data": { "count": 1, "keys": [{
                "name": "default",
                "is_active": true,
                "quota_is_unlimited": true,
                "quota_limit": 0,
                "quota_used": 2175,
                "rate_limit": 20
            }]}
        });
        let snap = parse(&raw, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.plan_name.as_deref(), Some("default"));
        assert_eq!(snap.source_id.as_deref(), Some("anysearch"));
        assert!(snap.is_healthy);
        // 2 行：quota + rate_limit
        assert_eq!(snap.rows.len(), 2);
        let main = &snap.rows[0];
        assert_eq!(main.label, t!("row.quota"));
        assert_eq!(main.used, Some(2175.0));
        assert_eq!(main.total, None, "无上限 → total=None");
        assert!(main.utilization.is_none(), "无上限 → 无进度条");
        assert_eq!(main.unit.as_deref(), Some(t!("row.calls").as_ref()));
        let rate = &snap.rows[1];
        assert_eq!(rate.label, t!("row.rate_limit"));
        assert_eq!(rate.used, Some(20.0));
        assert_eq!(rate.unit.as_deref(), Some(t!("row.calls_per_min").as_ref()));
    }

    #[test]
    fn parse_limited_shows_utilization_and_remaining() {
        let raw = json!({
            "code": 0,
            "data": { "keys": [{
                "name": "pro",
                "is_active": true,
                "quota_is_unlimited": false,
                "quota_limit": 1000,
                "quota_used": 250,
                "rate_limit": 60
            }]}
        });
        let snap = parse(&raw, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success);
        let main = &snap.rows[0];
        assert_eq!(main.used, Some(250.0));
        assert_eq!(main.total, Some(1000.0));
        assert_eq!(main.remaining, Some(750.0));
        assert!((main.utilization.unwrap() - 25.0).abs() < 0.001);
    }

    #[test]
    fn parse_limited_clamps_overuse() {
        // 超用：used > limit → utilization clamp 到 100，remaining clamp 到 0
        let raw = json!({
            "code": 0,
            "data": { "keys": [{
                "is_active": true,
                "quota_is_unlimited": false,
                "quota_limit": 100,
                "quota_used": 150,
                "rate_limit": 10
            }]}
        });
        let snap = parse(&raw, "anysearch", "AnySearch").expect("parse");
        let main = &snap.rows[0];
        assert!((main.utilization.unwrap() - 100.0).abs() < 0.001);
        assert_eq!(main.remaining, Some(0.0));
    }

    #[test]
    fn parse_inactive_key_marks_unhealthy() {
        let raw = json!({
            "code": 0,
            "data": { "keys": [{
                "is_active": false,
                "quota_is_unlimited": true,
                "quota_limit": 0,
                "quota_used": 5,
                "rate_limit": 10
            }]}
        });
        let snap = parse(&raw, "anysearch", "AnySearch").expect("parse");
        assert!(snap.success, "有 rows 仍算 success（数据本身有效）");
        assert!(!snap.is_healthy, "key 被禁用 → is_healthy=false（浮窗翻红）");
    }

    #[test]
    fn parse_missing_keys_array_is_parse_error() {
        let raw = json!({ "code": 0, "data": {} });
        let err = parse(&raw, "anysearch", "AnySearch").unwrap_err();
        assert_eq!(err.kind, super::super::ErrorKind::Parse);
    }

    #[test]
    fn parse_empty_keys_array_is_schema_unknown() {
        let raw = json!({ "code": 0, "data": { "keys": [] } });
        let err = parse(&raw, "anysearch", "AnySearch").unwrap_err();
        assert_eq!(err.kind, super::super::ErrorKind::SchemaUnknown);
    }

    #[test]
    fn parse_business_code_handled_in_do_fetch_not_parse() {
        // parse() 不拦 code（do_fetch 已拦）；但即便漏进来，code!=0 的响应
        // 通常也缺 data.keys → 走 parse 错误，不会塞脏 rows
        let raw = json!({ "code": 40141, "message": "token invalid" });
        let err = parse(&raw, "anysearch", "AnySearch").unwrap_err();
        assert_eq!(err.kind, super::super::ErrorKind::Parse);
    }

    #[test]
    fn parse_zero_rate_limit_still_shows_rate_row() {
        // rate_limit=0 也应显示（让用户知道"无限速 / 未设"），num_f64 返 Some(0.0)
        let raw = json!({
            "code": 0,
            "data": { "keys": [{
                "is_active": true,
                "quota_is_unlimited": true,
                "quota_limit": 0,
                "quota_used": 1,
                "rate_limit": 0
            }]}
        });
        let snap = parse(&raw, "anysearch", "AnySearch").expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[1].used, Some(0.0));
    }
}
