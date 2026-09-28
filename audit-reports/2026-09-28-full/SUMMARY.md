# 2026-09-28 全量代码审查报告（8 域并行）

> **修复状态（2026-09-28）**：62 条独立问题中 **61 条已全量修复**（commit `d4ab961`，48 文件 +4673/−611）。
> 唯一例外：**H-5（无单实例保护）**——修法需引入新依赖 `tauri-plugin-single-instance`，
> 维护者决定本轮不修，留 v0.3。该问题的两个后果（双进程写出非法 keys.json / 双倍消耗
> 5h·周配额）需要用户**不要同时跑已安装版和 `pnpm tauri dev`**，或等 v0.3。
>
> 随修复一并落地的还有 5 条 CI 守门脚本 + 1 篇准则文档（见第六节），它们比单条 bug
> 更值得保留——本轮 7 条 Medium 的根因是「约定写了但没有 enforcement」或「修复未平行
> 移植 / 修复是空操作」，单靠人记必然复发。
>
> 验证基线：`cargo check` 0 error / `cargo test --lib` **511 passed 0 failed**（审查前 416）
> / `cargo fmt --check` 0 违规 / `cargo clippy` 0 error / `tsc --noEmit` 0 errors /
> `vitest` 29/29 / 5 条守门脚本 exit 0。

> **基线**：`08734ba`（v0.2.9 发布后 3 个 commit）
> **规模**：~32k 行 Rust + ~10k 行 TypeScript，14 内置 provider + custom，4 个登录模块（xiaomi / anysearch / stepfun / kimi）
> **方法**：8 个并行审查 agent，每域独占文件集，逐行读完 + Grep 追调用链确认可达性。要求每条 finding 必须有具体触发条件 + `file:line` 代码证据。
> **对照基线**：`audit-reports/2026-09-04-full/SUMMARY.md`（103 条，已于 2026-09-05 全量修复）

## 总览

