# Musage 全量代码审查 — 前端报告

**范围**: 23 个 TS 文件 — `src/main.ts` (1679 行) 浮窗主逻辑 + `src/i18n/index.ts` i18n helper + `src/settings.ts` 4 行 re-export + `src/settings/` 下 19 个子模块 (`main.ts` / `providers.ts` (557) / `credentials.ts` (1190) / `order.ts` (1065) / `advanced.ts` (380) / `floating.ts` (350) / `extra-instance-form.ts` (632) / `source-extras.ts` (273) / `modal.ts` (119) / `api.ts` (301) / `app.ts` (106) / `groups.ts` (138) / `logos.ts` (121) / `logs.ts` (150) / `about.ts` (48) / `config.ts` (27) / `region-wizard.ts` (162) / `test.ts` (84) / `types.ts` (233) / `utils.ts` (197))。无单独 `src/types.ts`，共享类型在 `src/settings/types.ts`。

**未审**: Rust 端 (`src-tauri/`)、HTML 模板 (`src/*.html`)、CSS。

**已查上下文**: pnpm tsc --noEmit 0 errors / pnpm test 29/29 passed (v0.2.5)；v0.2.1 i18n dynamic import 已修；v0.2.0 advanced.ts M5 fix 把 `innerHTML = t(...)` 改成 `textContent = t(...)`；2026-07-30 B-M2 fix 把 `setLocale` 后端 invoke 失败时的回滚 + listener 通知补齐。

**调用前置**: AGENTS 已知 23 文件清单 + 11 项必查点 + 上文 5/6 priority areas。

整体判断:**真 bug 不多**(约 4 个可触发、其余是 dev/HMR 累积 anti-pattern)。前端基本盘是 OK 的 —— XSS 风险严格 escapeHtml / escapeXml 包,事件监听器 dedup 主入口用 `_xxxBound` flags,async 错误兜底齐全。多数发现是**config 表序列化缺失的兼容路径** + **i18n 缺失 key raw fallback 视觉错乱** + **HMR-only 累积 listener**(生产构建无害)。下面按 P0/P1/P2/P3 列出。

---

## P1 (高)

### D7-001 — BATCH_PREFIX_RULES 通用 `sk-` 兜底无差别归 minimax → Kimi / ZenMux / SiliconFlow 用户批量粘贴 key 时误存为 minimax

**置信度**:高(已确认) **文件**:`src/settings/credentials.ts:1113-1124`(BATCH_PREFIX_RULES 数组) + `:1146-1175`(`parseBatchLine` 循环匹配) **触发条件**:Kimi / ZenMux / SiliconFlow 用户(任意 1 个)走「批量粘贴」折叠 textarea 把自己的 key 粘进去,无 `provider=` 显式标注。

**根因**:BATCH_PREFIX_RULES 优先级排成长→短 prefix,通用 `sk-` 兜底把 Kimi (`sk-...`)/ ZenMux (`sk-...`)/ SiliconFlow (`sk-...`) 都静默归到 `minimax`。注释明确说"通用 sk- 前缀: minimax/zenmux/openrouter/kimi/siliconflow 都有 用 host 识别;无 host 的按 priority" —— 但实现**没真做 host 识别**,只是简单首个不匹配的前缀兜 minimax。

**实际匹配路径**(实测一行 `sk-kimi-abc123def456`):
1. `sk-or-v1-` 不命中(sk- 头不是 sk-or-v1-)
2. `sk-cp-` 不命中
3. `tvly-` 不命中
4. **通用 `sk-` 命中 → `{id: "minimax", field: "api_key", value: "kimi-abc123def456"}`** ❌
5. `batchPasteKeys` 调 `setSourceCredential("minimax", "kimi-abc123def456", "api_key")` → 误存为 minimax 的 key

