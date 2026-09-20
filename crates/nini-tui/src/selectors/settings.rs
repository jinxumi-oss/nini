#[derive(Default, Clone, Debug)]
pub struct SettingsSelector {
    pub result: Option<String>,
    pub settings: crate::settings::Settings,
}
impl SettingsSelector {
    pub fn new(_settings: crate::settings::SettingsManager) -> Self {
        Self { result: None, settings: crate::settings::Settings::default() }
    }
    pub fn apply(&mut self, _idx: usize) -> Option<Option<String>> { Some(None) }
}
