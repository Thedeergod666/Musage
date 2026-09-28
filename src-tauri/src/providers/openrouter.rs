//! OpenRouter 余额监控
//!
//! **策略**：先试 `/api/v1/credits`（账户余额），失败再试 `/api/v1/key`（per-key 限额）。
//!
//! ## 端点 1: `/api/v1/credits`（账户余额，需要 Management key）
//!
//! ```json
//! { "data": { "total_credits": 100.5, "total_usage": 25.75 } }
//! ```
//! 余额 = `total_credits - total_usage`（文档说 Management key required，
//! 但普通 key 也能用——OpenRouter 鉴权宽松）
//!
//! ## 端点 2: `/api/v1/key`（per-key 限额，任何 key 都行）
//!
//! ```json
//! { "data": { "limit": 100.0, "limit_remaining": 74.25,
//!             "is_free_tier": false, ... } }
//! ```
//!
//! **问题**：`limit_remaining` 是 per-key 级别的 credit limit，**不是账户余额**。
//! 账户可能有 $5 余额但 key 的 credit limit 是 $100 → 显示 $100 而不是 $5。
//!
//! 渲染：1 行「余额 $X.XX USD」（DeepSeek-style balance-row）

use std::borrow::Cow;
use std::pin::Pin;

use serde_json::Value;

use super::{
    humanize_reqwest_err, json_body_limited, shared_client, text_body_limited, validate_bearer_key,
    AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};
use crate::t;

const URL_CREDITS: &str = "https://openrouter.ai/api/v1/credits";
const URL_KEY: &str = "https://openrouter.ai/api/v1/key";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct OpenrouterSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

// M15 fix: fallback 缓存 —— /credits 和 /key 两个端点都可能被 401 / 5xx 拒绝。
// 之前每次 fetch 都要先试 /credits（失败）再试 /key，浪费 50% 请求。
// 缓存最近 5 分钟内成功的端点；TTL 过后重新探测（应对 endpoint 状态变化）。
//
// C2 fix (2026-07-03 audit): 之前缓存是进程级全局 static,不绑 instance_index。
// 多实例(普通 key + Management key)共享同一槽位 → 实例 1 写入 Credits 后,
// 实例 2 fetch 时 should_skip_endpoint(Key) 返 true 跳过 /key,永远拉不到数据。
// 改为 HashMap<unique_id, (Instant, Endpoint)> 按 instance 分桶。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Credits,
    Key,
}
static LAST_SUCCESSFUL: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Endpoint)>>,
> = std::sync::OnceLock::new();

fn last_successful(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Endpoint)>> {
    LAST_SUCCESSFUL.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn remember_endpoint(source_id: &str, ep: Endpoint) {
    if let Ok(mut g) = last_successful().lock() {
        g.insert(source_id.to_string(), (std::time::Instant::now(), ep));
    }
}

/// M12 fix: AuthFailed 时清缓存 —— 用户可能换了 key 类型（普通 → Management），
/// 下次 fetch 需要重新探测 /credits 和 /key，不能继续用 5 分钟前的成功记录。
fn clear_endpoint_cache(source_id: &str) {
    if let Ok(mut g) = last_successful().lock() {
        g.remove(source_id);
    }
}

fn should_skip_endpoint(source_id: &str, ep: Endpoint) -> bool {
    // D-009 fix (2026-07-30 audit): 之前 LAST_SUCCESSFUL HashMap 只增不删,
    // 用户频繁 add/delete extra instance 时 entry 永久残留 → HashMap 缓慢
    // 膨胀 (实操 low risk: source_id 是 provider_id + #index 形式, 实际
    // 不超过 builtin_sources + extra 数量; 但清理窗口是 free)。 修法:
    // 锁内顺便 prune 过期 entry (TTL = 5 分钟, 跟 should_skip 一致),
    // 每次 should_skip 调用都白嫖一次 GC, 不用额外 timer。
    let Ok(mut g) = last_successful().lock() else {
        return false;
    };
    let ttl = std::time::Duration::from_secs(300);
    g.retain(|_, (ts, _)| ts.elapsed() < ttl);
    match g.get(source_id) {
        Some((ts, last)) if ts.elapsed() < ttl => last != &ep,
        _ => false,
    }
}

impl Default for OpenrouterSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl OpenrouterSource {
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

impl QuotaSource for OpenrouterSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("openrouter")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "openrouter".to_string()
        } else {
            format!("openrouter#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        // i18n key `provider_name.openrouter` + `provider.suffix.dup` = " #{}"
        // t!() 返回 Cow 是临时值，统一用 Cow::Owned + into_owned，参考 minimax.rs:154-164
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.openrouter").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.openrouter").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKey
    }

    fn needs_state_update(&self) -> bool {
        false
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
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
                    t!("error.provider.unconfigured_key", provider = "OpenRouter").into_owned(),
                ));
            }
            do_fetch(api_key, &self.unique_id(), self.display_name().as_ref()).await
        })
    }
}

