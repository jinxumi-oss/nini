//! The `/resume` picker at Pi parity: scope, sort modes, delete, rename.
//!
//! These tests pin the four things Pi's session selector has and nini's
//! did not (`@earendil-works/pi-coding-agent/dist/modes/interactive/components/session-selector.js`):
//!
//! * **scope** — the current folder or every session (`Tab`),
//! * **sort modes** — threaded / recent / relevance (`Ctrl+S`),
//! * **delete** — with a confirmation, the running session excluded
//!   (`Ctrl+D`),
//! * **rename** — written as an appended `session_info` entry (`Ctrl+R`),
//!
//! plus the "named sessions only" filter (`Ctrl+N`) and the path column
//! (`Ctrl+P`).
//!
//! The pattern is the one `resume_selector.rs` (B34) established: the
//! `ENV_LOCK` serialises the tests that move `NINI_AGENT_DIR`, and every
//! session file lives in a scratch directory — a delete test that pointed
//! at the repository would delete a real session.

use nini_tui::selector::SelectorState;
use nini_tui::selectors::session::{SessionNameFilter, SessionScope, SessionSort};
use nini_tui::selectors::SessionSelector;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nini-resume-pi-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The agent directory the picker reads from.
fn agent_dir(dir: &Path) -> PathBuf {
    dir.join("agent")
}

/// The name of the directory nini is running in — the picker calls the
/// sessions under it "the current folder".
fn cwd_name() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// Write a session file by hand.
///
/// The header's `id` and `parentSession` are what the threaded view builds
/// the tree from, so they can be set.
fn seed_session(
    path: &Path,
    cwd: &str,
    id: &str,
    parent: Option<&str>,
    name: Option<&str>,
    first_user: Option<&str>,
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = String::new();
    let parent_field = match parent {
        Some(p) => format!(",\"parentSession\":\"{p}\""),
        None => String::new(),
    };
    out.push_str(&format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\
         \"timestamp\":\"2026-01-01T00:00:00.000Z\",\"cwd\":\"{cwd}\"{parent_field}}}\n"
    ));
    if let Some(msg) = first_user {
        out.push_str(&format!(
            "{{\"type\":\"message\",\"id\":\"msg_1\",\"parentId\":null,\
             \"timestamp\":\"2026-01-01T00:00:01.000Z\",\
             \"message\":{{\"role\":\"user\",\"content\":\"{msg}\",\
             \"timestamp\":1767225601000}}}}\n"
        ));
    }
    if let Some(n) = name {
        out.push_str(&format!(
            "{{\"type\":\"session_info\",\"id\":\"si_1\",\"parentId\":null,\
             \"timestamp\":\"2026-01-01T00:00:02.000Z\",\"name\":\"{n}\"}}\n"
        ));
    }
    std::fs::write(path, out).unwrap();
}

/// The labels of the rows the picker shows.
fn labels(sel: &SessionSelector) -> Vec<String> {
    sel.state_items().into_iter().map(|i| i.label).collect()
}

/// The scope switches between "every session" and "the current folder".
#[test]
fn the_scope_toggles_between_all_and_the_current_folder() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("scope");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let name = cwd_name();
    seed_session(
        &dir.join(format!("agent/sessions/{name}/mine.jsonl")),
        "/tmp/mine",
        "u_mine",
        None,
        None,
        Some("mine"),
    );
    seed_session(
        &dir.join("agent/sessions/somewhere-else/other.jsonl"),
        "/tmp/somewhere-else",
        "u_other",
        None,
        None,
        Some("other"),
    );

    let mut sel = SessionSelector::all_sessions();
    assert_eq!(
        sel.scope(),
        SessionScope::All,
        "the picker starts on every session"
    );
    assert_eq!(labels(&sel).len(), 2, "both sessions are listed");

    sel.toggle_scope();
    assert_eq!(sel.scope(), SessionScope::Current);
    let shown = labels(&sel);
    assert_eq!(
        shown.len(),
        1,
        "only the current folder's session is left: {shown:?}"
    );
    assert!(
        shown[0].contains("mine"),
        "the current folder's session must be the one left: {shown:?}"
    );

    sel.toggle_scope();
    assert_eq!(sel.scope(), SessionScope::All);
    assert_eq!(labels(&sel).len(), 2, "the scope toggles back");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The threaded sort nests a forked session under the session it came
