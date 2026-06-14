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
use crate::config::{FieldTriple, ProviderOverrides};

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
            let mut s = self.state.write().await;
            s.region = region;
            s.overrides = overrides;
        })
    }

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
            Xiaomimimo::do_fetch(cookie, state.region, &state.overrides).await.map(|(_, snap)| snap)
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

    // ── 1. 套餐（plan_total_token）—— 主指标，dashboard 显示的就是它
    if let Some(pct) =
        pick_item_percent(raw_usage, "/data/usage/items", &custom_names, "plan_total_token")
    {
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
    if let Some(pct) = pick_item_percent(
        raw_usage,
        "/data/usage/items",
        &custom_names,
        "compensation_total_token",
    ) {
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
    if let Some(pct) = pick_item_percent(
        raw_usage,
        "/data/monthUsage/items",
        &custom_names,
        "month_total_token",
    ) {
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
    use serde_json::json;

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
        assert_eq!(snap.rows[2].label, "月度");
        assert!((snap.rows[0].utilization.unwrap() - 6.0).abs() < 0.001);
        assert!((snap.rows[1].utilization.unwrap() - 5.0).abs() < 0.001);
        assert!((snap.rows[2].utilization.unwrap() - 30.0).abs() < 0.001);
        // 套餐行带 resets_at；补偿/月度不带
        assert!(snap.rows[0].resets_at.is_some());
        assert!(snap.rows[1].resets_at.is_none());
        assert!(snap.rows[2].resets_at.is_none());
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
}
