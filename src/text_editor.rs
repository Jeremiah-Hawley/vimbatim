use gpui::prelude::*;
use gpui::*;

use crate::auto_scroll::AutoScroller;
use crate::state::AppState;

/// Approximate monospace glyph metrics for the editor's `text_sm()`
/// monospace font (14px), used only to convert a mouse click's pixel
/// position into a character column/line. This is an estimate (0.6× font
/// size, the typical monospace advance width), not real glyph shaping —
/// precise hit-testing would require rendering lines through GPUI's
/// InteractiveText/ShapedLine APIs instead of plain divs, which is a larger
/// rework than click-to-position alone justifies right now.
const CHAR_WIDTH_PX: f32 = 8.4;
/// Matches the `.min_h(px(20.0))` set on each line div in render().
const LINE_HEIGHT_PX: f32 = 20.0;
/// Matches the `.p(px(16.0))` set on the outer editor div in render().
const CONTENT_PADDING_PX: f32 = 16.0;
/// Number of lines of buffer to keep visible above/below the cursor —
/// mirrors Vim's `scrolloff`. `scroll_to_cursor` starts scrolling once the
/// cursor comes within this many lines of the viewport edge, rather than
/// waiting until the cursor line itself is already clipped.
const SCROLL_MARGIN_LINES: f32 = 3.0;

/// The main document editing area.
///
/// Renders the text content of the currently active tab inside a focused,
/// scrollable div. Keyboard input is routed here when the div holds focus.
///
/// Designed to be the extensible base for .docx support: content currently lives
/// as plain `String` in `AppState::Tab`, meaning callers can swap in a richer
/// document model without touching this view's rendering or focus plumbing.
pub struct TextEditor {
    state: Entity<AppState>,
    /// GPUI focus handle — required to receive raw keyboard events.
    focus_handle: FocusHandle,
    /// Tracks this editor's scroll state (see `.track_scroll()` in
    /// render()). Besides the scroll offset itself, `.bounds()` also gives
    /// the editor's fixed viewport box in window coordinates — GPUI's own
    /// layout bounds for the tracked div, computed before any scroll
    /// translation is applied, so it can't drift with scroll position the
    /// way a hand-rolled bounds capture could. Click/drag positioning uses
    /// both `.offset()` and `.bounds()` to convert screen-relative
    /// coordinates into document-relative ones; drag-to-edge auto-scroll
    /// uses `.bounds()` for its edge-trigger check.
    scroll_handle: ScrollHandle,
    /// Drives continuous scrolling while a click-drag sits near the top/
    /// bottom edge of the viewport — see `auto_scroll::AutoScroller`.
    auto_scroller: AutoScroller,
}

