use super::*;

impl AppState {
    pub fn move_left(&mut self) {
        /*
         * Moves the cursor back one character boundary. Clamps at the start
         * of the document. Clears any active selection, matching plain
         * arrow-key behaviour (Shift+Left uses `extend_left` instead, which
         * shares this same char_left computation without clearing).
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = char_left(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_right(&mut self) {
        /*
         * Moves the cursor forward one character boundary. Clamps at the end
         * of the document.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = char_right(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_down(&mut self) {
        /*
         * Moves the cursor to the same character column on the next line,
         * clamped to that line's length if it's shorter. No-op on the last
         * line. Column is measured in chars (not bytes) so multi-byte
         * characters don't shift the apparent column.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = line_down(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_up(&mut self) {
        /*
         * Moves the cursor to the same character column on the previous
         * line, clamped to that line's length if it's shorter. No-op on the
         * first line.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = line_up(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_line_start(&mut self) {
        /*
         * Moves the cursor to the first byte of the current line.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = line_start(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_line_first_nonblank(&mut self) {
        /*
         * Moves the cursor to the first non-whitespace character on the
         * current line. If the line is entirely whitespace, lands at the
         * end of the line (matching vim's `^` on a blank line).
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = first_nonblank(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_line_end(&mut self) {
        /*
         * Moves the cursor to the end of the current line — the byte offset
         * of the line's trailing '\n', or the end of the document on the
         * last line (which has no trailing '\n').
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = line_end(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_word_forward(&mut self) {
        /*
         * Moves the cursor to the start of the next word, matching vim's
         * `w`. A "word" is a maximal run of alphanumeric/underscore chars,
         * OR a maximal run of other non-whitespace (punctuation) chars —
         * crossing from one class to the other, or over whitespace
         * (including newlines), ends the current word.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = word_forward(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_word_end(&mut self) {
        /*
         * Moves the cursor to the last character of the current or next
         * word, matching vim's `e`. If the cursor is already on a word's
         * last character, advances to the end of the following word.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = word_end(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_word_backward(&mut self) {
        /*
         * Moves the cursor to the start of the current or previous word,
         * matching vim's `b`.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = word_backward(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_word_forward_big(&mut self) {
        /*
         * Moves the cursor to the start of the next WORD, matching vim's
         * `W` — a WORD is any whitespace-delimited run, with no additional
         * split between alphanumeric and punctuation runs the way `w`
         * makes.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = word_forward_big(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_word_end_big(&mut self) {
        /*
         * Moves the cursor to the last character of the current or next
         * WORD, matching vim's `E`.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = word_end_big(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_word_backward_big(&mut self) {
        /*
         * Moves the cursor to the start of the current or previous WORD,
         * matching vim's `B`.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = word_backward_big(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_paragraph_forward(&mut self) {
        /*
         * Moves the cursor forward to the start of the next paragraph,
         * matching vim's `}` — a paragraph boundary is a completely blank
         * line. Always advances to a *later* blank line even if the cursor
         * is already sitting on one; lands at the end of the document if
         * there's no further blank line.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = paragraph_forward(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_paragraph_backward(&mut self) {
        /*
         * Moves the cursor backward to the start of the previous paragraph,
         * matching vim's `{`. Mirrors `move_paragraph_forward`.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = paragraph_backward(&tab.document.content(), tab.cursor);
        }
    }

    pub fn move_find_char_forward(&mut self, target: char) {
        /*
         * vim `f<char>`: moves the cursor to the next occurrence of
         * `target` on the current line, and remembers it as the most
         * recent find so `;`/`,` (spec 5.2) can repeat it. No-op —
         * including not updating the remembered find — when `target`
         * doesn't occur again before the end of the line.
         */
        self.apply_find('f', target, true);
    }

    pub fn move_find_char_backward(&mut self, target: char) {
        /*
         * vim `F<char>`: the backward counterpart to move_find_char_forward.
         */
        self.apply_find('F', target, true);
    }

    pub fn move_till_char_forward(&mut self, target: char) {
        /*
         * vim `t<char>`: moves the cursor to just before the next
         * occurrence of `target` on the current line.
         */
        self.apply_find('t', target, true);
    }

    pub fn move_till_char_backward(&mut self, target: char) {
        /*
         * vim `T<char>`: the backward counterpart to move_till_char_forward.
         */
        self.apply_find('T', target, true);
    }

