//! Shared visual components used by SelectList and friends.
//!
//! Pi has a full Component system (`@earendil-works/pi-tui`). For nini v1.0
//! we keep this minimal — SelectList shared by autocomplete, command
//! palette, and 6 selectors; SettingsList for displaying current settings.
//! No full Component trait; render.rs still does the layout.

pub mod select_list;
pub mod settings_list;

pub use select_list::{SelectItem, SelectList, DEFAULT_PRIMARY_COLUMN_WIDTH};
pub use settings_list::{SettingItem, SettingsList};