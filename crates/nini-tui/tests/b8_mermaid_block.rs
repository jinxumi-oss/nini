//! B8 — mermaid blocks render as an explicit placeholder, not silently as code.
//!
//! Gap B8 in the ledger: nini had no mermaid rendering at all. Pi 0.85.1
//! routes fenced ```mermaid blocks through
//! `components/mermaid.ts::createMermaidMarkdownTransformer`
//! (`interactive-mode.ts:131`, `:439`, `:2025`), gated by
//! `MermaidRenderingMode` (`settings-manager.ts:62`). That component is **not
//! in the reference snapshot** and nini has no graph layout engine, so this
//! round ships a declared **degradation**: a labelled box that says "this is a
//! diagram, this machine does not draw it", with the source kept line for
//! line so it stays readable and copyable.
//!
//! What these tests are for, in order of what would break a user:
//!
//! 1. a mermaid fence is *recognised* and labelled with its diagram kind;
//! 2. no source line is lost — a truncated diagram you cannot copy is worse
//!    than the code block it replaced;
//! 3. `off` is the old code block, byte for byte (the whole setting exists so
//!    a user can ask for exactly that);
//! 4. `sequenceDiagram` / `classDiagram` are classified, not lumped into
//!    `flowchart`;
//! 5. an unknown/future diagram type degrades to `diagram` instead of
//!    panicking on an exhausted match;
//! 6. every emitted line fits a supplied width — a placeholder that overflows
//!    the terminal folds the frame and shifts the transcript below it.

use nini_tui::markdown::{render_markdown, render_markdown_with_mermaid};
use nini_tui::mermaid::{diagram_kind, is_mermaid_fence_lang, MermaidRenderingMode};
use nini_tui::theme::Theme;
use nini_tui::width::display_width;
use ratatui::text::Line;

fn theme() -> Theme {
    Theme::dark()
}

