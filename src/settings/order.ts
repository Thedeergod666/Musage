// Provider 浮窗卡片顺序管理
//
// 把原 settings.ts 里的 canonicalizeOrder / renderProviderOrderPanels /
// moveProviderInOrder 集中到这里。
//
// v0.6+ 顺序列表从「每个 panel 内部 ↑↓ 按钮」挪到「providers section
// 顶部独立区块」（renderOrderSection），5 个 provider 共用一个 list。
// 这样用户 1 秒看清当前顺序，不用上下翻 5 个 panel。

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

/// 顶部的「浮窗卡片顺序」区块：列出当前所有 source + ↑↓ 按钮。
/// 插入到 providers section 的最顶端。
export function renderOrderSection(
  container: HTMLElement,
  sources: SourceMeta[],
  cfgProviderOrder: string[] | undefined,
) {
  // 用现有 cfg 顺序初始化 currentProviderOrder
  setCurrentProviderOrder(canonicalizeOrder(cfgProviderOrder ?? []));

  const list = el("ol", { class: "order-list" });
  currentProviderOrder.forEach((id, i) => {
    const meta = sources.find((s) => s.id === id);
    if (!meta) return; // 已删除 / 不存在的 source 跳过
    const providerMeta = getProviderMeta(id);
    const logo = providerMeta
      ? el("img", { class: "order-logo", src: providerMeta.logo, alt: providerMeta.name })
      : null;
    const li = el(
      "li",
      { class: "order-row" },
      el(
        "div",
        { class: "order-row-left" },
        ...(logo ? [logo] : []),
        el("span", { class: "order-pos", "data-id": id }, `位置 ${i + 1} / ${currentProviderOrder.length}`),
        el("span", { class: "order-name" }, meta.display_name),
      ),
      el(
        "div",
        { class: "order-btns" },
        el("button", { class: "order-up", "data-id": id, type: "button" }, "↑"),
        el("button", { class: "order-down", "data-id": id, type: "button" }, "↓"),
      ),
    );
    list.appendChild(li);
  });

  // 标题 + 列表塞进 section 容器
  const section = el(
    "section",
    { class: "order-section section-card" },
    el("h2", {}, "浮窗卡片顺序"),
    list,
  );
  // 清掉旧 order-section（如果有）再插新的（防止多次渲染堆积）
  const old = container.querySelector(".order-section");
  if (old) old.remove();
  container.prepend(section);
}

/// ↑↓ 按钮回调：调换顺序 → 调后端 set_provider_order（即时落盘 + emit）
/// 失败回滚内存里的顺序，flash 错误。
export async function moveProviderInOrder(id: string, dir: "up" | "down") {
  const idx = currentProviderOrder.indexOf(id);
  if (idx < 0) return;
  const newIdx = dir === "up" ? idx - 1 : idx + 1;
  if (newIdx < 0 || newIdx >= currentProviderOrder.length) return;
  // 内存里调换
  [currentProviderOrder[idx], currentProviderOrder[newIdx]] = [
    currentProviderOrder[newIdx],
    currentProviderOrder[idx],
  ];
  try {
    await setProviderOrder(currentProviderOrder);
    // 改完更新 DOM（不要重建整个 list —— 用户可能正在 focus 某个按钮）
    updateOrderDisplay();
    flash(`✓ ${id} 已移到位置 ${newIdx + 1}`);
  } catch (e) {
    // 回滚
    [currentProviderOrder[idx], currentProviderOrder[newIdx]] = [
      currentProviderOrder[newIdx],
      currentProviderOrder[idx],
    ];
    flash(`✗ 调整顺序失败: ${e}`, true);
  }
}

/// 顺序变更后只更新"位置 X/N" + ↑↓ disabled 边界，不重建 DOM。
function updateOrderDisplay() {
  for (let i = 0; i < currentProviderOrder.length; i++) {
    const id = currentProviderOrder[i];
    const posEl = document.querySelector<HTMLElement>(`[data-id="${id}"].order-pos`);
    if (posEl) posEl.textContent = `位置 ${i + 1}/${currentProviderOrder.length}`;
    const upBtn = document.querySelector<HTMLButtonElement>(`button.order-up[data-id="${id}"]`);
    const downBtn = document.querySelector<HTMLButtonElement>(`button.order-down[data-id="${id}"]`);
    if (upBtn) upBtn.disabled = i === 0;
    if (downBtn) downBtn.disabled = i === currentProviderOrder.length - 1;
  }
}

/// 兼容旧 caller（main.ts 的 .order-up / .order-down 按钮 click 事件）
/// —— 委托在 document 上，按 data-id 路由。
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

/// 把当前顺序渲染到每个 panel 顶部的 "位置 X/N" + ↑↓ 按钮边界
/// （Stage 4+ 顺序按钮挪到顶部独立区块，但保留函数签名以防老 caller）
export function renderProviderOrderPanels(order: string[]) {
  setCurrentProviderOrder(canonicalizeOrder(order));
  for (const id of BUILTIN_ORDER) {
    const pos = currentProviderOrder.indexOf(id);
    const posEl = document.getElementById(`order-pos-${id}`);
    if (posEl) posEl.textContent = `位置 ${pos + 1}/${currentProviderOrder.length}`;
    const upBtn = document.querySelector<HTMLButtonElement>(
      `.order-up[data-id="${id}"]`,
    );
    const downBtn = document.querySelector<HTMLButtonElement>(
      `.order-down[data-id="${id}"]`,
    );
    if (upBtn) upBtn.disabled = pos === 0;
    if (downBtn) downBtn.disabled = pos === currentProviderOrder.length - 1;
  }
}
