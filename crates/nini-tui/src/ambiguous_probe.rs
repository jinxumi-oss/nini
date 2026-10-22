//! Ask the terminal how wide it draws East-Asian **Ambiguous** characters.
//!
//! # The question
//!
//! `─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼`, `░ ▓ █`, `• ▸ ✓ ✗ ❯ ↑ ↓` and the braille
//! spinner are all ambiguous: one cell by the Unicode default, and also
//! the default of most modern terminals — but **two** in a terminal
//! configured for a Chinese, Japanese or Korean locale, which is what a
//! `wcwidth`-based terminal does when the locale says so.
//!
//! That difference is not cosmetic. A row of `cols` cells carrying *k*
//! ambiguous characters renders as `cols + k` columns in such a terminal:
//! the row overflows, the terminal folds it, and **every row below it
//! moves down one line**. A TUI frame becomes a wall of half-rules and
//! stray fragments — and the whole screen is shifted, so even the input
//! box and the cursor end up in the wrong place.
//!
//! # How the terminal is asked
//!
//! There is no capability query for this, but there is a measurement:
//!
//! 1. put the cursor at the start of a scratch row,
//! 2. write *n* copies of `─` (ambiguous, one cell by the Unicode
//!    default),
//! 3. ask where the cursor is (`CSI 6 n` — Device Status Report), which
//!    most terminals answer with `CSI row ; col R`.
//!
//! The answer is the number of columns the *terminal* gave those *n*
//! characters: `n` means narrow, `2n` means wide. A terminal that does
//! not answer within the timeout is narrow — which is the historical
//! behaviour and what every modern terminal does by default.
//!
//! The probe runs before the alternate screen is entered, on the last
//! row, and clears it afterwards. If the write or the read fails for any
//! reason, the caller is told "unknown" and falls back.

use std::io::{Read, Write};

/// The outcome of the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguousProbe {
    /// The terminal answered.
    Measured { wide: bool },
    /// The terminal did not answer (or the probe could not run).
    Unknown,
}

const PROBE_CHARS: usize = 8;
const TIMEOUT_MS: i32 = 120;

/// Measure the terminal's ambiguous-width behaviour.
///
/// `rows` is the terminal height; the probe uses the last row so it does
/// not disturb whatever is on screen.
pub fn detect(rows: u16) -> AmbiguousProbe {
    let row = rows.saturating_sub(1).max(1);
    let mut out = std::io::stdout();

    // Clear the row, write the probe, then ask where the cursor is.
    let mut write = || -> std::io::Result<()> {
        write!(out, "\x1b[{row};1H\x1b[2K")?;
        for _ in 0..PROBE_CHARS {
            out.write_all("\u{2500}".as_bytes())?;
        }
        out.write_all(b"\x1b[6n")?;
        out.flush()
    };
    if write().is_err() {
        return AmbiguousProbe::Unknown;
    }

    let reply = match read_cursor_report() {
        Some(r) => r,
        None => {
            let _ = clear_probe(row);
            return AmbiguousProbe::Unknown;
        }
    };
    let _ = clear_probe(row);

    // The column is 1-based; after `n` one-cell characters it is `n + 1`,
    // after `n` two-cell characters it is `2n + 1`.
    let expected_narrow = PROBE_CHARS as u32 + 1;
    match reply.col.cmp(&expected_narrow) {
        std::cmp::Ordering::Less => {
            // Fewer columns than the characters we wrote: the terminal
            // wrapped mid-row, which only happens when a character needs
            // more columns than we assumed.
            AmbiguousProbe::Measured { wide: true }
        }
        _ => AmbiguousProbe::Measured { wide: false },
    }
}

struct CursorReport {
    #[allow(dead_code)]
    row: u32,
    col: u32,
}

fn read_cursor_report() -> Option<CursorReport> {
    let mut buf = [0u8; 32];
    let mut filled = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(TIMEOUT_MS as u64);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if !wait_readable(remaining) {
            break;
        }
        match std::io::stdin().read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(_) => break,
        }
        let text = String::from_utf8_lossy(&buf[..filled]);
        if let Some(report) = parse_cursor_report(&text) {
            // Anything after the report belongs to the next read; the
            // caller gets the terminal back with the reply consumed.
            return Some(report);
        }
        if filled == buf.len() {
            break;
        }
    }
    None
}

fn parse_cursor_report(text: &str) -> Option<CursorReport> {
    let at = text.rfind("\u{1b}[")?;
    let body = &text[at + 2..];
    let end = body.find('R')?;
    let nums: Vec<u32> = body[..end]
        .split(';')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match nums.as_slice() {
        [row, col] => Some(CursorReport {
            row: *row,
            col: *col,
        }),
        [col] => Some(CursorReport { row: 1, col: *col }),
        _ => None,
    }
}

fn clear_probe(row: u16) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    write!(out, "\x1b[{row};1H\x1b[2K")?;
    out.flush()
}

/// Wait until stdin has something, or the timeout expires.
fn wait_readable(timeout: std::time::Duration) -> bool {
    // `poll` on fd 0. The alternative — a background thread with a read —
    // can steal a keystroke from the TUI that starts right after.
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: `pollfd` is a plain struct; nothing else touches it.
    let mut pfd = libc::pollfd {
        fd: 0,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pfd` is initialised above and outlives the call.
    let n = unsafe { libc::poll(&mut pfd, 1, ms) };
    n > 0
}