| 域 | 范围 | H | M | L | 小计 |
|---|---|---|---|---|---|
| D1 | providers 框架 + A（mod/parse/custom/minimax/deepseek/tavily/zenmux/openrouter） | 1 | 4 | 4 | 9 |
| D2 | providers B（xiaomi/kimi/kimi_desktop/anysearch/siliconflow） | 0 | 4 | 6 | 10 |
| D3 | providers C（volcengine_ark/stepfun/zhipu/claude_official/tokendance） | 0 | 6 | 5 | 11 |
| D4 | poller / poller_backoff / logstore / config.rs | 2 | 2 | 6 | 10 |
| D5 | commands/* IPC + config/extra_instances.rs | 1 | 3 | 4 | 8 |
| D6 | 登录模块 ×4 | 2 | 2 | 2 | 6 |
| D7 | platform / tray / lib | 0 | 5 | 6 | 11 |
| D8 | 前端全部 TypeScript | 3 | 8 | 4 | 15 |
| **原始合计** | | **9** | **34** | **37** | **80** |
| **跨域去重后** | 10 个系统性簇吸收 18 条 | **9** | **26** | **27** | **62** |

## 关键结论

- **无 Critical。** 上一轮（8-17）有 1 Critical，9-04 有 0 Critical，本轮仍为 0。
- **9 条 High** 全是真实用户可见的功能损坏，无数据损坏、无凭据泄漏、无 app 级挂死。
- **代码质量较上一轮明显提升**：9-04 的 103 条里 12 条 High，本轮 80 条里 9 条 High，且其中 6 条是新功能（volcengine 双套餐 / 托盘 Agent Plan / 浮窗底部余量 / AnySearch 登录双案 / 托盘色板）引入的。
- **本轮最有价值的产出不是单条 bug，而是 10 个跨域系统性簇**（见第五节）。其中「修复是空操作」和「修复未平行移植」两簇共 7 条，说明上一轮的部分修复**代码改了但不产生效果**，这类比未修的更危险——报告标已修、CI 绿、逻辑看起来对，但功能是坏的。

---

## 一、High（9 条）

### H-1 [D8] 「添加自定义来源」的 accent 色板永远选不中 —— `<button>` 不派发 `change`
- **位置**：`src/settings/extra-instance-form.ts:131-141`（监听）、`:337-345`（色板元素）、`:583-584`（读值）
- **问题**：色板按钮是 `<button type="button">`，浏览器对 button **只派发 `click`，永不派发 `change`**。选色逻辑挂在 `dynamicFields` 的 `change` 委托里，`.selected` 类永远加不上。
- **触发条件**：设置 → 数据源 → 「+ 添加新来源」→ 选 custom → 点任意颜色 → 填完保存。每个 custom 中转站都以 `accent: null` 落盘，浮窗永远用 `#888` 灰的首字母头像。
- **证据**：
```ts
// extra-instance-form.ts:131
dynamicFields.addEventListener("change", (e) => {
  const target = e.target as HTMLInputElement;
  if (target.classList.contains("accent-swatch")) {   // ← 永不命中
// :338 —— 元素是 button 不是 input
...ACCENT_PALETTE.map((c) => el("button", { type: "button", class: "accent-swatch", ... }))
```
- **对照**：`app.ts:213-215` 同款色板用 `click`，托盘颜色因此是好的 —— 这是笔误不是设计。
- **修法**：把 `.accent-swatch` 分支从 `change` 挪到独立 `click` 委托。

### H-2 [D8] 浮窗「4 档自定义色」在 macOS 全哑 —— `<input type=color>` + 只听 `change`
- **位置**：`src/settings/floating.ts:285-289`
- **问题**：仍是原生 `<input type="color">` 且只绑 `change`。WKWebView 的 NSColorPanel 能打开能取色，但 input/change **全程不派发**。托盘那 4 个已在 `6931dfb` 换掉，这 4 个漏了。
- **触发条件**：macOS 设置 → 浮窗 → 颜色档位 → 取色 → 什么都不发生，`color_overrides` 永不落盘。Windows WebView2 正常，故只在 mac 暴露。
- **证据**：
```ts
colorPickers[key] = el("input", { type: "color", id: `color-${key}`, ... });
colorPickers[key].addEventListener("change", () => void applyAll());   // ← NSColorPanel 不发
```
- **修法**：复用 `app.ts` 的色板 + hex 文本框。

### H-3 [D8] 拖拽排序在窗口外松开鼠标 → 幽灵卡 + 源行永久隐藏 + 每次 aborted 拖拽泄漏一份 ghost
- **位置**：`src/settings/order.ts:219-224`（绑 mouseup）、`:261-293`（唯一收尾路径）
- **问题**：`onDragMouseDown` 挂了 `document` 的 `mousemove`/`mouseup`，但**无 `blur`/`pointercancel`/`mouseleave` 兜底**。窗口外松手时两个平台都不派发 `mouseup` → `onDragMouseUp` 永不跑 → `dragGhost`（`position:fixed; z-index:9999`，挂 body）永远留着盖在面板上；`dragPlaceholder` 永远留在列表里；被拖源 `li` 永远 `display:none`。下一次 mousedown 会**再新建**一份并把 module 变量指向新的，旧的再没人回收。
- **触发条件**：按住任意行往窗口下方拖并在窗口外松手 → 列表出现一行空白 + 半透明幽灵浮在面板上，每次 aborted 泄漏一份。
- **修法**：`mousedown` 时同时挂 `window blur` + `pointercancel`，cancel 调已有的 `resetDragState()`（`order.ts:176-186` 已把 ghost/placeholder/`display:none` 清干净），三个 listener 一起摘。

### H-4 [D4] `read_keys()` 漏剥 UTF-8 BOM —— keys.json 带 BOM 时全部凭据不可读，且用户无法通过 UI 重存
- **位置**：`src-tauri/src/config.rs:1423-1440`
- **问题**：9-07 的 C1 fix（`5cc6f49`）给 `config.json` 和 `extra_instances.json` 都接了 `strip_bom_owned`，唯独漏了 keys 这条。`strip_bom_owned` 自己的文档（`config.rs:1412-1415`）明确写着「keys.json 同根问题……所有产线读路径必须经过本 helper」。
- **触发条件**：`keys.json` 被 Windows Notepad / 部分 IDE 保存过一次（带 `EF BB BF`）→ serde_json 拒收 → 后果链：
  1. `load_credential_for_id` 返 Err → `refresh_inner` 把**所有** provider 打成网络错误卡；
  2. `refresh_single_inner` 在写 snapshot/backoff **之前**就 return Err → per-provider 轮询静默什么都不做；
  3. `save_credential_for_id` 也是 `read_keys()?` → 用户在设置面板重新填 key **直接失败**，只能手改磁盘文件。
- **证据**：
```rust
fn read_keys() -> Result<KeysMap, String> {
    let s = std::fs::read_to_string(&path)          // ← 没有 strip_bom_owned
    let err = match parse_keys_payload(&s) { ... };  // ← serde_json 遇 BOM 必炸
```
- **修法**：`read_to_string` 结果过一遍 `strip_bom_owned`（一行）+ 补 BOM 单测。**这是 9-07 C1 修复的不彻底，不是新问题。**

### H-5 [D4] 无单实例保护 + 固定名 tmp 文件 → keys.json 可被写碎，且双进程双倍烧配额
- **位置**：`config.rs:921` / `:1360`、`config/extra_instances.rs:247`、`logstore.rs:447`；`Cargo.toml` 无 `tauri-plugin-single-instance`，`lib.rs`/`main.rs` 无 pid file / flock
- **问题**：`save_lock()`（`config.rs:50`）是**进程内** `std::sync::Mutex`，对第二个进程零保护；四条原子写路径全部用**固定 tmp 名** + `rename`。
- **触发条件**：同时跑已安装的 Musage.app 和 `pnpm tauri dev`（macOS 默认允许多实例），或开机自启撞上手动启动。典型交错：A truncate 打开 `keys.json.tmp` 开始写 → B truncate 并写完 → B rename → A 的 write 落在已被 rename 的 inode 上 → A `set_permissions(&tmp)` 得 ENOENT 返 Err，而 `keys.json` 里可能是交错的**非法 JSON** → 触发 H-4 全凭据不可读。config.json 侧则 last-writer-wins 静默吞掉改动且无日志。
- **附带**：两个进程各自跑 poller → 14 个 provider 的 HTTP 请求翻倍 → **5h / 周配额被双倍消耗**。
- **修法**：加 `tauri-plugin-single-instance`（Tauri 2 官方，init 一行）；或退一步在 cfg 目录放 `create_new(true)` 锁文件；tmp 名加 pid 后缀。

### H-6 [D5] `delete_extra_instance` 的 compact 重命名是**无效修复** → 幽灵卡 + 重复卡
- **位置**：`commands/extra_instances.rs:644-666`、`commands/mod.rs:2093-2098`
- **问题**：8-17 的 H-03 修法注释称「`source_id` 是 stable 标识，直接 mutate 即可」，但 `snapshot_key()` 明确**优先取 `unique_id`**，`apply_provider_order` 与前端 `snapKey()` 也都是 `unique_id ?? source_id ?? provider` 三级优先。重命名后 `unique_id` 仍是 `"minimax#3"`，身份键压根没变。
- **触发条件**：minimax base + #2 + #3 三份（#3 已存凭据），删掉 #2 → compact 把 #3 改名成 #2，幽灵卡 `unique_id="minimax#3"` 原地保留 → 下一轮 push 新的 `minimax#2` → 浮窗同时出现「MiniMax #3」陈旧卡和「MiniMax #2」真实卡。更糟的是同函数的 cfg.providers 迁移帮倒忙：`remove("minimax#3")` 后 `is_enabled_unique` 走 base fallback 返回 true（base 仍启用），幽灵卡**不会被 enabled 过滤清掉**，一直挂到重启。
- **证据**：
```rust
for p in snap.providers.iter_mut() {
    let sk = crate::commands::snapshot_key(p);   // = p.unique_id 优先 → "minimax#3"
    if sk == *old_ref { p.source_id = Some(new_ref.clone()); /* unique_id 没动 */ }
}
// commands/mod.rs:2093
pub(crate) fn snapshot_key(p: &ProviderSnapshot) -> &str {
    p.unique_id.as_deref().or(p.source_id.as_deref()).unwrap_or(&p.provider)
}
```
- **修法**：同时改 `p.unique_id = Some(new_ref.clone())`，改完断言 `snapshot_key(p) == *new_ref`。

### H-7 [D6] 小米「清除 Cookie → 重新登录」是死循环
- **位置**：`xiaomi_login.rs:235-296`（builder 未设 data_directory/incognito）+ `:562-597`（完整性校验无新鲜度门）
- **问题**：四个登录模块中**只有 xiaomi 的提取没有新鲜度门**（stepfun/kimi/anysearch 都有）。它只检查 4 个白名单 cookie「存在且非空」就写盘。同时 `WebviewWindowBuilder` 没设 `data_directory`/`incognito` → 走**持久化** WKWebView 数据 store，cookie jar 跨重启存活；而「清除 Cookie」按钮只调 `deleteSourceCredential("xiaomimimo")`，**从不碰 cookie jar**。
- **触发条件**（完全可复现，无需重启）：xiaomi cookie 失效 → 用户点「清除 Cookie」（i18n 明写"清除后需要重新登录"）→ 点「🔑 登录小米账号」→ webview 打开，**旧 cookie 还在** → `on_page_load` 命中 dashboard → 提取成功 → toast「登录成功」→ keys.json 里是**同一份已失效的 cookie** → 浮窗依旧 401。用户被困在「清除 → 登录 → 登录成功 → 还是 401」死循环，无任何逃生路径（全工程 `clear_all_browsing_data` 只在注释里出现，**零处实际调用**）。
- **证据**：
```rust
// xiaomi_login.rs:584-597 —— 只查 presence + 非空，没有 exp 门
if !(has_service_token && has_user_id) { ... return Err(...) }
let cred = Credentials { api_key: None, cookie: Some(cookie_str.clone()), secret_key: None };
// 对比 stepfun_login.rs:425-458 / kimi_login.rs:340-351 / anysearch_login.rs:235-241 —— 三个都有 60s skew 门
```
- **修法**：若新提取的 cookie 与 keys.json 中 `{target}:cookie` **完全相同**则拒绝写盘（返「请先在登录窗内登出」）；配合在「清除」动作里调一次 `clear_all_browsing_data()`。

### H-8 [D6+D2+D3] 登录 / 清除命令不带 instance id —— 多账号凭据成为 UI 删不掉的孤儿
- **位置**：`commands/mod.rs:2218-2243`（resolve 按 index 升序取第一个 enabled）、`src/main.ts:1060-1066`（relogin 按钮不带 `data-unique-id`）、`kimi_login.rs:225`（`delete_cookie_slot_for_id("kimi")` 硬编码 base）、`settings/credentials.ts:838-879/906-916/945-955`（四个 clear action 都 `if (id !== "<base>") return`）、`commands/mod.rs:644/662`（火山套餐 setter 只刷 base）
- **问题**：9-07 的 D7-02 让**写入侧**走 `resolve_login_refresh_target()`（可能是 `"<base>#N"`），但 **① 浮窗「🔑 重新登录」按钮完全不带是哪张卡的 id；② 所有「清除」按钮和状态徽章读写的仍是 base 槽；③ provider setter 只刷 base 实例**。
- **触发条件**（多账号用户可达）：
  1. base `anysearch` 取消勾选、在顺序 section 勾上 `anysearch#2`（`set_provider_enabled` 会写显式 `cfg.providers["anysearch#2"]` entry）；
  2. 浮窗「AnySearch #2」卡 auth_failed → 点重新登录；
  3. 解析出 `anysearch#2` → 新 token 写进 `anysearch#2:cookie`，**但 #2 卡仍旧红**，且设置面板 base 行徽章读 base 槽显示「未设置」；
  4. 点 base 行「清除」→ 只删 base 槽，**`anysearch#2:cookie` 删不掉**，用户再也无法通过 UI 撤销这次登录写进去的 token。
  
  同源：base 禁用 + `volcengine_ark#2` 启用时，两个套餐筛选 setter **完全空转**（`refresh_single_inner:2261` 的 `is_enabled_unique` 早退），浮窗纹丝不动直到 poller 全量 tick。
