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
        let mut g = self.inner.lock().unwrap_or_else(|e| {
            tracing::warn!("logstore mutex poisoned，自动恢复");
            e.into_inner()
        });
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
        let g = self.inner.lock().unwrap_or_else(|e| {
            tracing::warn!("logstore mutex poisoned，自动恢复");
            e.into_inner()
        });
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
            let mut g = self.inner.lock().unwrap_or_else(|e| {
                tracing::warn!("logstore mutex poisoned，自动恢复");
                e.into_inner()
            });
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
                                    let g = store.inner.lock().unwrap_or_else(|e| {
                                        tracing::warn!(
                                            "logstore mutex poisoned (truncate)，自动恢复"
                                        );
                                        e.into_inner()
                                    });
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
    let path = log_path().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    let s = serde_json::to_string(entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    writeln!(f, "{}", s)?;
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
    for entry in ring {
        if let Ok(s) = serde_json::to_string(entry) {
            let _ = writeln!(f, "{}", s);
        }
    }
    std::fs::rename(&tmp, &path).map_err(|e| format!("logstore truncate rename: {e}"))
}

fn log_path() -> Result<PathBuf, String> {
    let dir = dirs::config_dir().ok_or_else(|| "无法定位配置目录".to_string())?;
    Ok(dir.join("com.musage.app").join("app_log.jsonl"))
}

// ── 便捷构造器 ──────────────────────────────────────────────

impl LogEntry {
    /// 错误事件 —— `level=Error`，`provider` + `kind` 必填。
    pub fn error(provider: &str, kind: &str, message: impl Into<String>) -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            level: LogLevel::Error,
            provider: Some(provider.to_string()),
            kind: Some(kind.to_string()),
            message: message.into(),
        }
    }

    /// 警告事件。其它字段按需填。
    pub fn warn(provider: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            level: LogLevel::Warn,
            provider: provider.map(|s| s.to_string()),
            kind: None,
            message: message.into(),
        }
    }

    /// 信息事件。
    pub fn info(provider: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            ts: chrono::Utc::now().timestamp_millis(),
            level: LogLevel::Info,
            provider: provider.map(|s| s.to_string()),
            kind: None,
            message: message.into(),
        }
    }
}
