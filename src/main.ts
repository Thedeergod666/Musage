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
import { checkForUpdate, onUpdateState } from "./updater";
import minimaxLogo from "./assets/minimax-logo.png";
import deepseekLogo from "./assets/deepseek-icon.png";
import xiaomimimoLogo from "./assets/xiaomimimo-logo.png";
import "./styles.css";

/// 静态映射：provider id → 官网 logo + 显示名
/// logo 走 Vite `?url` import 拿到打包后的 URL
/// Tavily 暂时没有本地 logo（占位 PNG 没生成），用空字符串让 UI 显示一个 fallback
const PROVIDER_META: Record<string, { name: string; logo: string }> = {
  minimax: { name: "MiniMax", logo: minimaxLogo },
  deepseek: { name: "DeepSeek", logo: deepseekLogo },
  xiaomimimo: { name: "Xiaomi MiMo", logo: xiaomimimoLogo },
  tavily: { name: "Tavily", logo: "" },
};

type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

// ── 类型（必须和 src-tauri/src/providers/mod.rs 对齐）──

interface QuotaRow {
  label: string;
  utilization: number | null;
  remaining: number | null;
  /** Phase 1 新增：Tavily credits 用了多少 */
  used?: number | null;
  /** Phase 1 新增：Tavily credits 总量 */
  total?: number | null;
  resets_at: number | null;
  unit: string | null;
}

