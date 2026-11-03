//! The keymap parity oracle must be able to **fail**.
//!
//! # Why this file exists
//!
//! `scripts/keymap-parity.py` is the oracle that compares Pi 0.85.1's
//! binding inventory (`references/pi-0.85.1/src/ui/keybindings.ts` +
//! `src/tui/keybindings.ts`) with nini's own keymap
//! (`crates/nini-tui/src/keys.rs`).
//!
//! An oracle that cannot fail proves nothing. The screen-level oracle
//! (`scripts/tui-oracle.py`) is in exactly that state today — it is
//! skipped by the gate with the note "extractor too noisy", and
//! `docs/invariant-audit.md` §5.4 records that it mis-pairs box-drawing
//! characters as key names. A new oracle that repeats that fate is worse
//! than no oracle: it would sit in the gate looking green.
//!
//! So this file is the **outside guard**: it copies the tree the oracle
//! reads into a scratch directory, breaks it in four different ways, and
//! asserts the oracle goes red **each time** — and that it is green on
//! the real tree.
//!
//! # The four breakages
//!
//! | # | breakage | what must go red |
//! |---|----------|-------------------|
//! | 1 | delete one `map` row | "Pi 的 binding id … 没有登记" — a Pi key nobody adjudicated |
//! | 2 | repoint one pairing at a different nini action | the semantic class no longer matches |
//! | 3 | rebind a key in nini's own keymap | "keys=same … 没有交集" — an accidental rebind |
//! | 4 | delete one `divergences` entry | a known conflict becomes an unregistered fork |
//!
//! Every mutation is applied to a **copy**, never to the working tree.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root, from this crate's manifest.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/nini-tui has a parent and a grandparent")
        .to_path_buf()
}

/// The files the oracle reads. Copying only these keeps the scratch tree
/// small (the whole Pi snapshot is 1.6 MB of files the oracle never opens).
const NEEDED: &[&str] = &[
    "scripts/keymap-parity.py",
    "scripts/keymap-parity.json",
    "crates/nini-tui/src/keys.rs",
    "crates/nini-tui/src/commands/dispatch.rs",
    "references/pi-0.85.1/src/ui/keybindings.ts",
    "references/pi-0.85.1/src/tui/keybindings.ts",
    "references/pi-0.85.1/src/ui/interactive-mode.ts",
];

/// A scratch copy of everything the oracle reads.
fn scratch(root: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nini-keymap-oracle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    for rel in NEEDED {
        let src = root.join(rel);
        let dst = dir.join(rel);
        std::fs::create_dir_all(dst.parent().expect("a parent")).expect("mkdir");
        std::fs::copy(&src, &dst).expect("copy into the scratch tree");
    }
    dir
}

/// Run the oracle against `root`; return (exit code, stderr+stdout).
fn run_oracle(root: &Path) -> (Option<i32>, String) {
    let out = Command::new("python3")
        .arg(root.join("scripts/keymap-parity.py"))
        .env("NINI_KEYMAP_ROOT", root)
        .current_dir(root)
        .output()
        .expect("python3 runs the oracle");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code(), text)
}

fn contract_of(root: &Path) -> String {
    std::fs::read_to_string(root.join("scripts/keymap-parity.json")).expect("read the contract")
}

fn write_contract(root: &Path, value: &serde_json::Value) {
    let text = serde_json::to_string_pretty(value).expect("serialize the contract");
    std::fs::write(root.join("scripts/keymap-parity.json"), text).expect("write the contract");
}

/// Parse the contract into a mutable JSON value.
fn parse_contract(root: &Path) -> serde_json::Value {
    serde_json::from_str(&contract_of(root)).expect("the contract must be valid JSON")
}

/// Drop the entry of `section` whose `pi` field is `pi_id`.
fn drop_entry(value: &mut serde_json::Value, section: &str, pi_id: &str) {
    let arr = value
        .get_mut(section)
        .and_then(|v| v.as_array_mut())
        .unwrap_or_else(|| panic!("the contract must have a {section} array"));
    let before = arr.len();
    arr.retain(|e| e.get("pi").and_then(|v| v.as_str()) != Some(pi_id));
    assert_ne!(
        arr.len(),
        before,
        "the contract must have had a {section} entry for {pi_id}"
    );
}

