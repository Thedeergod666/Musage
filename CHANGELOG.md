# Changelog

All notable changes to Musage will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed（2026-07-30 全量审计批量修复）

基于 `audit-reports/2026-07-30-full/` 8 域并行审查（01 providers-A / 04 config-ipc / 05 poller-lifecycle 上一轮已完成，本轮基于剩余 04 报告 + 上轮已审未修条目做的 9 个 commit），按 P0→P3 顺序原子修复：

- **D-003 + D-008**：OpenRouter `display_name()` 硬编码 "OpenRouter" 字符串 → 改走 `t!("provider_name.openrouter")` + i18n key，多实例副本同款（`providers/openrouter.rs`）；`fetch_credits` 5xx 分支不带 body → 对齐 `fetch_key` 加 `text_body_limited` 200 字符截断的 body preview。
- **D4-001**：损坏 `config.json` 三段 fallback 全失败时 `std::fs::copy` 留原文件 → `std::fs::rename` 原子移动到 `.bak.<ts>`，后续 `save()` 不会再覆盖损坏版本；rename 失败兜底回 copy + ERROR 级日志。
- **D4-003**：`add_extra_instance` 锁外用 read 锁算 tentative idx 写 temp key 并发会撞 → 把"算 idx + save key + push instance + save extras"全部放进一把 write 锁，删 `try_rename_key` helper（dead code）。
- **D4-004**：`delete_extra_instance` H13 fix 只回滚 extras (内存) 不回滚 keys.json (磁盘) → 跟踪 `migrations_done + target_cred_backup`，save extras 失败时反向 LIFO 操作恢复 keys.json 状态。
- **D4-005**：`read_keys` 两处 `let _ = std::fs::copy(...)` 静默吞 backup 错误 → 改为 `if let Err(e)` + ERROR 级日志（empty 分支保留 warn 成功路径）。
- **D4-006**：`delete_extra_instance` gap-filling bug —— 删 deepseek#2 时 compact 把 deepseek#3 迁移到 deepseek#2，紧接着 `delete_credential_for_id("deepseek#2")` 误删刚迁移的凭据 → 删前确认 compact 之后 `target_api_key_ref` 是否已被其他 instance 占用，是则跳过删除。
- **D4-007**：`truncate_old_backups` 启动只清 `config.json.bak.*`，半年升级用户堆 keys.json.bak 撑爆磁盘 → 加 `truncate_old_backups(parent, "keys.json", 5)`。
- **D4-008**：`save_config` 不校验浮窗坐标 → 加 bounds 检查：xy 在 ±100k，wh 在 50~4000，越界返 Err 含字段名 + 影响说明。
- **D4-009**：`save_config` 不限 `provider_order` / `schema_overrides` 数量 → 加 `ORDER_LIST_MAX = 256` / `SCHEMA_OVERRIDES_MAX = 256`，挡 IPC DoS。
- **D5-007**：`refresh_inner` 12 次顺序 `backoff.write().await` → 三段式（Phase 1 收集 → Phase 2 单次写锁 record → Phase 3 多次读锁 fill_next_fetch_at）。
- **D5-038**：`refresh_inner` + `refresh_single_inner` 重复"填 source_display_name + emit snapshot + 刷新托盘" → 抽 `publish_snapshot` helper 共用。
- **D5-074**：长暂停（macOS sleep / 长时间不交互）唤醒时 12 个 `next_fetch` entry 全过期 → 同步连续 spawn 12 个 task，12 个并发 HTTP 同时打出去。spawn 前 `await jitter_for(...)` 让本次 fire 也散开，cap 到 interval 的 10%。
- **D5-101**：`spawn_debounced_geom_persister` 500ms 循环永久跑、不监听 shutdown，用户拖完最后 100ms 内 quit → tokio runtime drop 取消 task → latest 几何值丢失。select 加 `poller::SHUTDOWN.notified()` 分支，收到 signal 最后 flush 一次后 break。

新增单测：`openrouter::tests::display_name_uses_i18n_key`。

守门：`cargo test --lib` 354 passed (含 1 个新增)，`pnpm tsc --noEmit` 0 error。

## [0.2.5] - 2026-07-29

### Fixed (Critical — StepFun 一键登录永远抓不到 token)

- **症状**：点「🔑 登录 StepFun」→ webview 里登录成功、页面停在
  platform.stepfun.com 首页，但 token 始终没写进 keys.json，浮窗继续
  报「StepFun 登录凭据已过期」，登录窗口也不会自动关闭
- **根因**（4 个叠加 bug，详见 [stepfun_login.rs](src-tauri/src/stepfun_login.rs) 文件头）：
  1. `cookies_for_url` 拿 `account.stepfun.com` 的 LOGIN_URL 探测，而
     `Oasis-Token` 落在 **platform.stepfun.com** 域 —— 按域过滤下
     100% 拿不到（xiaomi 没踩这坑是因为 LOGIN_URL 和 cookie 同域）
  2. init script 在 platform 域 document_start 无差别清 `Oasis-Token`，
     登录后跳回 platform 的首次页面加载把刚 Set-Cookie 的新 token
     立刻删掉（非 HttpOnly 时）
  3. init script 的 `Storage.getItem` 锁在非 platform 域一律返 null，
     可能破坏 account 域登录 SPA 的 OIDC state 读取
  4. 开窗后 200ms `clear_all_browsing_data` 与 SSO 秒跳竞态（可能清掉
     刚落定的新 token），同时杀死「SSO 自动重登」零交互路径
- **修法**（对齐 anysearch_login.rs 的成熟模式）：
  - 删 init script / MUSAGE_READY 握手 / clear_all_browsing_data /
    `on_page_load` 依赖，改独立轮询任务（700ms × 1200 ≈ 14 min）直接读
    `cookies_for_url("https://platform.stepfun.com/")`
  - **JWT exp 新鲜度门**替代握手：旧会话残留 token 必已过期（用户正是
    因此才重登）→ 拒绝并继续等；新登录 token exp 在未来 → 接受。复用
    provider 侧 `access_token_exp_seconds_ago`（改 `pub(crate)`），
    「登录存盘」和「provider 预检」判定单一来源
  - 保留 webview profile 的 SSO session → token 过期后重登可走
    「点按钮 → SSO 秒跳 → 抓新 token → 自动关窗」零交互路径
  - `Oasis-Token` 单段且存在 `Oasis-Refresh-Token` cookie 时拼
    `<access>...<refresh>`（refresh 半段带 `device_id` claim，是
    provider 组 `Oasis-Webid` 请求头的来源）
  - 新增 8 个单元测试（exp 门 / combined 拼接 / probe URL 域回归防御）；
    locale `stepfun_login.*` 瘦身为 3 key
- **第二只 bug（真实账号二轮实测）**：登录已显示「提取 + 保存成功」，
  浮窗却继续报「登录凭据已过期 N 分钟」（N 随时间涨 = 一直在用旧 token）。
  根因：v0.2.4 手动粘贴时代在 keys.json 留了 `stepfun` api_key 槽位，
  `save_credential_for_id` 对 None 字段是**跳过而非删除**（不清 legacy
  槽），而 `stepfun.rs` fetch 是 `api_key.or(cookie)` **api_key 优先** ——
  新登录写入 `stepfun:cookie` 的新鲜 token 被永远忽略。修法：fetch 改
  **cookie 优先**（对齐 anysearch / claude_official 的 Cookie-kind 约定），
  api_key 仅作 legacy 兜底

