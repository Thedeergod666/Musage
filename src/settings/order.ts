// Provider 浮窗卡片顺序管理
//
// v0.6+ 支持：
// - **自定义拖拽**：mousedown/mousemove/mouseup 实现（不依赖 HTML5 DnD，
//   在 Tauri WKWebView 里可靠）
// - **↑↓ 按钮**：保留做快速单步调整 + accessibility
// - **即时响应**：DOM 交换在 IPC 之前完成（用户看到瞬间移位）；后端
//   set_provider_order 重排 in-memory snapshot 并 emit snapshot 给浮窗
// - **不抖动**："1/6" 固定格式 + font-variant-numeric: tabular-nums
//
// v0.6+ 新增：
// - **分区显示**：列表中部插入一条 "── 已隐藏（拖到上方即可在浮窗显示）──"
//   分隔线。分隔线之上是已勾选"在浮窗显示"的 provider（浮窗里会出现的卡）；
//   之下是未勾选的 provider。用户可拖动卡片跨过分隔线来切换是否在浮窗显示。
// - **位置控制可见性**：跨过分隔线的拖动同时更新 cfg.providers[id].enabled
//   + cfg.provider_order + 后端 emit snapshot —— 浮窗即时跟着显示/隐藏。
//
// v0.7+ 统一交互：
// - 移除 disabled 段卡片的「显示」按钮 —— 改用 ↑/↓ 按钮或拖拽跨过分隔线。
// - ↑/↓ 按钮允许跨段移动：在显示段最下面点 ↓ → 进入隐藏段首位；
//   在隐藏段最上面点 ↑ → 进入显示段末位。后端 set_provider_enabled
//   同步切换可见性。
// - 禁用规则统一：仅「显示段首张的 ↑」与「隐藏段末张的 ↓」被禁用（edge-of-list），
//   其余按钮全部可点 —— 包括跨段的那一对。
// - 分隔线本身可拖拽：用户可握住分隔线把它上下拖，跨越分隔线的卡片自动
//   切换 enabled 状态（拖下来 = 隐藏、拖上去 = 显示）。这是隐藏段的
//   另一种快捷调整方式。

import { setProviderOrder, setProviderEnabled } from "./api";
import {
  BUILTIN_ORDER,
  currentProviderOrder,
  el,
  flash,
  setCurrentProviderOrder,
} from "./utils";
import { getProviderMeta } from "./logos";
import type { AppConfig, ProviderId, SourceMeta } from "./types";

export function canonicalizeOrder(order: string[]): ProviderId[] {
  const ordered: ProviderId[] = [];
  for (const id of order) {
    if (
      (BUILTIN_ORDER as string[]).includes(id) &&
      !(ordered as string[]).includes(id)
    ) {
      ordered.push(id as ProviderId);
    }
  }
  for (const id of BUILTIN_ORDER) {
    if (!(ordered as string[]).includes(id)) ordered.push(id);
  }
  return ordered;
}

// ── 模块状态 ────────────────────────────────────────────────

let orderSources: SourceMeta[] = [];
let orderCfg: AppConfig | null = null;
let listRef: HTMLOListElement | null = null;

function isEnabledId(id: string): boolean {
  if (!orderCfg) return true;
  return orderCfg.providers?.[id]?.enabled ?? true;
}

/** 找到分隔点：currentProviderOrder 中第一个 disabled provider 的 index。
 *  若全部 enabled，返回 length（即分隔点在最末尾）。 */
function boundaryIdx(): number {
  for (let j = 0; j < currentProviderOrder.length; j++) {
    if (!isEnabledId(currentProviderOrder[j])) return j;
  }
  return currentProviderOrder.length;
}

// ── 自定义拖拽（mousedown/mousemove/mouseup）─────────────────

let dragging = false;
let dragSrcId: string | null = null;
let dragSrcIdx = -1;
let dragGhost: HTMLElement | null = null;
let dragPlaceholder: HTMLElement | null = null;
let dragOffsetY = 0;

