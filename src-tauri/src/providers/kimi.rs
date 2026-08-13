//! Kimi (Moonshot) For Coding 用量查询 + 「总套餐」月度共享池（v0.2.5）
//!
//! 端点：`GET https://api.kimi.com/coding/v1/usages`
//! 鉴权：`Authorization: Bearer <api_key>`
//!
//! ## 用途
//!
//! Kimi Coding Plan 是月之暗面（Moonshot AI）的编程套餐，跟 MiniMax 5h/周
//! 类似的滚动窗口设计。CCSwitch 已有 [同款实现](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/services/coding_plan.rs)
//! 可以参考（query_kimi + extract_reset_time 的容错处理）。
//!
//! ## 响应 schema
//!
//! ```json
//! {
//!   "limits": [
//!     {
//!       "detail": {
//!         "limit": 100,
//!         "remaining": 72,
//!         "resetTime": "2026-06-14T18:30:00.000Z"   // 也可能是 epoch 秒/毫秒
//!       }
//!     }
//!   ],
//!   "usage": {
//!     "limit": 1000,
//!     "remaining": 742,
//!     "resetTime": 1749840000                       // 数值（秒或毫秒）
//!   },
//!   "totalQuota": {}                                 // 总套餐月度池（目前恒空）
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 第一行（5h 滚动窗口）：`body.limits[].detail.{limit, remaining}`，kind = FiveHour
//! - 第二行（7 天滚动窗口）：`body.usage.{limit, remaining}`，kind = Weekly
//! - 浮窗左侧标签按 resets_at 动态显示窗口剩余（"5h" / "7d"），不显示 used/total
//! - `resetTime` 容错：字符串（ISO 8601）+ 数字（epoch 秒/毫秒自动识别）
//!
//! 字段名 / schema 参照 ccswitch；老套餐只回 `usage` 时只显示 1 行（自然降级）。
//!
//! ## 「总套餐」月度共享池（v0.2.5，FEATURE_OMNI）
//!
//! 2026-08 起 Kimi 网页端「我的额度」页新增**总使用量**进度条：所有会员
//! 功能（Kimi 对话 / Kimi Code / Kimi Work / PPT / 深度研究…）共享一个
//! 月度额度池，按 token 消耗。Kimi Code 的 5h/7d 限额独立于该池。
//!
//! **API key 拿不到总池**：响应里的 `authentication.scope` 锁死在
//! `FEATURE_CODING`（2026-08-04 实测 `?scope=FEATURE_OMNI` 参数被无视、
//! 调网页网关 401）。总池只通过 `www.kimi.com` 网页会话网关暴露：
//!
//! ```text
//! POST https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats
//! Authorization: Bearer <kimi-auth 会话 JWT>   （Cookie: kimi-auth=<同值>）
//! → { ratelimitCode5h, ratelimitCode7d,
//!     subscriptionBalance: { feature: "FEATURE_OMNI", type: "SUBSCRIPTION",
//!                            amountUsedRatio: 0.5548, kimiCodeUsedRatio: 0.2977,
//!                            expireTime: "2026-08-17T00:00:00Z" },
//!     boosterWallets: [...] }
//! ```
//! （端点 / schema 逆向自 CodexBar `KimiUsageFetcher.swift`，2026-08-04
//! 本机实测 200。）
//!
//! 集成策略（**hybrid enrich，增强失败绝不 fail 主快照**）：
//! 1. API 响应 `totalQuota` 防御性解析 —— 字段已在 schema 里，未来官方
//!    若填上（API key 直拿），直接消费不用再发版（猜测 schema 同 `usage`）
//! 2. 空则走网页会话：`kimi:cookie` 槽（WebView 登录预留）→ kimi-desktop
//!    本地 Cookies 库实时读（[`crate::kimi_desktop`]，零交互自动保鲜）
//! 3. 都没有 → 浮窗保持原样（只 5h + 7d 两行）
//!
//! 总套餐行：追加在 5h/7d **之后**（对齐火山方舟 5h → 7d → 月 窗口升序，
//! 2026-08-05 调整）。label「总套餐」+ utilization（`amountUsedRatio × 100`）
//! + resets_at（`expireTime`，前端 `extra.reset_period="monthly"` →「月重置」）。
//! `kimiCodeUsedRatio`（总池里 Kimi Code 消耗占比）塞进
//! `extra.kimi_code_used_ratio` → 前端在该行的 bar 和「月重置」之间
//! 渲染一行拆分小字「Kimi xx% · Code xx%」，Kimi 段 = 总 − Code
//! （API 无独立 Kimi Work 分项，官方黑段同样是"其余全部"）。
//! （2026-08-05 二轮：曾做官网同款双色堆叠条，黑段在深色玻璃上
//! 易读性太差，用户拍板改回普通单条 bar + 文本行。）

