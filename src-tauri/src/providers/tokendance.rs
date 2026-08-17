//! TokenDance (词元跳动) 钱包余额查询
//!
//! 端点：`GET https://tokendance.space/portal/api/v1/user/balance`
//! 鉴权：`Authorization: Bearer <api_key>` —— 跟模型调用同一个 API Key，
//!        **不需要**额外 IAM / AK-SK 对（跟火山方舟反着来）。
//!
//! ## 响应 schema（2026-08-14 实测确认）
//!
//! ```json
//! {
//!   "balance": {
//!     "credits": 58000000,      // 总积分（充值获得）
//!     "credits_used": 57837189, // 已消耗
//!     "balance": 162811         // 剩余可用 = credits - credits_used
//!   }
//! }
//! ```
//!
//! ## 错误格式
//!
//! ```json
//! { "error": { "code": "unauthorized", "message": "API 密钥不存在" } }
//! ```
//!
//! 401 unauthorized / 403 forbidden / 500 internal_error（跟 fetch 层 HTTP 状态码分流一致）
//!
//! ## 关键事实
//!
//! - **无**业务码字段（无 `code` / `status`），HTTP 401/403/5xx 在 fetch 层已兜底
//! - **无** `account.status` 字段 —— `is_healthy` 走 "HTTP 2xx 即健康" 一刀切
//! - **无** 5h/weekly/月窗口 —— 纯 cash 余额（积分），跟 deepseek / siliconflow 同款
//! - `unit = None`：credit 跟模型调用 token 比例不透明，避免误标币种；
//!   前端 [`format_balance_tray`] 支持空 unit，显示成 `162k` 之类短数字
//! - 没有多区域（单域名 tokendance.space）

use std::borrow::Cow;
use std::pin::Pin;

use super::{
    json_body_limited, shared_client, text_body_limited, AuthKind, Credentials, ErrorKind,
    FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};
use crate::t;

const URL: &str = "https://tokendance.space/portal/api/v1/user/balance";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct TokendanceSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for TokendanceSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl TokendanceSource {
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

impl QuotaSource for TokendanceSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("tokendance")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "tokendance".to_string()
        } else {
            format!("tokendance#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.tokendance").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.tokendance").as_ref(),
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
        // TokenDance 无 region / overrides 概念，忽略
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        let api_key = credentials
            .api_key
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let unique_id = self.unique_id();
        let display_name = self.display_name().to_string();
        Box::pin(async move {
            if api_key.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "TokenDance").into_owned(),
                ));
            }
            do_fetch(&api_key, &unique_id, &display_name).await
        })
    }
}

async fn do_fetch(
    api_key: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    if api_key.trim().is_empty() {
        return Err(FetchError::unconfigured(
            t!("error.common.api_key_empty").into_owned(),
        ));
    }

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
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "TokenDance").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.forbidden", provider = "TokenDance").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "TokenDance").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = text_body_limited(resp).await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "TokenDance",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw = json_body_limited(resp).await?;

    parse(&raw, source_id, display_name)
}

