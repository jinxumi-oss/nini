#[derive(Default, Clone, Debug)]
pub struct TreeSelector;
impl TreeSelector {
    pub fn from_entries(_e: &[nini_session::SessionEntry]) -> Self { Self }
    pub fn summarize_at(&mut self, _idx: usize, _entries: &Vec<nini_session::SessionEntry>) -> Option<String> { None }
}
