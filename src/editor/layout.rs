//! Pure editor row metrics. Kept independent of GPUI so layout invariants are testable.

use std::rc::Rc;

use crate::document_ops::paragraph_run_char_spans;
use crate::docx_parser::{ListKind, Paragraph, Run};
use crate::editor::style::{
    effective_char_advance_ratio, effective_char_size_px, line_font_px, run_is_hidden,
};

const LINE_HEIGHT_RATIO: f32 = 20.0 / 14.0;
pub(crate) const ROW_SUBDIVISIONS: usize = 6;

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

// GPUI-free row visibility and sizing policy.
/// How many `uniform_list` rows one line of body text is divided into.
///
/// `gpui::uniform_list` forces a single measured height onto every row, so a
/// line taller than one row cannot grow — it can only reserve whole extra
/// rows ahead of itself (`slot_count_for_paragraph`). That reservation is a
/// `ceil`, so the *row* is the quantum of vertical space, and with one row
/// per line every font size from 12pt to 22pt reserved exactly two lines: one
/// abrupt doubling at 12pt, then nothing at all for the next ten points (bug
/// report: "font size 12-22 both increase the distance between lines equally
/// ... this is harsh").
///
/// Subdividing the grid shrinks that quantum without giving up the uniform
/// height `uniform_list` requires: a plain line now occupies
/// `ROW_SUBDIVISIONS` rows instead of one, and a taller line rounds up to the
/// nearest *fraction* of a line. At 6, the step is ~2.6px at the 11px
/// default — roughly one step per point of font size, which reads as
/// continuous growth rather than a cliff.
///
/// This is the tuning knob for that trade: raising it makes spacing smoother
/// and multiplies the display-row table (and the number of small empty divs
/// `uniform_list` builds for the visible range) by the same factor; lowering
/// it is cheaper and chunkier. It must stay >= 1.
pub(crate) const CARD_BOX_EXTRA_PX: f32 = 20.0;

/// Vertical clearance reserved for an `emphasis_boxed` run, on top of the
/// font's own height.
///
/// The emphasis box is an inset box-shadow (`apply_run_style`), which is
/// paint-only and adds nothing to layout — it is drawn at the span's exact
/// bounds. Two consecutive lines carrying one therefore have boxes that are
/// as tall as the rows are apart, and any rounding at all makes them touch
/// or cross (bug report: "the Emphasis boxes on size 12 font overlap such
/// that the top of a box on a lower line is above the bottom of a box from
/// the line above it").
///
/// Relying on `slot_count_for_paragraph`'s own `ceil` to leave a gap is not
/// enough: a font size whose height lands exactly on a slot boundary rounds
/// to zero clearance. This reserves the gap explicitly — 1px above and 1px
/// below the ring. Not scaled by `zoom`, matching the box-shadow's own fixed
/// 1px spread.
pub(crate) const EMPHASIS_BOX_EXTRA_PX: f32 = 2.0;
/// Whether `run` paints a box-shaped visual (a background fill, a real
/// border, or an inset box-shadow standing in for one — see
/// `apply_run_style`'s `emphasis_boxed` case) rather than just styling the
/// glyphs themselves — `render_line`'s fast
/// path has to skip any such run, since its bare-div return isn't wrapped in
/// `line_div`'s per-span `flex_shrink(0.0)` and would otherwise stretch the
/// paint to the full row width instead of hugging the text. `underline`/
/// `strikethrough`/`bold`/`size`/`font`/`color` don't paint a box, so they're
/// deliberately not here.
pub(crate) fn paints_run_box(run: Option<&Run>) -> bool {
    run.is_some_and(|r| r.box_format || r.highlight || r.emphasis_boxed)
}

pub(crate) fn slot_count_for_paragraph(
    para: Option<&Paragraph>,
    zoom: f32,
    normal_size_px: f32,
    line_spacing: f32,
) -> usize {
    // Both early returns mean "exactly one ordinary line", which is
    // `ROW_SUBDIVISIONS` slots now rather than a single one.
    let Some(para) = para else {
        return ROW_SUBDIVISIONS;
    };
    if para.heading == 4 {
        return ROW_SUBDIVISIONS;
    }
    let font_px = line_font_px(Some(para), zoom, normal_size_px);
    let has_box = para.runs.iter().any(|r| r.box_format);
    let has_emphasis_box = para.runs.iter().any(|r| r.emphasis_boxed);
    let slot_px = row_slot_px(normal_size_px, line_spacing, zoom);
    if slot_px <= 0.0 {
        return ROW_SUBDIVISIONS;
    }
    let needed_px = font_px * LINE_HEIGHT_RATIO
        + if has_box { CARD_BOX_EXTRA_PX } else { 0.0 }
        + if has_emphasis_box {
            EMPHASIS_BOX_EXTRA_PX
        } else {
            0.0
        };
    // The epsilon keeps an exact fit from rounding up. A plain line's height
    // is `ROW_SUBDIVISIONS` slots exactly, but that division is float math:
    // a result of 6.0000001 would `ceil` to 7 and make every ordinary line in
    // the document a slot taller than it needs to be. 1e-3 of a slot is far
    // below a pixel and cannot hide a real overflow.
    (((needed_px / slot_px) - 1e-3).ceil() as usize).max(ROW_SUBDIVISIONS)
}

