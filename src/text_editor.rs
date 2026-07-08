use gpui::prelude::*;
use gpui::*;

use crate::auto_scroll::AutoScroller;
use crate::docx_parser::{Paragraph, Run};
use crate::document_ops::paragraph_run_char_spans;
use crate::state::{matches_shifted_symbol, vim_find_target_char, AppState, VimMode};

/// Approximate monospace glyph width, used only to convert a mouse click's
/// pixel X position into a character column within its row. This is an
/// estimate (0.6× font size, the typical monospace advance width), not real
/// glyph shaping — precise X hit-testing would require rendering lines
/// through GPUI's InteractiveText/ShapedLine APIs instead of plain divs,
/// which is a larger rework than click-to-position alone justifies right
/// now. Word-wrap decisions do *not* use this — see `char_width_fn`, which
/// measures each character's real rendered width instead, since a single
/// uniform estimate is wrong for narrow glyphs like '.' or '-' and folds
/// lines dominated by them far earlier than their actual on-screen width
/// would require.
const CHAR_WIDTH_PX: f32 = 8.4;
/// The editor's `text_sm()` resolves to 0.875rem, i.e. 14px at GPUI's
/// default 16px rem_size (this app never overrides rem_size). Used to query
/// real glyph widths for word-wrap via `TextSystem::layout_width`, which
/// needs an explicit font size rather than reading it from render()'s
/// ambient text style.
const FONT_SIZE_PX: f32 = 14.0;
/// Matches the `.min_h(px(20.0))` set on each line div in render().
const LINE_HEIGHT_PX: f32 = 20.0;
/// Matches the `.p(px(16.0))` set on the outer editor div in render().
const CONTENT_PADDING_PX: f32 = 16.0;
/// Number of lines of buffer to keep visible above/below the cursor —
/// mirrors Vim's `scrolloff`. `scroll_to_cursor` starts scrolling once the
/// cursor comes within this many lines of the viewport edge, rather than
/// waiting until the cursor line itself is already clipped.
const SCROLL_MARGIN_LINES: f32 = 3.0;
/// A literal, well-known monospace family name rather than the generic
/// CSS-style alias `"monospace"`. GPUI's font matching (`cosmic_text`'s
/// `load_family`) filters real system fonts by an *exact string* match
/// against each font file's own embedded family name — no font ever
/// declares its family as literally "monospace", so that name always
/// missed and fell through to GPUI's hardcoded fallback stack, which
/// resolves each candidate at default weight/style, discarding any
/// requested bold/italic before it ever reaches font matching. Separately,
/// `find_best_match` short-circuits (`candidates.len() == 1 => Ok(0)`)
/// without checking weight/style whenever the resolved family has only one
/// loaded face — together these silently dropped every bold/italic
/// request. "DejaVu Sans Mono" ships with separate Book/Bold/Oblique/Bold
/// Oblique faces under one family name on essentially all Linux/WSL
/// systems, giving `find_best_match` real candidates to choose between.
const FONT_FAMILY: &str = "DejaVu Sans Mono";
/// The editor's default (un-styled) text color, matching the literal used
/// at each row/placeholder `.text_color(rgb(0xd4d4d4))` call site — used by
/// `apply_run_style` as the "effective" text color when a run has no
/// explicit `color` override, to decide whether a highlight needs darkening.
const DEFAULT_TEXT_COLOR_HEX: u32 = 0xd4d4d4;

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
    /// True right after a bare `@` was pressed in Normal mode, waiting for
    /// the register character that completes `@<register>` (user-requested
    /// macro replay — not part of editor_instructions.md). Kept here rather
    /// than in `AppState` since resolving it triggers `replay_macro`, which
    /// needs this struct's GPUI context.
    macro_at_pending: bool,
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
        TextEditor { state, focus_handle, scroll_handle, auto_scroller, macro_at_pending: false }
    }

    fn scroll_to_cursor(&self, cx: &Context<Self>) {
        /*
         * Scrolls vertically so the cursor's visual row stays at least
         * `SCROLL_MARGIN_LINES` rows inside the visible viewport. Called
         * after every key event that could move the cursor.
         *
         * A wrapped logical line spans several visual rows, so the cursor's
         * document-space Y position is resolved via the same
         * `document_lines`/`build_visual_rows`/`visual_row_for_line_col`
         * pipeline `render()` and `line_col_from_mouse_position` use —
         * keeping all three in agreement about where each row actually sits.
         *
         * GPUI scroll offsets are ≤ 0: 0 means scrolled to the top, and
         * more-negative values mean the document has been scrolled further down.
         *
         * All positions here are in the same "content space" that
         * `line_col_from_mouse_position` uses: visual row `i`'s top sits at
         * `i * LINE_HEIGHT_PX`, with no padding baked into per-row offsets —
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
        let Some((cursor_top, viewport_h, max_y, offset_x)) = self.cursor_scroll_geometry(cx) else { return };
        let cursor_bottom  = cursor_top + LINE_HEIGHT_PX;
        let margin         = SCROLL_MARGIN_LINES * LINE_HEIGHT_PX;

        let offset         = self.scroll_handle.offset();
        let visible_top    = -offset.y.as_f32();
        let visible_bottom = visible_top + viewport_h;

        if cursor_top < visible_top + margin {
            // Cursor is within `margin` of the top edge (or above it) —
            // scroll up so `margin` worth of buffer opens above the line.
            // Clamped to 0 so this can't scroll past the top of the document
            // just because the margin asked for space that doesn't exist yet.
            let new_y = (margin - cursor_top).clamp(-max_y.max(0.0), 0.0);
            self.scroll_handle.set_offset(point(offset_x, px(new_y)));
        } else if cursor_bottom > visible_bottom - margin {
            // Cursor is within `margin` of the bottom edge (or below it) —
            // scroll down so `margin` worth of buffer opens below the line.
            let new_y = (viewport_h - margin - cursor_bottom).clamp(-max_y.max(0.0), 0.0);
            self.scroll_handle.set_offset(point(offset_x, px(new_y)));
        }
    }

    /// Shared setup for `scroll_to_cursor` and `scroll_to_cursor_centered`:
    /// resolves the cursor's current visual row into content-space Y (same
    /// space `line_col_from_mouse_position` uses), plus the viewport height
    /// and max scroll offset needed to clamp any new offset. `None` when the
    /// scroll handle hasn't been laid out yet (viewport_h <= 0), which can
    /// happen on the very first frame.
    fn cursor_scroll_geometry(&self, cx: &Context<Self>) -> Option<(f32, f32, f32, Pixels)> {
        let state = self.state.read(cx);
        let content = state.active_content().to_string();
        let (cursor_line, cursor_col) = state.cursor_line_col();
        let _ = state;

        let lines = document_lines(&content);
        let rows = visual_rows_for_viewport(cx, &lines, self.scroll_handle.bounds().size.width.as_f32());
        let visual_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
        let cursor_top = visual_row as f32 * LINE_HEIGHT_PX;

        let viewport_h = self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
        if viewport_h <= 0.0 { return None; }

        let max_y = self.scroll_handle.max_offset().y.as_f32();
        Some((cursor_top, viewport_h, max_y, self.scroll_handle.offset().x))
    }

    /// Unlike `scroll_to_cursor` (which only nudges the viewport when the
    /// cursor is near an edge), this always repositions the cursor's line to
    /// the vertical center of the viewport. Used exclusively by the Nav
    /// menu's jump-to-heading (`AppState::jump_to_line`, consumed via
    /// `Tab.pending_scroll_to_cursor` in `render()` below) — landing back on
    /// an already-visible line with no scroll at all reads as "nothing
    /// happened" even though the cursor did move, which defeats the point
    /// of clicking a heading to jump to it.
    fn scroll_to_cursor_centered(&self, cx: &Context<Self>) {
        let Some((cursor_top, viewport_h, max_y, offset_x)) = self.cursor_scroll_geometry(cx) else { return };
        let target_visible_top = cursor_top - (viewport_h - LINE_HEIGHT_PX) / 2.0;
        let new_y = (-target_visible_top).clamp(-max_y.max(0.0), 0.0);
        self.scroll_handle.set_offset(point(offset_x, px(new_y)));
    }

    fn move_cursor_visual_row(&self, cx: &mut Context<Self>, delta: isize, extend: bool) {
        /*
         * Moves the cursor to the visual row `delta` rows above/below its
         * current one (-1/+1 for Up/Down), preserving its on-screen column
         * rather than its logical-line column.
         *
         * Without this, pressing Up from the row directly below a wrapped
         * line would jump to the very first character of the line above
         * (using that *logical* line's column), skipping right past its
         * wrapped continuation rows entirely — landing on the wrong visual
         * spot on screen. This rebuilds the same row table `render()`
         * paints from, so "the row above" here always matches what's
         * actually drawn one row up on screen.
         *
         * No-op past the first/last visual row. `extend` selects between
         * `set_cursor_from_line_col` (Up/Down) and
         * `extend_selection_to_line_col` (Shift+Up/Down), mirroring every
         * other motion's plain/extending pair.
         */
        let state = self.state.read(cx);
        let content = state.active_content().to_string();
        let (cursor_line, cursor_col) = state.cursor_line_col();
        let _ = state;

        let lines = document_lines(&content);
        let rows = visual_rows_for_viewport(cx, &lines, self.scroll_handle.bounds().size.width.as_f32());

        let current_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
        let (_, row_start, _) = rows[current_row];
        let col_in_row = cursor_col - row_start;

        let Some((target_line, target_col)) = visual_row_step(&rows, current_row, col_in_row, delta) else {
            return; // no-op past the first/last visual row
        };

        self.state.update(cx, |state, cx| {
            if extend {
                state.extend_selection_to_line_col(target_line, target_col);
            } else {
                state.set_cursor_from_line_col(target_line, target_col);
            }
            cx.notify();
        });
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Dispatches raw key-down events to `process_key`, which does the
         * actual work — split out so macro replay (`@<register>`, a
         * user-requested feature not part of editor_instructions.md) can
         * re-invoke the exact same dispatch for a recorded keystroke
         * without a real `KeyDownEvent` to hand it.
         */
        let ks = &event.keystroke;
        self.process_key(&ks.key, ks.modifiers.shift, ks.modifiers.control, ks.modifiers.platform, ks.key_char.as_deref(), cx);
    }

    fn process_key(&mut self, key: &str, shift: bool, control: bool, platform: bool, key_char: Option<&str>, cx: &mut Context<Self>) {
        /*
         * The actual key-handling logic `handle_key_down` used to contain
         * directly, now parameterized so both a live `KeyDownEvent` and a
         * replayed macro keystroke (which has no `KeyDownEvent` to unpack)
         * funnel through the same path.
         *
         * Platform-modifier (Ctrl/Cmd) combinations are deliberately passed
         * through so global actions (toggle-settings, new-tab, etc.) can
         * fire normally — and deliberately excluded from macro-recording
         * capture below, an explicit scope decision (macros cover vim's
         * own keystroke stream, not app-global shortcuts).
         * Only pure character input, space, enter, tab, and backspace are
         * consumed. scroll_to_cursor is called at every exit point so the
         * cursor line stays visible regardless of which key moved it.
         */
        if control || platform {
            self.process_key_ctrl_combo(key, shift, cx);
            return;
        }

        // Macro recording capture (user-requested `q`/`@` macros, not part
        // of the written spec): record this keystroke iff a
        // recording was already active *before* dispatch and is *still*
        // active *after* — excluding the `q<register>` pair that starts a
        // recording (not yet active beforehand) and the bare `q` that ends
        // one (no longer active afterward), so only the macro's actual
        // content is captured.
        let was_recording = self.state.read(cx).vim_is_recording_macro();
        // `.`-repeat change capture (spec 5.5) — unlike macro recording,
        // this appends *before* dispatch, since the keystroke that
        // completes the operator (ending the recording) must still be
        // captured; see `vim_is_recording_change`'s doc comment.
        if self.state.read(cx).vim_is_recording_change() {
            self.state.update(cx, |state, _cx| state.record_change_key(key, shift, key_char));
        }
        self.process_key_plain(key, shift, key_char, cx);
        if was_recording && self.state.read(cx).vim_is_recording_macro() {
            self.state.update(cx, |state, _cx| state.record_macro_key(key, shift, key_char));
        }
    }

    fn process_key_ctrl_combo(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) {
        /*
         * Handles Ctrl/Cmd-modified keystrokes — split out of `process_key`
         * so its early `return` doesn't also need to skip macro-recording
         * capture (Ctrl combos are app-global shortcuts, not part of vim's
         * own keystroke stream, and are never recorded into a macro).
         *
         * Copy/Cut/Paste/Undo/Redo/SelectAll/Bold/Underline used to be
         * hardcoded here. They're now configurable GPUI actions
         * (`src/keybinds.rs`, handled in `main_window.rs`) — leaving them
         * here too would have them permanently shadowed anyway: GPUI stops
         * an event's propagation once a keybinding's action handler runs,
         * so this raw key-event path never actually fired for them once a
         * matching binding existed (confirmed the hard way — Ctrl+B here
         * never fired while Ctrl+B was also bound to ToggleSidebar).
         */
        match key {
            "o" => {
                // Ctrl+O: jump list back (spec 5.5). Vim-specific, out of
                // scope for the configurable keybind system.
                self.state.update(cx, |state, _cx| state.vim_jump_backward());
                cx.notify();
                self.scroll_to_cursor(cx);
            }
            "i" => {
                // Ctrl+I: jump list forward (spec 5.5). Vim-specific, out of
                // scope for the configurable keybind system.
                self.state.update(cx, |state, _cx| state.vim_jump_forward());
                cx.notify();
                self.scroll_to_cursor(cx);
            }
            // Ctrl+Left/Right jump by word; Ctrl+Home/End jump to document start/end
            // (spec 4.1). Shift+Ctrl+<key> extends the selection instead of just
            // moving (spec 4.3). Plain (unmodified) arrow/Home/End are handled below.
            "left" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_word_backward() } else { state.move_word_backward() }
                });
                cx.notify();
            }
            "right" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_word_forward() } else { state.move_word_forward() }
                });
                cx.notify();
            }
            "home" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_doc_start() } else { state.move_doc_start() }
                });
                cx.notify();
            }
            "end" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_doc_end() } else { state.move_doc_end() }
                });
                cx.notify();
            }
            _ => {} // Ctrl+S, Ctrl+T, Ctrl+W, etc. handled by global actions
        }
        self.scroll_to_cursor(cx);
    }

    fn process_key_plain(&mut self, key: &str, shift: bool, key_char: Option<&str>, cx: &mut Context<Self>) {
        /*
         * Handles every non-Ctrl/Cmd keystroke: vim-mode routing plus the
         * plain-editor fallback. Split out of `process_key` so macro
         * recording can wrap this call without also capturing Ctrl combos
         * (handled separately by `process_key_ctrl_combo`).
         */
        // Vim mode routing (Task D). Insert mode behaves like the plain
        // editor below except for Escape, which nothing in the plain-editor
        // match block otherwise handles. The other four modes route through
        // handle_vim_key first; it returns false only for Normal-mode
        // navigation keys it deliberately lets fall through (see its own
        // doc comment) — everything else it returns true for is fully
        // handled here and shouldn't reach the plain-editor logic below.
        let (vim_enabled, vim_mode) = {
            let state = self.state.read(cx);
            let mode = state.tabs.get(state.active_tab).map(|t| t.vim_mode).unwrap_or_default();
            (state.vim_enabled, mode)
        };
        if vim_enabled {
            if vim_mode == VimMode::Insert {
                if key == "escape" {
                    self.state.update(cx, |state, _cx| state.vim_exit_to_normal());
                    cx.notify();
                    self.scroll_to_cursor(cx);
                    return;
                }
                // else: fall through to the plain-editor handling below.
            } else {
                // 'j'/'k' need the current viewport's wrap layout (GPUI
                // context `handle_vim_key` doesn't have), so they're
                // special-cased here rather than dispatched through
                // AppState — mirroring how plain Up/Down are handled below,
                // and reusing the same visual-row-aware movement so j/k
                // feel identical to the arrow keys on this app's wrapped
                // content rather than vim's logical-line semantics (a
                // deliberate UX choice for this heavily-wrapping app).
                // Intercepted in Normal mode (moves the cursor) and Visual/
                // VisualLine (extends the selection, spec 5.6) with no
                // pending find/gg trigger *and* no pending `d`/`y`/`c`
                // operator — otherwise 'j'/'k' must reach `handle_vim_key`
                // so a pending `f`/`t` can treat them as a target character
                // (e.g. completing `fj`), or a pending operator can abandon
                // itself cleanly via `complete_vim_operator` (without this,
                // `dj` would silently move the cursor via
                // `move_cursor_visual_row` below and leave `d` dangling for
                // the *next* keystroke to complete instead). Also gated on
                // `!shift` (Task I) — shift+j is `J` (join lines, spec
                // 5.5), a completely different command that must reach
                // `handle_vim_key` instead of being swallowed as "move down".
                let no_pending_trigger = self.state.read(cx).vim_pending_trigger().is_none()
                    && self.state.read(cx).vim_pending_operator().is_none();
                let is_visual = matches!(vim_mode, VimMode::Visual | VimMode::VisualLine);
                if (vim_mode == VimMode::Normal || is_visual) && no_pending_trigger && !shift && (key == "j" || key == "k") {
                    let count = self.state.update(cx, |state, _cx| state.take_vim_count()).unwrap_or(1);
                    let delta: isize = if key == "k" { -1 } else { 1 };
                    for _ in 0..count {
                        self.move_cursor_visual_row(cx, delta, is_visual);
                    }
                    self.scroll_to_cursor(cx);
                    return;
                }

                // H/M/L: top/middle/bottom of the *visible* viewport (spec
                // 5.2) — needs the live scroll offset and visual-row
                // layout, same GPUI-context reason as j/k above. Resolves
                // down to a plain logical line number and hands off to
                // `vim_move_to_line_first_nonblank`, which doesn't need to
                // know anything about viewports.
                if (vim_mode == VimMode::Normal || is_visual) && no_pending_trigger
                    && shift && matches!(key, "h" | "m" | "l")
                {
                    let content = self.state.read(cx).active_content().to_string();
                    let lines = document_lines(&content);
                    let bounds = self.scroll_handle.bounds();
                    let rows = visual_rows_for_viewport(cx, &lines, bounds.size.width.as_f32());
                    if !rows.is_empty() {
                        let viewport_h = bounds.size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
                        let offset = self.scroll_handle.offset();
                        let top_row = ((-offset.y.as_f32()) / LINE_HEIGHT_PX).floor().max(0.0) as usize;
                        let top_row = top_row.min(rows.len() - 1);
                        let visible_count = ((viewport_h / LINE_HEIGHT_PX).floor().max(1.0)) as usize;
                        let bottom_row = (top_row + visible_count.saturating_sub(1)).min(rows.len() - 1);
                        let target_row = match key {
                            "h" => top_row,
                            "l" => bottom_row,
                            "m" => top_row + (bottom_row - top_row) / 2,
                            _ => unreachable!(),
                        };
                        let target_line = rows[target_row].0;
                        self.state.update(cx, |state, cx| {
                            state.vim_move_to_line_first_nonblank(target_line, is_visual);
                            cx.notify();
                        });
                        cx.notify();
                        self.scroll_to_cursor(cx);
                        return;
                    }
                }

                // `@`/`@<register>`/`@@` macro replay (user-requested, not
                // part of editor_instructions.md) — kept entirely here
                // rather than in `AppState::handle_vim_key`
                // since replaying re-enters `process_key` with full GPUI
                // context, which `AppState` doesn't have. Normal-mode only
                // (unlike `q` recording start/stop, which — being purely
                // state bookkeeping with no GPUI dependency — lives in
                // `AppState` and is reachable from Visual mode too via the
                // shared dispatcher; narrowing replay to Normal mode is a
                // deliberate, documented scope limit for this pass).
                if vim_mode == VimMode::Normal && no_pending_trigger {
                    if self.macro_at_pending {
                        self.macro_at_pending = false;
                        if let Some(register) = vim_find_target_char(key, shift, key_char) {
                            let register = if register == '@' {
                                self.state.read(cx).vim_last_macro_register
                            } else {
                                Some(register)
                            };
                            if let Some(register) = register {
                                self.replay_macro(register, cx);
                            }
                        }
                        self.scroll_to_cursor(cx);
                        return;
                    }
                    if matches_shifted_symbol(key, shift, key_char, "2", "@") {
                        self.macro_at_pending = true;
                        return;
                    }
                }

                // `"+p`/`"+P` (spec 5.8's clipboard register, read
                // direction): `state.rs` can't reach the OS clipboard
                // itself, so when the `+` register is about to be pasted
                // from, read it here (this is the only layer with `cx`)
                // and stage it into the register the ordinary,
                // GPUI-unaware paste path already knows how to read.
                if (key == "p") && self.state.read(cx).vim_selected_register() == Some('+') {
                    if let Some(item) = cx.read_from_clipboard() {
                        if let Some(text) = item.text() {
                            self.state.update(cx, |state, _cx| state.set_register('+', text.to_string()));
                        }
                    }
                }

                let (consumed, clipboard_sync) = self.state.update(cx, |state, cx| {
                    let handled = state.handle_vim_key(key, shift, key_char);
                    if handled { cx.notify(); }
                    (handled, state.take_pending_clipboard_sync())
                });
                // `"+y`/`"+d`/`"+c` (write direction): mirrors the read
                // direction above — `execute_vim_operator_range` stages the
                // text in `pending_clipboard_sync` when the `+` register
                // was targeted; this is the only place with `cx` to
                // actually push it onto the OS clipboard.
                if let Some(text) = clipboard_sync {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                if consumed {
                    cx.notify();
                    self.scroll_to_cursor(cx);
                    return;
                }
                // else: a Normal-mode navigation key fell through — continue
                // below to the same handling the plain editor uses.
            }
        }

        // Up/Down move by *visual* row (not logical line) so wrapped lines'
        // continuation rows are reachable — handled separately from the
        // match below since it needs the current viewport's wrap layout,
        // not just a plain AppState mutation. Extends instead of moving
        // when Shift is held (the plain editor's own convention) OR vim is
        // in Visual/VisualLine mode — reached here (rather than being
        // handled above) precisely when vim's own j/k branch let Up/Down
        // fall through, which requires extending too or it would silently
        // clear the active selection via a plain, non-extending move.
        if key == "up" || key == "down" {
            let vim_visual = vim_enabled && matches!(vim_mode, VimMode::Visual | VimMode::VisualLine);
            let delta = if key == "up" { -1 } else { 1 };
            self.move_cursor_visual_row(cx, delta, shift || vim_visual);
            self.scroll_to_cursor(cx);
            return;
        }

        let consumed = self.state.update(cx, |state, cx| {
            match key {
                "backspace" => { state.backspace(); cx.notify(); true }
                "enter"     => { state.insert_char('\n'); cx.notify(); true }
                "space"     => { state.insert_char(' '); cx.notify(); true }
                "tab"       => { state.insert_char('\t'); cx.notify(); true }
                // Shift+<key> extends the selection instead of moving plainly (spec 4.3).
                "left"      => { if shift { state.extend_left() } else { state.move_left() }; cx.notify(); true }
                "right"     => { if shift { state.extend_right() } else { state.move_right() }; cx.notify(); true }
                "home"      => { if shift { state.extend_line_start() } else { state.move_line_start() }; cx.notify(); true }
                "end"       => { if shift { state.extend_line_end() } else { state.move_line_end() }; cx.notify(); true }
                k if k.chars().count() == 1 => {
                    let mut ch = k.chars().next().unwrap();
                    // Apply shift for uppercase; GPUI gives lowercase key names
                    if shift && ch.is_alphabetic() {
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

    fn replay_macro(&mut self, register: char, cx: &mut Context<Self>) {
        /*
         * Replays a recorded macro (`@<register>`) by feeding
         * its captured keystrokes back through `process_key` one at a
         * time, in order — the same function a live keypress reaches, so
         * replay re-triggers the exact same mode-aware routing (Insert/
         * Normal/Visual, motions, H/M/L, j/k, etc.) a real keystroke would.
         *
         * The key vector is read and cloned *before* the loop starts, with
         * that borrow fully released before any `process_key` call — each
         * of those does its own `self.state.update`/`read`, and GPUI
         * panics if one of those runs while another is still open on the
         * same entity, which would happen if this loop were written inside
         * a `self.state.update(...)` closure instead.
         */
        self.state.update(cx, |state, _cx| { state.vim_last_macro_register = Some(register); });
        let Some(keys) = self.state.read(cx).macro_keys(register) else { return };
        for k in keys {
            self.process_key(&k.key, k.shift, false, false, k.key_char.as_deref(), cx);
        }
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the editor as a focusable, scrollable column.
         *
         * Content is split on '\n' into logical lines, then each logical
         * line is word-wrapped into one or more fixed-height visual rows via
         * `build_visual_rows` — this is what actually fixes long lines
         * running off the right edge instead of wrapping. One div is
         * painted per visual row, not per logical line, which keeps every
         * row exactly `LINE_HEIGHT_PX` tall so click-to-position and
         * scroll-to-cursor's pixel math (which assume a fixed row height)
         * stay correct even when lines wrap.
         *
         * The row `tab.cursor` actually points into is rendered as three
         * inline spans (text before / cursor cell / text after) so the cursor
         * marker sits at the real character position, rather than always
         * trailing the last line regardless of where the cursor is.
         *
         * Clicking anywhere in the editor reclaims keyboard focus.
         */
        // Nav menu jump (state.rs's `jump_to_line`): FileExplorer has no
        // direct reference to this view to call a scroll method on, so it
        // leaves a flag on the active tab instead. Honor and clear it here,
        // before laying out this frame — always centering (not the regular
        // edge-triggered scroll_to_cursor) so clicking an already-visible
        // heading still visibly does something.
        let should_scroll = self.state.update(cx, |state, _cx| {
            let active = state.active_tab;
            if let Some(tab) = state.tabs.get_mut(active) {
                if tab.pending_scroll_to_cursor {
                    tab.pending_scroll_to_cursor = false;
                    return true;
                }
            }
            false
        });
        if should_scroll {
            self.scroll_to_cursor_centered(cx);
        }

        let state = self.state.read(cx);
        let content = state.active_content().to_string();
        // Rich-text formatting (Phase 1): each logical line is exactly one
        // paragraph (§1 of formatting_todo.md), so `paragraphs[i]` gives
        // `lines[i]`'s formatting runs directly — no separate lookup needed.
        let paragraphs: Vec<Paragraph> = state.tabs.get(state.active_tab).map(|t| t.paragraphs.clone()).unwrap_or_default();
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
        // Mode indicator text. Deviates from spec 5.1's literal "nothing
        // shown for Normal" — showing `-- NORMAL --` removes the ambiguity
        // between "vim is on and in Normal mode" and "vim mode is off
        // entirely", both of which otherwise render an identical blank
        // indicator strip.
        let mode_indicator_text: Option<&'static str> = if state.vim_enabled {
            state.tabs.get(state.active_tab).map(|t| match t.vim_mode {
                VimMode::Normal => "-- NORMAL --",
                VimMode::Insert => "-- INSERT --",
                VimMode::Visual => "-- VISUAL --",
                VimMode::VisualLine => "-- VISUAL LINE --",
                VimMode::Command => "-- COMMAND --",
                VimMode::Replace => "-- REPLACE --",
                VimMode::Search => "-- SEARCH --",
            })
        } else {
            None
        };
        // Echoes every in-progress "waiting for the next key" state next
        // to the mode label — not just `vim_command_buf`'s own count/
        // pending-trigger grammar (`3f`), but also the pending states
        // Task F/G added afterward that deliberately live in *separate*
        // fields rather than `vim_command_buf` (to avoid colliding with
        // its existing grammar — see e.g. `start_vim_operator`'s doc
        // comment): a pending `d`/`y`/`c`/`>`/`<`/`gU`/`gu` operator, an
        // `i`/`a` text-object prefix after one, and `q`/`@` macro
        // record/replay's own pending-register state. Concretely, this
        // string is a UI-only concern — it's built by concatenating
        // whichever of these happen to be active; the underlying
        // functionality (recording, replaying, running operators) already
        // worked correctly without it, confirmed by testing after this
        // fix was requested — this closes a *feedback* gap, not a
        // functional one, matching what "no visual on the command mode
        // line" while everything actually worked turned out to mean.
        // Also shows "recording @<register>" for the whole duration of an
        // active recording (real vim does this too), not just the initial
        // `q<register>` keystroke. In Command mode (Task H), shows the
        // live `:command` text instead of the Normal/Visual pending-state
        // echo, since the two are mutually exclusive by construction (only
        // one `vim_mode` is active at a time). A `vim_command_error` from
        // the last dispatched command (e.g. `:q` refused on unsaved
        // changes, or an unrecognized command) is appended in any mode
        // until the next `:` is opened, matching real vim's persistent
        // error line.
        let pending_command_text: Option<String> = state
            .tabs
            .get(state.active_tab)
            .map(|t| {
                let mut buf = if t.vim_mode == VimMode::Command {
                    format!(":{}", t.vim_command_line)
                } else if t.vim_mode == VimMode::Search {
                    let prefix = if t.vim_search_direction { '/' } else { '?' };
                    format!("{prefix}{}", t.vim_command_line)
                } else {
                    let mut buf = t.vim_command_buf.clone();
                    if let Some(operator) = t.vim_pending_operator {
                        buf.push(operator);
                        if let Some(inner) = t.vim_pending_text_object_prefix {
                            buf.push(if inner { 'i' } else { 'a' });
                        }
                    }
                    if state.vim_macro_record_pending() {
                        buf.push('q');
                    }
                    if self.macro_at_pending {
                        buf.push('@');
                    }
                    if let Some(register) = state.vim_recording_register() {
                        buf.push_str(&format!(" [recording @{register}]"));
                    }
                    buf
                };
                if let Some(err) = &t.vim_command_error {
                    if !buf.is_empty() { buf.push(' '); }
                    buf.push_str(err);
                }
                buf
            })
            .filter(|buf| !buf.is_empty());
        let _ = state;

        let is_focused = self.focus_handle.is_focused(window);

        let lines = document_lines(&content);
        let line_chars: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();

        // Byte offset of each logical line's start within `content`, needed
        // to test `selection` (a document-wide byte range) against each line.
        let mut line_byte_starts: Vec<usize> = Vec::with_capacity(lines.len());
        let mut byte_offset = 0;
        for l in &lines {
            line_byte_starts.push(byte_offset);
            byte_offset += l.len() + 1; // +1 for the '\n' the split() consumed
        }

        // Word-wrap each logical line into fixed-height visual rows so long
        // lines reflow within the viewport instead of running off the right
        // edge. Using the viewport's own current bounds (not a hardcoded
        // width) keeps wrapping correct across window resizes. Click/drag
        // hit-testing and scroll-to-cursor rebuild this exact same row table
        // (via the same helper functions) so all three always agree on
        // where each row's boundaries fall.
        let rows = visual_rows_for_viewport(cx, &lines, self.scroll_handle.bounds().size.width.as_f32());
        let cursor_visual_row = is_focused.then(|| visual_row_for_line_col(&rows, cursor_line, cursor_col));

        // Outer wrapper: takes the same slot in main_window's flex row the
        // scrollable editor div used to occupy directly (`.flex_1()`,
        // `.min_w_0()`, `.min_h_0()` all moved here from that div below), and
        // stacks [scrollable editor, mode indicator] as siblings in a column.
        // The indicator must be a *sibling* of the scrollable div, not nested
        // inside it — nesting it inside would make it scroll with content and
        // perturb `scroll_handle.bounds()`/`max_offset()`, which
        // `scroll_to_cursor` and the wrap math both depend on reflecting only
        // the editor's own viewport.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
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
                cx.stop_propagation();
                this.focus_handle.clone().focus(window, cx);
                let bounds = this.scroll_handle.bounds();
                let scroll_y = this.scroll_handle.offset().y.as_f32();
                let content = this.state.read(cx).active_content().to_string();
                let lines = document_lines(&content);
                let rows = visual_rows_for_viewport(cx, &lines, bounds.size.width.as_f32());
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, &rows);
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
                let content = this.state.read(cx).active_content().to_string();
                let lines = document_lines(&content);
                let rows = visual_rows_for_viewport(cx, &lines, bounds.size.width.as_f32());
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, &rows);
                this.state.update(cx, |state, cx| {
                    state.extend_selection_to_line_col(line, col);
                    cx.notify();
                });
                this.auto_scroller.notify(ev.position, window);
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
            // Critical (see main_window.rs's `min_h_0` comment for the same
            // pattern): this div is now a flex_1 child on a flex_col's main
            // axis (its parent wrapper, added by this task) rather than a
            // cross-axis-stretched flex_row child like before — on the main
            // axis a flex item's default min-height is its content size, so
            // without this a document taller than the viewport could grow
            // the div past the wrapper's allocated height instead of
            // scrolling internally.
            .min_h_0()
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
                    // w_full constrains each row to the editor width so wrapped
                    // rows all line up under one another.
                    .w_full()
                    // Placeholder shown on an empty, unsaved tab
                    .when(is_new_tab, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x555555))
                                .font_family(FONT_FAMILY)
                                .child("Open a file from the sidebar, or start typing…"),
                        )
                    })
                    // One div per visual row (a wrapped logical line spans
                    // several); render_line overlays the cursor marker and/or
                    // selection highlight on whichever rows they actually touch.
                    .children(rows.iter().enumerate().map(|(visual_idx, &(li, row_start, row_end))| {
                        let chars = &line_chars[li];
                        let row_text: String = chars[row_start..row_end].iter().collect();

                        // `.then(|| ...)` (lazy), not `.then_some(...)` — the latter's
                        // argument is a plain value, evaluated eagerly *before* the
                        // bool is even checked. With `then_some`, `cursor_col - row_start`
                        // was computed for every row regardless of the condition, and
                        // underflowed (panicked) on any row whose row_start exceeded the
                        // cursor's column — i.e. almost any row that isn't the cursor's own.
                        let row_cursor_col = (cursor_visual_row == Some(visual_idx))
                            .then(|| cursor_col - row_start);

                        // Clip the logical line's selection char-range (if any) down
                        // to this row's own [row_start, row_end) sub-range, then
                        // rebase it to be relative to the row instead of the line.
                        let row_selection = selection
                            .and_then(|(s, e)| selection_span_for_line(&lines[li], line_byte_starts[li], s, e))
                            .and_then(|(sel_start, sel_end)| {
                                let clipped_start = sel_start.max(row_start);
                                let clipped_end = sel_end.min(row_end);
                                // Same eager-vs-lazy pitfall as row_cursor_col above: use
                                // `.then(|| ...)` since clipped_end can be < row_start when
                                // the selection doesn't reach this row, which would
                                // underflow `clipped_end - row_start` if evaluated eagerly.
                                (clipped_start < clipped_end)
                                    .then(|| (clipped_start - row_start, clipped_end - row_start))
                            });

                        // Rich-text formatting (Phase 1): clip this logical
                        // line's paragraph run boundaries down to this row's
                        // own [row_start, row_end) sub-range, same rebasing
                        // pattern as `row_selection` above — a wrapped row
                        // only needs to know about the runs it actually spans.
                        let row_run_spans: Vec<(usize, usize, usize)> = paragraphs
                            .get(li)
                            .map(|p| paragraph_run_char_spans(p))
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|(rs, re, run_idx)| {
                                let clipped_start = rs.max(row_start);
                                let clipped_end = re.min(row_end);
                                (clipped_start < clipped_end)
                                    .then(|| (clipped_start - row_start, clipped_end - row_start, run_idx))
                            })
                            .collect();

                        // Check if previous paragraph also has box_format (for merging boxes)
                        let prev_has_box = li > 0 && paragraphs.get(li - 1)
                            .is_some_and(|p| p.runs.iter().any(|r| r.box_format));

                        let content_el = render_line(
                            &row_text,
                            row_cursor_col,
                            row_selection,
                            &row_run_spans,
                            paragraphs.get(li),
                            prev_has_box,
                        );
                        // Heading styles (spec 6.5): a paragraph-wide default
                        // that per-run formatting (bold/size/etc., applied
                        // inside `content_el`'s own children) still overrides
                        // for the specific characters it covers, since GPUI's
                        // text style cascades to children and a child's own
                        // call wins. NOTE: a heading's larger font size can
                        // visually overflow this row's fixed `LINE_HEIGHT_PX`
                        // — row height doesn't adjust for it, since that
                        // height feeds click/scroll pixel math built for a
                        // uniform row size; needs real-hardware verification
                        // (this sandbox has no display) to see how it looks.
                        let heading = paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                        let row_div = div()
                            .font_family(FONT_FAMILY)
                            .text_sm()
                            .text_color(rgb(0xd4d4d4));
                        let row_div = match heading_font_size_px(heading) {
                            Some(size) => row_div.text_size(px(size)).font_weight(FontWeight::BOLD),
                            None => row_div,
                        };
                        row_div
                            // Locks this row's height so wrapping stays fully
                            // decided by `wrap_line_into_rows` up front — nowrap
                            // stops GPUI from *also* word-wrapping this row's text
                            // internally if CHAR_WIDTH_PX's monospace estimate
                            // ever slightly overshoots the real glyph width, which
                            // would otherwise grow this div past one row and break
                            // the fixed-row-height assumption click/scroll math relies on.
                            .whitespace_nowrap()
                            // min_h keeps empty rows visually present
                            .min_h(px(LINE_HEIGHT_PX))
                            .child(content_el)
                    }))
            ) // closes the lines-container .child(...)
            ) // closes the scrollable-editor .child(...) on the wrapper
            .child({
                // Mode indicator (spec 5.1) — a sibling below the scrollable
                // editor div, at a fixed height so switching modes doesn't
                // resize (and re-wrap) the editor's own viewport. The
                // in-progress command/count buffer (e.g. "3f"), when
                // present, is appended after the mode label on the same
                // line, matching real vim's bottom-right pending-keys echo.
                let mut line = mode_indicator_text.unwrap_or("").to_string();
                if let Some(pending) = &pending_command_text {
                    if !line.is_empty() { line.push(' '); }
                    line.push_str(pending);
                }
                div()
                    .h(px(LINE_HEIGHT_PX))
                    .px(px(16.0))
                    .bg(rgb(0x1e1e1e))
                    .font_family(FONT_FAMILY)
                    .text_sm()
                    .text_color(rgb(0xd4d4d4))
                    .child(line)
            })
    }
}

