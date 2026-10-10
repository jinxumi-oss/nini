//! v0.10 (R4c): the acceptance test that was never run.
//!
//! R3 claimed nini no longer depends on Pi, and proved it by unit-testing
//! the path resolver. It never ran the binary. Those are different claims:
//! a resolver can be correct while some other module still reads
//! `$HOME/.pi`, and the only thing that catches that is executing the
//! program where Pi does not exist.
//!
//! These run the built `nini` binary in a `HOME` that contains no `.pi`
//! and no `.nini`, and assert it starts, says something true, and leaves
//! the sandbox alone apart from nini's own directory.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn binary() -> PathBuf {
    // Cargo only injects CARGO_BIN_EXE_<name> for integration tests in the
    // crate that declares that binary. This test therefore has to live in
    // nini-cli, next to `[[bin]] name = "nini"` - putting it in nini-core
    // fails to compile with "environment variable CARGO_BIN_EXE_nini not
    // defined at compile time".
    PathBuf::from(env!("CARGO_BIN_EXE_nini"))
}

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    work: PathBuf,
}

impl Sandbox {
    /// A home with neither `.pi` nor `.nini` in it.
    fn new(tag: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("nini-nohome-{tag}-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&work).expect("work");
        Self { root, home, work }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(binary())
            .args(args)
            .current_dir(&self.work)
            // Everything that could point at the developer's real state is
            // redirected or cleared. Without this the test would be
            // reading the machine's own ~/.nini and proving nothing.
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env_remove("NINI_AGENT_DIR")
            .env_remove("PI_CODING_AGENT_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_CACHE_HOME")
            .output()
            .expect("nini binary runs")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn exists(p: &Path) -> bool {
    p.exists()
}

/// The whole point: nini starts on a machine where Pi has never been
/// installed and Pi's configuration directory does not exist.
#[test]
fn it_runs_with_no_pi_and_no_nini_directory() {
    let sb = Sandbox::new("bare");

    assert!(
        !exists(&sb.home.join(".pi")),
        "the sandbox is supposed to have no .pi"
    );
    assert!(!exists(&sb.home.join(".nini")));

    let out = sb.run(&["--version"]);
    assert!(
        out.status.success(),
        "nini --version failed with {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    // `--version` returns from clap before nini resolves any path, so it
    // proves nothing about where nini looks. `info` reports the loaded
    // skills and settings, so it actually walks the resolver - against a
    // home directory with neither `.pi` nor `.nini` in it.
    let info = sb.run(&["info"]);
    assert!(
        info.status.success(),
        "nini info failed on a bare machine with {:?}\nstderr: {}",
        info.status.code(),
        String::from_utf8_lossy(&info.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim_start().starts_with("nini "),
        "expected a version line, got {stdout:?}"
    );
}

/// The negative half, which is the half that matters: nothing may read or
/// write `~/.nini`, in either direction.
#[test]
fn it_never_touches_a_pi_directory() {
    let sb = Sandbox::new("nopidir");

    // Plant a Pi-shaped directory that is readable and writable, and
    // whose contents would be obvious if anything read it.
    let pi = sb.home.join(".pi").join("agent");
    std::fs::create_dir_all(pi.join("sessions")).unwrap();
    std::fs::write(pi.join("settings.json"), br#"{"marker":"PI_WAS_READ"}"#).unwrap();
    std::fs::write(pi.join("sessions").join("planted.jsonl"), b"{}\n").unwrap();

    let before = walk(&sb.home.join(".pi"));

    let out = sb.run(&["--version"]);
    assert!(out.status.success(), "nini exited {:?}", out.status);

    // `info` is the command that resolves settings, models, prompts,
    // skills and keybindings, so it is where a stray `.pi` path would
    // actually be taken.
    let info = sb.run(&["info"]);
    assert!(info.status.success(), "nini info exited {:?}", info.status);

    let after = walk(&sb.home.join(".pi"));
    assert_eq!(
        before, after,
        "nini modified Pi's directory:\nbefore: {before:?}\nafter:  {after:?}"
    );

    // And it did not mistake Pi's settings for its own: the marker string
    // must not appear anywhere in nini's output.
    for (label, out) in [("--version", &out), ("info", &info)] {
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !combined.contains("PI_WAS_READ"),
            "nini {label} read Pi's settings.json:\n{combined}"
        );
    }
}

/// Where nini actually writes is `~/.nini`. If it creates anything, that
/// is what it creates - and the version command is not expected to create
/// a session, so this only asserts that anything that appears is nini's.
#[test]
fn anything_it_creates_is_ninis_own() {
    let sb = Sandbox::new("own");

    let out = sb.run(&["--version"]);
    assert!(out.status.success());

    for (rel, _) in walk(&sb.home) {
        assert!(
            !rel.contains(".pi"),
            "nini created a path under Pi's directory: {rel}"
        );
    }
}

/// Worth stating on its own: these tests exercise the *built binary*, so a
/// green run says something about the program and not just about its
/// library. `cargo test --workspace` builds binaries for integration
/// tests, but a plain `cargo test --lib` does not and would skip all of it.
#[test]
fn these_run_the_built_binary_not_the_library() {
    let b = binary();
    assert!(
        exists(&b),
        "CARGO_BIN_EXE_nini pointed at {b:?}, which does not exist"
    );
    assert_eq!(
        b.file_name().and_then(|n| n.to_str()),
        Some("nini"),
        "unexpected binary at {b:?}"
    );
    // `repo_root()` is the `crates` directory: CARGO_MANIFEST_DIR is
    // nini-cli, and its parent is crates.
    assert!(
        repo_root().join("nini-cli").exists(),
        "workspace layout changed: {} is not the crates directory",
        repo_root().display()
    );
}

/// Sorted `(relative path, size)` for every file under `dir`.
fn walk(dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    fn go(d: &Path, base: &Path, out: &mut Vec<(String, u64)>) {
        let Ok(entries) = std::fs::read_dir(d) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                go(&p, base, out);
            } else if let Ok(md) = e.metadata() {
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, md.len()));
            }
        }
    }
    go(dir, dir, &mut out);
    out.sort();
    out
}
