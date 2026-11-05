//! Generic OpenAI-compat preset.
//!
//! Wraps the OpenAI Chat Completions protocol so any service exposing a
//! `/v1/chat/completions` endpoint (OpenRouter, Groq, Together, vLLM, etc.)
//! can be used with just `base_url` + `api_key` + model id.

use super::openai;
use nini_core::provider::*;
use std::pin::Pin;

/// Provider that speaks the OpenAI Chat Completions protocol at a custom URL.
pub struct OpenAiCompatProvider {
    inner: openai::OpenAiProvider,
}

impl OpenAiCompatProvider {
    /// Create a new OpenAI-compat provider. `base_url` is the full root URL
    /// (e.g., `https://openrouter.ai/api`).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: openai::OpenAiProvider::new(api_key).with_base_url(base_url),
        }
    }

    /// v0.10 (R2b): declare this endpoint's wire differences.
    ///
    /// The OpenAI-compatible ecosystem is not uniform. Pi keeps a
    /// per-provider table for exactly this reason: `detectCompat`
    /// (`openai-completions.ts:1577-1670`) derives the differences from the
    /// provider name and the base URL, and `getCompat`
    /// (`openai-completions.ts:1682`) lets a model's own `compat` entry
    /// override the detected values. DeepSeek and its neighbours take
    /// `max_tokens` and reject `store`, while OpenAI takes
    /// `max_completion_tokens` and carries `store: false`. Without this the
    /// compat brands inherit OpenAI's spelling and fail.
    ///
    /// nini 的移植见本文件的 [`detect_compat`]。
    pub fn with_compat(mut self, compat: openai::OpenAiCompat) -> Self {
        self.inner = self.inner.with_compat(compat);
        self
    }
}

/// Pi 的 `detectCompat` 移植：按 **provider id + base URL** 推导这个端点的
/// wire 差异。
///
/// 源：`references/pi-0.85.1/src/api/openai-completions.ts:1577-1670`
/// —— `detectCompat(model)`；Pi 的 `getCompat`（同文件 `:1682`）在模型没有
/// 显式 `compat` 时调用它。
///
/// **为什么需要它**：OpenAI 兼容生态并不统一。DeepSeek / Together / Z.ai /
/// Moonshot / NVIDIA / Ant Ling / Cloudflare AI Gateway 这些端点收的是
/// `max_tokens` 而不是 `max_completion_tokens`，而且其中一批会直接拒绝
/// `store` 字段。沿用 OpenAI 的拼写，请求会失败 —— 而失败信息（400）
/// 并不会告诉用户「你发错了字段名」。
///
/// Pi 用「provider 名 + baseUrl 的子串」这套启发式判定；nini 照抄同一套判定，
/// 逐条与 Pi 对齐（见 `crates/nini-cli/tests/provider_registry.rs`）。
///
/// Pi 的 `detectCompat` 还返回 `supportsDeveloperRole`、`thinkingFormat`
/// 等字段，但那些是按 **model** 决定的（Pi 的 `getCompat` 用 `model.compat`
/// 覆盖它们），nini 的 `openai::OpenAiCompat` 没有建模它们。所以这里只映射
/// nini 建模了的两个：`max_tokens_field` 与 `supports_store`。
///
/// 判定里**没有**移植的两处（因为它们的唯一作用落在 nini 未建模的字段上）：
/// `isOpenRouter`（只影响 `supportsDeveloperRole` / `cacheControlFormat`）与
/// `isGrok`（只影响 `supportsReasoningEffort`）。
pub fn detect_compat(provider_id: &str, base_url: &str) -> openai::OpenAiCompat {
    let base = base_url;

    // Pi: `provider === "zai" || provider === "zai-coding-cn" || ...`
    let is_zai = provider_id == "zai"
        || provider_id == "zai-coding-cn"
        || base.contains("api.z.ai")
        || base.contains("open.bigmodel.cn");
    let is_together = provider_id == "together"
        || base.contains("api.together.ai")
        || base.contains("api.together.xyz");
    let is_moonshot = provider_id == "moonshotai"
        || provider_id == "moonshotai-cn"
        || base.contains("api.moonshot.");
    let is_cloudflare_workers_ai =
        provider_id == "cloudflare-workers-ai" || base.contains("api.cloudflare.com");
    let is_cloudflare_ai_gateway =
        provider_id == "cloudflare-ai-gateway" || base.contains("gateway.ai.cloudflare.com");
    let is_nvidia = provider_id == "nvidia" || base.contains("integrate.api.nvidia.com");
    let is_ant_ling = provider_id == "ant-ling" || base.contains("api.ant-ling.com");
    // Pi 只把 baseUrl 转成小写再比这一条（`baseUrl.toLowerCase().includes(...)`）。
    let is_deepseek = provider_id == "deepseek" || base.to_lowercase().contains("deepseek.com");

    // Pi: `isNonStandard` —— 这些端点不吃 `store` 字段。
    let is_non_standard = is_nvidia
        || provider_id == "cerebras"
        || base.contains("cerebras.ai")
        || provider_id == "xai"
        || base.contains("api.x.ai")
        || is_together
        || base.contains("chutes.ai")
        || is_deepseek
        || is_zai
        || is_moonshot
        || provider_id == "opencode"
        || base.contains("opencode.ai")
        || is_cloudflare_workers_ai
        || is_cloudflare_ai_gateway
        || is_ant_ling;

    // Pi: `useMaxTokens` —— 这些端点收 `max_tokens` 而不是
    // `max_completion_tokens`。
    let use_max_tokens = base.contains("chutes.ai")
        || is_deepseek
        || is_moonshot
        || is_cloudflare_ai_gateway
        || is_together
        || is_nvidia
        || is_ant_ling
        || is_zai;

    openai::OpenAiCompat {
        max_tokens_field: if use_max_tokens {
            openai::MaxTokensField::MaxTokens
        } else {
            openai::MaxTokensField::MaxCompletionTokens
        },
        supports_store: !is_non_standard,
    }
}

impl Provider for OpenAiCompatProvider {
    fn name(&self) -> &'static str {
        "openai-compat"
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn stream(
        &self,
        req: Request,
    ) -> Pin<
        Box<dyn futures_core::Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>,
    > {
        self.inner.stream(req)
    }
}