function onDragMouseDown(e: MouseEvent) {
  // 只在左键点击 <li> 时启动
  if (e.button !== 0) return;
  const li = (e.target as HTMLElement).closest<HTMLLIElement>("li.order-row");
  if (!li) return;
  // 不拦截按钮点击
  if ((e.target as HTMLElement).closest("button")) return;

  e.preventDefault();
  dragSrcId = li.dataset.id ?? null;
  dragSrcIdx = currentProviderOrder.indexOf(dragSrcId!);
  const rect = li.getBoundingClientRect();
  dragOffsetY = e.clientY - rect.top;

  // 创建 ghost（半透明浮动克隆）
  dragGhost = li.cloneNode(true) as HTMLElement;
  dragGhost.classList.add("order-ghost");
  dragGhost.style.width = `${rect.width}px`;
  dragGhost.style.position = "fixed";
  dragGhost.style.left = `${rect.left}px`;
  dragGhost.style.top = `${e.clientY - dragOffsetY}px`;
  dragGhost.style.zIndex = "9999";
  dragGhost.style.pointerEvents = "none";
  dragGhost.style.opacity = "0.85";
  dragGhost.style.transition = "none";
  document.body.appendChild(dragGhost);

  // 原位 placeholder（虚线占位）
  dragPlaceholder = el("li", { class: "order-row order-placeholder" });
  dragPlaceholder.style.height = `${rect.height}px`;
  li.parentElement?.insertBefore(dragPlaceholder, li);
  li.style.display = "none";

  dragging = true;
  document.addEventListener("mousemove", onDragMouseMove);
  document.addEventListener("mouseup", onDragMouseUp);
}

function onDragMouseMove(e: MouseEvent) {
  if (!dragging || !dragGhost || !listRef) return;

  // 移动 ghost（保持 mousedown 时的 offset，光标位置 = ghost.top + offset）
  dragGhost.style.top = `${e.clientY - dragOffsetY}px`;

  // 找到光标所在的 <li>（不含 placeholder、divider、隐藏的 src li）
  const items = [...listRef.querySelectorAll("li.order-row:not(.order-placeholder):not([style*='display: none'])")] as HTMLLIElement[];
  let insertIdx = items.length; // 默认插到末尾

  for (let i = 0; i < items.length; i++) {
    const rect = items[i].getBoundingClientRect();
    const midY = rect.top + rect.height / 2;
    if (e.clientY < midY) {
      insertIdx = i;
      break;
    }
  }

  // 移动 placeholder 到 insertIdx
  if (dragPlaceholder && listRef) {
    const children = [...listRef.children] as HTMLElement[];
    const visibleItems = children.filter(
      (c) => c !== dragPlaceholder && !(c as HTMLElement).style?.display?.includes("none"),
    );
    if (insertIdx < visibleItems.length) {
      listRef.insertBefore(dragPlaceholder, visibleItems[insertIdx]);
    } else {
      listRef.appendChild(dragPlaceholder);
    }
  }
}

