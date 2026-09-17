use super::*;

impl AppState {
    pub(crate) fn set_keybind(
        &mut self,
        action: crate::keybinds::KeybindAction,
        slot: Option<usize>,
        combo: crate::keybinds::KeyCombo,
    ) {
        match slot {
            Some(index) => self.keybinds.set_at(action, index, combo),
            None => self.keybinds.add(action, combo),
        }
    }

    pub(crate) fn remove_keybind(&mut self, action: crate::keybinds::KeybindAction, index: usize) {
        self.keybinds.remove_at(action, index);
    }

    pub(crate) fn replace_keybinds(&mut self, keybinds: crate::keybinds::Keybinds) {
        self.keybinds = keybinds;
    }

    pub fn toggle_vim(&mut self) {
        self.global_vim.vim_enabled = !self.global_vim.vim_enabled;
        // Vim's flag rides in the keybinds file, not as a standalone setting.
        let _ = self
            .keybinds
            .save_to(&self.settings_path, self.global_vim.vim_enabled, &[]);
    }

    pub fn toggle_spellcheck(&mut self) {
        self.preferences.spellcheck_enabled = !self.preferences.spellcheck_enabled;
        self.save_flag("spellcheck", self.preferences.spellcheck_enabled);
    }

    pub fn toggle_nav_fold_buttons(&mut self) {
        self.preferences.nav_fold_buttons = !self.preferences.nav_fold_buttons;
        self.save_flag("nav_fold_buttons", self.preferences.nav_fold_buttons);
    }

    pub fn toggle_search_from_list(&mut self) {
        self.preferences.search_from_list_enabled = !self.preferences.search_from_list_enabled;
        self.save_flag(
            "search_from_list",
            self.preferences.search_from_list_enabled,
        );
    }

    pub fn toggle_search_list_whole_words(&mut self) {
        self.preferences.search_list_whole_words = !self.preferences.search_list_whole_words;
        self.save_flag(
            "search_list_whole_words",
            self.preferences.search_list_whole_words,
        );
        // The match count depends on this, so an open Search From List panel's
        // readout must follow the flip rather than going stale.
        self.refresh_find_matches();
    }

    pub fn toggle_command_palette_enabled(&mut self) {
        self.preferences.command_palette_enabled = !self.preferences.command_palette_enabled;
        self.save_flag("command_palette", self.preferences.command_palette_enabled);
        // Turning the feature off closes an already-open palette, rather than
        // leaving a panel up that its keybind can no longer reopen.
        if !self.preferences.command_palette_enabled {
            self.ui.command_palette = None;
        }
    }

    /// Writes one boolean to settings.conf. A failed write is logged, not
    /// propagated — losing the persisted flag must never take the flip with it.
    fn save_flag(&mut self, key: &str, value: bool) {
        if let Err(e) = crate::theme::save_setting_line(
            &self.settings_path,
            key,
            if value { "true" } else { "false" },
        ) {
            let message = format!("Failed to save settings: {e}");
            log_line(&format!("[settings] failed to save {key}: {e}"));
            self.apply_effect(crate::app::command::AppEffect::ShowError(message));
        }
    }

    // ── Command palette ─────────────────────────────────────────────────────

    /// Opens the palette with an empty query.
    ///
    /// Closes the find bar: both panels mount in the same slot under the
    /// ribbon (`main_window.rs`), so they cannot both be up. Enforced here (and
    /// symmetrically in `open_find_bar`) rather than at the call sites, so a
    /// future third opener can't forget it.
    pub fn toggle_paragraph_integrity(&mut self) {
        self.preferences.paragraph_integrity = !self.preferences.paragraph_integrity;
        if self.preferences.paragraph_integrity {
            self.set_paste_condense(false);
        }
    }

    /// Pilcrows: mark collapsed newlines with `¶`.
    ///
    /// Drives the `paste_condense_pilcrow` setting so the ribbon toggle and
    /// the settings modal are the same switch rather than two that disagree.
    pub fn toggle_pilcrows(&mut self) {
        self.preferences.pilcrows = !self.preferences.pilcrows;
        self.set_paste_condense_pilcrow(self.preferences.pilcrows);
    }

    /// Setters for the text settings that persist to settings.conf, so a
    /// change made from the ribbon survives a restart exactly like one made in
    /// the settings modal.
    pub fn set_paste_condense(&mut self, on: bool) {
        self.preferences.paste_condense = on;
        self.preferences.paste_condense = on;
        self.save_setting("paste_condense", if on { "true" } else { "false" });
    }

    pub fn set_paste_condense_pilcrow(&mut self, on: bool) {
        self.preferences.paste_condense_pilcrow = on;
        self.preferences.paste_condense_pilcrow = on;
        self.save_setting("paste_condense_pilcrow", if on { "true" } else { "false" });
    }

