//! KeybindingsManager: load and merge user-defined keybindings.
//!
//! Pi loads `~/.pi/agent/keybindings.json` and project-level
//! `./.pi/keybindings.json` to let users override default bindings. nini
//! follows the same pattern with a Rust-typed configuration format.
//!
//! User bindings take precedence over the built-in `default_keymap()`.
//! Scope priority: project > user > built-in.
//!
//! Format (JSON, example):
//! ```json
//! {
//!   "tui.editor.tab": "AcceptCompletionOrInsertTab",
//!   "app.exit":        "Quit"
//! }
//! ```
//!
//! Valid action names map to the variants of `KeyAction` (camelCase).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::keys::{Key, KeyAction, KeyModifiers};


/// Raw user-supplied override file format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserKeybindings {
    /// Map of action names → key spec strings, e.g. `"Ctrl+L"`.
    #[serde(default)]
    pub bindings: std::collections::BTreeMap<String, String>,
}

/// Loaded and merged keybindings state.
#[derive(Debug, Clone, Default)]
pub struct KeybindingsManager {
    /// Overrides keyed by action name (after parsing).
    overrides: Vec<(KeyAction, Key)>,
    /// Source file paths for diagnostics.
    sources: Vec<PathBuf>,
}

impl KeybindingsManager {
    /// Construct an empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load from `~/.pi/agent/keybindings.json` and (if `cwd` is given)
    /// `./.pi/keybindings.json`. Missing files are silently skipped.
    pub fn load(cwd: Option<&Path>) -> Self {
        let mut mgr = Self::new();
        if let Some(path) = user_keybindings_path() {
            if path.exists() {
                if let Err(e) = mgr.load_file(&path) {
                    eprintln!("[nini] keybindings: {} load failed: {e}", path.display());
                } else {
                    mgr.sources.push(path);
                }
            }
        }
        if let Some(cwd) = cwd {
            let path = cwd.join(".pi").join("keybindings.json");
            if path.exists() {
                if let Err(e) = mgr.load_file(&path) {
                    eprintln!("[nini] keybindings: {} load failed: {e}", path.display());
                } else {
                    mgr.sources.push(path);
                }
            }
        }
        mgr
    }

    /// Load overrides from a single file.
    pub fn load_file(&mut self, path: &Path) -> io::Result<()> {
        let raw = fs::read_to_string(path)?;
        let user: UserKeybindings = serde_json::from_str(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        for (action_name, key_spec) in user.bindings {
            if let Some(action) = parse_action(&action_name) {
                if let Some(key) = parse_key_spec(&key_spec) {
                    // Replace any existing override for the same action.
                    self.overrides.retain(|(a, _)| *a != action);
                    self.overrides.push((action, key));
                } else {
                    eprintln!(
                        "[nini] keybindings: {} unknown key spec '{}'",
                        path.display(),
                        key_spec
                    );
                }
            } else {
                eprintln!(
                    "[nini] keybindings: {} unknown action '{}'",
                    path.display(),
                    action_name
                );
            }
        }
        Ok(())
    }

    /// Source files that contributed overrides.
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    /// Number of overrides loaded.
    pub fn len(&self) -> usize {
        self.overrides.len()
    }

    /// True if no overrides are loaded.
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }

    /// Build the effective keymap: built-in `default_keymap()` plus user
    /// overrides. Both built-in and override bindings coexist — if user maps
    /// `Quit` to `Ctrl+Q`, the built-in `Ctrl+D → Quit` still works.
    pub fn effective_keymap(&self) -> Vec<(Key, KeyAction)> {
        let mut map: Vec<(Key, KeyAction)> = crate::default_keymap()
            .into_iter()
            .map(|b| (b.key, b.action))
            .collect();
        // Add user overrides. If a user-defined key was previously bound to
        // a different action in built-ins, the user's action wins (last
        // match in resolve loop). To fully replace a binding, users should
        // include the same action with their preferred key — there is no
        // way to "unbind" a built-in key in v1.
        for (action, key) in &self.overrides {
            map.push((*key, *action));
        }
        map
    }

