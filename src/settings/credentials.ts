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
  refreshNow,
} from "./api";
import { $, el, flash, providerDisplay } from "./utils";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ProviderId, SourceMeta } from "./types";

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

// ────────────────────────────────────────────────────────────────
// v0.6+ 动态渲染：registry-driven 凭据块
//
// `renderCredentialBlock(meta)` 给 createProviderPanel() 用，根据
// `meta.auth_kind` 决定 input vs textarea + 按钮组。事件用 document-level
// 委托（data-id 路由），避免每个 panel 都 addEventListener 100 遍。
// ────────────────────────────────────────────────────────────────

/// 按 meta.auth_kind 创建凭据输入 + 状态徽章 + 保存/删除/（可选）复制按钮
export function renderCredentialBlock(meta: SourceMeta): HTMLElement {
  const block = el("div", { class: "cred-block" });

  if (meta.auth_kind === "api_key") {
    // ── input + 📋 复制 + 状态 + 保存/删除 ──
    const input = el("input", {
      type: "password",
      id: `api-key-${meta.id}`,
      "data-id": meta.id,
      placeholder: apiKeyPlaceholder(meta.id),
      autocomplete: "off",
    }) as HTMLInputElement;
    const copy = el("button", {
      id: `copy-key-${meta.id}`,
      "data-id": meta.id,
      "data-action": "copy-key",
      title: "复制到剪贴板",
    }, "📋");
    block.appendChild(
      el("div", { class: "input-row" }, input, copy),
    );
    block.appendChild(
      el("div", {
        class: "status",
        id: `api-key-status-${meta.id}`,
        "data-id": meta.id,
      }, "—"),
    );
    block.appendChild(
      el("div", { class: "row" },
        el("button", { class: "primary", id: `save-key-${meta.id}`, "data-id": meta.id, "data-action": "save-key" }, "保存"),
        el("button", { class: "danger", id: `del-key-${meta.id}`, "data-id": meta.id, "data-action": "del-key" }, "删除"),
      ),
    );
    block.appendChild(
      el("div", { class: "help" },
        ...apiKeyHelpNodes(meta.id),
      ),
    );
  } else if (meta.auth_kind === "api_key_or_cookie") {
    // ── 多鉴权（Xiaomi 用）：两个独立块 ──
    // 优先 API key，401 时自动降级到 Cookie。两个都展示，各自独立保存。
    // delete 在 v1 是"清掉该 id 全部凭据"（api_key + cookie 都清）——
    // 用户要保留某一项时只能重输，可接受：v1 UX 重点是"丢 key 就行"。
    return renderMultiAuthBlock(meta);
  } else {
    // ── 纯 cookie 模式（备用，目前未使用，保留兼容）──
    const textarea = el("textarea", {
      id: `cookie-${meta.id}`,
      "data-id": meta.id,
      rows: "4",
      placeholder: 'api-platform_serviceToken="..."; userId=...; api-platform_slh="..."; api-platform_ph="..."',
    }) as HTMLTextAreaElement;
    block.appendChild(
      el("div", { class: "field" },
        el("label", {}, "Cookie header 值"),
        textarea,
        el("div", { class: "status", id: `cookie-status-${meta.id}`, "data-id": meta.id }, "—"),
      ),
    );
    block.appendChild(
      el("div", { class: "row" },
        el("button", { class: "primary", id: `save-cookie-${meta.id}`, "data-id": meta.id, "data-action": "save-cookie" }, "保存 Cookie"),
        el("button", { class: "danger", id: `del-cookie-${meta.id}`, "data-id": meta.id, "data-action": "del-cookie" }, "删除 Cookie"),
      ),
    );
    block.appendChild(
      el("div", { class: "help" },
        ...cookieHelpNodes(),
      ),
    );
  }
  return block;
}

