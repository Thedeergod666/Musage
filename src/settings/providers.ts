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
  // 跟「浮窗卡片顺序」section（order.ts buildOrderItems）的视觉顺序对齐：
  // enabled 在前、disabled 在后，段内按 currentProviderOrder 出现顺序。
  // 之前只按 currentProviderOrder 线性排 —— 新加 builtin（如 anysearch）不在
  // 用户旧 provider_order 里，被 canonicalizeOrder 追加到末尾，面板就置底，
  // 要翻到最后才看得到；而顺序 section 按 enabled/disabled 分段，它显示在第 6
  // 位，两处不一致。改成同样的「enabled 优先」分段排序后三处（浮窗 / 顺序
  // section / 面板列表）一致。
  const orderIdx = new Map(currentProviderOrder.map((id, i) => [id, i]));
  const isEnabledId = (id: string) => cfg.providers?.[id]?.enabled ?? true;
  const allSorted = [...allSources].sort((a, b) => {
    const ea = isEnabledId(a.id) ? 0 : 1;
    const eb = isEnabledId(b.id) ? 0 : 1;
    if (ea !== eb) return ea - eb;
    // 同段内按 currentProviderOrder 顺序；都不在时（ES2019+ 稳定排序）保留
    // builtin_sources() 注册顺序，新加 provider 不乱位。
    const ai = orderIdx.get(a.id) ?? Number.POSITIVE_INFINITY;
    const bi = orderIdx.get(b.id) ?? Number.POSITIVE_INFINITY;
    return ai - bi;
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
  // M9 fix (2026-07-06 全量审查): parseInt 不处理 "abc"/"99999999"。负值 / NaN
  // / 过大 值传播到后端会静默接受,变成垃圾配置。10s 是 poller tick 下限
  // (commands/mod.rs refresh tick 校验),按这个兜底。
  input.addEventListener("change", async () => {
    const raw = input.value.trim();
    let secs: number | null = null;
    if (raw) {
      const n = parseInt(raw, 10);
      if (!Number.isFinite(n) || n < 10 || n > 86400) {
        flash(t("settings.providers.invalid_interval", { val: raw }), true);
        return;
      }
      secs = n;
    }
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
///
/// M-FIX (2026-07-27): 用户反馈"点击 × 删除按钮无效"bug。旧代码用
/// `prompt()` 收二次确认，且要求严格 `input === display_name` 匹配。
/// 但 macOS WKWebView 的 native `prompt()` 有两个**已知行为**会导致
/// 用户感知"删除失败"：
///   1) `prompt()` 弹窗在 macOS 上可能被上一轮 `confirm()` 的 Z-order / focus
///      残余影响，用户看到空白或一闪而过的 dialog，下意识点 Cancel。
///   2) 用户复制 `display_name`（`DeepSeek #3`）粘贴时，前后会自带 trim，
///      但 `prompt()` 自带值的"选中并替换"行为可能让剪贴板里
///      "DeepSeek #3" 后面的换行 / 不可见字符混进来 → `input.trim()` 跟
///      `display_name` 比仍不等 → flash mismatch → 用户以为"删不掉"。
///
/// 修法：
///   - 改用 in-app `<dialog>` modal（avoid native prompt 的不可靠 UX）；
///     fallback 保留原生 `prompt()`（极端 macOS 弹窗被全屏遮挡时 fallback）。
///   - display_name 比较改为 case-insensitive + 全空白 trim，复制粘贴 99% 不会失败。
///   - 末尾追加详细的 console.error 调试日志（不影响 UI），方便后台抓 dev 日志诊断。
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
    // 二次输入：防误删短 id（"minimax#2" 看起来跟 "minimax" 像）。
    // 用 in-app 自定义 modal（避免 native prompt 的 macOS 怪行为），modal
    // 显示完整 name 提示 + 一个 input 让用户原样输入。
    const typed = await promptForNameInApp(
      meta.display_name,
      t("settings.providers.delete_extra_prompt", { name: meta.display_name }),
    );
    // 三种比较（越来越宽松，命中任一即过；避免单纯大小写 / 空白差异误判）：
    //   1) 严格相等（用户用 modal 默认值直接提交时命中）
    //   2) case-insensitive trim 后相等（复制粘贴常见 lead/trail 全角空格）
    //   3) 仅匹配 "#N" 后缀（quick-confirm 副本号，老用户熟悉编号时用）
    const norm = (s: string) => s.trim().replace(/\s+/g, " ");
    const expected = norm(meta.display_name);
    const got = norm(typed ?? "");
    const ok =
      got === expected ||
      got.toLowerCase() === expected.toLowerCase() ||
      // 副本快速删除捷径：只输入 "#3" 也接受（前提是 display_name 含 "#N"）
      (expected.includes("#") && got === "#" + expected.split("#").pop());
    if (!ok) {
      console.warn("[delete-extra] name mismatch", {
        expected: meta.display_name,
        got: typed,
        normExpected: expected,
        normGot: got,
      });
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
        console.error("[delete-extra] missing extra_instance_uuid", meta);
        flash(t("settings.providers.delete_extra_no_uuid"), true);
        return;
      }
      console.info("[delete-extra] invoking delete_extra_instance", {
        id: meta.extra_instance_uuid,
        metaId: meta.id,
        displayName: meta.display_name,
      });
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
      console.error("[delete-extra] IPC failed:", e, {
        id: meta.extra_instance_uuid,
        metaId: meta.id,
      });
      flash(t("settings.providers.delete_failed", { err: String(e) }), true);
    }
  });
  return btn;
}

