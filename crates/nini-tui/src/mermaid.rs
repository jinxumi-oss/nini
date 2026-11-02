//! Mermaid diagrams — **an explicit degradation, not a port.**
//!
//! # What Pi does, and what this module does *not* claim
//!
//! Pi 0.85.1 wires a Mermaid markdown transformer into the transcript:
//! `src/ui/interactive-mode.ts:131` imports `createMermaidMarkdownTransformer`
//! from `./components/mermaid.ts`, `:439` constructs it with
//! `getMode: () => settingsManager.getMermaidRenderingMode()`, and `:2025`
//! puts it at the head of the markdown transformer chain. The setting itself
//! is declared in `src/core/settings-manager.ts:62` as
//!
//! ```text
//! type MermaidRenderingMode = "off" | "final" | "streaming";   // default: "streaming"
//! ```
//!
//! **`components/mermaid.ts` is not part of the reference snapshot**, so
//! Pi's actual drawing code cannot be read from here — its box shape, its
//! colours and its truncation rule are all unverifiable. Nothing in this
//! file is "aligned with Pi"; it is aligned with the *setting*, not the
//! renderer.
//!
//! nini also has **no mermaid layout engine** — no graph layout, no text
//! measurement for node labels, no SVG-ish cell rasteriser — and this round
//! adds no dependency (no `mermaid`, no `mmdc`, no WASM runtime). So the
//! honest target is a **placeholder**: a block the user can recognise at a
//! glance as "this is a diagram, this machine does not draw it", with the
//! source kept verbatim underneath so it stays readable and copyable.
//!
//! What the placeholder gives you:
//!
//! ```text
//! ◇ mermaid · flowchart
//! not rendered here — diagram source preserved
//! ┌──────────────────────────┐
//! │ flowchart TD             │
//! │   A[Start] --> B[End]    │
//! └──────────────────────────┘
//! ```
//!
//! * the first line names the diagram kind ([`diagram_kind`]) so a reader
//!   knows whether to expect a flow, a sequence or a timeline;
//! * the second line says plainly that nothing was drawn;
//! * the body is the source, one line per source line, truncated only when a
//!   caller supplies a width — so no source line ever *disappears*.
//!
//! # Modes
//!
//! [`MermaidRenderingMode`] mirrors Pi's three values because the settings
//! file is shared territory: `off` must keep the old code-block path, and
//! `final`/`streaming` differ in Pi only in *when* the block is transformed
//! (during the response vs. when it is complete). nini renders a finished
//! transcript line and has no streaming markdown pass, so both modes are the
//! same code path here — see the note in `markdown.rs`.
//!
//! Wiring the mode to a settings key is deliberately **not** done in this
//! module: this round only provides the renderer and the hook.

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Pi's `MermaidRenderingMode` (`settings-manager.ts:62`).
///
/// Default is [`MermaidRenderingMode::Streaming`], matching Pi's documented
/// `// default: "streaming"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MermaidRenderingMode {
    /// Do not touch mermaid blocks: they render as ordinary fenced code.
    Off,
    /// Pi transforms once a response is complete.
    Final,
    /// Pi transforms while the response streams.
    #[default]
    Streaming,
}

impl MermaidRenderingMode {
    /// Parse the settings-file spelling: `off` / `final` / `streaming`,
    /// case-insensitively. Anything else is `None` (the caller keeps its
    /// current mode rather than silently flipping to a default).
    // Mirrors `str::from_str` naming on purpose: it is the settings keyword
    // parser, not `FromStr` with an error type nobody would ever match on.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "final" => Some(Self::Final),
            "streaming" => Some(Self::Streaming),
            _ => None,
        }
    }

    /// The settings-file spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Final => "final",
            Self::Streaming => "streaming",
        }
    }

    /// `true` when mermaid blocks must fall through to the plain code-block
    /// renderer, byte for byte.
    pub fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }
}

/// `true` when a fenced block's info string asks for mermaid.
///
/// The info string may carry more than the language (`mermaid title="x"`), so
/// the first whitespace-delimited word is what is compared. Case-insensitive:
/// pandoc writers emit `Mermaid`, and refusing to recognise it would show the
/// user a code block for no reason.
pub fn is_mermaid_fence_lang(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case("mermaid"))
}

