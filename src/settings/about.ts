// "关于" section —— 版本 + 仓库链接 + updater 注入点
//
// v0.6+ 起 updater section 跟着 about section 走，不再插在「保存配置」按钮前。
// setupUpdaterSection() 在 main.ts init 末尾调一次（DOM 元素已经存在）。

import { el } from "./utils";
import { getAppVersion } from "./api";

export async function renderAboutSection(container: HTMLElement) {
  let version = "—";
  try {
    version = await getAppVersion();
  } catch {
    // ignore
  }

  container.appendChild(
    el("section", { class: "section-card", id: "about-section" },
      el("h2", {}, "ℹ 关于"),
      el("div", { class: "field" },
        el("label", {}, "Musage"),
        el("div", { class: "help" },
          "多 Provider 实时用量监控悬浮窗。当前版本：",
          el("strong", { id: "updater-current-version" }, `v${version}`),
        ),
      ),
      // updater section 由 setupUpdaterSection() 注入到这里
      el("div", { id: "updater-section" }),
      el("div", { class: "field" },
        el("label", {}, "链接"),
        el("div", { class: "help" },
          "源码：",
          el("a", { href: "https://github.com/Thedeergod666/musage", target: "_blank", class: "link-ext" }, "github.com/Thedeergod666/musage"),
          el("br"),
          "问题反馈：同上 GitHub issues",
        ),
      ),
    ),
  );
}
