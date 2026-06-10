//! Provider 多源抽象
//!
//! 引入 Provider trait 后，单一 MiniMax 依赖被抽成统一接口。
//! 新增一个用量源只需要：
//! 1. 在 [`Provider`] enum 加一个变体
//! 2. 在本文件 `register` 函数里 `providers.push(Box::new(MyProvider))`
//! 3. 在 [`QuotaRow`] 渲染端点用其字段
//!
//! 所有 Provider 共享 [`ProviderSnapshot`] 结构（rows: 通用行 + extra: provider 特有字段）。

pub mod deepseek;
pub mod minimax;
pub mod xiaomi;

use serde::{Deserialize, Serialize};

/// 厂商标识。序列化成稳定字符串（不依赖 enum 顺序）。
#[derive(Debug, Default, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Minimax,
    Deepseek,
    Xiaomimimo,
}

impl Provider {
    pub fn id_str(&self) -> &'static str {
        match self {
            Provider::Minimax => "minimax",
            Provider::Deepseek => "deepseek",
            Provider::Xiaomimimo => "xiaomimimo",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Minimax => "MiniMax",
            Provider::Deepseek => "DeepSeek",
            Provider::Xiaomimimo => "Xiaomi MiMo",
        }
    }

    pub fn all() -> [Provider; 3] {
        [Provider::Minimax, Provider::Deepseek, Provider::Xiaomimimo]
    }
}

/// 错误分类（前端按 kind 选择不同样式 + 操作按钮）
///
/// 借鉴 ccswitch extractor 的 `{isValid: false}` 思路：除了用户友好文案，
/// 还给前端一个机器可读的类型，决定显示什么 UI。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// API key 还没设（设置面板里没填）
    UnconfiguredKey,
    /// 401 / 403 鉴权失败（key 错或失效）
    AuthFailed,
    /// 429 调用太频繁
    RateLimited,
    /// 网络层失败（DNS / 连接拒绝 / 超时）
    Network,
    /// 响应不是合法 JSON
    Parse,
    /// 解析成功但所有候选字段名都不匹配（schema 未识别）
    SchemaUnknown,
    /// 5xx 服务端错误
    ServerError,
    /// 其他未分类错误（兜底）
    Other,
}

impl ErrorKind {
    /// 给前端的简短文案（≤ 8 字中文，适合卡片标题）
    pub fn short_label(&self) -> &'static str {
        match self {
            ErrorKind::UnconfiguredKey => "未配置 Key",
            ErrorKind::AuthFailed => "Key 无效",
            ErrorKind::RateLimited => "请求过快",
            ErrorKind::Network => "网络错误",
            ErrorKind::Parse => "响应异常",
            ErrorKind::SchemaUnknown => "Schema 未知",
            ErrorKind::ServerError => "服务异常",
            ErrorKind::Other => "未知错误",
        }
    }

    /// 是否应该引导用户去设置面板
    pub fn needs_settings(&self) -> bool {
        matches!(self, ErrorKind::UnconfiguredKey | ErrorKind::AuthFailed)
    }
}

/// 一行展示数据。三种模式互斥（用 `utilization` / `remaining` / `extra.display` 区分）：
/// - **百分比模式**（MiniMax 5h/周）：`utilization` 有值，`resets_at` 有值
/// - **余额模式**（DeepSeek）：`remaining` 有值，`unit` 是货币
/// - **状态行**（DeepSeek is_available）：`extra.display` 有值
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaRow {
    /// 行标签，如 "5h" / "周" / "余额" / "状态"
    pub label: String,
    /// 0-100+ 的百分比（MiniMax 用）
    pub utilization: Option<f64>,
    /// 剩余数量（DeepSeek 钱包金额；MiniMax 暂未用）
    pub remaining: Option<f64>,
    /// 总量（暂未用，预留）
    pub total: Option<f64>,
    /// 重置时间（毫秒；MiniMax 用）
    pub resets_at: Option<i64>,
    /// 单位（"CNY" / "USD" / "%"）
    pub unit: Option<String>,
    /// provider 特有扩展字段（如 `{is_available, display}`）
    pub extra: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderSnapshot {
    pub provider: Provider,
    pub success: bool,
    pub rows: Vec<QuotaRow>,
    pub error: Option<String>,
    /// 错误分类（仅 `success == false` 时有意义）。前端按 kind 选样式/操作。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    pub fetched_at: Option<i64>,
    /// 原始响应，便于排查
    pub raw: Option<serde_json::Value>,
    /// provider 自身是否健康（用于整体托盘颜色 + tooltip 颜色）
    pub is_healthy: bool,
}

impl ProviderSnapshot {
    /// 构造一个空的成功/失败快照（错误态用）
    pub fn empty_error(provider: Provider, kind: ErrorKind, error: String) -> Self {
        Self {
            provider,
            success: false,
            rows: vec![],
            error: Some(error),
            error_kind: Some(kind),
            fetched_at: Some(chrono::Utc::now().timestamp_millis()),
            raw: None,
            is_healthy: false,
        }
    }

    /// 计算 health 等级：ok / warn / alert / unknown
    pub fn health_label(&self) -> &'static str {
        if !self.success {
            return "alert";
        }
        match self.provider {
            Provider::Deepseek => {
                if self.is_healthy { "ok" } else { "alert" }
            }
            Provider::Minimax => {
                // 取第一个有 utilization 的 row
                let u = self.rows.iter()
                    .filter_map(|r| r.utilization)
                    .next()
                    .unwrap_or(0.0);
                if u < 70.0 { "ok" }
                else if u < 90.0 { "warn" }
                else { "alert" }
            }
            Provider::Xiaomimimo => {
                // 跟 MiniMax 一样的百分比阈值
                let u = self.rows.iter()
                    .filter_map(|r| r.utilization)
                    .next()
                    .unwrap_or(0.0);
                if u < 70.0 { "ok" }
                else if u < 90.0 { "warn" }
                else { "alert" }
            }
        }
    }
}

/// 顶层快照：所有 provider 一次刷新的合集
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaSnapshot {
    pub providers: Vec<ProviderSnapshot>,
    /// 最近一次任一 provider 刷新的时间戳
    pub fetched_at: Option<i64>,
}

impl QuotaSnapshot {
    /// 整体最差 health（用于托盘图标颜色）
    pub fn worst_health(&self) -> &'static str {
        let mut worst = "ok";
        for p in &self.providers {
            let h = p.health_label();
            worst = match (worst, h) {
                (_, "alert") => "alert",
                ("ok", "warn") => "warn",
                (a, b) if a == b => a,
                _ => worst,
            };
        }
        worst
    }
}

/// Provider 抽象实现。
///
/// 用 Box<dyn> 持有，函数返回 `Pin<Box<dyn Future>>` 以避开 async_trait 宏。
#[allow(dead_code)] // `id` / `display_name` 预留给未来的 dispatch 用
pub trait ProviderImpl: Send + Sync {
    fn id(&self) -> Provider;
    fn display_name(&self) -> &'static str;

    /// 拉取用量。
    ///
    /// `api_key` 是用户在该 provider 下配的 key。
    /// 成功返回 [`ProviderSnapshot`]，失败返回带用户友好中文消息的 Err。
    fn fetch<'a>(
        &'a self,
        api_key: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ProviderSnapshot, String>> + Send + 'a>,
    >;
}