use std::borrow::Cow;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    json_body_limited, shared_client, text_body_limited, AuthKind, Credentials, ErrorKind,
    FetchError, ProviderSnapshot, QuotaRow, QuotaSource, RowKind,
};
use crate::kimi_desktop::KimiSessionInfo;
use crate::t;

const URL: &str = "https://api.kimi.com/coding/v1/usages";
/// 「总套餐」月度池查询端点（网页会话鉴权，见文件头注释）。
const STATS_URL: &str =
    "https://www.kimi.com/apiv2/kimi.gateway.membership.v2.MembershipService/GetSubscriptionStats";
/// www.kimi.com 网关风控未知，用已验证的浏览器 UA（CodexBar 同款，
/// 2026-08-04 本机实测 200），不用共享 client 的 Musage/x.y。
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct KimiSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for KimiSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl KimiSource {
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
}

impl QuotaSource for KimiSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("kimi")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "kimi".to_string()
        } else {
            format!("kimi#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.kimi").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.kimi").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKey
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Kimi 无 region/模式/overrides 概念，忽略
        Box::pin(async move {})
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
                    t!("error.provider.unconfigured_key", provider = "Kimi").into_owned(),
                ));
            }
            do_fetch(
                api_key,
                credentials.cookie.as_deref(),
                &self.unique_id(),
                self.display_name().as_ref(),
            )
            .await
        })
    }
}

async fn do_fetch(
    api_key: &str,
    cookie: Option<&str>,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    let resp = client
        .get(URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = URL, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    // H6 fix + L1 fix：429 显式 → RateLimited
    // helper 复用(L1):其它 provider 的 HTTP status → ErrorKind 分类统一走
    // `classify_http_status`,本文件保留对它的快速短路以便让具体 message
    // 走 rate_limited 模板(其它 status 走 http_error 模板),但 kind 计算
    // 复用 helper —— 加 402 Payment Required 时 11 处自动跟上。
    let kind = crate::providers::classify_http_status(status);
    if kind == ErrorKind::RateLimited {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "Kimi").into_owned(),
        ));
    }
    if kind == ErrorKind::AuthFailed {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "Kimi").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = text_body_limited(resp).await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "Kimi",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw = json_body_limited(resp).await?;

    let mut snap = parse(&raw, source_id, display_name)?;

    // ── 「总套餐」月度池增强（best-effort，失败只少一行，绝不 fail 主快照）──
    // 优先 API 响应自带 totalQuota（目前恒空，防御性预留）；空则走网页
    // 会话（kimi:cookie 槽 → kimi-desktop 本地 Cookies 库）。都没有 →
    // 浮窗保持原样（只 5h + 7d）。
    let total_row = match parse_total_quota(&raw) {
        Some(row) => Some(row),
        None => match resolve_session_token(cookie) {
            Some((token, info)) => match fetch_total_plan_row(&token, &info).await {
                Some((row, stats_raw)) => {
                    // stats 原始响应挂进 snapshot.raw，dump CLI 排查用
                    if let Some(r) = snap.raw.as_mut().and_then(|v| v.as_object_mut()) {
                        r.insert("subscription_stats_enrich".to_string(), stats_raw);
                    }
                    Some(row)
                }
                None => None,
            },
            None => None,
        },
    };
    if let Some(row) = total_row {
        // 追加到 5h/7d 之后（对齐火山方舟 5h → 7d → 月 的窗口升序；
        // 2026-08-05 用户反馈：插最前跟方舟视觉不一致）
        snap.rows.push(row);
    }

    Ok(snap)
}

