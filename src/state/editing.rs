use super::vim::{char_right, clamp_to_char_boundary, word_backward};
use super::*;
use crate::app::command::AppCommand;
use crate::app::repository::{SettingsRepository, WorkspaceRepository};
use crate::app::store::SettingsStore;

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        /*
         * Initialises the application with a single empty tab, the sidebar visible,
         * the settings modal hidden, and the working directory restored from
         * settings.conf's `working_directory` line (`load_working_directory`),
         * falling back to `default_working_directory()` (the user's home
         * directory) when there's no persisted prior working directory. The
         * file tree is populated by scanning that directory for .docx files,
         * then persisted nav-pane expansion (`expanded_dirs`) is re-applied
         * via `file_explorer::restore_expanded_dirs`. Keybindings and vim mode
         * are loaded from settings.conf, resolved via `settings_conf_path()`
         * (next to the running executable, not the process's CWD — see that
         * function's own doc comment).
         */
        let settings_path = settings_conf_path();
        let settings_path = settings_path.as_path();
        let preferences = SettingsStore::new(settings_path.to_path_buf())
            .load_preferences()
            .unwrap_or_default();
        let working_directory = preferences
            .working_directory
            .clone()
            .unwrap_or_else(default_working_directory);

        let mut file_tree = crate::app::store::WorkspaceFs
            .scan_directory(&working_directory)
            .unwrap_or_default();
        crate::file_explorer::restore_expanded_dirs(&mut file_tree, &preferences.expanded_dirs);
        let keybinds = crate::keybinds::Keybinds::load(settings_path);
        let vim_keybinds = crate::vim_keybinds::VimKeybinds::load(settings_path);
        let vim_enabled = crate::keybinds::load_vim_enabled(settings_path);
        // Present only if the user has actually imported one — a missing
        // file is the normal "never imported" state, not an error. Like
        // `user_dictionary_path()`, always the real global path regardless
        // of the `settings_path` this constructor was given.
        let custom_theme = std::fs::read_to_string(custom_theme_path())
            .ok()
            .and_then(|s| crate::theme::parse_custom_theme_toml(&s));
        // `CardStyleKind::font_size` is the one place these defaults live, so
        // an absent settings key and the enum can't disagree.

        AppState {
            workspace: WorkspaceState {
                tabs: vec![Tab::new_empty(TabId(0))],
                active_tab: 0,
                pending_focus_editor: None,
                next_tab_id: 1,
                closed_tabs: Vec::new(),
                working_directory,
                file_tree,
                split_view: false,
                secondary_tab_id: None,
                focused_pane: Pane::Primary,
                primary_tab_id: None,
                split_ratio: 0.5,
                split_dragging: false,
            },
            ui: UiState::default(),
            global_vim: GlobalVimState {
                vim_enabled,
                vim_keybinds,
                ..Default::default()
            },
            preferences,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            copied_file: None,
            search_word_list: load_word_list(&search_word_list_path()),
            sidebar_mode: SidebarMode::default(),
            // Recovery scanning happens after the window opens on GPUI's
            // background executor; startup must not recursively walk the
            // recovery directory on the UI thread.
            recovery: RecoveryState {
                pending_entries: Vec::new(),
            },
            keybinds,
            custom_theme,
            zoom: 1.0,
            settings_path: settings_path.to_path_buf(),
            user_dictionary: Rc::new(load_user_dictionary(&user_dictionary_path())),
        }
    }

    /// Adds a word to the user dictionary and appends it to
    /// `user_dictionary.txt`, so it survives a restart.
    ///
    /// The in-memory insert is what makes the squiggles vanish: the editor
    /// recomputes misspelled ranges for visible rows every frame, so there is
    /// nothing to invalidate — the next paint simply doesn't flag it, in this
    /// document and every other open tab at once.
    pub fn add_to_user_dictionary(&mut self, word: &str) {
        // Sibling of whichever settings.conf this state is bound to, so a
        // test's temp settings path carries its dictionary along with it.
        let path = self.settings_path.with_file_name("user_dictionary.txt");
        self.add_to_user_dictionary_at(word, &path);
    }

    /// `add_to_user_dictionary` with the backing file named explicitly —
    /// matching the `load_custom_colors`/`save_custom_colors` convention in
    /// this file, and so that tests can point at a temp path instead of
    /// appending to the real user's dictionary.
    pub fn add_to_user_dictionary_at(&mut self, word: &str, path: &Path) {
        let word = word.trim().to_lowercase();
        if word.is_empty() || !Rc::make_mut(&mut self.user_dictionary).insert(word.clone()) {
            return;
        }
        // Best-effort, same as every other settings write in this file — an
        // unwritable directory shouldn't break the in-memory add.
        use std::io::Write;
        let appended = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| writeln!(f, "{word}"));
        if let Err(e) = appended {
            log_line(&format!(
                "[spellcheck] couldn't write {}: {e}",
                path.display()
            ));
        }
    }

    /// Replaces a misspelled word with a chosen suggestion.
    ///
    /// Composed entirely from methods the mouse handlers already use — select
    /// the word's (line, col) span, then type over it. That's not just brevity:
    /// `insert_str` already pushes to the undo stack and bumps
    /// `content_version`, so a spelling fix is undoable and re-snapshotted for
    /// crash recovery without this method knowing either concept exists.
    pub fn replace_spell_target(&mut self, target: &SpellTarget, replacement: &str) {
        self.set_cursor_from_line_col(target.line, target.start_col);
        self.extend_selection_to_line_col(target.line, target.end_col);
        self.insert_str(replacement);
    }

    /// Word counts for the active tab, for the word-count panel.
    ///
    /// A Tag line's words and a highlighted run's words are counted
    /// separately and summed, so text that is *both* (a highlighted word
    /// inside a tag line) counts twice — deliberate: it is read once as part
    /// of the tag, and the double-count is the conservative direction for a
    /// speech-time estimate. Highlighted words are counted per run rather than
    /// across run boundaries; adjacent same-format runs are already fused by
    /// `merge_adjacent_same_format_runs`, so a highlighted phrase is one run
    /// and counts correctly.
    pub fn document_stats(&self) -> DocumentStats {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return DocumentStats::default();
        };

        let mut stats = DocumentStats {
            total_words: count_words(&tab.document.content()),
            ..DocumentStats::default()
        };
        for para in tab.document.paragraphs() {
            // Tag is the card style at heading level 4
            // (`CardStyleKind::heading_level`).
            if para.heading == 4 {
                let text: String = para.runs.iter().map(|r| r.text.as_str()).collect();
                stats.tag_words += count_words(&text);
            }
            for run in &para.runs {
                if run.highlight {
                    stats.highlighted_words += count_words(&run.text);
                }
            }
        }
        stats.spoken_words = stats.tag_words + stats.highlighted_words;
        stats
    }

    /// Words in the active tab's selection that get read aloud — highlighted
    /// runs plus every run marked Tag or Cite. Feeds the timer's WPM readout.
    ///
    /// `None` (rather than 0) when there is no selection at all, so the caller
    /// can tell "nothing selected" from "selected text that nobody reads".
    ///
    /// Driven off run style markers, unlike `document_stats`, which predates
    /// them and still recognises a Tag by `Paragraph.heading`. Markers are what
    /// every command written since uses, and they survive reformatting; a
    /// document imported from another editor with no markers at all will
    /// under-count here.
    pub fn spoken_words_in_selection(&self) -> Option<usize> {
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return None;
        }
        Some(
            runs_in_range(tab.document.paragraphs(), start, end)
                .iter()
                .filter(|r| {
                    r.highlight || matches!(r.style, Some(CardStyle::Tag) | Some(CardStyle::Cite))
                })
                .map(|r| count_words(&r.text))
                .sum(),
        )
    }

    /// Caselist Tools → Delete tags (`delete_tags` keybind): strips Tag
    /// formatting from every tagged paragraph, leaving the words behind as
    /// ordinary body text.
    ///
    /// The *formatting* is deleted, not the line — unlike `delete_analytics`,
    /// which removes the paragraph outright. Reuses `FormatOp::ClearAll`, the
    /// same op the Clear button applies, so a de-tagged line is byte-identical
    /// to one the user cleared by hand.
    pub fn delete_tags(&mut self) {
        let is_tag = Self::tag_paragraph_test();
        let any = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.document.paragraphs().iter().any(&is_tag))
            .unwrap_or(false);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        let default_size = self.preferences.normal_text_size_half_points;
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            for para in tab.document.paragraphs_mut() {
                if !is_tag(para) {
                    continue;
                }
                for run in &mut para.runs {
                    apply_format_op(run, &FormatOp::ClearAll { default_size });
                }
                // What made the line a Tag structurally: the heading marker is
                // what the Nav outline, the fold hierarchy and `document_stats`
                // all read. Alignment is left alone — `apply_card_style` never
                // centres a Tag, so any centring here was the user's own.
                para.heading = 0;
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.document.is_modified = true;
        }
    }

    /// Recognises a Tag paragraph, on the same terms as
    /// `analytic_paragraph_test`: the run style marker is authoritative when
    /// present, and the heading level is the fallback for documents written
    /// before markers existed or by Word itself.
    fn tag_paragraph_test() -> impl Fn(&Paragraph) -> bool {
        let tag_heading = CardStyleKind::Tag.heading_level();
        move |para: &Paragraph| {
            let substantive = || para.runs.iter().filter(|r| !r.text.trim().is_empty());
            // A blank line is never a tag, however its runs are styled.
            if substantive().next().is_none() {
                return false;
            }
            if substantive().any(|r| r.style.is_some()) {
                return substantive().all(|r| r.style == Some(CardStyle::Tag));
            }
            para.heading == tag_heading
        }
    }

    // ── Find / Replace bar (spec 4.6) ───────────────────────────────────────

    /// Opens the find bar, or refocuses the query field if it's already open.
    ///
    /// Seeds the query from the current selection when there is one, matching
    /// what every editor does with Ctrl+F over selected text.
    pub(super) fn push_undo_snapshot(&mut self) {
        /*
         * Pushes the active tab's current `content` onto its undo stack
         * before a mutation, so `undo()` can later restore it. Rapid edits
         * within UNDO_COALESCE_WINDOW of the previous push are coalesced
         * into the same undo step (spec 4.5) by skipping the push entirely
         * — the snapshot already on top of the stack still reflects "before
         * this whole burst of typing", which is what one undo should revert
         * to. Any new edit clears the redo stack, since it invalidates the
         * futures those redo entries pointed to. Capped at UNDO_STACK_CAP,
         * dropping the oldest snapshot once exceeded.
         *
         * This is also this codebase's de facto "a real content mutation is
         * about to happen" choke point, so `content_version` bumps here
         * unconditionally, *before* the coalescing check below — a fast
         * typing burst must still bump it on every keystroke (uniform_list_plan.md
         * Part 1), even though only the first keystroke of the burst pushes
         * an actual undo entry.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        tab.document.content_version += 1;
        let now = Instant::now();
        let within_coalesce_window = tab
            .document
            .last_edit_at
            .map(|t| now.duration_since(t) < UNDO_COALESCE_WINDOW)
            .unwrap_or(false);
        tab.document.last_edit_at = Some(now);
        if within_coalesce_window {
            return;
        }
        tab.document
            .undo_stack
            .push(tab.document.paragraphs().to_vec());
        let cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(
            &tab.document.content(),
            tab.document.paragraphs(),
        ));
        while tab.document.undo_stack.len() > cap {
            tab.document.undo_stack.remove(0);
        }
        tab.document.redo_stack.clear();
    }

    fn delete_selection_raw(&mut self) {
        /*
         * The actual selection-deletion mutation, without pushing an undo
         * snapshot. Used internally by insert_char/insert_str/backspace,
         * which already push their own snapshot capturing the true pre-edit
         * state (selection included) before delegating here — pushing again
         * here would create a spurious intermediate undo step between "text
         * with selection" and "text with selection deleted, before the new
         * character lands".
         */
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if let Some((a, f)) = tab.selection.take() {
                let (start, end) = (a.min(f), a.max(f));
                sync_delete_range(tab.document.paragraphs_mut(), start, end);
                tab.cursor = start;
                tab.document.is_modified = true;
            }
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        /*
         * Inserts a character at the cursor position and advances the cursor.
         * If a selection is active it is deleted first, mirroring the behaviour
         * a user expects when typing over highlighted text. Pushes an undo
         * snapshot before either happens, so one undo restores the pre-edit
         * text (selection included) in a single step.
         */
        self.push_undo_snapshot();
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.delete_selection_raw();
        }
        let mut inserted_range = None;
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            sync_insert_char(tab.document.paragraphs_mut(), tab.cursor, ch);
            let start = tab.cursor;
            tab.cursor += ch.len_utf8();
            tab.document.is_modified = true;
            inserted_range = Some((start, tab.cursor));
        }
        // A pending format (spec 7: armed with no selection, per
        // `apply_formatting_to_selection`) applies to every character typed
        // until the same action is triggered again — not just this one.
        if let Some((start, end)) = inserted_range {
            let pending = self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .and_then(|t| t.pending_format.clone());
            if let Some(op) = pending {
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    apply_formatting(tab.document.paragraphs_mut(), start, end, op);
                }
            }
        }
        if let Some(rec) = self.global_vim.vim_insertion_recording.as_mut() {
            rec.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        /*
         * Deletes the character immediately before the cursor. If a selection is
         * active the whole selection is deleted instead, leaving the cursor at the
         * start of the deleted range. Pushes an undo snapshot before any actual
         * mutation — not before the at-document-start no-op check, so a no-op
         * backspace doesn't create an empty undo step.
         */
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.delete_selection(); // already pushes its own undo snapshot
            return;
        }
        let at_document_start = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.cursor == 0)
            .unwrap_or(true);
        if at_document_start {
            return;
        }

        // Backspace at the very start of a list paragraph's text un-lists it
        // (matching Word) instead of falling through to the normal
        // cross-paragraph merge/delete — a list item's marker isn't a
        // character in the buffer, so there's nothing else for this
        // backspace to visibly remove first.
        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
            let (para_idx, run_idx, char_idx) = tab.document.resolve_position(tab.cursor);
            if run_idx == 0
                && char_idx == 0
                && tab
                    .document
                    .paragraphs()
                    .get(para_idx)
                    .is_some_and(|p| p.list.is_some())
            {
                self.push_undo_snapshot();
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    if let Some(para) = tab.document.paragraphs_mut().get_mut(para_idx) {
                        para.list = None;
                    }
                    tab.document.is_modified = true;
                }
                return;
            }
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            // Walk back one char boundary
            let prev = tab.document.content()[..tab.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            sync_delete_range(tab.document.paragraphs_mut(), prev, tab.cursor);
            tab.cursor = prev;
            tab.document.is_modified = true;
        }
        if let Some(rec) = self.global_vim.vim_insertion_recording.as_mut() {
            rec.pop();
        }
    }

    /// The Delete key: deletes the character immediately *after* the cursor —
    /// `backspace`'s forward counterpart. If a selection is active the whole
    /// selection is deleted instead, same as `backspace`. Doesn't touch
    /// `vim_insertion_recording`: that tracks characters just typed for `.`
    /// repeat, and this removes a character ahead of the cursor, never one of
    /// those.
    pub fn delete_forward(&mut self) {
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.delete_selection(); // already pushes its own undo snapshot
            return;
        }
        let at_document_end = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.cursor >= t.document.content().len())
            .unwrap_or(true);
        if at_document_end {
            return;
        }
        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            let next = char_right(&tab.document.content(), tab.cursor);
            sync_delete_range(tab.document.paragraphs_mut(), tab.cursor, next);
            tab.document.is_modified = true;
        }
    }

    pub fn delete_selection(&mut self) {
        /*
         * Public entry point for deleting the active selection as its own
         * standalone edit (e.g. Cut, or a future Delete key) — pushes an
         * undo snapshot first (only when there's actually a selection to
         * delete, so a no-op call doesn't create an empty undo step), then
         * delegates to the raw deletion. Clears the selection. No-op when
         * `selection` is `None`.
         */
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.push_undo_snapshot();
        }
        self.delete_selection_raw();
    }

    pub fn delete_word_backward(&mut self) {
        /*
         * Ctrl+Backspace: deletes the word immediately before the cursor.
         * Reuses `word_backward` (the same boundary math vim's `b` motion
         * is built on, see `move_word_backward` above) rather than
         * reimplementing word-boundary detection — its walk-back-over-
         * whitespace-then-over-the-word logic is exactly what "delete the
         * previous word" needs, and it already handles the mid-word-cursor
         * and start-of-document no-op cases vim's `b` has to.
         *
         * If a selection is active, deletes it instead (matching
         * `backspace`'s convention) rather than word-deleting from one of
         * its edges, which would be a confusing thing to do to a selection
         * the user can already see.
         */
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.delete_selection();
            return;
        }
        let Some((start, cursor)) = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| (word_backward(&t.document.content(), t.cursor), t.cursor))
        else {
            return;
        };
        if start == cursor {
            return;
        }
        self.push_undo_snapshot();
        let mut deleted_chars = 0;
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            deleted_chars = tab.document.content()[start..cursor].chars().count();
            sync_delete_range(tab.document.paragraphs_mut(), start, cursor);
            tab.cursor = start;
            tab.selection = None;
            tab.document.is_modified = true;
        }
        // Mirrors `backspace`'s per-char `rec.pop()`, scaled to a whole
        // word: truncate the recording by the same number of chars actually
        // removed, so vim's `.`-repeat (`VimChange::Insertion`) doesn't
        // replay text this deletion already erased. Capped at the
        // recording's own length — the deleted range can reach back past
        // where the current insertion segment started, into text that was
        // never part of this recording (same as `backspace`'s `pop()`
        // being a no-op once `rec` runs out).
        if let Some(rec) = self.global_vim.vim_insertion_recording.as_mut() {
            let rec_chars = rec.chars().count();
            let new_len = rec_chars.saturating_sub(deleted_chars);
            let byte_idx = rec
                .char_indices()
                .nth(new_len)
                .map(|(i, _)| i)
                .unwrap_or(rec.len());
            rec.truncate(byte_idx);
        }
    }

    pub fn apply_formatting_to_line(&mut self, op: FormatOp) {
        /*
         * Applies formatting to the entire line containing the cursor.
         * Used for card styles (Pocket, Hat, Block) which should format
         * the entire line, not just selected text.
         *
         * When applied to an empty line, also arms pending_format so that
         * subsequent typing inherits the formatting (mirroring the behavior
         * of apply_formatting_to_selection with no active selection).
         */
        let (line_start, line_end) = {
            let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                return;
            };
            let cursor = tab.cursor;

            // Find the start of the current line (after previous newline)
            let line_start = tab.document.content()[..cursor]
                .rfind('\n')
                .map(|pos| pos + 1)
                .unwrap_or(0);

            // Find the end of the current line (next newline or end of content)
            let line_end = tab.document.content()[cursor..]
                .find('\n')
                .map(|pos| cursor + pos)
                .unwrap_or(tab.document.content().len());

            (line_start, line_end)
        };

        let is_line_empty = line_start >= line_end;

        self.push_undo_snapshot();

        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        let effective_op =
            if is_uniformly_active(tab.document.paragraphs(), line_start, line_end, &op) {
                toggled_off(&op)
            } else {
                op.clone()
            };
        apply_formatting(
            tab.document.paragraphs_mut(),
            line_start,
            line_end,
            effective_op.clone(),
        );
        // Card styles (Pocket/Hat/Block/Tag) mark their line with
        // `para.heading` and center `para.alignment` (see
        // `apply_card_style`) — both paragraph-level fields `apply_formatting`
        // above never touches, since it only mutates run-level fields.
        // Left alone, a cleared line kept its heading's font-size/bold
        // override (text_editor.rs applies it at the paragraph-div level,
        // overriding a run's own now-cleared `size`/`bold`) and stayed
        // centered even after every run field was reset — the actual root
        // cause behind found_bugs.md's "Clear... fails to clear pocket,
        // hat, and block formatting".
        if let FormatOp::ClearAll { .. } = effective_op {
            reset_card_style_in_range(tab.document.paragraphs_mut(), line_start, line_end);
            // A prior card-style/formatting op on this same empty line (e.g.
            // apply_card_style's Bold+FontSize+Box sequence) may have armed
            // `pending_format`, which otherwise keeps force-applying to
            // every character typed from here on (insert_char's own doc
            // comment) — indefinitely, since nothing else was clearing it.
            // Clear Formatting has to reset this too, or newly typed text
            // (and, via paragraph-splitting on Enter, every line typed
            // after it) keeps resurrecting formatting that was supposedly
            // just cleared.
            tab.pending_format = None;
        }
        // apply_formatting no-ops on an empty [line_start, line_end) range
        // (nothing to split/iterate over), so it never touches the empty
        // line's own already-existing run(s). Apply directly here instead —
        // sync_insert_char reuses this exact run object when the user
        // starts typing, so seeding it now is what makes the very first
        // typed character(s) carry the formatting. Root cause: relying on
        // `pending_format` alone doesn't work for a multi-op call like
        // apply_card_style's Bold+FontSize+Box in sequence, since it's a
        // single slot — each call overwrote the previous one, so only the
        // last-applied op ever survived to the first keystroke.
        if is_line_empty {
            let (para_idx, _, _) = tab.document.resolve_position(line_start);
            if let Some(para) = tab.document.paragraphs_mut().get_mut(para_idx) {
                for run in para.runs.iter_mut() {
                    apply_format_op(&mut *run, &effective_op);
                }
            }
        }
        tab.document.is_modified = true;
        // Deliberately does NOT arm `pending_format` here (unlike
        // `apply_formatting_to_selection`'s no-selection case, which is a
        // genuine "stay bold until toggled off again" sticky mode the user
        // controls one keypress at a time). `apply_formatting_to_line` is
        // only ever called by `apply_card_style` (Bold+FontSize+Box/etc. in
        // sequence) and `ClearAll` — the empty-line run-seeding just above
        // already makes the very next keystroke correct, so arming
        // `pending_format` here was pure side effect, not a real need. It
        // used to leak indefinitely: since nothing but an unrelated
        // matching toggle or Clear Formatting ever cleared it, a single
        // Pocket line could resurrect its box on every line typed
        // afterward, including across a paragraph split on Enter (a real
        // reported bug — see this file's own history for the two
        // narrower fixes that preceded removing this block entirely).
    }

    pub fn apply_formatting_to_selection(&mut self, op: FormatOp) {
        /*
         * Spec 7.2's entry point for a ribbon button or formatting
         * shortcut. With an active selection, applies `op` to it directly
         * (pushing its own undo snapshot, paired content+paragraphs per
         * Phase 1) — unless the whole selection is already uniformly in
         * that state, in which case it toggles off instead (bug fix:
         * Word's toolbar buttons toggle off on re-click; re-applying
         * `Bold(true)` to already-bold text was previously a no-op).  With
         * no selection, applies formatting to the character under the cursor
         * and also arms `pending_format` for subsequent typing, so formatting
         * applies both retroactively and prospectively.
         */
        // "Select similar formatting" (Doc Menu) leaves its matches in
        // `similar_ranges` and blanks `selection`; when present they stand in
        // for the caret selection, so one button click restyles every matching
        // run in the document at once. Both empty = the no-selection path.
        let ranges: Option<Vec<(usize, usize)>> = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| {
                if !t.similar_ranges.is_empty() {
                    Some(t.similar_ranges.clone())
                } else {
                    t.selection.map(|(a, f)| vec![(a.min(f), a.max(f))])
                }
            });
        match ranges {
            Some(ranges) => {
                self.push_undo_snapshot();
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    // Toggle off only when *every* range is already in that
                    // state, so one already-bold match can't block bolding all
                    // the others. Identical to the old single-range rule when
                    // there is only one range.
                    let effective_op = if ranges
                        .iter()
                        .all(|&(s, e)| is_uniformly_active(tab.document.paragraphs(), s, e, &op))
                    {
                        toggled_off(&op)
                    } else {
                        op.clone()
                    };
                    let (small, normal) = (
                        self.preferences.small_size_half_points,
                        self.preferences.normal_text_size_half_points,
                    );
                    for &(start, end) in &ranges {
                        apply_formatting(
                            tab.document.paragraphs_mut(),
                            start,
                            end,
                            effective_op.clone(),
                        );
                        // Beta feedback: "when underlining a shrinked word
                        // with the F9 function, automatically reset word to
                        // the default font". Shrink marks text as *not read*
                        // and skips underlined runs for exactly that reason
                        // (`shrink_text`); underlining is the other half of
                        // that convention, so it puts a shrunk run back to
                        // body size. Gated on `effective_op` rather than `op`
                        // so toggling underline *off* doesn't also resize.
                        if matches!(effective_op, FormatOp::Underline(true)) {
                            Self::unshrink_range(
                                tab.document.paragraphs_mut(),
                                start,
                                end,
                                small,
                                normal,
                            );
                        }
                        // Mirrors apply_formatting_to_line's own ClearAll special
                        // case (see its comment above `reset_card_style_in_range`):
                        // apply_formatting above only ever mutates run-level
                        // fields, never a Pocket/Hat/Block paragraph's own
                        // `heading`/`alignment` — left alone, Clear Formatting
                        // through an active selection silently kept a card-styled
                        // paragraph boxed/centered while only stripping bold/size.
                        if let FormatOp::ClearAll { .. } = effective_op {
                            let (first, _, _) = tab.document.resolve_position(start);
                            let (last, _, _) = tab.document.resolve_position(end);
                            // A card's heading and alignment apply to its whole
                            // paragraph, so clearing any part of one clears its
                            // remaining run-level card formatting too.
                            let card_paragraphs: Vec<_> = (first..=last)
                                .filter(|&i| tab.document.paragraphs()[i].heading != 0)
                                .collect();
                            reset_card_style_in_range(tab.document.paragraphs_mut(), start, end);
                            for i in card_paragraphs {
                                for run in &mut tab.document.paragraphs_mut()[i].runs {
                                    apply_format_op(&mut *run, &effective_op);
                                }
                            }
                            tab.pending_format = None;
                        }
                    }
                    tab.document.is_modified = true;
                }
            }
            None => {
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return;
                };
                let cursor = tab.cursor;
                let content_len = tab.document.content().len();

                // Check if pending format matches current op to decide toggle behavior
                let should_toggle_off = tab.pending_format.as_ref() == Some(&op);

                // Apply to character under cursor if not at end of document
                if cursor < content_len {
                    let next_char_boundary = char_right(&tab.document.content(), cursor);
                    let effective_op = if should_toggle_off {
                        toggled_off(&op)
                    } else {
                        op.clone()
                    };
                    self.push_undo_snapshot();
                    let (small, normal) = (
                        self.preferences.small_size_half_points,
                        self.preferences.normal_text_size_half_points,
                    );
                    if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                        let unshrink = matches!(effective_op, FormatOp::Underline(true));
                        apply_formatting(
                            tab.document.paragraphs_mut(),
                            cursor,
                            next_char_boundary,
                            effective_op,
                        );
                        // Same rule as the selection path above, so underlining
                        // with no selection behaves the same way.
                        if unshrink {
                            Self::unshrink_range(
                                tab.document.paragraphs_mut(),
                                cursor,
                                next_char_boundary,
                                small,
                                normal,
                            );
                        }
                        tab.document.is_modified = true;
                    }
                }

                // Update pending format (same toggle logic as before)
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    if should_toggle_off {
                        tab.pending_format = None;
                    } else {
                        tab.pending_format = Some(op);
                    }
                }
            }
        }
    }

    pub fn clear_formatting(&mut self) {
        /*
         * Clears formatting across the active selection if one exists,
         * otherwise falls back to the current line (this codebase's
         * pre-existing behavior for Clear/card-style operations with no
         * selection). Root cause of the bug this method fixes: both call
         * sites (ClearFormattingAction's keybind and the ribbon's Clear
         * button) called `apply_formatting_to_line` unconditionally, which
         * ignores `tab.selection` and only ever clears the cursor's own
         * line — a multi-paragraph selection left every other paragraph
         * still formatted.
         */
        let default_size = self.preferences.normal_text_size_half_points;
        let has_selection = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false);
        if has_selection {
            self.apply_formatting_to_selection(FormatOp::ClearAll { default_size });
        } else {
            self.apply_formatting_to_line(FormatOp::ClearAll { default_size });
        }
    }

    /// Inserts clipboard text at the cursor, replacing any selection —
    /// `insert_str` with the paste command's own condensing rules applied
    /// first. Called from the ribbon's Paste button and its keybind.
    ///
    /// Condensing is now driven by the `paste_condense` setting rather than by
    /// reading `paragraph_integrity`/`pilcrows` directly. Those two ribbon
    /// toggles still control it, but through the setting (see
    /// `toggle_paragraph_integrity`/`toggle_pilcrows`), so the settings modal
    /// and the ribbon can't disagree about what a paste will do.
    pub fn paste_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let processed = if self.preferences.paste_condense {
            let replacement = if self.preferences.paste_condense_pilcrow {
                "¶"
            } else {
                " "
            };
            text.replace('\n', replacement)
        } else {
            text.to_string()
        };
        self.insert_str(&processed);
    }

    /// Card Menu → Standardize highlighting: repaints every highlighted run in
    /// the document to the current highlight color.
    pub fn standardize_highlighting(&mut self) {
        self.standardize_highlights(None);
    }

    /// Card Menu → Standardize highlighting with exception: the same, but
    /// leaves runs already in `standardize_highlight_exception` untouched.
    ///
    /// The use case is a document where one color carries meaning — an
    /// analytic marked in green, say — that shouldn't be flattened along with
    /// the ordinary highlighting. With no exception configured this is exactly
    /// the plain command.
    pub fn standardize_highlighting_with_exception(&mut self) {
        let exception = self.preferences.standardize_highlight_exception.clone();
        let exception = (!exception.is_empty()).then_some(exception);
        self.standardize_highlights(exception.as_deref());
    }

    /// Repaints every highlighted run to the current highlight color, skipping
    /// any already in `except`.
    ///
    /// Whole-file on purpose — the point is that a card assembled from several
    /// sources ends up consistent, so it acts regardless of selection. Runs
    /// that are not highlighted are untouched; this only changes *which*
    /// highlight, never adds or removes one.
    fn standardize_highlights(&mut self, except: Option<&str>) {
        let color = self.preferences.highlight_color.clone();
        let repaints = |run: &Run| {
            run.highlight
                && run.highlight_color != color
                && except.is_none_or(|e| run.highlight_color != e)
        };

        let nothing_to_do = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| {
                !t.document
                    .paragraphs()
                    .iter()
                    .flat_map(|para| &para.runs)
                    .any(repaints)
            })
            .unwrap_or(true);
        // Pushing an undo entry for a no-op would make Ctrl+Z appear broken.
        if nothing_to_do {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            for para in tab.document.paragraphs_mut() {
                for run in &mut para.runs {
                    if repaints(run) {
                        run.highlight_color = color.clone();
                    }
                }
                // Neighbours that differed only by highlight color are now
                // identical, so fuse them rather than leaving the document
                // fragmented on a distinction that no longer exists.
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.document.is_modified = true;
        }
    }

    /// Card Menu → "Condense, no pilcrows", and the ribbon's own Condense
    /// button: collapses the selection's newlines into spaces.
    ///
    /// Each collapsed newline actually becomes `CONDENSE_MARKER` — a real
    /// space (so condensed text still reads exactly like one) plus a
    /// trailing zero-width space, invisible but real: it's what lets
    /// `uncondense_selection` find exactly where a newline used to be
    /// without also matching an ordinary space the user typed.
    pub fn condense_selection(&mut self) {
        self.condense_selection_with(CONDENSE_MARKER);
    }

    /// Card Menu → "Condense, pilcrows": the same, but each collapsed newline
    /// leaves a `¶` behind so the original paragraph breaks stay visible.
    ///
    /// The same marker `paste_text` uses when condensing a paste, and no
    /// surrounding space — the pilcrow marks the exact point the break was.
    pub fn condense_with_pilcrows(&mut self) {
        self.condense_selection_with("¶");
    }

    /// Replaces every newline in the selection with `replacement`, preserving
    /// each character's own formatting (bold/highlight/size/etc.) rather than
    /// flattening the condensed text down to a single unformatted run —
    /// `runs_in_range` (already used by copy/paste's rich-clipboard path)
    /// captures the original per-character runs before the delete below
    /// discards them, and `sync_insert_str_with_runs` (the same rich-paste
    /// primitive) puts them back instead of `sync_insert_str`'s plain,
    /// inherit-whatever's-at-the-insertion-point behavior.
    ///
    /// Only works on an active selection; no-op without one, and no-op when
    /// the selection holds no newlines to collapse.
    fn condense_selection_with(&mut self, replacement: &str) {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return;
        }

        let selected_text = tab.document.content()[start..end].to_string();
        let condensed = selected_text.replace('\n', replacement);

        if condensed == selected_text {
            return;
        }

        // `runs_in_range` emits a dedicated unformatted `"\n"` run for every
        // paragraph boundary the selection crosses (same contract copy/paste
        // relies on) — replacing `\n` inside each run's own text turns those
        // into the replacement too, matching `condensed`, without touching any
        // real run's formatting.
        let condensed_runs: Vec<Run> = runs_in_range(tab.document.paragraphs(), start, end)
            .into_iter()
            .map(|mut r| {
                r.text = r.text.replace('\n', replacement);
                r
            })
            .collect();

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            sync_delete_range(tab.document.paragraphs_mut(), start, end);
            sync_insert_str_with_runs(
                tab.document.paragraphs_mut(),
                start,
                &condensed,
                &condensed_runs,
            );
            tab.cursor = start;
            tab.selection = Some((start, start + condensed.len()));
            tab.document.is_modified = true;
        }
    }

    /// Card Menu → "Uncondense": undoes condensing by turning each marker it
    /// left behind back into a real newline — `¶` if the selection was
    /// condensed with pilcrows, `CONDENSE_MARKER` if it was condensed
    /// without them. Whichever produced the selected text, this reverses it;
    /// a no-op if neither marker is present. Same run-preserving mechanics as
    /// `condense_selection_with`, just inverted.
    pub fn uncondense_selection(&mut self) {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return;
        }

        let selected_text = tab.document.content()[start..end].to_string();
        let uncondensed = uncondense_markers(&selected_text);

        if uncondensed == selected_text {
            return;
        }

        let uncondensed_runs: Vec<Run> = runs_in_range(tab.document.paragraphs(), start, end)
            .into_iter()
            .map(|mut r| {
                r.text = uncondense_markers(&r.text);
                r
            })
            .collect();

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            sync_delete_range(tab.document.paragraphs_mut(), start, end);
            sync_insert_str_with_runs(
                tab.document.paragraphs_mut(),
                start,
                &uncondensed,
                &uncondensed_runs,
            );
            tab.cursor = start;
            tab.selection = Some((start, start + uncondensed.len()));
            tab.document.is_modified = true;
        }
    }

    /// Applies `kind` to every paragraph the selection spans (or the
    /// cursor's own paragraph when there's no selection) — setting `list =
    /// Some(ListItem { kind, level: 0 })`. If every touched paragraph is
    /// already exactly that style at level 0, clears it instead (Word's own
    /// toggle-off convention, same as `apply_formatting_to_selection`'s
    /// `toggled_off` for other formatting).
    pub fn apply_list_style(&mut self, kind: ListKind) {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (start_para, end_para) = match tab.selection {
            Some((a, f)) => {
                let (start_para, ..) = tab.document.resolve_position(a.min(f));
                let (end_para, ..) = tab.document.resolve_position(a.max(f));
                (start_para, end_para)
            }
            None => {
                let (para_idx, ..) = tab.document.resolve_position(tab.cursor);
                (para_idx, para_idx)
            }
        };

        let already_this_style = (start_para..=end_para).all(|i| {
            tab.document.paragraphs().get(i).map(|p| p.list)
                == Some(Some(ListItem { kind, level: 0 }))
        });

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            for i in start_para..=end_para {
                if let Some(para) = tab.document.paragraphs_mut().get_mut(i) {
                    para.list = if already_this_style {
                        None
                    } else {
                        Some(ListItem { kind, level: 0 })
                    };
                }
            }
            tab.document.is_modified = true;
        }
    }

    /// Clears `list` on every paragraph the selection spans (or the
    /// cursor's own paragraph). Standalone entry point for the ribbon's
    /// "remove list" affordance, and what `apply_list_style`'s toggle-off
    /// branch does internally.
    pub fn remove_list_formatting(&mut self) {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (start_para, end_para) = match tab.selection {
            Some((a, f)) => {
                let (start_para, ..) = tab.document.resolve_position(a.min(f));
                let (end_para, ..) = tab.document.resolve_position(a.max(f));
                (start_para, end_para)
            }
            None => {
                let (para_idx, ..) = tab.document.resolve_position(tab.cursor);
                (para_idx, para_idx)
            }
        };

        let any = (start_para..=end_para).any(|i| {
            tab.document
                .paragraphs()
                .get(i)
                .map(|p| p.list.is_some())
                .unwrap_or(false)
        });
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            for i in start_para..=end_para {
                if let Some(para) = tab.document.paragraphs_mut().get_mut(i) {
                    para.list = None;
                }
            }
            tab.document.is_modified = true;
        }
    }

    /// Tab inside a list paragraph: increments its level, capped at 8
    /// (Word's own max `w:ilvl`). No-op when the cursor's paragraph isn't a
    /// list item — the call site (`text_editor.rs`) falls through to
    /// inserting a literal tab character in that case.
    pub fn indent_list_item(&mut self) {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (para_idx, ..) = tab.document.resolve_position(tab.cursor);
        let Some(item) = tab.document.paragraphs().get(para_idx).and_then(|p| p.list) else {
            return;
        };
        if item.level >= 8 {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if let Some(para) = tab.document.paragraphs_mut().get_mut(para_idx) {
                para.list = Some(ListItem {
                    level: item.level + 1,
                    ..item
                });
            }
            tab.document.is_modified = true;
        }
    }

    /// Shift+Tab inside a list paragraph: decrements its level, or clears
    /// `list` entirely when already at level 0 (matching Word: outdenting a
    /// top-level list item removes it from the list).
    pub fn outdent_list_item(&mut self) {
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let (para_idx, ..) = tab.document.resolve_position(tab.cursor);
        let Some(item) = tab.document.paragraphs().get(para_idx).and_then(|p| p.list) else {
            return;
        };

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            if let Some(para) = tab.document.paragraphs_mut().get_mut(para_idx) {
                para.list = if item.level == 0 {
                    None
                } else {
                    Some(ListItem {
                        level: item.level - 1,
                        ..item
                    })
                };
            }
            tab.document.is_modified = true;
        }
    }

    /// The font size (in half-points, `Run.size`'s unit) shared by every run
    /// the selection touches — or by the run under the cursor when nothing is
    /// selected — and `None` when the range mixes sizes. A `0` means the run
    /// carries no explicit override and therefore paints at the configured
    /// body size (`normal_text_size_half_points`).
    ///
    /// Byte offsets accumulate across paragraphs and count the separating
    /// newline, matching `document_ops::is_uniformly_active`. (The old
    /// `cycle_font_size` did neither, so on any multi-paragraph document it
    /// read the size off the wrong runs.)
    pub fn selection_font_size_half_points(&self) -> Option<u16> {
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (start, end) = match tab.selection {
            Some((a, f)) if a != f => (a.min(f), a.max(f)),
            // No selection: report what typing here would inherit — the
            // character *before* the caret, the way Word's size box does —
            // falling back to the one after it at the very start of the
            // document. Using the character after would blank the box every
            // time the caret sat at the end of a line, where nothing follows.
            //
            // The range needn't land on a char boundary: runs are matched by
            // byte *overlap*, so any byte inside the run identifies it.
            _ if tab.cursor > 0 => (tab.cursor - 1, tab.cursor),
            _ => (0, 1),
        };

        let mut cumulative = 0usize;
        let mut uniform: Option<u16> = None;
        for para in tab.document.paragraphs() {
            for run in &para.runs {
                let run_start = cumulative;
                let run_end = cumulative + run.text.len();
                cumulative = run_end;
                if run_start.max(start) >= run_end.min(end) {
                    continue;
                }
                match uniform {
                    None => uniform = Some(run.size),
                    Some(size) if size != run.size => return None,
                    _ => {}
                }
            }
            cumulative += 1; // the paragraph-separating '\n'
        }
        uniform
    }

    /// Applies an explicit font size (half-points) to the selection, or arms it
    /// as the pending format when nothing is selected.
    pub fn set_font_size_half_points(&mut self, half_points: u16) {
        self.apply_formatting_to_selection(FormatOp::FontSize(half_points));
    }

    pub fn cycle_text_color(&mut self) {
        /*
         * Cycles through preset text colors: yellow -> red -> blue -> yellow.
         * Detects current color uniformly applied to selection, then advances.
         * Applies to selection or sets pending format if no selection.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab);
        let selection = tab.and_then(|t| t.selection);

        let current_color = if let Some((a, f)) = selection {
            let (start, end) = (a.min(f), a.max(f));
            tab.and_then(|t| {
                // Check if all runs in range have same color
                let mut uniform_color: Option<String> = None;
                for para in t.document.paragraphs() {
                    let mut pos = 0;
                    for run in &para.runs {
                        let run_end = pos + run.text.len();
                        if run_end > start && pos < end {
                            if uniform_color.is_none() {
                                uniform_color = run.color.clone();
                            } else if uniform_color != run.color {
                                return None; // not uniform
                            }
                        }
                        pos = run_end;
                    }
                }
                uniform_color
            })
        } else {
            None
        };

        let next_color = match current_color.as_deref() {
            Some("ffff00") => "ff0000", // yellow -> red
            Some("ff0000") => "0000ff", // red -> blue
            Some("0000ff") => "ffff00", // blue -> yellow
            _ => "ffff00",              // default to yellow
        };

        self.apply_formatting_to_selection(FormatOp::Color(Some(next_color.to_string())));
    }

    /// Bounds and step for `AppState.zoom` — 50%-250% in 10% increments,
    /// matching common editors' (VS Code, Word) zoom granularity.
    pub const ZOOM_MIN: f32 = 0.5;
    pub const ZOOM_MAX: f32 = 2.5;
    pub const ZOOM_STEP: f32 = 0.1;

    pub fn zoom_in(&mut self) {
        self.zoom = (self.zoom + Self::ZOOM_STEP).min(Self::ZOOM_MAX);
    }

    pub fn zoom_out(&mut self) {
        self.zoom = (self.zoom - Self::ZOOM_STEP).max(Self::ZOOM_MIN);
    }

    pub fn zoom_reset(&mut self) {
        self.zoom = 1.0;
    }

    pub fn toggle_strikethrough(&mut self) {
        /*
         * Toggles strikethrough on selected text or sets pending format
         * for future typing if no selection. Data is stored but rendering
         * is deferred until GPUI supports text decoration.
         */
        self.apply_formatting_to_selection(FormatOp::Strikethrough(true));
    }

    /// Restores every run fully inside `[start, end)` that is sitting at
    /// Shrink's `small` size back to `normal`, leaving every other size alone.
    ///
    /// The inverse of `shrink_text`, and deliberately as narrow as it is:
    /// only runs at exactly the configured small size are touched, so
    /// underlining a Pocket or a Cite cannot yank it down to body size. Uses
    /// the same containment rule and byte-offset walk as `shrink_text`
    /// (`+ 1` per paragraph for the newline between them), so the two agree
    /// about which runs a range covers.
    fn unshrink_range(
        paragraphs: &mut [Paragraph],
        start: usize,
        end: usize,
        small: u16,
        normal: u16,
    ) {
        if small == normal {
            return;
        }
        let mut cumulative = 0usize;
        for para in paragraphs.iter_mut() {
            for run in &mut para.runs {
                let run_start = cumulative;
                let run_end = cumulative + run.text.len();
                if run_start >= start && run_end <= end && run.size == small {
                    run.size = normal;
                }
                cumulative = run_end;
            }
            cumulative += 1;
        }
    }

    pub fn shrink_text(&mut self) {
        /*
         * Sets the font size of every non-underlined run in the selection to
         * settings.conf's `small_size` (user-requested: underlined text is
         * left alone — e.g. a debate card's underlined emphasis shouldn't
         * shrink along with the rest of the tag/cite). Runs are only
         * touched when they fall fully inside the selection (no splitting
         * at partial overlaps), matching this method's pre-existing scan.
         */
        let small_size = self.preferences.small_size_half_points;
        let selection = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.selection);
        if let Some((a, f)) = selection {
            let (start, end) = (a.min(f), a.max(f));
            self.push_undo_snapshot();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                let mut cumulative = 0usize;
                for para in tab.document.paragraphs_mut() {
                    for run in &mut para.runs {
                        let run_start = cumulative;
                        let run_end = cumulative + run.text.len();
                        if run_start >= start && run_end <= end && !run.underline {
                            run.size = small_size;
                        }
                        cumulative = run_end;
                    }
                    cumulative += 1;
                }
                tab.document.is_modified = true;
            }
        }
    }

    pub fn apply_case_to_selection(&mut self, case_type: case_converter::CaseType) {
        /*
         * Changes case of selected text. No-op when no selection.
         */
        let selection = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.selection);
        if let Some((a, f)) = selection {
            let (start, end) = (a.min(f), a.max(f));
            if start >= end {
                return;
            }
            self.push_undo_snapshot();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                let (start_para, start_run, start_char) = tab.document.resolve_position(start);
                let (end_para, end_run, end_char) = tab.document.resolve_position(end);
                // Split at the boundaries first — same pattern as
                // `apply_formatting` — so a run that only partially
                // overlaps the selection doesn't get skipped entirely.
                // End before start so start's already-resolved indices
                // aren't shifted by a run being inserted ahead of it.
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
                for para in tab.document.paragraphs_mut() {
                    for run in &mut para.runs {
                        let run_start = cumulative;
                        let run_end = cumulative + run.text.len();
                        if run_start >= start && run_end <= end {
                            run.text = case_converter::apply_case(&run.text, case_type);
                        }
                        cumulative = run_end;
                    }
                    cumulative += 1;
                    crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
                }
                tab.document.is_modified = true;
                // Update content to match
            }
        }
    }

    /// Which paragraphs are hidden by the collapsed headings in `folded`.
    ///
    /// Level-aware, matching Word: collapsing a heading of level `L` hides
    /// everything after it until the next heading of level `L` or higher —
    /// body text *and* the lower-level headings nested under it. Collapsing a
    /// Pocket therefore takes its Hats, Blocks and Tags with it, not just its
    /// prose. Lower number = higher in the hierarchy (Pocket 1 .. Tag 4).
    ///
    /// One pass, no allocation beyond the result.
    pub fn folded_paragraphs(
        paragraphs: &[Paragraph],
        folded: &std::collections::HashSet<usize>,
    ) -> Vec<bool> {
        let mut hidden = vec![false; paragraphs.len()];
        if folded.is_empty() {
            return hidden;
        }
        // `Some(level)` while inside a collapsed section: hide until a heading
        // at that level or higher closes it.
        let mut hide_until: Option<u8> = None;

        for (i, para) in paragraphs.iter().enumerate() {
            let heading = para.heading;
            if let Some(level) = hide_until {
                // Body text never closes a section, only a heading does.
                if heading != 0 && heading <= level {
                    hide_until = None;
                } else {
                    hidden[i] = true;
                    continue;
                }
            }
            // A visible collapsed heading opens a new section. Checked after
            // the close above so one heading can end a section and start
            // another in the same step.
            if heading != 0 && folded.contains(&i) {
                hide_until = Some(heading);
            }
        }
        hidden
    }

    /// Drops fold state that can no longer be trusted — see
    /// `Tab.folded_headings`. Called before anything reads or writes it.
    fn sync_fold_state(&mut self) {
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if tab.folded_para_count != tab.document.paragraphs().len() {
            tab.folded_headings.clear();
            tab.folded_para_count = tab.document.paragraphs().len();
            tab.fold_version = tab.fold_version.wrapping_add(1);
        }
    }

    /// Collapses or expands the heading paragraph at `idx`. A no-op on body
    /// text — there is nothing under it to fold.
    pub fn toggle_paragraph_fold(&mut self, idx: usize) {
        self.sync_fold_state();
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        if tab
            .document
            .paragraphs()
            .get(idx)
            .map(|p| p.heading)
            .unwrap_or(0)
            == 0
        {
            return;
        }
        if !tab.folded_headings.remove(&idx) {
            tab.folded_headings.insert(idx);
        }
        tab.fold_version = tab.fold_version.wrapping_add(1);
    }

    /// True when anything is currently collapsed — drives the Fold button's
    /// engaged state and decides which way it toggles.
    pub fn any_folded(&self) -> bool {
        self.workspace
            .tabs
            .get(self.workspace.active_tab)
            .is_some_and(|t| {
                t.folded_para_count == t.document.paragraphs().len()
                    && !t.folded_headings.is_empty()
            })
    }

    /// The Fold button: collapse every heading, or expand everything if
    /// anything is already collapsed.
    ///
    /// Collapse-all-then-expand-what-you-need is the intended flow, so the
    /// button leads with collapsing and only expands once there is something
    /// to expand.
    pub fn toggle_fold(&mut self) {
        self.sync_fold_state();
        let expand = self.any_folded();
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        tab.folded_headings.clear();
        if !expand {
            for (i, para) in tab.document.paragraphs().iter().enumerate() {
                if para.heading != 0 {
                    tab.folded_headings.insert(i);
                }
            }
        }
        tab.folded_para_count = tab.document.paragraphs().len();
        tab.fold_version = tab.fold_version.wrapping_add(1);
    }

    /// Paragraph integrity: keep a paste's paragraph breaks intact.
    ///
    /// Turning it on switches condense-on-paste off — the two are opposites,
    /// and leaving both on meant the ribbon claimed to be preserving
    /// paragraphs while the paste collapsed them anyway.
    pub fn wikify_current_tab(&mut self) -> std::io::Result<()> {
        /*
         * Exports current tab to markdown file with heading hierarchy.
         * File is saved as document_name.md in same directory.
         */
        let tab = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "No active tab"))?;

        let markdown =
            wikifi_export::export_to_markdown(tab.document.paragraphs(), &tab.document.content());

        if let Some(path) = &tab.file_path {
            wikifi_export::save_markdown_file(path, &markdown)?;
        } else {
            return Err(std::io::Error::other("Tab must be saved first"));
        }
        Ok(())
    }

    pub fn apply_center_alignment(&mut self) {
        /*
         * Applies center alignment to all paragraphs overlapping the active
         * selection (or the paragraph containing the cursor, if no selection).
         * Phase 4.2: Center-align card styles (Pocket, Hat, Block).
         */
        let selection = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.selection);
        self.apply_center_alignment_with_selection(selection);
    }

    pub fn apply_center_alignment_with_selection(&mut self, selection: Option<(usize, usize)>) {
        /*
         * Applies center alignment using an explicitly passed selection instead
         * of reading from the current state. Used by button handlers that need
         * to preserve the selection from before other formatting operations.
         */
        let (start, end) = match selection {
            Some((a, f)) => (a.min(f), a.max(f)),
            None => {
                let cursor = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .map(|t| t.cursor)
                    .unwrap_or(0);
                (cursor, cursor)
            }
        };

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            apply_paragraph_alignment(tab.document.paragraphs_mut(), start, end, Alignment::Center);
            tab.document.is_modified = true;
        }
    }

    pub fn apply_line_alignment(&mut self, alignment: Alignment) {
        /*
         * Sets the alignment of the line containing the cursor (ribbon's
         * Align Left/Align Center buttons) — line-scoped like
         * `apply_formatting_to_line`/`apply_card_style`, not selection-
         * spanning like `apply_center_alignment`. Not a toggle: `Alignment`
         * is a single-valued paragraph field, so setting one value
         * inherently supersedes whatever was there before.
         */
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return;
        };
        let cursor = tab.cursor;
        let line_start = tab.document.content()[..cursor]
            .rfind('\n')
            .map(|pos| pos + 1)
            .unwrap_or(0);
        let line_end = tab.document.content()[cursor..]
            .find('\n')
            .map(|pos| cursor + pos)
            .unwrap_or(tab.document.content().len());

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            apply_paragraph_alignment(
                tab.document.paragraphs_mut(),
                line_start,
                line_end,
                alignment,
            );
            tab.document.is_modified = true;
        }
    }

    /// Applies one of the line-based card styles (Pocket/Hat/Block/Tag) to
    /// the entire line containing the cursor: bold + the style's font size,
    /// its special formatting (box/double-underline/underline), and center
    /// alignment. Extracted from `formatting_ribbon.rs`'s ribbon-button
    /// handler so both the ribbon and a configurable keybind
    /// (`src/keybinds.rs`) can trigger identical behavior without
    /// duplicating this logic.
    ///
    /// Cite and Emphasis are deliberately not `CardStyleKind` variants —
    /// both apply to the current *selection*, not the whole line (Cite per
    /// an earlier explicit fix; Emphasis was never line-based), so they
    /// keep going through `apply_formatting_to_selection` at each call site.
    pub fn apply_card_style(&mut self, kind: CardStyleKind) {
        // All four read their configured size from settings.conf;
        // `CardStyleKind::font_size` remains the default those settings fall
        // back to when the key is absent, not a second live value.
        let size = self.card_size_half_points(kind);

        self.apply_formatting_to_line(FormatOp::Bold(true));
        self.apply_formatting_to_line(FormatOp::FontSize(size));
        self.apply_formatting_to_line(FormatOp::Style(Some(kind.card_style())));
        match kind {
            CardStyleKind::Pocket => self.apply_formatting_to_line(FormatOp::Box(true)),
            CardStyleKind::Hat => self.apply_formatting_to_line(FormatOp::DoubleUnderline(true)),
            CardStyleKind::Block => self.apply_formatting_to_line(FormatOp::Underline(true)),
            CardStyleKind::Tag => {}
        }

        // Marks this line as a heading (Nav menu, Wikifi export, and
        // heading-level font sizing all read this field) — `content` and
        // `paragraphs` are always kept 1:1, one paragraph per line, so the
        // number of newlines before the cursor is that paragraph's index.
        //
        // Also clears any list marker the line was carrying: a card style
        // and a list item are mutually exclusive in this codebase already
        // (`split_paragraph_at` resets a heading line's list continuation on
        // Enter for the same reason) — but applying a style directly to an
        // *existing* list item left `.list` untouched, so the paragraph
        // could carry both. `rebuild_document_xml` writes one `<w:pStyle>`
        // per source (heading, then list), so that combination round-tripped
        // as two `<w:pStyle>` children in one `<w:pPr>` — invalid per
        // CT_PPrBase, and Word dropped the card style's own formatting on
        // reopen (bug report: "headings... worked in Word, but no longer").
        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
            let line_idx = tab.document.content()[..tab.cursor].matches('\n').count();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                if let Some(para) = tab.document.paragraphs_mut().get_mut(line_idx) {
                    para.heading = kind.heading_level();
                    para.list = None;
                }
            }
        }

        if kind.is_centered() {
            let tab = self.workspace.tabs.get_mut(self.workspace.active_tab);
            if let Some(t) = tab {
                let cursor = t.cursor;
                let line_start = t.document.content()[..cursor]
                    .rfind('\n')
                    .map(|pos| pos + 1)
                    .unwrap_or(0);
                let line_end = t.document.content()[cursor..]
                    .find('\n')
                    .map(|pos| cursor + pos)
                    .unwrap_or(t.document.content().len());
                self.apply_center_alignment_with_selection(Some((line_start, line_end)));
            }
        }
    }

    /// Applies the Cite style — bold + `cite_size_half_points` — to the
    /// current selection. Cite isn't a `CardStyleKind` (see the note on
    /// `apply_card_style`: it targets the selection, not the whole line),
    /// but shares the same reasoning for living here: the ribbon's Cite
    /// button and the `f8` keybind (`main_window.rs`) both call this so
    /// they can't drift apart.
    /// The Analytic style: Tag's weight and size, in the configured analytic
    /// color, but deliberately *not* a heading.
    ///
    /// Analytics are the debater's own argument rather than a structural
    /// marker, so they must stay out of the Nav outline, the fold hierarchy,
    /// and the Wikifi export's heading levels — all three of which key off
    /// `Paragraph.heading`. Applying this to a line that *was* a card style
    /// clears that marker rather than leaving a heading that no longer looks
    /// like one.
    pub fn apply_analytic_style(&mut self) {
        let size = self.preferences.tag_size_half_points;
        let color = self.preferences.analytic_color.clone();
        self.apply_formatting_to_line(FormatOp::Bold(true));
        self.apply_formatting_to_line(FormatOp::FontSize(size));
        self.apply_formatting_to_line(FormatOp::Color(Some(color)));
        self.apply_formatting_to_line(FormatOp::Style(Some(CardStyle::Analytic)));

        if let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
            let line_idx = tab.document.content()[..tab.cursor].matches('\n').count();
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                if let Some(para) = tab.document.paragraphs_mut().get_mut(line_idx) {
                    para.heading = 0;
                }
            }
        }
    }

    /// A predicate matching paragraphs formatted as analytics.
    ///
    /// Shared by every command that acts on analytics, so they cannot disagree
    /// about what one is. Analytics carry no marker of their own — exactly like
    /// cites — so they are recognised by what `apply_analytic_style` leaves
    /// behind: a non-heading paragraph whose runs are bold at the Tag size in
    /// the configured analytic color. A paragraph hand-formatted to match will
    /// be treated as one.
    ///
    /// Returns a closure so the borrow of `self` ends before callers mutate
    /// `tabs`.
    fn analytic_paragraph_test(&self) -> impl Fn(&Paragraph) -> bool {
        let size = self.preferences.tag_size_half_points;
        let color = self.preferences.analytic_color.clone();
        move |para: &Paragraph| {
            // A blank line is never an analytic, however its runs are styled.
            let has_text = !para.runs.iter().all(|r| r.text.trim().is_empty());
            if !has_text {
                return false;
            }
            let substantive = || para.runs.iter().filter(|r| !r.text.trim().is_empty());

            // The marker is authoritative: it says what the run *is*, so a
            // reformatted analytic is still one and a coincidentally-matching
            // line is not.
            if substantive().any(|r| r.style.is_some()) {
                return substantive().all(|r| r.style == Some(CardStyle::Analytic));
            }

            // Documents written before markers existed, or by another editor,
            // carry no marker at all — fall back to the formatting signature
            // `apply_analytic_style` produces.
            para.heading == 0
                && substantive()
                    .all(|r| r.bold && r.size == size && r.color.as_deref() == Some(color.as_str()))
        }
    }

    /// Doc Menu → Delete analytics: removes every analytic paragraph from the
    /// document, line and all.
    ///
    /// Whole lines rather than just their text: an analytic *is* its line, and
    /// blanking them would leave a run of empty paragraphs where the argument
    /// used to be. `content` is rebuilt from the surviving paragraphs to keep
    /// the 1:1 line/paragraph invariant the rest of the editor depends on.
    pub fn delete_analytics(&mut self) {
        let is_analytic = self.analytic_paragraph_test();
        let any = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.document.paragraphs().iter().any(&is_analytic))
            .unwrap_or(false);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.document
                .paragraphs_mut()
                .retain(|para| !is_analytic(para));
            // Every rich-text-aware function assumes at least one paragraph and
            // one run always exist (`default_paragraphs`).
            if tab.document.paragraphs().is_empty() {
                *tab.document.paragraphs_mut() = default_paragraphs();
            }
            // The cursor and any selection pointed into text that is gone.
            tab.cursor = clamp_to_char_boundary(
                &tab.document.content(),
                tab.cursor.min(tab.document.content().len()),
            );
            tab.selection = None;
            tab.document.is_modified = true;
        }
    }

    /// Doc Menu → Convert analytics to tags: promotes every Analytic-formatted
    /// paragraph in the document to a Tag.
    ///
    /// An analytic is recognised by what `apply_analytic_style` leaves behind —
    /// a non-heading paragraph whose runs are bold at the Tag size in the
    /// configured analytic color. That is the only signal available: analytics
    /// carry no marker of their own, exactly like cites. A paragraph the user
    /// hand-formatted to match will convert too.
    ///
    /// Converting drops the analytic color (a Tag is plain-colored) and sets
    /// the heading marker, which is what puts the line into the Nav outline and
    /// the fold hierarchy.
    pub fn convert_analytics_to_tags(&mut self) {
        let is_analytic = self.analytic_paragraph_test();

        let any = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.document.paragraphs().iter().any(&is_analytic))
            .unwrap_or(false);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        let tag_heading = CardStyleKind::Tag.heading_level();
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            for para in tab.document.paragraphs_mut() {
                if !is_analytic(para) {
                    continue;
                }
                for run in &mut para.runs {
                    run.color = None;
                    // The marker has to follow the heading. Leaving it at
                    // `Analytic` produced a line that was a Tag structurally
                    // (heading 4, so the Nav outline and the fold hierarchy
                    // treat it as one) but still answered "Analytic" to
                    // `tag_paragraph_test`, which prefers the marker over the
                    // heading — so Doc Menu -> Delete tags skipped every line
                    // this command had just converted.
                    run.style = Some(CardStyle::Tag);
                }
                para.heading = tag_heading;
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.document.is_modified = true;
        }
    }

    /// Applies whichever combination of bold/underline/box the Emphasis
    /// settings currently specify (plus the configured size, when
    /// `emphasis_change_size` is on), and marks every touched run with the
    /// `Emphasis`/`EmphasisBox` markers so `remove_emphasis` can find it
    /// again later.
    ///
    /// Deliberately does not go through `apply_formatting_to_selection`:
    /// that entry point applies `toggled_off(&op)` instead of `op` when the
    /// whole range is already in that state (Word's re-click-to-toggle-off
    /// convention). Reusing it here would mean clicking Emphasis on text
    /// that's already bold — inside a Tag, say — un-bolds it instead of
    /// emphasizing it. Calling `apply_formatting` directly per attribute
    /// sidesteps that, and collapses what would otherwise be up to 6 undo
    /// entries per click (one per `apply_formatting_to_selection` call) into
    /// one `push_undo_snapshot` for the whole operation.
    ///
    /// Also deliberately does not arm `pending_format` for the no-selection
    /// case — same reasoning `apply_formatting_to_line` already documents
    /// for its own multi-op card-style sequences: `pending_format` is a
    /// single slot, so the last of several `apply_formatting_to_selection`
    /// calls would silently win and the rest would be lost to the next
    /// keystroke, and a previous version of that exact bug leaked
    /// indefinitely across lines.
    pub fn apply_emphasis_style(&mut self) {
        let ranges: Option<Vec<(usize, usize)>> = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| {
                if !t.similar_ranges.is_empty() {
                    Some(t.similar_ranges.clone())
                } else {
                    t.selection.map(|(a, f)| vec![(a.min(f), a.max(f))])
                }
            });

        let bold = self.preferences.emphasis_bold;
        let underline = self.preferences.emphasis_underline;
        let boxed = self.preferences.emphasis_box;
        let size = self
            .preferences
            .emphasis_change_size
            .then_some(self.preferences.emphasis_size_half_points);

        let apply_all = |paragraphs: &mut Vec<Paragraph>, start: usize, end: usize| {
            if bold {
                apply_formatting(paragraphs, start, end, FormatOp::Bold(true));
            }
            if underline {
                apply_formatting(paragraphs, start, end, FormatOp::Underline(true));
            }
            // Not `FormatOp::Box` — that's Pocket's paragraph-wide box
            // (`has_box`/`slot_count_for_paragraph` in text_editor.rs both
            // read `run.box_format`, not `emphasis_boxed`), which would wrap
            // the whole paragraph and reserve extra row height for what's
            // meant to be a small inline span.
            if let Some(size) = size {
                apply_formatting(paragraphs, start, end, FormatOp::FontSize(size));
            }
            apply_formatting(paragraphs, start, end, FormatOp::Emphasis(true));
            if boxed {
                apply_formatting(paragraphs, start, end, FormatOp::EmphasisBox(true));
            }
        };

        match ranges {
            Some(ranges) => {
                self.push_undo_snapshot();
                if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                    for &(start, end) in &ranges {
                        apply_all(tab.document.paragraphs_mut(), start, end);
                    }
                    tab.document.is_modified = true;
                }
            }
            None => {
                let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
                    return;
                };
                let cursor = tab.cursor;
                let content_len = tab.document.content().len();
                if cursor < content_len {
                    let next_char_boundary = char_right(&tab.document.content(), cursor);
                    self.push_undo_snapshot();
                    if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                        apply_all(tab.document.paragraphs_mut(), cursor, next_char_boundary);
                        tab.document.is_modified = true;
                    }
                }
            }
        }
    }

    pub fn apply_cite_style(&mut self) {
        self.apply_formatting_to_selection(FormatOp::Bold(true));
        let size = self.preferences.cite_size_half_points;
        self.apply_formatting_to_selection(FormatOp::FontSize(size));
        self.apply_formatting_to_selection(FormatOp::Style(Some(CardStyle::Cite)));
    }

    pub fn undo(&mut self) {
        /*
         * Restores the most recent undo snapshot's `(content, paragraphs)`
         * pair as the active tab's, pushing the pair being replaced onto
         * the redo stack so `redo()` can restore it. No-op when there's
         * nothing to undo.
         *
         * The cursor isn't part of the snapshot, so it isn't restored to
         * its exact pre-edit position — it's clamped into the restored
         * content's bounds and onto its nearest valid char boundary
         * instead, since the old byte offset may no longer even be one.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        let Some(previous) = tab.document.undo_stack.pop() else {
            return;
        };
        tab.document.content_version += 1;

        let current_paragraphs = std::mem::replace(tab.document.paragraphs_mut(), previous);
        tab.document.redo_stack.push(current_paragraphs);
        // Same size-aware cap as `push_undo_snapshot` — repeatedly undoing
        // a huge document without any new edit would otherwise let
        // `redo_stack` grow past what `undo_stack` was ever bounded to.
        let cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(
            &tab.document.content(),
            tab.document.paragraphs(),
        ));
        while tab.document.redo_stack.len() > cap {
            tab.document.redo_stack.remove(0);
        }
        tab.selection = None;
        tab.cursor = clamp_to_char_boundary(&tab.document.content(), tab.cursor);
        tab.document.is_modified = true;
        // Break the coalescing window so the next edit doesn't merge into
        // whatever was on top of the undo stack before this undo.
        tab.document.last_edit_at = None;
    }

    pub fn redo(&mut self) {
        /*
         * The undo counterpart: restores the most recently undone
         * `(content, paragraphs)` pair from the redo stack, pushing the
         * pair being replaced back onto the undo stack. No-op when
         * there's nothing to redo. Cursor handling mirrors `undo()`.
         */
        let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) else {
            return;
        };
        let Some(next) = tab.document.redo_stack.pop() else {
            return;
        };
        tab.document.content_version += 1;

        let current_paragraphs = std::mem::replace(tab.document.paragraphs_mut(), next);
        tab.document.undo_stack.push(current_paragraphs);
        let cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(
            &tab.document.content(),
            tab.document.paragraphs(),
        ));
        while tab.document.undo_stack.len() > cap {
            tab.document.undo_stack.remove(0);
        }
        tab.selection = None;
        tab.cursor = clamp_to_char_boundary(&tab.document.content(), tab.cursor);
        tab.document.is_modified = true;
        tab.document.last_edit_at = None;
    }

    pub fn copy_selection(&self) -> Option<String> {
        /*
         * Returns the selected text as an owned String, or None when there is no
         * active selection. Does not modify state; safe to call via entity.read(cx).
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        Some(tab.document.content()[start..end].to_string())
    }

    /// Sibling of `copy_selection` that also returns the selection's
    /// per-run formatting, for `rich_clipboard::encode_with_lengths` to ride
    /// alongside the plain text on copy/cut. `None` under the same
    /// conditions `copy_selection` returns `None`.
    pub fn copy_selection_runs(&self) -> Option<Vec<Run>> {
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        Some(crate::document_ops::runs_in_range(
            tab.document.paragraphs(),
            start,
            end,
        ))
    }

    /// The `(heading, alignment)` of every paragraph the selection touches, in
    /// document order — the paragraph-level half of a copy.
    ///
    /// Separate from `copy_selection_runs` because runs cannot express a card
    /// style on their own: Pocket/Hat/Block/Tag are run-level bold/size/box
    /// *plus* these two paragraph fields (`apply_card_style`). Copying only the
    /// runs is what made a pasted card come back correctly sized but
    /// structurally plain.
    ///
    /// Shares `document_ops::paragraph_attrs_in_range` with vim's yank
    /// registers, so the two can't drift on what a range's paragraphs are.
    pub fn copy_selection_paragraph_attrs(
        &self,
    ) -> Option<Vec<crate::rich_clipboard::ParagraphAttrs>> {
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        Some(crate::document_ops::paragraph_attrs_in_range(
            tab.document.paragraphs(),
            start,
            end,
        ))
    }

    pub fn cut_selection(&mut self) -> Option<String> {
        /*
         * Extracts the selected text, deletes it, and returns the text so the
         * caller can write it to the clipboard. Returns None when there is no
         * selection. Delegates deletion to delete_selection so cursor/is_modified
         * logic stays in one place.
         */
        let tab = self.workspace.tabs.get(self.workspace.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        let text = tab.document.content()[start..end].to_string();
        self.delete_selection();
        Some(text)
    }

    pub fn insert_str(&mut self, text: &str) {
        /*
         * Inserts a string at the current cursor position, replacing any active
         * selection first. Advances the cursor past the inserted text.
         * Mirrors insert_char but handles the multi-char payloads that clipboard
         * paste produces. An empty string is a true no-op (returns before
         * pushing an undo snapshot) — otherwise pasting empty clipboard
         * content would create an undo step that changes nothing.
         *
         * Typing-shaped, deliberately: a `'\n'` in `text` splits the
         * paragraph exactly as pressing Enter does, new line reverting to
         * body style and all. That is what `.`-repeating an insert session
         * (`VimChange::Insertion`) needs, and what a same-line replacement
         * (find/replace, a spelling fix) is indifferent to. A *paste* wants
         * the opposite — the line it lands in keeps its own card style — so
         * paste goes through `insert_str_with_runs` even when it has no
         * formatting to restore.
         */
        if text.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.delete_selection_raw();
        }
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = clamp_to_char_boundary(&tab.document.content(), tab.cursor);
            sync_insert_str(tab.document.paragraphs_mut(), tab.cursor, text);
            tab.cursor += text.len(); // text is valid UTF-8 so len() == byte count
            tab.document.is_modified = true;
        }
        if let Some(rec) = self.global_vim.vim_insertion_recording.as_mut() {
            rec.push_str(text);
        }
    }

    /// Like `insert_str`, but also stitches `runs` (already boundary-aligned
    /// to `text`, per `rich_clipboard::decode`'s own guarantee) into
    /// `tab.document.paragraphs()` at the insertion point instead of leaving the
    /// inserted text as one unstyled run — restores formatting on an in-app
    /// paste. `document_ops::sync_insert_str_with_runs` falls back to plain,
    /// inheriting behavior itself when `runs` is empty, so this mirrors
    /// `insert_str` exactly otherwise.
    pub fn insert_str_with_runs(&mut self, text: &str, runs: &[Run]) {
        self.insert_str_with_runs_and_paragraphs(text, runs, &[]);
    }

    /// `insert_str_with_runs` that also restores the copied paragraphs'
    /// `heading`/`alignment`.
    ///
    /// The paragraph pass is needed because the insertion itself goes through
    /// `split_paragraph_at`, which is written for pressing Enter: it
    /// deliberately gives the new paragraph `heading: 0` and default
    /// alignment, matching how Word reverts to body style after Enter inside a
    /// heading. Correct for typing, wrong for paste — it silently flattened
    /// every card style in a multi-line paste. Rather than teach that
    /// primitive about paste (it is shared with the typing path), the copied
    /// attributes are re-applied over the affected paragraphs afterwards.
    ///
    /// An empty `paragraph_attrs` leaves paragraphs exactly as the split left
    /// them — the plain-paste path, and clipboard metadata from a build that
    /// predates paragraph attributes.
    pub fn insert_str_with_runs_and_paragraphs(
        &mut self,
        text: &str,
        runs: &[Run],
        paragraph_attrs: &[crate::rich_clipboard::ParagraphAttrs],
    ) {
        if text.is_empty() {
            return;
        }
        self.push_undo_snapshot();
        if self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .map(|t| t.selection.is_some())
            .unwrap_or(false)
        {
            self.delete_selection_raw();
        }
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.cursor = clamp_to_char_boundary(&tab.document.content(), tab.cursor);
            // Which paragraph the paste starts in, and what that paragraph's
            // own attributes are — both resolved *before* the insert, which
            // moves every offset and clears the attributes off the tail it
            // splits away.
            let first_para = tab.document.resolve_position(tab.cursor).0;
            let dest_attrs = tab
                .document
                .paragraphs()
                .get(first_para)
                .map(|p| (p.heading, p.alignment));
            crate::document_ops::sync_insert_str_with_runs(
                tab.document.paragraphs_mut(),
                tab.cursor,
                text,
                runs,
            );
            tab.cursor += text.len();
            tab.document.is_modified = true;

            crate::document_ops::apply_pasted_paragraph_attrs(
                tab.document.paragraphs_mut(),
                first_para,
                text.matches('\n').count() + 1,
                paragraph_attrs,
                dest_attrs,
            );
        }
        if let Some(rec) = self.global_vim.vim_insertion_recording.as_mut() {
            rec.push_str(text);
        }
    }
}

