//! Cohere provider (OpenAI-compat with Cohere base URL).
//!
//! Cohere's v2 chat API is largely OpenAI-compatible
//! (https://api.cohere.com/v1). Some model names differ
//! (command-r-plus, command-r, command-light, etc.) but the protocol
//! is the same. Auth via `COHERE_API_KEY`.
//!
//! Example models: `command-r-plus`, `command-r`, `command-light`,
//! `c4ai-command-r-plus`.

use super::openai_compat::OpenAiCompatProvider;
use futures_core::Stream;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
use std::pin::Pin;

pub const COHERE_BASE_URL: &str = "https://api.cohere.com/v1";

pub struct CohereProvider {
    inner: OpenAiCompatProvider,
}

impl CohereProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(COHERE_BASE_URL, api_key),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(base_url, api_key),
        }
    }
}

impl Provider for CohereProvider {
    fn name(&self) -> &'static str {
        "cohere"
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
    fn cohere_provider_name() {
        let p = CohereProvider::new("dummy");
        assert_eq!(p.name(), "cohere");
    }

    #[test]
    fn cohere_capabilities() {
        let p = CohereProvider::new("dummy");
        let caps = p.capabilities();
        assert!(caps.streaming);
        assert!(caps.tool_use);
    }

    #[test]
    fn cohere_base_url_correct() {
        assert!(COHERE_BASE_URL.starts_with("https://api.cohere.com"));
    }
}
