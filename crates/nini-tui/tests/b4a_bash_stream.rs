//! B4a: 运行中的 `!cmd` 必须**可中止、可流式、不阻塞 UI**。
//!
//! 台账差距 B4a 的三条症状（改动前实测）：`!cmd` 走
//! `BashRunner::run_blocking` 并**同步跑在 UI 事件循环线程上**，于是
//!
//! * 命令跑完之前**不重绘**——转录里一条 `BashExecution` 都没有；
//! * 输出不是流式的：`sleep 3` 之后才一次性出现；
//! * `Esc` 无效——`EscapeTarget` 里根本没有「bash 正在跑」这一态，
//!   唯一的终止者是 runner 自己的 10 s 超时。
//!
//! Pi 的行为（`references/pi-0.85.1`）：
//! * `interactive-mode.ts:6535-6538` —— 每读到一个 chunk 就
//!   `bashComponent.appendOutput(chunk); ui.requestRender();`
//! * `interactive-mode.ts:2855-2856` —— 运行中按 Esc →
//!   `this.session.abortBash()`，**只杀命令**，不中止回合。
//!
//! 下面每条断言都钉住其中一条，期望值写全，不做 `contains` 式凑数。

use nini_tui::runtime::{
    apply_action, escape_action, submit_user_input, AgentDriver, EscapeAction, SharedState,
};
use nini_tui::state::{AppState, EscapeTarget, TranscriptLine};
use nini_tui::Key;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nini-b4a-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn state_with_input(text: &str, tag: &str) -> (SharedState, std::path::PathBuf) {
    let dir = scratch(tag);
    let path = dir.join("s.jsonl");
    let mut s = AppState::new("test-model");
    s.session_init(path);
    s.input.text = text.to_string();
    s.input.cursor = text.len();
    (Arc::new(Mutex::new(s)), dir)
}

/// Every `(cmd, output)` pair currently in the transcript.
fn bash_lines(state: &AppState) -> Vec<(String, String)> {
    state
        .transcript_state
        .lines
        .iter()
        .filter_map(|l| match l {
            TranscriptLine::BashExecution { cmd, output, .. } => {
                Some((cmd.clone(), output.clone()))
            }
            _ => None,
        })
        .collect()
}

/// A driver that only counts the turns it was asked to start.
fn counting_driver(counter: Arc<AtomicUsize>) -> AgentDriver {
    Arc::new(move |text, _, _, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        let _ = text;
        tokio::spawn(async {})
    })
}

/// The messages the model would actually see — the same conversion the
/// session reader uses (`nini-core/src/conversion.rs`: `bashExecution`
/// entries are dropped there, a **user** message is not).
fn model_visible_messages(state: &AppState) -> Vec<nini_core::provider::Message> {
    let Some(arc) = state.session_state.session.clone() else {
        return Vec::new();
    };
    let guard = arc.lock().unwrap();
    guard
        .entries
        .iter()
        .filter_map(nini_core::conversion::session_entry_to_llm_message)
        .collect()
}

fn message_text(m: &nini_core::provider::Message) -> String {
    let mut out = String::new();
    for b in &m.content {
        if let nini_core::provider::ContentBlock::Text { text } = b {
            out.push_str(text);
        }
    }
    out
}

/// How many `bashExecution` entries the session has on disk.
fn bash_execution_entries(state: &AppState) -> usize {
    let Some(arc) = state.session_state.session.clone() else {
        return 0;
    };
    let guard = arc.lock().unwrap();
    guard
        .entries
        .iter()
        .filter(|e| match e {
            nini_core::SessionEntry::Message(m) => matches!(
                m.message,
                nini_core::entries::AgentMessage::BashExecution(_)
            ),
            _ => false,
        })
        .count()
}

