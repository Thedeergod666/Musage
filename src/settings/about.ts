// "关于" section —— 版本 + 仓库链接
//
// v0.2.0 起不再有 updater section —— 升级走"用户手动下 dmg/nsis 装"路径。
// 详细见 RELEASING.md 第 6 章。

import { el } from "./utils";
import { getAppVersion } from "./api";
import { t } from "../i18n";

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
}
