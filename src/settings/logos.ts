// 复用浮窗的 provider logo 资产 —— 跟 [src/main.ts] 的 PROVIDER_META 保持一致
//
// Vite 会在 build 时把 import 解析成打包后的 URL（asset pipeline），
// dev 模式直接给原文件路径。dev 实时刷新，build 后走缓存 hash。
//
// 加新 provider 同步：main.ts 加 import + 在 PROVIDER_META 加一行即可。
//
// P1 frontend 阶段：name 走 t(\`provider.${id}.name\`)（i18n），
// 跟 src/main.ts:37 PROVIDER_META 共享 src/i18n/{en,zh-CN}.json 同一组 key。
// settings 侧不再 hardcode "智谱 GLM" / "Z.ai" 等显示名。

import { t, onLocaleChange } from "../i18n";
import minimaxLogo from "../assets/minimax-logo.png";
import deepseekLogo from "../assets/deepseek-icon.png";
import xiaomimimoLogo from "../assets/xiaomimimo-logo.png";
import tavilyLogo from "../assets/tavily-logo.svg";
import zenmuxLogo from "../assets/zenmux-logo.svg";
import openrouterLogo from "../assets/openrouter-logo.png";
import kimiLogo from "../assets/kimi-logo.svg";
import zhipuLogo from "../assets/zhipu-logo.svg";
import zhipuEnLogo from "../assets/zhipu-en-logo.svg";
import stepfunLogo from "../assets/stepfun-logo.svg";
import siliconflowLogo from "../assets/siliconflow-logo.svg";
import claudeLogo from "../assets/claude-logo.svg";

export interface ProviderMeta {
  name: string;
  logo: string;
  /// 用于 sidebar 左侧 4×4 dot（每个 provider 一种颜色，区别 provider 边界）
  accent: string;
}

/// 没 logo 文件时用首字母 + accent 色生成 data: URL SVG。
/// 跟 [src/main.ts] 的同名函数同款，CSS variables 在 SVG data URL 里
/// 不解析（base64 内联后浏览器只看字符串），所以 settings 这边把
/// var(--id-*) 解析成具体 hex 值传进去。
function fallbackLogo(name: string, accent: string): string {
  const ch = name.trim().charAt(0).toUpperCase() || "?";
  const safeAccent = accent.startsWith("var(") ? "#888" : accent;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 56 56">
    <rect width="56" height="56" rx="12" fill="${safeAccent}"/>
    <text x="28" y="38" text-anchor="middle" font-family="-apple-system,BlinkMacSystemFont,'PingFang SC','Microsoft YaHei',sans-serif" font-size="30" font-weight="700" fill="#fff">${escapeXml(ch)}</text>
  </svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

function escapeXml(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]!),
  );
}

/// P0 fix: 之前 `export const PROVIDER_META = { ... }` 在模块顶层求值，import 时
/// `t()` 被同步调用但 dicts 还没 load（initLocale 是 async fire-and-forget），
/// 所以每个 name 都是原始 key 字符串 ("provider.minimax.name" 等)。
/// 改成像 main.ts::buildProviderMeta() 那样用函数延迟求值，locale 切换时重建。
export function buildProviderMeta(): Record<string, ProviderMeta> {
  return {
    minimax: { name: t("provider.minimax.name"), logo: minimaxLogo, accent: "var(--id-minimax)" },
    deepseek: { name: t("provider.deepseek.name"), logo: deepseekLogo, accent: "var(--id-deepseek)" },
    xiaomimimo: { name: t("provider.xiaomimimo.name"), logo: xiaomimimoLogo, accent: "var(--id-xiaomimimo)" },
    tavily: { name: t("provider.tavily.name"), logo: tavilyLogo, accent: "var(--id-tavily)" },
    zenmux: { name: t("provider.zenmux.name"), logo: zenmuxLogo, accent: "var(--id-zenmux)" },
    openrouter: { name: t("provider.openrouter.name"), logo: openrouterLogo, accent: "var(--id-openrouter)" },
    kimi: { name: t("provider.kimi.name"), logo: kimiLogo, accent: "var(--id-kimi)" },
    zhipu: { name: t("provider.zhipu_cn.name"), logo: zhipuLogo, accent: "var(--id-zhipu)" },
    zhipu_en: { name: t("provider.zhipu_en.name"), logo: zhipuEnLogo, accent: "var(--id-zhipu)" },
    stepfun: { name: t("provider.stepfun.name"), logo: stepfunLogo, accent: "#6366f1" },
    siliconflow: { name: t("provider.siliconflow.name"), logo: siliconflowLogo, accent: "#ff6b35" },
    claude_official: { name: t("provider.claude_official.name"), logo: claudeLogo, accent: "#d97706" },
  };
}

/// 当前 locale 的 provider 元数据缓存。初始为 {}，initLocale 后 + locale 变化时重建。
let _providerMeta: Record<string, ProviderMeta> = {};

/// 第一次被外部调时尝试填一次（settings panel 启动时序不一定在 initLocale 之后）
export function ensureProviderMetaReady() {
  if (Object.keys(_providerMeta).length === 0) {
    _providerMeta = buildProviderMeta();
  }
}

// 每次 locale 切换重建（settings panel 调用方需要监听这个然后重渲）
onLocaleChange(() => {
  _providerMeta = buildProviderMeta();
});

export function getProviderMeta(id: string): ProviderMeta | undefined {
  ensureProviderMetaReady();
  // 直接命中：deepseek / minimax 等 base id
  if (_providerMeta[id]) return _providerMeta[id];
  // extra instance (deepseek#2 → deepseek)：去掉 #N 后缀后查 base id
  const hashPos = id.indexOf('#');
  if (hashPos > 0) {
    const baseId = id.slice(0, hashPos);
    return _providerMeta[baseId];
  }
  return undefined;
}

/// 解析 meta：有 logo 直接用，没 logo 走首字母 fallback。
/// 返回 `{ logo, name }`，调用方拿这两个值设到 `<img>` 或别处。
export function getProviderDisplay(id: string, fallbackName?: string): { logo: string; name: string } {
  ensureProviderMetaReady();
  const meta = _providerMeta[id];
  if (meta) {
    return {
      logo: meta.logo || fallbackLogo(meta.name, meta.accent),
      name: meta.name,
    };
  }
  // 未知 id（比如后端新增了 source 但 settings 还没更新）→ 用 fallbackName 首字母
  const name = fallbackName ?? id;
  return { logo: fallbackLogo(name, "#888"), name };
}
