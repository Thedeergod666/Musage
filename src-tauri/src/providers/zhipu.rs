//! 智谱 GLM Coding Plan 用量查询
//!
//! 端点：`GET https://open.bigmodel.cn/api/monitor/usage/quota/limit`
//! 鉴权：`Authorization: <api_key>` ⚠️ **不加 Bearer 前缀**（智谱特殊）
//!
//! ## 用途
//!
//! 智谱 BigModel（开放平台）的 GLM Coding Plan 套餐，5h + 周双窗口。
//! CCSwitch 已有 [同款实现](https://github.com/farion1231/cc-switch/blob/main/src-tauri/src/services/coding_plan.rs)
//! 可以参考（parse_zhipu_token_tiers 的 unit=3/6 分类逻辑）。
//!
//! ## 响应 schema
//!
//! ```json
//! {
//!   "success": true,
//!   "data": {
//!     "level": "pro",
//!     "limits": [
//!       {
//!         "type": "TOKENS_LIMIT",
//!         "unit": 3,            // 3 = 5小时, 6 = 每周
//!         "number": 5,
//!         "percentage": 12.0,   // 已用百分比 0-100
//!         "nextResetTime": 1749000000000  // epoch 毫秒
//!       },
//!       {
//!         "type": "TOKENS_LIMIT",
//!         "unit": 6,
//!         "number": 7,
//!         "percentage": 86.0,
//!         "nextResetTime": 1749800000000
//!       },
//!       { "type": "TIME_LIMIT", "percentage": 7.0 }  // 非 TOKENS_LIMIT 跳过
//!     ]
//!   }
//! }
//! ```
//!
//! ## 关键坑
//!
//! 1. **鉴权不加 Bearer**：智谱特殊；`Authorization: <api_key>` 整段裸 key
//! 2. **unit 字段分类**：不能按 `nextResetTime` 排序代替，因为周期末尾周桶
//!    会比 5h 桶更早重置（参考 ccswitch issue #3036）
//! 3. **5h 桶 0% 时可能没有 nextResetTime**：启发式兜底：5h 优先取没有
//!    resetTime 的，剩下的按 reset 升序填入仍空缺的槽位
//! 4. **老套餐只回 1 条 TOKENS_LIMIT**：自然降级为只显示 5h
//! 5. **国际版（api.z.ai）**：与国区 schema 完全一致；base_url 二选一

use std::pin::Pin;

use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};

const URL_CN: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
// 国际版（api.z.ai）schema 一致；未来加 region 切换时启用：
// const URL_EN: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct ZhipuSource;

impl Default for ZhipuSource {
    fn default() -> Self { Self }
}

impl QuotaSource for ZhipuSource {
    fn id(&self) -> &'static str { "zhipu" }
    fn display_name(&self) -> &'static str { "智谱 GLM" }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        // Zhipu 走默认 CN endpoint；EN 通过自定义 URL 或未来 cfg 字段切换
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured("未配置智谱 GLM API key（设置面板填入）"));
            }
            do_fetch(api_key, URL_CN).await
        })
    }
}

async fn do_fetch(api_key: &str, url: &str) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();

    // ⚠️ 智谱鉴权不加 Bearer —— 直接用裸 key
    let resp = client
        .get(url)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .header("Accept-Language", "en-US,en")
        .send()
        .await
        .map_err(|e| FetchError::network(format!("智谱 GLM 网络错误: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth("鉴权失败 —— 智谱 GLM API key 无效（注意 key 不要加 Bearer 前缀）"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(FetchError::server(format!(
            "智谱 GLM 服务异常 (HTTP {status}): {}",
            body.chars().take(200).collect::<String>()
        )));
    }

    let raw: Value = resp
        .json()
        .await
        .map_err(|e| FetchError::parse(format!("响应不是 JSON: {e}")))?;

    // 业务级 success 检查
    if raw.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let msg = raw.get("msg").and_then(|v| v.as_str()).unwrap_or("Unknown error");
        return Err(FetchError::server(format!("智谱 GLM API error: {msg}")));
    }

    parse(&raw)
}

