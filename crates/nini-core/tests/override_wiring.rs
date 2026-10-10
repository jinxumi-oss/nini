//! v0.10 (R4b): `NINI_AGENT_DIR` was decorative.
//!
//! `paths::agent_dir()` honoured the override, and four call sites went
//! through it - `settings.json`, `models.json`, `skills/`, the `/resume`
//! listing. Six more built `~/.nini/agent/...` straight out of `$HOME` and
//! ignored it: session creation, the session picker, `/export`, and trust
//! load *and* save. Sessions, exports and trust are the three things a
//! user would notice, and they were the three that ignored it.
//!
//! The previous test (`the_override_is_ninis_own`) asserted
//! `paths::agent_file("settings.json")`. That is an accessor test: it
//! would stay green with all six call sites reverted. These assert the
//! locations the production code actually derives.
//!
//! `HOME` and `NINI_AGENT_DIR` are process-wide, so every test here holds
//! one lock for the whole read-modify-restore cycle. Cargo runs tests in
//! one binary on many threads; a lock scoped to a single `set_var` would
//! not help, because the window that matters is between setting the value
//! and the assertion reading it.

use nini_core::paths;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Run `f` with `HOME` and `NINI_AGENT_DIR` both under a private
/// directory, restoring whatever was there.
fn in_sandbox<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
    let _g = env_lock();
    let home = std::env::var_os("HOME");
    let over = std::env::var_os(paths::ENV_AGENT_DIR);
    unsafe {
        std::env::set_var("HOME", dir);
        std::env::set_var(paths::ENV_AGENT_DIR, dir);
    }
    let out = f();
    unsafe {
        match home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match over {
            Some(v) => std::env::set_var(paths::ENV_AGENT_DIR, v),
            None => std::env::remove_var(paths::ENV_AGENT_DIR),
        }
    }
    out
}

