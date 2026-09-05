// "浮窗" section —— pin mode + 归位 + 省电 + 全屏自动隐藏 + 显示阈值
//
// 这些是浮窗自身的视觉/行为设置，单独放一个 section 比塞在「数据源」底部
// 更合理。change 即时生效（已在 Stage 1 加了对应 IPC command）。

import { el, flash } from "./utils";
import {
  applyPinMode,
} from "./config";
import {
  getConfig,
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
  // **2026-06-20 audit**：之前 cfg.floating_pin_mode ?? "pin_top"，空字符串
  // 也会触发 silent fallback（?? 只查 null/undefined）。显式校验 union 后
  // 再默认，避免外部写脏 cfg 时被静默重置。
  const VALID_PIN_MODES: ReadonlySet<FloatingPinMode> = new Set([
    "pin_top",
    "pin_bottom",
    "normal",
  ]);
  const currentMode: FloatingPinMode = VALID_PIN_MODES.has(
    cfg.floating_pin_mode as FloatingPinMode,
  )
    ? (cfg.floating_pin_mode as FloatingPinMode)
    : "pin_top";
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
      if (!radio.checked) return;
      void applyPinMode(opt.value).then(async (ok) => {
        if (ok) return;
        // D8-03 (2026-09-04 audit): IPC 失败时 radio 已被浏览器翻到新值，
        // 后端仍是旧值 —— 重读 cfg 回滚，防 UI/后端状态分裂。
        try {
          const cur = await getConfig();
          const valid: FloatingPinMode = VALID_PIN_MODES.has(cur.floating_pin_mode as FloatingPinMode)
            ? (cur.floating_pin_mode as FloatingPinMode)
            : "pin_top";
          pinMode
            .querySelectorAll<HTMLInputElement>('input[name="pin-mode"]')
            .forEach((r) => {
              r.checked = r.value === valid;
            });
        } catch {
          radio.checked = false;
        }
      });
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
        // P0 fix: 之前 t() 不传 count，en.json 里的 '{count} providers' 占位符不被替换。
        // 改用固定描述：去掉花括号让 i18n 走字面量；中文用 1 个 provider 通用描述。
        el("div", { class: "help" }, t("settings.floating.footer_hint_help_no_placeholder")),
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
  // D8-16 (2026-09-04 audit): init 也跑 M33 顺序校验 —— 手编 config.json 的
  // 非递增阈值（后端只在 save_config 时拦）会把 input 初始化成非法值，之后
  // 用户任何微调都被"顺序无效"拦下且无从理解。非法则回落默认 + warn。
  const rawThresholds = cfg.color_thresholds ?? [50, 70, 88];
  const thresholdsValid =
    rawThresholds[0] < rawThresholds[1] && rawThresholds[1] < rawThresholds[2];
  if (!thresholdsValid) {
    console.warn("[floating] cfg.color_thresholds 非递增，回落默认 [50, 70, 88]", rawThresholds);
  }
  const [t0Init, t1Init, t2Init] = thresholdsValid ? rawThresholds : [50, 70, 88];
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
    // D8-09 (2026-09-04 audit): 只回填 <input type=color> 能 round-trip 的
    // #RRGGBB。手编 config 塞进非法值（"blue" / "rgb(...)"）或后端合法但
    // picker 不认的 3 位 hex 时，浏览器会把 input 静默 fallback 成
    // #000000 —— 下次 applyAll 就把存量色永久覆盖成黑。非法值回退默认色。
    const stored = overrides[key] ?? "";
    colorPickers[key] = el("input", {
      type: "color", id: `color-${key}`,
      value: /^#[0-9a-fA-F]{6}$/.test(stored) ? stored : DEFAULT_PALETTE[key],
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
  //
  // D8-04 (2026-09-04 audit): inflight 串行化 —— 3 个阈值 + wallet + 4 个
  // colorPicker 共用本函数且无防抖，连改会并发 fire 多个 IPC，读到的都是
  // 最新 input 值、后到请求覆盖先到，currentOverrides 也在每个成功回调里
  // 被中间值污染（失败回填用错"最近成功值"）。in-flight 期间的新触发合并
  // 成一次 trailing 调用。
  let applying = false;
  let pendingAgain = false;
  const applyAll = async () => {
    if (applying) {
      pendingAgain = true;
      return;
    }
    applying = true;
    try {
      await applyAllInner();
    } finally {
      applying = false;
      if (pendingAgain) {
        pendingAgain = false;
        void applyAll();
      }
    }
  };
  const applyAllInner = async () => {
    // M36 fix (2026-09-05 audit)：对齐后端校验 —— 后端 `[u8; 3]` +
    // `0 < t0 < t1 < t2 < 100`、`wallet >= 0`。此前前端只查顺序：t0=0 /
    // t2=150 落到后端才被拒（晦涩 template）；t0=-5 / wallet=-5 在 serde
    // 反序列化阶段炸出原始错误串。
    const v0 = Number(t0Input.value);
    const v1 = Number(t1Input.value);
    const v2 = Number(t2Input.value);
    if (![v0, v1, v2].every(Number.isInteger)) {
      flash(t("settings.floating.threshold_must_be_number"), true);
      return;
    }
    // M33 fix (2026-07-03 audit): 之前只校验是数字, 用户设 t0=80 t1=50 t2=88
    // (黄起点 > 红起点) 也能通过前端校验, 要等 IPC 往返后端拒绝才知道错。
    // 加客户端即时校验 t0 < t1 < t2, 错误立刻 flash 拦下。
    if (!(v0 > 0 && v2 < 100 && v0 < v1 && v1 < v2)) {
      flash(t("settings.floating.threshold_order_invalid"), true);
      return;
    }
    const wallet = walletCb.checked ? parseFloat(walletInput.value) : null;
    if (walletCb.checked && (!Number.isFinite(wallet) || (wallet ?? 0) < 0)) {
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
