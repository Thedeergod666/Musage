//! 应用配置 + 本地密钥存储
//!
//! 配置文件路径（%APPDATA%\com.musage.app\config.json）：
//! - providers: `{ minimax: { enabled, region }, deepseek: { enabled } }`
//! - refresh_interval_secs: 拉取间隔
//! - floating_x, floating_y: 悬浮窗位置
//!
//! API key 存到独立文件 `keys.json`（同目录），按 provider id 命名（`minimax` / `deepseek`）。
//! Unix 上文件权限强制 `0600`（仅当前用户可读写）；Windows 靠 NTFS 默认 ACL。
//!
//! 之前用 OS keyring（`keyring` crate），macOS 上每次启动会弹 Keychain 访问窗 + 解锁
//! 登录钥匙串的密码框，体验很糟。改成纯文件后启动零弹窗。
//!
//! ## 向后兼容
//!
//! 旧 config.json 顶层有 `region: "cn"` 字段（v0.1 格式），加载时自动迁到
//! `providers.minimax.region`。`keys.json` 没有历史包袱，直接从空开始。
//! 升级到本版本后用户需要重新输入一次 API key（keyring 的旧条目不再被读取）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::providers::minimax::Region;
use crate::providers::xiaomi::{XiaomiDisplayMode, XiaomiRegion};
use crate::providers::Provider;

const CONFIG_FILE: &str = "config.json";
const KEYS_FILE: &str = "keys.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub enabled: bool,
    /// MiniMax 用（DeepSeek 没 region 概念，序列化时跳过）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// Xiaomi MiMo 用（CN/SGP/AMS，序列化时跳过）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xiaomi_region: Option<XiaomiRegion>,
    /// 可选：覆盖全局轮询间隔（秒）。None = 用 AppConfig.refresh_interval_secs。
    /// Poller 拿这个值 per-provider 调度 —— 用户可以为不常变动的 provider
    /// 设长间隔（节流），重要的设短。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_interval_secs: Option<u64>,
    /// Xiaomi MiMo 用：浮窗显示模式（All / PlanOnly / TotalOnly）。
    /// None = 默认 All。序列化时跳过 None，老 config.json 不带这字段也能解析。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xiaomi_display_mode: Option<XiaomiDisplayMode>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            region: None,
            xiaomi_region: None,
            refresh_interval_secs: None,
            xiaomi_display_mode: None,
        }
    }
}

/// 浮窗置顶/置底行为
///
/// - `PinTop`   ：浮窗一直在最上层（系统 always-on-top 模式）
/// - `PinBottom`：默认在底部（不 always-on-top，会被其它窗口盖住），
///                鼠标 hover 进浮窗时临时切到置顶，鼠标离开后回到置底
/// - `Normal`   ：不强制层级，跟普通窗口一样（被聚焦时在前，失焦后被盖住）
///
/// 序列化用 snake_case 字符串，向后兼容旧 config（缺字段 → PinTop，老版本的默认行为）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FloatingPinMode {
    #[default]
    PinTop,
    PinBottom,
    Normal,
}

impl FloatingPinMode {
    pub fn is_serialized(&self) -> bool {
        // 所有枚举值都参与序列化；保留这个方法是为了语义一致
        true
    }
}

