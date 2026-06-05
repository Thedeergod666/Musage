// 悬浮窗：floating.html
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./styles.css";

interface QuotaTier {
  name: string;
  utilization: number;
  resets_at: number | null;
}

interface QuotaSnapshot {
  success: boolean;
  five_hour: QuotaTier | null;
  weekly: QuotaTier | null;
  error: string | null;
  fetched_at: number | null;
  region: "cn" | "en";
}

const app = document.getElementById("app")!;
let countdownTimer: number | null = null;

function render(snap: QuotaSnapshot) {
  if (!snap.success) {
    app.innerHTML = `
      <div class="err">
        <div class="err-title">⚠ 拉取失败</div>
        <div class="err-msg">${escapeHtml(snap.error ?? "未知错误")}</div>
        <button class="err-btn" id="open-settings">打开设置</button>
        <div class="hint">托盘右键 → 设置 亦可</div>
      </div>
    `;
    const btn = document.getElementById("open-settings");
    if (btn) {
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        invoke("open_settings_window").catch((e) => console.error(e));
      });
    }
    return;
  }

  const fh = snap.five_hour;
  const wk = snap.weekly;

  app.innerHTML = `
    <div class="row" data-tier="five_hour" data-resets-at="${fh?.resets_at ?? ""}">
      <div class="row-label">
        <span>5h</span>
        <span class="pct ${colorClass(fh?.utilization ?? 0)}">${formatPct(fh?.utilization)}</span>
      </div>
      <div class="bar"><div class="bar-fill ${colorClass(fh?.utilization ?? 0)}" style="width:${barWidth(fh?.utilization)}%"></div></div>
      <div class="row-foot">${formatReset(fh?.resets_at ?? null, "5h 重置")}</div>
    </div>
    <div class="row" data-tier="weekly" data-resets-at="${wk?.resets_at ?? ""}">
      <div class="row-label">
        <span>周</span>
        <span class="pct ${colorClass(wk?.utilization ?? 0)}">${formatPct(wk?.utilization)}</span>
      </div>
      <div class="bar"><div class="bar-fill ${colorClass(wk?.utilization ?? 0)}" style="width:${barWidth(wk?.utilization)}%"></div></div>
      <div class="row-foot">${formatReset(wk?.resets_at ?? null, "周重置")}</div>
    </div>
    <div class="foot">${regionLabel(snap.region)} · 拖动移动 · 右键菜单</div>
  `;

  startCountdown();
}

function startCountdown() {
  if (countdownTimer !== null) {
    clearInterval(countdownTimer);
  }
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
    const prefix = row.dataset.tier === "weekly" ? "周重置" : "5h 重置";
    foot.textContent = formatResetWithCountdown(ms, prefix);
  });
}

function formatPct(v: number | null | undefined): string {
  if (v == null) return "—";
  return `${Math.round(v)}%`;
}

function colorClass(util: number): string {
  if (util < 70) return "ok";
  if (util < 90) return "warn";
  return "alert";
}

function barWidth(util: number | null | undefined): number {
  if (util == null) return 0;
  return Math.min(util, 100);
}

function formatReset(ms: number | null, prefix: string): string {
  if (!ms) return prefix;
  const dt = new Date(ms);
  return `${prefix} ${dt.getHours().toString().padStart(2, "0")}:${dt.getMinutes().toString().padStart(2, "0")}`;
}

function formatResetWithCountdown(ms: number, prefix: string): string {
  const remainMs = ms - Date.now();
  const dt = new Date(ms);
  const time = `${dt.getHours().toString().padStart(2, "0")}:${dt.getMinutes().toString().padStart(2, "0")}`;
  if (remainMs <= 0) {
    return `${prefix} ${time}（已重置）`;
  }
  const minutes = Math.floor(remainMs / 60000);
  if (minutes < 60) {
    return `${prefix} ${time}（${minutes} 分钟后）`;
  }
  const hours = Math.floor(minutes / 60);
  const mins = minutes % 60;
  return `${prefix} ${time}（${hours}h${mins.toString().padStart(2, "0")}m）`;
}

function regionLabel(r: "cn" | "en"): string {
  return r === "cn" ? "🇨🇳 CN" : "🌐 EN";
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
}

// 启动时拉一次 + 订阅实时事件
async function init() {
  const w = getCurrentWindow();
  // 拖动：左键按住任意非按钮区域 → start_dragging
  app.addEventListener("mousedown", (e) => {
    const target = e.target as HTMLElement;
    if (target.closest("button, input, select, a")) return; // 交互元素不拖
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

  // 初次拉取
  try {
    const snap = await invoke<QuotaSnapshot>("get_snapshot");
    if (snap.fetched_at) {
      render(snap);
    } else {
      // 还没数据，主动触发一次
      const fresh = await invoke<QuotaSnapshot>("refresh_now");
      render(fresh);
    }
  } catch (e) {
    app.innerHTML = `<div class="err"><div class="err-title">⚠ 错误</div><div class="err-msg">${escapeHtml(String(e))}</div></div>`;
  }

  window.addEventListener("beforeunload", () => {
    if (unlisten) unlisten();
    if (countdownTimer !== null) clearInterval(countdownTimer);
  });
}

init();
