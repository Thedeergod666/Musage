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
//! ## 线程模型
//!
//! 用 `parking_lot::Mutex<VecDeque<...>>` 同步锁（不在 hot path，错误事件频率低）。
//! 避免与 `tokio::sync::RwLock` 混用产生潜在的死锁。

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

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

/// 进程内全局单例。包成 Mutex<VecDeque<...>> + 文件句柄
/// （File 不在 Mutex 里 —— 每次写单独 open 即可，简化并发模型）。
pub struct LogStore {
    inner: Mutex<VecDeque<LogEntry>>,
}

impl LogStore {
    /// 新建空 store（不读盘）。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)),
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
            inner: Mutex::new(buf),
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
    /// **L2 fix（2026-06-19）**：之前文件 IO 在锁外、ring 操作在锁内，
    /// 与 `clear()`（锁内清 ring + 锁外删文件）形成一个文件-内存不一致窗口：
    /// push 写完文件后被抢断 → clear 清 ring 并删文件 → push 继续把 entry
    /// 推进 ring → 文件没了但内存还有一条 → 下次 push 重建文件，孤儿 entry
    /// 永远不会被序列化。修法：把整个 push（包括文件 IO + ring 更新 + 可选
    /// truncate）放进同一个锁段。clear 也同样。
    pub fn push(&self, entry: LogEntry) {
        let mut g = self.inner.lock().unwrap_or_else(|e| {
            tracing::warn!("logstore mutex poisoned，自动恢复");
            e.into_inner()
        });
        // 1. 写文件（best-effort，IO 失败不阻塞业务流程）
        if let Ok(path) = log_path() {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                if let Ok(s) = serde_json::to_string(&entry) {
                    let _ = writeln!(f, "{}", s);
                }
            }
        }

        // 2. 更新 ring
        g.push_back(entry);
        if g.len() > MAX_ENTRIES {
            g.pop_front();
            // H6 fix: 磁盘文件也要同步截断，否则 .jsonl 无限增长。
            // 用 ring buffer 当前内容重写整个文件（写 tmp + rename 原子替换）。
            // 锁内调用 truncate_file：失败只 log，不影响内存状态。
            if let Err(e) = self.truncate_file(&g) {
                tracing::warn!(error = %e, "logstore truncate_file 失败");
            }
        }
    }

    /// 把 ring buffer 内容重写到磁盘（覆盖整个 .jsonl 文件）。
    /// push 里超过 MAX_ENTRIES 时调用，用 tmp + rename 保证原子。
    fn truncate_file(&self, ring: &VecDeque<LogEntry>) -> Result<(), String> {
        let path = log_path()?;
        let tmp = path.with_extension("jsonl.tmp");
        let mut f = std::fs::File::create(&tmp)
            .map_err(|e| format!("logstore truncate tmp: {e}"))?;
        for entry in ring {
            if let Ok(s) = serde_json::to_string(entry) {
                let _ = writeln!(f, "{}", s);
            }
        }
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("logstore truncate rename: {e}"))
    }

    /// 快照：返回最近 n 条（按时间正序）。n == None → 全部。
    pub fn recent(&self, n: Option<usize>) -> Vec<LogEntry> {
        let g = self.inner.lock().unwrap_or_else(|e| {
            tracing::warn!("logstore mutex poisoned，自动恢复");
            e.into_inner()
        });
        match n {
            None => g.iter().cloned().collect(),
            Some(k) => g.iter().rev().take(k).cloned().collect::<Vec<_>>().into_iter().rev().collect(),
        }
    }

    /// 清空内存 + 删文件。
    ///
    /// **L2 fix（2026-06-19）**：和 push 一起放进同一个锁段，避免 push 写文件
    /// 后被抢断 + clear 删文件造成的文件-内存不一致窗口。
    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| {
            tracing::warn!("logstore mutex poisoned，自动恢复");
            e.into_inner()
        });
        g.clear();
        if let Ok(path) = log_path() {
            let _ = std::fs::remove_file(&path);
        }
    }
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
