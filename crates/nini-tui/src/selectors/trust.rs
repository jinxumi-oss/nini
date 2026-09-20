use nini_core::project_trust::TrustDecision;
#[derive(Default, Clone, Debug)]
pub struct TrustSelector {
    pub result: Option<TrustDecision>,
    pub cwd: String,
}
impl TrustSelector {
    pub fn new(_cwd: String, _current: Option<TrustDecision>) -> Self {
        Self { result: None, cwd: String::new() }
    }
}
