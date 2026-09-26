//! v0.8.2: Provider factory — single entry point to construct an
//! `Arc<dyn Provider>` for any of the 11 supported providers, plus
//! the FallbackProvider wrapper for multi-key rotation.
//!
//! Pure factory: takes a name + turns + fallbacks, returns a provider.
//! No side effects beyond reading env vars (which is the convention
//! for provider config).
//!
//! Extracted from `main.rs` so each provider branch can be unit-tested
//! and so adding a new provider is a 1-file change instead of editing
//! the god-function.

use std::sync::Arc;

use anyhow::{Context, Result};

use nini_ai::fixture::{FixtureTurn, ProgrammedProvider};
use nini_core::provider::{Provider, Usage};

/// Construct an `Arc<dyn Provider>` for the named backend. Reads
/// provider-specific env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
/// etc.) and respects `*_BASE_URL` for gateway compatibility.
///
/// If `fallback_keys` is non-empty AND the primary provider supports
/// key rotation (currently only `anthropic`), wrap the primary + each
/// fallback key in a `FallbackProvider`. Empty/duplicate fallback keys
/// are skipped.
pub(crate) fn build_provider(
    provider: &str,
    turns: Vec<Vec<FixtureTurn>>,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
) -> Result<Arc<dyn Provider>> {
    match provider {
        "fixture" => Ok(Arc::new(ProgrammedProvider::from_turns(turns))),
        "anthropic" => build_anthropic_with_fallbacks(fallback_keys, fallback_base_urls),
        "openai" => {
            let key =
                std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY required for openai")?;
            Ok(Arc::new(nini_ai::openai::OpenAiProvider::new(key)))
        }
        "openai-responses" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai-responses")?;
            Ok(Arc::new(
                nini_ai::openai_responses::OpenAiResponsesProvider::new(key),
            ))
        }
        "openai-compat" => {
            let key = std::env::var("OPENAI_API_KEY")
                .context("OPENAI_API_KEY required for openai-compat")?;
            let base = std::env::var("OPENAI_BASE_URL")
                .context("OPENAI_BASE_URL required for openai-compat")?;
            Ok(Arc::new(
                nini_ai::openai_compat::OpenAiCompatProvider::new(base, key),
            ))
        }
        "google" => build_with_optional_base_url(
            "GOOGLE_API_KEY (or GEMINI_API_KEY)",
            "google",
            |key| nini_ai::google::GoogleProvider::new(key),
            |base, key| nini_ai::google::GoogleProvider::with_base_url(base, key),
        ),
        "deepseek" => build_with_optional_base_url(
            "DEEPSEEK_API_KEY",
            "deepseek",
            |key| nini_ai::deepseek::DeepSeekProvider::new(key),
            |base, key| nini_ai::deepseek::DeepSeekProvider::with_base_url(base, key),
        ),
        "groq" => build_with_optional_base_url(
            "GROQ_API_KEY",
            "groq",
            |key| nini_ai::groq::GroqProvider::new(key),
            |base, key| nini_ai::groq::GroqProvider::with_base_url(base, key),
        ),
        "mistral" => build_with_optional_base_url(
            "MISTRAL_API_KEY",
            "mistral",
            |key| nini_ai::mistral::MistralProvider::new(key),
            |base, key| nini_ai::mistral::MistralProvider::with_base_url(base, key),
        ),
        "cohere" => build_with_optional_base_url(
            "COHERE_API_KEY",
            "cohere",
            |key| nini_ai::cohere::CohereProvider::new(key),
            |base, key| nini_ai::cohere::CohereProvider::with_base_url(base, key),
        ),
        other => {
            eprintln!("nini: unknown provider: {other}");
            std::process::exit(2);
        }
    }
}

/// Anthropic-specific helper: builds the primary provider + a
/// chain of FallbackProvider instances if `fallback_keys` is non-empty.
fn build_anthropic_with_fallbacks(
    fallback_keys: &[String],
    fallback_base_urls: &[String],
) -> Result<Arc<dyn Provider>> {
    let key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY required for anthropic")?;
    let base = std::env::var("ANTHROPIC_BASE_URL").ok();

    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
    providers.push(Arc::new(make_anthropic(&key, base.as_deref())));

    for (i, fk) in fallback_keys.iter().enumerate() {
        let trimmed = fk.trim();
        if trimmed.is_empty() || trimmed == key {
            continue;
        }
        let fbase = fallback_base_urls
            .get(i)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .or(base.as_deref());
        providers.push(Arc::new(make_anthropic(trimmed, fbase)));
    }
    if providers.len() == 1 {
        Ok(providers.remove(0))
    } else {
        Ok(Arc::new(nini_ai::fallback::FallbackProvider::new(providers)))
    }
}