/// 多鉴权 source（`api_key_or_cookie`）的凭据块：两个独立块。
///
/// 数据流：
/// - save-key   按钮 → `setSourceCredential(id, value, "api_key")` → 落 api_key 字段
/// - save-cookie 按钮 → `setSourceCredential(id, value, "cookie")`  → 落 cookie 字段
/// - delete-*   按钮 → `deleteSourceCredential(id)` → 清掉该 id 的全部凭据
///   （v1 限制：会同时清 api_key + cookie）
///
/// fetch 路径（[src-tauri/src/providers/xiaomi.rs::decide_auth_strategy]）：
/// - 两个都填 → 先 Bearer（API key），401 自动退到 Cookie
/// - 只填一个 → 走对应单轨
function renderMultiAuthBlock(meta: SourceMeta): HTMLElement {
  const block = el("div", { class: "cred-block" });

  // ── 顶部快捷登录（小米专用）──
  // 一键弹 webview 让用户登录小米账号 → 后端自动提取 cookie 写进 keys.json
  // → 告别"开 DevTools 复制 header"。**只对 xiaomimimo 启用**，其他多鉴权
  // source 没接 dashboard 登录链路就显示占位。
  if (meta.id === "xiaomimimo") {
    block.appendChild(
      el("div", { class: "quick-login-banner" },
        el("div", { class: "quick-login-text" },
          el("strong", {}, "🚀 不想手动抄 Cookie？"),
          el("br"),
          "点下面按钮 → 弹窗登录小米账号 → 后端自动提取 Cookie 写进 keys.json，",
          "全程不需要碰 DevTools。",
        ),
        el("button", {
          class: "primary big",
          id: `xiaomi-login-${meta.id}`,
          "data-id": meta.id,
          "data-action": "xiaomi-login",
        }, "🔑 登录小米账号"),
      ),
    );
  }

  // ── Block 1: API key（优先）──
  block.appendChild(
    el("div", { class: "field" },
      el("label", {}, "API key（优先尝试）"),
      el("div", { class: "input-row" },
        el("input", {
          type: "password",
          id: `api-key-${meta.id}`,
          "data-id": meta.id,
          placeholder: apiKeyPlaceholder(meta.id),
          autocomplete: "off",
        }) as HTMLInputElement,
      ),
      el("div", { class: "status", id: `api-key-status-${meta.id}`, "data-id": meta.id }, "—"),
      el("div", { class: "row" },
        el("button", { class: "primary", id: `save-key-${meta.id}`, "data-id": meta.id, "data-action": "save-key" }, "保存 API key"),
        el("button", { class: "danger", id: `del-key-${meta.id}`, "data-id": meta.id, "data-action": "del-key" }, "删除"),
      ),
      el("div", { class: "help" },
        ...apiKeyHelpNodes(meta.id),
        el("br"),
        el("strong", {}, "提示："),
        "Xiaomi 用量 API 当前对 Bearer 返 401，但别担心——",
        "下面的 Cookie 一旦配好，Bearer 失败时会自动 fallback（你不用手动切）。",
      ),
    ),
  );

  // ── Block 2: Cookie（兜底）──
  block.appendChild(
    el("div", { class: "field" },
      el("label", {}, "Dashboard Cookie（兜底：401 时自动退到这里）"),
      el("textarea", {
        id: `cookie-${meta.id}`,
        "data-id": meta.id,
        rows: "4",
        placeholder: 'api-platform_serviceToken="..."; userId=...; api-platform_slh="..."; api-platform_ph="..."',
      }) as HTMLTextAreaElement,
      el("div", { class: "status", id: `cookie-status-${meta.id}`, "data-id": meta.id }, "—"),
      el("div", { class: "row" },
        el("button", { class: "primary", id: `save-cookie-${meta.id}`, "data-id": meta.id, "data-action": "save-cookie" }, "保存 Cookie"),
        el("button", { class: "danger", id: `del-cookie-${meta.id}`, "data-id": meta.id, "data-action": "del-cookie" }, "删除"),
      ),
      el("div", { class: "help" },
        ...cookieHelpNodes(),
      ),
    ),
  );

  return block;
}

// ── 各 provider 的占位符 + 帮助文本 ────────────────────────────

function apiKeyPlaceholder(id: string): string {
  switch (id) {
    case "minimax":    return "sk-cp-...";
    case "deepseek":   return "sk-...";
    case "xiaomimimo": return "tp-...";
    case "tavily":     return "tvly-...";
    case "zenmux":     return "sk-...";
    case "openrouter": return "sk-or-v1-...";
    case "kimi":       return "sk-...";
    case "zhipu":      return "id.secret";
    default:           return "...";
  }
}