/// 解析可用的网页会话 token：`kimi:cookie` 槽优先，kimi-desktop 本地
/// Cookies 库兜底。两侧都过 [`crate::kimi_desktop::validate_auth_token`]
/// 本地预检（过期 / 畸形 → 换下一源 / 降级）。
fn resolve_session_token(stored: Option<&str>) -> Option<(String, KimiSessionInfo)> {
    // 1) keys.json `kimi:cookie` 槽（WebView 一键登录写入的路径）
    if let Some(tok) = stored.map(str::trim).filter(|s| !s.is_empty()) {
        // 防御：用户手改 keys.json 时可能粘成 `kimi-auth=<jwt>` 整段形式
        //（cookie 栏习惯），剥掉前缀只留裸 JWT。
        let tok = tok.strip_prefix("kimi-auth=").unwrap_or(tok);
        if let Some(info) = crate::kimi_desktop::validate_auth_token(tok) {
            return Some((tok.to_string(), info));
        }
        tracing::debug!("[kimi] kimi:cookie 槽 token 过期/畸形 → 尝试 kimi-desktop 本地会话");
    }
    // 2) kimi-desktop 本地 Cookies 库（零交互，桌面端自己刷新会话 → 自动保鲜）
    let tok = crate::kimi_desktop::load_desktop_auth_token()?;
    let info = crate::kimi_desktop::validate_auth_token(&tok)?;
    Some((tok, info))
}

/// 解析 Kimi Coding usage 响应。
///
/// 解析失败时按 ROADMAP 策略返回 `Err(FetchError::Parse)`。
fn parse(raw: &Value, source_id: &str, display_name: &str) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut rows = Vec::new();

    // ── 5 小时窗口：从 limits[].detail 取 ──
    if let Some(limits) = raw.get("limits").and_then(|v| v.as_array()) {
        for limit_item in limits {
            let Some(detail) = limit_item.get("detail") else {
                continue;
            };
            let resets_at = extract_reset_ms(detail.get("resetTime"));
            if let Some(row) = build_window_row(
                detail,
                t!("row.five_hour").to_string(),
                RowKind::FiveHour,
                resets_at,
            ) {
                rows.push(row);
                break; // 只取第一条 5h 限额
            }
        }
    }

    // ── 周限额：从顶层 usage 取 ──
    // Kimi 的周窗固定显示 "7d"（跟 5h 形成对称 5h/7d），跟 MiniMax / GLM 区分
    if let Some(usage) = raw.get("usage") {
        let resets_at = extract_reset_ms(usage.get("resetTime"));
        if let Some(row) = build_window_row(
            usage,
            t!("row.weekly_7d").to_string(),
            RowKind::Weekly,
            resets_at,
        ) {
            rows.push(row);
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    Ok(ProviderSnapshot {
        // provider 字段: v0.2 删 enum 后从 Provider::Minimax 改成
        // "minimax" string 占位。前端走 source_id ("kimi") 路由,
        // 这个字段仅给老 JSON 反序列化兜底 (#[serde(default)] 让空 / 缺失
        // 字段不报错)
        provider: "kimi".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name: Some("Coding Plan".to_string()),
        transient: None,
    })
}

// ── 工具函数 ─────────────────────────────────────────────────────

/// 从一个 `{limit, remaining, used}` 对象构造窗口行（5h / 周）。
///
/// **回归修复（2026-07-17）**：5h 窗口达到 100% 上限时，Kimi API 会把
/// `remaining` 字段翻成 `0`、或干脆**省略**该字段（只回 `limit` / `used`）。
/// 旧逻辑严格要求 `limit` 与 `remaining` 同时 `Some` 才建行，导致 5h 达上限后
/// 整行 drop —— 浮窗里 5h 行消失（周限还在，因为周还没满）。跟 MiniMax 之前
/// 的 `status` 门控 bug（commit 7af0755）同源。
///
/// 新策略（对齐 ccswitch `query_kimi` 的 `unwrap_or` 容错）：
/// - `limit` 缺失 / <= 0 → 无法算百分比，返回 None（自然降级，非上限态）
/// - `remaining` 缺失 → 优先用显式 `used` 字段；再退化为 `0`（= 已用满 100%）
/// - `used` 优先取显式字段，否则用 `limit - remaining`
///
/// 这样只要拿到合法 `limit`，行就一定存在，哪怕 remaining/used 在上限态被省略。
fn build_window_row(
    obj: &Value,
    label: String,
    kind: RowKind,
    resets_at: Option<i64>,
) -> Option<QuotaRow> {
    let limit = parse_f64(obj.get("limit"))?;
    if limit <= 0.0 {
        return None;
    }
    // remaining 缺失时：先看显式 used，能反推就反推；否则视为已用满（0 剩余）。
    let explicit_used = parse_f64(obj.get("used"));
    let remaining = parse_f64(obj.get("remaining"))
        .unwrap_or_else(|| explicit_used.map(|u| (limit - u).max(0.0)).unwrap_or(0.0));
    let used = explicit_used.unwrap_or_else(|| (limit - remaining).max(0.0));
    // clamp：防御 used > limit 的异常上限态渲染出 >100% 的 bar
    let utilization = ((used / limit) * 100.0).clamp(0.0, 100.0);
    Some(QuotaRow {
        label,
        utilization: Some(utilization),
        remaining: Some(remaining),
        used: Some(used),
        total: Some(limit),
        resets_at,
        unit: Some("%".to_string()),
        extra: None,
        kind: Some(kind),
    })
}

/// 解析 JSON 值为 f64，兼容数字和字符串格式（如 `100` 和 `"100"`）。
fn parse_f64(v: Option<&Value>) -> Option<f64> {
    // D-014 fix (2026-07-30 audit): 过滤 NaN/inf 字符串 ("NaN" / "inf"
    // / "-inf" / "Infinity") → f64::clamp 对 NaN 是透传,前端会渲染
    // 诡异百分比 / 余额. 对齐共享 parse::num_f64 (H12 fix).
    v.and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_i64().map(|i| i as f64))
            .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
            .filter(|f| f.is_finite())
    })
}

