//! Startup diagnostics, D1: the tmux keyboard-setup check, and the
//! (shrunk) resource-diagnostics formatter.
//!
//! # 1. The tmux keyboard-setup check
//!
//! Pi 0.85.1 checks the tmux setup at startup
//! (`references/pi-0.85.1/src/ui/interactive-mode.ts:1161`, called from
//! `:1069`) and shows a warning if the terminal multiplexer is going to
//! swallow a key nini depends on:
//!
//! ```text
//! tmux extended-keys is off. Modified Enter keys may not work.
//! Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.
//!
//! tmux extended-keys-format is xterm. Pi works best with csi-u.
//! Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.
//! ```
//!
//! This is not a cosmetic warning for nini. nini binds **Alt+Enter** to
//! `followUp` (the A4 gap: one pending queue, split into steering and
//! follow-up), and a modified Enter is exactly the key tmux eats unless
//! `extended-keys` is on. The user presses Alt+Enter, nothing happens,
//! and the "follow up" they typed is simply lost — a silent data loss
//! with a one-line fix in `~/.tmux.conf`.
//!
//! Pi's rules, reproduced here exactly:
//!
//! * not inside tmux → no warning (the check does not apply);
//! * `tmux show -gv extended-keys` could not be answered (tmux missing,
//!   sandboxed, timed out) → **no warning** — Pi explicitly does not
//!   warn about a setup it could not read;
//! * `extended-keys` is neither `on` nor `always` → the first warning;
//! * `extended-keys-format` is `xterm` → the second warning;
//! * otherwise → the setup is fine.
//!
//! Pi queries tmux with a 2-second timeout
//! (`interactive-mode.ts:1172`); the same budget is used here
//! ([`TmuxCli::DEFAULT_TIMEOUT`]). The query is behind a trait
//! ([`TmuxOptionQuery`]) so the test can answer it without tmux being
//! installed, and the timeout itself is tested against a command that
//! really does hang.
//!
//! # 2. The resource-diagnostics formatter (Pi's `formatDiagnostics`,
//! shrunk)
//!
//! Pi formats the resources it loaded — and the ones it could not load,
//! or had to skip because another resource of the same name won — into
//! readable text (`references/pi-0.85.1/src/ui/interactive-mode.ts:1589`).
//! Pi's version covers four resource kinds (skills, prompts, themes,
//! extensions); nini has no extension runtime, so the shrunk version
//! covers **skills and prompts** only, which are the two nini actually
//! loads at startup (`nini-core/src/skills.rs`,
//! `nini-core/src/prompt_template.rs`).
//!
//! Pi's grouping rule (`interactive-mode.ts:1592-1602`): diagnostics of
//! type `collision` are grouped **by name** into a map, and every other
//! diagnostic is kept aside; the collisions are printed first (one block
//! per name: the winning path with a ✓, then every skipped path with a
//! ✗ and `(skipped)`), then the rest, one path line and one message
//! line each.
//!
//! nini's version keeps that shape, with one deliberate deviation: the
//! group key is `(kind, name)` rather than `name` alone, because skills
//! and prompts are **separate namespaces** — a skill named `review` and a
//! prompt template named `review` do not collide with each other, and
//! grouping them under one heading would tell the user a falsehood; and
//! the header carries the kind, because two groups can otherwise carry
//! the same name.
//!
//! The output is plain text (Pi colours it; nini's formatter does not,
//! because the caller decides how to present it).

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The verdict of the tmux keyboard-setup check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmuxKeyboardStatus {
    /// Not running inside tmux: the check does not apply.
    NotApplicable,
    /// tmux is set up the way nini needs: modified Enter keys work.
    Ok,
    /// `extended-keys` is off (or unset): modified Enter keys may not
    /// work. This is Pi's first warning.
    ExtendedKeysOff,
    /// `extended-keys` is on but `extended-keys-format` is `xterm`: nini
    /// works best with `csi-u`. This is Pi's second warning.
    XtermFormat,
    /// tmux could not be queried (missing, sandboxed, timed out). Pi does
    /// not warn about a setup it could not read, and neither does nini.
    Unknown,
}

/// Classify what tmux reported.
///
/// * `in_tmux` — whether the process is running inside tmux at all.
/// * `extended_keys` — `tmux show -gv extended-keys`; `None` means the
///   query could not be answered (Pi: `extendedKeys === undefined`).
/// * `extended_keys_format` — `tmux show -gv extended-keys-format`;
///   `None` means the same.
pub fn classify(
    in_tmux: bool,
    extended_keys: Option<&str>,
    extended_keys_format: Option<&str>,
) -> TmuxKeyboardStatus {
    if !in_tmux {
        return TmuxKeyboardStatus::NotApplicable;
    }
    let Some(keys) = extended_keys else {
        // Could not read the setting: do not warn about a setup we did
        // not see. Pi does the same (`interactive-mode.ts:1195`).
        return TmuxKeyboardStatus::Unknown;
    };
    let keys = keys.trim();
    if keys != "on" && keys != "always" {
        return TmuxKeyboardStatus::ExtendedKeysOff;
    }
    if extended_keys_format.map(str::trim) == Some("xterm") {
        return TmuxKeyboardStatus::XtermFormat;
    }
    TmuxKeyboardStatus::Ok
}

