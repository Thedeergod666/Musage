// 设置面板 "数据源" section —— 动态渲染
//
// **完全 registry-driven**：
// - list_sources() 拿到 SourceMeta[] → 调 createProviderPanel(meta) 每个出一个 panel
// - 加新 source 改 1 个文件（[src-tauri/src/providers/mod.rs] builtin_sources）
//   + [source-extras.ts] EXTRAS 表（如果该 source 有额外字段）
// - settings.html / providers.ts 主流程 0 改动
//
// 之前的 settings.html 硬编码 5 个 panel，每个 ~70 行 = 350 行。换成动态后
// 加一个 source 是 0 行 HTML 改动 + ~10 行 source-extras.ts（如果有 extras）。
//
// 兼容旧代码：所有 #id / .class 都跟原 v0.5.x 一致（api-key-{id} /
// enabled-{id} / interval-{id} / api-key-status-{id} 等），让 config.ts 里
// 的 loadConfig() / saveConfig() 不用改就能照常读写这些元素。

import { listSources, getConfig, setProviderEnabled } from "./api";
import { el } from "./utils";
import { getProviderExtras } from "./source-extras";
import { renderOrderSection } from "./order";
import { renderCredentialBlock, loadCredentialStatus } from "./credentials";
import { getProviderMeta } from "./logos";
import { flash } from "./utils";
import type { AppConfig, SourceMeta } from "./types";

/// 主入口：渲染整个 "数据源" section。
/// - 调 list_sources 拿 SourceMeta[]
/// - 调 get_config 拿 cfg（用来初始化 region/interval/extras 初值 + 启用状态）
/// - 先渲染顶部"浮窗卡片顺序"，再渲染每个 source 的 panel
export async function renderProvidersSection(container: HTMLElement) {
  let sources: SourceMeta[];
  let cfg: AppConfig;
  try {
    [sources, cfg] = await Promise.all([listSources(), getConfig()]);
  } catch (e) {
    container.innerHTML = `<div class="section-empty error">✗ 加载 sources 失败: ${String(e)}</div>`;
    return;
  }

  container.innerHTML = ""; // 清掉占位

  // 1) 顶部：浮窗卡片顺序（带 enabled/disabled 分区）
  renderOrderSection(container, sources, cfg.provider_order, cfg);

  // 2) 每个 source 一个 panel
  for (const meta of sources) {
    container.appendChild(createProviderPanel(meta, cfg));
  }
}

/// 一个 source → 一个 panel（带 header + credentials + EXTRAS + 启用/间隔）
export function createProviderPanel(meta: SourceMeta, cfg: AppConfig): HTMLElement {
  const section = el(
    "section",
    { class: "provider-section", "data-id": meta.id },
  );

  // 拿 logo 资产（沿用浮窗 [src/main.ts:15-30] 同款 import）
  const providerMeta = getProviderMeta(meta.id);
  const logoImg = providerMeta
    ? el("img", {
        class: "provider-logo",
        src: providerMeta.logo,
        alt: providerMeta.name,
        title: providerMeta.name,
      })
    : null;

  // ── Header: [logo] [display_name] ........ [在浮窗显示 checkbox] ──
  const enabledCheckbox = el("input", {
    type: "checkbox",
    id: `enabled-${meta.id}`,
    "data-id": meta.id,
  }) as HTMLInputElement;
  enabledCheckbox.checked = cfg.providers?.[meta.id]?.enabled ?? true;
  // 即时生效
  enabledCheckbox.addEventListener("change", () => {
    setProviderEnabled(meta.id, enabledCheckbox.checked).catch((e) => {
      flash(`✗ 切换显示失败: ${e}`, true);
    });
  });

  section.appendChild(
    el(
      "div",
      { class: "provider-header" },
      ...(logoImg ? [logoImg] : []),
      el("span", { class: "provider-name" }, meta.display_name),
      el(
        "div",
        { class: "provider-enabled" },
        enabledCheckbox,
        el("label", { for: `enabled-${meta.id}` }, "在浮窗显示"),
      ),
    ),
  );

  // ── 凭据块 ──
  section.appendChild(renderCredentialBlock(meta));

  // ── EXTRAS（per-id 区域下拉 / 集群 / 简洁模式 / base url 等）──
  for (const block of getProviderExtras(meta.id)) {
    section.appendChild(block(meta, cfg));
  }

  // ── 轮询间隔（每个 provider 都有，挪到 extras 也行；为简洁放最后）──
  section.appendChild(renderIntervalOverride(meta.id, cfg));

  return section;
}

/// 每个 provider 的「轮询间隔（覆盖）」字段
function renderIntervalOverride(id: string, cfg: AppConfig): HTMLElement {
  const v = cfg.providers?.[id]?.refresh_interval_secs;
  const input = el("input", {
    type: "number",
    id: `interval-${id}`,
    "data-id": id,
    min: "10",
    step: "5",
    placeholder: `默认 ${cfg.refresh_interval_secs ?? 60} 秒（顶部「轮询间隔」）`,
  }) as HTMLInputElement;
  if (v != null) input.value = String(v);
  return el(
    "div",
    { class: "field" },
    el("label", {}, "轮询间隔（覆盖）"),
    el("div", { class: "input-row" }, input, el("span", { class: "unit-suffix" }, "秒")),
    el(
      "div",
      { class: "help" },
      "留空 = 用顶部「轮询间隔」。至少 10 秒（避免触发 provider rate limit）。",
    ),
  );
}

/// 渲染后批量调 loadCredentialStatus 拉每个 source 的 key 状态。
/// 跟 init() 里的 loadKeyStatus / loadTavilyKeyStatus / loadZenmuxKeyStatus
/// 等价，但走 id-based 统一接口。
export async function loadAllCredentialStatus(sources: SourceMeta[]) {
  await Promise.all(sources.map((s) => loadCredentialStatus(s.id)));
}