- **证据**：
```ts
// src/main.ts:1062 —— 卡片 id 完全没传给后端
} else if (kind === "auth_failed" && baseId === "anysearch") {
  actionBtn = `<button class="err-btn err-btn-relogin" data-action="relogin-anysearch">...`;
// commands/mod.rs:2236 —— 与用户点的哪张卡无关，按 index 升序取第一个
for idx in indexes {
    let unique = format!("{base}#{idx}");
    if cfg.is_enabled_unique(&unique, base) { return Some(unique); }
}
```
- **修法**：4 个 `open_*_login_window` 加 `instance_id: Option<String>`（前端 relogin 按钮补 `data-unique-id="${id}"`、clear action 用当前 `meta.id`），`resolve_login_refresh_target` 优先返回传入 id；`clear_kimi_session` 与 provider setter 同样改为遍历匹配 `base_id_of()` 的全部实例。

### H-9 [D1+D6] i18n 缺 2 处 key —— 错误信息渲染成 `zh-CN.xxx` 字面量
- **位置**：`providers/openrouter.rs:355-361`（`error.common.api_error`）、`anysearch_login.rs:711-713`（`login.anysearch.timeout`）
- **问题**：两处 `t!()` 引用的 key 在 `locales/{en,zh-CN}.json` 里都不存在。rust-i18n 3 找不到 key 时返回 `format!("{locale}.{key}")` 且**命名参数不做替换**。
  - **openrouter**：9-07 的 H-Provider fix 本意是让用户看到真实报错，结果一个字都看不到。
  - **anysearch**：14 分钟登录超时时用户看到字面量 `"AnySearch 登录失败: login.anysearch.timeout"`；stepfun/kimi 同场景文案正常。
- **触发条件**：OpenRouter 返回 HTTP 200 + `{"error":{"code":401,...},"data":null}`（该 fix 明确针对的形态）；或 AnySearch 登录 14 分钟未完成。
- **证据**：全 crate 扫 `t!("a.b.c")` 对两份 locale，**只有这 2 处缺**；`error.common` 现有 27 个 key 里没有 `api_error`，`login` 对象只有 `stepfun`/`kimi` 两个子键。
- **修法**：两份 locale 各补 2 个 key。**建议同时加 CI 守门**：扫全部 `t!("x.y.z")` 对两份 locale 做存在性断言（见第六节）。

---

## 二、Medium（26 条）

