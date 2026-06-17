// "添加自定义 New API 来源" modal
//
// PR 3 核心 UI：
// - 1 个 `<dialog>` 弹窗
// - 表单：display_name / base_url / path / method (GET/POST) / 3 选 1 extract preset
//         / 动态字段（按 preset 显示 balance_path 等）/ accent 调色板 / API key
// - 「测试连接」按钮：调 testCustomSource，成功后才允许「保存」
// - 「保存」：调 addCustomSource → 后端立即 refresh_single → 浮窗立即出数据

import { el, flash } from "./utils";
import { showModal } from "./modal";
import {
  addCustomSource,
  testCustomSource,
  listCustomSources,
} from "./api";
import { t } from "../i18n";
import type { CustomSourceSpec, ExtractSpec } from "./types";
import { renderProvidersSection } from "./providers";

const ACCENT_PALETTE = [
  "#9b59ff",
  "#4a90e2",
  "#ff6a00",
  "#00d4a8",
  "#5ac8fa",
  "#ff6b35",
  "#9333ea",
  "#d97706",
];

/** 「+ 添加自定义来源」按钮绑的事件入口 */
export function openAddCustomSourceModal(): void {
  const body = buildForm();
  showModal({
    title: t("custom_source.title"),
    body,
    submitLabel: t("settings.common.save"),
    onSubmit: async () => {
      return submitHandler(body);
    },
  });
}

// ── 内部 ──────────────────────────────────────────────────────

/** 构造表单 body，存所有字段引用到 dataset。 */
function buildForm(): HTMLElement {
  const root = el("div", { class: "custom-source-form" });

  // display_name
  root.appendChild(
    field("display_name", t("custom_source.field.display_name"), el("input", {
      type: "text",
      id: "cs-name",
      placeholder: t("custom_source.field.display_name_placeholder"),
      required: "true",
      autocomplete: "off",
    })),
  );

  // base_url
  root.appendChild(
    field("base_url", t("custom_source.field.base_url"), el("input", {
      type: "url",
      id: "cs-base",
      placeholder: t("custom_source.field.base_url_placeholder"),
      required: "true",
      autocomplete: "off",
    })),
  );

  // path
  root.appendChild(
    field("path", t("custom_source.field.path"), el("input", {
      type: "text",
      id: "cs-path",
      placeholder: t("custom_source.field.path_placeholder"),
      required: "true",
      autocomplete: "off",
    })),
  );

  // method (radio GET / POST)
  const methodGroup = el("div", { class: "field" },
    el("label", {}, t("custom_source.field.method")),
    el("div", { class: "radio-group" },
      radio("cs-method", "GET", true),
      radio("cs-method", "POST", false),
    ),
  );
  root.appendChild(methodGroup);

  // extract preset (3 选 1 radio)
  const presetGroup = el("div", { class: "field" },
    el("label", {}, t("custom_source.field.extract_preset")),
    el("div", { class: "radio-group" },
      radio("cs-preset", "new_api", true, t("custom_source.preset.new_api")),
      radio("cs-preset", "balance", false, t("custom_source.preset.balance")),
      radio("cs-preset", "custom", false, t("custom_source.preset.custom")),
    ),
    el("div", { class: "help" },
      t("custom_source.preset_help"),
    ),
  );
  root.appendChild(presetGroup);

  // 动态字段容器：按 preset 切换
  const dynamicFields = el("div", { id: "cs-dynamic-fields" });
  root.appendChild(dynamicFields);

  // plan_name_path（可选）
  root.appendChild(
    field("plan_name_path", t("custom_source.field.plan_name"), el("input", {
      type: "text",
      id: "cs-plan",
      placeholder: t("custom_source.field.plan_name_placeholder"),
      autocomplete: "off",
    })),
  );

  // accent 调色板
  const accentGroup = el("div", { class: "field" },
    el("label", {}, t("custom_source.field.accent")),
    el("div", { class: "accent-palette", id: "cs-accent-palette" },
      ...ACCENT_PALETTE.map((c) => el("button", {
        type: "button",
        class: "accent-swatch",
        "data-color": c,
        style: `background: ${c};`,
        title: c,
      })),
    ),
    el("div", { class: "help" }, t("custom_source.field.accent_help")),
  );
  root.appendChild(accentGroup);

  // API key
  root.appendChild(
    field("api_key", t("custom_source.field.api_key"), el("input", {
      type: "password",
      id: "cs-api-key",
      placeholder: t("custom_source.field.api_key_placeholder"),
      autocomplete: "off",
    })),
  );

  // preset change → 动态字段
  root.addEventListener("change", (e) => {
    const t = e.target as HTMLInputElement;
    if (t.name === "cs-preset") renderDynamicFields(t.value);
    if (t.classList.contains("accent-swatch")) {
      // 高亮选中
      root
        .querySelectorAll<HTMLElement>(".accent-swatch")
        .forEach((s) => s.classList.remove("selected"));
      t.classList.add("selected");
    }
  });

  // 初始 NewApi 字段
  renderDynamicFields("new_api");

  return root;
}