async fn do_fetch(
    api_key: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    // ── 第一优先：/api/v1/credits（账户余额，准确） ──
    // M15 fix: 最近 5 分钟内 /key 成功过 → 跳过 /credits 探测（避免重复 401 浪费请求）
    let try_credits = !should_skip_endpoint(source_id, Endpoint::Credits);
    if try_credits {
        match fetch_credits(client, api_key, source_id, display_name).await {
            Ok(snap) => {
                remember_endpoint(source_id, Endpoint::Credits);
                return Ok(snap);
            }
            // P3 audit fix (2026-08-13): Parse 错误 (HTTP 200 + 非 schema body,
            // 如中转站 200 回显错误 payload / schema 漂移) 也 fallback 到 /key,
            // 否则该 key 卡在错误直到 body 形态变化。
            Err(e)
                if matches!(
                    e.kind,
                    ErrorKind::AuthFailed | ErrorKind::ServerError | ErrorKind::Parse
                ) =>
            {
                // M12 fix: AuthFailed 时清缓存，下次重新探测两个端点
                // (用户可能换了 key 类型：普通 → Management，/credits 应重试)
                if e.kind == ErrorKind::AuthFailed {
                    clear_endpoint_cache(source_id);
                }
                // Management key 被拒 / 5xx → fallback 到 /api/v1/key
                tracing::debug!(error = %e, "openrouter /credits 失败，fallback 到 /key");
            }
            Err(e) => return Err(e), // 网络 / 解析错误直接报
        }
    }

    // ── fallback：/api/v1/key（per-key 限额，任何 key 都行） ──
    match fetch_key(client, api_key, source_id, display_name).await {
        Ok(snap) => {
            remember_endpoint(source_id, Endpoint::Key);
            Ok(snap)
        }
        Err(e) => Err(e),
    }
}

/// `GET /api/v1/credits` → 账户余额
async fn fetch_credits(
    client: &reqwest::Client,
    api_key: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // L-6 fix (2026-09-05 audit)：key 内含控制字符时 send() 报 invalid header
    // 被兜底归成误导性 Network 错误；send 前显式拒绝并归类配置错误。
    // 2026-09-28 audit H-9 补齐：当日 L-6 只接了 4/14 个 provider。
    validate_bearer_key(api_key)?;
    let resp = client
        .get(URL_CREDITS)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!(
                    "error.common.network",
                    url = URL_CREDITS,
                    err = humanize_reqwest_err(&e)
                )
                .into_owned(),
            )
        })?;

    let status = resp.status();
    // H6 fix: 429 显式 → RateLimited（之前的 is_success() 兜底会归到 ServerError，
    // 触发 fallback 到 /key 端点，浪费请求）
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "OpenRouter").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "OpenRouter").into_owned(),
        ));
    }
    if !status.is_success() {
        // D-008 fix (2026-07-30 audit): fetch_key 已带 body preview (200 字符截断),
        // fetch_credits 之前只用 http_error_simple 不带 body, 排错时看不到上游响应
        let body = text_body_limited(resp).await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "OpenRouter",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw = json_body_limited(resp).await?;

    parse_credits(&raw, source_id, display_name)
}

