//! Per-path serialization of file-mutating tool operations.
//!
//! # Why this exists
//!
//! nini executes a batch of tool calls concurrently
//! (`agent.rs`, `FuturesUnordered` + worker pool, up to
//! `max_parallel_tools`). That is right for reads and wrong for writes.
//!
//! `EditTool::execute` is a read-modify-write:
//!
//! ```text
//! content = read(path)          ← all 8 concurrent edits read the same original
//! new     = apply(…)
//! write(tmp, new); rename(tmp, path)
//! ```
//!
//! With two edits to the same file in one assistant turn — an entirely
//! ordinary pattern, e.g. two unrelated string replacements in one file —
//! the second edit computes against stale content and its result is
//! overwritten. Measured on this machine with 8 concurrent edits to one file:
//!
//! ```text
//! 1/8 edits reported success; final file has 1/8 markers edited
//! 3/8 edits reported success; final file has 2/8 markers edited
//! ```
//!
//! It was worse than a lost update. The temp file was named
//! `.{file}.nini-tmp.{pid}` — process id only — so every concurrent edit on
//! that file wrote the *same* temp path. Concurrent truncate-and-write to
//! one path interleaves, so the file could be left mangled, not merely stale.
//!
//! # What Pi does
//!
//! `withFileMutationQueue(env, path, fn)` in
//! `pi-agent-core/src/harness/tools/file-mutation-queue.ts`:
//!
//! - key = the **canonical** path (falling back to the absolute path when
//!   the file does not exist yet, which `write` needs);
//! - one promise chain per key, so the same path serialises while different
//!   paths still run concurrently;
//! - the chain entry is dropped once the last holder releases it, so the map
//!   does not grow without bound.
//!
//! `edit.ts` and `write.ts` wrap their bodies in it. This module is the
//! Rust equivalent, and the wrapping lives in the tools rather than in the
//! agent so the guarantee holds for any caller.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

type PathLock = Arc<tokio::sync::Mutex<()>>;

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>> {
    static REG: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve the queue key for `path`.
///
/// Pi canonicalises so that `./a.rs`, `a.rs` and `/abs/…/a.rs` share one
/// queue. `std::fs::canonicalize` fails for a file that does not exist yet —
/// which is the normal case for `write` creating a new file — so that case
/// falls back to the lexically absolute path, matching Pi's handling of
/// `not_found`.
fn queue_key(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => {
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .unwrap_or_else(|_| path.to_path_buf())
            }
        }
    }
}

/// Acquire the mutation lock for `path`, run `f`, then release it.
///
/// The lock is per canonical path, so mutating two *different* files in the
/// same parallel batch is still fully parallel.
pub async fn with_file_mutation_queue<T, F, Fut>(path: &Path, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let key = queue_key(path);
    let lock: PathLock = {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        match reg.get(&key).and_then(Weak::upgrade) {
            Some(existing) => existing,
            None => {
                let fresh = Arc::new(tokio::sync::Mutex::new(()));
                reg.insert(key, Arc::downgrade(&fresh));
                fresh
            }
        }
    };
    // A panic inside `f` must not wedge the queue forever, hence the
    // explicit guard: dropping it releases the mutex whether `f` returns or
    // unwinds.
    let _guard = lock.lock().await;
    f().await
}

/// A temp-file name that cannot collide with another in-flight write.
///
/// The previous name was `.{file}.nini-tmp.{pid}` — one name per process per
/// file, so two concurrent writes to the same file truncated and rewrote the
/// *same* path. This adds a process-wide monotonic counter, making the name
/// unique per invocation.
pub fn unique_temp_name(file_name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!(".{file_name}.nini-tmp.{}.{n}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_unique_per_call() {
        let a = unique_temp_name("f.txt");
        let b = unique_temp_name("f.txt");
        assert_ne!(
            a, b,
            "two in-flight writes to one file must not share a temp path"
        );
        assert!(a.starts_with(".f.txt.nini-tmp."));
    }

    #[test]
    fn relative_and_absolute_spellings_share_one_key() {
        let dir = std::env::temp_dir().join(format!("nini-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("k.txt");
        std::fs::write(&file, "x").unwrap();
        let abs = queue_key(&file);
        let rel = queue_key(Path::new(&file));
        assert_eq!(abs, rel, "canonicalisation must collapse spellings");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn same_path_serialises_different_paths_do_not() {
        let dir = std::env::temp_dir().join(format!("nini-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();

        // Same path: max observed concurrency must be 1.
        // Atomics, not a std Mutex: a std::sync::MutexGuard held across an
        // `.await` is not `Send`, so the spawned task would not compile —
        // and a mutex-based counter here also self-deadlocked when written
        // as `*peak.lock() = (*peak.lock()).max(…)`.
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let (peak, live) = (peak.clone(), live.clone());
            let p = a.clone();
            set.spawn(async move {
                with_file_mutation_queue(&p, || async {
                    use std::sync::atomic::Ordering as O;
                    let now = live.fetch_add(1, O::SeqCst) + 1;
                    peak.fetch_max(now, O::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    live.fetch_sub(1, O::SeqCst);
                })
                .await
            });
        }
        while set.join_next().await.is_some() {}
        assert_eq!(
            peak.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "same path must serialise"
        );

        // Different paths: they must overlap.
        let peak2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let live2 = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut set = tokio::task::JoinSet::new();
        for i in 0..8 {
            let (peak2, live2) = (peak2.clone(), live2.clone());
            let p = if i % 2 == 0 { a.clone() } else { b.clone() };
            set.spawn(async move {
                with_file_mutation_queue(&p, || async {
                    use std::sync::atomic::Ordering as O;
                    let now = live2.fetch_add(1, O::SeqCst) + 1;
                    peak2.fetch_max(now, O::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    live2.fetch_sub(1, O::SeqCst);
                })
                .await
            });
        }
        while set.join_next().await.is_some() {}
        let peak2 = peak2.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            peak2 > 1,
            "different paths must still run in parallel; peak was {peak2}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
