//! Theme system: a small color palette + JSON load/save for user themes.
//!
//! Two built-in themes are provided out of the box:
//! - `Theme::dark()` — the default, cool grays with blue accent (the
//!   "look and feel" Pi ships with).
//! - `Theme::light()` — pale background with darker accent.
//!
//! Users can load a custom JSON theme from `~/.pi/agent/theme.json` (or
//! a project-local `./.pi/theme.json`) and merge it on top of the built-in
//! dark theme via [`Theme::load_from_file`] / [`Theme::merge`].
//!
//! ## JSON schema
//!
//! ```json
//! {
//!   "name": "solarized-dark",
//!   "colors": {
//!     "accent": "#268bd2",
//!     "dim": "#586e75",
//!     "error": "#dc322f",
//!     ...
//!   }
//! }
//! ```
//!
//! Unknown color names are kept at their default value. Unknown JSON keys
//! are ignored. Color values accept `#RGB`, `#RRGGBB`, and `#RRGGBBAA`.

use std::collections::HashMap;
use std::path::Path;

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

/// ANSI SGR reset (`\x1b[0m`). Appended after every styled string so
/// colors don't bleed into adjacent content. Mirrors Pi's chalk-style
/// API where each `theme.fg(...)` call auto-closes with a reset.
const RESET_FMT: &str = "\x1b[0m";