fn render_line(
    line: &str,
    cursor_col: Option<usize>,
    selection: Option<(usize, usize)>,
    run_spans: &[(usize, usize, usize)],
    para: Option<&Paragraph>,
    prev_has_box: bool,
) -> AnyElement {
    /*
     * Renders one (visual-row-clipped) line of text. Splits into
     * `(run_start, run_end, run_idx)` chunks per the paragraph's formatting
     * runs (spec 6.2, rich-text formatting plan Phase 1), then further
     * splits *within* each chunk via the existing `line_segments` wherever
     * the cursor and/or selection touch it — the two concerns are
     * orthogonal (which run a character's formatting comes from vs.
     * whether it's under the cursor/selection), so composing them as an
     * outer-run/inner-cursor split avoids needing one function that
     * understands both at once.
     *
     * Falls back to a single plain-text child when there's exactly one run
     * and no cursor/selection touches this row, matching the cheap path
     * every untouched line already took before formatting existed. An
     * empty `run_spans` (formatting not available for this row, e.g. a
     * brand-new tab with no parsed paragraphs) is treated as one big
     * unformatted run spanning the whole line, so cursor/selection
     * rendering never silently breaks when formatting data is absent.
     */
    let chars: Vec<char> = line.chars().collect();

    if cursor_col.is_none() && selection.is_none() {
        if run_spans.is_empty() {
            return line.to_string().into_any_element();
        }
        if let [(start, end, run_idx)] = run_spans {
            if *start == 0 && *end == chars.len() {
                let run = para.and_then(|p| p.runs.get(*run_idx));
                if run.is_none() {
                    return line.to_string().into_any_element();
                }
                // Don't take the fast path if alignment is needed — fall through to normal rendering
                use crate::docx_parser::Alignment;
                let needs_alignment = para.is_some_and(|p| !matches!(p.alignment, Alignment::Left));
                if !needs_alignment {
                    return apply_run_style(div(), run).child(line.to_string()).into_any_element();
                }
            }
        }
    }

    let effective_spans: Vec<(usize, usize, usize)> = if run_spans.is_empty() {
        vec![(0, chars.len(), usize::MAX)]
    } else {
        run_spans.to_vec()
    };

    let spans: Vec<AnyElement> = effective_spans
        .into_iter()
        .flat_map(|(run_start, run_end, run_idx)| {
            let run = para.and_then(|p| p.runs.get(run_idx));
            let sub_len = run_end - run_start;
            let sub_cursor = cursor_col.filter(|&c| c >= run_start && c <= run_end).map(|c| c - run_start);
            let sub_selection = selection.and_then(|(s, e)| {
                let (clipped_start, clipped_end) = (s.max(run_start), e.min(run_end));
                (clipped_start < clipped_end).then(|| (clipped_start - run_start, clipped_end - run_start))
            });
            let segments = line_segments(sub_len, sub_cursor, sub_selection);
            segments
                .into_iter()
                .map(|(start, end, style)| {
                    // A zero-width segment only ever occurs for the cursor
                    // sitting past the last character (end of line) — render
                    // it as a single space so the highlighted cell still has
                    // visible width.
                    let text: String = if start == end {
                        " ".to_string()
                    } else {
                        chars[run_start + start..run_start + end].iter().collect()
                    };
                    render_segment(text, run, style)
                })
                .collect::<Vec<_>>()
        })
        .collect();

    let mut line_div = div().flex().flex_row().children(spans);
    // Apply paragraph-level alignment if available (Phase 4.3)
    if let Some(p) = para {
        use crate::docx_parser::Alignment;
        line_div = match p.alignment {
            Alignment::Center => line_div.justify_center(),
            Alignment::Right => line_div.justify_end(),
            Alignment::Justify => line_div.justify_between(), // approximate for now
            Alignment::Left => line_div.justify_start(),
        };

        // Check if any run has box_format (Pocket formatting)
        // Wrap in full-width box container so box stays at full width while content is aligned
        // Increased vertical padding to create visual separation between consecutive Pockets
        let has_box = p.runs.iter().any(|r| r.box_format);
        if has_box {
            let mut box_div = div()
                .w_full()
                .border_color(rgb(0xd4d4d4))
                .px(px(8.0))
                .py(px(8.0))
                .child(line_div);

            // If previous line also has a box, merge them by removing top border
            if prev_has_box {
                box_div = box_div.border_b_1().border_l_1().border_r_1();
            } else {
                box_div = box_div.border_1();
            }

            return box_div.into_any_element();
        }
    }
    line_div.into_any_element()
}

