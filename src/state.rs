use std::path::PathBuf;
use std::sync::Arc;

use crate::docx_parser::{DocxDocument, create_new_docx, parse_docx};

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

    pub fn insert_char(&mut self, ch: char) {
        /*
         * Inserts a character at the cursor position and advances the cursor.
         * If a selection is active it is deleted first, mirroring the behaviour
         * a user expects when typing over highlighted text.
         */
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection();
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
         * start of the deleted range.
         */
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection();
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if tab.cursor == 0 { return; }
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
         * Drains the byte range covered by the active selection from `content`
         * and repositions the cursor at the range start. Clears the selection.
         * No-op when `selection` is `None`.
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
         * paste produces.
         */
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection();
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.content.insert_str(tab.cursor, text);
            tab.cursor += text.len(); // text is valid UTF-8 so len() == byte count
            tab.is_modified = true;
        }
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
            }],
            active_tab: 0,
            next_tab_id: 1,
            sidebar_visible: false,
            settings_visible: false,
            working_directory: std::path::PathBuf::from("."),
            file_tree: vec![],
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
}
