use super::vim_motion::*;
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
        // Navigation is an external click: return keyboard focus to the pane
        // whose tab was moved, not just the cursor/scroll position.
        self.workspace.pending_focus_editor = Some(self.workspace.focused_pane);
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
