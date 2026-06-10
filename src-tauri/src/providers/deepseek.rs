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

use std::pin::Pin;
use std::time::Duration;

use serde_json::json;

use super::{Provider, ProviderImpl, ProviderSnapshot, QuotaRow};

const URL: &str = "https://api.deepseek.com/user/balance";

#[derive(Debug, Default)]
pub struct Deepseek;

impl ProviderImpl for Deepseek {
    fn id(&self) -> Provider {
        Provider::Deepseek
    }
    fn display_name(&self) -> &'static str {
        "DeepSeek"
    }

    fn fetch<'a>(
        &'a self,
        api_key: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, String>> + Send + 'a>> {
        Box::pin(async move { do_fetch(api_key).await })
    }
}

async fn do_fetch(api_key: &str) -> Result<ProviderSnapshot, String> {
    if api_key.trim().is_empty() {
        return Err("API key 为空".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(concat!("Musage/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("build client: {e}"))?;

    let resp = client
        .get(URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("DeepSeek 网络错误: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("鉴权失败，请检查 DeepSeek API key".to_string());
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err("无权限访问 DeepSeek 钱包（HTTP 403）".to_string());
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("DeepSeek 请求过于频繁，请稍后再试".to_string());
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "DeepSeek 服务异常 (HTTP {status}): {}",
            body.chars().take(200).collect::<String>()
        ));
    }

    let raw: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("响应不是 JSON: {e}"))?;

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
                    if g + t > 0.0 { Some(g + t) } else { None }
                });
            rows.push(QuotaRow {
                label: "余额".to_string(),
                utilization: None,
                remaining: total_balance,
                total: None,
                resets_at: None,
                unit: Some(currency),
                extra: None,
            });
        }
    }

    if rows.is_empty() {
        return Err("DeepSeek 响应缺少 balance_infos".to_string());
    }

    // 状态行
    rows.push(QuotaRow {
        label: "状态".to_string(),
        utilization: None,
        remaining: None,
        total: None,
        resets_at: None,
        unit: None,
        extra: Some(json!({
            "is_available": is_available,
            "display": if is_available { "可用" } else { "余额不足" },
        })),
    });

    Ok(ProviderSnapshot {
        provider: Provider::Deepseek,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(chrono::Utc::now().timestamp_millis()),
        raw: Some(raw),
        is_healthy: is_available,
    })
}

/// 兼容数字和字符串两种 JSON 表示
fn parse_f64(obj: &serde_json::Value, field: &str) -> Option<f64> {
    obj.get(field).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    })
}
