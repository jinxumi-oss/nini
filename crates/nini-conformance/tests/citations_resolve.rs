//! Oracle: every behaviour-layer citation resolves **inside the pinned
//! Pi snapshot**.
//!
//! # The invariant
//!
//! nini's behaviour layer is written against Pi, and every comment that
//! says *what Pi does* points at Pi's source with a **symbol**
//! (`file#symbol`) rather than a line number. The point of a symbol is
//! that it can be checked: `scripts/pi-oracle-lock.py --verify-refs`
//! resolves every citation inside `references/pi-0.85.1/` and turns the
//! run red when a symbol is not there.
//!
//! That check had **four** citations that resolved to nothing:
//!
//! | citation | reality in 0.85.1 |
//! |---|---|
//! | `queueCompactionMessage`, cited against `agent-session.ts` (×2) | the method lives in `interactive-mode.ts` |
//! | `handleSlashCommand`, cited against `interactive-mode.ts` (×2) | there is no such method; the dispatch is inside `setupEditorSubmitHandler` |
//!
//! A citation that resolves to nothing is worse than a line number that
//! drifted: it *looks* like evidence. Four of them were live in the tree.
//!
//! # What this file asserts
//!
//! An **independent** implementation of the same rule, in Rust, over the
//! same snapshot:
//!
//! 1. every `file#symbol` citation in `crates/` names a symbol that
//!    exists in the snapshot file it points at;
//! 2. the scanner finds **some** citations — a rule that finds nothing
//!    passes for the wrong reason;
//! 3. the symbols this round fixed are in the files they claim.
//!
//! (The broken forms are **not** written out here in citation shape: this
//! file is scanned by the same rule, and an example of a bad citation is
//! a bad citation.)
//!
//! The Python script remains the gate's tool (it also verifies hashes);
//! this one is the same rule written a second time, so a mistake in
//! either implementation shows up as a disagreement rather than as a
//! silent pass.

//! invariant: 23c4340b —— 引用必须能在快照里解析；这条不变量就是它的断言。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The documents a citation may name. Mirrors `scripts/pi-line-refs.py`'s
/// `REF_RE` and `scripts/pi_refs.py`'s alias table.
const DOCS: &[&str] = &[
    "agent-loop",
    "interactive-mode",
    "agent-session",
    "keybindings",
    "agent-model",
    "tui-alt-screen",
    "fuzzy",
    "model-search",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("nini-conformance has a parent and a grandparent")
        .to_path_buf()
}

/// Every `.rs` file under `crates/`.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().map(|n| n == "target").unwrap_or(false) {
                    continue;
                }
                stack.push(path);
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Find `file#symbol` citations in one line.
///
/// The document names are matched literally (no regex engine needed):
/// a citation is `<doc>.js#<symbol>` or `<doc>.ts#<symbol>`.
fn citations_in(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for doc in DOCS {
        for ext in ["js", "ts"] {
            let needle = format!("{doc}.{ext}#");
            let mut rest = line;
            while let Some(pos) = rest.find(&needle) {
                let after = &rest[pos + needle.len()..];
                let symbol: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                if !symbol.is_empty() {
                    out.push((format!("{doc}.{ext}"), symbol));
                }
                rest = &rest[pos + needle.len()..];
            }
        }
    }
    out
}

