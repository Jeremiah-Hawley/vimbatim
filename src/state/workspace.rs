use super::*;
use crate::app::error::AppError;
use crate::app::repository::{DocumentRepository, WorkspaceRepository};
use crate::app::store::{DocumentStore, WorkspaceFs};

impl AppState {
    pub fn new_tab(&mut self) {
        /*
         * Appends a blank tab and makes it the active tab. Used when the user
         * clicks the "+" button in the tab bar or presses the new-tab keybind.
         */
        self.push_empty_tab();
        self.show_in_focused_pane(self.workspace.tabs.len() - 1);
    }

    /// Shows the tab at `idx` in whichever pane currently has focus.
    ///
    /// Needed because a pane's document is tracked by *id*, not by
    /// `active_tab`: opening a file while the secondary pane was focused
    /// otherwise moved `active_tab` while both panes kept pointing at their
    /// stored ids, and the new document appeared in neither half.
    ///
    /// This is also the affordance for getting an existing file into the
    /// second pane — focus it, then open from the sidebar.
    pub(super) fn show_in_focused_pane(&mut self, idx: usize) {
        self.workspace.active_tab = idx;
        let id = self.workspace.tabs.get(idx).map(|t| t.id);
        match self.workspace.focused_pane {
            Pane::Primary => self.workspace.primary_tab_id = id,
            Pane::Secondary => self.workspace.secondary_tab_id = id,
        }
        self.workspace.pending_focus_editor = Some(self.workspace.focused_pane);
    }

    /// Appends a blank tab and returns its stable id, without touching focus
    /// or `active_tab`. Shared by `new_tab` and `open_split`, which then do
    /// their own focusing — the two differ only in which pane ends up on it.
    pub(super) fn push_empty_tab(&mut self) -> TabId {
        let id = TabId(self.workspace.next_tab_id);
        self.workspace.tabs.push(Tab::new_empty(id));
        self.workspace.next_tab_id += 1;
        id
    }

    // ── Split view (notes/split_view_plan.md) ───────────────────────────────

    /// Enters or leaves reading mode.
    ///
    /// Entering collapses the split (the tab stays open — only the pane goes
    /// away, same as `close_split`) and hides the sidebar, so the document has
    /// the whole window. Leaving restores the sidebar to whatever it was.
    pub fn toggle_read_mode(&mut self) {
        self.ui.read_mode = !self.ui.read_mode;
        if self.ui.read_mode {
            self.ui.sidebar_before_read_mode = self.ui.sidebar_visible;
            self.close_split();
            self.ui.sidebar_visible = false;
        } else {
            self.ui.sidebar_visible = self.ui.sidebar_before_read_mode;
        }
    }

    /// Resolves a pane to a live index into `tabs`.
    ///
    /// The single place the secondary pane's stored `Tab.id` is turned into an
    /// index. `None` for `Secondary` when the split is closed, or when its tab
    /// has since been closed.
    pub fn tab_index(&self, id: TabId) -> Option<usize> {
        self.workspace.tabs.iter().position(|tab| tab.id == id)
    }

    pub fn take_pending_editor_focus(&mut self, pane: Pane) -> bool {
        if self.workspace.pending_focus_editor == Some(pane) {
            self.workspace.pending_focus_editor = None;
            true
        } else {
            false
        }
    }

    pub fn take_pending_scroll_to_cursor(&mut self, pane: Pane) -> bool {
        let Some(idx) = self.pane_tab_index(pane) else {
            return false;
        };
        std::mem::take(&mut self.workspace.tabs[idx].pending_scroll_to_cursor)
    }

    pub fn dismiss_unsupported_banner(&mut self, pane: Pane) {
        if let Some(idx) = self.pane_tab_index(pane) {
            self.workspace.tabs[idx].banner_dismissed = true;
        }
    }

    pub fn end_split_drag(&mut self) -> bool {
        std::mem::take(&mut self.workspace.split_dragging)
    }

    pub fn update_split_drag(&mut self, ratio: f32) -> bool {
        if self.workspace.split_ratio == ratio && self.workspace.split_dragging {
            return false;
        }
        self.workspace.split_ratio = ratio;
        self.workspace.split_dragging = true;
        true
    }

    pub fn pane_tab_index(&self, pane: Pane) -> Option<usize> {
        match pane {
            // While the secondary pane holds focus, `active_tab` names *its*
            // document, so the primary pane resolves through its remembered id
            // instead — otherwise both panes paint the same text.
            Pane::Primary => {
                if self.workspace.focused_pane == Pane::Primary
                    || self.workspace.primary_tab_id.is_none()
                {
                    (self.workspace.active_tab < self.workspace.tabs.len())
                        .then_some(self.workspace.active_tab)
                } else {
                    let id = self.workspace.primary_tab_id?;
                    self.workspace.tabs.iter().position(|t| t.id == id)
                }
            }
            Pane::Secondary => {
                if !self.workspace.split_view {
                    return None;
                }
                let id = self.workspace.secondary_tab_id?;
                self.workspace.tabs.iter().position(|t| t.id == id)
            }
        }
    }

    /// Moves editing focus to `pane`, pointing `active_tab` at that pane's tab.
    ///
    /// This is the whole mechanism that keeps split view from touching the 200+
    /// `self.workspace.active_tab` reads elsewhere in this file: "the active tab" and
    /// "the focused pane's tab" are the same thing, so every existing method
    /// acts on the right document without knowing panes exist.
    pub fn focus_pane(&mut self, pane: Pane) {
        match pane {
            Pane::Secondary => {
                let Some(idx) = self.pane_tab_index(Pane::Secondary) else {
                    return;
                };
                // Remember what the primary pane was on before `active_tab`
                // moves off it, so focusing back lands on the same document.
                if self.workspace.focused_pane == Pane::Primary {
                    self.workspace.primary_tab_id = self
                        .workspace
                        .tabs
                        .get(self.workspace.active_tab)
                        .map(|t| t.id);
                }
                self.workspace.active_tab = idx;
            }
            Pane::Primary => {
                if let Some(idx) = self.pane_tab_index(Pane::Primary) {
                    self.workspace.active_tab = idx;
                }
            }
        }
        self.workspace.focused_pane = pane;
        self.workspace.pending_focus_editor = Some(pane);
    }

    /// Opens the split with a fresh blank tab in the new pane.
    ///
    /// Idempotent: with the split already open this only focuses the secondary
    /// pane, rather than stacking up blank tabs on repeated clicks.
    pub fn open_split(&mut self) {
        if self.workspace.split_view {
            self.focus_pane(Pane::Secondary);
            return;
        }
        let id = self.push_empty_tab();
        self.workspace.secondary_tab_id = Some(id);
        self.workspace.split_view = true;
        self.focus_pane(Pane::Secondary);
    }

