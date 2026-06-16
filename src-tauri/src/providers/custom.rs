//! 用户自定义 New API quota source（PR 3 核心）
//!
//! 让用户在设置面板加/改/删自己的中转站（dmx / byteplus / lemondata / ctok /
//! silicon / crazyrouter / cubence / dds / runapi / ucloud / shengsuanyun / etc.），
//! 一次实现吃掉 10+ 个异构 provider。
//!
//! ## 架构
//!
//! - [`CustomSourceSpec`] 是从 `custom_sources.json` 反序列化的纯数据
//! - [`ExtractSpec`] 是 3 选 1 的提取模板（New API / 余额 / 自定义 JSON path）
//! - [`CustomSource`] 是 `QuotaSource` trait 的运行时实现，包一个 spec
//!
//! ## Provider 字段占位（重要）
//!
//! `ProviderSnapshot.provider` 字段类型是 [`super::Provider`] enum（只有 3 个
//! 变体：Minimax / Deepseek / Xiaomimimo），CustomSource 没有自己的变体。
//! 为了让序列化层编译，我们写 `Provider::Minimax` 占位 —— **前端一律用
//! `source_id` 路由**，不读 `provider` 字段（main.ts:411-445 走 `id = source_id ??
//! provider` 逻辑，source_id 存在时优先）。
//!
//! ## 持久化
//!
//! Spec 存在独立的 `custom_sources.json`（原子写 + 0600）。API key 走
//! `keys.json` 的 `custom_<uuid>` key（复用 [`crate::config::save_credential_for_id`]）。
//!
//! ## Extract 模板语义
//!
//! 三种 preset：
//! 1. **NewApi** —— 写死路径 `data.quota` / `data.used_quota`，可选 `divide` 覆盖
//!    默认 500000（New API 系中转站的 quota 字段通常是「余额 × 500000」整数）
//! 2. **Balance** —— 用户填 `balance_path`（必填）+ `currency_path`（可选）+ `divide`
//! 3. **Custom** —— 用户填 3 个独立 path（remaining / used / total）+ unit + divide
//!
//! `divide` 是数值后处理：在 num_f64 之后除以该值。NewApi 默认 500000 是因为
//! ccswitch 等中转站用 quota / 500000 = USD 的换算。其他中转站如果不涉及
//! 整数放大，填 1.0 即可。

use std::borrow::Cow;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::parse::{num_f64, read_path};
use super::{
    shared_client, AuthKind, Credentials, ErrorKind, FetchError, Provider, ProviderSnapshot,
    QuotaRow, QuotaSource,
};

// ── 类型定义 ────────────────────────────────────────────────────────

/// Extract 模板（3 选 1）。
///
/// 用 `#[serde(tag = "preset", rename_all = "snake_case")]` 让 JSON 形如：
/// ```json
/// { "preset": "new_api", "divide": 500000.0 }
/// { "preset": "balance", "balance_path": "data.credit", "currency_path": "data.unit" }
/// { "preset": "custom", "remaining_path": "x", "used_path": "y", "total_path": "z" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "preset", rename_all = "snake_case")]
pub enum ExtractSpec {
    /// New API 系中转站（写死 data.quota / data.used_quota）。
    ///
    /// - `remaining = data.quota / divide`
    /// - `used = data.used_quota / divide`
    /// - `total = remaining + used`（两边都解出时）
    /// - `unit = "USD"`
    NewApi { divide: Option<f64> },
    /// 余额系（用户填 balance_path 字符串 + 可选 currency_path + 可选 divide）。
    ///
    /// - `remaining = read_path(raw, balance_path) / divide`
    /// - `unit = read_path(raw, currency_path).as_str()`（如果 path 存在）
    Balance {
        balance_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        currency_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        divide: Option<f64>,
    },
    /// 自定义（3 个独立 path）。任一未填 → 该字段为 None。
    ///
    /// - 所有 path 都解不出 → 报 parse 错
    /// - `unit` 是写死字符串（不从 JSON 读，避免歧义）
    Custom {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remaining_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        used_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        divide: Option<f64>,
    },
}

/// 用户自定义 source 的完整 spec。序列化为 JSON 存 `custom_sources.json`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomSourceSpec {
    /// `"custom_a1b2c3d4"` —— UUID 后 8 位 hex，由后端在 `add_custom_source` 生成
    pub id: String,
    /// 用户起的名字（"DMX API"），设置面板显示 + 删除二次输入用
    pub display_name: String,
    /// 中转站 base URL（"https://api.dmx.com"），不带尾 /
    pub base_url: String,
    /// 路径（"/api/user/self"），必须以 / 开头
    pub path: String,
    /// HTTP method（"GET" / "POST"）。v1 POST 按 GET 处理（无 body），未来加 body 字段
    pub method: String,
    /// 提取模板（3 选 1）
    pub extract: ExtractSpec,
    /// 可选：从 JSON 读 plan_name 的 path（"data.group"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_name_path: Option<String>,
    /// 可选：accent 色 hex（"#ff6b35"）。None → 浮窗用首字母 + #888 fallback
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// 创建时间戳（秒）
    pub created_at: i64,
}

