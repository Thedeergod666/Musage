// 设置面板 —— 多 provider key + 全局配置
import { invoke } from "@tauri-apps/api/core";

type ProviderId = "minimax" | "deepseek";

interface ProviderConfig {
  enabled: boolean;
  region?: "cn" | "en" | null;
}

interface FieldTriple {
  total: string;
  remaining: string;
  end?: string | null;
}

interface TierOverrides {
  count_candidates: FieldTriple[];
}

interface ProviderOverrides {
  five_hour: TierOverrides;
  weekly: TierOverrides;
}

interface AppConfig {
  providers: Record<string, ProviderConfig>;
  refresh_interval_secs: number;
  autostart: boolean;
  floating_x: number | null;
  floating_y: number | null;
  // 用户加的字段名候选（应对 MiniMax 改 schema）
  schema_overrides?: Record<string, ProviderOverrides>;
}

interface ProviderSnapshot {
  provider: ProviderId;
  success: boolean;
  rows: Array<{
    label: string;
    utilization: number | null;
    remaining: number | null;
    unit: string | null;
  }>;
  error: string | null;
  error_kind?:
    | "unconfigured_key"
    | "auth_failed"
    | "rate_limited"
    | "network"
    | "parse"
    | "schema_unknown"
    | "server_error"
    | "other"
    | null;
}

interface QuotaSnapshot {
  providers: ProviderSnapshot[];
  fetched_at: number | null;
}

const $ = <T extends HTMLElement>(s: string): T => {
  const el = document.querySelector<T>(s);
  if (!el) throw new Error(`not found: ${s}`);
  return el;
};

// ── Tab 切换 ──

function setupTabs() {
  const tabs = document.querySelectorAll<HTMLButtonElement>(".tab");
  const panels = document.querySelectorAll<HTMLElement>(".provider-panel");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const p = tab.dataset.p!;
      tabs.forEach((t) => t.classList.toggle("active", t.dataset.p === p));
      panels.forEach((pn) => pn.classList.toggle("active", pn.dataset.p === p));
    });
  });
}

// ── Provider key 加载 / 保存 / 删除 ──

async function loadKeyStatus(provider: ProviderId) {
  const has = await invoke<boolean>("has_api_key_for", { provider });
  const el = $(`#api-key-status-${provider}`);
  el.textContent = has ? "✓ 已保存到本机" : "未设置";
  el.className = `status ${has ? "ok" : ""}`;
  $(`#api-key-${provider}` as keyof HTMLElementTagNameMap as string) as HTMLInputElement;
}

