//! 火山方舟 Coding + Agent 双套餐用量查询（v0.2.9 双 action）
//!
//! 端点（火山 OpenAPI 总网关，POST + v4 签名，ccswitch 同款）：
//!
//! - Coding Plan: `POST https://open.volcengineapi.com/?Action=GetCodingPlanUsage&Region=cn-beijing&Version=2024-01-01`
//! - Agent Plan: `POST https://open.volcengineapi.com/?Action=GetAFPUsage&Region=cn-beijing&Version=2024-01-01`
//!
//! 鉴权：账号级 AccessKey ID + SecretAccessKey（**不是** Coding Plan 推理 API Key）
//!
//! ## 鉴权流程（火山 v4，类 AWS SigV4）
//!
//! 火山方舟管控面跟其它火山云产品一样走 v4 HMAC-SHA256 签名：
//! 1. `X-Date: 20260727T100000Z`（ISO8601 去 - : 和毫秒）
//! 2. 算 body SHA256（空 body → `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`）
//! 3. CanonicalRequest = METHOD\n + path + sortedQuery + canonicalHeaders + signedHeaders + bodyHash
//!    （canonical headers 按字母序：content-type < host < x-content-sha256 < x-date）
//! 4. StringToSign = "HMAC-SHA256\n" + xDate + "/" + region + "/" + service + "/request\n" + sha256hex(canonicalRequest)
//! 5. kSigning = HMAC(HMAC(HMAC(HMAC(SK, shortDate), region), service), "request")
//! 6. Signature = hex(HMAC(kSigning, StringToSign))
//! 7. Authorization header = "HMAC-SHA256 Credential=" + AK + "/" + credentialScope + ", SignedHeaders=" + ..., ", Signature=" + ...
//!
//! 固定参数：Service=ark / Region=cn-beijing
//!
//! ## 双套餐（v0.2.9 改）
//!
//! 同一个 AppID 下可同时买 **Coding Plan** 和 **Agent Plan** 两份套餐，同一份
//! AK/SK 走不同 action 拿到不同数据。fetch 阶段按 `volcengine_ark_plan_filter`
//! 配置决定打哪些 action（默认两个都打），`tokio::join!` 并发、失败互相不连坐。
//! 行归属用 `extra.plan = "coding" | "agent"` 标记，每个套餐的数据行前插一行
//! `RowKind::PlanHeader` 标题行（无数据，仅供前端做视觉分组）。
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
//! ## Coding Plan 响应 schema（三个窗口）
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
//! ## Agent Plan（AFP）响应 schema（ccswitch 实测，火山无官方文档）
//!
//! ```json
//! {
//!   "Result": {
//!     "PlanType": "Large",
//!     "AFPFiveHour": { "Quota": 1000, "Used": 800, "ResetTime": 1778806800000 },
//!     "AFPWeekly":   { "Quota": 5000, "Used": 200, "ResetTime": 1779408000000 },
//!     "AFPMonthly":  { "Quota": 20000, "Used": 1500, "ResetTime": 1781990400000 }
//!   }
//! }
//! ```
//!
//! Quota <= 0 / 窗口缺失 = 未订阅该窗口，跳过；全部窗口都无数据 → 整个
//! Agent 组（含 PlanHeader）不出现。util = Used / Quota * 100。
//!
//! ## 渲染策略
//!
//! - Coding 在前、Agent 在后；每个套餐的数据行前各插一行 PlanHeader 标题行
//! - 套餐内行序：5h → (daily) → 7d → 月
//! - util = 100 - (remaining / total * 100)（Coding）/ (used / quota) * 100（Agent），clamp [0, 100]
//! - resets_at 走 "reset in" 倒计时（前端 settings 已有 daily/weekly/monthly prefix）

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::RwLock;

use serde_json::Value;

use super::{
    humanize_reqwest_err, text_body_limited, AuthKind, Credentials, ErrorKind, FetchError,
    ProviderSnapshot, QuotaRow, QuotaSource, RowKind,
};
use crate::t;

const HOST: &str = "open.volcengineapi.com";
const ACTION_CODING: &str = "GetCodingPlanUsage";
const ACTION_AFP: &str = "GetAFPUsage";
const VERSION: &str = "2024-01-01";
const SERVICE: &str = "ark";
const REGION: &str = "cn-beijing";
/// 火山 OpenAPI 总网关的标准形态：POST + 空 body + content-type header
/// （跟官方 SDK 同款）。canonical headers / SignedHeaders 按字母序。
const CONTENT_TYPE: &str = "application/json; charset=utf-8";
const SIGNED_HEADERS: &str = "content-type;host;x-content-sha256;x-date";

// ── QuotaSource 实现 ─────────────────────────────────────────────

