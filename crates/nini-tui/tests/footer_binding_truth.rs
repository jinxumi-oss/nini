//! Oracle: the footer may not advertise a key the keymap does not have.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! `render_key_hints` is a hand-written `Vec<(&str, &str)>` with no
//! connection to `default_keymap()`. Three hints drifted out of sync:
//!
//! | hint        | footer claims        | `default_keymap()`     |
//! |-------------|----------------------|------------------------|
//! | `Ctrl+Z`    | `undo`               | **no such binding**    |
//! | `Alt+Backspace` | `kill-word`      | **no such binding**    |
//! | `Ctrl+C`    | `quit`               | `Abort` — and `Abort` with no popup clears the input buffer |
//!
//! The first two are keys that do nothing at all: the user presses them
//! and the TUI shows no response whatsoever. The third is worse than a
//! dead key — the footer promises an exit, and pressing it **destroys
//! half-typed input** instead (`runtime.rs`, the `Abort` arm's final
//! `else { state.input.clear() }`).
//!
//! **Test discipline (v0.10 §35.6)**: this file only calls APIs that
//! exist on both sides of the fix — `render_frame`, `AppState`, and
//! `default_keymap` are all pre-existing. A test that cannot build is
//! not an oracle, and this defect has already cost the session two such
//! dead ends (§33/§34).
//!
//! The footer is read out of the rendered frame rather than from a
//! helper function, because no such helper is public — and inventing
//! one just to be able to test would put this file's whole premise
//! (compiles against the unfixed code) at risk.

use nini_tui::keys::{Key, KeyCode, KeyModifiers};
use nini_tui::render::render_frame;
use nini_tui::state::AppState;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const CTRL: KeyModifiers = KeyModifiers::CTRL;
const ALT: KeyModifiers = KeyModifiers::ALT;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

/// The rendered footer row: the last row of the frame.
fn footer_text(state: &AppState, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_frame(f, state)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let y = h - 1;
    let mut s = String::new();
    for x in 0..buf.area.width {
        if let Some(c) = buf.cell((x, y)) {
            s.push_str(c.symbol());
        }
    }
    s
}

/// Parse a footer key token such as `Ctrl+Z` into the normalized key the
/// keymap stores. Returns `None` for tokens this parser does not cover,
/// which the caller must treat as "unknown", not as "unbound".
fn parse_token(token: &str) -> Option<Key> {
    let t = token.trim();
    let (mods, rest) = if let Some(r) = t.strip_prefix("Ctrl+") {
        (CTRL, r)
    } else if let Some(r) = t.strip_prefix("Alt+") {
        (ALT, r)
    } else if let Some(r) = t.strip_prefix("Shift+") {
        (SHIFT, r)
    } else {
        (KeyModifiers(0), t)
    };
    let code = match rest {
        "Enter" => KeyCode::Enter,
        "Esc" => KeyCode::Esc,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        "F1" => KeyCode::F(1),
        other => {
            let mut cs = other.chars();
            let c = cs.next()?;
            if cs.next().is_some() {
                return None;
            }
            KeyCode::Char(c.to_ascii_lowercase())
        }
    };
    Some(Key::new(code, mods))
}

/// Every key-shaped token in a footer row. Slash-command hints (`/help`,
/// `/clear`) and shell hints (`!ls -la`) are prose, not keys, and are
/// skipped by construction: a token is only a key if it parses.
fn key_tokens(footer: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in footer.split_whitespace() {
        let t = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '+');
        if t.is_empty() || t.starts_with('/') || t.starts_with('!') {
            continue;
        }
        if let Some(k) = parse_token(t) {
            let _ = k;
            out.push(t.to_string());
        }
    }
    out
}

fn editing_state(extended: bool) -> AppState {
    let mut state = AppState::new("test-model");
    state.ui_state.help_extended = extended;
    state.push_assistant("hello");
    state
}

#[test]
fn every_key_the_footer_advertises_is_actually_bound() {
    let keymap = nini_tui::keys::default_keymap();
    let mut unbound: Vec<String> = Vec::new();

    // Both hint sets, both widths: the wide and narrow branches are
    // separate literals, so a fix that only patches one still fails here.
    for extended in [true, false] {
        for w in [120u16, 60u16] {
            let footer = footer_text(&editing_state(extended), w, 24);
            for token in key_tokens(&footer) {
                let Some(key) = parse_token(&token) else {
                    continue;
                };
                if !keymap.iter().any(|b| b.key == key) {
                    unbound.push(format!("{token} (w={w}, extended={extended})"));
                }
            }
        }
    }

    assert!(
        unbound.is_empty(),
        "the footer advertises {} key(s) that `default_keymap()` does not \
         bind — pressing them does nothing at all:\n  {}",
        unbound.len(),
        unbound.join("\n  ")
    );
}

#[test]
fn the_ctrl_c_hint_names_what_ctrl_c_actually_does() {
    // The footer says `Ctrl+C quit`. `Ctrl+C` is bound to `Abort`, and
    // with no popup open `Abort` clears the input buffer. Advertising an
    // exit that is actually a destructive clear is the defect; the
    // invariant is that the hint must not name an action the key does
    // not perform.
    let keymap = nini_tui::keys::default_keymap();
    let footer = footer_text(&editing_state(false), 120, 24);

    let ctrl_c = Key::new(KeyCode::Char('c'), CTRL);
    let bound = keymap
        .iter()
        .find(|b| b.key == ctrl_c)
        .map(|b| b.action)
        .expect("Ctrl+C must be bound");

    let claims_quit = footer.contains("Ctrl+C")
        && footer.split_whitespace().any(|w| {
            let idx = footer.find(w).unwrap_or(0);
            idx > 0 && footer[..idx].contains("Ctrl+C")
        })
        && footer.contains("quit");

    assert!(
        !(claims_quit && bound != nini_tui::keys::KeyAction::Quit),
        "the footer advertises `Ctrl+C quit`, but Ctrl+C is bound to \
         {bound:?}. Pressing it clears the half-typed input instead of \
         exiting (runtime.rs, `Abort` arm: `else {{ state.input.clear() }}`)."
    );
}
