// 设置面板入口
//
// 拆模块后的整合点：
// - 顶层（同步）调 setupTabs + 所有按钮事件绑定 —— 任何一个 invoke
//   抛错都不会影响按钮响应（保 Settings 1.x 的"init 失败也不死"特性）
// - async IIFE 调所有 loadXxx 拉初始数据
// - 全部即时生效控件（pin mode / low power / auto-hide）的 onchange 直接
//   invoke 自己的 command（v0.6+ 起这几个 command 真的存在，不再死按钮）

import { $, setupTabs } from "./utils";
import { applyPinMode, loadConfig } from "./config";
import { testConn } from "./test";
import { loadLogs, clearLogs, copyLogs } from "./logs";
import { setupUpdaterSection } from "./updater";
import { moveProviderInOrder } from "./order";
import { setLowPowerMode, setAutoHideInFullscreen, resetFloatingWindow } from "./api";
import {
  loadKeyStatus,
  saveKey,
  deleteKey,
  copyKey,
  loadCookieStatus,
  saveCookie,
  deleteCookie,
  loadTavilyKeyStatus,
  saveTavilyKey,
  deleteTavilyKey,
  copyTavilyKey,
  loadZenmuxKeyStatus,
  saveZenmuxKey,
  deleteZenmuxKey,
  copyZenmuxKey,
} from "./credentials";
import { flash } from "./utils";

// ── 顶层同步：Tab + 按钮绑定 ─────────────────────────────────

setupTabs();

// 保存配置（"保存" 按钮）
$("#save")?.addEventListener("click", () => void (async () => {
  const { saveConfig } = await import("./config");
  await saveConfig();
})());

// Provider 5 个 save / delete / copy
$("#save-key-minimax")?.addEventListener("click", () => void saveKey("minimax"));
$("#save-key-deepseek")?.addEventListener("click", () => void saveKey("deepseek"));
$("#save-key-xiaomimimo")?.addEventListener("click", () => void saveKey("xiaomimimo"));
$("#save-key-tavily")?.addEventListener("click", () => void saveTavilyKey());
$("#save-key-zenmux")?.addEventListener("click", () => void saveZenmuxKey());
$("#del-key-minimax")?.addEventListener("click", () => void deleteKey("minimax"));
$("#del-key-deepseek")?.addEventListener("click", () => void deleteKey("deepseek"));
$("#del-key-xiaomimimo")?.addEventListener("click", () => void deleteKey("xiaomimimo"));
$("#del-key-tavily")?.addEventListener("click", () => void deleteTavilyKey());
$("#del-key-zenmux")?.addEventListener("click", () => void deleteZenmuxKey());
$("#copy-key-minimax")?.addEventListener("click", () => void copyKey("minimax"));
$("#copy-key-deepseek")?.addEventListener("click", () => void copyKey("deepseek"));
$("#copy-key-xiaomimimo")?.addEventListener("click", () => void copyKey("xiaomimimo"));
$("#copy-key-tavily")?.addEventListener("click", () => void copyTavilyKey());
$("#copy-key-zenmux")?.addEventListener("click", () => void copyZenmuxKey());

// Xiaomi cookie
$("#save-cookie-xiaomimimo")?.addEventListener("click", () => void saveCookie("xiaomimimo"));
$("#del-cookie-xiaomimimo")?.addEventListener("click", () => void deleteCookie("xiaomimimo"));

// 测试连接
$("#test")?.addEventListener("click", () => void testConn());

// 浮窗归位
$("#reset-floating")?.addEventListener("click", () => {
  const btn = $("#reset-floating") as HTMLButtonElement;
  btn.disabled = true;
  void resetFloatingWindow()
    .then(() => flash("✓ 浮窗已归位到主屏幕正中央"))
    .catch((e) => flash(`✗ 归位失败: ${e}`, true))
    .finally(() => {
      btn.disabled = false;
    });
});

// 置顶/置底模式：单选按钮变更即生效（不依赖"保存配置"按钮）
document.querySelectorAll<HTMLInputElement>('input[name="pin-mode"]').forEach((r) => {
  r.addEventListener("change", () => {
    if (!r.checked) return;
    const mode = r.value as "pin_top" | "pin_bottom" | "normal";
    if (mode === "pin_top" || mode === "pin_bottom" || mode === "normal") {
      void applyPinMode(mode);
    }
  });
});

// 省电模式 / 全屏自动隐藏：勾选即生效（v0.6+ 这两个 command 真的存在了）
$("#low-power-mode")?.addEventListener("change", () => {
  const enabled = ($("#low-power-mode") as HTMLInputElement).checked;
  void setLowPowerMode(enabled)
    .then(() => flash(enabled ? "✓ 省电模式已开启" : "✓ 省电模式已关闭"))
    .catch((e) => flash(`✗ 切换失败: ${e}`, true));
});

$("#auto-hide-in-fullscreen")?.addEventListener("change", () => {
  const enabled = ($("#auto-hide-in-fullscreen") as HTMLInputElement).checked;
  void setAutoHideInFullscreen(enabled)
    .then(() =>
      flash(enabled ? "✓ 全屏自动隐藏已开启（仅 macOS）" : "✓ 全屏自动隐藏已关闭"),
    )
    .catch((e) => flash(`✗ 切换失败: ${e}`, true));
});

// 日志按钮
document.getElementById("logs-refresh")?.addEventListener("click", () => void loadLogs());
document.getElementById("logs-clear")?.addEventListener("click", () => void clearLogs());
document.getElementById("logs-copy")?.addEventListener("click", () => void copyLogs());
document.getElementById("logs-filter")?.addEventListener("change", () => void loadLogs());

// Provider 顺序按钮
document.querySelectorAll<HTMLButtonElement>(".order-up").forEach((btn) => {
  btn.addEventListener("click", () => {
    const id = btn.dataset.id;
    if (id) void moveProviderInOrder(id, "up");
  });
});
document.querySelectorAll<HTMLButtonElement>(".order-down").forEach((btn) => {
  btn.addEventListener("click", () => {
    const id = btn.dataset.id;
    if (id) void moveProviderInOrder(id, "down");
  });
});

// ── 异步 init：拉初始数据 + 注入 updater section ─────────────

(async () => {
  try {
    await loadKeyStatus("minimax");
    await loadKeyStatus("deepseek");
    await loadKeyStatus("xiaomimimo");
    await loadTavilyKeyStatus();
    await loadZenmuxKeyStatus();
    await loadCookieStatus("xiaomimimo");
    await loadConfig();
    setupUpdaterSection();
    await loadLogs();
  } catch (e) {
    console.error("[settings] init failed", e);
    flash(`✗ 初始化失败: ${e}`, true);
  }
})();
