// 设置面板共享类型
//
// v0.2 (2026-06-22) 后, `ProviderId` = `string` (是类型别名, 不是 enum)。
// 所有 source id 都走字符串, 包括内置 provider ("minimax" / "tavily" /...)
// 和用户自定义 source ("custom_<uuid>")。旧 enum IPC 已全部删除,
// 前端走 setSourceCredential / hasSourceCredential / deleteSourceCredential
// 统一 API。

export type ProviderId = string;

export type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

export interface ProviderConfig {
  enabled: boolean;
  region?: "cn" | "en" | null;
  xiaomi_region?: "cn" | "sgp" | "ams" | null;
  /// 可选：覆盖全局轮询间隔（秒）。None = 用全局 default
  refresh_interval_secs?: number | null;
  /// P2 起：可调显示模式 (Xiaomi 用)
  xiaomi_display_mode?: "all" | "plan_only" | "total_only" | null;
}

/// 4 个 provider 面板的 id 列表。决定 #3/#4 UI 循环读哪些元素。
///
/// **PR 3 起废弃** —— 改用 `getCurrentKnownIds()` 动态从 `listSources()`
/// 拿。`loadConfig()` 里 SPEC 化的 SPEC 化读 cfg.providers[id] 用 dynamic
/// id 路径。保留 const 是给少数需要"内置 13 个 id 字面量"的地方做兜底
/// （如 fallback 默认 region）。
/// 已迁移到 `getCurrentKnownIds()`。
export const PROVIDER_IDS = [
  "minimax",
  "deepseek",
  "xiaomimimo",
  "tavily",
  "zenmux",
  "openrouter",
  "kimi",
  "zhipu",
  // 2026-06-16 新增（PR 2）
  "stepfun",
  "siliconflow",
  "claude_official",
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
  /** true = 主面板不渲染凭据字段（移至"高级"tab） */
  hide_credentials?: boolean;
  /** true = STUB（公开 API 无 quota endpoint，2026-06-17 commit 加）。
   *  UI 加灰显 + "未支持" 角标；老面板忽略该字段不影响渲染。 */
  is_stub?: boolean;
  /** P0-1 fix: ExtraInstance 的 UUID（仅 extra instance 有值）。
   *  删除/更新 IPC 需要传 UUID，不能传 api_key_ref。 */
  extra_instance_uuid?: string;
}

/// Xiaomi MiMo 浮窗显示模式：
/// - `all`：完整（3 行，套餐和总额度数字一致时自动合并）
/// - `plan_only`：只显示套餐 1 行
/// - `total_only`：只显示总额度 1 行（带重置日期）
/// 默认 `all`
export type XiaomiDisplayMode = "all" | "plan_only" | "total_only";

/** 设置面板的"显示模式"选择项配置（label + 描述，给 UI 用） */
export const XIAOMI_DISPLAY_MODE_OPTIONS: Record<
  XiaomiDisplayMode,
  { label: string; description: string }
> = {
  all: {
    label: "完整显示",
    description: "3 行（套餐 / 补偿 / 总额度），数字一致时自动合并",
  },
  plan_only: {
    label: "只看套餐",
    description: "只显示套餐用量 + 重置时间，不显示补偿和总额度",
  },
  total_only: {
    label: "只看总额度",
    description: "只显示本月总消耗 + 重置时间，适合有补偿积分的用户",
  },
};

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
  /// 浮窗底部提示行（默认隐藏，用户手动开启）
  show_footer_hint?: boolean;
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
  /// 4 档色阈值分界（从小到大，3 个分界点切出 4 段：
  /// `[..t0]`=ok 绿 / `[t0..t1]`=cyan 青 / `[t1..t2]`=warn 黄 /
  /// `[t2..]`=alert 红）。默认 [50, 70, 88]。老 config.json 缺这字段
  /// 时 Rust 端用默认值，前端亦然。
  color_thresholds?: [number, number, number];
  /// 钱包/余额行（r.remaining 单独存在的那种）的"低额高亮"阈值。
  /// null/undefined = 关闭（按现状显示蓝色 / 默认色）；
  /// 数字 = remaining < 该值时把行翻成 alert 红。
  wallet_alert_threshold?: number | null;
  /// 用户自定义 4 档色（hover 时显示）：{ok, cyan, warn, alert} → "#RRGGBB"。
  /// 空对象 / 缺字段 = 走 iOS 系统默认色。浮窗 init 时把非空项写进
  /// #app 的 inline CSS 变量 --c-data-{key}，bar / dot 同步跟着变。
  color_overrides?: Record<string, string>;
  /// P0 国际化：UI 语言代码。"zh-CN" / "en"。老 config.json 缺这字段时
  /// Rust 端走 zh-CN 默认。frontend initLocale() 读它决定默认 locale。
  locale?: string;
  /// P2 区域：用户首启向导选定的区域。"cn" / "global" / "custom"。
  /// 影响默认 provider 顺序 + MiniMax endpoint。手动改过顺序/endpoint 后
  /// 自动变 "custom"（防止 wizard 反复弹）。
  user_region?: "cn" | "global" | "custom";
}

