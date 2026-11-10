//! `/resume` must be an **interactive session picker**, not a wall of text.
//!
//! What was wrong (and what these tests pin):
//!
//! * `/resume` printed up to 50 lines of `N: <uuid>.jsonl [38762 bytes, …]`
//!   into the transcript. A UUID filename identifies nothing, so the user
//!   could not tell which session was which — the picker now labels every
//!   row with the session's name or its first user message, plus the
//!   relative time and the project directory.
//! * The listing was returned as `CommandOutcome::Output`, and the runtime
//!   writes every output line with `push_assistant`, which **appends it to
//!   the session file**. The listing was therefore stored in the session as
//!   if the assistant had said it. `/resume` now returns no output lines at
//!   all: every word it produces is a transcript-only `Notice`.
//! * `/resume <query>` used to print a filtered listing; it now opens the
//!   picker with the query prefilled (Pi's behaviour — `Pi`'s `/resume`
//!   opens the picker and never prints a listing).
//! * The picker used to pre-set its result to the first item, so pressing
//!   Esc silently loaded that session: the runtime applies the result when
//!   the panel closes, whether the user picked or cancelled.

use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::selector::SelectorState;
use nini_tui::selectors::SessionSelector;
use nini_tui::settings::SettingsManager;
use nini_tui::state::{AppState, TranscriptLine};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nini-resume-sel-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Write a session file by hand: header, optional first user message,
/// optional `session_info` (the session's own name).
///
/// Written as raw JSONL on purpose: the picker has to read *any* session
/// file, not only the ones nini itself wrote.
fn seed_session(path: &Path, cwd: &str, name: Option<&str>, first_user: Option<&str>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut out = String::new();
    out.push_str(&format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"u_test\",\
         \"timestamp\":\"2026-01-01T00:00:00.000Z\",\"cwd\":\"{cwd}\"}}\n"
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

/// The agent directory the picker reads from.
fn agent_dir(dir: &Path) -> PathBuf {
    dir.join("agent")
}

/// `/resume` with no argument opens the picker and prints nothing.
#[test]
fn resume_without_arguments_opens_the_session_selector() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("open");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        None,
        Some("read the config"),
    );

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "");
    assert!(r.error.is_none(), "/resume must not fail: {:?}", r.error);
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.is_empty(),
                "/resume must not print a session listing: {lines:?}"
            );
        }
        other => panic!("expected Output, got {other:?}"),
    }
    assert_eq!(
        state.run_state.status, "open_selector:session",
        "/resume without arguments must open the interactive session selector"
    );
    // Nothing the assistant "said" — the listing must not be a model turn.
    assert!(
        !state
            .transcript_state
            .lines
            .iter()
            .any(|l| matches!(l, TranscriptLine::AssistantText(_))),
        "/resume must not put anything in the transcript as an assistant message"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `/resume <query>` opens the picker with the query prefilled.
#[test]
fn resume_with_a_query_prefills_the_selector_filter() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("query");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "claude");
    assert!(r.error.is_none(), "/resume must not fail: {:?}", r.error);
    match r.outcome {
        CommandOutcome::Output(lines) => {
            assert!(
                lines.is_empty(),
                "/resume <query> must not print a listing: {lines:?}"
            );
        }
        other => panic!("expected Output, got {other:?}"),
    }
    assert_eq!(
        state.run_state.status, "open_selector:session?claude",
        "/resume <query> must open the picker with the query prefilled"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The command must not write a single byte into the session file.
///
/// The runtime writes `CommandOutcome::Output` lines with `push_assistant`,
/// which appends them to the session — that is how the old 50-line listing
/// got stored in the session. The harness below reproduces exactly that
/// runtime step, so the assertion covers the real path.
#[test]
fn resume_writes_nothing_into_the_session_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("file");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let session_path = dir.join("agent/sessions/proj/20260101-000000_aa.jsonl");

    let mut state = AppState::new("test-model");
    state.session_init(session_path.clone());
    state.push_user("read the config");
    state.session_flush();
    std::thread::sleep(Duration::from_millis(200));
    let before = std::fs::read(&session_path).expect("the session file must exist");

    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Resume, "");
    assert!(r.error.is_none(), "/resume must not fail: {:?}", r.error);
    // What the runtime does with the command's output lines.
    if let CommandOutcome::Output(lines) = &r.outcome {
        for line in lines {
            state.push_assistant(line.clone());
        }
    }
    state.session_flush();
    std::thread::sleep(Duration::from_millis(200));
    let after = std::fs::read(&session_path).expect("the session file must still exist");

    assert_eq!(
        before, after,
        "/resume must not write anything into the session file — the old \
         listing was stored there as if the assistant had said it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A session with no name is labelled by what the user first said.
#[test]
fn the_picker_labels_a_session_by_its_first_user_message() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("label");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        None,
        Some("read the config"),
    );

    let sel = SessionSelector::all_sessions();
    let items = sel.state_items();
    assert_eq!(items.len(), 1, "one session on disk, one row");
    assert!(
        items[0].label.contains("read the config"),
        "the row must be recognisable by the first user message: {:?}",
        items[0].label
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A session that has a name is labelled by that name.
#[test]
fn a_named_session_is_labelled_by_its_name() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("named");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        Some("my session"),
        Some("read the config"),
    );

    let items = SessionSelector::all_sessions().state_items();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].label, "my session",
        "a named session must be labelled by its name"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A session with nothing in it says so instead of showing a blank row.