### Fixed (Critical — StepFun 用量响应被误判为 code -1 + access 30 分钟过期无续期)

- **症状**：第三只 bug。登录成功、token 新鲜，浮窗却报
  「StepFun API error (code -1): 」（空 message）
- **根因**（对照 CodexBar `StepFunUsageFetcher.swift` 源码 + 本机探针实测）：
  1. **成功标记搞错**：现行成功响应是 `{"status": 1, 用量字段在顶层}`，
     不是旧实现的 `{code: 0, data: {...}}`。旧代码 `code` 缺失 → 默认 -1 →
     把**真实的成功用量数据**误判为业务错误
  2. **access 半段仅 ~30 分钟寿命**（探针实测 exp-iat≈1800s），refresh
     半段 ~30 天带 `device_id` claim —— 浏览器靠 `RefreshToken` 端点持续
     续期，旧实现没有续期流程，登录 30 分钟后必掉线
  3. 失败路径不带响应体，「code -1 + 空 message」等于盲猜
- **修法**（[stepfun.rs](src-tauri/src/providers/stepfun.rs) 整体重写）：
  - **双 schema 兼容解析**：`status == 1` 成功标记（字段顶层）+ 旧
    `code == 0`（字段在 `data` 下）；数字字段 int/float/字符串通吃
    （CodexBar `FlexibleNumber` 同款）；时间戳吃 ISO 8601 / epoch 秒 /
    epoch 毫秒 / 数字字符串，`"0"` = 「无窗口配置」不再渲成 1970
  - **RefreshToken 续期流**（对齐 anysearch 模式）：access 过期或 120s
    SKEW 内且有 refresh 半段 → 调
    `POST …/PassportService/RefreshToken`（CodexBar 逆向：body `{}` +
    裸 `Oasis-Token` header + Cookie）换新 pair 并**原子写回
    keys.json**；服务端没返新 refresh 半段时保留旧的；请求仍 401 /
    业务层 auth 失败时兜底 refresh 一次再重试
  - **业务错误 auth 感知**：失败消息含 "auth failed" / "unauth" /
    "embezzled" / "illegal" → AuthFailed kind（让续期兜底接管）
  - 失败路径全加 `[diag] stepfun ...` 响应体日志（截断 500-800 字符）
  - 探针实锤的鉴权细节写进文件头：token 必须发 `<access>...<refresh>`
    **整段 pair**（裸 access → `401 token is illegal`）；`oasis-webid`
    必须等于 refresh 半段的 `device_id`（不匹配 → `401 embezzled`，
    CodexBar 源码注释同款结论）
  - 新增 18 个单元测试（新 schema / ensure_success 判定 / flex 数字 /
    epoch 字符串时间戳 / refresh 响应解析），`cargo test --lib` 296 passed
- **⚠ 已知未解**：本机 node 探针 replay 同一 token 被风控拒（401
  embezzled/illegal），但 app（reqwest/rustls）拿到过 200 —— 疑似对
  非浏览器 TLS 指纹有风控但 reqwest 目前放行。若后续 reqwest 也被风控，
  备选方案是 usage 请求改在登录 webview 内发（JS fetch 走浏览器 TLS +
  真实 cookie），按 `[diag]` 日志再定

### Added (StepFun credit 套餐到期/重置时间)

- 套餐用量行（credit 套餐）新增**到期/重置时间**显示（`musage dump`
  真实响应实测字段）：
  `subscription_credit_reset_time > 0` → 订阅制周期重置（「套餐用量重置」）；
  否则 bucket 最早 `next_reset_at` → 重置；否则 bucket 最早 `expire_at`
  → 一次性额度包**到期**（`extra.reset_period="expire"`，浮窗显示
  「到期 8-4（7天xh）」，过期后显示「已到期」）
- 前端：`resetsPrefixFor` helper 抽出共用（used/total 行行为
  byte-for-byte 不变），utilization-only 行也支持 `resetsPrefix`；
  `formatResetWithCountdown` 加 doneLabel 参数；新 i18n key
  `floating.countdown.expire_prefix` / `expire_done`（中英）
- `credit_plan_reset` 4 个单测（含真实响应 fixture），
  `cargo test --lib` 300 passed
- credit 行标签「额度」改「**套餐用量**」（`row.credit` value 变更，
  key 名不动；英文 "Credit" → "Plan usage"）

### Fixed (Critical — macOS 设置面板所有确认操作静默失效)

- **extra instance × 删除按钮点了没反应（含全局 7 处同类 bug）**
  - **症状**：设置面板给新建的副本（如 DeepSeek #3）点 × 删除按钮，
    不弹窗、不报错、不删除，完全没反应。同根因还影响：删除各 provider
    API key（3 处）、清除小米 / AnySearch / StepFun 登录态（3 处）、
    清空日志（1 处）—— macOS 上全部点了没反应
  - **根因**：删除流程第一道门是 native `confirm()`，但 macOS WKWebView
    （tauri 2.11 / wry 0.55）的 UIDelegate **没实现** JS dialog panel 方法
    （`runJavaScriptConfirmPanel` / `TextInputPanel` / `AlertPanel`），
    `confirm()` 不弹窗、同步返回 `false` → handler 第一行就 return。
    Windows WebView2 原生支持这三个对话框，所以 Win 端一直正常、
    从未暴露。上一版修复（`ae3818a`）只把 `prompt()` 那道门换成
    in-app modal、留了 `confirm()` 守门，流程仍死在第一行
  - **修法**：
    - 删除 extra instance：`confirm()` + 输全名两道门合并成**一个**
      in-app `<dialog>`（警告文案 + 输入完整名称，输全名本身就是强确认）；
      删掉恒返回 `null` 的 native `prompt()` fallback 死代码
    - `modal.ts` 新增 `confirmInApp(message): Promise<boolean>` helper，
      全局替换剩余 7 处 `confirm()`（credentials.ts × 6 / logs.ts × 1）；
      `showModal` 改为返回 `HTMLDialogElement`（取消/ESC 兜底 resolve
      不再靠脆弱的 `querySelector("dialog.modal:last-of-type")`）
    - 文案里的 `\n` 用新增的 `.modal-message { white-space: pre-line }`
      原样换行渲染
  - **防回归**：新增 `scripts/check-no-native-dialogs.sh`（grep 禁
    `confirm(`/`prompt(`/`alert(` 进 `src/`，排除注释行），接入
    `ci.yml` frontend job 第一步 fail-fast

### Fixed (Visual — 玻璃"时不时闪一下")

