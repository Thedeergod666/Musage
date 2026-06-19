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
import { el, setCurrentKnownIds, flash, currentProviderOrder } from "./utils";
import { getProviderExtras } from "./source-extras";
import { renderOrderSection, withSuppress } from "./order";
import { renderCredentialBlock, loadCredentialStatus } from "./credentials";
import { getProviderMeta } from "./logos";
import { getGroupDef, groupKeyFor } from "./groups";
import { openAddCustomSourceModal } from "./custom-source-form";
import { t } from "../i18n";
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
    container.innerHTML = `<div class="section-empty error">${t("settings.providers.load_failed", { err: String(e) })}</div>`;
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
    is_stub: false,  // customs 不打 STUB 标（用户自己加的，知道在干啥）
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

  // 3) 套餐区扁平列表：所有 provider 按「浮窗卡片顺序」铺在一个长列表里。
  // 组归属通过每个 provider header 里的 .provider-group-tag 体现（如
  // "Token Plan"），不再需要顶部的组分隔线。
  const orderIdx = new Map(currentProviderOrder.map((id, i) => [id, i]));
  const allSorted = [...allSources].sort((a, b) => {
    const ai = orderIdx.get(a.id);
    const bi = orderIdx.get(b.id);
    // ES2019+ Array.sort 稳定：两个都不在 orderIdx 里时保留 builtin_sources()
    // 注册顺序，新加 provider 不会跳到列表中乱位。
    return ((ai ?? Number.POSITIVE_INFINITY) - (bi ?? Number.POSITIVE_INFINITY));
  });

  const flatContainer = el("div", { class: "providers-flat" });
  for (const meta of allSorted) {
    flatContainer.appendChild(createProviderPanel(meta, cfg));
  }
  container.appendChild(flatContainer);

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
      placeholder: t("settings.providers.search_placeholder"),
      autocomplete: "off",
    }),
    el("span", { class: "provider-count" },
      t("settings.providers.count_label", { enabled, total: sources.length })),
    el(
      "button",
      { type: "button", id: "add-custom-source", class: "btn-primary" },
      t("settings.providers.add_custom"),
    ),
  );
}

/// 搜索过滤：把不匹配的 .provider-section 标 .hidden。空组的 inline 分隔线
/// 同步隐藏 —— 避免出现"分隔线悬空"或"两组相邻 divider 紧贴"的视觉。
function applySearchFilter(q: string, container: HTMLElement): void {
  const needle = q.trim().toLowerCase();
  container
    .querySelectorAll<HTMLElement>(".provider-section")
    .forEach((sec) => {
      const id = sec.dataset.id ?? "";
      const name = sec.querySelector(".provider-name")?.textContent ?? "";
      const hit =
        !needle ||
        id.toLowerCase().includes(needle) ||
        name.toLowerCase().includes(needle);
      sec.classList.toggle("hidden", !hit);
    });
}

/// 一个 source → 一个 panel（带 header + credentials + EXTRAS + 启用/间隔）
export function createProviderPanel(meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const section = el(
    "section",
    {
      class: "provider-section" + (meta.is_stub ? " provider-section--stub" : ""),
      "data-id": meta.id,
      ...(meta.is_stub ? { "data-stub": "true" } : {}),
    },
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

  // ── Header: [logo] [display_name] [group tag] ........ [在浮窗显示 checkbox] ──
  // 组标签（如 "Token Plan"）让用户一眼看出 provider 归属哪个类目，
  // 不用靠 divider 行来推断。
  const gk = groupKeyFor(meta);
  const gDef = getGroupDef(gk);
  const groupTag = el(
    "span",
    { class: "provider-group-tag", "data-group": gk },
    gDef.title,
  );

  const enabledCheckbox = el("input", {
    type: "checkbox",
    id: `enabled-${meta.id}`,
    "data-id": meta.id,
  }) as HTMLInputElement;
  enabledCheckbox.checked = cfg.providers?.[meta.id]?.enabled ?? true;
  // 即时生效
  // **L14 fix（2026-06-19）**：单点 checkbox 包进 withSuppress()，让 main.ts
  // 的 config-changed 监听器在 IPC 落地窗口内跳过 rebuild。否则连续点多个
  // checkbox 时第二次的 config-changed 事件会用后端"刚才"的状态覆盖我们
  // 乐观更新的 orderCfg，浮窗在「全隐藏」与「新位置」之间闪烁。批量操作
  // （onDividerMouseUp）原本就抑制；现在单点也走同一机制。
  enabledCheckbox.addEventListener("change", () => {
    const target = enabledCheckbox.checked;
    withSuppress(() => setProviderEnabled(meta.id, target))
      .catch((e) => {
        flash(t("settings.providers.flash_toggle_failed", { err: String(e) }), true);
      });
  });

  section.appendChild(
    el(
      "div",
      { class: "provider-header" },
      ...(logoImg ? [logoImg] : []),
      el("span", { class: "provider-name" }, meta.display_name),
      groupTag,
      // STUB 角标（2026-06-17 commit）：公开 API 无 quota endpoint 的
      // provider 显示"🚧 STUB"小角标，避免用户配 key 后看 30 min 退避风暴。
      // 用 data-stub-notice 属性挂载文本，CSS ::after 显示，i18n 走属性。
      ...(meta.is_stub
        ? [
            el(
              "span",
              {
                class: "provider-stub-badge",
                "data-stub-notice": "STUB",
                title: "Public API has no quota endpoint",
              },
              "STUB",
            ),
          ]
        : []),
      el(
        "div",
        { class: "provider-enabled" },
        // PR 3: custom source 在 header 加 🗑️ 删除按钮（带二次输入 name 确认）
        ...(meta.id.startsWith("custom_")
          ? [renderDeleteCustomButton(meta)]
          : []),
        enabledCheckbox,
        el("label", { for: `enabled-${meta.id}` }, t("settings.providers.show_in_floating")),
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
    placeholder: t("settings.providers.refresh_interval_placeholder",
      { secs: cfg.refresh_interval_secs ?? 60 }),
  }) as HTMLInputElement;
  if (v != null) input.value = String(v);
  return el(
    "div",
    { class: "field" },
    el("label", {}, t("settings.providers.refresh_interval_override")),
    el("div", { class: "input-row" }, input, el("span", { class: "unit-suffix" }, t("settings.providers.unit_seconds"))),
    el(
      "div",
      { class: "help" },
      t("settings.providers.refresh_interval_help"),
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
    title: t("settings.providers.delete_custom_btn_title", { name: meta.display_name }),
  }, "×");
  btn.addEventListener("click", async () => {
    if (!confirm(t("settings.providers.delete_custom_confirm", { name: meta.display_name }))) {
      return;
    }
    // 二次输入：防误删短 id（custom_<uuid> 看起来都差不多）
    const input = prompt(
      t("settings.providers.delete_custom_prompt", { name: meta.display_name }),
    )?.trim();
    if (input !== meta.display_name) {
      flash(t("settings.providers.delete_custom_mismatch"), true);
      return;
    }
    try {
      await deleteCustomSource(meta.id);
      flash(t("settings.providers.delete_custom_done", { name: meta.display_name }));
      // 重建整个 providers section
      const container = document.querySelector<HTMLElement>(
        '.section-view[data-section="providers"]',
      );
      if (container) await renderProvidersSection(container);
    } catch (e) {
      flash(t("settings.providers.delete_failed", { err: String(e) }), true);
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