### 配置 / 持久化
| ID | 位置 | 问题 | 触发条件 |
|---|---|---|---|
| M-1 | `commands/mod.rs:974-978` + `api.ts:45-51` | `save_config` 入参**全量替换**，前端 `saveChain` 只串行化「写」不串行化「读」（`getConfig()` 在队列外） | 改轮询间隔的同一瞬间拖浮窗 → geom persister 每 500ms 锁内保存写入**更新的** x/y → 随后 `save_config` 用旧快照覆盖 → 用户刚拖好的位置静默回滚并落盘 |
| M-2 | `commands/mod.rs:810-812` + `:833-840` | `schema_overrides` 超限「拒绝」分支是死代码：810 先无条件 `clear()`，833 的 `return Err` 恒 false | 导入 >256 条 override 的 config.json → 所有 MiniMax/Xiaomi 字段候选被**静默清空并报保存成功**，与同函数 `providers`/`provider_order` 的 Err 口径相反 |
| M-3 | `config.rs:1095-1098` + `:893-908` | best-effort 恢复路径把磁盘上的**未来** `schema_version` 搬进内存 → `save()` 的降级守卫让本会话**所有**保存永久失败 | 降级 Musage 后导入新版 config.json，且至少一个字段形状不兼容 → best-effort → 此后 `set_provider_enabled` 等全部写不进去，重启后 .bak 才恢复 |
| M-4 | `logstore.rs:171` | 遮蔽正则 `auth==` 是 typo（两个等号），永不命中 `auth=<token>` | custom 中转站非 2xx 时 body 前 200 字符进日志；custom 的 key 无厂商前缀（`sk-`/`tvly-`/`tp-`/`tk-`/`eyJ` 全不覆盖），`auth=` 是唯一通用兜底 → token 落 `app_log.jsonl` |

### Provider — 解析 / 口径
| ID | 位置 | 问题 | 触发条件 |
|---|---|---|---|
| M-5 | `providers/volcengine_ark.rs:508` | `raw.get("ResponseMetadata").and_then(\|m\| m.get("Error"))` 对「键存在值为 null」返 `Some(&Value::Null)`，无 `is_null()` 守卫 → `"Error": null` 的**成功**响应当成业务错误 | 火山网关返带 `"Error": null` 的成功信封 → Coding 组整组消失；只开 Coding 时整卡报 `code unknown` |
| M-6 | `providers/stepfun.rs:311-319` + `:585-599` | 顶部预检已 refresh 过一次，若随后 fetch 返 AuthFailed 则**再** refresh 一次；锁内重读守卫 `latest != token` 因刚写的就是同一串而不命中 → 拿刚被服务端 rotate 掉的 refresh 半段再 POST（文件自己的注释写着「第二次必败 40114 revoked」），失败后 `?` 直接抛 AuthFailed，**那个刚刷新的 access token 从没被用过** | token 临近过期（每 ~30min 一次窗口）+ 该次 fetch 返任何 AuthFailed（触发面很宽，见 M-7） |
| M-7 | `providers/stepfun.rs:487-504` | `haystack.contains("illegal")` 无边界匹配 | 服务端返 `{"status":-1,"message":"Illegal parameter"}` → 归 AuthFailed → 走进 M-6 的二次 refresh → 用户被迫重登，真因只是参数问题 |
| M-8 | `providers/stepfun.rs:567-691` + `stepfun_login.rs:486-502` | `REFRESH_LOCKS` 是 provider 私有 `OnceLock`，`stepfun_login::save_token` 完全不参与，两条写路径后写者赢 | poller 的 refresh POST 在途时用户点「重新登录」→ 登录写的新 token 被**旧 refresh 派生出的 pair 覆盖** → UI 显示登录成功但 token 已回滚 |
| M-9 | `providers/volcengine_ark.rs:620-639` | `.or_else(QuotaUsage)` 只在 key **缺失**时回退；`"UsageList": []` 存在但为空时 `as_array()` 返 `Some(&[])`，`or_else` 不触发 → `no_rows_found` | 网关某版本为兼容同时返回 `UsageList: []` + 实数据在 `QuotaUsage`（当前实测 schema 正是后者） |
| M-10 | `providers/volcengine_ark.rs:171-181` + `locales/*.json:225` | 卡片标题恒为「火山方舟 Coding Plan」，但一张卡同时渲染 Coding + Agent 两个分组 | 同时订阅双套餐的用户（v0.2.9 双套餐的**主目标用户**）看到标题"Coding Plan"下挂着 Agent Plan 的行 |
| M-11 | `providers/xiaomi.rs:733-738` + `:743-760` | `custom_names` 构造一次后同时喂给 `plan_pct`/`comp_pct`/`month_pct` 三个查找，而前两者查的是**同一个数组** | 用户为 xiaomi 配 monthly 候选（该功能唯一用途）+ 显示模式切 `All` → 补偿行渲染套餐行的百分比 |
| M-12 | `providers/kimi.rs:284-299` + `kimi_desktop.rs` | `resolve_session_token` 读本机 Kimi Desktop 的 `kimi-auth` cookie（**一台机器一个全局账号**），只校验 JWT `exp`、**不跟当前 source 的 API key 绑定** | 建了 `kimi#2`（账号 B 的 key）而桌面端登的是账号 A → 卡片上半部分是 B 的 5h/7d，下方「总套餐」行是 A 的月度池，**一张卡混两个账号** |
| M-13 | `providers/kimi.rs:518-538` vs `:406-408` | `595a9b1` 的 H-Provider 守卫（双字段皆缺 → 返 None）只加在 `build_window_row`，姊妹函数 `parse_total_quota` 走老逻辑：`remaining` 缺失 → `0.0` → `used = limit` → **恒 100%** | `totalQuota` 从 `{}` 变成只带 `limit` 的对象 |
| M-14 | `providers/openrouter.rs:424-434` vs `:348-367` | `parse_key` 缺 `parse_credits` 已有的 body-`error` 检查，第一行就是 `missing_data_field` | `/api/v1/key` 返 `{"error":{...},"data":null}` → 报 `ErrorKind::Parse`「缺少 data」而非「key 无效」，且 Parse 不退避、`needs_settings()` 为 false → **不给「打开设置」按钮**，用户无从下手 |
| M-15 | `providers/zenmux.rs:529` | `parse_subscription_window` 首行强制 `num_f64(q, "usage_percentage")?`，缺字段整行丢弃——而 L-3 fix 的注释自称「used/max 优先抗 schema 漂移」，那条路径恰在 `usage_percentage` **存在**时才可达 | ZenMux 再次调整 schema 去掉该字段 → 两行都丢 → 整卡报 `no_rows_found` |

