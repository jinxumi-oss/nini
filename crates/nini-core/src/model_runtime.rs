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
    /// Tiered pricing, selected by total input tokens.
    ///
    /// 21 of Pi's 1267 models carry `cost.tiers`; none of the 112 for the
    /// providers nini reaches do today. Carrying them anyway means a future
    /// catalog entry cannot be silently under-priced by a flat rate - which
    /// is exactly the class of quiet wrongness this project has spent R2c
    /// and R2d removing.
    pub cost_tiers: Vec<ModelCostTier>,
    /// The thinking levels this model declares in `thinkingLevelMap`.
    pub thinking_levels: Vec<String>,
}

/// A pricing tier: rates that apply once total input tokens exceed
/// `input_tokens_above`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelCostTier {
    pub input_tokens_above: u64,
    pub cost: ModelCost,
}

/// Per-token pricing, Pi's `ModelCostSchema`.
///
/// **Rates are dollars per million tokens**, matching Pi
/// (`types.ts:779`: `input: number; // $/million tokens`).
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
    #[serde(default)]
    cost_tiers: Vec<RawTier>,
}

#[derive(Debug, Deserialize)]
struct RawTier {
    input_tokens_above: u64,
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    cache_read: f64,
    #[serde(default)]
    cache_write: f64,
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
                    cost_tiers: r
                        .cost_tiers
                        .into_iter()
                        .map(|t| ModelCostTier {
                            input_tokens_above: t.input_tokens_above,
                            cost: ModelCost {
                                input: t.input,
                                output: t.output,
                                cache_read: t.cache_read,
                                cache_write: t.cache_write,
                            },
                        })
                        .collect(),
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

    /// Add a model to the catalogue.
    ///
    /// Used for the models a user's `models.json` declares: they are not
    /// in the shipped catalog, but they are models this installation can
    /// talk to, and a catalogue that hides them is a catalogue that lies
    /// about what is available.
    pub fn add(&mut self, spec: ModelSpec) {
        if !self.models.iter().any(|m| m.id == spec.id) {
            self.models.push(spec);
        }
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
/// Pi reads `~/.nini/agent/models.json` and `./.pi/models.json`, and nini
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

/// The models a user's `models.json` declares that the shipped catalog
/// does not have.
///
/// The shipped catalog covers the providers nini implements natively. A
/// user who routes a model through their own gateway — a proxy, a
/// self-hosted endpoint — declares it in `models.json`, and that
/// declaration is a **fact about their setup**: the id, the context
/// window, the output ceiling. nini used to be able to see none of it,
/// because `merge_user_overrides` only ever *patched* catalog entries and
/// a model absent from the catalog was therefore invisible.
///
/// What is NOT carried over is the price: `models.json` states no rates,
/// and inventing them is exactly the defect this project removed when it
/// stopped fabricating costs for models it had no numbers for. A user
/// model therefore has a zero cost, and the status bar hides a zero-cost
/// pill — "unknown" reads as absent, not as free.
pub fn user_model_specs(cfg: &crate::settings::ModelsJson) -> Vec<ModelSpec> {
    let catalog = ModelRuntime::with_defaults();
    let mut out: Vec<ModelSpec> = Vec::new();
    for (provider, pc) in &cfg.providers {
        for m in &pc.models {
            let Some(id) = m.id.clone() else { continue };
            let full = if id.contains('/') {
                id.clone()
            } else {
                format!("{provider}/{id}")
            };
            if catalog.find_by_id(&full).is_some() {
                continue;
            }
            out.push(ModelSpec {
                id: full.clone(),
                // Pi shows the model's own name; a `models.json` entry
                // carries none, so the id is the honest label.
                name: full.clone(),
                // The provider's declared wire format. It is carried for
                // display only — nothing in nini branches on it.
                api: pc
                    .api
                    .clone()
                    .unwrap_or_else(|| "openai-completions".to_string()),
                provider: provider.clone(),
                reasoning: m.reasoning.unwrap_or(false),
                input: m.input.clone().unwrap_or_else(|| vec!["text".to_string()]),
                context_window: m.context_window.unwrap_or(0),
                max_tokens: m.max_tokens.unwrap_or(0),
                cost: ModelCost::default(),
                supports_temperature: true,
                cost_tiers: Vec::new(),
                thinking_levels: Vec::new(),
            });
        }
    }
    out
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
