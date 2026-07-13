use crate::docx_parser::{Alignment, Paragraph, Run};

/// Resolves a byte offset into `content` (the flat string vim-mode and the
/// rest of the editor operate on) into a `(paragraph_index, run_index,
/// char_offset_within_run)` triple against the live `paragraphs` model —
/// the core primitive the rich-text formatting plan's Phase 1 (keeping
/// formatting in sync across edits) and Phase 2 (`apply_formatting`) both
/// build on.
///
/// Paragraph boundaries are exactly line boundaries: `docx_parser`'s
/// `paragraphs_to_plain_text` joins paragraphs with `'\n'` and never emits
/// one *within* a paragraph, so a byte offset landing exactly on that
/// separator is unambiguous.
///
/// A `byte_offset` landing exactly at a paragraph or run boundary resolves
/// to the *end of the earlier* paragraph/run (its last run, at
/// `text.len()`) rather than the start of the next one — matching how
/// typing at that position naturally continues the preceding run/paragraph's
/// formatting rather than adopting the next one's.
pub fn resolve_position(paragraphs: &[Paragraph], byte_offset: usize) -> (usize, usize, usize) {
    let mut cumulative = 0usize;
    for (para_idx, para) in paragraphs.iter().enumerate() {
        let para_len: usize = para.runs.iter().map(|r| r.text.len()).sum();
        let para_end = cumulative + para_len;
        if byte_offset <= para_end {
            let mut run_cum = cumulative;
            for (run_idx, run) in para.runs.iter().enumerate() {
                let run_end = run_cum + run.text.len();
                if byte_offset <= run_end {
                    return (para_idx, run_idx, byte_offset - run_cum);
                }
                run_cum = run_end;
            }
            return (para_idx, 0, 0); // defensive: paragraph has no runs
        }
        cumulative = para_end + 1; // +1 skips the separating '\n'
    }
    // byte_offset beyond the whole document — shouldn't happen for a valid
    // cursor, but clamp to the very end of the last paragraph rather than
    // panicking.
    let last_idx = paragraphs.len().saturating_sub(1);
    match paragraphs.get(last_idx).and_then(|p| p.runs.len().checked_sub(1)) {
        Some(last_run_idx) => (last_idx, last_run_idx, paragraphs[last_idx].runs[last_run_idx].text.len()),
        None => (0, 0, 0),
    }
}

/// Keeps `paragraphs` in sync with inserting `ch` at `byte_offset` into the
/// equivalent `content` string (rich-text formatting plan, Phase 1) — the
/// choke-point primitive `insert_char` and (via `sync_insert_str` below)
/// `insert_str`/`replace_vim_range` all build on.
///
/// A plain character inherits whatever run it lands inside — typing inside
/// a bold run produces more bold text, matching real rich-text editors.
/// `'\n'` is structural: it splits the paragraph at this position into two.
pub fn sync_insert_char(paragraphs: &mut Vec<Paragraph>, byte_offset: usize, ch: char) {
    let (para_idx, run_idx, char_offset) = resolve_position(paragraphs, byte_offset);
    if ch == '\n' {
        // split_paragraph_at constructs two brand-new Paragraph literals,
        // which already default unsupported_xml to None - no separate
        // clear needed for the newline case.
        split_paragraph_at(paragraphs, para_idx, run_idx, char_offset);
    } else {
        paragraphs[para_idx].runs[run_idx].text.insert(char_offset, ch);
        paragraphs[para_idx].unsupported_xml = None;
    }
}

/// Keeps `paragraphs` in sync with inserting `text` at `byte_offset`, one
/// character at a time via `sync_insert_char` — simple and correct at the
/// cost of not being O(1) for multi-line paste, which isn't a hot path in
/// this editor.
pub fn sync_insert_str(paragraphs: &mut Vec<Paragraph>, byte_offset: usize, text: &str) {
    let mut offset = byte_offset;
    for ch in text.chars() {
        sync_insert_char(paragraphs, offset, ch);
        offset += ch.len_utf8();
    }
}

fn split_paragraph_at(paragraphs: &mut Vec<Paragraph>, para_idx: usize, run_idx: usize, char_offset: usize) {
    /*
     * Splits paragraph `para_idx` into two at (run_idx, char_offset): the
     * run being split becomes a "head" run ending the first paragraph, and
     * a "tail" run starting the second, followed by whatever runs came
     * after it. The new second paragraph always gets `heading: 0` — real
     * Word itself reverts to body style after pressing Enter inside a
     * heading, so this matches rather than deviates from that convention.
     *
     * When splitting a line with card-style formatting (Pocket/Hat/Block/
     * Tag, i.e. `heading != 0`), the new paragraph's formatting reverts to
     * plain (see `was_heading` below) instead of inheriting the split run's
     * bold/size/box/underline/alignment — otherwise `heading` would reset
     * but the line would still visually look like the card style.
     */
    let para = &paragraphs[para_idx];
    let split_run = &para.runs[run_idx];
    let mut head_run = split_run.clone();
    head_run.text = split_run.text[..char_offset].to_string();

    let heading = para.heading;
    let alignment = para.alignment;
    // A card style (Pocket/Hat/Block/Tag) is entirely run-level formatting
    // plus this paragraph-level `heading` marker (state.rs's
    // `apply_card_style`) — so splitting a heading line must revert the
    // *new* paragraph's formatting the same way real Word reverts to Normal
    // style after Enter inside a heading, not just clone the split run's
    // bold/size/box/underline onto it. `size: 0` (via `Run::default()`) is
    // this codebase's existing "inherit normal size" convention (also what
    // a brand-new blank document's first paragraph starts with), so this
    // doesn't need a caller-supplied default size.
    let was_heading = heading != 0;
    let tail_text = split_run.text[char_offset..].to_string();
    let (tail_run, new_alignment) = if was_heading {
        (Run { text: tail_text, ..Run::default() }, Alignment::default())
    } else {
        let mut run = split_run.clone();
        run.text = tail_text;
        (run, alignment)
    };

    let mut para_a_runs: Vec<Run> = para.runs[..run_idx].to_vec();
    para_a_runs.push(head_run);

    let mut para_b_runs: Vec<Run> = vec![tail_run];
    para_b_runs.extend(para.runs[run_idx + 1..].to_vec());

    paragraphs.splice(
        para_idx..=para_idx,
        [
            Paragraph { runs: para_a_runs, heading, alignment, unsupported_xml: None },
            Paragraph { runs: para_b_runs, heading: 0, alignment: new_alignment, unsupported_xml: None },
        ],
    );
}

