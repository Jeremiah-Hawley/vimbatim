use super::*;

pub(super) fn clamp_to_char_boundary(content: &str, byte: usize) -> usize {
    /*
     * Clamps an arbitrary byte offset (e.g. a cursor position carried over
     * from before an undo/redo swapped in different content) to `content`'s
     * length and onto the nearest valid UTF-8 char boundary at or before it
     * — the offset may point past the end of the new content, or land
     * mid-character if the swap changed what's at that byte position.
     */
    let byte = byte.min(content.len());
    if content.is_char_boundary(byte) {
        byte
    } else {
        (0..byte)
            .rev()
            .find(|&i| content.is_char_boundary(i))
            .unwrap_or(0)
    }
}

pub(super) fn char_left(content: &str, cursor: usize) -> usize {
    /*
     * Returns the previous character boundary before `cursor`, clamped at 0.
     * Shared by `move_left` (clears selection) and `extend_left` (extends
     * it) so the two stay in lockstep by construction.
     */
    if cursor == 0 {
        return 0;
    }
    content[..cursor]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

pub(super) fn char_right(content: &str, cursor: usize) -> usize {
    /*
     * Returns the next character boundary after `cursor`, clamped at
     * `content.len()`.
     */
    if cursor >= content.len() {
        return content.len();
    }
    content[cursor..]
        .char_indices()
        .nth(1)
        .map(|(i, _)| cursor + i)
        .unwrap_or(content.len())
}

pub(super) fn line_down(content: &str, cursor: usize) -> usize {
    /*
     * Returns the byte offset at the same character column on the line
     * after `cursor`'s line, clamped to that line's length. Returns
     * `cursor` unchanged (no-op) when already on the last line.
     */
    let start = line_start(content, cursor);
    let end = line_end(content, cursor);
    if end >= content.len() {
        return cursor;
    } // last line, nothing below
    let col = content[start..cursor].chars().count();
    let next_start = end + 1; // skip the '\n'
    let next_end = line_end(content, next_start);
    byte_offset_for_col(&content[next_start..next_end], col) + next_start
}

pub(super) fn line_up(content: &str, cursor: usize) -> usize {
    /*
     * Returns the byte offset at the same character column on the line
     * before `cursor`'s line, clamped to that line's length. Returns
     * `cursor` unchanged (no-op) when already on the first line.
     */
    let start = line_start(content, cursor);
    if start == 0 {
        return cursor;
    } // first line, nothing above
    let col = content[start..cursor].chars().count();
    let prev_end = start - 1; // the '\n' ending the previous line
    let prev_start = line_start(content, prev_end);
    byte_offset_for_col(&content[prev_start..prev_end], col) + prev_start
}

pub(super) fn line_start(content: &str, pos: usize) -> usize {
    /*
     * Returns the byte offset of the start of the line containing `pos` —
     * the char immediately after the preceding '\n', or 0 for the first
     * line.
     */
    content[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

pub(super) fn line_end(content: &str, pos: usize) -> usize {
    /*
     * Returns the byte offset of the end of the line containing `pos` — the
     * index of the '\n' that ends it, or `content.len()` for the last line.
     */
    content[pos..]
        .find('\n')
        .map(|i| pos + i)
        .unwrap_or(content.len())
}

pub(super) fn line_index_for(content: &str, pos: usize) -> usize {
    /*
     * 0-indexed line number containing byte offset `pos` — counts the
     * newlines before it. Used by `apply_vim_motion`'s jump-list push
     * heuristic (spec 5.5's `Ctrl+o`/`Ctrl+i`): a motion is "large" if it
     * crosses more than one line.
     */
    content[..line_start(content, pos)].matches('\n').count()
}

pub(super) fn first_nonblank(content: &str, pos: usize) -> usize {
    /*
     * Byte offset of the first non-whitespace character on the line
     * containing `pos` — vim's `^`. If the line is entirely whitespace,
     * returns the line's end instead (matching vim's `^` on a blank line).
     */
    let start = line_start(content, pos);
    let end = line_end(content, pos);
    content[start..end]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| start + i)
        .unwrap_or(end)
}

pub(super) fn underscore_motion(content: &str, pos: usize, count: usize) -> usize {
    /*
     * vim `_`: first non-blank character `count - 1` lines below the
     * current one — count defaults to 1 (via the caller), landing on the
     * current line's own first non-blank, the same target `^` reaches.
     * Clamps at the document's last line when the requested line doesn't
     * exist, rather than panicking or wrapping.
     */
    let mut start = line_start(content, pos);
    for _ in 0..count.saturating_sub(1) {
        let end = line_end(content, start);
        if end >= content.len() {
            break;
        }
        start = end + 1;
    }
    first_nonblank(content, start)
}

pub(super) fn operator_forces_linewise(operator: char) -> bool {
    /*
     * `>`/`<` are always linewise regardless of the motion/selection's
     * own kind (vim's own rule: `>w` indents the *line(s)* the motion
     * spans, even though `w` itself is charwise). `gU`/`gu` (this
     * codebase's `'U'`/`'u'` operator ids) are deliberately *not*
     * included: unlike `>`/`<`, vim's case-change operators respect the
     * motion's actual charwise/linewise nature (`gUw` uppercases just the
     * word). Shared by `vim_operator_motion_range` (Normal-mode
     * operator+motion) and `vim_visual_operator_range` (Visual-mode
     * operator+selection) so the two can't drift on this rule.
     */
    matches!(operator, '>' | '<')
}

pub(super) fn linewise_bounds_for_operator(
    operator: char,
    start: usize,
    end: usize,
    content: &str,
) -> (usize, usize) {
    /*
     * Given a linewise span's `start` (a line's own start) and `end` (the
     * *last* spanned line's own end, not yet including its newline),
     * returns the final `[start, end)` byte range: `c` (`cc`/`c_`/`cgg`/
     * `c`+any linewise motion) excludes the trailing newline — real vim's
     * linewise change empties the line(s) in place rather than deleting
     * them outright, so typed replacement text lands where the old
     * content was instead of merging onto a neighboring line — while
     * every other linewise operator includes it, fully removing the
     * line(s). Shared by `vim_operator_motion_range`,
     * `vim_operator_doubled_range`, and `vim_visual_operator_range` — all
     * three build a linewise range this same way and need the rule
     * applied identically.
     */
    if operator == 'c' {
        (start, end)
    } else if end < content.len() {
        (start, end + 1)
    } else {
        (start, end)
    }
}

pub(super) fn vim_operator_motion_range(
    operator: char,
    cursor: usize,
    target: usize,
    kind: MotionKind,
    content: &str,
) -> (usize, usize) {
    /*
     * Builds the `[start, end)` byte range an operator acts on from a
     * resolved motion's target and `MotionKind` (vim's own `:help
     * exclusive`/`:help inclusive`/`:help linewise`):
     *   - `ExclusiveChar`: `[min, max)` — the target itself excluded.
     *   - `InclusiveChar`: `[min, max]` — the character *at* the target
     *     included too (`char_right` advances one char boundary past it).
     *   - `Linewise`: whole lines from `min`'s line through `max`'s line —
     *     see `linewise_bounds_for_operator` for the trailing-newline rule.
     * `cursor`/`target` may be in either order (a backward motion like `b`
     * or `F` has `target < cursor`) — `min`/`max` normalizes that.
     * `kind` is overridden to `Linewise` for `>`/`<` regardless of the
     * motion's own kind — see `operator_forces_linewise`.
     */
    let kind = if operator_forces_linewise(operator) {
        MotionKind::Linewise
    } else {
        kind
    };
    let (min, max) = if cursor <= target {
        (cursor, target)
    } else {
        (target, cursor)
    };
    match kind {
        MotionKind::ExclusiveChar => (min, max),
        MotionKind::InclusiveChar => (min, char_right(content, max)),
        MotionKind::Linewise => {
            let start = line_start(content, min);
            let end = line_end(content, max);
            linewise_bounds_for_operator(operator, start, end, content)
        }
    }
}

pub(super) fn vim_operator_doubled_range(
    operator: char,
    cursor: usize,
    count: usize,
    content: &str,
) -> (usize, usize) {
    /*
     * Builds the linewise range for a doubled operator (`dd`/`yy`/`cc`)
     * spanning `count` lines starting at `cursor`'s line — the `[count]`
     * from `d2d`, or 1 for a bare `dd`. Same trailing-newline rule as
     * `vim_operator_motion_range`, via `linewise_bounds_for_operator`.
     */
    let start = line_start(content, cursor);
    let mut end_pos = cursor;
    for _ in 0..count.saturating_sub(1) {
        let line_end_pos = line_end(content, end_pos);
        if line_end_pos >= content.len() {
            break;
        }
        end_pos = line_end_pos + 1;
    }
    let end = line_end(content, end_pos);
    linewise_bounds_for_operator(operator, start, end, content)
}

pub(super) fn line_offset(content: &str, line_idx: usize) -> usize {
    /*
     * Returns the byte offset of the start of the given 0-indexed line
     * number, clamping to the start of the last line if `line_idx` is past
     * the end of the document.
     */
    let mut offset = 0;
    for _ in 0..line_idx {
        let end = line_end(content, offset);
        if end >= content.len() {
            break;
        } // no more lines; clamp here
        offset = end + 1;
    }
    offset
}

pub(super) fn byte_offset_for_line_col(content: &str, line: usize, col: usize) -> usize {
    /*
     * Maps a 0-indexed (line, char_column) pair to a byte offset into
     * `content`, clamping both the line number and the column to the
     * document's actual bounds. Shared by `set_cursor_from_line_col` and
     * `extend_selection_to_line_col` so plain-click and click-drag
     * positioning stay in lockstep by construction.
     */
    let start = line_offset(content, line);
    let end = line_end(content, start);
    byte_offset_for_col(&content[start..end], col) + start
}

pub(super) fn byte_offset_for_col(line: &str, col: usize) -> usize {
    /*
     * Maps a character column (not byte column) within a single line to a
     * byte offset relative to the start of that line, clamping to the
     * line's length when `col` exceeds the number of characters on the
     * line.
     */
    line.char_indices()
        .nth(col)
        .map(|(i, _)| i)
        .unwrap_or(line.len())
}

pub(super) fn split_vim_command_buf(buf: &str) -> (Option<usize>, Option<char>) {
    /*
     * Splits a Normal-mode command buffer (spec 5.2) into its leading
     * digit-count (if any) and a single trailing non-digit "pending
     * trigger" character (if the buffer ends mid-way through a
     * two-keystroke command like `g` awaiting a second `g`, or `f`/`F`/
     * `t`/`T` awaiting a target character). By construction the buffer is
     * always [digits]*[trigger]? — never digits *after* a trigger — so the
     * trigger, if present, is always the buffer's last character.
     */
    let trigger = buf.chars().last().filter(|c| !c.is_ascii_digit());
    let digit_part = match trigger {
        Some(t) => &buf[..buf.len() - t.len_utf8()],
        None => buf,
    };
    let count = if digit_part.is_empty() {
        None
    } else {
        digit_part.parse::<usize>().ok()
    };
    (count, trigger)
}

/// The three character classes vim's word motions distinguish: alphanumeric
/// "word" characters, standalone "punctuation" characters (each run of
/// punctuation is its own word), and whitespace (never part of a word).
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(super) enum CharClass {
    Word,
    Punct,
    Space,
}

pub(super) fn char_class(c: char) -> CharClass {
    /*
     * Classifies a single character for vim `w`/`b`/`e` word-motion
     * purposes: alnum/`_` is a "word" char, whitespace is its own class,
     * and everything else (punctuation) is a third class — each
     * punctuation run is treated as its own word, matching vim rather than
     * a naive whitespace-only split.
     */
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub(super) fn big_word_class(c: char) -> CharClass {
    /*
     * Classifies a character for vim `W`/`B`/`E` WORD-motion purposes: only
     * whitespace vs. non-whitespace matters — a WORD is any
     * whitespace-delimited run, punctuation included, unlike `char_class`'s
     * additional word/punctuation split. Never produces `CharClass::Punct`;
     * shares the enum with `char_class` purely so both can drive the same
     * `word_forward`/`word_end`/`word_backward` implementations.
     */
    if c.is_whitespace() {
        CharClass::Space
    } else {
        CharClass::Word
    }
}

pub(super) fn skip_whitespace(content: &str, from: usize) -> usize {
    /*
     * Returns the byte offset of the first non-whitespace character at or
     * after `from`, or `content.len()` if the rest of the document is
     * whitespace.
     */
    content[from..]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace())
        .map(|(i, _)| from + i)
        .unwrap_or(content.len())
}

pub(super) fn word_forward(content: &str, pos: usize) -> usize {
    // vim `w`.
    word_forward_classified(content, pos, char_class)
}

pub(super) fn word_forward_big(content: &str, pos: usize) -> usize {
    // vim `W`.
    word_forward_classified(content, pos, big_word_class)
}

pub(super) fn word_forward_classified(
    content: &str,
    pos: usize,
    classify: fn(char) -> CharClass,
) -> usize {
    /*
     * Byte offset of the start of the next word after `pos`, per `classify`
     * (`char_class` for vim's `w`, `big_word_class` for `W`). Skips the
     * rest of the current char-class run, then skips whitespace (crossing
     * newlines freely) to land on the first character of the following word.
     */
    if pos >= content.len() {
        return pos;
    }
    let start_class = classify(content[pos..].chars().next().unwrap());
    // Find where the current char-class run ends; if it runs to the end of
    // the document without changing class, idx stays at content.len().
    let mut idx = content.len();
    for (i, c) in content[pos..].char_indices() {
        if classify(c) != start_class {
            idx = pos + i;
            break;
        }
    }
    // If the run ended on a non-space char, that's the next word's start.
    // Otherwise (it ended on whitespace, or `pos` itself was whitespace)
    // skip forward to the next non-space char.
    if idx < content.len() && classify(content[idx..].chars().next().unwrap()) != CharClass::Space {
        return idx;
    }
    skip_whitespace(content, idx)
}

pub(super) fn word_end(content: &str, pos: usize) -> usize {
    // vim `e`.
    word_end_classified(content, pos, char_class)
}

pub(super) fn word_end_big(content: &str, pos: usize) -> usize {
    // vim `E`.
    word_end_classified(content, pos, big_word_class)
}

pub(super) fn word_end_classified(
    content: &str,
    pos: usize,
    classify: fn(char) -> CharClass,
) -> usize {
    /*
     * Byte offset of the last character of the current word (if the cursor
     * isn't already there) or of the next word (if it is), per `classify`.
     */
    if pos >= content.len() {
        return pos;
    }
    let cur_char = content[pos..].chars().next().unwrap();
    let cur_class = classify(cur_char);
    let next_idx = pos + cur_char.len_utf8();
    let next_class =
        (next_idx < content.len()).then(|| classify(content[next_idx..].chars().next().unwrap()));
    // "At a word's end" means the cursor is on whitespace, or the next char
    // starts a different class's run — in either case there's nowhere left
    // to advance within the current word, so jump to the next word instead.
    let at_word_end =
        cur_class == CharClass::Space || next_class.map(|c| c != cur_class).unwrap_or(true);

    let i = if at_word_end {
        let skip_from = if cur_class == CharClass::Space {
            pos
        } else {
            next_idx
        };
        skip_whitespace(content, skip_from)
    } else {
        next_idx
    };
    if i >= content.len() {
        return content.len();
    }

    // Walk forward through the run starting at `i`, tracking the byte
    // offset of its last character (not the byte just past it).
    let run_class = classify(content[i..].chars().next().unwrap());
    let mut last = i;
    for (off, c) in content[i..].char_indices() {
        if classify(c) != run_class {
            break;
        }
        last = i + off;
    }
    last
}

pub(super) fn word_backward(content: &str, pos: usize) -> usize {
    // vim `b`.
    word_backward_classified(content, pos, char_class)
}

pub(super) fn word_backward_big(content: &str, pos: usize) -> usize {
    // vim `B`.
    word_backward_classified(content, pos, big_word_class)
}

pub(super) fn word_backward_classified(
    content: &str,
    pos: usize,
    classify: fn(char) -> CharClass,
) -> usize {
    /*
     * Byte offset of the start of the current word (if the cursor is
     * mid-word) or of the previous word (if it's at a word's start
     * already), per `classify`.
     */
    if pos == 0 {
        return 0;
    }
    // Step back one char boundary first — vim's `b` always looks at the
    // word before the cursor, even if the cursor already sits on a word's
    // first character.
    let mut i = content[..pos]
        .char_indices()
        .last()
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    // Skip backward over any whitespace between the cursor and the
    // preceding word.
    loop {
        let c = content[i..].chars().next().unwrap();
        if !c.is_whitespace() {
            break;
        }
        if i == 0 {
            return 0;
        }
        i = content[..i]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0);
    }
    // Walk backward while the previous char shares this run's class, to
    // find the start of the run `i` landed in.
    let class = classify(content[i..].chars().next().unwrap());
    loop {
        if i == 0 {
            break;
        }
        let prev = content[..i]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        if classify(content[prev..].chars().next().unwrap()) != class {
            break;
        }
        i = prev;
    }
    i
}

pub(super) fn is_blank_line(content: &str, line_start_pos: usize) -> bool {
    /*
     * True when the line starting at `line_start_pos` has zero characters
     * before its terminating '\n' (or the document's end) — vim's
     * paragraph-boundary definition (spec 5.2's `{`/`}`).
     */
    line_start_pos == line_end(content, line_start_pos)
}

pub(super) fn paragraph_forward(content: &str, pos: usize) -> usize {
    /*
     * vim `}`: byte offset of the start of the next blank line after
     * `pos`'s line, or `content.len()` if there is none. Always searches
     * strictly *after* the current line, even when the cursor already sits
     * on a blank line — `}` never stays put, it advances to a *later*
     * paragraph boundary.
     */
    let mut end = line_end(content, pos);
    loop {
        if end >= content.len() {
            return content.len();
        }
        let next_start = end + 1; // skip the '\n'
        if is_blank_line(content, next_start) {
            return next_start;
        }
        end = line_end(content, next_start);
    }
}

pub(super) fn paragraph_backward(content: &str, pos: usize) -> usize {
    /*
     * vim `{`: byte offset of the start of the previous blank line before
     * `pos`'s line, or `0` if there is none. Always searches strictly
     * *before* the current line, mirroring `paragraph_forward`.
     */
    let mut start = line_start(content, pos);
    loop {
        if start == 0 {
            return 0;
        }
        let prev_end = start - 1; // the '\n' ending the previous line
        let prev_start = line_start(content, prev_end);
        if is_blank_line(content, prev_start) {
            return prev_start;
        }
        start = prev_start;
    }
}

// ── Text objects (spec 5.4): iw/aw, is/as, ip/ap, i"/a", i'/a', brackets ────────

pub(super) fn resolve_vim_text_object(
    content: &str,
    cursor: usize,
    object_char: char,
    inner: bool,
) -> Option<(usize, usize)> {
    /*
     * Dispatches a resolved object character (already disambiguated from
     * the raw keystroke via `vim_find_target_char` by the caller) to its
     * resolver. `(`/`)` share one bracket pair, likewise `[`/`]` and
     * `{`/`}` — pressing either half of the pair selects the same
     * enclosing region, matching real vim.
     */
    match object_char {
        'w' => Some(text_object_word(content, cursor, inner)),
        's' => text_object_sentence(content, cursor, inner),
        'p' => text_object_paragraph(content, cursor, inner),
        '"' => text_object_quote(content, cursor, '"', inner),
        '\'' => text_object_quote(content, cursor, '\'', inner),
        '(' | ')' => text_object_bracket(content, cursor, '(', ')', inner),
        '[' | ']' => text_object_bracket(content, cursor, '[', ']', inner),
        '{' | '}' => text_object_bracket(content, cursor, '{', '}', inner),
        _ => None,
    }
}

pub(super) fn char_class_run_start(content: &str, cursor: usize, class: CharClass) -> usize {
    /*
     * Byte offset of the start of the contiguous run of `class`-classified
     * characters containing `cursor`, scanning backward. `cursor` itself
     * must already be within such a run (the caller checks this via
     * `char_class` on the character at `cursor`).
     */
    let mut start = cursor;
    for (i, c) in content[..cursor].char_indices().rev() {
        if char_class(c) != class {
            break;
        }
        start = i;
    }
    start
}

pub(super) fn char_class_run_end(content: &str, cursor: usize, class: CharClass) -> usize {
    /*
     * Exclusive byte offset just past the contiguous run of
     * `class`-classified characters containing `cursor`, scanning forward.
     */
    let mut end = cursor;
    for (i, c) in content[cursor..].char_indices() {
        if char_class(c) != class {
            break;
        }
        end = cursor + i + c.len_utf8();
    }
    end
}

pub(super) fn text_object_word(content: &str, cursor: usize, inner: bool) -> (usize, usize) {
    /*
     * vim `iw`/`aw`. `iw`: the contiguous run of the same `CharClass` as
     * the character under the cursor (a word run, a punctuation run, or a
     * whitespace run — each is its own "word" for this purpose, matching
     * `w`/`b`/`e`'s own classification). `aw`: `iw`'s range plus one
     * adjacent whitespace run — trailing preferred, falling back to
     * leading when there's no trailing whitespace (e.g. cursor on the
     * last word of the document). At document end (nothing under the
     * cursor) degenerates to a zero-width object at `cursor`.
     */
    let Some(ch) = content[cursor.min(content.len())..].chars().next() else {
        return (cursor, cursor);
    };
    let class = char_class(ch);
    let start = char_class_run_start(content, cursor, class);
    let end = char_class_run_end(content, cursor, class);
    if inner || class == CharClass::Space {
        // aw on whitespace itself just behaves like iw — there's no
        // "adjacent whitespace" to additionally swallow.
        return (start, end);
    }
    if end < content.len() && char_class(content[end..].chars().next().unwrap()) == CharClass::Space
    {
        (start, char_class_run_end(content, end, CharClass::Space))
    } else if start > 0
        && char_class(content[..start].chars().next_back().unwrap()) == CharClass::Space
    {
        (
            char_class_run_start(content, start - 1, CharClass::Space),
            end,
        )
    } else {
        (start, end)
    }
}

pub(super) fn is_sentence_end_punct(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

pub(super) fn text_object_sentence(
    content: &str,
    cursor: usize,
    inner: bool,
) -> Option<(usize, usize)> {
    /*
     * vim `is`/`as`, simplified: a sentence ends at the first `.`/`!`/`?`
     * followed by whitespace or end-of-content (no handling of
     * abbreviations, decimal numbers, or quote/paren-wrapped punctuation —
     * a documented simplification of vim's own, more elaborate sentence
     * grammar). `is` is the sentence containing `cursor`; `as` additionally
     * swallows the whitespace run up to the next sentence's start.
     */
    if content.is_empty() {
        return None;
    }
    let cursor = cursor.min(content.len());

    let mut end = None;
    for (i, c) in content[cursor..].char_indices() {
        if is_sentence_end_punct(c) {
            let after = cursor + i + c.len_utf8();
            let boundary = after >= content.len()
                || content[after..]
                    .chars()
                    .next()
                    .map(|c| c.is_whitespace())
                    .unwrap_or(true);
            if boundary {
                end = Some(after);
                break;
            }
        }
    }
    let end = end.unwrap_or(content.len());

    let mut start = 0;
    for (i, c) in content[..cursor].char_indices().rev() {
        if is_sentence_end_punct(c) {
            let after = i + c.len_utf8();
            let boundary = after >= content.len()
                || content[after..]
                    .chars()
                    .next()
                    .map(|c| c.is_whitespace())
                    .unwrap_or(true);
            if boundary && after <= cursor {
                start = skip_whitespace(content, after);
                break;
            }
        }
    }

    if inner {
        return Some((start, end));
    }
    Some((start, skip_whitespace(content, end)))
}

pub(super) fn paragraph_block_start(
    content: &str,
    from_line_start: usize,
    want_blank: bool,
) -> usize {
    /*
     * Scans backward from `from_line_start` (already a line-start
     * position) while the *preceding* line's blank/non-blank status
     * matches `want_blank`, returning the start of the earliest such
     * line — or `from_line_start` unchanged if the immediately preceding
     * line doesn't match (including "no preceding line", i.e. already at
     * the document start). Shared by `text_object_paragraph`'s `ip` scan
     * and `ap`'s leading-block fallback, which differ only in which
     * status they're matching.
     */
    let mut start = from_line_start;
    while start > 0 {
        let prev_end = start - 1;
        let prev_start = line_start(content, prev_end);
        if is_blank_line(content, prev_start) != want_blank {
            break;
        }
        start = prev_start;
    }
    start
}

pub(super) fn paragraph_block_end(content: &str, from_line_end: usize, want_blank: bool) -> usize {
    /*
     * Scans forward from `from_line_end` (already the end of a line, not
     * including its newline) while the *following* line's blank/non-blank
     * status matches `want_blank`, returning the end of the last such
     * line. Shared by `text_object_paragraph`'s `ip` scan and `ap`'s
     * trailing-block fallback.
     */
    let mut end = from_line_end;
    while end < content.len() {
        let next_start = end + 1;
        if is_blank_line(content, next_start) != want_blank {
            break;
        }
        end = line_end(content, next_start);
    }
    end
}

pub(super) fn text_object_paragraph(
    content: &str,
    cursor: usize,
    inner: bool,
) -> Option<(usize, usize)> {
    /*
     * vim `ip`/`ap`: a paragraph is a blank-line-delimited block (the same
     * definition `{`/`}` use, spec 5.2, via `is_blank_line`). `ip` is the
     * contiguous run of lines sharing the cursor line's blank/non-blank
     * status; `ap` additionally swallows one adjacent block of the
     * *opposite* status — trailing preferred, falling back to leading —
     * mirroring `aw`'s whitespace-inclusion rule at paragraph granularity.
     */
    if content.is_empty() {
        return None;
    }
    let cur_line_start = line_start(content, cursor);
    let blank = is_blank_line(content, cur_line_start);

    let mut start = paragraph_block_start(content, cur_line_start, blank);
    let block_end = paragraph_block_end(content, line_end(content, cur_line_start), blank);
    let mut end = if block_end < content.len() {
        block_end + 1
    } else {
        block_end
    };

    if !inner {
        if end < content.len() {
            let trail_end = paragraph_block_end(content, line_end(content, end), !blank);
            end = if trail_end < content.len() {
                trail_end + 1
            } else {
                trail_end
            };
        } else if start > 0 {
            start = paragraph_block_start(content, start, !blank);
        }
    }
    Some((start, end))
}

pub(super) fn text_object_quote(
    content: &str,
    cursor: usize,
    quote: char,
    inner: bool,
) -> Option<(usize, usize)> {
    /*
     * vim `i"`/`a"` (and `'`): scans the *current line only* (vim's own
     * quote objects never cross lines) for `quote` pairs, then picks the
     * first pair that contains or starts at/after `cursor`. `inner`
     * excludes both quote characters; `around` includes them.
     */
    let line_s = line_start(content, cursor);
    let line_e = line_end(content, cursor);
    let positions: Vec<usize> = content[line_s..line_e]
        .char_indices()
        .filter(|&(_, c)| c == quote)
        .map(|(i, _)| line_s + i)
        .collect();
    let mut i = 0;
    while i + 1 < positions.len() {
        let (open, close) = (positions[i], positions[i + 1]);
        if cursor <= close {
            return Some(if inner {
                (char_right(content, open), close)
            } else {
                (open, char_right(content, close))
            });
        }
        i += 2;
    }
    None
}

pub(super) fn text_object_bracket(
    content: &str,
    cursor: usize,
    open: char,
    close: char,
    inner: bool,
) -> Option<(usize, usize)> {
    /*
     * vim `i(`/`a(` (and `[`/`{`, either half of the pair): unlike quotes,
     * bracket objects search the *whole document* and are nesting-aware.
     * A single forward scan with a stack of open positions finds every
     * matched pair; among those enclosing `cursor` (inclusive of the
     * bracket characters themselves), the smallest one is the innermost
     * enclosing pair, matching real vim. Unmatched brackets (extra opens
     * left on the stack, or a stray close with an empty stack) are
     * ignored rather than erroring.
     */
    let mut stack: Vec<usize> = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    for (i, c) in content.char_indices() {
        if c == open {
            stack.push(i);
        } else if c == close {
            if let Some(open_i) = stack.pop() {
                if open_i <= cursor && cursor <= i {
                    best = match best {
                        Some((bs, be)) if (be - bs) <= (i - open_i) => Some((bs, be)),
                        _ => Some((open_i, i)),
                    };
                }
            }
        }
    }
    let (open_pos, close_pos) = best?;
    Some(if inner {
        (char_right(content, open_pos), close_pos)
    } else {
        (open_pos, char_right(content, close_pos))
    })
}

pub(super) fn find_char_forward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `f<char>`: byte offset of the next occurrence of `target` on the
     * current line, searching strictly after `pos`. `None` if the current
     * line has no later occurrence — `f`/`t` never cross a line boundary.
     */
    let end = line_end(content, pos);
    if pos >= end {
        return None;
    }
    let search_from = char_right(content, pos);
    content[search_from..end]
        .char_indices()
        .find(|(_, c)| *c == target)
        .map(|(i, _)| search_from + i)
}

pub(super) fn find_char_backward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `F<char>`: byte offset of the previous occurrence of `target` on
     * the current line, searching strictly before `pos`. `None` if not found.
     */
    let start = line_start(content, pos);
    content[start..pos]
        .char_indices()
        .rev()
        .find(|(_, c)| *c == target)
        .map(|(i, _)| start + i)
}

