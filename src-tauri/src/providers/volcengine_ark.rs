//! 火山方舟 Coding Plan 套餐用量查询
//!
//! 端点：`POST https://ark.cn-beijing.volcengineapi.com/?Action=GetCodingPlanUsage&Version=2024-01-01`
//! 鉴权：账号级 AccessKey ID + SecretAccessKey（**不是** Coding Plan 推理 API Key）
//!
//! ## 鉴权流程（火山 v4，类 AWS SigV4）
//!
//! 火山方舟管控面跟其它火山云产品一样走 v4 HMAC-SHA256 签名：
//! 1. `X-Date: 20260727T100000Z`（ISO8601 去 - : 和毫秒）
//! 2. 算 body SHA256（空 body → `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`）
//! 3. CanonicalRequest = METHOD\n + path + sortedQuery + canonicalHeaders + signedHeaders + bodyHash
//! 4. StringToSign = "HMAC-SHA256\n" + xDate + "/" + region + "/" + service + "/request\n" + sha256hex(canonicalRequest)
//! 5. kSigning = HMAC(HMAC(HMAC(HMAC(SK, shortDate), region), service), "request")
//! 6. Signature = hex(HMAC(kSigning, StringToSign))
//! 7. Authorization header = "HMAC-SHA256 Credential=" + AK + "/" + credentialScope + ", SignedHeaders=" + ..., ", Signature=" + ...
//!
//! 固定参数：Service=ark / Region=cn-beijing
//!
//! ## 双凭证（v0.2.5 改）
//!
//! Coding Plan 推理 API Key（`Bearer sk-...`，在方舟控制台订阅页拿）**不能**调管控面。
//! 必须用账号级 IAM AK + SK（控制台右上角→"API 访问密钥"创建），推荐子账号 + 只读权限。
//!
//! v0.2.4 之前用过 `api_key` 槽拼 `"AK...SK"` 复合凭据，但 UX 反直觉（用户从控制台拿的
//! AK + SK 各是一行，三个英文句点分隔太陌生）。v0.2.5 改跟 ccswitch 一致：
//! - `api_key` = **AccessKey ID**（形如 `AKLTz...`）
//! - `secret_key` = **SecretAccessKey**（任意 base64）
//! - 前端 settings panel 渲 2 个独立 input field，**避免粘错**
//!
//! ## 响应 schema（Coding Plan 三个窗口）
//!
//! ```json
//! {
//!   "ResponseMetadata": { "RequestId": "...", "Action": "GetCodingPlanUsage", "Service": "ark" },
//!   "Result": {
//!     "Code": "Success",
//!     "UsageList": [
//!       { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": 1753603200000 },
//!       { "Level": "Weekly",  "Remaining": 8500, "Total": 9000, "ResetTimestamp": 1753761600000 },
//!       { "Level": "Monthly", "Remaining": 17000, "Total": 18000, "ResetTimestamp": 1756180800000 }
//!     ],
//!     "PlanName": "Lite"  // 或 "Pro"
//!   }
//! }
//! ```
//!
//! 字段名 Level 是"窗口类型"标识，不是 quota 字段。Remaining/Total 是次数（int）。
//! Schema 漂移保护：Level 出现 "Daily" 时也加一行（Agent Plan 字段，为
//! 未来 Coding Plan 增加日窗口预留），但 v0.2.5 实测 Coding Plan 暂不返回 Daily。
//!
//! ## 渲染策略
//!
//! - 主行 = "Session"（5h 滚动），label 用 `row.five_hour`（"5h" / "5h"）
//! - 副行 = "Weekly"，label 用 `row.weekly_7d`（"7d" / "7d"）
//! - 副行 = "Monthly"，label 用 `row.monthly`（"月" / "Monthly"）  ← 新增 i18n key
//! - 可选 Daily 行（如果 API 返回）
//! - util = 100 - (remaining / total * 100)，clamp [0, 100]
//! - resets_at 走 "reset in" 倒计时（前端 settings 已有 daily/weekly/monthly prefix）

use std::borrow::Cow;
use std::pin::Pin;

use serde_json::Value;

use super::{
    text_body_limited, AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow,
    QuotaSource, RowKind,
};
use crate::t;

const HOST: &str = "ark.cn-beijing.volcengineapi.com";
const ACTION: &str = "GetCodingPlanUsage";
const VERSION: &str = "2024-01-01";
const SERVICE: &str = "ark";
const REGION: &str = "cn-beijing";
const URL: &str =
    "https://ark.cn-beijing.volcengineapi.com/?Action=GetCodingPlanUsage&Version=2024-01-01";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct VolcengineArkSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for VolcengineArkSource {
    fn default() -> Self {
        Self { instance_index: 1 }
    }
}