- **WKWebView backdrop sample throttling：移植 Usticky 的 3 层防御**
  - **症状**：浮窗玻璃静止 ~2s 后"褪色"成清水玻璃，下一次真实 paint
    （倒计时分钟翻转 / poller 刷新 / hover 切换）采样恢复又闪回来；
    PinBottom hover-raise 切 `NSWindow.setLevel` 时闪得更明显
  - **根因**：macOS WKWebView 对非 key / 透明 / backdrop-filter 窗口有
    合成层节流 —— 静止 ~2s 丢 CABackdropLayer 采样；level 切换时采样
    因 z-order 变化失效，~2s 后才重算。Musage 此前零防御（每秒
    `updateCountdowns` 写的是分钟/天粒度的相同字符串，不产生 paint，
    压不住 ~2s 节流窗口，不算心跳）
  - **修法**（移植自 Usticky styles.css 文件头同名方案，两边实现一致）：
    - L1：`.card` / `.err` 加 `will-change: backdrop-filter`（独立 GPU
      layer，不进空闲节流判定）
    - L2：`@keyframes musage-backdrop-heartbeat`（0.001° rotate +
      0.001 opacity，4s linear 无限循环）强制每帧 paint invalidation，
      亚像素振幅肉眼不可见
    - L3：`set_window_level` 切 level 后 emit
      `musage://backdrop-refresh`（[macos.rs](src-tauri/src/platform/macos.rs)，
      emit 放在 setLevel **之后**同一 main-thread dispatch 任务内，
      否则 reflow 击穿的是旧 z-order 的 sample 窗口），前端
      [main.ts](src/main.ts) listener 给 `.card` / `.err` 加 100ms
      `.force-reflow`（`filter: drop-shadow(0 0 0 transparent)`）立刻重采
  - **配套决策**：blur 28px / saturate 180% **写死**，idle 不再切
    10px/140%（backdrop-filter 插值过渡既是 throttling 触发路径，也是
    合成层重操作）；`.card` / `.err` 的 transition 移除 backdrop-filter
    项。玻璃 fold/unfold 只靠 tile-bg alpha / border / shadow 纯 paint 切换

### Changed (Visual)

- **idle 玻璃参数向 Usticky 对齐，两 app 同屏观感统一**：idle
  `--tile-blur` 10px → 28px、`--tile-saturate` 140% → 180%、`--tile-bg`
  rgb 22,24,30 → 28,30,38（alpha 0.30 不变）。此前同屏时 Musage 瓦片
  比 Usticky 偏灰偏"实"，主因就是 saturate/blur 参数差

### Added

- **StepFun 一键登录**（2026-07-27，v0.2.5 候选）：
  - **砍掉「粘 Oasis-Token」入口**：仿 Xiaomi/AnySearch 模式，settings
    面板 stepfun 行改 anysearch 同款 `quick-login-banner` 形态 —
    「🔑 登录 StepFun」按钮 + 「清除」按钮 + 官网链接。点按钮弹
    webview 加载 `platform.stepfun.com`，用户在 webview 里正常走
    邮箱+密码登录，后端自动从 webview cookie jar 抽 `Oasis-Token`
    写进 `keys.json` 的 `stepfun:cookie` 槽位。全程不需要碰 DevTools
  - **新增 `src-tauri/src/stepfun_login.rs`**（仿 `xiaomi_login.rs`
    同款 `EXTRACTING` 锁 + `ExtractingGuard` RAII + `DONE` 标记 +
    5 次重试 11s 覆盖）。新增 `capabilities/stepfun-login.json`
    capability 拆分（仅 `stepfun-login` 窗口拿
    `core:webview:allow-create-webview-window`）
  - **3 个关键 fix**（避免 11 个 peer 的踩过的坑在 stepfun 重演）：
    - `stepfun.rs` 的 `auth_kind` 改 `AuthKind::Cookie`（fetch 端
      走 `credentials.cookie.as_deref()` 路径，token 落 cookie
      槽位，settings 面板走纯 cookie 模式 + banner 路径）
    - init script `document_start` 清旧 `Oasis-Token` /
      `Oasis-Webid` / `Oasis-Refresh-Token` cookie + 置
      `MUSAGE_READY=1` 握手标记（仿 [anysearch_login.rs](src-tauri/src/anysearch_login.rs)，
      挡 webview profile 持久化过期 cookie 导致「弹出即消失」bug）
    - hardening 锁 `document.cookie` / `Storage.getItem` 到
      `platform.stepfun.com`，挡第三方 tracker 在受信 webview 上
      偷 token
  - **浮窗错误卡加 `relogin-stepfun` 分支**（仿 `relogin-anysearch`），
    点按钮调 `open_stepfun_login_window` 重新登录
  - **删 i18n**：`error.stepfun.token_unconfigured_hint` /
    `token_invalid_hint` / `token_expired_hint`（用户不再需要这 3
    条 — 跟「粘 token」入口一起删了）。`help.stepfun` 重写，删
    DevTools 教程段。`BATCH_PREFIX_RULES` 删 `Oasis-Token` 防误识别
  - **6 个新单测**：`is_dashboard_url` 基本 URL 接受 / 拒 login
    路径 / 拒无关 host / 拒非 https / 白名单非空 / window label
    匹配 capability
  - **风险**：webview 路径未 spike 验证（缺真实 StepFun 账号），
    CodexBar 选了 API 路径正是因为 webview 不可控。**降级方案**：
    保留 webview 框架 + capability，settings 面板改成账号密码
    内联表单（CodexBar 3 步 API POST：INGRESSCOOKIE → RegisterDevice
    → SignInByPassword；device_id 即回显的 oasis-webid，生成一个
    持久化用就行）

- **StepFun 集成重写**（commit `0d51124`，2026-07-21）：
  - **端点迁移 `platform.stepfun.com`**（`QueryStepPlanRateLimit` +
    `GetStepPlanStatus`），旧 stepfun.ai 提示文案全部同步
  - **Oasis-Webid 自动提取**：dashboard 端点要求 `oasis-webid` 请求头
    等于 token refresh half 的 JWT `device_id` claim（CodexBar 逆向），
    缺 webid 一律 401 且响应不区分「token 无效」与「Webid 缺失」。
    新增 `base64` 依赖（`URL_SAFE_NO_PAD`，JWT 标准）本地解 payload；
    解不出时兜底 `DEFAULT_WEBID` 并 warn
  - **token 本地预检**：JWT 过期 → `token_expired_hint`（带过期分钟数）；
    payload 无法识别 → `token_malformed_hint`。发请求前就给出可操作
    提示，不再让用户拿 401 猜原因
  - 支持整段 `Cookie: Oasis-Token=...; ...` 粘贴，程序自动剥离
  - **credit 套餐（plan_family=2，Mini/Pro）解析**：优先
    `subscription_credit_left_rate`，缺则 `topup_credit_left_rate`，
    再缺 `credit_buckets` 加权平均；单行「额度」渲染（新 i18n key
    `row.credit`）

### Fixed (Windows — PinBottom hover-raise)

- **hover 进浮窗可见区不临时置顶**（commit `ff309bb`，2026-07-20，
  v0.2.4 用户反馈）：
  - **根因**（修正 2026-06-12 的"OS 持续 demote"误判）：`WS_EX_TOPMOST`
    是 sticky 的，OS 不会自发清掉。真正机制是**自己撤销自己** —— 旧
    tracker 无防抖，`WindowFromPoint` 单 tick 抖动（DWM 重绘 / WebView2
    瞬态子窗口 / 光标压 rect 边界）返一拍 false，旧代码立刻 edge-drop
    塞回 `HWND_BOTTOM`，raise 后 50~100ms 内被自己的 drop 撤销（也是
    历史 3/7 命中率的根因）
  - **修法**：dwell-time hysteresis + edge-trigger —— `Visible` 1 tick
    即抬（v0.1.0 一档响应）/ `Covered` 连续 5 tick（250ms）dwell 才抬
    / `Outside` 3 tick（150ms）落回 `HWND_BOTTOM`；raise/drop 只在采纳
    的状态切换时各做一次，稳定 hover 期间 1s 低频 re-assert 一次兜底，
    不再 20Hz 反复 `SetWindowPos`
  - **两级命中语义**：Win 用户真实场景是浮窗长期被最大化窗口盖住大半、
    只露一条边，严格"未被遮挡才算"（macOS-parity）下鼠标几乎永远落在
    被盖区域，hover-raise 等于不存在。`Covered` 250ms dwell 挡住路过式
    误触发（鼠标横穿通常 <100ms），抬起走 `SWP_NOACTIVATE` 不抢焦点。
    macOS 端仍是严格"未被遮挡"语义，两平台**有意分歧**
  - **逃生口保留**：tray 菜单「强制置顶浮窗」走
    `AllowSetForegroundWindow(ASFW_ANY) + SetForegroundWindow`（会抢
    焦点，但用户主动触发）
  - 详见 [AGENTS.md](AGENTS.md)「Win 端 PinBottom 模式 hover-raise
    修复（2026-07-20 根因定位 + 重写）」节