pub(super) fn till_char_forward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `t<char>`: byte offset one character before the next occurrence
     * of `target` on the current line. A no-op (returns `pos`, wrapped in
     * `Some`) when `target` is the character immediately after `pos` — vim's
     * `t` never lands past its own starting position.
     */
    find_char_forward(content, pos, target).map(|found| char_left(content, found))
}

pub(super) fn till_char_backward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `T<char>`: byte offset one character after the previous
     * occurrence of `target` on the current line.
     */
    find_char_backward(content, pos, target).map(|found| char_right(content, found))
}

pub(super) fn resolve_find(content: &str, pos: usize, kind: char, target: char) -> Option<usize> {
    /*
     * Dispatches to the right find-char function for `kind` (`f`/`F`/`t`/
     * `T`). Shared by the four `move_*` methods (which also remember the
     * find for `;`/`,`) and `AppState::apply_find`'s repeat path (which
     * doesn't).
     */
    match kind {
        'f' => find_char_forward(content, pos, target),
        'F' => find_char_backward(content, pos, target),
        't' => till_char_forward(content, pos, target),
        'T' => till_char_backward(content, pos, target),
        _ => None,
    }
}

pub(super) fn resolve_find_with_nudge(
    content: &str,
    cursor: usize,
    kind: char,
    target: char,
    nudge: bool,
) -> Option<usize> {
    /*
     * `resolve_find`, but optionally nudged one character further in the
     * search direction first — needed when repeating a `t`/`T` from the
     * exact position it left the cursor at, which would otherwise
     * immediately re-find the same adjacent occurrence and no-op (see
     * `till_char_forward`'s doc comment). `nudge` should be true only for
     * `;`/`,` repeats, never for a fresh `f`/`F`/`t`/`T` keypress: plain
     * f/F don't need it either way since `find_char_forward`/`_backward`
     * already search strictly past the cursor. Shared by `AppState::
     * apply_find` (fresh finds and their repeats) and `resolve_repeat_find`
     * (the Visual-mode-aware repeat path) so this nudge behaviour can't
     * drift between the two.
     */
    let search_from = if nudge && (kind == 't' || kind == 'T') {
        match kind {
            't' => char_right(content, cursor),
            'T' => char_left(content, cursor),
            _ => cursor,
        }
    } else {
        cursor
    };
    resolve_find(content, search_from, kind, target)
}