/// from (Pi: `buildSessionTree`).
#[test]
fn the_threaded_sort_nests_a_forked_session_under_its_parent() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("threaded");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/parent.jsonl"),
        "/tmp/proj",
        "u_parent",
        None,
        None,
        Some("the parent"),
    );
    seed_session(
        &dir.join("agent/sessions/proj/child.jsonl"),
        "/tmp/proj",
        "u_child",
        Some("u_parent"),
        None,
        Some("the child"),
    );

    let sel = SessionSelector::all_sessions();
    assert_eq!(sel.sort_mode(), SessionSort::Threaded);
    let shown = labels(&sel);
    assert_eq!(shown.len(), 2);
    let child = shown
        .iter()
        .find(|l| l.contains("the child"))
        .expect("the forked session is listed");
    assert!(
        child.contains('└') || child.contains('├'),
        "a forked session must be drawn under its parent: {child:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The recent sort keeps the order the sessions were loaded in.
#[test]
fn the_recent_sort_keeps_the_loaded_order() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("recent");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let old = dir.join("agent/sessions/proj/old.jsonl");
    let new = dir.join("agent/sessions/proj/new.jsonl");
    seed_session(&old, "/tmp/proj", "u_old", None, None, Some("the old one"));
    seed_session(&new, "/tmp/proj", "u_new", None, None, Some("the new one"));
    // Make the "old" file the newer one, so the mtime order and the
    // written order disagree.
    let now = SystemTime::now();
    std::fs::File::open(&old)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(now)
                .set_accessed(now),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));

    let mut sel = SessionSelector::all_sessions();
    sel.toggle_sort(); // threaded → recent
    assert_eq!(sel.sort_mode(), SessionSort::Recent);
    let shown = labels(&sel);
    assert_eq!(shown.len(), 2);
    assert!(
        shown[0].contains("the old one"),
        "the most recently touched session comes first: {shown:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The relevance sort puts the session that matches the query first.
#[test]
fn the_relevance_sort_ranks_the_matching_session_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("relevance");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        "u_a",
        None,
        None,
        Some("unrelated chatter"),
    );
    seed_session(
        &dir.join("agent/sessions/proj/b.jsonl"),
        "/tmp/proj",
        "u_b",
        None,
        None,
        Some("rewrite the config parser"),
    );

    let mut sel = SessionSelector::all_sessions();
    sel.toggle_sort(); // threaded → recent
    sel.toggle_sort(); // recent → relevance
    assert_eq!(sel.sort_mode(), SessionSort::Relevance);

    let matching = sel.state_filter("config");
    assert_eq!(matching.len(), 1, "only one session matches");
    let row = sel.state_items();
    let hit = &row[matching[0]];
    assert!(
        hit.label.contains("config"),
        "the matching session must be the one that is ranked: {hit:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The named filter lists only the sessions that carry a name.
#[test]
fn the_named_filter_only_lists_named_sessions() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("named");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/named.jsonl"),
        "/tmp/proj",
        "u_named",
        None,
        Some("the named one"),
        None,
    );
    seed_session(
        &dir.join("agent/sessions/proj/plain.jsonl"),
        "/tmp/proj",
        "u_plain",
        None,
        None,
        Some("the unnamed one"),
    );

    let mut sel = SessionSelector::all_sessions();
    assert_eq!(sel.name_filter(), SessionNameFilter::All);
    assert_eq!(labels(&sel).len(), 2);

    sel.toggle_name_filter();
    assert_eq!(sel.name_filter(), SessionNameFilter::Named);
    let shown = labels(&sel);
    assert_eq!(shown.len(), 1, "only the named session is left: {shown:?}");
    assert!(shown[0].contains("the named one"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Deleting a session removes the file — but only after a confirmation.
#[test]
fn the_delete_removes_the_session_file_after_a_confirmation() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("delete");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let path = dir.join("agent/sessions/proj/gone.jsonl");
    seed_session(&path, "/tmp/proj", "u_gone", None, None, Some("doomed"));

    let mut sel = SessionSelector::all_sessions();
    assert!(
        sel.confirming_delete().is_none(),
        "nothing is waiting for a confirmation yet"
    );
    assert!(sel.start_delete(), "the delete starts");
    assert_eq!(
        sel.confirming_delete(),
        Some(path.as_path()),
        "the delete waits for a confirmation"
    );
    assert!(
        path.exists(),
        "the file is still there until the deletion is confirmed"
    );

    assert!(sel.confirm_delete(), "the deletion goes through");
    assert!(
        std::fs::remove_file(&path).is_err(),
        "the session file must be gone: {path:?}"
    );
    assert!(
        labels(&sel).is_empty(),
        "the deleted session is not listed any more"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The running session cannot be deleted — deleting it would leave nini
/// pointing at a file that is not there.
#[test]
fn the_current_session_cannot_be_deleted() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("current");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let path = dir.join("agent/sessions/proj/running.jsonl");
    seed_session(&path, "/tmp/proj", "u_running", None, None, Some("running"));

    let mut sel = SessionSelector::all_sessions().with_current_session(Some(path.clone()));
    assert!(
        !sel.start_delete(),
        "the running session must not be deletable"
    );
    assert!(
        sel.confirming_delete().is_none(),
        "no confirmation is asked for"
    );
    assert!(
        sel.status_message()
            .unwrap_or_default()
            .contains("Cannot delete"),
        "the picker has to say why: {:?}",
        sel.status_message()
    );
    assert!(path.exists(), "the session file is untouched");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Renaming writes a `session_info` entry — the session **file** keeps its
/// name (Pi: `appendSessionInfo`).
#[test]
fn the_rename_writes_a_session_info_entry_and_keeps_the_file_name() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("rename");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");
    seed_session(&path, "/tmp/proj", "u_renamed", None, None, Some("before"));

    let mut sel = SessionSelector::all_sessions();
    assert!(sel.start_rename(), "the rename starts");
    assert_eq!(sel.renaming(), Some(path.as_path()));
    for c in "after".chars() {
        sel.rename_push(c);
    }
    assert!(sel.commit_rename(), "the rename goes through");

    // The file keeps its name …
    assert!(
        path.exists(),
        "the session file must keep its name: {path:?}"
    );
    // … and carries the new name as a `session_info` entry.
    let raw = std::fs::read_to_string(&path).unwrap();
    let mut found = false;
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("session_info")
            && v.get("name").and_then(|n| n.as_str()) == Some("after")
        {
            found = true;
        }
    }
    assert!(
        found,
        "the new name must be written as a session_info entry:\n{raw}"
    );

    // The row shows the new name.
    let shown = labels(&sel);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0], "after", "the row shows the new name");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The path column can be switched on (Pi: `Ctrl+P`).
