// 悬浮窗：floating.html
// 多 provider 渲染：循环 providers 数组，每段一张卡片。
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./styles.css";

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

function render(snap: QuotaSnapshot) {
  if (!snap.providers || snap.providers.length === 0) {
    app.innerHTML = `<div class="err"><div class="err-title">⏳ 加载中…</div></div>`;
    return;
  }

  const cards = snap.providers.map(renderProviderCard).join("");

  const anyUnconfigured = snap.providers.some(
    (p) => !p.success && (p.error ?? "").includes("未配置 API key"),
  );
  const footHint = anyUnconfigured
    ? "未配置 provider 的 key → 右键托盘 → 设置"
    : "拖动移动 · 右键菜单";

  app.innerHTML = `${cards}<div class="foot">${snap.providers.length} 个 provider · ${footHint}</div>`;

  startCountdown();
}

function renderProviderCard(p: ProviderSnapshot): string {
  if (!p.success) {
    return `<section class="card err-card">
      <header class="card-head">
        <span class="card-title">${providerLabel(p.provider)}</span>
        <span class="err-dot">●</span>
      </header>
      <div class="err-msg">${escapeHtml(p.error ?? "未知错误")}</div>
      <button class="err-btn open-settings">打开设置</button>
    </section>`;
  }
  const rowsHtml = p.rows.map(renderRow).join("");
  return `<section class="card">
    <header class="card-head">
      <span class="card-title">${providerLabel(p.provider)}</span>
      <span class="card-dot ${dotClass(p)}"></span>
    </header>
    ${rowsHtml}
  </section>`;
}

function renderRow(r: QuotaRow, idx: number): string {
  // 百分比模式（MiniMax 5h/周）
  if (r.utilization != null) {
    return `<div class="row" data-tier="${idx}" data-resets-at="${r.resets_at ?? ""}">
      <div class="row-label">
        <span>${escapeHtml(r.label)}</span>
        <span class="pct ${colorClass(r.utilization)}">${formatPct(r.utilization)}</span>
      </div>
      <div class="bar"><div class="bar-fill ${colorClass(r.utilization)}" style="width:${barWidth(r.utilization)}%"></div></div>
      <div class="row-foot">${formatReset(r.resets_at, r.label + " 重置")}</div>
    </div>`;
  }
  // 余额模式（DeepSeek）
  if (r.remaining != null) {
    return `<div class="row balance-row">
      <div class="row-label">
        <span>${escapeHtml(r.label)}</span>
        <span class="pct balance">${formatAmount(r.remaining)} ${escapeHtml(r.unit ?? "")}</span>
      </div>
    </div>`;
  }
  // 状态行（DeepSeek is_available）
  if (r.extra?.display) {
    const ok = r.extra.is_available !== false;
    return `<div class="row status-row">
      <div class="row-label">
        <span>${escapeHtml(r.label)}</span>
        <span class="status ${ok ? "ok" : "alert"}">${escapeHtml(r.extra.display)}</span>
      </div>
    </div>`;
  }
  return "";
}

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
  // 千分位 + 2 位小数
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

function formatReset(ms: number | null, prefix: string): string {
  if (!ms) return prefix;
  const dt = new Date(ms);
  return `${prefix} ${pad2(dt.getHours())}:${pad2(dt.getMinutes())}`;
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

function providerLabel(p: "minimax" | "deepseek"): string {
  return p === "minimax" ? "🟪 MiniMax" : "🟦 DeepSeek";
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
  listen<QuotaSnapshot>("musage://snapshot", (e) => {
    render(e.payload);
  }).then((fn) => (unlisten = fn));

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

  window.addEventListener("beforeunload", () => {
    if (unlisten) unlisten();
    if (countdownTimer !== null) clearInterval(countdownTimer);
  });
}

init();
