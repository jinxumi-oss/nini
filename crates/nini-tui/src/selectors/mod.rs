//! Selector types — one per screen that the runtime can open.
pub mod model;
pub mod session;
pub mod settings;
pub mod thinking;
pub mod tree;
pub mod trust;

pub use model::ModelSelector;
pub use session::SessionSelector;
pub use settings::SettingsSelector;
pub use thinking::ThinkingSelector;
pub use tree::TreeSelector;
pub use trust::TrustSelector;
