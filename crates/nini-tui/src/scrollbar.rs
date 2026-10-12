//! Transcript scrollbar.
//!
//! v0.10. Pi paints one on the last column of the transcript area; nini
//! carried the theme key for it (`theme.rs` has `scrollbarThumb`, dark
//! `#3a3a4a` / light `#d0d0e0`) and nothing that drew it. The key name is
//! Pi's — `interactive-mode.ts:885` styles the thumb with
//! `theme.bg("scrollbarThumb", text)` — so the theme was ported and the
//! component never followed. Another instance of the shape the plan calls
//! "recorded but never wired", alongside `set_status_bar_usage` and
//! `default_thinking_level`.
//!
//! # The geometry is Pi's, transcribed
//!
//! `pi-tui/src/layout.ts:266-291`:
//!
//! ```ts
//! if (!box.scrollView?.isScrollbarVisible || …) return undefined;
//! const trackHeight = box.rect.height;
//! const minThumbHeight = Math.min(2, trackHeight);
//! const thumbHeight = Math.max(minThumbHeight,
//!     Math.min(trackHeight, Math.round((trackHeight * trackHeight) / contentHeight)));
//! const maxScrollTop = Math.max(0, contentHeight - trackHeight);
//! const maxThumbTop = trackHeight - thumbHeight;
//! const thumbOffset = maxScrollTop === 0 ? 0
//!     : Math.round((scrollView.scrollTop / maxScrollTop) * maxThumbTop);
//! const column = box.rect.x + box.rect.width - 1;
//! ```
//!
//! The `trackHeight * trackHeight / contentHeight` ratio is why the thumb
//! shrinks quadratically, and the floor of 2 rows is why it stays grabbable
//! on a short track. Both are kept rather than "improved".
//!
//! # Positions, and why nini's are inverted
//!
//! Pi's `scrollTop` is measured from the top of the content. nini's
//! `scroll_offset` counts `TranscriptLine`s skipped **from the bottom**, and
//! the render pass works in that direction too. So everything here takes
//! `items_skipped_from_bottom` and derives the top-down position:
//!
//! ```text
//! scroll_top = (content_height - viewport_height) - items_skipped_from_bottom
//! ```
//!
//! At the live tail `items_skipped_from_bottom` is 0, so `scroll_top` equals
//! the maximum and the thumb sits at the bottom — which is where a reader
//! expects it while the model is streaming.
//!
//! # What is deliberately not here
//!
//! Dragging. Pi supports it (`tui-alt-screen.ts:114` `ScrollbarDrag`,
//! with a `grabOffset` so the thumb keeps its position under the cursor).
//! nini has no mouse pipeline at all — no `EnableMouseCapture`, no
//! `MouseEvent` branch — so a draggable thumb is a decision about whether
//! nini takes the mouse, not a widget. See the plan.

/// Where the thumb goes, all in absolute screen cells within the transcript
/// area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarGeometry {
    /// Absolute x of the scrollbar column: the area's last column.
    pub column: u16,
    /// Absolute y of the first thumb row.
    pub thumb_top: u16,
    /// Thumb height in rows.
    pub thumb_height: u16,
    /// Rows the track spans, for hit-testing later.
    pub track_top: u16,
    pub track_height: u16,
    /// Largest top-down scroll position, i.e. how far the thumb can travel.
    pub max_scroll_top: usize,
}