function field(_name: string, label: string, input: HTMLElement): HTMLElement {
  return el("div", { class: "field" },
    el("label", { for: input.id }, label),
    el("div", { class: "input-row" }, input),
  );
}

function radio(name: string, value: string, checked: boolean, label?: string): HTMLElement {
  const id = `${name}-${value}`;
  // 注意：input 必须 margin: 0 + flex-shrink: 0，否则原生 radio 的
  // "clickable area" 会把旁边的文字推开 / 内部点遮住字母（之前 GET 被
  // 渲染成 GOT 的根因）。
  return el("label", { class: "radio", for: id },
    el("input", {
      type: "radio",
      name,
      id,
      value,
      ...(checked ? { checked: "true" } : {}),
    }),
    el("span", { class: "radio-label" }, label ?? value),
  );
}

/** 按 preset 渲染 dynamic fields（替换 #cs-dynamic-fields 内容） */
function renderDynamicFields(preset: string): void {
  const host = document.getElementById("cs-dynamic-fields");
  if (!host) return;
  host.innerHTML = "";

  if (preset === "new_api") {
    host.appendChild(
      field("divide", t("custom_source.field.divide", { value: "500000" }), el("input", {
        type: "number",
        id: "cs-divide",
        value: "500000",
        step: "1",
        min: "0",
      })),
    );
  } else if (preset === "balance") {
    host.appendChild(
      field("balance_path", t("custom_source.field.balance_path"), el("input", {
        type: "text",
        id: "cs-balance-path",
        placeholder: t("custom_source.field.balance_path_placeholder"),
        required: "true",
      })),
    );
    host.appendChild(
      field("currency_path", t("custom_source.field.currency_path"), el("input", {
        type: "text",
        id: "cs-currency-path",
        placeholder: t("custom_source.field.currency_path_placeholder"),
      })),
    );
    host.appendChild(
      field("divide_balance", t("custom_source.field.divide", { value: "1.0" }), el("input", {
        type: "number",
        id: "cs-divide",
        value: "1.0",
        step: "0.01",
        min: "0",
      })),
    );
  } else if (preset === "custom") {
    host.appendChild(
      field("remaining_path", t("custom_source.field.remaining_path"), el("input", {
        type: "text",
        id: "cs-remaining-path",
        placeholder: t("custom_source.field.remaining_path_placeholder"),
      })),
    );
    host.appendChild(
      field("used_path", t("custom_source.field.used_path"), el("input", {
        type: "text",
        id: "cs-used-path",
        placeholder: t("custom_source.field.used_path_placeholder"),
      })),
    );
    host.appendChild(
      field("total_path", t("custom_source.field.total_path"), el("input", {
        type: "text",
        id: "cs-total-path",
        placeholder: t("custom_source.field.total_path_placeholder"),
      })),
    );
    host.appendChild(
      field("unit", t("custom_source.field.unit"), el("input", {
        type: "text",
        id: "cs-unit",
        placeholder: t("custom_source.field.unit_placeholder"),
      })),
    );
    host.appendChild(
      field("divide_custom", t("custom_source.field.divide", { value: "1.0" }), el("input", {
        type: "number",
        id: "cs-divide",
        value: "1.0",
        step: "0.01",
        min: "0",
      })),
    );
  }
}