    /// Closes the split. The secondary tab stays *open* — only the pane goes
    /// away, so nothing the user typed into it is lost or hidden from the tab
    /// bar.
    pub fn close_split(&mut self) {
        if !self.workspace.split_view {
            return;
        }
        self.workspace.split_view = false;
        self.workspace.secondary_tab_id = None;
        self.workspace.focused_pane = Pane::Primary;
        self.workspace.pending_focus_editor = Some(Pane::Primary);
    }

    /// Toggles editing focus between the primary and secondary pane. A
    /// no-op while unsplit — `focus_pane(Secondary)` already early-returns
    /// via `pane_tab_index(Secondary)` returning `None` when
    /// `!self.workspace.split_view`, so this needs no guard of its own.
    pub fn switch_active_pane(&mut self) {
        let other = match self.workspace.focused_pane {
            Pane::Primary => Pane::Secondary,
            Pane::Secondary => Pane::Primary,
        };
        self.focus_pane(other);
    }

    /// Synchronous compatibility path for tests and non-GPUI callers.
    pub fn open_file(&mut self, path: PathBuf) {
        let result = DocumentStore.load_document(&path);
        self.complete_open_file(path, result);
    }

    /// Applies a document load result on the UI thread.
    pub fn complete_open_file(
        &mut self,
        path: PathBuf,
        result: Result<(Vec<Paragraph>, DocxOrigin), crate::app::error::AppError>,
    ) {
        /*
         * Opens a file in a new tab, parsing its docx content immediately.
         * If the file is already open, switches to the existing tab instead.
         *
         * When `parse_docx` fails (e.g., the file is corrupt or a 0-byte
         * placeholder), the tab still opens — empty, still titled after the
         * file — but *detached* from it; see `tab_from_docx`.
         *
         * Anything that isn't a .docx is refused outright. The guard lives here
         * rather than at the toolbar's file picker because that isn't the only
         * way an arbitrary path gets in: vim's `:e <path>` reaches this same
         * method, and GPUI's `PathPromptOptions` has no extension filter to set
         * on the native dialog. The sidebar's own tree is already .docx-only
         * (`scan_directory`), so this changes nothing for it.
         */
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
        {
            let message = format!("Not a .docx file: {}", path.display());
            log_line(&format!("[open] {message}"));
            self.apply_effect(crate::app::command::AppEffect::ShowError(message));
            return;
        }
        if let Some(idx) = self
            .workspace
            .tabs
            .iter()
            .position(|t| t.file_path.as_deref() == Some(&path))
        {
            // Already open in the *other* pane: focus that pane rather than
            // pulling the same document into this one (split-view decision 1).
            if self.workspace.split_view && self.pane_tab_index(Pane::Secondary) == Some(idx) {
                self.focus_pane(Pane::Secondary);
            } else if self.workspace.split_view && self.pane_tab_index(Pane::Primary) == Some(idx) {
                self.focus_pane(Pane::Primary);
            } else {
                self.show_in_focused_pane(idx);
            }
            return;
        }
        let load_error = result.as_ref().err().cloned();
        let tab = tab_from_loaded_docx(TabId(self.workspace.next_tab_id), &path, result);
        self.workspace.next_tab_id += 1;
        if let Some(error) = load_error {
            self.apply_effect(crate::app::command::AppEffect::ReportError(error));
        }
        if tab.opened_detached {
            self.apply_effect(crate::app::command::AppEffect::ShowError(format!(
                "Could not open {}; it was opened as a detached blank document.",
                path.display()
            )));
        }

        // An untouched "New Tab" is a placeholder, not work — opening a file
        // takes its slot instead of leaving a blank tab stranded beside the
        // document. Replacing *in place* keeps every other tab's index stable,
        // so the other pane's tab and any in-flight indices stay valid; the
        // replacement carries a fresh id, and `show_in_focused_pane` re-reads
        // it so this pane points at the new document rather than the discarded
        // placeholder.
        let reuse = self
            .pane_tab_index(self.workspace.focused_pane)
            .filter(|&i| {
                self.workspace
                    .tabs
                    .get(i)
                    .is_some_and(|t| t.is_blank_new_tab())
            });
        match reuse {
            Some(idx) => {
                self.workspace.tabs[idx] = tab;
                self.show_in_focused_pane(idx);
            }
            None => {
                self.workspace.tabs.push(tab);
                self.show_in_focused_pane(self.workspace.tabs.len() - 1);
            }
        }
    }

    pub fn save_active_tab(&mut self) -> Result<(), String> {
        /*
         * Saves the active tab's content to its associated file path. Thin
         * wrapper around `save_tab` (Task H pulled the actual work out into
         * an index-taking core so `:wa`, spec 5.7, can loop every tab
         * without needing to juggle `active_tab`).
         */
        self.save_tab(self.workspace.active_tab)
    }

    /// Writes the active tab to `path`, then re-points the tab at it — the
    /// "Save As" toolbar button and its Ctrl+Shift+S keybind.
    ///
    /// Re-points rather than just writing a copy, matching what every editor's
    /// Save As does: subsequent plain saves go to the new file, and the tab's
    /// title updates to the new name.
    ///
    /// `is_modified` is forced true before delegating because `save_tab`
    /// short-circuits on a clean tab — correct for plain Save (nothing
    /// changed, nothing to write) but wrong here, where the destination is a
    /// file that doesn't exist yet.
    pub fn save_active_tab_as(&mut self, path: PathBuf) -> Result<(), String> {
        self.save_tab_as(self.workspace.active_tab, path)
    }

    /// The index-taking core of `save_active_tab_as`, split out for the same
    /// reason `save_tab` was split out of `save_active_tab`: a caller that
    /// already knows which tab it means shouldn't have to route through
    /// `active_tab`.
    ///
    /// The tab-bar right-click menu specifically needs this — its "Save As"
    /// awaits a native file dialog, and `active_tab` can have moved to a
    /// different document by the time the user picks a path.
    pub fn prepare_save_as(
        &mut self,
        idx: usize,
        path: PathBuf,
    ) -> Result<
        Option<(
            TabId,
            Vec<Paragraph>,
            Option<Arc<DocxOrigin>>,
            PathBuf,
            crate::docx_parser::NewDocStyle,
        )>,
        String,
    > {
        let path = with_docx_extension(&path);

        let tab = self.workspace.tabs.get_mut(idx).ok_or("No active tab")?;
        if tab.is_saving {
            return Ok(None);
        }
        tab.file_path = Some(path.clone());
        tab.title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        tab.document.is_modified = true;
        self.prepare_save(idx)
    }

    pub fn save_tab_as(&mut self, idx: usize, path: PathBuf) -> Result<(), String> {
        // Core synchronous save-as for non-GPUI paths
        let Some((tab_id, paragraphs, origin, path, doc_style)) =
            self.prepare_save_as(idx, path)?
        else {
            return Ok(());
        };
        let save_started = Instant::now();
        let result = DocumentStore::save_document(&paragraphs, origin.as_deref(), &path, doc_style);

        let elapsed = save_started.elapsed();
        self.complete_save(tab_id, result.clone(), elapsed);

        result.map_err(|e| format!("Save failed: {e}"))
    }

