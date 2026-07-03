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
  listExtraInstances,
  deleteExtraInstance,
  saveConfig,
} from "./api";
import { el, escapeHtml, setCurrentKnownIds, flash, currentProviderOrder, formatDisplayName } from "./utils";
import { getProviderExtras } from "./source-extras";
import { renderOrderSection, withSuppress, resetDragState } from "./order";
import { renderCredentialBlock, loadCredentialStatus, batchPasteKeys } from "./credentials";
import { getProviderMeta } from "./logos";
import { getGroupDef, groupKeyFor } from "./groups";
import { openAddExtraInstanceModal } from "./extra-instance-form";
import { t } from "../i18n";
import type { AppConfig, ExtraInstance, SourceMeta } from "./types";

/// 主入口：渲染整个 "数据源" section。
/// - 调 list_sources 拿 SourceMeta[]（内置）+ list_extra_instances 拿 extras
/// - 调 get_config 拿 cfg（用来初始化 region/interval/extras 初值 + 启用状态）
/// - 先渲染顶部 toolbar（搜索 + 计数 + 添加按钮）
/// - 再渲染顶部"浮窗卡片顺序"
/// - 最后按分组（token_plan / balance / official / xiaomi / custom / misc）渲染
export async function renderProvidersSection(container: HTMLElement) {
  let sources: SourceMeta[];
  let extras: ExtraInstance[];
  let cfg: AppConfig;
  try {
    [sources, extras, cfg] = await Promise.all([
      listSources(),
      listExtraInstances(),
      getConfig(),
    ]);
  } catch (e) {
    container.innerHTML = `<div class="section-empty error">${escapeHtml(t("settings.providers.load_failed", { err: String(e) }))}</div>`;
    return;
  }

  container.innerHTML = ""; // 清掉占位

  // 把 extras 转成 SourceMeta 形状：
  // - 内置副本（provider_id != "custom"）：合并进对应的内置行下方作为 "副本行"
  // - custom：作为单独的 SourceMeta（id 用 api_key_ref）
  //
  // PR 1b 简化策略：先渲染内置 11 个 + custom 全部（按 SourceMeta 一视同仁）。
  // 副本用额外的"副本组"section 在内置行下面渲染。
  const builtinExtras: ExtraInstance[] = extras.filter((e) => e.provider_id !== "custom");
  const customExtras: ExtraInstance[] = extras.filter((e) => e.provider_id === "custom");

  // 内置副本：通过 api_key_ref 区分 → 用 "minimax#2" 这种 ID 作 DOM key
  // display_name：在设置面板渲染时用前端 t() 拿翻译好的基名 + "#N" 后缀，
  // 跟后端 display_name() 行为严格对齐（后者也用 t!("provider_name.xxx")）。
  const builtinExtrasAsMeta: SourceMeta[] = builtinExtras.map((e) => ({
    id: e.api_key_ref, // "minimax#2"
    display_name: formatDisplayName(t(`provider.${e.provider_id}.name`), e.instance_index),
    auth_kind: "api_key" as const, // 默认，副本通常不需要 cookie
    enabled: cfg.providers?.[e.api_key_ref]?.enabled ?? true,
    is_stub: false,
    extra_instance_uuid: e.id, // P0-1: UUID 给 delete/update IPC 用
  }));

  const customExtrasAsMeta: SourceMeta[] = customExtras.map((e) => ({
    id: e.api_key_ref, // "custom_<uuid>"
    display_name: e.custom?.display_name ?? "?",
    auth_kind: "api_key" as const,
    enabled: cfg.providers?.[e.api_key_ref]?.enabled ?? true,
    is_stub: false,
    extra_instance_uuid: e.id, // P0-1: UUID 给 delete/update IPC 用
  }));

  const allSources: SourceMeta[] = [...sources, ...builtinExtrasAsMeta, ...customExtrasAsMeta];
  setCurrentKnownIds(allSources.map((s) => s.id));

  // 1) 顶部 toolbar：搜索 + 计数 + 添加按钮
  const toolbar = renderToolbar(allSources, cfg);
  // 绑定「+ 添加新来源」按钮
  const addBtn = toolbar.querySelector<HTMLButtonElement>("#add-custom-source");
  addBtn?.addEventListener("click", () => openAddExtraInstanceModal());
  container.appendChild(toolbar);

  // v0.2.1 commit 6: 批量粘贴 key 的折叠 textarea,在 toolbar 下方。
  // 用户粘多行 `provider=value` 或纯 key,自动识别 provider 填入。
  container.appendChild(renderBatchPasteSection());

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
      ...(meta.is_stub
        ? [
            el(
              "span",
              {
                class: "provider-stub-badge",
                "data-stub-notice": t("provider.stub_badge"),
                title: t("provider.stub_badge_title"),
              },
              t("provider.stub_badge"),
            ),
          ]
        : []),
      el(
        "div",
        { class: "provider-enabled" },
        // PR 1b: 每个 panel header 加 📋 复制按钮（内置行）或 🗑️ 删除按钮（extra 行）
        // - meta.id 是 base provider_id ("minimax") → 显示 📋（用于复制副本）
        // - meta.id 包含 "#" 或 "custom_" → 显示 🗑️（副本 / custom 行）
        ...(meta.id.includes("#") || meta.id.startsWith("custom_")
          ? [renderDeleteExtraButton(meta)]
          : [renderCopyBuiltinButton(meta)]),
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

  // M4 fix: 绑定 change 事件，用户修改轮询间隔后自动保存
  input.addEventListener("change", async () => {
    const raw = input.value.trim();
    const secs = raw ? parseInt(raw, 10) : null;
    try {
      const latest = await getConfig();
      if (!latest.providers) latest.providers = {};
      if (!latest.providers[id]) latest.providers[id] = { enabled: true };
      latest.providers[id].refresh_interval_secs = secs;
      await saveConfig(latest);
    } catch (e) {
      flash(t("credentials.flash_save_failed", { err: String(e) }), true);
    }
  });

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

/// PR 1b: extra instance（副本 / custom）面板右上角 🗑️ 按钮
/// 删除流程：confirm() → 二次输入 display_name → 调 deleteExtraInstance → 重建 section
function renderDeleteExtraButton(meta: SourceMeta): HTMLElement {
  const btn = el("button", {
    type: "button",
    class: "btn-delete-custom",
    "data-id": meta.id,
    title: t("settings.providers.delete_extra_btn_title", { name: meta.display_name }),
  }, "×");
  btn.addEventListener("click", async () => {
    if (!confirm(t("settings.providers.delete_extra_confirm", { name: meta.display_name }))) {
      return;
    }
    // 二次输入：防误删短 id（"minimax#2" 看起来跟 "minimax" 像）
    const input = prompt(
      t("settings.providers.delete_extra_prompt", { name: meta.display_name }),
    )?.trim();
    if (input !== meta.display_name) {
      flash(t("settings.providers.delete_extra_mismatch"), true);
      return;
    }
    try {
      // P0-1: 删除必须传 UUID，不是 api_key_ref。meta.id 是 api_key_ref ("minimax#2")，
      // meta.extra_instance_uuid 才是真正的 UUID。
      // H22 fix (2026-07-03 audit): 之前 `?? meta.id` fallback 在数据不一致
      // (extra_instance_uuid 缺失) 时会把 "minimax#2" / "custom_<uuid>" 当 UUID
      // 传后端, 后端 uuid::Uuid 反序列化直接报错且错误信息难懂。改成显式
      // 拦截: uuid 缺失直接 flash 报错 "数据不一致, 请重启设置面板", 不调 IPC。
      if (!meta.extra_instance_uuid) {
        flash(t("settings.providers.delete_extra_no_uuid"), true);
        return;
      }
      await deleteExtraInstance(meta.extra_instance_uuid);
      flash(t("settings.providers.delete_extra_done", { name: meta.display_name }));
      // L2 fix: 重置拖拽状态，防止 section 重建后幽灵/placeholder 残留
      resetDragState();
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

/// PR 1b: 内置 provider 行的 📋 复制按钮
/// 流程：弹 modal（预填 provider type）+ 用户填 key → add_extra_instance
function renderCopyBuiltinButton(meta: SourceMeta): HTMLElement {
  const btn = el("button", {
    type: "button",
    class: "btn-copy-builtin",
    "data-id": meta.id,
    title: t("settings.providers.copy_builtin_btn_title", { name: meta.display_name }),
  }, "⎘");
  btn.addEventListener("click", () => {
    // 复用 openAddExtraInstanceModal —— 预选当前 provider。
    openAddExtraInstanceModal(meta.id);
  });
  return btn;
}

/// 渲染后批量调 loadCredentialStatus 拉每个 source 的 key 状态。
/// 跟 init() 里的 loadKeyStatus / loadTavilyKeyStatus / loadZenmuxKeyStatus
/// 等价，但走 id-based 统一接口。
export async function loadAllCredentialStatus(sources: SourceMeta[]) {
  await Promise.all(sources.map((s) => loadCredentialStatus(s.id)));
}

// ── v0.2.1 commit 6：批量粘贴 key 入口 (P2-A-5) ──────────────────────

/// 在 providers section 顶部 toolbar 下方渲染一个 `<details>` 折叠的
/// batch textarea。用户粘贴多行 key,自动识别 provider 前缀(`sk-cp-` /
/// `sk-or-v1-` / `tvly-` / `Oasis-Token` / `tp-` / `sessionKey=` /
/// 显式 `provider=xxx` 标注),批量调 `setSourceCredential` 填入。
///
/// flash 反馈:
/// - `recognized` 0 / `unrecognized` > 0 → "未识别 N 行" 红条
/// - `recognized` > 0 → "已识别 N 个 provider" 绿条
/// - `errors.length > 0` → "N 个错误: ..." 红条
function renderBatchPasteSection(): HTMLElement {
  const details = el("details", { class: "batch-paste-details" });
  const summary = el("summary", {},
    t("credentials.batch_paste_title"),
  );
  details.appendChild(summary);

  const textarea = el("textarea", {
    class: "batch-paste-textarea",
    id: "batch-paste-textarea",
    placeholder: t("credentials.batch_paste_help"),
    rows: "6",
    autocomplete: "off",
    spellcheck: "false",
  }) as HTMLTextAreaElement;

  const submitBtn = el("button", {
    type: "button",
    class: "btn-primary",
    "data-action": "batch-paste-submit",
  }, t("credentials.batch_paste_btn"));

  submitBtn.addEventListener("click", async () => {
    const text = textarea.value;
    if (!text.trim()) return;
    const result = await batchPasteKeys(text);
    if (result.errors.length > 0) {
      flash(t("credentials.batch_paste_errors", {
        n: result.errors.length,
        errs: result.errors.slice(0, 3).join("; "),
      }), true);
    } else if (result.recognized > 0 && result.unrecognized > 0) {
      flash(t("credentials.batch_paste_mixed", {
        rec: result.recognized,
        unrec: result.unrecognized,
      }));
    } else if (result.recognized > 0) {
      flash(t("credentials.batch_paste_recognized", { n: result.recognized }));
    } else if (result.unrecognized > 0) {
      flash(t("credentials.batch_paste_unrecognized", { n: result.unrecognized }), true);
    }
    // 成功后清空 textarea,失败保留让用户能修正
    if (result.errors.length === 0 && result.recognized > 0) {
      textarea.value = "";
    }
  });

  details.appendChild(textarea);
  details.appendChild(el("div", { class: "batch-paste-actions" }, submitBtn));
  return details;
}