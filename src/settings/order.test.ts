// 单元测试：浮窗卡片拖拽的纯 index 逻辑
//
// 覆盖的 bug：fix-drag-index-2026-06-18 —— 之前 `onDragMouseMove` 用
// `li.order-row` 集合（不含 divider）算 insertIdx，再去索引含 divider
// 的 visibleItems 做 insertBefore，导致 divider 之后的 drop 位置比
// 预期高 1 格（隐藏段内部、跨段、可见段末尾跨段都受影响）。
//
// 修复方案：统一索引 —— 一份 visibleItems（含 divider）既做 midY 检测
// 也做 insertBefore。`computeInsertIndex` / `isPlaceholderBeforeDivider`
// 提取为纯函数覆盖核心逻辑。

import { describe, expect, it } from "vitest";
import { computeInsertIndex, isPlaceholderBeforeDivider } from "./order";

// 模拟 buildOrderItems 的 DOM 布局：
//   visible [A, B, C]，hidden [D, E, F]
//   DOM 顺序：A(0,0,50) B(0,50,50) C(0,100,50) divider(0,150,10) D(0,160,50) E(0,210,50) F(0,260,50)
// 所有元素 height=50，divider height=10。
const RECT_HEIGHT = 50;
const DIVIDER_HEIGHT = 10;
function rowRect(top: number) {
  return { top, height: RECT_HEIGHT };
}
function dividerRect(top: number) {
  return { top, height: DIVIDER_HEIGHT };
}

