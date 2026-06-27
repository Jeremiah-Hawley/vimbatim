use std::path::PathBuf;

/// A single editor tab, representing either an unsaved "new" tab or an opened .docx file.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: usize,
    pub title: String,
    pub file_path: Option<PathBuf>,
    /// Plain-text content for this tab. Will be extended to parse/write .docx in the future.
    pub content: String,
    pub is_modified: bool,
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
        }
    }

    pub fn from_path(id: usize, path: PathBuf) -> Self {
        /*
         * Creates a Tab associated with an existing file path. The tab title is
         * set to the file name. Content is left empty — loading from disk is handled
         * separately so the tab can be created before the file is read.
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
         * Opens a file in a new tab. If the file is already open in an existing tab,
         * that tab is focused instead of creating a duplicate. Otherwise, a new tab
         * is appended for the given path. Content loading from disk is left for a
         * future implementation phase.
         */
        // If already open, switch to it
        if let Some(idx) = self.tabs.iter().position(|t| t.file_path.as_deref() == Some(&path)) {
            self.active_tab = idx;
            return;
        }
        let tab = Tab::from_path(self.next_tab_id, path);
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, idx: usize) {
        /*
         * Removes the tab at the given index. Always keeps at least one tab open.
         * Adjusts the active_tab index to remain valid after removal.
         */
        if self.tabs.len() <= 1 {
            return; // always keep at least one tab
        }
        self.tabs.remove(idx);
        // clamp active tab to valid range
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
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
         * Appends a character to the content of the currently active tab and
         * marks that tab as modified. This is the primary path for typed text input.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.content.push(ch);
            tab.is_modified = true;
        }
    }

    pub fn backspace(&mut self) {
        /*
         * Removes the last character from the active tab's content (i.e., basic
         * backspace behaviour). Marks the tab as modified.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.content.pop();
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
