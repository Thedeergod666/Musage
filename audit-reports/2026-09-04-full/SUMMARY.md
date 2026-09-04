# 2026-09-04 全量代码审查报告（8 域并行）

> 审查基线：commit `d801adc`（含当日 Win 三 bug 修复：hover-raise / Acrylic ExtendFrame / 双击进设置）。
> 项目规模：约 2.1 万行 Rust + 8400 行 TypeScript，14 内置 provider + custom，3 个登录模块。
> 方法：8 个独立审查 agent 并行，每域聚焦真实 bug（逻辑错误 / 边界条件 / 并发竞态 / 资源泄漏 / 错误处理 / 安全），要求 file:line + 代码证据 + 触发条件，排除风格与纯推测。

## 总览

| 域 | 范围 | C | H | M | L | 小计 |
|---|---|---|---|---|---|---|
| 1 | providers 核心（mod/parse/custom/minimax/deepseek/xiaomi/tavily） | 0 | 1 | 3 | 6 | 10 |
| 2 | providers 二批（zenmux…tokendance 共 10 个） | 0 | 1 | 3 | 6 | 10 |
| 3 | poller / backoff / logstore | 0 | 1 | 3 | 5 | 9 |
| 4 | config / commands IPC | 0 | 1 | 4 | 7 | 12 |
| 5 | platform / tray / lib | 0 | 2 | 5 | 8 | 15 |
| 6 | 登录模块 ×3 | 0 | 2 | 3 | 4 | 9 |
| 7 | 前端浮窗 | 0 | 2 | 7 | 10 | 19 |
| 8 | 前端设置面板 | 0 | 2 | 9 | 8 | 19 |
| **合计** | | **0** | **12** | **37** | **54** | **103** |

**无 Critical**。上一轮（2026-08-17）的 1C + 6H 修复与今日 Win 三 bug 修复均未发现回归。
12 条 High 全部是真实可触发的功能损坏/数据丢失级问题，建议 v0.2.9 第一批全修；M 建议按域批量修；L 择机。

---

## High（12 条，建议全部修复）

### H-1 [providers] DeepSeek 副本 health_label 永远 "ok"，is_healthy=false 被忽略
- 位置：`providers/mod.rs:623-647` + `providers/deepseek.rs:252`
- 副本的 `source_id` 是 unique_id（`"deepseek#2"`），`health_label` 按 base id 精确匹配 → 副本落 `_` 分支，utilization 恒 None → 按 0.0 判 "ok"。副本账号不可用时托盘绿点 + worst_health 误报 ok，与 base 实例行为不一致。
- 修法：匹配前用 `base_id_of()` 剥 `#N` 后缀。

### H-2 [providers] ZenMux 清除自定义 base_url 后内存残留，key 持续发往已删除端点
- 位置：`providers/zenmux.rs:166-174`（对照写入侧 `commands/mod.rs:549-553`）
- `set_state` 只在 `Some` 时写 `base_url` RwLock，从不清空；用户清空输入框（config 已正确置 None）后，旧 URL 残留内存直到重启，`Authorization: Bearer <key>` 持续发往用户已移除的中转端点。
- 修法：`*g = url.map(str::to_string)` 无条件写。

### H-3 [poller] 错误态副本快照 `provider` 字段写成 unique_id → 禁用 base 后错误卡永久残留，且可在 30s 窗口内复活
- 位置：`providers/mod.rs:538`（`empty_error` 用 `id.to_string()`）+ 消费端 `commands/mod.rs:631/1968/2269`
- 成功快照硬编码 base id，错误快照是 unique_id；`is_enabled_unique(unique, p.provider)` 两参数同值时 base fallback 失效 → 禁用 base 后 `minimax#2` 错误卡永不被过滤；tick 中禁用还会被已 spawn 的 fetch 重新 emit 回浮窗。
- 修法：消费端改 `cfg.is_enabled_unique(snapshot_key(p), base_id_of(snapshot_key(p)))`，或 `empty_error` 加 base 参数。

### H-4 [config] geom persister / save_config 的"锁外快照"与持锁保存的 setter 交错 → 新设置被旧快照回滚落盘
- 位置：`lib.rs:591-625`（geom tick：clone 后 drop 锁再 spawn_blocking save）、`commands/mod.rs:876-885`（save_config 先替换再 save）
- 拖窗 flush 与改设置并发时，旧快照可以后落盘：改的 `low_power` / `floating_x` 等在重启后静默回滚。慢盘放大窗口。
- 修法：统一"持 `config.write()` 期间完成 save"，或给 AppConfig 加版本号、旧快照拒绝落盘。

