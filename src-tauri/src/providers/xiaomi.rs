//! Xiaomi MiMo Token Plan 用量查询
//!
//! ⚠️ Xiaomi 的 usage query **走 dashboard admin API**，不是公开 endpoint：
//! - `GET https://platform.xiaomimimo.com/api/v1/tokenPlan/usage`
//! - `GET https://platform.xiaomimimo.com/api/v1/tokenPlan/detail`
//!
//! **Auth 不是 Bearer 也不是 api-key header，是 Cookie**：需要把浏览器登录后
//! DevTools 里那个请求的完整 Cookie header 值贴到 Musage 设置面板。
//! Cookie 内容：
//! ```
//! api-platform_serviceToken="..."; userId=...; api-platform_slh="..."; api-platform_ph="..."
//! ```
//!
//! 用户操作：
//! 1. 浏览器登录 https://platform.xiaomimimo.com → 进"订阅管理"
//! 2. F12 → Network → 任意一个 `/api/v1/tokenPlan/*` 请求 → 右键 → Copy → Copy headers
//! 3. 在 Musage 设置面板 · Xiaomi · "Dashboard Cookie" 字段粘贴
//! 4. 保存 → 后台轮询会用这个 cookie 拉数据
//!
//! Cookie 会随用户登出失效，过期时 (HTTP 401) 错误信息会引导用户重新粘贴。

use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{shared_client, AuthKind, Credentials, ErrorKind, FetchError, Provider, ProviderImpl, ProviderSnapshot, QuotaRow, QuotaSource};
use crate::config::ProviderOverrides;

/// 公开 endpoint（dashboard admin API，不是 token-plan 子域）
const USAGE_URL: &str = "https://platform.xiaomimimo.com/api/v1/tokenPlan/usage";
const DETAIL_URL: &str = "https://platform.xiaomimimo.com/api/v1/tokenPlan/detail";

/// 当前 Musage 不需要 region（endpoint 跟 cluster 无关，cookie 已绑定 user）
/// —— 但保留 enum 以备未来多账号
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum XiaomiRegion {
    #[default]
    Cn,
    Sgp,
    Ams,
}

impl XiaomiRegion {
    pub fn label(&self) -> &'static str {
        match self {
            XiaomiRegion::Cn => "🇨🇳 中国",
            XiaomiRegion::Sgp => "🌏 新加坡",
            XiaomiRegion::Ams => "🌍 欧洲",
        }
    }
}

/// 浮窗显示模式：用户可切 完整 / 只套餐 / 只总额度。
///
/// - `All`：3 行（套餐 + 补偿 + 总额度），套餐/总额度数字一致时自动去重
/// - `PlanOnly`：只显示套餐 1 行（适合只关心"套餐还剩多少"）
/// - `TotalOnly`（默认）：只显示总额度 1 行（适合有补偿积分的用户看综合消耗），
///   此时总额度会复用套餐的 resets_at（也是月度重置）
///
/// 序列化成 `"all" | "plan_only" | "total_only"`，跟前端 SourceMeta 同套约定。
///
/// 默认值改成 `TotalOnly` 是有意为之 —— 总额度是"本月总消耗"这个**最关键**
/// 单一指标（不管有没有补偿，都能在 1 行里讲清楚用户的真实用量）。
/// 想要看明细再切到 All 或 PlanOnly。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum XiaomiDisplayMode {
    All,
    PlanOnly,
    #[default]
    TotalOnly,
}

// ── QuotaSource 实现（Phase 1）────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct XiaomimimoState {
    pub region: XiaomiRegion,
    pub overrides: ProviderOverrides,
    /// 用户在设置面板选的显示模式（默认 All = 完整 3 行 + 自动去重）
    pub display_mode: XiaomiDisplayMode,
}

pub struct XiaomimimoSource {
    state: Arc<RwLock<XiaomimimoState>>,
}

impl Default for XiaomimimoSource {
    fn default() -> Self {
        Self { state: Arc::new(RwLock::new(XiaomimimoState::default())) }
    }
}

impl XiaomimimoSource {
    pub async fn set_state(
        &self,
        region: XiaomiRegion,
        overrides: ProviderOverrides,
        display_mode: XiaomiDisplayMode,
    ) {
        let mut s = self.state.write().await;
        s.region = region;
        s.overrides = overrides;
        s.display_mode = display_mode;
    }
}

