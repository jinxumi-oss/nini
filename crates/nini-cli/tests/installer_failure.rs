//! The installer must report the failure that actually happened.
//!
//! # The defect
//!
//! `install.sh` kept its temporary directory in a **local** variable:
//!
//! ```bash
//! download_and_install() {
//!   local tmpdir
//!   tmpdir="$(mktemp -d)"
//!   trap 'rm -rf "$tmpdir"' EXIT
//! ```
//!
//! When the download failed, `set -e` exited **from inside the function** —
//! and the function's local scope (with it, that `tmpdir`) was already gone
//! by the time the trap fired. The trap then hit `set -u` and died with
//!
//! ```text
//! tmpdir: unbound variable
//! ```
//!
//! so the user was told *that* instead of *what went wrong* (404, no
//! network, DNS failure), the exit code was the trap's, and the temporary
//! directory was **leaked** — `rm -rf` never ran.
//!
//! # What this file pins
//!
//! The installer is the only path for a user who did not clone the repo, so
//! its failure output is the only diagnostic such a user gets. This test
//! runs the **real script** with a `curl` that always fails, and asserts:
//!
//! 1. the exit code is the failure's own (1), not the trap's;
//! 2. stderr names the **download** and the **URL**;
//! 3. stderr does **not** contain `unbound variable` (the trap must never
//!    fire on an unbound name);
//! 4. no temporary directory is left behind.
//!
//! The test is hermetic: the fake `curl` is a shell script in a temporary
//! directory placed first on `PATH`, so no network is touched.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A `curl` that always fails, the way a 404 or an offline machine does.
fn failing_curl(dir: &Path) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("create bin dir");
    let script =
        "#!/bin/sh\necho 'curl: (22) The requested URL returned error: 404' >&2\nexit 22\n";
    std::fs::write(bin.join("curl"), script).expect("write fake curl");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(bin.join("curl"))
            .expect("stat fake curl")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(bin.join("curl"), perms).expect("chmod fake curl");
    }
}

/// Run `install.sh` with a failing `curl`; return (exit code, stdout, stderr).
fn run_installer_with_failing_download(repo: &Path, tmp: &Path) -> (Option<i32>, String, String) {
    let bin = tmp.join("bin");
    failing_curl(tmp);
    // `TMPDIR` is where `mktemp -d` puts its directory — pointing it at a
    // directory we own is how the "nothing was left behind" assertion
    // below becomes checkable.
    let tmproot = tmp.join("tmpdir");
    std::fs::create_dir_all(&tmproot).expect("create TMPDIR");

    let out = Command::new("bash")
        .arg(repo.join("install.sh"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("NINI_VERSION", "v0.0.0-not-a-real-release")
        .env("NINI_INSTALL_DIR", tmp.join("bin-install"))
        .env("NINI_NO_MODIFY_PATH", "1")
        .env("TMPDIR", &tmproot)
        .output()
        .expect("install.sh runs");

    let leaked: Vec<PathBuf> = std::fs::read_dir(&tmproot)
        .expect("read TMPDIR")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert!(
        leaked.is_empty(),
        "the installer leaked its temporary directory: {leaked:?}"
    );

    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/nini-cli has a parent and a grandparent")
        .to_path_buf()
}

#[test]
fn a_failed_download_reports_the_download_and_leaves_nothing_behind() {
    let repo = repo_root();
    let tmp = tempfile::tempdir().expect("tempdir");

    let (code, stdout, stderr) = run_installer_with_failing_download(&repo, tmp.path());

    assert_eq!(
        code,
        Some(1),
        "the exit code must be the failure's own (1), not the trap's; \
         stdout was {stdout:?}, stderr was {stderr:?}"
    );
    assert!(
        stderr.contains("download failed"),
        "stderr must name the failure that happened; stderr was {stderr:?}"
    );
    assert!(
        stderr.contains("github.com/jinxumi-oss/nini"),
        "stderr must name the URL it tried; stderr was {stderr:?}"
    );
    assert!(
        !stderr.contains("unbound variable"),
        "the cleanup trap must never fire on an unbound name — that is the \
         defect this test pins; stderr was {stderr:?}"
    );
}

#[test]
fn the_temporary_directory_is_global_so_the_trap_can_always_clean_up() {
    // The source-level half of the same defect: a *local* temp directory
    // is gone by the time the trap fires, so the trap can never clean up
    // (and `set -u` kills it before it tries).
    let src = std::fs::read_to_string(repo_root().join("install.sh")).expect("read install.sh");

    assert!(
        src.contains("NINI_TMPDIR="),
        "the temporary directory must live in a global (`NINI_TMPDIR`), not \
         in a function-local, so it outlives the function that made it"
    );
    assert!(
        !src.contains("local tmpdir"),
        "install.sh must not declare a function-local temp directory: it is \
         gone when the EXIT trap fires"
    );
    assert!(
        src.contains("cleanup_tmpdir"),
        "the trap must be a named function that checks the variable before \
         using it (an unbound name under `set -u` kills the trap)"
    );
}