/// Expands the word-wrapped `rows` table (one entry per visual row) into a
/// `uniform_list`-facing "display rows" table that reserves extra blank
/// slots *before* any oversized paragraph (see `slot_count_for_paragraph`).
/// Before, not after: `row_div` bottom-aligns its content (`justify_end`),
/// so an oversized paragraph's real overflow spills upward out of its slot,
/// not downward — reserving the blank space after it left the overflow with
/// nowhere to go but into the row above (or the ribbon toolbar, for the
/// first line in the file).
///
/// Returns `(display_to_wrap, wrap_to_display)`:
/// - `display_to_wrap[display_idx]` is `Some(wrap_idx)` for the row that
///   holds real content (the last of its reserved slots), or `None` for a
///   blank spacer slot reserved before it.
/// - `wrap_to_display[wrap_idx]` is the display index a given wrap-table
///   row starts at — needed anywhere pixel math is keyed off a wrap-row
///   index (cursor position, scroll-to-cursor) so it accounts for spacer
///   rows inserted earlier in the document.
///
/// Rows that paint nothing are dropped rather than retained as blank lines.
///
/// Two independent reasons a row disappears:
///
/// * **Fold** hides whatever sits under a collapsed heading. `folded_paras` is
///   the per-paragraph map `AppState::folded_paragraphs` computed, which is
///   level-aware — collapsing a Pocket takes its Hats, Blocks and Tags with it,
///   not just its prose.
/// * **Invisibility** hides individual runs, and a row whose every run is
///   hidden has nothing left to paint.
///
/// Fold is checked first because it is coarser: a folded body row is gone
/// regardless of what it contains, including highlights.
pub(crate) fn hidden_wrap_rows(
    rows: &[(usize, usize, usize)],
    paragraphs: &[Paragraph],
    invisibility: bool,
    cite_size_half_points: u16,
    folded_paras: &[bool],
) -> Vec<bool> {
    if !invisibility && folded_paras.iter().all(|f| !f) {
        return vec![false; rows.len()];
    }
    rows.iter()
        .map(|&(li, row_start, row_end)| {
            let Some(para) = paragraphs.get(li) else {
                return true;
            };
            if folded_paras.get(li).copied().unwrap_or(false) {
                return true;
            }
            if !invisibility {
                return false;
            }
            if para.heading != 0 {
                return false; // a card-style line stays whole
            }
            !paragraph_run_char_spans(para)
                .into_iter()
                .any(|(s, e, run_idx)| {
                    // Only runs actually on this row decide it.
                    if s.max(row_start) >= e.min(row_end) {
                        return false;
                    }
                    let run = para.runs.get(run_idx);
                    !run_is_hidden(
                        true,
                        para.heading,
                        run.is_some_and(|r| r.highlight),
                        run.is_some_and(|r| r.bold),
                        run.map(|r| r.size).unwrap_or(0),
                        cite_size_half_points,
                    )
                })
        })
        .collect()
}

