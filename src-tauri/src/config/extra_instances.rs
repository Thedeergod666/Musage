//! 用户额外添加的 source 实例持久化（PR 1a · 来源复制 / 副本支持）
//!
//! ## 设计动机
//!
//! PR 3 之前 [`crate::config::custom_sources`] 只存 New API 中转站的 spec ——
//! 跟内置 provider 是平行通道。用户无法添加「内置 provider 的副本」（比如
//! 同时持有 2 个 MiniMax 套餐 / 2 个 DeepSeek 账号），因为 builtin 是写死的
//! 11 个，新加一份要改注册表。
//!
//! PR 1a 把「任意 source 的额外实例」统一持久化到这一个文件：
//! - 内置 provider（minimax / deepseek / xiaomi / tavily / zenmux / openrouter /
//!   kimi / zhipu / stepfun / siliconflow / claude_official）的副本走同一份 spec
//! - New API 中转站（PR 3 的 custom flow）走 `provider_id = "custom"` + `custom: Some(spec)`
//!
//! 副本通过 `instance_index` 区分：内置第一份 index=1 不进本文件，副本从 #2 起。
//! 删除后**按类型内紧凑**重排（决策 D1 紧凑策略）。
//!
//! ## 文件位置
//!
//! `<config_dir>/com.musage.app/extra_instances.json` —— 跟 `custom_sources.json` /
//! `config.json` / `keys.json` 同目录，结构 top-level array（不是 map），方便
//! git diff + 手编辑。
//!
//! ## 文件名 vs custom_sources.json（v0.2.1 迁移完成）
//!
//! 历史 `custom_sources.json` 文件 v0.2.0 → v0.2.1 迁移期处理：v0.2.1 commit 2
//! (`a968029 chore(refactor): inline custom_sources migration`) 把迁移代码
//! 内联到本文件 `load_or_migrate()`，`config/custom_sources.rs` wrapper 删了。
//!
//! 当前行为：v0.2.1 启动时检查 `extra_instances.json` 是否存在；不存在但
//! 老 `custom_sources.json` 存在 → 迁：把 spec 转成 ExtraInstance，存
//! `extra_instances.json`，rename 老文件成 `custom_sources.json.migrated`。
//!
//! ## API key 不在这里
//!
//! 复用 [`super::save_credential_for_id`] / [`super::delete_credential_for_id`] /
//! [`super::load_credential_for_id`]。
//! - 内置副本的 `api_key_ref` = `"minimax#2"` 形式（跟 instance_index 对应）
//! - 自定义来源（custom）的 `api_key_ref` = `"custom_<uuid>"`（沿用 PR 3 约定）
//!
//! ## 锁顺序（重要）
//!
//! `extra_instances` 文件 IO 走 [`super::save_lock`]（std Mutex），跟 keys.json /
//! custom_sources.json 三条写路径互相竞争 → 串行化。任何调用方**必须**：
//! 1. 先拿 `state.extra_instances.write().await`（tokio RwLock）
//! 2. 再调 `save()`（拿 `save_lock`）
//!
//! 反过来会形成 `save_lock → extra_instances → ...` 反向锁链。
//!
//! ## 持久化格式
//!
//! ```json
//! [
//!   {
//!     "id": "550e8400-e29b-41d4-a716-446655440000",
//!     "provider_id": "minimax",
//!     "instance_index": 2,
//!     "api_key_ref": "minimax#2",
//!     "custom": null,
//!     "created_at": 1750000000
//!   },
//!   {
//!     "id": "custom_a1b2c3...",
//!     "provider_id": "custom",
//!     "instance_index": 2,
//!     "api_key_ref": "custom_a1b2c3...",
//!     "custom": { "id": "custom_a1b2c3...", "display_name": "DMX", ... },
//!     "created_at": 1750000100
//!   }
//! ]
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::providers::CustomSourceSpec;

const EXTRA_INSTANCES_FILE: &str = "extra_instances.json";

