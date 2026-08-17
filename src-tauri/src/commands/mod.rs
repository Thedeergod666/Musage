//! 暴露给前端的 tauri commands
//!
//! ## 架构 (v0.2)
//!
//! 所有 IPC 走字符串 source id（[`set_source_credential`] / [`get_source_credential`] /
//! [`has_source_credential`] / [`delete_source_credential`] / [`list_sources`]）。
//! v0.1 时代有 7 个 enum-based IPC (set_api_key_for / set_cookie_for 等),
//! 已在 v0.2 (2026-06-22) 硬删除 —— 这是 BREAKING 变更, 升级用户
//! 必须把第三方脚本/插件切到新 API。
//!
//! ## 关键路径
//!
//! [`refresh_inner`] 用 [`crate::providers::builtin_sources`] 注册表遍历所有启用的
//! source，每个 source 自己负责鉴权 + 拉数据 + 解析。这是 ROADMAP Phase 1 的核心。
//!
//! [`refresh_now`] 和 [`crate::poller::tick`] 共用 refresh_inner。
//!
//! PR 3：custom_sources 子模块装 5 个用户自定义 New API source 的 IPC。
//! 拆出子模块是因为 `commands/mod.rs` 本身已经 1200+ 行。

// PR 1b：用户额外 source 实例 IPC (6 commands)
pub mod extra_instances;
pub mod i18n;

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::config::{self, AppConfig, FloatingPinMode, ProviderConfig, TrayIconStyle, UserRegion};
use crate::poller_backoff::RefreshSource;
use crate::providers::minimax::Region as MinimaxRegion;
use crate::providers::xiaomi::XiaomiDisplayMode;
use crate::providers::{
    all_sources, builtin_sources, find_source, AuthKind, Credentials, ErrorKind, FetchError,
    ProviderSnapshot, QuotaSnapshot, QuotaSource,
};
use crate::t;
use crate::AppState;