/// `GET /api/v1/key` → per-key 限额
async fn fetch_key(
    client: &reqwest::Client,
    api_key: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // L-6 fix (2026-09-05 audit)：key 内含控制字符时 send() 报 invalid header
    // 被兜底归成误导性 Network 错误；send 前显式拒绝并归类配置错误。
    // 2026-09-28 audit H-9 补齐：当日 L-6 只接了 4/14 个 provider。
    validate_bearer_key(api_key)?;
    let resp = client
        .get(URL_KEY)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!(
                    "error.common.network",
                    url = URL_KEY,
                    err = humanize_reqwest_err(&e)
                )
                .into_owned(),
            )
        })?;

    let status = resp.status();
    // 同 fetch_credits：429 显式 → RateLimited
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "OpenRouter").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "OpenRouter").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = text_body_limited(resp).await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "OpenRouter",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw = json_body_limited(resp).await?;

    parse_key(&raw, source_id, display_name)
}

/// 解析 `/api/v1/credits` 响应 → 1 行「余额 $X.XX USD」
/// OpenRouter HTTP 200 + body 内 `error` 节点 → 分类好的 FetchError。
///
/// H-Provider fix (2026-09-07 audit): 旧实现只看 `data` 字段,忽略 body 内
/// `error` —— OpenRouter HTTP 200 + `{"error": {"code": 401, "message": "..."},
/// "data": null}` 会 fallback 到 "missing field data" Parse 错误, 内部错误
/// 当成 schema 漂移, 用户看不到 "重新登录" 引导。先 check `error` 字段,
/// 按 status/code 分类 (401 → auth, 其他 → server) 让前端走正确的错误路径。
///
/// 2026-09-28 audit H-9: 抽成 helper 给 `/api/v1/credits` + `/api/v1/key`
/// 两条路径共用 —— `parse_key` 此前完全没有 error 检查，key 失效
/// (`{"error": {"code": 401}, "data": null}`) 报 Parse「缺少 data」，
/// 而 Parse 不退避 + `needs_settings()` 返 false → 前端**不亮**「重新登录」
/// 按钮，用户只看到一句 schema 漂移文案。
///
/// 仅对**对象形态**的 `error` 生效（`.filter(|e| e.is_object())`）：非对象
/// （如 `{"error": "bad key"}`）没有 code/message 可分类，交给调用方的
/// missing-data 分支报 Parse，保持既有行为与既有单测断言不变。
fn body_error(raw: &Value) -> Option<FetchError> {
    let err = raw.get("error").filter(|e| e.is_object())?;
    let code = err
        .get("code")
        .and_then(|v| v.as_i64())
        .or_else(|| err.get("status").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("");
    // 2026-09-28 audit H-9: 原用 `error.common.api_error`，该 key 从未加进
    // locales/{en,zh-CN}.json → rust-i18n 找不到 key 时返 `zh-CN.error.common.api_error`
    // 字面量且不做参数替换，本 fix 想让用户看到的真实报错一个字都看不到。
    // `error.common.business_code` 的占位符与格式完全一致，直接复用。
    let reason = t!(
        "error.common.business_code",
        provider = "OpenRouter",
        code = code,
        msg = msg
    )
    .into_owned();
    Some(if code == 401 || code == 403 {
        FetchError::auth(reason)
    } else {
        FetchError::server(reason)
    })
}

fn parse_credits(
    raw: &Value,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    // 见 `body_error` 的 H-Provider fix 注释（2026-09-07 audit 引入 / 2026-09-28 抽 helper）
    if let Some(e) = body_error(raw) {
        return Err(e);
    }
    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "OpenRouter",
                field = "data"
            )
            .into_owned(),
        )
    })?;

    let total_credits = num_f64(data, "total_credits").ok_or_else(|| {
        FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "OpenRouter",
                field = "total_credits"
            )
            .into_owned(),
        )
    })?;
    let total_usage = num_f64(data, "total_usage").unwrap_or(0.0);
    let remaining = (total_credits - total_usage).max(0.0);

    let rows = vec![QuotaRow {
        label: t!("row.balance").to_string(),
        utilization: None,
        remaining: Some(remaining),
        used: None,
        total: None,
        resets_at: None,
        unit: Some("USD".to_string()),
        extra: None,
        kind: None,
    }];

    Ok(ProviderSnapshot {
        // v0.3: 用 source_id ("openrouter") 替代旧 "minimax" 占位
        provider: "openrouter".to_string(),
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
        plan_name: Some("OpenRouter".to_string()),
        transient: None,
    })
}

