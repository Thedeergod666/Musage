//! MiniMax Token Plan 用量查询
//!
//! 端点：GET /v1/api/openplatform/coding_plan/remains
//! 鉴权：Bearer <api_key>
//!
//! 响应 schema 2026-06-01 更新前/后可能不同，本模块用**字段名宽容解析**：
//! - 先尝试旧字段（ccswitch 用的）
//! - 找不到时打印原始 JSON 让人能定位新字段
//!
//! 关键事实（从 ccswitch coding_plan.rs 推得）：
//! - `current_interval_usage_count` 字段名虽然带 "usage"，**实际语义是"剩余"**（满=total，用完=0）
//! - 已用百分比 = ((total - remaining) / total) * 100

use serde::{Deserialize, Serialize};
use std::time::Duration;

const URL_CN: &str = "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains";
const URL_EN: &str = "https://api.minimax.io/v1/api/openplatform/coding_plan/remains";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Region {
    #[default]
    Cn,
    En,
}

impl Region {
    pub fn api_url(&self) -> &'static str {
        match self {
            Region::Cn => URL_CN,
            Region::En => URL_EN,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Region::Cn => "国内 (api.minimaxi.com)",
            Region::En => "国际 (api.minimax.io)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaTier {
    pub name: String, // "five_hour" / "weekly_limit"
    pub utilization: f64, // 百分比，0-100+（可超 100）
    pub resets_at: Option<i64>, // 毫秒时间戳
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaSnapshot {
    pub success: bool,
    pub five_hour: Option<QuotaTier>,
    pub weekly: Option<QuotaTier>,
    pub raw: Option<serde_json::Value>, // 原始响应，便于排查
    pub error: Option<String>,
    pub fetched_at: Option<i64>,
    pub region: Region,
}

impl QuotaSnapshot {
    pub fn to_health_label(&self) -> &'static str {
        if !self.success {
            return "err";
        }
        let u = self.five_hour.as_ref().map(|t| t.utilization).unwrap_or(0.0);
        if u < 70.0 {
            "ok"
        } else if u < 90.0 {
            "warn"
        } else {
            "alert"
        }
    }
}

/// 拉取 + 解析。返回 (raw, snapshot)。
pub async fn fetch_quota(api_key: &str, region: Region) -> Result<(serde_json::Value, QuotaSnapshot), String> {
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
        .get(region.api_url())
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("网络错误 [{}]: {e}", region.api_url()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!("鉴权失败 (HTTP {status})，请检查 API key"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()));
    }

    let raw: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("响应不是 JSON: {e}"))?;

    let snap = parse(&raw, region);
    Ok((raw, snap))
}

/// 灵活解析：兼容 6/1 前后的 schema
fn parse(raw: &serde_json::Value, region: Region) -> QuotaSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 业务级错误（ccswitch 也这么做）
    if let Some(base_resp) = raw.get("base_resp") {
        let code = base_resp.get("status_code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = base_resp
                .get("status_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");
            return QuotaSnapshot {
                success: false,
                error: Some(format!("API code {code}: {msg}")),
                fetched_at: Some(now_ms),
                region,
                raw: Some(raw.clone()),
                ..Default::default()
            };
        }
    }

    // 取第一个 model 记录
    let item = raw
        .get("model_remains")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first());

    let Some(item) = item else {
        return QuotaSnapshot {
            success: false,
            error: Some("响应缺少 model_remains[0]".to_string()),
            fetched_at: Some(now_ms),
            region,
            raw: Some(raw.clone()),
            ..Default::default()
        };
    };

    let five_hour = parse_tier(
        item,
        &[
            // 旧 schema（ccswitch 用的）
            ("current_interval_total_count", "current_interval_usage_count", "end_time"),
            // 候选新 schema（如果 6/1 改名了）
            ("interval_total", "interval_remaining", "interval_end"),
            ("window_total", "window_remaining", "window_end"),
            ("total_5h", "used_5h", "reset_5h"),
        ],
        "five_hour",
    );

    let weekly = parse_tier(
        item,
        &[
            ("current_weekly_total_count", "current_weekly_usage_count", "weekly_end_time"),
            ("weekly_total", "weekly_remaining", "weekly_end"),
            ("total_week", "used_week", "reset_week"),
        ],
        "weekly_limit",
    );

    // 至少要有一个 tier 解析成功才算 success
    let success = five_hour.is_some() || weekly.is_some();

    QuotaSnapshot {
        success,
        five_hour,
        weekly,
        raw: Some(raw.clone()),
        error: if success { None } else { Some("未识别 schema，请把 raw 字段贴给开发者".to_string()) },
        fetched_at: Some(now_ms),
        region,
    }
}

/// 尝试多组字段名，返回首个能解析的 QuotaTier
fn parse_tier(item: &serde_json::Value, candidates: &[(&str, &str, &str)], name: &str) -> Option<QuotaTier> {
    for (k_total, k_remain, k_reset) in candidates {
        let total = item.get(*k_total).and_then(num_to_f64);
        let remain = item.get(*k_remain).and_then(num_to_f64);
        if let (Some(t), Some(r)) = (total, remain) {
            if t > 0.0 {
                let utilization = ((t - r) / t) * 100.0;
                let resets_at = item.get(*k_reset).and_then(|v| {
                    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                });
                return Some(QuotaTier {
                    name: name.to_string(),
                    utilization,
                    resets_at,
                });
            }
        }
    }
    None
}

fn num_to_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}
