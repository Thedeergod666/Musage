//! 用户自定义 New API source 的 spec 持久化（PR 3）
//!
//! ## 文件位置
//!
//! `<config_dir>/com.musage.app/custom_sources.json` —— 跟 `config.json` /
//! `keys.json` 同目录，结构 top-level array（不是 map）。
//!
//! ## 为什么不进 `config.json`
//!
//! - `config.json` 是用户偏好（region / enabled / interval / pin mode 等），
//!   用 `AppConfig` serde 重度耦合（每个字段都有 default 兼容逻辑）。把 spec 塞
//!   进去会让 schema 解析变脆
//! - 用户可能想 git diff / 备份 / 跨设备同步 `custom_sources.json`，
//!   跟 `keys.json` 独立成文件更友好
//! - 跟 `keys.json` 一样做原子写 + 0600，避免半写状态
//!
//! ## API key 不在这里
//!
//! 复用 [`super::save_credential_for_id`] / [`super::delete_credential_for_id`]，
//! key 名 = `custom_<uuid>`（UUID 由 `add_custom_source` 命令生成）。

use std::path::PathBuf;

use crate::providers::CustomSourceSpec;

const CUSTOM_SOURCES_FILE: &str = "custom_sources.json";

/// 加载所有 custom source specs。
///
/// 行为：
/// - 文件不存在 → `Ok(vec![])`
/// - 文件存在但 parse 失败 → 备份到 `.bak.<timestamp>` + `Ok(vec![])`
///   （防止 schema 改版后用户 spec 全部丢失，至少 backup 留下来）
/// - 文件为空字符串 → `Ok(vec![])`
pub fn load_custom_sources() -> Result<Vec<CustomSourceSpec>, String> {
    let path = custom_sources_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let s = std::fs::read_to_string(&path).map_err(|e| format!("read custom_sources.json: {e}"))?;
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Vec<CustomSourceSpec>>(&s) {
        Ok(v) => Ok(v),
        Err(e) => {
            // parse 失败：备份到 .bak.<ts> + 返空。避免一次坏写入把全部 spec 删了。
            // **2026-06-20 audit**：之前 backup 失败用 `let _ = std::fs::copy(...)`
            // 吞错，read-only 目录 / 满盘 → backup 失败 → 下次 save 用空 Vec
            // 覆盖 → 用户全部 custom source 静默丢失。改 error 级日志。
            let ts = chrono::Utc::now().timestamp();
            let backup = path.with_extension(format!("json.bak.{ts}"));
            if let Err(copy_err) = std::fs::copy(&path, &backup) {
                tracing::error!(
                    source = %path.display(),
                    backup = %backup.display(),
                    copy_error = %copy_err,
                    parse_error = %e,
                    "custom_sources.json 解析失败且备份失败 — 下次 save 会用空 Vec 覆盖",
                );
            } else {
                tracing::warn!(
                    error = %e,
                    backup = %backup.display(),
                    "custom_sources.json parse 失败，已备份到 .bak",
                );
            }
            Ok(Vec::new())
        }
    }
}

/// 原子写：先写 .tmp + 0600，再 rename 覆盖（跟 [`super::write_keys_atomic`] 同款）。
/// M11 fix: 整个函数体在 save_lock() 保护下 —— 与 keys.json 写串行化，
/// 避免用户同时改 custom source + 改 key 时 cfg.save / keys 写 / customs 写
/// 三条路径互相竞争丢字段。
#[allow(dead_code)] // Phase E add/update/delete_custom_source IPC 会用
pub fn save_custom_sources(specs: &[CustomSourceSpec]) -> Result<(), String> {
    let _g = super::save_lock().lock().unwrap_or_else(|e| {
        tracing::warn!("save_custom_sources save_lock poisoned, recovering");
        e.into_inner()
    });
    let path = custom_sources_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(specs)
        .map_err(|e| format!("serialize custom_sources: {e}"))?;
    std::fs::write(&tmp, &s).map_err(|e| format!("write tmp: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod 0600: {e}"))?;
    }

    if let Err(e) = std::fs::rename(&tmp, &path) {
        // 跟 AppConfig::save 同款：rename 失败清理 tmp，避免下次启动看到孤儿
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("rename custom_sources: {e}"));
    }
    Ok(())
}

fn custom_sources_path() -> Result<PathBuf, String> {
    let dir = super::config_dir()?;
    Ok(dir.join("com.musage.app").join(CUSTOM_SOURCES_FILE))
}

// ── 单元测试（仅函数 + 文件 IO 部分，序列化在 spec crate 里） ──

#[cfg(test)]
mod tests {
    use super::*;
    // 完整路径(不走 re-export),同 `commands/custom_sources.rs` 测试模块
    use crate::providers::custom::ExtractSpec;
    use serde_json::json;
    // 本文件顶层 use 删了 `std::collections::BTreeMap`（非测试代码用不到）,
    // 但 `keys_map_contains_uuid` 测试还在用,这里局部 import。
    use std::collections::BTreeMap;

    fn sample_spec(id: &str) -> CustomSourceSpec {
        CustomSourceSpec {
            id: id.to_string(),
            display_name: format!("Test {id}"),
            base_url: "https://api.test.com".to_string(),
            path: "/api/user/self".to_string(),
            method: "GET".to_string(),
            extract: ExtractSpec::NewApi { divide: None },
            plan_name_path: None,
            accent: None,
            created_at: 1700000000,
        }
    }

    #[test]
    fn serialize_and_back() {
        let specs = vec![sample_spec("custom_a1"), sample_spec("custom_b2")];
        let s = serde_json::to_string(&specs).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        // 顶层是 array（不是 map），方便 git diff 和手编辑
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn extract_spec_newapi_serde() {
        let spec = sample_spec("custom_x");
        let s = serde_json::to_string(&spec.extract).unwrap();
        // tag = "preset", variant = "new_api"
        assert!(s.contains("\"preset\":\"new_api\""));
        let back: ExtractSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, spec.extract);
    }

    #[test]
    fn extract_spec_balance_serde() {
        let extract = ExtractSpec::Balance {
            balance_path: "data.credit".to_string(),
            currency_path: Some("data.unit".to_string()),
            divide: Some(100.0),
        };
        let s = serde_json::to_string(&extract).unwrap();
        assert!(s.contains("\"preset\":\"balance\""));
        assert!(s.contains("\"balance_path\":\"data.credit\""));
        let back: ExtractSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, extract);
    }

    #[test]
    fn extract_spec_custom_serde() {
        let extract = ExtractSpec::Custom {
            remaining_path: Some("x".to_string()),
            used_path: None,
            total_path: None,
            unit: Some("USD".to_string()),
            divide: None,
        };
        let s = serde_json::to_string(&extract).unwrap();
        assert!(s.contains("\"preset\":\"custom\""));
        let back: ExtractSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, extract);
    }

    #[test]
    fn keys_map_contains_uuid() {
        // 验证 keys.json 的 key 名格式（约定，不是测试代码）
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        map.insert("custom_a1b2c3d4".to_string(), "sk-test".to_string());
        let s = serde_json::to_string(&map).unwrap();
        assert!(s.contains("\"custom_a1b2c3d4\""));
    }

    #[test]
    fn json_value_back_compat() {
        // parse 失败的备份路径：构造一个坏 JSON 看 deserialize 报错
        let bad = json!({"this": "is", "not": "an array"}).to_string();
        let result: Result<Vec<CustomSourceSpec>, _> = serde_json::from_str(&bad);
        assert!(result.is_err());
    }
}
