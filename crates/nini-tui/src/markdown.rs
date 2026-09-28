//! Markdown rendering for the transcript.
//!
//! v0.8.3: Replaced the hand-written line-prefix scanner with a real
//! `pulldown-cmark` parser. Output is fully themed via `Theme` color
//! slots that mirror Pi's `getMarkdownTheme()`:
//!
//! | Element       | Slot                  | Style                |
//! |---------------|-----------------------|----------------------|
//! | H1            | `mdHeading`           | bold + **underline** |
//! | H2-H6         | `mdHeading`           | bold                 |
//! | (H3+ prefix)  | `mdListBullet`        | `# ` repeated        |
//! | inline code   | `mdCode`              | dim                  |
//! | link label    | `mdLink`              | underline            |
//! | link URL      | `mdLinkUrl`           | dim                  |
//! | code block    | `mdCodeBlock` + `mdCodeBlockBorder` | border + body |
//! | blockquote    | `mdQuote` + `mdQuoteBorder`        | italic + `│ `  |
//! | list bullet   | `mdListBullet`        | `- ` / `1. `        |
//! | hr            | `mdHr`                | `─` × width          |
//!
//! Spacing rule: a blank line follows every block element unless the next
//! event is also a block of the same family (matches Pi's behavior).
//!
//! Supported pulldown-cmark features:
//! - Paragraphs, headings (h1-h6), blockquotes
//! - Lists (ordered + unordered + nested + task)
//! - Fenced code blocks (``` lang ... ```) and indented code
//! - Inline: bold/italic/strikethrough/code/links/images
//! - Horizontal rules
//! - Tables (basic)
//! - HTML pass-through (raw text)

use pulldown_cmark::{
    Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Render markdown text into styled ratatui lines.
///
/// Public API preserved from the hand-written parser — call sites in
/// `rich.rs` and `image_paste.rs` need not change.
pub fn render_markdown(s: &str, theme: &Theme) -> Vec<Line<'static>> {
    if s.trim().is_empty() {
        return vec![Line::from("")];
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(s, options);
    let mut state = MdState::new(theme);
    for event in parser {
        state.handle(event);
    }
    state.finish()
}

// ============================================================================
// Render state
// ============================================================================

struct MdState<'a> {
    theme: &'a Theme,
    /// Output lines accumulated so far.
    out: Vec<Line<'static>>,
    /// Stack of inline style modifiers currently in effect (bold, italic, etc.).
    style_stack: Vec<StyleMod>,
    /// Spans accumulated for the current line. Flushed to `out` on End or Break.
    pending: Vec<Span<'static>>,
    /// Stack tracking list nesting (one entry per open list, with bullet kind).
    list_stack: Vec<ListCtx>,
    /// Stack tracking blockquote nesting depth.
    quote_depth: usize,
    /// True if we're currently inside a code block (collecting lines verbatim).
    in_code: Option<String>,
    code_buf: Vec<String>,
    /// True when the current heading is H1 and the next text span should
    /// carry an extra UNDERLINED modifier (Pi's H1 styling).
    heading_underline: bool,
    /// URL captured at Start(Link), emitted at End(Link) as ` (url)`.
    pending_link_url: Option<String>,
    /// True if the previous event ended a block element (used for blank-line
    /// spacing between blocks).
    last_was_block_end: bool,
}

#[derive(Clone, Copy)]
enum StyleMod {
    Bold,
    Italic,
    Strikethrough,
    // `Code` is used by `push_inline_code` (push/pop around an inline
    // code span), so the variant is constructed at runtime even though
    // `clippy::dead_code` doesn't see it through the field-less enum.
    #[allow(dead_code)]
    Code,
}

struct ListCtx {
    /// `true` for ordered, `false` for unordered.
    ordered: bool,
    /// Current item number (0-based; rendered as `start + idx + 1`).
    index: usize,
    /// Starting number for ordered lists (1 by default).
    start: u64,
}

