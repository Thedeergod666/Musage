// 设置面板 IPC 集中层
//
// **唯一** 接触 @tauri-apps/api/core::invoke 的地方（除 updater.ts 自己用的
// listen），方便单点替换 / mock 测试。settings 的所有功能都走这些 typed wrappers。

import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  CustomSourceSpec,
  ExtraInstance,
  LogEntry,
  PickerProvider,
  ProviderOverrides,
  ProviderSnapshot,
  QuotaSnapshot,
  SourceMeta,
} from "./types";

// ── Config 全量 ───────────────────────────────────────────────

export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

/** 即时更新 schema_overrides（MiniMax 5h / 周 + Xiaomi 月 字段名候选）。
 *  走单字段 command 路径，落盘 + 自动 trigger refresh 受影响的 provider。
 *  2026-06-20 audit fix：之前 src/settings/config.ts:saveConfig 是死代码，
 *  用户改 advanced.ts 3 个 textarea 后保存不下来。
 */
export async function setSchemaOverrides(
  overrides: Record<string, ProviderOverrides>,
): Promise<void> {
  await invoke("set_schema_overrides", { overrides });
}

export async function saveConfig(cfg: AppConfig): Promise<void> {
  await invoke("save_config", { cfg });
}

// ── 凭据（id-based，新 API，registry-driven）─────────────────────

export async function listSources(): Promise<SourceMeta[]> {
  return invoke<SourceMeta[]>("list_sources");
}

export async function hasSourceCredential(id: string): Promise<boolean> {
  return invoke<boolean>("has_source_credential", { id });
}

/**
 * 保存 source 的凭据。
 *
 * @param id      source id (e.g. "xiaomimimo")
 * @param value   要保存的值
 * @param field   可选：明确指定落到哪个字段 ("api_key" / "cookie")
 *                不传时按 source 的 auth_kind 默认：
 *                  - "api_key"        → api_key
 *                  - "cookie"         → cookie
 *                  - "api_key_or_cookie" → api_key
 *                多鉴权 source（Xiaomi）必须传 field，否则两输入都落 api_key。
 */
export async function setSourceCredential(
  id: string,
  value: string,
  field?: "api_key" | "cookie",
): Promise<void> {
  await invoke("set_source_credential", { id, value, field });
}

// ── Xiaomi 显示模式 ──────────────────────────────────────────────

/** 读 Xiaomi 当前显示模式（"all" / "plan_only" / "total_only"） */
export async function getXiaomiDisplayMode(): Promise<string> {
  return await invoke<string>("get_xiaomi_display_mode");
}

/** 切换 Xiaomi 显示模式 —— 即时生效（后端落盘 + refresh 一次） */
export async function setXiaomiDisplayMode(
  mode: "all" | "plan_only" | "total_only",
): Promise<void> {
  await invoke("set_xiaomi_display_mode", { mode });
}

export async function deleteSourceCredential(id: string): Promise<void> {
  await invoke("delete_source_credential", { id });
}

export async function getSourceCredential(
  id: string,
): Promise<string | null> {
  return invoke<string | null>("get_source_credential", { id });
}

// v0.2 (2026-06-22) 删除 7 个旧 enum-based IPC wrapper:
// hasApiKeyFor / setApiKeyFor / deleteApiKeyFor / getApiKeyFor /
// hasCookieFor / setCookieFor / deleteCookieFor
// 后端 IPC 已删 (PR 5 合并到 PR 4), 前端必须改用 setSourceCredential /
// hasSourceCredential / deleteSourceCredential / getSourceCredential

// ── 浮窗 / 窗口控制 ─────────────────────────────────────────

export async function setFloatingPinMode(
  mode: "pin_top" | "pin_bottom" | "normal",
): Promise<void> {
  await invoke("set_floating_pin_mode", { mode });
}

/// v0.6+ 新增：即时切换省电模式。修原 settings.ts:978 onchange 调不存在
/// command（被 catch 吞错 → 死按钮）的 bug。
export async function setLowPowerMode(enabled: boolean): Promise<void> {
  await invoke("set_low_power_mode", { enabled });
}

/// v0.6+ 新增：即时切换"全屏时自动隐藏浮窗"。
export async function setAutoHideInFullscreen(enabled: boolean): Promise<void> {
  await invoke("set_auto_hide_in_fullscreen", { enabled });
}

/// 即时切换浮窗底部提示行显隐。
export async function setShowFooterHint(enabled: boolean): Promise<void> {
  await invoke("set_show_footer_hint", { enabled });
}

/// v0.6+ 新增：即时切换托盘图标样式（logo / bars / percent）。
/// 后端会落盘 + 立即重渲托盘（不等下次 poller）。
export async function setTrayIconStyle(
  style: "logo" | "bars" | "percent",
): Promise<void> {
  await invoke("set_tray_icon_style", { style });
}

/**
 * 即时更新"显示阈值"：色档分界 + 钱包余额告警阈值 + 4 档自定义色。即时生效。
 *
 * @param colorThresholds   [t0, t1, t2]，0 < t0 < t1 < t2 < 100
 * @param walletAlertThreshold  null = 关闭；n = remaining < n 时该行翻红
 * @param colorOverrides   4 档自定义色：{ok?, cyan?, warn?, alert?} → "#RRGGBB"。
 *                          缺哪个 key = 哪个 key 走默认；空对象 = 全部默认。
 *                          key 名错 / hex 不合法会被 Rust 端 reject。
 */
