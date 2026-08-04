//! Kimi Desktop 本地会话读取 + kimi-auth JWT 校验（v0.2.5「总套餐」特性）
//!
//! ## 背景
//!
//! Kimi「总套餐」（`FEATURE_OMNI` 月度共享额度池，网页端「我的额度」页的
//! 总使用量进度条）只暴露在 `www.kimi.com` 的网页会话网关
//! （`MembershipService/GetSubscriptionStats`），鉴权要 `kimi-auth` cookie
//! 里的会话 JWT。API key 的 `authentication.scope` 被锁在 `FEATURE_CODING`
//! （2026-08-04 实测调网页网关 401 `REASON_INVALID_AUTH_TOKEN`），
//! 拿不到总池数据。
//!
//! ## 会话获取路径（v1 零交互）
//!
//! 直接读 Kimi Desktop（官方桌面端）的 Chromium Cookies SQLite 库里的
//! `kimi-auth` cookie —— CodexBar `KimiDesktopAuthToken.swift` 同款方案。
//! 2026-08-04 macOS 实测：`value` 列明文存储（`encrypted_value` 为空），
//! 同机用户进程直接可读。Kimi Desktop 自己会刷新会话 → **每次 fetch
//! 重新读库即自动保鲜**，不需要往 keys.json 写。
//!
//! 平台路径（`dirs::config_dir()` 三平台恰好都命中）：
//! - macOS:   `~/Library/Application Support/kimi-desktop/Cookies`
//! - Windows: `%APPDATA%/Roaming/kimi-desktop/Cookies`
//! - Linux:   `~/.config/kimi-desktop/Cookies`
//!
//! ## 已知限制 / 降级
//!
//! - Windows 端 Chromium cookie 若是 DPAPI / App-Bound 加密（`value` 空、
//!   `encrypted_value` 非空）→ 读不出明文，返 None 自然降级（浮窗只显示
//!   5h/7d，不加总套餐行）。
//! - 用户没装 / 没登录 Kimi Desktop → 返 None 同样自然降级。
//! - JWT 过期 / 畸形 → [`validate_auth_token`] 返 None 降级。
//! - v2 备用路径：WebView 一键登录写 `kimi:cookie` 槽（provider 侧优先
//!   消费该槽，见 `providers/kimi.rs::resolve_session_token`）。

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;

/// kimi-auth cookie 的 host 白名单（对齐 CodexBar 的 SQL 查询）。
const HOST_KEYS: &[&str] = &["www.kimi.com", ".www.kimi.com", ".kimi.com", "kimi.com"];

/// 从 kimi-auth JWT 解出的请求头身份信息（CodexBar `SessionInfo` 同款：
/// `x-msh-device-id` ← `device_id`，`x-msh-session-id` ← `ssid`，
/// `x-traffic-id` ← `sub`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KimiSessionInfo {
    pub device_id: Option<String>,
    pub session_id: Option<String>,
    pub traffic_id: Option<String>,
}

/// 解 JWT payload 的 `exp` claim，返回距过期的秒数（>0 = 已过期 X 秒，
/// <0 = 还有 X 秒有效）。解析不出（非 JWT / 缺 exp / 格式漂移）返 None。
///
/// `pub(crate)`：`kimi_login.rs` 的「新鲜度门」复用同一判定（对齐
/// stepfun 的 `access_token_exp_seconds_ago` 共享模式），保证「登录存下来
/// 的 token」和「provider 预检接受的 token」标准一致。不做签名校验
///（CodexBar 同款 —— web 客户端正常做法，服务端会验）。
pub(crate) fn jwt_exp_seconds_ago(token: &str) -> Option<i64> {
    let payload_b64 = token.trim().split('.').nth(1)?;
    // 容忍带 padding 的 base64url（对齐 stepfun jwt_device_id）
    let payload_b64 = payload_b64.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v.get("exp").and_then(|x| x.as_i64())?;
    Some(chrono::Utc::now().timestamp() - exp)
}

/// 本地预检 kimi-auth JWT 有效性 + 解 claims。
///
/// 判定（任一不满足 → 返 None，调用方自然降级，**不报错**）：
/// - 三段 JWT 结构，payload 可 base64url 解码为 JSON
/// - `exp` claim 存在且未过期（留 60s skew，避免拿到边界时刻即死的 token）
pub fn validate_auth_token(token: &str) -> Option<KimiSessionInfo> {
    let secs_ago = jwt_exp_seconds_ago(token)?;
    const SKEW_SECS: i64 = 60;
    if secs_ago + SKEW_SECS >= 0 {
        return None;
    }
    let payload_b64 = token.trim().split('.').nth(1)?;
    let payload_b64 = payload_b64.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some(KimiSessionInfo {
        device_id: v
            .get("device_id")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        session_id: v.get("ssid").and_then(|x| x.as_str()).map(str::to_string),
        traffic_id: v.get("sub").and_then(|x| x.as_str()).map(str::to_string),
    })
}

/// kimi-desktop Cookies SQLite 库路径（三平台统一走 `dirs::config_dir()`）。
pub fn cookies_db_path() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("kimi-desktop").join("Cookies"))
}

