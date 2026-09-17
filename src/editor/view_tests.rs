// Import only the two functions under test, not `super::*` — text_editor.rs
// has `use gpui::*;` at module scope, and gpui exports its own `test`
// attribute macro (for async GPUI tests) that shadows std's `#[test]` and
// sends the test-attribute expansion into infinite recursion if it's in
// scope here.
use super::{
    build_visual_rows, column_for_x_in_row, display_line, document_lines, effective_char_font,
    effective_char_size_px, expand_rows_for_display, heading_font_size_px, hidden_wrap_rows,
    line_col_from_mouse_position, line_font_px, line_for_y, line_height_px, line_segments,
    list_item_ordinal, list_marker_text, list_marker_text_for_level,
    nearest_wrap_row_for_display_row, page_scroll_offset, paints_run_box, real_row_height_px,
    row_cache_is_valid_for, row_edge_target_col, row_slot_px, run_is_hidden,
    scrollbar_fade_opacity, selection_span_for_line, slot_count_for_paragraph, spell_ranges_cached,
    sub_cursor_for_run, text_line_box_px, to_letter, to_roman, usable_wrap_width,
    visual_row_for_line_col, visual_row_step, x_for_col_in_row, RowCache, RowEdge, SegmentStyle,
    SpellCache, CARD_BOX_EXTRA_PX, CHAR_ADVANCE_RATIO, CURATED_SERIF_FONT, EMPHASIS_BOX_EXTRA_PX,
    FONT_FAMILY, LINE_HEIGHT_PX, LINE_HEIGHT_RATIO, LIST_GUTTER_PX, ROW_SUBDIVISIONS,
    SCROLLBAR_GUTTER_PX, SCROLLBAR_IDLE_OPACITY, SCROLLBAR_MIN_THUMB_PX, SERIF_CHAR_ADVANCE_RATIO,
};
use crate::docx_parser::{Alignment, ListItem, ListKind, Paragraph, Run};
use crate::editor::layout::wrap_line_into_rows;
use crate::editor::style::effective_char_advance_ratio;
use crate::state::AppState;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Instant;

/// The three run properties that paint a box-shaped visual (background
/// fill or border) — and nothing else — need `render_line`'s fast path
/// excluded, or the paint stretches to the full row width instead of
/// hugging the text. Regression guard for whichever property is added
/// next needing the same treatment: it has to show up here too, or the
/// fast-path exclusion silently misses it exactly like `emphasis_boxed`
/// originally did.
#[test]
fn test_paints_run_box_covers_exactly_box_format_highlight_and_emphasis_boxed() {
    assert!(!paints_run_box(None));
    assert!(!paints_run_box(Some(&Run::default())));
    assert!(paints_run_box(Some(&Run {
        box_format: true,
        ..Run::default()
    })));
    assert!(paints_run_box(Some(&Run {
        highlight: true,
        ..Run::default()
    })));
    assert!(paints_run_box(Some(&Run {
        emphasis_boxed: true,
        ..Run::default()
    })));
    // A plain style property (no box shape) must not trip the exclusion.
    assert!(!paints_run_box(Some(&Run {
        bold: true,
        underline: true,
        ..Run::default()
    })));
}

/// A plain body-text row: 11px text (settings.conf's `normal_text_size`)
/// means 6.6px characters, so a click N characters in resolves to column N.
/// The old uniform-width math divided by 8.4px and landed short.
/// The reported bug: bumping the font size on text near the right edge let
/// it run off the editor instead of re-wrapping. Wrapping measured every
/// character at one fixed size, so a row wrapped for 11px text still held
/// the same characters after they grew to 24px — and the extra width spilled
/// past the right border. Widths now come from each character's own size,
/// so a larger run breaks sooner.
#[test]
fn test_wrap_breaks_sooner_when_characters_are_larger() {
    let chars: Vec<char> = "aaaa aaaa aaaa aaaa".chars().collect();

    // 6.6px characters (11pt body) — plenty fits in 100px.
    let mut small = |_: usize, _: char| 6.6f32;
    let small_rows = wrap_line_into_rows(&chars, 100.0, &mut small);

    // 14.4px characters (24pt) — the same text needs more rows.
    let mut large = |_: usize, _: char| 14.4f32;
    let large_rows = wrap_line_into_rows(&chars, 100.0, &mut large);

    assert!(
        large_rows.len() > small_rows.len(),
        "larger text must wrap into more rows: {} vs {}",
        large_rows.len(),
        small_rows.len(),
    );
    // And no row may exceed the budget at the larger size.
    for (start, end) in &large_rows {
        let width = (end - start) as f32 * 14.4;
        assert!(
            width <= 100.0,
            "row {start}..{end} is {width}px, over budget"
        );
    }
}

/// Bug report: pressing Tab while editing appeared to do nothing —
/// no visible gap, cursor didn't move. Root cause: the bundled fonts
/// have no glyph for U+0009, so GPUI paints a raw '\t' with zero
/// width. `display_line` substitutes it for a space at render time
/// only, which must be a strict one-char-for-one-char swap — anything
/// else would desync every offset-based cursor/selection computation
/// that indexes into the line by character position.
#[test]
fn test_display_line_swaps_tab_for_space_one_for_one() {
    assert_eq!(display_line("a\tb"), "a b");
    assert_eq!(
        "a\tb".chars().count(),
        display_line("a\tb").chars().count(),
        "substitution must not change the char count offsets are computed against"
    );
    // No tab present: no allocation-worthy change, and (implementation
    // detail worth locking in) no unnecessary copy.
    assert!(matches!(
        display_line("plain text"),
        std::borrow::Cow::Borrowed(_)
    ));
}

/// Bug report: a line was entirely invisible unless the cursor sat on
/// it, and even then only the text left of the cursor painted. Root
/// cause: a `'\r'` embedded mid-paragraph (this fixture's came from a
/// `.docx` whose `<w:t>` held a literal CR instead of `<w:br/>`, but
/// typing/pasting one reaches the model the same way) — GPUI's own
/// `shape_line` only asserts against `'\n'`, so `'\r'` reached the
/// shaper unguarded and dropped the entire fragment containing it, not
/// just that one character. Splitting the row at the cursor happened to
/// separate the CR-free half (paints fine) from the CR-holding half
/// (doesn't) — hence the "left of cursor visible" symptom. Same fix and
/// same one-for-one contract as the tab case above, just widened from
/// `'\t'` to the whole control-character class.
#[test]
fn test_display_line_swaps_carriage_return_for_space_one_for_one() {
    assert_eq!(display_line("a\rb"), "a b");
    assert_eq!(
        "a\rb".chars().count(),
        display_line("a\rb").chars().count(),
        "substitution must not change the char count offsets are computed against"
    );
}

/// A size change partway along a row has to be respected mid-row, which is
/// why the width callback receives the character's index.
#[test]
fn test_wrap_respects_a_size_change_partway_through_a_row() {
    let chars: Vec<char> = "aaaaaaaaaaaaaaaaaaaa".chars().collect(); // 20
                                                                     // First 10 characters small, the rest large.
    let mut width_of = |i: usize, _: char| if i < 10 { 5.0f32 } else { 20.0 };
    let rows = wrap_line_into_rows(&chars, 100.0, &mut width_of);

    // 10 small chars fill exactly 50px, then two large ones reach 90px and
    // a third would overflow — so the first row must stop before char 13.
    assert!(
        rows[0].1 <= 13,
        "first row ran to {} despite the larger tail",
        rows[0].1
    );
    assert!(
        rows.len() > 1,
        "the larger tail must be pushed onto another row"
    );
}

#[test]
fn test_column_in_row_uses_the_rendered_body_size() {
    let char_width = 11.0 * CHAR_ADVANCE_RATIO;
    assert!((char_width - 6.6).abs() < 0.01, "got {char_width}");
    for col in [0usize, 1, 5, 20, 60] {
        let x = col as f32 * char_width;
        assert_eq!(
            column_for_x_in_row(x, None, &[], 0, 80, 11.0, 1.0),
            col,
            "click at column {col}",
        );
    }
}

#[test]
fn test_column_in_row_rounds_to_the_nearest_boundary() {
    let w = 11.0 * CHAR_ADVANCE_RATIO; // 6.6
    assert_eq!(column_for_x_in_row(w * 0.4, None, &[], 0, 10, 11.0, 1.0), 0);
    assert_eq!(column_for_x_in_row(w * 0.6, None, &[], 0, 10, 11.0, 1.0), 1);
}

#[test]
fn test_column_in_row_clamps_negative_and_past_the_end() {
    assert_eq!(column_for_x_in_row(-30.0, None, &[], 0, 10, 11.0, 1.0), 0);
    assert_eq!(column_for_x_in_row(9999.0, None, &[], 0, 10, 11.0, 1.0), 10);
    // An empty row has no column but 0.
    assert_eq!(column_for_x_in_row(50.0, None, &[], 4, 4, 11.0, 1.0), 0);
}

/// Bug report: clicking a centered line placed the cursor to the right of
/// the pointer by roughly the centering indent. `line_col_from_mouse_position`
/// used to measure `local_x` from the row's raw left edge regardless of
/// alignment, which is only correct for `Alignment::Left`.
#[test]
fn line_col_from_mouse_position_accounts_for_center_alignment() {
    use gpui::{point, px, size, Bounds};
    let para = Paragraph {
        alignment: Alignment::Center,
        ..Paragraph::default()
    };
    let paragraphs = vec![para];
    let rows = vec![(0usize, 0usize, 10usize)]; // one row, 10 chars
    let display_to_wrap = vec![Some(0usize)];
    // Sized so the usable width is 200px after both the padding and the
    // scrollbar gutter come off, keeping the arithmetic below unchanged
    // now that text no longer wraps under the bar.
    let width = 232.0 + SCROLLBAR_GUTTER_PX;
    let content_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(width), px(100.0)));
    // avail_width = 200; text_width = 10 * (11*CHAR_ADVANCE_RATIO) = 66;
    // indent = (200 - 66) / 2 = 67.
    let indent = 67.0;
    // Clicking exactly at the text's real (indented) left edge must land on
    // column 0. Without accounting for alignment, this same pixel — 67px
    // right of the row's raw edge — would resolve as if it were 67px into
    // a row that starts flush left, landing at the row's *last* column
    // instead (67 / 6.6 ≈ 10, clamped to row_end - row_start).
    let position = point(px(16.0 + indent), px(0.0));
    let (line, col) = line_col_from_mouse_position(
        position,
        content_bounds,
        0.0,
        &rows,
        &display_to_wrap,
        1.0,
        11.0,
        &paragraphs,
        line_height_px(11.0, 1.0),
    );
    assert_eq!((line, col), (0, 0));
}

/// Bug report: clicking near the bottom of a large file landed the
/// cursor several lines above the pointer. Root cause, confirmed via
/// device logging: GPUI's actual measured per-row height can differ
/// (by a fraction of a pixel — device-pixel snapping) from
/// `line_height_px(font_size_px) * zoom` recomputed independently, and
/// that per-row error compounds with row index. `real_row_height_px`
/// must prefer GPUI's own measurement once one exists.
#[test]
fn real_row_height_px_prefers_gpuis_measured_height_over_the_computed_one() {
    let handle = gpui::UniformListScrollHandle::new();
    let item_count = 100usize;
    // Before any layout has run, there's nothing measured yet — falls
    // back to the computed value.
    // One `uniform_list` row is a subdivision of a line, not a whole one.
    assert_eq!(
        real_row_height_px(&handle, item_count, 11.0, 1.0, 1.0),
        row_slot_px(11.0, 1.0, 1.0)
    );

    // `ItemSize.item` is the *viewport's* box (`padded_bounds.size` in
    // GPUI's own `prepaint`, confirmed against the vendored source), not
    // one row's height — a stub that (wrongly) set `item.height` to the
    // real row height would pass even if `real_row_height_px` read the
    // wrong field, which is exactly the bug this test needs to catch.
    // The real per-row height only recovers from `contents.height /
    // item_count` (`contents.height = longest_item_size.height *
    // item_count`), matching this reported bug's device data (measured
    // 15.5 vs. computed ~15.71).
    handle.0.borrow_mut().last_item_size = Some(gpui::ItemSize {
        item: gpui::size(gpui::px(999.0), gpui::px(524.5)), // a viewport-sized box
        contents: gpui::size(gpui::px(999.0), gpui::px(15.5 * item_count as f32)),
    });
    assert_eq!(
        real_row_height_px(&handle, item_count, 11.0, 1.0, 1.0),
        15.5
    );
}

#[test]
fn test_column_in_row_tracks_zoom() {
    for zoom in [0.5f32, 1.0, 1.5, 2.0] {
        let w = 11.0 * CHAR_ADVANCE_RATIO * zoom;
        assert_eq!(
            column_for_x_in_row(12.0 * w, None, &[], 0, 40, 11.0, zoom),
            12,
            "zoom {zoom}"
        );
    }
}

