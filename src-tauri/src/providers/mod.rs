//! Provider 多源抽象
//!
//! ## 架构（ROADMAP Phase 1 起）
//!
//! 每个用量源是 [`QuotaSource`] trait 的一个实现，由 [`builtin_sources`] 注册表
//! 集中管理。新增一个 source 不再需要改 commands.rs 的 `match` 块：
//!
//! 1. 在 [`providers`] 子模块下新增 `xxx.rs`，写一个 `XxxSource: QuotaSource`
//! 2. 在 [`builtin_sources`] 里 `Box::new(XxxSource::default())`
//! 3. 在 `config.json` 的 `providers` 字段下加默认配置
//!
//! ## 向后兼容
//!
//! 旧的 [`Provider`] enum（minimax / deepseek / xiaomimimo）继续存在，
//! 旧 [`ProviderSnapshot`] / [`ProviderImpl`] 也保留别名，commands.rs 走
//! [`builtin_sources`] 路径，但 `dump` CLI 和 `set_api_key_for` 仍按 enum 走。

pub mod claude_official;
pub mod custom;
pub mod deepseek;
pub mod kimi;
pub mod minimax;
pub mod novita;
pub mod openrouter;
pub mod parse;
pub mod qwen;
pub mod siliconflow;
pub mod stepfun;
pub mod tavily;
pub mod xiaomi;
pub mod zenmux;
pub mod zhipu;

// PR 3 重新导出：让 settings 面板 / 浮窗等 crate 外部消费者只 `use
// crate::providers::{CustomSource, CustomSourceSpec}` 即可。
//
// ExtractSpec 不再 re-export —— 公开 API 没用过它；测试代码用完整路径
// `crate::providers::custom::ExtractSpec`(见 `commands/custom_sources.rs`
// 单元测试 + `config/custom_sources.rs` 单元测试),省一个 unused re-export warning。
pub use custom::{CustomSource, CustomSourceSpec};

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// 厂商标识。序列化成稳定字符串（不依赖 enum 顺序）。
///
/// **新代码请优先用 `&str` id**（如 `"minimax"` / `"tavily"`），让 source 注册表
/// 成为唯一真相源。本 enum 留着是因为 `config.rs` / `dump` CLI / 已有的 IPC 命令
/// 还在用，加新 source 不需要改这一层。
///
/// Tavily / ZenMux 等 Phase 1 起新增的 source **不进 enum**，走
/// [`QuotaSource`] trait + [`builtin_sources`] 注册表。
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

    /// 已知 provider 列表（按固定顺序）
    pub fn all() -> [Provider; 3] {
        [Provider::Minimax, Provider::Deepseek, Provider::Xiaomimimo]
    }

    /// 把 provider id 字符串映射到 `Provider` enum。未知 id 统一 fallback
    /// 到 `Provider::Minimax`（占位，因为 Tavily 等 Phase 1 起的新 source
    /// 没有自己的 enum 变体；浮窗用 `source_id` 路由，enum 只用于兼容旧字段）。
    pub fn from_id_str(id: &str) -> Provider {
        match id {
            "minimax" => Provider::Minimax,
            "deepseek" => Provider::Deepseek,
            "xiaomimimo" => Provider::Xiaomimimo,
            _ => Provider::Minimax,
        }
    }
}

// ── 凭据（统一存放 api_key + cookie）────────────────────────────────

/// 一个 quota source 需要的全部凭据。
///
/// MiniMax / DeepSeek 只需要 `api_key`；Xiaomi 需要 `cookie`；未来可扩展。
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    pub api_key: Option<String>,
    pub cookie: Option<String>,
}

impl Credentials {
    #[allow(dead_code)] // 预留 v2 凭据校验 helper（前端 settings 面板 v2 要做"未设置任何凭据"的提示时启用）
    pub fn has_any(&self) -> bool {
        self.api_key.as_deref().map(str::trim).map(str::is_empty) == Some(false)
            || self.cookie.as_deref().map(str::trim).map(str::is_empty) == Some(false)
    }
}