impl QuotaSource for XiaomimimoSource {
    fn id(&self) -> &'static str { "xiaomimimo" }
    fn display_name(&self) -> &'static str { "Xiaomi MiMo" }
    /// 优先 Bearer（API key），401 时降级到 Cookie。两个输入都展示在设置面板。
    /// 决策逻辑见 [`decide_auth_strategy`] + [`Xiaomimimo::fetch`]。
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKeyOrCookie }

    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let region_str = cfg.get("providers")
                .and_then(|p| p.get("xiaomimimo"))
                .and_then(|m| m.get("xiaomi_region"))
                .and_then(|r| r.as_str())
                .unwrap_or("cn");
            let region = match region_str {
                "sgp" => XiaomiRegion::Sgp,
                "ams" => XiaomiRegion::Ams,
                _ => XiaomiRegion::Cn,
            };
            let overrides: ProviderOverrides = cfg.get("schema_overrides")
                .and_then(|so| so.get("xiaomimimo"))
                .and_then(|m| serde_json::from_value(m.clone()).ok())
                .unwrap_or_default();
            let display_mode: XiaomiDisplayMode = cfg.get("providers")
                .and_then(|p| p.get("xiaomimimo"))
                .and_then(|m| m.get("xiaomi_display_mode"))
                .and_then(|d| serde_json::from_value(d.clone()).ok())
                .unwrap_or_default();
            let mut s = self.state.write().await;
            s.region = region;
            s.overrides = overrides;
            s.display_mode = display_mode;
        })
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let state = self.state.read().await.clone();
            let display_mode = state.display_mode;
            let strategy = decide_auth_strategy(credentials);
            let fetch_result = match strategy {
                AuthStrategy::None => Err(FetchError::unconfigured(
                    "未配置 API key 或 Dashboard cookie（设置面板填入）"
                )),
                AuthStrategy::BearerOnly => {
                    // 安全：strategy 保证 Some
                    let key = credentials.api_key.as_deref().unwrap();
                    Xiaomimimo::do_fetch_bearer(key, state.region, &state.overrides)
                        .await
                        .map(|(_, snap)| snap)
                }
                AuthStrategy::CookieOnly => {
                    let cookie = credentials.cookie.as_deref().unwrap();
                    Xiaomimimo::do_fetch(cookie, state.region, &state.overrides)
                        .await
                        .map(|(_, snap)| snap)
                }
                AuthStrategy::BearerThenCookie => {
                    let key = credentials.api_key.as_deref().unwrap();
                    let cookie = credentials.cookie.as_deref().unwrap();
                    // 先 Bearer，401/403 退到 Cookie（其他错误原样返）
                    match Xiaomimimo::do_fetch_bearer(key, state.region, &state.overrides).await {
                        Ok((_, snap)) => {
                            tracing::debug!(provider = "xiaomimimo", "Bearer 路径成功");
                            Ok(snap)
                        }
                        Err(e) if matches!(e.kind, ErrorKind::AuthFailed) => {
                            tracing::info!(
                                provider = "xiaomimimo",
                                "Bearer 鉴权失败 ({}), 退到 Cookie 路径",
                                e.message
                            );
                            Xiaomimimo::do_fetch(cookie, state.region, &state.overrides)
                                .await
                                .map(|(_, snap)| snap)
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            fetch_result.map(|snap| apply_display_mode(snap, display_mode))
        })
    }
}

// ── 鉴权策略（pure 函数，易测）────────────────────────────────

/// 鉴权策略：根据 Credentials 里有几个非空字段决定走哪条 fetch 路径。
///
/// - 两个都有 → 先 Bearer，401 退 Cookie
/// - 只有 api_key → 只 Bearer
/// - 只有 cookie → 只 Cookie
/// - 都没有 → Unconfigured
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthStrategy {
    None,
    BearerOnly,
    CookieOnly,
    BearerThenCookie,
}

pub(crate) fn decide_auth_strategy(creds: &Credentials) -> AuthStrategy {
    let has_key = creds
        .api_key
        .as_deref()
        .map(str::trim)
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let has_cookie = creds
        .cookie
        .as_deref()
        .map(str::trim)
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    match (has_key, has_cookie) {
        (true, true) => AuthStrategy::BearerThenCookie,
        (true, false) => AuthStrategy::BearerOnly,
        (false, true) => AuthStrategy::CookieOnly,
        (false, false) => AuthStrategy::None,
    }
}

// ── 旧 ProviderImpl 兼容（dump CLI 还在用）────────────────────────

#[derive(Debug, Default)]
pub struct Xiaomimimo {
    pub region: XiaomiRegion,
}

