use super::*;

impl AppState {
    pub(crate) fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = clamp_sidebar_width(width);
    }

    pub(crate) fn set_sidebar_mode(&mut self, mode: SidebarMode) {
        self.sidebar_mode = mode;
    }

    pub fn notifications(&self) -> &[Notification] {
        &self.ui.notifications
    }

    pub fn find_bar_mut(&mut self) -> Option<&mut FindBar> {
        self.ui.find_bar.as_mut()
    }

    pub fn command_palette_mut(&mut self) -> Option<&mut CommandPaletteState> {
        self.ui.command_palette.as_mut()
    }

    pub fn file_context_menu_mut(&mut self) -> Option<&mut FileContextMenu> {
        self.ui.file_context_menu.as_mut()
    }

    pub fn take_file_context_menu(&mut self) -> Option<FileContextMenu> {
        self.ui.file_context_menu.take()
    }

    pub fn timer_mut(&mut self) -> &mut crate::timer::TimerState {
        &mut self.ui.timer
    }

    pub fn prep_timer_mut(&mut self) -> &mut crate::timer::PrepTimerState {
        &mut self.ui.prep_timer
    }

    pub fn toggle_timer(&mut self) {
        self.ui.timer.visible = !self.ui.timer.visible;
    }

    pub fn push_pending_keybind(&mut self, action: crate::keybinds::KeybindAction) {
        self.ui.pending_keybinds.push(action);
    }

    pub fn take_pending_keybinds(&mut self) -> Vec<crate::keybinds::KeybindAction> {
        std::mem::take(&mut self.ui.pending_keybinds)
    }

    pub fn close_settings(&mut self) {
        self.ui.settings_visible = false;
    }

    pub fn toggle_settings(&mut self) {
        self.ui.settings_visible = !self.ui.settings_visible;
    }

    pub fn close_word_count(&mut self) {
        self.ui.word_count_visible = false;
    }

    pub fn toggle_word_count(&mut self) {
        self.ui.word_count_visible = !self.ui.word_count_visible;
    }

    pub fn show_sidebar(&mut self) {
        self.ui.sidebar_visible = true;
    }

    pub fn close_editor_context_menu(&mut self) {
        self.ui.editor_context_menu = None;
    }

    pub fn open_editor_context_menu(&mut self, menu: EditorContextMenu) {
        self.ui.editor_context_menu = Some(menu);
    }

    pub fn open_find_bar(&mut self) {
        // Mutually exclusive with the command palette — see
        // `open_command_palette` for why this lives here and not at the call
        // sites.
        self.ui.command_palette = None;
        let selected = self.copy_selection().filter(|s| !s.contains('\n'));
        let bar = self.ui.find_bar.get_or_insert_with(FindBar::default);
        if let Some(text) = selected {
            bar.query = text;
        }
        bar.focus = FindField::Query;
        // `FindBar` is app-wide and reused across opens, so a stale `list_mode`
        // would otherwise survive into a plain Ctrl+F and leave the user in a
        // panel with no query field.
        bar.list_mode = false;
        self.refresh_find_matches();
    }

    /// Opens the same panel in Search From List mode — the toolbar's "Search
    /// From List" button.
    ///
    /// Opens even with an empty word list: the panel's readout is what tells
    /// the user the list lives in Settings, so refusing to open would hide the
    /// only signpost the feature has.
    pub fn open_search_from_list(&mut self) {
        self.ui.command_palette = None;
        let bar = self.ui.find_bar.get_or_insert_with(FindBar::default);
        bar.list_mode = true;
        bar.focus = FindField::Query;
        self.refresh_find_matches();
    }

    pub fn close_find_bar(&mut self) {
        self.ui.find_bar = None;
        self.workspace.pending_focus_editor = Some(self.workspace.focused_pane);
    }

    /// Replaces the Search From List words from the settings box's raw text
    /// (one word per line) and persists them. Re-runs the match count so an
    /// open panel's readout follows the edit live.
    pub fn set_search_word_list(&mut self, text: &str) {
        self.search_word_list = parse_word_list(text);
        save_word_list(&search_word_list_path(), &self.search_word_list);
        self.refresh_find_matches();
    }

    // ── Feature toggles ─────────────────────────────────────────────────────
    //
    // Flip *and* persist, together, here rather than in `settings_modal.rs`.
    // These used to live on `SettingsModal`, which meant the settings switch
    // was the only thing that saved them — so any second caller (the command
    // palette, which can run every one of these) would flip the flag in memory
    // and silently lose it on restart, with nothing to fail. Keeping the write
    // beside the flip is what makes a toggle correct from any caller.

    pub fn open_command_palette(&mut self) {
        self.ui.find_bar = None;
        self.ui.command_palette = Some(CommandPaletteState::default());
    }

    pub fn close_command_palette(&mut self) {
        self.ui.command_palette = None;
        self.workspace.pending_focus_editor = Some(self.workspace.focused_pane);
    }

    /// The word list as the settings box edits it: one word per line. The
    /// inverse of `set_search_word_list`, since the stored form is already
    /// trimmed and blank-free.
    pub fn search_word_list_text(&self) -> String {
        self.search_word_list.join("\n")
    }

    /// Recomputes the "N of M" readout. Cheap enough to run on every
    /// keystroke: it is one pass over the document with a byte comparison per
    /// candidate position.
    pub fn refresh_find_matches(&mut self) {
        let Some(bar) = self.ui.find_bar.as_ref() else {
            return;
        };
        let query = bar.query.clone();
        let list_mode = bar.list_mode;
        // Cloned up front so the `self.workspace.tabs` borrow below doesn't overlap the
        // word list's own `&self` borrow.
        let words = self.search_word_list.clone();
        let whole_words = self.preferences.search_list_whole_words;

        let (count, current) = match self.workspace.tabs.get(self.workspace.active_tab) {
            // List mode walks every word's matches merged into document order.
            // Match *ends* come from the matcher rather than `query.len()` —
            // list matches have differing lengths, which is exactly what the
            // single-needle arithmetic below can't express.
            Some(tab) if list_mode && !words.is_empty() => {
                let matches = list_matches(&tab.document.content(), &words, whole_words);
                let cursor = tab.cursor;
                // 1-based, like the single-needle branch; 0 means "the cursor
                // isn't sitting on a match".
                let current = matches
                    .iter()
                    .position(|&(start, end)| start < cursor && cursor <= end)
                    .map_or(0, |i| i + 1);
                (matches.len(), current)
            }
            Some(tab) if !list_mode && !query.is_empty() => {
                let cursor = tab.cursor;
                let mut count = 0;
                let mut current = 0;
                let mut at = 0;
                while let Some(pos) = find_from(&tab.document.content(), &query, at) {
                    count += 1;
                    // The match the cursor currently sits on or just past —
                    // `find_next` leaves the caret at the match's end.
                    if pos < cursor && cursor <= pos + query.len() {
                        current = count;
                    }
                    at = pos + query.len().max(1);
                }
                (count, current)
            }
            _ => (0, 0),
        };
        if let Some(bar) = self.ui.find_bar.as_mut() {
            bar.match_count = count;
            bar.current_match = current;
        }
    }

    /// Jumps to and selects the next (or previous) match, wrapping around the
    /// document like the vim `/` search this shares its wraparound semantics
    /// with. Returns false when there's nothing to find.
    pub fn find_next(&mut self, forward: bool) -> bool {
        let Some(bar) = self.ui.find_bar.as_ref() else {
            return false;
        };
        let (query, list_mode) = (bar.query.clone(), bar.list_mode);
        let words = self.search_word_list.clone();
        let whole_words = self.preferences.search_list_whole_words;
        if list_mode {
            if words.is_empty() {
                return false;
            }
        } else if query.is_empty() {
            return false;
        }
        let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) else {
            return false;
        };

        // Search from the current selection's far edge so repeated Next walks
        // forward instead of re-finding the match already highlighted.
        let from = match tab.selection {
            Some((a, f)) if forward => a.max(f),
            Some((a, f)) => a.min(f),
            None => tab.cursor,
        };
        // Both branches wrap the same way (retry from the far end of the
        // document), the vim `/` semantics this already shared. List mode
        // carries each match's own end, since a list's matches differ in
        // length and `query.len()` can't stand in for them.
        let found = match (list_mode, forward) {
            (true, true) => find_list_from(&tab.document.content(), &words, whole_words, from)
                .or_else(|| find_list_from(&tab.document.content(), &words, whole_words, 0)),
            (true, false) => rfind_list_before(&tab.document.content(), &words, whole_words, from)
                .or_else(|| {
                    rfind_list_before(
                        &tab.document.content(),
                        &words,
                        whole_words,
                        tab.document.content().len(),
                    )
                }),
            (false, true) => find_from(&tab.document.content(), &query, from)
                .or_else(|| find_from(&tab.document.content(), &query, 0))
                .map(|pos| (pos, pos + query.len())),
            (false, false) => rfind_before(&tab.document.content(), &query, from)
                .or_else(|| {
                    rfind_before(
                        &tab.document.content(),
                        &query,
                        tab.document.content().len(),
                    )
                })
                .map(|pos| (pos, pos + query.len())),
        };

        let Some((start, end)) = found else {
            return false;
        };
        if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
            tab.selection = Some((start, end));
            tab.cursor = end;
            tab.pending_scroll_to_cursor = true;
        }
        self.refresh_find_matches();
        true
    }

    /// Replaces the currently-selected match, then advances to the next one.
    /// A no-op unless the selection actually *is* a match — otherwise Replace
    /// pressed straight after opening the bar would overwrite arbitrary text.
    pub fn replace_current(&mut self) {
        let Some(bar) = self.ui.find_bar.as_ref() else {
            return;
        };
        let (query, replacement) = (bar.query.clone(), bar.replacement.clone());
        if query.is_empty() {
            return;
        }

        let selection_is_match = self
            .workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.selection.map(|(a, f)| (t, a.min(f), a.max(f))))
            .is_some_and(|(tab, start, end)| {
                end <= tab.document.content().len()
                    && end - start == query.len()
                    && tab.document.content()[start..end].eq_ignore_ascii_case(&query)
            });

        if selection_is_match {
            self.insert_str(&replacement);
        }
        self.find_next(true);
        self.refresh_find_matches();
    }

    /// Replaces every match in the document, returning how many were changed.
    ///
    /// Walks forward from the start, resuming *past* each replacement so a
    /// replacement containing the query (find "a", replace with "aa") can't
    /// loop forever.
    pub fn replace_all(&mut self) -> usize {
        let Some(bar) = self.ui.find_bar.as_ref() else {
            return 0;
        };
        let (query, replacement) = (bar.query.clone(), bar.replacement.clone());
        if query.is_empty() {
            return 0;
        }

        let mut replaced = 0;
        let mut at = 0;
        while let Some(tab) = self.workspace.tabs.get(self.workspace.active_tab) {
            let Some(pos) = find_from(&tab.document.content(), &query, at) else {
                break;
            };
            if let Some(tab) = self.workspace.tabs.get_mut(self.workspace.active_tab) {
                tab.selection = Some((pos, pos + query.len()));
                tab.cursor = pos + query.len();
            }
            self.insert_str(&replacement);
            replaced += 1;
            at = pos + replacement.len();
        }
        self.refresh_find_matches();
        replaced
    }

    /// Resolves the active theme's colors, custom or built-in — every view
    /// should call this instead of the bare `theme::palette()` free function,
    /// which is a `const fn` with no access to `custom_theme` and would
    /// silently fall back to a placeholder for `ThemeKind::Custom`.
    pub fn theme_palettes(&self) -> (crate::theme::Palette, crate::theme::Palette) {
        if self.preferences.theme == crate::theme::ThemeKind::Custom {
            if let Some(pair) = self.custom_theme {
                return pair;
            }
        }
        (
            crate::theme::palette(self.preferences.theme, crate::theme::ThemeMode::Dark),
            crate::theme::palette(self.preferences.theme, crate::theme::ThemeMode::Light),
        )
    }

    pub fn current_palette(&self) -> crate::theme::Palette {
        if self.preferences.theme == crate::theme::ThemeKind::Custom {
            if let Some((dark, light)) = self.custom_theme {
                return match self.preferences.theme_mode {
                    crate::theme::ThemeMode::Dark => dark,
                    crate::theme::ThemeMode::Light => light,
                };
            }
        }
        crate::theme::palette(self.preferences.theme, self.preferences.theme_mode)
    }

    /// Settings -> Themes -> Import Theme: parses `content` (the picked
    /// file's own text) as a custom theme, and if it's valid, adopts it —
    /// replacing any previously imported one — copies it to
    /// `custom_theme_path()` so it survives a restart even if the original
    /// file moves, switches `theme` to `Custom`, and persists that choice.
    /// Returns whether the import succeeded; a caller can show an error on
    /// `false` without touching any existing custom theme still in effect.
    pub fn import_custom_theme(&mut self, content: &str) -> bool {
        let Some(parsed) = crate::theme::parse_custom_theme_toml(content) else {
            return false;
        };
        self.custom_theme = Some(parsed);
        self.preferences.theme = crate::theme::ThemeKind::Custom;
        let _ = std::fs::write(custom_theme_path(), content);
        let _ = crate::theme::save_theme(&settings_conf_path(), self.preferences.theme);
        true
    }

    pub fn toggle_sidebar_mode(&mut self) {
        self.sidebar_mode = match self.sidebar_mode {
            SidebarMode::Files => SidebarMode::Nav,
            SidebarMode::Nav => SidebarMode::Files,
        };
        self.ui.sidebar_visible = true;
    }

    pub fn toggle_invisibility_mode(&mut self) {
        /*
         * Toggles invisibility mode. When on, only highlighted text,
         * tags, and citations are shown.
         */
        self.ui.invisibility_mode = !self.ui.invisibility_mode;
    }

    pub fn toggle_print_layout(&mut self) {
        self.ui.print_layout = !self.ui.print_layout;
    }
}
