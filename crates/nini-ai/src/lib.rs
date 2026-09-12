//! nini-ai — LLM provider implementations and shared streaming infrastructure.
//!
//! Phase 2 scope:
//! - SSE (Server-Sent Events) parser with comprehensive chunk-handling
//! - Anthropic Messages API provider
//! - OpenAI Chat Completions provider
//! - OpenAI Responses API provider
//! - Generic OpenAI-compat preset
//! - Deterministic fixture provider (no network) for tests and demos
//!
//! Provider trait and shared message types live in `nini-core` (so the agent
//! loop can depend on them without depending on the impls).

#![doc = "nini-ai — LLM provider implementations."]

/// Library version, mirrors workspace version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// SSE parser and streaming types.
pub mod sse;

/// Anthropic Messages API provider.
pub mod anthropic;

/// OpenAI Chat Completions provider.
pub mod openai;

/// OpenAI Responses API provider.
pub mod openai_responses;

/// Generic OpenAI-compat preset (any `/v1/chat/completions` endpoint).
pub mod openai_compat;

/// Deterministic fixture provider (no network). Used for tests and demos.
pub mod fixture;
