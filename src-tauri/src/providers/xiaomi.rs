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

// ── QuotaSource 实现（Phase 1）────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct XiaomimimoState {
    pub region: XiaomiRegion,
    pub overrides: ProviderOverrides,
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
    pub async fn set_state(&self, region: XiaomiRegion, overrides: ProviderOverrides) {
        let mut s = self.state.write().await;
        s.region = region;
        s.overrides = overrides;
    }
}

impl QuotaSource for XiaomimimoSource {
    fn id(&self) -> &'static str { "xiaomimimo" }
    fn display_name(&self) -> &'static str { "Xiaomi MiMo" }
    fn auth_kind(&self) -> AuthKind { AuthKind::Cookie }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let cookie = credentials.cookie.as_deref().unwrap_or("").trim();
            if cookie.is_empty() {
                return Err(FetchError::unconfigured("未配置 Dashboard cookie（设置面板填入）"));
            }
            let state = self.state.read().await.clone();
            do_fetch(cookie, state.region, &state.overrides).await
        })
    }
}

// ── 旧 ProviderImpl 兼容（dump CLI 还在用）────────────────────────

#[derive(Debug, Default)]
pub struct Xiaomimimo {
    pub region: XiaomiRegion,
}

impl Xiaomimimo {
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
    _overrides: &ProviderOverrides,
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

    let mut rows = Vec::new();

    // ── 1. 套餐（plan_total_token）—— 主指标，dashboard 显示的就是它
    if let Some(pct) = get_item_percent(raw_usage, "/data/usage/items", "plan_total_token") {
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
    if let Some(pct) = get_item_percent(raw_usage, "/data/usage/items", "compensation_total_token") {
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

    // ── 3. 月度累计（month_total_token）—— 当月合计
    if let Some(pct) = get_item_percent(raw_usage, "/data/monthUsage/items", "month_total_token") {
        rows.push(QuotaRow {
            label: "月度".to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
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
            Some("响应里找不到 plan_total_token / compensation_total_token / month_total_token 任何一项".to_string())
        },
        error_kind: if success { None } else { Some(ErrorKind::Parse) },
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

/// "2026-06-27 23:59:59" → epoch ms（**UTC**，按 dashboard 标注）
fn parse_datetime_utc_ms(s: &str) -> Option<i64> {
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(dt.and_utc().timestamp_millis())
}