**影响**:
- 用户密 MiniMax card 刷新成功,看起来 key 有效,但实际放的是 Kimi key → 后端调 `minimax` endpoint 时 MiniMax 返 401 → 用户看到 minimax 卡报 `auth_failed`,完全摸不着头脑(Kimi key 在自己 MiniMax 账号下当然无效)
- Kimi 这边倒是显示 `unconfigured_key` —— 因为扣到了 minimax 的 key slot 没碰 Kimi slot。两张卡同时错位,排查极耗时间
- 日志面板 / 下次刷新提示都不可信
- OpenRouter (有 `sk-or-v1-` 前缀) 平安, MiniMax Coding Plan (`sk-cp-`) 平安;**任何用通用 `sk-` 起步的 provider 都中招**

**证据/调用链**:
```typescript
// credentials.ts:1113-1124
const BATCH_PREFIX_RULES = [
  { prefix: "sk-or-v1-", id: "openrouter", field: "api_key" },
  { prefix: "sk-cp-",     id: "minimax",    field: "api_key" },
  { prefix: "tvly-",      id: "tavily",     field: "api_key" },
  { prefix: "sk-",        id: "minimax",    field: "api_key" },  // ← 兜底归 minimax
  { prefix: "tp-",        id: "xiaomimimo", field: "cookie" },
];

// credentials.ts:1146-1176 (parseBatchLine 循环)
for (const rule of BATCH_PREFIX_RULES) {
  if (value.startsWith(rule.prefix)) {
    return [{ id: rule.id, field: rule.field, value }];  // ← 命中即返,无主机探测
  }
}
```

**修复建议**:
**3 选 1**(任挑):
1. **删通用 `sk-` 兜底规则**,无 prefix 标识的行 → `counts.unrecognized++`。强制用户走 `minimax=sk-xxx` / `kimi=sk-xxx` 显式标注路径 —— 最稳,不引入误识别
2. **保留通用 `sk-` 兜底但返 `null`**(语义改成"无法识别"),flash 提示用户加 `provider=` 前缀;与 unrecognized > 0 走同一提示路径,不静默乱存
3. **加 host 探测**(异步):前缀命中后用 `setSourceCredential` 写,再调 `refresh_single` 看 `success` 字段;不成功回滚 + flash "无法识别 provider, 请加 provider=xxx 前缀" —— 实现成本高但 UX 最好

**建议选 1**(改 1 行就行,删 `sk-` 那条规则),跟 OpenRouter/MiniMax 显式 `sk-` 前缀不存在的现状一致。Kimi/ZenMux/SiliconFlow 用户 30 秒加个前缀即可。

**待实测**:构造 `parseBatchLine("sk-kimi-xyz789")` 单测,断言不命中 → 返 `[]`;构造 `parseBatchLine("minimax=sk-abc")` 仍走显式 providerHint 路径;构造 `parseBatchLine("openrouter=sk-or-v1-xyz")` 不被通用 sk- 抢匹配。

---

### D7-002 — `settings/main.ts` init 流程里 `initI18n` 双调用,onLocaleChange listener 累积导致切 locale 时 applyDataI18n + renderRegionSection 各跑 N 次

**置信度**:高(已确认) **文件**:`src/settings/main.ts:46-61` (initI18n 定义) + `:61` (顶层 `void initI18n()`) + `:126-131` (init 里 `await initI18n()`) **触发条件**:`src/settings/main.ts` 模块加载时顶层 fire-and-forget `void initI18n()` 先跑,然后 init() 在第 131 行又 `await initI18n()` —— **同一次 settings 启动就调 2 次**。HMR 重 init 时 N 次叠加。

**根因**:`initI18n` 内部 `onLocaleChange(() => { applyDataI18n(); ... renderRegionSection(...) })` —— `onLocaleChange` 是 `listeners.add(fn)`,**没有幂等保护**(对比 order.ts `_orderListenerBound` / credentials.ts `_credListenerBound` 都加了 module-scope flag 防重绑)。initI18n 不像 bind*() 那样有 `_initI18nBound` flag,所以每调一次 +1 listener。

