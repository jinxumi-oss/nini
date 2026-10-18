//! Oracle: a blockquote body carries Pi's quote style.
//!
//! **Reported class of bug (found by audit, not by a user report).**
//!
//! Pi (`markdown.ts:557`):
//! ```js
//! const quoteStyle = (text) => this.theme.quote(this.theme.italic(text));
//! ```
//! — the **body** of a blockquote is `mdQuote`-coloured and italic, and
//! the `│ ` gutter is a separate border style (`quoteBorder`).
//!
//! nini styles only the gutter (`markdown.rs`, `flush_pending`'s
//! `mdQuoteBorder` prefix). The body is `base_style()`, i.e. default
//! text. The `mdQuote` slot is in the palette (`theme.rs`) and read by
//! nothing — the same dead-slot shape as `mdCodeBlockBorder` in §32 and
//! `searchMatchBg` in the 10-17 audit.
//!
//! It is worse than a missing colour, because nini's own module header
//! documents the behaviour that is not implemented:
//!
//! ```text
//! | blockquote    | `mdQuote` + `mdQuoteBorder`        | italic + `│ `  |
//! ```
//!
//! A header table that describes a feature the file below it does not
//! have is the §31.4 failure mode in miniature: the doc is the only
//! evidence the behaviour exists.
//!
//! **Test discipline (v0.10 §35.6)**: assertions are on the rendered
//! `Line`s' styles — every API here (`render_assistant_message`) has the
//! same signature on both sides.

use nini_tui::rich::render_assistant_message;
use nini_tui::theme::Theme;
use ratatui::style::{Color, Modifier};
type Fg = Option<Color>;

fn lines(text: &str) -> Vec<ratatui::text::Line<'static>> {
    render_assistant_message(text, &Theme::dark(), Some(80))
}

fn body_spans(l: &ratatui::text::Line<'static>) -> Vec<(String, Fg, Modifier)> {
    l.spans
        .iter()
        .map(|s| (s.content.to_string(), s.style.fg, s.style.add_modifier))
        .collect()
}

#[test]
fn a_quote_body_is_styled_and_italic() {
    let rendered = lines("> quoted words");
    let quote_line = rendered
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("quoted words")))
        .expect("the quote body must be on screen");

    let body: Vec<_> = quote_line
        .spans
        .iter()
        .filter(|s| s.content.contains("quoted words"))
        .collect();
    assert!(
        !body.is_empty(),
        "no span carries the body text: {quote_line:?}"
    );

    for span in body {
        assert!(
            span.style.add_modifier.contains(Modifier::ITALIC),
            "Pi renders the body italic (theme.quote(theme.italic(text))); \
             this span is not: {:?}",
            span.style
        );
    }
}

/// Two distinct colours for the two quote slots, so the test can tell
/// which slot a span is bound to.
///
/// An earlier draft asserted `body.fg != gutter.fg` and failed — the dark
/// palette gives `mdQuote` and `mdQuoteBorder` the **same** `#808080`, so
/// that assertion was checking the palette, not the binding. A user who
/// edits their theme to separate them is the case that matters, so the
/// test now supplies that theme and checks the binding directly.
fn contrasting_theme() -> Theme {
    Theme::parse(
        r##"{"name":"probe","colors":{
            "mdQuote":"#ff0000",
            "mdQuoteBorder":"#00ff00",
            "mdCode":"#ff0000",
            "mdCodeBlock":"#ff0000",
            "mdCodeBlockBorder":"#00ff00",
            "mdHeading":"#ff0000","mdLink":"#ff0000","mdLinkUrl":"#ff0000",
            "mdListBullet":"#ff0000","mdHr":"#ff0000",
            "accent":"#ff0000","text":"#ff0000","dim":"#ff0000","muted":"#ff0000",
            "borderMuted":"#ff0000","success":"#ff0000","error":"#ff0000"
        }}"##,
    )
    .expect("probe theme parses")
}

fn is_rgb(c: Option<Color>, want: (u8, u8, u8)) -> bool {
    matches!(c, Some(Color::Rgb(r, g, b)) if (r, g, b) == want)
}

#[test]
fn the_body_and_the_gutter_are_bound_to_different_slots() {
    let rendered = render_assistant_message("> quoted words", &contrasting_theme(), Some(80));
    let quote_line = rendered
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("quoted words")))
        .expect("the quote body must be on screen");

    let body = quote_line
        .spans
        .iter()
        .find(|s| s.content.contains("quoted words"))
        .unwrap();
    let gutter = quote_line
        .spans
        .iter()
        .find(|s| s.content.contains('│'))
        .expect("Pi draws a `│ ` gutter");

    assert!(
        is_rgb(body.style.fg, (0xff, 0, 0)),
        "the body must follow the `mdQuote` slot (#ff0000), got {:?}",
        body.style.fg
    );
    assert!(
        is_rgb(gutter.style.fg, (0x00, 0xff, 0)),
        "the gutter must follow `mdQuoteBorder` (#00ff00), got {:?}",
        gutter.style.fg
    );
}

#[test]
fn the_body_carries_a_colour_at_all_in_the_default_palette() {
    // The original defect in its own terms: the body was the default
    // style, i.e. no colour. `#808080` is what `mdQuote` is in the dark
    // palette, and the paragraph default is `Reset`.
    let rendered = lines("> quoted words");
    let quote_line = rendered
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("quoted words")))
        .unwrap();
    let body = quote_line
        .spans
        .iter()
        .find(|s| s.content.contains("quoted words"))
        .unwrap();
    assert!(
        body.style.fg.is_some(),
        "the body must carry mdQuote; `Reset` is the default style, which \
         is exactly what the bug looked like on screen"
    );
}

#[test]
fn plain_text_is_not_accidentally_styled() {
    // The negative case: the fix must not turn every paragraph italic.
    let rendered = lines("an ordinary paragraph");
    let para = rendered
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("ordinary")))
        .expect("the paragraph must be on screen");
    for span in &para.spans {
        assert!(
            !span.style.add_modifier.contains(Modifier::ITALIC),
            "a paragraph outside a blockquote must not be italic: {:?}",
            span.style
        );
    }
}

#[test]
fn a_quote_still_keeps_its_gutter() {
    let rendered = lines("> quoted words");
    assert!(
        rendered
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains('│'))),
        "the gutter must survive the styling change"
    );
}

#[test]
fn the_body_span_helper_sees_real_content() {
    // Guard against the previous family of false greens: if `body_spans`
    // silently returned nothing, every assertion above would pass.
    let rendered = lines("> quoted words");
    let quote_line = rendered
        .iter()
        .find(|l| l.spans.iter().any(|s| s.content.contains("quoted words")))
        .unwrap();
    assert!(
        !body_spans(quote_line).is_empty(),
        "the measurement must see the spans it is measuring"
    );
}
