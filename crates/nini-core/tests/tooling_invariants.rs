//! 链③ 补账：v0.11.3 那批 `fix(oracle)` / `fix(gate)` 提交此前**裸奔**——
//! 它们修的是 shell 脚本与 CI 配置，而 `scripts/invariants.py` 的测试语料
//! 只有 Rust（`crates/*/tests/*.rs` + `#[cfg(test)]` 模块），于是词法启发式
//! 判不出它们有断言，裸奔数从 1 涨到 4。
//!
//! 这些缺陷**是可以被断言的**：它们的形状是确定的——
//!
//! * 提示里不能出现要等的那个词（否则「Pi 根本没回话」会被判成「跑完了」）；
//! * 必须等启动流程结束再贴第一条提示（否则第一条提示被启动吃掉）；
//! * 快照里的黄金值只能有一个生产者（否则近似版会覆盖运行时真值）；
//! * CI 必须真的装上被冻结的 Pi（否则最硬的那道门禁在 CI 里是空转的）。
//!
//! 这四条都是**文件形状**的断言：读脚本与工作流，逐条核对。它们不是覆盖率，
//! 是「这个缺陷不许原样复发」的最小看守。
//!
//! invariant: 5195b587
//! invariant: dcd75b80
//! invariant: bca7e070

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Every `wait_for "<marker>"` **after the first paste**.
///
/// The first `wait_for` in the script waits for the footer (the model
/// name) — that is a startup check, not content, and the model name is
/// not something the assistant says.
fn awaited_markers(script: &str) -> Vec<String> {
    let mut seen_paste = false;
    let mut out = Vec::new();
    for line in script.lines() {
        let t = line.trim();
        if t.starts_with("paste \"") {
            seen_paste = true;
            continue;
        }
        if !seen_paste {
            continue;
        }
        if let Some(rest) = t.strip_prefix("wait_for \"") {
            if let Some(marker) = rest.split('"').next() {
                out.push(marker.to_string());
            }
        }
    }
    out
}

/// Every `paste "<text>"` literal in the recording script.
fn pasted_texts(script: &str) -> Vec<String> {
    script
        .lines()
        .filter_map(|l| l.trim().strip_prefix("paste \""))
        .filter_map(|rest| rest.split('"').next())
        .map(|s| s.to_string())
        .collect()
}

/// The prompt must not contain the phrase the script is waiting for.
///
/// v0.11.3's defect: the prompt was "run the bash tool then say
/// TOOL_RESULT_SEEN" while `wait_for` grepped the pane for
/// TOOL_RESULT_SEEN — and the **user's own input line** satisfies that
/// grep. A Pi that never answered at all therefore passed the gate.
#[test]
fn the_recording_prompt_never_contains_the_awaited_phrase() {
    let script = read("scripts/pi-session-record.sh");
    let markers = awaited_markers(&script);
    assert!(
        !markers.is_empty(),
        "the recording script waits for nothing — the fixture gate is idle"
    );
    let pastes = pasted_texts(&script);
    assert!(!pastes.is_empty(), "the recording script pastes nothing");
    for text in &pastes {
        for marker in &markers {
            assert!(
                !text.contains(marker),
                "the prompt {text:?} contains the awaited phrase {marker:?} — \
                 the user's own input line would satisfy wait_for, so a Pi \
                 that never answered would pass"
            );
        }
    }
}

/// The awaited phrases must come from the **assistant**, not the user.
///
/// The mock provider's turns are what say TOOL_RESULT_SEEN; if the script
/// stopped asserting that, the wait would be waiting for nothing.
#[test]
fn the_awaited_phrases_come_from_the_mock_provider() {
    let script = read("scripts/pi-session-record.sh");
    for marker in awaited_markers(&script) {
        assert!(
            script.contains(&format!("\"text\": \"{marker}\"")),
            "the phrase {marker:?} is waited for but the mock provider never \
             says it — the wait can only be satisfied by the user's own input"
        );
    }
}