// ── QuotaSource 实现 ────────────────────────────────────────────────

/// 一个 `CustomSourceSpec` 包的运行时 source。
///
/// `QuotaSource` trait 要求 `Send + Sync`，CustomSource 默认就满足（String + f64）。
pub struct CustomSource {
    spec: CustomSourceSpec,
}

impl CustomSource {
    pub fn new(spec: CustomSourceSpec) -> Self {
        Self { spec }
    }

    #[allow(dead_code)]  // Phase E IPC 接收 spec 时用，避免重新 clone
    pub fn spec(&self) -> &CustomSourceSpec {
        &self.spec
    }
}

impl QuotaSource for CustomSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Owned(self.spec.id.clone())
    }
    fn display_name(&self) -> Cow<'_, str> {
        Cow::Owned(self.spec.display_name.clone())
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKey
    }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // CustomSource 无 region / overrides 概念（base_url / path 都是 spec 写死的）
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured("未配置 API key（设置面板填入）"));
            }
            let spec = self.spec.clone();  // 'static lifetime 需要 owned
            do_fetch(api_key, &spec).await
        })
    }
}

// ── 内部：HTTP 请求 + 解析 ──────────────────────────────────────────

async fn do_fetch(api_key: &str, spec: &CustomSourceSpec) -> Result<ProviderSnapshot, FetchError> {
    let url = format!(
        "{}{}",
        spec.base_url.trim_end_matches('/'),
        spec.path
    );

    let client = shared_client();
    let mut req = match spec.method.to_uppercase().as_str() {
        "GET" => client.get(&url),
        "POST" => client.post(&url),
        other => {
            return Err(FetchError::new(
                ErrorKind::Other,
                format!("不支持的 method: {other}"),
            ));
        }
    };
    req = req
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json");

    let resp = req
        .send()
        .await
        .map_err(|e| FetchError::network(format!("CustomSource 网络错误: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(FetchError::auth("鉴权失败，请检查 API key"));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth("无权限访问（HTTP 403）"));
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(FetchError::new(
            ErrorKind::RateLimited,
            "请求过于频繁，请稍后再试",
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(format!(
            "CustomSource HTTP {status}: {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

    let (rows, plan_name) = parse_with_extract(&raw, &spec.extract, spec.plan_name_path.as_deref())?;

    Ok(ProviderSnapshot {
        // ⚠ Provider::Minimax 是历史占位（plan §13 review #4）。
        // 前端 main.ts 走 `id = source_id ?? provider`，source_id 存在时优先。
        provider: Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(chrono::Utc::now().timestamp_millis()),
        raw: Some(raw),
        is_healthy: true,
        source_id: Some(spec.id.clone()),
        source_display_name: Some(spec.display_name.clone()),
        plan_name,
    })
}

/// 按 extract 模板把 raw JSON 解成 QuotaRow。
fn parse_with_extract(
    raw: &Value,
    extract: &ExtractSpec,
    plan_path: Option<&str>,
) -> Result<(Vec<QuotaRow>, Option<String>), FetchError> {
    // 1. plan_name（所有 preset 都共用，可选）
    let plan_name = plan_path
        .and_then(|p| read_path(raw, p))
        .and_then(|v| v.as_str().map(String::from));

    // 2. 按 preset 分支
    let row = match extract {
        ExtractSpec::NewApi { divide } => {
            let div = divide.unwrap_or(500_000.0);
            if div == 0.0 {
                return Err(FetchError::parse("NewApi 模板：divide 不能为 0"));
            }
            let remaining = read_path(raw, "data.quota")
                .and_then(num_f64)
                .map(|v| v / div);
            let used = read_path(raw, "data.used_quota")
                .and_then(num_f64)
                .map(|v| v / div);
            let total = match (remaining, used) {
                (Some(r), Some(u)) => Some(r + u),
                _ => None,
            };
            if remaining.is_none() && used.is_none() {
                return Err(FetchError::parse(
                    "NewApi 模板：data.quota / data.used_quota 都缺失或非数字",
                ));
            }
            QuotaRow {
                label: "余额".to_string(),
                utilization: None,
                remaining,
                used,
                total,
                resets_at: None,
                unit: Some("USD".to_string()),
                extra: None,
            }
        }
        ExtractSpec::Balance {
            balance_path,
            currency_path,
            divide,
        } => {
            let div = divide.unwrap_or(1.0);
            if div == 0.0 {
                return Err(FetchError::parse("Balance 模板：divide 不能为 0"));
            }
            let remaining = read_path(raw, balance_path.as_str())
                .and_then(num_f64)
                .map(|v| v / div);
            if remaining.is_none() {
                return Err(FetchError::parse(format!(
                    "Balance 模板：路径 '{}' 无效或非数字",
                    balance_path
                )));
            }
            let unit = currency_path
                .as_deref()
                .and_then(|p| read_path(raw, p))
                .and_then(|v| v.as_str().map(String::from));
            QuotaRow {
                label: "余额".to_string(),
                utilization: None,
                remaining,
                used: None,
                total: None,
                resets_at: None,
                unit,
                extra: None,
            }
        }
        ExtractSpec::Custom {
            remaining_path,
            used_path,
            total_path,
            unit,
            divide,
        } => {
            let div = divide.unwrap_or(1.0);
            if div == 0.0 {
                return Err(FetchError::parse("Custom 模板：divide 不能为 0"));
            }
            let remaining = remaining_path
                .as_deref()
                .and_then(|p| read_path(raw, p))
                .and_then(num_f64)
                .map(|v| v / div);
            let used = used_path
                .as_deref()
                .and_then(|p| read_path(raw, p))
                .and_then(num_f64)
                .map(|v| v / div);
            let total = total_path
                .as_deref()
                .and_then(|p| read_path(raw, p))
                .and_then(num_f64)
                .map(|v| v / div);
            if remaining.is_none() && used.is_none() && total.is_none() {
                return Err(FetchError::parse(
                    "Custom 模板：所有 path 都没匹配到值",
                ));
            }
            QuotaRow {
                label: "余额".to_string(),
                utilization: None,
                remaining,
                used,
                total,
                resets_at: None,
                unit: unit.clone(),
                extra: None,
            }
        }
    };

    Ok((vec![row], plan_name))
}

// ── 单元测试 ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_spec(extract: ExtractSpec) -> CustomSourceSpec {
        CustomSourceSpec {
            id: "custom_test1234".to_string(),
            display_name: "Test Custom".to_string(),
            base_url: "https://api.test.com".to_string(),
            path: "/api/user/self".to_string(),
            method: "GET".to_string(),
            extract,
            plan_name_path: None,
            accent: None,
            created_at: 1700000000,
        }
    }

    // ── ExtractSpec::NewApi ──

    #[test]
    fn extract_newapi_basic() {
        let raw = json!({
            "data": { "quota": 50000, "used_quota": 5000 }
        });
        let spec = make_spec(ExtractSpec::NewApi { divide: Some(500_000.0) });
        let (rows, plan) = parse_with_extract(&raw, &spec.extract, spec.plan_name_path.as_deref()).unwrap();
        assert_eq!(rows.len(), 1);
        assert!((rows[0].remaining.unwrap() - 0.1).abs() < 0.001);  // 50000 / 500000 = 0.1
        assert!((rows[0].used.unwrap() - 0.01).abs() < 0.001);     // 5000 / 500000 = 0.01
        assert!((rows[0].total.unwrap() - 0.11).abs() < 0.001);
        assert_eq!(rows[0].unit.as_deref(), Some("USD"));
        assert!(plan.is_none());
    }

    #[test]
    fn extract_newapi_default_divide_500000() {
        let raw = json!({ "data": { "quota": 500000 } });
        let spec = make_spec(ExtractSpec::NewApi { divide: None });  // 用默认
        let (rows, _) = parse_with_extract(&raw, &spec.extract, None).unwrap();
        assert!((rows[0].remaining.unwrap() - 1.0).abs() < 0.001);
    }

    #[test]
    fn extract_newapi_missing_both_fields_errors() {
        let raw = json!({ "data": { "username": "x" } });
        let spec = make_spec(ExtractSpec::NewApi { divide: None });
        let err = parse_with_extract(&raw, &spec.extract, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn extract_newapi_divide_zero_errors() {
        let raw = json!({ "data": { "quota": 100 } });
        let spec = make_spec(ExtractSpec::NewApi { divide: Some(0.0) });
        let err = parse_with_extract(&raw, &spec.extract, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // ── ExtractSpec::Balance ──

    #[test]
    fn extract_balance_with_currency() {
        let raw = json!({
            "data": { "credit": 12.34, "unit": "credits" }
        });
        let spec = make_spec(ExtractSpec::Balance {
            balance_path: "data.credit".to_string(),
            currency_path: Some("data.unit".to_string()),
            divide: None,
        });
        let (rows, _) = parse_with_extract(&raw, &spec.extract, None).unwrap();
        assert!((rows[0].remaining.unwrap() - 12.34).abs() < 0.001);
        assert_eq!(rows[0].unit.as_deref(), Some("credits"));
    }

    #[test]
    fn extract_balance_path_invalid_errors() {
        let raw = json!({ "data": { "credit": 100 } });
        let spec = make_spec(ExtractSpec::Balance {
            balance_path: "data.missing".to_string(),
            currency_path: None,
            divide: None,
        });
        let err = parse_with_extract(&raw, &spec.extract, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
        assert!(err.message.contains("data.missing"));
    }

    #[test]
    fn extract_balance_with_divide() {
        let raw = json!({ "balance_cents": 12345 });
        let spec = make_spec(ExtractSpec::Balance {
            balance_path: "balance_cents".to_string(),
            currency_path: None,
            divide: Some(100.0),
        });
        let (rows, _) = parse_with_extract(&raw, &spec.extract, None).unwrap();
        assert!((rows[0].remaining.unwrap() - 123.45).abs() < 0.001);
    }

    // ── ExtractSpec::Custom ──

    #[test]
    fn extract_custom_multi_path() {
        let raw = json!({
            "x": 100.0,
            "y": 30.0,
            "z": 130.0
        });
        let spec = make_spec(ExtractSpec::Custom {
            remaining_path: Some("x".to_string()),
            used_path: Some("y".to_string()),
            total_path: Some("z".to_string()),
            unit: Some("USD".to_string()),
            divide: None,
        });
        let (rows, _) = parse_with_extract(&raw, &spec.extract, None).unwrap();
        assert!((rows[0].remaining.unwrap() - 100.0).abs() < 0.001);
        assert!((rows[0].used.unwrap() - 30.0).abs() < 0.001);
        assert!((rows[0].total.unwrap() - 130.0).abs() < 0.001);
        assert_eq!(rows[0].unit.as_deref(), Some("USD"));
    }

    #[test]
    fn extract_custom_partial_paths() {
        // 只填 remaining_path，其他 None
        let raw = json!({ "balance": 50.0 });
        let spec = make_spec(ExtractSpec::Custom {
            remaining_path: Some("balance".to_string()),
            used_path: None,
            total_path: None,
            unit: None,
            divide: None,
        });
        let (rows, _) = parse_with_extract(&raw, &spec.extract, None).unwrap();
        assert!((rows[0].remaining.unwrap() - 50.0).abs() < 0.001);
        assert!(rows[0].used.is_none());
        assert!(rows[0].total.is_none());
        assert!(rows[0].unit.is_none());
    }

    #[test]
    fn extract_custom_all_paths_missing_errors() {
        let raw = json!({ "other": 100 });
        let spec = make_spec(ExtractSpec::Custom {
            remaining_path: Some("missing1".to_string()),
            used_path: Some("missing2".to_string()),
            total_path: Some("missing3".to_string()),
            unit: None,
            divide: None,
        });
        let err = parse_with_extract(&raw, &spec.extract, None).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    // ── plan_name_path ──

    #[test]
    fn plan_name_path_resolved() {
        let raw = json!({
            "data": { "quota": 100, "group": "VIP" }
        });
        let spec = make_spec(ExtractSpec::NewApi { divide: Some(1.0) });
        let (_, plan) = parse_with_extract(&raw, &spec.extract, Some("data.group")).unwrap();
        assert_eq!(plan.as_deref(), Some("VIP"));
    }

    // ── QuotaSource trait basic ──

    #[test]
    fn custom_source_id_and_display_name_return_owned() {
        let spec = make_spec(ExtractSpec::NewApi { divide: None });
        let src = CustomSource::new(spec);
        assert_eq!(src.id().as_ref(), "custom_test1234");
        assert_eq!(src.display_name().as_ref(), "Test Custom");
    }

    #[test]
    fn custom_source_auth_kind_is_api_key() {
        let spec = make_spec(ExtractSpec::NewApi { divide: None });
        let src = CustomSource::new(spec);
        assert!(matches!(src.auth_kind(), AuthKind::ApiKey));
    }

    #[test]
    fn custom_source_unconfigured_key_errors() {
        let spec = make_spec(ExtractSpec::NewApi { divide: None });
        let src = CustomSource::new(spec);
        let creds = Credentials { api_key: None, cookie: None };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(src.fetch(&creds)).unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnconfiguredKey);
    }
}
