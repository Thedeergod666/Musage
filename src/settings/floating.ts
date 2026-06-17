// "浮窗" section —— pin mode + 归位 + 省电 + 全屏自动隐藏 + 显示阈值
//
// 这些是浮窗自身的视觉/行为设置，单独放一个 section 比塞在「数据源」底部
// 更合理。change 即时生效（已在 Stage 1 加了对应 IPC command）。

import { el, flash } from "./utils";
import {
  applyPinMode,
} from "./config";
import {
  setLowPowerMode,
  setAutoHideInFullscreen,
  resetFloatingWindow,
  setDisplayThresholds,
  setShowFooterHint,
} from "./api";
import { t } from "../i18n";
import type { AppConfig, FloatingPinMode } from "./types";

export function renderFloatingSection(container: HTMLElement, cfg: AppConfig) {
  // ── 置顶/置底/普通 单选 ──
  const currentMode: FloatingPinMode = cfg.floating_pin_mode ?? "pin_top";
  const pinMode = el("div", { class: "pin-mode" });
  const options: Array<{ value: FloatingPinMode; title: string; desc: string }> = [
    { value: "pin_top", title: t("settings.floating.pin_modes.top.title"), desc: t("settings.floating.pin_modes.top.desc") },
    { value: "pin_bottom", title: t("settings.floating.pin_modes.bottom.title"), desc: t("settings.floating.pin_modes.bottom.desc") },
    { value: "normal", title: t("settings.floating.pin_modes.normal.title"), desc: t("settings.floating.pin_modes.normal.desc") },
  ];
  for (const opt of options) {
    const radio = el("input", {
      type: "radio",
      name: "pin-mode",
      value: opt.value,
    }) as HTMLInputElement;
    if (currentMode === opt.value) radio.checked = true;
    radio.addEventListener("change", () => {
      if (radio.checked) void applyPinMode(opt.value);
    });
    pinMode.appendChild(
      el("label", { class: "pin-opt" },
        radio,
        el("span", { class: "pin-opt-body" },
          el("span", { class: "pin-opt-title" }, opt.title),
          el("span", { class: "pin-opt-desc" }, opt.desc),
        ),
      ),
    );
  }

  // ── 归位按钮 ──
  const resetBtn = el("button", { id: "reset-floating", class: "primary" }, t("settings.floating.reset_to_center")) as HTMLButtonElement;
  resetBtn.addEventListener("click", () => {
    resetBtn.disabled = true;
    void resetFloatingWindow()
      .then(() => flash(t("settings.floating.reset_done")))
      .catch((e) => flash(t("settings.floating.reset_failed", { err: String(e) }), true))
      .finally(() => { resetBtn.disabled = false; });
  });

  // ── 省电模式 checkbox ──
  const lowPowerCb = el("input", {
    type: "checkbox",
    id: "low-power-mode",
  }) as HTMLInputElement;
  lowPowerCb.checked = cfg.low_power_mode ?? false;
  lowPowerCb.addEventListener("change", () => {
    const enabled = lowPowerCb.checked;
    void setLowPowerMode(enabled)
      .then(() => flash(enabled ? t("settings.floating.low_power_on") : t("settings.floating.low_power_off")))
      .catch((e) => flash(t("settings.floating.toggle_failed", { err: String(e) }), true));
  });

  // ── 全屏自动隐藏 checkbox ──
  const autoHideCb = el("input", {
    type: "checkbox",
    id: "auto-hide-in-fullscreen",
  }) as HTMLInputElement;
  autoHideCb.checked = cfg.auto_hide_in_fullscreen ?? false;
  autoHideCb.addEventListener("change", () => {
    const enabled = autoHideCb.checked;
    void setAutoHideInFullscreen(enabled)
      .then(() => flash(enabled ? t("settings.floating.auto_hide_on") : t("settings.floating.auto_hide_off")))
      .catch((e) => flash(t("settings.floating.toggle_failed", { err: String(e) }), true));
  });

  // ── 底部提示行 checkbox ──
  const footerHintCb = el("input", {
    type: "checkbox",
    id: "show-footer-hint",
  }) as HTMLInputElement;
  footerHintCb.checked = cfg.show_footer_hint ?? false;
  footerHintCb.addEventListener("change", () => {
    const enabled = footerHintCb.checked;
    void setShowFooterHint(enabled)
      .then(() => flash(enabled ? t("settings.floating.footer_hint_on") : t("settings.floating.footer_hint_off")))
      .catch((e) => flash(t("settings.floating.toggle_failed", { err: String(e) }), true));
  });

  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, `🪟 ${t("settings.floating.section_title")}`),
      // 置顶模式
      el("div", { class: "field" },
        el("label", {}, t("settings.floating.pin_mode_title")),
        pinMode,
        el("div", { class: "help" }, t("settings.floating.pin_mode_help")),
      ),
      // 归位
      el("div", { class: "field" },
        el("div", { class: "row" }, resetBtn),
        el("div", { class: "help" }, t("settings.floating.position_help")),
      ),
      // 省电模式
      el("div", { class: "field" },
        el("div", { class: "check" },
          lowPowerCb,
          el("label", { for: "low-power-mode" }, t("settings.floating.low_power_label")),
        ),
        el("div", { class: "help" }, t("settings.floating.low_power_help")),
      ),
      // 全屏自动隐藏
      el("div", { class: "field" },
        el("div", { class: "check" },
          autoHideCb,
          el("label", { for: "auto-hide-in-fullscreen" }, t("settings.floating.auto_hide_label")),
        ),
        el("div", { class: "help" }, t("settings.floating.auto_hide_help")),
      ),
      // 底部提示行
      el("div", { class: "field" },
        el("div", { class: "check" },
          footerHintCb,
          el("label", { for: "show-footer-hint" }, t("settings.floating.footer_hint_label")),
        ),
        el("div", { class: "help" }, t("settings.floating.footer_hint_help")),
      ),
      el("div", { class: "divider" }),
      // ── 颜色档位阈值（v0.6+ 用户可调） ──
      ...renderDisplayThresholdsFields(cfg),
    ),
  );
}

