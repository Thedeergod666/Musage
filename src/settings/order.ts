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
  currentProviderOrder,
  el,
  flash,
  getCurrentKnownIds,
  setCurrentKnownIds,
  setCurrentProviderOrder,
} from "./utils";
import { getProviderMeta } from "./logos";
import { t } from "../i18n";
import type { AppConfig, ProviderId, SourceMeta } from "./types";

/// 把 cfg.provider_order 规整成「已知 + 按 builtin 注册表补齐」的有序列表。
///
/// 流程：
/// 1. 保留 cfg.provider_order 里**已知**的 id（去重），按其顺序
/// 2. 把当前已知 list（来自 [setCurrentKnownIds]）里**没出现**的补到末尾
///
/// "已知" 集合是动态的（PR 2 起派生自 `SourceMeta[]`，PR 3 会包含
/// `custom_*` 中转站），所以新加 provider 不用改这个函数。
///
/// 防御性：known 列表为空时（极少数情况，比如 `renderProviderOrderPanels`
/// 在 `renderOrderSection` 之前被调用），fallback 到把 input dedup 后
/// 原样返回，避免空数组炸 UI。
export function canonicalizeOrder(order: string[]): ProviderId[] {
  const known = getCurrentKnownIds();
  const knownSet = new Set(known);
  if (knownSet.size === 0) {
    const seen = new Set<string>();
    const dedup: string[] = [];
    for (const id of order) {
      if (!seen.has(id)) { seen.add(id); dedup.push(id); }
    }
    return dedup;
  }
  const seen = new Set<string>();
  const ordered: ProviderId[] = [];
  for (const id of order) {
    if (knownSet.has(id) && !seen.has(id)) {
      seen.add(id);
      ordered.push(id);
    }
  }
  for (const id of known) {
    if (!seen.has(id)) {
      seen.add(id);
      ordered.push(id);
    }
  }
  return ordered;
}

// ── 模块状态 ────────────────────────────────────────────────

let orderSources: SourceMeta[] = [];
let orderCfg: AppConfig | null = null;
let listRef: HTMLOListElement | null = null;

/// 批量 async 操作期间，main.ts 的 config-changed 监听器要跳过 rebuild，
/// 否则每次 setProviderEnabled 都会触发 getConfig + buildOrderItems，把
/// 我们乐观更新的 orderCfg 覆盖回部分后端状态，导致 UI 在「全隐藏」和
/// 「新位置」之间闪烁。详见 onDividerMouseUp 注释。
///
/// **reentrance-safe（2026-06-19 audit fix）**：用 `suppressDepth` 计数器
/// 而不是单纯 boolean —— 连续点 3+ 个 checkbox 时，第一次 withSuppress 的
/// finally 不能在第二/第三次仍在等的时候关掉 suppress 标志，否则后续
/// config-changed 事件会 rebuild 把第二/三次的乐观更新覆盖回第一次的状态。
let suppressDepth = 0;

/** 手动 bump 抑制深度（批量操作 in-flight 期间用）。配对 `unsuppressRebuild()`。 */
export function suppressRebuild(): void {
  suppressDepth++;
}
export function unsuppressRebuild(): void {
  suppressDepth = Math.max(0, suppressDepth - 1);
}

export function isSuppressingConfigRebuild(): boolean {
  return suppressDepth > 0;
}

/** **L14 fix（2026-06-19）**：单次 enabled checkbox 点击同样需要抑制
 *  config-changed 监听器。原先只有批量操作（onDividerMouseUp / 拖拽 mousedown→up）
 *  会置 suppressConfigRebuild=true；单点 checkbox 不会，所以连续点多个
 *  checkbox 时第二次的 config-changed 会用后端旧 cfg 覆盖乐观更新，
 *  浮窗在「全隐藏」与「新位置」之间闪烁。
 *
 *  用法：把 setProviderEnabled 调包成 withSuppress(...)
 *    await withSuppress(() => setProviderEnabled(id, true));
 *
 *  实现：inc suppressDepth → await 业务 fn → 短暂延时等 config-changed 事件落地
 *  → 强制 resync 一次（防止乐观更新与后端状态有微小偏差）→ dec suppressDepth
 *  （仅当归零时清标志，避免 reentrance 误关）。
 *  短暂延时是关键 —— config-changed 是后端 cfg.save() 后 emit 的，需要等事件
 *  到达 main.ts listener 才会被 suppress 挡掉。 */
