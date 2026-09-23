//! Key types and bindings (Pi-compatible).
//!
//! Translates `crossterm::event::KeyEvent` into our own `Key` struct, then
//! resolves against `KeyBinding` rules to produce `KeyAction`s the app
//! actually understands.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers as CtMods};

/// Modifier mask. Wraps crossterm's `KeyModifiers` so we don't leak
/// crossterm into the rest of the crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KeyModifiers(pub u8);

impl KeyModifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(0b001);
    pub const CTRL: Self = Self(0b010);
    pub const ALT: Self = Self(0b100);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for KeyModifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for KeyModifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// A normalized key event. Decoupled from `crossterm` so the rest of the TUI
/// is testable without a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl Key {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// A plain printable character with no modifiers.
    pub fn char(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Backspace.
    pub fn backspace() -> Self {
        Self {
            code: KeyCode::Backspace,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Enter.
    pub fn enter() -> Self {
        Self {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Esc.
    pub fn esc() -> Self {
        Self {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
        }
    }

    pub fn with_modifiers(mut self, m: KeyModifiers) -> Self {
        self.modifiers = m;
        self
    }
}

impl From<KeyEvent> for Key {
    fn from(e: KeyEvent) -> Self {
        let mut m = KeyModifiers::NONE;
        if e.modifiers.contains(CtMods::SHIFT) {
            m |= KeyModifiers::SHIFT;
        }
        if e.modifiers.contains(CtMods::CONTROL) {
            m |= KeyModifiers::CTRL;
        }
        if e.modifiers.contains(CtMods::ALT) {
            m |= KeyModifiers::ALT;
        }
        Self {
            code: e.code,
            modifiers: m,
        }
    }
}

/// High-level action a key press should map to. The TUI runtime resolves a
/// `Key` to a `KeyAction` via the `KeyBinding` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    /// Insert this character at the cursor.
    Insert(char),
    /// Move cursor.
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
    MoveWordLeft,
    MoveWordRight,
    /// Edit operations.
    Backspace,
    Delete,
    KillToLineStart,
    KillToLineEnd,
    KillWordBackward,
    KillWordForward,
    Yank,
    YankPop,
    Undo,
    /// Submit the input (Enter without Shift).
    Submit,
    /// Insert a newline (Shift+Enter).
    Newline,
    /// Abort / cancel current operation.
    Abort,
    /// Quit the TUI.
    Quit,
    /// Switch model (placeholder).
    SwitchModel,
    /// Cycle to next model in `models_cycle` list (Ctrl+P).
    CycleModelNext,
    /// Cycle to previous model in `models_cycle` list (Ctrl+Shift+P).
    CycleModelPrev,
    /// Cycle to next thinking level (Ctrl+T).
    CycleThinkingNext,
    /// Cycle to previous thinking level (Ctrl+Shift+T).
    CycleThinkingPrev,
    /// Show help.
    ShowHelp,
    /// Clear current input.
    ClearInput,
    /// Paste an image from the system clipboard. Reads RGBA from
    /// arboard, encodes to PNG, saves to a temp file, and inserts
    /// the `[pasted image: <path>]` description into the prompt.
    PasteImage,
    /// Accept the highlighted completion popup item.
    /// Falls back to inserting a literal Tab if no popup is visible.
    AcceptCompletionOrInsertTab,
    /// Scroll conversation up/down.
    ScrollUp,
    ScrollDown,
    /// Toggle collapsed/expanded state on the focused transcript line.
    /// Default binding: Ctrl+O. Operates on the line at the current
    /// scroll offset (so the line currently visible at the top of the
    /// transcript pane toggles).
    ToggleCollapse,
    /// Open the transcript full-text search (F019). The bare `/` key
    /// (no Shift/Ctrl) when no slash-command popup is visible.
    OpenSearch,
    /// Open the command palette (F015). Ctrl+K on Linux/Windows,
    /// ⌘K on macOS — both bound to the same action.
    OpenCommandPalette,
    /// Open the current input buffer in the user's external
    /// editor ($VISUAL / $EDITOR / nano / vi) for multi-line
    /// editing (F020). Ctrl+G is the standard binding.
    OpenExternalEditor,
    /// Unhandled (no binding matched).
    Noop,
}

/// A single binding rule: matches a `Key` (modifiers + code), produces a
/// `KeyAction`. Modifiers are part of the match — `Enter` only matches without
/// Ctrl/Alt by default; `Shift+Enter` is the newline variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    pub key: Key,
    pub action: KeyAction,
}

/// Default Pi-compatible keymap.
pub fn default_keymap() -> Vec<KeyBinding> {
    use KeyAction::*;
    let ctrl = KeyModifiers::CTRL;
    vec![
        // Submit / newline
        KeyBinding {
            key: Key::enter(),
            action: Submit,
        },
        KeyBinding {
            key: Key::enter().with_modifiers(KeyModifiers::SHIFT),
            action: Newline,
        },
        // Abort / quit
        KeyBinding {
            key: Key::esc(),
            action: Abort,
        },
        KeyBinding {
            key: Key::new(KeyCode::Char('c'), ctrl),
            action: Abort,
        },
        KeyBinding {
            key: Key::new(KeyCode::Char('d'), ctrl),
            action: Quit,
        },
        // Switch model
        KeyBinding {
            key: Key::new(KeyCode::Char('l'), ctrl),
            action: SwitchModel,
        },
        // Cycle model next (Ctrl+P)
        KeyBinding {
            key: Key::new(KeyCode::Char('p'), ctrl),
            action: CycleModelNext,
        },
        // Cycle model prev (Ctrl+Shift+P)
        KeyBinding {
            key: Key::new(KeyCode::Char('p'), ctrl | KeyModifiers::SHIFT),
            action: CycleModelPrev,
        },
        // Cycle thinking next (Ctrl+T)
        KeyBinding {
            key: Key::new(KeyCode::Char('t'), ctrl),
            action: CycleThinkingNext,
        },
        // Cycle thinking prev (Ctrl+Shift+T)
        KeyBinding {
            key: Key::new(KeyCode::Char('t'), ctrl | KeyModifiers::SHIFT),
            action: CycleThinkingPrev,
        },
        // Help
        KeyBinding {
            key: Key::new(KeyCode::F(1), KeyModifiers::NONE),
            action: ShowHelp,
        },
        // Toggle collapsed/expanded on focused transcript line (Ctrl+O)
        KeyBinding {
            key: Key::new(KeyCode::Char('o'), ctrl),
            action: ToggleCollapse,
        },
        // Cursor motion
        KeyBinding {
            key: Key::new(KeyCode::Left, KeyModifiers::NONE),
            action: MoveLeft,
        },
        KeyBinding {
            key: Key::new(KeyCode::Right, KeyModifiers::NONE),
            action: MoveRight,
        },
        KeyBinding {
            key: Key::new(KeyCode::Up, KeyModifiers::NONE),
            action: MoveUp,
        },
        KeyBinding {
            key: Key::new(KeyCode::Down, KeyModifiers::NONE),
            action: MoveDown,
        },
        KeyBinding {
            key: Key::new(KeyCode::Home, KeyModifiers::NONE),
            action: MoveLineStart,
        },
        KeyBinding {
            key: Key::new(KeyCode::End, KeyModifiers::NONE),
            action: MoveLineEnd,
        },
        KeyBinding {
            key: Key::new(KeyCode::Left, ctrl),
            action: MoveWordLeft,
        },
        KeyBinding {
            key: Key::new(KeyCode::Right, ctrl),
            action: MoveWordRight,
        },
        // Editing
        KeyBinding {
            key: Key::backspace(),
            action: Backspace,
        },
        KeyBinding {
            key: Key::new(KeyCode::Delete, KeyModifiers::NONE),
            action: Delete,
        },
        KeyBinding {
            key: Key::new(KeyCode::Char('a'), ctrl),
            action: MoveLineStart,
        },
        // Ctrl+K → kill from cursor to end of line. Matches readline /
        // bash / Pi. The command palette (F015) lives on Ctrl+Shift+K
        // instead so we don't break either muscle memory.
        KeyBinding {
            key: Key::new(KeyCode::Char('k'), ctrl),
            action: KillToLineEnd,
        },
        KeyBinding {
            key: Key::new(KeyCode::Char('w'), ctrl),
            action: KillWordBackward,
        },
        // Alt+d → kill word forward.
        KeyBinding {
            key: Key::new(KeyCode::Char('d'), KeyModifiers::ALT),
            action: KillWordForward,
        },
        // Ctrl+y → yank most recent kill.
        KeyBinding {
            key: Key::new(KeyCode::Char('y'), ctrl),
            action: Yank,
        },
        // Alt+y → yank-pop (rotate kill ring backward).
        KeyBinding {
            key: Key::new(KeyCode::Char('y'), KeyModifiers::ALT),
            action: YankPop,
        },
        // Ctrl+/ → undo.
        KeyBinding {
            key: Key::new(KeyCode::Char('/'), ctrl),
            action: Undo,
        },
        // Ctrl+F → open transcript search. (Binding `/` was tried first but
        // it broke typing slash commands from an empty input — see
        // F019 in docs/plans/v0.6-ux-gap-fixes.md for the follow-up.)
        KeyBinding {
            key: Key::new(KeyCode::Char('f'), ctrl),
            action: OpenSearch,
        },
        // Ctrl+Shift+K → command palette (F015). Same chord as
        // Ctrl+K with Shift added — discoverable, mirrors VSCode ⌘K
        // (Shift modifier avoids colliding with readline's Ctrl+K).
        KeyBinding {
            key: Key::new(KeyCode::Char('k'), ctrl | KeyModifiers::SHIFT),
            action: OpenCommandPalette,
        },
        // Ctrl+G → open current input in external editor (F020).
        // Readline / bash muscle memory: Ctrl+G is "edit in
        // editor" on most shells. The runtime handles the
        // suspend/resume dance.
        KeyBinding {
            key: Key::new(KeyCode::Char('g'), ctrl),
            action: OpenExternalEditor,
        },
        // Scroll
        KeyBinding {
            key: Key::new(KeyCode::PageUp, KeyModifiers::NONE),
            action: ScrollUp,
        },
        KeyBinding {
            key: Key::new(KeyCode::PageDown, KeyModifiers::NONE),
            action: ScrollDown,
        },
        // Clear input
        KeyBinding {
            key: Key::new(KeyCode::Char('u'), ctrl),
            action: ClearInput,
        },
        // Tab: accept completion if popup visible, otherwise insert literal tab.
        // Note: the runtime checks completion.is_some() and routes to
        // apply_completion vs insert.
        KeyBinding {
            key: Key::new(KeyCode::Tab, KeyModifiers::NONE),
            action: AcceptCompletionOrInsertTab,
        },
        // Ctrl+V → paste an image from the clipboard. The runtime
        // reads via arboard, encodes to PNG, saves to a tmp file, and
        // inserts the description into the prompt. Falls through to
        // Noop if the clipboard has no image.
        KeyBinding {
            key: Key::new(KeyCode::Char('v'), ctrl),
            action: PasteImage,
        },
    ]
}

/// Resolve a key against a keymap. Returns `Noop` if nothing matches.
pub fn resolve(keymap: &[KeyBinding], key: Key) -> KeyAction {
    for b in keymap {
        if b.key == key {
            return b.action;
        }
    }
    // Fallback: printable char with no Ctrl/Alt modifiers → Insert(c).
    // Shift is allowed because terminals send SHIFT+letter for uppercase
    // letters and most editors (bash, readline, Pi) accept it as a plain
    // letter. This matches what `Pi` does and lets typed text reach the
    // input buffer when the user types uppercase letters.
    if !key.modifiers.contains(KeyModifiers::CTRL)
        && !key.modifiers.contains(KeyModifiers::ALT)
    {
        if let KeyCode::Char(c) = key.code {
            return KeyAction::Insert(c);
        }
    }
    KeyAction::Noop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_basic_keys() {
        let km = default_keymap();
        assert_eq!(resolve(&km, Key::enter()), KeyAction::Submit);
        assert_eq!(resolve(&km, Key::esc()), KeyAction::Abort);
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Char('c'), KeyModifiers::CTRL)),
            KeyAction::Abort
        );
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Char('d'), KeyModifiers::CTRL)),
            KeyAction::Quit
        );
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Char('l'), KeyModifiers::CTRL)),
            KeyAction::SwitchModel
        );
    }

    #[test]
    fn shift_enter_is_newline() {
        let km = default_keymap();
        let k = Key::enter().with_modifiers(KeyModifiers::SHIFT);
        assert_eq!(resolve(&km, k), KeyAction::Newline);
    }

    #[test]
    fn printable_char_inserts() {
        let km = default_keymap();
        // Unbound char → Insert
        assert_eq!(resolve(&km, Key::char('h')), KeyAction::Insert('h'));
        // Shift+letter (uppercase) is still Insert (matches bash/readline)
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Char('X'), KeyModifiers::SHIFT)),
            KeyAction::Insert('X'),
        );
        // Ctrl+letter is NOT Insert — must be a binding or Noop
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Char('a'), KeyModifiers::CTRL)),
            KeyAction::MoveLineStart,
        );
    }

    #[test]

    #[test]
    fn ctrl_o_binds_to_toggle_collapse() {
        use crate::keys::Key as K;
        let km = default_keymap();
        // Ctrl+O should map to ToggleCollapse.
        let k = K::new(KeyCode::Char('o'), KeyModifiers::CTRL);
        assert_eq!(resolve(&km, k), KeyAction::ToggleCollapse);
        // Plain 'o' (no Ctrl) should still insert, not toggle.
        assert_eq!(resolve(&km, K::char('o')), KeyAction::Insert('o'));
    }

    #[test]
    fn ctrl_k_kills_to_line_end() {
        // F015 (revised): Ctrl+K stays as KillToLineEnd to preserve
        // readline muscle memory; palette moved to Ctrl+Shift+K.
        use crate::keys::Key as K;
        let km = default_keymap();
        assert_eq!(
            resolve(&km, K::new(KeyCode::Char('k'), KeyModifiers::CTRL)),
            KeyAction::KillToLineEnd
        );
    }

    #[test]
    fn ctrl_shift_k_opens_palette() {
        // F015: Ctrl+Shift+K opens the command palette.
        use crate::keys::Key as K;
        let km = default_keymap();
        assert_eq!(
            resolve(
                &km,
                K::new(KeyCode::Char('k'), KeyModifiers::CTRL | KeyModifiers::SHIFT)
            ),
            KeyAction::OpenCommandPalette
        );
    }

    #[test]
    fn ctrl_g_opens_external_editor() {
        // F020: Ctrl+G opens the current input in $VISUAL/$EDITOR.
        use crate::keys::Key as K;
        let km = default_keymap();
        assert_eq!(
            resolve(&km, K::new(KeyCode::Char('g'), KeyModifiers::CTRL)),
            KeyAction::OpenExternalEditor
        );
    }

    fn arrow_keys_move() {
        let km = default_keymap();
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Left, KeyModifiers::NONE)),
            KeyAction::MoveLeft
        );
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Right, KeyModifiers::NONE)),
            KeyAction::MoveRight
        );
    }

    #[test]
    fn ctrl_arrows_move_word() {
        let km = default_keymap();
        let ctrl = KeyModifiers::CTRL;
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Left, ctrl)),
            KeyAction::MoveWordLeft
        );
        assert_eq!(
            resolve(&km, Key::new(KeyCode::Right, ctrl)),
            KeyAction::MoveWordRight
        );
    }

    #[test]
    fn from_crossterm_event() {
        let evt = KeyEvent::new(KeyCode::Char('x'), CtMods::CONTROL);
        let key: Key = evt.into();
        assert_eq!(key.code, KeyCode::Char('x'));
        assert_eq!(key.modifiers, KeyModifiers::CTRL);
    }

    #[test]
    fn empty_keymap_returns_noop_or_insert() {
        let km: Vec<KeyBinding> = vec![];
        assert_eq!(resolve(&km, Key::char('a')), KeyAction::Insert('a'));
        assert_eq!(resolve(&km, Key::esc()), KeyAction::Noop);
    }

    #[test]
    fn modifiers_bit_ops() {
        let m = KeyModifiers::CTRL | KeyModifiers::SHIFT;
        assert!(m.contains(KeyModifiers::CTRL));
        assert!(m.contains(KeyModifiers::SHIFT));
        assert!(!m.contains(KeyModifiers::ALT));
    }
}
