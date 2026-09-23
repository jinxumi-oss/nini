//! File-path completion behind an `@` prefix in the prompt.
//!
//! v0.5 left this as a 25-line stub. v0.6 wires it to `walkdir` so typing
//! `@Cargo` shows `Cargo.toml` / `Cargo.lock` in the popup.
//!
//! Behavior:
//! * `extract_at_prefix_pair` finds the byte offset of the `@` token
//!   preceding the cursor and returns the partial path the user is typing.
//! * `search_files_as_struct` walks the cwd up to `MAX_DEPTH` levels,
//!   filtering by prefix and skipping noisy directories (`.git`, `target`,
//!   `node_modules`, etc.). Results are cached for `CACHE_TTL_MS` to avoid
//!   rescanning on every keystroke.
//! * `search_files` and `search_files_typed` are kept as string-returning
//!   convenience wrappers for tests and `tui::info`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

/// Maximum directory depth (anchored on cwd). Bounded so that walking
/// `/` on a large repo doesn't take seconds.
pub const MAX_DEPTH: usize = 4;

/// Hard cap on the number of completion candidates. Beyond this we
/// surface a "(refine to narrow)" hint in the UI rather than listing more.
pub const MAX_RESULTS: usize = 50;

/// Cache lifetime. Keys are (cwd canonical path, prefix lowercase).
pub const CACHE_TTL_MS: u128 = 5_000;

/// Directories we never recurse into. Walks in monorepos with `target/` or
/// `node_modules/` are the worst offenders.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    "dist",
    "build",
    ".next",
    ".nuxt",
];

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub relative_path: String,
    pub is_dir: bool,
}

impl std::fmt::Display for FileEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.relative_path)
    }
}

#[derive(Default)]
struct CacheKey {
    cwd: PathBuf,
    prefix: String,
}

#[derive(Default)]
struct CacheValue {
    results: Vec<FileEntry>,
    inserted: Option<Instant>,
}

#[derive(Default, Clone)]
pub struct CompletionCache {
    inner: Arc<Mutex<Option<(CacheKey, CacheValue)>>>,
}

impl CompletionCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, key: &CacheKey) -> Option<Vec<FileEntry>> {
        let guard = self.inner.lock().ok()?;
        let (k, v) = guard.as_ref()?;
        if k.cwd != key.cwd || k.prefix != key.prefix {
            return None;
        }
        let inserted = v.inserted?;
        if inserted.elapsed().as_millis() > CACHE_TTL_MS {
            return None;
        }
        Some(v.results.clone())
    }

    fn put(&self, key: CacheKey, results: Vec<FileEntry>) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some((
                key,
                CacheValue {
                    results,
                    inserted: Some(Instant::now()),
                },
            ));
        }
    }
}

/// Find the `@` token preceding `cursor` in `line` and return its byte
/// position plus the partial path that follows it.
///
/// Examples:
///   line = "fix @Car", cursor = 8  → Some((4, "Car"))
///   line = "see @src/foo.rs", cursor = 14 → Some((4, "src/foo.rs"))
///   line = "no at-sign", cursor = 4 → None
///
/// The returned `at_byte` is the index of the `@` itself in `line`.
pub fn extract_at_prefix_pair(line: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(line.len());
    // Search backwards from cursor-1 (cursor may equal line.len()).
    let bytes = line.as_bytes();
    let mut i = cursor;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'@' => {
                // The character before `@` must be whitespace (or be at
                // position 0) so we don't treat e.g. an email address as
                // a file path. Also reject `@` immediately after `/` to
                // avoid interfering with slash commands.
                if i > 0 && !is_path_separator_boundary(bytes[i - 1]) {
                    continue;
                }
                let prefix = line[i + 1..cursor].to_string();
                return Some((i, prefix));
            }
            // Stop scanning once we hit whitespace or a shell
            // metacharacter — `@` further left belongs to a different
            // token. We deliberately do NOT stop on `/` because file
            // paths commonly contain `/` after the `@` (e.g. `@src/foo`).
            b' ' | b'\t' | b'\n' | b'|' | b';' | b'&' | b'>' | b'<' => return None,
            _ => {}
        }
    }
    None
}

/// Convenience wrapper: returns just the partial path (without byte offset).
pub fn extract_at_prefix(line: &str, cursor: usize) -> Option<String> {
    extract_at_prefix_pair(line, cursor).map(|(_, p)| p)
}

fn is_path_separator_boundary(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n')
}

/// Walk `dir` looking for entries whose relative path starts with `prefix`.
/// `prefix` may be empty (return everything up to `MAX_RESULTS`).
///
/// Honors the module-level `MAX_DEPTH` and `SKIP_DIRS` constants. Results
/// are NOT cached here — callers should pass a `CompletionCache` or wrap
/// the call themselves when keystroke-frequency matters.
pub fn search_files_as_struct<P: AsRef<Path>>(
    dir: P,
    prefix: &str,
) -> Vec<FileEntry> {
    let dir = dir.as_ref();
    // Canonicalize so strip_prefix works when callers pass a relative
    // path (WalkDir yields absolute paths otherwise).
    let abs_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let prefix_lower = prefix.to_lowercase();
    let mut results = Vec::new();
    let walker = WalkDir::new(&abs_dir)
        .max_depth(MAX_DEPTH)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_skip_dir(e));

    for entry in walker.flatten() {
        if results.len() >= MAX_RESULTS {
            break;
        }
        let abs = entry.path();
        let Ok(rel) = abs.strip_prefix(&abs_dir) else { continue };
        let rel_str = rel.to_string_lossy().to_string();
        if rel_str.is_empty() {
            // The root entry itself.
            continue;
        }
        if !prefix.is_empty() && !rel_str.to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        results.push(FileEntry {
            relative_path: rel_str,
            is_dir: entry.file_type().is_dir(),
        });
    }
    // Sort: directories first, then case-insensitive lexicographic.
    results.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.relative_path.to_lowercase().cmp(&b.relative_path.to_lowercase()),
    });
    results
}