    /// Resolve a key against the effective keymap.
    /// User-defined bindings take priority over built-ins.
    pub fn resolve(&self, key: Key) -> KeyAction {
        // User overrides first.
        for (action, k) in &self.overrides {
            if *k == key {
                return *action;
            }
        }
        // Built-in keymap.
        let km = crate::default_keymap();
        for b in &km {
            if b.key == key {
                return b.action;
            }
        }
        // Fallback: printable char with no Ctrl/Alt.
        if !key.modifiers.contains(KeyModifiers::CTRL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            if let crossterm::event::KeyCode::Char(c) = key.code {
                return KeyAction::Insert(c);
            }
        }
        KeyAction::Noop
    }
}

/// Default path: `~/.pi/agent/keybindings.json`.
pub fn user_keybindings_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".pi").join("agent").join("keybindings.json"))
}

/// Parse `"Ctrl+L"`, `"Shift+Tab"`, `"F1"`, etc. into a `Key`.
pub fn parse_key_spec(spec: &str) -> Option<Key> {
    use crossterm::event::KeyCode;
    let mut mods = KeyModifiers::NONE;
    let mut key_part = spec;
    for part in spec.split('+') {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => mods |= KeyModifiers::CTRL,
            "alt" => mods |= KeyModifiers::ALT,
            "shift" => mods |= KeyModifiers::SHIFT,
            _ => key_part = part,
        }
    }
    let code = match key_part.to_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        s if s.starts_with('f') && s.len() > 1 => {
            if let Ok(n) = s[1..].parse::<u8>() {
                KeyCode::F(n)
            } else {
                return None;
            }
        }
        s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap()),
        _ => return None,
    };
    Some(Key::new(code, mods))
}