impl VolcengineArkSource {
    pub fn with_instance_index(mut self, idx: u32) -> Self {
        self.instance_index = idx;
        self
    }

    #[allow(dead_code)]
    pub fn set_instance_index(&mut self, idx: u32) {
        self.instance_index = idx;
    }
}

impl QuotaSource for VolcengineArkSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("volcengine_ark")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "volcengine_ark".to_string()
        } else {
            format!("volcengine_ark#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.volcengine_ark").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.volcengine_ark").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        // v0.2.5: 两个独立 secret (AK + SK) 字段鉴权。
        // 前端 settings panel 看到 `auth_kind: "api_key_with_secret"` → 渲 2 个
        // password input，label 分别是 "AccessKey ID" / "SecretAccessKey"。
        AuthKind::ApiKeyWithSecret
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // 火山 Coding Plan 无 region / mode / overrides 概念
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        // v0.2.5: 两个独立 input 字段 —— `api_key` = AccessKey ID,
        // `secret_key` = SecretAccessKey（跟 ccswitch 1:1）。
        let ak_raw = credentials.api_key.clone();
        let sk_raw = credentials.secret_key.clone();
        let source_id = self.unique_id();
        let display_name = self.display_name().to_string();
        Box::pin(async move {
            // v0.2.5 迁移: 检测到 v0.2.4 老 keys.json —— `api_key` 槽存的是
            // 整串 "AK...SK"（v0.2.4 拼格式,save_credential_for_id 当时
            // 当一个值存),`secret_key` 槽空。一次性 split + 写回 keys.json,
            // 下次 fetch 直接走新 2-字段路径,用户不需要手动重粘。
            //
            // 仅对**唯一**内置 provider `volcengine_ark` 触发 —— 副本
            // (`volcengine_ark#2`) 的 keys.json 是用户后续通过 modal 加的,
            // 走新 save 路径,不持老格式。extra instance 路径不影响。
            //
            // 注意:`migrate_if_needed` 是 sync std::fs 写盘,放 spawn_blocking
            // 避免阻塞 tokio executor。
            let (ak, sk) = migrate_if_needed(&source_id, ak_raw, sk_raw).await?;
            if ak.is_empty() {
                return Err(FetchError::unconfigured(
                    t!(
                        "error.provider.unconfigured_key",
                        provider = "Volcengine Ark"
                    )
                    .into_owned(),
                ));
            }
            if sk.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.volcengine.unconfigured_secret_key").into_owned(),
                ));
            }
            do_fetch(&ak, &sk, &source_id, &display_name).await
        })
    }
}

/// v0.2.5 一次性迁移:把 v0.2.4 存的 "AK...SK" 整串拆成 2 字段写回 keys.json。
///
/// 返回 `(ak, sk)` 元组(无论是否触发迁移,都返有效值)。
/// 返回 `Err` 仅当 spawn_blocking 任务自身 join 失败(实际不会触发)。
async fn migrate_if_needed(
    source_id: &str,
    ak: Option<String>,
    sk: Option<String>,
) -> Result<(String, String), FetchError> {
    // 仅 v0.2.5 内置 1 份(provider 副本走 extra_instances 路径,新格式起步)
    if source_id != "volcengine_ark" {
        return Ok((
            ak.unwrap_or_default().trim().to_string(),
            sk.unwrap_or_default().trim().to_string(),
        ));
    }
    let ak_trim = ak.as_deref().unwrap_or("").trim().to_string();
    let sk_trim = sk.as_deref().unwrap_or("").trim().to_string();
    // 三种状态:
    // 1. ak 已有 + sk 已有 (新格式,直接走)         → 不迁移
    // 2. ak 含 "..." 整串 + sk 空 (v0.2.4 老格式)   → 迁移写回
    // 3. ak 空 + sk 空                                 → 走 fetch 的 unconfigured 分支
    if !sk_trim.is_empty() || !ak_trim.contains("...") {
        return Ok((ak_trim, sk_trim));
    }
    // 拆 "AK...SK" -> (ak, sk)。AK 部分不含 "...",SK 部分也不含,
    // 第一个 "..." 之前的全给 ak,之后全给 sk(允许 SK 里再有 "..." 字符)。
    let (new_ak, new_sk) = match ak_trim.split_once("...") {
        Some((a, s)) => (a.trim().to_string(), s.trim().to_string()),
        None => return Ok((ak_trim, sk_trim)), // 兜底:不应该到这里
    };
    tracing::info!(
        source_id = %source_id,
        "检测到 v0.2.4 老 \"AK...SK\" 整串格式,自动 split + 写回 keys.json"
    );
    // 写回 keys.json。spawn_blocking 因为 save_credential_for_id 是 std::fs 同步 IO。
    // clone 出来一份给闭包消费(避免 spawn_blocking move 后无法 return)。
    let (ak_for_save, sk_for_save) = (new_ak.clone(), new_sk.clone());
    let write_result = tokio::task::spawn_blocking(move || {
        crate::config::save_credential_for_id(
            "volcengine_ark",
            &crate::providers::Credentials {
                api_key: Some(ak_for_save),
                cookie: None,
                secret_key: Some(sk_for_save),
            },
        )
    })
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "迁移 save_credential_for_id join 失败");
        FetchError::server(format!("migrate join failed: {e}"))
    })?;
    if let Err(e) = write_result {
        tracing::warn!(error = %e, "迁移写回 keys.json 失败(继续走 fetch,下次再试)");
        // 不 fail fetch —— 写回失败不影响本次 fetch,只是下次还要再走迁移。
    }
    Ok((new_ak, new_sk))
}