/// 从 kimi-desktop 本地 Cookies 库读 `kimi-auth`（明文 `value` 列）。
///
/// 任何一步失败都返 None（自然降级）+ `tracing::debug` 日志 —— 这是
/// best-effort 增强路径，绝不能影响主 fetch。
pub fn load_desktop_auth_token() -> Option<String> {
    let path = cookies_db_path()?;
    if !path.is_file() {
        tracing::debug!(path = %path.display(), "[kimi] kimi-desktop Cookies 库不存在 → 跳过总套餐增强");
        return None;
    }
    match read_token_from_db(&path) {
        Ok(tok) => tok,
        Err(e) => {
            tracing::debug!(err = %e, "[kimi] 读 kimi-desktop Cookies 库失败 → 跳过总套餐增强");
            None
        }
    }
}

fn read_token_from_db(path: &std::path::Path) -> rusqlite::Result<Option<String>> {
    // 主路径：普通只读打开 + 250ms busy_timeout（kimi-desktop 运行中持有
    // WAL 时也能读到已提交记录）。失败（如干净退出后 WAL sidecar 已删但
    // 主库处于 WAL 模式打不开）→ immutable=1 URI 兜底（CodexBar 同款
    // 双路径策略）。
    match read_token_once(path, false) {
        Ok(tok) => Ok(tok),
        Err(_) => read_token_once(path, true),
    }
}

fn read_token_once(path: &std::path::Path, immutable: bool) -> rusqlite::Result<Option<String>> {
    let conn = if immutable {
        rusqlite::Connection::open_with_flags(
            format!("file:{}?immutable=1", path.display()),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?
    } else {
        let conn = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(250))?;
        conn
    };

    let mut stmt = conn.prepare(
        "SELECT value FROM cookies \
         WHERE name = 'kimi-auth' AND host_key IN (?1, ?2, ?3, ?4) \
         ORDER BY last_access_utc DESC LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        HOST_KEYS[0],
        HOST_KEYS[1],
        HOST_KEYS[2],
        HOST_KEYS[3]
    ])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let value: String = row.get(0)?;
    let value = value.trim().to_string();
    // value 空 = 该行走加密存储（Windows DPAPI / App-Bound）→ 无法明文读，
    // 返 None 降级（v2 走 WebView 登录路径）。
    if value.is_empty() {
        return Ok(None);
    }
    Ok(Some(value))
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 仿 stepfun.rs 测试：手工拼一个未签名的三段 JWT。
    fn make_jwt(claims: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(b"sig");
        format!("{header}.{payload}.{sig}")
    }

    #[test]
    fn validate_accepts_fresh_token_with_claims() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let jwt = make_jwt(&serde_json::json!({
            "exp": exp,
            "device_id": "dev-123",
            "ssid": "sess-456",
            "sub": "traffic-789"
        }));
        let info = validate_auth_token(&jwt).expect("fresh token should pass");
        assert_eq!(info.device_id.as_deref(), Some("dev-123"));
        assert_eq!(info.session_id.as_deref(), Some("sess-456"));
        assert_eq!(info.traffic_id.as_deref(), Some("traffic-789"));
    }

    #[test]
    fn validate_accepts_token_without_optional_claims() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let jwt = make_jwt(&serde_json::json!({ "exp": exp }));
        let info = validate_auth_token(&jwt).expect("exp-only token should pass");
        assert_eq!(info, KimiSessionInfo::default());
    }

    #[test]
    fn validate_rejects_expired_token() {
        let exp = chrono::Utc::now().timestamp() - 10;
        let jwt = make_jwt(&serde_json::json!({ "exp": exp }));
        assert_eq!(validate_auth_token(&jwt), None);
    }

    #[test]
    fn validate_rejects_token_expiring_within_skew() {
        // 30s 后过期 < 60s skew → 拒绝（边界时刻即死的 token 不用）
        let exp = chrono::Utc::now().timestamp() + 30;
        let jwt = make_jwt(&serde_json::json!({ "exp": exp }));
        assert_eq!(validate_auth_token(&jwt), None);
    }

    #[test]
    fn validate_rejects_malformed_tokens() {
        assert_eq!(validate_auth_token(""), None);
        assert_eq!(validate_auth_token("not-a-jwt"), None);
        assert_eq!(validate_auth_token("a.b"), None);
        // payload 不是 JSON
        let bad = format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(b"{}"),
            URL_SAFE_NO_PAD.encode(b"not json"),
            URL_SAFE_NO_PAD.encode(b"sig")
        );
        assert_eq!(validate_auth_token(&bad), None);
        // 缺 exp
        let no_exp = make_jwt(&serde_json::json!({ "device_id": "x" }));
        assert_eq!(validate_auth_token(&no_exp), None);
        // exp 是字符串而不是数字（schema 漂移防御）
        let str_exp = make_jwt(&serde_json::json!({ "exp": "9999999999" }));
        assert_eq!(validate_auth_token(&str_exp), None);
    }

    #[test]
    fn cookies_db_path_points_inside_config_dir() {
        let p = cookies_db_path().expect("config dir should exist on dev machine");
        assert!(p.ends_with("kimi-desktop/Cookies") || p.ends_with(r"kimi-desktop\Cookies"));
    }
}
