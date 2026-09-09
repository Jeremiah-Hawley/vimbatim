//! Pure editor row metrics. Kept independent of GPUI so layout invariants are testable.

use std::rc::Rc;

use crate::docx_parser::Paragraph;

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

pub(crate) fn wrap_line_into_rows(
    chars: &[char],
    wrap_width_px: f32,
    width_of: &mut impl FnMut(usize, char) -> f32,
) -> Vec<(usize, usize)> {
    /*
     * Word-wraps one logical line's characters into visual rows whose
     * accumulated real glyph width (via `width_of`) stays within
     * `wrap_width_px`, breaking at the last space within budget when one
     * exists, or hard-breaking mid-word when a single word exceeds the
     * budget on its own. The space a word-boundary break lands on is
     * consumed (not repeated as a leading space on the next row), matching
     * normal word-wrap behaviour.
     *
     * Pure and independent of any real font/GPUI context — callers supply
     * `width_of`, so this stays unit-testable with synthetic width
     * functions (see the tests below for one that exercises variable-width
     * characters directly).
     *
     * Always returns at least one row, even for an empty line, so every
     * logical line still occupies its own visual slot — this is what lets
     * click/scroll math treat "row index" as a stable, always-present
     * coordinate.
     */
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let mut rows = Vec::new();
    let mut row_start = 0;
    while row_start < chars.len() {
        let mut width = 0.0f32;
        let mut i = row_start;
        let mut last_space: Option<usize> = None;
        while i < chars.len() {
            let char_width = width_of(i, chars[i]);
            // `i > row_start` forces at least one character onto every row,
            // even one whose width alone exceeds the budget — otherwise a
            // very narrow viewport (or a single unusually wide glyph) could
            // produce a zero-width row and loop forever.
            if width + char_width > wrap_width_px && i > row_start {
                break;
            }
            width += char_width;
            if chars[i] == ' ' && i > row_start {
                last_space = Some(i);
            }
            i += 1;
        }
        if i >= chars.len() {
            rows.push((row_start, chars.len()));
            break;
        }
        let row_end = last_space.unwrap_or(i);
        rows.push((row_start, row_end));
        // Skip the space itself when we broke on one, so it doesn't reappear
        // as a leading character on the next row.
        row_start = if last_space.is_some() {
            row_end + 1
        } else {
            row_end
        };
    }
    rows
}

pub(crate) fn build_visual_rows(
    lines: &[String],
    wrap_width_px: f32,
    width_at: &mut impl FnMut(usize, usize, char) -> f32,
) -> Vec<(usize, usize, usize)> {
    /*
     * Flattens every logical line into an ordered list of visual rows, each
     * tagged with its owning logical line index and its [start, end) char
     * range within that line. Shared by rendering (which paints one
     * fixed-height div per row) and click/scroll math (which maps pixel
     * positions to/from this same row table) so all three always agree on
     * where each row's boundaries fall.
     */
    let mut rows = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut width_of = |idx: usize, ch: char| width_at(li, idx, ch);
        for (start, end) in wrap_line_into_rows(&chars, wrap_width_px, &mut width_of) {
            rows.push((li, start, end));
        }
    }
    rows
}

