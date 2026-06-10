//! MiniMax Token Plan 用量查询
//!
//! 端点：GET /v1/api/openplatform/coding_plan/remains
//! 鉴权：Bearer <api_key>
//!
//! ## Schema 历史
//!
//! **2026-06-01 之前（count-based 旧 schema）**：
//! - `current_interval_total_count` / `current_interval_usage_count`（5h）
//! - `current_weekly_total_count` / `current_weekly_usage_count`（周）
//! - 字段名虽带 "usage" 但**实际是"剩余"**；已用% = (total - remaining) / total * 100
//!
//! **2026-06-01 之后（percent-based 新 schema）** — 参考 ccswitch PR #3518：
//! - `current_interval_remaining_percent`（5h 剩余%, 0-100）
//! - `current_interval_status`（5h 状态门控，== 1 才有效）
//! - `end_time`（5h 距离重置的**秒数**，不是 epoch ms）
//! - `current_weekly_remaining_percent`（周剩余%）
//! - `current_weekly_status`（周状态，== 1 才有效；2/3 = 不在套餐内）
//! - `weekly_end_time`（周距离重置的秒数）
//! - 已用% = 100 - `*_remaining_percent`
//! - **重要**：Plus 订阅者的 `*_total_count` 旧字段全为 0，count 路径会得到空快照
//! - **重要**：`*_remaining_percent=100` 不代表"还有 100%"，可能是 status=2/3（不在套餐内）
//!
//! ## 解析策略
//!
//! 1. 从 `model_remains[]` 优先选 `model_name == "general"`，找不到则取第一条
//! 2. 先尝试 percent-based 路径（新 schema，5h/周独立 gate）
//! 3. 失败则回退到 count-based 路径（旧 schema）
//! 4. reset 字段智能识别：> 10^12 当 epoch ms，否则当 duration-seconds 加到 now

use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Provider, ProviderImpl, ProviderSnapshot, QuotaRow};

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

#[derive(Debug, Default)]
pub struct Minimax {
    /// 默认 CN；commands 层在调 fetch 前先从 config 复制一份给 Minimax
    pub region: Region,
}

impl Minimax {
    /// 真正的拉取实现（接受 region 参数）
    pub async fn do_fetch(
        api_key: &str,
        region: Region,
    ) -> Result<(serde_json::Value, ProviderSnapshot), String> {
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
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("鉴权失败，请检查 MiniMax API key".to_string());
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err("无权限访问 MiniMax 用量接口（HTTP 403）".to_string());
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "MiniMax 服务异常 (HTTP {status}): {}",
                body.chars().take(200).collect::<String>()
            ));
        }

        let raw: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("响应不是 JSON: {e}"))?;

        let snap = parse(&raw, region);
        Ok((raw, snap))
    }
}

impl ProviderImpl for Minimax {
    fn id(&self) -> Provider {
        Provider::Minimax
    }
    fn display_name(&self) -> &'static str {
        "MiniMax"
    }

    fn fetch<'a>(
        &'a self,
        api_key: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, String>> + Send + 'a>> {
        let region = self.region;
        Box::pin(async move {
            let (_, snap) = Self::do_fetch(api_key, region).await?;
            Ok(snap)
        })
    }
}