/// 从 JSON 值提取重置时间（毫秒），兼容字符串和数字格式。
/// - 字符串：ISO 8601 → 毫秒；不是 ISO 8601 时继续按数字字符串解析
/// - 数字：自动判断秒/毫秒（< 1e12 当作秒，否则毫秒）→ 毫秒
fn extract_reset_ms(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(s) = v.as_str() {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.timestamp_millis());
        }
        // P2 audit fix (2026-08-13): 之前 RFC3339 解析失败直接 return None,
        // 数字字符串 (如 "1749840000" —— 该 API 序列化数字字段的习惯,
        // 见 fixture parse_total_quota_populated_builds_row) 永远落不到
        // 数字分支 → reset 倒计时丢失。改为继续尝试数字解析。
        if let Ok(n) = s.trim().parse::<i64>() {
            if n > 0 {
                let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
                // sanity check：转回 DateTime 避免溢出
                return DateTime::<Utc>::from_timestamp_millis(ms).map(|_| ms);
            }
        }
        return None;
    }
    if let Some(n) = v.as_i64() {
        // D-013 fix (2026-07-30 audit): 拒绝 n <= 0 (epoch 0 / 负数 /
        // 服务端 schema 漂移) → 否则 from_timestamp_millis(0) 返 epoch
        // 1970-01-01 → 浮窗显示诡异重置时间
        if n <= 0 {
            return None;
        }
        let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
        // sanity check：转回 DateTime 避免溢出
        return DateTime::<Utc>::from_timestamp_millis(ms).map(|_| ms);
    }
    None
}

// ── 「总套餐」月度池（v0.2.5）──────────────────────────────────────

/// 构造「总套餐」行：utilization-only（不带 used/total），前端走
/// utilization 分支显示 label「总套餐」+ 百分比 + 进度条 +
/// 「月重置」倒计时（`extra.reset_period = "monthly"`）。
fn build_total_plan_row(
    utilization: f64,
    resets_at: Option<i64>,
    code_used_ratio_pct: Option<f64>,
) -> QuotaRow {
    let mut extra = serde_json::json!({ "reset_period": "monthly" });
    // 官方 UI 双色堆叠条的蓝色段（总池里 Kimi Code 消耗占比），
    // 塞 extra 供后续堆叠条渲染用。
    if let Some(c) = code_used_ratio_pct {
        // P2 audit fix (2026-08-13): clamp 到 [0, utilization] —— 之前原样
        // 存储, API 数据异常时 (code 段 > 总利用率) 前端"总 − Code"算出负段,
        // 堆叠条渲染破裂。
        let clamped = c.clamp(0.0, utilization.clamp(0.0, 100.0));
        extra["kimi_code_used_ratio"] = serde_json::json!(clamped);
    }
    QuotaRow {
        label: t!("row.total_plan").to_string(),
        utilization: Some(utilization.clamp(0.0, 100.0)),
        remaining: None,
        used: None,
        total: None,
        resets_at,
        unit: Some("%".to_string()),
        extra: Some(extra),
        kind: None,
    }
}