### 托盘 / 平台
| ID | 位置 | 问题 | 触发条件 |
|---|---|---|---|
| M-16 | `tray.rs:1423` | `provider_short_body` 用硬编码白名单 `matches!(base_id, "deepseek" \| "zenmux" \| "openrouter")` 判断余额系，**漏掉 siliconflow / tokendance / 全部 custom**（两者都是纯余额行且都在托盘菜单里） | 托盘数据源选 SiliconFlow/TokenDance → **图标正常显示余额**（`pick_tray_rows` 是泛化的 `find(remaining.is_some())`），但 hover tooltip **一个数字都没有** |
| M-17 | `tray.rs:1445-1457` | 遍历全部 rows 无 plan 分组，PlanHeader 行（`utilization: None`）被跳过 → 双套餐 tooltip 把两组一模一样的 5h/7d 并列，且 `source_display_name` 含 "Coding Plan" | 火山双套餐用户 hover 看到 `5h 20% / 7d 10% / 5h 90% / 7d 30%`，无法判断哪组是 Agent 的 |
| M-18 | `macos.rs:399-429` | `is_floating_topmost_at` 把「主线程派发失败/超时」当成「鼠标确定不在浮窗上」——两条路径都返语义确定的 `false` | macOS 主线程任何 ≥100ms 阻塞（文件对话框、WKWebView 首次初始化、NSAlert 嵌套循环）恰落在悬停期间 → 浮窗闪一下降到底部再抬起，玻璃效果同时闪断。**Windows 端同类失败走 `None → continue`，两平台语义相反** |
| M-19 | `tray.rs:880-886` + `commands/mod.rs:2740-2749` | 写侧 `set_tray_source` 的 suffix 白名单只查「后缀 == agent」+「base 能 find_source」，**不校验 (base, suffix) 配对**；消费侧余额分支又**没过** `row_matches_plan` | `"minimax:agent"` → 百分比分支全被滤空 → 余额分支 minimax 行 `remaining: None` → **永久纯 logo + 零日志**；`"deepseek:agent"` → 余额分支不过滤 → **正常显示余额**。同一个非法后缀两类 provider 表现完全相反 |
| M-20 | `tray.rs:926-935` + `providers/tokendance.rs:269-274` | `format_balance_tray` 在 `<1000` 分支直接取整，而 tooltip 走 `format_amount_short` 保留 2 位；且符号从**取整后**的值取 | 余额 ¥0.40 → 图标渲染 `¥0`、tooltip 显示 `0.40`；余额 -0.40 → 图标 `$0`、tooltip `-0.40`。9-04 的 L-tray-1 只统一了 >=1000 口径，漏了这条 |
| M-21 | `commands/mod.rs:164-175` | `set_provider_enabled` 禁用分支只做 `retain(\|p\| snapshot_key(p) != id)` 精确匹配，**无 base fallback**——而 `get_snapshot`/`refresh_inner`/`refresh_single_inner` 三处都统一走了 `is_enabled_unique` | minimax base + #2（副本未单独勾过）→ 取消勾选 base → base 卡消失但 `minimax#2` 的 snapshot 条目被原样 emit → **浮窗继续显示「MiniMax #2」**，用户以为没关掉 |

### 前端
| ID | 位置 | 问题 | 触发条件 |
|---|---|---|---|
| M-22 | `source-extras.ts:66-74` 等 7 处 + `floating.ts:98-129` 3 处 + `providers.ts:332/343` | **回滚读不到旧值**：D8-M4 的 `const previous = select.value` 写在 `change` 回调**内部**，此时控件值已被用户改掉，`previous === v`，回滚赋回它本来就有的值。D8-M5 的 3 个 checkbox 则**根本没有回滚**（`.catch` 只有 flash） | 任一控件 IPC 失败（后端落盘失败 / 校验 reject）→ 弹红色 flash「切换失败」但**控件仍停在新值**，后端仍是旧值，重开面板才发现 |
| M-23 | `providers.ts:319-331` | 非法间隔值只 flash 然后 `return`，**既不落盘也不回填盘上值** | 填 `5` 后 Tab 走人 → flash 报错，框里仍是 `5`，全局间隔没变；若改成同样的非法串 `change` 不再触发，会一直留着 |
| M-24 | `credentials.ts:794-798` | `deleteCredentialAction` 的 `deleteSourceCredential` / `loadCredentialStatus` 没包 try，调用点是 `void ...` | 后端因磁盘/锁 reject → 弹窗关了、什么都没删、**用户零反馈**，以为删了。同文件其它 7 个 delete/clear action 都包了 try+flash |
| M-25 | `app.ts:120-157` | `traySourceSelect` 只渲染 11 个固定 option，但后端接受**任何** `find_source` 能解析的 id；`currentSource` 不在列表时浏览器 `selectedIndex` 落到 0 **谎报当前是 MiniMax**，回滚时 `select.value = currentSource` 无匹配 option → `selectedIndex = -1` **下拉变空白** | 导出配置 → 另一台机器托盘选的是 `stepfun` → 这台导入 → 打开 section 看到 MiniMax → 选别的源后端 reject → 下拉空白 |
| M-26 | `app.ts:174/218-227` + `commands/mod.rs:2878` | 前端 `HEX6_RE` 严格 6 位，后端 `is_valid_hex_color` 接受 `3\|6\|8`，读侧 `parse_hex_color` 还多接受 4 位 | 从 DevTools 取色器复制 `#FFFFFFFF` 粘进 hex 框 → flash 报错颜色没变；config.json 手改成 `"#fff"` → 面板显示「未设置」但托盘其实是白色。**三侧口径各不相同** |

---

## 三、Low（27 条）