pub(super) fn resolve_vim_visual_operator_key(
    key: &str,
    shift: bool,
    key_char: Option<&str>,
) -> Option<char> {
    /*
     * Resolves a keystroke to the Visual-mode operator it represents
     * (spec 5.6), or `None` if it isn't one. `d`/`x` are equivalent here
     * (both "delete selection") — `x` has no Normal-mode meaning built yet
     * (that's Task I's single-character-under-cursor delete), but the
     * Visual-mode row of the spec lists it explicitly. `gU`/`gu` aren't
     * handled here — they're two-keystroke commands checked separately by
     * the caller, ahead of this function, so a pending `g` doesn't fall
     * through to here at all. `>`/`<`/`~` sit on shifted punctuation, so
     * `matches_shifted_symbol` is used for the same reliability reason as
     * everywhere else in this file.
     */
    if (key == "d" || key == "x") && !shift {
        return Some('d');
    }
    if key == "y" && !shift {
        return Some('y');
    }
    if key == "c" && !shift {
        return Some('c');
    }
    if matches_shifted_symbol(key, shift, key_char, ".", ">") {
        return Some('>');
    }
    if matches_shifted_symbol(key, shift, key_char, ",", "<") {
        return Some('<');
    }
    if matches_shifted_symbol(key, shift, key_char, "`", "~") {
        return Some('~');
    }
    None
}

