//! Pure editor row metrics. Kept independent of GPUI so layout invariants are testable.

use std::rc::Rc;

use crate::docx_parser::{ListKind, Paragraph};

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

// Pure text/list presentation helpers extracted from the GPUI view.

/// One-char-for-one-char substitution of any control character -> `' '` for
/// display only — `render_line`'s and `char_width_fn`'s shared reasoning for
/// why a raw control character can't be handed to GPUI's shaper (no glyph in
/// the bundled fonts) and why swapping it for a space here can't desync any
/// offset-based computation downstream (cursor/selection/misspelled ranges
/// all index by position, not content). Never touches the actual document
/// model — callers pass in a line already read out of `tab.document.content()`/
/// `paragraphs`, which still holds the real character for undo/.docx
/// export/Verbatim round-trip fidelity.
///
/// Originally just `'\t'` (GPUI paints it with zero width — no on-screen gap,
/// cursor doesn't visually advance). Widened to the whole control-character
/// class after a `'\r'` embedded mid-paragraph (reachable via typing, paste,
/// or a `.docx` whose `<w:t>` contains a literal CR instead of `<w:br/>`)
/// was found to make GPUI drop the *entire* text fragment containing it —
/// not just that one character — since `'\r'` never triggers this codebase's
/// own paragraph-split logic (only `'\n'` does) and GPUI's `shape_line` only
/// asserts against `'\n'` too, so it reaches the shaper unguarded. A `'\n'`
/// itself can never reach here (every `line` this app ever builds is already
/// split on it), but including it costs nothing.
pub(crate) fn display_line(line: &str) -> std::borrow::Cow<'_, str> {
    if line.contains(char::is_control) {
        std::borrow::Cow::Owned(line.replace(char::is_control, " "))
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

/// 1-indexed letter for `NumberLowerLetterDot`/`NumberLowerLetterParen`/
/// `NumberUpperLetter` — `1 -> "a"`, `26 -> "z"`, wrapping into double
/// letters beyond that the way Word's own `lowerLetter`/`upperLetter`
/// formats do (`27 -> "aa"`), which this app's practical list lengths won't
/// reach but is cheap to get right regardless.
pub(crate) fn to_letter(n: u32) -> String {
    let mut n = n;
    let mut out = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push((b'a' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    out.iter().rev().collect()
}

/// 1-indexed lowercase Roman numeral, for `NumberLowerRoman`/`NumberUpperRoman`
/// (the caller upper-cases when needed). Covers this app's practical list
/// range (well past 100) with the standard subtractive-notation table.
pub(crate) fn to_roman(n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (value, symbol) in TABLE {
        while n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    out
}

/// The text painted in a list paragraph's gutter: the fixed glyph for a
/// bullet kind, or `ordinal` formatted per the number kind's own template.
///
/// Deliberately does **not** paint Word's real Wingdings/Symbol
/// private-use-area codepoints (`docs/superpowers/specs/2026-08-11-lists-design.md`'s
/// ground-truth table) — this app bundles only DejaVu Sans Mono
/// (`FONT_FAMILY`), which has no glyphs at those codepoints, so painting
/// them here would show missing-glyph boxes on screen. The saved `.docx`
/// still gets Word's real codepoints (`docx_parser::build_numbering_xml`) —
/// this is purely an in-app rendering substitution, the same category of
/// deliberate simplification as `Run.font` already not being applied to
/// on-screen rendering (see that field's own doc comment).
pub(crate) fn list_marker_text(kind: ListKind, ordinal: u32) -> String {
    match kind {
        ListKind::BulletSolid => "\u{2022}".to_string(),
        ListKind::BulletHollow => "o".to_string(),
        ListKind::BulletSolidBox => "\u{25aa}".to_string(),
        ListKind::BulletDiamond => "\u{25c6}".to_string(),
        ListKind::BulletArrow => "\u{2192}".to_string(),
        ListKind::BulletCheckmark => "\u{2713}".to_string(),
        ListKind::NumberDecimalDot => format!("{ordinal}."),
        ListKind::NumberDecimalParen => format!("{ordinal})"),
        ListKind::NumberUpperRoman => format!("{}.", to_roman(ordinal).to_uppercase()),
        ListKind::NumberUpperLetter => format!("{}.", to_letter(ordinal).to_uppercase()),
        ListKind::NumberLowerLetterParen => format!("{})", to_letter(ordinal)),
        ListKind::NumberLowerLetterDot => format!("{}.", to_letter(ordinal)),
        ListKind::NumberLowerRoman => format!("{}.", to_roman(ordinal)),
    }
}

/// `list_marker_text`, extended for `level` (Phase 2 multi-level indent).
/// Level 0 uses the paragraph's own picked `kind`; levels 1+ always use the
/// one fixed cascade confirmed from real Word — the *complete* ilvl 0-8
/// range dumped from `Lists.docx`'s `BulletSolid`/`NumberDecimalDot`/
/// `NumberUpperRoman` `abstractNum`s, matching `docx_parser::cascade_level_xml`
/// exactly (see that function's own doc comment, including its fix
/// history — an earlier version of this pair only checked ilvl 0-2 and
/// shipped a wrong 2-value cascade for 1 for every 3 wrapped levels):
/// bullets go hollow `o` (level 1) -> filled square (level 2) -> plain
/// solid bullet (level 3) -> repeats every 3 levels; numbers go
/// `lowerLetter` (level 1) -> `lowerRoman` (level 2) -> `decimal` (level 3)
/// -> repeats every 3 levels — independent of which style was picked at
/// level 0. Uses this app's own in-app glyph substitutes (see
/// `list_marker_text`'s doc comment on why), not Word's real Wingdings
/// codepoints — reusing `list_marker_text`'s own `BulletSolid`/
/// `NumberDecimalDot` cases for the level-3-position glyph/format keeps
/// both cascade positions in exactly one place each.
pub(crate) fn list_marker_text_for_level(kind: ListKind, level: u8, ordinal: u32) -> String {
    if level == 0 {
        return list_marker_text(kind, ordinal);
    }
    let cascade_pos = (level - 1) % 3;
    if kind.is_bullet() {
        match cascade_pos {
            0 => "o".to_string(),
            1 => "\u{25aa}".to_string(),
            _ => list_marker_text(ListKind::BulletSolid, ordinal),
        }
    } else {
        match cascade_pos {
            0 => format!("{}.", to_letter(ordinal)),
            1 => format!("{}.", to_roman(ordinal)),
            _ => list_marker_text(ListKind::NumberDecimalDot, ordinal),
        }
    }
}

/// 1-indexed position of `paragraphs[index]` within its own contiguous run
/// of *same-`ListKind`* list paragraphs — the same run-boundary rule
/// `docx_parser::assign_list_num_ids` uses on the write side (confirmed
/// against real Word's own behavior: a style change starts a new list even
/// with no non-list paragraph between them — see that function's own doc
/// comment), so the on-screen ordinal always matches what gets saved and
/// what numId a paragraph actually resolves to. Walks backward from `index`
/// while the preceding paragraph is a list item of the *same kind*; a
/// non-list paragraph, a different `ListKind`, or the start of the document
/// ends the run. `index` itself must carry a list, or the count is
/// meaningless — callers only invoke this after checking
/// `paragraphs[index].list.is_some()`.
pub(crate) fn list_item_ordinal(paragraphs: &[Paragraph], index: usize) -> u32 {
    let Some(run_kind) = paragraphs[index].list.map(|item| item.kind) else {
        return 1;
    };
    let mut ordinal = 1u32;
    let mut i = index;
    while i > 0 && paragraphs[i - 1].list.map(|item| item.kind) == Some(run_kind) {
        ordinal += 1;
        i -= 1;
    }
    ordinal
}

pub(crate) fn document_lines(content: &str) -> Vec<String> {
    /*
     * Splits document content into logical lines on '\n', matching the model
     * used throughout rendering and click/scroll math. An empty document is
     * still one (empty) line so the editor always has somewhere to place
     * the cursor.
     */
    if content.is_empty() {
        vec![String::new()]
    } else {
        content.split('\n').map(|l| l.to_string()).collect()
    }
}

pub(crate) fn line_for_y(y: f32, line_height: f32, num_rows: usize) -> usize {
    /*
     * Converts a y pixel offset (relative to the start of the text) into a
     * 0-indexed visual row number, clamped to `num_rows - 1` so a click
     * below the last row still lands on it rather than panicking on an
     * out-of-range row index.
     */
    if line_height <= 0.0 || num_rows == 0 {
        return 0;
    }
    if y <= 0.0 {
        return 0;
    }
    ((y / line_height) as usize).min(num_rows - 1)
}
