// 悬浮窗：floating.html
// 多 provider 渲染：每段一张卡片。
//
// 渲染策略：增量 DOM 更新（diff by `data-provider` / `data-row-key`），
// 而不是 innerHTML 全量替换。innerHTML 会导致一帧空白，肉眼可见"闪一下"。
//
// 置顶/置底行为（设置面板里选）：
// - pin_top    ：一直置顶（系统 always-on-top）
// - pin_bottom ：默认不置顶（会被其它窗口盖住），鼠标 hover 进浮窗时临时置顶
// - normal     ：不强制层级
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import minimaxLogo from "./assets/minimax-logo.png";
import deepseekLogo from "./assets/deepseek-icon.png";
import "./styles.css";

/// 静态映射：provider id → 官网 logo + 显示名
/// logo 走 Vite `?url` import 拿到打包后的 URL
const PROVIDER_META: Record<string, { name: string; logo: string }> = {
  minimax: { name: "MiniMax", logo: minimaxLogo },
  deepseek: { name: "DeepSeek", logo: deepseekLogo },
};

type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

// ── 类型（必须和 src-tauri/src/providers/mod.rs 对齐）──

interface QuotaRow {
  label: string;
  utilization: number | null;
  remaining: number | null;
  total: number | null;
  resets_at: number | null;
  unit: string | null;
  extra: { is_available?: boolean; display?: string } | null;
}

interface ProviderSnapshot {
  provider: "minimax" | "deepseek";
  success: boolean;
  rows: QuotaRow[];
  error: string | null;
  error_kind?:
    | "unconfigured_key"
    | "auth_failed"
    | "rate_limited"
    | "network"
    | "parse"
    | "schema_unknown"
    | "server_error"
    | "other"
    | null;
  fetched_at: number | null;
  raw?: unknown;
  is_healthy: boolean;
}

interface QuotaSnapshot {
  providers: ProviderSnapshot[];
  fetched_at: number | null;
}

const app = document.getElementById("app")!;
let countdownTimer: number | null = null;

const ERROR_LABELS: Record<string, string> = {
  unconfigured_key: "未配置 Key",
  auth_failed: "Key 无效",
  rate_limited: "请求过快",
  network: "网络错误",
  parse: "响应异常",
  schema_unknown: "Schema 未知",
  server_error: "服务异常",
  other: "未知错误",
};

function errorKindLabel(k: string): string {
  return ERROR_LABELS[k] ?? "未知错误";
}

// ── 渲染入口 ──

function render(snap: QuotaSnapshot) {
  if (!snap.providers || snap.providers.length === 0) {
    renderLoading();
    return;
  }

  // 1. 增量更新每张 provider 卡片
  const existingCards = new Map<string, HTMLElement>();
  app.querySelectorAll<HTMLElement>(".card[data-provider]").forEach((el) => {
    const key = el.dataset.provider;
    if (key) existingCards.set(key, el);
  });

  let anchor: ChildNode | null = null;
  for (const p of snap.providers) {
    let card = existingCards.get(p.provider);
    if (card) {
      existingCards.delete(p.provider);
    } else {
      card = buildCardSkeleton(p.provider);
      // 保持顺序：插在 anchor 之后
      if (anchor && anchor.parentNode) {
        anchor.parentNode.insertBefore(card, anchor.nextSibling);
      } else {
        app.insertBefore(card, app.firstChild);
      }
    }
    updateCard(card, p);
    anchor = card;
  }
  // 移除 snap 里没有的卡（provider 被关了）
  for (const orphan of existingCards.values()) {
    orphan.remove();
  }

  // 2. 底部 footer（始终只有 1 个）
  updateFoot(snap);

  startCountdown();
}

function renderLoading() {
  // 留一个占位 .err —— 首次启动 / 还没拉到数据时
  if (!app.querySelector(".err")) {
    app.innerHTML = `<div class="err"><div class="err-title">⏳ 加载中…</div></div>`;
  }
}

// ── 卡片 ──

function buildCardSkeleton(providerId: string): HTMLElement {
  const card = document.createElement("section");
  card.className = "card";
  card.dataset.provider = providerId;
  card.innerHTML = `
    <header class="card-head">
      <span class="card-title">
        <img class="card-logo" alt="" />
        <span class="card-name"></span>
      </span>
      <span class="card-dot"></span>
    </header>
    <div class="rows"></div>
  `;
  return card;
}

