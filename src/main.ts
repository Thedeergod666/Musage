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
import tavilyLogo from "./assets/tavily-logo.svg?url";
import zenmuxLogo from "./assets/zenmux-logo.svg?url";
import openrouterLogo from "./assets/openrouter-logo.png";
import kimiLogo from "./assets/kimi-logo.svg?url";
import zhipuLogo from "./assets/zhipu-logo.svg?url";
import zhipuEnLogo from "./assets/zhipu-en-logo.svg?url";
import "./styles.css";

/// 静态映射：provider id → 官网 logo + 显示名 + accent 色
/// logo 走 Vite `?url` import 拿到打包后的 URL
///
/// 智谱 GLM 两个区域共用 id "zhipu"，运行时根据后端返回的
/// source_display_name（"智谱 GLM" / "Z.ai"）切换 logo：
/// - CN → zhipuLogo（紫色渐变 + 智字）
/// - EN → zhipuEnLogo（z.ai 官方 logo SVG）
///
/// 加新 provider 时如果暂时没有 logo 文件，把 `logo` 留空字符串，
/// `updateCard` 会自动用首字母 + accent 色生成 data: URL fallback。
/// 等拿到真 logo 直接 `cp` 替换 SVG 文件即可。
const PROVIDER_META: Record<string, { name: string; logo: string; accent: string }> = {
  minimax: { name: "MiniMax", logo: minimaxLogo, accent: "#9b59ff" },
  deepseek: { name: "DeepSeek", logo: deepseekLogo, accent: "#4a90e2" },
  xiaomimimo: { name: "Xiaomi MiMo", logo: xiaomimimoLogo, accent: "#ff6a00" },
  tavily: { name: "Tavily", logo: tavilyLogo, accent: "#00d4a8" },
  zenmux: { name: "ZenMux", logo: zenmuxLogo, accent: "#9b59ff" },
  openrouter: { name: "OpenRouter", logo: openrouterLogo, accent: "#5ac8fa" },
  kimi: { name: "Kimi", logo: kimiLogo, accent: "#5ac8fa" },
  zhipu: { name: "智谱 GLM", logo: zhipuLogo, accent: "#7b61ff" },
  "Z.ai": { name: "Z.ai", logo: zhipuEnLogo, accent: "#2D2D2D" },
};

/// 没有 logo 文件时，用首字母 + accent 色生成 data: URL SVG。
/// 渲染成本几乎为 0（base64 inline），但保证浮窗一定有头像可显示。
function fallbackLogo(name: string, accent: string): string {
  const ch = name.trim().charAt(0).toUpperCase() || "?";
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 56 56">
    <rect width="56" height="56" rx="12" fill="${accent}"/>
    <text x="28" y="38" text-anchor="middle" font-family="-apple-system,BlinkMacSystemFont,'PingFang SC','Microsoft YaHei',sans-serif" font-size="30" font-weight="700" fill="#fff">${escapeXml(ch)}</text>
  </svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

function escapeXml(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!),
  );
}

type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

/// 浮窗渲染相关的用户偏好（从 config 拉到，config 变时由 init 里的
/// "musage://config-changed" 监听刷新）
interface RenderPrefs {
  tavilyConciseMode: boolean;
  /// ZenMux PAYG 模式：只显示余额行，隐藏 充值/奖励 细分行。默认 true。
  zenmuxPaygConciseMode: boolean;
}
let renderPrefs: RenderPrefs = {
  tavilyConciseMode: true,
  zenmuxPaygConciseMode: true,
};

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
  provider: "minimax" | "deepseek" | "xiaomimimo" | "tavily" | "zenmux" | "openrouter" | "kimi" | "zhipu";
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

/// 跟 `updateCard` 的瞬态错误分支完全对齐：瞬态错误 + 有 lastGood →
/// 视觉上跟 lastGood 一样（只翻红点 + 标 stale，rowsBox DOM 保持不动）。
/// 持久错误 / 没历史成功 → 渲染 .err-msg + .err-btn（高度显著不同）。
///
/// **契约**：必须跟 `updateCard` 的瞬态错误分支（`isTransientError(kind) && good`
/// 那块）保持严格一致 —— 任一处改了另一处要同步改，否则 contentFingerprint
/// 算出来的"可见结构"就跟实际 DOM 脱节 → 要么漏 fit 要么白 fit。
function effectiveSnap(p: ProviderSnapshot): ProviderSnapshot {
  if (isTransientError(p.error_kind) && lastGoodSnap.has(p.provider)) {
    return lastGoodSnap.get(p.provider)!;
  }
  return p;
}

