// "关于" section —— 版本 + 仓库链接 + updater 注入点
//
// v0.6+ 起 updater section 跟着 about section 走，不再插在「保存配置」按钮前。
// setupUpdaterSection() 在 main.ts init 末尾调一次（DOM 元素已经存在）。

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
          el("strong", { id: "updater-current-version" }, `v${version}`),
        ),
      ),
      // updater section 由 setupUpdaterSection() 注入到这里
      el("div", { id: "updater-section" }),
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