/// 防御性解析 API 响应自带的 `totalQuota` 字段。
///
/// 2026-08-04 实测（Allegretto 档）该字段恒为空 `{}`，但已在 schema 里，
/// 未来官方若填上则 API key 直拿总池。猜测 schema 跟 `usage` 同款
/// `{limit, used/remaining, resetTime}`（数字或数字字符串）。
fn parse_total_quota(raw: &Value) -> Option<QuotaRow> {
    let tq = raw
        .get("totalQuota")?
        .as_object()
        .filter(|o| !o.is_empty())?;
    let tq = Value::Object(tq.clone());
    let limit = parse_f64(tq.get("limit"))?;
    if limit <= 0.0 {
        return None;
    }
    let explicit_used = parse_f64(tq.get("used"));
    let remaining = parse_f64(tq.get("remaining"))
        .unwrap_or_else(|| explicit_used.map(|u| (limit - u).max(0.0)).unwrap_or(0.0));
    let used = explicit_used.unwrap_or_else(|| (limit - remaining).max(0.0));
    let resets_at = extract_reset_ms(tq.get("resetTime"));
    Some(build_total_plan_row(
        (used / limit) * 100.0,
        resets_at,
        None,
    ))
}

/// 网页会话路径：调 `GetSubscriptionStats` 拿总池，返回（行, 原始响应）。
/// 任何失败返 None + 日志（best-effort 增强，不向上抛错）。
async fn fetch_total_plan_row(token: &str, info: &KimiSessionInfo) -> Option<(QuotaRow, Value)> {
    // 请求头对齐 CodexBar `webRequest`（2026-08-04 本机实测 200 的完整
    // header 集合）：Bearer + Cookie 双写、connect-protocol-version、
    // x-msh-platform、JWT claims 解出的 device/session/traffic id。
    let mut req = shared_client()
        .post(STATS_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Cookie", format!("kimi-auth={token}"))
        .header("Content-Type", "application/json")
        .header("Origin", "https://www.kimi.com")
        .header("Referer", "https://www.kimi.com/code/console")
        .header("Accept", "*/*")
        .header("connect-protocol-version", "1")
        .header("x-msh-platform", "web")
        .header(
            "x-language",
            match rust_i18n::locale().as_ref() {
                "zh-CN" => "zh-CN",
                _ => "en-US",
            },
        )
        .header(
            "r-timezone",
            iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
        )
        .header("User-Agent", BROWSER_UA)
        .body("{}");
    if let Some(d) = &info.device_id {
        req = req.header("x-msh-device-id", d);
    }
    if let Some(s) = &info.session_id {
        req = req.header("x-msh-session-id", s);
    }
    if let Some(t) = &info.traffic_id {
        req = req.header("x-traffic-id", t);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(err = %e, "[kimi] GetSubscriptionStats 网络失败 → 跳过总套餐行");
            return None;
        }
    };
    let status = resp.status();
    if !status.is_success() {
        // 401/403 = 会话失效（kimi-desktop 侧登出等）；5xx = 服务端抖动。
        // 都只跳行，不影响主快照。
        tracing::warn!(
            status = status.as_u16(),
            "[kimi] GetSubscriptionStats 非 2xx → 跳过总套餐行"
        );
        return None;
    }
    let raw = match json_body_limited(resp).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(err = %e, "[kimi] GetSubscriptionStats 响应解析失败 → 跳过总套餐行");
            return None;
        }
    };
    let row = parse_total_plan_from_stats(&raw)?;
    Some((row, raw))
}