/// Generic helper: build a provider with `API_KEY` env var +
/// optional `BASE_URL` override. Avoids 6 nearly-identical branches
/// in `build_provider`.
fn build_with_optional_base_url<F, G, P>(
    env_var_desc: &str,
    env_var_name: &str,
    no_base: F,
    with_base: G,
) -> Result<Arc<dyn Provider>>
where
    F: FnOnce(String) -> P,
    G: FnOnce(String, String) -> P,
    P: Provider + 'static,
{
    // Special case: google auto-detects from GOOGLE_API_KEY OR GEMINI_API_KEY.
    let key = if env_var_name == "google" {
        std::env::var("GOOGLE_API_KEY").or_else(|_| std::env::var("GEMINI_API_KEY"))
    } else {
        std::env::var(env_var_name)
    }
    .context(format!("{env_var_desc} required"))?;

    let base_env = format!("{}_BASE_URL", env_var_name.to_uppercase());
    let base = std::env::var(&base_env).ok();
    Ok(match base {
        Some(b) => Arc::new(with_base(b, key)),
        None => Arc::new(no_base(key)),
    })
}

/// Construct an `AnthropicProvider` with optional base URL override.
pub(crate) fn make_anthropic(key: &str, base: Option<&str>) -> nini_ai::anthropic::AnthropicProvider {
    let mut p = nini_ai::anthropic::AnthropicProvider::new(key);
    if let Some(b) = base {
        if !b.trim().is_empty() {
            p = p.with_base_url(b);
        }
    }
    p
}

/// Parse scripted turns from `NINI_TUI_FIXTURE_TURNS`.
///
/// Format: a JSON array of arrays of objects. Each inner array is one
/// agent turn's events. Each object has a `kind` field:
///   - `{"kind":"text","text":"..."}`                 → FixtureTurn::Text
///   - `{"kind":"tool","name":"bash","args":{...}}`   → FixtureTurn::ToolCall
///   - `{"kind":"stop","stop_reason":"end_turn"}`     → FixtureTurn::Stop
/// Any other shape fails the parse and the env var is ignored.
pub(crate) fn parse_scripted_turns(raw: &str) -> Option<Vec<Vec<FixtureTurn>>> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let arr = parsed.as_array()?;
    let mut out: Vec<Vec<FixtureTurn>> = Vec::with_capacity(arr.len());
    for turn_value in arr {
        let turn_arr = turn_value.as_array()?;
        let mut turn: Vec<FixtureTurn> = Vec::with_capacity(turn_arr.len());
        for item in turn_arr {
            let obj = item.as_object()?;
            let kind = obj.get("kind")?.as_str()?;
            match kind {
                "text" => turn.push(FixtureTurn::Text(obj.get("text")?.as_str()?.to_string())),
                "tool" => {
                    let name = obj.get("name")?.as_str()?.to_string();
                    let args = obj.get("args").cloned().unwrap_or(serde_json::Value::Null);
                    turn.push(FixtureTurn::ToolCall { name, args });
                }
                "stop" => {
                    let stop_reason = obj
                        .get("stop_reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("end_turn")
                        .to_string();
                    turn.push(FixtureTurn::Stop {
                        stop_reason,
                        usage: Usage::default(),
                    });
                }
                _ => return None,
            }
        }
        out.push(turn);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_scripted_turns_empty_input() {
        assert!(parse_scripted_turns("").is_none());
        assert!(parse_scripted_turns("not json").is_none());
    }

    #[test]
    fn parse_scripted_turns_text_event() {
        let raw = r#"[ [{"kind":"text","text":"hi"}] ]"#;
        let parsed = parse_scripted_turns(raw).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].len(), 1);
        match &parsed[0][0] {
            FixtureTurn::Text(t) => assert_eq!(t, "hi"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn parse_scripted_turns_tool_call() {
        let raw = r#"[ [{"kind":"tool","name":"bash","args":{"command":"ls"}}] ]"#;
        let parsed = parse_scripted_turns(raw).unwrap();
        match &parsed[0][0] {
            FixtureTurn::ToolCall { name, args } => {
                assert_eq!(name, "bash");
                assert_eq!(args["command"], "ls");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn parse_scripted_turns_stop_event() {
        let raw = r#"[ [{"kind":"stop","stop_reason":"end_turn"}] ]"#;
        let parsed = parse_scripted_turns(raw).unwrap();
        match &parsed[0][0] {
            FixtureTurn::Stop { stop_reason, .. } => {
                assert_eq!(stop_reason, "end_turn");
            }
            _ => panic!("expected Stop"),
        }
    }

    #[test]
    fn parse_scripted_turns_mixed_turn() {
        let raw = r#"[
            [
                {"kind":"text","text":"first"},
                {"kind":"tool","name":"bash","args":{"command":"x"}},
                {"kind":"stop","stop_reason":"end_turn"}
            ]
        ]"#;
        let parsed = parse_scripted_turns(raw).unwrap();
        assert_eq!(parsed[0].len(), 3);
    }

    #[test]
    fn parse_scripted_turns_unknown_kind_returns_none() {
        let raw = r#"[{"kind":"weird","data":"x"}]"#;
        assert!(parse_scripted_turns(raw).is_none());
    }
}