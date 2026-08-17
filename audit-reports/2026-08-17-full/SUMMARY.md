# Musage 全量代码审查报告（2026-08-17）

## 概览

- **方法**：8 个并行审查域，每域一个独立审查 agent 精读全部目标文件，并交叉验证直接耦合的调用方。高危发现由主会话独立 spot-check 复核（4/4 坐实）。跨域独立命中同一点的条目合并计一条（多域交叉验证 = 高置信度）。
- **范围**：~21k 行 Rust + ~8.4k 行 TypeScript 全覆盖。
  1. 应用外壳与 IPC 命令层（lib.rs / commands/* / platform/*）
  2. 配置与持久化（config.rs / config/extra_instances.rs / logstore.rs）
  3. 轮询引擎、退避与托盘（poller*.rs / tray.rs）
  4. 登录模块（4 个 *_login.rs + kimi_desktop.rs）
  5. Provider 框架与 custom（providers/mod.rs / parse.rs / custom.rs）
  6. Provider 实现 A 组（minimax / stepfun / xiaomi / kimi / zhipu / deepseek）
  7. Provider 实现 B 组（volcengine_ark / anysearch / zenmux / openrouter / claude_official / tavily / tokendance / siliconflow）
  8. 前端 TypeScript（main.ts / settings/* / i18n）
- **结果**：**74 个独立发现 = Critical 1 / High 6 / Medium 26 / Low 41**（原始 79 条，跨域去重 5 条）
- **总体评价**：历经 2026-06-20 全量修复及后续五轮审计，核心护栏（原子写、锁契约、JoinSet 收尸、SSRF 加固、token 轮换串行化、CSP）均验证在位且无回归。本轮风险集中在：**实例身份键（id vs unique_id）在边缘路径的契约违反**、**快照/配置在 compact 重命名时的迁移缺口**、**若干 panic 点与死控件**。

### 跨域交叉验证命中（≥2 域独立发现，置信度最高）

| 发现 | 命中域 |
|---|---|
| dump CLI 按 base id 查副本凭据（lib.rs:738） | 域1 / 域2 / 域5（三命中）✅ spot-check |
| refresh_now 合并缺 fetched_at 比较（tick 同款已修） | 域1 / 域3 |
| refresh_single_inner interval 只 `.max(10)` 无上限 clamp | 域1 / 域3 |
| ~10 个无状态 source 未 override `needs_state_update` | 域5 / 域6 |

---

## 🔴 CRITICAL（1）

### C-01 ZenMux 自定义 base_url 缺 userinfo(@) 校验 → Bearer key 发往攻击者域名
- **位置**：`src-tauri/src/providers/zenmux.rs:219-243`（URL 门只有 `https://` 前缀 + SSRF 两道检查）；对照 `src-tauri/src/providers/custom.rs:235-248`（H3 fix 已有 userinfo 拦截）；写入口 `commands/mod.rs:523-531` + `src/settings/source-extras.ts:142-167`
- **类别**：安全 ✅ spot-check 坐实
- **问题**：H3 安全审计给 custom.rs 加了 authority-userinfo 拦截（注释明确 "Bearer API key leaks to attacker's server"），但同为用户可配 base_url 的 zenmux 漏了。`url_is_ssrf_blocked` 看到的是 `@` 之后的真实 host（公网域名，不在拦截名单），挡不住这个形态。
- **触发**：设置面板 ZenMux base URL 自由文本框粘贴 `https://zenmux.ai@attacker.com/v1/usage` → url crate 把 `zenmux.ai` 解析为 userinfo、`attacker.com` 为真实 host → 通过全部检查 → **之后每次轮询都把 `Authorization: Bearer <ZenMux key>` 发往 attacker.com**。被投毒的共享 config.json 同样生效。
- **建议**：把 H3 检查抽成 mod.rs 共享 helper（`url_authority_has_userinfo`），custom / zenmux 统一调用；`set_zenmux_base_url` 写入时一并校验。一行级改动。

---

## 🟠 HIGH（6）

### H-01 `parse_hex_color` 字节切片可 panic + 颜色入参全链路零校验 → 启动 tick panic 后轮询永久停摆
- **位置**：`src-tauri/src/tray.rs:828-837`；入参侧 `commands/mod.rs:2268-2280`（set_tray_icon_color 无校验）、`config.rs:996-999`（加载无校验）
- **类别**：边界条件 / 错误处理 ✅ spot-check 坐实
- **问题**：`s.len() != 6` 是**字节**长度；6 字节但含多字节 UTF-8 且偏移 2/4 不在 char boundary 时（如 `"aé123"`），`&s[0..2]` 直接 panic。`tray_fill_color` 被**每条刷新路径**调用（publish_snapshot / refresh_now / set_tray_* / locale 监听器）。同文件已有安全写法 `is_valid_hex_color`（chars + is_ascii_hexdigit）却未复用。
- **触发**：手编 config.json 写入 `tray_icon_color: "aé123"` → 启动首个 tick → publish_snapshot panic → 整个 poller spawn task 死亡，主循环永不启动，**定时轮询永久停摆**（手动刷新仍可用，症状是"不自动更新"且无报错）。
- **建议**：`parse_hex_color` 改 `s.as_bytes()` 逐字节判断（或 `s.get(0..2)`）；写入/加载侧复用 `is_valid_hex_color` 拒非法值。

### H-02 enabled 语义不一致：禁用 base 后副本仍被全量刷新抓取并永久残留陈旧卡片
- **位置**：`src-tauri/src/poller.rs:336-341`（唯一做 unique→base fallback 的地方）vs `commands/mod.rs:1538/1750/614/1892/2003`（全走 `is_enabled_id` 精确匹配）、`config.rs:806-808`
- **类别**：逻辑错误（配置热更新）
- **问题**：poller 主循环注释明确意图"用户关 base 时 extra 也跟着关"，但其余消费点走的 `is_enabled_id` 只查精确 key、缺省 true；副本通常无独立 entry → `is_enabled_id("minimax#2")` 恒 true。（mod.rs:1966 注释甚至声称"双匹配"，与实现不符。）
- **触发**：添加 minimax 副本（默认态无 entry）→ 关闭 base 开关 → per-provider 轮询正确跳过副本，但启动 tick / 手动「立即刷新」仍 fetch 副本，`get_snapshot` 也放行 → 浮窗持续显示 "MiniMax #2" 卡片，且数据在启动拉取后**永久陈旧**。
- **建议**：把 poller 的两级 enabled 逻辑抽成共享函数（如 `cfg.is_enabled_unique(unique, base)`），所有消费点改用；或 `set_provider_enabled(false)` 时级联处理同 base 副本。

### H-03 delete_extra_instance compact 重命名后，幸存实例旧 unique_id 快照永不清理 → 幽灵卡片
- **位置**：`src-tauri/src/commands/extra_instances.rs:585-603`（配合 `commands/mod.rs:641-657/1981-1989` 只增不删合并）
- **类别**：逻辑错误
- **问题**：删除中间副本触发 `compact_indexes_for` 把幸存实例改名（`minimax#3`→`minimax#2`）。清理只处理 target ref；幸存实例**旧身份**在 `state.snapshot` 的条目无任何路径移除（tick/refresh 合并全是 replace-or-push，`get_snapshot` 过滤的 `is_enabled_id` 对无 entry id 默认 true）。
- **触发**：添加 #2、#3 副本后删除 #2 → 浮窗永久多出 "MiniMax #3" 幽灵卡（数据停在删除前），点重试报 unknown source，持续到重启。
- **建议**：compact 后收集所有被改名的 old_refs，同步 retain 掉 snapshot 旧键条目并 emit。

### H-04 order.ts 向下拖拽落点恒定偏移一格（orderIdx 未扣除隐藏源行）
- **位置**：`src/settings/order.ts:296-358`（`onDragMouseUp`）
- **类别**：逻辑错误
- **问题**：拖拽期间源 li 被 `display:none` 但仍在 `children`。`newIdx = children.indexOf(placeholder)` 只做了 divider 修正，没扣隐藏源行 → 向下拖时 `orderIdx` 偏大 1（向上拖不受影响，方向不对称）。错误顺序经 `commitOrder → setProviderOrder` 持久化到后端。行 351-357 注释用"拖到末尾"例子论证不需要 -1，但末尾场景被 splice clamp 掩盖；现有单测只覆盖纯函数，mouseup 索引映射无测试。
- **触发**：`[A,B,C]` 把 A 向下拖到 B、C 之间 → 期望 `[B,A,C]`，实际 `[B,C,A]`（A 直接落到 C 后面）。
- **建议**：divider 修正后补 `if (orderIdx > dragSrcIdx) orderIdx -= 1;`，加端到端索引映射单测。

### H-05 Xiaomi 区域切换调用不存在的命令 `set_xiaomi_region` → 功能彻底断裂
- **位置**：`src/settings/api.ts:280-282` vs 后端注册名 `set_xiaomi_region_field`（`commands/mod.rs:474`、`lib.rs:408`）
- **类别**：IPC 调用不匹配 ✅ spot-check 坐实
- **问题**：后端 `generate_handler!` 只注册了 `set_xiaomi_region_field`，全代码库不存在 `set_xiaomi_region` 命令（config.rs:790 同名方法是 Config 结构体方法，不是 IPC）。
- **触发**：设置 → 数据源 → Xiaomi MiMo 面板切换集群（cn/sgp/ams）→ invoke 必然 reject "command not found"，区域永远保存不了。
- **建议**：invoke 目标改为 `set_xiaomi_region_field`（或后端加 alias）。

### H-06 「应用」section 全局轮询间隔与开机自启是无事件绑定的死控件 → 配置静默丢失
- **位置**：`src/settings/app.ts:17-30、133-144`
- **类别**：逻辑错误
- **问题**：`#interval` 与 `#autostart` 只做了创建和回显，全文件 5 处 addEventListener 均属于其它控件；历史 loadConfig/saveConfig 已在 2026-06-20 audit 删除，api.ts 也没有对应 wrapper。后端 `save_config` 明确会同步 OS autostart（commands/mod.rs:828-836），功能是存在的，只是前端入口断了。
- **触发**：用户修改全局轮询间隔或勾选开机自启 → 无 IPC、无落盘、无报错；重启后还原。
- **建议**：两个控件补 change handler，走 `getConfig → mutate → saveConfig`（与 providers.ts renderIntervalOverride 同款），interval 校验 10..86400。

---

## 🟡 MEDIUM（26）

### 快照 / 实例身份 / 调度

- **M-01 dump CLI 按 base id 加载副本凭据** —— `lib.rs:738` `load_credential_for_id(src.id())`，副本槽位是 `unique_id()`（"minimax#2"）。`musage dump minimax#2` 用**基实例的 key** 拉数据（错账号用量）或误报"未配置"。【域1/2/5 三域命中 + spot-check】改一行 `src.unique_id()`。
- **M-02 refresh_now 合并快照缺 fetched_at 比较** —— `commands/mod.rs:641-657` 无条件覆盖；poller tick 同款竞态已有修复（poller.rs:500-523 比较 fetched_at），此处漏修。已 in-flight 的 per-provider 新数据可被回滚一个周期。【域1/3】移植 poller 的比较逻辑（含顶层时间戳）。
- **M-03 Resized 事件播种 (0,0) 覆盖用户已存浮窗位置** —— `lib.rs:492-508`：setup 里 set_position 发生在 geom persister 注册之前，初始位置不被 Moved 捕获；随后 auto-fit 触发 Resized 进入空槽 `g.unwrap_or((0,0,...))`，flush 对 x/y 无非零防护（Moved 分支的 (0,0) 防护被绕过）。重启后位置丢失回退默认。x/y 与 w/h 分开存，或 flush 补防护。
- **M-04 per-provider interval 无上限 clamp → next_fetch_at 溢出** —— `commands/mod.rs:1952-1958` 只 `.max(10)`，其余所有消费点走 `clamp_interval_secs`（poller.rs:80-91 P1 注释明确要求）。手编 config `refresh_interval_secs = u64::MAX` → `(interval as i64)*1000` 溢出（debug panic / release 回绕 1970），UI 倒计时垃圾值，backoff 记录被 u64::MAX 固化。save_config 也不校验 per-provider 值。【域1/3】改 `clamp_interval_secs` + save_config 补校验。
- **M-05 退避被手动成功清零后不唤醒调度** —— `poller.rs:356-363`：next_fetch deadline 只在 fire 时或 cfg interval 变化时重算。provider 退避到 1800s cap 后用户手动刷新成功（backoff entry 删除），但旧 deadline 仍指向 ~29 分钟后 → 已证实恢复的 provider 自动轮询继续停摆至旧窗口结束。backoff 变化时提前对应 deadline。
- **M-06 enabled 卡片清理与 compact 的配置迁移缺口**（H-03 姊妹问题）—— `commands/extra_instances.rs:585-603`：per-slot 配置（enabled/interval）以 api_key_ref 为键，compact 改名时 `cfg.providers` 不迁移 → [#2(禁用), #3(启用)] 删 #2 → #3 补位成 #2 继承 `enabled=false`，**静默停止轮询并从浮窗消失**；`cfg["minimax#3"]` 变孤儿残留。compact 同一步骤迁移 cfg 条目。

### 配置 / 持久化

- **M-07 best-effort 恢复路径跳过 `migrated()` 且整表替换 providers** —— `config.rs:652-655、887-906`：顶层解析失败走 best-effort 时不补 builtin、不做 schema 迁移；`cfg.providers = parsed` 整体替换 default 预置条目，单条解析失败只 skip。结果：被跳过 provider 的 `is_enabled_id` 缺省 true（**已禁用的重新轮询**）、minimax region 回退 Cn（EN 用户突然请求 CN 端点）。返回前走 `migrated()` 或按 key 合并。
- **M-08 `extra_instances::load()` 解析失败返 `Ok(vec![])` → 下次 save 永久覆盖** —— `config/extra_instances.rs:157-183`：与 keys.json 的 F3 fix（"不静默 fallback 到空 map，返 Err 阻断"）语义相反。文件一次损坏 → 启动无感 → 用户新增一个实例 → 全部老实例从 live 文件消失（仅 .bak 可恢复）。对齐 F3 返 Err。
- **M-09 keys.json tmp 以 0644 短暂落盘后才 chmod 0600** —— `config.rs:1219→1232`（同类：837→849、extra_instances.rs:201→214、logstore.rs:352→360 且 chmod 失败 `let _ =` 静默）：创建→chmod 窗口内多用户机器上可读全部明文凭据。改 `OpenOptions::mode(0o600)` 创建即 0600。
- **M-10 无单实例保护 + 固定 tmp 文件名 → 双开互相覆盖/丢写** —— save_lock 自述"进程级"；Cargo.toml 无 single-instance 插件。双开时：A rename 搬走 B 的 tmp 内容、keys.json read-modify-write last-writer-wins 丢另一方的新 key、启动 cleanup_orphan_tmp 删另一进程在飞的 tmp。加 tauri-plugin-single-instance 或 flock + 唯一 tmp 名。
- **M-11 update_extra_instance 第二步在写锁外写 key** —— `commands/extra_instances.rs:362-392`：第一步锁内落盘 spec 后释放锁，第二步才 save_credential；间隙中 delete 可完整执行 → key 写回**已删除实例**槽位，若 compact 重排编号还可能覆盖他人凭据。且 key/cookie 分两次写，与 add 路径"必须一次写入"结论不一致。移回写锁内 + 合并一次写入。
- **M-12 set_source_credential merge 是锁外 read-modify-write** —— `commands/mod.rs:950→973`：先锁外读旧凭据合并再 save，组合不原子。xiaomi webview 登录刚写入新 cookie，用户随即保存 api key → 旧 cookie 覆盖新登录态；kimi"清除会话"与保存 key 并发 → 已删 cookie 复活。merge 读+写放进同一 save_lock 临界区。

### 登录模块

- **M-13 xiaomi 提取重试日志未脱敏** —— `xiaomi_login.rs:415`：P3 audit 为此实现了 `redact_url_for_log`（286、306 都用了），唯独 415 漏掉；且该分支恰是 URL 不在 dashboard 时触发——记录的正是携带 ticket/serviceToken 最多的 SSO 中间 URL。改 `redact_url_for_log(&current_url)`。
- **M-14 xiaomi EXTRACTING 锁被旧代任务抢占后永久卡死** —— `xiaomi_login.rs:216 vs 284-304 vs 113-119`：`open_xiaomi_login_window` 先 `EXTRACTING.store(false)` 再关旧窗（≤2s 窗口）；旧窗最后一次 on_page_load CAS 成功 → spawn 旧代任务 → gen 不等的 guard drop **拒绝复位** → EXTRACTING 永久 true → 新登录窗所有 page load CAS 失败，且 xiaomi 无轮询 deadline → 新登录**静默挂死**。把 store(false) 移到 wait_window_closed 之后，或补兜底超时。
- **M-15 xiaomi userId URL 兜底白名单不含 `/console/*`** —— `xiaomi_login.rs:563-565 vs 122、134-140`：兜底白名单只有 /dashboard、/oauth、/，但 LOGIN_URL 本身就是 `/console/plan-manage`、`is_dashboard_url` 也接受 /console。WKWebView cookie jar 缺 userId cookie（代码显式处理的场景）+ 页面在 /console → 兜底返 None → 5 次重试全失败，反复重登无法成功。白名单与 is_dashboard_url 对齐。
- **M-16 anysearch READY cookie 15min wall-clock vs 14min monotonic deadline** —— `anysearch_login.rs:266 vs 462 vs 531`：READY 只写一次 `max-age=900`（墙钟），deadline 用 Instant（休眠不走时）。登录窗开着合盖休眠数分钟 → READY 过期而 deadline 未到 → 之后**即使用户成功登录也永不接受**，最终 Timeout。READY 首次见到后锁存，或放宽 max-age。

### Provider 框架 / 实现

- **M-17 SSRF 防护可被 DNS 域名整体绕过** —— `providers/mod.rs:263-285`：Domain 分支只匹配 `d == "localhost"`，从不做 DNS 解析后 IP 校验。`https://169.254.169.254.nip.io/latest/meta-data/...` 直接穿透（公共泛解析零基础设施），云元数据响应进 snapshot.raw emit 前端 + 落 logstore，请求同时送达该 source 的 Bearer key；3xx 重定向同样绕过。与项目五轮 SSRF 加固的威胁模型直接冲突。连接前解析 host 并对 IP 重跑判定。
- **M-18 NewApi relay 层失败一律归类 AuthFailed** —— `custom.rs:375-388`：New API 中转站在 HTTP 200 上用 `{success:false}` 上报一切失败（余额不足/限流/relay 内部错误）。AuthFailed 使 `needs_settings()` 返 true → 浮窗引导"重填 key"，key 本身有效形成误导循环。relay 失败默认归 Other，仅明确鉴权错误才 AuthFailed。
- **M-19 StepFun 所有行 `kind: None` → 托盘图标永不渲染其数据** —— `stepfun.rs:759、782、802`：tray `pick_tray_rows` 只按 RowKind 枚举匹配百分比行，stepfun 三种行 kind 全 None 且 remaining 全 None → 退化为静态 logo。与 minimax M2 fix 同类 bug，minimax/kimi/zhipu/xiaomi 都已填，唯独 stepfun 漏。"图标空白但 tooltip 有数"的分裂状态。两行分别填 FiveHour / Weekly。
- **M-20 DeepSeek 副本实例 health 特判失配 → 钱包不可用仍显示绿色** —— `deepseek.rs:236-247` + `mod.rs:558-582`：`health_label` 按 `source_id == "deepseek"` 精确匹配，副本 source_id 是 `"deepseek#2"` → 落入默认分支 u=0.0 恒 "ok"。特判改按 base id 匹配（`split('#').next()`）。
- **M-21 anysearch refresh 竞态：锁内不重读 keys.json** —— `anysearch.rs:243-254、365-390`：REFRESH_LOCKS 按 unique_id 串行化调用本身，但 refresh token 在进锁**前**拆出。single-use rotation 下并发两路拿到同一 R0：A 成功（R0 作废写盘）→ B 用 R0 → 40114 → 误报"请重新登录"（盘上其实是刚续期的有效 token）。手动 refresh_single 不查 in-flight、与 poller tick 可并发，真实可达。拿锁后重读盘上 combined，已续期则直接返回。
- **M-22 zenmux `account_status` 未反映到 is_healthy** —— `zenmux.rs:464-476、487`：suspended/unhealthy 只拼进展示字符串，is_healthy 硬编码 true。对照组 siliconflow.rs:241-245 对同类字段正确处理。停用账号浮窗仍亮绿点。改 `is_healthy: account_status.is_empty() || account_status == "healthy"`。

### 前端

- **M-23 extra instance 凭据状态徽章永远停在占位文案** —— `settings/main.ts:167-202、providers.ts:493-495`：init 只遍历 `listSources()`（内置），副本/custom 的徽章不在范围；rebuildProvidersSection 也不调 loadAllCredentialStatus。只有恰好点过保存才顺手更新。init 与 rebuild 用 allSources 调 loadAllCredentialStatus。
- **M-24 浮窗 auto-fit IPC 失败后 lastFitContentH 已推进、无重试** —— `main.ts:661-666、619-643`：grow/shrink 都先记账再调 IPC，invoke 失败仅 console.debug；observer 去重条件恒成立 → 窗口卡在旧高度直到内容结构变化。lastFitContentH 更新挪到 invoke 成功后。
- **M-25 providers.ts 启用开关 IPC 失败 checkbox 不回滚** —— `providers.ts:226-232`：withSuppress 的 finally 会恢复顺序列表，但 checkbox 本身停在用户切换后的值 → 两处状态矛盾（order.ts 拖拽路径有显式回滚）。catch 里 `checked = !target`。
- **M-26 renderIntervalOverride 整 cfg read-modify-write 无互斥** —— `providers.ts:303-323`：getConfig 与 saveConfig 之间若发生其它 cfg 写入（相邻 interval 同时改 / per-field setter / 登录落盘），后写者用旧快照整体覆盖。改单字段 command 或串行化。

---

## ⚪ LOW（41）

### 应用外壳 / IPC（8）
| # | 位置 | 问题 |
|---|---|---|
| L-01 | commands/extra_instances.rs:326-358 | update 凭据迁移读失败静默跳过，但旧槽随后仍删 → 实例指向空槽、凭据孤儿残留 |
| L-02 | commands/extra_instances.rs:585-603 | （见 M-06，compact 配置继承同源）cfg.providers 孤儿条目不清理 |
| L-03 | commands/mod.rs:715-728 | save_config 全量路径绕过 set_schema_overrides（单 tier 64 上限）/ sanitize_provider_order 校验 → 手搓 IPC DoS 面 |
| L-04 | commands/mod.rs:1429-1465 | set_region 接受 "custom" 却强制套 CN 默认 endpoint，覆盖用户手动定制 |
| L-05 | commands/mod.rs:1216-1220 | open_settings_window 固定 sleep 150ms 再 emit settings-navigate，慢机丢导航（事件无回放） |
| L-06 | lib.rs:521-551 | geom persister 只听 SHUTDOWN.notified()，无 SHUTDOWN_REQUESTED 兜底（对照 poller.rs:233）→ 退出前 <500ms 拖动丢失 |
| L-07 | commands/mod.rs 全部 cfg.save() 点 | 持有 tokio RwLock 写锁期间做同步磁盘 IO（tmp+fsync+rename），慢盘阻塞 worker 与所有读者 |
| L-08 | lib.rs:353 | 首启引导创建设置窗失败 `let _ =` 静默吞，新用户卡死无日志 |

### 配置 / 持久化（5）
| # | 位置 | 问题 |
|---|---|---|
| L-09 | logstore.rs:227-241、382 | 注释声称 ~1/200 频率 truncate，实际 ring 满后每次 push 全文件重写（当前 push 频率低，影响有限） |
| L-10 | logstore.rs:299-313 | append/truncate 交错产生瞬态重复条目；恰在 B append 后崩溃 → 重复持久化顶掉真实最老条目 |
| L-11 | config.rs:242-250 vs 558-560 | color_thresholds 顺序约束写路径校验、load 路径不校验，手改 `[90,70,50]` 直接进渲染 |
| L-12 | config.rs:842-851、1221-1238 | fsync 失败被 `if let Ok`/`let _ =` 忽略、rename 后未 fsync 目录 → 掉电可丢最后一次保存 |
| L-13 | logstore.rs:180-192 | 启动全量解析 pre-H6 遗留大 jsonl（可几十 MB）进内存后才截断 → 内存尖峰 |

### 轮询 / 托盘（4）
| # | 位置 | 问题 |
|---|---|---|
| L-14 | tray.rs:750-758 | `pick_tray_rows` 用 max_by，并列取**最后一个**副本，与注释"取 instance_index 小"相反；余额系多实例托盘显示末位副本 |
| L-15 | tray.rs:1238-1276 | tooltip error/balance 分支漏过 sanitize_tooltip_segment（percent 分支有）→ 换行/控制字符注入伪行 |
| L-16 | tray.rs:127-138、363-382 | 托盘左键 500ms 校验用 SystemTime 墙钟 → NTP 前跳丢真实点击 / 后跳放行合成事件。改 Instant |
| L-17 | commands/mod.rs:212-228 | set_provider_enabled placeholder 的 next_fetch_at 缺 base_id interval fallback（M18 同款漏改），倒计时与实际调度不一致 |

### 登录（4）
| # | 位置 | 问题 |
|---|---|---|
| L-18 | stepfun_login.rs:416-419 | 新鲜度门缺 60s skew，与 kimi/anysearch（均有）及注释声明不一致 → 寿命最后 60s 的 token 被接受 |
| L-19 | kimi_desktop.rs:137-147 | immutable 兜底 SQLite URI 未百分号编码（`?#%`/空格），且 Path::display 非 UTF-8 有损 → 兜底打开错误文件静默降级 |
| L-20 | anysearch_login.rs:53-56 vs 232-251 | 文档称非受信域"保持原生行为"，实际全局 override + 门禁返空/null —— 正是文档声称已修的行为，可能破坏第三方 SSO |
| L-21 | xiaomi_login.rs:404-411 | 提取窗口期用户手动关窗 → url() Err 冒泡 emit failed toast，违背"用户关窗静默退出"语义（其余三家均有 Cancelled 分支） |

### Provider 框架（4）
| # | 位置 | 问题 |
|---|---|---|
| L-22 | providers/mod.rs:958-966 | 自定义 redirect policy 丢失 reqwest 默认 10 跳上限 → 重定向环空转到 10s 超时（有 timeout 兜底） |
| L-23 | providers/mod.rs:942-970 | shared_client 未 `.https_only(true)` → 3xx 可 https→http 降级跟随，打破 H9 不变量（reqwest 跨 host 会剥 Authorization，泄露面小） |
| L-24 | providers/mod.rs:709-716 等 | ~10 个空 set_state 的 source（deepseek/kimi/stepfun/claude/tavily/siliconflow/openrouter/tokendance/custom…）未 override `needs_state_update=false` → 每次 fetch 白序列化整个 AppConfig。【域5/6 交叉命中】 |
| L-25 | providers/mod.rs:464-469 | empty_error 每个错误快照调两次 find_source（两次 all_sources 全量重建）；断网全源失败时 28 次构造 |

### Provider 实现（9）
| # | 位置 | 问题 |
|---|---|---|
| L-26 | config/extra_instances.rs:147-184 | load 路径不复核 spec.id 唯一性 → 手编重复 id 导致 poller/backoff/snapshot/凭据槽全碰撞（写路径有 sanitize） |
| L-27 | minimax.rs:685-688 | smart_reset_to_ms duration 分支 `raw*1000` 无 checked_mul → raw≥~9.2e15 时 debug panic / release 回绕 |
| L-28 | xiaomi.rs:811-818 | utilization 无 [0,100] clamp（其余 5 家都有防护）→ 异常响应进度条越界渲染 |
| L-29 | xiaomi.rs:405-426、522-571 | 唯一没有 429→RateLimited 分类的 provider（退避行为无差异，仅错误文案错位） |
| L-30 | stepfun.rs:935-974 | normalize_oasis_token：无 `;` 的单 cookie "Cookie: Oasis-Token=…" 粘贴不剥前缀 → 好 token 被污染报 401 |
| L-31 | zhipu.rs:190-200 | display_name() 忽略 region，国际版（api.z.ai）用户永远看到国区名；region-aware 的 display_label 是 dead_code |
| L-32 | zhipu.rs:235/283/288-295/311 + xiaomi.rs:944 | i18n 参数硬编码中文（"智谱 GLM"/"国区"/"(到期时间未知)"）→ en locale 半中半英文案 |
| L-33 | anysearch.rs:467-480 | 主请求业务码检查只认数字 code（refresh 路径 json_i64 兼容字符串，两处不一致）→ 字符串 code 漏检降级成 Parse 错误 |
| L-34 | siliconflow.rs:198-209 | status=false 分支 code 硬编码 0，真实错误码（40000/50000 段）丢失 |

### Provider 实现（续）/ 前端（7）
| # | 位置 | 问题 |
|---|---|---|
| L-35 | zenmux.rs:165-174 | set_state 无法清空已设置的 base_url（空串/缺字段被跳过）——当前每次 fetch 新实例不可观测，latent |
| L-36 | zenmux.rs:504-513 | usage_percentage ratio/percent 启发式在 ≤1.0 区间歧义，schema 漂移时低用量放大最高 100 倍；应优先用 used_flows/max_flows |
| L-37 | anysearch.rs:93-98 | 同一账号 token 配到两个实例时跨实例 refresh 互相作废（锁按 unique_id 分桶）；与 M-21 同款修复可覆盖 |
| L-38 | main.ts:1168、1208 | textContent 赋值里调 escapeHtml → 特殊字符 unit 显示字面实体 `&amp;` |
| L-39 | region-wizard.ts:25-28、87-107 | 首启自动 setRegion("global") fire-and-forget 与向导回显竞态 → radio 显示旧 CN，用户不改动点 Apply 不纠正任何事 |
| L-40 | settings/main.ts:218-233 | set_region 改 minimax/zhipu 区域后 config-changed 只重建顺序列表，区域下拉回显陈旧 |
| L-41 | main.ts:645-648、1604-1614 | hover 主动 fit 撞上 observerBusy 被静默跳过且无重试（box-shadow 变化不触发 ResizeObserver）→ 窗口高度带 hover 余量停留 |

---

## 修复优先级建议

**P0（立即）**：C-01（泄钥，一行级改动）；H-05（xiaomi region 功能断裂，改 invoke 名）。
**P1（本批）**：H-01（panic→轮询停摆）、H-02（enabled 语义）、H-03+M-06（compact 身份迁移，建议合并设计："快照/配置的身份键迁移随 compact 同步"）、H-04（拖拽偏移）、H-06（死控件）。
**P2（顺手一行/小段）**：M-01（dump unique_id）、M-04（clamp_interval_secs）、M-19（stepfun 填 RowKind）、M-22（zenmux is_healthy）、L-24（needs_state_update 批量补）。
**P3（排期）**：M-17（DNS-SSRF，与既定威胁模型冲突，建议与 C-01 同批）、M-10（single-instance）、M-21/L-37（refresh 锁内重读）、其余 Medium。
**Low**：随相关模块改动顺手清理。

## 已核验无问题的重点项（防误报备忘）

- **2026-06-20 死锁修复无回归**：write_keys_atomic 无内部锁，save_lock 全仓 5 个获取点互不嵌套、无跨 await 持锁。
- **Tavily→Minimax enum 占位 footgun 彻底清除**（B 组 8 文件 provider 字段全对齐）。
- **builtin_sources 状态丢失历史坑已正确修复**（refresh 路径对同一实例先 update_source_state 再 fetch）。
- **剩余/已用百分比语义 14 家全部核验正确**，无搞反；除零（total=0/limit=0）全防护；时间戳 `<1e12→秒` 智能判定无单位混淆。
- **MiniMax 双 schema 分支**（percent-first → count-fallback + status 门控）与文档一致，有回归测试锁定。
- **StepFun 续期链**（BUG-001 per-unique_id Mutex + 锁内重读复用）完整；anysearch 写回按 unique_id 与多实例隔离对齐。
- **前端**：Tauri 2 camelCase 参数名与 Rust 签名一致；CSP `script-src 'self'` + innerHTML 全走 escapeHtml/textContent，无注入点；listen 均有 unlisten 跟踪；offsetTop 测量方案坐标系自洽。
- **生产 Rust 路径无裸 unwrap/expect/越界索引**（providers 全部 14 家 + 登录 5 模块扫查确认）。
- **前后端 17 个 `musage://` 事件名完全一致**；sanitize_custom_spec_id 对 `:`/`#`/内置碰撞拦截完整；Windows apply_z_order 全局锁串行化、macOS Retained<NSWindow> use-after-free 已修。
