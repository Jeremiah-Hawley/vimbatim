use super::*;

impl AppState {
    pub fn set_vim_last_macro_register(&mut self, register: char) {
        self.global_vim.vim_last_macro_register = Some(register);
    }

    pub fn set_vim_keybind(
        &mut self,
        action: crate::keybinds::KeybindAction,
        old: Option<&str>,
        sequence: String,
        path: &Path,
    ) {
        if let Some(old) = old {
            self.global_vim.vim_keybinds.remove(old);
        }
        self.global_vim.vim_keybinds.add(action, sequence);
        let _ = self.global_vim.vim_keybinds.save_to(path);
    }

    pub fn remove_vim_keybind(&mut self, sequence: &str, path: &Path) {
        self.global_vim.vim_keybinds.remove(sequence);
        let _ = self.global_vim.vim_keybinds.save_to(path);
    }

    pub fn replace_vim_settings(
        &mut self,
        keybinds: crate::vim_keybinds::VimKeybinds,
        enabled: bool,
    ) {
        self.global_vim.vim_keybinds = keybinds;
        self.global_vim.vim_enabled = enabled;
    }

    pub fn clear_vim_keybind_sequence(&mut self) {
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.vim_keybind_seq.clear();
        }
    }

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
                tab.document.paragraphs_mut_slice(),
                end_para,
                end_run,
                end_char,
            );
            crate::document_ops::split_run_at_position(
                tab.document.paragraphs_mut_slice(),
                start_para,
                start_run,
                start_char,
            );

            let mut cumulative = 0usize;
            for para in tab.document.paragraphs_mut_slice().iter_mut() {
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
                tab.document.paragraphs_mut_slice(),
                end_para,
                end_run,
                end_char,
            );
            crate::document_ops::split_run_at_position(
                tab.document.paragraphs_mut_slice(),
                start_para,
                start_run,
                start_char,
            );

            let mut cumulative = 0usize;
            for para in tab.document.paragraphs_mut_slice().iter_mut() {
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
            tab.document.retain_paragraphs(|p| {
                let drop = in_scope(idx) && is_blank(p);
                idx += 1;
                !drop
            });
            // Every rich-text-aware function assumes at least one paragraph
            // and one run always exist (`default_paragraphs`).
            if tab.document.paragraphs().is_empty() {
                tab.document.replace_paragraphs(default_paragraphs());
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
            buffer_delete_range(&mut tab.document, start, end);
            buffer_insert_str_with_runs(&mut tab.document, start, &stripped, &stripped_runs);
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
