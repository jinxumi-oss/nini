//! Oracle: `/export` 必须**认路径**（v0.10.8 / B1b）。
//!
//! Pi 的 `/export` 描述明写「(HTML default, or specify path: .html/.jsonl)」。
//! nini 此前**声明了这个参数却完全不用它**（签名是 `_args`）——用户以为能指定
//! 文件，实际总被丢进 exports 目录。**声明了参数但实现不用**，正是 B1b 记的
//! 那一类缺陷（T4：诚实性）。
//!
//! 判据：
//! 1. 给 `.html` 路径 → 那个文件真的被写出来，且是 HTML；
//! 2. 给 `.jsonl` 路径 → 那个文件真的被写出来，且是**会话 JSONL**（第一行是
//!    Pi 的 session 头）；
//! 3. 给别的扩展名 → **明说**不支持（而不是悄悄写到别处）；
//! 4. 不给路径 → 仍然写 exports 目录（Pi 的默认行为不变）。

use nini_tui::commands::dispatch::dispatch;
use nini_tui::commands::registry::CommandId;
use nini_tui::state::AppState;
use std::path::PathBuf;

fn state_with_session() -> (AppState, PathBuf) {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("nini-export-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut g = AppState::new("test-model");
    g.session_init(dir.join("s.jsonl"));
    g.push_user("hello");
    g.push_assistant("hi");
    (g, dir)
}

fn run(state: &mut AppState, args: &str) -> String {
    let mut settings = nini_tui::settings::SettingsManager::default();
    use nini_tui::commands::result::CommandOutcome;
    match dispatch(state, &mut settings, CommandId::Export, args) {
        r if r.error.is_some() => r.error.unwrap_or_default(),
        r => match r.outcome {
            CommandOutcome::Output(lines) => lines.join("\n"),
            other => format!("{other:?}"),
        },
    }
}

#[test]
fn an_html_path_is_honoured() {
    let (mut state, dir) = state_with_session();
    let target = dir.join("out.html");
    let msg = run(&mut state, target.to_str().unwrap());

    assert!(
        target.exists(),
        "/export 必须写到指定的路径：{msg}\n目录：{}",
        dir.display()
    );
    let html = std::fs::read_to_string(&target).unwrap();
    assert!(
        html.contains("hello") && html.contains("hi"),
        "导出的 HTML 里必须有对话：{html}"
    );
}

#[test]
fn a_jsonl_path_is_honoured_and_holds_the_session() {
    let (mut state, dir) = state_with_session();
    let target = dir.join("out.jsonl");
    let msg = run(&mut state, target.to_str().unwrap());

    assert!(
        target.exists(),
        "/export .jsonl 必须写到指定的路径：{msg}\n目录：{}",
        dir.display()
    );
    let raw = std::fs::read_to_string(&target).unwrap();
    let first = raw.lines().next().expect("不能是空文件");
    let header: serde_json::Value = serde_json::from_str(first).expect("第一行必须是 JSON");
    assert_eq!(
        header.get("type").and_then(|t| t.as_str()),
        Some("session"),
        "导出的 JSONL 必须是会话格式：{first}"
    );
    assert!(raw.contains("hello"), "导出的会话里必须有对话：{raw}");
}

#[test]
fn an_unsupported_extension_is_refused_out_loud() {
    let (mut state, dir) = state_with_session();
    let target = dir.join("out.pdf");
    let msg = run(&mut state, target.to_str().unwrap());

    assert!(!target.exists(), "不支持的扩展名不该被写出来：{msg}");
    assert!(
        msg.contains(".html") && msg.contains(".jsonl"),
        "必须**明说**支持什么：{msg}"
    );
}