/// A scratch directory, unique per call. Keyed on a counter because these
/// run concurrently in one process and a shared name would have one test
/// delete another's directory.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut p = std::env::temp_dir();
        p.push(format!("nini-sandbox-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("scratch dir");
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The six locations that ignored the override. Each is the expression the
/// production call site evaluates, spelled out so a regression shows up as
/// a failing assertion rather than as a silent divergence.
#[test]
fn every_location_the_user_notices_follows_the_override() {
    let dir = Dir::new("locations");
    in_sandbox(dir.path(), || {
        // 1. new sessions - `/new`
        let project = std::env::current_dir()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "default".to_string());
        let new_session = paths::sessions_dir().expect("dir").join(&project);

        // 2. the session picker
        let picker = paths::sessions_dir().expect("dir").join(&project);

        // 3. `/export`
        let exports = paths::exports_dir().expect("dir");

        // 4 & 5. trust load and save share one file
        let trust = paths::trust_file().expect("file");

        // and the four that already worked, so a future move of *these*
        // is caught too. `models.json` has two candidates on purpose: the
        // project one is relative to the project, only the user-level one
        // belongs under the agent directory.
        let settings = paths::agent_file("settings.json").expect("file");
        let model_candidates = paths::candidates(Path::new("/proj"), "models.json");
        let models_user = model_candidates[1].clone();
        let models_project = model_candidates[0].clone();
        let skills = nini_core::skills::user_skills_dir().expect("dir");
        let resume = paths::sessions_dir().expect("dir");

        let root = dir.path();
        for (label, got) in [
            ("new session", new_session),
            ("session picker", picker),
            ("exports", exports),
            ("trust", trust),
            ("settings", settings),
            ("models (user level)", models_user),
            ("skills", skills),
            ("resume listing", resume),
        ] {
            assert!(
                got.starts_with(root),
                "{label} resolved to {}, which is outside the sandbox {root:?}",
                got.display()
            );
        }

        // The project candidate must stay project-relative. Asserted
        // separately rather than lumped in above: it is *supposed* to fall
        // outside the sandbox, and folding it in would have had this test
        // fail for the right reason at the wrong place.
        assert_eq!(
            models_project,
            PathBuf::from("/proj/.nini/models.json"),
            "a project-level file must not follow NINI_AGENT_DIR"
        );
    });
}

/// The inverse: with the override set and `HOME` pointing somewhere
/// entirely different, nothing may come from `HOME`. Without this, an
/// implementation that consults both would pass the test above whenever
/// they happened to agree.
#[test]
fn the_override_wins_over_home() {
    let override_dir = Dir::new("override");
    let home_dir = Dir::new("home");
    let _g = env_lock();

    let home = std::env::var_os("HOME");
    let over = std::env::var_os(paths::ENV_AGENT_DIR);
    unsafe {
        std::env::set_var("HOME", home_dir.path());
        std::env::set_var(paths::ENV_AGENT_DIR, override_dir.path());
    }

    let sessions = paths::sessions_dir().expect("dir");
    let exports = paths::exports_dir().expect("dir");
    let trust = paths::trust_file().expect("file");

    unsafe {
        match home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match over {
            Some(v) => std::env::set_var(paths::ENV_AGENT_DIR, v),
            None => std::env::remove_var(paths::ENV_AGENT_DIR),
        }
    }

    for (label, got) in [
        ("sessions", sessions),
        ("exports", exports),
        ("trust", trust),
    ] {
        assert!(
            got.starts_with(override_dir.path()),
            "{label} came from HOME: {}",
            got.display()
        );
        assert!(
            !got.starts_with(home_dir.path()),
            "{label} ignored the override: {}",
            got.display()
        );
    }
}

/// `/new` used to write `./.nini/agent/sessions/...` into the project the
/// user was standing in when `HOME` was unset. There is no honest path to
/// invent there, so the resolver returns `None` and the command records
/// nothing and says so.
#[test]
fn with_no_home_and_no_override_there_is_no_invented_path() {
    let _g = env_lock();
    let home = std::env::var_os("HOME");
    let over = std::env::var_os(paths::ENV_AGENT_DIR);
    unsafe {
        std::env::remove_var("HOME");
        std::env::remove_var(paths::ENV_AGENT_DIR);
    }

    let sessions = paths::sessions_dir();
    let exports = paths::exports_dir();
    let trust = paths::trust_file();
    let agent = paths::agent_dir();

    unsafe {
        match home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match over {
            Some(v) => std::env::set_var(paths::ENV_AGENT_DIR, v),
            None => std::env::remove_var(paths::ENV_AGENT_DIR),
        }
    }

    assert_eq!(sessions, None, "a session directory was invented");
    assert_eq!(exports, None, "an exports directory was invented");
    assert_eq!(trust, None, "a trust file was invented");
    assert_eq!(agent, None, "an agent directory was invented");
}

/// The specific fabrication the old code did. `unwrap_or_else(|| ".")`
/// produced `./.nini/agent/sessions`, and the `/resume` listing produced
/// `$HOME/fallback/sessions` - a path that exists nowhere and was never
/// written to, so the listing only ever looked like it was looking.
#[test]
fn neither_of_the_old_fallbacks_can_be_produced() {
    let dir = Dir::new("fallbacks");
    in_sandbox(dir.path(), || {
        for (label, got) in [
            ("sessions", paths::sessions_dir().expect("dir")),
            ("exports", paths::exports_dir().expect("dir")),
            ("trust", paths::trust_file().expect("file")),
        ] {
            let text = got.to_string_lossy().to_string();
            assert!(
                !text.contains("/fallback/"),
                "{label} still resolves through fallback/: {text}"
            );
            assert!(
                !got.is_relative(),
                "{label} is relative, so it lands wherever nini was started: {text}"
            );
            assert!(
                !text.contains("/./"),
                "{label} contains a `.` component: {text}"
            );
        }
    });
}
