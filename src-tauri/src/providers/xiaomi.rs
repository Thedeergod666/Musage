//! Xiaomi MiMo Token Plan 用量查询
//!
//! 端点（**猜测**：照 MiniMax `/v1/api/openplatform/coding_plan/remains` 模式）：
//! - CN: `https://token-plan-cn.xiaomimimo.com/v1/api/openplatform/coding_plan/remains`
//! - SGP: `https://token-plan-sgp.xiaomimimo.com/v1/api/openplatform/coding_plan/remains`
//! - AMS: `https://token-plan-ams.xiaomimimo.com/v1/api/openplatform/coding_plan/remains`
//!
//! **官方未文档化**，实际 endpoint 可能不同。如果字段名也对不上，
//! 走设置面板 · Schema overrides 加候选三元组（参考 ccswitch 的可配置 extractor 思路）。
//!
//! API key 格式：`tp-xxxxx`（Token Plan 专用，与 pay-as-you-go 的 `sk-` 区分）
//!
//! ## Schema 预期（基于小米后台 dashboard 显示的字段名）
//!
//! - 主额度：`used_credits` / `total_credits`（或 `remaining_credits` + `total_credits`）
//! - 补偿积分：`compensation_used_credits` / `compensation_total_credits`
//! - 套餐到期：`expires_at`（epoch ms）或 `expire_at` 或 `end_time`（distance seconds）
//!
//! UI 上每个用一行 `QuotaRow`：
//! - "月度" → 已用百分比 + 到期时间
//! - "补偿" → 已用百分比（无 reset）

use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ErrorKind, Provider, ProviderImpl, ProviderSnapshot, QuotaRow};
use crate::config::ProviderOverrides;

/// Xiaomi MiMo Token Plan base URL（不同 cluster 不同子域）
///
/// ⚠️ endpoint 是按 MiniMax 的 `/v1/api/openplatform/coding_plan/remains` 模式猜的。
/// 如果 Xiaomi 没暴露这个接口，会 404；届时用户得告诉我正确路径。
#[allow(dead_code)]
const URL_BASE_CN: &str = "https://token-plan-cn.xiaomimimo.com";
#[allow(dead_code)]
const URL_BASE_SGP: &str = "https://token-plan-sgp.xiaomimimo.com";
#[allow(dead_code)]
const URL_BASE_AMS: &str = "https://token-plan-ams.xiaomimimo.com";
#[allow(dead_code)]
const USAGE_PATH: &str = "/v1/api/openplatform/coding_plan/remains";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum XiaomiRegion {
    #[default]
    Cn,
    Sgp,
    Ams,
}

impl XiaomiRegion {
    pub fn api_url(&self) -> &'static str {
        match self {
            XiaomiRegion::Cn => "https://token-plan-cn.xiaomimimo.com/v1/api/openplatform/coding_plan/remains",
            XiaomiRegion::Sgp => "https://token-plan-sgp.xiaomimimo.com/v1/api/openplatform/coding_plan/remains",
            XiaomiRegion::Ams => "https://token-plan-ams.xiaomimimo.com/v1/api/openplatform/coding_plan/remains",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            XiaomiRegion::Cn => "🇨🇳 中国 (token-plan-cn)",
            XiaomiRegion::Sgp => "🌏 新加坡 (token-plan-sgp)",
            XiaomiRegion::Ams => "🌍 欧洲 (token-plan-ams)",
        }
    }
}

#[derive(Debug, Default)]
pub struct Xiaomimimo {
    pub region: XiaomiRegion,
}

impl Xiaomimimo {
    pub async fn do_fetch(
        api_key: &str,
        region: XiaomiRegion,
        overrides: &ProviderOverrides,
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
            .send()
            .await
            .map_err(|e| format!("网络错误 [{}]: {e}", region.api_url()))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("鉴权失败，请检查 Xiaomi MiMo API key（Token Plan 用 tp- 开头）".to_string());
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err("无权限访问 Xiaomi MiMo 用量接口（HTTP 403）".to_string());
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "Xiaomi MiMo 服务异常 (HTTP {status}): {}",
                body.chars().take(200).collect::<String>()
            ));
        }

        let raw: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("响应不是 JSON: {e}"))?;

        let snap = parse(&raw, region, overrides);
        Ok((raw, snap))
    }
}

impl ProviderImpl for Xiaomimimo {
    fn id(&self) -> Provider {
        Provider::Xiaomimimo
    }
    fn display_name(&self) -> &'static str {
        "Xiaomi MiMo"
    }

    fn fetch<'a>(
        &'a self,
        api_key: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, String>> + Send + 'a>> {
        let region = self.region;
        Box::pin(async move {
            let (_, snap) =
                Self::do_fetch(api_key, region, &ProviderOverrides::default()).await?;
            Ok(snap)
        })
    }
}

