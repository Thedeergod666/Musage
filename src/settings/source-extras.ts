// EXTRAS 表：id → 额外 UI 块
//
// 每个 source 在 createProviderPanel() 渲染 header + credentials 之后，
// 会按 EXTRAS[id] 顺序插入这些块（region select / cookie 字段 /
// concise mode checkbox / base url 输入框 等）。
//
// **加新 provider 改这里 + builtin_sources() 即可**，不用动 providers.ts
// 主流程。

import { el } from "./utils";
import { t } from "../i18n";
import type { AppConfig, SourceMeta } from "./types";

export type ExtraBlock = (meta: SourceMeta, cfg: AppConfig) => HTMLElement;

/// 静态表 —— key 是 source.id，value 是要插入的额外块工厂。
/// 找不到的 id 返回空数组（deepseek 就是这样）。
const EXTRAS: Record<string, ExtraBlock[]> = {
  minimax: [renderRegionSelect],
  xiaomimimo: [renderXiaomiRegionSelect],
  tavily: [renderConciseModeCheckbox],
  zenmux: [renderBaseUrlInput, renderZenmuxMode],
  openrouter: [renderOpenrouterHelp],
  zhipu: [renderZhipuRegionSelect],
  // deepseek / kimi: 无额外字段
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
    el("option", { value: "cn" }, t("settings.extras.minimax_region_cn")),
  );
  select.appendChild(
    el("option", { value: "en" }, t("settings.extras.minimax_region_en")),
  );
  select.value = current;
  // 即时生效：change → save_config（最简版 v1 仍走总保存，但 select 是 Stage 5 才换即时）
  // 这里暂不绑 change，避免 Stage 4 引入新行为

  return el(
    "div",
    { class: "field" },
    el("label", {}, t("settings.extras.minimax_region_label")),
    select,
  );
}

/// Xiaomi MiMo 集群选择（cn / sgp / ams）。从 cfg.xiaomi_region 读初值。
function renderXiaomiRegionSelect(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const current = cfg.providers?.xiaomimimo?.xiaomi_region ?? "cn";
  const select = el("select", { "data-id": "xiaomi-region", id: "xiaomi-region" });
  select.appendChild(el("option", { value: "cn" }, t("settings.extras.xiaomi_region_cn")));
  select.appendChild(el("option", { value: "sgp" }, t("settings.extras.xiaomi_region_sgp")));
  select.appendChild(el("option", { value: "ams" }, t("settings.extras.xiaomi_region_ams")));
  select.value = current;
  return el(
    "div",
    { class: "field" },
    el("label", {}, t("settings.extras.xiaomi_region_label")),
    el("div", { class: "help" }, t("settings.extras.xiaomi_region_help")),
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
      t("settings.extras.tavily_concise_label"),
    ),
    el(
      "div",
      { class: "check" },
      cb,
      el(
        "label",
        { for: "tavily-concise-mode" },
        t("settings.extras.tavily_concise_checkbox"),
      ),
    ),
    el(
      "div",
      { class: "help" },
      t("settings.extras.tavily_concise_help"),
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
    placeholder: t("settings.extras.zenmux_base_url_placeholder"),
    autocomplete: "off",
  }) as HTMLInputElement;
  input.value = value;
  return el(
    "div",
    { class: "field" },
    el("label", {}, t("settings.extras.zenmux_base_url_label")),
    el("div", { class: "input-row" }, input),
    el(
      "div",
      { class: "help" },
      t("settings.extras.zenmux_base_url_help"),
    ),
  );
}

/// ZenMux 查看模式（payg / subscription）+ payg 简洁 checkbox
function renderZenmuxMode(_meta: SourceMeta, _cfg: AppConfig): HTMLElement {
  // Stage 4 暂保留硬编码的初始值（注意：动态读 cfg 还需要 zhipu 端加对应字段）
  // TODO Stage 5 接 cfg.zenmux_mode
  const select = el("select", { id: "zenmux-mode" });
  select.appendChild(el("option", { value: "payg" }, t("settings.extras.zenmux_mode_payg")));
  select.appendChild(el("option", { value: "subscription" }, t("settings.extras.zenmux_mode_subscription")));
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
    el("label", {}, t("settings.extras.zenmux_mode_label")),
    el("div", { class: "input-row" }, select),
    el(
      "div",
      { class: "help" },
      t("settings.extras.zenmux_mode_help"),
    ),
    el(
      "div",
      { class: "check", id: "zenmux-payg-concise-wrap", style: "margin-top: 8px;" },
      cb,
      el("label", { for: "zenmux-payg-concise-mode" }, t("settings.extras.zenmux_payg_concise_label")),
    ),
  );
}

/// OpenRouter 帮助文案（无需额外字段，只需说明 key 格式）
function renderOpenrouterHelp(_meta: SourceMeta, _cfg: AppConfig): HTMLElement {
  // en.json: "OpenRouter 余额 = 账户 credit 余额。普通 API key（`sk-or-v1-...`）即可，不需要 Management key。端点 GET /api/v1/key。"
  // zh-CN.json: "OpenRouter 余额 = 账户 credit 余额。普通 API key（`sk-or-v1-...`）即可，不需要 Management key。端点 " + link
  const baseText = t("settings.extras.openrouter_help_text");
  // 中文版在末尾用空格 + 链接，英文版自带结尾。这里统一：基础文字 + 句点后接 link。
  // 注意：英文版的句点已在 baseText 末尾，中文版没有，所以拆开处理。
  const link = el("a", {
    href: "https://openrouter.ai/docs/api/reference/limits",
    target: "_blank",
    class: "link-ext",
  }, "GET /api/v1/key");
  return el(
    "div",
    { class: "field" },
    el(
      "div",
      { class: "help" },
      baseText,
      link,
      "。",
    ),
  );
}

/// 智谱 GLM 区域选择（cn = 国区 open.bigmodel.cn，en = 国际 api.z.ai）。
/// schema 完全一致，区别只是 host + API key 在两个平台分开创建。
function renderZhipuRegionSelect(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const current = cfg.zhipu_region ?? "cn";
  const select = el("select", { id: "zhipu-region", "data-id": "zhipu-region" });
  select.appendChild(el("option", { value: "cn" }, t("settings.extras.zhipu_region_cn")));
  select.appendChild(el("option", { value: "en" }, t("settings.extras.zhipu_region_en")));
  select.value = current;

  const helpDiv = document.createElement("div");
  helpDiv.className = "help";
  helpDiv.innerHTML = t("settings.extras.zhipu_region_help");

  return el(
    "div",
    { class: "field" },
    el("label", { for: "zhipu-region" }, t("settings.extras.zhipu_region_label")),
    select,
    helpDiv,
  );
}
