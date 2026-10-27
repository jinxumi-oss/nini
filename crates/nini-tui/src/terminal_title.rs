//! The terminal title: what the window (or tab, or tmux pane) says while
//! nini is running.
//!
//! # What Pi does
//!
//! Pi 0.85.1 sets the title in `updateTerminalTitle`
//! (`references/pi-0.85.1/src/ui/interactive-mode.ts:1020`), called from
//! four places (`:1064`, `:1991`, `:2263`, `:3210`):
//!
//! ```text
//! Pi - <sessionName> - <cwdBasename>   // a session name is known
//! Pi - <cwdBasename>                  // it is not
//! ```
//!
//! `APP_TITLE` is Pi's product title, imported from `config.js`
//! (`interactive-mode.ts:49`); that module is not part of the snapshot, so
//! the *literal* is not verifiable here. What is verifiable — and what this
//! module reproduces — is the **shape**: `<product> - <session label> -
//! <cwd basename>`.
//!
//! # What nini does differently, and why
//!
//! * **The product title is `nini`** (`APP_TITLE` below), the direct
//!   counterpart of Pi's `APP_TITLE`.
//! * **The session label is the session id, not a session name.** Pi has a
//!   user-facing session *name* (a `/rename`-able title). nini has no
//!   session-name concept at all: a session is identified by
//!   `u_<uuid>` (`nini-session/src/lib.rs:166`, `SessionHeader::new`).
//!   Using the id is the honest approximation — it is unique, it is what
//!   the on-disk file is called, and it never invents a name the user
//!   cannot see anywhere else. When no session exists yet (the TUI starts
//!   before the first session file is written), the id is absent and the
//!   title degrades to Pi's second form: `nini - <cwdBasename>`.
//! * **The cwd basename is computed the way Node computes it.** Pi calls
//!   `path.basename(cwd)`; `Path::file_name` is close but not identical
//!   (it returns `None` for `/`, and it does not agree with Node on a
//!   path with a trailing slash), so the rule is spelled out here in
//!   [`cwd_basename`] instead of being inherited from `std::path`.
//!
//! # Writing it
//!
//! The TUI owns stdout while it runs (see
//! `crates/nini-tui/tests/no_terminal_writes.rs`: a stdout write during a
//! TUI session is a defect). So the title is written through an
//! **injected writer**: [`update_terminal_title`] takes a `Fn(&str)` and
//! the test hands it a closure that records the string instead of writing
//! it. [`write_title_to_terminal`] is the real sink (crossterm's
//! `SetTitle`, an OSC 2 sequence, not text — it cannot corrupt a frame).

/// nini's product title — the counterpart of Pi's `APP_TITLE`.
pub const APP_TITLE: &str = "nini";

/// The last path segment of `cwd`, the way Node's `path.basename` computes it.
///
/// Node's rules, in full, because `std::path::Path::file_name` does not
/// agree with them:
///
/// | input        | `path.basename` | note                       |
/// |--------------|-----------------|----------------------------|
/// | `/home/u/p`  | `p`             | the usual case             |
/// | `/home/u/p/` | `p`             | a trailing slash is ignored |
/// | `/a//b`      | `b`             | an empty segment is ignored |
/// | `/`          | `""`            | the root has **no** name   |
/// | `""`         | `""`            | an empty path has no name |
/// | `rel/path`   | `path`          | relative paths work the same |
///
/// A path that ends in `..` keeps the `..`: `path.basename("/a/..")` is
/// `".."`, and the caller asked for a directory name, not a resolved one.
///
/// The root case is worth a note, because it is the one an
/// implementation is most likely to get wrong: Node's `path.basename("/")`
/// is the **empty string**, not `"/"`. (`path.basename("//")` likewise.)
/// Verified with node:
/// `path.basename('/') === ''`.
pub fn cwd_basename(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    // Node strips the trailing separators before taking the last segment.
    let trimmed = cwd.trim_end_matches('/');
    if trimmed.is_empty() {
        // The path was nothing but slashes: Node's basename("/") is "".
        return String::new();
    }
    match trimmed.rsplit_once('/') {
        Some((_, last)) => last.to_string(),
        None => trimmed.to_string(),
    }
}