/// 托盘图标渲染样式
///
/// - `Logo`   ：画 [src-tauri/icons/tray-base.png](crate) 静态应用图标
///              （白底 + 黑 M + 黑细环），不显示实时数据
/// - `Bars`   ：MiniMax 双水平进度条（上 = 5h utilization，下 = 周 utilization）
///              —— v0.5.x 唯一可用的样式
/// - `Percent`：MiniMax 双行百分比文本（上 "5h 45%"，下 "周 72%"）
///              —— v0.6+ 默认
///
/// 序列化 snake_case 字符串。`#[serde(default)]` 让老 config.json 缺字段时
/// 走 `Percent`（v0.6 起的行为变更）。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TrayIconStyle {
    Logo,
    Bars,
    #[default]
    Percent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// provider id → 配置
    pub providers: BTreeMap<String, ProviderConfig>,
    pub refresh_interval_secs: u64,
    pub floating_x: Option<i32>,
    pub floating_y: Option<i32>,
    /// 浮窗宽度（用户手动 resize 后记住）。None = 用 tauri.conf.json 默认值
    pub floating_w: Option<i32>,
    /// 浮窗高度
    pub floating_h: Option<i32>,
    /// 浮窗置顶/置底模式（缺省 = PinTop，保持旧版本行为）
    #[serde(default)]
    pub floating_pin_mode: FloatingPinMode,
    pub autostart: bool,
    /// 关闭主窗口时是否隐藏到托盘。serde_default 兜住老 config.json
    /// 缺这字段（v0.1 加的字段，老用户第一次保存会缺）。
    #[serde(default = "default_show_in_tray_on_close")]
    pub show_in_tray_on_close: bool,
    /// 省电模式：禁用 backdrop-filter 模糊 + 所有 CSS transition。
    /// 适合老 Intel Mac / 非 macOS WebView 性能不足时使用。
    /// 默认 false（开启玻璃材质）。前端通过 `body[data-low-power]` 属性响应。
    #[serde(default)]
    pub low_power_mode: bool,
    /// 全屏时自动隐藏浮窗（macOS 通过 NSMenu.menuBarVisible 检测）。
    /// 退出全屏后自动恢复显示。非 macOS 平台 stub（暂不支持）。
    /// 默认 false（保持原有行为：全屏时浮窗仍可能露出）。
    #[serde(default)]
    pub auto_hide_in_fullscreen: bool,
    /// Tavily 简洁模式：只显示 Free tier 主行（"209/1000 credits"），隐藏
    /// 5 个 endpoint 细分（search/extract/crawl/map/research）。默认 true
    /// —— 6 行挤在小窗里太啰嗦；想看明细可去设置面板关掉。
    #[serde(default = "tavily_concise_default")]
    pub tavily_concise_mode: bool,
    /// 用户手动指定的 provider 显示/轮询顺序（用 id 字符串）。空 Vec
    /// = 用 builtin_sources() 的注册表顺序。设置面板拖拽/上下按钮改
    /// 这个；poller 按这个顺序排，浮窗也按这个顺序渲染卡片。
    #[serde(default)]
    pub provider_order: Vec<String>,
    /// 用户自定义的字段名候选（应对 MiniMax 改 schema）
    /// key = provider.id_str()，value = 该 provider 的 overrides
    #[serde(default)]
    pub schema_overrides: BTreeMap<String, ProviderOverrides>,
    /// 托盘图标渲染样式（v0.6+ 新增）。`#[serde(default)]` 让老 config.json
    /// 缺字段时走 `Percent`（也是新装用户默认值）。
    #[serde(default)]
    pub tray_icon_style: TrayIconStyle,
}

const fn tavily_concise_default() -> bool {
    true
}

const fn default_show_in_tray_on_close() -> bool {
    true
}

/// 单个 tier（5h / 周 / 月等）的字段名 overrides
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TierOverrides {
    /// count-based schema 的候选三元组（total/remaining/end）
    /// 解析时按顺序尝试，用户在前 = 优先
    #[serde(default)]
    pub count_candidates: Vec<FieldTriple>,
}

/// count-based schema 的字段名三元组
///
/// `total` + `remaining` 同时存在且 total > 0 时认为命中。
/// `end` 可选（旧 schema 通常是 epoch ms，新 schema 是 duration 秒，smart_reset_to_ms 会自动识别）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldTriple {
    pub total: String,
    pub remaining: String,
    #[serde(default)]
    pub end: Option<String>,
}