    pub fn repeat_last_find(&mut self) {
        /*
         * vim `;`: repeats the most recent f/F/t/T in the same direction.
         * No-op if no find has been made yet on this tab. Does not update
         * `last_find` — repeating leaves the remembered original find
         * unchanged, matching vim (so a later `;` after a `,` still repeats
         * the *original* direction, not the reversed one).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        if let Some((kind, target)) = tab.last_find {
            self.apply_find(kind, target, false);
        }
    }

    pub fn repeat_last_find_reverse(&mut self) {
        /*
         * vim `,`: repeats the most recent f/F/t/T in the opposite
         * direction (f<->F, t<->T). See `repeat_last_find` for why
         * `last_find` itself isn't updated.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        if let Some((kind, target)) = tab.last_find {
            let reversed = match kind {
                'f' => 'F',
                'F' => 'f',
                't' => 'T',
                'T' => 't',
                other => other,
            };
            self.apply_find(reversed, target, false);
        }
    }

    pub(super) fn apply_find(&mut self, kind: char, target: char, remember: bool) {
        /*
         * Shared implementation for the four move_find/till_char_* methods
         * and the two repeat methods. `remember` controls whether this call
         * updates `last_find` (true for a fresh f/F/t/T keypress, false for
         * a `;`/`,` repeat) and doubles as the `nudge` flag for
         * `resolve_find_with_nudge` (a repeat is exactly when the nudge is
         * needed — see that function's doc comment).
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if let Some(new_pos) = resolve_find_with_nudge(
                &tab.document.content(),
                tab.cursor,
                kind,
                target,
                !remember,
            ) {
                tab.selection = None;
                tab.cursor = new_pos;
                if remember {
                    tab.last_find = Some((kind, target));
                }
            }
        }
    }

    pub fn move_doc_start(&mut self) {
        /*
         * Moves the cursor to the very start of the document.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = 0;
        }
    }

    pub fn move_doc_end(&mut self) {
        /*
         * Moves the cursor to the very end of the document.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = tab.document.content().len();
        }
    }

    pub fn move_to_line(&mut self, line: usize) {
        /*
         * Moves the cursor to the start of the given 1-indexed line number,
         * matching vim's `NG`/`Ng`. `line == 0` and `line == 1` both land on
         * the first line; a line number past the end of the document clamps
         * to the last line.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = line_offset(&tab.document.content(), line.saturating_sub(1));
        }
    }

    pub fn extend_left(&mut self) {
        /*
         * Shift+Left: moves the cursor back one character, extending (or
         * creating) the active selection instead of clearing it — see
         * `extend_selection` for how the anchor is chosen.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = char_left(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_right(&mut self) {
        /*
         * Shift+Right: the extending counterpart to move_right.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = char_right(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_up(&mut self) {
        /*
         * Shift+Up: the extending counterpart to move_up.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = line_up(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_down(&mut self) {
        /*
         * Shift+Down: the extending counterpart to move_down.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = line_down(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_word_forward(&mut self) {
        /*
         * Shift+Ctrl+Right: the extending counterpart to move_word_forward.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = word_forward(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_word_backward(&mut self) {
        /*
         * Shift+Ctrl+Left: the extending counterpart to move_word_backward.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = word_backward(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_line_start(&mut self) {
        /*
         * Shift+Home: the extending counterpart to move_line_start.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = line_start(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_line_end(&mut self) {
        /*
         * Shift+End: the extending counterpart to move_line_end.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = line_end(&tab.document.content(), tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_doc_start(&mut self) {
        /*
         * Shift+Ctrl+Home: the extending counterpart to move_doc_start.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            extend_selection(tab, 0);
        }
    }

    pub fn extend_doc_end(&mut self) {
        /*
         * Shift+Ctrl+End: the extending counterpart to move_doc_end.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = tab.document.content().len();
            extend_selection(tab, new_cursor);
        }
    }

    pub fn select_all(&mut self) {
        /*
         * Ctrl+A: selects the entire document and places the cursor at its
         * end, matching standard (non-vim) editor behaviour.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = Some((0, tab.document.content().len()));
            tab.cursor = tab.document.content().len();
        }
    }

    /// Doc Menu → "Select similar formatting": highlights every run in the
    /// document whose formatting matches the run under the caret (or, with an
    /// active selection, the run its start sits in). Word's own command of the
    /// same name.
    ///
    /// The result lands in `Tab.similar_ranges` rather than `selection`, and
    /// `selection` is blanked so the two can't both be drawn. See
    /// `similar_ranges`' own doc comment for why they are separate fields, and
    /// what does and doesn't act on it.
    pub fn select_similar_formatting(&mut self) {
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        // The selection's *start*, not the caret, when there is one — dragging
        // right-to-left leaves the caret at the low end and the anchor at the
        // high one, and the user means "the formatting I selected" either way.
        //
        // `+ 1` because `resolve_position` maps an offset sitting exactly on a
        // run boundary to the end of the *earlier* run (what typing there
        // inherits). That's right for a bare caret, but wrong for a selection:
        // its first selected byte belongs to the run on the *right*, so a
        // selection starting at a boundary would otherwise match the formatting
        // of text it doesn't cover. Byte arithmetic only — `resolve_position`
        // never slices, so landing mid-UTF-8 is harmless.
        let probe = match tab.selection {
            Some((a, f)) => a.min(f) + 1,
            None => tab.cursor,
        };
        let (para_idx, run_idx, _) = tab.document.resolve_position(probe);
        let Some(target) = tab
            .document
            .paragraphs()
            .get(para_idx)
            .and_then(|p| p.runs.get(run_idx))
        else {
            return;
        };
        tab.similar_ranges = ranges_matching_format(tab.document.paragraphs(), &target.clone());
        tab.selection = None;
    }

    /// The range a Doc Menu cleanup command acts on: the active selection
    /// when there is one, else the whole document. Shared by every Doc Menu
    /// cleanup command below.
    pub(super) fn selection_or_whole_document(&self) -> Option<(usize, usize)> {
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        Some(match tab.selection {
            Some((a, f)) => (a.min(f), a.max(f)),
            None => (0, tab.document.content().len()),
        })
    }

    /// Doc Menu → Remove emphasis: clears bold/underline/box (and the
    /// emphasis markers themselves) from any run carrying `Run.emphasis` —
    /// the marker `apply_emphasis_style` sets, not a guess from formatting.
    /// Scope is the active selection, or the whole document with none, same
    /// as its three siblings below.
    ///
    /// Marker-only, with no fallback to matching bold/underline/box
    /// combinations directly: the old heuristic couldn't tell manually-bolded
    /// plain text from Emphasis-applied text (neither had a marker) and could
    /// silently strip the former. The tradeoff is that text emphasized before
    /// this marker existed has none, and won't be caught here until
    /// re-emphasized once.
    ///
    /// Font size is left untouched — nothing here tracks what a run's size
    /// was before emphasis touched it, so there's nothing correct to revert
    /// to.
    pub fn remove_emphasis(&mut self) {
        let matches = |r: &Run| r.emphasis;

        let Some((start, end)) = self.selection_or_whole_document() else {
            return;
        };
        if start >= end {
            return;
        }
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let any = runs_in_range(tab.document.paragraphs(), start, end)
            .iter()
            .any(&matches);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let (start_para, start_run, start_char) = tab.document.resolve_position(start);
            let (end_para, end_run, end_char) = tab.document.resolve_position(end);
            // Same end-then-start split order as `apply_formatting`, so the
            // already-resolved start position isn't shifted by the end split.
            crate::document_ops::split_run_at_position(
                tab.document.paragraphs_mut(),
                end_para,
                end_run,
                end_char,
            );
            crate::document_ops::split_run_at_position(
                tab.document.paragraphs_mut(),
                start_para,
                start_run,
                start_char,
            );

            let mut cumulative = 0usize;
            for para in tab.document.paragraphs_mut().iter_mut() {
                for run in para.runs.iter_mut() {
                    let run_start = cumulative;
                    let run_end = cumulative + run.text.len();
                    if run_start >= start && run_end <= end && matches(run) {
                        run.bold = false;
                        run.underline = false;
                        run.box_format = false;
                        run.emphasis = false;
                        run.emphasis_boxed = false;
                    }
                    cumulative = run_end;
                }
                cumulative += 1;
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.document.is_modified = true;
        }
    }

    /// Doc Menu → Remove non highlighted underlining: clears `underline`
    /// (not `double_underline` — that's Hat's own marker, never plain
    /// "underlining") from every run in scope that isn't highlighted. Scope
    /// is the active selection, or the whole document with none.
    ///
    /// Blunt by design, like every Doc Menu command here: there's no per-run
    /// marker distinguishing a Block heading's structural underline from a
    /// user's own, so an unhighlighted Block gets cleared too — reapplying
    /// Block afterward is one click.
    pub fn remove_non_highlighted_underlining(&mut self) {
        let Some((start, end)) = self.selection_or_whole_document() else {
            return;
        };
        if start >= end {
            return;
        }
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let any = runs_in_range(tab.document.paragraphs(), start, end)
            .iter()
            .any(|r| r.underline && !r.highlight);
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let (start_para, start_run, start_char) = tab.document.resolve_position(start);
            let (end_para, end_run, end_char) = tab.document.resolve_position(end);
            // Same end-then-start split order as `apply_formatting`, so the
            // already-resolved start position isn't shifted by the end split.
            crate::document_ops::split_run_at_position(
                tab.document.paragraphs_mut(),
                end_para,
                end_run,
                end_char,
            );
            crate::document_ops::split_run_at_position(
                tab.document.paragraphs_mut(),
                start_para,
                start_run,
                start_char,
            );

            let mut cumulative = 0usize;
            for para in tab.document.paragraphs_mut().iter_mut() {
                for run in para.runs.iter_mut() {
                    let run_start = cumulative;
                    let run_end = cumulative + run.text.len();
                    if run_start >= start && run_end <= end && run.underline && !run.highlight {
                        run.underline = false;
                    }
                    cumulative = run_end;
                }
                cumulative += 1;
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.document.is_modified = true;
        }
    }

    /// Doc Menu → Remove blank lines: deletes every paragraph with no text
    /// (blank however it's styled), across the whole document, or — with an
    /// active selection — only those whose line falls inside it.
    pub fn remove_blank_lines(&mut self) {
        let is_blank = |p: &Paragraph| p.runs.iter().all(|r| r.text.trim().is_empty());
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        // Inclusive [first_line, last_line] the selection's bytes fall
        // across, in the same "count '\n's before the offset" terms every
        // other line-index lookup here uses (see `cursor_line_col`). `None`
        // means no selection: every paragraph is in scope.
        let line_range = tab.selection.map(|(a, f)| {
            let (start, end) = (a.min(f), a.max(f));
            (
                tab.document.content()[..start].matches('\n').count(),
                tab.document.content()[..end].matches('\n').count(),
            )
        });
        let in_scope =
            |idx: usize| line_range.is_none_or(|(first, last)| idx >= first && idx <= last);
        let any = tab
            .document
            .paragraphs()
            .iter()
            .enumerate()
            .any(|(i, p)| in_scope(i) && is_blank(p));
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let mut idx = 0usize;
            tab.document.paragraphs_mut().retain(|p| {
                let drop = in_scope(idx) && is_blank(p);
                idx += 1;
                !drop
            });
            // Every rich-text-aware function assumes at least one paragraph
            // and one run always exist (`default_paragraphs`).
            if tab.document.paragraphs().is_empty() {
                *tab.document.paragraphs_mut() = default_paragraphs();
            }
            tab.cursor = clamp_to_char_boundary(
                &tab.document.content(),
                tab.cursor.min(tab.document.content().len()),
            );
            tab.selection = None;
            tab.document.is_modified = true;
        }
    }

    /// Doc Menu → Remove pilcrows: strips every literal `¶` — the marker
    /// `condense_with_pilcrows`/a pilcrow-marked paste leaves behind for a
    /// collapsed newline — from the selection, or the whole document with
    /// none.
    ///
    /// Distinct from the `pilcrows` *setting* (`toggle_pilcrows`): that one
    /// only decides what a future condense/paste leaves behind, not what to
    /// do with `¶`s already sitting in the document.
    pub fn remove_pilcrows(&mut self) {
        let Some((start, end)) = self.selection_or_whole_document() else {
            return;
        };
        if start >= end {
            return;
        }
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        if !tab.document.content()[start..end].contains('¶') {
            return;
        }

        let stripped = tab.document.content()[start..end].replace('¶', "");
        // Mirrors `condense_selection_with`: capture the range's own runs
        // first, strip `¶` out of each run's text, then delete-and-reinsert
        // so every surviving character keeps its original formatting. A run
        // that was only a `¶` strips down to an empty string, which
        // `sync_insert_str_with_runs` silently contributes zero characters
        // for — nothing else to special-case.
        let stripped_runs: Vec<Run> = runs_in_range(tab.document.paragraphs(), start, end)
            .into_iter()
            .map(|mut r| {
                r.text = r.text.replace('¶', "");
                r
            })
            .collect();

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            sync_delete_range(tab.document.paragraphs_mut(), start, end);
            sync_insert_str_with_runs(
                tab.document.paragraphs_mut(),
                start,
                &stripped,
                &stripped_runs,
            );
            tab.cursor = start + stripped.len();
            tab.selection = None;
            tab.document.is_modified = true;
        }
    }

    /// Drops any "select similar formatting" highlight. Called from the
    /// editor's key-down and mouse-down handlers, the same two choke points
    /// that dismiss the right-click menu: the ranges are byte offsets with no
    /// way to follow an edit, so they must not outlive the next input event.
    pub fn clear_similar_selection(&mut self) {
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.similar_ranges.clear();
        }
    }

    /// Moves the cursor to the start of `line` (0-indexed) and arms
    /// `Tab.pending_scroll_to_cursor` so `TextEditor::render()` scrolls it
    /// into view on its next paint — used by the Nav menu (`FileExplorer`
    /// has no direct reference to `TextEditor` to call its own
    /// `scroll_to_cursor()` on, only this shared state). Ordinary in-editor
    /// navigation should keep calling `set_cursor_from_line_col` directly
    /// and its own `scroll_to_cursor()`, not this — this flag is a signal
    /// for cursor moves that happen from *outside* the editor view.
    pub fn jump_to_line(&mut self, line: usize) {
        self.set_cursor_from_line_col(line, 0);
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.pending_scroll_to_cursor = true;
        }
    }

    /// The byte range a Nav heading "and its contents" spans: from the start
    /// of `line` to the start of the next heading at an equal-or-shallower
    /// level (i.e. `1..=level`), or the end of the document.
    ///
    /// The **terminating newline is included** whenever there is one — decided
    /// here, once, so all four Nav actions agree. Excluding it would leave
    /// "Delete Heading and Contents" a stranded blank line where the section
    /// used to be, and make a copied section paste without its own final
    /// break.
    ///
    /// `None` when `line` isn't a heading (level 1–4) at all. Reads
    /// `paragraphs` through `.get()` rather than indexing: content lines and
    /// paragraphs are kept 1:1, but a range walk is no place to rely on that.
    pub fn heading_contents_range(&self, line: usize) -> Option<(usize, usize)> {
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let level = tab.document.paragraphs().get(line)?.heading;
        if !(1..=4).contains(&level) {
            return None;
        }
        let line_count = tab.document.content().split('\n').count();
        let end_line = (line + 1..line_count).find(|&i| {
            tab.document
                .paragraphs()
                .get(i)
                .is_some_and(|p| (1..=level).contains(&p.heading))
        });
        let start = byte_offset_for_line_col(&tab.document.content(), line, 0);
        let end = match end_line {
            // The next section's first byte — which sits just past the '\n'
            // that terminates ours, so that newline is included.
            Some(i) => byte_offset_for_line_col(&tab.document.content(), i, 0),
            None => tab.document.content().len(),
        };
        Some((start, end.max(start)))
    }

    /// Nav right-click "Select Heading and Contents": selects the whole
    /// section and scrolls it into view. The shared first step of Select /
    /// Copy / Cut / Delete — the other three just run the existing
    /// selection-scoped operation afterwards, so the Nav menu adds no
    /// second implementation of copy, cut, or delete.
    pub fn select_heading_and_contents(&mut self, line: usize) {
        let Some((start, end)) = self.heading_contents_range(line) else {
            return;
        };
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = Some((start, end));
            tab.cursor = end;
            tab.pending_scroll_to_cursor = true;
        }
    }

    pub fn set_cursor_from_line_col(&mut self, line: usize, col: usize) {
        /*
         * Places the cursor at the given 0-indexed (line, char_column) pair,
         * clamping both to the document's actual bounds — used by a plain
         * click, which derives an approximate line/column from pixel
         * coordinates and needs both ends clamped rather than panicking on
         * an out-of-range click. Inverse of `cursor_line_col`.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = None;
            tab.cursor = byte_offset_for_line_col(&tab.document.content(), line, col);
        }
    }

    pub fn extend_selection_to_line_col(&mut self, line: usize, col: usize) {
        /*
         * The click-drag counterpart to `set_cursor_from_line_col`: moves
         * the cursor to the given (line, char_column) pair while extending
         * the active selection instead of clearing it, via the same
         * `extend_selection` anchor logic every Shift+motion uses. Called
         * once per `on_mouse_move` while the left button is held — the very
         * first call naturally anchors at wherever `on_mouse_down` (which
         * clears any selection) left the cursor, since `extend_selection`
         * falls back to the current cursor when there's no selection yet.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let new_cursor = byte_offset_for_line_col(&tab.document.content(), line, col);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn select_word_at(&mut self, byte_pos: usize) {
        /*
         * Double-click word selection: selects the same contiguous
         * char-class run vim's `iw` text object would (an alphanumeric
         * word run, a punctuation run, or a whitespace run — see
         * `text_object_word`), reusing its boundary math rather than
         * reimplementing word-boundary detection for the mouse gesture.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (start, end) = text_object_word(&tab.document.content(), byte_pos, true);
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = Some((start, end));
            tab.cursor = end;
        }
    }

    pub fn select_line_at(&mut self, byte_pos: usize) {
        /*
         * Triple-click paragraph selection: reuses vim's `ip` text object
         * (`text_object_paragraph`) — the blank-line-delimited block
         * containing `byte_pos` — so a triple-click selects the whole
         * paragraph, not just the clicked line.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        if let Some((start, end)) = text_object_paragraph(&tab.document.content(), byte_pos, true) {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.selection = Some((start, end));
                tab.cursor = end;
            }
        }
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        /*
         * Maps the active tab's byte-offset cursor to a (line_index,
         * char_column) pair — both 0-indexed, column counted in characters
         * rather than bytes so multi-byte characters don't skew it. Used by
         * the renderer to place the visible cursor marker on the right line
         * div at the right character position.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return (0, 0);
        };
        let start = line_start(&tab.document.content(), tab.cursor);
        let col = tab.document.content()[start..tab.cursor].chars().count();
        let line_idx = tab.document.content()[..start].matches('\n').count();
        (line_idx, col)
    }

    /// `active_content` for a specific pane. The secondary pane is showing a
    /// different document than `active_tab` names, so it cannot go through the
    /// `active_*` helpers — those all mean "the focused pane's tab".
    pub fn pane_content(&self, pane: Pane) -> String {
        self.pane_tab_index(pane)
            .and_then(|i| self.workspace.tabs.get(i))
            .map(|t| t.document.content().to_owned())
            .unwrap_or_default()
    }

    /// `cursor_line_col` for a specific pane — see `pane_content`.
    pub fn pane_cursor_line_col(&self, pane: Pane) -> (usize, usize) {
        let Some(tab) = self
            .pane_tab_index(pane)
            .and_then(|i| self.workspace.tabs.get(i))
        else {
            return (0, 0);
        };
        let start = line_start(&tab.document.content(), tab.cursor);
        let col = tab.document.content()[start..tab.cursor].chars().count();
        let line_idx = tab.document.content()[..start].matches('\n').count();
        (line_idx, col)
    }

    pub fn active_content(&self) -> String {
        /*
         * Returns the text content of the currently active tab, or an empty
         * string if there are no tabs.
         */
        self.workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.document.content().to_owned())
            .unwrap_or_default()
    }

    /// Applies a completed background scan only if the explorer has not been
    /// pointed at another directory while it was running.
    pub fn vim_enter_insert_before_cursor(&mut self) {
        /*
         * 'i' — enters Insert mode at the current cursor position, unchanged.
         * Clears any in-progress Normal-mode count/pending-trigger buffer
         * (spec 5.2) — a stale count left over from before the mode switch
         * must not silently apply to whatever's typed after returning to
         * Normal mode later.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::Insert;
            tab.selection = None;
            tab.vim_command_buf.clear();
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
        // `.` repeat (spec 5.5): starts capturing what gets typed in this
        // Insert session — every entry point (`i`/`I`/`a`/`A`/`o`/`O`, and
        // `c`'s operator-to-Insert transition) funnels through here.
        // Committed to `last_change` when Insert exits (`vim_exit_to_normal`).
        self.global_vim.vim_insertion_recording = Some(String::new());
    }

    pub fn vim_enter_insert_line_start(&mut self) {
        /*
         * 'I' — moves to the line's first non-blank character (vim's `^`
         * semantics, not literal byte 0 of the line) before entering Insert.
         */
        self.move_line_first_nonblank();
        self.vim_enter_insert_before_cursor();
    }

    pub fn vim_enter_insert_after_cursor(&mut self) {
        /*
         * 'a' — moves one character right (clamped at document end) before
         * entering Insert, so typed text lands after the character the
         * cursor was on rather than before it.
         */
        self.move_right();
        self.vim_enter_insert_before_cursor();
    }

    pub fn vim_enter_insert_line_end(&mut self) {
        /*
         * 'A' — moves to the end of the current line before entering Insert.
         */
        self.move_line_end();
        self.vim_enter_insert_before_cursor();
    }

    pub fn vim_open_line_below(&mut self) {
        /*
         * 'o' — moves to the end of the current line and inserts a newline
         * there via insert_char (undo-tracked per Task C), which naturally
         * leaves the cursor on the new blank line created below.
         */
        self.move_line_end();
        self.insert_char('\n');
        self.vim_enter_insert_before_cursor();
    }

    pub fn vim_open_line_above(&mut self) {
        /*
         * 'O' — moves to the start of the current line and inserts a
         * newline immediately before it (undo-tracked via insert_char),
         * then pulls the cursor back onto the new blank line. insert_char
         * always advances the cursor past what it inserted, which for 'O'
         * lands it at the start of the old line now pushed down a row —
         * one line too far, unlike 'o' where that's exactly where we want
         * to end up.
         */
        self.move_line_start();
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let new_line_start = tab.cursor;
        self.insert_char('\n');
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = new_line_start;
        }
        self.vim_enter_insert_before_cursor();
    }

    pub fn vim_enter_visual(&mut self) {
        /*
         * 'v' — character-wise Visual mode, selecting the single character
         * under the cursor (matching real vim's immediate 1-char selection
         * on entry). Degenerates to a zero-width selection at document end,
         * where there's no character under the cursor. Sets `tab.cursor`
         * to the selection's far edge (not just its start) — without this,
         * the rendered cursor stays at the pre-Visual position and any
         * subsequent motion (`apply_vim_motion`, which reads `tab.cursor`
         * as its starting point) would resolve from the wrong place,
         * effectively dropping the first character of the entry selection.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::Visual;
            let end = char_right(&tab.document.content(), tab.cursor);
            tab.selection = Some((tab.cursor, end));
            tab.cursor = end;
            tab.vim_command_buf.clear(); // see vim_enter_insert_before_cursor
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
    }

    pub fn vim_enter_visual_line(&mut self) {
        /*
         * 'V' — line-wise Visual mode, selecting the whole current line
         * including its trailing newline when one exists, so a future
         * line-wise operator acts on the complete line. Sets `tab.cursor`
         * to the line's own end (not the selection's newline-inclusive far
         * edge) — same reasoning as `vim_enter_visual` for why `tab.cursor`
         * must track the selection's growing edge, but landing on the
         * line's last real character rather than past its `\n` keeps the
         * visible cursor on that line instead of appearing to jump onto
         * the next one.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::VisualLine;
            let start = line_start(&tab.document.content(), tab.cursor);
            let end = line_end(&tab.document.content(), tab.cursor);
            let end_with_newline = if end < tab.document.content().len() {
                end + 1
            } else {
                end
            };
            tab.selection = Some((start, end_with_newline));
            tab.cursor = end;
            tab.vim_command_buf.clear(); // see vim_enter_insert_before_cursor
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
    }

    pub fn vim_enter_replace(&mut self) {
        /*
         * `R` (spec 5.5) — enters Replace mode (see `VimMode::Replace`'s
         * doc comment for the scope decision behind adding a real mode).
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::Replace;
            tab.vim_command_buf.clear();
        }
    }

    pub fn vim_enter_search(&mut self, forward: bool) {
        /*
         * `/` (forward) or `?` (backward), spec 5.5 — enters Search mode.
         * `forward` is stashed so `Enter` (via `handle_vim_search_key`)
         * knows which direction to dispatch once the pattern is typed.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::Search;
            tab.vim_command_buf.clear();
            tab.vim_command_line.clear();
            tab.vim_search_direction = forward;
        }
    }

    pub fn vim_enter_command(&mut self) {
        /*
         * ':' — enters Command mode (spec 5.7). Clears any error left by a
         * previous command, matching real vim's "error persists until the
         * next `:` is opened" behavior.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::Command;
            tab.vim_command_buf.clear(); // see vim_enter_insert_before_cursor
            tab.vim_command_line.clear();
            tab.vim_command_error = None;
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
    }

    pub fn vim_exit_to_normal(&mut self) {
        /*
         * Escape (from Insert/Visual/VisualLine/Command/Replace/Search),
         * or the Visual/VisualLine toggle-off key — every "-> Normal"
         * transition in spec 5.1's table shares this one method.
         *
         * When exiting Insert mode, move the cursor back one character so it
         * lands ON the last typed character rather than after it (standard vim
         * behavior: the cursor in Normal mode is always ON a character, not
         * between characters).
         */
        let was_insert = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.vim_mode == VimMode::Insert)
            .unwrap_or(false);
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_mode = VimMode::Normal;
            tab.selection = None;
            tab.vim_command_buf.clear();
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
            // In vim, exiting Insert mode moves cursor back one char to land
            // ON the last character, not after it
            if was_insert && tab.cursor > 0 {
                tab.cursor = char_left(&tab.document.content(), tab.cursor);
            }
        }
        // `.` repeat (spec 5.5): an Insert session just ended — commit
        // what was typed, combining it with the operator that led into it
        // (`c`) if there was one.
        if was_insert {
            if let Some(text) = self.global_vim.vim_insertion_recording.take() {
                self.global_vim.last_change =
                    match self.global_vim.vim_pending_change_before_insert.take() {
                        Some((operator, keys)) => {
                            Some(VimChange::OperatorInsert(operator, keys, text))
                        }
                        None => Some(VimChange::Insertion(text)),
                    };
            }
        }
    }

    // ── Normal-mode count/pending-trigger buffer (spec 5.2) ─────────────────────

    pub(super) fn push_vim_command_buf_char(&mut self, c: char) {
        /*
         * Appends one character (a count digit, or a two-keystroke
         * command's first key like `g`/`f`/`F`/`t`/`T`) to the active tab's
         * `vim_command_buf`.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_command_buf.push(c);
        }
    }

    pub(super) fn clear_vim_command_buf(&mut self) {
        /*
         * Discards the active tab's in-progress count/pending-trigger
         * buffer — called once a Normal-mode command completes (whether it
         * was recognized or not) so a stale prefix can't bleed into the
         * next, unrelated keystroke.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_command_buf.clear();
        }
    }

    // ── macro recording/replay: q<register> / @<register> (user-requested, not in the written spec) ────────────

    pub fn vim_is_recording_macro(&self) -> bool {
        /*
         * True while a `q<register>` recording is in progress. Checked by
         * `handle_vim_normal_key` to decide whether a bare `q` should stop
         * the recording rather than start a new one.
         */
        self.global_vim.vim_macro_recording.is_some()
    }

    pub fn vim_recording_register(&self) -> Option<char> {
        /*
         * The register currently being recorded into, or `None` when not
         * recording. Used by `text_editor.rs`'s mode indicator to show
         * "recording @<register>" for the whole duration of a recording
         * (real vim shows this too) — without it, there's no feedback that
         * a recording is in progress at all until the user presses `q`
         * again to stop it.
         */
        self.global_vim
            .vim_macro_recording
            .as_ref()
            .map(|(register, _)| *register)
    }

    pub fn vim_macro_record_pending(&self) -> bool {
        /*
         * True right after a bare `q` (with nothing already recording),
         * waiting for the register character that completes `q<register>`.
         * Used by `text_editor.rs`'s mode indicator to echo the pending
         * `q` next to the mode label — this state doesn't live in
         * `vim_command_buf`, so the existing pending-command echo
         * (Task E pass 2) can't see it without this accessor.
         */
        self.global_vim.vim_macro_record_pending
    }

    pub fn vim_selected_register(&self) -> Option<char> {
        /*
         * Peeks (without consuming) the register selected by a `"<char>`
         * prefix. `text_editor.rs` uses this to detect `"+p`/`"+P` *before*
         * dispatching the keystroke, since only it has the `cx` needed to
         * read the OS clipboard — `take_vim_selected_register` is the
         * consuming counterpart used internally once an operator/paste
         * actually runs.
         */
        self.workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.vim_selected_register)
    }

    pub fn set_register(&mut self, register: char, text: String, metadata: Option<String>) {
        /*
         * Public setter so `text_editor.rs` can stage the OS clipboard's
         * text into register `'+'` right before dispatching a `"+p`/`"+P`
         * paste — the ordinary (GPUI-unaware) paste path then reads it
         * back out via `registers.get` exactly like any other register.
         *
         * `metadata` is the clipboard item's own rich-formatting metadata
         * when there is any — present when the copy came from this app,
         * absent when another app wrote the clipboard, in which case the
         * register pastes plain.
         */
        self.global_vim.registers.insert(register, text);
        match metadata {
            Some(meta) => {
                self.global_vim.register_formats.insert(register, meta);
            }
            None => {
                self.global_vim.register_formats.remove(&register);
            }
        }
    }

    pub fn take_pending_clipboard_sync(&mut self) -> Option<(String, String)> {
        /*
         * Drains the `'+'`-register write mailbox. `text_editor.rs` calls
         * this right after dispatching every vim keystroke and, if it
         * returns `Some`, pushes the text onto the real OS clipboard via
         * `cx.write_to_clipboard` — the one step this file can't do itself.
         */
        self.global_vim.pending_clipboard_sync.take()
    }

    /// Drains the vim-keybind mailbox. `text_editor.rs` calls this right
    /// after dispatching every vim keystroke, same as
    /// `take_pending_clipboard_sync`, and if it returns `Some`, dispatches
    /// the action via `window.dispatch_action` — the one step this file
    /// can't do itself (no `window`/`cx` here).
    pub fn take_pending_vim_action(&mut self) -> Option<crate::keybinds::KeybindAction> {
        self.global_vim.pending_vim_action.take()
    }

    pub(super) fn start_macro_recording(&mut self, register: char) {
        /*
         * Begins capturing keystrokes into `register`, discarding any
         * previous recording under that register (matching real vim:
         * `q<register>` always overwrites, never appends — appending needs
         * the uppercase-register form, out of scope here).
         */
        self.global_vim.vim_macro_recording = Some((register, Vec::new()));
    }

    pub fn record_macro_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        /*
         * Appends one keystroke to the in-progress recording, if any.
         * Called by `text_editor.rs` for every keystroke it sees (before
         * or after its own handling — order doesn't matter to this
         * method), so it's a no-op rather than a panic when nothing is
         * being recorded.
         */
        if let Some((_, keys)) = self.global_vim.vim_macro_recording.as_mut() {
            keys.push(RecordedVimKey {
                key: key.to_string(),
                shift,
                key_char: key_char.map(str::to_string),
            });
        }
    }

    pub(super) fn stop_macro_recording(&mut self) {
        /*
         * Ends the in-progress recording (if any) and saves it into
         * `vim_macros` under its register, overwriting whatever was there.
         */
        if let Some((register, keys)) = self.global_vim.vim_macro_recording.take() {
            self.global_vim.vim_macros.insert(register, keys);
        }
    }

    pub fn vim_is_recording_change(&self) -> bool {
        /*
         * True while a change-recordable operator (spec 5.5's `.`) is
         * pending. `text_editor.rs` checks this *before* dispatching each
         * keystroke (unlike macro recording's after-the-fact check) so
         * that the keystroke which completes the operator — ending this
         * recording — is still captured, since it's part of what `.`
         * needs to replay.
         */
        self.global_vim.vim_change_recording.is_some()
    }

    pub fn record_change_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        /*
         * Appends one completion keystroke to the in-progress change
         * recording, if any — the `.`-repeat counterpart to
         * `record_macro_key`.
         */
        if let Some(keys) = self.global_vim.vim_change_recording.as_mut() {
            keys.push(RecordedVimKey {
                key: key.to_string(),
                shift,
                key_char: key_char.map(str::to_string),
            });
        }
    }

    pub fn macro_keys(&self, register: char) -> Option<Vec<RecordedVimKey>> {
        /*
         * Returns the recorded keystrokes for `register`, or `None` if
         * nothing has ever been recorded into it. Used by
         * `text_editor.rs`'s `@<register>` replay.
         */
        self.global_vim.vim_macros.get(&register).cloned()
    }

    pub fn take_vim_count(&mut self) -> Option<usize> {
        /*
         * Parses and clears the digit-count prefix (if any) from the active
         * tab's `vim_command_buf`, leaving any trailing pending-trigger
         * character (see `vim_pending_trigger`) untouched — the count still
         * belongs to whatever two-keystroke command is in progress. Used by
         * `text_editor.rs` for `j`/`k`, which need a GPUI context
         * (`move_cursor_visual_row`) and so can't be dispatched from
         * `handle_vim_normal_key` itself. Returns `None` when no count was
         * typed, distinct from an explicit `1`.
         */
        let tab = self.workspace.tabs.get_mut(self.workspace.active_tab)?;
        let (count, _trigger) = split_vim_command_buf(&tab.vim_command_buf);
        let digit_len = tab
            .vim_command_buf
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .count();
        tab.vim_command_buf.drain(..digit_len);
        count
    }

    pub fn vim_pending_trigger(&self) -> Option<char> {
        /*
         * Returns the trailing pending-trigger character (`g`, `f`, `F`,
         * `t`, or `T`) if the active tab is mid-way through a two-keystroke
         * Normal-mode command, or `None` otherwise. Used by `text_editor.rs`
         * to decide whether `j`/`k` should be treated as a find-target
         * character (e.g. completing `fj`) instead of a cursor motion.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        split_vim_command_buf(&tab.vim_command_buf).1
    }

    pub fn vim_pending_operator(&self) -> Option<char> {
        /*
         * Returns the active tab's pending `d`/`y`/`c` operator (spec
         * 5.3), if any — the operator-sequence counterpart to
         * `vim_pending_trigger()`. Used by `text_editor.rs` for the same
         * reason: `j`/`k`/`H`/`M`/`L`/`@` are intercepted there (GPUI
         * context `handle_vim_key` doesn't have) *before* reaching
         * `handle_vim_key`, so without this check a pending `d` would let
         * `dj` silently move the cursor and leave the operator dangling
         * instead of falling through to `complete_vim_operator`, which
         * knows how to abandon it cleanly.
         */
        self.workspace
            .tabs
            .get(self.workspace.active_tab)?
            .vim_pending_operator
    }

    pub fn handle_vim_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
        /*
         * Top-level vim key dispatcher, called by text_editor.rs for every
         * keystroke while `vim_enabled` is true and the active tab isn't in
         * Insert mode (Insert falls through to plain-editor handling by the
         * caller, except for Escape which it checks separately). Returns
         * true when the key was consumed, false when the caller should fall
         * through to its own (non-vim) handling instead.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return false;
        };
        match tab.vim_mode {
            VimMode::Normal => self.handle_vim_normal_key(key, shift, key_char),
            VimMode::Visual | VimMode::VisualLine => {
                self.handle_vim_visual_key(key, shift, key_char)
            }
            VimMode::Command => {
                self.handle_vim_command_key(key, shift, key_char);
                true
            }
            VimMode::Replace => {
                self.handle_vim_replace_key(key, shift, key_char);
                true
            }
            VimMode::Search => {
                self.handle_vim_search_key(key, shift, key_char);
                true
            }
            VimMode::Insert => false,
        }
    }

    pub(super) fn handle_vim_normal_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Normal-mode key dispatch. Routes through
         * `handle_vim_motion_key(extend: false)` first — shared with Visual
         * mode's dispatch (spec 5.6: "Motions in this mode extend the
         * selection") — see its own doc comment for the full count/
         * pending-trigger state machine and motion table. Checking it
         * *before* `:` matters: a pending `f`/`F`/`t`/`T` must still treat
         * a would-be colon keypress as its target character (state 1 of
         * the shared dispatcher), not have this function hijack it into
         * Command mode first. A `None` back means `key` isn't a motion at
         * all (and no two-keystroke command is pending it might complete)
         * — only then is `:` (not part of the shared motion system; spec
         * 5.1's table has no Visual-mode `:` transition, out of scope
         * here) and Normal's own mode-switch keys (`i`/`I`/`a`/`A`/`o`/`O`/
         * `v`/`V`) checked, with anything still unrecognized swallowed —
         * real vim's Normal mode never falls through to text insertion for
         * an unmapped key.
         *
         * Macro record start/stop (`q`, user-requested — not part of editor_instructions.md) is checked first, ahead
         * of even the motion dispatcher, but ONLY when no f/F/t/T/g
         * two-keystroke command is already pending (`vim_pending_trigger()`
         * is `None`) — otherwise `fq` (find the literal character 'q')
         * would be hijacked into starting a macro instead of completing
         * the pending find, since 'q' would never reach state 1 of
         * `handle_vim_motion_key`. `@<register>` replay is handled
         * entirely in `text_editor.rs` instead, since replaying needs to
         * re-enter GPUI-context-dependent key handling (j/k/H/M/L) that
         * this method can't reach.
         *
         * A pending `d`/`y`/`c` operator (spec 5.3) is checked before even
         * that: whichever "waiting for the next key" state is already
         * active wins, and only one can be active at a time (starting an
         * operator clears `vim_command_buf`, so a pending operator and a
         * pending find/macro-register can't coexist). Without this
         * ordering, `d` then `q` would misfire as "start recording into
         * register q" instead of correctly abandoning the pending `d`
         * (real vim: an invalid motion just cancels the operator).
         */
        // Checklist: Settings -> Vim Mode. A vim-keybind sequence already
        // in progress (`Tab.vim_keybind_seq` non-empty) claims this key
        // unconditionally, checked before even the pending operator below.
        // Safe to check first because it's mutually exclusive with every
        // other pending state in this function by construction: a sequence
        // only ever *starts* via this function's final catch-all, which is
        // only reached once every other pending state has already declined
        // the keystroke — so if the buffer is non-empty, nothing else
        // could be racing it.
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .is_some_and(|t| !t.vim_keybind_seq.is_empty())
        {
            return self.continue_vim_keybind_sequence(key, shift, key_char);
        }

        if let Some(operator) = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.vim_pending_operator)
        {
            return self.complete_vim_operator(operator, key, shift, key_char);
        }

        // `r<char>` (spec 5.5): a bare `r` arms `vim_pending_replace`, then
        // the *next* keystroke overwrites the character under the cursor
        // (or cancels harmlessly on `Escape`) rather than being interpreted
        // as anything else — checked ahead of every other pending state for
        // the same reason a pending operator is: it must claim its next key
        // unconditionally.
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.vim_pending_replace)
            .unwrap_or(false)
        {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_pending_replace = false;
            }
            if key != "escape" {
                if let Some(c) = vim_find_target_char(key, shift, key_char) {
                    self.vim_replace_char(c);
                }
            }
            return true;
        }

        // `gU`/`gu` (spec 5.3, case-change operators): a `g` is already
        // pending (from `vim_command_buf`'s ordinary `g`/`gg` trigger
        // mechanism) and this key is `u`/`U`. Checked *before*
        // `handle_vim_motion_key`, which would otherwise claim this same
        // keystroke as `gg`'s pending-completion state and simply abandon
        // it (no other `g...` command exists there yet) — starting an
        // operator instead needs to happen here, one layer up, since
        // `resolve_vim_motion`'s job is resolving motions, not starting
        // operators. `is_pending_g_case_trigger` is shared with Visual
        // mode's identical detection (`handle_vim_visual_key`); only what
        // happens *after* detecting it differs (Normal mode starts a
        // pending operator, Visual mode executes immediately). Internally
        // identified as operator `'U'`/`'u'` (not `'g'`) since they're
        // two-keystroke commands, distinguished from each other only by
        // `shift` on this second key, same pattern as every other letter
        // key in this file.
        let pending_trigger = self.vim_pending_trigger();
        if self.is_pending_g_case_trigger(pending_trigger, key) {
            self.start_vim_operator(if shift { 'U' } else { 'u' });
            return true;
        }

        if pending_trigger.is_none() {
            if self.try_handle_vim_register_prefix(key, shift, key_char) {
                return true;
            }
            if self.global_vim.vim_macro_record_pending {
                self.global_vim.vim_macro_record_pending = false;
                if let Some(register) = vim_find_target_char(key, shift, key_char) {
                    self.start_macro_recording(register);
                }
                return true;
            }
            if key == "q" && !shift {
                if self.vim_is_recording_macro() {
                    self.stop_macro_recording();
                } else {
                    self.global_vim.vim_macro_record_pending = true;
                }
                return true;
            }
        }

        if let Some(result) = self.handle_vim_motion_key(key, shift, key_char, false) {
            return result;
        }

        if matches_shifted_symbol(key, shift, key_char, ";", ":") {
            self.vim_enter_command();
            return true;
        }

        if matches_shifted_symbol(key, shift, key_char, ".", ">") {
            self.start_vim_operator('>');
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, ",", "<") {
            self.start_vim_operator('<');
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "`", "~") {
            self.vim_toggle_case_char();
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "/", "?") {
            self.vim_enter_search(false);
            return true;
        }
        if key == "/" || key_char == Some("/") {
            self.vim_enter_search(true);
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "8", "*") {
            self.vim_search_word_under_cursor(true);
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "3", "#") {
            self.vim_search_word_under_cursor(false);
            return true;
        }

        match (key, shift) {
            ("i", false) => {
                self.vim_enter_insert_before_cursor();
                true
            }
            ("i", true) => {
                self.vim_enter_insert_line_start();
                true
            }
            ("a", false) => {
                self.vim_enter_insert_after_cursor();
                true
            }
            ("a", true) => {
                self.vim_enter_insert_line_end();
                true
            }
            ("o", false) => {
                self.vim_open_line_below();
                true
            }
            ("o", true) => {
                self.vim_open_line_above();
                true
            }
            ("v", false) => {
                self.vim_enter_visual();
                true
            }
            ("v", true) => {
                self.vim_enter_visual_line();
                true
            }
            ("d", false) => {
                self.start_vim_operator('d');
                true
            }
            ("y", false) => {
                self.start_vim_operator('y');
                true
            }
            ("c", false) => {
                self.start_vim_operator('c');
                true
            }
            ("p", false) => {
                self.vim_paste_register(false);
                true
            }
            ("p", true) => {
                self.vim_paste_register(true);
                true
            }
            ("x", false) => {
                self.vim_delete_char_forward();
                true
            }
            ("x", true) => {
                self.vim_delete_char_backward();
                true
            }
            ("s", false) => {
                self.vim_substitute_char();
                true
            }
            ("s", true) => {
                self.vim_substitute_line();
                true
            }
            ("j", true) => {
                self.vim_join_lines();
                true
            }
            ("r", false) => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_pending_replace = true;
                }
                true
            }
            ("r", true) => {
                self.vim_enter_replace();
                true
            }
            ("n", false) => {
                self.vim_search_next(false);
                true
            }
            ("n", true) => {
                self.vim_search_next(true);
                true
            }
            (".", false) => {
                self.vim_repeat_last_change();
                true
            }
            // Real vim's Undo — this app's vim mode never wired it up before
            // (only `g`+`u`/`U`, the case-change operator, used the letter).
            // Calls the exact same `undo()` the app-level Undo keybind does,
            // so vim's `u` and the configurable Ctrl+Z stay in sync rather
            // than tracking two separate undo stacks. Shifted `U` (real
            // vim's "undo whole line") is out of scope, left unbound.
            ("u", false) => {
                self.undo();
                true
            }
            // Checklist: Settings -> Vim Mode. The only place a *fresh*
            // vim-keybind sequence can start — every real vim command above
            // has already had first refusal, so a key that reaches here is
            // genuinely free to be claimed. A single-key binding fires
            // immediately; a longer one starts `vim_keybind_seq` for
            // `continue_vim_keybind_sequence` (checked at the very top of
            // this function) to pick up on the next keystroke. No match:
            // silently swallowed, exactly like this catch-all always has.
            _ => {
                if let Some(c) = vim_find_target_char(key, shift, key_char) {
                    self.dispatch_fresh_vim_keybind_key(c);
                }
                true
            }
        }
    }

    /// The continuation half of the vim-keybind sequence state machine —
    /// see `handle_vim_normal_key`'s own top-of-function check, which is
    /// what routes here. `Escape`, or any key that isn't a literal
    /// character (an arrow key, say), abandons the in-progress sequence
    /// rather than silently absorbing something unrelated to it.
    pub(super) fn continue_vim_keybind_sequence(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        if key == "escape" {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_keybind_seq.clear();
            }
            return true;
        }
        let Some(c) = vim_find_target_char(key, shift, key_char) else {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_keybind_seq.clear();
            }
            return true;
        };
        let seq = {
            let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
                return true;
            };
            tab.vim_keybind_seq.push(c);
            tab.vim_keybind_seq.clone()
        };
        match self.global_vim.vim_keybinds.lookup(&seq) {
            crate::vim_keybinds::VimLookup::Exact(action) => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_keybind_seq.clear();
                }
                self.global_vim.pending_vim_action = Some(action);
            }
            crate::vim_keybinds::VimLookup::Prefix => {}
            crate::vim_keybinds::VimLookup::None => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_keybind_seq.clear();
                }
            }
        }
        true
    }

    /// A single fresh character, unclaimed by every real vim command ahead
    /// of it in `handle_vim_normal_key`'s dispatch order — checks whether it
    /// starts (or, for a one-character binding, completes) a vim-keybind
    /// sequence. Shares its match-and-branch logic with
    /// `continue_vim_keybind_sequence` but starts from an empty buffer
    /// rather than an in-progress one, so it's kept as its own small
    /// function rather than forcing one path to pretend it has a buffer to
    /// continue.
    pub(super) fn dispatch_fresh_vim_keybind_key(&mut self, c: char) {
        let seq = c.to_string();
        match self.global_vim.vim_keybinds.lookup(&seq) {
            crate::vim_keybinds::VimLookup::Exact(action) => {
                self.global_vim.pending_vim_action = Some(action);
            }
            crate::vim_keybinds::VimLookup::Prefix => {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.vim_keybind_seq = seq;
                }
            }
            crate::vim_keybinds::VimLookup::None => {}
        }
    }

    pub(super) fn handle_vim_motion_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
        extend: bool,
    ) -> Option<bool> {
        /*
         * Thin wrapper around `resolve_vim_motion` for Normal mode
         * (`extend = false`: a motion moves the cursor, clearing any
         * selection) and Visual/VisualLine mode (`extend = true`: the same
         * resolved target grows the active selection instead, via
         * `apply_vim_motion` -> `extend_selection` — spec 5.6).
         *
         * The one piece of `extend`-dependent routing that isn't just
         * "apply the resolved target differently": Normal mode's existing
         * `left`/`right`/`home`/`end` "let plain navigation through"
         * convenience. `resolve_vim_motion` itself always resolves these
         * locally (as h/l/0/$ equivalents — Task F's operators need that),
         * so this wrapper intercepts them *before* calling it, but only
         * when `extend` is false — letting them fall through in Visual
         * mode would corrupt the selection via the plain editor's
         * cursor-clearing Left/Right/Home/End handling, same as before
         * this method was split.
         *
         * Returns `None` when `key` isn't part of the shared motion system
         * at all — the caller (`handle_vim_normal_key`/
         * `handle_vim_visual_key`) handles those itself. Returns
         * `Some(true)` once a motion is resolved and applied, or for
         * pending-command bookkeeping. Returns `Some(false)` to signal
         * "this key needs GPUI viewport context this method doesn't have,
         * handle it in `text_editor.rs`".
         */
        if !extend && matches!(key, "left" | "right" | "home" | "end") {
            return Some(false);
        }
        match self.resolve_vim_motion(key, shift, key_char) {
            MotionResolution::NotAMotion => None,
            MotionResolution::Pending => Some(true),
            MotionResolution::NeedsGpui => Some(false),
            MotionResolution::Resolved { target, .. } => {
                Some(self.apply_vim_motion(extend, target))
            }
        }
    }

    pub(super) fn resolve_vim_motion(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> MotionResolution {
        /*
         * Shared motion resolution — the state machine every motion-aware
         * mode (Normal cursor movement, Visual/VisualLine selection
         * extension via `handle_vim_motion_key`, and Task F's `d`/`y`/`c`
         * operators, which call this directly) is built on. Resolves a
         * keystroke down to a `MotionResolution` without applying it to
         * any cursor/selection/register — application is entirely up to
         * the caller, which is *why* this exists as its own method rather
         * than being folded back into `handle_vim_motion_key`: an operator
         * needs the same target-plus-`MotionKind` a motion produces, but
         * must build a delete/yank range from it instead of moving the
         * cursor.
         *
         * A small state machine, checked in order:
         * 1. A two-keystroke command is already pending
         *    (`vim_pending_trigger()` is `Some`) — this key completes it
         *    (`gg`'s second `g`, or an `f`/`F`/`t`/`T` target character) or
         *    abandons it otherwise. Checked first so a pending find target
         *    correctly treats any key — including `;`, `g`, or a digit —
         *    as the character to search for.
         * 2. No pending command, but this key either starts/extends a
         *    `[count]` digit prefix, or starts a new two-keystroke command
         *    (`g`, `f`, `t`, or their shifted `F`/`T` forms).
         * 3. A complete, single-key motion — any count from 1/2 is
         *    consumed here. `left`/`right`/`home`/`end` are always
         *    resolved here (as h/l/0/$ equivalents) — unlike the old,
         *    single combined method, there's no Normal-mode GPUI-
         *    fallthrough special case at this layer; that's
         *    `handle_vim_motion_key`'s concern now. `up`/`down`/`j`/`k`
         *    still always need GPUI viewport context this method doesn't
         *    have, so operators can't yet act on them either (`dj`/`dk`
         *    are a documented gap, not silently wrong).
         *
         * `$`/`^`/`{`/`}` sit on shifted number/bracket keys; `key_char`,
         * the literal key itself, and the unshifted base key + `shift` are
         * all checked (`matches_shifted_symbol`) since which one GPUI
         * actually reports isn't reliable across platforms — confirmed
         * empirically after `$` didn't fire under a narrower check.
         */
        let buf = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.vim_command_buf.clone())
            .unwrap_or_default();
        let (pending_count, pending_trigger) = split_vim_command_buf(&buf);

        // 1. Complete (or abandon) a pending two-keystroke command.
        if let Some(trigger) = pending_trigger {
            self.clear_vim_command_buf();
            match trigger {
                'g' => {
                    if key == "g" && !shift {
                        let line = pending_count.unwrap_or(1);
                        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                            let start =
                                line_offset(&tab.document.content(), line.saturating_sub(1));
                            let target = first_nonblank(&tab.document.content(), start);
                            return MotionResolution::Resolved {
                                target,
                                kind: MotionKind::Linewise,
                            };
                        }
                    }
                    // `g$`/`g0`/`g^`: the escape hatch to real vim's original
                    // (logical-line) meaning of `$`/`0`/`^`, now that bare
                    // `$`/`0`/`^` resolve to the current *visual* row instead
                    // (`text_editor.rs`'s own interception, ahead of this
                    // dispatcher, for a heavily-wrapping document editor —
                    // see its doc comment). Deliberately the exact same
                    // `line_end`/`line_start`/`first_nonblank` calls the
                    // plain (non-`g`) arms below already use — this is only
                    // reachable from a pending `g`, so it never overlaps
                    // with them.
                    else if matches_shifted_symbol(key, shift, key_char, "4", "$") {
                        let target = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|tab| line_end(&tab.document.content(), tab.cursor))
                            .unwrap_or(0);
                        return MotionResolution::Resolved {
                            target,
                            kind: MotionKind::InclusiveChar,
                        };
                    } else if key == "0" && !shift {
                        let target = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|tab| line_start(&tab.document.content(), tab.cursor))
                            .unwrap_or(0);
                        return MotionResolution::Resolved {
                            target,
                            kind: MotionKind::ExclusiveChar,
                        };
                    } else if matches_shifted_symbol(key, shift, key_char, "6", "^") {
                        let target = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|tab| first_nonblank(&tab.document.content(), tab.cursor))
                            .unwrap_or(0);
                        return MotionResolution::Resolved {
                            target,
                            kind: MotionKind::ExclusiveChar,
                        };
                    }
                    // any other key: no other `g...` command exists yet,
                    // so the sequence is simply abandoned.
                }
                'f' | 'F' | 't' | 'T' => {
                    if let Some(target_char) = vim_find_target_char(key, shift, key_char) {
                        let count = pending_count.unwrap_or(1);
                        let mut pos = self
                            .workspace
                            .tabs
                            .get(self.workspace.active_tab)
                            .map(|t| t.cursor)
                            .unwrap_or(0);
                        let mut found = false;
                        for _ in 0..count {
                            let next =
                                self.workspace
                                    .tabs
                                    .get(self.workspace.active_tab)
                                    .and_then(|t| {
                                        resolve_find(
                                            &t.document.content(),
                                            pos,
                                            trigger,
                                            target_char,
                                        )
                                    });
                            match next {
                                Some(p) => {
                                    pos = p;
                                    found = true;
                                }
                                None => break,
                            }
                        }
                        if found {
                            if let Some(tab) =
                                self.workspace.tabs.get_mut(self.workspace.active_tab)
                            {
                                tab.last_find = Some((trigger, target_char));
                            }
                            let kind = find_kind_to_motion_kind(trigger);
                            return MotionResolution::Resolved { target: pos, kind };
                        }
                    }
                }
                _ => {}
            }
            return MotionResolution::Pending;
        }

        // $/^/{/} — checked here, *before* digit-count accumulation, since
        // their unshifted base keys ("4", "6") are themselves valid count
        // digits and would otherwise be swallowed by state 2a below.
        if matches_shifted_symbol(key, shift, key_char, "4", "$") {
            self.clear_vim_command_buf();
            let target = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|tab| line_end(&tab.document.content(), tab.cursor))
                .unwrap_or(0);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::InclusiveChar,
            };
        }
        if matches_shifted_symbol(key, shift, key_char, "6", "^") {
            self.clear_vim_command_buf();
            let target = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|tab| first_nonblank(&tab.document.content(), tab.cursor))
                .unwrap_or(0);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::ExclusiveChar,
            };
        }
        if matches_shifted_symbol(key, shift, key_char, "[", "{") {
            let count = pending_count.unwrap_or(1);
            self.clear_vim_command_buf();
            let target = self.repeat_motion(count, paragraph_backward);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::ExclusiveChar,
            };
        }
        if matches_shifted_symbol(key, shift, key_char, "]", "}") {
            let count = pending_count.unwrap_or(1);
            self.clear_vim_command_buf();
            let target = self.repeat_motion(count, paragraph_forward);
            return MotionResolution::Resolved {
                target,
                kind: MotionKind::ExclusiveChar,
            };
        }

        // 2a. Digit count accumulation. A leading '0' is never a count
        // digit — it's the "start of line" motion (state 3) — but '0'
        // after an existing nonzero count extends it normally.
        if !shift && key.chars().count() == 1 {
            let c = key.chars().next().unwrap();
            if c.is_ascii_digit() && (c != '0' || pending_count.is_some()) {
                self.push_vim_command_buf_char(c);
                return MotionResolution::Pending;
            }
        }

        // 2b. Keys that start a new two-keystroke command.
        if key == "g" && !shift {
            self.push_vim_command_buf_char('g');
            return MotionResolution::Pending;
        }
        if key == "f" || key == "t" {
            let trigger = if shift {
                key.to_ascii_uppercase().chars().next().unwrap()
            } else {
                key.chars().next().unwrap()
            };
            self.push_vim_command_buf_char(trigger);
            return MotionResolution::Pending;
        }

        // 3. Complete, single-key motions. The count accumulated so far
        // (if any) is consumed here regardless of whether `key` turns out
        // to be recognized, so a stray count can't bleed into a later,
        // unrelated keystroke.
        let count = pending_count;
        self.clear_vim_command_buf();

        match (key, shift) {
            ("h", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), char_left);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("l", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), char_right);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("w", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_forward);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("w", true) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_forward_big);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("b", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_backward);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("b", true) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_backward_big);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("e", false) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_end);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::InclusiveChar,
                }
            }
            ("e", true) => {
                let t = self.repeat_motion(count.unwrap_or(1), word_end_big);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::InclusiveChar,
                }
            }
            ("0", false) => {
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| line_start(&tab.document.content(), tab.cursor))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("_", false) => {
                let c = count.unwrap_or(1);
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| underscore_motion(&tab.document.content(), tab.cursor, c))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::Linewise,
                }
            }
            ("g", true) => {
                // `G` — no count means "last line" (sentinel usize::MAX,
                // which `line_offset`'s own clamp-on-overrun handles),
                // unlike `gg`'s "no count means line 1" above.
                let line = count.unwrap_or(usize::MAX);
                if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                    let start = line_offset(&tab.document.content(), line.saturating_sub(1));
                    let target = first_nonblank(&tab.document.content(), start);
                    MotionResolution::Resolved {
                        target,
                        kind: MotionKind::Linewise,
                    }
                } else {
                    MotionResolution::Pending
                }
            }
            // The guard excludes the case where key_char indicates the
            // actual typed character was ':' despite shift reporting
            // false (the same GPUI-reliability concern matches_shifted_
            // symbol exists for) — falling through to None here lets the
            // caller's ':' check (which also consults key_char) claim it
            // as Command-mode entry instead of this repeat-find motion.
            (";", false) if key_char != Some(":") => match self.resolve_repeat_find(false) {
                Some((target, kind)) => MotionResolution::Resolved {
                    target,
                    kind: find_kind_to_motion_kind(kind),
                },
                None => MotionResolution::Pending,
            },
            (",", false) => match self.resolve_repeat_find(true) {
                Some((target, kind)) => MotionResolution::Resolved {
                    target,
                    kind: find_kind_to_motion_kind(kind),
                },
                None => MotionResolution::Pending,
            },
            ("left", _) => {
                let t = self.repeat_motion(1, char_left);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("right", _) => {
                let t = self.repeat_motion(1, char_right);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("home", _) => {
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| line_start(&tab.document.content(), tab.cursor))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::ExclusiveChar,
                }
            }
            ("end", _) => {
                let t = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|tab| line_end(&tab.document.content(), tab.cursor))
                    .unwrap_or(0);
                MotionResolution::Resolved {
                    target: t,
                    kind: MotionKind::InclusiveChar,
                }
            }
            ("up", _) | ("down", _) | ("j", false) | ("k", false) => MotionResolution::NeedsGpui,
            _ => MotionResolution::NotAMotion,
        }
    }

    pub(super) fn apply_vim_motion(&mut self, extend: bool, target: usize) -> bool {
        /*
         * The single application point every resolved motion target goes
         * through: moves the cursor and clears any selection (Normal
         * mode), or grows the active selection to `target` instead
         * (Visual/VisualLine, via the same `extend_selection` Shift+motion
         * already uses). Always returns `true` (consumed) — a thin helper
         * so every dispatch arm in `handle_vim_motion_key` can end with
         * `Some(self.apply_vim_motion(...))`.
         *
         * Also the single point that feeds the jump list (spec 5.5's
         * `Ctrl+o`/`Ctrl+i`): every Normal-mode motion lands here
         * (including `gg`/`G`, and — since `dispatch_vim_command`'s
         * `:<n>` and every search dispatch also call this — `:`-line
         * jumps and `/`/`?`/`n`/`N`/`*`/`#` too), so checking "did this
         * motion cross more than one line" once, right here, covers all
         * of `vim_todo.md`'s named "large motion" examples without
         * special-casing each call site individually. Visual-mode
         * extension (`extend`) never pushes — it's growing a selection,
         * not jumping.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if extend {
                extend_selection(tab, target);
            } else {
                if line_index_for(&tab.document.content(), target)
                    .abs_diff(line_index_for(&tab.document.content(), tab.cursor))
                    > 1
                {
                    let old_cursor = tab.cursor;
                    tab.vim_jump_back.push(old_cursor);
                    tab.vim_jump_forward.clear();
                }
                tab.selection = None;
                tab.cursor = target;
            }
        }
        true
    }

    pub fn vim_jump_backward(&mut self) {
        /*
         * `Ctrl+o` (spec 5.5): jumps to the previous position in the jump
         * list, pushing the current position onto the forward stack so
         * `Ctrl+i` can return to it — the same back/forward-stack shape
         * as `undo`/`redo`.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if let Some(pos) = tab.vim_jump_back.pop() {
            tab.vim_jump_forward.push(tab.cursor);
            tab.cursor = pos.min(tab.document.content().len());
            tab.selection = None;
        }
    }

    pub fn vim_jump_forward(&mut self) {
        /*
         * `Ctrl+i` (spec 5.5): the reverse of `vim_jump_backward`.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if let Some(pos) = tab.vim_jump_forward.pop() {
            tab.vim_jump_back.push(tab.cursor);
            tab.cursor = pos.min(tab.document.content().len());
            tab.selection = None;
        }
    }

    // ── Operators: d/y/c + dd/yy/cc (spec 5.3) ───────────────────────────────────

    pub(super) fn is_pending_g_case_trigger(
        &mut self,
        pending_trigger: Option<char>,
        key: &str,
    ) -> bool {
        /*
         * Shared by `handle_vim_normal_key` and `handle_vim_visual_key`:
         * true (after clearing the pending `g`) when a `g` is pending
         * (from `vim_command_buf`'s ordinary `g`/`gg` mechanism) and `key`
         * is `u` — the detection half of `gU`/`gu` (spec 5.3). Takes the
         * caller's already-computed `pending_trigger` rather than calling
         * `vim_pending_trigger()` again. What happens *after* this returns
         * true differs by mode (Normal starts a pending operator, Visual
         * executes immediately), so only the detection is shared, not the
         * resulting action.
         */
        if pending_trigger == Some('g') && key == "u" {
            self.clear_vim_command_buf();
            true
        } else {
            false
        }
    }

    pub(super) fn try_handle_vim_register_prefix(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Spec 5.8's `"<register>` prefix: a bare `"` arms
         * `vim_pending_register_select`, then the *next* keystroke selects
         * which register the following `d`/`y`/`c`/`p`/`P` uses (one-shot —
         * `take_vim_selected_register` consumes it). `a`-`z` and `0` select
         * that register by name (lowercased, so shift doesn't matter);
         * `+` (shift+`=` on this keyboard layout) selects the clipboard
         * register, which `write_vim_register`/`vim_paste_register` treat
         * as just another entry in `registers` — `text_editor.rs` is the
         * only place that needs to know `'+'` is special, via the
         * `pending_clipboard_sync` mailbox. Same pattern as the existing
         * macro-register-pending flow (`vim_macro_record_pending`), and
         * checked in the same place for both reasons: it's a distinct
         * "waiting for the next key" state that must claim its key before
         * anything else (motions, operators) gets a chance to.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return false;
        };
        if tab.vim_pending_register_select {
            tab.vim_pending_register_select = false;
            if matches_shifted_symbol(key, shift, key_char, "=", "+") {
                tab.vim_selected_register = Some('+');
            } else if let Some(c) = vim_find_target_char(key, shift, key_char) {
                tab.vim_selected_register = Some(c.to_ascii_lowercase());
            }
            return true;
        }
        if matches_shifted_symbol(key, shift, key_char, "'", "\"") {
            tab.vim_pending_register_select = true;
            return true;
        }
        false
    }

    pub(super) fn take_vim_selected_register(&mut self) -> char {
        self.workspace
            .tabs
            .get_mut(self.workspace.active_tab)
            .and_then(|t| t.vim_selected_register.take())
            .unwrap_or('"')
    }

    /// The rich-clipboard metadata for `content[start..end)` — the same
    /// encoding `rich_clipboard` writes for Ctrl+C, so a vim register and the
    /// system clipboard carry formatting identically and a `"+y` can hand its
    /// formatting straight to a Ctrl+V.
    ///
    /// Must be read *before* the operator mutates anything: `d`/`c`/`x`/`s`
    /// are about to delete the very runs this describes.
    pub(super) fn vim_range_metadata(&self, start: usize, end: usize) -> String {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return String::new();
        };
        crate::rich_clipboard::encode_with_lengths(
            &crate::document_ops::runs_in_range(tab.document.paragraphs(), start, end),
            &crate::document_ops::paragraph_attrs_in_range(tab.document.paragraphs(), start, end),
        )
    }

    pub(super) fn write_vim_register(&mut self, text: String, metadata: String, also_yank: bool) {
        /*
         * The single place any operator's removed/copied text lands in
         * `registers`: always the default (`'"'`) and, for `y`, also the
         * yank register (`'0'`) — mirroring real vim, whatever register
         * was explicitly named still updates `'"'` too. If the named
         * register was `'+'`, stages `pending_clipboard_sync` so
         * `text_editor.rs` can push it onto the real OS clipboard (needs
         * `cx`, which this file doesn't have).
         *
         * `metadata` (from `vim_range_metadata`) shadows every one of those
         * writes in `register_formats`, so `p` can put the formatting back
         * rather than letting the pasted text inherit the run it lands in.
         */
        let selected = self.take_vim_selected_register();
        self.global_vim.registers.insert('"', text.clone());
        self.global_vim
            .register_formats
            .insert('"', metadata.clone());
        if also_yank {
            self.global_vim.registers.insert('0', text.clone());
            self.global_vim
                .register_formats
                .insert('0', metadata.clone());
        }
        if selected != '"' {
            self.global_vim.registers.insert(selected, text.clone());
            self.global_vim
                .register_formats
                .insert(selected, metadata.clone());
            if selected == '+' {
                self.global_vim.pending_clipboard_sync = Some((text, metadata));
            }
        }
    }

    pub(super) fn vim_paste_register(&mut self, before: bool) {
        /*
         * `p`/`P` (spec 5.8). Reads (and consumes any `"<register>`
         * selection for) whichever register, defaulting to `'"'`.
         * Whether the paste is linewise or charwise is read off the
         * register text itself — "ends with `\n`" — rather than tracked
         * separately, since every linewise operator range already ends in
         * a trailing newline by construction (`linewise_bounds_for_operator`).
         * Linewise: inserts as a whole new line below (`p`) or above (`P`)
         * the cursor's line, landing on the pasted line's first non-blank.
         * Charwise: inserts right after (`p`) or right at (`P`) the
         * cursor, landing on the last pasted character.
         */
        let register = self.take_vim_selected_register();
        let Some(text) = self.global_vim.registers.get(&register).cloned() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        // The formatting yanked alongside the text (`write_vim_register`), in
        // the same encoding Ctrl+C uses. Absent — or unreadable, for a `+`
        // register another app filled — pastes plain, exactly as before:
        // empty runs make `sync_insert_str_with_runs` fall back to
        // `sync_insert_str`, and empty attrs leave paragraphs as the split
        // left them.
        let (runs, attrs) = self
            .global_vim
            .register_formats
            .get(&register)
            .and_then(|meta| crate::rich_clipboard::decode(meta, &text))
            .unwrap_or_default();
        let spanned = text.matches('\n').count() + 1;
        self.push_undo_snapshot();
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if text.ends_with('\n') {
            let insert_at = if before {
                line_start(&tab.document.content(), tab.cursor)
            } else {
                let end = line_end(&tab.document.content(), tab.cursor);
                if end < tab.document.content().len() {
                    end + 1
                } else {
                    tab.document.content().len()
                }
            };
            let needs_leading_newline = insert_at == tab.document.content().len()
                && !tab.document.content().is_empty()
                && !tab.document.content().ends_with('\n');
            let first_para = tab.document.resolve_position(insert_at).0;
            // With a leading newline the paste appends a line instead of
            // splitting `first_para`, so that paragraph keeps its own
            // attributes and the new blank line at the end must not inherit
            // them; the pasted paragraphs also start one further along.
            let dest_attrs = (!needs_leading_newline)
                .then(|| {
                    tab.document
                        .paragraphs()
                        .get(first_para)
                        .map(|p| (p.heading, p.alignment))
                })
                .flatten();
            let insertion = if needs_leading_newline {
                format!("\n{}", text)
            } else {
                text
            };
            let mut runs = runs;
            if needs_leading_newline && !runs.is_empty() {
                runs.insert(
                    0,
                    Run {
                        text: "\n".to_string(),
                        ..Run::default()
                    },
                );
            }
            crate::document_ops::sync_insert_str_with_runs(
                tab.document.paragraphs_mut(),
                insert_at,
                &insertion,
                &runs,
            );
            let landing_start = insert_at + if needs_leading_newline { 1 } else { 0 };
            crate::document_ops::apply_pasted_paragraph_attrs(
                tab.document.paragraphs_mut(),
                first_para + usize::from(needs_leading_newline),
                spanned,
                &attrs,
                dest_attrs,
            );
            tab.cursor = first_nonblank(&tab.document.content(), landing_start);
        } else {
            let at = if before {
                tab.cursor
            } else {
                char_right(&tab.document.content(), tab.cursor)
            };
            let first_para = tab.document.resolve_position(at).0;
            let dest_attrs = tab
                .document
                .paragraphs()
                .get(first_para)
                .map(|p| (p.heading, p.alignment));
            crate::document_ops::sync_insert_str_with_runs(
                tab.document.paragraphs_mut(),
                at,
                &text,
                &runs,
            );
            crate::document_ops::apply_pasted_paragraph_attrs(
                tab.document.paragraphs_mut(),
                first_para,
                spanned,
                &attrs,
                dest_attrs,
            );
            let last_char_start = text.char_indices().last().map(|(i, _)| i).unwrap_or(0);
            tab.cursor = at + last_char_start;
        }
        tab.document.is_modified = true;
    }

    pub(super) fn vim_delete_char_forward(&mut self) {
        /*
         * `x` (spec 5.5): deletes the character under the cursor, writing
         * it to the register like any `d`. Clamped to the current line —
         * real vim's `x` never deletes the trailing newline (an empty
         * line, or a cursor already at the line's end, is a no-op), unlike
         * `dl`'s more general motion-based range.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        if end == tab.cursor {
            return;
        }
        let start = tab.cursor;
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
    }

    pub(super) fn vim_delete_char_backward(&mut self) {
        /*
         * `X` (spec 5.5): deletes the character before the cursor, clamped
         * to the current line's start (a no-op at column 0).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let start = char_left(&tab.document.content(), tab.cursor)
            .max(line_start(&tab.document.content(), tab.cursor));
        if start == tab.cursor {
            return;
        }
        let end = tab.cursor;
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
    }

    pub(super) fn vim_substitute_char(&mut self) {
        /*
         * `s` (spec 5.5): `x` immediately followed by entering Insert —
         * real vim's shorthand for "delete this one character, then type
         * its replacement".
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        let start = tab.cursor;
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
        self.vim_enter_insert_before_cursor();
    }

    pub(super) fn vim_substitute_line(&mut self) {
        /*
         * `S` (spec 5.5): clears the current line's content (not the
         * trailing newline — same as `cc`) and enters Insert at its start.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let start = line_start(&tab.document.content(), tab.cursor);
        let end = line_end(&tab.document.content(), tab.cursor);
        let meta = self.vim_range_metadata(start, end);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, meta, false);
        self.vim_enter_insert_before_cursor();
    }

    pub(super) fn vim_toggle_case_char(&mut self) {
        /*
         * `~` (spec 5.5): toggles the case of the character under the
         * cursor and advances the cursor, reusing `toggle_case_vim_range`
         * (built for Visual mode's `~`). Clamped to the current line, same
         * no-op-at-EOL reasoning as `x`.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        if end == tab.cursor {
            return;
        }
        let start = tab.cursor;
        self.toggle_case_vim_range(start, end);
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = end;
        }
    }

    pub(super) fn vim_replace_char(&mut self, replacement: char) {
        /*
         * The completion half of `r<char>` (spec 5.5): overwrites the
         * character under the cursor with `replacement` and leaves the
         * cursor in place (unlike `x`/`s`, real vim's `r` doesn't move
         * it). No-op on an empty line (nothing under the cursor to
         * replace), and — unlike every `d`/`y`/`c` operator — never
         * touches any register, matching real vim.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let end = char_right(&tab.document.content(), tab.cursor)
            .min(line_end(&tab.document.content(), tab.cursor));
        if end == tab.cursor {
            return;
        }
        let cursor = tab.cursor;
        self.replace_vim_range(cursor, end, |_| replacement.to_string());
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = cursor;
        }
    }

    pub(super) fn vim_join_lines(&mut self) {
        /*
         * `J` (spec 5.5): joins the current line with the next, replacing
         * the newline and the next line's leading spaces/tabs with a
         * single space. A no-op on the last line. Simplified vs. real
         * vim's full behavior (no special-casing for lines already ending
         * in whitespace, or a next line starting with `)`).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let line_end_pos = line_end(&tab.document.content(), tab.cursor);
        if line_end_pos >= tab.document.content().len() {
            return;
        }
        let next_line_start = line_end_pos + 1;
        let next_line_end = line_end(&tab.document.content(), next_line_start);
        let trimmed_start = tab.document.content()[next_line_start..next_line_end]
            .char_indices()
            .find(|(_, c)| *c != ' ' && *c != '\t')
            .map(|(i, _)| next_line_start + i)
            .unwrap_or(next_line_end);
        self.replace_vim_range(line_end_pos, trimmed_start, |_| " ".to_string());
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = line_end_pos;
        }
    }

    pub(super) fn start_vim_operator(&mut self, operator: char) {
        /*
         * `d`/`y`/`c` pressed with no operator already pending: discards
         * any `[count]` sitting in `vim_command_buf` (a documented scope
         * limit — this first slice supports a count typed *after* the
         * operator, e.g. `d3w`, or between a doubled operator's two keys,
         * e.g. `d2d`, but not *before* it, e.g. `3dd`; combining both would
         * need multiplying two separate counts together, deliberately left
         * for a later pass) and marks the operator pending.
         */
        self.clear_vim_command_buf();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_pending_operator = Some(operator);
        }
        // `.` repeat (spec 5.5): starts capturing this operator's
        // completion keystrokes, unless it's `y` — yanking doesn't modify
        // the document, so it isn't a "change" `.` should repeat.
        if operator != 'y' {
            self.global_vim.vim_change_recording = Some(Vec::new());
        }
    }

    pub(super) fn clear_vim_pending_operator(&mut self) {
        /*
         * Ends a pending `d`/`y`/`c` sequence, whatever stage it was at
         * (plain, or mid-way through an `i`/`a` text-object prefix) —
         * the single place both fields are cleared together so neither
         * can be forgotten as new completion paths are added. Called on
         * *every* completion path (successful or abandoned) *before*
         * `execute_vim_operator_range` runs, so it must NOT touch
         * `vim_change_recording` — that still holds this keystroke and is
         * consumed by `execute_vim_operator_range` on success, or
         * explicitly discarded by the `NotAMotion`/`NeedsGpui` abandon
         * branch in `complete_vim_operator` on failure.
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
    }

    pub(super) fn complete_vim_operator(
        &mut self,
        operator: char,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Resolves the second (or third) half of an `operator[count]motion`,
         * doubled-operator (`dd`/`yy`/`cc`), or `operator[i/a]object`
         * (spec 5.4) sequence and, once resolved, executes it. Always
         * returns `true` (Normal mode swallows every keystroke while an
         * operator is pending, matching real vim rather than falling
         * through to text insertion).
         *
         * Checked in order:
         * 1. A text-object prefix (`i`/`a`) is already pending — this key
         *    names the object (`w`/`s`/`p`/a quote/a bracket char).
         *    Resolved via `vim_find_target_char` (not the raw `key`
         *    string) so shifted punctuation like `"`/`(`/`{` resolves
         *    correctly regardless of which of `key`/`key_char` GPUI
         *    happens to report it in — the same reliability concern
         *    `matches_shifted_symbol` exists for elsewhere in this file.
         * 2. The doubled-operator case (`key` matches `operator` itself),
         *    checked before delegating to `resolve_vim_motion` since
         *    `d`/`y`/`c` aren't part of the shared motion table at all —
         *    without this check the second `d` of `dd` would just resolve
         *    to `NotAMotion` and silently abandon the operator instead of
         *    running it linewise. `take_vim_count()` picks up any count
         *    typed between the two keys (`d2d`), consistent with
         *    `start_vim_operator`'s scope note.
         * 3. An `i`/`a` prefix starting a text object — also not part of
         *    the motion table, so also checked before `resolve_vim_motion`.
         * 4. Otherwise, delegate to `resolve_vim_motion`. Its `Pending`
         *    outcome (still accumulating a count or a two-keystroke motion
         *    trigger like `f`) leaves the operator pending rather than
         *    clearing it — only `Resolved`, `NeedsGpui`, and `NotAMotion`
         *    end the sequence (the latter two by abandoning it, matching
         *    real vim's "invalid motion cancels the pending operator"
         *    behaviour; `NeedsGpui` — `dj`/`dk`/`d<up>`/`d<down>` — is a
         *    documented gap, not silently wrong, since `resolve_vim_motion`
         *    has no viewport context to resolve them).
         */
        if let Some(inner) = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.vim_pending_text_object_prefix)
        {
            self.clear_vim_pending_operator();
            if let Some(object_char) = vim_find_target_char(key, shift, key_char) {
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return true;
                };
                if let Some((start, end)) =
                    resolve_vim_text_object(&tab.document.content(), tab.cursor, object_char, inner)
                {
                    self.execute_vim_operator_range(
                        operator,
                        start,
                        end,
                        MotionKind::ExclusiveChar,
                    );
                }
            }
            return true;
        }

        // The doubled-key check itself: `>`/`<` sit on shifted `.`/`,` and
        // are just as unreliable to detect via a plain string/shift
        // comparison as `$`/`^`/etc. were (same `matches_shifted_symbol`
        // reasoning) — `d`/`y`/`c` are plain unshifted letters, so the
        // simple comparison stays correct for them.
        let doubled = match operator {
            '>' => matches_shifted_symbol(key, shift, key_char, ".", ">"),
            '<' => matches_shifted_symbol(key, shift, key_char, ",", "<"),
            _ => key == operator.to_string() && !shift,
        };
        if doubled {
            let count = self.take_vim_count().unwrap_or(1);
            self.clear_vim_pending_operator();
            let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                return true;
            };
            let (start, end) =
                vim_operator_doubled_range(operator, tab.cursor, count, &tab.document.content());
            self.execute_vim_operator_range(operator, start, end, MotionKind::Linewise);
            return true;
        }

        if (key == "i" || key == "a") && !shift {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_pending_text_object_prefix = Some(key == "i");
            }
            return true;
        }

        match self.resolve_vim_motion(key, shift, key_char) {
            MotionResolution::Pending => true,
            MotionResolution::Resolved { target, kind } => {
                self.clear_vim_pending_operator();
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return true;
                };
                let (start, end) = vim_operator_motion_range(
                    operator,
                    tab.cursor,
                    target,
                    kind,
                    &tab.document.content(),
                );
                self.execute_vim_operator_range(operator, start, end, kind);
                true
            }
            MotionResolution::NeedsGpui | MotionResolution::NotAMotion => {
                self.clear_vim_pending_operator();
                // An invalid/unsupported motion abandons the operator
                // (spec 5.3) — nothing ran, so there's no change for `.`
                // to remember.
                self.global_vim.vim_change_recording = None;
                true
            }
        }
    }

    pub(super) fn execute_vim_operator_range(
        &mut self,
        operator: char,
        start: usize,
        end: usize,
        kind: MotionKind,
    ) {
        /*
         * The one place an operator's actual effect happens, given an
         * already-resolved `[start, end)` byte range (built by
         * `vim_operator_motion_range`/`vim_operator_doubled_range`, so this
         * method doesn't need to know whether it came from a motion or a
         * doubled operator). `d`/`c` write the removed text to the default
         * register (`'"'`); `y` additionally writes to `'0'`, the yank
         * register, and — unlike `d`/`c` — doesn't touch `content` at all.
         * `c` reuses `vim_enter_insert_before_cursor` for its mode
         * transition (Task D), landing in Insert at the deletion's start.
         * `>`/`<` indent/unindent every line the range spans (always
         * linewise by the time this runs — see `vim_operator_motion_range`);
         * `'U'`/`'u'` (this codebase's internal ids for `gU`/`gu`, since
         * they're two-keystroke commands, not single operator chars)
         * upper/lowercase the range's text in place.
         */
        // The range's formatting, read before `d`/`c` delete the runs it
        // describes. Only the three register-writing operators pay for it.
        let meta = matches!(operator, 'd' | 'y' | 'c')
            .then(|| self.vim_range_metadata(start, end))
            .unwrap_or_default();
        match operator {
            'd' => {
                let text = self.delete_vim_range(start, end);
                self.write_vim_register(text, meta, false);
            }
            'y' => {
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return;
                };
                let text = tab.document.content()[start..end].to_string();
                let landing = if kind == MotionKind::Linewise {
                    first_nonblank(&tab.document.content(), start)
                } else {
                    start
                };
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    tab.cursor = landing;
                    tab.selection = None;
                }
                self.write_vim_register(text, meta, true);
            }
            'c' => {
                let text = self.delete_vim_range(start, end);
                self.write_vim_register(text, meta, false);
                self.vim_enter_insert_before_cursor();
            }
            '>' => self.indent_vim_range(start, end, true),
            '<' => self.indent_vim_range(start, end, false),
            'U' => self.change_case_vim_range(start, end, true),
            'u' => self.change_case_vim_range(start, end, false),
            _ => {}
        }
        // `.` repeat (spec 5.5): commit this operator's completion
        // keystrokes now that it's actually run. `c` can't commit yet —
        // `vim_enter_insert_before_cursor` (just called above) started a
        // fresh Insert session whose typed text belongs in the same
        // change, so stash the keystrokes and let Insert's own exit
        // (`vim_exit_to_normal`) finish the commit once that text exists.
        // `y` never started a recording (see `start_vim_operator`), so
        // there's nothing to commit here for it.
        if let Some(keys) = self.global_vim.vim_change_recording.take() {
            if operator == 'c' {
                self.global_vim.vim_pending_change_before_insert = Some((operator, keys));
            } else {
                self.global_vim.last_change = Some(VimChange::Operator(operator, keys));
            }
        }
    }

    pub(super) fn replace_vim_range(
        &mut self,
        start: usize,
        end: usize,
        transform: impl FnOnce(&str) -> String,
    ) -> String {
        /*
         * Shared mutation for every operator that rewrites
         * `content[start..end]` in place (`d`/`c`'s delete, `>`/`<`'s
         * indent, `gU`/`gu`'s case-change, `~`'s case-toggle): pushes an
         * undo snapshot, replaces the range with `transform`'s output,
         * clears the selection, and marks the tab modified. Returns the
         * *original* (pre-transform) text so callers that need it
         * (delete, for registers) can use it. Deliberately doesn't set
         * the cursor — that varies by caller (delete/case-change/toggle
         * land at `start`; indent lands at the new first non-blank, which
         * needs the *post*-replace content to compute), so each caller
         * sets it themselves afterward.
         */
        self.push_undo_snapshot();
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return String::new();
        };
        let original = tab.document.content()[start..end].to_string();
        let replacement = transform(&original);
        // Every operator that rewrites a range this way (d/c/x/s/>/</gU/gu/
        // ~/r/J) gets its formatting kept in sync for free via this one
        // choke point — reduces to the same delete+insert primitives every
        // other mutation site uses.
        sync_delete_range(tab.document.paragraphs_mut(), start, end);
        sync_insert_str(tab.document.paragraphs_mut(), start, &replacement);
        tab.selection = None;
        tab.document.is_modified = true;
        original
    }

    pub(super) fn indent_vim_range(&mut self, start: usize, end: usize, indent: bool) {
        /*
         * `>`/`<`: adds or removes one leading indent unit on every line
         * `content[start..end]` spans (always whole lines by construction
         * — see `vim_operator_motion_range`). This app has no configurable
         * shiftwidth (spec doesn't define one for vim mode either), so a
         * literal tab is the indent unit, matching the plain editor's own
         * Tab-key behaviour (`text_editor.rs` inserts `'\t'`, not spaces).
         * Unindent removes one leading tab if present, else up to 4
         * leading spaces — a reasonable stand-in for "one shiftwidth" of
         * space-indented content, since there's no configured width to
         * match exactly.
         *
         * The transform rebuilds-and-splices (split on `\n`, transform
         * each line, rejoin) rather than editing in place, since
         * inserting or removing characters on an early line would
         * otherwise invalidate the byte offsets of every later line in
         * the same pass.
         */
        self.replace_vim_range(start, end, |segment| {
            let mut parts: Vec<String> = segment.split('\n').map(str::to_string).collect();
            let last = parts.len() - 1;
            for (i, line) in parts.iter_mut().enumerate() {
                if i == last && line.is_empty() {
                    // trailing empty entry from a `\n` at the very end of
                    // the segment — not a real line, leave it alone.
                    continue;
                }
                if indent {
                    line.insert(0, '\t');
                } else if line.starts_with('\t') {
                    line.remove(0);
                } else {
                    let strip = line.chars().take(4).take_while(|c| *c == ' ').count();
                    line.replace_range(0..strip, "");
                }
            }
            parts.join("\n")
        });
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = first_nonblank(&tab.document.content(), start);
        }
    }

    pub(super) fn change_case_vim_range(&mut self, start: usize, end: usize, upper: bool) {
        /*
         * `gU`/`gu`: upper/lowercases `content[start..end]` in place.
         * `String::to_uppercase`/`to_lowercase` are UTF-8-aware and may
         * change the byte length (e.g. German `ß` -> `SS`) —
         * `replace_vim_range`'s `replace_range` call handles that
         * correctly, same as every other operator mutation here.
         */
        self.replace_vim_range(start, end, |segment| {
            if upper {
                segment.to_uppercase()
            } else {
                segment.to_lowercase()
            }
        });
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = start;
        }
    }

    pub(super) fn delete_vim_range(&mut self, start: usize, end: usize) -> String {
        /*
         * The shared mutation for `d`/`c`: removes `content[start..end]`
         * and leaves the cursor at `start` — mirrors `delete_selection_
         * raw`'s undo/is_modified handling but over an explicit range
         * instead of `tab.selection`. Returns the removed text so the
         * caller can write it to a register.
         */
        let text = self.replace_vim_range(start, end, |_| String::new());
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = start;
        }
        text
    }

    pub fn vim_move_to_line_first_nonblank(&mut self, line: usize, extend: bool) {
        /*
         * Moves to (or extends the selection to, when `extend`) the first
         * non-blank character of the given 0-indexed line. Backs `H`/`M`/
         * `L` (spec 5.2), which need the live scroll position and
         * visual-row layout to know which *visual* row is currently at the
         * top/middle/bottom of the viewport — `text_editor.rs` resolves
         * that GPUI-context-dependent lookup down to a plain logical line
         * number and calls this rather than a key string, the same
         * division of labour as `j`/`k`'s `take_vim_count()`.
         */
        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
            let start = line_offset(&tab.document.content(), line);
            let target = first_nonblank(&tab.document.content(), start);
            self.apply_vim_motion(extend, target);
        }
    }

    pub(super) fn repeat_motion(&self, count: usize, motion: fn(&str, usize) -> usize) -> usize {
        /*
         * Applies a pure single-step motion function `count` times in a
         * row, starting from the active tab's cursor, without mutating
         * anything — the caller applies the final result via
         * `apply_vim_motion`. Shared by every `[count]motion` in
         * `handle_vim_motion_key` that's a simple repeated pure function
         * (h/l/w/W/b/B/e/E/{/}); f/F/t/T need their own loop since a
         * failed search should stop the repeat early rather than clamping
         * silently.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return 0;
        };
        let mut pos = tab.cursor;
        for _ in 0..count {
            pos = motion(&tab.document.content(), pos);
        }
        pos
    }

    pub(super) fn resolve_repeat_find(&self, reverse: bool) -> Option<(usize, char)> {
        /*
         * Resolves (without applying or updating `last_find`) the target
         * for `;` (`reverse = false`) or `,` (`reverse = true`) — the
         * Visual-mode-aware counterpart to `repeat_last_find`/
         * `repeat_last_find_reverse`, sharing their nudge-past-adjacent-
         * match logic via `resolve_find_with_nudge` (always nudged: a
         * repeat is exactly when it's needed). Returns `None` when there's
         * no prior find or the repeat search itself fails to find anything
         * (both true no-ops). The returned `char` is the *effective* find
         * kind actually used (post `,`-reversal) — `f`/`F`/`t`/`T` — so
         * `resolve_vim_motion` can derive the right `MotionKind` for an
         * operator without re-deriving the reversal itself.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (kind, target_char) = tab.last_find?;
        let kind = if reverse {
            match kind {
                'f' => 'F',
                'F' => 'f',
                't' => 'T',
                'T' => 't',
                k => k,
            }
        } else {
            kind
        };
        resolve_find_with_nudge(&tab.document.content(), tab.cursor, kind, target_char, true)
            .map(|pos| (pos, kind))
    }

    pub(super) fn handle_vim_visual_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> bool {
        /*
         * Visual/VisualLine key dispatch. Escape and the mode-specific
         * toggle-off key (lowercase `v` closes Visual, shifted `V` closes
         * VisualLine — spec 5.1; the mismatched key/shift combination that
         * would switch directly between the two Visual variants in real
         * vim isn't in the spec table and stays out of scope, swallowed as
         * a no-op) are checked first, since they must win over everything
         * below regardless of what it would otherwise do with the same key.
         *
         * Operators (spec 5.6: `d`/`x`, `y`, `c`, `>`, `<`, `~`, `gU`,
         * `gu`) are checked next, before the shared motion dispatcher —
         * unlike Normal mode's operators, these act *immediately* on the
         * already-existing selection rather than starting a pending
         * sequence waiting for a motion (there's no "waiting for the next
         * key" state to manage here, since the selection is already
         * there). `gU`/`gu` need their own check ahead of
         * `handle_vim_motion_key` for the same reason Normal mode's does:
         * a pending `g` (from `gg`) would otherwise claim the following
         * `u`/`U` as `gg`'s failed completion and silently abandon it.
         *
         * `o` (swap which end of the selection the cursor is on) is
         * checked after operators, since it's not a motion either but also
         * isn't an operator — it doesn't touch content or exit Visual
         * mode.
         *
         * Everything else routes through `handle_vim_motion_key(extend:
         * true)` — spec 5.6: "Motions in this mode extend the selection."
         * A `None` back means `key` isn't a motion at all: unlike Normal
         * mode, this does NOT fall back to `i`/`a`/`o` mode-switch handling
         * — in Visual mode `i`/`a` are text-object prefixes (spec 5.4) for
         * a future pass (notes/editor_instructions.md §11.1 tracks this as
         * an optional, not-yet-built extension), not insert-entry.
         * Swallowed rather than falling through to text insertion, same
         * reasoning as Normal mode. `Some(false)` (the `up`/`down`/`j`/`k`
         * GPUI-context fallthrough) is propagated as-is so `text_editor.rs`
         * can apply visual-row movement with `extend: true`.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return true;
        };
        let mode = tab.vim_mode;
        if key == "escape" {
            self.vim_exit_to_normal();
            return true;
        }

        // A pending `f`/`F`/`t`/`T`/`g` trigger must win over these checks
        // — e.g. `f` then `d` must complete as "find target 'd'", not
        // misfire as starting the delete operator (and in Visual mode,
        // `f` then `v` must complete as "find target 'v'", not misfire as
        // exiting visual mode) — the same collision class the advisor
        // flagged for Normal mode's macro/operator checks, caught here by
        // this test suite's own pre-existing regression test rather than
        // shipping unverified. `gU`/`gu`'s own check
        // (`is_pending_g_case_trigger`, shared with Normal mode) is
        // narrower (only fires when `g` specifically is pending) and must
        // stay *ahead* of `handle_vim_motion_key`, which would otherwise
        // silently claim `u` as `gg`'s failed completion first.
        let pending_trigger = self.vim_pending_trigger();
        if pending_trigger.is_none() {
            match (mode, key, shift) {
                (VimMode::Visual, "v", false) => {
                    self.vim_exit_to_normal();
                    return true;
                }
                (VimMode::VisualLine, "v", true) => {
                    self.vim_exit_to_normal();
                    return true;
                }
                _ => {}
            }
        }
        if self.is_pending_g_case_trigger(pending_trigger, key) {
            self.execute_vim_visual_operator(if shift { 'U' } else { 'u' });
            return true;
        }
        if pending_trigger.is_none() {
            if self.try_handle_vim_register_prefix(key, shift, key_char) {
                return true;
            }
            if let Some(operator) = resolve_vim_visual_operator_key(key, shift, key_char) {
                self.execute_vim_visual_operator(operator);
                return true;
            }
            if key == "o" && !shift {
                self.vim_visual_swap_ends();
                return true;
            }
        }

        self.handle_vim_motion_key(key, shift, key_char, true)
            .unwrap_or(true)
    }

    pub(super) fn vim_visual_operator_range(
        &self,
        operator: char,
    ) -> Option<(usize, usize, MotionKind)> {
        /*
         * Resolves the active tab's current selection into the
         * `(start, end, MotionKind)` an operator needs — the Visual-mode
         * counterpart to `vim_operator_motion_range`, except the range is
         * already given (the selection) rather than needing to be built
         * from a cursor/target pair.
         *
         * `VisualLine` selections are always linewise; so are `>`/`<` even
         * in plain (charwise) `Visual` mode — see `operator_forces_
         * linewise`, shared with `vim_operator_motion_range`. `c` on a
         * linewise range excludes the trailing newline — see
         * `linewise_bounds_for_operator`, also shared. Recomputes the
         * line-aligned bounds from the selection's current min/max rather
         * than trusting the selection to already sit exactly on line
         * boundaries — `VisualLine`'s selection is only guaranteed
         * line-aligned at entry (`vim_enter_visual_line`); a charwise
         * motion extending it afterward isn't specially re-snapped (a
         * separate, pre-existing gap, not fixed here), so being defensive
         * about it here is what keeps *this* method correct regardless.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (min, max) = (a.min(f), a.max(f));
        if tab.vim_mode != VimMode::VisualLine && !operator_forces_linewise(operator) {
            return Some((min, max, MotionKind::ExclusiveChar));
        }
        let last_included = if max > min { max - 1 } else { min };
        let start = line_start(&tab.document.content(), min);
        let line_end_pos = line_end(&tab.document.content(), last_included);
        let (start, end) =
            linewise_bounds_for_operator(operator, start, line_end_pos, &tab.document.content());
        Some((start, end, MotionKind::Linewise))
    }

    pub(super) fn execute_vim_visual_operator(&mut self, operator: char) {
        /*
         * Runs a Visual-mode operator (spec 5.6) against the current
         * selection and returns to Normal mode afterward — except `c`,
         * which already transitions to Insert mode on its own (via
         * `execute_vim_operator_range`'s existing `vim_enter_insert_before_
         * cursor` call, reused unchanged from Task F), so calling
         * `vim_exit_to_normal` afterward would wrongly revert that.
         * `~` (toggle case) has no Normal-mode equivalent built yet (that's
         * Task I's single-character `~`), so it gets its own small
         * `toggle_case_vim_range` rather than reusing
         * `execute_vim_operator_range`, which only knows upper/lower
         * (`gU`/`gu`), not per-character toggling.
         */
        let Some((start, end, kind)) = self.vim_visual_operator_range(operator) else {
            return;
        };
        if operator == '~' {
            self.toggle_case_vim_range(start, end);
        } else {
            self.execute_vim_operator_range(operator, start, end, kind);
        }
        if operator != 'c' {
            self.vim_exit_to_normal();
        }
    }

    pub(super) fn toggle_case_vim_range(&mut self, start: usize, end: usize) {
        /*
         * `~` in Visual mode: flips the case of every alphabetic character
         * in `content[start..end]` independently (unlike `gU`/`gu`, which
         * push everything one direction). Uses `char::to_uppercase`/
         * `to_lowercase`'s first yielded char per character rather than
         * the whole-string `String::to_uppercase`/`to_lowercase` Task F's
         * `change_case_vim_range` uses — a per-character toggle can't rely
         * on those, since each character's direction depends on its own
         * current case. A documented simplification for characters whose
         * case mapping isn't 1:1 (e.g. German `ß` -> `SS`): only the first
         * mapped character is kept.
         */
        self.replace_vim_range(start, end, |segment| {
            segment
                .chars()
                .map(|c| {
                    if c.is_uppercase() {
                        c.to_lowercase().next().unwrap_or(c)
                    } else if c.is_lowercase() {
                        c.to_uppercase().next().unwrap_or(c)
                    } else {
                        c
                    }
                })
                .collect()
        });
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = start;
        }
    }

    pub(super) fn vim_visual_swap_ends(&mut self) {
        /*
         * `o` (spec 5.6): swaps the selection's anchor and focus, moving
         * the cursor to what was previously the anchor — the highlighted
         * range itself doesn't change, only which end the cursor now sits
         * on (so a following motion extends from the *other* side).
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if let Some((a, f)) = tab.selection {
                tab.selection = Some((f, a));
                tab.cursor = a;
            }
        }
    }

    pub(super) fn capture_vim_line_input(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) -> VimLineInput {
        /*
         * Shared text-capture state machine behind both Command mode
         * (`:`, spec 5.7) and Search mode (`/`/`?`, spec 5.5) — the two
         * are mutually exclusive per tab, so sharing `vim_command_line`
         * for the typed text is safe, and their `Escape`/`Enter`/
         * `Backspace`/character-capture behavior is identical; only what
         * happens with the finished text differs, which is the caller's
         * job. `Escape` discards and reports `Cancelled`. `Enter` reports
         * `Dispatch(line)` with the accumulated text (already cleared from
         * `vim_command_line`). `Backspace` deletes the last character, or
         * reports `Cancelled` if the buffer is already empty (real vim:
         * backspacing past the leading `:`/`/`/`?` cancels). Every other
         * key resolves to a literal character via `vim_find_target_char`
         * (proven correct for shifted punctuation on this GPUI backend)
         * and is appended.
         */
        if key == "escape" {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_command_line.clear();
            }
            return VimLineInput::Cancelled;
        }
        if key == "enter" {
            let line = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|t| t.vim_command_line.clone())
                .unwrap_or_default();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_command_line.clear();
            }
            return VimLineInput::Dispatch(line);
        }
        if key == "backspace" {
            let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
                return VimLineInput::Consumed;
            };
            if tab.vim_command_line.pop().is_none() {
                return VimLineInput::Cancelled;
            }
            return VimLineInput::Consumed;
        }
        if let Some(c) = vim_find_target_char(key, shift, key_char) {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.vim_command_line.push(c);
            }
        }
        VimLineInput::Consumed
    }

    pub(super) fn handle_vim_command_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) {
        match self.capture_vim_line_input(key, shift, key_char) {
            VimLineInput::Dispatch(line) => {
                self.dispatch_vim_command(&line);
                self.vim_exit_to_normal();
            }
            VimLineInput::Cancelled => self.vim_exit_to_normal(),
            VimLineInput::Consumed => {}
        }
    }

    pub(super) fn handle_vim_search_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        match self.capture_vim_line_input(key, shift, key_char) {
            VimLineInput::Dispatch(pattern) => {
                let forward = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|t| t.vim_search_direction)
                    .unwrap_or(true);
                self.dispatch_vim_search(&pattern, forward);
                self.vim_exit_to_normal();
            }
            VimLineInput::Cancelled => self.vim_exit_to_normal(),
            VimLineInput::Consumed => {}
        }
    }

    pub(super) fn dispatch_vim_command(&mut self, line: &str) {
        /*
         * Parses and executes one of spec 5.7's Command-mode commands.
         * `line` is the text typed after `:`, already stripped of the
         * leading colon by `handle_vim_command_key`. Any error (an
         * unrecognized command, or `:q` refused on unsaved changes — real
         * vim doesn't pop a confirmation dialog, it just refuses, so this
         * mirrors that instead of building new prompt UI) is recorded in
         * `vim_command_error` for the mode indicator to show; nothing here
         * ever panics or silently no-ops without saying so, except the
         * genuinely-inert `noh` (nothing to clear until Task I's search
         * highlighting exists).
         */
        let set_error = |state: &mut Self, msg: String| {
            if let Some(tab) = state.workspace.tabs.get_mut(state.workspace.active_tab) {
                tab.vim_command_error = Some(msg);
            }
        };

        match line {
            "w" => {
                if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                    self.global_vim
                        .pending_effects
                        .push(crate::app::command::AppEffect::PerformSave(tab.id));
                }
            }
            "wa" => {
                self.global_vim.pending_effects.extend(
                    self.workspace
                        .tabs
                        .iter()
                        .map(|tab| crate::app::command::AppEffect::PerformSave(tab.id)),
                );
            }
            "q" => {
                let modified = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|t| t.document.is_modified)
                    .unwrap_or(false);
                if modified {
                    set_error(self, "E37: No write since last change".to_string());
                } else {
                    self.close_tab(self.workspace.active_tab);
                }
            }
            "q!" => self.close_tab(self.workspace.active_tab),
            "wq" | "x" => {
                if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
                    self.global_vim
                        .pending_effects
                        .push(crate::app::command::AppEffect::PerformSave(tab.id));
                }
                self.close_tab(self.workspace.active_tab);
            }
            "set vim" => self.global_vim.vim_enabled = true,
            "set novim" => self.global_vim.vim_enabled = false,
            "noh" => {} // nothing to clear yet — Task I adds search highlighting
            _ => {
                if let Some(path) = line.strip_prefix("e ") {
                    let path = self.workspace.working_directory.join(path.trim());
                    self.global_vim
                        .pending_effects
                        .push(crate::app::command::AppEffect::LoadDocument(path));
                } else if let Ok(count) = line.parse::<usize>() {
                    if count >= 1 {
                        self.vim_move_to_line_first_nonblank(count - 1, false);
                    }
                } else if let Some(rest) = line.strip_prefix("%s") {
                    if let Err(e) = self.dispatch_vim_substitute(rest) {
                        set_error(self, e);
                    }
                } else {
                    set_error(self, format!("E492: Not an editor command: {}", line));
                }
            }
        }
    }

    pub(super) fn vim_repeat_last_change(&mut self) {
        /*
         * `.` (spec 5.5): replays `last_change` at the *current* cursor
         * position. For `Operator`/`OperatorInsert`, this re-invokes
         * `start_vim_operator`/`complete_vim_operator` with the exact
         * stored completion keystrokes — since those don't need any GPUI
         * context (unlike `j`/`k`/H/M/L), the whole replay lives here in
         * `state.rs`, unlike macro replay (`@`), which needs
         * `text_editor.rs`. Re-running these also naturally re-records
         * into `vim_change_recording`/`last_change` (`start_vim_operator`/
         * `execute_vim_operator_range` don't know they're being replayed)
         * — harmless, since it just re-commits the same content.
         */
        let Some(change) = self.global_vim.last_change.clone() else {
            return;
        };
        match change {
            VimChange::Operator(operator, keys) => {
                self.start_vim_operator(operator);
                for k in &keys {
                    self.complete_vim_operator(operator, &k.key, k.shift, k.key_char.as_deref());
                }
            }
            VimChange::OperatorInsert(operator, keys, text) => {
                self.start_vim_operator(operator);
                for k in &keys {
                    self.complete_vim_operator(operator, &k.key, k.shift, k.key_char.as_deref());
                }
                self.insert_str(&text);
                self.vim_exit_to_normal();
            }
            VimChange::Insertion(text) => {
                self.insert_str(&text);
            }
        }
    }

    pub(super) fn dispatch_vim_search(&mut self, pattern: &str, forward: bool) {
        /*
         * `/pattern<Enter>` / `?pattern<Enter>` (spec 5.5). A minimal
         * `content.find`/`rfind`-with-wraparound, not a regex search — per
         * `vim_todo.md`'s explicit guidance, since a full inline find-bar
         * (highlighting, incremental search) is spec 4.6 territory and out
         * of scope here. Remembers the pattern+direction so `n`/`N` can
         * repeat it.
         */
        if pattern.is_empty() {
            return;
        }
        self.global_vim.last_search = Some((pattern.to_string(), forward));
        let cursor = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.cursor)
            .unwrap_or(0);
        self.jump_to_search_match_from(pattern, forward, cursor);
    }

    pub(super) fn jump_to_search_match_from(&mut self, pattern: &str, forward: bool, from: usize) {
        /*
         * The shared search-and-jump core: searches `pattern` starting
         * just past (`forward`) or just before (backward) `from` — never
         * matching a position the caller is already standing at — and
         * wraps around the whole document if nothing is found in that
         * direction, matching real vim's default `wrapscan` behavior.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let content = &tab.document.content();
        let found = if forward {
            let start = char_right(content, from);
            content[start..]
                .find(pattern)
                .map(|i| start + i)
                .or_else(|| content.find(pattern))
        } else {
            content[..from]
                .rfind(pattern)
                .or_else(|| content.rfind(pattern))
        };
        if let Some(pos) = found {
            self.apply_vim_motion(false, pos);
        }
    }

    pub(super) fn vim_search_next(&mut self, reverse: bool) {
        /*
         * `n`/`N` (spec 5.5): repeats the last `/`/`?`/`*`/`#` search.
         * `N` (`reverse`) searches the opposite direction from the one
         * originally used, matching real vim.
         */
        let Some((pattern, forward)) = self.global_vim.last_search.clone() else {
            return;
        };
        let effective_forward = if reverse { !forward } else { forward };
        let cursor = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.cursor)
            .unwrap_or(0);
        self.jump_to_search_match_from(&pattern, effective_forward, cursor);
    }

    pub(super) fn vim_search_word_under_cursor(&mut self, forward: bool) {
        /*
         * `*`/`#` (spec 5.5): searches for the literal word under the
         * cursor (reusing Task F's `text_object_word`), starting from
         * just past its end (`*`) or just before its start (`#`) so the
         * word the cursor is already standing in doesn't match itself.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (start, end) = text_object_word(&tab.document.content(), tab.cursor, true);
        if start == end {
            return;
        }
        let word = tab.document.content()[start..end].to_string();
        self.global_vim.last_search = Some((word.clone(), forward));
        let from = if forward { end } else { start };
        self.jump_to_search_match_from(&word, forward, from);
    }

    pub(super) fn dispatch_vim_substitute(&mut self, rest: &str) -> Result<(), String> {
        /*
         * `:%s/pattern/replacement/[g][i]` (spec 5.7) — `rest` is
         * everything after `%s`, e.g. `/foo/bar/gi`. The delimiter is
         * always `/` (real vim allows other delimiters; out of scope
         * here). Substitutes across the whole document using the `regex`
         * crate (already a dependency). Without `g`, only the first match
         * per line is replaced, matching real vim's default.
         */
        let mut parts = rest.splitn(4, '/');
        let _ = parts.next(); // text before the first '/', always empty
        let pattern = parts.next().ok_or("E486: Pattern not found")?;
        let replacement = parts.next().ok_or("E486: Pattern not found")?;
        let flags = parts.next().unwrap_or("");
        let global = flags.contains('g');
        let case_insensitive = flags.contains('i');

        let pattern_src = if case_insensitive {
            format!("(?i){}", pattern)
        } else {
            pattern.to_string()
        };
        let re = regex::Regex::new(&pattern_src).map_err(|e| format!("E486: {}", e))?;

        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return Ok(());
        };
        let old_lines: Vec<String> = tab
            .document
            .content()
            .split('\n')
            .map(|l| l.to_string())
            .collect();
        let new_lines: Vec<String> = old_lines
            .iter()
            .map(|l| {
                if global {
                    re.replace_all(l, replacement).into_owned()
                } else {
                    re.replace(l, replacement).into_owned()
                }
            })
            .collect();
        let new_content = new_lines.join("\n");

        if new_content != tab.document.content() {
            self.push_undo_snapshot();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                // Formatting sync scope limit (rich-text formatting plan,
                // Phase 1): a regex substitution has no clean per-character
                // mapping back to the original runs, so any paragraph whose
                // text actually changed gets replaced with a single default
                // (unformatted) run. Paragraphs the substitution didn't
                // touch keep their existing runs exactly.
                for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
                    if old != new {
                        if let Some(para) = tab.document.paragraphs_mut().get_mut(i) {
                            para.runs = vec![Run {
                                text: new.clone(),
                                ..Run::default()
                            }];
                        }
                    }
                }
                tab.document.is_modified = true;
                tab.cursor = tab.cursor.min(tab.document.content().len());
                tab.selection = None;
            }
        }
        Ok(())
    }

    pub(super) fn handle_vim_replace_key(
        &mut self,
        key: &str,
        shift: bool,
        key_char: Option<&str>,
    ) {
        /*
         * `R` mode (spec 5.5, `VimMode::Replace`). `Escape` returns to
         * Normal. `Backspace` moves the cursor back one character —
         * deliberately not restoring whatever it overwrote (real vim
         * tracks per-position originals so backspacing is non-destructive;
         * out of scope here, documented in `vim_todo.md`). Anything else
         * resolves to a literal character via `vim_find_target_char` (same
         * resolver as Command mode's text capture) and overwrites in place.
         */
        if key == "escape" {
            self.vim_exit_to_normal();
            return;
        }
        if key == "backspace" {
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.cursor = char_left(&tab.document.content(), tab.cursor);
            }
            return;
        }
        if let Some(c) = vim_find_target_char(key, shift, key_char) {
            self.vim_replace_mode_type_char(c);
        }
    }

    pub(super) fn vim_replace_mode_type_char(&mut self, c: char) {
        /*
         * Overwrites the character under the cursor with `c` and advances
         * past it — or, once the cursor reaches the end of the line (or
         * document), appends instead, since there's nothing left to
         * overwrite (matches real vim: Replace mode can extend a line's
         * length by typing past its original end).
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        if tab.cursor < line_end(&tab.document.content(), tab.cursor) {
            let end = char_right(&tab.document.content(), tab.cursor);
            let cursor = tab.cursor;
            self.replace_vim_range(cursor, end, |_| c.to_string());
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.cursor = cursor + c.len_utf8();
            }
        } else {
            self.insert_char(c);
        }
    }
}

pub(super) fn extend_selection(tab: &mut Tab, new_cursor: usize) {
    /*
     * Shared by every Shift+motion method: moves `tab.cursor` to
     * `new_cursor` while growing (or starting) the active selection instead
     * of clearing it. The anchor is the existing selection's anchor if one
     * is active, or the cursor's position before this move otherwise — so
     * repeated Shift+motions extend the same selection, and reversing
     * direction shrinks it back towards the anchor rather than resetting it.
     * A selection is kept as `Some((anchor, anchor))` even when it's
     * currently zero-width, so the anchor survives a Shift+motion that
     * returns exactly to the start.
     */
    let anchor = tab.selection.map(|(a, _)| a).unwrap_or(tab.cursor);
    tab.selection = Some((anchor, new_cursor));
    tab.cursor = new_cursor;
}

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
