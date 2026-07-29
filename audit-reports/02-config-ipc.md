# Musage 配置 / 凭证持久化 + IPC 边界代码审查报告

**审查范围**:`src-tauri/src/config.rs` (1233行), `src-tauri/src/config/extra_instances.rs` (619行), `src-tauri/src/commands/mod.rs` (2264行), `src-tauri/src/commands/extra_instances.rs` (680行), `src-tauri/src/commands/i18n.rs` (58行), `src-tauri/src/lib.rs` (752行)。

## 总体评价

上一轮 2026-06-20 死锁修复 + 后续多次全量审查已经把绝大多数 critical bug 摁住了。这一轮没发现会立刻炸进程的 critical,但仍有 4 个 HIGH 级别的隐患(主要是用户文件被误删 / 错误状态漏盘 / DoS 面)和十几个需要打磨的 MEDIUM/LOW 项。

## 🔴 CRITICAL
**未发现**。死锁 / 不可重入 / 启动 panic 这类硬伤 2026-06-20 audit 已全部修完。

## 🟠 HIGH

### H1. `cleanup_orphan_tmp_files` 会静默删除用户在 cfg 目录下的 `*.tmp` 文件
- **位置**:`src-tauri/src/config.rs:935-955`
- **类型**:文件所有权 / 数据丢失
- **描述**:启动时遍历 `~/.config/com.musage.app/` 删除所有 `.tmp` 后缀文件。用户完全可能自己放 `download.tmp` / `database.tmp` 之类的临时文件,会静默删除 — 没有日期过滤,没有「看是不是 musage 写的」(mtime / 前缀)。
- **影响**:一旦真实触发(用户碰巧在 cfg 目录放了个 `.tmp`),用户视角就是「软件把我文件偷了」。即使概率低,**沉默 + 无法恢复 + 责任在 app** 三件套。
- **建议**:改成只清理自己产生的 `*.json.tmp` / `*.jsonl.tmp`,或者限定为 `app_log.jsonl.tmp` / `config.json.tmp` / `keys.json.tmp` / `extra_instances.json.tmp` 这四个固定文件名。

### H2. `save_config` 接受前端传的 `cfg.providers` map 任意 key,几乎没校验
- **位置**:`src-tauri/src/commands/mod.rs:586-690`
- **类型**:IPC 边界 / 输入校验缺失
- **描述**:只校验 4 项(refresh_interval / color_thresholds / wallet_alert / color_overrides)。`cfg.providers` map key 可任意字符串,`floating_x/y/w/h` 可被设成 `i32::MIN` 或负的极大值,`provider_order` 无大小校验。
- **影响**:前端 bug / 攻击可塞 `{"../foo": {...}, "💩": {...}}` 进 `config.json`,日志刷屏 + 多余 IPC;浮窗坐标到屏幕外;`Vec<String>` 1MB DoS 面。
- **建议**:key 白名单过滤(11 内置 + 副本 + custom_<uuid>);`provider_order` max_len=128;`floating_x/y` range check `-32768..=32767`。

### H3. `delete_source_credential` 级联 disable 副本落盘失败,但 key 已删 → 永久不一致
- **位置**:`src-tauri/src/commands/mod.rs:862-933`
- **类型**:状态机一致性 / 错误处理
- **描述**:删 builtin `minimax` 的 key 时 cascade 要 disable 副本 `minimax#2`。如果 `cfg.save()` 因磁盘满失败,in-memory `cfg.providers["minimax#2"].enabled` 已经是 `false`,但 disk 上还是 `true`。**但 key 已经删了**。结果:副本 enabled=true (disk 旧值) + key 没了 (disk 新值)。下次启动读到 enabled=true 继续 poll,报「未配置」死循环。
- **建议**:要么 cfg.save 失败时 abort 整个 cascade(保留旧 key),要么改成「先 save cfg → 失败则不删 key」的顺序。

### H4. `set_source_credential` / `save_config` 不限制 value 长度,IPC 边界无 size cap
- **位置**:`src-tauri/src/commands/mod.rs:726-741`, `:586`
- **类型**:DoS / 内存放大
- **描述**:`value: "x".repeat(2_000_000_000)` 反序列化阶段已 alloc 2GB。`trim()` + `save_credential_for_id` 内部 `read_keys()` + `serde_json::to_string_pretty` 再 alloc 多份。后果是 IPC handler 在 tokio runtime thread 上死锁,配置文件被覆写成垃圾。
- **建议**:handler 入口 `value.chars().take(8 * 1024).collect::<String>()`;`provider_order` / `providers` map 加 max entries 截断。

## 🟡 MEDIUM (15 项要点)

### M1. `set_provider_enabled` 持 `config.write` 期间嵌套 `extra_instances.read` —— 破坏锁顺序契约
- **位置**:`src-tauri/src/commands/mod.rs:122-128`
- **修法**:helper `acquire_config_and_extras_for_save()` 强制单一入口。

### M2. `std::sync::Mutex` 的 `save_lock()` 在 tokio runtime 内阻塞当前 worker 线程
- **位置**:`src-tauri/src/config.rs:36-52`
- **影响**:Win HDD `sync_all` 50-200ms 阻塞 worker 线程,UI 抖动。
- **修法**:改 `tokio::sync::Mutex`,写盘包 `tokio::task::spawn_blocking`。

