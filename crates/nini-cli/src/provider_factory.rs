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
use nini_core::config_value::resolve_config_value_detailed;
use nini_core::provider::{Provider, Usage};
use nini_core::settings::{ModelsJson, ProviderConfig};

/// A provider that the user's own `models.json` declares but does not
/// configure completely.
///
/// The type exists so a caller can tell "the file the user wrote is
/// incomplete" from "the backend could not be built": the first must not
/// be papered over with a fixture (the reason the user needs is the
/// reason they are missing), the second may be.
#[derive(Debug)]
pub(crate) struct ModelsJsonProviderError {
    /// The sentence the user reads. It names the provider the user
    /// wrote and the item that is missing. The three phrases Pi uses
    /// are reproduced verbatim, because a refusal that names a
    /// different missing item than Pi's is a refusal the user cannot
    /// act on.
    pub(crate) detail: String,
    /// Why the value could not be read, when it was there and could
    /// not be resolved.
    ///
    /// Pi does not show this to the user: it resolves a credential with
    /// `resolveConfigValueOrThrow`
    /// (`resolve-config-value.js:199-217`), the failure is swallowed by
    /// the authentication layer, and the user is told
    /// `No API key found for <provider>.` (`auth-guidance.js:17-19`).
    /// nini says the same thing and keeps the reason here, where a
    /// diagnostic can have it.
    pub(crate) cause: Option<nini_core::config_value::ConfigValueError>,
}

impl std::fmt::Display for ModelsJsonProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for ModelsJsonProviderError {}

/// What nini knows about a provider it ships with.
///
/// Three columns, and the order of the columns is the order of the
/// fallback chain:
///
///   * `default_base` — the built-in's own default host, referenced by
///     the constant's **real** name (only `openai`, `anthropic` and
///     `openai_responses` call theirs `DEFAULT_BASE_URL`);
///   * `base_env` — the `*_BASE_URL` variable the built-in branch reads.
///     It is still the user's way of pointing nini at a gateway, and
///     v0.8.4 was a bugfix about exactly that;
///   * `api` — the api this provider speaks, and the credential
///     variables it may fall back to.
///
/// `openai-compat` is the one built-in with **no** default base URL: its
/// constructor takes the base URL, so there is nothing to fall back to.
struct Builtin {
    default_base: Option<&'static str>,
    base_env: Option<&'static str>,
    api: &'static str,
    key_envs: &'static [&'static str],
}

/// The providers nini ships with, and what each one defaults to.
const BUILTINS: &[(&str, Builtin)] = &[
    (
        "anthropic",
        Builtin {
            default_base: Some(nini_ai::anthropic::DEFAULT_BASE_URL),
            base_env: Some("ANTHROPIC_BASE_URL"),
            api: "anthropic-messages",
            key_envs: &["ANTHROPIC_API_KEY"],
        },
    ),
    (
        "openai",
        Builtin {
            default_base: Some(nini_ai::openai::DEFAULT_BASE_URL),
            base_env: Some("OPENAI_BASE_URL"),
            api: "openai-completions",
            key_envs: &["OPENAI_API_KEY"],
        },
    ),
    (
        "openai-responses",
        Builtin {
            default_base: Some(nini_ai::openai_responses::DEFAULT_BASE_URL),
            // The built-in branch reads no `*_BASE_URL` at all, so there
            // is no environment layer for this one.
            base_env: None,
            api: "openai-responses",
            key_envs: &["OPENAI_API_KEY"],
        },
    ),
    (
        "openai-compat",
        Builtin {
            default_base: None,
            base_env: Some("OPENAI_BASE_URL"),
            api: "openai-completions",
            key_envs: &["OPENAI_API_KEY"],
        },
    ),
    (
        "google",
        Builtin {
            default_base: Some(nini_ai::google::GEMINI_BASE_URL),
            base_env: Some("GOOGLE_BASE_URL"),
            // Pi's built-in API for this one is `google-generative-ai`.
            // nini has no such implementation: its `google` speaks the
            // OpenAI-compatible protocol (`GoogleProvider` wraps
            // `OpenAiCompatProvider`), so the API it can be built from
            // is `openai-completions`. Naming Pi's API here would make
            // every `google` entry in a user's `models.json` fail with
            // `No API provider registered for api: google-generative-ai`.
            api: "openai-completions",
            key_envs: &["GOOGLE_API_KEY", "GEMINI_API_KEY"],
        },
    ),
    (
        "deepseek",
        Builtin {
            default_base: Some(nini_ai::deepseek::DEEPSEEK_BASE_URL),
            base_env: Some("DEEPSEEK_BASE_URL"),
            api: "openai-completions",
            key_envs: &["DEEPSEEK_API_KEY"],
        },
    ),
    (
        "groq",
        Builtin {
            default_base: Some(nini_ai::groq::GROQ_BASE_URL),
            base_env: Some("GROQ_BASE_URL"),
            api: "openai-completions",
            key_envs: &["GROQ_API_KEY"],
        },
    ),
    (
        "mistral",
        Builtin {
            default_base: Some(nini_ai::mistral::MISTRAL_BASE_URL),
            base_env: Some("MISTRAL_BASE_URL"),
            api: "openai-completions",
            key_envs: &["MISTRAL_API_KEY"],
        },
    ),
    (
        "cohere",
        Builtin {
            default_base: Some(nini_ai::cohere::COHERE_BASE_URL),
            base_env: Some("COHERE_BASE_URL"),
            api: "openai-completions",
            key_envs: &["COHERE_API_KEY"],
        },
    ),
];