function updateCard(card: HTMLElement, p: ProviderSnapshot): void {
  const title = card.querySelector<HTMLElement>(".card-title")!;
  const meta = PROVIDER_META[p.provider] ?? { name: p.provider, logo: "" };
  const logo = title.querySelector<HTMLImageElement>(".card-logo")!;
  const name = title.querySelector<HTMLElement>(".card-name")!;
  if (logo.src !== meta.logo) logo.src = meta.logo;
  logo.alt = meta.name;
  name.textContent = meta.name;

  const dot = card.querySelector<HTMLElement>(".card-dot")!;
  dot.className = `card-dot ${dotClass(p)}`;

  const rowsBox = card.querySelector<HTMLElement>(".rows")!;

  if (!p.success) {
    // 错误卡片：替换 rowsBox 为错误 UI
    const kind = p.error_kind ?? "other";
    const label = errorKindLabel(kind);
    const needsSettings = kind === "unconfigured_key" || kind === "auth_failed";
    const settingsBtn = needsSettings
      ? `<button class="err-btn open-settings">打开设置</button>`
      : "";
    const schemaHint =
      kind === "schema_unknown"
        ? `<div class="hint">→ 设置面板 · Schema overrides 加新字段名</div>`
        : "";
    // err-* class 让 CSS 决定错误卡样式
    card.classList.add("err-card", `err-${kind}`);
    // header 里也加个 err-label 标种类
    const head = card.querySelector<HTMLElement>(".card-head")!;
    let headLabel = head.querySelector<HTMLElement>(".err-label");
    if (!headLabel) {
      headLabel = document.createElement("span");
      headLabel.className = "err-label";
      head.appendChild(headLabel);
    }
    headLabel.textContent = label;
    // rowsBox 重新填充
    rowsBox.innerHTML = `
      <div class="err-msg">${escapeHtml(p.error ?? "未知错误")}</div>
      ${settingsBtn}
      ${schemaHint}
    `;
    return;
  }

  // 成功卡片：rowsBox 走 diff
  card.classList.remove("err-card");
  card.classList.forEach((c) => {
    if (c.startsWith("err-") && c !== "err-card") card.classList.remove(c);
  });
  // 清掉 err-label
  const headLabel = card.querySelector<HTMLElement>(".err-label");
  if (headLabel) headLabel.remove();

  const existing = new Map<string, HTMLElement>();
  rowsBox.querySelectorAll<HTMLElement>(".row[data-row-key]").forEach((el) => {
    const k = el.dataset.rowKey;
    if (k) existing.set(k, el);
  });

  let rowAnchor: ChildNode | null = null;
  for (const r of p.rows) {
    const key = rowKey(r);
    let rowEl = existing.get(key);
    if (rowEl) {
      existing.delete(key);
    } else {
      rowEl = buildRowSkeleton(r);
      rowEl.dataset.rowKey = key;
      if (rowAnchor && rowAnchor.parentNode === rowsBox) {
        rowAnchor.parentNode.insertBefore(rowEl, rowAnchor.nextSibling);
      } else {
        rowsBox.insertBefore(rowEl, rowsBox.firstChild);
      }
    }
    updateRow(rowEl, r);
    rowAnchor = rowEl;
  }
  for (const orphan of existing.values()) orphan.remove();
}

// ── 行 ──

function rowKey(r: QuotaRow): string {
  if (r.utilization != null) return `pct:${r.label}`;
  if (r.remaining != null) return `amt:${r.label}`;
  if (r.extra?.display) return `status:${r.label}`;
  return "unknown";
}

function buildRowSkeleton(r: QuotaRow): HTMLElement {
  const row = document.createElement("div");
  row.className = "row";
  if (r.utilization != null) {
    row.innerHTML = `
      <div class="row-label">
        <span></span>
        <span class="pct"></span>
      </div>
      <div class="bar"><div class="bar-fill"></div></div>
      <div class="row-foot"></div>
    `;
  } else if (r.remaining != null) {
    row.classList.add("balance-row");
    row.innerHTML = `
      <div class="row-label">
        <span></span>
        <span class="pct balance"></span>
      </div>
    `;
  } else if (r.extra?.display) {
    row.classList.add("status-row");
    row.innerHTML = `
      <div class="row-label">
        <span></span>
        <span class="status"></span>
      </div>
    `;
  }
  return row;
}

