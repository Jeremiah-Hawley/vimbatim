use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::docx_parser::{DocxDocument, create_new_docx, parse_docx};

/// Rapid edits within this window of the previous undo-stack push are
/// coalesced into the same undo step (spec 4.5), so e.g. typing a whole
/// word doesn't need one Ctrl+Z per character.
const UNDO_COALESCE_WINDOW: Duration = Duration::from_millis(300);
/// Maximum number of snapshots kept on a tab's undo stack (spec 4.5).
const UNDO_STACK_CAP: usize = 200;

/// The vim mode a tab's editing state is currently in (spec 5.1). `Insert`
/// behaves like the plain (non-vim) editor; the other four modes swallow
/// keystrokes that aren't part of their own command grammar rather than
/// letting them fall through to text insertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
}

/// A single editor tab, representing either an unsaved "new" tab or an opened .docx file.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: usize,
    pub title: String,
    pub file_path: Option<PathBuf>,
    pub content: String,
    pub is_modified: bool,
    /// Parsed docx document retained for lossless round-trip save. `None` for
    /// brand-new tabs that have never been saved or for files that failed to parse.
    pub document: Option<Arc<DocxDocument>>,
    /// Byte offset into `content` where the cursor currently sits.
    /// Always points to a valid UTF-8 char boundary.
    pub cursor: usize,
    /// Active text selection as (anchor, focus) byte offsets.
    /// Anchor is where the selection started; focus tracks the cursor.
    /// Normalise to (min, max) before any range operation. `None` means no selection.
    pub selection: Option<(usize, usize)>,
    /// Snapshots of `content` taken before each edit, most recent last.
    /// `undo()` pops from here onto `redo_stack`. Capped at UNDO_STACK_CAP.
    pub undo_stack: Vec<String>,
    /// Snapshots of `content` that `undo()` has moved past, most recent
    /// last. `redo()` pops from here back onto `undo_stack`. Cleared
    /// whenever a new edit is made, since it invalidates that history.
    pub redo_stack: Vec<String>,
    /// When the most recent undo-stack push happened, used to coalesce a
    /// burst of rapid edits (e.g. typing) into a single undo step rather
    /// than one per keystroke. `None` means no edit has been made yet, or
    /// the coalescing window was deliberately broken (e.g. by an undo/redo).
    pub last_edit_at: Option<Instant>,
    /// The tab's current vim mode. Only meaningful when `AppState.vim_enabled`
    /// is true; unused otherwise.
    pub vim_mode: VimMode,
    /// In-progress `:command` text while `vim_mode == Command`. Not yet
    /// populated by Task D (entry/exit only) — reserved for Task H's
    /// command parsing and Task E's count-prefix handling.
    pub vim_command_buf: String,
}

impl Tab {
    pub fn new_empty(id: usize) -> Self {
        /*
         * Creates a blank "New Tab" with no associated file. This is the default
         * starting state when the application opens or the user creates a new tab.
         */
        Tab {
            id,
            title: "New Tab".to_string(),
            file_path: None,
            content: String::new(),
            is_modified: false,
            document: None,
            cursor: 0,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
            vim_mode: VimMode::Normal,
            vim_command_buf: String::new(),
        }
    }

    pub fn from_path(id: usize, path: PathBuf) -> Self {
        /*
         * Creates a Tab associated with an existing file path. The tab title is
         * set to the file name. Content is populated by `open_file` which calls
         * this constructor then parses the docx immediately after.
         */
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        Tab {
            id,
            title,
            file_path: Some(path),
            content: String::new(),
            is_modified: false,
            document: None,
            cursor: 0,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
            vim_mode: VimMode::Normal,
            vim_command_buf: String::new(),
        }
    }
}

/// A node in the file explorer tree representing either a directory or a .docx file.
#[derive(Clone, Debug)]
pub enum FileNode {
    Dir {
        name: String,
        path: PathBuf,
        children: Vec<FileNode>,
        expanded: bool,
    },
    File {
        name: String,
        path: PathBuf,
    },
}

impl FileNode {
    pub fn name(&self) -> &str {
        /*
         * Returns the display name (file or directory name) for this node,
         * used when rendering the file explorer tree.
         */
        match self {
            FileNode::Dir { name, .. } => name,
            FileNode::File { name, .. } => name,
        }
    }

    pub fn path(&self) -> &PathBuf {
        /*
         * Returns the full filesystem path for this node.
         */
        match self {
            FileNode::Dir { path, .. } => path,
            FileNode::File { path, .. } => path,
        }
    }
}

/// The shared application state, owned as a GPUI Model and read/written by all views.
pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub next_tab_id: usize,
    pub sidebar_visible: bool,
    pub settings_visible: bool,
    pub working_directory: PathBuf,
    pub file_tree: Vec<FileNode>,
    /// Whether vim keybindings are active. Hardcoded to `true` for now —
    /// wiring this to settings.conf's `[KEYBINDS] vim` flag (spec 2.3) is a
    /// separate, explicitly deferred gap, not part of the Task A-I vim mode
    /// breakdown.
    pub vim_enabled: bool,
}

impl AppState {
    pub fn new() -> Self {
        /*
         * Initialises the application with a single empty tab, the sidebar visible,
         * the settings modal hidden, and the working directory set to the process's
         * current directory. The file tree is populated immediately by scanning that
         * directory for .docx files.
         */
        let working_directory = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."));

        let file_tree = scan_directory(&working_directory);

        AppState {
            tabs: vec![Tab::new_empty(0)],
            active_tab: 0,
            next_tab_id: 1,
            sidebar_visible: true,
            settings_visible: false,
            working_directory,
            file_tree,
            vim_enabled: true,
        }
    }