### Fixed (Settings — extra instance 删除 UX)

- **点击 × 删除副本"无效"修复**（[providers.ts:renderDeleteExtraButton](src/settings/providers.ts)）
  - **症状**：用户报告"新建 deepseek #3 后点 × 删除按钮无效"。检查
    后端 `delete_extra_instance` IPC 流程、UUID 序列化、
    `compact_indexes_for` / keys.json 同步、`save_lock` 顺序均正确
  - **真正根因（macOS WKWebView native prompt UX）**：旧代码用
    `prompt()` 收二次确认，要求 `input === display_name` 严格相等。
    但 native prompt 在 macOS WKWebView 上有两个**已知怪行为**导致
    用户感知"删除失败"：
    1. `confirm()` 之后立刻 `prompt()`，Z-order / focus 残余影响
       可能让 prompt 一闪而过或被遮挡，用户下意识点 Cancel
       → `input === undefined` → flash "名称不匹配，未删除"
    2. 用户复制 `DeepSeek #3` 粘贴时，剪贴板字符串尾随不可见字符
       进 `prompt()` 自带"选中并替换"机制 → `input.trim()` 跟
       `meta.display_name` 比仍不等 → flash mismatch
  - **修法**（in-app modal + 宽松匹配）：
    - 二次确认替换为 in-app `<dialog>` modal（[src/settings/modal.ts](src/settings/modal.ts)）
      + `<input class="delete-confirm-input">`，自动 focus+select 全选
      默认值；用户可一键覆盖或粘贴新值
    - 名称比较三级 fallback：严格相等 → case-insensitive 全 whitespace
      折叠后相等 → 仅匹配 `#N` 后缀（`#3` 直接删深拷贝）；不卡空白 /
      大小写 / 前后空白
    - 加 console.info / console.warn / console.error 调试日志
      （不上 UI），方便用户反馈问题时抓 dev 日志一键诊断
    - 极端 fallback：in-app modal 抛错时回退到 native `prompt()`

### Fixed (2026-07-28 全量审查接力 + 收口)

承接另一个 CC agent 留下的 22 文件 / 78 条 catalog WIP，完成 4 个
后端 commit + 1 个 docs commit。完整 false-positive 拒绝证据见
[docs/codeplan/2026-07-28-wip-catalog-completion.md](docs/codeplan/2026-07-28-wip-catalog-completion.md)。

- **fix(i18n)**：补 `commands.schema_override_duplicate_field`（rust）
  + `settings.advanced.io_export_failed`（frontend）4 条缺失 key。
  之前 `t!` / `t()` 引用编译期不报错（rust_i18n / 自写 helper 都返
  raw key），用户触发对应分支时看到的会是原始 key 字符串。
- **fix(login)**：把 `is_current_gen` + `WindowCloseGuard` 真正接到
  xiaomi / anysearch / stepfun 三个 login 模块的 poll 循环和 emit
  路径。WIP 中这两个机制只定义未使用（doc-only fix），老任务在用户
  重复点登录时实际不会自检也不会 panic 兜底关窗。
- **fix(providers)**：minimax 3 个错误返回路径（base_resp 业务错误 /
  model_remains 缺失 / parse 全失败）改用调用方 `source_id` /
  `display_name` 参数，跟成功路径对齐。副本 (`minimax#2`) fetch 失败
  时不再错误地落 base id 覆盖基础实例的 snapshot。
- **fix(providers)**：`enforce_body_limit` / `read_body_limited` /
  `json_body_limited` / `text_body_limited` 一组 8 MiB body 上限 helper
  之前 0 call site，stepfun × 2 + volcengine × 1 + minimax × 1 切到
  受限路径。恶意/异常中转站无法再撑爆 reqwest 内部 buffer。
- **chore**：4 个 v0.3+ 预留 helper（`default_enabled` /
  `instantiate_builtin` / `display_label` / `Minimax::region`）标
  `#[allow(dead_code)]` 并写明留作未来 v0.3 stub / API 暴露使用。

### Validation

- `cargo check --locked --all-targets` 0 error
- `cargo test --locked --lib` **316 passed** / 0 failed
- `cargo fmt --all -- --check` 0 diff
- `cargo clippy --locked --all-targets` 0 error（warnings = pre-existing）
- `pnpm tsc --noEmit` 0 error
- `pnpm vitest run` 29/29 passed
- `bash scripts/check-no-native-dialogs.sh` clean
- `bash scripts/validate-i18n-keys.sh` consistent

## [0.2.4] - 2026-07-17

### Added

- **双击浮窗打开设置面板**（commit `361fc55`）：原双击绑定「立即刷新」
  移除（`refresh_now` 仍可从托盘菜单触发）。双击跳过
  `button/input/select/a`，避免双击错误卡按钮时既触发按钮动作又弹
  设置窗
- **Kimi 浮窗左侧标签改动态窗口剩余**（commit `75a5d8f`）：滚动窗口行
  不再渲染 `used/total`，改显示动态标签（剩 <1 天 → `5h`，≥1 天 →
  `7d`），每秒随倒计时刷新；foot 前缀跟随（`5h重置` / `7d重置`），不再
  硬编码「月重置」。`kimi.rs` 两行打 `RowKind::FiveHour` /
  `RowKind::Weekly` 标记，Tavily 无 kind 标记保持 `used/total` 不变

### Fixed

- **5h 用量达 100% 上限时行消失**（commit `de6668b`）：kimi / zhipu /
  claude_official 三个 provider 的渲染路径在满额时行被错误隐藏，修为
  正常渲染

## [0.2.3] - 2026-07-10

v0.2.2 (2026-07-10) 紧跟的视觉 hotfix：macOS 26 (Tahoe) 菜单栏上 M 圆比预期大一圈 + 圆外多一圈"halo"空间。**1 个文件改动**（`src-tauri/icons/tray-base.png`），Rust 代码完全不动。

### Fixed (Visual)

