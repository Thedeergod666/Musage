下面给出对当前 `HEAD = 9cdbb1c` 的全量审查 D4 报告。我把范围严格限定在仍存在并可在当前代码路径上重现的问题，**已修复**或仅是修复建议且当前代码已合规的条目不再列入。每条按 ID `D4-xxx` 编号，含置信度、文件:行号、触发条件、根因、影响、调用链证据和最小修复建议；只暴露当前未修补面。

---

## 概览

- 范围:`config.rs` / `config/extra_instances.rs` / `commands/{mod,extra_instances,i18n}.rs` / `providers/parse.rs` / `logstore.rs` / `lib.rs` / `settings/advanced.ts`
- 体量: 6570 行 Rust + 380 行 `settings/advanced.ts` / `api.ts` / `extra-instance-form.ts` 直接相关
- 审查时间: 2026-07-30,代码基线 `9cdbb1c`
- 旧报告 60 条已修/或与当前代码冲突的项不计;新发现 1 P0 / 4 P1 / 8 P2 / 6 P3,合计 19

---

## 🔴 P0（必须先修）

### D4-001 损坏的现代 `config.json` 会被旧版解析器吞掉,直接落 `AppConfig::default()`
- **置信度**: 高
- **位置**: `src-tauri/src/config.rs:530-600`
- **触发条件**: `serde_json::from_str::<AppConfig>(s)` 失败且旧版 `Legacy` 结构体也解析失败时,函数走"备份 + best-effort"分支。但 `best_effort_from_value` 仅在 `serde_json::from_str::<Value>` **也** 成功后才被调用 —— 一旦损坏文件根本不是合法 JSON(例如部分写入截断 / 末尾缺 `}` / 多了尾部逗号),`Value` 解析也会失败,函数直接 `return Ok(Self::default())`。
- **根因**: `load_from_disk` 在最坏情况下对损坏文件执行三步降级:① 新 schema 失败 ② Legacy schema 失败 ③ `best_effort_from_value` 失败 → 返回 `default()`。前两步已经把"全字段齐全的最新配置"判死,第三步没法恢复。`save_lock` 不参与加载路径,因此一旦进程内某 IPC 调 `save_config` 触发 `cfg.save()`,完整的现代配置会**被默认值永久覆盖**。
- **影响**: 升级过程中掉电 / 内核 panic / 用户编辑器误改 / 写一半被 SIGKILL,会触发完整配置清零;旧报告 H5 修了"部分字段保留",但**完全损坏**仍走 default 路径。
- **证据 / 调用链**:
  1. `config.rs:530` `serde_json::from_str::<AppConfig>(&s)` 失败
  2. `config.rs:546` `from_str::<Legacy>(&s)` 也失败
  3. `config.rs:594` `if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s)` 解析也失败
  4. `config.rs:599` `return Ok(Self::default())`
  5. `lib.rs:119` 把 `default()` 写进 `AppState.config`
  6. 任意 IPC(例如 `set_provider_enabled` / `save_config`)在 `commands/mod.rs:134/657` 调 `cfg.save()` 写回磁盘
- **最小修复**: 在 `load_from_disk` 解析全失败时,直接把整文件原样复制到 `config.json.damaged.<ts>` 之后**返回 Err**,让 `lib.rs:119` 的 `unwrap_or_default()` 失败兜底改为 **首次启动默认值 + 标记 IO 警告日志**;同时把 `AppState` 设为 "磁盘存在但解析失败" 状态,在 setup 阶段发出系统通知,让用户知道有备份可恢复。或者最少:在 `return Ok(Self::default())` 前要求用户确认或写 `*.locked` sentinel,阻止 `save()` 覆盖。

### D4-002 进程级 `save_lock` 是 `std::sync::Mutex` 但调用方在 `tokio::spawn` 上下文中持锁跨越 `.await`
- **置信度**: 高
- **位置**:
  - 锁定义 `src-tauri/src/config.rs:50-53`
  - 锁文档警告 `src-tauri/src/config.rs:46-49`
  - 持锁调用方 `src-tauri/src/commands/mod.rs:923` `delete_source_credential` cascade 内的 `cfg.save()` 在 `state.config.write()` 守卫内调用