### M3. 错误消息在多个地方泄露绝对文件路径(含 home dir)
- **位置**:`src-tauri/src/config.rs:1086-1090`, `:498-512`
- **修法**:日志保留全 path 方便运维,IPC 返前端的 Err 只放 basename。

### M4. `set_app_locale` 的错误消息绕过 i18n(其他 IPC 都用 `t!()`)
- **位置**:`src-tauri/src/commands/i18n.rs:31-33`
- **修法**:加 `commands.locale_invalid` 到 locales/*.json。

### M5. `delete_extra_instance` 多处 `.ok()` 吞掉删除失败
- **位置**:`src-tauri/src/commands/extra_instances.rs:471-486`
- **影响**:keys.json 残留孤儿 entry,下次启动 read_keys 读进内存但 poller 不 poll。
- **修法**:`.map_err(|e| tracing::error!(...))?`。

### M6. `set_zenmux_base_url` 只校验 scheme 前缀,不解析 URL 也不限长度
- **位置**:`src-tauri/src/commands/mod.rs:437-459`
- **修法**:`reqwest::Url::parse(trimmed)` 二次校验,max length 2048。

### M7. `read_keys` 空文件 → backup 失败 silent → 下次 save 覆盖原始 keys.json
- **位置**:`src-tauri/src/config.rs:1060-1090`
- **影响**:磁盘坏块 → 0 字节 keys.json,backup silent 失败,下次 save 覆盖。
- **修法**:backup 失败 → log error + 写 `.bak.failed` 标记 + AppState flag 拒绝后续 save。

### M8. `best_effort_from_value` 的 field 抓取与 `AppConfig` 字段漂移
- **位置**:`src-tauri/src/config.rs:783-922`
- **影响**:典型「两份源码对一份 schema」脆弱设计。
- **修法**:CI 加 grep 扫「新增字段但 best_effort 漏抓」。

### M9. `set_provider_order` 入口 `Vec<String>` 无 size cap
- **位置**:`src-tauri/src/commands/mod.rs:41-50`, `:86-105`
- **修法**:handler 入口 `order.truncate(MAX_LEN * 4)`。

### M10. `save_credential_for_id` 三个字段都 None 时静默删除已有 credential
- **位置**:`src-tauri/src/config.rs:1135-1167`
- **影响**:函数名 `save_`,但传全 None 是 `delete_` 语义。`build_credentials` 总是保证 wrote_any=true,但 API 公开 + 长期累积 bug 风险。
- **修法**:拆 `save_` (必须 wrote_some) 和 `upsert_`(允许 None)。

### M11. `is_valid_hex_color` 接受 8 位 `#RRGGBBAA` 但拒 4 位 `#RGBA`
- **位置**:`src-tauri/src/commands/mod.rs:2027-2034`
- **影响**:风格不一致但行为正确。
- **建议**:加 4 位支持。

### M12. `delete_credential_for_id` redundant `path.exists()` 检查
- **位置**:`src-tauri/src/config.rs:1176-1199`

### M13. `load_or_migrate` save 成功但 rename 老文件失败 → 老文件永久残留
- **位置**:`src-tauri/src/config/extra_instances.rs:280-290`
- **修法**:rename 失败重试 N 次,或加 `.pending_rename` marker。

### M14. `save_lock` 中毒恢复 log level 是 WARN 而非 ERROR
- **位置**:`src-tauri/src/config.rs:58-65`
- **修法**:改 `tracing::error!` + 附 panic 上下文。

### M15. `set_provider_enabled` 的「乐观 emit placeholder」路径可能触发 enable→fetch→disabled race
- **位置**:`src-tauri/src/commands/mod.rs:155-221`
- **修法**:用 `unique_id` 作 retain key,避免 source_id collision。

## 🟢 LOW (10 项)

- L1. `migrated()` 的 `0 | 1` 分支是死代码 — 加 `tracing::info!` 留脚印
- L2. `refresh_interval_secs` 只校验下限,无上限 — 加 `> 86_400` 拒
- L3. `write_keys_atomic` 和 `write_config_atomic` 错误消息命名不一致 — 合并 key
- L4. `keys.json.bak.<ts>` / `config.json.bak.<ts>` 永久累积 — 只保留最近 N=5 份
- L5. `save_credential_for_id` 的 `secret_key` 槽无独立命名空间 — 改 `BTreeMap<String, Credentials>`
- L6. `floating_w/h` 用 `i32` 实际存物理像素 — 加 doc comment 或改 `f64`
- L7. `process::exit(1)` 在 tauri builder 失败时没 cleanup — 至少 log error
- L8. `is_serialized()` 方法在 `FloatingPinMode` 是 no-op dead code
- L9. `set_provider_enabled` 的 M4 fix 依赖隐式 drop — 用内嵌作用域
- L10. `save_config` 改 autostart 时 log warn 但不返 Err — 累积到 `Err(String)` 给前端显示

## 总结

| 等级 | 数量 | 主要类别 |
|---|---|---|
| CRITICAL | 0 | — |
| HIGH | 4 | 文件所有权误删 (H1) / IPC 校验缺失 (H2, H4) / 状态机不一致 (H3) |
| MEDIUM | 15 | 锁顺序 / 阻塞 runtime / 路径泄露 / i18n / 错误吞掉 / DoS 面 / API 陷阱 |
| LOW | 10 | 维护负担 + 小一致性问题 |

**重点关注 H1**:清理逻辑主动删用户文件。
**重点关注 H3**:cascade 落盘失败 + key 已删 = 「修了一半比不修更糟」。