/// 一条额外 source 实例（内置 provider 副本 / New API 中转站）。
///
/// `id` 是 UUID（v4，simple 格式 32 字符 hex）—— PR 1a 阶段前端 IPC 还用
/// 这个 id 删 / 改；后续 PR 3 UI 重构可以用 `provider_id + instance_index`
/// 定位，但底层 id 仍作为稳定主键。
///
/// `instance_index` 语义：
/// - 内置 provider 第一份 = 1（**不**进本文件，由 [`crate::providers::builtin_sources`] 提供）
/// - 内置 provider 副本 ≥ 2，进本文件
/// - New API 中转站：第一个 custom instance 也是 2（因为"内置 custom"不存在，
///   用户的第一个 New API 算 instance_index=1 也即唯一份，但为了统一编号规则，
///   custom 也从 2 起 —— 见 [`next_index_for`] 注释）
///
/// **不对外承诺 instance_index 稳定**：删除时按类型内紧凑重排（[`compact_indexes_for`]），
/// api_key_ref 同步重命名。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtraInstance {
    pub id: Uuid,
    pub provider_id: String,
    pub instance_index: u32,
    /// keys.json 里的 key 名：`"minimax#2"` / `"custom_<uuid>"`
    pub api_key_ref: String,
    /// 仅 `provider_id == "custom"` 时 `Some(spec)`；内置副本恒 `None`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<CustomSourceSpec>,
    pub created_at: i64,
}

impl ExtraInstance {
    /// 构造一个 New API 中转站（custom）实例 —— PR 1a / PR 3 兼容。
    pub fn new_custom(spec: CustomSourceSpec) -> Self {
        let id_str = spec.id.clone();
        Self {
            id: Uuid::new_v4(),
            provider_id: "custom".to_string(),
            instance_index: 2, // 在 next_index_for 里被覆盖；初始值仅占位
            api_key_ref: id_str,
            custom: Some(spec),
            created_at: chrono::Utc::now().timestamp(),
        }
    }

    /// 构造一个内置 provider 的副本实例。
    ///
    /// `base_provider_id`: `"minimax"` / `"deepseek"` / ...
    /// `instance_index`: 由 [`next_index_for`] 算好（≥ 2）
    pub fn new_builtin_duplicate(base_provider_id: &str, instance_index: u32) -> Self {
        let api_key_ref = format!("{base_provider_id}#{instance_index}");
        Self {
            id: Uuid::new_v4(),
            provider_id: base_provider_id.to_string(),
            instance_index,
            api_key_ref,
            custom: None,
            created_at: chrono::Utc::now().timestamp(),
        }
    }
}

/// 加载所有 extra instance specs。
///
/// 行为：
/// - 文件不存在 → `Ok(vec![])`
/// - 文件存在但 parse 失败 → 备份到 `.bak.<timestamp>` + `Ok(vec![])`
/// - 文件为空字符串 → `Ok(vec![])`
pub fn load() -> Result<Vec<ExtraInstance>, String> {
    let path = extra_instances_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let s = std::fs::read_to_string(&path)
        .map_err(|e| format!("read extra_instances.json: {e}"))?;
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Vec<ExtraInstance>>(&s) {
        Ok(v) => Ok(v),
        Err(e) => {
            // parse 失败：备份到 .bak.<ts> + 返空。跟 custom_sources 同款护栏。
            // 2026-06-20 audit 修复：之前 backup 失败用 `let _ = std::fs::copy(...)`
            // 吞错，read-only 目录 / 满盘 → backup 失败 → 下次 save 用空 Vec
            // 覆盖 → 用户全部 extra instance 静默丢失。改 error 级日志。
            let ts = chrono::Utc::now().timestamp();
            let backup = path.with_extension(format!("json.bak.{ts}"));
            if let Err(copy_err) = std::fs::copy(&path, &backup) {
                tracing::error!(
                    source = %path.display(),
                    backup = %backup.display(),
                    copy_error = %copy_err,
                    parse_error = %e,
                    "extra_instances.json 解析失败且备份失败 — 下次 save 会用空 Vec 覆盖",
                );
            } else {
                tracing::warn!(
                    error = %e,
                    backup = %backup.display(),
                    "extra_instances.json parse 失败，已备份到 .bak",
                );
            }
            Ok(Vec::new())
        }
    }
}