/// 返回 help 文本的节点数组（含 inline 链接用 a 元素）
function apiKeyHelpNodes(id: string): (Node | string)[] {
  switch (id) {
    case "minimax":
      return [
        "🔒 密钥以 ",
        el("code", {}, "0600"),
        " 权限存到本机 ",
        el("code", {}, "keys.json"),
        "（与 config 同目录，不走系统钥匙串，启动零弹窗）。",
        el("br"),
        "从 ",
        el("a", { href: "https://platform.minimaxi.com/user-center/basic-information/interface-key", target: "_blank", class: "link-ext" }, "platform.minimaxi.com"),
        " 获取。",
      ];
    case "deepseek":
      return [
        "🔒 密钥以 ",
        el("code", {}, "0600"),
        " 权限存到本机 ",
        el("code", {}, "keys.json"),
        "（与 MiniMax 的 key 互不影响）。",
        el("br"),
        "从 ",
        el("a", { href: "https://platform.deepseek.com/api_keys", target: "_blank", class: "link-ext" }, "platform.deepseek.com"),
        " 获取。",
      ];
    case "xiaomimimo":
      return [
        "🔒 Token Plan 专用 key（",
        el("code", {}, "tp-"),
        " 开头，区别于 pay-as-you-go 的 ",
        el("code", {}, "sk-"),
        "）。",
        el("br"),
        "从 ",
        el("a", { href: "https://platform.xiaomimimo.com/tokenplan/subscription", target: "_blank", class: "link-ext" }, "platform.xiaomimimo.com"),
        " 订阅后获取。",
      ];
    case "tavily":
      return [
        "Tavily 是 AI agent 常用的 search API（",
        el("strong", {}, "不是"),
        " LLM token plan）。",
        el("br"),
        "从 ",
        el("a", { href: "https://app.tavily.com/home", target: "_blank", class: "link-ext" }, "app.tavily.com"),
        " 获取 API key。",
      ];
    case "zenmux":
      return [
        "ZenMux 是 AI model gateway（聚合 Claude / GPT / Gemini 等）。",
        el("br"),
        "从 ",
        el("a", { href: "https://zenmux.ai/platform/management", target: "_blank", class: "link-ext" }, "zenmux.ai/platform/management"),
        " 创建。",
      ];
    case "openrouter":
      return [
        "OpenRouter 是 AI model gateway（聚合 130+ 模型）。",
        el("br"),
        "用普通 API key（",
        el("code", {}, "sk-or-v1-"),
        " 开头）即可，",
        el("strong", {}, "不需要"),
        " Management key。",
        el("br"),
        "从 ",
        el("a", { href: "https://openrouter.ai/settings/keys", target: "_blank", class: "link-ext" }, "openrouter.ai/settings/keys"),
        " 获取。",
      ];
    case "kimi":
      return [
        "Kimi 是月之暗面（Moonshot AI）的编程套餐，5h + 周双窗口。",
        el("br"),
        "从 ",
        el("a", { href: "https://platform.moonshot.cn/console/api-keys", target: "_blank", class: "link-ext" }, "platform.moonshot.cn"),
        " 创建 API key。",
      ];
    case "zhipu":
      return [
        "智谱 GLM Coding Plan 套餐，5h + 周双窗口。",
        el("br"),
        el("strong", {}, "鉴权特殊"),
        "：",
        el("code", {}, "Authorization"),
        " 头",
        el("strong", {}, "不加"),
        " ",
        el("code", {}, "Bearer"),
        " 前缀 —— 直接用裸 key（",
        el("code", {}, "id.secret"),
        " 格式）。",
        el("br"),
        "在「区域」下拉里选 CN（",
        el("a", { href: "https://bigmodel.cn/user-center/projection-meter", target: "_blank", class: "link-ext" }, "bigmodel.cn"),
        "）或 EN（",
        el("a", { href: "https://z.ai/manage-apikey/subscription", target: "_blank", class: "link-ext" }, "z.ai"),
        "）。两个平台的 key 不通用。",
      ];
    default:
      return ["API key 存到本机 keys.json。"];
  }
}

function cookieHelpNodes(): (Node | string)[] {
  return [
    "⚠️ Xiaomi 用量走 dashboard admin API，需要浏览器登录态。",
    el("br"),
    el("strong", {}, "获取方法"),
    "：Chrome 登录 ",
    el("a", { href: "https://platform.xiaomimimo.com/console/plan-manage", target: "_blank", class: "link-ext" }, "platform.xiaomimimo.com"),
    " → F12 → Network → 任意 ",
    el("code", {}, "/api/v1/tokenPlan/*"),
    " 请求 → 右键 → Copy → Copy request headers → 找 ",
    el("code", {}, "cookie:"),
    " 这一行整段粘贴到上面。",
    el("br"),
    "Cookie 登出后失效，过期时 (HTTP 401) 错误信息会引导重粘。",
  ];
}

// ── 统一 id-based 凭据操作（动态 panel 按钮事件委托走这里）──

export async function loadCredentialStatus(id: string) {
  const has = await hasSourceCredential(id);
  const status = document.getElementById(`api-key-status-${id}`)
    ?? document.getElementById(`cookie-status-${id}`);
  if (status) {
    status.textContent = has ? "✓ 已保存到本机" : "未设置";
    status.className = `status ${has ? "ok" : ""}`;
  }
}