/// 鉴权方式（前端 UI 用，决定显示"API Key"输入框还是"Cookie"输入框）。
///
/// 实际拼 header 的逻辑在 [`crate::http::apply_auth`]（下一阶段）里；
/// 这里先定义枚举值给 trait 用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    /// 走 `Authorization: <prefix><api_key>`（Bearer / 空前缀）
    ApiKey,
    /// 走 `Cookie: <cookie>`
    Cookie,
    /// 优先 Bearer，401 时降级到 Cookie（Xiaomi 用）。
    /// 两个输入都展示在设置面板，用户可只填一个；fetch 路径按
    /// `decide_auth_strategy` 决定。
    ApiKeyOrCookie,
}

// ── 错误分类（前端按 kind 选样式 + 操作按钮）───────────────────────────

/// 错误分类（前端按 kind 选择不同样式 + 操作按钮）。
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
    /// snake_case 形式（跟 serde rename_all 一致）—— 给 LogEntry / dedup key 用。
    /// **不要**拿这个当用户可见 label：用户可见的 label 走前端 `t("error.${kind}")`。
    /// P1 错误分类重构：删了原来的 `short_label`（中文），改用这个跟 serde
    /// 一致的稳定字符串，dedup 不会因为 i18n 切换失效。
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::UnconfiguredKey => "unconfigured_key",
            ErrorKind::AuthFailed => "auth_failed",
            ErrorKind::RateLimited => "rate_limited",
            ErrorKind::Network => "network",
            ErrorKind::Parse => "parse",
            ErrorKind::SchemaUnknown => "schema_unknown",
            ErrorKind::ServerError => "server_error",
            ErrorKind::Other => "other",
        }
    }

    /// 是否应该引导用户去设置面板
    pub fn needs_settings(&self) -> bool {
        matches!(self, ErrorKind::UnconfiguredKey | ErrorKind::AuthFailed)
    }
}

/// 结构化 fetch 错误。Phase 1 引入，用来替代散落在各 provider 里的中文 `String` 错误。
///
/// 配套 [`crate::commands::error_kind`] 把它转成 [`ErrorKind`]。
#[derive(Debug, Clone)]
pub struct FetchError {
    pub kind: ErrorKind,
    pub message: String,
}

impl FetchError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
    pub fn unconfigured(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::UnconfiguredKey, message)
    }
    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::AuthFailed, message)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }
    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, message)
    }
    #[allow(dead_code)] // 预留 v2 helper（v2 schema 推断路径要按 ErrorKind::SchemaUnknown 分类时启用）
    pub fn schema_unknown(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::SchemaUnknown, message)
    }
    pub fn server(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ServerError, message)
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for FetchError {}

// ── 一行展示数据 ─────────────────────────────────────────────────────

/// 一行展示数据。三种模式互斥（用 `utilization` / `remaining` / `extra.display` 区分）：
/// - **百分比模式**（MiniMax 5h/周）：`utilization` 有值，`resets_at` 有值
/// - **余额模式**（DeepSeek）：`remaining` 有值，`unit` 是货币
/// - **状态行**（DeepSeek is_available）：`extra.display` 有值
///
/// Phase 1 起加入 `used` + `total`（之前只有 `remaining`），用来支持 Tavily
/// "150/1000 credits" 这种数字展示。
///
/// H2 fix: 加 `kind` 字段做"语义分类"——xiaomi 的 apply_display_mode 之前
/// 用 r.label == \"套餐\" / \"总额度\" 硬编码中文做 filter,en locale 下
/// label 是 \"Plan\" / \"Monthly total\",filter 全部落空 → 0 rows 但
/// success=true。改用 kind (RowKind 枚举) 过滤,跟 locale 解耦。
/// 现有 row 默认 `None` = 旧 backend 兼容,小米 provider 自己填。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaRow {
    /// 行标签，如 "5h" / "周" / "余额" / "状态" / "search"
    pub label: String,
    /// 0-100+ 的百分比（MiniMax / Xiaomi 用）
    pub utilization: Option<f64>,
    /// 剩余数量（DeepSeek 钱包金额；Tavily credits）
    pub remaining: Option<f64>,
    /// 已用数量（Tavily credits 用，Phase 1 起开始填充）
    pub used: Option<f64>,
    /// 总量（Tavily credits 用，Phase 1 起开始填充）
    pub total: Option<f64>,
    /// 重置时间（毫秒；MiniMax 用）
    pub resets_at: Option<i64>,
    /// 单位（"CNY" / "USD" / "%" / "credits"）
    pub unit: Option<String>,
    /// provider 特有扩展字段（如 `{is_available, display}`）
    pub extra: Option<serde_json::Value>,
    /// 行的语义分类（H2 fix: 用于不依赖 locale 的过滤，如小米 display_mode）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RowKind>,
}

