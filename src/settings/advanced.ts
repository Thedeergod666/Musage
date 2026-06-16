// "高级" section —— Schema overrides（MiniMax / Xiaomi 改字段名后用）
//
// 只在解析失败 / 显示 "Schema 未知" 时用。每行一个 JSON 对象，total + remaining
// 同时存在才算命中，end 可选。改完 blur 触发 saveConfig()。
//
// Xiaomi 的 API key + 手动 Cookie 也放这里（主面板只留快捷登录按钮）。

import { el } from "./utils";
import type { AppConfig } from "./types";
import { apiKeyPlaceholder, loadCredentialStatus } from "./credentials";
import { t } from "../i18n";

export function renderAdvancedSection(container: HTMLElement, cfg: AppConfig) {
  const ov = cfg.schema_overrides ?? {};
  const mm = ov.minimax ?? {
    five_hour: { count_candidates: [] },
    weekly: { count_candidates: [] },
  };
  const xm = (ov as Record<string, any>).xiaomimimo ?? {
    monthly: { count_candidates: [] },
  };

  // 3 个 textarea —— #id 跟 config.ts 里的 loadConfig / saveConfig 对齐
  const ta5h = el("textarea", {
    id: "overrides-5h-minimax",
    rows: "3",
    placeholder: '[{"total":"...","remaining":"...","end":"..."}]',
  }) as HTMLTextAreaElement;
  ta5h.value = JSON.stringify(mm.five_hour?.count_candidates ?? [], null, 2);

  const taWeek = el("textarea", {
    id: "overrides-weekly-minimax",
    rows: "3",
    placeholder: '[{"total":"...","remaining":"...","end":"..."}]',
  }) as HTMLTextAreaElement;
  taWeek.value = JSON.stringify(mm.weekly?.count_candidates ?? [], null, 2);

  const taXmMonth = el("textarea", {
    id: "overrides-monthly-xiaomimimo",
    rows: "3",
    placeholder: '[{"total":"...","remaining":"...","end":"..."}]',
  }) as HTMLTextAreaElement;
  taXmMonth.value = JSON.stringify(xm.monthly?.count_candidates ?? [], null, 2);

  // ── Schema Overrides ──
  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, `🔧 ${t("settings.nav.advanced")}`),
      el("div", { class: "help" },
        t("settings.advanced.schema_help_1"),
        el("strong", {}, t("settings.advanced.schema_help_2")),
        t("settings.advanced.schema_help_3"),
        el("code", {}, "total"),
        " + ",
        el("code", {}, "remaining"),
        " 同时存在才算命中，",
        el("code", {}, "end"),
        " 可选。",
        el("br"),
        t("settings.advanced.schema_help_4"),
      ),
      el("div", { class: "field" },
        el("label", { for: "overrides-5h-minimax" }, "MiniMax · 5h 候选字段名"),
        ta5h,
      ),
      el("div", { class: "field" },
        el("label", { for: "overrides-weekly-minimax" }, "MiniMax · 周 候选字段名"),
        taWeek,
      ),
      el("div", { class: "field" },
        el("label", { for: "overrides-monthly-xiaomimimo" }, "Xiaomi MiMo · 月度 候选字段名"),
        taXmMonth,
      ),
    ),
  );

  // ── Xiaomi MiMo 凭据（从主面板移过来）──
  // API key 对 Bearer 永远 401，手动 Cookie 是兜底——放高级 tab 不占主面板空间。
  const xmApiKeyInput = el("input", {
    type: "password",
    id: "api-key-xiaomimimo-adv",
    placeholder: apiKeyPlaceholder("xiaomimimo"),
    autocomplete: "off",
  }) as HTMLInputElement;
  const xmCookieInput = el("textarea", {
    id: "cookie-xiaomimimo-adv",
    rows: "4",
    placeholder: 'api-platform_serviceToken="..."; userId=...; api-platform_slh="..."; api-platform_ph="..."',
  }) as HTMLTextAreaElement;

  const xmSection = el("section", { class: "section-card" },
    el("h2", {}, "🔑 Xiaomi MiMo · 凭据"),
    el("div", { class: "help" },
      "Xiaomi 用量 API 当前对 Bearer 返 401，API key 填了也不会生效。",
      el("br"),
      "正常情况下用主面板的「🔑 登录小米账号」即可，这里只做手动兜底。",
    ),
    // API key
    el("div", { class: "field" },
      el("label", {}, "API key（当前无效，预留）"),
      xmApiKeyInput,
      el("div", { class: "status", id: "api-key-status-xiaomimimo-adv" }, "—"),
      el("div", { class: "row" },
        el("button", {
          class: "primary",
          id: "save-key-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "save-key",
          "data-advanced": "true",
        }, t("settings.common.save") + " API key"),
        el("button", {
          class: "danger",
          id: "del-key-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "del-key",
          "data-advanced": "true",
        }, t("settings.common.delete")),
      ),
    ),
    // Cookie
    el("div", { class: "field" },
      el("label", {}, "Dashboard Cookie（兜底：401 时自动退到这里）"),
      xmCookieInput,
      el("div", { class: "status", id: "cookie-status-xiaomimimo-adv" }, "—"),
      el("div", { class: "row" },
        el("button", {
          class: "primary",
          id: "save-cookie-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "save-cookie",
          "data-advanced": "true",
        }, t("settings.common.save") + " Cookie"),
        el("button", {
          class: "danger",
          id: "del-cookie-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "del-cookie",
          "data-advanced": "true",
        }, t("settings.common.delete")),
      ),
      el("div", { class: "help" },
        "⚠ Xiaomi 用量走 dashboard admin API，需要浏览器登录态。",
        el("br"),
        "获取方法：Chrome 登录 ",
        el("a", { href: "https://platform.xiaomimimo.com", target: "_blank" }, "platform.xiaomimimo.com"),
        " → F12 → Network → 任意 /api/v1/tokenPlan/* 请求 → 右键 → Copy → Copy request headers → 找 ",
        el("code", {}, "cookie:"),
        " 这一行整段粘贴到上面。",
        el("br"),
        "Cookie 登出后失效，过期时 (HTTP 401) 错误信息会引导重粘。",
      ),
    ),
  );
  container.appendChild(xmSection);

  // 加载凭据状态（延迟，等 DOM 就绪）
  setTimeout(() => {
    void loadCredentialStatus("xiaomimimo");
  }, 100);
}
