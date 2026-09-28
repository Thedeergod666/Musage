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
  setVolcengineArkPlanCoding,
  setVolcengineArkPlanAgent,
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
  // v0.2.9：Coding Plan / Agent Plan 双套餐筛选 checkbox
  volcengine_ark: [renderVolcenginePlanFilter],
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
  // H-Frontend-4 fix (2026-09-28 audit)：M33 的"回滚到改前值"是**空操作** ——
  // `const previous = select.value` 是在 change 回调**内部**读的，而 change
  // 派发时控件值已被用户改掉了 → previous === v，回滚等于什么都没做。触发：
  // IPC 失败 → 红条「切换失败」，但控件仍停在新值、后端仍是旧值，用户以为
  // 改了。改用 app.ts 的 lastGood 闭包可变模式（本文件 7 处同款）。
  let lastGoodRegion = current;
  select.addEventListener("change", () => {
    const v = select.value as "cn" | "en";
    if (v !== "cn" && v !== "en") return;
    void setMinimaxRegion(v)
      .then(() => { lastGoodRegion = v; }) // 只在成功后更新最近成功值
      .catch((e) => {
        select.value = lastGoodRegion;
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
  // H-Frontend-4 fix (2026-09-28 audit)：M33 回滚是空操作（previous 在 change
  // 内读 === 新值）。改 lastGood 闭包可变，详见 renderRegionSelect 同款注释。
  let lastGoodXiaomiRegion = current;
  select.addEventListener("change", () => {
    const v = select.value as "cn" | "sgp" | "ams";
    if (v !== "cn" && v !== "sgp" && v !== "ams") return;
    void setXiaomiRegion(v)
      .then(() => { lastGoodXiaomiRegion = v; })
      .catch((e) => {
        select.value = lastGoodXiaomiRegion;
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
  // H-Frontend-4 fix (2026-09-28 audit)：M33 回滚是空操作（previous 在 change
  // 内读 === 浏览器已翻过的新值）。改 lastGood 闭包可变。
  let lastGoodConcise = checked;
  cb.addEventListener("change", () => {
    const v = cb.checked;
    void setTavilyConciseMode(v)
      .then(() => { lastGoodConcise = v; })
      .catch((e) => {
        cb.checked = lastGoodConcise;
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
  // H-Frontend-4 fix (2026-09-28 audit)：M33 回滚是空操作（previous 在 change
  // 内读 === 新值）。改 lastGood 闭包可变。
  let lastGoodBaseUrl = value;
  input.addEventListener("change", () => {
    const v = input.value.trim();
    // L-3 fix (2026-09-05 audit)：前端先做同款 https:// 前缀校验 —— 后端只收
    // https://，`http://` 直送 IPC 只能收到晦涩后端报错。
    if (v && !v.startsWith("https://")) {
      input.value = lastGoodBaseUrl;
      flash(t("extras.zenmux_base_url_invalid"), true);
      return;
    }
    void setZenmuxBaseUrl(v)
      .then(() => { lastGoodBaseUrl = v; })
      .catch((e) => {
        input.value = lastGoodBaseUrl;
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
  // H-Frontend-4 fix (2026-09-28 audit)：M33 回滚是空操作。改 lastGood 闭包可变。
  let lastGoodMode = currentMode;
  select.addEventListener("change", () => {
    const v = select.value as "payg" | "subscription";
    if (v !== "payg" && v !== "subscription") return;
    void setZenmuxMode(v)
      .then(() => { lastGoodMode = v; })
      .catch((e) => {
        select.value = lastGoodMode;
        flash(t("settings.app.switch_failed", { err: String(e) }), true);
      });
  });

  const cb = el("input", {
    type: "checkbox",
    id: "zenmux-payg-concise-mode",
    "data-id": "zenmux-payg-concise",
  }) as HTMLInputElement;
  const paygConciseInit = cfg.zenmux_payg_concise_mode ?? true;
  cb.checked = paygConciseInit;
  // H-Frontend-4 fix (2026-09-28 audit)：M33 回滚是空操作。改 lastGood 闭包可变。
  let lastGoodPaygConcise = paygConciseInit;
  cb.addEventListener("change", () => {
    const v = cb.checked;
    void setZenmuxPaygConcise(v)
      .then(() => { lastGoodPaygConcise = v; })
      .catch((e) => {
        cb.checked = lastGoodPaygConcise;
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
  // H-Frontend-4 fix (2026-09-28 audit)：M33 回滚是空操作。改 lastGood 闭包可变。
  let lastGoodZhipuRegion = current;
  select.addEventListener("change", () => {
    const v = select.value as "cn" | "en";
    if (v !== "cn" && v !== "en") return;
    void setZhipuRegion(v)
      .then(() => { lastGoodZhipuRegion = v; })
      .catch((e) => {
        select.value = lastGoodZhipuRegion;
        flash(t("settings.app.switch_failed", { err: String(e) }), true);
      });
  });

  const helpDiv = document.createElement("div");
  helpDiv.className = "help";
  // D7-005 fix (2026-07-30 audit): 对齐 advanced.ts M5 fix (2026-07-06)
  // 干掉 innerHTML = t(...) 模式, 改 textContent. zhipu_region_help 当前
  // JSON 值含硬编码 <a href> / <strong>, 改为 textContent 后链接以纯文本
  // 显示, 用户手抄 URL 即可. 后续如要恢复可点击链接, 改用 el() 拆 DOM
  // + DOMPurify sanitize(白名单 anchor) 路径.
  helpDiv.textContent = t("extras.zhipu_region_help");

  return el(
    "div",
    { class: "field" },
    el("label", { for: "zhipu-region" }, t("extras.zhipu_region_label")),
    select,
    helpDiv,
  );
}

/// 火山方舟双套餐筛选（v0.2.9）：Coding Plan / Agent Plan 两个独立 checkbox。
/// 同一 AppID 可同时买两份套餐，未勾选的 action 后端直接不打（省配额）。
/// 从 cfg.volcengine_ark_plan_filter 读初值（缺省两个都勾）。
function renderVolcenginePlanFilter(_meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const cur = cfg.volcengine_ark_plan_filter ?? { coding: true, agent: true };

  const mkCheckbox = (
    id: string,
    checked: boolean,
    labelText: string,
    onChange: (v: boolean) => Promise<void>,
  ): HTMLElement => {
    const cb = el("input", {
      type: "checkbox",
      id,
      "data-id": id,
    }) as HTMLInputElement;
    cb.checked = checked;
    // H-Frontend-4 fix (2026-09-28 audit)：同款空操作/无回滚 bug —— 失败时只
    // flash，勾留在新值而后端仍是旧值。改 lastGood 闭包可变。
    let lastGood = checked;
    cb.addEventListener("change", () => {
      const v = cb.checked;
      void onChange(v)
        .then(() => { lastGood = v; })
        .catch((e) => {
          cb.checked = lastGood;
          flash(t("settings.app.switch_failed", { err: String(e) }), true);
        });
    });
    return el("div", { class: "check" }, cb, el("label", { for: id }, labelText));
  };

  return el(
    "div",
    { class: "field" },
    el("label", {}, t("extras.volcengine_plan_filter_label")),
    mkCheckbox(
      "volcengine-plan-coding",
      cur.coding,
      t("extras.volcengine_plan_coding"),
      setVolcengineArkPlanCoding,
    ),
    mkCheckbox(
      "volcengine-plan-agent",
      cur.agent,
      t("extras.volcengine_plan_agent"),
      setVolcengineArkPlanAgent,
    ),
    el("div", { class: "help" }, t("extras.volcengine_plan_help")),
  );
}