    pub fn prepare_save(
        &mut self,
        idx: usize,
    ) -> Result<
        Option<(
            TabId,
            Vec<Paragraph>,
            Option<Arc<DocxOrigin>>,
            PathBuf,
            crate::docx_parser::NewDocStyle,
        )>,
        String,
    > {
        let (tab_id, paragraphs, origin, path) = {
            let tab = self.workspace.tabs.get_mut(idx).ok_or("No active tab")?;
            let Some(path) = tab.file_path.clone() else {
                if tab.opened_detached {
                    tab.banner_dismissed = false;
                }
                return Ok(None); // nothing to save yet
            };
            if tab.is_saving || !tab.document.is_modified {
                return Ok(None);
            }
            tab.is_saving = true;
            tab.saving_version = Some(tab.document.content_version);
            (
                tab.id,
                tab.document.paragraphs().to_vec(),
                tab.docx_origin.clone(),
                path,
            )
        };
        Ok(Some((
            tab_id,
            paragraphs.to_vec(),
            origin,
            path,
            self.new_doc_style(),
        )))
    }

    pub fn complete_save(
        &mut self,
        tab_id: TabId,
        result: Result<(), crate::app::error::AppError>,
        elapsed: Duration,
    ) {
        match result {
            Ok(()) => {
                if let Some(tab) = self.workspace.tabs.iter_mut().find(|t| t.id == tab_id) {
                    let saved_version = tab.saving_version.take();
                    tab.is_saving = false;
                    if saved_version == Some(tab.document.content_version) {
                        tab.document.is_modified = false;
                        tab.last_snapshot_version = tab.document.content_version;
                        crate::recovery::delete_snapshot(tab_id);
                    }
                    log_save_cost(tab.document.paragraphs(), elapsed);
                }
            }
            Err(e) => {
                if let Some(tab) = self.workspace.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.is_saving = false;
                    tab.saving_version = None;
                }
                self.apply_effect(crate::app::command::AppEffect::ReportError(e));
            }
        }
    }

    pub fn save_tab(&mut self, idx: usize) -> Result<(), String> {
        let Some((tab_id, paragraphs, origin, path, doc_style)) = self.prepare_save(idx)? else {
            return Ok(());
        };
        let save_started = Instant::now();
        let result = DocumentStore::save_document(&paragraphs, origin.as_deref(), &path, doc_style);

        let elapsed = save_started.elapsed();
        self.complete_save(tab_id, result.clone(), elapsed);

        result.map_err(|e| format!("Save failed: {e}"))
    }

    pub fn close_tab(&mut self, idx: usize) {
        /*
         * Removes the tab at the given index. Always keeps at least one tab open.
         * Adjusts the active_tab index to remain valid after removal.
         */
        if self.workspace.tabs.len() <= 1 {
            return; // always keep at least one tab
        }
        if idx >= self.workspace.tabs.len() {
            return;
        }
        // A deliberately closed tab has no unsaved work worth recovering.
        let closed_id = self.workspace.tabs.get(idx).map(|t| t.id);
        if let Some(id) = closed_id {
            crate::recovery::delete_snapshot(id);
        }
        // Only a file-backed tab can be reopened — a blank "New Tab" has
        // nothing on disk for `reopen_closed_tab` to load back.
        if let Some(path) = self
            .workspace
            .tabs
            .get(idx)
            .and_then(|t| t.file_path.clone())
        {
            self.workspace.closed_tabs.push(path);
        }
        self.workspace.tabs.remove(idx);

        // Two panes need two tabs. Collapse the split when the closed tab was
        // the secondary pane's own, or when only one tab is left for both to
        // share — either way the pane has nothing legal left to show.
        if self.workspace.split_view
            && (closed_id == self.workspace.secondary_tab_id || self.workspace.tabs.len() < 2)
        {
            self.close_split();
        }
        // If a tab to the left of the active one was removed, shift active_tab left.
        if idx < self.workspace.active_tab {
            self.workspace.active_tab -= 1;
        }
        // clamp active tab to valid range
        if self.workspace.active_tab >= self.workspace.tabs.len() {
            self.workspace.active_tab = self.workspace.tabs.len() - 1;
        }
        // Closing a tab (via its close button, not a click into the editor)
        // can leave a different tab active — same focus-loss bug as
        // `set_active_tab`/`open_file`, so request the same reclaim.
        // Harmless when the active tab didn't actually change: GPUI's
        // `focus()` is a no-op if the handle is already focused.
        self.workspace.pending_focus_editor = Some(self.workspace.focused_pane);
    }

    /// Settings → Keybinds "open a closed tab" (`Ctrl+Shift+W` by default):
    /// reopens the most recently closed file-backed tab, popping it off
    /// `closed_tabs`. Repeating the keybind walks back through however many
    /// tabs were closed, most-recent first. A no-op with nothing on the
    /// stack.
    pub fn reopen_closed_tab(&mut self) {
        if let Some(path) = self.workspace.closed_tabs.pop() {
            self.open_file(path);
        }
    }

    /// Entry point for the tab-bar's `×` button. Closes the tab immediately
    /// when it has no unsaved changes (unchanged behavior); otherwise arms
    /// `pending_close` so `close_confirm.rs` can ask Save/Discard/Cancel
    /// instead of silently dropping edits.
    pub fn request_close_tab(&mut self, idx: usize) {
        if self
            .workspace
            .tabs
            .get(idx)
            .map(|t| t.document.is_modified)
            .unwrap_or(false)
        {
            self.ui.pending_close = self
                .workspace
                .tabs
                .get(idx)
                .map(|tab| PendingClose::Tab(tab.id));
        } else {
            self.close_tab(idx);
        }
    }

    /// Tab-bar right-click "Close Tabs to the Left" / "…to the Right":
    /// closes every tab on that side of `idx`.
    ///
    // ponytail: skips dirty tabs instead of confirming them — `pending_close`
    // holds exactly one confirmation, so a batch close cannot ask about
    // several unsaved tabs, and closing them anyway would be a silent
    // discard. Upgrade path if this becomes annoying: make `pending_close`
    // carry a queue of indices that `close_confirm.rs` walks one at a time.
    ///
    /// Iterates from the far end inward so each removal only shifts indices
    /// that have already been visited.
    pub fn close_tabs_to_right(&mut self, idx: usize) {
        for i in (idx + 1..self.workspace.tabs.len()).rev() {
            if self
                .workspace
                .tabs
                .get(i)
                .is_some_and(|t| !t.document.is_modified)
            {
                self.close_tab(i);
            }
        }
    }

    /// Mirror of `close_tabs_to_right`. Same dirty-tab skip; walks downward
    /// from `idx - 1` so surviving (dirty) tabs below keep their indices.
    pub fn close_tabs_to_left(&mut self, idx: usize) {
        for i in (0..idx.min(self.workspace.tabs.len())).rev() {
            if self
                .workspace
                .tabs
                .get(i)
                .is_some_and(|t| !t.document.is_modified)
            {
                self.close_tab(i);
            }
        }
    }

    /// Entry point for the app-close `×`. When any tab has unsaved changes,
    /// arms `pending_close` so the confirm dialog can show. Otherwise
    /// resolves it straight back to `None` via `confirm_close_discard` —
    /// the GPUI caller reads "pending_close is None right after this call
    /// returns" as its own signal to call `cx.quit()` immediately, since
    /// this GPUI-free layer has no way to quit the app itself.
    pub fn request_close_app(&mut self) {
        self.ui.pending_close = Some(PendingClose::App);
        if !self.workspace.tabs.iter().any(|t| t.document.is_modified) {
            self.confirm_close_discard();
        }
    }

    /// Resolves the pending close by saving first: the target tab (or every
    /// tab, for an app-close) via `save_tab`, then closing it (tab-close
    /// only — an app-close still leaves the actual quitting to the caller).
    ///
    /// `save_tab` silently no-ops for a tab with no `file_path` (there's no
    /// "Save As" flow in this app to fall back to), and returns `Err` if the
    /// write itself fails. Either way it leaves `is_modified` `true` — the
    /// one reliable "did this actually get persisted?" signal available
    /// here — so that's what gates whether we actually close/quit. Returns
    /// whether it's now safe to proceed (close the tab / let the caller
    /// `cx.quit()`): `false` means at least one tab is still dirty and was
    /// deliberately left open rather than silently discarded.
    pub fn confirm_close_save(&mut self) -> bool {
        match self.ui.pending_close.take() {
            Some(PendingClose::Tab(id)) => {
                let Some(idx) = self.workspace.tabs.iter().position(|tab| tab.id == id) else {
                    return true;
                };
                let _ = self.save_tab(idx);
                let persisted = self
                    .workspace
                    .tabs
                    .get(idx)
                    .map(|t| !t.document.is_modified)
                    .unwrap_or(true);
                if persisted {
                    self.close_tab(idx);
                }
                persisted
            }
            Some(PendingClose::App) => {
                for idx in 0..self.workspace.tabs.len() {
                    let _ = self.save_tab(idx);
                }
                self.workspace.tabs.iter().all(|t| !t.document.is_modified)
            }
            None => true,
        }
    }

    /// Resolves the pending close by discarding unsaved changes: closes the
    /// target tab without saving (or, for an app-close, just clears
    /// `pending_close` — no tabs to remove, the caller quits).
    pub fn confirm_close_discard(&mut self) {
        match self.ui.pending_close.take() {
            Some(PendingClose::Tab(id)) => {
                if let Some(idx) = self.workspace.tabs.iter().position(|tab| tab.id == id) {
                    self.close_tab(idx);
                }
            }
            Some(PendingClose::App) => {
                // Quitting with changes deliberately discarded: nothing here
                // is worth recovering, so clear every snapshot rather than
                // prompting about it on next launch.
                for tab in &self.workspace.tabs {
                    crate::recovery::delete_snapshot(tab.id);
                }
            }
            None => {}
        }
    }

    /// Backs out of a pending close (Cancel button, or the confirm dialog's
    /// backdrop click) — leaves everything untouched.
    pub fn cancel_close(&mut self) {
        self.ui.pending_close = None;
    }

    /// Opens the "Add Font" popup (`font_import_modal.rs`) — the Font
    /// Family dropdown's "+ Add Font" row and the Fonts settings section
    /// both call this.
    pub fn open_font_import_modal(&mut self) {
        self.ui.font_import_modal_open = true;
    }

    /// Cancel button, or backdrop click, on the "Add Font" popup.
    pub fn close_font_import_modal(&mut self) {
        self.ui.font_import_modal_open = false;
    }

    /// Deletes a previously-imported font (Fonts settings section's remove
    /// button). See `font_import::remove`'s doc comment for why this takes
    /// effect on the picker/render path immediately but the GPUI-side face
    /// bytes themselves only clear on restart.
    pub fn remove_imported_font(&mut self, name: &str) {
        let _ = crate::font_import::remove(name);
    }

    /// The dirty tabs, flattened for the panic hook.
    pub fn dirty_tab_snapshots(&self) -> Vec<TabSnapshot> {
        let doc_style = self.new_doc_style();
        self.workspace
            .tabs
            .iter()
            .filter(|t| t.document.is_modified)
            .map(|t| TabSnapshot {
                id: t.id,
                paragraphs: t.document.paragraphs().to_vec(),
                origin: t.docx_origin.clone(),
                file_path: t.file_path.clone(),
                title: t.title.clone(),
                doc_style,
            })
            .collect()
    }

    /// Recovery option 1: throw the recovered changes away and delete the
    /// temporary file.
    pub fn discard_recovery(&mut self) {
        if self.recovery.pending_entries.is_empty() {
            return;
        }
        let entry = self.recovery.pending_entries.remove(0);
        crate::recovery::delete_entry(&entry);
    }

    /// Recovery option 2, part 1: hands the entry to the view so it can run
    /// the native save-file picker (which this GPUI-free layer cannot
    /// await), without popping it yet — a cancelled picker must leave the
    /// entry in place. The view calls `complete_recovery_save_as` once it
    /// has a destination.
    pub fn take_recovery_for_save_as(&mut self) -> Option<RecoveryEntry> {
        self.recovery.pending_entries.first().cloned()
    }

    /// Recovery option 2, part 2: copies the snapshot to the user's chosen
    /// path, then pops and deletes it.
    ///
    /// A plain file copy, not a re-save: the snapshot is already a valid
    /// .docx carrying the original template, so copying is both cheaper and
    /// lossless compared with parse-then-write.
    pub fn complete_recovery_save_as(
        &mut self,
        entry: &RecoveryEntry,
        dest: &Path,
    ) -> Result<(), String> {
        let dest = with_docx_extension(dest);
        std::fs::copy(&entry.snapshot, &dest).map_err(|e| format!("Save failed: {e}"))?;
        self.finish_recovery_save_as(entry)
    }

    /// Applies the UI-state half of an already completed recovery copy.
    /// The copy itself belongs on the background executor.
    pub fn finish_recovery_save_as(&mut self, entry: &RecoveryEntry) -> Result<(), String> {
        self.recovery
            .pending_entries
            .retain(|e| e.snapshot != entry.snapshot);
        crate::recovery::delete_entry(entry);
        Ok(())
    }

    /// Recovery option 3: reopen the document being edited with the
    /// recovered changes applied but *not* saved, so the user decides
    /// whether to keep them.
    ///
    /// Per the recovery spec, the original file is not validated: if it
    /// moved, changed, or was deleted since the crash, the tab still points
    /// at that path and a later save writes there.
    /// Synchronous compatibility path for tests and non-GPUI callers.
    pub fn resume_recovery(&mut self) {
        let Some(entry) = self.take_recovery_for_resume() else {
            return;
        };
        self.complete_recovery_resume(
            entry.clone(),
            parse_docx(&entry.snapshot).map_err(|e| e.to_string()),
        );
    }

    /// Removes the next pending recovery entry before its snapshot is parsed
    /// off the UI thread.
    pub fn take_recovery_for_resume(&mut self) -> Option<RecoveryEntry> {
        (!self.recovery.pending_entries.is_empty()).then(|| self.recovery.pending_entries.remove(0))
    }

    /// Applies a completed recovery parse on the UI thread.
    pub fn complete_recovery_resume(
        &mut self,
        entry: RecoveryEntry,
        result: Result<(Vec<Paragraph>, DocxOrigin), String>,
    ) {
        let Ok((paragraphs, origin)) = result else {
            crate::recovery::delete_entry(&entry);
            self.apply_effect(crate::app::command::AppEffect::ShowError(
                "Could not restore the recovery snapshot.".into(),
            ));
            return;
        };

        let mut tab = match &entry.original_path {
            Some(path) => Tab::from_path(TabId(self.workspace.next_tab_id), path.clone()),
            // Never-saved tab: reopen untitled, exactly as it was pre-crash.
            None => Tab::new_empty(TabId(self.workspace.next_tab_id)),
        };
        tab.document.replace_paragraphs(paragraphs);
        tab.has_unsupported_blocks = origin.has_unsupported_blocks;
        tab.docx_origin = Some(Arc::new(origin));
        if entry.original_path.is_none() {
            tab.title = entry.title.clone();
        }
        // The whole point of Resume: changes are present but unsaved.
        tab.document.is_modified = true;
        // `delete_entry` below removes the only on-disk copy of this content,
        // so the tab must be eligible for a fresh snapshot without waiting
        // for the user to type. A freshly built Tab has content_version ==
        // last_snapshot_version == 0, which `needs_snapshot` reads as "this
        // version is already written"; bumping the version clears that. The
        // edit stamp then just starts the normal idle debounce from now, so
        // the rewrite lands one interval after the resume rather than on the
        // very next tick.
        tab.document.content_version = 1;
        tab.document.last_edit_at = Some(Instant::now());

        self.workspace.next_tab_id += 1;
        self.workspace.tabs.push(tab);
        self.show_in_focused_pane(self.workspace.tabs.len() - 1);

        crate::recovery::delete_entry(&entry);
    }

    pub fn move_tab(&mut self, from: usize, to: usize) {
        /*
         * Moves the tab at `from` to position `to`, shifting other tabs as needed.
         * Updates `active_tab` so the visually active tab does not change.
         *
         * `to` is an *insert-before* position, so `to == tabs.len()` is legal
         * and means "past the last tab". That case is the whole reason a tab
         * could never be dragged to the end: the per-tab drop targets can only
         * ever say "before tab N", leaving the final slot unreachable until
         * the trailing drop targets (the "+" button and the empty strip beside
         * it, tab_bar.rs) could pass `len` here. Rejecting it, as this guard
         * used to, is what made those drops silent no-ops.
         */
        if from == to || from >= self.workspace.tabs.len() || to > self.workspace.tabs.len() {
            return;
        }
        let tab = self.workspace.tabs.remove(from);
        // When dragging right (from < to), remove() shifts the drop target left by one,
        // so insert at to-1 to land before the visual indicator.
        let insert_at = if from < to { to - 1 } else { to };
        self.workspace.tabs.insert(insert_at, tab);
        // Keep active_tab pointing at the same logical tab after the move.
        self.workspace.active_tab = if self.workspace.active_tab == from {
            insert_at
        } else if from < self.workspace.active_tab && insert_at >= self.workspace.active_tab {
            self.workspace.active_tab - 1
        } else if from > self.workspace.active_tab && insert_at <= self.workspace.active_tab {
            self.workspace.active_tab + 1
        } else {
            self.workspace.active_tab
        };
    }

    pub fn set_active_tab(&mut self, idx: usize) {
        /*
         * Switches focus to the tab at the given index, if it exists.
         * Requests that the text editor reclaim keyboard focus too (see
         * `pending_focus_editor`'s doc comment) — a tab-bar click never
         * touches GPUI focus on its own.
         *
         * A document is never shown in both panes at once
         * (`notes/split_view_plan.md`), and this is where that is enforced:
         * asking for the tab the secondary pane already holds focuses that
         * pane instead of pulling the document into the primary one. Doing it
         * here means the tab bar needs no special-casing at all.
         */
        if idx >= self.workspace.tabs.len() {
            return;
        }
        if self.workspace.split_view {
            let other = match self.workspace.focused_pane {
                Pane::Primary => Pane::Secondary,
                Pane::Secondary => Pane::Primary,
            };
            // Already showing in the other pane: focus it rather than
            // duplicating the document (split-view decision 1).
            if self.pane_tab_index(other) == Some(idx) {
                self.focus_pane(other);
                return;
            }
        }
        // Otherwise the tab opens in whichever pane is live. Clicking a tab is
        // how a document gets into the split at all, so this must *not* force
        // the primary pane — doing so made the second pane unable to show
        // anything but the blank tab the Split button created.
        self.show_in_focused_pane(idx);
    }

    pub fn next_tab(&mut self) {
        /*
         * Cycles to the next tab (Ctrl+Tab), wrapping from the last tab back
         * to the first. Routes through `set_active_tab` so keyboard focus
         * gets reclaimed the same way clicking a tab does.
         */
        if self.workspace.tabs.is_empty() {
            return;
        }
        self.set_active_tab((self.workspace.active_tab + 1) % self.workspace.tabs.len());
    }

    pub fn prev_tab(&mut self) {
        /*
         * Cycles to the previous tab (Ctrl+Shift+Tab), wrapping from the
         * first tab back to the last.
         */
        if self.workspace.tabs.is_empty() {
            return;
        }
        self.set_active_tab(
            (self.workspace.active_tab + self.workspace.tabs.len() - 1) % self.workspace.tabs.len(),
        );
    }

    pub fn rename_tab(&mut self, id: TabId, new_title: String) {
        /*
         * Renames the tab with the given stable id (double-click rename in
         * TabBar). Looks up by id rather than index so the caller doesn't
         * need to worry about tabs having shifted since the rename was
         * armed. A blank/whitespace-only title is silently ignored — real
         * vim/GUI editors don't let a document lose its name to an empty
         * text field.
         */
        if new_title.trim().is_empty() {
            return;
        }
        if let Some(tab) = self.workspace.tabs.iter_mut().find(|t| t.id == id) {
            tab.title = new_title;
        }
    }

    pub fn complete_file_tree_scan(&mut self, directory: &PathBuf, mut file_tree: Vec<FileNode>) {
        if &self.workspace.working_directory != directory {
            return;
        }
        let expanded = crate::file_explorer::collect_expanded_dirs(&self.workspace.file_tree);
        crate::file_explorer::restore_expanded_dirs(&mut file_tree, &expanded);
        self.workspace.file_tree = file_tree;
    }

    pub fn refresh_file_tree(&mut self) {
        /*
         * Re-scans the working directory and updates the file tree. Call this
         * after creating new files so the explorer reflects the new state.
         *
         * Expansion is carried across the rescan: `scan_directory` always
         * returns every directory collapsed, so without this every file
         * operation (new file, new folder, rename, duplicate, paste, delete)
         * would snap the whole tree shut and lose the user's place. Uses the
         * same collect/restore pair startup already uses to replay persisted
         * expansion onto a fresh scan.
         */
        let expanded = crate::file_explorer::collect_expanded_dirs(&self.workspace.file_tree);
        self.workspace.file_tree = WorkspaceFs
            .scan_directory(&self.workspace.working_directory)
            .unwrap_or_default();
        crate::file_explorer::restore_expanded_dirs(&mut self.workspace.file_tree, &expanded);
    }

    pub fn set_working_directory(&mut self, dir: PathBuf) {
        self.begin_working_directory(dir);
        self.refresh_file_tree();
    }

    /// Changes the root without scanning; views schedule the scan in the
    /// background and call `complete_file_tree_scan` on completion.
    pub fn begin_working_directory(&mut self, dir: PathBuf) {
        /*
         * Re-roots the file explorer at `dir` (the "Open Folder" button) and
         * shows the sidebar, since picking a folder implies the user wants to
         * see it. Open tabs are untouched — this only affects the tree.
         * Persists the new `working_directory` to settings.conf so the app
         * reopens here next launch instead of resetting to the default.
         */
        self.workspace.working_directory = dir;
        self.ui.sidebar_visible = true;
        let _ = save_working_directory(&self.settings_path, &self.workspace.working_directory);
    }

    // ── File explorer right-click menu (found_bugs.md Forgotten Implicit
    // Feature: right-click to delete or create) ────────────────────────────

    pub fn open_file_context_menu(&mut self, position: (f32, f32), target: FileContextMenuTarget) {
        /*
         * Opens (or repositions/retargets, if one is already open) the file
         * explorer's right-click menu. Always starts un-confirmed — even a
         * right-click while a delete confirmation is showing starts over
         * rather than carrying the old confirmation state to a new target.
         */
        self.ui.file_context_menu = Some(FileContextMenu {
            position,
            target,
            confirming_delete: false,
            rename_buffer: None,
        });
    }

    pub fn close_file_context_menu(&mut self) {
        self.ui.file_context_menu = None;
    }

    // ── Nav outline right-click menu ────────────────────────────────────────

    pub fn open_nav_context_menu(&mut self, position: (f32, f32), target: NavContextMenuTarget) {
        self.ui.nav_context_menu = Some(NavContextMenu { position, target });
    }

    pub fn close_nav_context_menu(&mut self) {
        self.ui.nav_context_menu = None;
    }

    pub fn request_context_menu_delete_confirmation(&mut self) {
        /*
         * Arms the "Delete <name>? Confirm / Cancel" step — see
         * `FileContextMenu.confirming_delete`'s doc comment for why deletion
         * isn't one click.
         */
        if let Some(menu) = self.ui.file_context_menu.as_mut() {
            menu.confirming_delete = true;
        }
    }

    pub fn confirm_context_menu_delete(&mut self) -> Result<(), AppError> {
        let result = match self.ui.file_context_menu.take() {
            Some(FileContextMenu {
                target: FileContextMenuTarget::File(path),
                ..
            }) => WorkspaceFs.remove_file(&path),
            _ => Ok(()),
        };
        self.refresh_file_tree();
        result
    }

    pub fn create_file_at_context_menu_location(&mut self) -> Result<(), AppError> {
        /*
         * Creates a new blank .docx in the context menu's target directory
         * (a File target's parent directory, a Dir target itself, or
         * `working_directory` for Background) and opens it — the
         * right-click counterpart to the sidebar's own "+" button
         * (`create_new_docx_in`), just targeting wherever was clicked
         * instead of always the tree's root.
         */
        let Some(menu) = self.ui.file_context_menu.take() else {
            return Ok(());
        };
        let dir = match menu.target {
            FileContextMenuTarget::File(path) => path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| self.workspace.working_directory.clone()),
            FileContextMenuTarget::Dir(path) => path,
            FileContextMenuTarget::Background => self.workspace.working_directory.clone(),
        };
        self.create_new_docx_in(&dir)
    }

    pub fn create_new_docx_in(&mut self, dir: &std::path::Path) -> Result<(), AppError> {
        let path = unique_path_in(dir, "Untitled", "docx");
        crate::app::store::DocumentStore.save_new_docx(
            &default_paragraphs(),
            &path,
            &self.new_doc_style(),
        )?;
        self.refresh_file_tree();
        self.open_file(path);
        Ok(())
    }

    /// The "New File" keybind/palette entry — same `create_new_docx_in`
    /// the sidebar's own "+" button (`FileExplorer::create_new_file`)
    /// calls, just resolving `dir = working_directory` itself since a
    /// global keybind handler has no clicked file-tree node to target.
    pub fn create_new_file_in_working_directory(&mut self) {
        let dir = self.workspace.working_directory.clone();
        if let Err(e) = self.create_new_docx_in(&dir) {
            let message = format!("Failed to create a file in {}: {e}", dir.display());
            log_line(&format!("[new file] {message}"));
            self.apply_effect(crate::app::command::AppEffect::ShowError(message));
        }
    }

    // ── File explorer right-click file operations ───────────────────────────
    //
    // Every one of these ends in `refresh_file_tree()` so the sidebar shows
    // the result immediately, and every one is .docx-scoped — `scan_directory`
    // only ever surfaces .docx files and directories, so no other extension
    // can reach here from the tree.

    /// "Open file in current tab" — replaces whatever the focused pane is
    /// showing, rather than `open_file`'s "reuse only a blank New Tab,
    /// otherwise append".
    ///
    /// Refuses to replace a *modified* tab and falls back to opening
    /// normally: replacing in place discards the tab's unsaved content with
    /// no confirmation, which is the exact thing `request_close_tab`'s
    /// Save/Discard/Cancel dialog exists to prevent. A clean tab has nothing
    /// to lose, which is the case this is actually for.
    /// Synchronous compatibility path for tests and non-GPUI callers.
    pub fn open_file_in_current_tab(&mut self, path: PathBuf) {
        let result = DocumentStore.load_document(&path);
        self.complete_open_file_in_current_tab(path, result);
    }

    /// Applies an asynchronously loaded document to the focused clean tab.
    pub fn complete_open_file_in_current_tab(
        &mut self,
        path: PathBuf,
        result: Result<(Vec<Paragraph>, DocxOrigin), crate::app::error::AppError>,
    ) {
        let replaceable = self
            .pane_tab_index(self.workspace.focused_pane)
            .filter(|&i| {
                self.workspace
                    .tabs
                    .get(i)
                    .is_some_and(|t| !t.document.is_modified)
            });
        let Some(idx) = replaceable else {
            self.complete_open_file(path, result);
            return;
        };
        if self
            .workspace
            .tabs
            .iter()
            .any(|t| t.file_path.as_deref() == Some(&path))
        {
            self.complete_open_file(path, result);
            return;
        }
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
        {
            let message = format!("Not a .docx file: {}", path.display());
            log_line(&format!("[open] {message}"));
            self.apply_effect(crate::app::command::AppEffect::ShowError(message));
            return;
        }
        let load_error = result.as_ref().err().cloned();
        let tab = tab_from_loaded_docx(TabId(self.workspace.next_tab_id), &path, result);
        self.workspace.next_tab_id += 1;
        if let Some(error) = load_error {
            self.apply_effect(crate::app::command::AppEffect::ReportError(error));
        }
        if tab.opened_detached {
            self.apply_effect(crate::app::command::AppEffect::ShowError(format!(
                "Could not open {}; it was opened as a detached blank document.",
                path.display()
            )));
        }
        if let Some(old) = self.workspace.tabs.get(idx) {
            crate::recovery::delete_snapshot(old.id);
            if let Some(old_path) = old.file_path.clone() {
                self.workspace.closed_tabs.push(old_path);
            }
        }
        self.workspace.tabs[idx] = tab;
        self.show_in_focused_pane(idx);
    }

    /// "Open file in side pane" — opens the split (idempotently) and loads
    /// `path` into the secondary pane. `open_split` leaves a blank tab that
    /// `open_file`'s own reuse branch then consumes, so this needs no
    /// special-casing for the already-split case.
    pub fn open_file_in_side_pane(&mut self, path: PathBuf) {
        self.open_split();
        self.open_file(path);
    }

    /// "Duplicate file" — copies `path` alongside itself as
    /// "<name> copy.docx" (or "<name> copy 2.docx", …). Does not open the
    /// copy: duplicating is usually a backup gesture, not an editing one.
    pub fn duplicate_file(&mut self, path: &std::path::Path) -> Result<(), AppError> {
        let dir = path
            .parent()
            .ok_or(AppError::Workspace("file has no parent directory".into()))?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or(AppError::Workspace("file has no name".into()))?;
        let dest = unique_path_in(dir, &format!("{stem} copy"), "docx");
        WorkspaceFs.copy_file(path, &dest)?;
        self.refresh_file_tree();
        Ok(())
    }

    /// "Copy file" — remembers `path` for a later "Paste file". Nothing
    /// touches the filesystem until the paste.
    pub fn copy_file(&mut self, path: PathBuf) {
        self.copied_file = Some((path, false));
    }

    /// "Cut file" — same mailbox as `copy_file`, marked as a move, so the
    /// next "Paste file" relocates the original instead of duplicating it.
    ///
    /// This is what `rename_path` used to do by accident when a path was
    /// typed into the rename box, only discoverable, able to move folders,
    /// and unable to drop something outside the project.
    pub fn cut_file(&mut self, path: PathBuf) {
        self.copied_file = Some((path, true));
    }

    /// "Paste file" — puts whatever "Copy file"/"Cut file" remembered into
    /// `dir`, under a name that can't collide (so pasting back into the
    /// source folder produces a copy rather than overwriting the original).
    ///
    /// A copy leaves `copied_file` set, so one copy can be pasted into
    /// several folders. A cut clears it: the original has moved, and pasting
    /// it a second time would only fail on a path that no longer exists.
    pub fn paste_file_into(&mut self, dir: &std::path::Path) -> Result<(), AppError> {
        let (src, is_cut) = self
            .copied_file
            .clone()
            .ok_or(AppError::Workspace("nothing copied".into()))?;
        let is_dir = src.is_dir();
        if is_cut {
            if src.parent() == Some(dir) {
                self.copied_file = None;
                return Ok(());
            }
            let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
            if is_dir && canon(dir).starts_with(canon(&src)) {
                return Err(AppError::Workspace(
                    "can't move a folder into itself".into(),
                ));
            }
        }
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or(AppError::Workspace("file has no name".into()))?;
        let dest = if is_dir {
            let mut candidate = dir.join(stem);
            let mut counter = 1;
            while candidate.exists() {
                candidate = dir.join(format!("{stem} {counter}"));
                counter += 1;
            }
            candidate
        } else {
            unique_path_in(dir, stem, "docx")
        };
        if is_cut {
            self.relocate_path(&src, dest, is_dir)?;
            self.copied_file = None;
        } else {
            WorkspaceFs.copy_file(&src, &dest)?;
            self.refresh_file_tree();
        }
        Ok(())
    }

    /// "New folder" — creates the first free "New Folder" / "New Folder 2" /
    /// … inside `dir`.
    pub fn create_new_folder_in(&mut self, dir: &std::path::Path) -> Result<(), AppError> {
        let mut name = "New Folder".to_string();
        let mut counter = 2;
        while dir.join(&name).exists() {
            name = format!("New Folder {counter}");
            counter += 1;
        }
        WorkspaceFs.create_dir(&dir.join(&name))?;
        self.refresh_file_tree();
        Ok(())
    }

    /// "Rename" — renames the file or folder at `old` to `new_name` in the
    /// same parent directory, then re-points whatever open tabs it affects.
    ///
    /// Re-pointing is the part that isn't optional: renaming something
    /// currently open is the common case, and a tab left holding a stale
    /// path would write its next save to a location that no longer exists.
    ///
    /// `with_docx_extension` is the same funnel-level guard "Save As" uses —
    /// a user typing a bare file name must not produce a file the tree
    /// (`scan_directory`, .docx-only) can never show again. Folders take no
    /// extension: `old.is_dir()` (checked before anything on disk has
    /// moved) picks the branch.
    pub fn rename_path(&mut self, old: &std::path::Path, new_name: &str) -> Result<(), AppError> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(AppError::Workspace("name cannot be empty".into()));
        }

        let mut components = std::path::Path::new(trimmed).components();
        let single_segment = matches!(
            (components.next(), components.next()),
            (Some(std::path::Component::Normal(_)), None),
        );
        if !single_segment {
            return Err(AppError::Workspace(
                "name can't contain a path — use Cut File and Paste File to move it".into(),
            ));
        }
        let dir = old
            .parent()
            .ok_or(AppError::Workspace("path has no parent directory".into()))?;
        let is_dir = old.is_dir();
        let new = if is_dir {
            dir.join(trimmed)
        } else {
            with_docx_extension(&dir.join(trimmed))
        };
        if new == old {
            return Ok(());
        }
        self.relocate_path(old, new, is_dir)
    }

    /// Moves `old` to `new` on disk and re-points everything that referred to
    /// it — shared by `rename_path` and by a cut/paste move, which differ
    /// only in how they arrive at `new`.
    ///
    /// Re-pointing is the part that isn't optional: renaming or moving
    /// something currently open is the common case, and a tab left holding a
    /// stale path would write its next save to a location that no longer
    /// exists.
    ///
    /// ponytail: `fs::rename` only, so a move across filesystems fails
    /// instead of falling back to copy-then-delete. Everything here lives
    /// under one working directory; add the fallback if that stops being
    /// true.
    pub(super) fn relocate_path(
        &mut self,
        old: &std::path::Path,
        new: PathBuf,
        is_dir: bool,
    ) -> Result<(), AppError> {
        if new.exists() {
            return Err(AppError::Workspace(format!(
                "{} already exists",
                new.display()
            )));
        }
        WorkspaceFs.rename(old, &new)?;
        // The reopen stack (Shift+Ctrl+W) holds paths of tabs closed earlier in
        // the session. They have to move with the file too: left stale, "reopen
        // last closed tab" pointed at a path that no longer exists, and — since
        // a *missing* file stays attached to its tab by design (`tab_from_docx`)
        // — the next save would recreate an empty document back at the old
        // location. Prefix-swapped for both branches: `strip_prefix` on a file
        // path against a file path yields an empty remainder, so `new.join("")`
        // is `new` and the same expression covers the single-file case.
        for path in self.workspace.closed_tabs.iter_mut() {
            if let Ok(rest) = path.strip_prefix(old) {
                *path = new.join(rest);
            }
        }
        if is_dir {
            // Files nested inside the renamed folder didn't rename
            // themselves — only prefix-swap `file_path`, never `title`.
            for tab in self.workspace.tabs.iter_mut() {
                if let Some(rest) = tab
                    .file_path
                    .as_deref()
                    .and_then(|p| p.strip_prefix(old).ok())
                {
                    tab.file_path = Some(new.join(rest));
                }
            }
            // `refresh_file_tree` carries expansion across a rescan by
            // matching absolute paths against the *old* tree — without this
            // prefix swap, the renamed folder (and any expanded descendant)
            // would silently collapse since its old path no longer exists.
            let expanded: Vec<PathBuf> =
                crate::file_explorer::collect_expanded_dirs(&self.workspace.file_tree)
                    .into_iter()
                    .map(|p| p.strip_prefix(old).map(|rest| new.join(rest)).unwrap_or(p))
                    .collect();
            self.workspace.file_tree = WorkspaceFs
                .scan_directory(&self.workspace.working_directory)
                .unwrap_or_default();
            crate::file_explorer::restore_expanded_dirs(&mut self.workspace.file_tree, &expanded);
        } else {
            let title = new
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Untitled")
                .to_string();
            for tab in self
                .workspace
                .tabs
                .iter_mut()
                .filter(|t| t.file_path.as_deref() == Some(old))
            {
                tab.file_path = Some(new.clone());
                tab.title = title.clone();
            }
            self.refresh_file_tree();
        }
        Ok(())
    }

    /// "Open all files in new tabs" — opens every .docx directly inside
    /// `dir`, in tree order. Deliberately not recursive: the menu item names
    /// the folder that was clicked, and walking a deep tree could open
    /// hundreds of tabs from one click.
    pub fn open_all_files_in_dir(&mut self, dir: &std::path::Path) {
        for path in WorkspaceFs
            .scan_directory(dir)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|n| match n {
                FileNode::File { path, .. } => Some(path),
                FileNode::Dir { .. } => None,
            })
        {
            self.open_file(path);
        }
    }
}