### Provider
| 位置 | 问题 |
|---|---|
| `providers/xiaomi.rs:467/635` | 业务码读 `message`，但本文件自己的测试 fixture 记录真实字段是 `msg`（`zhipu.rs:335` 取的正是 `msg`）→ 非 401xx 业务失败时错误原因恒为空 |
| `providers/xiaomi.rs:465/633`、`siliconflow.rs:219` | 业务码只吃 `as_i64`，字符串码绕过整个拦截分支。9-04 的 L-2 已在 `anysearch.rs:393` 用 `json_i64` 修过同款，这三处未修 |
| `providers/mod.rs:1028` vs `xiaomi.rs:141` | `instantiate_builtin_with_index` 接受 `"xiaomi"` 别名，但 `unique_id()` 写死 `"xiaomimimo"` → 凭据键分叉（`xiaomi#2` vs `xiaomimimo#2`），副本永久「未配置凭据」。正常 UI 走不到，属导入旧配置/手改 IPC |
| `providers/custom.rs:367-419` | NewApi preset 注释声称过滤负余额，实现只校验了 `divide`；`data.quota` 为负时 `remaining` 照样写进 `QuotaRow` → 浮窗显示 `-1.00 USD` + 托盘红点 |
| `providers/custom.rs:380-393` | 只认 `success`/`status_code` 信封，one-api 系的 `{"code":401,...}` 落到 Parse 错而非 AuthFailed |
| `providers/zhipu.rs:497-528` | `unit`/`nextResetTime` 只吃 `as_i64`（同函数里 `percentage` 走宽松的 `num_f64`）；字符串形态会让 5h/周两行对调——正是模块 doc 第 2 条明确警告的场景 |
| `providers/zhipu.rs:513-525` | 第二个同 `unit` 的额度包被静默丢弃，只有 `tracing::debug!` |
| `providers/stepfun.rs:328-340` | 注释写「并行拉 rate limit + plan status」实际串行；plan_status 只为拿可选的 `plan_name` 却给每次 fetch 加一个完整 RTT，401 兜底路径变 4 次串行 |
| `providers/kimi.rs:412-414` | 窗口行 `remaining` 只钳下界不钳上界；`remaining > limit` 时 `used` 归 0 → utilization 0% 但显示 "100/100" |
| `providers/kimi_desktop.rs:182-192` | cookie 查询 `ORDER BY last_access_utc DESC LIMIT 1` 跨 4 个 host_key；`www.kimi.com` 与 `.kimi.com` 可存不同账号的 token → 总套餐行在两个账号间跳变 |
| `providers/kimi.rs:296` + `kimi_desktop.rs:103-127` | 总套餐 enrich 每轮同步读 SQLite（`busy_timeout(250ms)`）且在 async fn 内直接执行，无 `spawn_blocking` → 阻塞 tokio worker |

### 托盘 / 平台 / lib
| 位置 | 问题 |
|---|---|
| `tray.rs:832-837` + `:876-879` | plan 过滤后无匹配行走 `(Empty, Empty) → return None`，**一行日志都没有**（唯一的 warn 只在 provider 本身没数据时打）→ 用户看到一个永远不变的 logo |
| `tray.rs:489-495` + `macos.rs:702-714` + `lib.rs:205-226` | `build_tray_menu` 在主线程上 `blocking_read()` 配置锁；同时若有 worker 持 config 读锁调 `tray_fill_color → menu_bar_is_light`（要往主线程派发并等 condvar 200ms）→ 闭包排在被 park 的主线程后 → 超时保守返 false → 浅色菜单栏上白字不可见。locale 切换时序恰好是「先 rebuild_tray 再 tray_fill_color」，命中概率不低 |
| `tray.rs:978-992` | `parse_hex_color` 采纳 8 位 hex 的 alpha 字节且无下限保护 → `#ffffff00` 让托盘图标**彻底全透明 = 用户以为 app 没启动**（正是 H7 fix 想消灭的症状）。4 位 `#RGBA` 分支读侧支持写侧拒绝，是死代码 |
| `tray.rs:1221-1229` | `draw_percent` 两行各调一次 `fit_scale` → 5h 行 100%（缩到 ~50%）而周行 5%（不缩）时同一枚图标里两行字号差近一倍 |
| `tray.rs:630-636` | `set_icon` 失败即 `return`，同批的 tooltip 被跳过 → 一次瞬时失败让 tooltip 冻结到下次成功为止，两个显示源长期漂移 |
| `commands/mod.rs:1394` + `lib.rs:283-337` | `build_floating_window` 用 `.visible(true)` 造窗，位置恢复发生在其后，而 `lib.rs:283` 的注释明确写着「必须在 show() 之前调用，否则会有 1 帧错位」→ 每次冷启动浮窗先闪在 tao 默认位置 |
| `config.rs:924-940` + `:1365-1387` + `extra_instances.rs:251-273` | 三条原子写路径都只 fsync 了 tmp 文件、没 fsync **父目录** → POSIX 上 `rename()` 只有在父目录 fsync 后才持久，掉电可能出现「新内容在、目录项不在」 |
| `config.rs:930-935` + `:1339-1349` | `save()` 的 0600 chmod 是 best-effort（`let _ =` 静默吞），且 tmp 已存在时 `mode(0o600)` 不生效；同场景 `write_keys_atomic` 是 hard error，两处不对称 |
| `config.rs:955-1194` | best-effort 逐字段挑取漏了新字段 `floating_fit_bottom_margin`（v0.2.9 刚加）→ 走 best-effort 时静默回落 80 且此后任何 save 都写死 |
| `config/extra_instances.rs:437-445` **+ `commands/mod.rs`** | `next_index_for` 的 `max().unwrap_or(1) + 1` 未用 `saturating_add` → 手改文件填 `instance_index = 4294967295` 时 release 回绕成 0（`minimax#0`）、debug panic。**D4 与 D5 独立命中同一处** |
| `lib.rs:117` + `extra_instances.rs:225-231` | `extra_instances.json` 损坏时 `unwrap_or_default()` 静默清空全部副本，用户此后任意写操作会覆盖损坏文件（备份仍在 .bak 但无提示）。keys.json 走的是相反且更严格的路 |
| `commands/i18n.rs:37-42` | `cfg.locale = locale` 在 `cfg.save()?` **之前**执行，save 失败时内存已脏而 `rust_i18n::set_locale` 没被调用 → `get_app_locale` 与实际 UI 语言分叉，后续任意成功 save 会把 "en" 落盘，重启后语言突变 |
| `poller.rs:267-270/370-375/447-448` | `backoff_snapshot` 在 loop body 开头克隆，本轮 spawn 的 fetch 才通过 `record()` 写新间隔，`next_fetch` 用**旧值**排期 → 退避翻倍永远滞后一拍（自愈，错误卡上的倒计时与真实调度差一个周期） |