export async function withSuppress<T>(fn: () => Promise<T>): Promise<T> {
  suppressDepth++;
  try {
    return await fn();
  } finally {
    // 给 config-changed 一个落地窗口（~30ms 足够 emit/listener 一来一回）
    await new Promise((r) => setTimeout(r, 30));
    suppressDepth--;
    if (suppressDepth === 0) {
      // 强制 resync：以防乐观更新与后端状态有微小偏差
      try {
        const { getConfig } = await import("./api");
        const cfg = await getConfig();
        orderCfg = cfg;
        if (listRef) buildOrderItems(listRef);
      } catch (e) {
        console.warn("[settings] withSuppress resync 失败", e);
      }
    }
    // 仍有更外层的 withSuppress 在等 → 不动标志,等最外层退出时 resync
  }
}

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
/// mousedown 时的 clientY —— mouseup 时算 |clientY - dragStartY|，
/// 小于阈值视为「点击未拖动」，避免抖动误触 reorder / 跨段。
/// 详见 onDragMouseUp 内的 DRAG_THRESHOLD_PX 注释。
let dragStartY = 0;

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
  dragStartY = e.clientY;

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

  // 找到光标所在的 <li>。注意：这里**不**用 `li.order-row` 过滤 —— 必须
  // 包含 divider，否则当用户在 divider 下方 (隐藏段内) 拖动时，
  // `insertIdx` 索引到的 `visibleItems[i]` 跨过 divider 位置一格，
  // 出现"drop 在第 N 行上方却落到第 N-1 行"的位置错位。
  //
  // 之前版本 (fix-drag-index-2026-06-18 之前) 用两套数组：
  //   - `items`  = order-row 集合 (不含 divider) → 算 insertIdx
  //   - `visibleItems` = children 过滤 (含 divider) → insertBefore
  // 两边索引不对应，divider 之后的所有 drop 位置都比预期高 1 格。
  // 现在统一用一份数组（含 divider）做 midY 检测 + insertBefore。
  if (dragPlaceholder && listRef) {
    const children = [...listRef.children] as HTMLElement[];
    const visibleItems = children.filter(
      (c) => c !== dragPlaceholder && !(c as HTMLElement).style?.display?.includes("none"),
    );
    const rects = visibleItems.map((el) => {
      const r = el.getBoundingClientRect();
      return { top: r.top, height: r.height };
    });
    const insertIdx = computeInsertIndex(rects, e.clientY);
    if (insertIdx < visibleItems.length) {
      listRef.insertBefore(dragPlaceholder, visibleItems[insertIdx]);
    } else {
      listRef.appendChild(dragPlaceholder);
    }
  }
}

