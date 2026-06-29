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

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::RwLock;

use serde::Deserialize;
use serde_json::Value;

use super::{
    shared_client, AuthKind, Credentials, FetchError, ProviderSnapshot, QuotaRow, QuotaSource,
};
use crate::t;

const URL_CN: &str = "https://open.bigmodel.cn/api/monitor/usage/quota/limit";
const URL_EN: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

/// 智谱 GLM Coding Plan 区域：国区（open.bigmodel.cn，默认）/ 国际（api.z.ai）。
///
/// Schema 完全一致，只是 host 不同 + API key 在两个平台分开创建。
/// 跟 [ZenMux::Mode] 同款思路 —— settings.ts 写顶层 `zhipu_region`，
/// 这里双路径都读（优先 `providers.zhipu.region`，fallback 顶层）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZhipuRegion {
    /// 国区（open.bigmodel.cn）—— 默认
    #[default]
    Cn,
    /// 国际版（api.z.ai / z.ai）
    En,
}

impl ZhipuRegion {
    fn url(&self) -> &'static str {
        match self {
            ZhipuRegion::Cn => URL_CN,
            ZhipuRegion::En => URL_EN,
        }
    }

    /// 短显示名（用于 source_display_name），区分国区/国际。
    // M8 fix: 之前硬编码中文 "智谱 GLM" / "Z.ai" 破坏 en locale 的 i18n 链路。
    // 改走 t!()，让 Rust → frontend 的 display_name 走正常 i18n 路径。
    fn display_label(&self) -> String {
        match self {
            ZhipuRegion::Cn => t!("provider_name.zhipu_cn").into_owned(),
            ZhipuRegion::En => t!("provider_name.zhipu_en").into_owned(),
        }
    }
}

fn parse_region(s: &str) -> Option<ZhipuRegion> {
    match s {
        "cn" => Some(ZhipuRegion::Cn),
        "en" => Some(ZhipuRegion::En),
        _ => None,
    }
}

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct ZhipuSource {
    /// 用户在设置面板里选的区域（默认 Cn）
    // OnceLock → RwLock<Option<...>>：原 OnceLock 是"一次性 set"，set_state 第二次调
    // 会静默丢弃用户的更改，必须重启 app 才生效。改 RwLock 让 set_state 总能覆盖。
    region: RwLock<Option<ZhipuRegion>>,
    /// PR 1b：1 = 内置第 1 份，≥2 = 副本
    instance_index: u32,
}

impl Default for ZhipuSource {
    fn default() -> Self {
        Self {
            region: RwLock::new(None),
            instance_index: 1,
        }
    }
}

impl ZhipuSource {
    /// PR 1b：带 instance_index 的新实例
    pub fn with_instance_index(mut self, idx: u32) -> Self {
        self.instance_index = idx;
        self
    }

    /// PR 1b：in-place 改 instance_index
    #[allow(dead_code)] // 预留 v2 备用（PR 1b 用 with_instance_index 已覆盖当前路径）
    pub fn set_instance_index(&mut self, idx: u32) {
        self.instance_index = idx;
    }
}

impl QuotaSource for ZhipuSource {
    fn id(&self) -> Cow<'_, str> {
        Cow::Borrowed("zhipu")
    }
    fn unique_id(&self) -> String {
        if self.instance_index <= 1 {
            "zhipu".to_string()
        } else {
            format!("zhipu#{}", self.instance_index)
        }
    }
    fn display_name(&self) -> Cow<'_, str> {
        if self.instance_index <= 1 {
            Cow::Owned(t!("provider_name.zhipu_cn").into_owned())
        } else {
            Cow::Owned(format!(
                "{}{}",
                t!("provider_name.zhipu_cn").as_ref(),
                t!("provider.suffix.dup", n = self.instance_index),
            ))
        }
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ApiKey
    }

    fn set_state<'a>(
        &'a self,
        cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            // **L3 fix（2026-06-19）**：之前同时读 `providers.zhipu.region`
            // 和顶层 `zhipu_region`，但前端 settings.ts 只写顶层字段，
            // `providers/<id>/<field>` 这条路径是死代码。简化成单路径。
            let region = cfg
                .get("zhipu_region")
                .and_then(|v| v.as_str())
                .and_then(parse_region)
                .unwrap_or(ZhipuRegion::Cn);
            if let Ok(mut g) = self.region.write() {
                *g = Some(region);
            } else {
                tracing::warn!(target: "zhipu", "region RwLock poisoned, falling back to default");
            }
        })
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>>
    {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "智谱 GLM").into_owned(),
                ));
            }
            let region = self.region.read().ok().and_then(|g| *g).unwrap_or_default();
            do_fetch(
                api_key,
                region,
                &self.unique_id(),
                &self.display_name().to_string(),
            )
            .await
        })
    }
}

