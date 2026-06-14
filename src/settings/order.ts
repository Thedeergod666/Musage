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

import { setProviderOrder, setProviderEnabled, getConfig } from "./api";
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

// ── 自定义拖拽（mousedown/mousemove/mouseup）─────────────────

let dragging = false;
let dragSrcId: string | null = null;
let dragSrcIdx = -1;
let dragGhost: HTMLElement | null = null;
let dragPlaceholder: HTMLElement | null = null;
let dragStartY = 0;
let dragOffsetY = 0;
void dragStartY; // used in onDragMouseDown for reference

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
  dragStartY = e.clientY;
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

  // 移动 ghost
  dragGhost.style.top = `${e.clientY - dragOffsetY}px`;

  // 找到光标所在的 <li>（不含 placeholder）
  const items = [...listRef.querySelectorAll("li.order-row:not(.order-placeholder):not([style*='display: none'])")] as HTMLLIElement[];
  let insertIdx = currentProviderOrder.length; // 默认插到末尾

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
    // 找到 insertIdx 对应的实际 DOM 位置（跳过隐藏的 src li）
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
    // 跨过分隔线：先落盘 enabled，再落盘 order。Rust 端 set_provider_enabled
    // 内部已经会 emit snapshot，set_provider_order 又会再 emit 一次。两次
    // 几乎同时发到浮窗，浮窗 listen 端会按到达顺序处理（最终是 enabled 切换
    // 后的新 order）—— 但为了避免极端竞态，先 await 一下 enabled。
    void (async () => {
      try {
        await setProviderEnabled(dragSrcId!, willBeEnabled);
        await setProviderOrder(currentProviderOrder);
        // 拉最新 cfg 刷新"已勾选"checkbox 状态（让 provider panel 同步）
        const cfg = await getConfig();
        orderCfg = cfg;
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

  // 绑定 mousedown（自定义拖拽）
  list.addEventListener("mousedown", onDragMouseDown);

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
  if (disabledIds.length > 0) {
    list.appendChild(buildDivider());
    for (const id of disabledIds) {
      list.appendChild(buildRow(id, pos, enabledIds.length + disabledIds.length, "disabled"));
      pos++;
    }
  }
}

function buildDivider(): HTMLElement {
  return el(
    "li",
    { class: "order-divider", "aria-hidden": "true" },
    el("span", { class: "order-divider-line" }),
    el("span", { class: "order-divider-label" }, "已隐藏（拖到上方即可在浮窗显示）"),
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
  // disabled 段加一个"在浮窗显示"快速勾选按钮（避免用户必须滚到底部找 checkbox）
  if (section === "disabled") {
    const showBtn = el("button", {
      class: "order-show",
      "data-id": id,
      type: "button",
      title: "在浮窗显示",
    }, "显示");
    showBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      void (async () => {
        try {
          // 把这个 id 挪到 enabled 段最末尾（其它顺序不变）
          const i = currentProviderOrder.indexOf(id);
          if (i >= 0) currentProviderOrder.splice(i, 1);
          // 找当前 enabled 段的末尾：第一个 disabled id 之前，或 list 末尾
          let insertAt = currentProviderOrder.length;
          for (let j = 0; j < currentProviderOrder.length; j++) {
            if (!isEnabledId(currentProviderOrder[j])) {
              insertAt = j;
              break;
            }
          }
          currentProviderOrder.splice(insertAt, 0, id);
          buildOrderItems(listRef!);
          await setProviderEnabled(id, true);
          await setProviderOrder(currentProviderOrder);
          const cfg = await getConfig();
          orderCfg = cfg;
          flash(`✓ ${displayName} 已显示在浮窗`);
        } catch (err) {
          flash(`✗ 切换失败: ${err}`, true);
        }
      })();
    });
    li.querySelector(".order-btns")?.appendChild(showBtn);
  }
  refreshRowButtons(li, idx, total);
  return li;
}

function refreshRowButtons(li: HTMLElement, idx: number, total: number) {
  // enabled 段：上移到顶就禁 up，下移到底就禁 down
  // disabled 段：单独成段，上移到顶就禁 up，下移到底就禁 down（与段内相对位置一致）
  const upBtn = li.querySelector<HTMLButtonElement>(".order-up");
  const downBtn = li.querySelector<HTMLButtonElement>(".order-down");
  if (upBtn) upBtn.disabled = idx === 0;
  if (downBtn) downBtn.disabled = idx === total - 1;
}

function posLabel(i: number): string {
  return `${i + 1}/${currentProviderOrder.length}`;
}

// ── ↑↓ 按钮 ─────────────────────────────────────────────────

export async function moveProviderInOrder(id: string, dir: "up" | "down") {
  const idx = currentProviderOrder.indexOf(id);
  if (idx < 0) return;
  const newIdx = dir === "up" ? idx - 1 : idx + 1;
  if (newIdx < 0 || newIdx >= currentProviderOrder.length) return;

  // 阻止跨段移动：↑/↓ 只在 enabled/disabled 同段内有效。跨段改用拖拽
  // 或 disabled 段的"显示"按钮 —— 避免单步按钮不小心把 provider
  // 隐藏/显示。
  const wasEnabled = isEnabledId(id);
  const willBeEnabled = isEnabledId(currentProviderOrder[newIdx]);
  if (wasEnabled !== willBeEnabled) {
    flash("⚠ 跨区移动请用拖拽（或点「显示」按钮）", true);
    return;
  }

  const list = document.querySelector<HTMLOListElement>(".order-list");
  if (list) {
    const items = [...list.children] as HTMLElement[];
    const fromItem = items[idx];
    const toItem = items[newIdx];
    if (fromItem && toItem) {
      if (dir === "up") {
        list.insertBefore(fromItem, toItem);
      } else {
        list.insertBefore(fromItem, toItem.nextSibling);
      }
    }
  }

  [currentProviderOrder[idx], currentProviderOrder[newIdx]] = [
    currentProviderOrder[newIdx],
    currentProviderOrder[idx],
  ];

  refreshPosLabels();
  void commitOrder(newIdx, id);
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
    if (downBtn) downBtn.disabled = i === currentProviderOrder.length - 1;
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
