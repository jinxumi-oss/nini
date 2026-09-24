//! v0.7 (M5b) Operations traits for remoteable / mockable tool backends.
//!
//! Tools that interact with the OS (process execution, file I/O) have
//! historically been hardcoded to `std::process` / `std::fs`. That
//! makes them:
//!
//!   * Impossible to mock in tests (no way to inject a fake command
//!     runner that returns canned stdout / exit codes).
//!   * Impossible to delegate to a remote executor (e.g. SSH into a
//!     build box, run inside a container, sandboxed via Landlock).
//!   * Impossible to audit centrally (every tool rolls its own
//!     process-group / killpg logic).
//!
//! `BashOperations` and `ReadOperations` extract the OS-touching
//! primitives behind traits. `BashRunner` (the sync shell runner
//! used by the TUI's `!cmd` passthrough) is the default impl of
//! `BashOperations`. `ReadTool` can adopt `ReadOperations` when it
//! wants to mock or sandbox reads.
//!
//! The default impl lives in nini-tools because most tools depend
//! on it; extensions / remote backends would live in their own
//! crate and implement the traits against their own transport.

use std::io;
use std::path::{Path, PathBuf};

/// v0.7 (M5b) — outcome of a single shell execution. Mirrors the
/// shape of the nini-tui `BashResult` so the migration is wire-
/// compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutcome {
    /// Decoded UTF-8 stdout.
    pub output: String,
    /// Decoded UTF-8 stderr.
    pub stderr: String,
    /// Exit code, or `None` if the child was killed by a signal /
    /// timeout.
    pub exit_code: Option<i32>,
    /// Wall-clock duration, in milliseconds.
    pub duration_ms: u64,
    /// True when `exit_code == Some(0)`.
    pub ok: bool,
    /// True when the run exceeded its timeout and was killed.
    pub timed_out: bool,
}