/// 解析 `GetSubscriptionStats` 响应里的总池余额。
///
/// CodexBar 同款门控：只认 `feature == FEATURE_OMNI`（或缺失）+
/// `type == SUBSCRIPTION`（或缺失）—— `amountUsedRatio` 才是全功能共享池
/// （`kimiCodeUsedRatio` 只是池子里 Code 的部分）。
fn parse_total_plan_from_stats(raw: &Value) -> Option<QuotaRow> {
    let balance = raw.get("subscriptionBalance")?;
    let feature = balance.get("feature").and_then(|x| x.as_str());
    if feature.is_some_and(|f| f != "FEATURE_OMNI") {
        return None;
    }
    let ty = balance.get("type").and_then(|x| x.as_str());
    if ty.is_some_and(|t| t != "SUBSCRIPTION") {
        return None;
    }
    let ratio = parse_f64(balance.get("amountUsedRatio"))?;
    let resets_at = extract_reset_ms(balance.get("expireTime"));
    let code_ratio_pct = parse_f64(balance.get("kimiCodeUsedRatio")).map(|r| r * 100.0);
    Some(build_total_plan_row(
        ratio * 100.0,
        resets_at,
        code_ratio_pct,
    ))
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_full_response() {
        let raw = json!({
            "limits": [
                {
                    "detail": {
                        "limit": 100,
                        "remaining": 72,
                        "resetTime": "2026-06-14T18:30:00.000Z"
                    }
                }
            ],
            "usage": {
                "limit": 1000,
                "remaining": 742,
                "resetTime": 1749840000   // epoch 秒
            }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("kimi"));
        assert_eq!(snap.plan_name.as_deref(), Some("Coding Plan"));
        // 2 rows: 5h + weekly
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        assert_eq!(five_h.kind, Some(RowKind::FiveHour));
        assert!((five_h.utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(five_h.remaining, Some(72.0));
        assert_eq!(five_h.total, Some(100.0));
        assert_eq!(five_h.used, Some(28.0));
        // resetTime ISO 8601 → 2026-06-14T18:30:00.000Z = 1771005000000 ms (approximate)
        assert!(five_h.resets_at.is_some());

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, t!("row.weekly_7d"));
        assert_eq!(weekly.kind, Some(RowKind::Weekly));
        assert!((weekly.utilization.unwrap() - 25.8).abs() < 0.001);
        assert_eq!(weekly.remaining, Some(742.0));
        // epoch 秒 1749840000 → 1749840000000 ms
        assert_eq!(weekly.resets_at, Some(1749840000000));
    }

    #[test]
    fn parse_only_limits_no_usage() {
        // 老套餐只回 limits
        let raw = json!({
            "limits": [
                { "detail": { "limit": 50, "remaining": 50, "resetTime": null } }
            ]
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.five_hour").as_ref());
        assert_eq!(snap.rows[0].resets_at, None);
    }

    #[test]
    fn parse_only_usage_no_limits() {
        let raw = json!({
            "usage": { "limit": 500, "remaining": 100, "resetTime": 1749840000000_i64 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.weekly_7d"));
        assert_eq!(snap.rows[0].resets_at, Some(1749840000000));
    }

    #[test]
    fn parse_zero_limit_is_skipped() {
        // limit = 0 不展示（防御性，正常 schema 不会给）
        let raw = json!({
            "limits": [{ "detail": { "limit": 0, "remaining": 0 } }],
            "usage":  { "limit": 100, "remaining": 50 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1); // 5h 被跳过
        assert_eq!(snap.rows[0].label, t!("row.weekly_7d"));
    }

    #[test]
    fn parse_empty_is_error() {
        let raw = json!({});
        let err = parse(&raw, "kimi", "Kimi").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_missing_limit_is_error() {
        let raw = json!({
            "limits": [{ "detail": { "remaining": 50 } }]
        });
        let err = parse(&raw, "kimi", "Kimi").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    // ── 回归：5h 达 100% 上限后行不消失（2026-07-17）───────────────

    #[test]
    fn parse_5h_exhausted_remaining_zero_keeps_row() {
        // 5h 达上限：remaining=0。旧逻辑 (Some,Some) 门控其实能过,但确认
        // 100% utilization 正常建行(不被后续 clamp / 空判 drop)。
        let raw = json!({
            "limits": [{ "detail": { "limit": 100, "remaining": 0, "resetTime": 1749840000 } }],
            "usage": { "limit": 1000, "remaining": 530 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 2, "5h + weekly 都要在");
        let five_h = &snap.rows[0];
        assert_eq!(five_h.kind, Some(RowKind::FiveHour));
        assert!((five_h.utilization.unwrap() - 100.0).abs() < 0.001);
        assert!(five_h.resets_at.is_some());
    }

    #[test]
    fn parse_5h_exhausted_remaining_omitted_keeps_row() {
        // **核心回归**：5h 达上限时 API 省略 remaining 字段,只回 limit(+used)。
        // 旧逻辑 `(Some(l), Some(r))` 门控 → r=None → 整行 drop → 浮窗 5h 消失。
        // 新逻辑：remaining 缺失退化为已用满 → 100% 行仍在。
        let raw = json!({
            "limits": [{ "detail": { "limit": 100, "resetTime": 1749840000 } }],
            "usage": { "limit": 1000, "remaining": 742 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 2, "remaining 省略时 5h 行不能消失");
        let five_h = &snap.rows[0];
        assert_eq!(five_h.kind, Some(RowKind::FiveHour));
        assert!((five_h.utilization.unwrap() - 100.0).abs() < 0.001);
        assert_eq!(five_h.total, Some(100.0));
        assert_eq!(five_h.remaining, Some(0.0));
    }

    #[test]
    fn parse_window_row_prefers_explicit_used() {
        // 某些 schema 回 used 而不回 remaining（codexbar/usagebar 观测形态）。
        // used=139, limit=200 → utilization=69.5%, remaining 反推=61。
        let raw = json!({
            "limits": [{ "detail": { "limit": 200, "used": 139, "resetTime": 1749840000 } }]
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        let five_h = &snap.rows[0];
        assert!((five_h.utilization.unwrap() - 69.5).abs() < 0.001);
        assert_eq!(five_h.used, Some(139.0));
        assert_eq!(five_h.remaining, Some(61.0));
    }

    #[test]
    fn parse_window_row_clamps_over_limit() {
        // 防御：used > limit（异常上限态）不渲染 >100% 的 bar。
        let raw = json!({
            "usage": { "limit": 100, "used": 130 }
        });
        let snap = parse(&raw, "kimi", "Kimi").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert!((snap.rows[0].utilization.unwrap() - 100.0).abs() < 0.001);
    }

    #[test]
    fn extract_reset_ms_handles_iso_string() {
        let v = json!("2026-06-14T18:30:00.000Z");
        let ms = extract_reset_ms(Some(&v)).expect("iso");
        assert!(ms > 1_700_000_000_000 && ms < 1_800_000_000_000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_seconds() {
        let v = json!(1749840000_i64);
        let ms = extract_reset_ms(Some(&v)).expect("secs");
        assert_eq!(ms, 1749840000000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_millis() {
        let v = json!(1749840000000_i64);
        let ms = extract_reset_ms(Some(&v)).expect("ms");
        assert_eq!(ms, 1749840000000);
    }

    #[test]
    fn extract_reset_ms_invalid_returns_none() {
        assert_eq!(extract_reset_ms(None), None);
        assert_eq!(extract_reset_ms(Some(&json!("not a date"))), None);
        // 远超合理范围的数（from_timestamp_millis 返回 None）→ None
        assert_eq!(extract_reset_ms(Some(&json!(i64::MAX))), None);
    }

    #[test]
    fn extract_reset_ms_zero_or_negative_returns_none() {
        // D-013 fix (2026-07-30 audit): n == 0 / 负数不能被当成合法 epoch,
        // 否则 from_timestamp_millis(0) 返 epoch_0 → 浮窗显示 1970-01-01
        assert_eq!(extract_reset_ms(Some(&json!(0_i64))), None);
        assert_eq!(extract_reset_ms(Some(&json!(-1_i64))), None);
    }

    // ── 「总套餐」月度池（v0.2.5）──────────────────────────────────

    #[test]
    fn parse_total_quota_populated_builds_row() {
        // 防御性预留：官方未来若在 totalQuota 填数据（猜测 schema 同 usage）
        let raw = json!({
            "totalQuota": {
                "limit": "7168",
                "used": "3745",
                "resetTime": "2026-08-17T00:00:00Z"
            }
        });
        let row = parse_total_quota(&raw).expect("populated totalQuota");
        assert_eq!(row.label, t!("row.total_plan").as_ref());
        assert!((row.utilization.unwrap() - 52.247).abs() < 0.01);
        assert!(row.resets_at.is_some());
        // utilization-only 行（前端走 label 显示分支，不是 used/total 分支）
        assert_eq!(row.used, None);
        assert_eq!(row.total, None);
        assert_eq!(row.kind, None);
        assert_eq!(row.extra.as_ref().unwrap()["reset_period"], "monthly");
    }

    #[test]
    fn parse_total_quota_empty_or_missing_returns_none() {
        // 2026-08-04 实测现状：totalQuota 恒为空 {}
        assert!(parse_total_quota(&json!({ "totalQuota": {} })).is_none());
        assert!(parse_total_quota(&json!({})).is_none());
        // 非对象 / 缺 limit / limit=0 → None
        assert!(parse_total_quota(&json!({ "totalQuota": null })).is_none());
        assert!(parse_total_quota(&json!({ "totalQuota": { "used": 10 } })).is_none());
        assert!(parse_total_quota(&json!({ "totalQuota": { "limit": 0, "used": 0 } })).is_none());
    }

    #[test]
    fn parse_total_plan_from_stats_full_response() {
        // 2026-08-04 本机实测真实响应（Allegretto 档）
        let raw = json!({
            "ratelimitCode5h": { "ratio": 0.0613, "enabled": true, "resetTime": "2026-08-04T10:56:17.085661711Z" },
            "ratelimitCode7d": { "ratio": 0.0777, "enabled": true, "resetTime": "2026-08-07T07:56:18.085661711Z" },
            "subscriptionBalance": {
                "id": "19f6f13c-eaa2-8bef-8000-0000f5fa4151",
                "feature": "FEATURE_OMNI",
                "type": "SUBSCRIPTION",
                "unit": "UNIT_CREDIT",
                "amountUsedRatio": 0.5548,
                "kimiCodeUsedRatio": 0.2977,
                "expireTime": "2026-08-17T00:00:00Z"
            },
            "boosterWallets": []
        });
        let row = parse_total_plan_from_stats(&raw).expect("omni subscription balance");
        assert_eq!(row.label, t!("row.total_plan").as_ref());
        assert!((row.utilization.unwrap() - 55.48).abs() < 0.001);
        assert!(row.resets_at.is_some());
        let extra = row.extra.unwrap();
        assert_eq!(extra["reset_period"], "monthly");
        assert!((extra["kimi_code_used_ratio"].as_f64().unwrap() - 29.77).abs() < 0.001);
    }

    #[test]
    fn parse_total_plan_from_stats_gates_non_omni_and_non_subscription() {
        // CodexBar 门控：feature / type 不匹配 → 不是共享总池 → None
        let coding = json!({
            "subscriptionBalance": {
                "feature": "FEATURE_CODING",
                "type": "SUBSCRIPTION",
                "amountUsedRatio": 0.5
            }
        });
        assert!(parse_total_plan_from_stats(&coding).is_none());
        let booster = json!({
            "subscriptionBalance": {
                "feature": "FEATURE_OMNI",
                "type": "BOOSTER",
                "amountUsedRatio": 0.5
            }
        });
        assert!(parse_total_plan_from_stats(&booster).is_none());
        // feature / type 缺失（字段缺省）→ 放行（CodexBar 同款 nil-放行）
        let missing = json!({
            "subscriptionBalance": { "amountUsedRatio": 0.5 }
        });
        assert!(parse_total_plan_from_stats(&missing).is_some());
    }

    #[test]
    fn parse_total_plan_from_stats_missing_ratio_returns_none() {
        let raw = json!({
            "subscriptionBalance": {
                "feature": "FEATURE_OMNI",
                "type": "SUBSCRIPTION",
                "expireTime": "2026-08-17T00:00:00Z"
            }
        });
        assert!(parse_total_plan_from_stats(&raw).is_none());
        assert!(parse_total_plan_from_stats(&json!({})).is_none());
        // NaN 字符串防御（D-014 同款）
        let nan = json!({
            "subscriptionBalance": { "amountUsedRatio": "NaN" }
        });
        assert!(parse_total_plan_from_stats(&nan).is_none());
    }

    #[test]
    fn parse_total_plan_from_stats_clamps_over_100_percent() {
        let raw = json!({
            "subscriptionBalance": { "amountUsedRatio": 1.23 }
        });
        let row = parse_total_plan_from_stats(&raw).expect("row");
        assert!((row.utilization.unwrap() - 100.0).abs() < 0.001);
    }
}
