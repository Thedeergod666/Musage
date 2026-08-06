//! 应用运行日志 —— 错误/警告/信息事件流
//!
//! ## 用途
//!
//! 1. **网络抖动归因**：用户看浮窗上的小红点时，进设置面板 → 日志模块能看到
//!    每次失败的具体 `error_kind` + 原始 error 串。
//! 2. **历史回放**：错误恢复后浮窗恢复绿点，但日志里还有这一条，便于事后排查。
//! 3. **避免污染浮窗**：报错信息不再 over 卡片 UI，浮窗只留红点 → 用户能继续看用量。
//!
//! ## 存储
//!
//! - 内存 ring buffer（最近 `MAX_ENTRIES` 条）
//! - 持久化到 `<config_dir>/com.musage.app/app_log.jsonl`（JSON Lines，一行一条）
//! - 启动时把文件里最近 `MAX_ENTRIES` 条 load 进来
//! - 写新条目时 append 文件 + push 到 ring；超 cap 时弹出最旧的（不删除文件旧行，
//!   下次启动会被 cap 重新截断）
//!
//! ## 线程模型（M1 fix 2026-07-02）
//!
//! 把 `inner` 从 `Mutex<VecDeque>` 改成 `Arc<Mutex<VecDeque>>`,允许 background
//! worker 线程拿一份共享引用(避免 Mutex 跨线程 Send 问题)。所有磁盘 I/O
//! (append + truncate + clear 时删文件) 通过持久 worker 线程串行处理,
//! 不阻塞调用方的 ring update。这样极慢盘(NAS / 网盘)也不会让 recent()
//! 和 clear() 数百 ms 卡顿。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use serde::{Deserialize, Serialize};

// C8 fix (2026-07-28 审查): mutex poison 恢复统一走 config 模块的
// lock_recover helper,不再各处手写 `unwrap_or_else(|e| { warn; into_inner })`。
use crate::config::lock_recover;

/// Ring buffer 上限。够看一周左右的故障，避免文件无限增长。
pub const MAX_ENTRIES: usize = 200;

/// 给 commands 用：让 tauri command 能用同一个常量限制 limit 参数
/// 防止前端乱传 100000 把内存吃光。
pub fn max_entries() -> usize {
    MAX_ENTRIES
}

/// 日志级别。前端按 level 选徽章色。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// 一条日志。前端需要的字段都直接展开（不包枚举的 Option<...>），
/// 这样 TS 侧 `entry.kind` 是 `string | null` 直接可用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// 毫秒时间戳
    pub ts: i64,
    pub level: LogLevel,
    /// provider id（"minimax" / "deepseek" / "xiaomimimo"），全局事件为 null
    pub provider: Option<String>,
    /// 错误分类字符串（前端跟 ErrorKind 的 short_label 对齐用），非错误事件为 null
    pub kind: Option<String>,
    /// 人类可读的描述
    pub message: String,
}