function onDragMouseUp(e: MouseEvent) {
  if (!dragging) return;
  document.removeEventListener("mousemove", onDragMouseMove);
  document.removeEventListener("mouseup", onDragMouseUp);

  // 恢复源 li
  const srcLi = listRef?.querySelector(`li[data-id="${dragSrcId}"]`) as HTMLElement | null;
  if (srcLi) srcLi.style.display = "";

  // ── 距离阈值：< 5px 视为「点击未拖动」，跳过整个 drag 流程 ──
  // 修复「点 enabled 段最后一条 + 轻微拖动」误触跨段
  // (fix-click-vs-drag-2026-06-18)。
  // OpenRouter 处于 position 6/13（enabled 段最末），divider 紧跟其后，
  // 鼠标只要滑过 OpenRouter 中线 1-2px 就会命中 divider 的 midY →
  // crossedDivider=true → OpenRouter 被乐观 disable。重复点击 = 反复
  // enabled ↔ disabled toggle → 每次都 rebuild + IPC + 浮窗 snapshot +
  // listener 跑 updateOrderConfig → 整列表频繁刷新 + 位置标签错位 + 浮窗
  // 卡片跳变（用户感受到「Step/Claude 串上来」）。
  //
  // 5px > 人手 click 抖动（实测 1-3px）且 < 真拖最小距离（5+px）。
  // 阈值局部常量便于以后调。
  const DRAG_THRESHOLD_PX = 5;
  if (Math.abs(e.clientY - dragStartY) < DRAG_THRESHOLD_PX) {
    // 清理 placeholder/ghost（跟正常 mouseup 一样）
    dragPlaceholder?.remove();
    dragGhost?.remove();
    dragPlaceholder = null;
    dragGhost = null;
    dragging = false;
    dragSrcId = null;
    dragSrcIdx = -1;
    return;
  }

  // 计算新位置：placeholder 在 list 里的 index
  let newIdx = 0;
  let placeholderBeforeDivider = true;
  if (dragPlaceholder && listRef) {
    const children = [...listRef.children];
    newIdx = children.indexOf(dragPlaceholder);
    if (newIdx < 0) newIdx = currentProviderOrder.length;
    // 检测 placeholder 是否落在 divider 之后 → 视为"未勾选"区
    const dividerIdx = children.findIndex((c) => c.classList.contains("order-divider"));
    placeholderBeforeDivider = isPlaceholderBeforeDivider(newIdx, dividerIdx);
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
    // 没移动也没跨分隔线 → 纯 no-op。**2026-06-20 audit**：之前还调
    // buildOrderItems 全量 rebuild DOM，触发跟"实际拖拽完成"一样的视觉闪烁
    // 但其实啥都没变。改为早返，避免不必要的 DOM 操作。
    dragging = false;
    dragSrcId = null;
    dragSrcIdx = -1;
    return;
  }

  // 执行移位（在 currentProviderOrder 里 splice）
  const moved = currentProviderOrder.splice(dragSrcIdx, 1)[0];
  // splice(adjusted, 0, moved) 直接把 moved 放到 adjusted，**不需要 -1**：
  //   targetIdx > srcIdx（往下拖）时，splice 移除 src 后 array 短了 1，
  //   但 splice 的第二个参数是「插入位置」而不是「目标位置」，所以
  //   adjusted 直接 = orderIdx 即可，移到 targetIdx。
  //   之前 `targetIdx - 1` 是错的：arr=[A,B,C,D,E] 从 idx=0 拖到 idx=4
  //   应该变 [B,C,D,E,A]（A 在 4），旧公式给 [B,C,D,A,E]（A 在 3）。
  const adjusted = orderIdx;
  currentProviderOrder.splice(adjusted, 0, moved);

  if (crossedDivider) {
    // ── 乐观更新：先翻 enabled flag + sync checkbox，再 rebuild DOM，再 IPC ──
    // 顺序必须是 orderCfg → buildOrderItems，否则 buildOrderItems 读旧
    // enabled flag 把卡片画回原分区（bug #2）。
    // **2026-06-20 audit**：之前有 `const wasEnabled = !willBeEnabled;` 内层 shadow，
    // 设了但从未读；catch 用的是外层 wasEnabled（正确）。删 inner shadow 消除
    // 维护期混淆（未来改代码容易踩雷）。
    if (orderCfg && dragSrcId) {
      if (!orderCfg.providers) orderCfg.providers = {};
      const entry = orderCfg.providers[dragSrcId] ?? { enabled: willBeEnabled };
      orderCfg.providers[dragSrcId] = { ...entry, enabled: willBeEnabled };
    }
    if (dragSrcId) {
      const cb = document.getElementById(`enabled-${dragSrcId}`) as HTMLInputElement | null;
      if (cb) cb.checked = willBeEnabled;
    }
    // orderCfg 已更新，现在 rebuild 才会把卡片画到正确分区
    if (listRef) buildOrderItems(listRef);
    refreshPosLabels();

    // suppressRebuild：防止 main.ts 的 config-changed 监听器用后端旧
    // config 覆盖我们的乐观更新（bug #3 的卡片拖拽侧）。
    suppressRebuild();
    void (async () => {
      try {
        await setProviderEnabled(dragSrcId!, willBeEnabled);
        await setProviderOrder(currentProviderOrder);
        flash(
          willBeEnabled
            ? t("settings.order.flash_moved_to_floating", { id: dragSrcId! })
            : t("settings.order.flash_hidden", { id: dragSrcId! }),
        );
      } catch (e) {
        // IPC 失败 → 回滚 orderCfg + DOM（bug #4）
        if (orderCfg?.providers?.[dragSrcId!]) {
          orderCfg.providers[dragSrcId!] = {
            ...orderCfg.providers[dragSrcId!],
            enabled: wasEnabled,
          };
        }
        if (dragSrcId) {
          const cb = document.getElementById(`enabled-${dragSrcId}`) as HTMLInputElement | null;
          if (cb) cb.checked = wasEnabled;
        }
        if (listRef) buildOrderItems(listRef);
        refreshPosLabels();
        flash(t("settings.order.flash_move_failed", { err: String(e) }), true);
      } finally {
        unsuppressRebuild();
        // 最终 resync：确保与后端一致
        const cfg = await import("./api").then((m) => m.getConfig());
        orderCfg = cfg;
        if (listRef) buildOrderItems(listRef);
      }
    })();
  } else {
    // ── Surgical DOM move：单步 insertBefore，同步刷新 position label ──
    // 之前完全靠 config-changed 监听器异步 rebuild，体感「拖完没动，要单击
    // 分隔线才刷新」；buildOrderItems 全量重建又会闪一帧（违反
    // [musage-ui-design] memory「不闪烁」原则）。改成单步 insertBefore 把
    // src row 移到目标位置，其他 row 完全不动 → 零闪烁 + 立即可见。
    // index 映射复用 moveProviderInOrder (↑↓ 按钮) 的规则，提取为
    // computeSameSectionMove 纯函数并配套 11 个单元测试覆盖所有边界。
    // (fix-drag-samesection-surgery-2026-06-18)
    if (listRef) {
      const { srcDomIdx, refDomIdx, shouldMove } = computeSameSectionMove({
        dragSrcIdx,
        orderIdx: adjusted,
        boundary: boundaryIdx(),
      });
      if (shouldMove && refDomIdx !== null) {
        const srcEl = listRef.children[srcDomIdx] as HTMLElement | undefined;
        const ref = (listRef.children[refDomIdx] as
          | HTMLElement
          | undefined) ?? null;
        // ref === srcEl 表示 src 已在目标位置（理论上 shouldMove=false
        // 已经过滤了，但兜底一下避免无谓 reflow）
        if (srcEl && ref !== srcEl) {
          listRef.insertBefore(srcEl, ref);
        }
      }
    }
    refreshPosLabels();
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

  // ── 批量 IPC：先禁止 main.ts 的 config-changed 监听器 rebuild（否则每次
  // setProviderEnabled 都会触发 getConfig 覆盖我们的 orderCfg，导致 UI 闪烁
  // 在「全隐藏」与「新位置」之间穿梭）。所有 IPC 跑完再 force resync 一次。──
  suppressRebuild();
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
          ? t("settings.order.flash_cards_added", { delta })
          // P0 fix: 之前用 "{-delta}" 配 t() 的替换正则 /\{(\w+)\}/g —— \w 不含连字符，
          // 所以占位符永远不被替换。改成 {count}（positive number，翻译里就显式说
          // "隐藏 N 张" 而非 "隐藏 {-N} 张"），同时传 Math.abs(delta) 直接用正数。
          : t("settings.order.flash_cards_removed", { count: Math.abs(delta) }),
      );
    } catch (e) {
      flash(t("settings.order.flash_move_failed", { err: String(e) }), true);
    } finally {
      unsuppressRebuild();
      // 最终 resync：以防乐观更新与后端状态有微小偏差（不可能，但兜底）
      const cfg = await import("./api").then((m) => m.getConfig());
      orderCfg = cfg;
      if (listRef) buildOrderItems(listRef);
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
  // 先把 known list 同步到 utils（必须早于 canonicalizeOrder，因为后者读它）
  setCurrentKnownIds(sources.map((s) => s.id));
  setCurrentProviderOrder(canonicalizeOrder(cfgProviderOrder ?? []));
  orderSources = sources;
  orderCfg = cfg;

  const list = el("ol", { class: "order-list" }) as HTMLOListElement;
  listRef = list;
  buildOrderItems(list);

  // 绑定 mousedown（统一路由：分隔线 → onDividerMouseDown，卡片 → onDragMouseDown）
  list.addEventListener("mousedown", (e) => {
    const target = (e.target as HTMLElement | null);
    if (!target) return;
    if (target.closest("li.order-divider") && !target.closest("li.order-row")) {
      onDividerMouseDown(e);
    } else if (target.closest("li.order-row") && !target.closest("button")) {
      onDragMouseDown(e);
    }
  });

  const section = el(
    "section",
    { class: "order-section section-card" },
    el("h2", {}, t("settings.order.section_title")),
    el("p", { class: "order-hint" }, t("settings.order.panels_follow_hint")),
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
  // 兜底：任何已知 source（builtin + custom）但不在 currentProviderOrder 里的 id，
  // 按 enabled 状态加进对应段（首次启动时 order 为空、但每个 provider 都有 enabled）。
  // PR 3 加 CustomSource 后这个循环自动接住（known list 在 renderOrderSection 时更新）。
  for (const id of getCurrentKnownIds()) {
    if (currentProviderOrder.includes(id)) continue;
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
      title: t("settings.order.divider_title"),
    },
    el("span", { class: "order-divider-grip", "aria-hidden": "true" }, t("settings.order.divider_grip")),
    el("span", { class: "order-divider-line" }),
    el("span", { class: "order-divider-label" }, t("settings.order.divider_label")),
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
      el("button", { class: "order-up", "data-id": id, type: "button", title: t("settings.order.move_up_title") }, "↑"),
      el("button", { class: "order-down", "data-id": id, type: "button", title: t("settings.order.move_down_title") }, "↓"),
    ),
  );
  refreshRowButtons(li, idx, total);
  return li;
}

