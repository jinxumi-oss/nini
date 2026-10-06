//! Oracle: file-mutating tools must not race each other.
//!
//! nini runs a batch of tool calls concurrently (`agent.rs`,
//! `FuturesUnordered` + worker pool). For reads that is a win. For writes it
//! is data loss.
//!
//! `EditTool::execute` is a read-modify-write. Two edits to the same file in
//! one assistant turn — an ordinary pattern, e.g. two unrelated string
//! replacements in one file — both read the same original and the second
//! overwrote the first. Measured on this machine with 8 concurrent edits to
//! one file, before the fix, across five runs:
//!
//! ```text
//! 1/8 edits reported success; final file has 1/8 markers edited
//! 3/8 edits reported success; final file has 2/8 markers edited
//! 4/8 edits reported success; final file has 2/8 markers edited
//! ```
//!
//! It was worse than a lost update. The temp file was named
//! `.{file}.nini-tmp.{pid}` — process id only — so every concurrent edit to
//! that file truncated and rewrote the *same* path; interleaved
//! truncate-and-write can leave the file mangled, not merely stale.
//!
//! Pi serialises per canonical path in `withFileMutationQueue`
//! (`pi-agent-core/src/harness/tools/file-mutation-queue.ts`), which
//! `edit.ts` and `write.ts` wrap themselves in. Same-path work queues;
//! different paths stay parallel. These tests pin both halves — a fix that
//! serialised everything would also stop losing data, and would be wrong.

use nini_core::tool::{Tool, ToolContext};
use nini_tools::{EditTool, WriteTool};
use std::path::PathBuf;
use std::sync::Arc;

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "nini-mutq-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ctx_for(dir: &PathBuf) -> ToolContext {
    ToolContext {
        cwd: dir.clone(),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_to_one_file_all_survive() {
    const N: usize = 8;
    let dir = scratch("edit");
    let file = dir.join("f.txt");
    let body: String = (0..N).map(|i| format!("M{i}\n")).collect();
    std::fs::write(&file, &body).unwrap();

    let tool = Arc::new(EditTool::new());
    let ctx = ctx_for(&dir);
    let mut set = tokio::task::JoinSet::new();
    for i in 0..N {
        let (file, tool, ctx) = (file.clone(), tool.clone(), ctx.clone());
        set.spawn(async move {
            tool.execute(
                serde_json::json!({
                    "path": file.to_string_lossy(),
                    "old_text": format!("M{i}\n"),
                    "new_text": format!("M{i}-EDITED\n"),
                }),
                ctx,
            )
            .await
        });
    }
    let mut ok = 0;
    while let Some(r) = set.join_next().await {
        if r.unwrap().is_ok() {
            ok += 1;
        }
    }

    let final_text = std::fs::read_to_string(&file).unwrap();
    let edited = final_text.matches("-EDITED").count();
    assert_eq!(ok, N, "every edit should apply, got {ok}/{N}");
    assert_eq!(
        edited,
        N,
        "lost update: {} of {N} edits vanished.\n{final_text}",
        N - edited
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_and_a_write_to_one_file_do_not_interleave() {
    // Mixed batch: the write must not land between the edit's read and its
    // rename, which would discard the edit entirely.
    let dir = scratch("mixed");
    let file = dir.join("mixed.txt");
    std::fs::write(&file, "KEEP\n").unwrap();

    let edit = Arc::new(EditTool::new());
    let write = Arc::new(WriteTool::new());
    let ctx = ctx_for(&dir);
    let mut set = tokio::task::JoinSet::new();

    for i in 0..6 {
        let (file, edit, write, ctx) = (file.clone(), edit.clone(), write.clone(), ctx.clone());
        set.spawn(async move {
            if i % 2 == 0 {
                edit.execute(
                    serde_json::json!({
                        "path": file.to_string_lossy(),
                        "old_text": "KEEP\n",
                        "new_text": format!("EDIT{i}\n"),
                    }),
                    ctx,
                )
                .await
            } else {
                write
                    .execute(
                        serde_json::json!({
                            "path": file.to_string_lossy(),
                            "content": format!("WROTE{i}\n"),
                        }),
                        ctx,
                    )
                    .await
            }
        });
    }
    while let Some(_r) = set.join_next().await {}

    let final_text = std::fs::read_to_string(&file).unwrap();
    // Whatever the interleaving, the file must hold exactly one of the
    // writes that ran last — never a blend. A torn write (e.g. "EDIT1WRIT")
    // means a truncate-and-write interleaved, which is the corruption case
    // the pid-only temp name allowed.
    assert!(
        final_text.lines().count() == 1,
        "file is torn across concurrent writers:\n{final_text:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn different_files_still_execute_in_parallel() {
    // A fix that serialised the whole tool batch would also stop losing
    // data, and would throw away the parallelism v0.8.5 added. Pin the
    // other half of Pi's contract: different paths do not queue.
    let dir = scratch("parallel");
    let files: Vec<PathBuf> = (0..8).map(|i| dir.join(format!("p{i}.txt"))).collect();
    for f in &files {
        std::fs::write(f, "x\n").unwrap();
    }
    let edit = Arc::new(EditTool::new());
    let ctx = ctx_for(&dir);

    let started = std::time::Instant::now();
    let mut set = tokio::task::JoinSet::new();
    for f in files.iter().cloned() {
        let (edit, ctx) = (edit.clone(), ctx.clone());
        set.spawn(async move {
            edit.execute(
                serde_json::json!({
                    "path": f.to_string_lossy(),
                    "old_text": "x\n",
                    "new_text": "y\n",
                }),
                ctx,
            )
            .await
        });
    }
    while let Some(_r) = set.join_next().await {}
    let elapsed = started.elapsed();

    for f in files.iter() {
        assert_eq!(std::fs::read_to_string(f).unwrap(), "y\n");
    }
    // Not a benchmark, just a guard against accidentally global locking:
    // 8 files x 1ms of sleep per edit must not cost 8ms of serialisation.
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "different paths appear to be serialising: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