export async function setDisplayThresholds(
  colorThresholds: [number, number, number],
  walletAlertThreshold: number | null,
  colorOverrides: Record<string, string>,
): Promise<void> {
  await invoke("set_display_thresholds", {
    colorThresholds,
    walletAlertThreshold,
    colorOverrides,
  });
}

export async function setProviderEnabled(
  id: string,
  enabled: boolean,
): Promise<void> {
  await invoke("set_provider_enabled", { id, enabled });
}

export async function setProviderOrder(order: string[]): Promise<void> {
  await invoke("set_provider_order", { order });
}

export async function resetFloatingWindow(): Promise<void> {
  await invoke("reset_floating_window");
}

// ── 测试连接 ──────────────────────────────────────────────────

export async function refreshNow(): Promise<QuotaSnapshot> {
  return invoke<QuotaSnapshot>("refresh_now");
}

// ── 日志 ──────────────────────────────────────────────────────

export async function getRecentLogs(limit = 200): Promise<LogEntry[]> {
  return invoke<LogEntry[]>("get_recent_logs", { limit });
}

export async function clearLogs(): Promise<void> {
  await invoke("clear_logs");
}

// ── App 元信息（updater 用）────────────────────────────────────

export async function getAppVersion(): Promise<string> {
  return invoke<string>("get_app_version");
}

// ── PR 1b: 用户额外 source 实例 (内置副本 + New API 中转站) ──────────

/** PR 1b：列出所有 extra instance（包含 custom + 内置副本） */
export async function listExtraInstances(): Promise<ExtraInstance[]> {
  return invoke<ExtraInstance[]>("list_extra_instances");
}

/** PR 1b：provider picker 数据源（11 内置 + custom） */
export async function listPickerProviders(): Promise<PickerProvider[]> {
  return invoke<PickerProvider[]>("list_picker_providers");
}

/** PR 1b：添加一个 extra instance（内置副本或 custom） */
export interface AddExtraInstanceRequest {
  provider_id: string;
  api_key?: string;
  api_cookie?: string;
  custom?: Omit<CustomSourceSpec, "id" | "created_at">;
}
export async function addExtraInstance(
  req: AddExtraInstanceRequest,
): Promise<ExtraInstance> {
  return invoke<ExtraInstance>("add_extra_instance", { req });
}

/** PR 1b：更新一个 extra instance（按 id 找） */
export interface UpdateExtraInstanceRequest {
  id: string;
  api_key?: string;
  api_cookie?: string;
  custom?: Omit<CustomSourceSpec, "id" | "created_at">;
}
export async function updateExtraInstance(
  req: UpdateExtraInstanceRequest,
): Promise<ExtraInstance> {
  return invoke<ExtraInstance>("update_extra_instance", { req });
}

/** PR 1b：删除一个 extra instance（按 id） */
export async function deleteExtraInstance(id: string): Promise<void> {
  await invoke("delete_extra_instance", { id });
}

/** PR 1b：测试连接（不写 state） */
export interface TestExtraInstanceRequest {
  provider_id: string;
  api_key?: string;
  api_cookie?: string;
  custom?: Omit<CustomSourceSpec, "id" | "created_at">;
}
export async function testExtraInstance(
  req: TestExtraInstanceRequest,
): Promise<ProviderSnapshot> {
  return invoke<ProviderSnapshot>("test_extra_instance", { req });
}

// ── C3 fix: source-extras 6 个交互控件的 per-field setter ────────
//
// 之前 source-extras.ts 里的 region select / concise checkbox / base_url input /
// zenmux mode / zhipu region 全部没有 change handler，配置改完静默丢失。
// 每个 setter 后端做: 改 cfg 对应字段 → 落盘 → emit config-changed → refresh_single。
//（浮窗/托盘立即看到新值，不等 poller。）

export async function setMinimaxRegion(region: "cn" | "en"): Promise<void> {
  await invoke("set_minimax_region", { region });
}

export async function setXiaomiRegion(region: "cn" | "sgp" | "ams"): Promise<void> {
  await invoke("set_xiaomi_region", { region });
}

export async function setTavilyConciseMode(enabled: boolean): Promise<void> {
  await invoke("set_tavily_concise_mode", { enabled });
}

export async function setZenmuxBaseUrl(url: string): Promise<void> {
  await invoke("set_zenmux_base_url", { url });
}

export async function setZenmuxMode(mode: "payg" | "subscription"): Promise<void> {
  await invoke("set_zenmux_mode", { mode });
}

export async function setZenmuxPaygConcise(enabled: boolean): Promise<void> {
  await invoke("set_zenmux_payg_concise", { enabled });
}

export async function setZhipuRegion(region: "cn" | "en"): Promise<void> {
  await invoke("set_zhipu_region", { region });
}

// ── P2 区域向导 ──

/** P2 区域：用户选定后 apply 默认 provider 顺序 + endpoint */
export async function setRegion(region: string): Promise<void> {
  await invoke("set_region", { region });
}

/** P2 区域：取当前 user_region（"cn" / "global" / "custom"） */
export async function getRegion(): Promise<string> {
  return invoke<string>("get_region");
}
