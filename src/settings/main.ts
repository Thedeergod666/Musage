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
import { setupUpdaterSection } from "./updater";
import { bindCredentialButtonsGlobal, bindXiaomiLoginEvents, loadXiaomiDisplayMode } from "./credentials";
import { bindOrderButtonsGlobal, updateOrderConfig } from "./order";
import { flash } from "./utils";

// ── 1) 同步：sidebar 切换 + tabs ───────────────────────────────

function setupNav() {
  const navItems = document.querySelectorAll<HTMLButtonElement>(".nav-item");
  const sections = document.querySelectorAll<HTMLElement>(".section-view");
  navItems.forEach((item) => {
    item.addEventListener("click", () => {
      const target = item.dataset.section;
      if (!target) return;
      navItems.forEach((n) =>
        n.classList.toggle("active", n.dataset.section === target),
      );
      sections.forEach((s) => {
        s.hidden = s.dataset.section !== target;
      });
    });
  });
}

setupNav();
// tabs 是 provider 内部 5 个 tab 切换的兼容（万一以后想用），现在 panel
// 顺序由 list_sources 决定，tab 切换是 no-op。
setupTabs();

// ── 2) 异步：拉数据 + 渲染 6 个 section ───────────────────────

async function init() {
  try {
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

    // 拉每个 source 的 key 状态 + 日志 + 注入 updater
    // （cfg 初值已经在 render*Section 里用上了；不用再调 loadConfig）
    await loadAllCredentialStatus(sources);
    await loadXiaomiDisplayMode();
    await loadLogs();
    setupUpdaterSection();

    // 订阅后端 config-changed：用户改了「在浮窗显示」或调整了 provider
    // 顺序时，Rust 会 emit 这个事件；设置面板需要重渲浮窗卡片顺序区域
    // （enabled/disabled 分区会随之调整）和重读 cfg。
    let unlistenCfg: UnlistenFn | null = null;
    listen("musage://config-changed", async () => {
      try {
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
    flash(`✗ 初始化失败: ${e}`, true);
  }
}

void init();
