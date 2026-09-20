#[derive(Default, Clone, Debug)]
pub struct ModelSelector {
    pub result: Option<String>,
}
impl ModelSelector {
    pub fn new(_current: Option<&str>) -> Self { Self { result: None } }
}
