//! StepFun（阶跃星辰）Step Plan 用量查询
//!
//! 端点（[CodexBar docs/stepfun.md](https://github.com/steipete/CodexBar/blob/main/docs/stepfun.md)
//! + `StepFunUsageFetcher.swift` 源码参考）：
//! - `POST https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit`
//! - `POST https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus`
//! - `POST https://platform.stepfun.com/passport/proto.api.passport.v1.PassportService/RefreshToken`
//!
//! ## 鉴权
//!
//! Dashboard 端点要求一组浏览器侧 headers（CodexBar reverse-engineered）：
//!
//! - `oasis-appid: 10300`
//! - `oasis-platform: web`
//! - `oasis-webid: <device_id>` — 必须等于 token **refresh 半段**的 JWT `device_id`
//!   claim（详见 [`device_id_for_token`]）。不匹配时服务端返
//!   `401 "auth failed: oasis-token is embezzled"`（CodexBar 源码注释实锤）。
//! - `Cookie: Oasis-Token=<token>; Oasis-Webid=<webid>` — token 值是
//!   `<access>...<refresh>` **整段 pair**（CodexBar combinedToken 约定；实测
//!   只发裸 access 半段会被拒：`401 "token is illegal"`）。
//! - 浏览器 UA（Chrome 147 / macOS）。
//!
//! ## ⭐ access 半段短命 + 主动续期（2026-07-28 真实账号探针实测）
//!
//! webview 登录抓到的 pair：**access 半段寿命仅 ~30 分钟**（签发后 exp-iat≈1800s），
//! **refresh 半段 ~30 天**且带 `device_id` claim（claims: app_id / device_id /
//! exp / oasis_id / oasis_r_at / platform / version）。浏览器靠 refresh 半段持续
//! 续期。本 provider 对齐 anysearch 的续期模式：
//!
//! 1. 请求前本地预检 access 的 `exp`：已过期或 `SKEW_SECS`(120s) 内将过期，
//!    且有 refresh 半段 → 先调 `RefreshToken` 端点换新 pair。
//! 2. refresh 成功后**立即**把新 pair 原子写回 keys.json（cookie 槽位，
//!    `Oasis-Token=` 前缀格式，跟登录存盘一致）。服务端没返新 refresh 半段时
//!    保留旧的（CodexBar 同款兜底）。
//! 3. 兜底：请求仍返 401 / 业务层 auth 失败（本地预检没抓到），有 refresh
//!    半段 → 再 refresh 一次 + 重试一遍。
//!
//! refresh 也失效（30 天没开 app / 风控 burn）→ auth 错误引导用户点浮窗
//! 「重新登录 StepFun」走 webview 重登（[`crate::stepfun_login`]）。
//!
//! ## 响应 schema（⚠ 2026-07-28 修正：成功标记是 `status == 1`，字段在顶层）
//!
//! 旧实现按 `{code: 0, data: {...}}` 解析 —— 实测服务端成功响应是**字段在顶层**，
//! 造成「请求其实成功、却被误判为 `code -1` 业务错误」的 bug。`parse` 现在两种
//! schema 都兼容（有 `data` 对象用 `data`，否则用顶层）。
//!
//! QueryStepPlanRateLimit 成功响应（CodexBar `StepFunRateLimitResponse`）：
//! ```json
//! {
//!   "status": 1,                                       // ⚠ 成功标记：status == 1
//!   "five_hour_usage_left_rate": 0.99781543,           // 5h 剩余比例 (0-1)
//!   "weekly_usage_left_rate": 0.85,                    // 周剩余比例
//!   "five_hour_usage_reset_time": "1785221470",        // ⚠ epoch 秒，可能是字符串
//!   "weekly_usage_reset_time": 1785686400,             // epoch 秒，可能是数字
//!   "plan_family": 2,                                  // 2 = credit 套餐 (Mini/Pro)
//!   "plan_credit_rate_limit": {
//!     "subscription_credit_left_rate": 0.96,
//!     "subscription_credit_reset_time": "1787068799",
//!     "topup_credit_left_rate": 0.5,
//!     "credit_buckets": [
//!       { "credit_total": 100, "credit_residual": 80, "expire_at": "...", "next_reset_at": "..." }
//!     ]
//!   }
//! }
//! ```
//!
//! 数字字段可能是 **int / float / 字符串**（CodexBar `StepFunFlexibleNumber`
//! 同款防御，如 `"400000000"`）；时间戳是 epoch 秒（字符串或数字），`"0"` =
//! 「无窗口配置」。失败响应形态：`{"status": <非1>, "message": ..., "desc": ...}`
//! 或 401 `{"code": "unauthenticated", "message": "auth failed: ..."}`。
//!
//! GetStepPlanStatus 返回 `{status, subscription: {name, plan_type, status}}`
//! （`subscription.name` = "Plus" / "Mini"，顶层，兼容旧 `/data/subscription/name`）。
//!
//! ## 渲染策略
//!
//! - Rate-window 套餐（`plan_family` 缺失或 ≠ 2）：
//!   - 第一行 `5h`：`(1.0 - five_hour_usage_left_rate) * 100`
//!   - 第二行 `周`：`(1.0 - weekly_usage_left_rate) * 100`
//! - Credit 套餐（`plan_family == 2`）：
//!   - 单行 `额度`：优先 `subscription_credit_left_rate`（缺则用
//!     `topup_credit_left_rate`，再缺则用 `credit_buckets` 加权平均）。
//! - plan_name 来自 GetStepPlanStatus（如 "Plus" / "Mini"）。
//!
//! ## 已知坑
//!
//! 1. **Token 失效**：refresh 半段 ~30 天过期。本地预检 + 自动续期覆盖日常；
//!    refresh 也废时返友好错误引导 webview 重登。
//! 2. **请求是 POST 而非 GET**：Step Plan rate limit 用 POST + JSON body（空
//!    body 也可），不是常规 GET。
//! 3. **风控**：服务端会按 `oasis-webid` ↔ token `device_id` 匹配性校验
//!    （不匹配 = "embezzled"）；从非浏览器客户端 replay 也可能被风控，
//!    失败路径已加响应体诊断日志（`[diag] stepfun ...`）便于排查。

use std::borrow::Cow;
use std::pin::Pin;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource,
};
use crate::config;
use crate::t;

const URL_RATE_LIMIT: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/QueryStepPlanRateLimit";
const URL_PLAN_STATUS: &str =
    "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard/GetStepPlanStatus";

/// access token 刷新端点（CodexBar `StepFunUsageFetcher.refreshTokenURL` 逆向）。
/// access 半段仅 ~30 分钟寿命（2026-07-28 探针实测 exp-iat≈1800s），refresh
/// 半段 ~30 天 —— 浏览器靠它持续续期，我们也一样。
const URL_REFRESH: &str =
    "https://platform.stepfun.com/passport/proto.api.passport.v1.PassportService/RefreshToken";

/// CodexBar 的 login/register 流使用的默认 Webid。
///
/// 仅在 token 中无法解析出 `device_id` 时作为兜底。注意 dashboard 端点
/// 大概率会拒绝这个值（不是当前 token 对应的 device），届时会落到
/// 401/403 错误路径；用户需要重新登录获取带 `device_id` claim 的 token。
const DEFAULT_WEBID: &str = "c8a1002d2c457e758785a9979832217c7c0b884c";

/// CodexBar 的固定 app id。
const OASIS_APPID: &str = "10300";