    pub fn new_tab(&mut self) {
        /*
         * Appends a blank tab and makes it the active tab. Used when the user
         * clicks the "+" button in the tab bar or presses the new-tab keybind.
         */
        let tab = Tab::new_empty(self.next_tab_id);
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn open_file(&mut self, path: PathBuf) {
        /*
         * Opens a file in a new tab, parsing its docx content immediately.
         * If the file is already open, switches to the existing tab instead.
         *
         * When `parse_docx` fails (e.g., the file is corrupt or a 0-byte placeholder),
         * the tab still opens with empty content and `document = None`.
         */
        if let Some(idx) = self.tabs.iter().position(|t| t.file_path.as_deref() == Some(&path)) {
            self.active_tab = idx;
            return;
        }
        let mut tab = Tab::from_path(self.next_tab_id, path.clone());
        if let Ok(doc) = parse_docx(&path) {
            tab.content  = doc.to_plain_text();
            tab.document = Some(Arc::new(doc));
        }
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn save_active_tab(&mut self) -> Result<(), String> {
        /*
         * Saves the active tab's content to its associated file path.
         *
         * When `document` is `Some`: uses `save_from_content` so the original
         * docx structure (styles, images) is preserved and only the body text
         * is replaced.
         *
         * When `document` is `None` (file created fresh inside vimbatim): uses
         * `create_new_docx` to write a valid minimal docx from scratch.
         *
         * Tabs with no file path (plain "New Tab") are silently skipped — there
         * is nowhere to write to yet.
         */
        let tab = self.tabs.get(self.active_tab).ok_or("No active tab")?;
        let path = match &tab.file_path {
            Some(p) => p.clone(),
            None    => return Ok(()), // nothing to save yet
        };
        if !tab.is_modified {
            return Ok(());
        }
        let content  = tab.content.clone();
        let document = tab.document.clone();
        match document {
            Some(doc) => doc.save_from_content(&content, &path)
                .map_err(|e| format!("Save failed: {}", e))?,
            None => create_new_docx(&content, &path)
                .map_err(|e| format!("Save failed: {}", e))?,
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.is_modified = false;
        }
        Ok(())
    }

    pub fn close_tab(&mut self, idx: usize) {
        /*
         * Removes the tab at the given index. Always keeps at least one tab open.
         * Adjusts the active_tab index to remain valid after removal.
         */
        if self.tabs.len() <= 1 {
            return; // always keep at least one tab
        }
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        // If a tab to the left of the active one was removed, shift active_tab left.
        if idx < self.active_tab {
            self.active_tab -= 1;
        }
        // clamp active tab to valid range
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn move_tab(&mut self, from: usize, to: usize) {
        /*
         * Moves the tab at `from` to position `to`, shifting other tabs as needed.
         * Updates `active_tab` so the visually active tab does not change.
         */
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        // When dragging right (from < to), remove() shifts the drop target left by one,
        // so insert at to-1 to land before the visual indicator.
        let insert_at = if from < to { to - 1 } else { to };
        self.tabs.insert(insert_at, tab);
        // Keep active_tab pointing at the same logical tab after the move.
        self.active_tab = if self.active_tab == from {
            insert_at
        } else if from < self.active_tab && insert_at >= self.active_tab {
            self.active_tab - 1
        } else if from > self.active_tab && insert_at <= self.active_tab {
            self.active_tab + 1
        } else {
            self.active_tab
        };
    }

    pub fn set_active_tab(&mut self, idx: usize) {
        /*
         * Switches focus to the tab at the given index, if it exists.
         */
        if idx < self.tabs.len() {
            self.active_tab = idx;
        }
    }

    fn push_undo_snapshot(&mut self) {
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
         */
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let now = Instant::now();
        let within_coalesce_window = tab.last_edit_at
            .map(|t| now.duration_since(t) < UNDO_COALESCE_WINDOW)
            .unwrap_or(false);
        tab.last_edit_at = Some(now);
        if within_coalesce_window {
            return;
        }
        tab.undo_stack.push(tab.content.clone());
        if tab.undo_stack.len() > UNDO_STACK_CAP {
            tab.undo_stack.remove(0);
        }
        tab.redo_stack.clear();
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some((a, f)) = tab.selection.take() {
                let (start, end) = (a.min(f), a.max(f));
                tab.content.drain(start..end);
                tab.cursor    = start;
                tab.is_modified = true;
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
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection_raw();
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.content.insert(tab.cursor, ch);
            tab.cursor += ch.len_utf8();
            tab.is_modified = true;
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
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection(); // already pushes its own undo snapshot
            return;
        }
        let at_document_start = self.tabs.get(self.active_tab).map(|t| t.cursor == 0).unwrap_or(true);
        if at_document_start { return; }
        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            // Walk back one char boundary
            let prev = tab.content[..tab.cursor]
                .char_indices().last().map(|(i, _)| i).unwrap_or(0);
            tab.content.remove(prev);
            tab.cursor = prev;
            tab.is_modified = true;
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
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.push_undo_snapshot();
        }
        self.delete_selection_raw();
    }

    pub fn undo(&mut self) {
        /*
         * Restores the most recent undo snapshot as the active tab's
         * content, pushing the content being replaced onto the redo stack
         * so `redo()` can restore it. No-op when there's nothing to undo.
         *
         * Only `content` is snapshotted (spec 4.5's undo_stack is
         * `Vec<String>`), so the cursor isn't restored to its exact
         * pre-edit position — it's clamped into the restored content's
         * bounds and onto its nearest valid char boundary instead, since
         * the old byte offset may no longer even be one.
         */
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let Some(previous) = tab.undo_stack.pop() else { return };
        let current = std::mem::replace(&mut tab.content, previous);
        tab.redo_stack.push(current);
        tab.selection = None;
        tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor);
        tab.is_modified = true;
        // Break the coalescing window so the next edit doesn't merge into
        // whatever was on top of the undo stack before this undo.
        tab.last_edit_at = None;
    }

    pub fn redo(&mut self) {
        /*
         * The undo counterpart: restores the most recently undone content
         * from the redo stack, pushing the content being replaced back onto
         * the undo stack. No-op when there's nothing to redo. Cursor
         * handling mirrors `undo()`.
         */
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let Some(next) = tab.redo_stack.pop() else { return };
        let current = std::mem::replace(&mut tab.content, next);
        tab.undo_stack.push(current);
        tab.selection = None;
        tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor);
        tab.is_modified = true;
        tab.last_edit_at = None;
    }