/// The diagram type of a mermaid source, from its first non-blank line.
///
/// Returns a `&'static str` naming the construct in mermaid's own spelling
/// (`sequenceDiagram`, not `sequence diagram`) so a reader can copy it back
/// into a mermaid editor. Anything unknown — a future diagram type, a
/// directive such as `%%{init: …}%%`, or an empty block — degrades to
/// `"diagram"` rather than panicking or guessing.
///
/// nini does not lay these out; the name is a label only.
pub fn diagram_kind(source: &str) -> &'static str {
    let first = match source.lines().find(|l| !l.trim().is_empty()) {
        Some(l) => l.trim(),
        None => return "diagram",
    };
    let keyword = first.split_whitespace().next().unwrap_or("");
    // `gitGraph LR:` and `stateDiagram-v2` both put a suffix on the keyword,
    // so the match is on the lowercased prefix rather than the whole token.
    let k = keyword.to_ascii_lowercase();
    if k == "flowchart" || k == "graph" {
        "flowchart"
    } else if k == "sequencediagram" {
        "sequenceDiagram"
    } else if k == "classdiagram" {
        "classDiagram"
    } else if k.starts_with("statediagram") {
        // `stateDiagram`, `stateDiagram-v2`
        "stateDiagram"
    } else if k == "erdiagram" {
        "erDiagram"
    } else if k == "gantt" {
        "gantt"
    } else if k == "pie" {
        "pie"
    } else if k == "journey" {
        "journey"
    } else if k.starts_with("gitgraph") {
        "gitGraph"
    } else if k == "mindmap" {
        "mindmap"
    } else if k == "timeline" {
        "timeline"
    } else if k == "quadrantchart" {
        "quadrantChart"
    } else if k == "requirementdiagram" {
        "requirementDiagram"
    } else if k == "c4context" {
        "C4Context"
    } else {
        "diagram"
    }
}

/// Render a mermaid block as a labelled, bordered placeholder.
///
/// `width` is the caller's content width when known:
///
/// * every emitted line is at most `width` display cells (`width.rs` has the
///   measurement) — a wide diagram line is **truncated with `…`**, never
///   dropped, because the point of the placeholder is that the source is
///   still readable;
/// * `None` means "no measurement available": nothing is truncated and the
///   frame takes the width of the longest source line.
///
/// The returned lines are `'static` and styled from `theme` only through
/// existing slots (`mdCodeBlockBorder`, `mdCodeBlock`, `mdHeading`, `muted`),
/// so this adds no theme key that a theme author has to learn.
pub fn render_mermaid_block(
    source: &str,
    theme: &Theme,
    width: Option<usize>,
) -> Vec<Line<'static>> {
    let kind = diagram_kind(source);

    let title_style = theme.fg_style("mdHeading").add_modifier(Modifier::BOLD);
    let hint_style = theme.fg_style("muted");
    let border_style = theme.fg_style("mdCodeBlockBorder");
    let body_style = theme.fg_style("mdCodeBlock");

    // `pulldown-cmark` hands over the fenced block's raw text with its
    // trailing newline; that newline terminates the last line, it is not an
    // extra empty one. `\r` from a CRLF document is stripped for the same
    // reason.
    let body: Vec<&str> = source
        .trim_end_matches('\n')
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let fit = |s: &str| -> String {
        match width {
            Some(w) => crate::width::truncate_cells(s, w),
            None => s.to_string(),
        }
    };

    let mut out: Vec<Line<'static>> = Vec::with_capacity(body.len() + 4);
    out.push(Line::from(Span::styled(
        fit(&format!("\u{25c7} mermaid \u{00b7} {kind}")),
        title_style,
    )));
    out.push(Line::from(Span::styled(
        fit("not rendered here \u{2014} diagram source preserved"),
        hint_style,
    )));

    // The frame is `│ ` + inner + ` │`, so a line is `inner + 4` cells wide
    // and the rule above/below it must be `inner + 2` box characters.
    let inner = match width {
        Some(w) => w.saturating_sub(4),
        None => body
            .iter()
            .map(|l| crate::width::display_width(l))
            .max()
            .unwrap_or(0),
    };

    // Too narrow for a frame: `w < 5` leaves no room for one inner cell plus
    // the two 2-cell margins. Rather than emit a three-line box with nothing
    // inside it, drop the frame and keep the source lines, truncated.
    let framed = width.is_none() || inner >= 1;

    if framed {
        let rule = |left: &str, right: &str| {
            crate::width::ascii_safe(&format!("{left}{}{right}", "\u{2500}".repeat(inner + 2)))
        };
        out.push(Line::from(Span::styled(
            rule("\u{250c}", "\u{2510}"),
            border_style,
        )));
        for line in &body {
            let text = if width.is_some() {
                crate::width::truncate_cells(line, inner)
            } else {
                (*line).to_string()
            };
            let pad = inner.saturating_sub(crate::width::display_width(&text));
            // Only the right side is padded, so a source line's own leading
            // indentation survives at the same offset it had in the fenced
            // block — mermaid is whitespace-significant in lists and notes.
            let padded = format!("{text}{}", " ".repeat(pad));
            out.push(Line::from(Span::styled(
                format!("\u{2502} {padded} \u{2502}"),
                body_style,
            )));
        }
        out.push(Line::from(Span::styled(
            rule("\u{2514}", "\u{2518}"),
            border_style,
        )));
    } else {
        for line in &body {
            out.push(Line::from(Span::styled(fit(line), body_style)));
        }
    }

    out
}