describe("computeInsertIndex", () => {
  it("光标在第一个元素上半部 → 0", () => {
    const rects = [rowRect(0)];
    expect(computeInsertIndex(rects, 20)).toBe(0);
  });

  it("光标在第一个元素 midY 之上（含等于）→ 0", () => {
    // midY=25；光标=25 时 `clientY < midY` 为 false → 落到 1
    const rects = [rowRect(0)];
    expect(computeInsertIndex(rects, 25)).toBe(1);
  });

  it("光标在第一个元素下半部 → 1（插到末尾）", () => {
    const rects = [rowRect(0)];
    expect(computeInsertIndex(rects, 30)).toBe(1);
  });

  it("光标低于所有元素 → length（append）", () => {
    const rects = [rowRect(0)];
    expect(computeInsertIndex(rects, 200)).toBe(1);
  });

  it("光标在第二个元素上半部 → 1", () => {
    // rows at top=0 (midY=25), top=50 (midY=75); clientY=60 → 落在 1
    const rects = [rowRect(0), rowRect(50)];
    expect(computeInsertIndex(rects, 60)).toBe(1);
  });

  it("光标在第二个元素 midY → 2（落到第二个之后）", () => {
    // midY of item 1 = 75；clientY=75 时 `clientY < 75` false → 落到 2
    const rects = [rowRect(0), rowRect(50)];
    expect(computeInsertIndex(rects, 75)).toBe(2);
  });

  it("含 divider 时仍正确：光标在 divider 上方 → 插到 divider 之前", () => {
    // A(0,50) B(0,50) divider(0,10) D(0,50) E(0,50)
    // midY: A=25, B=75, divider=155, D=185, E=235
    const rects = [
      rowRect(0),       // A, midY=25
      rowRect(50),      // B, midY=75
      dividerRect(100), // divider, midY=105
      rowRect(110),     // D, midY=135
      rowRect(160),     // E, midY=185
    ];
    // 光标在 A 下半部（clientY=40），B 上半部（midY=75 之上）→ insertIdx=1
    expect(computeInsertIndex(rects, 40)).toBe(1);
    // 光标在 B 下半部（clientY=90），divider 上半部（midY=105 之上）→ insertIdx=2
    // 注：clientY=90 时 `90 < 105` 为 true → insertIdx=2
    expect(computeInsertIndex(rects, 90)).toBe(2);
  });

  it("含 divider 时：光标在 divider midY → 插到 divider 之后", () => {
    // midY of divider = 105；clientY=105 时 `105 < 105` false → 落到 3
    const rects = [
      rowRect(0),
      rowRect(50),
      dividerRect(100), // midY=105
      rowRect(110),
      rowRect(160),
    ];
    expect(computeInsertIndex(rects, 105)).toBe(3);
  });

  it("含 divider 时：光标在 divider 下方（D 上半部）→ 插到 D 之前", () => {
    // divider midY=105, D midY=135；clientY=120 → `120 < 135` true → insertIdx=3
    const rects = [
      rowRect(0),
      rowRect(50),
      dividerRect(100),
      rowRect(110),
      rowRect(160),
    ];
    expect(computeInsertIndex(rects, 120)).toBe(3);
  });

  it("🐛 REGRESSION: 隐藏段内部拖动 - 拖到 D 和 E 之间应得 4", () => {
    // 修复前的 bug：`items` 数组不含 divider，insertIdx=3 (E 在 items 中
    // 的位置)，`visibleItems[3]=D` → 插到 D 之前（错位 1 格）。
    // 修复后：`visibleItems` 含 divider，E 在其中 idx=4 → insertIdx=4。
    const rects = [
      rowRect(0),       // A
      rowRect(50),      // B
      dividerRect(100), // divider
      rowRect(110),     // D
      rowRect(160),     // E (midY=185)
    ];
    // 光标在 D 下半部 (D midY=135 之下) + E 上半部 (E midY=185 之上) → insertIdx=4
    expect(computeInsertIndex(rects, 170)).toBe(4);
  });

  it("🐛 REGRESSION: 跨段 - 拖 visible 卡到 hidden 段 D 和 E 之间", () => {
    // 修复前：A 被 set display:none；items=[B,C,D,E] (不含 divider)；
    // 光标在 E 上半部 → insertIdx=3 (E 在 items 中位置)；
    // visibleItems=[B,C,divider,D,E]，visibleItems[3]=D → 插到 D 之前（错位）。
    // 修复后：visibleItems=[B,C,divider,D,E] 同样用，E 在其中 idx=4 → insertIdx=4。
    const rects = [
      rowRect(0),       // B (A 已隐藏)
      rowRect(50),      // C
      dividerRect(100), // divider
      rowRect(110),     // D
      rowRect(160),     // E
    ];
    expect(computeInsertIndex(rects, 170)).toBe(4);
  });

  it("空数组 → 0（append 到空 list）", () => {
    expect(computeInsertIndex([], 100)).toBe(0);
  });

  it("光标等于某元素 midY → 落到该元素之后（严格 < 语义）", () => {
    // 这是 onDragMouseMove 的 hit-test 决策点：等于 midY 视为"已穿过"该元素。
    const rects = [rowRect(0), rowRect(50), rowRect(100)];
    // item 0 midY=25；clientY=25 → 落到 1
    expect(computeInsertIndex(rects, 25)).toBe(1);
    // item 1 midY=75；clientY=75 → 落到 2
    expect(computeInsertIndex(rects, 75)).toBe(2);
  });
});

describe("isPlaceholderBeforeDivider", () => {
  it("dividerIdx < 0（无 divider）→ 始终返回 true（视作整体 enabled）", () => {
    expect(isPlaceholderBeforeDivider(0, -1)).toBe(true);
    expect(isPlaceholderBeforeDivider(5, -1)).toBe(true);
  });

  it("placeholder 在 divider 之前 → true", () => {
    expect(isPlaceholderBeforeDivider(2, 4)).toBe(true);
  });

  it("placeholder 在 divider 之后 → false", () => {
    expect(isPlaceholderBeforeDivider(5, 4)).toBe(false);
  });

  it("placeholder 紧贴 divider 之前 → true", () => {
    expect(isPlaceholderBeforeDivider(3, 4)).toBe(true);
  });

  it("placeholder 紧贴 divider 之后 → false", () => {
    expect(isPlaceholderBeforeDivider(5, 4)).toBe(false);
  });
});
