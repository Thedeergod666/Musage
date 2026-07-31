// "关于" section —— 版本 + 仓库链接
//
// v0.2.0 起不再有 updater section —— 升级走"用户手动下 dmg/nsis 装"路径。
// 详细见 RELEASING.md 第 6 章。

import { el, flash } from "./utils";
import { getAppVersion } from "./api";
import { dumpMissingKeys, t } from "../i18n";

export async function renderAboutSection(container: HTMLElement) {
  let version = "—";
  try {
    version = await getAppVersion();
  } catch {
    // ignore
  }

  container.appendChild(
    el("section", { class: "section-card", id: "about-section" },
      el("h2", {}, `${t("settings.about.section_prefix")}${t("settings.about.section_title")}`),
      el("div", { class: "field" },
        el("label", {}, "Musage"),
        el("div", { class: "help" },
          t("settings.about.description"),
          t("settings.about.current_version"),
          el("strong", {}, `v${version}`),
        ),
      ),
      // 升级提示 —— 走 GitHub releases 页手动下新版
      el("div", { class: "field" },
        el("label", {}, t("settings.about.upgrade")),
        el("div", { class: "help" },
          t("settings.about.upgrade_hint"),
          el("a", { href: "https://github.com/Thedeergod666/Musage/releases/latest", target: "_blank", class: "link-ext" }, "github.com/Thedeergod666/Musage/releases/latest"),
        ),
      ),
      el("div", { class: "field" },
        el("label", {}, t("settings.about.links")),
        el("div", { class: "help" },
          t("settings.about.source"),
          el("a", { href: "https://github.com/Thedeergod666/musage", target: "_blank", class: "link-ext" }, "github.com/Thedeergod666/musage"),
          el("br"),
          t("settings.about.feedback"),
        ),
      ),
    ),
  );

  // D8-007 fix (2026-07-30 audit): dev 模式追加 i18n missing-key dump 按钮。
  // dumpMissingKeys() 由 src/i18n/index.ts 提供, 生产 build 里
  // import.meta.env.DEV = false 整段消失 (Vite dead-code-eliminate)。
  if ((import.meta as any).env?.DEV) {
    const dumpBtn = el("button", { class: "btn-secondary", id: "dev-dump-missing-keys" },
      t("settings.about.dump_missing_keys"),
    );
    dumpBtn.addEventListener("click", () => {
      const keys = dumpMissingKeys();
      // 顺手 console.log, dev tools 直接展开; flash 给视觉反馈
      // eslint-disable-next-line no-console
      console.log("[musage dev] missing i18n keys:", keys);
      flash(keys.length === 0
        ? t("settings.about.dump_missing_keys_empty")
        : t("settings.about.dump_missing_keys_done", { count: keys.length }));
    });
    container.appendChild(
      el("section", { class: "section-card", id: "about-dev-section" },
        el("h2", {}, `${t("settings.about.section_prefix")}${t("settings.about.dev_menu_title")}`),
        el("div", { class: "field" },
          el("label", {}, t("settings.about.dev_menu_hint")),
          el("div", { class: "help" }, t("settings.about.dev_menu_body")),
          dumpBtn,
        ),
      ),
    );
  }
}