/** 统一规则：
 *  - up 仅在「显示段第一张」时禁用（enabled 且 idx === 0）
 *  - down 仅在「隐藏段最后一张」时禁用（disabled 且 idx === total - 1）
 *  - 其余全部可点 —— 包括「显示段最后一张的 ↓」（跨段进隐藏段首位） */
function refreshRowButtons(li: HTMLElement, idx: number, total: number) {
  const upBtn = li.querySelector<HTMLButtonElement>(".order-up");
  const downBtn = li.querySelector<HTMLButtonElement>(".order-down");
  if (upBtn) {
    // 全隐藏时 idx=0 的卡也是隐藏段首张，↑ 应该可点（跨段进显示段）
    const isFirstEnabled =
      !li.classList.contains("order-row-disabled") && idx === 0;
    upBtn.disabled = isFirstEnabled;
  }
  if (downBtn) {
    const isLastDisabled =
      li.classList.contains("order-row-disabled") && idx === total - 1;
    downBtn.disabled = isLastDisabled;
  }
}

// L3 fix: 之前 posLabel 用 currentProviderOrder.length 作分母，
// 但 buildOrderItems 会从 getCurrentKnownIds() 追加"不在 currentProviderOrder 里"的 row
// （PR 3 加 CustomSource 之后常见）—— 末位 row 会显示 "8/7"。
// 改为按 buildOrderItems 的真实分母（enabledIds + disabledIds）来算。
function currentTotalRows(): number {
  const known = getCurrentKnownIds();
  const orderSet = new Set(currentProviderOrder);
  let extra = 0;
  for (const id of known) {
    if (!orderSet.has(id)) extra++;
  }
  return currentProviderOrder.length + extra;
}

