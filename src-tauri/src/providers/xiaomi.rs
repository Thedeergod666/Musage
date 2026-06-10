//! Xiaomi MiMo Token Plan 用量查询
//!
//! ⚠️ **小米官方没公开 usage query endpoint** —— dashboard 里的"当前套餐用量"
//! 数据来自某个内部 API，没在 docs 里。文档只列了**推理 endpoint**：
//! 数据来自某个内部 API，没在 docs 里。文档只列了**推理 endpoint**：
//! - Anthropic 兼容: `POST {BASE_URL}/v1/messages`，header `api-key: tp-xxxxx`
//! - OpenAI 兼容:   `POST {BASE_URL}/chat/completions`，header `api-key: tp-xxxxx`
//!
//! ccswitch 也没把 Xiaomi Token Plan 用量监控做出来（只把 Xiaomi 作为
//! pay-as-you-go 模型 provider）。
//!
//! ## 已知信息
//! - 集群：`token-plan-cn` / `token-plan-sgp` / `token-plan-ams`（用户截图用 cn）
//! - API key 格式：`tp-xxxxx`（Token Plan 专用，区别于 `sk-` pay-as-you-go）
//! - Auth header：**`api-key: tp-xxxxx`**（不是 `Authorization: Bearer`）
//! - 响应里有 `meta.usage.{credits_used, usd_spent}`（per-request，不是剩余额度）
//!
//! ## 做法：探测候选路径
//! 用户在浏览器 dashboard (platform.xiaomimimo.com) 上打开 DevTools → Network，
//! 找到那个拉"套餐用量"的请求，告诉我 URL，我改常量。
//!
//! 当前实现的 endpoint 都是**猜的**，按响应特征做宽容解析。如果用户配了 overrides
//! schema 字段名，照样能命中（`schema_overrides.xiaomimimo.monthly`）。

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
    /// 只返回 base URL（不含 path），具体 path 在 do_fetch 里逐个试
    pub fn host_base(&self) -> &'static str {
        match self {
            XiaomiRegion::Cn => "https://token-plan-cn.xiaomimimo.com",
            XiaomiRegion::Sgp => "https://token-plan-sgp.xiaomimimo.com",
            XiaomiRegion::Ams => "https://token-plan-ams.xiaomimimo.com",
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

        // ⚠️ Xiaomi 用的是 `api-key: tp-xxxxx`，不是 `Authorization: Bearer`
        //    （参考官方 curl 样例；MiniMax 才用 Bearer）
        // ⚠️ usage query endpoint 没文档公开 —— 当前 base url 是按"OpenAI 风格管理 API"
        //    的常见模式猜的几个候选，逐个试；第一个 200 的就拿来 parse。
        let host_base = match region {
            XiaomiRegion::Cn => "https://token-plan-cn.xiaomimimo.com",
            XiaomiRegion::Sgp => "https://token-plan-sgp.xiaomimimo.com",
            XiaomiRegion::Ams => "https://token-plan-ams.xiaomimimo.com",
        };
        let candidates: &[&str] = &[
            "/v1/usage",
            "/v1/quota",
            "/v1/account/usage",
            "/v1/dashboard/billing/credit_grants",  // 仿 OpenAI 的
            "/v1/subscription",
            "/v1/api/openplatform/coding_plan/remains", // 仿 MiniMax
        ];

        let mut last_err = String::new();
        for path in candidates {
            let url = format!("{host_base}{path}");
            let resp = match client
                .get(&url)
                .header("api-key", api_key)
                .header("Accept", "application/json")
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("{}: {}", url, e);
                    continue;
                }
            };
            let status = resp.status();
            // 404 / 401 / 405 / 501 都不是我们要的；继续试下一个
            if !status.is_success() {
                last_err = format!("{} → HTTP {}", url, status);
                continue;
            }
            // 成功
            let raw: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("{} → 响应不是 JSON: {}", url, e))?;
            let snap = parse(&raw, region, overrides);
            return Ok((raw, snap));
        }

        // 全部候选都失败
        Err(format!(
            "Xiaomi MiMo usage endpoint 未公开 —— {} 个候选路径全部失败（最后一个错: {}）。\
             请打开 platform.xiaomimimo.com dashboard → DevTools Network → 找那个拉'套餐用量'的请求，\
             把 URL 贴给我。",
            candidates.len(),
            last_err
        ))
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