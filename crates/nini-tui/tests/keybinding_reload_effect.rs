//! Oracle: `/reload` 改键位之后，**按键解析**必须真的变。
//!
//! # The defect (found while implementing A4)
//!
//! v0.11 (B3) 把用户键位覆盖做成了「可重载的缓存」，但写成了**两份**：
//!
//! ```text
//! resolve_with_user_overrides()  →  static OVERRIDES  (A)
//! reload_user_overrides()        →  static OVERRIDES  (B)   ← 另一个
//! ```
//!
//! 于是 `/reload` 把自己读到的覆盖写进缓存 B，而按键解析读的是缓存 A：
//! **键位从头到尾没有变过**。而 `/reload` 的输出会说
//! 「N user override(s) in effect」——一句关于它**并没有做到**的事的话。
//!
//! 这正是 B3 那条差距的**核心**（「`/reload` 的范围远小于 Pi」）：
//! 键位要能被重载，重载必须作用在**解析用的那份**状态上。
//!
//! # 钉住什么
//!
//! 1. 改写 `keybindings.json` → `/reload` → 同一个按键解析成**不同的动作**；
//! 2. 没有 `/reload` 时，解析结果**不变**（缓存确实在生效，不是每次都重读）；
//! 3. `/reload` 报告的条数与**实际生效**的条数一致。
//!
//! 用 `nini_core::paths::agent_lock()` 做进程级隔离（环境变量是全局的）。

use nini_tui::commands::{dispatch, CommandId, CommandOutcome};
use nini_tui::keys::{Key, KeyAction, KeyCode, KeyModifiers};
use nini_tui::runtime::{reload_user_overrides, resolve_with_user_overrides};
use nini_tui::settings::SettingsManager;
use nini_tui::state::AppState;
use std::io::Write as _;

/// The key we will rebind. `Ctrl+Q` is not bound to anything by default,
/// so rebinding it makes the change observable.
fn ctrl_q() -> Key {
    Key::new(KeyCode::Char('q'), KeyModifiers::CTRL)
}

fn write_keybindings(dir: &std::path::Path, action: &str) {
    let path = dir.join("keybindings.json");
    let mut f = std::fs::File::create(path).expect("create keybindings.json");
    writeln!(f, r#"{{"bindings": {{"{}": "Ctrl+Q"}}}}"#, action).expect("write");
}

fn reload(state: &mut AppState) -> String {
    let mut settings = SettingsManager::default();
    let r = dispatch(state, &mut settings, CommandId::Reload, "");
    match r.outcome {
        CommandOutcome::Output(lines) => lines.join("\n"),
        _ => String::new(),
    }
}

#[test]
fn a_reload_changes_what_the_key_resolves_to() {
    let guard = nini_core::paths::agent_lock();
    let dir = guard.path().to_path_buf();

    // 1. No user file yet: `Ctrl+Q` is not bound to anything.
    reload_user_overrides();
    assert_ne!(
        resolve_with_user_overrides(ctrl_q()),
        KeyAction::Quit,
        "precondition: Ctrl+Q must not be bound before the file exists"
    );

    // 2. Write a user override, reload, and ask again.
    write_keybindings(&dir, "Quit");
    let mut state = AppState::new("test-model");
    let out = reload(&mut state);
    assert!(
        out.contains("keybindings: 1 user override"),
        "the reload must report the override it read: {out:?}"
    );

    assert_eq!(
        resolve_with_user_overrides(ctrl_q()),
        KeyAction::Quit,
        "**the resolution must change after /reload** — this is the whole \
         point of B3. Before the fix, the reload wrote to a different cache \
         than the one the resolver read, so the key never changed."
    );
}

#[test]
fn without_a_reload_the_resolution_does_not_change() {
    let guard = nini_core::paths::agent_lock();
    let dir = guard.path().to_path_buf();

    write_keybindings(&dir, "Quit");
    // The first read populates the cache.
    reload_user_overrides();
    assert_eq!(resolve_with_user_overrides(ctrl_q()), KeyAction::Quit);

    // Now change the file **without** reloading: the cached answer stands.
    write_keybindings(&dir, "SwitchModel");
    assert_eq!(
        resolve_with_user_overrides(ctrl_q()),
        KeyAction::Quit,
        "without a reload the cached override still wins — the cache is \
         real, not an accident of reading the file every time"
    );
}

#[test]
fn the_reported_count_matches_the_overrides_in_effect() {
    let guard = nini_core::paths::agent_lock();
    let dir = guard.path().to_path_buf();

    write_keybindings(&dir, "Quit");
    let reported = reload_user_overrides();
    assert_eq!(reported, 1, "one override in the file");

    // The count is not a decoration: the override it counted is in effect.
    assert_eq!(resolve_with_user_overrides(ctrl_q()), KeyAction::Quit);
}

/// A user who copies Pi's own `keybindings.json` must not end up with a
/// file that silently does nothing.
///
/// Pi's user file is **flat** (`references/pi-0.85.1/src/ui/keybindings.ts`'s
/// `toKeybindingsConfig` takes `Object.entries(value)` as
/// action → key spec), while nini's own shape nests them under
/// `"bindings"`. Before v0.11 (A4 review) only the nested shape parsed, so
/// a copied Pi file yielded **zero** overrides — and the module's own
/// documentation claimed the opposite.
#[test]
fn pis_flat_keybindings_file_works_too() {
    let guard = nini_core::paths::agent_lock();
    let dir = guard.path().to_path_buf();

    // Pi's shape: flat, and with Pi's dotted action names.
    let path = dir.join("keybindings.json");
    std::fs::write(&path, r#"{"app.message.followUp": "Ctrl+Q"}"#).expect("write");

    let reported = reload_user_overrides();
    assert_eq!(
        reported, 1,
        "a Pi-shaped keybindings file must be read, not silently ignored"
    );
    assert_eq!(
        resolve_with_user_overrides(ctrl_q()),
        KeyAction::FollowUp,
        "and the binding it declared must be the one in effect"
    );
}
