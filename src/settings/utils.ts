// 设置面板工具函数 + 共享 mutable 状态
//
// 状态管理用最朴素的模块级单例 + 共享 mutable 变量（不引 React/Solid）。
// v1 不大改，未来 Stage 3+ 加侧边栏 + 动态渲染时也只用 DOM 局部 patch，
// 不会全量 re-render，所以不需要 reactive 框架。

import type { ProviderId } from "./types";

/// provider 排序（拖拽/上下按钮）
/// DOM 结构：每个 provider 一个 row（[id="provider-order-row-{id}"]），
/// 包含 ↑ ↓ 按钮，移位后重新调换 DOM 顺序。保存时按当前 DOM 顺序生成
/// Provider 顺序（per-panel ↑↓ 按钮）
/// 之前的实现是单独的「浮窗顺序」section + ↑↓ 按钮，调完要滚到那里再保存。
/// 改成在每个 provider panel 内部放「位置 X/4」+ ↑↓ 按钮，逻辑分散但
/// 视觉跟 provider 关联更紧密，调完即时生效。
export const BUILTIN_ORDER: ProviderId[] = [
  "minimax",
  "deepseek",
  "xiaomimimo",
  "tavily",
  "zenmux",
];

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

let flashTimer: number | null = null;
export function flash(msg: string, isError = false) {
  const el = $("#flash") as HTMLElement;
  el.textContent = msg;
  el.style.color = isError ? "#f44336" : "#4caf50";
  if (flashTimer !== null) clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (el.textContent = ""), 5000);
}

// ── 文本/时间/数字格式 ─────────────────────────────────────

export function providerDisplay(p: ProviderId): string {
  switch (p) {
    case "minimax":
      return "MiniMax";
    case "deepseek":
      return "DeepSeek";
    case "xiaomimimo":
      return "Xiaomi MiMo";
    case "tavily":
      return "Tavily";
    case "zenmux":
      return "ZenMux";
  }
}

export function formatAmount(v: number): string {
  return v.toLocaleString("zh-CN", {
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