/// 灵活解析：兼容 6/1 前后的 schema
#[allow(dead_code)] // region 参数为未来多区域 provider 准备
fn parse(raw: &serde_json::Value, _region: Region) -> ProviderSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 业务级错误（ccswitch 也这么做）
    if let Some(base_resp) = raw.get("base_resp") {
        let code = base_resp.get("status_code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = base_resp
                .get("status_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");
            return ProviderSnapshot {
                provider: Provider::Minimax,
                success: false,
                rows: vec![],
                error: Some(format!("MiniMax API code {code}: {msg}")),
                fetched_at: Some(now_ms),
                raw: Some(raw.clone()),
                is_healthy: false,
            };
        }
    }

    // 选 model_remains[] 中 model_name == "general" 的那条（ccswitch 3.16.2 行为），
    // 找不到则取第一条
    let item = raw
        .get("model_remains")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|v| {
                    v.get("model_name")
                        .and_then(|n| n.as_str())
                        .map(|s| s == "general")
                        .unwrap_or(false)
                })
                .or_else(|| arr.first())
        });

    let Some(item) = item else {
        return ProviderSnapshot {
            provider: Provider::Minimax,
            success: false,
            rows: vec![],
            error: Some("响应缺少 model_remains[0]".to_string()),
            fetched_at: Some(now_ms),
            raw: Some(raw.clone()),
            is_healthy: false,
        };
    };

    // 先试新 schema（percent-based），任一 tier 解不出时回退到旧 schema（count-based）
    let five_hour = parse_tier_percent(item, "current_interval_remaining_percent",
                                          "current_interval_status", "end_time")
        .or_else(|| parse_tier_count(item, &[
            // 旧 schema（ccswitch 老版本用的）
            ("current_interval_total_count", "current_interval_usage_count", "end_time"),
            // 候选新 schema（如果 6/1 改名了）
            ("interval_total", "interval_remaining", "interval_end"),
            ("window_total", "window_remaining", "window_end"),
            ("total_5h", "used_5h", "reset_5h"),
        ]));

    let weekly = parse_tier_percent(item, "current_weekly_remaining_percent",
                                       "current_weekly_status", "weekly_end_time")
        .or_else(|| parse_tier_count(item, &[
            ("current_weekly_total_count", "current_weekly_usage_count", "weekly_end_time"),
            ("weekly_total", "weekly_remaining", "weekly_end"),
            ("total_week", "used_week", "reset_week"),
        ]));

    // 转成 QuotaRow
    let mut rows = Vec::new();
    if let Some(t) = five_hour {
        rows.push(QuotaRow {
            label: "5h".to_string(),
            utilization: Some(t.utilization),
            remaining: None,
            total: None,
            resets_at: t.resets_at,
            unit: Some("%".to_string()),
            extra: None,
        });
    }
    if let Some(t) = weekly {
        rows.push(QuotaRow {
            label: "周".to_string(),
            utilization: Some(t.utilization),
            remaining: None,
            total: None,
            resets_at: t.resets_at,
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    let success = !rows.is_empty();
    let is_healthy = success; // MiniMax 拉到数据就认为可用

    ProviderSnapshot {
        provider: Provider::Minimax,
        success,
        rows,
        error: if success {
            None
        } else {
            Some("未识别 schema，请把 raw 字段贴给开发者".to_string())
        },
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy,
    }
}

#[derive(Debug, Clone)]
struct TierInternal {
    utilization: f64,
    resets_at: Option<i64>,
}

/// 新 schema 解析（ccswitch 3.16.2+ / MiniMax 2026-06-01 之后）
///
/// - `k_percent`：剩余百分比字段（0-100）
/// - `k_status`：状态门控字段（必须 == 1 才算数；2/3 = 不在套餐内）
/// - `k_reset`：距离重置的**秒数**（不是 epoch ms）
///
/// 返回已用百分比 = 100 - remain%。如 status != 1 或字段缺失返回 None。
fn parse_tier_percent(
    item: &serde_json::Value,
    k_percent: &str,
    k_status: &str,
    k_reset: &str,
) -> Option<TierInternal> {
    // 1. status 必须 == 1
    let status = item.get(k_status).and_then(|v| v.as_i64())?;
    if status != 1 {
        return None;
    }
    // 2. 读取剩余百分比
    let remain_pct = item.get(k_percent).and_then(num_to_f64)?;
    if !(0.0..=100.0).contains(&remain_pct) {
        return None;
    }
    let utilization = 100.0 - remain_pct;
    // 3. reset：智能识别（duration-seconds vs epoch-ms）
    let resets_at = item.get(k_reset).and_then(|v| v.as_i64()).map(smart_reset_to_ms);
    Some(TierInternal { utilization, resets_at })
}

/// 旧 schema 解析（count-based，已知 2026-06-01 后对 Plus 订阅者已不可靠）
///
/// 字段名带 "usage" 实际语义是"剩余"。已用% = (total - remaining) / total * 100。
fn parse_tier_count(
    item: &serde_json::Value,
    candidates: &[(&str, &str, &str)],
) -> Option<TierInternal> {
    for (k_total, k_remain, k_reset) in candidates {
        let total = item.get(*k_total).and_then(num_to_f64);
        let remain = item.get(*k_remain).and_then(num_to_f64);
        if let (Some(t), Some(r)) = (total, remain) {
            if t > 0.0 {
                let utilization = ((t - r) / t) * 100.0;
                let resets_at = item.get(*k_reset)
                    .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .map(smart_reset_to_ms);
                return Some(TierInternal {
                    utilization,
                    resets_at,
                });
            }
        }
    }
    None
}

/// 把 reset 字段智能转成 epoch ms。
///
/// - 旧 schema：`end_time` 是 epoch ms（> 10^12）
/// - 新 schema：`end_time` 是距离重置的秒数（< 10^10，绝大多数情况）
/// - 边界 10^12：2001-09-09 之后才合法为 epoch ms
/// - 边界 4*10^12：2100-01-01 上限（防止异常值）
fn smart_reset_to_ms(raw: i64) -> i64 {
    const EPOCH_MS_MIN: i64 = 1_000_000_000_000; // 2001-09-09
    const EPOCH_MS_MAX: i64 = 4_102_444_800_000; // 2100-01-01
    if (EPOCH_MS_MIN..=EPOCH_MS_MAX).contains(&raw) {
        raw
    } else {
        // 当作 duration-seconds，加到当前时间
        chrono::Utc::now().timestamp_millis() + raw * 1000
    }
}

fn num_to_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}