/// The provider configuration a `models.json` entry resolves to.
#[derive(Debug)]
struct ResolvedProvider {
    api: String,
    base_url: String,
    api_key: String,
}

/// Look up what nini knows about a provider it ships with.
fn builtin_of(id: &str) -> Option<&'static Builtin> {
    BUILTINS
        .iter()
        .find(|(name, _)| *name == id)
        .map(|(_, builtin)| builtin)
}

/// The first model a provider declares, if any.
///
/// Pi names the model in its "no api specified" sentence, because the
/// api can be set per model as well as per provider.
fn first_model_id(config: &ProviderConfig) -> Option<&str> {
    config.models.iter().find_map(|model| model.id.as_deref())
}

/// Resolve the endpoint a `models.json` entry names.
///
/// The chain, in order:
///
///   * **api**: the file's `api`, else the built-in's api, else
///     `no "api" specified`;
///   * **base URL**: the file's `baseUrl`, else the `*_BASE_URL`
///     variable the built-in branch reads, else the built-in's default
///     host, else `"baseUrl" is required`.
///
/// The base URL is taken **literally**. Pi resolves configuration
/// values only for credentials and headers (`provider-composer.js:241`
/// is the one `resolveConfigValueOrThrow` call for a provider's key);
/// a `baseUrl` is read as written (`provider-composer.js:53`). Resolving
/// it here would run a `!command` that Pi never runs, and would send a
/// half-expanded URL to the network.
///
/// The sentences are Pi's, verbatim: a user who compares the two
/// programs must read the same reason on both sides.
fn resolve_endpoint(
    id: &str,
    config: &ProviderConfig,
) -> Result<(String, String), ModelsJsonProviderError> {
    let builtin = builtin_of(id);

    let api = config
        .api
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| builtin.map(|b| b.api.to_string()))
        .ok_or_else(|| ModelsJsonProviderError {
            cause: None,
            detail: match first_model_id(config) {
                Some(model) => format!(
                    "Provider {id}, model {model}: no \"api\" specified. Set at provider or model level."
                ),
                None => format!(
                    "Provider {id}: no \"api\" specified. Set at provider or model level."
                ),
            },
        })?;

    let base_url = match config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(base) => base.to_string(),
        None => match builtin {
            Some(builtin) => {
                let from_env = builtin
                    .base_env
                    .and_then(|var| std::env::var(var).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                match from_env {
                    Some(base) => base,
                    None => match builtin.default_base {
                        Some(default) => default.to_string(),
                        None => {
                            // The one built-in without a default host:
                            // it has always said this, and it keeps
                            // saying it.
                            let var = builtin.base_env.unwrap_or("OPENAI_BASE_URL");
                            return Err(ModelsJsonProviderError {
                                cause: None,
                                detail: format!("{var} required for {id}"),
                            });
                        }
                    },
                }
            }
            None => {
                return Err(ModelsJsonProviderError {
                    cause: None,
                    detail: format!(
                        "Provider {id}: \"baseUrl\" is required when defining custom models."
                    ),
                })
            }
        },
    };

    Ok((api, base_url))
}