    /// The size Shrink drops text to, in points. Stored as half-points
    /// (`Run.size`'s unit) but written to settings.conf as points, which is
    /// what `small_size` has always held and what the user reads.
    pub fn set_shrink_size_points(&mut self, points: u16) {
        let points = clamp_shrink_size_points(points);
        self.preferences.small_size_half_points = points * 2;
        self.preferences.small_size_half_points = points * 2;
        self.save_setting("small_size", &points.to_string());
    }

    /// The body text size to render at, half-points: the active document's own
    /// `<w:docDefaults>` when it declares one, otherwise the `normal_text_size`
    /// setting.
    ///
    /// A document written in Word or Verbatim carries its own default size, and
    /// rendering it at this app's setting instead is what made an imported file
    /// look wrong before a single run had been touched. Rendering only — the
    /// setting still governs what *this* app applies (`ClearAll`'s
    /// `default_size`, a new document's own `docDefaults`), because that is a
    /// user preference rather than a property of the file being read.
    pub fn effective_normal_size_half_points(&self) -> u16 {
        self.workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.docx_origin.as_ref())
            .map(|o| o.doc_defaults.size)
            .filter(|&size| size > 0)
            .unwrap_or(self.preferences.normal_text_size_half_points)
    }

    /// The active document's own default body font, if it declares one — the
    /// `<w:rFonts>` half of the same fallback. Returned raw; the caller decides
    /// whether the app can actually render it (`text_editor`'s
    /// `is_curated_font`, which also covers imported families, so a Word
    /// document's Calibri renders once Calibri has been imported).
    pub fn effective_body_font(&self) -> Option<&str> {
        self.workspace
            .tabs
            .get(self.workspace.active_tab)
            .and_then(|t| t.docx_origin.as_ref())
            .and_then(|o| o.doc_defaults.font.as_deref())
    }

    /// The settings a freshly created `.docx` bakes into its `word/styles.xml`
    /// — body size and line spacing for `<w:docDefaults>`, the four card sizes
    /// for the `Heading1`-`Heading4` definitions, and what Emphasis means here.
    ///
    /// Collected in one place so `docx_parser` never has to reach back into
    /// settings, and so the crash-snapshot path can carry the same values as a
    /// normal save instead of quietly writing the defaults.
    pub fn new_doc_style(&self) -> crate::docx_parser::NewDocStyle {
        crate::docx_parser::NewDocStyle {
            normal_size: self.preferences.normal_text_size_half_points,
            line_spacing: self.preferences.line_spacing,
            pocket_size: self.preferences.pocket_size_half_points,
            hat_size: self.preferences.hat_size_half_points,
            block_size: self.preferences.block_size_half_points,
            tag_size: self.preferences.tag_size_half_points,
            cite_size: self.preferences.cite_size_half_points,
            emphasis_bold: self.preferences.emphasis_bold,
            emphasis_underline: self.preferences.emphasis_underline,
            emphasis_box: self.preferences.emphasis_box,
            emphasis_size: self
                .preferences
                .emphasis_change_size
                .then_some(self.preferences.emphasis_size_half_points),
        }
    }

    /// The configured size for one card style, in half-points — the single
    /// place `apply_card_style` and `build_new_doc_styles_xml` both read, so
    /// the style definition written into a new `.docx` can't drift from the
    /// size actually applied to the runs.
    pub fn card_size_half_points(&self, kind: CardStyleKind) -> u16 {
        match kind {
            CardStyleKind::Pocket => self.preferences.pocket_size_half_points,
            CardStyleKind::Hat => self.preferences.hat_size_half_points,
            CardStyleKind::Block => self.preferences.block_size_half_points,
            CardStyleKind::Tag => self.preferences.tag_size_half_points,
        }
    }

    /// Sets one card style's font size, in points, and persists it — the
    /// backing for the Text Settings steppers. Same
    /// half-points-internally/points-on-disk shape as
    /// `set_shrink_size_points`.
    pub fn set_card_size_points(&mut self, kind: CardStyleKind, points: u16) {
        let points = clamp_card_size_points(points);
        match kind {
            CardStyleKind::Pocket => self.preferences.pocket_size_half_points = points * 2,
            CardStyleKind::Hat => self.preferences.hat_size_half_points = points * 2,
            CardStyleKind::Block => self.preferences.block_size_half_points = points * 2,
            CardStyleKind::Tag => self.preferences.tag_size_half_points = points * 2,
        }
        match kind {
            CardStyleKind::Pocket => self.preferences.pocket_size_half_points = points * 2,
            CardStyleKind::Hat => self.preferences.hat_size_half_points = points * 2,
            CardStyleKind::Block => self.preferences.block_size_half_points = points * 2,
            CardStyleKind::Tag => self.preferences.tag_size_half_points = points * 2,
        }
        let key = match kind {
            CardStyleKind::Pocket => "pocket_size",
            CardStyleKind::Hat => "hat_size",
            CardStyleKind::Block => "block_size",
            CardStyleKind::Tag => "tag_size",
        };
        self.save_setting(key, &points.to_string());
    }

    /// Cite's size, which isn't a `CardStyleKind` (it targets the selection,
    /// not the whole line) and so keeps its own setter.
    pub fn set_cite_size_points(&mut self, points: u16) {
        let points = clamp_card_size_points(points);
        self.preferences.cite_size_half_points = points * 2;
        self.preferences.cite_size_half_points = points * 2;
        self.save_setting("cite_size", &points.to_string());
    }

    /// The size Emphasis resizes text to when `emphasis_change_size` is on,
    /// in points. Same half-points-internally/points-on-disk shape as
    /// `set_shrink_size_points`.
    pub fn set_emphasis_size_points(&mut self, points: u16) {
        let points = clamp_emphasis_size_points(points);
        self.preferences.emphasis_size_half_points = points * 2;
        self.preferences.emphasis_size_half_points = points * 2;
        self.save_setting("emphasis_size", &points.to_string());
    }

    /// Sets the document's line spacing and persists it, in Word's own
    /// multiplier unit (1.0 single, 1.5, 2.0 double).
    ///
    /// The future line-spacing button calls exactly this — it is the whole
    /// backing behaviour for that control, the same way
    /// `set_shrink_size_points` backs the Shrink stepper. Rendering picks the
    /// new value up on the next frame with no extra plumbing: every row-height
    /// call site reads `line_spacing` through `text_editor::line_height_px`,
    /// and `AppState` is a GPUI `Entity`, so the `update` that calls this
    /// already notifies the editor to repaint.
    ///
    /// Written back with one decimal place rather than `to_string()`'s
    /// shortest round-trip: `1.5f32.to_string()` is fine, but a value
    /// arrived at by repeated stepping can print as `1.2000001`, and
    /// settings.conf is a file users read and hand-edit.
    pub fn set_line_spacing(&mut self, spacing: f32) {
        let spacing = clamp_line_spacing(spacing);
        self.preferences.line_spacing = spacing;
        self.preferences.line_spacing = spacing;
        self.save_setting("line_spacing", &format!("{spacing:.1}"));
    }

    pub fn set_emphasis_change_size(&mut self, on: bool) {
        self.preferences.emphasis_change_size = on;
        self.preferences.emphasis_change_size = on;
        self.save_setting("emphasis_change_size", if on { "true" } else { "false" });
    }

    /// Sets the current highlight color and persists it.
    ///
    /// Called when a color is chosen from the ribbon's HL Color dropdown —
    /// picking one there is what "the current highlight color" means, and it
    /// is what the Highlight button, the Highlight keybind, Standardize
    /// Highlighting, and the HL Color button's own tint all read.
    ///
    /// `name` is a Word highlight-color name or a bare 6-digit hex, matching
    /// what `Run.highlight_color` stores.
    pub fn set_highlight_color(&mut self, name: &str) {
        self.preferences.highlight_color = name.to_string();
        self.preferences.highlight_color = name.to_string();
        self.save_setting("highlight_color", name);
    }

    pub fn set_analytic_color(&mut self, hex: &str) {
        self.preferences.analytic_color = hex.to_string();
        self.preferences.analytic_color = hex.to_string();
        self.save_setting("analytic_color", hex);
    }

    /// The highlight color "Standardize highlighting with exception" spares.
    /// An empty string clears it.
    pub fn set_standardize_exception(&mut self, name: &str) {
        self.preferences.standardize_highlight_exception = name.to_string();
        self.preferences.standardize_highlight_exception = name.to_string();
        self.save_setting("standardize_highlight_exception", name);
    }

    pub fn set_emphasis(&mut self, bold: bool, underline: bool, boxed: bool) {
        self.preferences.emphasis_bold = bold;
        self.preferences.emphasis_underline = underline;
        self.preferences.emphasis_box = boxed;
        self.preferences.emphasis_bold = bold;
        self.preferences.emphasis_underline = underline;
        self.preferences.emphasis_box = boxed;
        self.save_setting("emphasis_bold", if bold { "true" } else { "false" });
        self.save_setting(
            "emphasis_underline",
            if underline { "true" } else { "false" },
        );
        self.save_setting("emphasis_box", if boxed { "true" } else { "false" });
    }

    pub fn set_spellcheck_underline_color(&mut self, color: &str) {
        self.preferences.spellcheck_underline_color = color.to_string();
        self.save_setting("spellcheck_underline_color", color);
    }

    pub fn set_spreading_wpm(&mut self, wpm: u32) {
        let wpm = clamp_spreading_wpm(wpm);
        self.preferences.spreading_wpm = wpm;
        self.save_setting("spreading_wpm", &wpm.to_string());
    }

    pub fn set_theme(&mut self, theme: crate::theme::ThemeKind) {
        self.preferences.theme = theme;
        if let Err(error) = crate::theme::save_theme(&self.settings_path, theme) {
            self.report_settings_error(error);
        }
    }

    pub fn set_theme_mode(&mut self, mode: crate::theme::ThemeMode) {
        self.preferences.theme_mode = mode;
        if let Err(error) = crate::theme::save_theme_mode(&self.settings_path, mode) {
            self.report_settings_error(error);
        }
    }

    pub fn set_theme_color_mode(&mut self, mode: crate::theme::ThemeColorMode) {
        self.preferences.theme_color_mode = mode;
        if let Err(error) = crate::theme::save_theme_color_mode(&self.settings_path, mode) {
            self.report_settings_error(error);
        }
    }

    pub fn apply_theme_preferences(
        &mut self,
        theme: crate::theme::ThemeKind,
        mode: crate::theme::ThemeMode,
        color_mode: crate::theme::ThemeColorMode,
    ) {
        self.preferences.theme = theme;
        self.preferences.theme_mode = mode;
        self.preferences.theme_color_mode = color_mode;
    }

    fn report_settings_error(&mut self, error: impl std::fmt::Display) {
        self.apply_effect(crate::app::command::AppEffect::ReportError(
            crate::app::error::AppError::Settings(error.to_string()),
        ));
    }

    /// Writes one key to this state's settings.conf. Best-effort, matching
    /// every other settings write in this file — an unwritable directory must
    /// not break the in-memory change.
    fn save_setting(&mut self, key: &str, value: &str) {
        if let Err(e) = crate::preferences::Preferences::update(&self.settings_path, key, value) {
            self.apply_effect(crate::app::command::AppEffect::ShowError(format!(
                "Failed to save setting {}: {}",
                key, e
            )));
        }
    }

    /// Swaps the sidebar between its Files and Nav views.
    ///
    /// Beta feedback asked for a way to flip between them from the keyboard.
    /// Also reveals the sidebar when it is hidden — asking for a view while
    /// the panel is closed can only mean "show me that view", and toggling a
    /// mode nobody can see would look like the key did nothing.
    pub fn custom_colors(&self, target: CustomColorTarget) -> &[u32] {
        match target {
            CustomColorTarget::Font => &self.preferences.custom_font_colors,
            CustomColorTarget::Highlight => &self.preferences.custom_highlight_colors,
        }
    }

    /// Appends `hex` to the target list and persists it. Re-adding an existing
    /// color moves it to the end (most recent) rather than duplicating it.
    pub fn add_custom_color(&mut self, target: CustomColorTarget, hex: u32) {
        let list = match target {
            CustomColorTarget::Font => &mut self.preferences.custom_font_colors,
            CustomColorTarget::Highlight => &mut self.preferences.custom_highlight_colors,
        };
        if let Some(pos) = list.iter().position(|c| *c == hex) {
            list.remove(pos);
        }
        list.push(hex);
        while list.len() > MAX_CUSTOM_COLORS {
            list.remove(0);
        }
        self.persist_custom_colors(target);
    }

    /// Drops `hex` from the target list and persists the removal. Deliberately
    /// not confirmed — unlike a file delete, re-adding a swatch costs one click
    /// in the picker.
    pub fn remove_custom_color(&mut self, target: CustomColorTarget, hex: u32) {
        let list = match target {
            CustomColorTarget::Font => &mut self.preferences.custom_font_colors,
            CustomColorTarget::Highlight => &mut self.preferences.custom_highlight_colors,
        };
        let Some(pos) = list.iter().position(|c| *c == hex) else {
            return;
        };
        list.remove(pos);
        self.persist_custom_colors(target);
    }

    /// Writes one custom color list back to settings.conf. A failed write is
    /// logged, not propagated — losing a saved swatch must never take the
    /// applied color (or the user's edit) with it.
    pub(super) fn persist_custom_colors(&mut self, target: CustomColorTarget) {
        if let Err(e) = save_custom_colors(
            &self.settings_path,
            target.settings_key(),
            self.custom_colors(target),
        ) {
            let message = format!("Failed to save custom colors: {e}");
            log_line(&format!("[settings] {message}"));
            self.apply_effect(crate::app::command::AppEffect::ShowError(message));
        }
    }
}