/// Convert a ratatui `Style` into an ANSI SGR prefix (without the reset).
/// We only emit codes for the parts of the style that are actually set,
/// so callers can layer `theme.fg(...)` calls without resetting siblings.
fn style_to_ansi(style: Style) -> String {
    let mut codes: Vec<String> = Vec::new();
    if let Some(fg) = style.fg {
        codes.push(color_to_sgr(fg, /*is_bg=*/ false));
    }
    if let Some(bg) = style.bg {
        codes.push(color_to_sgr(bg, /*is_bg=*/ true));
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        codes.push("3".into());
    }
    if style.add_modifier.contains(Modifier::UNDERLINED) {
        codes.push("4".into());
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// Modifier-only SGR prefix (used by `Theme::bold()` / `italic()` / `underline()`).
fn modifier_to_ansi(m: Modifier) -> String {
    let mut codes: Vec<String> = Vec::new();
    if m.contains(Modifier::BOLD) {
        codes.push("1".into());
    }
    if m.contains(Modifier::ITALIC) {
        codes.push("3".into());
    }
    if m.contains(Modifier::UNDERLINED) {
        codes.push("4".into());
    }
    if codes.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// Emit an ANSI color code (truecolor or 256-color) for a ratatui `Color`.
fn color_to_sgr(c: Color, is_bg: bool) -> String {
    let base = if is_bg { 40 } else { 30 };
    match c {
        Color::Rgb(r, g, b) => format!("{};2;{};{};{}", if is_bg { 48 } else { 38 }, r, g, b),
        Color::Indexed(i) => {
            if i < 8 {
                format!("{}", base + i as u16)
            } else if i < 16 {
                format!("{};1", base + (i as u16 - 8))
            } else {
                format!("{};5;{}", if is_bg { 48 } else { 38 }, i)
            }
        }
        Color::Black => format!("{}", base),
        Color::Red => format!("{};1", base + 1),
        Color::Green => format!("{};1", base + 2),
        Color::Yellow => format!("{};1", base + 3),
        Color::Blue => format!("{};1", base + 4),
        Color::Magenta => format!("{};1", base + 5),
        Color::Cyan => format!("{};1", base + 6),
        Color::White => format!("{};1", base + 7),
        Color::Gray | Color::DarkGray => format!("{};0", base + 8), // bright black
        Color::LightRed => format!("{}", base + 9),
        Color::LightGreen => format!("{}", base + 10),
        Color::LightYellow => format!("{}", base + 11),
        Color::LightBlue => format!("{}", base + 12),
        Color::LightMagenta => format!("{}", base + 13),
        Color::LightCyan => format!("{}", base + 14),
        Color::LightGreen => format!("{}", base + 15),
        _ => String::new(),
    }
}

/// Names of every color slot nini-tui can request. The list is closed —
/// the theme system panics on unknown names so we don't silently fall back
/// to defaults (which would mask typos in callers).
///
/// v0.8.3: Aligned with Pi's `dark.json` / `light.json` (earendil-works/pi-mono
/// `packages/coding-agent/src/modes/interactive/theme/`). Pi uses ~55 slots;
/// we expose all of them so Markdown / Tool / Message / Editor / Diff / Syntax
/// rendering can use the same names as Pi's `theme.fg(...)` calls.
///
/// Legacy nini-only slots (`background`, `foreground`, `info`, `header`,
/// `code`, `link`) are kept for back-compat; they alias to Pi-equivalent
/// slots in the palettes.
pub const COLOR_NAMES: &[&str] = &[
    // === Core UI (10) — mirrors Pi ===
    "accent",
    "border",
    "borderAccent",
    "borderMuted",
    "success",
    "error",
    "warning",
    "muted",
    "dim",
    "text",
    "thinkingText",
    // === Backgrounds (8) ===
    "selectedBg",
    "scrollbarThumb",
    "searchMatchBg",
    "searchMatchText",
    "userMessageBg",
    "userMessageText",
    "customMessageBg",
    "customMessageText",
    "customMessageLabel",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "toolTitle",
    "toolOutput",
    // === Markdown (10) ===
    "mdHeading",
    "mdLink",
    "mdLinkUrl",
    "mdCode",
    "mdCodeBlock",
    "mdCodeBlockBorder",
    "mdQuote",
    "mdQuoteBorder",
    "mdHr",
    "mdListBullet",
    // === Diff (3) ===
    "toolDiffAdded",
    "toolDiffRemoved",
    "toolDiffContext",
    // === Syntax (9) — unused by nini today, kept for future synctect ===
    "syntaxComment",
    "syntaxKeyword",
    "syntaxFunction",
    "syntaxVariable",
    "syntaxString",
    "syntaxNumber",
    "syntaxType",
    "syntaxOperator",
    "syntaxPunctuation",
    // === Thinking borders (7) ===
    "thinkingOff",
    "thinkingMinimal",
    "thinkingLow",
    "thinkingMedium",
    "thinkingHigh",
    "thinkingXhigh",
    "thinkingMax",
    // === Bash mode (1) ===
    "bashMode",
    // === Legacy nini-only slots (5) — keep for back-compat ===
    "background",
    "foreground",
    "info",
    "header",
    "code",
    "link",
];

/// A theme is just a name plus a per-color palette.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    #[serde(default = "default_theme_name")]
    pub name: String,
    #[serde(default = "default_palette")]
    pub colors: HashMap<String, String>,
}

fn default_theme_name() -> String {
    "dark".to_string()
}

fn default_palette() -> HashMap<String, String> {
    dark_palette()
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            name: "dark".to_string(),
            colors: dark_palette(),
        }
    }
}

impl Theme {
    /// Built-in dark theme (cool grays + blue accent — the Pi default).
    pub fn dark() -> Self {
        Self {
            name: "dark".to_string(),
            colors: dark_palette(),
        }
    }

    /// Built-in light theme (off-white background + indigo accent).
    pub fn light() -> Self {
        Self {
            name: "light".to_string(),
            colors: light_palette(),
        }
    }

