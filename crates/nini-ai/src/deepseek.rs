//! DeepSeek provider (OpenAI-compat with DeepSeek base URL).
//!
//! DeepSeek's API is fully OpenAI-compatible. Auth via `DEEPSEEK_API_KEY`.

use super::openai_compat::OpenAiCompatProvider;
use futures_core::Stream;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
use std::pin::Pin;

pub const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com/v1";

pub struct DeepSeekProvider {
    inner: OpenAiCompatProvider,
}

impl DeepSeekProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(DEEPSEEK_BASE_URL, api_key),
        }
    }

    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(base_url, api_key),
        }
    }
}

impl Provider for DeepSeekProvider {
    fn name(&self) -> &'static str {
        "deepseek"
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
    fn deepseek_provider_name() {
        let p = DeepSeekProvider::new("dummy");
        assert_eq!(p.name(), "deepseek");
    }

    #[test]
    fn deepseek_capabilities() {
        let p = DeepSeekProvider::new("dummy");
        let caps = p.capabilities();
        assert!(caps.streaming);
        assert!(caps.tool_use);
    }

    #[test]
    fn deepseek_base_url_correct() {
        assert!(DEEPSEEK_BASE_URL.starts_with("https://api.deepseek.com"));
    }
}