/// 已知敏感模式 → `<redacted>`。
///
/// 覆盖：
/// - `Bearer xxx` / `Basic xxx` HTTP 头
/// - `sk-*` / `sk-or-v1-*` / `sk-cp-*` provider key 前缀
/// - `tvly-*` / `tp-*` / `tk-*` 各家自定义前缀
/// - `eyJ...` JWT 三段式
/// - `Oasis-Token=` / `Oasis-Refresh-Token=` / `MUSAGE_TOKEN=` cookie 名
/// - `Cookie:` / `Set-Cookie:` 整行（即便值不匹配上面也遮蔽，最保守）
///
/// **正则 caveat**：贪婪匹配到下一个分隔符（空格 / 引号 / 逗号 / 行尾），
/// 不会跨越这些边界。多个 token 在同一行也会全部被替换。
///
/// 用于 [`LogEntry`] 三种构造器 + 写盘前的 `append_entry`，**两层防御**
/// 防 caller 漏掉调用 redact。
pub fn redact_message(s: &str) -> std::borrow::Cow<'_, str> {
    use regex::Regex;
    use std::borrow::Cow;
    use std::sync::OnceLock;

    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // 用 raw string 拼接 (concat!)，避免 Rust normal string 把 "\t" / "\s"
        // 转义成字面 "\ + t/s"——regex 引擎收到错的字符类。raw string 里每一个
        // "\t" "\s" "\b" 都是真的 regex 转义；字符类 "[ \t]" 才是真的「空格 或 tab」。
        // 之前的 `(?ix)` 配 normal string 的写法因为双层转义全部错位：
        //   - Bearer[ \t]+ 实际收到 "[\t]+"（单 \ + tab），regex 字符类变 "[\t]"，
        //     只匹配 tab 不匹配 space → "Bearer xxx" 全行不命中
        //   - \b 在 (?ix) 下仍是 word boundary，但 ?ix 把正常空格也吃掉 → 多余噪音
        // 用 raw string 后这些全部回归正常。
        Regex::new(concat!(
            // 不开 (?i) —— 全局 case-insensitive 会让 "cookie:" (lowercase)
            // 也匹配 `(?:Cookie|Set-Cookie):`,在 "; cookie: tk-xxx" 这种
            // 业务错误串里误吞整行 → 后续 sk-/eyJ/等 pattern 全被吃掉。
            // 改为局部 [Bb]earer/[Bb]asic (HTTP 头按 RFC 大小写不敏感,实际
            // 日志里两种都见过),Cookie/Set-Cookie 保持字面 (HTTP 规范就是
            // 这两个拼写,无歧义)。prefix (sk-/tvly-/tp-/tk-/eyJ) 全 case-
            // sensitive (厂商 token 格式都是 lowercase)。
            r"(?:",
            r"[Bb]earer\s+[A-Za-z0-9._\-+/=]{8,}",
            r"|[Bb]asic\s+[A-Za-z0-9._\-+/=]{4,}",
            r"|\bsk-[A-Za-z0-9_\-]{8,}",
            r"|\bsk-or-v1-[A-Za-z0-9_\-]{8,}",
            r"|\bsk-cp-[A-Za-z0-9_\-]{8,}",
            r"|\btvly-[A-Za-z0-9_\-]{8,}",
            r"|\btp-[A-Za-z0-9_\-]{8,}",
            r"|\btk-[A-Za-z0-9_\-]{8,}",
            r"|\beyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{2,}\.[A-Za-z0-9_\-]{2,}",
            r"|Oasis-Token=[^\s;,]+",
            r"|Oasis-Refresh-Token=[^\s;,]+",
            r"|MUSAGE_TOKEN=[^\s;,]+",
            // 2026-08-06 cross-verify (#5/#7): kimi v0.2.6 集成新增 kimi-auth cookie,
            // URL query ?access_token= 也是 token 载体,redact 之前都漏。kimi-auth
            // 的值是 JWT(已被上面 eyJ pattern 覆盖),这里补名字前缀防裸串边界;
            // access_token= 防回显进日志的 URL query。
            r"|kimi-auth=[^\s;,]+",
            r"|access_token=[^\s;&]+",
            r"|(?:Cookie|Set-Cookie):[^\n]+",
            r")",
        ))
        .expect("redact regex compile failed")
    });
    if !re.is_match(s) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(re.replace_all(s, "<redacted>").into_owned())
}