    pub fn copy_selection(&self) -> Option<String> {
        /*
         * Returns the selected text as an owned String, or None when there is no
         * active selection. Does not modify state; safe to call via entity.read(cx).
         */
        let tab = self.tabs.get(self.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        Some(tab.content[start..end].to_string())
    }

    pub fn cut_selection(&mut self) -> Option<String> {
        /*
         * Extracts the selected text, deletes it, and returns the text so the
         * caller can write it to the clipboard. Returns None when there is no
         * selection. Delegates deletion to delete_selection so cursor/is_modified
         * logic stays in one place.
         */
        let tab = self.tabs.get(self.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        let text = tab.content[start..end].to_string();
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
         */
        if text.is_empty() { return; }
        self.push_undo_snapshot();
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection_raw();
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.content.insert_str(tab.cursor, text);
            tab.cursor += text.len(); // text is valid UTF-8 so len() == byte count
            tab.is_modified = true;
        }
    }

    pub fn move_left(&mut self) {
        /*
         * Moves the cursor back one character boundary. Clamps at the start
         * of the document. Clears any active selection, matching plain
         * arrow-key behaviour (Shift+Left uses `extend_left` instead, which
         * shares this same char_left computation without clearing).
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = char_left(&tab.content, tab.cursor);
        }
    }

    pub fn move_right(&mut self) {
        /*
         * Moves the cursor forward one character boundary. Clamps at the end
         * of the document.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = char_right(&tab.content, tab.cursor);
        }
    }

    pub fn move_down(&mut self) {
        /*
         * Moves the cursor to the same character column on the next line,
         * clamped to that line's length if it's shorter. No-op on the last
         * line. Column is measured in chars (not bytes) so multi-byte
         * characters don't shift the apparent column.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = line_down(&tab.content, tab.cursor);
        }
    }

    pub fn move_up(&mut self) {
        /*
         * Moves the cursor to the same character column on the previous
         * line, clamped to that line's length if it's shorter. No-op on the
         * first line.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = line_up(&tab.content, tab.cursor);
        }
    }


    pub fn move_line_start(&mut self) {
        /*
         * Moves the cursor to the first byte of the current line.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = line_start(&tab.content, tab.cursor);
        }
    }

    pub fn move_line_first_nonblank(&mut self) {
        /*
         * Moves the cursor to the first non-whitespace character on the
         * current line. If the line is entirely whitespace, lands at the
         * end of the line (matching vim's `^` on a blank line).
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            let start = line_start(&tab.content, tab.cursor);
            let end   = line_end(&tab.content, tab.cursor);
            tab.cursor = tab.content[start..end]
                .char_indices()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| start + i)
                .unwrap_or(end);
        }
    }

    pub fn move_line_end(&mut self) {
        /*
         * Moves the cursor to the end of the current line — the byte offset
         * of the line's trailing '\n', or the end of the document on the
         * last line (which has no trailing '\n').
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = line_end(&tab.content, tab.cursor);
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = word_forward(&tab.content, tab.cursor);
        }
    }

    pub fn move_word_end(&mut self) {
        /*
         * Moves the cursor to the last character of the current or next
         * word, matching vim's `e`. If the cursor is already on a word's
         * last character, advances to the end of the following word.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = word_end(&tab.content, tab.cursor);
        }
    }

    pub fn move_word_backward(&mut self) {
        /*
         * Moves the cursor to the start of the current or previous word,
         * matching vim's `b`.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = word_backward(&tab.content, tab.cursor);
        }
    }

    pub fn move_doc_start(&mut self) {
        /*
         * Moves the cursor to the very start of the document.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = 0;
        }
    }

    pub fn move_doc_end(&mut self) {
        /*
         * Moves the cursor to the very end of the document.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = tab.content.len();
        }
    }

    pub fn move_to_line(&mut self, line: usize) {
        /*
         * Moves the cursor to the start of the given 1-indexed line number,
         * matching vim's `NG`/`Ng`. `line == 0` and `line == 1` both land on
         * the first line; a line number past the end of the document clamps
         * to the last line.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = line_offset(&tab.content, line.saturating_sub(1));
        }
    }

    pub fn extend_left(&mut self) {
        /*
         * Shift+Left: moves the cursor back one character, extending (or
         * creating) the active selection instead of clearing it — see
         * `extend_selection` for how the anchor is chosen.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = char_left(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_right(&mut self) {
        /*
         * Shift+Right: the extending counterpart to move_right.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = char_right(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_up(&mut self) {
        /*
         * Shift+Up: the extending counterpart to move_up.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = line_up(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_down(&mut self) {
        /*
         * Shift+Down: the extending counterpart to move_down.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = line_down(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_word_forward(&mut self) {
        /*
         * Shift+Ctrl+Right: the extending counterpart to move_word_forward.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = word_forward(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_word_backward(&mut self) {
        /*
         * Shift+Ctrl+Left: the extending counterpart to move_word_backward.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = word_backward(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_line_start(&mut self) {
        /*
         * Shift+Home: the extending counterpart to move_line_start.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = line_start(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_line_end(&mut self) {
        /*
         * Shift+End: the extending counterpart to move_line_end.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = line_end(&tab.content, tab.cursor);
            extend_selection(tab, new_cursor);
        }
    }

    pub fn extend_doc_start(&mut self) {
        /*
         * Shift+Ctrl+Home: the extending counterpart to move_doc_start.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            extend_selection(tab, 0);
        }
    }

    pub fn extend_doc_end(&mut self) {
        /*
         * Shift+Ctrl+End: the extending counterpart to move_doc_end.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = tab.content.len();
            extend_selection(tab, new_cursor);
        }
    }

    pub fn select_all(&mut self) {
        /*
         * Ctrl+A: selects the entire document and places the cursor at its
         * end, matching standard (non-vim) editor behaviour.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = Some((0, tab.content.len()));
            tab.cursor = tab.content.len();
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = byte_offset_for_line_col(&tab.content, line, col);
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let new_cursor = byte_offset_for_line_col(&tab.content, line, col);
            extend_selection(tab, new_cursor);
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return (0, 0) };
        let start = line_start(&tab.content, tab.cursor);
        let col = tab.content[start..tab.cursor].chars().count();
        let line_idx = tab.content[..start].matches('\n').count();
        (line_idx, col)
    }

    pub fn active_content(&self) -> &str {
        /*
         * Returns the text content of the currently active tab, or an empty
         * string if there are no tabs.
         */
        self.tabs
            .get(self.active_tab)
            .map(|t| t.content.as_str())
            .unwrap_or("")
    }

    pub fn refresh_file_tree(&mut self) {
        /*
         * Re-scans the working directory and updates the file tree. Call this
         * after creating new files so the explorer reflects the new state.
         */
        self.file_tree = scan_directory(&self.working_directory);
    }

    // ── vim mode transitions (spec 5.1) ─────────────────────────────────────────

    pub fn vim_enter_insert_before_cursor(&mut self) {
        /*
         * 'i' — enters Insert mode at the current cursor position, unchanged.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::Insert;
            tab.selection = None;
        }
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let new_line_start = tab.cursor;
        self.insert_char('\n');
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.cursor = new_line_start;
        }
        self.vim_enter_insert_before_cursor();
    }

    pub fn vim_enter_visual(&mut self) {
        /*
         * 'v' — character-wise Visual mode, selecting the single character
         * under the cursor (matching real vim's immediate 1-char selection
         * on entry). Degenerates to a zero-width selection at document end,
         * where there's no character under the cursor.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::Visual;
            let end = char_right(&tab.content, tab.cursor);
            tab.selection = Some((tab.cursor, end));
        }
    }

    pub fn vim_enter_visual_line(&mut self) {
        /*
         * 'V' — line-wise Visual mode, selecting the whole current line
         * including its trailing newline when one exists, so a future
         * line-wise operator acts on the complete line.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::VisualLine;
            let start = line_start(&tab.content, tab.cursor);
            let end = line_end(&tab.content, tab.cursor);
            let end_with_newline = if end < tab.content.len() { end + 1 } else { end };
            tab.selection = Some((start, end_with_newline));
        }
    }

    pub fn vim_enter_command(&mut self) {
        /*
         * ':' — enters Command mode. Actual command text capture/parsing/
         * dispatch (spec 5.7) is Task H's job; this handles only the entry
         * transition.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::Command;
        }
    }

    pub fn vim_exit_to_normal(&mut self) {
        /*
         * Escape (from Insert/Visual/VisualLine/Command), or the Visual/
         * VisualLine toggle-off key — every "-> Normal" transition in spec
         * 5.1's table shares this one method.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::Normal;
            tab.selection = None;
            tab.vim_command_buf.clear();
        }
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return false };
        match tab.vim_mode {
            VimMode::Normal => self.handle_vim_normal_key(key, shift, key_char),
            VimMode::Visual | VimMode::VisualLine => {
                self.handle_vim_visual_key(key, shift);
                true
            }
            VimMode::Command => {
                self.handle_vim_command_key(key);
                true
            }
            VimMode::Insert => false,
        }
    }

    fn handle_vim_normal_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
        /*
         * Handles Normal-mode key presses. Only Task D's mode-switch keys
         * are implemented here — Task E adds real vim motions/operators.
         * Named navigation keys (arrow keys, Home/End) are deliberately let
         * through (return false) so the editor stays usable for moving
         * around before Task E lands; a plain cursor move can't corrupt
         * anything since Normal mode has no active selection to
         * accidentally clear. Everything else — in particular single
         * printable characters — is swallowed: real vim's Normal mode never
         * lets an unrecognized keystroke fall through to text insertion,
         * whether or not that specific command exists yet.
         *
         * ':' detection checks both the unshifted base key GPUI reports in
         * `key` (";" with shift held) and `key_char` (the actual shifted
         * character, when GPUI supplies it), since which of the two
         * reliably carries shifted punctuation hasn't been confirmed
         * against a real keyboard yet.
         */
        let is_colon = (key == ";" && shift) || key_char == Some(":");
        if is_colon {
            self.vim_enter_command();
            return true;
        }
        match (key, shift) {
            ("i", false) => { self.vim_enter_insert_before_cursor(); true }
            ("i", true)  => { self.vim_enter_insert_line_start(); true }
            ("a", false) => { self.vim_enter_insert_after_cursor(); true }
            ("a", true)  => { self.vim_enter_insert_line_end(); true }
            ("o", false) => { self.vim_open_line_below(); true }
            ("o", true)  => { self.vim_open_line_above(); true }
            ("v", false) => { self.vim_enter_visual(); true }
            ("v", true)  => { self.vim_enter_visual_line(); true }
            ("left", _) | ("right", _) | ("up", _) | ("down", _) | ("home", _) | ("end", _) => false,
            _ => true,
        }
    }

    fn handle_vim_visual_key(&mut self, key: &str, shift: bool) {
        /*
         * Visual/VisualLine consume every key (the caller always treats
         * this mode as handled) since letting navigation fall through here
         * would clear the active selection via move_left/right/etc.'s
         * selection-clearing behaviour, corrupting the selection instead of
         * extending it — unlike Normal mode, which has no selection to
         * protect. Real selection-extending motions are Task E/G's job.
         *
         * Only the exact exits spec 5.1 lists are implemented: lowercase
         * 'v' closes Visual, shifted 'V' closes VisualLine. Real vim also
         * lets 'v' and 'V' switch directly between the two Visual variants
         * without returning to Normal — that's not in the spec table and is
         * out of scope here, so the mismatched key/shift combination is
         * just swallowed as a no-op.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let mode = tab.vim_mode;
        if key == "escape" {
            self.vim_exit_to_normal();
            return;
        }
        match (mode, key, shift) {
            (VimMode::Visual, "v", false) => self.vim_exit_to_normal(),
            (VimMode::VisualLine, "v", true) => self.vim_exit_to_normal(),
            _ => {}
        }
    }

    fn handle_vim_command_key(&mut self, key: &str) {
        /*
         * Task D implements only the entry/exit half of Command mode —
         * actual `:`-command text capture, parsing, and dispatch (spec 5.7)
         * is Task H's job. Escape discards and Enter both return to Normal
         * without executing anything; every other key is swallowed rather
         * than falling through to text insertion, which would be actively
         * wrong for Command mode.
         */
        if key == "escape" || key == "enter" {
            self.vim_exit_to_normal();
        }
    }
}

/// Recursively scans `dir` and builds a tree of FileNodes containing only .docx
/// files (or directories that contain them).
pub fn scan_directory(dir: &PathBuf) -> Vec<FileNode> {
    /*
     * Reads the given directory and returns a sorted list of FileNodes.
     * Directories are listed before files. Only .docx files are included.
     * Directories without any .docx descendants are still shown so the user
     * can see the folder structure.
     */
    let mut nodes: Vec<FileNode> = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return nodes,
    };

    let mut dirs: Vec<FileNode> = Vec::new();
    let mut files: Vec<FileNode> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry
            .file_name()
            .to_string_lossy()
            .to_string();

