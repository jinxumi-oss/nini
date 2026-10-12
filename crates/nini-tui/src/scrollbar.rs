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
}

/// Milliseconds since the Unix epoch.
///
/// Only ever compared against another reading of the same clock, so
/// `SystemTime` is enough and pulls in no dependency. Panics on a clock set
/// before 1970 are not reachable here — `saturating_sub` at the call site
/// covers a zero reading.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
