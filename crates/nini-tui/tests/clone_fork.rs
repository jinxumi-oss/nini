//! R10（`/clone` 对齐 Pi）+ R11（`/login` / `/logout` 的 provider 清单）。
//!
//! R10 的判据来自 Pi 的 `handleCloneCommand`
//! （references/pi-0.85.1/src/ui/interactive-mode.ts:5186）：
//!
//! 1. `getLeafId()` —— 取 session 树的叶子；没有就 `Nothing to clone yet`；
//! 2. `runtimeHost.fork(leafId, { position: "at" })`；
//! 3. `editor.setText("")` —— **清空编辑器**；
//! 4. `showStatus("Cloned to new session")`。
//!
//! nini 之前的 `/clone` 只把 JSONL 复制一份（产物有了，**当前会话没切**），
//! 用户还得自己 `/resume` —— 做了一半。
//!
//! R11 的判据：`/login` 列出的 provider 必须与 `nini_ai::provider_registry`
//! **同源**。写死的清单早于实现漂移过一次（`together` 在
//! `provider_factory` 里根本构造不出来，命令面却在教用户用它）。

use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scratch(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nini-clone-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 种一个有两轮对话的 session（用户 → 助手 → 用户 → 助手）。
fn seed_session(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join("agent/sessions/proj").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut st = AppState::new("test-model");
    st.session_init(path.clone());
    st.push_user("hello");
    st.push_assistant("hi there");
    st.push_user("more please");
    st.push_assistant("sure thing");
    st.session_flush();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(path.exists(), "the seeded session file must exist");
    path
}

fn open_session(path: &std::path::Path) -> AppState {
    let mut st = AppState::new("test-model");
    st.session_load(path.to_path_buf())
        .expect("the seeded session must load");
    st
}

fn output_lines(r: &nini_tui::commands::CommandResult) -> Vec<String> {
    match &r.outcome {
        CommandOutcome::Output(lines) => lines.clone(),
        other => panic!("expected Output, got {other:?}"),
    }
}

fn file_lines(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} must be readable: {e}", path.display()))
        .lines()
        .map(str::to_string)
        .collect()
}

