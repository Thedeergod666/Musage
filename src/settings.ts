// 设置面板 —— 多 provider key + 全局配置
import { invoke } from "@tauri-apps/api/core";
import {
  checkForUpdate,
  downloadAndInstall,
  onUpdateState,
  relaunchApp,
  type UpdateState,
} from "./updater";

type ProviderId = "minimax" | "deepseek" | "xiaomimimo" | "tavily";
type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

interface ProviderConfig {
  enabled: boolean;
  region?: "cn" | "en" | null;
  xiaomi_region?: "cn" | "sgp" | "ams" | null;
  /// 可选：覆盖全局轮询间隔（秒）。None = 用全局 default
  refresh_interval_secs?: number | null;
}

/// 4 个 provider 面板的 id 列表。决定 #3/#4 UI 循环读哪些元素。
const PROVIDER_IDS = ["minimax", "deepseek", "xiaomimimo", "tavily"] as const;

/// Phase 1 新增：注册表元信息（后端 list_sources 返回）。
/// 当前 settings.ts 直接用 list_sources 返回 SourceMeta[] 来构建面板，
/// 这里的接口保留给未来的动态渲染。设置面板硬编码 4 个 tab 还在
/// （Phase 2 会改成从 SourceMeta 自动生成）。
// interface SourceMeta {
//   id: string;
//   display_name: string;
//   auth_kind: "api_key" | "cookie";
//   enabled: boolean;
// }

interface FieldTriple {
  total: string;
  remaining: string;
  end?: string | null;
}

interface TierOverrides {
  count_candidates: FieldTriple[];
}

interface ProviderOverrides {
  five_hour: TierOverrides;
  weekly: TierOverrides;
  /** Phase 1 新增：xiaomi MiMo 月度 tier 候选 */
  monthly?: TierOverrides;
}

interface AppConfig {
  providers: Record<string, ProviderConfig>;
  refresh_interval_secs: number;
  autostart: boolean;
  /// 关闭主窗口时是否隐藏到托盘（旧字段，Rust 端必填，缺了 save_config 会报
  /// "missing field" —— 务必保留在 TS interface 里，否则 spread 展开后丢字段）
  show_in_tray_on_close?: boolean;
  floating_x: number | null;
  floating_y: number | null;
  floating_w?: number | null;
  floating_h?: number | null;
  floating_pin_mode?: FloatingPinMode;
  low_power_mode?: boolean;
  auto_hide_in_fullscreen?: boolean;
  /// Tavily 简洁模式：只显示主指标 + 进度条，隐藏 5 个 endpoint 细分行
  tavily_concise_mode?: boolean;
  /// Provider 在浮窗里的渲染顺序。空数组 = 用 builtin_sources() 注册表顺序
  provider_order?: string[];
  // 用户加的字段名候选（应对 MiniMax 改 schema）
  schema_overrides?: Record<string, ProviderOverrides>;
}

interface ProviderSnapshot {
  /** 兼容字段（minimax / deepseek / xiaomimimo）。Phase 1 起请用 source_id。 */
  provider: ProviderId;
  /** Phase 1 新增。 */
  source_id?: string | null;
  source_display_name?: string | null;
  plan_name?: string | null;
  success: boolean;
  rows: Array<{
    label: string;
    utilization: number | null;
    remaining: number | null;
    used?: number | null;
    total?: number | null;
    unit: string | null;
  }>;
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
}

interface QuotaSnapshot {
  providers: ProviderSnapshot[];
  fetched_at: number | null;
}

const $ = <T extends HTMLElement>(s: string): T => {
  const el = document.querySelector<T>(s);
  if (!el) throw new Error(`not found: ${s}`);
  return el;
};

// ── Tab 切换 ──

function setupTabs() {
  const tabs = document.querySelectorAll<HTMLButtonElement>(".tab");
  const panels = document.querySelectorAll<HTMLElement>(".provider-panel");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const p = tab.dataset.p!;
      tabs.forEach((t) => t.classList.toggle("active", t.dataset.p === p));
      panels.forEach((pn) => pn.classList.toggle("active", pn.dataset.p === p));
    });
  });
}

// ── Provider key 加载 / 保存 / 删除 ──

async function loadKeyStatus(provider: ProviderId) {
  const has = await invoke<boolean>("has_api_key_for", { provider });
  const el = $(`#api-key-status-${provider}`);
  el.textContent = has ? "✓ 已保存到本机" : "未设置";
  el.className = `status ${has ? "ok" : ""}`;
  $(`#api-key-${provider}` as keyof HTMLElementTagNameMap as string) as HTMLInputElement;
}

