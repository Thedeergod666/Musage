// 设置面板工具函数 + 共享 mutable 状态
//
// 状态管理用最朴素的模块级单例 + 共享 mutable 变量（不引 React/Solid）。
// v1 不大改，未来 Stage 3+ 加侧边栏 + 动态渲染时也只用 DOM 局部 patch，
// 不会全量 re-render，所以不需要 reactive 框架。

import { getLocale } from "../i18n";

/// 当前已知的 source id 列表（**派生自后端 registry**，不写死）。
///
/// 之前叫 `BUILTIN_ORDER` —— 写死 8 个 provider id。问题：
/// 1. 加新 provider 要改 2 处（mod.rs + utils.ts），容易漏
/// 2. PR 3 的 CustomSource（`custom_<uuid>`）会从 `list_custom_sources` 动态来，
///    写死 list 接不住
///
/// 新方案：`renderOrderSection(sources, ...)` 每次渲染时把 `sources.map(s => s.id)`
/// 写进 `setCurrentKnownIds()`，canonicalizeOrder / buildOrderItems 走这个 list。
/// 后续加新 provider（builtin / custom）都**零代码改动**自动出现在浮窗顺序里。
let currentKnownIds: string[] = [];

export function setCurrentKnownIds(ids: string[]): void {
  currentKnownIds = ids.slice();
}

export function getCurrentKnownIds(): string[] {
  return currentKnownIds;
}

/// 当前内存里的 provider 顺序（同步 set_provider_order 时维护，
/// UI 渲染时也以这个为准）。
export let currentProviderOrder: string[] = [];

export function setCurrentProviderOrder(order: string[]): void {
  currentProviderOrder = order;
}

// ── DOM helper ──────────────────────────────────────────────

export const $ = <T extends HTMLElement>(s: string): T => {
  const el = document.querySelector<T>(s);
  if (!el) throw new Error(`not found: ${s}`);
  return el;
};

/// DOM helper：createElement + setAttribute + 拼接 children 的 6 行简写。
/// 用法：el("div", { class: "foo", "data-id": "minimax" }, "text", childEl)
/// 不用 React/Preact 也能 1 行起 1 个带 attr 的元素 —— 阶段 4 动态渲染用。
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs?: Record<string, string>,
  ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") e.className = v;
      else e.setAttribute(k, v);
    }
  }
  for (const c of children) {
    e.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return e;
}

// ── Tab 切换 ────────────────────────────────────────────────

export function setupTabs() {
  const tabs = document.querySelectorAll<HTMLButtonElement>(".tab");
  const panels = document.querySelectorAll<HTMLElement>(".provider-panel");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const p = tab.dataset.p!;
      tabs.forEach((t) => t.classList.toggle("active", t.dataset.p === p));
      panels.forEach((pn) =>
        pn.classList.toggle("active", pn.dataset.p === p),
      );
    });
  });
}

// ── 全局 flash（顶部 #flash，schema overrides 等整段错误用）──

export type FlashKind = "ok" | "warn" | "err";

let flashTimer: number | null = null;

/**
 * 双签名：
 * - 旧风格 flash(msg) / flash(msg, true) — 57 处调用兼容
 * - 新风格 flash("ok", msg) / flash("err", msg) / flash("warn", msg)
 */
export function flash(kindOrMsg: FlashKind | string, msgOrErr?: string | boolean) {
  let kind: FlashKind;
  let text: string;
  if (typeof msgOrErr === "boolean" || msgOrErr === undefined) {
    // 旧风格: flash(msg) / flash(msg, true)
    text = kindOrMsg as string;
    kind = msgOrErr === true ? "err" : "ok";
  } else {
    kind = kindOrMsg as FlashKind;
    text = msgOrErr;
  }

  const flashEl = $("#flash") as HTMLElement;
  flashEl.textContent = text;
  flashEl.style.color = kind === "ok" ? "#4caf50" : kind === "warn" ? "#ff9800" : "#f44336";
  flashEl.style.display = "block";
  if (flashTimer !== null) clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => {
    flashEl.replaceChildren();
    flashEl.style.display = "none";
  }, 2500);
}

// ── 文本/时间/数字格式 ─────────────────────────────────────

// P1 frontend 阶段：providerDisplay 已废弃 —— 直接用 t(\`provider.${id}.name\`)
// 走 src/i18n helper，跟 [src/main.ts:37 PROVIDER_META] 共享同一组 key。
// 历史：以前是个 switch hardcode 13 个名字，i18n 后三处合一（main.ts /
// settings/logos.ts / utils.ts:providerDisplay）。
// 调用方迁移：把 \`providerDisplay(x)\` 替换成 \`t(\\\`provider.\\\${x}.name\\\`)\`。

export function formatAmount(v: number): string {
  // P1 fix: 之前硬编码 "zh-CN"，跟当前 locale 无关。改成 getLocale() 当前值。
  // 数字格式化（千位 / 小数点）跨 locale 差异大：de-DE → 1.234,56，en-US → 1,234.56。
  return v.toLocaleString(getLocale(), {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
}

export function withTimeout<T>(
  p: Promise<T>,
  ms: number,
  msg: string,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(msg)), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}

export function formatLogTime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** Debounce wrapper —— 连续调用只触发最后一次，delay ms 后执行。
 *  用法: `const flush = debounce(async () => {...}, 300); input.oninput = flush;`
 */
export function debounce<T extends (...args: never[]) => unknown>(
  fn: T,
  delay: number,
): (...args: Parameters<T>) => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  return (...args: Parameters<T>) => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      void fn(...args);
    }, delay);
  };
}

export function escapeHtml(s: string): string {
  return s.replace(
    /[&<>"']/g,
    (c) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[c]!,
  );
}