/// 立即更新 provider 顺序 + 落盘 + emit config-changed（前端调，无需走
/// save_config 全量保存）。前端用这个实现「↑↓ 按钮即时生效」。
#[tauri::command]
pub async fn set_provider_order(
    state: State<'_, AppState>,
    app: AppHandle,
    order: Vec<String>,
) -> Result<(), String> {
    // CM10 fix (2026-07-28 审查): 落盘前清洗 —— 之前不校验 order 字符串,
    // 未知 id / 重复 / 超长直接落盘(前端 bug 或手搓 IPC 会写进垃圾顺序)。
    // known = 当前全部 source 的 unique_id(内置 base id + 副本 "minimax#2"
    // + custom_<uuid>)。注意在拿 config.write 之前调 all_sources(它内部
    // 拿 extra_instances.read),避免锁嵌套。
    let known: std::collections::HashSet<String> = all_sources(&state)
        .await
        .iter()
        .map(|s| s.unique_id())
        .collect();
    let order = sanitize_provider_order(order, &known);
    // 锁顺序契约（**2026-06-20 audit fix**：之前注释描述的顺序跟实际代码
    // 不一致，误导未来维护者）：
    //
    //   本函数:  config.write → save → drop cfg  →  cfg.read + snapshot.write
    //   refresh_single_inner:  snapshot.write → drop snap → cfg.read
    //
    // 两者都是「持有 snapshot 时不持有 config.write」的对称结构，不会
    // 死锁。如果本函数改用「先 config.write → snapshot.write → drop」就会跟
    // refresh_single_inner 形成 config.write + snapshot.write 的循环等待。
    {
        let mut cfg = state.config.write().await;
        cfg.provider_order = order;
        cfg.save()?;
    }
    // 重排 in-memory snapshot 并 emit 给浮窗，让浮窗立刻按新顺序渲染。
    //
    // ⚠️ 关键：必须先 drop cfg_snap 和 snap 两个锁，再 emit。
    // 如果持有锁期间 emit，refresh_single_inner 同时拿 snapshot.write
    // 会死锁 → emit 永远发不出 → 浮窗永远不刷新。
    {
        let cfg_snap = state.config.read().await;
        let mut snap = state.snapshot.write().await;
        apply_provider_order(&mut snap, &cfg_snap);
        let s = snap.clone();
        drop(snap);
        drop(cfg_snap);
        let _ = app.emit("musage://snapshot", &s);
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// CM10 fix (2026-07-28 审查): provider_order 落盘前的基础清洗 ——
/// 过滤未知 id + 去重(保留首次出现) + 限长。宽容策略:不 reject 整个
/// 请求,只剔除坏条目,合法顺序完全不受影响。
fn sanitize_provider_order(
    order: Vec<String>,
    known: &std::collections::HashSet<String>,
) -> Vec<String> {
    // 13 内置 + N extras,128 足够宽容(正常 order 长度 == known 数量)
    const MAX_LEN: usize = 128;
    let mut seen = std::collections::HashSet::new();
    order
        .into_iter()
        .filter(|id| known.contains(id))
        .filter(|id| seen.insert(id.clone()))
        .take(MAX_LEN)
        .collect()
}

/// 立即更新单个 provider 的 enabled 标志 + 落盘 + emit。供设置面板
/// 「在浮窗显示 X」复选框 onchange 即时调用。
#[tauri::command]
pub async fn set_provider_enabled(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    // P3 audit fix (2026-08-13): 之前任意字符串都写进 cfg.providers (entry
    // .or_insert), 前端注入 / 手搓 IPC 会留垃圾 key, 还可能跟 snapshot_key
    // / merge 逻辑冲突。校验 id 是已知 source (内置 base / 副本 unique_id /
    // custom_<uuid>) 再写。all_sources 在 config.write 之前调 (锁顺序同
    // set_provider_order)。
    let known: std::collections::HashSet<String> = all_sources(&state)
        .await
        .iter()
        .map(|s| s.unique_id())
        .collect();
    if !known.contains(&id) {
        return Err(format!("unknown provider id: {id}"));
    }
    {
        let mut cfg = state.config.write().await;
        // 缺 key 时插一份默认配置（保持 BTreeMap key 顺序 + 默认值）
        let entry = cfg
            .providers
            .entry(id.clone())
            .or_insert(crate::config::ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            });
        entry.enabled = enabled;
        // M4 fix (2026-07-06 全量审查): 跟 `add_extra_instance` / 锁顺序契约
        // 保持一致 —— 在持有 config.write 时,同步取一份 extra_instances
        // read guard(即使不用值),保证调用方刷新时可观测一致快照。
        let _extras_raii = state.extra_instances.read().await;
        cfg.save()?;
    }
    // 如果用户关掉了某个 provider，立刻清掉它在 in-memory snapshot 里
    // 的条目（不然浮窗下次刷新前还会显示旧数据）。
    if !enabled {
        let state_arc = app.state::<AppState>();
        let mut snap = state_arc.snapshot.write().await;
        // H2 fix (2026-08-03 audit): 用 snapshot_key 统一身份键 (P3 同款规则)
        // —— source_id 匹配置信 base id,副本 (minimax#2) 关闭时不会真被移除。
        snap.providers.retain(|p| snapshot_key(p) != id);
        let emit_snap = snap.clone();
        drop(snap);
        // 排序 + emit
        let cfg2 = state_arc.config.read().await;
        let mut emit = emit_snap;
        apply_provider_order(&mut emit, &cfg2);
        drop(cfg2);
        let _ = app.emit("musage://snapshot", &emit);
    } else {
        // ── 乐观 emit：先发 placeholder，让浮窗立刻显示新卡片 ─────────
        // 之前 await refresh_single_inner 才会 emit snapshot，浮窗要等
        // HTTP fetch（2-5s）才看到新卡片。改成"placeholder 立即显示 →
        // 后台 fetch → 真数据替换"，体验更跟手（fix-drag-delay-2026-06-18）。
        //
        // 实现要点：
        // 1. placeholder 复用 empty_error(UnconfiguredKey) —— 浮窗对它有
        //    专用渲染路径（带「打开设置」按钮），无论用户是否配了 key
        //    都能正确显示。
        // 2. fetch 用 tokio::spawn 后台跑 —— set_provider_enabled 立即
        //    返回，让上层 setProviderOrder 能紧跟执行。fetch 失败时
        //    refresh_single_inner 自己会 emit 错误态 snapshot 覆盖 placeholder。
        // 3. 必须先 emit 再 spawn：fetch 完成时 in-memory snapshot 已被
        //    替换为真数据并 emit，浮窗的 snapshot 事件订阅者会再次收到
        //    一次更新，行为完全等价。
        {
            let state_arc = app.state::<AppState>();
            // P1 audit fix (2026-08-13): 之前先拿 snapshot.write 再 await
            // config.read / find_source(extra_instances.read) / backoff.read ——
            // 与 set_provider_order 第二段的 "config.read → snapshot.write"
            // 顺序相反, 三方竞争 (另有 config.write 排队) 时形成锁环,
            // tokio runtime 卡死浮窗冻结。改为先 gather 全部依赖数据
            // (不持 snapshot 锁), 最后拿 snapshot.write 只做纯内存操作。
            let already_present = state_arc
                .snapshot
                .read()
                .await
                .providers
                .iter()
                .any(|p| snapshot_key(p) == id);
            // H2 fix (2026-08-03 audit): 同样改 snapshot_key,避免 placeholder
            // 在已存在的副本上重复 push(老 source_id 匹配置信 base)。
            let placeholder = if already_present {
                None
            } else {
                let mut placeholder = ProviderSnapshot::placeholder(&state_arc, &id).await;
                // **B-NEW-10（2026-06-19 audit）**：placeholder 默认 next_fetch_at=None，
                // 浮窗错误卡片显示"未知"倒计时。
                //
                // H4 fix: 填的 5s 默认值在 backoff 持续 30min 时会让浮窗显示
                // "5s 后取数"但实际要等 30min (退避窗口) → UI 不一致。
                // 改为: 先查 backoff 拿真实 next_interval; 无 backoff 用 cfg
                // 默认 refresh_interval (跟 refresh_single_inner 同款策略)。
                let default_secs = {
                    let cfg_read = state_arc.config.read().await;
                    // P1 audit fix: interval clamp 与 poller/refresh_inner 同款
                    crate::poller::clamp_interval_secs(
                        cfg_read
                            .providers
                            .get(&id)
                            .and_then(|p| p.refresh_interval_secs)
                            .unwrap_or(cfg_read.refresh_interval_secs),
                    )
                };
                let interval_secs = {
                    let backoff = state_arc.backoff.read().await;
                    backoff.next_interval_secs(&id, default_secs)
                };
                placeholder.next_fetch_at =
                    Some(chrono::Utc::now().timestamp_millis() + (interval_secs as i64) * 1000);
                Some(placeholder)
            };
            // 预读整份 config (apply_provider_order 需要 provider_order) ——
            // 在 snapshot.write 之外, 不构成反向锁链
            let cfg_snap = state_arc.config.read().await.clone();
            let mut snap = state_arc.snapshot.write().await;
            if let Some(placeholder) = placeholder {
                // gather 期间可能有并发写入了同 id 条目 —— 二次检查保证
                // check+push 在 snapshot.write 下原子, 不重复 push
                let still_present = snap.providers.iter().any(|p| snapshot_key(p) == id);
                if !still_present {
                    snap.providers.push(placeholder);
                }
            }
            apply_provider_order(&mut snap, &cfg_snap);
            let emit = snap.clone();
            drop(snap);
            let _ = app.emit("musage://snapshot", &emit);
        }
        // 后台 fetch（不 await）
        let app_clone = app.clone();
        let id_owned = id.clone();
        tokio::spawn(async move {
            let _ = refresh_single_inner(
                &app_clone,
                &id_owned,
                crate::poller_backoff::RefreshSource::Manual,
            )
            .await;
        });
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 即时切换 Xiaomi MiMo 浮窗显示模式：完整 / 只套餐 / 只总额度。
///
/// 走单字段 command 路径（参考 `set_provider_enabled`），不走 `save_config` 全量保存。
/// 保存后立即 refresh 一次（poller 下一分钟才 fire，user 等不了）。
#[tauri::command]
pub async fn set_xiaomi_display_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    mode: String,
) -> Result<(), String> {
    let parsed = match mode.as_str() {
        "all" => XiaomiDisplayMode::All,
        "plan_only" => XiaomiDisplayMode::PlanOnly,
        "total_only" => XiaomiDisplayMode::TotalOnly,
        other => return Err(t!("commands.display_mode_unknown_xiaomi", other = other).into_owned()),
    };
    {
        let mut cfg = state.config.write().await;
        let entry = cfg
            .providers
            .entry("xiaomimimo".to_string())
            .or_insert(ProviderConfig {
                enabled: true,
                region: None,
                xiaomi_region: None,
                refresh_interval_secs: None,
                xiaomi_display_mode: None,
            });
        entry.xiaomi_display_mode = Some(parsed);
        cfg.save()?;
    }
    // 立即刷新（让浮窗按新模式显示）。**B-NEW-3（2026-06-19 audit）**：
    // 之前 await refresh_single_inner 让 IPC 调用方阻塞 2-5s（HTTP fetch），
    // 与 sibling set_provider_enabled 不一致（后者走 tokio::spawn 后台
    // fetch + 立即 emit placeholder）。改成 spawn 后立即返回 —— 用户切换
    // 模式时浮窗在 ~100ms 内就有响应。
    let app_clone = app.clone();
    tokio::spawn(async move {
        let _ = refresh_single_inner(
            &app_clone,
            "xiaomimimo",
            crate::poller_backoff::RefreshSource::Manual,
        )
        .await;
    });
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 即时更新 schema_overrides（MiniMax 5h / 周 + Xiaomi 月 字段名候选）。
///
/// 走单字段 command 路径（参考 `set_provider_enabled` / `set_tray_icon_style`），
/// 不走 `save_config` 全量保存。**2026-06-20 audit fix**：之前
/// `advanced.ts` 3 个 textarea 改完根本存不下来 —— 注释里说"blur 触发
/// saveConfig()"，但 `src/settings/config.ts:saveConfig` 没有任何调用方
/// （grep 全 src 0 hit），且 `settings.html` 里没有 `#save` 按钮，schema
/// 改名时用户改的候选字段名永远不会被持久化。
///
/// 现在：advanced.ts 3 个 textarea blur → 立即调本命令（debounce 300ms，
/// 避免连续键入的 N 次 IPC）→ 落盘 + emit config-changed → 下次 poller
/// tick 用新 overrides 重新解析（自动 trigger refresh）。
///
/// 校验：每个 tier 必须是 `count_candidates: []` 或完整对象；
/// `FieldTriple.total` / `.remaining` 必须非空字符串。空数组视为
/// "清空本 tier 的 overrides"，等价于恢复默认字段名。
#[tauri::command]
pub async fn set_schema_overrides(
    state: State<'_, AppState>,
    app: AppHandle,
    // key = provider id ("minimax" / "xiaomimimo"), value = 该 provider 的 overrides
    overrides: std::collections::BTreeMap<String, config::ProviderOverrides>,
) -> Result<(), String> {
    // P2 audit fix (2026-08-13): 本命令不走 save_config 的 256 上限, 之前可
    // 直接灌 100k entry / 单 tier 100k candidates → O(n) 校验 + 序列化 +
    // 写盘 DoS。与 save_config 同款上限 + 单 tier candidates 上限。
    if overrides.len() > SCHEMA_OVERRIDES_MAX {
        return Err(format!(
            "commands.schema_overrides_too_many: count={} max={}",
            overrides.len(),
            SCHEMA_OVERRIDES_MAX
        ));
    }
    // 1. 校验（避免 N+1 个 3-tuple 静默通过，最后 parse 时才报错 —— 早 fail 早定位）
    for (id, prov) in &overrides {
        for (tier_name, tier) in [
            ("five_hour", &prov.five_hour),
            ("weekly", &prov.weekly),
            ("monthly", &prov.monthly),
        ] {
            if tier.count_candidates.len() > COUNT_CANDIDATES_MAX {
                return Err(format!(
                    "commands.schema_overrides_candidates_too_many: id={} tier={} count={} max={}",
                    id,
                    tier_name,
                    tier.count_candidates.len(),
                    COUNT_CANDIDATES_MAX
                ));
            }
            for (i, ft) in tier.count_candidates.iter().enumerate() {
                if ft.total.trim().is_empty() || ft.remaining.trim().is_empty() {
                    return Err(t!(
                        "commands.schema_override_empty_field",
                        id = id,
                        tier = tier_name,
                        idx = i
                    )
                    .into_owned());
                }
                // CM12 fix (2026-07-28 审查): total 与 remaining 同名字段是
                // 自指 override —— 解析时 remaining == total,用量恒算成 0%,
                // 保存后静默出错误数据。提前拒绝。
                if ft.total.trim() == ft.remaining.trim() {
                    return Err(t!(
                        "commands.schema_override_duplicate_field",
                        id = id,
                        tier = tier_name,
                        idx = i
                    )
                    .into_owned());
                }
            }
        }
    }
    // 2. 写 cfg + 落盘
    {
        let mut cfg = state.config.write().await;
        cfg.schema_overrides = overrides;
        cfg.save()?;
    }
    // 3. 立即 refresh 那些受影响的 provider（让新 overrides 立刻生效，
    //    poller 下个 tick 等不及）。后台 spawn，不阻塞 IPC。
    let app_clone = app.clone();
    let ids: Vec<String> = {
        let cfg = state.config.read().await;
        cfg.providers.keys().cloned().collect()
    };
    tokio::spawn(async move {
        for id in ids {
            // CM9 fix (2026-07-28 审查): ids 来自 cfg.providers 的 key,含
            // 副本 unique_id("minimax#2") —— 之前 matches! 全串匹配永远
            // 漏掉副本(副本跟 base 共享同一套 schema override,也该刷新)。
            let base = id.split('#').next().unwrap_or(id.as_str());
            if matches!(base, "minimax" | "xiaomimimo") {
                let _ = refresh_single_inner(
                    &app_clone,
                    &id,
                    crate::poller_backoff::RefreshSource::Manual,
                )
                .await;
            }
        }
    });
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 读 Xiaomi 当前显示模式（给设置面板初始化用）。
#[tauri::command]
pub async fn get_xiaomi_display_mode(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = state.config.read().await;
    let mode = cfg
        .providers
        .get("xiaomimimo")
        .and_then(|p| p.xiaomi_display_mode)
        .unwrap_or_default();
    Ok(match mode {
        XiaomiDisplayMode::All => "all".to_string(),
        XiaomiDisplayMode::PlanOnly => "plan_only".to_string(),
        XiaomiDisplayMode::TotalOnly => "total_only".to_string(),
    })
}

// ── C3 fix: source-extras 6 个 per-field setter ────────────────
//
// 之前 source-extras.ts 里的 6 个控件（MiniMax region / Xiaomi region / Tavily
// concise / ZenMux base_url+mode+payg_concise / Zhipu region）只有 UI 没有
// change handler，改了静默丢失。现在每个 setter 后端做：改 cfg 顶层字段 →
// 落盘 → emit config-changed → 立即 refresh_single 让浮窗/托盘立刻反映。

#[tauri::command]
pub async fn set_minimax_region(
    state: State<'_, AppState>,
    app: AppHandle,
    region: String,
) -> Result<(), String> {
    let parsed = match region.as_str() {
        "cn" => MinimaxRegion::Cn,
        "en" => MinimaxRegion::En,
        other => return Err(t!("commands.region_unknown", other = other).into_owned()),
    };
    {
        let mut cfg = state.config.write().await;
        let entry = cfg
            .providers
            .entry("minimax".to_string())
            .or_insert(ProviderConfig::default());
        entry.region = Some(parsed);
        cfg.save()?;
    }
    let _ = refresh_single_inner(
        &app,
        "minimax",
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn set_xiaomi_region_field(
    state: State<'_, AppState>,
    app: AppHandle,
    region: String,
) -> Result<(), String> {
    use crate::providers::xiaomi::XiaomiRegion as Xr;
    let parsed = match region.as_str() {
        "cn" => Xr::Cn,
        "sgp" => Xr::Sgp,
        "ams" => Xr::Ams,
        other => return Err(t!("commands.xiaomi_region_unknown", other = other).into_owned()),
    };
    {
        let mut cfg = state.config.write().await;
        let entry = cfg
            .providers
            .entry("xiaomimimo".to_string())
            .or_insert(ProviderConfig::default());
        entry.xiaomi_region = Some(parsed);
        cfg.save()?;
    }
    let _ = refresh_single_inner(
        &app,
        "xiaomimimo",
        crate::poller_backoff::RefreshSource::Manual,
    )
    .await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn set_tavily_concise_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        cfg.tavily_concise_mode = enabled;
        cfg.save()?;
    }
    let _ =
        refresh_single_inner(&app, "tavily", crate::poller_backoff::RefreshSource::Manual).await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn set_zenmux_base_url(
    state: State<'_, AppState>,
    app: AppHandle,
    url: String,
) -> Result<(), String> {
    let trimmed = url.trim();
    if !trimmed.is_empty() && !trimmed.starts_with("https://") {
        return Err(t!("error.common.url_scheme_invalid", url = trimmed).into_owned());
    }
    // 2026-08-17 audit C-01: 写入侧也拦 userinfo bypass（`https://zenmux.ai@evil.com`），
    // 与 fetch 侧 providers::url_authority_has_userinfo 对齐，防御纵深。
    if !trimmed.is_empty() && crate::providers::url_authority_has_userinfo(trimmed) {
        return Err(t!("error.common.url_authority_has_userinfo", url = trimmed).into_owned());
    }
    {
        let mut cfg = state.config.write().await;
        cfg.zenmux_base_url = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        cfg.save()?;
    }
    let _ =
        refresh_single_inner(&app, "zenmux", crate::poller_backoff::RefreshSource::Manual).await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn set_zenmux_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    mode: String,
) -> Result<(), String> {
    if mode != "payg" && mode != "subscription" {
        return Err(t!("commands.zenmux_mode_unknown", other = mode.as_str()).into_owned());
    }
    {
        let mut cfg = state.config.write().await;
        cfg.zenmux_mode = Some(mode.clone());
        cfg.save()?;
    }
    let _ =
        refresh_single_inner(&app, "zenmux", crate::poller_backoff::RefreshSource::Manual).await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn set_zenmux_payg_concise(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        cfg.zenmux_payg_concise_mode = Some(enabled);
        cfg.save()?;
    }
    let _ =
        refresh_single_inner(&app, "zenmux", crate::poller_backoff::RefreshSource::Manual).await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn set_zhipu_region(
    state: State<'_, AppState>,
    app: AppHandle,
    region: String,
) -> Result<(), String> {
    if region != "cn" && region != "en" {
        return Err(t!("commands.zhipu_region_unknown", other = region.as_str()).into_owned());
    }
    {
        let mut cfg = state.config.write().await;
        cfg.zhipu_region = Some(region);
        cfg.save()?;
    }
    let _ = refresh_single_inner(&app, "zhipu", crate::poller_backoff::RefreshSource::Manual).await;
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn get_snapshot(state: State<'_, AppState>) -> Result<QuotaSnapshot, String> {
    let snap = state.snapshot.read().await.clone();
    let cfg = state.config.read().await;
    // 过滤被关掉的 provider —— 设置面板的「在浮窗显示 X」开关关闭后，
    // 浮窗不应该再看到这张卡。poller 自己也会跳过 disabled，但旧的成功
    // 数据还留在 vecdeque 里，所以需要在这里也过滤一次。
    let mut filtered = snap;
    filtered.providers.retain(|p| {
        // H2 fix (2026-08-03 audit): 改用 snapshot_key —— 副本 (minimax#2)
        // 关闭后浮窗不再显示,跟 set_provider_enabled 的 disable/retain 口径一致。
        cfg.is_enabled_id(snapshot_key(p))
    });
    // 按用户配置的 provider_order 排序（空 = 用 builtin_sources() 顺序）
    apply_provider_order(&mut filtered, &cfg);
    Ok(filtered)
}

#[tauri::command]
pub async fn refresh_now(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<QuotaSnapshot, String> {
    // P6 fix (2026-07-28 审查): 跟 poller::tick 共用全量刷新互斥位。
    // 之前 refresh_now 不做去重直接 refresh_inner —— 启动初始 tick 还在
    // 跑时用户点「立即刷新」→ 13+ provider 双倍并发 fetch。已有实例在跑
    // 时返回当前 in-memory snapshot:在跑那轮完成时会 emit
    // musage://snapshot + 刷新托盘,前端订阅者照常拿到最新数据。
    let Some(_tick_guard) = crate::poller::try_acquire_tick() else {
        tracing::debug!("refresh_now 已有全量刷新在跑,返回当前 snapshot");
        return Ok(state.snapshot.read().await.clone());
    };
    let cfg = state.config.read().await.clone();
    let snap = refresh_inner(&app, &cfg, RefreshSource::Manual).await?;
    // 合并写回 state（而不是整块覆写）—— 跟 tick() 同理：
    // refresh_inner 并发拉所有 provider 的过程中，per-provider poller 可能已经
    // 把某个 provider 更新到 state.snapshot 里了；整块覆写会把那份新数据回滚。
    {
        let mut guard = state.snapshot.write().await;
        for new_p in &snap.providers {
            // P3 fix (2026-07-28 审查): 合并键统一为 snapshot_key(unique_id
            // 优先),详见 tick() 同处注释。
            let new_id = snapshot_key(new_p);
            if let Some(idx) = guard
                .providers
                .iter()
                .position(|p| snapshot_key(p) == new_id)
            {
                guard.providers[idx] = new_p.clone();
            } else {
                guard.providers.push(new_p.clone());
            }
        }
        guard.fetched_at = snap.fetched_at;
        guard.wallet_alert_threshold = snap.wallet_alert_threshold;
    }
    // refresh_inner 内部已经 emit 过一次，这里再 emit 合并后的完整快照
    // （refresh_inner emit 的是它自己收集的版本，不含 per-provider 的中间更新）
    let state2 = app.state::<AppState>();
    let final_snap = state2.snapshot.read().await.clone();
    let _ = app.emit("musage://snapshot", &final_snap);
    let tray_style = cfg.tray_icon_style;
    let tray_source = cfg.tray_source.as_deref().unwrap_or("minimax").to_string();
    let tray_color = crate::tray::tray_fill_color(cfg.tray_icon_color.as_deref());
    if let Err(e) = crate::tray::update_tray_from_snapshot(
        &app,
        &final_snap,
        tray_style,
        &tray_source,
        tray_color,
    ) {
        tracing::warn!(error = %e, "刷新托盘失败");
    }
    Ok(final_snap)
}

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    Ok(state.config.read().await.clone())
}

/// H4 fix (2026-07-29 审查): providers map size cap 防 DoS。
/// 12 builtin + ~240 extra 是合理上限 (实际场景 < 50)。
/// 前端绕过 set_provider_enabled / add_custom_source 直接 save_config
/// 灌 100k 条空 entry → serde_json 序列化 + 写盘都慢,无意义。
const PROVIDERS_MAP_MAX: usize = 256;
// D4-009 fix (2026-07-30 audit): provider_order / schema_overrides 也加
// 上限,挡住 IPC DoS 路径。provider_order 跟 builtin_sources() 12 + custom
// 256 走同一上限 (256);schema_overrides 跟 provider_id 一一对应, 同样
// 上限 256。
const ORDER_LIST_MAX: usize = 256;
const SCHEMA_OVERRIDES_MAX: usize = 256;
// P2 audit fix (2026-08-13): set_schema_overrides 单 tier 候选字段数上限
const COUNT_CANDIDATES_MAX: usize = 64;

#[tauri::command]
pub async fn save_config(
    state: State<'_, AppState>,
    app: AppHandle,
    cfg: AppConfig,
) -> Result<(), String> {
    if cfg.providers.len() > PROVIDERS_MAP_MAX {
        return Err(t!(
            "commands.providers_too_many",
            count = cfg.providers.len(),
            max = PROVIDERS_MAP_MAX
        )
        .into_owned());
    }
    // D4-009 fix (2026-07-30 audit): 之前只挡 providers 数, 不挡
    // provider_order Vec / schema_overrides BTreeMap 数量。攻击者灌
    // 100k 空 entry → serde_json 序列化 + 写盘都慢, 无意义。
    if cfg.provider_order.len() > ORDER_LIST_MAX {
        return Err(format!(
            "commands.provider_order_too_many: count={} max={}",
            cfg.provider_order.len(),
            ORDER_LIST_MAX
        ));
    }
    if cfg.schema_overrides.len() > SCHEMA_OVERRIDES_MAX {
        return Err(format!(
            "commands.schema_overrides_too_many: count={} max={}",
            cfg.schema_overrides.len(),
            SCHEMA_OVERRIDES_MAX
        ));
    }
    // P2 audit fix (2026-08-13): set_app_locale 白名单 (zh-CN / en) 在
    // save_config 全量路径被绕过 —— 非法 locale 落盘后, 下次启动
    // rust_i18n::set_locale 吃到未知 locale, 全部 t!() 回退原始键, 界面
    // 显示 key 名而不是文案。
    if !matches!(cfg.locale.as_str(), "zh-CN" | "en") {
        return Err(format!(
            "unsupported locale: {}（仅支持 zh-CN / en）",
            cfg.locale
        ));
    }
    // L2 fix (2026-07-30 audit): 上限 1 天,挡住 webhook 入口塞 86400 * 365
    // 把轮询当 background daemon 跑的死循环。前端 settings panel 默认 60s。
    if cfg.refresh_interval_secs < 10 {
        return Err(t!("commands.interval_too_small").into_owned());
    }
    if cfg.refresh_interval_secs > 86_400 {
        return Err(t!(
            "commands.interval_too_large",
            value = cfg.refresh_interval_secs
        )
        .into_owned());
    }
    // 校验色阈值（settings 面板的保存路径也要兜底 —— 即使用户绕过 set_display_thresholds
    // 直接调 save_config 也会在这里被挡）
    let [t0, t1, t2] = cfg.color_thresholds;
    if !(0 < t0 && t0 < t1 && t1 < t2 && t2 < 100) {
        return Err(t!("commands.threshold_invalid", t0 = t0, t1 = t1, t2 = t2).into_owned());
    }
    if let Some(n) = cfg.wallet_alert_threshold {
        if !(n.is_finite() && n >= 0.0) {
            return Err(t!("commands.wallet_threshold_negative", n = n).into_owned());
        }
    }
    // D4-008 fix (2026-07-30 audit): save_config 之前不校验浮窗坐标,
    // 负数 / 极大值 (例如 IntMax) 会让 position_is_visible 永久 false,
    // 下次启动浮窗不在可见区,用户看到"浮窗不见了"。补上和 lib.rs:542
    // 同样的 bounds 检查:xy 在 ±100k (覆盖极端多屏 / 副屏在主屏左侧的负偏移),
    // wh 在 50~4000 (可见窗口范围, 浮窗实际不会超过 2000 像素)。
    // None = 用默认值, 放行。
    const COORD_MIN: i32 = -100_000;
    const COORD_MAX: i32 = 100_000;
    const DIM_MIN: i32 = 50;
    const DIM_MAX: i32 = 4000;
    for (name, val) in [
        ("floating_x", cfg.floating_x),
        ("floating_y", cfg.floating_y),
    ] {
        if let Some(v) = val {
            if !(COORD_MIN <= v && v <= COORD_MAX) {
                return Err(format!(
                    "commands.coord_out_of_range: {name}={v} 不在 [{COORD_MIN}, {COORD_MAX}] 范围,                      极端值会让 position_is_visible 永久 false 导致浮窗不可见"
                ));
            }
        }
    }
    for (name, val) in [
        ("floating_w", cfg.floating_w),
        ("floating_h", cfg.floating_h),
    ] {
        if let Some(v) = val {
            if !(DIM_MIN <= v && v <= DIM_MAX) {
                return Err(format!(
                    "commands.dim_out_of_range: {name}={v} 不在 [{DIM_MIN}, {DIM_MAX}] 范围,                      极端值会让浮窗渲染异常"
                ));
            }
        }
    }
    // 校验自定义色（同 set_display_thresholds 路径）
    for (k, v) in &cfg.color_overrides {
        match k.as_str() {
            "ok" | "cyan" | "warn" | "alert" => {}
            other => {
                return Err(t!("commands.color_key_unknown", other = other).into_owned());
            }
        }
        if !is_valid_hex_color(v) {
            return Err(t!(
                "commands.color_value_invalid",
                k = k.as_str(),
                v = v.as_str()
            )
            .into_owned());
        }
    }
    // H2 fix: 先更 in-memory state，再 save + 副作用。
    // 原顺序 cfg.save() → autostart → emit → *guard = cfg，进程若在 save 与 guard 写之间
    // crash，盘上是新值、内存是旧值，下次启动加载新值，但 run-time 一致性已坏。
    // 现在 in-memory 永远是真相之源，磁盘 + 平台副作用最后再 commit。
    {
        let mut guard = state.config.write().await;
        *guard = cfg.clone();
    }

    // M2 fix: 先 save disk，再做 OS 副作用。
    // 之前 autostart toggle 在 save 之前执行，cfg.save() 失败时 OS autostart
    // 已经切换但 disk 没更新 → 下次启动读到旧值，OS 状态与 disk 不一致。
    // 改为先 save 成功再做副作用（与 set_auto_hide_in_fullscreen:1286-1301 风格一致）。
    cfg.save()?;

    // 同步 autostart
    let mgr = app.autolaunch();
    if cfg.autostart {
        if let Err(e) = mgr.enable() {
            tracing::warn!(error = %e, "autostart enable 失败 (disk 已保存)");
        }
    } else {
        if let Err(e) = mgr.disable() {
            tracing::warn!(error = %e, "autostart disable 失败 (disk 已保存)");
        }
    }

    // 同步「全屏自动隐藏」开关到平台层（watcher 始终运行，这里翻原子开关）
    // 注：platform::set_auto_hide_in_fullscreen 当前是 infallible swap；future 如果加
    // 真可能失败的 platform call，再改成 Result<(), String> 传播
    crate::platform::set_auto_hide_in_fullscreen(&app, cfg.auto_hide_in_fullscreen);

    // 广播省电模式给浮窗，让前端 toggle body[data-low-power]
    // 失败 log warn 但不阻断（emit 失败不应让 user 重试整个 save_config）
    if let Err(e) = app.emit("musage://low-power-mode-changed", cfg.low_power_mode) {
        tracing::warn!(error = %e, "emit low-power-mode-changed 失败");
    }

    // 广播「配置变了」给浮窗，让浮窗按需 re-fetch（比如 Tavily 简洁模式开关）
    if let Err(e) = app.emit("musage://config-changed", ()) {
        tracing::warn!(error = %e, "emit config-changed 失败");
    }
    Ok(())
}

// ── 新 API：按字符串 id 操作（推荐） ──────────────────────────────

/// 注册表元信息：前端拿到后能动态渲染设置面板（避免硬编码 3 个 provider）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceMeta {
    pub id: String,
    pub display_name: String,
    /// "api_key" | "cookie" | "api_key_or_cookie"
    pub auth_kind: &'static str,
    pub enabled: bool,
    /// true = 主面板不渲染凭据字段（移至"高级"tab）。Xiaomi 用：
    /// API key 对 Bearer 永远 401，手动 cookie 是兜底，都放高级 tab。
    #[serde(default)]
    pub hide_credentials: bool,
    /// true = STUB（公开 API 无 quota endpoint，fetch 永远返"未支持"错）。
    /// UI 用这个加灰显 + "未支持" 角标。2026-06-17 commit 加。
    /// 老前端如果没识别这个字段会忽略，对老面板渲染无影响。
    #[serde(default)]
    pub is_stub: bool,
}

/// 列出所有内置 source 的元信息 + 当前启用状态。
#[tauri::command]
pub async fn list_sources(state: State<'_, AppState>) -> Result<Vec<SourceMeta>, String> {
    let cfg = state.config.read().await;
    Ok(builtin_sources()
        .iter()
        .map(|s| SourceMeta {
            id: s.id().to_string(),
            display_name: s.display_name().to_string(),
            auth_kind: match s.auth_kind() {
                AuthKind::ApiKey => "api_key",
                AuthKind::Cookie => "cookie",
                AuthKind::ApiKeyOrCookie => "api_key_or_cookie",
                AuthKind::ApiKeyWithSecret => "api_key_with_secret",
            },
            enabled: cfg.is_enabled_id(s.id().as_ref()),
            // Xiaomi: API key (Bearer) 永远 401，手动 cookie 是兜底 → 都放高级 tab
            hide_credentials: {
                let id = s.id();
                id == "xiaomimimo"
                    || id == "claude_official" // sessionKey 约 8h 过期，不常改
                    || id == "anysearch" // 一键登录 banner 在主面板，cookie textarea 放高级
            },
            is_stub: s.is_stub(),
        })
        .collect())
}

#[tauri::command]
pub async fn has_source_credential(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    // 验证 id 存在（防 IPC 注入任意 key 名）
    let _ = find_source(&state, &id)
        .await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    Ok(config::load_credential_for_id(&id)?.is_some())
}

#[tauri::command]
pub async fn set_source_credential(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
    value: String,
    // 可选：明确指定这个 value 落到哪个字段（"api_key" / "cookie"）。
    // 不传时按 source 的 `auth_kind()` 默认：
    //   ApiKey / ApiKeyOrCookie → api_key
    //   Cookie                   → cookie
    // 多鉴权 source（ApiKeyOrCookie）必须传 field hint，
    // 否则两个输入框都保存到 api_key，cookie 永远落不进去。
    field: Option<String>,
) -> Result<(), String> {
    let src = find_source(&state, &id)
        .await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(t!("commands.credential_empty").into_owned());
    }
    let cred = build_credentials(&src, trimmed, field.as_deref())?;
    // Bug fix (2026-06-25): 对 ApiKeyOrCookie (Xiaomi)，build_credentials
    // 在 field=None 时默认只写 api_key、显式设 cookie=None。如果用户之前
    // 只存了 cookie，这会静默删除已有的 cookie。下面的 merge 在读 keys.json
    // 之前做一次检查，把旧凭据中未被本次 update 触碰的字段保留。
    //
    // merge 策略：build_credentials 返回的 Credentials 中，哪个字段是
    // Some → 本次有意写入；None → 未指定，应从已有凭据中继承（如果存在）。
    let cred = if src.auth_kind() == AuthKind::ApiKeyOrCookie && field.is_none() {
        // L11 fix (2026-07-06 全量审查): unwrap_or(None) 静默吞掉 IO 错误 /
        // parse 错误。当 keys.json 损坏时,ApiKeyOrCookie(xiaomimimo) 用户的
        // 已有 cookie / api_key 一侧会消失。改为:Err → 走 no-merge 分支
        // + log warn,保留本次 build_credentials 的值。
        let existing = match config::load_credential_for_id(&id) {
            Ok(opt) => opt,
            Err(e) => {
                tracing::warn!(error = %e, id = %id, "load 旧凭据失败,本次保存按 fresh 处理");
                None
            }
        };
        match existing {
            Some(old) => Credentials {
                api_key: cred.api_key.or(old.api_key),
                cookie: cred.cookie.or(old.cookie),
                // v0.2.5: ApiKeyOrCookie merge 块补 secret_key 字段。
                // 实际上火山 (ApiKeyWithSecret) 不走这段(只 ApiKeyOrCookie 走),
                // 但字面量必须补全字段,否则编译错。这里对 ApiKeyWithSecret
                // 是无害 no-op:or(old.secret_key) 在 ApiKeyOrCookie 路径上
                // old.secret_key 永远是 None(老用户没存过 secret_key 槽)。
                secret_key: cred.secret_key.or(old.secret_key),
            },
            None => cred,
        }
    } else {
        cred
    };
    config::save_credential_for_id(&id, &cred)?;
    tracing::debug!(provider = %id, field = ?field, "set_source_credential: saved to keys.json");
    // 关键：用户刚配完 key 浮窗应当立刻看到数据。per-provider 调度最早
    // 在下一分钟才 fire（启动时初始化为 now+interval），不手动拉一次用户得
    // 等 1 分钟甚至更久。refresh_single_inner 内部会更新 in-memory
    // snapshot + emit，浮窗自动跟着变。
    let enabled = state.config.read().await.is_enabled_id(&id);
    tracing::debug!(provider = %id, enabled, "set_source_credential: refresh decision");
    if enabled {
        // CM11 fix (2026-07-28 审查): 之前在这里 await refresh_single_inner,
        // HTTP fetch 2-5s 全程阻塞 IPC(设置面板保存 key 按钮一直转圈)。
        // 对齐 set_provider_enabled / set_xiaomi_display_mode 的 spawn 模式:
        // 立即返回;fetch 完成后 refresh_single_inner 自己会 emit snapshot,
        // 浮窗照常更新。
        let app_clone = app.clone();
        let id_owned = id.clone();
        tokio::spawn(async move {
            if let Err(e) = refresh_single_inner(
                &app_clone,
                &id_owned,
                crate::poller_backoff::RefreshSource::Manual,
            )
            .await
            {
                tracing::warn!(error = %e, provider = %id_owned, "set_source_credential 后立即拉取失败（不阻塞保存）");
            }
        });
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 把 "value 落到 Credentials 哪个字段" 这条规则集中到一处。
///
/// `field` 取值：
/// - `Some("api_key")` / `Some("cookie")` → 强制指定
/// - `None` → 按 source 的 auth_kind 默认
/// - `Some(其他)` → 报错（避免 typo 默默走错字段）
// clippy::borrowed_box 对 dyn trait object 是 false positive: 调用方传
// &Box<dyn QuotaSource>,改签名 &dyn 后 &box 不自动 deref coerce 到 &dyn
// (E0277) -- dyn 的 unsized coercion 走不通。保留 &Box 签名。
#[allow(clippy::borrowed_box)]
fn build_credentials(
    src: &Box<dyn QuotaSource>,
    value: &str,
    field: Option<&str>,
) -> Result<Credentials, String> {
    let target = match field {
        Some("api_key") => "api_key",
        Some("cookie") => "cookie",
        // v0.2.5: 火山方舟 Coding Plan 第二字段。跟 ccswitch 的 `getCodingPlanQuota`
        // 一致：AK + SK 两个独立 secret 各自走自己的 setSourceCredential 调用。
        Some("secret_key") => "secret_key",
        Some(other) => return Err(t!("commands.field_unknown", other = other).into_owned()),
        None => match src.auth_kind() {
            AuthKind::ApiKey | AuthKind::ApiKeyOrCookie => "api_key",
            AuthKind::Cookie => "cookie",
            // v0.2.5: 双字段 AuthKind 走前端显式传 field（saveVolcengineTwoFields
            // 调两次 setSourceCredential 显式带 "api_key" / "secret_key"），
            // 不走 None 默认分支。给个保守 fallback = api_key（安全方向：
            // 写错也只是把 SK 写到 api_key 槽，fetch 时签名失败 401，提示明确）。
            AuthKind::ApiKeyWithSecret => "api_key",
        },
    };
    Ok(match target {
        "api_key" => Credentials {
            api_key: Some(value.to_string()),
            cookie: None,
            secret_key: None,
        },
        "cookie" => Credentials {
            api_key: None,
            cookie: Some(value.to_string()),
            secret_key: None,
        },
        "secret_key" => Credentials {
            api_key: None,
            cookie: None,
            secret_key: Some(value.to_string()),
        },
        _ => unreachable!(),
    })
}

#[tauri::command]
pub async fn delete_source_credential(
    state: State<'_, AppState>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let _ = find_source(&state, &id)
        .await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    config::delete_credential_for_id(&id)?;
    // H4 fix (2026-07-06 全量审查): 删 builtin key 时,扫描 extra_instances
    // 找到同一 provider 的副本,disable 它们 + 清掉对应 keys.json entry,
    // 避免孤儿元数据(浮窗显示"未配置"的死副本)。
    // 删 extra instance id 自身(如 "minimax#2")时不级联 —— #2 可能单独
    // 没 key 也能保留给未来用户配置;只对 builtin 删做级联。
    if !id.contains('#') {
        let extras = state.extra_instances.read().await;
        let orphan_refs: Vec<String> = extras
            .iter()
            .filter(|e| e.provider_id == id)
            .map(|e| e.api_key_ref.clone())
            .collect();
        drop(extras);
        // CM4 fix (2026-07-28 审查): 兑现上方注释承诺的另一半 —— 级联
        // disable 副本。之前只清 keys.json entry,副本 enabled 仍为 true,
        // poller 继续调度 + 浮窗显示「未配置」死卡。api_key_ref
        // ("minimax#2") 同时是 keys.json key 和 cfg.providers key,disable
        // 是可逆操作(设置面板可重新打开);extra_instances 条目本身的删除
        // 归 delete_extra_instance,这里不越权。
        if !orphan_refs.is_empty() {
            let mut cfg = state.config.write().await;
            for r in &orphan_refs {
                let entry = cfg.providers.entry(r.clone()).or_insert(ProviderConfig {
                    enabled: true,
                    region: None,
                    xiaomi_region: None,
                    refresh_interval_secs: None,
                    xiaomi_display_mode: None,
                });
                entry.enabled = false;
            }
            if let Err(e) = cfg.save() {
                // 级联落盘失败不阻断主流程(keys.json 主删除已成功)
                tracing::warn!(error = %e, "delete 级联 disable 副本落盘失败");
            }
            drop(cfg);
        }
        for r in &orphan_refs {
            if let Err(e) = config::delete_credential_for_id(r) {
                tracing::warn!(error = %e, ref_ = %r, "delete 级联清孤儿 entry 失败");
            }
        }
        if !orphan_refs.is_empty() {
            tracing::info!(
                builder = %id,
                count = orphan_refs.len(),
                "delete_source_credential 级联清理 extra_instances 副本"
            );
        }
    }
    // 跟 set_source_credential 对称：删了 key 浮窗应该立刻看到 "未配置"
    // 错误态，而不是等下一次 poller 周期。
    let enabled = state.config.read().await.is_enabled_id(&id);
    if enabled {
        if let Err(e) =
            refresh_single_inner(&app, &id, crate::poller_backoff::RefreshSource::Manual).await
        {
            tracing::warn!(error = %e, provider = %id, "delete 后立即拉取失败");
        }
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 用于设置面板"复制到剪贴板"按钮。返回值仅一次 IPC 用，不在前端持久化。
#[tauri::command]
pub async fn get_source_credential(
    state: State<'_, AppState>,
    id: String,
    // 可选：明确读哪个字段（"api_key" / "cookie" / "secret_key"）。
    // 不传时按 source 的 `auth_kind()` 默认：
    //   ApiKey / ApiKeyOrCookie / ApiKeyWithSecret → api_key
    //   Cookie → cookie
    // 多字段 source（火山 Coding Plan）必须传 field，否则永远返 api_key。
    // v0.2.5 火山 Coding Plan 需要 "secret_key" 才能拿到 SK。
    field: Option<String>,
) -> Result<Option<String>, String> {
    let _ = find_source(&state, &id)
        .await
        .ok_or_else(|| t!("commands.source_unknown", id = id.as_str()).into_owned())?;
    let cred = config::load_credential_for_id(&id)?;
    let target = match field.as_deref() {
        Some("api_key") => cred.and_then(|c| c.api_key),
        Some("cookie") => cred.and_then(|c| c.cookie),
        Some("secret_key") => cred.and_then(|c| c.secret_key),
        Some(other) => return Err(t!("commands.field_unknown", other = other).into_owned()),
        None => cred.and_then(|c| c.api_key.or(c.cookie)),
    };
    Ok(target)
}

// 旧 enum-based IPC (has_api_key_for / set_api_key_for / delete_api_key_for /
// get_api_key_for / has_cookie_for / set_cookie_for / delete_cookie_for) 已在
// v0.2 (2026-06-22) 删除。前端必须用 set_source_credential / get_source_credential /
// has_source_credential / delete_source_credential (按 string id)。

/// 设置窗 builder —— commands.rs 的 open_settings_window + lib.rs 的首启引导
/// 都走这里，防止两处 builder 配置漂移（之前是 byte-for-byte 复制两份）。
///
/// **Win11 闪白修复（2026-06-11）**：不设 background_color 时，WebView2 surface
/// 在第一帧 HTML/CSS 抵达前是系统默认白色，而 settings.css 的 body 背景是
/// `#1a1c22`，用户看到的是「白窗 → 一帧后变深色 = 闪一下」。`background_color`
/// 在 native 层（窗口 chrome + WebView2 surface）就预先涂成 `#1a1c22`，HTML
/// 还没解析的那几十毫秒里画的就是深色，肉眼无感。注意：Windows 8+ 上
/// `Color` 的 alpha 通道会被 webview 层忽略（见 tauri_utils config 注释），
/// `0xff` 只是给阅读代码的人看的。
pub(crate) fn build_settings_window(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let bg = tauri::webview::Color(0x1a, 0x1c, 0x22, 0xff);
    tauri::WebviewWindowBuilder::new(
        app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title(t!("window.settings").to_string())
    .inner_size(780.0, 680.0)
    .min_inner_size(720.0, 600.0)
    .resizable(true)
    .decorations(true)
    // **任务栏映射**：设置窗才是用户面对的 app 窗口，应该出现在 Win 任务栏
    // （这样 ALT+TAB / 任务栏右键能正常操作，icon 也走 bundle.icon）。
    // 浮窗在 tauri.conf.json 里设了 skipTaskbar:true（小悬浮 overlay 不该
    // 出现在任务栏）—— 两侧必须保持一反一正，否则 Win 用户会看到一个
    // "Musage" 任务栏条目对应错误的窗口。
    .skip_taskbar(false)
    .center()
    .background_color(bg)
    .build()
}

#[tauri::command]
pub async fn open_settings_window(app: AppHandle, section: Option<String>) -> Result<(), String> {
    // v0.2.1 commit 8: section 参数取值 "providers" / "floating" / "app" /
    // "advanced" / "logs" / "about" / "region"。
    // 修复之前 P1 commit `5b976e2` 留的隐藏 bug:前端 \`data-section="advanced"\`
    // 传过来但后端不接收 → "open advanced" 按钮点了只开 settings,不跳 tab。
    if let Some(w) = app.get_webview_window("settings") {
        // Win11 已存在窗口的恢复链：unminimize 必须在 show 之前 ——
        // Win 上 show() 对 minimized 窗口是 no-op（不会自动 SW_RESTORE），
        // 不 unminimize 的话用户最小化设置窗后再从托盘点"设置"会以为
        // 命令死了。set_focus 收尾把窗口拉前台 + 抢焦点。
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    } else {
        build_settings_window(&app)
            .map_err(|e| t!("commands.create_settings", err = e.to_string()).into_owned())?;
    }
    // v0.2.1 commit 8: 跳 section。settings.ts init() 监听这个事件,
    // 找到对应 .nav-item 调 .click()。
    if let Some(s) = section.as_deref() {
        // 短暂 sleep 等 settings webview 起来(首次创建窗口时)
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let _ = app.emit("musage://settings-navigate", s);
    }
    Ok(())
}

#[tauri::command]
pub async fn hide_floating_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("floating") {
        let _ = w.hide();
    }
    Ok(())
}

#[tauri::command]
pub async fn show_floating_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("floating") {
        // 与 open_settings_window 同样的"先 unminimize 再 show"链 —— 即使
        // 浮窗 decorations:false 没有最小化按钮，WIN+M / 命令行也能最小化。
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn hide_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.hide();
    }
    Ok(())
}

/// 浮窗归位到主屏幕正中央，并把位置持久化。
#[tauri::command]
pub async fn reset_floating_window(app: AppHandle) -> Result<(), String> {
    let win = app
        .get_webview_window("floating")
        .ok_or_else(|| t!("commands.floating_not_found").into_owned())?;

    // 优先用 Tauri 内置 center() —— 自己算 monitor 几何的旧实现
    // (commands.rs:209-216 旧版) 有 .max(0) 截断的 bug，多显示器 / 负坐标场景会偏。
    win.center()
        .map_err(|e| t!("commands.center_failed", err = e.to_string()).into_owned())?;

    // 持久化（on_window_event(Moved) 也会触发，但先写一次更稳）
    if let Ok(pos) = win.outer_position() {
        let state = app.state::<crate::AppState>();
        let mut cfg = state.config.write().await;
        cfg.floating_x = Some(pos.x);
        cfg.floating_y = Some(pos.y);
        // P3 audit fix (2026-08-13): save 失败之前静默吞 -> 用户改了浮窗
        // 位置/置顶模式但磁盘没更新, 下次启动回退, 无日志可查。补 warn
        // (不返 Err -- IPC 已向前端返 Ok, 内存状态最新, 下次成功 save 覆盖)。
        if let Err(e) = cfg.save() {
            tracing::warn!(error = %e, "config save 失败 (内存状态已更新, 下次成功 save 会覆盖)");
        }
    }
    Ok(())
}

/// 退出前 drain 等待时间。
///
/// 退出时调用 `crate::poller::SHUTDOWN.notify_waiters()` 后, 当前 tokio task
/// sleep 这个时间让 poller 主循环跑完 in-flight fetch + cleanup。**这个值
/// 必须远小于 `poller_backoff::MAX_BACKOFF_SECS` (30min)** —— 否则用户点
/// quit 后最长要等 30min 进程才退, 实际只需要 <500ms。
const POLLER_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);

#[tauri::command]
pub async fn quit_app(app: AppHandle) {
    // H1 fix (2026-07-30 audit): 之前 app.exit(0) 直接终止,per-provider
    // in-flight fetch + JoinSet task 全丢日志(可能 panic 但不致命),用户
    // 偶发看到 "poller 主循环...task panicked" 启动日志告警。改为:
    //   1) notify shutdown signal,让 poller 主循环走 drain 路径
    //   2) 短暂 yield 一次让它真有机会跑完
    //   3) 才 app.exit(0)
    //
    // 2026-08-05 审查交叉验证修复: 先置 SHUTDOWN_REQUESTED, 再 notify_waiters.
    // notify_waiters 只唤醒当前已注册的 notified() future -- 若主循环正在
    // loop body 里 (无 notified() 注册), 通知会丢. SHUTDOWN_REQUESTED AtomicBool
    // 是兜底, 主循环 select! 退出后必查, 保证不漏 shutdown.
    crate::poller::SHUTDOWN_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::poller::SHUTDOWN.notify_waiters();
    // D5-102 fix (2026-07-30 audit): OS 线程 (Win hover emitter / macOS
    // 全屏监听) 用 std::thread::spawn, 不能 await tokio Notify。设
    // SHUTDOWN_NATIVE_THREADS atomic, OS 线程每个 tick 检查一次退出。
    crate::poller::SHUTDOWN_NATIVE_THREADS.store(true, std::sync::atomic::Ordering::SeqCst);
    // 让出当前 task 让 poller 主循环调度起来跑 drain (通常 <100ms 完成,
    // 500ms 留 buffer 应对最坏情况)
    tokio::time::sleep(POLLER_DRAIN_TIMEOUT).await;
    app.exit(0);
}

#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
pub async fn set_floating_pin_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    mode: String,
) -> Result<(), String> {
    let parsed = parse_pin_mode(&mode)?;
    apply_pin_mode_to_window(&app, parsed);
    {
        let mut cfg = state.config.write().await;
        if cfg.floating_pin_mode != parsed {
            cfg.floating_pin_mode = parsed;
            // P3 audit fix (2026-08-13): save 失败之前静默吞 -> 用户改了浮窗
            // 位置/置顶模式但磁盘没更新, 下次启动回退, 无日志可查。补 warn
            // (不返 Err -- IPC 已向前端返 Ok, 内存状态最新, 下次成功 save 覆盖)。
            if let Err(e) = cfg.save() {
                tracing::warn!(error = %e, "config save 失败 (内存状态已更新, 下次成功 save 会覆盖)");
            }
        }
    }
    let _ = app.emit("musage://pin-mode-changed", &parsed);
    Ok(())
}

#[tauri::command]
pub async fn set_floating_hover_raise(
    state: State<'_, AppState>,
    app: AppHandle,
    hovering: bool,
) -> Result<(), String> {
    let mode = {
        let cfg = state.config.read().await;
        cfg.floating_pin_mode
    };
    if mode != FloatingPinMode::PinBottom {
        return Ok(());
    }
    crate::platform::set_window_hover_raise(&app, hovering);
    Ok(())
}

fn parse_pin_mode(s: &str) -> Result<FloatingPinMode, String> {
    match s {
        "pin_top" | "PinTop" => Ok(FloatingPinMode::PinTop),
        "pin_bottom" | "PinBottom" => Ok(FloatingPinMode::PinBottom),
        "normal" | "Normal" => Ok(FloatingPinMode::Normal),
        other => Err(t!("commands.pin_mode_unknown", other = other).into_owned()),
    }
}

/// 调整浮窗高度以适配内容（前端在 render 后调用）。
///
/// 浮窗默认 height=100，多 provider 全堆一起会装不下 —— 用户手动拉能拉
/// 一点但 maxHeight 也会卡。改用这个 command 在每次 render 后把窗口
/// resize 到内容实际需要的高度（限在 tauri.conf.json 的 minHeight=100 /
/// maxHeight=2400 范围内）。auto-resize 跟手拉并存：手拉的尺寸会被 debounced
/// 写盘，但下一次 render 又会贴内容。H5。
///
/// **maxHeight 为什么是 2400**：8+ provider 全开（旧上限 800 装不下 → 用户
/// 反馈底部卡片被截）。2400 logical 像素覆盖到 4K 工作区（2160p ≈ 2000+ 可用）。
/// 真正"别超出屏幕"的兜底由前端 `screen.availHeight - 80` 处理 —— 后端这层只
/// 是 OS 硬上限的镜像，避免 Tauri 把窗口拉到天文数字。
///
/// **`height` 是 logical / CSS 像素**（前端读 `app.scrollHeight` 拿到的就是
/// 这个单位）。Tauri 2 在 macOS / Win / Linux 各自对 `set_size(LogicalSize)`
/// 的处理一致 —— 内部转物理像素，避免前端用 `scale_factor` 手算带来的舍入误差。
/// 之前的 `set_size(PhysicalSize::new(w, height*scale))` 在 Retina 上若 scale
/// 算错就会比预期高 1px，再叠加前端的 +1 就会造成 [H5 静置越长越高] 的反馈环。
#[tauri::command]
pub async fn resize_floating_window(app: AppHandle, height: f64) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("floating") {
        // 保留用户当前的宽度（auto-resize 只调高度，不动宽 —— 宽度由用户拖控制）
        // 用 inner_size 的 logical 版本，绕开 macOS 上 outer/inner 的细微差
        let cur_logical: tauri::LogicalSize<f64> = w
            .inner_size()
            .map_err(|e| t!("commands.size_failed", err = e.to_string()).into_owned())?
            .to_logical(w.scale_factor().unwrap_or(1.0));
        let width = cur_logical.width;
        // H16 fix (2026-07-03 audit): NaN.clamp(100.0, 2400.0) 返 NaN(Rust 文档明确),
        // set_size 收到 NaN 行为未定义可能 panic。前端 scrollHeight 在 DOM 未渲染 /
        // display:none 时可能传 NaN。±Infinity 会被 clamp 正确处理,但 NaN 不会。
        if !height.is_finite() {
            tracing::warn!(height, "resize_floating_window 收到非有限值,跳过");
            return Ok(());
        }
        // 限高 —— 必须与 tauri.conf.json 的 minHeight/maxHeight 同步，否则
        // Tauri 会把后端 set_size 拽回 conf 设的范围 → "前端给 1500 但窗口还是 800"。
        // 真正"别超出 monitor 工作区"由前端 `screen.availHeight` 兜底。
        let height = height.clamp(100.0, 2400.0);
        let _ = w.set_size(tauri::LogicalSize::new(width, height));
    }
    Ok(())
}

pub fn apply_pin_mode_to_window(app: &AppHandle, mode: FloatingPinMode) {
    match mode {
        FloatingPinMode::PinTop => crate::platform::set_window_pin_top(app),
        FloatingPinMode::PinBottom => crate::platform::set_window_pin_bottom(app),
        FloatingPinMode::Normal => crate::platform::set_window_normal(app),
    }
}

/// P2 区域向导：用户选定区域后 apply 该区域的默认 provider 顺序 + 默认
/// endpoint（MiniMax/Zhipu CN/EN），并把 user_region 标为 Custom
/// （之后用户手动改顺序/endpoint 不会触发 wizard 重新弹出）。
#[tauri::command]
pub async fn set_region(
    state: State<'_, AppState>,
    app: AppHandle,
    region: String,
) -> Result<(), String> {
    let parsed = match region.as_str() {
        "cn" => UserRegion::Cn,
        "global" => UserRegion::Global,
        "custom" => UserRegion::Custom,
        other => return Err(t!("commands.region_invalid", other = other).into_owned()),
    };

    let default_order: Vec<String> = parsed
        .default_provider_order()
        .iter()
        .map(|s| s.to_string())
        .collect();

    {
        let mut cfg = state.config.write().await;
        // 1. apply 默认 provider 顺序（仅在当前是 default empty 时覆盖）
        if cfg.provider_order.is_empty() {
            cfg.provider_order = default_order;
        }
        // 2. apply 默认 endpoint（MiniMax / Zhipu 都跟 region 走）。
        // H10 fix (2026-07-28 审查): 之前只切 MiniMax —— 旧注释说 zhipu
        // "缺独立 field" 已过时,cfg.zhipu_region 顶层字段早已存在
        // (zhipu.rs set_state 读它,取值 "cn"/"en",同 set_zhipu_region 的
        // 校验约定),Global 分支漏设导致首启向导选 global 后 Zhipu 仍走 CN。
        if parsed == UserRegion::Global {
            if let Some(mm) = cfg.providers.get_mut("minimax") {
                mm.region = Some(MinimaxRegion::En);
            }
            cfg.zhipu_region = Some("en".to_string());
        } else {
            // Cn (默认) —— 显式归位 CN（zhipu 同 minimax,防先选 global
            // 再切回 cn 时残留 en）
            if let Some(mm) = cfg.providers.get_mut("minimax") {
                mm.region = Some(MinimaxRegion::Cn);
            }
            cfg.zhipu_region = Some("cn".to_string());
        }
        // 3. 标 user_region 为 Custom（之后用户手动改任何字段都不会触发 wizard）
        cfg.user_region = UserRegion::Custom;
        cfg.save()?;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 取当前 user_region（给前端决定是否显示 wizard）
#[tauri::command]
pub async fn get_region(state: State<'_, AppState>) -> Result<String, String> {
    let region = match state.config.read().await.user_region {
        UserRegion::Cn => "cn",
        UserRegion::Global => "global",
        UserRegion::Custom => "custom",
    };
    Ok(region.to_string())
}

// ── 核心：refresh_inner ───────────────────────────────────────

/// 根据当前 backoff 状态填充 `next_fetch_at`（下次自动 fetch 的 epoch ms）。
///
/// 调用方负责: 先写 `backoff.record()` 更新 interval, 再调本函数。
/// 本函数只是读 backoff 的当前 interval + 算时间戳,不写 backoff。
///
/// 用途: 浮窗错误卡片用 `next_fetch_at` 显示 "下次重试 in Xm" 倒计时。
/// 2026-06-17 commit 加。
async fn fill_next_fetch_at(
    app: &AppHandle,
    id: &str,
    default_secs: u64,
    snap: &mut ProviderSnapshot,
) {
    let interval = {
        let state = app.state::<AppState>();
        let backoff = state.backoff.read().await;
        backoff.next_interval_secs(id, default_secs)
    };
    let now = chrono::Utc::now().timestamp_millis();
    snap.next_fetch_at = Some(now + (interval as i64) * 1000);
}

/// 刷新所有启用的 source。**并发**跑，互不拖累。
///
/// 被 [`refresh_now`] 和 [`crate::poller::tick`] 共用。
///
/// Phase 1：每个 source 自己负责鉴权和 fetch，commands.rs 不再 `match provider`。
pub async fn refresh_inner(
    app: &AppHandle,
    cfg: &AppConfig,
    caller: RefreshSource,
) -> Result<QuotaSnapshot, String> {
    // H1: builtin_sources() 不含 custom sources。refresh_inner 必须走 all_sources
    // 才能让用户添加的 New API 中转站出现在全量刷新里。lock 顺序:
    // state.config.read() 先拿+释放,再调 all_sources(state) 拿+释放 customs.read(),
    // 不嵌套,无 deadlock 风险。
    let state = app.state::<AppState>();
    let sources = all_sources(&state).await;
    // P1 重构：closure 返 FetchError 而不是 String，kind 在 collect 时直接拿。
    #[allow(clippy::type_complexity)]
    let mut tasks: Vec<(
        String,
        u64,
        tokio::task::JoinHandle<Result<ProviderSnapshot, FetchError>>,
    )> = Vec::new();

    for src in &sources {
        let id = src.unique_id(); // extra instance fix：必须用 unique_id() 而不是 id()
        let id_str = id.as_str(); // "deepseek#2" 而非 "deepseek"
                                  // id() 仍用在 enabled / credential 查找前做 enabled check ——
                                  // enabled 状态按 api_key_ref("deepseek#2") 查 config。
        if !cfg.is_enabled_id(id_str) {
            continue;
        }
        // 默认间隔（per-provider override 优先）—— backoff 写入时用。
        // extra instance 优先按 unique_id 查，否则 fallback 到 base id。
        // P1 audit fix (2026-08-13): interval 来自用户可改 config, 用 poller
        // 同款 clamp (10..86400) —— u64::MAX 会让 `(as i64)*1000` 溢出成
        // 负数, next_fetch_at 落到 1970 / 倒计时负数。
        let base_id = src.id(); // Cow<'_, str>
        let default_interval_secs = crate::poller::clamp_interval_secs(
            cfg.providers
                .get(id_str)
                .or_else(|| cfg.providers.get(base_id.as_ref()))
                .and_then(|p| p.refresh_interval_secs)
                .unwrap_or(cfg.refresh_interval_secs),
        );

        // 1. 同步加载凭据（避免在 tokio::spawn 里 await I/O）。
        // extra instance 按 unique_id 查 credential（"deepseek#2" → keys.json 里的 key）。
        let creds_res = config::load_credential_for_id(id_str);
        tracing::trace!(provider = %id_str, has_creds = creds_res.as_ref().ok().and_then(|c| c.as_ref()).is_some(), "refresh_inner load_credential");

        match creds_res {
            Ok(Some(creds)) => {
                let id_owned = id.to_string();
                // 每次 fetch 都重新构造 source 实例 —— builtin_sources() 内部
                // 走 `Box::new(XxxSource::default())`，每次都产生**全新**的
                // `Arc<RwLock<state>>`，跟外层 `src` 的 state 不是同一份。
                // 所以 set_state 必须推给真正用于 fetch 的 `src_box`，而不是
                // 循环变量 `src`（早期代码注释误以为"内部 state 是 Arc<RwLock
                // 共享的"，实际不共享 —— 症状：用户在设置面板切到 Xiaomi
                // 显示模式 "all" 后保存，托盘右键"立即刷新"又把模式拉回默认
                // "total_only"，因为 fetch 用的是新建 src_box 的默认空 state）。
                //
                // H1: 必须用 find_source(state, id) 而不是 builtin_sources().find(),
                // 否则 custom_<uuid> 在这里 expect("source still registered") 会 panic。
                // M1 fix: expect 改 ok_or_else —— concurrent delete_custom_source
                // 可能在 all_sources() 和 find_source() 之间把 source 删掉,
                // expect 会 panic 但 ok_or_else 只跳过这个 source。
                let src_box: Box<dyn QuotaSource> = match find_source(&state, &id).await {
                    Some(src) => src,
                    None => {
                        tracing::warn!(provider = %id, "source 在 all_sources() 后被并发删除,跳过");
                        continue;
                    }
                };
                update_source_state(&src_box, cfg).await;
                // P1 重构：返回 FetchError 而不是 String，kind 在 closure 内就
                // 保留住，collect 时直接 e.kind 拿（不再走 classify_error_message）。
                // v0.2.1 commit 3: src_box 在 spawn 前算 unique_id 字符串,闭包
                // 内部 move 进 fetch 结果的 snapshot.unique_id 字段。多 instance
                // 时返 "minimax#2" 之类;老 fallback 走 source_id。
                let unique_id_str = src_box.unique_id();
                let task: tokio::task::JoinHandle<Result<ProviderSnapshot, FetchError>> =
                    tokio::spawn(async move {
                        let result = src_box.fetch(&creds).await;
                        match result {
                            Ok(mut s) => {
                                s.unique_id = Some(unique_id_str);
                                Ok(s)
                            }
                            Err(e) => Err(e),
                        }
                    });
                tasks.push((id_owned, default_interval_secs, task));
            }
            Ok(None) => {
                let id_owned = id.to_string();
                let task = tokio::spawn(async move {
                    Err(FetchError::unconfigured("未配置凭据（设置面板填入）"))
                });
                tasks.push((id_owned, default_interval_secs, task));
            }
            Err(e) => {
                let id_owned = id.to_string();
                let task = tokio::spawn(async move {
                    // 读 keys.json 失败归到 Network（IO 错误类），不归到 Other
                    // 让前端能正确分类显示
                    Err(FetchError::network(
                        t!("error.common.read_keys_failed", err = e.to_string()).into_owned(),
                    ))
                });
                tasks.push((id_owned, default_interval_secs, task));
            }
        }
    }

    // D5-007 fix (2026-07-30 audit): 之前 for 循环里每条 provider 各拿一次
    // backoff.write().await (12 个 source → 12 次写锁串行排队), 和 poller.rs:172-180
    // 注释里"write 锁不能跨 for 持有"的设计意图冲突。改成 Phase 1 收集 →
    // Phase 2 单次写锁 → Phase 3 多次读锁(fill_next_fetch_at)三段式:
    // - Phase 1: 等所有 task 落地, 收集 Rec 条目, 不持任何 backoff 锁
    // - Phase 2: 一次性 backoff.write(), for 循环逐条 record (record 是 O(1) HashMap 操作,
    //   12 条串行 in-lock 总耗时 <1ms, 远比 12 次 lock acquire/release 省)
    // - Phase 3: drop 写锁后, 每条 fill_next_fetch_at 单独拿读锁(读锁不互斥,
    //   且与 record 路径串行隔离, 不会出现"读到的 next_interval_secs 不是
    //   本轮 record 后的最新值"的情况 —— 因 drop 后 fill 才发生)
    struct Rec {
        id: String,
        snap: ProviderSnapshot,
        default_secs: u64,
        // join_err 走 Other, 不参与 backoff, 但保持调 record 以维持代码对称;
        // next_fetch_at 直接用默认间隔(不走 fill_next_fetch_at 读 backoff)
        is_join_err: bool,
    }
    let mut snap = QuotaSnapshot::default();
    let mut recs: Vec<Rec> = Vec::with_capacity(tasks.len());
    // P2 audit fix (2026-08-13): 之前直接 task.await 无超时 —— 单个 provider
    // 挂起 (deepseek/stepfun 曾实测挂 30s+) 会把 refresh_now / poller tick
    // 整体阻塞到挂起时长, UI 一直转圈。与 dump CLI (lib.rs) 的 30s 超时对齐。
    const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
    for (id, default_interval_secs, task) in tasks {
        match tokio::time::timeout(FETCH_TIMEOUT, task).await {
            Ok(Ok(Ok(s))) => recs.push(Rec {
                id,
                snap: s,
                default_secs: default_interval_secs,
                is_join_err: false,
            }),
            Ok(Ok(Err(e))) => {
                // P1 重构:kind 直接从 FetchError 取,不再走 classify_error_message
                // 子串匹配(旧实现 i18n 一动就破)。
                log_provider_error(app, &id, e.kind, &e.message);
                let err_snap = ProviderSnapshot::empty_error(
                    &app.state::<AppState>(),
                    &id,
                    e.kind,
                    e.message,
                    false, // L8: 真实错误,非 transient
                )
                .await;
                recs.push(Rec {
                    id,
                    snap: err_snap,
                    default_secs: default_interval_secs,
                    is_join_err: false,
                });
            }
            Ok(Err(join_err)) => {
                let msg =
                    t!("error.common.join_task_failed", err = join_err.to_string()).into_owned();
                log_provider_error(app, &id, ErrorKind::Other, &msg);
                let err_snap = ProviderSnapshot::empty_error(
                    &app.state::<AppState>(),
                    &id,
                    ErrorKind::Other,
                    msg,
                    false, // L8: 真实错误
                )
                .await;
                recs.push(Rec {
                    id,
                    snap: err_snap,
                    default_secs: default_interval_secs,
                    is_join_err: true,
                });
            }
            Err(_elapsed) => {
                // P2 audit fix: 超时 provider 记错误卡, 不阻塞其余结果
                let msg = t!(
                    "error.common.fetch_timeout",
                    provider = id.as_str(),
                    secs = 30
                )
                .into_owned();
                log_provider_error(app, &id, ErrorKind::Network, &msg);
                let err_snap = ProviderSnapshot::empty_error(
                    &app.state::<AppState>(),
                    &id,
                    ErrorKind::Network,
                    msg,
                    false,
                )
                .await;
                recs.push(Rec {
                    id,
                    snap: err_snap,
                    default_secs: default_interval_secs,
                    is_join_err: false,
                });
            }
        }
    }

    // Phase 2: 单次写锁,逐条 record。drop 锁后再做 fill_next_fetch_at
    {
        let state = app.state::<AppState>();
        let mut backoff = state.backoff.write().await;
        for rec in &recs {
            backoff.record(&rec.id, &rec.snap, rec.default_secs, caller);
        }
    }

    // Phase 3: 填 next_fetch_at。join_err 走默认间隔(不查 backoff), 其余读 backoff
    for mut rec in recs {
        if rec.is_join_err {
            rec.snap.next_fetch_at =
                Some(chrono::Utc::now().timestamp_millis() + (rec.default_secs as i64) * 1000);
        } else {
            fill_next_fetch_at(app, &rec.id, rec.default_secs, &mut rec.snap).await;
        }
        snap.providers.push(rec.snap);
    }

    snap.fetched_at = Some(chrono::Utc::now().timestamp_millis());

    // 过滤 + 排序 (filter 必须在 publish 前: 用户禁用 provider 不应 emit)
    let state = app.state::<AppState>();
    let cfg_read = state.config.read().await;
    snap.providers.retain(|p| {
        // P3 fix (2026-07-28 审查): 统一 snapshot_key 规则(unique_id 优先)。
        let id = snapshot_key(p);
        cfg_read.is_enabled_id(id)
    });
    apply_provider_order(&mut snap, &cfg_read);
    // 把全局余额告警阈值带到 snapshot —— health_label 据此翻红/翻黄
    snap.wallet_alert_threshold = cfg_read.wallet_alert_threshold;
    drop(cfg_read);

    // D5-038 fix (2026-07-30 audit): display_name + emit + tray 三步封装
    // 到 publish_snapshot helper, refresh_single 路径共用同款 (2026-06-25 i18n fix)。
    publish_snapshot(app, &state, &mut snap).await;

    Ok(snap)
}

/// P3 fix (2026-07-28 审查): snapshot 条目身份键的统一规则 ——
/// `unique_id` 优先,fallback `source_id`,最后 `provider` 兼容字段。
///
/// 为什么必须 unique_id 优先:provider 的 do_fetch 成功路径把 `source_id`
/// 硬编码为 base id("minimax"),`unique_id` 才由 caller 注入("minimax#2")。
/// 按 source_id 匹配合并时,副本的 snapshot 会命中并覆盖基础实例的条目。
/// tick() / refresh_now / refresh_single_inner 的合并 + 两处的 enabled
/// retain 全部走这一条规则,与 apply_provider_order 的 order_key 口径一致。
pub(crate) fn snapshot_key(p: &ProviderSnapshot) -> &str {
    p.unique_id
        .as_deref()
        .or(p.source_id.as_deref())
        .unwrap_or(&p.provider)
}

/// 按 AppConfig.provider_order 给 snapshot.providers 排序。
/// - provider_order 为空 → 不动（保留 builtin_sources() 注册表顺序）
/// - 非空 → 按用户在设置面板拖拽/上下按钮指定的顺序排
///   不在 order 里的 provider 沉到末尾（usize::MAX）—— 防止用户
///   删掉一个 provider 后剩下的"消失"。
fn apply_provider_order(snap: &mut QuotaSnapshot, cfg: &AppConfig) {
    if cfg.provider_order.is_empty() {
        return;
    }
    // B-NEW-4（2026-06-19 audit）+ extra instance fix（2026-06-25）：
    // 之前只按 source_id 匹配 provider_order，但 extra instance（如
    // "deepseek#2"）的 source_id 现在 = unique_id() → 需同时用 unique_id
    // 匹配。匹配优先级：unique_id → source_id → provider（保证副本和
    // 内置实例都在 provider_order 里找到位置）。
    let mut indexed: Vec<(usize, crate::providers::ProviderSnapshot)> =
        snap.providers.drain(..).enumerate().collect();
    indexed.sort_by(|(ai, a), (bi, b)| {
        let a_order_key = a
            .unique_id
            .as_deref()
            .or(a.source_id.as_deref())
            .unwrap_or(&a.provider);
        let b_order_key = b
            .unique_id
            .as_deref()
            .or(b.source_id.as_deref())
            .unwrap_or(&b.provider);
        // 2026-08-03 audit (Darwin B12): 删冗余 fallback ——
        // a_order_key 已经走 unique_id || source_id || provider 三级 fallback,
        // a.source_id.as_deref().unwrap_or(&a.provider) 是 a_order_key 没匹配上的
        // 边缘场景(legacy provider_order 只有 source_id 而 snap 没 source_id 字段)
        // 实战不存在 (snap.source_id 总是 Some),留 belt-and-suspenders 文档即可
        let apos = cfg
            .provider_order
            .iter()
            .position(|o| o == a_order_key)
            .unwrap_or(usize::MAX);
        let bpos = cfg
            .provider_order
            .iter()
            .position(|o| o == b_order_key)
            .unwrap_or(usize::MAX);
        apos.cmp(&bpos).then(ai.cmp(bi))
    });
    snap.providers = indexed.into_iter().map(|(_, p)| p).collect();
}

/// D5-038 helper (2026-07-30 audit): fill source_display_name + emit snapshot
/// + refresh tray。refresh_inner (全量) 和 refresh_single_inner (per-provider)
/// 两条路径共用, 避免两处漂移 (2026-06-25 i18n fix 的 display_name 策略
/// 跟 emit/tray 顺序耦合, 集中一处)。
///
/// 不做 filter + apply_provider_order (refresh_inner 在外层完成, refresh_single
/// 不需要 — per-provider 路径只有 1 个 provider)。
async fn publish_snapshot(
    app: &AppHandle,
    state: &AppState,
    snap: &mut crate::providers::QuotaSnapshot,
) {
    // ── post-fill source_display_name（2026-06-25 i18n 修复）──────────────
    // 12 个 provider 的 do_fetch / parse 各自硬编码 source_display_name 为静态
    // 字符串（"MiniMax" / "DeepSeek" / ...），跟 QuotaSource::display_name() 脱钩。
    // 逐遍改 12 个 do_fetch 签名风险高且 future provider 仍会再犯。更好的策略：
    // fetch 产出 snapshot 之后，用 find_source(id) 查 display_name() 统一填回。
    // - 副本（"minimax#2"）→ "MiniMax #2"（经 i18n）
    // - 默认（"minimax"）   → "MiniMax"（经 i18n）
    // - CustomSource        → 用户的 display_name（"DMX API"）
    for p in &mut snap.providers {
        let id = p
            .unique_id
            .as_deref()
            .unwrap_or(p.source_id.as_deref().unwrap_or(&p.provider));
        if let Some(src) = crate::providers::find_source(state, id).await {
            p.source_display_name = Some(src.display_name().to_string());
        }
    }
    // 推送给前端 (浮窗 + settings 面板)
    let _ = app.emit("musage://snapshot", &snap);
    // 刷新托盘 (tray_style 从 cfg 读, 浮窗不需要但 tray 渲染需要)
    let (tray_style, tray_source, tray_color) = {
        let cfg = state.config.read().await;
        (
            cfg.tray_icon_style,
            cfg.tray_source.as_deref().unwrap_or("minimax").to_string(),
            crate::tray::tray_fill_color(cfg.tray_icon_color.as_deref()),
        )
    };
    if let Err(e) =
        crate::tray::update_tray_from_snapshot(app, &snap, tray_style, &tray_source, tray_color)
    {
        tracing::warn!(error = %e, "刷新托盘失败 (publish_snapshot)");
    }
}

/// 拉取单个 provider —— 供 poller 的 per-provider 调度使用（H9）。
///
/// 不重新跑全部 enabled source，只跑指定的一个；fetch 完成后
/// 替换 in-memory snapshot 里对应那条，再 emit + 刷新托盘。
/// 这样每个 provider 可以有自己的轮询间隔。
#[tauri::command]
pub async fn refresh_single(app: AppHandle, id: String) -> Result<(), String> {
    refresh_single_inner(&app, &id, crate::poller_backoff::RefreshSource::Manual).await
}

/// H5 fix (2026-07-30 audit): `caller` 区分失败行为 (见 poller_backoff::RefreshSource)。
/// - 全量 refresh / Poller 入口 → Poller(失败退避)
/// - 用户点「立即刷新」、设置面板「单源刷新」、登录完成后刷新 → Manual(失败 no-op)
pub async fn refresh_single_inner(
    app: &AppHandle,
    id: &str,
    caller: crate::poller_backoff::RefreshSource,
) -> Result<(), String> {
    let cfg = app.state::<AppState>().config.read().await.clone();
    if !cfg.is_enabled_id(id) {
        return Ok(()); // 已被关掉，跳过
    }
    // H1: builtin_sources() 不含 custom sources,改用 find_source(state, id)
    // ——这才能让 custom_<uuid> 被单源刷新(add/update_custom_source 后立即拉
    // 第一条数据、set_source_credential 后立即拉数据,都走这条路径)。
    let state = app.state::<AppState>();
    let src = find_source(&state, id)
        .await
        .ok_or_else(|| t!("error.common.unknown_source_id", id = id).into_owned())?;
    // 手动 "立即刷新" 始终拉取 —— 即便是 STUB (用户显式点击,让 fetch 返
    // "未支持" 错就清楚表达 STUB 状态;poller 才按 default_enabled 自动跳过)。
    let creds = config::load_credential_for_id(id)?;
    update_source_state(&src, &cfg).await;
    // v0.2.1 commit 3: 多 instance 区分 —— snapshot.unique_id 由 src 注入。
    let unique_id_str = src.unique_id();
    let mut provider_snap = match creds {
        Some(c) => match src.fetch(&c).await {
            Ok(mut s) => {
                s.unique_id = Some(unique_id_str);
                s
            }
            Err(e) => {
                let kind = e.kind;
                log_provider_error(app, id, kind, &e.message);
                ProviderSnapshot::empty_error(
                    &app.state::<AppState>(),
                    id,
                    kind,
                    e.message,
                    false, // L8: 真实错误
                )
                .await
            }
        },
        None => {
            let kind = ErrorKind::UnconfiguredKey;
            // M6 fix: 把 provider id 拼进错误消息,日志/通知里能一眼看到是
            // 哪个 source 缺 key。之前 t!("error.common.no_credential") 没有
            // {provider} 占位符,用户看到 "未配置凭据" 不知道是哪个 source。
            let msg = t!("error.common.no_credential_with_provider", provider = id).into_owned();
            log_provider_error(app, id, kind, &msg);
            ProviderSnapshot::empty_error(
                &app.state::<AppState>(),
                id,
                kind,
                msg,
                false, // L8: 真实错误(无 credential,持久)
            )
            .await
        }
    };

    // 写 backoff：让 poller 下次调度知道这个 provider 是不是该延长间隔
    // (失败 → 翻倍；成功 → reset)。详见 `poller_backoff::BackoffState::record`。
    //
    // M18 fix (2026-07-28 审查): 查 interval 补 base_id fallback —— 副本
    // ("minimax#2") 在 cfg.providers 里通常没独立 entry,之前直接用全局
    // interval,忽略 base 的 per-provider 覆盖(poller 主循环和
    // refresh_inner 都有 fallback,这里漏了)。
    let default_interval_secs = cfg
        .providers
        .get(id)
        .or_else(|| cfg.providers.get(src.id().as_ref()))
        .and_then(|p| p.refresh_interval_secs)
        .unwrap_or(cfg.refresh_interval_secs)
        .max(10);
    {
        let state = app.state::<AppState>();
        let mut backoff = state.backoff.write().await;
        // 2026-08-06 cross-verify (#4): backoff 槽位键必须用 unique_id,与 poller
        // 读取端(poller.rs 用 src.unique_id())一致。之前用 IPC 入参 `id`(前端
        // main.ts refresh_single 实际传 uniqueId,故当前未触发 bug;但若误传
        // base id 会写错副本槽位 -> 副本永退避 / 永不退避)。显式取 src.unique_id()
        // 杜绝 latent 风险。find_source / is_enabled_id 仍按 base+unique 双匹配。
        let backoff_key = src.unique_id();
        backoff.record(&backoff_key, &provider_snap, default_interval_secs, caller);
    }
    // 填 next_fetch_at(同 refresh_inner 的 fill_next_fetch_at,逻辑共享)
    fill_next_fetch_at(app, id, default_interval_secs, &mut provider_snap).await;

    // 替换 in-memory snapshot 里对应那条。
    // P3 fix (2026-07-28 审查): 匹配规则统一为 snapshot_key(unique_id
    // 优先) —— 之前的 4 条件交叉匹配(match_key/fallback_key ×
    // unique_id/source_id)在"老条目 unique_id=None + 新副本错误 snapshot
    // source_id=base id"场景会跨条目误命中,把副本错误盖到基础实例上。
    let state = app.state::<AppState>();
    let mut snap = state.snapshot.write().await;
    let match_key = snapshot_key(&provider_snap);
    if let Some(idx) = snap
        .providers
        .iter()
        .position(|p| snapshot_key(p) == match_key)
    {
        snap.providers[idx] = provider_snap;
    } else {
        snap.providers.push(provider_snap);
    }
    snap.fetched_at = Some(chrono::Utc::now().timestamp_millis());
    drop(snap);

    // 重新读最新 config（可能用户在两次 fetch 之间改了 enabled/order），
    // 过滤 + 排序后再 emit
    let state = app.state::<AppState>();
    let cfg2 = state.config.read().await;
    let cfg2_snapshot = cfg2.clone();
    drop(cfg2);
    let mut snap = state.snapshot.write().await;
    snap.providers.retain(|p| {
        // P3 fix (2026-07-28 审查): 统一 snapshot_key 规则(unique_id 优先;
        // 之前 source_id 优先,跟合并链的口径相反)。
        cfg2_snapshot.is_enabled_id(snapshot_key(p))
    });
    apply_provider_order(&mut snap, &cfg2_snapshot);
    // 同步全局余额告警阈值(per-provider 调度只更一个 provider,不能丢顶层字段)
    snap.wallet_alert_threshold = cfg2_snapshot.wallet_alert_threshold;
    let emit_snap = snap.clone();
    drop(snap);
    // D5-038 fix (2026-07-30 audit): 走共享 helper, 见 publish_snapshot 注释。
    let state = app.state::<AppState>();
    let mut emit_snap = emit_snap;
    publish_snapshot(app, &state, &mut emit_snap).await;
    Ok(())
}

/// poller per-provider 调度专用入口（P4 fix, 2026-07-28 审查）。
///
/// tick()/refresh_now 的全量刷新在跑时跳过本次 —— 全量刷新本来就会拉到
/// 这个 provider,并发再跑 refresh_single_inner 会让 backoff.record
/// 双倍计数、fetch 量翻倍(TICK_RUNNING 只防 tick vs tick,堵不住
/// tick vs per-provider 这对)。entry 在 spawn 前已推进,跳过不丢数据。
///
/// 手动触发路径(保存 key / 启用开关 / 设置面板各 setter)仍走
/// [`refresh_single_inner`] 原入口,不受此跳过影响 —— 那些是用户动作,
/// 必须立即生效。
pub async fn refresh_single_from_poller(app: &AppHandle, id: &str) -> Result<(), String> {
    if crate::poller::tick_is_running() {
        tracing::debug!(provider = %id, "全量刷新进行中,跳过本次 per-provider 拉取");
        return Ok(());
    }
    refresh_single_inner(app, id, crate::poller_backoff::RefreshSource::Poller).await
}

/// 在 fetch 前把 cfg 里的 region / overrides 推给 source（如果 source 实现了的话）。
///
/// 公开给 [`crate::lib::run_dump_subcommand`] 共享。
#[allow(clippy::borrowed_box)]
pub async fn update_source_state(src: &Box<dyn QuotaSource>, cfg: &AppConfig) {
    // 跳过无状态 source（deepseek / kimi / claude_official）的 set_state，
    // 避免每分钟 × 3 provider × ~2KB JSON 序列化 + alloc + drop 的无意义开销。
    if !src.needs_state_update() {
        return;
    }
    // 把整个 cfg 序列化成 JSON，让 source 自己按需取字段
    let cfg_json = match serde_json::to_value(cfg) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "序列化 AppConfig 失败，跳过 set_state");
            return;
        }
    };
    src.set_state(cfg_json).await;
}

/// 把 provider 抛出的中文错误串映射成 [`ErrorKind`]。
///
/// P1 错误分类重构：删了。
/// 旧实现对中文字符串做子串匹配（鉴权失败 / 网络错误 / ...），i18n 一动
/// （Rust 错误消息改 tr!() 走 en.json）就全破。
/// 现在 refresh_inner closure 直接返回 [`FetchError`]（带 kind），
/// 这里不再需要兜底分类。详见 `refresh_inner` L774 注释。
#[allow(dead_code)]
fn _classify_error_message_removed(_msg: &str) -> ErrorKind {
    // 保留一个占位 stub 防止别处误引用（编译期 dead_code 警告，不影响产物）。
    ErrorKind::Other
}

// ── 日志：错误事件下沉到 LogStore ────────────────────────────────────
//
// 设计要点（commit 3d5ee5d）：
// - refresh_inner 每个失败的 provider 都打一条 LogEntry::error
// - 60s 去重窗口（同 provider + 同 kind）避免长断网刷爆日志
// - 浮窗 UI 此时只翻红点，rowsBox 仍保留最后一次成功的数据
// - 设置面板通过 `get_recent_logs` 拉取查看，`clear_logs` 清空

/// (provider_id, kind_short_label) → 上次写日志的毫秒时间戳。
/// 在 60s 窗口内的同 key 错误被吞掉，不重复写。
fn dedup_cache() -> &'static std::sync::Mutex<std::collections::HashMap<(String, &'static str), i64>>
{
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<(String, &'static str), i64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const LOG_DEDUP_WINDOW_MS: i64 = 60_000;

/// M1 fix (2026-07-08 全量审查): dedup_cache 单条 entry 超过 24h 无活动
/// 就清掉,防止 add/delete extra instance 长期累积导致 entry 永久驻留。
/// 24h 远大于 60s 去重窗口,正常活跃 entry 不会在窗口内被误清。
/// 复用 `now.saturating_sub(*ts)` 处理时钟回拨(now < ts 时视为未过期)。
const LOG_DEDUP_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// 检查 `(provider, kind)` 是否在 `now_ms` 时刻应被 dedup 吞掉 —— 纯函数,
/// 方便单测覆盖;`log_provider_error` 拿锁后调用。
///
/// true = 在 60s 窗口内命中过同 key,应 dedup;false = 允许写新日志。
/// 时钟回拨处理跟 L13 saturating_sub 保持一致:last_ts > now 时视为"很久以前"。
fn is_dedup_window_hit(
    cache: &std::collections::HashMap<(String, &'static str), i64>,
    key: &(String, &'static str),
    now_ms: i64,
) -> bool {
    match cache.get(key) {
        Some(&last_ts) => {
            let delta = if now_ms >= last_ts {
                now_ms - last_ts
            } else {
                i64::MAX
            };
            delta < LOG_DEDUP_WINDOW_MS
        }
        None => false,
    }
}

/// 把一次 provider 拉取失败写进 [`crate::logstore::LogStore`]。
///
/// 同 (provider_id, kind) 在 60s 窗口内只保留第一条，避免长断网刷爆 ring buffer。
/// IO 失败 / mutex 中毒都不阻塞调用方 —— 这是热路径的旁路。
fn log_provider_error(app: &AppHandle, provider_id: &str, kind: ErrorKind, message: &str) {
    let now = chrono::Utc::now().timestamp_millis();
    // H12 宽限期：用户刚点过「清空」日志的 60s 内，所有新错误一律不写
    // —— 让用户真切看到「已清空」状态，不被立刻涌出的新错误淹没。
    if is_in_clear_grace(now) {
        return;
    }
    // P1 重构：用 ErrorKind::as_str()（snake_case）作为 dedup key —— 跟 serde
    // 序列化的形式一致，i18n 切换不会破坏去重窗口。
    let key = (provider_id.to_string(), kind.as_str());

    // 去重判断 + 24h 老化：拿锁尽量短,顺手清过期 entry 避免无限增长
    {
        let mut g = match dedup_cache().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(), // 中毒也继续，日志比强一致重要
        };
        // M1 老化:24h 内无活动的 entry 删掉。共用同一次 lock acquire,O(n) 成本可忽略
        // (n ≤ ~66 builtin + 用户 extras,sub-microsecond)。
        g.retain(|_, ts| now.saturating_sub(*ts) < LOG_DEDUP_TTL_MS);
        if is_dedup_window_hit(&g, &key, now) {
            return;
        }
        g.insert(key, now);
    }

    let state = app.state::<AppState>();
    state.log.push(crate::logstore::LogEntry::error(
        provider_id,
        kind.as_str(),
        message,
    ));

    // v0.2.1 commit 7 (P2-B-8): Xiaomi/Claude cookie 失效弹系统通知。
    // dedup 60s 窗口已经保证一次失败 → 一条通知,poller 60s 一次轮询不会
    // 弹疯 (8h cookie 失效 → 60min 内最多 60 条通知)。其他 provider (minimax 等
    // Bearer key 失败) 不弹,减少干扰。
    //
    // H11 fix (2026-07-28 审查): provider_id 可能是副本 unique_id
    // ("xiaomimimo#2"/"claude_official#2") —— 之前全串 matches! 让副本
    // cookie 过期永远不弹通知;且内层 if 也用全串比较,"xiaomimimo#2"
    // 会错拿 claude 文案。取 base id 再匹配/选文案;body 仍传完整
    // unique_id,通知里能定位到具体副本。
    let base_id = provider_id.split('#').next().unwrap_or(provider_id);
    if matches!(kind, ErrorKind::AuthFailed) && matches!(base_id, "xiaomimimo" | "claude_official")
    {
        let (title_key, body_key) = if base_id == "xiaomimimo" {
            (
                "notification.xiaomi_cookie_expired_title",
                "notification.xiaomi_cookie_expired_body",
            )
        } else {
            (
                "notification.claude_session_expired_title",
                "notification.claude_session_expired_body",
            )
        };
        let title = t!(title_key).to_string();
        let body = t!(body_key, provider = provider_id).to_string();
        let app_for_notif = app.clone();
        // 异步 fire-and-forget:通知失败也不影响主流程
        tauri::async_runtime::spawn(async move {
            use tauri_plugin_notification::NotificationExt;
            if let Err(e) = app_for_notif
                .notification()
                .builder()
                .title(title)
                .body(body)
                .show()
            {
                tracing::warn!(error = %e, "系统通知发送失败");
            }
        });
    }
}

// ── 设置面板"即时生效"command 群 ──────────────────────────────────
//
// 设置面板"勾选即生效 / 切 radio 即生效"那条路不依赖 `save_config` 全量保存。
// 每个 command 自己：写 cfg + 落盘 + 必要时 emit 给浮窗 / 调 platform 层。
//
// 修复原 settings.ts:978-997 调 `set_low_power_mode` / `set_auto_hide_in_fullscreen`
// 但后端没注册 → 死按钮（catch 吞错）的 bug。

/// 即时切换省电模式：写 cfg + emit `musage://low-power-mode-changed` 给浮窗
/// 让它 toggle body[data-low-power]（styles.css 切玻璃材质）。
#[tauri::command]
pub async fn set_low_power_mode(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.low_power_mode == enabled {
            return Ok(());
        }
        cfg.low_power_mode = enabled;
        cfg.save()?;
    }
    let _ = app.emit("musage://low-power-mode-changed", enabled);
    Ok(())
}

/// 即时切换"全屏时自动隐藏浮窗"：写 cfg + 同步给 platform 层的原子开关
/// （watcher 始终运行，仅翻开关）。
#[tauri::command]
pub async fn set_auto_hide_in_fullscreen(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.auto_hide_in_fullscreen == enabled {
            return Ok(());
        }
        cfg.auto_hide_in_fullscreen = enabled;
        cfg.save()?;
    }
    crate::platform::set_auto_hide_in_fullscreen(&app, enabled);
    Ok(())
}

/// 即时切换浮窗底部提示行显隐：写 cfg + emit config-changed 让浮窗重读。
#[tauri::command]
pub async fn set_show_footer_hint(
    state: State<'_, AppState>,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.show_footer_hint == enabled {
            return Ok(());
        }
        cfg.show_footer_hint = enabled;
        cfg.save()?;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 即时切换托盘图标样式：写 cfg + 立即用新 style 重渲托盘（不等下次 poller）。
/// 即时切换托盘图标前景色：写 cfg + 立即重渲。color=null 切回自动（按菜单栏明暗）。
#[tauri::command]
pub async fn set_tray_icon_color(
    state: State<'_, AppState>,
    app: AppHandle,
    color: Option<String>,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.tray_icon_color == color {
            return Ok(());
        }
        cfg.tray_icon_color = color;
        cfg.save()?;
    }
    let state2 = app.state::<AppState>();
    let snap = state2.snapshot.read().await.clone();
    let (style, tray_source, tray_color) = {
        let cfg = state2.config.read().await;
        (
            cfg.tray_icon_style,
            cfg.tray_source.as_deref().unwrap_or("minimax").to_string(),
            crate::tray::tray_fill_color(cfg.tray_icon_color.as_deref()),
        )
    };
    if let Err(e) =
        crate::tray::update_tray_from_snapshot(&app, &snap, style, &tray_source, tray_color)
    {
        tracing::warn!(error = %e, "切换托盘颜色后重渲失败");
    }
    Ok(())
}

#[tauri::command]
pub async fn set_tray_source(
    state: State<'_, AppState>,
    app: AppHandle,
    source: Option<String>,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.tray_source == source {
            return Ok(());
        }
        cfg.tray_source = source;
        cfg.save()?;
    }
    // 立即重渲（不阻塞 cmd 返回）。source=None 时切回默认 minimax。
    let state2 = app.state::<AppState>();
    let snap = state2.snapshot.read().await.clone();
    let (style, tray_source, tray_color) = {
        let cfg = state2.config.read().await;
        (
            cfg.tray_icon_style,
            cfg.tray_source.as_deref().unwrap_or("minimax").to_string(),
            crate::tray::tray_fill_color(cfg.tray_icon_color.as_deref()),
        )
    };
    if let Err(e) =
        crate::tray::update_tray_from_snapshot(&app, &snap, style, &tray_source, tray_color)
    {
        tracing::warn!(error = %e, "切换托盘数据源后重渲失败");
    }
    Ok(())
}

#[tauri::command]
pub async fn set_tray_icon_style(
    state: State<'_, AppState>,
    app: AppHandle,
    style: TrayIconStyle,
) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        if cfg.tray_icon_style == style {
            return Ok(());
        }
        cfg.tray_icon_style = style;
        cfg.save()?;
    }
    // 立即重渲（不阻塞 cmd 返回）
    let state2 = app.state::<AppState>();
    let snap = state2.snapshot.read().await.clone();
    let (tray_source, tray_color) = {
        let cfg = state2.config.read().await;
        (
            cfg.tray_source.as_deref().unwrap_or("minimax").to_string(),
            crate::tray::tray_fill_color(cfg.tray_icon_color.as_deref()),
        )
    };
    if let Err(e) =
        crate::tray::update_tray_from_snapshot(&app, &snap, style, &tray_source, tray_color)
    {
        tracing::warn!(error = %e, "切换托盘样式后重渲失败");
    }
    Ok(())
}

/// 即时更新"显示阈值"：色档分界 [ok/cyan/warn/alert] + 钱包余额告警阈值 +
/// 4 档自定义色。
///
/// 走单字段 command 路径（参考 `set_provider_enabled` / `set_tray_icon_style`），
/// 不走 `save_config` 全量保存。写 cfg + 落盘 + emit `config-changed` 让浮窗
/// 重新渲染（颜色立刻反映新阈值/新色）。
///
/// 校验：
/// - `color_thresholds`：3 个 u8，必须 0 < t0 < t1 < t2 < 100
/// - `wallet_alert_threshold`：None 关闭；Some(n) 要求 n >= 0
/// - `color_overrides`：只允许 key ∈ {ok, cyan, warn, alert}，value 必须是
///   `#RGB` / `#RRGGBB` 形式的 hex（与 `<input type="color">` 输出一致）；
///   其他 key 一律 reject（防 typo 默默走默认）
#[tauri::command]
pub async fn set_display_thresholds(
    state: State<'_, AppState>,
    app: AppHandle,
    color_thresholds: [u8; 3],
    wallet_alert_threshold: Option<f64>,
    color_overrides: std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    let [t0, t1, t2] = color_thresholds;
    if !(0 < t0 && t0 < t1 && t1 < t2 && t2 < 100) {
        return Err(t!("commands.threshold_invalid", t0 = t0, t1 = t1, t2 = t2).into_owned());
    }
    if let Some(n) = wallet_alert_threshold {
        if !(n.is_finite() && n >= 0.0) {
            return Err(t!("commands.wallet_threshold_negative", n = n).into_owned());
        }
    }
    for (k, v) in &color_overrides {
        match k.as_str() {
            "ok" | "cyan" | "warn" | "alert" => {}
            other => {
                return Err(t!("commands.color_key_unknown", other = other).into_owned());
            }
        }
        if !is_valid_hex_color(v) {
            return Err(t!(
                "commands.color_value_invalid",
                k = k.as_str(),
                v = v.as_str()
            )
            .into_owned());
        }
    }
    {
        let mut cfg = state.config.write().await;
        cfg.color_thresholds = color_thresholds;
        cfg.wallet_alert_threshold = wallet_alert_threshold;
        cfg.color_overrides = color_overrides;
        cfg.save()?;
    }
    let _ = app.emit("musage://config-changed", ());
    Ok(())
}

/// 校验 CSS 颜色串：`#RGB` / `#RRGGBB` / `#RRGGBBAA` 形式的 hex（区分大小写不敏感）。
/// 与 `<input type="color">` 的 6 位输出对齐,同时接受 8 位(带 alpha)的 hex——
/// 浏览器 DevTools / 系统取色器复制出来常带 alpha,过去会被静默拒掉。
/// 4 位 `#RGBA` 太罕见(<input type="color"> 不产,且 hex 与 RGBA 短形式容易混淆),
/// 不接受。
fn is_valid_hex_color(s: &str) -> bool {
    let s = s.trim();
    if !s.starts_with('#') {
        return false;
    }
    let hex = &s[1..];
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// 设置面板「📋 日志」拉取最近 N 条（最新在末尾）。
///
/// `limit` 上限被裁到 [`crate::logstore::max_entries`]，防止前端乱传 100000。
#[tauri::command]
pub fn get_recent_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Vec<crate::logstore::LogEntry> {
    let cap = crate::logstore::max_entries();
    let n = limit.map(|l| l.min(cap));
    state.log.recent(n)
}

/// 设置面板「清空」按钮：清内存 + 删 jsonl 文件。**保留** dedup 缓存 + 加
/// 60s 宽限期：
/// - dedup 保留 → 用户清完 log 1s 后 poller 跑出同 (provider, kind) 错误
///   会被 60s 去重窗口吞掉，不刷出新日志
/// - 宽限期 60s → 期间所有新错误一律不写（即使不同 kind）
///
/// 两个机制叠加让用户真切看到「已清空」状态（1 分钟内），不被立刻涌出的
/// 新错误淹没。
const LOG_CLEAR_GRACE_MS: i64 = 60_000;
// 跟同文件 dedup_cache() 同款 pattern —— OnceLock<Mutex<Option<i64>>>,
// init 只跑一次。原版直接 `static Mutex::new(None)` 也能跑(Mutex::new 是 const fn),
// 但风格上跟 dedup_cache 不一致,统一过来少一种"两种风格并存"的认知负担。
static LAST_CLEAR_TS: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> =
    std::sync::OnceLock::new();

pub(crate) fn is_in_clear_grace(now_ms: i64) -> bool {
    let g = match LAST_CLEAR_TS
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        Ok(g) => g,
        Err(_) => return false,
    };
    match *g {
        // L14 fix (2026-07-06 全量审查): clock rollback → now_ms - t 负数,
        // < LOG_CLEAR_GRACE_MS 永远 true, grace 卡死后续 error 全被吞。
        // 改为 saturating + 单调性 guard:now_ms < t 时当作 grace 已过。
        Some(t) if now_ms >= t && now_ms - t < LOG_CLEAR_GRACE_MS => true,
        // Some(t) 且 now_ms < t 或超出窗口 → 都不是 grace
        Some(_) => false,
        // None → 没清过日志,grace 不生效
        None => false,
    }
}

#[tauri::command]
pub fn clear_logs(state: State<'_, AppState>) {
    state.log.clear();
    if let Ok(mut g) = dedup_cache().lock() {
        g.clear();
    }
    // 记下清空时间戳，宽限期内 log_provider_error 直接 return
    if let Ok(mut g) = LAST_CLEAR_TS
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *g = Some(chrono::Utc::now().timestamp_millis());
    }
}