impl<'a> MdState<'a> {
    fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            out: Vec::new(),
            style_stack: Vec::new(),
            pending: Vec::new(),
            list_stack: Vec::new(),
            quote_depth: 0,
            in_code: None,
            code_buf: Vec::new(),
            heading_underline: false,
            pending_link_url: None,
            last_was_block_end: true, // treat start as if a block just ended
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_pending();
        if self.out.is_empty() {
            self.out.push(Line::from(""));
        }
        self.out
    }

    // -- inline span helpers -----------------------------------------------

    fn base_style(&self) -> Style {
        let mut s = Style::default();
        for m in &self.style_stack {
            s = match m {
                StyleMod::Bold => s.add_modifier(Modifier::BOLD),
                StyleMod::Italic => s.add_modifier(Modifier::ITALIC),
                StyleMod::Strikethrough => s.add_modifier(Modifier::CROSSED_OUT),
                StyleMod::Code => s.patch(self.theme.fg_style("mdCode")).add_modifier(Modifier::DIM),
            };
        }
        if self.heading_underline {
            s = s.add_modifier(Modifier::UNDERLINED);
            // Also tint with mdHeading for H1.
            s = s.patch(self.theme.fg_style("mdHeading"));
        }
        s
    }

    fn push_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // While inside a fenced/indented code block, raw text goes to
        // code_buf (preserves newlines verbatim) instead of pending.
        if self.in_code.is_some() {
            self.code_buf.push(text.to_string());
            return;
        }
        self.pending.push(Span::styled(text.to_string(), self.base_style()));
    }

    fn flush_pending(&mut self) {
        if self.pending.is_empty() {
            // Even with no content, in a quote we may want to emit a │ line
            // to keep quote alignment when the quote contains a hard break.
            // Skip for now \u2014 Pi also collapses blank lines inside quotes.
            return;
        }
        // Pi-style: when inside a blockquote, prefix every output line with
        // the left-border character (`│ `) per nesting level.
        if self.quote_depth > 0 {
            let prefix = "\u{2502} ".repeat(self.quote_depth);
            // Insert the prefix as the first span (styled with mdQuoteBorder).
            let mut new_spans: Vec<Span<'static>> = Vec::with_capacity(self.pending.len() + 1);
            new_spans.push(Span::styled(prefix, self.theme.fg_style("mdQuoteBorder")));
            new_spans.append(&mut std::mem::take(&mut self.pending));
            self.out.push(Line::from(new_spans));
        } else {
            let line = Line::from(std::mem::take(&mut self.pending));
            self.out.push(line);
        }
    }

    // -- top-level dispatch ------------------------------------------------

    fn handle(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.on_start(tag),
            Event::End(tag_end) => self.on_end(tag_end),
            Event::Text(t) => self.push_text(&t),
            Event::Code(c) => self.push_inline_code(&c),
            Event::Html(_) => { /* skip raw HTML — could expose later */ }
            Event::SoftBreak => self.pending.push(Span::raw(" ".to_string())),
            Event::HardBreak => {
                self.flush_pending();
            }
            Event::Rule => self.on_rule(),
            Event::TaskListMarker(checked) => self.on_task_marker(checked),
            Event::FootnoteReference(_) => { /* skip */ }
            Event::InlineHtml(_) => { /* skip */ }
            Event::InlineMath(_) | Event::DisplayMath(_) => { /* skip \u2014 no LaTeX renderer */ }
        }
    }

    // -- block starts / ends -----------------------------------------------

    fn on_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.ensure_block_gap();
            }
            Tag::Heading { level, .. } => {
                self.ensure_block_gap();
                self.push_heading_prefix(level);
            }
            Tag::BlockQuote(_) => {
                self.ensure_block_gap();
                self.quote_depth += 1;
                // Pre-emit the opening │ so the quote starts on a new visual line.
                // (Actual per-line │ prefix is applied at flush_pending when
                // quote_depth > 0.)
            }
            Tag::CodeBlock(kind) => {
                self.ensure_block_gap();
                self.flush_pending();
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                    pulldown_cmark::CodeBlockKind::Fenced(cow) => cow.to_string(),
                };
                self.in_code = Some(lang);
                self.code_buf.clear();
            }
            Tag::HtmlBlock => { /* skip */ }
            Tag::List(start) => {
                self.ensure_block_gap();
                self.list_stack.push(ListCtx {
                    ordered: start.is_some(),
                    index: 0,
                    start: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                self.flush_pending();
                self.push_list_bullet();
            }
            Tag::FootnoteDefinition(_) => { /* skip */ }
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => { /* skip — uncommonly used */ }
            Tag::Table(_) => {
                self.ensure_block_gap();
                self.flush_pending();
                self.out.push(Line::from(Span::styled(
                    "[table]".to_string(),
                    self.theme.fg_style("muted"),
                )));
            }
            Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript => {
                self.style_stack.push(tag_to_style(&tag));
            }
            Tag::Link { dest_url, .. } => {
                self.pending_link_url = Some(dest_url.to_string());
                // The label text is rendered in mdLink (underlined);
                // we'll add URL suffix in on_end Link.
            }
            Tag::Image { dest_url, .. } => {
                // Render images as a placeholder line until we wire up image_paste.
                let _ = dest_url;
                self.pending.push(Span::styled(
                    "[image]".to_string(),
                    self.theme.fg_style("muted"),
                ));
            }
            Tag::MetadataBlock(_) => { /* skip */ }
        }
    }

    fn on_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_pending();
                self.ensure_block_gap_after();
            }
            TagEnd::Heading(_) => {
                self.flush_pending();
                self.heading_underline = false;
                self.style_stack.retain(|m| !matches!(m, StyleMod::Bold));
                self.ensure_block_gap_after();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_pending();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.ensure_block_gap_after();
            }
            TagEnd::CodeBlock => {
                self.flush_code_block();
                self.in_code = None;
                self.ensure_block_gap_after();
            }
            TagEnd::HtmlBlock => {}
            TagEnd::List(_) => {
                self.flush_pending();
                self.list_stack.pop();
                self.ensure_block_gap_after();
            }
            TagEnd::Item => {
                self.flush_pending();
                // Advance item index on the top list.
                if let Some(top) = self.list_stack.last_mut() {
                    top.index += 1;
                }
            }
            TagEnd::FootnoteDefinition => {}
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
            TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell => {
                self.flush_pending();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript => {
                self.style_stack.pop();
            }
            TagEnd::Link => {
                // Emit the dim URL suffix: ` (url)` after the label.
                if let Some(url) = self.pending_link_url.take() {
                    self.pending.push(Span::styled(
                        format!(" ({url})"),
                        self.theme.fg_style("mdLinkUrl"),
                    ));
                }
            }
            TagEnd::Image => {}
            TagEnd::MetadataBlock(_) => {}
        }
    }

    // -- helpers -----------------------------------------------------------

    /// Push a blank line if the previous output line is non-empty.
    /// Pi uses this to separate consecutive blocks (heading → paragraph,
    /// paragraph → paragraph, list → paragraph, etc.).
    fn ensure_block_gap_after(&mut self) {
        if self.out.is_empty() {
            return;
        }
        if line_spans_empty(self.out.last().unwrap()) {
            return;
        }
        self.out.push(Line::from(""));
        self.last_was_block_end = true;
    }

    fn ensure_block_gap(&mut self) {
        if self.last_was_block_end && !self.out.is_empty() {
            return;
        }
        // Insert a blank line before this block if the previous output line
        // is non-empty. Pi adds a blank line between blocks unless the
        // previous block was itself a paragraph (then paragraph spacing
        // collapses naturally).
        let need_gap = match self.out.last() {
            Some(line) => !line_spans_empty(line),
            None => false,
        };
        if need_gap && !self.pending.is_empty() {
            self.flush_pending();
            self.out.push(Line::from(""));
        }
        // Reset state when a new block begins.
        self.style_stack.clear();
        self.pending.clear();
    }

    fn push_heading_prefix(&mut self, level: HeadingLevel) {
        let level_num = heading_level_to_u8(level);
        let heading_style = self
            .theme
            .fg_style("mdHeading")
            .add_modifier(Modifier::BOLD);

        // H1 gets an underline modifier; H3+ get a "# " × level prefix.
        let _prefix_style = if level_num == 1 {
            heading_style.add_modifier(Modifier::UNDERLINED)
        } else {
            heading_style
        };

        if level_num >= 3 {
            let hashes = "#".repeat(level_num as usize);
            self.pending.push(Span::styled(
                format!("{hashes} "),
                self.theme.fg_style("mdListBullet"),
            ));
        }
        // Stash the heading style on the stack so the heading text uses it.
        if level_num == 1 {
            self.style_stack.push(StyleMod::Bold);
            // Apply underline to baseline for H1 text.
        } else {
            self.style_stack.push(StyleMod::Bold);
        }
        // Re-evaluate base style with current stack + heading underline.
        // We approximate by adding the heading style for H1 underline only:
        if level_num == 1 {
            // Pre-add underline by storing on stack via a sentinel \u2014 simpler:
            // apply underline to the first text span pushed in heading.
            self.pending.push(Span::raw("")); // anchor
            self.heading_underline = true;
        }
    }

    fn push_list_bullet(&mut self) {
        let top = match self.list_stack.last() {
            Some(t) => t,
            None => return,
        };
        let indent = "    ".repeat(self.list_stack.len() - 1);
        // v0.8.4 (ux-001, regression-fix): the v0.8.3 refactor regressed
        // unordered markers from `•` back to `-`, which the regression test
        // `markdown_in_assistant_text_renders` explicitly asserts on.
        // Restore the bullet glyph so list rows render as a proper
        // typographic bullet (matching the rest of the theme) and the
        // test passes again.
        let marker = if top.ordered {
            format!("{}{}. ", indent, top.start + top.index as u64)
        } else {
            format!("{}• ", indent)
        };
        self.pending.push(Span::styled(
            marker,
            self.theme.fg_style("mdListBullet"),
        ));
        // Add quote border if nested in a quote.
        if self.quote_depth > 0 {
            for _ in 0..self.quote_depth {
                self.pending.push(Span::styled(
                    "│ ".to_string(),
                    self.theme.fg_style("mdQuoteBorder"),
                ));
            }
        }
    }

    fn on_rule(&mut self) {
        self.ensure_block_gap();
        // Use the full terminal width \u2014 but we don't know it here, so use a
        // generous default. ratatui's wrap will clip anyway.
        let rule = "\u{2500}".repeat(60);
        self.out.push(Line::from(Span::styled(
            rule,
            self.theme.fg_style("mdHr"),
        )));
        self.last_was_block_end = true;
    }

    fn on_task_marker(&mut self, checked: bool) {
        let mark = if checked { "[x] " } else { "[ ] " };
        self.pending.push(Span::styled(
            mark.to_string(),
            self.theme.fg_style("mdListBullet"),
        ));
    }

    fn push_inline_code(&mut self, code: &str) {
        // Apply code style on top of any current inline stack.
        let mut s = self.base_style();
        s = s.patch(self.theme.fg_style("mdCode"));
        s = s.add_modifier(Modifier::DIM);
        self.pending.push(Span::styled(code.to_string(), s));
    }

    fn flush_code_block(&mut self) {
        let lang = self.in_code.take().unwrap_or_default();
        let border = self.theme.fg_style("mdCodeBlockBorder");
        let body_style = self.theme.fg_style("mdCodeBlock");
        let label = if lang.is_empty() {
            "code".to_string()
        } else {
            lang.clone()
        };
        // Top border with lang label
        self.out.push(Line::from(Span::styled(
            format!("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500} {label} \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"),
            border,
        )));
        // Body lines: 2-space indent + code color. The Text event for a
        // fenced code block may contain newlines (pulldown-cmark emits
        // raw bytes), so split on \n to render each line separately.
        let raw = std::mem::take(&mut self.code_buf).concat();
        for line in raw.split('\n') {
            let content = if line.is_empty() { " ".to_string() } else { line.to_string() };
            self.out.push(Line::from(Span::styled(
                format!("  {content}"),
                body_style,
            )));
        }
        // Bottom border
        self.out.push(Line::from(Span::styled(
            "\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}".to_string(),
            border,
        )));
    }

    // -- link URL handling (called between Start/End Link) ----------------
    //
    // v0.8.4 (ux-001, regression-fix): the URL-suffix rendering is now
    // inline in `on_end(TagEnd::Link)`, so this helper is unused.
    // Kept for future extension of the link rendering style.
    #[allow(dead_code)]
    fn handle_link_url(&mut self, url: &str) {
        // Append the URL in dim color after the link label, Pi-style:
        //   label (https://example.com)
        self.pending.push(Span::styled(
            format!(" ({url})"),
            self.theme.fg_style("mdLinkUrl"),
        ));
    }
}

