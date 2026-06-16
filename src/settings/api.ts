// 设置面板 IPC 集中层
//
// **唯一** 接触 @tauri-apps/api/core::invoke 的地方（除 updater.ts 自己用的
// listen），方便单点替换 / mock 测试。settings 的所有功能都走这些 typed wrappers。

import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  CustomSourceSpec,
  LogEntry,
  ProviderId,
  ProviderSnapshot,
  QuotaSnapshot,
  SourceMeta,
} from "./types";

// ── Config 全量 ───────────────────────────────────────────────

export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
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

// ── 凭据（旧 Provider enum-based API，保留给老的 3 个 provider）────

export async function hasApiKeyFor(provider: ProviderId): Promise<boolean> {
  return invoke<boolean>("has_api_key_for", { provider });
}

export async function setApiKeyFor(
  provider: ProviderId,
  key: string,
): Promise<void> {
  await invoke("set_api_key_for", { provider, key });
}

export async function deleteApiKeyFor(provider: ProviderId): Promise<void> {
  await invoke("delete_api_key_for", { provider });
}

export async function getApiKeyFor(
  provider: ProviderId,
): Promise<string | null> {
  return invoke<string | null>("get_api_key_for", { provider });
}

export async function hasCookieFor(provider: ProviderId): Promise<boolean> {
  return invoke<boolean>("has_cookie_for", { provider });
}

export async function setCookieFor(
  provider: ProviderId,
  cookie: string,
): Promise<void> {
  await invoke("set_cookie_for", { provider, cookie });
}

export async function deleteCookieFor(provider: ProviderId): Promise<void> {
  await invoke("delete_cookie_for", { provider });
}

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

// ── PR 3: 用户自定义 New API source ─────────────────────────────

/** 列出所有 custom source specs（无 customs 时返空数组） */
export async function listCustomSources(): Promise<CustomSourceSpec[]> {
  return invoke<CustomSourceSpec[]>("list_custom_sources");
}

/**
 * 添加 custom source。
 *
 * 后端会：
 * 1. 校验 display_name/base_url/path/method
 * 2. 生成 `custom_<uuid>` id 和 created_at
 * 3. 持久化到 `custom_sources.json`
 * 4. emit config-changed（settings 重建）
 * 5. **立即 refresh_single**（浮窗不等 poller 就出第一条数据）
 *
 * @returns 新 source 的 id (`custom_<uuid>`)
 */
export async function addCustomSource(
  spec: Omit<CustomSourceSpec, "id" | "created_at">,
): Promise<string> {
  return invoke<string>("add_custom_source", { spec });
}

/** 更新 custom source（按 id 匹配）—— 后端持久化 + 立即 refresh */
export async function updateCustomSource(spec: CustomSourceSpec): Promise<void> {
  await invoke("update_custom_source", { spec });
}

/** 删除 custom source —— 后端从 custom_sources.json + keys.json 都删 */
export async function deleteCustomSource(id: string): Promise<void> {
  await invoke("delete_custom_source", { id });
}

/**
 * 测试连接 —— 不写 spec，只用临时 CustomSource fetch 一次。
 * 用于 modal 的「测试一下」按钮。
 *
 * @param spec     用户填好的 spec（base_url / path / extract 必填）
 * @param apiKey   用户填的 API key（暂存，不持久化）
 * @returns ProviderSnapshot（成功时 success=true + rows，失败时 error+error_kind）
 */
export async function testCustomSource(
  spec: Omit<CustomSourceSpec, "id" | "created_at">,
  apiKey: string,
): Promise<ProviderSnapshot> {
  return invoke<ProviderSnapshot>("test_custom_source", { spec, apiKey });
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