/// 解析 `/api/v1/key` 响应 → 1 行「余额 $X.XX USD」（per-key fallback）
fn parse_key(
    raw: &Value,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    // 2026-09-28 audit H-9: 与 `parse_credits` 共用 `body_error` —— key 失效
    // (`{"error": {"code": 401}, "data": null}`) 必须归 AuthFailed 才能亮
    // 「重新登录」，见 helper 注释。放在 missing-data 检查**之前**，否则
    // `"data": null` 分支先短路，error 分类永远跑不到。
    if let Some(e) = body_error(raw) {
        return Err(e);
    }
    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse(
            t!("error.common.missing_data_field", provider = "OpenRouter").into_owned(),
        )
    })?;

    let remaining = num_f64(data, "limit_remaining");
    let limit = num_f64(data, "limit");
    let is_free_tier = data
        .get("is_free_tier")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let plan_name = if is_free_tier {
        Some(t!("row.free_tier").to_string())
    } else {
        Some("OpenRouter".to_string())
    };

    let mut rows = Vec::new();

    if let Some(r) = remaining {
        rows.push(QuotaRow {
            label: t!("row.balance").to_string(),
            utilization: None,
            remaining: Some(r),
            used: None,
            total: limit,
            resets_at: None,
            unit: Some("USD".to_string()),
            extra: None,
            kind: None,
        });
    }

    // H8 fix (2026-07-03 audit): free_tier 用户 API 可能不返回 limit_remaining
    // (无限额度)。之前 rows 为空 → 报 Parse 错。改为 free_tier + 无 remaining
    // 时显示"Free tier"行,而非报错。
    if rows.is_empty() && is_free_tier {
        rows.push(QuotaRow {
            label: t!("row.free_tier").to_string(),
            utilization: Some(0.0),
            remaining: None,
            used: None,
            total: None,
            resets_at: None,
            unit: None,
            extra: None,
            kind: None,
        });
    }

    // D3-02 (2026-09-04 audit): 付费 key 未设 per-key limit 时 limit /
    // limit_remaining 双 null（OpenRouter 语义 = 无上限，付费用户常见默认）。
    // H8 只修了 free_tier 半边，非 free_tier 的 unlimited 走到这仍报
    // "缺 limit_remaining" Parse 错。改渲染 used-only 行（仿 anysearch
    // unlimited 分支），无上限不显示进度条。
    if rows.is_empty() {
        if remaining.is_none() && limit.is_none() {
            let used = num_f64(data, "usage").unwrap_or(0.0);
            rows.push(QuotaRow {
                label: t!("row.balance").to_string(),
                utilization: None,
                remaining: None,
                used: Some(used),
                total: None,
                resets_at: None,
                unit: Some("USD".to_string()),
                extra: None,
                kind: None,
            });
        } else {
            return Err(FetchError::parse(
                t!(
                    "error.common.missing_field_generic",
                    field = "limit_remaining"
                )
                .into_owned(),
            ));
        }
    }

    Ok(ProviderSnapshot {
        // v0.3: 用 source_id ("openrouter") 替代旧 "minimax" 占位
        provider: "openrouter".to_string(),
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
        plan_name,
        transient: None,
    })
}

