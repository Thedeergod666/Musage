// EXTRAS 表：id → 额外 UI 块
//
// 每个 source 在 createProviderPanel() 渲染 header + credentials 之后，
// 会按 EXTRAS[id] 顺序插入这些块（region select / cookie 字段 /
// concise mode checkbox / base url 输入框 等）。
//
// **加新 provider 改这里 + builtin_sources() 即可**，不用动 providers.ts
// 主流程。
//
// C3 fix: 6 个交互控件（region select / concise checkbox / base_url / mode /
// zhipu region）全部加 change listener + 调对应 per-field setter
// （src/settings/api.ts:7 个 setXxx）。之前用户改了值，set_state 不会
// 触发 → 配置改完静默丢失，必须重启 app 才生效。Stage 4 删了"保存"
// 按钮但 Stage 6 的即时生效改造没做，已在这里补齐。

import { el, flash } from "./utils";
import { t } from "../i18n";
import {
  setMinimaxRegion,
  setXiaomiRegion,
  setTavilyConciseMode,
  setZenmuxBaseUrl,
  setZenmuxMode,
  setZenmuxPaygConcise,
  setZhipuRegion,
} from "./api";
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
    el("option", { value: "cn" }, t("extras.minimax_region_cn")),
  );
  select.appendChild(
    el("option", { value: "en" }, t("extras.minimax_region_en")),
  );
  select.value = current;
  // C3 fix: 即时生效 —— change → set_minimax_region（后端落盘 + emit + refresh）
  select.addEventListener("change", () => {
    const v = select.value as "cn" | "en";
    if (v !== "cn" && v !== "en") return;
    void setMinimaxRegion(v).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });

  return el(
    "div",
    { class: "field" },
    el("label", {}, t("extras.minimax_region_label")),
    select,
  );
}