        // skip hidden files/dirs (those starting with '.')
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            let children = scan_directory(&path);
            dirs.push(FileNode::Dir {
                name,
                path,
                children,
                expanded: false,
            });
        } else if path.extension().and_then(|e| e.to_str()) == Some("docx") {
            files.push(FileNode::File { name, path });
        }
    }

    // Sort each group alphabetically
    dirs.sort_by(|a, b| a.name().cmp(b.name()));
    files.sort_by(|a, b| a.name().cmp(b.name()));

    nodes.extend(dirs);
    nodes.extend(files);
    nodes
}

fn extend_selection(tab: &mut Tab, new_cursor: usize) {
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

fn clamp_to_char_boundary(content: &str, byte: usize) -> usize {
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
        (0..byte).rev().find(|&i| content.is_char_boundary(i)).unwrap_or(0)
    }
}

fn char_left(content: &str, cursor: usize) -> usize {
    /*
     * Returns the previous character boundary before `cursor`, clamped at 0.
     * Shared by `move_left` (clears selection) and `extend_left` (extends
     * it) so the two stay in lockstep by construction.
     */
    if cursor == 0 { return 0; }
    content[..cursor].char_indices().last().map(|(i, _)| i).unwrap_or(0)
}

fn char_right(content: &str, cursor: usize) -> usize {
    /*
     * Returns the next character boundary after `cursor`, clamped at
     * `content.len()`.
     */
    if cursor >= content.len() { return content.len(); }
    content[cursor..].char_indices().nth(1).map(|(i, _)| cursor + i).unwrap_or(content.len())
}

fn line_down(content: &str, cursor: usize) -> usize {
    /*
     * Returns the byte offset at the same character column on the line
     * after `cursor`'s line, clamped to that line's length. Returns
     * `cursor` unchanged (no-op) when already on the last line.
     */
    let start = line_start(content, cursor);
    let end   = line_end(content, cursor);
    if end >= content.len() { return cursor; } // last line, nothing below
    let col = content[start..cursor].chars().count();
    let next_start = end + 1; // skip the '\n'
    let next_end   = line_end(content, next_start);
    byte_offset_for_col(&content[next_start..next_end], col) + next_start
}

fn line_up(content: &str, cursor: usize) -> usize {
    /*
     * Returns the byte offset at the same character column on the line
     * before `cursor`'s line, clamped to that line's length. Returns
     * `cursor` unchanged (no-op) when already on the first line.
     */
    let start = line_start(content, cursor);
    if start == 0 { return cursor; } // first line, nothing above
    let col = content[start..cursor].chars().count();
    let prev_end   = start - 1; // the '\n' ending the previous line
    let prev_start = line_start(content, prev_end);
    byte_offset_for_col(&content[prev_start..prev_end], col) + prev_start
}