#[test]
fn the_show_path_toggle_adds_the_path_to_the_row() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("path");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let path = dir.join("agent/sessions/proj/a.jsonl");
    seed_session(&path, "/tmp/proj", "u_a", None, None, Some("hello"));

    let mut sel = SessionSelector::all_sessions();
    let without = sel.state_items()[0].description.clone().unwrap_or_default();
    assert!(
        !without.contains("a.jsonl"),
        "the path is not shown by default: {without:?}"
    );

    sel.toggle_show_path();
    assert!(sel.show_path());
    let with = sel.state_items()[0].description.clone().unwrap_or_default();
    assert!(
        with.contains("a.jsonl"),
        "the path must be shown when it is switched on: {with:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The search box matches the session's name and its first message — the
/// text Pi searches (`getSessionSearchText`).
#[test]
fn the_search_matches_the_name_and_the_first_message() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("search");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        "u_a",
        None,
        Some("the deployment"),
        None,
    );
    seed_session(
        &dir.join("agent/sessions/proj/b.jsonl"),
        "/tmp/proj",
        "u_b",
        None,
        None,
        Some("rewrite the parser"),
    );

    let sel = SessionSelector::all_sessions();
    let named = sel.state_filter("deployment");
    assert_eq!(named.len(), 1, "the name is searchable");
    assert!(
        sel.state_items()[named[0]].label.contains("deployment"),
        "the named session is the one that matches"
    );

    let message = sel.state_filter("parser");
    assert_eq!(message.len(), 1, "the first message is searchable");
    assert!(
        sel.state_items()[message[0]].label.contains("parser"),
        "the session whose first message matches is the one that matches"
    );

    // A quoted phrase has to appear literally.
    let phrase = sel.state_filter("\"rewrite the\"");
    assert_eq!(phrase.len(), 1, "a quoted phrase is matched literally");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The row shows the message count and the age, and the panel says
