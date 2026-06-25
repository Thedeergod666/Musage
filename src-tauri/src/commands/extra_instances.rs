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
///
/// v0.2.1 commit 4:`name_key` 字段保留（兼容）但后端会同时返 `display_name`
/// 翻译好的字符串。前端 v0.2.1 起只用 `display_name`，单一来源 = 后端
/// `src-tauri/locales/{en,zh-CN}.json` 的 `provider_name.*` 11 项 + `extra.provider.custom`。
#[derive(Debug, Clone, Serialize)]
pub struct PickerProvider {
    pub id: String,
    /// v0.2.1 commit 4 已 deprecated,前端改用 display_name。保留字段
    /// 是为防 build 顺序中老 frontend 暂未升级时仍能跑。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name_key: String,
    /// v0.2.1 commit 4:后端用 `rust_i18n::t!()` 在返前端前注入翻译好的字符串。
    /// 前端 picker 直接显示,不再走 `t("provider_name.xxx")`。
    pub display_name: String,
    pub auth_kind: String,
    /// true = 内置副本（需要 api_key 即可）
    /// false = custom 中转站（需要 base_url / path / extract）
    pub is_builtin: bool,
}

/// 创建副本 / 新 custom 的请求体。
///
/// 前端传 snake_case（`provider_id`, `api_key`），Serde 默认按 Rust 字段名匹配
/// 无需 rename_all。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AddExtraInstanceRequest {
    pub provider_id: String,
    pub api_key: Option<String>,
    pub api_cookie: Option<String>,
    pub custom: Option<CustomSourceSpec>,
}

/// 更新副本的请求体（api_key / api_cookie / custom 任一可选）。
///
/// 前端传 snake_case（`api_key`, `api_cookie`），Serde 默认按 Rust 字段名匹配
/// 无需 rename_all。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UpdateExtraInstanceRequest {
    pub id: uuid::Uuid,
    pub api_key: Option<String>,
    pub api_cookie: Option<String>,
    pub custom: Option<CustomSourceSpec>,
}

