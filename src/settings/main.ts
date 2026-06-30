// 设置面板入口（v0.6+ Stage 5 终极版）
//
// 流程：
// 1. 同步：sidebar 切换 + tabs（如果还有）
// 2. 异步：拉 cfg + sources → 并发渲染 6 个 section 到对应 .section-view
// 3. 异步：拉每个 source 的 key 状态 + 日志

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { setupTabs } from "./utils";
import { listSources, getConfig } from "./api";
import { renderProvidersSection, loadAllCredentialStatus } from "./providers";
import { renderFloatingSection } from "./floating";
import { renderAppSection } from "./app";
import { renderAdvancedSection } from "./advanced";
import { renderLogsSection, loadLogs } from "./logs";
import { renderAboutSection } from "./about";
// v0.2.0 不再自动检查更新 —— 升级走"用户手动下 dmg/nsis 装"路径
import { renderRegionSection } from "./region-wizard";
import { bindCredentialButtonsGlobal, bindXiaomiLoginEvents, loadXiaomiDisplayMode } from "./credentials";
import { bindOrderButtonsGlobal, updateOrderConfig, isSuppressingConfigRebuild } from "./order";
import { flash } from "./utils";
import { t, initLocale, onLocaleChange } from "../i18n";
import {
  navProvidersIcon,
  navFloatingIcon,
  navAppIcon,
  navAdvancedIcon,
  navLogsIcon,
  navAboutIcon,
} from "../icons";

// ── 1) 同步：sidebar 切换 + tabs ───────────────────────────────

/// 把 settings.html 里所有带 [data-i18n] 的元素 textContent 改成对应 key 的翻译。
/// 在 initLocale() 之后 + 每次 locale-changed 之后都跑一次（onLocaleChange listener）。
function applyDataI18n() {
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((el) => {
    const key = el.dataset.i18n;
    if (!key) return;
    el.textContent = t(key);
  });
  // 同步 window title（settings.html）
  document.title = t("window.settings");
}

async function initI18n() {
  await initLocale();
  applyDataI18n();
  // 监听前端 setLocale → 重新跑 data-i18n 翻译 + 通知 listeners
  // L1 fix: region-wizard.ts 的 renderRegionSection 在 init 时调一次 t() 把标题
  // 烘到 DOM textContent，切 locale 后不跟着换。这里加 region section 重渲。
  onLocaleChange(() => {
    applyDataI18n();
    // region section 的 radio title / section title / apply 按钮全走 t() 烘焙，
    // 不是 data-i18n 静态元素，必须整个 section 重渲。
    const regionContainer = document.getElementById("section-region");
    if (regionContainer) void renderRegionSection(regionContainer);
  });
}

void initI18n();

function setupNav() {
  const navItems = document.querySelectorAll<HTMLButtonElement>(".nav-item");
  const sections = document.querySelectorAll<HTMLElement>(".section-view");

  // 按 data-section 注入 Lucide icon（settings.html 里 .nav-emoji 是空 span）
  const iconBySection: Record<string, string> = {
    providers: navProvidersIcon,
    floating: navFloatingIcon,
    app: navAppIcon,
    advanced: navAdvancedIcon,
    logs: navLogsIcon,
    about: navAboutIcon,
  };

  // v0.2.1 commit 8: 抽 helper,nav click 跟外部 navigate 事件都走同一条路径。
  function navigateToSection(target: string): boolean {
    const btn = document.querySelector<HTMLButtonElement>(`.nav-item[data-section="${target}"]`);
    if (!btn) return false;
    navItems.forEach((n) =>
      n.classList.toggle("active", n.dataset.section === target),
    );
    sections.forEach((s) => {
      s.hidden = s.dataset.section !== target;
    });
    return true;
  }

  navItems.forEach((item) => {
    const section = item.dataset.section;
    const iconUrl = section ? iconBySection[section] : undefined;
    if (iconUrl) {
      const slot = item.querySelector<HTMLElement>(".nav-emoji");
      if (slot) {
        const img = document.createElement("img");
        img.src = iconUrl;
        img.alt = "";
        img.className = "nav-emoji";
        slot.replaceWith(img);
      }
    }

    item.addEventListener("click", () => {
      const target = item.dataset.section;
      if (!target) return;
      navigateToSection(target);
    });
  });

  // v0.2.1 commit 8: 后端 open_settings_window 命令带 section 参数时发
  // 这个事件,settings 窗口接收后跳对应 tab。修复之前 P1 commit `5b976e2`
  // 留的半残按钮(open advanced 只开窗不跳 section)。
  listen<string>("musage://settings-navigate", (e) => {
    navigateToSection(e.payload);
  }).catch((err) => console.error("settings-navigate listen failed:", err));
}

