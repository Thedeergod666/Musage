// 设置面板 "数据源" section —— 动态渲染
//
// **完全 registry-driven**：
// - list_sources() 拿到 SourceMeta[] → 调 createProviderPanel(meta) 每个出一个 panel
// - 加新 source 改 1 个文件（[src-tauri/src/providers/mod.rs] builtin_sources）
//   + [source-extras.ts] EXTRAS 表（如果该 source 有额外字段）
// - settings.html / providers.ts 主流程 0 改动
//
// 之前的 settings.html 硬编码 5 个 panel，每个 ~70 行 = 350 行。换成动态后
// 加一个 source 是 0 行 HTML 改动 + ~10 行 source-extras.ts（如果有 extras）。
//
// 兼容旧代码：所有 #id / .class 都跟原 v0.5.x 一致（api-key-{id} /
// enabled-{id} / interval-{id} / api-key-status-{id} 等），让 config.ts 里
// 的 loadConfig() / saveConfig() 不用改就能照常读写这些元素。
//
// **PR 3** 起 13+ source 太多，加分组 + 搜索 + 「+ 添加自定义」按钮。

import {
  listSources,
  getConfig,
  setProviderEnabled,
  listCustomSources,
  deleteCustomSource,
} from "./api";
import { el, setCurrentKnownIds, flash } from "./utils";
import { getProviderExtras } from "./source-extras";
import { renderOrderSection } from "./order";
import { renderCredentialBlock, loadCredentialStatus } from "./credentials";
import { getProviderMeta } from "./logos";
import { groupSources, renderGroup } from "./groups";
import { openAddCustomSourceModal } from "./custom-source-form";
import type { AppConfig, SourceMeta } from "./types";

/// 主入口：渲染整个 "数据源" section。
/// - 调 list_sources 拿 SourceMeta[]（内置）+ list_custom_sources 拿 customs
/// - 调 get_config 拿 cfg（用来初始化 region/interval/extras 初值 + 启用状态）
/// - 先渲染顶部 toolbar（搜索 + 计数 + 添加按钮）
/// - 再渲染顶部"浮窗卡片顺序"
/// - 最后按分组（token_plan / balance / official / xiaomi / custom / misc）渲染
export async function renderProvidersSection(container: HTMLElement) {
  let sources: SourceMeta[];
  let customs: import("./types").CustomSourceSpec[];
  let cfg: AppConfig;
  try {
    [sources, customs, cfg] = await Promise.all([
      listSources(),
      listCustomSources(),
      getConfig(),
    ]);
  } catch (e) {
    container.innerHTML = `<div class="section-empty error">✗ 加载 sources 失败: ${String(e)}</div>`;
    return;
  }

  container.innerHTML = ""; // 清掉占位

  // 把 customs 转成 SourceMeta 形状（list_sources 只返内置），merge 进 sources。
  // enabled 状态：默认 true，存 cfg.providers["custom_<id>"].enabled。
  const customsAsMeta: SourceMeta[] = customs.map((c) => ({
    id: c.id,
    display_name: c.display_name,
    auth_kind: "api_key" as const,
    enabled: cfg.providers?.[c.id]?.enabled ?? true,
  }));
  const allSources: SourceMeta[] = [...sources, ...customsAsMeta];
  // 让 utils.currentKnownIds 跟上（含 customs），order section 立即能用
  setCurrentKnownIds(allSources.map((s) => s.id));

  // 1) 顶部 toolbar：搜索 + 计数 + 添加按钮
  const toolbar = renderToolbar(allSources, cfg);
  // 绑定「+ 添加自定义来源」按钮
  const addBtn = toolbar.querySelector<HTMLButtonElement>("#add-custom-source");
  addBtn?.addEventListener("click", () => openAddCustomSourceModal());
  container.appendChild(toolbar);

  // 2) 顶部"浮窗卡片顺序"（带 enabled/disabled 分区）
  renderOrderSection(container, allSources, cfg.provider_order, cfg);

  // 3) 按分组渲染
  const groups = groupSources(allSources);
  for (const [key, metas] of groups) {
    container.appendChild(
      renderGroup(key, metas, (m) => createProviderPanel(m, cfg)),
    );
  }

  // 4) 搜索 input 事件 → toggle .hidden
  const search = container.querySelector<HTMLInputElement>("#provider-search")!;
  search.addEventListener("input", () => applySearchFilter(search.value, container));
}

/// 顶部 toolbar：搜索框 + 计数 + 「+ 添加自定义来源」按钮
function renderToolbar(sources: SourceMeta[], cfg: AppConfig): HTMLElement {
  const enabled = sources.filter(
    (s) => cfg.providers?.[s.id]?.enabled ?? true,
  ).length;
  return el(
    "div",
    { class: "provider-toolbar" },
    el("input", {
      type: "search",
      id: "provider-search",
      placeholder: "🔍 搜索 provider (id 或名字)...",
      autocomplete: "off",
    }),
    el("span", { class: "provider-count" }, `启用 ${enabled} / 共 ${sources.length}`),
    el(
      "button",
      { type: "button", id: "add-custom-source", class: "btn-primary" },
      "+ 添加自定义来源",
    ),
  );
}