/// 进程内全局单例。`Arc<Mutex<VecDeque>>` 是 M1 fix 的核心 —— 让
/// background worker 能 clone 一份共享引用做 disk I/O,主线程 push 的
/// 锁段只覆盖 ring buffer 的内存更新,不阻塞任何 I/O。
#[derive(Clone)]
pub struct LogStore {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl LogStore {
    /// 新建空 store（不读盘）。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_ENTRIES))),
        }
    }

    /// 从磁盘 reload 最近 MAX_ENTRIES 条。文件不存在 / 解析失败 → 当成空。
    pub fn load_from_disk() -> Self {
        let mut buf: VecDeque<LogEntry> = VecDeque::with_capacity(MAX_ENTRIES);
        if let Ok(path) = log_path() {
            if let Ok(file) = File::open(&path) {
                let reader = BufReader::new(file);
                for line in reader.lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(entry) = serde_json::from_str::<LogEntry>(&line) {
                        buf.push_back(entry);
                    }
                }
                // 只保留最后 MAX_ENTRIES 条
                if buf.len() > MAX_ENTRIES {
                    let drop = buf.len() - MAX_ENTRIES;
                    buf.drain(..drop);
                }
                // B-H3 fix (2026-07-30 audit): 升级场景下老 app_log.jsonl 可能
                // 保留 0644 权限,append_entry 的 0600 设置只在下次写时才生效。
                // 启动期先 force 一遍,关闭「历史文件权限泄漏同机其他用户可读
                // history 错误日志」的窗口 —— message 字段可能含 API key /
                // cookie(redact 后还是会泄漏结构)。
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        Self {
            inner: Arc::new(Mutex::new(buf)),
        }
    }

    /// Append 一条。内部处理 ring buffer cap + 文件追加。
    ///
    /// H6 fix: 之前只裁内存 VecDeque，磁盘文件永远 append 不截断。
    /// 注释说"下次启动会被 cap 重新截断"——与代码不符(load_from_disk
    /// 只裁内存，不管文件)。1 年用户可能堆出几十 MB log 文件。
    /// 现在 push 达到 cap 时用 ring buffer 内容重写文件（写 tmp + rename，
    /// 原子替换，避免 half-written 坏文件）。
    ///
    /// **M1 fix（2026-07-02 audit）**：之前整个 file I/O (OpenOptions::open +
    /// writeln + 可选 truncate_file 全文件重写) 都在锁内 ——
    /// 极慢盘(NAS / 网盘 / 机械盘满载)上数百 ms 阻塞,期间 recent() 和
    /// clear() 全部卡住。改为:锁内只更新 ring (push_back + 可选 pop_front),
    /// 之后把 entry + needs_truncate flag 一并派到 background worker,
    /// worker clone 一份 store 引用做磁盘操作。
    ///
    /// **L2 fix（2026-06-19）**：append / clear 走同一 channel —— 避免
    /// clear 删文件 + append 重建文件的"死而复生"竞态。
    pub fn push(&self, entry: LogEntry) {
        let mut g = self.inner.lock().unwrap_or_else(lock_recover);
        g.push_back(entry.clone());
        let needs_truncate = g.len() > MAX_ENTRIES;
        if needs_truncate {
            g.pop_front();
        }
        drop(g);
        spawn_append_job(self.clone(), AppendJob::Append(entry, needs_truncate));
    }

    /// 快照：返回最近 n 条（按时间正序）。n == None → 全部。
    pub fn recent(&self, n: Option<usize>) -> Vec<LogEntry> {
        let g = self.inner.lock().unwrap_or_else(lock_recover);
        match n {
            None => g.iter().cloned().collect(),
            Some(k) => g
                .iter()
                .rev()
                .take(k)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
        }
    }

    /// 清空内存 + 删文件。
    ///
    /// **L2 fix（2026-06-19）**：跟 push 共用同一 channel —— 避免 push 写文件
    /// 后被抢断 + clear 删文件造成的文件-内存不一致窗口。
    pub fn clear(&self) {
        {
            let mut g = self.inner.lock().unwrap_or_else(lock_recover);
            g.clear();
        }
        spawn_append_job(self.clone(), AppendJob::ClearMarker);
    }
}

// ── Background worker（M1 fix 取代锁内 I/O）────────────────────────
//
// 一条持久 std::thread 串行处理 push/clear 的磁盘工作。任意磁盘故障
// (worst case 几百 ms) 只影响这条后台线程,不阻塞 hot path 的 ring
// buffer 更新 —— 调用方的 recent() 和 clear() 不再因为磁盘慢而 hang。

#[derive(Debug)]
enum AppendJob {
    Append(LogEntry, bool), // entry, needs_truncate
    ClearMarker,
}

static APPEND_JOB_TX: OnceLock<std::sync::mpsc::Sender<(LogStore, AppendJob)>> = OnceLock::new();

