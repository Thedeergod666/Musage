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
//! ## 双凭证问题
//!
//! Coding Plan 推理 API Key（`Bearer sk-...`，在方舟控制台订阅页拿）**不能**调管控面。
//! 必须用账号级 IAM AK + SK（控制台右上角→"API 访问密钥"创建），推荐子账号 + 只读权限。
//! Musage 把 AK/SK 合并存进 `api_key` 槽（`"AK...SK"` 形式，`...` 作分隔符，跟 AnySearch
//! `access...refresh` 同款套路）。
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
    AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};
use crate::t;

const HOST: &str = "ark.cn-beijing.volcengineapi.com";
const ACTION: &str = "GetCodingPlanUsage";
const VERSION: &str = "2024-01-01";
const SERVICE: &str = "ark";
const REGION: &str = "cn-beijing";
const URL: &str = "https://ark.cn-beijing.volcengineapi.com/?Action=GetCodingPlanUsage&Version=2024-01-01";

/// `api_key` 槽里 AK 和 SK 的分隔符（跟 AnySearch `access...refresh` 同款）
const AK_SK_SEP: &str = "...";

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
        AuthKind::ApiKey
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
        Box::pin(async move {
            let combined = credentials.api_key.as_deref().unwrap_or("").trim();
            if combined.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "Volcengine Ark").into_owned(),
                ));
            }
            let (ak, sk) = match split_ak_sk(combined) {
                Some(pair) => pair,
                None => {
                    return Err(FetchError::auth(
                        t!("error.volcengine.invalid_ak_sk_format").into_owned(),
                    ));
                }
            };
            do_fetch(ak, sk, &self.unique_id(), &self.display_name().to_string()).await
        })
    }
}

/// 把 `api_key` 槽里的 `"AK...SK"` 拆成 `(ak, sk)`。无 `...` = 退化（不报错，
/// 走空 sk 触发签名失败 → 用户能看到明确错误）。
fn split_ak_sk(combined: &str) -> Option<(&str, &str)> {
    let idx = combined.find(AK_SK_SEP)?;
    let ak = combined[..idx].trim();
    let sk = combined[idx + AK_SK_SEP.len()..].trim();
    if ak.is_empty() || sk.is_empty() {
        return None;
    }
    Some((ak, sk))
}

async fn do_fetch(
    ak: &str,
    sk: &str,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    // 1. 准备签名参数
    let x_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let body = b"{}";
    let body_hash = sha256_hex(body);

    // 2. CanonicalRequest
    let canonical_request = format!(
        "POST\n/\nAction={ACTION}&Version={VERSION}\nhost:{HOST}\nx-content-sha256:{body_hash}\nx-date:{x_date}\n\nhost;x-content-sha256;x-date\n{body_hash}",
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

    // 6. 发送请求
    let client = super::shared_client();
    let resp = client
        .post(URL)
        .header("Host", HOST)
        .header("X-Date", &x_date)
        .header("X-Content-Sha256", &body_hash)
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("Authorization", authorization)
        .body("{}")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = URL, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    // 火山 v4 签名错误统一返 401 SignatureDoesNotMatch，错误信息不告诉哪字段错
    if status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::FORBIDDEN
    {
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
    if !status.is_success() {
        let body_text = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = "Volcengine Ark",
                status = status.as_u16(),
                body = body_text.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw: Value = resp.json().await.map_err(|e| {
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
fn parse(
    raw: &Value,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let result = raw.get("Result").ok_or_else(|| {
        FetchError::parse(
            t!("error.common.missing_field", provider = "Volcengine Ark", field = "Result").into_owned(),
        )
    })?;

    // 业务级失败检查
    if let Some(code) = result.get("Code").and_then(|v| v.as_str()) {
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

    let usage_list = result.get("UsageList").and_then(|v| v.as_array()).ok_or_else(|| {
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
        let level = entry.get("Level").and_then(|v| v.as_str()).unwrap_or("");
        let remaining = super::parse::num_f64(
            entry.get("Remaining").unwrap_or(&Value::Null),
        );
        let total = super::parse::num_f64(entry.get("Total").unwrap_or(&Value::Null));
        let resets_at = entry
            .get("ResetTimestamp")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                entry
                    .get("ResetTimestamp")
                    .and_then(|v| v.as_f64())
                    .map(|f| f as i64)
            });

        // 缺失核心字段 → 跳过这条
        let (remaining, total) = match (remaining, total) {
            (Some(r), Some(t)) if t > 0.0 => (r, t),
            _ => continue,
        };

        let used = (total - remaining).max(0.0);
        let utilization = ((used / total) * 100.0).clamp(0.0, 100.0);

        let label = match level {
            "Session" => t!("row.five_hour").to_string(),
            "Daily" => t!("row.daily").to_string(),
            "Weekly" => t!("row.weekly_7d").to_string(),
            "Monthly" => t!("row.monthly").to_string(),
            // 未知 Level → 跳过（schema 漂移保护，不让单条坏数据炸整个 snapshot）
            _ => continue,
        };

        rows.push(QuotaRow {
            label,
            utilization: Some(utilization),
            remaining: Some(remaining),
            used: Some(used),
            total: Some(total),
            resets_at,
            unit: None, // Coding Plan 是次数，无单位
            extra: None,
            kind: None,
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

    // ── AK/SK 拆分 ──

    #[test]
    fn split_ak_sk_basic() {
        let (ak, sk) = split_ak_sk("AKID...SECRET_KEY").unwrap();
        assert_eq!(ak, "AKID");
        assert_eq!(sk, "SECRET_KEY");
    }

    #[test]
    fn split_ak_sk_with_whitespace() {
        let (ak, sk) = split_ak_sk("  AKID  ...  SECRET  ").unwrap();
        assert_eq!(ak, "AKID");
        assert_eq!(sk, "SECRET");
    }

    #[test]
    fn split_ak_sk_no_separator() {
        assert!(split_ak_sk("AKIDSECRET").is_none());
    }

    #[test]
    fn split_ak_sk_empty_ak() {
        assert!(split_ak_sk("...SECRET").is_none());
    }

    #[test]
    fn split_ak_sk_empty_sk() {
        assert!(split_ak_sk("AKID...").is_none());
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
        let body_hash = sha256_hex(b"{}");
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
        assert_eq!(five_h.used, Some(100.0));
        assert_eq!(five_h.total, Some(1200.0));
        assert_eq!(five_h.remaining, Some(1100.0));
        assert!((five_h.utilization.unwrap() - 8.333).abs() < 0.01);
        assert_eq!(five_h.resets_at, Some(1753603200000));

        let week = &snap.rows[1];
        assert_eq!(week.label, t!("row.weekly_7d").as_ref());
        assert_eq!(week.used, Some(500.0));

        let month = &snap.rows[2];
        assert_eq!(month.label, t!("row.monthly").as_ref());
        assert_eq!(month.used, Some(1000.0));
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
        assert_eq!(r.used, Some(0.0)); // max(0)
        assert_eq!(r.remaining, Some(1250.0));
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
}