#[cfg(test)]
mod dedup_cache_tests {
    use super::*;
    use std::collections::HashMap;

    fn k(s: &str) -> (String, &'static str) {
        (s.to_string(), "auth_failed")
    }

    #[test]
    fn is_dedup_window_hit_within_60s() {
        let mut cache: HashMap<(String, &'static str), i64> = HashMap::new();
        let now = 1_000_000;
        cache.insert(k("minimax"), now - 30_000);
        assert!(is_dedup_window_hit(&cache, &k("minimax"), now));
    }

    #[test]
    fn is_dedup_window_hit_after_60s_allows() {
        let mut cache: HashMap<(String, &'static str), i64> = HashMap::new();
        let now = 1_000_000;
        cache.insert(k("minimax"), now - 90_000);
        assert!(!is_dedup_window_hit(&cache, &k("minimax"), now));
    }

    #[test]
    fn is_dedup_window_hit_unknown_key_allows() {
        let cache: HashMap<(String, &'static str), i64> = HashMap::new();
        assert!(!is_dedup_window_hit(&cache, &k("minimax"), 1_000_000));
    }

    /// M1 24h 老化：retain 谓词应清掉 24h+ 的 entry,保留活跃 entry。
    /// 模拟 `log_provider_error` 锁块里的 retain 行(纯逻辑,不依赖 AppHandle)。
    #[test]
    fn retain_evicts_entries_older_than_24h() {
        let mut cache: HashMap<(String, &'static str), i64> = HashMap::new();
        let now = 1_000_000_000_000_i64; // 随便一个 now
        let ttl = LOG_DEDUP_TTL_MS;

        // 24h+1s 前插入 → 期望被清
        cache.insert(k("minimax"), now - ttl - 1_000);
        // 24h 边界前 1ms → 期望保留(now - ts < ttl)
        cache.insert(k("claude_official"), now - ttl + 1);
        // 30s 前 → 活跃,保留
        cache.insert(k("zhipu"), now - 30_000);

        cache.retain(|_, ts| now.saturating_sub(*ts) < ttl);

        assert!(!cache.contains_key(&k("minimax")), "24h+ entry 应被清");
        assert!(
            cache.contains_key(&k("claude_official")),
            "边界内 entry 保留"
        );
        assert!(cache.contains_key(&k("zhipu")), "活跃 entry 保留");
    }

    /// 时钟回拨：last_ts > now 时 saturating_sub 返 0,entry 不会被误清。
    /// (保留 entry 等下个正常周期再清;同时 L13 已经在 is_dedup_window_hit 里
    /// 把回拨当"很久以前"处理,允许写新日志)
    #[test]
    fn retain_clock_rollback_keeps_entry() {
        let mut cache: HashMap<(String, &'static str), i64> = HashMap::new();
        let now = 1_000_000;
        // 未来时间戳(last_ts = now + 5min)
        cache.insert(k("minimax"), now + 5 * 60 * 1000);

        cache.retain(|_, ts| now.saturating_sub(*ts) < LOG_DEDUP_TTL_MS);

        assert!(cache.contains_key(&k("minimax")), "回拨时 entry 不应被清");
    }

    /// 端到端风格:模拟 `log_provider_error` 走完一次 insert + dedup,
    /// 验证后续 60s 内的同 key 调用被 helper 命中。
    #[test]
    fn dedup_window_blocks_repeat_calls() {
        let mut cache: HashMap<(String, &'static str), i64> = HashMap::new();
        let t0 = 1_000_000;
        let key = k("minimax");

        // 首次调用:cache 未知 → is_dedup_window_hit = false → 允许写
        assert!(!is_dedup_window_hit(&cache, &key, t0));
        cache.insert(key.clone(), t0);

        // 30s 后同 key → 命中
        assert!(is_dedup_window_hit(&cache, &key, t0 + 30_000));

        // 65s 后同 key → 放过
        assert!(!is_dedup_window_hit(&cache, &key, t0 + 65_000));
    }
}

/// P3 / CM10 fix (2026-07-28 审查) 的纯逻辑单测:snapshot_key 身份键规则
/// + provider_order 清洗。
#[cfg(test)]
mod snapshot_key_tests {
    use super::*;

    fn snap(unique: Option<&str>, source: Option<&str>, provider: &str) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: provider.to_string(),
            unique_id: unique.map(|s| s.to_string()),
            source_id: source.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn unique_id_wins_over_source_id() {
        // 副本成功 snapshot: source_id 是 provider 侧硬编码的 base id,
        // unique_id 由 caller 注入 —— 身份键必须取 unique_id,否则副本
        // 合并时覆盖基础实例条目。
        let p = snap(Some("minimax#2"), Some("minimax"), "minimax");
        assert_eq!(snapshot_key(&p), "minimax#2");
    }

    #[test]
    fn falls_back_to_source_id_then_provider() {
        // 老 snapshot(无 unique_id) → source_id;两个都没 → provider 字段
        let p = snap(None, Some("minimax"), "minimax");
        assert_eq!(snapshot_key(&p), "minimax");
        let p2 = snap(None, None, "deepseek");
        assert_eq!(snapshot_key(&p2), "deepseek");
    }

    #[test]
    fn base_and_dup_have_distinct_keys() {
        // 基础实例 vs 副本错误 snapshot(source_id 同为 base)必须区分
        let base = snap(Some("minimax"), Some("minimax"), "minimax");
        let dup_err = snap(Some("minimax#2"), Some("minimax"), "minimax");
        assert_ne!(snapshot_key(&base), snapshot_key(&dup_err));
    }

    #[test]
    fn sanitize_order_filters_unknown_and_dupes() {
        let known: std::collections::HashSet<String> = ["minimax", "deepseek", "minimax#2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = sanitize_provider_order(
            vec![
                "minimax".into(),
                "bogus".into(),   // 未知 id → 剔除
                "minimax".into(), // 重复 → 剔除(保留首次出现)
                "deepseek".into(),
            ],
            &known,
        );
        assert_eq!(out, vec!["minimax".to_string(), "deepseek".to_string()]);
    }

    #[test]
    fn sanitize_order_keeps_extra_instance_ids() {
        // 副本 unique_id 是合法条目,不能被当未知 id 剔掉
        let known: std::collections::HashSet<String> = ["minimax", "minimax#2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = sanitize_provider_order(vec!["minimax#2".into(), "minimax".into()], &known);
        assert_eq!(out, vec!["minimax#2".to_string(), "minimax".to_string()]);
    }

    /// H2 fix (2026-08-03 audit) 回归测试:两个 snapshot 共享同一 source_id
    /// ("minimax" base),但 unique_id 不同("minimax#1" / "minimax#2"),`snapshot_key`
    /// 必须把它们当成两条独立条目,retain/iter().any() 都按 unique_id 而非 source_id
    /// 区分。模拟的正是设置面板里"关掉 minimax#2 但不要碰 minimax#1"的 H2 bug 场景。
    #[test]
    fn retain_by_snapshot_key_keeps_other_duplicate_source_id() {
        let base = ProviderSnapshot {
            provider: "minimax".into(),
            unique_id: Some("minimax#1".into()),
            source_id: Some("minimax".into()),
            ..Default::default()
        };
        let dup = ProviderSnapshot {
            provider: "minimax".into(),
            unique_id: Some("minimax#2".into()),
            source_id: Some("minimax".into()),
            ..Default::default()
        };
        let mut snap = QuotaSnapshot {
            providers: vec![base, dup],
            ..Default::default()
        };

        // 关掉副本 minimax#2 (id 走 unique_id 路径):只删 dup,base 留下。
        let id = "minimax#2".to_string();
        snap.providers.retain(|p| snapshot_key(p) != id);

        assert_eq!(snap.providers.len(), 1);
        assert_eq!(snapshot_key(&snap.providers[0]), "minimax#1");
    }

    /// H2 fix 反向场景:set_provider_enabled(true) 在副本已存在时不应再
    /// push 一份 placeholder。`already_present` 检查必须按 snapshot_key,
    /// 否则用 source_id 匹配置信 base,副本会被当成"新条目"再来一份。
    #[test]
    fn any_already_present_uses_snapshot_key_not_source_id() {
        let base = ProviderSnapshot {
            provider: "minimax".into(),
            unique_id: Some("minimax#1".into()),
            source_id: Some("minimax".into()),
            ..Default::default()
        };
        let dup = ProviderSnapshot {
            provider: "minimax".into(),
            unique_id: Some("minimax#2".into()),
            source_id: Some("minimax".into()),
            ..Default::default()
        };
        let snap = QuotaSnapshot {
            providers: vec![base.clone(), dup],
            ..Default::default()
        };

        // 副本 id="minimax#2" 已在 vec 里 → already_present=true,不重复 push。
        let id_dup = "minimax#2".to_string();
        assert!(snap.providers.iter().any(|p| snapshot_key(p) == id_dup));

        // base id="minimax#1" 也在 vec 里 → 已存在。
        let id_base = "minimax#1".to_string();
        assert!(snap.providers.iter().any(|p| snapshot_key(p) == id_base));

        // 不存在的 id → 不命中。
        let id_missing = "minimax#3".to_string();
        assert!(!snap.providers.iter().any(|p| snapshot_key(p) == id_missing));
    }

    /// H2 fix 第三处 get_snapshot 的 retain 也走 snapshot_key:
    /// 用 bug 描述里同样的双 snapshot 模拟"副本已禁用"过滤,base 不应被误过。
    #[test]
    fn get_snapshot_filter_distinguishes_dup_from_base() {
        use std::collections::{BTreeMap, HashSet};
        // 模拟 is_enabled_id 的简化语义:enabled 集合里的 id 才通过 retain。
        let enabled: HashSet<String> = ["minimax#1"].into_iter().map(String::from).collect();
        let mut by_id: BTreeMap<String, bool> = BTreeMap::new();
        for k in &enabled {
            by_id.insert(k.clone(), true);
        }
        let is_enabled = |id: &str| by_id.get(id).copied().unwrap_or(false);

        let base = ProviderSnapshot {
            provider: "minimax".into(),
            unique_id: Some("minimax#1".into()),
            source_id: Some("minimax".into()),
            ..Default::default()
        };
        let dup = ProviderSnapshot {
            provider: "minimax".into(),
            unique_id: Some("minimax#2".into()),
            source_id: Some("minimax".into()),
            ..Default::default()
        };
        let mut snap = QuotaSnapshot {
            providers: vec![base, dup],
            ..Default::default()
        };

        snap.providers.retain(|p| is_enabled(snapshot_key(p)));

        // 只留 base (minimax#1),副本 (minimax#2) 因为源里 enabled=false 被滤掉。
        // 老实现按 source_id 匹配 → 两条都是 "minimax" → 两条都过 / 都不通过,
        // 取决于 enabled 集合里填的是 base 名还是副本名。
        assert_eq!(snap.providers.len(), 1);
        assert_eq!(snapshot_key(&snap.providers[0]), "minimax#1");
    }
}