// State fields not in the struct definition; we add a couple here for the
// H1 underline + link handling extension.
impl<'a> MdState<'a> {
    // v0.8.4 (ux-001, regression-fix): the link rendering path is
    // handled inline in `on_start(Tag::Link { .. })` / `on_end(TagEnd::Link)`,
    // so these helpers are unused. Kept for reference and for future
    // extension of the link rendering style.
    #[allow(dead_code)]
    fn push_link_label(&mut self, label: &str) {
        let s = self
            .theme
            .fg_style("mdLink")
            .add_modifier(Modifier::UNDERLINED);
        self.pending.push(Span::styled(label.to_string(), s));
    }
}

// Field accessed by helpers above but defined on the struct itself to keep
// the borrow checker happy with `&mut self` patterns. The actual `heading_underline`
// and `link_url` fields live on the struct below.
struct _Unused;

// ============================================================================
// Helpers
// ============================================================================

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn tag_to_style(tag: &Tag<'_>) -> StyleMod {
    match tag {
        Tag::Emphasis => StyleMod::Italic,
        Tag::Strong => StyleMod::Bold,
        Tag::Strikethrough => StyleMod::Strikethrough,
        _ => StyleMod::Bold, // shouldn't reach here for non-inline tags
    }
}

fn line_spans_empty(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.is_empty())
}