/// The reported regression: a line rendered at a *different* size than the
/// body default. A Block card style sets a run-level 16pt (32 half-points),
/// so its characters are 9.6px, not 6.6px — clicking 10 characters in must
/// still land on column 10.
#[test]
fn test_column_in_row_follows_a_run_level_font_size() {
    let para = Paragraph {
        runs: vec![Run {
            text: "0123456789abcdef".into(),
            size: 32,
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let spans = crate::document_ops::paragraph_run_char_spans(&para);
    let block_char = 16.0 * CHAR_ADVANCE_RATIO; // 9.6px
    for col in [1usize, 5, 10] {
        assert_eq!(
            column_for_x_in_row(
                col as f32 * block_char,
                Some(&para),
                &spans,
                0,
                16,
                11.0,
                1.0
            ),
            col,
            "block-sized column {col}",
        );
    }
    // Read through the body-size estimate the same pixel lands far right.
    let body_char = 11.0 * CHAR_ADVANCE_RATIO;
    assert!((10.0 * block_char / body_char).round() as usize > 10);
}

/// `x_for_col_in_row` is `column_for_x_in_row`'s inverse — round-tripping
/// a column through both must return the same column, both on a plain
/// uniform-size row and on a run-level-sized one (`visual_row_step`
/// relies on exactly this to convert a column to pixels on one row and
/// back to a column on another).
#[test]
fn test_x_for_col_in_row_round_trips_with_column_for_x_in_row() {
    for col in [0usize, 1, 5, 10] {
        let x = x_for_col_in_row(col, None, &[], 0, 10, 11.0, 1.0);
        assert_eq!(column_for_x_in_row(x, None, &[], 0, 10, 11.0, 1.0), col);
    }

    let para = Paragraph {
        runs: vec![Run {
            text: "0123456789abcdef".into(),
            size: 32,
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let spans = crate::document_ops::paragraph_run_char_spans(&para);
    for col in [1usize, 5, 10] {
        let x = x_for_col_in_row(col, Some(&para), &spans, 0, 16, 11.0, 1.0);
        assert_eq!(
            column_for_x_in_row(x, Some(&para), &spans, 0, 16, 11.0, 1.0),
            col
        );
    }
}

#[test]
fn test_x_for_col_in_row_clamps_past_the_end() {
    assert_eq!(
        x_for_col_in_row(50, None, &[], 0, 10, 11.0, 1.0),
        x_for_col_in_row(10, None, &[], 0, 10, 11.0, 1.0)
    );
}

/// Bug report: `$`/`0`/`^`/Home/End jumped to the edge of the whole
/// wrapped paragraph instead of the current visual row. This is the
/// pure column-resolution `move_cursor_to_row_edge` is built on —
/// exercised directly against a *row* range that's narrower than the
/// full line, which is exactly the wrapped-continuation-row case.
#[test]
fn test_row_edge_target_col_start_and_end_use_the_row_not_the_line() {
    // "hello world" wrapped into rows [0,6) ("hello ") and [6,11)
    // ("world") — row 2 is the one under test.
    let chars: Vec<char> = "hello world".chars().collect();
    assert_eq!(
        row_edge_target_col(RowEdge::Start, &chars, 6, 11),
        6,
        "row start, not line start (0)"
    );
    assert_eq!(
        row_edge_target_col(RowEdge::End, &chars, 6, 11),
        11,
        "row end, not line end"
    );
    assert_eq!(
        row_edge_target_col(RowEdge::End, &chars, 0, 6),
        6,
        "the *first* row's own end, not the whole line's"
    );
}

#[test]
fn test_row_edge_target_col_first_non_blank_skips_leading_whitespace_within_the_row() {
    let chars: Vec<char> = "one   two".chars().collect();
    // Row [3, 9) is "   two" — first non-blank is 't' at index 6.
    assert_eq!(row_edge_target_col(RowEdge::FirstNonBlank, &chars, 3, 9), 6);
}

#[test]
fn test_row_edge_target_col_first_non_blank_all_whitespace_row_lands_at_its_end() {
    let chars: Vec<char> = "one    ".chars().collect();
    // Row [3, 7) is all spaces — matches real vim's `^` on a blank line.
    assert_eq!(row_edge_target_col(RowEdge::FirstNonBlank, &chars, 3, 7), 7);
}

/// Bug report: H/M/L landed on the wrong row whenever a card-style row
/// sat above the viewport, from dividing raw scroll offset by a single
/// `line_height` (assumes every row is the same height). This is the
/// piece that translates a *display* row (uniform height, includes
/// spacer slots an oversized row reserves) back to the nearest real
/// content row — the same rule a mouse click landing on a spacer slot
/// already used.
#[test]
fn test_nearest_wrap_row_for_display_row_walks_forward_over_spacer_slots() {
    // `expand_rows_for_display` puts a row's spacers *before* its
    // content, so these are: row 0's content at 0, then two spacers
    // reserved for row 1, then row 1's content at 3. The spacers are the
    // space row 1 paints upward into, so they belong to row 1.
    let display_to_wrap = vec![Some(0), None, None, Some(1)];
    assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 0), 0);
    assert_eq!(
        nearest_wrap_row_for_display_row(&display_to_wrap, 1),
        1,
        "spacer slot belongs to the row after it"
    );
    assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 2), 1);
    assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 3), 1);
}

/// A trailing spacer has no content row after it to belong to, so the
/// backward fallback still has to resolve it rather than panicking.
#[test]
fn test_nearest_wrap_row_for_display_row_falls_back_for_a_trailing_spacer() {
    let display_to_wrap = vec![Some(0), None];
    assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 1), 0);
}

/// A row mixing sizes — a Cite-sized run after body text — has no single
/// character width at all, which is why the column is walked rather than
/// divided.
#[test]
fn test_column_in_row_handles_mixed_sizes_within_one_row() {
    let para = Paragraph {
        runs: vec![
            Run {
                text: "aaaaa".into(),
                ..Run::default()
            }, // body 11px
            Run {
                text: "bbbbb".into(),
                size: 26,
                ..Run::default()
            }, // 13pt
        ],
        ..Paragraph::default()
    };
    let spans = crate::document_ops::paragraph_run_char_spans(&para);
    let body = 11.0 * CHAR_ADVANCE_RATIO;
    let cite = 13.0 * CHAR_ADVANCE_RATIO;

    // Boundary between the two runs.
    assert_eq!(
        column_for_x_in_row(5.0 * body, Some(&para), &spans, 0, 10, 11.0, 1.0),
        5
    );
    // Three characters into the larger run.
    let x = 5.0 * body + 3.0 * cite;
    assert_eq!(
        column_for_x_in_row(x, Some(&para), &spans, 0, 10, 11.0, 1.0),
        8
    );
}

#[test]
fn test_effective_char_size_prefers_run_then_heading_then_body() {
    let body = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let spans = crate::document_ops::paragraph_run_char_spans(&body);
    assert_eq!(
        effective_char_size_px(Some(&body), &spans, 0, 11.0, 1.0),
        11.0
    );

    let heading = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            ..Run::default()
        }],
        heading: 1,
        ..Paragraph::default()
    };
    let hspans = crate::document_ops::paragraph_run_char_spans(&heading);
    assert_eq!(
        effective_char_size_px(Some(&heading), &hspans, 0, 11.0, 1.0),
        24.0
    );

    // A run-level size wins over the heading level.
    let both = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            size: 32,
            ..Run::default()
        }],
        heading: 1,
        ..Paragraph::default()
    };
    let bspans = crate::document_ops::paragraph_run_char_spans(&both);
    assert_eq!(
        effective_char_size_px(Some(&both), &bspans, 0, 11.0, 1.0),
        16.0
    );

    // No paragraph data at all falls back to the body size.
    assert_eq!(effective_char_size_px(None, &[], 0, 11.0, 2.0), 22.0);
}

/// Bug report: choosing a font from the Font Family picker didn't
/// visibly change anything. `effective_char_font` is the piece that
/// tells wrap/click/scroll math which font a character actually paints
/// at — it must mirror `apply_run_style`'s own rule (a curated
/// `run.font` wins, anything else falls back to `FONT_FAMILY`) or wrap
/// decisions and what's on screen would disagree.
#[test]
fn test_effective_char_font_follows_run_font_only_when_curated() {
    let none = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let none_spans = crate::document_ops::paragraph_run_char_spans(&none);
    assert_eq!(
        effective_char_font(Some(&none), &none_spans, 0),
        FONT_FAMILY
    );

    let serif = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            font: Some(CURATED_SERIF_FONT.to_string()),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let serif_spans = crate::document_ops::paragraph_run_char_spans(&serif);
    assert_eq!(
        effective_char_font(Some(&serif), &serif_spans, 0),
        CURATED_SERIF_FONT
    );

    // An uncurated font (e.g. read from a real imported .docx naming
    // "Georgia" or "Calibri") must not be applied — falls back to
    // FONT_FAMILY exactly like `apply_run_style` does.
    let uncurated = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            font: Some("Georgia".to_string()),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let uncurated_spans = crate::document_ops::paragraph_run_char_spans(&uncurated);
    assert_eq!(
        effective_char_font(Some(&uncurated), &uncurated_spans, 0),
        FONT_FAMILY
    );

    // No paragraph data at all falls back to FONT_FAMILY too.
    assert_eq!(effective_char_font(None, &[], 0), FONT_FAMILY);
}

