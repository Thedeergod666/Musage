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

/// 「颜色档位阈值」+「钱包余额告警」两个相关配置。
///
/// 两者都走 set_display_thresholds 单字段 command（参考 set_low_power_mode
/// 的"勾选即生效"模式），不依赖"保存"按钮。Rust 端会校验 t0<t1<t2<100 和
/// wallet ≥ 0，失败回退到旧值 + flash 报错。
function renderDisplayThresholdsFields(cfg: AppConfig) {
  const [t0, t1, t2] = cfg.color_thresholds ?? [50, 70, 88];

  // 3 个 number input + "保存" 按钮
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
  const saveThresholdsBtn = el("button", { class: "primary", id: "save-thresholds" }, "保存阈值") as HTMLButtonElement;
  const resetThresholdsBtn = el("button", { id: "reset-thresholds" }, "恢复默认 [50, 70, 88]") as HTMLButtonElement;

  // 钱包告警（默认关闭）
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

  // 共享的"立即应用"动作：从 3 个 input 读 → 调 setDisplayThresholds
  // 失败时 flash 报错 + 回填旧值。
  const applyThresholds = async () => {
    const v0 = parseInt(t0Input.value, 10);
    const v1 = parseInt(t1Input.value, 10);
    const v2 = parseInt(t2Input.value, 10);
    if (![v0, v1, v2].every(Number.isFinite)) {
      flash("✗ 阈值必须是数字", true);
      return;
    }
    const wallet = walletCb.checked
      ? parseFloat(walletInput.value)
      : null;
    if (walletCb.checked && !Number.isFinite(wallet)) {
      flash("✗ 钱包告警阈值必须是数字（或取消勾选关闭）", true);
      return;
    }
    try {
      await setDisplayThresholds([v0, v1, v2], wallet);
      flash("✓ 阈值已更新");
    } catch (e) {
      flash(`✗ 保存失败: ${e}`, true);
      // 回填旧值
      t0Input.value = String(t0);
      t1Input.value = String(t1);
      t2Input.value = String(t2);
      walletInput.value = cfg.wallet_alert_threshold != null
        ? String(cfg.wallet_alert_threshold)
        : "";
    }
  };

  saveThresholdsBtn.addEventListener("click", () => void applyThresholds());
  resetThresholdsBtn.addEventListener("click", () => {
    t0Input.value = "50";
    t1Input.value = "70";
    t2Input.value = "88";
    void applyThresholds();
  });
  walletCb.addEventListener("change", () => {
    walletInput.disabled = !walletCb.checked;
    void applyThresholds();
  });
  // wallet input 单独监听，勾选时也要响应（避免必须点保存按钮）
  walletInput.addEventListener("change", () => {
    if (walletCb.checked) void applyThresholds();
  });

  return [
    el("div", { class: "field" },
      el("label", {}, "🎨 颜色档位阈值（%）"),
      el("div", { class: "row", style: "display: flex; gap: 6px; align-items: center;" },
        t0Input, el("span", {}, " → "), t1Input, el("span", {}, " → "), t2Input,
        el("span", { style: "color: var(--text-faint); margin-left: 6px; font-size: 11px;" },
          "ok / cyan / warn / alert"),
      ),
      el("div", { class: "row", style: "display: flex; gap: 6px; margin-top: 6px;" },
        saveThresholdsBtn, resetThresholdsBtn,
      ),
      el("div", { class: "help" },
        "默认 [50, 70, 88]。改动会立即生效（不需点保存）。后端校验 0 < t0 < t1 < t2 < 100。"),
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
  ];
}
