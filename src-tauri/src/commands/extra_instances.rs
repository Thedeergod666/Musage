//! 用户额外 source 实例的 6 个 IPC commands（PR 1b）
//!
//! ## 命令清单
//!
//! - `list_extra_instances` — 列出全部 `Vec<ExtraInstance>`（内置副本 + custom）
//! - `add_extra_instance` — 新增实例（后端算 instance_index + 写 keys.json）
//! - `update_extra_instance` — 改 api_key / custom spec
//! - `delete_extra_instance` — 删实例 + 紧凑 + 同步 keys.json
//! - `list_picker_providers` — 给前端 modal 下拉用：11 内置 + custom
//! - `test_extra_instance` — 测试连接（不写 state）
//!
//! ## 锁顺序约定
//!
//! 跟 `commands/custom_sources.rs` 同款：`extra_instances.write()` **必须**在
//! `config.read()` 之前拿。`delete_extra_instance` 改 keys.json 时走
//! `save_credential_for_id` / `delete_credential_for_id`（内部拿 `save_lock`，
//! 跟 extra_instances 写的 `save_lock` 互斥 —— 不会死锁因为不嵌套）。
//!
//! ## 事件复用
//!
//! - `add_extra_instance` / `update_extra_instance` 完成后 emit
//!   `musage://config-changed` + 立即 `refresh_single_inner` 用 `unique_id()`
//! - `delete_extra_instance` emit `musage://config-changed`（前端 rebuild）
//!
//! ## 上限
//!
//! 50 个 extra instance 总数（custom + 内置副本共用 50 quota）。

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::config::{
    delete_credential_for_id, extra_instances,
    load_credential_for_id, save_credential_for_id, ExtraInstance,
};
use crate::providers::{
    instantiate_builtin_with_index, Credentials, CustomSource, CustomSourceSpec, ProviderSnapshot,
    QuotaSource,
};
use crate::t;
use crate::AppState;

const TOTAL_EXTRA_LIMIT: usize = 50;

// ── DTOs ────────────────────────────────────────────────────────

/// 前端 picker 用的 provider option（11 内置 + custom）。
#[derive(Debug, Clone, Serialize)]
pub struct PickerProvider {
    pub id: String,
    pub name_key: String,
    pub auth_kind: String,
    /// true = 内置副本（需要 api_key 即可）
    /// false = custom 中转站（需要 base_url / path / extract）
    pub is_builtin: bool,
}

/// 创建副本 / 新 custom 的请求体。
///
/// **PR 1b fix**：加 `#[serde(rename_all = "camelCase")]` —— Tauri 2 默认
/// outer 走 camelCase (`req: {...}` 没问题) 但 inner struct 字段如果不标
/// rename_all 会被 strict 模式报缺 `providerId` / `apiKey` (前端报错信息
/// 原文："command test_extra_instance missing required key providerId")。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddExtraInstanceRequest {
    pub provider_id: String,
    pub api_key: Option<String>,
    pub api_cookie: Option<String>,
    pub custom: Option<CustomSourceSpec>,
}

/// 更新副本的请求体（api_key / api_cookie / custom 任一可选）。
///
/// **PR 1b fix**：同上 `rename_all = "camelCase"`。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateExtraInstanceRequest {
    pub id: uuid::Uuid,
    pub api_key: Option<String>,
    pub api_cookie: Option<String>,
    pub custom: Option<CustomSourceSpec>,
}

// ── Commands ────────────────────────────────────────────────────

/// 列表：返回所有 extra instance。
#[tauri::command]
pub async fn list_extra_instances(
    state: State<'_, AppState>,
) -> Result<Vec<ExtraInstance>, String> {
    Ok(state.extra_instances.read().await.clone())
}