async function submitHandler(body: HTMLElement): Promise<boolean> {
  // 1. 收集字段
  const displayName = (body.querySelector<HTMLInputElement>("#cs-name")!.value ?? "").trim();
  const baseUrl = (body.querySelector<HTMLInputElement>("#cs-base")!.value ?? "").trim();
  const path = (body.querySelector<HTMLInputElement>("#cs-path")!.value ?? "").trim();
  const method = (body.querySelector<HTMLInputElement>('input[name="cs-method"]:checked')!.value ?? "GET");
  const preset = (body.querySelector<HTMLInputElement>('input[name="cs-preset"]:checked')!.value ?? "new_api");
  const apiKey = (body.querySelector<HTMLInputElement>("#cs-api-key")!.value ?? "").trim();
  const planNamePath = (body.querySelector<HTMLInputElement>("#cs-plan")!.value ?? "").trim();
  const accentEl = body.querySelector<HTMLElement>(".accent-swatch.selected");
  const accent = accentEl ? accentEl.dataset.color ?? null : null;

  // 2. 前端基本校验
  if (!displayName) { flash(t("custom_source.err.name_required"), true); return false; }
  if (!baseUrl.startsWith("http://") && !baseUrl.startsWith("https://")) {
    flash(t("custom_source.err.base_url_invalid"), true);
    return false;
  }
  if (!path.startsWith("/")) {
    flash(t("custom_source.err.path_invalid"), true);
    return false;
  }
  if (!apiKey) { flash(t("custom_source.err.api_key_required"), true); return false; }

  // 3. 构造 ExtractSpec
  const divideRaw = (body.querySelector<HTMLInputElement>("#cs-divide")?.value ?? "").trim();
  const divide = divideRaw === "" ? null : Number(divideRaw);
  let extract: ExtractSpec;
  if (preset === "new_api") {
    extract = { preset: "new_api", divide };
  } else if (preset === "balance") {
    const balancePath = (body.querySelector<HTMLInputElement>("#cs-balance-path")!.value ?? "").trim();
    if (!balancePath) { flash(t("custom_source.err.balance_path_required"), true); return false; }
    const currencyPath = (body.querySelector<HTMLInputElement>("#cs-currency-path")!.value ?? "").trim();
    extract = {
      preset: "balance",
      balance_path: balancePath,
      currency_path: currencyPath || null,
      divide,
    };
  } else {
    // custom
    extract = {
      preset: "custom",
      remaining_path: (body.querySelector<HTMLInputElement>("#cs-remaining-path")?.value ?? "").trim() || null,
      used_path: (body.querySelector<HTMLInputElement>("#cs-used-path")?.value ?? "").trim() || null,
      total_path: (body.querySelector<HTMLInputElement>("#cs-total-path")?.value ?? "").trim() || null,
      unit: (body.querySelector<HTMLInputElement>("#cs-unit")?.value ?? "").trim() || null,
      divide,
    };
  }

  // 4. 测试连接（不在 store 时跳过 test，让后端 add 失败也 OK）
  try {
    const testSpec: Omit<CustomSourceSpec, "id" | "created_at"> = {
      display_name: displayName,
      base_url: baseUrl,
      path,
      method: method as "GET" | "POST",
      extract,
      plan_name_path: planNamePath || null,
      accent,
    };
    const snap = await testCustomSource(testSpec, apiKey);
    if (!snap.success) {
      flash(t("custom_source.err.test_failed", { err: snap.error ?? t("floating.error.unknown") }), true);
      return false;
    }
    flash(t("custom_source.test_passing"));
  } catch (e) {
    flash(t("custom_source.err.test_error", { err: String(e) }), true);
    return false;
  }

  // 5. 保存
  try {
    const id = await addCustomSource({
      display_name: displayName,
      base_url: baseUrl,
      path,
      method: method as "GET" | "POST",
      extract,
      plan_name_path: planNamePath || null,
      accent,
    });
    flash(t("custom_source.added", { name: displayName, id }));
    // 重建 settings providers section
    const container = document.querySelector<HTMLElement>(
      '.section-view[data-section="providers"]',
    );
    if (container) {
      // 防再 fetch listCustomSources 漏 customs 数量变化
      await listCustomSources();  // warm
      await renderProvidersSection(container);
    }
    return true;
  } catch (e) {
    flash(t("custom_source.err.save_failed", { err: String(e) }), true);
    return false;
  }
}