### H-5 [platform] Resized 分支把 (0,0) 播种进 latest 并无条件落盘 → 用户拖好的浮窗位置丢失
- 位置：`lib.rs:536-537`（`g.unwrap_or((0,0,…))`）+ `lib.rs:593-600`（x/y 无 (0,0) 过滤）
- tao 纯 resize 只发 Resized 不发 Moved（SWP_NOMOVE）；persister 注册晚于启动恢复 → 首次 auto-fit 后 `(0,0,w,h)` 落盘，用户位置被清，下次启动回右上角默认位。
- 修法：位置维度改 Option，None 不播种；flush 侧 x/y 同样套 (0,0) 过滤。

### H-6 [platform] macOS `menu_bar_is_light()` 依赖主线程，所有调用点在 tokio worker → 恒 false，浅色菜单栏上白字不可见
- 位置：`platform/macos.rs:628-650`；调用点 `commands/mod.rs:2089/698/2562/2604/2636`、`lib.rs:203`
- `MainThreadMarker::new()` 非 main thread 恒 None → 保守返 false。该功能自引入以来在 macOS 上从未真正生效。
- 修法：走 `run_on_main_thread` + Condvar 单槽位（`is_floating_topmost_at` 同款模式），或主线程缓存到 atomic。

### H-7 [login] init script 无条件覆盖 `Storage.getItem`/`document.cookie`，破坏 SSO/OAuth 中间页
- 位置：`anysearch_login.rs:239-251`、`xiaomi_login.rs:272-284`
- 模块文档声称 L12 已修（"只在受信域安装"），实际是无条件安装、调用时门控——非受信域 `getItem` 返 null、cookie 写被丢弃。`stepfun_login.rs:24-29` 记录了同一 bug 并以删除 init script 修复；anysearch/xiaomi 未对齐。登录链路跳第三方登录页时流程直接坏掉。
- 修法：override 安装本身门控受信 host，或按 stepfun 删除。

### H-8 [login] 登录 token 写 base 槽，但"登录后立即拉取"指向副本槽 → base 禁用+副本启用时登录成功但浮窗报"未配置凭据"
- 位置：`commands/mod.rs:2115-2140`（resolve 目标可为 `"base#N"`）vs 三个登录模块硬编码写 base 槽；`config.rs:1408-1427` 无 fallback
- 修法：resolve 结果同时决定写入槽位，或副本读槽 fallback 到 base。

### H-9 [前端] rowKey 冲突 → 行无限增殖 + 数据串位
- 位置：`src/main.ts:1000-1056`
- 两行 `(kind,label)` 相同时 stable key 相同，`existing` map 后者覆盖前者 → 每轮 render 净增 1 个 stale 行，ResizeObserver 跟涨 → 浮窗无限变高。custom 双余额行 / 多额度包时可触发。
- 修法：key 加入 index 维度，或建 map 时拒绝重复 key。

### H-10 [前端] resize 回声判定纯值比较 → 高度碰撞时用户拖动被吞，auto-shrink 下限残留导致窗口"越缩越涨"
- 位置：`src/main.ts:612/677-684/1739-1744`
- 修法：回声判定加 ~500ms 时间窗，或 Rust 端给 fit 触发的 Resized 打来源标记。

### H-11 [settings] region-wizard Apply 按钮 stale closure → 切回原区域静默 no-op，UI 与后端端点分裂
- 位置：`src/settings/region-wizard.ts:25-28/87-107`
- 初始 cn → Global（成功）→ 再选回 CN → `selRegion === currentRegion` 跳过 setRegion → flash "已应用" 但后端仍 global，请求全走国际端点。
- 修法：apply 成功后更新闭包变量 / 去掉短路 / 重渲 section。

### H-12 [settings] advanced.ts flush 整体替换 `schema_overrides` → 抹掉其它 provider 的 overrides
- 位置：`src/settings/advanced.ts:71-81` + 后端 `commands/mod.rs:393`（整体赋值不 merge）
- 导入过含其它 key 的配置后，用户在高级 tab 每敲一键（300ms debounce）都会清掉这些条目。
- 修法：flush 前 getConfig 浅合并，或后端改 per-key merge。