- **macOS 26 菜单栏 tray icon 偏大 + halo**：
  - **症状**：托盘 M 圆**视觉大小**比期望大 25%，圆外多一圈"halo"空间，light/dark mode 都有
  - **根因**：[src-tauri/icons/tray-base.png](src-tauri/icons/tray-base.png) 是 32×32，M 圆**几乎贴边**（外径 32/32 = 100%）。tray-icon 0.24.1 强制 `setSize(18, 18)`，macOS retina 下需要 36×36 物理像素 → **从 32 上采样 12.5%**，边缘模糊 + 圆看着"大一圈"
  - **对比**：Usticky dev 走 `default_window_icon()` → `icons/icon.png` (1024×1024)，**下采样**到 36×36 锐利得多
  - **修法**（fix-B，最小改动）：`tray-base.png` 重做为 **64×64**，**48×48 内容居中** + **8 像素透明 padding 四边**。`tray.rs:558` 的 `if w < ICON_SIZE` Lanczos 上采样分支跳过（64 > 32），64×64 直接透传给 NSImage。retina 36×36 下采样 44% 锐利，**opaque bbox 从 32×32 缩到 24×24**（-25% 视觉大小），圆外 6px 透明 padding 让 halo 消失
  - **验证**（Python PIL ASCII 对比）：

    | 状态 | Opaque bbox @ 36×36 | 占比 |
    |---|---|---|
    | 之前 (32×32) | (2,2)→(33,33) = 32×32 | 89% |
    | 之后 (64×64 fix-B) | (6,6)→(29,29) = 24×24 | 67% |

  - **tray.rs:541-573 行为**：64 source 进 `Image::new_owned(rgba.into_raw(), 64, 64)`，rust `match w < ICON_SIZE` 不进 resize 分支（64 ≥ 32），64×64 透传 → NSImage `setSize(18, 18)` 渲染
  - **不影响的平台**：Win/Linux 不读 `tray-base.png`（Win 走 `tray-base.png` 同样的 `include_bytes!` 但 `ICON_SIZE = 64`，64 → 64 直接透传，行为不变；Linux 同）

## [0.2.2] - 2026-07-10

v0.2.1 (2026-07-09) 紧急 hotfix：macOS release 跑起来 UI 全是 raw i18n key（`provider.minimax.name` / `settings.nav.providers` / `floating.countdown.reset_suffix` 等）。7 platform bundle 内容不变，只换 frontend bundle。

### Fixed (Critical — frontend i18n 全炸)

- **v0.2.1 release UI 显示 raw i18n key**：浮窗 / 设置面板所有 t() 调用的位置都返 key 字面量（截图证据：provider.minima... / 5hfloating.countdown.reset_suffix / settings.nav.providers / settings.region.section_title / window.settings）。
  - **根因**：[src/i18n/index.ts](src/i18n/index.ts) 之前用 `await import(\`./${l}.json\`)` 模板字符串动态 import 加载 locale JSON。Vite/Rollup 只能静态分析 **literal 路径**（`./en.json`），模板字符串里的 `${l}` 让 build 阶段**没有 chunk 生成目标**，runtime 时 `import()` 静默失败（没有错误弹窗）→ loadLocale catch 走 fallback `dicts[l] = {}` → t() 永远 lookup null → 返 raw key
  - **跟 v0.2.0 那个"Windows LTO+strip 把 HashMap backing 段丢了"的 bug 不同**：那个是后端 Rust i18n 全空（probe 启动即崩），这次是前端 dicts 一直是 `{}`（probe OK，UI 露馅）。两个 bug 独立
  - **跟 v0.1.0 也没关系** —— 这条路径在 v0.2.0 引入 `loadLocale()` 时就这么写，只是当时 locales 文件也是"空结构（仅 metadata）"（index.ts:9-11 注释），症状相同被掩盖。v0.2.0 follow-up 把 en.json / zh-CN.json 填到 543 行（11 sections, ~180 keys）后，**bug 还在**，只是没人跑过 release dmg 验证（dev 模式 console warn 一堆但 UI 仍能继续用）
  - **修法**：src/i18n/index.ts module 顶部 static import 两个 locale（`import enDict from "./en.json"` + `import zhCNDict from "./zh-CN.json"`），dicts 在 module load 时填好。`loadLocale()` 退化成"确认 dict 已就位"的 no-op（保留 export 不破坏外部调用方）
  - **验证**：rebuild 后 `dist/assets/main.js` 里 `"MiniMax"` / `"Xiaomi MiMo"` / `"Settings"` 等翻译字符串出现 7 处（之前 0）；`provider.minimax.name` 这种 raw key 在 bundle 里消失

### Documentation

- **RELEASING.md §3.5a 新增**：macOS 未签名 dmg ad-hoc sign 应急步骤（`xattr -cr` + `codesign --force --deep --options runtime --sign -`），v0.2.x 每次升级都要重做
- **README 故障排查表加一行**：v0.2.1 raw i18n key 现象的诊断 + 修法（升 v0.2.2 / rebuild）

### Notes