/// 原子写：先写 .tmp + 0600，再 rename 覆盖（跟 [`super::write_keys_atomic`] 同款）。
///
/// 跟 keys.json 写路径共享 `save_lock` —— 保证两条写不会并发丢字段。
pub fn save(instances: &[ExtraInstance]) -> Result<(), String> {
    let _g = super::save_lock().lock().unwrap_or_else(|e| {
        tracing::warn!("save_extra_instances save_lock poisoned, recovering");
        e.into_inner()
    });
    let path = extra_instances_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(instances)
        .map_err(|e| format!("serialize extra_instances: {e}"))?;
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
        return Err(format!("rename extra_instances: {e}"));
    }
    Ok(())
}

fn extra_instances_path() -> Result<PathBuf, String> {
    let dir = super::config_dir()?;
    Ok(dir.join("com.musage.app").join(EXTRA_INSTANCES_FILE))
}

/// 启动时调一次：读 `extra_instances.json`，不存在则从老 `custom_sources.json` 迁。
///
/// v0.2.1 commit 2 后:从 `config/custom_sources.rs::load_or_migrate` 内联过来,
/// wrapper 文件 `config/custom_sources.rs` 删除。语义未变,只是把函数搬到家。
///
/// 行为：
/// 1. `extra_instances.json` 存在 → 直接 `load()`
/// 2. 否则 `custom_sources.json` 存在 → 迁:
///    - 读老 `Vec<CustomSourceSpec>`
///    - 转成 `Vec<ExtraInstance>`(每个 spec 一个 ExtraInstance,instance_index 按
///      created_at 升序从 2 起 —— 跟 builtin 副本编号语义一致)
///    - 写 `extra_instances.json`(原子写 + 0600)
///    - rename 老文件 → `custom_sources.json.migrated`(失败也不 panic,只是日志)
/// 3. 都不存在 → `Ok(vec![])`
pub fn load_or_migrate() -> Result<Vec<ExtraInstance>, String> {
    // 1. 新文件已存在 → 直接返
    if extra_instances_path_exists() {
        return load();
    }

    // 2. 尝试读老文件
    let old_path = custom_sources_path()?;
    if !old_path.exists() {
        return Ok(Vec::new());
    }

    let old_specs = match load_custom_sources_for_migration() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "load_or_migrate: 老 custom_sources.json 读取失败,跳过迁移");
            return Ok(Vec::new());
        }
    };

    // 转 spec → ExtraInstance
    let now = chrono::Utc::now().timestamp();
    let mut by_created: Vec<crate::providers::CustomSourceSpec> = old_specs;
    by_created.sort_by_key(|s| s.created_at);

    let new_instances: Vec<ExtraInstance> = by_created
        .into_iter()
        .enumerate()
        .map(|(i, spec)| ExtraInstance {
            id: uuid::Uuid::new_v4(),
            provider_id: "custom".to_string(),
            instance_index: (i as u32) + 2, // custom 也从 2 起
            api_key_ref: spec.id.clone(),
            custom: Some(spec),
            created_at: now,
        })
        .collect();

    // 写新文件(best-effort:写失败返空,不 panic —— 用户数据全在老文件)
    if let Err(e) = save(&new_instances) {
        tracing::error!(error = %e, "load_or_migrate: 写 extra_instances.json 失败");
        return Ok(Vec::new());
    }

    // rename 老文件 → .migrated(best-effort)
    let migrated_path = old_path.with_extension("json.migrated");
    if let Err(e) = std::fs::rename(&old_path, &migrated_path) {
        tracing::warn!(
            error = %e,
            old = %old_path.display(),
            migrated = %migrated_path.display(),
            "load_or_migrate: rename 老 custom_sources.json 失败(不影响功能,老文件留在原地)",
        );
    } else {
        tracing::info!(
            count = new_instances.len(),
            migrated = %migrated_path.display(),
            "load_or_migrate: 已把 custom_sources.json 迁到 extra_instances.json",
        );
    }

    Ok(new_instances)
}