/// 单个 provider 的全部 overrides
///
/// 各 provider 用各自用得到的 tier：
/// - MiniMax：`five_hour` + `weekly`
/// - Xiaomi MiMo：`monthly`
/// - DeepSeek：不走 schema_overrides（响应固定）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderOverrides {
    #[serde(default)]
    pub five_hour: TierOverrides,
    #[serde(default)]
    pub weekly: TierOverrides,
    #[serde(default)]
    pub monthly: TierOverrides,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            Provider::Minimax.id_str().to_string(),
            ProviderConfig {
                enabled: true,
                region: Some(Region::Cn),
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
        );
        providers.insert(
            Provider::Deepseek.id_str().to_string(),
            ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
        );
        providers.insert(
            Provider::Xiaomimimo.id_str().to_string(),
            ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: Some(XiaomiRegion::Cn),
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
        );
        // Phase 1: Tavily 作为第一个非 AI provider，默认 enabled。
        // 没有 region 概念，所以 region/xiaomi_region 都 None。
        providers.insert(
            "tavily".to_string(),
            ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
        );
        Self {
            providers,
            refresh_interval_secs: 60,
            floating_x: None,
            floating_y: None,
            floating_w: None,
            floating_h: None,
            floating_pin_mode: FloatingPinMode::default(),
            autostart: false,
            show_in_tray_on_close: true,
            low_power_mode: false,
            auto_hide_in_fullscreen: false,
            tavily_concise_mode: true,
            provider_order: Vec::new(),
            schema_overrides: BTreeMap::new(),
            tray_icon_style: TrayIconStyle::default(),
        }
    }
}

impl AppConfig {
    /// 从磁盘加载；不存在或损坏则返回默认
    pub fn load_from_disk() -> Result<Self, String> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(&path).map_err(|e| format!("read config: {e}"))?;

