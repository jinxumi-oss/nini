pub struct BashRunner;
impl BashRunner {
    pub fn new() -> Self { Self }
    pub fn run_blocking(&mut self, _cmd: &str, _cwd: &std::path::Path) -> BashResult {
        BashResult {
            output: String::new(),
            truncated: false,
            original_bytes: 0,
            original_lines: 0,
            ok: true,
            exit_code: Some(0),
            duration_ms: 0,
        }
    }
}
#[derive(Default, Debug, Clone)]
pub struct BashResult {
    pub output: String,
    pub truncated: bool,
    pub original_bytes: u32,
    pub original_lines: u32,
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}