fn extra_instances_path_exists() -> bool {
    extra_instances_path()
        .map(|p| p.exists())
        .unwrap_or(false)
}

const CUSTOM_SOURCES_LEGACY_FILE: &str = "custom_sources.json";

fn custom_sources_path() -> Result<PathBuf, String> {
    let dir = super::config_dir()?;
    Ok(dir.join("com.musage.app").join(CUSTOM_SOURCES_LEGACY_FILE))
}

/// 老 `custom_sources.json` 读取(仅供 [`load_or_migrate`] 内部迁移用)。
///
/// 行为跟原来 [`crate::config::custom_sources::load_custom_sources`] 完全一致:
/// - 文件不存在 → `Ok(vec![])`
/// - parse 失败 → 备份到 `.bak.<ts>` + `Ok(vec![])`
/// - 文件为空字符串 → `Ok(vec![])`
fn load_custom_sources_for_migration() -> Result<Vec<crate::providers::CustomSourceSpec>, String> {
    let path = custom_sources_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let s = std::fs::read_to_string(&path)
        .map_err(|e| format!("read custom_sources.json: {e}"))?;
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    match serde_json::from_str::<Vec<crate::providers::CustomSourceSpec>>(&s) {
        Ok(v) => Ok(v),
        Err(e) => {
            // parse 失败:备份到 .bak.<ts> + 返空。避免一次坏写入把全部 spec 删了。
            // 2026-06-20 audit:之前 backup 失败用 `let _ = std::fs::copy(...)`
            // 吞错,read-only 目录 / 满盘 → backup 失败 → 下次 save 用空 Vec
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
                    "custom_sources.json parse 失败,已备份到 .bak",
                );
            }
            Ok(Vec::new())
        }
    }
}

/// 计算下一个 instance_index —— 同 `provider_id` 内 max+1。
///
/// **从 2 起**：内置 provider 的第 1 份由 [`crate::providers::builtin_sources`]
/// 直接提供，instance_index=1 是隐含的、不进本文件。新副本 = max+1，max 初始 1
/// → 第一个副本就是 2。
///
/// Custom 来源特殊：`provider_id == "custom"` 的内置那一份**不存在**（没有
/// "内置 New API 中转站"），但为了统一编号规则，用户的第一个 New API 也算
/// "副本"，从 2 起 —— 浮窗渲染时会显示 `display_name`（用户起的名字）不带
/// `#2`，因为 New API 没有"内置第 1 份"做参照。
///
/// # 决策
///
/// D1（紧凑）+ D5（按类型内编号）直接落地在这里。
pub fn next_index_for(provider_id: &str, existing: &[ExtraInstance]) -> u32 {
    existing
        .iter()
        .filter(|e| e.provider_id == provider_id)
        .map(|e| e.instance_index)
        .max()
        .unwrap_or(1)
        + 1
}