fn render_segment(text: String, run: Option<&Run>, style: SegmentStyle) -> AnyElement {
    /*
     * Applies the run's formatting first, then layers the cursor/selection
     * overlay on top — each of GPUI's style calls simply overwrites the
     * previous value for that field (confirmed against `Styled`'s own
     * implementation), so applying the overlay's `.bg()`/`.text_color()`
     * *after* the run's own correctly makes it win, matching real editors
     * drawing the cursor/selection on top of a highlight rather than
     * underneath it.
     *
     * Use flex_shrink(0.0) to prevent the div from expanding beyond the text width,
     * so highlights only extend as far as the text itself.
     */
    let el = apply_run_style(div().flex_shrink(0.0), run);
    let el = match style {
        SegmentStyle::Cursor => el.bg(rgb(0xd4d4d4)).text_color(rgb(0x1e1e1e)),
        // #264F78 at ~50% opacity, per spec 6.4's selection-highlight color.
        SegmentStyle::Selection => el.bg(rgba(0x264F7880)),
        SegmentStyle::Plain => el,
    };
    el.child(text).into_any_element()
}

fn apply_run_style(el: Div, run: Option<&Run>) -> Div {
    /*
     * Maps a `Run`'s fields onto GPUI style calls per spec 6.2 (extended
     * with italic/font/color, rich-text formatting plan Phase 1's scope
     * decision). `run: None` (formatting data unavailable for this
     * position) leaves `el` untouched, rendering as plain text.
     */
    let Some(run) = run else { return el };
    let mut el = el;
    if run.bold { el = el.font_weight(FontWeight::BOLD); }
    if run.italic { el = el.italic(); }
    if run.underline { el = el.underline(); }
    if run.double_underline { el = el.underline(); }
    // ponytail: strikethrough data is stored and toggled, rendering deferred until GPUI supports text decoration
    // Note: box_format is applied at the line level in render_line(), not here at the run level
    if run.highlight {
        let base_hex = highlight_color_hex(&run.highlight_color);
        let text_hex = run.color.as_deref()
            .and_then(|c| u32::from_str_radix(c, 16).ok())
            .unwrap_or(DEFAULT_TEXT_COLOR_HEX);
        // Word darkens light highlights under light text in dark mode so
        // the text stays legible (e.g. white text on yellow highlight is
        // otherwise unreadable) — this app is always dark-themed, so the
        // check is unconditional rather than gated on a light/dark toggle.
        let highlight_hex = if is_light_color(base_hex) && is_light_color(text_hex) {
            darken_for_light_text(base_hex)
        } else {
            base_hex
        };
        el = el.bg(rgb(highlight_hex));
    }
    if run.size > 0 {
        el = el.text_size(px(run.size as f32 / 2.0));
    }
    if let Some(font) = &run.font {
        el = el.font_family(font.clone());
    }
    if let Some(color) = &run.color {
        if let Ok(value) = u32::from_str_radix(color, 16) {
            el = el.text_color(rgb(value));
        }
    }
    el
}