impl TextEditor {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        /*
         * Creates the text editor and registers a focus handle. Focus is claimed
         * lazily the first time the user clicks inside the editor.
         *
         * The `cx.focus_handle()` call creates a new entry in GPUI's focus registry;
         * the handle must be passed to `.track_focus()` in render() so the element
         * participates in the focus tree.
         */
        let focus_handle = cx.focus_handle();
        let scroll_handle = ScrollHandle::new();
        let auto_scroller = AutoScroller::new(scroll_handle.clone(), state.clone());
        TextEditor { state, focus_handle, scroll_handle, auto_scroller }
    }

    fn scroll_to_cursor(&self, cx: &Context<Self>) {
        /*
         * Scrolls vertically so the cursor line stays at least
         * `SCROLL_MARGIN_LINES` lines inside the visible viewport. Called
         * after every key event that could move the cursor.
         *
         * GPUI scroll offsets are ≤ 0: 0 means scrolled to the top, and
         * more-negative values mean the document has been scrolled further down.
         *
         * All positions here are in the same "content space" that
         * `line_col_from_mouse_position` uses: line `i`'s top sits at
         * `i * LINE_HEIGHT_PX`, with no padding baked into per-line offsets —
         * padding is only ever a one-time inset when converting to/from
         * screen space. `bounds().size.height` is the div's full border-box
         * height, which includes the top *and* bottom padding, so the actual
         * visible content window is `viewport_h - 2 * CONTENT_PADDING_PX`,
         * not the raw bounds height — using the raw height here previously
         * overestimated how much content was visible and let the cursor
         * drift below the real bottom edge before scrolling kicked in.
         *
         * The trigger checks use `margin` so scrolling begins while the
         * cursor is still comfortably visible, not only once it's already
         * clipped — otherwise a single keystroke can move the cursor from
         * "just visible" to "off-screen" with nothing to catch it. The
         * target offsets then re-open exactly `margin` worth of space on the
         * side being scrolled toward, so the buffer is restored rather than
         * just barely satisfied.
         *
         * The method is a no-op when the scroll handle has not been laid out
         * yet (viewport_h <= 0), which can happen on the very first frame.
         */
        let cursor_line    = self.state.read(cx).cursor_line_col().0;
        let cursor_top     = cursor_line as f32 * LINE_HEIGHT_PX;
        let cursor_bottom  = cursor_top + LINE_HEIGHT_PX;
        let margin         = SCROLL_MARGIN_LINES * LINE_HEIGHT_PX;

        let offset         = self.scroll_handle.offset();
        let viewport_h     = self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
        if viewport_h <= 0.0 { return; }

        let max_y          = self.scroll_handle.max_offset().y.as_f32();
        let visible_top    = -offset.y.as_f32();
        let visible_bottom = visible_top + viewport_h;

        if cursor_top < visible_top + margin {
            // Cursor is within `margin` of the top edge (or above it) —
            // scroll up so `margin` worth of buffer opens above the line.
            // Clamped to 0 so this can't scroll past the top of the document
            // just because the margin asked for space that doesn't exist yet.
            let new_y = (margin - cursor_top).clamp(-max_y.max(0.0), 0.0);
            self.scroll_handle.set_offset(point(offset.x, px(new_y)));
        } else if cursor_bottom > visible_bottom - margin {
            // Cursor is within `margin` of the bottom edge (or below it) —
            // scroll down so `margin` worth of buffer opens below the line.
            let new_y = (viewport_h - margin - cursor_bottom).clamp(-max_y.max(0.0), 0.0);
            self.scroll_handle.set_offset(point(offset.x, px(new_y)));
        }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Dispatches raw key-down events to the AppState so text content is updated.
         *
         * Platform-modifier (Ctrl/Cmd) combinations are deliberately passed through
         * so global actions (toggle-settings, new-tab, etc.) can fire normally.
         * Only pure character input, space, enter, tab, and backspace are consumed.
         * scroll_to_cursor is called at every exit point so the cursor line stays
         * visible regardless of which key moved it.
         */
        let ks = &event.keystroke;

        // Handle the Ctrl/platform combos that belong to the editor; let the
        // rest bubble to the global action dispatcher (Ctrl+S, Ctrl+T, etc.).
        if ks.modifiers.control || ks.modifiers.platform {
            match ks.key.as_str() {
                "c" => {
                    // Copy: read-only — use entity.read so no update closure needed.
                    let text = self.state.read(cx).copy_selection();
                    if let Some(text) = text {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
                "x" => {
                    // Cut: delete selection inside update, write result to clipboard.
                    let text = self.state.update(cx, |state, cx| {
                        let result = state.cut_selection();
                        if result.is_some() { cx.notify(); }
                        result
                    });
                    if let Some(text) = text {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        cx.notify();
                    }
                }
                "v" => {
                    // Paste: read clipboard on outer cx first, then insert inside update.
                    if let Some(item) = cx.read_from_clipboard() {
                        if let Some(text) = item.text() {
                            self.state.update(cx, |state, cx| {
                                state.insert_str(&text);
                                cx.notify();
                            });
                            cx.notify();
                        }
                    }
                }
                "a" => {
                    // Ctrl+A: select all (spec 4.3).
                    self.state.update(cx, |state, _cx| state.select_all());
                    cx.notify();
                }
                // Ctrl+Left/Right jump by word; Ctrl+Home/End jump to document start/end
                // (spec 4.1). Shift+Ctrl+<key> extends the selection instead of just
                // moving (spec 4.3). Plain (unmodified) arrow/Home/End are handled below.
                "left" => {
                    self.state.update(cx, |state, _cx| {
                        if ks.modifiers.shift { state.extend_word_backward() } else { state.move_word_backward() }
                    });
                    cx.notify();
                }
                "right" => {
                    self.state.update(cx, |state, _cx| {
                        if ks.modifiers.shift { state.extend_word_forward() } else { state.move_word_forward() }
                    });
                    cx.notify();
                }
                "home" => {
                    self.state.update(cx, |state, _cx| {
                        if ks.modifiers.shift { state.extend_doc_start() } else { state.move_doc_start() }
                    });
                    cx.notify();
                }
                "end" => {
                    self.state.update(cx, |state, _cx| {
                        if ks.modifiers.shift { state.extend_doc_end() } else { state.move_doc_end() }
                    });
                    cx.notify();
                }
                _ => {} // Ctrl+S, Ctrl+T, Ctrl+W, etc. handled by global actions
            }
            self.scroll_to_cursor(cx);
            return;
        }

        let key = ks.key.as_str();
        let consumed = self.state.update(cx, |state, cx| {
            match key {
                "backspace" => { state.backspace(); cx.notify(); true }
                "enter"     => { state.insert_char('\n'); cx.notify(); true }
                "space"     => { state.insert_char(' '); cx.notify(); true }
                "tab"       => { state.insert_char('\t'); cx.notify(); true }
                // Shift+<key> extends the selection instead of moving plainly (spec 4.3).
                "left"      => { if ks.modifiers.shift { state.extend_left() } else { state.move_left() }; cx.notify(); true }
                "right"     => { if ks.modifiers.shift { state.extend_right() } else { state.move_right() }; cx.notify(); true }
                "up"        => { if ks.modifiers.shift { state.extend_up() } else { state.move_up() }; cx.notify(); true }
                "down"      => { if ks.modifiers.shift { state.extend_down() } else { state.move_down() }; cx.notify(); true }
                "home"      => { if ks.modifiers.shift { state.extend_line_start() } else { state.move_line_start() }; cx.notify(); true }
                "end"       => { if ks.modifiers.shift { state.extend_line_end() } else { state.move_line_end() }; cx.notify(); true }
                k if k.chars().count() == 1 => {
                    let mut ch = k.chars().next().unwrap();
                    // Apply shift for uppercase; GPUI gives lowercase key names
                    if ks.modifiers.shift && ch.is_alphabetic() {
                        ch = ch.to_uppercase().next().unwrap_or(ch);
                    }
                    state.insert_char(ch);
                    cx.notify();
                    true
                }
                _ => false,
            }
        });
        if consumed { cx.notify(); }
        self.scroll_to_cursor(cx);
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the editor as a focusable, scrollable column.
         *
         * Content is split on '\n' so each line is its own div — this preserves
         * blank lines and avoids GPUI collapsing inline text across newlines.
         *
         * The line `tab.cursor` actually points into is rendered as three
         * inline spans (text before / cursor cell / text after) so the cursor
         * marker sits at the real character position, rather than always
         * trailing the last line regardless of where the cursor is.
         *
         * Clicking anywhere in the editor reclaims keyboard focus.
         */
        let state = self.state.read(cx);
        let content = state.active_content().to_string();
        let is_new_tab = state
            .tabs
            .get(state.active_tab)
            .map(|t| t.file_path.is_none() && t.content.is_empty())
            .unwrap_or(true);
        let (cursor_line, cursor_col) = state.cursor_line_col();
        // Normalise (anchor, focus) into (min, max) once so per-line lookups
        // below don't each have to re-derive the ordering.
        let selection = state
            .tabs
            .get(state.active_tab)
            .and_then(|t| t.selection)
            .map(|(a, f)| (a.min(f), a.max(f)));
        let _ = state;

        let is_focused = self.focus_handle.is_focused(window);

        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.split('\n').map(|l| l.to_string()).collect()
        };

        let num_lines = lines.len();
        // Byte offset of each line's start within `content`, needed to test
        // `selection` (a document-wide byte range) against each line.
        let mut line_byte_starts: Vec<usize> = Vec::with_capacity(lines.len());
        let mut offset = 0;
        for l in &lines {
            line_byte_starts.push(offset);
            offset += l.len() + 1; // +1 for the '\n' the split() consumed
        }

        div()
            // `.id()` must come before `.overflow_y_scroll()` because GPUI tracks
            // scroll position per unique element ID (requires Stateful<Div>).
            .id("text-editor")
            .key_context("TextEditor")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            // Clicking the editor area claims keyboard focus and moves the
            // cursor to the clicked position (spec 4.1 click-to-position).
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                this.focus_handle.clone().focus(window, cx);
                let bounds = this.scroll_handle.bounds();
                let scroll_y = this.scroll_handle.offset().y.as_f32();
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, num_lines);
                this.state.update(cx, |state, cx| {
                    state.set_cursor_from_line_col(line, col);
                    cx.notify();
                });
                cx.notify();
            }))
            // Dragging with the left button held extends a selection from
            // wherever on_mouse_down landed (spec 4.3 "mouse click-drag
            // creates a selection"). `auto_scroller.notify` starts (or feeds)
            // a per-frame auto-scroll loop when the drag is near the top/
            // bottom edge of the viewport, so the selection can extend past
            // what's currently visible even if the mouse stops moving.
            // `on_mouse_move` only fires while the cursor is over this
            // element's own bounds, so a drag that exits the editor (e.g.
            // into the sidebar) stops updating until it re-enters —
            // acceptable for a first pass, not spec-required to track drags
            // that leave the editor.
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, window, cx| {
                if !ev.dragging() { return; }
                let bounds = this.scroll_handle.bounds();
                let scroll_y = this.scroll_handle.offset().y.as_f32();
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, num_lines);
                this.state.update(cx, |state, cx| {
                    state.extend_selection_to_line_col(line, col);
                    cx.notify();
                });
                this.auto_scroller.notify(ev.position, num_lines, window);
                cx.notify();
            }))
            // Stop any in-progress auto-scroll loop on mouse-up, whether the
            // release happens over the editor (on_mouse_up) or elsewhere
            // (on_mouse_up_out, e.g. the user dragged into the sidebar and
            // released there) — otherwise a drag that ends while parked in
            // the edge zone would keep scrolling forever with nothing left
            // to stop it.
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _ev, _window, _cx| {
                this.auto_scroller.stop();
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _ev, _window, _cx| {
                this.auto_scroller.stop();
            }))
            .flex_1()
            .min_w_0()
            .bg(rgb(0x1e1e1e))
            .overflow_y_scroll()
            // Prevent long lines from expanding the editor div beyond its flex_1 allocation.
            // overflow_y_scroll only sets the vertical axis; without this, the x axis
            // defaults to visible and content flows off-screen to the right.
            .overflow_x_hidden()
            .track_scroll(&self.scroll_handle)
            .p(px(16.0))
            // Thin focus ring so the user can tell where key input lands
            .border_1()
            .border_color(if is_focused { rgb(0x007acc) } else { rgb(0x1e1e1e) })
            .child(
                div()
                    .flex()
                    .flex_col()
                    // w_full constrains each line to the editor width so text wraps
                    // rather than extending off-screen to the right.
                    .w_full()
                    // Placeholder shown on an empty, unsaved tab
                    .when(is_new_tab, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x555555))
                                .font_family("monospace")
                                .child("Open a file from the sidebar, or start typing…"),
                        )
                    })
                    // One div per line of content; render_line overlays the
                    // cursor marker and/or selection highlight on whichever
                    // lines they actually touch.
                    .children(lines.iter().enumerate().map(|(i, line)| {
                        let line_cursor_col = (i == cursor_line && is_focused).then_some(cursor_col);
                        let line_selection = selection.and_then(|(s, e)| {
                            selection_span_for_line(line, line_byte_starts[i], s, e)
                        });
                        let content_el = render_line(line, line_cursor_col, line_selection);
                        div()
                            .font_family("monospace")
                            .text_sm()
                            .text_color(rgb(0xd4d4d4))
                            // min_h keeps empty lines visually present
                            .min_h(px(20.0))
                            .child(content_el)
                    }))
            )
    }
}