- **本次 release 是 frontend-only fix**，Rust binary 没动。理论上不需要重打 macOS / Windows / Linux bundle —— 但 tauri-action 是按 tag 触发全平台重 build，7 个 bundle 都会重产。文件 hash 会变（dist/assets/* 因为 vite content hash 改了），但**功能上 100% 等价于 v0.2.1 + i18n fix**
- **用户操作**：v0.2.1 → v0.2.2 升级路径同 v0.2.0 → v0.2.1（覆盖装）。macOS 仍需重做 ad-hoc sign 步骤

## [0.2.1] - 2026-07-09

v0.2.0 (2026-06-29) 之后 10 天的累计：**3 critical + 23 high + 19 medium security/quality 全量审查修复 + macOS signing saga + Linux + Windows MSI 发板**。version 字段 0.2.1，git tag `v0.2.1` 落在本 commit。

### Added

- **Linux x64 发板**：`.AppImage` (免安装) + `.deb` (Debian/Ubuntu) + `.rpm` (Fedora/RHEL) 三种 bundle，由 `ubuntu-22.04` runner 构建。`platform/` Linux stub、`tray.rs` Linux 字体路径 (DejaVu/Liberation Sans) 之前已就位，0 个 Rust 代码改动
- **Windows MSI installer** (`.msi`)：与 v0.1.0 行为对齐。NSIS (`.exe`) + MSI 双产物并存
- **`tauri-action` 升级 v0 → v1** (Jun 29 2026 latest)：Linux bundle 集成更稳，asset 上传逻辑修了几个 edge case

### Fixed (Critical — 安全/正确性)

- **C1 (poller)**: `src.id()` (base id) 查 `cfg.providers` enabled 状态时多实例场景下 `minimax#2` / `minimax#3` 永远走 fallback (commit `201687a`)
- **C1 (xiaomi_login)**: `is_dashboard_url` 子串匹配 → DNS rebinding 攻击，改 `host_str() == "platform.xiaomimimo.com"` + `scheme == "https"` 严格匹配；`extract_user_id_from_url` 加 digits-only 校验 (commit `4cb80d8`)
- **C2 (openrouter)**: `LAST_SUCCESSFUL` 之前是 `Mutex<Option<(Instant, Endpoint)>>` 全局单例，`minimax#2` 切 endpoint 会覆盖 `minimax` 缓存。改 per-source endpoint 缓存；`free_tier` 不再误报 `Parse` (commit `ca706c0`)
- **C2 (capabilities)**: 拆分 `default.json` + `xiaomi-login.json`，`create-webview-window` 权限只给 xiaomi-login 窗口，default webview 不再能起新窗口 (commit `4cb80d8`)
- **C3 (release profile)**: `panic = "abort"` → `"unwind"` — spawn task panic 不再 abort 整个进程，托盘/浮窗不会丢 (commit `4cb80d8`)
- **C3 (extra_instances)**: `delete_extra_instance` 先 read lock → 释放 → write lock 删除，两个 lock 之间用户 add 同 provider 会导致删除错位。改 write lock 全程 (commit `a3e6950`)
- **C1 (config migrate)**: `config::migrated()` 老 config.json 升级用 `unreachable!()` → `default_provider_config(id)` helper，老配置升级到 v0.2.x 不再 panic (commit `ee3dfc3`)

### Fixed (High — 行为正确性)

- **H1 (tray)**: `pick_minimax_rows` 之前 `source_id == Some("minimax")` 严格相等匹配，多实例 `minimax#2` / `minimax#3` 全被过滤掉。改 starts_with 匹配 base id (commit `d5612ab`)
- **H2 (resize)**: 浮窗 resize 时 NaN 检查缺失，未捕获路径会让 emit 逻辑算脏值 (commit `d5612ab`)
- **H2 (Win HiDPI)**: 显式 `SetProcessDpiAwarenessContext(Per-Monitor V2)`，跨 DPI 屏 hover 检测不再挂 (commit `4cb80d8`)
- **H4 (delete cascade)**: `delete_source_builtin_key` 级联清 `extra_instances` 副本，防止浮窗显示"未配置"的死副本 (commit `4cb80d8`)
- **H4 (tavily)**: `((u/l)*100)` 没 clamp，`limit_remaining` 负值或 `u>l` 时返 >100% / 负数。进度条爆框 (commit `28f32e1`)
- **H5 (config)**: `config.json` 损坏不再 fallback 到 `Self::default()`，`best_effort_from_value` 保留可知字段 (commit `4cb80d8`)
- **H6-H9 (providers)**: 11 provider 一致性 + 共享 SSRF 防护 (commit `28f32e1`)
- **H11-H14 (extra_instances)**: add/update save 失败回滚 + 删死代码 (commit `a3e6950`)
- **H15 (delete_credential fallback)**: 找不到 builtin 时正确 fallback (commit `d5612ab`)
- **H16 (macOS hideOnDeactivate)**: 浮窗隐藏行为符合 macOS 习惯 (commit `d5612ab`)
- **H17/H21 (frontend credentials)**: `saveCredentialAction` 之前 `input.value.trim()` 把整段 textarea 当一个值，多行 cookie 粘贴全丢。改逐行处理 (commit `e04cffe`)
- **H18/H19/H22/H23 (frontend)**: UUID 拦截 / 死代码 / `card-dot-error` CSS / 阈值顺序校验 (commit `e04cffe`)
- **H1/H2 (Windows 浮窗重启)**: 重启浮窗回左上角 — `(0,0)` NSWindow 默认值两层拦截 (commit `7b488a7`)

### Fixed (Medium — 全量审查 batch 2a/2b/3/4)

29 个 medium 级别问题分 4 批修复，覆盖 persistence / providers / platform / frontend / i18n 全栈：

- **batch 2a (persistence)**: fsync + worker recover + lock ordering (commit `31aeb44`)
- **batch 2b (providers)**: body cap + jpath depth + ANSI filter + tz parse + spawn recover doc (commit `ad05224`)
- **batch 3 (platform+frontend+locs)**: M5/M9/M10/M11/M17/L1/L7/L13/L14/L16 (commit `bcecd57`)
- **batch 4 (misc+locs)**: frontend regex wiring + region race + first-launch + L11 + L15 (commit `f38a91e`)

### Fixed (UI / UX)

- **delete builtin key 二次 confirm** 警告副本级联 (commit `d0def65`)
- **tray 5h/Weekly 行匹配**: 改用 `RowKind` 枚举，跟 locale 解耦 (commit `b7e8d65`)
- **logs dedup_cache 24h 过期回收** (commit `0eda5c9`)
- **minimax provider QuotaRow 填 `RowKind` 枚举**: 补 M2 漏改 (commit `707db33`)
- **浮窗显示逻辑**: 移除不必要的 `focused` 状态检查 (commit `40f0c11`)
- **浮窗内容高度测量**: 避免底部留空白 (commit `760dddd`)
- **浮窗 ResizeObserver**: 自动调整高度，保留用户手动调整窗口能力 (commit `63d01eb`)
- **浮窗 emit 简化**: 提升代码可读性 (commit `a0f0025`)
- **浮窗 hover 响应速度**: 恢复 v0.1.0 体验 (commit `7822901`)
- **浮窗毛玻璃后台偶发闪一下**: macOS 修复 (commit `5983104`)
- **hover emitter 加 dwell-time hysteresis**: 治根因（多 transparent 窗口共存抖动）(commit `f2eeb3c`)
- **macOS menubar hidden 检测**: 修 `unused variable` warning (commit `4e6ccd9`)
- **macos.rs M7 refactor 漏掉的 `std::thread` import** (commit `92b98d9`)

### Fixed (Build / CI / i18n)

- **rust-i18n Windows release bug**: MSVC + lto + strip 组合下链接器会把 HashMap 字面量 backing 数据段当 unreferenced 丢掉，导致 release binary 的 backend 是空的 —— t!() 全部回退成 `locale.key` 字面量。**砍 `strip = true` 解决** (commit `5e87468`)
- **CI 跨平台 i18n test stability**: 4 个 i18n test 在 ubuntu/macos runner fail 但本地 zh-CN 过。修测试假设 (commit `f199efb`)
- **cargo fmt**: 修 batch 4 引入的 6 处格式偏差 (commit `4209c75`)
- **rustfmt 修 batch 4 引入的格式偏差** (`src-tauri/src/lib.rs:437` emit 合并行) (commit `7822901` 之前的 patch)

### Known Caveats

- **macOS 安装包未签名**: 用户机器没有 Apple Developer ID，dmg 走未签名构建 + quarantine xattr → macOS 弹"应用已损坏"。**应急**: `xattr -cr /Applications/Musage.app && codesign --force --deep --options runtime --sign - /Applications/Musage.app` (详见 AGENTS.md 「macOS 安装后显示『应用已损坏』」)
- **Linux AppImage 无签名**: 第一次运行需 `chmod +x` + 双击或命令行运行。GNOME 桌面需要装 AppIndicator extension 才能看到系统托盘
- **Linux Wayland**: 默认走 XWayland；原生 Wayland 渲染有 dpi / 字体微调问题，未专门适配

## [0.2.0] - 2026-06-29

v0.1.0 (2026-06-13) 之后 16 天的累计：6+ PR + 1 次大清理 + PR 1b 额外实例 + 35 个 follow-up commit。version 字段钉死 0.2.0，git tag `v0.2.0` 落在本 commit。

### Pre-cleanup 累积 (PR 2-3 + PR 1b, commits `0ae07d0`–`1985250`)

#### Added
- **Kimi (Moonshot Coding)** 套餐源 (commit `0ae07d0`)
- **智谱 GLM** 套餐源,支持国内/国际 endpoint 切换 (commit `0ae07d0` + `4aedaef`)
- **Xiaomi MiMo 一键登录**:应用内 WebView 自动提取 Cookie (commit `c561c2e`)
- **Xiaomi 多鉴权 fallback**:Bearer→Cookie 自动降级,"丢个 API key 就跑" (commit `d232b31`)
- **Per-provider 指数退避**:429/5xx 翻倍间隔,30 分钟上限 (commit `77bd65d`)
- **Xiaomi 浮窗显示模式 3 档选择器**:完整 / 只套餐 / 只总额度 (commit `2c6d2d7`)
- **首字母 fallback logo**:新 provider 不写 SVG 也能跑 (commit `6d3d822`)
- **顺序区按 enabled/disabled 分区**:浮窗卡片顺序更清晰 (commit `8c8b749`)
- **CustomSource (New API 中转站)**:用户可加任意 New API 系中转,3 选 1 extract 模板,JSONPath 解析,UUID 持久化 (commits `68b5dff` / `b234e45` / `bb5151c`)
- **设置面板重构**:6 组分组 + 顶部搜索 + 原生 `<dialog>` modal + ↑/↓ 跨段排序
- **i18n P0-P2**: 后端 rust-i18n + 前端自写 helper + `set_app_locale` IPC + 11 provider 错误 + settings 5 section + tooltip 全 i18n 化
- **PR 1b Extra Instance (2026-06-24)**: 内置 provider 副本 + 统一 `extra_instances.json` 持久化 + 复制按钮预选 (`c6ce2be` / `1985250`)
- **3 个新 built-in provider**: StepFun / SiliconFlow / Claude 官方 (PR 2)

### 2026-06-22 集中清理 (PR 1-6, commits `b253720`–`54a5554`)

#### BREAKING
- **Config schema**: `Provider` enum removed. All source id is now a string (e.g. `"minimax"`, `"tavily"`, `"custom_<uuid>"`). Old config.json files still load (`ProviderSnapshot.provider` field defaults to `""` via `#[serde(default)]`).
- **IPC API**: 7 enum-based commands removed (`has_api_key_for` / `set_api_key_for` / `delete_api_key_for` / `get_api_key_for` / `has_cookie_for` / `set_cookie_for` / `delete_cookie_for`). Use the string-based `set_source_credential` / `get_source_credential` / `has_source_credential` / `delete_source_credential` instead.
- **Config keys API**: 7 enum-based helpers removed in `config.rs` (`load_api_key_for` / `save_api_key_for` / `delete_api_key_for` / `load_cookie_for` / `save_cookie_for` / `delete_cookie_for` / `cookie_key`). Use `load_credential_for_id` / `save_credential_for_id` / `delete_credential_for_id`.

#### Removed
- `Provider` enum (35 placeholder sites consolidated to string ids; `tavily-enum-placeholder-footgun` warning fully resolved).
- `ProviderImpl` trait (replaced by `QuotaSource` trait).
- **Novita + Qwen STUB providers** (公开 API 无 quota endpoint,永久返回「未支持」错)。用户看到「未支持」比看不到更沮丧 (F1 反模式),直接砍更干净。
- 2 个 STUB logo 资产 (`novita-logo.svg` / `qwen-logo.svg`).

#### Fixed
- **Tray tooltip 永久只看 MiniMax** (`tray.rs`): 之前 `Provider::Minimax` enum filter 导致 Tavily / ZenMux / Kimi / Zhipu / StepFun / SiliconFlow / Claude 官方 / 用户自定义 New API 中转站 **全部不进 tray tooltip**。改成 `source_id == "minimax"` 字符串路由,所有 provider 都进 tray tooltip。
- **Xiaomi `apply_display_mode` i18n bug**: 之前 hardcode `"套餐"` / `"总额度"` 字符串 filter,en locale 下永远 filter 0 行 → 浮窗空白。改成 `t!("row.plan")` / `t!("row.monthly_total")`,跟 locale 解耦。
- **生产代码 10 个 broken test** (lib test 编不过) — 9× `RwLock::get()` 弃用 (rust 1.78+) 改 `read().unwrap()`,1× `Cow<'_, str>` vs `&str` 类型 mismatch 加 `.as_ref()`.
- **23 个 i18n assertion** (test 写死中文 label, en locale 下不匹配) — 改 `t!("row.xxx")` 形式。
- **2 个 epoch range 太严** (timezone drift ±1 天) — 放宽到 ±6 天。

#### Changed
- **CI now runs `cargo test`** (was just `--no-run`); added `cargo clippy` + `cargo fmt --check` + `pnpm exec tsc --noEmit` gate.
- **Docs-only PR check**: `docs:` PR titles fail if non-.md files modified (force separation of docs-only PR from mixed PR).
- **`ProviderSnapshot.provider` field**: `Provider` enum → `String`. `#[serde(default)]` 让老 JSON 反向兼容。
- **`ProviderSnapshot::health_label()`**: `match self.provider` 改 `match self.source_id`（deepseek 走 `is_healthy` 分支, 其它 utilization 分支）。
- **`ProviderSnapshot::empty_error()`**: 删 `provider: Provider` 参数（`Provider` enum 已删）。
- **Frontend**: `settings/order.ts` 删 9 处冗余 `as ProviderId` cast。
- **Backend**: `lock_recover()` helper 抽出（mutex poison recover 模板统一 3 处使用）。
- **`cargo fmt --all`** 跑过一次：28 文件重新格式化（纯格式变化，无逻辑）。

#### Tech debt cleared
- `commands/mod.rs`: 1732 → ~1590 行 (-142 from 7 IPC + Provider 清理)
- `providers/mod.rs`: 删 60 行 enum 定义 + ProviderImpl trait
- `poller_backoff.rs` test fixtures: 改用 string ids
- `apply_display_mode` production bug: hardcode `套餐`/`总额度` 改 `t!()` (i18n 解耦)
- 11 个 provider 文件: `Provider::Xxx` 占位改 `"xxx"` string (消除 [memory/tavily-enum-placeholder-footgun](memory/tavily-enum-placeholder-footgun.md) 警示)

### v0.2.0 后续 follow-up (commits `00565a1`–`28e4b4e`, 35 个 commit, 2026-06-22 ~ 2026-06-29)

#### Added (PR 1b 收尾 + 增量)
- **批量粘贴 key 自动匹配**:providers section 顶部 `<details>` 折叠 batch textarea,粘贴多行 `provider=value` 或纯 key 自动识别 provider 前缀(`sk-cp-` / `tp-` / `tvly-` / `Oasis-Token` 等复用现有 placeholder regex)并填入。
- **New API preset callout**:`+ 添加新来源` modal 选中 custom 时显示强调框,引导用户用 New API 模板。
- **系统通知(Xiaomi/Claude cookie 失效)**:`tauri-plugin-notification` + 60s 去重,`ErrorKind::AuthFailed` + provider 是 `xiaomimimo` / `claude_official` 时弹系统通知(走 `log_provider_error` hook,跟现有去重共用一套缓存)。
- **import/export 配置(无 keys)**:设置面板"高级"页 Import/Export section,Export 走纯 web `Blob` + `<a download>` 下载 JSON,Import 走 `<input type="file">` + `FileReader` 走 `saveConfig` 全量保存,0 新 Tauri dep。
- **托盘 tooltip `#N` 后缀**:多 instance 时同 base provider 区分显示(如 `MiniMax 5h 45% / 周 72% #2`)。
- **托盘进度条遍历多 instance**:原 `pick_minimax_rows` `find()` 只取第一个 minimax,改成遍历所有 instance + 进度条颜色按 `#1` 走。
- **`ProviderSnapshot.unique_id` 字段**:后端 snapshot 加 `#[serde(default)] unique_id: Option<String>`,所有 provider 构造时填 `self.unique_id()`,前端 7 处 id 路由(`contentFingerprint` / `existingCards.get(id)` / 重排 / `rowsForRender` / `card.dataset.provider` / `updateCard` / err button)优先取 `unique_id`,fallback `source_id` → `provider`。
- **错误恢复完整版 (P2-A-7)**:浮窗错误卡新增 2 个通用按钮 (任何 error_kind 都可用):
  - `📋 Copy error`: 复制 `p.error` 到剪贴板 + mini flash 反馈 (3 秒自动淡出,玻璃风格 backdrop-filter)
  - `📋 Logs`: 打开设置 + 跳到 logs section (复用修好的 `open_settings_window(section)` 通道)
- **浮窗位置跨屏感知 (P1-1)**:多屏用户拖到副屏后重启,位置可能"不在任何 monitor 内"(副屏拔了 / DPI 变 / 显卡驱动重置)。`saved_pos_valid` 判定从 `x > 50 || y > 50` 启发式升级成 `position_is_visible(x, y, &[Monitor])` 几何检查:遍历 `win.available_monitors()` 矩形包含测试。拿不到 monitor 列表(Wayland 老版本)时 fallback 老启发式。monitor hotplug 监听留 v0.3。

#### Changed
- **后端 `list_picker_providers` 返翻译好的 `display_name`**:`PickerProvider.name_key` → `display_name`,后端用 `t!("provider_name.xxx").into_owned()` 注入翻译串。前端 `src/i18n/{en,zh-CN}.json` 的 `provider_name.*` 11 项镜像删除,**单一来源 = 后端 `src-tauri/locales/{en,zh-CN}.json`**。
- **浮窗 DOM `data-source-id` → `data-unique-id`**:错误卡 retry 按钮 click 委托从 `target.dataset.sourceId` → `target.dataset.uniqueId`。
- **`config/custom_sources.rs` wrapper 内联到 `lib.rs` 后删除**:v0.2.0 清理点 2 天后老 `custom_sources.json` 都被启动 rename 成 `.migrated`,wrapper 无 active caller。
- **后端 `open_settings_window(app, section: Option<String>)` 接收 section 参数**:窗口起后 emit `musage://settings-navigate` 事件,settings.ts 新增 listener 调 `navigateToSection(target)` helper 跳 section。修隐藏 bug:错误卡 advanced 按钮之前传 `{section: "advanced"}` 给后端被静默忽略。

#### Fixed (full audit 收尾 + 13 隐患)
- **PR 1b 5 项残留限制全部清理**:
  - 浮窗 `data-unique-id` 改 `unique_id()` 渲染 ✅
  - 托盘 tooltip 拼 `#N` 后缀 + 进度条遍历多 instance ✅
  - `list_picker_providers` 返 `display_name` ✅
  - 前端 `provider_name.*` 镜像删除 ✅
  - `delete_extra_instance` keys.json rename 留 v0.3 (本 commit 不打 keys.json schema break)
- **P0 3 个 blocking bug 修** (`f1f74f9`): delete UUID / CustomSourceSpec serde / orphan extra instance
- **P1 5 个 audit 项修** (`3f99686` + `0180715` + 后续 5ef575c/edd7f82/5c64314/d328eeb):
  - 12 provider `do_fetch` 统一接受 `source_id/display_name` 参数 + C1/C2/H2/H5/M3/M6 修复
  - 5 处全量审查潜在 bug
  - add_extra_instance / update_extra_instance / compact key migration 错误处理 / 顺序修复
  - claude_official copy form 按 auth_kind 切显示 cookie 输入框
- **i18n full audit** (`99320c0`): duplicate provider key / 硬编码 label / fallback path / 10 test assertion
- **extra-instances 3 合 1** (`70068a0`): 重复卡片 / 图标缺失 / 拖拽顺序 + TOML test 500
- **display_name post-fill** (`a47fd1d`): `refresh_inner` / `refresh_single_inner` 通过 `find_source()` 回填 `source_display_name`
- **i18n window title** (`c8cd7fe`): 浮窗 window title 走 i18n 路径（不再硬编码 'Musage'）
- **小米 plan-expired 路由** (`a176b60`): 独立 `error_kind` + 消息
- **ci doctest + Linux Manager import** (`08bdd84`): 2 个 CI 隐患修
- **tray 时间戳 invalid** (`b164b8b`): invalid 时间戳不再显示空串
- **v0.3 placeholder 清理** (`5cb344b`): `"minimax"` provider placeholder 改真实 `provider_id`
- **cargo fmt** (`28e4b4e`): 28 文件重新格式化

#### Tech debt 收尾
- Tech debt 7 项刷新:2 ✅ 已修(原硬编码中文 / Provider enum)/ 5 ⏳ 留 v0.3(`http_status_to_error_kind` / `refresh_inner` Box::new / Backoff 持久化 / Per-provider shutdown signal / Frontend 单测)
- PR 1b 5 项限制:4 ✅ / 1 ⏳(`delete_extra_instance` v2 完整重做留 v0.3)
- P2-A 完成度从 13% → 87%(批量粘贴 + New API callout 完成;错误恢复完整版完成)
- P2-B 完成度从 17% → 75%(系统通知 + import/export 完成;Claude 一键重登留 v0.3)

### Upgrade notes
- 老 v0.1.x 用户升级 v0.2.0:
  1. 应用启动不会崩 — `keys.json` / `config.json` / `custom_sources.json` 通过 `#[serde(default)]` 自动迁移
  2. 设置面板老凭据仍能读 — 因为 keys.json 文件格式没变 (只是 key 是字符串不是 enum 序号)
  3. **第三方插件/脚本如果之前调 `set_api_key_for(minimax)` / `set_cookie_for(xiaomimimo)` 等老 IPC 命令会失败** — 改用 `set_source_credential("minimax", key)` / `set_source_credential("xiaomimimo", cookie, field="cookie")`
- 老 v0.1.x 用户的 `custom_sources.json` 在 v0.2.0 启动时会被自动迁移到 `extra_instances.json` 并 rename 成 `.migrated`
- `keys.json` 格式不变(`delete_extra_instance` v2 重做留 v0.3 单独 PR)

[0.2.0]: https://github.com/Thedeergod666/Musage/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Thedeergod666/Musage/releases/tag/v0.1.0

## [0.1.0] - 2026-06-13

首发版本。

### Added
- 桌面悬浮窗 + 系统托盘
- 6 个 quota source:
  - **MiniMax** (Bearer, Token Plan 5h/周)
  - **DeepSeek** (Bearer, 余额)
  - **Xiaomi MiMo** (Cookie, 套餐+总额度)
  - **Tavily** (Bearer, Credits)
  - **ZenMux** (Bearer, PAYG/Subscription)
  - **OpenRouter** (Bearer, 余额)
- Tauri 2 + Windows NSIS 安装包(2.5 MB)
- 本地 `keys.json` 存储(0600 权限)
- 后台 tokio 轮询
- CLI `musage dump` 子命令(独立验证 schema)
- 托盘动态图标(颜色随用量变:绿/橙/红)
- 文档:README / AGENTS / ROADMAP

### Known limitations
- 6 个 provider 平铺在设置面板,UI 待重构
- 浮窗卡片无位置记忆
- Windows 端 PinBottom 模式 hover-raise 是 best-effort(见 AGENTS.md)

[Unreleased]: https://github.com/Thedeergod666/Musage/compare/v0.2.4...HEAD
[0.2.4]: https://github.com/Thedeergod666/Musage/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/Thedeergod666/Musage/compare/v0.2.1...v0.2.3