async function saveKey(provider: ProviderId) {
  const input = $(`#api-key-${provider}`) as HTMLInputElement;
  const key = input.value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 API key", true);
    return;
  }
  try {
    await invoke("set_api_key_for", { provider, key });
    input.value = "";
    await loadKeyStatus(provider);
    flash(`✓ ${providerDisplay(provider)} key 已保存`);
    // 立即拉一次
    await testConn();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

async function deleteKey(provider: ProviderId) {
  if (!confirm(`确认删除 ${providerDisplay(provider)} 的 API key？`)) return;
  await invoke("delete_api_key_for", { provider });
  await loadKeyStatus(provider);
  flash("✓ 已删除");
}

// 从 keys.json 读明文 → 写剪贴板。用完即弃，不在 JS 侧长期保存。
async function copyKey(provider: ProviderId) {
  try {
    const key = await invoke<string | null>("get_api_key_for", { provider });
    if (!key) {
      flash(`⚠ ${providerDisplay(provider)} 未设置 key`, true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash(`✓ ${providerDisplay(provider)} key 已复制到剪贴板`);
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
  }
}

// ── 全局配置加载 / 保存 ──

async function loadConfig() {
  const cfg = await invoke<AppConfig>("get_config");
  const regionEl = $("#region") as HTMLSelectElement;
  const minimaxRegion = cfg.providers?.minimax?.region ?? "cn";
  regionEl.value = minimaxRegion;
  ($("#interval") as HTMLInputElement).value = String(cfg.refresh_interval_secs);
  ($("#autostart") as HTMLInputElement).checked = cfg.autostart;

  // schema overrides (高级)
  const ov = cfg.schema_overrides ?? {};
  const mm = ov.minimax ?? { five_hour: { count_candidates: [] }, weekly: { count_candidates: [] } };
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
}

async function saveConfig() {
  // 解析 schema overrides 的 JSON；解析失败给提示但不影响其它字段保存
  let fiveHourCandidates: FieldTriple[] = [];
  let weeklyCandidates: FieldTriple[] = [];
  try {
    const raw5h = ($("#overrides-5h-minimax") as HTMLTextAreaElement).value.trim() || "[]";
    const rawWeek = ($("#overrides-weekly-minimax") as HTMLTextAreaElement).value.trim() || "[]";
    fiveHourCandidates = JSON.parse(raw5h);
    weeklyCandidates = JSON.parse(rawWeek);
    if (!Array.isArray(fiveHourCandidates) || !Array.isArray(weeklyCandidates)) {
      throw new Error("必须是 JSON 数组");
    }
  } catch (e) {
    flash(`✗ Schema overrides JSON 解析失败: ${e}`, true);
    return;
  }

  const cfg: AppConfig = {
    providers: {
      minimax: {
        enabled: true,
        region: ($("#region") as HTMLSelectElement).value as "cn" | "en",
      },
      deepseek: { enabled: true },
    },
    refresh_interval_secs: parseInt(($("#interval") as HTMLInputElement).value, 10) || 60,
    autostart: ($("#autostart") as HTMLInputElement).checked,
    floating_x: null,
    floating_y: null,
    schema_overrides: {
      minimax: {
        five_hour: { count_candidates: fiveHourCandidates },
        weekly: { count_candidates: weeklyCandidates },
      },
      deepseek: { five_hour: { count_candidates: [] }, weekly: { count_candidates: [] } },
    },
  };
  try {
    await invoke("save_config", { cfg });
    flash("✓ 配置已保存");
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

// ── 测试连接 ──

async function testConn() {
  flash("测试中…");
  try {
    const snap = await withTimeout(invoke<QuotaSnapshot>("refresh_now"), 12000, "请求超时（12s）");
    const ok = snap.providers.filter((p) => p.success);
    if (ok.length === 0) {
      const errs = snap.providers.map((p) => `${p.provider}: ${p.error}`).join("; ");
      flash(`✗ 全部失败: ${errs}`, true);
      return;
    }
    const summary = ok
      .map((p) => {
        if (p.provider === "minimax") {
          const fiveHour = p.rows.find((r) => r.utilization != null);
          return fiveHour
            ? `MiniMax 5h ${Math.round(fiveHour.utilization ?? 0)}%`
            : "MiniMax OK";
        } else {
          const balance = p.rows.find((r) => r.remaining != null);
          return balance
            ? `DeepSeek ${formatAmount(balance.remaining ?? 0)} ${balance.unit ?? ""}`
            : "DeepSeek OK";
        }
      })
      .join(" / ");
    flash(`✓ ${summary}`);
  } catch (e) {
    flash(`✗ 失败: ${e}`, true);
  }
}

function formatAmount(v: number): string {
  return v.toLocaleString("zh-CN", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function withTimeout<T>(p: Promise<T>, ms: number, msg: string): Promise<T> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(msg)), ms);
    p.then(
      (v) => { clearTimeout(t); resolve(v); },
      (e) => { clearTimeout(t); reject(e); },
    );
  });
}

// ── 工具 ──

let flashTimer: number | null = null;
function flash(msg: string, isError = false) {
  const el = $("#flash") as HTMLElement;
  el.textContent = msg;
  el.style.color = isError ? "#f44336" : "#4caf50";
  if (flashTimer !== null) clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (el.textContent = ""), 5000);
}

function providerDisplay(p: ProviderId): string {
  return p === "minimax" ? "MiniMax" : "DeepSeek";
}

// ── 启动 ──

setupTabs();

$("#save")?.addEventListener("click", saveConfig);
$("#save-key-minimax")?.addEventListener("click", () => saveKey("minimax"));
$("#save-key-deepseek")?.addEventListener("click", () => saveKey("deepseek"));
$("#del-key-minimax")?.addEventListener("click", () => deleteKey("minimax"));
$("#del-key-deepseek")?.addEventListener("click", () => deleteKey("deepseek"));
$("#copy-key-minimax")?.addEventListener("click", () => copyKey("minimax"));
$("#copy-key-deepseek")?.addEventListener("click", () => copyKey("deepseek"));
$("#test")?.addEventListener("click", testConn);

$("#reset-floating")?.addEventListener("click", async () => {
  const btn = $("#reset-floating") as HTMLButtonElement;
  btn.disabled = true;
  try {
    await invoke("reset_floating_window");
    flash("✓ 浮窗已归位到主屏幕正中央");
  } catch (e) {
    flash(`✗ 归位失败: ${e}`, true);
  } finally {
    btn.disabled = false;
  }
});

(async () => {
  await loadKeyStatus("minimax");
  await loadKeyStatus("deepseek");
  await loadConfig();
})();
