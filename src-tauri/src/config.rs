//! 应用配置 + keyring 集成
//!
//! 配置文件路径（%APPDATA%\com.musage.app\config.json）：
//! - region: "cn" / "en"
//! - refresh_interval_secs: 拉取间隔
//! - floating_x, floating_y: 悬浮窗位置
//!
//! API key 存到 OS keyring，不写文件。

use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::api::Region;

const KEYRING_SERVICE: &str = "com.musage.app";
const KEYRING_USER: &str = "api_key";
const CONFIG_FILE: &str = "config.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub region: Region,
    pub refresh_interval_secs: u64,
    pub floating_x: Option<i32>,
    pub floating_y: Option<i32>,
    pub autostart: bool,
    pub show_in_tray_on_close: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            region: Region::Cn,
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
        let s = std::fs::read_to_string(&path)
            .map_err(|e| format!("read config: {e}"))?;
        let cfg: AppConfig = serde_json::from_str(&s)
            .map_err(|e| format!("parse config: {e}"))?;
        Ok(cfg)
    }

    /// 兼容入口：带 AppHandle 时使用
    pub fn load(_app: &AppHandle) -> Result<Self, String> {
        Self::load_from_disk()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("mkdir: {e}"))?;
        }
        let s = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize: {e}"))?;
        std::fs::write(&path, s)
            .map_err(|e| format!("write config: {e}"))?;
        Ok(())
    }
}

fn config_path() -> Result<PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or_else(|| "无法定位配置目录".to_string())?;
    Ok(dir.join("com.musage.app").join(CONFIG_FILE))
}

// ── Keyring ──────────────────────────────────────────────

pub fn load_api_key_from_keyring() -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("keyring entry: {e}"))?;
    match entry.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring read: {e}")),
    }
}

pub fn save_api_key_to_keyring(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("keyring entry: {e}"))?;
    entry.set_password(key).map_err(|e| format!("keyring write: {e}"))
}

pub fn delete_api_key_from_keyring() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("keyring entry: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete: {e}")),
    }
}
