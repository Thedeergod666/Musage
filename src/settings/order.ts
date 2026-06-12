// Provider 浮窗卡片顺序管理
//
// v0.6+ 支持：
// - **自定义拖拽**：mousedown/mousemove/mouseup 实现（不依赖 HTML5 DnD，
//   在 Tauri WKWebView 里可靠）
// - **↑↓ 按钮**：保留做快速单步调整 + accessibility
// - **即时响应**：DOM 交换在 IPC 之前完成（用户看到瞬间移位）；后端
//   set_provider_order 重排 in-memory snapshot 并 emit snapshot 给浮窗
// - **不抖动**："1/6" 固定格式 + font-variant-numeric: tabular-nums

import { setProviderOrder } from "./api";
import {
  BUILTIN_ORDER,
  currentProviderOrder,
  el,
  flash,
  setCurrentProviderOrder,
} from "./utils";
import { getProviderMeta } from "./logos";
import type { ProviderId, SourceMeta } from "./types";

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
let listRef: HTMLOListElement | null = null;

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
  if (dragPlaceholder && listRef) {
    const children = [...listRef.children];
    newIdx = children.indexOf(dragPlaceholder);
    if (newIdx < 0) newIdx = currentProviderOrder.length;
  }

  // 移除 placeholder + ghost
  dragPlaceholder?.remove();
  dragGhost?.remove();
  dragPlaceholder = null;
  dragGhost = null;

  // 执行移位
  if (dragSrcIdx >= 0 && newIdx !== dragSrcIdx && dragSrcId) {
    const moved = currentProviderOrder.splice(dragSrcIdx, 1)[0];
    // newIdx 是 placeholder 在 visible items 中的位置，需要映射到 currentProviderOrder
    // 因为 splice 已经移除了 src，后面的 index 都 -1
    const adjustedIdx = newIdx > dragSrcIdx ? newIdx - 1 : newIdx;
    currentProviderOrder.splice(adjustedIdx, 0, moved);

    // DOM 重建
    if (listRef) buildOrderItems(listRef, orderSources);

    // IPC
    void commitOrder(adjustedIdx, moved);
  } else {
    // 没移动，恢复原状
    if (listRef) buildOrderItems(listRef, orderSources);
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
) {
  setCurrentProviderOrder(canonicalizeOrder(cfgProviderOrder ?? []));
  orderSources = sources;

  const list = el("ol", { class: "order-list" }) as HTMLOListElement;
  listRef = list;
  buildOrderItems(list, sources);

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

function buildOrderItems(list: HTMLOListElement, sources: SourceMeta[]) {
  list.innerHTML = "";
  for (let i = 0; i < currentProviderOrder.length; i++) {
    const id = currentProviderOrder[i];
    const meta = sources.find((s) => s.id === id);
    if (!meta) continue;
    const providerMeta = getProviderMeta(id);
    const logo = providerMeta
      ? el("img", {
          class: "order-logo",
          src: providerMeta.logo,
          alt: providerMeta.name,
        })
      : null;

    const li = el(
      "li",
      { class: "order-row", "data-id": id },
      el(
        "div",
        { class: "order-row-left" },
        ...(logo ? [logo] : []),
        el("span", { class: "order-pos", "data-id": id }, posLabel(i)),
        el("span", { class: "order-name" }, meta.display_name),
      ),
      el(
        "div",
        { class: "order-btns" },
        el("button", { class: "order-up", "data-id": id, type: "button", title: "上移" }, "↑"),
        el("button", { class: "order-down", "data-id": id, type: "button", title: "下移" }, "↓"),
      ),
    );
    const upBtn = li.querySelector<HTMLButtonElement>(".order-up")!;
    const downBtn = li.querySelector<HTMLButtonElement>(".order-down")!;
    upBtn.disabled = i === 0;
    downBtn.disabled = i === currentProviderOrder.length - 1;

    list.appendChild(li);
  }
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

  const list = document.querySelector<HTMLOListElement>(".order-list");
  if (list) {
    const items = [...list.children] as HTMLLIElement[];
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
