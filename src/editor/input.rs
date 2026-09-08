//! Keyboard input dispatch for the editor.

use super::*;
use crate::editor::layout::row_slot_px;
use crate::state::{matches_shifted_symbol, vim_find_target_char, VimMode};

impl TextEditor {
    pub(super) fn process_key(
        &mut self,
        key: &str,
        shift: bool,
        control: bool,
        platform: bool,
        key_char: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.state.update(cx, |state, _cx| {
                state.record_change_key(key, shift, key_char)
            });
        }
        self.process_key_plain(key, shift, key_char, window, cx);
        if was_recording && self.state.read(cx).vim_is_recording_macro() {
            self.state.update(cx, |state, _cx| {
                state.record_macro_key(key, shift, key_char)
            });
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
                self.state
                    .update(cx, |state, _cx| state.vim_jump_backward());
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
            // Ctrl+R: real vim's Redo, in Normal mode only — the app's own
            // configurable Redo keybind defaults to Ctrl+Y instead (see
            // `keybinds.rs`), so this doesn't collide with it; Ctrl+R itself
            // has no default binding and was previously a complete no-op
            // here. Gated the same way `process_key_plain` reads vim state,
            // just below, since this function otherwise reads none.
            "r" => {
                let (vim_enabled, vim_mode) = {
                    let state = self.state.read(cx);
                    let mode = self
                        .tab_index(cx)
                        .and_then(|i| state.workspace.tabs.get(i))
                        .map(|t| t.vim_mode)
                        .unwrap_or(VimMode::Insert);
                    (state.global_vim.vim_enabled, mode)
                };
                if vim_enabled && vim_mode == VimMode::Normal {
                    self.state.update(cx, |state, _cx| state.redo());
                    cx.notify();
                }
            }
            // Ctrl+Left/Right jump by word; Ctrl+Home/End jump to document start/end
            // (spec 4.1). Shift+Ctrl+<key> extends the selection instead of just
            // moving (spec 4.3). Plain (unmodified) arrow/Home/End are handled below.
            "left" => {
                self.state.update(cx, |state, _cx| {
                    if shift {
                        state.extend_word_backward()
                    } else {
                        state.move_word_backward()
                    }
                });
                cx.notify();
            }
            "right" => {
                self.state.update(cx, |state, _cx| {
                    if shift {
                        state.extend_word_forward()
                    } else {
                        state.move_word_forward()
                    }
                });
                cx.notify();
            }
            // Ctrl+Up/Down jump a whole paragraph, the vertical counterpart
            // to Ctrl+Left/Right's word jump (beta feedback). Reuses the
            // paragraph motions vim's `{`/`}` already go through, so a
            // "paragraph" means the same thing to both — a completely blank
            // line. Lands here rather than in the configurable keybind system
            // for the same reason as its neighbours: it only makes sense with
            // editor focus, and plain Up/Down are handled as raw keys too
            // (`process_key_plain`'s visual-row branch).
            //
            // These deliberately do not extend on Shift. `move_paragraph_*`
            // clears the selection outright, and there is no
            // `extend_paragraph_*` to pair with; wiring Shift to a
            // selection-clearing move would read as a broken extend rather
            // than an unimplemented one.
            "up" => {
                self.state
                    .update(cx, |state, _cx| state.move_paragraph_backward());
                cx.notify();
            }
            "down" => {
                self.state
                    .update(cx, |state, _cx| state.move_paragraph_forward());
                cx.notify();
            }
            "home" => {
                self.state.update(cx, |state, _cx| {
                    if shift {
                        state.extend_doc_start()
                    } else {
                        state.move_doc_start()
                    }
                });
                cx.notify();
            }
            "end" => {
                self.state.update(cx, |state, _cx| {
                    if shift {
                        state.extend_doc_end()
                    } else {
                        state.move_doc_end()
                    }
                });
                cx.notify();
            }
            // Ctrl+Backspace deletes the previous word (spec bugfix/QoL
            // task 8). Content-editing key, so it lives here rather than
            // the global keybind/action system (`src/keybinds.rs`) —
            // it only makes sense with editor focus, same reasoning as
            // Ctrl+Left/Right/Home/End/O/I above.
            "backspace" => {
                self.state.update(cx, |state, cx| {
                    state.delete_word_backward();
                    cx.notify();
                });
            }
            _ => {} // Ctrl+S, Ctrl+T, Ctrl+W, etc. handled by global actions
        }
        self.scroll_to_cursor(cx);
    }

    fn process_key_plain(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            let idx = self.tab_index(cx);
            let state = self.state.read(cx);
            let mode = idx
                .and_then(|i| state.workspace.tabs.get(i))
                .map(|t| t.vim_mode)
                .unwrap_or_default();
            (state.global_vim.vim_enabled, mode)
        };
        if vim_enabled {
            if vim_mode == VimMode::Insert {
                if key == "escape" {
                    self.state
                        .update(cx, |state, _cx| state.vim_exit_to_normal());
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
                if (vim_mode == VimMode::Normal || is_visual)
                    && no_pending_trigger
                    && !shift
                    && (key == "j" || key == "k")
                {
                    let count = self
                        .state
                        .update(cx, |state, _cx| state.take_vim_count())
                        .unwrap_or(1);
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
                //
                // Bug report: landed on the wrong row whenever a card-style
                // row (Pocket/Block/Tag/Cite/heading) sat above the
                // viewport. Root cause: dividing the raw scroll offset by a
                // single `line_height` assumes every row is the same
                // height, but an oversized row reserves extra blank
                // *display* spacer rows (`expand_rows_for_display`) that
                // push everything after it down — `cursor_scroll_geometry`
                // already accounts for this by translating through
                // `display_to_wrap`; this now does the same translation in
                // the opposite direction (pixels -> display row -> real
                // content row), reusing the identical "a spacer slot
                // belongs to the nearest real row before it" rule
                // `line_col_from_mouse_position` already established for
                // clicks landing on one.
                if (vim_mode == VimMode::Normal || is_visual)
                    && no_pending_trigger
                    && shift
                    && matches!(key, "h" | "m" | "l")
                {
                    let zoom = self.state.read(cx).zoom;
                    let normal_size_px =
                        self.state.read(cx).effective_normal_size_half_points() as f32 / 2.0;
                    let line_spacing = self.state.read(cx).preferences.line_spacing;
                    let viewport_width = self.scroll_handle.bounds().size.width.as_f32();
                    let (rows, display_to_wrap, _) =
                        self.cached_or_fresh_row_tables(cx, viewport_width);
                    if !rows.is_empty() && !display_to_wrap.is_empty() {
                        // Display-row indices, so this steps by the display
                        // grid's own pitch, not by a full line of text.
                        let slot_px = row_slot_px(normal_size_px, line_spacing, zoom);
                        let viewport_h = self.scroll_handle.bounds().size.height.as_f32()
                            - 2.0 * CONTENT_PADDING_PX;
                        let offset = self.scroll_handle.offset();
                        let last_display_row = display_to_wrap.len() - 1;
                        let top_display = (((-offset.y.as_f32()) / slot_px).floor().max(0.0)
                            as usize)
                            .min(last_display_row);
                        let visible_count = ((viewport_h / slot_px).floor().max(1.0)) as usize;
                        let bottom_display =
                            (top_display + visible_count.saturating_sub(1)).min(last_display_row);
                        let top_row =
                            nearest_wrap_row_for_display_row(&display_to_wrap, top_display);
                        let bottom_row =
                            nearest_wrap_row_for_display_row(&display_to_wrap, bottom_display);
                        let target_row = match key {
                            "h" => top_row,
                            "l" => bottom_row,
                            "m" => top_row + bottom_row.saturating_sub(top_row) / 2,
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

                // Real vim's `zz`/`zt`/`zb` (center/scroll-to-top/scroll-
                // to-bottom the viewport on the cursor's line) — needs live
                // scroll-handle geometry, same GPUI-context reason j/k and
                // H/M/L above are intercepted here rather than reaching
                // `AppState::handle_vim_key`. Piggybacks on the z-leader
                // custom-keybind buffer (`vim_keybind_seq`) that the first
                // `z` keystroke already starts via the ordinary catch-all
                // below — only *after* that buffer already holds exactly
                // "z" does this claim the second keystroke, so every other
                // zX default/custom binding (zs, zn, zy, ...) is completely
                // unaffected and still resolves through the normal path.
                // `zz`/`zt`/`zb` are deliberately no longer in
                // `VimKeybinds::defaults()` (see `vim_keybinds.rs`'s
                // `NATIVE_VIM_SEQUENCES`) — without this intercept they'd
                // fall through to `continue_vim_keybind_sequence`, resolve
                // to `VimLookup::None`, and silently do nothing, which is
                // exactly the reported bug.
                if vim_mode == VimMode::Normal
                    && no_pending_trigger
                    && !shift
                    && matches!(key, "z" | "t" | "b")
                {
                    let pending_z = self
                        .tab_index(cx)
                        .and_then(|i| {
                            self.state
                                .read(cx)
                                .workspace
                                .tabs
                                .get(i)
                                .map(|t| t.vim_keybind_seq == "z")
                        })
                        .unwrap_or(false);
                    if pending_z {
                        self.state.update(cx, |state, _cx| {
                            if let Some(tab) =
                                state.workspace.tabs.get_mut(state.workspace.active_tab)
                            {
                                tab.vim_keybind_seq.clear();
                            }
                        });
                        match key {
                            "z" => self.scroll_to_cursor_centered(cx),
                            "t" => self.scroll_to_cursor_top(cx),
                            "b" => self.scroll_to_cursor_bottom(cx),
                            _ => unreachable!(),
                        }
                        // Bug report: the view didn't move until the *next*
                        // keystroke. The scroll helpers only call
                        // `scroll_handle.set_offset` — they never request a
                        // repaint, and this is the one branch in this file
                        // that changes the offset without also mutating state.
                        // Every other caller gets its frame for free from an
                        // accompanying `state.update(.., cx.notify())` (the
                        // `scroll_to_cursor` sites) or notifies explicitly
                        // (read mode's `page_scroll`, `:879`); the
                        // `state.update` above takes `_cx` because it only
                        // clears the sequence buffer. So nothing scheduled a
                        // frame, and the new offset sat unpainted until an
                        // unrelated key (w/e/b) caused one.
                        //
                        // Deliberately notified here rather than inside the
                        // three helpers: `scroll_to_cursor_centered` is also
                        // called from inside `render()` (the
                        // `pending_scroll_to_cursor` drain), where notifying
                        // would dirty the view mid-paint and cost an extra
                        // frame every time the Nav menu jumps to a line.
                        cx.notify();
                        return;
                    }
                }

                // Bug report: `$` jumped to the end of the whole wrapped
                // paragraph instead of the current visual row — same
                // "logical line vs visual row" inversion `j`/`k` already
                // make for this heavily-wrapping app (see
                // `move_cursor_to_row_edge`'s doc comment). `0`/`^`/Home/End
                // get the identical treatment for consistency; `g$`/`g0`/
                // `g^` (`state.rs`) reach the original logical-line target.
                // Deliberately does NOT cover an operator's pending target
                // (`d$`/`c$`/`D`/`C`) — confirmed with the reporter that
                // those should keep deleting to the end of the paragraph,
                // which is exactly what gating this on `no_pending_trigger`
                // (already excludes a pending operator) achieves: with one
                // pending, this block is skipped and the key falls through
                // to `handle_vim_key` below, which still resolves `$`/`0`/
                // `^`/Home/End through the unchanged, logical-line
                // `resolve_vim_motion` arms.
                if (vim_mode == VimMode::Normal || is_visual) && no_pending_trigger {
                    // Bare "0" only means "start of row" when no count
                    // digits are already being typed — "10" must still
                    // accumulate as count 10, not treat its second "0" as
                    // a motion.
                    let buf_empty = self
                        .tab_index(cx)
                        .and_then(|i| {
                            self.state
                                .read(cx)
                                .workspace
                                .tabs
                                .get(i)
                                .map(|t| t.vim_command_buf.is_empty())
                        })
                        .unwrap_or(true);
                    let edge =
                        if matches_shifted_symbol(key, shift, key_char, "4", "$") || key == "end" {
                            Some(RowEdge::End)
                        } else if matches_shifted_symbol(key, shift, key_char, "6", "^") {
                            Some(RowEdge::FirstNonBlank)
                        } else if key == "home" || (key == "0" && !shift && buf_empty) {
                            Some(RowEdge::Start)
                        } else {
                            None
                        };
                    if let Some(edge) = edge {
                        self.move_cursor_to_row_edge(cx, edge, is_visual);
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
                                self.state.read(cx).global_vim.vim_last_macro_register
                            } else {
                                Some(register)
                            };
                            if let Some(register) = register {
                                self.replay_macro(register, window, cx);
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
                            // The clipboard's rich metadata rides along when
                            // this app wrote it, so `"+p` restores formatting
                            // the same way Ctrl+V does; another app's
                            // clipboard has none and pastes plain.
                            let metadata = item.metadata().map(|m| m.to_string());
                            self.state.update(cx, |state, _cx| {
                                state.set_register('+', text.to_string(), metadata)
                            });
                        }
                    }
                }

                let (consumed, clipboard_sync, vim_action) = self.state.update(cx, |state, cx| {
                    let handled = state.handle_vim_key(key, shift, key_char);
                    if handled {
                        cx.notify();
                    }
                    (
                        handled,
                        state.take_pending_clipboard_sync(),
                        state.take_pending_vim_action(),
                    )
                });
                // `"+y`/`"+d`/`"+c` (write direction): mirrors the read
                // direction above — `execute_vim_operator_range` stages the
                // text in `pending_clipboard_sync` when the `+` register
                // was targeted; this is the only place with `cx` to
                // actually push it onto the OS clipboard.
                if let Some((text, metadata)) = clipboard_sync {
                    // Carries the formatting as clipboard metadata, exactly as
                    // `CopyAction` does, so `"+y` then Ctrl+V pastes a styled
                    // card rather than bare text.
                    cx.write_to_clipboard(ClipboardItem::new_string_with_metadata(text, metadata));
                }
                // Checklist: Settings -> Vim Mode. Same mailbox pattern as
                // `clipboard_sync` above — `state.rs` staged the resolved
                // `KeybindAction`, this is the one place with `window`+`cx`
                // to actually fire it, via the same `dispatch_action` call
                // `app_toolbar.rs`'s toolbar buttons already use.
                if let Some(action) = vim_action {
                    window.dispatch_action(crate::keybinds::action_for(action), cx);
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
            let vim_visual =
                vim_enabled && matches!(vim_mode, VimMode::Visual | VimMode::VisualLine);
            let delta = if key == "up" { -1 } else { 1 };
            self.move_cursor_visual_row(cx, delta, shift || vim_visual);
            self.scroll_to_cursor(cx);
            return;
        }

        // Home/End, reached here rather than above precisely when vim is
        // disabled or in Insert mode — the vim-Normal/Visual case is
        // already handled above (same visual-row logic, see
        // `move_cursor_to_row_edge`'s doc comment), and an operator-pending
        // Home/End (`dEnd`) is consumed before ever reaching this point.
        if key == "home" || key == "end" {
            let edge = if key == "home" {
                RowEdge::Start
            } else {
                RowEdge::End
            };
            self.move_cursor_to_row_edge(cx, edge, shift);
            return;
        }

        let consumed = self.state.update(cx, |state, cx| {
            match key {
                "backspace" => {
                    state.backspace();
                    cx.notify();
                    true
                }
                "delete" => {
                    state.delete_forward();
                    cx.notify();
                    true
                }
                "enter" => {
                    state.insert_char('\n');
                    cx.notify();
                    true
                }
                "space" => {
                    state.insert_char(' ');
                    cx.notify();
                    true
                }
                "tab" => {
                    let in_list = state
                        .workspace
                        .tabs
                        .get(state.workspace.active_tab)
                        .is_some_and(|t| {
                            let (para_idx, ..) = crate::document_ops::resolve_position(
                                &t.document.paragraphs,
                                t.cursor,
                            );
                            t.document
                                .paragraphs
                                .get(para_idx)
                                .is_some_and(|p| p.list.is_some())
                        });
                    if in_list {
                        if shift {
                            state.outdent_list_item()
                        } else {
                            state.indent_list_item()
                        }
                    } else {
                        state.insert_char('\t');
                    }
                    cx.notify();
                    true
                }
                // Shift+<key> extends the selection instead of moving plainly (spec 4.3).
                "left" => {
                    if shift {
                        state.extend_left()
                    } else {
                        state.move_left()
                    };
                    cx.notify();
                    true
                }
                "right" => {
                    if shift {
                        state.extend_right()
                    } else {
                        state.move_right()
                    };
                    cx.notify();
                    true
                }
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
        if consumed {
            cx.notify();
        }
        self.scroll_to_cursor(cx);
    }

    fn replay_macro(&mut self, register: char, window: &mut Window, cx: &mut Context<Self>) {
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
        self.state.update(cx, |state, _cx| {
            state.global_vim.vim_last_macro_register = Some(register);
        });
        let Some(keys) = self.state.read(cx).macro_keys(register) else {
            return;
        };
        for k in keys {
            self.process_key(
                &k.key,
                k.shift,
                false,
                false,
                k.key_char.as_deref(),
                window,
                cx,
            );
        }
    }
}
