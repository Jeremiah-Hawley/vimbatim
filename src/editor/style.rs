//! Pure presentation style calculations.

/// Returns the zoomed font size for a heading; body text has no override.
pub(crate) fn heading_font_size_px(heading: u8, zoom: f32) -> Option<f32> {
    match heading {
        0 => None,
        1 => Some(24.0 * zoom),
        2 => Some(20.0 * zoom),
        3 => Some(18.0 * zoom),
        4..=6 => Some(16.0 * zoom),
        _ => Some(14.0 * zoom),
    }
}

/// Font size used to reserve a paragraph's visual row.
pub(crate) fn line_font_px(
    para: Option<&crate::docx_parser::Paragraph>,
    zoom: f32,
    normal_size_px: f32,
) -> f32 {
    let Some(para) = para else {
        return normal_size_px * zoom;
    };
    if para.heading == 4 {
        return normal_size_px * zoom;
    }
    // Emphasis runs are no longer excluded. They were, because the quantum
    // used to be a whole line: one emphasized word in a plain paragraph cost
    // the entire paragraph a full extra line of spacing (that bug report is
    // what added the filter). With `ROW_SUBDIVISIONS` the same run now costs
    // a fraction of a line, which is small enough to be the honest answer —
    // and excluding it is what left an emphasis box painting outside the row
    // reserved for it, overlapping the box on the line above.
    let run_max_px = para
        .runs
        .iter()
        .filter(|r| r.size > 0 && r.style != Some(crate::docx_parser::CardStyle::Cite))
        .map(|r| r.size as f32 / 2.0 * zoom)
        .fold(0.0_f32, f32::max);
    // An explicit run-level size (card styles always set one, covering the
    // whole line) already reflects what's actually drawn and wins over the
    // heading fallback below it — using both would over-reserve whenever the
    // card style's real size is smaller than its heading level's generic
    // default (e.g. Tag: 13px run size vs. heading level 4's 16px fallback),
    // padding in a spurious blank row under every Tag line. `heading_px` is
    // only relevant when no run carries an explicit size — plain document
    // headings with no card-style override.
    let heading_px = heading_font_size_px(para.heading, zoom).unwrap_or(0.0);
    if run_max_px > 0.0 {
        run_max_px
    } else {
        heading_px
    }
    .max(normal_size_px * zoom)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_sizes_are_zoomed() {
        assert_eq!(heading_font_size_px(0, 2.0), None);
        assert_eq!(heading_font_size_px(2, 1.5), Some(30.0));
    }
}