/// 解析 TokenDance /portal/api/v1/user/balance 响应 → QuotaRow 列表。
///
/// TokenDance **不**返业务码字段（无 `code` / `status` / `account.status`），
/// HTTP 401/403 已由 fetch 层分流，parse 只做"字段缺失"错误检查。
///
/// `is_healthy` 一刀切 `true` —— 余额极低靠 [`crate::providers::ProviderSnapshot::health_label`]
/// 的 `wallet_alert_threshold` 触发 alert（mod.rs 通用逻辑，余额为 0 仍视为健康，
/// 因为可能是用户刚充完还没到账）。
fn parse(
    raw: &serde_json::Value,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let balance_obj = raw
        .get("balance")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            FetchError::parse(
                t!(
                    "error.common.missing_field",
                    provider = "TokenDance",
                    field = "balance"
                )
                .into_owned(),
            )
        })?;

    let balance = super::parse::num_f64(
        balance_obj
            .get("balance")
            .unwrap_or(&serde_json::Value::Null),
    )
    .ok_or_else(|| {
        FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "TokenDance",
                field = "balance.balance"
            )
            .into_owned(),
        )
    })?;

    let rows = vec![QuotaRow {
        label: t!("row.balance").to_string(),
        utilization: None,
        remaining: Some(balance),
        used: None,
        total: None,
        resets_at: None,
        // unit 留 None —— credit 不是货币单位，避免误显示 ¥ / $。
        // 前端 format_balance_tray(v, "") 走无前缀数字格式（"162k"）。
        unit: None,
        extra: None,
        kind: None,
    }];

    Ok(ProviderSnapshot {
        // v0.3+ 用 source_id ("tokendance") 替代旧 enum 占位
        provider: "tokendance".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        // 一刀切 healthy：余额极低由 wallet_alert_threshold 通用逻辑触发 alert
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name: None,
        transient: None,
    })
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_full_response() {
        // 实测 schema (2026-08-14): 顶层 balance 对象 + 三个 int 字段
        let raw = json!({
            "balance": {
                "credits": 58000000_i64,
                "credits_used": 57837189_i64,
                "balance": 162811_i64
            }
        });
        let snap = parse(&raw, "tokendance", "TokenDance").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("tokendance"));
        assert_eq!(snap.source_display_name.as_deref(), Some("TokenDance"));
        assert!(snap.is_healthy);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.balance"));
        assert_eq!(snap.rows[0].remaining, Some(162811.0));
        // credit 不是货币 —— unit 必须 None，避免前端误加 ¥ / $ 前缀
        assert_eq!(snap.rows[0].unit, None);
    }

    #[test]
    fn parse_balance_only() {
        // credits_used 缺失也能跑 —— 我们只读 balance.balance 一个字段。
        // 实测中 TokenDance 通常三个字段都在，但 schema 漂移防御性必须有。
        let raw = json!({
            "balance": { "balance": 50.5 }
        });
        let snap = parse(&raw, "tokendance", "TokenDance").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].remaining, Some(50.5));
        assert_eq!(snap.rows[0].unit, None);
    }

    #[test]
    fn parse_balance_as_string() {
        // 防御性：未来 schema 把 balance 改成字符串数字也兼容
        let raw = json!({
            "balance": { "balance": "12.34" }
        });
        let snap = parse(&raw, "tokendance", "TokenDance").expect("parse");
        assert!((snap.rows[0].remaining.unwrap() - 12.34).abs() < 0.001);
    }

    #[test]
    fn parse_zero_balance() {
        // 余额 0 仍 is_healthy=true —— 用户可能刚充完还没到账,
        // 极低余额靠 wallet_alert_threshold 通用逻辑触发 alert,不在这判
        let raw = json!({
            "balance": { "credits": 100, "credits_used": 100, "balance": 0 }
        });
        let snap = parse(&raw, "tokendance", "TokenDance").expect("parse");
        assert!(snap.is_healthy);
        assert_eq!(snap.rows[0].remaining, Some(0.0));
    }

    #[test]
    fn parse_missing_balance_obj() {
        // 顶层无 balance 对象 → Parse error
        let raw = json!({
            "data": { "credit": 100 }
        });
        let err = parse(&raw, "tokendance", "TokenDance").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
        assert!(
            err.message.contains("balance") || err.message.contains("字段"),
            "err.message 应提到 'balance' 字段: {}",
            err.message
        );
    }

    #[test]
    fn parse_missing_inner_balance() {
        // balance 对象存在但里面无 balance 字段 → Parse error
        let raw = json!({
            "balance": { "credits": 100, "credits_used": 50 }
        });
        let err = parse(&raw, "tokendance", "TokenDance").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
        assert!(
            err.message.contains("balance") || err.message.contains("字段"),
            "err.message 应提到 'balance.balance' 字段: {}",
            err.message
        );
    }

    #[test]
    fn parse_f64_handles_null() {
        // num_f64 对 null 返 None —— 验证共享 helper 行为正确,
        // 防止 num_f64 后续修改时这里静默 break
        let v = serde_json::Value::Null;
        assert_eq!(super::super::parse::num_f64(&v), None);
    }
}
