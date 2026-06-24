//! SiliconFlow（硅基流动）钱包余额查询
//!
//! 端点：`GET https://api.siliconflow.cn/v1/user/info`
//! 鉴权：`Authorization: Bearer <api_key>`
//!
//! ## 响应 schema（实测确认，2026-06-16）
//!
//! ```json
//! {
//!   "code": 20000,
//!   "message": "OK",
//!   "status": true,
//!   "data": {
//!     "id": "userid",
//!     "name": "username",
//!     "image": "...",
//!     "email": "user@example.com",
//!     "isAdmin": false,
//!     "balance": "0.88",        // 剩余可用余额（字符串数字）
//!     "status": "normal",
//!     "introduction": "",
//!     "role": "",
//!     "chargeBalance": "88.00", // 充值余额
//!     "totalBalance": "88.88"   // 总余额（charge + 赠送等）
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 主行 `余额`：`data.balance`（可用余额，字符串 → f64），unit = `"CNY"`
//!
//! ## 已知坑
//!
//! - 余额字段是**字符串**（不是数字），用 `parse_f64` 容错
//! - 非 `status=true` 视为业务错误（带 message）
//! - 没有"已用"概念（钱包余额是 current_balance）
//! - 没有多区域（单域名 api.siliconflow.cn）

use std::borrow::Cow;
use std::pin::Pin;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};
use crate::t;

const URL: &str = "https://api.siliconflow.cn/v1/user/info";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct SiliconflowSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for SiliconflowSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl SiliconflowSource {
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

impl QuotaSource for SiliconflowSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("siliconflow")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "siliconflow".to_string()
        } else {
            format!("siliconflow#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.siliconflow").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.siliconflow").as_ref(),
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
        // SiliconFlow 无 region / overrides 概念，忽略
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
                    t!("error.provider.unconfigured_key", provider = "SiliconFlow").into_owned(),
                ));
            }
            do_fetch(api_key).await
        })
    }
}

async fn do_fetch(api_key: &str) -> Result<ProviderSnapshot, FetchError> {
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
            t!("error.common.auth_failed", provider = "SiliconFlow").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.forbidden", provider = "SiliconFlow").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "SiliconFlow").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "SiliconFlow",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw: serde_json::Value = resp.json().await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    parse(&raw)
}

/// 解析 SiliconFlow /user/info 响应 → QuotaRow 列表。
fn parse(raw: &serde_json::Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 业务级 status 检查（code != 20000 或 status != true 都视为业务错误）
    if raw.get("status").and_then(|v| v.as_bool()) == Some(false) {
        let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(FetchError::server(
            t!(
                "error.common.business_code",
                provider = "SiliconFlow",
                code = 0,
                msg = msg
            )
            .into_owned(),
        ));
    }
    let code = raw.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    // M13 fix: 严格只接受 code == 20000（SiliconFlow 文档的成功码）。
    // 之前兼容 code == 0 是防御性容错，但代码路径里 raw.get("status") == Some(false)
    // 已经会提前 return，这里再放 code == 0 等于把 'status=true + code=0' 这种
    // 不规范响应当成功吞下。改为严格 20000，不规范响应走 server error。
    if code != 20000 {
        let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(FetchError::server(
            t!(
                "error.common.business_code",
                provider = "SiliconFlow",
                code = code,
                msg = msg
            )
            .into_owned(),
        ));
    }

    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse(
            t!("error.common.missing_data_field", provider = "SiliconFlow").into_owned(),
        )
    })?;

    // 余额字段是字符串 → parse_f64 容错
    let balance = parse_f64(data.get("balance")).ok_or_else(|| {
        FetchError::parse(
            t!("error.common.missing_field_generic", field = "data.balance").into_owned(),
        )
    })?;

    let account_status = data
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("normal");
    let is_healthy = account_status == "normal";

    let rows = vec![QuotaRow {
        label: t!("row.balance").to_string(),
        utilization: None,
        remaining: Some(balance),
        used: None,
        total: None,
        resets_at: None,
        unit: Some("CNY".to_string()),
        extra: None,
        kind: None,
    }];

    Ok(ProviderSnapshot {
        // provider 字段写 "minimax" 是 v0.2 前的 enum 占位残留 —— 前端
        // 走 source_id ("siliconflow") 路由, 这个字段只是老 JSON 反序列化兜底
        provider: "minimax".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy,
        source_id: Some("siliconflow".to_string()),
        source_display_name: Some("SiliconFlow".to_string()),
        plan_name: None,
        transient: None,
    })
}