pub(crate) fn matches_shifted_symbol(
    key: &str,
    shift: bool,
    key_char: Option<&str>,
    unshifted_key: &str,
    symbol: &str,
) -> bool {
    /*
     * True when a keystroke represents `symbol`, a shifted number/
     * punctuation-row character GPUI might report in any of several ways
     * depending on platform/backend — confirmed empirically (`$` did
     * nothing under the original two-way check) that which one actually
     * fires isn't reliable enough to pick a single method:
     *   - `key == symbol` directly — observed on this app's WSLg/X11
     *     backend, where XKB appears to resolve shift into the reported
     *     key before GPUI ever sees it, contradicting the vendored
     *     `Keystroke` docs' claim that `key` is always the unshifted base
     *     glyph.
     *   - `key_char == Some(symbol)` — GPUI's documented "character that
     *     would actually be typed" field.
     *   - `key == unshifted_key && shift` — the vendored docs' literal
     *     unshifted-base-glyph-plus-modifier behaviour, kept as a fallback
     *     in case a different backend really does behave that way.
     */
    key == symbol || key_char == Some(symbol) || (key == unshifted_key && shift)
}

/// True if `(key, shift)` already has a real meaning somewhere in vim's own
/// Normal-mode dispatch (`handle_vim_normal_key`/`resolve_vim_motion`/
/// `complete_vim_operator`), as of this writing — the keyspace a
/// vim-keybind's *first* key (see `AppState.vim_keybind_seq`) must never
/// collide with. Every key *after* the first is safe regardless of this
/// check: it's consumed by our own sequence buffer before ever reaching the
/// native dispatcher.
///
/// Hand-maintained, not derived — the real dispatcher has no single
/// declarative table to derive this from; it's a long, carefully-ordered
/// chain of match arms and if-chains. What keeps this list honest is
/// `test_every_non_reserved_key_is_a_true_vim_noop` (below, in `tests`): an
/// exhaustive test replaying every key/shift combination *not* covered here
/// through a fresh Normal-mode `AppState` with no pending state, asserting
/// zero observable change. Adding a new real vim command later means
/// updating this function too, or that test fails — which is the point:
/// drift becomes a build failure, not a silent shadowing bug.
///
/// Deliberately narrower than "everything real vim binds" — only what THIS
/// app's vim mode actually implements today. Shifted `D`/`Y`/`C` (real vim:
/// delete/yank/change to end of line), `U` (undo whole line), `K` (keyword
/// lookup), `Q` (Ex mode) are genuinely unclaimed here and left out on
/// purpose — claiming them defensively for commands that don't exist yet
/// would take away first-keys from users for no present benefit. `H`/`M`/`L`
/// (visual screen jump) and `@`/`@@`/`@<register>` (macro replay) are
/// reserved here even though they're actually intercepted a layer up, in
/// `text_editor.rs`, before a keystroke ever reaches `handle_vim_key` at all
/// — this function can't see that layer, so it errs toward reserving them
/// anyway rather than silently assuming they're free.
pub(crate) fn is_vim_reserved_normal_key(key: &str, shift: bool, key_char: Option<&str>) -> bool {
    // Digits are always reserved (count-prefix accumulation; '0' doubles as
    // the "start of line" motion).
    if key.len() == 1 && key.chars().next().unwrap().is_ascii_digit() {
        return true;
    }
    match key {
        // Both cases meaningful.
        "h" | "l" | "w" | "b" | "e" | "g" | "f" | "t" | "i" | "a" | "o" | "v" | "p" | "x" | "s"
        | "j" | "r" | "n" => true,
        // One case meaningful today (see doc comment above for what's
        // deliberately left unclaimed): lowercase only.
        "d" | "y" | "c" | "u" | "q" | "_" => !shift,
        // "m"/"k" are the odd ones out: lowercase free (marks/keyword-lookup
        // never implemented), uppercase reserved (H/M/L visual jump).
        "m" | "k" => shift,
        // GPUI-reliability-dependent shifted symbols — same multi-way check
        // used everywhere else in this file, since which of key/key_char/
        // (key+shift) actually fires isn't consistent across backends.
        _ => {
            matches_shifted_symbol(key, shift, key_char, ";", ":")
                || matches_shifted_symbol(key, shift, key_char, ".", ">")
                || matches_shifted_symbol(key, shift, key_char, ",", "<")
                || matches_shifted_symbol(key, shift, key_char, "`", "~")
                || matches_shifted_symbol(key, shift, key_char, "8", "*")
                || matches_shifted_symbol(key, shift, key_char, "3", "#")
                || matches_shifted_symbol(key, shift, key_char, "[", "{")
                || matches_shifted_symbol(key, shift, key_char, "]", "}")
                || matches_shifted_symbol(key, shift, key_char, "4", "$")
                || matches_shifted_symbol(key, shift, key_char, "6", "^")
                || matches_shifted_symbol(key, shift, key_char, "'", "\"")
                || (key == ";" && !shift)
                || (key == "," && !shift)
                || (key == "/" || key_char == Some("/"))
                || key == "@"
        }
    }
}