/// something useful when there is nothing to show.
#[test]
fn the_picker_frame_shows_the_pi_row_layout() {
    use nini_tui::render::render_selector_panel_with_empty;
    use nini_tui::theme::Theme;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("frame");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/projects/myproj",
        "u_a",
        None,
        None,
        Some("read the config"),
    );

    let sel = SessionSelector::all_sessions();
    let items = sel.state_items();
    let visible: Vec<usize> = (0..items.len()).collect();
    let theme = Theme::default();
    let title = sel.state_title();

    let backend = TestBackend::new(100, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            render_selector_panel_with_empty(
                f,
                &title,
                "",
                &items,
                &visible,
                sel.state_selected(),
                &theme,
                Rect {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 20,
                },
                sel.state_empty_message(),
            );
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let mut frame = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf.cell((x, y)).unwrap().symbol());
        }
        frame.push_str(line.trim_end_matches(' '));
        frame.push('\n');
    }

    assert!(frame.contains("Resume session"), "title:\n{frame}");
    assert!(
        frame.contains("read the config"),
        "the first user message must be on screen:\n{frame}"
    );
    assert!(
        frame.contains("1 msgs"),
        "the row carries the message count:\n{frame}"
    );
    assert!(
        frame.contains("now"),
        "the row carries the relative time:\n{frame}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The empty panel says what to do about it (Pi's wording).
#[test]
fn the_empty_state_says_what_to_do_about_it() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("empty");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));

    let mut sel = SessionSelector::all_sessions();
    assert_eq!(
        sel.state_empty_message().as_deref(),
        Some("No sessions found"),
        "nothing at all to show"
    );
    sel.toggle_scope();
    assert_eq!(
        sel.state_empty_message().as_deref(),
        Some("No sessions in current folder. Press Tab to view all."),
        "the current folder is empty — the panel says how to see the rest"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The runtime really routes the session keys into the picker.
///
/// The tests above drive the selector directly. This one goes through
/// the runtime's key handler, because a picker whose keys are not wired
/// up is indistinguishable from a picker that works.
#[test]
fn the_runtime_routes_the_session_keys_into_the_picker() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use nini_tui::runtime::{handle_selector_key, SharedState};
    use nini_tui::state::AppState;
    use std::sync::Arc;

    fn key(c: char, ctrl: bool) -> KeyEvent {
        let mods = if ctrl {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
        KeyEvent::new(KeyCode::Char(c), mods)
    }
    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("runtime");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let path = dir.join("agent/sessions/proj/named.jsonl");
    seed_session(
        &path,
        "/tmp/proj",
        "u_named",
        None,
        Some("the named one"),
        None,
    );

    let mut sel: Box<dyn SelectorState + Send> = Box::new(SessionSelector::all_sessions());
    let shared: SharedState = Arc::new(Mutex::new(AppState::new("test")));
    let mut query = String::new();
    let mut visible: Vec<usize> = (0..sel.state_items().len()).collect();

    // Ctrl+N — only the named sessions.
    handle_selector_key(key('n', true), &shared, &mut sel, &mut query, &mut visible);
    let sess = sel
        .as_any_mut()
        .downcast_mut::<SessionSelector>()
        .expect("the session picker");
    assert_eq!(sess.name_filter(), SessionNameFilter::Named);
    assert_eq!(visible.len(), 1, "the list follows the filter: {visible:?}");

    // Ctrl+R — rename; the typed characters go into the input, not the list.
    handle_selector_key(key('r', true), &shared, &mut sel, &mut query, &mut visible);
    {
        let sess = sel
            .as_any_mut()
            .downcast_mut::<SessionSelector>()
            .expect("the session picker");
        assert_eq!(
            sess.rename_input(),
            "the named one",
            "the input starts with the current name (Pi: `renameInput.setValue`)"
        );
    }
    handle_selector_key(key('x', false), &shared, &mut sel, &mut query, &mut visible);
    handle_selector_key(
        plain(KeyCode::Enter),
        &shared,
        &mut sel,
        &mut query,
        &mut visible,
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.contains("\"name\":\"the named onex\""),
        "the rename must be written into the session file:\n{raw}"
    );
    assert!(
        path.exists(),
        "the session file must keep its name: {path:?}"
    );

    // Ctrl+D — delete asks first; Esc backs out and the file stays.
    handle_selector_key(key('d', true), &shared, &mut sel, &mut query, &mut visible);
    let sess = sel
        .as_any_mut()
        .downcast_mut::<SessionSelector>()
        .expect("the session picker");
    assert!(
        sess.confirming_delete().is_some(),
        "Ctrl+D must ask before deleting"
    );
    handle_selector_key(
        plain(KeyCode::Esc),
        &shared,
        &mut sel,
        &mut query,
        &mut visible,
    );
    assert!(
        path.exists(),
        "Esc must leave the session file alone: {path:?}"
    );

    // The running session cannot be deleted — and the user is told why
    // (Pi: `onError`), instead of the key doing nothing at all.
    let mut sel: Box<dyn SelectorState + Send> =
        Box::new(SessionSelector::all_sessions().with_current_session(Some(path.clone())));
    let mut visible: Vec<usize> = (0..sel.state_items().len()).collect();
    handle_selector_key(key('d', true), &shared, &mut sel, &mut query, &mut visible);
    let notices: Vec<String> = shared
        .lock()
        .unwrap()
        .transcript_state
        .lines
        .iter()
        .filter_map(|l| match l {
            nini_tui::state::TranscriptLine::Notice(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|n| n.contains("Cannot delete")),
        "the refusal has to reach the screen: {notices:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `/resume <n>` counts the rows the picker shows — the tree included.
///
/// A fork is newer than the session it came from, so the raw listing
/// puts it first; the picker draws it *under* its parent. The number has
/// to count what the user sees, not what the disk lists.
#[test]
fn the_resume_number_counts_the_rows_the_picker_shows() {
    use nini_tui::commands::{dispatch, CommandId};
    use nini_tui::settings::SettingsManager;
    use nini_tui::state::AppState;

    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("rown");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let parent = dir.join("agent/sessions/proj/parent.jsonl");
    let child = dir.join("agent/sessions/proj/child.jsonl");
    seed_session(
        &parent,
        "/tmp/proj",
        "u_parent",
        None,
        None,
        Some("the parent session"),
    );
    seed_session(
        &child,
        "/tmp/proj",
        "u_child",
        Some("u_parent"),
        None,
        Some("the forked session"),
    );
    // The fork is the newer file, so the raw listing would put it first.
    let now = SystemTime::now();
    std::fs::File::open(&child)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(now)
                .set_accessed(now),
        )
        .unwrap();

    let items = SessionSelector::all_sessions().state_items();
    assert_eq!(items.len(), 2);
    assert!(
        items[0].label.contains("the parent session"),
        "the parent is the first row — the fork is drawn under it: {items:?}"
    );

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "1");
    assert!(r.error.is_none(), "/resume 1 must not fail: {:?}", r.error);
    assert_eq!(
        state.session_state.session_path,
        Some(PathBuf::from(&items[0].value)),
        "`/resume 1` must load the session the picker's first row shows"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
