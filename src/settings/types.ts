// 设置面板共享类型
//
// 历史包袱：`ProviderId` 联合保留了 5 个 provider（minimax/deepseek/
// xiaomimimo/tavily/zenmux）。Phase 2 起新 source 走 registry id（string），
// 旧 enum 路径只在 has_/set_/delete_/get_api_key_for 等命令里继续用。

export type ProviderId =
  | "minimax"
  | "deepseek"
  | "xiaomimimo"
  | "tavily"
  | "zenmux"
  | "openrouter"
  | "kimi"
  | "zhipu";

export type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

export interface ProviderConfig {
  enabled: boolean;
  region?: "cn" | "en" | null;
  xiaomi_region?: "cn" | "sgp" | "ams" | null;
  /// 可选：覆盖全局轮询间隔（秒）。None = 用全局 default
  refresh_interval_secs?: number | null;
}

/// 4 个 provider 面板的 id 列表。决定 #3/#4 UI 循环读哪些元素。
export const PROVIDER_IDS = [
  "minimax",
  "deepseek",
  "xiaomimimo",
  "tavily",
  "zenmux",
  "openrouter",
  "kimi",
  "zhipu",
] as const;

/// Phase 1 起新 source 的元信息（后端 list_sources 返回）。
/// 当前 settings.ts 直接用 list_sources 返回 SourceMeta[] 来构建面板，
/// 这里的接口保留给未来的动态渲染。
export interface SourceMeta {
  id: string;
  display_name: string;
  /** "api_key" / "cookie" / "api_key_or_cookie"（多鉴权，Xiaomi 用） */
  auth_kind: "api_key" | "cookie" | "api_key_or_cookie";
  enabled: boolean;
}

export interface FieldTriple {
  total: string;
  remaining: string;
  end?: string | null;
}

export interface TierOverrides {
  count_candidates: FieldTriple[];
}

export interface ProviderOverrides {
  five_hour: TierOverrides;
  weekly: TierOverrides;
  /** Phase 1 新增：xiaomi MiMo 月度 tier 候选 */
  monthly?: TierOverrides;
}

export interface AppConfig {
  providers: Record<string, ProviderConfig>;
  refresh_interval_secs: number;
  autostart: boolean;
  /// 关闭主窗口时是否隐藏到托盘（旧字段，Rust 端必填，缺了 save_config 会报
  /// "missing field" —— 务必保留在 TS interface 里，否则 spread 展开后丢字段）
  show_in_tray_on_close?: boolean;
  floating_x: number | null;
  floating_y: number | null;
  floating_w?: number | null;
  floating_h?: number | null;
  floating_pin_mode?: FloatingPinMode;
  low_power_mode?: boolean;
  auto_hide_in_fullscreen?: boolean;
  /// Tavily 简洁模式：只显示主指标 + 进度条，隐藏 5 个 endpoint 细分行
  tavily_concise_mode?: boolean;
  /// Provider 在浮窗里的渲染顺序。空数组 = 用 builtin_sources() 注册表顺序
  provider_order?: string[];
  /// ZenMux 自定义 Management API endpoint URL。null/空 = 用 zenmux.rs 里的默认 URL
  zenmux_base_url?: string | null;
  /// ZenMux 查看模式（payg / subscription）。Stage 5 新增
  zenmux_mode?: "payg" | "subscription";
  /// ZenMux PAYG 简洁模式（只显示余额，不显示充值/奖励）
  zenmux_payg_concise_mode?: boolean;
  /// 智谱 GLM 区域：cn = 国区 open.bigmodel.cn（默认），en = 国际 api.z.ai
  zhipu_region?: "cn" | "en";
  // 用户加的字段名候选（应对 MiniMax 改 schema）
  schema_overrides?: Record<string, ProviderOverrides>;
  /// v0.6+ 托盘图标样式（Rust 端 TrayIconStyle enum）
  tray_icon_style?: "logo" | "bars" | "percent";
}

export interface ProviderSnapshot {
  /** 兼容字段（minimax / deepseek / xiaomimimo）。Phase 1 起请用 source_id。 */
  provider: ProviderId;
  /** Phase 1 新增。 */
  source_id?: string | null;
  source_display_name?: string | null;
  plan_name?: string | null;
  success: boolean;
  rows: Array<{
    label: string;
    utilization: number | null;
    remaining: number | null;
    used?: number | null;
    total?: number | null;
    unit: string | null;
  }>;
  error: string | null;
  error_kind?:
    | "unconfigured_key"
    | "auth_failed"
    | "rate_limited"
    | "network"
    | "parse"
    | "schema_unknown"
    | "server_error"
    | "other"
    | null;
}

export interface QuotaSnapshot {
  providers: ProviderSnapshot[];
  fetched_at: number | null;
}

export interface LogEntry {
  ts: number;
  level: "info" | "warn" | "error";
  provider: string | null;
  kind: string | null;
  message: string;
}
