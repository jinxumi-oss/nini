#[derive(Default, Clone, Debug)]
pub struct ThinkingSelector {
    pub result: Option<String>,
}
impl ThinkingSelector {
    pub fn new(_current: Option<&str>) -> Self { Self { result: None } }
}
