// 凭据 load / save / delete / copy
//
// 3 条路径：
// 1. 旧 API（按 Provider enum）：minimax / deepseek 走 API key；xiaomimimo 走 cookie
// 2. 新 API（按 string id）：tavily / zenmux 走 API key
// 3. 同样的字符串 id 路径理论上也覆盖 minimax / deepseek（Phase 2+ 会切），
//    当前保留两套是因为 enum 路径还要支持 cookie 模式

import {
  deleteApiKeyFor,
  deleteCookieFor,
  deleteSourceCredential,
  getApiKeyFor,
  getSourceCredential,
  hasApiKeyFor,
  hasCookieFor,
  hasSourceCredential,
  setApiKeyFor,
  setCookieFor,
  setSourceCredential,
} from "./api";
import { $, flash, providerDisplay } from "./utils";
import type { ProviderId } from "./types";

// ── 旧 enum-based API（minimax / deepseek / xiaomimimo api_key）──

export async function loadKeyStatus(provider: ProviderId) {
  const has = await hasApiKeyFor(provider);
  const el = $(`#api-key-status-${provider}`);
  el.textContent = has ? "✓ 已保存到本机" : "未设置";
  el.className = `status ${has ? "ok" : ""}`;
}

export async function saveKey(provider: ProviderId) {
  const input = $(`#api-key-${provider}`) as HTMLInputElement;
  const key = input.value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 API key", true);
    return;
  }
  try {
    await setApiKeyFor(provider, key);
    input.value = "";
    await loadKeyStatus(provider);
    flash(`✓ ${providerDisplay(provider)} key 已保存`);
    // 立即拉一次
    const { refreshNow } = await import("./api");
    await refreshNow();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

export async function deleteKey(provider: ProviderId) {
  if (!confirm(`确认删除 ${providerDisplay(provider)} 的 API key？`)) return;
  await deleteApiKeyFor(provider);
  await loadKeyStatus(provider);
  flash("✓ 已删除");
}

// 从 keys.json 读明文 → 写剪贴板。用完即弃，不在 JS 侧长期保存。
export async function copyKey(provider: ProviderId) {
  try {
    const key = await getApiKeyFor(provider);
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

// ── Cookie（xiaomimimo 单独） ─────────────────────────────────

export async function loadCookieStatus(provider: ProviderId) {
  const has = await hasCookieFor(provider);
  const el = document.getElementById(`cookie-status-${provider}`);
  if (el) {
    el.textContent = has ? "✓ 已保存到本机" : "未设置";
    el.className = `status ${has ? "ok" : ""}`;
  }
}

export async function saveCookie(provider: ProviderId) {
  const input = document.getElementById(
    `cookie-${provider}`,
  ) as HTMLTextAreaElement | null;
  if (!input) return;
  const cookie = input.value.trim();
  if (!cookie) {
    flash("⚠ 请先粘贴 Cookie", true);
    return;
  }
  try {
    await setCookieFor(provider, cookie);
    input.value = "";
    await loadCookieStatus(provider);
    flash(`✓ ${providerDisplay(provider)} Cookie 已保存`);
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

export async function deleteCookie(provider: ProviderId) {
  if (!confirm(`确认删除 ${providerDisplay(provider)} 的 Cookie？`)) return;
  await deleteCookieFor(provider);
  await loadCookieStatus(provider);
  flash("✓ Cookie 已删除");
}

// ── 新 id-based API（tavily / zenmux） ────────────────────────

async function loadIdKeyStatus(id: string) {
  const has = await hasSourceCredential(id);
  const el = document.getElementById(`api-key-status-${id}`);
  if (el) {
    el.textContent = has ? "✓ 已保存到本机" : "未设置";
    el.className = `status ${has ? "ok" : ""}`;
  }
}

export async function loadTavilyKeyStatus() {
  await loadIdKeyStatus("tavily");
}

export async function saveTavilyKey() {
  const input = document.getElementById(
    "api-key-tavily",
  ) as HTMLInputElement | null;
  if (!input) return;
  const key = input.value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 Tavily API key", true);
    return;
  }
  try {
    await setSourceCredential("tavily", key);
    input.value = "";
    await loadTavilyKeyStatus();
    flash("✓ Tavily key 已保存");
    const { refreshNow } = await import("./api");
    await refreshNow();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

export async function deleteTavilyKey() {
  if (!confirm("确认删除 Tavily 的 API key？")) return;
  await deleteSourceCredential("tavily");
  await loadTavilyKeyStatus();
  flash("✓ Tavily key 已删除");
}

export async function copyTavilyKey() {
  try {
    const key = await getSourceCredential("tavily");
    if (!key) {
      flash("⚠ Tavily 未设置 key", true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash("✓ Tavily key 已复制到剪贴板");
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
  }
}

export async function loadZenmuxKeyStatus() {
  await loadIdKeyStatus("zenmux");
}

export async function saveZenmuxKey() {
  const input = document.getElementById(
    "api-key-zenmux",
  ) as HTMLInputElement | null;
  if (!input) return;
  const key = input.value.trim();
  if (!key) {
    flash("⚠ 请先粘贴 ZenMux API key", true);
    return;
  }
  try {
    await setSourceCredential("zenmux", key);
    input.value = "";
    await loadZenmuxKeyStatus();
    flash("✓ ZenMux key 已保存");
    const { refreshNow } = await import("./api");
    await refreshNow();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

export async function deleteZenmuxKey() {
  if (!confirm("确认删除 ZenMux 的 API key？")) return;
  await deleteSourceCredential("zenmux");
  await loadZenmuxKeyStatus();
  flash("✓ ZenMux key 已删除");
}

export async function copyZenmuxKey() {
  try {
    const key = await getSourceCredential("zenmux");
    if (!key) {
      flash("⚠ ZenMux 未设置 key", true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash("✓ ZenMux key 已复制到剪贴板");
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
  }
}
