//! Pure editor row metrics. Kept independent of GPUI so layout invariants are testable.

const LINE_HEIGHT_RATIO: f32 = 20.0 / 14.0;
const ROW_SUBDIVISIONS: usize = 6;

pub(crate) fn line_height_px(normal_size_px: f32, spacing: f32) -> f32 {
    normal_size_px * LINE_HEIGHT_RATIO * spacing
}

pub(crate) fn text_line_box_px(font_px: f32) -> f32 {
    (font_px * LINE_HEIGHT_RATIO).floor().max(1.0)
}

pub(crate) fn row_slot_px(normal_size_px: f32, spacing: f32, zoom: f32) -> f32 {
    line_height_px(normal_size_px, spacing) * zoom / ROW_SUBDIVISIONS as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_slot_is_one_subdivision_of_a_line() {
        assert_eq!(
            row_slot_px(14.0, 1.0, 1.0) * ROW_SUBDIVISIONS as f32,
            line_height_px(14.0, 1.0)
        );
        assert!(text_line_box_px(14.0) <= line_height_px(14.0, 1.0));
    }
}
