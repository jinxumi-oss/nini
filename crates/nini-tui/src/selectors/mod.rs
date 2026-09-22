//! Selector types — one per screen that the runtime can open.
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