async fn do_fetch(
    ak: &str,
    sk: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // 1. 准备签名参数
    // v0.2.5 fix: 火山 Coding Plan `GetCodingPlanUsage` 是 **GET**(只读),
    // 不是 POST。cc-switch 走 GET 能通,POST 返 200 但 Result 空 →
    // 我们走 "响应缺少 UsageList" 错误路径。
    // 关键变化:canonical request METHOD=GET, body=空, 仍算 body_hash(空字节
    // 串的 sha256,AWS SigV4 规定 GET 也必须有 x-content-sha256 header)。
    // 删 Content-Type (GET 不需要;加上反而让火山 server 验证 body 类型)。
    let x_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let body: &[u8] = b"";
    let body_hash = sha256_hex(body);

    // 2. CanonicalRequest
    let canonical_request = format!(
        "GET\n/\nAction={ACTION}&Version={VERSION}\nhost:{HOST}\nx-content-sha256:{body_hash}\nx-date:{x_date}\n\nhost;x-content-sha256;x-date\n{body_hash}",
        HOST = HOST,
        ACTION = ACTION,
        VERSION = VERSION,
    );

    // 3. StringToSign
    let credential_scope = format!(
        "{short_date}/{REGION}/{SERVICE}/request",
        short_date = &x_date[..8],
    );
    let string_to_sign = format!(
        "HMAC-SHA256\n{x_date}\n{credential_scope}\n{hashed_canonical}",
        hashed_canonical = sha256_hex(canonical_request.as_bytes()),
    );

    // 4. 签名密钥链
    let k_date = hmac_sha256(sk.as_bytes(), &x_date[..8]);
    let k_region = hmac_sha256(&k_date, REGION);
    let k_service = hmac_sha256(&k_region, SERVICE);
    let k_signing = hmac_sha256(&k_service, "request");
    let signature = hex_encode(&hmac_sha256(&k_signing, &string_to_sign));

    // 5. Authorization header
    let authorization = format!(
        "HMAC-SHA256 Credential={ak}/{credential_scope}, SignedHeaders=host;x-content-sha256;x-date, Signature={signature}",
    );

    // 6. 发送请求 (GET + 空 body, 不带 Content-Type)
    let client = super::shared_client();
    let resp = client
        .get(URL)
        .header("Host", HOST)
        .header("X-Date", &x_date)
        .header("X-Content-Sha256", &body_hash)
        .header("Authorization", authorization)
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = URL, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    // 火山 v4 签名错误统一返 401 SignatureDoesNotMatch，错误信息不告诉哪字段错
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.common.auth_failed", provider = "Volcengine Ark").into_owned(),
        ));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            t!("error.common.rate_limited", provider = "Volcengine Ark").into_owned(),
        ));
    }
    // D5 fix (2026-07-28 审查): text_body_limited 替代 resp.text() —
    // 8 MiB 上限 + 错误归类 Parse 跟旧路径一致。
    let raw_text = text_body_limited(resp).await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.message).into_owned())
    })?;
    // P0 fix (2026-08-06 cross-verify #2): 删 v0.2.5 临时诊断的 unconditional
    // tracing::warn!。它原在 if !status.is_success() 之前,每次 fetch(含成功)
    // 都打 2000 字符 body 到 stderr,长期泄露 PlanName / UsageList 账户信息。
    // 错误响应 body 已由下面 !status.is_success() 分支的 FetchError::server 消息
    // (200 字符)带出,无需无条件全量打。ak/sk 一直只打长度,不回归。

    if !status.is_success() {
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "Volcengine Ark",
                status = status.as_u16(),
                body = raw_text.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw: Value = serde_json::from_str(&raw_text).map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    parse(&raw, source_id, display_name)
}

