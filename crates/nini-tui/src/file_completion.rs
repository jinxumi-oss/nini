//! File completion stub.
pub fn complete(_partial: &str) -> Vec<String> { vec![] }

pub fn extract_at_prefix(_line: &str, _cursor: usize) -> Option<String> { None }
pub fn extract_at_prefix_pair(_line: &str, _cursor: usize) -> Option<(usize, String)> { None }
pub fn search_files<P: std::fmt::Display>(_dir: P, _prefix: &str) -> Vec<String> { vec![] }
pub fn search_files_as_struct<P: AsRef<std::path::Path>>(_dir: P, _prefix: &str) -> Vec<FileEntry> { vec![] }
pub struct FileEntry {
    pub relative_path: String,
    pub is_dir: bool,
}
impl std::fmt::Display for FileEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.relative_path)
    }
}
pub fn search_files_typed<P: AsRef<std::path::Path>>(_dir: P, _prefix: &str) -> Vec<FileEntry> { vec![] }
