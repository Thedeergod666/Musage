// 国际化 helper —— 自写 ~80 行，零依赖，零运行时开销。
//
// 关键约定（详见 memory/musage-i18n-conventions.md）：
// - key 用语义命名（"button.save" 不是 "Save"）
// - 永远用 {name} 占位符，绝不字符串拼接
// - plural 优先 key 后缀（"footer.count.one" / "footer.count.other"）
// - locale 文件在 src/i18n/{en,zh-CN}.json，跟 tauri 端 src-tauri/locales/*.json 同名
// - 切语言走 setLocale() → 持久化（后端 set_locale 命令）+ 触发 listeners
//
// **v0.2.2 fix (2026-07-10)**：loadLocale 之前用 `await import(\`./${l}.json\`)`
// 模板字符串动态 import。Vite/Rollup 只能静态分析 literal 路径（`./en.json`），
// 模板字符串里的 `${l}` 让 build 阶段没有 chunk 生成目标，runtime 时
// import() 静默失败 → dicts 永远是 `{}` → t() 全部返 raw key。
// 修法：static import 两个 locale，在 loadLocale 里用 dict 选。

export type Locale = "en" | "zh-CN";
const SUPPORTED: Locale[] = ["en", "zh-CN"];

let current: Locale = "zh-CN";
// **v0.2.2 fix**：static import 两个 locale。Vite 在 build 时能把这两份 JSON
// 内联进 main.js 同一 chunk（也可以拆 chunk，取决于 assets 大小），但**一定**
// 能拿到 —— 不再依赖 dynamic import 静态分析。
// JSON 内容由 Vite `import` 时直接 attach 到 module namespace，dicts[l] = mod.default。
import enDict from "./en.json";
import zhCNDict from "./zh-CN.json";

const dicts: Record<Locale, Record<string, any>> = {
  "en": enDict as Record<string, any>,
  "zh-CN": zhCNDict as Record<string, any>,
};
let loaded = false;
const listeners = new Set<(l: Locale) => void>();

// dev-only missing-key 警告。生产 build 关闭（Vite 替换 import.meta.env.DEV = false）。
const dev = (typeof import.meta !== "undefined" && (import.meta as any).env?.DEV) === true;

// H18 fix (2026-07-03 audit): 之前 t() 找不到 key 时直接 return key,
// UI 上暴露 raw key 字符串("settings.floating.xxx")。dev 模式只有
// console.warn 单条日志, 开发者不主动看 console 就漏掉。
// 改: 收集所有 missing key 到全局 Set, 在 dev 模式下提供 dumpMissingKeys()
// 让开发者一次性看到所有缺失 key 列表(可手动 console 调用, 也可在
// settings 面板 dev menu 调)。生产模式不收集(零开销)。
const missingKeys: Set<string> = new Set();
export function dumpMissingKeys(): string[] {
  return Array.from(missingKeys).sort();
}
export function _resetMissingKeysForTest(): void {
  missingKeys.clear();
}

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
    // 必须走点号路径查找（lookupInDict），不能用 dicts.en[key] ——
    // key 是 "provider.minimax.name" 这种嵌套路径，直接属性访问永远 undefined。
    const enFallback = lookupInDict(dicts.en, key) ?? lookupInDict(dicts.en, effectiveKey);
    if (enFallback != null) s = enFallback;
    else {
      // H18 fix: 收集 missing key (dev 模式才填, 生产模式 Set 始终空 = 零开销)
      if (dev) {
        missingKeys.add(key);
        console.warn(`[i18n] missing key '${key}' in locale '${current}'`);
      }
      return key;
    }
  }
  if (typeof s !== "string") {
    if (dev) {
      missingKeys.add(`${key}#non-string`);
      console.warn(`[i18n] key '${key}' is not a string in locale '${current}'`);
    }
    return key;
  }
  if (!params) return s;
  // 2026-06-20 audit：之前 /\\{(\w+)\\}/g 只匹配 [A-Za-z0-9_]+，{user-id} /
  // {err.code} 这种 placeholder 名 silent fallback。扩展为 [\w.-]+ 兼容。
  return s.replace(/\{([\w.-]+)\}/g, (_, k) => {
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

/// 在指定的 dict 上走点号路径查找（与 lookup 逻辑相同但不绑定 current locale）。
/// 供跨 locale 回退使用。
function lookupInDict(dict: Record<string, any>, key: string): any {
  const parts = key.split(".");
  let cur: any = dict;
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
 * 加载 locale 的 JSON dict。
 *
 * **v0.2.2 fix**：之前走 `await import(\`./${l}.json\`)` 动态 import 失败
 * （Vite 不能静态分析模板字符串 → build 没生成 chunk → runtime 静默失败
 * → dicts 永远空 → t() 返 raw key）。现在两个 locale 在 module 顶部 static
 * import，dicts 在 module load 时就填好。loadLocale 退化成"确认 dict 已就位"
 * 的轻量 check，保留 export 以免破坏外部调用方。
 *
 * 重复调同 locale 直接返回（不重复 fetch）。
 */
export const loadLocale = async (l: Locale): Promise<void> => {
  if (!dicts[l] || Object.keys(dicts[l]).length === 0) {
    // 防御：理论上 static import 不会让 dict 空，进了这里就是 dev-time 误用
    if (dev) console.warn(`[i18n] loadLocale(${l}) called but dict is empty`);
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