/// The thumb's shape and place, or `None` when there is nothing to show.
///
/// `None` is Pi's `getScrollbarGeometry` returning `undefined`: a track
/// with no room, or content that fits entirely. A thumb that covers the
/// whole track says "there is nothing above and nothing below", which is the
/// same information, so it is not drawn.
pub fn geometry(
    area_x: u16,
    area_y: u16,
    area_width: u16,
    area_height: u16,
    content_height: usize,
    viewport_height: usize,
    items_skipped_from_bottom: usize,
) -> Option<ScrollbarGeometry> {
    if area_width == 0 || area_height == 0 {
        return None;
    }
    let track_height = area_height as usize;
    if track_height == 0 || content_height <= viewport_height {
        // Pi: `getScrollbarGeometry` is only reached when
        // `isScrollbarVisible`, and `isScrollbarVisible` for "auto" requires
        // `contentHeight > currentViewportHeight`.
        return None;
    }

    // `layout.ts:270-274`
    let min_thumb_height = std::cmp::min(2, track_height);
    let thumb_height = std::cmp::max(
        min_thumb_height,
        std::cmp::min(
            track_height,
            // `Math.round` in Pi: half away from zero, and Rust's
            // `f64::round` matches that. Integer division would floor
            // instead, which shifts the thumb by a row at every boundary.
            ((track_height * track_height) as f64 / content_height as f64).round() as usize,
        ),
    );

    let max_scroll_top = content_height.saturating_sub(viewport_height);
    let max_thumb_top = track_height.saturating_sub(thumb_height);

    // See the module doc: nini measures from the bottom, Pi from the top.
    let scroll_top = max_scroll_top.saturating_sub(items_skipped_from_bottom);
    let thumb_offset = if max_scroll_top == 0 {
        0
    } else {
        ((scroll_top as f64 / max_scroll_top as f64) * max_thumb_top as f64).round() as usize
    };

    Some(ScrollbarGeometry {
        column: area_x + area_width - 1,
        thumb_top: area_y + thumb_offset.min(max_thumb_top) as u16,
        thumb_height: thumb_height as u16,
        track_top: area_y,
        track_height: area_height,
        max_scroll_top,
    })
}

/// Whether the bar is drawn at all.
///
/// Pi (`scroll-view.ts:71-77`):
///
/// ```ts
/// get isScrollbarVisible(): boolean {
///   if (this.scrollbar === "always") return this.currentViewportHeight > 0;
///   return this.scrollbar === "auto"
///       && this.contentHeight > this.currentViewportHeight
///       && this.transientScrollbarVisible;
/// }
/// ```
///
/// and `interactive-mode.ts:884` feeds it `settingsManager.getFullscreenScrollbar()`,
/// which defaults to `"auto"` (`settings-manager.ts:139`). So the bar is on
/// by default and fades out when the reader stops scrolling.
///
/// `hidden_millis` is how long since the last scroll. Pi uses a 1000 ms
/// timer (`scroll-view.ts:49`); nini compares timestamps instead of owning a
/// timer, which gives the same result without a background task.
pub fn should_draw(
    mode: ScrollbarMode,
    content_height: usize,
    viewport_height: usize,
    hidden_millis: u64,
) -> bool {
    match mode {
        ScrollbarMode::Hidden => false,
        ScrollbarMode::Always => viewport_height > 0,
        ScrollbarMode::Auto => {
            content_height > viewport_height && hidden_millis < TRANSIENT_HIDE_MS
        }
    }
}

/// How long after the last scroll the bar fades, matching
/// `scroll-view.ts:49`'s default `scrollbarHideDelayMs`.
pub const TRANSIENT_HIDE_MS: u64 = 1000;

/// `settings.fullscreenScrollbar`, mirroring Pi's `ScrollViewScrollbar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarMode {
    /// Pi's default. Shown while the transcript overflows and the reader has
    /// scrolled recently.
    #[default]
    Auto,
    /// Always painted while the transcript has a viewport, overflow or not.
    Always,
    /// Never painted.
    Hidden,
}

impl ScrollbarMode {
    /// Pi's `getFullscreenScrollbar` (`settings-manager.ts:1151-1154`)
    /// coerces anything that is not `always` or `hidden` to `auto`.
    pub fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("always") => ScrollbarMode::Always,
            Some("hidden") => ScrollbarMode::Hidden,
            _ => ScrollbarMode::Auto,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ScrollbarMode::Auto => "auto",
            ScrollbarMode::Always => "always",
            ScrollbarMode::Hidden => "hidden",
        }
    }
}