---

## Medium（37 条）

### providers 域
- **M1** `minimax.rs:686-689` smart_reset_to_ms 超大 `raw*1000` 溢出 i64（release 无 overflow-checks 静默回绕，debug panic）→ saturating_mul + clamp。
- **M2** `xiaomi.rs:861-868` get_item_percent 绕过共享 num_f64：字符串 percent 静默丢行、负/超界不钳制。
- **M3** `providers/mod.rs:263-285` SSRF 防护无 DNS 解析层：`localtest.me` 等公网域名解析到 127.0.0.1/169.254.169.254 可绕过（含 redirect 每跳检查同缺口）→ 自定义 Resolver 在 connect 层复检 IP。
- **M4** `anysearch.rs:378-388` 主动续期失败无条件 return Err，丢弃仍有效的 access token（stepfun P2 修复未移植）。
- **M5** `anysearch.rs:243-346` refresh_token 锁内不重读 keys.json，并发刷新烧掉单次轮换 refresh token（同上未移植）。
- **M6** `zhipu.rs:323-343` 业务码 401xx 一律归 ServerError（xiaomi 同源问题已修，此处漏网）→ AuthFailed。

### poller 域
- **M7** `logstore.rs:229/302-311` ring 到 cap 后每条日志触发整文件重写 + 双 fsync（设计意图 ~1/200）→ 记录自上次 truncate 累计 pop 数再触发。
- **M8** `commands/mod.rs:1414/1429-1438` quit 500ms drain vs poller 1s tick：SHUTDOWN 通知落在 loop body 时 poller 被强杀；geom_persister / updater_check 连 AtomicBool 兜底都没有 → select! 加 biased flag 分支 + 补两处兜底。
- **M9** `commands/mod.rs:1861-1866/1911-1934` 30s 超时只放弃等待，JoinHandle drop 而非 abort → fetch task 泄漏 + 同 provider 并发双拉 → 显式 `task.abort()`。

### config/commands 域
- **M10** `save_config` 绕过 set_provider_order / set_tray_source / set_schema_overrides 三处写入侧校验（导入配置是现实触发器）→ 抽共享校验函数。
- **M11** `commands/mod.rs:2583-2587` set_tray_source 放行 `"minimax#2"` 形态，pick_tray_rows 永不匹配 → 托盘永久退化 logo（恰是 D6-05 要堵的症状）→ 拒绝含 `#` 入参。
- **M12** `config.rs:840-847` schema_version 高于本 build 时 save() 静默 Ok → 导入新版配置后本会话所有保存静默失效且 UI 报成功 → 入口校验 + 该分支返 Err。
- **M13** `commands/mod.rs:1344-1348` settings-navigate 固定 150ms sleep + 单次 emit：首次创建窗口时事件必然早于前端 listen → 深链跳 tab 静默丢失 → pending-section 握手。

### platform 域
- **M14** `macos.rs:507-519` 全屏退出复活用户手动隐藏的浮窗（swap 未检查当前可见性）→ hide 前查 `is_visible()`。
- **M15** `windows.rs:308-324` PinBottom→PinTop 切换与 emitter 在途 exit 采纳竞态，Bottom 后落地且无自愈 → 代际计数或切换后 re-assert。
- **M16** `windows.rs:582-642` 隐藏窗口被 hit-test 判 Covered → 发 hover=true 并把隐藏窗口抬 TOPMOST → `hit_test_floating` 开头查 `is_visible()`。
- **M17** `lib.rs:551-580` 退出时几何 flush 丢通知窗口 + shutdown flush 阻塞 save → 补 SHUTDOWN_REQUESTED 检查 + spawn_blocking。
- **M18** `windows.rs:264-272` apply_z_order 路 A `SetWindowPos` 返回值未检查无日志 → 失败 warn。

### login 域
- **M19** `anysearch_login.rs:262-268` 每次 document_start 无条件清 localStorage 登录态 + 中转 cookie：整页导航抹掉刚建立的会话 → 清理前先查有效 token。
- **M20** `xiaomi_login.rs:214-217/419-422` 写盘前缺最终 gen 复查 + open 时无条件清 EXTRACTING → 新旧提取任务可并发写盘（anysearch/stepfun 已修，xiaomi 漏）。
- **M21** `stepfun_login.rs:439-446` refresh 半段校验无 60s skew（access 半段已修，注释与实现不一致）。

