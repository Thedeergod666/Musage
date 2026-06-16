// "应用" section —— 跨 provider 的全局设置 + 托盘图标样式
//
// 内容：
// - 全局轮询间隔（秒）
// - 开机自启
// - **托盘图标样式**（v0.6+ 新增，3 选 1 radio：percent 默认 / bars / logo）
// - 「测试连接」按钮（拉一次所有 source + 摘要）

import { el, flash } from "./utils";
import { setTrayIconStyle } from "./api";
import { testConn } from "./test";
import { t } from "../i18n";
import type { AppConfig } from "./types";

export function renderAppSection(container: HTMLElement, cfg: AppConfig) {
  // ── 全局轮询间隔 ──
  const intervalInput = el("input", {
    type: "number",
    id: "interval",
    min: "10",
    max: "3600",
    value: String(cfg.refresh_interval_secs),
  }) as HTMLInputElement;

  // ── 开机自启 ──
  const autostartCb = el("input", {
    type: "checkbox",
    id: "autostart",
  }) as HTMLInputElement;
  autostartCb.checked = cfg.autostart;

  // ── 托盘图标样式 (3 选 1) ──
  const currentStyle = cfg.tray_icon_style ?? "percent";
  const trayOptions: Array<{ value: "percent" | "bars" | "logo"; title: string; desc: string }> = [
    { value: "percent", title: t("settings.app.tray_options.percent.title"), desc: t("settings.app.tray_options.percent.desc") },
    { value: "bars", title: t("settings.app.tray_options.bars.title"), desc: t("settings.app.tray_options.bars.desc") },
    { value: "logo", title: t("settings.app.tray_options.logo.title"), desc: t("settings.app.tray_options.logo.desc") },
  ];
  const trayMode = el("div", { class: "tray-style" });
  for (const opt of trayOptions) {
    const radio = el("input", {
      type: "radio",
      name: "tray-style",
      value: opt.value,
    }) as HTMLInputElement;
    if (currentStyle === opt.value) radio.checked = true;
    radio.addEventListener("change", () => {
      if (!radio.checked) return;
      void setTrayIconStyle(opt.value)
        .then(() => flash(t("settings.app.tray_style_changed", { name: opt.title })))
        .catch((e) => {
          flash(t("settings.app.tray_style_failed", { err: String(e) }), true);
          // 回滚所有 radio 到 cfg 的旧值
          const oldRadio = document.querySelector<HTMLInputElement>(
            `input[name="tray-style"][value="${currentStyle}"]`,
          );
          if (oldRadio) oldRadio.checked = true;
        });
    });
    trayMode.appendChild(
      el("label", { class: "pin-opt" },
        radio,
        el("span", { class: "pin-opt-body" },
          el("span", { class: "pin-opt-title" }, opt.title),
          el("span", { class: "pin-opt-desc" }, opt.desc),
        ),
      ),
    );
  }

  // ── 测试连接按钮 ──
  const testBtn = el("button", { id: "test", class: "primary" }, t("settings.common.test")) as HTMLButtonElement;
  testBtn.addEventListener("click", () => void testConn());

  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, `⚙ ${t("settings.nav.app")}`),
      // 轮询间隔
      el("div", { class: "field" },
        el("label", { for: "interval" }, t("settings.app.refresh_interval")),
        intervalInput,
        el("div", { class: "help" }, t("settings.app.refresh_interval_help")),
      ),
      // 开机自启
      el("div", { class: "field" },
        el("div", { class: "check" },
          autostartCb,
          el("label", { for: "autostart" }, t("settings.app.autostart")),
        ),
      ),
      el("div", { class: "divider" }),
      // 托盘图标样式
      el("div", { class: "field" },
        el("label", {}, t("settings.app.tray_style_title")),
        trayMode,
        el("div", { class: "help" }, t("settings.app.tray_style_help")),
      ),
      el("div", { class: "divider" }),
      // 测试连接
      el("div", { class: "field" },
        el("div", { class: "row" }, testBtn),
        el("div", { class: "help" }, t("settings.updater.checking") /* placeholder, see real help */),
      ),
    ),
  );
}