/// Milliseconds since the Unix epoch.
///
/// Only ever compared against another reading of the same clock, so
/// `SystemTime` is enough and pulls in no dependency.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How many `TranscriptLine`s it takes to cover `items` rendered rows,
/// counting from the bottom up — the same direction the renderer skips in.
///
/// Mirrors the user-scroll branch in `render.rs` so the number PageUp uses
/// and the number PageUp actually moves stay the same thing.
pub fn lines_needed_for_items(items_per_line: &[usize], items: usize, total: usize) -> usize {
    let mut remaining = items;
    let mut lines = 0usize;
    for i in (0..total).rev() {
        if remaining == 0 {
            break;
        }
        remaining = remaining.saturating_sub(items_per_line[i]);
        lines += 1;
    }
    lines
}

/// Everything a mouse event needs to act on the transcript.
///
/// Published by the render pass, because `row` and the per-line row counts
/// only exist there. A pointer position alone is not enough: the same `y` is
/// a different scroll position at a different terminal height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Viewport {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Total rendered rows in the transcript.
    pub content_height: usize,
    /// Rows the viewport shows.
    pub viewport_height: usize,
}

impl Viewport {
    pub fn geometry(&self, items_skipped_from_bottom: usize) -> Option<ScrollbarGeometry> {
        geometry(
            self.x,
            self.y,
            self.width,
            self.height,
            self.content_height,
            self.viewport_height,
            items_skipped_from_bottom,
        )
    }

    /// Is `(x, y)` on the scrollbar column, within the track?
    ///
    /// Pi's `getScrollbarTargetAt` matches the column and the track: grabbing
    /// the rail above or below the thumb is still a grab, which is how a
    /// reader jumps to a position in one press.
    pub fn hits_column(&self, x: u16, y: u16) -> bool {
        x == self.x + self.width.saturating_sub(1)
            && y >= self.y
            && y < self.y.saturating_add(self.height)
    }
}

/// Convert a thumb offset within the track into a `scroll_offset`.
///
/// Pi's half of the drag (`tui-alt-screen.ts:741-760`):
///
/// ```ts
/// const maxThumbOffset = geometry.trackHeight - geometry.thumbHeight;
/// const thumbOffset = clamp(event.y - geometry.trackTop - grabOffset);
/// const scrollTop = round((thumbOffset / maxThumbOffset) * geometry.maxScrollTop);
/// scrollView.scrollTo(scrollTop);
/// ```
///
/// Then the part Pi does not have to write: turning a top-down row position
/// into nini's bottom-up line count, using the row heights the renderer
/// measured.
///
/// `None` when there is nothing to scroll, which is also the signal that a
/// drag should not start.
pub fn line_offset_for_thumb_offset(
    line_heights: &[usize],
    thumb_offset: usize,
    geometry: &ScrollbarGeometry,
) -> Option<usize> {
    if line_heights.is_empty() || geometry.max_scroll_top == 0 {
        return None;
    }
    let max_thumb_offset = (geometry.track_height - geometry.thumb_height) as usize;
    let thumb_offset = thumb_offset.min(max_thumb_offset);

    // Pi's `scrollTop`, measured from the top.
    let scroll_top = if max_thumb_offset == 0 {
        0
    } else {
        ((thumb_offset as f64 / max_thumb_offset as f64) * geometry.max_scroll_top as f64).round()
            as usize
    };

    // nini measures from the bottom.
    let items_skipped = geometry.max_scroll_top.saturating_sub(scroll_top);
    Some(lines_needed_for_items(
        line_heights,
        items_skipped,
        line_heights.len(),
    ))
}

/// Rows a wheel notch moves, matching Pi's `wheelScrollLines`
/// (`tui-alt-screen.ts:144-146`), whose default is 3.
///
/// Expressed in *lines* because that is the unit `scroll_offset` counts, so
/// this is not Pi's arithmetic exactly — that difference is the line-vs-cell
/// gap already recorded in the plan.
pub fn wheel_lines() -> usize {
    WHEEL_SCROLL_LINES
}

