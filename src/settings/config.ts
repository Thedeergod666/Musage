// 全局 AppConfig 加载 / 保存 / 浮窗置顶模式
//
// 包含原 settings.ts 的 loadConfig / saveConfig / applyPinMode 三段。
// 拆出的目的是让 settings/main.ts 不会太胖。

import {
  $,
  currentProviderOrder,
  flash,
} from "./utils";
import {
  getConfig as ipcGetConfig,
  saveConfig as ipcSaveConfig,
  setFloatingPinMode,
} from "./api";
import type { AppConfig, FieldTriple, FloatingPinMode, ProviderId } from "./types";

// ── 加载 ─────────────────────────────────────────────────────

export async function loadConfig() {
  const cfg = await ipcGetConfig();
  const regionEl = $("#region") as HTMLSelectElement;
  const minimaxRegion = cfg.providers?.minimax?.region ?? "cn";
  regionEl.value = minimaxRegion;
  const xiaomiRegionEl = $("#xiaomi-region") as HTMLSelectElement;
  const xiaomiRegion = cfg.providers?.xiaomimimo?.xiaomi_region ?? "cn";
  xiaomiRegionEl.value = xiaomiRegion;
  ($("#interval") as HTMLInputElement).value = String(cfg.refresh_interval_secs);
  ($("#autostart") as HTMLInputElement).checked = cfg.autostart;

  // 置顶/置底模式：缺省 = pin_top（保持老版本行为）
  const pinMode: FloatingPinMode = cfg.floating_pin_mode ?? "pin_top";
  const radio = document.querySelector<HTMLInputElement>(
    `input[name="pin-mode"][value="${pinMode}"]`,
  );
  if (radio) radio.checked = true;

  // 性能 / 可见性
  ($("#low-power-mode") as HTMLInputElement).checked = cfg.low_power_mode ?? false;
  ($("#auto-hide-in-fullscreen") as HTMLInputElement).checked =
    cfg.auto_hide_in_fullscreen ?? false;
  // Tavily 简洁模式（默认开）
  const tavilyConcise = document.getElementById(
    "tavily-concise-mode",
  ) as HTMLInputElement | null;
  if (tavilyConcise) tavilyConcise.checked = cfg.tavily_concise_mode ?? true;

  // ZenMux 自定义 URL
  const zenmuxUrlInput = document.getElementById(
    "zenmux-base-url",
  ) as HTMLInputElement | null;
  if (zenmuxUrlInput) zenmuxUrlInput.value = cfg.zenmux_base_url ?? "";

  // 智谱 GLM 区域
  const zhipuRegionSelect = document.getElementById(
    "zhipu-region",
  ) as HTMLSelectElement | null;
  if (zhipuRegionSelect) zhipuRegionSelect.value = cfg.zhipu_region ?? "cn";

  // 各 provider「在浮窗显示」开关（缺省视为 true）+ 轮询间隔覆盖
  const { PROVIDER_IDS } = await import("./types");
  for (const id of PROVIDER_IDS) {
    const el = document.getElementById(`enabled-${id}`) as HTMLInputElement | null;
    if (el) {
      el.checked = cfg.providers?.[id]?.enabled ?? true;
      // 即时生效：勾选/取消 → 调 set_provider_enabled → 后端落盘 + emit
      // config-changed → 浮窗 re-fetch → 显隐立即反映
      el.addEventListener("change", () => {
        void import("./api").then(({ setProviderEnabled }) =>
          setProviderEnabled(id, el.checked).catch((e) => {
            flash(`✗ 切换显示失败: ${e}`, true);
          }),
        );
      });
    }
    const intervalEl = document.getElementById(
      `interval-${id}`,
    ) as HTMLInputElement | null;
    if (intervalEl) {
      const v = cfg.providers?.[id]?.refresh_interval_secs;
      intervalEl.value = v != null ? String(v) : "";
      intervalEl.placeholder = `默认 ${cfg.refresh_interval_secs} 秒（顶部"轮询间隔"）`;
    }
  }

  // Provider 排序（per-panel ↑↓ 按钮）
  const { renderProviderOrderPanels } = await import("./order");
  renderProviderOrderPanels(cfg.provider_order ?? []);

  // schema overrides (高级)
  const ov = cfg.schema_overrides ?? {};
  const mm = ov.minimax ?? {
    five_hour: { count_candidates: [] },
    weekly: { count_candidates: [] },
  };
  ($("#overrides-5h-minimax") as HTMLTextAreaElement).value = JSON.stringify(
    mm.five_hour?.count_candidates ?? [],
    null,
    2,
  );
  ($("#overrides-weekly-minimax") as HTMLTextAreaElement).value = JSON.stringify(
    mm.weekly?.count_candidates ?? [],
    null,
    2,
  );
  const xm = (ov as Record<string, any>).xiaomimimo ?? {
    monthly: { count_candidates: [] },
  };
  const xmMonthly = xm.monthly?.count_candidates ?? [];
  const xmEl = document.getElementById(
    "overrides-monthly-xiaomimimo",
  ) as HTMLTextAreaElement | null;
  if (xmEl) xmEl.value = JSON.stringify(xmMonthly, null, 2);
}

