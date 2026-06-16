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
    title: "添加自定义 New API 来源",
    body,
    submitLabel: "保存",
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
    field("display_name", "名称", el("input", {
      type: "text",
      id: "cs-name",
      placeholder: "DMX API",
      required: "true",
      autocomplete: "off",
    })),
  );

  // base_url
  root.appendChild(
    field("base_url", "Base URL", el("input", {
      type: "url",
      id: "cs-base",
      placeholder: "https://api.dmx.com",
      required: "true",
      autocomplete: "off",
    })),
  );

  // path
  root.appendChild(
    field("path", "路径", el("input", {
      type: "text",
      id: "cs-path",
      placeholder: "/api/user/self",
      required: "true",
      autocomplete: "off",
    })),
  );

  // method (radio GET / POST)
  const methodGroup = el("div", { class: "field" },
    el("label", {}, "方法"),
    el("div", { class: "radio-group" },
      radio("cs-method", "GET", true),
      radio("cs-method", "POST", false),
    ),
  );
  root.appendChild(methodGroup);

  // extract preset (3 选 1 radio)
  const presetGroup = el("div", { class: "field" },
    el("label", {}, "提取模板"),
    el("div", { class: "radio-group" },
      radio("cs-preset", "new_api", true, "New API 系"),
      radio("cs-preset", "balance", false, "余额系"),
      radio("cs-preset", "custom", false, "自定义"),
    ),
    el("div", { class: "help" },
      "New API 系 = data.quota / data.used_quota（divide 500000）；余额系 = 用户填 path；自定义 = 3 个独立 path",
    ),
  );
  root.appendChild(presetGroup);

  // 动态字段容器：按 preset 切换
  const dynamicFields = el("div", { id: "cs-dynamic-fields" });
  root.appendChild(dynamicFields);

  // plan_name_path（可选）
  root.appendChild(
    field("plan_name_path", "套餐名 JSON path（可选）", el("input", {
      type: "text",
      id: "cs-plan",
      placeholder: "data.group",
      autocomplete: "off",
    })),
  );

  // accent 调色板
  const accentGroup = el("div", { class: "field" },
    el("label", {}, "主题色"),
    el("div", { class: "accent-palette", id: "cs-accent-palette" },
      ...ACCENT_PALETTE.map((c) => el("button", {
        type: "button",
        class: "accent-swatch",
        "data-color": c,
        style: `background: ${c};`,
        title: c,
      })),
    ),
    el("div", { class: "help" }, "浮窗卡片背景色（fallback：首字母 + 此色）"),
  );
  root.appendChild(accentGroup);

  // API key
  root.appendChild(
    field("api_key", "API key（先填，测试 + 保存用）", el("input", {
      type: "password",
      id: "cs-api-key",
      placeholder: "sk-...",
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
      field("divide", "divide（默认 500000，New API 经典）", el("input", {
        type: "number",
        id: "cs-divide",
        value: "500000",
        step: "1",
        min: "0",
      })),
    );
  } else if (preset === "balance") {
    host.appendChild(
      field("balance_path", "balance JSON path", el("input", {
        type: "text",
        id: "cs-balance-path",
        placeholder: "data.credit",
        required: "true",
      })),
    );
    host.appendChild(
      field("currency_path", "currency JSON path（可选）", el("input", {
        type: "text",
        id: "cs-currency-path",
        placeholder: "data.unit",
      })),
    );
    host.appendChild(
      field("divide_balance", "divide（默认 1.0）", el("input", {
        type: "number",
        id: "cs-divide",
        value: "1.0",
        step: "0.01",
        min: "0",
      })),
    );
  } else if (preset === "custom") {
    host.appendChild(
      field("remaining_path", "remaining JSON path（可选）", el("input", {
        type: "text",
        id: "cs-remaining-path",
        placeholder: "data.credits",
      })),
    );
    host.appendChild(
      field("used_path", "used JSON path（可选）", el("input", {
        type: "text",
        id: "cs-used-path",
        placeholder: "data.used",
      })),
    );
    host.appendChild(
      field("total_path", "total JSON path（可选）", el("input", {
        type: "text",
        id: "cs-total-path",
        placeholder: "data.total",
      })),
    );
    host.appendChild(
      field("unit", "unit（写死字符串，可选）", el("input", {
        type: "text",
        id: "cs-unit",
        placeholder: "USD / CNY / credits",
      })),
    );
    host.appendChild(
      field("divide_custom", "divide（默认 1.0）", el("input", {
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
  if (!displayName) { flash("✗ 名称不能为空", true); return false; }
  if (!baseUrl.startsWith("http://") && !baseUrl.startsWith("https://")) {
    flash("✗ Base URL 必须以 http:// 或 https:// 开头", true);
    return false;
  }
  if (!path.startsWith("/")) {
    flash("✗ 路径必须以 / 开头", true);
    return false;
  }
  if (!apiKey) { flash("✗ API key 不能为空（先填再保存）", true); return false; }

  // 3. 构造 ExtractSpec
  const divideRaw = (body.querySelector<HTMLInputElement>("#cs-divide")?.value ?? "").trim();
  const divide = divideRaw === "" ? null : Number(divideRaw);
  let extract: ExtractSpec;
  if (preset === "new_api") {
    extract = { preset: "new_api", divide };
  } else if (preset === "balance") {
    const balancePath = (body.querySelector<HTMLInputElement>("#cs-balance-path")!.value ?? "").trim();
    if (!balancePath) { flash("✗ Balance path 必填", true); return false; }
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
      flash(`✗ 测试失败: ${snap.error ?? "未知错误"}`, true);
      return false;
    }
    flash("✓ 测试通过，正在保存...");
  } catch (e) {
    flash(`✗ 测试连接出错: ${String(e)}`, true);
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
    flash(`✓ ${displayName} 已添加 (id: ${id})`);
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
    flash(`✗ 保存失败: ${String(e)}`, true);
    return false;
  }
}