function updateRow(rowEl: HTMLElement, r: QuotaRow): void {
  if (r.utilization != null) {
    const cls = colorClass(r.utilization);
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = r.label;
    const pct = rowEl.querySelector<HTMLElement>(".pct")!;
    pct.textContent = formatPct(r.utilization);
    pct.className = `pct ${cls}`;
    const bar = rowEl.querySelector<HTMLElement>(".bar-fill")!;
    bar.className = `bar-fill ${cls}`;
    bar.style.width = `${barWidth(r.utilization)}%`;
    if (r.resets_at) rowEl.dataset.resetsAt = String(r.resets_at);
    else delete rowEl.dataset.resetsAt;
  } else if (r.remaining != null) {
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = r.label;
    const pct = rowEl.querySelector<HTMLElement>(".pct")!;
    pct.textContent = `${formatAmount(r.remaining)} ${escapeHtml(r.unit ?? "")}`;
    pct.className = "pct balance";
  } else if (r.extra?.display) {
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = r.label;
    const ok = r.extra.is_available !== false;
    const status = rowEl.querySelector<HTMLElement>(".status")!;
    status.textContent = r.extra.display;
    status.className = `status ${ok ? "ok" : "alert"}`;
  }
}

// ── Footer ──

function updateFoot(snap: QuotaSnapshot) {
  let foot = app.querySelector<HTMLElement>(".foot");
  const anyUnconfigured = snap.providers.some(
    (p) => !p.success && (p.error ?? "").includes("未配置 API key"),
  );
  const hint = anyUnconfigured
    ? "未配置 provider 的 key → 右键托盘 → 设置"
    : "拖动移动 · 右键菜单";
  const text = `${snap.providers.length} 个 provider · ${hint}`;
  if (foot) {
    foot.textContent = text;
  } else {
    foot = document.createElement("div");
    foot.className = "foot";
    foot.textContent = text;
    app.appendChild(foot);
  }
}

// ── 倒计时（每秒就地更新 .row-foot，不动其他 DOM） ──

function startCountdown() {
  if (countdownTimer !== null) clearInterval(countdownTimer);
  countdownTimer = window.setInterval(updateCountdowns, 1000);
}

function updateCountdowns() {
  const rows = app.querySelectorAll<HTMLElement>(".row[data-resets-at]");
  rows.forEach((row) => {
    const raw = row.dataset.resetsAt;
    if (!raw) return;
    const ms = Number(raw);
    if (!Number.isFinite(ms) || ms <= 0) return;
    const foot = row.querySelector<HTMLElement>(".row-foot");
    if (!foot) return;
    const label = row.querySelector<HTMLElement>(".row-label > span:first-child")?.textContent ?? "";
    foot.textContent = formatResetWithCountdown(ms, label + " 重置");
  });
}

// ── 工具函数 ──

function formatPct(v: number | null | undefined): string {
  if (v == null) return "—";
  return `${Math.round(v)}%`;
}