fn render_line(line: &str, cursor_col: Option<usize>, selection: Option<(usize, usize)>) -> AnyElement {
    /*
     * Renders one line of text, splitting it into styled spans wherever the
     * cursor and/or selection touch it, via `line_segments`. Falls back to a
     * single plain-text child when neither applies, matching the cheap path
     * every other (untouched) line already takes.
     */
    let chars: Vec<char> = line.chars().collect();
    let segments = line_segments(chars.len(), cursor_col, selection);

    if let [(start, end, SegmentStyle::Plain)] = segments.as_slice() {
        if *start == 0 && *end == chars.len() {
            return line.to_string().into_any_element();
        }
    }

    let spans: Vec<AnyElement> = segments
        .into_iter()
        .map(|(start, end, style)| {
            // A zero-width segment only ever occurs for the cursor sitting
            // past the last character (end of line) — render it as a
            // single space so the highlighted cell still has visible width.
            let text: String = if start == end {
                " ".to_string()
            } else {
                chars[start..end].iter().collect()
            };
            match style {
                SegmentStyle::Cursor => div()
                    .bg(rgb(0xd4d4d4))
                    .text_color(rgb(0x1e1e1e))
                    .child(text)
                    .into_any_element(),
                // #264F78 at ~50% opacity, per spec 6.4's selection-highlight color.
                SegmentStyle::Selection => div().bg(rgba(0x264F7880)).child(text).into_any_element(),
                SegmentStyle::Plain => text.into_any_element(),
            }
        })
        .collect();

    div().flex().flex_row().children(spans).into_any_element()
}