fn num_f64(obj: &Value, field: &str) -> Option<f64> {
    obj.get(field).and_then(super::parse::num_f64)
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn num_f64_rejects_non_finite_strings() {
        let raw = json!({ "value": "NaN", "infinite": "inf" });
        assert_eq!(num_f64(&raw, "value"), None);
        assert_eq!(num_f64(&raw, "infinite"), None);
    }

    // ── /credits 端点 ──

    #[test]
    fn parse_credits_full() {
        let raw = json!({
            "data": { "total_credits": 100.5, "total_usage": 25.75 }
        });
        let snap = parse_credits(&raw, "openrouter", "OpenRouter").expect("parse_credits");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        let row = &snap.rows[0];
        assert_eq!(row.label, t!("row.balance"));
        assert!((row.remaining.unwrap() - 74.75).abs() < 0.01);
        assert_eq!(row.unit.as_deref(), Some("USD"));
    }

    #[test]
    fn parse_credits_zero_balance() {
        let raw = json!({ "data": { "total_credits": 10.0, "total_usage": 10.0 } });
        let snap = parse_credits(&raw, "openrouter", "OpenRouter").expect("parse_credits");
        assert!((snap.rows[0].remaining.unwrap()).abs() < 0.01);
    }

    #[test]
    fn parse_credits_missing_total_credits() {
        let raw = json!({ "data": { "total_usage": 5.0 } });
        let err = parse_credits(&raw, "openrouter", "OpenRouter").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // ── /key 端点 ──

    #[test]
    fn parse_key_full() {
        let raw = json!({
            "data": {
                "label": "Musage 测试",
                "limit": 100.0,
                "limit_remaining": 74.25,
                "is_free_tier": false
            }
        });
        let snap = parse_key(&raw, "openrouter", "OpenRouter").expect("parse_key");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].remaining, Some(74.25));
        assert_eq!(snap.rows[0].unit.as_deref(), Some("USD"));
    }

    #[test]
    fn parse_key_free_tier_no_limit() {
        // H8 fix (2026-07-03 audit): 之前 free_tier + limit=null + limit_remaining=null
        // 报 Parse 错。但 OpenRouter free tier 用户没配额限制是合法状态 →
        // 应该返一行 "Free tier" 提示(utilization=0), 不报错。
        let raw = json!({
            "data": {
                "label": "free",
                "limit": null,
                "limit_remaining": null,
                "is_free_tier": true
            }
        });
        let snap = parse_key(&raw, "openrouter", "OpenRouter").unwrap();
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.free_tier").to_string());
        assert_eq!(snap.rows[0].utilization, Some(0.0));
    }

    #[test]
    fn parse_key_missing_data() {
        let raw = json!({ "error": "bad key" });
        let err = parse_key(&raw, "openrouter", "OpenRouter").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    /// 2026-09-28 audit H-9 回归：`/api/v1/key` 的对象形态 `error` 节点
    /// 必须按 code 分类（401 → AuthFailed，前端亮「重新登录」），而不是
    /// 落进 missing-data 的 Parse 错（不退避 + needs_settings=false）。
    /// 非对象 `error`（见上一个 test）仍走 Parse。
    #[test]
    fn parse_key_object_error_is_auth() {
        let raw = json!({
            "error": { "code": 401, "message": "Invalid key" },
            "data": null
        });
        let err = parse_key(&raw, "openrouter", "OpenRouter").unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthFailed);
    }

    #[test]
    fn parse_key_object_error_non_401_is_server() {
        let raw = json!({
            "error": { "code": 500, "message": "boom" },
            "data": null
        });
        let err = parse_key(&raw, "openrouter", "OpenRouter").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
    }

    /// D3-02 回归：付费 key 未设 per-key limit（limit/limit_remaining 双 null、
    /// is_free_tier=false）= 无上限，渲染 used-only 行而非报 Parse 错。
    #[test]
    fn parse_key_paid_unlimited_no_error() {
        let raw = json!({
            "data": {
                "label": "prod",
                "limit": null,
                "limit_remaining": null,
                "is_free_tier": false,
                "usage": 5.5
            }
        });
        let snap = parse_key(&raw, "openrouter", "OpenRouter").unwrap();
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].used, Some(5.5));
        assert_eq!(snap.rows[0].total, None, "unlimited → total=None");
        assert!(snap.rows[0].utilization.is_none(), "unlimited → 无进度条");
    }

    /// D3-02 反向：limit 已设但 remaining 缺失仍应报 Parse（非 unlimited 语义）。
    #[test]
    fn parse_key_limit_set_remaining_missing_still_parses_error() {
        let raw = json!({
            "data": {
                "label": "prod",
                "limit": 20.0,
                "limit_remaining": null,
                "is_free_tier": false
            }
        });
        let err = parse_key(&raw, "openrouter", "OpenRouter").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // D-003 fix (2026-07-30 audit): display_name 走 i18n key 而非硬编码
    #[test]
    fn display_name_uses_i18n_key() {
        // 单实例：直接用 provider_name.openrouter
        let s = OpenrouterSource::default();
        assert_eq!(s.display_name(), t!("provider_name.openrouter").as_ref());

        // 多实例：provider_name.openrouter + provider.suffix.dup 空格前缀
        let s2 = OpenrouterSource::default().with_instance_index(2);
        assert_eq!(
            s2.display_name(),
            format!(
                "{}{}",
                t!("provider_name.openrouter").as_ref(),
                t!("provider.suffix.dup", n = 2),
            )
        );
    }
}
