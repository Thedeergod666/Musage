// 凭据 load / save / delete / copy
//
// 3 条路径：
// 1. 旧 API（按 Provider enum）：minimax / deepseek 走 API key；xiaomimimo 走 cookie
// 2. 新 API（按 string id）：tavily / zenmux 走 API key
// 3. 同样的字符串 id 路径理论上也覆盖 minimax / deepseek（Phase 2+ 会切），
//    当前保留两套是因为 enum 路径还要支持 cookie 模式

import {
  deleteSourceCredential,
  getSourceCredential,
  getXiaomiDisplayMode,
  hasSourceCredential,
  setSourceCredential,
  setXiaomiDisplayMode,
  refreshNow,
} from "./api";
import { el, flash } from "./utils";
import { t } from "../i18n";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ProviderId, SourceMeta } from "./types";

/// 把 i18n 字符串里的 HTML 片段（<code>/<a>/<br> 等）一次性渲染为
/// HTMLElement 节点。`help.<id>` 键的 value 已经在 json 里写好 HTML，
/// 信任度高（无用户输入），可以直接 innerHTML。
function renderHelp(html: string): HTMLElement {
  const div = document.createElement("div");
  div.innerHTML = html;
  return div;
}

// v0.2 (2026-06-22) 删除 8 个旧 enum-based helper (loadKeyStatus / saveKey /
// deleteKey / copyKey / loadCookieStatus / saveCookie / deleteCookie):
//   - Rust 端 has_api_key_for / set_api_key_for / delete_api_key_for / get_api_key_for /
//     has_cookie_for / set_cookie_for / delete_cookie_for 已删 (PR 5 合并到 PR 4)
//   - 前端必须改用 setSourceCredential(id, value) / hasSourceCredential(id) /
//     deleteSourceCredential(id) / getSourceCredential(id)
//   - 新路径逻辑: saveCredentialAction / deleteCredentialAction / copyCredentialAction
//     (文件下方) 走统一按钮事件委托, 不再按 provider id 写死

// ── 统一 id-based 凭据操作（动态 panel 按钮事件委托走这里）──

async function loadIdKeyStatus(id: string) {
  const has = await hasSourceCredential(id);
  const el = document.getElementById(`api-key-status-${id}`);
  if (el) {
    el.textContent = has
      ? t("credentials.cookie_status_saved")
      : t("credentials.cookie_status_unset");
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
    flash(t("credentials.flash_paste_tavily"), true);
    return;
  }
  try {
    await setSourceCredential("tavily", key);
    input.value = "";
    await loadTavilyKeyStatus();
    flash(t("credentials.flash_saved_key", { name: t("provider.tavily.name") }));
    const { refreshNow } = await import("./api");
    await refreshNow();
  } catch (e) {
    flash(t("credentials.flash_save_failed", { err: String(e) }), true);
  }
}

export async function deleteTavilyKey() {
  if (!confirm(t("credentials.confirm_delete_key_tavily"))) return;
  await deleteSourceCredential("tavily");
  await loadTavilyKeyStatus();
  flash(t("credentials.flash_deleted_tavily"));
}

