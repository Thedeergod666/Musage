// 「测试连接」按钮：refresh_now + 把每个 provider 的核心指标压成一行 flash

import { refreshNow } from "./api";
import { flash, formatAmount, withTimeout } from "./utils";

export async function testConn() {
  flash("测试中…");
  try {
    const snap = await withTimeout(
      refreshNow(),
      12000,
      "请求超时（12s）",
    );
    const ok = snap.providers.filter((p) => p.success);
    if (ok.length === 0) {
      const errs = snap.providers
        .map((p) => {
          const id = p.source_id ?? p.provider;
          return `${id}: ${p.error}`;
        })
        .join("; ");
      flash(`✗ 全部失败: ${errs}`, true);
      return;
    }
    const summary = ok
      .map((p) => {
        // Phase 1：用 source_id 路由（registry 驱动），provider 字段保兼容
        const id = p.source_id ?? p.provider;
        if (id === "minimax") {
          const fiveHour = p.rows.find((r) => r.utilization != null);
          return fiveHour
            ? `MiniMax 5h ${Math.round(fiveHour.utilization ?? 0)}%`
            : "MiniMax OK";
        } else if (id === "deepseek") {
          const balance = p.rows.find((r) => r.remaining != null);
          return balance
            ? `DeepSeek ${formatAmount(balance.remaining ?? 0)} ${balance.unit ?? ""}`
            : "DeepSeek OK";
        } else if (id === "xiaomimimo") {
          const plan = p.rows.find((r) => r.utilization != null);
          return plan
            ? `Xiaomi 套餐 ${Math.round(plan.utilization ?? 0)}%`
            : "Xiaomi OK";
        } else if (id === "tavily") {
          // 主指标在 used/total/unit="credits" 的那一行
          const main = p.rows.find((r) => r.unit === "credits");
          if (main && main.used != null && main.total != null) {
            return `Tavily ${Math.round(main.used)}/${Math.round(main.total)} credits`;
          }
          return "Tavily OK";
        }
        return `${id} OK`;
      })
      .join(" / ");
    flash(`✓ ${summary}`);
  } catch (e) {
    flash(`✗ 失败: ${e}`, true);
  }
}