/// 解析智谱 quota 响应 → QuotaRow 列表。
///
/// 分类策略（参考 ccswitch parse_zhipu_token_tiers，issue #3036）：
/// 1. 显式字段 `unit`：3 → 5h, 6 → weekly
/// 2. 兜底启发式（unit 缺失或不识别）：无 resetTime 的优先归 5h（5h 桶 0%
///    时可能没 reset），其余按 reset 升序填入仍空缺的槽位
/// 3. 老套餐只回 1 条 TOKENS_LIMIT → 自然降级为只显示 5h
fn parse(raw: &Value) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let data = raw
        .get("data")
        .ok_or_else(|| FetchError::parse("智谱响应缺少 data 字段".to_string()))?;

    let (five_h, weekly) = classify_zhipu_limits(data);

    let mut rows = Vec::new();

    if let Some((pct, resets_at)) = five_h {
        rows.push(QuotaRow {
            label: "5h".to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: Some("%".to_string()),
            extra: None,
        });
    }
    if let Some((pct, resets_at)) = weekly {
        rows.push(QuotaRow {
            label: "周".to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: Some("%".to_string()),
            extra: None,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            "智谱响应里没找到任何 TOKENS_LIMIT 条目".to_string(),
        ));
    }

    let plan_name = data
        .get("level")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(ProviderSnapshot {
        // 沿用 Provider::Minimax 是 Zhipu 还没有自己的 enum 变体；
        // source_id 才是前端应该用的字段。
        provider: super::Provider::Minimax,
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some("zhipu".to_string()),
        source_display_name: Some("智谱 GLM".to_string()),
        plan_name,
    })
}

