// P2 区域向导 + 语言切换 section —— 顶部两个 radio 一起展示（语言 + 区域），
// 共用一个 Apply 按钮，apply 时并发触发前端 setLocale + 后端 setRegion。
//
// 行为：
// - 语言 radio：zh-CN / en
//   - 调前端 setLocale(lang)（[../i18n]）—— 立刻重渲 DOM，emit locale-changed
//     → 后端 rebuild tray + 同步窗口 title
// - 区域 radio：CN / Global
//   - 调后端 setRegion(region) —— apply 默认 provider 顺序 + MiniMax endpoint
//   - 把 user_region 标 Custom
// - 两个独立决策：海外华人可以选 🇨🇳 CN + 🇺🇸 en；反之亦然
//
// 显示逻辑（v2 占位实现，未做折叠）：
// - user_region != Custom：section 完整显示 + apply 按钮
// - user_region == Custom：section 仍然显示，让用户随时能换语言

import { el, flash } from "./utils";
import { t, setLocale, getLocale } from "../i18n";
import { setRegion, getRegion } from "./api";
import type { AppConfig } from "./types";

export async function renderRegionSection(container: HTMLElement) {
  // M13 fix (2026-07-06 全量审查): 并发 getRegion + getLocale,避免 UI 显示
  // "新 locale + 旧 region" 的中间态(用户切换时 apply 刚触发就有 read 错位)。
  const [currentRegion, currentLocale] = await Promise.all([
    getRegion().catch(() => "cn"),
    Promise.resolve(getLocale()),
  ]);

  // ── 语言 radio ──
  const langOpts: Array<{ value: "zh-CN" | "en"; title: string }> = [
    { value: "zh-CN", title: t("settings.region.lang.zh-CN") },
    { value: "en",    title: t("settings.region.lang.en") },
  ];
  const langRadios = el("div", { class: "region-wizard" });
  for (const opt of langOpts) {
    const radio = el("input", {
      type: "radio",
      name: "ui-language",
      value: opt.value,
    }) as HTMLInputElement;
    if (currentLocale === opt.value) radio.checked = true;
    langRadios.appendChild(
      el("label", { class: "region-opt" },
        radio,
        el("div", { class: "region-opt-body" },
          el("div", { class: "region-opt-title" }, opt.title),
        ),
      ),
    );
  }

  // ── 区域 radio ──
  const regionOpts: Array<{ value: string; title: string; desc: string }> = [
    {
      value: "cn",
      title: t("settings.region.option_cn"),
      desc: t("settings.region.desc_cn"),
    },
    {
      value: "global",
      title: t("settings.region.option_global"),
      desc: t("settings.region.desc_global"),
    },
  ];
  const regionRadios = el("div", { class: "region-wizard" });
  for (const opt of regionOpts) {
    const radio = el("input", {
      type: "radio",
      name: "user-region",
      value: opt.value,
    }) as HTMLInputElement;
    if (currentRegion === opt.value) radio.checked = true;
    regionRadios.appendChild(
      el("label", { class: "region-opt" },
        radio,
        el("div", { class: "region-opt-body" },
          el("div", { class: "region-opt-title" }, opt.title),
          el("div", { class: "region-opt-desc" }, opt.desc),
        ),
      ),
    );
  }

  // ── Apply 按钮：一次 apply 两个设置（前端语言 + 后端区域）──
  const applyBtn = el("button", { class: "primary" }, t("settings.region.apply"));
  applyBtn.addEventListener("click", async () => {
    const selLang = (document.querySelector<HTMLInputElement>(
      "input[name=\"ui-language\"]:checked",
    )?.value as "zh-CN" | "en" | undefined) ?? "zh-CN";
    const selRegion = (document.querySelector<HTMLInputElement>(
      "input[name=\"user-region\"]:checked",
    )?.value) ?? "cn";
    applyBtn.disabled = true;
    try {
      // 并发触发：语言（前端）和区域（后端）独立，互不阻塞
      const tasks: Promise<unknown>[] = [];
      if (selLang !== currentLocale) tasks.push(setLocale(selLang));
      if (selRegion !== currentRegion) tasks.push(setRegion(selRegion));
      await Promise.all(tasks);
      flash(t("settings.region.applied"));
    } catch (e) {
      flash(t("settings.region.apply_failed", { err: String(e) }), true);
    } finally {
      applyBtn.disabled = false;
    }
  });

  container.appendChild(
    el("section", { class: "section-card", id: "region-section" },
      el("h2", {}, t("settings.region.section_title")),
      el("div", { class: "help" }, t("settings.region.help")),
      // 语言子组
      el("div", { class: "region-subgroup" },
        el("div", { class: "region-subgroup-title" }, t("settings.region.lang_label")),
        langRadios,
      ),
      // 区域子组
      el("div", { class: "region-subgroup" },
        el("div", { class: "region-subgroup-title" }, t("settings.region.region_label")),
        regionRadios,
      ),
      el("div", { class: "row" }, applyBtn),
    ),
  );
}

/// 在 settings panel 启动时检测：user_region != Custom → 在主面板顶部
/// 插一条 banner 提示用户去区域向导
///
/// M14 fix (2026-07-06 全量审查): 首启 (cfg.user_region == 'cn' 仍是 serde
/// 默认) 时,navigator.language 以 'en' / 'en-*' 开头 → 自动应用 Global
/// + emit apply 事件。US 用户无需手动点 Apply,CNC endpoint 不会再挂在
/// 配置面板上(老 bug: 美区用户安装后 MiniMax card 显示 'cn' endpoint 直接
/// fail)。
export async function maybeShowRegionBanner(cfg: AppConfig, app: HTMLElement) {
  if (cfg.user_region === "custom") return;

  // 首启检测:navigator.language 非 zh-* → 自动 apply Global
  const lang = (navigator?.language ?? "").toLowerCase();
  const isZh = lang.startsWith("zh");
  const isFirstLaunchCnDefault =
    cfg.user_region === "cn" &&
    Array.isArray(cfg.provider_order) &&
    cfg.provider_order.length === 0;
  if (isFirstLaunchCnDefault && !isZh && lang) {
    // fire-and-forget: 自动 setRegion('global');失败也无所谓,banner 继续
    setRegion("global").then(() =>
      flash(t("settings.region.auto_applied"), false),
    ).catch(() => {});
  }

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