/// 解析：尝试 percent-based（优先）→ count-based → 失败
///
/// 跟 MiniMax 一样，用户可通过 `ProviderOverrides::monthly.count_candidates` 加新字段名
fn parse(
    raw: &serde_json::Value,
    _region: XiaomiRegion,
    overrides: &ProviderOverrides,
) -> ProviderSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 业务级错误（base_resp.status_code != 0）
    if let Some(base_resp) = raw.get("base_resp") {
        let code = base_resp
            .get("status_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if code != 0 {
            let msg = base_resp
                .get("status_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");
            return ProviderSnapshot {
                provider: Provider::Xiaomimimo,
                success: false,
                rows: vec![],
                error: Some(format!("Xiaomi MiMo API code {code}: {msg}")),
                error_kind: Some(ErrorKind::ServerError),
                fetched_at: Some(now_ms),
                raw: Some(raw.clone()),
                is_healthy: false,
            };
        }
    }

    // 通用解析：从 raw 里找 monthly_quota
    // 数据可能在 `data.monthly_quota` / `data` / 顶层，三个位置都试
    let data = raw.get("data").unwrap_or(raw);

    // ── 1. 主额度（percent-based 优先）
    let main = parse_quota_percent(data, "monthly_remaining_percent", "monthly_status")
        .or_else(|| {
            parse_quota_count(
                data,
                &[
                    // 内置默认候选
                    ("used_credits", "total_credits", "expires_at"),
                    ("monthly_used", "monthly_total", "monthly_end"),
                    ("plan_used", "plan_total", "plan_end"),
                    ("used", "total", "end"),
                ],
                &overrides.monthly.count_candidates,
            )
        });

    // ── 2. 补偿积分（可选，count-based）
    let comp = parse_quota_count(
        data,
        &[
            ("compensation_used_credits", "compensation_total_credits", ""),
            ("comp_used", "comp_total", ""),
        ],
        &[], // 补偿积分暂不开 overrides（用得少）
    );

    let mut rows = Vec::new();
    if let Some(q) = main {
        rows.push(QuotaRow {
            label: "月度".to_string(),
            utilization: Some(q.utilization),
            remaining: None,
            total: None,
            resets_at: q.resets_at,
            unit: Some("%".to_string()),
            extra: None,
        });
    }
    if let Some(q) = comp {
        rows.push(QuotaRow {
            label: "补偿".to_string(),
            utilization: Some(q.utilization),
            remaining: None,
            total: None,
            resets_at: None,
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    let success = !rows.is_empty();
    ProviderSnapshot {
        provider: Provider::Xiaomimimo,
        success,
        rows,
        error: if success {
            None
        } else {
            Some("未识别 schema，请把 raw 字段贴给开发者，或在设置面板添加候选字段名".to_string())
        },
        error_kind: if success { None } else { Some(ErrorKind::SchemaUnknown) },
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: success,
    }
}

#[derive(Debug, Clone)]
struct TierInternal {
    utilization: f64,
    resets_at: Option<i64>,
}

/// percent-based：`remaining_percent` ∈ [0, 100]，已用% = 100 - remain
fn parse_quota_percent(
    obj: &serde_json::Value,
    k_percent: &str,
    k_status: &str,
) -> Option<TierInternal> {
    let status = obj.get(k_status).and_then(|v| v.as_i64())?;
    if status != 1 {
        return None;
    }
    let remain_pct = obj.get(k_percent).and_then(num_to_f64)?;
    if !(0.0..=100.0).contains(&remain_pct) {
        return None;
    }
    let utilization = 100.0 - remain_pct;
    Some(TierInternal {
        utilization,
        resets_at: None, // percent 路径下 reset 不一定在同一字段，跳过
    })
}

/// count-based：total > 0 才算命中；先试用户 overrides，再试内置默认
fn parse_quota_count(
    obj: &serde_json::Value,
    candidates: &[(&str, &str, &str)],
    user_overrides: &[crate::config::FieldTriple],
) -> Option<TierInternal> {
    for triple in user_overrides {
        let total = obj.get(&triple.total).and_then(num_to_f64);
        let used = obj.get(&triple.remaining).and_then(num_to_f64);
        if let (Some(t), Some(u)) = (total, used) {
            if t > 0.0 {
                let utilization = (u / t) * 100.0;
                let resets_at = triple
                    .end
                    .as_deref()
                    .filter(|k| !k.is_empty())
                    .and_then(|k| {
                        obj.get(k)
                            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    })
                    .map(smart_reset_to_ms);
                return Some(TierInternal {
                    utilization,
                    resets_at,
                });
            }
        }
    }
    for (k_used, k_total, k_end) in candidates {
        if k_used.is_empty() || k_total.is_empty() {
            continue;
        }
        let total = obj.get(*k_total).and_then(num_to_f64);
        let used = obj.get(*k_used).and_then(num_to_f64);
        if let (Some(t), Some(u)) = (total, used) {
            if t > 0.0 {
                let utilization = (u / t) * 100.0;
                let resets_at = if k_end.is_empty() {
                    None
                } else {
                    obj.get(*k_end)
                        .and_then(|v| {
                            v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                        .map(smart_reset_to_ms)
                };
                return Some(TierInternal {
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

/// reset 字段智能转 epoch ms（跟 minimax.rs 一致）：
/// - > 10^12 当作 epoch ms
/// - 否则当 duration-seconds 加到当前时间
fn smart_reset_to_ms(raw: i64) -> i64 {
    const EPOCH_MS_MIN: i64 = 1_000_000_000_000;
    const EPOCH_MS_MAX: i64 = 4_102_444_800_000;
    if (EPOCH_MS_MIN..=EPOCH_MS_MAX).contains(&raw) {
        raw
    } else {
        chrono::Utc::now().timestamp_millis() + raw * 1000
    }
}