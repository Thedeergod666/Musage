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

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};

use crate::config::ProviderOverrides;
use crate::t;

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

// ── QuotaSource 实现（Phase 1）────────────────────────────────────

/// 每次 fetch 前可更新的运行时状态（region + 用户 overrides）。
#[derive(Debug, Clone, Default)]
pub struct MinimaxState {
    pub region: Region,
    pub overrides: ProviderOverrides,
}

/// 新的 trait 实现。commands.rs 走这条路径。
///
/// ## PR 1a · instance_index
///
/// 决策 1（id vs unique_id 分离）+ 决策 3（display_name 渲染时拼）：
/// - `id()` 永远返 base `"minimax"`（走 `Cow::Borrowed`，零分配）
/// - `unique_id()` 返 `"minimax#N"`（instance_index > 1 时）—— 给 poller map / DOM 区分
/// - `display_name()` 走 `t!("provider_name.minimax")` i18n + `t!("provider.suffix.dup", n = idx)`
///   后缀（**i18n key 复用** —— 中英都带 1 空格，符合"minimax #2"原例）
pub struct MinimaxSource {
    state: Arc<RwLock<MinimaxState>>,
    /// 1 = 内置第 1 份（默认），≥2 = 副本（由 `set_instance_index` / `with_instance_index` 设置）
    instance_index: u32,
}

impl Default for MinimaxSource {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(MinimaxState::default())),
            instance_index: 1,
        }
    }
}

impl MinimaxSource {
    /// Builder-style：带 instance_index 的新实例。
    ///
    /// **PR 1a 新增**。给 `instantiate_builtin(...)` 之后 `.set_instance_index()`
    /// 的等价函数式版本。后续 10 个 provider 复制这个签名。
    pub fn with_instance_index(mut self, idx: u32) -> Self {
        self.instance_index = idx;
        self
    }

    /// In-place 改 instance_index。给 `all_sources(state)` 已经拿到 Box 的场景用。
    #[allow(dead_code)] // 预留 v2 备用（PR 1b 用 with_instance_index 已覆盖当前路径）
    pub fn set_instance_index(&mut self, idx: u32) {
        self.instance_index = idx;
    }

    /// 每次 refresh tick 前由 commands.rs 调用，更新 region / overrides。
    #[allow(dead_code)] // 预留 v2 状态推送 API，Phase 1 重构后 commands 改走 trait 方法（impl QuotaSource 那个 set_state）。这里保留旧的 helper 给后续 unit test / 调试路径用。
    pub async fn set_state(&self, region: Region, overrides: ProviderOverrides) {
        let mut s = self.state.write().await;
        s.region = region;
        s.overrides = overrides;
    }
}

impl QuotaSource for MinimaxSource {
    fn id(&self) -> Cow<'_, str> {
        // 决策 1：id 永远返 base provider_id（"minimax"），不分副本
        Cow::Borrowed("minimax")
    }

    /// PR 1a 新增：全局唯一 id，含 instance_index 后缀。
    ///
    /// - instance_index == 1 → `"minimax"`（跟 id() 一样）
    /// - instance_index >= 2  → `"minimax#2"` / `"minimax#3"` / ...
    ///
    /// 用 `#` 分隔（不跟 filesystem 不友好字符撞，跟 js / DOM `data-source-id`
    /// 用法一致）。poller / settings panel / 浮窗 DOM 区分用这个。
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "minimax".to_string()
        } else {
            format!("minimax#{}", self.instance_index)
        }
    }

    fn display_name(&self) -> Cow<'_, str> {
        // 决策 3：渲染时拼。i18n key `provider.suffix.dup` = " #{}"
        // （中英都带 1 空格前缀，符合 "minimax #2" 原例）
        // 注意：t!() 返回 Cow 是临时值，**不能**包成 Cow::Borrowed（生命周期
        // 不够），统一用 Cow::Owned + into_owned。零分配优化等 v2 再说。
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.minimax").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.minimax").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKey
    }

    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // cfg 是 AppConfig 的 JSON，自己取需要的字段
            let region_str = cfg
                .get("providers")
                .and_then(|p| p.get("minimax"))
                .and_then(|m| m.get("region"))
                .and_then(|r| r.as_str())
                .unwrap_or("cn");
            let region = match region_str {
                "en" => Region::En,
                _ => Region::Cn,
            };
            let overrides: ProviderOverrides = cfg
                .get("schema_overrides")
                .and_then(|so| so.get("minimax"))
                .and_then(|m| serde_json::from_value(m.clone()).ok())
                .unwrap_or_default();
            let mut s = self.state.write().await;
            s.region = region;
            s.overrides = overrides;
        })
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
                    t!("error.provider.unconfigured_key", provider = "MiniMax").into_owned(),
                ));
            }
            let state = self.state.read().await.clone();
            Minimax::do_fetch(
                api_key,
                state.region,
                &state.overrides,
                &self.unique_id(),
                &self.display_name().to_string(),
            )
            .await
            .map(|(_, snap)| snap)
        })
    }
}