**实际触发链**:
1. `settings.html` 加载 `main.ts` → 顶层 `void initI18n()` 启动(模块 scope)
2. 紧接着 `void init()` 启动 → 内部 `await initI18n()` 时 initI18n 又跑一次,第二次 `onLocaleChange(...)` 注册第二个 callback
3. 用户在设置面板切 locale → `setLocale` 通知 listeners → 第一个 callback 跑 `applyDataI18n` + `renderRegionSection`,第二个 callback 又跑一遍 → `renderRegionSection` 内部 append 重复 section → settings 面板 region section 重复出现
4. HMR 重 init 时 = 3 次 initI18n = 3 个 listener,每切一次 locale region section 跑 3 次

**影响**:
- 首次启动切 locale 时 `renderRegionSection` 把 node append 到 `containers.app` 第二次,**DOM 出现两个 region-section**("语言"radio 块重复 + Apply 按钮 2 个),点 Apply 一次触发 2 次 region apply
- 持续累积:用户每次切语言,DOM 多一节 region-section → settings 面板在「应用」section 越来越长
- `applyDataI18n` 重复跑成本低(textContent 二次写 idempotent),但 N 个 region 实例的 IPC `setRegion` 重复触发是真实开销

**证据/调用链**:
```typescript
// settings/main.ts:46-61
async function initI18n() {
  await initLocale();
  applyDataI18n();
  onLocaleChange(() => {     // ← 每次 initI18n 注册一个新 listener,无 flag
    applyDataI18n();
    const regionContainer = document.getElementById("section-region");
    if (regionContainer) void renderRegionSection(regionContainer);
  });
}

void initI18n();             // ← 顶层 fire-and-forget,先启动

async function init() {
  try {
    await initI18n();        // ← init() 内再 await 一次,重复注册
    ...
  }
}
void init();
```

对比 `credentials.ts:922-933` bindStepfunLoginEvents 的正确写法:
```typescript
let _stepfunListenersBound = false;
export function bindStepfunLoginEvents() {
  if (_stepfunListenersBound) return;  // ← module-scope flag
  _stepfunListenersBound = true;
  void listen(...).then((un) => _stepfunListeners.push(un));
  ...
}
```

**修复建议**:加 module-scope `_initI18nBound` flag,仿 bindStepfunLoginEvents 模式。同时删顶层 `void initI18n()` —— init() 内 await 已覆盖。或者保留顶层 fire-and-forget 但 init() 里别 await,改 `initI18nStarted.wait()`。

**待实测**:开 settings 面板 → 切 locale → 看「应用」section 是否出现 2 个 region-section;HMR reload 后切 locale 计数。

---

## P2 (中)

### D7-003 — `settings/main.ts` 顶层调用的 `setupNav` / `setupTabs` / `region-wizard` 都无幂等保护,HMR 重 init 时累积 listener

**置信度**:中(dev-only,生产构建不触发) **文件**:`src/settings/main.ts:114-119` (setupNav,含 listen) + `:122` (setupTabs) + `utils.ts:68-80` (setupTabs 实现) **触发条件**:`pnpm tauri dev` 时 Vite HMR 重新评估 `settings/main.ts` 模块 → 第 119 行 `setupNav()` 重跑 → 顶部 `navItems.forEach((item) => { item.addEventListener("click", ...) })` 给每个 nav 按钮加 1 个新 click handler;同时 `setupTabs` 给每个 `.tab` 按钮多加 1 个 handler。

**根因**:和 D7-002 同根 —— `setupNav` / `setupTabs` 模块顶层同步调用,**没有 module-scope flag** 防重复执行。HMR reload settings/main.ts 时它们被 re-invoked,在原 DOM 上叠 N 个 click listener(同一个 nav 按钮)。

**实际触发**:
1. 用户开 settings → setupNav() 跑,N 个 `.nav-item` 各挂 1 click handler
2. dev 时改一个 settings 文件 → Vite HMR 重新评估 settings/main.ts → setupNav() 再跑,每个 `.nav-item` 再挂 1 个
3. 用户点「Providers」nav → 第 1 次 handler 切到 providers + 第 2 次再切(noop)+ 第 3 次再切...
4. 类似 setupTabs 每个 tab 按钮叠 N 次 click handler

**影响**:
- dev 时设置面板变卡、nav 切换闪烁(N 次 navigate 调用)
- 改 `renderAdvancedSection` 等任何文件都会触发重 init
- 生产构建 (tauri build) 不触发 → 用户体验无影响,但 dev 体验劣化 + 排查易混淆