impl AppState {
    pub fn execute(
        &mut self,
        command: crate::app::command::AppCommand,
    ) -> Vec<crate::app::command::AppEffect> {
        use crate::app::command::AppCommand;

        match command {
            AppCommand::ApplyFormatting(op) => {
                self.apply_formatting_to_selection(op);
                vec![]
            }
            AppCommand::ApplyCardStyle(kind) => {
                self.apply_card_style(kind);
                vec![]
            }
            AppCommand::ApplyCiteStyle => {
                self.apply_cite_style();
                vec![]
            }
            AppCommand::ApplyAnalyticStyle => {
                self.apply_analytic_style();
                vec![]
            }
            AppCommand::ApplyEmphasisStyle => {
                self.apply_emphasis_style();
                vec![]
            }
            AppCommand::ClearFormatting => {
                self.clear_formatting();
                vec![]
            }
            AppCommand::ToggleStrikethrough => {
                self.toggle_strikethrough();
                vec![]
            }
            AppCommand::ApplyCaseToSelection(case_type) => {
                self.apply_case_to_selection(case_type);
                vec![]
            }
            AppCommand::ApplyLineAlignment(alignment) => {
                self.apply_line_alignment(alignment);
                vec![]
            }
            AppCommand::ToggleFold => {
                self.toggle_fold();
                vec![]
            }
            AppCommand::ToggleInvisibilityMode => {
                self.toggle_invisibility_mode();
                vec![]
            }
            AppCommand::ToggleSidebarMode => {
                self.toggle_sidebar_mode();
                vec![]
            }
            AppCommand::ToggleSidebar => {
                self.ui.sidebar_visible = !self.ui.sidebar_visible;
                vec![]
            }
            AppCommand::SwitchTab(id) => {
                if let Some(idx) = self.tab_index(id) {
                    self.set_active_tab(idx);
                }
                vec![]
            }
            AppCommand::Undo => {
                self.undo();
                vec![]
            }
            AppCommand::Redo => {
                self.redo();
                vec![]
            }
            AppCommand::ClearToast => {
                self.ui.notifications.clear();
                vec![]
            }
            AppCommand::Backspace => {
                self.backspace();
                vec![]
            }
            AppCommand::DeleteForward => {
                self.delete_forward();
                vec![]
            }
            AppCommand::InsertChar(ch) => {
                self.insert_char(ch);
                vec![]
            }
            AppCommand::IndentListItem => {
                let in_list = self
                    .workspace
                    .tabs
                    .get(self.workspace.active_tab)
                    .is_some_and(|t| {
                        let (para_idx, ..) = crate::document_ops::resolve_position(
                            t.document.paragraphs(),
                            t.cursor,
                        );
                        t.document
                            .paragraphs()
                            .get(para_idx)
                            .is_some_and(|p| p.list.is_some())
                    });
                if in_list {
                    self.indent_list_item();
                } else {
                    self.insert_char('\t');
                }
                vec![]
            }
            AppCommand::OutdentListItem => {
                self.outdent_list_item();
                vec![]
            }
            AppCommand::MoveLeft => {
                self.move_left();
                vec![]
            }
            AppCommand::MoveRight => {
                self.move_right();
                vec![]
            }
            AppCommand::ExtendLeft => {
                self.extend_left();
                vec![]
            }
            AppCommand::ExtendRight => {
                self.extend_right();
                vec![]
            }
            AppCommand::Save => self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|tab| vec![crate::app::command::AppEffect::PerformSave(tab.id)])
                .unwrap_or_default(),
            AppCommand::SaveAs => self
                .workspace
                .tabs
                .get(self.workspace.active_tab)
                .map(|tab| vec![crate::app::command::AppEffect::PromptSaveAs(tab.id)])
                .unwrap_or_default(),
            AppCommand::SaveTab(id) => vec![crate::app::command::AppEffect::PerformSave(id)],
            AppCommand::SaveTabAs(id) => vec![crate::app::command::AppEffect::PromptSaveAs(id)],
            AppCommand::OpenFile => vec![crate::app::command::AppEffect::PromptOpenFile],
            AppCommand::OpenFolder => vec![crate::app::command::AppEffect::PromptOpenFolder],
            AppCommand::OpenFileAt(path) => {
                vec![crate::app::command::AppEffect::LoadDocument(path)]
            }
            AppCommand::OpenFileInCurrentTab(path) => {
                vec![crate::app::command::AppEffect::LoadDocumentInCurrentTab(
                    path,
                )]
            }
            AppCommand::OpenFileInSidePane(path) => {
                if !self.workspace.split_view {
                    self.workspace.split_view = true;
                    self.workspace
                        .tabs
                        .push(crate::state::Tab::new_empty(crate::document::TabId(
                            self.workspace.next_tab_id,
                        )));
                    self.workspace.secondary_tab_id =
                        Some(crate::document::TabId(self.workspace.next_tab_id));
                    self.workspace.next_tab_id += 1;
                }
                self.focus_pane(crate::state::Pane::Secondary);
                vec![crate::app::command::AppEffect::LoadDocument(path)]
            }
            AppCommand::RefreshFileTree => {
                vec![crate::app::command::AppEffect::ScanWorkspace(
                    self.workspace.working_directory.clone(),
                )]
            }
            AppCommand::ReopenClosedTab => self
                .workspace
                .closed_tabs
                .pop()
                .map(crate::app::command::AppEffect::LoadDocument)
                .into_iter()
                .collect(),
            command @ AppCommand::VimKey { .. } => self.execute_vim_command(command).1,
        }
    }

    /// Runs a translated Vim command through the application boundary. The
    /// boolean retains the editor-view fallthrough contract for visual-row
    /// navigation; all platform work is returned as effects.
    pub fn execute_vim_command(
        &mut self,
        command: AppCommand,
    ) -> (bool, Vec<crate::app::command::AppEffect>) {
        let AppCommand::VimKey {
            key,
            shift,
            key_char,
        } = command
        else {
            return (false, Vec::new());
        };
        let handled = self.handle_vim_key(&key, shift, key_char.as_deref());
        let mut effects = Vec::new();
        if let Some((text, metadata)) = self.take_pending_clipboard_sync() {
            effects.push(crate::app::command::AppEffect::WriteClipboard { text, metadata });
        }
        if let Some(action) = self.take_pending_vim_action() {
            effects.push(crate::app::command::AppEffect::DispatchKeybind(action));
        }
        effects.append(&mut self.global_vim.pending_effects);
        (handled, effects)
    }

    pub fn dispatch(&mut self, command: crate::app::command::AppCommand) {
        for effect in self.execute(command) {
            self.apply_effect(effect);
        }
    }

    pub fn apply_effect(&mut self, effect: crate::app::command::AppEffect) {
        match effect {
            crate::app::command::AppEffect::ShowError(message) => {
                self.ui.notifications.push(crate::state::Notification {
                    severity: crate::state::NotificationSeverity::Error,
                    message,
                });
            }
            crate::app::command::AppEffect::WriteClipboard { .. }
            | crate::app::command::AppEffect::DispatchKeybind(_)
            | crate::app::command::AppEffect::PromptOpenFolder
            | crate::app::command::AppEffect::PromptOpenFile
            | crate::app::command::AppEffect::LoadDocument(_)
            | crate::app::command::AppEffect::LoadDocumentInCurrentTab(_)
            | crate::app::command::AppEffect::ScanWorkspace(_)
            | crate::app::command::AppEffect::PromptSaveAs(_)
            | crate::app::command::AppEffect::PerformSave(_) => {}
        }
    }
}

#[cfg(test)]
#[path = "editing_tests.rs"]
mod tests;