/// 浏览器 UA — CodexBar 用的 Chrome 147 / macOS，避免被风控识别为
/// 非浏览器客户端。
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

/// combined token 哨兵分隔符：`<access>...<refresh>`（CodexBar combinedToken
/// 约定，跟 anysearch 一致）。`...` 不是 base64url 合法字符（JWT 只含
/// `A-Za-z0-9-_` + `.`），拿它当分隔符绝不跟 token 自身内容冲突。
const TOKEN_SEP: &str = "...";

/// access token 过期缓冲：距 `exp` 不足这个秒数就提前 refresh（避免「刚好卡在
/// 请求发出瞬间过期」的边界 401）。
const SKEW_SECS: i64 = 120;

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct StepfunSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for StepfunSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl StepfunSource {
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

impl QuotaSource for StepfunSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("stepfun")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "stepfun".to_string()
        } else {
            format!("stepfun#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.stepfun").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.stepfun").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        // v0.2.5+: stepfun 改用 webview 一键登录 (src/stepfun_login.rs)，
        // token 落 `stepfun:cookie` 槽位。fetch 端 cookie 字段优先;
        // api_key 字段仅作 v0.2.4 手动粘贴时代 legacy 槽位的兜底。
        // AuthKind::Cookie 让 settings 面板走纯 cookie 模式 +
        // quick-login-banner 路径,跟 anysearch 同款 UX。
        AuthKind::Cookie
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // StepFun 无 region / mode 概念（虽然 URL 有 .com/.ai，但 Oasis-Token 跨域通用）
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            // v0.2.5+ webview 一键登录的 token 落 `stepfun:cookie` 槽位，
            // 必须 cookie 优先（跟 anysearch / claude_official 的 Cookie-kind
            // 约定一致）。**不能 api_key 优先**：v0.2.4 手动粘贴时代的
            // `stepfun` legacy 槽位不会被登录流程清掉（save_credential_for_id
            // 对 None 字段是跳过而非删除），若 api_key 优先会永远读到那个
            // 过期 token，新登录的 cookie 被完全忽略（2026-07-28 实测 bug）。
            // api_key 仅作 legacy 兜底（cookie 槽不存在时）。
            let raw = credentials
                .cookie
                .as_deref()
                .or(credentials.api_key.as_deref())
                .unwrap_or("");

            // 规范化（处理 "Cookie: Oasis-Token=..." / "Oasis-Token=..." 整段粘贴）
            let token = match normalize_oasis_token(raw) {
                Some(t) if !t.is_empty() => t,
                _ => {
                    return Err(FetchError::unconfigured(
                        t!("error.stepfun.token_unconfigured_hint").into_owned(),
                    ));
                }
            };

            do_fetch(&token, &self.unique_id(), &self.display_name().to_string()).await
        })
    }
}

/// 带续期的 fetch 主流程（对齐 anysearch 的 do_fetch 结构）。
async fn do_fetch(
    oasis_token: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // ── 主动续期：本地预检 access exp，已过期 / SKEW 内将过期且有
    //    refresh 半段 → 先调 RefreshToken 换新 pair（并写回 keys.json）──
    let mut token = match access_token_exp_seconds_ago(oasis_token) {
        Some(secs_ago) if secs_ago >= -SKEW_SECS => {
            if refresh_half(oasis_token).is_some() {
                refresh_oasis_token(oasis_token, source_id).await?
            } else if secs_ago >= 0 {
                // 无 refresh 半段（手动粘贴的裸 access）且已过期 → 友好错误
                return Err(FetchError::auth(
                    t!("error.stepfun.token_expired_hint", mins = secs_ago / 60).into_owned(),
                ));
            } else {
                oasis_token.to_string()
            }
        }
        Some(_) => oasis_token.to_string(), // access 还有效
        None => {
            // 完全无法识别为 JWT：给"格式无效"提示，避免落入 401 误导
            tracing::warn!(
                "StepFun Oasis-Token not decodable as JWT; dashboard request will likely 401"
            );
            oasis_token.to_string()
        }
    };

    match fetch_once(&token, source_id, display_name).await {
        // 兜底：请求仍 401 / 业务层 auth 失败（本地预检没抓到、但服务端已
        // 作废 / 风控 burn），有 refresh 半段 → refresh 一次再重试一遍。
        Err(e) if e.kind == ErrorKind::AuthFailed && refresh_half(&token).is_some() => {
            token = refresh_oasis_token(&token, source_id).await?;
            fetch_once(&token, source_id, display_name).await
        }
        other => other,
    }
}

/// 单次拉取（rate limit + plan status），不含续期逻辑。
async fn fetch_once(
    token: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // 并行拉 rate limit + plan status（互不依赖）
    let rate = fetch_rate_limit(token).await?;
    let plan = fetch_plan_status(token).await.ok().flatten(); // 失败不阻塞

    parse(rate, plan, source_id, display_name)
}

/// 组装 Step Plan dashboard 请求（带 cookie 鉴权 + 浏览器侧 headers）。
///
/// 抽出来让 `fetch_rate_limit` / `fetch_plan_status` 共用同一套鉴权，
/// 也方便单元测试断言 header 是否齐全。
fn build_request(client: &reqwest::Client, url: &str, token: &str) -> reqwest::RequestBuilder {
    let webid = device_id_for_token(token).unwrap_or_else(|| {
        tracing::warn!("StepFun token missing device_id claim; falling back to DEFAULT_WEBID");
        DEFAULT_WEBID.to_string()
    });
    let cookie_value = format!("Oasis-Token={token}; Oasis-Webid={webid}");

    client
        .post(url)
        .header("Cookie", cookie_value)
        // CodexBar 同时使用首字母大写和小写两种 header 名,保险都发。
        .header("Oasis-Webid", webid.clone())
        .header("oasis-webid", webid)
        .header("oasis-appid", OASIS_APPID)
        .header("oasis-platform", "web")
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body("{}")
}