function posLabel(i: number): string {
  return `${i + 1}/${currentTotalRows()}`;
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
    // 只有「显示段首张」的 ↑ 被禁用；全隐藏时 idx=0 的卡也是隐藏段首张，
    // 其 ↑ 是「跨段 ↑ → 进显示段」的合法入口 —— 必须放行。
    if (idx === 0 && wasEnabled) return;
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

    // suppressRebuild：防止 main.ts 的 config-changed 监听器用后端旧
    // config 覆盖我们的乐观更新（bug #3）。
    suppressRebuild();
    void (async () => {
      try {
        await setProviderEnabled(id, newEnabled);
        flash(
          wasEnabled
            ? t("settings.order.flash_hidden", { id })
            : t("settings.order.flash_moved_to_floating", { id }),
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
        flash(t("settings.order.flash_move_failed", { err: String(e) }), true);
      } finally {
        unsuppressRebuild();
        // 最终 resync：确保与后端一致
        const cfg = await import("./api").then((m) => m.getConfig());
        orderCfg = cfg;
        if (listRef) buildOrderItems(listRef);
      }
    })();
    return;
  }

  // 同段移动：在 currentProviderOrder 里 splice + 轻量 DOM 交换
  const targetIdx = dir === "up" ? idx - 1 : idx + 1;
  const moved = currentProviderOrder.splice(idx, 1)[0];
  // splice(idx, 1) 后 array 短了 1，但 splice(adjusted, 0, x) 把 x 放到 adjusted。
  // 不需要因为 idx 移除了就 -1 —— x 直接落到 targetIdx 即可。
  const adjusted = targetIdx;
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
    flash(t("settings.order.flash_moved_to_pos", { id, pos: finalIdx + 1 }));
  } catch (e) {
    flash(t("settings.order.flash_order_failed", { err: String(e) }), true);
  }
}

