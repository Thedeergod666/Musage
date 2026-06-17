//! Novita AI 用量查询 —— **STUB**
//!
//! ## 状态
//!
//! **当前不可用**。Novita AI 公开 API ref 完全没有 balance / quota endpoint
//! （[https://novita.ai/docs/api-reference/api-reference-overview](https://novita.ai/docs/api-reference/api-reference-overview)，
//! 2026-06-16 确认）。plan URL 里的 `/v1/user/balance` 是**猜测**，
//! Phase 0 实测不可达。
//!
//! 详见 plan §10 风险表：
//! > | StepFun/Novita schema 跟预想差太多 | 中 | Phase 0 curl 验证后如实填, Phase 1 真 key 失败就 mark TODO, 不影响其它 |
//!
//! 本 stub 的目的：
//! 1. 让前端设置面板能识别 `novita` 这个 source（settings.ts / UI 可正常渲染）
//! 2. fetch 永远返回明确的"未实现"错，用户看到后知道不是 bug
//! 3. 后续如果 Novita 开放了 quota API，**只改 `do_fetch` 一个函数**即可
//!
//! ## 计划（Phase X）
//!
//! 一旦 Novita 公开 balance API，按 SiliconFlow 模式做：
//! - `GET https://api.novita.ai/v1/user/balance` (假设)
//! - `Authorization: Bearer <api_key>`
//! - 响应字段 `balance` (number) + `currency` ("USD" 假设)

use std::borrow::Cow;
use std::pin::Pin;

use super::{AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaSource};

use crate::t;

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct NovitaSource;

impl Default for NovitaSource {
    fn default() -> Self { Self }
}

impl QuotaSource for NovitaSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("novita") }
    fn display_name(&self) -> Cow<'_, str> { Cow::Borrowed("Novita AI") }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }
    // STUB: 公开 API 无 quota endpoint,默认不拉。用户显式启用 → 仍可拉(返 "未支持" 错)。
    fn default_enabled(&self) -> bool { false }
    fn is_stub(&self) -> bool { true }

    fn set_state<'a>(
        &'a self,
        _cfg: serde_json::Value,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {})
    }

    fn fetch<'a>(
        &'a self,
        credentials: &'a Credentials,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ProviderSnapshot, FetchError>> + Send + 'a>> {
        Box::pin(async move {
            let api_key = credentials.api_key.as_deref().unwrap_or("").trim();
            if api_key.is_empty() {
                return Err(FetchError::unconfigured(
                    t!("error.provider.unconfigured_key", provider = "Novita AI").into_owned()
                ));
            }
            // ⚠️ STUB: 真实 fetch 路径未实现。Novita 公开 API ref 没有 balance endpoint，
            // 等官方开放或 community 提供 hack 后再实现。
            Err(FetchError::new(
                ErrorKind::ServerError,
                t!("error.provider.not_supported",
                    provider = "Novita AI",
                    reason = "public API ref has no balance/quota endpoint (confirmed 2026-06-16)"
                ).into_owned()
            ))
        })
    }
}

// ── 单元测试 ─────────────────────────────────────────────────────
//
// STUB 阶段只验证 auth check + 未实现错误；后续接入真 schema 后加 parse 测试。

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_without_key_is_unconfigured() {
        let src = NovitaSource::default();
        let creds = Credentials::default();
        let err = src.fetch(&creds).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnconfiguredKey);
    }

    #[tokio::test]
    async fn fetch_with_key_returns_not_implemented() {
        let src = NovitaSource::default();
        let creds = Credentials {
            api_key: Some("sk-test-1234567890".to_string()),
            cookie: None,
        };
        let err = src.fetch(&creds).await.unwrap_err();
        // STUB 错误走 ServerError kind（不是 SchemaUnknown，避免前端误以为 schema 不对）
        assert_eq!(err.kind, ErrorKind::ServerError);
        assert!(err.message.contains("Novita"));
        assert!(err.message.contains("未支持") || err.message.contains("未实现") || err.message.contains("暂未"));
    }

    #[tokio::test]
    async fn set_state_is_noop() {
        let src = NovitaSource::default();
        src.set_state(serde_json::json!({ "anything": true })).await;
        // 不 panic 即可
    }
}
