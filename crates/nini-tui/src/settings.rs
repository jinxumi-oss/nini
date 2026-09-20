//! Placeholder SettingsManager stub.

use std::path::{Path, PathBuf};

#[derive(Default, Debug)]
pub struct SettingsManager {
    pub theme: String,
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
    pub settings: Settings,
}

impl SettingsManager {
    pub fn theme_name(&mut self) -> Option<String> {
        Some(self.theme.clone())
    }
    pub fn theme(&mut self) -> crate::theme::Theme {
        crate::theme::Theme::default()
    }
    pub fn get(&self) -> Settings {
        self.settings.clone()
    }
    pub fn set_default_model<T: std::fmt::Display>(&mut self, _model: T) {}
    pub fn set_default_thinking_level<T: std::fmt::Display>(&mut self, _level: T) {}
    pub fn flush(&mut self) -> Result<(), String> { Ok(()) }
    pub fn settings_path(&self) -> PathBuf {
        PathBuf::from(".pi/agent/settings.json")
    }
    pub fn set_model_thinking_level(&mut self, _model: &str, _level: &str) {}
    pub fn load_from_disk(_path: PathBuf) -> Self { Self::default() }
}

#[derive(Default, Debug, Clone)]
pub struct Settings {
    pub default_model: Option<String>,
    pub default_thinking_level: Option<String>,
}

impl Settings {
    pub fn model_name(&self) -> Option<String> { self.default_model.clone() }
}

impl Settings {
    pub fn model_name_or_default(opt: &Option<Settings>) -> Option<String> {
        opt.as_ref().and_then(|s| s.model_name())
    }
}

impl std::ops::Deref for SettingsManager {
    type Target = Settings;
    fn deref(&self) -> &Settings { &self.settings }
}