### 前端浮窗域
- **M22** `main.ts:29` 浮窗未加载 tokens.css → 空态引导页 CSS 变量全失效 → 补 import。
- **M23** `main.ts:1758-1802` init 先 render 后读 config，读到后不重渲 → 非默认配置 + 缓存快照时首屏样式错误 → 读后补 render。
- **M24** `styles.css:647` + `main.ts:744-749` footer `margin-top:auto` 钉死测量高度 → show_footer_hint 开启后浮窗永不缩窗。
- **M25** `main.ts:621-645/650` commitPendingShrink 与 fitOnObserverTick 无互斥 → 并发 resize IPC 乱序 → 回声误判。
- **M26** `main.ts:1656-1695` 失焦清 hover 与 lastHoverPayload 去重不同步 → hover 玻璃效果死锁到鼠标出再入。
- **M27** `main.ts:1738` 启动把恢复高度当 userLastManualH 下限 → 跨会话内容变矮后底部永久留白。
- **M28** `styles.css:416/428-433` CSS 按 data-provider 精确匹配 → openrouter/tokendance 副本（#2）余额行样式漏配。

### settings 域
- **M29** `order.ts:390-421/453-455` 跨分隔线拖拽异步 continuation 读到已置 null 的 dragSrcId → 回滚失效 + flash "null" → IIFE 开头捕获局部常量。
- **M30** `providers.ts:314-319`/`app.ts`/`advanced.ts` getConfig→改一字段→saveConfig 全量读改写竞态（覆盖并发修改/浮窗位置）→ 单字段 command 或前端串行化。
- **M31** `providers.ts:226-232` 「在浮窗显示」checkbox IPC 失败不回滚（D8-03 只修了 pin radio）。
- **M32** `app.ts:92-147` 托盘样式/数据源失败回滚用渲染时初始值而非最近成功值（floating.ts 已修同款，此处漏）。
- **M33** `source-extras.ts` 7 个控件 IPC 失败一律不回滚 UI（系统性）。
- **M34** `extra-instance-form.ts:508-527` 火山双字段两步保存部分失败不可重试 → 重试产生重复实例 → 失败回滚 delete 或明确提示。
- **M35** `providers.ts:96-97` + `extra-instance-form.ts` 「添加新来源」无重入防护 → 双击堆叠两个 modal + 重复 DOM id 串台。
- **M36** `floating.ts:287-302` 阈值前端校验与后端 `[u8;3]` + 0<t0<t1<t2<100 不一致 → 非法值直送 IPC 报晦涩 serde 错。
- **M37** `region-wizard.ts:146-156` + `main.ts:192-199` 首启自动 apply Global 后不重渲 → 区域 radio 与后端不一致（D8-10 只修一半）。

---

## Low（54 条，一行索引）

**providers 核心（6）**：xiaomi 双路径缺 429→RateLimited 分支；minimax percent 路径 resets_at 丢字符串/浮点容错；refresh_inner 未配置凭据错误消息硬编码中文绕过 i18n；"xiaomi" 别名副本 api_key_ref 与 unique_id 分叉；custom.rs URL 配置/SSRF 拦截错误一律归 AuthFailed（退避策略错误）；Bearer key 无内部控制字符校验（换行 key 得误导性 Network 错误）。

**providers 二批（6）**：stepfun body 先读后判状态，>8MiB 401 响应绕过兜底刷新；anysearch 业务码只吃 as_i64 字符串绕过；zenmux usage>1.0 启发式在百分制语义下把 1% 渲染成 100%；volcengine 数字 Result.Code=0 被误判业务错误；kimi/tokendance remaining 负值未钳；zhipu 国际版 display_name 恒用国区 label。

**poller（5）**：新增/重启用 provider 时 Manual 单刷与 poller 立即 fire 双拉；backoff 禁用期残留重启用后首轮按旧退避；fill_next_fetch_at 用 IPC 入参 id 与 record 键口径不一致；redact 正则对 HTTP/2 小写 `set-cookie:` 盲点；tick/refresh_now emit 的快照未按 enabled 过滤未排序（H-3 放大器）。