// L4 fix: 之前只遍历 currentProviderOrder，跳过 buildOrderItems 追加的 extra row。
// 现在按 buildOrderItems 的真实分两段路径走（enabledIds + disabledIds），
// 这样新增的 CustomSource 也能正确刷新 pos / up / down 状态。
function refreshPosLabels() {
  // 与 buildOrderItems 保持完全相同的分段 + 追加逻辑
  const enabledIds: string[] = [];
  const disabledIds: string[] = [];
  for (const id of currentProviderOrder) {
    if (isEnabledId(id)) enabledIds.push(id);
    else disabledIds.push(id);
  }
  const orderSet = new Set(currentProviderOrder);
  for (const id of getCurrentKnownIds()) {
    if (orderSet.has(id)) continue;
    if (isEnabledId(id)) enabledIds.push(id);
    else disabledIds.push(id);
  }
  const displayed = [...enabledIds, ...disabledIds];
  const total = displayed.length;
  for (let i = 0; i < displayed.length; i++) {
    const id = displayed[i];
    const posEl = document.querySelector<HTMLElement>(`.order-pos[data-id="${id}"]`);
    if (posEl) posEl.textContent = posLabel(i);
    const upBtn = document.querySelector<HTMLButtonElement>(`.order-up[data-id="${id}"]`);
    const downBtn = document.querySelector<HTMLButtonElement>(`.order-down[data-id="${id}"]`);
    if (upBtn) {
      const row = upBtn?.closest("li.order-row");
      const isFirstEnabled =
        !!row && !row.classList.contains("order-row-disabled") && i === 0;
      upBtn.disabled = isFirstEnabled;
    }
    if (downBtn) {
      const row = downBtn?.closest("li.order-row");
      const isLastDisabled =
        !!row?.classList.contains("order-row-disabled") &&
        i === total - 1;
      downBtn.disabled = isLastDisabled;
    }
  }
}

// ── 全局按钮委托 ────────────────────────────────────────────

// M23 fix: 之前 document.addEventListener 无幂等保护，重复调会累积 N 个 listener。
// 改成 module-scope flag，第一次调才绑，后续 return。
// C2 fix: 用 closest('.order-up, .order-down') 而不是直接 e.target.classList，
// 否则点 button 内部的 text node（"↑"/"↓"）会抛 "Cannot read properties of undefined (reading 'contains')"。
let _orderListenerBound = false;
export function bindOrderButtonsGlobal() {
  if (_orderListenerBound) return;
  _orderListenerBound = true;
  document.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement | null)?.closest<HTMLElement>(".order-up, .order-down");
    if (!btn) return;
    const id = btn.dataset.id;
    if (!id) return;
    if (btn.classList.contains("order-up")) {
      void moveProviderInOrder(id, "up");
    } else if (btn.classList.contains("order-down")) {
      void moveProviderInOrder(id, "down");
    }
  });
}

export function renderProviderOrderPanels(order: string[]) {
  setCurrentProviderOrder(canonicalizeOrder(order));
  refreshPosLabels();
}

// ── 纯函数（可被单元测试覆盖）─────────────────────────────────

/**
 * 给定一组"可见 DOM 元素"的 bounding rect（按 DOM 顺序，**包含 divider**）
 * 和光标 Y 坐标，返回 placeholder 应插入的位置索引。
 *
 * hit-test 规则：光标 Y 落在哪个元素的 midY 之上 → 插到该元素之前；
 * 都不命中 → 插到末尾。
 *
 * 提取为纯函数让 `onDragMouseMove` 单元测试可写 —— 之前版本 bug 的根因
 * 正是这套索引和 `visibleItems` 的索引错位（不含 divider 的 items 算
 * insertIdx，用含 divider 的 visibleItems 做 insertBefore）。
 */