function onDragMouseUp(_e: MouseEvent) {
  if (!dragging) return;
  document.removeEventListener("mousemove", onDragMouseMove);
  document.removeEventListener("mouseup", onDragMouseUp);

  // 恢复源 li
  const srcLi = listRef?.querySelector(`li[data-id="${dragSrcId}"]`) as HTMLElement | null;
  if (srcLi) srcLi.style.display = "";

  // 计算新位置：placeholder 在 list 里的 index
  let newIdx = 0;
  let placeholderBeforeDivider = true;
  if (dragPlaceholder && listRef) {
    const children = [...listRef.children];
    newIdx = children.indexOf(dragPlaceholder);
    if (newIdx < 0) newIdx = currentProviderOrder.length;
    // 检测 placeholder 是否落在 divider 之后 → 视为"未勾选"区
    const dividerIdx = children.findIndex((c) => c.classList.contains("order-divider"));
    placeholderBeforeDivider = dividerIdx < 0 || newIdx < dividerIdx;
  }

  // 移除 placeholder + ghost
  dragPlaceholder?.remove();
  dragGhost?.remove();
  dragPlaceholder = null;
  dragGhost = null;

  if (dragSrcIdx < 0 || !dragSrcId) {
    dragging = false;
    dragSrcId = null;
    dragSrcIdx = -1;
    return;
  }

  // ── 跨区分支：源在 enabled 段，但放到 divider 之后 → 视为禁用 ──
  //   源在 disabled 段，但放到 divider 之前 → 视为启用。
  //   源和落点在同一段 → 单纯改顺序（老逻辑）。
  const wasEnabled = isEnabledId(dragSrcId);
  const willBeEnabled = placeholderBeforeDivider;
  const crossedDivider = wasEnabled !== willBeEnabled;

  // 把 newIdx（DOM 位置，含 divider）映射到 currentProviderOrder 的 index。
  // 列表 DOM 里有一个 divider li，divider 之前/之后各是 enabled/disabled 段。
  // divider 本身不计入 currentProviderOrder。
  let orderIdx = newIdx;
  if (listRef) {
    const dividerEl = listRef.querySelector(".order-divider");
    if (dividerEl) {
      const divIdx = [...listRef.children].indexOf(dividerEl);
      if (newIdx > divIdx) orderIdx = newIdx - 1;
    }
  }

  if (orderIdx === dragSrcIdx && !crossedDivider) {
    // 没移动也没跨分隔线，恢复原状
    if (listRef) buildOrderItems(listRef);
    dragging = false;
    dragSrcId = null;
    dragSrcIdx = -1;
    return;
  }

  // 执行移位（在 currentProviderOrder 里 splice）
  const moved = currentProviderOrder.splice(dragSrcIdx, 1)[0];
  // splice 后 index 都可能 -1（拖到原位置之后）
  const adjusted = orderIdx > dragSrcIdx ? orderIdx - 1 : orderIdx;
  currentProviderOrder.splice(adjusted, 0, moved);

  // DOM 重建（含 divider）
  if (listRef) buildOrderItems(listRef);

  if (crossedDivider) {
    // ── 乐观更新（fix bug #3 + #4）：先在内存里翻 enabled flag、sync
    // provider panel checkbox + 立即 rebuild，再异步 IPC 落盘。──
    if (orderCfg && dragSrcId) {
      if (!orderCfg.providers) orderCfg.providers = {};
      const entry = orderCfg.providers[dragSrcId] ?? { enabled: willBeEnabled };
      orderCfg.providers[dragSrcId] = { ...entry, enabled: willBeEnabled };
    }
    if (dragSrcId) {
      const cb = document.getElementById(`enabled-${dragSrcId}`) as HTMLInputElement | null;
      if (cb) cb.checked = willBeEnabled;
    }
    refreshPosLabels();

    void (async () => {
      try {
        await setProviderEnabled(dragSrcId!, willBeEnabled);
        await setProviderOrder(currentProviderOrder);
        flash(
          willBeEnabled
            ? `✓ ${dragSrcId} 已移到浮窗显示区`
            : `✓ ${dragSrcId} 已隐藏（拖回上方可恢复）`,
        );
      } catch (e) {
        flash(`✗ 切换显示失败: ${e}`, true);
      }
    })();
  } else {
    void commitOrder(adjusted, moved);
  }

  dragging = false;
  dragSrcId = null;
  dragSrcIdx = -1;
}

// ── 分隔线拖拽 ──────────────────────────────────────────────

let dividerDragging = false;
let dividerGhost: HTMLElement | null = null;
let dividerPlaceholder: HTMLElement | null = null;
let dividerOffsetY = 0;

function onDividerMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  if (!listRef) return;
  const divider = (e.target as HTMLElement).closest<HTMLLIElement>("li.order-divider");
  if (!divider) return;
  // 只接受 divider 本身或其文字标签被按下；不要拦截 row 上的事件
  if ((e.target as HTMLElement).closest("li.order-row")) return;

  e.preventDefault();
  const rect = divider.getBoundingClientRect();
  dividerOffsetY = e.clientY - rect.top;

  // ghost：浮动分隔线
  dividerGhost = divider.cloneNode(true) as HTMLElement;
  dividerGhost.classList.add("order-divider-ghost");
  dividerGhost.style.width = `${rect.width}px`;
  dividerGhost.style.position = "fixed";
  dividerGhost.style.left = `${rect.left}px`;
  dividerGhost.style.top = `${e.clientY - dividerOffsetY}px`;
  dividerGhost.style.zIndex = "9999";
  dividerGhost.style.pointerEvents = "none";
  dividerGhost.style.opacity = "0.85";
  document.body.appendChild(dividerGhost);

  // placeholder：原位占位
  dividerPlaceholder = el("li", { class: "order-divider order-divider-placeholder" });
  dividerPlaceholder.style.height = `${rect.height}px`;
  divider.parentElement?.insertBefore(dividerPlaceholder, divider);
  divider.style.display = "none";

  dividerDragging = true;
  document.addEventListener("mousemove", onDividerMouseMove);
  document.addEventListener("mouseup", onDividerMouseUp);
}

