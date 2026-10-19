//! Oracle: two sessions exporting at the same instant must not share a file.
//!
//! # The defect (`3df653cc`)
//!
//! `/export` derived its filename from a wall-clock stamp alone:
//!
//! ```text
//! let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S-%6f"); // microseconds
//! let p = d.join(format!("session-{ts}.html"));
//! ```
//!
//! Microsecond resolution is a *guess* about how close two writers can get,
//! not a claim about who wrote what. When two threads reach that line inside
//! one microsecond — which `cargo test` does routinely, since a whole test
//! binary is a process full of them — both compute the same path and the
//! second `write` clobbers the first. The reported symptom in the CI log was
//! a test panicking on "path should exist" or on the path line containing
//! `(failed`, because the file it thought it had written was someone else's
//! — or was still held open mid-write.
//!
//! A filename that cannot name its writer is not a filename, it is a
//! coincidence with an extension. The stamp was never the identity; the
//! **process id** and the **thread id** are.
//!
//! # The oracle
//!
//! Not a filename regex. Eight threads, released from a barrier so they are
//! inside the same microsecond as often as the scheduler allows, each with
//! a transcript containing **its own** marker:
//!
//! 1. every writer gets a **different** path (8 paths, 8 distinct strings);
//! 2. every file **exists** and its content contains its **own** marker;
//! 3. no file contains **anybody else's** marker — this is the one that
//!    bites, because a clobbered file is a valid HTML file holding the
//!    wrong session;
//! 4. the name carries the writer's identity: this process's pid and a
//!    per-thread component, because a timestamp separates two *moments* and
//!    only these separate two *writers*.
//!
//! invariant: 3df653cc

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};

use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;

/// One environment variable, one lock: this is the only test in this binary
/// that touches the environment, and cargo runs tests in a binary in
/// parallel threads.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const WRITERS: usize = 8;

fn marker(i: usize) -> String {
    format!("session-marker-writer-{i}")
}

/// Export once on a fresh thread and hand back the path `/export` reported.
fn export_once(tag: usize) -> String {
    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    state.push_user(marker(tag));
    state.push_assistant(format!("{}", marker(tag)));

    let r = dispatch(&mut state, &mut settings, CommandId::Export, "");
    let lines = match r.outcome {
        CommandOutcome::Output(lines) => lines,
        other => panic!("expected Output, got {other:?}"),
    };
    let path_line = &lines[0];
    assert!(
        path_line.starts_with("export → "),
        "unexpected export output: {path_line}"
    );
    assert!(
        !path_line.contains("(failed"),
        "the export reported a write failure: {path_line}"
    );
    assert!(
        !path_line.contains("(no session directory"),
        "no exports directory: {path_line}"
    );
    path_line.trim_start_matches("export → ").to_string()
}

#[test]
fn concurrent_exporters_get_distinct_files_holding_their_own_transcript() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("NINI_AGENT_DIR", dir.path());

    // All writers are released together, so the window in which they
    // collide is as small as the scheduler can make it.
    let barrier = Arc::new(Barrier::new(WRITERS));
    let mut handles = Vec::new();
    for i in 0..WRITERS {
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            export_once(i)
        }));
    }
    let paths: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // 1. Distinct paths. 8 writers, 8 files.
    let distinct: BTreeSet<&String> = paths.iter().collect();
    assert_eq!(
        distinct.len(),
        WRITERS,
        "two writers were handed the same export path, so one session \
         overwrote the other:\n{paths:#?}"
    );

    // 2 & 3. Each file exists, holds its own transcript, and nobody else's.
    for (i, path) in paths.iter().enumerate() {
        let p = std::path::Path::new(path);
        assert!(p.exists(), "exported file missing: {path}");
        let content = std::fs::read_to_string(p).unwrap_or_default();
        assert!(
            content.contains("<!DOCTYPE html>"),
            "not an HTML export: {path}"
        );
        assert!(
            content.contains(&marker(i)),
            "writer {i}'s export does not contain its own transcript \
             (another writer clobbered it): {path}"
        );
        for j in 0..WRITERS {
            if j != i {
                assert!(
                    !content.contains(&marker(j)),
                    "writer {i}'s export contains writer {j}'s transcript: {path}"
                );
            }
        }
    }

    // 4. The name identifies the writer, not the moment.
    let pid = std::process::id().to_string();
    for path in &paths {
        let stem = std::path::Path::new(path)
            .file_stem()
            .expect("export has a file name")
            .to_string_lossy()
            .into_owned();
        assert!(
            stem.contains(&pid),
            "export name {stem} carries no process id — it cannot tell two \
             processes' exports apart, only two moments"
        );
        assert!(
            stem.split('-').any(|part| {
                part.len() > 1
                    && part.starts_with('t')
                    && part[1..].chars().all(|c| c.is_ascii_digit())
            }),
            "export name {stem} carries no thread id — it cannot tell two \
             writers inside one process apart, which is the case the \
             microsecond stamp failed at"
        );
    }
}