**config/commands（7）**：set_app_locale 错误路径运行时/持久化分叉；best_effort_from_value 顶层 legacy region 覆盖已解析 providers.minimax；updater 缓存 up_to_date 永不刷新 + None 时无 in-flight 去重；4 条 format! 错误串绕过 i18n；update_extra_instance 的 key 写入在锁外与 compact 竞态；custom base_url 无写入侧 scheme/userinfo 校验（与 zenmux 双重防御不一致）；keys.json 损坏后无应用内恢复入口。

**platform（8）**：format_balance_tray 分支口径不一致 + 负数渲染怪；pick_tray_rows max_by 平局取最后与注释相反；macOS is_floating_topmost_at 迟到 closure 写 stale 槽；几何 persister 缓存 scale_factor 跨 DPI 失效 → floating-resized 逻辑像素错报 → 前端回声误判；ensure_per_monitor_v2_dpi 是死代码（tao 已设过恒失败）；pick_tray_rows 可返回 (Empty,Empty) 画全透明图标 + placeholder 图标尺寸不一致；D6-04 失败计数 off-by-one（首次 warn 在第 1001 次）；tray_source 存量失效静默永久 fallback 无提示。

**login（4）**：xiaomi debug 日志记完整 URL 未脱敏（同文件 P3 修复不一致）；userId 明文 info 日志；anysearch is_jwt_like 4096 上限作用于 combined 整串（token 膨胀即静默登录失败）；xiaomi F4 校验接受空值 cookie。

**前端浮窗（10）**：classList.forEach 边遍历边删 err-kind 残留；split-note 骨架首帧定型 extra 后到永不显示；三击重复 open_settings IPC；mousedown preventDefault 阻断滚动条拖拽；setLocale 重入守卫静默丢弃并发变更；logo.src 与 Vite 相对 URL 比较永真每次重设；fallbackLogo accent 未转义（XSS 纵深链唯一缺口）；启动 500ms 内点左上角 8×8px 误判双击；ZenMux PAYG 判定两处不一致（find vs rows[0]）；复制失败静默无反馈。

**settings（8）**：advanced debounce 无 flush 兜底（关窗丢编辑/一段坏 JSON 连累另两段）；interval 校验失败不回滚 + parseInt 截断小数；zenmux base_url 前端放行 http:// 后端只收 https；凭据保存 await 后清 input 竞态；阈值 parseInt 小数漂移；火山副本 SK 选填缺口（主面板必填）；api.ts updateExtraInstance 等 legacy 死代码；副本 display_name i18n 缺 key 显示原始 key。

---

## 域间交叉主题（修复时建议一并处理）

1. **unique_id vs base id 语义裂缝**（H-1 / H-3 / H-8 / M11 / L 多条）：副本（`#N`）在 source_id、provider 字段、凭据槽、托盘匹配、CSS 选择器五个层面反复出现 base/unique 混用。建议一次性梳理：内部状态统一存 unique_id，所有"按 provider 语义"的匹配/查表统一过 `base_id_of()`。
2. **stepfun 修复未移植**（M4/M5/M21 + xiaomi M20）：2026-08-13 P2 在 stepfun 修的 refresh 三连（锁内重读、非 auth 失败不放弃、skew 对齐）在 anysearch/zhipu/xiaomi 各有漏网。建议做一次"同类修复全 provider 扫描"。
3. **IPC 失败回滚缺失**（M29-M33）：设置面板十余个控件在 IPC 失败时停留新值。建议统一封装一个 `withRollback(control, apply)` helper 批量接入。
4. **退出时序**（M8/M17 + H-4）：quit drain、SHUTDOWN 通知丢失、锁外快照三件事同根，建议一起改。
5. **截图验证注意**：本机 GDI `CopyFromScreen`（DPI 虚拟化路径）**截不到** topmost+transparent 的浮窗（本次排查中曾长时间误导诊断）；验证浮窗视觉必须用 host 级截屏。

## 已验证无问题的面（负面结果摘要）

SSRF `fe80::` 掩码运算优先级、parse 深度限制、read_body_limited 截断、in-flight guard/abort 语义、interval clamp 四处口径、jitter 算术、环形缓冲边界、regex 线性时间（无 ReDoS）、原子写三处 tmp→fsync→rename、三把写锁锁序、DTO camelCase 全量比对、i18n 53 key 存在性、escapeHtml 覆盖（除 L7 accent 一处）、logs 无轮询无泄漏、modal 状态机、order.ts H-04 修复推演、登录模块 token 不外发第三方域、capabilities 最小集。