/// 行的语义分类。H2 fix: 不绑定 label 文案,跨 locale 稳定。
/// 现有 builtin provider 大多用 None（不影响现有逻辑）;Xiaomi 必须填。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    /// MiniMax 5h 用量
    FiveHour,
    /// MiniMax 周用量
    Weekly,
    /// 小米 / New API 套餐余额
    Plan,
    /// 小米 / New API 补偿
    Compensation,
    /// 小米 / New API 总额度
    MonthlyTotal,
}

// ── 单个 source 的 snapshot ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderSnapshot {
    /// 兼容字段：序列化成 `"minimax"` 等。新代码可以忽略。
    pub provider: Provider,
    /// 兼容字段：true = 成功拉到至少一行 row
    pub success: bool,
    pub rows: Vec<QuotaRow>,
    pub error: Option<String>,
    /// 错误分类（仅 `success == false` 时有意义）。前端按 kind 选样式/操作。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    pub fetched_at: Option<i64>,
    /// 下次自动 fetch 的时间戳（epoch ms）。浮窗错误卡片用这个显示
    /// "下次重试 in Xm" 倒计时。None = 未知（poller 还在调度但还没到点）。
    /// 2026-06-17 commit 加。`#[serde(default)]` 让老 snapshot（落 logstore
    /// 的历史）仍能反序列化。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fetch_at: Option<i64>,
    /// 原始响应，便于排查
    pub raw: Option<serde_json::Value>,
    /// provider 自身是否健康（用于整体托盘颜色 + tooltip 颜色）
    pub is_healthy: bool,
    /// Phase 1 新增：source id（字符串）。前端新代码用这个。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Phase 1 新增：显示名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_display_name: Option<String>,
    /// Phase 1 新增：套餐名（如 "Standard" / "Free tier"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    /// **L8 fix（2026-06-19）**：true = 这个 snapshot 是 placeholder（乐观 emit
    /// 给浮窗的临时态），不是真实 fetch 结果。浮窗应跳过"打开设置"按钮渲染，
    /// 避免 2-5s 真实 fetch 完成前的闪烁。None / false = 真实快照，正常渲染。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transient: Option<bool>,
}

impl ProviderSnapshot {
    /// 构造一个空的成功/失败快照（错误态用）
    ///
    /// `id` 是 source 的真实字符串 id（"minimax" / "tavily"），不是 `Provider` enum
    /// 变体名。原因：Tavily 等 Phase 1 起的新 source 没有自己的 enum 变体，
    /// `provider_from_id` 拿到 Tavily 会 fallback 到 `Provider::Minimax` —— 旧实现
    /// 直接用 `provider.id_str()` 写 `source_id` 会把 Tavily 错标成 "minimax"，
    /// 前端 PROVIDER_META 查表时 logo + 名字都串了。
    /// 现在从 builtin_sources 里查真正的 display_name，没有就 fallback 到 enum。
    ///
    /// PR 3 起 `async` —— [`find_source`] 改成 async（customs 在 `AppState` 里）。
    ///
    /// `transient` 参数（**L8 fix 2026-06-19**）：true = 这个 snapshot 是 placeholder
    /// 而非真实错误，浮窗跳过错误 UI 渲染。默认 false = 真实错误。
    pub async fn empty_error(
        state: &crate::AppState,
        provider: Provider,
        id: &str,
        kind: ErrorKind,
        error: String,
        transient: bool,
    ) -> Self {
        let display_name = find_source(state, id)
            .await
            .map(|s| s.display_name().to_string())
            .unwrap_or_else(|| provider.display_name().to_string());
        Self {
            provider,
            success: false,
            rows: vec![],
            error: Some(error),
            error_kind: Some(kind),
            fetched_at: Some(chrono::Utc::now().timestamp_millis()),
            // next_fetch_at 留 None:调用方在 record 完 backoff 后填
            // (record 后才能算下一次间隔)
            next_fetch_at: None,
            raw: None,
            is_healthy: false,
            source_id: Some(id.to_string()),
            source_display_name: Some(display_name),
            plan_name: None,
            transient: if transient { Some(true) } else { None },
        }
    }