fn spawn_append_job(store: LogStore, job: AppendJob) {
    let tx = APPEND_JOB_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(LogStore, AppendJob)>();
        thread::Builder::new()
            .name("musage-logstore-append".into())
            .spawn(move || {
                tracing::debug!("logstore 后台 append 线程启动");
                while let Ok((store, job)) = rx.recv() {
                    match job {
                        AppendJob::Append(entry, needs_truncate) => {
                            if let Err(e) = append_entry(&entry) {
                                tracing::warn!(error = %e, "logstore 后台 append 失败");
                            }
                            if needs_truncate {
                                // truncate:从 store clone 整份 ring,tmp + rename 重写
                                let ring = {
                                    let g = store.inner.lock().unwrap_or_else(lock_recover);
                                    g.iter().cloned().collect::<Vec<_>>()
                                };
                                if let Err(e) = truncate_file_from_ring(&ring) {
                                    tracing::warn!(error = %e, "logstore 后台 truncate 失败");
                                }
                            }
                        }
                        AppendJob::ClearMarker => {
                            if let Ok(path) = log_path() {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                }
                tracing::debug!("logstore 后台 append 线程退出");
            })
            .expect("启动 logstore 后台 append 线程");
        tx
    });
    // M3 fix (2026-07-06 全量审查): send 返 Err 通常意味着后台 worker
    // 已死(panic / OOM / ring clone 失败)。静默 `let _ = tx.send(...)`
    // 只能让运维盲飞。升级 error 级 log —— 磁盘落盘停止的事实要被看见。
    if let Err(e) = tx.send((store, job)) {
        tracing::error!(
            error = ?e,
            "logstore background append worker 已死 —— 后续 push 在内存 ring 里更新,但不再落盘"
        );
    }
}

/// 后台线程实际写的 append 实现。
fn append_entry(entry: &LogEntry) -> std::io::Result<()> {
    let path = log_path().map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    // C4 fix (2026-07-28 审查): app_log.jsonl 的 message 字段可能带 API
    // key / cookie 的错误串,跟 keys.json 同级别敏感。OpenOptions 默认 0644
    // 会暴露给同机其他用户 —— 跟 write_keys_atomic / extra_instances::save
    // 对齐,显式 0600(每次 append 都设一遍,顺带覆盖历史遗留的 0644 文件)。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    // 双层 redact 防御:构造器已经过 redact,但 caller 也可能直接
    // 构造 LogEntry{ message: raw_string } 绕过 → 写盘前再过一遍。
    let safe_entry = LogEntry {
        ts: entry.ts,
        level: entry.level,
        provider: entry.provider.clone(),
        kind: entry.kind.clone(),
        message: redact_message(&entry.message).into_owned(),
    };
    let json = serde_json::to_string(&safe_entry).map_err(std::io::Error::other)?;
    writeln!(f, "{}", json)?;
    // M2 fix (2026-07-06 全量审查): flush + sync_all,确保崩溃后最关键
    // 错误日志不丢。否则 forensic 关键时刻(应用 crash 前最后一条 error)
    // 会因为 page cache 没刷盘而缺失 —— 留下"为什么崩溃"的无解之谜。
    let _ = f.flush();
    let _ = f.sync_all();
    Ok(())
}

/// 把 ring buffer 内容重写到磁盘（覆盖整个 .jsonl 文件）。
/// 后台 truncate 用,频率 ~1/200 pushes,可接受作"次优同步"。
fn truncate_file_from_ring(ring: &[LogEntry]) -> Result<(), String> {
    let path = log_path()?;
    let tmp = path.with_extension("jsonl.tmp");
    let mut f = std::fs::File::create(&tmp).map_err(|e| format!("logstore truncate tmp: {e}"))?;
    let write_result = (|| -> Result<(), String> {
        for entry in ring {
            if let Ok(s) = serde_json::to_string(entry) {
                // C5 fix (2026-07-28 审查): 之前 `let _ = writeln!(...)` 吞错误
                // —— 磁盘满时静默写出截断文件,再 rename 覆盖掉完好的 log。
                writeln!(f, "{}", s).map_err(|e| format!("logstore truncate write: {e}"))?;
            }
        }
        // C5 fix: rename 前 flush + sync_all,确保 tmp 数据落盘后再原子替换,
        // 否则掉电可能留下 0 字节 / 半写文件覆盖好文件。
        f.flush()
            .map_err(|e| format!("logstore truncate flush: {e}"))?;
        f.sync_all()
            .map_err(|e| format!("logstore truncate sync: {e}"))?;
        Ok(())
    })();
    if let Err(e) = write_result {
        // 写 / 刷盘失败:清掉残缺 tmp,不 rename
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // C4 fix (2026-07-28 审查): 跟 append_entry 同款 0600(默认 0644)。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    // C5 fix: rename 失败清理 tmp,避免孤儿残留(跟 write_keys_atomic 同款)。
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("logstore truncate rename: {e}"));
    }
    Ok(())
}

fn log_path() -> Result<PathBuf, String> {
    let dir = dirs::config_dir().ok_or_else(|| "无法定位配置目录".to_string())?;
    Ok(dir.join("com.musage.app").join("app_log.jsonl"))
}

// ── 便捷构造器 ──────────────────────────────────────────────

