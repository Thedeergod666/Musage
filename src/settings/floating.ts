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
  setFloatingFitBottomMargin,
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
  // H-Frontend-3 fix (2026-09-28 audit)：此前 3 个 checkbox 的 `.catch` 只有
  // flash，**没有把 cb.checked 还原**（同目录 app.ts 的开机自启是有回滚的）。
  // 触发：IPC 失败 → 红条「切换失败」但勾还留在新值、后端仍是旧值，用户以为
  // 改成功，重开面板才发现没生效。改成 app.ts 的 lastGood 闭包可变模式。
  let lastGoodLowPower = cfg.low_power_mode ?? false;
  const lowPowerCb = el("input", {
    type: "checkbox",
    id: "low-power-mode",
  }) as HTMLInputElement;
  lowPowerCb.checked = lastGoodLowPower;
  lowPowerCb.addEventListener("change", () => {
    const enabled = lowPowerCb.checked;
    void setLowPowerMode(enabled)
      .then(() => {
        lastGoodLowPower = enabled; // 只在成功后更新最近成功值
        flash(enabled ? t("settings.floating.low_power_on") : t("settings.floating.low_power_off"));
      })
      .catch((e) => {
        lowPowerCb.checked = lastGoodLowPower;
        flash(t("settings.floating.toggle_failed", { err: String(e) }), true);
      });
  });

  // ── 全屏自动隐藏 checkbox ──
  let lastGoodAutoHide = cfg.auto_hide_in_fullscreen ?? false;
  const autoHideCb = el("input", {
    type: "checkbox",
    id: "auto-hide-in-fullscreen",
  }) as HTMLInputElement;
  autoHideCb.checked = lastGoodAutoHide;
  autoHideCb.addEventListener("change", () => {
    const enabled = autoHideCb.checked;
    void setAutoHideInFullscreen(enabled)
      .then(() => {
        lastGoodAutoHide = enabled;
        flash(enabled ? t("settings.floating.auto_hide_on") : t("settings.floating.auto_hide_off"));
      })
      .catch((e) => {
        autoHideCb.checked = lastGoodAutoHide;
        flash(t("settings.floating.toggle_failed", { err: String(e) }), true);
      });
  });

  // ── 底部提示行 checkbox ──
  let lastGoodFooterHint = cfg.show_footer_hint ?? false;
  const footerHintCb = el("input", {
    type: "checkbox",
    id: "show-footer-hint",
  }) as HTMLInputElement;
  footerHintCb.checked = lastGoodFooterHint;
  footerHintCb.addEventListener("change", () => {
    const enabled = footerHintCb.checked;
    void setShowFooterHint(enabled)
      .then(() => {
        lastGoodFooterHint = enabled;
        flash(enabled ? t("settings.floating.footer_hint_on") : t("settings.floating.footer_hint_off"));
      })
      .catch((e) => {
        footerHintCb.checked = lastGoodFooterHint;
        flash(t("settings.floating.toggle_failed", { err: String(e) }), true);
      });
  });

  // ── fit 底部余量 number input（0–120 逻辑 px，默认 80）──
  // fit 上限 = screen.availHeight − 该值。手编 config.json 塞进非法值时
  // 回落默认 80（对齐 D8-16 对 color_thresholds 的 init 校验策略）。
  const FIT_MARGIN_DEFAULT = 80;
  const rawMargin = cfg.floating_fit_bottom_margin;
  const marginInit =
    typeof rawMargin === "number" && Number.isInteger(rawMargin) &&
    rawMargin >= 0 && rawMargin <= 120
      ? rawMargin
      : FIT_MARGIN_DEFAULT;
  if (rawMargin !== undefined && marginInit !== rawMargin) {
    console.warn("[floating] cfg.floating_fit_bottom_margin 非法，回落默认 80", rawMargin);
  }
  let lastGoodMargin = marginInit;
  const marginInput = el("input", {
    type: "number",
    id: "fit-bottom-margin",
    min: "0",
    max: "120",
    step: "1",
    value: String(marginInit),
  }) as HTMLInputElement;
  marginInput.addEventListener("change", () => {
    const v = Number(marginInput.value);
    // M33 式客户端即时校验：非整数 / 越界立刻 flash 拦下，不等 IPC 往返
    if (!Number.isInteger(v) || v < 0 || v > 120) {
      flash(t("settings.floating.fit_margin_invalid"), true);
      marginInput.value = String(lastGoodMargin);
      return;
    }
    void setFloatingFitBottomMargin(v)
      .then(() => {
        lastGoodMargin = v;
        flash(t("settings.floating.fit_margin_saved"));
      })
      .catch((e) => {
        flash(t("settings.floating.toggle_failed", { err: String(e) }), true);
        marginInput.value = String(lastGoodMargin);
      });
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
      // fit 底部余量
      el("div", { class: "field" },
        el("label", { for: "fit-bottom-margin" }, t("settings.floating.fit_margin_label")),
        el("div", { class: "row" }, marginInput),
        el("div", { class: "help" }, t("settings.floating.fit_margin_help")),
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

  // H-Frontend-2 fix (2026-09-28 audit)：4 档自定义色原来仍是原生
  // `<input type="color">` 且**只绑 `change`**。WKWebView 的 NSColorPanel 能
  // 打开能取色，但 input/change **全程不派发**（见 memory
  // wkwebview-color-input-change-footgun「WKWebView 里不要用 <input type=color>」，
  // 跟 commit 6931dfb 给托盘颜色做过的判定同款）。托盘那 4 个 2026-09-08 已经
  // 换成纯 DOM 色板 + hex 文本框（app.ts），浮窗这 4 个漏换 → macOS 上浮窗
  // 自定义色永远存不下去；Windows WebView2 正常，所以只在 mac 暴露。
  // 照抄 app.ts 的方案。
  const COLOR_PALETTE = [
    "#ffffff",
    "#000000",
    "#30d158",
    "#5ac8fa",
    "#ff9f0a",
    "#ff453a",
  ];
  // 与 Rust is_valid_hex_color 同口径（3|4|6|8 位 hex）。后端读侧写侧都放宽后
  // 前端同步放宽，否则用户手输 #abc 会被前端判非法、后端却认。
  const HEX_COLOR_RE = /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;
  // colorUi = 控件当前**显示**值（点色板/敲 hex 立刻更新，applyAll 从这里读）；
  // currentOverrides = 最近一次 IPC **成功**的值，失败回填用。二者分离是
  // app.ts lastGood 模式在颜色控件上的落地。
  const colorUi: Record<typeof colorKeys[number], string> = {} as any;
  const colorSwatches: Record<typeof colorKeys[number], HTMLElement[]> = {} as any;
  const colorHexInputs: Record<typeof colorKeys[number], HTMLInputElement> = {} as any;

  const markColorSwatches = (key: typeof colorKeys[number], color: string): void => {
    for (const s of colorSwatches[key] ?? []) {
      s.classList.toggle("selected", color.toLowerCase() === s.dataset.color);
    }
  };
  const fillColorUi = (key: typeof colorKeys[number], color: string): void => {
    colorUi[key] = color;
    colorHexInputs[key].value = color;
    markColorSwatches(key, color);
  };
  const setColorValue = (key: typeof colorKeys[number], color: string): void => {
    fillColorUi(key, color);
    void applyAll();
  };

  for (const key of colorKeys) {
    // D8-09 (2026-09-04 audit)：非法存量值（"blue" / "rgb(...)"）不喂给控件
    // —— 原生 color input 会静默 fallback 成 #000000，下次 applyAll 就把存量色
    // 永久覆盖成黑。非法值回退默认色。
    const stored = overrides[key] ?? "";
    const init = HEX_COLOR_RE.test(stored) ? stored.toLowerCase() : DEFAULT_PALETTE[key];

    const swatches = COLOR_PALETTE.map((c) =>
      el("button", {
        type: "button",
        class: "accent-swatch",
        "data-color": c,
        style: `background: ${c};`,
        title: c,
      }),
    );
    colorSwatches[key] = swatches;
    // hex 文本框沿用原 `color-${key}` id（本项目无任何外部引用，已 grep 确认），
    // text 类型的 change 事件在 WKWebView 里可靠 —— 只有 color 类型坏。
    const hexInput = el("input", {
      type: "text",
      id: `color-${key}`,
      "data-id": `color-${key}`,
      class: "color-hex-input",
      placeholder: "#RRGGBB",
      autocomplete: "off",
      spellcheck: "false",
      style: "width: 86px;",
      value: init,
    }) as HTMLInputElement;
    colorHexInputs[key] = hexInput;

    markColorSwatches(key, init);
    for (const s of swatches) {
      // 色板是 <button> —— 只派发 click，**永不派发 change**（H-Frontend-1
      // 同款坑：extra-instance-form 的 accent 色板就挂在 change 委托上，
      // 导致自定义中转站 accent 永远存不下来）。照 app.ts 的 click 写法。
      s.addEventListener("click", () => setColorValue(key, s.dataset.color ?? init));
    }
    hexInput.addEventListener("change", () => {
      const v = hexInput.value.trim().toLowerCase();
      // 清空 = 回默认色（默认色本身不写进 config，保持 config.json 干净）
      if (v === "") {
        setColorValue(key, DEFAULT_PALETTE[key]);
        return;
      }
      if (!HEX_COLOR_RE.test(v)) {
        // 复用 app.ts 托盘 hex 校验同款 key（没有 floating 专用 invalid key，
        // 见 2026-09-28 前端审查报告的 i18n 缺口项）。
        flash(t("settings.app.tray_color_failed", { err: hexInput.value }), true);
        hexInput.value = colorUi[key];
        return;
      }
      setColorValue(key, v);
    });
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
      // 读 colorUi（控件当前显示值）而不是直接读 DOM —— 三个读取点
      // （applyAll / 失败回填 / 全部重置）共用同一份状态，不会各自漂移。
      const v = colorUi[key].toLowerCase();
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
        fillColorUi(key, currentOverrides[key] ?? DEFAULT_PALETTE[key]);
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
      fillColorUi(key, DEFAULT_PALETTE[key]);
    }
    walletCb.checked = false;
    walletInput.value = "";
    walletInput.disabled = true;
    void applyAll();
  });

  // 4 组自定义色，每组一排：label + 6 个色板 + hex 文本框
  // （原为 4 个 <input type=color>，macOS WKWebView 全哑，见上方 H-Frontend-2）
  const colorRow = el("div", {
    class: "row",
    style: "display: flex; gap: 14px; align-items: center; flex-wrap: wrap;",
  });
  for (const key of colorKeys) {
    colorRow.appendChild(
      el("div", {
        class: "color-override-row",
        style: "display: inline-flex; align-items: center; gap: 6px;",
      },
        el("span", { style: "font-size: 11px; min-width: 44px;" }, colorLabels[key]),
        el("div", { class: "accent-palette" }, ...colorSwatches[key]),
        colorHexInputs[key],
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
