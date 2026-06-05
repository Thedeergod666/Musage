//! 应用配置 + keyring 集成
//!
//! 配置文件路径（%APPDATA%\com.musage.app\config.json）：
//! - providers: `{ minimax: { enabled, region }, deepseek: { enabled } }`
//! - refresh_interval_secs: 拉取间隔
//! - floating_x, floating_y: 悬浮窗位置
//!
//! API key 存到 OS keyring，user 按 provider 命名（`api_key:minimax` / `api_key:deepseek`），
//! 不写文件。
//!
//! ## 向后兼容
//!
//! 旧 config.json 顶层有 `region: "cn"` 字段（v0.1 格式），加载时自动迁到
//! `providers.minimax.region`；用户需要重新输入 key（旧 `api_key` credential 不复用）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::providers::minimax::Region;
use crate::providers::Provider;

const KEYRING_SERVICE: &str = "com.musage.app";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub enabled: bool,
    /// 仅 MiniMax 用（DeepSeek 没 region 概念，序列化时跳过）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            region: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// provider id → 配置
    pub providers: BTreeMap<String, ProviderConfig>,
    pub refresh_interval_secs: u64,
    pub floating_x: Option<i32>,
    pub floating_y: Option<i32>,
    pub autostart: bool,
    pub show_in_tray_on_close: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert(
            Provider::Minimax.id_str().to_string(),
            ProviderConfig {
                enabled: true,
                region: Some(Region::Cn),
            },
        );
        providers.insert(
            Provider::Deepseek.id_str().to_string(),
            ProviderConfig {
                enabled: true,
                region: None,
            },
        );
        Self {
            providers,
            refresh_interval_secs: 60,
            floating_x: None,
            floating_y: None,
            autostart: false,
            show_in_tray_on_close: true,
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
                },
            );
            cfg.refresh_interval_secs = legacy.refresh_interval_secs.unwrap_or(60);
            cfg.floating_x = legacy.floating_x;
            cfg.floating_y = legacy.floating_y;
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
                    },
                    Provider::Deepseek => ProviderConfig {
                        enabled: true,
                        region: None,
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

    /// 启用/禁用某个 provider
    pub fn set_enabled(&mut self, provider: Provider, enabled: bool) {
        let entry = self
            .providers
            .entry(provider.id_str().to_string())
            .or_insert_with(|| match provider {
                Provider::Minimax => ProviderConfig {
                    enabled,
                    region: Some(Region::Cn),
                },
                Provider::Deepseek => ProviderConfig {
                    enabled,
                    region: None,
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
            });
        entry.region = Some(region);
    }

    /// 启用的 provider 列表（按 [`Provider::all`] 顺序）
    pub fn enabled_providers(&self) -> Vec<Provider> {
        Provider::all()
            .into_iter()
            .filter(|p| self.is_enabled(*p))
            .collect()
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

fn config_path() -> Result<PathBuf, String> {
    let dir = dirs::config_dir().ok_or_else(|| "无法定位配置目录".to_string())?;
    Ok(dir.join("com.musage.app").join(CONFIG_FILE))
}

// ── Keyring ──────────────────────────────────────────────

fn keyring_user(provider: Provider) -> String {
    format!("api_key:{}", provider.id_str())
}

pub fn load_api_key_for(provider: Provider) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_user(provider))
        .map_err(|e| format!("keyring entry: {e}"))?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring read: {e}")),
    }
}

pub fn save_api_key_for(provider: Provider, key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_user(provider))
        .map_err(|e| format!("keyring entry: {e}"))?;
    entry
        .set_password(key)
        .map_err(|e| format!("keyring write: {e}"))
}

pub fn delete_api_key_for(provider: Provider) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, &keyring_user(provider))
        .map_err(|e| format!("keyring entry: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete: {e}")),
    }
}
