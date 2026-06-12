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

export interface ProviderMeta {
  name: string;
  logo: string;
  /// 用于 sidebar 左侧 4×4 dot（每个 provider 一种颜色，区别 provider 边界）
  accent: string;
}

/// id → 浮窗同款 logo + 名字 + 标识色（跟 tokens.css 的 --id-* 对齐）
export const PROVIDER_META: Record<string, ProviderMeta> = {
  minimax: { name: "MiniMax", logo: minimaxLogo, accent: "var(--id-minimax)" },
  deepseek: { name: "DeepSeek", logo: deepseekLogo, accent: "var(--id-deepseek)" },
  xiaomimimo: { name: "Xiaomi MiMo", logo: xiaomimimoLogo, accent: "var(--id-xiaomimimo)" },
  tavily: { name: "Tavily", logo: tavilyLogo, accent: "var(--id-tavily)" },
  zenmux: { name: "ZenMux", logo: zenmuxLogo, accent: "var(--id-zenmux)" },
  openrouter: { name: "OpenRouter", logo: openrouterLogo, accent: "var(--id-openrouter)" },
};

export function getProviderMeta(id: string): ProviderMeta | undefined {
  return PROVIDER_META[id];
}