/// POST Step Plan rate limit endpoint。
async fn fetch_rate_limit(token: &str) -> Result<Value, FetchError> {
    let client = shared_client();

    let resp = build_request(client, URL_RATE_LIMIT, token)
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!(
                    "error.common.network",
                    url = URL_RATE_LIMIT,
                    err = e.to_string()
                )
                .into_owned(),
            )
        })?;

    let status = resp.status();
    // 先拿 body 文本再 parse —— 失败路径能把原始响应写进诊断日志
    // （2026-07-28 教训：只报 "code -1" 不带 body 等于盲猜）。
    let body = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        tracing::warn!(status = %status, body = %truncate_body(&body, 500),
            "[diag] stepfun rate-limit 401/403 raw response");
        return Err(FetchError::auth(
            t!("error.stepfun.token_invalid_hint").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "StepFun").into_owned(),
        ));
    }
    if !status.is_success() {
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "StepFun",
                status = status.as_u16(),
                body = truncate_body(&body, 200)
            )
            .into_owned(),
        ));
    }

    let raw: Value = serde_json::from_str(&body).map_err(|e| {
        tracing::warn!(body = %truncate_body(&body, 500),
            "[diag] stepfun rate-limit 响应非 JSON");
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    ensure_success(&raw, &body)?;
    Ok(raw)
}

/// 成功判定（双 schema 兼容）+ 失败时构造业务错误。
///
/// - `status`（数字）存在 → `== 1` 才算成功（CodexBar `isSuccess` 同款；
///   **这是现行 schema 的成功标记**，旧实现只看 `code == 0` 导致把成功
///   响应误判为 `code -1` 业务错误）
/// - 否则 `code`（数字）存在 → `== 0` 才算成功（旧 schema）
/// - 都没有 → 宽容看有没有用量字段（顶层或 `data` 下）
///
/// 失败时：`message` / `desc` / `code` 字符串化拼进错误消息；消息含
/// "auth failed" / "unauth" / "embezzled" / "illegal" 时归 AuthFailed
/// （让上层 do_fetch 的「401 兜底 refresh 重试」接管），否则 Server。
fn ensure_success(raw: &Value, body: &str) -> Result<(), FetchError> {
    let status_field = raw.get("status").and_then(flex_i64);
    let code_num = raw.get("code").and_then(flex_i64);
    let ok = match (status_field, code_num) {
        (Some(s), _) => s == 1,
        (None, Some(c)) => c == 0,
        (None, None) => has_usage_fields(raw),
    };
    if ok {
        return Ok(());
    }

    let msg = ["message", "desc", "msg"]
        .iter()
        .filter_map(|k| raw.get(k).and_then(|v| v.as_str()))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();
    let code_display = raw
        .get("code")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other
                .as_i64()
                .map(|i| i.to_string())
                .unwrap_or_else(|| other.to_string()),
        })
        .or_else(|| status_field.map(|s| s.to_string()))
        .unwrap_or_else(|| "?".to_string());

    tracing::warn!(body = %truncate_body(body, 800),
        "[diag] stepfun rate-limit 业务失败 raw response");

    let haystack = format!("{msg} {code_display}").to_lowercase();
    let is_auth = haystack.contains("auth failed")
        || haystack.contains("unauth")
        || haystack.contains("embezzled")
        || haystack.contains("illegal");

    let msg_out = t!(
        "error.common.business_code",
        provider = "StepFun",
        code = code_display,
        msg = msg
    )
    .into_owned();
    Err(if is_auth {
        FetchError::auth(msg_out)
    } else {
        FetchError::server(msg_out)
    })
}

/// 宽容的「有没有用量字段」探测（顶层或 `data` 下），用于既无 `status`
/// 也无 `code` 时的成功判定兜底。
fn has_usage_fields(raw: &Value) -> bool {
    const FIELDS: &[&str] = &[
        "five_hour_usage_left_rate",
        "weekly_usage_left_rate",
        "plan_credit_rate_limit",
    ];
    let at = |obj: &Value| FIELDS.iter().any(|f| obj.get(f).is_some());
    at(raw)
        || raw
            .get("data")
            .filter(|d| d.is_object())
            .map(at)
            .unwrap_or(false)
}