impl Xiaomimimo {
    /// Bearer 路径：拼 `Authorization: Bearer <api_key>`，401 走 AuthFailed，
    /// 让上层 [`XiaomimimoSource::fetch`] 的 BearerThenCookie 策略退到 Cookie。
    ///
    /// 当前实测（2026-06）：`platform.xiaomimimo.com/api/v1/tokenPlan/usage`
    /// 对纯 Bearer 返 401（dashboard admin API 走 session 守护）—— 所以
    /// Bearer 单独填 API key 不会成功，**但**配双鉴权（API key + Cookie）
    /// 时，401 触发自动 fallback 不会让用户感知。
    pub async fn do_fetch_bearer(
        api_key: &str,
        _region: XiaomiRegion,
        overrides: &ProviderOverrides,
    ) -> Result<(serde_json::Value, ProviderSnapshot), FetchError> {
        if api_key.trim().is_empty() {
            return Err(FetchError::unconfigured("API key 为空"));
        }
        let client = shared_client();
        let resp = client
            .get(USAGE_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| FetchError::network(format!("网络错误 [{}]: {e}", USAGE_URL)))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(FetchError::auth(
                "Xiaomi API key 鉴权失败 (HTTP 401) — 当前用量 API 仅对 dashboard cookie 放行，请改填 Cookie 或两者都填（401 会自动退到 Cookie 路径）",
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchError::auth(
                "无权限访问 Xiaomi dashboard API (HTTP 403) — API key 可能未订阅 Token Plan，或用量 API 对 Bearer key 关闭",
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FetchError::server(format!(
                "Xiaomi dashboard API 异常 (HTTP {status}): {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        let raw: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;
        if let Some(code) = raw.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
                return Err(FetchError::server(format!(
                    "Xiaomi dashboard API code {code}: {msg}"
                )));
            }
        }
        // detail 失败不阻塞
        let detail_raw: serde_json::Value = match client
            .get(DETAIL_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                r.json().await.unwrap_or(serde_json::Value::Null)
            }
            _ => serde_json::Value::Null,
        };
        // 先借用 raw 算 snap，再 move raw 进 tuple（顺序很关键）
        let snap = parse(&raw, &detail_raw, overrides);
        Ok((raw, snap))
    }

    pub async fn do_fetch(
        cookie: &str,
        _region: XiaomiRegion,
        overrides: &ProviderOverrides,
    ) -> Result<(serde_json::Value, ProviderSnapshot), FetchError> {
        if cookie.trim().is_empty() {
            return Err(FetchError::unconfigured("Dashboard cookie 为空（设置面板 · Xiaomi · Dashboard Cookie）"));
        }

        let client = shared_client();

        // ── 1. 拉用量
        let resp = client
            .get(USAGE_URL)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| FetchError::network(format!("网络错误 [{}]: {e}", USAGE_URL)))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(FetchError::auth("Cookie 失效或无效 —— 请重新登录 platform.xiaomimimo.com → DevTools 复制新 Cookie"));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchError::auth("无权限访问 Xiaomi dashboard API（HTTP 403）"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FetchError::server(format!(
                "Xiaomi dashboard API 异常 (HTTP {status}): {}",
                body.chars().take(200).collect::<String>()
            )));
        }

        let raw: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

        // 业务级 code
        if let Some(code) = raw.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
                return Err(FetchError::server(format!("Xiaomi dashboard API code {code}: {msg}")));
            }
        }

        // ── 2. 拉详情（拿套餐到期时间）—— 失败不阻塞
        let detail_raw: serde_json::Value = match client
            .get(DETAIL_URL)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r.json().await.unwrap_or(serde_json::Value::Null),
            _ => serde_json::Value::Null,
        };

        let snap = parse(&raw, &detail_raw, overrides);
        Ok((raw, snap))
    }
}

impl ProviderImpl for Xiaomimimo {
    fn id(&self) -> Provider { Provider::Xiaomimimo }
    fn display_name(&self) -> &'static str { "Xiaomi MiMo" }

    fn fetch<'a>(
        &'a self,
        _api_key: &'a str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, String>> + Send + 'a>> {
        Box::pin(async move {
            Err("Xiaomi MiMo 走 do_fetch（需要 dashboard cookie），ProviderImpl::fetch 未实现".to_string())
        })
    }
}

// ── 解析逻辑（不变）────────────────────────────────────────────────