/// Resolve a cited document to a file in the snapshot.
///
/// Same rule as `scripts/pi_refs.py`: the alias table first, then a
/// filename search. `keybindings` is the interesting one — pi-tui and
/// the coding agent both ship a file with that name, and they are not
/// the same thing.
fn resolve(snapshot: &Path, doc: &str) -> Option<PathBuf> {
    let aliases: &[&str] = match doc {
        "agent-loop.js" => &["loop/agent-loop.js"],
        "agent-loop.ts" => &["loop/agent-loop.ts"],
        "agent-session.js" => &["session/agent-session.js"],
        "agent-session.ts" => &["session/agent-session.ts"],
        "interactive-mode.js" => &["ui/interactive-mode.js"],
        "interactive-mode.ts" => &["ui/interactive-mode.ts"],
        "keybindings.ts" => &["tui/keybindings.ts", "ui/keybindings.ts"],
        "keybindings.js" => &["tui/keybindings.js", "ui/keybindings.js"],
        "tui-alt-screen.ts" => &["tui/tui-alt-screen.ts"],
        "tui-alt-screen.js" => &["tui/tui-alt-screen.js"],
        _ => &[],
    };
    for cand in aliases {
        for sub in ["dist", "src"] {
            let p = snapshot.join(sub).join(cand);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    for sub in ["dist", "src"] {
        let dir = snapshot.join(sub);
        let mut hits: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && p.file_name().map(|n| n == doc).unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        hits.sort();
        if let Some(first) = hits.into_iter().next() {
            return Some(first);
        }
    }
    // `fuzzy.js` lives under `dist/tui/`, `model-search.js` under
    // `dist/ui/` — one level deeper than the alias table's flat names.
    for sub in ["dist", "src"] {
        let mut hits: Vec<PathBuf> = walk(&snapshot.join(sub))
            .into_iter()
            .filter(|p| p.file_name().map(|n| n == doc).unwrap_or(false))
            .collect();
        hits.sort();
        if let Some(first) = hits.into_iter().next() {
            return Some(first);
        }
    }
    None
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn every_citation_resolves_inside_the_pinned_snapshot() {
    let root = repo_root();
    let snapshot = root.join("references/pi-0.85.1");
    assert!(
        snapshot.is_dir(),
        "the pinned snapshot is missing: {}",
        snapshot.display()
    );

    let mut cache: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut total = 0usize;

    for file in rust_files(&root) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .display()
            .to_string();
        for (lineno, line) in text.lines().enumerate() {
            for (doc, symbol) in citations_in(line) {
                total += 1;
                let body = cache.entry(doc.clone()).or_insert_with(|| {
                    resolve(&snapshot, &doc)
                        .map(|p| std::fs::read_to_string(&p).unwrap_or_default())
                });
                let Some(body) = body else {
                    unresolved.push(format!(
                        "{rel}:{lineno}: {doc}#{symbol} — the document is not in the snapshot"
                    ));
                    continue;
                };
                if !body.contains(&symbol) {
                    unresolved.push(format!(
                        "{rel}:{lineno}: {doc}#{symbol} — the symbol is not in that document"
                    ));
                }
            }
        }
    }

    assert!(
        total > 0,
        "the scanner found no citations at all — a rule that finds nothing \
         passes for the wrong reason"
    );
    assert!(
        unresolved.is_empty(),
        "citations that do not resolve inside the pinned snapshot:\n  {}",
        unresolved.join("\n  ")
    );
}

#[test]
fn the_symbols_this_round_fixed_are_where_they_claim_to_be() {
    let root = repo_root();
    let snapshot = root.join("references/pi-0.85.1");

    // `queueCompactionMessage` is in interactive-mode.ts, not in
    // agent-session.ts — the file two citations used to name.
    let interactive = resolve(&snapshot, "interactive-mode.ts").expect("interactive-mode.ts");
    let body = std::fs::read_to_string(&interactive).expect("read");
    assert!(
        body.contains("queueCompactionMessage"),
        "the symbol the citations now name must be in the file they name"
    );
    assert!(
        body.contains("setupEditorSubmitHandler"),
        "the slash-command dispatch lives in setupEditorSubmitHandler"
    );
    assert!(
        !body.contains("handleSlashCommand"),
        "there is no handleSlashCommand in 0.85.1 — two citations said there was"
    );

    // The model's search text and Pi's fuzzy filter are pinned now too:
    // a citation that cannot be checked is not a citation.
    let model_search = resolve(&snapshot, "model-search.js").expect("model-search.js");
    let body = std::fs::read_to_string(&model_search).expect("read");
    assert!(body.contains("getModelSelectorSearchText"));

    let fuzzy = resolve(&snapshot, "fuzzy.js").expect("fuzzy.js");
    let body = std::fs::read_to_string(&fuzzy).expect("read");
    assert!(body.contains("fuzzyFilter"));
    assert!(body.contains("fuzzyMatch"));
}