/// 删除一个 instance 后，**同 provider_id 内**按 created_at 升序重排 2,3,4,...
///
/// 同时同步改 `api_key_ref`：
/// - 内置 provider（`"minimax"` / `"deepseek"` / ...）：`"minimax#3"` → `"minimax#2"`
/// - custom（`"custom"`）：**不动 `api_key_ref`**（custom 走 `custom_<uuid>`，不带 #）
///
/// 调用方在改完 instance_index 后需要把旧 `api_key_ref` 下的 api_key 复制 /
/// 重命名到新 `api_key_ref`。**本函数不改 keys.json**，由调用方配合
/// [`super::save_credential_for_id`] / [`super::delete_credential_for_id`] 处理。
///
/// **不**改 `id`（UUID 是稳定主键，PR 3 UI 用它做删除 / 更新 key）。
pub fn compact_indexes_for(provider_id: &str, existing: &mut [ExtraInstance]) {
    let mut same_type: Vec<usize> = existing
        .iter()
        .enumerate()
        .filter(|(_, e)| e.provider_id == provider_id)
        .map(|(i, _)| i)
        .collect();
    // 按 created_at 升序，紧凑编号从 2 起
    same_type.sort_by_key(|&i| existing[i].created_at);
    for (new_idx, &src_idx) in same_type.iter().enumerate() {
        let new_index = (new_idx as u32) + 2;
        let old_index = existing[src_idx].instance_index;
        if old_index != new_index {
            existing[src_idx].instance_index = new_index;
            // 内置 provider 才重写 api_key_ref；custom 走 spec.id 不带 #N
            if provider_id != "custom" {
                let new_api_key_ref = format!("{}#{}", provider_id, new_index);
                tracing::info!(
                    provider = provider_id,
                    old_index,
                    new_index,
                    old_api_key_ref = %existing[src_idx].api_key_ref,
                    new_api_key_ref = %new_api_key_ref,
                    "compact_indexes_for: 重排 instance_index",
                );
                existing[src_idx].api_key_ref = new_api_key_ref;
            } else {
                tracing::info!(
                    provider = provider_id,
                    old_index,
                    new_index,
                    api_key_ref = %existing[src_idx].api_key_ref,
                    "compact_indexes_for: 重排 custom instance_index (api_key_ref 保持 custom_<uuid>)",
                );
            }
        }
    }
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::custom::ExtractSpec;

    fn builtin_dup(provider_id: &str, idx: u32, created_at: i64) -> ExtraInstance {
        ExtraInstance {
            id: Uuid::new_v4(),
            provider_id: provider_id.to_string(),
            instance_index: idx,
            api_key_ref: format!("{provider_id}#{idx}"),
            custom: None,
            created_at,
        }
    }

    fn custom_dup(display_name: &str, idx: u32, created_at: i64) -> ExtraInstance {
        let spec = CustomSourceSpec {
            id: format!("custom_{}", Uuid::new_v4().simple()),
            display_name: display_name.to_string(),
            base_url: "https://api.test.com".to_string(),
            path: "/api/user/self".to_string(),
            method: "GET".to_string(),
            extract: ExtractSpec::NewApi { divide: None },
            plan_name_path: None,
            accent: None,
            created_at,
        };
        ExtraInstance {
            id: Uuid::new_v4(),
            provider_id: "custom".to_string(),
            instance_index: idx,
            api_key_ref: spec.id.clone(),
            custom: Some(spec),
            created_at,
        }
    }

    #[test]
    fn next_index_for_empty() {
        // 新 provider 没任何副本 → 下一个是 2
        assert_eq!(next_index_for("minimax", &[]), 2);
        assert_eq!(next_index_for("custom", &[]), 2);
    }

    #[test]
    fn next_index_for_existing_increments() {
        let v = vec![
            builtin_dup("minimax", 2, 1000),
            builtin_dup("minimax", 3, 2000),
            builtin_dup("minimax", 5, 3000),
        ];
        // max(2,3,5) + 1 = 6
        assert_eq!(next_index_for("minimax", &v), 6);
    }

    #[test]
    fn next_index_for_filters_by_provider_id() {
        // minimax 副本到 4，deepseek 副本到 2 → 互不影响
        let v = vec![
            builtin_dup("minimax", 4, 1000),
            builtin_dup("deepseek", 2, 1500),
        ];
        assert_eq!(next_index_for("minimax", &v), 5);
        assert_eq!(next_index_for("deepseek", &v), 3);
        // 还没出现过的 provider
        assert_eq!(next_index_for("xiaomi", &v), 2);
    }

    #[test]
    fn compact_indexes_after_delete_middle() {
        // 现有 minimax: #2/#3/#4，删 #3 后剩 #2/#4 → 重排为 #2/#3
        let mut v = vec![
            builtin_dup("minimax", 2, 1000),
            builtin_dup("minimax", 3, 2000),
            builtin_dup("minimax", 4, 3000),
        ];
        // 模拟删 #3 后剩 #2 + #4
        v.retain(|e| e.instance_index != 3);
        compact_indexes_for("minimax", &mut v);

        assert_eq!(v.len(), 2);
        // 按 created_at 升序：1000 (原 #2) → #2，3000 (原 #4) → #3
        assert_eq!(v[0].instance_index, 2);
        assert_eq!(v[0].api_key_ref, "minimax#2");
        assert_eq!(v[1].instance_index, 3);
        assert_eq!(v[1].api_key_ref, "minimax#3");
    }

    #[test]
    fn compact_indexes_preserves_other_providers() {
        // minimax #2/#3，删 #2 后剩 #3 → minimax 重排为 #2
        // deepseek #2 不该被影响
        let mut v = vec![
            builtin_dup("minimax", 2, 1000),
            builtin_dup("minimax", 3, 2000),
            builtin_dup("deepseek", 2, 3000),
        ];
        v.retain(|e| !(e.provider_id == "minimax" && e.instance_index == 2));
        compact_indexes_for("minimax", &mut v);

        // minimax 剩 #3 → 变成 #2
        let minimax: Vec<&ExtraInstance> =
            v.iter().filter(|e| e.provider_id == "minimax").collect();
        assert_eq!(minimax.len(), 1);
        assert_eq!(minimax[0].instance_index, 2);
        assert_eq!(minimax[0].api_key_ref, "minimax#2");
        // deepseek 不变
        let deepseek: Vec<&ExtraInstance> =
            v.iter().filter(|e| e.provider_id == "deepseek").collect();
        assert_eq!(deepseek[0].instance_index, 2);
        assert_eq!(deepseek[0].api_key_ref, "deepseek#2");
    }

    #[test]
    fn compact_indexes_no_op_when_already_compact() {
        // 已经是 2/3/4，compact 应该不动（不触发重命名噪声）
        let mut v = vec![
            builtin_dup("minimax", 2, 1000),
            builtin_dup("minimax", 3, 2000),
            builtin_dup("minimax", 4, 3000),
        ];
        let snapshot: Vec<(u32, String)> = v
            .iter()
            .map(|e| (e.instance_index, e.api_key_ref.clone()))
            .collect();
        compact_indexes_for("minimax", &mut v);
        let after: Vec<(u32, String)> = v
            .iter()
            .map(|e| (e.instance_index, e.api_key_ref.clone()))
            .collect();
        assert_eq!(snapshot, after, "already-compact should be no-op");
    }

    #[test]
    fn compact_indexes_handles_custom() {
        // custom 也走同一逻辑（虽然 custom 没"内置第 1 份"做参照）
        let mut v = vec![
            custom_dup("DMX", 2, 1000),
            custom_dup("CrazyRouter", 3, 2000),
        ];
        // 删 DMX → CrazyRouter 变 #2
        v.retain(|e| {
            e.custom
                .as_ref()
                .map(|s| s.display_name != "DMX")
                .unwrap_or(true)
        });
        compact_indexes_for("custom", &mut v);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].instance_index, 2);
        // custom 的 api_key_ref 不重命名（"custom_<uuid>" 不带 #）
        assert!(v[0].api_key_ref.starts_with("custom_"));
    }

    #[test]
    fn new_builtin_duplicate_sets_correct_fields() {
        let e = ExtraInstance::new_builtin_duplicate("deepseek", 3);
        assert_eq!(e.provider_id, "deepseek");
        assert_eq!(e.instance_index, 3);
        assert_eq!(e.api_key_ref, "deepseek#3");
        assert!(e.custom.is_none());
        assert!(e.created_at > 0);
    }

    #[test]
    fn new_custom_uses_spec_id_as_api_key_ref() {
        let spec = CustomSourceSpec {
            id: "custom_abc123".to_string(),
            display_name: "DMX".to_string(),
            base_url: "https://x.com".to_string(),
            path: "/p".to_string(),
            method: "GET".to_string(),
            extract: ExtractSpec::NewApi { divide: None },
            plan_name_path: None,
            accent: None,
            created_at: 1000,
        };
        let e = ExtraInstance::new_custom(spec.clone());
        assert_eq!(e.provider_id, "custom");
        assert_eq!(e.api_key_ref, spec.id);
        assert!(e.custom.is_some());
    }
}