    /// Load a theme from a JSON file. Falls back to `Theme::dark()` if the
    /// file is missing; returns an error for malformed JSON or unparsable
    /// color values.
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read theme file {}: {e}", path.display()))?;
        Self::parse(&raw)
    }

    /// Parse a theme from a JSON string. Unknown top-level keys and
    /// unknown color names are ignored.
    pub fn parse(s: &str) -> Result<Self, String> {
        let raw: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| format!("invalid theme JSON: {e}"))?;
        let obj = raw
            .as_object()
            .ok_or_else(|| "theme JSON must be an object".to_string())?;
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("dark")
            .to_string();
        let mut colors = match obj.get("colors") {
            Some(v) => {
                let raw_colors = v
                    .as_object()
                    .ok_or_else(|| "\"colors\" must be an object".to_string())?;
                let mut out = HashMap::new();
                for (k, v) in raw_colors {
                    let s = v
                        .as_str()
                        .ok_or_else(|| format!("color \"{k}\" must be a string"))?;
                    out.insert(k.clone(), s.to_string());
                }
                out
            }
            None => HashMap::new(),
        };
        // Drop entries that aren't in COLOR_NAMES — they cannot influence
        // the UI but would inflate the file on re-save.
        colors.retain(|k, _| COLOR_NAMES.contains(&k.as_str()));
        Ok(Self { name, colors })
    }

    /// Merge a partial theme on top of `self`. The other theme's `colors`
    /// overwrite ours; `name` is preserved unless the other theme has a
    /// non-default name.
    pub fn merge(&mut self, other: Theme) {
        for (k, v) in other.colors {
            self.colors.insert(k, v);
        }
        if other.name != "dark" || !self.colors.is_empty() {
            self.name = other.name;
        }
    }

    /// Resolve a color slot name to a ratatui `Color`. Falls back to
    /// Foreground (or Background for `theme.background`) when the slot is
    /// unset or the stored value cannot be parsed.
    pub fn color(&self, name: &str) -> Color {
        let raw = self.colors.get(name).map(String::as_str).unwrap_or("");
        parse_color(raw).unwrap_or_else(|| default_color(name))
    }

    /// Foreground style for a named slot. Adds BOLD when the slot is one
    /// of the "weightier" variants (accent, success, error, warning, info)
    /// — this matches how Pi's default themes behave.
    pub fn fg_style(&self, name: &str) -> Style {
        let mut s = Style::default().fg(self.color(name));
        if is_emphatic(name) {
            s = s.add_modifier(Modifier::BOLD);
        }
        s
    }

    /// Background style for a named slot.
    pub fn bg_style(&self, name: &str) -> Style {
        Style::default().bg(self.color(name))
    }

    /// Convenience: apply foreground color to text and return ANSI-styled
    /// string. Mirrors Pi's `theme.fg("name", text)`. Bold is auto-applied
    /// for emphatic slots (accent, success, error, etc.).
    pub fn fg(&self, name: &str, text: &str) -> String {
        format!("{}{}{}", style_to_ansi(self.fg_style(name)), text, RESET_FMT)
    }

    /// Convenience: apply background color to text. Mirrors Pi's
    /// `theme.bg("name", text)`.
    pub fn bg(&self, name: &str, text: &str) -> String {
        format!("{}{}{}", style_to_ansi(self.bg_style(name)), text, RESET_FMT)
    }

    /// Convenience: bold. Mirrors Pi's `theme.bold(text)`.
    pub fn bold(&self, text: &str) -> String {
        format!("{}{}{}", modifier_to_ansi(Modifier::BOLD), text, RESET_FMT)
    }

    /// Convenience: italic. Mirrors Pi's `theme.italic(text)`.
    pub fn italic(&self, text: &str) -> String {
        format!("{}{}{}", modifier_to_ansi(Modifier::ITALIC), text, RESET_FMT)
    }

    /// Convenience: underline. Mirrors Pi's `theme.underline(text)`.
    pub fn underline(&self, text: &str) -> String {
        format!(
            "{}{}{}",
            modifier_to_ansi(Modifier::UNDERLINED),
            text,
            RESET_FMT
        )
    }

    /// Convenience for tests: dump the resolved color palette as a flat
    /// list of (slot, color-hex) pairs.
    pub fn resolved_palette(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for slot in COLOR_NAMES {
            let c = self.color(slot);
            out.push((slot.to_string(), color_to_hex(c)));
        }
        out
    }
}

/// Slots that get a BOLD modifier by default when `fg_style` is called.
/// Mirrors the "weightier" set in Pi's themes (status pills, accent text,
/// tool results).
fn is_emphatic(name: &str) -> bool {
    matches!(
        name,
        "accent"
            | "borderAccent"
            | "success"
            | "error"
            | "warning"
            | "info"
            | "header"
            | "toolTitle"
            | "customMessageLabel"
            | "mdHeading"
    )
}

