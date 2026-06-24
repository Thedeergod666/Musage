// "高级" section —— Schema overrides（MiniMax / Xiaomi 改字段名后用）
//
// 只在解析失败 / 显示 "Schema 未知" 时用。每行一个 JSON 对象，total + remaining
// 同时存在才算命中，end 可选。改完 blur 触发 setSchemaOverrides() 即时落盘。
//
// Xiaomi 的 API key + 手动 Cookie 也放这里（主面板只留快捷登录按钮）。

import { debounce } from "./utils";
import { setSchemaOverrides } from "./api";
import { el, flash } from "./utils";
import type { AppConfig, FieldTriple, ProviderOverrides } from "./types";
import { apiKeyPlaceholder, loadCredentialStatus } from "./credentials";
import { t } from "../i18n";

export function renderAdvancedSection(container: HTMLElement, cfg: AppConfig) {
  const ov = cfg.schema_overrides ?? {};
  const mm = ov.minimax ?? {
    five_hour: { count_candidates: [] },
    weekly: { count_candidates: [] },
  };
  const xm = (ov as Record<string, ProviderOverrides>).xiaomimimo ?? {
    monthly: { count_candidates: [] },
  };

  // 3 个 textarea —— #id 跟 config.ts 里的 saveConfig 对齐
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

  // ── 即时生效：blur → 解析 + 调 setSchemaOverrides IPC → 后端落盘 ──
  // 2026-06-20 audit fix：之前 advanced.ts 注释说 "改完 blur 触发 saveConfig()"，
  // 但 src/settings/config.ts:saveConfig 全 src 零调用方（grep 验证），
  // 用户改的 schema 候选字段名永远不会被持久化。现在接上真正的 IPC。
  //
  // debounce 300ms：连续键入时只在停止输入 300ms 后才触发一次 IPC，
  // 避免 N+1 次 IPC 同时跑（后端 cfg.save() 会串行化，但 IPC handler
  // 也得排队）。
  const flush = debounce(async () => {
    const parseField = (raw: string): FieldTriple[] | null => {
      const trimmed = raw.trim() || "[]";
      const parsed = JSON.parse(trimmed);
      if (!Array.isArray(parsed)) throw new Error("must be a JSON array");
      return parsed as FieldTriple[];
    };
    let fiveHour: FieldTriple[], weekly: FieldTriple[], monthly: FieldTriple[];
    try {
      fiveHour = parseField(ta5h.value) ?? [];
      weekly = parseField(taWeek.value) ?? [];
      monthly = parseField(taXmMonth.value) ?? [];
    } catch (e) {
      flash(t("settings.config.schema_parse_failed", { err: String(e) }), true);
      return;
    }
    const overrides: Record<string, ProviderOverrides> = {
      minimax: { five_hour: { count_candidates: fiveHour }, weekly: { count_candidates: weekly } },
      xiaomimimo: { five_hour: { count_candidates: [] }, weekly: { count_candidates: [] }, monthly: { count_candidates: monthly } },
    };
    try {
      await setSchemaOverrides(overrides);
      flash(t("settings.app.config_saved"));
    } catch (e) {
      flash(t("settings.app.config_save_failed", { err: String(e) }), true);
    }
  }, 300);
  for (const ta of [ta5h, taWeek, taXmMonth]) {
    ta.addEventListener("input", flush);
  }

  // ── Schema Overrides ──
  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, t("settings.nav.advanced")),
      el("div", { class: "help" },
        t("settings.advanced.schema_help_1"),
        el("strong", {}, t("settings.advanced.schema_help_2")),
        t("settings.advanced.schema_help_3"),
        el("code", {}, "total"),
        " + ",
        el("code", {}, "remaining"),
        t("settings.advanced.schema_hit_note"),
        el("code", {}, "end"),
        t("settings.advanced.schema_end_optional"),
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

  // v0.2.1 commit 7 (P2-B-10): import/export 配置 section
  container.appendChild(renderImportExportSection());
}

// ── v0.2.1 commit 7: Import/Export 配置(无 keys) ──────────────────

/// 构造 import/export section DOM。
///
/// Export 走纯 web 路径:`Blob` + `<a download>` 触发浏览器下载,后端零参与。
/// Import 走 `<input type="file">` + `FileReader` 读 JSON,校验后调
/// `saveConfig` IPC 全量保存(覆盖式)。**两个方向都不包含 keys.json** —— keys
/// 永远留在本机,跟 settings 完全解耦。
function renderImportExportSection(): HTMLElement {
  const section = el("section", { class: "import-export-section" });
  section.appendChild(el("h3", {}, t("settings.advanced.io_title")));
  section.appendChild(
    el("p", { class: "help" }, t("settings.advanced.io_help")),
  );

  const exportBtn = el("button", {
    type: "button",
    class: "btn-primary",
    "data-action": "export-config",
  }, t("settings.advanced.export_btn"));

  const importInput = el("input", {
    type: "file",
    accept: "application/json,.json",
    "data-action": "import-config",
    hidden: "true",
  }) as HTMLInputElement;

  const importBtn = el("button", {
    type: "button",
    class: "btn-primary",
    "data-action": "import-config-trigger",
  }, t("settings.advanced.import_btn"));
  importBtn.addEventListener("click", () => importInput.click());

  exportBtn.addEventListener("click", () => doExportConfig());
  importInput.addEventListener("change", () => {
    const file = importInput.files?.[0];
    if (file) void doImportConfig(file);
    importInput.value = ""; // 重置以便同名文件可再次选
  });

  section.appendChild(
    el("div", { class: "io-actions" }, exportBtn, importBtn, importInput),
  );
  return section;
}

/// 构造 export 对象:AppConfig 的字段 + `extra_instances`(独立文件),但
/// 排除 `floating_x/y` / `*_key` / cookie 字段 —— 跟 keys.json 完全解耦。
async function doExportConfig() {
  try {
    const { getConfig, listExtraInstances } = await import("./api");
    const [cfg, extras] = await Promise.all([getConfig(), listExtraInstances()]);
    const exportObj = {
      // 版本标记: import 时校验
      _musage_export_version: 1,
      _exported_at: new Date().toISOString(),
      config: cfg,
      extra_instances: extras,
    };
    const json = JSON.stringify(exportObj, null, 2);
    const blob = new Blob([json], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const dateStr = new Date().toISOString().slice(0, 10).replace(/-/g, "");
    const a = el("a", {
      href: url,
      download: `musage-config-${dateStr}.json`,
    }) as HTMLAnchorElement;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    // 释放 blob URL
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    flash(t("settings.advanced.io_exported", { n: 1 }));
  } catch (e) {
    flash(t("settings.advanced.io_import_failed", { err: String(e) }), true);
  }
}

async function doImportConfig(file: File) {
  try {
    const text = await file.text();
    const obj = JSON.parse(text);
    // 校验: 必须有 _musage_export_version + config 字段
    if (obj._musage_export_version !== 1 || typeof obj.config !== "object") {
      flash(t("settings.advanced.io_import_failed", {
        err: "invalid format",
      }), true);
      return;
    }
    const { saveConfig } = await import("./api");
    await saveConfig(obj.config);
    // 注意: extra_instances 单独存 extra_instances.json,不走 saveConfig
    // (PR 1b 设计),import 只覆盖 config 部分。extra_instances 手动添加。
    flash(t("settings.advanced.io_imported", { n: 1 }));
    // 触发刷新当前 panel
    setTimeout(() => location.reload(), 500);
  } catch (e) {
    flash(t("settings.advanced.io_import_failed", { err: String(e) }), true);
  }
}