// ── 保存 ─────────────────────────────────────────────────────

export async function saveConfig() {
  // 解析 schema overrides 的 JSON；解析失败给提示但不影响其它字段保存
  let fiveHourCandidates: FieldTriple[] = [];
  let weeklyCandidates: FieldTriple[] = [];
  let monthlyCandidates: FieldTriple[] = [];
  try {
    const raw5h = ($("#overrides-5h-minimax") as HTMLTextAreaElement).value.trim() || "[]";
    const rawWeek =
      ($("#overrides-weekly-minimax") as HTMLTextAreaElement).value.trim() || "[]";
    const xmMonthlyEl = document.getElementById(
      "overrides-monthly-xiaomimimo",
    ) as HTMLTextAreaElement | null;
    const rawMonth = xmMonthlyEl?.value.trim() || "[]";
    fiveHourCandidates = JSON.parse(raw5h);
    weeklyCandidates = JSON.parse(rawWeek);
    monthlyCandidates = JSON.parse(rawMonth);
    if (
      !Array.isArray(fiveHourCandidates) ||
      !Array.isArray(weeklyCandidates) ||
      !Array.isArray(monthlyCandidates)
    ) {
      throw new Error("必须是 JSON 数组");
    }
  } catch (e) {
    flash(`✗ Schema overrides JSON 解析失败: ${e}`, true);
    return;
  }

  // 先拉一次当前 config，把浮窗位置/置顶模式这类用户没在面板上改的字段保留下来。
  // 旧实现把 floating_x/y 写死成 null，会把已记忆的窗口位置清空 —— 已修。
  const existing = await ipcGetConfig();
  const pinRadio = document.querySelector<HTMLInputElement>(
    'input[name="pin-mode"]:checked',
  );
  const pinMode: FloatingPinMode =
    (pinRadio?.value as FloatingPinMode | undefined) ??
    existing.floating_pin_mode ??
    "pin_top";

  // 读每个 provider 的轮询间隔覆盖（空字符串 = None = 用全局）
  function readProviderInterval(id: ProviderId): number | null {
    const el = document.getElementById(
      `interval-${id}`,
    ) as HTMLInputElement | null;
    if (!el) return null;
    const raw = el.value.trim();
    if (raw === "") return null;
    const n = parseInt(raw, 10);
    if (!Number.isFinite(n) || n < 10) return 10; // 后端会再 clamp 一次
    return n;
  }

  const cfg: AppConfig = {
    providers: {
      minimax: {
        enabled:
          (document.getElementById("enabled-minimax") as HTMLInputElement | null)
            ?.checked ?? true,
        region: ($("#region") as HTMLSelectElement).value as "cn" | "en",
        refresh_interval_secs: readProviderInterval("minimax"),
      },
      deepseek: {
        enabled:
          (document.getElementById("enabled-deepseek") as HTMLInputElement | null)
            ?.checked ?? true,
        refresh_interval_secs: readProviderInterval("deepseek"),
      },
      xiaomimimo: {
        enabled:
          (document.getElementById("enabled-xiaomimimo") as HTMLInputElement | null)
            ?.checked ?? true,
        xiaomi_region: ($("#xiaomi-region") as HTMLSelectElement).value as
          | "cn"
          | "sgp"
          | "ams",
        refresh_interval_secs: readProviderInterval("xiaomimimo"),
      },
      tavily: {
        enabled:
          (document.getElementById("enabled-tavily") as HTMLInputElement | null)
            ?.checked ?? true,
        refresh_interval_secs: readProviderInterval("tavily"),
      },
      zenmux: {
        enabled:
          (document.getElementById("enabled-zenmux") as HTMLInputElement | null)
            ?.checked ?? true,
        refresh_interval_secs: readProviderInterval("zenmux"),
      },
      openrouter: {
        enabled:
          (document.getElementById("enabled-openrouter") as HTMLInputElement | null)
            ?.checked ?? true,
        refresh_interval_secs: readProviderInterval("openrouter"),
      },
      kimi: {
        enabled:
          (document.getElementById("enabled-kimi") as HTMLInputElement | null)
            ?.checked ?? true,
        refresh_interval_secs: readProviderInterval("kimi"),
      },
      zhipu: {
        enabled:
          (document.getElementById("enabled-zhipu") as HTMLInputElement | null)
            ?.checked ?? true,
        refresh_interval_secs: readProviderInterval("zhipu"),
      },
    },
    zenmux_base_url:
      (document.getElementById("zenmux-base-url") as HTMLInputElement | null)
        ?.value.trim() || null,
    zenmux_mode:
      ((document.getElementById("zenmux-mode") as HTMLSelectElement | null)?.value as
        | "payg"
        | "subscription"
        | undefined) ?? "payg",
    zenmux_payg_concise_mode:
      (document.getElementById("zenmux-payg-concise-mode") as HTMLInputElement | null)
        ?.checked ?? true,
    zhipu_region:
      ((document.getElementById("zhipu-region") as HTMLSelectElement | null)?.value as
        | "cn"
        | "en"
        | undefined) ?? "cn",
    refresh_interval_secs:
      parseInt(($("#interval") as HTMLInputElement).value, 10) || 60,
    autostart: ($("#autostart") as HTMLInputElement).checked,
    floating_x: existing.floating_x ?? null,
    floating_y: existing.floating_y ?? null,
    floating_pin_mode: pinMode,
    low_power_mode: ($("#low-power-mode") as HTMLInputElement).checked,
    auto_hide_in_fullscreen: ($("#auto-hide-in-fullscreen") as HTMLInputElement).checked,
    tavily_concise_mode:
      (document.getElementById("tavily-concise-mode") as HTMLInputElement | null)
        ?.checked ?? true,
    provider_order: currentProviderOrder,
    schema_overrides: {
      minimax: {
        five_hour: { count_candidates: fiveHourCandidates },
        weekly: { count_candidates: weeklyCandidates },
      },
      deepseek: {
        five_hour: { count_candidates: [] },
        weekly: { count_candidates: [] },
      },
      xiaomimimo: {
        five_hour: { count_candidates: [] },
        weekly: { count_candidates: [] },
        monthly: { count_candidates: monthlyCandidates },
      },
    },
  };
  try {
    await ipcSaveConfig(cfg);
    flash("✓ 配置已保存");
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

// ── 置顶模式即时生效 ─────────────────────────────────────────

/// 选中即生效（通过 set_floating_pin_mode 命令）。
/// 不需要走"保存配置"按钮，因为这条改动是即时的，command 内部会同步落盘。
export async function applyPinMode(mode: FloatingPinMode) {
  try {
    await setFloatingPinMode(mode);
    const label =
      mode === "pin_top"
        ? "已设为：始终置顶"
        : mode === "pin_bottom"
        ? "已设为：置底（hover 置顶）"
        : "已设为：普通窗口";
    flash(`✓ ${label}`);
  } catch (e) {
    flash(`✗ 切换置顶模式失败: ${e}`, true);
  }
}
