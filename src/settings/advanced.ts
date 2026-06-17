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
    placeholder: t("settings.advanced.schema_placeholder"),
  }) as HTMLTextAreaElement;
  ta5h.value = JSON.stringify(mm.five_hour?.count_candidates ?? [], null, 2);

  const taWeek = el("textarea", {
    id: "overrides-weekly-minimax",
    rows: "3",
    placeholder: t("settings.advanced.schema_placeholder"),
  }) as HTMLTextAreaElement;
  taWeek.value = JSON.stringify(mm.weekly?.count_candidates ?? [], null, 2);

  const taXmMonth = el("textarea", {
    id: "overrides-monthly-xiaomimimo",
    rows: "3",
    placeholder: t("settings.advanced.schema_placeholder"),
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
        el("label", { for: "overrides-5h-minimax" }, t("settings.advanced.minimax_5h_label")),
        ta5h,
      ),
      el("div", { class: "field" },
        el("label", { for: "overrides-weekly-minimax" }, t("settings.advanced.minimax_weekly_label")),
        taWeek,
      ),
      el("div", { class: "field" },
        el("label", { for: "overrides-monthly-xiaomimimo" }, t("settings.advanced.xiaomi_monthly_label")),
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
    placeholder: t("credentials.cookie_textarea_placeholder"),
  }) as HTMLTextAreaElement;

  // 顶部 help：拆为多段（en/zh 不需要 1:1 翻，但 help 5 段更模块化）
  const help = el("div", { class: "help" });
  help.innerHTML = t("settings.advanced.xiaomi_credentials_help");

  const cookieHelp = el("div", { class: "help" });
  cookieHelp.innerHTML =
    t("settings.advanced.xiaomi_cookie_help") + "<br>" +
    t("settings.advanced.xiaomi_cookie_help_2") +
    `<a href="https://platform.xiaomimimo.com" target="_blank">platform.xiaomimimo.com</a>` +
    t("settings.advanced.xiaomi_cookie_help_3") +
    `<code>cookie:</code>` +
    t("settings.advanced.xiaomi_cookie_help_4") + "<br>" +
    t("settings.advanced.xiaomi_cookie_help_5");

  const xmSection = el("section", { class: "section-card" },
    el("h2", {}, t("settings.advanced.xiaomi_credentials_title")),
    help,
    // API key
    el("div", { class: "field" },
      el("label", {}, t("settings.advanced.xiaomi_api_key_label")),
      xmApiKeyInput,
      el("div", { class: "status", id: "api-key-status-xiaomimimo-adv" }, t("credentials.cookie_status_placeholder")),
      el("div", { class: "row" },
        el("button", {
          class: "primary",
          id: "save-key-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "save-key",
          "data-advanced": "true",
        }, t("settings.advanced.xiaomi_api_key_save", { save: t("settings.common.save") })),
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
      el("label", {}, t("settings.advanced.xiaomi_cookie_label")),
      xmCookieInput,
      el("div", { class: "status", id: "cookie-status-xiaomimimo-adv" }, t("credentials.cookie_status_placeholder")),
      el("div", { class: "row" },
        el("button", {
          class: "primary",
          id: "save-cookie-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "save-cookie",
          "data-advanced": "true",
        }, t("settings.advanced.xiaomi_cookie_save", { save: t("settings.common.save") })),
        el("button", {
          class: "danger",
          id: "del-cookie-xiaomimimo-adv",
          "data-id": "xiaomimimo",
          "data-action": "del-cookie",
          "data-advanced": "true",
        }, t("settings.common.delete")),
      ),
      cookieHelp,
    ),
  );
  container.appendChild(xmSection);

  // 加载凭据状态（延迟，等 DOM 就绪）
  setTimeout(() => {
    void loadCredentialStatus("xiaomimimo");
  }, 100);
}