fn column_for_x(x: f32, char_width: f32) -> usize {
    /*
     * Converts an x pixel offset (relative to the start of the text, i.e.
     * after subtracting the container's left padding) into a character
     * column, rounding to the nearest column and clamping negative input
     * to 0.
     */
    if char_width <= 0.0 || x <= 0.0 { return 0; }
    (x / char_width).round() as usize
}

fn line_for_y(y: f32, line_height: f32, num_lines: usize) -> usize {
    /*
     * Converts a y pixel offset (relative to the start of the text) into a
     * 0-indexed line number, clamped to `num_lines - 1` so a click below
     * the last line still lands on it rather than panicking on an
     * out-of-range line index.
     */
    if line_height <= 0.0 || num_lines == 0 { return 0; }
    if y <= 0.0 { return 0; }
    ((y / line_height) as usize).min(num_lines - 1)
}

pub(crate) fn line_col_from_mouse_position(
    position: Point<Pixels>,
    content_bounds: Bounds<Pixels>,
    scroll_offset_y: f32,
    num_lines: usize,
) -> (usize, usize) {
    /*
     * Converts a window-space mouse position into a (line, char_column)
     * pair, via `column_for_x`/`line_for_y`. Shared by on_mouse_down (plain
     * click) and on_mouse_move (click-drag) so the two can never disagree
     * about where a given pixel position maps to.
     *
     * `content_bounds` is the editor's fixed viewport box — GPUI's own
     * layout bounds for the tracked div (`ScrollHandle::bounds()`), which
     * doesn't move when the document scrolls, so a position relative to it
     * alone would describe screen position, not document position, on any
     * document taller than one screen. `scroll_offset_y` is
     * `ScrollHandle::offset().y`, which goes more negative the further the
     * document has been scrolled down — subtracting it converts
     * screen-relative Y into document-relative Y.
     */
    // Subtract the container's padding (spec: `.p(px(16.0))` in render())
    // so (0, 0) lines up with the first character of the text.
    let local_x = position.x.as_f32() - content_bounds.origin.x.as_f32() - CONTENT_PADDING_PX;
    let local_y = position.y.as_f32() - content_bounds.origin.y.as_f32() - CONTENT_PADDING_PX - scroll_offset_y;
    let col  = column_for_x(local_x, CHAR_WIDTH_PX);
    let line = line_for_y(local_y, LINE_HEIGHT_PX, num_lines);
    (line, col)
}