/// fetch 阶段要打哪些 action。由 `set_state` 从 AppConfig 顶层
/// `volcengine_ark_plan_filter` 推出；缺省两个都查。
#[derive(Debug, Clone, Copy)]
struct VolcengineArkState {
    show_coding: bool,
    show_agent: bool,
}

impl Default for VolcengineArkState {
    fn default() -> Self {
        // 默认两个都查（开箱即用）。set_state 没被调过的路径
        // （dump CLI / test_extra_instance）也走这个兜底。
        Self {
            show_coding: true,
            show_agent: true,
        }
    }
}

pub struct VolcengineArkSource {
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
    /// 套餐筛选（poller 每次 fetch 前 set_state 推入）。
    /// std RwLock 够用：读写在无 await 点完成，不跨 await 持锁。
    state: RwLock<Option<VolcengineArkState>>,
}

impl Default for VolcengineArkSource {
    fn default() -> Self {
        Self {
            instance_index: 1,
            state: RwLock::new(None),
        }
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

    fn needs_state_update(&self) -> bool {
        // v0.2.9：火山双套餐筛选（coding / agent checkbox）走 set_state 推入，
        // 跟 zenmux mode 同款模式 —— poller 每轮序列化 AppConfig 后由这里读取。
        true
    }

    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // 顶层 `volcengine_ark_plan_filter`（前端 settings 写到这里，跟
            // zenmux_mode 同款约定）。字段缺失 → 对应 plan 默认 true
            // （老 config.json 无感升级为全勾状态）。
            let filter = cfg.get("volcengine_ark_plan_filter");
            let show_coding = filter
                .and_then(|f| f.get("coding"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let show_agent = filter
                .and_then(|f| f.get("agent"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if let Ok(mut g) = self.state.write() {
                *g = Some(VolcengineArkState {
                    show_coding,
                    show_agent,
                });
            }
        })
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
        // 套餐筛选快照：即拿即放（Copy 值），不跨 await 持锁。
        // set_state 未调过（dump CLI / test_extra_instance）→ 默认两个都查。
        let plan_state = match self.state.read() {
            Ok(g) => g.unwrap_or_default(),
            Err(_) => VolcengineArkState::default(),
        };
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
            do_fetch(&ak, &sk, &source_id, &display_name, plan_state).await
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
    plan_state: VolcengineArkState,
) -> Result<ProviderSnapshot, FetchError> {
    // 双筛选全关：用户显式关掉了两个套餐，无 action 可打。
    // 归 UnconfiguredKey（跟"没配 key"同款 UI：引导回设置面板重新勾选）。
    if !plan_state.show_coding && !plan_state.show_agent {
        return Err(FetchError::unconfigured(
            t!("error.volcengine.both_filtered").into_owned(),
        ));
    }

    // 双 action 并发探测（tokio::join! 同时起飞，一个慢不拖另一个）。
    // merge 层"失败不连坐"：一个 action 挂了仍渲染另一个套餐的数据，
    // 只有两个都失败才把错误抛给浮窗。
    let coding_fut = async {
        if plan_state.show_coding {
            Some(do_fetch_one(ak, sk, ACTION_CODING).await)
        } else {
            None
        }
    };
    let agent_fut = async {
        if plan_state.show_agent {
            Some(do_fetch_one(ak, sk, ACTION_AFP).await)
        } else {
            None
        }
    };
    let (coding_res, agent_res) = tokio::join!(coding_fut, agent_fut);

    // 行序由 results 顺序决定：Coding 在前、Agent 在后（视觉稳定）。
    let mut results: Vec<(&'static str, Result<Value, FetchError>)> = Vec::new();
    if let Some(r) = coding_res {
        results.push(("coding", r));
    }
    if let Some(r) = agent_res {
        results.push(("agent", r));
    }

    let (rows, plan_name, raw) = merge_plan_results(results)?;
    Ok(ProviderSnapshot {
        provider: "volcengine_ark".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(chrono::Utc::now().timestamp_millis()),
        next_fetch_at: None,
        raw: Some(raw),
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
    })
}

/// 打单个 action（coding 或 agent），返回原始 JSON。
///
/// 火山 OpenAPI 总网关标准形态：POST + 空 body + content-type header
/// （跟官方 SDK 同款）。两个 action 只有 query 里的 `Action` 不同，
/// 签名 / 鉴权 / 错误分类完全复用。
async fn do_fetch_one(ak: &str, sk: &str, action: &'static str) -> Result<Value, FetchError> {
    // canonical query 按 key 字母序：Action < Region < Version
    let query = format!("Action={action}&Region={REGION}&Version={VERSION}");
    let url = format!("https://{HOST}/?{query}");

    // 1. 准备签名参数
    let x_date = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let body: &[u8] = b"";
    let body_hash = sha256_hex(body);

    // 2. CanonicalRequest（POST + content-type；canonical headers 字母序）
    let canonical_request = format!(
        "POST\n/\n{query}\ncontent-type:{CONTENT_TYPE}\nhost:{HOST}\nx-content-sha256:{body_hash}\nx-date:{x_date}\n\n{SIGNED_HEADERS}\n{body_hash}",
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
        "HMAC-SHA256 Credential={ak}/{credential_scope}, SignedHeaders={SIGNED_HEADERS}, Signature={signature}",
    );

    // 6. 发送请求（POST + 空 body）
    let client = super::shared_client();
    let resp = client
        .post(&url)
        .header("Host", HOST)
        .header("Content-Type", CONTENT_TYPE)
        .header("X-Date", &x_date)
        .header("X-Content-Sha256", &body_hash)
        .header("Authorization", authorization)
        .body("")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!(
                    "error.common.network",
                    url = url,
                    err = humanize_reqwest_err(&e)
                )
                .into_owned(),
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
    // P0 fix (2026-08-06 cross-verify #2): 删 unconditional tracing::warn!，
    // 成功响应不打 body（PlanName / UsageList 账户信息不落日志）。错误
    // body 由下面 !status.is_success() 分支的 FetchError::server 消息带出。

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

    serde_json::from_str(&raw_text).map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })
}

// ── 解析 ─────────────────────────────────────────────────────────

/// 从原始响应中提取 `Result` 节点，顺带处理两层业务错误。
///
/// Coding / AFP 两条解析路径共用（火山 OpenAPI 错误形态一致）：
/// 1. 顶层 `ResponseMetadata.Error`（权限 / 参数错误时没有 Result 节点）
/// 2. `Result.Code != "Success"`（业务级失败）
fn extract_result(raw: &Value) -> Result<&Value, FetchError> {
    // P2 audit fix (2026-08-13): 火山 OpenAPI 的业务错误常放在顶层
    // ResponseMetadata.Error (此时没有 Result 节点)。之前先查 Result →
    // 真实 Code/Message 被"缺 Result 字段"的通用 Parse 错误吞掉, 用户
    // 看不到权限/参数错误原因。先查这里。
    if let Some(err) = raw.get("ResponseMetadata").and_then(|m| m.get("Error")) {
        // D2-02: 真实 Code 透传进模板 code 槽位（此前硬编码 0、code 挪进 msg 拼接，
        // 渲染成 "code 0: InvalidParameter" 自相矛盾）。
        let code = err
            .get("Code")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown");
        let msg = err.get("Message").and_then(|v| v.as_str()).unwrap_or("");
        return Err(FetchError::server(
            t!(
                "error.common.business_code",
                provider = "Volcengine Ark",
                code = code,
                msg = msg
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
    //
    // L-4 fix (2026-09-05 audit)：数字 0 是约定俗成的成功码 —— 原实现把
    // 数字 Code 字符串化后与 "Success" 比较，`0` → `"0" != "Success"` 把
    // 成功响应打成业务错误（修复前 as_str() 返 None 反而静默放行）。
    // 判定改为：字符串 Code 必须 == "Success"；数字 Code 必须 == 0；
    // 其余视为业务失败。
    {
        let code_val = result.get("Code");
        let is_success = match code_val.map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        }) {
            Some(Some(code)) => code == "Success" || code == "0",
            _ => true, // 无 Code 字段 = 成功（保持原语义）
        };
        if !is_success {
            let code = code_val
                .and_then(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .unwrap_or_default();
            let msg = result.get("Message").and_then(|v| v.as_str()).unwrap_or("");
            return Err(FetchError::server(
                t!(
                    "error.common.business_code",
                    provider = "Volcengine Ark",
                    code = code,
                    msg = msg
                )
                .into_owned(),
            ));
        }
    }

    Ok(result)
}

/// 套餐标题行（`RowKind::PlanHeader`，无数据）。
///
/// 前端把它渲染成 muted 小字分组锚点；托盘 / tooltip 天然跳过
/// （`utilization` / `remaining` 都是 None）。
fn plan_header_row(plan: &str) -> QuotaRow {
    let label = if plan == "coding" {
        t!("row.plan_header_coding")
    } else {
        t!("row.plan_header_agent")
    };
    QuotaRow {
        label: label.to_string(),
        utilization: None,
        remaining: None,
        used: None,
        total: None,
        resets_at: None,
        unit: None,
        extra: Some(serde_json::json!({ "plan": plan, "is_header": true })),
        kind: Some(RowKind::PlanHeader),
    }
}

/// 解析 `GetCodingPlanUsage` 响应 → (带 PlanHeader 的 rows, PlanName)。
///
/// Coding Plan 返回的 `Level` 字段枚举（实测 + 文档）：
/// - `Session`  → 5h 滚动窗口（主行）
/// - `Weekly`   → 周窗口（每周一 00:00 重置）
/// - `Monthly`  → 月窗口（订阅月首日 00:00 重置）
/// - `Daily`    → 日窗口（Agent Plan 字段，Coding Plan 暂不返回；预留以应对 schema 加字段）
///
/// 不认识的 Level 静默跳过（schema 漂移保护），不让单条坏数据炸整个 snapshot。
fn parse_coding_rows(raw: &Value) -> Result<(Vec<QuotaRow>, Option<String>), FetchError> {
    let result = extract_result(raw)?;

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
        //   字段 = **已用百分比, 0.0~100.0**（8ccd8d2 用户 dev-stderr body 实测：
        //   session=0.3346 / weekly=2.408 / monthly=11.356，ccswitch 同款 0~100
        //   语义显示 0% / 2% / 11%）。**不是** 0~1 ratio —— 2026-09-07 H-Provider#2
        //   误判成 ratio 乘 100，真实值 11.356 × 100 → clamp 全 100%，浮窗全满。
        // - ResetTimestamp: epoch seconds (10 位) 上面 smart parse 转 ms。
        // - 老 UsageList[] + Remaining/Total 形态保留(虽然火山不返),做
        //   schema 漂移 fallback。
        let (used, total) = if let Some(percent) =
            super::parse::num_f64(entry.get("Percent").unwrap_or(&Value::Null))
        {
            // Percent 已是 0~100 百分比（实测），直接 clamp，不再乘 100。
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
            extra: Some(serde_json::json!({
                "reset_period": reset_period,
                "plan": "coding"
            })),
            kind,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    // 排序：Session → Daily → Weekly → Monthly（让浮窗渲染稳定）。
    // PlanHeader 在排序**之后**插到开头 —— 它的 label 不是窗口标签，
    // 不参与排序。
    rows.sort_by_key(|r| match r.label.as_str() {
        x if x == t!("row.five_hour").as_ref() => 0,
        x if x == t!("row.daily").as_ref() => 1,
        x if x == t!("row.weekly_7d").as_ref() => 2,
        x if x == t!("row.monthly").as_ref() => 3,
        _ => 99,
    });
    rows.insert(0, plan_header_row("coding"));

    let plan_name = result
        .get("PlanName")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok((rows, plan_name))
}

/// 解析 `GetCodingPlanUsage` 响应为完整 snapshot。
///
/// 单 action 兼容入口，仅单测使用（生产路径 poller 走 [`do_fetch`] 的
/// 双 action merge；dump CLI 同样经 do_fetch，不再单独调本函数）。
#[cfg(test)]
fn parse(raw: &Value, source_id: &str, display_name: &str) -> Result<ProviderSnapshot, FetchError> {
    let (rows, plan_name) = parse_coding_rows(raw)?;
    Ok(ProviderSnapshot {
        provider: "volcengine_ark".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(chrono::Utc::now().timestamp_millis()),
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

/// 解析 `GetAFPUsage`（Agent Plan）响应。
///
/// AFP schema（ccswitch 实测，火山无官方文档）：`Result` 下平铺
/// `AFPFiveHour / AFPWeekly / AFPMonthly` 三个窗口对象（Quota / Used /
/// ResetTime），外加 `PlanType` 套餐名。
///
/// - Quota <= 0 或窗口缺失 = 未订阅该窗口，跳过该行
/// - 全部窗口都无数据 → 返回空 Vec（merge 层不为空组渲染 PlanHeader，
///   没订阅 Agent Plan 的用户看不到空标题行）
/// - ResetTime 兼容秒 / 毫秒（< 10^12 当秒 × 1000，同 Coding 路径的
///   schema 漂移保护）；ts <= 0 → None
fn parse_afp_rows(raw: &Value) -> Result<Vec<QuotaRow>, FetchError> {
    let result = extract_result(raw)?;

    let mut data = Vec::new();
    for (key, period, kind, label) in [
        (
            "AFPFiveHour",
            "five_hour",
            Some(RowKind::FiveHour),
            t!("row.five_hour").to_string(),
        ),
        (
            "AFPWeekly",
            "weekly",
            Some(RowKind::Weekly),
            t!("row.weekly_7d").to_string(),
        ),
        ("AFPMonthly", "monthly", None, t!("row.monthly").to_string()),
    ] {
        let Some(win) = result.get(key) else {
            continue;
        };
        let quota = super::parse::num_f64(win.get("Quota").unwrap_or(&Value::Null));
        let used = super::parse::num_f64(win.get("Used").unwrap_or(&Value::Null));
        let (Some(q), Some(u)) = (quota, used) else {
            continue;
        };
        if q <= 0.0 {
            continue; // 未订阅该窗口
        }
        let utilization = ((u / q) * 100.0).clamp(0.0, 100.0);
        let resets_at = super::parse::num_f64(win.get("ResetTime").unwrap_or(&Value::Null))
            .map(|f| f as i64)
            .filter(|ts| *ts > 0)
            .map(|ts| {
                if ts < 1_000_000_000_000 {
                    ts * 1000
                } else {
                    ts
                }
            });
        data.push(QuotaRow {
            label,
            utilization: Some(utilization),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: None,
            extra: Some(serde_json::json!({ "reset_period": period, "plan": "agent" })),
            kind,
        });
    }

    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut rows = vec![plan_header_row("agent")];
    rows.append(&mut data);
    Ok(rows)
}

/// 合并双 action 结果 → (rows, plan_name, merged_raw)。
///
/// "失败不连坐"：一个 action 失败（HTTP 错误 / 解析失败 / 无数据行）只
/// 记下错误，另一个套餐的行照常渲染；只有**全部** action 都没产出任何行
/// 时才把第一个错误抛出去（Coding 的错误优先 —— 它是老 action，鉴权 /
/// 权限问题在它身上最常见）。
///
/// 行序 = results 顺序（Coding 在前、Agent 在后）。plan_name AFP 优先
/// （PlanType 更具体，如 "Large"），Coding fallback。
fn merge_plan_results(
    results: Vec<(&'static str, Result<Value, FetchError>)>,
) -> Result<(Vec<QuotaRow>, Option<String>, Value), FetchError> {
    let mut rows: Vec<QuotaRow> = Vec::new();
    let mut coding_plan_name: Option<String> = None;
    let mut agent_plan_name: Option<String> = None;
    let mut merged_raw = serde_json::Map::new();
    let mut first_err: Option<FetchError> = None;

    for (tag, res) in results {
        match res {
            Ok(raw) => {
                merged_raw.insert(tag.to_string(), raw.clone());
                let parsed = if tag == "coding" {
                    match parse_coding_rows(&raw) {
                        Ok((r, plan)) => {
                            coding_plan_name = plan;
                            Ok(r)
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    // PlanType 更具体（如 "Large"），merge 后 plan_name 优先用它
                    agent_plan_name = extract_result(&raw)
                        .ok()
                        .and_then(|r| r.get("PlanType"))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    parse_afp_rows(&raw)
                };
                match parsed {
                    Ok(mut r) => rows.append(&mut r),
                    Err(e) => {
                        tracing::warn!(
                            plan = tag,
                            error = %e,
                            "火山方舟单套餐解析失败（另一个套餐照常渲染）"
                        );
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    }
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }

    if rows.is_empty() {
        return Err(first_err
            .unwrap_or_else(|| FetchError::parse(t!("error.parse.no_rows_found").into_owned())));
    }
    let plan_name = agent_plan_name.or(coding_plan_name);
    Ok((rows, plan_name, Value::Object(merged_raw)))
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
        // 固定 x_date 测试签名可重现。canonical form 跟 do_fetch_one 保持
        // 一致（POST + content-type + 字母序 SignedHeaders）。
        let x_date = "20260727T100000Z";
        let body = b"";
        let body_hash = sha256_hex(body);
        let query = format!("Action={ACTION_CODING}&Region={REGION}&Version={VERSION}");
        let canonical_request = format!(
            "POST\n/\n{query}\ncontent-type:{CONTENT_TYPE}\nhost:{HOST}\nx-content-sha256:{body_hash}\nx-date:{x_date}\n\n{SIGNED_HEADERS}\n{body_hash}",
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

    // ── parse（Coding）单元测试 ──
    //
    // v0.2.9 起 parse 输出开头多一行 PlanHeader 标题行 —— 以下断言的
    // row index 相应 +1。

    /// 读 row.extra.plan 的辅助（测试专用）。
    fn row_plan(r: &QuotaRow) -> Option<&str> {
        r.extra
            .as_ref()
            .and_then(|e| e.get("plan"))
            .and_then(|p| p.as_str())
    }

    /// ccswitch 实测的 AFP 响应 fixture。
    fn afp_raw() -> Value {
        json!({
            "Result": {
                "PlanType": "Large",
                "AFPFiveHour": { "Quota": 1000, "Used": 800, "ResetTime": 1778806800000_i64 },
                "AFPWeekly":   { "Quota": 5000, "Used": 200, "ResetTime": 1779408000000_i64 },
                "AFPMonthly":  { "Quota": 20000, "Used": 1500, "ResetTime": 1781990400000_i64 }
            }
        })
    }

    #[test]
    fn parse_quota_usage_schema_lowercase() {
        // 火山 Coding Plan 真返 schema (2026-07-28 实测, 8ccd8d2 body 日志):
        // Result.QuotaUsage[] + Level: "session"/"weekly"/"monthly"(小写)
        // + Percent 字段 = **已用百分比 0~100**（实测 session=0.3346 /
        //   weekly=2.408 / monthly=11.356，ccswitch 显示 0% / 2% / 11%）。
        //   **不是** 0~1 ratio —— 2026-09-07 H-Provider#2 误乘 100 导致
        //   全 100% 回归，本测试用实测值锁死语义。
        // + ResetTimestamp: epoch **seconds** (10 位) — smart parse 转 ms
        // + 额外有 Status="Running" / UpdateTimestamp(seconds)
        let raw = json!({
            "Result": {
                "Status": "Running",
                "UpdateTimestamp": 1785217273_i64,
                "QuotaUsage": [
                    // 实测值 (2026-07-28): Percent 已经是 0~100 百分比,
                    // 0.3346 = 0.33% 已用,不是 33.46%。
                    { "Level": "session", "Percent": 0.33462600000000003_f64, "ResetTimestamp": 1785221470_i64 },
                    { "Level": "weekly",  "Percent": 2.408004733333333_f64,   "ResetTimestamp": 1785686400_i64 },
                    { "Level": "monthly", "Percent": 11.356161100000001_f64,  "ResetTimestamp": 1787068799_i64 }
                ]
            }
        });
        let snap = parse(&raw, "volcengine_ark", "Volcengine Ark").expect("parse");
        // v0.2.9: 3 数据行 + 1 PlanHeader 标题行
        assert_eq!(snap.rows.len(), 4);
        assert_eq!(snap.rows[0].kind, Some(RowKind::PlanHeader));
        let five_h = &snap.rows[1];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        // Percent=0.3346 (0~100) → 0.33% utilization (直接当百分比,不乘 100)
        assert!((five_h.utilization.unwrap() - 0.3346).abs() < 0.001);
        // ResetTimestamp 1785221470 是 seconds → smart parse 转 ms
        assert_eq!(five_h.resets_at, Some(1785221470 * 1000));
        assert_eq!(row_plan(five_h), Some("coding"));
        let month = &snap.rows[3];
        assert_eq!(month.label, t!("row.monthly").as_ref());
        // Percent=11.356 → 11.356% utilization，**不是** 1135.6% clamp 成 100%
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
        // 3 数据行 + 1 PlanHeader
        assert_eq!(snap.rows.len(), 4);

        // 排序后：PlanHeader → Session (5h) → Weekly (7d) → Monthly
        let header = &snap.rows[0];
        assert_eq!(header.kind, Some(RowKind::PlanHeader));
        assert_eq!(row_plan(header), Some("coding"));
        assert_eq!(
            header.extra.as_ref().unwrap().get("is_header").unwrap(),
            &json!(true)
        );

        let five_h = &snap.rows[1];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        assert_eq!(five_h.used, None);
        assert_eq!(five_h.total, None);
        assert_eq!(five_h.remaining, None);
        assert!((five_h.utilization.unwrap() - 8.333).abs() < 0.01);
        assert_eq!(five_h.resets_at, Some(1753603200000));

        let week = &snap.rows[2];
        assert_eq!(week.label, t!("row.weekly_7d").as_ref());
        assert_eq!(week.used, None);

        let month = &snap.rows[3];
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
        assert_eq!(snap.rows.len(), 4);
        // Daily 应排在 Session 之后、Weekly 之前
        assert_eq!(snap.rows[0].kind, Some(RowKind::PlanHeader));
        assert_eq!(snap.rows[1].label, t!("row.five_hour").as_ref());
        assert_eq!(snap.rows[2].label, t!("row.daily").as_ref());
        assert_eq!(snap.rows[3].label, t!("row.weekly_7d").as_ref());
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
        // Yearly 未知 → 跳过，只剩 Session + PlanHeader
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[1].label, t!("row.five_hour").as_ref());
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
        let r = &snap.rows[1]; // rows[0] = PlanHeader
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
        assert_eq!(snap.rows.len(), 2); // PlanHeader + Weekly
        assert_eq!(snap.rows[1].label, t!("row.weekly_7d").as_ref());
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
        assert_eq!(snap.rows.len(), 3); // PlanHeader + 5h + Weekly
                                        // Session 行 ResetTimestamp=0 → resets_at=None (不显示 1970)
                                        // 通过 resets_at 验证 5h 行(0 被过滤为 None)
        let _five_h = snap
            .rows
            .iter()
            .find(|r| r.resets_at.is_none() && r.kind != Some(RowKind::PlanHeader))
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
        assert_eq!(snap.rows.len(), 2); // PlanHeader + 5h
        let five_h = &snap.rows[1];
        assert_eq!(five_h.resets_at, None, "ts=-1 must be filtered to None");
    }

    // ── v0.2.9 PlanHeader / 双套餐单元测试 ──

    #[test]
    fn parse_coding_includes_plan_header() {
        // Coding 解析后 rows[0] 是 PlanHeader 行（即使只显示 Coding 一个套餐）
        let raw = json!({
            "Result": {
                "Code": "Success",
                "PlanName": "Lite",
                "UsageList": [
                    { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": 1753603200000_i64 }
                ]
            }
        });
        let (rows, _) = parse_coding_rows(&raw).expect("parse coding");
        assert_eq!(rows.len(), 2);
        let header = &rows[0];
        assert_eq!(header.kind, Some(RowKind::PlanHeader));
        assert_eq!(row_plan(header), Some("coding"));
        assert_eq!(header.utilization, None);
        assert_eq!(header.resets_at, None);
        assert!(
            header.label.contains("Coding"),
            "header label 应含套餐名,实际: {}",
            header.label
        );
    }

    #[test]
    fn parse_afp_basic() {
        // 三窗口齐全，util 计算正确：800/1000=80%、200/5000=4%、1500/20000=7.5%
        let rows = parse_afp_rows(&afp_raw()).expect("parse afp");
        assert_eq!(rows.len(), 4); // header + 3
        let five = &rows[1];
        assert_eq!(five.label, t!("row.five_hour").as_ref());
        assert!((five.utilization.unwrap() - 80.0).abs() < 0.01);
        assert_eq!(five.kind, Some(RowKind::FiveHour));
        assert_eq!(row_plan(five), Some("agent"));
        assert_eq!(
            five.extra.as_ref().unwrap().get("reset_period").unwrap(),
            &json!("five_hour")
        );
        let weekly = &rows[2];
        assert!((weekly.utilization.unwrap() - 4.0).abs() < 0.01);
        assert_eq!(weekly.kind, Some(RowKind::Weekly));
        let monthly = &rows[3];
        assert!((monthly.utilization.unwrap() - 7.5).abs() < 0.01);
    }

    #[test]
    fn parse_afp_partial() {
        // monthly 缺 → 跳过该行，其余照常
        let raw = json!({
            "Result": {
                "PlanType": "Large",
                "AFPFiveHour": { "Quota": 1000, "Used": 800, "ResetTime": 1778806800000_i64 },
                "AFPWeekly":   { "Quota": 5000, "Used": 200, "ResetTime": 1779408000000_i64 }
            }
        });
        let rows = parse_afp_rows(&raw).expect("parse afp");
        assert_eq!(rows.len(), 3); // header + 2
        assert_eq!(rows.iter().last().unwrap().kind, Some(RowKind::Weekly));
    }

    #[test]
    fn parse_afp_ms_reset() {
        // ResetTime 13 位毫秒直用（不乘 1000）
        let raw = json!({
            "Result": {
                "AFPFiveHour": { "Quota": 1000, "Used": 0, "ResetTime": 1778806800000_i64 }
            }
        });
        let rows = parse_afp_rows(&raw).expect("parse afp");
        assert_eq!(rows[1].resets_at, Some(1778806800000));
    }

    #[test]
    fn parse_afp_negative_reset() {
        // ResetTime = -1 → resets_at = None（D-013 一致性，不显示 1970 / 溢出）
        let raw = json!({
            "Result": {
                "AFPFiveHour": { "Quota": 1000, "Used": 0, "ResetTime": -1_i64 }
            }
        });
        let rows = parse_afp_rows(&raw).expect("parse afp");
        assert_eq!(rows[1].resets_at, None);
    }

    #[test]
    fn parse_afp_includes_plan_header() {
        // rows[0] 是 PlanHeader 行，kind=PlanHeader、extra.plan=agent
        let rows = parse_afp_rows(&afp_raw()).expect("parse afp");
        let header = &rows[0];
        assert_eq!(header.kind, Some(RowKind::PlanHeader));
        assert_eq!(row_plan(header), Some("agent"));
        assert_eq!(
            header.extra.as_ref().unwrap().get("is_header").unwrap(),
            &json!(true)
        );
        assert_eq!(header.utilization, None);
        assert!(
            header.label.contains("Agent"),
            "header label 应含套餐名,实际: {}",
            header.label
        );
    }

    #[test]
    fn parse_afp_all_windows_unsubscribed_is_empty() {
        // 全部窗口 Quota=0（未订阅 Agent Plan）→ 空 Vec，
        // merge 层不为空组渲染 PlanHeader（浮窗不出现空标题行）
        let raw = json!({
            "Result": {
                "PlanType": "",
                "AFPFiveHour": { "Quota": 0, "Used": 0, "ResetTime": 0 },
                "AFPWeekly":   { "Quota": 0, "Used": 0, "ResetTime": 0 }
            }
        });
        let rows = parse_afp_rows(&raw).expect("parse afp");
        assert!(rows.is_empty());
    }

    #[test]
    fn parse_afp_business_error_propagates() {
        // ResponseMetadata.Error → 直接透传错误（跟 Coding 同款）
        let raw = json!({
            "ResponseMetadata": { "Error": { "Code": "NotAuthorized", "Message": "no perm" } }
        });
        let err = parse_afp_rows(&raw).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
    }

    #[test]
    fn parse_combined_orders_coding_first() {
        // 合 fetch 后 Coding 行（含 PlanHeader）整体在 Agent 行之前；
        // plan_name AFP 优先（PlanType 更具体）
        let results = vec![
            (
                "coding",
                Ok(json!({
                    "Result": {
                        "Code": "Success",
                        "PlanName": "Lite",
                        "UsageList": [
                            { "Level": "Session", "Remaining": 1100, "Total": 1200, "ResetTimestamp": 1753603200000_i64 }
                        ]
                    }
                })),
            ),
            ("agent", Ok(afp_raw())),
        ];
        let (rows, plan_name, raw) = merge_plan_results(results).expect("merge");
        let first_coding = rows
            .iter()
            .position(|r| row_plan(r) == Some("coding"))
            .expect("coding rows");
        let first_agent = rows
            .iter()
            .position(|r| row_plan(r) == Some("agent"))
            .expect("agent rows");
        assert!(
            first_coding < first_agent,
            "Coding 行必须整体在 Agent 行之前"
        );
        assert_eq!(rows[0].kind, Some(RowKind::PlanHeader));
        assert_eq!(row_plan(&rows[0]), Some("coding"));
        assert_eq!(plan_name.as_deref(), Some("Large"));
        // merged_raw 双 key（调试面板 / dump 可看两个原始响应）
        assert!(raw.get("coding").is_some());
        assert!(raw.get("agent").is_some());
    }

    #[test]
    fn merge_skips_failed_action() {
        // Coding 挂（HTTP 500）+ Agent 成功 → 照常渲染 Agent 组（失败不连坐）
        let results = vec![
            (
                "coding",
                Err(FetchError::server("HTTP 500: boom".to_string())),
            ),
            ("agent", Ok(afp_raw())),
        ];
        let (rows, plan_name, _) = merge_plan_results(results).expect("agent rows survive");
        assert!(rows.iter().all(|r| row_plan(r) == Some("agent")));
        assert_eq!(plan_name.as_deref(), Some("Large"));
    }

    #[test]
    fn merge_all_fail_returns_first_err() {
        // 两个 action 都挂 → 抛第一个错误（Coding 优先）
        let results = vec![
            ("coding", Err(FetchError::server("coding boom".to_string()))),
            ("agent", Err(FetchError::auth("agent boom".to_string()))),
        ];
        let err = merge_plan_results(results).unwrap_err();
        assert_eq!(err.message, "coding boom");
    }

    #[test]
    fn merge_coding_parse_fail_agent_empty_is_err() {
        // Coding 数据行空（parse 错）+ Agent 空组 → 无行可渲染 → 抛错误
        let results = vec![
            (
                "coding",
                Ok(json!({ "Result": { "Code": "Success", "UsageList": [] } })),
            ),
            ("agent", Ok(json!({ "Result": {} }))),
        ];
        let err = merge_plan_results(results).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // ── v0.2.9 set_state（套餐筛选推送）单元测试 ──

    #[tokio::test]
    async fn set_state_reads_filter() {
        let src = VolcengineArkSource::default();
        src.set_state(json!({
            "volcengine_ark_plan_filter": { "coding": false, "agent": true }
        }))
        .await;
        let g = src.state.read().unwrap();
        let s = g.expect("state should be set");
        assert!(!s.show_coding);
        assert!(s.show_agent);
    }

    #[tokio::test]
    async fn set_state_defaults_both_true_when_filter_missing() {
        // 老 config.json 无该字段（或半缺）→ 对应 plan 默认 true（无感升级）
        let src = VolcengineArkSource::default();
        src.set_state(json!({})).await;
        let g = src.state.read().unwrap();
        let s = g.expect("state should be set");
        assert!(s.show_coding && s.show_agent);

        let src2 = VolcengineArkSource::default();
        src2.set_state(json!({ "volcengine_ark_plan_filter": { "coding": false } }))
            .await;
        let g2 = src2.state.read().unwrap();
        let s2 = g2.expect("state should be set");
        assert!(!s2.show_coding);
        assert!(s2.show_agent, "缺 agent 键时默认 true");
    }

    #[test]
    fn default_state_enables_both_plans() {
        // dump CLI / test_extra_instance 不走 set_state → fetch 用 Default 兜底
        let s = VolcengineArkState::default();
        assert!(s.show_coding && s.show_agent);
    }
}
