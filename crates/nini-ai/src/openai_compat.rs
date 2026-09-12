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
