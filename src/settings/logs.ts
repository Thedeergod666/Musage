// 设置面板的日志模块（查看应用运行日志 + 筛选 + 复制 + 清空）
//
// 后端：每次 provider 拉取失败（去重窗口 60s）写一条 LogEntry 到 LogStore，
//       设置面板手动 `get_recent_logs` 拉。
// 前端：打开面板时自动拉一次；点 "刷新" 再拉；点 "清空" 调 `clear_logs`。
//       不做 live push（避免 IPC 噪音 + 设置面板跟浮窗双源同步）。
//       最新消息在顶部（用户一打开就能看到最近错误，不用翻到底）。
//
// v0.6+ 起 logs 单独成 section（renderLogsSection），不再塞 legacy template。

import { clearLogs as clearLogsIPC, getRecentLogs } from "./api";
import { el, escapeHtml, flash, formatLogTime } from "./utils";
import { t } from "../i18n";
import { navLogsIcon } from "../icons";
import type { LogEntry } from "./types";

/// v0.6+ 把 logs 渲染到独立 section。
/// 暴露 loadLogs / clearLogs / copyLogs / handleLogFilter 给按钮事件。
export function renderLogsSection(container: HTMLElement) {
  // 筛选 select
  const filter = el("select", {
    id: "logs-filter",
    class: "logs-filter",
    title: t("settings.logs.filter_level"),
  }) as HTMLSelectElement;
  filter.appendChild(el("option", { value: "all" }, t("settings.logs.filter_all")));
  filter.appendChild(el("option", { value: "error" }, t("settings.logs.filter_error")));
  filter.appendChild(el("option", { value: "warn" }, t("settings.logs.filter_warn")));
  filter.appendChild(el("option", { value: "info" }, t("settings.logs.filter_info")));
  filter.addEventListener("change", () => void loadLogs());

  // 按钮
  const refresh = el("button", { id: "logs-refresh", class: "primary" }, t("settings.common.refresh"));
  refresh.addEventListener("click", () => void loadLogs());
  const copy = el("button", { id: "logs-copy", class: "primary", title: t("settings.logs.copy_filtered") }, t("settings.common.copy"));
  copy.addEventListener("click", () => void copyLogs());
  const clear = el("button", { id: "logs-clear", class: "danger" }, t("settings.logs.clear_all"));
  clear.addEventListener("click", () => void clearLogs());
  const count = el("span", { id: "logs-count" });

  // 列表
  const list = el("div", { class: "logs-list", id: "logs-list" });

  container.appendChild(
    el("section", { class: "section-card", id: "logs-section" },
      (() => { const img = document.createElement("img"); img.src = navLogsIcon; img.alt = ""; img.className = "icon icon-20"; return el("h2", {}, img, ` ${t("settings.logs.section_title")}`); })(),
      el("div", { class: "help" }, t("settings.logs.section_help")),
      el("div", { class: "row row-tight" }, filter, refresh, copy, clear, count),
      list,
    ),
  );
}

export async function loadLogs() {
  const list = document.getElementById("logs-list");
  const count = document.getElementById("logs-count");
  if (!list) return;
  try {
    const entries = await getRecentLogs(200);
    const filter = currentLogFilter();
    const filtered =
      filter === "all" ? entries : entries.filter((e) => e.level === filter);
    renderLogs(filtered);
    // count 数字要反映"当前显示"而非"总条数"，跟筛选状态保持一致
    if (count) {
      count.textContent =
        filter === "all"
          ? t("settings.logs.count_all", { count: entries.length })
          : t("settings.logs.count_filtered", {
              filtered: filtered.length,
              total: entries.length,
            });
    }
  } catch (e) {
    list.innerHTML = `<div class="logs-empty error">${t("settings.logs.load_failed", { err: escapeHtml(String(e)) })}</div>`;
    if (count) count.textContent = "";
    console.error("[logs] load failed", e);
  }
}

function renderLogs(entries: LogEntry[]) {
  const list = document.getElementById("logs-list");
  if (!list) return;
  if (entries.length === 0) {
    list.innerHTML = `<div class="logs-empty">${t("settings.logs.no_logs_filtered")}</div>`;
    return;
  }
  // 后端按时间正序返回（oldest → newest）。用户要"最新在最前面" → 翻过来。
  // 复制后再 reverse，不修改入参（避免影响外面）。
  const reversed = [...entries].reverse();
  list.innerHTML = reversed
    .map((e) => {
      const t = formatLogTime(e.ts);
      return `<div class="log-row">
        <span class="log-time">${escapeHtml(t)}</span>
        <span class="log-level ${e.level}">${escapeHtml(e.level)}</span>
        <span class="log-provider">${escapeHtml(e.provider ?? "—")}</span>
        <span class="log-kind">${escapeHtml(e.kind ?? "—")}</span>
        <span class="log-msg">${escapeHtml(e.message)}</span>
      </div>`;
    })
    .join("");
  // 最新在顶部 → 滚到顶（用户打开面板第一眼就看到最新的）
  list.scrollTop = 0;
}

export async function clearLogs() {
  if (!confirm(t("settings.logs.confirm_clear"))) return;
  try {
    await clearLogsIPC();
    await loadLogs();
    flash(t("settings.logs.cleared"));
  } catch (e) {
    flash(t("settings.logs.clear_failed", { err: String(e) }), true);
    console.error("[logs] clear failed", e);
  }
}

/// 当前筛选级别（"all" | "error" | "warn" | "info"），从 select 元素读
function currentLogFilter(): "all" | "error" | "warn" | "info" {
  const sel = document.getElementById("logs-filter") as HTMLSelectElement | null;
  const v = sel?.value ?? "all";
  if (v === "error" || v === "warn" || v === "info") return v;
  return "all";
}

/// 把当前筛选后的日志拼成纯文本，写剪贴板。每行：
/// `[12:34:56] ERROR minimax 未配置 Key | 未配置 API key`
export async function copyLogs() {
  try {
    const entries = await getRecentLogs(200);
    const filter = currentLogFilter();
    const filtered =
      filter === "all" ? entries : entries.filter((e) => e.level === filter);
    if (filtered.length === 0) {
      flash(t("settings.logs.copy_empty"), true);
      return;
    }
    const lines = filtered.map((e) => {
      const t = formatLogTime(e.ts);
      return `[${t}] ${e.level.toUpperCase().padEnd(5)} ${e.provider ?? "—"} | ${e.kind ?? "—"} | ${e.message}`;
    });
    const text = lines.join("\n");
    await navigator.clipboard.writeText(text);
    flash(t("settings.logs.copied", { count: filtered.length }));
  } catch (e) {
    flash(t("settings.logs.copy_failed", { err: String(e) }), true);
    console.error("[logs] copy failed", e);
  }
}
