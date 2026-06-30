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
import xiaomimimoLogo from "./assets/xiaomimimo-logo.png";
import tavilyLogo from "./assets/tavily-logo.svg?url";
import zenmuxLogo from "./assets/zenmux-logo.svg?url";
import openrouterLogo from "./assets/openrouter-logo.png";
import kimiLogo from "./assets/kimi-logo.svg?url";
import zhipuLogo from "./assets/zhipu-logo.svg?url";
import zhipuEnLogo from "./assets/zhipu-en-logo.svg?url";
import stepfunLogo from "./assets/stepfun-logo.svg?url";
import siliconflowLogo from "./assets/siliconflow-logo.svg?url";
import claudeLogo from "./assets/claude-logo.svg?url";
import "./styles.css";

/// 静态映射：provider id → 官网 logo + 显示名 + accent 色
/// logo 走 Vite `?url` import 拿到打包后的 URL
///
/// 智谱 GLM 两个区域共用 id "zhipu"，运行时根据后端返回的
/// source_display_name（"智谱 GLM" / "Z.ai"）切换 logo：
/// - CN → zhipuLogo（紫色渐变 + 智字）
/// - EN → zhipuEnLogo（z.ai 官方 logo SVG）
///
/// 显示名走 t(\`provider.${id}.name\`) —— 跟 settings 那边用同一个 i18n key，
/// P1 frontend 阶段合一过 [src/settings/logos.ts] 和 [src/settings/utils.ts:providerDisplay]
/// 三处 dup。settings panel 加新 provider 时只改 [src/i18n/{en,zh-CN}.json] 即可。
///
/// 加新 provider 时如果暂时没有 logo 文件，把 `logo` 留空字符串，
/// `updateCard` 会自动用首字母 + accent 色生成 data: URL fallback。
/// 构建 provider 元数据表。必须在 initLocale() 之后调用（t() 需要 dict 已加载）。
/// locale 切换时重新构建 + 刷新卡片名称。
function buildProviderMeta(): Record<string, { name: string; logo: string; accent: string }> {
  return {
    minimax: { name: t("provider.minimax.name"), logo: minimaxLogo, accent: "#9b59ff" },
    deepseek: { name: t("provider.deepseek.name"), logo: deepseekLogo, accent: "#4a90e2" },
    xiaomimimo: { name: t("provider.xiaomimimo.name"), logo: xiaomimimoLogo, accent: "#ff6a00" },
    tavily: { name: t("provider.tavily.name"), logo: tavilyLogo, accent: "#00d4a8" },
    zenmux: { name: t("provider.zenmux.name"), logo: zenmuxLogo, accent: "#9b59ff" },
    openrouter: { name: t("provider.openrouter.name"), logo: openrouterLogo, accent: "#5ac8fa" },
    kimi: { name: t("provider.kimi.name"), logo: kimiLogo, accent: "#5ac8fa" },
    zhipu: { name: t("provider.zhipu_cn.name"), logo: zhipuLogo, accent: "#7b61ff" },
    "Z.ai": { name: t("provider.zhipu_en.name"), logo: zhipuEnLogo, accent: "#2D2D2D" },
    stepfun: { name: t("provider.stepfun.name"), logo: stepfunLogo, accent: "#6366f1" },
    siliconflow: { name: t("provider.siliconflow.name"), logo: siliconflowLogo, accent: "#ff6b35" },
    claude_official: { name: t("provider.claude_official.name"), logo: claudeLogo, accent: "#d97706" },
  };
}

/// 当前 locale 的 provider 元数据缓存。initLocale 后 + locale 变化时重建。
let PROVIDER_META = {} as Record<string, { name: string; logo: string; accent: string }>;

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

/// v0.2.1 commit 10: 浮窗就地 mini flash 反馈(复制成功等)。
/// 不导入 settings/utils.ts 的 flash —— 跨模块会拖进 setCurrentKnownIds /
/// currentProviderOrder 等 settings-only 状态,污染浮窗 runtime。
/// 自己写一个最小实现,setTimeout 3 秒淡出。复用浮窗 .card 的 err-card 样式。
let miniFlashTimer: ReturnType<typeof setTimeout> | null = null;
function showMiniFlash(msg: string): void {
  const root = document.getElementById("app");
  if (!root) return;
  let el = root.querySelector<HTMLElement>(".mini-flash");
  if (!el) {
    el = document.createElement("div");
    el.className = "mini-flash";
    root.appendChild(el);
  }
  el.textContent = msg;
  el.classList.add("visible");
  if (miniFlashTimer) clearTimeout(miniFlashTimer);
  miniFlashTimer = setTimeout(() => {
    el?.classList.remove("visible");
  }, 3000);
}

type FloatingPinMode = "pin_top" | "pin_bottom" | "normal";

/// 浮窗渲染相关的用户偏好（从 config 拉到，config 变时由 init 里的
/// "musage://config-changed" 监听刷新）
interface RenderPrefs {
  tavilyConciseMode: boolean;
  /// ZenMux PAYG 模式：只显示余额行，隐藏 充值/奖励 细分行。默认 true。
  zenmuxPaygConciseMode: boolean;
  /// 4 档色阈值分界 [t0, t1, t2]（默认 [50, 70, 88]）。由 colorClass 使用。
  colorThresholds: [number, number, number];
  /// 钱包/余额行的低额高亮阈值。null = 关闭（保持默认蓝色）。
  walletAlertThreshold: number | null;
  /// 用户自定义 4 档色：{ok, cyan, warn, alert} → "#RRGGBB"。空对象 = 走 iOS 默认。
  /// 改这个会触发 applyColorOverrides() 把 CSS 变量写进 #app。
  colorOverrides: Record<string, string>;
  /// 浮窗底部提示行显隐。默认 false（不显示）。
  showFooterHint: boolean;
}
let renderPrefs: RenderPrefs = {
  tavilyConciseMode: true,
  zenmuxPaygConciseMode: true,
  colorThresholds: [50, 70, 88],
  walletAlertThreshold: null,
  colorOverrides: {},
  showFooterHint: false,
};