pub(crate) fn expand_rows_for_display(
    rows: &[(usize, usize, usize)],
    paragraphs: &[Paragraph],
    zoom: f32,
    hidden: &[bool],
    normal_size_px: f32,
    line_spacing: f32,
) -> (Vec<Option<usize>>, Vec<usize>) {
    let mut display_to_wrap = Vec::with_capacity(rows.len());
    let mut wrap_to_display = Vec::with_capacity(rows.len());
    for (wrap_idx, (li, _, _)) in rows.iter().enumerate() {
        // A fully hidden row gets no display slot, which is what closes up the
        // vertical gap. It still records a `wrap_to_display` entry pointing at
        // wherever the next visible row lands, so cursor scrolling on a hidden
        // row resolves to the nearest thing actually on screen.
        if hidden.get(wrap_idx).copied().unwrap_or(false) {
            wrap_to_display.push(display_to_wrap.len());
            continue;
        }
        // Blank filler slots go *before* the content row, not after: `row_div`
        // bottom-aligns its content (`justify_end`, for aligning mixed-size
        // text on the bottom rather than the top — see its own comment), so
        // a paragraph too tall for one slot overflows *upward* out of its
        // slot, not downward. Filler reserved after it left nothing above to
        // absorb that overflow — the box of the next Pocket/Hat/heading would
        // bleed into whatever sat above it (the previous line's text, or the
        // ribbon toolbar if it was the first line in the file) while the
        // filler itself just added unused space below. `wrap_to_display`
        // still has to point at the content slot specifically (not the first
        // filler), since cursor/scroll pixel math is keyed off it.
        let slots =
            slot_count_for_paragraph(paragraphs.get(*li), zoom, normal_size_px, line_spacing);
        for _ in 1..slots {
            display_to_wrap.push(None);
        }
        wrap_to_display.push(display_to_wrap.len());
        display_to_wrap.push(Some(wrap_idx));
    }
    (display_to_wrap, wrap_to_display)
}

// Pure display-row and cursor-edge resolution.
/// See `TextEditor::move_cursor_to_row_edge`'s doc comment.
#[derive(Clone, Copy)]
pub(crate) enum RowEdge {
    Start,
    End,
    FirstNonBlank,
}

/// Resolves a display row (see `expand_rows_for_display`) to the nearest
/// real content row at or before it. A display row can be a blank spacer
/// slot reserved by an earlier oversized card-style/heading row (which has
/// no content of its own to land on), so this walks backward to the
/// nearest one that does — shared by `line_col_from_mouse_position` (a
/// click landing on a spacer slot) and H/M/L's row resolution (bug report:
/// H/M/L landed on the wrong row whenever a card-style row sat above the
/// viewport, from assuming every row was the same pixel height instead of
/// going through this same display-row translation).
pub(crate) fn nearest_wrap_row_for_display_row(
    display_to_wrap: &[Option<usize>],
    display_row: usize,
) -> usize {
    /*
     * Forwards, not backwards. `expand_rows_for_display` reserves a row's
     * blank slots *before* its content (its own doc comment explains why:
     * `row_div` bottom-aligns, so a too-tall line overflows upward), which
     * means the blank slots at index `i` are the space the *next* content
     * row's glyphs are painted into — they belong to the row after them, not
     * the one before.
     *
     * This used to scan backwards, a leftover from the earlier
     * fillers-after layout. That mapped a click in an oversized line's
     * overflow area to the line above it. It went mostly unnoticed while
     * only card-style and heading lines reserved spacers at all; once every
     * line is subdivided (`ROW_SUBDIVISIONS`) most slots in the document are
     * blank ones, and scanning the wrong way would put nearly every click a
     * line off.
     *
     * The backward scan survives only as the fallback for a slot past the
     * last content row (nothing follows it to claim it), so this is still
     * total for any in-range index.
     */
    display_to_wrap
        .iter()
        .skip(display_row)
        .find_map(|w| *w)
        .or_else(|| (0..=display_row).rev().find_map(|i| display_to_wrap[i]))
        .unwrap_or(0)
}

/// Pure resolution of `RowEdge` into a char column within `line_chars`,
/// given the current visual row's `[row_start, row_end)` char range —
/// factored out of `TextEditor::move_cursor_to_row_edge` so this (the part
/// with an actual branch worth testing) doesn't need a live GPUI context.
pub(crate) fn row_edge_target_col(
    edge: RowEdge,
    line_chars: &[char],
    row_start: usize,
    row_end: usize,
) -> usize {
    match edge {
        RowEdge::Start => row_start,
        RowEdge::End => row_end,
        RowEdge::FirstNonBlank => line_chars
            .get(row_start..row_end.min(line_chars.len()))
            .unwrap_or(&[])
            .iter()
            .position(|c| !c.is_whitespace())
            .map(|i| row_start + i)
            .unwrap_or(row_end), // an all-whitespace row: land at its end, matching real vim's `^` on a blank line
    }
}