export async function saveCredentialAction(id: string, action: "key" | "cookie") {
  const inputId = action === "key" ? `api-key-${id}` : `cookie-${id}`;
  const input = document.getElementById(inputId) as HTMLInputElement | HTMLTextAreaElement | null;
  if (!input) return;
  const value = input.value.trim();
  if (!value) {
    flash("⚠ 请先粘贴", true);
    return;
  }
  try {
    // 多鉴权 source 必传 field hint，否则两个输入都落 api_key。
    // 单鉴权 source（auth_kind=api_key / cookie）忽略 field 走默认也安全。
    await setSourceCredential(id, value, action === "key" ? "api_key" : "cookie");
    input.value = "";
    // 立即更新对应字段的"已保存"状态（不等 loadCredentialStatus 兜底），
    // 多鉴权时 loadCredentialStatus 只能更新第一个找到的 status 元素，
    // 这里精确到字段。
    const statusId = action === "key" ? `api-key-status-${id}` : `cookie-status-${id}`;
    const status = document.getElementById(statusId);
    if (status) {
      status.textContent = "✓ 已保存到本机";
      status.className = "status ok";
    }
    flash(`✓ ${providerDisplay(id as ProviderId)} 已保存`);
    await refreshNow();
  } catch (e) {
    flash(`✗ 保存失败: ${e}`, true);
  }
}

export async function deleteCredentialAction(id: string, action: "key" | "cookie") {
  const label = action === "key" ? "API key" : "Cookie";
  if (!confirm(`确认删除 ${providerDisplay(id as ProviderId)} 的 ${label}？`)) return;
  // 后端 delete_source_credential 会同时清 api_key 和 cookie，统一用一个入口
  await deleteSourceCredential(id);
  await loadCredentialStatus(id);
  flash("✓ 已删除");
}

export async function copyCredentialAction(id: string) {
  try {
    const value = await getSourceCredential(id);
    if (!value) {
      flash(`⚠ ${providerDisplay(id as ProviderId)} 未设置 key`, true);
      return;
    }
    await navigator.clipboard.writeText(value);
    flash(`✓ ${providerDisplay(id as ProviderId)} key 已复制到剪贴板`);
  } catch (e) {
    flash(`✗ 复制失败: ${e}`, true);
  }
}

/// 一键登录小米账号：弹 webview → 用户登录 → 后端自动提取 cookie。
///
/// 数据流：
/// 1. `invoke("open_xiaomi_login_window")` → 后端开 webview
/// 2. 用户在 webview 里登录小米账号
/// 3. 后端监听到 dashboard URL → 提取 cookie → 写 keys.json → 关 webview
/// 4. 后端 emit `musage://xiaomi-login-success` / `-failed`
/// 5. 本函数在 init 时绑一次事件监听（见 `bindXiaomiLoginEvents`）
export async function xiaomiLoginAction(id: string) {
  if (id !== "xiaomimimo") {
    flash("⚠ 一键登录只对 Xiaomi 启用", true);
    return;
  }
  try {
    await invoke("open_xiaomi_login_window");
    flash("🔑 已打开登录窗口 —— 登录完成后会自动关闭窗口");
  } catch (e) {
    flash(`✗ 打开登录窗口失败: ${e}`, true);
  }
}

/// 绑一次后端登录事件 → UI 反馈。
/// 在 main.ts init() 末尾调一次就行（**只绑一次**，多次绑会重复响应）。
export function bindXiaomiLoginEvents() {
  // 走 Tauri 2 标准 listen API（不是 window.__TAURI__ 全局，TS 类型干净）
  void listen<number>("musage://xiaomi-login-success", (e) => {
    const savedLen = e.payload;
    flash(`✓ Xiaomi 登录成功，已保存 cookie（${savedLen} 字节）`);
    // 立即刷新状态徽章
    void loadCredentialStatus("xiaomimimo");
  });
  void listen<string>("musage://xiaomi-login-failed", (e) => {
    flash(`✗ Xiaomi 登录失败: ${e.payload}`, true);
  });
}

/// document-level 委托：处理动态 panel 里的 save-key / del-key / copy-key / save-cookie / del-cookie / xiaomi-login
/// 在 main.ts init() 末尾调一次就行。
export function bindCredentialButtonsGlobal() {
  document.addEventListener("click", (e) => {
    const t = e.target as HTMLElement;
    const action = t.dataset.action;
    const id = t.dataset.id;
    if (!action || !id) return;
    switch (action) {
      case "save-key":
        void saveCredentialAction(id, "key");
        break;
      case "del-key":
        void deleteCredentialAction(id, "key");
        break;
      case "copy-key":
        void copyCredentialAction(id);
        break;
      case "save-cookie":
        void saveCredentialAction(id, "cookie");
        break;
      case "del-cookie":
        void deleteCredentialAction(id, "cookie");
        break;
      case "xiaomi-login":
        void xiaomiLoginAction(id);
        break;
    }
  });
}
