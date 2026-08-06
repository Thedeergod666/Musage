// "应用" section —— 跨 provider 的全局设置 + 托盘图标样式
//
// 内容：
// - 全局轮询间隔（秒）
// - 开机自启
// - **托盘图标样式**（v0.6+ 新增，3 选 1 radio：percent 默认 / bars / logo）
// - 「测试连接」按钮（拉一次所有 source + 摘要）

import { el, flash } from "./utils";
import { setTrayIconStyle, setTraySource, setTrayIconColor } from "./api";
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
  // ── 托盘数据源 (方案 A: 选 tray icon 显示哪个 provider) ──
  const traySourceOptions = [
    { value: "minimax", label: t("settings.app.tray_source.options.minimax") },
    { value: "kimi", label: t("settings.app.tray_source.options.kimi") },
    { value: "volcengine_ark", label: t("settings.app.tray_source.options.volcengine_ark") },
    { value: "zhipu", label: t("settings.app.tray_source.options.zhipu") },
    { value: "claude_official", label: t("settings.app.tray_source.options.claude_official") },
    { value: "deepseek", label: t("settings.app.tray_source.options.deepseek") },
    { value: "openrouter", label: t("settings.app.tray_source.options.openrouter") },
    { value: "siliconflow", label: t("settings.app.tray_source.options.siliconflow") },
    { value: "zenmux", label: t("settings.app.tray_source.options.zenmux") },
  ];
  const currentSource = cfg.tray_source ?? "minimax";
  const traySourceSelect = el("select", { id: "tray-source" }) as HTMLSelectElement;
  for (const opt of traySourceOptions) {
    const o = el("option", { value: opt.value }, opt.label) as HTMLOptionElement;
    if (currentSource === opt.value) o.selected = true;
    traySourceSelect.appendChild(o);
  }
  traySourceSelect.addEventListener("change", () => {
    const v = traySourceSelect.value;
    void setTraySource(v)
      .then(() =>
        flash(
          t("settings.app.tray_source_changed", {
            name: traySourceOptions.find((o) => o.value === v)?.label ?? v,
          }),
        ),
      )
      .catch((e) => {
        flash(t("settings.app.tray_source_failed", { err: String(e) }), true);
        traySourceSelect.value = currentSource;
      });
  });

  // ── 托盘图标颜色 (方案 A: 选 tray icon 数字/进度条颜色) ──
  const trayColorInput = el("input", { type: "color", id: "tray-color" }) as HTMLInputElement;
  trayColorInput.value = cfg.tray_icon_color ?? "#ffffff";
  const trayColorAutoBtn = el("button", { type: "button", class: "tray-color-auto" }, t("settings.app.tray_color_auto")) as HTMLButtonElement;
  trayColorInput.addEventListener("change", () => {
    void setTrayIconColor(trayColorInput.value)
      .then(() => flash(t("settings.app.tray_color_changed")))
      .catch((e) => flash(t("settings.app.tray_color_failed", { err: String(e) }), true));
  });
  trayColorAutoBtn.addEventListener("click", () => {
    void setTrayIconColor(null)
      .then(() => {
        trayColorInput.value = "#ffffff";
        flash(t("settings.app.tray_color_changed"));
      })
      .catch((e) => flash(t("settings.app.tray_color_failed", { err: String(e) }), true));
  });

  const testBtn = el("button", { id: "test", class: "primary" }, t("settings.common.test")) as HTMLButtonElement;
  testBtn.addEventListener("click", () => void testConn());

  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, t("settings.app.section_title")),
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
      // 托盘数据源（方案 A）
      el("div", { class: "field" },
        el("label", { for: "tray-source" }, t("settings.app.tray_source_title")),
        traySourceSelect,
        el("div", { class: "help" }, t("settings.app.tray_source_help")),
      ),
      el("div", { class: "divider" }),
      // 托盘图标颜色
      el("div", { class: "field" },
        el("label", { for: "tray-color" }, t("settings.app.tray_color_title")),
        el("div", { class: "row" },
          trayColorInput,
          trayColorAutoBtn,
        ),
        el("div", { class: "help" }, t("settings.app.tray_color_help")),
      ),
      el("div", { class: "divider" }),
      // 测试连接
      el("div", { class: "field" },
        el("div", { class: "row" }, testBtn),
        el("div", { class: "help" }, t("settings.test.app_help")),
      ),
    ),
  );
}