impl LogEntry {
    /// 错误事件 —— `level=Error`，`provider` + `kind` 必填。
    pub fn error(provider: &str, kind: &str, message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            level: LogLevel::Error,
            provider: Some(provider.to_string()),
            kind: Some(kind.to_string()),
            // H3 fix (2026-07-29 审查): 跟 warn/info 一样走 redact_message,
            // 否则 caller 直接构造 LogEntry::error 时带 Bearer/sk-/etc. 的
            // 错误串会落盘到 app_log.jsonl。即使 append_entry 写盘前还有一层
            // 兜底 (safe_entry),构造器层先 redact 能省一次分配,也防 caller
            // 误把同一个 LogEntry 通过 IPC 直接吐给前端展示 (那层没双层兜底)。
            message: redact_message(&msg).into_owned(),
        }
    }

    /// 警告事件。其它字段按需填。`message` 自动过 [`redact_message`]。
    pub fn warn(provider: Option<&str>, message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            level: LogLevel::Warn,
            provider: provider.map(|s| s.to_string()),
            kind: None,
            message: redact_message(&msg).into_owned(),
        }
    }

    /// 信息事件。`message` 自动过 [`redact_message`]。
    pub fn info(provider: Option<&str>, message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            level: LogLevel::Info,
            provider: provider.map(|s| s.to_string()),
            kind: None,
            message: redact_message(&msg).into_owned(),
        }
    }
}

#[cfg(test)]
mod redact_tests {
    use super::*;

    #[test]
    fn bearer_token_redacted() {
        let s = "HTTP 401: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig returned";
        let r = redact_message(s);
        assert!(r.contains("<redacted>"), "r = {r}");
        assert!(!r.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn sk_prefix_redacted() {
        let r = redact_message("api_key=sk-or-v1-abcdefghij1234567890 not valid");
        assert!(r.contains("<redacted>"));
        assert!(!r.contains("sk-or-v1-abcdefghij"));
    }

    #[test]
    fn tvly_prefix_redacted() {
        let r = redact_message("Tavily: tvly-XYZ123abc456 not configured");
        assert!(r.contains("<redacted>"));
        assert!(!r.contains("tvly-XYZ"));
    }

    #[test]
    fn tp_prefix_redacted() {
        let r = redact_message("cookie: tp-abcdefgh1234 stored");
        assert!(r.contains("<redacted>"));
        assert!(!r.contains("tp-abcdefgh"));
    }

    #[test]
    fn jwt_redacted() {
        let r = redact_message("got token eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc.signature_xyz");
        assert!(r.contains("<redacted>"));
        assert!(!r.contains("eyJzdWIiOi"));
    }

    #[test]
    fn oasis_cookie_redacted() {
        let r = redact_message("saved Oasis-Token=eyJabc.signature_x; expires 2030");
        assert!(r.contains("<redacted>"));
        assert!(!r.contains("eyJabc"));
    }

    #[test]
    fn cookie_header_redacted() {
        let r = redact_message("Cookie: api-platform_serviceToken=secret123; path=/");
        assert!(r.contains("<redacted>"));
        assert!(!r.contains("secret123"));
    }

    #[test]
    fn multiple_sensitive_redacted() {
        let s = "failed auth: Bearer xyz12345678aaa; cookie: tk-aaa111bbb; jwt: eyJabc12345.sig12345.q9";
        let r = redact_message(s);
        assert_eq!(r.matches("<redacted>").count(), 3, "r = {r}");
    }

    #[test]
    fn plain_text_unchanged() {
        let s = "provider minimax returned 503 server error";
        assert_eq!(redact_message(s).as_ref(), s);
    }

    #[test]
    fn log_entry_constructors_apply_redact() {
        let e = LogEntry::error(
            "minimax",
            "auth_failed",
            "Authorization: Bearer eyJhbGciOi.payload.sig invalid",
        );
        assert!(!e.message.contains("eyJhbGciOi"), "msg = {}", e.message);
        assert!(e.message.contains("<redacted>"));

        let w = LogEntry::warn(Some("stepfun"), "token tp-abcdef1234 expired");
        assert!(!w.message.contains("tp-abcdef"));
    }

    #[test]
    fn redact_is_idempotent() {
        let s = "Bearer abc12345678xxx";
        let r1 = redact_message(s);
        let r2 = redact_message(&r1);
        assert_eq!(r1.as_ref(), r2.as_ref(), "redact 必须幂等");
    }
}