fn highlight_color_hex(name: &str) -> u32 {
    /*
     * Maps Word's highlight color names to their GPUI hex value (spec
     * 6.2's 15-entry table, plus a fallback for anything unrecognized).
     */
    match name {
        "yellow" => 0xFFD700,
        "green" => 0x00FF00,
        "cyan" => 0x00FFFF,
        "magenta" => 0xFF00FF,
        "red" => 0xFF0000,
        "darkBlue" => 0x00008B,
        "darkCyan" => 0x008B8B,
        "darkGreen" => 0x006400,
        "darkMagenta" => 0x8B008B,
        "darkRed" => 0x8B0000,
        "darkYellow" => 0x8B8B00,
        "darkGray" => 0xA9A9A9,
        "lightGray" => 0xD3D3D3,
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        _ => 0x888888,
    }
}

fn relative_luminance(hex: u32) -> f32 {
    /*
     * Standard perceived-luminance weighting (ITU-R BT.709 coefficients),
     * used to decide whether a color reads as "light" (spec: bug fix,
     * darken highlight under light text, matching Word's dark-mode
     * behavior).
     */
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn is_light_color(hex: u32) -> bool {
    relative_luminance(hex) > 0.5
}

fn darken_for_light_text(hex: u32) -> u32 {
    /*
     * Scales each channel down uniformly (preserving hue) so a light
     * highlight color stops washing out light-colored text on top of it.
     */
    const SCALE: f32 = 0.4;
    let r = (((hex >> 16) & 0xFF) as f32 * SCALE) as u32;
    let g = (((hex >> 8) & 0xFF) as f32 * SCALE) as u32;
    let b = ((hex & 0xFF) as f32 * SCALE) as u32;
    (r << 16) | (g << 8) | b
}

fn heading_font_size_px(heading: u8) -> Option<f32> {
    /*
     * Spec 6.5's heading-level font size table. `None` for `heading == 0`
     * (body text — no override).
     */
    match heading {
        0 => None,
        1 => Some(24.0),
        2 => Some(20.0),
        3 => Some(18.0),
        4..=6 => Some(16.0),
        _ => Some(14.0), // 7-9
    }
}

fn usable_wrap_width(viewport_width_px: f32) -> f32 {
    /*
     * Computes how many pixels of width are available for wrapping text,
     * given the current viewport pixel width. Subtracts the left+right
     * content padding so it matches the actual usable text area (mirrors
     * CONTENT_PADDING_PX's use elsewhere).
     *
     * Returns a sentinel of `f32::MAX` when the viewport hasn't been laid
     * out yet (width <= 0, which happens on the very first frame before
     * `scroll_handle.bounds()` has real numbers) so lines render unwrapped
     * for that one frame instead of collapsing to almost nothing.
     */
    let usable = viewport_width_px - 2.0 * CONTENT_PADDING_PX;
    if usable <= 0.0 { f32::MAX } else { usable }
}

fn char_width_fn(cx: &App, font: Font) -> impl Fn(char) -> f32 {
    /*
     * Builds a closure that returns a character's real, rendered pixel
     * width for `font` at `FONT_SIZE_PX`, backed by GPUI's own
     * `TextSystem::layout_width` (the same glyph-shaping measurement GPUI
     * itself uses to paint text, cached internally per character).
     *
     * This replaces the old approach of assuming every character is
     * `CHAR_WIDTH_PX` wide for wrap purposes: that uniform estimate is
     * systematically wrong for narrow glyphs like '.' or '-', which render
     * much thinner than the average — folding lines dominated by them
     * (e.g. citation ellipses, en-dashes) far earlier than their actual
     * on-screen width warrants.
     *
     * The returned closure owns its own `Arc<TextSystem>` clone (cheap —
     * it's a refcount bump) and a resolved `FontId`, so it doesn't borrow
     * `cx` and can be passed freely into the pure wrap functions below.
     */
    let text_system = cx.text_system().clone();
    let font_id = text_system.resolve_font(&font);
    move |c: char| text_system.layout_width(font_id, px(FONT_SIZE_PX), c).as_f32()
}

pub(crate) fn visual_rows_for_viewport(cx: &App, lines: &[String], viewport_width_px: f32) -> Vec<(usize, usize, usize)> {
    /*
     * Convenience wrapper combining `char_width_fn` + `usable_wrap_width` +
     * `build_visual_rows` — the single entry point every cx-having call
     * site (render, click/drag hit-testing, scroll-to-cursor, Up/Down,
     * auto-scroll) uses to build the row table, so they can never disagree
     * about wrap width or glyph metrics.
     */
    let width_of = char_width_fn(cx, font(FONT_FAMILY));
    build_visual_rows(lines, usable_wrap_width(viewport_width_px), &width_of)
}

fn wrap_line_into_rows(chars: &[char], wrap_width_px: f32, width_of: &impl Fn(char) -> f32) -> Vec<(usize, usize)> {
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
            let char_width = width_of(chars[i]);
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
        row_start = if last_space.is_some() { row_end + 1 } else { row_end };
    }
    rows
}

