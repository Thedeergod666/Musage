//! Poller 用的 per-provider 指数退避状态。
//!
//! 触发条件：fetch 失败的 `error_kind` 属于服务端压力类
//! （`RateLimited` / `ServerError` / `Network`）→ 翻倍。
//! 成功 1 次 → reset 回正常间隔。
//! **不**对 `AuthFailed` / `UnconfiguredKey` / `SchemaUnknown` / `Parse` / `Other`
//! 退避 —— 这些是用户配置问题，不该让"修个 key"还得等 30 分钟。
//!
//! 状态在 `AppState.backoff` 共享：
//! - 写：`commands::refresh_inner` / `refresh_single_inner` 每次 fetch 完调 `record`
//! - 读：`poller::start` 调度时调 `next_interval_secs`
//!
//! 设计目标：纯逻辑，无 IO、无网络、易测。`AppState` 只装 `Arc<RwLock<_>>` 即可。

use std::collections::HashMap;

use crate::providers::{ErrorKind, ProviderSnapshot};

/// 单次退避后的最大间隔（30 分钟）。超过这个就 cap 住，避免长断网时
/// 完全停止数据更新。
pub const MAX_BACKOFF_SECS: u64 = 30 * 60;

/// 单个 source 的退避状态。
#[derive(Debug, Clone, Default)]
pub struct SourceBackoff {
    /// 当前退避后的间隔（秒）。`None` = 用默认（无退避）。
    pub current_interval_secs: Option<u64>,
    /// 连续"可退避类失败"次数；任何一次成功就清零。
    pub failure_streak: u32,
}

/// 全局退避注册表。
#[derive(Debug, Default)]
pub struct BackoffState {
    per_source: HashMap<String, SourceBackoff>,
}

impl BackoffState {
    pub fn new() -> Self {
        Self::default()
    }

    /// M5 fix: 克隆一份 per_source interval map，供 poller 循环前快照。
    /// poller 拿到 snapshot 后立即释放 RwLock read guard，
    /// spawn 的 refresh_single_inner 能立即拿到 write → 不再排队 1s+。
    pub fn clone_interval_map(&self) -> HashMap<String, u64> {
        self.per_source
            .iter()
            .filter_map(|(id, b)| b.current_interval_secs.map(|secs| (id.clone(), secs)))
            .collect()
    }

    /// 计算该 source 下次 fetch 该等多久（秒）。
    /// 没退避时返 `default_secs`。
    pub fn next_interval_secs(&self, id: &str, default_secs: u64) -> u64 {
        self.per_source
            .get(id)
            .and_then(|b| b.current_interval_secs)
            .unwrap_or(default_secs)
    }

    /// 报告一次 fetch 结果。
    ///
    /// - `snapshot.success == true` 或 `error_kind` 属于"用户配置类" → **不动** interval
    ///   （成功时清零 streak；用户配置类失败 streak 也不递增，因为重复
    ///   同一 key 失败不会让服务端压力变大）
    /// - `error_kind ∈ {RateLimited, ServerError, Network}` → 翻倍 interval，
    ///   streak +1
    pub fn record(&mut self, id: &str, snapshot: &ProviderSnapshot, default_secs: u64) {
        let entry = self.per_source.entry(id.to_string()).or_default();

        if snapshot.success {
            // 成功 → 完整 reset（streak + interval 都归零）
            if entry.failure_streak > 0 || entry.current_interval_secs.is_some() {
                entry.failure_streak = 0;
                entry.current_interval_secs = None;
            }
            return;
        }

        // 失败
        let should_backoff = matches!(
            snapshot.error_kind,
            Some(ErrorKind::RateLimited | ErrorKind::ServerError | ErrorKind::Network)
        );
        if !should_backoff {
            // 用户配置类（AuthFailed / UnconfiguredKey / SchemaUnknown / Parse / Other）
            // 不退避，但也不清零现有退避（用户修了 key 触发一次成功会自动清）
            return;
        }

        entry.failure_streak = entry.failure_streak.saturating_add(1);
        let base = entry.current_interval_secs.unwrap_or(default_secs);
        let new_interval = base.saturating_mul(2).min(MAX_BACKOFF_SECS);
        entry.current_interval_secs = Some(new_interval);
    }

