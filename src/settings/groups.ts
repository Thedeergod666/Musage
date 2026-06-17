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
import { t } from "../i18n";

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
    title: t("groups.token_plan_title"),
    icon: "📊",
    predicate: (m) => ["minimax", "kimi", "zhipu", "qwen"].includes(m.id),
  },
  balance: {
    title: t("groups.balance_title"),
    icon: "💰",
    predicate: (m) =>
      ["deepseek", "siliconflow", "novita", "stepfun", "openrouter"].includes(m.id),
  },
  official: {
    title: t("groups.official_title"),
    icon: "🏛️",
    predicate: (m) => ["tavily", "zenmux", "claude_official"].includes(m.id),
  },
  xiaomi: {
    title: t("groups.xiaomi_title"),
    icon: "🍚",
    predicate: (m) => m.id === "xiaomimimo",
  },
  custom: {
    title: t("groups.custom_title"),
    icon: "🧩",
    predicate: (m) => m.id.startsWith("custom_"),
  },
  misc: {
    title: t("groups.misc_title"),
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

/** PR 3 (UX 调整)：把分组拆成「顶部 tabs」+「下面特殊组」两段。
 *
 * - 顶部 tabs (tab strip, sticky 置顶)：token_plan / balance / official
 *   —— 高频类目，tab 切换 + 置顶方便随时切换；默认显示 token_plan
 * - 下面特殊组（collapsible <details>）：xiaomi / custom / misc
 *   —— 这些是低频或动态长内容的
 */
export function splitGroupsForLayout(groups: Map<GroupKey, SourceMeta[]>): {
  tabs: Array<[GroupKey, SourceMeta[]]>;
  special: Array<[GroupKey, SourceMeta[]]>;
} {
  const tabKeys: GroupKey[] = ["token_plan", "balance", "official"];
  const all = Array.from(groups.entries());
  const tabs = all.filter(([k]) => tabKeys.includes(k));
  const special = all.filter(([k]) => !tabKeys.includes(k));
  return { tabs, special };
}

/** 暴露 group definition 给 providers.ts 读 title / icon。 */
export function getGroupDef(key: GroupKey): GroupDef {
  return GROUP_DEFINITIONS[key];
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
