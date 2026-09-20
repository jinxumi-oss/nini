//! Mistral AI provider (OpenAI-compat with Mistral base URL).
//!
//! Mistral's chat completions endpoint at `https://api.mistral.ai/v1`
//! is fully OpenAI-compatible. Auth via `MISTRAL_API_KEY`.
//!
//! Example models: `mistral-large-latest`, `mistral-small-latest`,
//! `codestral-latest`, `open-mistral-7b`.

use super::openai_compat::OpenAiCompatProvider;
use futures_core::Stream;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
use std::pin::Pin;

pub const MISTRAL_BASE_URL: &str = "https://api.mistral.ai/v1";

pub struct MistralProvider {
    inner: OpenAiCompatProvider,
}

impl MistralProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(MISTRAL_BASE_URL, api_key),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(base_url, api_key),
        }
    }
}

impl Provider for MistralProvider {
    fn name(&self) -> &'static str {
        "mistral"
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn stream(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        self.inner.stream(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_provider_name() {
        let p = MistralProvider::new("dummy");
        assert_eq!(p.name(), "mistral");
    }

    #[test]
    fn mistral_capabilities() {
        let p = MistralProvider::new("dummy");
        let caps = p.capabilities();
        assert!(caps.streaming);
        assert!(caps.tool_use);
    }

    #[test]
    fn mistral_base_url_correct() {
        assert!(MISTRAL_BASE_URL.starts_with("https://api.mistral.ai"));
    }
}