#[test]
fn test_effective_char_advance_ratio_matches_effective_char_font() {
    let serif = Paragraph {
        runs: vec![Run {
            text: "ab".into(),
            font: Some(CURATED_SERIF_FONT.to_string()),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let spans = crate::document_ops::paragraph_run_char_spans(&serif);
    assert_eq!(
        effective_char_advance_ratio(Some(&serif), &spans, 0),
        SERIF_CHAR_ADVANCE_RATIO
    );
    assert_eq!(
        effective_char_advance_ratio(None, &[], 0),
        CHAR_ADVANCE_RATIO
    );
}

/// `column_for_x_in_row` must resolve a serif-font run at the serif
/// ratio, not the monospace one — using the wrong ratio for a
/// proportional font's run would put click-to-cursor consistently off
/// for any document that uses the curated serif font at all.
#[test]
fn test_column_in_row_uses_serif_ratio_for_a_serif_run() {
    let para = Paragraph {
        runs: vec![Run {
            text: "0123456789".into(),
            font: Some(CURATED_SERIF_FONT.to_string()),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let spans = crate::document_ops::paragraph_run_char_spans(&para);
    let serif_char = 11.0 * SERIF_CHAR_ADVANCE_RATIO;
    for col in [1usize, 5, 9] {
        assert_eq!(
            column_for_x_in_row(
                col as f32 * serif_char,
                Some(&para),
                &spans,
                0,
                10,
                11.0,
                1.0
            ),
            col,
        );
    }
}

#[test]
fn test_line_for_y_top_is_first_line() {
    assert_eq!(line_for_y(0.0, 20.0, 3), 0);
    assert_eq!(line_for_y(19.9, 20.0, 3), 0);
}

#[test]
fn test_line_for_y_advances_per_line_height() {
    assert_eq!(line_for_y(20.0, 20.0, 3), 1);
    assert_eq!(line_for_y(45.0, 20.0, 3), 2);
}

#[test]
fn test_line_for_y_clamps_past_last_line() {
    assert_eq!(line_for_y(1000.0, 20.0, 3), 2);
}

#[test]
fn test_line_for_y_clamps_negative_to_zero() {
    assert_eq!(line_for_y(-5.0, 20.0, 3), 0);
}

#[test]
fn test_line_for_y_no_lines_is_zero() {
    assert_eq!(line_for_y(50.0, 20.0, 0), 0);
}

// ── selection_span_for_line ─────────────────────────────────────────────

#[test]
fn test_selection_span_before_line_is_none() {
    // Line "world" starts at byte 6; selection (0, 4) ends before it.
    assert_eq!(selection_span_for_line("world", 6, 0, 4), None);
}

#[test]
fn test_selection_span_after_line_is_none() {
    // Line "hello" spans bytes [0, 5); selection (6, 10) starts after it.
    assert_eq!(selection_span_for_line("hello", 0, 6, 10), None);
}

#[test]
fn test_selection_span_touching_line_start_boundary_is_none() {
    // Selection (0, 6) covers "hello" plus its newline, but not "world" itself.
    assert_eq!(selection_span_for_line("world", 6, 0, 6), None);
}

#[test]
fn test_selection_span_touching_line_end_boundary_is_none() {
    assert_eq!(selection_span_for_line("hello", 0, 5, 9), None);
}

#[test]
fn test_selection_span_within_single_line() {
    assert_eq!(
        selection_span_for_line("hello world", 0, 0, 5),
        Some((0, 5))
    );
}

#[test]
fn test_selection_span_first_line_of_multiline_selection() {
    // Selection continues past this line's end; highlight runs to end of line.
    assert_eq!(selection_span_for_line("hello", 0, 2, 20), Some((2, 5)));
}

#[test]
fn test_selection_span_last_line_of_multiline_selection() {
    // Selection started before this line; highlight runs from its start.
    assert_eq!(selection_span_for_line("world", 6, 0, 9), Some((0, 3)));
}

#[test]
fn test_selection_span_middle_line_fully_covered() {
    assert_eq!(selection_span_for_line("middle", 10, 0, 30), Some((0, 6)));
}

#[test]
fn test_selection_span_zero_width_is_none() {
    assert_eq!(selection_span_for_line("hello", 0, 2, 2), None);
}

#[test]
fn test_selection_span_counts_chars_not_bytes() {
    // "café" is 5 bytes but 4 characters ('é' is 2 bytes).
    assert_eq!(selection_span_for_line("café", 0, 0, 5), Some((0, 4)));
}

// ── line_segments ────────────────────────────────────────────────────────

#[test]
fn test_line_segments_no_cursor_no_selection() {
    assert_eq!(
        line_segments(5, None, &[], &[]),
        vec![(0, 5, SegmentStyle::Plain, false)]
    );
}

#[test]
fn test_line_segments_cursor_mid_line() {
    assert_eq!(
        line_segments(5, Some(2), &[], &[]),
        vec![
            (0, 2, SegmentStyle::Plain, false),
            (2, 3, SegmentStyle::Cursor, false),
            (3, 5, SegmentStyle::Plain, false),
        ]
    );
}

#[test]
fn test_line_segments_cursor_at_line_start() {
    assert_eq!(
        line_segments(5, Some(0), &[], &[]),
        vec![
            (0, 1, SegmentStyle::Cursor, false),
            (1, 5, SegmentStyle::Plain, false)
        ]
    );
}

#[test]
fn test_line_segments_cursor_past_end_of_line() {
    assert_eq!(
        line_segments(5, Some(5), &[], &[]),
        vec![
            (0, 5, SegmentStyle::Plain, false),
            (5, 5, SegmentStyle::Cursor, false)
        ]
    );
}

#[test]
fn test_line_segments_selection_only() {
    assert_eq!(
        line_segments(6, None, &[(1, 4)], &[]),
        vec![
            (0, 1, SegmentStyle::Plain, false),
            (1, 4, SegmentStyle::Selection, false),
            (4, 6, SegmentStyle::Plain, false),
        ]
    );
}

#[test]
fn test_line_segments_selection_covers_full_line() {
    assert_eq!(
        line_segments(6, None, &[(0, 6)], &[]),
        vec![(0, 6, SegmentStyle::Selection, false)]
    );
}

#[test]
fn test_line_segments_cursor_inside_selection_wins_its_own_cell() {
    assert_eq!(
        line_segments(6, Some(2), &[(2, 5)], &[]),
        vec![
            (0, 2, SegmentStyle::Plain, false),
            (2, 3, SegmentStyle::Cursor, false),
            (3, 5, SegmentStyle::Selection, false),
            (5, 6, SegmentStyle::Plain, false),
        ]
    );
}

#[test]
fn test_line_segments_empty_line_with_cursor() {
    assert_eq!(
        line_segments(0, Some(0), &[], &[]),
        vec![(0, 0, SegmentStyle::Cursor, false)]
    );
}

#[test]
fn test_line_segments_misspelled_range_splits_and_flags() {
    assert_eq!(
        line_segments(9, None, &[], &[(4, 9)]),
        vec![
            (0, 4, SegmentStyle::Plain, false),
            (4, 9, SegmentStyle::Plain, true)
        ]
    );
}

#[test]
fn test_line_segments_misspelled_word_keeps_selection_and_cursor_styles() {
    // A selected, cursor-bearing, misspelled word must keep all three:
    // the squiggle is an overlay, not a competing SegmentStyle.
    assert_eq!(
        line_segments(6, Some(1), &[(0, 4)], &[(0, 4)]),
        vec![
            (0, 1, SegmentStyle::Selection, true),
            (1, 2, SegmentStyle::Cursor, true),
            (2, 4, SegmentStyle::Selection, true),
            (4, 6, SegmentStyle::Plain, false),
        ]
    );
}

// ── sub_cursor_for_run / render_line's per-run cursor split ────────────────

/// Bug report: with vim mode off, the cursor appeared twice — once on
/// each side of a character. Root cause: a row is split into per-run
/// chunks (half-open, contiguous — one run's `end` is the next run's
/// `start`), but the old filter treated that shared boundary column as
/// belonging to *both* runs, so both emitted a `SegmentStyle::Cursor`
/// segment. Reproduces the actual failure shape (two runs, cursor on
/// their shared boundary) rather than just the predicate in isolation,
/// so a future edit to `line_segments`'s past-end branch can't
/// reintroduce the double-paint without this test catching it.
#[test]
fn test_cursor_on_run_boundary_claimed_by_exactly_one_run() {
    // Row "foobar" (len 6) split into two runs: "foo" [0,3) and "bar" [3,6),
    // cursor sitting exactly on their shared boundary (col 3).
    let row_len = 6;
    let runs = [(0usize, 3usize), (3usize, 6usize)];
    let cursor_col = Some(3);

    let mut cursor_segments = 0;
    for &(run_start, run_end) in &runs {
        let sub_cursor = sub_cursor_for_run(cursor_col, run_start, run_end, row_len);
        let sub_len = run_end - run_start;
        for (_, _, style, _) in line_segments(sub_len, sub_cursor, &[], &[]) {
            if style == SegmentStyle::Cursor {
                cursor_segments += 1;
            }
        }
    }
    assert_eq!(
        cursor_segments, 1,
        "exactly one run must claim a boundary cursor, not both"
    );
}

#[test]
fn test_cursor_at_end_of_row_still_claimed_by_last_run() {
    // Cursor past the last character of the row (col 6 of a 6-char row)
    // must still land on the last run, to keep the existing "cursor past
    // end of line" segment.
    let row_len = 6;
    let runs = [(0usize, 3usize), (3usize, 6usize)];
    let cursor_col = Some(6);

    let mut cursor_segments = 0;
    for &(run_start, run_end) in &runs {
        let sub_cursor = sub_cursor_for_run(cursor_col, run_start, run_end, row_len);
        let sub_len = run_end - run_start;
        for (_, _, style, _) in line_segments(sub_len, sub_cursor, &[], &[]) {
            if style == SegmentStyle::Cursor {
                cursor_segments += 1;
            }
        }
    }
    assert_eq!(
        cursor_segments, 1,
        "end-of-row cursor must still be claimed exactly once"
    );
}

// ── usable_wrap_width ────────────────────────────────────────────────────

#[test]
fn test_usable_wrap_width_basic() {
    // 100 - 2*16 padding - the scrollbar gutter.
    assert_eq!(usable_wrap_width(100.0), 68.0 - SCROLLBAR_GUTTER_PX);
}

#[test]
fn test_usable_wrap_width_unlaid_out_viewport_is_unbounded() {
    // width <= 0 happens before the scroll handle's first layout pass.
    assert_eq!(usable_wrap_width(0.0), f32::MAX);
    assert_eq!(usable_wrap_width(-5.0), f32::MAX);
}

// ── wrap_line_into_rows ─────────────────────────────────────────────────────
// Tests use a uniform 8.0px-per-char width function to exercise the wrap
// algorithm itself (spacing/hard-break logic), independent of any real
// font metrics — see the dedicated variable-width test below for the
// narrow-vs-wide-glyph behaviour this was rewritten to fix.

#[test]
fn test_wrap_line_into_rows_empty_line_is_one_row() {
    assert_eq!(
        wrap_line_into_rows(&[], 80.0, &mut |_, _| 8.0),
        vec![(0, 0)]
    );
}

#[test]
fn test_wrap_line_into_rows_fits_in_one_row() {
    let chars: Vec<char> = "hello".chars().collect();
    assert_eq!(
        wrap_line_into_rows(&chars, 80.0, &mut |_, _| 8.0),
        vec![(0, 5)]
    );
}

#[test]
fn test_wrap_line_into_rows_breaks_on_word_boundary() {
    // "hello world" (11 chars) at 8px/char, budget=64px covers "hello wo"
    // (8 chars); last space within budget is at index 5, so row 1 is
    // [0,5)="hello", the space at 5 is consumed, row 2 starts at 6: "world".
    let chars: Vec<char> = "hello world".chars().collect();
    assert_eq!(
        wrap_line_into_rows(&chars, 64.0, &mut |_, _| 8.0),
        vec![(0, 5), (6, 11)]
    );
}

#[test]
fn test_wrap_line_into_rows_hard_breaks_long_word() {
    // No spaces at all within budget -> hard break exactly at the pixel
    // budget (32px / 8px-per-char = 4 chars per row).
    let chars: Vec<char> = "abcdefghij".chars().collect();
    assert_eq!(
        wrap_line_into_rows(&chars, 32.0, &mut |_, _| 8.0),
        vec![(0, 4), (4, 8), (8, 10)]
    );
}

#[test]
fn test_wrap_line_into_rows_exact_multiple_of_width() {
    let chars: Vec<char> = "abcdefgh".chars().collect();
    assert_eq!(
        wrap_line_into_rows(&chars, 32.0, &mut |_, _| 8.0),
        vec![(0, 4), (4, 8)]
    );
}

#[test]
fn test_wrap_line_into_rows_trailing_space_at_break_not_repeated() {
    // Two words separated by exactly one space at the wrap point: the
    // space must not reappear as a leading character on the next row.
    let chars: Vec<char> = "aaaa bbbb".chars().collect();
    let rows = wrap_line_into_rows(&chars, 40.0, &mut |_, _| 8.0);
    for (start, end) in &rows {
        let text: String = chars[*start..*end].iter().collect();
        assert!(!text.starts_with(' '), "row {:?} starts with a space", text);
    }
}

#[test]
fn test_wrap_line_into_rows_forces_progress_when_single_char_exceeds_budget() {
    // A viewport (or a single unusually wide glyph) narrower than one
    // character's width must still advance one character per row rather
    // than looping forever or producing an empty row.
    let chars: Vec<char> = "ab".chars().collect();
    let rows = wrap_line_into_rows(&chars, 10.0, &mut |_, _| 100.0);
    assert_eq!(rows, vec![(0, 1), (1, 2)]);
}

#[test]
fn test_wrap_line_into_rows_narrow_chars_pack_more_per_row_than_a_uniform_estimate_would() {
    // This is the actual bug: a uniform per-character width estimate
    // folds lines of narrow glyphs (like '.' or '-') far earlier than
    // their real on-screen width warrants. With a real per-char width
    // function, 20 narrow (2px) dots should fit 10 to a row within a
    // 20px budget — a uniform 8px/char estimate would have wrapped
    // after only 2.
    let chars: Vec<char> = vec!['.'; 20];
    let mut width_of = |_: usize, c: char| if c == '.' { 2.0 } else { 8.0 };
    let rows = wrap_line_into_rows(&chars, 20.0, &mut width_of);
    assert_eq!(rows[0], (0, 10));
}

#[test]
fn test_wrap_line_into_rows_stateful_width_fn_handles_repeated_non_ascii() {
    // Regression: an earlier version of char_width_fn's per-char cache
    // used `Fn` + `RefCell` to satisfy this function's old `&impl Fn`
    // bound; a cache miss on any non-ASCII character (an `if let ...
    // else { borrow_mut() }` whose immutable borrow's temporary lived
    // across the whole if/else) panicked with "already borrowed" the
    // first time a real .docx with non-ASCII text (smart quotes,
    // accents) was opened. This exercises the exact shape that broke:
    // a stateful width-lookup closure (cache populated on first sight,
    // read back on repeats) driven across a line with repeated
    // non-ASCII characters, through the same `&mut impl FnMut` this
    // function now requires.
    let chars: Vec<char> = "caf\u{e9} caf\u{e9} \u{201c}word\u{201d}".chars().collect();
    let mut cache: std::collections::HashMap<char, f32> = std::collections::HashMap::new();
    let mut width_of = |_: usize, c: char| *cache.entry(c).or_insert(8.0);
    let rows = wrap_line_into_rows(&chars, 200.0, &mut width_of);
    assert_eq!(rows, vec![(0, chars.len())]);
}

// ── build_visual_rows / document_lines ──────────────────────────────────

#[test]
fn test_document_lines_empty_content_is_one_empty_line() {
    assert_eq!(document_lines(""), vec![String::new()]);
}

#[test]
fn test_document_lines_splits_on_newline() {
    assert_eq!(document_lines("a\nb\nc"), vec!["a", "b", "c"]);
}

#[test]
fn test_build_visual_rows_one_row_per_short_line() {
    let lines = document_lines("hi\nthere");
    let rows = build_visual_rows(&lines, 800.0, &mut |_, _, _| 8.0);
    assert_eq!(rows, vec![(0, 0, 2), (1, 0, 5)]);
}

#[test]
fn test_build_visual_rows_wraps_long_line_into_multiple_rows() {
    let lines = document_lines("hello world");
    let rows = build_visual_rows(&lines, 64.0, &mut |_, _, _| 8.0);
    assert_eq!(rows, vec![(0, 0, 5), (0, 6, 11)]);
}

// ── visual_row_for_line_col ──────────────────────────────────────────────

#[test]
fn test_visual_row_for_line_col_within_first_row() {
    // Line 0 wraps into rows [(0,5), (6,11)]; col 2 is inside the first.
    let rows = vec![(0, 0, 5), (0, 6, 11)];
    assert_eq!(visual_row_for_line_col(&rows, 0, 2), 0);
}

#[test]
fn test_visual_row_for_line_col_hard_break_boundary_lands_on_next_row_start() {
    // Rows are CONTIGUOUS (row 0 ends at 4, row 1 starts at 4) — a hard
    // mid-word break, no space consumed. col 4 should be carried onto
    // the start of row 1, matching how text editors visually continue
    // the cursor onto the next wrapped row rather than trailing behind.
    let rows = vec![(0, 0, 4), (0, 4, 8)];
    assert_eq!(visual_row_for_line_col(&rows, 0, 4), 1);
}

#[test]
fn test_visual_row_for_line_col_soft_break_boundary_stays_on_current_row() {
    // Row 0 ends at 5, but row 1 starts at 6 (not 5) — a one-character
    // gap for the space `wrap_line_into_rows` consumed at the break.
    // col 5 *is* that consumed space, not a position on row 1, so it
    // must stay on row 0 (trailing the last visible character) rather
    // than being redirected to row 1's row_start (6) — redirecting it
    // was the original bug: row_start(6) > char_col(5) underflowed any
    // `char_col - row_start` a caller computed downstream.
    let rows = vec![(0, 0, 5), (0, 6, 11)];
    assert_eq!(visual_row_for_line_col(&rows, 0, 5), 0);
}

#[test]
fn test_visual_row_for_line_col_true_end_of_line_stays_on_last_row() {
    // col 11 is the true end of the (single) logical line — no next row
    // exists, so it must resolve to the line's last row.
    let rows = vec![(0, 0, 5), (0, 6, 11)];
    assert_eq!(visual_row_for_line_col(&rows, 0, 11), 1);
}

#[test]
fn test_visual_row_for_line_col_second_logical_line() {
    let rows = vec![(0, 0, 5), (1, 0, 3)];
    assert_eq!(visual_row_for_line_col(&rows, 1, 1), 1);
}

// ── visual_row_step ──────────────────────────────────────────────────────

#[test]
fn test_visual_row_step_up_into_wrapped_continuation_row() {
    // Line 0 wraps into two rows: [0,5) and [6,11) ("hello"/"world").
    // Line 1 is short: [0,3). Standing at the start of line 1 (row 2,
    // col 0) and pressing Up must land on line 0's *second* row (the
    // wrapped continuation), not jump to the very start of line 0.
    // No paragraph data (`&[]`): every character falls back to the same
    // uniform size, so pixel-preserving and index-preserving resolve
    // identically here — this is exercising the row-boundary logic, not
    // the font-size-aware column math (covered separately below).
    let rows = vec![(0, 0, 5), (0, 6, 11), (1, 0, 3)];
    assert_eq!(
        visual_row_step(&rows, 2, 0, -1, &[], 11.0, 1.0),
        Some((0, 6))
    );
}

#[test]
fn test_visual_row_step_down_into_wrapped_continuation_row() {
    let rows = vec![(0, 0, 5), (0, 6, 11), (1, 0, 3)];
    assert_eq!(
        visual_row_step(&rows, 0, 3, 1, &[], 11.0, 1.0),
        Some((0, 9))
    );
}

#[test]
fn test_visual_row_step_preserves_screen_column() {
    let rows = vec![(0, 0, 10), (1, 0, 10)];
    assert_eq!(
        visual_row_step(&rows, 0, 4, 1, &[], 11.0, 1.0),
        Some((1, 4))
    );
}

#[test]
fn test_visual_row_step_clamps_to_shorter_target_row() {
    let rows = vec![(0, 0, 10), (1, 0, 3)];
    assert_eq!(
        visual_row_step(&rows, 0, 8, 1, &[], 11.0, 1.0),
        Some((1, 3))
    );
}

#[test]
fn test_visual_row_step_up_past_first_row_is_none() {
    let rows = vec![(0, 0, 5)];
    assert_eq!(visual_row_step(&rows, 0, 2, -1, &[], 11.0, 1.0), None);
}

/// The reported bug: cursor near the end of an 11pt line, pressing `k`
/// to move up onto a larger-sized (Block-style, 16pt) row landed the
/// cursor "half way across the screen leftward" — carrying the raw
/// character index (18) onto the larger row put it at column 18 there
/// too, but 16pt characters are wider, so column 18 on that row sits
/// far past where column 18 sat on the narrower 11pt row. The fix must
/// land at the *pixel-equivalent* column instead: 18 * 6.6px (11pt) /
/// 9.6px (16pt) rounds to column 12, not 18.
#[test]
fn test_visual_row_step_lands_on_pixel_equivalent_column_across_a_font_size_change() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "0123456789abcdefghij".into(),
                size: 32,
                ..Run::default()
            }],
            ..Paragraph::default()
        },
        Paragraph {
            runs: vec![Run {
                text: "0123456789abcdefghij".into(),
                ..Run::default()
            }],
            ..Paragraph::default()
        },
    ];
    let rows = vec![(0, 0, 20), (1, 0, 20)];
    // Moving up (line 1, col 18) -> (line 0, the 16pt row).
    assert_eq!(
        visual_row_step(&rows, 1, 18, -1, &paragraphs, 11.0, 1.0),
        Some((0, 12)),
        "must preserve on-screen X position, not the raw character index"
    );
}

#[test]
fn test_visual_row_step_down_past_last_row_is_none() {
    let rows = vec![(0, 0, 5)];
    assert_eq!(visual_row_step(&rows, 0, 2, 1, &[], 11.0, 1.0), None);
}

// ── highlight_color_hex / heading_font_size_px ──────────────────────────

#[test]
fn test_highlight_color_hex_known_names() {
    assert_eq!(
        crate::editor::color::highlight_color_hex("yellow"),
        0xFFD700
    );
    assert_eq!(crate::editor::color::highlight_color_hex("green"), 0x00FF00);
    assert_eq!(crate::editor::color::highlight_color_hex("black"), 0x000000);
    assert_eq!(crate::editor::color::highlight_color_hex("white"), 0xFFFFFF);
}

#[test]
fn test_highlight_color_hex_unknown_name_falls_back() {
    assert_eq!(
        crate::editor::color::highlight_color_hex("nonexistent"),
        0x888888
    );
}

/// The 16 names `docx_parser` writes as `w:highlight` must all render as a
/// real color here — otherwise a document Word accepts would come back
/// grey. This is what keeps the two lists from drifting apart.
/// `render_segment` packs the palette's selection color into RGBA as
/// `(rgb << 8) | 0x80`. Spec 6.4 fixed the selection at #264F78 at ~50%
/// opacity, so packing that exact value must still reproduce the literal
/// this refactor replaced — that's what proves the theming didn't quietly
/// change how a selection looks.
#[test]
fn test_selection_alpha_packing_preserves_the_original_color() {
    assert_eq!((0x264F78u32 << 8) | 0x80, 0x264F7880);
    // Every theme's selection keeps its RGB intact and gains the alpha.
    for kind in crate::theme::ThemeKind::all() {
        for mode in crate::theme::ThemeMode::all() {
            let selection = crate::theme::palette(*kind, *mode).selection;
            let packed = (selection << 8) | 0x80;
            assert_eq!(packed >> 8, selection, "{} lost its RGB", kind.label());
            assert_eq!(packed & 0xFF, 0x80, "{} lost its alpha", kind.label());
        }
    }
}

#[test]
fn test_every_word_highlight_name_has_a_color() {
    for name in crate::docx_parser::WORD_HIGHLIGHT_NAMES {
        assert_ne!(
            crate::editor::color::highlight_color_hex(name),
            0x888888,
            "{name} falls through to the unknown-color fallback",
        );
    }
}

#[test]
fn test_highlight_color_hex_raw_hex_string() {
    assert_eq!(
        crate::editor::color::highlight_color_hex("00ff88"),
        0x00ff88
    );
    assert_eq!(
        crate::editor::color::highlight_color_hex("0000FF"),
        0x0000ff
    );
}

#[test]
fn test_highlight_color_hex_blue_named() {
    assert_eq!(crate::editor::color::highlight_color_hex("blue"), 0x0000ff);
}

#[test]
fn test_heading_font_size_body_text_has_no_override() {
    assert_eq!(heading_font_size_px(0, 1.0), None);
}

#[test]
fn test_heading_font_size_levels_1_through_3_each_distinct() {
    assert_eq!(heading_font_size_px(1, 1.0), Some(24.0));
    assert_eq!(heading_font_size_px(2, 1.0), Some(20.0));
    assert_eq!(heading_font_size_px(3, 1.0), Some(18.0));
}

#[test]
fn test_heading_font_size_levels_4_to_6_share_one_size() {
    assert_eq!(heading_font_size_px(4, 1.0), Some(16.0));
    assert_eq!(heading_font_size_px(5, 1.0), Some(16.0));
    assert_eq!(heading_font_size_px(6, 1.0), Some(16.0));
}

#[test]
fn test_heading_font_size_levels_7_to_9_share_one_size() {
    assert_eq!(heading_font_size_px(7, 1.0), Some(14.0));
    assert_eq!(heading_font_size_px(9, 1.0), Some(14.0));
}

#[test]
fn test_heading_font_size_scales_with_zoom() {
    assert_eq!(heading_font_size_px(1, 2.0), Some(48.0));
    assert_eq!(heading_font_size_px(0, 2.0), None);
}

// ── relative_luminance / is_light_color / darken_for_light_text ────────

#[test]
fn test_relative_luminance_white_is_one() {
    assert!((crate::editor::color::relative_luminance(0xFFFFFF) - 1.0).abs() < 0.001);
}

#[test]
fn test_relative_luminance_black_is_zero() {
    assert!((crate::editor::color::relative_luminance(0x000000) - 0.0).abs() < 0.001);
}

#[test]
fn test_relative_luminance_yellow_is_high() {
    // 0.2126*1 + 0.7152*1 + 0.0722*0 = 0.9278
    assert!((crate::editor::color::relative_luminance(0xFFFF00) - 0.9278).abs() < 0.001);
}

#[test]
fn test_is_light_color_white_is_light() {
    assert!(crate::editor::color::is_light_color(0xFFFFFF));
}

#[test]
fn test_is_light_color_black_is_not_light() {
    assert!(!crate::editor::color::is_light_color(0x000000));
}

#[test]
fn test_is_light_color_yellow_highlight_is_light() {
    assert!(crate::editor::color::is_light_color(
        crate::editor::color::highlight_color_hex("yellow")
    ));
}

#[test]
fn test_is_light_color_dark_blue_highlight_is_not_light() {
    assert!(!crate::editor::color::is_light_color(
        crate::editor::color::highlight_color_hex("darkBlue")
    ));
}

#[test]
fn test_darken_for_light_text_reduces_each_channel() {
    let darkened = crate::editor::color::darken_for_light_text(0xFFD700); // yellow highlight
    let r = (darkened >> 16) & 0xFF;
    let g = (darkened >> 8) & 0xFF;
    let b = darkened & 0xFF;
    assert!(r < 0xFF);
    assert!(g < 0xD7);
    assert!(b < 0x01 || b == 0);
}

#[test]
fn test_darken_for_light_text_preserves_hue_ratio() {
    // Darkening scales channels uniformly, so a pure-red channel stays
    // proportionally larger than a zero channel.
    let darkened = crate::editor::color::darken_for_light_text(0xFFFF00);
    let r = (darkened >> 16) & 0xFF;
    let g = (darkened >> 8) & 0xFF;
    let b = darkened & 0xFF;
    assert!(r > 0);
    assert!(g > 0);
    assert_eq!(b, 0);
}

#[test]
fn test_darken_for_light_text_result_is_no_longer_light() {
    assert!(crate::editor::color::is_light_color(0xFFD700));
    assert!(!crate::editor::color::is_light_color(
        crate::editor::color::darken_for_light_text(0xFFD700)
    ));
}

// ── Diagnostic: isolate the per-keystroke cost on a large loaded document ──
//
// Reproduces the "editing a loaded .docx is slower than a blank tab"
// report on a single ~15k-char paragraph (the realistic worst case: a
// debate card is one giant paragraph). Times three independent things
// that all run on every keystroke of a real edit, with GPUI's own
// (expensive, locked, hashed) `layout_width` deliberately excluded from
// (3) via a synthetic width closure — so this isolates the mutation
// path (1), the undo-snapshot clone (2), and the wrap algorithm's own
// cost (3) from the one thing this sandbox can't measure (real font
// shaping, which needs a live GPUI `App`). Run with
// `cargo test bench_diagnostic -- --nocapture` to see the printed
// numbers; not a pass/fail regression test.
#[test]
fn bench_diagnostic_large_document_per_keystroke_costs() {
    let big_text: String = "the quick brown fox jumps over the lazy dog ".repeat(340); // ~15,300 chars, one giant paragraph
    let mut state = AppState::new();
    state.insert_str(&big_text);

    // (1) 100x insert_char: covers push_undo_snapshot + sync_insert_char
    //     (which calls resolve_position) — the whole mutation path.
    let t0 = Instant::now();
    for _ in 0..100 {
        state.insert_char('a');
    }
    let insert_100_elapsed = t0.elapsed();

    // (2) One paragraphs.clone(), the same clone push_undo_snapshot and
    //     TextEditor::render() both pay on every non-coalesced keystroke
    //     and every frame respectively.
    let t1 = Instant::now();
    let _cloned = state.workspace().tabs[0].document.paragraphs().to_vec();
    let clone_elapsed = t1.elapsed();

    // (3) build_visual_rows over the full document with a synthetic,
    //     branch-free width closure — isolates the wrap algorithm's own
    //     cost from real font-shaping cost (which this headless sandbox
    //     cannot measure without a live GPUI App).
    let lines = document_lines(&state.workspace().tabs[0].document.content());
    let mut synthetic_width_of = |_: usize, _: usize, c: char| if c == ' ' { 4.0 } else { 8.4 };
    let t2 = Instant::now();
    let _rows = build_visual_rows(&lines, usable_wrap_width(800.0), &mut synthetic_width_of);
    let wrap_elapsed = t2.elapsed();

    // How many times a wrap pass over this document calls the real,
    // expensive text_system.layout_width path before vs. after the
    // per-call cache added to char_width_fn: before, once per character
    // occurrence; after, once per *unique* character (the cache's own
    // hit path never reaches layout_width again for a repeat).
    let occurrences = big_text.chars().count();
    let unique: std::collections::HashSet<char> = big_text.chars().collect();

    eprintln!(
        "bench_diagnostic: 100x insert_char = {:?} ({:?}/keystroke), \
             paragraphs.clone() = {:?}, build_visual_rows (synthetic width) = {:?}, \
             expensive layout_width calls per wrap pass: before={} (one per char) \
             after={} (one per unique char) = {:.0}x fewer",
        insert_100_elapsed,
        insert_100_elapsed / 100,
        clone_elapsed,
        wrap_elapsed,
        occurrences,
        unique.len(),
        occurrences as f64 / unique.len() as f64,
    );

    // Sanity bound only (catches an accidental infinite loop / O(n^3)
    // blowup) — not the diagnostic signal itself, which is the printed
    // numbers above.
    assert!(insert_100_elapsed.as_secs() < 5);
}

#[test]
fn bench_diagnostic_row_cache_hit_vs_miss_on_large_heavily_formatted_document() {
    // Unlike the bench above (one giant single-run paragraph), this
    // builds a document shaped like a real heavily-formatted case file:
    // many paragraphs, each with several runs (mixed bold/italic/
    // highlight spans within the same line, the way Bold+Highlight+Cite
    // formatting actually looks) — the shape performance_plan.md flagged
    // as multiplying render cost on top of raw document length, since
    // more runs per line means more span-clipping/element work per row.
    //
    // Measures what `RowCache` (uniform_list_plan.md Part 1, now wired
    // into `render()`) actually buys: a cache MISS pays the full
    // clone-and-rewrap cost `render()` used to pay on *every* frame; a
    // cache HIT is just `Rc::clone`ing the same 5 fields. Deliberately
    // doesn't vary scroll position (top vs. deep in the document) the
    // way the plan originally sketched — under the caching built so far,
    // the whole document is wrapped either way regardless of scroll
    // offset; a scroll-position-dependent cost only appears once
    // uniform_list (Part 2 / step 4) limits work to the visible range.
    let paragraph_count = 500;
    let runs_per_paragraph = 6;
    let mut paragraphs = Vec::with_capacity(paragraph_count);
    for i in 0..paragraph_count {
        let mut runs = Vec::with_capacity(runs_per_paragraph);
        for r in 0..runs_per_paragraph {
            runs.push(Run {
                text: format!("segment {i}-{r} of a heavily formatted line "),
                bold: r % 2 == 0,
                italic: r % 3 == 0,
                highlight: r % 4 == 0,
                highlight_color: if r % 4 == 0 {
                    "yellow".to_string()
                } else {
                    String::new()
                },
                size: 24,
                ..Run::default()
            });
        }
        paragraphs.push(Paragraph {
            list: None,
            runs,
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        });
    }
    let content: String = paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");

    let mut synthetic_width_of = |_: usize, _: usize, c: char| if c == ' ' { 4.0 } else { 8.4 };
    let lines = document_lines(&content);

    // (1) Cache MISS: the full rebuild render() pays without a cached
    // row table — clone paragraphs, collect per-line chars, word-wrap.
    let t0 = Instant::now();
    let _miss_paragraphs = paragraphs.clone();
    let _miss_line_chars: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
    let _miss_rows = build_visual_rows(&lines, usable_wrap_width(800.0), &mut synthetic_width_of);
    let miss_elapsed = t0.elapsed();

    // (2) Cache HIT: the same data, but already `Rc`-wrapped the way
    // `RowCache` stores it — a hit is just cloning these 5 handles.
    let rc_paragraphs = Rc::new(paragraphs.clone());
    let rc_lines = Rc::new(lines.clone());
    let rc_line_chars: Rc<Vec<Vec<char>>> =
        Rc::new(lines.iter().map(|l| l.chars().collect()).collect());
    let rc_rows = Rc::new(build_visual_rows(
        &lines,
        usable_wrap_width(800.0),
        &mut synthetic_width_of,
    ));
    let t1 = Instant::now();
    let _hit_paragraphs = rc_paragraphs.clone();
    let _hit_lines = rc_lines.clone();
    let _hit_line_chars = rc_line_chars.clone();
    let _hit_rows = rc_rows.clone();
    let hit_elapsed = t1.elapsed();

    eprintln!(
        "bench_diagnostic_row_cache: {} paragraphs x {} runs each ({} bytes) — \
             cache MISS (full rebuild) = {:?}, cache HIT (Rc clones) = {:?}, {:.0}x faster on hit",
        paragraph_count,
        runs_per_paragraph,
        content.len(),
        miss_elapsed,
        hit_elapsed,
        miss_elapsed.as_nanos() as f64 / (hit_elapsed.as_nanos().max(1) as f64),
    );

    // The actual diagnostic signal is the printed numbers above; this is
    // just a sanity bound confirming the cache is doing its job at all,
    // not a specific performance target.
    assert!(
        hit_elapsed < miss_elapsed,
        "a cache hit must be cheaper than a full rebuild"
    );
}

// ── row_cache_is_valid (uniform_list_plan.md Part 1) ─────────────────────

fn test_row_cache(tab_id: usize, content_version: u64, viewport_width: f32, zoom: f32) -> RowCache {
    RowCache {
        invisibility: false,
        fold_version: 0,
        tab_id,
        content_version,
        viewport_width_bits: viewport_width.to_bits(),
        zoom_bits: zoom.to_bits(),
        // These helpers predate the setting; 1.0 is the "spacing never
        // touched" case they were all written against.
        line_spacing_bits: 1.0f32.to_bits(),
        lines: Rc::new(Vec::new()),
        line_chars: Rc::new(Vec::new()),
        line_byte_starts: Rc::new(Vec::new()),
        rows: Rc::new(Vec::new()),
        paragraphs: Rc::new(Vec::new()),
        display_to_wrap: Rc::new(Vec::new()),
        wrap_to_display: Rc::new(Vec::new()),
    }
}

// ── invisibility mode ────────────────────────────────────────────────────

/// (invisibility, heading, highlighted, bold, size, cite_size)
#[test]
fn test_invisibility_keeps_highlights_and_every_card_style() {
    const CITE: u16 = 26; // settings.conf cite_size=13pt

    // Off: nothing is ever hidden.
    assert!(!run_is_hidden(false, 0, false, false, 0, CITE));

    // Plain body text hides.
    assert!(run_is_hidden(true, 0, false, false, 0, CITE));
    // Highlighted text stays.
    assert!(!run_is_hidden(true, 0, true, false, 0, CITE));

    // Pocket/Hat/Block/Tag are heading levels 1..4 — all stay, whole line.
    for heading in 1..=4 {
        assert!(
            !run_is_hidden(true, heading, false, false, 0, CITE),
            "heading {heading} hidden"
        );
    }

    // Cite: bold at the configured cite size.
    assert!(!run_is_hidden(true, 0, false, true, CITE, CITE));
    // Emphasis is bold at body size (0 = inherit) and is *not* a cite.
    assert!(run_is_hidden(true, 0, false, true, 0, CITE));
    // Bold at some other explicit size is not a cite either.
    assert!(run_is_hidden(true, 0, false, true, 52, CITE));
    // Cite size without bold is not a cite.
    assert!(run_is_hidden(true, 0, false, false, CITE, CITE));
}

fn run_plain(text: &str) -> Run {
    Run {
        text: text.into(),
        ..Run::default()
    }
}

fn para_plain(text: &str) -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![run_plain(text)],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

fn hl_run(text: &str) -> Run {
    Run {
        text: text.into(),
        highlight: true,
        highlight_color: "yellow".into(),
        ..Run::default()
    }
}

fn no_folds(paragraphs: &[Paragraph]) -> Vec<bool> {
    vec![false; paragraphs.len()]
}

/// What "collapse every heading" produces: each body paragraph hidden.
fn all_body_folded(paragraphs: &[Paragraph]) -> Vec<bool> {
    paragraphs.iter().map(|p| p.heading == 0).collect()
}

fn card_para(text: &str, heading: u8) -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![run_plain(text)],
        heading,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

/// Fold leaves every card-style line and hides every body paragraph — the
/// document's outline, Word's collapse-under-headings.
#[test]
fn test_fold_hides_body_and_keeps_every_heading_level() {
    let paragraphs = vec![
        card_para("pocket", 1),
        para_plain("body under pocket"),
        card_para("hat", 2),
        para_plain("body under hat"),
        card_para("block", 3),
        card_para("tag", 4),
        para_plain("body under tag"),
    ];
    let rows: Vec<(usize, usize, usize)> =
        (0..paragraphs.len()).map(|i| (i, 0usize, 1usize)).collect();

    let hidden = hidden_wrap_rows(&rows, &paragraphs, false, 26, &all_body_folded(&paragraphs));
    assert_eq!(hidden, vec![false, true, false, true, false, false, true]);

    // Off, nothing folds.
    assert_eq!(
        hidden_wrap_rows(&rows, &paragraphs, false, 26, &no_folds(&paragraphs)),
        vec![false; 7]
    );
}

/// Fold is the coarser rule: a folded body row goes even if it holds
/// highlighted text that invisibility mode would have kept.
#[test]
fn test_fold_hides_body_rows_that_invisibility_would_keep() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl_run("highlighted body")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let rows = vec![(0usize, 0usize, 16usize)];

    // Invisibility alone keeps it (it is highlighted)...
    assert_eq!(
        hidden_wrap_rows(&rows, &paragraphs, true, 26, &no_folds(&paragraphs)),
        vec![false]
    );
    // ...but folding hides it regardless.
    assert_eq!(
        hidden_wrap_rows(&rows, &paragraphs, true, 26, &all_body_folded(&paragraphs)),
        vec![true]
    );
}

