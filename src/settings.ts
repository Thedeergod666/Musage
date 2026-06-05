// 设置面板
import { invoke } from "@tauri-apps/api/core";

interface AppConfig {
  region: "cn" | "en";
  refresh_interval_secs: number;
  autostart: boolean;
  floating_x: number | null;
  floating_y: number | null;
}

const $ = <T extends HTMLElement>(s: string): T => {
  const el = document.querySelector<T>(s);
  if (!el) throw new Error(`not found: ${s}`);
  return el;
};

async function load() {
  const hasKey = await invoke<boolean>("has_api_key");
  const cfg = await invoke<AppConfig>("get_config");
  ($("#region") as HTMLSelectElement).value = cfg.region;
  ($("#interval") as HTMLInputElement).value = String(cfg.refresh_interval_secs);
  ($("#autostart") as HTMLInputElement).checked = cfg.autostart;
  ($("#api-key") as HTMLInputElement).value = "";
  ($("#api-key-status") as HTMLElement).textContent = hasKey ? "✓ 已保存到 Windows 凭据" : "未设置";
  ($("#api-key-status") as HTMLElement).style.color = hasKey ? "#4caf50" : "#888";
}

async function save() {
  const cfg: AppConfig = {
    region: ($("#region") as HTMLSelectElement).value as "cn" | "en",
    refresh_interval_secs: parseInt(($("#interval") as HTMLInputElement).value, 10) || 60,
    autostart: ($("#autostart") as HTMLInputElement).checked,
    floating_x: null,
    floating_y: null,
  };
  await invoke("save_config", { cfg });
  flash("✓ 配置已保存");
}

async function saveKey() {
  const key = ($("#api-key") as HTMLInputElement).value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 API key", true);
    return;
  }
  try {
    await invoke("set_api_key", { key });
    ($("#api-key") as HTMLInputElement).value = "";
    await load();
    flash("✓ API key 已保存到 Windows 凭据");
    // 立即拉一次：避免等 60s 轮询周期
    try {
      const snap = await invoke("refresh_now");
      const s = snap as { success: boolean; error?: string };
      if (s.success) {
        flash("✓ API key 已保存并测试通过");
        // 测试成功：关闭 settings 窗口
        setTimeout(() => invoke("hide_settings_window").catch(() => {}), 800);
      } else {
        flash(`✗ key 已存但拉取失败: ${s.error ?? "?"}`, true);
      }
    } catch (e) {
      flash(`✗ key 已存但拉取异常: ${e}`, true);
    }
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

async function deleteKey() {
  if (!confirm("确认删除保存的 API key？")) return;
  await invoke("delete_api_key");
  await load();
  flash("✓ 已删除");
}

async function testConn() {
  flash("测试中…");
  try {
    const snap = await withTimeout(invoke("refresh_now"), 12000, "请求超时（12s）");
    flash(`✓ 成功: 5h ${(snap as any).five_hour?.utilization ?? "?"}% / 周 ${(snap as any).weekly?.utilization ?? "?"}%`);
  } catch (e) {
    flash(`✗ 失败: ${e}`, true);
  }
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

let flashTimer: number | null = null;
function flash(msg: string, isError = false) {
  const el = $("#flash") as HTMLElement;
  el.textContent = msg;
  el.style.color = isError ? "#f44336" : "#4caf50";
  if (flashTimer !== null) clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (el.textContent = ""), 4000);
}

$("#save")?.addEventListener("click", save);
$("#save-key")?.addEventListener("click", saveKey);
$("#del-key")?.addEventListener("click", deleteKey);
$("#test")?.addEventListener("click", testConn);

load();