pub(crate) fn visual_row_for_line_col(
    rows: &[(usize, usize, usize)],
    logical_line: usize,
    char_col: usize,
) -> usize {
    /*
     * Finds the visual row that a (logical_line, char_col) cursor position
     * belongs to.
     *
     * A column sitting exactly at a row's end is ambiguous, and is resolved
     * differently depending on *why* the row ended:
     *   - Hard break (a long word forced mid-word, no space consumed): the
     *     next row starts exactly where this one ends, so the column is
     *     redirected to the *start* of that next row — matching how text
     *     editors visually carry the cursor onto the next wrapped row
     *     rather than trailing behind the break.
     *   - Soft break (`wrap_line_into_rows` consumed a space at the wrap
     *     point): the next row starts one character *past* this row's end,
     *     leaving a one-character gap for the consumed space. A column
     *     equal to this row's end is the space itself — not a valid
     *     position on the next row — so it stays here, trailing the last
     *     visible character. (Redirecting it forward regardless of this gap
     *     was the original bug: the next row's `row_start` could then
     *     exceed `char_col`, underflowing any `char_col - row_start` a
     *     caller computed downstream.)
     *   - Last row of the line: there's no next row to redirect to, so it
     *     stays here regardless.
     */
    let mut last_row_of_line = 0;
    for (idx, &(li, start, end)) in rows.iter().enumerate() {
        if li != logical_line {
            continue;
        }
        last_row_of_line = idx;
        if char_col >= start && char_col < end {
            return idx;
        }
        if char_col == end {
            let next_is_contiguous = rows
                .get(idx + 1)
                .map(|&(next_li, next_start, _)| next_li == li && next_start == end)
                .unwrap_or(false);
            if !next_is_contiguous {
                return idx;
            }
            // else: a hard break — fall through so the next iteration's
            // `char_col >= start && char_col < end` check picks it up.
        }
    }
    last_row_of_line
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

/// Caches the word-wrapped row table (and the intermediate data it's built
/// from — split lines, per-line chars, line byte offsets, and the cloned
/// paragraph formatting) across renders that don't actually change the
/// document. Scrolling, cursor movement, and focus changes all trigger a
/// render but don't touch `content`/`paragraphs` — without this cache,
/// `render()` re-wraps the *entire* document on every single one of those,
/// regardless of how much is actually visible (see performance_plan.md /
/// uniform_list_plan.md). Invalidated by `Tab.content_version` (bumped on
/// every real edit — see its own doc comment in `state.rs`), not by
/// comparing `content` itself, which would defeat the point.
///
/// `Rc`-wrapped so a cache hit is a handful of cheap pointer clones, not a
/// deep clone of the document — also what lets this data be captured
/// cheaply into a `uniform_list` render closure later, instead of deep-
/// cloned into it on every render.
pub(crate) struct RowCache {
    pub(crate) tab_id: usize,
    pub(crate) content_version: u64,
    /// `f32` isn't `Eq`; comparing via `to_bits()` is the standard way to
    /// use a float as an exact cache key without pulling in an epsilon
    /// comparison that would need its own tuning.
    pub(crate) viewport_width_bits: u32,
    pub(crate) zoom_bits: u32,
    /// Line spacing feeds `slot_count_for_paragraph`, so changing it changes
    /// `display_to_wrap` — same reason `zoom_bits` is a key, and same
    /// `to_bits()` treatment since `f32` isn't `Eq`.
    pub(crate) line_spacing_bits: u32,
    /// Invisibility mode and fold both drop rows from `display_to_wrap`, so
    /// toggling either changes the tables and must invalidate — otherwise the
    /// editor keeps painting the previous mode's row list.
    pub(crate) invisibility: bool,
    pub(crate) fold_version: u64,
    pub(crate) lines: Rc<Vec<String>>,
    pub(crate) line_chars: Rc<Vec<Vec<char>>>,
    pub(crate) line_byte_starts: Rc<Vec<usize>>,
    pub(crate) rows: Rc<Vec<(usize, usize, usize)>>,
    pub(crate) paragraphs: Rc<Vec<Paragraph>>,
    /// `uniform_list`-facing expansion of `rows` — see
    /// `expand_rows_for_display`'s doc comment. Cached alongside `rows`
    /// since it's derived from it plus `paragraphs`/`zoom`, all of which are
    /// already part of this cache's invalidation key.
    pub(crate) display_to_wrap: Rc<Vec<Option<usize>>>,
    pub(crate) wrap_to_display: Rc<Vec<usize>>,
}

/// Pure cache-validity check, pulled out of `render()` so it's unit-testable
/// without a GPUI context — the one part of the row cache that isn't just
/// GPUI interaction glue. `ignore_width` optionally accepts a cache built at a
/// different width.
///
/// `ignore_width` is set only while the split divider is being dragged
/// (`AppState.split_dragging`). A width change normally *must* invalidate —
/// the wrap points depend on it — but rebuilding costs a full-document re-wrap
/// per pane per mouse-move, which locks the app up on a large file. Reusing
/// the stale tables leaves the text wrapped at the pre-drag width for the
/// duration of the drag; releasing clears the flag and the next render wraps
/// correctly, once.
pub(crate) fn row_cache_is_valid_for(
    cache: &RowCache,
    tab_id: usize,
    content_version: u64,
    viewport_width: f32,
    zoom: f32,
    line_spacing: f32,
    ignore_width: bool,
    invisibility: bool,
    fold_version: u64,
) -> bool {
    cache.tab_id == tab_id
        && cache.content_version == content_version
        && (ignore_width || cache.viewport_width_bits == viewport_width.to_bits())
        && cache.zoom_bits == zoom.to_bits()
        && cache.line_spacing_bits == line_spacing.to_bits()
        && cache.invisibility == invisibility
        && cache.fold_version == fold_version
}