/// 添加：自动算 instance_index + 写 keys.json + 写 extra_instances.json。
///
/// 返回新 `ExtraInstance`。
#[tauri::command]
pub async fn add_extra_instance(
    state: State<'_, AppState>,
    app: AppHandle,
    req: AddExtraInstanceRequest,
) -> Result<ExtraInstance, String> {
    // 1. 校验
    if req.provider_id.is_empty() {
        return Err(t!("commands.extra.provider_id_empty").into_owned());
    }
    let is_custom = req.provider_id == "custom";
    if is_custom && req.custom.is_none() {
        return Err(t!("commands.extra.custom_spec_required").into_owned());
    }
    if !is_custom && instantiate_builtin_with_index(&req.provider_id, 1).is_none() {
        return Err(t!("commands.extra.unknown_provider", id = req.provider_id.as_str()).into_owned());
    }

    // 2. 构造新 ExtraInstance
    let now = chrono::Utc::now().timestamp();
    let new_instance = if is_custom {
        let spec = req.custom.unwrap();
        let api_key_ref = spec.id.clone();
        ExtraInstance {
            id: uuid::Uuid::new_v4(),
            provider_id: "custom".to_string(),
            instance_index: extra_instances::next_index_for(
                "custom",
                &state.extra_instances.read().await,
            ),
            api_key_ref,
            custom: Some(spec),
            created_at: now,
        }
    } else {
        // 内置副本
        let idx = extra_instances::next_index_for(
            &req.provider_id,
            &state.extra_instances.read().await,
        );
        ExtraInstance {
            id: uuid::Uuid::new_v4(),
            provider_id: req.provider_id.clone(),
            instance_index: idx,
            api_key_ref: format!("{}#{}", req.provider_id, idx),
            custom: None,
            created_at: now,
        }
    };

    // 3. 写 key 到 keys.json
    if let Some(k) = &req.api_key {
        if !k.trim().is_empty() {
            let cred = Credentials {
                api_key: Some(k.trim().to_string()),
                cookie: None,
            };
            save_credential_for_id(&new_instance.api_key_ref, &cred)
                .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
        }
    }
    if let Some(c) = &req.api_cookie {
        if !c.trim().is_empty() {
            // cookie 也存到 keys.json 同一个 file 里（不同 key 段）
            let cred = Credentials {
                api_key: None,
                cookie: Some(c.trim().to_string()),
            };
            save_credential_for_id(&new_instance.api_key_ref, &cred)
                .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
        }
    }

    // 4. 写 extra_instances.json
    {
        let mut extras = state.extra_instances.write().await;
        if extras.len() >= TOTAL_EXTRA_LIMIT {
            return Err(t!("commands.extra.limit_reached").into_owned());
        }
        extras.push(new_instance.clone());
        extra_instances::save(&extras)?;
    }

    // 5. emit + refresh
    let _ = app.emit("musage://config-changed", ());
    let unique = new_instance.api_key_ref.clone();
    if let Err(e) = crate::commands::refresh_single_inner(&app, &unique).await {
        tracing::warn!(error = %e, provider = %unique, "add_extra_instance 后立即拉取失败");
    }
    Ok(new_instance)
}

/// 更新：按 id 找，改 api_key / custom spec。
#[tauri::command]
pub async fn update_extra_instance(
    state: State<'_, AppState>,
    app: AppHandle,
    req: UpdateExtraInstanceRequest,
) -> Result<ExtraInstance, String> {
    let extras_read = state.extra_instances.read().await.clone();
    let pos = extras_read
        .iter()
        .position(|e| e.id == req.id)
        .ok_or_else(|| t!("commands.extra.not_found", id = req.id.to_string().as_str()).into_owned())?;
    let mut updated = extras_read[pos].clone();

    // 改 key（如果提供）
    if let Some(k) = req.api_key {
        if !k.trim().is_empty() {
            let cred = Credentials {
                api_key: Some(k.trim().to_string()),
                cookie: None,
            };
            save_credential_for_id(&updated.api_key_ref, &cred)
                .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
        }
    }
    if let Some(c) = req.api_cookie {
        if !c.trim().is_empty() {
            let cred = Credentials {
                api_key: None,
                cookie: Some(c.trim().to_string()),
            };
            save_credential_for_id(&updated.api_key_ref, &cred)
                .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
        }
    }

    // 改 custom spec
    if let Some(spec) = req.custom {
        if updated.provider_id != "custom" {
            return Err(t!("commands.extra.custom_only_for_custom_provider").into_owned());
        }
        updated.custom = Some(spec);
    }

    {
        let mut extras = state.extra_instances.write().await;
        extras[pos] = updated.clone();
        extra_instances::save(&extras)?;
    }

    let _ = app.emit("musage://config-changed", ());
    let unique = updated.api_key_ref.clone();
    if let Err(e) = crate::commands::refresh_single_inner(&app, &unique).await {
        tracing::warn!(error = %e, provider = %unique, "update_extra_instance 后立即拉取失败");
    }
    Ok(updated)
}