export async function copyTavilyKey() {
  try {
    const key = await getSourceCredential("tavily");
    if (!key) {
      flash(t("credentials.flash_unset_tavily"), true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash(t("credentials.flash_copy_ok_tavily"));
  } catch (e) {
    flash(t("credentials.flash_copy_failed", { err: String(e) }), true);
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
    flash(t("credentials.flash_paste_zenmux"), true);
    return;
  }
  try {
    await setSourceCredential("zenmux", key);
    input.value = "";
    await loadZenmuxKeyStatus();
    flash(t("credentials.flash_saved_key", { name: t("provider.zenmux.name") }));
    const { refreshNow } = await import("./api");
    await refreshNow();
  } catch (e) {
    flash(t("credentials.flash_save_failed", { err: String(e) }), true);
  }
}

export async function deleteZenmuxKey() {
  if (!confirm(t("credentials.confirm_delete_key_zenmux"))) return;
  await deleteSourceCredential("zenmux");
  await loadZenmuxKeyStatus();
  flash(t("credentials.flash_deleted_zenmux"));
}

export async function copyZenmuxKey() {
  try {
    const key = await getSourceCredential("zenmux");
    if (!key) {
      flash(t("credentials.flash_unset_zenmux"), true);
      return;
    }
    await navigator.clipboard.writeText(key);
    flash(t("credentials.flash_copy_ok_zenmux"));
  } catch (e) {
    flash(t("credentials.flash_copy_failed", { err: String(e) }), true);
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
    const copyBtn = el("button", {
      id: `copy-key-${meta.id}`,
      "data-id": meta.id,
      "data-action": "copy-key",
      title: t("credentials.copy_title"),
    }, "Copy");
    block.appendChild(
      el("div", { class: "input-row" }, input, copyBtn),
    );
    block.appendChild(
      el("div", {
        class: "status",
        id: `api-key-status-${meta.id}`,
        "data-id": meta.id,
      }, t("credentials.cookie_status_placeholder")),
    );
    block.appendChild(
      el("div", { class: "row" },
        el("button", { class: "primary", id: `save-key-${meta.id}`, "data-id": meta.id, "data-action": "save-key" }, t("credentials.save")),
        el("button", { class: "danger", id: `del-key-${meta.id}`, "data-id": meta.id, "data-action": "del-key" }, t("credentials.delete")),
      ),
    );
    block.appendChild(
      el("div", { class: "help" },
        apiKeyHelpNode(meta.id),
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
      placeholder: t("credentials.cookie_textarea_placeholder"),
    }) as HTMLTextAreaElement;
    block.appendChild(
      el("div", { class: "field" },
        el("label", {}, t("credentials.cookie_label")),
        textarea,
        el("div", { class: "status", id: `cookie-status-${meta.id}`, "data-id": meta.id }, t("credentials.cookie_status_placeholder")),
      ),
    );
    block.appendChild(
      el("div", { class: "row" },
        el("button", { class: "primary", id: `save-cookie-${meta.id}`, "data-id": meta.id, "data-action": "save-cookie" }, t("credentials.save_cookie")),
        el("button", { class: "danger", id: `del-cookie-${meta.id}`, "data-id": meta.id, "data-action": "del-cookie" }, t("credentials.del_cookie")),
      ),
    );
    block.appendChild(
      el("div", { class: "help" },
        cookieHelpNode(),
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
          el("strong", {}, t("credentials.cookie_login_hint")),
          el("br"),
          t("credentials.cookie_login_help"),
        ),
        el("div", { class: "row" },
          el("button", {
            class: "primary big",
            id: `xiaomi-login-${meta.id}`,
            "data-id": meta.id,
            "data-action": "xiaomi-login",
          }, t("credentials.login_xiaomi")),
          el("button", {
            class: "danger",
            id: `xiaomi-clear-cookie-${meta.id}`,
            "data-id": meta.id,
            "data-action": "xiaomi-clear-cookie",
          }, t("credentials.clear_cookie")),
          el("a", {
            class: "link-ext",
            href: "https://platform.xiaomimimo.com",
            target: "_blank",
            rel: "noopener noreferrer",
          }, t("credentials.visit_official_site")),
        ),
      ),
    );
  }

  // ── Block 1 + 2: API key + Cookie（hide_credentials 时移至"高级"tab）──
  if (!meta.hide_credentials) {
    block.appendChild(
      el("div", { class: "field" },
        el("label", {}, t("credentials.api_key_priority_label")),
        el("div", { class: "input-row" },
          el("input", {
            type: "password",
            id: `api-key-${meta.id}`,
            "data-id": meta.id,
            placeholder: apiKeyPlaceholder(meta.id),
            autocomplete: "off",
          }) as HTMLInputElement,
        ),
        el("div", { class: "status", id: `api-key-status-${meta.id}`, "data-id": meta.id }, t("credentials.cookie_status_placeholder")),
        el("div", { class: "row" },
          el("button", { class: "primary", id: `save-key-${meta.id}`, "data-id": meta.id, "data-action": "save-key" }, t("credentials.save_key")),
          el("button", { class: "danger", id: `del-key-${meta.id}`, "data-id": meta.id, "data-action": "del-key" }, t("credentials.delete")),
        ),
        el("div", { class: "help" },
          apiKeyHelpNode(meta.id),
          el("br"),
          el("strong", {}, "提示："),
          t("credentials.xiaomi_api_key_hint_extra"),
        ),
      ),
    );

    block.appendChild(
      el("div", { class: "field" },
        el("label", {}, t("credentials.dashboard_cookie_label")),
        el("textarea", {
          id: `cookie-${meta.id}`,
          "data-id": meta.id,
          rows: "4",
          placeholder: t("credentials.cookie_textarea_placeholder"),
        }) as HTMLTextAreaElement,
        el("div", { class: "status", id: `cookie-status-${meta.id}`, "data-id": meta.id }, t("credentials.cookie_status_placeholder")),
        el("div", { class: "row" },
          el("button", { class: "primary", id: `save-cookie-${meta.id}`, "data-id": meta.id, "data-action": "save-cookie" }, t("credentials.save_cookie")),
          el("button", { class: "danger", id: `del-cookie-${meta.id}`, "data-id": meta.id, "data-action": "del-cookie" }, t("credentials.delete")),
        ),
        el("div", { class: "help" },
          cookieHelpNode(),
        ),
      ),
    );
  }

  // ── 显示模式选择（Xiaomi 专用）──
  // 下拉选"完整 / 只套餐 / 只总额度"3 档（label 同时给出"包含什么"提示）。
  // 切完即时生效：后端落盘 + refresh 一次（poller 下一分钟才 fire）
  // 当前选中值在 init 时由 `loadXiaomiDisplayMode` 回填。
  if (meta.id === "xiaomimimo") {
    const modeSelect = el("select", {
      id: `xiaomi-display-mode-${meta.id}`,
      "data-id": meta.id,
      "data-action": "xiaomi-display-mode",
    }) as HTMLSelectElement;
    const options: Array<{ value: "all" | "plan_only" | "total_only"; key: string }> = [
      { value: "all",        key: "xiaomi_mode_all" },
      { value: "plan_only",  key: "xiaomi_mode_plan" },
      { value: "total_only", key: "xiaomi_mode_total" },
    ];
    const hintKey = (v: string) =>
      v === "all" ? "xiaomi_mode_all_hint" :
      v === "plan_only" ? "xiaomi_mode_plan_hint" :
      "xiaomi_mode_total_hint";
    for (const o of options) {
      modeSelect.appendChild(
        el("option", { value: o.value },
          `${t(`credentials.${o.key}`)} ${t(`credentials.${hintKey(o.value)}`)}`),
      );
    }
    block.appendChild(
      el("div", { class: "field" },
        el("label", { for: `xiaomi-display-mode-${meta.id}` }, t("credentials.xiaomi_display_mode_label")),
        modeSelect,
        el("div", { class: "help" },
          t("credentials.xiaomi_display_mode_help"),
        ),
      ),
    );
  }
  return block;
}

// ── 各 provider 的占位符 + 帮助文本 ────────────────────────────

export function apiKeyPlaceholder(id: string): string {
  switch (id) {
    case "minimax":    return "sk-cp-...";
    case "deepseek":   return "sk-...";
    case "xiaomimimo": return "tp-...";
    case "tavily":     return "tvly-...";
    case "zenmux":     return "sk-...";
    case "openrouter": return "sk-or-v1-...";
    case "kimi":       return "sk-...";
    case "zhipu":      return "id.secret";
    // 2026-06-16 新增（PR 2）
    case "stepfun":    return "Oasis-Token...";
    case "siliconflow":return "sk-...";
    case "claude_official": return "sessionKey=...（或纯 value）";
    default:           return "...";
  }
}

/// 返回单个 provider 的 help 节点（i18n 字符串内联 HTML，innerHTML 一次渲染）。
function apiKeyHelpNode(id: string): HTMLElement {
  const key = `help.${id}`;
  const html = t(key);
  // t() 找不到时会原样回退 key 字符串（"help.minimax"），但 JSON 里若漏
  // 配某个 id，我们走 default。dev 模式会有 console.warn。
  if (html === key) {
    return renderHelp(t("help.default"));
  }
  return renderHelp(html);
}

function cookieHelpNode(): HTMLElement {
  return renderHelp(t("help.xiaomi_cookie"));
}

// ── 统一 id-based 凭据操作（动态 panel 按钮事件委托走这里）──

export async function loadCredentialStatus(id: string) {
  const has = await hasSourceCredential(id);
  const text = has
    ? t("credentials.cookie_status_saved")
    : t("credentials.cookie_status_unset");
  const cls = `status ${has ? "ok" : ""}`;
  // 更新主面板 + 高级 tab 的 status 元素
  for (const suffix of ["", "-adv"]) {
    const status = document.getElementById(`api-key-status-${id}${suffix}`)
      ?? document.getElementById(`cookie-status-${id}${suffix}`);
    if (status) {
      status.textContent = text;
      status.className = cls;
    }
  }
}

export async function saveCredentialAction(id: string, action: "key" | "cookie", advInputId?: string) {
  // advInputId: 高级 tab 用不同 ID（如 "api-key-xiaomimimo-adv"）
  const inputId = advInputId ?? (action === "key" ? `api-key-${id}` : `cookie-${id}`);
  const input = document.getElementById(inputId) as HTMLInputElement | HTMLTextAreaElement | null;
  if (!input) return;
  const value = input.value.trim();
  if (!value) {
    flash(t("credentials.flash_paste"), true);
    return;
  }
  try {
    // 多鉴权 source 必传 field hint，否则两个输入都落 api_key。
    // 单鉴权 source（auth_kind=api_key / cookie）忽略 field 走默认也安全。
    await setSourceCredential(id, value, action === "key" ? "api_key" : "cookie");
    input.value = "";
    // H4 fix: 高级 tab 的 status 元素 id 拼了 `-adv` 后缀，主面板没有；
    // 这里必须两个后缀都更新，否则高级 tab 保存后状态元素停留在 "未保存"。
    // (与 line 508-515 的 loadCredentialStatus 同样的双后缀循环)
    const statusKey = action === "key" ? "api-key-status" : "cookie-status";
    for (const suffix of ["", "-adv"]) {
      const status = document.getElementById(`${statusKey}-${id}${suffix}`);
      if (status) {
        status.textContent = t("credentials.cookie_status_saved");
        status.className = "status ok";
      }
    }
    flash(t("credentials.flash_saved_generic", { name: t(`provider.${id as ProviderId}.name`) }));
    await refreshNow();
  } catch (e) {
    flash(t("credentials.flash_save_failed", { err: String(e) }), true);
  }
}

export async function deleteCredentialAction(id: string, action: "key" | "cookie") {
  // P0 fix: 之前传 { name: "<provider> API key" } 给 confirm_delete_key，模板是
  // "Delete {name} API key?" → 渲染 "Delete Tavily API key API key?"（"API key" 重复）。
  // 改成只传 provider 名字，让模板自带 "API key" / "Cookie" 后缀。
  const providerName = t(`provider.${id as ProviderId}.name`);
  const key = action === "key" ? "credentials.confirm_delete_key" : "credentials.confirm_delete_cookie";
  if (!confirm(t(key, { name: providerName }))) return;
  // 后端 delete_source_credential 会同时清 api_key 和 cookie，统一用一个入口
  await deleteSourceCredential(id);
  await loadCredentialStatus(id);
  flash(t("credentials.flash_deleted"));
}

export async function copyCredentialAction(id: string) {
  try {
    const value = await getSourceCredential(id);
    if (!value) {
      flash(t("credentials.flash_unset_key", { name: t(`provider.${id as ProviderId}.name`) }), true);
      return;
    }
    await navigator.clipboard.writeText(value);
    flash(t("credentials.flash_copy_ok", { name: t(`provider.${id as ProviderId}.name`) }));
  } catch (e) {
    flash(t("credentials.flash_copy_failed", { err: String(e) }), true);
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
    flash(t("credentials.xiaomi_login_only"), true);
    return;
  }
  try {
    await invoke("open_xiaomi_login_window");
    flash(t("credentials.xiaomi_login_opened"));
  } catch (e) {
    flash(t("credentials.xiaomi_login_failed", { err: String(e) }), true);
  }
}

/// 清除 Xiaomi cookie 并提示用户重新登录。
/// 用于 cookie 过期（API 返 401）时，一键清掉旧 cookie + 刷新状态。
export async function xiaomiClearCookieAction(id: string) {
  if (id !== "xiaomimimo") return;
  if (!confirm(t("credentials.confirm_clear_xiaomi"))) return;
  // 同 deleteKey：补 try/catch + flash（2026-06-20 audit）
  try {
    await deleteSourceCredential(id);
    flash(t("credentials.xiaomi_clear_done"));
    await loadCredentialStatus(id);
  } catch (e) {
    flash(t("credentials.flash_save_failed", { err: String(e) }), true);
  }
}

/// 绑一次后端登录事件 → UI 反馈。
/// 在 main.ts init() 末尾调一次就行（**只绑一次**，多次绑会重复响应）。
// M8 fix: 之前 void listen(...) 丢 unlisten。模块 scope 存 unlisten 句柄，第二次
// 调用先 unlisten 再绑，防止 listener 累积（init 重试 / dev hot-reload 场景）。
const _xiaomiListeners: UnlistenFn[] = [];
let _xiaomiListenersBound = false;
export function bindXiaomiLoginEvents() {
  if (_xiaomiListenersBound) return;
  void listen<number>("musage://xiaomi-login-success", (e) => {
    const savedLen = e.payload;
    flash(t("credentials.xiaomi_login_success", { bytes: savedLen }));
    // 立即刷新状态徽章
    void loadCredentialStatus("xiaomimimo");
  }).then((un) => _xiaomiListeners.push(un));
  void listen<string>("musage://xiaomi-login-failed", (e) => {
    flash(t("credentials.xiaomi_login_failure", { err: e.payload }), true);
  }).then((un) => _xiaomiListeners.push(un));
  _xiaomiListenersBound = true;
}

/// document-level 委托：处理动态 panel 里的 save-key / del-key / copy-key / save-cookie / del-cookie / xiaomi-login
/// 在 main.ts init() 末尾调一次就行。
// C1 fix: 用 closest('[data-action]') 而不是直接 e.target.dataset，
// 否则点 button 内部的 text node 会抛 "Cannot read properties of undefined (reading 'action')"。
// 与 src/main.ts:1010 / order.ts:128 等其它委托风格一致。
// M10 fix: 加 _credListenerBound 守卫，init 重试 / Vite HMR 不会累积 listener。
let _credListenerBound = false;
export function bindCredentialButtonsGlobal() {
  if (_credListenerBound) return;
  _credListenerBound = true;
  document.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
    if (!btn) return;
    const action = btn.dataset.action;
    const id = btn.dataset.id;
    if (!action || !id) return;
    // 高级 tab 按钮带 data-advanced="true"，用不同 ID 的 input
    const isAdv = btn.dataset.advanced === "true";
    const advInputId = isAdv
      ? (action === "save-key" || action === "del-key" ? `api-key-${id}-adv` : `cookie-${id}-adv`)
      : undefined;

    switch (action) {
      case "save-key":
        void saveCredentialAction(id, "key", advInputId);
        break;
      case "del-key":
        void deleteCredentialAction(id, "key");
        break;
      case "copy-key":
        void copyCredentialAction(id);
        break;
      case "save-cookie":
        void saveCredentialAction(id, "cookie", advInputId);
        break;
      case "del-cookie":
        void deleteCredentialAction(id, "cookie");
        break;
      case "xiaomi-login":
        void xiaomiLoginAction(id);
        break;
      case "xiaomi-clear-cookie":
        void xiaomiClearCookieAction(id);
        break;
    }
  });

  // select 用 'change' 事件：用户切了 option 就即时落盘。
  // **2026-06-20 audit**：之前 closest("[data-action='xiaomi-display-mode']")
  // 把 selector 拼字符串里 — 如果未来 data-action 值带引号会断。改用通用
  // closest("[data-action]") + 检查 dataset.action（参考 click delegate 同款）。
  document.addEventListener("change", (e) => {
    const t = (e.target as HTMLElement | null)?.closest<HTMLElement>("[data-action]");
    if (!t || t.dataset.action !== "xiaomi-display-mode") return;
    const value = (t as HTMLSelectElement).value;
    if (value !== "all" && value !== "plan_only" && value !== "total_only") return;
    void xiaomiDisplayModeAction(value);
  });
}

