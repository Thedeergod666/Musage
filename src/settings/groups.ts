// 设置面板 provider 分组
//
// PR 3 起把 11+ 个 provider 按用途分组显示 + 可折叠 + 顶部搜索。
// 复用 `el()` 工具（utils.ts），不引入新依赖。
//
// ## 分组定义
//
// - `token_plan`：Token Plan 套餐（5h/周百分比窗口）—— minimax / kimi / zhipu
// - `balance`：余额查询（钱包数字）—— deepseek / siliconflow / stepfun / openrouter
// - `official`：官方/特殊（Cookie 鉴权 or 第三方）—— tavily / zenmux / claude_official
// - `xiaomi`：Xiaomi MiMo 单独一组（cookie 登录流程特殊）
// - `custom`：用户自定义 New API（id 以 `custom_` 开头）
// - `misc`：catch-all（不应该到这里，但留兜底）

import type { SourceMeta } from "./types";
import { t, onLocaleChange } from "../i18n";

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

/// P0 fix: 之前 `const GROUP_DEFINITIONS = { ... }` 在模块顶层求值，import 时 t() 同步调
/// 用但 dicts 还没 load（initLocale 是 async fire-and-forget），所以 title 永远 = 原始
/// key 字符串。改成像 main.ts::buildProviderMeta() 那样用函数延迟求值。
export function buildGroupDefinitions(): Record<GroupKey, GroupDef> {
  return {
    token_plan: {
      title: t("groups.token_plan_title"),
      icon: "wallet",
      predicate: (m) => ["minimax", "kimi", "zhipu"].includes(m.id),
    },
    balance: {
      title: t("groups.balance_title"),
      icon: "piggy-bank",
      predicate: (m) =>
        ["deepseek", "siliconflow", "stepfun", "openrouter"].includes(m.id),
    },
    official: {
      title: t("groups.official_title"),
      icon: "building-2",
      predicate: (m) => ["tavily", "zenmux", "claude_official"].includes(m.id),
    },
    xiaomi: {
      title: t("groups.xiaomi_title"),
      icon: "utensils",
      predicate: (m) => m.id === "xiaomimimo",
    },
    custom: {
      title: t("groups.custom_title"),
      icon: "puzzle",
      predicate: (m) => m.id.startsWith("custom_"),
    },
    misc: {
      title: t("groups.misc_title"),
      icon: "package",
      predicate: () => true, // catch-all
    },
  };
}

let _groupDefinitions: Record<GroupKey, GroupDef> = {} as Record<GroupKey, GroupDef>;

// locale 切换时重建（settings panel 调用方需要监听这个然后重渲整组列表）
onLocaleChange(() => {
  _groupDefinitions = buildGroupDefinitions();
});

const GROUP_ORDER: GroupKey[] = [
  "token_plan",
  "balance",
  "official",
  "xiaomi",
  "custom",
  "misc",
];

/** 暴露给 providers.ts 用来按固定顺序遍历 + 决定 inline 分隔线插入点。 */
export function getGroupOrder(): readonly GroupKey[] {
  return GROUP_ORDER;
}

/// 第一次被外部调时尝试填一次（settings panel 启动时序不一定在 initLocale 之后）
function ensureGroupsReady() {
  if (Object.keys(_groupDefinitions).length === 0) {
    _groupDefinitions = buildGroupDefinitions();
  }
}

/** 把 SourceMeta[] 按 _groupDefinitions 分配到各组。空组会被剔除。 */
export function groupSources(
  sources: SourceMeta[],
): Map<GroupKey, SourceMeta[]> {
  ensureGroupsReady();
  const buckets: Record<GroupKey, SourceMeta[]> = {
    token_plan: [],
    balance: [],
    official: [],
    xiaomi: [],
    custom: [],
    misc: [],
  };
  for (const meta of sources) {
    const key = groupKeyFor(meta);
    buckets[key].push(meta);
  }
  // 按 GROUP_ORDER 输出 + 跳过空组
  const result = new Map<GroupKey, SourceMeta[]>();
  for (const key of GROUP_ORDER) {
    if (buckets[key].length > 0) result.set(key, buckets[key]);
  }
  return result;
}

/** 单个 meta 命中哪个组（按 GROUP_ORDER 顺序匹配第一个）。fallback = "misc"。 */
export function groupKeyFor(meta: SourceMeta): GroupKey {
  ensureGroupsReady();
  return (
    GROUP_ORDER.find((k) => _groupDefinitions[k].predicate(meta)) ?? "misc"
  );
}

/** 暴露 group definition 给 providers.ts 读 title / icon。 */
export function getGroupDef(key: GroupKey): GroupDef {
  ensureGroupsReady();
  return _groupDefinitions[key];
}