function onDividerMouseMove(e: MouseEvent) {
  if (!dividerDragging || !dividerGhost || !dividerPlaceholder || !listRef) return;
  // ghost 跟随光标（保持 mousedown 时记录的 offset）
  dividerGhost.style.top = `${e.clientY - dividerOffsetY}px`;

  // 找到 insert 位置：基于 row 的中线
  const items = [...listRef.querySelectorAll("li.order-row:not([style*='display: none'])")] as HTMLLIElement[];
  let insertIdx = items.length;
  for (let i = 0; i < items.length; i++) {
    const midY = items[i].getBoundingClientRect().top + items[i].getBoundingClientRect().height / 2;
    if (e.clientY < midY) {
      insertIdx = i;
      break;
    }
  }
  if (insertIdx < items.length) {
    listRef.insertBefore(dividerPlaceholder, items[insertIdx]);
  } else {
    listRef.appendChild(dividerPlaceholder);
  }
}

function onDividerMouseUp(_e: MouseEvent) {
  if (!dividerDragging) return;
  document.removeEventListener("mousemove", onDividerMouseMove);
  document.removeEventListener("mouseup", onDividerMouseUp);

  const originalDivider = listRef?.querySelector("li.order-divider:not(.order-divider-placeholder)") as HTMLElement | null;
  if (originalDivider) originalDivider.style.display = "";

  // 找 placeholder 位置对应的"分割点"：上面有多少个 row
  let newBoundaryPos = 0;
  if (dividerPlaceholder && listRef) {
    const children = [...listRef.children];
    const placeholderIdx = children.indexOf(dividerPlaceholder);
    for (let i = 0; i < placeholderIdx; i++) {
      if (children[i].classList.contains("order-row")) newBoundaryPos++;
    }
  }

  // 清理 ghost / placeholder
  dividerPlaceholder?.remove();
  dividerGhost?.remove();
  dividerPlaceholder = null;
  dividerGhost = null;
  dividerDragging = false;

  const oldBoundary = boundaryIdx();
  if (newBoundaryPos === oldBoundary) {
    // 没移动
    if (listRef) buildOrderItems(listRef);
    return;
  }

  // 算出要切换 enabled 的 provider 列表
  const toEnable: string[] = [];
  const toDisable: string[] = [];
  if (newBoundaryPos > oldBoundary) {
    // 边界下移：[oldBoundary, newBoundary) 从 disabled → enabled
    for (let i = oldBoundary; i < newBoundaryPos; i++) {
      const id = currentProviderOrder[i];
      if (!isEnabledId(id)) toEnable.push(id);
    }
  } else {
    // 边界上移：[newBoundary, oldBoundary) 从 enabled → disabled
    for (let i = newBoundaryPos; i < oldBoundary; i++) {
      const id = currentProviderOrder[i];
      if (isEnabledId(id)) toDisable.push(id);
    }
  }

  // DOM 立即按新分割点重建（给用户即时反馈）
  if (listRef) buildOrderItems(listRef);

  // ── 乐观更新（fix bug #3 + #4）：先翻内存 flag + 同步 checkbox，再 IPC ──
  if (orderCfg) {
    if (!orderCfg.providers) orderCfg.providers = {};
    for (const id of toEnable) {
      const entry = orderCfg.providers[id] ?? { enabled: true };
      orderCfg.providers[id] = { ...entry, enabled: true };
      const cb = document.getElementById(`enabled-${id}`) as HTMLInputElement | null;
      if (cb) cb.checked = true;
    }
    for (const id of toDisable) {
      const entry = orderCfg.providers[id] ?? { enabled: false };
      orderCfg.providers[id] = { ...entry, enabled: false };
      const cb = document.getElementById(`enabled-${id}`) as HTMLInputElement | null;
      if (cb) cb.checked = false;
    }
  }
  refreshPosLabels();

  // 顺序触发后端切换（避免并发 emit snapshot 导致浮窗闪烁）
  void (async () => {
    try {
      for (const id of toEnable) {
        await setProviderEnabled(id, true);
      }
      for (const id of toDisable) {
        await setProviderEnabled(id, false);
      }
      const delta = newBoundaryPos - oldBoundary;
      flash(
        delta > 0
          ? `✓ 已新增 ${delta} 张卡片到浮窗`
          : `✓ 已隐藏 ${-delta} 张卡片`,
      );
    } catch (e) {
      flash(`✗ 调整失败: ${e}`, true);
    }
  })();
}

