//! Placeholder FallbackProvider stub.

use std::pin::Pin;
use std::sync::Arc;

use async_stream::stream;
use futures_core::Stream;
use nini_core::provider::{Capabilities, Provider, ProviderError, Request, StreamEvent};

pub struct FallbackProvider {
    pub _providers: Vec<Arc<dyn Provider>>,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { _providers: providers }
    }
}

impl Provider for FallbackProvider {
    fn name(&self) -> &'static str { "fallback" }

    fn capabilities(&self) -> Capabilities { Capabilities::default() }

    fn stream(
        &self,
        _req: Request,
    ) -> Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send + 'static>> {
        Box::pin(stream! {
            yield Err(ProviderError::NotImplemented("fallback provider stub".into()));
        })
    }
}