- **触发条件**: 启动后任意 IPC 触发 `delete_source_credential(minimax)` 删 builtin key,且 `state.config.write().await` 与 `state.extra_instances.read().await` 锁顺序契约被破坏,使得 cascade 路径下两个 tokio task 同时争用 `save_lock`,持锁线程在 cfg.save() 期间被 tokio 调度切到其他 runtime 任务。`std::sync::Mutex` 不可重入,且若持锁线程让出给另一个也在等 `save_lock` 的 future,死锁。
- **根因**: `save_lock` 是 `std::sync::Mutex<()>`,但 `save()` / `write_keys_atomic` / `extra_instances::save()` 的调用方都在 `tokio::runtime` 上下文内,临界区里又会被 `#[tokio::main]` 派发。`std::sync::Mutex` 在 Windows / Linux 上没绑定特定调度器,但若持锁 task 在持锁期间 await 别的 `tokio::sync::Mutex`(`state.config.write()`),就出现"std Mutex 持锁 + tokio Mutex 阻塞"链路。Audit L2 (2026-07-30) 只是**记**了这个隐患,没改锁类型。
- **影响**: 概率低(主要是同时多 IPC 触发),但一旦发生会永久挂住 IPC handler,用户视角是"设置面板保存按钮死锁",只能 kill -9。无可观测警告。
- **证据 / 调用链**:
  1. `commands/mod.rs:912-928` `delete_source_credential` 取 `state.config.write().await` → `cfg.save()`
  2. `cfg.save()` 内部 `config.rs:755` 拿 `save_lock().lock()`
  3. 持锁期间 Rust 异步 runtime 仍可调度该 task 让出
  4. 任意其他持锁任务 `set_provider_enabled`(已 持有 `state.config.write().await` 守卫) 也在等 `save_lock`,形成 `config.write → save_lock → ...` 反向链
- **最小修复**: 改 `save_lock` 为 `tokio::sync::Mutex<()>`,在 `save()` / `write_keys_atomic` / `extra_instances::save()` 入口包 `tokio::sync::Mutex::lock().await`;或在每个调用方把 `cfg.save()` 移到 `spawn_blocking` 内执行,且**所有**持久化点都迁移。

---

## 🟠 P1（下次发布前修）

### D4-003 `add_extra_instance` 锁外写 `keys.json` 用临时 `api_key_ref`,并发添加时仍会把两个 instance 共享同一个 key
- **置信度**: 高
- **位置**: `src-tauri/src/commands/extra_instances.rs:184-221`
- **触发条件**: 两个 `add_extra_instance(deepseek, ...)` IPC 并发触发(例如设置面板"添加副本"按钮在多窗口打开 + 同一用户连点两下)。两者都在拿 `state.extra_instances.read()` 之前,基于读到的快照算出同样的 `tentative_idx`。
- **根因**: 旧报告 (C3) 把"读 → 算 index → push"放进同一 write lock 解决了 push 阶段重复,但**写 key 阶段仍在锁外**。`temp_api_key_ref = "{provider_id}#{tentative_idx}"` 在两个并发路径上相同,两者各自调 `save_credential_for_id(&temp_api_key_ref, ...)`;第一次 `try_rename_key` 把 `deepseek#2` 改名 `deepseek#3`(并删 temp),第二次的 rename 目标也是 `deepseek#3`,**先把 `temp_api_key_ref` 复制成 `deepseek#3`**,再删 temp。第二次的 `deepseek#3` 写入覆盖第一次,首次的 instance 现在指向 `deepseek#3` 但其 key 已被第二次的 instance 覆盖。
- **影响**: 两个 builtin 副本最终只剩一份有效的 key;另一 instance 静默"未配置",poller 拿不到 key,显示"未配置"红卡,用户重启前看不到修复路径。
- **证据 / 调用链**:
  1. `extra_instances.rs:193` 读 lock 算 `tentative_idx`(两并发同值 N)
  2. `extra_instances.rs:219` 各自 `save_credential_for_id(temp_api_key_ref, &cred)` 把 key 写到 `deepseek#N`
  3. `extra_instances.rs:230` 串行进入 write lock,实际 index = N+1(后者),target = `deepseek#N+1`
  4. `extra_instances.rs:265-269` `try_rename_key(temp_api_key_ref, final_api_key_ref)` 把 `deepseek#N` 的 key 复制到 `deepseek#N+1`,**复用同一个 key 内容**
  5. 后到的 instance 也走相同逻辑,最终两份 instance 共享同一 key
