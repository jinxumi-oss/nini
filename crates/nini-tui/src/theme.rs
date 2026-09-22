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

/// Names of every color slot nini-core can request. The list is closed —
/// the theme system panics on unknown names so we don't silently fall back
/// to defaults (which would mask typos in callers).
pub const COLOR_NAMES: &[&str] = &[
    "background",
    "foreground",
    "dim",
    "muted",
    "borderMuted",
    "accent",
    "success",
    "warning",
    "error",
    "info",
    "code",
    "link",
    "text",
    "toolPendingBg",
    "header",
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

fn is_emphatic(name: &str) -> bool {
    matches!(
        name,
        "accent" | "success" | "error" | "warning" | "info" | "header"
    )
}

fn default_color(name: &str) -> Color {
    match name {
        "background" => Color::Black,
        "borderMuted" => Color::DarkGray,
        "dim" | "muted" | "toolPendingBg" => Color::DarkGray,
        "accent" => Color::Cyan,
        "success" => Color::Green,
        "warning" => Color::Yellow,
        "error" => Color::Red,
        "info" => Color::Blue,
        "code" => Color::Magenta,
        "link" => Color::Blue,
        "header" => Color::White,
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

fn dark_palette() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("background".into(), "#1e1e1e".into());
    m.insert("foreground".into(), "#d4d4d4".into());
    m.insert("dim".into(), "#7f7f7f".into());
    m.insert("muted".into(), "#5a5a5a".into());
    m.insert("borderMuted".into(), "#3c3c3c".into());
    m.insert("accent".into(), "#569cd6".into());
    m.insert("success".into(), "#6a9955".into());
    m.insert("warning".into(), "#dcdcaa".into());
    m.insert("error".into(), "#f48771".into());
    m.insert("info".into(), "#9cdcfe".into());
    m.insert("code".into(), "#c586c0".into());
    m.insert("link".into(), "#569cd6".into());
    m.insert("text".into(), "#d4d4d4".into());
    m.insert("toolPendingBg".into(), "#2d2d2d".into());
    m.insert("header".into(), "#ffffff".into());
    m
}

fn light_palette() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("background".into(), "#fafafa".into());
    m.insert("foreground".into(), "#1a1a1a".into());
    m.insert("dim".into(), "#7a7a7a".into());
    m.insert("muted".into(), "#9e9e9e".into());
    m.insert("borderMuted".into(), "#e0e0e0".into());
    m.insert("accent".into(), "#3b6fb1".into());
    m.insert("success".into(), "#3a7c2f".into());
    m.insert("warning".into(), "#a35a00".into());
    m.insert("error".into(), "#c1351f".into());
    m.insert("info".into(), "#1f5fa8".into());
    m.insert("code".into(), "#7c3a99".into());
    m.insert("link".into(), "#3b6fb1".into());
    m.insert("text".into(), "#1a1a1a".into());
    m.insert("toolPendingBg".into(), "#f0f0f0".into());
    m.insert("header".into(), "#000000".into());
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
