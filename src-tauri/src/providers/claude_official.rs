//! Claude Code 官方（OAuth + Cookie）用量查询
//!
//! 端点：`GET https://api.anthropic.com/api/oauth/usage`
//! 鉴权：`Cookie: sessionKey=<key>`（claude.ai 浏览器 session，非 Bearer）
//!
//! ## 用途
//!
//! Claude Pro / Max 订阅用户的官方 OAuth 配额监控。CCSwitch / Claude Code
//! 的 HUD 内部走的就是这个端点（参考 [ianlpaterson.com](https://ianlpaterson.com/blog/tracking-claude-codex-gemini-quotas-from-one-script)
//! 和 [Maciek-roboblog/Claude-Code-Usage-Monitor#202](https://github.com/Maciek-roboblog/Claude-Code-Usage-Monitor/issues/202)）。
//! 未在 Anthropic 公开 API ref 出现，但实际可用。
//!
//! ## 关键 Headers
//!
//! - `User-Agent: claude-code/<version>`（**必填**，普通 UA 会被拒）
//! - `Anthropic-Beta: oauth-2025-04-20`（**必填**，OAuth 端点 beta feature gate）
//! - `Cookie: sessionKey=<key>`
//!
//! ## 响应 schema（实测逆向，2026-06-16）
//!
//! ```json
//! {
//!   "five_hour": {
//!     "utilization": 72.0,                  // 5h 滚动窗口已用百分比 (0-100)
//!     "resets_at": "2026-06-16T18:30:00.000Z"  // ISO 8601 重置时间
//!   },
//!   "seven_day": {
//!     "utilization": 45.0,
//!     "resets_at": "2026-06-19T03:00:00.000Z"
//!   }
//! }
//! ```
//!
//! ## 渲染策略
//!
//! - 第一行 `5h`：`five_hour.utilization` + `resets_at`
//! - 第二行 `周`：`seven_day.utilization` + `resets_at`
//! - 任意 tier 缺失 → 自然降级为 1 行
//!
//! ## 已知坑
//!
//! 1. **429 频发**（[claude-code#31021](https://github.com/anthropics/claude-code/issues/31021)）：
//!    即便间隔几小时，OAuth usage API 也会持续 429。`is_healthy=false` 不算错，
//!    浮窗照常显示，但用户会看到 "请求过快"。**对这种 429 不重试** —— 重试
//!    只会恶化问题，浮窗让用户自己等。
//! 2. **Cookie 8h 过期**：claude.ai 登录 session 大约 8 小时失效，前端需
//!    提供 "Cookie 已过期" 提示 + 引导用户重新提取（`ErrorKind::AuthFailed`
//!    已覆盖）。
//! 3. **`sessionKey` 提取路径**：浏览器登录 claude.ai → DevTools → Application
//!    → Cookies → `sessionKey` 的 value 整段复制。
//!
//! ## Auth 字段设计
//!
//! `auth_kind = Cookie`：用户在前端 cookie 输入框粘贴 `sessionKey=<value>`
//! 整行，**或**只粘贴纯 value（程序自动补 `sessionKey=` 前缀，容错）。

use std::borrow::Cow;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow, QuotaSource};
use crate::t;

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
// H10 fix: 之前硬编码 "claude-code/1.0.0" —— 全部用户共享一个版本，Anthropic
// 一收紧 UA 全部同时崩。改成从 CARGO_PKG_VERSION 取（Musage 自己版本号跟随
// 每个 release 走），未来 musage 升版本自动带新 UA 出去。
const USER_AGENT: &str = concat!("claude-code/", env!("CARGO_PKG_VERSION"));
const OAUTH_BETA: &str = "oauth-2025-04-20";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct ClaudeOfficialSource;

impl Default for ClaudeOfficialSource {
    fn default() -> Self { Self }
}

impl QuotaSource for ClaudeOfficialSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("claude_official") }
    fn display_name(&self) -> Cow<'_, str> { Cow::Owned(t!("provider_name.claude_official").into_owned()) }
    fn auth_kind(&self) -> AuthKind { AuthKind::Cookie }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Claude 官方没有 region / mode 概念
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            // 优先用 cookie 字段；其次用 api_key 字段（用户可能误填进 api_key）
            let raw = credentials
                .cookie
                .as_deref()
                .or(credentials.api_key.as_deref())
                .unwrap_or("")
                .trim();
            if raw.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "Claude").into_owned()
                ));
            }
            let session_key = normalize_session_key(raw);
            do_fetch(&session_key).await
        })
    }
}