fn text_of(line: &Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn text_of_all(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(text_of).collect()
}

/// The content of each framed body row: drop `│ ` / ` │` and the right-hand
/// padding, so what is left can be compared to the source line directly.
fn inner_rows(lines: &[Line<'static>]) -> Vec<String> {
    text_of_all(lines)
        .into_iter()
        .filter_map(|l| {
            l.strip_prefix("\u{2502} ")
                .and_then(|s| s.strip_suffix(" \u{2502}"))
                .map(|s| s.trim_end().to_string())
        })
        .collect()
}

fn mermaid(src_body: &str, mode: MermaidRenderingMode, width: Option<usize>) -> Vec<String> {
    text_of_all(&render_markdown_with_mermaid(
        &format!("```mermaid\n{src_body}```\n"),
        &theme(),
        width,
        mode,
    ))
}

// ---------------------------------------------------------------------------
// 1. recognition + label
// ---------------------------------------------------------------------------

/// A ```mermaid flowchart block is recognised as `flowchart` and its first
/// output line is the placeholder's title, naming mermaid and the kind.
#[test]
fn flowchart_fence_is_labelled_flowchart_under_a_mermaid_title() {
    let src = "flowchart TD\n  A[Start] --> B[End]\n  B --> C{Done}\n";
    assert_eq!(diagram_kind(src), "flowchart");

    let out = mermaid(src, MermaidRenderingMode::Streaming, Some(80));
    assert_eq!(out[0], "\u{25c7} mermaid \u{b7} flowchart");
    assert_eq!(
        out[1],
        "not rendered here \u{2014} diagram source preserved"
    );
    // The old code-block fence is gone: this must not read as an ordinary
    // language-tagged code block.
    assert_eq!(
        out.iter().filter(|l| l.starts_with("```")).count(),
        0,
        "a recognised mermaid block must not keep the ``` fence: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. source preservation
// ---------------------------------------------------------------------------

/// Every source line is still there, in order and character for character —
/// including the diagram's own leading indentation, which mermaid reads as
/// structure (subgraph membership, note bodies).
#[test]
fn every_source_line_survives_in_order_and_verbatim() {
    let src = "sequenceDiagram\n    Alice->>Bob: hi\n    Note right of Bob: a long note here\n";
    let out = render_markdown_with_mermaid(
        &format!("```mermaid\n{src}```\n"),
        &theme(),
        None,
        MermaidRenderingMode::Streaming,
    );
    assert_eq!(
        inner_rows(&out),
        vec![
            "sequenceDiagram".to_string(),
            "    Alice->>Bob: hi".to_string(),
            "    Note right of Bob: a long note here".to_string(),
        ]
    );
}

// ---------------------------------------------------------------------------
// 3. `off` is the untouched code-block path
// ---------------------------------------------------------------------------

/// `Off` must render exactly what the code-block path rendered before this
/// hook existed — the path exists so a user can turn the feature off.
///
/// The expected value is spelled out rather than compared to a snapshot, so a
/// change to *either* the placeholder or `flush_code_block` shows up here. It
/// includes two lines of the **pre-existing** code path that look like noise:
/// `"   "` (a body line for the newline that terminates the last source line)
/// and a trailing `""` (the block separator `ensure_block_gap_after` adds).
/// Neither is this round's business; `off` means *unchanged*, so they are
/// pinned as they are.
#[test]
fn off_mode_is_byte_for_byte_the_plain_code_block() {
    let src = "flowchart TD\n  A --> B\n";
    let off = mermaid(src, MermaidRenderingMode::Off, Some(80));
    assert_eq!(
        off,
        vec![
            "```mermaid".to_string(),
            "  flowchart TD".to_string(),
            "    A --> B".to_string(),
            "   ".to_string(),
            "```".to_string(),
            "".to_string(),
        ],
        "off mode must keep the pre-hook code block, got {off:?}"
    );

    // Same body, same mode, but tagged as a plain code block: the only
    // difference allowed is the language on the opening fence.
    let as_text = text_of_all(&render_markdown_with_mermaid(
        "```text\nflowchart TD\n  A --> B\n```\n",
        &theme(),
        Some(80),
        MermaidRenderingMode::Off,
    ));
    assert_eq!(
        as_text.first().map(String::as_str),
        Some("```text"),
        "the control render should be a ```text block: {as_text:?}"
    );
    let normalised: Vec<String> = as_text
        .iter()
        .map(|l| l.replace("```text", "```mermaid"))
        .collect();
    assert_eq!(
        off, normalised,
        "off mode diverged from the ordinary fenced-code path"
    );
}

/// `Off` does not merely "look like" a code block: it does not truncate.
/// The placeholder clips to the caller's width; the code path never did, and
/// a regression where `off` fell into the placeholder would show up as a
/// clipped body here.
#[test]
fn off_mode_keeps_a_wide_line_that_the_placeholder_would_clip() {
    let wide = "A --> B".repeat(20); // 140 cells
    let src = format!("flowchart TD\n{wide}\n");
    let off = mermaid(&src, MermaidRenderingMode::Off, Some(20));
    assert_eq!(off[1], format!("  flowchart TD"));
    assert_eq!(
        off[2],
        format!("  {wide}"),
        "off mode must not truncate a 140-cell line"
    );

    let on = mermaid(&src, MermaidRenderingMode::Streaming, Some(20));
    assert!(
        on.iter().all(|l| display_width(l) <= 20),
        "the placeholder is width-bounded: {on:?}"
    );
    assert!(
        on.iter().any(|l| l.ends_with('\u{2026}')),
        "the placeholder marks the clip with an ellipsis: {on:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. classification
// ---------------------------------------------------------------------------

/// A `sequenceDiagram` is not a `flowchart`, and its label says so.
#[test]
fn sequence_diagram_is_classified_as_sequencediagram() {
    let src = "sequenceDiagram\n    Alice->>Bob: hello\n";
    assert_eq!(diagram_kind(src), "sequenceDiagram");
    let out = mermaid(src, MermaidRenderingMode::Streaming, None);
    assert_eq!(out[0], "\u{25c7} mermaid \u{b7} sequenceDiagram");
    assert_eq!(
        inner_rows(&render_markdown_with_mermaid(
            &format!("```mermaid\n{src}```\n"),
            &theme(),
            None,
            MermaidRenderingMode::Streaming,
        ))[0],
        "sequenceDiagram"
    );
}

/// A `classDiagram` likewise — and in both modes it is the same block, since
/// nini has no streaming markdown pass (see `markdown.rs`).
#[test]
fn class_diagram_is_classified_and_modes_agree() {
    let src = "classDiagram\n    Animal <|-- Duck\n";
    assert_eq!(diagram_kind(src), "classDiagram");
    let streaming = mermaid(src, MermaidRenderingMode::Streaming, None);
    let fin = mermaid(src, MermaidRenderingMode::Final, None);
    assert_eq!(streaming[0], "\u{25c7} mermaid \u{b7} classDiagram");
    assert_eq!(
        streaming, fin,
        "final and streaming are one code path in nini"
    );
}

// ---------------------------------------------------------------------------
// 5. unknown diagram types
// ---------------------------------------------------------------------------

/// An unrecognised first line — a future mermaid type, a `%%{init}%%`
/// directive, or an empty block — degrades to `diagram` and still renders.
#[test]
fn unknown_diagram_type_degrades_to_diagram() {
    assert_eq!(
        diagram_kind("%%{init: {'theme':'dark'}}%%\npie\n"),
        "diagram"
    );
    assert_eq!(diagram_kind("kanban\n  todo[ ]"), "diagram");
    assert_eq!(diagram_kind(""), "diagram");
    assert_eq!(diagram_kind("\n\n   \n"), "diagram");

    let out = mermaid(
        "kanban\n  todo[ ] card\n",
        MermaidRenderingMode::Streaming,
        None,
    );
    assert_eq!(out[0], "\u{25c7} mermaid \u{b7} diagram");
    assert_eq!(
        inner_rows(&render_markdown_with_mermaid(
            "```mermaid\nkanban\n  todo[ ] card\n```\n",
            &theme(),
            None,
            MermaidRenderingMode::Streaming,
        )),
        vec!["kanban".to_string(), "  todo[ ] card".to_string()]
    );
}

/// An *empty* mermaid block is legal input and must not panic or vanish.
#[test]
fn empty_mermaid_block_renders_an_empty_frame() {
    let out = mermaid("", MermaidRenderingMode::Streaming, None);
    assert_eq!(out[0], "\u{25c7} mermaid \u{b7} diagram");
    // No body rows, but the frame is still two rules.
    let rules: Vec<&String> = out
        .iter()
        .filter(|l| l.starts_with('\u{250c}') || l.starts_with('\u{2514}'))
        .collect();
    assert_eq!(rules.len(), 2, "got {out:?}");
}

// ---------------------------------------------------------------------------
// 6. width
// ---------------------------------------------------------------------------

/// At `width: Some(20)` no emitted line exceeds 20 display cells — measured
/// with `width::display_width`, i.e. the same measurement the renderer uses,
/// so CJK and ambiguous-width characters are accounted for.
#[test]
fn no_line_exceeds_the_requested_width() {
    let src = "flowchart TD\n\
               A[一个非常非常非常长的节点名称] --> B[another extremely long node label]\n\
               B --> C\n";
    let out = mermaid(src, MermaidRenderingMode::Streaming, Some(20));
    for line in &out {
        assert!(
            display_width(line) <= 20,
            "line is {} cells wide (max 20): {line:?}",
            display_width(line)
        );
    }
    // …and the body is still there, clipped rather than dropped.
    assert_eq!(
        inner_rows(&render_markdown_with_mermaid(
            &format!("```mermaid\n{src}```\n"),
            &theme(),
            Some(20),
            MermaidRenderingMode::Streaming,
        ))
        .len(),
        3
    );
}

// ---------------------------------------------------------------------------
// surrounding text is untouched
// ---------------------------------------------------------------------------

/// The hook sits inside the paragraph/block state machine; text on either
/// side of the block must still render, and the block must still be separated
/// from it by a blank line.
#[test]
fn text_around_a_mermaid_block_still_renders() {
    let doc = "before\n\n```mermaid\nflowchart TD\n  A-->B\n```\n\nafter\n";
    let out = text_of_all(&render_markdown_with_mermaid(
        doc,
        &theme(),
        Some(80),
        MermaidRenderingMode::Streaming,
    ));
    assert_eq!(out[0], "before");
    assert_eq!(out[1], "");
    assert_eq!(out[2], "\u{25c7} mermaid \u{b7} flowchart");
    let after_idx = out
        .iter()
        .position(|l| l == "after")
        .unwrap_or_else(|| panic!("trailing paragraph lost: {out:?}"));
    assert_eq!(
        out[after_idx - 1],
        "",
        "no blank line before `after`: {out:?}"
    );
    // Non-mermaid fences in the same document keep the old rendering.
    let mixed = text_of_all(&render_markdown_with_mermaid(
        "```mermaid\ngraph LR\n  A-->B\n```\n\n```rust\nlet x = 1;\n```\n",
        &theme(),
        Some(80),
        MermaidRenderingMode::Streaming,
    ));
    assert_eq!(mixed[0], "\u{25c7} mermaid \u{b7} flowchart");
    let rust_fence = mixed
        .iter()
        .position(|l| l == "```rust")
        .unwrap_or_else(|| panic!("the rust fence disappeared: {mixed:?}"));
    assert_eq!(
        mixed[rust_fence + 1],
        "  let x = 1;",
        "a rust block after a mermaid block must keep its code rendering: {mixed:?}"
    );
    assert_eq!(
        mixed
            .iter()
            .filter(|l| l.starts_with("\u{25c7} mermaid"))
            .count(),
        1,
        "only the mermaid block is transformed: {mixed:?}"
    );
    // And `render_markdown` — the unchanged public entry point — agrees with
    // the default mode.
    assert_eq!(
        text_of_all(&render_markdown(
            "```mermaid\ngraph LR\n  A-->B\n```\n",
            &theme(),
            Some(80)
        )),
        mermaid(
            "graph LR\n  A-->B\n",
            MermaidRenderingMode::Streaming,
            Some(80)
        )
    );
    assert!(is_mermaid_fence_lang("MERMAID"));
}
