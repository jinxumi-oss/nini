//! Google Gemini provider.
//!
//! Gemini exposes an OpenAI-compatible chat completions endpoint at
//! `https://generativelanguage.googleapis.com/v1beta/openai/`, so we
//! reuse the OpenAI-compat preset with a fixed base URL.
//!
//! Auth: pass the API key via env (`GOOGLE_API_KEY` or `GEMINI_API_KEY`)
//! or `--api-key`. nini auto-detects from env.
//!
//! Example models: `gemini-2.5-pro`, `gemini-2.5-flash`, `gemini-2.0-flash-exp`.

use super::openai_compat::OpenAiCompatProvider;
use futures_core::Stream;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};
use std::pin::Pin;

/// Default Gemini OpenAI-compat base URL.
pub const GEMINI_BASE_URL: &str =
    "https://generativelanguage.googleapis.com/v1beta/openai";

/// Provider for Google's Gemini family via the OpenAI-compat endpoint.
pub struct GoogleProvider {
    inner: OpenAiCompatProvider,
}

impl GoogleProvider {
    /// Create with the default base URL.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(GEMINI_BASE_URL, api_key),
        }
    }

    /// Create with a custom base URL (useful for testing).
    pub fn with_base_url(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            inner: OpenAiCompatProvider::new(base_url, api_key),
        }
    }
}

impl Provider for GoogleProvider {
    fn name(&self) -> &'static str {
        "google"
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
    fn google_provider_name_is_google() {
        let p = GoogleProvider::new("dummy-key");
        assert_eq!(p.name(), "google");
    }

    #[test]
    fn google_provider_capabilities_match_openai() {
        let p = GoogleProvider::new("dummy-key");
        let caps = p.capabilities();
        assert!(caps.streaming, "Gemini supports streaming");
        assert!(caps.tool_use, "Gemini supports tool use");
    }

    #[test]
    fn google_default_base_url() {
        assert!(GEMINI_BASE_URL.starts_with("https://generativelanguage"));
    }

    #[test]
    fn google_with_custom_base_url() {
        let p = GoogleProvider::with_base_url("https://custom.example/v1", "key");
        assert_eq!(p.name(), "google");
    }
}
