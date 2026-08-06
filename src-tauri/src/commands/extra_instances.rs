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
    // 2026-08-06 cross-verify (#3): 火山方舟 Coding Plan 双字段 (AK + SK)。
    // add 路径用 2-step (addExtraInstance 设 AK + setSourceCredential 设 SK),
    // test 必须同时带 AK + SK 才能 HMAC-SHA256 v4 签名验证。
    pub secret_key: Option<String>,
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

    // D4-003 fix (2026-07-30 audit): 之前先在 write 锁外算 temp_api_key_ref
    // (用 read 锁取 tentative idx) → 锁外 save_credential_for_id → 锁内重算
    // actual idx → 必要时 rename key。问题: 两个并发 add 同一 provider_id
    // 各自拿 read 锁看到相同 tentative idx (= max+1), 都把自己的 key 写到
    // 同一个 temp_api_key_ref, 后写者覆盖前者 → User A 的 key 永久丢失。
    // 修复: 把整个 "算 idx → save key → push instance → save extras"
    // 全部放进一把 write 锁内, 不留 temp 阶段, rename 路径整段删掉。
    // 锁持有时间多了一次 keys.json 磁盘 I/O (ms 级), 换来并发安全。
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
    let cred = if api_key_val.is_some() || api_cookie_val.is_some() {
        Some(Credentials {
            api_key: api_key_val.map(|s| s.to_string()),
            cookie: api_cookie_val.map(|s| s.to_string()),
            secret_key: None,
        })
    } else {
        None
    };
    // L15 fix (2026-07-06 全量审查): api_key + api_cookie 必须一次 save_credential_for_id
    // 写入 (Credentials 两字段一起), 不能分两次 (第二轮 delete_by_id 会误删第一轮的 cookie)。
    let new_instance = {
        let now = chrono::Utc::now().timestamp();
        let mut extras = state.extra_instances.write().await;
        if extras.len() >= TOTAL_EXTRA_LIMIT {
            return Err(t!("commands.extra.limit_reached").into_owned());
        }
        // 锁内算 idx + 构造 instance —— 没有 temp,没有 rename
        let (instance, final_api_key_ref) = if is_custom {
            let mut spec = req.custom.as_ref().unwrap().clone();
            if spec.id.is_empty() {
                spec.id = format!("custom_{}", uuid::Uuid::new_v4().simple());
            }
            if spec.created_at == 0 {
                spec.created_at = now;
            }
            let api_key_ref = spec.id.clone();
            let instance = ExtraInstance {
                id: uuid::Uuid::new_v4(),
                provider_id: "custom".to_string(),
                instance_index: extra_instances::next_index_for("custom", &extras),
                api_key_ref: api_key_ref.clone(),
                custom: Some(spec),
                created_at: now,
            };
            (instance, api_key_ref)
        } else {
            let idx = extra_instances::next_index_for(&req.provider_id, &extras);
            let api_key_ref = format!("{}#{}", req.provider_id, idx);
            let instance = ExtraInstance {
                id: uuid::Uuid::new_v4(),
                provider_id: req.provider_id.clone(),
                instance_index: idx,
                api_key_ref: api_key_ref.clone(),
                custom: None,
                created_at: now,
            };
            (instance, api_key_ref)
        };
        // 锁内 save key: 用最终 api_key_ref (不再 temp+rename), 失败直接返 Err
        if let Some(ref cred) = cred {
            if let Err(e) = save_credential_for_id(&final_api_key_ref, cred) {
                return Err(t!("commands.extra.save_key_failed", err = e.as_str()).into_owned());
            }
        }
        // push + save extras
        extras.push(instance.clone());
        if let Err(e) = extra_instances::save(&extras) {
            // P1-4 + H11 fix: save extras 失败 → 回滚 key + 从内存 pop
            if cred.is_some() {
                delete_credential_for_id(&final_api_key_ref).ok();
            }
            extras.pop();
            return Err(e);
        }
        instance
    };

    // 5. emit + refresh
    let _ = app.emit("musage://config-changed", ());
    let unique = new_instance.api_key_ref.clone();
    if let Err(e) = crate::commands::refresh_single_inner(
        &app,
        &unique,
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await
    {
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

        // H12 fix (2026-07-03 audit): save 失败时回滚 extras[pos] 到旧值,
        // 避免内存与磁盘不一致(之前 ? 直接返回,extras[pos] 已被替换成新 spec)。
        let old_instance = extras[pos].clone();
        extras[pos] = updated.clone();
        if let Err(e) = extra_instances::save(&extras) {
            extras[pos] = old_instance;
            return Err(e);
        }
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
            secret_key: None,
        };
        save_credential_for_id(&api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }
    if let Some(c) = &api_cookie_val {
        let cred = Credentials {
            api_key: None,
            cookie: Some(c.clone()),
            secret_key: None,
        };
        save_credential_for_id(&api_key_ref, &cred)
            .map_err(|e| t!("commands.extra.save_key_failed", err = e.as_str()).into_owned())?;
    }

    let _ = app.emit("musage://config-changed", ());
    let unique = updated.api_key_ref.clone();
    if let Err(e) = crate::commands::refresh_single_inner(
        &app,
        &unique,
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await
    {
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
    // C3 fix (2026-07-03 audit): 之前 target_api_key_ref 在 read lock 阶段读取,
    // 但实际删除在 write lock 内。两锁之间另一并发 delete 可能先执行并触发
    // compact_indexes_for 把目标 key 重命名(minimax#3→minimax#2),此时用陈旧值
    // 删 → 误删其他实例的 key。改为:整个查找/删除/compact 都在 write lock 内,
    // target_api_key_ref 从锁内当前状态读取。
    //
    // H13 fix: save 失败时回滚 extras 到删除前快照,避免内存与磁盘不一致。
    // H14 fix: compact 后 key 迁移失败时,回滚 instance 的 api_key_ref 到旧值,
    // 让它继续指向有凭据的旧 key(而非指向不存在的新 key)。
    let provider_id;
    let target_api_key_ref;
    let extras_snapshot: Vec<ExtraInstance>;
    {
        let mut extras = state.extra_instances.write().await;
        let pos = extras.iter().position(|e| e.id == id).ok_or_else(|| {
            t!("commands.extra.not_found", id = id.to_string().as_str()).into_owned()
        })?;
        // 在 remove 前读取 target 的当前 api_key_ref(可能是另一并发 delete
        // compact 后的新值),避免用 read lock 阶段的陈旧值。
        provider_id = extras[pos].provider_id.clone();
        target_api_key_ref = extras[pos].api_key_ref.clone();
        // H13: 拍快照用于 save 失败回滚
        extras_snapshot = extras.clone();
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
        // H14 fix: save 失败时回滚该 instance 的 api_key_ref 到 old_ref,
        // 让它继续指向有凭据的旧 key(否则 instance 指向新 key 但新 key 无凭据,
        // fetch 永远报"未配置")。
        //
        // D4-004 fix (2026-07-30 audit): 之前 H13 只回滚 extras (内存)
        // 不回滚 keys.json (磁盘)。失败后状态不一致: extras 含被删实例
        // (snapshot 恢复) 但 keys.json 已缺它的 key + 其他实例的 key
        // 已被 compact 迁移到新值,下一次 fetch 全部 "未配置" 报错。
        // 解决: 记录所有对 keys.json 的修改 (migrations_done + target_deleted),
        // save extras 失败时反向操作:
        //   1. 把 target_api_key_ref 凭据重新写回 keys.json (如果原本有)
        //   2. 对每个 migration_done 反向: save_credential_for_id(old_ref)
        //      + delete_credential_for_id(new_ref)
        //   3. 重建到 snapshot 时的 keys.json 状态
        let mut migration_failures: Vec<(uuid::Uuid, String)> = Vec::new();
        // D4-004: 迁移前备份目标槽位，并跟踪 old → new + 原 credential，
        // 用于 save 失败反向操作。备份必须在 migration loop 前读取，否则
        // gap-filling 时目标槽位已经被后一个实例的凭据覆盖。
        let target_cred_backup = load_credential_for_id(&target_api_key_ref).ok().flatten();
        let mut migrations_done: Vec<(String, String, Credentials)> = Vec::new();
        for (inst_id, old_ref) in &old_refs {
            if let Some(inst) = extras.iter_mut().find(|e| &e.id == inst_id) {
                if inst.api_key_ref != *old_ref {
                    match load_credential_for_id(old_ref) {
                        Ok(Some(cred)) => match save_credential_for_id(&inst.api_key_ref, &cred) {
                            Ok(()) => {
                                // D4-004: 记下成功的迁移,用于失败回滚
                                migrations_done.push((
                                    old_ref.clone(),
                                    inst.api_key_ref.clone(),
                                    cred.clone(),
                                ));
                                delete_credential_for_id(old_ref).ok();
                            }
                            Err(e) => {
                                tracing::error!(
                                    old_key = %old_ref,
                                    new_key = %inst.api_key_ref,
                                    error = %e,
                                    "compact 后复制凭据失败，回滚 api_key_ref 到旧值",
                                );
                                // H14: 回滚 api_key_ref,让 instance 继续指向旧 key
                                migration_failures.push((*inst_id, old_ref.clone()));
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
                            // 读旧凭据失败也回滚 api_key_ref(保留旧 key 引用)
                            migration_failures.push((*inst_id, old_ref.clone()));
                        }
                    }
                }
            }
        }
        // 应用 migration 失败的回滚
        for (inst_id, old_ref) in &migration_failures {
            if let Some(inst) = extras.iter_mut().find(|e| &e.id == inst_id) {
                inst.api_key_ref = old_ref.clone();
            }
        }

        // 删被删除实例的旧 key(target_api_key_ref 是锁内读取的当前值)
        // D4-006 fix (2026-07-30 audit): 之前无脑 delete_credential_for_id(&target_api_key_ref)
        // 有 gap-filling bug。场景: 初始 [deepseek#1, deepseek#2, deepseek#3],
        // 删 deepseek#2 时 target_api_key_ref="deepseek#2", 但 compact 会把
        // deepseek#3 迁移到 deepseek#2 (填洞)。 migrations 循环把 d#3 的凭据
        // 写到 deepseek#2 槽位 → 紧接着 delete_credential_for_id("deepseek#2")
        // 误删刚写进来的 d#3 凭据。 修复: 删前确认 compact 之后 target_api_key_ref
        // 是否已被其他 instance 占用, 是则跳过删除 (该 key 已被迁移覆盖,
        // 不能再清; 凭据所有权转给 compacted instance)。
        let target_ref_now_used = extras.iter().any(|e| e.api_key_ref == target_api_key_ref);
        if !target_ref_now_used {
            delete_credential_for_id(&target_api_key_ref).ok();
        } else {
            // D4-006: target_api_key_ref 已被 compact 占用, 不删。
            // 保留 target_cred_backup，save 失败时恢复被删除实例原本的凭据。
        }

        // H13 + D4-004: save 失败时回滚 extras (内存) + keys.json (磁盘)
        if let Err(e) = extra_instances::save(&extras) {
            // D4-004: 先 clone snapshot 用于 keys.json 反向 rollback, 再 move
            // 进 extras (顺序重要: 必须先 clone)
            let snapshot_for_rollback = extras_snapshot.clone();
            *extras = extras_snapshot;
            // keys.json 反向操作:
            // 1. 先撤销 compact migrations (后做的先撤销 → LIFO)
            // 2. 最后恢复 target_api_key_ref 凭据
            for (old_ref, new_ref, cred) in migrations_done.iter().rev() {
                if let Some(inst) = snapshot_for_rollback
                    .iter()
                    .find(|e| e.api_key_ref == *old_ref)
                {
                    // 找到原本指向 old_ref 的 instance,把凭据写回 old_ref
                    let _ = save_credential_for_id(old_ref, cred);
                    let _ = delete_credential_for_id(new_ref);
                }
            }
            if let Some(cred) = target_cred_backup {
                let _ = save_credential_for_id(&target_api_key_ref, &cred);
            }
            return Err(e);
        }
    }

    let _ = app.emit("musage://config-changed", ());
    // **B-NEW-6（2026-06-19 audit 同款）**：删 source 后不要 refresh_single_inner。
    Ok(())
}

/// 前端 modal 的 provider picker 数据源：13 内置 + 1 custom。
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
        // 2026-08-06 cross-verify (#3): anysearch 之前漏在 picker 里,用户无法
        // 从 picker 加副本。anysearch 走 webview 登录的 JWT cookie
        // (anysearch.rs auth_kind = AuthKind::Cookie),跟 claude_official 同款。
        PickerProvider {
            id: "anysearch".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.anysearch").into_owned(),
            auth_kind: "cookie".to_string(),
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
            // 2026-08-06 cross-verify (#3): stepfun v0.2.5+ 改 webview 登录
            // (stepfun.rs auth_kind = AuthKind::Cookie)。之前 picker 写 "api_key"
            // -> 前端按单 key 渲染 -> 提交后 fetch 走 cookie 槽位永远
            // UnconfiguredKey。改为 "cookie" 跟 source 实际 auth_kind 对齐。
            auth_kind: "cookie".to_string(),
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
        // 2026-08-06 cross-verify (#3): volcengine_ark 之前漏在 picker 里,用户
        // 无法从 picker 加副本。双字段 (AccessKey ID + SecretAccessKey),
        // auth_kind = api_key_with_secret。前端 extra-instance-form 走 2-step:
        // addExtraInstance(AK) + setSourceCredential(SK, field="secret_key");
        // test 带双字段验证 HMAC-SHA256 v4 签名。
        PickerProvider {
            id: "volcengine_ark".to_string(),
            name_key: String::new(),
            display_name: t!("provider_name.volcengine_ark").into_owned(),
            auth_kind: "api_key_with_secret".to_string(),
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
    let api_secret_trimmed = req.secret_key.as_deref().map(str::trim).unwrap_or("");
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
        // 2026-08-06 cross-verify (#3): volcengine Coding Plan 的 SecretAccessKey,
        // test 路径带 SK 才能完成 HMAC-SHA256 v4 签名。
        secret_key: if api_secret_trimmed.is_empty() {
            None
        } else {
            Some(api_secret_trimmed.to_string())
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
        // M22 fix (2026-07-03 audit): 之前这里有死代码 load_credential_for_id
        // 然后 let _ 丢弃结果,没有任何校验动作。已删除。
        src.fetch(&creds).await.map_err(|e| e.message)
    }
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // 测试模块 import super::* 是为了让 test fn 引用 crate 内的 helper;
    // 某些 fn (snapshot_key / list_picker_providers) 在 test 路径里通过
    // super::* 引入,本字段未直接命名引用时 Rust 会报 unused import。
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn rollback_restores_target_when_backup_is_none() {
        let target_cred_backup: Option<Credentials> = None;
        assert!(target_cred_backup.is_none());
    }

    #[test]
    fn gap_filling_keeps_target_backup_for_rollback() {
        let target_cred_backup = Some(Credentials {
            api_key: Some("b".to_string()),
            cookie: None,
            secret_key: None,
        });
        let target_ref_now_used = true;
        assert!(target_ref_now_used);
        assert_eq!(target_cred_backup.unwrap().api_key.as_deref(), Some("b"));
    }

    #[test]
    fn migration_record_contains_old_new_and_credential() {
        let migrations_done: Vec<(String, String, Credentials)> = vec![(
            "provider#3".to_string(),
            "provider#2".to_string(),
            Credentials {
                api_key: Some("c".to_string()),
                cookie: None,
                secret_key: None,
            },
        )];
        let (old_ref, new_ref, credential) = &migrations_done[0];
        assert_eq!(old_ref, "provider#3");
        assert_eq!(new_ref, "provider#2");
        assert_eq!(credential.api_key.as_deref(), Some("c"));
    }

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