// ── 旧 ProviderImpl 兼容（dump CLI 还在用）────────────────────────

#[derive(Debug, Default)]
pub struct Minimax {
    /// 默认 CN；commands 层在调 fetch 前先从 config 复制一份给 Minimax
    pub region: Region,
}

impl Minimax {
    /// 真正的拉取实现（接受 region + 用户 overrides）。
    ///
    /// 返回 `Err(FetchError)` 给新代码用；`dump` CLI 把它转成 String。
    pub async fn do_fetch(
        api_key: &str,
        region: Region,
        overrides: &ProviderOverrides,
        source_id: &str,
        display_name: &str,
    ) -> Result<(serde_json::Value, ProviderSnapshot), FetchError> {
        if api_key.trim().is_empty() {
            return Err(FetchError::unconfigured(
                t!("error.common.api_key_empty").into_owned(),
            ));
        }

        let client = shared_client();

        let resp = client
            .get(region.api_url())
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| {
                FetchError::network(
                    t!(
                        "error.common.network",
                        url = region.api_url(),
                        err = e.to_string()
                    )
                    .into_owned(),
                )
            })?;

        let status = resp.status();
        // M16 fix: 429 显式 → RateLimited（前端用 RateLimited UI 展示 + poller 走 backoff）
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(FetchError::new(
                ErrorKind::RateLimited,
                t!("error.common.rate_limited", provider = "MiniMax").into_owned(),
            ));
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(FetchError::auth(
                t!("error.common.auth_failed", provider = "MiniMax").into_owned(),
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchError::auth(
                t!("error.provider.minimax_403").into_owned(),
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FetchError::server(
                t!(
                    "error.common.http_error",
                    provider = "MiniMax",
                    status = status.as_u16(),
                    body = body.chars().take(200).collect::<String>()
                )
                .into_owned(),
            ));
        }

        let raw: serde_json::Value = resp.json().await.map_err(|e| {
            FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
        })?;

        let snap = parse(&raw, region, overrides, source_id, display_name);
        Ok((raw, snap))
    }
}

// ── 解析逻辑（不变）────────────────────────────────────────────────