async function saveKey(provider: ProviderId) {
  const input = $(`#api-key-${provider}`) as HTMLInputElement;
  const key = input.value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 API key", true);
    return;
  }
  try {
    await invoke("set_api_key_for", { provider, key });
    input.value = "";
    await loadKeyStatus(provider);
    flash(`✓ ${providerDisplay(provider)} key 已保存`);
    // 立即拉一次
    await testConn();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

async function deleteKey(provider: ProviderId) {
  if (!confirm(`确认删除 ${providerDisplay(provider)} 的 API key？`)) return;
  await invoke("delete_api_key_for", { provider });
  await loadKeyStatus(provider);
  flash("✓ 已删除");
}

async function loadCookieStatus(provider: ProviderId) {
  const has = await invoke<boolean>("has_cookie_for", { provider });
  const el = document.getElementById(`cookie-status-${provider}`);
  if (el) {
    el.textContent = has ? "✓ 已保存到本机" : "未设置";
    el.className = `status ${has ? "ok" : ""}`;
  }
}

async function saveCookie(provider: ProviderId) {
  const input = document.getElementById(`cookie-${provider}`) as HTMLTextAreaElement | null;
  if (!input) return;
  const cookie = input.value.trim();
  if (!cookie) {
    flash("⚠ 请先粘贴 Cookie", true);
    return;
  }
  try {
    await invoke("set_cookie_for", { provider, cookie });
    input.value = "";
    await loadCookieStatus(provider);
    flash(`✓ ${providerDisplay(provider)} Cookie 已保存`);
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

async function deleteCookie(provider: ProviderId) {
  if (!confirm(`确认删除 ${providerDisplay(provider)} 的 Cookie？`)) return;
  await invoke("delete_cookie_for", { provider });
  await loadCookieStatus(provider);
  flash("✓ Cookie 已删除");
}

// 从 keys.json 读明文 → 写剪贴板。用完即弃，不在 JS 侧长期保存。
async function copyKey(provider: ProviderId) {
  try {
    const key = await invoke<string | null>("get_api_key_for", { provider });
    if (!key) {
      flash(`⚠ ${providerDisplay(provider)} 未设置 key`, true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash(`✓ ${providerDisplay(provider)} key 已复制到剪贴板`);
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
  }
}

// ── 全局配置加载 / 保存 ──

async function loadConfig() {
  const cfg = await invoke<AppConfig>("get_config");
  const regionEl = $("#region") as HTMLSelectElement;
  const minimaxRegion = cfg.providers?.minimax?.region ?? "cn";
  regionEl.value = minimaxRegion;
  const xiaomiRegionEl = $("#xiaomi-region") as HTMLSelectElement;
  const xiaomiRegion = cfg.providers?.xiaomimimo?.xiaomi_region ?? "cn";
  xiaomiRegionEl.value = xiaomiRegion;
  ($("#interval") as HTMLInputElement).value = String(cfg.refresh_interval_secs);
  ($("#autostart") as HTMLInputElement).checked = cfg.autostart;

  // 置顶/置底模式：缺省 = pin_top（保持老版本行为）
  const pinMode: FloatingPinMode = cfg.floating_pin_mode ?? "pin_top";
  const radio = document.querySelector<HTMLInputElement>(
    `input[name="pin-mode"][value="${pinMode}"]`,
  );
  if (radio) radio.checked = true;

  // 性能 / 可见性
  ($("#low-power-mode") as HTMLInputElement).checked = cfg.low_power_mode ?? false;
  ($("#auto-hide-in-fullscreen") as HTMLInputElement).checked =
    cfg.auto_hide_in_fullscreen ?? false;
  // Tavily 简洁模式（默认开）
  const tavilyConcise = document.getElementById("tavily-concise-mode") as HTMLInputElement | null;
  if (tavilyConcise) tavilyConcise.checked = cfg.tavily_concise_mode ?? true;
  // 各 provider 「在浮窗显示」开关（缺省视为 true）+ 轮询间隔覆盖
  for (const id of PROVIDER_IDS) {
    const el = document.getElementById(`enabled-${id}`) as HTMLInputElement | null;
    if (el) {
      el.checked = cfg.providers?.[id]?.enabled ?? true;
      // 即时生效：勾选/取消 → 调 set_provider_enabled → 后端落盘 + emit
      // config-changed → 浮窗 re-fetch → 显隐立即反映
      el.addEventListener("change", () => {
        void invoke("set_provider_enabled", { id, enabled: el.checked }).catch((e) => {
          flash(`✗ 切换显示失败: ${e}`, true);
        });
      });
    }
    const intervalEl = document.getElementById(`interval-${id}`) as HTMLInputElement | null;
    if (intervalEl) {
      const v = cfg.providers?.[id]?.refresh_interval_secs;
      intervalEl.value = v != null ? String(v) : "";
      intervalEl.placeholder = `默认 ${cfg.refresh_interval_secs} 秒（顶部"轮询间隔"）`;
    }
  }

  // Provider 排序（拖拽/上下按钮）
  renderProviderOrder(cfg.provider_order ?? []);

  // schema overrides (高级)
  const ov = cfg.schema_overrides ?? {};
  const mm = ov.minimax ?? { five_hour: { count_candidates: [] }, weekly: { count_candidates: [] } };
  ($("#overrides-5h-minimax") as HTMLTextAreaElement).value = JSON.stringify(
    mm.five_hour?.count_candidates ?? [],
    null,
    2,
  );
  ($("#overrides-weekly-minimax") as HTMLTextAreaElement).value = JSON.stringify(
    mm.weekly?.count_candidates ?? [],
    null,
    2,
  );
  const xm = (ov as Record<string, any>).xiaomimimo ?? { monthly: { count_candidates: [] } };
  const xmMonthly = xm.monthly?.count_candidates ?? [];
  const xmEl = document.getElementById("overrides-monthly-xiaomimimo") as HTMLTextAreaElement | null;
  if (xmEl) xmEl.value = JSON.stringify(xmMonthly, null, 2);
}

/// 置顶/置底模式：选中即生效（通过 `set_floating_pin_mode` 命令）。
/// 不需要走"保存配置"按钮，因为这条改动是即时的，command 内部会同步落盘。
async function applyPinMode(mode: FloatingPinMode) {
  try {
    await invoke("set_floating_pin_mode", { mode });
    const label = mode === "pin_top" ? "已设为：始终置顶" : mode === "pin_bottom" ? "已设为：置底（hover 置顶）" : "已设为：普通窗口";
    flash(`✓ ${label}`);
  } catch (e) {
    flash(`✗ 切换置顶模式失败: ${e}`, true);
  }
}

async function saveConfig() {
  // 解析 schema overrides 的 JSON；解析失败给提示但不影响其它字段保存
  let fiveHourCandidates: FieldTriple[] = [];
  let weeklyCandidates: FieldTriple[] = [];
  let monthlyCandidates: FieldTriple[] = [];
  try {
    const raw5h = ($("#overrides-5h-minimax") as HTMLTextAreaElement).value.trim() || "[]";
    const rawWeek = ($("#overrides-weekly-minimax") as HTMLTextAreaElement).value.trim() || "[]";
    const xmMonthlyEl = document.getElementById("overrides-monthly-xiaomimimo") as HTMLTextAreaElement | null;
    const rawMonth = xmMonthlyEl?.value.trim() || "[]";
    fiveHourCandidates = JSON.parse(raw5h);
    weeklyCandidates = JSON.parse(rawWeek);
    monthlyCandidates = JSON.parse(rawMonth);
    if (!Array.isArray(fiveHourCandidates) || !Array.isArray(weeklyCandidates) || !Array.isArray(monthlyCandidates)) {
      throw new Error("必须是 JSON 数组");
    }
  } catch (e) {
    flash(`✗ Schema overrides JSON 解析失败: ${e}`, true);
    return;
  }

  // 先拉一次当前 config，把浮窗位置/置顶模式这类用户没在面板上改的字段保留下来。
  // 旧实现把 floating_x/y 写死成 null，会把已记忆的窗口位置清空 —— 已修。
  const existing = await invoke<AppConfig>("get_config");
  const pinRadio = document.querySelector<HTMLInputElement>('input[name="pin-mode"]:checked');
  const pinMode: FloatingPinMode =
    (pinRadio?.value as FloatingPinMode | undefined) ??
    existing.floating_pin_mode ??
    "pin_top";

  // 读每个 provider 的轮询间隔覆盖（空字符串 = None = 用全局）
  function readProviderInterval(id: ProviderId): number | null {
    const el = document.getElementById(`interval-${id}`) as HTMLInputElement | null;
    if (!el) return null;
    const raw = el.value.trim();
    if (raw === "") return null;
    const n = parseInt(raw, 10);
    if (!Number.isFinite(n) || n < 10) return 10;  // 后端会再 clamp 一次
    return n;
  }

  const cfg: AppConfig = {
    providers: {
      minimax: {
        enabled:
          (document.getElementById("enabled-minimax") as HTMLInputElement | null)?.checked ?? true,
        region: ($("#region") as HTMLSelectElement).value as "cn" | "en",
        refresh_interval_secs: readProviderInterval("minimax"),
      },
      deepseek: {
        enabled:
          (document.getElementById("enabled-deepseek") as HTMLInputElement | null)?.checked ?? true,
        refresh_interval_secs: readProviderInterval("deepseek"),
      },
      xiaomimimo: {
        enabled:
          (document.getElementById("enabled-xiaomimimo") as HTMLInputElement | null)?.checked ??
          true,
        xiaomi_region: ($("#xiaomi-region") as HTMLSelectElement).value as "cn" | "sgp" | "ams",
        refresh_interval_secs: readProviderInterval("xiaomimimo"),
      },
      tavily: {
        enabled:
          (document.getElementById("enabled-tavily") as HTMLInputElement | null)?.checked ?? true,
        refresh_interval_secs: readProviderInterval("tavily"),
      },
    },
    refresh_interval_secs: parseInt(($("#interval") as HTMLInputElement).value, 10) || 60,
    autostart: ($("#autostart") as HTMLInputElement).checked,
    floating_x: existing.floating_x ?? null,
    floating_y: existing.floating_y ?? null,
    floating_pin_mode: pinMode,
    low_power_mode: ($("#low-power-mode") as HTMLInputElement).checked,
    auto_hide_in_fullscreen: ($("#auto-hide-in-fullscreen") as HTMLInputElement).checked,
    tavily_concise_mode:
      (document.getElementById("tavily-concise-mode") as HTMLInputElement | null)?.checked ?? true,
    provider_order: readProviderOrder(),
    schema_overrides: {
      minimax: {
        five_hour: { count_candidates: fiveHourCandidates },
        weekly: { count_candidates: weeklyCandidates },
      },
      deepseek: { five_hour: { count_candidates: [] }, weekly: { count_candidates: [] } },
      xiaomimimo: {
        five_hour: { count_candidates: [] },
        weekly: { count_candidates: [] },
        monthly: { count_candidates: monthlyCandidates },
      },
    },
  };
  try {
    await invoke("save_config", { cfg });
    flash("✓ 配置已保存");
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

// ── 测试连接 ──

async function testConn() {
  flash("测试中…");
  try {
    const snap = await withTimeout(invoke<QuotaSnapshot>("refresh_now"), 12000, "请求超时（12s）");
    const ok = snap.providers.filter((p) => p.success);
    if (ok.length === 0) {
      const errs = snap.providers.map((p) => {
        const id = p.source_id ?? p.provider;
        return `${id}: ${p.error}`;
      }).join("; ");
      flash(`✗ 全部失败: ${errs}`, true);
      return;
    }
    const summary = ok
      .map((p) => {
        // Phase 1：用 source_id 路由（registry 驱动），provider 字段保兼容
        const id = p.source_id ?? p.provider;
        if (id === "minimax") {
          const fiveHour = p.rows.find((r) => r.utilization != null);
          return fiveHour
            ? `MiniMax 5h ${Math.round(fiveHour.utilization ?? 0)}%`
            : "MiniMax OK";
        } else if (id === "deepseek") {
          const balance = p.rows.find((r) => r.remaining != null);
          return balance
            ? `DeepSeek ${formatAmount(balance.remaining ?? 0)} ${balance.unit ?? ""}`
            : "DeepSeek OK";
        } else if (id === "xiaomimimo") {
          const plan = p.rows.find((r) => r.utilization != null);
          return plan
            ? `Xiaomi 套餐 ${Math.round(plan.utilization ?? 0)}%`
            : "Xiaomi OK";
        } else if (id === "tavily") {
          // 主指标在 used/total/unit="credits" 的那一行
          const main = p.rows.find((r) => r.unit === "credits");
          if (main && main.used != null && main.total != null) {
            return `Tavily ${Math.round(main.used)}/${Math.round(main.total)} credits`;
          }
          return "Tavily OK";
        }
        return `${id} OK`;
      })
      .join(" / ");
    flash(`✓ ${summary}`);
  } catch (e) {
    flash(`✗ 失败: ${e}`, true);
  }
}

function formatAmount(v: number): string {
  return v.toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function withTimeout<T>(p: Promise<T>, ms: number, msg: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(msg)), ms);
    p.then(
      (v) => { clearTimeout(t); resolve(v); },
      (e) => { clearTimeout(t); reject(e); },
    );
  });
}

// ── 工具 ──

/// Provider 排序（拖拽/上下按钮）
/// DOM 结构：每个 provider 一个 row（[id="provider-order-row-{id}"]），
/// 包含 ↑ ↓ 按钮，移位后重新调换 DOM 顺序。保存时按当前 DOM 顺序生成
/// provider_order 数组。
const PROVIDER_DISPLAY_NAME: Record<ProviderId, string> = {
  minimax: "MiniMax",
  deepseek: "DeepSeek",
  xiaomimimo: "Xiaomi MiMo",
  tavily: "Tavily",
};

function renderProviderOrder(order: string[]) {
  const list = document.getElementById("provider-order-list");
  if (!list) return;
  // 用 config.order 做主序，没列出的 provider 沉到末尾（用 builtin 顺序）
  const builtin: ProviderId[] = ["minimax", "deepseek", "xiaomimimo", "tavily"];
  const ordered: ProviderId[] = [];
  for (const id of order) {
    if ((builtin as string[]).includes(id) && !(ordered as string[]).includes(id)) {
      ordered.push(id as ProviderId);
    }
  }
  for (const id of builtin) {
    if (!(ordered as string[]).includes(id)) ordered.push(id);
  }
  list.innerHTML = ordered
    .map(
      (id, i) => `
      <div class="order-row" data-id="${id}">
        <span class="order-name">${PROVIDER_DISPLAY_NAME[id]}</span>
        <span class="order-btns">
          <button type="button" data-move="up"   ${i === 0 ? "disabled" : ""} aria-label="上移">↑</button>
          <button type="button" data-move="down" ${i === ordered.length - 1 ? "disabled" : ""} aria-label="下移">↓</button>
        </span>
      </div>`,
    )
    .join("");
  // 绑按钮
  list.querySelectorAll<HTMLButtonElement>("button[data-move]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const row = btn.closest<HTMLElement>(".order-row");
      if (!row) return;
      const dir = btn.dataset.move;
      const sibling = dir === "up" ? row.previousElementSibling : row.nextElementSibling;
      if (sibling) {
        if (dir === "up") list.insertBefore(row, sibling);
        else list.insertBefore(sibling, row);
        refreshOrderButtons();
        // 即时落盘 + emit config-changed → 浮窗 re-fetch → 顺序立即生效
        // 不必再让用户点"保存顺序"
        void saveProviderOrderOnly();
      }
    });
  });
  refreshOrderButtons();
}

function refreshOrderButtons() {
  const list = document.getElementById("provider-order-list");
  if (!list) return;
  const rows = list.querySelectorAll<HTMLElement>(".order-row");
  rows.forEach((row, i) => {
    const up = row.querySelector<HTMLButtonElement>('button[data-move="up"]');
    const down = row.querySelector<HTMLButtonElement>('button[data-move="down"]');
    if (up) up.disabled = i === 0;
    if (down) down.disabled = i === rows.length - 1;
  });
}

function readProviderOrder(): string[] {
  const list = document.getElementById("provider-order-list");
  if (!list) return [];
  const ids: string[] = [];
  list.querySelectorAll<HTMLElement>(".order-row").forEach((row) => {
    const id = row.dataset.id;
    if (id) ids.push(id);
  });
  return ids;
}

/// 只保存 provider_order（不触碰其它字段），让用户调完顺序后不必滚到底部
/// 点全局"保存"也能直接落盘 —— 浮窗顺序是用户最高频调的。
async function saveProviderOrderOnly() {
  try {
    const order = readProviderOrder();
    const existing = await invoke<AppConfig>("get_config");
    await invoke("save_config", {
      cfg: { ...existing, provider_order: order },
    });
    flash(`✓ 顺序已保存（${order.join(" → ")}）`);
  } catch (e) {
    flash(`✗ 保存顺序失败: ${e}`, true);
  }
}

function resetProviderOrder() {
  renderProviderOrder([]);  // 空 = builtin 注册表顺序
}

let flashTimer: number | null = null;
function flash(msg: string, isError = false) {
  const el = $("#flash") as HTMLElement;
  el.textContent = msg;
  el.style.color = isError ? "#f44336" : "#4caf50";
  if (flashTimer !== null) clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (el.textContent = ""), 5000);
}