/// 描述"用户看到的浮窗内容结构" —— 只看会改变 layout 的维度：
///   - provider / source_id 列表（决定几张卡）
///   - 每张卡是 ok 还是 err（错误态有 .err-msg / .err-btn 行）
///   - 渲染哪些 row（按 rowKey，不看 utilization 数字）
///
/// 利用率、倒计时文字、logo / name 变化都**不**计入 → 不触发 fit。
function contentFingerprint(snap: QuotaSnapshot): string {
  return snap.providers.map((p) => {
    const eff = effectiveSnap(p);
    const id = eff.source_id ?? eff.provider;
    const state = eff.success ? "ok" : `err:${eff.error_kind ?? "other"}`;
    const rows = rowsForRender(eff);
    return `${id}|${state}|${rows.length}|${rows.map((r) => rowKey(r)).join(",")}`;
  }).join(";");
}

/// 每个 provider 的"最后一次成功"快照。
/// 瞬态错误来时，浮窗用这份数据继续渲染 + dot 翻红。
/// 持久错误（且无历史成功）才走完整的错误 UI。
const lastGoodSnap = new Map<string, ProviderSnapshot>();

/// 上一次 fit-to-content 时的"可见结构"指纹。
/// 内容数据刷新（utilization / countdown）不改变这个值 → auto-resize 跳过，
/// 保留用户手动改的窗口高度。结构变化（卡片增删 / 新错误 / 行数变化）才
/// 重新 fit。详见 `contentFingerprint` + `autoResizeWindow`。
let lastFitFingerprint: string | null = null;

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

  // 真数据到达 → 清掉首屏的「加载中…」占位，避免它一直挂在 DOM 里
  // 跟真卡片叠在一起（之前 [H8] 状态下的 bug：4 张 provider 卡 + 1 张残留 err）
  const errPlaceholder = app.querySelector<HTMLElement>(".err");
  if (errPlaceholder) errPlaceholder.remove();

  // 1. 增量更新每张 provider 卡片
  const existingCards = new Map<string, HTMLElement>();
  app.querySelectorAll<HTMLElement>(".card[data-provider]").forEach((el) => {
    const key = el.dataset.provider;
    if (key) existingCards.set(key, el);
  });

  // 第一遍：确保所有 snap 里的 card 都存在 DOM（按 snap 顺序决定插入位置）
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

  // 第二遍：按 snap.providers 顺序把 DOM 卡片摆到正确位置。
  // 修复"调整顺序后浮窗不跟随"bug —— 上面第一遍只在新卡时按 snap 顺序插，
  // 已存在的卡只是 update 不动 DOM；用户在设置面板把 DeepSeek 排到第二位、
  // Xiaomi MiMo 排到第三位后，set_provider_order 重排 in-memory snapshot
  // 并 emit 新 snap，但卡片本身在 DOM 里的物理顺序还是旧顺序 → 浮窗看上去
  // "没动"。这里把每张 card 按 snap 顺序依次挪到 anchor 之后即可重排。
  // 注：footer 永远在最后（updateFoot 会 append 到 app 末尾），所以
  // anchor.nextSibling 不会跨过 footer 干扰卡片排布。
  let reorderAnchor: ChildNode | null = null;
  for (const p of snap.providers) {
    const id = p.source_id ?? p.provider;
    const card = app.querySelector<HTMLElement>(`.card[data-provider="${cssEscape(id)}"]`);
    if (!card) continue;
    if (reorderAnchor == null) {
      // 第一个：挪到 app 的最前
      if (card !== app.firstChild) {
        app.insertBefore(card, app.firstChild);
      }
    } else {
      // 后续：挪到 anchor 之后
      const desiredNext = reorderAnchor.nextSibling;
      if (card !== desiredNext) {
        reorderAnchor.parentNode?.insertBefore(card, desiredNext);
      }
    }
    reorderAnchor = card;
  }

  // 2. 底部 footer（始终只有 1 个）
  updateFoot(snap);

  startCountdown();
  // 改完 DOM 后 → 量内容高度，调 Rust 把浮窗 resize 到 fit-content。
  // autoResizeWindow 自己用 contentFingerprint 去重：utilization 刷新等
  // 数据变化不动窗口；只有卡片/行结构变了才 fit。
  void autoResizeWindow(snap);
}

