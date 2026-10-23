//! Oracle: **启动即建立会话**（v0.10.8 / A5）。
//!
//! 此前 nini 启动后不建会话文件：footer 显示 `[no session]`，对话在退出后
//! **整个消失**，`/resume` 也列不出任何东西。Pi 每次启动一个会话就有一个文件
//! （`~/.pi/agent/sessions/<project>/…`）。
//!
//! 判据：
//! 1. 启动后状态里**有**会话路径，且落在 `<agent dir>/sessions/<project>/` 下；
//! 2. 路径带**唯一后缀**（同一秒里开两个会话不能撞名）；
//! 3. 对话写进去之后，文件**真的存在**，并且第一行是 Pi 那种 session 头；
//! 4. 没有 agent 目录时**明说**「没记录」，而不是假装记下了。

use nini_core::paths;
use nini_tui::state::AppState;
use std::path::PathBuf;

/// 直接调用**产品用的那一份**（`AppState::start_session`）。
///
/// 第一版的测试在这里**自己重写了一遍逻辑**——于是把 CLI 里的实现换成
/// 「不建会话」之后，测试**照样全绿**：它根本没碰到被测对象。
/// 这类「测试测的是自己，不是产品」的假绿，比没有测试更危险。
#[inline]
fn start_session(state: &mut AppState, cwd: &std::path::Path) {
    state.start_session(cwd);
}

#[test]
fn starting_up_creates_a_session_path_under_the_project() {
    let _guard = paths::agent_lock();
    let project = PathBuf::from("/tmp/nini-a5-project");
    let mut state = AppState::new("test-model");

    start_session(&mut state, &project);

    let path = state
        .session_state
        .session_path
        .clone()
        .expect("启动后必须有会话路径——否则对话在退出后就整个消失了");

    let sessions = paths::sessions_dir().expect("测试里必须有 agent 目录");
    assert!(
        path.starts_with(&sessions),
        "会话必须落在 sessions 目录下：{} 不在 {} 里",
        path.display(),
        sessions.display()
    );
    assert!(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .ends_with(".jsonl"),
        "会话文件必须是 .jsonl：{}",
        path.display()
    );
    // 项目名进路径（Pi 的形状：sessions/<project>/…）
    assert!(
        path.to_string_lossy().contains("nini-a5-project"),
        "路径里要带项目名：{}",
        path.display()
    );
}

#[test]
fn two_sessions_started_in_the_same_second_do_not_collide() {
    let _guard = paths::agent_lock();
    let project = PathBuf::from("/tmp/nini-a5-project");
    let mut a = AppState::new("test-model");
    let mut b = AppState::new("test-model");

    start_session(&mut a, &project);
    start_session(&mut b, &project);

    let pa = a.session_state.session_path.clone().unwrap();
    let pb = b.session_state.session_path.clone().unwrap();
    assert_ne!(
        pa, pb,
        "同一秒里开两个会话撞名了——后一个会把前一个的对话覆盖掉"
    );
}

#[test]
fn the_conversation_actually_reaches_the_disk() {
    let _guard = paths::agent_lock();
    let project = PathBuf::from("/tmp/nini-a5-project");
    let mut state = AppState::new("test-model");
    start_session(&mut state, &project);
    let path = state.session_state.session_path.clone().unwrap();

    state.push_user("hello");
    state.push_assistant("hi");
    state.session_flush();

    // flush 是异步落盘（后台线程 + 原子替换），等它一下
    let mut found = None;
    for _ in 0..50 {
        if path.exists() {
            found = Some(path.clone());
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let path = found.expect("对话必须真的落到磁盘上——否则 /resume 永远列不出东西");

    let raw = std::fs::read_to_string(&path).unwrap();
    let first = raw.lines().next().expect("文件不能是空的");
    let header: serde_json::Value = serde_json::from_str(first).expect("第一行必须是 JSON");
    assert_eq!(
        header.get("type").and_then(|t| t.as_str()),
        Some("session"),
        "第一行必须是 session 头：{first}"
    );
    assert!(raw.contains("hello"), "用户的消息必须在文件里：{raw}");
}
