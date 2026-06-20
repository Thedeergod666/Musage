// 「测试连接」按钮：refresh_now + 把每个 provider 的核心指标压成一行 flash

import { refreshNow } from "./api";
import { flash, formatAmount, withTimeout } from "./utils";
import { t } from "../i18n";

export async function testConn() {
  flash(t("settings.test.testing"));
  try {
    const snap = await withTimeout(
      refreshNow(),
      12000,
      t("settings.test.timeout"),
    );
    const ok = snap.providers.filter((p) => p.success);
    if (ok.length === 0) {
      const errs = snap.providers
        .map((p) => {
          const id = p.source_id ?? p.provider;
          return `${id}: ${p.error}`;
        })
        .join("; ");
      flash(t("settings.test.all_failed", { errs }), true);
      return;
    }
    const summary = ok
      .map((p) => {
        // Phase 1：用 source_id 路由（registry 驱动），provider 字段保兼容
        const id = p.source_id ?? p.provider;
        if (id === "minimax") {
          const fiveHour = p.rows.find((r) => r.utilization != null);
          return fiveHour
            ? t("settings.test.minimax_5h", { pct: Math.round(fiveHour.utilization ?? 0) })
            : t("settings.test.minimax_ok");
        } else if (id === "deepseek") {
          const balance = p.rows.find((r) => r.remaining != null);
          return balance
            ? t("settings.test.deepseek_balance", {
                amount: formatAmount(balance.remaining ?? 0),
                unit: balance.unit ?? "",
              })
            : t("settings.test.deepseek_ok");
        } else if (id === "xiaomimimo") {
          const plan = p.rows.find((r) => r.utilization != null);
          return plan
            ? t("settings.test.xiaomi_plan", { pct: Math.round(plan.utilization ?? 0) })
            : t("settings.test.xiaomi_ok");
        } else if (id === "tavily") {
          // 主指标在 used/total/unit="credits" 的那一行
          const main = p.rows.find((r) => r.unit === "credits");
          if (main && main.used != null && main.total != null) {
            return t("settings.test.tavily_credits", {
              used: Math.round(main.used),
              total: Math.round(main.total),
            });
          }
          return t("settings.test.tavily_ok");
        } else if (id === "kimi") {
          // Kimi 与 MiniMax 同款 5h/周 百分比行
          const fiveHour = p.rows.find((r) => r.label === "5h");
          return fiveHour
            ? t("settings.test.kimi_5h", { pct: Math.round(fiveHour.utilization ?? 0) })
            : t("settings.test.kimi_ok");
        } else if (id === "zhipu") {
          // CN = "智谱 GLM"，EN = "Z.ai"（后端 source_display_name 决定）
          // 2026-06-20 audit：之前硬编码 "Z.ai" 字符串（漏 t()）。改用现有
// provider.zhipu_en.name i18n key 跟其它 provider 路径一致。
const label = p.source_display_name === "Z.ai" ? t("provider.zhipu_en.name") : t("provider.zhipu_cn.name");
          const fiveHour = p.rows.find((r) => r.label === "5h");
          return fiveHour
            ? t("settings.test.zhipu_5h", {
                label,
                pct: Math.round(fiveHour.utilization ?? 0),
              })
            : t("settings.test.zhipu_ok", { label });
        }
        return t("settings.test.generic_ok", { id });
      })
      .join(" / ");
    flash(t("settings.test.success", { summary }));
  } catch (e) {
    flash(t("settings.test.failed", { err: String(e) }), true);
  }
}