/// 按 RenderPrefs 过滤 / 改写 rows（影响渲染前的数据，不动后端）
function rowsForRender(p: ProviderSnapshot): QuotaRow[] {
  const id = p.source_id ?? p.provider;
  if (id === "tavily" && renderPrefs.tavilyConciseMode) {
    // 简洁模式：只保留主指标行（"209/1000 credits" 那条），隐藏 5 个
    // endpoint 细分行（search/extract/crawl/map/research）。
    // 进度条保留在 rowLabel 下方，跟 MiniMax 5h/周 一致。
    //
    // 取首行即可 —— tavily.rs 的 parse() 永远把主指标行（label="Free tier"）
    // push 在最前，5 个 endpoint 细分在后。改成严格 "used+total 都有才保留"
    // 会把 "limit=null"（无限制套餐或某些 paid plan）的账号主行也过滤掉
    // 导致浮窗空卡片。
    return p.rows.length > 0 ? [p.rows[0]] : [];
  }
  if (id === "zenmux" && renderPrefs.zenmuxPaygConciseMode) {
    // ZenMux PAYG 简洁模式：只保留余额行，隐藏「充值 / 奖励」细分。
    // 检测：PAYG 模式第一行 remaining 字段非空且 utilization 为空；
    // subscription 模式第一行 utilization 非空（不受此 toggle 影响）。
    const main = p.rows.find((r) => r.remaining != null && r.utilization == null);
    return main ? [main] : p.rows;
  }
  return p.rows;
}