/// 把 renderPrefs.colorOverrides 应用到 #app 的 inline CSS 变量。
/// 没 override 的键 → 走 iOS 默认（= 删掉 inline var，CSS 自己 fallback）。
///
/// 触发时机：
/// 1. init() 首次读 config 之后
/// 2. config-changed 事件之后（用户在设置面板改了色 / 点了"全部重置"）
///
/// 实现方式：直接 `app.style.setProperty('--c-data-ok', '#xxxxxx')`。
/// inline style 优先级 > 任何 stylesheet 规则，所以一定能盖过。
function applyColorOverrides(): void {
  for (const key of ["ok", "cyan", "warn", "alert"] as const) {
    const user = renderPrefs.colorOverrides[key];
    if (user) {
      app.style.setProperty(`--c-data-${key}`, user);
    } else {
      app.style.removeProperty(`--c-data-${key}`);
    }
  }
}

import { t, initLocale, onLocaleChange, setLocale, getLocale } from "./i18n";



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
  /** 行的语义分类（与 locale 解耦，**L7 fix 2026-06-19**）。
   *  rowKey 优先用这个做 DOM 稳定 key，避免切 locale 后 key 变化导致全量重建。 */
  kind?:
    | "five_hour"
    | "weekly"
    | "plan"
    | "compensation"
    | "monthly_total"
    | null;
}

interface ProviderSnapshot {
  /** 兼容字段（minimax / deepseek / xiaomimimo）。新代码用 source_id。
   * **PR 3** 起改成 string（用户自定义 source 的 id 是 `custom_<uuid>`）。 */
  provider: string;
  /** Phase 1 新增。 */
  source_id?: string | null;
  source_display_name?: string | null;
  plan_name?: string | null;
  /** PR 3 新增：CustomSource 透传 display_name，避免前端用 id 走 PROVIDER_META 漏匹配 */
  display_name?: string | null;
  /** PR 3 新增：CustomSource 透传 accent 色，浮窗 first-letter fallback 用 */
  accent?: string | null;
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
  /** 下次自动 fetch 的 epoch ms (2026-06-17 commit 加)。
   *  浮窗错误卡片用这个显示 "Next auto-retry in ~Xm"。 */
  next_fetch_at?: number | null;
  raw?: unknown;
  is_healthy: boolean;
  /** v0.2.1 commit 3：多 instance 唯一标识。
   *  - 内置 provider 第 1 份 = `"minimax"`(同 `id()`)
   *  - 内置 provider 副本 = `"minimax#2"` / `"minimax#3"` ...
   *  - New API 中转站 = `"custom_<uuid>"`(UUID 已唯一,不带 #N)
   *  老 snapshot (v0.2.0 落 logstore) 没这个字段,fallback 到 source_id。 */
  unique_id?: string | null;
  /** **L8 fix（2026-06-19）**：true = 这个 snapshot 是 placeholder（乐观 emit
   *  给浮窗的临时态），不是真实 fetch 结果。浮窗应跳过"打开设置"按钮渲染，
   *  避免 2-5s 真实 fetch 完成前的闪烁。None / false = 真实快照，正常渲染。 */
  transient?: boolean | null;
}

interface QuotaSnapshot {
  providers: ProviderSnapshot[];
  fetched_at: number | null;
}

const app = document.getElementById("app")!;
let countdownTimer: number | null = null;
/// 最后一次 render 的 snapshot —— locale 变化时用来重新渲染。
let lastRenderedSnap: QuotaSnapshot | null = null;

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
/// H7 fix: lastGoodSnap key 从 p.provider 改为 (p.source_id ?? p.provider)，
/// 与 render 主路径的 id 路由(line 290/483/552)保持一致。
/// v0.2 前 CustomSource 的 p.provider 全是 "minimax" 占位, 多个 CustomSource
/// 共享同一 key → set() 互相覆盖, get() 拿到错的 snapshot。
/// v0.2 删 enum 后 p.provider = "minimax" (历史残留), p.source_id 才是真 id。
/// v0.2.1 commit 3: p.unique_id 优先 —— 多 instance 时返 "minimax#2" / "custom_<uuid>",
/// p.source_id 是 base id (多 instance 撞),p.provider 是 v0.1 兼容字段。fallback 链
/// unique_id → source_id → provider,确保老 snapshot (没 unique_id 字段) 仍能渲染。
function snapKey(p: ProviderSnapshot): string {
  return (p.unique_id ?? p.source_id ?? p.provider) as string;
}