    /// 轻量 placeholder：用于 `set_provider_enabled` 乐观 emit（2026-06-18
    /// fix-drag-delay 加）。
    ///
    /// 场景：用户拖拽已隐藏卡片到可见段，set_provider_enabled(true) 要触发
    /// HTTP fetch 才会有真数据。如果 await fetch，浮窗要等 2-5s 才会看到
    /// 新卡片。改成"先 emit placeholder → 浮窗立即显示 → 后台 fetch → 真
    /// 数据替换"，体验更跟手。
    ///
    /// placeholder 用 UnconfiguredKey 错误态 + **transient=true**（**L8 fix
    /// 2026-06-19**）：
    /// - 用户已配 key：fetch 完成后（< 2-5s）替换为真数据，中间窗口 frontend
    ///   看到 transient=true 跳过"打开设置"按钮渲染，无闪烁；
    /// - 用户没配 key：placeholder 就是终态，fetch 也返 UnconfiguredKey；
    ///   但 transient 仍然是 true（fetch 完成后真正的 UnconfiguredKey 替换它
    ///   —— transient 由真实 fetch emit 设为 None / false）。
    pub async fn placeholder(state: &crate::AppState, id: &str) -> Self {
        Self::empty_error(
            state,
            Provider::from_id_str(id),
            id,
            ErrorKind::UnconfiguredKey,
            String::new(),
            true, // L8: transient flag
        )
        .await
    }

