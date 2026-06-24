// "添加新来源" modal（PR 1b）
//
// PR 3 → PR 1b 重构：
// - PR 3：只支持 New API 中转站（custom）
// - PR 1b：两段式表单
//   - Step 1：provider picker（11 内置下拉 + 1 custom 选项）
//   - Step 2A（内置）：只显示 API key 输入框
//   - Step 2B（custom）：原 3 选 1 Extract 模板（New API / 余额 / 自定义）
// - 提交：调 add_extra_instance(provider_id, api_key, custom?) —— 后端自动算 instance_index
//
// 老的 openAddCustomSourceModal / buildForm 等导出**已删除**（PR 1b 砍），
// 前端只通过新入口 openAddExtraInstanceModal 进入。

import { el, flash } from "./utils";
import { showModal } from "./modal";
import {
  addExtraInstance,
  testExtraInstance,
  listExtraInstances,
  listPickerProviders,
} from "./api";
import { t } from "../i18n";
import type {
  CustomSourceSpec,
  ExtractSpec,
  PickerProvider,
} from "./types";
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

/** 「+ 添加新来源」按钮绑的事件入口 */
export async function openAddExtraInstanceModal(): Promise<void> {
  // 1. 拉 provider picker 数据
  let providers: PickerProvider[];
  try {
    providers = await listPickerProviders();
  } catch (e) {
    flash(t("settings.providers.add_load_picker_failed", { err: String(e) }), true);
    return;
  }
  // 2. 构造表单
  const body = buildForm(providers);
  showModal({
    title: t("settings.providers.add_source"),
    body,
    submitLabel: t("settings.common.save"),
    onSubmit: async () => submitHandler(body),
  });
}

// ── 内部 ──────────────────────────────────────────────────────

/** 构造两段式表单 body。 */
function buildForm(providers: PickerProvider[]): HTMLElement {
  const root = el("div", { class: "extra-instance-form" });

  // Step 1: provider picker
  const pickerField = el("div", { class: "field" },
    el("label", {}, t("extra.form.provider_type_label")),
    el("select", {
      id: "ei-provider",
      class: "provider-picker",
    },
      ...providers.map((p) => el("option", {
        value: p.id,
        "data-is-builtin": String(p.is_builtin),
      }, t(p.name_key as any) as string)),
    ),
    el("div", { class: "help" },
      t("extra.form.provider_type_help")),
  );
  root.appendChild(pickerField);

  // Step 2: dynamic fields（按 provider 类型切换）
  const dynamicFields = el("div", { id: "ei-dynamic-fields" });
  root.appendChild(dynamicFields);

  // 初始渲染（默认第一个 provider）
  renderDynamicFields(providers[0]?.id ?? "minimax", providers, dynamicFields);

  // picker change → 重渲染 dynamic fields
  root.addEventListener("change", (e) => {
    const target = e.target as HTMLSelectElement;
    if (target.id === "ei-provider") {
      renderDynamicFields(target.value, providers, dynamicFields);
    }
  });

  return root;
}

/** 按 provider_id 渲染 dynamic fields。内置走"只填 key"，custom 走 3 选 1 Extract 模板。 */
function renderDynamicFields(
  providerId: string,
  providers: PickerProvider[],
  host: HTMLElement,
): void {
  host.innerHTML = "";
  const provider = providers.find((p) => p.id === providerId);
  if (!provider) return;

  if (providerId === "custom") {
    renderCustomFields(host);
  } else {
    renderBuiltinFields(host, provider);
  }
}

function renderBuiltinFields(host: HTMLElement, _provider: PickerProvider): void {
  // 内置副本：只填 key（key 类型按 auth_kind 决定，UI 不重复提示）
  // 简化：auth_kind = cookie 的也用同一 password 框（不区分 cookie / api_key）
  host.appendChild(
    el("div", { class: "field" },
      el("label", { for: "ei-api-key" }, t("extra.form.api_key_label")),
      el("input", {
        type: "password",
        id: "ei-api-key",
        autocomplete: "off",
        placeholder: t("extra.form.api_key_placeholder"),
      }),
      el("div", { class: "help" }, t("extra.form.api_key_help")),
    ),
  );
}

