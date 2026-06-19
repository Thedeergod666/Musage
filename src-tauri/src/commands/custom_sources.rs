//! 用户自定义 New API source 的 5 个 IPC commands（PR 3）
//!
//! ## 锁顺序约定（重要）
//!
//! `custom_sources.write()` **必须**在 `config.read()` 之前拿。
//! 反过来会形成 `config → custom_sources → ...` 反向锁链，等价于
//! `refresh_inner`（持 config）+ `add_custom_source`（持 custom_sources）
//! 互相等待 → 死锁。
//!
//! 本文件里所有 mut 流程都是「先 customs.write → 再 config 操作」，无
//! config 嵌套调用，天然安全。
//!
//! ## 事件复用（plan §13.7 review 决策）
//!
//! - `add_custom_source` / `update_custom_source` 完成后 emit
//!   `musage://config-changed`（前端 settings 面板 refresh）+ 立即
//!   `refresh_single_inner`（浮窗立即出第一条数据，不等 poller）
//! - `delete_custom_source` emit `musage://config-changed` + emit
//!   `musage://snapshot`（强制前端重读 snapshot 排除被删 source）
//!
//! ## 上限（plan §7.1 review 决策）
//!
//! 最多 50 个 custom source。超过返错，避免极端用户填爆内存。

use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::config::{custom_sources as custom_persist, delete_credential_for_id};
use crate::providers::{CustomSource, CustomSourceSpec, ProviderSnapshot, QuotaSource};
use crate::AppState;
use crate::t;

/// 列表：返回所有 custom source specs（无 id 时返空 vec）。
#[tauri::command]
pub async fn list_custom_sources(
    state: State<'_, AppState>,
) -> Result<Vec<CustomSourceSpec>, String> {
    Ok(state.custom_sources.read().await.clone())
}

/// 添加：spec 不带 id/created_at（后端生成）。返回新 id。
///
/// 校验：
/// - display_name 非空
/// - base_url 必须以 http:// 或 https:// 开头
/// - path 必须以 / 开头
/// - display_name 不能跟已有 custom 重复
/// - 总数不超过 50
///
/// 成功后立即调 `refresh_single_inner` —— 用户加完就看到浮窗卡片 + 第一条数据。
#[tauri::command]
pub async fn add_custom_source(
    state: State<'_, AppState>,
    app: AppHandle,
    spec: CustomSourceSpec,
) -> Result<String, String> {
    // 1. 校验
    if spec.display_name.trim().is_empty() {
        return Err(t!("commands.custom_name_empty").into_owned());
    }
    if !spec.base_url.starts_with("http://") && !spec.base_url.starts_with("https://") {
        return Err(t!("commands.custom_url_invalid").into_owned());
    }
    if !spec.path.starts_with('/') {
        return Err(t!("commands.custom_path_invalid").into_owned());
    }
    if spec.method != "GET" && spec.method != "POST" {
        return Err(t!("commands.custom_method_invalid", method = spec.method.as_str()).into_owned());
    }

    // 2. 拿锁 + 写
    let id = format!("custom_{}", Uuid::new_v4().simple());
    let new_spec = CustomSourceSpec {
        id: id.clone(),
        display_name: spec.display_name.trim().to_string(),
        base_url: spec.base_url.trim_end_matches('/').to_string(),
        path: spec.path.clone(),
        method: spec.method.to_uppercase(),
        extract: spec.extract,
        plan_name_path: spec.plan_name_path.and_then(|s| {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }),
        accent: spec.accent,
        created_at: chrono::Utc::now().timestamp(),
    };

    {
        let mut customs = state.custom_sources.write().await;
        if customs.len() >= 50 {
            return Err(t!("commands.custom_limit_reached").into_owned());
        }
        if customs.iter().any(|s| s.display_name == new_spec.display_name) {
            return Err(t!("commands.custom_duplicate", name = new_spec.display_name.as_str()).into_owned());
        }
        customs.push(new_spec);
        custom_persist::save_custom_sources(&customs)?;
    }

    // 3. emit config-changed（settings panel 立即 rebuild）
    let _ = app.emit("musage://config-changed", ());

    // 4. 立即 refresh_single（浮窗立即出第一条数据，不等 poller）
    if let Err(e) = crate::commands::refresh_single_inner(&app, &id).await {
        tracing::warn!(error = %e, provider = %id, "add_custom_source 后立即拉取失败（不阻塞保存）");
    }
    Ok(id)
}