/// 把 "sessionKey=xxx" / 纯 "xxx" / 整段 cookie 字符串 统一规整成纯 value。
///
/// 防御性：
/// - 用户可能只复制 value（最常见）
/// - 用户可能整段复制 `sessionKey=xxx; other=yyy`（少见但要支持）
/// - 用户可能整段复制 `sessionKey=xxx`（带前缀）
fn normalize_session_key(raw: &str) -> String {
    let s = raw.trim();
    // 整段 cookie 形式（含分号）→ 拆出 sessionKey 的值
    if s.contains(';') {
        for part in s.split(';') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                if k.trim() == "sessionKey" {
                    return v.trim().to_string();
                }
            }
        }
    }
    // 带前缀的 "sessionKey=xxx" → 去掉前缀
    if let Some(v) = s.strip_prefix("sessionKey=") {
        return v.trim().to_string();
    }
    s.to_string()
}

async fn do_fetch(session_key: &str) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    let resp = client
        .get(URL)
        .header("Cookie", format!("sessionKey={session_key}"))
        .header("User-Agent", USER_AGENT)
        .header("Anthropic-Beta", OAUTH_BETA)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::network(
            t!("error.common.network", url = URL, err = e.to_string()).into_owned()
        ))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.claude_official.cookie_invalid_hint").into_owned()
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // M14 fix: 解析 Retry-After header（RFC 6585 §4）—— 可能是秒数也可能是 HTTP-date。
        // 把 wait_seconds 附加到 error message，让用户知道大概要等多久。
        let retry_secs = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| {
                // 尝试 HTTP-date（RFC 7231）—— 解析为 epoch seconds 差
                use chrono::DateTime;
                resp.headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| DateTime::parse_from_rfc2822(s).ok())
                    .map(|dt| {
                        let now = chrono::Utc::now();
                        (dt.with_timezone(&chrono::Utc) - now).num_seconds().max(0) as u64
                    })
            });
        let msg = match retry_secs {
            Some(s) => t!(
                "error.common.rate_limited_with_retry",
                provider = "Claude",
                retry_secs = s
            ).into_owned(),
            None => t!("error.common.rate_limited", provider = "Claude").into_owned(),
        };
        return Err(FetchError::new(ErrorKind::RateLimited, msg));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "Claude",
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            ).into_owned()
        ));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(
            t!("error.common.parse_json", err = e.to_string()).into_owned()
        ))?;

    parse(&raw)
}

/// 解析 Claude OAuth usage 响应 → QuotaRow 列表。
///
/// 任意 tier 缺失 → 自然降级（5h 没回就只显示周；反之亦然）。两个都缺 → 报错。
fn parse(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let mut rows = Vec::new();

    // 5h tier
    if let Some(five_h) = raw.get("five_hour") {
        if let Some(row) = build_tier_row("5h", five_h) {
            rows.push(row);
        }
    }

    // 周 tier
    if let Some(weekly) = raw.get("seven_day") {
        if let Some(row) = build_tier_row("周", weekly) {
            rows.push(row);
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.claude_official.missing_five_hour_seven_day").into_owned(),
        ));
    }

    Ok(ProviderSnapshot {
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("claude_official".to_string()),
        source_display_name: Some(t!("provider_name.claude_official").into_owned()),
        plan_name: Some("Pro/Max".to_string()),
        transient: None,
    })
}

/// 把单个 tier object 解析成 QuotaRow。任一关键字段缺失 → None（自然降级）。
fn build_tier_row(label: &str, tier: &Value) -> Option<QuotaRow> {
    let utilization = tier.get("utilization").and_then(|v| v.as_f64())?;
    if !(0.0..=100.0).contains(&utilization) {
        // 异常值（> 100 或负数）不显示，避免 UI 渲染出 bar
        return None;
    }
    let resets_at = tier
        .get("resets_at")
        .and_then(extract_reset_ms_from_string_or_int);
    Some(QuotaRow {
        label: label.to_string(),
        utilization: Some(utilization),
        remaining: None,
        used: None,
        total: None,
        resets_at,
        unit: Some("%".to_string()),
        extra: None,
            kind: None,
    })
}