**证据/调用链**:
```typescript
// settings/main.ts:119, 122
setupNav();   // ← 顶层同步调,无 flag
setupTabs();

// utils.ts:68-80
export function setupTabs() {
  const tabs = document.querySelectorAll<HTMLButtonElement>(".tab");
  const panels = document.querySelectorAll<HTMLElement>(".provider-panel");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {  // ← 每次 setupTabs 重跑,叠加
      ...
    });
  });
}
```

**修复建议**:加 module-scope flag,仿 bindCredentialButtonsGlobal 模式:
```typescript
let _navBound = false;
function setupNav() {
  if (_navBound) return;
  _navBound = true;
  // ... 现有代码
}
```

**待实测**:dev 模式改 settings/advanced.ts → 检查 `.nav-item` 元素上的 click listener 数(DevTools Event Listeners panel,或 `getEventListeners(btn)` console.log),应保持 = 1 而非 2+。

---

### D7-004 — `settings/main.ts:114` `listen<string>("musage://settings-navigate"...)` 没存 unlisten 句柄,HMR leak

**置信度**:中(dev-only) **文件**:`src/settings/main.ts:114-116` **触发条件**:`pnpm tauri dev` 时设置面板 HMR 重 init,settings-navigate listener 不被卸载。生产不触发。

**根因**:listen() 返回 `Promise<UnlistenFn>`,当前实现 `.catch(err => console.error(...))` 但**不存 unlisten**(对比 main.ts:1301 `trackUnlisten(promise, label)` 模式 —— settings/main.ts 没用)。

**实际触发**:
1. setupNav() 跑,listen("settings-navigate", ...) 内部注册一个 Tauri 端 event listener
2. HMR 重 init → setupNav 再跑,再注册 1 个 listener
3. 用户在浮窗点「打开设置面板」并带 section 参数 → Tauri emit settings-navigate → **两个 listener 都跑**,前者切到正确 section 后后者再切一次(no-op 但 IPC 多一跳)

**证据**:
```typescript
// settings/main.ts:114-117 (setupNav 内部)
listen<string>("musage://settings-navigate", (e) => {
  navigateToSection(e.payload);
}).catch((err) => console.error("settings-navigate listen failed:", err));
// ↑ Promise 丢弃,unlisten 句柄无法拿
```

对比 main.ts:1301-1308 的正确模式:
```typescript
const trackUnlisten = (promise: Promise<UnlistenFn>, label: string) => {
  void promise
    .then((unlisten) => {
      if (disposed) unlisten();
      else trackedUnlisteners.add(unlisten);
    })
    .catch(...);
};
```

**修复建议**:用 `trackUnlisten` 模式(共用 main.ts 的 `domAbort` / beforeunload cleanup),或在 setupNav 外面套一层 module-scope `_settingsNavigateBound` flag 同 `_navBound`(D7-003 一并修)。

**待实测**:dev 改 settings/region-wizard.ts → Tauri DevTools 里 `invoke('plugin:event|listen')` 的 listener list 数。

---

### D7-005 — `settings/source-extras.ts:264` `helpDiv.innerHTML = t(...)` 是 advanced.ts M5 fix 干掉过的同一 anti-pattern 残留

**置信度**:中(没真安全漏洞但与 M5 fix 不一致) **文件**:`src/settings/source-extras.ts:264` **触发条件**:`zhipu_region_help` 在 i18n JSON 里包含未转义的 `<a href>` / `<code>` / `<br>` 之类 HTML 片段(当前是静态 baked HTML in JSON,信任度高)。

**根因**:`src/settings/advanced.ts:134-142` M5 fix(2026-07-06 全量审查)把 `help.innerHTML = t(...)` 改成 `help.textContent = t(...)`,因为同款 anti-pattern 的 XSS 风险(i18n 翻译者误闭合属性 + JSON 文件污染 + locale 缺失 fallback 到字面 key 都会被 `<img onerror>` 攻击 —— 虽然 strict CSP 会拦 onerror, 但 dev mode CSP 宽松时被穿透)。