/// ── 1. 逐块流式：命令还在跑的时候，第一块输出就已经在转录里 ────────────
///
/// The command prints `a`, waits 400 ms, prints `b`. If the output were
/// collected to end-of-run (the old behaviour) no transcript line could
/// contain `a` before the whole command returned, because the line is only
/// pushed afterwards. So: poll the *live* transcript, record the instant
/// the first `a` appears and the instant the whole command is done, and
/// require the gap to be a real gap, not "both at the end".
#[test]
fn the_first_chunk_lands_while_the_command_is_still_running() {
    let (shared, dir) = state_with_input("!sh -c 'printf a; sleep 0.4; printf b'", "stream");
    let counter = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver(counter.clone());
    let started = Instant::now();
    submit_user_input(
        &shared,
        &driver,
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );
    let finished = started.elapsed();

    // The line is there, and it carries **both** chunks in order.
    let g = shared.lock().unwrap();
    let lines = bash_lines(&g);
    assert_eq!(
        lines.len(),
        1,
        "a `!cmd` produces exactly one BashExecution line: {lines:?}"
    );
    assert_eq!(
        lines[0].0, "sh -c 'printf a; sleep 0.4; printf b'",
        "the line shows the command as typed (without the `!`): {lines:?}"
    );
    assert_eq!(
        lines[0].1, "ab",
        "both chunks land on the one line, in order: {lines:?}"
    );
    assert!(
        finished >= Duration::from_millis(380),
        "the command really did take its 400 ms — the test is not asserting \
         against a fast path (elapsed {finished:?})"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The same invariant, asserted on the **timeline** rather than the result:
/// a separate thread watches the live transcript and must see `a` (and only
/// `a`) strictly before the command's total 600 ms has elapsed.
#[test]
fn output_is_visible_on_the_live_transcript_before_the_command_finishes() {
    let (shared, dir) = state_with_input("!sh -c 'printf a; sleep 0.6; printf b'", "live");
    let watcher_state = Arc::clone(&shared);
    let started = Instant::now();
    let seen = Arc::new(Mutex::new(None::<(Duration, String)>));
    let seen_w = Arc::clone(&seen);
    let watcher = std::thread::spawn(move || {
        for _ in 0..400 {
            let snapshot = {
                let g = watcher_state.lock().unwrap();
                bash_lines(&g).into_iter().next()
            };
            if let Some((_, output)) = snapshot {
                if output.contains('a') {
                    let at = started.elapsed();
                    *seen_w.lock().unwrap() = Some((at, output));
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });

    let counter = Arc::new(AtomicUsize::new(0));
    submit_user_input(
        &shared,
        &counting_driver(counter),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );
    let finished = started.elapsed();
    watcher.join().unwrap();

    // Final state: both chunks, one line.
    {
        let g = shared.lock().unwrap();
        let lines = bash_lines(&g);
        assert_eq!(lines.len(), 1, "one line for one command: {lines:?}");
        assert_eq!(lines[0].1, "ab", "both chunks, in order: {lines:?}");
    }

    let observed = seen.lock().unwrap().clone().expect(
        "the live transcript was watched for 4 s and never showed the first \
         chunk — the output is not streaming",
    );
    let (at, output) = observed;
    assert_eq!(
        output, "a",
        "the first observation must be the **first chunk only** — seeing \
         \"ab\" means the whole output arrived at the end, which is the \
         buffering behaviour B4a removes"
    );
    assert!(
        at < Duration::from_millis(500),
        "the first chunk took {at:?} to appear, but the command does not \
         finish until {finished:?} — that is end-of-run buffering, not \
         streaming"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── 2. Esc 可中止：1 秒内出现中止标记，作业被清空，可再次提交 ──────────
#[test]
fn escape_aborts_the_running_command_and_clears_the_job() {
    let (shared, dir) = state_with_input("!sleep 5", "abort");
    let counter = Arc::new(AtomicUsize::new(0));
    let driver = counting_driver(counter.clone());
    let (tx, _rx) = tokio::sync::watch::channel(false);

    // Start the job the way the event loop does. It must return at once —
    // this call *is* the UI thread's work in the real TUI.
    let started = Instant::now();
    assert!(
        nini_tui::runtime::start_bash_job_from_input(&shared, tx.clone()),
        "`!sleep 5` must be recognised as a command"
    );
    let returned = started.elapsed();
    assert!(
        returned < Duration::from_millis(500),
        "starting a command must not block the caller (took {returned:?}) —          this is the UI thread in the real TUI"
    );

    {
        let g = shared.lock().unwrap();
        assert!(
            g.is_bash_running(),
            "the job is running: {:?}",
            g.run_state.bash_job
        );
        assert_eq!(
            g.run_state.escape_target,
            EscapeTarget::Bash,
            "Pi: `session.isBashRunning` is its own Escape target"
        );
        assert_eq!(
            escape_action(&g),
            EscapeAction::AbortBash,
            "Esc while a command runs must abort the command"
        );
    }

    // Press Esc through the real key path.
    let pressed = Instant::now();
    {
        let mut g = shared.lock().unwrap();
        assert_eq!(g.run_state.mode, nini_tui::state::RunMode::Editing);
        apply_action(&mut g, Key::esc());
        assert!(
            g.run_state.bash_job.as_ref().is_some_and(|j| j.aborted),
            "Esc must mark the job aborted immediately"
        );
    }

    let mut marker_at = None;
    let mut cleared_at = None;
    for _ in 0..300 {
        let (has_marker, cleared) = {
            let g = shared.lock().unwrap();
            let marker = bash_lines(&g)
                .iter()
                .any(|(_, out)| out.ends_with("…(aborted)"));
            (marker, !g.is_bash_running())
        };
        if has_marker && marker_at.is_none() {
            marker_at = Some(pressed.elapsed());
        }
        if cleared && cleared_at.is_none() {
            cleared_at = Some(pressed.elapsed());
        }
        if marker_at.is_some() && cleared_at.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let marker_at = marker_at.expect("the transcript never showed `…(aborted)` within 1.5 s");
    let cleared_at = cleared_at.expect("the job state was never cleared within 1.5 s");
    assert!(
        marker_at < Duration::from_millis(1000),
        "the abort marker took {marker_at:?} to appear; Esc must kill the \
         command in well under a second"
    );
    assert!(
        cleared_at < Duration::from_millis(1000),
        "the job was still considered running {cleared_at:?} after Esc"
    );
    assert!(
        pressed.elapsed() < Duration::from_secs(3),
        "the 5 s command must not have run to completion ({:?})",
        pressed.elapsed()
    );

    {
        let g = shared.lock().unwrap();
        let lines = bash_lines(&g);
        assert_eq!(lines.len(), 1, "still one line: {lines:?}");
        assert_eq!(lines[0].0, "sleep 5", "the command as typed: {lines:?}");
        assert_eq!(
            lines[0].1, "…(aborted)",
            "the aborted line carries the marker at its tail and nothing else: \
             {lines:?}"
        );
        assert_eq!(
            g.run_state.escape_target,
            EscapeTarget::Idle,
            "once the job is gone the Escape target goes back to Idle"
        );
        assert_eq!(
            escape_action(&g),
            EscapeAction::ClearInput,
            "and Esc gets its old meaning back"
        );
    }

    // The job slot is free: a fresh command runs and completes.
    {
        let mut g = shared.lock().unwrap();
        g.input.text = "!echo again".to_string();
        g.input.cursor = g.input.text.len();
    }
    assert!(
        nini_tui::runtime::start_bash_job_from_input(&shared, tx.clone()),
        "after the abort a new command must be accepted"
    );
    let mut second = None;
    for _ in 0..400 {
        {
            let g = shared.lock().unwrap();
            if !g.is_bash_running() {
                if let Some((cmd, out)) = bash_lines(&g).into_iter().last() {
                    if cmd == "echo again" {
                        second = Some(out);
                    }
                }
            }
        }
        if second.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        second.as_deref(),
        Some("again\n"),
        "a command submitted after an abort must run and produce its output"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "neither command reached the agent driver"
    );
    let _ = driver;

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── 3. 中止不误伤：bash 跑着时 Esc 不中止 agent 回合 ─────────────────────
#[test]
fn escape_while_a_command_runs_does_not_abort_the_turn() {
    let (shared, dir) = state_with_input("!sleep 5", "no-turn-abort");
    let counter = Arc::new(AtomicUsize::new(0));
    // The turn is running: `mode == Running` AND `escape_target == Agent`,
    // which is exactly the state in which Esc used to abort the turn.
    {
        let mut g = shared.lock().unwrap();
        g.run_state.mode = nini_tui::state::RunMode::Running;
        g.run_state.escape_target = EscapeTarget::Agent;
        let signal = Arc::new(nini_core::abort_signal::AbortSignal::new());
        g.run_state.abort_signal = Some(Arc::clone(&signal));
        g.run_state
            .pending_steer_messages
            .push("a message the user queued".to_string());
    }
    let signal = shared
        .lock()
        .unwrap()
        .run_state
        .abort_signal
        .clone()
        .expect("installed above");

    let (tx, _rx) = tokio::sync::watch::channel(false);
    assert!(nini_tui::runtime::start_bash_job_from_input(
        &shared,
        tx.clone()
    ));

    // `begin_bash_job` puts the target on Bash; the action is AbortBash.
    {
        let g = shared.lock().unwrap();
        assert_eq!(
            escape_action(&g),
            EscapeAction::AbortBash,
            "a running command outranks a running turn"
        );
        // **这是一处与 Pi 的有意差异，登记在台账 B4a 里。**
        //
        // Pi 的 Esc 阶梯（`references/pi-0.85.1/src/ui/interactive-mode.ts:2853-2861`）：
        // `isStreaming` → 中止回合；`else if (isBashRunning)` → `abortBash()`；
        // `else if (isBashMode)` → 清空输入。
        //
        // 也就是说 **Pi 把「回合在流」排在「命令在跑」前面**：回合与命令
        // 同时在跑时，Pi 的 Esc 中止的是**回合**，命令继续跑到自己结束。
        //
        // nini 把 `isBashRunning` 提到了**回合之前**：命令活着时 Esc 只杀
        // 命令（整棵进程树），**不碰**那一轮的 `abort_signal`、队列与
        // `mode`。理由是台账 B4a 自己的判据——「Esc 能中止正在跑的
        // `!cmd`」——在 Pi 的顺序下**不成立**（回合还活着时 Esc 够不着
        // 命令）。这条差异在此被断言钉住，不许无声漂移。
    }

    {
        let mut g = shared.lock().unwrap();
        apply_action(&mut g, Key::esc());
        assert_eq!(
            g.run_state.mode,
            nini_tui::state::RunMode::Running,
            "Esc must not drop the turn out of Running"
        );
        assert!(
            !signal.is_triggered(),
            "Esc must not trigger the turn's abort signal"
        );
        assert_eq!(
            g.run_state.pending_steer_messages,
            vec!["a message the user queued".to_string()],
            "Esc must not hand the queue back to the editor"
        );
    }

    // Wait for the command to be killed, then confirm the turn again.
    for _ in 0..300 {
        if !shared.lock().unwrap().is_bash_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let g = shared.lock().unwrap();
    assert!(!g.is_bash_running(), "the command must have been killed");
    assert!(!signal.is_triggered(), "and the turn is still alive");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "a `!cmd` never reaches the agent driver"
    );
    drop(g);

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── 4. 结束后 session 里**恰好一条** bashExecution 记录 ─────────────────
#[test]
fn the_session_gets_exactly_one_record_with_the_full_output() {
    let (shared, dir) = state_with_input("!sh -c 'printf a; sleep 0.3; printf b'", "session");
    let counter = Arc::new(AtomicUsize::new(0));
    submit_user_input(
        &shared,
        &counting_driver(counter),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );

    {
        let g = shared.lock().unwrap();
        assert_eq!(
            bash_execution_entries(&g),
            1,
            "one command → exactly one bashExecution entry, not one per chunk"
        );
        // And that entry is the **whole** output, not the last chunk.
        let arc = g.session_state.session.clone().expect("session");
        let guard = arc.lock().unwrap();
        let recorded: Vec<String> = guard
            .entries
            .iter()
            .filter_map(|e| match e {
                nini_core::SessionEntry::Message(m) => match &m.message {
                    nini_core::entries::AgentMessage::BashExecution(b) => Some(b.output.clone()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(recorded.len(), 1, "exactly one bashExecution entry");
        assert_eq!(
            recorded[0], "ab",
            "the record carries the entire output (`a` **and** `b`), not \
             only the last chunk"
        );
        let _ = arc;
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── 5. `!!cmd` 的输出仍然不进模型上下文（`!cmd` 仍然进）────────────────
#[tokio::test]
async fn double_bang_output_stays_out_of_the_model_context() {
    let marker = format!("nini-b4a-private-{}", std::process::id());
    let (shared, dir) = state_with_input(&format!("!!echo {marker}"), "private");
    let counter = Arc::new(AtomicUsize::new(0));
    submit_user_input(
        &shared,
        &counting_driver(counter.clone()),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );

    {
        let g = shared.lock().unwrap();
        let lines = bash_lines(&g);
        assert_eq!(lines.len(), 1, "the command ran: {lines:?}");
        assert_eq!(
            lines[0].0,
            format!("!!echo {marker}"),
            "the user still sees the privacy marker: {lines:?}"
        );
        assert_eq!(
            lines[0].1,
            format!("{marker}\n"),
            "and the output: {lines:?}"
        );
        assert_eq!(counter.load(Ordering::SeqCst), 0, "`!!cmd` starts no turn");
        let visible = model_visible_messages(&g);
        assert!(
            !visible.iter().any(|m| message_text(m).contains(&marker)),
            "`!!`'s output must never reach the model: {visible:?}"
        );
        assert_eq!(
            g.run_state.status, "executed (private)",
            "the status bar says the marker fired"
        );
    }

    // Control: the same pattern with **one** `!` does reach the model, so
    // the assertion above is about the `!!` marker and not about bash output
    // being invisible in general.
    {
        let mut g = shared.lock().unwrap();
        g.input.text = format!("!echo {marker}-public");
        g.input.cursor = g.input.text.len();
    }
    submit_user_input(
        &shared,
        &counting_driver(counter.clone()),
        Arc::new(tokio::sync::Notify::new()),
        tokio::sync::watch::channel(false).0,
    );
    {
        let g = shared.lock().unwrap();
        let visible = model_visible_messages(&g);
        assert!(
            visible
                .iter()
                .any(|m| message_text(m).contains(&format!("{marker}-public"))),
            "`!cmd`'s output does reach the model (Pi: only `!!` excludes it): \
             {visible:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// ── 6. 超时标记保留：`…(timed out after N ms)` ────────────────────────
///
/// The streaming path must keep the blocking path's timeout marker: a
/// command that hit the 10 s limit shows **partial** output, and the user
/// has to be told so. This drives the state machine directly (a real
/// 10 s timeout would make the suite slow) — the marker is produced by
/// `finish_bash_job`, the same function the runner's `Done` feeds.
#[test]
fn the_timeout_marker_survives_the_streaming_path() {
    let mut s = AppState::new("test-model");
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let id = s.begin_bash_job("sleep 60", false, cancel);
    assert!(s.append_bash_chunk(&id, false, "partial output\n"));
    let (output, _stderr, _private) = s
        .finish_bash_job(&id, false, None, 10_000, true, false)
        .expect("the job was running");
    // The marker is appended after a newline — exactly like the blocking
    // path did (`format!("{output}\n…(timed out after {ms} ms)")`), so a
    // command whose output already ended in a newline gets a blank line
    // before the marker. Same text, same shape.
    assert_eq!(
        output, "partial output\n\n…(timed out after 10000 ms)",
        "the timeout marker must say how long the limit was"
    );
    let lines = bash_lines(&s);
    assert_eq!(lines.len(), 1, "one line: {lines:?}");
    assert_eq!(lines[0].1, output, "the transcript shows the same text");
}

/// A late callback from a job that already finished must not touch the
/// **next** job (the v0.15 A1 lesson: read the id in the same critical
/// section that mutates the state).
#[test]
fn a_late_callback_cannot_touch_the_next_job() {
    let mut s = AppState::new("test-model");
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let first = s.begin_bash_job("echo one", false, Arc::clone(&cancel));
    assert!(s
        .finish_bash_job(&first, true, Some(0), 5, false, false)
        .is_some());

    let second = s.begin_bash_job("echo two", false, cancel);
    assert_ne!(first, second, "the two jobs have different ids");

    // The stale callback from job #1 arrives now.
    assert!(
        !s.append_bash_chunk(&first, false, "stale"),
        "a stale chunk must be refused"
    );
    assert!(
        s.finish_bash_job(&first, true, Some(0), 5, false, false)
            .is_none(),
        "a stale finish must be refused"
    );
    // … and job #2 is untouched.
    assert!(s.is_bash_running(), "job #2 is still the running one");
    assert!(s.append_bash_chunk(&second, false, "live"));
    let lines = bash_lines(&s);
    assert_eq!(lines.len(), 2, "two lines: {lines:?}");
    assert_eq!(lines[1].1, "live", "job #2's output: {lines:?}");
}

/// ── 7. 一次只跑一条命令 ────────────────────────────────────────────────
#[test]
fn a_second_command_is_refused_while_one_is_running() {
    let (shared, dir) = state_with_input("!sleep 5", "one-at-a-time");
    let (tx, _rx) = tokio::sync::watch::channel(false);
    assert!(nini_tui::runtime::start_bash_job_from_input(
        &shared,
        tx.clone()
    ));

    {
        let mut g = shared.lock().unwrap();
        // The user types a second command while the first is running.
        g.input.text = "!echo second".to_string();
        g.input.cursor = g.input.text.len();
    }
    assert!(
        nini_tui::runtime::start_bash_job_from_input(&shared, tx.clone()),
        "the input **is** a command, so the caller must not fall through to \
         a normal submit"
    );
    {
        let g = shared.lock().unwrap();
        assert_eq!(
            g.run_state.status, "a command is already running",
            "the second command is refused, not run"
        );
        assert_eq!(
            g.input.text, "!echo second",
            "the refused command stays in the editor so it can be retried"
        );
        let lines = bash_lines(&g);
        assert_eq!(
            lines.len(),
            1,
            "only the first command has a line: {lines:?}"
        );
    }

    // Kill the first one so the test does not wait 5 s.
    {
        let mut g = shared.lock().unwrap();
        apply_action(&mut g, Key::esc());
    }
    for _ in 0..300 {
        if !shared.lock().unwrap().is_bash_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