export interface ProviderSnapshot {
  /** 兼容字段（minimax / deepseek / xiaomimimo）。Phase 1 起请用 source_id。
   * **PR 3** 起改成 string（用户自定义 source 没有 Provider enum 变体）。 */
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
  /** 下次自动 fetch 的 epoch ms。浮窗错误卡片用这个显示 "下次重试 in Xm" 倒计时。
   *  2026-06-17 commit 加。null/undefined = 未知。 */
  next_fetch_at?: number | null;
}

// ── PR 3: 用户自定义 New API source ───────────────────────────

/// Extract 模板（3 选 1）。
///
/// 跟 Rust `ExtractSpec` enum 一一对应（`#[serde(tag = "preset", rename_all = "snake_case")]`）。
export type ExtractSpec =
  | { preset: "new_api"; divide?: number | null }
  | {
      preset: "balance";
      balance_path: string;
      currency_path?: string | null;
      divide?: number | null;
    }
  | {
      preset: "custom";
      remaining_path?: string | null;
      used_path?: string | null;
      total_path?: string | null;
      unit?: string | null;
      divide?: number | null;
    };

/// 用户自定义 source 的完整 spec。序列化为 JSON 存 `custom_sources.json`。
///
/// 跟 Rust `CustomSourceSpec` 一致。`id` 和 `created_at` 由后端生成，
/// 前端在「添加」时只传 `Omit<CustomSourceSpec, "id" | "created_at">`。
export interface CustomSourceSpec {
  /** "custom_a1b2c3d4" —— UUID 后 8 位 hex */
  id: string;
  display_name: string;
  base_url: string;
  path: string;
  method: "GET" | "POST";
  extract: ExtractSpec;
  plan_name_path?: string | null;
  accent?: string | null;
  created_at: number;
}

// ── PR 1b: 用户额外 source 实例 (内置副本 + New API 中转站) ──────

/**
 * 一条 extra instance（内置 provider 副本 / New API 中转站）。
 *
 * 后端 `commands::extra_instances::*` IPC 返回这个结构。
 * 浮窗 / settings 面板用 `unique_id` (`minimax#2` / `custom_<uuid>`)
 * 做 DOM 区分；`instance_index` 用来做后端紧凑重排。
 */
export interface ExtraInstance {
  id: string;
  provider_id: string;
  instance_index: number;
  api_key_ref: string;
  custom: CustomSourceSpec | null;
  created_at: number;
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

/** PR 1b: provider picker 用的 option（11 内置 + custom）。
 *  后端 list_picker_providers 返回这个结构。
 *
 *  v0.2.1 commit 4:`display_name` 是后端 `t!()` 注入的翻译字符串(单一来源 =
 *  后端 `src-tauri/locales/{en,zh-CN}.json`),前端不再用 `t("provider_name.xxx")`。
 *  `name_key` 字段在 DTO 上是 `skip_serializing_if="is_empty"`,新版本后端不会返。
 *  保留字段是为了兼容过渡期前端代码不崩。 */
export interface PickerProvider {
  id: string;
  name_key?: string;
  display_name: string;
  auth_kind: "api_key" | "cookie" | "api_key_or_cookie";
  is_builtin: boolean;
}
