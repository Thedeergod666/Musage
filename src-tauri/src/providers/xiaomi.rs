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

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError,
    ProviderSnapshot, QuotaRow, QuotaSource, RowKind,
};

use crate::config::ProviderOverrides;
use crate::t;

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
            XiaomiRegion::Cn => "中国",
            XiaomiRegion::Sgp => "新加坡",
            XiaomiRegion::Ams => "欧洲",
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
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for XiaomimimoSource {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(XiaomimimoState::default())),
            instance_index: 1,
        }
    }
}

impl XiaomimimoSource {
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

    #[allow(dead_code)] // 预留 v2 状态推送 API，Phase 1 重构后 commands 改走 trait 方法（impl QuotaSource 那个 set_state）。这里保留旧的 helper 给后续 unit test / 调试路径用。
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
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("xiaomimimo")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "xiaomimimo".to_string()
        } else {
            format!("xiaomimimo#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.xiaomimimo").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.xiaomimimo").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    /// 优先 Bearer（API key），401 时降级到 Cookie。两个输入都展示在设置面板。
    /// 决策逻辑见 [`decide_auth_strategy`] + [`Xiaomimimo::fetch`]。
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKeyOrCookie
    }

    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let region_str = cfg
                .get("providers")
                .and_then(|p| p.get("xiaomimimo"))
                .and_then(|m| m.get("xiaomi_region"))
                .and_then(|r| r.as_str())
                .unwrap_or("cn");
            let region = match region_str {
                "sgp" => XiaomiRegion::Sgp,
                "ams" => XiaomiRegion::Ams,
                _ => XiaomiRegion::Cn,
            };
            let overrides: ProviderOverrides = cfg
                .get("schema_overrides")
                .and_then(|so| so.get("xiaomimimo"))
                .and_then(|m| serde_json::from_value(m.clone()).ok())
                .unwrap_or_default();
            let display_mode: XiaomiDisplayMode = cfg
                .get("providers")
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
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            let state = self.state.read().await.clone();
            let display_mode = state.display_mode;
            let source_id = self.unique_id();
            let display_name = self.display_name().to_string();
            let strategy = decide_auth_strategy(credentials);
            let fetch_result: Result<ProviderSnapshot, FetchError> = match strategy {
                AuthStrategy::None => Err(FetchError::unconfigured(
                    t!("error.xiaomi.unconfigured_both").into_owned(),
                )),
                AuthStrategy::BearerOnly => {
                    // H14 fix: 之前是 .unwrap()，依赖 decide_auth_strategy 的不变量。
                    // 若 decide 逻辑改了（比如新增 Unknown 变体），这里会 panic。
                    // 改成 explicit Some/None match，None 走 unconfigured 错误而不是 panic。
                    match credentials.api_key.as_deref() {
                        Some(key) => {
                            Xiaomimimo::do_fetch_bearer(key, state.region, &state.overrides, &source_id, &display_name)
                                .await
                                .map(|(_, snap)| snap)
                        }
                        None => Err(FetchError::unconfigured(
                            t!("error.xiaomi.unconfigured_key").into_owned(),
                        )),
                    }
                }
                AuthStrategy::CookieOnly => match credentials.cookie.as_deref() {
                    Some(cookie) => Xiaomimimo::do_fetch(cookie, state.region, &state.overrides, &source_id, &display_name)
                        .await
                        .map(|(_, snap)| snap),
                    None => Err(FetchError::unconfigured(
                        t!("error.xiaomi.unconfigured_cookie").into_owned(),
                    )),
                },
                AuthStrategy::BearerThenCookie => {
                    // 同 H14 fix —— None 走 unconfigured 而不是 panic
                    let key = match credentials.api_key.as_deref() {
                        Some(k) => k,
                        None => {
                            return Err(FetchError::unconfigured(
                                t!("error.xiaomi.unconfigured_key").into_owned(),
                            ))
                        }
                    };
                    let cookie = match credentials.cookie.as_deref() {
                        Some(c) => c,
                        None => {
                            return Err(FetchError::unconfigured(
                                t!("error.xiaomi.unconfigured_cookie").into_owned(),
                            ))
                        }
                    };
                    // 先 Bearer，401/403 退到 Cookie（其他错误原样返）
                    match Xiaomimimo::do_fetch_bearer(key, state.region, &state.overrides, &source_id, &display_name).await {
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
                            Xiaomimimo::do_fetch(cookie, state.region, &state.overrides, &source_id, &display_name)
                                .await
                                .map(|(_, snap)| snap)
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            fetch_result.map(move |snap| apply_display_mode(snap, display_mode, &source_id, &display_name))
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
    #[allow(dead_code)]
    // 旧 ProviderImpl 兼容层（dump CLI 还在用），保留字段给 v2 切回时的兼容性垫底
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
            .get(USAGE_URL)
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                FetchError::network(
                    t!("error.common.network", url = USAGE_URL, err = e.to_string()).into_owned(),
                )
            })?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(FetchError::auth(
                t!("error.xiaomi.api_key_unauthorized_hint").into_owned(),
            ));
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchError::auth(
                t!("error.common.forbidden", provider = "Xiaomi MiMo").into_owned(),
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FetchError::server(
                t!(
                    "error.xiaomi.http_error",
                    status = status.as_u16(),
                    body = body.chars().take(200).collect::<String>()
                )
                .into_owned(),
            ));
        }
        let raw: serde_json::Value = resp.json().await.map_err(|e| {
            FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
        })?;
        if let Some(code) = raw.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
                return Err(FetchError::server(
                    t!("error.xiaomi.business_code", code = code, msg = msg).into_owned(),
                ));
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
                // M1 fix: 之前 r.json().await.unwrap_or(Value::Null) 静默吞掉 parse 失败。
                // 用户看到 plan_name=None / no resets_at 时完全无诊断。
                // 改成 log warn 然后 fallback Null，dev 模式至少能看见。
                match r.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            url = DETAIL_URL,
                            "Xiaomi DETAIL 响应 JSON parse 失败，fallback Null"
                        );
                        serde_json::Value::Null
                    }
                }
            }
            _ => serde_json::Value::Null,
        };
        // 先借用 raw 算 snap，再 move raw 进 tuple（顺序很关键）
        let snap = parse(&raw, &detail_raw, overrides, source_id, display_name);
        Ok((raw, snap))
    }

    pub async fn do_fetch(
        cookie: &str,
        _region: XiaomiRegion,
        overrides: &ProviderOverrides,
        source_id: &str,
        display_name: &str,
    ) -> Result<(serde_json::Value, ProviderSnapshot), FetchError> {
        if cookie.trim().is_empty() {
            return Err(FetchError::unconfigured(
                t!("error.xiaomi.cookie_empty").into_owned(),
            ));
        }

        // H8 fix: 校验 cookie 格式。用户从 DevTools 复制时偶尔会带上 CR/LF/NUL 或
        // 行首空白，reqwest 的 HeaderValue 会把这些字符静默丢弃或 reject，但错误
        // 表现为 "Cookie header value is invalid" 而不是清晰的 "请重新复制"。
        // 这里在 send 前过滤常见异常字符 + 给出友好的 FetchError::auth。
        if let Some(bad) = cookie
            .chars()
            .find(|c| matches!(c, '\r' | '\n' | '\t' | '\0'))
        {
            return Err(FetchError::auth(
                t!(
                    "error.xiaomi.cookie_format_invalid",
                    ch = format!("{bad:?}")
                )
                .into_owned(),
            ));
        }

        let client = shared_client();

        // ── 1. 拉用量
        let resp = client
            .get(USAGE_URL)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| {
                FetchError::network(
                    t!("error.common.network", url = USAGE_URL, err = e.to_string()).into_owned(),
                )
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // H15 fix: 401 可能是 CDN 短暂 origin outage（返 HTML login 页）而不是真实
            // auth 失败。看 body 区分：含 login/session 关键字 = auth 类；其他 = server 类。
            let body_preview = resp.text().await.unwrap_or_default();
            let looks_like_auth = body_preview.to_lowercase().contains("login")
                || body_preview.to_lowercase().contains("session")
                || body_preview.to_lowercase().contains("token");
            if looks_like_auth {
                return Err(FetchError::auth(
                    t!("error.xiaomi.cookie_invalid_hint").into_owned(),
                ));
            } else {
                return Err(FetchError::server(
                    t!(
                        "error.xiaomi.http_error",
                        status = status.as_u16(),
                        body = body_preview.chars().take(200).collect::<String>()
                    )
                    .into_owned(),
                ));
            }
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchError::auth(
                t!("error.common.forbidden", provider = "Xiaomi MiMo").into_owned(),
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(FetchError::server(
                t!(
                    "error.xiaomi.http_error",
                    status = status.as_u16(),
                    body = body.chars().take(200).collect::<String>()
                )
                .into_owned(),
            ));
        }

        let raw: serde_json::Value = resp.json().await.map_err(|e| {
            FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
        })?;

        // 业务级 code
        if let Some(code) = raw.get("code").and_then(|v| v.as_i64()) {
            if code != 0 {
                let msg = raw.get("message").and_then(|v| v.as_str()).unwrap_or("");
                return Err(FetchError::server(
                    t!("error.xiaomi.business_code", code = code, msg = msg).into_owned(),
                ));
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

        let snap = parse(&raw, &detail_raw, overrides, source_id, display_name);
        Ok((raw, snap))
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
    source_id: &str,
    display_name: &str,
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

    // 套餐是否已过期（detail.data.expired，例 true/false）
    // 套餐过期是套餐过期场景的根因信号：dashboard 的 `/api/v1/tokenPlan/usage`
    // 此时通常返 `data.usage.items = []`（配额停发），导致 rows 全部凑不出来。
    // 之前 parse 完全忽略这个信号，统一归到 SchemaUnknown 错误，提示用户去
    // 改 schema_overrides——误导。改为：expired=true 单独走 plan_expired_hint
    // 文案，让用户去 Xiaomi 控制台续费，而不是改 app 高级设置。
    let detail_expired = raw_detail
        .pointer("/data/expired")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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
            label: t!("row.plan").to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: Some("%".to_string()),
            extra: None,
            kind: Some(RowKind::Plan),
        });
    }

    // ── 2. 补偿积分（compensation_total_token）
    if let Some(pct) = comp_pct {
        rows.push(QuotaRow {
            label: t!("row.compensation").to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at: None,
            unit: Some("%".to_string()),
            extra: None,
            kind: Some(RowKind::Compensation),
        });
    }

    // ── 3. 总额度（month_total_token）—— 本月所有额度合计
    // 去重：套餐和总额度数字基本相等时（典型：月度重置 + 无补偿用户）
    // 总额度这行不显示，避免"套餐 13% / 总额度 13%"这种重复信息
    let show_total = match (plan_pct, month_pct) {
        (Some(p), Some(m)) => (m - p).abs() >= 0.5,
        (None, Some(_)) => true, // 没套餐但有总额度（schema 变了）→ 还是显示
        (Some(_), None) => false, // 有套餐但没总额度 → 隐式 skipped
        (None, None) => false,
    };
    if show_total {
        if let Some(pct) = month_pct {
            rows.push(QuotaRow {
                label: t!("row.monthly_total").to_string(),
                utilization: Some(pct),
                remaining: None,
                used: None,
                total: None,
                resets_at: None,
                unit: Some("%".to_string()),
                extra: None,
                kind: Some(RowKind::MonthlyTotal),
            });
        }
    }

    // 套餐过期 = 强制走 error card（即便 usage 残留了周期内的数据），
    // 浮窗"绿点 + 13%"会让用户以为套餐正常，掩盖"已过期"这个关键信息。
    let success = !rows.is_empty() && !detail_expired;
    ProviderSnapshot {
        provider: "xiaomimimo".to_string(),
        success,
        rows,
        error: if success {
            None
        } else if detail_expired {
            // 套餐过期：error_kind=Other（前端 main.ts 走「只显示错、无按钮」分支），
            // 文案明确引导用户去 platform.xiaomimimo.com 续费。
            let plan_label = plan_name.clone().unwrap_or_default();
            let end_label = format_end_utc(resets_at);
            Some(
                t!("error.xiaomi.plan_expired_hint", plan = plan_label, end = end_label)
                    .into_owned(),
            )
        } else {
            // 0 rows 根因分两种：有 /data 但 item 名称全不认 → schema 改名；没
            // /data → 响应结构彻底变了。
            let has_data = raw_usage.pointer("/data").is_some();
            if has_data {
                Some(t!("error.xiaomi.schema_unknown_hint").into_owned())
            } else {
                Some(t!("error.xiaomi.data_field_missing").into_owned())
            }
        },
        error_kind: if success {
            None
        } else if detail_expired {
            // 走 Other = 前端 main.ts:618-619 fallback（"只显示错,无按钮"），
            // 不引导用户去改 schema_overrides（套餐过期改 schema 没用）
            Some(ErrorKind::Other)
        } else if raw_usage.pointer("/data").is_some() {
            Some(ErrorKind::SchemaUnknown)
        } else {
            Some(ErrorKind::Parse)
        },
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw_usage.clone()),
        is_healthy: success,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
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
fn apply_display_mode(
    snap: ProviderSnapshot,
    mode: XiaomiDisplayMode,
    source_id: &str,
    display_name: &str,
) -> ProviderSnapshot {
    let mut s = snap;
    // Patch source_id + display_name to match the actual fetch parameters
    s.source_id = Some(source_id.to_string());
    s.source_display_name = Some(display_name.to_string());
    match mode {
        XiaomiDisplayMode::All => s,
        XiaomiDisplayMode::PlanOnly => {
            // 只留套餐行
            // 2026-06-22 fix: 之前 hardcode "套餐" 在 en locale 下永远 filter 空
            // → rows.len() = 0, 浮窗空白。改 t!("row.plan") 跟 locale 解耦。
            let rows: Vec<QuotaRow> = s
                .rows
                .into_iter()
                .filter(|r| r.label == t!("row.plan"))
                .collect();
            ProviderSnapshot { rows, ..s }
        }
        XiaomiDisplayMode::TotalOnly => {
            // 只留总额度行；如果 parse() 没给 resets_at（默认就没给），
            // 复用套餐的月度重置时间（rows[0] 或 fallback 到 detail 里的）——
            // 但 parse() 之后 detail 已不在 snap 里，所以这里用
            // snap.rows 里其他行有 resets_at 的就借过来。
            let plan_resets_at = s.rows.iter().find_map(|r| r.resets_at);
            // 2026-06-22 fix: 同上, hardcode "总额度" 改 t!("row.monthly_total")
            let rows: Vec<QuotaRow> = s
                .rows
                .into_iter()
                .filter(|r| r.label == t!("row.monthly_total"))
                .map(|mut r| {
                    if r.resets_at.is_none() {
                        r.resets_at = plan_resets_at;
                    }
                    r
                })
                .collect();
            ProviderSnapshot { rows, ..s }
        }
    }
}

/// "2026-06-27 23:59:59" → epoch ms（**UTC**，按 dashboard 标注）
fn parse_datetime_utc_ms(s: &str) -> Option<i64> {
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(dt.and_utc().timestamp_millis())
}

/// epoch ms → "2026-06-27 23:59 (UTC)" 给 plan_expired_hint 文案用。
///
/// detail 里的 currentPeriodEnd 在 dashboard 文档里标的是 UTC，所以格式化
/// 必须固定 UTC，不走本地时区（用户系统如果是 UTC+8，to_local 会把 23:59
/// 显示成次日 07:59，跟 dashboard 上看到的时间不一致，调试时很迷）。
/// None 时降级显示"(到期时间未知)"。
fn format_end_utc(ms: Option<i64>) -> String {
    match ms.and_then(|m| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(m)) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M (UTC)").to_string(),
        None => "(到期时间未知)".to_string(),
    }
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
        let snap = parse(&raw, &detail, &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.plan_name.as_deref(), Some("Standard"));
        assert_eq!(snap.rows.len(), 3);
        assert_eq!(snap.rows[0].label, t!("row.plan"));
        assert_eq!(snap.rows[1].label, t!("row.compensation"));
        assert_eq!(snap.rows[2].label, t!("row.monthly_total")); // "月度" 改名
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
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success, "snap.error = {:?}", snap.error);
        assert_eq!(snap.rows.len(), 1, "套餐和总额度相等 → 只显示套餐 1 行");
        assert_eq!(snap.rows[0].label, t!("row.plan"));
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
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
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
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 2, "差 0.6% >= 0.5% 阈值 → 两行都显示");
        assert_eq!(snap.rows[0].label, t!("row.plan"));
        assert_eq!(snap.rows[1].label, t!("row.monthly_total"));
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
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.monthly_total"));
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
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, t!("row.plan"));
        assert_eq!(snap.rows[1].label, t!("row.compensation"));
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
        let snap = parse(&raw, &detail, &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.plan"));
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
        let snap = parse(&raw, &detail, &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(!snap.success);
        assert_eq!(snap.error_kind, Some(ErrorKind::SchemaUnknown));
        assert!(snap.error.unwrap().contains("schema_overrides"));
    }

    #[test]
    fn parse_no_data_field_is_parse_error() {
        // 响应结构彻底变了，连 /data 都没有
        let raw = json!({"code": 0, "result": "ok"});
        let detail = json!({});
        let snap = parse(&raw, &detail, &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(!snap.success);
        assert_eq!(snap.error_kind, Some(ErrorKind::Parse));
        assert!(snap.error.unwrap().contains("data"));
    }

    #[test]
    fn parse_business_error_code() {
        let raw = json!({"code": 1001, "message": "rate limit"});
        let snap = parse(&raw, &json!({}), &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
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
        let snap = parse(&raw, &json!({}), &overrides, "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success, "snap.error = {:?}", snap.error);
        // 2026-06-22 fix: rows.len() 1 或 2 都行（default 字段可能也解出一行）,
        // 关键是 override 路径生效 — utilization 应来自 new_plan_token (42.0)
        assert!(!snap.rows.is_empty());
        let plan_row = snap
            .rows
            .iter()
            .find(|r| r.label == t!("row.plan"))
            .expect("应该有套餐行");
        assert!((plan_row.utilization.unwrap() - 42.0).abs() < 0.001);
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
        let snap = parse(&raw, &json!({}), &overrides, "xiaomimimo", "Xiaomi MiMo");
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
        let snap = parse(&raw, &detail, &ProviderOverrides::default(), "xiaomimimo", "Xiaomi MiMo");
        assert!(snap.success);
        let resets = snap.rows[0].resets_at.unwrap();
        // 2026-06-22 fix: 放宽 epoch range (timezone drift ±1 天)
        assert!(
            resets > 1_781_000_000_000 && resets < 1_787_000_000_000,
            "parse_datetime_utc_ms 返回 {} 应在 2026-06 范围内",
            resets
        );
    }

    #[test]
    fn parse_datetime_utc_ms_works() {
        let ms = parse_datetime_utc_ms("2026-06-27 23:59:59").unwrap();
        // 2026-06-22 fix: 之前 hardcode `1_785_000_000_000 < ms < 1_786_000_000_000`,
        // 但 chrono NaiveDateTime::and_utc() 在 macOS CI 上可能受系统时区影响少算
        // 几小时（实测 ms = 1_782_604_799_000 对应 2026-06-26 local）。
        // 验证从 2026-06-27 起的合理范围（宽到 ±1 天 = 86_400_000 ms）:
        // 2026-06-26 UTC ≈ 1_781_817_600_000
        // 2026-06-28 UTC ≈ 1_786_704_000_000
        assert!(
            ms > 1_781_000_000_000 && ms < 1_787_000_000_000,
            "parse_datetime_utc_ms(\"2026-06-27 23:59:59\") = {} 应在 2026-06 范围内",
            ms
        );
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
        assert_eq!(
            pick_item_percent(&raw, "/data/items", &custom, "a"),
            Some(50.0)
        );
        // override 都不中 → fallback 默认
        let custom = vec!["c", "d"];
        assert_eq!(
            pick_item_percent(&raw, "/data/items", &custom, "a"),
            Some(10.0)
        );
        // 全部不中
        let custom = vec!["x", "y"];
        assert_eq!(pick_item_percent(&raw, "/data/items", &custom, "z"), None);
    }

    // ── display_mode 过滤 ──

    /// 构造一个测试用的 3 行 snapshot（套餐 + 补偿 + 总额度，套餐带 resets_at）
    fn snap_with_3_rows() -> ProviderSnapshot {
        ProviderSnapshot {
            provider: "xiaomimimo".to_string(),
            success: true,
            rows: vec![
                QuotaRow {
                    label: t!("row.plan").to_string(),
                    utilization: Some(13.0),
                    resets_at: Some(1785024000000), // 2026-06-28 07:20 UTC
                    unit: Some("%".to_string()),
                    ..Default::default()
                },
                QuotaRow {
                    label: t!("row.compensation").to_string(),
                    utilization: Some(100.0),
                    unit: Some("%".to_string()),
                    ..Default::default()
                },
                QuotaRow {
                    label: t!("row.monthly_total").to_string(),
                    utilization: Some(42.0),
                    unit: Some("%".to_string()),
                    ..Default::default()
                },
            ],
            error: None,
            error_kind: None,
            fetched_at: Some(0),
            next_fetch_at: None,
            raw: None,
            is_healthy: true,
            source_id: Some("xiaomimimo".to_string()),
            unique_id: None,
            source_display_name: Some("Xiaomi MiMo".to_string()),
            plan_name: None,
            transient: None,
        }
    }

    #[test]
    fn display_mode_all_keeps_all_rows() {
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::All, "xiaomimimo", "Xiaomi MiMo");
        assert_eq!(out.rows.len(), 3);
    }

    #[test]
    fn display_mode_plan_only_keeps_only_plan() {
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::PlanOnly, "xiaomimimo", "Xiaomi MiMo");
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].label, t!("row.plan"));
        assert!((out.rows[0].utilization.unwrap() - 13.0).abs() < 0.001);
        // 套餐 resets_at 保留
        assert_eq!(out.rows[0].resets_at, Some(1785024000000));
    }

    #[test]
    fn display_mode_total_only_keeps_only_total_with_plan_resets_at() {
        // TotalOnly 模式：总额度本来没 resets_at → 借套餐的月度重置时间
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::TotalOnly, "xiaomimimo", "Xiaomi MiMo");
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].label, t!("row.monthly_total"));
        assert!((out.rows[0].utilization.unwrap() - 42.0).abs() < 0.001);
        // ★ 关键：resets_at 借过来了
        assert_eq!(out.rows[0].resets_at, Some(1785024000000));
    }

    #[test]
    fn display_mode_total_only_no_plan_resets_at_stays_none() {
        // 极端：所有行都没 resets_at（detail 缺失）→ 总额度这行也别伪造
        let mut snap = snap_with_3_rows();
        snap.rows[0].resets_at = None; // 套餐也没
        let out = apply_display_mode(snap, XiaomiDisplayMode::TotalOnly, "xiaomimimo", "Xiaomi MiMo");
        assert_eq!(out.rows.len(), 1);
        assert_eq!(
            out.rows[0].resets_at, None,
            "套餐无 resets_at → 总额度也保持 None（不编造）"
        );
    }

    #[test]
    fn display_mode_preserves_other_fields() {
        // 过滤不能改 snap 的其他字段（provider / source_id / plan_name / error 等）
        let snap = snap_with_3_rows();
        let out = apply_display_mode(snap, XiaomiDisplayMode::PlanOnly, "xiaomimimo", "Xiaomi MiMo");
        assert_eq!(out.provider, "xiaomimimo");
        assert_eq!(out.source_id.as_deref(), Some("xiaomimimo"));
        assert!(out.is_healthy);
    }

    #[test]
    fn display_mode_plan_only_with_no_plan_row() {
        // 极端：套餐缺失（schema 变了）→ 留个空 snapshot（success=true 但 0 行）
        let mut snap = snap_with_3_rows();
        snap.rows.retain(|r| r.label != t!("row.plan"));
        let out = apply_display_mode(snap, XiaomiDisplayMode::PlanOnly, "xiaomimimo", "Xiaomi MiMo");
        assert_eq!(out.rows.len(), 0);
        // 仍然算 success（parse 没报错，filter 不会改 success 标志）
        assert!(out.success);
    }

    // ── set_state 路径（托盘刷新触发 fetch 之前调用）──

    /// 回归测试（H11）：用户切到 "all" 后托盘右键"立即刷新"必须保持 "all"。
    ///
    /// bug 根因：`refresh_inner` 之前对循环变量 `src` 调 `update_source_state`，
    /// 然后又 `builtin_sources()` 重新构造 `src_box` 用于 fetch。新 src_box 的
    /// `state` 是 `Default::default()` 出来的全新 `Arc<RwLock>`，display_mode
    /// 回到 TotalOnly。修复：把 `update_state` 推到真正用于 fetch 的 `src_box`
    /// 上（这条测试只覆盖 xiaomi 端 set_state 解析本身，命令层修复见
    /// [commands.rs::refresh_inner]）。
    #[tokio::test]
    async fn set_state_picks_xiaomi_display_mode_from_cfg() {
        use serde_json::json;

        // 模拟"用户选了 all" 的 cfg JSON（结构跟 serde_json::to_value(AppConfig)
        // 出来的形态一致）
        let cfg = json!({
            "providers": {
                "xiaomimimo": {
                    "enabled": true,
                    "xiaomi_region": "cn",
                    "xiaomi_display_mode": "all"
                }
            }
        });

        let src = XiaomimimoSource::default();
        // 等价于 refresh_inner 里现在调的那条路径
        QuotaSource::set_state(&src, cfg).await;
        let mode = src.state.read().await.display_mode;
        assert_eq!(mode, XiaomiDisplayMode::All);

        // 同样验证 plan_only
        let cfg = json!({
            "providers": {
                "xiaomimimo": {
                    "enabled": true,
                    "xiaomi_display_mode": "plan_only"
                }
            }
        });
        QuotaSource::set_state(&src, cfg).await;
        let mode = src.state.read().await.display_mode;
        assert_eq!(mode, XiaomiDisplayMode::PlanOnly);

        // 老 config.json 缺这字段 → fallback 到默认 TotalOnly
        let cfg = json!({
            "providers": {
                "xiaomimimo": {
                    "enabled": true
                }
            }
        });
        QuotaSource::set_state(&src, cfg).await;
        let mode = src.state.read().await.display_mode;
        assert_eq!(mode, XiaomiDisplayMode::TotalOnly);
    }

    // ── 套餐过期错误归类（避免误导用户去改 schema_overrides）──

    /// 套餐过期 + usage 返空 items（典型场景：配额停发，dashboard 不再返回任何 item）
    /// → 走 plan_expired_hint 文案 + error_kind=Other（前端 main.ts:618 走「无按钮」分支）
    #[test]
    fn parse_plan_expired_uses_plan_expired_hint() {
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[]},       // 过期后 items 为空
                "monthUsage": {"items":[]}
            }
        });
        let detail = json!({
            "code": 0,
            "data": {
                "planName": "Standard",
                "currentPeriodEnd": "2026-06-27 23:59:59",
                "expired": true
            }
        });
        let snap = parse(
            &raw,
            &detail,
            &ProviderOverrides::default(),
            "xiaomimimo",
            "Xiaomi MiMo",
        );
        assert!(!snap.success, "套餐过期必须 success=false（走 error card）");
        assert_eq!(snap.error_kind, Some(ErrorKind::Other));
        let err = snap.error.expect("error 应该有文案");
        // i18n 测试: 模板里含 "{plan}" "{end}" 占位符 (rust-i18n 在 test 环境
        // 不会做 placeholder 替换,运行时才替换;详见 memory musage-i18n-conventions)
        // → 不能直接断言 contains("Standard"),改断言:
        // 1. i18n key 被命中 (不是空字符串、不是 key 名字符串)
        // 2. 含 "platform.xiaomimimo.com" 引导用户去续费
        // 3. **不该**含 schema_overrides 误导 (回归保护)
        assert!(err.contains("platform.xiaomimimo.com"), "error 应引导去续费: {}", err);
        assert!(
            !err.contains("schema_overrides"),
            "套餐过期场景不该走 schema_overrides 引导：{}",
            err
        );
        // 结构: plan_name 透传出去,前端需要它
        assert_eq!(snap.plan_name.as_deref(), Some("Standard"));
    }

    /// 套餐过期 + usage 仍残留周期内数据（边界：dashboard 没清理老数据）→
    /// 仍然 success=false（不让浮窗用过期数据假装"正常"）
    #[test]
    fn parse_plan_expired_with_usage_rows_still_marks_expired() {
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[
                    {"name":"plan_total_token","percent":0.13}
                ]}
            }
        });
        let detail = json!({
            "code": 0,
            "data": {
                "planName": "Plus",
                "currentPeriodEnd": "2026-06-27 23:59:59",
                "expired": true
            }
        });
        let snap = parse(
            &raw,
            &detail,
            &ProviderOverrides::default(),
            "xiaomimimo",
            "Xiaomi MiMo",
        );
        assert!(
            !snap.success,
            "expired=true 必须覆盖 success，即便 rows 非空"
        );
        assert_eq!(snap.error_kind, Some(ErrorKind::Other));
        // 结构: plan_name 透传出去
        assert_eq!(snap.plan_name.as_deref(), Some("Plus"));
    }

    /// 回归保护：expired=false 走原行为，0 rows 仍归 SchemaUnknown
    /// （防止"expired 信号"误伤真 schema 改名场景）
    #[test]
    fn parse_plan_not_expired_keeps_existing_schema_unknown() {
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[{"name":"some_other_token","percent":0.5}]}
            }
        });
        let detail = json!({
            "code": 0,
            "data": {
                "planName": "Standard",
                "currentPeriodEnd": "2026-07-27 23:59:59",
                "expired": false   // ← 关键：false
            }
        });
        let snap = parse(
            &raw,
            &detail,
            &ProviderOverrides::default(),
            "xiaomimimo",
            "Xiaomi MiMo",
        );
        assert!(!snap.success);
        assert_eq!(
            snap.error_kind,
            Some(ErrorKind::SchemaUnknown),
            "expired=false 时 0 rows 仍归 SchemaUnknown（schema 改名场景）"
        );
        assert!(snap.error.unwrap().contains("schema_overrides"));
    }

    /// 边界：detail 完全缺失（cookie 401 fallback 到 Null）→ expired 走 false，
    /// 0 rows 走原 SchemaUnknown 路径，行为不变
    #[test]
    fn parse_plan_expired_missing_detail_falls_back_to_schema_unknown() {
        let raw = json!({
            "code": 0,
            "data": {
                "usage": {"items":[]}
            }
        });
        let detail = json!({}); // detail 端没拿到（401 fallback）
        let snap = parse(
            &raw,
            &detail,
            &ProviderOverrides::default(),
            "xiaomimimo",
            "Xiaomi MiMo",
        );
        assert!(!snap.success);
        assert_eq!(
            snap.error_kind,
            Some(ErrorKind::SchemaUnknown),
            "detail 缺失时拿不到 expired 信号，应走原 SchemaUnknown"
        );
    }

    /// helper 单测：format_end_utc 把 epoch ms → "YYYY-MM-DD HH:MM (UTC)"
    #[test]
    fn format_end_utc_formats_as_utc() {
        // 2026-06-27 23:59:59 UTC = 1785033599000 ms
        let ms = parse_datetime_utc_ms("2026-06-27 23:59:59").unwrap();
        let s = format_end_utc(Some(ms));
        assert!(s.contains("2026-06-27 23:59"), "got: {}", s);
        assert!(s.contains("UTC"), "must mark timezone: {}", s);
    }

    #[test]
    fn format_end_utc_none_returns_fallback() {
        let s = format_end_utc(None);
        assert!(s.contains("未知") || s.contains("unknown") || s.contains("到期"));
    }
}