/// Keeps `paragraphs` in sync with deleting `[start, end)` from the
/// equivalent `content` string (rich-text formatting plan, Phase 1) — the
/// choke-point primitive `delete_selection_raw`/`replace_vim_range` build
/// on. A no-op when `start >= end`.
///
/// Runs left empty by truncation are dropped (formatting shouldn't "leak"
/// via a zero-length run) — except a paragraph is never left with zero
/// runs, since every other function here assumes at least one always
/// exists.
pub fn sync_delete_range(paragraphs: &mut Vec<Paragraph>, start: usize, end: usize) {
    if start >= end { return; }
    let (start_para, start_run, start_char) = resolve_position(paragraphs, start);
    let (end_para, end_run, end_char) = resolve_position(paragraphs, end);

    if start_para == end_para {
        delete_within_runs(&mut paragraphs[start_para].runs, start_run, start_char, end_run, end_char);
        paragraphs[start_para].unsupported_xml = None;
        clear_heading_if_now_empty(&mut paragraphs[start_para]);
        return;
    }

    let heading = paragraphs[start_para].heading;
    let alignment = paragraphs[start_para].alignment;
    let mut merged_runs: Vec<Run> = paragraphs[start_para].runs[..start_run].to_vec();
    let mut head_run = paragraphs[start_para].runs[start_run].clone();
    head_run.text.truncate(start_char);
    merged_runs.push(head_run);

    let mut tail_run = paragraphs[end_para].runs[end_run].clone();
    tail_run.text = tail_run.text[end_char..].to_string();
    merged_runs.push(tail_run);
    merged_runs.extend(paragraphs[end_para].runs[end_run + 1..].to_vec());

    merged_runs.retain(|r| !r.text.is_empty());
    merge_adjacent_same_format_runs(&mut merged_runs);
    // A card style's box/bold/size lives on its runs, but its heading/
    // center-alignment are paragraph-level fields deletion never otherwise
    // touches. Merging across a paragraph boundary should carry over the
    // surviving (start) paragraph's own heading/alignment — e.g. backspacing
    // away just an empty trailing line should leave a Pocket line exactly as
    // it was, still centered. But if this merge also emptied out all the
    // text, there's no run-level formatting left to justify keeping them
    // either: reset both to plain, matching what Clear Formatting already
    // does explicitly (see `apply_formatting_to_line`'s `ClearAll` arm) —
    // otherwise `text_editor.rs`'s heading-driven bold/oversized paragraph
    // render keeps applying to an empty line whose actual pocket-formatted
    // text has been fully backspaced away.
    let now_empty = merged_runs.is_empty();
    if now_empty {
        merged_runs.push(Run::default());
    }
    let (heading, alignment) = if now_empty { (0, Alignment::default()) } else { (heading, alignment) };

    paragraphs.splice(start_para..=end_para, [Paragraph { runs: merged_runs, heading, alignment, unsupported_xml: None }]);
}

/// Once a paragraph's own within-paragraph deletion (not a cross-paragraph
/// merge — see `sync_delete_range`'s other branch) empties out all its
/// runs, `delete_within_runs` already resets the surviving run to
/// `Run::default()` — but the paragraph-level `heading`/`alignment` fields
/// it never touches would otherwise keep a card style's phantom bold/
/// oversized/centered look alive on what's now just an empty line.
fn clear_heading_if_now_empty(para: &mut Paragraph) {
    if para.runs.iter().all(|r| r.text.is_empty()) {
        para.heading = 0;
        para.alignment = Alignment::default();
    }
}

fn delete_within_runs(runs: &mut Vec<Run>, start_run: usize, start_char: usize, end_run: usize, end_char: usize) {
    if start_run == end_run {
        runs[start_run].text.replace_range(start_char..end_char, "");
    } else {
        runs[start_run].text.truncate(start_char);
        let tail = runs[end_run].text[end_char..].to_string();
        runs[end_run].text = tail;
        runs.drain(start_run + 1..end_run);
    }
    runs.retain(|r| !r.text.is_empty());
    merge_adjacent_same_format_runs(runs);
    if runs.is_empty() {
        runs.push(Run::default());
    }
}