// ── 渲染 ────────────────────────────────────────────────────

export function renderOrderSection(
  container: HTMLElement,
  sources: SourceMeta[],
  cfgProviderOrder: string[] | undefined,
  cfg: AppConfig | null = null,
) {
  setCurrentProviderOrder(canonicalizeOrder(cfgProviderOrder ?? []));
  orderSources = sources;
  orderCfg = cfg;

  const list = el("ol", { class: "order-list" }) as HTMLOListElement;
  listRef = list;
  buildOrderItems(list);

  // 绑定 mousedown（卡片拖拽 + 分隔线拖拽）
  list.addEventListener("mousedown", onDragMouseDown);
  list.addEventListener("mousedown", onDividerMouseDown);

  const section = el(
    "section",
    { class: "order-section section-card" },
    el("h2", {}, "浮窗卡片顺序"),
    list,
  );
  const old = container.querySelector(".order-section");
  if (old) old.remove();
  container.prepend(section);
}

/// 接收最新的 cfg（provider panel 改了 enabled 后通知过来）
export function updateOrderConfig(cfg: AppConfig) {
  orderCfg = cfg;
  if (listRef) buildOrderItems(listRef);
}

function buildOrderItems(list: HTMLOListElement) {
  list.innerHTML = "";
  // 分两段：enabled 在上、disabled 在下，中间一条 divider。
  // 段内各自按 currentProviderOrder 出现顺序排（用户在段内拖拽时已经
  // 调整过 currentProviderOrder 的对应切片）。
  const enabledIds: string[] = [];
  const disabledIds: string[] = [];
  for (const id of currentProviderOrder) {
    if (isEnabledId(id)) enabledIds.push(id);
    else disabledIds.push(id);
  }
  // 兜底：任何 builtin 但不在 currentProviderOrder 里的 id，按 enabled
  // 状态加进对应段（首次启动时 order 为空、但每个 provider 都有 enabled）。
  for (const id of BUILTIN_ORDER as readonly string[]) {
    if (currentProviderOrder.includes(id as ProviderId)) continue;
    if (isEnabledId(id)) enabledIds.push(id);
    else disabledIds.push(id);
  }

  let pos = 0;
  for (const id of enabledIds) {
    list.appendChild(buildRow(id, pos, enabledIds.length + disabledIds.length, "enabled"));
    pos++;
  }
  // 分隔线永远渲染：即使 disabledIds 为 0 也保留，让用户能拖下去添加
  // 隐藏项 —— 同时视觉上保持「显示段 / 隐藏段」分区恒在。
  list.appendChild(buildDivider());
  if (disabledIds.length > 0) {
    for (const id of disabledIds) {
      list.appendChild(buildRow(id, pos, enabledIds.length + disabledIds.length, "disabled"));
      pos++;
    }
  }
}

function buildDivider(): HTMLElement {
  return el(
    "li",
    {
      class: "order-divider",
      "aria-hidden": "true",
      title: "按住拖动可调整浮窗显示数量",
    },
    el("span", { class: "order-divider-grip", "aria-hidden": "true" }, "⋮⋮"),
    el("span", { class: "order-divider-line" }),
    el("span", { class: "order-divider-label" }, "已隐藏"),
    el("span", { class: "order-divider-line" }),
  );
}