/// R10-1：clone 之后**当前 session 已经切到新文件**。
/// 之前 `/clone` 只复制文件、不切换，用户看到的还是原会话。
#[test]
fn clone_switches_the_current_session_to_the_new_file() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("switch");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let orig = seed_session(&dir, "20260101-000000_aa.jsonl");

    let mut state = open_session(&orig);
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Clone, "");
    assert!(r.error.is_none(), "/clone errored: {:?}", r.error);

    let new_path = state
        .session_state
        .session_path
        .clone()
        .expect("a clone must leave a current session");
    assert_ne!(
        new_path, orig,
        "/clone must switch the current session to the new file"
    );
    assert!(
        new_path.exists(),
        "the cloned session file must exist: {}",
        new_path.display()
    );

    let out = output_lines(&r);
    assert!(
        out.iter().any(|l| l.contains("cloned to new session")),
        "the user must be told the clone happened: {out:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// R10-2：原分支仍在（文件没被删），新 session 的内容是原内容的前缀
/// （fork-at-leaf 的语义：整条链都保留）。
#[test]
fn clone_keeps_the_original_branch_and_copies_its_entries() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("prefix");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let orig = seed_session(&dir, "20260101-000000_bb.jsonl");

    let mut state = open_session(&orig);
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Clone, "");
    assert!(r.error.is_none(), "/clone errored: {:?}", r.error);
    let new_path = state.session_state.session_path.clone().unwrap();

    assert!(
        orig.exists(),
        "the original session file must survive the clone"
    );

    let orig_lines = file_lines(&orig);
    let new_lines = file_lines(&new_path);
    // 第 0 行是 header（clone 换了 id 与 parentSession），后面是 entries。
    let orig_entries = &orig_lines[1..];
    let new_entries = &new_lines[1..];
    assert!(
        new_entries.starts_with(orig_entries),
        "the clone must keep the original entries as a prefix:\norig={orig_entries:?}\nnew={new_entries:?}"
    );
    assert!(
        !new_lines[0].eq(&orig_lines[0]),
        "the clone must be its own session (different header)"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// R10-3：编辑器被清空（Pi 的 `editor.setText("")`）。
#[test]
fn clone_clears_the_editor() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("editor");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));
    let orig = seed_session(&dir, "20260101-000000_cc.jsonl");

    let mut state = open_session(&orig);
    state.input.replace_whole("half-typed prompt".to_string());
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Clone, "");
    assert!(r.error.is_none(), "/clone errored: {:?}", r.error);
    assert!(
        state.input.text.is_empty(),
        "the editor must be empty after a clone, got {:?}",
        state.input.text
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// R10-4：没有可 clone 的内容时**给用户看提示**，不是静默。
/// Pi: `showStatus("Nothing to clone yet")`。
#[test]
fn clone_without_content_says_so() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("empty");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));

    let mut state = AppState::new("test-model");
    state.session_init(dir.join("agent/sessions/proj/empty.jsonl"));
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Clone, "");
    assert!(r.error.is_none(), "/clone errored: {:?}", r.error);
    let out = output_lines(&r);
    assert!(
        out.iter().any(|l| l.contains("nothing to clone")),
        "an empty session must produce a visible message: {out:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// R11：`/login` 的清单与 `nini_ai::provider_registry` **一致**。
/// 这条 oracle 是防止「清单与实现再次漂移」的关键。
#[test]
fn login_lists_exactly_the_registered_providers() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("login");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Login, "");
    assert!(r.error.is_none(), "/login errored: {:?}", r.error);
    let out = output_lines(&r);

    let listed = out
        .iter()
        .filter(|l| l.contains("✓ set") || l.contains("✗ not set"))
        .count();
    assert_eq!(
        listed,
        nini_ai::provider_registry::all().count(),
        "/login must list every registered provider exactly once: {out:?}"
    );
    for entry in nini_ai::provider_registry::all() {
        assert!(
            out.iter()
                .any(|l| l.starts_with(&format!("{}: ", entry.id))),
            "/login is missing the provider {:?}: {out:?}",
            entry.id
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// R11：`openai-compat` 还差一个 base URL，输出里必须说清楚
/// （密钥之外没有地址就没法发请求）。
#[test]
fn login_mentions_the_base_url_openai_compat_needs() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("login-base-url");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(&mut state, &mut settings, CommandId::Login, "openai-compat");
    assert!(r.error.is_none(), "/login errored: {:?}", r.error);
    let out = output_lines(&r);
    assert!(
        out.iter().any(|l| l.contains("OPENAI_BASE_URL")),
        "/login openai-compat must mention OPENAI_BASE_URL: {out:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// R11：`/logout` 的清单同样来自注册表（语义不变：只告诉用户该取消哪个变量）。
#[test]
fn logout_knows_the_registered_providers() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("logout");
    std::env::set_var("NINI_AGENT_DIR", dir.join("agent"));

    let mut state = AppState::new("test-model");
    let mut settings = SettingsManager::default();
    let r = dispatch(
        &mut state,
        &mut settings,
        CommandId::Logout,
        "no-such-provider",
    );
    assert!(r.error.is_none(), "/logout errored: {:?}", r.error);
    let out = output_lines(&r);
    assert!(
        out.iter().any(|l| l.contains("unknown provider")),
        "an unknown provider must be reported: {out:?}"
    );
    let known_line = out
        .iter()
        .find(|l| l.starts_with("known: "))
        .expect("the known list must be printed");
    for entry in nini_ai::provider_registry::all() {
        assert!(
            known_line.contains(entry.id),
            "/logout's known list is missing {:?}: {known_line:?}",
            entry.id
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
