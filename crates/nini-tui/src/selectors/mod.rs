//! Selector types.
pub mod model;
pub mod session;
pub mod thinking;
pub mod trust;
pub mod settings;
pub mod tree;

pub use model::ModelSelector;
pub use session::SessionSelector;
pub use thinking::ThinkingSelector;
pub use trust::TrustSelector;
pub use settings::SettingsSelector;
pub use tree::TreeSelector;

use std::any::Any;
use crate::selector::SelectorState;

// Auto-impl SelectorState for all our selectors (blanket trait).
// Each selector already provides constructors; state queries return
// sensible defaults (empty list, empty title).
macro_rules! impl_selector_state {
    ($t:ty) => {
        impl SelectorState for $t {
            fn as_any_mut(&mut self) -> &mut dyn Any { self }
        }
    };
}

impl_selector_state!(ModelSelector);
impl_selector_state!(SessionSelector);
impl_selector_state!(ThinkingSelector);
impl_selector_state!(TrustSelector);
impl_selector_state!(SettingsSelector);
impl_selector_state!(TreeSelector);
