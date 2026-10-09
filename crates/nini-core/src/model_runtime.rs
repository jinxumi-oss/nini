//! The model catalog.
//!
//! v0.10 (R2d): this was a stub that listed ten hand-typed model ids and
//! gave every one of them `context_window: 128_000`. Three entries
//! (`cohere/command-r-plus`, `deepseek/deepseek-chat`,
//! `groq/llama-3.3-70b`) do not exist in Pi at all, `from_models_json`
//! ignored its argument and returned the stub, and `resolve` returned
//! `None` unconditionally — so a `--models` flag and every provider-scoped
//! lookup silently did nothing.
//!
//! The catalog is now generated from Pi's own shipped data
//! (`pi-ai/dist/providers/data/*.json`), field for field, and committed
//! rather than read from a Pi installation at runtime: nini must work
//! without Pi present, and a data file that changes under a dependency
//! bump is worse than one that changes in a commit.

use serde::Deserialize;

/// One model, as Pi describes it.
///
/// Field names follow Pi's `ModelMetadataSchema`
/// (`pi-protocol/src/schemas.ts`) so a diff against Pi's data is a diff of
/// the same names. The one exception is `cost`, where Pi's camelCase
/// sub-keys are stored snake_case because the Rust side never round-trips
/// this document.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpec {
    /// `provider/model-id`, the key everything is looked up by.
    pub id: String,
    /// Pi's `name`: the display label, e.g. "Claude Opus 4.7".
    pub name: String,
    /// Which Pi API family speaks to it.
    pub api: String,
    pub provider: String,
    /// Whether the model supports extended thinking.
    pub reasoning: bool,
    /// Input modalities: "text" and/or "image".
    pub input: Vec<String>,
    pub context_window: u32,
    pub max_tokens: u32,
    pub cost: ModelCost,
    /// Pi's `compat.supportsTemperature`, defaulting to true. Declared
    /// false for `claude-opus-4-7`, `claude-opus-4-8` and `claude-opus-5`.
    pub supports_temperature: bool,
    /// The thinking levels this model declares in `thinkingLevelMap`.
    pub thinking_levels: Vec<String>,
}

/// Per-token pricing, Pi's `ModelCostSchema`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModelCost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Debug, Deserialize)]
struct RawSpec {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    api: String,
    provider: String,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    input: Vec<String>,
    context_window: u32,
    max_tokens: u32,
    #[serde(default)]
    cost: RawCost,
    #[serde(default = "yes")]
    supports_temperature: bool,
    #[serde(default)]
    thinking_levels: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawCost {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
}

fn yes() -> bool {
    true
}

/// The catalog as shipped, generated from Pi's data files.
pub const CATALOG_JSON: &str = include_str!("../data/model-catalog.json");

pub struct ModelRuntime {
    models: Vec<ModelSpec>,
}

impl ModelRuntime {
    pub fn new() -> Self {
        Self { models: Vec::new() }
    }

    /// The catalog as shipped.
    pub fn with_defaults() -> Self {
        Self::from_models_json(CATALOG_JSON)
    }

    /// Parse a catalog.
    ///
    /// The previous signature took any `T` and returned `with_defaults()`,
    /// so a `--models` flag looked like it worked and did nothing. This one
    /// falls over on input it cannot use, which is the point.
    pub fn from_models_json(json: &str) -> Self {
        let raw: Vec<RawSpec> =
            serde_json::from_str(json).expect("model catalog must be a JSON array of models");
        Self {
            models: raw
                .into_iter()
                .map(|r| ModelSpec {
                    id: r.id.clone(),
                    name: r.name,
                    api: r.api,
                    provider: r.provider,
                    reasoning: r.reasoning,
                    input: r.input,
                    context_window: r.context_window,
                    max_tokens: r.max_tokens,
                    cost: ModelCost {
                        input: r.cost.input,
                        output: r.cost.output,
                        cache_read: r.cost.cache_read,
                        cache_write: r.cost.cache_write,
                    },
                    supports_temperature: r.supports_temperature,
                    thinking_levels: r.thinking_levels,
                })
                .collect(),
        }
    }

    pub fn list(&self) -> Vec<ModelSpec> {
        self.models.clone()
    }

    pub fn all(&self) -> Vec<ModelSpec> {
        self.models.clone()
    }

    pub fn find_by_id(&self, id: &str) -> Option<ModelSpec> {
        self.models.iter().find(|m| m.id == id).cloned()
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

impl Default for ModelRuntime {
    fn default() -> Self {
        Self::with_defaults()
    }
}

pub fn available_models() -> Vec<ModelSpec> {
    ModelRuntime::with_defaults().list()
}

/// Look a model up by provider and model id.
///
/// Used to return `None` unconditionally, so every provider-scoped lookup
/// in the crate got nothing and no test noticed.
pub fn resolve(provider: &str, model: &str) -> Option<ModelSpec> {
    ModelRuntime::with_defaults().find_by_id(&format!("{provider}/{model}"))
}

/// Layer a user `models.json` over the shipped catalog.
///
/// Pi reads `~/.pi/agent/models.json` and `./.pi/models.json`, and nini
/// already looks in both places (`settings::load_models_json`). That file
/// is *configuration* — which providers to use, their base URLs and keys —
/// and its per-provider `models` list carries at most an id and a context
/// window. So it is an override, not a replacement: entries it names take
/// their `context_window` from it and keep everything else from the catalog.
///
/// It used to be passed to `from_models_json`, which ignored its argument,
/// so a user's `models.json` had no effect on `--list-models` at all.
pub fn merge_user_overrides(runtime: &mut ModelRuntime, cfg: &crate::settings::ModelsJson) {
    for (provider, pc) in &cfg.providers {
        for m in &pc.models {
            let Some(id) = m.id.clone() else { continue };
            let full = if id.contains('/') {
                id.clone()
            } else {
                format!("{provider}/{id}")
            };
            let Some(window) = m.context_window else {
                continue;
            };
            if let Some(spec) = runtime.models.iter_mut().find(|s| s.id == full) {
                spec.context_window = window;
            }
        }
    }
}

pub fn resolve_pattern(runtime: &ModelRuntime, pattern: &str) -> Vec<ModelSpec> {
    if pattern.is_empty() {
        return runtime.all();
    }
    // Support simple glob: '*' -> wildcard, otherwise substring.
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').filter(|s| !s.is_empty()).collect();
        runtime
            .all()
            .into_iter()
            .filter(|m| {
                parts.iter().all(|p| m.id.contains(p))
                    || m.id.starts_with(parts.first().copied().unwrap_or(""))
            })
            .collect()
    } else {
        runtime
            .all()
            .into_iter()
            .filter(|m| m.id.contains(pattern))
            .collect()
    }
}
