use super::*;

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
        // Tab switch (TabBar's on_click -> `set_active_tab`) and file-open
        // (sidebar, new-tab) never touch GPUI keyboard focus directly — they
        // only flip `active_tab` on the shared AppState. Left alone, the
        // text editor's FocusHandle stays wherever it was (often nowhere),
        // so Enter/keys silently stop reaching `handle_key_down` until the
        // user clicks into the editor again. Honor and clear the request
        // here, once per frame, mirroring `pending_scroll_to_cursor` below.
        // Only when *this* pane is the one being asked for — with two editors
        // mounted, an unqualified flag lets whichever renders first steal the
        // keyboard from the pane the user actually acted on.
        if self
            .state
            .update(cx, |state, _cx| state.take_pending_editor_focus(self.pane))
        {
            self.focus_handle.clone().focus(window, cx);
        }

        // Nav menu jump (state.rs's `jump_to_line`): FileExplorer has no
        // direct reference to this view to call a scroll method on, so it
        // leaves a flag on the active tab instead. Honor and clear it here,
        // before laying out this frame — always centering (not the regular
        // edge-triggered scroll_to_cursor) so clicking an already-visible
        // heading still visibly does something.
        let pane_idx = self.tab_index(cx);
        let should_scroll = self.state.update(cx, |state, _cx| {
            state.take_pending_scroll_to_cursor(self.pane)
        });
        if should_scroll {
            self.scroll_to_cursor_centered(cx);
        }

        // Tab-scroll isolation: this view has a single shared `scroll_handle`
        // for the whole window (see the struct-field doc comment above), so
        // without this check the previously active tab's scroll offset just
        // stays put when the user switches tabs, "leaking" into whichever
        // tab becomes active. Detect the switch by comparing the active
        // tab's stable `id` (not its positional index) against what was
        // seen last render: on a switch, stash the outgoing tab's current
        // offset under its old id, then restore the incoming tab's saved
        // offset — or `Point::default()` (scrolled to top) if this is the
        // first time that tab has ever been active.
        let active_tab_id = self
            .tab_index(cx)
            .and_then(|i| self.state.read(cx).workspace().tabs.get(i))
            .map(|t| t.id.0);
        if self.last_seen_active_tab != active_tab_id {
            if let Some(prev_id) = self.last_seen_active_tab {
                self.tab_scroll_offsets
                    .insert(prev_id, self.scroll_handle.offset());
            }
            let restore = active_tab_id
                .and_then(|id| self.tab_scroll_offsets.get(&id))
                .copied()
                .unwrap_or_default();
            self.scroll_handle.set_offset(restore);
            self.last_seen_active_tab = active_tab_id;
        }

        let idx = pane_idx;
        let state = self.state.read(cx);
        let zoom = state.zoom();
        // The editor pane is themed like the rest of the chrome — every color
        // below comes from the palette so light mode reaches the document
        // surface too, not just the frame around it.
        let p = state.current_palette();
        let theme_mode = state.preferences().theme_mode;
        let cursor_style = if state.global_vim().vim_enabled {
            CursorStyle::Block
        } else {
            CursorStyle::Line
        };
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        // The document's own `<w:docDefaults>` font, when it names one this app
        // can actually render. `is_curated_font` is the same gate `run.font`
        // already passes through — it covers the bundled families and any the
        // user has imported, and everything else keeps falling back to
        // `FONT_FAMILY` for the reason `apply_run_style` documents (GPUI can't
        // match bold/italic within a family whose faces aren't all loaded).
        let body_font: SharedString = state
            .effective_body_font()
            .filter(|name| is_curated_font(name))
            .map(|name| SharedString::from(name.to_string()))
            .unwrap_or_else(|| SharedString::from(FONT_FAMILY));
        let line_spacing = state.preferences().line_spacing;
        let viewport_width = self.scroll_handle.bounds().size.width.as_f32();
        let dragging = state.workspace().split_dragging;
        // Scroll movement re-arms the scrollbar's fade. Compared with a small
        // tolerance so sub-pixel jitter in the offset can't hold the bar
        // permanently visible by restarting the animation every frame.
        let scroll_y_now = self.scroll_handle.offset().y.as_f32();
        if self
            .last_scrollbar_offset_y
            .is_none_or(|prev| (prev - scroll_y_now).abs() > 0.5)
        {
            self.last_scrollbar_offset_y = Some(scroll_y_now);
            self.scrollbar_activity = self.scrollbar_activity.wrapping_add(1);
        }
        let scrollbar_activity = self.scrollbar_activity;
        let invisibility = state.ui().invisibility_mode;
        let cite_size = state.preferences().cite_size_half_points;
        let fold_version = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.fold_version)
            .unwrap_or(0);
        let folds = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.folded_headings.clone())
            .unwrap_or_default();
        let tab_id = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.id.0)
            .unwrap_or(usize::MAX);
        let content_version = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.document.content_version)
            .unwrap_or(0);
        let cache_valid = self.row_cache.as_ref().is_some_and(|c| {
            row_cache_is_valid_for(
                c,
                tab_id,
                content_version,
                viewport_width,
                zoom,
                line_spacing,
                dragging,
                invisibility,
                fold_version,
            )
        });
        // Only pay for the full content/paragraphs clone on a cache miss.
        // `document_lines`/word-wrap need `cx` free of `state`'s borrow (see
        // `let _ = state;` below), so the actual wrap happens further down —
        // this just captures the owned data a miss needs before that borrow ends.
        let fresh_content_and_paragraphs = (!cache_valid).then(|| {
            (
                state.pane_content(self.pane).to_string(),
                state
                    .workspace()
                    .tabs
                    .get(idx.unwrap_or(usize::MAX))
                    .map(|t| t.document.paragraphs().to_vec())
                    .unwrap_or_default(),
            )
        });
        let is_new_tab = state
            .workspace()
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .map(|t| t.is_blank_new_tab())
            .unwrap_or(true);
        let banner_message = state
            .workspace()
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .and_then(|t| t.banner_message());
        let (cursor_line, cursor_col) = state.pane_cursor_line_col(self.pane);
        // Normalise (anchor, focus) into (min, max) once so per-line lookups
        // below don't each have to re-derive the ordering.
        // Flattened with "Select similar formatting"'s own matched ranges
        // (`Tab.similar_ranges`) — the two draw identically, so the whole
        // paint path below takes one list rather than knowing about both.
        // In practice only one is ever non-empty: selecting-similar clears
        // the caret selection, and the next keystroke or click clears the
        // similar ranges.
        let selections: Vec<(usize, usize)> = state
            .workspace()
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .into_iter()
            .flat_map(|t| {
                t.selection
                    .map(|(a, f)| (a.min(f), a.max(f)))
                    .into_iter()
                    .chain(t.similar_ranges.iter().copied())
            })
            .collect();
        // Mode indicator text. Deviates from spec 5.1's literal "nothing
        // shown for Normal" — showing `-- NORMAL --` removes the ambiguity
        // between "vim is on and in Normal mode" and "vim mode is off
        // entirely", both of which otherwise render an identical blank
        // indicator strip.
        let mode_indicator_text: Option<&'static str> = if state.global_vim().vim_enabled {
            idx.and_then(|i| state.workspace().tabs.get(i))
                .map(|t| match t.vim_mode {
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
            .workspace()
            .tabs
            .get(idx.unwrap_or(usize::MAX))
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
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
                    buf.push_str(err);
                }
                buf
            })
            .filter(|buf| !buf.is_empty());
        let _ = state;

        let is_focused = self.focus_handle.is_focused(window);

        // Rebuild the word-wrapped row table only on a cache miss (see
        // `RowCache`'s doc comment) — `state`'s borrow of `cx` has already
        // ended (`let _ = state;` above), so `cx` is free for
        // `visual_rows_for_viewport` again here.
        if let Some((content, paragraphs)) = fresh_content_and_paragraphs {
            let lines = document_lines(&content);
            let line_chars: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();

            // Byte offset of each logical line's start within `content`,
            // needed to test `selection` (a document-wide byte range)
            // against each line.
            let mut line_byte_starts: Vec<usize> = Vec::with_capacity(lines.len());
            let mut byte_offset = 0;
            for l in &lines {
                line_byte_starts.push(byte_offset);
                byte_offset += l.len() + 1; // +1 for the '\n' the split() consumed
            }

            // Word-wrap each logical line into fixed-height visual rows so
            // long lines reflow within the viewport instead of running off
            // the right edge. Click/drag hit-testing and scroll-to-cursor
            // rebuild this exact same row table (via the same helper
            // functions) so all three always agree on where each row's
            // boundaries fall.
            let rows = visual_rows_for_viewport(
                cx,
                &lines,
                viewport_width,
                zoom,
                &paragraphs,
                normal_size_px,
            );
            let folded_paras = AppState::folded_paragraphs(&paragraphs, &folds);
            let hidden =
                hidden_wrap_rows(&rows, &paragraphs, invisibility, cite_size, &folded_paras);
            let (display_to_wrap, wrap_to_display) = expand_rows_for_display(
                &rows,
                &paragraphs,
                zoom,
                &hidden,
                normal_size_px,
                line_spacing,
            );

            self.row_cache = Some(RowCache {
                tab_id,
                content_version,
                invisibility,
                fold_version,
                viewport_width_bits: viewport_width.to_bits(),
                zoom_bits: zoom.to_bits(),
                line_spacing_bits: line_spacing.to_bits(),
                lines: Rc::new(lines),
                line_chars: Rc::new(line_chars),
                line_byte_starts: Rc::new(line_byte_starts),
                rows: Rc::new(rows),
                paragraphs: Rc::new(paragraphs.to_vec()),
                display_to_wrap: Rc::new(display_to_wrap),
                wrap_to_display: Rc::new(wrap_to_display),
            });
        }
        let cache = self.row_cache.as_ref().expect(
            "populated just above on a miss; cache_valid guarantees it already existed on a hit",
        );
        let lines = cache.lines.clone();
        let line_chars = cache.line_chars.clone();
        let line_byte_starts = cache.line_byte_starts.clone();
        let rows = cache.rows.clone();
        let paragraphs = cache.paragraphs.clone();
        let display_to_wrap = cache.display_to_wrap.clone();
        let wrap_to_display = cache.wrap_to_display.clone();

        // Display-space cursor row (see `expand_rows_for_display`) — the
        // `uniform_list` closure below iterates display indices, not raw
        // wrap-row indices, so the cursor-row comparison inside it needs to
        // be in the same space.
        let cursor_visual_row = is_focused.then(|| {
            let wrap_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
            wrap_to_display[wrap_row]
        });

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
            .when_some(banner_message, |d, message| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px(px(12.0))
                        .py(px(6.0))
                        // A warning strip, so it keeps its amber identity
                        // rather than becoming palette chrome — but the dark
                        // amber is illegible on a light theme, so each mode
                        // gets its own pairing.
                        .bg(rgb(match theme_mode {
                            ThemeMode::Dark => 0x5a3d1a,
                            ThemeMode::Light => 0xfaecc8,
                        }))
                        .text_color(rgb(match theme_mode {
                            ThemeMode::Dark => 0xf0d9a8,
                            ThemeMode::Light => 0x6b4e10,
                        }))
                        .text_sm()
                        .child(message)
                        .child(
                            div()
                                .id("dismiss-unsupported-banner")
                                .cursor_pointer()
                                .px(px(8.0))
                                .child("×")
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    // Resolved at click time, not captured from
                                    // render: this listener outlives the frame
                                    // and the pane's tab can change meanwhile.
                                    let pane = this.pane;
                                    this.state.update(cx, |s, cx| {
                                        s.dismiss_unsupported_banner(pane);
                                        cx.notify();
                                    });
                                })),
                        ),
                )
            })
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // Claim the pane before anything reads a tab: `focus_pane`
                            // repoints `active_tab`, and every state call below (cursor
                            // placement, selection) resolves through it.
                            let pane = this.pane;
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(pane);
                                cx.notify();
                            });
                            this.focus_handle.clone().focus(window, cx);
                            let bounds = this.scroll_handle.bounds();
                            let scroll_y = this.scroll_handle.offset().y.as_f32();
                            let zoom = this.state.read(cx).zoom();
                            let font_size_px =
                                this.state.read(cx).effective_normal_size_half_points() as f32
                                    / 2.0;
                            let line_spacing = this.state.read(cx).preferences().line_spacing;
                            let paragraphs = {
                                let st = this.state.read(cx);
                                pane_idx
                                    .and_then(|i| st.workspace().tabs.get(i))
                                    .map(|t| t.document.paragraphs().to_vec())
                                    .unwrap_or_default()
                            };
                            let (rows, display_to_wrap, _) =
                                this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                            let row_height_px = real_row_height_px(
                                &this.uniform_list_scroll_handle,
                                display_to_wrap.len(),
                                font_size_px,
                                zoom,
                                line_spacing,
                            );
                            let (line, col) = line_col_from_mouse_position(
                                ev.position,
                                bounds,
                                scroll_y,
                                &rows,
                                &display_to_wrap,
                                zoom,
                                font_size_px,
                                &paragraphs,
                                row_height_px,
                            );
                            let click_count = ev.click_count;
                            let shift_click = ev.modifiers.shift && click_count == 1;
                            this.state.update(cx, |state, cx| {
                                state.close_editor_context_menu();
                                state.clear_similar_selection();
                                if shift_click {
                                    // Shift+Click: extend the selection from wherever the
                                    // cursor already is to the click point — the same
                                    // `extend_selection_to_line_col` a click-drag calls on
                                    // every `on_mouse_move`, just driven by one click
                                    // instead of a series of move events. Anchors at the
                                    // current cursor position when there's no selection
                                    // yet (see that function's own doc comment).
                                    state.extend_selection_to_line_col(line, col);
                                } else {
                                    // `set_cursor_from_line_col` does the line/col -> byte-offset
                                    // conversion (there's no standalone public helper for it) and
                                    // leaves the result in `tab.cursor`, so double/triple-click
                                    // reuse that single call instead of re-deriving the byte
                                    // position themselves.
                                    state.set_cursor_from_line_col(line, col);
                                    let byte_pos = pane_idx
                                        .and_then(|i| state.workspace().tabs.get(i))
                                        .map(|t| t.cursor)
                                        .unwrap_or(0);
                                    match click_count {
                                        2 => state.select_word_at(byte_pos),
                                        3 => state.select_line_at(byte_pos),
                                        _ => {}
                                    }
                                }
                                cx.notify();
                            });
                            cx.notify();
                        }),
                    )
                    // Right-click opens the Cut/Copy/Paste menu (rendered by
                    // `render_context_menu` at the bottom of this wrapper).
                    //
                    // ponytail: an existing selection is left alone rather than
                    // hit-tested against the click point — right-clicking *inside* a
                    // selection must keep it (that's the whole point of the Copy
                    // item), and right-clicking outside one is rare enough that
                    // "menu opens, selection unchanged" beats redoing the byte-offset
                    // math just to decide whether to clear it. Add the hit-test if
                    // that ever bites.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let pane = this.pane;
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(pane);
                                cx.notify();
                            });
                            this.focus_handle.clone().focus(window, cx);
                            let has_selection = {
                                let st = this.state.read(cx);
                                pane_idx
                                    .and_then(|i| st.workspace().tabs.get(i))
                                    .is_some_and(|t| t.selection.is_some())
                            };
                            // Resolve the click to a (line, col) whether or not there's a
                            // selection — without a selection it also moves the caret,
                            // but either way it's what locates a misspelled word below.
                            let bounds = this.scroll_handle.bounds();
                            let scroll_y = this.scroll_handle.offset().y.as_f32();
                            let zoom = this.state.read(cx).zoom();
                            let font_size_px =
                                this.state.read(cx).effective_normal_size_half_points() as f32
                                    / 2.0;
                            let line_spacing = this.state.read(cx).preferences().line_spacing;
                            let paragraphs = {
                                let st = this.state.read(cx);
                                pane_idx
                                    .and_then(|i| st.workspace().tabs.get(i))
                                    .map(|t| t.document.paragraphs().to_vec())
                                    .unwrap_or_default()
                            };
                            let (rows, display_to_wrap, _) =
                                this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                            let row_height_px = real_row_height_px(
                                &this.uniform_list_scroll_handle,
                                display_to_wrap.len(),
                                font_size_px,
                                zoom,
                                line_spacing,
                            );
                            let (line, col) = line_col_from_mouse_position(
                                ev.position,
                                bounds,
                                scroll_y,
                                &rows,
                                &display_to_wrap,
                                zoom,
                                font_size_px,
                                &paragraphs,
                                row_height_px,
                            );
                            if !has_selection {
                                this.state.update(cx, |state, _cx| {
                                    state.set_cursor_from_line_col(line, col)
                                });
                            }

                            // Did the click land on a squiggle? `suggest` runs here, once,
                            // rather than during render — it's a dictionary search, far
                            // slower than the per-word `check` the squiggles use.
                            let spell_target = {
                                let st = this.state.read(cx);
                                if !st.preferences().spellcheck_enabled {
                                    None
                                } else {
                                    let content = pane_idx
                                        .and_then(|i| st.workspace().tabs.get(i))
                                        .map(|t| t.document.content())
                                        .unwrap_or_default();
                                    let lines = document_lines(&content);
                                    lines.get(line).and_then(|text| {
                                        crate::spellcheck::misspelled_ranges(
                                            text,
                                            st.user_dictionary(),
                                        )
                                        .into_iter()
                                        .find(|&(s, e)| col >= s && col < e)
                                        .map(
                                            |(start_col, end_col)| {
                                                let word: String = text
                                                    .chars()
                                                    .skip(start_col)
                                                    .take(end_col - start_col)
                                                    .collect();
                                                let suggestions = crate::spellcheck::suggest(&word);
                                                SpellTarget {
                                                    line,
                                                    start_col,
                                                    end_col,
                                                    word,
                                                    suggestions,
                                                }
                                            },
                                        )
                                    })
                                }
                            };

                            this.state.update(cx, |state, cx| {
                                state.open_editor_context_menu(EditorContextMenu {
                                    position: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                                    spell_target,
                                });
                                cx.notify();
                            });
                        }),
                    )
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
                        if !ev.dragging() {
                            // Self-heal. If a release ever escapes both mouse-up
                            // handlers, a stuck flag would kill text selection for
                            // the rest of the session; the first move with no button
                            // held proves the pointer is up and clears it.
                            this.scrollbar_pressed.set(false);
                            return;
                        }
                        // A drag that belongs to something else is not a text
                        // selection. Dragging the scrollbar holds the left button
                        // down and moves the pointer across the document, which is
                        // indistinguishable from a click-drag at this level — so it
                        // selected everything it passed over, and fed the edge
                        // auto-scroller besides (bug report: "scrolling down
                        // highlights text because it requires LMB down"). The same
                        // guard covers the tab, sidebar-resize and split-resize
                        // drags, none of which should extend a selection either.
                        if cx.has_active_drag() || this.scrollbar_pressed.get() {
                            return;
                        }
                        let bounds = this.scroll_handle.bounds();
                        let scroll_y = this.scroll_handle.offset().y.as_f32();
                        let zoom = this.state.read(cx).zoom();
                        let font_size_px =
                            this.state.read(cx).effective_normal_size_half_points() as f32 / 2.0;
                        let line_spacing = this.state.read(cx).preferences().line_spacing;
                        let paragraphs = {
                            let st = this.state.read(cx);
                            pane_idx
                                .and_then(|i| st.workspace().tabs.get(i))
                                .map(|t| t.document.paragraphs().to_vec())
                                .unwrap_or_default()
                        };
                        let (rows, display_to_wrap, _) =
                            this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                        let row_height_px = real_row_height_px(
                            &this.uniform_list_scroll_handle,
                            display_to_wrap.len(),
                            font_size_px,
                            zoom,
                            line_spacing,
                        );
                        let (line, col) = line_col_from_mouse_position(
                            ev.position,
                            bounds,
                            scroll_y,
                            &rows,
                            &display_to_wrap,
                            zoom,
                            font_size_px,
                            &paragraphs,
                            row_height_px,
                        );
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
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _window, _cx| {
                            this.auto_scroller.stop();
                            this.scrollbar_pressed.set(false);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _window, _cx| {
                            this.auto_scroller.stop();
                            this.scrollbar_pressed.set(false);
                        }),
                    )
                    .flex()
                    .flex_col()
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
                    .bg(rgb(p.editor_bg))
                    // No `.overflow_y_scroll()`/`.track_scroll()`/`.p()`/`.border_1()`
                    // here anymore — `uniform_list` below owns the actual scrolling
                    // now (it sets its own vertical overflow internally and is
                    // `.track_scroll()`ed to `uniform_list_scroll_handle`), and
                    // padding/border move with it: `self.scroll_handle.bounds()`
                    // (via the shared handle, see `TextEditor::new()`) reflects
                    // *uniform_list's* box, and the click/scroll pixel math's
                    // `CONTENT_PADDING_PX` subtraction assumes that box already
                    // includes the padding inset, same as it did when this div was
                    // both the padded box and the tracked scroll container at once.
                    // This div is now just a plain flex_col wrapper stacking
                    // [new-tab placeholder?, the scrollable row list].
                    // Placeholder shown on an empty, unsaved tab — a plain sibling
                    // above the row list now rather than uniform_list's first
                    // "item" (uniform_list has no such prepend slot); low-risk since
                    // a genuinely empty new tab has nothing to scroll to anyway.
                    .when(is_new_tab, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(p.text_faint))
                                .font_family(FONT_FAMILY)
                                .p(px(16.0))
                                .child("Open a file from the sidebar, or start typing…"),
                        )
                    })
                    .child(
                        uniform_list("text-editor-rows", display_to_wrap.len(), {
                            let lines = lines.clone();
                            let selections = selections.clone();
                            let line_chars = line_chars.clone();
                            let line_byte_starts = line_byte_starts.clone();
                            let rows = rows.clone();
                            let paragraphs = paragraphs.clone();
                            let display_to_wrap = display_to_wrap.clone();
                            // Spellcheck inputs, read once per frame rather than per
                            // row. `user_dictionary` is `Rc` in `AppState` precisely
                            // so this is a refcount bump, not a deep clone.
                            let spellcheck_enabled =
                                self.state.read(cx).preferences().spellcheck_enabled;
                            let user_dictionary = self.state.read(cx).user_dictionary().clone();
                            let spell_cache = self.spell_cache.clone();
                            let spellcheck_color = crate::editor::color::highlight_color_hex(
                                &self.state.read(cx).preferences().spellcheck_underline_color,
                            );
                            let invisibility_mode = self.state.read(cx).ui().invisibility_mode;
                            let cite_size_half_points =
                                self.state.read(cx).preferences().cite_size_half_points;
                            let folded_headings = {
                                let st = self.state.read(cx);
                                st.workspace()
                                    .tabs
                                    .get(st.workspace().active_tab)
                                    .map(|t| t.folded_headings.clone())
                                    .unwrap_or_default()
                            };
                            let fold_state = self.state.clone();
                            move |range: std::ops::Range<usize>, _window, _cx| {
                                range
                                    .map(|display_idx| {
                                        // A `None` slot is a blank spacer reserved before an
                                        // oversized (card-style/heading) row so its content
                                        // has empty space to visually spill upward into
                                        // instead of overlapping the row above — see
                                        // `expand_rows_for_display`'s doc comment. Same
                                        // fixed `.h()` as a real row, just no content, so
                                        // `uniform_list`'s single-measurement layout still
                                        // sees a uniform row height everywhere.
                                        let Some(visual_idx) = display_to_wrap[display_idx] else {
                                            return div()
                                                .h(px(row_slot_px(
                                                    normal_size_px,
                                                    line_spacing,
                                                    zoom,
                                                )))
                                                .into_any_element();
                                        };
                                        let (li, row_start, row_end) = rows[visual_idx];
                                        let chars = &line_chars[li];
                                        let row_text: String =
                                            chars[row_start..row_end].iter().collect();

                                        // `.then(|| ...)` (lazy), not `.then_some(...)` — the latter's
                                        // argument is a plain value, evaluated eagerly *before* the
                                        // bool is even checked. With `then_some`, `cursor_col - row_start`
                                        // was computed for every row regardless of the condition, and
                                        // underflowed (panicked) on any row whose row_start exceeded the
                                        // cursor's column — i.e. almost any row that isn't the cursor's own.
                                        let row_cursor_col = (cursor_visual_row
                                            == Some(display_idx))
                                        .then(|| cursor_col - row_start);

                                        // Clip the logical line's selection char-range (if any) down
                                        // to this row's own [row_start, row_end) sub-range, then
                                        // rebase it to be relative to the row instead of the line.
                                        let row_selections: Vec<(usize, usize)> = selections
                                            .iter()
                                            .filter_map(|&(s, e)| {
                                                selection_span_for_line(
                                                    &lines[li],
                                                    line_byte_starts[li],
                                                    s,
                                                    e,
                                                )
                                            })
                                            .filter_map(|(sel_start, sel_end)| {
                                                let clipped_start = sel_start.max(row_start);
                                                let clipped_end = sel_end.min(row_end);
                                                // Same eager-vs-lazy pitfall as row_cursor_col above: use
                                                // `.then(|| ...)` since clipped_end can be < row_start when
                                                // the selection doesn't reach this row, which would
                                                // underflow `clipped_end - row_start` if evaluated eagerly.
                                                (clipped_start < clipped_end).then(|| {
                                                    (
                                                        clipped_start - row_start,
                                                        clipped_end - row_start,
                                                    )
                                                })
                                            })
                                            .collect();

                                        // Rich-text formatting (Phase 1): clip this logical
                                        // line's paragraph run boundaries down to this row's
                                        // own [row_start, row_end) sub-range, same rebasing
                                        // pattern as `row_selection` above — a wrapped row
                                        // only needs to know about the runs it actually spans.
                                        let row_run_spans: Vec<(usize, usize, usize)> = paragraphs
                                            .get(li)
                                            .map(paragraph_run_char_spans)
                                            .unwrap_or_default()
                                            .into_iter()
                                            .filter_map(|(rs, re, run_idx)| {
                                                let clipped_start = rs.max(row_start);
                                                let clipped_end = re.min(row_end);
                                                (clipped_start < clipped_end).then(|| {
                                                    (
                                                        clipped_start - row_start,
                                                        clipped_end - row_start,
                                                        run_idx,
                                                    )
                                                })
                                            })
                                            .collect();

                                        // Spellcheck: the logical line's misspelled ranges,
                                        // clipped and rebased onto this row exactly like
                                        // `row_selection` and `row_run_spans` above.
                                        //
                                        // Memoized per line text (see `spell_ranges_cached`),
                                        // so a keystroke re-checks only the line being edited
                                        // and scrolling is free. Measured uncached, for
                                        // reference: ~10µs per realistic card paragraph in
                                        // release, ~6x that in debug.
                                        let row_misspelled: Vec<(usize, usize)> =
                                            if spellcheck_enabled {
                                                spell_ranges_cached(
                                                    &spell_cache,
                                                    &lines[li],
                                                    &user_dictionary,
                                                )
                                                .iter()
                                                .filter_map(|&(ms, me)| {
                                                    let clipped_start = ms.max(row_start);
                                                    let clipped_end = me.min(row_end);
                                                    (clipped_start < clipped_end).then(|| {
                                                        (
                                                            clipped_start - row_start,
                                                            clipped_end - row_start,
                                                        )
                                                    })
                                                })
                                                .collect()
                                            } else {
                                                Vec::new()
                                            };

                                        // Check if previous paragraph also has box_format (for merging boxes)
                                        let prev_has_box = li > 0
                                            && paragraphs.get(li - 1).is_some_and(|p| {
                                                p.runs.iter().any(|r| r.box_format)
                                            });

                                        // Fold marker, on a heading's *first* row only — a
                                        // wrapped heading gets one marker, not one per visual
                                        // row. Hidden until the row is hovered, so a folded
                                        // outline reads as clean text rather than a column of
                                        // arrows.
                                        let row_heading =
                                            paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                                        let fold_toggle = (row_heading != 0 && row_start == 0)
                                            .then(|| {
                                                let collapsed = folded_headings.contains(&li);
                                                let state = fold_state.clone();
                                                div()
                                                    .id(ElementId::named_usize("fold-toggle", li))
                                                    .w(px(12.0 * zoom))
                                                    .ml(px(-12.0 * zoom))
                                                    .flex_none()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    // Independent of the heading's own (possibly
                                                    // very large) font size, so markers stay a
                                                    // consistent size down the outline.
                                                    .text_size(px(9.0 * zoom))
                                                    .font_weight(FontWeight::NORMAL)
                                                    // The negative margin puts the marker in the
                                                    // gutter without indenting heading text.
                                                    // Hover changes only its visibility.
                                                    .text_color(transparent_black())
                                                    .group_hover(FOLD_ROW_GROUP, move |s| {
                                                        s.text_color(rgb(p.text_muted))
                                                    })
                                                    .cursor_pointer()
                                                    .hover(move |s| s.text_color(rgb(p.text)))
                                                    .on_mouse_down(MouseButton::Left, {
                                                        // A plain closure over a cloned handle,
                                                        // not `cx.listener` — the `uniform_list`
                                                        // closure is `'static` and cannot borrow
                                                        // the view's context.
                                                        move |_ev, _window, cx: &mut App| {
                                                            // Stops the click also placing the
                                                            // caret in the heading.
                                                            cx.stop_propagation();
                                                            state.update(cx, |st, cx| {
                                                                st.toggle_paragraph_fold(li);
                                                                cx.notify();
                                                            });
                                                        }
                                                    })
                                                    .child(if collapsed { "▶" } else { "▼" })
                                                    .into_any_element()
                                            });

                                        // List marker (bullet glyph or number), on a list
                                        // paragraph's *first* row only — see
                                        // `LIST_GUTTER_PX`'s doc comment on why wrapped
                                        // continuation rows don't get one. `list_item_ordinal`
                                        // uses the same contiguous-run rule
                                        // `docx_parser::assign_list_num_ids` uses on the
                                        // write side, so what's on screen always matches
                                        // what gets saved. `(level + 1)` widths: level 0
                                        // gets one gutter's worth of indent (unchanged from
                                        // Phase 1), each deeper level shifts the marker one
                                        // more gutter-width right — approximates Word's real
                                        // per-level indent step (720 twips per
                                        // `cascade_level_xml`) as a single fixed pixel step
                                        // rather than modeling twips.
                                        let list_marker = (row_start == 0)
                                            .then(|| paragraphs.get(li).and_then(|para| para.list))
                                            .flatten()
                                            .map(|item| {
                                                let ordinal = list_item_ordinal(&paragraphs, li);
                                                div()
                                                    .id(ElementId::named_usize("list-marker", li))
                                                    .w(px(LIST_GUTTER_PX
                                                        * (item.level as f32 + 1.0)
                                                        * zoom))
                                                    .flex_none()
                                                    .flex()
                                                    .justify_end()
                                                    .pr(px(4.0 * zoom))
                                                    .text_color(rgb(p.text))
                                                    .child(list_marker_text_for_level(
                                                        item.kind, item.level, ordinal,
                                                    ))
                                                    .into_any_element()
                                            });

                                        let content_el = render_line(
                                            &row_text,
                                            row_cursor_col,
                                            &row_selections,
                                            &row_run_spans,
                                            paragraphs.get(li),
                                            prev_has_box,
                                            zoom,
                                            p,
                                            cursor_style,
                                            &row_misspelled,
                                            spellcheck_color,
                                            invisibility_mode,
                                            cite_size_half_points,
                                            fold_toggle,
                                            list_marker,
                                        );
                                        // Heading styles (spec 6.5): a paragraph-wide default
                                        // that per-run formatting (bold/size/etc., applied
                                        // inside `content_el`'s own children) still overrides
                                        // for the specific characters it covers, since GPUI's
                                        // text style cascades to children and a child's own
                                        // call wins. NOTE: a heading's larger font size can
                                        // visually overflow this row's fixed `LINE_HEIGHT_PX`
                                        // by design — `expand_rows_for_display` reserves
                                        // blank spacer rows after this one sized for exactly
                                        // that overflow (see `slot_count_for_paragraph`), so
                                        // it spills into empty space rather than the next
                                        // row's content; still needs real-hardware
                                        // verification (this sandbox has no display) to
                                        // confirm how it actually looks.
                                        let heading =
                                            paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                                        // `normal_size_px` (settings.conf's `normal_text_size`,
                                        // read once above as `normal_text_size_half_points`)
                                        // is the visual default for any run with no explicit
                                        // `FontSize` override (`size == 0` — brand-new
                                        // documents' single default run, and any plain-typed
                                        // text) — a run-level or heading-level override still
                                        // wins underneath, same as before this was configurable.
                                        // The row's base font size, and the line box that
                                        // goes with it. `.line_height()` writes into the
                                        // cascading text style, so every span in this row —
                                        // including the ones carrying a highlight background
                                        // or an emphasis ring — is laid out in a box this
                                        // tall instead of GPUI's default golden-ratio one.
                                        // See `text_line_box_px` for why that default is
                                        // what made highlights cover each other.
                                        // The same size the row's space was reserved against
                                        // (`line_font_px`), not just the heading size — a
                                        // manually enlarged run in a plain paragraph grows the
                                        // reservation, so its line box has to grow with it or
                                        // its highlight would be painted shorter than its own
                                        // glyphs.
                                        let row_font_px =
                                            line_font_px(paragraphs.get(li), zoom, normal_size_px);
                                        let row_div = div()
                                            .font_family(body_font.clone())
                                            .text_size(px(normal_size_px * zoom))
                                            .line_height(px(text_line_box_px(row_font_px)))
                                            .text_color(rgb(p.text));
                                        let row_div = match heading_font_size_px(heading, zoom) {
                                            Some(size) => row_div
                                                .text_size(px(size))
                                                .font_weight(FontWeight::BOLD),
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
                                            // `.h()`, not `.min_h()` — uniform_list measures
                                            // exactly one row (`measure_item`, always at
                                            // `list_width: None` i.e. unconstrained/MinContent
                                            // width — confirmed in the vendored gpui source,
                                            // `elements/uniform_list.rs`'s `request_layout`/
                                            // `prepaint`) and applies *that single row's*
                                            // height to *every* row in the whole list
                                            // (`item_top = item_height * item_index`, same
                                            // file). `.min_h()` only floors the height, so
                                            // any row whose content naturally measures taller
                                            // than `LINE_HEIGHT_PX` under that unconstrained-
                                            // width measurement pass — which any wrapped or
                                            // multi-span row can — poisoned every row's
                                            // spacing uniformly (found from a real bug
                                            // report: lines rendering ~2x too far apart, and
                                            // auto-scroll/scroll-to-cursor firing late since
                                            // their pixel math assumes exactly
                                            // `LINE_HEIGHT_PX` per row). An explicit `.h()`
                                            // is a fixed layout size independent of content
                                            // or measurement width, so `measure_item` always
                                            // returns exactly `LINE_HEIGHT_PX * zoom`
                                            // regardless of which row it happens to measure.
                                            // A heading's larger font can still visually
                                            // overflow this box (unchanged from before this
                                            // fix, still not clipped since overflow stays
                                            // visible — see the comment on `heading` above).
                                            .h(px(row_slot_px(normal_size_px, line_spacing, zoom)))
                                            // Column direction + justify_end bottom-aligns
                                            // `content_el` within this fixed-height slot when
                                            // it's shorter (a Shrunk line next to normal-size
                                            // ones) instead of the block-layout default of
                                            // sitting flush at the top with empty space below.
                                            // Column's *cross* axis is horizontal and defaults
                                            // to `Stretch`, so this doesn't change width
                                            // behavior — `content_el` still fills the row
                                            // exactly as it did as a plain block child, which
                                            // is what its own internal justify_center/
                                            // justify_end (paragraph alignment) and the
                                            // Pocket box's `w_full()` depend on.
                                            //
                                            // `.w_full()`: this row_div is a genuine Taffy
                                            // *root* for this layout pass — uniform_list's
                                            // paint loop calls `item.layout_as_root(available_space)`
                                            // per row (`elements/uniform_list.rs`) — and
                                            // Taffy's root-sizing carve-out that auto-stretches
                                            // an unsized node to its available space only
                                            // applies to `display: block` nodes
                                            // (`compute_root_layout`, gated on
                                            // `style.is_block()`); this is `display: flex`,
                                            // so with no width of its own it fell through to
                                            // ordinary flex content-sizing and hugged its
                                            // widest child instead — the real reason
                                            // alignment silently did nothing no matter what
                                            // was set further down the tree (`line_div`'s own
                                            // `w_full()`, `box_div`'s too, both resolve
                                            // against *this* node's width, which was never
                                            // definite). Confirmed by reading Taffy's actual
                                            // `compute_root_layout`/`perform_child_layout`
                                            // source, not guessed — this is the fourth
                                            // reported attempt at this bug, and the first
                                            // three all added width one or more levels too
                                            // low to matter.
                                            .w_full()
                                            .flex()
                                            .flex_col()
                                            .justify_end()
                                            // Marks this row as the hover group the fold
                                            // marker inside it watches.
                                            .group(FOLD_ROW_GROUP)
                                            .child(content_el)
                                            .into_any_element()
                                    })
                                    .collect()
                            }
                        })
                        // `uniform_list` is the actual scrollable element now (it
                        // sets vertical overflow internally); padding/border move
                        // here from the old outer div for the reason explained
                        // above this `.child(...)` block.
                        .with_decoration(ScrollbarDecoration {
                            scroll_handle: self.scroll_handle.clone(),
                            grab_offset: self.scrollbar_grab.clone(),
                            pressed: self.scrollbar_pressed.clone(),
                            activity: scrollbar_activity,
                            track: p.editor_bg_raised,
                            thumb: p.border,
                            thumb_hover: p.text_muted,
                        })
                        .track_scroll(&self.uniform_list_scroll_handle)
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .w_full()
                        // Prevent long lines from expanding the editor past its
                        // flex_1 allocation — uniform_list only constrains the
                        // vertical axis internally, same as the old div's own
                        // `.overflow_y_scroll()` needed an explicit
                        // `.overflow_x_hidden()` alongside it.
                        .overflow_x_hidden()
                        // Scrollbar thumb drag. `on_drag_move` dispatches in the
                        // capture phase and checks only the drag's *type*, not the
                        // pointer's position (gpui's `Interactivity::on_drag_move`),
                        // so this keeps tracking after the cursor leaves the thumb —
                        // or the editor entirely — which is what dragging a scrollbar
                        // demands. Registered here rather than on the thumb because
                        // the thumb is rebuilt every frame by the decoration.
                        .on_drag_move({
                            let scroll_handle = self.scroll_handle.clone();
                            let grab = self.scrollbar_grab.clone();
                            move |e: &DragMoveEvent<ScrollbarDragPayload>, _window, cx| {
                                let d = e.drag(cx);
                                if d.travel <= 0.0 {
                                    return;
                                }
                                // Where the thumb's *top* would sit if it followed the
                                // pointer, keeping the grab point under the cursor.
                                let thumb_top =
                                    e.event.position.y.as_f32() - grab.get() - d.track_top;
                                let fraction = (thumb_top / d.travel).clamp(0.0, 1.0);
                                let offset = scroll_handle.offset();
                                // Negative Y scrolls down, matching every other scroll
                                // path in this file.
                                scroll_handle
                                    .set_offset(point(offset.x, px(-(fraction * d.max_scroll))));
                                cx.refresh_windows();
                            }
                        })
                        .p(px(16.0))
                        // Thin focus ring so the user can tell where key input lands
                        .border_1()
                        .border_color(if is_focused {
                            rgb(p.accent)
                        } else {
                            rgb(p.editor_bg)
                        }),
                    ), // closes the "text-editor" div's .child(uniform_list...)
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
                    if !line.is_empty() {
                        line.push(' ');
                    }
                    line.push_str(pending);
                }
                div()
                    .h(px(LINE_HEIGHT_PX))
                    .px(px(16.0))
                    .bg(rgb(p.editor_bg))
                    .font_family(FONT_FAMILY)
                    .text_sm()
                    .text_color(rgb(p.text))
                    .child(line)
            })
            .when_some(
                self.state.read(cx).ui().editor_context_menu.clone(),
                |el, menu| {
                    let has_selection = self
                        .state
                        .read(cx)
                        .workspace()
                        .tabs
                        .get(self.tab_index(cx).unwrap_or(usize::MAX))
                        .is_some_and(|t| t.selection.is_some());
                    el.child(render_context_menu(menu, p, has_selection, &self.state))
                },
            )
    }
}