/// 切换 Xiaomi 显示模式：invoke 后端 → 浮窗即时更新（后端落盘 + refresh 一次）
async function xiaomiDisplayModeAction(
  mode: "all" | "plan_only" | "total_only",
): Promise<void> {
  try {
    await setXiaomiDisplayMode(mode);
    // 不需要显式调 refreshNow：后端 command 内部会 refresh_single
    const labelKey: Record<typeof mode, string> = {
      all: "xiaomi_mode_all",
      plan_only: "xiaomi_mode_plan",
      total_only: "xiaomi_mode_total",
    };
    flash(t("credentials.xiaomi_mode_changed", { label: t(`credentials.${labelKey[mode]}`) }));
  } catch (e) {
    flash(t("settings.app.switch_failed", { err: String(e) }), true);
  }
}

/// 初始化 Xiaomi 显示模式的下拉选中状态。
/// 在 settings/main.ts init() 调一次（renderProvidersSection 之后），
/// 渲染完面板后让 select 反映后端的当前值。
///
/// 后端默认 `total_only` —— 老 config.json 没有 xiaomi_display_mode 字段
/// → `unwrap_or_default()` 落到 TotalOnly → 这里 fallback 也走 total_only，
/// 保持前后端默认值一致。
export async function loadXiaomiDisplayMode(): Promise<void> {
  const container = document.querySelector<HTMLElement>("[data-id='xiaomimimo']");
  if (!container) return;  // Xiaomi panel 还没渲染
  const mode = await getXiaomiDisplayMode().catch(() => "total_only");
  const select = container.querySelector<HTMLSelectElement>(
    "select[data-action='xiaomi-display-mode']"
  );
  if (select) select.value = mode;
}