pub(super) fn find_kind_to_motion_kind(kind: char) -> MotionKind {
    /*
     * `f`/`F` (find, land *on* the target) are inclusive; `t`/`T` (till,
     * land *before* it) are exclusive — vim's own `:help f`/`:help t`
     * convention, mirrored here so `df<char>`/`dt<char>` (and their `;`/
     * `,` repeats) build the right operator range.
     */
    match kind {
        'f' | 'F' => MotionKind::InclusiveChar,
        _ => MotionKind::ExclusiveChar,
    }
}

pub(crate) fn vim_find_target_char(key: &str, shift: bool, key_char: Option<&str>) -> Option<char> {
    /*
     * Resolves a single literal target character from a keystroke — used
     * for a pending `f`/`F`/`t`/`T` command's find-target and for a
     * pending `q`/`@` command's register name. Prefers `key_char` (the
     * character GPUI reports would actually be typed, correctly reflecting
     * shift for punctuation) when present; otherwise falls back to `key`
     * with alphabetic shift-to-uppercase applied (mirroring the
     * plain-editor insertion arm in `text_editor.rs`), since `key_char`
     * isn't guaranteed for every key GPUI reports. Returns `None` for
     * named multi-character keys (e.g. "escape", "tab") that aren't a
     * literal character — pressing one of those while a command is
     * pending simply abandons it (see each caller), matching vim's
     * Escape-cancels-pending-command behaviour.
     *
     * Space is the one exception to that rule: GPUI names it "space", a
     * multi-character key that the fallback below would reject, but it is a
     * real literal character — `text_editor.rs`'s own insertion arm spells it
     * out the same way (`"space" => insert_char(' ')`). Without this, `f<space>`
     * can't jump to a space and neither rename input (tab titles, file names)
     * can type one, which rules out most real file names in this app.
     */
    if let Some(kc) = key_char.and_then(|s| s.chars().next()) {
        return Some(kc);
    }
    if key == "space" {
        return Some(' ');
    }
    if key == "'" {
        return Some(if shift { '"' } else { '\'' });
    }
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(if shift && c.is_alphabetic() {
        c.to_ascii_uppercase()
    } else {
        c
    })
}