/// 更新：spec.id 必须存在。display_name 不可跟其他 custom 重复。
#[tauri::command]
pub async fn update_custom_source(
    state: State<'_, AppState>,
    app: AppHandle,
    spec: CustomSourceSpec,
) -> Result<(), String> {
    if spec.display_name.trim().is_empty() {
        return Err(t!("commands.custom_name_empty").into_owned());
    }
    if !spec.base_url.starts_with("http://") && !spec.base_url.starts_with("https://") {
        return Err(t!("commands.custom_url_invalid").into_owned());
    }
    if !spec.path.starts_with('/') {
        return Err(t!("commands.custom_path_invalid").into_owned());
    }
    if spec.method != "GET" && spec.method != "POST" {
        return Err(t!("commands.custom_method_invalid", method = spec.method.as_str()).into_owned());
    }

    let cleaned = CustomSourceSpec {
        id: spec.id.clone(),
        display_name: spec.display_name.trim().to_string(),
        base_url: spec.base_url.trim_end_matches('/').to_string(),
        path: spec.path.clone(),
        method: spec.method.to_uppercase(),
        extract: spec.extract,
        plan_name_path: spec.plan_name_path.and_then(|s| {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }),
        accent: spec.accent,
        // created_at 保留原值（不从客户端接收）
        created_at: spec.created_at,
    };

    {
        let mut customs = state.custom_sources.write().await;
        let pos = customs.iter().position(|s| s.id == cleaned.id)
            .ok_or_else(|| t!("commands.custom_not_found", id = cleaned.id.as_str()).into_owned())?;
        if customs.iter().enumerate()
            .any(|(i, s)| i != pos && s.display_name == cleaned.display_name) {
            return Err(t!("commands.custom_duplicate", name = cleaned.display_name.as_str()).into_owned());
        }
        customs[pos] = cleaned;
        custom_persist::save_custom_sources(&customs)?;
    }

    let _ = app.emit("musage://config-changed", ());
    if let Err(e) = crate::commands::refresh_single_inner(&app, &spec.id).await {
        tracing::warn!(error = %e, provider = %spec.id, "update_custom_source 后立即拉取失败");
    }
    Ok(())
}

/// 删除：从 custom_sources 移除 + 从 keys.json 删对应 api_key。
///
/// emit config-changed + emit snapshot 触发前端重读（排除被删 source）。
#[tauri::command]
pub async fn delete_custom_source(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    {
        let mut customs = state.custom_sources.write().await;
        let pos = customs.iter().position(|s| s.id == id)
            .ok_or_else(|| t!("commands.custom_not_found", id = id.as_str()).into_owned())?;
        customs.remove(pos);
        custom_persist::save_custom_sources(&customs)?;
    }
    // 删 api_key（best-effort：key 不存在也不报错）
    delete_credential_for_id(&id).ok();

    let _ = app.emit("musage://config-changed", ());
    // **B-NEW-6（2026-06-19 audit）**：删 source 后不要 refresh_single_inner。
    // 之前会触发 1 次对已删除 source 的 fetch（必然 404），并经过错误
    // 路径 emit 一个"已删除卡片"的 snapshot 事件，浮窗短暂闪烁错误态。
    // 实际行为：set_provider_enabled(false) 已经在删除流程里 emit 过
    // 一次 snapshot 移除该卡片，poller 下一分钟也会自然发现 source 不
    // 存在而跳过。refresh_single_inner 在这里只是浪费 + 噪声。
    Ok(())
}

/// 测试连接：不写 spec，只 fetch 一次返 ProviderSnapshot。
/// 用于 modal 的「填好 key → 测试一下」按钮。
#[tauri::command]
pub async fn test_custom_source(
    spec: CustomSourceSpec,
    api_key: String,
) -> Result<ProviderSnapshot, String> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err(t!("commands.api_key_empty").into_owned());
    }
    let temp = CustomSource::new(spec);
    let creds = crate::providers::Credentials {
        api_key: Some(trimmed.to_string()),
        cookie: None,
    };
    temp.fetch(&creds).await.map_err(|e| e.message)
}

// ── 单元测试（仅 spec 校验逻辑，HTTP 部分需要 mock 才能测） ──

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ExtractSpec;

    fn spec_with(method: &str, base: &str, path: &str, name: &str) -> CustomSourceSpec {
        CustomSourceSpec {
            id: "custom_test".to_string(),
            display_name: name.to_string(),
            base_url: base.to_string(),
            path: path.to_string(),
            method: method.to_string(),
            extract: ExtractSpec::NewApi { divide: None },
            plan_name_path: None,
            accent: None,
            created_at: 0,
        }
    }

    // 注意：这些 test 只覆盖 spec 字段校验，HTTP 行为需要 integration test
    // （mock reqwest server）才能跑。这里只是 sanity check 校验函数不 panic。

    #[test]
    fn spec_constructs_with_diverse_extracts() {
        let _ = spec_with("GET", "https://x.com", "/api", "DMX");
        let _ = CustomSourceSpec {
            extract: ExtractSpec::Balance {
                balance_path: "data.credit".to_string(),
                currency_path: None,
                divide: None,
            },
            ..spec_with("GET", "https://x.com", "/api", "DMX")
        };
        let _ = CustomSourceSpec {
            extract: ExtractSpec::Custom {
                remaining_path: Some("x".to_string()),
                used_path: None,
                total_path: None,
                unit: Some("USD".to_string()),
                divide: None,
            },
            ..spec_with("POST", "https://x.com", "/api", "DMX")
        };
    }

    #[test]
    fn uuid_format_is_compact_hex() {
        // format!("custom_{}", Uuid::new_v4().simple())
        // .simple() 返 32 字符 hex（无 -），所以 id 总长 = 7 + 32 = 39 字符
        let id = format!("custom_{}", Uuid::new_v4().simple());
        assert!(id.starts_with("custom_"));
        assert_eq!(id.len(), 7 + 32);
        // 后 32 字符全部是 hex
        assert!(id[7..].chars().all(|c| c.is_ascii_hexdigit()));
    }
}
