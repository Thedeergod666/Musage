// EXTRAS 表：id → 额外 UI 块
//
// 每个 source 在 createProviderPanel() 渲染 header + credentials 之后，
// 会按 EXTRAS[id] 顺序插入这些块（region select / cookie 字段 /
// concise mode checkbox / base url 输入框 等）。
//
// **加新 provider 改这里 + builtin_sources() 即可**，不用动 providers.ts
// 主流程。

import { el } from "./utils";
import type { AppConfig, SourceMeta } from "./types";

export type ExtraBlock = (meta: SourceMeta, cfg: AppConfig) => HTMLElement;

/// 静态表 —— key 是 source.id，value 是要插入的额外块工厂。
/// 找不到的 id 返回空数组（deepseek 就是这样）。
const EXTRAS: Record<string, ExtraBlock[]> = {
  minimax: [renderRegionSelect],
  xiaomimimo: [renderXiaomiRegionSelect],
  tavily: [renderConciseModeCheckbox],
  zenmux: [renderBaseUrlInput, renderZenmuxMode],
  // deepseek: 无额外字段
};

export function getProviderExtras(id: string): ExtraBlock[] {
  return EXTRAS[id] ?? [];
}

// ── 各 provider 的额外块 ──────────────────────────────────────

/// MiniMax 区域选择（cn / en）。从 cfg.providers.minimax.region 读初值。
function renderRegionSelect(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const current = cfg.providers?.minimax?.region ?? "cn";
  const select = el("select", { "data-id": "region", id: "region" });
  select.appendChild(
    el("option", { value: "cn" }, "🌐 国内 (api.minimaxi.com)"),
  );
  select.appendChild(
    el("option", { value: "en" }, "🌐 国际 (api.minimax.io)"),
  );
  select.value = current;
  // 即时生效：change → save_config（最简版 v1 仍走总保存，但 select 是 Stage 5 才换即时）
  // 这里暂不绑 change，避免 Stage 4 引入新行为

  return el(
    "div",
    { class: "field" },
    el("label", {}, "MiniMax 区域"),
    select,
  );
}

/// Xiaomi MiMo 集群选择（cn / sgp / ams）。从 cfg.xiaomi_region 读初值。
function renderXiaomiRegionSelect(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const current = cfg.providers?.xiaomimimo?.xiaomi_region ?? "cn";
  const select = el("select", { "data-id": "xiaomi-region", id: "xiaomi-region" });
  select.appendChild(el("option", { value: "cn" }, "🇨🇳 中国 (token-plan-cn)"));
  select.appendChild(el("option", { value: "sgp" }, "🌏 新加坡 (token-plan-sgp)"));
  select.appendChild(el("option", { value: "ams" }, "🌍 欧洲 (token-plan-ams)"));
  select.value = current;
  return el(
    "div",
    { class: "field" },
    el("label", {}, "Xiaomi MiMo 集群"),
    el("div", { class: "help" }, "集群字段当前用于未来扩展（多账号 / region-specific URL）。当前 cookie auth 已绑定 user，无视。"),
    select,
  );
}

/// Tavily 简洁模式 checkbox。从 cfg.tavily_concise_mode 读初值。
function renderConciseModeCheckbox(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const checked = cfg.tavily_concise_mode ?? true;
  const cb = el("input", {
    type: "checkbox",
    id: "tavily-concise-mode",
    "data-id": "tavily-concise-mode",
  }) as HTMLInputElement;
  cb.checked = checked;

  return el(
    "div",
    { class: "field" },
    el(
      "label",
      {},
      "浮窗显示",
    ),
    el(
      "div",
      { class: "check" },
      cb,
      el(
        "label",
        { for: "tavily-concise-mode" },
        "简洁模式（只显示主指标 \"X/Y credits\" + 进度条，隐藏 5 个 endpoint 细分）",
      ),
    ),
    el(
      "div",
      { class: "help" },
      "默认开启 —— 6 行在小浮窗里太挤。关掉后会显示 search/extract/crawl/map/research 五个 endpoint 的细分行。改完点「保存配置」生效。",
    ),
  );
}

/// ZenMux 自定义 base URL。从 cfg.zenmux_base_url 读初值。
function renderBaseUrlInput(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const value = cfg.zenmux_base_url ?? "";
  const input = el("input", {
    type: "text",
    id: "zenmux-base-url",
    "data-id": "zenmux-base-url",
    placeholder: "默认 https://zenmux.ai/api/v1/management/payg/balance",
    autocomplete: "off",
  }) as HTMLInputElement;
  input.value = value;
  return el(
    "div",
    { class: "field" },
    el("label", {}, "自定义 API URL（可选）"),
    el("div", { class: "input-row" }, input),
    el(
      "div",
      { class: "help" },
      "默认走 mode 选的端点（PAYG → /payg/balance，订阅 → /subscription/detail）。自部署 / 改了路径才需要改；留空用默认。",
    ),
  );
}

/// ZenMux 查看模式（payg / subscription）+ payg 简洁 checkbox
function renderZenmuxMode(_meta: SourceMeta, _cfg: AppConfig): HTMLElement {
  // Stage 4 暂保留硬编码的初始值（注意：动态读 cfg 还需要 zhipu 端加对应字段）
  // TODO Stage 5 接 cfg.zenmux_mode
  const select = el("select", { id: "zenmux-mode" });
  select.appendChild(el("option", { value: "payg" }, "💰 钱包余额（PAYG，默认）"));
  select.appendChild(el("option", { value: "subscription" }, "📊 订阅用量（5h / 7d）"));
  select.value = "payg";

  const cb = el("input", {
    type: "checkbox",
    id: "zenmux-payg-concise-mode",
    checked: "checked",
  }) as HTMLInputElement;
  cb.checked = true;

  return el(
    "div",
    { class: "field" },
    el("label", {}, "查看模式"),
    el("div", { class: "input-row" }, select),
    el(
      "div",
      { class: "help" },
      "PAYG：监控 Pay-As-You-Go 钱包余额（仿 DeepSeek）。订阅：监控 5h / 7d 滚动窗口的订阅用量（仿 MiniMax）。",
    ),
    el(
      "div",
      { class: "check", id: "zenmux-payg-concise-wrap", style: "margin-top: 8px;" },
      cb,
      el("label", { for: "zenmux-payg-concise-mode" }, "只显示余额（不显示充值 / 奖励）"),
    ),
  );
}
