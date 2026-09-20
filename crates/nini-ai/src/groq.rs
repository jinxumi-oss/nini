//! Groq provider (OpenAI-compat with Groq base URL).
//!
//! Groq exposes `https://api.groq.com/openai/v1` for chat completions.
//! Auth via `GROQ_API_KEY`.

use super::openai_compat::OpenAiCompatProvider;
use futures_core::Stream;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
use std::pin::Pin;

pub const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";

pub struct GroqProvider {
    inner: OpenAiCompatProvider,
}

impl GroqProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(GROQ_BASE_URL, api_key),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(base_url, api_key),
        }
    }
}

impl Provider for GroqProvider {
    fn name(&self) -> &'static str {
        "groq"
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
    fn groq_provider_name() {
        let p = GroqProvider::new("dummy");
        assert_eq!(p.name(), "groq");
    }

    #[test]
    fn groq_capabilities() {
        let p = GroqProvider::new("dummy");
        let caps = p.capabilities();
        assert!(caps.streaming);
        assert!(caps.tool_use);
    }

    #[test]
    fn groq_base_url_correct() {
        assert!(GROQ_BASE_URL.starts_with("https://api.groq.com"));
    }
}