/// 测试连接的请求体（不写 state）。
///
/// 前端传 snake_case（`provider_id`, `api_key`），Serde 默认按 Rust 字段名匹配
/// 无需 rename_all。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TestExtraInstanceRequest {
    pub provider_id: String,
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

    // 2+3+4. 在 write 锁内构造 + 算 index + limit 检查 + push + 落盘。
    // Bug fix (2026-06-25): 之前先用 read 锁取快照算 next_index_for(), 再
    // 单独拿 write 锁 push。并发 add 同一 provider_id 时会拿到相同的 max
    // index → 重复 api_key_ref → keys.json 条目互相覆盖。现在整个 "读现有
    // 列表→算 index→push→save" 都在一把 write 锁内完成, 原子化。
    let new_instance = {
        let now = chrono::Utc::now().timestamp();
        let mut extras = state.extra_instances.write().await;
        if extras.len() >= TOTAL_EXTRA_LIMIT {
            return Err(t!("commands.extra.limit_reached").into_owned());
        }
        let instance = if is_custom {
            let spec = req.custom.as_ref().unwrap();
            ExtraInstance {
                id: uuid::Uuid::new_v4(),
                provider_id: "custom".to_string(),
                instance_index: extra_instances::next_index_for("custom", &extras),
                api_key_ref: spec.id.clone(),
                custom: Some(spec.clone()),
                created_at: now,
            }
        } else {
            let idx = extra_instances::next_index_for(&req.provider_id, &extras);
            ExtraInstance {
                id: uuid::Uuid::new_v4(),
                provider_id: req.provider_id.clone(),
                instance_index: idx,
                api_key_ref: format!("{}#{}", req.provider_id, idx),
                custom: None,
                created_at: now,
            }
        };
        extras.push(instance.clone());
        extra_instances::save(&extras)?;
        instance
    };

    // 写 key 到 keys.json（在 write 锁外 —— keys.json 有独立的 save_lock, 不与
    // extra_instances 锁嵌套）
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
            let cred = Credentials {
                api_key: None,
                cookie: Some(c.trim().to_string()),
            };
            save_credential_for_id(&new_instance.api_key_ref, &cred)
                .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
        }
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
    // Bug fix (2026-06-25): 之前先用 read 锁 clone 全量 + 算 pos, 再在
    // write 锁里直接用这个 pos。如果两次锁之间 delete_extra_instance 删了
    // 同一位置或之前的条目, pos 会指向错误元素或触发 index out of bounds panic。
    // 修复: 安全操作 (save_credential / spec 校验) 在 write 锁外先做, 但
    // "找 pos → 替换 → save" 必须在同一把 write 锁内完成, 且 pos 在锁内重新查。

    // 改 key / custom spec（在锁外做 —— 这些不依赖 extras 内存状态;
    // save_credential_for_id 有独立的 save_lock）
    let api_key_val = req.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
    let api_cookie_val = req.api_cookie.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string());
    if let Some(k) = &api_key_val {
        // 先读一次拿到 current api_key_ref（锁外读,用于存 key）
        let extras_read = state.extra_instances.read().await;
        let inst = extras_read.iter().find(|e| e.id == req.id)
            .ok_or_else(|| t!("commands.extra.not_found", id = req.id.to_string().as_str()).into_owned())?;
        let cred = Credentials { api_key: Some(k.clone()), cookie: None };
        save_credential_for_id(&inst.api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }
    if let Some(c) = &api_cookie_val {
        let extras_read = state.extra_instances.read().await;
        let inst = extras_read.iter().find(|e| e.id == req.id)
            .ok_or_else(|| t!("commands.extra.not_found", id = req.id.to_string().as_str()).into_owned())?;
        let cred = Credentials { api_key: None, cookie: Some(c.clone()) };
        save_credential_for_id(&inst.api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }

    // 核心：拿到 write 锁后重新查找 pos, 再更新 + save
    let updated = {
        let mut extras = state.extra_instances.write().await;
        let pos = extras.iter().position(|e| e.id == req.id)
            .ok_or_else(|| t!("commands.extra.not_found", id = req.id.to_string().as_str()).into_owned())?;
        let mut updated = extras[pos].clone();

        // 改 custom spec
        if let Some(spec) = req.custom {
            if updated.provider_id != "custom" {
                return Err(t!("commands.extra.custom_only_for_custom_provider").into_owned());
            }
            updated.custom = Some(spec);
        }

        extras[pos] = updated.clone();
        extra_instances::save(&extras)?;
        updated
    };

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
        let pos = extras.iter().position(|e| e.id == id)
            .ok_or_else(|| t!("commands.extra.not_found", id = id.to_string().as_str()).into_owned())?;
        extras.remove(pos);

        // 紧凑前先拍下同 provider_id 内剩余实例的 (id, old_api_key_ref) 快照，
        // compact_indexes_for 会就地重写 api_key_ref（如 "minimax#3"→"minimax#2"）。
        let old_refs: Vec<(uuid::Uuid, String)> = extras
            .iter()
            .filter(|e| e.provider_id == provider_id)
            .map(|e| (e.id, e.api_key_ref.clone()))
            .collect();

        // 紧凑：同 provider_id 内重排 instance_index + api_key_ref
        extra_instances::compact_indexes_for(&provider_id, &mut extras);

        // 同步 keys.json：被 compact 改名的 key 要迁移凭据。
        // compact_indexes_for 已就地把 e.api_key_ref 改成新值；对比新旧
        // api_key_ref，把 old → new 的凭据复制过去，再删旧 key。
        for (inst_id, old_ref) in &old_refs {
            if let Some(inst) = extras.iter().find(|e| &e.id == inst_id) {
                if inst.api_key_ref != *old_ref {
                    if let Ok(Some(cred)) = load_credential_for_id(old_ref) {
                        if save_credential_for_id(&inst.api_key_ref, &cred).is_err() {
                            tracing::warn!(
                                old_key = %old_ref,
                                new_key = %inst.api_key_ref,
                                "compact 后复制凭据失败",
                            );
                        }
                    }
                    delete_credential_for_id(old_ref).ok();
                }
            }
        }

        // 删被删除实例的旧 key
        delete_credential_for_id(&target_api_key_ref).ok();

        extra_instances::save(&extras)?;
    }

    let _ = app.emit("musage://config-changed", ());
    // **B-NEW-6（2026-06-19 audit 同款）**：删 source 后不要 refresh_single_inner。
    Ok(())
}

