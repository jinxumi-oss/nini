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
//! | code block    | `mdCodeBlock` + `mdCodeBlockBorder` | ``` fences + body |
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

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Render markdown text into styled ratatui lines.
///
/// Public API preserved from the hand-written parser — call sites in
/// `rich.rs` and `image_paste.rs` need not change.
/// `width` is the content width the caller will render into, when it knows
/// it. v0.10.1: the markdown renderer had no idea how wide the screen is, so
/// a thematic break drew `\u{2500}`.repeat(60) and relied on a later
/// truncation pass — see `on_rule`. Pi has the width, because its
/// `Markdown` is a layout component; this is the same value arriving from
/// the other direction.
/// Pi's thematic-break length cap: `Math.min(width, 80)`.
const MAX_HR_WIDTH: usize = 80;

pub fn render_markdown(s: &str, theme: &Theme, width: Option<usize>) -> Vec<Line<'static>> {
    if s.trim().is_empty() {
        return vec![Line::from("")];
    }

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(s, options);
    let mut state = MdState::new(theme, width);
    state.content_width = width;
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
    /// Content width the caller will render into, if known. Used by the
    /// handful of constructs whose length is not dictated by the source
    /// text — today that is the thematic break alone.
    width: Option<usize>,
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
    /// Table collection, while `in_table` is true.
    ///
    /// v0.10.1: `ENABLE_TABLES` was on from the parser swap and the module
    /// header claimed "Tables (basic)", but `Tag::Table` pushed the literal
    /// string `[table]` and the cells fell through to the paragraph path, so
    /// a three-column table rendered one cell per line with a marker in
    /// front of it.
    in_table: bool,
    /// Column alignments from `Tag::Table`, consumed at `TagEnd::Table`.
    table_alignment: Vec<pulldown_cmark::Alignment>,
    /// Width the caller will render into, when known. Set from the
    /// `render_markdown` argument so tables can size their columns.
    content_width: Option<usize>,
    table_rows: Vec<Vec<Vec<Span<'static>>>>,
    table_row: Vec<Vec<Span<'static>>>,
    table_cell: Vec<Span<'static>>,
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
    fn new(theme: &'a Theme, width: Option<usize>) -> Self {
        Self {
            theme,
            width,
            out: Vec::new(),
            style_stack: Vec::new(),
            pending: Vec::new(),
            list_stack: Vec::new(),
            quote_depth: 0,
            in_code: None,
            in_table: false,
            table_alignment: Vec::new(),
            content_width: None,
            table_rows: Vec::new(),
            table_row: Vec::new(),
            table_cell: Vec::new(),
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
                StyleMod::Code => s
                    .patch(self.theme.fg_style("mdCode"))
                    .add_modifier(Modifier::DIM),
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
        self.pending
            .push(Span::styled(text.to_string(), self.base_style()));
    }

    fn flush_pending(&mut self) {
        if self.in_table {
            // v0.10.1: inside a table, inline text belongs to the cell being
            // built. Without this it was flushed as a paragraph line, which
            // is why cells rendered one per row.
            self.table_cell.append(&mut self.pending);
            return;
        }
        if self.pending.is_empty() {
            // Even with no content, in a quote we may want to emit a │ line
            // to keep quote alignment when the quote contains a hard break.
            // Skip for now \u2014 Pi also collapses blank lines inside quotes.
            return;
        }
        // Pi-style: when inside a blockquote, prefix every output line with
        // the left-border character (`│ `) per nesting level.
        if self.quote_depth > 0 {
            let prefix = crate::width::ascii_safe("\u{2502} ").repeat(self.quote_depth);
            // v0.10.2: the body gets Pi's `theme.quote(theme.italic(text))`
            // (`markdown.ts:557`).
            //
            // Applied here, at flush time, rather than in `base_style`,
            // because Pi wraps the **finished** line
            // (`quoteStyle(line)` where `line` is already rendered). The
            // outer layer is what makes the quote colour survive inline
            // formatting: patching `base_style` instead would leave
            // `**bold**` inside a quote in the bold colour, where Pi
            // paints the quote colour with bold on top.
            let body = self
                .theme
                .fg_style("mdQuote")
                .add_modifier(Modifier::ITALIC);
            let mut new_spans: Vec<Span<'static>> = Vec::with_capacity(self.pending.len() + 1);
            new_spans.push(Span::styled(prefix, self.theme.fg_style("mdQuoteBorder")));
            new_spans.extend(std::mem::take(&mut self.pending).into_iter().map(|sp| {
                let mut sp = sp;
                sp.style = sp.style.patch(body);
                sp
            }));
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
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => { /* skip — uncommonly used */
            }
            Tag::Table(aligns) => {
                // v0.10.1: collect instead of announcing. `flush_pending`
                // routes into `table_cell` while `in_table`, so inline text
                // keeps its styles instead of escaping to the paragraph
                // path one cell at a time.
                self.ensure_block_gap();
                self.flush_pending();
                self.in_table = true;
                self.table_alignment = aligns.clone();
                self.table_rows.clear();
                self.table_row.clear();
                self.table_cell.clear();
            }
            Tag::TableHead | Tag::TableRow => {
                self.flush_pending();
                self.table_cell.clear();
            }
            Tag::TableCell => {
                self.flush_pending();
                self.table_cell.clear();
            }
            Tag::Emphasis
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
            TagEnd::TableCell => {
                self.flush_pending();
                let cell = std::mem::take(&mut self.table_cell);
                self.table_row.push(cell);
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                self.flush_pending();
                let row = std::mem::take(&mut self.table_row);
                if !row.is_empty() {
                    self.table_rows.push(row);
                }
            }
            TagEnd::Table => {
                self.flush_pending();
                let rows = std::mem::take(&mut self.table_rows);
                let aligns = std::mem::take(&mut self.table_alignment);
                let width = self.content_width.get_or_insert(80);
                let width = *width;
                self.in_table = false;
                self.render_table(&rows, &aligns, width);
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
        self.pending
            .push(Span::styled(marker, self.theme.fg_style("mdListBullet")));
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

    /// The cells, one row per line, with no frame — the narrow-terminal
    /// fallback (`markdown.js:557` has the raw source to print instead).
    fn render_table_without_frame(&mut self, rows: &[Vec<Vec<Span<'static>>>], available: usize) {
        let sep = " │ ";
        for row in rows {
            let mut line = String::new();
            for (i, cell) in row.iter().enumerate() {
                if i > 0 {
                    if crate::width::display_width(&line) + 3 > available {
                        break;
                    }
                    line.push_str(sep);
                }
                let t: String = cell.iter().map(|s| s.content.as_ref()).collect();
                line.push_str(&crate::width::truncate_cells(
                    &t,
                    available.saturating_sub(crate::width::display_width(&line) + 3 * i),
                ));
            }
            let line = crate::width::truncate_cells(&line, available);
            self.out.push(Line::from(Span::raw(line)));
        }
        self.last_was_block_end = true;
    }

    /// Draw a table: `┌─┬─┐` frame, columns sized between a natural and a
    /// minimum width, cell text wrapped to its column.
    ///
    /// v0.10.1. Pi: `renderTable(token, availableWidth, …)` —
    /// `@mariozechner/pi-tui` `dist/components/markdown.js:543`.
    ///
    /// The column arithmetic is Pi's, because it is the part that decides
    /// whether the reader sees a table or a staircase:
    ///
    /// * `border_overhead = 3n + 1` for `│ a │ b │` and the three borders
    /// * `natural[i]` — the widest cell in the column
    /// * `min_word[i]` — the longest *word*, capped at 30, so a column has
    ///   a floor that is not zero even when the table cannot fit
    /// * when the floors overflow the available width, every column starts at
    ///   1 and the remainder is shared by `(word - 1)` weight
    /// * when the naturals fit, use them; otherwise hand out the slack in
    ///   proportion to each column's growth headroom, then one cell at a
    ///   time for the rounding remainder (`markdown.js:625-634`)
    fn render_table(
        &mut self,
        rows: &[Vec<Vec<Span<'static>>>],
        aligns: &[pulldown_cmark::Alignment],
        available: usize,
    ) {
        if rows.is_empty() {
            return;
        }
        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return;
        }
        // The smallest frame that still fits: `3n + 1` when a border
        // character costs one cell (Pi's number: `n` columns of one cell
        // each, plus `n` separators of two cells, plus the closing bar).
        //
        // In a terminal that draws East-Asian **Ambiguous** characters two
        // cells wide — `┌ ─ ┬ │` are all ambiguous, and they are all this
        // frame — every one of the `2n + 1` border characters costs two,
        // and the padding around each cell is unaffected. That is
        // `2(2n + 1) + 3n = 7n + 2`, and a frame narrower than this is not
        // drawn at all: the table degrades to a borderless one rather than
        // overflow, which is the whole point of the check.
        let border_overhead: usize = if crate::width::ambiguous_width_is_wide() {
            7 * num_cols + 2
        } else {
            3 * num_cols + 1
        };
        // Pi gives up on the frame when even one cell per column will not
        // fit and falls back to the raw source (`markdown.js:557`). nini
        // has no raw text to fall back to, so the degradation is a
        // borderless table: the cells still line up, nothing overflows, and
        // no half-frame is drawn.
        if available <= border_overhead {
            self.render_table_without_frame(rows, available);
            return;
        }
        let available_for_cells = available - border_overhead;

        let text_of = |cell: &Vec<Span<'static>>| -> String {
            cell.iter().map(|s| s.content.as_ref()).collect::<String>()
        };
        let longest_word = |t: &str| -> usize {
            // Pi's `getLongestWordWidth(text, 30)`: whitespace-delimited,
            // capped, so one enormous token cannot set the floor.
            t.split_whitespace()
                .map(|w| crate::width::display_width(w))
                .max()
                .unwrap_or(1)
                .min(30)
                .max(1)
        };

        let mut natural = vec![1usize; num_cols];
        let mut min_word = vec![1usize; num_cols];
        for (r, row) in rows.iter().enumerate() {
            for (i, cell) in row.iter().enumerate().take(num_cols) {
                let t = text_of(cell);
                let w = crate::width::display_width(&t);
                natural[i] = natural[i].max(w);
                // `markdown.js:567`: the header's natural width is set, not
                // maxed, so a wide header cannot inflate a narrow column.
                if r > 0 {
                    min_word[i] = min_word[i].max(longest_word(&t));
                } else {
                    min_word[i] = min_word[i].max(longest_word(&t));
                }
            }
        }

        // Floors that still do not fit: every column gets 1, then share.
        let mut floors = min_word.clone();
        let mut floor_total: usize = floors.iter().sum();
        if floor_total > available_for_cells {
            floors = vec![1; num_cols];
            let remaining = available_for_cells.saturating_sub(num_cols);
            if remaining > 0 {
                let weights: Vec<usize> = min_word.iter().map(|w| w.saturating_sub(1)).collect();
                let total_weight: usize = weights.iter().sum();
                let mut growth = vec![0usize; num_cols];
                if total_weight > 0 {
                    for (i, w) in weights.iter().enumerate() {
                        growth[i] = (*w * remaining) / total_weight;
                    }
                }
                for (i, g) in growth.iter().enumerate() {
                    floors[i] += g;
                }
                let mut leftover = remaining.saturating_sub(growth.iter().sum::<usize>());
                let mut i = 0;
                while leftover > 0 && i < num_cols {
                    floors[i] += 1;
                    leftover -= 1;
                    i += 1;
                }
            }
            floor_total = floors.iter().sum();
        }

        // Naturals that fit: use them. Otherwise share the slack in
        // proportion to growth headroom.
        let natural_total: usize = natural.iter().sum::<usize>() + border_overhead;
        let mut widths = natural.clone();
        if natural_total > available {
            let extra = available_for_cells.saturating_sub(floor_total);
            let headroom: Vec<usize> = natural
                .iter()
                .zip(&floors)
                .map(|(n, f)| n.saturating_sub(*f))
                .collect();
            let total_headroom: usize = headroom.iter().sum();
            widths = floors.clone();
            if total_headroom > 0 {
                for (i, h) in headroom.iter().enumerate() {
                    widths[i] += (*h * extra) / total_headroom;
                }
            }
            let allocated: usize = widths.iter().sum();
            let mut remaining = available_for_cells.saturating_sub(allocated);
            let mut grew = true;
            while remaining > 0 && grew {
                grew = false;
                for i in 0..num_cols {
                    if remaining == 0 {
                        break;
                    }
                    if widths[i] < natural[i] {
                        widths[i] += 1;
                        remaining -= 1;
                        grew = true;
                    }
                }
            }
        } else {
            for i in 0..num_cols {
                widths[i] = widths[i].max(floors[i]);
            }
        }
        // A column must be at least one cell wide for the frame to close.
        for w in widths.iter_mut() {
            *w = (*w).max(1);
        }

        let border_style = self.theme.fg_style("mdCodeBlockBorder");
        // Pi styles the header with `theme.bold(padded)` and nothing else
        // (`markdown.js:652`) — no colour slot. An invented key here would
        // silently fall through `color()`'s default and look right in
        // tests while differing from Pi in the terminal.
        let head_style =
            ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::BOLD);

        // Pi: `┌─${topBorderCells.join("─┬─")}─┐` (`markdown.js:637`). The
        // `─` next to each corner is load-bearing: it is what makes the
        // border `Σw + 3n + 1` wide, the same as `│ a │ b │`. Leaving it
        // out is a one-cell-per-row mismatch that only shows on tables
        // whose cells are not a multiple of the width.
        // Every character of a table frame is East-Asian **Ambiguous**,
        // so in a terminal that draws them two cells wide the frame is
        // twice as wide as the arithmetic that sized it — and the row
        // overflows, which folds it and shifts the rest of the screen.
        // `ascii_safe` is a no-op in the normal case.
        let rule = |left: &str, mid: &str, right: &str, w: &[usize]| {
            let cells: Vec<String> = w
                .iter()
                .map(|n| "─".repeat(crate::width::chars_for_cells('─', *n)))
                .collect();
            crate::width::ascii_safe(&format!("{left}─{}─{right}", cells.join(mid)))
        };

        self.out.push(Line::from(Span::styled(
            rule("┌", "─┬─", "┐", &widths),
            border_style,
        )));

        for (r, row) in rows.iter().enumerate() {
            // Wrap every cell of the row to its column width, then emit as
            // many physical lines as the tallest cell needs.
            let wrapped: Vec<Vec<String>> = row
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    let t = text_of(cell);
                    match crate::width::wrap_text_to_width(&t, widths[i]) {
                        Some(parts) => parts,
                        None => vec![crate::width::truncate_cells(&t, widths[i])],
                    }
                })
                .collect();
            let height = wrapped.iter().map(|c| c.len()).max().unwrap_or(1);
            for line_idx in 0..height {
                let mut spans: Vec<Span<'static>> =
                    vec![Span::styled(crate::width::ascii_safe("│ "), border_style)];
                for (i, cell_lines) in wrapped.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(crate::width::ascii_safe(" │ "), border_style));
                    }
                    let text = cell_lines.get(line_idx).cloned().unwrap_or_default();
                    let pad = widths[i].saturating_sub(crate::width::display_width(&text));
                    let content = format!("{text}{}", " ".repeat(pad));
                    let style = if r == 0 {
                        head_style
                    } else {
                        ratatui::style::Style::default()
                    };
                    let align = aligns.get(i).copied();
                    let content = match align {
                        Some(pulldown_cmark::Alignment::Right) => {
                            let lead = " ".repeat(pad);
                            format!("{lead}{text}")
                        }
                        Some(pulldown_cmark::Alignment::Center) => {
                            let lead = pad / 2;
                            format!("{}{text}{}", " ".repeat(lead), " ".repeat(pad - lead))
                        }
                        _ => content,
                    };
                    spans.push(Span::styled(content, style));
                }
                spans.push(Span::styled(crate::width::ascii_safe(" │"), border_style));
                self.out.push(Line::from(spans));
            }
            if r == 0 {
                self.out.push(Line::from(Span::styled(
                    rule("├", "─┼─", "┤", &widths),
                    border_style,
                )));
            } else if r + 1 < rows.len() {
                self.out.push(Line::from(Span::styled(
                    rule("├", "─┼─", "┤", &widths),
                    border_style,
                )));
            }
        }

        self.out.push(Line::from(Span::styled(
            rule("└", "─┴─", "┘", &widths),
            border_style,
        )));
        self.last_was_block_end = true;
    }

    /// How long a thematic break is drawn.
    ///
    /// Pi: `"\u{2500}".repeat(Math.min(width, 80))`
    /// (`@mariozechner/pi-tui` `dist/components/markdown.js`, `case "hr"`).
    /// The cap matters as much as the width — on a 200-column terminal Pi
    /// still draws 80, so the rule never spans the screen.
    ///
    /// v0.10.1: this was a hard-coded 60 with the comment "we don't know it
    /// here, so use a generous default. ratatui's wrap will clip anyway",
    /// and the comment's own premise was wrong: the caller *did* know, it
    /// just did not pass it down. The result on a 100-column terminal was a
    /// rule of 60, and on a 40-column one a full-width rule — the "long
    /// horizontal lines" in the middle of a long task, where models put
    /// `---` between sections all the time.
    ///
    /// With no width (only reachable from tests, or a degenerate area
    /// narrower than 4 cells) this draws the shortest rule that still
    /// exists rather than guessing a length: 60 columns of rule is a
    /// statement about the screen that nothing here can support.
    fn rule_width(&self) -> usize {
        self.width.map_or(1, |w| w.min(MAX_HR_WIDTH).max(1))
    }

    fn on_rule(&mut self) {
        self.ensure_block_gap();
        let rule = crate::width::horizontal_rule(self.rule_width());
        self.out
            .push(Line::from(Span::styled(rule, self.theme.fg_style("mdHr"))));
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

    /// Code block: ```lang / body / ```.
    ///
    /// v0.10: the borders used to be `\u{2500}` rules — five, the lang, five,
    /// over a hardcoded 21 below. Two defects came out of that:
    ///
    ///   * **Not Pi's shape.** `pi-tui/src/components/markdown.ts:522,535`
    ///     fences a code block with backticks, not a rule. A transcript that
    ///     is mostly bash and file excerpts therefore drew a horizontal rule
    ///     above and below every block, which reads as "every line is
    ///     underlined" once there is more than a screenful. Reported as a
    ///     rendering bug; the cause was a rule where Pi puts a fence.
    ///   * **The two borders did not match.** The top was `12 + lang.len()`
    ///     columns and the bottom a fixed 21, so every block was framed
    ///     unevenly.
    ///
    /// The theme slot was already Pi's (`theme.ts:1278`,
    /// `codeBlockBorder: theme.fg("mdCodeBlockBorder", ...)`), which makes
    /// this the third time nini's theme schema was ported and the component
    /// that uses it was not - after `scrollbarThumb` and
    /// `default_thinking_level`.
    fn flush_code_block(&mut self) {
        let lang = self.in_code.take().unwrap_or_default();
        let border = self.theme.fg_style("mdCodeBlockBorder");
        let body_style = self.theme.fg_style("mdCodeBlock");
        // `markdown.ts:522` - three backticks then the language, and nothing
        // else. The old version substituted the placeholder "code" when
        // there was no language; Pi emits a bare fence.
        self.out
            .push(Line::from(Span::styled(format!("```{lang}"), border)));
        // Body lines: 2-space indent + code color. The Text event for a
        // fenced code block may contain newlines (pulldown-cmark emits
        // raw bytes), so split on \n to render each line separately.
        let raw = std::mem::take(&mut self.code_buf).concat();
        for line in raw.split('\n') {
            let content = if line.is_empty() {
                " ".to_string()
            } else {
                line.to_string()
            };
            self.out
                .push(Line::from(Span::styled(format!("  {content}"), body_style)));
        }
        // Bottom fence — `markdown.ts:535`, always bare.
        self.out
            .push(Line::from(Span::styled("```".to_string(), border)));
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
        let lines = render_markdown("# Title", &theme(), Some(80));
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
        assert!(
            mods.contains(Modifier::UNDERLINED),
            "H1 should be underlined"
        );
    }

    #[test]
    fn renders_h2_bold_no_underline() {
        let lines = render_markdown("## Subhead", &theme(), Some(80));
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
        let lines = render_markdown("### Three", &theme(), Some(80));
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
        let lines = render_markdown("- one\n- two", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("• one"), "got: {joined}");
        assert!(joined.contains("• two"));
    }

    #[test]
    fn renders_ordered_list_with_number_dot() {
        let lines = render_markdown("1. first\n2. second", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("1. first"));
        assert!(joined.contains("2. second"));
    }

    #[test]
    fn renders_task_list_with_bracket_marker() {
        let lines = render_markdown("- [ ] todo\n- [x] done", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("[ ]"));
        assert!(joined.contains("[x]"));
    }

    #[test]
    fn renders_blockquote_with_pipe_border() {
        let lines = render_markdown("> quoted text", &theme(), Some(80));
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
        let lines = render_markdown("```rust\nfn main() {}\n```\n", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("rust"), "lang label missing: {joined}");
        assert!(
            joined.contains("fn main() {}"),
            "code body missing: {joined}"
        );
        // Body should have 2-space indent
        assert!(
            joined.contains("  fn main() {}"),
            "missing indent: {joined}"
        );
    }

    #[test]
    fn renders_inline_code_with_md_code_color() {
        let lines = render_markdown("use `foo()` to call", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("foo()"));
    }

    #[test]
    fn renders_inline_link_with_url_suffix() {
        let lines = render_markdown("see [docs](https://example.com) here", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("docs"));
        assert!(joined.contains("https://example.com"), "got: {joined}");
    }

    #[test]
    fn renders_strikethrough() {
        let lines = render_markdown("this is ~~deleted~~ text", &theme(), Some(80));
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
        assert!(strike_span
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn renders_horizontal_rule() {
        let lines = render_markdown("---\n\ntext", &theme(), Some(80));
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
        let lines = render_markdown("**bold** and *italic*", &theme(), Some(80));
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(joined.contains("bold"));
        assert!(joined.contains("italic"));
        let bold_span = lines[0].spans.iter().find(|s| s.content == "bold").unwrap();
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
        let lines = render_markdown("", &theme(), Some(80));
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn heading_levels_recognized() {
        let lines = render_markdown(
            "# h1\n## h2\n### h3\n#### h4\n##### h5\n###### h6\n",
            &theme(),
            Some(80),
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
        let lines = render_markdown("# Title\n\nbody text", &theme(), Some(80));
        // Title on first line, blank line, body text on next.
        assert!(lines.len() >= 2);
        // Find body text line index.
        let body_idx = lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.content.contains("body text")))
            .unwrap();
        // Some line between Title and body should be empty.
        assert!(body_idx > 1);
    }

    // ---------------------------------------------------------------
    // v0.10: the code block fence.
    //
    // Reported as "every line is underlined once there is more than a
    // screenful". The cause was a `─` rule above and below every block,
    // where Pi uses ``` fences — and 342 unit tests passed throughout,
    // because none of them looked at the characters.
    // ---------------------------------------------------------------

    fn render(text: &str) -> String {
        render_markdown(text, &theme(), Some(80))
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_code_block_is_fenced_with_backticks_not_a_rule() {
        let out = render("```rust\nlet x = 1;\n```");
        assert!(
            out.contains("```rust"),
            "the opening fence should be ```rust, got:\n{out}"
        );
        assert!(
            out.trim_end().ends_with("```"),
            "the closing fence should be a bare ```, got:\n{out}"
        );
        assert!(
            !out.contains('─'),
            "a code block must not draw a horizontal rule anywhere:\n{out}"
        );
    }

    #[test]
    fn a_fence_without_a_language_is_bare() {
        // The old code substituted the placeholder "code". Pi emits nothing.
        let out = render("```\nplain\n```");
        assert!(
            out.lines().any(|l| l.trim() == "```"),
            "expected a bare ``` fence, got:\n{out}"
        );
        assert!(
            !out.contains("code"),
            "the placeholder language is gone, got:\n{out}"
        );
    }

    /// Not "the fences are the same width" — Pi's own are not:
    /// ```` ```rust ```` is seven columns and ```` ``` ```` is three. That
    /// asymmetry is invisible because a fence is a marker; it is only
    /// conspicuous when the same shape is drawn with a rule, which is what
    /// made the old `12 + lang.len()` versus a fixed 21 read as a broken
    /// frame.
    ///
    /// The first draft of this test asserted equal widths and failed on
    /// correct code — the same paragraph that argued they differ was written
    /// twenty minutes earlier in the same investigation. What is asserted
    /// here is the property that actually matters: both fences are fences.
    #[test]
    fn both_fences_are_fences_not_rules() {
        let out = render("```rust\na\n```");
        let fences: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with('`'))
            .collect();
        assert_eq!(
            fences,
            vec!["```rust", "```"],
            "expected Pi's exact fences in:\n{out}"
        );
    }

    /// The defect was only visible in bulk: one rule per block is easy to
    /// miss, two hundred is not.
    #[test]
    fn a_code_heavy_transcript_draws_no_rules() {
        let mut text = String::new();
        for i in 0..50 {
            text.push_str(&format!("```bash\necho step {i}\n```\n\n"));
        }
        let out = render(&text);
        assert_eq!(
            out.matches('─').count(),
            0,
            "a screenful of code blocks must not produce a single rule"
        );
        assert_eq!(out.matches("```bash").count(), 50);
    }

    #[test]
    fn a_horizontal_rule_still_draws_a_rule() {
        // The fix is scoped to code fences. A real markdown `hr` is Pi's
        // `mdHr` and must keep rendering as a rule.
        let out = render("above\n\n---\n\nbelow");
        assert!(
            out.contains('─'),
            "a markdown hr is not a code block and should still be a rule:\n{out}"
        );
        assert!(
            !out.contains("```"),
            "and it must not become a fence:\n{out}"
        );
    }
}