/// The warning to show for a status, if any.
///
/// The wording is Pi's (`interactive-mode.ts:1198` and `:1203`), with
/// "Pi" replaced by "nini" in the second one — the sentence is about the
/// product that has to read the key.
pub fn warning_message(status: TmuxKeyboardStatus) -> Option<&'static str> {
    match status {
        TmuxKeyboardStatus::ExtendedKeysOff => Some(
            "tmux extended-keys is off. Modified Enter keys may not work. \
             Add `set -g extended-keys on` to ~/.tmux.conf and restart tmux.",
        ),
        TmuxKeyboardStatus::XtermFormat => Some(
            "tmux extended-keys-format is xterm. nini works best with csi-u. \
             Add `set -g extended-keys-format csi-u` to ~/.tmux.conf and restart tmux.",
        ),
        TmuxKeyboardStatus::NotApplicable
        | TmuxKeyboardStatus::Ok
        | TmuxKeyboardStatus::Unknown => None,
    }
}

/// A source of tmux option values.
///
/// The seam that keeps the test from needing tmux: a fake implementation
/// answers with whatever the test wants, including "the command hung"
/// (`None`).
pub trait TmuxOptionQuery {
    /// The value of `tmux show -gv <option>`, or `None` if tmux could not
    /// be asked (not installed, non-zero exit, timed out).
    fn show_option(&self, option: &str) -> Option<String>;
}

/// The real tmux query: `tmux show -gv <option>`, with a timeout.
#[derive(Debug, Clone)]
pub struct TmuxCli {
    program: OsString,
    timeout: Duration,
}

impl TmuxCli {
    /// Pi's budget for the query: 2 seconds
    /// (`references/pi-0.85.1/src/ui/interactive-mode.ts:1172`).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

    /// The real `tmux` binary, with Pi's timeout.
    pub fn new() -> Self {
        Self::with_program("tmux", Self::DEFAULT_TIMEOUT)
    }

    /// A different binary (or a different timeout) — for tests, and for
    /// the machine where tmux is called something else.
    pub fn with_program(program: impl Into<OsString>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
        }
    }

    /// The timeout this query gives a command.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for TmuxCli {
    fn default() -> Self {
        Self::new()
    }
}

impl TmuxOptionQuery for TmuxCli {
    fn show_option(&self, option: &str) -> Option<String> {
        run_with_timeout(&self.program, &["show", "-gv", option], self.timeout)
    }
}

/// Run a command and return its trimmed stdout, or `None` if it failed,
/// timed out, or printed nothing usable.
///
/// The timeout is real: the child is killed when the budget runs out, so
/// a hung `tmux` cannot hang the TUI's startup. Pi does the same
/// (`interactive-mode.ts:1172-1175`).
///
/// `Some("")` is a legitimate answer — tmux exits 0 and prints nothing
/// when the option is unset, and Pi treats that as "the option is not
/// `on`" rather than as "the query failed".
pub fn run_with_timeout(
    program: impl AsRef<OsStr>,
    args: &[&str],
    timeout: Duration,
) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    // The child's stdout is read **while it runs**.
    //
    // A pipe has a finite buffer (64 KiB on Linux); a child that writes
    // more than that blocks in `write()` and never exits. The previous
    // version waited for the child to exit *first* and only then read the
    // pipe — so a chatty child would hang until the timeout killed it, and
    // the timeout would report "could not ask tmux" for a question that had
    // been answered. (Pi streams the child's output as it arrives —
    // `interactive-mode.ts:1170-1180` — so this is where nini should have
    // looked too.)
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = sender.send(String::from_utf8_lossy(&buf).to_string());
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }?;

    // The reader thread ends when the pipe closes (the child is gone).
    let out = receiver
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();
    let _ = reader.join();

    if !status.success() {
        return None;
    }
    Some(out.trim().to_string())
}

/// Run the whole check: query tmux, classify the answer.
///
/// `in_tmux` is passed in rather than read from the environment, so the
/// test can say "not in tmux" without unsetting `TMUX` for the process.
pub fn check_tmux_keyboard_setup<Q>(in_tmux: bool, query: &Q) -> TmuxKeyboardStatus
where
    Q: TmuxOptionQuery + ?Sized,
{
    if !in_tmux {
        return TmuxKeyboardStatus::NotApplicable;
    }
    let extended_keys = query.show_option("extended-keys");
    let extended_keys_format = query.show_option("extended-keys-format");
    classify(
        in_tmux,
        extended_keys.as_deref(),
        extended_keys_format.as_deref(),
    )
}