/// 灵活解析：兼容 6/1 前后的 schema
fn parse(
    raw: &serde_json::Value,
    _region: Region,
    overrides: &ProviderOverrides,
    source_id: &str,
    display_name: &str,
) -> ProviderSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 业务级错误（ccswitch 也这么做）
    if let Some(base_resp) = raw.get("base_resp") {
        let code = base_resp
            .get("status_code")
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if code != 0 {
            let msg = base_resp
                .get("status_msg")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            return ProviderSnapshot {
                // v0.3: 用 source_id ("minimax") 替代旧 enum 占位
                provider: "minimax".to_string(),
                success: false,
                rows: vec![],
                error: Some(
                    t!(
                        "error.common.business_code",
                        provider = "MiniMax",
                        code = code,
                        msg = msg
                    )
                    .into_owned(),
                ),
                error_kind: Some(ErrorKind::ServerError),
                fetched_at: Some(now_ms),
                next_fetch_at: None,
                raw: Some(raw.clone()),
                is_healthy: false,
                source_id: Some("minimax".to_string()),
                unique_id: None,
                source_display_name: Some("MiniMax".to_string()),
                plan_name: None,
                transient: None,
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
            provider: "minimax".to_string(),
            success: false,
            rows: vec![],
            error: Some(
                t!(
                    "error.common.missing_field",
                    provider = "MiniMax",
                    field = "model_remains[0]"
                )
                .into_owned(),
            ),
            error_kind: Some(ErrorKind::Parse),
            fetched_at: Some(now_ms),
            next_fetch_at: None,
            raw: Some(raw.clone()),
            is_healthy: false,
            source_id: Some("minimax".to_string()),
            unique_id: None,
            source_display_name: Some("MiniMax".to_string()),
            plan_name: None,
            transient: None,
        };
    };

    // 先试新 schema（percent-based），任一 tier 解不出时回退到旧 schema（count-based）
    let five_hour = parse_tier_percent(
        item,
        "current_interval_remaining_percent",
        "current_interval_status",
        "end_time",
    )
    .or_else(|| {
        parse_tier_count(
            item,
            &[
                // 旧 schema（ccswitch 老版本用的）
                (
                    "current_interval_total_count",
                    "current_interval_usage_count",
                    "end_time",
                ),
                // 候选新 schema（如果 6/1 改名了）
                ("interval_total", "interval_remaining", "interval_end"),
                ("window_total", "window_remaining", "window_end"),
                ("total_5h", "used_5h", "reset_5h"),
            ],
            &overrides.five_hour.count_candidates,
        )
    });

    let weekly = parse_tier_percent(
        item,
        "current_weekly_remaining_percent",
        "current_weekly_status",
        "weekly_end_time",
    )
    .or_else(|| {
        parse_tier_count(
            item,
            &[
                (
                    "current_weekly_total_count",
                    "current_weekly_usage_count",
                    "weekly_end_time",
                ),
                ("weekly_total", "weekly_remaining", "weekly_end"),
                ("total_week", "used_week", "reset_week"),
            ],
            &overrides.weekly.count_candidates,
        )
    });

    // 转成 QuotaRow
    let mut rows = Vec::new();
    if let Some(t) = five_hour {
        rows.push(QuotaRow {
            label: t!("row.five_hour").to_string(),
            utilization: Some(t.utilization),
            remaining: None,
            used: None,
            total: None,
            resets_at: t.resets_at,
            unit: Some("%".to_string()),
            extra: None,
            kind: None,
        });
    }
    if let Some(t) = weekly {
        rows.push(QuotaRow {
            label: t!("row.weekly").to_string(),
            utilization: Some(t.utilization),
            remaining: None,
            used: None,
            total: None,
            resets_at: t.resets_at,
            unit: Some("%".to_string()),
            extra: None,
            kind: None,
        });
    }

    let success = !rows.is_empty();
    let is_healthy = success; // MiniMax 拉到数据就认为可用

    ProviderSnapshot {
        provider: "minimax".to_string(),
        success,
        rows,
        error: if success {
            None
        } else {
            Some(t!("error.provider.schema_unknown_hint").into_owned())
        },
        error_kind: if success {
            None
        } else {
            Some(ErrorKind::SchemaUnknown)
        },
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name: None,
        transient: None,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TierInternal {
    utilization: f64,
    resets_at: Option<i64>,
}

/// 新 schema 解析（ccswitch 3.16.2+ / MiniMax 2026-06-01 之后）
///
/// - `k_percent`：剩余百分比字段（0-100）
/// - `k_status`：状态门控字段（1 = 套餐内）。status=2/3 仍可能带合法 percent：
///   - percent=100  → 哨兵，套餐外 / 未生效 → 视为无效
///   - percent=0    → "已用完 / 上限达到"，**仍要返回**让用户看到 100% + 重置时间
///     （旧逻辑在这里整行 drop，5h 达上限后浮窗 5h 行消失，回归：[bug]）
/// - `k_reset`：距离重置的**秒数**（不是 epoch ms）
///
/// 返回已用百分比 = 100 - remain%。
#[allow(dead_code)] // 预留 v2 公共 API（被同模块内未链接的 tier 解析路径使用，crate 内可见）
pub fn parse_tier_percent(
    item: &serde_json::Value,
    k_percent: &str,
    k_status: &str,
    k_reset: &str,
) -> Option<TierInternal> {
    // 1. 读取剩余百分比（核心字段，必须存在且在 0-100 范围内）
    let remain_pct = item.get(k_percent).and_then(num_to_f64)?;
    if !(0.0..=100.0).contains(&remain_pct) {
        return None;
    }
    let utilization = 100.0 - remain_pct;
    // 2. status 字段：信息性，不影响"是否显示"，只影响"值是否可信"。
    //    - status=1 或字段缺失：信任 percent
    //    - status=2/3 + percent=0：5h 达 100% 上限的合法状态，照常返回
    //    - status=2/3 + percent>0：percent 可能是"不在套餐"的哨兵 100，
    //      也可能是 percent 真实但 status 标错。为安全仍按原语义 drop。
    if let Some(s) = item.get(k_status).and_then(|v| v.as_i64()) {
        if s != 1 && remain_pct > 0.0 {
            return None;
        }
    }
    // 3. reset：智能识别（duration-seconds vs epoch-ms）
    let resets_at = item
        .get(k_reset)
        .and_then(|v| v.as_i64())
        .map(smart_reset_to_ms);
    Some(TierInternal {
        utilization,
        resets_at,
    })
}

/// 旧 schema 解析（count-based，已知 2026-06-01 后对 Plus 订阅者已不可靠）
///
/// 字段名带 "usage" 实际语义是"剩余"。已用% = (total - remaining) / total * 100。
///
/// `user_overrides` 先于内置 `candidates` 尝试，方便用户在设置面板加新字段名
/// （MiniMax 改 schema 后不用等发版）。
#[allow(dead_code)] // 预留 v2 公共 API（同 parse_tier_percent，crate 内可见）
pub fn parse_tier_count(
    item: &serde_json::Value,
    candidates: &[(&str, &str, &str)],
    user_overrides: &[crate::config::FieldTriple],
) -> Option<TierInternal> {
    // 1. 先试用户 overrides（按数组顺序）
    for triple in user_overrides {
        let total = item.get(&triple.total).and_then(num_to_f64);
        let remain = item.get(&triple.remaining).and_then(num_to_f64);
        if let (Some(t), Some(r)) = (total, remain) {
            if t > 0.0 {
                let utilization = ((t - r) / t) * 100.0;
                let resets_at = triple
                    .end
                    .as_deref()
                    .and_then(|k| {
                        item.get(k).and_then(|v| {
                            v.as_i64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        })
                    })
                    .map(smart_reset_to_ms);
                return Some(TierInternal {
                    utilization,
                    resets_at,
                });
            }
        }
    }
    // 2. 再试内置默认
    for (k_total, k_remain, k_reset) in candidates {
        let total = item.get(*k_total).and_then(num_to_f64);
        let remain = item.get(*k_remain).and_then(num_to_f64);
        if let (Some(t), Some(r)) = (total, remain) {
            if t > 0.0 {
                let utilization = ((t - r) / t) * 100.0;
                let resets_at = item
                    .get(*k_reset)
                    .and_then(|v| {
                        v.as_i64()
                            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    })
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

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderOverrides;
    use crate::providers::test_fixtures::minimax_new_schema;

    #[test]
    fn parse_tier_percent_status_must_be_1() {
        let item = serde_json::json!({
            "current_interval_remaining_percent": 80,
            "current_interval_status": 2,  // not in plan, percent=80 视为哨兵
            "end_time": 100
        });
        assert!(parse_tier_percent(
            &item,
            "current_interval_remaining_percent",
            "current_interval_status",
            "end_time"
        )
        .is_none());
    }

    #[test]
    fn parse_tier_percent_status_2_or_3_with_zero_percent_still_returns() {
        // 回归：MiniMax 5h 达 100% 上限时，API 会把 status 翻成 2/3
        // （exhausted / rate-limited 状态），但 percent 字段仍是 0。
        // 旧逻辑因 status != 1 整行 drop，浮窗 5h 行消失。新逻辑：percent=0
        // 时（无论 status 是什么）都返回 100% utilization，让用户看到上限 +
        // 重置时间。status=4 是"未知状态"的兜底，按同样规则处理。
        for status in [2, 3, 4] {
            let item = serde_json::json!({
                "current_interval_remaining_percent": 0,
                "current_interval_status": status,
                "end_time": 14523
            });
            let t = parse_tier_percent(
                &item,
                "current_interval_remaining_percent",
                "current_interval_status",
                "end_time",
            )
            .unwrap_or_else(|| panic!("status={status} + percent=0 must not drop"));
            assert!(
                (t.utilization - 100.0).abs() < 0.001,
                "status={status} utilization={}",
                t.utilization
            );
            // reset 字段也要正常解析
            assert!(
                t.resets_at.is_some(),
                "status={status} resets_at should be Some"
            );
        }
    }

    #[test]
    fn parse_tier_percent_missing_status_trusts_percent() {
        // 兼容：status 字段缺失（API 改名 / 老 schema）→ 直接信任 percent。
        // 旧逻辑会因 `let status = ...?` 早返 None，整行消失。
        let item = serde_json::json!({
            "current_interval_remaining_percent": 35,
            "end_time": 14523
        });
        let t = parse_tier_percent(
            &item,
            "current_interval_remaining_percent",
            "current_interval_status",
            "end_time",
        )
        .expect("missing status should fall back to trusting percent");
        assert!((t.utilization - 65.0).abs() < 0.001);
    }

    #[test]
    fn parse_tier_percent_basic() {
        let item = serde_json::json!({
            "current_interval_remaining_percent": 72,
            "current_interval_status": 1,
            "end_time": 14523  // duration seconds
        });
        let t = parse_tier_percent(
            &item,
            "current_interval_remaining_percent",
            "current_interval_status",
            "end_time",
        )
        .unwrap();
        assert!(
            (t.utilization - 28.0).abs() < 0.001,
            "utilization = {}",
            t.utilization
        );
        let resets = t.resets_at.unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        // resets should be ~14523s in the future
        assert!(resets > now, "resets should be in the future");
        assert!(
            resets - now <= 14523 * 1000 + 100,
            "resets within 14523s + 100ms"
        );
    }

    #[test]
    fn parse_tier_percent_epoch_ms() {
        // end_time > 10^12 → treat as epoch ms
        let future = chrono::Utc::now().timestamp_millis() + 3_600_000;
        let item = serde_json::json!({
            "current_interval_remaining_percent": 50,
            "current_interval_status": 1,
            "end_time": future
        });
        let t = parse_tier_percent(
            &item,
            "current_interval_remaining_percent",
            "current_interval_status",
            "end_time",
        )
        .unwrap();
        assert_eq!(t.resets_at, Some(future));
    }

    #[test]
    fn parse_tier_percent_out_of_range() {
        let item = serde_json::json!({
            "current_interval_remaining_percent": 150,  // invalid
            "current_interval_status": 1,
            "end_time": 100
        });
        assert!(parse_tier_percent(
            &item,
            "current_interval_remaining_percent",
            "current_interval_status",
            "end_time"
        )
        .is_none());
    }

    #[test]
    fn parse_tier_count_basic() {
        let item = serde_json::json!({
            "current_interval_total_count": 200,
            "current_interval_usage_count": 56,
            "end_time": 14523
        });
        let t = parse_tier_count(
            &item,
            &[(
                "current_interval_total_count",
                "current_interval_usage_count",
                "end_time",
            )],
            &[],
        )
        .unwrap();
        // (200-56)/200 = 0.72 → 72%
        assert!((t.utilization - 72.0).abs() < 0.001);
    }

    #[test]
    fn parse_tier_count_user_override_wins() {
        // user adds a custom field name; it should be tried first
        let item = serde_json::json!({
            "new_total": 100,
            "new_remaining": 25
        });
        let overrides = vec![crate::config::FieldTriple {
            total: "new_total".into(),
            remaining: "new_remaining".into(),
            end: None,
        }];
        let t = parse_tier_count(&item, &[], &overrides).unwrap();
        assert!((t.utilization - 75.0).abs() < 0.001);
    }

    #[test]
    fn parse_tier_count_zero_total_rejected() {
        let item = serde_json::json!({
            "current_interval_total_count": 0,
            "current_interval_usage_count": 0
        });
        assert!(parse_tier_count(
            &item,
            &[(
                "current_interval_total_count",
                "current_interval_usage_count",
                "end_time"
            )],
            &[]
        )
        .is_none());
    }

    #[test]
    fn parse_full_new_schema_snapshot() {
        let raw = minimax_new_schema();
        let snap = parse(
            &raw,
            Region::Cn,
            &ProviderOverrides::default(),
            "minimax",
            "MiniMax",
        );
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, t!("row.five_hour"));
        assert_eq!(snap.rows[1].label, t!("row.weekly"));
        // 5h: 100-72=28%; week: 100-86=14%
        assert!((snap.rows[0].utilization.unwrap() - 28.0).abs() < 0.001);
        assert!((snap.rows[1].utilization.unwrap() - 14.0).abs() < 0.001);
    }

    #[test]
    fn parse_full_business_error() {
        let raw = serde_json::json!({
            "base_resp": { "status_code": 1004, "status_msg": "rate limit" }
        });
        let snap = parse(
            &raw,
            Region::Cn,
            &ProviderOverrides::default(),
            "minimax",
            "MiniMax",
        );
        assert!(!snap.success);
        assert_eq!(snap.error_kind, Some(ErrorKind::ServerError));
    }

    #[test]
    fn parse_full_no_model_remains() {
        let raw = serde_json::json!({
            "base_resp": { "status_code": 0 }
        });
        let snap = parse(
            &raw,
            Region::Cn,
            &ProviderOverrides::default(),
            "minimax",
            "MiniMax",
        );
        assert!(!snap.success);
        assert_eq!(snap.error_kind, Some(ErrorKind::Parse));
    }
}