/// Merges adjacent runs that share identical formatting into one, comparing
/// every `Run` field except `text`. Deletion can make two runs that
/// previously had unrelated text become textually adjacent — without this,
/// repeated edits would let paragraphs accumulate more and more same-format
/// runs indefinitely.
pub(crate) fn merge_adjacent_same_format_runs(runs: &mut Vec<Run>) {
    let mut i = 0;
    while i + 1 < runs.len() {
        let same_format = runs[i].bold == runs[i + 1].bold
            && runs[i].italic == runs[i + 1].italic
            && runs[i].underline == runs[i + 1].underline
            && runs[i].double_underline == runs[i + 1].double_underline
            && runs[i].strikethrough == runs[i + 1].strikethrough
            && runs[i].highlight == runs[i + 1].highlight
            && runs[i].highlight_color == runs[i + 1].highlight_color
            && runs[i].size == runs[i + 1].size
            && runs[i].font == runs[i + 1].font
            && runs[i].color == runs[i + 1].color
            && runs[i].box_format == runs[i + 1].box_format
            && runs[i].whitespace_preserve == runs[i + 1].whitespace_preserve;
        if same_format {
            let next_text = runs[i + 1].text.clone();
            runs[i].text.push_str(&next_text);
            runs.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Maps a paragraph's runs onto `(char_start, char_end, run_index)` spans
/// in *character*-column space (not bytes) — the coordinate system
/// `text_editor.rs`'s existing cursor/selection-overlay code
/// (`line_segments`) already uses, since a paragraph is exactly one
/// rendered line (rich-text formatting plan, Phase 1's rendering task).
/// Lets the renderer merge formatting-run boundaries into that same
/// breakpoint-and-classify algorithm instead of needing a second,
/// competing way to split a line into spans.
pub fn paragraph_run_char_spans(para: &Paragraph) -> Vec<(usize, usize, usize)> {
    let mut spans = Vec::with_capacity(para.runs.len());
    let mut char_cum = 0usize;
    for (run_idx, run) in para.runs.iter().enumerate() {
        let run_char_len = run.text.chars().count();
        spans.push((char_cum, char_cum + run_char_len, run_idx));
        char_cum += run_char_len;
    }
    spans
}

/// A formatting operation applied to a byte range (rich-text formatting
/// plan, Phase 2). Mirrors spec 7.2's `FormatOp`, extended with
/// `Italic`/`FontFamily`/`Color` per this feature's scope decision.
#[derive(Clone, Debug, PartialEq)]
pub enum FormatOp {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    DoubleUnderline(bool),
    Strikethrough(bool),
    /// `None` removes the highlight; `Some(name)` sets it to one of spec
    /// 6.2's Word highlight-color names.
    Highlight(Option<String>),
    /// `0` removes any explicit size override (half-points, matching
    /// `Run.size`'s own unit).
    FontSize(u16),
    FontFamily(Option<String>),
    /// Docx hex color (`"RRGGBB"`), or `None` to remove the override.
    Color(Option<String>),
    Box(bool),
    /// Clears every character-formatting field (bold/italic/underline/
    /// highlight/font/color) back to the unformatted default, and size to
    /// `default_size` (half-points — spec: "Clear" resets to settings.conf's
    /// `large_size`, not to "no override").
    ClearAll { default_size: u16 },
}

/// Applies `op` to every run (byte-)range `[start, end)` spans (spec 7.2),
/// splitting runs at the boundaries first so a run that only partially
/// overlaps the range doesn't get formatted in its entirety. A no-op when
/// `start >= end`.
pub fn apply_formatting(paragraphs: &mut Vec<Paragraph>, start: usize, end: usize, op: FormatOp) {
    if start >= end { return; }
    let (start_para, start_run, start_char) = resolve_position(paragraphs, start);
    let (end_para, end_run, end_char) = resolve_position(paragraphs, end);

    // Split at the END position first so START's already-resolved
    // (para_idx, run_idx) pair isn't shifted by a run being inserted
    // ahead of it.
    split_run_at_position(paragraphs, end_para, end_run, end_char);
    split_run_at_position(paragraphs, start_para, start_run, start_char);

    // After splitting, no run straddles `start` or `end` anymore — every
    // run now falls either fully inside [start, end) or fully outside it.
    // Re-walking every run's absolute byte range (the same cumulative-sum
    // approach `resolve_position` itself uses) and formatting the ones
    // inside is simpler and more robust than trying to track exactly which
    // indices the two splits above shifted.
    let mut cumulative = 0usize;
    for para in paragraphs.iter_mut() {
        let mut touched = false;
        for run in para.runs.iter_mut() {
            let run_start = cumulative;
            let run_end = cumulative + run.text.len();
            if run_start >= start && run_end <= end {
                apply_format_op(run, &op);
                touched = true;
            }
            cumulative = run_end;
        }
        cumulative += 1; // the paragraph-separating '\n'
        merge_adjacent_same_format_runs(&mut para.runs);
        if touched {
            para.unsupported_xml = None;
        }
    }
}

fn split_run_at_position(paragraphs: &mut [Paragraph], para_idx: usize, run_idx: usize, byte_offset: usize) {
    /*
     * Splits `paragraphs[para_idx].runs[run_idx]` into two runs (same
     * formatting, just the text divided) at `byte_offset` — unless
     * `byte_offset` already sits at a natural run boundary (0 or the run's
     * full length), in which case there's nothing to split.
     */
    let run = &paragraphs[para_idx].runs[run_idx];
    if byte_offset == 0 || byte_offset >= run.text.len() {
        return;
    }
    let mut head = run.clone();
    head.text.truncate(byte_offset);
    let mut tail = run.clone();
    tail.text = run.text[byte_offset..].to_string();
    paragraphs[para_idx].runs.splice(run_idx..=run_idx, [head, tail]);
}

/// True if every run overlapping `[start, end)` is already in the "on"
/// state `op` would set (bug fix: toolbar buttons should toggle off when
/// re-clicked on already-formatted text, matching Word's own toolbar
/// behavior, rather than always re-applying). An empty range, or an
/// untogglable op (`FontSize`/`FontFamily`/`Color`/`ClearAll`), is never
/// considered active. Runs entirely outside `[start, end)` are ignored, so
/// this reads the range's current state without mutating or splitting
/// anything (unlike `apply_formatting`).
/// Applies paragraph-level alignment to all paragraphs that overlap `[start, end)`,
/// or to the single paragraph containing the cursor when start == end.
pub fn apply_paragraph_alignment(paragraphs: &mut Vec<Paragraph>, start: usize, end: usize, alignment: Alignment) {
    if start > end { return; }
    let (start_para, _, _) = resolve_position(paragraphs, start);
    let (end_para, _, _) = if start == end {
        (start_para, 0, 0) // When no selection, only affect the paragraph at start
    } else {
        resolve_position(paragraphs, end)
    };

    for idx in start_para..=end_para.min(paragraphs.len() - 1) {
        if let Some(para) = paragraphs.get_mut(idx) {
            para.alignment = alignment;
        }
    }
}

pub fn is_uniformly_active(paragraphs: &[Paragraph], start: usize, end: usize, op: &FormatOp) -> bool {
    if start >= end { return false; }
    let mut cumulative = 0usize;
    let mut touched_any = false;
    for para in paragraphs {
        for run in &para.runs {
            let run_start = cumulative;
            let run_end = cumulative + run.text.len();
            cumulative = run_end;
            let overlap_start = run_start.max(start);
            let overlap_end = run_end.min(end);
            if overlap_start >= overlap_end { continue; }
            touched_any = true;
            let active = match op {
                FormatOp::Bold(true) => run.bold,
                FormatOp::Italic(true) => run.italic,
                FormatOp::Underline(true) => run.underline,
                FormatOp::DoubleUnderline(true) => run.double_underline,
                FormatOp::Strikethrough(true) => run.strikethrough,
                FormatOp::Highlight(Some(color)) => run.highlight && run.highlight_color == *color,
                FormatOp::Box(true) => run.box_format,
                _ => false,
            };
            if !active { return false; }
        }
        cumulative += 1; // the paragraph-separating '\n'
    }
    touched_any
}

/// The "turn it back off" counterpart to an "on" `FormatOp`, used once
/// `is_uniformly_active` confirms the whole selection is already in that
/// state. Only meaningful for the four togglable ops above.
pub fn toggled_off(op: &FormatOp) -> FormatOp {
    match op {
        FormatOp::Bold(_) => FormatOp::Bold(false),
        FormatOp::Italic(_) => FormatOp::Italic(false),
        FormatOp::Underline(_) => FormatOp::Underline(false),
        FormatOp::DoubleUnderline(_) => FormatOp::DoubleUnderline(false),
        FormatOp::Highlight(_) => FormatOp::Highlight(None),
        FormatOp::Box(_) => FormatOp::Box(false),
        other => other.clone(),
    }
}

pub(crate) fn apply_format_op(run: &mut Run, op: &FormatOp) {
    match op {
        FormatOp::Bold(b) => run.bold = *b,
        FormatOp::Italic(b) => run.italic = *b,
        FormatOp::Underline(b) => run.underline = *b,
        FormatOp::DoubleUnderline(b) => run.double_underline = *b,
        FormatOp::Strikethrough(b) => run.strikethrough = *b,
        FormatOp::Highlight(color) => {
            run.highlight = color.is_some();
            run.highlight_color = color.clone().unwrap_or_default();
        }
        FormatOp::FontSize(size) => run.size = *size,
        FormatOp::FontFamily(font) => run.font = font.clone(),
        FormatOp::Color(color) => run.color = color.clone(),
        FormatOp::Box(b) => run.box_format = *b,
        FormatOp::ClearAll { default_size } => {
            run.bold = false;
            run.italic = false;
            run.underline = false;
            run.double_underline = false;
            run.strikethrough = false;
            run.highlight = false;
            run.highlight_color = String::new();
            run.size = *default_size;
            run.font = None;
            run.color = None;
            run.box_format = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx_parser::Run;

    fn run(text: &str) -> Run {
        Run { text: text.to_string(), ..Run::default() }
    }

    fn para(runs: Vec<Run>) -> Paragraph {
        Paragraph { runs, heading: 0, alignment: Alignment::default(), unsupported_xml: None }
    }

    #[test]
    fn test_resolves_start_of_first_run() {
        let paragraphs = vec![para(vec![run("hello")])];
        assert_eq!(resolve_position(&paragraphs, 0), (0, 0, 0));
    }

    #[test]
    fn test_resolves_middle_of_single_run() {
        let paragraphs = vec![para(vec![run("hello")])];
        assert_eq!(resolve_position(&paragraphs, 3), (0, 0, 3));
    }

    #[test]
    fn test_resolves_across_multiple_runs_in_one_paragraph() {
        // "foo" (0..3) + "bar" (3..6)
        let paragraphs = vec![para(vec![run("foo"), run("bar")])];
        assert_eq!(resolve_position(&paragraphs, 4), (0, 1, 1)); // 'a' in "bar"
    }

    #[test]
    fn test_boundary_between_two_runs_resolves_to_end_of_earlier_run() {
        let paragraphs = vec![para(vec![run("foo"), run("bar")])];
        assert_eq!(resolve_position(&paragraphs, 3), (0, 0, 3));
    }

    #[test]
    fn test_boundary_between_two_paragraphs_resolves_to_end_of_earlier_paragraph() {
        // content = "one\ntwo" -> "one" is bytes 0..3, '\n' at 3, "two" at 4..7
        let paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        assert_eq!(resolve_position(&paragraphs, 3), (0, 0, 3));
    }

    #[test]
    fn test_start_of_second_paragraph() {
        let paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        assert_eq!(resolve_position(&paragraphs, 4), (1, 0, 0));
    }

    #[test]
    fn test_middle_of_second_paragraph() {
        let paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        assert_eq!(resolve_position(&paragraphs, 6), (1, 0, 2));
    }

    #[test]
    fn test_end_of_last_paragraph() {
        let paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        assert_eq!(resolve_position(&paragraphs, 7), (1, 0, 3));
    }

    #[test]
    fn test_offset_past_end_of_document_clamps_to_last_position() {
        let paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        assert_eq!(resolve_position(&paragraphs, 999), (1, 0, 3));
    }

    #[test]
    fn test_empty_paragraph_between_two_others() {
        // "one\n\ntwo": "one" 0..3, '\n' at 3, empty para at 4..4, '\n' at 4, "two" 5..8
        let paragraphs = vec![
            para(vec![run("one")]),
            para(vec![]),
            para(vec![run("two")]),
        ];
        assert_eq!(resolve_position(&paragraphs, 4), (1, 0, 0));
        assert_eq!(resolve_position(&paragraphs, 5), (2, 0, 0));
    }

    // ── sync_insert_char / sync_insert_str ───────────────────────────────────

    #[test]
    fn test_insert_char_inherits_surrounding_runs_format() {
        let mut paragraphs = vec![para(vec![Run { text: "abc".into(), bold: true, ..Run::default() }])];
        sync_insert_char(&mut paragraphs, 1, 'X');
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "aXbc");
        assert!(paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_insert_str_multiple_chars_into_one_run() {
        let mut paragraphs = vec![para(vec![run("ac")])];
        sync_insert_str(&mut paragraphs, 1, "XYZ");
        assert_eq!(paragraphs[0].runs[0].text, "aXYZc");
    }

    #[test]
    fn test_insert_newline_splits_paragraph_into_two() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        sync_insert_char(&mut paragraphs, 2, '\n');
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "he");
        assert_eq!(paragraphs[1].runs[0].text, "llo");
    }

    #[test]
    fn test_insert_newline_mid_run_preserves_format_on_both_sides() {
        let mut paragraphs = vec![para(vec![Run { text: "hello".into(), bold: true, ..Run::default() }])];
        sync_insert_char(&mut paragraphs, 2, '\n');
        assert!(paragraphs[0].runs[0].bold);
        assert!(paragraphs[1].runs[0].bold);
    }

    #[test]
    fn test_insert_newline_splits_paragraph_across_multiple_runs() {
        let mut paragraphs = vec![para(vec![run("foo"), run("bar")])];
        sync_insert_char(&mut paragraphs, 4, '\n'); // splits inside "bar", after 'b'
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "foo");
        assert_eq!(paragraphs[0].runs[1].text, "b");
        assert_eq!(paragraphs[1].runs[0].text, "ar");
    }

    #[test]
    fn test_insert_newline_at_end_of_heading_line_resets_new_paragraph_to_plain() {
        // A Pocket-styled line (heading 1, bold, sized, boxed, centered) —
        // pressing Enter at its end should start a plain body-text line,
        // not continue looking like a Pocket.
        let heading_line = Paragraph {
            runs: vec![Run { text: "hello".into(), bold: true, size: 52, box_format: true, ..Run::default() }],
            heading: 1,
            alignment: Alignment::Center,
            unsupported_xml: None,
        };
        let mut paragraphs = vec![heading_line];
        sync_insert_char(&mut paragraphs, 5, '\n'); // cursor at end of "hello"

        assert_eq!(paragraphs.len(), 2);
        // Original line keeps its heading formatting untouched.
        assert_eq!(paragraphs[0].heading, 1);
        assert!(paragraphs[0].runs[0].bold);
        // New line is plain: no heading, no inherited run formatting, left-aligned.
        assert_eq!(paragraphs[1].heading, 0);
        assert_eq!(paragraphs[1].alignment, Alignment::Left);
        assert!(!paragraphs[1].runs[0].bold);
        assert_eq!(paragraphs[1].runs[0].size, 0);
        assert!(!paragraphs[1].runs[0].box_format);
    }

    #[test]
    fn test_insert_newline_mid_heading_line_resets_trailing_text_to_plain() {
        // Splitting in the middle of a Hat-styled line: the trailing half
        // that moves to the new paragraph loses the Hat formatting too,
        // matching Word's "Enter inside a heading reverts to body style".
        let heading_line = Paragraph {
            runs: vec![Run { text: "hello world".into(), bold: true, size: 44, double_underline: true, ..Run::default() }],
            heading: 2,
            alignment: Alignment::Center,
            unsupported_xml: None,
        };
        let mut paragraphs = vec![heading_line];
        sync_insert_char(&mut paragraphs, 5, '\n'); // split after "hello"

        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "hello");
        assert!(paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].heading, 2);

        assert_eq!(paragraphs[1].runs[0].text, " world");
        assert_eq!(paragraphs[1].heading, 0);
        assert_eq!(paragraphs[1].alignment, Alignment::Left);
        assert!(!paragraphs[1].runs[0].bold);
        assert!(!paragraphs[1].runs[0].double_underline);
        assert_eq!(paragraphs[1].runs[0].size, 0);
    }

    #[test]
    fn test_insert_str_with_embedded_newline_splits_paragraphs() {
        let mut paragraphs = vec![para(vec![run("ac")])];
        sync_insert_str(&mut paragraphs, 1, "X\nY");
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "aX");
        assert_eq!(paragraphs[1].runs[0].text, "Yc");
    }

    // ── sync_delete_range ────────────────────────────────────────────────────

    #[test]
    fn test_delete_within_single_run() {
        let mut paragraphs = vec![para(vec![run("hello world")])];
        sync_delete_range(&mut paragraphs, 5, 11);
        assert_eq!(paragraphs[0].runs[0].text, "hello");
    }

    #[test]
    fn test_delete_preserves_formatting_outside_deleted_range() {
        let mut paragraphs = vec![para(vec![
            Run { text: "bold".into(), bold: true, ..Run::default() },
            run(" plain"),
        ])];
        // delete " plai" (indices 4..9), leaving "bold" + "n"
        sync_delete_range(&mut paragraphs, 4, 9);
        assert_eq!(paragraphs[0].runs[0].text, "bold");
        assert!(paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[1].text, "n");
        assert!(!paragraphs[0].runs[1].bold);
    }

    #[test]
    fn test_delete_spanning_multiple_runs_in_one_paragraph_removes_middle_run() {
        let mut paragraphs = vec![para(vec![run("foo"), run("bar"), run("baz")])];
        // delete from middle of "foo" (offset 2) through middle of "baz" (offset 7)
        // "foobarbaz": f-o-o-b-a-r-b-a-z indices 0..9; delete [2,7) = "obarb".
        // The remaining "fo" and "az" pieces share identical (default,
        // unformatted) styling, so they merge into one run.
        sync_delete_range(&mut paragraphs, 2, 7);
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "foaz");
    }

    #[test]
    fn test_delete_across_paragraph_boundary_merges_paragraphs() {
        let mut paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        // delete the trailing "e" of "one", the '\n', and leading "t" of "two":
        // content = "one\ntwo", delete [2, 5) -> merges into "on" + "wo" = "onwo"
        sync_delete_range(&mut paragraphs, 2, 5);
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "onwo");
    }

    #[test]
    fn test_delete_whole_line_including_newline_merges_with_next() {
        let mut paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        // dd-style: delete [0, 4) = "one\n" entirely
        sync_delete_range(&mut paragraphs, 0, 4);
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "two");
    }

    #[test]
    fn test_delete_middle_paragraph_entirely_removes_it() {
        let mut paragraphs = vec![
            para(vec![run("one")]),
            para(vec![run("two")]),
            para(vec![run("three")]),
        ];
        // content = "one\ntwo\nthree"; delete [3, 8) = "\ntwo\n" exactly —
        // both separating newlines are consumed, so "one" and "three" end
        // up on the very same (now single) paragraph, not just adjacent ones.
        sync_delete_range(&mut paragraphs, 3, 8);
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "onethree");
    }

    #[test]
    fn test_delete_never_leaves_zero_runs() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        sync_delete_range(&mut paragraphs, 0, 5);
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "");
    }

    #[test]
    fn test_delete_noop_when_start_equals_end() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        sync_delete_range(&mut paragraphs, 2, 2);
        assert_eq!(paragraphs[0].runs[0].text, "hello");
    }

    // ── paragraph_run_char_spans ─────────────────────────────────────────────

    #[test]
    fn test_char_spans_single_run() {
        let p = para(vec![run("hello")]);
        assert_eq!(paragraph_run_char_spans(&p), vec![(0, 5, 0)]);
    }

    #[test]
    fn test_char_spans_multiple_runs() {
        let p = para(vec![run("foo"), run("bar")]);
        assert_eq!(paragraph_run_char_spans(&p), vec![(0, 3, 0), (3, 6, 1)]);
    }

    #[test]
    fn test_char_spans_empty_paragraph() {
        let p = para(vec![]);
        assert_eq!(paragraph_run_char_spans(&p), vec![]);
    }

    #[test]
    fn test_char_spans_use_char_count_not_byte_len_for_multibyte_text() {
        // "café" is 4 chars but 5 bytes (é is 2 bytes in UTF-8).
        let p = para(vec![Run { text: "café".into(), ..Run::default() }, run("bar")]);
        assert_eq!(paragraph_run_char_spans(&p), vec![(0, 4, 0), (4, 7, 1)]);
    }

    // ── apply_formatting ─────────────────────────────────────────────────────

    #[test]
    fn test_apply_bold_to_whole_single_run() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        apply_formatting(&mut paragraphs, 0, 5, FormatOp::Bold(true));
        assert!(paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[0].text, "hello");
    }

    #[test]
    fn test_apply_bold_to_trailing_word_splits_run_in_two() {
        let mut paragraphs = vec![para(vec![run("hello world")])];
        apply_formatting(&mut paragraphs, 6, 11, FormatOp::Bold(true)); // "world"
        assert_eq!(paragraphs[0].runs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "hello ");
        assert!(!paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[1].text, "world");
        assert!(paragraphs[0].runs[1].bold);
    }

    #[test]
    fn test_apply_bold_to_interior_range_splits_into_three_runs() {
        let mut paragraphs = vec![para(vec![run("one two three")])];
        apply_formatting(&mut paragraphs, 4, 7, FormatOp::Bold(true)); // "two"
        assert_eq!(paragraphs[0].runs.len(), 3);
        assert_eq!(paragraphs[0].runs[0].text, "one ");
        assert!(!paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[1].text, "two");
        assert!(paragraphs[0].runs[1].bold);
        assert_eq!(paragraphs[0].runs[2].text, " three");
        assert!(!paragraphs[0].runs[2].bold);
    }

    #[test]
    fn test_apply_formatting_across_multiple_runs_in_one_paragraph() {
        let mut paragraphs = vec![para(vec![run("foo"), run("bar")])];
        apply_formatting(&mut paragraphs, 1, 5, FormatOp::Italic(true)); // "oob" + "a" of "bar" -> spans both runs partially
        // "foobar": f-o-o-b-a-r, italic [1,5) = "ooba"
        assert_eq!(paragraphs[0].runs.len(), 3);
        assert_eq!(paragraphs[0].runs[0].text, "f");
        assert!(!paragraphs[0].runs[0].italic);
        assert_eq!(paragraphs[0].runs[1].text, "ooba");
        assert!(paragraphs[0].runs[1].italic);
        assert_eq!(paragraphs[0].runs[2].text, "r");
        assert!(!paragraphs[0].runs[2].italic);
    }

    #[test]
    fn test_apply_formatting_spanning_multiple_paragraphs() {
        let mut paragraphs = vec![para(vec![run("one")]), para(vec![run("two")])];
        // content = "one\ntwo"; bold [1, 6) = "ne" + '\n' skipped + "tw"
        apply_formatting(&mut paragraphs, 1, 6, FormatOp::Bold(true));
        assert_eq!(paragraphs[0].runs.len(), 2);
        assert_eq!(paragraphs[0].runs[0].text, "o");
        assert!(!paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[1].text, "ne");
        assert!(paragraphs[0].runs[1].bold);
        assert_eq!(paragraphs[1].runs.len(), 2);
        assert_eq!(paragraphs[1].runs[0].text, "tw");
        assert!(paragraphs[1].runs[0].bold);
        assert_eq!(paragraphs[1].runs[1].text, "o");
        assert!(!paragraphs[1].runs[1].bold);
    }

    #[test]
    fn test_apply_formatting_merges_adjacent_same_format_runs() {
        // Two already-bold runs with a plain run between them; bolding the
        // plain run's exact range should merge all three into one.
        let mut paragraphs = vec![para(vec![
            Run { text: "one".into(), bold: true, ..Run::default() },
            run("two"),
            Run { text: "three".into(), bold: true, ..Run::default() },
        ])];
        apply_formatting(&mut paragraphs, 3, 6, FormatOp::Bold(true));
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "onetwothree");
        assert!(paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_apply_highlight_sets_color_name() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        apply_formatting(&mut paragraphs, 0, 5, FormatOp::Highlight(Some("yellow".to_string())));
        assert!(paragraphs[0].runs[0].highlight);
        assert_eq!(paragraphs[0].runs[0].highlight_color, "yellow");
    }

    #[test]
    fn test_apply_highlight_none_removes_it() {
        let mut paragraphs = vec![para(vec![Run {
            text: "hello".into(), highlight: true, highlight_color: "yellow".into(), ..Run::default()
        }])];
        apply_formatting(&mut paragraphs, 0, 5, FormatOp::Highlight(None));
        assert!(!paragraphs[0].runs[0].highlight);
    }

    #[test]
    fn test_apply_clear_all_resets_every_field() {
        let mut paragraphs = vec![para(vec![Run {
            text: "hello".into(),
            bold: true,
            italic: true,
            underline: true,
            highlight: true,
            highlight_color: "green".into(),
            size: 24,
            font: Some("Georgia".into()),
            color: Some("FF0000".into()),
            ..Run::default()
        }])];
        apply_formatting(&mut paragraphs, 0, 5, FormatOp::ClearAll { default_size: 22 });
        let r = &paragraphs[0].runs[0];
        assert!(!r.bold && !r.italic && !r.underline && !r.highlight);
        assert_eq!(r.size, 22);
        assert_eq!(r.font, None);
        assert_eq!(r.color, None);
    }

    #[test]
    fn test_apply_formatting_noop_when_start_equals_end() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        apply_formatting(&mut paragraphs, 2, 2, FormatOp::Bold(true));
        assert!(!paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs.len(), 1);
    }

    #[test]
    fn test_apply_font_family_and_color() {
        let mut paragraphs = vec![para(vec![run("hello")])];
        apply_formatting(&mut paragraphs, 0, 5, FormatOp::FontFamily(Some("Georgia".to_string())));
        apply_formatting(&mut paragraphs, 0, 5, FormatOp::Color(Some("00FF00".to_string())));
        assert_eq!(paragraphs[0].runs[0].font, Some("Georgia".to_string()));
        assert_eq!(paragraphs[0].runs[0].color, Some("00FF00".to_string()));
    }

    // ── is_uniformly_active / toggled_off ───────────────────────────────────

    #[test]
    fn test_is_uniformly_active_true_when_whole_range_already_bold() {
        let paragraphs = vec![para(vec![Run { text: "hello".into(), bold: true, ..Run::default() }])];
        assert!(is_uniformly_active(&paragraphs, 0, 5, &FormatOp::Bold(true)));
    }

    #[test]
    fn test_is_uniformly_active_false_when_only_part_is_bold() {
        let paragraphs = vec![para(vec![
            Run { text: "hel".into(), bold: true, ..Run::default() },
            run("lo"),
        ])];
        assert!(!is_uniformly_active(&paragraphs, 0, 5, &FormatOp::Bold(true)));
    }

    #[test]
    fn test_is_uniformly_active_false_when_nothing_is_bold() {
        let paragraphs = vec![para(vec![run("hello")])];
        assert!(!is_uniformly_active(&paragraphs, 0, 5, &FormatOp::Bold(true)));
    }

    #[test]
    fn test_is_uniformly_active_checks_highlight_color_match() {
        let paragraphs = vec![para(vec![Run {
            text: "hello".into(), highlight: true, highlight_color: "yellow".into(), ..Run::default()
        }])];
        assert!(is_uniformly_active(&paragraphs, 0, 5, &FormatOp::Highlight(Some("yellow".into()))));
        // Same range is highlighted, but a *different* color — clicking the
        // green button on yellow-highlighted text should apply green, not
        // toggle it off.
        assert!(!is_uniformly_active(&paragraphs, 0, 5, &FormatOp::Highlight(Some("green".into()))));
    }

    #[test]
    fn test_is_uniformly_active_spans_multiple_paragraphs() {
        let paragraphs = vec![
            para(vec![Run { text: "one".into(), italic: true, ..Run::default() }]),
            para(vec![Run { text: "two".into(), italic: true, ..Run::default() }]),
        ];
        // "one\ntwo", italic [0, 7)
        assert!(is_uniformly_active(&paragraphs, 0, 7, &FormatOp::Italic(true)));
    }

    #[test]
    fn test_is_uniformly_active_false_for_non_togglable_ops() {
        let paragraphs = vec![para(vec![run("hello")])];
        assert!(!is_uniformly_active(&paragraphs, 0, 5, &FormatOp::FontSize(24)));
        assert!(!is_uniformly_active(&paragraphs, 0, 5, &FormatOp::ClearAll { default_size: 22 }));
    }

    #[test]
    fn test_toggled_off_maps_each_togglable_op() {
        assert_eq!(toggled_off(&FormatOp::Bold(true)), FormatOp::Bold(false));
        assert_eq!(toggled_off(&FormatOp::Italic(true)), FormatOp::Italic(false));
        assert_eq!(toggled_off(&FormatOp::Underline(true)), FormatOp::Underline(false));
        assert_eq!(
            toggled_off(&FormatOp::Highlight(Some("yellow".into()))),
            FormatOp::Highlight(None)
        );
    }

    // ── unsupported_xml invalidation on edit ────────────────────────────────

    #[test]
    fn test_sync_insert_char_clears_unsupported_xml_on_touched_paragraph() {
        let mut paragraphs = vec![para(vec![Run { text: "hi".into(), ..Run::default() }])];
        paragraphs[0].unsupported_xml = Some("<w:hyperlink/>".to_string());

        sync_insert_char(&mut paragraphs, 1, 'X');

        assert_eq!(paragraphs[0].unsupported_xml, None);
    }

    #[test]
    fn test_sync_delete_range_clears_unsupported_xml_on_touched_paragraph() {
        let mut paragraphs = vec![para(vec![Run { text: "hello".into(), ..Run::default() }])];
        paragraphs[0].unsupported_xml = Some("<w:hyperlink/>".to_string());

        sync_delete_range(&mut paragraphs, 1, 3);

        assert_eq!(paragraphs[0].unsupported_xml, None);
    }

    #[test]
    fn test_apply_formatting_clears_unsupported_xml_only_on_touched_paragraphs() {
        let mut paragraphs = vec![
            para(vec![Run { text: "one".into(), ..Run::default() }]),
            para(vec![Run { text: "two".into(), ..Run::default() }]),
        ];
        paragraphs[0].unsupported_xml = Some("<w:hyperlink/>".to_string());
        paragraphs[1].unsupported_xml = Some("<w:hyperlink/>".to_string());

        // "one\ntwo" - byte 0..3 is entirely within paragraph 0 only.
        apply_formatting(&mut paragraphs, 0, 3, FormatOp::Bold(true));

        assert_eq!(paragraphs[0].unsupported_xml, None);
        assert_eq!(paragraphs[1].unsupported_xml, Some("<w:hyperlink/>".to_string()));
    }
}