/// Resolve the credential a `models.json` entry names.
///
/// `--api-key` first (it is nini's own explicit contract), then the
/// file's `apiKey` — resolved the way Pi resolves it, so `$VAR` and
/// `!cmd` mean what they mean in Pi — then the environment variable the
/// built-in provider reads, and finally Pi's
/// `No API key found for`.
///
/// A credential that is **configured but unreadable** is an error, not a
/// fallback: Pi resolves it with `resolveConfigValueOrThrow`
/// (`resolve-config-value.js:199-217`) and lets the failure through. If
/// nini fell back to the built-in's environment variable instead, a
/// typo in one file would silently send the request somewhere else.
fn resolve_credential(
    id: &str,
    config: &ProviderConfig,
    api_key_override: Option<&str>,
) -> Result<String, ModelsJsonProviderError> {
    let builtin = builtin_of(id);

    // The built-in's own environment variable: the last resort before
    // `No API key found for`.
    let from_builtin_env = || {
        builtin.and_then(|b| {
            b.key_envs
                .iter()
                .find_map(|var| std::env::var(var).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    };

    match api_key_override {
        Some(key) if key.trim().is_empty() => Err(ModelsJsonProviderError {
            cause: None,
            detail: "--api-key was passed but is empty. Pass a real key.".to_string(),
        }),
        Some(key) => Ok(key.trim().to_string()),
        None => match config.api_key.as_deref() {
            Some(raw) => match resolve_config_value_detailed(raw) {
                Ok(Some(key)) => {
                    let key = key.trim().to_string();
                    if key.is_empty() {
                        from_builtin_env().ok_or_else(|| ModelsJsonProviderError {
                            cause: None,
                            detail: format!("No API key found for {id}."),
                        })
                    } else {
                        Ok(key)
                    }
                }
                Ok(None) => from_builtin_env().ok_or_else(|| ModelsJsonProviderError {
                    cause: None,
                    detail: format!("No API key found for {id}."),
                }),
                Err(cause) => {
                    // Pi: the failure of `resolveConfigValueOrThrow` is
                    // swallowed by the authentication layer, and the user
                    // is told `No API key found for <provider>.`
                    // (`auth-guidance.js:17-19`) — **not** the reason.
                    // The reason stays in `cause`, for a diagnostic.
                    //
                    // What matters, and is *not* about wording: nini does
                    // **not** fall back to the built-in's environment
                    // variable here. Pi does not either — a value that was
                    // configured and could not be read is a failure
                    // (`provider-composer.js:239-243`), not a reason to
                    // send the request somewhere the user did not choose.
                    Err(ModelsJsonProviderError {
                        cause: Some(cause),
                        detail: format!("No API key found for {id}."),
                    })
                }
            },
            None => from_builtin_env().ok_or_else(|| ModelsJsonProviderError {
                cause: None,
                detail: format!("No API key found for {id}."),
            }),
        },
    }
}

/// Resolve a provider the user's `models.json` declares.
fn resolve_models_json_provider(
    id: &str,
    config: &ProviderConfig,
    api_key_override: Option<&str>,
) -> Result<ResolvedProvider, ModelsJsonProviderError> {
    let (api, base_url) = resolve_endpoint(id, config)?;
    let api_key = resolve_credential(id, config, api_key_override)?;
    Ok(ResolvedProvider {
        api,
        base_url,
        api_key,
    })
}

/// The warnings Pi prints for a `models.json` it could not use.
///
/// Pi collects the error of every provider it could not compose and
/// prints them under one `Warning: errors loading models.json:`
/// heading — one line per provider, at most. nini does the same, and
/// for the **user-level** file: the project-level one is a catalogue,
/// not a configuration, so a provider in it is never composed and never
/// complained about.
pub(crate) fn models_json_warnings(models: &ModelsJson) -> Vec<String> {
    let mut warnings = Vec::new();
    for (id, config) in &models.providers {
        // Pi composes a provider's *models*; a provider that declares
        // none has nothing to compose and nothing to complain about.
        if config.models.is_empty() {
            continue;
        }
        if let Err(e) = resolve_endpoint(id, config) {
            warnings.push(format!("Provider \"{id}\": {}", e.detail));
        }
    }
    warnings
}

/// Build the provider a `models.json` entry describes.
///
/// The `api` decides which implementation speaks to it. nini implements
/// three of Pi's apis; the rest are reported the way Pi reports them.
fn build_models_json_provider(
    id: &str,
    config: &ProviderConfig,
    api_key_override: Option<&str>,
) -> Result<Arc<dyn Provider>> {
    let resolved = resolve_models_json_provider(id, config, api_key_override)?;
    match resolved.api.as_str() {
        "openai-completions" => Ok(Arc::new(nini_ai::openai_compat::OpenAiCompatProvider::new(
            resolved.base_url,
            resolved.api_key,
        ))),
        "anthropic-messages" => Ok(Arc::new(
            nini_ai::anthropic::AnthropicProvider::new(resolved.api_key)
                .with_base_url(resolved.base_url),
        )),
        "openai-responses" => Ok(Arc::new(
            nini_ai::openai_responses::OpenAiResponsesProvider::new(resolved.api_key)
                .with_base_url(resolved.base_url),
        )),
        other => Err(ModelsJsonProviderError {
            cause: None,
            detail: format!("No API provider registered for api: {other}"),
        }
        .into()),
    }
}

/// v0.9.0: resolve the API key for a provider.
///
/// Precedence, as `--help` promises: explicit `--api-key` **overrides**
/// the environment variable.
///
/// v0.9.0 also rejects an *empty* key. `std::env::var` returns `Ok("")`
/// for a variable that is set-but-blank, which sailed past the
/// `.context(...)` check and produced a provider that sent
/// `Authorization: Bearer ` — a silent 401 with no diagnostic at all.
/// An empty key is a configuration mistake; it deserves a sentence
/// saying so, not a round trip to the server.
/// Resolve a provider's key: `--api-key` first, then the provider's
/// environment variable.
///
/// v0.10 (R2i): five providers read `std::env::var` directly instead of
/// coming through here - anthropic, deepseek, groq, mistral, cohere - so
/// `--api-key` was silently ignored for them. `nini --api-key sk-... --provider
/// anthropic` failed with "ANTHROPIC_API_KEY required for anthropic" while
/// the user was holding the key on the command line. A flag that works for
/// some providers and is invisible for the rest is worse than one that does
/// not exist: it looks like it was honoured.
fn resolve_api_key(explicit: Option<&str>, env_var: &str, provider: &str) -> Result<String> {
    if let Some(k) = explicit {
        let trimmed = k.trim();
        if trimmed.is_empty() {
            anyhow::bail!(
                "--api-key was passed but is empty. Pass a real key, or unset it \
                 and set {env_var} instead."
            );
        }
        return Ok(trimmed.to_string());
    }
    match std::env::var(env_var) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        Ok(_) => anyhow::bail!("{env_var} is set but empty. Unset it, or pass --api-key <key>."),
        Err(_) => anyhow::bail!("{env_var} required for {provider}"),
    }
}

/// Construct an `Arc<dyn Provider>` for the named backend. Reads
/// provider-specific env vars (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
/// etc.) and respects `*_BASE_URL` for gateway compatibility.
///
/// If `fallback_keys` is non-empty AND the primary provider supports
/// key rotation (currently only `anthropic`), wrap the primary + each
/// fallback key in a `FallbackProvider`. Empty/duplicate fallback keys
/// are skipped.
///
/// v0.9.0: `api_key_override` is the `--api-key` flag. It used to be
/// dropped on the floor in `main.rs` (`api_key: _`), so the flag's help
/// text — "API key (overrides env vars)" — was simply false. Verified by
/// hand: `--api-key wrong` with a correct env var succeeded, and
/// `--api-key correct` with no env var failed with "OPENAI_API_KEY
/// required".
///
/// v0.13: `user_models` is the **user-level** `models.json` (the file in
/// the agent directory — never the project-level one). A provider it
/// declares wins over the built-in of the same name, which is what Pi
/// does: the file's `baseUrl` and `apiKey` are the ones that are used.
pub(crate) fn build_provider(
    provider: &str,
    turns: Vec<Vec<FixtureTurn>>,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
    api_key_override: Option<&str>,
    user_models: Option<&ModelsJson>,
) -> Result<Arc<dyn Provider>> {
    // v0.13: the user's own `models.json` speaks first. The eleven
    // branches below are untouched — a provider the file declares never
    // reaches them.
    if let Some(models) = user_models {
        if let Some(config) = models.providers.get(provider) {
            return build_models_json_provider(provider, config, api_key_override);
        }
    }
    match provider {
        "fixture" => Ok(Arc::new(ProgrammedProvider::from_turns(turns))),
        "anthropic" => {
            build_anthropic_with_fallbacks(api_key_override, fallback_keys, fallback_base_urls)
        }
        // v0.8.4 (bugfix): `OPENAI_BASE_URL` was previously ignored
        // — `OpenAiProvider::new(key)` hard-codes the upstream URL
        // to `https://api.openai.com`. Any user pointing the env at
        // a proxy / gateway / staging host (e.g.
        // `OPENAI_BASE_URL=http://localhost:8080`) had their
        // requests silently go to the wrong place. We previously
        // routed this through `build_with_optional_base_url`, which
        // derives `{ENV_VAR_NAME}_BASE_URL` from the API-key name —
        // but the actual env var is `OPENAI_BASE_URL`, not
        // `OPENAI_API_KEY_BASE_URL`. Inline the read instead.
        "openai" => {
            let key = resolve_api_key(api_key_override, "OPENAI_API_KEY", "openai")?;
            let provider = match std::env::var("OPENAI_BASE_URL").ok() {
                Some(base) => nini_ai::openai::OpenAiProvider::new(key).with_base_url(base),
                None => nini_ai::openai::OpenAiProvider::new(key),
            };
            Ok(Arc::new(provider))
        }
        "openai-responses" => {
            let key = resolve_api_key(api_key_override, "OPENAI_API_KEY", "openai-responses")?;
            Ok(Arc::new(
                nini_ai::openai_responses::OpenAiResponsesProvider::new(key),
            ))
        }
        "openai-compat" => {
            let key = resolve_api_key(api_key_override, "OPENAI_API_KEY", "openai-compat")?;
            let base = std::env::var("OPENAI_BASE_URL")
                .context("OPENAI_BASE_URL required for openai-compat")?;
            Ok(Arc::new(nini_ai::openai_compat::OpenAiCompatProvider::new(
                base, key,
            )))
        }
        "google" if api_key_override.is_some() => {
            let key = resolve_api_key(api_key_override, "GOOGLE_API_KEY", "google")?;
            let base = std::env::var("GOOGLE_BASE_URL").ok();
            Ok(Arc::new(match base {
                Some(b) => nini_ai::google::GoogleProvider::with_base_url(b, key),
                None => nini_ai::google::GoogleProvider::new(key),
            }))
        }
        "google" => build_with_optional_base_url(
            "GOOGLE_API_KEY (or GEMINI_API_KEY)",
            "google",
            |key| nini_ai::google::GoogleProvider::new(key),
            |base, key| nini_ai::google::GoogleProvider::with_base_url(base, key),
        ),
        // v0.8.4 (bugfix): the second positional arg of
        // `build_with_optional_base_url` is `env_var_name`, not a
        // provider label. The original `("DEEPSEEK_API_KEY", "deepseek", ...)`
        // caused `std::env::var("deepseek")` to be looked up, which
        // is never set — every deepseek invocation hit
        // `DEEPSEEK_API_KEY required` even when the env var was
        // present. Pass the actual env var name.
        // v0.8.4 (bugfix): same env-var-derive design flaw as
        // `openai` — `build_with_optional_base_url` reads
        // `{NAME}_BASE_URL` (= `DEEPSEEK_API_KEY_BASE_URL`) instead
        // of `DEEPSEEK_BASE_URL`. Inline the read.
        "deepseek" => {
            let key = resolve_api_key(api_key_override, "DEEPSEEK_API_KEY", "deepseek")?;
            let provider = match std::env::var("DEEPSEEK_BASE_URL").ok() {
                Some(base) => nini_ai::deepseek::DeepSeekProvider::with_base_url(base, key),
                None => nini_ai::deepseek::DeepSeekProvider::new(key),
            };
            Ok(Arc::new(provider))
        }
        "groq" => {
            let key = resolve_api_key(api_key_override, "GROQ_API_KEY", "groq")?;
            let provider = match std::env::var("GROQ_BASE_URL").ok() {
                Some(base) => nini_ai::groq::GroqProvider::with_base_url(base, key),
                None => nini_ai::groq::GroqProvider::new(key),
            };
            Ok(Arc::new(provider))
        }
        "mistral" => {
            let key = resolve_api_key(api_key_override, "MISTRAL_API_KEY", "mistral")?;
            let provider = match std::env::var("MISTRAL_BASE_URL").ok() {
                Some(base) => nini_ai::mistral::MistralProvider::with_base_url(base, key),
                None => nini_ai::mistral::MistralProvider::new(key),
            };
            Ok(Arc::new(provider))
        }
        "cohere" => {
            let key = resolve_api_key(api_key_override, "COHERE_API_KEY", "cohere")?;
            let provider = match std::env::var("COHERE_BASE_URL").ok() {
                Some(base) => nini_ai::cohere::CohereProvider::with_base_url(base, key),
                None => nini_ai::cohere::CohereProvider::new(key),
            };
            Ok(Arc::new(provider))
        }
        other => {
            eprintln!("nini: unknown provider: {other}");
            std::process::exit(2);
        }
    }
}

/// Anthropic-specific helper: builds the primary provider + a
/// chain of FallbackProvider instances if `fallback_keys` is non-empty.
fn build_anthropic_with_fallbacks(
    api_key_override: Option<&str>,
    fallback_keys: &[String],
    fallback_base_urls: &[String],
) -> Result<Arc<dyn Provider>> {
    let key = resolve_api_key(api_key_override, "ANTHROPIC_API_KEY", "anthropic")?;
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
        Ok(Arc::new(nini_ai::fallback::FallbackProvider::new(
            providers,
        )))
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
pub(crate) fn make_anthropic(
    key: &str,
    base: Option<&str>,
) -> nini_ai::anthropic::AnthropicProvider {
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

    // ---- v0.9.0: `--api-key` actually overrides the env var ----
    //
    // The flag used to be dropped on the floor in `main.rs`
    // (`api_key: _`), so `--help`'s "API key (overrides env vars)" was
    // false. Verified by hand before the fix:
    //
    //   --api-key wrong   + correct env var  ->  SUCCEEDED
    //   --api-key correct + no env var      ->  "OPENAI_API_KEY required"
    //
    // Both are the opposite of what the flag promises.

    #[test]
    fn explicit_api_key_wins_over_the_env_var() {
        // SAFETY: single-threaded test process, no other thread reads
        // this var concurrently.
        unsafe { std::env::set_var("NINI_TEST_KEY", "from-env") };
        let k = resolve_api_key(Some("from-flag"), "NINI_TEST_KEY", "test").unwrap();
        assert_eq!(k, "from-flag");
    }

    #[test]
    fn env_var_is_used_when_no_flag_is_given() {
        unsafe { std::env::set_var("NINI_TEST_KEY", "from-env") };
        let k = resolve_api_key(None, "NINI_TEST_KEY", "test").unwrap();
        assert_eq!(k, "from-env");
    }

    #[test]
    fn an_empty_flag_is_an_error_not_a_silent_blank_bearer_token() {
        // This is the shape that produced a silent 401 with no
        // diagnostic at all: `std::env::var` returns `Ok("")` for a
        // variable that is set-but-blank, which sailed past the old
        // `.context(...)` check and built a provider that sent
        // `Authorization: Bearer `.
        let err = resolve_api_key(Some(""), "NINI_TEST_KEY", "test").unwrap_err();
        assert!(
            err.to_string().contains("empty"),
            "error should say the key is empty, got: {err}"
        );
        let err = resolve_api_key(Some("   "), "NINI_TEST_KEY", "test").unwrap_err();
        assert!(err.to_string().contains("empty"), "whitespace is empty too");
    }

    #[test]
    fn a_blank_env_var_is_an_error() {
        unsafe { std::env::set_var("NINI_TEST_KEY", "   ") };
        let err = resolve_api_key(None, "NINI_TEST_KEY", "test").unwrap_err();
        assert!(
            err.to_string().contains("set but empty"),
            "error should distinguish blank-set from unset, got: {err}"
        );
    }

    #[test]
    fn a_missing_env_var_names_the_variable() {
        let err = resolve_api_key(None, "NINI_TEST_ABSENT_KEY", "test").unwrap_err();
        assert!(
            err.to_string().contains("NINI_TEST_ABSENT_KEY"),
            "error should name the variable, got: {err}"
        );
    }

    // ---------------------------------------------------------------
    // R2i: the flag must reach every provider, not just some of them.
    //
    // `resolve_api_key` was always correct - flag first, then the
    // provider's env var, naming the right variable on failure. Five
    // providers just did not call it: anthropic, deepseek, groq, mistral
    // and cohere read `std::env::var` directly. So
    //
    //     nini --api-key sk-... --provider anthropic
    //
    // failed with "ANTHROPIC_API_KEY required for anthropic" while the
    // user was holding the key on the command line. A flag that works for
    // some providers and is invisible for the rest is worse than one that
    // does not exist: it looks like it was honoured.
    // ---------------------------------------------------------------

    /// Every provider that reaches `resolve_api_key`, with the variable it
    /// names. Kept as data so a new provider cannot be added without a row.
    const PROVIDERS: &[(&str, &str)] = &[
        ("anthropic", "ANTHROPIC_API_KEY"),
        ("openai", "OPENAI_API_KEY"),
        ("openai-responses", "OPENAI_API_KEY"),
        ("openai-compat", "OPENAI_API_KEY"),
        ("google", "GOOGLE_API_KEY"),
        ("google", "GEMINI_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("groq", "GROQ_API_KEY"),
        ("mistral", "MISTRAL_API_KEY"),
        ("cohere", "COHERE_API_KEY"),
    ];

    /// Run `f` with every relevant environment variable unset, restoring
    /// them afterwards. The bug only shows when the variable is *absent*:
    /// with it set, the direct `env::var` read happened to work.
    ///
    /// # Safety
    /// `set_var`/`remove_var` are not thread-safe in Rust 2024. These tests
    /// live in this binary's own test module and use their own variable
    /// names, but the process still runs tests in parallel, so the call is
    /// guarded the way `std` documents.
    /// Every variable these tests touch, so a developer's shell cannot decide
    /// whether they pass.
    ///
    /// v0.10: `an_api_key_on_the_command_line_reaches_every_provider` only
    /// ever cleared the API-key variables. `openai-compat` also needs
    /// `OPENAI_BASE_URL`, so on a machine where it happened to be exported -
    /// `~/.bashrc:317-318` sets it here - the test passed, and in a clean
    /// environment or CI it failed on a provider the flag *does* reach. The
    /// bug was never in `--api-key`; the bug was a test whose verdict came
    /// from the ambient environment.
    ///
    /// Variables a developer's shell may export that change the outcome,
    /// mapped to the fixed value used while the assertion runs.
    ///
    /// The `*_BASE_URL` ones are given a value rather than removed, because
    /// `openai-compat` refuses to construct without one and the point of the
    /// test is `--api-key`, not `OPENAI_BASE_URL`. The credentials are
    /// removed, because those are what would mask the flag.
    ///
    /// `PI_CODING_AGENT_DIR` is here for a different reason: it points at a
    /// config directory, and nini must not be reading anybody's.
    const AMBIENT: &[(&str, Option<&str>)] = &[
        ("OPENAI_BASE_URL", Some("http://test.invalid/v1")),
        ("ANTHROPIC_BASE_URL", Some("http://test.invalid")),
        ("GOOGLE_BASE_URL", Some("http://test.invalid/v1")),
        ("DEEPSEEK_BASE_URL", Some("http://test.invalid/v1")),
        ("GROQ_BASE_URL", Some("http://test.invalid/v1")),
        ("MISTRAL_BASE_URL", Some("http://test.invalid/v1")),
        ("COHERE_BASE_URL", Some("http://test.invalid/v1")),
        ("GEMINI_API_KEY", None),
        ("PI_CODING_AGENT_DIR", None),
    ];

    /// Process-wide lock for the tests that mutate the environment.
    ///
    /// `with_env_unset` removes and restores process-global variables, and
    /// the test harness runs tests in parallel. Two of these tests touching
    /// `OPENAI_API_KEY` at once is a data race on the environment: one
    /// restores a value the other has already removed. That is the second
    /// reason the `--api-key` test had a verdict nobody could reproduce —
    /// the first was the unneutralised `OPENAI_BASE_URL`.
    ///
    /// Same shape as `nini_core::paths::agent_lock()`, which exists for the
    /// same reason on the agent directory.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with_env_unset<T>(names: &[&str], f: impl FnOnce() -> T) -> T {
        let _guard = env_guard();
        let mut all: Vec<&str> = names.to_vec();
        all.extend(AMBIENT.iter().map(|(n, _)| *n));
        let saved: Vec<(String, Option<String>)> = all
            .iter()
            .map(|n| (n.to_string(), std::env::var(n).ok()))
            .collect();
        for (n, _) in &saved {
            unsafe { std::env::remove_var(n) };
        }
        for (n, value) in AMBIENT {
            if let Some(v) = value {
                unsafe { std::env::set_var(n, v) };
            }
        }
        let out = f();
        for (n, v) in saved {
            match v {
                Some(v) => unsafe { std::env::set_var(&n, v) },
                None => unsafe { std::env::remove_var(&n) },
            }
        }
        out
    }

    fn build(provider: &str, key: Option<&str>) -> Result<Arc<dyn Provider>> {
        // The user's own `models.json` must not decide these tests'
        // verdicts: the agent directory is pointed at an empty sandbox
        // for the duration.
        let _guard = nini_core::paths::agent_lock();
        let models = nini_core::settings::load_user_models_json();
        build_provider(provider, Vec::new(), &[], &[], key, Some(&models))
    }

    // ---- v0.13: a provider the user's `models.json` declares ----
    //
    // The file used to be a catalogue and nothing else: the provider
    // entries were read (badly — `baseUrl` and `apiKey` were dropped by
    // serde) and never *used*. A provider the user had configured was
    // in the list and unreachable.

    /// A `models.json` entry, in Pi's spelling.
    fn models_json_entry(
        base_url: Option<&str>,
        api: Option<&str>,
        api_key: Option<&str>,
    ) -> ProviderConfig {
        let mut entry = ProviderConfig::default();
        entry.base_url = base_url.map(str::to_string);
        entry.api = api.map(str::to_string);
        entry.api_key = api_key.map(str::to_string);
        entry
    }

    #[test]
    fn a_models_json_provider_wins_over_the_built_in_one() {
        // Pi's rule: the file's `baseUrl` and `apiKey` are the ones that
        // are used, even when the provider has the same name as one nini
        // ships with.
        let entry = models_json_entry(
            Some("http://models-json.invalid/v1"),
            Some("openai-completions"),
            Some("from-the-file"),
        );
        let resolved =
            resolve_models_json_provider("openai", &entry, None).expect("the file configures it");
        assert_eq!(resolved.base_url, "http://models-json.invalid/v1");
        assert_eq!(resolved.api_key, "from-the-file");
    }

    #[test]
    fn a_models_json_provider_without_a_base_url_says_base_url_is_required() {
        let entry = models_json_entry(None, Some("openai-completions"), Some("k"));
        let err = resolve_models_json_provider("my-gw", &entry, None).unwrap_err();
        assert!(
            err.detail.contains("\"baseUrl\" is required"),
            "Pi's phrase, verbatim: {}",
            err.detail
        );
    }

    #[test]
    fn a_models_json_provider_without_an_api_says_no_api_specified() {
        let entry = models_json_entry(Some("http://x.invalid/v1"), None, Some("k"));
        let err = resolve_models_json_provider("my-gw", &entry, None).unwrap_err();
        assert!(
            err.detail.contains("no \"api\" specified"),
            "Pi's phrase, verbatim: {}",
            err.detail
        );
    }

    #[test]
    fn a_built_in_google_entry_in_the_file_can_still_be_built() {
        // Pi's built-in API for google is `google-generative-ai`. nini has
        // no such implementation — its `google` speaks the
        // OpenAI-compatible protocol — so the file's `google` entry has
        // to resolve to an API nini can build, not to Pi's name for it.
        let entry = models_json_entry(
            Some("http://models-json.invalid/v1"),
            None,
            Some("from-the-file"),
        );
        let resolved = resolve_models_json_provider("google", &entry, None)
            .expect("a google entry in the file must not break the built-in google");
        assert_eq!(
            resolved.api, "openai-completions",
            "nini's google speaks the OpenAI-compatible protocol"
        );
        assert_eq!(resolved.base_url, "http://models-json.invalid/v1");
    }

    #[test]
    fn a_base_url_is_taken_literally_and_never_run_as_a_command() {
        // Pi resolves configuration values for credentials and headers
        // only (`provider-composer.js:241`); a `baseUrl` is read as
        // written (`provider-composer.js:53`). A `!command` in a base URL
        // is a URL that starts with a bang, not a shell instruction.
        let entry = models_json_entry(
            Some("!echo should-not-run"),
            Some("openai-completions"),
            Some("k"),
        );
        let resolved = resolve_models_json_provider("my-gw", &entry, None)
            .expect("the URL is taken as written");
        assert_eq!(
            resolved.base_url, "!echo should-not-run",
            "the base URL is literal — no command is run, no value is substituted"
        );
    }

    #[test]
    fn a_credential_that_is_configured_but_unreadable_is_an_error() {
        // Pi: a credential that is **configured but unreadable** is a
        // failure (`provider-composer.js:239-243` calls
        // `resolveConfigValueOrThrow`), and the user is told
        // `No API key found for <provider>.` (`auth-guidance.js:17-19`).
        //
        // Two things are asserted here:
        //
        //   * the sentence the user reads is Pi's;
        //   * the built-in's environment variable is **not** used as a
        //     fallback — a value that was configured and could not be
        //     read must not send the request somewhere else.
        //
        // The reason is kept in `cause`: Pi swallows it, nini keeps it
        // for a diagnostic without showing it to the user.
        let entry = models_json_entry(
            Some("http://x.invalid/v1"),
            Some("openai-completions"),
            Some("$NINI_A_VARIABLE_THAT_IS_NOT_SET"),
        );
        let err = resolve_models_json_provider("my-gw", &entry, None).unwrap_err();
        assert_eq!(
            err.detail, "No API key found for my-gw.",
            "Pi's sentence, verbatim"
        );
        assert!(
            matches!(
                err.cause,
                Some(nini_core::config_value::ConfigValueError::MissingEnvVars(ref names))
                    if names == &["NINI_A_VARIABLE_THAT_IS_NOT_SET".to_string()]
            ),
            "the reason names the variable that was missing: {:?}",
            err.cause
        );
    }

    #[test]
    fn a_models_json_provider_without_a_credential_says_no_api_key_found() {
        let entry = models_json_entry(
            Some("http://x.invalid/v1"),
            Some("openai-completions"),
            None,
        );
        let err = resolve_models_json_provider("my-gw", &entry, None).unwrap_err();
        assert!(
            err.detail.contains("No API key found for"),
            "Pi's phrase, verbatim: {}",
            err.detail
        );
    }

    #[test]
    fn an_unregistered_api_is_reported_the_way_pi_reports_it() {
        let entry = models_json_entry(
            Some("http://x.invalid/v1"),
            Some("google-generative-ai"),
            Some("k"),
        );
        let err = match build_models_json_provider("my-gw", &entry, None) {
            Ok(_) => panic!("an api nini does not implement must not build a provider"),
            Err(e) => e,
        };
        assert!(
            err.to_string()
                .contains("No API provider registered for api: google-generative-ai"),
            "Pi's phrase, verbatim: {err}"
        );
    }

    #[test]
    fn the_environment_variable_still_wins_over_the_default_host() {
        // v0.8.4 fixed exactly this: `OPENAI_BASE_URL` is how a user
        // points nini at a gateway, and it must not be ignored just
        // because the entry came from a file.
        with_env_unset(&["OPENAI_BASE_URL"], || {
            let entry = models_json_entry(None, None, None);
            let resolved = resolve_models_json_provider("openai", &entry, Some("k"))
                .expect("the built-in defaults fill the gaps");
            assert_eq!(resolved.base_url, "http://test.invalid/v1");
            assert_eq!(resolved.api, "openai-completions");
        });
    }

    #[test]
    fn openai_compat_keeps_the_message_it_has_always_had() {
        // It has no default host, so the environment variable is the
        // only way in — and the sentence stays the one users have read.
        with_env_unset(&["OPENAI_BASE_URL"], || {
            // The ambient table gives every `*_BASE_URL` a value; this
            // one must be **absent** for the message to be the one the
            // user gets.
            unsafe { std::env::remove_var("OPENAI_BASE_URL") };
            let entry = models_json_entry(None, Some("openai-completions"), Some("k"));
            let err = resolve_models_json_provider("openai-compat", &entry, None).unwrap_err();
            assert_eq!(err.detail, "OPENAI_BASE_URL required for openai-compat");
        });
    }

    #[test]
    fn the_api_key_on_the_command_line_wins_over_the_file() {
        let entry = models_json_entry(
            Some("http://x.invalid/v1"),
            Some("openai-completions"),
            Some("from-the-file"),
        );
        let resolved =
            resolve_models_json_provider("my-gw", &entry, Some("from-the-flag")).unwrap();
        assert_eq!(resolved.api_key, "from-the-flag");
    }

    #[test]
    fn a_credential_in_the_file_is_resolved_like_pi_resolves_it() {
        // SAFETY: the variable name is this test's own.
        unsafe { std::env::set_var("NINI_TEST_MODELS_JSON_KEY", "from-the-environment") };
        let entry = models_json_entry(
            Some("http://x.invalid/v1"),
            Some("openai-completions"),
            Some("$NINI_TEST_MODELS_JSON_KEY"),
        );
        let resolved = resolve_models_json_provider("my-gw", &entry, None).unwrap();
        assert_eq!(resolved.api_key, "from-the-environment");
    }

    #[test]
    fn a_provider_the_file_does_not_declare_keeps_its_built_in_behaviour() {
        // The eleven branches are untouched: a name the file does not
        // mention still goes to them.
        //
        // `openai` and not `openai-compat`: the latter has **no default
        // base URL** (it needs `OPENAI_BASE_URL`), so whether it builds
        // depends on the environment the test happens to run in — a test
        // whose verdict depends on someone else's shell is not a test.
        let _guard = nini_core::paths::agent_lock();
        let models = nini_core::settings::load_user_models_json();
        let provider = build_provider("openai", Vec::new(), &[], &[], Some("k"), Some(&models));
        assert!(provider.is_ok(), "the built-in branch still builds it");
    }

    #[test]
    fn an_api_key_on_the_command_line_reaches_every_provider() {
        let envs: Vec<&str> = PROVIDERS.iter().map(|(_, e)| *e).collect();
        with_env_unset(&envs, || {
            for (provider, env) in PROVIDERS {
                assert!(
                    build(provider, Some("sk-test-key")).is_ok(),
                    "`--api-key` was ignored for `{provider}`. With no {env} \
                     set the flag is the only source, so a failure here is \
                     the bug this test exists for."
                );
            }
        });
    }

    #[test]
    fn without_a_flag_it_still_names_the_right_variable() {
        let envs: Vec<&str> = PROVIDERS.iter().map(|(_, e)| *e).collect();
        with_env_unset(&envs, || {
            for (provider, env) in PROVIDERS {
                let err = build(provider, None)
                    .err()
                    .unwrap_or_else(|| panic!("{provider} built with no credential at all"));
                assert!(
                    err.to_string().contains(env),
                    "{provider} should name {env} in its error, got: {err}"
                );
            }
        });
    }

    #[test]
    fn an_empty_flag_is_refused_rather_than_ignored() {
        let envs: Vec<&str> = PROVIDERS.iter().map(|(_, e)| *e).collect();
        with_env_unset(&envs, || {
            let err = match build("anthropic", Some("   ")) {
                Ok(_) => panic!("an empty --api-key must not be silently treated as unset"),
                Err(e) => e,
            };
            assert!(
                err.to_string().contains("empty"),
                "and the error should say so: {err}"
            );
        });
    }
}