/// Pi's `wheelScrollLines` default.
pub const WHEEL_SCROLL_LINES: usize = 3;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_overflows_nothing_is_drawn() {
        assert!(geometry(0, 0, 80, 24, 20, 24, 0).is_none());
        assert!(
            geometry(0, 0, 80, 24, 24, 24, 0).is_none(),
            "content exactly filling the viewport has nothing to scroll"
        );
    }

    #[test]
    fn a_zero_width_area_has_no_column() {
        assert!(geometry(0, 0, 0, 24, 60, 24, 0).is_none());
        assert!(geometry(0, 0, 80, 0, 60, 24, 0).is_none());
    }

    #[test]
    fn the_bar_occupies_the_last_column() {
        let g = geometry(4, 2, 80, 24, 60, 24, 0).unwrap();
        assert_eq!(g.column, 4 + 80 - 1, "layout.ts: `rect.x + rect.width - 1`");
    }

    /// `thumbHeight = round(trackHeight² / contentHeight)`.
    #[test]
    fn the_thumb_shrinks_with_content() {
        let g = geometry(0, 0, 80, 24, 60, 24, 0).unwrap();
        // 24 * 24 / 60 = 9.6 -> 10
        assert_eq!(g.thumb_height, 10);

        let g = geometry(0, 0, 80, 24, 240, 24, 0).unwrap();
        // 576 / 240 = 2.4 -> 2, which is also the floor
        assert_eq!(g.thumb_height, 2);

        let g = geometry(0, 0, 80, 24, 1200, 24, 0).unwrap();
        // 576 / 1200 = 0.48 -> 0, floored to 2 so it stays grabbable
        assert_eq!(g.thumb_height, 2, "the 2-row floor is Pi's minimum thumb");
    }

    #[test]
    fn a_tiny_track_still_gets_a_thumb() {
        // trackHeight 2, content 100: 4/100 = 0.04 -> 0, floored to min(2, 2).
        let g = geometry(0, 0, 80, 2, 100, 2, 0).unwrap();
        assert_eq!(g.thumb_height, 2);
        assert_eq!(g.thumb_height, g.track_height, "i.e. it fills the track");
    }

    /// The live tail is the maximum scroll position, so the thumb is at the
    /// bottom. Getting this backwards would park it at the top while the
    /// model streams, which is the most visible way to get it wrong.
    #[test]
    fn at_the_live_tail_the_thumb_is_at_the_bottom() {
        let g = geometry(0, 0, 80, 24, 60, 24, 0).unwrap();
        assert_eq!(g.max_scroll_top, 36);
        assert_eq!(g.thumb_top, g.track_top + (g.track_height - g.thumb_height));
    }

    #[test]
    fn scrolled_to_the_top_the_thumb_is_at_the_top() {
        let g = geometry(0, 0, 80, 24, 60, 24, 36).unwrap();
        assert_eq!(g.thumb_top, g.track_top);
    }

    /// Content 60, viewport 24, so `maxScrollTop` is 36 and the thumb travels
    /// 14 rows. `items_skipped_from_bottom` is inverted into a top-down
    /// position, so 0 is the bottom of the track, 18 the middle, 36 the top.
    #[test]
    fn the_thumb_travels_proportionally() {
        let bottom = geometry(0, 0, 80, 24, 60, 24, 0).unwrap();
        let middle = geometry(0, 0, 80, 24, 60, 24, 18).unwrap();
        let top = geometry(0, 0, 80, 24, 60, 24, 36).unwrap();

        assert_eq!(bottom.thumb_top, bottom.track_top + 14, "live tail");
        assert_eq!(middle.thumb_top, middle.track_top + 7, "18 of 36 is half");
        assert_eq!(top.thumb_top, top.track_top, "scrolled to the start");
    }

    /// nini measures from the bottom and Pi from the top, and this is the
    /// line where that gets converted.
    #[test]
    fn a_skip_beyond_the_end_is_clamped_not_wrapped() {
        let g = geometry(0, 0, 80, 24, 60, 24, 9999).unwrap();
        assert_eq!(g.thumb_top, g.track_top, "must not wrap to the bottom");
    }

    #[test]
    fn the_thumb_never_leaves_its_track() {
        for skip in [0usize, 1, 5, 17, 35, 36, 37, 1000] {
            let g = geometry(3, 5, 80, 24, 60, 24, skip).unwrap();
            assert!(
                g.thumb_top >= g.track_top,
                "skip {skip}: thumb top {} above track {}",
                g.thumb_top,
                g.track_top
            );
            assert!(
                g.thumb_top + g.thumb_height <= g.track_top + g.track_height,
                "skip {skip}: thumb runs off the bottom of the track"
            );
        }
    }

    // ---- visibility ----

    #[test]
    fn hidden_never_draws() {
        assert!(!should_draw(ScrollbarMode::Hidden, 600, 24, 0));
    }

    #[test]
    fn always_draws_whenever_there_is_a_viewport() {
        assert!(should_draw(ScrollbarMode::Always, 0, 24, 0));
        assert!(should_draw(ScrollbarMode::Always, 600, 24, 999_999));
        assert!(!should_draw(ScrollbarMode::Always, 600, 0, 0));
    }

    #[test]
    fn auto_needs_overflow_and_recent_activity() {
        assert!(should_draw(ScrollbarMode::Auto, 600, 24, 0));
        assert!(should_draw(
            ScrollbarMode::Auto,
            600,
            24,
            TRANSIENT_HIDE_MS - 1
        ));
        assert!(
            !should_draw(ScrollbarMode::Auto, 600, 24, TRANSIENT_HIDE_MS),
            "the hide delay is inclusive"
        );
        assert!(!should_draw(ScrollbarMode::Auto, 20, 24, 0), "no overflow");
    }

    #[test]
    fn the_setting_defaults_to_auto_like_pis() {
        assert_eq!(ScrollbarMode::from_setting(None), ScrollbarMode::Auto);
        assert_eq!(
            ScrollbarMode::from_setting(Some("nonsense")),
            ScrollbarMode::Auto,
            "Pi coerces anything it does not recognise to auto"
        );
        assert_eq!(
            ScrollbarMode::from_setting(Some("always")),
            ScrollbarMode::Always
        );
        assert_eq!(
            ScrollbarMode::from_setting(Some("hidden")),
            ScrollbarMode::Hidden
        );
        assert_eq!(ScrollbarMode::default(), ScrollbarMode::Auto);
    }

    // ---- mouse: thumb offset -> line offset ----

    /// Forty transcript lines, each two rendered rows, in a 17-row viewport.
    fn tall_lines() -> Vec<usize> {
        vec![2usize; 40]
    }

    fn viewport_of() -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width: 80,
            height: 17,
            content_height: 80,
            viewport_height: 17,
        }
    }

    #[test]
    fn the_column_is_the_areas_last_one() {
        let vp = Viewport {
            x: 4,
            y: 3,
            width: 80,
            height: 17,
            ..viewport_of()
        };
        assert!(vp.hits_column(4 + 79, 3));
        assert!(vp.hits_column(83, 19));
        assert!(!vp.hits_column(82, 10), "one column to the left");
        assert!(!vp.hits_column(83, 2), "above the track");
        assert!(!vp.hits_column(83, 20), "below the track");
    }

    #[test]
    fn a_thumb_at_the_bottom_means_the_live_tail() {
        let vp = viewport_of();
        let g = vp.geometry(0).unwrap();
        let next = line_offset_for_thumb_offset(&tall_lines(), (17 - g.thumb_height) as usize, &g)
            .unwrap();
        assert_eq!(
            next, 0,
            "dragging the thumb to the bottom returns to the tail"
        );
    }

    /// The top of the track means one viewport's worth of rows hidden, which
    /// in lines is however many lines those rows occupy - not all of them.
    ///
    /// The first draft asserted 40, on the assumption that "top" meant "every
    /// line skipped". It does not: `maxScrollTop` is `contentHeight -
    /// viewportHeight` = 80 - 17 = 63 rows, and at two rows per line that is
    /// 32 lines. Skipping all 40 would hide 80 rows, twice as much as the
    /// viewport can scroll.
    #[test]
    fn a_thumb_at_the_top_hides_exactly_one_viewport() {
        let vp = viewport_of();
        let g = vp.geometry(0).unwrap();
        let next = line_offset_for_thumb_offset(&tall_lines(), 0, &g).unwrap();
        assert_eq!(g.max_scroll_top, 63);
        assert_eq!(next, 32, "63 rows at 2 rows per line");
        assert!(next < tall_lines().len(), "and not every line");
    }

    #[test]
    fn a_drag_moves_monotonically() {
        let vp = viewport_of();
        let g = vp.geometry(0).unwrap();
        let travel = (17 - g.thumb_height) as usize;
        let mut previous = usize::MAX;
        for step in 0..=travel {
            let offset = line_offset_for_thumb_offset(&tall_lines(), step, &g).unwrap();
            assert!(
                offset <= previous,
                "step {step}: offset went from {previous} up to {offset}"
            );
            previous = offset;
        }
    }

    #[test]
    fn a_drag_past_the_track_clamps_rather_than_wrapping() {
        let vp = viewport_of();
        let g = vp.geometry(0).unwrap();
        let huge = line_offset_for_thumb_offset(&tall_lines(), 99_999, &g).unwrap();
        let bottom =
            line_offset_for_thumb_offset(&tall_lines(), (17 - g.thumb_height) as usize, &g)
                .unwrap();
        assert_eq!(
            huge, bottom,
            "clamped to the end of the track, which is the live tail"
        );

        let top = line_offset_for_thumb_offset(&tall_lines(), 0, &g).unwrap();
        assert_ne!(huge, top, "and certainly not wrapped round to the top");
    }

    #[test]
    fn nothing_to_scroll_is_not_a_drag() {
        let vp = Viewport {
            content_height: 10,
            viewport_height: 17,
            ..viewport_of()
        };
        assert!(vp.geometry(0).is_none());
    }

    /// Empty heights means no frame has been drawn, or the transcript is
    /// empty. Either way there is nothing to convert, and the drag must be
    /// refused rather than silently scrolling to zero.
    #[test]
    fn without_measured_heights_a_drag_is_refused() {
        let vp = viewport_of();
        let g = vp.geometry(0).unwrap();
        assert!(line_offset_for_thumb_offset(&[], 5, &g).is_none());
    }

    /// Uneven lines are the reason the conversion exists at all: forty lines
    /// of equal height would let a character count stand in for a row count.
    #[test]
    fn uneven_lines_still_land_on_the_right_row() {
        let heights: Vec<usize> = (0..40).map(|i| if i % 5 == 0 { 7 } else { 1 }).collect();
        let total: usize = heights.iter().sum();
        let vp = Viewport {
            content_height: total,
            ..viewport_of()
        };
        let g = vp.geometry(0).unwrap();

        let top = line_offset_for_thumb_offset(&heights, 0, &g).unwrap();
        let top_rows: usize = heights[..top].iter().sum();

        // Lines are discrete, so converting `maxScrollTop` rows into whole
        // lines rounds *up*: the last line pulled in overshoots. The first
        // draft asserted `top_rows == max_scroll_top` and got 77 against 71.
        // What is actually guaranteed is that at least a viewport is hidden
        // and that the overshoot is under one line.
        let tallest = heights.iter().copied().max().unwrap_or(1);
        assert!(
            top_rows >= g.max_scroll_top,
            "the top must hide a full viewport: {top_rows} < {}",
            g.max_scroll_top
        );
        assert!(
            top_rows < g.max_scroll_top + tallest,
            "and must not overshoot by a whole line: {top_rows} vs {}",
            g.max_scroll_top
        );

        let bottom =
            line_offset_for_thumb_offset(&heights, (17 - g.thumb_height) as usize, &g).unwrap();
        assert_eq!(bottom, 0);
    }

    #[test]
    fn the_wheel_moves_pi_s_default_number_of_lines() {
        assert_eq!(wheel_lines(), WHEEL_SCROLL_LINES);
        assert_eq!(wheel_lines(), 3);
    }
}
