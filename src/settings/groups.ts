// 设置面板 provider 分组
//
// PR 3 起把 13+ 个 provider 按用途分组显示 + 可折叠 + 顶部搜索。
// 复用 `el()` 工具（utils.ts），不引入新依赖。
//
// ## 分组定义
//
// - `token_plan`：Token Plan 套餐（5h/周百分比窗口）—— minimax / kimi / zhipu / qwen
// - `balance`：余额查询（钱包数字）—— deepseek / siliconflow / novita / stepfun / openrouter
// - `official`：官方/特殊（Cookie 鉴权 or 第三方）—— tavily / zenmux / claude_official
// - `xiaomi`：Xiaomi MiMo 单独一组（cookie 登录流程特殊）
// - `custom`：用户自定义 New API（id 以 `custom_` 开头）
// - `misc`：catch-all（不应该到这里，但留兜底）

import type { SourceMeta } from "./types";
import { el } from "./utils";

export type GroupKey =
  | "token_plan"
  | "balance"
  | "official"
  | "xiaomi"
  | "custom"
  | "misc";

interface GroupDef {
  title: string;
  icon: string;
  /** 决定哪些 SourceMeta 进这组。第一个匹配的组生效。 */
  predicate: (meta: SourceMeta) => boolean;
}

const GROUP_DEFINITIONS: Record<GroupKey, GroupDef> = {
  token_plan: {
    title: "Token Plan 套餐",
    icon: "📊",
    predicate: (m) => ["minimax", "kimi", "zhipu", "qwen"].includes(m.id),
  },
  balance: {
    title: "余额查询",
    icon: "💰",
    predicate: (m) =>
      ["deepseek", "siliconflow", "novita", "stepfun", "openrouter"].includes(m.id),
  },
  official: {
    title: "官方 / 特殊",
    icon: "🏛️",
    predicate: (m) => ["tavily", "zenmux", "claude_official"].includes(m.id),
  },
  xiaomi: {
    title: "Xiaomi MiMo",
    icon: "🍚",
    predicate: (m) => m.id === "xiaomimimo",
  },
  custom: {
    title: "用户自定义 New API",
    icon: "🧩",
    predicate: (m) => m.id.startsWith("custom_"),
  },
  misc: {
    title: "其他",
    icon: "🔧",
    predicate: () => true, // catch-all
  },
};

const GROUP_ORDER: GroupKey[] = [
  "token_plan",
  "balance",
  "official",
  "xiaomi",
  "custom",
  "misc",
];

/** 把 SourceMeta[] 按 GROUP_DEFINITIONS 分配到各组。空组会被剔除。 */
export function groupSources(
  sources: SourceMeta[],
): Map<GroupKey, SourceMeta[]> {
  const buckets: Record<GroupKey, SourceMeta[]> = {
    token_plan: [],
    balance: [],
    official: [],
    xiaomi: [],
    custom: [],
    misc: [],
  };
  for (const meta of sources) {
    const key = GROUP_ORDER.find((k) => GROUP_DEFINITIONS[k].predicate(meta)) ?? "misc";
    buckets[key].push(meta);
  }
  // 按 GROUP_ORDER 输出 + 跳过空组
  const result = new Map<GroupKey, SourceMeta[]>();
  for (const key of GROUP_ORDER) {
    if (buckets[key].length > 0) result.set(key, buckets[key]);
  }
  return result;
}

/** 渲染单个组（原生 `<details>` + `<summary>`，无 CSS 依赖）。 */
export function renderGroup(
  key: GroupKey,
  metas: SourceMeta[],
  createPanel: (meta: SourceMeta) => HTMLElement,
): HTMLElement {
  const def = GROUP_DEFINITIONS[key];
  const details = el("details", {
    class: "provider-group",
    "data-group": key,
    open: "",
  });
  details.appendChild(
    el(
      "summary",
      {},
      `${def.icon} ${def.title} (${metas.length})`,
    ),
  );
  for (const meta of metas) details.appendChild(createPanel(meta));
  return details;
}
