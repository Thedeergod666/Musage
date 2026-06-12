// "高级" section —— Schema overrides（MiniMax / Xiaomi 改字段名后用）
//
// 只在解析失败 / 显示 "Schema 未知" 时用。每行一个 JSON 对象，total + remaining
// 同时存在才算命中，end 可选。改完 blur 触发 saveConfig()。

import { el } from "./utils";
import type { AppConfig } from "./types";

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

  container.appendChild(
    el("section", { class: "section-card" },
      el("h2", {}, "🔧 高级"),
      el("div", { class: "help" },
        "只在 ",
        el("strong", {}, "解析失败 / 显示 \"Schema 未知\""),
        " 时用。每行一个 JSON 对象，",
        el("code", {}, "total"),
        " + ",
        el("code", {}, "remaining"),
        " 同时存在才算命中，",
        el("code", {}, "end"),
        " 可选。",
        el("br"),
        "改完点「保存配置」（或等 Stage 6 改成 blur 即时落盘）。参考 ccswitch 的 schema 字段名逆向。",
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
}