/// First non-existent `dir/<stem>.<ext>`, falling back to `<stem> 1`,
/// `<stem> 2`, … — the exact collision loop "New File" has always run for
/// "Untitled" (counter starting at 1, preserved), now shared with Duplicate
/// and Paste so all three name their results the same way.
///
/// Racy by construction (something could create the path between the check
/// and the write), which is fine here: every caller's own `fs` call reports
/// the failure, and the alternative is exclusive-create plumbing no
/// single-user desktop editor needs.
/// Builds the tab for `path`, parsing its docx content — the one place both
/// `open_file` and `open_file_in_current_tab` do it, so they cannot drift on
/// what a failed parse means.
///
/// When the parse fails on a file that *holds bytes*, the tab still opens,
/// still titled after the file, but *detached* from it: `file_path` is
/// cleared. That detachment is the whole point. With the path still attached
/// and `docx_origin` left at `None`, the first Ctrl+S took `save_tab`'s
/// `create_new_docx` branch and replaced the unreadable original with a blank
/// minimal document — silently, with no error and nothing to undo, on exactly
/// the files least likely to have a backup. Detached, Ctrl+S is the no-op
/// every path-less tab already is, and Save As still writes the content
/// anywhere the user picks, including back over the original when that is
/// genuinely what the user wants.
///
/// Deliberately *not* a refusal to open, and deliberately not applied to a
/// missing or empty file: a `touch`ed 0-byte placeholder is a real way to
/// start a document here, and a tab reopened after its file was deleted
/// should still be able to write itself back. Neither can lose content that
/// isn't there. Only the silent overwrite of real bytes goes away.
pub(super) fn tab_from_loaded_docx(
    id: TabId,
    path: &std::path::Path,
    result: Result<(Vec<Paragraph>, DocxOrigin), crate::app::error::AppError>,
) -> Tab {
    let mut tab = Tab::from_path(id, path.to_path_buf());
    match result {
        Ok((paragraphs, origin)) => {
            tab.document.replace_paragraphs(paragraphs);
            tab.has_unsupported_blocks = origin.has_unsupported_blocks;
            tab.docx_origin = Some(Arc::new(origin));
        }
        Err(e) => {
            // Bytes we couldn't read are bytes we must not destroy — but only
            // if there are any. A missing or zero-length file has nothing to
            // lose, so it stays attached and the first save creates it: that
            // is how a `touch`ed placeholder, and a tab reopened after its
            // file was deleted, are meant to work. Anything else is a file
            // holding content this build can't parse, and *that* is the case
            // where staying attached let `save_tab`'s `create_new_docx`
            // branch replace the original with a blank document.
            let holds_content = std::fs::metadata(path).is_ok_and(|m| m.len() > 0);
            if holds_content {
                tab.file_path = None;
                // Say so on screen. Detaching alone left the tab looking like it
                // had opened the file while Save silently did nothing.
                tab.opened_detached = true;
            }
            // stderr, matching how every other non-fatal file error in this
            // file reports itself — there's no in-app notification surface.
            log_line(&format!(
                "[open] couldn't read {}: {e}{}",
                path.display(),
                if holds_content {
                    " — opened detached from the file, use Save As to write it"
                } else {
                    ""
                },
            ));
        }
    }
    tab
}

pub fn unique_path_in(dir: &std::path::Path, stem: &str, ext: &str) -> PathBuf {
    let mut candidate = dir.join(format!("{stem}.{ext}"));
    let mut counter = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} {counter}.{ext}"));
        counter += 1;
    }
    candidate
}
