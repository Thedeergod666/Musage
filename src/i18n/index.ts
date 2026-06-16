// 国际化 helper —— 自写 ~80 行，零依赖，零运行时开销。
//
// 关键约定（详见 memory/musage-i18n-conventions.md）：
// - key 用语义命名（"button.save" 不是 "Save"）
// - 永远用 {name} 占位符，绝不字符串拼接
// - plural 优先 key 后缀（"footer.count.one" / "footer.count.other"）
// - locale 文件在 src/i18n/{en,zh-CN}.json，跟 tauri 端 src-tauri/locales/*.json 同名
// - 切语言走 setLocale() → 持久化（后端 set_locale 命令）+ 触发 listeners
//
// P0 阶段：loadLocale / setLocale 已有最小可用实现，t() / plural 支持完整。
// en.json / zh-CN.json 是空结构（仅 metadata），任何 t() 调用会回退 key 字符串。
// P1 阶段填实际翻译。

export type Locale = "en" | "zh-CN";
const SUPPORTED: Locale[] = ["en", "zh-CN"];

let current: Locale = "zh-CN";
const dicts: Record<Locale, Record<string, any>> = {
  "en": {} as any,
  "zh-CN": {} as any,
};
let loaded = false;
const listeners = new Set<(l: Locale) => void>();

// dev-only missing-key 警告。生产 build 关闭（Vite 替换 import.meta.env.DEV = false）。
const dev = (typeof import.meta !== "undefined" && (import.meta as any).env?.DEV) === true;

/**
 * 取翻译字符串。找不到 key 时回退到 en dict，再找不到回退到 key 本身。
 * `params` 里的 `{name}` 占位符被替换。
 *
 * 例子：
 *   t("button.save")                    → "保存" / "Save"
 *   t("footer.count", { count: 3 })     → "3 个 provider · ..." / "3 providers · ..."
 *   t("logs.entries", { count: 0 })     → plural-aware（先试 .one / .other）
 */
export const t = (key: string, params?: Record<string, string | number>): string => {
  let effectiveKey = key;
  // plural：优先 .one / .other（Intl.PluralRules 标准）
  if (params && typeof params.count === "number") {
    const rule = new Intl.PluralRules(current).select(params.count);
    const pluralKey = `${key}.${rule}`;
    if (lookup(pluralKey) != null) effectiveKey = pluralKey;
  }
  let s = lookup(effectiveKey);
  if (s == null) {
    // 跨 locale 回退：zh-CN 缺 key 时回退 en，en 还缺才打 key
    const en = dicts.en?.[key] ?? dicts.en?.[effectiveKey];
    if (en != null) s = en;
    else {
      if (dev) console.warn(`[i18n] missing key '${key}' in locale '${current}'`);
      return key;
    }
  }
  if (typeof s !== "string") {
    if (dev) console.warn(`[i18n] key '${key}' is not a string in locale '${current}'`);
    return key;
  }
  if (!params) return s;
  return s.replace(/\{(\w+)\}/g, (_, k) => {
    const v = params[k];
    return v == null ? `{${k}}` : String(v);
  });
};

function lookup(key: string): any {
  const parts = key.split(".");
  let cur: any = dicts[current];
  for (const p of parts) {
    if (cur == null || typeof cur !== "object") return undefined;
    cur = cur[p];
  }
  return cur;
}

/**
 * 切语言。持久化到后端 config（让下次启动沿用），触发所有 onLocaleChange 监听。
 * 调用方通常在 UI 切换 radio 时调一次。
 */
export const setLocale = async (l: Locale): Promise<void> => {
  if (!SUPPORTED.includes(l)) {
    console.warn(`[i18n] unsupported locale: ${l}`);
    return;
  }
  await loadLocale(l);
  current = l;
  document.documentElement.lang = l;
  // 通知后端 + 同步到 config.json
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("set_app_locale", { locale: l });
  } catch (e) {
    // set_app_locale 命令在 P0 之前不存在；P0 之后会成功，错误可吞
    if (dev) console.debug("[i18n] set_app_locale invoke failed (expected before backend ready)", e);
  }
  // 通知前端 listeners
  listeners.forEach((fn) => {
    try { fn(l); } catch (e) { console.error("[i18n] listener error", e); }
  });
};

/**
 * 加载 locale 的 JSON dict。Vite 支持 JSON import 同步，但为支持动态切语言，
 * 走 dynamic import 让第一次 setLocale() 时才下载 / 解析。
 *
 * 重复调同 locale 直接返回（不重复 fetch）。
 */
export const loadLocale = async (l: Locale): Promise<void> => {
  if (dicts[l] && Object.keys(dicts[l]).length > 0) return;
  try {
    const mod = await import(`./${l}.json`);
    dicts[l] = mod.default ?? mod;
  } catch (e) {
    if (dev) console.warn(`[i18n] failed to load ${l}.json`, e);
    dicts[l] = {};
  }
};

/**
 * 启动时初始化：根据后端 cfg.locale 设置 current + 加载 dict + 改 lang attr。
 * 后端不知道前端已 ready，所以这里要主动调 get_locale。
 */
export const initLocale = async (): Promise<Locale> => {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const saved = await invoke<string>("get_app_locale");
    if (saved === "en" || saved === "zh-CN") {
      current = saved;
    } else {
      // cfg 缺这字段（老用户）→ 后端默认 zh-CN，但前端仍按 navigator.language
      // 兜底一次：非中文 OS 切到 en
      const nav = (typeof navigator !== "undefined" && navigator.language) || "zh-CN";
      current = nav.startsWith("zh") ? "zh-CN" : "en";
    }
  } catch (e) {
    if (dev) console.debug("[i18n] get_app_locale invoke failed (probably first run)", e);
    const nav = (typeof navigator !== "undefined" && navigator.language) || "zh-CN";
    current = nav.startsWith("zh") ? "zh-CN" : "en";
  }
  await loadLocale(current);
  document.documentElement.lang = current;
  loaded = true;
  return current;
};

/** 取当前 locale（用于 UI 显示当前语言 / 写日志时标 locale）。 */
export const getLocale = (): Locale => current;

/** 监听 locale 变化（setLocale 触发后回调）。返回 unsub 函数。 */
export const onLocaleChange = (fn: (l: Locale) => void): (() => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};

/** 内部状态：是否已 init（dev 工具 / 调试用）。 */
export const isInitialized = (): boolean => loaded;