    /// 计算 health 等级：ok / warn / alert / unknown
    ///
    /// `wallet_alert_threshold`: 全局余额告警阈值。
    /// - None → 不消费,余额类 source 永远走 "ok"(utilization = 0.0)
    /// - Some(n) → 任一 row 的 `remaining` < n 时直接返 "alert",不再走 utilization 分支
    ///
    /// 设计:这个检查在 match self.provider 之前——DeepSeek 走 is_healthy 分支
    /// 时(API 直接告诉钱包健康状态),如果 remaining 也 < 阈值,balance_low 更
    /// 重要,优先翻红。Minimax / Xiaomimimo 没 remaining,这条短路无效,继续
    /// 走 utilization 判断。
    ///
    /// **L9 fix（2026-06-19）**：之前 `success=true && rows=[]`（例如 schema
    /// 漂移 / 新行类型 backend 还没识别 / Tavily 等返回空数据但 success=true）
    /// 走 "rows.filter_map(utilization).next().unwrap_or(0.0)" → u=0.0 → "ok"，
    /// 托盘显示绿色 dot + 空内容，UX 死锁。改为直接返 "unknown"（中间色）。
    pub fn health_label(&self, wallet_alert_threshold: Option<f64>) -> &'static str {
        if !self.success {
            return "alert";
        }
        // L9: rows 为空时不要假装健康（即使 success=true）。这种情况多是 schema
        // 漂移或新行类型 backend 暂未支持，UI 应用 unknown（中间色 ⚪）提示
        // 用户"数据未识别"而非"数据正常"。DeepSeek 走 is_healthy 分支时即便
        // rows 为空也可能有意义（API 自身告诉健康状态），下面 match 会再覆盖。
        if self.rows.is_empty() {
            return "unknown";
        }
        if let Some(threshold) = wallet_alert_threshold {
            if self.rows.iter().any(|r| {
                r.remaining.map(|rem| rem < threshold).unwrap_or(false)
            }) {
                return "alert";
            }
        }
        match self.provider {
            Provider::Deepseek => {
                if self.is_healthy { "ok" } else { "alert" }
            }
            Provider::Minimax | Provider::Xiaomimimo => {
                // 取第一个有 utilization 的 row
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

// ── 顶层快照 ────────────────────────────────────────────────────────

/// 顶层快照：所有 provider 一次刷新的合集
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuotaSnapshot {
    pub providers: Vec<ProviderSnapshot>,
    /// 最近一次任一 provider 刷新的时间戳
    pub fetched_at: Option<i64>,
    /// 全局"余额/钱包行低额告警"阈值。None = 关闭(余额类 source 不会因为
    /// remaining < N 翻红); Some(n) = 任一 row 的 remaining < n 时 health_label
    /// 返 "alert",托盘 dot 翻红。
    ///
    /// 从 AppConfig.wallet_alert_threshold 复制,前端不需要再单独传(看 tray tooltip
    /// 渲染逻辑)。每个 refresh 路径(refresh_inner / refresh_now / refresh_single_inner)
    /// 都要 populate 这个字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_alert_threshold: Option<f64>,
}

impl QuotaSnapshot {
    /// 整体最差 health（用于托盘图标颜色）
    pub fn worst_health(&self) -> &'static str {
        let mut worst = "ok";
        for p in &self.providers {
            let h = p.health_label(self.wallet_alert_threshold);
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

// ── QuotaSource trait + 注册表（Phase 1 核心）────────────────────────

/// 单个 quota source 的"身份 + 鉴权 + endpoint"配置。
///
/// 写死每个 source 的静态属性（id / display / 默认 endpoint 等）。
/// 运行时（per-fetch）需要的东西（region、overrides、用户填的 key）通过
/// [`QuotaSource::fetch`] 的参数传进去。
pub trait QuotaSource: Send + Sync {
    /// 稳定字符串 id（"minimax" / "tavily" / `"custom_<uuid>"`）
    ///
    /// PR 3 起改 `Cow<'_, str>`：内置 source 返 `Cow::Borrowed(...)`（零分配），
    /// [`CustomSource`] 返 `Cow::Owned(spec.id.clone())`。
    fn id(&self) -> Cow<'_, str>;
    /// 给用户看的名字（"MiniMax" / "Tavily" / 用户自定义 `"DMX API"`）
    ///
    /// 同 [`id`](Self::id)，PR 3 起 `Cow<'_, str>`。
    fn display_name(&self) -> Cow<'_, str>;
    /// 鉴权方式（决定设置面板显示什么输入框）
    fn auth_kind(&self) -> AuthKind;
    /// 默认是否启用。**false = STUB**（公开 API 无 quota endpoint，等官方
    /// 开放后再实装）。Poller / refresh_inner 在用户没显式配置时跳过。
    /// 用户在设置面板可显式勾选启用（覆盖默认值），多用于"提前知道是 stub
    /// 也想看其他卡片的布局"。
    ///
    /// 默认 `true`（绝大多数 provider 是真实现）。
    fn default_enabled(&self) -> bool { true }
    /// 是否是 STUB（公开 API 无 quota endpoint、fetch 永远返 `error.provider.not_supported`）。
    /// UI 用这个加灰显 + "未支持" 角标，避免用户配 key 后看到 30 min 退避风暴。
    ///
    /// 默认 `false`。
    fn is_stub(&self) -> bool { false }
    /// 更新运行时状态（region / overrides）。`value` 是 [`AppConfig`] 的
    /// 完整 JSON 序列化，source 自己按需取字段。无状态的 source 可以忽略。
    ///
    /// Phase 1 用这个来替代 downcast —— typed dispatch 痛点 + dyn 没法
    /// 装成具体类型，所以用"发 JSON 让 source 自己解析"的折中。
    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
    /// 拉数据。`credentials` 里能拿到这个 source 需要的凭据（api_key / cookie）。
    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>;
}

/// 全部内置 source 的注册表。commands.rs 和 dump CLI 都从这里拿 source。
///
/// **新增 source 只需要在这里加一行**。
///
/// ## 顺序 = 浮窗卡片默认顺序（cfg.provider_order 为空时）
///
/// 历史顺序：minimax / deepseek / xiaomi / tavily / zenmux / openrouter / kimi / zhipu
/// 2026-06-16 新增 5 个：
/// - **stepfun**：Oasis-Token（手动粘贴），Step Plan 套餐用量
/// - **siliconflow**：Bearer，硅基流动钱包余额
/// - **novita** / **qwen**：STUB，公开 API 无 quota endpoint，fetch 永久返回
///   "未支持"错（前端可见，UI 不报错）
/// - **claude_official**：Cookie，Claude Pro/Max 官方 OAuth 用量
pub fn builtin_sources() -> Vec<Box<dyn QuotaSource>> {
    vec![
        Box::new(minimax::MinimaxSource::default()),
        Box::new(deepseek::DeepseekSource::default()),
        Box::new(xiaomi::XiaomimimoSource::default()),
        Box::new(tavily::TavilySource::default()),
        Box::new(zenmux::ZenmuxSource::default()),
        Box::new(openrouter::OpenrouterSource::default()),
        Box::new(kimi::KimiSource::default()),
        Box::new(zhipu::ZhipuSource::default()),
        // 2026-06-16 新增（PR 2）
        Box::new(stepfun::StepfunSource::default()),
        Box::new(siliconflow::SiliconflowSource::default()),
        Box::new(novita::NovitaSource::default()),
        Box::new(qwen::QwenSource::default()),
        Box::new(claude_official::ClaudeOfficialSource::default()),
    ]
}

/// 按 id 查 source（**异步**，PR 3 起 —— customs 在 `AppState` 里，需要 await lock）。
///
/// ## Lock 顺序约定
///
/// 调用方在持 `state.config` 锁的情况下**不能**调本函数（会死锁）——
/// `all_sources` 先拿 `state.custom_sources.read()` 再用 `builtin_sources()`
/// 同步版（无锁），不冲突；但拿 `state.config` 后又调本函数会形成
/// config → custom_sources → ... 的反向锁链。
pub async fn find_source(state: &crate::AppState, id: &str) -> Option<Box<dyn QuotaSource>> {
    all_sources(state).await.into_iter().find(|s| s.id() == id)
}

/// 全部 source 的注册表（内置 + 用户自定义）。async 是因为 customs 在
/// `AppState.custom_sources` 里，需要拿 lock。
///
/// **绝大多数 commands 都应该走这个**而不是 `builtin_sources()`，否则 customs
/// 不会被 poller / refresh_inner 看到。
pub async fn all_sources(state: &crate::AppState) -> Vec<Box<dyn QuotaSource>> {
    let mut sources = builtin_sources();
    let customs = state.custom_sources.read().await;
    for spec in customs.iter() {
        sources.push(Box::new(custom::CustomSource::new(spec.clone())));
    }
    sources
}

// ── 共享 HTTP client ────────────────────────────────────────────────

/// 进程内共享的 [`reqwest::Client`]。
///
/// 避免每次 poll 都重建 client（每个 provider 各重建一次，10s + 5s timeout + UA
/// 全是重复代码 → M2 review 建议）。
///
/// 何时不要共享：per-source TLS tuning（目前没有），per-source proxy（没有）。
/// 等真有需求再切回 per-source。
static SHARED_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

pub fn shared_client() -> &'static reqwest::Client {
    SHARED_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .user_agent(concat!("Musage/", env!("CARGO_PKG_VERSION")))
            // M9 fix: 长跑 tray app idle TCP 永久堆积。每个 host 最多 2 idle conn，
            // 30s 没流量就关。
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("build shared reqwest client")
    })
}

// ── 兼容旧代码：ProviderImpl trait 保留为 dead-code ────────────────────

/// 旧的 trait。新代码请用 [`QuotaSource`]。
///
/// 留着是因为 [`crate::commands`] 旧路径和 `dump` CLI 还引用；
/// Phase 2 删。
#[allow(dead_code)]
pub trait ProviderImpl: Send + Sync {
    fn id(&self) -> Provider;
    fn display_name(&self) -> &'static str;
    fn fetch<'a>(
        &'a self,
        api_key: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ProviderSnapshot, String>> + Send + 'a>,
    >;
}

// ── 单元测试 fixture（共享 JSON） ───────────────────────────────────

#[cfg(test)]
pub(crate) mod test_fixtures {
    use serde_json::json;

    pub fn minimax_new_schema() -> serde_json::Value {
        json!({
            "base_resp": { "status_code": 0, "status_msg": "success" },
            "model_remains": [{
                "model_name": "general",
                "current_interval_remaining_percent": 72,
                "current_interval_status": 1,
                "end_time": 14523,
                "current_weekly_remaining_percent": 86,
                "current_weekly_status": 1,
                "weekly_end_time": 803245
            }]
        })
    }
}