- **最小修复**: 在拿 `state.extra_instances.read()` 之后,把 `temp_api_key_ref` 用一个**真随机** suffix(例如 `format!("{provider_id}#tmp-{uuid}")`),即使并发写,各自的 temp key 不冲突;然后在 write lock 内 `next_index_for` 算真实 index 后做 `temp → final` rename,删除 temp。

### D4-004 `delete_extra_instance` 失败回滚只恢复 `extras` 列表,不撤销已经在 `keys.json` 上做的 compact 迁移
- **置信度**: 高
- **位置**: `src-tauri/src/commands/extra_instances.rs:488-491`
- **触发条件**: 用户先点"删 minimax#2",`compact_indexes_for` 把 minimax#3 改名为 minimax#2(`api_key_ref` 已重写),`load_credential_for_id("minimax#3")` + `save_credential_for_id("minimax#2", ...)` 成功;但 `extra_instances::save(&extras)` 失败(磁盘满 / 权限)。
- **根因**: 锁内的回滚路径 `*extras = extras_snapshot` 恢复了内存中 `extras` 列表,但 `api_key_ref` 已经在 keys.json 上重命名过。函数 `return Err(e)`,前端显示错误,但 minimax#2 旧 key 已经被删除,minimax#3 key 已经被改成 minimax#2。重启后 `read_keys` 看到 minimax#3 不存在,minimax#2 有凭据;`extras` 列表里 minimax#2 仍存在(因为快照恢复),`minimax#3` 也在 —— 出现两条 instance 但 keys.json 只剩一份,poller 拿到一次 OK,一次"未配置"。
- **影响**: 用户看错误提示以为是普通失败,实际数据已部分迁移;重启后才能看到不一致。
- **证据 / 调用链**:
  1. `extra_instances.rs:441-473` 成功完成 `save_credential_for_id(new, old_cred)` + `delete_credential_for_id(old)`(但 old 已经写入新 key)
  2. `extra_instances.rs:488-490` save 失败 `*extras = extras_snapshot; return Err(e)`,extras 回滚但 keys.json 不回滚
  3. `extra_instances.rs:485` 之后还执行 `delete_credential_for_id(&target_api_key_ref).ok()` —— target 是删除前的 instance,刚好对得上;但 compact 阶段的迁移已经发生
- **最小修复**: 在 save 失败的回滚路径里,把 `api_key_ref` 也改回 old 引用,并在 keys.json 上反向重命名(`save_credential_for_id(old, &new_cred)` + `delete_credential_for_id(new)`);或者把 compact 迁移推迟到 save 成功之后再做。

### D4-005 `read_keys` 损坏文件 backup 走 `let _ = std::fs::copy(...)` 静默吞错
- **置信度**: 高
- **位置**: `src-tauri/src/config.rs:1097-1098` 与 `:1112-1117`
- **触发条件**: keys.json 损坏 + backup 失败(read-only 文件系统 / 满盘 / 权限) → 用户启动时 `read_keys` 静默走 Err 路径,前端显示"读 keys.json 失败",但 `save_lock` 仍能拿到;任何 IPC(set / delete credential)会调 `write_keys_atomic` 用新内容**覆盖**这个损坏但**尚未被 backup** 的文件。
- **根因**: `read_keys` 错误处理把 backup 失败当成"可忽略",但 M7 fix(2026-07-06)没改 `let _ = std::fs::copy(...)` 的静默吞错。错误信息被合并成 `commands.parse_keys` 给前端,前端 catch 后只显示"凭据读失败",用户不知道 backup 是否成功。
- **影响**: 真实数据丢失在两个场景:① read-only 卷导致 backup 失败,save 覆盖原损坏文件 → 原始内容(可能是用户刚刚粘贴的临时有效 key 残段)丢失;② 满盘导致 backup 失败,新 save 仍然写入 → 错误循环。
- **证据 / 调用链**:
  1. `config.rs:1106-1119` 损坏 keys.json → backup 失败 `let _ = std::fs::copy(...)` 静默
  2. 进程继续,任何 IPC 触发 `write_keys_atomic`(config.rs:1072)成功 rename,但旧文件内容已被丢失