/// 搜索过滤：把不匹配的 .provider-section 标 .hidden，相应组也隐藏（如果全空）。
function applySearchFilter(q: string, container: HTMLElement): void {
  const needle = q.trim().toLowerCase();
  container
    .querySelectorAll<HTMLElement>(".provider-section")
    .forEach((sec) => {
      const id = sec.dataset.id ?? "";
      const name =
        sec.querySelector(".provider-name")?.textContent ?? "";
      const hit =
        !needle ||
        id.toLowerCase().includes(needle) ||
        name.toLowerCase().includes(needle);
      sec.classList.toggle("hidden", !hit);
    });
  // 空组也隐藏（避免 "其他 (0)" 这种空组还在占位）
  container
    .querySelectorAll<HTMLDetailsElement>(".provider-group")
    .forEach((g) => {
      const anyVisible = Array.from(
        g.querySelectorAll<HTMLElement>(".provider-section"),
      ).some((s) => !s.classList.contains("hidden"));
      g.classList.toggle("hidden", !anyVisible);
    });
}

/// 一个 source → 一个 panel（带 header + credentials + EXTRAS + 启用/间隔）
export function createProviderPanel(meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const section = el(
    "section",
    { class: "provider-section", "data-id": meta.id },
  );

  // 拿 logo 资产（沿用浮窗 [src/main.ts:15-30] 同款 import）
  const providerMeta = getProviderMeta(meta.id);
  const logoImg = providerMeta
    ? el("img", {
        class: "provider-logo",
        src: providerMeta.logo,
        alt: providerMeta.name,
        title: providerMeta.name,
      })
    : null;

  // ── Header: [logo] [display_name] ........ [在浮窗显示 checkbox] ──
  const enabledCheckbox = el("input", {
    type: "checkbox",
    id: `enabled-${meta.id}`,
    "data-id": meta.id,
  }) as HTMLInputElement;
  enabledCheckbox.checked = cfg.providers?.[meta.id]?.enabled ?? true;
  // 即时生效
  enabledCheckbox.addEventListener("change", () => {
    setProviderEnabled(meta.id, enabledCheckbox.checked).catch((e) => {
      flash(`✗ 切换显示失败: ${e}`, true);
    });
  });

  section.appendChild(
    el(
      "div",
      { class: "provider-header" },
      ...(logoImg ? [logoImg] : []),
      el("span", { class: "provider-name" }, meta.display_name),
      el(
        "div",
        { class: "provider-enabled" },
        // PR 3: custom source 在 header 加 🗑️ 删除按钮（带二次输入 name 确认）
        ...(meta.id.startsWith("custom_")
          ? [renderDeleteCustomButton(meta)]
          : []),
        enabledCheckbox,
        el("label", { for: `enabled-${meta.id}` }, "在浮窗显示"),
      ),
    ),
  );

  // ── 凭据块 ──
  section.appendChild(renderCredentialBlock(meta));

  // ── EXTRAS（per-id 区域下拉 / 集群 / 简洁模式 / base url 等）──
  for (const block of getProviderExtras(meta.id)) {
    section.appendChild(block(meta, cfg));
  }

  // ── 轮询间隔（每个 provider 都有，挪到 extras 也行；为简洁放最后）──
  section.appendChild(renderIntervalOverride(meta.id, cfg));

  return section;
}

/// 每个 provider 的「轮询间隔（覆盖）」字段
function renderIntervalOverride(id: string, cfg: AppConfig): HTMLElement {
  const v = cfg.providers?.[id]?.refresh_interval_secs;
  const input = el("input", {
    type: "number",
    id: `interval-${id}`,
    "data-id": id,
    min: "10",
    step: "5",
    placeholder: `默认 ${cfg.refresh_interval_secs ?? 60} 秒（顶部「轮询间隔」）`,
  }) as HTMLInputElement;
  if (v != null) input.value = String(v);
  return el(
    "div",
    { class: "field" },
    el("label", {}, "轮询间隔（覆盖）"),
    el("div", { class: "input-row" }, input, el("span", { class: "unit-suffix" }, "秒")),
    el(
      "div",
      { class: "help" },
      "留空 = 用顶部「轮询间隔」。至少 10 秒（避免触发 provider rate limit）。",
    ),
  );
}

/// PR 3: custom source 面板右上角 🗑️ 按钮
/// 删除流程：confirm() → 二次输入 display_name → 调 deleteCustomSource → 重建 section
function renderDeleteCustomButton(meta: SourceMeta): HTMLElement {
  const btn = el("button", {
    type: "button",
    class: "btn-delete-custom",
    "data-id": meta.id,
    title: `删除自定义来源 ${meta.display_name}`,
  }, "🗑️");
  btn.addEventListener("click", async () => {
    if (!confirm(`确认删除自定义来源 "${meta.display_name}"？\n其 API key 也会被一起删除。`)) {
      return;
    }
    // 二次输入：防误删短 id（custom_<uuid> 看起来都差不多）
    const input = prompt(
      `为防误删，请再次输入 display_name "${meta.display_name}"：`,
    )?.trim();
    if (input !== meta.display_name) {
      flash("✗ display_name 不匹配，未删除", true);
      return;
    }
    try {
      await deleteCustomSource(meta.id);
      flash(`✓ ${meta.display_name} 已删除`);
      // 重建整个 providers section
      const container = document.querySelector<HTMLElement>(
        '.section-view[data-section="providers"]',
      );
      if (container) await renderProvidersSection(container);
    } catch (e) {
      flash(`✗ 删除失败: ${String(e)}`, true);
    }
  });
  return btn;
}

/// 渲染后批量调 loadCredentialStatus 拉每个 source 的 key 状态。
/// 跟 init() 里的 loadKeyStatus / loadTavilyKeyStatus / loadZenmuxKeyStatus
/// 等价，但走 id-based 统一接口。
export async function loadAllCredentialStatus(sources: SourceMeta[]) {
  await Promise.all(sources.map((s) => loadCredentialStatus(s.id)));
}