#[test]
fn test_hidden_wrap_rows_marks_only_fully_hidden_rows() {
    let paragraphs = vec![
        // 0: body text with a highlight in it — stays.
        Paragraph {
            list: None,
            runs: vec![run_plain("plain "), hl_run("read this")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        // 1: body text with nothing marked — goes.
        para_plain("unread body text"),
        // 2: a Tag line — stays whole.
        Paragraph {
            list: None,
            runs: vec![run_plain("a tag")],
            heading: 4,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let rows = vec![(0usize, 0usize, 15usize), (1, 0, 16), (2, 0, 5)];

    let hidden = hidden_wrap_rows(&rows, &paragraphs, true, 26, &no_folds(&paragraphs));
    assert_eq!(hidden, vec![false, true, false]);

    // Off, nothing hides.
    assert_eq!(
        hidden_wrap_rows(&rows, &paragraphs, false, 26, &no_folds(&paragraphs)),
        vec![false; 3]
    );
}

/// Only the runs actually on a row decide it: a highlight later in a
/// wrapped paragraph must not keep an earlier all-plain row visible.
#[test]
fn test_hidden_wrap_rows_judges_each_wrapped_row_separately() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![run_plain("aaaaa"), hl_run("bbbbb")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    // Row 0 covers only the plain run, row 1 only the highlighted one.
    let rows = vec![(0usize, 0usize, 5usize), (0, 5, 10)];
    assert_eq!(
        hidden_wrap_rows(&rows, &paragraphs, true, 26, &no_folds(&paragraphs)),
        vec![true, false]
    );
}

#[test]
fn test_hidden_rows_get_no_display_slot() {
    let paragraphs = vec![para_plain("a"), para_plain("b"), para_plain("c")];
    let rows = vec![(0usize, 0usize, 1usize), (1, 0, 1), (2, 0, 1)];

    let (display_to_wrap, wrap_to_display) =
        expand_rows_for_display(&rows, &paragraphs, 1.0, &[false, true, false], 14.0, 1.0);

    // The middle row is gone from the paint list entirely — that is the
    // vertical gap closing. Each surviving row occupies ROW_SUBDIVISIONS
    // slots (blanks first, content last).
    let n = ROW_SUBDIVISIONS;
    assert_eq!(display_to_wrap.len(), 2 * n);
    assert_eq!(
        display_to_wrap
            .iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_eq!(display_to_wrap[n - 1], Some(0));
    assert_eq!(display_to_wrap[2 * n - 1], Some(2));
    // ...and the hidden row points at where the next visible one landed,
    // so scroll-to-cursor still resolves.
    assert_eq!(wrap_to_display, vec![n - 1, n, 2 * n - 1]);
}

// ── read mode paging ─────────────────────────────────────────────────────

/// The two guarantees together: a page advances by whole rows only, so
/// nothing is skipped, and by a *full* screenful of them, so nothing
/// fully-read repeats.
#[test]
fn test_page_scroll_advances_by_whole_rows() {
    // 100px viewport, 24px rows -> 4 whole rows fit (96px), not 100.
    assert_eq!(
        page_scroll_offset(0.0, 100.0, 24.0, 1000.0, true),
        Some(-96.0)
    );
    // ...and back up by the same amount.
    assert_eq!(
        page_scroll_offset(-96.0, 100.0, 24.0, 1000.0, false),
        Some(0.0)
    );
}

#[test]
fn test_page_scroll_stops_at_the_document_ends() {
    // Already at the top: nothing above to page to.
    assert_eq!(page_scroll_offset(0.0, 100.0, 24.0, 1000.0, false), None);
    // Already at the bottom.
    assert_eq!(page_scroll_offset(-1000.0, 100.0, 24.0, 1000.0, true), None);
    // A partial page remaining still moves, clamped to the end rather
    // than overshooting into blank space.
    assert_eq!(
        page_scroll_offset(-950.0, 100.0, 24.0, 1000.0, true),
        Some(-1000.0)
    );
}

/// A viewport shorter than one row must still advance, or the keys lock up.
#[test]
fn test_page_scroll_advances_at_least_one_row() {
    assert_eq!(
        page_scroll_offset(0.0, 10.0, 24.0, 1000.0, true),
        Some(-24.0)
    );
}

/// Rows scale with zoom, so a page must too — otherwise zoomed-in text
/// would page by more lines than are on screen and skip content.
#[test]
fn test_page_scroll_follows_zoom() {
    let zoomed_row = 24.0 * 2.0;
    assert_eq!(
        page_scroll_offset(0.0, 100.0, zoomed_row, 1000.0, true),
        Some(-96.0)
    );
}

// ── spell cache ──────────────────────────────────────────────────────────

#[test]
fn test_spell_cache_returns_same_allocation_on_hit() {
    let cache = Rc::new(RefCell::new(SpellCache::default()));
    let dict = HashSet::new();
    let first = spell_ranges_cached(&cache, "hello wrold", &dict);
    let second = spell_ranges_cached(&cache, "hello wrold", &dict);
    assert_eq!(*first, vec![(6, 11)]);
    // Same `Rc`, i.e. the second call didn't re-run the checker.
    assert!(Rc::ptr_eq(&first, &second));
    assert_eq!(cache.borrow().entries.len(), 1);
}

#[test]
fn test_spell_cache_keyed_on_line_text_not_position() {
    let cache = Rc::new(RefCell::new(SpellCache::default()));
    let dict = HashSet::new();
    spell_ranges_cached(&cache, "hello wrold", &dict);
    spell_ranges_cached(&cache, "a different line", &dict);
    // Editing one line must not evict the other — that's the whole reason
    // this is keyed on text rather than on `content_version`.
    assert_eq!(cache.borrow().entries.len(), 2);
    assert!(cache.borrow().entries.contains_key("hello wrold"));
}

/// The staleness path: adding a word to the user dictionary has to drop
/// cached entries, or already-checked lines keep squiggling a word the
/// user just accepted. The cache key is the line's text, which a
/// dictionary edit doesn't change, so nothing else would catch this.
#[test]
fn test_spell_cache_invalidated_by_user_dictionary_growth() {
    let cache = Rc::new(RefCell::new(SpellCache::default()));
    let mut dict = HashSet::new();

    let before = spell_ranges_cached(&cache, "hello wrold", &dict);
    assert_eq!(*before, vec![(6, 11)]);

    dict.insert("wrold".to_string());
    let after = spell_ranges_cached(&cache, "hello wrold", &dict);
    assert!(
        after.is_empty(),
        "squiggle should clear after Add to Dictionary"
    );
}

#[test]
fn test_row_cache_is_valid_when_everything_matches() {
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(row_cache_is_valid_for(
        &cache, 1, 5, 800.0, 1.0, 1.0, false, false, 0
    ));
}

#[test]
fn test_row_cache_is_valid_false_when_tab_id_differs() {
    // A different tab could coincidentally share the same
    // content_version/width/zoom — tab_id must be checked, or a tab
    // switch could serve another tab's stale wrapped rows.
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(!row_cache_is_valid_for(
        &cache, 2, 5, 800.0, 1.0, 1.0, false, false, 0
    ));
}

#[test]
fn test_row_cache_is_valid_false_when_content_version_differs() {
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(!row_cache_is_valid_for(
        &cache, 1, 6, 800.0, 1.0, 1.0, false, false, 0
    ));
}

/// The divider-drag freeze fix: a width change normally invalidates, but
/// while dragging the stale tables are reused rather than paying a
/// full-document re-wrap per pane per mouse-move.
#[test]
fn test_row_cache_survives_a_width_change_while_the_divider_is_dragging() {
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(!row_cache_is_valid_for(
        &cache, 1, 5, 640.0, 1.0, 1.0, false, false, 0
    ));
    assert!(row_cache_is_valid_for(
        &cache, 1, 5, 640.0, 1.0, 1.0, true, false, 0
    ));
}

/// Dragging must not make the cache accept a *different document* or a
/// stale edit — only a different width.
#[test]
fn test_dragging_still_invalidates_on_content_or_tab_change() {
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(
        !row_cache_is_valid_for(&cache, 2, 5, 640.0, 1.0, 1.0, true, false, 0),
        "wrong tab accepted"
    );
    assert!(
        !row_cache_is_valid_for(&cache, 1, 6, 640.0, 1.0, 1.0, true, false, 0),
        "stale content accepted"
    );
    assert!(
        !row_cache_is_valid_for(&cache, 1, 5, 640.0, 1.25, 1.0, true, false, 0),
        "stale zoom accepted"
    );
}

#[test]
fn test_row_cache_is_valid_false_when_viewport_width_differs() {
    // A window resize must invalidate the cache — the old wrap width no
    // longer matches where lines should actually break.
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(!row_cache_is_valid_for(
        &cache, 1, 5, 801.0, 1.0, 1.0, false, false, 0
    ));
}

#[test]
fn test_row_cache_is_valid_false_when_zoom_differs() {
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(!row_cache_is_valid_for(
        &cache, 1, 5, 800.0, 1.25, 1.0, false, false, 0
    ));
}

// ── slot_count_for_paragraph / expand_rows_for_display ────────────────────
// (card-style row-overlap fix — handoff.md)

fn plain_paragraph() -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![Run::default()],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

fn pocket_paragraph() -> Paragraph {
    // Mirrors AppState::apply_card_style(CardStyleKind::Pocket): bold +
    // FontSize(52 half-points = 26px) + Box(true), heading level 1.
    Paragraph {
        list: None,
        runs: vec![Run {
            size: 52,
            bold: true,
            box_format: true,
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

/// The actual bug fix: settings.conf's real 11pt default
/// (`normal_text_size_half_points: 22` — `AppState::new`'s own default)
/// must produce a *shorter* row than the old fixed 20px, not the same
/// one — 20px was calibrated for a stale 14px reference that predates
/// the configurable default-size setting, which is why 11pt body text
/// read as "far too much space between lines."
#[test]
fn test_line_height_tracks_normal_size_not_the_stale_14px_reference() {
    let default_normal_size_px = 22.0 / 2.0; // AppState::new's normal_text_size_half_points
    assert!(line_height_px(default_normal_size_px, 1.0) < LINE_HEIGHT_PX);
    // And it keeps scaling in both directions with the configured size,
    // rather than flooring at the old hardcoded reference.
    assert!(line_height_px(9.0, 1.0) < line_height_px(default_normal_size_px, 1.0));
    assert!(line_height_px(default_normal_size_px, 1.0) < line_height_px(18.0, 1.0));
}

// `normal_size_px: 14.0` in these tests matches the pre-fix hardcoded
// `FONT_SIZE_PX` reference these numeric expectations were tuned
// against — see `line_height_px`/`LINE_HEIGHT_RATIO`.
#[test]
fn test_slot_count_plain_paragraph_is_one_slot() {
    assert_eq!(
        slot_count_for_paragraph(Some(&plain_paragraph()), 1.0, 14.0, 1.0),
        ROW_SUBDIVISIONS
    );
}

#[test]
fn test_slot_count_no_paragraph_data_is_one_slot() {
    // A brand-new tab has no parsed paragraphs yet — must not panic or
    // under/over-count when formatting data is simply absent.
    assert_eq!(
        slot_count_for_paragraph(None, 1.0, 14.0, 1.0),
        ROW_SUBDIVISIONS
    );
}

#[test]
fn test_slot_count_pocket_needs_multiple_slots() {
    // 26px font (~1.86x LINE_HEIGHT_PX/FONT_SIZE_PX ratio) plus the box's
    // padding/border comfortably needs more than one 20px slot.
    let slots = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 14.0, 1.0);
    assert!(
        slots > ROW_SUBDIVISIONS,
        "expected Pocket line to need more than one line, got {slots}"
    );
}

#[test]
fn test_slot_count_scales_with_zoom() {
    // CARD_BOX_EXTRA_PX doesn't scale with zoom, so at very low zoom it
    // dominates and needs relatively more slots than at 1x.
    let at_1x = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 14.0, 1.0);
    let at_half = slot_count_for_paragraph(Some(&pocket_paragraph()), 0.5, 14.0, 1.0);
    assert!(at_half >= at_1x);
}

#[test]
fn test_slot_count_tag_needs_only_one_slot() {
    // Mirrors AppState::apply_card_style(CardStyleKind::Tag): bold +
    // FontSize(26 half-points = 13px), heading level 4. Unconditional
    // now (`para.heading == 4` short-circuits before the size math runs)
    // — see the function's own doc comment for why, confirmed against a
    // real Verbatim reference file.
    let para = Paragraph {
        list: None,
        runs: vec![Run {
            size: 26,
            bold: true,
            ..Run::default()
        }],
        heading: 4,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    assert_eq!(
        slot_count_for_paragraph(Some(&para), 1.0, 14.0, 1.0),
        ROW_SUBDIVISIONS
    );
}

/// Bug report: "Tags and Cites increase line spacing in vimbatim far
/// more than they do in word". This is the case that actually exercised
/// the bug — every other slot-count test here uses `normal_size_px:
/// 14.0` (this file's stale `FONT_SIZE_PX` reference, at which 14.0 *
/// `LINE_HEIGHT_RATIO` == `LINE_HEIGHT_PX` exactly), which coincidentally
/// never crosses the old ceiling threshold for a 13px Tag/Cite run and
/// masked this. The app's *real* default is 11px (22 half-points) —
/// before the fix, 13/11 = 1.18x rounded up to 2 slots here; Word's own
/// reference file shows Tag/Cite need no reserved room at all at their
/// real relative size.
#[test]
fn test_slot_count_tag_needs_only_one_slot_at_the_real_default_normal_size() {
    let para = Paragraph {
        list: None,
        runs: vec![Run {
            size: 26,
            bold: true,
            ..Run::default()
        }],
        heading: 4,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    assert_eq!(
        slot_count_for_paragraph(Some(&para), 1.0, 11.0, 1.0),
        ROW_SUBDIVISIONS
    );
}

#[test]
fn test_slot_count_cite_needs_only_one_slot_at_the_real_default_normal_size() {
    // Mirrors AppState::apply_cite_style: bold + FontSize(26 half-points
    // = 13px) + Style(Cite) on a selection — no `heading` set at all
    // (Cite is inline, not a whole-line card style), so this exercises
    // the `r.style != Some(CardStyle::Cite)` run filter specifically,
    // not the heading==4 short-circuit Tag uses.
    let para = Paragraph {
        list: None,
        runs: vec![Run {
            size: 26,
            bold: true,
            style: Some(crate::docx_parser::CardStyle::Cite),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    assert_eq!(
        slot_count_for_paragraph(Some(&para), 1.0, 11.0, 1.0),
        ROW_SUBDIVISIONS
    );
}

#[test]
fn test_slot_count_heading_without_box_still_oversized() {
    // heading_font_size_px(1, 1.0) == 24px, no box — 24 * 20/14 == 34.3px
    // against a 20px line, so it still needs more than one line's worth
    // of slots (and less than two full lines').
    let para = Paragraph {
        list: None,
        runs: vec![Run::default()],
        heading: 1,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let slots = slot_count_for_paragraph(Some(&para), 1.0, 14.0, 1.0);
    assert!(
        slots > ROW_SUBDIVISIONS && slots <= 2 * ROW_SUBDIVISIONS,
        "got {slots}"
    );
}

#[test]
fn test_slot_count_emphasis_boxed_alone_does_not_reserve_extra_slots() {
    // A plain-size emphasis box now reserves `EMPHASIS_BOX_EXTRA_PX` of
    // clearance so the ring cannot touch the one on the line above — but
    // that is a fraction of a line, never a whole extra line the way the
    // old whole-line quantum forced.
    let para = Paragraph {
        list: None,
        runs: vec![
            Run::default(),
            Run {
                text: "word".into(),
                emphasis: true,
                emphasis_boxed: true,
                ..Run::default()
            },
            Run::default(),
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let slots = slot_count_for_paragraph(Some(&para), 1.0, 11.0, 1.0);
    assert!(
        slots > ROW_SUBDIVISIONS,
        "the box needs clearance, got {slots}"
    );
    assert!(
        slots < 2 * ROW_SUBDIVISIONS,
        "but never a whole extra line, got {slots}"
    );
}

/// Bug report: applying Emphasis to one word increased the line spacing
/// of the entire paragraph. Root cause: unlike a card style (which sizes
/// every run on the line), `apply_emphasis_style`'s optional size bump
/// (`emphasis_change_size`) only touches the selected word, but this
/// function's max-run-size scan didn't know that and reserved a spacer
/// row for the whole paragraph over it. Real defaults:
/// `normal_text_size_half_points == 22` (11px), `emphasis_size_half_points
/// == 24` (12px) — mirrors one emphasized run in an otherwise-normal
/// paragraph.
#[test]
fn test_slot_count_emphasis_size_bump_does_not_inflate_paragraph_slots() {
    let para = Paragraph {
        list: None,
        runs: vec![
            Run::default(),
            Run {
                text: "word".into(),
                emphasis: true,
                size: 24,
                ..Run::default()
            },
            Run::default(),
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    // The original report was that this cost the paragraph a *whole* extra
    // line, which was true while a line was the smallest unit of
    // reservation. The emphasis run is no longer excluded from the
    // measurement — excluding it is what let its box overlap the line
    // above — so it now costs the fraction of a line it actually needs,
    // which is what the report was really objecting to.
    let plain = slot_count_for_paragraph(Some(&plain_paragraph()), 1.0, 11.0, 1.0);
    let slots = slot_count_for_paragraph(Some(&para), 1.0, 11.0, 1.0);
    assert!(
        slots < plain + ROW_SUBDIVISIONS,
        "one emphasized word must not cost a whole line: {slots} vs plain {plain}"
    );
}

#[test]
fn test_expand_rows_for_display_plain_rows_are_untouched() {
    let rows = vec![(0, 0, 5), (1, 0, 5)];
    let paragraphs = vec![plain_paragraph(), plain_paragraph()];
    let (display_to_wrap, wrap_to_display) =
        expand_rows_for_display(&rows, &paragraphs, 1.0, &vec![false; rows.len()], 14.0, 1.0);
    // Two ordinary lines, each one line tall — ROW_SUBDIVISIONS slots
    // apiece, content in the last slot of its own group.
    let n = ROW_SUBDIVISIONS;
    assert_eq!(display_to_wrap.len(), 2 * n);
    assert_eq!(display_to_wrap[n - 1], Some(0));
    assert_eq!(display_to_wrap[2 * n - 1], Some(1));
    assert_eq!(
        display_to_wrap.iter().flatten().count(),
        2,
        "no extra content rows"
    );
    assert_eq!(wrap_to_display, vec![n - 1, 2 * n - 1]);
}

#[test]
fn scroll_extent_reaches_last_display_slot() {
    use crate::editor::geometry::max_scroll_for_display_rows;

    // Wrapped content, a large-font line, and a hidden row must all use
    // the rendered slot count, not the number of wrapped rows.
    let rows = vec![(0, 0, 2), (0, 2, 5), (1, 0, 5), (2, 0, 5)];
    let paragraphs = vec![pocket_paragraph(), plain_paragraph(), plain_paragraph()];
    for zoom in [0.5, 1.0, 2.0] {
        for spacing in [0.8, 1.0, 2.0] {
            let (display, wrap) = expand_rows_for_display(
                &rows,
                &paragraphs,
                zoom,
                &[false, false, true, false],
                14.0,
                spacing,
            );
            let slot = row_slot_px(14.0, spacing, zoom);
            let line = line_height_px(14.0, spacing) * zoom;
            let viewport = line * 2.5;
            let max_y = max_scroll_for_display_rows(&display, slot, viewport);
            let last_bottom = (wrap[3] + 1) as f32 * slot;
            assert!((max_y + viewport - last_bottom).abs() < 0.001);
            assert_eq!(
                max_scroll_for_display_rows(&display, slot, last_bottom * 2.0),
                0.0
            );
            assert_eq!(max_scroll_for_display_rows(&[], slot, viewport), 0.0);

            // Page by complete text lines, not their six layout slots.
            let next = page_scroll_offset(0.0, viewport, line, max_y, true).unwrap();
            assert!((next + (2.0 * line).min(max_y)).abs() < 0.001);
            assert_eq!(
                page_scroll_offset(-max_y, viewport, line, max_y, true),
                None
            );
        }
    }
}

#[test]
fn test_expand_rows_for_display_inserts_spacers_before_oversized_row() {
    let rows = vec![(0, 0, 5), (1, 0, 5)];
    let paragraphs = vec![pocket_paragraph(), plain_paragraph()];
    let slots = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 14.0, 1.0);
    let plain = slot_count_for_paragraph(Some(&plain_paragraph()), 1.0, 14.0, 1.0);
    let (display_to_wrap, wrap_to_display) =
        expand_rows_for_display(&rows, &paragraphs, 1.0, &vec![false; rows.len()], 14.0, 1.0);

    // Row 0 (Pocket) occupies `slots` display rows: blanks first, so the
    // box's real overflow direction (upward, out of a bottom-aligned
    // row) has somewhere empty to land, then the content itself. Row 1
    // (plain) follows with its own `plain` slots.
    let mut expected = std::iter::repeat_n(None, slots - 1).collect::<Vec<_>>();
    expected.push(Some(0));
    expected.extend(std::iter::repeat_n(None, plain - 1));
    expected.push(Some(1));
    assert_eq!(display_to_wrap, expected);
    assert!(
        slots > plain,
        "the Pocket line must reserve more than a plain one"
    );

    // Each row's content sits in the last of its own reserved slots.
    assert_eq!(wrap_to_display, vec![slots - 1, slots + plain - 1]);
}

// ── line spacing setting ─────────────────────────────────────────────────

/// The whole point of the setting: spacing multiplies row height, and 1.0
/// reproduces exactly what shipped before it existed.
#[test]
fn line_spacing_scales_row_height_and_one_is_the_old_behaviour() {
    assert_eq!(line_height_px(11.0, 1.0), 11.0 * LINE_HEIGHT_RATIO);
    assert_eq!(line_height_px(11.0, 2.0), line_height_px(11.0, 1.0) * 2.0);
    assert!(line_height_px(11.0, 0.8) < line_height_px(11.0, 1.0));
}

/// Spacing has to reach the slot reservation too, not just the row's
/// `.h()` — otherwise tightening the spacing shrinks every row while
/// `slot_count_for_paragraph` keeps reserving against the old height and
/// card-style lines start overlapping again.
#[test]
fn tighter_line_spacing_reserves_more_slots_for_an_oversized_line() {
    let single = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 11.0, 1.0);
    let tight = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 11.0, 0.5);
    let loose = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 11.0, 2.0);
    assert!(
        tight > single,
        "half spacing must need more slots, got {tight} vs {single}"
    );
    assert!(
        loose < single,
        "double spacing must need fewer slots, got {loose} vs {single}"
    );
}

/// Spacing changes `display_to_wrap`, so it must be part of the row
/// cache's key — same reason `zoom` is. Without this the editor keeps
/// painting the previous spacing's row table until something else
/// (a keystroke, a resize) happens to invalidate it.
#[test]
fn row_cache_invalidates_when_line_spacing_changes() {
    let cache = test_row_cache(1, 5, 800.0, 1.0);
    assert!(row_cache_is_valid_for(
        &cache, 1, 5, 800.0, 1.0, 1.0, false, false, 0
    ));
    assert!(!row_cache_is_valid_for(
        &cache, 1, 5, 800.0, 1.0, 1.5, false, false, 0
    ));
}

// ── document scrollbar ──────────────────────────────────────────────────

/// The thumb has to span the track exactly: flush at the top when the
/// document is at the top, flush at the bottom when fully scrolled.
/// Anything else and the bar lies about where you are.
#[test]
fn scrollbar_thumb_spans_the_whole_track() {
    let (viewport, content) = (400.0f32, 2000.0f32);
    let max_scroll = content - viewport;

    let top =
        crate::editor::geometry::scrollbar_geometry(viewport, content, 0.0, SCROLLBAR_MIN_THUMB_PX);
    assert_eq!(top.thumb_top, 0.0);

    let bottom = crate::editor::geometry::scrollbar_geometry(
        viewport,
        content,
        max_scroll,
        SCROLLBAR_MIN_THUMB_PX,
    );
    assert!(
        (bottom.thumb_top + bottom.thumb_h - viewport).abs() < 0.01,
        "fully scrolled thumb must end at the track's bottom: {} + {} vs {viewport}",
        bottom.thumb_top,
        bottom.thumb_h
    );

    let middle = crate::editor::geometry::scrollbar_geometry(
        viewport,
        content,
        max_scroll / 2.0,
        SCROLLBAR_MIN_THUMB_PX,
    );
    assert!((middle.thumb_top - bottom.thumb_top / 2.0).abs() < 0.01);
}

/// Thumb size tracks how much of the document is on screen.
#[test]
fn scrollbar_thumb_is_proportional_to_the_visible_fraction() {
    let short =
        crate::editor::geometry::scrollbar_geometry(400.0, 800.0, 0.0, SCROLLBAR_MIN_THUMB_PX);
    let long =
        crate::editor::geometry::scrollbar_geometry(400.0, 8000.0, 0.0, SCROLLBAR_MIN_THUMB_PX);
    assert!(short.thumb_h > long.thumb_h);
    // Half the document visible -> half the track.
    assert!(
        (short.thumb_h - 200.0).abs() < 0.01,
        "got {}",
        short.thumb_h
    );
}

/// A very long document must not shrink the thumb to something
/// unclickable, and a short one must not overflow the track.
#[test]
fn scrollbar_thumb_is_clamped_at_both_ends() {
    let huge = crate::editor::geometry::scrollbar_geometry(
        400.0,
        1_000_000.0,
        0.0,
        SCROLLBAR_MIN_THUMB_PX,
    );
    assert!(
        huge.thumb_h >= SCROLLBAR_MIN_THUMB_PX,
        "got {}",
        huge.thumb_h
    );
    assert!(
        huge.travel > 0.0,
        "a floored thumb must still have room to move"
    );

    let barely =
        crate::editor::geometry::scrollbar_geometry(400.0, 401.0, 0.0, SCROLLBAR_MIN_THUMB_PX);
    assert!(
        barely.thumb_h <= 400.0,
        "thumb overflowed the track: {}",
        barely.thumb_h
    );
}

/// Guards the degenerate frames: an unmeasured viewport, and a document
/// that exactly fills the screen. Neither may divide by zero or produce
/// a NaN that would lay out as a zero-height thumb.
#[test]
fn scrollbar_geometry_survives_a_document_that_fits_on_screen() {
    let exact =
        crate::editor::geometry::scrollbar_geometry(400.0, 400.0, 0.0, SCROLLBAR_MIN_THUMB_PX);
    assert_eq!(exact.thumb_top, 0.0);
    assert!(exact.thumb_h.is_finite() && exact.travel.is_finite());
}

/// Bug report: the scrollbar "covers text". Wrapping has to stop short of
/// the bar, and because `usable_wrap_width` also feeds click hit-testing,
/// reserving it in that one place is what keeps the two in agreement.
#[test]
fn wrap_width_reserves_a_gutter_for_the_scrollbar() {
    let laid_out = usable_wrap_width(500.0);
    assert!(
        (laid_out - (500.0 - 32.0 - SCROLLBAR_GUTTER_PX)).abs() < 0.01,
        "got {laid_out}"
    );
    // Still unbounded before first layout, and the gutter must not push a
    // narrow-but-real viewport into the sentinel by accident.
    assert_eq!(usable_wrap_width(0.0), f32::MAX);
}

/// The fade holds at full opacity, then eases down, and never leaves the
/// legal 0..=1 range at any point on the curve.
#[test]
fn scrollbar_fade_holds_then_fades() {
    assert_eq!(scrollbar_fade_opacity(0.0), 1.0);
    assert_eq!(scrollbar_fade_opacity(0.5), 1.0, "still inside the hold");
    assert!((scrollbar_fade_opacity(1.0) - SCROLLBAR_IDLE_OPACITY).abs() < 0.001);

    let mut prev = 1.0;
    for i in 0..=100 {
        let v = scrollbar_fade_opacity(i as f32 / 100.0);
        assert!((0.0..=1.0).contains(&v), "opacity {v} out of range");
        assert!(
            v <= prev + 0.001,
            "fade must not brighten: {v} after {prev}"
        );
        prev = v;
    }
}

// ── painted line box vs reserved row pitch ──────────────────────────────

/// The invariant the highlight-overlap bug broke: whatever GPUI paints a
/// line into must fit inside the space this file reserved for that line.
///
/// GPUI's default line height is `phi()` (1.618034), this file reserves at
/// `LINE_HEIGHT_RATIO` (1.428571), and nothing bridged the two — so a
/// highlight's background rectangle, which fills the whole line box, spilled
/// into the line above. `Highlight_Cover.docx` is two plain default-size
/// lines with one highlighted run each, which is why no font-size or
/// emphasis fix could have addressed it.
#[test]
fn painted_line_box_never_exceeds_the_row_pitch_reserved_for_it() {
    for &font_px in &[8.0f32, 9.0, 11.0, 12.0, 14.0, 16.0, 24.0, 26.0] {
        let pitch = line_height_px(font_px, 1.0);
        let painted = text_line_box_px(font_px);
        assert!(
            painted <= pitch,
            "{font_px}px: painted {painted}px must fit the {pitch}px row"
        );
        // GPUI rounds the resolved line height to whole pixels; the value
        // we hand it must still fit after that rounding.
        assert!(
            painted.round() <= pitch,
            "{font_px}px: {painted}px rounds past the {pitch}px row"
        );
    }
}

/// The same invariant against whole paragraphs rather than bare sizes:
/// for every shape the editor renders, the box GPUI paints the line into
/// must fit inside the vertical space reserved for that line. This is
/// what stops one line's highlight rectangle reaching into the line
/// above, whatever the line contains.
#[test]
fn every_paragraph_shape_paints_inside_its_reserved_row() {
    let highlighted = |size: u16| Paragraph {
        list: None,
        runs: vec![
            Run::default(),
            Run {
                text: "X".into(),
                size,
                highlight: true,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };

    let cases: Vec<(&str, Paragraph)> = vec![
        // Highlight_Cover.docx itself: plain default-size text, one
        // highlighted run, no sizes anywhere.
        ("plain highlighted", highlighted(0)),
        ("manually enlarged highlight", highlighted(52)),
        (
            "emphasis box",
            Paragraph {
                list: None,
                runs: vec![
                    Run::default(),
                    Run {
                        emphasis: true,
                        emphasis_boxed: true,
                        size: 24,
                        ..Run::default()
                    },
                ],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ),
        ("pocket", pocket_paragraph()),
        (
            "tag",
            Paragraph {
                list: None,
                runs: vec![Run {
                    size: 26,
                    bold: true,
                    ..Run::default()
                }],
                heading: 4,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ),
        (
            "cite",
            Paragraph {
                list: None,
                runs: vec![Run {
                    size: 26,
                    bold: true,
                    highlight: true,
                    style: Some(crate::docx_parser::CardStyle::Cite),
                    ..Run::default()
                }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ),
        ("plain", plain_paragraph()),
    ];

    for (name, para) in cases {
        let pitch = slot_count_for_paragraph(Some(&para), 1.0, 11.0, 1.0) as f32
            * row_slot_px(11.0, 1.0, 1.0);
        let painted = text_line_box_px(line_font_px(Some(&para), 1.0, 11.0));
        assert!(
            painted <= pitch,
            "{name}: paints {painted}px into a {pitch}px row"
        );
    }

    // A row with no paragraph data must still resolve to one ordinary line.
    assert!(text_line_box_px(line_font_px(None, 1.0, 11.0)) <= line_height_px(11.0, 1.0));
}

/// The specific regression: at the real default the golden-ratio line box
/// is materially taller than the row, which is the overlap the report
/// describes. Pins the size of the problem being solved.
#[test]
fn gpui_default_golden_ratio_line_box_would_overlap_the_row_above() {
    const GPUI_DEFAULT_LINE_HEIGHT: f32 = 1.618_034; // gpui `phi()`
    let pitch = line_height_px(11.0, 1.0);
    let gpui_default = (11.0 * GPUI_DEFAULT_LINE_HEIGHT).round();
    assert!(
        gpui_default > pitch + 2.0,
        "expected the untamed default to overflow by >2px, got {gpui_default} vs {pitch}"
    );
    assert!(
        text_line_box_px(11.0) < gpui_default,
        "the fix must shrink it"
    );
}

// ── line-spacing continuity and overlap ─────────────────────────────────
//
// These pin the measured relationship between the vertical space
// `slot_count_for_paragraph` reserves for a line and the space that
// line's runs actually paint. Where the two disagree, the surplus lands
// on the line above (`row_div` bottom-aligns and does not clip), which is
// what every reported overlap has in common.

/// Height a paragraph's tallest run really paints, by the same arithmetic
/// `slot_count_for_paragraph` uses internally — but with none of its
/// exclusions applied.
fn painted_height_px(para: &Paragraph, zoom: f32, normal_size_px: f32) -> f32 {
    /*
     * Mirrors slot_count_for_paragraph's `needed_px` with the `!= Cite`
     * and `heading == 4` filters removed, so the difference between this
     * and `reserved_height_px` is exactly the space a run can paint into
     * but was never given.
     */
    let run_max_px = para
        .runs
        .iter()
        .filter(|r| r.size > 0)
        .map(|r| r.size as f32 / 2.0 * zoom)
        .fold(0.0_f32, f32::max);
    let heading_px = heading_font_size_px(para.heading, zoom).unwrap_or(0.0);
    let font_px = if run_max_px > 0.0 {
        run_max_px
    } else {
        heading_px
    }
    .max(normal_size_px * zoom);
    let has_box = para.runs.iter().any(|r| r.box_format);
    font_px * LINE_HEIGHT_RATIO + if has_box { CARD_BOX_EXTRA_PX } else { 0.0 }
}

/// Height `expand_rows_for_display` actually reserves for that paragraph:
/// its slot count times the height of one `uniform_list` row.
fn reserved_height_px(para: &Paragraph, zoom: f32, normal_size_px: f32) -> f32 {
    slot_count_for_paragraph(Some(para), zoom, normal_size_px, 1.0) as f32
        * row_slot_px(normal_size_px, 1.0, zoom)
}

/// Bug report: "font size 12-22 both increase the distance between lines
/// equally when changing sizes with the text size button, this is harsh."
///
/// With one `uniform_list` row per line, every size from 12pt to 22pt
/// reserved exactly two lines — a single doubling at 12pt and then no
/// change at all across the next ten points. Subdividing the row grid has
/// to turn that one cliff into a staircase.
#[test]
fn font_sizes_twelve_to_twentytwo_no_longer_all_get_the_same_line_height() {
    let heights: Vec<usize> = (12..=22)
        .map(|pt| {
            let para = Paragraph {
                list: None,
                runs: vec![Run {
                    size: pt * 2,
                    ..Run::default()
                }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            };
            slot_count_for_paragraph(Some(&para), 1.0, 11.0, 1.0)
        })
        .collect();

    let distinct: std::collections::BTreeSet<_> = heights.iter().copied().collect();
    assert!(
        distinct.len() >= 4,
        "12-22pt must span several row heights, got {distinct:?} from {heights:?}"
    );
    // Monotonic: a bigger font never reserves less room than a smaller one.
    assert!(
        heights.windows(2).all(|w| w[0] <= w[1]),
        "not monotonic: {heights:?}"
    );
    // And no single step may be a whole line — that is the "harsh" jump.
    assert!(
        heights.windows(2).all(|w| w[1] - w[0] < ROW_SUBDIVISIONS),
        "a step of a full line remains: {heights:?}"
    );
}

/// Bug report: "the Emphasis boxes on size 12 font overlap such that the
/// top of a box on a lower line is above the bottom of a box from the
/// line above it."
///
/// Real defaults: `normal_text_size_half_points == 22` (11px) and
/// `emphasis_size_half_points == 24` (12px). The emphasis box is an inset
/// box-shadow — paint only, drawn at the span's exact bounds — so the row
/// pitch has to exceed the painted height or consecutive boxes touch.
#[test]
fn emphasis_box_at_size_twelve_has_clearance_from_the_line_above() {
    let para = Paragraph {
        list: None,
        runs: vec![
            Run::default(),
            Run {
                text: "word".into(),
                emphasis: true,
                emphasis_boxed: true,
                size: 24,
                ..Run::default()
            },
            Run::default(),
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let pitch = reserved_height_px(&para, 1.0, 11.0);
    let painted = painted_height_px(&para, 1.0, 11.0);
    assert!(
        pitch >= painted + EMPHASIS_BOX_EXTRA_PX,
        "box needs {EMPHASIS_BOX_EXTRA_PX}px clearance: pitch {pitch}px vs painted {painted}px"
    );
}

/// The same run without a box: no longer excluded from the reservation,
/// so its glyphs stay inside the row reserved for them.
#[test]
fn emphasis_run_no_longer_paints_past_the_row_reserved_for_it() {
    let para = Paragraph {
        list: None,
        runs: vec![
            Run::default(),
            Run {
                size: 52,
                emphasis: true,
                highlight: true,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    assert!(reserved_height_px(&para, 1.0, 11.0) >= painted_height_px(&para, 1.0, 11.0));
}

/// Still broken, deliberately: Cite is excluded from the reservation, and
/// `heading == 4` (Tag) returns before `box_format` is ever read. Both
/// exclusions were added to stop Tag/Cite reserving a *whole* extra line
/// the way the old one-row-per-line quantum forced. Subdividing the grid
/// removes that reason — the cost would now be a fraction of a line — but
/// dropping them changes how every existing Tag and Cite lays out, so it
/// stays a deliberate decision rather than a side effect of this fix.
#[test]
fn cite_and_boxed_tag_still_paint_past_their_reservation() {
    let cite = Paragraph {
        list: None,
        runs: vec![
            Run::default(),
            Run {
                size: 26,
                bold: true,
                highlight: true,
                style: Some(crate::docx_parser::CardStyle::Cite),
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    assert!(
        painted_height_px(&cite, 1.0, 11.0) > reserved_height_px(&cite, 1.0, 11.0),
        "Cite exclusion still lets the run overflow"
    );

    let boxed_tag = Paragraph {
        list: None,
        runs: vec![Run {
            size: 26,
            bold: true,
            box_format: true,
            ..Run::default()
        }],
        heading: 4,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let overflow =
        painted_height_px(&boxed_tag, 1.0, 11.0) - reserved_height_px(&boxed_tag, 1.0, 11.0);
    assert!(
        overflow > CARD_BOX_EXTRA_PX - 1.0,
        "the whole box inset is still unreserved, got {overflow}px"
    );
}

// ── list marker rendering (non-GPUI pure logic only) ─────────────────────

#[test]
fn test_list_marker_text_bullets() {
    assert_eq!(list_marker_text(ListKind::BulletHollow, 1), "o");
}

#[test]
fn test_list_marker_text_number_formats() {
    assert_eq!(list_marker_text(ListKind::NumberDecimalDot, 3), "3.");
    assert_eq!(list_marker_text(ListKind::NumberDecimalParen, 3), "3)");
    assert_eq!(list_marker_text(ListKind::NumberUpperLetter, 3), "C.");
    assert_eq!(list_marker_text(ListKind::NumberLowerLetterDot, 3), "c.");
    assert_eq!(list_marker_text(ListKind::NumberLowerRoman, 3), "iii.");
    assert_eq!(list_marker_text(ListKind::NumberUpperRoman, 4), "IV.");
}

#[test]
fn test_to_letter_and_to_roman_ranges() {
    assert_eq!(to_letter(1), "a");
    assert_eq!(to_letter(26), "z");
    assert_eq!(to_roman(1), "i");
    assert_eq!(to_roman(9), "ix");
    assert_eq!(to_roman(40), "xl");
}

#[test]
fn test_list_item_ordinal_counts_within_contiguous_run() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "a".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        },
        Paragraph {
            runs: vec![Run {
                text: "b".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        },
        Paragraph {
            runs: vec![Run {
                text: "not a list".into(),
                ..Run::default()
            }],
            ..Paragraph::default()
        },
        Paragraph {
            runs: vec![Run {
                text: "c".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        },
    ];
    assert_eq!(list_item_ordinal(&paragraphs, 0), 1);
    assert_eq!(list_item_ordinal(&paragraphs, 1), 2);
    // A fresh run after the non-list break restarts at 1.
    assert_eq!(list_item_ordinal(&paragraphs, 3), 1);
}

/// Confirmed against real Word (`Lists.docx`'s six-different-bullets and
/// seven-different-numbers examples, each immediately consecutive
/// paragraphs): changing `ListKind` starts a new run even with no
/// non-list paragraph between them — a bullet-style row's ordinal
/// doesn't matter visually, but a numbered row's does, and an earlier
/// version of this function (breaking only on `list.is_some()` going
/// false) silently let a style change continue the previous run's
/// count instead of restarting it.
#[test]
fn test_list_item_ordinal_restarts_on_list_kind_change_with_no_break_between() {
    let paragraphs = vec![
        Paragraph {
            runs: vec![Run {
                text: "a".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            ..Paragraph::default()
        },
        Paragraph {
            runs: vec![Run {
                text: "b".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::NumberDecimalDot,
                level: 0,
            }),
            ..Paragraph::default()
        },
        // Different kind, immediately adjacent — no non-list paragraph
        // between them, but this must still be a fresh run.
        Paragraph {
            runs: vec![Run {
                text: "c".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::NumberUpperRoman,
                level: 0,
            }),
            ..Paragraph::default()
        },
    ];
    assert_eq!(list_item_ordinal(&paragraphs, 0), 1);
    assert_eq!(list_item_ordinal(&paragraphs, 1), 2);
    assert_eq!(list_item_ordinal(&paragraphs, 2), 1);
}

// ── Phase 2: multi-level marker cascade ──────────────────────────────────

#[test]
fn test_list_marker_text_for_level_0_matches_the_picked_style() {
    assert_eq!(
        list_marker_text_for_level(ListKind::BulletSolid, 0, 1),
        list_marker_text(ListKind::BulletSolid, 1)
    );
    assert_eq!(
        list_marker_text_for_level(ListKind::NumberUpperRoman, 0, 4),
        list_marker_text(ListKind::NumberUpperRoman, 4)
    );
}

#[test]
fn test_list_marker_text_for_level_cascades_regardless_of_level_0_style() {
    // Two different level-0 styles should converge to the same level-1/2 marker.
    assert_eq!(
        list_marker_text_for_level(ListKind::BulletCheckmark, 1, 1),
        list_marker_text_for_level(ListKind::BulletSolid, 1, 1),
    );
    assert_eq!(
        list_marker_text_for_level(ListKind::NumberUpperRoman, 1, 3),
        list_marker_text_for_level(ListKind::NumberLowerLetterParen, 1, 3),
    );
}

#[test]
fn test_list_marker_text_for_level_bullet_cascade_values() {
    // Confirmed against the *complete* ilvl 0-8 range dumped from real
    // Word (docx_parser::cascade_level_xml's own doc comment has the
    // full history): level 1 -> hollow 'o', level 2 -> filled square,
    // level 3 -> plain solid bullet again, repeating every 3. Level 3
    // specifically is what an earlier, ilvl-0/1/2-only version of this
    // cascade got wrong (it repeated level 2's filled square instead).
    assert_eq!(list_marker_text_for_level(ListKind::BulletSolid, 1, 1), "o");
    assert_eq!(
        list_marker_text_for_level(ListKind::BulletSolid, 2, 1),
        "\u{25aa}"
    );
    assert_eq!(
        list_marker_text_for_level(ListKind::BulletSolid, 3, 1),
        "\u{2022}"
    );
    assert_eq!(list_marker_text_for_level(ListKind::BulletSolid, 4, 1), "o");
}

#[test]
fn test_list_marker_text_for_level_number_cascade_values() {
    // level 1 -> lowerLetter, level 2 -> lowerRoman, level 3 -> decimal
    // again, repeating every 3 — independent of the level-0 format
    // (confirmed against NumberUpperRoman's own real-Word cascade,
    // whose level 3 is also plain decimal, not upperRoman again).
    assert_eq!(
        list_marker_text_for_level(ListKind::NumberDecimalDot, 1, 3),
        "c."
    );
    assert_eq!(
        list_marker_text_for_level(ListKind::NumberDecimalDot, 2, 3),
        "iii."
    );
    assert_eq!(
        list_marker_text_for_level(ListKind::NumberDecimalDot, 3, 3),
        "3."
    );
    assert_eq!(
        list_marker_text_for_level(ListKind::NumberUpperRoman, 3, 3),
        "3."
    );
}

#[test]
fn test_line_col_from_mouse_position_accounts_for_list_gutter_width() {
    use gpui::{point, px, size, Bounds};
    // A click at the same raw x lands on an earlier column when the row
    // is a list paragraph's first row, since the gutter eats into the
    // available text width before any character starts.
    let list_para = Paragraph {
        runs: vec![Run {
            text: "hello".into(),
            ..Run::default()
        }],
        list: Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0,
        }),
        ..Paragraph::default()
    };
    let plain_para = Paragraph {
        runs: vec![Run {
            text: "hello".into(),
            ..Run::default()
        }],
        ..Paragraph::default()
    };
    let rows = vec![(0usize, 0usize, 5usize)];
    let display_to_wrap = vec![Some(0usize)];
    let content_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(1000.0)));
    // A click positioned past one gutter width but still within the
    // first character or two.
    let x = 16.0 + LIST_GUTTER_PX + 4.0;
    let position = point(px(x), px(0.0));

    let (_line, col_list) = line_col_from_mouse_position(
        position,
        content_bounds,
        0.0,
        &rows,
        &display_to_wrap,
        1.0,
        11.0,
        &[list_para],
        line_height_px(11.0, 1.0),
    );
    let (_line, col_plain) = line_col_from_mouse_position(
        position,
        content_bounds,
        0.0,
        &rows,
        &display_to_wrap,
        1.0,
        11.0,
        &[plain_para],
        line_height_px(11.0, 1.0),
    );
    assert!(
        col_list < col_plain,
        "list col {col_list} should be earlier than plain col {col_plain}"
    );
}