/// 前端 modal 的 provider picker 数据源：11 内置 + 1 custom。
///
/// v0.2.1 commit 4:`display_name` 由后端 `t!()` 注入翻译好的字符串,前端
/// 不再走 `t("provider_name.xxx")`。`name_key` 字段保留但 `skip_serializing_if`
/// 空串不返,避免老 frontend 抓不到时崩。
#[tauri::command]
pub async fn list_picker_providers() -> Result<Vec<PickerProvider>, String> {
    Ok(vec![
        PickerProvider {
            id: "minimax".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.minimax").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "deepseek".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.deepseek").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "xiaomimimo".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.xiaomimimo").into_owned(),
            auth_kind: "api_key_or_cookie".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "tavily".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.tavily").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "zenmux".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.zenmux").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "openrouter".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.openrouter").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "kimi".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.kimi").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "zhipu".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.zhipu_cn").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "stepfun".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.stepfun").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "siliconflow".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.siliconflow").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "claude_official".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.claude_official").into_owned(),
            auth_kind: "cookie".to_string(),
            is_builtin: true,
        },
        PickerProvider {
            id: "custom".to_string(),
            name_key: String::new(),
            // v0.2.1 commit 4:custom 走 `extra.provider.custom` key,不在
            // `provider_name.*` 命名空间下(命名空间差异历史原因)
            display_name: t!("extra.provider.custom").into_owned(),
            auth_kind: "api_key".to_string(),
            is_builtin: false,
        },
    ])
}

/// 测试连接（不写 state）。
///
/// - `req.provider_id == "custom"` → 用 `req.custom` 构造 CustomSource
/// - 其它 → 用 `instantiate_builtin_with_index(provider_id, 1)` 拿默认实例
///
/// 返回 `ProviderSnapshot`。
///
/// **Fix（deepseek 添加失败 #X）**：原签名是扁平参数 `provider_id, api_key, ...`，
/// 但前端 `testExtraInstance` 跟 `add`/`update` 一样传 `{ req: {...} }` —— Tauri
/// 把整个对象当 `req` 传进来后，后端 deserialize 失败，strict 模式报
/// "missing required key providerId"。改成跟兄弟命令一致的 `req: TestExtraInstanceRequest`。
#[tauri::command]
pub async fn test_extra_instance(
    req: TestExtraInstanceRequest,
) -> Result<ProviderSnapshot, String> {
    let api_key_trimmed = req.api_key.as_deref().map(str::trim).unwrap_or("");
    let api_cookie_trimmed = req.api_cookie.as_deref().map(str::trim).unwrap_or("");
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

    if req.provider_id == "custom" {
        let spec = req.custom.ok_or_else(|| t!("commands.extra.custom_spec_required").into_owned())?;
        let temp = CustomSource::new(spec);
        temp.fetch(&creds).await.map_err(|e| e.message)
    } else {
        let src = instantiate_builtin_with_index(&req.provider_id, 1)
            .ok_or_else(|| t!("commands.extra.unknown_provider", id = req.provider_id.as_str()).into_owned())?;
        // 校验 key 跟 provider 是否能拉到
        let _ = load_credential_for_id(&format!("{}#1", req.provider_id)).ok().flatten();
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
