// 复用浮窗的 provider logo 资产 —— 跟 [src/main.ts:15-30] 保持一致
//
// Vite 会在 build 时把 import 解析成打包后的 URL（asset pipeline），
// dev 模式直接给原文件路径。dev 实时刷新，build 后走缓存 hash。
//
// 加新 provider 同步：main.ts 加 import + 在 PROVIDER_META 加一行即可。

import minimaxLogo from "../assets/minimax-logo.png";
import deepseekLogo from "../assets/deepseek-icon.png";
import xiaomimimoLogo from "../assets/xiaomimimo-logo.png";
import tavilyLogo from "../assets/tavily-logo.svg";
import zenmuxLogo from "../assets/zenmux-logo.svg";
import openrouterLogo from "../assets/openrouter-logo.png";
import kimiLogo from "../assets/kimi-logo.svg";
import zhipuLogo from "../assets/zhipu-logo.svg";
import zhipuEnLogo from "../assets/zhipu-en-logo.svg";

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

/// id → 浮窗同款 logo + 名字 + 标识色（跟 tokens.css 的 --id-* 对齐）
///
/// 加新 provider 时如果暂时没 logo 文件，把 `logo` 留空字符串，
/// getLogoSrc() 会自动用首字母 + accent 色生成 data: URL fallback。
export const PROVIDER_META: Record<string, ProviderMeta> = {
  minimax: { name: "MiniMax", logo: minimaxLogo, accent: "var(--id-minimax)" },
  deepseek: { name: "DeepSeek", logo: deepseekLogo, accent: "var(--id-deepseek)" },
  xiaomimimo: { name: "Xiaomi MiMo", logo: xiaomimimoLogo, accent: "var(--id-xiaomimimo)" },
  tavily: { name: "Tavily", logo: tavilyLogo, accent: "var(--id-tavily)" },
  zenmux: { name: "ZenMux", logo: zenmuxLogo, accent: "var(--id-zenmux)" },
  openrouter: { name: "OpenRouter", logo: openrouterLogo, accent: "var(--id-openrouter)" },
  kimi: { name: "Kimi", logo: kimiLogo, accent: "var(--id-kimi)" },
  zhipu: { name: "智谱 GLM", logo: zhipuLogo, accent: "var(--id-zhipu)" },
  zhipu_en: { name: "Z.ai", logo: zhipuEnLogo, accent: "var(--id-zhipu)" },
  // 2026-06-16 新增（PR 2）—— 暂没 logo 文件，fallback 用首字母 + accent 色
  stepfun: { name: "StepFun", logo: "", accent: "#6366f1" },
  siliconflow: { name: "SiliconFlow", logo: "", accent: "#ff6b35" },
  novita: { name: "Novita AI", logo: "", accent: "#9333ea" },
  qwen: { name: "Qwen", logo: "", accent: "#615ced" },
  claude_official: { name: "Claude 官方", logo: "", accent: "#d97706" },
};

export function getProviderMeta(id: string): ProviderMeta | undefined {
  return PROVIDER_META[id];
}

/// 解析 meta：有 logo 直接用，没 logo 走首字母 fallback。
/// 返回 `{ logo, name }`，调用方拿这两个值设到 `<img>` 或别处。
export function getProviderDisplay(id: string, fallbackName?: string): { logo: string; name: string } {
  const meta = PROVIDER_META[id];
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