/// The title nini would show, for a session id and a working directory.
///
/// Mirrors Pi's `updateTerminalTitle`
/// (`references/pi-0.85.1/src/ui/interactive-mode.ts:1020`):
///
/// ```text
/// nini - <session id> - <cwd basename>
/// nini - <cwd basename>            // no session id
/// ```
///
/// A blank session id counts as no session id: an empty segment in a
/// title (`"nini -  - proj"`) is a bug that reads as a typo, and the
/// degraded form is exactly Pi's second form.
pub fn terminal_title(session_id: Option<&str>, cwd: &str) -> String {
    let basename = sanitize(&cwd_basename(cwd));
    match session_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(sanitize)
    {
        Some(id) => format!("{APP_TITLE} - {id} - {basename}"),
        None => format!("{APP_TITLE} - {basename}"),
    }
}

/// Strip control characters from a piece of the title.
///
/// The title is written as an **OSC 2** sequence
/// (`ESC ] 0 ; <title> BEL`), and the title text comes from the
/// filesystem: the cwd's basename is a directory name, and on Linux a
/// directory name may contain `ESC` or `BEL`. An unescaped `BEL` would
/// end the OSC sequence early, and an unescaped `ESC` would start
/// another one — so the title could carry a control sequence of the
/// user's own making into the terminal.
///
/// Three lines of stripping is cheaper than that.
fn sanitize(part: &str) -> String {
    part.chars()
        .filter(|c| !c.is_control() && *c != '\u{7f}')
        .collect()
}

/// Compute the title and hand it to `write`.
///
/// The writer is injected so a test can capture the title instead of
/// writing to the terminal — the TUI owns stdout while it runs, and a
/// title write is a deliberate, narrow exception to that rule
/// (an OSC 2 sequence, not text). Any `Fn(&str)` closure qualifies; the
/// bound is `FnMut` so a writer that accumulates (a recorder) also
/// fits.
pub fn update_terminal_title<F>(session_id: Option<&str>, cwd: &str, mut write: F)
where
    F: FnMut(&str),
{
    write(&terminal_title(session_id, cwd));
}

/// Write `title` to the terminal through crossterm's `SetTitle`.
///
/// This is the real sink: `SetTitle` emits an OSC 2 sequence, which sets
/// the window title and puts **no glyph** on the painted screen. The
/// no-terminal-writes invariant
/// (`crates/nini-tui/tests/no_terminal_writes.rs`) forbids *text* writes
/// that would land on top of the frame; a control sequence is not text,
/// and the TUI's own renderer writes to stdout through crossterm anyway
/// (that is how the frame gets there).
pub fn write_title_to_terminal(title: &str) -> std::io::Result<()> {
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(title.to_string())
    )
}

/// Compute the title and write it to the terminal.
///
/// The convenience form of [`update_terminal_title`] +
/// [`write_title_to_terminal`], for the call site that has a real
/// terminal and no test to satisfy.
pub fn set_terminal_title(session_id: Option<&str>, cwd: &str) -> std::io::Result<()> {
    let mut title = String::new();
    update_terminal_title(session_id, cwd, |t| title.push_str(t));
    write_title_to_terminal(&title)
}

/// A writer that keeps the last title it was given, for tests and for
/// the "what does the title say right now?" question.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TitleRecorder {
    last: Option<String>,
}

impl TitleRecorder {
    /// A recorder that has seen nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last title written, if any.
    pub fn last(&self) -> Option<&str> {
        self.last.as_deref()
    }

    /// The writer to hand to [`update_terminal_title`].
    pub fn writer(&mut self) -> impl FnMut(&str) + '_ {
        move |title: &str| {
            self.last = Some(title.to_string());
        }
    }
}