/// The first prompt must not be pasted before Pi finished starting.
///
/// v0.11.3's defect: on a cold runner Pi downloads fd/ripgrep on startup
/// and does not process input while that is in flight, so the first prompt
/// was swallowed and the recording timed out.
#[test]
fn the_recording_waits_for_pi_startup_before_pasting() {
    let script = read("scripts/pi-session-record.sh");
    assert!(
        script.contains("PI_OFFLINE=1"),
        "PI_OFFLINE=1 must be set: Pi otherwise downloads fd/ripgrep on a \
         cold runner and eats the first prompt"
    );
    // 定位**真正的那次检查**（`grep -qF`），而不是提到这句话的注释。
    let startup_wait = script
        .lines()
        .position(|l| l.contains("grep -qF \"Startup is still in progress\""))
        .unwrap_or_else(|| panic!("the script no longer waits for startup to finish"));
    let first_paste = script
        .lines()
        .position(|l| l.trim_start().starts_with("paste \""))
        .unwrap_or_else(|| panic!("the script pastes nothing"));
    assert!(
        startup_wait < first_paste,
        "the first prompt is pasted at line {} but the startup wait is at \
         line {} — Pi does not process input while it is still starting",
        first_paste + 1,
        startup_wait + 1,
    );
}

/// One file, one producer.
///
/// v0.11.3's defect: `pi-tool-dump.mjs` (runtime truth) and
/// `pi-tool-contracts.py --extract` (a source-reading approximation) both
/// wrote `references/pi-0.85.1/tool-contracts.json`, so the CI step that
/// diffed the snapshot was red for a reason unrelated to tampering — and a
/// local run of `--extract` overwrote a golden value that SNAPSHOT.lock
/// hashes.
#[test]
fn the_snapshot_tool_contracts_have_exactly_one_producer() {
    let approx = read("scripts/pi-tool-contracts.py");
    for line in approx.lines() {
        let writes = line.contains("write_text")
            || line.contains("open(")
            || line.contains("json.dump")
            || line.contains("out_path");
        assert!(
            !(writes && line.contains("references/pi-0.85.1/tool-contracts.json")),
            "the approximation writes the snapshot path again ({line:?}) — the \
             runtime golden value and the source approximation are competing \
             for one file"
        );
    }
    // 真正写出去的那个路径必须在 references/derived/ 下。看**赋值**而不是
    // 看字面量：把 `out_path = DERIVED / "…"` 改成 `out_path =
    // SNAPSHOT_DIR / "…"` 时，字面量检查是看不见的。
    let out_lines: Vec<&str> = approx
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.starts_with("#")
                && t.contains("out_path")
                && t.contains('=')
                && !t.contains("out_path.")
                && !t.contains("{out_path}")
        })
        .collect();
    assert!(
        !out_lines.is_empty(),
        "the approximation script no longer assigns an output path"
    );
    for line in out_lines {
        assert!(
            line.contains("DERIVED") || line.contains("derived"),
            "the approximation writes outside references/derived/: {line:?}"
        );
    }

    let runtime = read("scripts/pi-tool-dump.mjs");
    assert!(
        runtime.contains("tool-contracts.json"),
        "the runtime dumper no longer writes the snapshot"
    );
}

/// CI must actually install the frozen Pi.
///
/// v0.11.3's defect: the three golden values are produced by *running*
/// Pi 0.85.1, and the runner had no Pi — so the hardest gate was idle
/// (skipped or failing), while everything looked green.
#[test]
fn ci_installs_the_frozen_pi_and_passes_its_root() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("@earendil-works/pi-coding-agent@0.85.1"),
        "CI no longer installs the frozen Pi — the runtime oracle is idle"
    );
    assert!(
        ci.contains("PI_ROOT"),
        "CI installs Pi but does not pass PI_ROOT to the dumpers"
    );
}

/// The frozen version in CI must be the one the ledger freezes.
#[test]
fn ci_installs_the_version_the_target_freezes() {
    let ci = read(".github/workflows/ci.yml");
    let target = read("references/TARGET.json");
    let version = target
        .split("\"targetVersion\"")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .unwrap_or_else(|| panic!("references/TARGET.json has no targetVersion"));
    assert!(
        ci.contains(&format!("pi-coding-agent@{version}")),
        "CI installs a different Pi version than references/TARGET.json freezes ({version})"
    );
}

/// The helper the test uses must not be able to silently pass.
#[test]
fn the_helpers_actually_find_the_markers() {
    let script = read("scripts/pi-session-record.sh");
    assert!(
        awaited_markers(&script).contains(&"TOOL_RESULT_SEEN".to_string()),
        "the parsing helper no longer sees the script's waits — the \
         assertions above would pass vacuously"
    );
    assert!(
        pasted_texts(&script)
            .iter()
            .any(|t| t.contains("run the bash tool")),
        "the parsing helper no longer sees the script's prompts — the \
         assertions above would pass vacuously"
    );
    let _ = Path::new("");
}