fn selection_span_for_line(line: &str, line_byte_start: usize, sel_start: usize, sel_end: usize) -> Option<(usize, usize)> {
    /*
     * Maps a selection's document-wide byte range onto the char-column range
     * of a single line, or None if the selection doesn't touch this line at
     * all (including the boundary case where the selection ends exactly at
     * this line's first byte, or starts exactly at its last byte — those
     * describe a selection that stops at the newline, not one that includes
     * this line's visible characters). `sel_start`/`sel_end` must already be
     * normalized so `sel_start <= sel_end`.
     */
    if sel_start == sel_end { return None; } // nothing selected
    let line_byte_end = line_byte_start + line.len();
    if sel_end <= line_byte_start || sel_start >= line_byte_end { return None; }

    // Clamp each selection edge into this line's byte range, then convert
    // that relative byte offset into a char column (not byte column).
    let to_col = |byte: usize| -> usize {
        let rel = byte.saturating_sub(line_byte_start).min(line.len());
        line[..rel].chars().count()
    };
    let start_col = to_col(sel_start.max(line_byte_start));
    let end_col = to_col(sel_end.min(line_byte_end));
    if start_col == end_col { return None; } // e.g. an empty line fully inside the selection
    Some((start_col, end_col))
}

/// How a single rendered line segment should be styled — plain text, the
/// cursor's highlighted cell, or the selection's background overlay.
#[derive(Debug, PartialEq, Clone, Copy)]
enum SegmentStyle {
    Plain,
    Cursor,
    Selection,
}

