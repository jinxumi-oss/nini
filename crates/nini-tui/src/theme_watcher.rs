//! Theme watcher stub.
use std::path::Path;

pub enum ThemeEvent {
    Changed(std::path::PathBuf),
    Removed(std::path::PathBuf),
    Error(std::path::PathBuf, String),
}

pub fn spawn_theme_watcher(
    cwd: Option<&Path>,
    _tx: tokio::sync::mpsc::UnboundedSender<ThemeEvent>,
) -> Option<std::path::PathBuf> {
    cwd.map(|p| p.to_path_buf())
}
