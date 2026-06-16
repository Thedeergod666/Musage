//! Qwen（阿里 DashScope 百炼）用量查询 —— **STUB**
//!
//! ## 状态
//!
//! **当前不可用**。阿里云 DashScope 公开 API **没有 quota endpoint**：
//!
//! - [CodexBar issue #612](https://github.com/steipete/CodexBar/issues/612) 明确说
//!   "No DashScope quota API exists"，作者实测了**所有** plausible endpoint 都失败
//! - [CodexBar docs/alibaba-token-plan.md](https://github.com/steipete/CodexBar/blob/main/docs/alibaba-token-plan.md)
//!   唯一可靠方式是用 **Coding Plan** 专属 endpoint（`coding-intl.dashscope.aliyuncs.com`）
//!   + 用 OAuth 登录 flow
//!
//! plan URL `https://dashscope.aliyuncs.com/api/v1/account/quota` 是**猜测**，
//! Phase 0 实测不可达。
//!
//! ## 计划（Phase X）
//!
//! 参考 [CodexBar alibaba-token-plan.md](https://github.com/steipete/CodexBar/blob/main/docs/alibaba-token-plan.md)：
//! - 走阿里 Coding Plan OAuth flow（类似 StepFun 3 步登录）
//! - `POST https://coding-intl.dashscope.aliyuncs.com/v1/quota/remaining`（待实测）
//! - 或订阅页 dashboard HTML 解析（不推荐，脆）

use std::borrow::Cow;
use std::pin::Pin;

use super::{AuthKind, Credentials, ErrorKind, FetchError, ProviderSnapshot, QuotaSource};

// ── QuotaSource 实现 ─────────────────────────────────────────────

pub struct QwenSource;

impl Default for QwenSource {
    fn default() -> Self { Self }
}

impl QuotaSource for QwenSource {
    fn id(&self) -> Cow<'_, str> { Cow::Borrowed("qwen") }
    fn display_name(&self) -> Cow<'_, str> { Cow::Borrowed("Qwen（DashScope）") }
    fn auth_kind(&self) -> AuthKind { AuthKind::ApiKey }

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
                return Err(FetchError::unconfigured("未配置 Qwen DashScope API key（设置面板填入）"));
            }
            // ⚠️ STUB: 真实 fetch 路径未实现。DashScope 公开 API 无 quota endpoint；
            // 只有 Coding Plan 走 OAuth flow + 私有 endpoint（参考 CodexBar alibaba-token-plan.md），
            // 工作量约 1 周，Phase X 补。
            Err(FetchError::new(
                ErrorKind::ServerError,
                "Qwen / DashScope 暂未支持 —— 公开 API 无 quota endpoint \
                 （CodexBar issue #612 实测确认）。Phase X 走 Coding Plan OAuth \
                 flow（参考 CodexBar alibaba-token-plan.md）补 do_fetch。",
            ))
        })
    }
}

// ── 单元测试 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_without_key_is_unconfigured() {
        let src = QwenSource::default();
        let creds = Credentials::default();
        let err = src.fetch(&creds).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::UnconfiguredKey);
    }

    #[tokio::test]
    async fn fetch_with_key_returns_not_implemented() {
        let src = QwenSource::default();
        let creds = Credentials {
            api_key: Some("sk-test-1234567890".to_string()),
            cookie: None,
        };
        let err = src.fetch(&creds).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::ServerError);
        assert!(err.message.contains("Qwen") || err.message.contains("DashScope"));
    }

    #[tokio::test]
    async fn set_state_is_noop() {
        let src = QwenSource::default();
        src.set_state(serde_json::json!({ "anything": true })).await;
        // 不 panic 即可
    }
}
