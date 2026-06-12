// Provider 浮窗卡片顺序管理
//
// v0.6+ 支持：
// - **拖拽**：每个 `<li draggable>` 可拖拽到任意位置（HTML5 Drag & Drop API）
// - **↑↓ 按钮**：保留做快速单步调整 + accessibility
// - **即时响应**：DOM 交换在 IPC 之前完成（用户看到瞬间移位）；后端
//   set_provider_order 重排 in-memory snapshot 并 emit snapshot 给浮窗
// - **不抖动**："1/6" 固定格式 + `font-variant-numeric: tabular-nums`

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

/// 用 config.order 做主序，没列出的 provider 沉到末尾（用 builtin 顺序）
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

/// 拖拽状态（模块级单例）
let draggedId: string | null = null;
let dragOverId: string | null = null;
/// renderOrderSection 传入的 sources 引用（drop handler 重建 DOM 需要用）
let orderSources: SourceMeta[] = [];

/// 顶部的「浮窗卡片顺序」区块：列出当前所有 source + ↑↓ + 拖拽。
/// 插入到 providers section 的最顶端。
export function renderOrderSection(
  container: HTMLElement,
  sources: SourceMeta[],
  cfgProviderOrder: string[] | undefined,
) {
  setCurrentProviderOrder(canonicalizeOrder(cfgProviderOrder ?? []));
  orderSources = sources;

  const list = el("ol", { class: "order-list" }) as HTMLOListElement;
  buildOrderItems(list, sources);

  // document-level 拖拽事件（只绑一次，委托在 order-list 上）
  bindDragEvents(list);

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

/// 根据 currentProviderOrder 重新生成 `<li>` 元素。
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
          draggable: "false",
        })
      : null;

    const li = el(
      "li",
      { class: "order-row", draggable: "true", "data-id": id },
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
    // 首/末 disabled
    const upBtn = li.querySelector<HTMLButtonElement>(".order-up")!;
    const downBtn = li.querySelector<HTMLButtonElement>(".order-down")!;
    upBtn.disabled = i === 0;
    downBtn.disabled = i === currentProviderOrder.length - 1;

    list.appendChild(li);
  }
}

/// "1/6" 固定格式（比 "位置 1 / 6" 短，宽度更稳）
function posLabel(i: number): string {
  return `${i + 1}/${currentProviderOrder.length}`;
}

// ── 拖拽 ────────────────────────────────────────────────────

function bindDragEvents(list: HTMLOListElement) {
  list.addEventListener("dragstart", (e) => {
    const li = (e.target as HTMLElement).closest<HTMLLIElement>("li.order-row");
    if (!li) return;
    draggedId = li.dataset.id ?? null;
    li.classList.add("dragging");
    // dragImage 用默认即可（浏览器会截图 li）
    e.dataTransfer!.effectAllowed = "move";
  });

  list.addEventListener("dragover", (e) => {
    e.preventDefault();
    e.dataTransfer!.dropEffect = "move";
    const li = (e.target as HTMLElement).closest<HTMLLIElement>("li.order-row");
    if (!li || li.dataset.id === draggedId) return;

    // 之前的 drag-over 清掉
    if (dragOverId && dragOverId !== li.dataset.id) {
      const prev = list.querySelector(`.order-row[data-id="${dragOverId}"]`);
      prev?.classList.remove("drag-over-top", "drag-over-bottom");
    }
    dragOverId = li.dataset.id!;

    // 光标在 li 上半/下半 → 标记"插入到前面/后面"
    const rect = li.getBoundingClientRect();
    const midY = rect.top + rect.height / 2;
    li.classList.remove("drag-over-top", "drag-over-bottom");
    if (e.clientY < midY) {
      li.classList.add("drag-over-top");
    } else {
      li.classList.add("drag-over-bottom");
    }
  });

  list.addEventListener("dragleave", (e) => {
    const li = (e.target as HTMLElement).closest<HTMLLIElement>("li.order-row");
    if (li) {
      li.classList.remove("drag-over-top", "drag-over-bottom");
    }
  });

  list.addEventListener("drop", (e) => {
    e.preventDefault();
    const li = (e.target as HTMLElement).closest<HTMLLIElement>("li.order-row");
    if (!li || !draggedId || li.dataset.id === draggedId) {
      cleanupDrag(list);
      return;
    }

    const targetId = li.dataset.id!;
    const fromIdx = currentProviderOrder.indexOf(draggedId);
    const toIdx = currentProviderOrder.indexOf(targetId);
    if (fromIdx < 0 || toIdx < 0) {
      cleanupDrag(list);
      return;
    }

    // 判断插入位置：光标在目标上半 → 插到目标前面；下半 → 插到目标后面
    const rect = li.getBoundingClientRect();
    const midY = rect.top + rect.height / 2;
    const insertBefore = e.clientY < midY;

    // 计算新位置
    let newIdx = insertBefore ? toIdx : toIdx + 1;
    if (fromIdx < newIdx) newIdx--; // 移除 fromIdx 后后面的 index 会 -1

    // 执行移位
    const moved = currentProviderOrder.splice(fromIdx, 1)[0];
    currentProviderOrder.splice(newIdx, 0, moved);

    // DOM 重建（拖拽后整个 list 需要刷新位置编号 + disabled 状态）
    buildOrderItems(list, orderSources);

    // IPC（异步，不阻塞 UI）
    void commitOrder(newIdx, moved);
  });

  list.addEventListener("dragend", () => {
    cleanupDrag(list);
  });
}

function cleanupDrag(list: HTMLOListElement) {
  draggedId = null;
  dragOverId = null;
  list.querySelectorAll(".dragging").forEach((el) => el.classList.remove("dragging"));
  list.querySelectorAll(".drag-over-top, .drag-over-bottom").forEach((el) => {
    el.classList.remove("drag-over-top", "drag-over-bottom");
  });
}

// ── ↑↓ 按钮（保留做快速单步 + accessibility）─────────────────

export async function moveProviderInOrder(id: string, dir: "up" | "down") {
  const idx = currentProviderOrder.indexOf(id);
  if (idx < 0) return;
  const newIdx = dir === "up" ? idx - 1 : idx + 1;
  if (newIdx < 0 || newIdx >= currentProviderOrder.length) return;

  // DOM 交换（即时响应，不等 IPC）
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

  // 内存里调换
  [currentProviderOrder[idx], currentProviderOrder[newIdx]] = [
    currentProviderOrder[newIdx],
    currentProviderOrder[idx],
  ];

  // 更新位置编号 + disabled（DOM 已经 swap 了，只需更新文本）
  refreshPosLabels();
  void commitOrder(newIdx, id);
}

/// 调后端 set_provider_order（即时落盘 + 重排 snapshot + emit）。
/// 失败回滚内存里的顺序 + flash 错误。
async function commitOrder(finalIdx: number, id: string) {
  try {
    await setProviderOrder(currentProviderOrder);
    flash(`✓ ${id} 已移到位置 ${finalIdx + 1}`);
  } catch (e) {
    flash(`✗ 调整顺序失败: ${e}`, true);
    // TODO: 回滚（需要记住之前的 order 快照，v1 暂不处理）
  }
}

/// 刷新所有 "X/N" 标签 + up/down disabled 状态。
/// DOM 不重建，只改文本内容和按钮状态（不抖动）。
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

// ── 全局按钮委托 + 拖拽事件 ──────────────────────────────────

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

/// 兼容老 caller（settings.html legacy 里的 per-panel 顺序按钮）
export function renderProviderOrderPanels(order: string[]) {
  setCurrentProviderOrder(canonicalizeOrder(order));
  refreshPosLabels();
}