/// 把智谱 `data.limits[]` 按 unit 字段分类成 (5h_row, weekly_row)。
///
/// 返回 (utilization%, resets_at_ms)。
///
/// 参考 ccswitch `parse_zhipu_token_tiers`：
/// - 显式 `unit=3` → 5h
/// - 显式 `unit=6` → weekly
/// - 未识别 unit 的条目进 unclassified 兜底：
///   - 优先把无 resetTime 的归 5h（5h 桶 0% 时可能没 reset）
///   - 其余按 reset 升序填入仍空缺的槽位
fn classify_zhipu_limits(data: &Value) -> (Option<(f64, Option<i64>)>, Option<(f64, Option<i64>)>) {
    type Entry = (i64, f64, Option<i64>); // (reset_ms_or_MiN, percentage, resets_at_ms)

    let mut five_h: Option<Entry> = None;
    let mut weekly: Option<Entry> = None;
    let mut unclassified: Vec<Entry> = Vec::new();

    if let Some(limits) = data.get("limits").and_then(|v| v.as_array()) {
        for item in limits {
            let limit_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            // 大小写不敏感：上游若把 "TOKENS_LIMIT" 改成小写或驼峰，依然识别
            if !limit_type.eq_ignore_ascii_case("TOKENS_LIMIT") {
                continue;
            }
            let percentage = item
                .get("percentage")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let reset_ms = item.get("nextResetTime").and_then(|v| v.as_i64());
            // 排序键：None 排最前（无 resetTime 的优先归 5h）
            let sort_key = reset_ms.unwrap_or(i64::MIN);
            let entry = (sort_key, percentage, reset_ms);

            match item.get("unit").and_then(|v| v.as_i64()) {
                Some(3) if five_h.is_none() => five_h = Some(entry),
                Some(6) if weekly.is_none() => weekly = Some(entry),
                _ => unclassified.push(entry),
            }
        }
    }

    // 兜底：按 reset 升序（无 reset 的排最前）依次填入空缺槽位
    unclassified.sort_by_key(|(sort_key, _, _)| *sort_key);
    for entry in unclassified {
        if five_h.is_none() {
            five_h = Some(entry);
        } else if weekly.is_none() {
            weekly = Some(entry);
        }
        // 智谱当前最多 2 条 TOKENS_LIMIT，多余的忽略
        else {
            break;
        }
    }

    (
        five_h.map(|(_, pct, reset)| (pct, reset)),
        weekly.map(|(_, pct, reset)| (pct, reset)),
    )
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_new_plan_two_tiers() {
        let raw = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 44.0, "nextResetTime": 1_000_000_000_000_i64 },
                    { "type": "TOKENS_LIMIT", "unit": 6, "number": 7, "percentage": 53.0, "nextResetTime": 2_000_000_000_000_i64 },
                    { "type": "TIME_LIMIT",   "percentage": 7.0 }
                ]
            }
        });
        let snap = parse(&raw).expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("zhipu"));
        assert_eq!(snap.plan_name.as_deref(), Some("pro"));
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, "5h");
        assert!((five_h.utilization.unwrap() - 44.0).abs() < 0.001);
        assert_eq!(five_h.resets_at, Some(1_000_000_000_000));

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, "周");
        assert!((weekly.utilization.unwrap() - 53.0).abs() < 0.001);
        assert_eq!(weekly.resets_at, Some(2_000_000_000_000));
    }

    #[test]
    fn parse_old_plan_single_tier_falls_back_to_5h() {
        // 老套餐（2026-02-12 前订阅）：仅一条 TOKENS_LIMIT，无周桶
        let raw = json!({
            "success": true,
            "data": {
                "level": "free",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 3, "percentage": 2.0, "nextResetTime": 1_774_967_594_803_i64 },
                    { "type": "TIME_LIMIT", "percentage": 0.0 }
                ]
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, "5h");
        assert!((snap.rows[0].utilization.unwrap() - 2.0).abs() < 0.001);
    }

    #[test]
    fn parse_no_token_limits_is_error() {
        let raw = json!({
            "success": true,
            "data": { "level": "free", "limits": [{ "type": "TIME_LIMIT", "percentage": 5.0 }] }
        });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_5h_zero_pct_no_reset_fallback_heuristic() {
        // 真实反馈：5h 桶 0% 时可能没有 nextResetTime；每周桶带 reset。
        // 这种形态不能按 reset 升序把每周桶误判为 5h（unit 显式分类兜底）。
        let raw = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 6, "percentage": 25.0, "nextResetTime": 2_000_000_000_000_i64 },
                    { "type": "TOKENS_LIMIT", "unit": 3, "percentage": 0.0 }
                ]
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, "5h");
        assert!((snap.rows[0].utilization.unwrap()).abs() < 0.001);
        assert_eq!(snap.rows[0].resets_at, None);
        assert_eq!(snap.rows[1].label, "周");
        assert!((snap.rows[1].utilization.unwrap() - 25.0).abs() < 0.001);
    }

    #[test]
    fn parse_unit_missing_uses_heuristic() {
        // 没 unit 字段的条目（schema 异常）走兜底：按 reset 升序
        let raw = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "percentage": 11.0, "nextResetTime": 1_000_000_000_000_i64 },
                    { "type": "TOKENS_LIMIT", "percentage": 22.0, "nextResetTime": 2_000_000_000_000_i64 }
                ]
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 2);
        // reset 较早的归 5h
        assert_eq!(snap.rows[0].label, "5h");
        assert!((snap.rows[0].utilization.unwrap() - 11.0).abs() < 0.001);
        assert_eq!(snap.rows[1].label, "周");
        assert!((snap.rows[1].utilization.unwrap() - 22.0).abs() < 0.001);
    }

    #[test]
    fn parse_no_unit_no_reset_picks_first_as_5h() {
        // 两个都没 unit 也没 reset 的边界：第一个归 5h，第二个归 weekly
        let raw = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "percentage": 11.0 },
                    { "type": "TOKENS_LIMIT", "percentage": 22.0 }
                ]
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, "5h");
        assert!((snap.rows[0].utilization.unwrap() - 11.0).abs() < 0.001);
        assert_eq!(snap.rows[1].label, "周");
        assert!((snap.rows[1].utilization.unwrap() - 22.0).abs() < 0.001);
    }

    #[test]
    fn parse_case_insensitive_type() {
        let raw = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "tokens_limit", "unit": 3, "percentage": 33.0 },
                    { "type": "Tokens_Limit", "unit": 6, "percentage": 44.0 }
                ]
            }
        });
        let snap = parse(&raw).expect("parse");
        assert_eq!(snap.rows.len(), 2);
    }

    #[test]
    fn parse_missing_data_is_error() {
        let raw = json!({ "success": true });
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_success_false_is_error() {
        let raw = json!({ "success": false, "msg": "API key invalid" });
        let err = parse(&raw).unwrap_err();
        // success=false → server error（业务级 4xx）
        assert_eq!(err.kind, FetchError::server("test").kind);
    }
}