- **最小修复**: backup 失败时记录 error 级日志(不是 trace/debug)并把"读失败但未备份"的状态写进 `AppState` 标记,让后续 `write_keys_atomic` 在写入前再尝试一次 backup,或者把 `Err` 透传给前端,让用户先处理磁盘问题。

### D4-006 `delete_extra_instance` 在 `target_api_key_ref` 已删除但 compact migration 还没走完的窗口里被并发 `add_extra_instance` 撞
- **置信度**: 中
- **位置**: `src-tauri/src/commands/extra_instances.rs:441-485` + `:485`
- **触发条件**: 用户并发触发 `delete_extra_instance(deepseek#3)` 和 `add_extra_instance(deepseek, key3)`,前者走到 `:485` `delete_credential_for_id(&target_api_key_ref).ok()` 时,后者的 write lock 在排队;前者释放 write lock,后者进入,把 `temp_api_key_ref = "deepseek#4"` 写进 keys.json,但前者**已经**删过 `target_api_key_ref = "deepseek#3"`(因为 extra_instances 里 `#3` 已被 remove,compact 把 `#4` 改成 `#3`,最终 deepseek#3 是新副本的 key)。
- **根因**: 跨 write lock 的连环交错:`delete` 的 compaction 改写 `api_key_ref` 字段,但 `delete_credential_for_id(&target_api_key_ref)` 用的是 remove **前**的旧 ref(已经在该锁内读取的快照是早期值),没有做"是否已经被 compact 改写过"的二次确认。
- **影响**: 删错 key 静默进行,用户视角是"删除后下次重启少一个 provider 数据",debug 极难。
- **证据 / 调用链**:
  1. `extra_instances.rs:415` 在 write lock 内 `target_api_key_ref = extras[pos].api_key_ref.clone();` —— 这是删除**前**的 ref
  2. `extra_instances.rs:429` `compact_indexes_for` 改写其他 instance 的 `api_key_ref`,但变量 `target_api_key_ref` 仍是早期值
  3. `extra_instances.rs:485` `delete_credential_for_id(&target_api_key_ref)` 删的是"原 deepseek#3" —— 巧合,但如果并发 `add_extra_instance` 在前一步创建了 `deepseek#3`,这里会**误删新建副本的 key**
- **最小修复**: 在 `delete_credential_for_id(&target_api_key_ref)` 之前,重读 `extras` 中 target 的 ref 是否已经改写;或干脆用 `*_credential_for_id(&extras[pos].api_key_ref)` 在 compact 之后做。

---

## 🟡 P2（v0.2.6 周期修复）

### D4-007 启动时 `truncate_old_backups` 只清 `config.json.bak.*`,不清 `keys.json.bak.*` / `app_log.jsonl.tmp` 孤儿
- **置信度**: 高
- **位置**: `src-tauri/src/config.rs:495-499` + 缺失的 `keys.json` / `app_log.jsonl` 启动清理
- **触发条件**: 升级到 0.2.5 + 之前已经触发过 keys.json 损坏/空文件 → `.bak.<ts>` 累积超过 5 份不会自动清理;同样 `app_log.jsonl.tmp` 残留孤儿也不会清。
- **根因**: L4 fix(2026-07-30)只给 `config.json` 调了 `truncate_old_backups`,但 `keys.json` / `app_log.jsonl` 写路径都有 `.bak.<ts>` / `.tmp`(logstore.rs:387-390)生成,启动期没对称清理。
- **影响**: 长跑用户(2-3 年升级跨度)磁盘增长;`app_log.jsonl.tmp` 孤儿永久占空间。
- **最小修复**: 在 `lib.rs:117-118` setup 阶段调 `truncate_old_backups(cfg_dir, "keys.json", 5)` 与 `truncate_old_backups(cfg_dir, "app_log.jsonl", 5)`,并在 `logstore::load_from_disk` 末尾 `let _ = std::fs::remove_file(log_path.with_extension("jsonl.tmp"))`。

### D4-008 `save_config` 接受整个 `AppConfig` JSON 但不校验 `floating_x/y/w/h` 范围,负数 / 极值会破坏 `position_is_visible`
- **置信度**: 高
- **位置**: `src-tauri/src/commands/mod.rs:593-687`
- **触发条件**: 前端/外部绕过 `set_provider_enabled` 直接 `invoke("save_config", {cfg: {...floating_x: 2_000_000_000}})` → 落盘后下次启动 `lib.rs:486-500` 写进 `state.config`;`reset_floating_window` 调 `win.outer_position()` 后把 `(2_000_000_000, ...)` 写入 cfg,后续 `position_is_visible` 永远 false,poller 触发归位。
- **根因**: H2 fix(2026-07-29)只加 `providers` map cap,没加 `floating_x/y/w/h` range check。`set_provider_enabled` 等单字段 setter 没改 cfg 的 floating 字段,所以攻击面只剩 `save_config` 直接调用,需要前端或脚本主动触发。
- **影响**: DoS(浮窗归位循环)+ 配置文件被巨大数污染。
- **最小修复**: 在 `save_config` 入口加 `if cfg.floating_x.is_some_and(|n| !(-32768..=32767).contains(&n))` 等 range check;`floating_w/h` 同理(0..=2400),超过 reject。

### D4-009 `save_config` 不限制 `provider_order` / `schema_overrides` 数量,IPC DoS 面仍存
- **置信度**: 中
- **位置**: `src-tauri/src/commands/mod.rs:593-687`
- **触发条件**: 外部传 `provider_order: Vec<String>` 含 10 万条 → `cfg.save()` 写盘前 `serde_json::to_string_pretty` 在 runtime thread 上 alloc 几百 MB,IPC handler 阻塞。
- **根因**: H4 fix 加了 `providers` map cap (256) 与 `refresh_interval_secs` 上下限,但 `provider_order` / `schema_overrides` 仍无 cap。
- **影响**: DoS,与 H4 报告同性质但未完全补完。
- **最小修复**: `provider_order.truncate(128)`,`schema_overrides` 同样 cap,例如 64 个 provider 限制。

### D4-010 `LogStore::push` 容量 cap 后每次 push 都触发全文件重写,poller 高频错误场景下慢盘卡 IPC
- **置信度**: 高
- **位置**: `src-tauri/src/logstore.rs:216-225` + `:284-293`
- **触发条件**: poller 每 60s 一次全量刷新,12+ provider × 持续鉴权失败 → `log_provider_error` 调 `state.log.push` 高频触发,达到 `MAX_ENTRIES = 200` 后每条 push 都让后台 worker 整文件重写 `app_log.jsonl`(200 行 × ~200 字节 ≈ 40 KB)。
- **根因**: M1 fix 把 truncate 移到后台 worker 是为了不阻塞 hot path,但 truncate 频率没限制,只是 push 频率。
- **影响**: 慢盘(NAS / 网盘 / 满载 HDD)下每分钟多次 40KB 写,后台 worker 队列堵死,`tx.send()` 返 Err(报告里的 M3 修复点)就触发,`tracing::error!` 频繁喷日志。
- **最小修复**: `needs_truncate` 改成 `if needs_truncate && last_truncate.elapsed() > 30s { truncate }`,节流到每 30s 一次;或用 `MAX_BYTES` 触发而不是按条数。

### D4-011 `load_from_disk` 启动不脱敏历史日志,`get_recent_logs` IPC 直接返回未 redact 的明文
- **置信度**: 高
- **位置**: `src-tauri/src/logstore.rs:160-197` + `commands/mod.rs:2082-2089`
- **触发条件**: 用户在 0.2.4 之前的版本下曾把 stepfun access token 当 Bearer 报错记到 `app_log.jsonl`,升级到 0.2.5 后启动 `LogStore::load_from_disk` 直接 push 进内存 ring buffer(`buf.push_back(entry)`),没走 `redact_message`。前端调 `get_recent_logs(200)` 拿到原文,渲染到设置面板"日志"标签页。
- **根因**: H3 fix(2026-07-29)只给 `LogEntry::error/warn/info` 构造器加了 redact;启动加载 + 直接 deserialize 的旧日志没走。报告里 M3 建议对 `load_from_disk` 也加,但 commit `b179b27` 实际只改了构造器。
- **影响**: 升级用户在前端能直接读到 `Bearer eyJ...` / `Oasis-Token=...` 原文(redact 仅在 push 时构造器执行)。
- **最小修复**: `load_from_disk` push 之前,`buf.push_back(LogEntry { message: redact_message(&entry.message).into_owned(), ..entry })`。

### D4-012 `set_app_locale` 内存 `rust_i18n::set_locale` 成功但后续 `cfg.save()` 失败时,`locale-changed` 事件已 emit,前端按新 locale 渲染但下次启动回到旧 locale
- **置信度**: 中
- **位置**: `src-tauri/src/commands/i18n.rs:34-44`
- **触发条件**: 用户切 locale,`cfg.save()` 因磁盘满失败,返回 Err 给前端,但 emit 已发出,前端按新 locale 渲染;用户重启,`lib.rs:123` 重新 `rust_i18n::set_locale(&config.locale)` 走旧 locale。
- **根因**: 命令顺序是 `set_locale → cfg.save() → emit locale-changed → emit config-changed`。save 失败时,前端拿不到 err(emit 已经发了),但内存已变,disk 还是旧 locale。
- **影响**: 用户看到的是"已切到 en",重启后变回 zh-CN,困惑。
- **最小修复**: 调换顺序为 `cfg.save() → rust_i18n::set_locale → emit`;或 `cfg.save()` 失败时 `rust_i18n::set_locale(&old)` 回滚 + 不 emit。

### D4-013 `add_extra_instance` custom 路径 spec.id 来自前端可被恶意覆写,后端无白名单校验
- **置信度**: 中
- **位置**: `src-tauri/src/commands/extra_instances.rs:184-191`
- **触发条件**: 前端 / 外部构造 `add_extra_instance({ provider_id: "custom", custom: { id: "minimax" } })` 提交,后端在 `:241` 检测 `spec.id.is_empty()` 才补,否则 `spec.id` 沿用前端给的字符串。这条 instance 的 `api_key_ref = spec.id = "minimax"`,`load_credential_for_id("minimax")` 直接覆盖 builtin key。
- **根因**: `update_extra_instance` / `add_extra_instance` 路径里只校验了 `provider_id == "custom"`,没校验 `custom.id` 必须是 `custom_<uuid>` 格式,也没校验不与 builtin id 冲突。`build_credentials` 也无命名空间隔离。
- **影响**: 凭据 namespace 冲突 —— 用户精心配置的 minimax key 被悄悄改写为 custom instance 的 key,poller 拉不到 minimax 数据,显示"未配置"。
- **最小修复**: `add_extra_instance` / `update_extra_instance` 入口加 `if custom.id != "" && !custom.id.starts_with("custom_")` 拒绝;并且 `custom.id` 一律由后端生成覆盖(与 builtin 副本同款)。

### D4-014 `update_extra_instance` 不接受 `provider_id` 字段,但 `extras[pos].provider_id` 与 `req.custom` 无联动校验,改 custom spec 等于新建一类
- **置信度**: 中
- **位置**: `src-tauri/src/commands/extra_instances.rs:320-345`
- **触发条件**: 用户改 `update_extraInstance({ id: <minimax extra uuid>, custom: { ... } })` —— 后端 `:331` 检测 `updated.provider_id != "custom"`,直接返 `commands.extra.custom_only_for_custom_provider`。但**没改 builtin** 时,`req.custom` 字段透传到 IPC 解析阶段,被 serde 静默忽略,用户看到 "成功" 但其实没存。
- **根因**: API 表面允许但后端不接受 `req.custom` 改 builtin,前端在改 builtin 时若误带 custom 字段不会报错,只是"成功"但无变化,极难 debug。
- **影响**: 静默吞错,用户视角"改了但保存按钮没反馈"。
- **最小修复**: 入口统一加 `if req.custom.is_some() && updated.provider_id != "custom" { return Err(...) }` 提前拒绝,前端不会误以为成功。

---

## 🟢 P3（v0.3 tech debt / 不影响 release）

### D4-015 `parse.rs::read_path` 对 Unicode 同形字符不归一化,custom source path 视觉欺骗
- **置信度**: 低
- **位置**: `src-tauri/src/providers/parse.rs:43-126`
- **触发条件**: 用户在 settings 面板写 `data.balance`,中转站运营商却在响应里**额外**塞同形字符路径 `data.Ьalance`(西里尔 Ь),实际 data 节点没这个 key,parse 返 None,fetch 报"未配置"。
- **根因**: 旧 B-M1 报告里 `buf.push(c)` 不做 NFKC 归一化。
- **影响**: 极低概率(要求攻击者控制中转站响应),但路径界面提示字相近,debug 极难。
- **最小修复**: 在 `read_path` 第一段对 `buf` 做 `unicode_normalization::char::decompose_canonical` 等价 NFKC。

### D4-016 `parse.rs::read_path` 的 `MAX_SEGMENTS=32` 在 `[idx]` 段不计数,递归深层数组 `data[0][0][0]...` 仍能 hit
- **置信度**: 中
- **位置**: `src-tauri/src/providers/parse.rs:100-120`
- **触发条件**: 中转站返回 1 万层嵌套数组 `[ [ [ [ ... ] ] ] ]`,即使对象层 ≤32,数组下标路径无上限。
- **根因**: `segments_traversed` 只在 `.` 段自增,`[idx]` 分支没自增。
- **影响**: DoS / 栈风险低于对象,但仍是 configurable resource cap 漏洞。
- **最小修复**: `[` 分支也 `segments_traversed += 1` + 校验。

### D4-017 `doImportConfig` 不限制文件大小,浏览器 `File.text()` 一次性读 1GB 撑爆 webview
- **置信度**: 高
- **位置**: `src/settings/advanced.ts:359-380`
- **触发条件**: 用户误选 1GB log 文件当"配置 JSON"导入,`file.text()` 把整个文件读进 webview heap,`JSON.parse` 触发 OOM。
- **根因**: 没有 `file.size > 1_000_000` 早 return。
- **影响**: 单次 DoS,需要重启 webview。
- **最小修复**: 入口 `if (file.size > 256 * 1024) { flash("文件过大", true); return; }`;同时 `JSON.parse` 之后立刻校验 `extra_instances` 数组长度 cap。

### D4-018 `doImportConfig` 校验 `obj.config` 是 object 但不校验内部字段,导入文件可注入 `"providers": { "💩": {...} }`
- **置信度**: 中
- **位置**: `src/settings/advanced.ts:363-380`
- **触发条件**: 攻击者制作恶意 JSON,`saveConfig(obj.config)` 后端 save_config 已加 map cap 256,不会 panic,但 `floating_x: "💩"` 等不合法值会落进 cfg(serde 不强校验这些字段类型)。
- **根因**: 校验不充分。
- **影响**: 配置文件被污染,前端启动渲染炸裂。
- **最小修复**: 前端先做基础校验(floating_x/y 是 number、provider_order 是 string[]、color_thresholds 是 3 元素递增数组)再 invoke;或者依赖后端 save_config 的新校验(目前缺)。

### D4-019 `run_dump_subcommand` 走 `tokio::runtime::Runtime::new()` 单线程,共享 `reqwest::Client` 与 GUI 实例并行调用时可能错位
- **置信度**: 低
- **位置**: `src-tauri/src/lib.rs:555-685`
- **触发条件**: 用户同时跑 GUI + 终端 `musage dump deepseek` → `dump` 进程独立拉取 deepseek,与 GUI 的轮询并发,`update_source_state_for_dump` 不会写 state(独立 runtime),但 `extra_instances::load_or_migrate` 在 dump 里读 + GUI 的 poller 写可能 race。
- **根因**: 没有进程间文件锁,`extra_instances.json` 被两个进程同时读写。
- **影响**: 概率极低(用户极少同时跑 GUI + CLI),但 dump 的输出可能与 GUI 当前显示不一致。
- **最小修复**: `dump` 路径用 `flock(LOCK_EX | LOCK_NB)` 锁 extra_instances.json;或加 advisory `fcntl` 锁文件。

### D4-020 `delete_source_credential` cascade `cfg.save()` 失败时 `tracing::warn!` 后不返 Err,前端看到 OK 但副本未被 disable
- **置信度**: 中
- **位置**: `src-tauri/src/commands/mod.rs:923-925`
- **触发条件**: builtin key 删除时 cascade 副本 disable,`cfg.save()` 失败(磁盘满),代码 `tracing::warn!` 但不 `return Err`,继续后面的 `delete_credential_for_id(r)` —— keys.json entry 删除成功,disk cfg 未保存,下次启动 poller 看到 enabled=true 副本继续 poll 报"未配置"。
- **根因**: H3 报告 + CM4 修复(2026-07-28)用 `if let Err(e) = cfg.save()` 吞错,本意"级联落盘失败不阻断主流程",但实际上等于"副本级联 disable 静默丢"。
- **影响**: 旧 H3 bug 修了一半 —— 删 key 成功,副本"未配置"死卡。
- **最小修复**: cascade 失败时 `return Err` 或写入 "cascade_partial_failed" 标记供下次启动 reconcile。

---

## 已确认旧报告已修不在本轮复发

为了避免重复旧报告,以下条目已逐项对照源码确认已修或已合规: `save_config` 256 map cap (`commands/mod.rs:590`)、`refresh_interval_secs` 上下限 (`commands/mod.rs:607-615`)、`truncate_old_backups` (`config.rs:1012`)、`LogEntry::*` redact (`logstore.rs:403-441`)、`set_source_credential` length / URL 校验、`delete_credential_for_id` 空 map fallback (`config.rs:1189-1207`)、`extra_instances::save` 0600 (`config/extra_instances.rs:207-212`)、`save_lock` poison recover 升级 ERROR (`config.rs:67-75`)、`load_from_disk` 启动 chmod 0600 (`logstore.rs:184-191`)、`stepfun/anysearch` per-unique_id refresh 锁 (`stepfun.rs:252` / `anysearch.rs:245`)、`Minimax` status==1 严格门 (`minimax.rs:545`)、`Xiaomi` 401 默认 AuthFailed (`xiaomi.rs:500`)、`ExtraInstance` 写锁内 compact (`commands/extra_instances.rs:408-492`)、`set_provider_order` 白名单 (`commands/mod.rs:92-105`)、`save_credential_for_id` 全字段写入避免覆盖 (`commands/extra_instances.rs:213-221`)、`schema_overrides` self-ref 拒绝 (`commands/mod.rs:286-316`)、`build_credentials` 字段三选一 (`commands/mod.rs:840-880`)、`LogEntry::clear` 60s grace + dedup (`commands/mod.rs:2099-2139`)、`backtrace` capture on poison (`config.rs:67-75`)、`base64 = "=0.22.1"` 钉版 (`Cargo.toml:42`)、`tray.rs` percent NaN/Infinity sanitize (`tray.rs` 2026-07-30 `0cc75b6`)、macOS `NSWindow` Retained 包 (`platform/macos.rs:248-270`)、Win `apply_z_order` 全局锁 (`platform/windows.rs:171-220`)、Quit 走 SHUTDOWN Notify (`lib.rs:1106-1108`)、`extend_geom_persister` debounce (`lib.rs:416-420`)。

---

## 修复优先级建议

- **立刻**(本次 release blocker):D4-001,D4-002
- **下周**(P1):D4-003,D4-004,D4-005,D4-006
- **v0.2.6 窗口**:D4-007,D4-008,D4-009,D4-010,D4-011,D4-012,D4-013,D4-014
- **v0.3 backlog**:D4-015 至 D4-020

总览:本轮未发现 CRITICAL,新发现 1 P0 / 4 P1 / 8 P2 / 6 P3,合计 19,集中在三方面:① 配置/凭据原子写边界(损坏/并发/回滚)仍是隐式分布式事务;② IPC 边界校验对 `save_config` 全量路径不完整,DoS 与凭据 namespace 注入面仍在;③ `logstore` 启动加载路径没复刻 redact,`get_recent_logs` 仍能回吐明文敏感字段。