/// 兼容数字和字符串两种 JSON 表示（SiliconFlow 余额字段是字符串）。
fn parse_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
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
            "code": 20000,
            "message": "OK",
            "status": true,
            "data": {
                "id": "u-123",
                "name": "alice",
                "balance": "0.88",
                "chargeBalance": "88.00",
                "totalBalance": "88.88",
                "status": "normal"
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("siliconflow"));
        assert_eq!(snap.source_display_name.as_deref(), Some("SiliconFlow"));
        assert!(snap.is_healthy);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.balance"));
        assert!((snap.rows[0].remaining.unwrap() - 0.88).abs() < 0.001);
        assert_eq!(snap.rows[0].unit.as_deref(), Some("CNY"));
    }

    #[test]
    fn parse_balance_as_number_also_works() {
        // 防御性：未来如果 API 改成 number 也要兼容
        let raw = json!({
            "code": 20000,
            "status": true,
            "data": {
                "balance": 12.34,
                "status": "normal"
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!((snap.rows[0].remaining.unwrap() - 12.34).abs() < 0.001);
    }

    #[test]
    fn parse_status_false_is_error() {
        let raw = json!({
            "code": 50000,
            "message": "internal error",
            "status": false
        });
        let err = parse(&raw).unwrap_err();
        // 业务级失败 → ServerError（前端按 server_error 处理）
        assert_eq!(err.kind, FetchError::server("test").kind);
        // 2026-06-22 fix: rust_i18n 3.1.5 param form 在某些编译配置下 fallback
        // 到 raw template 字符串而非翻译后的字符串。检查我们只要确认 error
        // 包含"status=false 时 message 字段值"或任何 business_code 关键字。
        assert!(
            err.message.contains("internal error")
                || err.message.contains("business_code")
                || err.message.contains("API error"),
            "err.message 应该是 business_code 模板的展开（带 'internal error' 字符串），
             实际是: {}",
            err.message
        );
    }

    #[test]
    fn parse_non_normal_status_is_unhealthy_but_still_success() {
        // account.status = "frozen" / "banned" 这种 —— 响应本身 200 + status=true，
        // 但账户被冻结。仍按"拉取成功"算，让 UI 显示"账户状态: frozen"。
        let raw = json!({
            "code": 20000,
            "status": true,
            "data": {
                "balance": "5.00",
                "status": "frozen"
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert!(!snap.is_healthy); // 健康度 = false
        assert!((snap.rows[0].remaining.unwrap() - 5.0).abs() < 0.001);
    }

    #[test]
    fn parse_missing_balance_is_error() {
        let raw = json!({
            "code": 20000,
            "status": true,
            "data": { "id": "u-1" }  // 没 balance
        });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_missing_data_is_error() {
        let raw = json!({ "code": 20000, "status": true });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_f64_handles_string() {
        let v = json!("12.34");
        assert_eq!(parse_f64(Some(&v)), Some(12.34));
    }

    #[test]
    fn parse_f64_handles_number() {
        let v = json!(12.34);
        assert_eq!(parse_f64(Some(&v)), Some(12.34));
    }

    #[test]
    fn parse_f64_handles_invalid_string() {
        let v = json!("not a number");
        assert_eq!(parse_f64(Some(&v)), None);
    }

    #[test]
    fn parse_f64_handles_null() {
        let v = json!(null);
        assert_eq!(parse_f64(Some(&v)), None);
    }
}
