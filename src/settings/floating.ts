// "浮窗" section —— pin mode + 归位 + 省电 + 全屏自动隐藏
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
    ),
  );
}