function formatAmount(v: number | null | undefined): string {
  if (v == null) return "—";
  return v.toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function colorClass(util: number): string {
  if (util < 70) return "ok";
  if (util < 90) return "warn";
  return "alert";
}

function dotClass(p: ProviderSnapshot): string {
  if (!p.success) return "alert";
  return p.is_healthy ? "ok" : "alert";
}

function barWidth(util: number | null | undefined): number {
  if (util == null) return 0;
  return Math.min(util, 100);
}

function formatResetWithCountdown(ms: number, prefix: string): string {
  const remainMs = ms - Date.now();
  const dt = new Date(ms);
  const time = `${pad2(dt.getHours())}:${pad2(dt.getMinutes())}`;
  if (remainMs <= 0) {
    return `${prefix} ${time}（已重置）`;
  }
  const minutes = Math.floor(remainMs / 60000);
  if (minutes < 60) {
    return `${prefix} ${time}（${minutes} 分钟后）`;
  }
  const hours = Math.floor(minutes / 60);
  const mins = minutes % 60;
  return `${prefix} ${time}（${hours}h${pad2(mins)}m）`;
}

function pad2(n: number): string {
  return n.toString().padStart(2, "0");
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
}

// ── 启动 ──

async function init() {
  const w = getCurrentWindow();
  // 拖动：左键按住任意非按钮区域 → start_dragging
  app.addEventListener("mousedown", (e) => {
    const target = e.target as HTMLElement;
    if (target.closest("button, input, select, a")) return;
    e.preventDefault();
    w.startDragging();
  });
  // 双击 → 立即刷新
  app.addEventListener("dblclick", async () => {
    try {
      const snap = await invoke<QuotaSnapshot>("refresh_now");
      render(snap);
    } catch (e) {
      console.error(e);
    }
  });

  // 订阅后端推送
  let unlisten: UnlistenFn | null = null;
  let unlistenHover: UnlistenFn | null = null;
  listen<QuotaSnapshot>("musage://snapshot", (e) => {
    render(e.payload);
  }).then((fn) => (unlisten = fn));

  // ── Hover 状态同步：驱动 body[data-hover] 让 iOS 26 玻璃效果生效 ──
  //
  // 双路径并存（先到先生效，幂等）：
  //   1. Rust `musage://floating-hover`（macOS 必需 —— WKWebView 非
  //      key window 不分发 mouseMoved，CSS `:hover` 在浮窗未聚焦时失效，
  //      Rust 用 NSEvent.mouseLocation 全局轮询绕过）
  //   2. JS mouseenter/mouseleave（Win/Linux 主路径；macOS 聚焦态下兜底）
  //
  // 跟原有 PinBottom mode 的 setupHoverRaise(level 切换 IPC) 并行存在，
  // 关注点不同：这里只管 CSS attribute，不动 always-on-top。
  const setHoverAttr = (on: boolean) => {
    if (on) document.body.dataset.hover = "1";
    else delete document.body.dataset.hover;
  };
  document.body.addEventListener("mouseenter", () => setHoverAttr(true));
  document.body.addEventListener("mouseleave", () => setHoverAttr(false));
  listen<boolean>("musage://floating-hover", (e) => {
    setHoverAttr(e.payload);
  }).then((fn) => (unlistenHover = fn));

  // 启动时立即 render loading 占位，避免空白窗口
  app.innerHTML = `<div class="err"><div class="err-title">⏳ 加载中…</div></div>`;

  // 初次拉取
  try {
    const snap = await invoke<QuotaSnapshot>("get_snapshot");
    if (snap.fetched_at && snap.providers.length > 0) {
      render(snap);
    } else {
      const fresh = await invoke<QuotaSnapshot>("refresh_now");
      render(fresh);
    }
  } catch (e) {
    app.innerHTML = `<div class="err"><div class="err-title">⚠ 错误</div><div class="err-msg">${escapeHtml(String(e))}</div><button class="err-btn open-settings">打开设置</button><div class="hint">托盘右键 → 设置 亦可</div></div>`;
  }

  // 事件代理：所有 .open-settings 按钮
  app.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    if (t.closest(".open-settings")) {
      e.stopPropagation();
      invoke("open_settings_window").catch((e) => console.error(e));
    }
  });

  // 读取用户选的置顶/置底模式。
  // PinBottom 模式下，监听 mouseenter/mouseleave 让后端临时切到 always-on-top。
  let pinMode: FloatingPinMode = "pin_top";
  try {
    const cfg = await invoke<{ floating_pin_mode?: FloatingPinMode }>("get_config");
    pinMode = cfg.floating_pin_mode ?? "pin_top";
  } catch (e) {
    console.error("读 pin mode 失败", e);
  }
  setupHoverRaise(pinMode);

  // 设置面板改了模式时，重新挂/摘 hover 监听。
  // （设置面板那边调 set_floating_pin_mode 会 emit 这个事件）
  listen<FloatingPinMode>("musage://pin-mode-changed", (e) => {
    // 清掉旧的监听再装新的（幂等）
    document.removeEventListener("mouseenter", hoverEnterHandler);
    document.removeEventListener("mouseleave", hoverLeaveHandler);
    setupHoverRaise(e.payload);
  });

  window.addEventListener("beforeunload", () => {
    if (unlisten) unlisten();
    if (unlistenHover) unlistenHover();
    if (countdownTimer !== null) clearInterval(countdownTimer);
  });
}

/// 在 PinBottom 模式下挂 mouseenter/mouseleave 监听，调用
/// `set_floating_hover_raise` 让后端临时把 always-on-top 打开 / 关掉。
/// 其它模式不挂监听，避免无意义的 IPC。
///
/// 处理器是 module-scope 命名函数，setupHoverRaise 多次调用时先摘再装，幂等。
function hoverEnterHandler() {
  invoke("set_floating_hover_raise", { hovering: true }).catch((e) =>
    console.error(e),
  );
}
function hoverLeaveHandler() {
  invoke("set_floating_hover_raise", { hovering: false }).catch((e) =>
    console.error(e),
  );
}

function setupHoverRaise(mode: FloatingPinMode) {
  if (mode !== "pin_bottom") return;
  // 鼠标进浮窗（窗口整体）→ 临时置顶
  document.addEventListener("mouseenter", hoverEnterHandler);
  // 鼠标离开浮窗 → 取消置顶，让其它窗口能盖住它
  document.addEventListener("mouseleave", hoverLeaveHandler);
}

init();