#[test]
fn the_oracle_is_green_on_the_real_tree() {
    let root = repo_root();
    let (code, text) = run_oracle(&root);
    assert_eq!(
        code,
        Some(0),
        "the keymap oracle must pass on the real tree; it said:\n{text}"
    );
    assert!(
        text.contains("键位对照通过"),
        "the oracle must say it passed; it said:\n{text}"
    );
}

#[test]
fn a_pi_key_that_nobody_adjudicated_makes_it_red() {
    let root = repo_root();
    let scratch = scratch(&root);
    // Drop one pairing: Pi's `app.suspend` row is simply gone.
    let mut value = parse_contract(&scratch);
    drop_entry(&mut value, "map", "app.suspend");
    write_contract(&scratch, &value);

    let (code, text) = run_oracle(&scratch);
    assert_ne!(
        code,
        Some(0),
        "an un-adjudicated Pi key must make the oracle red; it said:\n{text}"
    );
    assert!(
        text.contains("app.suspend") && text.contains("没有登记"),
        "the oracle must name the key that was not adjudicated:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_pairing_whose_semantic_class_does_not_match_makes_it_red() {
    let root = repo_root();
    let scratch = scratch(&root);
    // Repoint Pi's `tui.editor.undo` (class `undo`) at nini's `Yank`
    // (class `yank`): the pairing now claims a behaviour nini's action
    // does not have.
    let mut value = parse_contract(&scratch);
    {
        let arr = value
            .get_mut("map")
            .and_then(|v| v.as_array_mut())
            .expect("the contract must have a map array");
        let entry = arr
            .iter_mut()
            .find(|e| e.get("pi").and_then(|v| v.as_str()) == Some("tui.editor.undo"))
            .expect("the contract must pair tui.editor.undo");
        entry["nini"] = serde_json::Value::String("Yank".into());
    }
    write_contract(&scratch, &value);

    let (code, text) = run_oracle(&scratch);
    assert_ne!(
        code,
        Some(0),
        "a mismatched semantic class must make the oracle red; it said:\n{text}"
    );
    assert!(
        text.contains("tui.editor.undo"),
        "the oracle must name the pairing that broke:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn an_accidental_rebind_in_ninis_own_keymap_makes_it_red() {
    let root = repo_root();
    let scratch = scratch(&root);

    // Rebind nini's `Ctrl+A` (MoveLineStart) to `Ctrl+B`. Pi's
    // `tui.editor.cursorLineStart` is registered `keys: same`, so the
    // rebind must be caught — this is the check that protects nini's
    // keymap from drifting away from Pi without anybody noticing.
    let keys_path = scratch.join("crates/nini-tui/src/keys.rs");
    let keys = std::fs::read_to_string(&keys_path).expect("read keys.rs");
    let broken = keys.replace("KeyCode::Char('a')", "KeyCode::Char('b')");
    assert_ne!(
        broken, keys,
        "the binding to rebind must have been found verbatim — keys.rs's \
         formatting changed and this test would otherwise rebind nothing"
    );
    std::fs::write(&keys_path, broken).expect("write the rebound keymap");

    let (code, text) = run_oracle(&scratch);
    assert_ne!(
        code,
        Some(0),
        "an accidental rebind must make the oracle red; it said:\n{text}"
    );
    assert!(
        text.contains("keys=same"),
        "the oracle must report the key mismatch:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_known_conflict_without_its_registration_makes_it_red() {
    let root = repo_root();
    let scratch = scratch(&root);
    // Drop the registration of the `app.thinking.toggle` conflict: the
    // pairing now claims a class Pi does not have, with nobody owning it.
    let mut value = parse_contract(&scratch);
    drop_entry(&mut value, "divergences", "app.thinking.toggle");
    write_contract(&scratch, &value);

    let (code, text) = run_oracle(&scratch);
    assert_ne!(
        code,
        Some(0),
        "an unregistered conflict must make the oracle red; it said:\n{text}"
    );
    assert!(
        text.contains("app.thinking.toggle"),
        "the oracle must name the conflict that lost its registration:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