setupNav();
// tabs 是 provider 内部 5 个 tab 切换的兼容（万一以后想用），现在 panel
// 顺序由 list_sources 决定，tab 切换是 no-op。
setupTabs();

// ── 2) 异步：拉数据 + 渲染 6 个 section ───────────────────────

async function init() {
  try {
    // P0 fix: 等 initLocale 完再渲染。原顺序是 initI18n() 与 init() 并行 fire-and-forget，
    // IPC 竞速下 init() 先跑完 → 渲染时 dicts 是空的 → 所有 t() 返原始 key。
    // initI18n() 内部 loadLocale 已带缓存，二次调是 O(1) noop，所以这里 await 安全。
    await initI18n();

    // 全局事件委托（只绑一次，document-level）
    bindCredentialButtonsGlobal();
    bindXiaomiLoginEvents();
    bindOrderButtonsGlobal();

    // 拉 cfg + sources（并发）
    const cfg = await getConfig();
    const sources = await listSources();

    // 6 个 section 容器
    const containers = {
      providers: document.querySelector<HTMLElement>('.section-view[data-section="providers"]'),
      floating: document.querySelector<HTMLElement>('.section-view[data-section="floating"]'),
      app: document.querySelector<HTMLElement>('.section-view[data-section="app"]'),
      advanced: document.querySelector<HTMLElement>('.section-view[data-section="advanced"]'),
      logs: document.querySelector<HTMLElement>('.section-view[data-section="logs"]'),
      about: document.querySelector<HTMLElement>('.section-view[data-section="about"]'),
    };

    // 渲染 6 section（providers 异步，others 同步）
    if (containers.providers) {
      await renderProvidersSection(containers.providers);
    }
    if (containers.floating) renderFloatingSection(containers.floating, cfg);
    if (containers.app) renderAppSection(containers.app, cfg);
    if (containers.advanced) renderAdvancedSection(containers.advanced, cfg);
    if (containers.logs) renderLogsSection(containers.logs);
    if (containers.about) await renderAboutSection(containers.about);

    // P2 区域向导 + 语言切换：放在「应用」section 底部
    if (containers.app) {
      await renderRegionSection(containers.app);
    }

    // 拉每个 source 的 key 状态 + 日志
    // （cfg 初值已经在 render*Section 里用上了；不用再调 loadConfig）
    await loadAllCredentialStatus(sources);
    await loadXiaomiDisplayMode();
    await loadLogs();

    // 订阅后端 config-changed：用户改了「在浮窗显示」或调整了 provider
    // 顺序时，Rust 会 emit 这个事件；设置面板需要重渲浮窗卡片顺序区域
    // （enabled/disabled 分区会随之调整）和重读 cfg。
    //
    // 但 order.ts 在批量 IPC（如分隔线拖拽→连续 setProviderEnabled N 次）
    // 期间会置 suppressConfigRebuild=true 屏蔽本 listener，避免每次
    // getConfig 覆盖乐观更新的 orderCfg 导致 UI 在「全隐藏」与「新位置」
    // 之间穿梭。批量结束后 order.ts 会强制 resync 一次。
    let unlistenCfg: UnlistenFn | null = null;
    listen("musage://config-changed", async () => {
      try {
        if (isSuppressingConfigRebuild()) return;
        const cfg = await getConfig();
        updateOrderConfig(cfg);
      } catch (e) {
        console.warn("[settings] config-changed 刷新失败", e);
      }
    }).then((fn) => (unlistenCfg = fn));
    window.addEventListener("beforeunload", () => {
      if (unlistenCfg) unlistenCfg();
    });
  } catch (e) {
    console.error("[settings] init failed", e);
    flash(t("settings.init_failed", { err: String(e) }), true);
  }
}

void init();