source-extras.ts:264 留同款 anti-pattern **没修**:
```typescript
// source-extras.ts:262-264
const helpDiv = document.createElement("div");
helpDiv.className = "help";
helpDiv.innerHTML = t("extras.zhipu_region_help");  // ← anti-pattern 残留
```

**影响**:
- 当前翻译 JSON 是开发者 baked,无 XSS 风险(同 advanced.ts M5 fix 同款"信任度高"前提)
- 未来如果 dev 改 JSON 时手抖粘了 `<img src=x onerror=fetch('/keys')>`,CSP 在生产拦了,但 dev mode / Tauri release debug 模式下 CSP 较弱时可能泄露
- 项目约定一致性:M5 fix 同款 anti-pattern 该一致处理

**证据/调用链**:
```typescript
// settings/advanced.ts:134-142 (M5 fix 后的"正确"实现)
const cookieHelp = el("div", { class: "help" });
cookieHelp.textContent = t("settings.advanced.xiaomi_cookie_help") + "...";

// settings/source-extras.ts:264 (同款 anti-pattern 未修)
helpDiv.innerHTML = t("extras.zhipu_region_help");
```

`credentials.ts:30-34` `renderHelp()` 也是同款,**已注释 "信任度高(无用户输入),可以直接 innerHTML"** 当作 intentional。但 advanced.ts M5 fix 后,渲染 help 类节点的推荐路径已迁到 textContent。

**修复建议**:跟 M5 fix 对齐,改 `helpDiv.textContent = t(...)`(智谱 region help 当前 JSON 值是纯文本,改 textContent 后链接会变成字面 "https://..." 不再可点)。若要保留链接 markup,引入 `marked` + DOMPurify 白名单,sanitize 后 innerHTML。

**待实测**:v0.3 加新 provider 带 region selector 时,所有类似 help 节点都用 textContent 渲染。

---

## P3 (低 / 边缘情况)

### D7-006 — `credentialProviderName` 调用 `t()` 对 builtin extra 副本 base id 拿不到 i18n key 时返 "provider.X.name" raw 字面

**置信度**:中(新加 provider 触发) **文件**:`src/settings/credentials.ts:622-639` **触发条件**:Builtin extra 副本("minimax#2")的 base id 在未来如果改了(比如 rename provider)、或者新加 builtin provider 还没同步进 `src/i18n/{en,zh-CN}.json`。

**根因**:
```typescript
async function credentialProviderName(id: string): Promise<string> {
  const hashIdx = id.indexOf("#");
  if (hashIdx > 0) {
    const base = id.slice(0, hashIdx);
    const n = Number(id.slice(hashIdx + 1));
    return formatDisplayName(t(`provider.${base as ProviderId}.name`), n);
    //               ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
    //               如果 base 在 i18n dict 没 key,t() 返 "provider.X.name"
    //               formatDisplayName 拼成 "provider.X.name #N" → flash 显示给用户
  }
  ...
}
```

**影响**:flash 消息从 "已删除 MiniMax #2" / "已保存 MiniMax #2 key" 退化成 "已删除 provider.minimax.name #2 key"。用户看到字面 key 报 bug,但功能 OK(dev 模式 console.warn 会打 "missing key")。

