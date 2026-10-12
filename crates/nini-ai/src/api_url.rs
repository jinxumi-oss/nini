//! Wire URLs, derived from a provider's configured `base_url`.
//!
//! # Why this module exists
//!
//! v0.10 (M0): every dialect built its endpoint with an inline `format!`
//! inside `stream()`. An expression with no name and no parameters cannot be
//! called from a test, so the three URL rules below had **no oracle at all** —
//! and the one that had a test had a test that never touched it (it copied
//! the `format!` into the test body and asserted on the copy).
//!
//! Naming them is the whole fix. Nothing here needs a network, and every
//! rule is now a pure function that a test can call directly.
//!
//! # The benchmark: what Pi actually does
//!
//! Pi does not concatenate these by hand:
//!
//! | Pi location | What it does |
//! |---|---|
//! | `anthropic-messages.ts:887` | `baseURL: model.baseUrl` → the SDK appends `/v1/messages` |
//! | `openai-responses.ts:251` | `baseURL: model.baseUrl` → the SDK appends `/v1/responses` |
//! | `mistral-conversations.ts:291` | `new URL("v1/chat/completions", baseUrl)` |
//!
//! nini has no SDK, so it has to write out what the SDKs do. The part that
//! matters is that **the three rules are not the same rule**, which is why
//! this is three functions and not one:
//!
//! | dialect | SDK behaviour | when `base` already ends in `/v1` |
//! |---|---|---|
//! | OpenAI chat completions | normalises, does not append twice | `/v1` kept once |
//! | Anthropic messages | always appends `/v1/messages` | `/v1/v1/messages` |
//! | OpenAI responses | always appends `/v1/responses` | `/v1/v1/responses` |
//!
//! Pi's own model catalogue is what makes this asymmetry necessary, rather
//! than a quirk of its history. Of the `baseUrl` values in
//! `pi-ai/src/providers/*.ts`, 19 end in `/v1` (`openai.ts:10` is
//! `https://api.openai.com/v1`) and 16 do not (`anthropic.ts:47` is
//! `https://api.anthropic.com`). One rule cannot serve both.
//!
//! **The last two rows look like bugs and are not.** They are what the
//! vendor SDKs do, and matching them is what this module is for. The tests
//! below assert those exact strings, and say so, so the next reader does
//! not "fix" a deliberate alignment.
//!
//! # The one place nini was not equivalent
//!
//! `chat_completions_url` used to be
//!
//! ```text
//! format!("{base}{}/chat/completions", if base.ends_with("/v1") { "" } else { "/v1" })
//! ```
//!
//! — a hand-written stand-in for the OpenAI SDK's URL resolution. It agreed
//! with the SDK on every input except one: a trailing slash. The SDK
//! resolves the path relatively, so `https://x/v1/` is fine there; the
//! stand-in failed its own `ends_with("/v1")` test and produced
//! `https://x/v1//v1/chat/completions`.
//!
//! That is a 404 whose only diagnostic is `request failed: 404`, on a
//! configuration people write routinely — `OPENAI_BASE_URL` pointing at a
//! gateway, vLLM, or an OpenRouter proxy, with the slash they typed.
//!
//! The test that "covered" this (`handles_trailing_slash`) asserted the
//! double slash as expected behaviour and called it a known minor quirk,
//! while never calling the production expression. It could not have gone
//! red for any change to the code it claimed to cover.

/// The `chat/completions` endpoint for an OpenAI-compatible base URL.
///
/// Mirrors the OpenAI SDK: a base already ending in `/v1` is not given a
/// second one, and trailing slashes are insignificant.
pub fn chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

/// The `v1/messages` endpoint for an Anthropic base URL.
///
/// Mirrors the Anthropic SDK, which appends unconditionally. Pi's
/// `anthropic.ts:47` supplies `https://api.anthropic.com` — no `/v1` —
/// so the unconditional append is what produces the correct URL.
///
/// A base that already ends in `/v1` therefore yields `/v1/v1/messages`.
/// That is the SDK's behaviour, reproduced deliberately; see the module
/// docs.
pub fn anthropic_messages_url(base_url: &str) -> String {
    format!("{base_url}/v1/messages")
}