export function computeInsertIndex(
  rects: ReadonlyArray<{ readonly top: number; readonly height: number }>,
  clientY: number,
): number {
  for (let i = 0; i < rects.length; i++) {
    const midY = rects[i].top + rects[i].height / 2;
    if (clientY < midY) return i;
  }
  return rects.length; // append to end
}

/**
 * 给定 placeholder 和 divider 在 `listRef.children` 里的索引，判断
 * placeholder 是否落在 divider 之前（视为"显示段"）还是之后（"隐藏段"）。
 *
 * 边界情况：dividerIdx === -1（无 divider 场景，例如 disabledIds 为空
 * 时 buildOrderItems 仍渲染一条空 divider，但 #951 后改成始终渲染）→ 返回
 * true（视作整体 enabled，无 hidden 段）。
 */
export function isPlaceholderBeforeDivider(
  placeholderIdx: number,
  dividerIdx: number,
): boolean {
  return dividerIdx < 0 || placeholderIdx < dividerIdx;
}

/**
 * 同段拖拽的 DOM surgical move index 映射。
 *
 * 复用 moveProviderInOrder (↑↓ 按钮) 的规则：
 *   - divider 占一格，disabled 段 DOM 索引比 currentProviderOrder 索引大 1
 *   - srcDomIdx = dragSrcIdx (enabled) | dragSrcIdx + 1 (disabled)
 *   - dstDomIdxLogical = orderIdx (enabled) | orderIdx + 1 (disabled)
 *   - src 还在 DOM children 里占 srcDomIdx；splice 后实际目标 ref：
 *       - srcDomIdx < dstDomIdxLogical：ref = children[dstDomIdxLogical - 1]
 *         （src 在 dst 之前，splice 后 src 移到 dst 位置会"挤掉"一个 ref）
 *       - 否则：ref = children[dstDomIdxLogical]
 *
 * 返回 srcDomIdx / refDomIdx / shouldMove。caller 拿 srcEl / ref 后
 * 调 listRef.insertBefore(srcEl, ref) 完成 DOM 移动。
 * srcDomIdx === refDomIdx 时 shouldMove=false（已在目标位置，no-op）。
 *
 * 提取为纯函数方便单元测试覆盖所有边界 case（不依赖 DOM mock 形式）。
 *
 * (fix-drag-samesection-surgery-2026-06-18)
 */
export function computeSameSectionMove(args: {
  dragSrcIdx: number;
  orderIdx: number;
  boundary: number;
}): { srcDomIdx: number; refDomIdx: number | null; shouldMove: boolean } {
  const { dragSrcIdx, orderIdx, boundary } = args;
  // 同位置 no-op (splice 自身也是 no-op, DOM 不该动)
  if (dragSrcIdx === orderIdx) {
    const srcDomIdx = dragSrcIdx >= boundary ? dragSrcIdx + 1 : dragSrcIdx;
    return { srcDomIdx, refDomIdx: null, shouldMove: false };
  }
  // 跨段 (src enabled 段, target disabled 段边界): surgical move 跨不过 divider,
  // caller 应走 crossedDivider 分支 (buildOrderItems partition 切换) 而不是这里。
  if (orderIdx >= boundary && dragSrcIdx < boundary) {
    return { srcDomIdx: dragSrcIdx, refDomIdx: null, shouldMove: false };
  }
  const srcDomIdx = dragSrcIdx >= boundary ? dragSrcIdx + 1 : dragSrcIdx;
  let refIdx: number;
  if (orderIdx >= boundary) {
    // src + target 都在 disabled 段: refIdx 恒为 orderIdx + 2
    // (mousedown-after DOM 中, target 后一位 logical = orderIdx + 1, DOM 映射
    //  = (orderIdx + 1) + 1 = orderIdx + 2). 拖向上 / 拖向下对称.
    refIdx = orderIdx + 2;
  } else {
    // target 在 enabled 段:
    //   拖向下 (orderIdx > dragSrcIdx): ref = target 后一位 logical = orderIdx + 1
    //   拖向上 (orderIdx < dragSrcIdx): ref = orderIdx (target 后一位 = src 之前一位)
    refIdx = orderIdx > dragSrcIdx ? orderIdx + 1 : orderIdx;
  }
  return { srcDomIdx, refDomIdx: refIdx, shouldMove: true };
}
