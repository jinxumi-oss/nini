use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Watches the settings file and reports theme changes.
///
/// # What this was
///
/// ```text
/// //! Theme watcher stub.
/// pub fn spawn_theme_watcher(cwd: Option<&Path>, _tx: UnboundedSender<ThemeEvent>) -> Option<PathBuf> {
///     cwd.map(|p| p.to_path_buf())
/// }
/// ```
///
/// It ignored the sender. `runtime.rs` selected on `theme_rx.recv()`, which
/// therefore never fired, so the whole theme-reload branch was unreachable -
/// and unreachable in a way that looked deliberate.
///
/// # Why it matters
///
/// Pi watches for theme changes
/// (`pi-coding-agent/src/utils/fs-watch.ts`). nini has no `/theme`
/// command at all - `commands::parse` has no theme id and `dispatch.rs`
/// has no handler - so the only way a user changes theme is to edit
/// `settings.json` while nini is running. Without a watcher that edit has
/// no effect until restart, and there is nothing else they can do.
///
/// # Polling, not fs-notify
///
/// `notify` is not in `Cargo.lock` and nini adds dependencies reluctantly.
/// A stat every 500 ms on one file is not measurable next to a full LLM
/// turn, and it has none of the failure modes a native watcher has across
/// editors that replace files rather than writing in place - which is most
/// of them, since editors commonly write a temp file and rename.
///
/// The first poll deliberately does not fire: the caller has just read
/// this file to render the first frame, and reporting "changed" at
/// startup would make the UI reload the theme it already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeEvent {
    Changed(PathBuf),
    Removed(PathBuf),
}

impl ThemeEvent {
    pub fn path(&self) -> &Path {
        match self {
            ThemeEvent::Changed(p) | ThemeEvent::Removed(p) => p,
        }
    }
}

/// How often the watched file is checked.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Watch `settings_file` and report changes to it.
///
/// Takes the **path**, not a working directory. The first version took
/// `cwd` and derived `<cwd>/.nini/settings.json`, which is only right when
/// settings came from the project level - it watches the wrong file when
/// they came from the agent directory, and it silently watches nothing at
/// all when the user has no project-level settings, which is the common
/// case. The caller already knows which file it loaded; making it repeat
/// that knowledge here is how the two drifted apart.
///
/// Returns `None` when there is no file to watch.
pub fn spawn_theme_watcher(
    settings_file: Option<PathBuf>,
    tx: tokio::sync::mpsc::UnboundedSender<ThemeEvent>,
) -> Option<PathBuf> {
    let file = settings_file?;

    // Establish the baseline without reporting it.
    let mut last = stamp(&file);

    // The caller gets the path back *and* the task keeps a copy; moving
    // the same `PathBuf` into both would not compile.
    let watched = file.clone();
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(POLL_INTERVAL);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes immediately; the baseline above already
        // covers it.
        ticks.tick().await;

        loop {
            ticks.tick().await;
            let now = stamp(&file);
            if now == last {
                continue;
            }
            let existed = last.is_some();
            let exists = now.is_some();
            last = now;

            let event = match (existed, exists) {
                (_, true) => Some(ThemeEvent::Changed(file.clone())),
                (true, false) => Some(ThemeEvent::Removed(file.clone())),
                (false, false) => None,
            };
            if let Some(e) = event {
                // A closed receiver means the TUI has stopped. Not an error.
                if tx.send(e).is_err() {
                    return;
                }
            }
        }
    });

    Some(watched)
}

/// Modification time and length, or `None` when the file is absent.
///
/// Size is included because editors that preserve mtime granularity but
/// rewrite in place would otherwise be invisible.
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!(
            "nini-theme-watch-{tag}-{}-{}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("scratch dir");
        p
    }

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn a_change_is_reported() {
        let dir = scratch("change");
        let _d = Dir(dir.clone());
        std::fs::write(dir.join("settings.json"), br#"{"theme":"dark"}"#).unwrap();

        let file = dir.join("settings.json");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let watched = spawn_theme_watcher(Some(file.clone()), tx).expect("watcher");
        assert_eq!(watched, file);

        std::fs::write(
            dir.join("settings.json"),
            br#"{"theme":"light","padding":"aaaaaaaa"}"#,
        )
        .unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a change should be reported")
            .expect("sender alive");
        assert_eq!(ev.path(), watched);
        assert!(matches!(ev, ThemeEvent::Changed(_)), "got {ev:?}");
    }

    /// The caller has just rendered a frame from this file; firing at
    /// startup would make it reload the theme it already applied.
    #[tokio::test]
    async fn startup_does_not_report_a_change() {
        let dir = scratch("startup");
        let _d = Dir(dir.clone());
        std::fs::write(dir.join("settings.json"), br#"{"theme":"dark"}"#).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_theme_watcher(Some(dir.join("settings.json")), tx);

        assert!(
            tokio::time::timeout(Duration::from_millis(1200), rx.recv())
                .await
                .is_err(),
            "the watcher fired for a file that had not changed"
        );
    }

    #[tokio::test]
    async fn removal_is_reported() {
        let dir = scratch("removal");
        let _d = Dir(dir.clone());
        let file = dir.join("settings.json");
        std::fs::write(&file, br#"{"theme":"dark"}"#).unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        spawn_theme_watcher(Some(file.clone()), tx);

        std::fs::write(&file, b"different length for sure").unwrap();
        // drain the first event
        let _ = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        std::fs::remove_file(&file).unwrap();

        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a removal should be reported")
            .expect("sender alive");
        assert!(matches!(ev, ThemeEvent::Removed(_)), "got {ev:?}");
    }

    #[test]
    fn no_cwd_means_no_watcher() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // synchronous context is fine here: with `None` nothing is spawned
        assert!(spawn_theme_watcher(None, tx).is_none());
    }

    /// Two writes of the same length inside one mtime tick are genuinely
    /// indistinguishable to a (mtime, length) stamp. That is a documented
    /// limit of watching without hashing, not a bug to paper over.
    #[test]
    fn the_stamp_notices_a_length_change() {
        let dir = scratch("stamp");
        let _d = Dir(dir.clone());
        let file = dir.join("settings.json");

        std::fs::write(&file, b"aaaa").unwrap();
        let a = stamp(&file);
        std::fs::write(&file, b"bbbbbbbb").unwrap();
        assert_ne!(a, stamp(&file), "a length change must be visible");

        std::fs::remove_file(&file).unwrap();
        assert_eq!(stamp(&file), None, "an absent file must read as None");
    }
}