        // 尝试新格式
        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&s) {
            return Ok(cfg.migrated());
        }

        // 旧格式：顶层 region 字段 + 无 providers
        #[derive(Deserialize)]
        struct Legacy {
            region: Option<Region>,
            refresh_interval_secs: Option<u64>,
            floating_x: Option<i32>,
            floating_y: Option<i32>,
            floating_w: Option<i32>,
            floating_h: Option<i32>,
            autostart: Option<bool>,
            show_in_tray_on_close: Option<bool>,
        }
        if let Ok(legacy) = serde_json::from_str::<Legacy>(&s) {
            tracing::info!("检测到旧版 config.json，自动迁移到多 provider 格式");
            let mut cfg = AppConfig::default();
            cfg.providers.insert(
                Provider::Minimax.id_str().to_string(),
                ProviderConfig {
                    enabled: true,
                    region: legacy.region.or(Some(Region::Cn)),
                    xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
            );
            cfg.refresh_interval_secs = legacy.refresh_interval_secs.unwrap_or(60);
            cfg.floating_x = legacy.floating_x;
            cfg.floating_y = legacy.floating_y;
            cfg.floating_w = legacy.floating_w;
            cfg.floating_h = legacy.floating_h;
            cfg.floating_pin_mode = FloatingPinMode::default();
            cfg.autostart = legacy.autostart.unwrap_or(false);
            cfg.show_in_tray_on_close = legacy.show_in_tray_on_close.unwrap_or(true);
            // 落盘
            let _ = cfg.save();
            return Ok(cfg);
        }

        Err(format!(
            "config.json 格式无法识别（既不是新格式也不是旧格式）：{}",
            path.display()
        ))
    }

    /// 兼容入口：带 AppHandle 时使用
    pub fn load(_app: &AppHandle) -> Result<Self, String> {
        Self::load_from_disk()
    }

    /// 加载后兜底：补齐所有 provider（防止老配置文件缺了某个）
    fn migrated(mut self) -> Self {
        for p in Provider::all() {
            self.providers
                .entry(p.id_str().to_string())
                .or_insert_with(|| match p {
                    Provider::Minimax => ProviderConfig {
                        enabled: true,
                        region: Some(Region::Cn),
                        xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
                    Provider::Deepseek => ProviderConfig {
                        enabled: true,
                        region: None,
                        xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
                    Provider::Xiaomimimo => ProviderConfig {
                        enabled: true,
                        region: None,
                        xiaomi_region: Some(XiaomiRegion::Cn),
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
                });
        }
        self
    }

    /// 取某个 provider 的 enabled 状态（缺省视为 true）
    pub fn is_enabled(&self, provider: Provider) -> bool {
        self.providers
            .get(provider.id_str())
            .map(|c| c.enabled)
            .unwrap_or(true)
    }

    /// 取 MiniMax 的 region（其他 provider 返回默认 CN）
    pub fn region(&self) -> Region {
        self.providers
            .get(Provider::Minimax.id_str())
            .and_then(|c| c.region)
            .unwrap_or(Region::Cn)
    }

    /// 取 Xiaomi MiMo 的 region（默认 CN）
    pub fn xiaomi_region(&self) -> XiaomiRegion {
        self.providers
            .get(Provider::Xiaomimimo.id_str())
            .and_then(|c| c.xiaomi_region)
            .unwrap_or(XiaomiRegion::Cn)
    }

    /// 启用/禁用某个 provider
    pub fn set_enabled(&mut self, provider: Provider, enabled: bool) {
        let entry = self
            .providers
            .entry(provider.id_str().to_string())
            .or_insert_with(|| match provider {
                Provider::Minimax => ProviderConfig {
                    enabled,
                    region: Some(Region::Cn),
                    xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
                Provider::Deepseek => ProviderConfig {
                    enabled,
                    region: None,
                    xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
                Provider::Xiaomimimo => ProviderConfig {
                    enabled,
                    region: None,
                    xiaomi_region: Some(XiaomiRegion::Cn),
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            },
            });
        entry.enabled = enabled;
    }

    /// 设置 MiniMax 的 region
    pub fn set_region(&mut self, region: Region) {
        let entry = self
            .providers
            .entry(Provider::Minimax.id_str().to_string())
            .or_insert(ProviderConfig {
                enabled: true,
                region: Some(region),
                xiaomi_region: None,
                refresh_interval_secs: None,
            });
        entry.region = Some(region);
    }

    /// 设置 Xiaomi MiMo 的 region
    pub fn set_xiaomi_region(&mut self, region: XiaomiRegion) {
        let entry = self
            .providers
            .entry(Provider::Xiaomimimo.id_str().to_string())
            .or_insert(ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: Some(region),
                refresh_interval_secs: None,
            });
        entry.xiaomi_region = Some(region);
    }

    /// 启用的 provider 列表（按 [`Provider::all`] 顺序）
    pub fn enabled_providers(&self) -> Vec<Provider> {
        Provider::all()
            .into_iter()
            .filter(|p| self.is_enabled(*p))
            .collect()
    }

    /// 按 source id 查 enabled（用于 registry 驱动的轮询循环）。
    /// 缺省视为 true（首次启动时还没这个 key 的 entry，按"启用"处理）。
    pub fn is_enabled_id(&self, id: &str) -> bool {
        self.providers.get(id).map(|c| c.enabled).unwrap_or(true)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        let s = serde_json::to_string_pretty(self).map_err(|e| format!("serialize: {e}"))?;
        std::fs::write(&path, s).map_err(|e| format!("write config: {e}"))?;
        Ok(())
    }
}

fn config_dir() -> Result<PathBuf, String> {
    dirs::config_dir().ok_or_else(|| "无法定位配置目录".to_string())
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("com.musage.app").join(CONFIG_FILE))
}

// ── 本地文件存 key（替代 OS keyring）─────────────────────

/// `keys.json` 存储格式：`{ "minimax": "sk-cp-...", "deepseek": "sk-..." }`
type KeysMap = BTreeMap<String, String>;

fn keys_path() -> Result<PathBuf, String> {
    let dir = config_dir()?;
    Ok(dir.join("com.musage.app").join(KEYS_FILE))
}

/// 原子写：先写 .tmp 文件 + 设 0600 权限，再 rename 覆盖。
/// 避免半写状态把 key 写坏 / 漏权限。
fn write_keys_atomic(map: &KeysMap) -> Result<(), String> {
    let path = keys_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(map).map_err(|e| format!("serialize keys: {e}"))?;
    std::fs::write(&tmp, &s).map_err(|e| format!("write tmp: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod 0600: {e}"))?;
    }

    std::fs::rename(&tmp, &path).map_err(|e| format!("rename keys: {e}"))?;
    Ok(())
}

fn read_keys() -> Result<KeysMap, String> {
    let path = keys_path()?;
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let s = std::fs::read_to_string(&path).map_err(|e| format!("read keys: {e}"))?;
    if s.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    serde_json::from_str::<KeysMap>(&s).map_err(|e| format!("parse keys.json: {e}"))
}

pub fn load_api_key_for(provider: Provider) -> Result<Option<String>, String> {
    let map = read_keys()?;
    Ok(map.get(provider.id_str()).cloned())
}

pub fn save_api_key_for(provider: Provider, key: &str) -> Result<(), String> {
    let mut map = read_keys().unwrap_or_default();
    map.insert(provider.id_str().to_string(), key.to_string());
    write_keys_atomic(&map)
}

pub fn delete_api_key_for(provider: Provider) -> Result<(), String> {
    let mut map = read_keys().unwrap_or_default();
    map.remove(provider.id_str());
    if map.is_empty() {
        // 全部删完就连文件一起删，避免空文件 + 0 字节文件混在目录里
        let path = keys_path()?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("remove empty keys: {e}"))?;
        }
    } else {
        write_keys_atomic(&map)?;
    }
    Ok(())
}

