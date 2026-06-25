//! DeepSeek 钱包余额查询
//!
//! 端点：GET https://api.deepseek.com/user/balance
//! 鉴权：Authorization: Bearer <api_key>
//!
//! 响应 schema（参考 ccswitch services/balance.rs）：
//! ```json
//! {
//!   "is_available": true,
//!   "balance_infos": [
//!     {
//!       "currency": "CNY",
//!       "total_balance": "100.00",
//!       "granted_balance": "50.00",
//!       "topped_up_balance": "50.00"
//!     }
//!   ]
//! }
//! ```
//!
//! 关键事实：
//! - `total_balance` 可能是字符串或数字（兼容解析）
//! - 没有多区域（单域名 api.deepseek.com）
//! - 没有"已用"概念（钱包余额是 current_balance）
//! - `is_available=false` 通常意味着余额不足，UI 用红色提示

use std::borrow::Cow;
use std::pin::Pin;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError,
    ProviderSnapshot, QuotaRow, QuotaSource,
};

use crate::t;

const URL: &str = "https://api.deepseek.com/user/balance";

// ── QuotaSource 实现（Phase 1）────────────────────────────────────

pub struct DeepseekSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for DeepseekSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl DeepseekSource {
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

impl QuotaSource for DeepseekSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("deepseek")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "deepseek".to_string()
        } else {
            format!("deepseek#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Borrowed("DeepSeek")
        } else {
            Cow::Owned(format!(
                "DeepSeek{}",
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
        // deepseek 无 region / overrides 概念，忽略
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        let api_key = credentials.api_key.as_deref().unwrap_or("").trim().to_string();
        let unique_id = self.unique_id();
        let display_name = self.display_name().to_string();
        Box::pin(async move {
            if api_key.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "DeepSeek").into_owned(),
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
            t!("error.common.auth_failed", provider = "DeepSeek").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.forbidden", provider = "DeepSeek").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "DeepSeek").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "DeepSeek",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw: serde_json::Value = resp.json().await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    let is_available = raw
        .get("is_available")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let infos = raw.get("balance_infos").and_then(|v| v.as_array());

    let mut rows = Vec::new();
    if let Some(infos) = infos {
        for info in infos {
            let currency = info
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("CNY")
                .to_string();
            let total_balance = parse_f64(info, "total_balance")
                // 兜底：granted + topped_up
                .or_else(|| {
                    let g = parse_f64(info, "granted_balance").unwrap_or(0.0);
                    let t = parse_f64(info, "topped_up_balance").unwrap_or(0.0);
                    if g + t > 0.0 {
                        Some(g + t)
                    } else {
                        None
                    }
                });
            rows.push(QuotaRow {
                label: t!("row.balance").to_string(),
                utilization: None,
                remaining: total_balance,
                used: None,
                total: None,
                resets_at: None,
                unit: Some(currency),
                extra: None,
                kind: None,
            });
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "DeepSeek",
                field = "balance_infos"
            )
            .into_owned(),
        ));
    }

    // 不再 push "状态" 行 —— `is_available` 仍驱动 `is_healthy`（即右上角
    // dot 颜色），浮窗靠 dot 表达"可用/不可用"已经够清晰，多一行徽章
    // 跟 dot 完全重复（dot 变红 + 状态"余额不足" 是同一个信息）。

    Ok(ProviderSnapshot {
        provider: "deepseek".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(chrono::Utc::now().timestamp_millis()),
        next_fetch_at: None,
        raw: Some(raw),
        is_healthy: is_available,
        // source_id 用传入的 unique_id 而不是硬编码 "deepseek"：
        // instance_index=1 → "deepseek"，2 → "deepseek#2"。
        // 否则 deepseek#2.fetch() 清一色返回 source_id="deepseek" →
        // refresh_single_inner 按 source_id 做 in-memory snapshot 替换时
        // 会把 deepseek#2 的结果写入到 deepseek#1 的位置 → 浮窗出现
        // 两个 deepseek（一条真实 + 一条覆盖了内置）且余额显示原实例余额。
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name: None,
        transient: None,
    })
}

/// 兼容数字和字符串两种 JSON 表示
fn parse_f64(obj: &serde_json::Value, field: &str) -> Option<f64> {
    obj.get(field).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}