// Pure horizontal and text-span coordinate transforms.
/// Converts an x pixel offset (relative to the start of the text, i.e. after
/// subtracting the container's left padding) into a character column *within
/// one visual row*, rounding to the nearest character boundary and clamping
/// negative input to 0.
///
/// Walks the row character by character rather than dividing by a single
/// width: a row can mix font sizes (a Cite run inside a body line, a Shrunk
/// span, a heading), so no one width describes it. Dividing by a uniform
/// estimate put the cursor increasingly far from the pointer the further into
/// a differently-sized line the user clicked.
pub(crate) fn column_for_x_in_row(
    x: f32,
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    row_start: usize,
    row_end: usize,
    normal_size_px: f32,
    zoom: f32,
) -> usize {
    if x <= 0.0 || row_end <= row_start {
        return 0;
    }
    let mut left = 0.0f32;
    for char_idx in row_start..row_end {
        let width = effective_char_size_px(para, spans, char_idx, normal_size_px, zoom)
            * effective_char_advance_ratio(para, spans, char_idx);
        if x < left + width {
            // Past this character's midpoint means the nearer boundary is the
            // one after it — same round-to-nearest feel as clicking in Word.
            let col = char_idx - row_start;
            return if x - left > width / 2.0 { col + 1 } else { col };
        }
        left += width;
    }
    row_end - row_start
}

/// Inverse of `column_for_x_in_row`: the pixel X position `col_in_row`
/// characters into a row, summing each character's own effective size.
///
/// Bug report: pressing `k`/`j` (`visual_row_step`) to move between two
/// rows of different font size (e.g. off the end of an 11pt line onto a
/// larger-sized one above, or vice versa) landed the cursor far off to one
/// side — it was carrying the raw character *index* across rows, but two
/// rows at different sizes don't share one width-per-character, so the
/// same index sits at very different on-screen X positions on each. This
/// is the piece that lets `visual_row_step` convert the current row's
/// column to a real pixel X *before* re-resolving it against the target
/// row's own sizes via `column_for_x_in_row` — only pixel position is
/// actually preserved by real "move up/down" behavior.
pub(crate) fn x_for_col_in_row(
    col_in_row: usize,
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    row_start: usize,
    row_end: usize,
    normal_size_px: f32,
    zoom: f32,
) -> f32 {
    let end = row_start + col_in_row.min(row_end - row_start);
    let mut x = 0.0f32;
    for char_idx in row_start..end {
        x += effective_char_size_px(para, spans, char_idx, normal_size_px, zoom)
            * effective_char_advance_ratio(para, spans, char_idx);
    }
    x
}

pub(crate) fn selection_span_for_line(
    line: &str,
    line_byte_start: usize,
    sel_start: usize,
    sel_end: usize,
) -> Option<(usize, usize)> {
    /*
     * Maps a selection's document-wide byte range onto the char-column range
     * of a single line, or None if the selection doesn't touch this line at
     * all (including the boundary case where the selection ends exactly at
     * this line's first byte, or starts exactly at its last byte — those
     * describe a selection that stops at the newline, not one that includes
     * this line's visible characters). `sel_start`/`sel_end` must already be
     * normalized so `sel_start <= sel_end`.
     */
    if sel_start == sel_end {
        return None;
    } // nothing selected
    let line_byte_end = line_byte_start + line.len();
    if sel_end <= line_byte_start || sel_start >= line_byte_end {
        return None;
    }

    // Clamp each selection edge into this line's byte range, then convert
    // that relative byte offset into a char column (not byte column).
    let to_col = |byte: usize| -> usize {
        let rel = byte.saturating_sub(line_byte_start).min(line.len());
        line[..rel].chars().count()
    };
    let start_col = to_col(sel_start.max(line_byte_start));
    let end_col = to_col(sel_end.min(line_byte_end));
    if start_col == end_col {
        return None;
    } // e.g. an empty line fully inside the selection
    Some((start_col, end_col))
}

/// Rebases a row-relative cursor column into one formatting run's own
/// [0, run_len] coordinate space, or `None` if the cursor isn't on this run.
///
/// Runs are half-open and contiguous (`run_start..run_end`), so a cursor
/// sitting exactly on the boundary between two runs must be claimed by
/// exactly one of them — otherwise both draw a cursor segment and the caret
/// appears twice, straddling the character at the boundary. It belongs to
/// the *later* run (the caret sits before the character being typed into) —
/// except when the boundary is the true end of the row (`run_end == row_len`
/// *and* `c == row_len`), which the last run must still claim to produce the
/// existing "cursor past end of line" segment. Checking `run_end == row_len`
/// (not just `c == row_len`) matters: without it, a cursor sitting at the
/// row's end column would wrongly also match every *earlier* run, since
/// `c == row_len` alone says nothing about which run actually reaches that
/// column.
pub(crate) fn sub_cursor_for_run(
    cursor_col: Option<usize>,
    run_start: usize,
    run_end: usize,
    row_len: usize,
) -> Option<usize> {
    cursor_col
        .filter(|&c| c >= run_start && (c < run_end || (run_end == row_len && c == row_len)))
        .map(|c| c - run_start)
}