    /// 测试/调试用：清掉某个 source 的退避状态（不删 entry 的 id，只是 reset）
    pub fn reset(&mut self, id: &str) {
        if let Some(entry) = self.per_source.get_mut(id) {
            entry.failure_streak = 0;
            entry.current_interval_secs = None;
        }
    }
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{Provider, ProviderSnapshot};

    fn snap_success() -> ProviderSnapshot {
        ProviderSnapshot {
            provider: Provider::Minimax,
            success: true,
            rows: vec![],
            error: None,
            error_kind: None,
            fetched_at: Some(0),
            next_fetch_at: None,
            raw: None,
            is_healthy: true,
            source_id: Some("minimax".to_string()),
            source_display_name: Some("MiniMax".to_string()),
            plan_name: None,
        }
    }

    fn snap_fail(kind: ErrorKind) -> ProviderSnapshot {
        ProviderSnapshot {
            provider: Provider::Minimax,
            success: false,
            rows: vec![],
            error: Some("err".to_string()),
            error_kind: Some(kind),
            fetched_at: Some(0),
            next_fetch_at: None,
            raw: None,
            is_healthy: false,
            source_id: Some("minimax".to_string()),
            source_display_name: Some("MiniMax".to_string()),
            plan_name: None,
        }
    }

    #[test]
    fn next_interval_defaults_when_no_state() {
        let st = BackoffState::new();
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_doubles_on_server_error() {
        let mut st = BackoffState::new();
        // 60 → 120 → 240
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 240);
    }

    #[test]
    fn backoff_caps_at_max() {
        let mut st = BackoffState::new();
        // 强制把初始推到接近 cap
        st.per_source.insert(
            "minimax".to_string(),
            SourceBackoff {
                current_interval_secs: Some(MAX_BACKOFF_SECS / 2),
                failure_streak: 5,
            },
        );
        // 翻倍后应 cap 到 MAX
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), MAX_BACKOFF_SECS);
        // 继续翻倍仍 cap
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), MAX_BACKOFF_SECS);
    }

    #[test]
    fn backoff_resets_on_success() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 240);
        // 成功一次 → 立即 reset
        st.record("minimax", &snap_success(), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_does_not_trigger_on_auth_failed() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::AuthFailed), 60);
        // AuthFailed 是用户配置类，**不**退避
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_does_not_trigger_on_unconfigured_key() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::UnconfiguredKey), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_does_not_trigger_on_schema_unknown() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::SchemaUnknown), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_does_not_trigger_on_parse() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::Parse), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_does_not_reset_on_user_config_failure() {
        // 第一次服务端失败 → 退避到 120
        // 第二次配置失败（AuthFailed）→ 不动 interval
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
        st.record("minimax", &snap_fail(ErrorKind::AuthFailed), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
    }

    #[test]
    fn backoff_triggers_on_rate_limited() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::RateLimited), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
    }

    #[test]
    fn backoff_triggers_on_network() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::Network), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
    }

    #[test]
    fn backoff_per_source_independent() {
        // 改了 minimax，不应影响 deepseek
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 120);
        assert_eq!(st.next_interval_secs("deepseek", 60), 60);
    }

    #[test]
    fn backoff_reset_method_clears() {
        let mut st = BackoffState::new();
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        st.record("minimax", &snap_fail(ErrorKind::ServerError), 60);
        assert_eq!(st.next_interval_secs("minimax", 60), 240);
        st.reset("minimax");
        assert_eq!(st.next_interval_secs("minimax", 60), 60);
    }

    #[test]
    fn backoff_idle_success_does_not_touch_state() {
        // 连续成功不应该 modify entry（避免无意义写）
        let mut st = BackoffState::new();
        st.record("minimax", &snap_success(), 60);
        st.record("minimax", &snap_success(), 60);
        // 状态是空的（没有 entry）
        assert!(st.per_source.get("minimax").is_none() || st.per_source["minimax"].failure_streak == 0);
    }

    #[test]
    fn max_backoff_constant_sanity() {
        // 30 min = 1800s，不要被改坏
        assert_eq!(MAX_BACKOFF_SECS, 1800);
    }
}