/// 提取 resets_at 为毫秒。接受 ISO 8601 字符串（首选）或 epoch 数字（兜底）。
fn extract_reset_ms_from_string_or_int(v: &Value) -> Option<i64> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis());
    }
    if let Some(n) = v.as_i64() {
        let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
        return DateTime::<Utc>::from_timestamp_millis(ms).map(|_| ms);
    }
    None
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_full_response() {
        let raw = json!({
            "five_hour": {
                "utilization": 72.0,
                "resets_at": "2026-06-16T18:30:00.000Z"
            },
            "seven_day": {
                "utilization": 45.0,
                "resets_at": "2026-06-19T03:00:00.000Z"
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("claude_official"));
        assert_eq!(snap.source_display_name.as_deref(), Some(t!("provider_name.claude_official")));
        assert_eq!(snap.plan_name.as_deref(), Some("Pro/Max"));
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, "5h");
        assert!((five_h.utilization.unwrap() - 72.0).abs() < 0.001);
        assert_eq!(five_h.unit.as_deref(), Some("%"));
        // ISO 8601 → 2026-06-16T18:30:00.000Z 是 1781253000000 ms（近似）
        assert!(five_h.resets_at.is_some());
        assert!(five_h.resets_at.unwrap() > 1_780_000_000_000);

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, "周");
        assert!((weekly.utilization.unwrap() - 45.0).abs() < 0.001);
    }

    #[test]
    fn parse_only_five_hour() {
        // 只有 5h（Pro 套餐 / 5h-only 模式？）
        let raw = json!({
            "five_hour": {
                "utilization": 10.0,
                "resets_at": "2026-06-16T18:30:00.000Z"
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "5h");
    }

    #[test]
    fn parse_only_seven_day() {
        let raw = json!({
            "seven_day": {
                "utilization": 30.0,
                "resets_at": "2026-06-19T03:00:00.000Z"
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "周");
    }

    #[test]
    fn parse_no_tiers_is_error() {
        let raw = json!({});
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_missing_utilization_skips_tier() {
        // 5h 缺 utilization → 跳过；7d 正常 → 只显示 7d
        let raw = json!({
            "five_hour": { "resets_at": "2026-06-16T18:30:00.000Z" },
            "seven_day": { "utilization": 50.0, "resets_at": "2026-06-19T03:00:00.000Z" }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "周");
    }

    #[test]
    fn parse_out_of_range_utilization_skips_tier() {
        // utilization > 100 视为异常（不应该出现）→ 跳过
        let raw = json!({
            "five_hour": { "utilization": 150.0, "resets_at": "2026-06-16T18:30:00.000Z" },
            "seven_day": { "utilization": 20.0, "resets_at": "2026-06-19T03:00:00.000Z" }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "周");
    }

    #[test]
    fn parse_resets_at_as_epoch_seconds() {
        // 兜底：resets_at 是 epoch 秒而非 ISO
        let raw = json!({
            "five_hour": { "utilization": 5.0, "resets_at": 1_750_000_000_i64 }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows[0].resets_at, Some(1_750_000_000_000));
    }

    #[test]
    fn parse_resets_at_as_epoch_millis() {
        let raw = json!({
            "five_hour": { "utilization": 5.0, "resets_at": 1_750_000_000_000_i64 }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows[0].resets_at, Some(1_750_000_000_000));
    }

    #[test]
    fn parse_invalid_resets_at_is_none() {
        let raw = json!({
            "five_hour": { "utilization": 5.0, "resets_at": "not a date" }
        });
        let snap = parse(&raw).expect("parse");
        // 5h 仍显示，但 resets_at = None
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].resets_at, None);
    }

    // ── normalize_session_key 单元测试 ──

    #[test]
    fn normalize_plain_value() {
        assert_eq!(normalize_session_key("abc123"), "abc123");
    }

    #[test]
    fn normalize_with_prefix() {
        assert_eq!(
            normalize_session_key("sessionKey=abc123"),
            "abc123"
        );
    }

    #[test]
    fn normalize_full_cookie_string() {
        // 浏览器整段复制：sessionKey=xxx; other=yyy
        assert_eq!(
            normalize_session_key("sessionKey=abc123; foo=bar"),
            "abc123"
        );
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_session_key("  sessionKey=abc123  "),
            "abc123"
        );
    }

    #[test]
    fn normalize_empty_string() {
        assert_eq!(normalize_session_key(""), "");
    }
}
