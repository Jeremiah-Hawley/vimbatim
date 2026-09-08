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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_sizes_are_zoomed() {
        assert_eq!(heading_font_size_px(0, 2.0), None);
        assert_eq!(heading_font_size_px(2, 1.5), Some(30.0));
    }
}