/// The `v1/responses` endpoint for an OpenAI base URL.
///
/// Mirrors the OpenAI SDK's responses path, which like the Anthropic one
/// appends unconditionally. As above, a base already ending in `/v1`
/// yields `/v1/v1/responses`, and that is the intended alignment.
pub fn responses_url(base_url: &str) -> String {
    format!("{base_url}/v1/responses")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- chat completions: the rule that was wrong ----

    #[test]
    fn a_base_without_a_version_gets_v1() {
        assert_eq!(
            chat_completions_url("https://api.openai.com"),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn a_base_that_already_ends_in_v1_does_not_get_a_second_one() {
        // `openai.ts:10` ships exactly this baseUrl.
        assert_eq!(
            chat_completions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        // `openrouter.ts:11` ships this one, with a path prefix.
        assert_eq!(
            chat_completions_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    /// The regression this module exists for.
    ///
    /// The previous expression produced `.../v1//v1/chat/completions` here,
    /// and the test that used to stand in for this one asserted that string
    /// as correct.
    #[test]
    fn a_trailing_slash_does_not_produce_a_double_slash() {
        assert_eq!(
            chat_completions_url("https://example.com/v1/"),
            "https://example.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://example.com/"),
            "https://example.com/v1/chat/completions"
        );
    }

    /// Several `*_BASE_URL` values people actually write end in a slash.
    /// They must all land on the same URL as their slashless twin.
    #[test]
    fn a_slash_is_insignificant_wherever_it_sits() {
        for (slashed, plain) in [
            (
                "https://gateway.internal/v1/",
                "https://gateway.internal/v1",
            ),
            ("https://gateway.internal/", "https://gateway.internal"),
            (
                "https://gateway.internal/openai/v1/",
                "https://gateway.internal/openai/v1",
            ),
        ] {
            assert_eq!(
                chat_completions_url(slashed),
                chat_completions_url(plain),
                "{slashed} and {plain} name the same endpoint"
            );
        }
    }

    /// Guards the *shape* of the rule rather than a fixed list, so a future
    /// rewrite cannot satisfy the other tests while getting `/v1` wrong.
    #[test]
    fn the_path_is_appended_exactly_once() {
        for base in [
            "https://api.openai.com",
            "https://api.openai.com/",
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/",
            "https://openrouter.ai/api/v1",
        ] {
            let url = chat_completions_url(base);
            assert!(
                url.ends_with("/chat/completions"),
                "{base} -> {url} lost the path"
            );
            assert_eq!(
                url.matches("/v1").count(),
                1,
                "{base} -> {url} does not have exactly one /v1"
            );
            assert!(
                !url.contains("//v"),
                "{base} -> {url} has a doubled separator"
            );
        }
    }

    // ---- anthropic: unchanged behaviour, now observable ----

    #[test]
    fn anthropic_appends_v1_messages_which_is_what_its_base_url_needs() {
        // `anthropic.ts:47`: https://api.anthropic.com — no /v1.
        assert_eq!(
            anthropic_messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    /// Pi's Anthropic SDK appends `/v1/messages` unconditionally, so a base
    /// that already carries `/v1` gets two. This test exists so that the
    /// behaviour is recorded as intentional rather than as an oversight
    /// somebody corrects later. If this ever changes, the change is a
    /// decision about nini's alignment, not a drive-by fix.
    #[test]
    fn anthropic_reproduces_the_sdk_unconditional_append() {
        assert_eq!(
            anthropic_messages_url("https://proxy.internal/v1"),
            "https://proxy.internal/v1/v1/messages"
        );
    }

    #[test]
    fn anthropic_accepts_a_custom_base_url() {
        assert_eq!(
            anthropic_messages_url("https://bedrock-proxy.internal"),
            "https://bedrock-proxy.internal/v1/messages"
        );
    }

    // ---- responses: unchanged behaviour, now observable ----

    #[test]
    fn responses_appends_v1_responses() {
        assert_eq!(
            responses_url("https://api.openai.com"),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn responses_reproduces_the_sdk_unconditional_append() {
        assert_eq!(
            responses_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/v1/responses"
        );
    }
}
