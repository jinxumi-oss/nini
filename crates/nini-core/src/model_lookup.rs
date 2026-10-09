//! Looking a model's facts up in the shipped catalog.

use crate::model_runtime::{resolve, ModelRuntime};

/// `contextWindow` for a `provider/model` string, if the catalog has it.
///
/// A bare id is accepted too, matched against any provider — the catalog is
/// small enough that the ambiguity does not matter for a window size, and
/// refusing to guess here would put the settings fallback in play for a
/// model whose real value we already hold.
pub fn context_window_for(model: &str) -> Option<u32> {
    if model.is_empty() {
        return None;
    }
    if model.contains('/') {
        if let Some((provider, id)) = model.split_once('/') {
            if let Some(m) = resolve(provider, id) {
                return Some(m.context_window);
            }
        }
    }
    ModelRuntime::with_defaults()
        .all()
        .into_iter()
        .find(|m| m.id.ends_with(&format!("/{model}")))
        .map(|m| m.context_window)
}