/// The check as the runtime would run it: is this process inside tmux,
/// and what does tmux say?
pub fn check_tmux_keyboard_setup_env() -> TmuxKeyboardStatus {
    let in_tmux = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty());
    check_tmux_keyboard_setup(in_tmux, &TmuxCli::new())
}

// ---------------------------------------------------------------------------
// 2. Resource diagnostics (Pi's `formatDiagnostics`, shrunk)
// ---------------------------------------------------------------------------

/// The resource kinds nini loads at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// A skill (`SKILL.md`).
    Skill,
    /// A prompt template (`.md` with frontmatter).
    Prompt,
}

impl ResourceKind {
    /// The kind's name, for callers that want to label it.
    pub fn label(self) -> &'static str {
        match self {
            ResourceKind::Skill => "skill",
            ResourceKind::Prompt => "prompt",
        }
    }
}

/// How bad a load failure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    /// The resource could not be read at all.
    Error,
    /// The resource was read, but something about it is off.
    Warning,
}

/// One thing that happened while loading a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceDiagnostic {
    /// Two resources have the same name: `winner` was loaded, `loser` was
    /// skipped.
    Collision {
        /// Which namespace the collision happened in.
        kind: ResourceKind,
        /// The contested name.
        name: String,
        /// The path that won.
        winner: PathBuf,
        /// The path that was skipped.
        loser: PathBuf,
    },
    /// A resource that could not be loaded.
    Load {
        /// Which kind of resource it was.
        kind: ResourceKind,
        /// How bad it is.
        severity: DiagnosticSeverity,
        /// The file it came from, if it is known.
        path: Option<PathBuf>,
        /// What went wrong.
        message: String,
    },
}

/// Format resource diagnostics into readable text.
///
/// Pi's shape (`references/pi-0.85.1/src/ui/interactive-mode.ts:1589`),
/// without the colours:
///
/// ```text
///   "review" collision (skill):
///     ✓ /home/u/.nini/agent/skills/review/SKILL.md
///     ✗ /home/u/.claude/skills/review/SKILL.md (skipped)
///   /home/u/.nini/agent/prompts/broken.md
///     parse failed: frontmatter is not a YAML mapping
/// ```
///
/// The rules, in order:
/// 1. collisions are grouped by `(kind, name)` and printed **first**, one
///    block per group: the header, the winning path with a `✓`, then
///    every skipped path with a `✗` and `(skipped)`;
/// 2. the other diagnostics follow, in the order they were reported: the
///    path on one line (when there is one) and the message on the next.
///
/// An empty input produces an empty string — Pi's `lines.join("\n")` of
/// no lines is `""`, and a caller that shows an empty block is showing
/// nothing, which is right.
pub fn format_diagnostics(diagnostics: &[ResourceDiagnostic]) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Group the collisions by (kind, name), keeping the order in which
    // the names were first seen — Pi's `Map` is insertion-ordered, and
    // the order the user sees their resources in is the order they were
    // loaded in.
    let mut groups: Vec<(ResourceKind, String, Vec<&ResourceDiagnostic>)> = Vec::new();
    let mut others: Vec<&ResourceDiagnostic> = Vec::new();
    for d in diagnostics {
        match d {
            ResourceDiagnostic::Collision { kind, name, .. } => {
                match groups.iter_mut().find(|(k, n, _)| *k == *kind && n == name) {
                    Some((_, _, list)) => list.push(d),
                    None => groups.push((*kind, name.clone(), vec![d])),
                }
            }
            ResourceDiagnostic::Load { .. } => others.push(d),
        }
    }

    for (kind, name, list) in groups {
        // The kind is in the header because nini's two namespaces can
        // hold the same name: a skill called `review` and a prompt
        // template called `review` are two different groups, and two
        // identical headers would not say which is which.
        lines.push(format!("  \"{name}\" collision ({}):", kind.label()));
        // The winner is the path the first diagnostic in the group names:
        // every diagnostic for a name agrees on it.
        if let Some(ResourceDiagnostic::Collision { winner, .. }) = list.first() {
            lines.push(format!("    \u{2713} {}", winner.display()));
        }
        for d in list {
            if let ResourceDiagnostic::Collision { loser, .. } = d {
                lines.push(format!("    \u{2717} {} (skipped)", loser.display()));
            }
        }
    }

    for d in others {
        if let ResourceDiagnostic::Load { path, message, .. } = d {
            match path {
                Some(p) => {
                    lines.push(format!("  {}", p.display()));
                    lines.push(format!("    {message}"));
                }
                None => lines.push(format!("  {message}")),
            }
        }
    }

    lines.join("\n")
}