/// 自适应高度：把 #app 的实际内容高度发给 Rust，让浮窗 resize 上去。
///
/// **不能用 `document.documentElement.scrollHeight`** —— body 是
/// `height: 100%` + `overflow: hidden`，所以 documentElement.scrollHeight =
/// `max(body.clientHeight, body.contentHeight)`。设窗口到 N+1 后 body.clientHeight
/// 涨到 N+1，下一轮 scrollHeight 就变成 N+1，再 +1 = N+2 …… 每次 render 长 1px，
/// 几小时下来浮窗能涨几十像素。回归：[H5]、浮窗静置桌面越长越高的 bug。
///
/// 正确读法：`#app.scrollHeight` —— 这是 #app 内部所有卡片 + padding 的**自然**
/// 高度，不受窗口 clientHeight 干扰。设到这个值后窗口内刚好能容下所有内容，
/// 下一轮 scrollHeight 不变，达到稳态。
///
/// **不每次 render 都 fit**（修复"手动拖窗口被自动 fit 覆盖"）：
/// 算 `contentFingerprint(snap)`，跟 `lastFitFingerprint` 比，相同就跳过。
/// 数据刷新（utilization / countdown / logo 变化）不改变 fingerprint →
/// 保留用户手动尺寸；结构变化（新增/移除卡、新错误、行数变化）才重新 fit。
async function autoResizeWindow(snap: QuotaSnapshot) {
  await new Promise<void>((r) => requestAnimationFrame(() => r()));
  const appEl = document.getElementById("app");
  if (!appEl) return;

  // 内容结构没变（只是 utilization / countdown 刷新）→ 保留用户手动尺寸
  const fp = contentFingerprint(snap);
  if (fp === lastFitFingerprint) return;
  lastFitFingerprint = fp;

  const contentH = appEl.scrollHeight;
  // 已经在 1px 容差内就跳过，避免子像素抖动触发的 re-resize
  const currentH = window.innerHeight;
  if (Math.abs(currentH - contentH) <= 1) return;
  const target = Math.round(contentH);
  try {
    await invoke("resize_floating_window", { height: target });
  } catch (e) {
    console.debug("[floating] auto-resize 失败", e);
  }
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
      <div class="card-head-status">
        <span class="card-dot"></span>
      </div>
    </header>
    <div class="rows"></div>
  `;
  return card;
}

function updateCard(card: HTMLElement, p: ProviderSnapshot): void {
  const title = card.querySelector<HTMLElement>(".card-title")!;
  // Phase 1：用 source_id 路由（registry-driven），provider 字段保兼容
  const id = p.source_id ?? p.provider;
  // 智谱 GLM 用 source_display_name 二次路由：CN="智谱 GLM" / EN="Z.ai"
  // 让两张 logo（紫色渐变 vs z.ai 官方）按区域切换。
  const regionKey = (id === "zhipu" && p.source_display_name)
    ? p.source_display_name
    : id;
  const meta = PROVIDER_META[regionKey] ?? {
    name: p.source_display_name ?? id,
    logo: "",
    accent: "#888",
  };
  const logoSrc = meta.logo || fallbackLogo(meta.name, meta.accent);
  const logo = title.querySelector<HTMLImageElement>(".card-logo")!;
  const name = title.querySelector<HTMLElement>(".card-name")!;
  if (logo.src !== logoSrc) logo.src = logoSrc;
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

  // ZenMux PAYG 模式：第 1 行有 remaining 但无 utilization 时 → 加 class
  // 让 CSS 把所有行 (含充值/奖励) 统一成余额样式（17px、白色、hover 不变色）。
  // subscription 模式第 1 行有 utilization → 不加 class，保留 MiniMax-style。
  if (id === "zenmux" && p.success) {
    const isPayg = p.rows[0]?.remaining != null && p.rows[0]?.utilization == null;
    card.classList.toggle("zenmux-payg", isPayg);
  } else {
    card.classList.remove("zenmux-payg");
  }

  if (!p.success) {
    const kind = p.error_kind ?? "other";
    const good = lastGoodSnap.get(p.provider);

    // ── 瞬态错误（网络抖动 / 限流 / 服务端错误）+ 之前有过成功数据 ──
    // **不**碰 rowsBox 的 DOM（最后一次成功渲染留下的用量数据原封不动），
    // 只翻红点 + 标 stale。具体报错已由后端写进 LogStore，浮窗不再展示。
    // ⚠️ 这条分支的判定条件跟 `effectiveSnap`（上面）必须严格保持一致，
    // 否则 contentFingerprint 算出来的"可见结构"就跟实际 DOM 脱节。
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
    // H8 修复：err-label 加在 .card-head-status 里（紧贴 .card-dot 的右侧），
    // 而不是直接 append 到 .card-head —— 后者会触发 flex space-between
    // 把 dot 推到中间、label 占右端，dot 失去"右上角"位置。
    // 现在 head 右侧用 .card-head-status 包裹 [dot, label]，dot 永远在
    // 包裹最左 = head 右上区域左侧，label 紧跟其右。
    const headStatus = card.querySelector<HTMLElement>(".card-head-status")!;
    let headLabel = headStatus.querySelector<HTMLElement>(".err-label");
    if (!headLabel) {
      headLabel = document.createElement("span");
      headLabel.className = "err-label";
      headStatus.appendChild(headLabel);
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
  // 清掉 err-label（H8 修复后，err-label 在 .card-head-status 里，querySelector 仍能找）
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

  // 按用户偏好过滤行（Tavily 简洁模式等）—— 跟下面 diff 逻辑透明衔接
  const rows = rowsForRender(p);

  let rowAnchor: ChildNode | null = null;
  for (const r of rows) {
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
  // Phase 1: credits 行（MiniMax 风格：大 % + used/total 副文字 + 进度条 + row-foot）
  if (r.used != null && r.total != null) {
    const util = r.utilization ?? (r.used / r.total) * 100;
    const cls = colorClass(util);
    // 左侧：used/total（如 "253/1000"）
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = `${Math.round(r.used)}/${Math.round(r.total)}`;
    // 右侧：大字 utilization %（如 "25%"）
    const pct = rowEl.querySelector<HTMLElement>(".pct")!;
    pct.textContent = formatPct(util);
    pct.className = `pct ${cls}`;
    // 进度条
    const bar = rowEl.querySelector<HTMLElement>(".bar-fill")!;
    bar.className = `bar-fill ${cls}`;
    bar.style.width = `${barWidth(util)}%`;
    // row-foot：plan_name + 月重置倒计时
    if (r.resets_at) {
      rowEl.dataset.resetsAt = String(r.resets_at);
      // Tavily 月重置：用 "月重置" 前缀，不复用 label（"Free tier"）
      rowEl.dataset.resetsPrefix = "月重置";
    } else {
      delete rowEl.dataset.resetsAt;
      delete rowEl.dataset.resetsPrefix;
    }
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
    // 优先用 data-resets-prefix（Tavily 用 "月重置"），否则用 label + " 重置"
    const prefix = row.dataset.resetsPrefix
      ?? (row.querySelector<HTMLElement>(".row-label > span:first-child")?.textContent ?? "") + " 重置";
    foot.textContent = formatResetWithCountdown(ms, prefix);
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
  // 剩余 > 1 天 → 显示日期 + "(N天)"，数日子比读 321h30m 直观
  //（典型：Xiaomi 套餐按月重置；MiniMax 周限额是滚动 7 天，跨日也用日期更清楚）
  // 剩余 < 1 天 → 显示时分 + "(Nh Mm)"，精度需要到分钟
  // 跨日边界：> 1 day 用日期，< 1 day 用时分 —— 跟用户对"剩多久"的认知一致
  // 日期走本地时区，跟 getHours()/getMinutes() 一致（用户看的是自己时区里的时间）
  const days = Math.floor(remainMs / 86400000);
  if (remainMs <= 0) {
    const label = `${dt.getMonth() + 1}-${dt.getDate()}`;
    return `${prefix} ${label}（已重置）`;
  }
  if (days >= 1) {
    const label = `${dt.getMonth() + 1}-${dt.getDate()}`;
    return `${prefix} ${label}（${days}天）`;
  }
  // < 1 天：显示时分 + "Nh Mm" 倒计时
  const time = `${pad2(dt.getHours())}:${pad2(dt.getMinutes())}`;
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

/// CSS attr 选择器需要转义特殊字符（id 里可能有 "." / ":" 等）。
/// 浏览器原生 CSS.escape 2021+ 才有，老 WKWebView 兜底手写最小集。
function cssEscape(s: string): string {
  if (typeof (CSS as any).escape === "function") return (CSS as any).escape(s);
  return s.replace(/([!"#$%&'()*+,./:;<=>?@[\]^`{|}~])/g, "\\$1");
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
  let unlistenCfg: UnlistenFn | null = null;
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
      tavily_concise_mode?: boolean;
      zenmux_payg_concise_mode?: boolean;
    }>("get_config");
    pinMode = cfg.floating_pin_mode ?? "pin_top";
    setLowPowerAttr(cfg.low_power_mode ?? false);
    renderPrefs = {
      tavilyConciseMode: cfg.tavily_concise_mode ?? true,
      zenmuxPaygConciseMode: cfg.zenmux_payg_concise_mode ?? true,
    };
  } catch (e) {
    console.error("读 config 失败", e);
  }
  setupHoverRaise(pinMode);

  // 配置变化时（设置面板改 Tavily / ZenMux 简洁模式等）→ 重新拉 config + snapshot
  // 后端 save_config 已经 emit `musage://config-changed`。
  listen("musage://config-changed", async () => {
    try {
      const cfg = await invoke<{
        tavily_concise_mode?: boolean;
        zenmux_payg_concise_mode?: boolean;
      }>("get_config");
      renderPrefs = {
        tavilyConciseMode: cfg.tavily_concise_mode ?? true,
        zenmuxPaygConciseMode: cfg.zenmux_payg_concise_mode ?? true,
      };
      const snap = await invoke<QuotaSnapshot>("get_snapshot");
      if (snap.providers.length > 0) render(snap);
    } catch (e) {
      console.error("[floating] 重新读 config 失败", e);
    }
  }).then((fn) => (unlistenCfg = fn));

  // 设置面板改了模式时，重新挂/摘 hover 监听。
  // （设置面板那边调 set_floating_pin_mode 会 emit 这个事件）
  listen<FloatingPinMode>("musage://pin-mode-changed", (e) => {
    // 清掉旧的监听再装新的（幂等）
    document.body.removeEventListener("mouseenter", hoverEnterHandler);
    document.body.removeEventListener("mouseleave", hoverLeaveHandler);
    setupHoverRaise(e.payload);
  });

  window.addEventListener("beforeunload", () => {
    if (unlisten) unlisten();
    if (unlistenHover) unlistenHover();
    if (unlistenLowPower) unlistenLowPower();
    if (unlistenCfg) unlistenCfg();
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
  // **挂在 document.body 而不是 document**：Chromium 的 mouseenter/mouseleave
  // 在 `document` 这个非元素对象上对"鼠标离开窗口"的判定不可靠 —— mouseleave
  // 不冒泡，只对带 bounding box 的真实元素稳定触发。document.body 是整个窗口
  // 的根元素（CSS 已经 margin:0 + background:transparent 把 body 撑满），
  // 鼠标移出浮窗时 mouseleave 100% 在它上面触发。和上面 setHoverAttr 的
  // CSS hover 监听用同一个 target，行为一致。
  //
  // 之前用 document 时，Win 上"hover 临时置顶后鼠标移开浮窗，always-on-top
  // 一直留着"的 bug 就是 mouseleave 没触发导致的。
  document.body.addEventListener("mouseenter", hoverEnterHandler);
  document.body.addEventListener("mouseleave", hoverLeaveHandler);
}

init();