function buildRow(id: string, idx: number, total: number, section: "enabled" | "disabled"): HTMLElement {
  const meta = orderSources.find((s) => s.id === id);
  const providerMeta = getProviderMeta(id);
  const logo = providerMeta
    ? el("img", {
        class: "order-logo",
        src: providerMeta.logo,
        alt: providerMeta.name,
      })
    : null;
  const displayName = meta?.display_name ?? providerMeta?.name ?? id;
  const li = el(
    "li",
    { class: `order-row order-row-${section}`, "data-id": id },
    el(
      "div",
      { class: "order-row-left" },
      ...(logo ? [logo] : []),
      el("span", { class: "order-pos", "data-id": id }, posLabel(idx)),
      el("span", { class: "order-name" }, displayName),
    ),
    el(
      "div",
      { class: "order-btns" },
      el("button", { class: "order-up", "data-id": id, type: "button", title: "上移" }, "↑"),
      el("button", { class: "order-down", "data-id": id, type: "button", title: "下移" }, "↓"),
    ),
  );
  refreshRowButtons(li, idx, total);
  return li;
}

/** 统一规则：
 *  - up 仅在「显示段第一张」时禁用（idx === 0；disabled 段的 ↑ 永远可点）
 *  - down 仅在「隐藏段最后一张」时禁用（disabled 且 idx === total - 1）
 *  - 其余全部可点 —— 包括「显示段最后一张的 ↓」（跨段进隐藏段首位） */
function refreshRowButtons(li: HTMLElement, idx: number, total: number) {
  const upBtn = li.querySelector<HTMLButtonElement>(".order-up");
  const downBtn = li.querySelector<HTMLButtonElement>(".order-down");
  if (upBtn) upBtn.disabled = idx === 0;
  if (downBtn) {
    const isLastDisabled =
      li.classList.contains("order-row-disabled") && idx === total - 1;
    downBtn.disabled = isLastDisabled;
  }
}

function posLabel(i: number): string {
  return `${i + 1}/${currentProviderOrder.length}`;
}

// ── ↑↓ 按钮 ─────────────────────────────────────────────────