fn line_segments(len: usize, cursor_col: Option<usize>, selection: Option<(usize, usize)>) -> Vec<(usize, usize, SegmentStyle)> {
    /*
     * Splits a line of `len` characters into styled segments by merging the
     * cursor position (if this is the cursor's line) and the selection's
     * char-column range (if any) into one ordered list of breakpoints, then
     * classifying each resulting [start, end) run. The cursor always gets
     * its own single-character segment even where it sits inside a
     * selection — real editors draw the block cursor on top of selection
     * highlighting, not the other way around. A cursor sitting past the
     * last character (end of line) produces a synthetic zero-width segment
     * (`len, len`) that the renderer turns into a single highlighted space.
     */
    if cursor_col.is_none() && selection.is_none() {
        return vec![(0, len, SegmentStyle::Plain)];
    }

    let mut breaks: Vec<usize> = vec![0, len];
    if let Some(c) = cursor_col {
        let c = c.min(len);
        breaks.push(c);
        if c < len { breaks.push(c + 1); }
    }
    if let Some((s, e)) = selection {
        breaks.push(s.min(len));
        breaks.push(e.min(len));
    }
    breaks.sort_unstable();
    breaks.dedup();

    let mut segments = Vec::new();
    for w in breaks.windows(2) {
        let (start, end) = (w[0], w[1]);
        let is_cursor = cursor_col.map(|c| c.min(len) == start && end == start + 1).unwrap_or(false);
        let in_selection = selection
            .map(|(s, e)| start >= s.min(len) && end <= e.min(len))
            .unwrap_or(false);
        let style = if is_cursor {
            SegmentStyle::Cursor
        } else if in_selection {
            SegmentStyle::Selection
        } else {
            SegmentStyle::Plain
        };
        segments.push((start, end, style));
    }

    // A cursor at (or past) the end of the line has no character to occupy,
    // so the main loop above never produces a segment for it — append one.
    if let Some(c) = cursor_col {
        if c >= len {
            segments.push((len, len, SegmentStyle::Cursor));
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    // Import only the two functions under test, not `super::*` — text_editor.rs
    // has `use gpui::*;` at module scope, and gpui exports its own `test`
    // attribute macro (for async GPUI tests) that shadows std's `#[test]` and
    // sends the test-attribute expansion into infinite recursion if it's in
    // scope here.
    use super::{column_for_x, line_for_y, selection_span_for_line, line_segments, SegmentStyle};

    #[test]
    fn test_column_for_x_zero_is_first_column() {
        assert_eq!(column_for_x(0.0, 8.4), 0);
    }

    #[test]
    fn test_column_for_x_rounds_to_nearest_column() {
        assert_eq!(column_for_x(4.0, 8.4), 0); // 4.0/8.4 = 0.48 -> rounds down
        assert_eq!(column_for_x(5.0, 8.4), 1); // 5.0/8.4 = 0.60 -> rounds up
        assert_eq!(column_for_x(8.4, 8.4), 1);
    }

    #[test]
    fn test_column_for_x_clamps_negative_to_zero() {
        assert_eq!(column_for_x(-3.0, 8.4), 0);
    }

    #[test]
    fn test_column_for_x_zero_char_width_is_zero() {
        assert_eq!(column_for_x(5.0, 0.0), 0);
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
        assert_eq!(selection_span_for_line("hello world", 0, 0, 5), Some((0, 5)));
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
        assert_eq!(line_segments(5, None, None), vec![(0, 5, SegmentStyle::Plain)]);
    }

    #[test]
    fn test_line_segments_cursor_mid_line() {
        assert_eq!(
            line_segments(5, Some(2), None),
            vec![(0, 2, SegmentStyle::Plain), (2, 3, SegmentStyle::Cursor), (3, 5, SegmentStyle::Plain)]
        );
    }

    #[test]
    fn test_line_segments_cursor_at_line_start() {
        assert_eq!(
            line_segments(5, Some(0), None),
            vec![(0, 1, SegmentStyle::Cursor), (1, 5, SegmentStyle::Plain)]
        );
    }

    #[test]
    fn test_line_segments_cursor_past_end_of_line() {
        assert_eq!(
            line_segments(5, Some(5), None),
            vec![(0, 5, SegmentStyle::Plain), (5, 5, SegmentStyle::Cursor)]
        );
    }

    #[test]
    fn test_line_segments_selection_only() {
        assert_eq!(
            line_segments(6, None, Some((1, 4))),
            vec![(0, 1, SegmentStyle::Plain), (1, 4, SegmentStyle::Selection), (4, 6, SegmentStyle::Plain)]
        );
    }

    #[test]
    fn test_line_segments_selection_covers_full_line() {
        assert_eq!(line_segments(6, None, Some((0, 6))), vec![(0, 6, SegmentStyle::Selection)]);
    }

    #[test]
    fn test_line_segments_cursor_inside_selection_wins_its_own_cell() {
        assert_eq!(
            line_segments(6, Some(2), Some((2, 5))),
            vec![
                (0, 2, SegmentStyle::Plain),
                (2, 3, SegmentStyle::Cursor),
                (3, 5, SegmentStyle::Selection),
                (5, 6, SegmentStyle::Plain),
            ]
        );
    }

    #[test]
    fn test_line_segments_empty_line_with_cursor() {
        assert_eq!(line_segments(0, Some(0), None), vec![(0, 0, SegmentStyle::Cursor)]);
    }
}
