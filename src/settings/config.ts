// 浮窗置顶/置底模式即时切换
//
// 选中即生效（通过 set_floating_pin_mode 命令）。
// 不需要走"保存配置"按钮，因为这条改动是即时的，command 内部会同步落盘。
//
// 历史：原 settings.ts 拆出时包含 loadConfig / saveConfig / applyPinMode 三段。
// loadConfig / saveConfig 在 2026-06-20 audit 时发现全 src 零调用方 ——
// loadConfig 行为已分散到 renderProvidersSection / renderFloatingSection /
// renderAppSection / renderAdvancedSection / renderAboutSection 各自跑
// （main.ts:122-156），saveConfig 行为被新的 setSchemaOverrides /
// setProviderEnabled / setXiaomiDisplayMode / setTrayIconStyle / 等单字段
// IPC 替代。两段代码彻底删除，只保留 applyPinMode。

import { flash } from "./utils";
import { setFloatingPinMode } from "./api";
import { t } from "../i18n";
import type { FloatingPinMode } from "./types";

export async function applyPinMode(mode: FloatingPinMode) {
  try {
    await setFloatingPinMode(mode);
    const label = t(`settings.pin_mode.${mode === "pin_top" ? "top" : mode === "pin_bottom" ? "bottom" : "normal"}`);
    flash("ok", label);
  } catch (e) {
    flash(t("settings.pin_mode.failed", { err: String(e) }), true);
  }
}