/// Cached variant: pass the same `CompletionCache` across calls to avoid
/// rescanning the directory on every keystroke.
pub fn search_files_cached<P: AsRef<Path>>(
    cache: &CompletionCache,
    dir: P,
    prefix: &str,
) -> Vec<FileEntry> {
    let dir = dir.as_ref();
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let key = CacheKey {
        cwd: canonical,
        prefix: prefix.to_string(),
    };
    if let Some(hit) = cache.get(&key) {
        return hit;
    }
    let results = search_files_as_struct(dir, prefix);
    cache.put(key, results.clone());
    results
}

fn should_skip_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    SKIP_DIRS.iter().any(|skip| name == *skip)
}

/// String-returning convenience used by tests and `nini info`.
pub fn search_files<P: std::fmt::Display>(dir: P, prefix: &str) -> Vec<String> {
    let p = PathBuf::from(dir.to_string());
    search_files_as_struct(&p, prefix)
        .into_iter()
        .map(|e| {
            if e.is_dir {
                format!("{}/", e.relative_path)
            } else {
                e.relative_path
            }
        })
        .collect()
}

/// Typed-string convenience for callers that want directories flagged.
pub fn search_files_typed<P: AsRef<Path>>(dir: P, prefix: &str) -> Vec<FileEntry> {
    search_files_as_struct(dir, prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_at_prefix_basic() {
        assert_eq!(extract_at_prefix("fix @Car", 8), Some("Car".to_string()));
        // cursor 15 puts us after the trailing 's' in "src/foo.rs".
        assert_eq!(
            extract_at_prefix("see @src/foo.rs", 15),
            Some("src/foo.rs".to_string())
        );
        // cursor in the middle of the path returns the partial prefix.
        assert_eq!(
            extract_at_prefix("see @src/foo.rs", 14),
            Some("src/foo.r".to_string())
        );
    }

    #[test]
    fn extract_at_prefix_none() {
        assert_eq!(extract_at_prefix("no at-sign here", 4), None);
        assert_eq!(extract_at_prefix("", 0), None);
        assert_eq!(extract_at_prefix("@", 1), Some(String::new()));
    }

    #[test]
    fn extract_at_prefix_ignores_email_like() {
        // `user@example` — the @ is preceded by 'r' (not a boundary), so
        // we must not treat it as a file path. The scanner will skip
        // past it and look for an earlier @, finding none.
        assert_eq!(extract_at_prefix("user@example.com", 15), None);
    }

    #[test]
    fn extract_at_prefix_cursor_past_end() {
        // cursor may equal line.len() (end-of-line).
        assert_eq!(extract_at_prefix_pair("@Car", 4), Some((0, "Car".to_string())));
    }

    #[test]
    fn extract_at_prefix_pair_returns_offset() {
        let (off, prefix) =
            extract_at_prefix_pair("see @src/foo.rs", 15).expect("must extract");
        assert_eq!(off, 4);
        assert_eq!(prefix, "src/foo.rs");
    }

    #[test]
    fn search_files_finds_cargo() {
        // Tests run from target/debug/deps, so use the absolute crate dir.
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hits = search_files_as_struct(&crate_dir, "Cargo");
        assert!(
            hits.iter().any(|e| e.relative_path == "Cargo.toml"),
            "Cargo.toml missing from {hits:?}"
        );
    }

    #[test]
    fn search_files_skips_target_dir() {
        // target/ exists in the workspace but should never appear in
        // completions.
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let hits = search_files_as_struct(&workspace_root, "");
        assert!(
            !hits.iter().any(|e| e.relative_path.contains("target/")),
            "target/ leaked into completions: {hits:?}"
        );
    }

    #[test]
    fn search_files_cached_returns_same_results() {
        let cache = CompletionCache::new();
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let first = search_files_cached(&cache, &crate_dir, "Car");
        let second = search_files_cached(&cache, &crate_dir, "Car");
        assert_eq!(first.len(), second.len());
        assert!(!first.is_empty());
    }

    #[test]
    fn search_files_capped_at_max_results() {
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .to_path_buf();
        let hits = search_files_as_struct(&workspace_root, "");
        assert!(hits.len() <= MAX_RESULTS, "got {} results", hits.len());
    }

    #[test]
    fn directories_sort_first() {
        let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let hits = search_files_as_struct(&crate_dir, "");
        let mut seen_non_dir = false;
        for h in &hits {
            if !h.is_dir {
                seen_non_dir = true;
            } else if seen_non_dir && h.is_dir {
                panic!("directory after non-directory: {h:?}");
            }
        }
    }
}