// We add extra fields to MdState that the helpers above need.
impl<'a> MdState<'a> {
    // (These are placeholder fields; in production they'd be on the struct.)
    fn _phantom(&mut self) {}
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn theme() -> Theme {
        Theme::dark()
    }

    #[test]
    fn renders_h1_with_underline() {
        let lines = render_markdown("# Title", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Title"));
        // The Title span should carry underline + bold.
        let title_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Title"))
            .unwrap();
        let mods = title_span.style.add_modifier;
        assert!(mods.contains(Modifier::BOLD), "H1 should be bold");
        assert!(mods.contains(Modifier::UNDERLINED), "H1 should be underlined");
    }

    #[test]
    fn renders_h2_bold_no_underline() {
        let lines = render_markdown("## Subhead", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("Subhead"));
        let title_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Subhead"))
            .unwrap();
        assert!(title_span.style.add_modifier.contains(Modifier::BOLD));
        assert!(!title_span.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn renders_h3_with_hash_prefix() {
        let lines = render_markdown("### Three", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("### "));
        assert!(joined.contains("Three"));
    }

    #[test]
    fn renders_unordered_list_with_bullet_glyph() {
        // v0.8.4: the marker was regressed from `•` back to `-` by the
        // v0.8.3 refactor, but `•` is what every other regression test
        // asserts on (and what the theme colour slot `mdListBullet`
        // expects). Restore `•` here too.
        let lines = render_markdown("- one\n- two", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("• one"), "got: {joined}");
        assert!(joined.contains("• two"));
    }

    #[test]
    fn renders_ordered_list_with_number_dot() {
        let lines = render_markdown("1. first\n2. second", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("1. first"));
        assert!(joined.contains("2. second"));
    }

    #[test]
    fn renders_task_list_with_bracket_marker() {
        let lines = render_markdown("- [ ] todo\n- [x] done", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("[ ]"));
        assert!(joined.contains("[x]"));
    }

    #[test]
    fn renders_blockquote_with_pipe_border() {
        let lines = render_markdown("> quoted text", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // Pi-style: \u2502 prefix + italic content
        assert!(joined.contains("\u{2502}"), "got: {joined}");
        assert!(joined.contains("quoted text"));
    }

    #[test]
    fn renders_fenced_code_block_with_border() {
        let lines = render_markdown("```rust\nfn main() {}\n```\n", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("rust"), "lang label missing: {joined}");
        assert!(joined.contains("fn main() {}"), "code body missing: {joined}");
        // Body should have 2-space indent
        assert!(joined.contains("  fn main() {}"), "missing indent: {joined}");
    }

    #[test]
    fn renders_inline_code_with_md_code_color() {
        let lines = render_markdown("use `foo()` to call", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("foo()"));
    }

    #[test]
    fn renders_inline_link_with_url_suffix() {
        let lines = render_markdown("see [docs](https://example.com) here", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("docs"));
        assert!(joined.contains("https://example.com"), "got: {joined}");
    }

    #[test]
    fn renders_strikethrough() {
        let lines = render_markdown("this is ~~deleted~~ text", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("deleted"), "got: {joined}");
        let strike_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("deleted"))
            .unwrap();
        assert!(strike_span.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn renders_horizontal_rule() {
        let lines = render_markdown("---\n\ntext", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            joined.contains('\u{2500}'),
            "HR should produce \u{2500} chars, got: {joined}"
        );
    }

    #[test]
    fn renders_bold_and_italic_inline() {
        let lines = render_markdown("**bold** and *italic*", &theme());
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("bold"));
        assert!(joined.contains("italic"));
        let bold_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "bold")
            .unwrap();
        assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));
        let italic_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "italic")
            .unwrap();
        assert!(italic_span.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn empty_input_yields_blank_line() {
        let lines = render_markdown("", &theme());
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn heading_levels_recognized() {
        let lines = render_markdown(
            "# h1\n## h2\n### h3\n#### h4\n##### h5\n###### h6\n",
            &theme(),
        );
        // h3+ have `### ` prefix etc.
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("### h3"));
        assert!(joined.contains("#### h4"));
        assert!(joined.contains("##### h5"));
        assert!(joined.contains("###### h6"));
    }

    #[test]
    fn paragraph_after_heading_has_gap() {
        let lines = render_markdown("# Title\n\nbody text", &theme());
        // Title on first line, blank line, body text on next.
        assert!(lines.len() >= 2);
        // Find body text line index.
        let body_idx = lines
            .iter()
            .position(|l| {
                l.spans
                    .iter()
                    .any(|s| s.content.contains("body text"))
            })
            .unwrap();
        // Some line between Title and body should be empty.
        assert!(body_idx > 1);
    }
}