/// 「颜色档位阈值」+「自定义 4 档色」+「钱包余额告警」三个相关配置。
///
/// 全部走 set_display_thresholds 单字段 command（参考 set_low_power_mode
/// 的"勾选即生效"模式），不依赖"保存"按钮。Rust 端会校验 t0<t1<t2<100 、
/// wallet ≥ 0、color key ∈ {ok,cyan,warn,alert} 且 value 是 #RGB/#RRGGBB，
/// 失败回退到旧值 + flash 报错。
function renderDisplayThresholdsFields(cfg: AppConfig) {
  // ── 颜色档位阈值（3 个 number input） ──
  const [t0Init, t1Init, t2Init] = cfg.color_thresholds ?? [50, 70, 88];
  // M19 fix: 之前是 const [t0, t1, t2] = ...，applyAll 失败时回填旧值但旧值永远是
  // 初始 cfg 拷贝。改成 mutable 数组，成功后更新它，失败回填用"最近一次成功值"。
  const currentThresholds: [number, number, number] = [t0Init, t1Init, t2Init];
  const t0Input = el("input", {
    type: "number", id: "color-t0", min: "0", max: "99", step: "1",
    value: String(currentThresholds[0]), title: t("settings.floating.threshold_t0_title"),
  }) as HTMLInputElement;
  const t1Input = el("input", {
    type: "number", id: "color-t1", min: "0", max: "99", step: "1",
    value: String(currentThresholds[1]), title: t("settings.floating.threshold_t1_title"),
  }) as HTMLInputElement;
  const t2Input = el("input", {
    type: "number", id: "color-t2", min: "0", max: "99", step: "1",
    value: String(currentThresholds[2]), title: t("settings.floating.threshold_t2_title"),
  }) as HTMLInputElement;

  // ── 4 档自定义色（4 个 color picker） ──
  // iOS 系统默认色（与 main.ts::DEFAULT_PALETTE + styles.css 保持一致）
  const DEFAULT_PALETTE: Record<"ok" | "cyan" | "warn" | "alert", string> = {
    ok: "#30d158",
    cyan: "#5ac8fa",
    warn: "#ff9f0a",
    alert: "#ff453a",
  };
  const colorKeys = ["ok", "cyan", "warn", "alert"] as const;
  const colorLabels: Record<typeof colorKeys[number], string> = {
    ok: t("settings.floating.color_ok"),
    cyan: t("settings.floating.color_cyan"),
    warn: t("settings.floating.color_warn"),
    alert: t("settings.floating.color_alert"),
  };
  const overrides = cfg.color_overrides ?? {};
  // M19 fix: mutable 副本，applyAll 成功后写入，失败回填用最近成功值
  let currentOverrides: Record<string, string> = { ...overrides };
  let currentWallet: number | null = cfg.wallet_alert_threshold ?? null;
  const colorPickers: Record<typeof colorKeys[number], HTMLInputElement> = {} as any;
  for (const key of colorKeys) {
    colorPickers[key] = el("input", {
      type: "color", id: `color-${key}`,
      value: overrides[key] ?? DEFAULT_PALETTE[key],
    }) as HTMLInputElement;
    colorPickers[key].addEventListener("change", () => void applyAll());
  }

  // ── 钱包告警（默认关闭） ──
  const walletCb = el("input", { type: "checkbox", id: "wallet-alert-enabled" }) as HTMLInputElement;
  const walletInput = el("input", {
    type: "number", id: "wallet-alert-threshold", min: "0", step: "0.01",
    placeholder: "2",
  }) as HTMLInputElement;
  walletCb.checked = cfg.wallet_alert_threshold != null;
  walletInput.value = cfg.wallet_alert_threshold != null
    ? String(cfg.wallet_alert_threshold)
    : "";
  walletInput.disabled = !walletCb.checked;

  // ── 共享"立即应用"动作 ──
  // 一次性从所有 input 读 → 调 setDisplayThresholds。
  // 失败时 flash 报错 + 回填旧值（不阻塞其他 input 的后续修改）。
  const applyAll = async () => {
    const v0 = parseInt(t0Input.value, 10);
    const v1 = parseInt(t1Input.value, 10);
    const v2 = parseInt(t2Input.value, 10);
    if (![v0, v1, v2].every(Number.isFinite)) {
      flash(t("settings.floating.threshold_must_be_number"), true);
      return;
    }
    const wallet = walletCb.checked ? parseFloat(walletInput.value) : null;
    if (walletCb.checked && !Number.isFinite(wallet)) {
      flash(t("settings.floating.wallet_must_be_number"), true);
      return;
    }
    // 只把"非默认色"的项加进 overrides（保持 config.json 干净）
    const newOverrides: Record<string, string> = {};
    for (const key of colorKeys) {
      const v = colorPickers[key].value.toLowerCase();
      if (v !== DEFAULT_PALETTE[key].toLowerCase()) {
        newOverrides[key] = v;
      }
    }
    try {
      await setDisplayThresholds([v0, v1, v2], wallet, newOverrides);
      // M19 fix: 成功后更新 currentThresholds / currentWallet / currentOverrides，
      // 失败回填用"最近一次成功值"而不是 init 时的 cfg 拷贝
      currentThresholds[0] = v0;
      currentThresholds[1] = v1;
      currentThresholds[2] = v2;
      currentWallet = wallet;
      currentOverrides = newOverrides;
      flash(t("settings.floating.display_saved"));
    } catch (e) {
      flash(t("settings.floating.display_save_failed", { err: String(e) }), true);
      // 回填最近一次成功值
      t0Input.value = String(currentThresholds[0]);
      t1Input.value = String(currentThresholds[1]);
      t2Input.value = String(currentThresholds[2]);
      walletInput.value = currentWallet != null ? String(currentWallet) : "";
      for (const key of colorKeys) {
        colorPickers[key].value = currentOverrides[key] ?? DEFAULT_PALETTE[key];
      }
    }
  };

  // ── 事件绑定 ──
  for (const input of [t0Input, t1Input, t2Input]) {
    input.addEventListener("change", () => void applyAll());
  }
  walletCb.addEventListener("change", () => {
    walletInput.disabled = !walletCb.checked;
    void applyAll();
  });
  walletInput.addEventListener("change", () => {
    if (walletCb.checked) void applyAll();
  });

  // ── "全部重置"按钮：阈值 / 自定义色 / 钱包告警 一次性还原到出厂值 ──
  const resetAllBtn = el("button", { class: "primary", id: "reset-all-display" },
    t("settings.floating.reset_all")) as HTMLButtonElement;
  resetAllBtn.addEventListener("click", () => {
    t0Input.value = "50";
    t1Input.value = "70";
    t2Input.value = "88";
    for (const key of colorKeys) {
      colorPickers[key].value = DEFAULT_PALETTE[key];
    }
    walletCb.checked = false;
    walletInput.value = "";
    walletInput.disabled = true;
    void applyAll();
  });

  // 4 个 color picker 一行排开，每个右边带 label
  const colorRow = el("div", {
    class: "row",
    style: "display: flex; gap: 10px; align-items: center; flex-wrap: wrap;",
  });
  for (const key of colorKeys) {
    colorRow.appendChild(
      el("label", {
        style: "display: inline-flex; align-items: center; gap: 4px; font-size: 11px;",
      },
        colorPickers[key],
        el("span", {}, colorLabels[key]),
      ),
    );
  }

  return [
    el("div", { class: "field" },
      el("label", {}, t("settings.floating.color_thresholds_label")),
      el("div", { class: "row", style: "display: flex; gap: 6px; align-items: center;" },
        t0Input, el("span", {}, t("settings.floating.threshold_arrow")), t1Input, el("span", {}, t("settings.floating.threshold_arrow")), t2Input,
        el("span", { style: "color: var(--text-faint); margin-left: 6px; font-size: 11px;" },
          t("settings.floating.tier_labels")),
      ),
      el("div", { class: "help" }, t("settings.floating.color_thresholds_help")),
    ),
    el("div", { class: "field" },
      el("label", {}, t("settings.floating.color_custom_label")),
      colorRow,
      el("div", { class: "help" }, t("settings.floating.color_custom_help")),
    ),
    el("div", { class: "field" },
      el("label", {}, t("settings.floating.wallet_label")),
      el("div", { class: "check" },
        walletCb,
        el("label", { for: "wallet-alert-enabled" }, t("settings.floating.wallet_enable")),
        walletInput,
      ),
      el("div", { class: "help" }, t("settings.floating.wallet_help")),
    ),
    el("div", { class: "field" },
      el("div", { class: "row" }, resetAllBtn),
      el("div", { class: "help" }, t("settings.floating.reset_all_help")),
    ),
  ];
}
