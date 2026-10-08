//! Oracle: a session format nini cannot read must be skipped, not fatal.
//!
//! P2: `pi-v4-unsupported.jsonl` had been sitting in the fixtures directory
//! with no test referencing it — a file that looks like coverage and is not.
//! Wired up here so the behaviour it implies is actually pinned.
//!
//! The reader's contract is tolerance *that is visible*: an unreadable
//! format must not take the whole session down, and it must not disappear
//! silently either. `Session::skipped_lines` is how it stays visible — the
//! TUI surfaces it as a stop notice next to the content it affects.

use nini_conformance::fixture;
use nini_session::Session;

#[test]
fn an_unreadable_format_is_skipped_not_fatal() {
    let path = fixture("pi-v4-unsupported.jsonl");
    assert!(path.exists(), "fixture missing: {}", path.display());

    let loaded = Session::read_from_file(&path)
        .expect("a session nini cannot parse must still load, with the bad line skipped");

    assert!(
        loaded.skipped_lines > 0,
        "the skipped line must be counted. Tolerance that is invisible is \
         its own kind of lie — the user would see a session with a hole in \
         it and no explanation."
    );
    assert!(
        loaded.entries.is_empty(),
        "a v4 header is not a session entry and must not be smuggled in \
         as one: {:?}",
        loaded.entries
    );
}

#[test]
fn the_fixture_really_is_a_format_we_cannot_read() {
    // Precondition, stated so this file cannot rot into a test that passes
    // because the fixture became something nini *can* read.
    let path = fixture("pi-v4-unsupported.jsonl");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("\"v\":4"),
        "the fixture is supposed to be a v4 header; it now reads:\n{text}"
    );
    assert!(
        !text.contains("\"type\":\"session\""),
        "and it is not supposed to contain a v3 session line:\n{text}"
    );
}
