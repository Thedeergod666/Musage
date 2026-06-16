// P2 区域向导 section —— 首次启动或 user_region != Custom 时显示
//
// 行为：
// - 3 radio（CN / Global / Custom）— 当前值高亮
// - 选完点 "应用" → 调 set_region 后端命令 → 后端 apply 默认 provider 顺序
//   + MiniMax endpoint (CN/EN) + 把 user_region 标 Custom
// - 选 "Custom" 不动 cfg.user_region（已是 Custom）
//
// 显示逻辑：
// - user_region == Custom：整个 section 折叠成 "当前: 自定义" + "重选" 按钮
// - user_region == Cn / Global：完整 3 radio + apply 按钮

import { el, flash } from "./utils";
import { t } from "../i18n";
import { setRegion, getRegion } from "./api";
import type { AppConfig } from "./types";

export async function renderRegionSection(container: HTMLElement) {
  let currentRegion = "cn";
  try {
    currentRegion = await getRegion();
  } catch {
    // 启动时 cfg 还没读 → 忽略，render 后用 cfg fallback
  }

  const opts: Array<{ value: string; emoji: string; title: string; desc: string }> = [
    {
      value: "cn",
      emoji: "🇨🇳",
      title: t("settings.region.option_cn"),
      desc: t("settings.region.desc_cn"),
    },
    {
      value: "global",
      emoji: "🌍",
      title: t("settings.region.option_global"),
      desc: t("settings.region.desc_global"),
    },
  ];

  const radios = el("div", { class: "region-wizard" });
  for (const opt of opts) {
    const radio = el("input", {
      type: "radio",
      name: "user-region",
      value: opt.value,
    }) as HTMLInputElement;
    if (currentRegion === opt.value) radio.checked = true;
    radios.appendChild(
      el("label", { class: "region-opt" },
        radio,
        el("div", { class: "region-opt-body" },
          el("div", { class: "region-opt-title" }, `${opt.emoji} ${opt.title}`),
          el("div", { class: "region-opt-desc" }, opt.desc),
        ),
      ),
    );
  }

  const applyBtn = el("button", { class: "primary" }, t("settings.region.apply"));
  applyBtn.addEventListener("click", async () => {
    const sel = (document.querySelector<HTMLInputElement>(
      "input[name=\"user-region\"]:checked",
    )?.value) ?? "cn";
    applyBtn.disabled = true;
    try {
      await setRegion(sel);
      flash(t("settings.region.applied"));
      // 后端已经 emit "musage://config-changed"，main.ts 监听会重读 cfg
      // 刷新各 section（除本 section 外，因为 wizard 已经折叠）
    } catch (e) {
      flash(`✗ ${e}`, true);
    } finally {
      applyBtn.disabled = false;
    }
  });

  container.appendChild(
    el("section", { class: "section-card", id: "region-section" },
      el("h2", {}, `🌐 ${t("settings.region.section_title")}`),
      el("div", { class: "help" }, t("settings.region.help")),
      radios,
      el("div", { class: "row" }, applyBtn),
    ),
  );
}

/// 在 settings panel 启动时检测：user_region != Custom → 在主面板顶部
/// 插一条 banner 提示用户去区域向导
export async function maybeShowRegionBanner(cfg: AppConfig, app: HTMLElement) {
  if (cfg.user_region === "custom") return;
  const banner = el("div", { class: "region-banner" },
    el("span", {}, t("settings.region.banner")),
    el("button", { class: "primary" }, t("settings.region.go_to_wizard")),
  );
  banner.querySelector("button")?.addEventListener("click", () => {
    const sec = document.getElementById("region-section");
    if (sec) sec.scrollIntoView({ behavior: "smooth" });
  });
  app.insertBefore(banner, app.firstChild);
}