/// v0.7 (M5b) — trait abstracting shell execution. The default
/// impl (`DefaultBashOperations`) wraps `BashRunner` and provides
/// the same sync, blocking semantics the TUI's `!cmd` already
/// relied on. Extensions / remote backends can swap in their own
/// impl (SSH, sandbox, mock) by replacing the registry field.
pub trait BashOperations: Send + Sync {
    /// Run `cmd` synchronously in `cwd`. Blocks the caller for up
    /// to `timeout_ms` milliseconds. Pass `0` to disable the
    /// timeout (use with care — the TUI will hang).
    fn exec(
        &self,
        cmd: &str,
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<ExecOutcome>;

    /// Resolve `name` on `$PATH`. Returns the absolute path to
    /// the executable, or `None` if it isn't found. Used by the
    /// bash tool's path safety check (see M2) and by extension
    /// hooks that want to verify a binary exists before invoking
    /// it.
    fn which(&self, name: &str) -> Option<PathBuf>;

    /// Send SIGTERM then SIGKILL to the entire process group
    /// rooted at `pid`. On non-Unix this is a no-op. Used to
    /// forcibly abort a long-running exec from the outside
    /// (e.g. the bash tool's before/after hook pipeline).
    fn kill_pg(&self, pid: u32);
}

/// v0.7 (M5b) — default `BashOperations` impl that delegates to
/// the existing `BashRunner`. The runner already provides all the
/// behavior we want (setsid + killpg on Unix, best-effort on
/// Windows) so the trait is just a thin wrapper.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultBashOperations;

impl BashOperations for DefaultBashOperations {
    fn exec(
        &self,
        cmd: &str,
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<ExecOutcome> {
        let mut runner = crate::bash_runner::BashRunner::new();
        let r = runner.run_blocking_with_timeout(cmd, cwd, timeout_ms);
        Ok(ExecOutcome {
            output: r.output,
            stderr: r.stderr,
            exit_code: r.exit_code,
            duration_ms: r.duration_ms,
            ok: r.ok,
            timed_out: r.timed_out,
        })
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        bash_which(name)
    }

    fn kill_pg(&self, _pid: u32) {
        // BashRunner already does its own killpg internally on
        // timeout. The trait-level `kill_pg` is for callers that
        // want to abort an exec from the outside (e.g. an
        // extension's before-execute hook that detects a
        // forbidden command). For the default impl, we don't
        // track enough state to kill externally; callers should
        // rely on the per-run timeout instead. We log and
        // return — this is a no-op for the default backend.
        eprintln!(
            "[nini] DefaultBashOperations::kill_pg({_pid}) is a no-op; \
             rely on per-run timeout instead"
        );
    }
}

/// v0.7 (M5b) — small `which` helper. We avoid pulling in the
/// `which` crate just for this.
pub fn bash_which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return if p.exists() { Some(p.to_path_buf()) } else { None };
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            for ext in ["exe", "bat", "cmd"] {
                let c = dir.join(format!("{name}.{ext}"));
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// v0.7 (M5b) — trait abstracting file reads. The default impl
/// uses `std::fs`. Extensions can sandbox this (Landlock,
/// allow-list) or mock it for testing.
pub trait ReadOperations: Send + Sync {
    /// Read the file at `path` and return its UTF-8 contents.
    /// The implementation decides whether to fail loudly on
    /// non-UTF-8 bytes (lossy vs strict); the default returns
    /// lossy-decoded text + a flag.
    fn read(&self, path: &Path) -> io::Result<String>;

    /// Stat the file at `path`. Returns the byte size or an error
    /// if the path doesn't exist. Useful for the read tool's
    /// "file too large" guard.
    fn stat_size(&self, path: &Path) -> io::Result<u64>;
}

/// v0.7 (M5b) — default `ReadOperations` impl using std::fs.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultReadOperations;

impl ReadOperations for DefaultReadOperations {
    fn read(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn stat_size(&self, path: &Path) -> io::Result<u64> {
        let metadata = std::fs::metadata(path)?;
        Ok(metadata.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_which_finds_absolute_paths() {
        #[cfg(unix)]
        assert!(bash_which("/bin/sh").is_some());
        #[cfg(not(unix))]
        let _ = bash_which("/bin/sh");
    }

    #[test]
    fn bash_which_returns_none_for_nonexistent() {
        assert!(bash_which("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn default_bash_operations_can_echo() {
        let ops = DefaultBashOperations;
        let cwd = std::path::PathBuf::from("/tmp");
        let outcome = ops
            .exec("echo HELLO", &cwd, 5000)
            .expect("exec should succeed");
        assert!(outcome.output.contains("HELLO"));
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.ok);
        assert!(!outcome.timed_out);
    }

    #[test]
    fn default_bash_operations_captures_nonzero_exit() {
        let ops = DefaultBashOperations;
        let cwd = std::path::PathBuf::from("/tmp");
        let outcome = ops
            .exec("exit 7", &cwd, 5000)
            .expect("exec should succeed");
        assert_eq!(outcome.exit_code, Some(7));
        assert!(!outcome.ok);
    }

    #[test]
    fn default_bash_operations_separates_stderr() {
        let ops = DefaultBashOperations;
        let cwd = std::path::PathBuf::from("/tmp");
        let outcome = ops
            .exec("echo OOPS >&2", &cwd, 5000)
            .expect("exec should succeed");
        assert!(outcome.stderr.contains("OOPS"));
        assert!(outcome.output.is_empty());
    }

    #[test]
    fn default_bash_operations_timeout_kills_command() {
        let ops = DefaultBashOperations;
        let cwd = std::path::PathBuf::from("/tmp");
        let outcome = ops
            .exec("sleep 30", &cwd, 500)
            .expect("exec should succeed");
        assert!(outcome.timed_out, "expected timed_out, got: {outcome:?}");
        assert!(outcome.duration_ms < 5000);
    }

    #[test]
    fn default_read_operations_reads_a_file() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hello.txt");
        {
            let mut f = std::fs::File::create(&path).expect("create");
            write!(f, "hello world").expect("write");
        }
        let ops = DefaultReadOperations;
        let content = ops.read(&path).expect("read");
        assert_eq!(content, "hello world");
        let size = ops.stat_size(&path).expect("stat");
        assert_eq!(size, 11);
    }

    #[test]
    fn default_read_operations_errors_on_missing() {
        let ops = DefaultReadOperations;
        let bogus = std::path::PathBuf::from("/no/such/path/here/xyzzy");
        assert!(ops.read(&bogus).is_err());
        assert!(ops.stat_size(&bogus).is_err());
    }

    /// MOCK `BashOperations` — used to verify that tools can be
    /// parameterized with an alternative backend. This is the
    /// primary value of the trait: tests don't need a real shell.
    #[derive(Debug, Clone)]
    struct MockBash {
        canned_output: String,
        canned_exit: Option<i32>,
    }
    impl BashOperations for MockBash {
        fn exec(
            &self,
            _cmd: &str,
            _cwd: &Path,
            _timeout_ms: u64,
        ) -> io::Result<ExecOutcome> {
            Ok(ExecOutcome {
                output: self.canned_output.clone(),
                stderr: String::new(),
                exit_code: self.canned_exit,
                duration_ms: 1,
                ok: self.canned_exit == Some(0),
                timed_out: false,
            })
        }
        fn which(&self, _name: &str) -> Option<PathBuf> { None }
        fn kill_pg(&self, _pid: u32) {}
    }

    /// MOCK `ReadOperations` — used to verify the read tool can be
    /// backed by something other than `std::fs`. Tools that take
    /// an `Arc<dyn ReadOperations>` can be parameterized.
    #[derive(Debug, Clone)]
    struct MockRead {
        canned: String,
    }
    impl ReadOperations for MockRead {
        fn read(&self, _path: &Path) -> io::Result<String> {
            Ok(self.canned.clone())
        }
        fn stat_size(&self, _path: &Path) -> io::Result<u64> {
            Ok(self.canned.len() as u64)
        }
    }

    #[test]
    fn mock_bash_operations_returns_canned_response() {
        let mock = MockBash {
            canned_output: "mocked stdout\n".into(),
            canned_exit: Some(0),
        };
        let cwd = std::path::PathBuf::from("/tmp");
        let outcome = mock.exec("anything", &cwd, 1000).expect("exec");
        assert_eq!(outcome.output, "mocked stdout\n");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.ok);
    }

    #[test]
    fn mock_read_operations_returns_canned_content() {
        let mock = MockRead { canned: "synthetic content".into() };
        let path = std::path::PathBuf::from("/tmp/anything");
        assert_eq!(mock.read(&path).unwrap(), "synthetic content");
        assert_eq!(mock.stat_size(&path).unwrap(), "synthetic content".len() as u64);
    }

    #[test]
    fn kill_pg_default_impl_is_a_logged_no_op() {
        // The default impl logs + returns; we just verify it
        // doesn't panic and stays on the happy path.
        let ops = DefaultBashOperations;
        ops.kill_pg(12345);
    }
}