// "应用" section —— 跨 provider 的全局设置 + 托盘图标样式
//
// 内容：
// - 全局轮询间隔（秒）
// - 开机自启
// - **托盘图标样式**（v0.6+ 新增，3 选 1 radio：percent 默认 / bars / logo）
// - 「测试连接」按钮（拉一次所有 source + 摘要）

import { el, flash } from "./utils";
import { setTrayIconStyle, setTraySource, setTrayIconColor, getConfig, saveConfigSerialized } from "./api";
import { testConn } from "./test";
import { t } from "../i18n";
import type { AppConfig } from "./types";

export function renderAppSection(container: HTMLElement, cfg: AppConfig) {
  // ── 全局轮询间隔 ──
  // 2026-08-17 audit H-06: 之前只创建+回显，无 change handler → 死控件，改了静默丢失。
  // max 对齐后端 save_config 的 86_400 上限（原 3600 会误拦合法值）。走 getConfig→
  // mutate→saveConfig，与 providers.ts renderIntervalOverride 同款。
  const intervalInput = el("input", {
    type: "number",
    id: "interval",
    min: "10",
    max: "86400",
    value: String(cfg.refresh_interval_secs),
  }) as HTMLInputElement;
  intervalInput.addEventListener("change", async () => {
    const raw = intervalInput.value.trim();
    let secs: number;
    const n = parseInt(raw, 10);
    if (!Number.isFinite(n) || n < 10 || n > 86400) {
      flash(t("settings.providers.invalid_interval", { val: raw }), true);
      // 回滚到盘上当前值
      intervalInput.value = String(cfg.refresh_interval_secs);
      return;
    }
    secs = n;
    try {
      const latest = await getConfig();
      latest.refresh_interval_secs = secs;
      await saveConfigSerialized(latest);
      cfg.refresh_interval_secs = secs;
      flash(t("settings.app.refresh_interval_saved", { secs: String(secs) }));
    } catch (e) {
      flash(t("credentials.flash_save_failed", { err: String(e) }), true);
      intervalInput.value = String(cfg.refresh_interval_secs);
    }
  });

  // ── 开机自启 ──
  // 2026-08-17 audit H-06: 同样补 change handler。save_config 落盘后会同步
  // 调 tauri-plugin-autostart 的 enable/disable（commands/mod.rs:833-843）。
  const autostartCb = el("input", {
    type: "checkbox",
    id: "autostart",
  }) as HTMLInputElement;
  autostartCb.checked = cfg.autostart;
  autostartCb.addEventListener("change", async () => {
    const target = autostartCb.checked;
    try {
      const latest = await getConfig();
      latest.autostart = target;
      await saveConfigSerialized(latest);
      cfg.autostart = target;
      flash(target ? t("settings.app.autostart_enabled") : t("settings.app.autostart_disabled"));
    } catch (e) {
      // 失败回滚 checkbox 到盘上旧值
      autostartCb.checked = cfg.autostart;
      flash(t("credentials.flash_save_failed", { err: String(e) }), true);
    }
  });

  // ── 托盘图标样式 (3 选 1) ──
  // M32 fix (2026-09-05 audit)：currentStyle 改可变量，成功后同步 —— 原来
  // 是渲染时快照，第一次切换成功、第二次失败会回滚到**初始**值而不是最近
  // 成功值（floating.ts 的 M19 模式，此处漏了）。
  let currentStyle = cfg.tray_icon_style ?? "percent";
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
        .then(() => {
          currentStyle = opt.value; // M32 fix：只在成功后更新最近成功值
          flash(t("settings.app.tray_style_changed", { name: opt.title }));
        })
        .catch((e) => {
          flash(t("settings.app.tray_style_failed", { err: String(e) }), true);
          // 回滚所有 radio 到最近成功值
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
    // v0.2.9 火山双套餐：Agent 源（base 值 = Coding 优先 + fallback）
    { value: "volcengine_ark:agent", label: t("settings.app.tray_source.options.volcengine_ark_agent") },
    { value: "zhipu", label: t("settings.app.tray_source.options.zhipu") },
    { value: "claude_official", label: t("settings.app.tray_source.options.claude_official") },
    { value: "deepseek", label: t("settings.app.tray_source.options.deepseek") },
    { value: "openrouter", label: t("settings.app.tray_source.options.openrouter") },
    { value: "siliconflow", label: t("settings.app.tray_source.options.siliconflow") },
    { value: "zenmux", label: t("settings.app.tray_source.options.zenmux") },
    { value: "tokendance", label: t("settings.app.tray_source.options.tokendance") },
  ];
  // M32 fix (2026-09-05 audit)：同 tray-style，最近成功值可变 + 成功后更新。
  let currentSource = cfg.tray_source ?? "minimax";
  const traySourceSelect = el("select", { id: "tray-source" }) as HTMLSelectElement;
  for (const opt of traySourceOptions) {
    const o = el("option", { value: opt.value }, opt.label) as HTMLOptionElement;
    if (currentSource === opt.value) o.selected = true;
    traySourceSelect.appendChild(o);
  }
  traySourceSelect.addEventListener("change", () => {
    const v = traySourceSelect.value;
    void setTraySource(v)
      .then(() => {
        currentSource = v; // M32 fix
        flash(
          t("settings.app.tray_source_changed", {
            name: traySourceOptions.find((o) => o.value === v)?.label ?? v,
          }),
        );
      })
      .catch((e) => {
        flash(t("settings.app.tray_source_failed", { err: String(e) }), true);
        traySourceSelect.value = currentSource;
      });
  });

  // ── 托盘图标颜色 (方案 A: 选 tray icon 数字/进度条颜色) ──
  // 2026-09-08 第二轮：原生 <input type="color"> 在 WKWebView 判死刑 ——
  // NSColorPanel 打开/取色/关闭全程不派发 input/change（上一轮 input+change
  // 双听实测仍全哑，config.json 从未落盘过 tray_icon_color）。换纯 DOM 色板 +
  // hex 文本框，不依赖原生取色器。色板复用 extra-instance-form 的
  // .accent-swatch 样式；白/黑排最前（托盘字色两大真实用例：深菜单栏白字 /
  // 浅菜单栏黑字）。
  const TRAY_COLOR_PALETTE = [
    "#ffffff",
    "#000000",
    "#9b59ff",
    "#4a90e2",
    "#00d4a8",
    "#ff6a00",
  ];
  const HEX6_RE = /^#[0-9a-fA-F]{6}$/;
  // H-Frontend fix (2026-09-07 audit) 精神保留：非法存量值不展示成默认色。
  const initialTrayColor =
    cfg.tray_icon_color != null && HEX6_RE.test(cfg.tray_icon_color)
      ? cfg.tray_icon_color.toLowerCase()
      : null;
  const trayColorHexInput = el("input", {
    type: "text",
    id: "tray-color",
    placeholder: "#RRGGBB",
    autocomplete: "off",
    spellcheck: "false",
    style: "width: 96px;",
    value: initialTrayColor ?? "",
  });
  const trayColorSwatches = TRAY_COLOR_PALETTE.map((c) =>
    el("button", {
      type: "button",
      class: "accent-swatch",
      "data-color": c,
      style: `background: ${c};`,
      title: c,
    }),
  );
  const markTraySwatch = (color: string | null) => {
    for (const s of trayColorSwatches) {
      s.classList.toggle("selected", color !== null && s.dataset.color === color);
    }
  };
  markTraySwatch(initialTrayColor);
  const applyTrayColor = (color: string | null) => {
    void setTrayIconColor(color)
      .then(() => {
        markTraySwatch(color);
        trayColorHexInput.value = color ?? "";
        flash(t("settings.app.tray_color_changed"));
      })
      .catch((e) => flash(t("settings.app.tray_color_failed", { err: String(e) }), true));
  };
  for (const s of trayColorSwatches) {
    s.addEventListener("click", () => applyTrayColor(s.dataset.color ?? null));
  }
  // hex 文本框：Enter / 失焦提交。text 输入的 change 事件在 WKWebView 里可靠
  // （只有 color 类型的事件链是坏的）。#RRGGBB 严格校验；清空提交 = 切回自动。
  trayColorHexInput.addEventListener("change", () => {
    const v = trayColorHexInput.value.trim().toLowerCase();
    if (v === "") {
      applyTrayColor(null);
    } else if (HEX6_RE.test(v)) {
      applyTrayColor(v);
    } else {
      flash(t("settings.app.tray_color_failed", { err: trayColorHexInput.value }), true);
    }
  });
  const trayColorAutoBtn = el("button", { type: "button", class: "tray-color-auto" }, t("settings.app.tray_color_auto"));
  trayColorAutoBtn.addEventListener("click", () => applyTrayColor(null));

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
          el("div", { class: "accent-palette" }, ...trayColorSwatches),
          trayColorHexInput,
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