function renderCustomFields(host: HTMLElement): void {
  // ===== display_name =====
  host.appendChild(
    field("display_name", t("custom_source.field.display_name"), el("input", {
      type: "text",
      id: "cs-name",
      placeholder: t("custom_source.field.display_name_placeholder"),
      required: "true",
      autocomplete: "off",
    })),
  );

  // ===== base_url =====
  host.appendChild(
    field("base_url", t("custom_source.field.base_url"), el("input", {
      type: "url",
      id: "cs-base",
      placeholder: t("custom_source.field.base_url_placeholder"),
      required: "true",
      autocomplete: "off",
    })),
  );

  // ===== path =====
  host.appendChild(
    field("path", t("custom_source.field.path"), el("input", {
      type: "text",
      id: "cs-path",
      placeholder: t("custom_source.field.path_placeholder"),
      required: "true",
      autocomplete: "off",
    })),
  );

  // ===== method (radio GET / POST) =====
  const methodGroup = el("div", { class: "field" },
    el("label", {}, t("custom_source.field.method")),
    el("div", { class: "radio-group" },
      radio("cs-method", "GET", true),
      radio("cs-method", "POST", false),
    ),
  );
  host.appendChild(methodGroup);

  // ===== extract preset (3 选 1 radio) =====
  const presetGroup = el("div", { class: "field" },
    el("label", {}, t("custom_source.field.extract_preset")),
    el("div", { class: "radio-group" },
      radio("cs-preset", "new_api", true, t("custom_source.preset.new_api")),
      radio("cs-preset", "balance", false, t("custom_source.preset.balance")),
      radio("cs-preset", "custom", false, t("custom_source.preset.custom")),
    ),
    el("div", { class: "help" }, t("custom_source.preset_help")),
  );
  host.appendChild(presetGroup);

  // ===== dynamic fields (按 preset 切换) =====
  const csDynamicFields = el("div", { id: "cs-dynamic-fields" });
  host.appendChild(csDynamicFields);

  // ===== plan_name_path =====
  host.appendChild(
    field("plan_name_path", t("custom_source.field.plan_name"), el("input", {
      type: "text",
      id: "cs-plan",
      placeholder: t("custom_source.field.plan_name_placeholder"),
      autocomplete: "off",
    })),
  );

  // ===== accent 调色板 =====
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
  host.appendChild(accentGroup);

  // ===== api_key =====
  host.appendChild(
    field("api_key", t("custom_source.field.api_key"), el("input", {
      type: "password",
      id: "cs-api-key",
      placeholder: t("custom_source.field.api_key_placeholder"),
      autocomplete: "off",
    })),
  );

  // preset change → 重新渲染 dynamic fields + accent 选中
  host.addEventListener("change", (e) => {
    const t = e.target as HTMLInputElement;
    if (t.name === "cs-preset") renderCustomPresetFields(t.value);
    if (t.classList.contains("accent-swatch")) {
      host
        .querySelectorAll<HTMLElement>(".accent-swatch")
        .forEach((s) => s.classList.remove("selected"));
      t.classList.add("selected");
    }
  });

  // 初始 NewApi 字段
  renderCustomPresetFields("new_api");
}

function renderCustomPresetFields(preset: string): void {
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

function field(_name: string, label: string, input: HTMLElement): HTMLElement {
  return el("div", { class: "field" },
    el("label", { for: input.id }, label),
    el("div", { class: "input-row" }, input),
  );
}

function radio(name: string, value: string, checked: boolean, label?: string): HTMLElement {
  const id = `${name}-${value}`;
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

// ── 提交 ──────────────────────────────────────────────────────

async function submitHandler(body: HTMLElement): Promise<boolean> {
  // 1. 取 provider 类型
  const providerId = body.querySelector<HTMLSelectElement>("#ei-provider")!.value;
  if (!providerId) {
    flash(t("extra.err.provider_required"), true);
    return false;
  }

  // 2. 按类型分支收集字段
  if (providerId === "custom") {
    return submitCustom(body);
  } else {
    return submitBuiltin(body, providerId);
  }
}

async function submitBuiltin(body: HTMLElement, providerId: string): Promise<boolean> {
  const apiKey = (body.querySelector<HTMLInputElement>("#ei-api-key")?.value ?? "").trim();
  if (!apiKey) {
    flash(t("extra.err.api_key_required"), true);
    return false;
  }

  // 测试连接
  try {
    const snap = await testExtraInstance({ provider_id: providerId, api_key: apiKey });
    if (!snap.success) {
      flash(t("extra.err.test_failed", { err: snap.error ?? t("floating.error.unknown") }), true);
      return false;
    }
    flash(t("extra.test_passing"));
  } catch (e) {
    flash(t("extra.err.test_error", { err: String(e) }), true);
    return false;
  }

  // 保存
  try {
    const inst = await addExtraInstance({ provider_id: providerId, api_key: apiKey });
    flash(t("extra.added", { id: inst.api_key_ref }));
    await rebuildProvidersSection();
    return true;
  } catch (e) {
    flash(t("extra.err.save_failed", { err: String(e) }), true);
    return false;
  }
}

async function submitCustom(body: HTMLElement): Promise<boolean> {
  // 1. 收集 custom 字段（跟原 custom-source-form.ts 一样）
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
    extract = { preset: "balance", balance_path: balancePath, currency_path: currencyPath || null, divide };
  } else {
    extract = {
      preset: "custom",
      remaining_path: (body.querySelector<HTMLInputElement>("#cs-remaining-path")?.value ?? "").trim() || null,
      used_path: (body.querySelector<HTMLInputElement>("#cs-used-path")?.value ?? "").trim() || null,
      total_path: (body.querySelector<HTMLInputElement>("#cs-total-path")?.value ?? "").trim() || null,
      unit: (body.querySelector<HTMLInputElement>("#cs-unit")?.value ?? "").trim() || null,
      divide,
    };
  }

  const customSpec: Omit<CustomSourceSpec, "id" | "created_at"> = {
    display_name: displayName,
    base_url: baseUrl,
    path,
    method: method as "GET" | "POST",
    extract,
    plan_name_path: planNamePath || null,
    accent,
  };

  // 4. 测试连接
  try {
    const snap = await testExtraInstance({
      provider_id: "custom",
      api_key: apiKey,
      custom: customSpec,
    });
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
    const inst = await addExtraInstance({
      provider_id: "custom",
      api_key: apiKey,
      custom: customSpec,
    });
    flash(t("custom_source.added", { name: displayName, id: inst.api_key_ref }));
    await rebuildProvidersSection();
    return true;
  } catch (e) {
    flash(t("custom_source.err.save_failed", { err: String(e) }), true);
    return false;
  }
}

async function rebuildProvidersSection(): Promise<void> {
  const container = document.querySelector<HTMLElement>(
    '.section-view[data-section="providers"]',
  );
  if (container) {
    await listExtraInstances(); // warm
    await renderProvidersSection(container);
  }
}
