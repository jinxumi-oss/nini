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
    /// per-provider table for exactly this reason
    /// (`getCompatOptions`, `openai-completions.ts:1462-1502`): DeepSeek
    /// and its neighbours take `max_tokens` and reject `store`, while
    /// OpenAI takes `max_completion_tokens` and carries `store: false`.
    /// Without this the compat brands inherit OpenAI's spelling and fail.
    pub fn with_compat(mut self, compat: openai::OpenAiCompat) -> Self {
        self.inner = self.inner.with_compat(compat);
        self
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