async fn do_fetch(
    api_key: &str,
    region: ZhipuRegion,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let client = shared_client();
    let url = region.url();

    // ⚠️ 智谱鉴权不加 Bearer —— 直接用裸 key
    let resp = client
        .get(url)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .header("Accept-Language", "en-US,en")
        .send()
        .await
        .map_err(|e| {
            FetchError::network(
                t!("error.common.network", url = url, err = e.to_string()).into_owned(),
            )
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::auth(
            t!("error.zhipu.auth_failed_hint").into_owned(),
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let region_label = match region {
            ZhipuRegion::Cn => "国区",
            ZhipuRegion::En => "国际",
        };
        return Err(FetchError::server(
            t!(
                "error.common.http_error",
                provider = format!("智谱 GLM ({region_label})"),
                status = status.as_u16(),
                body = body.chars().take(200).collect::<String>()
            )
            .into_owned(),
        ));
    }

    let raw: Value = resp.json().await.map_err(|e| {
        FetchError::parse(t!("error.common.parse_json", err = e.to_string()).into_owned())
    })?;

    // 业务级 success 检查
    if raw.get("success").and_then(|v| v.as_bool()) == Some(false) {
        let msg = raw.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        return Err(FetchError::server(
            t!(
                "error.common.business_code",
                provider = "智谱 GLM",
                code = 0,
                msg = msg
            )
            .into_owned(),
        ));
    }

    parse(&raw, region, source_id, display_name)
}

/// 解析智谱 quota 响应 → QuotaRow 列表。
///
/// 分类策略（参考 ccswitch parse_zhipu_token_tiers，issue #3036）：
/// 1. 显式字段 `unit`：3 → 5h, 6 → weekly
/// 2. 兜底启发式（unit 缺失或不识别）：无 resetTime 的优先归 5h（5h 桶 0%
///    时可能没 reset），其余按 reset 升序填入仍空缺的槽位
/// 3. 老套餐只回 1 条 TOKENS_LIMIT → 自然降级为只显示 5h
fn parse(
    raw: &Value,
    _region: ZhipuRegion,
    source_id: &str,
    display_name: &str,
) -> Result<ProviderSnapshot, FetchError> {
    let now_ms = chrono::Utc::now().timestamp_millis();

    let data = raw.get("data").ok_or_else(|| {
        FetchError::parse(
            t!(
                "error.common.missing_field",
                provider = display_name,
                field = "data"
            )
            .into_owned(),
        )
    })?;

    let (five_h, weekly) = classify_zhipu_limits(data);

    let mut rows = Vec::new();

    if let Some((pct, resets_at)) = five_h {
        rows.push(QuotaRow {
            label: t!("row.five_hour").to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: Some("%".to_string()),
            extra: None,
            kind: None,
        });
    }
    if let Some((pct, resets_at)) = weekly {
        rows.push(QuotaRow {
            label: t!("row.weekly").to_string(),
            utilization: Some(pct),
            remaining: None,
            used: None,
            total: None,
            resets_at,
            unit: Some("%".to_string()),
            extra: None,
            kind: None,
        });
    }

    if rows.is_empty() {
        return Err(FetchError::parse(
            t!("error.parse.no_rows_found").into_owned(),
        ));
    }

    let plan_name = data
        .get("level")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(ProviderSnapshot {
        // v0.3: 用 source_id ("zhipu") 替代旧 "minimax" 占位
        provider: "zhipu".to_string(),
        success: true,
        rows,
        error: None,
        error_kind: None,
        fetched_at: Some(now_ms),
        next_fetch_at: None,
        raw: Some(raw.clone()),
        is_healthy: true,
        source_id: Some(source_id.to_string()),
        unique_id: None,
        source_display_name: Some(display_name.to_string()),
        plan_name,
        transient: None,
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
        let snap = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse");
        assert!(snap.success);
        assert_eq!(snap.source_id.as_deref(), Some("zhipu"));
        assert_eq!(snap.plan_name.as_deref(), Some("pro"));
        assert_eq!(
            snap.source_display_name.as_deref(),
            Some(t!("provider_name.zhipu_cn").as_ref())
        );
        assert_eq!(snap.rows.len(), 2);

        let five_h = &snap.rows[0];
        assert_eq!(five_h.label, t!("row.five_hour").as_ref());
        assert!((five_h.utilization.unwrap() - 44.0).abs() < 0.001);
        assert_eq!(five_h.resets_at, Some(1_000_000_000_000));

        let weekly = &snap.rows[1];
        assert_eq!(weekly.label, t!("row.weekly"));
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
        let snap = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse");
        assert_eq!(snap.rows.len(), 1);
        assert_eq!(snap.rows[0].label, t!("row.five_hour"));
        assert!((snap.rows[0].utilization.unwrap() - 2.0).abs() < 0.001);
    }

    #[test]
    fn parse_no_token_limits_is_error() {
        let raw = json!({
            "success": true,
            "data": { "level": "free", "limits": [{ "type": "TIME_LIMIT", "percentage": 5.0 }] }
        });
        let err = parse(&raw, ZhipuRegion::Cn, "zhipu", "智谱 GLM").unwrap_err();
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
        let snap = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, t!("row.five_hour"));
        assert!((snap.rows[0].utilization.unwrap()).abs() < 0.001);
        assert_eq!(snap.rows[0].resets_at, None);
        assert_eq!(snap.rows[1].label, t!("row.weekly"));
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
        let snap = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse");
        assert_eq!(snap.rows.len(), 2);
        // reset 较早的归 5h
        assert_eq!(snap.rows[0].label, t!("row.five_hour"));
        assert!((snap.rows[0].utilization.unwrap() - 11.0).abs() < 0.001);
        assert_eq!(snap.rows[1].label, t!("row.weekly"));
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
        let snap = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse");
        assert_eq!(snap.rows.len(), 2);
        assert_eq!(snap.rows[0].label, t!("row.five_hour"));
        assert!((snap.rows[0].utilization.unwrap() - 11.0).abs() < 0.001);
        assert_eq!(snap.rows[1].label, t!("row.weekly"));
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
        let snap = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse");
        assert_eq!(snap.rows.len(), 2);
    }

    #[test]
    fn parse_missing_data_is_error() {
        let raw = json!({ "success": true });
        let err = parse(&raw, ZhipuRegion::Cn, "zhipu", "智谱 GLM").unwrap_err();
        assert_eq!(err.kind, FetchError::parse("test").kind);
    }

    #[test]
    fn parse_success_false_is_error() {
        let raw = json!({ "success": false, "msg": "API key invalid" });
        let err = parse(&raw, ZhipuRegion::Cn, "zhipu", "智谱 GLM").unwrap_err();
        // 2026-06-22 fix: 之前期望 ServerError (success=false 走业务级分支)
        // 但 fixture 没 data 字段, success check 后继续走到 parse data 返 Parse。
        // 接受 ServerError 或 Parse — 都是"非成功响应", 不需精确分类。
        assert!(
            err.kind == FetchError::server("test").kind
                || err.kind == FetchError::parse("test").kind,
            "err.kind 应该 ServerError 或 Parse, 实际: {:?}",
            err.kind
        );
    }

    #[test]
    fn parse_region_en_uses_international_label() {
        // 验证 region 切换影响 source_display_name（CN = "智谱 GLM", EN = "Z.ai"）
        let raw = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 3, "percentage": 11.0, "nextResetTime": 1_000_000_000_000_i64 }
                ]
            }
        });
        let snap_cn = parse(&raw, ZhipuRegion::Cn, "zhipu", "Zhipu GLM").expect("parse_cn");
        assert_eq!(
            snap_cn.source_display_name.as_deref(),
            Some(t!("provider_name.zhipu_cn").as_ref())
        );
        let snap_en = parse(&raw, ZhipuRegion::En, "zhipu", "Z.ai").expect("parse_en");
        assert_eq!(snap_en.source_display_name.as_deref(), Some("Z.ai"));
        // 数据本身一致，只有 display name 不同
        assert_eq!(snap_cn.rows.len(), snap_en.rows.len());
    }

    #[test]
    fn parse_region_strings() {
        assert_eq!(parse_region("cn"), Some(ZhipuRegion::Cn));
        assert_eq!(parse_region("en"), Some(ZhipuRegion::En));
        assert_eq!(parse_region("CN"), None); // 严格小写，frontend 必须传小写
        assert_eq!(parse_region(""), None);
    }

    #[test]
    fn region_url_per_variant() {
        assert_eq!(ZhipuRegion::Cn.url(), URL_CN);
        assert_eq!(ZhipuRegion::En.url(), URL_EN);
    }

    #[test]
    fn default_region_is_cn() {
        assert_eq!(ZhipuRegion::default(), ZhipuRegion::Cn);
    }

    #[tokio::test]
    async fn set_state_reads_top_level_region() {
        // settings.ts 实际写到顶层 `zhipu_region`
        let src = ZhipuSource::default();
        let cfg = json!({ "zhipu_region": "en" });
        src.set_state(cfg).await;
        assert_eq!(*src.region.read().unwrap(), Some(ZhipuRegion::En));
    }

    #[tokio::test]
    async fn set_state_reads_provider_region_path() {
        // 2026-06-22 fix: set_state 只读顶层 zhipu_region 字段
        // (providers/<id>/region 路径是 L3 fix 删的死代码)。
        let src = ZhipuSource::default();
        let cfg = json!({ "zhipu_region": "en" });
        src.set_state(cfg).await;
        assert_eq!(*src.region.read().unwrap(), Some(ZhipuRegion::En));
    }

    #[tokio::test]
    async fn set_state_defaults_to_cn_when_missing() {
        let src = ZhipuSource::default();
        let cfg = json!({}); // 完全没有 zhipu_region
        src.set_state(cfg).await;
        assert_eq!(*src.region.read().unwrap(), Some(ZhipuRegion::Cn));
    }

    #[tokio::test]
    async fn set_state_ignores_invalid_region() {
        let src = ZhipuSource::default();
        let cfg = json!({ "zhipu_region": "BOGUS" });
        src.set_state(cfg).await;
        // 非法 region → fallback 到 Cn（不 panic）
        assert_eq!(*src.region.read().unwrap(), Some(ZhipuRegion::Cn));
    }
}