function effectiveSnap(p: ProviderSnapshot): ProviderSnapshot {
  const entry = lastGoodSnap.get(snapKey(p));
  if (isTransientError(p.error_kind) && entry && (Date.now() - entry.at < LAST_GOOD_TTL_MS)) {
    return entry.snap;
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
  const providers = snap.providers.map((p) => {
    const eff = effectiveSnap(p);
    const id = eff.source_id ?? eff.provider;
    const state = eff.success ? "ok" : `err:${eff.error_kind ?? "other"}`;
    const rows = rowsForRender(eff);
    return `${id}|${state}|${rows.length}|${rows.map((r, i) => rowKey(id, i, r)).join(",")}`;
  }).join(";");
  // footer 显隐也影响内容高度，加入 fingerprint 确保切换时触发 fit
  return `fh:${renderPrefs.showFooterHint ? 1 : 0};${providers}`;
}

/// 每个 provider 的"最后一次成功"快照 + 记录时间。
/// 瞬态错误来时，浮窗用这份数据继续渲染 + dot 翻红。
/// 持久错误（且无历史成功）才走完整的错误 UI。
///
/// TTL：超过 5 分钟的缓存视为过期——不再用于瞬态错误兜底，改为显示错误 UI。
/// 避免退避期间浮窗一直卡在过期数据上（用户看到"962"但实际已经是 1000 了）。
const LAST_GOOD_TTL_MS = 5 * 60 * 1000;
const lastGoodSnap = new Map<string, { snap: ProviderSnapshot; at: number }>();

/// 上一次 fit-to-content 时的"可见结构"指纹。
/// 内容数据刷新（utilization / countdown）不改变这个值 → auto-resize 跳过，
/// 保留用户手动改的窗口高度。结构变化（卡片增删 / 新错误 / 行数变化）才
/// 重新 fit。详见 `contentFingerprint` + `autoResizeWindow`。
let lastFitFingerprint: string | null = null;

/**
 * ErrorKind 简短 label —— P1 错误分类重构后走 i18n。
 * 8 个 key 跟 Rust 端 `ErrorKind::as_str()`（snake_case）严格对齐，
 * 后端改 ErrorKind 变体时这里要同步。缺失 → dev mode console 报警。
 */
function errorKindLabel(k: string): string {
  return t(`error.${k}`);
}

// ── 渲染入口 ──

function render(snap: QuotaSnapshot) {
  lastRenderedSnap = snap;
  if (!snap.providers || snap.providers.length === 0) {
    renderEmptyState();
    return;
  }
  // 首启空态:所有 provider 都未配 key → 不显示假 Loading,直接展示引导页
  // (大按钮「打开设置面板」)。比"⏳ Loading..."更明确告诉新用户干嘛。
  if (snap.providers.every((p) => !p.success && p.error_kind === "unconfigured_key")) {
    renderEmptyState();
    return;
  }

  // 真数据到达 → 清掉首屏的「加载中…」占位，避免它一直挂在 DOM 里
  // 跟真卡片叠在一起（之前 [H8] 状态下的 bug：4 张 provider 卡 + 1 张残留 err）
  const errPlaceholder = app.querySelector<HTMLElement>(".err");
  if (errPlaceholder) errPlaceholder.remove();

  // **2026-06-20 audit**：lastGoodSnap 之前只 set() 不 delete()，用户禁掉
  // source 后 key 永久泄漏。趁每轮 render 把"已不在 snap 里"的 stale entry 清掉。
  const presentKeys = new Set(snap.providers.map((p) => snapKey(p)));
  for (const k of [...lastGoodSnap.keys()]) {
    if (!presentKeys.has(k)) lastGoodSnap.delete(k);
  }

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
    const id = p.unique_id ?? p.source_id ?? p.provider;
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
  //
  // ── 快速路径（fix-reorder-fast-path-2026-06-18）──
  // 大多数 render（utilization 刷新、countdown 刷新、新数据到达但顺序不变）
  // 走的是同 snap.providers 顺序的 render。如果 DOM 顺序跟 snap 期望顺序
  // 已经一致，整个 reorder loop 就是 no-op。但原实现每张卡都要 querySelector
  // + 检查 desiredNext → 即使不需要 reorder 也要遍历 N 次，浪费且微 reflow。
  // 先做一次 string comparison 决定是否要 reorder，99% 的 case 直接跳过。
  const expectedOrder = snap.providers.map((p) => p.unique_id ?? p.source_id ?? p.provider).join("|");
  const actualOrder = [...app.querySelectorAll<HTMLElement>(".card[data-provider]")]
    .map((el) => el.dataset.provider ?? "")
    .join("|");
  if (expectedOrder !== actualOrder) {
    let reorderAnchor: ChildNode | null = null;
    for (const p of snap.providers) {
      const id = p.unique_id ?? p.source_id ?? p.provider;
      const card = app.querySelector<HTMLElement>(`.card[data-provider="${cssEscape(id)}"]`);
      if (!card) continue;
      if (reorderAnchor == null) {
        // 第一个：挪到 app 的最前
        if (card !== app.firstChild) {
          app.insertBefore(card, app.firstChild);
        }
      } else {
        // 后续：挪到 anchor 之后
        const desiredNext: Node | null = reorderAnchor.nextSibling;
        if (card !== desiredNext) {
          reorderAnchor.parentNode?.insertBefore(card, desiredNext);
        }
      }
      reorderAnchor = card;
    }
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
  const id = p.unique_id ?? p.source_id ?? p.provider;
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

  await resizeWindowToContent(appEl);
}

/// 空态 resize（renderEmptyState 后调用）:不走 contentFingerprint 去重,
/// 每次都量当前 DOM 高度。
async function autoResizeWindowToContent() {
  await new Promise<void>((r) => requestAnimationFrame(() => r()));
  const appEl = document.getElementById("app");
  if (!appEl) return;
  await resizeWindowToContent(appEl);
}

async function resizeWindowToContent(appEl: HTMLElement) {
  const contentH = appEl.scrollHeight;
  const screenH = window.screen?.availHeight ?? 2400;
  const maxH = Math.max(200, screenH - 80);
  const currentH = window.innerHeight;
  const desired = Math.min(contentH, maxH);
  if (Math.abs(currentH - desired) <= 1) return;
  const target = Math.round(desired);
  try {
    await invoke("resize_floating_window", { height: target });
  } catch (e) {
    console.debug("[floating] auto-resize 失败", e);
  }
}

function renderEmptyState() {
  // 首启空态:用户还没配任何 API key 时,展示引导页 + 大按钮"打开设置面板"。
  // 取代原来的假"⏳ Loading..."占位(2026-06-17 commit 删)。文案:
  //   - 标题:🚀 Musage  (品牌 + emoji,不需要 i18n)
  //   - 副标题:t("floating.footer.hint_unconfigured")  (复用已有 key,跟
  //            footer 提示保持一致;新用户首屏也最需要这条信息)
  //   - 按钮:t("floating.open_settings")  (已有 key)
  //   - 提示:t("floating.tray_right_to_settings")  (已有 key)
  // 不引入新 i18n key,等 P0-P2 收尾的 agent 统一加"empty_state.title"再升级。
  app.innerHTML = `
    <div class="empty-state">
      <div class="empty-state-title">Musage</div>
      <div class="empty-state-subtitle">${escapeHtml(t("floating.footer.hint_unconfigured"))}</div>
      <button class="empty-state-cta open-settings">${escapeHtml(t("floating.open_settings"))}</button>
      <div class="empty-state-hint">${escapeHtml(t("floating.tray_right_to_settings"))}</div>
    </div>
  `;
  void autoResizeWindowToContent();
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
  const id = p.unique_id ?? p.source_id ?? p.provider;
  // 副本的 unique_id 带 #N 后缀（"deepseek#2"），PROVIDER_META 只按
  // base id（"deepseek"）索引。剥离 #N 后缀再查 logo/name/accent。
  const baseId = id.replace(/#\d+$/, "");
  // 智谱 GLM 用 source_display_name 二次路由：CN="智谱 GLM" / EN="Z.ai"
  // 让两张 logo（紫色渐变 vs z.ai 官方）按区域切换。
  // 智谱 GLM 两个区域共用 source_id "zhipu"；只有 EN 区（Z.ai）
  // 需要切换到 zhipuEnLogo，CN 区直接用 "zhipu" key。
  const regionKey = (baseId === "zhipu" && p.source_display_name === "Z.ai")
    ? "Z.ai"
    : baseId;
  // PR 3：CustomSource 透传 display_name / accent。优先级：
  // 1. PROVIDER_META（内置 source 有固定 logo）
  // 2. snapshot 的 display_name / accent（CustomSource 后端透传）
  // 3. fallback：name=id, accent=#888
  const builtinMeta = PROVIDER_META[regionKey];
  const meta = builtinMeta ?? {
    name: p.display_name ?? p.source_display_name ?? id,
    logo: "",
    accent: p.accent ?? "#888",
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
    lastGoodSnap.set(snapKey(p), { snap: p, at: Date.now() });
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
    const entry = lastGoodSnap.get(snapKey(p));
    const good = entry && (Date.now() - entry.at < LAST_GOOD_TTL_MS) ? entry.snap : null;

    // ── 瞬态错误（网络抖动 / 限流 / 服务端错误）+ 之前有过成功数据（且未过期） ──
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
    const id = p.unique_id ?? p.source_id ?? p.provider;
    // 按 error_kind 分发按钮 (2026-06-17 commit):
    // - unconfigured_key / auth_failed: 打开设置面板
    // - auth_failed + xiaomimimo:        🔑 重新登录 (走 xiaomi_login window)
    // - auth_failed + claude_official:   打开设置面板 (cookie 在设置面板里改)
    // - schema_unknown:                  高级设置 (跳到 schema overrides section)
    // - network / rate_limited / server_error: 🔄 重试 + 下次自动重试倒计时
    // - parse / other:                   只显示错,无按钮 (开发者向)
    let actionBtn = "";
    let retryInfo = "";
    // v0.2.1 commit 10 (P2-A-7 错误恢复完整版): 通用按钮 —— 任何错误态都
    // 允许复制错误信息和跳日志 tab,跟具体 error_kind 的恢复按钮并列。
    // 用 data-unique-id 反查 snap,前端不需要给每个按钮传 p.error 长字符串。
    const commonErrBtns = p.error
      ? `<button class="err-btn err-btn-copy" data-unique-id="${escapeHtml(id)}">${escapeHtml(t("floating.err_btn_copy"))}</button>`
        + `<button class="err-btn err-btn-logs" data-unique-id="${escapeHtml(id)}">${escapeHtml(t("floating.err_btn_logs"))}</button>`
      : "";
    if (kind === "unconfigured_key" || kind === "auth_failed") {
      // **L8 fix（2026-06-19）**：transient=true 时（这是 placeholder 而非
      // 真实错误态）跳过"打开设置"按钮渲染。placeholder 只会持续 2-5s，
      // 紧接着真实 fetch 结果会替换它；现在不闪"打开设置"了，体验跟手。
      if (p.transient === true) {
        // no action button
      } else if (kind === "auth_failed" && id === "xiaomimimo") {
        actionBtn = `<button class="err-btn err-btn-relogin" data-action="relogin-xiaomi">${escapeHtml(t("floating.err_btn_relogin_xiaomi"))}</button>`;
      } else {
        actionBtn = `<button class="err-btn open-settings">${escapeHtml(t("floating.open_settings"))}</button>`;
      }
    } else if (kind === "schema_unknown") {
      actionBtn = `<button class="err-btn err-btn-advanced" data-section="advanced">${escapeHtml(t("settings.nav.advanced"))}</button>`;
      const schemaHint = `<div class="hint">${escapeHtml(t("floating.init_error_hint"))}</div>`;
      retryInfo = schemaHint;
    } else if (kind === "network" || kind === "rate_limited" || kind === "server_error") {
      actionBtn = `<button class="err-btn err-btn-retry" data-unique-id="${escapeHtml(id)}">${escapeHtml(t("floating.err_btn_retry"))}</button>`;
      if (p.next_fetch_at && p.next_fetch_at > Date.now()) {
        const mins = Math.max(1, Math.ceil((p.next_fetch_at - Date.now()) / 60000));
        retryInfo = `<div class="hint">${escapeHtml(t("floating.err_retry_in_minutes", { mins }))}</div>`;
      }
    }
    // v0.2.1 commit 10: 通用复制+日志按钮追加在 actionBtn 之后。
    actionBtn = actionBtn + commonErrBtns;
    card.classList.add("err-card", `err-${kind}`);
    // H8 修复：err-label 加在 .card-head-status 里，CSS 用 row-reverse
    // 把 [dot, label] 渲染成 [label, dot] —— dot 永远在卡片右上角，
    // label 在其左侧展开。不要直接 append 到 .card-head（会触发
    // flex space-between 把 dot 推到中间）。
    const headStatus = card.querySelector<HTMLElement>(".card-head-status")!;
    let headLabel = headStatus.querySelector<HTMLElement>(".err-label");
    if (!headLabel) {
      headLabel = document.createElement("span");
      headLabel.className = "err-label";
      headStatus.appendChild(headLabel);
    }
    headLabel.textContent = label;
    rowsBox.innerHTML = `
      <div class="err-msg">${escapeHtml(p.error ?? t("floating.error.unknown"))}</div>
      ${actionBtn}
      ${retryInfo}
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
  const providerId = p.source_id ?? p.provider;
  rows.forEach((r, i) => {
    const key = rowKey(providerId, i, r);
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
  });
  for (const orphan of existing.values()) orphan.remove();
}

// ── 行 ──

function rowKey(providerId: string, index: number, r: QuotaRow): string {
  // **L7 fix（2026-06-19）**：之前用 `r.label`（"周" / "Weekly"）做 key，
  // 切 locale 后 label 变 → 整个 rowsBox 的 DOM diff 失败 → 所有行
  // 重建 + 短暂闪烁。改用三层 fallback：
  //   1. r.kind（RowKind 枚举，与 locale 解耦）— 最稳定
  //   2. r.label（已 deprecated，仅 kind 缺失时用）— 仍会跨 locale 失效
  //   3. index（位置 fallback）— 保证任何 r 都有稳定 key
  // prefix 用 providerId 让不同 provider 的 rowsBox key 互不撞。
  let stable: string;
  if (r.kind) {
    stable = `kind:${r.kind}`;
  } else if (r.label) {
    stable = `label:${r.label}`;
  } else {
    stable = `idx:${index}`;
  }
  // Phase 1: Tavily 走"used/total"组合（"150/1000 credits"），优先于 remaining/utilization
  let kind = "unknown";
  if (r.used != null && r.total != null) kind = "credits";
  else if (r.used != null) kind = "used";
  else if (r.utilization != null) kind = "pct";
  else if (r.remaining != null) kind = "amt";
  return `${providerId}:${kind}:${stable}`;
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
      // Tavily 月重置：用 i18n 前缀，不复用 label（"Free tier"）
      rowEl.dataset.resetsPrefix = t("floating.countdown.monthly_prefix");
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
    else {
      delete rowEl.dataset.resetsAt;
      delete rowEl.dataset.resetsPrefix;
    }
  } else if (r.remaining != null) {
    const labelSpan = rowEl.querySelector<HTMLElement>(".row-label > span:first-child")!;
    labelSpan.textContent = r.label;
    const pct = rowEl.querySelector<HTMLElement>(".pct")!;
    pct.textContent = `${formatAmount(r.remaining)} ${escapeHtml(r.unit ?? "")}`;
    // 钱包余额低额高亮：用户在设置面板启用了 wallet_alert_threshold 且
    // remaining < 阈值时，整行翻红（alert 类），否则保持默认蓝色（balance）。
    // 默认阈值 = null → 始终走 balance，跟旧行为 byte-for-byte 一致。
    const walletThr = renderPrefs.walletAlertThreshold;
    pct.className =
      walletThr != null && r.remaining < walletThr
        ? "pct alert"
        : "pct balance";
  }
}

// ── Footer ──

function updateFoot(snap: QuotaSnapshot) {
  let foot = app.querySelector<HTMLElement>(".foot");
  // 底部提示行默认隐藏，用户在 设置→浮窗 里手动开启
  if (!renderPrefs.showFooterHint) {
    if (foot) foot.remove();
    return;
  }
  // H9 修复：用 error_kind 枚举判断，不再依赖中文错误串（前者 Rust 改文案不会破）
  const anyUnconfigured = snap.providers.some(
    (p) => !p.success && p.error_kind === "unconfigured_key",
  );
  const hint = anyUnconfigured
    ? t("floating.footer.hint_unconfigured")
    : t("floating.footer.hint_normal");
  // plural-aware：英文 1 个 provider vs N providers
  const text = t("floating.footer.count", { count: snap.providers.length, hint });
  if (foot) {
    foot.textContent = text;
    // M22 fix: 已有 foot 时只更新 text，不 reposition。reorder 循环会调
    // insertBefore 把新 card 插到 firstChild，可能把 foot 推到中间。
    // 强制 reposition 到末尾。
    if (foot !== app.lastChild) {
      app.appendChild(foot);
    }
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
    // 优先用 data-resets-prefix（Tavily 用 t("floating.countdown.monthly_prefix")），否则用 label + reset suffix
    const prefix = row.dataset.resetsPrefix
      ?? (row.querySelector<HTMLElement>(".row-label > span:first-child")?.textContent ?? "") + t("floating.countdown.reset_suffix");
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
  // P1 fix: 之前硬编码 "zh-CN"。getLocale() 已在 i18n/index.ts 顶层导入。
  return v.toLocaleString(getLocale(), { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function colorClass(util: number): string {
  // 4 档离散色 —— 整条 bar + 文字单色，**不**是位置性渐变。
  // 分界点来自 renderPrefs.colorThresholds（默认 [50, 70, 88]，
  // 用户可在设置面板里调）。Rust 端 save_config / set_display_thresholds
  // 两路都做 0 < t0 < t1 < t2 < 100 校验，这里不再兜底，信赖 cfg 合法。
  const [t0, t1, t2] = renderPrefs.colorThresholds;
  if (util < t0) return "ok";
  if (util < t1) return "cyan";
  if (util < t2) return "warn";
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
    return `${prefix} ${label}${t("floating.countdown.reset_done")}`;
  }
  if (days >= 1) {
    const label = `${dt.getMonth() + 1}-${dt.getDate()}`;
    return `${prefix} ${label}${t("floating.countdown.days_left", { days })}`;
  }
  // < 1 天：显示时分 + "Nh Mm" 倒计时
  const time = `${pad2(dt.getHours())}:${pad2(dt.getMinutes())}`;
  const minutes = Math.floor(remainMs / 60000);
  if (minutes < 60) {
    return `${prefix} ${time}${t("floating.countdown.minutes_left", { minutes })}`;
  }
  const hours = Math.floor(minutes / 60);
  const mins = minutes % 60;
  return `${prefix} ${time}${t("floating.countdown.hours_minutes", { hours, minutes: pad2(mins) })}`;
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
  // ── i18n 初始化：必须在任何 t() 调用前完成（加载 dict） ──
  await initLocale();
  PROVIDER_META = buildProviderMeta();
  // 同步浮窗 document.title（settings/main.ts 对 settings 做了同样的事）
  document.title = t("window.floating");
  // locale 变化时重建元数据 + 刷新所有卡片名称 + 同步标题
  onLocaleChange(() => {
    PROVIDER_META = buildProviderMeta();
    document.title = t("window.floating");
    // 刷新已渲染卡片的名称（不触发 full re-render，只更新 .card-name）
    // extra instance 的 dataset.provider 是 "minimax#2"，需取 base id 查 PROVIDER_META
    app.querySelectorAll<HTMLElement>(".card[data-provider]").forEach((card) => {
      let id = card.dataset.provider;
      if (!id) return;
      // 副本 "minimax#2" → "minimax"；"custom_<uuid>" → 保持
      if (id.includes("#")) {
        const base = id.split("#")[0];
        if (PROVIDER_META[base]) id = base;
      }
      const meta = PROVIDER_META[id];
      if (!meta) return;
      const name = card.querySelector<HTMLElement>(".card-name");
      if (name) name.textContent = meta.name;
    });
    // 重新渲染 loading/error 态（文字也会变）
    const snap = lastRenderedSnap;
    if (snap) render(snap);
  });
  // 监听 Rust 端 locale-changed 事件（设置面板切语言时触发，跨 webview 同步）
  // 防无限循环：若当前 locale 与事件相同，跳过（避免 set_app_locale 二次触发再 emit）
  listen<string>("musage://locale-changed", async (e) => {
    const newLocale = e.payload;
    if ((newLocale === "en" || newLocale === "zh-CN") && newLocale !== getLocale()) {
      await setLocale(newLocale);
    }
  });

  const w = getCurrentWindow();
  // 拖动：左键按住任意非按钮区域 → start_dragging
  // **2026-06-20 audit**：之前不检查 e.button，右键 / 中键也触发拖动 →
  // 跟系统右键菜单（特别是 macOS 上的 NSWindow 右键）冲突。显式仅响应左键。
  app.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("button, input, select, a")) return;
    e.preventDefault();
    // w.startDragging 返 Promise，不 await（同步阻止默认行为已足够），
    // 但加 catch 防止 IPC 拒绝变成 unhandled rejection。
    w.startDragging().catch((err) => console.debug("[floating] startDragging 失败", err));
  });
  // 双击 → 立即刷新
  // **2026-06-20 audit**：之前 catch 只 console.error，用户看到"啥反应都没有"。
  // 改为 flash dot 变红（用 settings 用的 flash helper 不可取 —— main.ts 没引
  // 入。最简：用 status dot 临时翻红 + 100ms 后恢复）。
  app.addEventListener("dblclick", async () => {
    try {
      const snap = await invoke<QuotaSnapshot>("refresh_now");
      render(snap);
    } catch (e) {
      console.error(e);
      const dot = document.querySelector<HTMLElement>(".card-dot");
      if (dot) {
        dot.classList.add("card-dot-error");
        setTimeout(() => dot.classList.remove("card-dot-error"), 1500);
      }
    }
  });

  // 订阅后端推送
  let unlisten: UnlistenFn | null = null;
  let unlistenHover: UnlistenFn | null = null;
  let unlistenCfg: UnlistenFn | null = null;
  // **2026-06-20 audit**：5 处 listen().then() 之前都没 .catch()，
  // IPC bridge 启动挂掉时 promise reject → unhandled rejection。补 catch + log。
  listen<QuotaSnapshot>("musage://snapshot", (e) => {
    render(e.payload);
  })
    .then((fn) => (unlisten = fn))
    .catch((e) => console.error("[floating] listen musage://snapshot 失败", e));

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
  //
  // **2026-06-20 audit**：body mouseenter/mouseleave 之前是匿名 arrow，
  // beforeunload 没办法 remove。改成 named fn 配对 remove。
  const setHoverAttr = (on: boolean) => {
    if (on) document.body.dataset.hover = "1";
    else delete document.body.dataset.hover;
  };
  const onBodyMouseEnter = () => setHoverAttr(true);
  const onBodyMouseLeave = () => setHoverAttr(false);
  document.body.addEventListener("mouseenter", onBodyMouseEnter);
  document.body.addEventListener("mouseleave", onBodyMouseLeave);
  listen<boolean>("musage://floating-hover", (e) => {
    setHoverAttr(e.payload);
  })
    .then((fn) => (unlistenHover = fn))
    .catch((e) => console.error("[floating] listen musage://floating-hover 失败", e));

  // ── 省电模式同步：body[data-low-power] 让 CSS 关掉 backdrop-filter + transition ──
  let unlistenLowPower: UnlistenFn | null = null;
  const setLowPowerAttr = (on: boolean) => {
    if (on) document.body.dataset.lowPower = "1";
    else delete document.body.dataset.lowPower;
  };
  listen<boolean>("musage://low-power-mode-changed", (e) => {
    setLowPowerAttr(e.payload);
  })
    .then((fn) => (unlistenLowPower = fn))
    .catch((e) => console.error("[floating] listen musage://low-power-mode-changed 失败", e));

  // 启动时立即调 render —— render() 内部检测"所有 provider 都未配 key"会
  // 走 renderEmptyState(),不再显示"⏳ Loading..."这个假占位 (2026-06-17 commit
  // 删)。第一次启动时 get_snapshot 会返空 snap → renderEmptyState() 立刻展示。
  try {
    const snap = await invoke<QuotaSnapshot>("get_snapshot");
    if (snap.fetched_at && snap.providers.length > 0) {
      render(snap);
    } else {
      const fresh = await invoke<QuotaSnapshot>("refresh_now");
      render(fresh);
    }
  } catch (e) {
    // 2026-06-20 audit：t() 不做 HTML 转义。这里已用 escapeHtml 包每段
    // 值（t() 返回 + String(e)），CSP script-src 'self' 也挡 inline
    // script 执行。两层防御。
    app.innerHTML = `<div class="err"><div class="err-title">${escapeHtml(t("floating.loading_error_title"))}</div><div class="err-msg">${escapeHtml(String(e))}</div><button class="err-btn open-settings">${escapeHtml(t("floating.open_settings"))}</button><div class="hint">${escapeHtml(t("floating.tray_right_to_settings"))}</div></div>`;
  }

  // 事件代理：错误卡片的恢复按钮 (2026-06-17 commit)
  // 5 种 action 通过 data-* 区分:
  // - .open-settings (无 data-*):  打开设置面板 (原有)
  // - .err-btn-retry (data-unique-id): 立即重拉该 provider (绕过 backoff)
  // - .err-btn-advanced (data-section="advanced"): 打开设置 + 跳到高级 section
  // - .err-btn-relogin (data-action="relogin-xiaomi"): 打开小米登录窗
  // - .err-btn-copy (data-unique-id): 复制错误信息到剪贴板 (commit 10)
  // - .err-btn-logs (data-unique-id): 打开设置 + 跳到日志 section (commit 10)
  app.addEventListener("click", async (e) => {
    const el = e.target as HTMLElement;
    const target = el.closest<HTMLElement>(".err-btn, .empty-state-cta");
    if (!target) return;
    e.stopPropagation();
    if (target.classList.contains("open-settings")) {
      invoke("open_settings_window").catch((err) => console.error(err));
    } else if (target.classList.contains("err-btn-retry")) {
      const uniqueId = target.dataset.uniqueId;
      if (uniqueId) {
        invoke("refresh_single", { id: uniqueId }).catch((err) => console.error(err));
      }
    } else if (target.classList.contains("err-btn-advanced")) {
      const section = target.dataset.section ?? "advanced";
      invoke("open_settings_window", { section }).catch((err) => console.error(err));
    } else if (target.classList.contains("err-btn-relogin")) {
      invoke("open_xiaomi_login_window").catch((err) => console.error(err));
    } else if (target.classList.contains("err-btn-copy")) {
      // v0.2.1 commit 10: 反查 snapshot 拿 p.error 复制到剪贴板。
      // 不在按钮上拼长字符串(每次 render 都 escape,效率差)。
      const uniqueId = target.dataset.uniqueId;
      if (!uniqueId) return;
      try {
        const snap = await invoke<QuotaSnapshot>("get_snapshot");
        const p = snap.providers.find(
          (x) => (x.unique_id ?? x.source_id ?? x.provider) === uniqueId,
        );
        const errText = p?.error ?? "";
        if (errText) {
          await navigator.clipboard.writeText(errText);
          showMiniFlash(t("floating.err_btn_copied"));
        }
      } catch (err) {
        console.error("copy error failed:", err);
      }
    } else if (target.classList.contains("err-btn-logs")) {
      // v0.2.1 commit 10 + commit 8: 跳日志 section (commit 8 后端已支持)
      invoke("open_settings_window", { section: "logs" }).catch((err) =>
        console.error(err),
      );
    }
  });

  // ── v0.2.0 不再自动检查更新 —— 升级走"用户手动下 dmg/nsis 装"路径 ──

  // 读取用户选的置顶/置底模式 + 省电模式初始状态。
  // PinBottom 模式下，监听 mouseenter/mouseleave 让后端临时切到 always-on-top。
  let pinMode: FloatingPinMode = "pin_top";
  try {
    const cfg = await invoke<{
      floating_pin_mode?: FloatingPinMode;
      low_power_mode?: boolean;
      tavily_concise_mode?: boolean;
      zenmux_payg_concise_mode?: boolean;
      color_thresholds?: [number, number, number];
      wallet_alert_threshold?: number | null;
      color_overrides?: Record<string, string>;
      show_footer_hint?: boolean;
    }>("get_config");
    pinMode = cfg.floating_pin_mode ?? "pin_top";
    setLowPowerAttr(cfg.low_power_mode ?? false);
    renderPrefs = {
      tavilyConciseMode: cfg.tavily_concise_mode ?? true,
      zenmuxPaygConciseMode: cfg.zenmux_payg_concise_mode ?? true,
      colorThresholds: cfg.color_thresholds ?? [50, 70, 88],
      walletAlertThreshold: cfg.wallet_alert_threshold ?? null,
      colorOverrides: cfg.color_overrides ?? {},
      showFooterHint: cfg.show_footer_hint ?? false,
    };
    applyColorOverrides();
  } catch (e) {
    console.error("读 config 失败", e);
  }
  setupHoverRaise(pinMode);

  // 配置变化时（设置面板改 Tavily / ZenMux 简洁模式等）→ 更新 renderPrefs 后
  // 用 lastRenderedSnap 重渲染。
  //
  // 之前这里会 `invoke<QuotaSnapshot>("get_snapshot")` + `render(snap)`，但
  // 后端每次 IPC 都会 emit `musage://snapshot`（带最新数据），所以这次
  // get_snapshot 拉到的就是 stale 的"刚 emit 完"的版本，渲染一遍完全冗余。
  // 更糟的是它触发 render → 浮窗走 reorder loop → 紧跟 snapshot 事件
  // 触发的 render 又 reorder 一次 → 闪烁（fix-config-double-render-2026-06-18）。
  //
  // 改成：用 `lastRenderedSnap`（snapshot 事件 handler 里已经更新过了）
  // 重渲染。如果 snapshot 事件先到达，lastRenderedSnap 已是最新数据，渲染
  // 完全幂等；如果 config-changed 先到（极端竞态），用旧数据渲染一遍也
  // 安全，snapshot 事件到了会再渲染一次。
  listen("musage://config-changed", async () => {
    try {
      const cfg = await invoke<{
        tavily_concise_mode?: boolean;
        zenmux_payg_concise_mode?: boolean;
        color_thresholds?: [number, number, number];
        wallet_alert_threshold?: number | null;
        color_overrides?: Record<string, string>;
        show_footer_hint?: boolean;
      }>("get_config");
      renderPrefs = {
        tavilyConciseMode: cfg.tavily_concise_mode ?? true,
        zenmuxPaygConciseMode: cfg.zenmux_payg_concise_mode ?? true,
        colorThresholds: cfg.color_thresholds ?? [50, 70, 88],
        walletAlertThreshold: cfg.wallet_alert_threshold ?? null,
        colorOverrides: cfg.color_overrides ?? {},
        showFooterHint: cfg.show_footer_hint ?? false,
      };
      applyColorOverrides();
      if (lastRenderedSnap) render(lastRenderedSnap);
    } catch (e) {
      console.error("[floating] 重新读 config 失败", e);
    }
  }).then((fn) => (unlistenCfg = fn))
    .catch((e) => console.error("[floating] listen musage://config-changed 失败", e));

  // 设置面板改了模式时，重新挂/摘 hover 监听。
  // （设置面板那边调 set_floating_pin_mode 会 emit 这个事件）
  // **2026-06-20 audit**：之前 listen() 没 .catch() + UnlistenFn 没存，dev hot-reload
  // / 极端情况累积 stale handler。补 catch + 存 unlisten，beforeunload 时 remove。
  let unlistenPinMode: UnlistenFn | null = null;
  listen<FloatingPinMode>("musage://pin-mode-changed", (e) => {
    // 清掉旧的监听再装新的（幂等）
    document.body.removeEventListener("mouseenter", hoverEnterHandler);
    document.body.removeEventListener("mouseleave", hoverLeaveHandler);
    setupHoverRaise(e.payload);
  })
    .then((fn) => (unlistenPinMode = fn))
    .catch((e) => console.error("[floating] listen musage://pin-mode-changed 失败", e));

  window.addEventListener("beforeunload", () => {
    if (unlisten) unlisten();
    if (unlistenHover) unlistenHover();
    if (unlistenLowPower) unlistenLowPower();
    if (unlistenCfg) unlistenCfg();
    if (unlistenPinMode) unlistenPinMode();
    if (countdownTimer !== null) clearInterval(countdownTimer);
    // 2026-06-20 audit: 配对 remove body mouseenter/mouseleave
    document.body.removeEventListener("mouseenter", onBodyMouseEnter);
    document.body.removeEventListener("mouseleave", onBodyMouseLeave);
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