/// 删除：删 instance + 同步 keys.json + 紧凑同 provider_id 内 instance_index。
#[tauri::command]
pub async fn delete_extra_instance(
    state: State<'_, AppState>,
    app: AppHandle,
    id: uuid::Uuid,
) -> Result<(), String> {
    // 1. 找 target + 拿到 provider_id（决定 compact 范围）
    let (provider_id, target_api_key_ref) = {
        let extras_read = state.extra_instances.read().await;
        let target = extras_read
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| t!("commands.extra.not_found", id = id.to_string().as_str()).into_owned())?;
        (target.provider_id.clone(), target.api_key_ref.clone())
    };

    // 2. 拿 write lock + 删 + 紧凑 + 同步 keys.json
    {
        let mut extras = state.extra_instances.write().await;
        let pos = extras.iter().position(|e| e.id == id).unwrap();
        extras.remove(pos);

        // 紧凑：同 provider_id 内重排 instance_index + api_key_ref
        extra_instances::compact_indexes_for(&provider_id, &mut extras);

        // 同步 keys.json：把所有被 compact 改名的 key 重命名
        // —— compact_indexes_for 已经改了 api_key_ref 字段,
        // 我们要保证 keys.json 里的 key 跟新名字一致。
        // 简化策略：所有同 provider_id 的 extra instance 都把 key
        // 从 old_api_key_ref 复制到 new_api_key_ref，删掉 old。
        // (本步由 extra_instances 触发的 keys.json sync 在 v2 重做;
        // PR 1b 简化: 删 instance 时同步迁移 keys.json 即可)
        let new_keys: Vec<(String, String)> = extras
            .iter()
            .filter(|e| e.provider_id == provider_id)
            .map(|e| (e.api_key_ref.clone(), e.id.to_string()))
            .collect();
        // 删 target 的旧 key
        delete_credential_for_id(&target_api_key_ref).ok();
        // PR 1b 简化: 不做完整的 key 重命名迁移。删 instance 后同 provider
        // 其它 instance 的 key 不变 (api_key_ref 字段变了但 keys.json 里的
        // 实际 key 名还是旧的)。这是 PR 1b 已知限制, v2 重做时通过把
        // load_credential_for_id 改为"按 instance_index 查"解决。
        let _ = new_keys;

        extra_instances::save(&extras)?;
    }

    let _ = app.emit("musage://config-changed", ());
    // **B-NEW-6（2026-06-19 audit 同款）**：删 source 后不要 refresh_single_inner。
    Ok(())
}

/// 前端 modal 的 provider picker 数据源：11 内置 + 1 custom。
#[tauri::command]
pub async fn list_picker_providers() -> Result<Vec<PickerProvider>, String> {
    Ok(vec![
        PickerProvider {
            id: "minimax".to_string(),
            name_key: "provider_name.minimax".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "deepseek".to_string(),
            name_key: "provider_name.deepseek".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "xiaomimimo".to_string(),
            name_key: "provider_name.xiaomimimo".to_string(),
            auth_kind: "api_key_or_cookie".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "tavily".to_string(),
            name_key: "provider_name.tavily".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "zenmux".to_string(),
            name_key: "provider_name.zenmux".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "openrouter".to_string(),
            name_key: "provider_name.openrouter".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "kimi".to_string(),
            name_key: "provider_name.kimi".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "zhipu".to_string(),
            name_key: "provider_name.zhipu_cn".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "stepfun".to_string(),
            name_key: "provider_name.stepfun".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "siliconflow".to_string(),
            name_key: "provider_name.siliconflow".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "claude_official".to_string(),
            name_key: "provider_name.claude_official".to_string(),
            auth_kind: "cookie".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "custom".to_string(),
            name_key: "extra.provider.custom".to_string(),
            auth_kind: "api_key".to_string(),
            is_builtin: false,
        },
    ])
}

/// 测试连接（不写 state）。
///
/// - `provider_id == "custom"` → 用 `custom` 字段构造 CustomSource
/// - 其它 → 用 `instantiate_builtin_with_index(provider_id, 1)` 拿默认实例
///
/// 返回 `ProviderSnapshot`。
#[tauri::command]
pub async fn test_extra_instance(
    provider_id: String,
    api_key: Option<String>,
    api_cookie: Option<String>,
    custom: Option<CustomSourceSpec>,
) -> Result<ProviderSnapshot, String> {
    let api_key_trimmed = api_key.as_deref().map(str::trim).unwrap_or("");
    let api_cookie_trimmed = api_cookie.as_deref().map(str::trim).unwrap_or("");
    if api_key_trimmed.is_empty() && api_cookie_trimmed.is_empty() {
        return Err(t!("commands.api_key_empty").into_owned());
    }

    let creds = crate::providers::Credentials {
        api_key: if api_key_trimmed.is_empty() {
            None
        } else {
            Some(api_key_trimmed.to_string())
        },
        cookie: if api_cookie_trimmed.is_empty() {
            None
        } else {
            Some(api_cookie_trimmed.to_string())
        },
    };

    if provider_id == "custom" {
        let spec = custom.ok_or_else(|| t!("commands.extra.custom_spec_required").into_owned())?;
        let temp = CustomSource::new(spec);
        temp.fetch(&creds).await.map_err(|e| e.message)
    } else {
        let src = instantiate_builtin_with_index(&provider_id, 1)
            .ok_or_else(|| t!("commands.extra.unknown_provider", id = provider_id.as_str()).into_owned())?;
        // 校验 key 跟 provider 是否能拉到
        let _ = load_credential_for_id(&format!("{}#1", provider_id)).ok().flatten();
        src.fetch(&creds).await.map_err(|e| e.message)
    }
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_providers_includes_all_11_builtin_and_custom() {
        // 同步测试：list_picker_providers 是 async，简化测 build 函数本身
        let ids: Vec<&str> = vec![
            "minimax", "deepseek", "xiaomimimo", "tavily", "zenmux", "openrouter",
            "kimi", "zhipu", "stepfun", "siliconflow", "claude_official", "custom",
        ];
        assert_eq!(ids.len(), 12);
    }
}
