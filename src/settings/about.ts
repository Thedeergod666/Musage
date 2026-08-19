// "关于" section —— 版本 + GitHub 新版本检查 + 仓库链接
//
// A 选项（用户决策 2026-08-19）：设置页显示 GitHub releases 有无新版本，
// 但不做应用内自动更新（macOS 签名 + notarize 链路没稳定前会触发
// 「应用已损坏」）。详细见 [src-tauri/src/commands/updater_check.rs] +
// RELEASING.md 第 6 章。
//
// 触发时机（用户决策）：
// - 启动 5s 后后端 spawn 一次探测，结果写模块私有 cache
// - 设置页打开 about section 时 → checkForUpdate(false)：同步返 cache
//   + 缓存空时后端自动 spawn 后台 fetch
// - 「检查更新」按钮 → checkForUpdate(true)：强制 await fetch
// pre-release 不算（GitHub /releases/latest 端点天然只返 stable）。

import { el, flash } from "./utils";
import { checkForUpdate, getAppVersion, type UpdateInfo } from "./api";
import { dumpMissingKeys, t } from "../i18n";

export async function renderAboutSection(container: HTMLElement) {
  let version = "—";
  try {
    version = await getAppVersion();
  } catch {
    // ignore
  }

  // 升级段节点 —— 闭包内共享 bannerSlot / checkStatus，避免 getElementById
  // 反查。所有 setter 直接拿这些引用 setTextContent / replaceChildren。
  // bannerSlot 默认空 div, 有新版本时由 renderUpdateBanner 插入 .update-banner。
  const bannerSlot = el("div");
  const checkStatus = el("span", { class: "about-check-status" });
  const checkBtn = el("button", {
    class: "btn-secondary",
    type: "button",
  }, t("settings.about.upgrade_check_update_btn"));

  // 集中渲染结果：把 fetch 完的 Option<UpdateInfo> 转成 UI 状态。
  // error 走 flash（user-initiated 路径的强反馈），静默路径不 flash。
  function renderResult(info: UpdateInfo | null, opts: { flashOnError: boolean }): void {
    if (info) {
      renderUpdateBanner(bannerSlot, info);
      checkStatus.textContent = t("settings.about.upgrade_new_version_available", {
        version: info.latest_version,
      });
    } else {
      clearUpdateBanner(bannerSlot);
      checkStatus.textContent = t("settings.about.upgrade_up_to_date");
    }
    void opts; // 标记参数使用, 防止 strict-mode unused 警告
  }

  function renderError(err: unknown, opts: { flashOnError: boolean }): void {
    const msg = String(err);
    // 失败时保留旧 banner —— 网络抖一次不该让用户觉得新版本消失了。
    checkStatus.textContent = t("settings.about.upgrade_check_failed", { err: msg });
    if (opts.flashOnError) {
      flash("err", t("settings.about.upgrade_check_failed", { err: msg }));
    }
    console.warn("[about] checkForUpdate 失败", err);
  }

  // 统一 fetch + render 入口，替代之前的 runManualCheck / runCheckSilent
  // 两个近似重复函数。差异只有 force + 是否 disable button + 是否 flash 错误。
  async function runCheck(force: boolean, opts: { disableBtn: boolean; flashOnError: boolean }): Promise<void> {
    if (opts.disableBtn) {
      checkBtn.setAttribute("disabled", "true");
    }
    checkStatus.textContent = t("settings.about.upgrade_checking");
    try {
      const info = await checkForUpdate(force);
      renderResult(info, { flashOnError: opts.flashOnError });
    } catch (e) {
      renderError(e, { flashOnError: opts.flashOnError });
    } finally {
      if (opts.disableBtn) {
        checkBtn.removeAttribute("disabled");
      }
    }
  }

  checkBtn.addEventListener("click", () => {
    void runCheck(true, { disableBtn: true, flashOnError: true });
  });

  // 升级段 field —— banner + 按钮 + 静态 GitHub 链接 hint
  const upgradeField = el("div", { class: "field" },
    el("label", {}, t("settings.about.upgrade")),
    bannerSlot,
    el("div", { class: "about-check-row" },
      checkBtn,
      checkStatus,
    ),
    el("div", { class: "help" },
      t("settings.about.upgrade_hint"),
      el("a", {
        href: "https://github.com/Thedeergod666/Musage/releases/latest",
        target: "_blank",
        class: "link-ext",
        rel: "noopener noreferrer",
      }, "github.com/Thedeergod666/Musage/releases/latest"),
    ),
  );

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
      upgradeField,
      el("div", { class: "field" },
        el("label", {}, t("settings.about.links")),
        el("div", { class: "help" },
          t("settings.about.source"),
          el("a", { href: "https://github.com/Thedeergod666/musage", target: "_blank", class: "link-ext", rel: "noopener noreferrer" }, "github.com/Thedeergod666/musage"),
          el("br"),
          t("settings.about.feedback"),
        ),
      ),
    ),
  );

  // 首次渲染：force=false 立刻拿缓存 + 后端自动 spawn 后台 fetch（如果
  // 缓存空）。设置页打开瞬间不阻塞 UI。
  void runCheck(false, { disableBtn: false, flashOnError: false });

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

/** 在 bannerSlot 里渲染"有新版本"的横幅 + 跳 GitHub releases 的链接。 */
function renderUpdateBanner(slot: HTMLElement, info: UpdateInfo) {
  // replaceChildren 而不是 innerHTML = "" 避免残留子节点导致多次渲染时 DOM 累积
  slot.replaceChildren(
    el("div", { class: "update-banner" },
      el("strong", {}, t("settings.about.upgrade_new_version_available", {
        version: info.latest_version,
      })),
      " — ",
      el("a", {
        href: info.html_url,
        target: "_blank",
        class: "link-ext",
        rel: "noopener noreferrer",
      }, t("settings.about.upgrade_open_release")),
    ),
  );
}

/** 清掉 bannerSlot（检查结果是"无新版本"或出错时）。 */
function clearUpdateBanner(slot: HTMLElement) {
  slot.replaceChildren();
}