/// Fallback color (ANSI-16 indexed) when a slot is missing from the
/// palette JSON. Most slots have a sensible default; only the truly
/// Pi-specific backgrounds (`selectedBg`, `toolSuccessBg`, etc.) fall
/// through to a muted gray.
fn default_color(name: &str) -> Color {
    match name {
        // Core UI
        "accent" | "border" | "borderAccent" | "info" => Color::Cyan,
        "borderMuted" | "dim" | "muted" | "thinkingText" => Color::DarkGray,
        "success" | "bashMode" => Color::Green,
        "warning" => Color::Yellow,
        "error" => Color::Red,
        "text" | "header" => Color::White,
        // Backgrounds
        "background" => Color::Black,
        "selectedBg"
        | "scrollbarThumb"
        | "searchMatchBg"
        | "userMessageBg"
        | "customMessageBg"
        | "toolPendingBg"
        | "toolSuccessBg"
        | "toolErrorBg" => Color::DarkGray,
        "searchMatchText" | "userMessageText" | "customMessageText" | "toolOutput" => {
            Color::White
        }
        "customMessageLabel" | "toolTitle" => Color::Cyan,
        // Markdown
        "mdHeading" => Color::Yellow,
        "mdLink" | "link" => Color::Blue,
        "mdLinkUrl" => Color::DarkGray,
        "mdCode" | "code" => Color::Magenta,
        "mdCodeBlock" => Color::Green,
        "mdCodeBlockBorder" | "mdQuote" | "mdQuoteBorder" | "mdHr" => Color::DarkGray,
        "mdListBullet" => Color::Cyan,
        // Diff
        "toolDiffAdded" => Color::Green,
        "toolDiffRemoved" => Color::Red,
        "toolDiffContext" => Color::DarkGray,
        // Syntax (fallback only — real values come from palette)
        "syntaxComment" => Color::Green,
        "syntaxKeyword" => Color::Blue,
        "syntaxFunction" | "syntaxVariable" | "syntaxType" => Color::Cyan,
        "syntaxString" => Color::Yellow,
        "syntaxNumber" => Color::Magenta,
        "syntaxOperator" | "syntaxPunctuation" => Color::White,
        // Thinking borders
        "thinkingOff" | "thinkingMinimal" | "thinkingLow" | "thinkingMedium" | "thinkingHigh"
        | "thinkingXhigh" | "thinkingMax" => Color::Magenta,
        // Legacy
        "foreground" => Color::White,
        _ => Color::White,
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('#') {
        match rest.len() {
            3 => {
                // #RGB → expand to #RRGGBB
                let r = u8::from_str_radix(&rest[0..1], 16).ok()?;
                let g = u8::from_str_radix(&rest[1..2], 16).ok()?;
                let b = u8::from_str_radix(&rest[2..3], 16).ok()?;
                Some(Color::Rgb(r * 16 + r, g * 16 + g, b * 16 + b))
            }
            6 => {
                let r = u8::from_str_radix(&rest[0..2], 16).ok()?;
                let g = u8::from_str_radix(&rest[2..4], 16).ok()?;
                let b = u8::from_str_radix(&rest[4..6], 16).ok()?;
                Some(Color::Rgb(r, g, b))
            }
            8 => {
                let r = u8::from_str_radix(&rest[0..2], 16).ok()?;
                let g = u8::from_str_radix(&rest[2..4], 16).ok()?;
                let b = u8::from_str_radix(&rest[4..6], 16).ok()?;
                Some(Color::Rgb(r, g, b))
            }
            _ => None,
        }
    } else if let Some(idx) = ansi_index(s) {
        Some(Color::Indexed(idx))
    } else {
        None
    }
}

fn ansi_index(s: &str) -> Option<u8> {
    let s = s.to_ascii_lowercase();
    let named = match s.as_str() {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "white" => 7,
        "bright_black" | "gray" | "grey" => 8,
        "bright_red" => 9,
        "bright_green" => 10,
        "bright_yellow" => 11,
        "bright_blue" => 12,
        "bright_magenta" => 13,
        "bright_cyan" => 14,
        "bright_white" => 15,
        _ => return None,
    };
    Some(named)
}

fn color_to_hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("ansi({i})"),
        Color::Black => "#000000".to_string(),
        Color::Red => "#aa0000".to_string(),
        Color::Green => "#00aa00".to_string(),
        Color::Yellow => "#aa5500".to_string(),
        Color::Blue => "#0000aa".to_string(),
        Color::Magenta => "#aa00aa".to_string(),
        Color::Cyan => "#00aaaa".to_string(),
        Color::White => "#aaaaaa".to_string(),
        Color::DarkGray => "#555555".to_string(),
        Color::LightRed => "#ff5555".to_string(),
        Color::LightGreen => "#55ff55".to_string(),
        Color::LightYellow => "#ffff55".to_string(),
        Color::LightBlue => "#5555ff".to_string(),
        Color::LightMagenta => "#ff55ff".to_string(),
        Color::LightCyan => "#55ffff".to_string(),
        Color::Gray => "#aaaaaa".to_string(),
        _ => "#ffffff".to_string(),
    }
}