/// in-app 二次确认 modal：弹 `<dialog>` 让用户输入 display_name。
///
/// 优先用 in-app modal（更可靠，macOS WKWebView 不会被 system prompt 干扰）；
/// `promptDismissed=true`（用户点取消）返 `null`，跟 native `prompt` 语义一致。
/// 失败/未实现 modal 时 fallback 到 native `prompt()`。
async function promptForNameInApp(
  expectedName: string,
  promptText: string,
): Promise<string | null> {
  // Try in-app modal first.
  try {
    // 走 modal.ts 的 showModal（带 form method=dialog，ESC 关闭）
    const { showModal } = await import("./modal");
    const input = el("input", {
      type: "text",
      class: "delete-confirm-input",
      value: expectedName,
      autocomplete: "off",
      spellcheck: "false",
    }) as HTMLInputElement;
    // 自动 focus + 全选, 让用户一眼看到默认值, 直接覆盖即可
    // (setTimeout 0 跳过当前事件循环, 等 modal::showModal 完成)
    setTimeout(() => {
      input.focus();
      input.select();
    }, 0);
    const body = el("div", { class: "field" },
      el("p", { class: "help" }, promptText),
      input,
    );
    // 用户输入完成 → form submit，submitHandler 返 true 关闭 modal
    return await new Promise<string | null>((resolve) => {
      let resolved = false;
      const finish = (val: string | null) => {
        if (resolved) return;
        resolved = true;
        resolve(val);
      };
      showModal({
        title: t("settings.common.delete"), // 复用 "删除" 标题文案
        body,
        // submit 按钮 label 复用普通 "save" 文案（避免引入新的 i18n key）
        submitLabel: t("settings.common.save"),
        cancelLabel: t("settings.common.cancel"),
        onSubmit: async () => {
          finish(input.value);
          return true; // 关闭 modal
        },
      });
      // 用户点 cancel / ESC → modal 关闭但 onSubmit 不会被调，需要靠
      // dialog 的 'close' event 兜底
      const dialog = document.querySelector<HTMLDialogElement>("dialog.modal:last-of-type");
      if (dialog) {
        // 给 modal close 事件挂一次性 listener（cancel / ESC 触发）
        dialog.addEventListener("close", () => {
          // 如果 onSubmit 已经 resolve，不要重复 set
          if (!resolved) finish(null);
        }, { once: true });
      }
    });
  } catch (e) {
    console.warn("[delete-extra] in-app modal failed, fallback to native prompt", e);
    // Fallback: native prompt
    const r = prompt(promptText);
    return r ?? null;
  }
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