function providerDisplay(p: ProviderId): string {
  switch (p) {
    case "minimax":    return "MiniMax";
    case "deepseek":   return "DeepSeek";
    case "xiaomimimo": return "Xiaomi MiMo";
    case "tavily":     return "Tavily";
  }
}

// ── 日志模块（设置面板查看应用运行日志） ──
//
// 后端：每次 provider 拉取失败（去重窗口 60s）写一条 LogEntry 到 LogStore，
//       设置面板手动 `get_recent_logs` 拉。
// 前端：打开面板时自动拉一次；点 "刷新" 再拉；点 "清空" 调 `clear_logs`。
//       不做 live push（避免 IPC 噪音 + 设置面板跟浮窗双源同步）。
//       最新消息在顶部（用户一打开就能看到最近错误，不用翻到底）。

interface LogEntry {
  ts: number;
  level: "info" | "warn" | "error";
  provider: string | null;
  kind: string | null;
  message: string;
}

async function loadLogs() {
  const list = document.getElementById("logs-list");
  const count = document.getElementById("logs-count");
  if (!list) return;
  try {
    const entries = await invoke<LogEntry[]>("get_recent_logs", { limit: 200 });
    const filter = currentLogFilter();
    const filtered = filter === "all" ? entries : entries.filter((e) => e.level === filter);
    renderLogs(filtered);
    // count 数字要反映"当前显示"而非"总条数"，跟筛选状态保持一致
    if (count) {
      count.textContent =
        filter === "all"
          ? `${entries.length} 条`
          : `${filtered.length} / ${entries.length} 条`;
    }
  } catch (e) {
    list.innerHTML = `<div class="logs-empty" style="color:#f44336">✗ 加载失败: ${escapeHtml(String(e))}</div>`;
    if (count) count.textContent = "";
    console.error("[logs] load failed", e);
  }
}