/// Dark palette — values copied from Pi's `dark.json` with `vars` resolved
/// to their literal hex equivalents. Source:
/// `@earendil-works/pi-coding-agent/dist/modes/interactive/theme/dark.json`.
fn dark_palette() -> HashMap<String, String> {
    let mut m = HashMap::new();
    // Core UI
    m.insert("accent".into(), "#8abeb7".into());
    m.insert("border".into(), "#5f87ff".into());
    m.insert("borderAccent".into(), "#00d7ff".into());
    m.insert("borderMuted".into(), "#505050".into());
    m.insert("success".into(), "#b5bd68".into());
    m.insert("error".into(), "#cc6666".into());
    m.insert("warning".into(), "#ffff00".into());
    m.insert("muted".into(), "#808080".into());
    m.insert("dim".into(), "#666666".into());
    m.insert("text".into(), "#d4d4d4".into());
    m.insert("thinkingText".into(), "#808080".into());
    // Backgrounds
    m.insert("selectedBg".into(), "#3a3a4a".into());
    m.insert("scrollbarThumb".into(), "#3a3a4a".into());
    m.insert("searchMatchBg".into(), "#3a3a4a".into());
    m.insert("searchMatchText".into(), "#d4d4d4".into());
    m.insert("userMessageBg".into(), "#343541".into());
    m.insert("userMessageText".into(), "#d4d4d4".into());
    m.insert("customMessageBg".into(), "#2d2838".into());
    m.insert("customMessageText".into(), "#d4d4d4".into());
    m.insert("customMessageLabel".into(), "#9575cd".into());
    m.insert("toolPendingBg".into(), "#282832".into());
    m.insert("toolSuccessBg".into(), "#283228".into());
    m.insert("toolErrorBg".into(), "#3c2828".into());
    m.insert("toolTitle".into(), "#d4d4d4".into());
    m.insert("toolOutput".into(), "#808080".into());
    // Markdown
    m.insert("mdHeading".into(), "#f0c674".into());
    m.insert("mdLink".into(), "#81a2be".into());
    m.insert("mdLinkUrl".into(), "#666666".into());
    m.insert("mdCode".into(), "#8abeb7".into());
    m.insert("mdCodeBlock".into(), "#b5bd68".into());
    m.insert("mdCodeBlockBorder".into(), "#808080".into());
    m.insert("mdQuote".into(), "#808080".into());
    m.insert("mdQuoteBorder".into(), "#808080".into());
    m.insert("mdHr".into(), "#808080".into());
    m.insert("mdListBullet".into(), "#8abeb7".into());
    // Diff
    m.insert("toolDiffAdded".into(), "#b5bd68".into());
    m.insert("toolDiffRemoved".into(), "#cc6666".into());
    m.insert("toolDiffContext".into(), "#808080".into());
    // Syntax (unused today; reserved for future synctect integration)
    m.insert("syntaxComment".into(), "#6A9955".into());
    m.insert("syntaxKeyword".into(), "#569CD6".into());
    m.insert("syntaxFunction".into(), "#DCDCAA".into());
    m.insert("syntaxVariable".into(), "#9CDCFE".into());
    m.insert("syntaxString".into(), "#CE9178".into());
    m.insert("syntaxNumber".into(), "#B5CEA8".into());
    m.insert("syntaxType".into(), "#4EC9B0".into());
    m.insert("syntaxOperator".into(), "#D4D4D4".into());
    m.insert("syntaxPunctuation".into(), "#D4D4D4".into());
    // Thinking
    m.insert("thinkingOff".into(), "#505050".into());
    m.insert("thinkingMinimal".into(), "#6e6e6e".into());
    m.insert("thinkingLow".into(), "#5f87af".into());
    m.insert("thinkingMedium".into(), "#81a2be".into());
    m.insert("thinkingHigh".into(), "#b294bb".into());
    m.insert("thinkingXhigh".into(), "#d183e8".into());
    m.insert("thinkingMax".into(), "#ff5fff".into());
    // Bash mode
    m.insert("bashMode".into(), "#b5bd68".into());
    // Legacy nini-only slots — aliased to Pi-equivalents
    m.insert("background".into(), "#1e1e1e".into());
    m.insert("foreground".into(), "#d4d4d4".into());
    m.insert("info".into(), "#9cdcfe".into());
    m.insert("header".into(), "#ffffff".into());
    m.insert("code".into(), "#8abeb7".into()); // alias of mdCode
    m.insert("link".into(), "#81a2be".into()); // alias of mdLink
    m
}

