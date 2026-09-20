//! Model-runtime stub: tracks known models so the CLI can list them.

#[derive(Clone)]
pub struct ModelSpec {
    pub id: String,
    pub provider: String,
    pub context_window: u32,
}

pub struct ModelRuntime {
    models: Vec<ModelSpec>,
}

const BUILTIN_CATALOG: &[(&str, &str)] = &[
    ("anthropic/claude-opus-4-7", "Anthropic Claude Opus 4.7"),
    ("anthropic/claude-sonnet-4-5", "Anthropic Claude Sonnet 4.5"),
    ("anthropic/claude-haiku-4-5", "Anthropic Claude Haiku 4.5"),
    ("openai/gpt-5", "OpenAI GPT-5"),
    ("openai/gpt-4o", "OpenAI GPT-4o"),
    ("google/gemini-2.5-pro", "Google Gemini 2.5 Pro"),
    ("mistral/mistral-large-latest", "Mistral Large"),
    ("cohere/command-r-plus", "Cohere Command R+"),
    ("deepseek/deepseek-chat", "DeepSeek Chat"),
    ("groq/llama-3.3-70b", "Groq Llama 3.3 70B"),
];

impl ModelRuntime {
    pub fn new() -> Self { Self { models: Vec::new() } }
    pub fn with_defaults() -> Self {
        let models = BUILTIN_CATALOG.iter().map(|(id, _)| {
            let (provider, model) = id.split_once('/').unwrap_or(("unknown", id));
            ModelSpec {
                id: id.to_string(),
                provider: provider.to_string(),
                context_window: 128_000,
            }
            ._set_model_id(model)
        }).collect();
        Self { models }
    }
    pub fn from_models_json<T>(_json: T) -> Self { Self::with_defaults() }
    pub fn from_models_value(_json: &serde_json::Value) -> Self { Self::with_defaults() }
    pub fn list(&self) -> Vec<ModelSpec> { self.models.clone() }
    pub fn all(&self) -> Vec<ModelSpec> { self.models.clone() }
}

impl Default for ModelRuntime {
    fn default() -> Self { Self::with_defaults() }
}

impl ModelSpec {
    fn _set_model_id(mut self, _id: &str) -> Self { self }
}

pub fn available_models() -> Vec<ModelSpec> {
    ModelRuntime::with_defaults().list()
}

pub fn resolve(_provider: &str, _model: &str) -> Option<ModelSpec> { None }

pub fn resolve_pattern(runtime: &ModelRuntime, pattern: &str) -> Vec<ModelSpec> {
    if pattern.is_empty() {
        return runtime.all();
    }
    // Support simple glob: '*' → wildcard, otherwise substring.
    if pattern.contains('*') {
        let parts: Vec<&str> = pattern.split('*').filter(|s| !s.is_empty()).collect();
        runtime.all().into_iter().filter(|m| {
            parts.iter().all(|p| m.id.contains(p)) || m.id.starts_with(parts.first().copied().unwrap_or(""))
        }).collect()
    } else {
        runtime.all().into_iter().filter(|m| m.id.contains(pattern)).collect()
    }
}