interface ProviderSnapshot {
  /** 兼容字段（minimax / deepseek / xiaomimimo）。新代码用 source_id。 */
  provider: "minimax" | "deepseek" | "xiaomimimo";
  /** Phase 1 新增。 */
  source_id?: string | null;
  source_display_name?: string | null;
  plan_name?: string | null;
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

/// 瞬态错误：网络抖动 / 限流 / 服务端错误 → 浮窗只翻红点 + 写日志，
/// **不**覆盖 rows（保留最后一次成功的数据让用户能继续看用量）。
/// 其它错误（未配 key / key 错 / 解析失败 / schema 未知）是**持久**错误，
/// 仍走老 UI 提示用户去操作。
const TRANSIENT_ERROR_KINDS = new Set(["network", "rate_limited", "server_error"]);

function isTransientError(kind: string | null | undefined): boolean {
  return kind != null && TRANSIENT_ERROR_KINDS.has(kind);
}

/// 每个 provider 的"最后一次成功"快照。
/// 瞬态错误来时，浮窗用这份数据继续渲染 + dot 翻红。
/// 持久错误（且无历史成功）才走完整的错误 UI。
const lastGoodSnap = new Map<string, ProviderSnapshot>();

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
    // Phase 1：用 source_id 路由（registry-driven），provider 字段保兼容
    const id = p.source_id ?? p.provider;
    let card = existingCards.get(id);
    if (card) {
      existingCards.delete(id);
    } else {
      card = buildCardSkeleton(id);
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
  // Phase 1：用 source_id 路由（registry-driven），provider 字段保兼容
  const id = p.source_id ?? p.provider;
  const meta = PROVIDER_META[id] ?? { name: p.source_display_name ?? id, logo: "" };
  const logo = title.querySelector<HTMLImageElement>(".card-logo")!;
  const name = title.querySelector<HTMLElement>(".card-name")!;
  if (logo.src !== meta.logo) logo.src = meta.logo;
  logo.alt = meta.name;
  name.textContent = meta.name;

  const dot = card.querySelector<HTMLElement>(".card-dot")!;
  dot.className = `card-dot ${dotClass(p)}`;

  const rowsBox = card.querySelector<HTMLElement>(".rows")!;

  // 成功 → 记录到 lastGood 备用（瞬态错误时复用这份数据继续渲染）
  if (p.success) {
    lastGoodSnap.set(p.provider, p);
    card.dataset.stale = ""; // 清 stale 标记（即使之前是 stale）
  }

  if (!p.success) {
    const kind = p.error_kind ?? "other";
    const good = lastGoodSnap.get(p.provider);

    // ── 瞬态错误（网络抖动 / 限流 / 服务端错误）+ 之前有过成功数据 ──
    // **不**碰 rowsBox 的 DOM（最后一次成功渲染留下的用量数据原封不动），
    // 只翻红点 + 标 stale。具体报错已由后端写进 LogStore，浮窗不再展示。
    if (isTransientError(kind) && good) {
      card.classList.remove("err-card");
      card.classList.forEach((c) => {
        if (c.startsWith("err-") && c !== "err-card") card.classList.remove(c);
      });
      const headLabel = card.querySelector<HTMLElement>(".err-label");
      if (headLabel) headLabel.remove();
      card.dataset.stale = "1";
      return;
    }

    // ── 持久错误 / 还没拉到过任何成功数据：走老 UI ──
    const label = errorKindLabel(kind);
    const needsSettings = kind === "unconfigured_key" || kind === "auth_failed";
    const settingsBtn = needsSettings
      ? `<button class="err-btn open-settings">打开设置</button>`
      : "";
    const schemaHint =
      kind === "schema_unknown"
        ? `<div class="hint">→ 设置面板 · Schema overrides 加新字段名</div>`
        : "";
    card.classList.add("err-card", `err-${kind}`);
    const head = card.querySelector<HTMLElement>(".card-head")!;
    let headLabel = head.querySelector<HTMLElement>(".err-label");
    if (!headLabel) {
      headLabel = document.createElement("span");
      headLabel.className = "err-label";
      head.appendChild(headLabel);
    }
    headLabel.textContent = label;
    rowsBox.innerHTML = `
      <div class="err-msg">${escapeHtml(p.error ?? "未知错误")}</div>
      ${settingsBtn}
      ${schemaHint}
    `;
    card.dataset.stale = "";
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

  // H7 修复：错误态 → 成功态过渡时清掉 rowsBox 里残留的错误 UI。
  // error path 用 `rowsBox.innerHTML = ...` 写过 `.err-msg` / `.err-btn` / `.hint`，
  // 成功 path 走 diff 只增删 `.row[data-row-key]`，不会碰这些孤儿元素。
  // 结果：用户导入 key 后，rowsBox 既有新行数据又有旧的"未配置凭据 + 打开设置"，
  // 重启才好（重启后 buildCardSkeleton 给出空 rowsBox）。
  // 修法：检测到残留错误元素就清空整个 rowsBox，让下面的 diff 重新填。
  if (rowsBox.querySelector(".err-msg, .err-btn, .hint")) {
    rowsBox.innerHTML = "";
  }

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
  // Phase 1: Tavily 走"used/total"组合（"150/1000 credits"），优先于 remaining/utilization
  if (r.used != null && r.total != null) return `credits:${r.label}`;
  if (r.used != null) return `used:${r.label}`;
  if (r.utilization != null) return `pct:${r.label}`;
  if (r.remaining != null) return `amt:${r.label}`;
  return "unknown";
}

function buildRowSkeleton(r: QuotaRow): HTMLElement {
  const row = document.createElement("div");
  row.className = "row";
  if (r.used != null && r.total != null) {
    // Phase 1: credits 行（"150/1000 credits"） + 进度条
    row.classList.add("credits-row");
    row.innerHTML = `
      <div class="row-label">
        <span></span>
        <span class="pct credits"></span>
      </div>
      <div class="bar"><div class="bar-fill"></div></div>
      <div class="row-foot"></div>
    `;
  } else if (r.used != null) {
    // 只有 used 没有 total（无限制套餐）
    row.classList.add("credits-row", "no-total");
    row.innerHTML = `
      <div class="row-label">
        <span></span>
        <span class="pct credits"></span>
      </div>
    `;
  } else if (r.utilization != null) {
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
  }
  return row;
}

function updateRow(rowEl: HTMLElement, r: QuotaRow): void {
  // Phase 1: credits 行（"150/1000 credits"）
  if (r.used != null && r.total != null) {
    const cls = colorClass(r.utilization ?? (r.used / r.total) * 100);
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = r.label;
    const pct = rowEl.querySelector<HTMLElement>(".pct")!;
    pct.textContent = `${Math.round(r.used)}/${Math.round(r.total)} ${escapeHtml(r.unit ?? "")}`;
    pct.className = `pct credits ${cls}`;
    const bar = rowEl.querySelector<HTMLElement>(".bar-fill")!;
    bar.className = `bar-fill ${cls}`;
    bar.style.width = `${barWidth(r.utilization)}%`;
    if (r.resets_at) rowEl.dataset.resetsAt = String(r.resets_at);
    else delete rowEl.dataset.resetsAt;
  } else if (r.used != null) {
    // 只有 used 没有 total
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = r.label;
    const pct = rowEl.querySelector<HTMLElement>(".pct")!;
    pct.textContent = `${Math.round(r.used)} ${escapeHtml(r.unit ?? "")}`;
    pct.className = "pct credits";
  } else if (r.utilization != null) {
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
  }
}

// ── Footer ──

function updateFoot(snap: QuotaSnapshot) {
  let foot = app.querySelector<HTMLElement>(".foot");
  // H9 修复：用 error_kind 枚举判断，不再依赖中文错误串（前者 Rust 改文案不会破）
  const anyUnconfigured = snap.providers.some(
    (p) => !p.success && p.error_kind === "unconfigured_key",
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
  // 4 档离散色 —— 整条 bar + 文字单色，**不**是位置性渐变。
  //   < 50%    → ok     (绿，安全)
  //   50-70%   → cyan   (青，过半提醒)
  //   70-88%   → warn   (黄，警告)
  //   >= 88%   → alert  (红，告警)
  if (util < 50) return "ok";
  if (util < 70) return "cyan";
  if (util < 88) return "warn";
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

  // ── 省电模式同步：body[data-low-power] 让 CSS 关掉 backdrop-filter + transition ──
  let unlistenLowPower: UnlistenFn | null = null;
  const setLowPowerAttr = (on: boolean) => {
    if (on) document.body.dataset.lowPower = "1";
    else delete document.body.dataset.lowPower;
  };
  listen<boolean>("musage://low-power-mode-changed", (e) => {
    setLowPowerAttr(e.payload);
  }).then((fn) => (unlistenLowPower = fn));

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

  // ── 启动 5s 后静默检查更新（不弹窗、不抢焦点） ──
  // 延迟是为了让首屏数据先到位，不要跟初始拉取抢资源
  setTimeout(() => {
    checkForUpdate(/* silent */ true)
      .then((s) => {
        if (s.status === "available" && s.version) {
          console.info(`[updater] 新版本 v${s.version} 可用，请到设置面板查看`);
        } else if (s.status === "error") {
          // 静默检查时错误只 log，不打扰用户（离线/没配 pubkey 都会触发）
          console.debug(`[updater] 静默检查失败: ${s.error}`);
        }
      })
      .catch((e) => console.debug("[updater] 静默检查异常", e));
  }, 5000);

  // ── 订阅 updater 状态：托盘气泡 / 设置面板 banner 可以挂这里 ──
  onUpdateState((s) => {
    if (s.status === "error") {
      console.warn(`[updater] ${s.error}`);
    }
  });

  // 读取用户选的置顶/置底模式 + 省电模式初始状态。
  // PinBottom 模式下，监听 mouseenter/mouseleave 让后端临时切到 always-on-top。
  let pinMode: FloatingPinMode = "pin_top";
  try {
    const cfg = await invoke<{
      floating_pin_mode?: FloatingPinMode;
      low_power_mode?: boolean;
    }>("get_config");
    pinMode = cfg.floating_pin_mode ?? "pin_top";
    setLowPowerAttr(cfg.low_power_mode ?? false);
  } catch (e) {
    console.error("读 config 失败", e);
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
    if (unlistenLowPower) unlistenLowPower();
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