**证据/调用链**:
```typescript
// settings/credentials.ts:626-628
const base = id.slice(0, hashIdx);
const n = Number(id.slice(hashIdx + 1));
return formatDisplayName(t(`provider.${base as ProviderId}.name`), n);

// utils.ts:163-166 (formatDisplayName)
export function formatDisplayName(base: string, instanceIndex: number): string {
  if (instanceIndex <= 1) return base;
  return `${base} #${instanceIndex}`;  // ← "provider.X.name #2" 当 raw key 传入
}
```

**修复建议**:`t()` 返原 key 时(missing key)走 fallback + dev warn),新加 builtin provider 时 CI 守卫检查 `src/i18n/{en,zh-CN}.json` 跟 `builtin_sources` 注册表对齐(已有的 prevent-missing-key test 可扩展)。代码层加防御:`if (name === rawKey) return id`(返原始 id 字符串比 raw i18n key 干净)。

**待实测**:rename `xiaomi_region_wizard` 没 i18n key 时,`deleteCredentialAction` 显示什么。

---

### D7-007 — `applyPinMode` 拼 key 时 mode 不在 enum 范围返 `settings.pin_mode.` raw

**置信度**:低(防御性,实际 Rust 端 enum 强制约束) **文件**:`src/settings/floating.ts:22` **触发条件**:Rust 后端 `set_floating_pin_mode` 接受 enum 但前端拼 key 漏 nullish check。

**根因**:
```typescript
const label = t(`settings.pin_mode.${mode === "pin_top" ? "top" : mode === "pin_bottom" ? "bottom" : "normal"}`);
```

mode 不在三个值的范围时(类型层面不可能,但运行时 RPC 数据可被替换),fallback 进 `else` 分支 → key 拼成 `settings.pin_mode.normal`(语义 OK)。真实问题:mode 落 `null` 时 `mode === "pin_top"` 返 false → else 进 → "normal"。看似正常但浮窗实际是 None 态。

**影响**:几乎不可触发(`mode: FloatingPinMode` 类型联合 + 后端 enum 同步约束)。Dev menu / 用户手动调 IPC 改 cfg.floating_pin_mode = 未知字符串 时可能触发。

**修复建议**:floatinPinMode 不通过类型断言时 default to "normal" 已经做了。无硬修复必要。

**待实测**:构造 `cfg.floating_pin_mode = "garbage"`,看 flash 行为。

---

### D7-008 — `testConn()` 用 `p.source_id ?? p.provider` 拼接,不识别 `unique_id` → 多 instance 测试摘要合并显示

**置信度**:中(可观察但非 bug) **文件**:`src/settings/test.ts:19, 29` **触发条件**:用户有 minimax#2 extra instance,点「测试连接」按钮。

**根因**:
```typescript
const id = p.source_id ?? p.provider;  // ← 没试 unique_id
if (id === "minimax") {  // ← minimax#2 也掉这里
  ...
}
```

多 instance 的 summary 行都用 base id "minimax",**两份 minimax 出现时 flash 一行写两个 "minimax 25% / minimax 50%"** —— 用户没法分辨哪份是第一张 minimax、哪份是 #2 副本。

**影响**:UX 降级,可读性问题,不是真 bug。功能上 refresh_now 确实都测了。

**修复建议**:用 `p.unique_id ?? p.source_id ?? p.provider`,summary 行展示 unique_id 后缀。和浮窗 main.ts:1356 `snapKey()` 一致。

**待实测**:2 份 minimax extra instance → 测试连接 → 看 flash 行数 + label 格式。

---

## 没找到 bug 的可疑点(防御性确认)

| 检查点 | 文件:行 | 结论 |
|---|---|---|
| XSS via `p.error` | `src/main.ts:823` | ✅ `escapeHtml(p.error ?? t(...))` 包,**安全**。自定义 source 错误信息/zenmux 自定义 URL 错误响应全经 escapeHtml |
| XSS via `p.raw` 渲染 | 无 | ✅ main.ts 不渲染 `raw` 字段,只在 LogStore / DevTools 用 |
| XSS via `t("settings.logs.no_logs_filtered")` | `src/settings/logs.ts:91-99` | ✅ log entries 用 `escapeHtml(e.message)` / `escapeHtml(e.provider)` 等 |
| 事件监听器累积(main.ts init) | `src/main.ts:1271-1330` | ✅ 用 `domAbort` + `disposeFloatingInit` cleanup pattern,init 重入幂等(A-C1 fix 2026-07-29 兜底 click handler 注册在首个 await 前) |
| 计数 timer 泄漏 | `src/main.ts:1082-1092` | ✅ `countdownTimer === null` guard,`stalePurgeTimer === null` guard,cleanup 时 clearInterval |
| `dragGhost` 残留 | `src/settings/order.ts:166-181` | ✅ `resetDragState()` 在 `renderDeleteExtraButton` 点击后调(clean section 前),ghost + placeholder 都 remove |
| `lastGoodSnap` 内存泄漏 | `src/main.ts:289-308` | ✅ `purgeStaleLastGoodSnap` 60s setInterval 清 TTL 过期 entry |
| `escapeXml` SVG fallback | `src/settings/logos.ts:49-53` | ✅ escapeXml 函数正确转义 & < > " ' |
| DRAG threshold | `src/settings/order.ts:347-356` | ✅ 5px 阈值 + norm trim 兜底多次实测调过 |
| locale 二次监听 | `src/i18n/index.ts:139-174` | ✅ setLocale 后端失败时回滚 current + 主动通知 listeners(B-M2 fix 2026-07-30) |
| search filter 中文 | `src/settings/providers.ts:167-180` | ✅ `applySearchFilter` 用 trim().toLowerCase().includes() 走通用路径,中文 case-fold no-op,substr 命中 works |
| order test 覆盖 | `src/settings/order.test.ts` (263 行) | ✅ 11 个边界 case 覆盖 `computeSameSectionMove` (src/order 跨段 / 同段 / 同位置 no-op) |
| order drag 边界 (`dragSrcIdx === -1`) | `src/settings/order.ts:464` | ✅ 检 `dragSrcIdx < 0` 早返 (没有 src id 拖不到) |
| `credentialProviderName` custom_<uuid> | `src/settings/credentials.ts:631-638` | ⚠️ 走 listExtraInstances 异步 fallback,return id 字面(uuid 太长,flash 难看)。D7-006 顺带提(没单列) |
| i18n plural 边界 | `src/i18n/index.ts:62-67` | ✅ count = 0 时 Intl.PluralRules 返 "other",走 .other 后缀,合理 |
| `t()` placeholder 大小写 | `src/i18n/index.ts:91-93` | ✅ `[\w.-]+` 扩过(D5 fix 2026-06-20),`{user-id}` / `{err.code}` / `{-N}` 都能匹配 |

---

## 待实测清单(超 4 个未在生产实测触发)

| 编号 | 实测命令 | 预期 |
|---|---|---|
| D7-001 | `pnpm test` 加新 case: `parseBatchLine("sk-kimi-abc123")` | 应返 `[]`,不命中 minimax |
| D7-001 验证 | 批量粘贴 `sk-or-v1-xxx` / `sk-cp-yyy` / `sk-zzz` 三行 | 1st → openrouter, 2nd → minimax, 3rd → unrecognized |
| D7-002 | dev settings 切 locale 一次 | region section 应出现 1 次,不是 2 次 |
| D7-003 | dev 改 settings/advanced.ts → 切「Providers」nav | 切换应立即响应,不重复触发 IPC |
| D7-005 | 改 `extras.zhipu_region_help` 值 → 注入 `<img src=x onerror=alert(1)>` | 严格 CSP 应拦 onerror,dev mode CSP 宽松时是否泄露 |
| D7-008 | 加 minimax#2 extra instance → 测试连接 | flash 应显示 "minimax 25% / minimax#2 30%" |

---

## 总结

**真 P0 bug**:0 个 —— 主要功能浮窗渲染 / 凭据管理 / 设置面板 / 拖拽排序 / 一键登录主链路无 P0 崩溃级问题。

**真 P1 bug**:2 个 —— **D7-001**(批量粘贴 key 误归 minimax) 是用户可感知的数据错位,**D7-002**(initI18n 累积 listener) 是 settings 面板切 locale 时 region section 重复。两者都可在 dev settings 实测触发。

**P2/P3**:6 个 —— 主链路 safe,都是边缘/未来扩展场景。生产构建(`pnpm tauri build`)不会触发 HMR 累积(D7-003/D7-004),dev 模式才会暴露。

**对比 v0.2.0 已修项**:v0.2.0 杀 `loadConfig`/`saveConfig` 死代码 + v0.2.1 修 i18n dynamic import + v0.2.1 `bindCredentialButtonsGlobal` `_credListenerBound` flag + advanced.ts M5 fix 改 textContent,大部分常见反模式已陆续清理。本轮发现的都是这一波没 cover 到的"最后一公里"。