### 前端
| 位置 | 问题 |
|---|---|
| `logos.ts:125-137` | 副本（`minimax#2`）永远显示字母头像而非真 logo：`logos.ts:110-121` 已写好剥离 `#N` 的 `getProviderMeta`，但**全项目零调用方**；实际被调的 `getProviderDisplay` 直接 `_providerMeta[id]` 查表。浮窗 `main.ts:945` 反而是对的 → 同一实例浮窗真 logo、设置面板灰头像 |
| `main.ts:1349-1364` | Kimi「总套餐」拆分小字节点一旦补建就永不移除，`kimi_code_used_ratio` 消失后显示假的「Code 0%」 |
| `credentials.ts:622-640` | `t()` 找不到 key 时返回 key 本身（不是 null），所以 `?? id` 永不触发 → flash 出现字面量 `provider.xxx.name`。`providers.ts:78` 做了显式 key 比对（`t(k) === k`），此处漏 |
| `providers.ts:145-146` + `:431-436` | 删除/添加来源后重渲染整个 section，搜索词被静默清空（新建的 input 不恢复 value），长列表瞬间铺满、滚动位置也丢 |
| `main.ts:644-651` + `:744-784` | `applyFitMargin` 置 `lastFitContentH = -1` 后**只调一次** `fitOnObserverTick()`，而该函数有 `<800ms 用户刚拖过` 与 `observerBusy` 两个静默早退；并发那条路上在飞的 tick 会把 `-1` 抹掉 → 改 margin 后浮窗高度纹丝不动，直到下次内容变化 |

---

## 四、本轮确认「已修好、无回归」的点（复核 9-04 / 8-17 条目）

这轮 agent 逐条复核了上一轮的修复，以下确认闭合：

- **D7**：9-04 的 H-4（锁外快照回滚）/ H-5（Resized 把 `(0,0)` 播种）/ H-6（`menu_bar_is_light` 非主线程恒 None）/ M17（geom flush 丢通知）**全部真修**——`GeomLatest { pos: Option, size: Option }` 分维度跟踪、域内需要主线程的 6 个调用全走 dispatch 无漏网。
- **D7**：Win `apply_z_order` 的 T1/M18 修复在位（RMW 全局串行化 + 返值检查 + 代际计数在采纳前复查）；托盘跨线程 SIGTRAP 的前提已变（tauri-2.11.5 无 `impl Drop for TrayIcon`）。
- **D4**：9-04 的 H-02（`is_enabled_unique` 语义）**未回归**，poller/`refresh_inner`/`get_snapshot`/`refresh_single_inner`/两处 publish retain 全部走同一个两级 fallback helper。**AGENTS.md v0.3 待做的「per-provider poller task shutdown」其实已落地**（`SHUTDOWN` Notify + `abort_all()` + drain），该 v0.3 条目已过时可划掉。
- **D1**：SSRF 三层防护扎实（URL 字面 → redirect 每跳复查 → DNS 解析层），`[::1]`/`::ffff:127.x`/十六进制 IPv4/`localhost.` 尾点/DNS 重绑定均被堵住；14 个 provider 全部走 `json_body_limited`，无裸 `resp.json()`；`refresh_inner` 与 `refresh_single_inner` 都先 `update_source_state(&src_box)` 再 fetch 同一 Box，`source-instance-rebuild-footgun` 已根治。
- **D2**：9-04 的 M2/M4/M5/M5.1/L-1/L-5/H-Provider 全部修复到位无回归；anysearch 的 `combined` vs `refresh` 锁内重读语义正确。
- **D3**：volcengine 双套餐**并发无竞态**（state 在 async block 前 Copy 成局部值，两 future 零共享可变状态）；「失败不连坐」判定隔离正确（三种订阅组合推演均对）；`c97ce3b` 的 Percent 语义回滚彻底无残留相反假设；HMAC 签名无重放/溢出风险。
- **D5**：**前端 59 个 invoke ↔ 后端 68 个 handler 全量对账，0 不匹配**（历史上出过 `set_xiaomi_region` 那种前后端命令名不一致导致功能静默失效）；17 个 `musage://*` 事件前后端拼写完全对齐；4 个 `#[derive(Deserialize)]` DTO 的 camelCase 契约两侧一致；`set_floating_fit_bottom_margin` 双端 clamp + NaN 防护齐全。
- **D6**：`b8cb37d` 的 init-script 跨域早返位置正确（早返发生在 prototype override **之前**且目标域内不触发）；`9c51917` 两个方案都真修好；四个模块全有 wall-clock deadline + MAX_ITERS 双保险；凭据槽位无重叠；四个 capability 文件齐全且未开 `dangerousRemoteDomainIpcAccess`。
- **D8**：**`a7e2662` 的 DOM diff 修复彻底**（卡片层/行层都是多值 Map + orphan 清理，各种场景推演均收敛，无需刷新）；全项目 31 处 innerHTML **无用户可控字符串进入、无 XSS**；全项目 `confirm/prompt/alert` **零实际调用**（守门脚本覆盖正确）；i18n 前后端 key 集合完全一致；测高已全走 `offsetTop`/`offsetHeight`，合成层漂移未复发；事件监听/timer 无泄漏。

---

## 五、系统性簇（10 簇，62 条独立问题的来源）

| 簇 | 原始条数 | 合并后 | 根因 | 涉及域 |
|---|---|---|---|---|
| 1. i18n 缺 key | 2 | 1 | `t!()` 引用的 key 没进 locale 文件，rust-i18n 静默返 key 路径 | D1, D6 |
| 2. instance index 没串到底 | 3 | 1 | 9-07 的 D7-02 只改了**写入侧**，读取/清除/setter 刷新侧全漏 | D6, D2, D3 |
| 3. 行语义不区分 | 4 | 1 | 对任意 `QuotaRow` 取 `remaining`（wallet 阈值 / 余额判定 / plan 过滤各自一套） | D1, D2, D5, D7 |
| 4. 修复未平行移植 | 4 | 1 | 改了 A 函数忘了同款 B 函数 | D1, D2, D3 |
| 5. **修复是空操作** | 2 | 1 | 代码改了但不产生效果 | D8 |
| 6. `save_config` 回滚竞态 | 2 | 1 | 前端只串行「写」不串行「读」+ 后端全量替换 | D5, D8 |
| 7. hex 颜色三侧口径不一 | 3 | 1 | 前端 6 位 / 写侧 `3\|6\|8` / 读侧 `3\|4\|6\|8` | D5, D7, D8 |
| 8. `next_index_for` u32 溢出 | 2 | 1 | `max()+1` 未 saturating | D4, D5 |
| 9. `format_balance_tray` 取整 | 2 | 1 | 图标取整、tooltip 保留 2 位，符号从取整后取 | D3, D7 |
| 10. `":agent"` 不校验配对 | 2 | 1 | 写侧只白名单 suffix，消费侧余额分支又不过 plan 过滤 | D5, D7 |