/// POST Step Plan status endpoint。
/// L8 fix: 之前 HTTP 非 200 时返 Ok(None) 静默吞掉错误，
/// do_fetch 里 .ok().flatten() 也吞。plan_name 显示为 None 时
/// 用户/开发者查不到原因，日志也没有任何记录。
/// 改为非 200 时 log warn 后返 Ok(None)（plan_name 是可选字段，不阻塞主 fetch）。
async fn fetch_plan_status(token: &str) -> Result<Option<String>, FetchError> {
    let client = shared_client();

    let resp = build_request(client, URL_PLAN_STATUS, token)
        .send()
        .await
        .map_err(|e| FetchError::network(format!("StepFun plan status 网络错误: {e}")))?;

    if !resp.status().is_success() {
        // L8 fix: log warn 而不是静默返 Ok(None)
        tracing::warn!(
            status = %resp.status(),
            "StepFun plan status endpoint 非 200，plan_name 将为 None"
        );
        return Ok(None);
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("plan status 响应不是 JSON: {e}")))?;

    // 双 schema：新 {subscription: {...}} 顶层；旧 {data: {subscription: {...}}}
    let name = raw
        .pointer("/subscription/name")
        .or_else(|| raw.pointer("/data/subscription/name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(name)
}

/// 调 RefreshToken 端点用 refresh 半段换新 pair，并**立即**把新 combined
/// 原子写回 keys.json（cookie 槽位，`Oasis-Token=` 前缀格式，跟登录存盘
/// 一致；写回 id 用 unique_id，副本 `stepfun#2` 也对得上）。
///
/// CodexBar 实测该端点要求：body `{}` + 常规 oasis headers + 裸
/// `Oasis-Token` header + `Cookie: Oasis-Token=<combined>; Oasis-Webid=<webid>`。
/// 响应 `{accessToken: {raw}, refreshToken: {raw}}`（顶层，兼容 `data` 嵌套）。
/// 服务端没返新 refresh 半段时保留旧的（CodexBar 同款兜底）。
async fn refresh_oasis_token(token: &str, unique_id: &str) -> Result<String, FetchError> {
    let client = shared_client();
    let webid = device_id_for_token(token).unwrap_or_else(|| DEFAULT_WEBID.to_string());

    let resp = client
        .post(URL_REFRESH)
        .header("Cookie", format!("Oasis-Token={token}; Oasis-Webid={webid}"))
        // CodexBar 在 refresh 请求里额外发裸 Oasis-Token header（usage 查询不发）
        .header("Oasis-Token", token)
        .header("Oasis-Webid", webid.clone())
        .header("oasis-webid", webid)
        .header("oasis-appid", OASIS_APPID)
        .header("oasis-platform", "web")
        .header("User-Agent", BROWSER_USER_AGENT)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = URL_REFRESH, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        tracing::warn!(status = %status, body = %truncate_body(&body, 500),
            "[diag] stepfun refresh 401/403 raw response（refresh 半段也废了 → 引导重登）");
        return Err(FetchError::auth(
            t!("error.stepfun.token_invalid_hint").into_owned(),
        ));
    }
    if !status.is_success() {
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "StepFun",
                status = status.as_u16(),
                body = truncate_body(&body, 200)
            )
            .into_owned(),
        ));
    }

    let raw: Value = serde_json::from_str(&body).map_err(|e| {
        tracing::warn!(body = %truncate_body(&body, 500), "[diag] stepfun refresh 响应非 JSON");
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    let combined = parse_refresh_response(&raw, refresh_half(token)).ok_or_else(|| {
        tracing::warn!(body = %truncate_body(&body, 500),
            "[diag] stepfun refresh 响应缺 accessToken.raw");
        FetchError::server(t!("error.common.missing_data_field", provider = "StepFun").into_owned())
    })?;

    // 原子写回（refresh 半段可能轮换 —— 以服务端返的为准）。失败只 warn：
    // 本轮拿到的新 access 仍可用，只是下次得重登。
    let cred = Credentials {
        api_key: None,
        cookie: Some(format!("Oasis-Token={combined}")),
        secret_key: None,
    };
    if let Err(e) = config::save_credential_for_id(unique_id, &cred) {
        tracing::warn!(error = %e, unique_id,
            "stepfun refresh 后写回 keys.json 失败（本轮仍可用，下次可能需重登）");
    } else {
        tracing::info!(unique_id, "stepfun access token 已通过 refresh 续期并写回");
    }

    Ok(combined)
}

/// 解析 RefreshToken 响应 → 新 combined token（pure function，便于单测）。
///
/// `{accessToken: {raw}, refreshToken: {raw}}`（顶层或 `data` 嵌套）。
/// access 缺失/为空 → None（上层报错）；refresh 缺失 → 保留旧半段。
fn parse_refresh_response(raw: &Value, old_refresh: Option<&str>) -> Option<String> {
    let access = raw
        .pointer("/accessToken/raw")
        .or_else(|| raw.pointer("/data/accessToken/raw"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let new_refresh = raw
        .pointer("/refreshToken/raw")
        .or_else(|| raw.pointer("/data/refreshToken/raw"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    match new_refresh.or_else(|| old_refresh.map(String::from)) {
        Some(r) => Some(format!("{access}{TOKEN_SEP}{r}")),
        None => Some(access.to_string()),
    }
}

/// 解析 rate limit 响应 → QuotaRow 列表。
///
/// `usedPercent = (1.0 - left_rate) * 100`
///
/// ⚠ 双 schema 兼容（2026-07-28 修正）：现行成功响应字段在**顶层**
/// （`status == 1` 标记成功）；旧 schema 字段在 `data` 下（`code == 0`）。
/// 数字字段用 [`flex_f64`]（int / float / 字符串都吃），时间戳用
/// [`extract_reset_ms`]（ISO 8601 / epoch 秒 / epoch 毫秒 / 字符串形式）。
///
/// 两种 plan 形态：
/// - Rate-window（默认，`plan_family` 缺失或 ≠ 2）：5h + 周双行。
/// - Credit 套餐（`plan_family == 2`，Mini/Pro）：单行，按
///   `subscription_credit_left_rate` → `topup_credit_left_rate` →
///   `credit_buckets` 加权平均 的优先级挑出 left 比例。
fn parse(
    rate_raw: Value,
    plan_name: Option<String>,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // 双 schema：有 `data` 对象用 `data`（旧），否则用顶层（现行）
    let data: &Value = match rate_raw.get("data").filter(|d| d.is_object()) {
        Some(d) => d,
        None => &rate_raw,
    };

    let mut rows = Vec::new();

    let plan_family = data.get("plan_family").and_then(flex_i64).unwrap_or(0);
    if plan_family == 2 {
        // ── Credit 套餐（Mini/Pro）：单行 + bucket 加权平均 ──
        if let Some(left) = credit_plan_left_rate(data) {
            if (0.0..=1.0).contains(&left) {
                let used_pct = (1.0 - left) * 100.0;
                // 重置 / 到期时间（2026-07-28 真实响应实测）：订阅制周期
                // 重置 → resets_at（「额度重置」）；一次性额度包 expire_at
                // → resets_at + extra.reset_period="expire"（浮窗显示「到期」）
                let (reset_ms, is_expire) = credit_plan_reset(data);
                rows.push(QuotaRow {
                    label: t!("row.credit").to_string(),
                    utilization: Some(used_pct),
                    remaining: None,
                    used: None,
                    total: None,
                    resets_at: reset_ms,
                    unit: Some("%".to_string()),
                    extra: if is_expire {
                        Some(serde_json::json!({ "reset_period": "expire" }))
                    } else {
                        None
                    },
                    kind: None,
                });
            }
        }
    } else {
        // ── Rate-window 套餐：5h + 周双行 ──

        // 5h tier
        if let Some(left) = data
            .get("five_hour_usage_left_rate")
            .and_then(flex_f64)
        {
            if (0.0..=1.0).contains(&left) {
                let used_pct = (1.0 - left) * 100.0;
                let reset = data
                    .get("five_hour_usage_reset_time")
                    .and_then(extract_reset_ms);
                rows.push(QuotaRow {
                    label: t!("row.five_hour").to_string(),
                    utilization: Some(used_pct),
                    remaining: None,
                    used: None,
                    total: None,
                    resets_at: reset,
                    unit: Some("%".to_string()),
                    extra: None,
                    kind: None,
                });
            }
        }

        // 周 tier
        if let Some(left) = data.get("weekly_usage_left_rate").and_then(flex_f64) {
            if (0.0..=1.0).contains(&left) {
                let used_pct = (1.0 - left) * 100.0;
                let reset = data
                    .get("weekly_usage_reset_time")
                    .and_then(extract_reset_ms);
                rows.push(QuotaRow {
                    label: t!("row.weekly").to_string(),
                    utilization: Some(used_pct),
                    remaining: None,
                    used: None,
                    total: None,
                    resets_at: reset,
                    unit: Some("%".to_string()),
                    extra: None,
                    kind: None,
                });
            }
        }
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    Ok(ProviderSnapshot {
        // v0.3: 用 source_id ("stepfun") 替代旧 "minimax" 占位
        provider: "stepfun".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(rate_raw.clone()),
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
    })
}

/// credit 套餐的重置 / 到期时间（2026-07-28 真实响应实测字段）。
///
/// 优先级（返回 `(Some(ms), is_expire)`，都没有 → `(None, false)`）：
/// 1. `subscription_credit_reset_time > 0` → 订阅制周期**重置**（Mini/Pro
///    按月 refill），is_expire=false
/// 2. bucket 里最早的 `next_reset_at > 0` → bucket 周期重置，is_expire=false
/// 3. bucket 里最早的 `expire_at > 0` → 一次性额度包**到期**（实测：
///    `credit_total=400000000 / expire_at≈7天后 / next_reset_at="0"`，
///    到期即作废不 refill），is_expire=true
///
/// 「重置」语义走浮窗默认 prefix（label + 「重置」后缀）；「到期」语义由
/// 调用方在 `extra.reset_period` 塞 `"expire"`，浮窗据此显示「到期」。
fn credit_plan_reset(data: &Value) -> (Option<i64>, bool) {
    let credit = match data.get("plan_credit_rate_limit") {
        Some(c) => c,
        None => return (None, false),
    };

    if let Some(ms) = credit
        .get("subscription_credit_reset_time")
        .and_then(extract_reset_ms)
    {
        return (Some(ms), false);
    }

    if let Some(arr) = credit.get("credit_buckets").and_then(|x| x.as_array()) {
        let mut resets: Vec<i64> = arr
            .iter()
            .filter_map(|b| b.get("next_reset_at").and_then(extract_reset_ms))
            .collect();
        resets.sort_unstable();
        if let Some(&ms) = resets.first() {
            return (Some(ms), false);
        }

        let mut expires: Vec<i64> = arr
            .iter()
            .filter_map(|b| b.get("expire_at").and_then(extract_reset_ms))
            .collect();
        expires.sort_unstable();
        if let Some(&ms) = expires.first() {
            return (Some(ms), true);
        }
    }

    (None, false)
}

/// 解析 credit 套餐的 `left_rate`：subscription > topup > bucket 加权平均。
fn credit_plan_left_rate(data: &Value) -> Option<f64> {
    let credit = data.get("plan_credit_rate_limit")?;

    if let Some(v) = credit
        .get("subscription_credit_left_rate")
        .and_then(flex_f64)
    {
        return Some(v);
    }
    if let Some(v) = credit.get("topup_credit_left_rate").and_then(flex_f64) {
        return Some(v);
    }
    // 兜底：credit_buckets 加权平均 (residual / total)
    if let Some(arr) = credit.get("credit_buckets").and_then(|x| x.as_array()) {
        if !arr.is_empty() {
            let mut sum_r = 0.0_f64;
            let mut sum_t = 0.0_f64;
            for b in arr {
                let r = b.get("credit_residual").and_then(flex_f64);
                let t = b.get("credit_total").and_then(flex_f64);
                if let (Some(r), Some(t)) = (r, t) {
                    sum_r += r;
                    sum_t += t;
                }
            }
            if sum_t > 0.0 {
                return Some(sum_r / sum_t);
            }
        }
    }
    None
}

// ── Auth helpers (CodexBar reverse-engineered) ────────────────────
//
// StepFun dashboard 端点要求一组浏览器侧 headers,缺 Oasis-Webid 时
// 服务端会无差别返 401/403。本节把"用户粘的 token 字符串"加工成
// 可正常鉴权的请求,并提供本地 exp 预检 + 友好错误。

/// 把 "sessionKey=xxx" / 纯 "xxx" / 整段 cookie 字符串 / 多行粘贴
/// 统一规整成纯 token value。失败时返 `None`,让调用方走"未配置"路径。
///
/// 防御性：
/// - 用户可能只复制 value（最常见）
/// - 用户可能整段复制 `Oasis-Token=xxx; yyy=zzz`
/// - 用户可能整段复制 `Cookie: Oasis-Token=xxx; yyy=zzz`（DevTools
///   右键 → Copy headers 会带前缀）
/// - 多行粘贴取第一行非空（与 `saveCredentialAction` 行为一致）
fn normalize_oasis_token(raw: &str) -> Option<String> {
    // 多行粘贴：取第一行非空
    let first_line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let s = first_line.trim();
    if s.is_empty() {
        return None;
    }

    // 整段 cookie 形式（带 `;`） → 拆出 Oasis-Token 的 value
    if s.contains(';') {
        for part in s.split(';') {
            // 容忍 "Cookie: Oasis-Token=xxx" 整段带前缀
            let p = part.trim().trim_start_matches("Cookie:").trim();
            if let Some((k, v)) = p.split_once('=') {
                if k.trim().eq_ignore_ascii_case("Oasis-Token") {
                    let v = v.trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }

    // "Oasis-Token=xxx" 无 sibling 段
    for prefix in ["Oasis-Token=", "oasis-token="] {
        if let Some(v) = s.strip_prefix(prefix) {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }

    Some(s.to_string())
}

/// 从 combined token 拆出 refresh 半段。无 `...` = 老格式 / 手动粘贴的
/// 裸 access → None（不能续期，只能等过期后重登）。
fn refresh_half(token: &str) -> Option<&str> {
    token
        .split_once(TOKEN_SEP)
        .map(|(_, r)| r.trim())
        .filter(|r| !r.is_empty())
}

/// 从 token 中提取 `device_id` claim（用作 Oasis-Webid）。
///
/// Token 两种形态（CodexBar `combinedToken`）：
/// - 单 JWT: `header.payload.sig`
/// - 双 JWT 组合: `access_jwt...refresh_jwt`
///
/// 优先 refresh half 的 `device_id`（CodexBar `webID(forToken:)`
/// 倒序遍历）。任一半解析失败就跳过。
fn device_id_for_token(token: &str) -> Option<String> {
    let halves: Vec<&str> = if token.contains(TOKEN_SEP) {
        // combined access...refresh: 反序,先试 refresh
        let mut h: Vec<&str> = token.split(TOKEN_SEP).collect();
        h.reverse();
        h
    } else if token.contains('.') {
        // 单 JWT
        vec![token]
    } else {
        return None;
    };

    for half in halves {
        if let Some(id) = jwt_device_id(half) {
            return Some(id);
        }
    }
    None
}

/// 从单个 JWT 字符串中提取 `device_id` claim。不做签名校验（参考
/// CodexBar —— 这是 web 客户端正常做法）。
fn jwt_device_id(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload_b64 = parts.next()?;
    // 容忍带 padding 的 base64url
    let payload_b64 = payload_b64.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("device_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// 本地预检 access token 的 `exp` claim。
///
/// 返回 `Some(secs)`：
/// - `secs >= 0` → 已过期 X 秒（让 `fetch` 走"已过期"错误路径）
/// - `secs < 0` → 距过期还有 -X 秒（即还有 X 秒有效，本函数不阻止请求，
///   仅在返回时调用方可以据此决定是否加 log）
///
/// 解析不出 exp 时返 `None`（交给服务端校验）。
///
/// `pub(crate)`：`stepfun_login.rs` 的「新鲜度门」复用同一判定，保证
/// 「登录存下来的 token」和「provider 预检接受的 token」标准一致。
pub(crate) fn access_token_exp_seconds_ago(token: &str) -> Option<i64> {
    // 只看 access half（combined token 的第一段），因为 exp 是 access 的
    let access = token.split(TOKEN_SEP).next().unwrap_or(token);
    let payload_b64 = access.split('.').nth(1)?;
    let payload_b64 = payload_b64.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v.get("exp").and_then(|x| x.as_i64())?;
    Some(Utc::now().timestamp() - exp)
}

/// 宽松数字解析：int / float / **数字字符串** 都吃（StepFun API 会把数字
/// 序列化成字符串，如 `"400000000"` —— CodexBar `StepFunFlexibleNumber`
/// 同款防御）。
fn flex_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// 宽松整数解析：int / 数字字符串（`status` / `code` / `plan_family` 用）。
fn flex_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// 提取 resets_at 为毫秒。接受 ISO 8601 字符串（首选）、epoch 数字
/// （秒/毫秒自适应）、**epoch 数字字符串**（CodexBar `StepFunFlexibleTimestamp`
/// 同款，如 `"1777528800"`）。
fn extract_reset_ms(v: &Value) -> Option<i64> {
    if let Some(s) = v.as_str() {
        let s = s.trim();
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.timestamp_millis());
        }
        if let Ok(n) = s.parse::<i64>() {
            return epoch_to_ms(n);
        }
        return None;
    }
    if let Some(n) = v.as_i64() {
        return epoch_to_ms(n);
    }
    None
}

/// epoch 秒 / 毫秒 → 毫秒。`n <= 0` 视为「无窗口配置」（credit 套餐的
/// rate-window 字段就是 0 —— CodexBar 注释：NOT "fully consumed"）返 None。
fn epoch_to_ms(n: i64) -> Option<i64> {
    if n <= 0 {
        return None;
    }
    let ms = if n < 1_000_000_000_000 { n * 1000 } else { n };
    DateTime::<Utc>::from_timestamp_millis(ms).map(|_| ms)
}

/// 截断响应体用于诊断日志（防超长 body 爆日志）。
fn truncate_body(body: &str, max: usize) -> String {
    body.chars().take(max).collect()
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_full_response() {
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 0.72,
                "weekly_usage_left_rate": 0.55,
                "five_hour_usage_reset_time": "2026-06-16T18:30:00Z",
                "weekly_usage_reset_time": "2026-06-19T03:00:00Z"
            }
        });
        let snap =
            parse(raw.clone(), Some("Plus".to_string()), "stepfun", "StepFun").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("stepfun"));
        assert_eq!(snap.plan_name.as_deref(), Some("Plus"));
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        // 1.0 - 0.72 = 0.28 → 28%
        assert!((five_h.utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(five_h.unit.as_deref(), Some("%"));
        assert!(five_h.resets_at.is_some());

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, t!("row.weekly"));
        // 1.0 - 0.55 = 0.45 → 45%
        assert!((weekly.utilization.unwrap() - 45.0).abs() < 0.001);
    }

    #[test]
    fn parse_only_five_hour() {
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 0.9,
                "five_hour_usage_reset_time": "2026-06-16T18:30:00Z"
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.five_hour").as_ref());
        assert!((snap.rows[0].utilization.unwrap() - 10.0).abs() < 0.001);
        assert_eq!(snap.plan_name, None);
    }

    #[test]
    fn parse_zero_left_rate_is_full() {
        // 0.0 = 100% used
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 0.0,
                "weekly_usage_left_rate": 0.0
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        for row in &snap.rows {
            assert!((row.utilization.unwrap() - 100.0).abs() < 0.001);
        }
    }

    #[test]
    fn parse_left_rate_one_is_zero_used() {
        // 1.0 = 0% used (clean state)
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": 1.0
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert!((snap.rows[0].utilization.unwrap() - 0.0).abs() < 0.001);
    }

    #[test]
    fn parse_out_of_range_left_rate_is_skipped() {
        // -0.5 / 1.5 视为异常 → 跳过
        let raw = json!({
            "code": 0,
            "data": {
                "five_hour_usage_left_rate": -0.5,
                "weekly_usage_left_rate": 0.5
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        // 5h 跳过，只剩 weekly
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.weekly"));
    }

    #[test]
    fn parse_no_data_is_error() {
        let raw = json!({ "code": 0 });
        let err = parse(raw, None, "stepfun", "StepFun").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_code_nonzero_is_error() {
        // 业务级 code != 0 应在 fetch_rate_limit 阶段就报错（这里 raw 直接 parse 不会触发）
        // parse 本身只检查用量字段
        let raw = json!({ "code": 401, "message": "token expired" });
        let err = parse(raw, None, "stepfun", "StepFun").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind); // 无用量字段 → parse 错
    }

    #[test]
    fn extract_reset_ms_handles_iso() {
        let v = json!("2026-06-16T18:30:00Z");
        let ms = extract_reset_ms(&v).expect("iso");
        assert!(ms > 1_780_000_000_000 && ms < 1_800_000_000_000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_seconds() {
        let v = json!(1_750_000_000_i64);
        let ms = extract_reset_ms(&v).expect("secs");
        assert_eq!(ms, 1_750_000_000_000);
    }

    #[test]
    fn extract_reset_ms_handles_epoch_millis() {
        let v = json!(1_750_000_000_000_i64);
        let ms = extract_reset_ms(&v).expect("ms");
        assert_eq!(ms, 1_750_000_000_000);
    }

    #[test]
    fn extract_reset_ms_invalid_returns_none() {
        assert_eq!(extract_reset_ms(&json!("not a date")), None);
        assert_eq!(extract_reset_ms(&json!(null)), None);
    }

    // ── normalize_oasis_token ──

    #[test]
    fn normalize_oasis_token_plain() {
        assert_eq!(
            normalize_oasis_token("eyJhbGciOiJIUzI1NiJ9.eyJkZXZpY2VfaWQiOiJhYmMifQ.sig"),
            Some("eyJhbGciOiJIUzI1NiJ9.eyJkZXZpY2VfaWQiOiJhYmMifQ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_with_prefix() {
        assert_eq!(
            normalize_oasis_token("Oasis-Token=eyJ.eyJ.sig"),
            Some("eyJ.eyJ.sig".to_string())
        );
        assert_eq!(
            normalize_oasis_token("oasis-token=eyJ.eyJ.sig"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_full_cookie_string() {
        assert_eq!(
            normalize_oasis_token("Oasis-Token=eyJ.eyJ.sig; other=zzz"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_with_leading_cookie_header() {
        assert_eq!(
            normalize_oasis_token("Cookie: Oasis-Token=eyJ.eyJ.sig; foo=bar"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_multiline_first_line() {
        assert_eq!(
            normalize_oasis_token("\n\n  eyJ.eyJ.sig  \nOasis-Webid=yyy"),
            Some("eyJ.eyJ.sig".to_string())
        );
    }

    #[test]
    fn normalize_oasis_token_empty_returns_none() {
        assert_eq!(normalize_oasis_token(""), None);
        assert_eq!(normalize_oasis_token("   \n  "), None);
    }

    // ── device_id_for_token / jwt_device_id ──

    fn make_jwt_with_claims(claims: &str) -> String {
        // 任意 header / sig;只关心 payload 解析
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload = URL_SAFE_NO_PAD.encode(claims.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(b"sig");
        format!("{header}.{payload}.{sig}")
    }

    #[test]
    fn device_id_for_single_jwt() {
        let jwt = make_jwt_with_claims(r#"{"device_id":"dev-abc"}"#);
        assert_eq!(device_id_for_token(&jwt), Some("dev-abc".to_string()));
    }

    #[test]
    fn device_id_for_combined_access_refresh() {
        // 模拟 access...refresh：refresh 含 device_id
        let access = make_jwt_with_claims(r#"{"sub":"u1","exp":1}"#);
        let refresh = make_jwt_with_claims(r#"{"device_id":"dev-xyz"}"#);
        let combined = format!("{access}...{refresh}");
        assert_eq!(device_id_for_token(&combined), Some("dev-xyz".to_string()));
    }

    #[test]
    fn device_id_prefers_refresh_when_both_have_it() {
        // 两半都含 device_id,CodexBar 倒序偏好 → refresh 胜
        let access = make_jwt_with_claims(r#"{"device_id":"dev-access"}"#);
        let refresh = make_jwt_with_claims(r#"{"device_id":"dev-refresh"}"#);
        let combined = format!("{access}...{refresh}");
        assert_eq!(
            device_id_for_token(&combined),
            Some("dev-refresh".to_string())
        );
    }

    #[test]
    fn device_id_returns_none_for_non_jwt() {
        assert_eq!(device_id_for_token("not-a-jwt"), None);
        assert_eq!(device_id_for_token(""), None);
    }

    #[test]
    fn device_id_returns_none_for_jwt_without_claim() {
        let jwt = make_jwt_with_claims(r#"{"sub":"u1"}"#);
        assert_eq!(device_id_for_token(&jwt), None);
    }

    // ── build_request headers ──

    #[test]
    fn build_request_includes_required_headers() {
        let client = reqwest::Client::new();
        let jwt = make_jwt_with_claims(r#"{"device_id":"dev-headers"}"#);
        let req = build_request(&client, URL_RATE_LIMIT, &jwt).build().unwrap();
        let headers = req.headers();

        // 必发 headers
        for name in [
            "cookie",
            "oasis-appid",
            "oasis-platform",
            "user-agent",
            "accept",
            "content-type",
        ] {
            assert!(
                headers.get(name).is_some(),
                "missing header: {name}\nheaders: {headers:?}"
            );
        }

        // Oasis-Webid 用首字母大写 + 全小写两种名(CodexBar 兼容)
        assert!(headers.get("Oasis-Webid").is_some());
        assert!(headers.get("oasis-webid").is_some());
        assert_eq!(
            headers.get("oasis-appid").unwrap().to_str().unwrap(),
            "10300"
        );

        // Cookie 头同时含 Oasis-Token= 和 Oasis-Webid=
        let cookie = headers.get("cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains("Oasis-Token="), "cookie: {cookie}");
        assert!(cookie.contains("Oasis-Webid=dev-headers"), "cookie: {cookie}");
    }

    #[test]
    fn build_request_falls_back_to_default_webid() {
        let client = reqwest::Client::new();
        let req = build_request(&client, URL_RATE_LIMIT, "not-a-jwt")
            .build()
            .unwrap();
        let cookie = req.headers().get("cookie").unwrap().to_str().unwrap();
        assert!(cookie.contains(&format!("Oasis-Webid={DEFAULT_WEBID}")));
    }

    // ── access_token_exp_seconds_ago ──

    #[test]
    fn access_exp_already_expired() {
        let now = Utc::now().timestamp();
        let claims = format!(r#"{{"exp":{}}}"#, now - 600); // 10 min ago
        let jwt = make_jwt_with_claims(&claims);
        let secs = access_token_exp_seconds_ago(&jwt).expect("exp");
        assert!(secs >= 590 && secs <= 620, "got {secs}");
    }

    #[test]
    fn access_exp_not_yet_expired_returns_negative() {
        let now = Utc::now().timestamp();
        let claims = format!(r#"{{"exp":{}}}"#, now + 3600); // 1h 之后
        let jwt = make_jwt_with_claims(&claims);
        let secs = access_token_exp_seconds_ago(&jwt).expect("exp");
        assert!(secs < 0, "got {secs}");
    }

    #[test]
    fn access_exp_no_claim_returns_none() {
        let jwt = make_jwt_with_claims(r#"{"sub":"u1"}"#);
        assert_eq!(access_token_exp_seconds_ago(&jwt), None);
    }

    #[test]
    fn access_exp_uses_access_half_when_combined() {
        // access.exp 已过期;refresh.exp 还很远 → 应该用 access 的
        let now = Utc::now().timestamp();
        let access = make_jwt_with_claims(&format!(r#"{{"exp":{}}}"#, now - 60));
        let refresh = make_jwt_with_claims(&format!(r#"{{"exp":{}}}"#, now + 86400));
        let combined = format!("{access}...{refresh}");
        let secs = access_token_exp_seconds_ago(&combined).expect("exp");
        assert!(secs >= 30 && secs <= 90, "got {secs}");
    }

    // ── Credit 套餐（plan_family == 2）──

    #[test]
    fn parse_credit_plan_uses_subscription_rate() {
        let raw = json!({
            "code": 0,
            "data": {
                "plan_family": 2,
                "plan_credit_rate_limit": {
                    "subscription_credit_left_rate": 0.8
                }
            }
        });
        let snap = parse(raw, Some("Mini".into()), "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.credit").as_ref());
        // 1 - 0.8 = 0.2 → 20%
        assert!((snap.rows[0].utilization.unwrap() - 20.0).abs() < 0.001);
        assert_eq!(snap.plan_name.as_deref(), Some("Mini"));
    }

    #[test]
    fn parse_credit_plan_falls_back_to_topup() {
        let raw = json!({
            "code": 0,
            "data": {
                "plan_family": 2,
                "plan_credit_rate_limit": {
                    "topup_credit_left_rate": 0.5
                }
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        // 1 - 0.5 = 0.5 → 50%
        assert!((snap.rows[0].utilization.unwrap() - 50.0).abs() < 0.001);
    }

    #[test]
    fn parse_credit_plan_weighted_buckets() {
        // bucket1: 20/100, bucket2: 30/100 → (20+30)/(100+100) = 0.25 → 75% used
        let raw = json!({
            "code": 0,
            "data": {
                "plan_family": 2,
                "plan_credit_rate_limit": {
                    "credit_buckets": [
                        { "credit_total": 100, "credit_residual": 20 },
                        { "credit_total": 100, "credit_residual": 30 }
                    ]
                }
            }
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert!((snap.rows[0].utilization.unwrap() - 75.0).abs() < 0.001);
    }

    #[test]
    fn parse_credit_plan_no_credit_data_is_error() {
        // plan_family==2 但 plan_credit_rate_limit 缺关键字段 → empty rows → 错误
        let raw = json!({
            "code": 0,
            "data": { "plan_family": 2 }
        });
        let err = parse(raw, None, "stepfun", "StepFun").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    // ── 2026-07-28 新 schema（status==1 成功标记 + 字段在顶层）──

    #[test]
    fn parse_new_schema_top_level_fields() {
        // CodexBar StepFunRateLimitResponse 形态：status==1，字段顶层，
        // reset 时间是 epoch 秒（字符串 / 数字混合）
        let raw = json!({
            "status": 1,
            "five_hour_usage_left_rate": 0.72,
            "weekly_usage_left_rate": 0.55,
            "five_hour_usage_reset_time": "1785221470",
            "weekly_usage_reset_time": 1785686400
        });
        let snap = parse(raw, Some("Plus".into()), "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert!((snap.rows[0].utilization.unwrap() - 28.0).abs() < 0.001);
        assert_eq!(snap.rows[0].resets_at, Some(1_785_221_470_000));
        assert!((snap.rows[1].utilization.unwrap() - 45.0).abs() < 0.001);
        assert_eq!(snap.rows[1].resets_at, Some(1_785_686_400_000));
        assert_eq!(snap.plan_name.as_deref(), Some("Plus"));
    }

    #[test]
    fn parse_new_schema_string_numbers() {
        // 数字字段序列化成字符串（CodexBar FlexibleNumber 同款防御）
        let raw = json!({
            "status": "1",
            "five_hour_usage_left_rate": "0.72",
            "weekly_usage_left_rate": "0.55"
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert!((snap.rows[0].utilization.unwrap() - 28.0).abs() < 0.001);
    }

    #[test]
    fn parse_new_schema_credit_plan_top_level() {
        let raw = json!({
            "status": 1,
            "plan_family": 2,
            "plan_credit_rate_limit": {
                "subscription_credit_left_rate": "0.8"
            }
        });
        let snap = parse(raw, Some("Mini".into()), "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.credit").as_ref());
        assert!((snap.rows[0].utilization.unwrap() - 20.0).abs() < 0.001);
    }

    #[test]
    fn parse_new_schema_zero_reset_is_none() {
        // credit 套餐 rate-window 字段是 0 / "0" = 「无窗口配置」，不能渲成 1970
        let raw = json!({
            "status": 1,
            "five_hour_usage_left_rate": 0.5,
            "five_hour_usage_reset_time": "0"
        });
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].resets_at, None);
    }

    // ── ensure_success 成功判定 ──

    #[test]
    fn ensure_success_status_one_ok() {
        let raw = json!({ "status": 1, "five_hour_usage_left_rate": 0.5 });
        assert!(ensure_success(&raw, "{}").is_ok());
    }

    #[test]
    fn ensure_success_legacy_code_zero_ok() {
        let raw = json!({ "code": 0, "data": { "five_hour_usage_left_rate": 0.5 } });
        assert!(ensure_success(&raw, "{}").is_ok());
    }

    #[test]
    fn ensure_success_status_nonzero_is_server_error() {
        let raw = json!({ "status": 0, "message": "no plan configured" });
        let err = ensure_success(&raw, "{}").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
        assert!(err.message.contains("no plan configured"), "msg: {}", err.message);
    }

    #[test]
    fn ensure_success_auth_message_maps_to_auth_failed() {
        // 业务层 auth 失败 → AuthFailed，让 do_fetch 的 refresh 兜底接管
        let raw = json!({ "status": -1, "message": "auth failed: oasis-token is embezzled" });
        let err = ensure_success(&raw, "{}").unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthFailed);
    }

    #[test]
    fn ensure_success_string_code_unauthenticated_is_auth() {
        // 200 + {"code":"unauthenticated",...}（字符串 code）→ AuthFailed
        let raw = json!({ "code": "unauthenticated", "message": "auth failed: token is illegal" });
        let err = ensure_success(&raw, "{}").unwrap_err();
        assert_eq!(err.kind, ErrorKind::AuthFailed);
    }

    #[test]
    fn ensure_success_tolerant_when_no_status_no_code() {
        // 既无 status 也无 code，但有用量字段 → 宽容放行
        let raw = json!({ "five_hour_usage_left_rate": 0.5 });
        assert!(ensure_success(&raw, "{}").is_ok());
        // 没有任何字段 → 拒绝
        let raw = json!({});
        assert!(ensure_success(&raw, "{}").is_err());
    }

    // ── flex 数字 / epoch 时间戳 ──

    #[test]
    fn flex_numbers_accept_string_forms() {
        assert_eq!(flex_f64(&json!("0.72")), Some(0.72));
        assert_eq!(flex_f64(&json!("400000000")), Some(400_000_000.0));
        assert_eq!(flex_f64(&json!(0.5)), Some(0.5));
        assert_eq!(flex_f64(&json!(1)), Some(1.0));
        assert_eq!(flex_f64(&json!("abc")), None);
        assert_eq!(flex_i64(&json!("2")), Some(2));
        assert_eq!(flex_i64(&json!(2)), Some(2));
        assert_eq!(flex_i64(&json!("x")), None);
    }

    #[test]
    fn extract_reset_ms_handles_string_epoch() {
        assert_eq!(extract_reset_ms(&json!("1785221470")), Some(1_785_221_470_000));
        assert_eq!(extract_reset_ms(&json!("1785221470000")), Some(1_785_221_470_000));
    }

    #[test]
    fn extract_reset_ms_zero_or_negative_is_none() {
        // "0" / 0 = 「无窗口配置」（credit 套餐 rate-window 字段），不能渲成 1970
        assert_eq!(extract_reset_ms(&json!("0")), None);
        assert_eq!(extract_reset_ms(&json!(0)), None);
        assert_eq!(extract_reset_ms(&json!(-5)), None);
    }

    // ── refresh 流程（pure function 部分）──

    #[test]
    fn refresh_half_splits_combined() {
        assert_eq!(refresh_half("aaa.bbb.ccc...ddd.eee.fff"), Some("ddd.eee.fff"));
        assert_eq!(refresh_half("aaa.bbb.ccc"), None);
        assert_eq!(refresh_half("aaa.bbb.ccc..."), None, "空 refresh 半段 → None");
    }

    #[test]
    fn parse_refresh_response_top_level() {
        let raw = json!({
            "accessToken": { "raw": "new-access" },
            "refreshToken": { "raw": "new-refresh" }
        });
        assert_eq!(
            parse_refresh_response(&raw, Some("old-refresh")),
            Some("new-access...new-refresh".to_string())
        );
    }

    #[test]
    fn parse_refresh_response_data_nested() {
        let raw = json!({
            "data": {
                "accessToken": { "raw": "new-access" },
                "refreshToken": { "raw": "new-refresh" }
            }
        });
        assert_eq!(
            parse_refresh_response(&raw, None),
            Some("new-access...new-refresh".to_string())
        );
    }

    #[test]
    fn parse_refresh_response_keeps_old_refresh_when_missing() {
        // 服务端没返新 refresh 半段 → 保留旧的（CodexBar 同款兜底）
        let raw = json!({ "accessToken": { "raw": "new-access" } });
        assert_eq!(
            parse_refresh_response(&raw, Some("old-refresh")),
            Some("new-access...old-refresh".to_string())
        );
    }

    #[test]
    fn parse_refresh_response_empty_access_is_none() {
        let raw = json!({ "accessToken": { "raw": "  " } });
        assert_eq!(parse_refresh_response(&raw, Some("old")), None);
        let raw = json!({ "status": 0, "message": "refresh token revoked" });
        assert_eq!(parse_refresh_response(&raw, Some("old")), None);
    }

    // ── credit 套餐重置 / 到期时间（2026-07-28 真实响应实测）──

    /// 用户真实响应形态：单个一次性额度包，expire_at ≈ 7 天后，
    /// next_reset_at / subscription_credit_reset_time 都是 "0"。
    const REAL_CREDIT_RESPONSE: &str = r#"{
        "desc": "",
        "five_hour_usage_left_rate": 0,
        "five_hour_usage_reset_time": "0",
        "plan_credit_rate_limit": {
            "credit_buckets": [
                {
                    "credit_residual": "374369257",
                    "credit_total": "400000000",
                    "expire_at": "1785831838",
                    "next_reset_at": "0",
                    "type": 1
                }
            ],
            "subscription_credit_left_rate": 0.9359231,
            "subscription_credit_reset_time": "0",
            "topup_credit_left_rate": 0
        },
        "plan_family": 2,
        "status": 1,
        "weekly_usage_left_rate": 0,
        "weekly_usage_reset_time": "0"
    }"#;

    #[test]
    fn parse_real_credit_response_shows_expire() {
        let raw: Value = serde_json::from_str(REAL_CREDIT_RESPONSE).unwrap();
        let snap = parse(raw, None, "stepfun", "StepFun").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        let row = &snap.rows[0];
        // 374369257/400000000 = 0.93592… → used ≈ 6.4%
        assert!((row.utilization.unwrap() - 6.407689999999999).abs() < 0.001);
        // expire_at=1785831838 (epoch 秒字符串) → resets_at + expire 标记
        assert_eq!(row.resets_at, Some(1_785_831_838_000));
        assert_eq!(
            row.extra
                .as_ref()
                .and_then(|e| e.get("reset_period"))
                .and_then(|v| v.as_str()),
            Some("expire"),
            "一次性额度包 → extra.reset_period=expire（浮窗显示「到期」）"
        );
    }

    #[test]
    fn credit_reset_prefers_subscription_reset_time() {
        // subscription_credit_reset_time > 0 → 周期重置语义（非 expire）
        let raw = json!({
            "plan_credit_rate_limit": {
                "subscription_credit_reset_time": "1787068799",
                "credit_buckets": [
                    { "expire_at": "1785831838", "next_reset_at": "0" }
                ]
            }
        });
        let (ms, is_expire) = credit_plan_reset(&raw);
        assert_eq!(ms, Some(1_787_068_799_000));
        assert!(!is_expire, "订阅制周期重置 → 非 expire");
    }

    #[test]
    fn credit_reset_falls_back_to_bucket_next_reset_then_expire() {
        // 无 subscription reset，bucket next_reset_at > 0 → 重置语义
        let raw = json!({
            "plan_credit_rate_limit": {
                "subscription_credit_reset_time": "0",
                "credit_buckets": [
                    { "expire_at": "1785831838", "next_reset_at": "1785686400" }
                ]
            }
        });
        let (ms, is_expire) = credit_plan_reset(&raw);
        assert_eq!(ms, Some(1_785_686_400_000));
        assert!(!is_expire);

        // 多 bucket：取最早 expire_at
        let raw = json!({
            "plan_credit_rate_limit": {
                "credit_buckets": [
                    { "expire_at": "1787068799", "next_reset_at": "0" },
                    { "expire_at": "1785831838", "next_reset_at": "0" }
                ]
            }
        });
        let (ms, is_expire) = credit_plan_reset(&raw);
        assert_eq!(ms, Some(1_785_831_838_000));
        assert!(is_expire);
    }

    #[test]
    fn credit_reset_all_zero_is_none() {
        let raw = json!({
            "plan_credit_rate_limit": {
                "subscription_credit_reset_time": "0",
                "credit_buckets": [
                    { "expire_at": "0", "next_reset_at": "0" }
                ]
            }
        });
        assert_eq!(credit_plan_reset(&raw), (None, false));
        // 无 plan_credit_rate_limit
        assert_eq!(credit_plan_reset(&json!({})), (None, false));
    }
}
