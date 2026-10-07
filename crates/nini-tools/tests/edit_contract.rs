//! Oracle: the `edit` tool must speak Pi 0.84.2's contract.
//!
//! Ground truth here is **captured, not transcribed**. Pi 0.84.2 was
//! pointed at a local mock endpoint, made to emit
//! `edits: [{oldText,newText}, x3]`, and the file on disk afterwards was
//!
//! ```text
//!   ALPHA BETA GAMMA   ->   X1X X2X X3X
//! ```
//!
//! — one call, three disjoint replacements. Pi's own `tool_result` text:
//!
//! ```text
//!   success: Successfully replaced 3 block(s) in <path>.
//!   failure: Could not find edits[0] in <path>. The oldText must match
//!            exactly including all whitespace and newlines.
//! ```
//!
//! nini accepted `{path, old_text, new_text}` — a single replacement — so a
//! model following Pi's system prompt ("use one edit call with multiple
//! entries in edits[]") was rejected outright. This file pins the contract
//! so the two shapes cannot drift again.

use nini_core::tool::{Tool, ToolContext};
use nini_tools::EditTool;
use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "nini-edit-contract-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

async fn run(dir: &PathBuf, args: serde_json::Value) -> Result<String, String> {
    let tool = EditTool::new();
    let out = tool
        .execute(
            args,
            ToolContext {
                cwd: dir.clone(),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(out.content)
}

#[tokio::test(flavor = "current_thread")]
async fn accepts_pi_edits_array_with_disjoint_replacements() {
    let dir = scratch("array");
    let p = dir.join("t.txt");
    std::fs::write(&p, "ALPHA\nBETA\nGAMMA\n").unwrap();

    let msg = run(
        &dir,
        serde_json::json!({
            "path": "t.txt",
            "edits": [
                {"oldText": "ALPHA", "newText": "X1X"},
                {"oldText": "BETA",  "newText": "X2X"},
                {"oldText": "GAMMA", "newText": "X3X"}
            ]
        }),
    )
    .await
    .expect("Pi-shaped edits[] must be accepted");

    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "X1X\nX2X\nX3X\n",
        "all three disjoint replacements must apply, exactly as Pi did"
    );
    assert!(
        msg.contains("Successfully replaced 3 block(s)"),
        "Pi's success wording, verbatim from a real capture: {msg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn old_single_replacement_shape_still_works() {
    // Backwards compatibility: `old_text` / `new_text` are no longer
    // advertised in the schema but must keep working, so nothing that
    // worked before this change breaks.
    let dir = scratch("legacy");
    let p = dir.join("t.txt");
    std::fs::write(&p, "hello\n").unwrap();

    run(
        &dir,
        serde_json::json!({"path": "t.txt", "old_text": "hello", "new_text": "goodbye"}),
    )
    .await
    .expect("legacy single-replacement shape must still work");

    assert_eq!(std::fs::read_to_string(&p).unwrap(), "goodbye\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn a_missing_replacement_reports_the_offending_index() {
    // Pi: "Could not find edits[0] in <path>. The oldText must match
    // exactly including all whitespace and newlines."
    let dir = scratch("missing");
    let p = dir.join("t.txt");
    std::fs::write(&p, "ALPHA\n").unwrap();

    let err = run(
        &dir,
        serde_json::json!({
            "path": "t.txt",
            "edits": [
                {"oldText": "ALPHA", "newText": "X1X"},
                {"oldText": "NOPE",  "newText": "X2X"}
            ]
        }),
    )
    .await
    .expect_err("the second replacement does not exist");

    assert!(
        err.contains("edits[1]"),
        "the error must name which replacement failed: {err}"
    );
    // Atomic: a partial batch must not be written.
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "ALPHA\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn an_ambiguous_replacement_is_rejected() {
    let dir = scratch("ambig");
    let p = dir.join("t.txt");
    std::fs::write(&p, "dup\ndup\n").unwrap();

    let err = run(
        &dir,
        serde_json::json!({
            "path": "t.txt",
            "edits": [{"oldText": "dup", "newText": "x"}]
        }),
    )
    .await
    .expect_err("an oldText matching twice is not a targeted replacement");
    assert!(err.contains("matches 2 locations"), "{err}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "dup\ndup\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "current_thread")]
async fn the_advertised_schema_is_pis() {
    // The model reads the schema, not the parser. If `edits` is not
    // advertised, the model keeps emitting `old_text` and the multi-edit
    // capability is unreachable no matter what the parser accepts.
    let schema = EditTool::new().spec().input_schema.clone();
    let props = schema["properties"].as_object().expect("properties");
    assert!(
        props.contains_key("edits"),
        "Pi's `edits` must be advertised; got {:?}",
        props.keys().collect::<Vec<_>>()
    );
    assert!(
        !props.contains_key("old_text"),
        "the legacy spelling must not be advertised: one advertised shape \
         is what makes the model behave like Pi's"
    );
    let required = schema["required"].as_array().unwrap();
    assert!(
        required.iter().any(|r| r == "path") && required.iter().any(|r| r == "edits"),
        "required must be [path, edits], got {required:?}"
    );
    let item = &props["edits"]["items"];
    let item_props = item["properties"].as_object().unwrap();
    assert!(
        item_props.contains_key("oldText") && item_props.contains_key("newText"),
        "camelCase inside edits[], as Pi captures: {:?}",
        item_props.keys().collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_edits_list_is_rejected() {
    let dir = scratch("empty");
    std::fs::write(dir.join("t.txt"), "x\n").unwrap();
    let err = run(&dir, serde_json::json!({"path": "t.txt", "edits": []}))
        .await
        .expect_err("an empty edit list is a caller error, not a no-op write");
    assert!(err.contains("must not be empty"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