/// Xiaomi MiMo 集群选择（cn / sgp / ams）。从 cfg.xiaomi_region 读初值。
function renderXiaomiRegionSelect(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const current = cfg.providers?.xiaomimimo?.xiaomi_region ?? "cn";
  const select = el("select", { "data-id": "xiaomi-region", id: "xiaomi-region" });
  select.appendChild(el("option", { value: "cn" }, t("extras.xiaomi_region_cn")));
  select.appendChild(el("option", { value: "sgp" }, t("extras.xiaomi_region_sgp")));
  select.appendChild(el("option", { value: "ams" }, t("extras.xiaomi_region_ams")));
  select.value = current;
  select.addEventListener("change", () => {
    const v = select.value as "cn" | "sgp" | "ams";
    if (v !== "cn" && v !== "sgp" && v !== "ams") return;
    void setXiaomiRegion(v).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });
  return el(
    "div",
    { class: "field" },
    el("label", {}, t("extras.xiaomi_region_label")),
    el("div", { class: "help" }, t("extras.xiaomi_region_help")),
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
  cb.addEventListener("change", () => {
    void setTavilyConciseMode(cb.checked).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });

  return el(
    "div",
    { class: "field" },
    el(
      "label",
      {},
      t("extras.tavily_concise_label"),
    ),
    el(
      "div",
      { class: "check" },
      cb,
      el(
        "label",
        { for: "tavily-concise-mode" },
        t("extras.tavily_concise_checkbox"),
      ),
    ),
    el(
      "div",
      { class: "help" },
      t("extras.tavily_concise_help"),
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
    placeholder: t("extras.zenmux_base_url_placeholder"),
    autocomplete: "off",
  }) as HTMLInputElement;
  input.value = value;
  // C3 fix: input 失焦后落盘 + refresh（避免每个按键就 IPC）
  input.addEventListener("change", () => {
    void setZenmuxBaseUrl(input.value.trim()).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });
  return el(
    "div",
    { class: "field" },
    el("label", {}, t("extras.zenmux_base_url_label")),
    el("div", { class: "input-row" }, input),
    el(
      "div",
      { class: "help" },
      t("extras.zenmux_base_url_help"),
    ),
  );
}

/// ZenMux 查看模式（payg / subscription）+ payg 简洁 checkbox
function renderZenmuxMode(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  // 修 hardcoded: 之前写死 "payg" 永远不反映用户改的值（注释自己写了 TODO Stage 5）
  const currentMode = cfg.zenmux_mode ?? "payg";
  const select = el("select", { id: "zenmux-mode", "data-id": "zenmux-mode" });
  select.appendChild(el("option", { value: "payg" }, t("extras.zenmux_mode_payg")));
  select.appendChild(el("option", { value: "subscription" }, t("extras.zenmux_mode_subscription")));
  select.value = currentMode;
  select.addEventListener("change", () => {
    const v = select.value as "payg" | "subscription";
    if (v !== "payg" && v !== "subscription") return;
    void setZenmuxMode(v).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });

  const cb = el("input", {
    type: "checkbox",
    id: "zenmux-payg-concise-mode",
    "data-id": "zenmux-payg-concise",
  }) as HTMLInputElement;
  cb.checked = cfg.zenmux_payg_concise_mode ?? true;
  cb.addEventListener("change", () => {
    void setZenmuxPaygConcise(cb.checked).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });

  return el(
    "div",
    { class: "field" },
    el("label", {}, t("extras.zenmux_mode_label")),
    el("div", { class: "input-row" }, select),
    el(
      "div",
      { class: "help" },
      t("extras.zenmux_mode_help"),
    ),
    el(
      "div",
      { class: "check", id: "zenmux-payg-concise-wrap", style: "margin-top: 8px;" },
      cb,
      el("label", { for: "zenmux-payg-concise-mode" }, t("extras.zenmux_payg_concise_label")),
    ),
  );
}

/// OpenRouter 帮助文案（无需额外字段，只需说明 key 格式）
function renderOpenrouterHelp(_meta: SourceMeta, _cfg: AppConfig): HTMLElement {
  // P1 fix: 之前英文版 baseText 已经内含 "GET /api/v1/key."（重复渲染端点 URL），
  // 且末尾硬编码中文句号 '。'。统一改成 baseText 只放描述，链接是单独 element。
  // 句号走 t() 拿当前 locale 的句号（en=".", zh="。"）。
  const baseText = t("extras.openrouter_help_text");
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
      // M9 fix: 之前用 t("common.punctuation_period") 但该 key 在
      // en.json/zh-CN.json 里只存在于 settings.common.punctuation_period，
      // 找不到的 key 走 fallback 会原样回退成 raw key 字符串。
      t("settings.common.punctuation_period"),
    ),
  );
}

/// 智谱 GLM 区域选择（cn = 国区 open.bigmodel.cn，en = 国际 api.z.ai）。
/// schema 完全一致，区别只是 host + API key 在两个平台分开创建。
function renderZhipuRegionSelect(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const current = cfg.zhipu_region ?? "cn";
  const select = el("select", { id: "zhipu-region", "data-id": "zhipu-region" });
  select.appendChild(el("option", { value: "cn" }, t("extras.zhipu_region_cn")));
  select.appendChild(el("option", { value: "en" }, t("extras.zhipu_region_en")));
  select.value = current;
  select.addEventListener("change", () => {
    const v = select.value as "cn" | "en";
    if (v !== "cn" && v !== "en") return;
    void setZhipuRegion(v).catch((e) => {
      flash(t("settings.app.switch_failed", { err: String(e) }), true);
    });
  });

  const helpDiv = document.createElement("div");
  helpDiv.className = "help";
  helpDiv.innerHTML = t("extras.zhipu_region_help");

  return el(
    "div",
    { class: "field" },
    el("label", { for: "zhipu-region" }, t("extras.zhipu_region_label")),
    select,
    helpDiv,
  );
}