fn build_visual_rows(lines: &[String], wrap_width_px: f32, width_of: &impl Fn(char) -> f32) -> Vec<(usize, usize, usize)> {
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
        for (start, end) in wrap_line_into_rows(&chars, wrap_width_px, width_of) {
            rows.push((li, start, end));
        }
    }
    rows
}

fn visual_row_for_line_col(rows: &[(usize, usize, usize)], logical_line: usize, char_col: usize) -> usize {
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
        if li != logical_line { continue; }
        last_row_of_line = idx;
        if char_col >= start && char_col < end {
            return idx;
        }
        if char_col == end {
            let next_is_contiguous = rows.get(idx + 1)
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

fn visual_row_step(rows: &[(usize, usize, usize)], current_row: usize, col_in_row: usize, delta: isize) -> Option<(usize, usize)> {
    /*
     * Steps `delta` visual rows away from `current_row` (-1/+1 for Up/Down),
     * carrying `col_in_row` — the cursor's on-screen column within its
     * current row — over onto the target row, clamped to that row's own
     * width if it's narrower. Returns `None` past the first/last visual row,
     * i.e. Up on the first row or Down on the last, matching the no-op
     * behaviour of every other boundary motion in this editor.
     */
    let target_row = current_row as isize + delta;
    if target_row < 0 || target_row as usize >= rows.len() {
        return None;
    }
    let (target_line, target_row_start, target_row_end) = rows[target_row as usize];
    let target_col = target_row_start + col_in_row.min(target_row_end - target_row_start);
    Some((target_line, target_col))
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

fn line_for_y(y: f32, line_height: f32, num_rows: usize) -> usize {
    /*
     * Converts a y pixel offset (relative to the start of the text) into a
     * 0-indexed visual row number, clamped to `num_rows - 1` so a click
     * below the last row still lands on it rather than panicking on an
     * out-of-range row index.
     */
    if line_height <= 0.0 || num_rows == 0 { return 0; }
    if y <= 0.0 { return 0; }
    ((y / line_height) as usize).min(num_rows - 1)
}

pub(crate) fn line_col_from_mouse_position(
    position: Point<Pixels>,
    content_bounds: Bounds<Pixels>,
    scroll_offset_y: f32,
    rows: &[(usize, usize, usize)],
) -> (usize, usize) {
    /*
     * Converts a window-space mouse position into a (logical_line,
     * char_column) pair. Shared by on_mouse_down (plain click) and
     * on_mouse_move (click-drag, including `AutoScroller`'s edge-scroll
     * ticks) so all three can never disagree about where a given pixel
     * position maps to.
     *
     * Takes the same visual-row table `render()` paints from (built via
     * `visual_rows_for_viewport`, which needs a live GPUI context for real
     * glyph-width measurement) rather than rebuilding it internally — this
     * function itself stays plain and cx-free. A pixel Y is first resolved
     * to a *visual* row (a wrapped logical line spans several) and only
     * then translated back to the logical (line, column) pair that
     * `AppState` understands.
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
    let col_in_row = column_for_x(local_x, CHAR_WIDTH_PX);
    let visual_row = line_for_y(local_y, LINE_HEIGHT_PX, rows.len());

    let (logical_line, row_start, row_end) = rows[visual_row];
    let col = row_start + col_in_row.min(row_end - row_start);
    (logical_line, col)
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
    use super::{
        column_for_x, line_for_y, selection_span_for_line, line_segments, SegmentStyle,
        usable_wrap_width, wrap_line_into_rows, build_visual_rows, visual_row_for_line_col,
        visual_row_step, document_lines, highlight_color_hex, heading_font_size_px,
        relative_luminance, is_light_color, darken_for_light_text,
    };

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

    // ── usable_wrap_width ────────────────────────────────────────────────────

    #[test]
    fn test_usable_wrap_width_basic() {
        assert_eq!(usable_wrap_width(100.0), 68.0); // 100 - 2*16
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
        assert_eq!(wrap_line_into_rows(&[], 80.0, &|_| 8.0), vec![(0, 0)]);
    }

    #[test]
    fn test_wrap_line_into_rows_fits_in_one_row() {
        let chars: Vec<char> = "hello".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 80.0, &|_| 8.0), vec![(0, 5)]);
    }

    #[test]
    fn test_wrap_line_into_rows_breaks_on_word_boundary() {
        // "hello world" (11 chars) at 8px/char, budget=64px covers "hello wo"
        // (8 chars); last space within budget is at index 5, so row 1 is
        // [0,5)="hello", the space at 5 is consumed, row 2 starts at 6: "world".
        let chars: Vec<char> = "hello world".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 64.0, &|_| 8.0), vec![(0, 5), (6, 11)]);
    }

    #[test]
    fn test_wrap_line_into_rows_hard_breaks_long_word() {
        // No spaces at all within budget -> hard break exactly at the pixel
        // budget (32px / 8px-per-char = 4 chars per row).
        let chars: Vec<char> = "abcdefghij".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 32.0, &|_| 8.0), vec![(0, 4), (4, 8), (8, 10)]);
    }

    #[test]
    fn test_wrap_line_into_rows_exact_multiple_of_width() {
        let chars: Vec<char> = "abcdefgh".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 32.0, &|_| 8.0), vec![(0, 4), (4, 8)]);
    }

    #[test]
    fn test_wrap_line_into_rows_trailing_space_at_break_not_repeated() {
        // Two words separated by exactly one space at the wrap point: the
        // space must not reappear as a leading character on the next row.
        let chars: Vec<char> = "aaaa bbbb".chars().collect();
        let rows = wrap_line_into_rows(&chars, 40.0, &|_| 8.0);
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
        let rows = wrap_line_into_rows(&chars, 10.0, &|_| 100.0);
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
        let width_of = |c: char| if c == '.' { 2.0 } else { 8.0 };
        let rows = wrap_line_into_rows(&chars, 20.0, &width_of);
        assert_eq!(rows[0], (0, 10));
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
        let rows = build_visual_rows(&lines, 800.0, &|_| 8.0);
        assert_eq!(rows, vec![(0, 0, 2), (1, 0, 5)]);
    }

    #[test]
    fn test_build_visual_rows_wraps_long_line_into_multiple_rows() {
        let lines = document_lines("hello world");
        let rows = build_visual_rows(&lines, 64.0, &|_| 8.0);
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
        let rows = vec![(0, 0, 5), (0, 6, 11), (1, 0, 3)];
        assert_eq!(visual_row_step(&rows, 2, 0, -1), Some((0, 6)));
    }

    #[test]
    fn test_visual_row_step_down_into_wrapped_continuation_row() {
        let rows = vec![(0, 0, 5), (0, 6, 11), (1, 0, 3)];
        assert_eq!(visual_row_step(&rows, 0, 3, 1), Some((0, 9)));
    }

    #[test]
    fn test_visual_row_step_preserves_screen_column() {
        let rows = vec![(0, 0, 10), (1, 0, 10)];
        assert_eq!(visual_row_step(&rows, 0, 4, 1), Some((1, 4)));
    }

    #[test]
    fn test_visual_row_step_clamps_to_shorter_target_row() {
        let rows = vec![(0, 0, 10), (1, 0, 3)];
        assert_eq!(visual_row_step(&rows, 0, 8, 1), Some((1, 3)));
    }

    #[test]
    fn test_visual_row_step_up_past_first_row_is_none() {
        let rows = vec![(0, 0, 5)];
        assert_eq!(visual_row_step(&rows, 0, 2, -1), None);
    }

    #[test]
    fn test_visual_row_step_down_past_last_row_is_none() {
        let rows = vec![(0, 0, 5)];
        assert_eq!(visual_row_step(&rows, 0, 2, 1), None);
    }

    // ── highlight_color_hex / heading_font_size_px ──────────────────────────

    #[test]
    fn test_highlight_color_hex_known_names() {
        assert_eq!(highlight_color_hex("yellow"), 0xFFD700);
        assert_eq!(highlight_color_hex("green"), 0x00FF00);
        assert_eq!(highlight_color_hex("black"), 0x000000);
        assert_eq!(highlight_color_hex("white"), 0xFFFFFF);
    }

    #[test]
    fn test_highlight_color_hex_unknown_name_falls_back() {
        assert_eq!(highlight_color_hex("nonexistent"), 0x888888);
    }

    #[test]
    fn test_heading_font_size_body_text_has_no_override() {
        assert_eq!(heading_font_size_px(0), None);
    }

    #[test]
    fn test_heading_font_size_levels_1_through_3_each_distinct() {
        assert_eq!(heading_font_size_px(1), Some(24.0));
        assert_eq!(heading_font_size_px(2), Some(20.0));
        assert_eq!(heading_font_size_px(3), Some(18.0));
    }

    #[test]
    fn test_heading_font_size_levels_4_to_6_share_one_size() {
        assert_eq!(heading_font_size_px(4), Some(16.0));
        assert_eq!(heading_font_size_px(5), Some(16.0));
        assert_eq!(heading_font_size_px(6), Some(16.0));
    }

    #[test]
    fn test_heading_font_size_levels_7_to_9_share_one_size() {
        assert_eq!(heading_font_size_px(7), Some(14.0));
        assert_eq!(heading_font_size_px(9), Some(14.0));
    }

    // ── relative_luminance / is_light_color / darken_for_light_text ────────

    #[test]
    fn test_relative_luminance_white_is_one() {
        assert!((relative_luminance(0xFFFFFF) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_relative_luminance_black_is_zero() {
        assert!((relative_luminance(0x000000) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_relative_luminance_yellow_is_high() {
        // 0.2126*1 + 0.7152*1 + 0.0722*0 = 0.9278
        assert!((relative_luminance(0xFFFF00) - 0.9278).abs() < 0.001);
    }

    #[test]
    fn test_is_light_color_white_is_light() {
        assert!(is_light_color(0xFFFFFF));
    }

    #[test]
    fn test_is_light_color_black_is_not_light() {
        assert!(!is_light_color(0x000000));
    }

    #[test]
    fn test_is_light_color_yellow_highlight_is_light() {
        assert!(is_light_color(highlight_color_hex("yellow")));
    }

    #[test]
    fn test_is_light_color_dark_blue_highlight_is_not_light() {
        assert!(!is_light_color(highlight_color_hex("darkBlue")));
    }

    #[test]
    fn test_darken_for_light_text_reduces_each_channel() {
        let darkened = darken_for_light_text(0xFFD700); // yellow highlight
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
        let darkened = darken_for_light_text(0xFFFF00);
        let r = (darkened >> 16) & 0xFF;
        let g = (darkened >> 8) & 0xFF;
        let b = darkened & 0xFF;
        assert!(r > 0);
        assert!(g > 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_darken_for_light_text_result_is_no_longer_light() {
        assert!(is_light_color(0xFFD700));
        assert!(!is_light_color(darken_for_light_text(0xFFD700)));
    }
}
