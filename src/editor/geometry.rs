pub(crate) struct ScrollbarGeometry {
    pub thumb_h: f32,
    pub travel: f32,
    pub thumb_top: f32,
}

pub(crate) fn scrollbar_geometry(
    viewport_h: f32,
    content_h: f32,
    scrolled: f32,
    min_thumb_px: f32,
) -> ScrollbarGeometry {
    let thumb_h = (viewport_h * (viewport_h / content_h))
        .max(min_thumb_px)
        .min(viewport_h);
    let travel = viewport_h - thumb_h;
    let max_scroll = content_h - viewport_h;
    let thumb_top = if max_scroll > 0.0 {
        (scrolled / max_scroll).clamp(0.0, 1.0) * travel
    } else {
        0.0
    };
    ScrollbarGeometry {
        thumb_h,
        travel,
        thumb_top,
    }
}

/// Display slots include the spacers reserved for each rendered line.
pub(crate) fn max_scroll_for_display_rows(
    display_rows: &[Option<usize>],
    slot_px: f32,
    viewport_h: f32,
) -> f32 {
    (display_rows.len() as f32 * slot_px - viewport_h).max(0.0)
}

// Pixel-only reading-mode page movement.
/// The scroll arithmetic behind reading mode's Left/Right paging, split out
/// from `TextEditor::page_scroll` so it is testable without a laid-out view.
///
/// `current` and the result are GPUI scroll offsets: `<= 0`, growing more
/// negative further down the document. `max_y` is the positive maximum scroll
/// distance. Returns `None` when the page would not move — already at that end.
pub(crate) fn page_scroll_offset(
    current: f32,
    viewport_h: f32,
    row_height: f32,
    max_y: f32,
    forward: bool,
) -> Option<f32> {
    // Whole rows only: a raw pixel jump lands mid-row and slices the line
    // straddling the fold, scrolling half of it past unread. At least one row
    // so a viewport shorter than a single line still advances.
    let rows_per_page = (viewport_h / row_height).floor().max(1.0);
    let delta = rows_per_page * row_height;
    let target = if forward {
        current - delta
    } else {
        current + delta
    };
    let clamped = target.clamp(-max_y.max(0.0), 0.0);
    ((clamped - current).abs() >= 0.5).then_some(clamped)
}
