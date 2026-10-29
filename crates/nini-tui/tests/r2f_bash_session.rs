//! R2f: a `!cmd` the user ran must survive `/resume`.
//!
//! `!cmd` pushed a `TranscriptLine::BashExecution` and stopped there.
//! `BashExecutionMessage` had no production constructor anywhere in the
//! tree - only a test helper built one - so a shell command the user ran was
//! visible while the session was open and gone the moment they resumed.
//! `render_context_entries`, added in A4 to rebuild the transcript from the
//! session, dropped the variant into its `_` arm regardless.
//!
//! The output is still injected as a user message for the model, so what the
//! agent saw was never the problem. What a resumed session showed was.

use nini_tui::state::{AppState, TranscriptLine};

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nini-r2f-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Wait for the session file to land.
///
/// `session_flush` writes on a **background thread** (`state.rs`: the IO
/// deliberately left the lock, so a reader is not blocked behind a
/// serialise), so the file does not exist the moment `flush` returns.
/// A fixed sleep is a race: 120ms is usually enough on a developer
/// machine and not enough on a loaded CI runner — which is exactly how
/// this test went red on macOS.
///
/// So: poll for the file, with a bounded wait. The write is a
/// serialise-then-rename, so the file's **existence** already means it
/// is complete.
fn wait_for_session_file(path: &std::path::Path) {
    for _ in 0..250 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("the session file never landed: {}", path.display());
}

/// The headline. Record a `!cmd`, reload from disk, find it.
#[test]
fn a_shell_command_the_user_ran_survives_a_reload() {
    let dir = scratch("reload");
    let path = dir.join("s.jsonl");

    {
        let mut s = AppState::new("test-model");
        s.session_init(path.clone());
        s.push_user("what does this repo do?");
        s.session_append_bash_execution(
            "rg --files | head -20",
            "crates/nini-core/src/lib.rs\ncrates/nini-ai/src/lib.rs",
            "",
            true,
            Some(0),
            42,
        );
        s.session_flush();
        wait_for_session_file(&path);
    }

    let mut fresh = AppState::new("test-model");
    fresh.session_load(path.clone()).expect("must load");

    let found = fresh.transcript_state.lines.iter().find_map(|l| match l {
        TranscriptLine::BashExecution { cmd, output, .. } => Some((cmd.clone(), output.clone())),
        _ => None,
    });
    assert!(
        found.is_some(),
        "a command the user ran must be in the reloaded transcript. Before \
         R2f it was display-only and vanished on resume."
    );
    let (cmd, output) = found.unwrap();
    assert_eq!(cmd, "rg --files | head -20");
    assert!(
        output.contains("nini-core/src/lib.rs"),
        "and it must carry its output, not just the command line"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The entry has to be a real `BashExecution` message, not a user message
/// dressed as one — the wire type is what Pi reads.
#[test]
fn it_is_recorded_as_a_bash_execution_message() {
    let dir = scratch("wire");
    let path = dir.join("s.jsonl");

    {
        let mut s = AppState::new("test-model");
        s.session_init(path.clone());
        s.session_append_bash_execution("echo hi", "hi", "", true, Some(0), 1);
        s.session_flush();
        wait_for_session_file(&path);
    }

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("\"role\":\"bashExecution\""),
        "Pi reads this as a bashExecution message, not a user message: {text}"
    );
    assert!(
        text.contains("\"exitCode\":0"),
        "and the exit code has to be on the wire: {text}"
    );
    // The negative half matters: this assertion would pass just as well
    // against a file that happened to contain the word somewhere else, so
    // it is paired with one that says the entry is NOT a user message. A
    // user message serialises with `"role":"user"` and can never produce
    // the tag above.
    let bash_line = text
        .lines()
        .find(|l| l.contains("\"role\":\"bashExecution\""))
        .expect("a bashExecution line");
    assert!(
        !bash_line.contains("\"role\":\"user\""),
        "the entry must not be a user message in disguise: {bash_line}"
    );

    // And it must parse back through the normal reader.
    let reloaded = nini_session::Session::read_from_file(&path).expect("must reload");
    assert!(
        reloaded.entries.iter().any(|e| matches!(
            e,
            nini_core::SessionEntry::Message(m)
                if matches!(m.message, nini_core::entries::AgentMessage::BashExecution(_))
        )),
        "a real session entry, not merely text in the file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// The model's view of a `!cmd` is deliberately out of scope here.
//
// The output is injected as a user message for the agent by runtime.rs,
// and that path was never broken - it is not worth a test that only
// proves the helper it calls returns a value. What was broken, and what
// this file covers, is what a resumed *session* shows.