/// Map an action name (camelCase) to its `KeyAction`.
pub fn parse_action(name: &str) -> Option<KeyAction> {
    use KeyAction::*;
    Some(match name {
        // Built-in actions
        "Insert" => Insert('\0'), // placeholder — Insert requires char
        "MoveLeft" => MoveLeft,
        "MoveRight" => MoveRight,
        "MoveUp" => MoveUp,
        "MoveDown" => MoveDown,
        "MoveLineStart" => MoveLineStart,
        "MoveLineEnd" => MoveLineEnd,
        "MoveWordLeft" => MoveWordLeft,
        "MoveWordRight" => MoveWordRight,
        "Backspace" => Backspace,
        "Delete" => Delete,
        "KillToLineStart" => KillToLineStart,
        "KillToLineEnd" => KillToLineEnd,
        "KillWordBackward" => KillWordBackward,
        "Submit" => Submit,
        "Newline" => Newline,
        "Abort" => Abort,
        "Quit" => Quit,
        "SwitchModel" => SwitchModel,
        "ShowHelp" => ShowHelp,
        "ClearInput" => ClearInput,
        "ScrollUp" => ScrollUp,
        "ScrollDown" => ScrollDown,
        "Noop" => Noop,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use tempfile::TempDir;

    #[test]
    fn empty_manager_has_no_overrides() {
        let m = KeybindingsManager::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn parse_ctrl_l() {
        let k = parse_key_spec("Ctrl+L").unwrap();
        assert_eq!(k.modifiers, KeyModifiers::CTRL);
        assert!(matches!(k.code, crossterm::event::KeyCode::Char('l')));
    }

    #[test]
    fn parse_shift_tab() {
        let k = parse_key_spec("Shift+Tab").unwrap();
        assert_eq!(k.modifiers, KeyModifiers::SHIFT);
        assert!(matches!(k.code, crossterm::event::KeyCode::Tab));
    }

    #[test]
    fn parse_f1() {
        let k = parse_key_spec("F1").unwrap();
        assert!(matches!(k.code, crossterm::event::KeyCode::F(1)));
    }

    #[test]
    fn parse_alt_enter() {
        let k = parse_key_spec("Alt+Enter").unwrap();
        assert_eq!(k.modifiers, KeyModifiers::ALT);
        assert!(matches!(k.code, crossterm::event::KeyCode::Enter));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_key_spec("BogusKey").is_none());
    }

    #[test]
    fn parse_action_known() {
        assert!(matches!(parse_action("Quit"), Some(KeyAction::Quit)));
        assert!(matches!(parse_action("Submit"), Some(KeyAction::Submit)));
    }

    #[test]
    fn parse_action_unknown() {
        assert!(parse_action("NonExistent").is_none());
    }

    #[test]
    fn load_file_replaces_default_binding() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.json");
        std::fs::write(
            &path,
            r#"{
                "bindings": {
                    "Quit": "Ctrl+Q",
                    "SwitchModel": "Ctrl+M"
                }
            }"#,
        )
        .unwrap();

        let mut mgr = KeybindingsManager::new();
        mgr.load_file(&path).unwrap();
        assert_eq!(mgr.len(), 2);

        let map = mgr.effective_keymap();
        // Ctrl+Q → Quit (user)
        let quit_key = Key::new(crossterm::event::KeyCode::Char('q'), KeyModifiers::CTRL);
        assert_eq!(mgr.resolve(quit_key), KeyAction::Quit);
        // Ctrl+D → should still quit (built-in not overridden)
        let d_key = Key::new(crossterm::event::KeyCode::Char('d'), KeyModifiers::CTRL);
        assert_eq!(mgr.resolve(d_key), KeyAction::Quit);
    }

    #[test]
    fn load_unknown_action_warns_but_doesnt_crash() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.json");
        std::fs::write(
            &path,
            r#"{ "bindings": { "NonExistent": "Ctrl+X" } }"#,
        )
        .unwrap();
        let mut mgr = KeybindingsManager::new();
        mgr.load_file(&path).unwrap();
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn later_overrides_replace_earlier() {
        let tmp = TempDir::new().unwrap();
        let p1 = tmp.path().join("a.json");
        let p2 = tmp.path().join("b.json");
        std::fs::write(&p1, r#"{ "bindings": { "Quit": "Ctrl+Q" } }"#).unwrap();
        std::fs::write(&p2, r#"{ "bindings": { "Quit": "Ctrl+X" } }"#).unwrap();

        let mut mgr = KeybindingsManager::new();
        mgr.load_file(&p1).unwrap();
        mgr.load_file(&p2).unwrap();
        // p2's Ctrl+X should win.
        let x = Key::new(crossterm::event::KeyCode::Char('x'), KeyModifiers::CTRL);
        assert_eq!(mgr.resolve(x), KeyAction::Quit);
    }

    #[test]
    fn apply_to_appstate_doesnt_panic() {
        let mut state = AppState::new("test");
        // Just verify KeybindingsManager can be queried for the state's resolver
        // without panic — currently the manager is independent of state but the
        // path forward is to thread it through `run()` and pass to handle_key.
        let mgr = KeybindingsManager::new();
        let _ = mgr.effective_keymap();
        let _ = state.model; // suppress unused warning
    }
}
/// Read and parse the user keybindings.json file.
/// Returns a vector of (KeyAction, Key) overrides.
/// Silently returns an empty Vec if the file doesn't exist or is malformed.
pub fn load_user_overrides() -> Vec<(KeyAction, Key)> {
    use std::collections::HashMap;

    let path = match user_keybindings_path() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let map: HashMap<String, String> = match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for (key_spec, action_name) in &map {
        let Some(key) = parse_key_spec(key_spec) else { continue };
        let Some(action) = parse_action(action_name) else { continue };
        out.push((action, key));
    }
    out
}
