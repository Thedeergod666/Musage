// Provider 浮窗卡片顺序管理
//
// 把原 settings.ts 里的 canonicalizeOrder / renderProviderOrderPanels /
// moveProviderInOrder 集中到这里。

import { setProviderOrder } from "./api";
import { BUILTIN_ORDER, currentProviderOrder, flash, setCurrentProviderOrder } from "./utils";
import type { ProviderId } from "./types";

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

/// 把当前顺序渲染到每个 panel 顶部的 "位置 X/N" + ↑↓ 按钮边界
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
    renderProviderOrderPanels(currentProviderOrder);
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
