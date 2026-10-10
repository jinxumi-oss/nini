//! v0.10 (R4a): `/trust` used to report success and remember nothing.
//!
//! `ProjectTrustStore::load` was `Ok(Self::default())` with the path
//! parameter named `_path`, `save` was `Ok(())`, `get` ignored its key and
//! `clear` did nothing. A user picked "trust this project", the UI said so,
//! and no file changed. The nine unit tests in the module all passed,
//! because they tested the policy arithmetic (`check`, `to_binary`) and
//! never the storage.
//!
//! These test the storage.

use nini_core::project_trust::{ProjectTrustStore, TrustDecision};
use std::path::PathBuf;

/// A unique scratch directory, removed when the `Dir` is dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        // A counter, not the tag. These tests run concurrently in one
        // process, so keying the directory on the tag gave three of them
        // the *same* directory — and the first to finish removed it out
        // from under the other two, which then failed with NotFound.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut p = std::env::temp_dir();
        p.push(format!("nini-trust-{tag}-{}-{}", std::process::id(), n));
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

fn tmp(tag: &str, name: &str) -> (PathBuf, Dir) {
    let dir = Dir::new(tag);
    (dir.path().join(name), dir)
}

#[test]
fn a_decision_survives_a_round_trip() {
    let (path, _d) = tmp("roundtrip", "trust.json");

    let mut store = ProjectTrustStore::default();
    store.set("/work/alpha", TrustDecision::Trusted);
    store.set("/work/beta", TrustDecision::Never);
    store.save(&path).expect("save");

    let back = ProjectTrustStore::load(&path).expect("load");
    assert_eq!(back.get_for_cwd("/work/alpha"), TrustDecision::Trusted);
    assert_eq!(back.get_for_cwd("/work/beta"), TrustDecision::Never);
}

#[test]
fn a_missing_file_is_an_empty_store_not_an_error() {
    let (path, _d) = tmp("absent", "absent.json");
    let store = ProjectTrustStore::load(&path).expect("a first run is not a failure");
    // everything falls back to the default, which asks
    assert_eq!(store.get_for_cwd("/anywhere"), TrustDecision::Ask);
}

#[test]
fn a_corrupt_file_is_an_error_not_a_silent_reset() {
    let (path, _d) = tmp("roundtrip", "trust.json");
    std::fs::write(&path, b"{ this is not json").unwrap();

    let err = ProjectTrustStore::load(&path).expect_err("corrupt must not read as empty");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

    // and the bad file is still there for the user to look at, rather
    // than having been quietly replaced
    assert!(path.exists());
}

/// Before this, `default_path()` returned the relative
/// `.nini/agent/trust.json`, which resolves against the *working
/// directory*. The same decision then landed in a different file
/// depending on where nini was launched from.
#[test]
fn the_default_path_is_absolute_and_under_the_agent_directory() {
    let path = ProjectTrustStore::default_path().expect("a home directory");
    assert!(
        path.is_absolute(),
        "trust.json path must not be relative: {path:?}"
    );
    assert!(
        path.ends_with("agent/trust.json"),
        "unexpected location: {path:?}"
    );
}

/// The path has to land somewhere writable for the round trip above to
/// mean anything, and the parent directory has to be created — a user who
/// has never opened `/trust` has no `agent/` directory either.
#[test]
fn saving_creates_the_directory_it_needs() {
    let dir = Dir::new("mkdir");
    let path = dir.path().join("agent").join("trust.json");
    assert!(!path.parent().unwrap().exists());

    ProjectTrustStore::default().save(&path).expect("save");
    assert!(path.exists(), "save did not create the parent directory");
}

/// Trust decisions are security state. A world-writable trust.json is a
/// store any local user can rewrite into "everything is trusted".
#[cfg(unix)]
#[test]
fn the_file_is_not_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let (path, _d) = tmp("roundtrip", "trust.json");
    ProjectTrustStore::default().save(&path).expect("save");

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "trust.json is mode {mode:o}, expected 600");
}

/// A temp file left next to the real one would make it look like a second
/// project had been trusted.
#[test]
fn no_temp_file_is_left_behind() {
    let (path, dir) = tmp("strays", "trust.json");
    ProjectTrustStore::default().save(&path).expect("save");
    let strays: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "trust.json")
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

/// `clear` was an empty function, so once a directory was trusted there
/// was no way back to "ask me".
#[test]
fn clearing_forgets_one_project_and_only_one() {
    let mut store = ProjectTrustStore::default();
    store.set("/work/alpha", TrustDecision::Trusted);
    store.set("/work/beta", TrustDecision::Trusted);

    store.clear("/work/alpha");

    assert_eq!(store.get_for_cwd("/work/alpha"), TrustDecision::Ask);
    assert_eq!(store.get_for_cwd("/work/beta"), TrustDecision::Trusted);
}

/// `get` returned the store-wide default for *any* key, including one that
/// had been explicitly set, so the trust selector showed the wrong
/// project's answer.
#[test]
fn get_returns_a_projects_own_decision() {
    let mut store = ProjectTrustStore::default();
    store.decision = TrustDecision::Distrusted;
    store.set("/work/alpha", TrustDecision::Trusted);

    assert_eq!(store.get("/work/alpha"), Some(TrustDecision::Trusted));
    assert_eq!(store.get("/work/gamma"), Some(TrustDecision::Distrusted));
}

/// The shape on disk is nini's own, and it survives the rename of every
/// field. Pinning it means a later edit that renames `cwd_map` shows up
/// here rather than silently orphaning every user's decisions.
#[test]
fn the_file_shape_is_stable() {
    let (path, _d) = tmp("roundtrip", "trust.json");
    let mut store = ProjectTrustStore::default();
    store.set("/work/alpha", TrustDecision::Trusted);
    store.save(&path).unwrap();

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["decision"]["level"], "ask");
    assert_eq!(raw["cwd_map"]["/work/alpha"]["level"], "trusted");
}