// ── 通用 secret 存取（用于 cookie / 其它 token） ─────────────

fn cookie_key(provider: Provider) -> String {
    format!("{}:cookie", provider.id_str())
}

pub fn load_cookie_for(provider: Provider) -> Result<Option<String>, String> {
    let map = read_keys()?;
    Ok(map.get(&cookie_key(provider)).cloned())
}

pub fn save_cookie_for(provider: Provider, cookie: &str) -> Result<(), String> {
    let mut map = read_keys().unwrap_or_default();
    map.insert(cookie_key(provider), cookie.to_string());
    write_keys_atomic(&map)
}

pub fn delete_cookie_for(provider: Provider) -> Result<(), String> {
    let mut map = read_keys().unwrap_or_default();
    map.remove(&cookie_key(provider));
    if map.is_empty() {
        let path = keys_path()?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("remove empty keys: {e}"))?;
        }
    } else {
        write_keys_atomic(&map)?;
    }
    Ok(())
}

// ── 按 source id 操作的凭据（Phase 1 新 API，registry-driven） ───────

use crate::providers::Credentials;

pub fn load_credential_for_id(id: &str) -> Result<Option<Credentials>, String> {
    let map = read_keys()?;
    let api_key = map.get(id).cloned();
    let cookie = map.get(&format!("{id}:cookie")).cloned();
    Ok(if api_key.is_some() || cookie.is_some() {
        Some(Credentials { api_key, cookie })
    } else {
        None
    })
}

pub fn save_credential_for_id(id: &str, cred: &Credentials) -> Result<(), String> {
    let mut map = read_keys().unwrap_or_default();
    match (&cred.api_key, &cred.cookie) {
        (Some(k), _) => { map.insert(id.to_string(), k.clone()); }
        (None, Some(c)) => { map.insert(format!("{id}:cookie"), c.clone()); }
        (None, None) => {
            map.remove(id);
            map.remove(&format!("{id}:cookie"));
        }
    }
    write_keys_atomic(&map)
}

pub fn delete_credential_for_id(id: &str) -> Result<(), String> {
    let mut map = read_keys().unwrap_or_default();
    map.remove(id);
    map.remove(&format!("{id}:cookie"));
    if map.is_empty() {
        let path = keys_path()?;
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("remove empty keys: {e}"))?;
        }
    } else {
        write_keys_atomic(&map)?;
    }
    Ok(())
}