fn line_start(content: &str, pos: usize) -> usize {
    /*
     * Returns the byte offset of the start of the line containing `pos` —
     * the char immediately after the preceding '\n', or 0 for the first
     * line.
     */
    content[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

fn line_end(content: &str, pos: usize) -> usize {
    /*
     * Returns the byte offset of the end of the line containing `pos` — the
     * index of the '\n' that ends it, or `content.len()` for the last line.
     */
    content[pos..].find('\n').map(|i| pos + i).unwrap_or(content.len())
}

fn line_offset(content: &str, line_idx: usize) -> usize {
    /*
     * Returns the byte offset of the start of the given 0-indexed line
     * number, clamping to the start of the last line if `line_idx` is past
     * the end of the document.
     */
    let mut offset = 0;
    for _ in 0..line_idx {
        let end = line_end(content, offset);
        if end >= content.len() { break; } // no more lines; clamp here
        offset = end + 1;
    }
    offset
}

fn byte_offset_for_line_col(content: &str, line: usize, col: usize) -> usize {
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

fn byte_offset_for_col(line: &str, col: usize) -> usize {
    /*
     * Maps a character column (not byte column) within a single line to a
     * byte offset relative to the start of that line, clamping to the
     * line's length when `col` exceeds the number of characters on the
     * line.
     */
    line.char_indices().nth(col).map(|(i, _)| i).unwrap_or(line.len())
}

/// The three character classes vim's word motions distinguish: alphanumeric
/// "word" characters, standalone "punctuation" characters (each run of
/// punctuation is its own word), and whitespace (never part of a word).
#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Word,
    Punct,
    Space,
}

fn char_class(c: char) -> CharClass {
    /*
     * Classifies a single character for vim word-motion purposes: alnum/`_`
     * is a "word" char, whitespace is its own class, and everything else
     * (punctuation) is a third class — each punctuation run is treated as
     * its own word, matching vim rather than a naive whitespace-only split.
     */
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn skip_whitespace(content: &str, from: usize) -> usize {
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

fn word_forward(content: &str, pos: usize) -> usize {
    /*
     * vim `w`: byte offset of the start of the next word after `pos`. Skips
     * the rest of the current char-class run, then skips whitespace
     * (crossing newlines freely) to land on the first character of the
     * following word.
     */
    if pos >= content.len() { return pos; }
    let start_class = char_class(content[pos..].chars().next().unwrap());
    // Find where the current char-class run ends; if it runs to the end of
    // the document without changing class, idx stays at content.len().
    let mut idx = content.len();
    for (i, c) in content[pos..].char_indices() {
        if char_class(c) != start_class {
            idx = pos + i;
            break;
        }
    }
    // If the run ended on a non-space char, that's the next word's start.
    // Otherwise (it ended on whitespace, or `pos` itself was whitespace)
    // skip forward to the next non-space char.
    if idx < content.len() && char_class(content[idx..].chars().next().unwrap()) != CharClass::Space {
        return idx;
    }
    skip_whitespace(content, idx)
}

fn word_end(content: &str, pos: usize) -> usize {
    /*
     * vim `e`: byte offset of the last character of the current word (if
     * the cursor isn't already there) or of the next word (if it is).
     */
    if pos >= content.len() { return pos; }
    let cur_char = content[pos..].chars().next().unwrap();
    let cur_class = char_class(cur_char);
    let next_idx = pos + cur_char.len_utf8();
    let next_class = (next_idx < content.len())
        .then(|| char_class(content[next_idx..].chars().next().unwrap()));
    // "At a word's end" means the cursor is on whitespace, or the next char
    // starts a different class's run — in either case there's nowhere left
    // to advance within the current word, so jump to the next word instead.
    let at_word_end = cur_class == CharClass::Space
        || next_class.map(|c| c != cur_class).unwrap_or(true);

    let i = if at_word_end {
        let skip_from = if cur_class == CharClass::Space { pos } else { next_idx };
        skip_whitespace(content, skip_from)
    } else {
        next_idx
    };
    if i >= content.len() { return content.len(); }

    // Walk forward through the run starting at `i`, tracking the byte
    // offset of its last character (not the byte just past it).
    let run_class = char_class(content[i..].chars().next().unwrap());
    let mut last = i;
    for (off, c) in content[i..].char_indices() {
        if char_class(c) != run_class { break; }
        last = i + off;
    }
    last
}

fn word_backward(content: &str, pos: usize) -> usize {
    /*
     * vim `b`: byte offset of the start of the current word (if the cursor
     * is mid-word) or of the previous word (if it's at a word's start
     * already).
     */
    if pos == 0 { return 0; }
    // Step back one char boundary first — vim's `b` always looks at the
    // word before the cursor, even if the cursor already sits on a word's
    // first character.
    let mut i = content[..pos].char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
    // Skip backward over any whitespace between the cursor and the
    // preceding word.
    loop {
        let c = content[i..].chars().next().unwrap();
        if !c.is_whitespace() { break; }
        if i == 0 { return 0; }
        i = content[..i].char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
    }
    // Walk backward while the previous char shares this run's class, to
    // find the start of the run `i` landed in.
    let class = char_class(content[i..].chars().next().unwrap());
    loop {
        if i == 0 { break; }
        let prev = content[..i].char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
        if char_class(content[prev..].chars().next().unwrap()) != class { break; }
        i = prev;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal AppState with one tab whose content, cursor, and
    /// selection are set to the given values. Avoids touching the filesystem
    /// or GPUI context.
    fn make_state(content: &str, cursor: usize, selection: Option<(usize, usize)>) -> AppState {
        let state = AppState {
            tabs: vec![Tab {
                id: 0,
                title: "test".into(),
                file_path: None,
                content: content.to_string(),
                is_modified: false,
                document: None,
                cursor,
                selection,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
                last_edit_at: None,
                vim_mode: VimMode::Normal,
                vim_command_buf: String::new(),
            }],
            active_tab: 0,
            next_tab_id: 1,
            sidebar_visible: false,
            settings_visible: false,
            working_directory: std::path::PathBuf::from("."),
            file_tree: vec![],
            vim_enabled: true,
        };
        state
    }

    // ── copy_selection ────────────────────────────────────────────────────────

    #[test]
    fn test_copy_selection_basic() {
        let state = make_state("hello world", 5, Some((0, 5)));
        assert_eq!(state.copy_selection(), Some("hello".to_string()));
    }

    #[test]
    fn test_copy_selection_backward() {
        // anchor > focus (reversed selection) — should still return correct text
        let state = make_state("hello world", 0, Some((5, 0)));
        assert_eq!(state.copy_selection(), Some("hello".to_string()));
    }

    #[test]
    fn test_copy_selection_no_selection() {
        let state = make_state("hello world", 0, None);
        assert_eq!(state.copy_selection(), None);
    }

    // ── cut_selection ─────────────────────────────────────────────────────────

    #[test]
    fn test_cut_selection_basic() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        let text = state.cut_selection();
        assert_eq!(text, Some("hello".to_string()));
        assert_eq!(state.tabs[0].content, " world");
        assert_eq!(state.tabs[0].cursor, 0);
        assert!(state.tabs[0].selection.is_none());
    }

    #[test]
    fn test_cut_selection_no_selection() {
        let mut state = make_state("hello world", 5, None);
        let text = state.cut_selection();
        assert_eq!(text, None);
        assert_eq!(state.tabs[0].content, "hello world"); // unchanged
    }

    // ── insert_str ────────────────────────────────────────────────────────────

    #[test]
    fn test_insert_str_no_selection() {
        let mut state = make_state("hello", 5, None);
        state.insert_str(" world");
        assert_eq!(state.tabs[0].content, "hello world");
        assert_eq!(state.tabs[0].cursor, 11);
    }

    #[test]
    fn test_insert_str_replaces_selection() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        state.insert_str("goodbye");
        assert_eq!(state.tabs[0].content, "goodbye world");
        assert_eq!(state.tabs[0].cursor, 7);
        assert!(state.tabs[0].selection.is_none());
    }

    #[test]
    fn test_insert_str_empty() {
        // Inserting an empty string is a no-op (no crash, content unchanged).
        let mut state = make_state("hello", 5, None);
        state.insert_str("");
        assert_eq!(state.tabs[0].content, "hello");
        assert_eq!(state.tabs[0].cursor, 5);
    }

    // ── move_left / move_right ──────────────────────────────────────────────

    #[test]
    fn test_move_right_advances_one_char() {
        let mut state = make_state("hello", 0, None);
        state.move_right();
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_move_right_stops_at_end() {
        let mut state = make_state("hi", 2, None);
        state.move_right();
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_move_right_skips_whole_multibyte_char() {
        // 'é' is 2 bytes in UTF-8; cursor must land on the next char boundary,
        // never inside the char.
        let mut state = make_state("café", 3, None);
        state.move_right();
        assert_eq!(state.tabs[0].cursor, 5);
        assert!(state.tabs[0].content.is_char_boundary(state.tabs[0].cursor));
    }

    #[test]
    fn test_move_left_retreats_one_char() {
        let mut state = make_state("hello", 3, None);
        state.move_left();
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_move_left_stops_at_start() {
        let mut state = make_state("hi", 0, None);
        state.move_left();
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_move_left_skips_whole_multibyte_char() {
        let mut state = make_state("café", 5, None);
        state.move_left();
        assert_eq!(state.tabs[0].cursor, 3);
        assert!(state.tabs[0].content.is_char_boundary(state.tabs[0].cursor));
    }

    // ── move_up / move_down ─────────────────────────────────────────────────

    #[test]
    fn test_move_down_same_column() {
        let mut state = make_state("abc\ndefgh", 1, None); // cursor after 'a'
        state.move_down();
        assert_eq!(state.tabs[0].cursor, 5); // "abc\nd|efgh" -> after 'd'
    }

    #[test]
    fn test_move_down_clamps_to_shorter_line() {
        let mut state = make_state("abcdef\nxy", 5, None); // cursor after "abcde"
        state.move_down();
        assert_eq!(state.tabs[0].cursor, 9); // end of "xy" (only 2 chars)
    }

    #[test]
    fn test_move_down_on_last_line_is_noop() {
        let mut state = make_state("abc\ndef", 5, None);
        state.move_down();
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_move_up_same_column() {
        let mut state = make_state("abc\ndefgh", 6, None); // cursor after "de"
        state.move_up();
        assert_eq!(state.tabs[0].cursor, 2); // "ab|c" -> after "ab"
    }

    #[test]
    fn test_move_up_clamps_to_shorter_line() {
        let mut state = make_state("xy\nabcdef", 8, None); // cursor after "abcde"
        state.move_up();
        assert_eq!(state.tabs[0].cursor, 2); // end of "xy"
    }

    #[test]
    fn test_move_up_on_first_line_is_noop() {
        let mut state = make_state("abc\ndef", 2, None);
        state.move_up();
        assert_eq!(state.tabs[0].cursor, 2);
    }

    // ── move_line_start / move_line_first_nonblank / move_line_end ─────────

    #[test]
    fn test_move_line_start() {
        let mut state = make_state("abc\n  defgh", 9, None); // cursor inside "defgh"
        state.move_line_start();
        assert_eq!(state.tabs[0].cursor, 4); // start of second line
    }

    #[test]
    fn test_move_line_first_nonblank_skips_leading_whitespace() {
        let mut state = make_state("abc\n  defgh", 9, None);
        state.move_line_first_nonblank();
        assert_eq!(state.tabs[0].cursor, 6); // 'd' in "  defgh"
    }

    #[test]
    fn test_move_line_first_nonblank_all_whitespace_line_lands_at_end() {
        let mut state = make_state("abc\n   \ndef", 5, None); // middle line is all spaces
        state.move_line_first_nonblank();
        assert_eq!(state.tabs[0].cursor, 7); // end of the blank line, no non-blank found
    }

    #[test]
    fn test_move_line_end() {
        let mut state = make_state("abc\ndefgh\nij", 5, None); // cursor inside "defgh"
        state.move_line_end();
        assert_eq!(state.tabs[0].cursor, 9); // just before the '\n'
    }

    #[test]
    fn test_move_line_end_last_line() {
        let mut state = make_state("abc\ndef", 5, None);
        state.move_line_end();
        assert_eq!(state.tabs[0].cursor, 7); // end of content, no trailing '\n'
    }

    // ── move_word_forward / move_word_end / move_word_backward ─────────────

    #[test]
    fn test_move_word_forward_skips_to_next_word() {
        let mut state = make_state("hello world", 0, None);
        state.move_word_forward();
        assert_eq!(state.tabs[0].cursor, 6); // start of "world"
    }

    #[test]
    fn test_move_word_forward_stops_at_punctuation_boundary() {
        let mut state = make_state("foo.bar baz", 0, None);
        state.move_word_forward();
        assert_eq!(state.tabs[0].cursor, 3); // start of "." (punctuation is its own word)
    }

    #[test]
    fn test_move_word_forward_crosses_newline() {
        let mut state = make_state("foo\nbar", 0, None);
        state.move_word_forward();
        assert_eq!(state.tabs[0].cursor, 4); // start of "bar" on next line
    }

    #[test]
    fn test_move_word_forward_at_last_word_goes_to_end() {
        let mut state = make_state("hello", 0, None);
        state.move_word_forward();
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_move_word_end_lands_on_last_char_of_word() {
        let mut state = make_state("hello world", 0, None);
        state.move_word_end();
        assert_eq!(state.tabs[0].cursor, 4); // last char of "hello" ('o')
    }

    #[test]
    fn test_move_word_end_from_inside_word_goes_to_its_end() {
        let mut state = make_state("hello world", 2, None); // cursor on 'l'
        state.move_word_end();
        assert_eq!(state.tabs[0].cursor, 4);
    }

    #[test]
    fn test_move_word_end_at_last_char_advances_to_next_word_end() {
        let mut state = make_state("hello world", 4, None); // cursor already at 'o'
        state.move_word_end();
        assert_eq!(state.tabs[0].cursor, 10); // last char of "world" ('d')
    }

    #[test]
    fn test_move_word_backward_to_word_start() {
        let mut state = make_state("hello world", 11, None); // cursor at end
        state.move_word_backward();
        assert_eq!(state.tabs[0].cursor, 6); // start of "world"
    }

    #[test]
    fn test_move_word_backward_from_inside_word_goes_to_its_start() {
        let mut state = make_state("hello world", 8, None); // cursor on 'r'
        state.move_word_backward();
        assert_eq!(state.tabs[0].cursor, 6);
    }

    #[test]
    fn test_move_word_backward_at_start_is_noop() {
        let mut state = make_state("hello", 0, None);
        state.move_word_backward();
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── move_doc_start / move_doc_end / move_to_line ───────────────────────

    #[test]
    fn test_move_doc_start() {
        let mut state = make_state("abc\ndef\nghi", 9, None);
        state.move_doc_start();
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_move_doc_end() {
        let mut state = make_state("abc\ndef\nghi", 0, None);
        state.move_doc_end();
        assert_eq!(state.tabs[0].cursor, 11);
    }

    #[test]
    fn test_move_to_line_one_indexed() {
        let mut state = make_state("abc\ndef\nghi", 0, None);
        state.move_to_line(2);
        assert_eq!(state.tabs[0].cursor, 4); // start of "def"
    }

    #[test]
    fn test_move_to_line_clamps_past_last_line() {
        let mut state = make_state("abc\ndef", 0, None);
        state.move_to_line(99);
        assert_eq!(state.tabs[0].cursor, 4); // start of last line
    }

    #[test]
    fn test_move_to_line_zero_clamps_to_first_line() {
        let mut state = make_state("abc\ndef", 5, None);
        state.move_to_line(0);
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── cursor_line_col ──────────────────────────────────────────────────

    #[test]
    fn test_cursor_line_col_start_of_document() {
        let state = make_state("hello\nworld", 0, None);
        assert_eq!(state.cursor_line_col(), (0, 0));
    }

    #[test]
    fn test_cursor_line_col_end_of_first_line() {
        let state = make_state("hello\nworld", 5, None);
        assert_eq!(state.cursor_line_col(), (0, 5));
    }

    #[test]
    fn test_cursor_line_col_start_of_second_line() {
        let state = make_state("hello\nworld", 6, None);
        assert_eq!(state.cursor_line_col(), (1, 0));
    }

    #[test]
    fn test_cursor_line_col_end_of_document() {
        let state = make_state("hello\nworld", 11, None);
        assert_eq!(state.cursor_line_col(), (1, 5));
    }

    #[test]
    fn test_cursor_line_col_counts_chars_not_bytes() {
        // "café" is 4 characters but 5 bytes ('é' is 2 bytes in UTF-8).
        let state = make_state("café\nx", 5, None);
        assert_eq!(state.cursor_line_col(), (0, 4));
    }

    // ── set_cursor_from_line_col ────────────────────────────────────────────

    #[test]
    fn test_set_cursor_from_line_col_basic() {
        let mut state = make_state("abc\ndefgh", 0, None);
        state.set_cursor_from_line_col(1, 2);
        assert_eq!(state.tabs[0].cursor, 6); // "abc\nde|fgh"
    }

    #[test]
    fn test_set_cursor_from_line_col_clamps_column_past_line_end() {
        let mut state = make_state("ab\ndefgh", 0, None);
        state.set_cursor_from_line_col(0, 99);
        assert_eq!(state.tabs[0].cursor, 2); // end of "ab"
    }

    #[test]
    fn test_set_cursor_from_line_col_clamps_line_past_last() {
        let mut state = make_state("abc\ndef", 0, None);
        state.set_cursor_from_line_col(99, 0);
        assert_eq!(state.tabs[0].cursor, 4); // start of last line
    }

    #[test]
    fn test_set_cursor_from_line_col_clears_selection() {
        let mut state = make_state("abc\ndefgh", 0, Some((0, 3)));
        state.set_cursor_from_line_col(0, 1);
        assert!(state.tabs[0].selection.is_none());
    }

    // round-trip against cursor_line_col confirms the two stay inverse of
    // each other, since click-positioning depends on that symmetry.
    #[test]
    fn test_set_cursor_from_line_col_round_trips_with_cursor_line_col() {
        let mut state = make_state("hello\nworld", 0, None);
        state.set_cursor_from_line_col(1, 3);
        assert_eq!(state.cursor_line_col(), (1, 3));
    }

    // ── extend_left / extend_right ──────────────────────────────────────────

    #[test]
    fn test_extend_right_creates_selection_from_current_cursor() {
        let mut state = make_state("hello", 0, None);
        state.extend_right();
        assert_eq!(state.tabs[0].selection, Some((0, 1)));
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_extend_right_twice_keeps_original_anchor() {
        let mut state = make_state("hello", 0, None);
        state.extend_right();
        state.extend_right();
        assert_eq!(state.tabs[0].selection, Some((0, 2)));
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_extend_left_keeps_anchor_when_selection_already_exists() {
        // Simulate having extended right first, then reversing direction.
        let mut state = make_state("hello", 2, Some((0, 2)));
        state.extend_left();
        assert_eq!(state.tabs[0].selection, Some((0, 1)));
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_extend_left_and_right_back_to_anchor_is_zero_width_not_none() {
        let mut state = make_state("hello", 0, None);
        state.extend_right();
        state.extend_left();
        assert_eq!(state.tabs[0].selection, Some((0, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_extend_left_clamps_at_document_start() {
        let mut state = make_state("hello", 0, None);
        state.extend_left();
        assert_eq!(state.tabs[0].selection, Some((0, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── extend_up / extend_down ─────────────────────────────────────────────

    #[test]
    fn test_extend_down_creates_selection() {
        let mut state = make_state("abc\ndefgh", 1, None);
        state.extend_down();
        assert_eq!(state.tabs[0].selection, Some((1, 5)));
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_extend_up_creates_selection() {
        let mut state = make_state("abc\ndefgh", 6, None);
        state.extend_up();
        assert_eq!(state.tabs[0].selection, Some((6, 2)));
        assert_eq!(state.tabs[0].cursor, 2);
    }

    // ── extend_word_forward / extend_word_backward ──────────────────────────

    #[test]
    fn test_extend_word_forward_creates_selection() {
        let mut state = make_state("hello world", 0, None);
        state.extend_word_forward();
        assert_eq!(state.tabs[0].selection, Some((0, 6)));
        assert_eq!(state.tabs[0].cursor, 6);
    }

    #[test]
    fn test_extend_word_backward_creates_selection() {
        let mut state = make_state("hello world", 11, None);
        state.extend_word_backward();
        assert_eq!(state.tabs[0].selection, Some((11, 6)));
        assert_eq!(state.tabs[0].cursor, 6);
    }

    // ── extend_line_start / extend_line_end ─────────────────────────────────

    #[test]
    fn test_extend_line_start_creates_selection() {
        let mut state = make_state("abc\n  defgh", 9, None);
        state.extend_line_start();
        assert_eq!(state.tabs[0].selection, Some((9, 4)));
        assert_eq!(state.tabs[0].cursor, 4);
    }

    #[test]
    fn test_extend_line_end_creates_selection() {
        let mut state = make_state("abc\ndefgh\nij", 5, None);
        state.extend_line_end();
        assert_eq!(state.tabs[0].selection, Some((5, 9)));
        assert_eq!(state.tabs[0].cursor, 9);
    }

    // ── extend_doc_start / extend_doc_end ───────────────────────────────────

    #[test]
    fn test_extend_doc_start_creates_selection() {
        let mut state = make_state("abc\ndef\nghi", 9, None);
        state.extend_doc_start();
        assert_eq!(state.tabs[0].selection, Some((9, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_extend_doc_end_creates_selection() {
        let mut state = make_state("abc\ndef\nghi", 0, None);
        state.extend_doc_end();
        assert_eq!(state.tabs[0].selection, Some((0, 11)));
        assert_eq!(state.tabs[0].cursor, 11);
    }

    // ── select_all ───────────────────────────────────────────────────────────

    #[test]
    fn test_select_all() {
        let mut state = make_state("hello\nworld", 3, None);
        state.select_all();
        assert_eq!(state.tabs[0].selection, Some((0, 11)));
        assert_eq!(state.tabs[0].cursor, 11);
    }

    #[test]
    fn test_select_all_empty_document() {
        let mut state = make_state("", 0, None);
        state.select_all();
        assert_eq!(state.tabs[0].selection, Some((0, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── extend_selection_to_line_col (click-drag) ───────────────────────────

    #[test]
    fn test_extend_selection_to_line_col_creates_selection_from_cursor() {
        let mut state = make_state("abc\ndefgh", 1, None);
        state.extend_selection_to_line_col(1, 2);
        assert_eq!(state.tabs[0].selection, Some((1, 6))); // anchor = old cursor
        assert_eq!(state.tabs[0].cursor, 6); // line 1, col 2 -> "de|fgh"
    }

    #[test]
    fn test_extend_selection_to_line_col_keeps_existing_anchor() {
        // Simulates a drag already in progress: selection exists, anchor
        // must not move even as the drag continues past it in either direction.
        let mut state = make_state("abc\ndefgh", 6, Some((1, 6)));
        state.extend_selection_to_line_col(0, 0);
        assert_eq!(state.tabs[0].selection, Some((1, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_extend_selection_to_line_col_clamps_out_of_range_line_and_col() {
        let mut state = make_state("abc\ndef", 0, None);
        state.extend_selection_to_line_col(99, 99);
        assert_eq!(state.tabs[0].selection, Some((0, 7))); // clamps to end of doc
        assert_eq!(state.tabs[0].cursor, 7);
    }

    #[test]
    fn test_extend_selection_to_line_col_same_position_is_zero_width_not_none() {
        let mut state = make_state("abc\ndef", 0, None);
        state.extend_selection_to_line_col(0, 0);
        assert_eq!(state.tabs[0].selection, Some((0, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── clamp_to_char_boundary ───────────────────────────────────────────────

    #[test]
    fn test_clamp_to_char_boundary_already_valid_is_unchanged() {
        assert_eq!(clamp_to_char_boundary("hello", 3), 3);
    }

    #[test]
    fn test_clamp_to_char_boundary_past_end_clamps_to_len() {
        assert_eq!(clamp_to_char_boundary("hi", 99), 2);
    }

    #[test]
    fn test_clamp_to_char_boundary_mid_multibyte_char_walks_back() {
        // "café" — 'é' is 2 bytes, spanning byte offsets 3..5. Offset 4 sits
        // inside it and must walk back to 3, the char's own start.
        assert_eq!(clamp_to_char_boundary("café", 4), 3);
    }

    #[test]
    fn test_clamp_to_char_boundary_zero_is_always_valid() {
        assert_eq!(clamp_to_char_boundary("", 0), 0);
    }

    // ── undo / redo ──────────────────────────────────────────────────────────

    /// Rewinds the active tab's `last_edit_at` far enough into the past that
    /// the next edit's `push_undo_snapshot` call will not coalesce with it —
    /// lets tests control coalescing deterministically without sleeping.
    fn break_coalesce_window(state: &mut AppState) {
        if let Some(tab) = state.tabs.get_mut(state.active_tab) {
            tab.last_edit_at = Some(Instant::now() - UNDO_COALESCE_WINDOW - Duration::from_millis(1));
        }
    }

    #[test]
    fn test_insert_char_pushes_undo_snapshot() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        assert_eq!(state.tabs[0].undo_stack, vec!["ab".to_string()]);
    }

    #[test]
    fn test_rapid_inserts_coalesce_into_one_undo_step() {
        // Two inserts with no time passing between them (the normal case for
        // fast typing) must land as ONE undo step, not two.
        let mut state = make_state("a", 1, None);
        state.insert_char('b');
        state.insert_char('c');
        assert_eq!(state.tabs[0].undo_stack, vec!["a".to_string()]);
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_inserts_outside_coalesce_window_are_separate_undo_steps() {
        let mut state = make_state("a", 1, None);
        state.insert_char('b');
        break_coalesce_window(&mut state);
        state.insert_char('c');
        assert_eq!(state.tabs[0].undo_stack, vec!["a".to_string(), "ab".to_string()]);
    }

    #[test]
    fn test_undo_restores_previous_content() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        assert_eq!(state.tabs[0].content, "abc");
        state.undo();
        assert_eq!(state.tabs[0].content, "ab");
    }

    #[test]
    fn test_undo_clears_selection_and_marks_modified() {
        let mut state = make_state("ab", 2, Some((0, 1)));
        state.tabs[0].undo_stack.push("ab".to_string());
        state.undo();
        assert!(state.tabs[0].selection.is_none());
        assert!(state.tabs[0].is_modified);
    }

    #[test]
    fn test_undo_clamps_cursor_into_shorter_restored_content() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c'); // content = "abc", cursor = 3
        state.undo();
        // Restored content is "ab" (len 2); cursor must not remain at 3.
        assert_eq!(state.tabs[0].content, "ab");
        assert!(state.tabs[0].cursor <= state.tabs[0].content.len());
        assert!(state.tabs[0].content.is_char_boundary(state.tabs[0].cursor));
    }

    #[test]
    fn test_undo_with_empty_stack_is_noop() {
        let mut state = make_state("abc", 3, None);
        state.undo();
        assert_eq!(state.tabs[0].content, "abc");
        assert_eq!(state.tabs[0].cursor, 3);
    }

    #[test]
    fn test_undo_pushes_onto_redo_stack() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        state.undo();
        assert_eq!(state.tabs[0].redo_stack, vec!["abc".to_string()]);
    }

    #[test]
    fn test_redo_restores_undone_content() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        state.undo();
        assert_eq!(state.tabs[0].content, "ab");
        state.redo();
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_redo_with_empty_stack_is_noop() {
        let mut state = make_state("abc", 3, None);
        state.redo();
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_new_edit_after_undo_clears_redo_stack() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        state.undo();
        assert!(!state.tabs[0].redo_stack.is_empty());
        break_coalesce_window(&mut state);
        state.insert_char('d');
        assert!(state.tabs[0].redo_stack.is_empty());
    }

    #[test]
    fn test_undo_stack_capped_at_200() {
        let mut state = make_state("", 0, None);
        for _ in 0..250 {
            state.insert_char('x');
            break_coalesce_window(&mut state); // force every insert onto its own step
        }
        assert_eq!(state.tabs[0].undo_stack.len(), 200);
    }

    #[test]
    fn test_backspace_pushes_undo_snapshot() {
        let mut state = make_state("abc", 3, None);
        state.backspace();
        assert_eq!(state.tabs[0].undo_stack, vec!["abc".to_string()]);
    }

    #[test]
    fn test_backspace_noop_at_document_start_does_not_push_undo() {
        let mut state = make_state("abc", 0, None);
        state.backspace();
        assert!(state.tabs[0].undo_stack.is_empty());
    }

    #[test]
    fn test_backspace_over_selection_pushes_one_undo_step() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        state.backspace();
        assert_eq!(state.tabs[0].undo_stack, vec!["hello world".to_string()]);
    }

    #[test]
    fn test_delete_selection_pushes_undo_snapshot() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        state.delete_selection();
        assert_eq!(state.tabs[0].undo_stack, vec!["hello world".to_string()]);
    }

    #[test]
    fn test_delete_selection_noop_does_not_push_undo() {
        let mut state = make_state("hello world", 5, None);
        state.delete_selection();
        assert!(state.tabs[0].undo_stack.is_empty());
    }

    #[test]
    fn test_insert_str_pushes_undo_snapshot() {
        let mut state = make_state("hello", 5, None);
        state.insert_str(" world");
        assert_eq!(state.tabs[0].undo_stack, vec!["hello".to_string()]);
    }

    #[test]
    fn test_insert_str_empty_does_not_push_undo() {
        let mut state = make_state("hello", 5, None);
        state.insert_str("");
        assert!(state.tabs[0].undo_stack.is_empty());
    }

    #[test]
    fn test_insert_str_replacing_selection_pushes_one_undo_step() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        state.insert_str("goodbye");
        assert_eq!(state.tabs[0].undo_stack, vec!["hello world".to_string()]);
    }

    // ── vim mode-entry transitions (Task D) ─────────────────────────────────────

    #[test]
    fn test_vim_enter_insert_before_cursor_sets_mode_and_preserves_cursor() {
        let mut state = make_state("hello", 2, None);
        state.vim_enter_insert_before_cursor();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_vim_enter_insert_before_cursor_clears_selection() {
        let mut state = make_state("hello", 2, Some((0, 2)));
        state.vim_enter_insert_before_cursor();
        assert_eq!(state.tabs[0].selection, None);
    }

    #[test]
    fn test_vim_enter_insert_line_start_moves_to_first_nonblank() {
        let mut state = make_state("  hello", 5, None);
        state.vim_enter_insert_line_start();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_vim_enter_insert_after_cursor_moves_right() {
        let mut state = make_state("hello", 0, None);
        state.vim_enter_insert_after_cursor();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_vim_enter_insert_after_cursor_clamps_at_document_end() {
        let mut state = make_state("hi", 2, None);
        state.vim_enter_insert_after_cursor();
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_vim_enter_insert_line_end_moves_to_line_end() {
        let mut state = make_state("hello\nworld", 0, None);
        state.vim_enter_insert_line_end();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        assert_eq!(state.tabs[0].cursor, 5); // byte offset of the '\n'
    }

    #[test]
    fn test_vim_open_line_below_creates_new_line_and_places_cursor_on_it() {
        let mut state = make_state("hello", 2, None);
        state.vim_open_line_below();
        assert_eq!(state.tabs[0].content, "hello\n");
        assert_eq!(state.tabs[0].cursor, 6);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_vim_open_line_below_pushes_undo_snapshot() {
        let mut state = make_state("hello", 2, None);
        state.vim_open_line_below();
        assert_eq!(state.tabs[0].undo_stack, vec!["hello".to_string()]);
    }

    #[test]
    fn test_vim_open_line_below_on_last_line_of_multiline_doc() {
        let mut state = make_state("first\nsecond", 8, None);
        state.vim_open_line_below();
        assert_eq!(state.tabs[0].content, "first\nsecond\n");
        assert_eq!(state.tabs[0].cursor, 13);
    }

    #[test]
    fn test_vim_open_line_below_on_empty_document() {
        let mut state = make_state("", 0, None);
        state.vim_open_line_below();
        assert_eq!(state.tabs[0].content, "\n");
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_vim_open_line_above_inserts_before_current_line() {
        let mut state = make_state("hello", 2, None);
        state.vim_open_line_above();
        assert_eq!(state.tabs[0].content, "\nhello");
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_vim_open_line_above_pushes_undo_snapshot() {
        let mut state = make_state("hello", 2, None);
        state.vim_open_line_above();
        assert_eq!(state.tabs[0].undo_stack, vec!["hello".to_string()]);
    }

    #[test]
    fn test_vim_open_line_above_on_second_line() {
        let mut state = make_state("first\nsecond", 8, None);
        state.vim_open_line_above();
        assert_eq!(state.tabs[0].content, "first\n\nsecond");
        assert_eq!(state.tabs[0].cursor, 6);
    }

    #[test]
    fn test_vim_open_line_above_on_empty_document() {
        let mut state = make_state("", 0, None);
        state.vim_open_line_above();
        assert_eq!(state.tabs[0].content, "\n");
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_vim_enter_visual_selects_char_under_cursor() {
        let mut state = make_state("hello", 1, None);
        state.vim_enter_visual();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual);
        assert_eq!(state.tabs[0].selection, Some((1, 2)));
    }

    #[test]
    fn test_vim_enter_visual_at_document_end_zero_width_selection() {
        let mut state = make_state("hi", 2, None);
        state.vim_enter_visual();
        assert_eq!(state.tabs[0].selection, Some((2, 2)));
    }

    #[test]
    fn test_vim_enter_visual_line_selects_whole_line_including_newline() {
        let mut state = make_state("first\nsecond", 2, None); // on "first"
        state.vim_enter_visual_line();
        assert_eq!(state.tabs[0].vim_mode, VimMode::VisualLine);
        assert_eq!(state.tabs[0].selection, Some((0, 6))); // "first\n"
    }

    #[test]
    fn test_vim_enter_visual_line_on_last_line_no_trailing_newline() {
        let mut state = make_state("first\nsecond", 8, None);
        state.tabs[0].cursor = 8; // on "second"
        state.vim_enter_visual_line();
        // "second" is the last line and has no trailing '\n' to include.
        assert_eq!(state.tabs[0].selection, Some((6, 12)));
    }

    #[test]
    fn test_vim_enter_command_sets_mode() {
        let mut state = make_state("hello", 2, None);
        state.vim_enter_command();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Command);
    }

    #[test]
    fn test_vim_exit_to_normal_clears_selection_and_mode() {
        let mut state = make_state("hello", 2, Some((0, 2)));
        state.tabs[0].vim_mode = VimMode::Visual;
        state.vim_exit_to_normal();
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].selection, None);
    }

    // ── handle_vim_key dispatch (Task D) ─────────────────────────────────────────

    #[test]
    fn test_handle_vim_key_normal_i_enters_insert() {
        let mut state = make_state("hello", 0, None);
        let handled = state.handle_vim_key("i", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_handle_vim_key_normal_colon_via_shift_semicolon_enters_command() {
        let mut state = make_state("hello", 0, None);
        let handled = state.handle_vim_key(";", true, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Command);
    }

    #[test]
    fn test_handle_vim_key_normal_colon_via_key_char_enters_command() {
        // Covers the case where GPUI reports the shifted character directly
        // via key_char instead of (or in addition to) the base key + shift.
        let mut state = make_state("hello", 0, None);
        let handled = state.handle_vim_key(";", false, Some(":"));
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Command);
    }

    #[test]
    fn test_handle_vim_key_normal_navigation_falls_through() {
        let mut state = make_state("hello", 2, None);
        let handled = state.handle_vim_key("left", false, None);
        assert!(!handled);
        // handle_vim_key itself must not move the cursor when it declines
        // to consume the key — the caller applies the plain-editor movement.
        assert_eq!(state.tabs[0].cursor, 2);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_handle_vim_key_normal_unmapped_printable_is_swallowed() {
        let mut state = make_state("hello", 2, None);
        let handled = state.handle_vim_key("q", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].content, "hello"); // not inserted as text
    }

    #[test]
    fn test_handle_vim_key_insert_mode_returns_false() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Insert;
        let handled = state.handle_vim_key("x", false, None);
        assert!(!handled);
    }

    #[test]
    fn test_handle_vim_key_visual_escape_exits_to_normal() {
        let mut state = make_state("hello", 2, Some((2, 3)));
        state.tabs[0].vim_mode = VimMode::Visual;
        let handled = state.handle_vim_key("escape", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].selection, None);
    }

    #[test]
    fn test_handle_vim_key_visual_v_exits_to_normal() {
        let mut state = make_state("hello", 2, Some((2, 3)));
        state.tabs[0].vim_mode = VimMode::Visual;
        let handled = state.handle_vim_key("v", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_handle_vim_key_visual_shift_v_is_swallowed_without_mode_change() {
        // Switching Visual -> VisualLine on shift-V isn't in spec 5.1's
        // table and is out of scope for Task D; it should be swallowed,
        // not fall through to text insertion, but also not change mode.
        let mut state = make_state("hello", 2, Some((2, 3)));
        state.tabs[0].vim_mode = VimMode::Visual;
        let handled = state.handle_vim_key("v", true, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual);
    }

    #[test]
    fn test_handle_vim_key_visual_line_shift_v_exits_to_normal() {
        let mut state = make_state("hello", 2, Some((0, 5)));
        state.tabs[0].vim_mode = VimMode::VisualLine;
        let handled = state.handle_vim_key("v", true, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_handle_vim_key_visual_line_plain_v_is_noop() {
        let mut state = make_state("hello", 2, Some((0, 5)));
        state.tabs[0].vim_mode = VimMode::VisualLine;
        let handled = state.handle_vim_key("v", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::VisualLine);
    }

    #[test]
    fn test_handle_vim_key_command_escape_exits_to_normal() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        let handled = state.handle_vim_key("escape", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_handle_vim_key_command_enter_exits_to_normal() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        let handled = state.handle_vim_key("enter", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_handle_vim_key_command_other_key_is_swallowed_no_mode_change() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        let handled = state.handle_vim_key("x", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Command);
        assert_eq!(state.tabs[0].content, "hello"); // not inserted as text
    }
}
