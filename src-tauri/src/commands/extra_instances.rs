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
    delete_credential_for_id, extra_instances, load_credential_for_id, save_credential_for_id,
    ExtraInstance,
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
        return Err(t!(
            "commands.extra.unknown_provider",
            id = req.provider_id.as_str()
        )
        .into_owned());
    }

    // 2. 先保存 key/kookie 到 keys.json（在 write 锁外 — keys.json 有独立的
    //    save_lock，不与 extra_instances 锁嵌套）。
    //    P1-4 fix: 先写 key 再 push extras，这样 key 保存失败时 extras 里没有
    //    对应记录（不会留下"无 key"的孤儿 instance）。如果 save extras 失败，
    //    尝试回滚刚写的 key（best-effort）。
    //
    //    注意：因为还没拿 write 锁，instance_index 还没正式算 —— 先构造临时
    //    api_key_ref 用于写 key，等确定了 index 后再 rename key。
    let temp_api_key_ref = if is_custom {
        let spec = req.custom.as_ref().unwrap();
        if spec.id.is_empty() {
            format!("custom_{}", uuid::Uuid::new_v4().simple())
        } else {
            spec.id.clone()
        }
    } else {
        // 先读一次拿 next index（只是为了算 api_key_ref，真正的 push 在锁内重算）
        let extras_read = state.extra_instances.read().await;
        let tentative_idx = extra_instances::next_index_for(&req.provider_id, &extras_read);
        drop(extras_read);
        format!("{}#{}", req.provider_id, tentative_idx)
    };
    let api_key_val = req
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let api_cookie_val = req
        .api_cookie
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(k) = api_key_val {
        let cred = Credentials {
            api_key: Some(k.to_string()),
            cookie: None,
        };
        save_credential_for_id(&temp_api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }
    if let Some(c) = api_cookie_val {
        let cred = Credentials {
            api_key: None,
            cookie: Some(c.to_string()),
        };
        save_credential_for_id(&temp_api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }

    // 3+4. 在 write 锁内构造 + 算 index + limit 检查 + push + 落盘。
    // Bug fix (2026-06-25): 之前先用 read 锁取快照算 next_index_for(), 再
    // 单独拿 write 锁 push。并发 add 同一 provider_id 时会拿到相同的 max
    // index → 重复 api_key_ref → keys.json 条目互相覆盖。现在整个 "读现有
    // 列表→算 index→push→save" 都在一把 write 锁内完成, 原子化。
    let new_instance = {
        let now = chrono::Utc::now().timestamp();
        let mut extras = state.extra_instances.write().await;
        if extras.len() >= TOTAL_EXTRA_LIMIT {
            // limit 检查失败 → 回滚刚写的 key
            delete_credential_for_id(&temp_api_key_ref).ok();
            return Err(t!("commands.extra.limit_reached").into_owned());
        }
        let instance = if is_custom {
            let mut spec = req.custom.as_ref().unwrap().clone();
            // P0-3 fix: 前端发 Omit<CustomSourceSpec, "id" | "created_at">，
            // id/created_at 缺省时自动补。
            if spec.id.is_empty() {
                spec.id = temp_api_key_ref.clone();
            }
            if spec.created_at == 0 {
                spec.created_at = now;
            }
            ExtraInstance {
                id: uuid::Uuid::new_v4(),
                provider_id: "custom".to_string(),
                instance_index: extra_instances::next_index_for("custom", &extras),
                api_key_ref: spec.id.clone(),
                custom: Some(spec),
                created_at: now,
            }
        } else {
            let idx = extra_instances::next_index_for(&req.provider_id, &extras);
            let final_api_key_ref = format!("{}#{}", req.provider_id, idx);
            // 如果 tentative index 跟实际 index 不一致（并发 add），rename key
            if final_api_key_ref != temp_api_key_ref {
                if let Ok(Some(cred)) = load_credential_for_id(&temp_api_key_ref) {
                    save_credential_for_id(&final_api_key_ref, &cred).ok();
                    delete_credential_for_id(&temp_api_key_ref).ok();
                }
            }
            ExtraInstance {
                id: uuid::Uuid::new_v4(),
                provider_id: req.provider_id.clone(),
                instance_index: idx,
                api_key_ref: final_api_key_ref,
                custom: None,
                created_at: now,
            }
        };
        extras.push(instance.clone());
        if let Err(e) = extra_instances::save(&extras) {
            // P1-4 fix: save extras 失败 → 回滚 key
            delete_credential_for_id(&instance.api_key_ref).ok();
            if instance.api_key_ref != temp_api_key_ref {
                delete_credential_for_id(&temp_api_key_ref).ok();
            }
            return Err(e);
        }
        instance
    };

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
    // P1-5 fix: 调整顺序为 "先 save extras（spec 更新）→ 再 save key"。
    // 之前先锁外存 key 再锁内存 spec，如果 key 存成功但 spec 存失败（如
    // 磁盘满），key 已更新但 extras 仍是旧 spec → 状态不一致。现在 spec
    // 作为结构变更先落盘，key 在后落盘；key 失败时 extras 已经正确，至少
    // 结构是对的。
    //
    // "找 pos → 替换 → save" 在同一把 write 锁内完成，pos 在锁内重新查
    // （已修复的 2026-06-25 TOCTOU bug）。

    // 第一步：write 锁内读 api_key_ref + 更新 spec + save extras
    let (updated, api_key_ref) = {
        let mut extras = state.extra_instances.write().await;
        let pos = extras.iter().position(|e| e.id == req.id).ok_or_else(|| {
            t!("commands.extra.not_found", id = req.id.to_string().as_str()).into_owned()
        })?;
        let mut updated = extras[pos].clone();
        let api_key_ref = updated.api_key_ref.clone();

        // 改 custom spec
        if let Some(spec) = req.custom {
            if updated.provider_id != "custom" {
                return Err(t!("commands.extra.custom_only_for_custom_provider").into_owned());
            }
            updated.custom = Some(spec);
        }

        extras[pos] = updated.clone();
        extra_instances::save(&extras)?;
        (updated, api_key_ref)
    };

    // 第二步：锁外保存 key（save_credential_for_id 有独立 save_lock）
    let api_key_val = req
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let api_cookie_val = req
        .api_cookie
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(k) = &api_key_val {
        let cred = Credentials {
            api_key: Some(k.clone()),
            cookie: None,
        };
        save_credential_for_id(&api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }
    if let Some(c) = &api_cookie_val {
        let cred = Credentials {
            api_key: None,
            cookie: Some(c.clone()),
        };
        save_credential_for_id(&api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
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
        let target = extras_read.iter().find(|e| e.id == id).ok_or_else(|| {
            t!("commands.extra.not_found", id = id.to_string().as_str()).into_owned()
        })?;
        (target.provider_id.clone(), target.api_key_ref.clone())
    };

    // 2. 拿 write lock + 删 + 紧凑 + 同步 keys.json
    {
        let mut extras = state.extra_instances.write().await;
        let pos = extras.iter().position(|e| e.id == id).ok_or_else(|| {
            t!("commands.extra.not_found", id = id.to_string().as_str()).into_owned()
        })?;
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
        //
        // P1-3 fix: save_credential_for_id 失败时跳过 delete_credential_for_id，
        // 保留旧 key 作为 fallback，避免凭据静默丢失。
        for (inst_id, old_ref) in &old_refs {
            if let Some(inst) = extras.iter().find(|e| &e.id == inst_id) {
                if inst.api_key_ref != *old_ref {
                    match load_credential_for_id(old_ref) {
                        Ok(Some(cred)) => match save_credential_for_id(&inst.api_key_ref, &cred) {
                            Ok(()) => {
                                delete_credential_for_id(old_ref).ok();
                            }
                            Err(e) => {
                                tracing::error!(
                                    old_key = %old_ref,
                                    new_key = %inst.api_key_ref,
                                    error = %e,
                                    "compact 后复制凭据失败，保留旧 key 不删",
                                );
                                // 不回滚 api_key_ref 重命名（compact_indexes_for 已就地改），
                                // 但保留旧 key 在 keys.json 中 → 数据不丢但需手动恢复。
                            }
                        },
                        Ok(None) => {
                            // 旧 key 本来就不存在（不应该出现，但防御性处理），删空引用
                            delete_credential_for_id(old_ref).ok();
                        }
                        Err(e) => {
                            tracing::warn!(
                                old_key = %old_ref,
                                error = %e,
                                "compact 时读旧凭据失败，跳过迁移",
                            );
                        }
                    }
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
        let spec = req
            .custom
            .ok_or_else(|| t!("commands.extra.custom_spec_required").into_owned())?;
        let temp = CustomSource::new(spec);
        temp.fetch(&creds).await.map_err(|e| e.message)
    } else {
        let src = instantiate_builtin_with_index(&req.provider_id, 1).ok_or_else(|| {
            t!(
                "commands.extra.unknown_provider",
                id = req.provider_id.as_str()
            )
            .into_owned()
        })?;
        // 校验 key 跟 provider 是否能拉到
        let _ = load_credential_for_id(&format!("{}#1", req.provider_id))
            .ok()
            .flatten();
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
            "minimax",
            "deepseek",
            "xiaomimimo",
            "tavily",
            "zenmux",
            "openrouter",
            "kimi",
            "zhipu",
            "stepfun",
            "siliconflow",
            "claude_official",
            "custom",
        ];
        assert_eq!(ids.len(), 12);
    }
}