function renderLogs(entries: LogEntry[]) {
  const list = document.getElementById("logs-list");
  if (!list) return;
  if (entries.length === 0) {
    list.innerHTML = `<div class="logs-empty">— 当前筛选下暂无日志 —</div>`;
    return;
  }
  // HTML 转义避免 message 里的 < > & 弄坏 layout
  const esc = (s: string) =>
    s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
  // 后端按时间正序返回（oldest → newest）。用户要"最新在最前面" → 翻过来。
  // 复制后再 reverse，不修改入参（避免影响外面）。
  const reversed = [...entries].reverse();
  list.innerHTML = reversed
    .map((e) => {
      const t = formatLogTime(e.ts);
      return `<div class="log-row">
        <span class="log-time">${esc(t)}</span>
        <span class="log-level ${e.level}">${esc(e.level)}</span>
        <span class="log-provider">${esc(e.provider ?? "—")}</span>
        <span class="log-kind">${esc(e.kind ?? "—")}</span>
        <span class="log-msg">${esc(e.message)}</span>
      </div>`;
    })
    .join("");
  // 最新在顶部 → 滚到顶（用户打开面板第一眼就看到最新的）
  list.scrollTop = 0;
}

function formatLogTime(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

async function clearLogs() {
  if (!confirm("确认清空所有日志？")) return;
  try {
    await invoke("clear_logs");
    await loadLogs();
    flash("✓ 日志已清空");
  } catch (e) {
    flash(`✗ 清空失败: ${e}`, true);
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
async function copyLogs() {
  try {
    const entries = await invoke<LogEntry[]>("get_recent_logs", { limit: 200 });
    const filter = currentLogFilter();
    const filtered = filter === "all" ? entries : entries.filter((e) => e.level === filter);
    if (filtered.length === 0) {
      flash(`⚠ 当前筛选下没有日志可复制`, true);
      return;
    }
    const lines = filtered.map((e) => {
      const t = formatLogTime(e.ts);
      return `[${t}] ${e.level.toUpperCase().padEnd(5)} ${e.provider ?? "—"} | ${e.kind ?? "—"} | ${e.message}`;
    });
    const text = lines.join("\n");
    await navigator.clipboard.writeText(text);
    flash(`✓ 已复制 ${filtered.length} 条日志到剪贴板`);
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
    console.error("[logs] copy failed", e);
  }
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!));
}

// ── 自动更新面板 ──
//
// 从 TS 动态注入，不动 settings.html。包含：
//   - 当前版本
//   - "检查更新" 按钮
//   - 状态显示（最新 / 有可用 / 下载中 / 错误）
//   - 有可用更新时的"立即下载" / "下载并重启" 按钮

function setupUpdaterSection() {
  // 找到 "保存配置" 按钮所在 row，插一个新区块在它前面
  const saveRow = $("#save")?.closest(".row");
  if (!saveRow) return;

  const block = document.createElement("div");
  block.className = "row";
  block.id = "updater-section";
  block.innerHTML = `
    <h3 style="margin: 0 0 8px 0; font-size: 14px;">自动更新</h3>
    <div style="font-size: 12px; opacity: 0.7; margin-bottom: 8px;">
      当前版本：<span id="updater-current-version">—</span>
    </div>
    <div style="display: flex; gap: 8px; align-items: center; flex-wrap: wrap;">
      <button id="updater-check" class="primary">检查更新</button>
      <button id="updater-install" class="primary" style="display: none;">下载并安装</button>
      <button id="updater-relaunch" class="primary" style="display: none;">立即重启</button>
      <span id="updater-status" style="font-size: 12px; opacity: 0.8;"></span>
    </div>
    <div id="updater-notes" style="display: none; margin-top: 8px; font-size: 12px;
                                     padding: 8px; background: rgba(255,255,255,0.05);
                                     border-radius: 4px; white-space: pre-wrap;"></div>
  `;
  saveRow.parentElement?.insertBefore(block, saveRow);

  // 读当前版本
  invoke<string>("get_app_version")
    .then((v) => {
      const el = document.getElementById("updater-current-version");
      if (el) el.textContent = `v${v}`;
    })
    .catch(() => {});

  // 绑按钮
  document.getElementById("updater-check")?.addEventListener("click", () => {
    void doCheck();
  });
  document.getElementById("updater-install")?.addEventListener("click", () => {
    void doInstall();
  });
  document.getElementById("updater-relaunch")?.addEventListener("click", () => {
    relaunchApp().catch((e) => flash(`✗ 重启失败: ${e}`, true));
  });

  // 订阅状态
  onUpdateState(renderUpdaterState);
}

async function doCheck() {
  const btn = document.getElementById("updater-check") as HTMLButtonElement | null;
  if (btn) btn.disabled = true;
  try {
    await checkForUpdate(false);
  } finally {
    if (btn) btn.disabled = false;
  }
}

async function doInstall() {
  const installBtn = document.getElementById("updater-install") as HTMLButtonElement | null;
  const checkBtn = document.getElementById("updater-check") as HTMLButtonElement | null;
  if (installBtn) installBtn.disabled = true;
  if (checkBtn) checkBtn.disabled = true;
  try {
    const result = await downloadAndInstall();
    if (result.status === "ready") {
      // 显示"立即重启"按钮
    }
  } finally {
    if (installBtn) installBtn.disabled = false;
    if (checkBtn) checkBtn.disabled = false;
  }
}

function renderUpdaterState(s: UpdateState) {
  const status = document.getElementById("updater-status");
  const installBtn = document.getElementById("updater-install") as HTMLButtonElement | null;
  const relaunchBtn = document.getElementById("updater-relaunch") as HTMLButtonElement | null;
  const notes = document.getElementById("updater-notes");
  if (!status) return;

  switch (s.status) {
    case "checking":
      status.textContent = "检查中…";
      status.style.color = "";
      if (installBtn) installBtn.style.display = "none";
      if (relaunchBtn) relaunchBtn.style.display = "none";
      if (notes) notes.style.display = "none";
      break;
    case "up-to-date":
      status.textContent = "✓ 已是最新";
      status.style.color = "#4caf50";
      if (installBtn) installBtn.style.display = "none";
      if (relaunchBtn) relaunchBtn.style.display = "none";
      if (notes) notes.style.display = "none";
      break;
    case "available":
      status.textContent = `🎉 发现新版本 v${s.version}`;
      status.style.color = "#2196f3";
      if (installBtn) installBtn.style.display = "";
      if (relaunchBtn) relaunchBtn.style.display = "none";
      if (notes) {
        if (s.notes) {
          notes.textContent = s.notes;
          notes.style.display = "";
        } else {
          notes.style.display = "none";
        }
      }
      break;
    case "downloading":
      status.textContent =
        s.progress != null
          ? `下载中… ${(s.progress * 100).toFixed(0)}%`
          : "下载中…";
      status.style.color = "#ff9800";
      if (installBtn) installBtn.style.display = "none";
      if (relaunchBtn) relaunchBtn.style.display = "none";
      break;
    case "ready":
      status.textContent = "✓ 已下载完成，点击重启生效";
      status.style.color = "#4caf50";
      if (installBtn) installBtn.style.display = "none";
      if (relaunchBtn) relaunchBtn.style.display = "";
      break;
    case "error":
      status.textContent = `✗ ${s.error ?? "更新失败"}`;
      status.style.color = "#f44336";
      if (installBtn) installBtn.style.display = "none";
      if (relaunchBtn) relaunchBtn.style.display = "none";
      break;
    default:
      // idle / disabled —— 不动 UI
      break;
  }
}

// ── Tavily (Phase 1 新增) ────────────────────────────────────────
//
// Tavily 不是 Provider enum 的成员，只能走 id-based 新 API。

async function loadTavilyKeyStatus() {
  const has = await invoke<boolean>("has_source_credential", { id: "tavily" });
  const el = document.getElementById("api-key-status-tavily");
  if (el) {
    el.textContent = has ? "✓ 已保存到本机" : "未设置";
    el.className = `status ${has ? "ok" : ""}`;
  }
}

async function saveTavilyKey() {
  const input = document.getElementById("api-key-tavily") as HTMLInputElement | null;
  if (!input) return;
  const key = input.value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 Tavily API key", true);
    return;
  }
  try {
    await invoke("set_source_credential", { id: "tavily", value: key });
    input.value = "";
    await loadTavilyKeyStatus();
    flash("✓ Tavily key 已保存");
    await testConn();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

async function deleteTavilyKey() {
  if (!confirm("确认删除 Tavily 的 API key？")) return;
  await invoke("delete_source_credential", { id: "tavily" });
  await loadTavilyKeyStatus();
  flash("✓ Tavily key 已删除");
}

async function copyTavilyKey() {
  try {
    const key = await invoke<string | null>("get_source_credential", { id: "tavily" });
    if (!key) {
      flash("⚠ Tavily 未设置 key", true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash("✓ Tavily key 已复制到剪贴板");
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
  }
}

// ── 启动 ──

setupTabs();

$("#save")?.addEventListener("click", saveConfig);
$("#save-key-minimax")?.addEventListener("click", () => saveKey("minimax"));
$("#save-key-deepseek")?.addEventListener("click", () => saveKey("deepseek"));
$("#save-key-xiaomimimo")?.addEventListener("click", () => saveKey("xiaomimimo"));
$("#save-key-tavily")?.addEventListener("click", () => saveTavilyKey());
$("#del-key-minimax")?.addEventListener("click", () => deleteKey("minimax"));
$("#del-key-deepseek")?.addEventListener("click", () => deleteKey("deepseek"));
$("#del-key-xiaomimimo")?.addEventListener("click", () => deleteKey("xiaomimimo"));
$("#del-key-tavily")?.addEventListener("click", () => deleteTavilyKey());
$("#copy-key-minimax")?.addEventListener("click", () => copyKey("minimax"));
$("#copy-key-deepseek")?.addEventListener("click", () => copyKey("deepseek"));
$("#copy-key-xiaomimimo")?.addEventListener("click", () => copyKey("xiaomimimo"));
$("#copy-key-tavily")?.addEventListener("click", () => copyTavilyKey());
$("#save-cookie-xiaomimimo")?.addEventListener("click", () => saveCookie("xiaomimimo"));
$("#del-cookie-xiaomimimo")?.addEventListener("click", () => deleteCookie("xiaomimimo"));
$("#test")?.addEventListener("click", testConn);

$("#reset-floating")?.addEventListener("click", async () => {
  const btn = $("#reset-floating") as HTMLButtonElement;
  btn.disabled = true;
  try {
    await invoke("reset_floating_window");
    flash("✓ 浮窗已归位到主屏幕正中央");
  } catch (e) {
    flash(`✗ 归位失败: ${e}`, true);
  } finally {
    btn.disabled = false;
  }
});

// 置顶/置底模式：单选按钮变更即生效（不依赖"保存配置"按钮）
document.querySelectorAll<HTMLInputElement>('input[name="pin-mode"]').forEach((r) => {
  r.addEventListener("change", () => {
    if (!r.checked) return;
    const mode = r.value as FloatingPinMode;
    if (mode === "pin_top" || mode === "pin_bottom" || mode === "normal") {
      applyPinMode(mode);
    }
  });
});

// 省电模式 / 全屏自动隐藏：勾选即生效（独立 command，不必点"保存配置"）
$("#low-power-mode")?.addEventListener("change", async () => {
  const enabled = ($("#low-power-mode") as HTMLInputElement).checked;
  try {
    await invoke("set_low_power_mode", { enabled });
    flash(enabled ? "✓ 省电模式已开启" : "✓ 省电模式已关闭");
  } catch (e) {
    flash(`✗ 切换失败: ${e}`, true);
  }
});

$("#auto-hide-in-fullscreen")?.addEventListener("change", async () => {
  const enabled = ($("#auto-hide-in-fullscreen") as HTMLInputElement).checked;
  try {
    await invoke("set_auto_hide_in_fullscreen", { enabled });
    flash(enabled ? "✓ 全屏自动隐藏已开启（仅 macOS）" : "✓ 全屏自动隐藏已关闭");
  } catch (e) {
    flash(`✗ 切换失败: ${e}`, true);
  }
});

// ── 日志按钮绑定：放在 IIFE 外面、脚本顶层 ──
//
// 之前在 IIFE 里 await 别的初始化函数（如 loadConfig / loadKeyStatus），
// 一旦某个 invoke 抛错（网络/未知 provider/tauri command 改了签名），
// 后续的按钮绑定就不执行 —— 用户看到「点清空没反应」其实是 listener 没绑上。
// 把按钮绑在脚本顶层（同步），保证永远生效；init 失败也只是数据加载不到，
// 不会让"清空/刷新/复制"变成死的。
document.getElementById("logs-refresh")?.addEventListener("click", () => void loadLogs());
document.getElementById("logs-clear")?.addEventListener("click", () => void clearLogs());
document.getElementById("logs-copy")?.addEventListener("click", () => void copyLogs());
document.getElementById("logs-filter")?.addEventListener("change", () => void loadLogs());
document.getElementById("save-provider-order")?.addEventListener("click", () => void saveProviderOrderOnly());
document.getElementById("reset-provider-order")?.addEventListener("click", () => resetProviderOrder());

(async () => {
  try {
    await loadKeyStatus("minimax");
    await loadKeyStatus("deepseek");
    await loadKeyStatus("xiaomimimo");
    await loadTavilyKeyStatus();
    await loadCookieStatus("xiaomimimo");
    await loadConfig();
    setupUpdaterSection();
    await loadLogs();
  } catch (e) {
    console.error("[settings] init failed", e);
    flash(`✗ 初始化失败: ${e}`, true);
  }
})();