export async function moveProviderInOrder(id: string, dir: "up" | "down") {
  const idx = currentProviderOrder.indexOf(id);
  if (idx < 0) return;

  const boundary = boundaryIdx();
  const wasEnabled = isEnabledId(id);
  const isLastEnabled = wasEnabled && idx === boundary - 1;
  const isFirstDisabled = !wasEnabled && idx === boundary;

  // 跨段快捷：last enabled 的 ↓ / first disabled 的 ↑ 直接切换 enabled
  // 状态（currentProviderOrder 数组本身不需要 reorder —— 跨段时该卡在
  // 数组中的"逻辑位置"已经天然落在对方段的边界上）。
  let willCrossBoundary = false;
  if (dir === "up") {
    if (idx === 0) return; // 第一张 enabled 的 ↑ 被禁用（按钮也已 disabled 兜底）
    if (isFirstDisabled) {
      // 隐藏段首张 ↑ → 进入显示段末尾
      willCrossBoundary = true;
    }
  } else {
    // 只有「隐藏段最后一张」的 ↓ 不响应；显示段最后一张的 ↓ 仍可点（跨段进隐藏段）
    if (!wasEnabled && idx === currentProviderOrder.length - 1) return;
    if (isLastEnabled) {
      // 显示段末张 ↓ → 进入隐藏段首位
      willCrossBoundary = true;
    }
  }

  if (willCrossBoundary) {
    // ── 乐观更新（fix bug #4：避免 rebuild 用旧 cfg 导致 partition 不变）──
    // 1) 改 orderCfg 内存里的 enabled flag，让 buildOrderItems 用新值
    // 2) 同步 provider panel 的 checkbox（fix bug #3：UI 不同步）
    // 3) 全量 rebuild + refreshPosLabels（位置标签 6/8 → 7/8 同步）
    const newEnabled = !wasEnabled;
    if (orderCfg) {
      if (!orderCfg.providers) orderCfg.providers = {};
      const entry = orderCfg.providers[id] ?? { enabled: newEnabled };
      orderCfg.providers[id] = { ...entry, enabled: newEnabled };
    }
    const cb = document.getElementById(`enabled-${id}`) as HTMLInputElement | null;
    if (cb) cb.checked = newEnabled;
    if (listRef) buildOrderItems(listRef);
    refreshPosLabels();

    // IPC 后端落盘 + emit snapshot（异步，不阻塞视觉）
    void (async () => {
      try {
        await setProviderEnabled(id, newEnabled);
        flash(
          wasEnabled
            ? `✓ ${id} 已隐藏（点 ↑ 或拖回上方可恢复）`
            : `✓ ${id} 已移到浮窗显示区`,
        );
      } catch (e) {
        // IPC 失败 → 回滚
        if (orderCfg?.providers?.[id]) {
          orderCfg.providers[id] = {
            ...orderCfg.providers[id],
            enabled: wasEnabled,
          };
        }
        if (cb) cb.checked = wasEnabled;
        if (listRef) buildOrderItems(listRef);
        refreshPosLabels();
        flash(`✗ 切换失败: ${e}`, true);
      }
    })();
    return;
  }

  // 同段移动：在 currentProviderOrder 里 splice + 轻量 DOM 交换
  const targetIdx = dir === "up" ? idx - 1 : idx + 1;
  const moved = currentProviderOrder.splice(idx, 1)[0];
  const adjusted = targetIdx > idx ? targetIdx - 1 : targetIdx;
  currentProviderOrder.splice(adjusted, 0, moved);

  // DOM 同步：listRef.children 含 divider，disabled 段的 DOM 索引比
  // currentProviderOrder 索引大 1。算出映射后再 insertBefore。
  if (listRef) {
    const boundary = boundaryIdx();
    const fromDomIdx = idx >= boundary ? idx + 1 : idx;
    const toDomIdx = adjusted >= boundary ? adjusted + 1 : adjusted;
    const items = [...listRef.children] as HTMLElement[];
    const fromItem = items[fromDomIdx];
    const toItem = items[toDomIdx];
    if (fromItem && toItem) {
      if (dir === "up") {
        listRef.insertBefore(fromItem, toItem);
      } else {
        listRef.insertBefore(fromItem, toItem.nextSibling);
      }
    }
  }

  refreshPosLabels();
  void commitOrder(adjusted, moved);
}

async function commitOrder(finalIdx: number, id: string) {
  try {
    await setProviderOrder(currentProviderOrder);
    flash(`✓ ${id} 已移到位置 ${finalIdx + 1}`);
  } catch (e) {
    flash(`✗ 调整顺序失败: ${e}`, true);
  }
}

function refreshPosLabels() {
  for (let i = 0; i < currentProviderOrder.length; i++) {
    const id = currentProviderOrder[i];
    const posEl = document.querySelector<HTMLElement>(`.order-pos[data-id="${id}"]`);
    if (posEl) posEl.textContent = posLabel(i);
    const upBtn = document.querySelector<HTMLButtonElement>(`.order-up[data-id="${id}"]`);
    const downBtn = document.querySelector<HTMLButtonElement>(`.order-down[data-id="${id}"]`);
    if (upBtn) upBtn.disabled = i === 0;
    if (downBtn) {
      const row = upBtn?.closest("li.order-row");
      const isLastDisabled =
        !!row?.classList.contains("order-row-disabled") &&
        i === currentProviderOrder.length - 1;
      downBtn.disabled = isLastDisabled;
    }
  }
}

// ── 全局按钮委托 ────────────────────────────────────────────

export function bindOrderButtonsGlobal() {
  document.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    if (target.classList.contains("order-up")) {
      const id = target.dataset.id;
      if (id) void moveProviderInOrder(id, "up");
    } else if (target.classList.contains("order-down")) {
      const id = target.dataset.id;
      if (id) void moveProviderInOrder(id, "down");
    }
  });
}

export function renderProviderOrderPanels(order: string[]) {
  setCurrentProviderOrder(canonicalizeOrder(order));
  refreshPosLabels();
}