### 两个值得单独强调的元模式

**（A）「修复是空操作」——比未修的更危险**

9-04 报告的 M33 / M30 / M32 都标为已修，实际改的东西不做功：
- M33：`const previous = select.value` 写在 `change` 回调**内部**，此时控件值已被用户改掉，`previous === v`，回滚赋回它本来就有的值；
- M30：`saveConfigSerialized` 的队列里只有「写」，`getConfig()` 的快照在队列外读到，队列只保证「B 在 A 之后写」，B 的快照不含 A 的改动；
- M32：`select.value = currentSource` 在无匹配 option 时把 `selectedIndex` 置 -1，回滚把下拉变空白。

这类 bug 的共同特征：**报告说已修、CI 绿、代码里有看起来正确的逻辑，但功能是坏的**。纯断言式单测抓不到（空操作回滚在「成功路径」断言下会通过），必须断言**失败路径**上控件值确实变了回去。

**（B）「修复未平行移植」——人工审查的结构性盲区**

4 条同一形态：
- 9-04 用 `json_i64` 修了 anysearch 业务码 → 漏 xiaomi 两处 + siliconflow 一处；
- 9-04 用 `config_error` 修了 custom.rs → 漏 zenmux 三处；
- `595a9b1` 的 H-Provider 双字段守卫加在 `build_window_row` → 漏同文件的 `parse_total_quota`；
- volcengine 的 `"Error": null` 守卫在 `openrouter` 有 → 本文件漏。

---

## 六、CI 守门建议（针对上面的元模式）

这 6 条守门脚本能把「靠人工记忆逐处同步」变成机器保证：

1. **i18n key 完整性**（挡簇 1）
   扫全部 `t!("x.y.z")` 字面量（Rust `t!()` + TS `t("...")`），对 `src-tauri/locales/{en,zh-CN}.json` 和 `src/i18n/{en,zh-CN}.json` 各做一次存在性断言。注意排除注释里的命中。

2. **失败回滚有效性**（挡元模式 A）
   对每条「失败回滚」逻辑配一条单测，**断言 IPC 失败路径上控件值确实被还原**。现有的 `order.test.ts` 只覆盖纯函数（`computeInsertIndex` / `canonicalizeOrder` 等），回滚类逻辑零覆盖。

3. **`type="color"` 禁用扫描**（挡 H-2）
   在 `check-no-native-dialogs.sh` 旁边加一条：禁止 `<input type="color">`（WKWebView 死控件），颜色一律走纯 DOM 色板 + hex 文本框。memory `wkwebview-color-input-change-footgun` 已写死这条约定，但 enforcement 只做了一半。

4. **hex 长度口径单点化**（挡簇 7）
   写一个共享的 `parse_tray_color` 前端模块（或从后端经 IPC 暴露一次校验），让「前端校验 / 写侧校验 / 读侧解析」三处共用同一个长度集合。当前是三份各自维护的常量。

5. **姊妹路径同步检查**（挡元模式 B）
   对同款解析 helper（`json_i64` / `validate_bearer_key` / `config_error` / `is_null` 守卫）grep 所有 provider 文件，列出**未接入**的文件并在 CI 报警而非静默通过。9-04 的 L-6 注释写着「各 provider 在拼 Authorization 头前调用」，实际 14 个里只有 4 个接了。

6. **instance id 贯通断言**（挡簇 2）
   对 `clear_*` / `open_*_login_window` / provider setter 三类命令，断言它们接受的 id 形态与写入侧一致（能处理 `{base}#{N}`）。当前 D7-02 的半修就是这条缺失导致的。

---

## 七、修复批次建议

| 批次 | 内容 | 条数 | 理由 |
|---|---|---|---|
| **第一批** | H-4（BOM 一行）、H-9（2 个 i18n key）、M-4（`auth==` typo + 单测）、簇 5 的回滚空操作（M-22/M-25） | ~8 | 都是**一处一行**的低风险修复，且每条都堵死一整类用户可见故障。H-4 尤其：坏了之后用户连自救路径都没有 |
| **第二批** | H-1 / H-2（两个 macOS 死控件）、H-3（拖拽兜底）、H-6（unique_id 重命名）、M-16/M-17/M-19/M-20（托盘四处口径） | ~8 | 都是**用户直接看见的显示错误**，修完体感提升最大。M-19 顺带把 D5+D7 两侧一起收敛 |
| **第三批** | H-7 / H-8（登录 instance id 贯通，需前后端一起改） | 2 | 改动面最大（4 个登录命令 + 前端 relogin/clear 按钮 + provider setter），但 H-7 是死循环、H-8 是凭据无法撤销，都不能拖 |
| **第四批** | H-5（single-instance plugin）、M-1/M-3（save_config 语义根改） | 3 | 需要架构决策（全量替换改字段级 merge / 加依赖），适合排 v0.3 |
| **第五批** | 其余 M/L | ~40 | 按模块择机；其中 M-5~M-15（provider 解析类）建议**同一个 commit 批量修**，它们多数是 schema 漂移防护，正好一起补单测锁住 |
| **并行** | 第六节的 6 条 CI 守门 | — | **建议在第一批之前就落地 1/2/3 条**，否则第一批修完，同款问题会在下次改动里原样复发 |

### 一句话结论

代码质量比上一轮明显提升（无 Critical、SSRF/凭据/持久化三条安全边界都很扎实），**9 条 High 全部是流程性缺陷而非算法错误**——集中在一类根因：某个 ID、某个校验、某个监听器没有在所有姊妹路径上平行落地。比起逐条修，第六节的 6 条 CI 守门更值得优先做，否则下一轮还会长出同款。