/// 解析 usage + detail 的 response
///
/// usage schema：
/// ```json
/// {
///   "data": {
///     "monthUsage": {"percent":0.3483,"items":[{"name":"month_total_token","used":...,"limit":...,"percent":...}]},
///     "usage":      {"percent":0.06,  "items":[{"name":"plan_total_token", ...},
///                                            {"name":"compensation_total_token", ...}]}
///   }
/// }
/// ```
///
/// detail schema：
/// ```json
/// {
///   "data": {
///     "planCode":"standard",
///     "planName":"Standard",
///     "currentPeriodEnd":"2026-06-27 23:59:59",  ← UTC
///     "expired":false
///   }
/// }
/// ```
fn parse(
    raw_usage: &serde_json::Value,
    raw_detail: &serde_json::Value,
    overrides: &ProviderOverrides,
) -> ProviderSnapshot {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // ── 套餐到期时间（UTC）
    let resets_at = raw_detail
        .pointer("/data/currentPeriodEnd")
        .and_then(|v| v.as_str())
        .and_then(parse_datetime_utc_ms);

    // plan 名（detail.data.planName，例 "Standard" / "Plus"）
    let plan_name = raw_detail
        .pointer("/data/planName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // 用户自定义字段名：用 monthly tier 的 count_candidates[].total 当作
    // "item.name 候选"（FieldTriple 的 remaining/end 对 Xiaomi 没用，忽略）。
    // 不引入新结构，复用 minimax 已有的 ProviderOverrides 路径。
    let custom_names: Vec<&str> = overrides
        .monthly
        .count_candidates
        .iter()
        .map(|t| t.total.as_str())
        .collect();

    let mut rows = Vec::new();

    // 抓 3 个 item 的 percent（None = 字段不存在或 % 越界）
    let plan_pct = pick_item_percent(
        raw_usage,
        "/data/usage/items",
        &custom_names,
        "plan_total_token",
    );
    let comp_pct = pick_item_percent(
        raw_usage,
        "/data/usage/items",
        &custom_names,
        "compensation_total_token",
    );
    let month_pct = pick_item_percent(
        raw_usage,
        "/data/monthUsage/items",
        &custom_names,
        "month_total_token",
    );

    // ── 1. 套餐（plan_total_token）—— 主指标，dashboard 显示的就是它
    if let Some(pct) = plan_pct {
        rows.push(QuotaRow {
            label: "套餐".to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    // ── 2. 补偿积分（compensation_total_token）
    if let Some(pct) = comp_pct {
        rows.push(QuotaRow {
            label: "补偿".to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at: None,
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    // ── 3. 总额度（month_total_token）—— 本月所有额度合计
    // 去重：套餐和总额度数字基本相等时（典型：月度重置 + 无补偿用户）
    // 总额度这行不显示，避免"套餐 13% / 总额度 13%"这种重复信息
    let show_total = match (plan_pct, month_pct) {
        (Some(p), Some(m)) => (m - p).abs() >= 0.5,
        (None, Some(_)) => true,   // 没套餐但有总额度（schema 变了）→ 还是显示
        (Some(_), None) => false,  // 有套餐但没总额度 → 隐式 skipped
        (None, None) => false,
    };
    if show_total {
        if let Some(pct) = month_pct {
            rows.push(QuotaRow {
                label: "总额度".to_string(),
                utilization: Some(pct),
                remaining: None,
                used: None,
                total: None,
                resets_at: None,
                unit: Some("%".to_string()),
                extra: None,
            });
        }
    }

    let success = !rows.is_empty();
    ProviderSnapshot {
        provider: Provider::Xiaomimimo,
        success,
        rows,
        error: if success {
            None
        } else {
            // empty rows 的根因可能不一样：raw 里有没有 data 字段？有 data 但
            // 没有任何已知 item 名称 → schema 改名了，提示用户去 schema_overrides
            // 配新名；没 data → 响应结构变了，归 Parse。
            let has_data = raw_usage.pointer("/data").is_some();
            if has_data {
                Some("响应里找不到 plan_total_token / compensation_total_token / month_total_token 任何一项（schema 改名？去设置面板 schema_overrides 配新名）".to_string())
            } else {
                Some("响应缺少 data 字段".to_string())
            }
        },
        error_kind: if success {
            None
        } else if raw_usage.pointer("/data").is_some() {
            Some(ErrorKind::SchemaUnknown)
        } else {
            Some(ErrorKind::Parse)
        },
        fetched_at: Some(now_ms),
        raw: Some(raw_usage.clone()),
        is_healthy: success,
        source_id: Some(Provider::Xiaomimimo.id_str().to_string()),
        source_display_name: Some(Provider::Xiaomimimo.display_name().to_string()),
        plan_name,
    }
}

/// 在 JSON 树里找 `items[]` 中 `name == <target>` 的那一项，返回 `percent * 100`
fn get_item_percent(root: &serde_json::Value, items_path: &str, name: &str) -> Option<f64> {
    let items = root.pointer(items_path).and_then(|v| v.as_array())?;
    items
        .iter()
        .find(|i| i.get("name").and_then(|n| n.as_str()) == Some(name))
        .and_then(|i| i.get("percent").and_then(|p| p.as_f64()))
        .map(|p| p * 100.0)
}

/// 按"用户自定义名（按数组顺序）→ 内置默认名"依次查找 `items[].percent`。
///
/// `custom_names` 来自 `ProviderOverrides.monthly.count_candidates[].total` ——
///// minimax 用的 `count_candidates` 字段被复用为"item.name 候选"（Xiaomi 没
/// 有 count 概念，FieldTriple 的 remaining/end 字段对 Xiaomi 没用，所以不引入
/// 新结构）。
///
/// 第一个命中就返回；都不命中返 None。
fn pick_item_percent(
    raw: &serde_json::Value,
    items_path: &str,
    custom_names: &[&str],
    default_name: &str,
) -> Option<f64> {
    for name in custom_names {
        if let Some(p) = get_item_percent(raw, items_path, name) {
            return Some(p);
        }
    }
    get_item_percent(raw, items_path, default_name)
}

/// 按用户选的 [`XiaomiDisplayMode`] 过滤 rows，并在 TotalOnly 时给总额度
/// 注入 resets_at（复用套餐的月度重置时间，因为总额度也是按月清零）。
///
/// 放在 parse() 之后、抛给前端之前的过滤层 —— parse() 保持 pure
/// 行为（不依赖 state），方便单测。
fn apply_display_mode(snap: ProviderSnapshot, mode: XiaomiDisplayMode) -> ProviderSnapshot {
    match mode {
        XiaomiDisplayMode::All => snap,
        XiaomiDisplayMode::PlanOnly => {
            // 只留套餐行
            let rows: Vec<QuotaRow> = snap
                .rows
                .into_iter()
                .filter(|r| r.label == "套餐")
                .collect();
            ProviderSnapshot { rows, ..snap }
        }
        XiaomiDisplayMode::TotalOnly => {
            // 只留总额度行；如果 parse() 没给 resets_at（默认就没给），
            // 复用套餐的月度重置时间（rows[0] 或 fallback 到 detail 里的）——
            // 但 parse() 之后 detail 已不在 snap 里，所以这里用
            // snap.rows 里其他行有 resets_at 的就借过来。
            let plan_resets_at = snap.rows.iter().find_map(|r| r.resets_at);
            let rows: Vec<QuotaRow> = snap
                .rows
                .into_iter()
                .filter(|r| r.label == "总额度")
                .map(|mut r| {
                    if r.resets_at.is_none() {
                        r.resets_at = plan_resets_at;
                    }
                    r
                })
                .collect();
            ProviderSnapshot { rows, ..snap }
        }
    }
}

/// "2026-06-27 23:59:59" → epoch ms（**UTC**，按 dashboard 标注）
fn parse_datetime_utc_ms(s: &str) -> Option<i64> {
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(dt.and_utc().timestamp_millis())
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FieldTriple, TierOverrides};
    use crate::providers::Credentials;
    use serde_json::json;

    // ── 鉴权策略 ──

    #[test]
    fn decide_strategy_both_present() {
        let c = Credentials {
            api_key: Some("tp-xxx".to_string()),
            cookie: Some("a=1; b=2".to_string()),
        };
        assert_eq!(decide_auth_strategy(&c), AuthStrategy::BearerThenCookie);
    }

    #[test]
    fn decide_strategy_bearer_only() {
        let c = Credentials {
            api_key: Some("tp-xxx".to_string()),
            cookie: None,
        };
        assert_eq!(decide_auth_strategy(&c), AuthStrategy::BearerOnly);
    }

    #[test]
    fn decide_strategy_cookie_only() {
        let c = Credentials {
            api_key: None,
            cookie: Some("a=1; b=2".to_string()),
        };
        assert_eq!(decide_auth_strategy(&c), AuthStrategy::CookieOnly);
    }

    #[test]
    fn decide_strategy_none() {
        let c = Credentials::default();
        assert_eq!(decide_auth_strategy(&c), AuthStrategy::None);
    }

    #[test]
    fn decide_strategy_whitespace_only_is_empty() {
        // trim 后为空字符串 → 当作没配
        let c = Credentials {
            api_key: Some("   ".to_string()),
            cookie: Some("\t\n".to_string()),
        };
        assert_eq!(decide_auth_strategy(&c), AuthStrategy::None);
    }

    #[test]
    fn decide_strategy_mixed_whitespace() {
        // api_key 真有值，cookie 全空白 → BearerOnly
        let c = Credentials {
            api_key: Some("tp-xxx".to_string()),
            cookie: Some("   ".to_string()),
        };
        assert_eq!(decide_auth_strategy(&c), AuthStrategy::BearerOnly);
    }

    #[test]
    fn parse_full_response_three_rows() {
        let raw = json!({
            "code": 0,
            "data": {
                "monthUsage": {"percent":0.3, "items":[
                    {"name":"month_total_token","used":1.0,"limit":3.0,"percent":0.3}
                ]},
                "usage": {"percent":0.06, "items":[
                    {"name":"plan_total_token","used":0.5,"limit":8.0,"percent":0.06},
                    {"name":"compensation_total_token","used":0.1,"limit":2.0,"percent":0.05}
                ]}
            }
        });
        let detail = json!({
            "code": 0,
            "data": {
                "planName": "Standard",
                "currentPeriodEnd": "2026-06-27 23:59:59",
                "expired": false
            }
        });
        let snap = parse(&raw, &detail, &ProviderOverrides::default());
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.plan_name.as_deref(), Some("Standard"));
        assert_eq!(snap.rows.len(), 3);
        assert_eq!(snap.rows[0].label, "套餐");
        assert_eq!(snap.rows[1].label, "补偿");
        assert_eq!(snap.rows[2].label, "总额度");  // "月度" 改名
        assert!((snap.rows[0].utilization.unwrap() - 6.0).abs() < 0.001);
        assert!((snap.rows[1].utilization.unwrap() - 5.0).abs() < 0.001);
        assert!((snap.rows[2].utilization.unwrap() - 30.0).abs() < 0.001);
        // 套餐行带 resets_at；补偿/总额度不带
        assert!(snap.rows[0].resets_at.is_some());
        assert!(snap.rows[1].resets_at.is_none());
        assert!(snap.rows[2].resets_at.is_none());
    }

    #[test]
    fn parse_dedup_total_when_equal_to_plan() {
        // 月度重置 + 无补偿用户：套餐和总额度数字一致 → 总额度这一行 skip
        let raw = json!({
            "code": 0,
            "data": {
                "monthUsage": {"percent":0.13, "items":[
                    {"name":"month_total_token","percent":0.13}
                ]},
                "usage": {"percent":0.13, "items":[
                    {"name":"plan_total_token","percent":0.13}
                    // 注意：没有 compensation_total_token（无补偿用户）
                ]}
            }
        });
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default());
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.rows.len(), 1, "套餐和总额度相等 → 只显示套餐 1 行");
        assert_eq!(snap.rows[0].label, "套餐");
        assert!((snap.rows[0].utilization.unwrap() - 13.0).abs() < 0.001);
    }

    #[test]
    fn parse_dedup_total_near_equal_but_not_exact() {
        // 0.3% 差（小于 0.5% 阈值）→ 算"基本相等" → 总额度 skip
        // 容忍度：避免 dashboard 浮点精度导致 13.0 vs 12.97 这种误判
        let raw = json!({
            "code": 0,
            "data": {
                "monthUsage": {"percent":0.1301, "items":[
                    {"name":"month_total_token","percent":0.1301}
                ]},
                "usage": {"percent":0.13, "items":[
                    {"name":"plan_total_token","percent":0.13}
                ]}
            }
        });
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default());
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1, "差 0.01% < 0.5% 阈值 → 总额度 skip");
    }

    #[test]
    fn parse_dedup_total_just_above_threshold() {
        // 0.6% 差（超过 0.5% 阈值）→ 算"不同" → 总额度显示
        let raw = json!({
            "code": 0,
            "data": {
                "monthUsage": {"percent":0.136, "items":[
                    {"name":"month_total_token","percent":0.136}
                ]},
                "usage": {"percent":0.13, "items":[
                    {"name":"plan_total_token","percent":0.13}
                ]}
            }
        });
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default());
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 2, "差 0.6% >= 0.5% 阈值 → 两行都显示");
        assert_eq!(snap.rows[0].label, "套餐");
        assert_eq!(snap.rows[1].label, "总额度");
    }

    #[test]
    fn parse_total_only_no_plan() {
        // 极端：套餐字段缺失（schema 改了），只有总额度 → 还是显示总额度
        let raw = json!({
            "code": 0,
            "data": {
                "monthUsage": {"percent":0.5, "items":[
                    {"name":"month_total_token","percent":0.5}
                ]},
                "usage": {"items":[]}
            }
        });
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default());
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "总额度");
    }

    #[test]
    fn parse_no_total_field() {
        // 总额度字段缺失 → 只显示套餐和补偿
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[
                    {"name":"plan_total_token","percent":0.5},
                    {"name":"compensation_total_token","percent":0.2}
                ]}
                // monthUsage 整个缺失
            }
        });
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default());
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, "套餐");
        assert_eq!(snap.rows[1].label, "补偿");
    }

    #[test]
    fn parse_only_plan_no_compensation_no_month() {
        // dashboard 升级过程中可能暂时只回 plan 一项
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[
                    {"name":"plan_total_token","used":0.4,"limit":1.0,"percent":0.4}
                ]}
            }
        });
        let detail = json!({});
        let snap = parse(&raw, &detail, &ProviderOverrides::default());
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "套餐");
        assert!((snap.rows[0].utilization.unwrap() - 40.0).abs() < 0.001);
    }

    #[test]
    fn parse_no_known_items_is_schema_unknown() {
        // 有 data 字段但没有任何已知 item 名称 → 提示用户去 schema_overrides 配
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[{"name":"some_other_token","percent":0.5}]}
            }
        });
        let detail = json!({});
        let snap = parse(&raw, &detail, &ProviderOverrides::default());
        assert!(!snap.success);
        assert_eq!(snap.error_kind, Some(ErrorKind::SchemaUnknown));
        assert!(snap.error.unwrap().contains("schema_overrides"));
    }

    #[test]
    fn parse_no_data_field_is_parse_error() {
        // 响应结构彻底变了，连 /data 都没有
        let raw = json!({"code": 0, "result": "ok"});
        let detail = json!({});
        let snap = parse(&raw, &detail, &ProviderOverrides::default());
        assert!(!snap.success);
        assert_eq!(snap.error_kind, Some(ErrorKind::Parse));
        assert!(snap.error.unwrap().contains("data"));
    }

    #[test]
    fn parse_business_error_code() {
        let raw = json!({"code": 1001, "message": "rate limit"});
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default());
        assert!(!snap.success);
        // NOTE: xiaomi 的 do_fetch 在 code != 0 时已经返回 Err 走 FetchError 路径，
        // 所以 parse() 本身不会见到业务级 code。这里模拟的是"parse 兜底"——就算
        // 调用方没拦 code，parse 也不会把脏数据塞进 rows。
        assert_eq!(snap.rows.len(), 0);
    }

    #[test]
    fn parse_user_override_picks_custom_field_name() {
        // 模拟 Xiaomi 改名 plan_total_token → new_plan_token
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[{"name":"new_plan_token","used":0.42,"limit":1.0,"percent":0.42}]}
            }
        });
        let overrides = ProviderOverrides {
            monthly: TierOverrides {
                count_candidates: vec![FieldTriple {
                    total: "new_plan_token".to_string(),
                    remaining: String::new(),
                    end: None,
                }],
            },
            ..Default::default()
        };
        let snap = parse(&raw, &json!({}), &overrides);
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "套餐");
        assert!((snap.rows[0].utilization.unwrap() - 42.0).abs() < 0.001);
    }

    #[test]
    fn parse_user_override_wrong_name_falls_back_to_default() {
        // 用户配错名了 → 不要直接 None，fallback 到内置默认名
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[{"name":"plan_total_token","percent":0.5}]}
            }
        });
        let overrides = ProviderOverrides {
            monthly: TierOverrides {
                count_candidates: vec![FieldTriple {
                    total: "wrong_name".to_string(),
                    remaining: String::new(),
                    end: None,
                }],
            },
            ..Default::default()
        };
        let snap = parse(&raw, &json!({}), &overrides);
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.rows.len(), 1);
        assert!((snap.rows[0].utilization.unwrap() - 50.0).abs() < 0.001);
    }

    #[test]
    fn parse_resets_at_parsed_from_detail() {
        let raw = json!({
            "code": 0,
            "data": {"usage":{"items":[{"name":"plan_total_token","percent":0.1}]}}
        });
        let detail = json!({"data":{"currentPeriodEnd":"2026-06-27 23:59:59"}});
        let snap = parse(&raw, &detail, &ProviderOverrides::default());
        assert!(snap.success);
        let resets = snap.rows[0].resets_at.unwrap();
        // 2026-06-27 23:59:59 UTC ≈ 1.785 * 10^12 ms
        assert!(resets > 1_785_000_000_000 && resets < 1_786_000_000_000);
    }

    #[test]
    fn parse_datetime_utc_ms_works() {
        let ms = parse_datetime_utc_ms("2026-06-27 23:59:59").unwrap();
        assert!(ms > 1_785_000_000_000 && ms < 1_786_000_000_000);
        assert!(parse_datetime_utc_ms("not a date").is_none());
        assert!(parse_datetime_utc_ms("").is_none());
    }

    #[test]
    fn pick_item_percent_order() {
        // override 在前 → override 命中就停
        let raw = json!({
            "data": {
                "items": [
                    {"name":"a","percent":0.1},
                    {"name":"b","percent":0.5}
                ]
            }
        });
        let custom = vec!["b", "a"];
        assert_eq!(pick_item_percent(&raw, "/data/items", &custom, "a"), Some(50.0));
        // override 都不中 → fallback 默认
        let custom = vec!["c", "d"];
        assert_eq!(pick_item_percent(&raw, "/data/items", &custom, "a"), Some(10.0));
        // 全部不中
        let custom = vec!["x", "y"];
        assert_eq!(pick_item_percent(&raw, "/data/items", &custom, "z"), None);
    }

    // ── display_mode 过滤 ──

    /// 构造一个测试用的 3 行 snapshot（套餐 + 补偿 + 总额度，套餐带 resets_at）
    fn snap_with_3_rows() -> ProviderSnapshot {
        ProviderSnapshot {
            provider: Provider::Xiaomimimo,
            success: true,
            rows: vec![
                QuotaRow {
                    label: "套餐".to_string(),
                    utilization: Some(13.0),
                    resets_at: Some(1785024000000),  // 2026-06-28 07:20 UTC
                    unit: Some("%".to_string()),
                    ..Default::default()
                },
                QuotaRow {
                    label: "补偿".to_string(),
                    utilization: Some(100.0),
                    unit: Some("%".to_string()),
                    ..Default::default()
                },
                QuotaRow {
                    label: "总额度".to_string(),
                    utilization: Some(42.0),
                    unit: Some("%".to_string()),
                    ..Default::default()
                },
            ],
            error: None,
            error_kind: None,
            fetched_at: Some(0),
            raw: None,
            is_healthy: true,
            source_id: Some("xiaomimimo".to_string()),
            source_display_name: Some("Xiaomi MiMo".to_string()),
            plan_name: None,
        }
    }

    #[test]
    fn display_mode_all_keeps_all_rows() {
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::All);
        assert_eq!(out.rows.len(), 3);
    }

    #[test]
    fn display_mode_plan_only_keeps_only_plan() {
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::PlanOnly);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].label, "套餐");
        assert!((out.rows[0].utilization.unwrap() - 13.0).abs() < 0.001);
        // 套餐 resets_at 保留
        assert_eq!(out.rows[0].resets_at, Some(1785024000000));
    }

    #[test]
    fn display_mode_total_only_keeps_only_total_with_plan_resets_at() {
        // TotalOnly 模式：总额度本来没 resets_at → 借套餐的月度重置时间
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::TotalOnly);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].label, "总额度");
        assert!((out.rows[0].utilization.unwrap() - 42.0).abs() < 0.001);
        // ★ 关键：resets_at 借过来了
        assert_eq!(out.rows[0].resets_at, Some(1785024000000));
    }

    #[test]
    fn display_mode_total_only_no_plan_resets_at_stays_none() {
        // 极端：所有行都没 resets_at（detail 缺失）→ 总额度这行也别伪造
        let mut snap = snap_with_3_rows();
        snap.rows[0].resets_at = None;  // 套餐也没
        let out = apply_display_mode(snap, XiaomiDisplayMode::TotalOnly);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].resets_at, None,
            "套餐无 resets_at → 总额度也保持 None（不编造）");
    }

    #[test]
    fn display_mode_preserves_other_fields() {
        // 过滤不能改 snap 的其他字段（provider / source_id / plan_name / error 等）
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::PlanOnly);
        assert_eq!(out.provider, Provider::Xiaomimimo);
        assert_eq!(out.source_id.as_deref(), Some("xiaomimimo"));
        assert!(out.is_healthy);
    }

    #[test]
    fn display_mode_plan_only_with_no_plan_row() {
        // 极端：套餐缺失（schema 变了）→ 留个空 snapshot（success=true 但 0 行）
        let mut snap = snap_with_3_rows();
        snap.rows.retain(|r| r.label != "套餐");
        let out = apply_display_mode(snap, XiaomiDisplayMode::PlanOnly);
        assert_eq!(out.rows.len(), 0);
        // 仍然算 success（parse 没报错，filter 不会改 success 标志）
        assert!(out.success);
    }
}
