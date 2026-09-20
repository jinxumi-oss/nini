#[derive(Default, Clone, Debug)]
pub struct SessionSelector {
    pub result: Option<String>,
}
impl SessionSelector {
    pub fn from_dir<D>(_dir: D) -> Self { Self { result: None } }
}