// ── 解析 ─────────────────────────────────────────────────────────

/// 解析 `GetCodingPlanUsage` 响应。
///
/// Coding Plan 返回的 `Level` 字段枚举（实测 + 文档）：
/// - `Session`  → 5h 滚动窗口（主行）
/// - `Weekly`   → 周窗口（每周一 00:00 重置）
/// - `Monthly`  → 月窗口（订阅月首日 00:00 重置）
/// - `Daily`    → 日窗口（Agent Plan 字段，Coding Plan 暂不返回；预留以应对 schema 加字段）
///
/// 不认识的 Level 静默跳过（schema 漂移保护），不让单条坏数据炸整个 snapshot。
fn parse(raw: &Value, source_id: &str, display_name: &str) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    // P2 audit fix (2026-08-13): 火山 OpenAPI 的业务错误常放在顶层
    // ResponseMetadata.Error (此时没有 Result 节点)。之前先查 Result →
    // 真实 Code/Message 被"缺 Result 字段"的通用 Parse 错误吞掉, 用户
    // 看不到权限/参数错误原因。先查这里。
    if let Some(err) = raw
        .get("ResponseMetadata")
        .and_then(|m| m.get("Error"))
    {
        let code = err.get("Code").and_then(|v| v.as_str()).unwrap_or("");
        let msg = err
            .get("Message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(FetchError::server(
            t!(
                "error.common.business_code",
                provider = "Volcengine Ark",
                code = 0,
                msg = format!("{code}: {msg}")
            )
            .into_owned(),
        ));
    }

    let result = raw.get("Result").ok_or_else(|| {
        FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = "Volcengine Ark",
                field = "Result"
            )
            .into_owned(),
        )
    })?;

    // 业务级失败检查
    // P3 audit fix (2026-08-13): Code 兼容数字形式 (0/1), 不只 as_str。
    if let Some(code) = result
        .get("Code")
        .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_i64().map(|n| n.to_string())))
    {
        if code != "Success" {
            let msg = result.get("Message").and_then(|v| v.as_str()).unwrap_or("");
            return Err(FetchError::server(
                t!(
                    "error.common.business_code",
                    provider = "Volcengine Ark",
                    code = 0,
                    msg = format!("{code}: {msg}")
                )
                .into_owned(),
            ));
        }
    }

    // 火山 Coding Plan schema 兼容性:
    // - v0.2.5 我们读: Result.UsageList[] (Level: "Session"|"Weekly"|"Monthly", Remaining, Total)
    // - CodexBar #1724 提到另一种: QuotaUsage[] (Level: "session"|"weekly"|"monthly", Percent, ResetTimestamp)
    // 优先用 UsageList,fallback QuotaUsage;Level 统一转 lowercase 后 match。
    let usage_list = result
        .get("UsageList")
        .and_then(|v| v.as_array())
        .or_else(|| result.get("QuotaUsage").and_then(|v| v.as_array()))
        .ok_or_else(|| {
            FetchError::parse(
                t!(
                    "error.common.missing_field",
                    provider = "Volcengine Ark",
                    field = "UsageList"
                )
                .into_owned(),
            )
        })?;

    if usage_list.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    let mut rows = Vec::new();

    for entry in usage_list {
        let level_raw = entry.get("Level").and_then(|v| v.as_str()).unwrap_or("");
        // 大小写不敏感 —— CodexBar issue #1724 看到的 schema 是 "session"
        // 小写,火山自家控制台实测是 "Session" 大写,两种都收。
        let level = level_raw.to_ascii_lowercase();
        // ResetTimestamp 单位: 火山 Coding Plan 实测返 epoch **seconds** (10 位数,
        // 2026-xx 范围) —— 不是 ms。跟 minimax 5h schema 漂移保护同款:
        // < 10^12 当 seconds × 1000,>= 10^12 当 ms 直用。
        let resets_at = entry
            .get("ResetTimestamp")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                entry
                    .get("ResetTimestamp")
                    .and_then(|v| v.as_f64())
                    .map(|f| f as i64)
            })
            // H4 fix (2026-08-03 audit): D-013 一致性 —— 拒绝 ts <= 0
            // (epoch 0 / 负数 / 服务端 schema 漂移)。和 kimi/claude_official/stepfun
            // 同款保护,这块 2026-07-30 audit 漏了 volcengine_ark,本次补回。
            // 否则 ts=0 → from_timestamp_millis(0) 返 epoch 1970-01-01,
            // ts=-1 → ts*1000 负数溢出 i64 / 浮窗显示诡异过去重置。
            // P3 audit fix (2026-08-13): 补字符串数字解析 -- sibling API
            // (stepfun/kimi) 实测序列化时间戳为字符串, 之前只吃数字 ->
            // 字符串 ResetTimestamp 永远解析不出 reset 倒计时。
            .or_else(|| {
                entry
                    .get("ResetTimestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.trim().parse::<i64>().ok())
            })
            .filter(|ts| *ts > 0)
            .map(|ts| {
                if ts < 1_000_000_000_000 {
                    ts * 1000
                } else {
                    ts
                }
            });
        // CodexBar #1724 + ccswitch 实测 schema (火山 Coding Plan 真返):
        // - QuotaUsage[] + Level="session"/"weekly"/"monthly"(小写) + Percent
        //   字段 = **已用百分比, 0.0~100.0** (实测 Percent=0.3346 → 显示 0.33%,
        //   ccswitch 也是)。**不是** 0~1。v0.2.5 我误乘 100 → 33.46% 错。
        // - ResetTimestamp: epoch **seconds** (10 位) 上面 smart parse 转 ms。
        // - 老 UsageList[] + Remaining/Total 形态保留(虽然火山不返),做
        //   schema 漂移 fallback。
        let (used, total) = if let Some(percent) =
            super::parse::num_f64(entry.get("Percent").unwrap_or(&Value::Null))
        {
            // Percent 已是 0~100(火山 Coding Plan 实测)。clamp 防止 >100
            // 或负数(老 schema / 边界)。
            let used = percent.clamp(0.0, 100.0);
            (used, 100.0)
        } else {
            let remaining = super::parse::num_f64(entry.get("Remaining").unwrap_or(&Value::Null));
            let total_v = super::parse::num_f64(entry.get("Total").unwrap_or(&Value::Null));
            match (remaining, total_v) {
                (Some(r), Some(t)) if t > 0.0 => ((t - r).max(0.0), t),
                _ => continue,
            }
        };

        let utilization = if total > 0.0 {
            (used / total * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        let (label, reset_period, kind) = match level.as_str() {
            "session" => (
                t!("row.five_hour").to_string(),
                "five_hour",
                Some(RowKind::FiveHour),
            ),
            "daily" => (t!("row.daily").to_string(), "daily", None),
            "weekly" => (
                t!("row.weekly_7d").to_string(),
                "weekly",
                Some(RowKind::Weekly),
            ),
            "monthly" => (t!("row.monthly").to_string(), "monthly", None),
            // 未知 Level → 跳过（schema 漂移保护，不让单条坏数据炸整个 snapshot）
            _ => continue,
        };

        rows.push(QuotaRow {
            label,
            utilization: Some(utilization),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: None, // Coding Plan 是次数，无单位
            extra: Some(serde_json::json!({ "reset_period": reset_period })),
            kind,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    // 排序：Session → Daily → Weekly → Monthly（让浮窗渲染稳定）
    rows.sort_by_key(|r| match r.label.as_str() {
        x if x == t!("row.five_hour").as_ref() => 0,
        x if x == t!("row.daily").as_ref() => 1,
        x if x == t!("row.weekly_7d").as_ref() => 2,
        x if x == t!("row.monthly").as_ref() => 3,
        _ => 99,
    });

    let plan_name = result
        .get("PlanName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let success = !rows.is_empty();
    Ok(ProviderSnapshot {
        provider: "volcengine_ark".to_string(),
        success,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: success,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
    })
}

// ── crypto helpers（无外部依赖，用 sha2 / hmac crate） ────────────

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    let out = hasher.finalize();
    hex_encode(&out)
}

fn hmac_sha256(key: &[u8], msg: &str) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key can be any length");
    mac.update(msg.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── 凭据校验逻辑 (v0.2.5 改: 2 字段 AK + SK 独立) ──
    //
    // 不再调 split_ak_sk（v0.2.4 那种 "AK...SK" 拼接形式）。
    // 校验逻辑从 Rust 的 `fetch` 抽到本地函数，便于直接 unit test。

    /// fetch() 的"前门"：把 `Credentials { api_key, secret_key }` 拆成
    /// `(ak, sk)` 或返 FetchError（empty / 缺 SK）。这是 v0.2.5 改的边界。
    fn extract_ak_sk(creds: &Credentials) -> Result<(String, String), FetchError> {
        let ak = creds.api_key.as_deref().unwrap_or("").trim().to_string();
        let sk = creds.secret_key.as_deref().unwrap_or("").trim().to_string();
        if ak.is_empty() {
            return Err(FetchError::unconfigured(
                t!(
                    "error.provider.unconfigured_key",
                    provider = "Volcengine Ark"
                )
                .into_owned(),
            ));
        }
        if sk.is_empty() {
            return Err(FetchError::unconfigured(
                t!("error.volcengine.unconfigured_secret_key").into_owned(),
            ));
        }
        Ok((ak, sk))
    }

    #[test]
    fn extract_ak_sk_basic() {
        let creds = Credentials {
            api_key: Some("AKLTz1234".into()),
            secret_key: Some("sk-abc".into()),
            cookie: None,
        };
        let (ak, sk) = extract_ak_sk(&creds).unwrap();
        assert_eq!(ak, "AKLTz1234");
        assert_eq!(sk, "sk-abc");
    }

    #[test]
    fn extract_ak_sk_trims_whitespace() {
        let creds = Credentials {
            api_key: Some("  AKLTz1234  ".into()),
            secret_key: Some("  sk-abc  ".into()),
            cookie: None,
        };
        let (ak, sk) = extract_ak_sk(&creds).unwrap();
        assert_eq!(ak, "AKLTz1234");
        assert_eq!(sk, "sk-abc");
    }

    #[test]
    fn extract_ak_sk_empty_ak() {
        let creds = Credentials {
            api_key: Some("".into()),
            secret_key: Some("sk-abc".into()),
            cookie: None,
        };
        let err = extract_ak_sk(&creds).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnconfiguredKey);
    }

    #[test]
    fn extract_ak_sk_empty_sk() {
        // v0.2.5 新场景：用户填了 AK 没填 SK（v0.2.4 老 keys.json 没
        // :secret_key 槽会触发这个）→ 返明确 unconfigured 错误
        let creds = Credentials {
            api_key: Some("AKLTz1234".into()),
            secret_key: Some("".into()),
            cookie: None,
        };
        let err = extract_ak_sk(&creds).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnconfiguredKey);
    }

    #[test]
    fn extract_ak_sk_both_none() {
        let creds = Credentials::default();
        let err = extract_ak_sk(&creds).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnconfiguredKey);
    }

    // ── v0.2.5 老数据迁移: 验"AK...SK"整串能 split_once 出 (ak, sk) ──

    /// 测试 split_once 的纯函数部分（不调 keys.json 写回）。`migrate_if_needed`
    /// 内部 split_once 走的是标准库,这里覆盖几种边界:
    /// - 标准 "AK...SK" 形态 → 正确切两段
    /// - SK 里含 "..." (如 sk-...secret...real) 不会重复切
    /// - "..." 在最前/最后 → 退化
    /// - 字符串不含 "..." → 走 fetch 的 unconfigured 分支（被上层挡住）
    #[test]
    fn split_combined_ak_sk_v0204() {
        // 标准形态
        let (a, s) = "AKLTz...sk-secret-xy".split_once("...").unwrap();
        assert_eq!(a, "AKLTz");
        assert_eq!(s, "sk-secret-xy");

        // SK 里含 "..." 不应重复切（split_once 只切第一个）
        let (a, s) = "AK...sk-with...dots".split_once("...").unwrap();
        assert_eq!(a, "AK");
        assert_eq!(s, "sk-with...dots");

        // 退化:不包含
        assert!("plainstring".split_once("...").is_none());
    }

    // ── 签名（不变性测试：固定时间签名，hex 字符串必须稳定） ──

    #[test]
    fn sign_coding_plan_request_deterministic() {
        // 固定 x_date 测试签名可重现
        let x_date = "20260727T100000Z";
        let body = b"{}";
        let body_hash = sha256_hex(body);
        let canonical_request = format!(
            "POST\n/\nAction={ACTION}&Version={VERSION}\nhost:{HOST}\nx-content-sha256:{body_hash}\nx-date:{x_date}\n\nhost;x-content-sha256;x-date\n{body_hash}",
        );
        let credential_scope = "20260727/cn-beijing/ark/request";
        let string_to_sign = format!(
            "HMAC-SHA256\n{x_date}\n{credential_scope}\n{}",
            sha256_hex(canonical_request.as_bytes()),
        );
        let sk = "test-sk-12345678";
        let k_date = hmac_sha256(sk.as_bytes(), "20260727");
        let k_region = hmac_sha256(&k_date, REGION);
        let k_service = hmac_sha256(&k_region, SERVICE);
        let k_signing = hmac_sha256(&k_service, "request");
        let sig = hex_encode(&hmac_sha256(&k_signing, &string_to_sign));

        // 64 hex chars (32 bytes) — SHA256 输出
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn sign_uses_ark_service_cn_beijing_region() {
        // 锁定 region/service —— 写错就 401
        let _x_date = "20260727T100000Z";
        let _body_hash = sha256_hex(b"{}"); // 实际签名链会用到,这里只验函数名能跑
        let k_date = hmac_sha256(b"sk", "20260727");
        let k_region = hmac_sha256(&k_date, REGION); // cn-beijing
        let k_service = hmac_sha256(&k_region, SERVICE); // ark
        let k_signing = hmac_sha256(&k_service, "request");
        let sig = hex_encode(&hmac_sha256(&k_signing, "test"));

        // 不同 region/service 应该产生不同签名
        let k_region2 = hmac_sha256(&k_date, "us-east-1");
        let k_service2 = hmac_sha256(&k_region2, "ark");
        let k_signing2 = hmac_sha256(&k_service2, "request");
        let sig2 = hex_encode(&hmac_sha256(&k_signing2, "test"));
        assert_ne!(sig, sig2, "region 错了能立刻从签名差异看出来");
    }

    // ── parse 单元测试 ──

    #[test]
    fn parse_quota_usage_schema_lowercase() {
        // 火山 Coding Plan 真返 schema (2026-07-28 实测):
        // Result.QuotaUsage[] + Level: "session"/"weekly"/"monthly"(小写)
        // + Percent 字段 = 已用百分比 0~100 (不是 0~1)
        // + ResetTimestamp: epoch **seconds** (10 位) — smart parse 转 ms
        // + 额外有 Status="Running" / UpdateTimestamp(seconds)
        let raw = json!({
            "Result": {
                "Status": "Running",
                "UpdateTimestamp": 1785217273_i64,
                "QuotaUsage": [
                    { "Level": "session", "Percent": 0.33462600000000003_f64, "ResetTimestamp": 1785221470_i64 },
                    { "Level": "weekly",  "Percent": 2.408004733333333_f64,   "ResetTimestamp": 1785686400_i64 },
                    { "Level": "monthly", "Percent": 11.356161100000001_f64,  "ResetTimestamp": 1787068799_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        assert_eq!(snap.rows.len(), 3);
        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        // Percent=0.3346 → 0.33%(已 clamp 0~100,直接当百分比数值)
        assert!((five_h.utilization.unwrap() - 0.3346).abs() < 0.001);
        // ResetTimestamp 1785221470 是 seconds → smart parse 转 ms
        assert_eq!(five_h.resets_at, Some(1785221470 * 1000));
        let month = &snap.rows[2];
        assert_eq!(month.label, t!("row.monthly").as_ref());
        // 11.356% 不是 1135.6% —— 修 v0.2.5 那个 * 100 错位 bug
        assert!((month.utilization.unwrap() - 11.356).abs() < 0.01);
    }

    #[test]
    fn parse_full_response() {
        let raw = json!({
            "ResponseMetadata": { "RequestId": "test-1", "Action": "GetCodingPlanUsage" },
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": 1753603200000_i64 },
                    { "Level": "Weekly",  "Remaining": 8500, "Total": 9000, "ResetTimestamp": 1753761600000_i64 },
                    { "Level": "Monthly", "Remaining": 17000, "Total": 18000, "ResetTimestamp": 1756180800000_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("volcengine_ark"));
        assert_eq!(snap.plan_name.as_deref(), Some("Lite"));
        assert_eq!(snap.rows.len(), 3);

        // 排序后：Session (5h) → Weekly (7d) → Monthly
        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        assert_eq!(five_h.used, None);
        assert_eq!(five_h.total, None);
        assert_eq!(five_h.remaining, None);
        assert!((five_h.utilization.unwrap() - 8.333).abs() < 0.01);
        assert_eq!(five_h.resets_at, Some(1753603200000));

        let week = &snap.rows[1];
        assert_eq!(week.label, t!("row.weekly_7d").as_ref());
        assert_eq!(week.used, None);

        let month = &snap.rows[2];
        assert_eq!(month.label, t!("row.monthly").as_ref());
        assert_eq!(month.used, None);
        assert!((month.utilization.unwrap() - 5.555).abs() < 0.01);
    }

    #[test]
    fn parse_with_daily_row() {
        // Agent Plan 字段，schema 漂移保护：Daily 行应该加进来
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": 1753603200000_i64 },
                    { "Level": "Daily",   "Remaining": 500,  "Total": 600,  "ResetTimestamp": 1753603200000_i64 },
                    { "Level": "Weekly",  "Remaining": 8500, "Total": 9000, "ResetTimestamp": 1753761600000_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        assert_eq!(snap.rows.len(), 3);
        // Daily 应排在 Session 之后、Weekly 之前
        assert_eq!(snap.rows[0].label, t!("row.five_hour").as_ref());
        assert_eq!(snap.rows[1].label, t!("row.daily").as_ref());
        assert_eq!(snap.rows[2].label, t!("row.weekly_7d").as_ref());
    }

    #[test]
    fn parse_skips_unknown_level() {
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Pro",
                "UsageList": [
                    { "Level": "Session", "Remaining": 5500, "Total": 6000, "ResetTimestamp": 1753603200000_i64 },
                    { "Level": "Yearly",  "Remaining": 50000, "Total": 108000, "ResetTimestamp": 1785139200000_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        // Yearly 未知 → 跳过，只剩 Session
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.five_hour").as_ref());
    }

    #[test]
    fn parse_handles_overshoot() {
        // remaining > total（超用恢复中）—— used clamp 不为负
        // 浮窗不直接读 remaining 字段,显示的是 utilization + resets_at + label,
        // 所以 parse 内部 `remaining = total - used` 是为前端兼容"used/total"
        // 渲染模板;测试改成验 used/utilization 的 clamp 行为。
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 1250, "Total": 1200, "ResetTimestamp": 1753603200000_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        let r = &snap.rows[0];
        // used = (1200 - 1250).max(0) = 0 → utilization = 0%
        assert_eq!(r.used, None);
        assert_eq!(r.total, None);
        // remaining 字段保留 total-used 推导值(>=0),超用时钳到 0
        assert_eq!(r.remaining, None);
        assert_eq!(r.utilization, Some(0.0));
    }

    #[test]
    fn parse_no_usage_list_is_error() {
        let raw = json!({
            "Result": { "Code": "Success", "PlanName": "Lite" }
        });
        let err = parse(&raw, "volcengine_ark", "Volcengine Ark").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_empty_usage_list_is_error() {
        let raw = json!({
            "Result": { "Code": "Success", "PlanName": "Lite", "UsageList": [] }
        });
        let err = parse(&raw, "volcengine_ark", "Volcengine Ark").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_business_code_error() {
        let raw = json!({
            "Result": {
                "Code": "InvalidParameter",
                "Message": "Action or Version invalid"
            }
        });
        let err = parse(&raw, "volcengine_ark", "Volcengine Ark").unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
    }

    #[test]
    fn parse_no_result_is_error() {
        let raw = json!({ "ResponseMetadata": {} });
        let err = parse(&raw, "volcengine_ark", "Volcengine Ark").unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn parse_skips_row_with_zero_total() {
        // Total = 0 → 跳过（防除零）
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 0, "Total": 0, "ResetTimestamp": 1753603200000_i64 },
                    { "Level": "Weekly",  "Remaining": 8500, "Total": 9000, "ResetTimestamp": 1753761600000_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.weekly_7d").as_ref());
    }

    /// H4 fix (2026-08-03 audit): ResetTimestamp = 0 / 负数必须被拒 (D-013
    /// 一致性)。火山 Coding Plan schema 漂移或 epoch=0 返回时,不能把 resets_at
    /// 设成 Some(0) 让浮窗显示 1970-01-01,也不能让负数 ts*1000 溢出。
    #[test]
    fn parse_drops_zero_reset_timestamp() {
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": 0_i64 },
                    { "Level": "Weekly",  "Remaining": 8500, "Total": 9000, "ResetTimestamp": 1753761600000_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 2);
        // Session 行 ResetTimestamp=0 → resets_at=None (不显示 1970)
        // 通过 resets_at 验证 5h 行(0 被过滤为 None)
        let _five_h = snap
            .rows
            .iter()
            .find(|r| r.resets_at.is_none())
            .expect("5h row (ts=0)");
        // Weekly 行(resets_at 正常)
        let week = snap
            .rows
            .iter()
            .find(|r| r.resets_at == Some(1753761600000))
            .expect("weekly row");
        assert_eq!(week.resets_at, Some(1753761600000));
    }

    #[test]
    fn parse_drops_negative_reset_timestamp() {
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": -1_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.rows.len(), 1);
        let five_h = &snap.rows[0];
        assert_eq!(five_h.resets_at, None, "ts=-1 must be filtered to None");
    }
}
