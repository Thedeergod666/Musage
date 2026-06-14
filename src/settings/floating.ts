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
} from "./api";
import type { AppConfig, FloatingPinMode } from "./types";

export function renderFloatingSection(container: HTMLElement, cfg: AppConfig) {
  // ── 置顶/置底/普通 单选 ──
  const currentMode: FloatingPinMode = cfg.floating_pin_mode ?? "pin_top";
  const pinMode = el("div", { class: "pin-mode" });
  const options: Array<{ value: FloatingPinMode; title: string; desc: string }> = [
    { value: "pin_top", title: "📌 始终置顶", desc: "浮窗永远在最上层，不被其它窗口盖住" },
    { value: "pin_bottom", title: "⬇ 置底，鼠标 hover 时置顶", desc: "默认在底部，鼠标进入浮窗时自动浮上来" },
    { value: "normal", title: "🔓 普通窗口", desc: "不强制层级，跟普通窗口一样" },
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
  const resetBtn = el("button", { id: "reset-floating", class: "primary" }, "归位到主屏幕正中央") as HTMLButtonElement;
  resetBtn.addEventListener("click", () => {
    resetBtn.disabled = true;
    void resetFloatingWindow()
      .then(() => flash("✓ 浮窗已归位到主屏幕正中央"))
      .catch((e) => flash(`✗ 归位失败: ${e}`, true))
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
      .then(() => flash(enabled ? "✓ 省电模式已开启" : "✓ 省电模式已关闭"))
      .catch((e) => flash(`✗ 切换失败: ${e}`, true));
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
      .then(() => flash(enabled ? "✓ 全屏自动隐藏已开启（仅 macOS）" : "✓ 全屏自动隐藏已关闭"))
      .catch((e) => flash(`✗ 切换失败: ${e}`, true));
  });

  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, "🪟 浮窗"),
      // 置顶模式
      el("div", { class: "field" },
        el("label", {}, "置顶 / 置底 行为"),
        pinMode,
        el("div", { class: "help" }, "改动会立即生效（不需要点保存），并写入 config.json 下次启动恢复。"),
      ),
      // 归位
      el("div", { class: "field" },
        el("div", { class: "row" }, resetBtn),
        el("div", { class: "help" }, "浮窗位置 + 大小已经自动记忆，下次启动恢复。点归位可把窗口挪到当前主屏幕正中央。"),
      ),
      // 省电模式
      el("div", { class: "field" },
        el("div", { class: "check" },
          lowPowerCb,
          el("label", { for: "low-power-mode" }, "省电模式（关闭玻璃模糊 + 过渡动画）"),
        ),
        el("div", { class: "help" }, "老 Intel Mac / Linux WebKitGTK 等 GPU 弱的平台开启可显著减少 backdrop-filter 开销。"),
      ),
      // 全屏自动隐藏
      el("div", { class: "field" },
        el("div", { class: "check" },
          autoHideCb,
          el("label", { for: "auto-hide-in-fullscreen" }, "全屏时自动隐藏浮窗"),
        ),
        el("div", { class: "help" }, "检测到任何 app 进入全屏（菜单栏自动隐藏）→ 浮窗暂时隐藏；退出全屏 → 自动恢复。目前仅 macOS 生效。"),
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
  const [t0, t1, t2] = cfg.color_thresholds ?? [50, 70, 88];
  const t0Input = el("input", {
    type: "number", id: "color-t0", min: "0", max: "99", step: "1",
    value: String(t0), title: "ok → cyan 分界（0..100）",
  }) as HTMLInputElement;
  const t1Input = el("input", {
    type: "number", id: "color-t1", min: "0", max: "99", step: "1",
    value: String(t1), title: "cyan → warn 分界",
  }) as HTMLInputElement;
  const t2Input = el("input", {
    type: "number", id: "color-t2", min: "0", max: "99", step: "1",
    value: String(t2), title: "warn → alert 分界",
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
    ok: "OK (绿 · 安全)",
    cyan: "CYAN (青 · 过半提醒)",
    warn: "WARN (黄 · 警告)",
    alert: "ALERT (红 · 告警)",
  };
  const overrides = cfg.color_overrides ?? {};
  const colorPickers: Record<typeof colorKeys[number], HTMLInputElement> = {} as any;
  for (const key of colorKeys) {
    colorPickers[key] = el("input", {
      type: "color", id: `color-${key}`,
      value: overrides[key] ?? DEFAULT_PALETTE[key],
      title: `自定义 ${key.toUpperCase()} 色`,
    }) as HTMLInputElement;
    colorPickers[key].addEventListener("change", () => void applyAll());
  }

  // ── 钱包告警（默认关闭） ──
  const walletCb = el("input", { type: "checkbox", id: "wallet-alert-enabled" }) as HTMLInputElement;
  const walletInput = el("input", {
    type: "number", id: "wallet-alert-threshold", min: "0", step: "0.01",
    placeholder: "2", title: "remaining < 该值时该行翻红",
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
      flash("✗ 阈值必须是数字", true);
      return;
    }
    const wallet = walletCb.checked ? parseFloat(walletInput.value) : null;
    if (walletCb.checked && !Number.isFinite(wallet)) {
      flash("✗ 钱包告警阈值必须是数字（或取消勾选关闭）", true);
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
      flash("✓ 显示设置已更新");
    } catch (e) {
      flash(`✗ 保存失败: ${e}`, true);
      // 回填旧值
      t0Input.value = String(t0);
      t1Input.value = String(t1);
      t2Input.value = String(t2);
      walletInput.value = cfg.wallet_alert_threshold != null
        ? String(cfg.wallet_alert_threshold)
        : "";
      for (const key of colorKeys) {
        colorPickers[key].value = overrides[key] ?? DEFAULT_PALETTE[key];
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
    "↺ 全部重置为默认") as HTMLButtonElement;
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
      el("label", {}, "🎨 颜色档位阈值（%）"),
      el("div", { class: "row", style: "display: flex; gap: 6px; align-items: center;" },
        t0Input, el("span", {}, " → "), t1Input, el("span", {}, " → "), t2Input,
        el("span", { style: "color: var(--text-faint); margin-left: 6px; font-size: 11px;" },
          "ok / cyan / warn / alert"),
      ),
      el("div", { class: "help" },
        "默认 [50, 70, 88]。改动会立即生效（不需点保存）。后端校验 0 < t0 < t1 < t2 < 100。"),
    ),
    el("div", { class: "field" },
      el("label", {}, "🎨 自定义 4 档色（hover 时显示）"),
      colorRow,
      el("div", { class: "help" },
        "默认用 iOS 系统色（绿/青/黄/红）。改任意一个 → 立即生效；" +
        "浮窗文字 / 进度条 / 状态点 三个地方的颜色会同步跟着变。"),
    ),
    el("div", { class: "field" },
      el("label", {}, "💰 钱包余额告警（剩余多少时变红）"),
      el("div", { class: "check" },
        walletCb,
        el("label", { for: "wallet-alert-enabled" }, "启用低额高亮（剩余金额 < 阈值时行翻红）"),
        walletInput,
      ),
      el("div", { class: "help" },
        "默认关闭（保持现状蓝色）。开启后，对所有「余额 / 剩余积分」行生效，不区分钱 / 积分（" +
        "DeepSeek / ZenMux / OpenRouter 等），按你自己 provider 的余额量级调阈值。"),
    ),
    el("div", { class: "field" },
      el("div", { class: "row" }, resetAllBtn),
      el("div", { class: "help" },
        "一键把上述 3 项全部还原到默认：阈值 [50,70,88] / 颜色 iOS 默认色 / 钱包告警关闭。"),
    ),
  ];
}