/// Light palette — values copied from Pi's `light.json` with `vars` resolved.
fn light_palette() -> HashMap<String, String> {
    let mut m = HashMap::new();
    // Core UI
    m.insert("accent".into(), "#5a8080".into());
    m.insert("border".into(), "#547da7".into());
    m.insert("borderAccent".into(), "#5a8080".into());
    m.insert("borderMuted".into(), "#b0b0b0".into());
    m.insert("success".into(), "#588458".into());
    m.insert("error".into(), "#aa5555".into());
    m.insert("warning".into(), "#9a7326".into());
    m.insert("muted".into(), "#6c6c6c".into());
    m.insert("dim".into(), "#767676".into());
    m.insert("text".into(), "#1f2328".into());
    m.insert("thinkingText".into(), "#6c6c6c".into());
    // Backgrounds
    m.insert("selectedBg".into(), "#d0d0e0".into());
    m.insert("scrollbarThumb".into(), "#d0d0e0".into());
    m.insert("searchMatchBg".into(), "#d0d0e0".into());
    m.insert("searchMatchText".into(), "#1f2328".into());
    m.insert("userMessageBg".into(), "#e8e8e8".into());
    m.insert("userMessageText".into(), "#1f2328".into());
    m.insert("customMessageBg".into(), "#ede7f6".into());
    m.insert("customMessageText".into(), "#1f2328".into());
    m.insert("customMessageLabel".into(), "#7e57c2".into());
    m.insert("toolPendingBg".into(), "#e8e8f0".into());
    m.insert("toolSuccessBg".into(), "#e8f0e8".into());
    m.insert("toolErrorBg".into(), "#f0e8e8".into());
    m.insert("toolTitle".into(), "#1f2328".into());
    m.insert("toolOutput".into(), "#6c6c6c".into());
    // Markdown
    m.insert("mdHeading".into(), "#9a7326".into());
    m.insert("mdLink".into(), "#547da7".into());
    m.insert("mdLinkUrl".into(), "#767676".into());
    m.insert("mdCode".into(), "#5a8080".into());
    m.insert("mdCodeBlock".into(), "#588458".into());
    m.insert("mdCodeBlockBorder".into(), "#6c6c6c".into());
    m.insert("mdQuote".into(), "#6c6c6c".into());
    m.insert("mdQuoteBorder".into(), "#6c6c6c".into());
    m.insert("mdHr".into(), "#6c6c6c".into());
    m.insert("mdListBullet".into(), "#588458".into());
    // Diff
    m.insert("toolDiffAdded".into(), "#588458".into());
    m.insert("toolDiffRemoved".into(), "#aa5555".into());
    m.insert("toolDiffContext".into(), "#6c6c6c".into());
    // Syntax
    m.insert("syntaxComment".into(), "#008000".into());
    m.insert("syntaxKeyword".into(), "#0000FF".into());
    m.insert("syntaxFunction".into(), "#795E26".into());
    m.insert("syntaxVariable".into(), "#001080".into());
    m.insert("syntaxString".into(), "#A31515".into());
    m.insert("syntaxNumber".into(), "#098658".into());
    m.insert("syntaxType".into(), "#267F99".into());
    m.insert("syntaxOperator".into(), "#000000".into());
    m.insert("syntaxPunctuation".into(), "#000000".into());
    // Thinking
    m.insert("thinkingOff".into(), "#b0b0b0".into());
    m.insert("thinkingMinimal".into(), "#767676".into());
    m.insert("thinkingLow".into(), "#547da7".into());
    m.insert("thinkingMedium".into(), "#5a8080".into());
    m.insert("thinkingHigh".into(), "#875f87".into());
    m.insert("thinkingXhigh".into(), "#8b008b".into());
    m.insert("thinkingMax".into(), "#af005f".into());
    // Bash mode
    m.insert("bashMode".into(), "#588458".into());
    // Legacy nini-only slots
    m.insert("background".into(), "#fafafa".into());
    m.insert("foreground".into(), "#1a1a1a".into());
    m.insert("info".into(), "#1f5fa8".into());
    m.insert("header".into(), "#000000".into());
    m.insert("code".into(), "#5a8080".into());
    m.insert("link".into(), "#547da7".into());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_has_all_color_names() {
        let t = Theme::dark();
        for slot in COLOR_NAMES {
            assert!(t.colors.contains_key(*slot), "missing dark slot {slot}");
        }
    }

    #[test]
    fn light_theme_has_all_color_names() {
        let t = Theme::light();
        for slot in COLOR_NAMES {
            assert!(t.colors.contains_key(*slot), "missing light slot {slot}");
        }
    }

    #[test]
    fn parse_color_accepts_short_and_long_hex() {
        assert!(matches!(parse_color("#fff"), Some(Color::Rgb(255, 255, 255))));
        assert!(matches!(parse_color("#000"), Some(Color::Rgb(0, 0, 0))));
        assert!(matches!(parse_color("#1e1e1e"), Some(Color::Rgb(30, 30, 30))));
        assert!(matches!(
            parse_color("#569cd6"),
            Some(Color::Rgb(86, 156, 214))
        ));
    }

    #[test]
    fn parse_color_rejects_invalid() {
        assert!(parse_color("not a color").is_none());
        assert!(parse_color("#zzzzzz").is_none());
        assert!(parse_color("").is_none());
        assert!(parse_color("#1").is_none()); // wrong length
    }

    #[test]
    fn parse_color_accepts_ansi_names() {
        assert!(matches!(parse_color("red"), Some(Color::Indexed(1))));
        assert!(matches!(parse_color("bright_blue"), Some(Color::Indexed(12))));
        assert!(matches!(parse_color("BLACK"), Some(Color::Indexed(0))));
    }

    #[test]
    fn fg_style_is_bold_for_emphatic_slots() {
        let t = Theme::dark();
        assert!(t.fg_style("accent").add_modifier.contains(Modifier::BOLD));
        assert!(t.fg_style("success").add_modifier.contains(Modifier::BOLD));
        assert!(t.fg_style("error").add_modifier.contains(Modifier::BOLD));
        assert!(!t.fg_style("dim").add_modifier.contains(Modifier::BOLD));
        assert!(!t.fg_style("muted").add_modifier.contains(Modifier::BOLD));
        // New emphatic slots
        assert!(t.fg_style("borderAccent").add_modifier.contains(Modifier::BOLD));
        assert!(t.fg_style("toolTitle").add_modifier.contains(Modifier::BOLD));
        assert!(t.fg_style("mdHeading").add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn fg_string_wraps_with_ansi_reset() {
        let t = Theme::dark();
        let out = t.fg("accent", "hi");
        assert!(out.starts_with("\x1b["));
        assert!(out.ends_with("\x1b[0m"));
        assert!(out.contains("hi"));
    }

    #[test]
    fn bg_string_wraps_with_ansi_reset() {
        let t = Theme::dark();
        let out = t.bg("userMessageBg", "text");
        assert!(out.starts_with("\x1b["));
        assert!(out.ends_with("\x1b[0m"));
    }

    #[test]
    fn bold_italic_underline_helpers_emit_sgr() {
        let t = Theme::dark();
        // Bold: \x1b[1m
        assert_eq!(t.bold("x"), "\x1b[1mx\x1b[0m");
        assert_eq!(t.italic("x"), "\x1b[3mx\x1b[0m");
        assert_eq!(t.underline("x"), "\x1b[4mx\x1b[0m");
    }

    #[test]
    fn pi_dark_palette_matches_dark_json() {
        // Anchor values from Pi's dark.json to catch palette drift.
        let t = Theme::dark();
        assert_eq!(t.color("accent"), Color::Rgb(0x8a, 0xbe, 0xb7));
        assert_eq!(t.color("border"), Color::Rgb(0x5f, 0x87, 0xff));
        assert_eq!(t.color("borderAccent"), Color::Rgb(0x00, 0xd7, 0xff));
        assert_eq!(t.color("userMessageBg"), Color::Rgb(0x34, 0x35, 0x41));
        assert_eq!(t.color("toolSuccessBg"), Color::Rgb(0x28, 0x32, 0x28));
        assert_eq!(t.color("toolErrorBg"), Color::Rgb(0x3c, 0x28, 0x28));
        assert_eq!(t.color("mdHeading"), Color::Rgb(0xf0, 0xc6, 0x74));
        assert_eq!(t.color("thinkingMax"), Color::Rgb(0xff, 0x5f, 0xff));
    }

    #[test]
    fn pi_light_palette_matches_light_json() {
        let t = Theme::light();
        assert_eq!(t.color("accent"), Color::Rgb(0x5a, 0x80, 0x80));
        assert_eq!(t.color("userMessageBg"), Color::Rgb(0xe8, 0xe8, 0xe8));
        assert_eq!(t.color("mdHeading"), Color::Rgb(0x9a, 0x73, 0x26));
    }

    #[test]
    fn color_names_count_matches_pi() {
        // Pi defines ~55 color slots. We should be at or above that.
        assert!(
            COLOR_NAMES.len() >= 55,
            "expected >=55 Pi-equivalent color slots, got {}",
            COLOR_NAMES.len()
        );
    }

    #[test]
    fn parse_full_theme_json() {
        let json = r##"{
            "name": "solarized-dark",
            "colors": {
                "accent": "#268bd2",
                "dim": "#586e75",
                "unknown_slot": "#ff0000"
            }
        }"##;
        let t = Theme::parse(json).unwrap();
        assert_eq!(t.name, "solarized-dark");
        assert!(t.colors.contains_key("accent"));
        assert!(t.colors.contains_key("dim"));
        assert!(!t.colors.contains_key("unknown_slot"));
    }

    #[test]
    fn merge_overrides_colors() {
        let mut a = Theme::dark();
        let b = Theme::parse(r##"{"name": "x", "colors": {"accent": "#ff0000"}}"##).unwrap();
        a.merge(b);
        assert_eq!(a.color("accent"), Color::Rgb(255, 0, 0));
        assert_eq!(a.name, "x");
    }

    #[test]
    fn dark_and_light_differ() {
        let d = Theme::dark();
        let l = Theme::light();
        assert_ne!(d.color("background"), l.color("background"));
        assert_ne!(d.color("foreground"), l.color("foreground"));
    }

    #[test]
    fn resolved_palette_covers_every_known_slot() {
        let t = Theme::dark();
        let pal = t.resolved_palette();
        assert_eq!(pal.len(), COLOR_NAMES.len());
        for (slot, _) in &pal {
            assert!(COLOR_NAMES.contains(&slot.as_str()));
        }
    }

    #[test]
    fn unknown_color_name_falls_back_to_white() {
        let t = Theme::dark();
        // Unknown slots aren't in COLOR_NAMES but we still want the
        // theme not to panic if someone hits one. We default to White.
        assert_eq!(t.color("nonexistent_slot"), Color::White);
    }
}