#[test]
fn an_empty_session_is_labelled_as_empty() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("empty");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        None,
        None,
    );

    let items = SessionSelector::all_sessions().state_items();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "(no messages)");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Every row carries the relative time and the project directory.
#[test]
fn the_picker_shows_the_relative_time_and_the_project_directory() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("desc");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/projects/myproj",
        None,
        Some("read the config"),
    );

    let items = SessionSelector::all_sessions().state_items();
    let desc = items[0].description.clone().expect("a description");
    assert!(
        desc.contains("myproj"),
        "the row must name the project directory: {desc:?}"
    );
    let rel = desc.split(" · ").next().unwrap_or_default();
    assert!(
        rel == "now" || rel.ends_with('m') || rel.ends_with('h') || rel.ends_with('d'),
        "the row must carry a relative time: {desc:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The current project's sessions come first, even when another project's
/// session was touched more recently.
#[test]
fn the_picker_lists_the_current_project_first() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("order");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    let cwd_name = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    assert!(!cwd_name.is_empty(), "precondition: a current directory");

    let mine = dir.join(format!("agent/sessions/{cwd_name}/mine.jsonl"));
    let other = dir.join("agent/sessions/somewhere-else/other.jsonl");
    seed_session(
        &mine,
        &dir.join(&cwd_name).display().to_string(),
        None,
        Some("mine"),
    );
    seed_session(&other, "/tmp/somewhere-else", None, Some("other"));
    // The other project's file is the newer one, so only the project rule
    // can put ours first.
    let now = SystemTime::now();
    std::fs::File::open(&other)
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
        items[0].label.contains("mine"),
        "the current project's session must come first: {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The number in `/resume <n>` and the row number in the picker mean the
/// same session.
#[test]
fn the_picker_row_n_is_the_session_resume_n_loads() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("rown");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        None,
        Some("read the config"),
    );

    let items = SessionSelector::all_sessions().state_items();
    assert_eq!(items.len(), 1);

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

/// A number that is out of range is reported as such — it never falls
/// through to the picker.
///
/// `/resume 99999999999999999999` does not fit in a `usize`, so it used
/// to skip the index branch entirely and open the picker with that
/// number prefilled as a filter — a filter that matches nothing, with no
/// word about why. Same for `/resume -1`.
#[test]
fn an_out_of_range_index_is_reported_not_treated_as_a_query() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("range");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        None,
        Some("read the config"),
    );

    for arg in ["99999999999999999999", "-1", "0", "7"] {
        let mut state = AppState::new("test-model");
        let mut settings = SettingsManager::default();
        let r = dispatch(&mut state, &mut settings, CommandId::Resume, arg);
        assert!(
            r.error.is_none(),
            "/resume {arg} must not fail: {:?}",
            r.error
        );
        assert_ne!(
            state.run_state.status, "open_selector:session",
            "/resume {arg} is an index, not a query — it must not open the picker"
        );
        let said = state
            .transcript_state
            .lines
            .iter()
            .filter_map(|l| match l {
                TranscriptLine::Notice(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            said.contains("Invalid index"),
            "/resume {arg} must say the index is invalid; transcript said: {said}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Cancelling the picker must load nothing.
#[test]
fn cancelling_the_picker_loads_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("cancel");
    std::env::set_var("NINI_AGENT_DIR", agent_dir(&dir));
    seed_session(
        &dir.join("agent/sessions/proj/a.jsonl"),
        "/tmp/proj",
        None,
        Some("read the config"),
    );

    let sel = SessionSelector::all_sessions();
    assert!(
        sel.result.is_none(),
        "the picker must not pre-select a session — the runtime applies \
         the result when the panel closes, Esc included"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The picker is on screen: title, first user message, project directory.
#[test]
fn the_picker_frame_shows_the_session_rows() {
    use nini_tui::render::render_selector_panel;
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
            let area = Rect {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            };
            render_selector_panel(
                f,
                &title,
                "",
                &items,
                &visible,
                sel.state_selected(),
                &theme,
                area,
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
        frame.contains("myproj"),
        "the project directory must be on screen:\n{frame}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
