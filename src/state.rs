use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::case_converter;
use crate::color_picker;
use crate::docx_parser::{Alignment, DocxOrigin, Paragraph, Run, create_new_docx, paragraphs_to_plain_text, parse_docx};
use crate::document_ops::{apply_formatting, apply_paragraph_alignment, is_uniformly_active, sync_delete_range, sync_insert_char, sync_insert_str, toggled_off, FormatOp};
use crate::wikifi_export;

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
    /// `R` (spec 5.5): typing overwrites characters in place instead of
    /// inserting. Not in editor_instructions.md's mode table (a
    /// documented, task-vim_todo.md-flagged spec gap) — added as a real
    /// mode rather than treating `R` as out of scope, per user decision.
    Replace,
    /// `/` or `?` (spec 5.5): typing a search pattern, dispatched on
    /// `Enter`. Reuses the same text-capture buffer/machinery as
    /// `Command` (the two are mutually exclusive per tab).
    Search,
}

/// Outcome of one keystroke fed to `capture_vim_line_input`, the text
/// entry state machine shared by Command and Search mode.
enum VimLineInput {
    /// The keystroke was captured (a character appended, or a backspace
    /// that still left text); no further action needed this keystroke.
    Consumed,
    /// `Enter` was pressed; the accumulated (and already-cleared) line
    /// text is ready for the caller's mode-specific dispatch.
    Dispatch(String),
    /// `Escape`, or `Backspace` on an already-empty buffer; the caller
    /// should return to Normal mode without dispatching anything.
    Cancelled,
}

/// How a resolved motion's target combines with the cursor to form a
/// range — the piece a bare `target: usize` loses, and the reason Task F's
/// operators (`d`/`y`/`c`) can't just reuse `handle_vim_motion_key`'s
/// existing `usize` output: `dw` and `de` from the same cursor position
/// must produce different ranges even though both are "move forward,"
/// which only `MotionKind` can distinguish (spec 5.3/5.2; vim's own
/// `:help exclusive`/`:help inclusive`/`:help linewise`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MotionKind {
    /// `dw`-style: the range is `[min(cursor, target), max(cursor, target))`
    /// — the target position itself is excluded.
    ExclusiveChar,
    /// `de`/`d$`-style: the range is
    /// `[min(cursor, target), max(cursor, target) + 1)` (clamped to
    /// content length) — the character *at* the target is included.
    InclusiveChar,
    /// `dd`/`dgg`/`d_`-style: the range spans whole lines, from the start
    /// of `min(cursor, target)`'s line through the end of
    /// `max(cursor, target)`'s line (newline included, so the line is
    /// fully removed rather than left blank).
    Linewise,
}

/// The outcome of resolving one keystroke against the shared motion state
/// machine — `resolve_vim_motion`'s return type. Separated from actually
/// moving the cursor/extending a selection/feeding an operator so all
/// three consumers share one motion table instead of duplicating it.
#[derive(Debug, PartialEq)]
enum MotionResolution {
    /// `key` isn't part of the shared motion system at all (mode-switch
    /// keys, or genuinely unmapped) — the caller decides what to do.
    NotAMotion,
    /// Consumed as bookkeeping only — a `[count]` digit, the first key of
    /// a two-keystroke command, or a pending two-keystroke command that
    /// keystroke abandoned rather than completed. No target to move to.
    Pending,
    /// `key` needs GPUI viewport context this method doesn't have
    /// (`up`/`down`/`j`/`k` always; `left`/`right`/`home`/`end` only via
    /// `handle_vim_motion_key`'s own Normal-mode fallthrough convenience —
    /// `resolve_vim_motion` itself always resolves the latter four
    /// locally, see its doc comment).
    NeedsGpui,
    /// A motion fully resolved to a target byte offset and its kind.
    Resolved { target: usize, kind: MotionKind },
}

/// A single editor tab, representing either an unsaved "new" tab or an opened .docx file.
#[derive(Clone, Debug)]
pub struct Tab {
    pub id: usize,
    pub title: String,
    pub file_path: Option<PathBuf>,
    pub content: String,
    pub is_modified: bool,
    /// The tab's live, editable formatted content (rich-text formatting
    /// plan, Phase 1) — always has at least one paragraph with one run,
    /// even for a brand-new tab with no file. Kept in sync with `content`
    /// by every content-mutation function once Phase 1 Task 4 lands;
    /// until then this mirrors `content` only at load time.
    pub paragraphs: Vec<Paragraph>,
    /// Save-time constants (original ZIP bytes, XML preamble/sectPr) needed
    /// to write `paragraphs` back out as a real .docx. `None` for brand-new
    /// tabs that have never been associated with a real docx file, or for
    /// files that failed to parse — `create_new_docx` handles that case at
    /// save time instead. Immutable for the tab's lifetime, so still cheap
    /// to share via `Arc` (see `DocxOrigin`'s own doc comment for why this
    /// is no longer bundled with `paragraphs` the way the old
    /// `DocxDocument` was).
    pub docx_origin: Option<Arc<DocxOrigin>>,
    /// Copied from `DocxOrigin.has_unsupported_blocks` in `open_file` (or
    /// `false` for a brand-new tab with no source file) so `text_editor.rs`'s
    /// render path can check it directly without unwrapping
    /// `Option<Arc<DocxOrigin>>` on every frame.
    pub has_unsupported_blocks: bool,
    /// True once the user has dismissed the "this document has content we
    /// can't preserve" banner for this tab. View-level UI state, same as
    /// every other per-tab boolean already in this struct.
    pub unsupported_banner_dismissed: bool,
    /// A formatting toggle (spec 7) armed with no active selection, per
    /// spec 7's own intro: "or (if no selection) toggles the property for
    /// subsequent typing". Consumed by `insert_char`, which applies it to
    /// each newly-typed character — persists across multiple keystrokes
    /// until the same action is triggered again (an explicit toggle-off),
    /// not just for one character. A single slot (not a set): arming a
    /// different op while one is already pending replaces it, a documented
    /// simplification — real Word can have several pending toggles at
    /// once (bold *and* italic), this can only have one.
    pub pending_format: Option<FormatOp>,
    /// Byte offset into `content` where the cursor currently sits.
    /// Always points to a valid UTF-8 char boundary.
    pub cursor: usize,
    /// Active text selection as (anchor, focus) byte offsets.
    /// Anchor is where the selection started; focus tracks the cursor.
    /// Normalise to (min, max) before any range operation. `None` means no selection.
    pub selection: Option<(usize, usize)>,
    /// Snapshots of `(content, paragraphs)` taken before each edit, most
    /// recent last. `undo()` pops from here onto `redo_stack`. Capped at
    /// UNDO_STACK_CAP. Paired together (rich-text formatting plan, Phase 1)
    /// so undo can't restore old text while leaving stale/shifted-wrong
    /// formatting attached to it.
    pub undo_stack: Vec<(String, Vec<Paragraph>)>,
    /// Snapshots of `(content, paragraphs)` that `undo()` has moved past,
    /// most recent last. `redo()` pops from here back onto `undo_stack`.
    /// Cleared whenever a new edit is made, since it invalidates that
    /// history.
    pub redo_stack: Vec<(String, Vec<Paragraph>)>,
    /// When the most recent undo-stack push happened, used to coalesce a
    /// burst of rapid edits (e.g. typing) into a single undo step rather
    /// than one per keystroke. `None` means no edit has been made yet, or
    /// the coalescing window was deliberately broken (e.g. by an undo/redo).
    pub last_edit_at: Option<Instant>,
    /// The tab's current vim mode. Only meaningful when `AppState.vim_enabled`
    /// is true; unused otherwise.
    pub vim_mode: VimMode,
    /// Normal-mode command-in-progress text: an optional leading run of
    /// digits (a `[count]` prefix, spec 5.2), followed by an optional
    /// single trailing "pending trigger" character for a two-keystroke
    /// command still waiting on its second key (`g` awaiting a second `g`,
    /// or `f`/`F`/`t`/`T` awaiting a target character). Also doubles as
    /// in-progress `:command` text while `vim_mode == Command` — not yet
    /// populated for that purpose (Task D left Command mode entry/exit
    /// only; Task H adds real command-text capture).
    pub vim_command_buf: String,
    /// The most recent `f`/`F`/`t`/`T` search on this tab, as
    /// (variant, target char) — `;` replays it as-is, `,` replays it with
    /// the variant reversed (f<->F, t<->T). `None` until the first find.
    pub last_find: Option<(char, char)>,
    /// The operator (`d`/`y`/`c`, spec 5.3) waiting for its motion,
    /// doubled-key (`dd`/`yy`/`cc`), or text object to complete it. `None`
    /// outside of that two-part sequence. Separate from `vim_command_buf`'s
    /// pending-trigger mechanism (used by `f`/`g`/etc.) since an operator
    /// is a distinct kind of "waiting for the next key" state with its own
    /// completion rules (see `complete_vim_operator`).
    pub vim_pending_operator: Option<char>,
    /// While `vim_pending_operator` is set: `Some(true)` after an `i`
    /// prefix (inner), `Some(false)` after an `a` prefix (around), waiting
    /// for the text-object key (`w`/`s`/`p`/`"`/`'`/a bracket, spec 5.4).
    /// `None` when no text-object prefix has been typed yet (or the
    /// operator is being completed by a plain motion/doubled-key instead).
    pub vim_pending_text_object_prefix: Option<bool>,
    /// In-progress `:command` text (spec 5.7), captured while
    /// `vim_mode == Command`. Deliberately separate from `vim_command_buf`,
    /// which is a digit+single-trigger-char buffer with its own parser
    /// (`split_vim_command_buf`) not built for arbitrary text like
    /// `%s/foo/bar/g`.
    pub vim_command_line: String,
    /// An error message from the last dispatched `:command` (e.g. `:q` on
    /// a modified buffer, or an unrecognized command), shown in the mode
    /// indicator until the next command is entered or dispatched.
    pub vim_command_error: Option<String>,
    /// True right after a bare `"` (spec 5.8's register-select prefix),
    /// while waiting for the register character (`a`-`z`, `+`, `0`, `"`)
    /// that completes it.
    pub vim_pending_register_select: bool,
    /// The register selected by a `"<char>` prefix, consumed by the very
    /// next register-writing (`d`/`y`/`c`) or register-reading (`p`/`P`)
    /// action, then reset. `None` means the default register (`'"'`).
    pub vim_selected_register: Option<char>,
    /// True right after `r` (spec 5.5), waiting for the character that
    /// overwrites the one under the cursor. `Escape` cancels without
    /// changing anything.
    pub vim_pending_replace: bool,
    /// Set when entering `VimMode::Search` (spec 5.5's `/`/`?`): `true`
    /// for `/` (forward), `false` for `?` (backward). Read once the typed
    /// pattern in `vim_command_line` (reused — the two modes are mutually
    /// exclusive) is dispatched on `Enter`.
    pub vim_search_direction: bool,
    /// Jump list (spec 5.5's `Ctrl+o`/`Ctrl+i`): cursor positions to jump
    /// back to, and (once `Ctrl+o` has been used) positions to jump
    /// forward to again — a back/forward stack pair, the same shape as
    /// `undo_stack`/`redo_stack`. Pushed to by `apply_vim_motion` whenever
    /// a motion moves the cursor more than one line, per `vim_todo.md`'s
    /// heuristic ("push before any jump that moves the cursor more than
    /// one line").
    pub vim_jump_back: Vec<usize>,
    pub vim_jump_forward: Vec<usize>,
    /// Set by `AppState::jump_to_line` (the Nav menu's click-to-jump), read
    /// and cleared by `TextEditor::render()` on its next paint. Ordinary
    /// in-editor cursor movement never touches this — those call
    /// `scroll_to_cursor()` directly, since they already run inside
    /// `TextEditor` and have a `Context<TextEditor>` to call it with. This
    /// flag exists only because `FileExplorer` (where Nav lives) has no
    /// reference to `TextEditor` to call that private method on directly —
    /// only the shared `AppState` — so it leaves a note for `TextEditor` to
    /// act on next time it redraws instead.
    pub pending_scroll_to_cursor: bool,
}

/// A single empty paragraph containing one default (unformatted) run — the
/// starting state for `Tab.paragraphs` before any docx has been parsed into
/// it. Never `vec![]`: every rich-text-aware function assumes at least one
/// paragraph and run always exist.
pub fn default_paragraphs() -> Vec<Paragraph> {
    vec![Paragraph { runs: vec![Run::default()], heading: 0, alignment: Alignment::default(), unsupported_xml: None }]
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
            paragraphs: default_paragraphs(),
            docx_origin: None,
            pending_format: None,
            cursor: 0,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
            vim_mode: VimMode::Normal,
            vim_command_buf: String::new(),
            last_find: None,
            vim_pending_operator: None,
            vim_pending_text_object_prefix: None,
            vim_command_line: String::new(),
            vim_command_error: None,
            vim_pending_register_select: false,
            vim_selected_register: None,
            vim_pending_replace: false,
            vim_search_direction: true,
            vim_jump_back: Vec::new(),
            vim_jump_forward: Vec::new(),
            pending_scroll_to_cursor: false,
            has_unsupported_blocks: false,
            unsupported_banner_dismissed: false,
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
            paragraphs: default_paragraphs(),
            docx_origin: None,
            pending_format: None,
            cursor: 0,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
            vim_mode: VimMode::Normal,
            vim_command_buf: String::new(),
            last_find: None,
            vim_pending_operator: None,
            vim_pending_text_object_prefix: None,
            vim_command_line: String::new(),
            vim_command_error: None,
            vim_pending_register_select: false,
            vim_selected_register: None,
            vim_pending_replace: false,
            vim_search_direction: true,
            vim_jump_back: Vec::new(),
            vim_jump_forward: Vec::new(),
            pending_scroll_to_cursor: false,
            has_unsupported_blocks: false,
            unsupported_banner_dismissed: false,
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

/// Which view the left sidebar (`FileExplorer`) currently shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarMode {
    #[default]
    Files,
    Nav,
}

/// The shared application state, owned as a GPUI Model and read/written by all views.
pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub next_tab_id: usize,
    pub sidebar_visible: bool,
    /// Which view the left sidebar shows — the file tree, or (Nav) a
    /// heading outline of the active tab's Pocket/Hat/Block/Tag lines.
    /// Toggled from two places that both flip the same field: the ribbon's
    /// Nav button, and a Files/Nav button pair in the sidebar's own header.
    pub sidebar_mode: SidebarMode,
    pub settings_visible: bool,
    pub working_directory: PathBuf,
    pub file_tree: Vec<FileNode>,
    /// Whether vim keybindings are active, loaded from settings.conf's
    /// `[KEYBINDS] vim` flag (see `keybinds::load_vim_enabled`) and toggled
    /// live from the settings modal's Vim Mode switch.
    pub vim_enabled: bool,
    /// Every configurable, non-vim keybinding (see `src/keybinds.rs`),
    /// loaded from settings.conf at startup. Owned here (rather than a
    /// standalone global) so the settings modal can mutate it through the
    /// same `Entity<AppState>` every other view already shares, then call
    /// `keybinds::rebuild_keymap` and `Keybinds::save_to` to make an edit
    /// take effect immediately and persist.
    pub keybinds: crate::keybinds::Keybinds,
    pub theme: crate::theme::ThemeKind,
    pub theme_color_mode: crate::theme::ThemeColorMode,
    /// Saved macro recordings, keyed by register (user-requested, not in editor_instructions.md).
    pub vim_macros: HashMap<char, Vec<RecordedVimKey>>,
    /// The register currently being recorded into and its keystrokes so
    /// far; `None` when not recording.
    vim_macro_recording: Option<(char, Vec<RecordedVimKey>)>,
    /// True right after a bare `q` (with nothing already recording), while
    /// waiting for the register character that completes `q<register>`.
    vim_macro_record_pending: bool,
    /// The register most recently replayed via `@<register>`, so a
    /// following `@@` can repeat it without re-specifying.
    pub vim_last_macro_register: Option<char>,
    /// Vim registers (spec 5.8), keyed by name. `d`/`c` write the deleted
    /// text to `'"'` (plus the selected named register, if any); `y` also
    /// writes to `'0'` (the yank register). `'+'` is stored here like any
    /// other named register — `text_editor.rs` mirrors it to/from the OS
    /// clipboard around dispatch, since that needs a GPUI `cx` this file
    /// doesn't have.
    pub registers: HashMap<char, String>,
    /// Mailbox for the `'+'` register: set to the text just written to it
    /// (by a `"+y`/`"+d`/`"+c`), drained by `text_editor.rs` right after
    /// dispatch to push it onto the real OS clipboard. `None` means no
    /// pending clipboard write.
    pub pending_clipboard_sync: Option<String>,
    /// The last `/`/`?` search dispatched, or the last `*`/`#` word-search
    /// (spec 5.5) — (pattern, is_forward). Not per-tab: real vim shares
    /// the search register across buffers, same reasoning `registers`/
    /// `vim_macros` use. `n`/`N` repeat it (`N` reverses the direction).
    pub last_search: Option<(String, bool)>,
    /// The last repeatable change (spec 5.5's `.`), scoped to operator +
    /// motion/text-object changes and `i`/`a`/`c`-style insertions per
    /// `vim_todo.md`'s explicit guidance — not arbitrary multi-command
    /// sequences. `None` until the first repeatable change happens.
    pub last_change: Option<VimChange>,
    /// While a change-recordable operator (`d`/`c`/`>`/`<`/`gU`/`gu` — not
    /// `y`, which isn't a "change") is pending: the completion keystrokes
    /// fed to it so far, mirroring `RecordedVimKey` so `.` can replay them
    /// through `complete_vim_operator` again at the new cursor position.
    /// `text_editor.rs` appends to this (mirroring macro recording's own
    /// capture site) *before* dispatching each keystroke while it's
    /// `Some`, so the completing keystroke itself is captured too.
    pub(crate) vim_change_recording: Option<Vec<RecordedVimKey>>,
    /// While in an Insert-mode session that should be captured for `.`:
    /// the text typed so far. Started unconditionally by
    /// `vim_enter_insert_before_cursor` (so `i`/`a`/`I`/`A`/`c` all cover
    /// it — `o`/`O` also start one, but since they aren't in `.`'s
    /// documented scope, replaying it back will insert the text inline
    /// rather than reopening a new line, a known simplification).
    /// Committed to `last_change` when Insert mode exits.
    vim_insertion_recording: Option<String>,
    /// Set by `execute_vim_operator_range`'s `'c'` case: the operator +
    /// completion keystrokes that ran just before entering Insert, held
    /// until that Insert session ends so the two can be combined into one
    /// `VimChange::OperatorInsert` — real vim's `.` after `cw<text><Esc>`
    /// repeats both the deletion and the retyped text.
    vim_pending_change_before_insert: Option<(char, Vec<RecordedVimKey>)>,
    pub paragraph_integrity: bool,
    pub pilcrows: bool,
    pub fold_all: bool,
    pub invisibility_mode: bool,
    pub split_view: bool,
}

/// The last repeatable change (spec 5.5's `.`) — see `AppState.last_change`.
#[derive(Clone, Debug, PartialEq)]
pub enum VimChange {
    /// A non-inserting operator (`d`/`>`/`<`/`gU`/`gu`) plus the
    /// keystrokes that completed it (a motion, a doubled key, or a
    /// text-object prefix + object character).
    Operator(char, Vec<RecordedVimKey>),
    /// `c` plus its completion keystrokes, plus the text typed in the
    /// Insert session it led into.
    OperatorInsert(char, Vec<RecordedVimKey>, String),
    /// A plain `i`/`a`/`I`/`A`-style insertion with no preceding operator.
    Insertion(String),
}

/// One recorded keystroke, captured verbatim so macro replay can feed it
/// back through the same key-handling path a live keypress takes.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedVimKey {
    pub key: String,
    pub shift: bool,
    pub key_char: Option<String>,
}

/// The line-based card styles from `notes/ribbon_instructions.md` — each
/// applies bold + a fixed font size + its own special formatting + center
/// alignment to the entire current line. See `AppState::apply_card_style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardStyleKind {
    Pocket,
    Hat,
    Block,
    Tag,
}

impl CardStyleKind {
    fn font_size(&self) -> u16 {
        match self {
            CardStyleKind::Pocket => 52, // 26pt
            CardStyleKind::Hat => 44,    // 22pt
            CardStyleKind::Block => 32,  // 16pt
            CardStyleKind::Tag => 26,    // 13pt
        }
    }

    fn is_centered(&self) -> bool {
        matches!(self, CardStyleKind::Pocket | CardStyleKind::Hat | CardStyleKind::Block)
    }

    /// The `Paragraph.heading` value each card style marks its line with —
    /// also the markdown level `wikifi_export.rs` maps it to (1=H1 .. 4=H4)
    /// and the nesting depth the Nav menu indents it at.
    fn heading_level(&self) -> u8 {
        match self {
            CardStyleKind::Pocket => 1,
            CardStyleKind::Hat => 2,
            CardStyleKind::Block => 3,
            CardStyleKind::Tag => 4,
        }
    }
}

impl AppState {
    pub fn new() -> Self {
        /*
         * Initialises the application with a single empty tab, the sidebar visible,
         * the settings modal hidden, and the working directory set to the process's
         * current directory. The file tree is populated immediately by scanning that
         * directory for .docx files. Keybindings and vim mode are loaded from
         * settings.conf (an app-level config, always read relative to the process's
         * CWD rather than `working_directory`, since it isn't tied to whichever
         * folder the user opens in the file explorer).
         */
        let working_directory = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."));

        let file_tree = scan_directory(&working_directory);
        let settings_path = std::path::Path::new("settings.conf");
        let keybinds = crate::keybinds::Keybinds::load(settings_path);
        let vim_enabled = crate::keybinds::load_vim_enabled(settings_path);
        let theme = crate::theme::load_theme(settings_path);
        let theme_color_mode = crate::theme::load_theme_color_mode(settings_path);

        AppState {
            tabs: vec![Tab::new_empty(0)],
            active_tab: 0,
            next_tab_id: 1,
            sidebar_visible: true,
            sidebar_mode: SidebarMode::default(),
            settings_visible: false,
            working_directory,
            file_tree,
            vim_enabled,
            keybinds,
            theme,
            theme_color_mode,
            vim_macros: HashMap::new(),
            vim_macro_recording: None,
            vim_macro_record_pending: false,
            vim_last_macro_register: None,
            registers: HashMap::new(),
            pending_clipboard_sync: None,
            last_search: None,
            last_change: None,
            vim_change_recording: None,
            vim_insertion_recording: None,
            vim_pending_change_before_insert: None,
            paragraph_integrity: false,
            pilcrows: false,
            fold_all: false,
            invisibility_mode: false,
            split_view: false,
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
         * the tab still opens with empty content and `docx_origin = None`
         * (`paragraphs` stays at its default single empty paragraph/run).
         */
        if let Some(idx) = self.tabs.iter().position(|t| t.file_path.as_deref() == Some(&path)) {
            self.active_tab = idx;
            return;
        }
        let mut tab = Tab::from_path(self.next_tab_id, path.clone());
        if let Ok((paragraphs, origin)) = parse_docx(&path) {
            tab.content = paragraphs_to_plain_text(&paragraphs);
            tab.paragraphs = paragraphs;
            tab.has_unsupported_blocks = origin.has_unsupported_blocks;
            tab.docx_origin = Some(Arc::new(origin));
        }
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    pub fn save_active_tab(&mut self) -> Result<(), String> {
        /*
         * Saves the active tab's content to its associated file path. Thin
         * wrapper around `save_tab` (Task H pulled the actual work out into
         * an index-taking core so `:wa`, spec 5.7, can loop every tab
         * without needing to juggle `active_tab`).
         */
        self.save_tab(self.active_tab)
    }

    fn save_tab(&mut self, idx: usize) -> Result<(), String> {
        /*
         * Saves the tab at `idx` to its associated file path, from the
         * live, formatting-synced `paragraphs` (rich-text formatting plan,
         * Phase 1 Task 7) — the fix for the long-standing "editing a
         * loaded docx destroys its formatting on save" simplification
         * (`editor_instructions.md` line 82), since `paragraphs` now stays
         * accurate through every edit (Phase 1 Task 4) instead of being
         * regenerated from scratch as plain unstyled runs.
         *
         * When `docx_origin` is `Some`: uses it as the template (original
         * ZIP bytes, XML preamble/sectPr) so styles/images/fonts survive
         * untouched.
         *
         * When `docx_origin` is `None` (file created fresh inside
         * vimbatim): uses `create_new_docx` to write a valid minimal docx
         * from scratch.
         *
         * Tabs with no file path (plain "New Tab") are silently skipped — there
         * is nowhere to write to yet.
         */
        let tab = self.tabs.get(idx).ok_or("No active tab")?;
        let path = match &tab.file_path {
            Some(p) => p.clone(),
            None    => return Ok(()), // nothing to save yet
        };
        if !tab.is_modified {
            return Ok(());
        }
        let paragraphs = tab.paragraphs.clone();
        let origin = tab.docx_origin.clone();
        match origin {
            Some(origin) => origin.save(&paragraphs, &path)
                .map_err(|e| format!("Save failed: {}", e))?,
            None => create_new_docx(&paragraphs, &path)
                .map_err(|e| format!("Save failed: {}", e))?,
        }
        if let Some(tab) = self.tabs.get_mut(idx) {
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
        tab.undo_stack.push((tab.content.clone(), tab.paragraphs.clone()));
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
                sync_delete_range(&mut tab.paragraphs, start, end);
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
        let mut inserted_range = None;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_insert_char(&mut tab.paragraphs, tab.cursor, ch);
            tab.content.insert(tab.cursor, ch);
            let start = tab.cursor;
            tab.cursor += ch.len_utf8();
            tab.is_modified = true;
            inserted_range = Some((start, tab.cursor));
        }
        // A pending format (spec 7: armed with no selection, per
        // `apply_formatting_to_selection`) applies to every character typed
        // until the same action is triggered again — not just this one.
        if let Some((start, end)) = inserted_range {
            let pending = self.tabs.get(self.active_tab).and_then(|t| t.pending_format.clone());
            if let Some(op) = pending {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    apply_formatting(&mut tab.paragraphs, start, end, op);
                }
            }
        }
        if let Some(rec) = self.vim_insertion_recording.as_mut() {
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
            sync_delete_range(&mut tab.paragraphs, prev, tab.cursor);
            tab.content.remove(prev);
            tab.cursor = prev;
            tab.is_modified = true;
        }
        if let Some(rec) = self.vim_insertion_recording.as_mut() {
            rec.pop();
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
            let Some(tab) = self.tabs.get(self.active_tab) else { return };
            let cursor = tab.cursor;

            // Find the start of the current line (after previous newline)
            let line_start = tab.content[..cursor]
                .rfind('\n')
                .map(|pos| pos + 1)
                .unwrap_or(0);

            // Find the end of the current line (next newline or end of content)
            let line_end = tab.content[cursor..]
                .find('\n')
                .map(|pos| cursor + pos)
                .unwrap_or(tab.content.len());

            (line_start, line_end)
        };

        let is_line_empty = line_start >= line_end;

        self.push_undo_snapshot();

        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let effective_op = if is_uniformly_active(&tab.paragraphs, line_start, line_end, &op) {
            toggled_off(&op)
        } else {
            op.clone()
        };
        apply_formatting(&mut tab.paragraphs, line_start, line_end, effective_op.clone());
        tab.is_modified = true;

        // If applying to an empty line, arm pending_format so subsequent typing inherits formatting.
        // This ensures card styles are visible immediately when the user starts typing.
        if is_line_empty && !is_uniformly_active(&self.tabs.get(self.active_tab).map(|t| &t.paragraphs).unwrap_or(&vec![]), line_start, line_end, &op) {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                // Only set pending_format for run-level operations, not paragraph-level ones
                match &effective_op {
                    FormatOp::Bold(_) | FormatOp::FontSize(_) | FormatOp::Underline(_) |
                    FormatOp::DoubleUnderline(_) | FormatOp::Box(_) | FormatOp::Italic(_) |
                    FormatOp::Strikethrough(_) | FormatOp::Highlight(_) | FormatOp::Color(_) => {
                        tab.pending_format = Some(effective_op);
                    }
                    _ => {}
                }
            }
        }
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
        let selection = self.tabs.get(self.active_tab).and_then(|t| t.selection);
        match selection {
            Some((a, f)) => {
                let (start, end) = (a.min(f), a.max(f));
                self.push_undo_snapshot();
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    let effective_op = if is_uniformly_active(&tab.paragraphs, start, end, &op) {
                        toggled_off(&op)
                    } else {
                        op.clone()
                    };
                    apply_formatting(&mut tab.paragraphs, start, end, effective_op);
                    tab.is_modified = true;
                }
            }
            None => {
                let Some(tab) = self.tabs.get(self.active_tab) else { return };
                let cursor = tab.cursor;
                let content_len = tab.content.len();

                // Check if pending format matches current op to decide toggle behavior
                let should_toggle_off = tab.pending_format.as_ref() == Some(&op);

                // Apply to character under cursor if not at end of document
                if cursor < content_len {
                    let next_char_boundary = char_right(&tab.content, cursor);
                    let effective_op = if should_toggle_off {
                        toggled_off(&op)
                    } else {
                        op.clone()
                    };
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        apply_formatting(&mut tab.paragraphs, cursor, next_char_boundary, effective_op);
                        tab.is_modified = true;
                    }
                }

                // Update pending format (same toggle logic as before)
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    if should_toggle_off {
                        tab.pending_format = None;
                    } else {
                        tab.pending_format = Some(op);
                    }
                }
            }
        }
    }

    pub fn paste_text(&mut self, text: &str) {
        /*
         * Inserts clipboard text at cursor or replaces selection.
         * Mirrors insert_str but is called from ribbon button handler.
         * Respects paragraph_integrity and pilcrows toggles.
         */
        if text.is_empty() {
            return;
        }
        let processed = if self.paragraph_integrity {
            text.replace('\n', " ")
        } else if self.pilcrows {
            text.replace('\n', "¶")
        } else {
            text.to_string()
        };
        self.insert_str(&processed);
    }

    pub fn condense_selection(&mut self) {
        /*
         * Removes newlines from selected text and replaces with spaces.
         * Only works on active selection; no-op if no selection.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return;
        }

        let selected_text = tab.content[start..end].to_string();
        let condensed = selected_text.replace('\n', " ");

        if condensed == selected_text {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_delete_range(&mut tab.paragraphs, start, end);
            tab.content.drain(start..end);
            sync_insert_str(&mut tab.paragraphs, start, &condensed);
            tab.content.insert_str(start, &condensed);
            tab.cursor = start;
            tab.selection = Some((start, start + condensed.len()));
            tab.is_modified = true;
        }
    }

    pub fn apply_bullet_list(&mut self) {
        /*
         * Adds bullet prefixes to each line in the selection.
         * Replaces existing bullets if lines already have them.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end { return }

        let selected_text = tab.content[start..end].to_string();
        let lines: Vec<&str> = selected_text.lines().collect();
        if lines.is_empty() { return }

        let bulleted: Vec<String> = lines.into_iter()
            .map(|line| {
                let trimmed = line.trim_start();
                if trimmed.starts_with("• ") || trimmed.starts_with("- ") {
                    trimmed.to_string()
                } else {
                    format!("• {}", trimmed)
                }
            })
            .collect();

        let new_text = bulleted.join("\n");
        if new_text == selected_text { return }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_delete_range(&mut tab.paragraphs, start, end);
            tab.content.drain(start..end);
            sync_insert_str(&mut tab.paragraphs, start, &new_text);
            tab.content.insert_str(start, &new_text);
            tab.is_modified = true;
        }
    }

    pub fn apply_numbered_list(&mut self) {
        /*
         * Adds number prefixes to each line in the selection.
         * Replaces existing numbers if lines already have them.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end { return }

        let selected_text = tab.content[start..end].to_string();
        let lines: Vec<&str> = selected_text.lines().collect();
        if lines.is_empty() { return }

        let numbered: Vec<String> = lines.into_iter()
            .enumerate()
            .map(|(i, line)| {
                let trimmed = line.trim_start();
                // Remove existing number prefix if present
                let content = if let Some(pos) = trimmed.find(". ") {
                    if pos < 4 && trimmed[..pos].chars().all(|c| c.is_numeric()) {
                        trimmed[pos+2..].to_string()
                    } else {
                        trimmed.to_string()
                    }
                } else {
                    trimmed.to_string()
                };
                format!("{}. {}", i + 1, content)
            })
            .collect();

        let new_text = numbered.join("\n");
        if new_text == selected_text { return }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_delete_range(&mut tab.paragraphs, start, end);
            tab.content.drain(start..end);
            sync_insert_str(&mut tab.paragraphs, start, &new_text);
            tab.content.insert_str(start, &new_text);
            tab.is_modified = true;
        }
    }

    pub fn cycle_font_size(&mut self) {
        /*
         * Cycles through preset font sizes: 24pt -> 32pt -> 48pt -> 24pt (in half-points).
         * Detects the current size uniformly applied to selection, then advances to next.
         * Applies to selection or sets pending format if no selection.
         */
        let tab = self.tabs.get(self.active_tab);
        let selection = tab.and_then(|t| t.selection);

        let current_size = if let Some((a, f)) = selection {
            let (start, end) = (a.min(f), a.max(f));
            tab.and_then(|t| {
                // Check if all runs in range have same size
                let mut uniform_size: Option<u16> = None;
                for para in &t.paragraphs {
                    let mut pos = 0;
                    for run in &para.runs {
                        let run_end = pos + run.text.len();
                        if run_end > start && pos < end {
                            if uniform_size.is_none() {
                                uniform_size = Some(run.size);
                            } else if uniform_size != Some(run.size) {
                                return None; // not uniform
                            }
                        }
                        pos = run_end;
                    }
                }
                uniform_size
            })
        } else {
            None
        };

        let next_size = match current_size {
            Some(24) => 32,
            Some(32) => 48,
            Some(48) => 24,
            _ => 24, // default to first size
        };

        self.apply_formatting_to_selection(FormatOp::FontSize(next_size));
    }

    pub fn cycle_text_color(&mut self) {
        /*
         * Cycles through preset text colors: yellow -> red -> blue -> yellow.
         * Detects current color uniformly applied to selection, then advances.
         * Applies to selection or sets pending format if no selection.
         */
        let tab = self.tabs.get(self.active_tab);
        let selection = tab.and_then(|t| t.selection);

        let current_color = if let Some((a, f)) = selection {
            let (start, end) = (a.min(f), a.max(f));
            tab.and_then(|t| {
                // Check if all runs in range have same color
                let mut uniform_color: Option<String> = None;
                for para in &t.paragraphs {
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
            _ => "ffff00", // default to yellow
        };

        self.apply_formatting_to_selection(FormatOp::Color(Some(next_color.to_string())));
    }

    pub fn cycle_highlight_color(&mut self) {
        /*
         * Cycles through preset highlight colors: yellow -> green -> blue -> yellow.
         * Detects current highlight uniformly applied to selection, then advances.
         * Applies to selection or sets pending format if no selection.
         */
        let tab = self.tabs.get(self.active_tab);
        let selection = tab.and_then(|t| t.selection);

        let current_highlight = if let Some((a, f)) = selection {
            let (start, end) = (a.min(f), a.max(f));
            tab.and_then(|t| {
                // Check if all runs in range have same highlight color
                let mut uniform_highlight: Option<String> = None;
                for para in &t.paragraphs {
                    let mut pos = 0;
                    for run in &para.runs {
                        let run_end = pos + run.text.len();
                        if run_end > start && pos < end {
                            if run.highlight {
                                if uniform_highlight.is_none() {
                                    uniform_highlight = Some(run.highlight_color.clone());
                                } else if uniform_highlight.as_ref() != Some(&run.highlight_color) {
                                    return None; // not uniform
                                }
                            } else if uniform_highlight.is_some() {
                                return None; // some have highlight, some don't
                            }
                        }
                        pos = run_end;
                    }
                }
                uniform_highlight
            })
        } else {
            None
        };

        let next_color = match current_highlight.as_deref() {
            Some("yellow") => "green",
            Some("green") => "blue",
            Some("blue") => "yellow",
            _ => "yellow", // default to yellow
        };

        self.apply_formatting_to_selection(FormatOp::Highlight(Some(next_color.to_string())));
    }

    pub fn toggle_strikethrough(&mut self) {
        /*
         * Toggles strikethrough on selected text or sets pending format
         * for future typing if no selection. Data is stored but rendering
         * is deferred until GPUI supports text decoration.
         */
        self.apply_formatting_to_selection(FormatOp::Strikethrough(true));
    }

    pub fn shrink_text(&mut self) {
        /*
         * Reduces font size of all non-underlined text in selection by 1 point.
         * Finds runs without underline and decreases their size.
         */
        let selection = self.tabs.get(self.active_tab).and_then(|t| t.selection);
        match selection {
            Some((a, f)) => {
                let (start, end) = (a.min(f), a.max(f));
                self.push_undo_snapshot();
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    let mut cumulative = 0usize;
                    for para in &mut tab.paragraphs {
                        for run in &mut para.runs {
                            let run_start = cumulative;
                            let run_end = cumulative + run.text.len();
                            if run_start >= start && run_end <= end && !run.underline && run.size > 2 {
                                run.size = run.size.saturating_sub(2); // 2 half-points = 1pt
                            }
                            cumulative = run_end;
                        }
                        cumulative += 1;
                    }
                    tab.is_modified = true;
                }
            }
            None => {} // No-op when no selection
        }
    }

    pub fn apply_case_to_selection(&mut self, case_type: case_converter::CaseType) {
        /*
         * Changes case of selected text. No-op when no selection.
         */
        let selection = self.tabs.get(self.active_tab).and_then(|t| t.selection);
        match selection {
            Some((a, f)) => {
                let (start, end) = (a.min(f), a.max(f));
                self.push_undo_snapshot();
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    let mut cumulative = 0usize;
                    for para in &mut tab.paragraphs {
                        for run in &mut para.runs {
                            let run_start = cumulative;
                            let run_end = cumulative + run.text.len();
                            if run_start >= start && run_end <= end {
                                run.text = case_converter::apply_case(&run.text, case_type);
                            }
                            cumulative = run_end;
                        }
                        cumulative += 1;
                    }
                    tab.is_modified = true;
                    // Update content to match
                    tab.content = paragraphs_to_plain_text(&tab.paragraphs);
                }
            }
            None => {}
        }
    }

    pub fn apply_font_color(&mut self, color: color_picker::ColorChoice) {
        /*
         * Applies font color to selected text.
         */
        let hex_str = format!("{:06x}", color.hex_value());
        self.apply_formatting_to_selection(FormatOp::Color(Some(hex_str)));
    }

    pub fn toggle_fold(&mut self) {
        /*
         * Toggles folding of all headings. When folded, only heading lines
         * are shown; body text is hidden.
         */
        if let Some(_tab) = self.tabs.get(self.active_tab) {
            self.fold_all = !self.fold_all;
        }
    }

    pub fn toggle_paragraph_integrity(&mut self) {
        /*
         * Toggles paragraph integrity mode. When on, newlines are
         * excluded from pastes.
         */
        self.paragraph_integrity = !self.paragraph_integrity;
    }

    pub fn toggle_pilcrows(&mut self) {
        /*
         * Toggles pilcrow display mode. When on, newlines are
         * shown as pilcrow characters (¶).
         */
        self.pilcrows = !self.pilcrows;
    }

    pub fn toggle_invisibility_mode(&mut self) {
        /*
         * Toggles invisibility mode. When on, only highlighted text,
         * tags, and citations are shown.
         */
        self.invisibility_mode = !self.invisibility_mode;
    }

    pub fn get_tab_titles(&self) -> Vec<(usize, String)> {
        /*
         * Returns list of (index, title) for all open tabs.
         */
        self.tabs.iter().enumerate()
            .map(|(idx, tab)| (idx, tab.title.clone()))
            .collect()
    }

    pub fn toggle_split_view(&mut self) {
        /*
         * Toggles split view mode. When on, editor is split into
         * two windows side-by-side.
         */
        self.split_view = !self.split_view;
    }

    pub fn wikify_current_tab(&mut self) -> std::io::Result<()> {
        /*
         * Exports current tab to markdown file with heading hierarchy.
         * File is saved as document_name.md in same directory.
         */
        let tab = self.tabs.get(self.active_tab).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No active tab")
        })?;

        let markdown = wikifi_export::export_to_markdown(&tab.paragraphs, &tab.content);

        if let Some(path) = &tab.file_path {
            wikifi_export::save_markdown_file(path, &markdown)?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Tab must be saved first"
            ));
        }
        Ok(())
    }

    pub fn apply_center_alignment(&mut self) {
        /*
         * Applies center alignment to all paragraphs overlapping the active
         * selection (or the paragraph containing the cursor, if no selection).
         * Phase 4.2: Center-align card styles (Pocket, Hat, Block).
         */
        let selection = self.tabs.get(self.active_tab).and_then(|t| t.selection);
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
                let cursor = self.tabs.get(self.active_tab).map(|t| t.cursor).unwrap_or(0);
                (cursor, cursor)
            }
        };

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            apply_paragraph_alignment(&mut tab.paragraphs, start, end, Alignment::Center);
            tab.is_modified = true;
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
        let size = kind.font_size();

        self.apply_formatting_to_line(FormatOp::Bold(true));
        self.apply_formatting_to_line(FormatOp::FontSize(size));
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
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let line_idx = tab.content[..tab.cursor].matches('\n').count();
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                if let Some(para) = tab.paragraphs.get_mut(line_idx) {
                    para.heading = kind.heading_level();
                }
            }
        }

        if kind.is_centered() {
            let tab = self.tabs.get_mut(self.active_tab);
            if let Some(t) = tab {
                let cursor = t.cursor;
                let line_start = t.content[..cursor]
                    .rfind('\n')
                    .map(|pos| pos + 1)
                    .unwrap_or(0);
                let line_end = t.content[cursor..]
                    .find('\n')
                    .map(|pos| cursor + pos)
                    .unwrap_or(t.content.len());
                self.apply_center_alignment_with_selection(Some((line_start, line_end)));
            }
        }
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let Some(previous) = tab.undo_stack.pop() else { return };
        let current_content = std::mem::replace(&mut tab.content, previous.0);
        let current_paragraphs = std::mem::replace(&mut tab.paragraphs, previous.1);
        tab.redo_stack.push((current_content, current_paragraphs));
        tab.selection = None;
        tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor);
        tab.is_modified = true;
        // Break the coalescing window so the next edit doesn't merge into
        // whatever was on top of the undo stack before this undo.
        tab.last_edit_at = None;
    }

    pub fn redo(&mut self) {
        /*
         * The undo counterpart: restores the most recently undone
         * `(content, paragraphs)` pair from the redo stack, pushing the
         * pair being replaced back onto the undo stack. No-op when
         * there's nothing to redo. Cursor handling mirrors `undo()`.
         */
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let Some(next) = tab.redo_stack.pop() else { return };
        let current_content = std::mem::replace(&mut tab.content, next.0);
        let current_paragraphs = std::mem::replace(&mut tab.paragraphs, next.1);
        tab.undo_stack.push((current_content, current_paragraphs));
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
            sync_insert_str(&mut tab.paragraphs, tab.cursor, text);
            tab.content.insert_str(tab.cursor, text);
            tab.cursor += text.len(); // text is valid UTF-8 so len() == byte count
            tab.is_modified = true;
        }
        if let Some(rec) = self.vim_insertion_recording.as_mut() {
            rec.push_str(text);
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
            tab.cursor = first_nonblank(&tab.content, tab.cursor);
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

    pub fn move_word_forward_big(&mut self) {
        /*
         * Moves the cursor to the start of the next WORD, matching vim's
         * `W` — a WORD is any whitespace-delimited run, with no additional
         * split between alphanumeric and punctuation runs the way `w`
         * makes.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = word_forward_big(&tab.content, tab.cursor);
        }
    }

    pub fn move_word_end_big(&mut self) {
        /*
         * Moves the cursor to the last character of the current or next
         * WORD, matching vim's `E`.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = word_end_big(&tab.content, tab.cursor);
        }
    }

    pub fn move_word_backward_big(&mut self) {
        /*
         * Moves the cursor to the start of the current or previous WORD,
         * matching vim's `B`.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = word_backward_big(&tab.content, tab.cursor);
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = paragraph_forward(&tab.content, tab.cursor);
        }
    }

    pub fn move_paragraph_backward(&mut self) {
        /*
         * Moves the cursor backward to the start of the previous paragraph,
         * matching vim's `{`. Mirrors `move_paragraph_forward`.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = None;
            tab.cursor = paragraph_backward(&tab.content, tab.cursor);
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        if let Some((kind, target)) = tab.last_find {
            let reversed = match kind {
                'f' => 'F', 'F' => 'f', 't' => 'T', 'T' => 't',
                other => other,
            };
            self.apply_find(reversed, target, false);
        }
    }

    fn apply_find(&mut self, kind: char, target: char, remember: bool) {
        /*
         * Shared implementation for the four move_find/till_char_* methods
         * and the two repeat methods. `remember` controls whether this call
         * updates `last_find` (true for a fresh f/F/t/T keypress, false for
         * a `;`/`,` repeat) and doubles as the `nudge` flag for
         * `resolve_find_with_nudge` (a repeat is exactly when the nudge is
         * needed — see that function's doc comment).
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(new_pos) = resolve_find_with_nudge(&tab.content, tab.cursor, kind, target, !remember) {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
         * Clears any in-progress Normal-mode count/pending-trigger buffer
         * (spec 5.2) — a stale count left over from before the mode switch
         * must not silently apply to whatever's typed after returning to
         * Normal mode later.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        self.vim_insertion_recording = Some(String::new());
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
         * where there's no character under the cursor. Sets `tab.cursor`
         * to the selection's far edge (not just its start) — without this,
         * the rendered cursor stays at the pre-Visual position and any
         * subsequent motion (`apply_vim_motion`, which reads `tab.cursor`
         * as its starting point) would resolve from the wrong place,
         * effectively dropping the first character of the entry selection.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::Visual;
            let end = char_right(&tab.content, tab.cursor);
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::VisualLine;
            let start = line_start(&tab.content, tab.cursor);
            let end = line_end(&tab.content, tab.cursor);
            let end_with_newline = if end < tab.content.len() { end + 1 } else { end };
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        let was_insert = self.tabs.get(self.active_tab).map(|t| t.vim_mode == VimMode::Insert).unwrap_or(false);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_mode = VimMode::Normal;
            tab.selection = None;
            tab.vim_command_buf.clear();
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
            // In vim, exiting Insert mode moves cursor back one char to land
            // ON the last character, not after it
            if was_insert && tab.cursor > 0 {
                tab.cursor = char_left(&tab.content, tab.cursor);
            }
        }
        // `.` repeat (spec 5.5): an Insert session just ended — commit
        // what was typed, combining it with the operator that led into it
        // (`c`) if there was one.
        if was_insert {
            if let Some(text) = self.vim_insertion_recording.take() {
                self.last_change = match self.vim_pending_change_before_insert.take() {
                    Some((operator, keys)) => Some(VimChange::OperatorInsert(operator, keys, text)),
                    None => Some(VimChange::Insertion(text)),
                };
            }
        }
    }

    // ── Normal-mode count/pending-trigger buffer (spec 5.2) ─────────────────────

    fn push_vim_command_buf_char(&mut self, c: char) {
        /*
         * Appends one character (a count digit, or a two-keystroke
         * command's first key like `g`/`f`/`F`/`t`/`T`) to the active tab's
         * `vim_command_buf`.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_command_buf.push(c);
        }
    }

    fn clear_vim_command_buf(&mut self) {
        /*
         * Discards the active tab's in-progress count/pending-trigger
         * buffer — called once a Normal-mode command completes (whether it
         * was recognized or not) so a stale prefix can't bleed into the
         * next, unrelated keystroke.
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        self.vim_macro_recording.is_some()
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
        self.vim_macro_recording.as_ref().map(|(register, _)| *register)
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
        self.vim_macro_record_pending
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
        self.tabs.get(self.active_tab).and_then(|t| t.vim_selected_register)
    }

    pub fn set_register(&mut self, register: char, text: String) {
        /*
         * Public setter so `text_editor.rs` can stage the OS clipboard's
         * text into register `'+'` right before dispatching a `"+p`/`"+P`
         * paste — the ordinary (GPUI-unaware) paste path then reads it
         * back out via `registers.get` exactly like any other register.
         */
        self.registers.insert(register, text);
    }

    pub fn take_pending_clipboard_sync(&mut self) -> Option<String> {
        /*
         * Drains the `'+'`-register write mailbox. `text_editor.rs` calls
         * this right after dispatching every vim keystroke and, if it
         * returns `Some`, pushes the text onto the real OS clipboard via
         * `cx.write_to_clipboard` — the one step this file can't do itself.
         */
        self.pending_clipboard_sync.take()
    }

    fn start_macro_recording(&mut self, register: char) {
        /*
         * Begins capturing keystrokes into `register`, discarding any
         * previous recording under that register (matching real vim:
         * `q<register>` always overwrites, never appends — appending needs
         * the uppercase-register form, out of scope here).
         */
        self.vim_macro_recording = Some((register, Vec::new()));
    }

    pub fn record_macro_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        /*
         * Appends one keystroke to the in-progress recording, if any.
         * Called by `text_editor.rs` for every keystroke it sees (before
         * or after its own handling — order doesn't matter to this
         * method), so it's a no-op rather than a panic when nothing is
         * being recorded.
         */
        if let Some((_, keys)) = self.vim_macro_recording.as_mut() {
            keys.push(RecordedVimKey {
                key: key.to_string(),
                shift,
                key_char: key_char.map(str::to_string),
            });
        }
    }

    fn stop_macro_recording(&mut self) {
        /*
         * Ends the in-progress recording (if any) and saves it into
         * `vim_macros` under its register, overwriting whatever was there.
         */
        if let Some((register, keys)) = self.vim_macro_recording.take() {
            self.vim_macros.insert(register, keys);
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
        self.vim_change_recording.is_some()
    }

    pub fn record_change_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        /*
         * Appends one completion keystroke to the in-progress change
         * recording, if any — the `.`-repeat counterpart to
         * `record_macro_key`.
         */
        if let Some(keys) = self.vim_change_recording.as_mut() {
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
        self.vim_macros.get(&register).cloned()
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return None };
        let (count, _trigger) = split_vim_command_buf(&tab.vim_command_buf);
        let digit_len = tab.vim_command_buf.chars().take_while(|c| c.is_ascii_digit()).count();
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
        let tab = self.tabs.get(self.active_tab)?;
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
        self.tabs.get(self.active_tab)?.vim_pending_operator
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
            VimMode::Visual | VimMode::VisualLine => self.handle_vim_visual_key(key, shift, key_char),
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

    fn handle_vim_normal_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
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
        if let Some(operator) = self.tabs.get(self.active_tab).and_then(|t| t.vim_pending_operator) {
            return self.complete_vim_operator(operator, key, shift, key_char);
        }

        // `r<char>` (spec 5.5): a bare `r` arms `vim_pending_replace`, then
        // the *next* keystroke overwrites the character under the cursor
        // (or cancels harmlessly on `Escape`) rather than being interpreted
        // as anything else — checked ahead of every other pending state for
        // the same reason a pending operator is: it must claim its next key
        // unconditionally.
        if self.tabs.get(self.active_tab).map(|t| t.vim_pending_replace).unwrap_or(false) {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_pending_replace = false; }
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
            if self.vim_macro_record_pending {
                self.vim_macro_record_pending = false;
                if let Some(register) = vim_find_target_char(key, shift, key_char) {
                    self.start_macro_recording(register);
                }
                return true;
            }
            if key == "q" && !shift {
                if self.vim_is_recording_macro() {
                    self.stop_macro_recording();
                } else {
                    self.vim_macro_record_pending = true;
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
            ("i", false) => { self.vim_enter_insert_before_cursor(); true }
            ("i", true)  => { self.vim_enter_insert_line_start(); true }
            ("a", false) => { self.vim_enter_insert_after_cursor(); true }
            ("a", true)  => { self.vim_enter_insert_line_end(); true }
            ("o", false) => { self.vim_open_line_below(); true }
            ("o", true)  => { self.vim_open_line_above(); true }
            ("v", false) => { self.vim_enter_visual(); true }
            ("v", true)  => { self.vim_enter_visual_line(); true }
            ("d", false) => { self.start_vim_operator('d'); true }
            ("y", false) => { self.start_vim_operator('y'); true }
            ("c", false) => { self.start_vim_operator('c'); true }
            ("p", false) => { self.vim_paste_register(false); true }
            ("p", true)  => { self.vim_paste_register(true); true }
            ("x", false) => { self.vim_delete_char_forward(); true }
            ("x", true)  => { self.vim_delete_char_backward(); true }
            ("s", false) => { self.vim_substitute_char(); true }
            ("s", true)  => { self.vim_substitute_line(); true }
            ("j", true)  => { self.vim_join_lines(); true }
            ("r", false) => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_pending_replace = true; }
                true
            }
            ("r", true) => { self.vim_enter_replace(); true }
            ("n", false) => { self.vim_search_next(false); true }
            ("n", true)  => { self.vim_search_next(true); true }
            (".", false) => { self.vim_repeat_last_change(); true }
            _ => true,
        }
    }

    fn handle_vim_motion_key(&mut self, key: &str, shift: bool, key_char: Option<&str>, extend: bool) -> Option<bool> {
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
            MotionResolution::Resolved { target, .. } => Some(self.apply_vim_motion(extend, target)),
        }
    }

    fn resolve_vim_motion(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> MotionResolution {
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
        let buf = self.tabs.get(self.active_tab).map(|t| t.vim_command_buf.clone()).unwrap_or_default();
        let (pending_count, pending_trigger) = split_vim_command_buf(&buf);

        // 1. Complete (or abandon) a pending two-keystroke command.
        if let Some(trigger) = pending_trigger {
            self.clear_vim_command_buf();
            match trigger {
                'g' => {
                    if key == "g" && !shift {
                        let line = pending_count.unwrap_or(1);
                        if let Some(tab) = self.tabs.get(self.active_tab) {
                            let start = line_offset(&tab.content, line.saturating_sub(1));
                            let target = first_nonblank(&tab.content, start);
                            return MotionResolution::Resolved { target, kind: MotionKind::Linewise };
                        }
                    }
                    // any other key: no other `g...` command exists yet,
                    // so the sequence is simply abandoned.
                }
                'f' | 'F' | 't' | 'T' => {
                    if let Some(target_char) = vim_find_target_char(key, shift, key_char) {
                        let count = pending_count.unwrap_or(1);
                        let mut pos = self.tabs.get(self.active_tab).map(|t| t.cursor).unwrap_or(0);
                        let mut found = false;
                        for _ in 0..count {
                            let next = self.tabs.get(self.active_tab)
                                .and_then(|t| resolve_find(&t.content, pos, trigger, target_char));
                            match next {
                                Some(p) => { pos = p; found = true; }
                                None => break,
                            }
                        }
                        if found {
                            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
            let target = self.tabs.get(self.active_tab).map(|tab| line_end(&tab.content, tab.cursor)).unwrap_or(0);
            return MotionResolution::Resolved { target, kind: MotionKind::InclusiveChar };
        }
        if matches_shifted_symbol(key, shift, key_char, "6", "^") {
            self.clear_vim_command_buf();
            let target = self.tabs.get(self.active_tab).map(|tab| first_nonblank(&tab.content, tab.cursor)).unwrap_or(0);
            return MotionResolution::Resolved { target, kind: MotionKind::ExclusiveChar };
        }
        if matches_shifted_symbol(key, shift, key_char, "[", "{") {
            let count = pending_count.unwrap_or(1);
            self.clear_vim_command_buf();
            let target = self.repeat_motion(count, paragraph_backward);
            return MotionResolution::Resolved { target, kind: MotionKind::ExclusiveChar };
        }
        if matches_shifted_symbol(key, shift, key_char, "]", "}") {
            let count = pending_count.unwrap_or(1);
            self.clear_vim_command_buf();
            let target = self.repeat_motion(count, paragraph_forward);
            return MotionResolution::Resolved { target, kind: MotionKind::ExclusiveChar };
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
            let trigger = if shift { key.to_ascii_uppercase().chars().next().unwrap() } else { key.chars().next().unwrap() };
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
            ("h", false) => { let t = self.repeat_motion(count.unwrap_or(1), char_left); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("l", false) => { let t = self.repeat_motion(count.unwrap_or(1), char_right); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("w", false) => { let t = self.repeat_motion(count.unwrap_or(1), word_forward); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("w", true)  => { let t = self.repeat_motion(count.unwrap_or(1), word_forward_big); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("b", false) => { let t = self.repeat_motion(count.unwrap_or(1), word_backward); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("b", true)  => { let t = self.repeat_motion(count.unwrap_or(1), word_backward_big); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("e", false) => { let t = self.repeat_motion(count.unwrap_or(1), word_end); MotionResolution::Resolved { target: t, kind: MotionKind::InclusiveChar } }
            ("e", true)  => { let t = self.repeat_motion(count.unwrap_or(1), word_end_big); MotionResolution::Resolved { target: t, kind: MotionKind::InclusiveChar } }
            ("0", false) => {
                let t = self.tabs.get(self.active_tab).map(|tab| line_start(&tab.content, tab.cursor)).unwrap_or(0);
                MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar }
            }
            ("_", false) => {
                let c = count.unwrap_or(1);
                let t = self.tabs.get(self.active_tab)
                    .map(|tab| underscore_motion(&tab.content, tab.cursor, c))
                    .unwrap_or(0);
                MotionResolution::Resolved { target: t, kind: MotionKind::Linewise }
            }
            ("g", true)  => {
                // `G` — no count means "last line" (sentinel usize::MAX,
                // which `line_offset`'s own clamp-on-overrun handles),
                // unlike `gg`'s "no count means line 1" above.
                let line = count.unwrap_or(usize::MAX);
                if let Some(tab) = self.tabs.get(self.active_tab) {
                    let start = line_offset(&tab.content, line.saturating_sub(1));
                    let target = first_nonblank(&tab.content, start);
                    MotionResolution::Resolved { target, kind: MotionKind::Linewise }
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
            (";", false) if key_char != Some(":") => {
                match self.resolve_repeat_find(false) {
                    Some((target, kind)) => MotionResolution::Resolved { target, kind: find_kind_to_motion_kind(kind) },
                    None => MotionResolution::Pending,
                }
            }
            (",", false) => {
                match self.resolve_repeat_find(true) {
                    Some((target, kind)) => MotionResolution::Resolved { target, kind: find_kind_to_motion_kind(kind) },
                    None => MotionResolution::Pending,
                }
            }
            ("left", _)  => { let t = self.repeat_motion(1, char_left); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("right", _) => { let t = self.repeat_motion(1, char_right); MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar } }
            ("home", _) => {
                let t = self.tabs.get(self.active_tab).map(|tab| line_start(&tab.content, tab.cursor)).unwrap_or(0);
                MotionResolution::Resolved { target: t, kind: MotionKind::ExclusiveChar }
            }
            ("end", _) => {
                let t = self.tabs.get(self.active_tab).map(|tab| line_end(&tab.content, tab.cursor)).unwrap_or(0);
                MotionResolution::Resolved { target: t, kind: MotionKind::InclusiveChar }
            }
            ("up", _) | ("down", _) | ("j", false) | ("k", false) => MotionResolution::NeedsGpui,
            _ => MotionResolution::NotAMotion,
        }
    }

    fn apply_vim_motion(&mut self, extend: bool, target: usize) -> bool {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if extend {
                extend_selection(tab, target);
            } else {
                if line_index_for(&tab.content, target).abs_diff(line_index_for(&tab.content, tab.cursor)) > 1 {
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if let Some(pos) = tab.vim_jump_back.pop() {
            tab.vim_jump_forward.push(tab.cursor);
            tab.cursor = pos.min(tab.content.len());
            tab.selection = None;
        }
    }

    pub fn vim_jump_forward(&mut self) {
        /*
         * `Ctrl+i` (spec 5.5): the reverse of `vim_jump_backward`.
         */
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if let Some(pos) = tab.vim_jump_forward.pop() {
            tab.vim_jump_back.push(tab.cursor);
            tab.cursor = pos.min(tab.content.len());
            tab.selection = None;
        }
    }

    // ── Operators: d/y/c + dd/yy/cc (spec 5.3) ───────────────────────────────────

    fn is_pending_g_case_trigger(&mut self, pending_trigger: Option<char>, key: &str) -> bool {
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

    fn try_handle_vim_register_prefix(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return false };
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

    fn take_vim_selected_register(&mut self) -> char {
        self.tabs.get_mut(self.active_tab).and_then(|t| t.vim_selected_register.take()).unwrap_or('"')
    }

    fn write_vim_register(&mut self, text: String, also_yank: bool) {
        /*
         * The single place any operator's removed/copied text lands in
         * `registers`: always the default (`'"'`) and, for `y`, also the
         * yank register (`'0'`) — mirroring real vim, whatever register
         * was explicitly named still updates `'"'` too. If the named
         * register was `'+'`, stages `pending_clipboard_sync` so
         * `text_editor.rs` can push it onto the real OS clipboard (needs
         * `cx`, which this file doesn't have).
         */
        let selected = self.take_vim_selected_register();
        self.registers.insert('"', text.clone());
        if also_yank {
            self.registers.insert('0', text.clone());
        }
        if selected != '"' {
            self.registers.insert(selected, text.clone());
            if selected == '+' {
                self.pending_clipboard_sync = Some(text);
            }
        }
    }

    fn vim_paste_register(&mut self, before: bool) {
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
        let Some(text) = self.registers.get(&register).cloned() else { return };
        if text.is_empty() { return; }
        self.push_undo_snapshot();
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if text.ends_with('\n') {
            let insert_at = if before {
                line_start(&tab.content, tab.cursor)
            } else {
                let end = line_end(&tab.content, tab.cursor);
                if end < tab.content.len() { end + 1 } else { tab.content.len() }
            };
            let needs_leading_newline = insert_at == tab.content.len() && !tab.content.is_empty() && !tab.content.ends_with('\n');
            let insertion = if needs_leading_newline { format!("\n{}", text) } else { text };
            sync_insert_str(&mut tab.paragraphs, insert_at, &insertion);
            tab.content.insert_str(insert_at, &insertion);
            let landing_start = insert_at + if needs_leading_newline { 1 } else { 0 };
            tab.cursor = first_nonblank(&tab.content, landing_start);
        } else {
            let at = if before { tab.cursor } else { char_right(&tab.content, tab.cursor) };
            sync_insert_str(&mut tab.paragraphs, at, &text);
            tab.content.insert_str(at, &text);
            let last_char_start = text.char_indices().last().map(|(i, _)| i).unwrap_or(0);
            tab.cursor = at + last_char_start;
        }
        tab.is_modified = true;
    }

    fn vim_delete_char_forward(&mut self) {
        /*
         * `x` (spec 5.5): deletes the character under the cursor, writing
         * it to the register like any `d`. Clamped to the current line —
         * real vim's `x` never deletes the trailing newline (an empty
         * line, or a cursor already at the line's end, is a no-op), unlike
         * `dl`'s more general motion-based range.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let end = char_right(&tab.content, tab.cursor).min(line_end(&tab.content, tab.cursor));
        if end == tab.cursor { return; }
        let start = tab.cursor;
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, false);
    }

    fn vim_delete_char_backward(&mut self) {
        /*
         * `X` (spec 5.5): deletes the character before the cursor, clamped
         * to the current line's start (a no-op at column 0).
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let start = char_left(&tab.content, tab.cursor).max(line_start(&tab.content, tab.cursor));
        if start == tab.cursor { return; }
        let text = self.delete_vim_range(start, tab.cursor);
        self.write_vim_register(text, false);
    }

    fn vim_substitute_char(&mut self) {
        /*
         * `s` (spec 5.5): `x` immediately followed by entering Insert —
         * real vim's shorthand for "delete this one character, then type
         * its replacement".
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let end = char_right(&tab.content, tab.cursor).min(line_end(&tab.content, tab.cursor));
        let text = self.delete_vim_range(tab.cursor, end);
        self.write_vim_register(text, false);
        self.vim_enter_insert_before_cursor();
    }

    fn vim_substitute_line(&mut self) {
        /*
         * `S` (spec 5.5): clears the current line's content (not the
         * trailing newline — same as `cc`) and enters Insert at its start.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let start = line_start(&tab.content, tab.cursor);
        let end = line_end(&tab.content, tab.cursor);
        let text = self.delete_vim_range(start, end);
        self.write_vim_register(text, false);
        self.vim_enter_insert_before_cursor();
    }

    fn vim_toggle_case_char(&mut self) {
        /*
         * `~` (spec 5.5): toggles the case of the character under the
         * cursor and advances the cursor, reusing `toggle_case_vim_range`
         * (built for Visual mode's `~`). Clamped to the current line, same
         * no-op-at-EOL reasoning as `x`.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let end = char_right(&tab.content, tab.cursor).min(line_end(&tab.content, tab.cursor));
        if end == tab.cursor { return; }
        let start = tab.cursor;
        self.toggle_case_vim_range(start, end);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = end; }
    }

    fn vim_replace_char(&mut self, replacement: char) {
        /*
         * The completion half of `r<char>` (spec 5.5): overwrites the
         * character under the cursor with `replacement` and leaves the
         * cursor in place (unlike `x`/`s`, real vim's `r` doesn't move
         * it). No-op on an empty line (nothing under the cursor to
         * replace), and — unlike every `d`/`y`/`c` operator — never
         * touches any register, matching real vim.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let end = char_right(&tab.content, tab.cursor).min(line_end(&tab.content, tab.cursor));
        if end == tab.cursor { return; }
        let cursor = tab.cursor;
        self.replace_vim_range(cursor, end, |_| replacement.to_string());
        if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = cursor; }
    }

    fn vim_join_lines(&mut self) {
        /*
         * `J` (spec 5.5): joins the current line with the next, replacing
         * the newline and the next line's leading spaces/tabs with a
         * single space. A no-op on the last line. Simplified vs. real
         * vim's full behavior (no special-casing for lines already ending
         * in whitespace, or a next line starting with `)`).
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let line_end_pos = line_end(&tab.content, tab.cursor);
        if line_end_pos >= tab.content.len() { return; }
        let next_line_start = line_end_pos + 1;
        let next_line_end = line_end(&tab.content, next_line_start);
        let trimmed_start = tab.content[next_line_start..next_line_end]
            .char_indices()
            .find(|(_, c)| *c != ' ' && *c != '\t')
            .map(|(i, _)| next_line_start + i)
            .unwrap_or(next_line_end);
        self.replace_vim_range(line_end_pos, trimmed_start, |_| " ".to_string());
        if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = line_end_pos; }
    }

    fn start_vim_operator(&mut self, operator: char) {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_pending_operator = Some(operator);
        }
        // `.` repeat (spec 5.5): starts capturing this operator's
        // completion keystrokes, unless it's `y` — yanking doesn't modify
        // the document, so it isn't a "change" `.` should repeat.
        if operator != 'y' {
            self.vim_change_recording = Some(Vec::new());
        }
    }

    fn clear_vim_pending_operator(&mut self) {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.vim_pending_operator = None;
            tab.vim_pending_text_object_prefix = None;
        }
    }

    fn complete_vim_operator(&mut self, operator: char, key: &str, shift: bool, key_char: Option<&str>) -> bool {
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
        if let Some(inner) = self.tabs.get(self.active_tab).and_then(|t| t.vim_pending_text_object_prefix) {
            self.clear_vim_pending_operator();
            if let Some(object_char) = vim_find_target_char(key, shift, key_char) {
                let Some(tab) = self.tabs.get(self.active_tab) else { return true };
                if let Some((start, end)) = resolve_vim_text_object(&tab.content, tab.cursor, object_char, inner) {
                    self.execute_vim_operator_range(operator, start, end, MotionKind::ExclusiveChar);
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
            let Some(tab) = self.tabs.get(self.active_tab) else { return true };
            let (start, end) = vim_operator_doubled_range(operator, tab.cursor, count, &tab.content);
            self.execute_vim_operator_range(operator, start, end, MotionKind::Linewise);
            return true;
        }

        if (key == "i" || key == "a") && !shift {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.vim_pending_text_object_prefix = Some(key == "i");
            }
            return true;
        }

        match self.resolve_vim_motion(key, shift, key_char) {
            MotionResolution::Pending => true,
            MotionResolution::Resolved { target, kind } => {
                self.clear_vim_pending_operator();
                let Some(tab) = self.tabs.get(self.active_tab) else { return true };
                let (start, end) = vim_operator_motion_range(operator, tab.cursor, target, kind, &tab.content);
                self.execute_vim_operator_range(operator, start, end, kind);
                true
            }
            MotionResolution::NeedsGpui | MotionResolution::NotAMotion => {
                self.clear_vim_pending_operator();
                // An invalid/unsupported motion abandons the operator
                // (spec 5.3) — nothing ran, so there's no change for `.`
                // to remember.
                self.vim_change_recording = None;
                true
            }
        }
    }

    fn execute_vim_operator_range(&mut self, operator: char, start: usize, end: usize, kind: MotionKind) {
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
        match operator {
            'd' => {
                let text = self.delete_vim_range(start, end);
                self.write_vim_register(text, false);
            }
            'y' => {
                let Some(tab) = self.tabs.get(self.active_tab) else { return };
                let text = tab.content[start..end].to_string();
                let landing = if kind == MotionKind::Linewise { first_nonblank(&tab.content, start) } else { start };
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    tab.cursor = landing;
                    tab.selection = None;
                }
                self.write_vim_register(text, true);
            }
            'c' => {
                let text = self.delete_vim_range(start, end);
                self.write_vim_register(text, false);
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
        if let Some(keys) = self.vim_change_recording.take() {
            if operator == 'c' {
                self.vim_pending_change_before_insert = Some((operator, keys));
            } else {
                self.last_change = Some(VimChange::Operator(operator, keys));
            }
        }
    }

    fn replace_vim_range(&mut self, start: usize, end: usize, transform: impl FnOnce(&str) -> String) -> String {
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return String::new() };
        let original = tab.content[start..end].to_string();
        let replacement = transform(&original);
        // Every operator that rewrites a range this way (d/c/x/s/>/</gU/gu/
        // ~/r/J) gets its formatting kept in sync for free via this one
        // choke point — reduces to the same delete+insert primitives every
        // other mutation site uses.
        sync_delete_range(&mut tab.paragraphs, start, end);
        sync_insert_str(&mut tab.paragraphs, start, &replacement);
        tab.content.replace_range(start..end, &replacement);
        tab.selection = None;
        tab.is_modified = true;
        original
    }

    fn indent_vim_range(&mut self, start: usize, end: usize, indent: bool) {
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
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.cursor = first_nonblank(&tab.content, start);
        }
    }

    fn change_case_vim_range(&mut self, start: usize, end: usize, upper: bool) {
        /*
         * `gU`/`gu`: upper/lowercases `content[start..end]` in place.
         * `String::to_uppercase`/`to_lowercase` are UTF-8-aware and may
         * change the byte length (e.g. German `ß` -> `SS`) —
         * `replace_vim_range`'s `replace_range` call handles that
         * correctly, same as every other operator mutation here.
         */
        self.replace_vim_range(start, end, |segment| {
            if upper { segment.to_uppercase() } else { segment.to_lowercase() }
        });
        if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = start; }
    }

    fn delete_vim_range(&mut self, start: usize, end: usize) -> String {
        /*
         * The shared mutation for `d`/`c`: removes `content[start..end]`
         * and leaves the cursor at `start` — mirrors `delete_selection_
         * raw`'s undo/is_modified handling but over an explicit range
         * instead of `tab.selection`. Returns the removed text so the
         * caller can write it to a register.
         */
        let text = self.replace_vim_range(start, end, |_| String::new());
        if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = start; }
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
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let start = line_offset(&tab.content, line);
            let target = first_nonblank(&tab.content, start);
            self.apply_vim_motion(extend, target);
        }
    }

    fn repeat_motion(&self, count: usize, motion: fn(&str, usize) -> usize) -> usize {
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return 0 };
        let mut pos = tab.cursor;
        for _ in 0..count {
            pos = motion(&tab.content, pos);
        }
        pos
    }

    fn resolve_repeat_find(&self, reverse: bool) -> Option<(usize, char)> {
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
        let tab = self.tabs.get(self.active_tab)?;
        let (kind, target_char) = tab.last_find?;
        let kind = if reverse {
            match kind { 'f' => 'F', 'F' => 'f', 't' => 'T', 'T' => 't', k => k }
        } else {
            kind
        };
        resolve_find_with_nudge(&tab.content, tab.cursor, kind, target_char, true).map(|pos| (pos, kind))
    }

    fn handle_vim_visual_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return true };
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
                (VimMode::Visual, "v", false) => { self.vim_exit_to_normal(); return true; }
                (VimMode::VisualLine, "v", true) => { self.vim_exit_to_normal(); return true; }
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

        match self.handle_vim_motion_key(key, shift, key_char, true) {
            Some(result) => result,
            None => true,
        }
    }

    fn vim_visual_operator_range(&self, operator: char) -> Option<(usize, usize, MotionKind)> {
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
        let tab = self.tabs.get(self.active_tab)?;
        let (a, f) = tab.selection?;
        let (min, max) = (a.min(f), a.max(f));
        if tab.vim_mode != VimMode::VisualLine && !operator_forces_linewise(operator) {
            return Some((min, max, MotionKind::ExclusiveChar));
        }
        let last_included = if max > min { max - 1 } else { min };
        let start = line_start(&tab.content, min);
        let line_end_pos = line_end(&tab.content, last_included);
        let (start, end) = linewise_bounds_for_operator(operator, start, line_end_pos, &tab.content);
        Some((start, end, MotionKind::Linewise))
    }

    fn execute_vim_visual_operator(&mut self, operator: char) {
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
        let Some((start, end, kind)) = self.vim_visual_operator_range(operator) else { return };
        if operator == '~' {
            self.toggle_case_vim_range(start, end);
        } else {
            self.execute_vim_operator_range(operator, start, end, kind);
        }
        if operator != 'c' {
            self.vim_exit_to_normal();
        }
    }

    fn toggle_case_vim_range(&mut self, start: usize, end: usize) {
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
            segment.chars().map(|c| {
                if c.is_uppercase() {
                    c.to_lowercase().next().unwrap_or(c)
                } else if c.is_lowercase() {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                }
            }).collect()
        });
        if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = start; }
    }

    fn vim_visual_swap_ends(&mut self) {
        /*
         * `o` (spec 5.6): swaps the selection's anchor and focus, moving
         * the cursor to what was previously the anchor — the highlighted
         * range itself doesn't change, only which end the cursor now sits
         * on (so a following motion extends from the *other* side).
         */
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some((a, f)) = tab.selection {
                tab.selection = Some((f, a));
                tab.cursor = a;
            }
        }
    }

    fn capture_vim_line_input(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> VimLineInput {
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
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.vim_command_line.clear();
            }
            return VimLineInput::Cancelled;
        }
        if key == "enter" {
            let line = self.tabs.get(self.active_tab).map(|t| t.vim_command_line.clone()).unwrap_or_default();
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.vim_command_line.clear();
            }
            return VimLineInput::Dispatch(line);
        }
        if key == "backspace" {
            let Some(tab) = self.tabs.get_mut(self.active_tab) else { return VimLineInput::Consumed };
            if tab.vim_command_line.pop().is_none() {
                return VimLineInput::Cancelled;
            }
            return VimLineInput::Consumed;
        }
        if let Some(c) = vim_find_target_char(key, shift, key_char) {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.vim_command_line.push(c);
            }
        }
        VimLineInput::Consumed
    }

    fn handle_vim_command_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        match self.capture_vim_line_input(key, shift, key_char) {
            VimLineInput::Dispatch(line) => {
                self.dispatch_vim_command(&line);
                self.vim_exit_to_normal();
            }
            VimLineInput::Cancelled => self.vim_exit_to_normal(),
            VimLineInput::Consumed => {}
        }
    }

    fn handle_vim_search_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
        match self.capture_vim_line_input(key, shift, key_char) {
            VimLineInput::Dispatch(pattern) => {
                let forward = self.tabs.get(self.active_tab).map(|t| t.vim_search_direction).unwrap_or(true);
                self.dispatch_vim_search(&pattern, forward);
                self.vim_exit_to_normal();
            }
            VimLineInput::Cancelled => self.vim_exit_to_normal(),
            VimLineInput::Consumed => {}
        }
    }

    fn dispatch_vim_command(&mut self, line: &str) {
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
            if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                tab.vim_command_error = Some(msg);
            }
        };

        match line {
            "w" => {
                if let Err(e) = self.save_active_tab() { set_error(self, e); }
            }
            "wa" => {
                let mut errors = Vec::new();
                for idx in 0..self.tabs.len() {
                    if let Err(e) = self.save_tab(idx) { errors.push(e); }
                }
                if let Some(e) = errors.into_iter().next() { set_error(self, e); }
            }
            "q" => {
                let modified = self.tabs.get(self.active_tab).map(|t| t.is_modified).unwrap_or(false);
                if modified {
                    set_error(self, "E37: No write since last change".to_string());
                } else {
                    self.close_tab(self.active_tab);
                }
            }
            "q!" => self.close_tab(self.active_tab),
            "wq" | "x" => {
                if let Err(e) = self.save_active_tab() { set_error(self, e); return; }
                self.close_tab(self.active_tab);
            }
            "set vim" => self.vim_enabled = true,
            "set novim" => self.vim_enabled = false,
            "noh" => {} // nothing to clear yet — Task I adds search highlighting
            _ => {
                if let Some(path) = line.strip_prefix("e ") {
                    let path = self.working_directory.join(path.trim());
                    self.open_file(path);
                } else if let Some(count) = line.parse::<usize>().ok() {
                    if count >= 1 {
                        self.vim_move_to_line_first_nonblank(count - 1, false);
                    }
                } else if let Some(rest) = line.strip_prefix("%s") {
                    if let Err(e) = self.dispatch_vim_substitute(rest) { set_error(self, e); }
                } else {
                    set_error(self, format!("E492: Not an editor command: {}", line));
                }
            }
        }
    }

    fn vim_repeat_last_change(&mut self) {
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
        let Some(change) = self.last_change.clone() else { return };
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

    fn dispatch_vim_search(&mut self, pattern: &str, forward: bool) {
        /*
         * `/pattern<Enter>` / `?pattern<Enter>` (spec 5.5). A minimal
         * `content.find`/`rfind`-with-wraparound, not a regex search — per
         * `vim_todo.md`'s explicit guidance, since a full inline find-bar
         * (highlighting, incremental search) is spec 4.6 territory and out
         * of scope here. Remembers the pattern+direction so `n`/`N` can
         * repeat it.
         */
        if pattern.is_empty() { return; }
        self.last_search = Some((pattern.to_string(), forward));
        let cursor = self.tabs.get(self.active_tab).map(|t| t.cursor).unwrap_or(0);
        self.jump_to_search_match_from(pattern, forward, cursor);
    }

    fn jump_to_search_match_from(&mut self, pattern: &str, forward: bool, from: usize) {
        /*
         * The shared search-and-jump core: searches `pattern` starting
         * just past (`forward`) or just before (backward) `from` — never
         * matching a position the caller is already standing at — and
         * wraps around the whole document if nothing is found in that
         * direction, matching real vim's default `wrapscan` behavior.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let content = &tab.content;
        let found = if forward {
            let start = char_right(content, from);
            content[start..].find(pattern).map(|i| start + i)
                .or_else(|| content.find(pattern))
        } else {
            content[..from].rfind(pattern)
                .or_else(|| content.rfind(pattern))
        };
        if let Some(pos) = found {
            self.apply_vim_motion(false, pos);
        }
    }

    fn vim_search_next(&mut self, reverse: bool) {
        /*
         * `n`/`N` (spec 5.5): repeats the last `/`/`?`/`*`/`#` search.
         * `N` (`reverse`) searches the opposite direction from the one
         * originally used, matching real vim.
         */
        let Some((pattern, forward)) = self.last_search.clone() else { return };
        let effective_forward = if reverse { !forward } else { forward };
        let cursor = self.tabs.get(self.active_tab).map(|t| t.cursor).unwrap_or(0);
        self.jump_to_search_match_from(&pattern, effective_forward, cursor);
    }

    fn vim_search_word_under_cursor(&mut self, forward: bool) {
        /*
         * `*`/`#` (spec 5.5): searches for the literal word under the
         * cursor (reusing Task F's `text_object_word`), starting from
         * just past its end (`*`) or just before its start (`#`) so the
         * word the cursor is already standing in doesn't match itself.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let (start, end) = text_object_word(&tab.content, tab.cursor, true);
        if start == end { return; }
        let word = tab.content[start..end].to_string();
        self.last_search = Some((word.clone(), forward));
        let from = if forward { end } else { start };
        self.jump_to_search_match_from(&word, forward, from);
    }

    fn dispatch_vim_substitute(&mut self, rest: &str) -> Result<(), String> {
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

        let pattern_src = if case_insensitive { format!("(?i){}", pattern) } else { pattern.to_string() };
        let re = regex::Regex::new(&pattern_src).map_err(|e| format!("E486: {}", e))?;

        let Some(tab) = self.tabs.get(self.active_tab) else { return Ok(()) };
        let old_lines: Vec<String> = tab.content.split('\n').map(|l| l.to_string()).collect();
        let new_lines: Vec<String> = old_lines.iter()
            .map(|l| if global { re.replace_all(l, replacement).into_owned() } else { re.replace(l, replacement).into_owned() })
            .collect();
        let new_content = new_lines.join("\n");

        if new_content != tab.content {
            self.push_undo_snapshot();
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                // Formatting sync scope limit (rich-text formatting plan,
                // Phase 1): a regex substitution has no clean per-character
                // mapping back to the original runs, so any paragraph whose
                // text actually changed gets replaced with a single default
                // (unformatted) run. Paragraphs the substitution didn't
                // touch keep their existing runs exactly.
                for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
                    if old != new {
                        if let Some(para) = tab.paragraphs.get_mut(i) {
                            para.runs = vec![Run { text: new.clone(), ..Run::default() }];
                        }
                    }
                }
                tab.content = new_content;
                tab.is_modified = true;
                tab.cursor = tab.cursor.min(tab.content.len());
                tab.selection = None;
            }
        }
        Ok(())
    }

    fn handle_vim_replace_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) {
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
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.cursor = char_left(&tab.content, tab.cursor);
            }
            return;
        }
        if let Some(c) = vim_find_target_char(key, shift, key_char) {
            self.vim_replace_mode_type_char(c);
        }
    }

    fn vim_replace_mode_type_char(&mut self, c: char) {
        /*
         * Overwrites the character under the cursor with `c` and advances
         * past it — or, once the cursor reaches the end of the line (or
         * document), appends instead, since there's nothing left to
         * overwrite (matches real vim: Replace mode can extend a line's
         * length by typing past its original end).
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        if tab.cursor < line_end(&tab.content, tab.cursor) {
            let end = char_right(&tab.content, tab.cursor);
            let cursor = tab.cursor;
            self.replace_vim_range(cursor, end, |_| c.to_string());
            if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.cursor = cursor + c.len_utf8(); }
        } else {
            self.insert_char(c);
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

fn line_index_for(content: &str, pos: usize) -> usize {
    /*
     * 0-indexed line number containing byte offset `pos` — counts the
     * newlines before it. Used by `apply_vim_motion`'s jump-list push
     * heuristic (spec 5.5's `Ctrl+o`/`Ctrl+i`): a motion is "large" if it
     * crosses more than one line.
     */
    content[..line_start(content, pos)].matches('\n').count()
}

fn first_nonblank(content: &str, pos: usize) -> usize {
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

fn underscore_motion(content: &str, pos: usize, count: usize) -> usize {
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
        if end >= content.len() { break; }
        start = end + 1;
    }
    first_nonblank(content, start)
}

fn operator_forces_linewise(operator: char) -> bool {
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

fn linewise_bounds_for_operator(operator: char, start: usize, end: usize, content: &str) -> (usize, usize) {
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

fn vim_operator_motion_range(operator: char, cursor: usize, target: usize, kind: MotionKind, content: &str) -> (usize, usize) {
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
    let kind = if operator_forces_linewise(operator) { MotionKind::Linewise } else { kind };
    let (min, max) = if cursor <= target { (cursor, target) } else { (target, cursor) };
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

fn vim_operator_doubled_range(operator: char, cursor: usize, count: usize, content: &str) -> (usize, usize) {
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
        if line_end_pos >= content.len() { break; }
        end_pos = line_end_pos + 1;
    }
    let end = line_end(content, end_pos);
    linewise_bounds_for_operator(operator, start, end, content)
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

fn split_vim_command_buf(buf: &str) -> (Option<usize>, Option<char>) {
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
    let count = if digit_part.is_empty() { None } else { digit_part.parse::<usize>().ok() };
    (count, trigger)
}

/// The three character classes vim's word motions distinguish: alphanumeric
/// "word" characters, standalone "punctuation" characters (each run of
/// punctuation is its own word), and whitespace (never part of a word).
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum CharClass {
    Word,
    Punct,
    Space,
}

fn char_class(c: char) -> CharClass {
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

fn big_word_class(c: char) -> CharClass {
    /*
     * Classifies a character for vim `W`/`B`/`E` WORD-motion purposes: only
     * whitespace vs. non-whitespace matters — a WORD is any
     * whitespace-delimited run, punctuation included, unlike `char_class`'s
     * additional word/punctuation split. Never produces `CharClass::Punct`;
     * shares the enum with `char_class` purely so both can drive the same
     * `word_forward`/`word_end`/`word_backward` implementations.
     */
    if c.is_whitespace() { CharClass::Space } else { CharClass::Word }
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
    // vim `w`.
    word_forward_classified(content, pos, char_class)
}

fn word_forward_big(content: &str, pos: usize) -> usize {
    // vim `W`.
    word_forward_classified(content, pos, big_word_class)
}

fn word_forward_classified(content: &str, pos: usize, classify: fn(char) -> CharClass) -> usize {
    /*
     * Byte offset of the start of the next word after `pos`, per `classify`
     * (`char_class` for vim's `w`, `big_word_class` for `W`). Skips the
     * rest of the current char-class run, then skips whitespace (crossing
     * newlines freely) to land on the first character of the following word.
     */
    if pos >= content.len() { return pos; }
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

fn word_end(content: &str, pos: usize) -> usize {
    // vim `e`.
    word_end_classified(content, pos, char_class)
}

fn word_end_big(content: &str, pos: usize) -> usize {
    // vim `E`.
    word_end_classified(content, pos, big_word_class)
}

fn word_end_classified(content: &str, pos: usize, classify: fn(char) -> CharClass) -> usize {
    /*
     * Byte offset of the last character of the current word (if the cursor
     * isn't already there) or of the next word (if it is), per `classify`.
     */
    if pos >= content.len() { return pos; }
    let cur_char = content[pos..].chars().next().unwrap();
    let cur_class = classify(cur_char);
    let next_idx = pos + cur_char.len_utf8();
    let next_class = (next_idx < content.len())
        .then(|| classify(content[next_idx..].chars().next().unwrap()));
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
    let run_class = classify(content[i..].chars().next().unwrap());
    let mut last = i;
    for (off, c) in content[i..].char_indices() {
        if classify(c) != run_class { break; }
        last = i + off;
    }
    last
}

fn word_backward(content: &str, pos: usize) -> usize {
    // vim `b`.
    word_backward_classified(content, pos, char_class)
}

fn word_backward_big(content: &str, pos: usize) -> usize {
    // vim `B`.
    word_backward_classified(content, pos, big_word_class)
}

fn word_backward_classified(content: &str, pos: usize, classify: fn(char) -> CharClass) -> usize {
    /*
     * Byte offset of the start of the current word (if the cursor is
     * mid-word) or of the previous word (if it's at a word's start
     * already), per `classify`.
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
    let class = classify(content[i..].chars().next().unwrap());
    loop {
        if i == 0 { break; }
        let prev = content[..i].char_indices().last().map(|(idx, _)| idx).unwrap_or(0);
        if classify(content[prev..].chars().next().unwrap()) != class { break; }
        i = prev;
    }
    i
}

fn is_blank_line(content: &str, line_start_pos: usize) -> bool {
    /*
     * True when the line starting at `line_start_pos` has zero characters
     * before its terminating '\n' (or the document's end) — vim's
     * paragraph-boundary definition (spec 5.2's `{`/`}`).
     */
    line_start_pos == line_end(content, line_start_pos)
}

fn paragraph_forward(content: &str, pos: usize) -> usize {
    /*
     * vim `}`: byte offset of the start of the next blank line after
     * `pos`'s line, or `content.len()` if there is none. Always searches
     * strictly *after* the current line, even when the cursor already sits
     * on a blank line — `}` never stays put, it advances to a *later*
     * paragraph boundary.
     */
    let mut end = line_end(content, pos);
    loop {
        if end >= content.len() { return content.len(); }
        let next_start = end + 1; // skip the '\n'
        if is_blank_line(content, next_start) {
            return next_start;
        }
        end = line_end(content, next_start);
    }
}

fn paragraph_backward(content: &str, pos: usize) -> usize {
    /*
     * vim `{`: byte offset of the start of the previous blank line before
     * `pos`'s line, or `0` if there is none. Always searches strictly
     * *before* the current line, mirroring `paragraph_forward`.
     */
    let mut start = line_start(content, pos);
    loop {
        if start == 0 { return 0; }
        let prev_end = start - 1; // the '\n' ending the previous line
        let prev_start = line_start(content, prev_end);
        if is_blank_line(content, prev_start) {
            return prev_start;
        }
        start = prev_start;
    }
}

// ── Text objects (spec 5.4): iw/aw, is/as, ip/ap, i"/a", i'/a', brackets ────────

fn resolve_vim_text_object(content: &str, cursor: usize, object_char: char, inner: bool) -> Option<(usize, usize)> {
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

fn char_class_run_start(content: &str, cursor: usize, class: CharClass) -> usize {
    /*
     * Byte offset of the start of the contiguous run of `class`-classified
     * characters containing `cursor`, scanning backward. `cursor` itself
     * must already be within such a run (the caller checks this via
     * `char_class` on the character at `cursor`).
     */
    let mut start = cursor;
    for (i, c) in content[..cursor].char_indices().rev() {
        if char_class(c) != class { break; }
        start = i;
    }
    start
}

fn char_class_run_end(content: &str, cursor: usize, class: CharClass) -> usize {
    /*
     * Exclusive byte offset just past the contiguous run of
     * `class`-classified characters containing `cursor`, scanning forward.
     */
    let mut end = cursor;
    for (i, c) in content[cursor..].char_indices() {
        if char_class(c) != class { break; }
        end = cursor + i + c.len_utf8();
    }
    end
}

fn text_object_word(content: &str, cursor: usize, inner: bool) -> (usize, usize) {
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
    if end < content.len() && char_class(content[end..].chars().next().unwrap()) == CharClass::Space {
        (start, char_class_run_end(content, end, CharClass::Space))
    } else if start > 0 && char_class(content[..start].chars().next_back().unwrap()) == CharClass::Space {
        (char_class_run_start(content, start - 1, CharClass::Space), end)
    } else {
        (start, end)
    }
}

fn is_sentence_end_punct(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

fn text_object_sentence(content: &str, cursor: usize, inner: bool) -> Option<(usize, usize)> {
    /*
     * vim `is`/`as`, simplified: a sentence ends at the first `.`/`!`/`?`
     * followed by whitespace or end-of-content (no handling of
     * abbreviations, decimal numbers, or quote/paren-wrapped punctuation —
     * a documented simplification of vim's own, more elaborate sentence
     * grammar). `is` is the sentence containing `cursor`; `as` additionally
     * swallows the whitespace run up to the next sentence's start.
     */
    if content.is_empty() { return None; }
    let cursor = cursor.min(content.len());

    let mut end = None;
    for (i, c) in content[cursor..].char_indices() {
        if is_sentence_end_punct(c) {
            let after = cursor + i + c.len_utf8();
            let boundary = after >= content.len()
                || content[after..].chars().next().map(|c| c.is_whitespace()).unwrap_or(true);
            if boundary { end = Some(after); break; }
        }
    }
    let end = end.unwrap_or(content.len());

    let mut start = 0;
    for (i, c) in content[..cursor].char_indices().rev() {
        if is_sentence_end_punct(c) {
            let after = i + c.len_utf8();
            let boundary = after >= content.len()
                || content[after..].chars().next().map(|c| c.is_whitespace()).unwrap_or(true);
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

fn paragraph_block_start(content: &str, from_line_start: usize, want_blank: bool) -> usize {
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
        if is_blank_line(content, prev_start) != want_blank { break; }
        start = prev_start;
    }
    start
}

fn paragraph_block_end(content: &str, from_line_end: usize, want_blank: bool) -> usize {
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
        if is_blank_line(content, next_start) != want_blank { break; }
        end = line_end(content, next_start);
    }
    end
}

fn text_object_paragraph(content: &str, cursor: usize, inner: bool) -> Option<(usize, usize)> {
    /*
     * vim `ip`/`ap`: a paragraph is a blank-line-delimited block (the same
     * definition `{`/`}` use, spec 5.2, via `is_blank_line`). `ip` is the
     * contiguous run of lines sharing the cursor line's blank/non-blank
     * status; `ap` additionally swallows one adjacent block of the
     * *opposite* status — trailing preferred, falling back to leading —
     * mirroring `aw`'s whitespace-inclusion rule at paragraph granularity.
     */
    if content.is_empty() { return None; }
    let cur_line_start = line_start(content, cursor);
    let blank = is_blank_line(content, cur_line_start);

    let mut start = paragraph_block_start(content, cur_line_start, blank);
    let block_end = paragraph_block_end(content, line_end(content, cur_line_start), blank);
    let mut end = if block_end < content.len() { block_end + 1 } else { block_end };

    if !inner {
        if end < content.len() {
            let trail_end = paragraph_block_end(content, line_end(content, end), !blank);
            end = if trail_end < content.len() { trail_end + 1 } else { trail_end };
        } else if start > 0 {
            start = paragraph_block_start(content, start, !blank);
        }
    }
    Some((start, end))
}

fn text_object_quote(content: &str, cursor: usize, quote: char, inner: bool) -> Option<(usize, usize)> {
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

fn text_object_bracket(content: &str, cursor: usize, open: char, close: char, inner: bool) -> Option<(usize, usize)> {
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

fn find_char_forward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `f<char>`: byte offset of the next occurrence of `target` on the
     * current line, searching strictly after `pos`. `None` if the current
     * line has no later occurrence — `f`/`t` never cross a line boundary.
     */
    let end = line_end(content, pos);
    if pos >= end { return None; }
    let search_from = char_right(content, pos);
    content[search_from..end]
        .char_indices()
        .find(|(_, c)| *c == target)
        .map(|(i, _)| search_from + i)
}

fn find_char_backward(content: &str, pos: usize, target: char) -> Option<usize> {
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

fn till_char_forward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `t<char>`: byte offset one character before the next occurrence
     * of `target` on the current line. A no-op (returns `pos`, wrapped in
     * `Some`) when `target` is the character immediately after `pos` — vim's
     * `t` never lands past its own starting position.
     */
    find_char_forward(content, pos, target).map(|found| char_left(content, found))
}

fn till_char_backward(content: &str, pos: usize, target: char) -> Option<usize> {
    /*
     * vim `T<char>`: byte offset one character after the previous
     * occurrence of `target` on the current line.
     */
    find_char_backward(content, pos, target).map(|found| char_right(content, found))
}

fn resolve_find(content: &str, pos: usize, kind: char, target: char) -> Option<usize> {
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

fn resolve_find_with_nudge(content: &str, cursor: usize, kind: char, target: char, nudge: bool) -> Option<usize> {
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

fn resolve_vim_visual_operator_key(key: &str, shift: bool, key_char: Option<&str>) -> Option<char> {
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
    if (key == "d" || key == "x") && !shift { return Some('d'); }
    if key == "y" && !shift { return Some('y'); }
    if key == "c" && !shift { return Some('c'); }
    if matches_shifted_symbol(key, shift, key_char, ".", ">") { return Some('>'); }
    if matches_shifted_symbol(key, shift, key_char, ",", "<") { return Some('<'); }
    if matches_shifted_symbol(key, shift, key_char, "`", "~") { return Some('~'); }
    None
}

pub(crate) fn matches_shifted_symbol(key: &str, shift: bool, key_char: Option<&str>, unshifted_key: &str, symbol: &str) -> bool {
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

fn find_kind_to_motion_kind(kind: char) -> MotionKind {
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
     */
    if let Some(kc) = key_char.and_then(|s| s.chars().next()) {
        return Some(kc);
    }
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() { return None; }
    Some(if shift && c.is_alphabetic() { c.to_ascii_uppercase() } else { c })
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
                paragraphs: default_paragraphs(),
                docx_origin: None,
                pending_format: None,
                cursor,
                selection,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
                last_edit_at: None,
                vim_mode: VimMode::Normal,
                vim_command_buf: String::new(),
                last_find: None,
                vim_pending_operator: None,
                vim_pending_text_object_prefix: None,
                vim_command_line: String::new(),
                vim_command_error: None,
                vim_pending_register_select: false,
                vim_selected_register: None,
                vim_pending_replace: false,
                vim_search_direction: true,
                vim_jump_back: Vec::new(),
                vim_jump_forward: Vec::new(),
                pending_scroll_to_cursor: false,
                has_unsupported_blocks: false,
                unsupported_banner_dismissed: false,
            }],
            active_tab: 0,
            next_tab_id: 1,
            sidebar_visible: false,
            sidebar_mode: SidebarMode::default(),
            settings_visible: false,
            working_directory: std::path::PathBuf::from("."),
            file_tree: vec![],
            vim_enabled: true,
            keybinds: crate::keybinds::Keybinds::defaults(),
            theme: crate::theme::ThemeKind::WorkbenchDark,
            theme_color_mode: crate::theme::ThemeColorMode::Minimal,
            vim_macros: HashMap::new(),
            vim_macro_recording: None,
            vim_macro_record_pending: false,
            vim_last_macro_register: None,
            registers: HashMap::new(),
            pending_clipboard_sync: None,
            last_search: None,
            last_change: None,
            vim_change_recording: None,
            vim_insertion_recording: None,
            vim_pending_change_before_insert: None,
            paragraph_integrity: false,
            pilcrows: false,
            fold_all: false,
            invisibility_mode: false,
            split_view: false,
        };
        state
    }

    /// Mirrors `text_editor.rs`'s `process_key`: records the keystroke
    /// into `vim_change_recording` (if active) *before* dispatching it —
    /// needed since plain `handle_vim_key` calls in tests bypass that
    /// capture step entirely (it's normally done one layer up).
    fn vim_key_recorded(state: &mut AppState, key: &str, shift: bool, key_char: Option<&str>) {
        if state.vim_is_recording_change() {
            state.record_change_key(key, shift, key_char);
        }
        state.handle_vim_key(key, shift, key_char);
    }

    // ── Rich text formatting Phase 1: default_paragraphs / Tab construction ────

    #[test]
    fn test_default_paragraphs_is_one_empty_paragraph_one_default_run() {
        let paragraphs = default_paragraphs();
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].heading, 0);
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0], Run::default());
    }

    #[test]
    fn test_new_empty_tab_has_default_paragraphs_and_no_docx_origin() {
        let tab = Tab::new_empty(0);
        assert_eq!(tab.paragraphs, default_paragraphs());
        assert!(tab.docx_origin.is_none());
    }

    /// Sets up a state whose tab has a *specific* multi-run/multi-paragraph
    /// `paragraphs` structure (not the default single-run one `make_state`
    /// builds), for testing that choke-point mutations keep `paragraphs`
    /// in sync with `content` through real editor operations.
    fn make_state_with_paragraphs(paragraphs: Vec<Paragraph>, cursor: usize) -> AppState {
        let content = paragraphs_to_plain_text(&paragraphs);
        let mut state = make_state(&content, cursor, None);
        state.tabs[0].paragraphs = paragraphs;
        state
    }

    // ── Rich text formatting Phase 1: choke-point mutation sync ─────────────

    #[test]
    fn test_insert_char_choke_point_keeps_paragraphs_synced() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "abc".into(), bold: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let mut state = make_state_with_paragraphs(paragraphs, 1);
        state.insert_char('X');
        assert_eq!(state.tabs[0].content, "aXbc");
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "aXbc");
        assert!(state.tabs[0].paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_backspace_choke_point_keeps_paragraphs_synced() {
        let paragraphs = vec![Paragraph { runs: vec![Run { text: "abc".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(), unsupported_xml: None }];
        let mut state = make_state_with_paragraphs(paragraphs, 2);
        state.backspace();
        assert_eq!(state.tabs[0].content, "ac");
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "ac");
    }

    #[test]
    fn test_delete_selection_choke_point_keeps_paragraphs_synced() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "bold".into(), bold: true, ..Run::default() }, Run { text: " plain".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((2, 6)); // deletes "ld p"
        state.delete_selection();
        assert_eq!(state.tabs[0].content, "bolain");
        let runs = &state.tabs[0].paragraphs[0].runs;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "bo");
        assert!(runs[0].bold);
        assert_eq!(runs[1].text, "lain");
        assert!(!runs[1].bold);
    }

    #[test]
    fn test_vim_dd_choke_point_keeps_paragraphs_synced() {
        let paragraphs = vec![
            Paragraph { runs: vec![Run { text: "one".into(), bold: true, ..Run::default() }], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("two")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("d", false, None);
        assert_eq!(state.tabs[0].content, "two");
        assert_eq!(state.tabs[0].paragraphs.len(), 1);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "two");
    }

    #[test]
    fn test_vim_paste_choke_point_keeps_paragraphs_synced() {
        let paragraphs = vec![Paragraph { runs: vec![run_plain("abc")], heading: 0, alignment: Alignment::default(), unsupported_xml: None }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.registers.insert('"', "XY".to_string());
        state.handle_vim_key("p", false, None);
        assert_eq!(state.tabs[0].content, "aXYbc");
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "aXYbc");
    }

    #[test]
    fn test_dispatch_vim_substitute_only_touches_changed_paragraphs() {
        let paragraphs = vec![
            Paragraph { runs: vec![Run { text: "foo bar".into(), bold: true, ..Run::default() }], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("untouched")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.dispatch_vim_command("%s/foo/baz/");
        assert_eq!(state.tabs[0].content, "baz bar\nuntouched");
        // changed paragraph loses formatting (documented scope limit)
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "baz bar");
        assert!(!state.tabs[0].paragraphs[0].runs[0].bold);
        // untouched paragraph is byte-for-byte unchanged
        assert_eq!(state.tabs[0].paragraphs[1].runs[0].text, "untouched");
    }

    #[test]
    fn test_insert_newline_via_enter_splits_paragraph_in_sync() {
        let paragraphs = vec![Paragraph { runs: vec![run_plain("hello")], heading: 0, alignment: Alignment::default(), unsupported_xml: None }];
        let mut state = make_state_with_paragraphs(paragraphs, 2);
        state.insert_char('\n');
        assert_eq!(state.tabs[0].content, "he\nllo");
        assert_eq!(state.tabs[0].paragraphs.len(), 2);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "he");
        assert_eq!(state.tabs[0].paragraphs[1].runs[0].text, "llo");
    }

    fn run_plain(text: &str) -> Run {
        Run { text: text.to_string(), ..Run::default() }
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

    /// Extracts just the content half of each undo-stack snapshot — most
    /// existing undo/redo tests predate the rich-text formatting plan's
    /// paired `(content, paragraphs)` snapshot shape and only care about
    /// the content side.
    fn undo_contents(state: &AppState) -> Vec<String> {
        state.tabs[0].undo_stack.iter().map(|(c, _)| c.clone()).collect()
    }

    fn redo_contents(state: &AppState) -> Vec<String> {
        state.tabs[0].redo_stack.iter().map(|(c, _)| c.clone()).collect()
    }

    #[test]
    fn test_insert_char_pushes_undo_snapshot() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        assert_eq!(undo_contents(&state), vec!["ab".to_string()]);
    }

    #[test]
    fn test_rapid_inserts_coalesce_into_one_undo_step() {
        // Two inserts with no time passing between them (the normal case for
        // fast typing) must land as ONE undo step, not two.
        let mut state = make_state("a", 1, None);
        state.insert_char('b');
        state.insert_char('c');
        assert_eq!(undo_contents(&state), vec!["a".to_string()]);
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_inserts_outside_coalesce_window_are_separate_undo_steps() {
        let mut state = make_state("a", 1, None);
        state.insert_char('b');
        break_coalesce_window(&mut state);
        state.insert_char('c');
        assert_eq!(undo_contents(&state), vec!["a".to_string(), "ab".to_string()]);
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
        state.tabs[0].undo_stack.push(("ab".to_string(), default_paragraphs()));
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
        assert_eq!(redo_contents(&state), vec!["abc".to_string()]);
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
    fn test_undo_restores_paragraphs_not_just_content() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "bold".into(), bold: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let mut state = make_state_with_paragraphs(paragraphs, 4);
        state.insert_char('X'); // "boldX", paragraphs now ["boldX"] still bold
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "boldX");
        state.undo();
        assert_eq!(state.tabs[0].content, "bold");
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "bold");
        assert!(state.tabs[0].paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_redo_restores_paragraphs_not_just_content() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "bold".into(), bold: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let mut state = make_state_with_paragraphs(paragraphs, 4);
        state.insert_char('X');
        state.undo();
        state.redo();
        assert_eq!(state.tabs[0].content, "boldX");
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "boldX");
        assert!(state.tabs[0].paragraphs[0].runs[0].bold);
    }

    // ── Rich text formatting Phase 2: apply_formatting_to_selection ─────────

    #[test]
    fn test_apply_formatting_to_active_selection() {
        let paragraphs = vec![para_plain("hello world")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 5));
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert!(state.tabs[0].paragraphs[0].runs[0].bold);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "hello");
    }

    #[test]
    fn test_apply_formatting_to_selection_is_undoable() {
        let paragraphs = vec![para_plain("hello")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 5));
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        state.undo();
        assert!(!state.tabs[0].paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_apply_formatting_to_selection_toggles_off_when_already_active() {
        // Bug fix: re-clicking Bold on an already-bold selection should
        // un-bold it, matching Word's toolbar toggle behavior, instead of
        // being a no-op re-application.
        let paragraphs = vec![para_plain("hello world")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 5));
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert!(state.tabs[0].paragraphs[0].runs[0].bold);
        state.tabs[0].selection = Some((0, 5));
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert!(!state.tabs[0].paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_apply_formatting_no_selection_arms_pending_format() {
        let mut state = make_state("hello", 0, None);
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert_eq!(state.tabs[0].pending_format, Some(FormatOp::Bold(true)));
    }

    #[test]
    fn test_apply_formatting_no_selection_same_op_again_disarms() {
        let mut state = make_state("hello", 0, None);
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert_eq!(state.tabs[0].pending_format, None);
    }

    #[test]
    fn test_apply_formatting_no_selection_different_op_replaces_pending() {
        let mut state = make_state("hello", 0, None);
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        state.apply_formatting_to_selection(FormatOp::Italic(true));
        assert_eq!(state.tabs[0].pending_format, Some(FormatOp::Italic(true)));
    }

    #[test]
    fn test_pending_format_applies_to_newly_typed_chars() {
        let mut state = make_state_with_paragraphs(vec![para_plain("ab")], 2);
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        state.insert_char('X');
        state.insert_char('Y');
        assert_eq!(state.tabs[0].content, "abXY");
        let runs = &state.tabs[0].paragraphs[0].runs;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "ab");
        assert!(!runs[0].bold);
        assert_eq!(runs[1].text, "XY");
        assert!(runs[1].bold);
    }

    #[test]
    fn test_pending_format_stops_after_toggled_off() {
        // Insert 'Y' at a position that doesn't touch the just-bolded 'X'
        // run — typing immediately adjacent to an existing bold run would
        // inherit its formatting regardless of `pending_format`'s state
        // (the same "typed text takes on the format of whatever it's
        // typed inside" rule every insert follows), which isn't what this
        // test is checking.
        let mut state = make_state_with_paragraphs(vec![para_plain("ab")], 1);
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        state.insert_char('X'); // "aXb", X is bold
        state.apply_formatting_to_selection(FormatOp::Bold(true)); // toggle off
        state.tabs[0].cursor = 0;
        state.insert_char('Y'); // "YaXb", Y at the very start
        assert_eq!(state.tabs[0].content, "YaXb");
        let runs = &state.tabs[0].paragraphs[0].runs;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "Ya");
        assert!(!runs[0].bold);
        assert_eq!(runs[1].text, "X");
        assert!(runs[1].bold);
        assert_eq!(runs[2].text, "b");
        assert!(!runs[2].bold);
    }

    fn para_plain(text: &str) -> Paragraph {
        Paragraph { runs: vec![Run { text: text.to_string(), ..Run::default() }], heading: 0, alignment: Alignment::default(), unsupported_xml: None }
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
        assert_eq!(undo_contents(&state), vec!["abc".to_string()]);
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
        assert_eq!(undo_contents(&state), vec!["hello world".to_string()]);
    }

    #[test]
    fn test_delete_selection_pushes_undo_snapshot() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        state.delete_selection();
        assert_eq!(undo_contents(&state), vec!["hello world".to_string()]);
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
        assert_eq!(undo_contents(&state), vec!["hello".to_string()]);
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
        assert_eq!(undo_contents(&state), vec!["hello world".to_string()]);
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
        assert_eq!(undo_contents(&state), vec!["hello".to_string()]);
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
        assert_eq!(undo_contents(&state), vec!["hello".to_string()]);
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
    fn test_handle_vim_key_normal_colon_via_key_reported_as_symbol_directly() {
        let mut state = make_state("hello", 0, None);
        let handled = state.handle_vim_key(":", false, None);
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
        state.tabs[0].vim_command_line = "wq".to_string();
        let handled = state.handle_vim_key("escape", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].vim_command_line, ""); // discarded, not dispatched
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
        assert_eq!(state.tabs[0].vim_command_line, "x"); // captured into command line instead
    }

    // ── Task H.1: Command-mode text capture ─────────────────────────────────

    #[test]
    fn test_command_mode_captures_typed_letters() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        state.handle_vim_key("w", false, None);
        state.handle_vim_key("q", false, None);
        assert_eq!(state.tabs[0].vim_command_line, "wq");
    }

    #[test]
    fn test_command_mode_captures_punctuation_via_key_char() {
        // GPUI reports shifted punctuation via key_char on this backend;
        // vim_find_target_char is the proven-correct resolver for it.
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        state.handle_vim_key("5", true, Some("%"));
        state.handle_vim_key("s", false, None);
        assert_eq!(state.tabs[0].vim_command_line, "%s");
    }

    #[test]
    fn test_command_mode_backspace_removes_last_char() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        state.tabs[0].vim_command_line = "wq".to_string();
        state.handle_vim_key("backspace", false, None);
        assert_eq!(state.tabs[0].vim_command_line, "w");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Command);
    }

    #[test]
    fn test_command_mode_backspace_on_empty_exits_to_normal() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        state.handle_vim_key("backspace", false, None);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_command_mode_enter_clears_command_line_after_dispatch() {
        let mut state = make_state("hello", 2, None);
        state.tabs[0].vim_mode = VimMode::Command;
        state.tabs[0].vim_command_line = "nonsense".to_string();
        state.handle_vim_key("enter", false, None);
        assert_eq!(state.tabs[0].vim_command_line, "");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    // ── Task H.2: dispatch_vim_command ──────────────────────────────────────

    #[test]
    fn test_dispatch_vim_command_set_novim_disables_vim() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("set novim");
        assert!(!state.vim_enabled);
    }

    #[test]
    fn test_dispatch_vim_command_set_vim_reenables_vim() {
        let mut state = make_state("hello", 0, None);
        state.vim_enabled = false;
        state.dispatch_vim_command("set vim");
        assert!(state.vim_enabled);
    }

    #[test]
    fn test_dispatch_vim_command_line_number_jumps_cursor() {
        let mut state = make_state("aaa\nbbb\nccc", 0, None);
        state.dispatch_vim_command("2");
        assert_eq!(state.tabs[0].cursor, 4); // start of line 2 ("bbb")
    }

    #[test]
    fn test_dispatch_vim_command_noh_is_noop_no_error() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("noh");
        assert_eq!(state.tabs[0].vim_command_error, None);
    }

    #[test]
    fn test_dispatch_vim_command_unknown_command_sets_error() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("bogus");
        assert!(state.tabs[0].vim_command_error.is_some());
    }

    #[test]
    fn test_dispatch_vim_command_w_with_no_file_path_is_noop() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("w");
        assert_eq!(state.tabs[0].vim_command_error, None);
    }

    #[test]
    fn test_dispatch_vim_command_q_on_modified_tab_sets_error_and_does_not_close() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].is_modified = true;
        state.tabs.push(Tab::new_empty(1));
        state.active_tab = 0;
        state.dispatch_vim_command("q");
        assert_eq!(state.tabs.len(), 2);
        assert!(state.tabs[0].vim_command_error.is_some());
    }

    #[test]
    fn test_dispatch_vim_command_q_on_unmodified_tab_closes() {
        let mut state = make_state("hello", 0, None);
        state.tabs.push(Tab::new_empty(1));
        state.active_tab = 0;
        state.dispatch_vim_command("q");
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn test_dispatch_vim_command_q_bang_force_closes_even_if_modified() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].is_modified = true;
        state.tabs.push(Tab::new_empty(1));
        state.active_tab = 0;
        state.dispatch_vim_command("q!");
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn test_dispatch_vim_command_wq_closes_tab_when_no_file_path() {
        let mut state = make_state("hello", 0, None);
        state.tabs.push(Tab::new_empty(1));
        state.active_tab = 0;
        state.dispatch_vim_command("wq");
        assert_eq!(state.tabs.len(), 1);
    }

    // ── Task H.3: :%s/pattern/replacement/[g][i] ────────────────────────────

    #[test]
    fn test_dispatch_vim_command_substitute_first_match_per_line() {
        let mut state = make_state("foo foo\nbar", 0, None);
        state.dispatch_vim_command("%s/foo/baz/");
        assert_eq!(state.tabs[0].content, "baz foo\nbar");
        assert!(state.tabs[0].is_modified);
    }

    #[test]
    fn test_dispatch_vim_command_substitute_global_flag_replaces_all_on_line() {
        let mut state = make_state("foo foo\nbar", 0, None);
        state.dispatch_vim_command("%s/foo/baz/g");
        assert_eq!(state.tabs[0].content, "baz baz\nbar");
    }

    #[test]
    fn test_dispatch_vim_command_substitute_case_insensitive_flag() {
        let mut state = make_state("Foo bar", 0, None);
        state.dispatch_vim_command("%s/foo/baz/i");
        assert_eq!(state.tabs[0].content, "baz bar");
    }

    #[test]
    fn test_dispatch_vim_command_substitute_no_match_leaves_content_unmodified() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("%s/xyz/abc/");
        assert_eq!(state.tabs[0].content, "hello");
        assert!(!state.tabs[0].is_modified);
    }

    #[test]
    fn test_dispatch_vim_command_substitute_bad_regex_sets_error() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("%s/[/x/");
        assert!(state.tabs[0].vim_command_error.is_some());
    }

    #[test]
    fn test_dispatch_vim_command_e_opens_new_tab_with_given_path() {
        let mut state = make_state("hello", 0, None);
        state.dispatch_vim_command("e nonexistent_test_file.docx");
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, 1);
        assert_eq!(
            state.tabs[1].file_path.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
            Some("nonexistent_test_file.docx")
        );
    }

    // ── Task H.4: "<register> prefix + wiring into d/y/c ────────────────────

    #[test]
    fn test_quote_letter_dd_writes_to_named_register_and_default() {
        // Two lines so dd's linewise range naturally includes the trailing
        // '\n' (deleting the last line of a doc with no final newline
        // wouldn't have one to include — not this test's concern).
        let mut state = make_state("hello world\nsecond", 0, None);
        state.handle_vim_key("'", true, Some("\"")); // "
        state.handle_vim_key("a", false, None);      // select register a
        state.handle_vim_key("d", false, None);      // dd
        state.handle_vim_key("d", false, None);
        assert_eq!(state.registers.get(&'a'), Some(&"hello world\n".to_string()));
        assert_eq!(state.registers.get(&'"'), Some(&"hello world\n".to_string()));
    }

    #[test]
    fn test_quote_letter_yank_also_writes_yank_register() {
        let mut state = make_state("hello world\nsecond", 0, None);
        state.handle_vim_key("'", true, Some("\""));
        state.handle_vim_key("b", false, None);
        state.handle_vim_key("y", false, None);
        state.handle_vim_key("y", false, None);
        assert_eq!(state.registers.get(&'b'), Some(&"hello world\n".to_string()));
        assert_eq!(state.registers.get(&'0'), Some(&"hello world\n".to_string()));
    }

    #[test]
    fn test_register_selection_is_one_shot_reverts_to_default_after() {
        let mut state = make_state("one\ntwo\nthree", 0, None);
        state.handle_vim_key("'", true, Some("\""));
        state.handle_vim_key("a", false, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("d", false, None); // "add -> register a
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("d", false, None); // plain dd -> default only
        assert_eq!(state.registers.get(&'a'), Some(&"one\n".to_string()));
        assert_eq!(state.registers.get(&'"'), Some(&"two\n".to_string()));
    }

    #[test]
    fn test_plus_register_prefix_stages_pending_clipboard_sync() {
        let mut state = make_state("hello\nworld", 0, None);
        state.handle_vim_key("'", true, Some("\""));
        state.handle_vim_key("=", true, Some("+"));
        state.handle_vim_key("y", false, None);
        state.handle_vim_key("y", false, None);
        assert_eq!(state.registers.get(&'+'), Some(&"hello\n".to_string()));
        assert_eq!(state.pending_clipboard_sync, Some("hello\n".to_string()));
    }

    // ── Task H.5: p/P paste ──────────────────────────────────────────────────

    #[test]
    fn test_paste_charwise_after_cursor() {
        let mut state = make_state("abc", 0, None);
        state.registers.insert('"', "XY".to_string());
        state.handle_vim_key("p", false, None);
        assert_eq!(state.tabs[0].content, "aXYbc");
        assert_eq!(state.tabs[0].cursor, 2); // lands on last pasted char 'Y'
    }

    #[test]
    fn test_paste_charwise_before_cursor_capital_p() {
        let mut state = make_state("abc", 1, None);
        state.registers.insert('"', "XY".to_string());
        state.handle_vim_key("p", true, None);
        assert_eq!(state.tabs[0].content, "aXYbc");
    }

    #[test]
    fn test_paste_linewise_inserts_as_new_line_below() {
        let mut state = make_state("one\ntwo", 0, None);
        state.registers.insert('"', "middle\n".to_string());
        state.handle_vim_key("p", false, None);
        assert_eq!(state.tabs[0].content, "one\nmiddle\ntwo");
    }

    #[test]
    fn test_paste_linewise_capital_p_inserts_above() {
        let mut state = make_state("one\ntwo", 4, None); // cursor on "two"
        state.registers.insert('"', "middle\n".to_string());
        state.handle_vim_key("p", true, None);
        assert_eq!(state.tabs[0].content, "one\nmiddle\ntwo");
    }

    #[test]
    fn test_paste_empty_register_is_noop() {
        let mut state = make_state("abc", 0, None);
        state.handle_vim_key("p", false, None);
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_paste_named_register_after_quote_prefix() {
        let mut state = make_state("abc", 0, None);
        state.registers.insert('a', "Z".to_string());
        state.handle_vim_key("'", true, Some("\""));
        state.handle_vim_key("a", false, None);
        state.handle_vim_key("p", false, None);
        assert_eq!(state.tabs[0].content, "aZbc");
    }

    // ── Task I.1: x/X/s/S/~/J convenience commands ──────────────────────────

    #[test]
    fn test_x_deletes_char_under_cursor() {
        let mut state = make_state("abc", 1, None);
        state.handle_vim_key("x", false, None);
        assert_eq!(state.tabs[0].content, "ac");
        assert_eq!(state.tabs[0].cursor, 1);
        assert_eq!(state.registers.get(&'"'), Some(&"b".to_string()));
    }

    #[test]
    fn test_x_at_end_of_line_does_not_cross_newline() {
        let mut state = make_state("ab\ncd", 1, None); // cursor on 'b', last char of line
        state.handle_vim_key("x", false, None);
        assert_eq!(state.tabs[0].content, "a\ncd");
    }

    #[test]
    fn test_x_on_empty_line_is_noop() {
        let mut state = make_state("\nabc", 0, None);
        state.handle_vim_key("x", false, None);
        assert_eq!(state.tabs[0].content, "\nabc");
    }

    #[test]
    fn test_capital_x_deletes_char_before_cursor() {
        let mut state = make_state("abc", 2, None);
        state.handle_vim_key("x", true, None);
        assert_eq!(state.tabs[0].content, "ac");
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_capital_x_at_line_start_does_not_cross_newline() {
        let mut state = make_state("ab\ncd", 3, None); // cursor on 'c', first char of line 2
        state.handle_vim_key("x", true, None);
        assert_eq!(state.tabs[0].content, "ab\ncd");
    }

    #[test]
    fn test_s_deletes_char_and_enters_insert() {
        let mut state = make_state("abc", 1, None);
        state.handle_vim_key("s", false, None);
        assert_eq!(state.tabs[0].content, "ac");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_capital_s_deletes_line_and_enters_insert() {
        let mut state = make_state("abc\ndef", 1, None);
        state.handle_vim_key("s", true, None);
        assert_eq!(state.tabs[0].content, "\ndef");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_tilde_toggles_case_and_advances_cursor() {
        let mut state = make_state("aBc", 0, None);
        state.handle_vim_key("`", true, Some("~"));
        assert_eq!(state.tabs[0].content, "ABc");
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_tilde_at_end_of_line_is_noop() {
        let mut state = make_state("\nabc", 0, None);
        state.handle_vim_key("`", true, Some("~"));
        assert_eq!(state.tabs[0].content, "\nabc");
    }

    #[test]
    fn test_join_joins_current_line_with_next() {
        let mut state = make_state("one\ntwo", 0, None);
        state.handle_vim_key("j", true, None); // J (shift+j)
        assert_eq!(state.tabs[0].content, "one two");
    }

    #[test]
    fn test_join_collapses_next_line_leading_whitespace() {
        let mut state = make_state("one\n   two", 0, None);
        state.handle_vim_key("j", true, None);
        assert_eq!(state.tabs[0].content, "one two");
    }

    #[test]
    fn test_join_on_last_line_is_noop() {
        let mut state = make_state("only", 0, None);
        state.handle_vim_key("j", true, None);
        assert_eq!(state.tabs[0].content, "only");
    }

    // ── Task I.2: r<char> replace one character ─────────────────────────────

    #[test]
    fn test_r_replaces_char_under_cursor() {
        let mut state = make_state("abc", 1, None);
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("z", false, None);
        assert_eq!(state.tabs[0].content, "azc");
        assert_eq!(state.tabs[0].cursor, 1); // stays on the replaced char
    }

    #[test]
    fn test_r_with_shifted_replacement_char() {
        let mut state = make_state("abc", 0, None);
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("z", true, None); // shift+z -> 'Z'
        assert_eq!(state.tabs[0].content, "Zbc");
    }

    #[test]
    fn test_r_escape_cancels_without_changing_content() {
        let mut state = make_state("abc", 1, None);
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("escape", false, None);
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_r_does_not_write_register() {
        let mut state = make_state("abc", 1, None);
        state.registers.insert('"', "unchanged".to_string());
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("z", false, None);
        assert_eq!(state.registers.get(&'"'), Some(&"unchanged".to_string()));
    }

    #[test]
    fn test_r_on_empty_line_is_noop() {
        let mut state = make_state("\nabc", 0, None);
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("z", false, None);
        assert_eq!(state.tabs[0].content, "\nabc");
    }

    // ── Task I.3: R Replace mode ─────────────────────────────────────────────

    #[test]
    fn test_capital_r_enters_replace_mode() {
        let mut state = make_state("abc", 0, None);
        state.handle_vim_key("r", true, None);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Replace);
    }

    #[test]
    fn test_replace_mode_typing_overwrites_chars() {
        let mut state = make_state("abcdef", 0, None);
        state.tabs[0].vim_mode = VimMode::Replace;
        state.handle_vim_key("x", false, Some("x"));
        state.handle_vim_key("y", false, Some("y"));
        assert_eq!(state.tabs[0].content, "xycdef");
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_replace_mode_appends_past_end_of_line() {
        let mut state = make_state("ab", 2, None);
        state.tabs[0].vim_mode = VimMode::Replace;
        state.handle_vim_key("z", false, Some("z"));
        assert_eq!(state.tabs[0].content, "abz");
    }

    #[test]
    fn test_replace_mode_escape_returns_to_normal() {
        let mut state = make_state("abc", 0, None);
        state.tabs[0].vim_mode = VimMode::Replace;
        state.handle_vim_key("escape", false, None);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_replace_mode_backspace_moves_cursor_back() {
        let mut state = make_state("abc", 0, None);
        state.tabs[0].vim_mode = VimMode::Replace;
        state.handle_vim_key("x", false, Some("x"));
        assert_eq!(state.tabs[0].cursor, 1);
        state.handle_vim_key("backspace", false, None);
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── Task I.4: Search mode (/, ?, n, N, *, #) ────────────────────────────

    #[test]
    fn test_slash_enters_search_mode_forward() {
        let mut state = make_state("hello world", 0, None);
        state.handle_vim_key("/", false, Some("/"));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Search);
        assert!(state.tabs[0].vim_search_direction);
    }

    #[test]
    fn test_question_mark_enters_search_mode_backward() {
        let mut state = make_state("hello world", 0, None);
        state.handle_vim_key("/", true, Some("?"));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Search);
        assert!(!state.tabs[0].vim_search_direction);
    }

    #[test]
    fn test_search_forward_jumps_to_next_match() {
        let mut state = make_state("foo bar foo baz", 0, None);
        state.handle_vim_key("/", false, Some("/"));
        state.handle_vim_key("f", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("enter", false, None);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].cursor, 8); // second "foo"
    }

    #[test]
    fn test_search_forward_wraps_around() {
        let mut state = make_state("foo bar", 4, None); // cursor on "bar"
        state.handle_vim_key("/", false, Some("/"));
        state.handle_vim_key("f", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("enter", false, None);
        assert_eq!(state.tabs[0].cursor, 0); // wrapped to the only "foo"
    }

    #[test]
    fn test_search_escape_cancels_without_moving_cursor() {
        let mut state = make_state("foo bar foo", 0, None);
        state.handle_vim_key("/", false, Some("/"));
        state.handle_vim_key("b", false, None);
        state.handle_vim_key("escape", false, None);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_n_repeats_last_search_forward() {
        let mut state = make_state("foo bar foo baz foo", 0, None);
        state.handle_vim_key("/", false, Some("/"));
        state.handle_vim_key("f", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("enter", false, None);
        assert_eq!(state.tabs[0].cursor, 8);
        state.handle_vim_key("n", false, None);
        assert_eq!(state.tabs[0].cursor, 16);
    }

    #[test]
    fn test_capital_n_repeats_search_in_reverse() {
        let mut state = make_state("foo bar foo baz foo", 0, None);
        state.handle_vim_key("/", false, Some("/"));
        state.handle_vim_key("f", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("o", false, None);
        state.handle_vim_key("enter", false, None);
        assert_eq!(state.tabs[0].cursor, 8);
        state.handle_vim_key("n", true, None); // N: reverse direction
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_star_searches_forward_for_word_under_cursor() {
        let mut state = make_state("foo bar foo baz", 0, None); // cursor on first "foo"
        state.handle_vim_key("8", true, Some("*"));
        assert_eq!(state.tabs[0].cursor, 8);
    }

    #[test]
    fn test_hash_searches_backward_for_word_under_cursor() {
        let mut state = make_state("foo bar foo baz", 8, None); // cursor on second "foo"
        state.handle_vim_key("3", true, Some("#"));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    // ── Task I.5: Jump list (Ctrl+o/Ctrl+i) ──────────────────────────────────

    #[test]
    fn test_large_motion_pushes_jump_and_ctrl_o_returns() {
        let mut state = make_state("one\ntwo\nthree\nfour\nfive", 0, None);
        state.handle_vim_key("g", true, None); // G: last line
        assert_eq!(state.tabs[0].cursor, 19); // start of "five"
        state.vim_jump_backward();
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_ctrl_i_returns_forward_after_ctrl_o() {
        let mut state = make_state("one\ntwo\nthree\nfour\nfive", 0, None);
        state.handle_vim_key("g", true, None); // G
        state.vim_jump_backward();
        assert_eq!(state.tabs[0].cursor, 0);
        state.vim_jump_forward();
        assert_eq!(state.tabs[0].cursor, 19);
    }

    #[test]
    fn test_single_line_motion_does_not_push_jump() {
        let mut state = make_state("one\ntwo\nthree", 0, None);
        state.handle_vim_key("l", false, None); // small same-line motion
        state.vim_jump_backward(); // nothing was pushed; should be a no-op
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_ctrl_o_with_empty_jump_list_is_noop() {
        let mut state = make_state("abc", 1, None);
        state.vim_jump_backward();
        assert_eq!(state.tabs[0].cursor, 1);
    }

    // ── Task I.6: '.' repeat last change ─────────────────────────────────────

    #[test]
    fn test_dot_repeats_operator_motion_at_new_cursor() {
        let mut state = make_state("foo bar baz", 0, None);
        vim_key_recorded(&mut state, "d", false, None);
        vim_key_recorded(&mut state, "w", false, None);
        assert_eq!(state.tabs[0].content, "bar baz");
        // cursor now at start of "bar" (0). Move to "baz" and repeat.
        state.tabs[0].cursor = 4;
        state.vim_repeat_last_change();
        assert_eq!(state.tabs[0].content, "bar ");
    }

    #[test]
    fn test_dot_repeats_doubled_operator() {
        let mut state = make_state("one\ntwo\nthree", 0, None);
        vim_key_recorded(&mut state, "d", false, None);
        vim_key_recorded(&mut state, "d", false, None);
        assert_eq!(state.tabs[0].content, "two\nthree");
        state.vim_repeat_last_change();
        assert_eq!(state.tabs[0].content, "three");
    }

    #[test]
    fn test_dot_repeats_text_object() {
        let mut state = make_state("(a) (b)", 1, None); // cursor inside first parens
        vim_key_recorded(&mut state, "d", false, None);
        vim_key_recorded(&mut state, "i", false, None);
        vim_key_recorded(&mut state, "(", true, Some("("));
        assert_eq!(state.tabs[0].content, "() (b)");
        state.tabs[0].cursor = 4; // inside second parens
        state.vim_repeat_last_change();
        assert_eq!(state.tabs[0].content, "() ()");
    }

    #[test]
    fn test_yank_does_not_set_last_change() {
        let mut state = make_state("foo bar", 0, None);
        vim_key_recorded(&mut state, "y", false, None);
        vim_key_recorded(&mut state, "w", false, None);
        assert_eq!(state.last_change, None);
    }

    #[test]
    fn test_dot_repeats_plain_insertion() {
        // Insert mode's Escape is handled by the caller (text_editor.rs),
        // not `handle_vim_key` (which returns `false` for it, per its own
        // doc comment) — so tests call `vim_exit_to_normal` directly here,
        // same as text_editor.rs does.
        let mut state = make_state("ab", 0, None);
        vim_key_recorded(&mut state, "i", false, None);
        state.insert_char('X');
        state.insert_char('Y');
        state.vim_exit_to_normal();
        assert_eq!(state.tabs[0].content, "XYab");
        state.tabs[0].cursor = 4; // end of content
        state.vim_repeat_last_change();
        assert_eq!(state.tabs[0].content, "XYabXY");
    }

    #[test]
    fn test_dot_repeats_change_operator_plus_insertion() {
        let mut state = make_state("foo bar", 0, None);
        vim_key_recorded(&mut state, "c", false, None);
        vim_key_recorded(&mut state, "w", false, None);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        state.insert_char('X');
        state.vim_exit_to_normal();
        // `cw` consumes through the motion's exclusive end the same way
        // `dw` does (this codebase doesn't special-case `cw` to stop
        // before trailing whitespace like real vim's `ce`-like quirk) —
        // so the space goes with it.
        assert_eq!(state.tabs[0].content, "Xbar");
        state.tabs[0].cursor = 1; // start of "bar"
        state.vim_repeat_last_change();
        assert_eq!(state.tabs[0].content, "XX");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_dot_with_no_prior_change_is_noop() {
        let mut state = make_state("abc", 0, None);
        state.handle_vim_key(".", false, None);
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_abandoned_operator_does_not_set_last_change() {
        let mut state = make_state("abc", 0, None);
        vim_key_recorded(&mut state, "d", false, None);
        vim_key_recorded(&mut state, "up", false, None); // invalid motion for d: abandons
        assert_eq!(state.last_change, None);
    }

    // ── split_vim_command_buf / take_vim_count / vim_pending_trigger (Task E) ───

    #[test]
    fn test_split_vim_command_buf_empty() {
        assert_eq!(split_vim_command_buf(""), (None, None));
    }

    #[test]
    fn test_split_vim_command_buf_digits_only() {
        assert_eq!(split_vim_command_buf("42"), (Some(42), None));
    }

    #[test]
    fn test_split_vim_command_buf_trigger_only() {
        assert_eq!(split_vim_command_buf("f"), (None, Some('f')));
    }

    #[test]
    fn test_split_vim_command_buf_digits_and_trigger() {
        assert_eq!(split_vim_command_buf("12t"), (Some(12), Some('t')));
    }

    #[test]
    fn test_take_vim_count_none_when_buffer_empty() {
        let mut state = make_state("hello", 0, None);
        assert_eq!(state.take_vim_count(), None);
    }

    #[test]
    fn test_take_vim_count_parses_and_clears_digits() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "7".to_string();
        assert_eq!(state.take_vim_count(), Some(7));
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_take_vim_count_preserves_trailing_trigger() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "3f".to_string();
        assert_eq!(state.take_vim_count(), Some(3));
        assert_eq!(state.tabs[0].vim_command_buf, "f");
    }

    #[test]
    fn test_vim_pending_trigger_none_when_no_trigger() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "5".to_string();
        assert_eq!(state.vim_pending_trigger(), None);
    }

    #[test]
    fn test_vim_pending_trigger_returns_trailing_char() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "g".to_string();
        assert_eq!(state.vim_pending_trigger(), Some('g'));
    }

    #[test]
    fn test_vim_enter_insert_clears_command_buf() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "3".to_string();
        state.vim_enter_insert_before_cursor();
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_vim_enter_visual_clears_command_buf() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "3".to_string();
        state.vim_enter_visual();
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_vim_enter_visual_line_clears_command_buf() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "3".to_string();
        state.vim_enter_visual_line();
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_vim_enter_command_clears_command_buf() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].vim_command_buf = "3".to_string();
        state.vim_enter_command();
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    // ── WORD motions: W/B/E (Task E) ─────────────────────────────────────────────

    #[test]
    fn test_move_word_forward_big_treats_punctuation_as_part_of_word() {
        // "foo.bar" is ONE WORD for `W` (no word/punct split), unlike `w`
        // which would stop at the '.'.
        let mut state = make_state("foo.bar baz", 0, None);
        state.move_word_forward_big();
        assert_eq!(state.tabs[0].cursor, 8); // start of "baz"
    }

    #[test]
    fn test_move_word_backward_big_treats_punctuation_as_part_of_word() {
        let mut state = make_state("foo.bar baz", 8, None); // on "baz"
        state.move_word_backward_big();
        assert_eq!(state.tabs[0].cursor, 0); // start of "foo.bar"
    }

    #[test]
    fn test_move_word_end_big_treats_punctuation_as_part_of_word() {
        let mut state = make_state("foo.bar baz", 0, None);
        state.move_word_end_big();
        assert_eq!(state.tabs[0].cursor, 6); // last char of "foo.bar"
    }

    #[test]
    fn test_move_word_forward_big_crosses_newline() {
        let mut state = make_state("foo\nbar", 0, None);
        state.move_word_forward_big();
        assert_eq!(state.tabs[0].cursor, 4);
    }

    // ── big_word_class / classified free functions ──────────────────────────────

    #[test]
    fn test_big_word_class_punctuation_is_word() {
        assert_eq!(big_word_class('.'), CharClass::Word);
        assert_eq!(big_word_class('_'), CharClass::Word);
        assert_eq!(big_word_class('a'), CharClass::Word);
    }

    #[test]
    fn test_big_word_class_whitespace_is_space() {
        assert_eq!(big_word_class(' '), CharClass::Space);
        assert_eq!(big_word_class('\n'), CharClass::Space);
    }

    // ── paragraph motions: { / } (Task E) ────────────────────────────────────────

    #[test]
    fn test_move_paragraph_forward_lands_on_next_blank_line() {
        let mut state = make_state("one\ntwo\n\nthree", 0, None);
        state.move_paragraph_forward();
        assert_eq!(state.tabs[0].cursor, 8); // start of the blank line
    }

    #[test]
    fn test_move_paragraph_forward_no_next_paragraph_goes_to_end() {
        let mut state = make_state("one\ntwo\nthree", 0, None);
        state.move_paragraph_forward();
        assert_eq!(state.tabs[0].cursor, 13); // content.len()
    }

    #[test]
    fn test_move_paragraph_forward_already_on_blank_line_advances_past_it() {
        let mut state = make_state("one\n\ntwo\n\nthree", 4, None); // on the first blank line
        state.move_paragraph_forward();
        assert_eq!(state.tabs[0].cursor, 9); // the *second* blank line, not staying at 4
    }

    #[test]
    fn test_move_paragraph_backward_lands_on_previous_blank_line() {
        let mut state = make_state("one\n\ntwo\nthree", 9, None); // on "three"
        state.move_paragraph_backward();
        assert_eq!(state.tabs[0].cursor, 4);
    }

    #[test]
    fn test_move_paragraph_backward_no_previous_paragraph_goes_to_start() {
        let mut state = make_state("one\ntwo\nthree", 9, None);
        state.move_paragraph_backward();
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_move_paragraph_backward_already_on_blank_line_retreats_past_it() {
        let mut state = make_state("one\n\ntwo\n\nthree", 9, None); // on the second blank line
        state.move_paragraph_backward();
        assert_eq!(state.tabs[0].cursor, 4); // the *first* blank line, not staying at 9
    }

    // ── f/F/t/T find-char motions + ;/, repeat (Task E) ──────────────────────────

    #[test]
    fn test_move_find_char_forward_lands_on_target() {
        let mut state = make_state("abcdef", 0, None);
        state.move_find_char_forward('d');
        assert_eq!(state.tabs[0].cursor, 3);
        assert_eq!(state.tabs[0].last_find, Some(('f', 'd')));
    }

    #[test]
    fn test_move_find_char_forward_not_found_is_noop_and_does_not_remember() {
        let mut state = make_state("abcdef", 0, None);
        state.move_find_char_forward('z');
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].last_find, None);
    }

    #[test]
    fn test_move_find_char_forward_does_not_cross_line_boundary() {
        let mut state = make_state("abc\ndef", 0, None);
        state.move_find_char_forward('d');
        assert_eq!(state.tabs[0].cursor, 0); // 'd' is on the next line
    }

    #[test]
    fn test_move_find_char_backward_lands_on_target() {
        let mut state = make_state("abcdef", 5, None);
        state.move_find_char_backward('b');
        assert_eq!(state.tabs[0].cursor, 1);
        assert_eq!(state.tabs[0].last_find, Some(('F', 'b')));
    }

    #[test]
    fn test_move_till_char_forward_lands_one_before_target() {
        let mut state = make_state("abcdef", 0, None);
        state.move_till_char_forward('d');
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_move_till_char_forward_target_immediately_next_is_noop() {
        let mut state = make_state("abcdef", 0, None);
        state.move_till_char_forward('b');
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_move_till_char_backward_lands_one_after_target() {
        let mut state = make_state("abcdef", 5, None);
        state.move_till_char_backward('b');
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_repeat_last_find_repeats_forward_find() {
        let mut state = make_state("a.b.c.d", 0, None);
        state.move_find_char_forward('.');
        assert_eq!(state.tabs[0].cursor, 1);
        state.repeat_last_find();
        assert_eq!(state.tabs[0].cursor, 3);
        state.repeat_last_find();
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_repeat_last_find_noop_when_no_prior_find() {
        let mut state = make_state("abcdef", 0, None);
        state.repeat_last_find();
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_repeat_last_find_does_not_update_last_find() {
        let mut state = make_state("a.b.c.d", 0, None);
        state.move_find_char_forward('.');
        state.repeat_last_find();
        assert_eq!(state.tabs[0].last_find, Some(('f', '.'))); // unchanged
    }

    #[test]
    fn test_repeat_last_find_reverse_flips_direction() {
        let mut state = make_state("a.b.c.d", 5, None); // on the second '.'
        state.move_find_char_backward('.');
        assert_eq!(state.tabs[0].cursor, 3);
        // ',' reverses F back into f, continuing forward past the original start.
        state.repeat_last_find_reverse();
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_repeat_last_find_reverse_does_not_update_last_find() {
        let mut state = make_state("a.b.c.d", 5, None);
        state.move_find_char_backward('.');
        state.repeat_last_find_reverse();
        assert_eq!(state.tabs[0].last_find, Some(('F', '.'))); // unchanged
    }

    #[test]
    fn test_repeat_last_find_reverse_after_reverse_still_repeats_original() {
        // ';' after a ',' must repeat the *original* find direction, not
        // the reversed one from the preceding ',' — this is the reason
        // apply_find's `remember` flag exists.
        let mut state = make_state("a.b.c.d", 0, None);
        state.move_find_char_forward('.'); // last_find = ('f', '.'), cursor -> 1
        state.repeat_last_find_reverse();  // reversed to 'F': searches backward from 1, no match, no-op
        assert_eq!(state.tabs[0].cursor, 1); // unchanged: no earlier '.' before position 1
        state.repeat_last_find();          // still 'f' (unchanged by the ',' above): forward to next '.'
        assert_eq!(state.tabs[0].cursor, 3);
    }

    #[test]
    fn test_repeat_last_find_till_nudges_past_adjacent_match() {
        // Without the repeat-nudge, ';' after a 't' would be a no-op
        // (landing back on the same position it already stopped at).
        let mut state = make_state("a.b.c.d", 0, None);
        state.move_till_char_forward('.'); // cursor -> 0 (immediately before the first '.')
        assert_eq!(state.tabs[0].cursor, 0);
        state.repeat_last_find();
        assert_eq!(state.tabs[0].cursor, 2); // one before the *second* '.'
    }

    // ── handle_vim_normal_key dispatch state machine (Task E) ────────────────────

    #[test]
    fn test_handle_vim_key_normal_h_moves_left() {
        let mut state = make_state("hello", 3, None);
        assert!(state.handle_vim_key("h", false, None));
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_handle_vim_key_normal_count_prefix_repeats_motion() {
        let mut state = make_state("hello world", 0, None);
        assert!(state.handle_vim_key("3", false, None)); // accumulate count
        assert_eq!(state.tabs[0].vim_command_buf, "3");
        assert!(state.handle_vim_key("l", false, None)); // 3l
        assert_eq!(state.tabs[0].cursor, 3);
        assert_eq!(state.tabs[0].vim_command_buf, ""); // consumed
    }

    #[test]
    fn test_handle_vim_key_normal_multi_digit_count() {
        let mut state = make_state(&"x".repeat(20), 0, None);
        state.handle_vim_key("1", false, None);
        state.handle_vim_key("0", false, None);
        assert_eq!(state.tabs[0].vim_command_buf, "10");
        state.handle_vim_key("l", false, None);
        assert_eq!(state.tabs[0].cursor, 10);
    }

    #[test]
    fn test_handle_vim_key_normal_leading_zero_is_line_start_motion() {
        let mut state = make_state("hello", 3, None);
        assert!(state.handle_vim_key("0", false, None));
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_handle_vim_key_normal_zero_after_nonzero_extends_count() {
        let mut state = make_state(&"x".repeat(20), 0, None);
        state.handle_vim_key("2", false, None);
        state.handle_vim_key("0", false, None); // "20", not the 0-motion
        assert_eq!(state.tabs[0].vim_command_buf, "20");
        state.handle_vim_key("l", false, None);
        assert_eq!(state.tabs[0].cursor, 20.min("xxxxxxxxxxxxxxxxxxxx".len()));
    }

    #[test]
    fn test_handle_vim_key_normal_w_shift_is_big_word() {
        let mut state = make_state("foo.bar baz", 0, None);
        assert!(state.handle_vim_key("w", true, None)); // W
        assert_eq!(state.tabs[0].cursor, 8);
    }

    #[test]
    fn test_handle_vim_key_normal_gg_no_count_goes_to_first_line_first_nonblank() {
        let mut state = make_state("one\n  two\n  three", 15, None);
        assert!(state.handle_vim_key("g", false, None)); // pending 'g'
        assert_eq!(state.tabs[0].vim_command_buf, "g");
        assert!(state.handle_vim_key("g", false, None)); // gg
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_handle_vim_key_normal_count_gg_goes_to_that_line() {
        let mut state = make_state("one\n  two\n  three", 0, None);
        state.handle_vim_key("2", false, None);
        state.handle_vim_key("g", false, None);
        state.handle_vim_key("g", false, None);
        assert_eq!(state.tabs[0].cursor, 6); // first non-blank of line 2 ("two")
    }

    #[test]
    fn test_handle_vim_key_normal_g_abandoned_by_unrelated_key() {
        let mut state = make_state("hello", 0, None);
        state.handle_vim_key("g", false, None); // pending
        let handled = state.handle_vim_key("x", false, None); // not a second 'g'
        assert!(handled); // still consumed (swallowed), just not gg
        assert_eq!(state.tabs[0].cursor, 0); // no motion happened
        assert_eq!(state.tabs[0].vim_command_buf, ""); // pending state cleared
    }

    #[test]
    fn test_handle_vim_key_normal_shift_g_no_count_goes_to_last_line() {
        let mut state = make_state("one\ntwo\n  three", 0, None);
        assert!(state.handle_vim_key("g", true, None)); // G
        assert_eq!(state.tabs[0].cursor, 10); // first non-blank of "three"
    }

    #[test]
    fn test_handle_vim_key_normal_dollar_via_shift_and_digit4() {
        let mut state = make_state("hello\nworld", 0, None);
        assert!(state.handle_vim_key("4", true, None)); // $
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_handle_vim_key_normal_dollar_via_key_char() {
        let mut state = make_state("hello\nworld", 0, None);
        assert!(state.handle_vim_key("4", false, Some("$")));
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_handle_vim_key_normal_dollar_via_key_reported_as_symbol_directly() {
        // Confirmed empirically on this app's WSLg/X11 backend: `$` did
        // nothing under the original key_char/shift-only check because
        // GPUI reports `key == "$"` directly here, not "4"+shift and not
        // key_char. This is the case that was actually broken.
        let mut state = make_state("hello\nworld", 0, None);
        assert!(state.handle_vim_key("$", false, None));
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_handle_vim_key_normal_plain_4_is_not_dollar() {
        // Guards against matches_shifted_symbol over-triggering: an
        // unshifted "4" (a legitimate count digit) must not be treated as
        // `$` just because key_char happens to echo the same digit.
        let mut state = make_state(&"x".repeat(10), 0, None);
        state.handle_vim_key("4", false, Some("4"));
        assert_eq!(state.tabs[0].vim_command_buf, "4"); // accumulated as a count
        assert_eq!(state.tabs[0].cursor, 0); // not moved to end of line
    }

    #[test]
    fn test_handle_vim_key_normal_caret_via_key_char() {
        let mut state = make_state("  hello", 5, None);
        assert!(state.handle_vim_key("6", false, Some("^")));
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_handle_vim_key_normal_caret_via_key_reported_as_symbol_directly() {
        let mut state = make_state("  hello", 5, None);
        assert!(state.handle_vim_key("^", false, None));
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_handle_vim_key_normal_brace_motions_via_key_reported_as_symbol_directly() {
        let mut state = make_state("one\n\ntwo", 0, None);
        assert!(state.handle_vim_key("}", false, None));
        assert_eq!(state.tabs[0].cursor, 4);
        assert!(state.handle_vim_key("{", false, None));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_handle_vim_key_normal_brace_motions_via_key_char() {
        let mut state = make_state("one\n\ntwo", 0, None);
        assert!(state.handle_vim_key("]", false, Some("}")));
        assert_eq!(state.tabs[0].cursor, 4);
        assert!(state.handle_vim_key("[", false, Some("{")));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_handle_vim_key_normal_f_pending_then_target_finds_char() {
        let mut state = make_state("abcdef", 0, None);
        assert!(state.handle_vim_key("f", false, None)); // pending 'f'
        assert_eq!(state.tabs[0].vim_command_buf, "f");
        assert!(state.handle_vim_key("d", false, None)); // target
        assert_eq!(state.tabs[0].cursor, 3);
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_handle_vim_key_normal_shift_f_pending_is_capital_f() {
        let mut state = make_state("abcdef", 5, None);
        state.handle_vim_key("f", true, None); // pending 'F'
        assert_eq!(state.tabs[0].vim_command_buf, "F");
        state.handle_vim_key("b", false, None);
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_handle_vim_key_normal_count_f_repeats_find() {
        let mut state = make_state("a.b.c.d", 0, None);
        state.handle_vim_key("2", false, None);
        state.handle_vim_key("f", false, None);
        state.handle_vim_key(".", false, None); // 2f. -> second '.'
        assert_eq!(state.tabs[0].cursor, 3);
    }

    #[test]
    fn test_handle_vim_key_normal_f_pending_target_via_key_char_for_symbol() {
        // A shifted-symbol target (e.g. f") relies on key_char since `key`
        // alone can't disambiguate it — same dual-detection pattern as ':'.
        let mut state = make_state("a\"b\"c", 0, None);
        state.handle_vim_key("f", false, None);
        state.handle_vim_key("'", true, Some("\"")); // shift+' = " on a US layout
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_handle_vim_key_normal_f_pending_escape_abandons_find() {
        let mut state = make_state("abcdef", 0, None);
        state.handle_vim_key("f", false, None);
        let handled = state.handle_vim_key("escape", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_command_buf, "");
    }

    #[test]
    fn test_handle_vim_key_normal_semicolon_repeats_find() {
        let mut state = make_state("a.b.c.d", 0, None);
        state.handle_vim_key("f", false, None);
        state.handle_vim_key(".", false, None);
        assert_eq!(state.tabs[0].cursor, 1);
        assert!(state.handle_vim_key(";", false, None));
        assert_eq!(state.tabs[0].cursor, 3);
    }

    #[test]
    fn test_handle_vim_key_normal_comma_reverses_find() {
        let mut state = make_state("a.b.c.d", 5, None);
        state.handle_vim_key("f", true, None); // F
        state.handle_vim_key(".", false, None);
        assert_eq!(state.tabs[0].cursor, 3);
        assert!(state.handle_vim_key(",", false, None));
        assert_eq!(state.tabs[0].cursor, 5);
    }

    #[test]
    fn test_handle_vim_key_normal_semicolon_shift_is_still_colon_not_repeat() {
        // Regression: shift+';' must remain the Command-mode trigger even
        // though plain ';' is now the find-repeat key.
        let mut state = make_state("hello", 0, None);
        assert!(state.handle_vim_key(";", true, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Command);
    }

    #[test]
    fn test_handle_vim_key_normal_pending_find_target_colon_key_is_not_command_mode() {
        // A pending f/F/t/T must treat shift+';' (a ':' keypress) as its
        // target character, not as the Command-mode trigger, even though
        // that exact key/shift/key_char combo *would* enter Command mode
        // via the top-level ':' check when nothing is pending.
        let mut state = make_state("ab:cd", 0, None);
        state.handle_vim_key("f", false, None);
        let handled = state.handle_vim_key(";", true, Some(":"));
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal); // did NOT enter Command
        assert_eq!(state.tabs[0].cursor, 2); // found literal ':'
    }

    #[test]
    fn test_handle_vim_key_normal_navigation_still_falls_through() {
        let mut state = make_state("hello", 2, None);
        assert!(!state.handle_vim_key("left", false, None));
        assert_eq!(state.tabs[0].cursor, 2);
    }

    #[test]
    fn test_handle_vim_key_normal_jk_fall_through() {
        let mut state = make_state("hello", 2, None);
        assert!(!state.handle_vim_key("j", false, None));
        assert!(!state.handle_vim_key("k", false, None));
    }

    #[test]
    fn test_handle_vim_key_normal_mode_switch_still_works_after_rewrite() {
        let mut state = make_state("hello", 0, None);
        assert!(state.handle_vim_key("i", false, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_handle_vim_key_normal_stale_count_does_not_leak_into_mode_switch() {
        let mut state = make_state("hello", 0, None);
        state.handle_vim_key("3", false, None);
        state.handle_vim_key("v", false, None); // enters Visual, should clear buf
        assert_eq!(state.tabs[0].vim_command_buf, "");
        state.handle_vim_key("escape", false, None); // back to Normal
        let cursor_before = state.tabs[0].cursor;
        state.handle_vim_key("l", false, None); // should move by 1, not 3
        assert_eq!(state.tabs[0].cursor, cursor_before + 1);
    }

    // ── Visual-mode motion extension (Task E pass 2) ─────────────────────────────

    #[test]
    fn test_handle_vim_key_visual_h_extends_selection() {
        let mut state = make_state("hello", 3, None);
        state.vim_enter_visual(); // selects (3, 4), cursor -> 4 (the selection's far edge)
        assert!(state.handle_vim_key("h", false, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual);
        // 'h' from cursor 4 lands back on 3 — the anchor — shrinking the
        // selection to zero-width rather than reversing past it.
        assert_eq!(state.tabs[0].selection, Some((3, 3)));
        assert_eq!(state.tabs[0].cursor, 3);
    }

    #[test]
    fn test_handle_vim_key_visual_l_extends_selection_forward() {
        let mut state = make_state("hello world", 0, None);
        state.vim_enter_visual(); // selects (0, 1)
        assert!(state.handle_vim_key("l", false, None));
        assert_eq!(state.tabs[0].selection, Some((0, 2)));
    }

    #[test]
    fn test_handle_vim_key_visual_count_w_extends_by_multiple_words() {
        let mut state = make_state("one two three four", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("2", false, None);
        assert!(state.handle_vim_key("w", false, None));
        assert_eq!(state.tabs[0].cursor, 8); // start of "three"
        assert_eq!(state.tabs[0].selection, Some((0, 8)));
    }

    #[test]
    fn test_handle_vim_key_visual_dollar_extends_to_line_end() {
        let mut state = make_state("hello\nworld", 0, None);
        state.vim_enter_visual();
        assert!(state.handle_vim_key("$", false, None));
        assert_eq!(state.tabs[0].selection, Some((0, 5)));
    }

    #[test]
    fn test_handle_vim_key_visual_gg_extends_to_first_line() {
        let mut state = make_state("one\ntwo\nthree", 9, None); // on "three"
        state.vim_enter_visual();
        state.handle_vim_key("g", false, None);
        assert!(state.handle_vim_key("g", false, None));
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].selection.unwrap().1, 0);
    }

    #[test]
    fn test_handle_vim_key_visual_f_extends_to_found_char() {
        let mut state = make_state("abcdef", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("f", false, None);
        assert!(state.handle_vim_key("d", false, None));
        assert_eq!(state.tabs[0].cursor, 3);
        assert_eq!(state.tabs[0].selection, Some((0, 3)));
    }

    #[test]
    fn test_handle_vim_key_visual_semicolon_repeats_find_and_extends() {
        let mut state = make_state("a.b.c.d", 0, None);
        state.vim_enter_visual(); // cursor -> 1 (char_right(0), the selection's far edge)
        state.handle_vim_key("f", false, None);
        state.handle_vim_key(".", false, None); // finds the '.' at 3, searching from cursor 1
        assert_eq!(state.tabs[0].cursor, 3);
        assert!(state.handle_vim_key(";", false, None));
        assert_eq!(state.tabs[0].cursor, 5);
        assert_eq!(state.tabs[0].selection.unwrap().1, 5);
    }

    #[test]
    fn test_handle_vim_key_visual_left_right_extend_instead_of_falling_through() {
        // Unlike Normal mode, Visual's left/right must NOT fall through
        // (that would clear the selection via the plain editor's Left/
        // Right handling) — they're resolved directly as h/l equivalents.
        let mut state = make_state("hello", 2, None);
        state.vim_enter_visual();
        assert!(state.handle_vim_key("right", false, None));
        assert_eq!(state.tabs[0].selection, Some((2, 4)));
        assert!(state.handle_vim_key("left", false, None));
        assert_eq!(state.tabs[0].selection, Some((2, 3)));
    }

    #[test]
    fn test_handle_vim_key_visual_home_end_extend() {
        let mut state = make_state("hello world", 5, None);
        state.vim_enter_visual();
        assert!(state.handle_vim_key("end", false, None));
        assert_eq!(state.tabs[0].cursor, 11);
        assert!(state.handle_vim_key("home", false, None));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_handle_vim_key_visual_up_down_jk_fall_through_for_visual_row_movement() {
        let mut state = make_state("hello\nworld", 0, None);
        state.vim_enter_visual();
        assert!(!state.handle_vim_key("j", false, None));
        assert!(!state.handle_vim_key("k", false, None));
        assert!(!state.handle_vim_key("up", false, None));
        assert!(!state.handle_vim_key("down", false, None));
        // None of these should have been silently swallowed as a no-op motion.
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual);
    }

    #[test]
    fn test_handle_vim_key_visual_line_h_extends_within_visual_line() {
        let mut state = make_state("one\ntwo\nthree", 4, None); // on "two"
        state.vim_enter_visual_line(); // selects "two\n" as (4, 8)
        assert!(state.handle_vim_key("j", false, None) == false); // falls through, unaffected here
        // Directly verify a pure motion extends VisualLine's selection too.
        assert!(state.handle_vim_key("l", false, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::VisualLine);
    }

    #[test]
    fn test_handle_vim_key_visual_i_is_swallowed_not_insert_entry() {
        // In Visual mode 'i'/'a' are text-object prefixes (spec 5.4, not
        // yet implemented) — must NOT enter Insert mode the way Normal's
        // 'i' does.
        let mut state = make_state("hello", 2, None);
        state.vim_enter_visual();
        let handled = state.handle_vim_key("i", false, None);
        assert!(handled);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual); // not Insert
        assert_eq!(state.tabs[0].content, "hello"); // not inserted as text
    }

    #[test]
    fn test_handle_vim_key_visual_escape_still_exits_after_refactor() {
        let mut state = make_state("hello", 2, None);
        state.vim_enter_visual();
        assert!(state.handle_vim_key("escape", false, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].selection, None);
    }

    #[test]
    fn test_handle_vim_key_visual_v_still_toggles_off_after_refactor() {
        let mut state = make_state("hello", 2, None);
        state.vim_enter_visual();
        assert!(state.handle_vim_key("v", false, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_handle_vim_key_visual_line_shift_v_still_toggles_off_after_refactor() {
        let mut state = make_state("hello", 2, None);
        state.vim_enter_visual_line();
        assert!(state.handle_vim_key("v", true, None));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    // ── _ motion (Task E pass 2) ──────────────────────────────────────────────────

    #[test]
    fn test_underscore_motion_no_count_is_current_line_first_nonblank() {
        assert_eq!(underscore_motion("  hello\nworld", 5, 1), 2);
    }

    #[test]
    fn test_underscore_motion_count_moves_down_lines() {
        assert_eq!(underscore_motion("one\n  two\nthree", 0, 2), 6);
    }

    #[test]
    fn test_underscore_motion_clamps_past_last_line() {
        assert_eq!(underscore_motion("one\ntwo", 0, 50), 4);
    }

    #[test]
    fn test_handle_vim_key_normal_underscore_moves_to_first_nonblank() {
        let mut state = make_state("hello\n  world", 0, None);
        state.handle_vim_key("2", false, None);
        assert!(state.handle_vim_key("_", false, None));
        assert_eq!(state.tabs[0].cursor, 8); // "world" preceded by 2 spaces on line 2
    }

    #[test]
    fn test_handle_vim_key_visual_underscore_extends_selection() {
        let mut state = make_state("one\n  two", 0, None);
        state.vim_enter_visual(); // cursor -> 1, selection (0, 1)
        state.handle_vim_key("2", false, None); // count=2: down one line
        assert!(state.handle_vim_key("_", false, None));
        assert_eq!(state.tabs[0].cursor, 6);
        assert_eq!(state.tabs[0].selection, Some((0, 6)));
    }

    // ── vim_move_to_line_first_nonblank / H/M/L groundwork (Task E pass 2) ───────

    #[test]
    fn test_vim_move_to_line_first_nonblank_moves_cursor() {
        let mut state = make_state("one\n  two\nthree", 0, None);
        state.vim_move_to_line_first_nonblank(1, false);
        assert_eq!(state.tabs[0].cursor, 6);
        assert_eq!(state.tabs[0].selection, None);
    }

    #[test]
    fn test_vim_move_to_line_first_nonblank_extends_selection() {
        let mut state = make_state("one\n  two\nthree", 0, None);
        state.vim_enter_visual(); // cursor -> 1, selection (0,1)
        state.vim_move_to_line_first_nonblank(1, true);
        assert_eq!(state.tabs[0].cursor, 6);
        assert_eq!(state.tabs[0].selection, Some((0, 6)));
    }

    #[test]
    fn test_vim_move_to_line_first_nonblank_clamps_past_last_line() {
        let mut state = make_state("one\ntwo", 0, None);
        state.vim_move_to_line_first_nonblank(50, false);
        assert_eq!(state.tabs[0].cursor, 4); // start of "two", the last line
    }

    // ── macro recording: q<register> / bare q (Task E pass 2) ────────────────────

    #[test]
    fn test_q_then_register_starts_recording() {
        let mut state = make_state("hello", 0, None);
        assert!(state.handle_vim_key("q", false, None)); // pending: waiting for register
        assert!(!state.vim_is_recording_macro());
        assert!(state.handle_vim_key("a", false, None)); // register 'a'
        assert!(state.vim_is_recording_macro());
    }

    #[test]
    fn test_vim_macro_record_pending_accessor() {
        // Backs the mode-indicator's pending-command echo, which needs to
        // show `q` is waiting for its register name — this state doesn't
        // live in `vim_command_buf`, so the indicator can't see it without
        // this accessor.
        let mut state = make_state("hello", 0, None);
        assert!(!state.vim_macro_record_pending());
        state.handle_vim_key("q", false, None);
        assert!(state.vim_macro_record_pending());
        state.handle_vim_key("a", false, None);
        assert!(!state.vim_macro_record_pending());
    }

    #[test]
    fn test_vim_recording_register_accessor() {
        // Backs the mode-indicator showing which register is actively
        // recording (real vim's "recording @a") for the whole duration of
        // a recording, not just the initial `q<register>` keystroke.
        let mut state = make_state("hello", 0, None);
        assert_eq!(state.vim_recording_register(), None);
        state.handle_vim_key("q", false, None);
        state.handle_vim_key("a", false, None);
        assert_eq!(state.vim_recording_register(), Some('a'));
        state.handle_vim_key("q", false, None); // stop
        assert_eq!(state.vim_recording_register(), None);
    }

    #[test]
    fn test_bare_q_while_recording_stops_and_saves() {
        let mut state = make_state("hello", 0, None);
        state.handle_vim_key("q", false, None);
        state.handle_vim_key("a", false, None); // recording into 'a'
        state.record_macro_key("l", false, None);
        state.record_macro_key("l", false, None);
        assert!(state.handle_vim_key("q", false, None)); // bare q: stop
        assert!(!state.vim_is_recording_macro());
        assert_eq!(
            state.macro_keys('a'),
            Some(vec![
                RecordedVimKey { key: "l".into(), shift: false, key_char: None },
                RecordedVimKey { key: "l".into(), shift: false, key_char: None },
            ])
        );
    }

    #[test]
    fn test_record_macro_key_noop_when_not_recording() {
        let mut state = make_state("hello", 0, None);
        state.record_macro_key("l", false, None);
        assert_eq!(state.macro_keys('a'), None);
    }

    #[test]
    fn test_macro_pending_register_does_not_leak_into_next_command() {
        // 'q' followed by 'a' resolves the register; the *next* keystroke
        // must be handled normally (not swallowed as a second register).
        let mut state = make_state("hello", 0, None);
        state.handle_vim_key("q", false, None);
        state.handle_vim_key("a", false, None);
        assert!(state.handle_vim_key("l", false, None));
        assert_eq!(state.tabs[0].cursor, 1);
    }

    #[test]
    fn test_fq_resolves_as_find_target_not_macro_start() {
        // A pending f/F/t/T trigger takes priority over macro-start: `fq`
        // must find the literal character 'q', not begin `q<register>`.
        let mut state = make_state("qab", 0, None);
        state.handle_vim_key("f", false, None); // pending find trigger
        assert!(state.handle_vim_key("q", false, None));
        assert_eq!(state.tabs[0].cursor, 0); // found 'q' at position 0
        assert!(!state.vim_is_recording_macro());
    }

    #[test]
    fn test_macro_keys_returns_none_for_unset_register() {
        let state = make_state("hello", 0, None);
        assert_eq!(state.macro_keys('z'), None);
    }

    // ── resolve_vim_motion / MotionKind (Task F groundwork) ──────────────────────

    #[test]
    fn test_resolve_vim_motion_w_is_exclusive_e_is_inclusive_same_target() {
        // The whole point of MotionKind: `w` and `e` land on the same
        // offset for "one two" from 0 (the 'o' at the end of "one" is
        // position... actually `w` lands at start of "two" (4), `e` lands
        // on the last char of "one" (2) — different targets AND different
        // kinds. Use content where they'd coincide in target to prove the
        // *kind* is what distinguishes them, not just the position.
        let mut state = make_state("one two", 0, None);
        let w = state.resolve_vim_motion("w", false, None);
        assert_eq!(w, MotionResolution::Resolved { target: 4, kind: MotionKind::ExclusiveChar });
        let mut state2 = make_state("one two", 0, None);
        let e = state2.resolve_vim_motion("e", false, None);
        assert_eq!(e, MotionResolution::Resolved { target: 2, kind: MotionKind::InclusiveChar });
    }

    #[test]
    fn test_resolve_vim_motion_dollar_is_inclusive_caret_is_exclusive() {
        let mut state = make_state("  hi", 2, None);
        let dollar = state.resolve_vim_motion("4", true, Some("$"));
        assert_eq!(dollar, MotionResolution::Resolved { target: 4, kind: MotionKind::InclusiveChar });
        let mut state2 = make_state("  hi", 2, None);
        let caret = state2.resolve_vim_motion("6", true, Some("^"));
        assert_eq!(caret, MotionResolution::Resolved { target: 2, kind: MotionKind::ExclusiveChar });
    }

    #[test]
    fn test_resolve_vim_motion_gg_and_g_shift_are_linewise() {
        let mut state = make_state("one\ntwo\nthree", 10, None);
        state.handle_vim_key("g", false, None); // pending
        let gg = state.resolve_vim_motion("g", false, None);
        assert_eq!(gg, MotionResolution::Resolved { target: 0, kind: MotionKind::Linewise });

        let mut state2 = make_state("one\ntwo\nthree", 0, None);
        let g_shift = state2.resolve_vim_motion("g", true, None);
        assert_eq!(g_shift, MotionResolution::Resolved { target: 8, kind: MotionKind::Linewise });
    }

    #[test]
    fn test_resolve_vim_motion_underscore_is_linewise() {
        let mut state = make_state("one\ntwo", 0, None);
        let r = state.resolve_vim_motion("_", false, None);
        assert_eq!(r, MotionResolution::Resolved { target: 0, kind: MotionKind::Linewise });
    }

    #[test]
    fn test_resolve_vim_motion_find_f_is_inclusive_t_is_exclusive() {
        let mut state = make_state("abcXdef", 0, None);
        state.handle_vim_key("f", false, None); // pending
        let f = state.resolve_vim_motion("X", true, Some("X"));
        assert_eq!(f, MotionResolution::Resolved { target: 3, kind: MotionKind::InclusiveChar });

        let mut state2 = make_state("abcXdef", 0, None);
        state2.handle_vim_key("t", false, None); // pending
        let t = state2.resolve_vim_motion("X", true, Some("X"));
        assert_eq!(t, MotionResolution::Resolved { target: 2, kind: MotionKind::ExclusiveChar });
    }

    #[test]
    fn test_resolve_vim_motion_left_right_home_end_always_resolve_locally() {
        // Unlike the old combined `handle_vim_motion_key`, `resolve_vim_motion`
        // itself never defers left/right/home/end to GPUI — that fallthrough
        // is `handle_vim_motion_key`'s own concern now, so operators (which
        // call `resolve_vim_motion` directly) can act on arrow keys too.
        let mut state = make_state("hello", 2, None);
        assert_eq!(
            state.resolve_vim_motion("left", false, None),
            MotionResolution::Resolved { target: 1, kind: MotionKind::ExclusiveChar }
        );
        let mut state2 = make_state("hello", 2, None);
        assert_eq!(
            state2.resolve_vim_motion("end", false, None),
            MotionResolution::Resolved { target: 5, kind: MotionKind::InclusiveChar }
        );
    }

    #[test]
    fn test_resolve_vim_motion_up_down_j_k_need_gpui() {
        let mut state = make_state("one\ntwo", 0, None);
        assert_eq!(state.resolve_vim_motion("j", false, None), MotionResolution::NeedsGpui);
        assert_eq!(state.resolve_vim_motion("k", false, None), MotionResolution::NeedsGpui);
    }

    #[test]
    fn test_resolve_vim_motion_digit_and_pending_trigger_start_are_pending() {
        let mut state = make_state("hello", 0, None);
        assert_eq!(state.resolve_vim_motion("3", false, None), MotionResolution::Pending);
        let mut state2 = make_state("hello", 0, None);
        assert_eq!(state2.resolve_vim_motion("g", false, None), MotionResolution::Pending);
    }

    #[test]
    fn test_resolve_vim_motion_unmapped_key_is_not_a_motion() {
        let mut state = make_state("hello", 0, None);
        assert_eq!(state.resolve_vim_motion("i", false, None), MotionResolution::NotAMotion);
    }

    #[test]
    fn test_handle_vim_motion_key_normal_still_defers_left_right_to_gpui() {
        // Regression: handle_vim_motion_key's own extend=false special case
        // must still return Some(false) for these, exactly as before the
        // resolve_vim_motion split.
        let mut state = make_state("hello", 2, None);
        assert_eq!(state.handle_vim_motion_key("left", false, None, false), Some(false));
        assert_eq!(state.handle_vim_motion_key("home", false, None, false), Some(false));
    }

    #[test]
    fn test_handle_vim_motion_key_visual_resolves_left_right_locally() {
        let mut state = make_state("hello", 2, None);
        assert_eq!(state.handle_vim_motion_key("left", false, None, true), Some(true));
        assert_eq!(state.tabs[0].cursor, 1);
    }

    // ── Operators: d/y/c + dd/yy/cc (Task F) ──────────────────────────────────────

    #[test]
    fn test_dw_deletes_exclusive_up_to_next_word() {
        let mut state = make_state("one two three", 0, None);
        state.handle_vim_key("d", false, None);
        assert_eq!(state.tabs[0].vim_pending_operator, Some('d'));
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "two three");
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_pending_operator, None);
        assert_eq!(state.registers.get(&'"'), Some(&"one ".to_string()));
    }

    #[test]
    fn test_d3w_count_typed_after_operator_deletes_three_words() {
        let mut state = make_state("one two three four", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("3", false, None);
        assert_eq!(state.tabs[0].vim_pending_operator, Some('d')); // still pending
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "four");
    }

    #[test]
    fn test_de_deletes_inclusive_through_word_end() {
        let mut state = make_state("one two", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("e", false, None);
        assert_eq!(state.tabs[0].content, " two");
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_dw_and_de_from_same_cursor_produce_different_ranges() {
        // The whole point of MotionKind: same starting cursor, same
        // starting content, different operator result.
        let mut dw = make_state("one two", 0, None);
        dw.handle_vim_key("d", false, None);
        dw.handle_vim_key("w", false, None);
        let mut de = make_state("one two", 0, None);
        de.handle_vim_key("d", false, None);
        de.handle_vim_key("e", false, None);
        assert_ne!(dw.tabs[0].content, de.tabs[0].content);
    }

    #[test]
    fn test_dd_deletes_current_line() {
        let mut state = make_state("one\ntwo\nthree", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("d", false, None);
        assert_eq!(state.tabs[0].content, "two\nthree");
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.registers.get(&'"'), Some(&"one\n".to_string()));
    }

    #[test]
    fn test_d2d_deletes_two_lines_via_count_between_doubled_keys() {
        let mut state = make_state("a\nb\nc\nd", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("2", false, None);
        assert_eq!(state.tabs[0].vim_pending_operator, Some('d')); // still pending
        state.handle_vim_key("d", false, None);
        assert_eq!(state.tabs[0].content, "c\nd");
    }

    #[test]
    fn test_d_dollar_deletes_inclusive_to_end_of_line() {
        let mut state = make_state("hello world", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("4", true, None); // shifted 4 => $
        assert_eq!(state.tabs[0].content, "");
    }

    #[test]
    fn test_yy_yanks_current_line_without_deleting() {
        let mut state = make_state("one\ntwo", 0, None);
        state.handle_vim_key("y", false, None);
        state.handle_vim_key("y", false, None);
        assert_eq!(state.tabs[0].content, "one\ntwo"); // unchanged
        assert_eq!(state.registers.get(&'"'), Some(&"one\n".to_string()));
        assert_eq!(state.registers.get(&'0'), Some(&"one\n".to_string()));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_yw_yanks_word_and_moves_cursor_to_start_not_target() {
        let mut state = make_state("one two", 0, None);
        state.handle_vim_key("y", false, None);
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "one two");
        assert_eq!(state.registers.get(&'"'), Some(&"one ".to_string()));
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_cc_changes_line_keeping_it_as_empty_line_and_enters_insert() {
        let mut state = make_state("one\ntwo", 0, None);
        state.handle_vim_key("c", false, None);
        state.handle_vim_key("c", false, None);
        assert_eq!(state.tabs[0].content, "\ntwo"); // line kept, just emptied
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        assert_eq!(state.registers.get(&'"'), Some(&"one".to_string()));
    }

    #[test]
    fn test_d_find_deletes_inclusive_through_target_char() {
        let mut state = make_state("abcXdef", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("f", false, None);
        state.handle_vim_key("X", true, Some("X"));
        assert_eq!(state.tabs[0].content, "def");
    }

    #[test]
    fn test_d_till_deletes_exclusive_up_to_target_char() {
        // `t` lands just *before* 'X' (position 2, the 'c') — exclusive
        // range [0, 2) deletes "ab", leaving "cXdef" behind. Distinct from
        // `df` (test above), which deletes through 'X' itself.
        let mut state = make_state("abcXdef", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("t", false, None);
        state.handle_vim_key("X", true, Some("X"));
        assert_eq!(state.tabs[0].content, "cXdef");
    }

    #[test]
    fn test_operator_delete_is_undoable() {
        let mut state = make_state("one two three", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "two three");
        state.undo();
        assert_eq!(state.tabs[0].content, "one two three");
    }

    #[test]
    fn test_operator_abandoned_by_invalid_key_does_not_leak_into_macro() {
        // 'd' then 'q': must abandon the pending operator, NOT start
        // recording into register 'q' — regression guard for the ordering
        // decided between complete_vim_operator and the macro q-pending
        // check in handle_vim_normal_key.
        let mut state = make_state("one two", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("q", false, None);
        assert_eq!(state.tabs[0].vim_pending_operator, None);
        assert!(!state.vim_is_recording_macro());
        assert_eq!(state.tabs[0].content, "one two"); // unchanged
    }

    #[test]
    fn test_operator_abandoned_by_needs_gpui_key() {
        // dj: j needs GPUI context resolve_vim_motion doesn't have —
        // documented gap, must abandon cleanly rather than panic/misfire.
        let mut state = make_state("one\ntwo", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("j", false, None);
        assert_eq!(state.tabs[0].vim_pending_operator, None);
        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    #[test]
    fn test_operator_pending_cleared_on_mode_transitions() {
        let mut state = make_state("hello", 0, None);
        state.handle_vim_key("d", false, None);
        state.vim_exit_to_normal();
        assert_eq!(state.tabs[0].vim_pending_operator, None);

        let mut state2 = make_state("hello", 0, None);
        state2.handle_vim_key("d", false, None);
        state2.vim_enter_visual();
        assert_eq!(state2.tabs[0].vim_pending_operator, None);
    }

    #[test]
    fn test_d_backward_motion_normalizes_range_regardless_of_direction() {
        let mut state = make_state("one two three", 8, None); // cursor on "three"
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("b", false, None); // b moves backward to "two"
        assert_eq!(state.tabs[0].content, "one three");
    }

    #[test]
    fn test_vim_pending_operator_accessor() {
        // text_editor.rs's j/k, H/M/L, and `@` interceptions all gate on
        // this being None — regression guard for the "dj silently moves
        // the cursor and leaves d dangling" bug caught by the advisor
        // (same failure class as the pending-find-trigger check this
        // mirrors, `vim_pending_trigger()`).
        let mut state = make_state("one two", 0, None);
        assert_eq!(state.vim_pending_operator(), None);
        state.handle_vim_key("d", false, None);
        assert_eq!(state.vim_pending_operator(), Some('d'));
        state.handle_vim_key("w", false, None);
        assert_eq!(state.vim_pending_operator(), None);
    }

    // ── Text objects (Task F): iw/aw, is/as, ip/ap, quotes, brackets ─────────────

    #[test]
    fn test_text_object_word_inner_and_around_with_trailing_space() {
        let content = "one two three";
        let cursor = content.find("two").unwrap();
        let (s, e) = text_object_word(content, cursor, true);
        assert_eq!(&content[s..e], "two");
        let (s, e) = text_object_word(content, cursor, false);
        assert_eq!(&content[s..e], "two ");
    }

    #[test]
    fn test_text_object_aw_falls_back_to_leading_space_when_no_trailing() {
        let content = "one two";
        let cursor = content.find("two").unwrap();
        let (s, e) = text_object_word(content, cursor, false);
        assert_eq!(&content[s..e], " two");
    }

    #[test]
    fn test_text_object_iw_on_whitespace_selects_just_the_whitespace_run() {
        let content = "one  two";
        let cursor = content.find("  ").unwrap();
        let (s, e) = text_object_word(content, cursor, true);
        assert_eq!(&content[s..e], "  ");
    }

    #[test]
    fn test_text_object_iw_on_punctuation_run() {
        let content = "one,,two";
        let cursor = content.find(",,").unwrap();
        let (s, e) = text_object_word(content, cursor, true);
        assert_eq!(&content[s..e], ",,");
    }

    #[test]
    fn test_text_object_sentence_inner_and_around() {
        let content = "Hello world. Foo bar. Baz.";
        let cursor = content.find("bar").unwrap();
        let (s, e) = text_object_sentence(content, cursor, true).unwrap();
        assert_eq!(&content[s..e], "Foo bar.");
        let (s, e) = text_object_sentence(content, cursor, false).unwrap();
        assert_eq!(&content[s..e], "Foo bar. ");
    }

    #[test]
    fn test_text_object_sentence_first_sentence_has_no_leading_boundary() {
        let content = "Hello world. Foo bar.";
        let cursor = content.find("Hello").unwrap();
        let (s, e) = text_object_sentence(content, cursor, true).unwrap();
        assert_eq!(&content[s..e], "Hello world.");
    }

    #[test]
    fn test_text_object_paragraph_inner_and_around() {
        let content = "one\ntwo\n\nthree\nfour";
        let cursor = content.find("two").unwrap();
        let (s, e) = text_object_paragraph(content, cursor, true).unwrap();
        assert_eq!(&content[s..e], "one\ntwo\n");
        let (s, e) = text_object_paragraph(content, cursor, false).unwrap();
        assert_eq!(&content[s..e], "one\ntwo\n\n");
    }

    #[test]
    fn test_text_object_paragraph_ap_falls_back_to_leading_blank_block() {
        let content = "one\ntwo\n\nthree\nfour";
        let cursor = content.find("four").unwrap();
        let (s, e) = text_object_paragraph(content, cursor, false).unwrap();
        assert_eq!(&content[s..e], "\nthree\nfour");
    }

    #[test]
    fn test_text_object_quote_inner_and_around() {
        let content = "say \"hello world\" now";
        let cursor = content.find("hello").unwrap();
        let (s, e) = text_object_quote(content, cursor, '"', true).unwrap();
        assert_eq!(&content[s..e], "hello world");
        let (s, e) = text_object_quote(content, cursor, '"', false).unwrap();
        assert_eq!(&content[s..e], "\"hello world\"");
    }

    #[test]
    fn test_text_object_quote_none_when_no_pair_on_line() {
        let content = "no quotes here";
        assert_eq!(text_object_quote(content, 0, '"', true), None);
    }

    #[test]
    fn test_text_object_bracket_innermost_pair() {
        let content = "foo(bar(baz)qux)end";
        let cursor = content.find("baz").unwrap();
        let (s, e) = text_object_bracket(content, cursor, '(', ')', true).unwrap();
        assert_eq!(&content[s..e], "baz");
        let (s, e) = text_object_bracket(content, cursor, '(', ')', false).unwrap();
        assert_eq!(&content[s..e], "(baz)");
    }

    #[test]
    fn test_text_object_bracket_outer_pair_when_cursor_outside_inner() {
        let content = "foo(bar(baz)qux)end";
        let cursor = content.find("qux").unwrap();
        let (s, e) = text_object_bracket(content, cursor, '(', ')', true).unwrap();
        assert_eq!(&content[s..e], "bar(baz)qux");
    }

    #[test]
    fn test_diw_deletes_word_via_operator_and_text_object() {
        let mut state = make_state("one two three", 0, None);
        state.tabs[0].cursor = "one two three".find("two").unwrap();
        state.handle_vim_key("d", false, None);
        assert_eq!(state.tabs[0].vim_pending_operator, Some('d'));
        state.handle_vim_key("i", false, None);
        assert_eq!(state.tabs[0].vim_pending_text_object_prefix, Some(true));
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "one  three");
        assert_eq!(state.tabs[0].vim_pending_operator, None);
        assert_eq!(state.tabs[0].vim_pending_text_object_prefix, None);
    }

    #[test]
    fn test_daw_deletes_word_and_surrounding_space() {
        let mut state = make_state("one two three", 0, None);
        state.tabs[0].cursor = "one two three".find("two").unwrap();
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("a", false, None);
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "one three");
    }

    #[test]
    fn test_ci_quote_changes_inside_quotes_and_enters_insert() {
        let content = "say \"hello world\" now";
        let mut state = make_state(content, 0, None);
        state.tabs[0].cursor = content.find("hello").unwrap();
        state.handle_vim_key("c", false, None);
        state.handle_vim_key("i", false, None);
        state.handle_vim_key("\"", true, Some("\""));
        assert_eq!(state.tabs[0].content, "say \"\" now");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
        assert_eq!(state.registers.get(&'"'), Some(&"hello world".to_string()));
    }

    #[test]
    fn test_di_bracket_deletes_innermost_parens_content() {
        let content = "foo(bar(baz)qux)end";
        let mut state = make_state(content, 0, None);
        state.tabs[0].cursor = content.find("baz").unwrap();
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("i", false, None);
        state.handle_vim_key("(", true, Some("("));
        assert_eq!(state.tabs[0].content, "foo(bar()qux)end");
    }

    #[test]
    fn test_text_object_with_no_match_abandons_operator_cleanly() {
        let mut state = make_state("no quotes here", 0, None);
        state.handle_vim_key("d", false, None);
        state.handle_vim_key("i", false, None);
        state.handle_vim_key("\"", true, Some("\""));
        assert_eq!(state.tabs[0].content, "no quotes here");
        assert_eq!(state.tabs[0].vim_pending_operator, None);
    }

    // ── >>/<</gU/gu operators (Task F) ────────────────────────────────────────────

    #[test]
    fn test_gt_gt_indents_current_line() {
        let mut state = make_state("one\ntwo", 0, None);
        state.handle_vim_key(".", true, Some(">")); // shifted '.' reported directly as '>'
        assert_eq!(state.tabs[0].vim_pending_operator, Some('>'));
        state.handle_vim_key(".", true, Some(">"));
        assert_eq!(state.tabs[0].content, "\tone\ntwo");
    }

    #[test]
    fn test_lt_lt_removes_leading_tab() {
        let mut state = make_state("\tone\ntwo", 0, None);
        state.handle_vim_key(",", true, Some("<"));
        state.handle_vim_key(",", true, Some("<"));
        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    #[test]
    fn test_lt_lt_removes_up_to_four_leading_spaces_when_no_tab() {
        let mut state = make_state("      one\ntwo", 0, None);
        state.handle_vim_key(",", true, Some("<"));
        state.handle_vim_key(",", true, Some("<"));
        assert_eq!(state.tabs[0].content, "  one\ntwo");
    }

    #[test]
    fn test_gt_ip_indents_paragraph_lines() {
        // `>j` isn't testable here: `j` always resolves to `NeedsGpui`
        // (same documented gap as `dj`), so a multi-line indent needs a
        // motion/text-object `resolve_vim_motion` can actually resolve —
        // `ip` (paragraph text object) exercises the same linewise-range
        // path without touching that gap.
        let mut state = make_state("one\ntwo\n\nthree", 0, None);
        state.handle_vim_key(".", true, Some(">"));
        state.handle_vim_key("i", false, None);
        state.handle_vim_key("p", false, None);
        assert_eq!(state.tabs[0].content, "\tone\n\ttwo\n\nthree");
    }

    #[test]
    fn test_g_upper_u_w_uppercases_word() {
        let mut state = make_state("one two three", 0, None);
        state.handle_vim_key("g", false, None);
        assert_eq!(state.vim_pending_trigger(), Some('g'));
        state.handle_vim_key("u", true, None); // gU
        assert_eq!(state.tabs[0].vim_pending_operator, Some('U'));
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "ONE two three");
    }

    #[test]
    fn test_gu_iw_lowercases_inner_word() {
        let mut state = make_state("ONE TWO THREE", 0, None);
        state.tabs[0].cursor = "ONE TWO THREE".find("TWO").unwrap();
        state.handle_vim_key("g", false, None);
        state.handle_vim_key("u", false, None); // gu
        state.handle_vim_key("i", false, None);
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "ONE two THREE");
    }

    #[test]
    fn test_g_upper_u_is_charwise_not_linewise_unlike_indent_operators() {
        // Distinguishes gUw (charwise, only the word changes) from an
        // indent operator's forced-linewise rule — regression guard for
        // the `matches!(operator, '>' | '<')` override in
        // vim_operator_motion_range not accidentally also catching 'U'/'u'.
        let mut state = make_state("one two\nthree", 0, None);
        state.handle_vim_key("g", false, None);
        state.handle_vim_key("u", true, None);
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].content, "ONE two\nthree"); // not the whole line
    }

    #[test]
    fn test_gu_u_doubled_form_is_not_supported_and_abandons_cleanly() {
        // Documented scope gap: gUU/guu (doubled-key linewise form) isn't
        // implemented — must abandon without crashing or corrupting state,
        // not silently misfire as something else.
        let mut state = make_state("one two", 0, None);
        state.handle_vim_key("g", false, None);
        state.handle_vim_key("u", true, None); // gU
        state.handle_vim_key("u", true, None); // second U: not a supported completion
        assert_eq!(state.tabs[0].vim_pending_operator, None);
        assert_eq!(state.tabs[0].content, "one two");
    }

    #[test]
    fn test_indent_operator_undoable() {
        let mut state = make_state("one\ntwo", 0, None);
        state.handle_vim_key(".", true, Some(">"));
        state.handle_vim_key(".", true, Some(">"));
        assert_eq!(state.tabs[0].content, "\tone\ntwo");
        state.undo();
        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    // ── Visual-mode operators (Task G) ────────────────────────────────────────────

    #[test]
    fn test_visual_d_deletes_selection_and_returns_to_normal() {
        let mut state = make_state("one two three", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None); // extend selection to (0,2)
        assert!(state.handle_vim_key("d", false, None));
        assert_eq!(state.tabs[0].content, "e two three");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.tabs[0].selection, None);
        assert_eq!(state.registers.get(&'"'), Some(&"on".to_string()));
    }

    #[test]
    fn test_visual_x_is_equivalent_to_d() {
        let mut state = make_state("one two three", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None);
        assert!(state.handle_vim_key("x", false, None));
        assert_eq!(state.tabs[0].content, "e two three");
    }

    #[test]
    fn test_visual_y_yanks_without_deleting() {
        let mut state = make_state("one two three", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None);
        assert!(state.handle_vim_key("y", false, None));
        assert_eq!(state.tabs[0].content, "one two three");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
        assert_eq!(state.registers.get(&'"'), Some(&"on".to_string()));
        assert_eq!(state.registers.get(&'0'), Some(&"on".to_string()));
    }

    #[test]
    fn test_visual_c_deletes_and_enters_insert() {
        let mut state = make_state("one two three", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None);
        assert!(state.handle_vim_key("c", false, None));
        assert_eq!(state.tabs[0].content, "e two three");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_visual_line_d_deletes_whole_lines() {
        let mut state = make_state("one\ntwo\nthree", 4, None); // on "two"
        state.vim_enter_visual_line();
        assert!(state.handle_vim_key("d", false, None));
        assert_eq!(state.tabs[0].content, "one\nthree");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_visual_line_c_keeps_line_as_empty_and_enters_insert() {
        let mut state = make_state("one\ntwo\nthree", 4, None); // on "two"
        state.vim_enter_visual_line();
        assert!(state.handle_vim_key("c", false, None));
        assert_eq!(state.tabs[0].content, "one\n\nthree");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Insert);
    }

    #[test]
    fn test_visual_charwise_gt_forces_linewise_indent() {
        // Real vim rule: `>` always indents whole lines, even from a
        // charwise (not VisualLine) selection.
        let mut state = make_state("one\ntwo", 0, None); // charwise selection covers only part of "one"
        state.vim_enter_visual();
        assert!(state.handle_vim_key(".", true, Some(">")));
        assert_eq!(state.tabs[0].content, "\tone\ntwo");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_visual_lt_unindents() {
        let mut state = make_state("\tone\ntwo", 0, None);
        state.vim_enter_visual();
        assert!(state.handle_vim_key(",", true, Some("<")));
        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    #[test]
    fn test_visual_line_gt_indents_all_selected_lines() {
        // `j`/`k` need GPUI context (not resolvable here, same limitation
        // as everywhere else in this test suite) — `w` twice extends the
        // selection from "one\n" (0,4) to "one\ntwo\n" (0,8) instead,
        // spanning two lines without touching that gap.
        let mut state = make_state("one\ntwo\nthree", 0, None);
        state.vim_enter_visual_line();
        state.handle_vim_key("w", false, None);
        state.handle_vim_key("w", false, None);
        assert_eq!(state.tabs[0].selection, Some((0, 8)));
        assert!(state.handle_vim_key(".", true, Some(">")));
        assert_eq!(state.tabs[0].content, "\tone\n\ttwo\nthree");
    }

    #[test]
    fn test_visual_g_upper_u_uppercases_only_selected_chars_not_whole_line() {
        let mut state = make_state("one two\nthree", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None); // selects "on"
        state.handle_vim_key("g", false, None);
        assert!(state.handle_vim_key("u", true, None)); // gU
        assert_eq!(state.tabs[0].content, "ONe two\nthree");
        assert_eq!(state.tabs[0].vim_mode, VimMode::Normal);
    }

    #[test]
    fn test_visual_gu_lowercases_selection() {
        let mut state = make_state("ONE two", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None);
        state.handle_vim_key("l", false, None); // selection now covers all of "ONE"
        state.handle_vim_key("g", false, None);
        assert!(state.handle_vim_key("u", false, None)); // gu
        assert_eq!(state.tabs[0].content, "one two");
    }

    #[test]
    fn test_visual_tilde_toggles_case_of_selection() {
        let mut state = make_state("One two", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("l", false, None); // selects "On"
        assert!(state.handle_vim_key("`", true, Some("~")));
        assert_eq!(state.tabs[0].content, "oNe two");
    }

    #[test]
    fn test_visual_o_swaps_selection_ends() {
        let mut state = make_state("one two three", 0, None);
        state.vim_enter_visual(); // selection (0,1), cursor 1
        state.handle_vim_key("l", false, None); // selection (0,2), cursor 2
        assert_eq!(state.tabs[0].selection, Some((0, 2)));
        assert!(state.handle_vim_key("o", false, None));
        assert_eq!(state.tabs[0].selection, Some((2, 0)));
        assert_eq!(state.tabs[0].cursor, 0);
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual); // stays in Visual
    }

    #[test]
    fn test_visual_pending_find_wins_over_operator_start() {
        // Regression for the collision this session's own test suite
        // caught: `f` then `d` must complete the find (target 'd'), not
        // misfire as starting the delete operator.
        let mut state = make_state("abcdef", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("f", false, None);
        assert!(state.handle_vim_key("d", false, None));
        assert_eq!(state.tabs[0].cursor, 3);
        assert_eq!(state.tabs[0].selection, Some((0, 3)));
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual); // not executed as an operator
    }

    #[test]
    fn test_visual_pending_find_v_target_wins_over_exit_visual() {
        // Bug: in Visual mode, `f` then `v` should complete the find with
        // target 'v', not exit visual mode. The `v`-to-exit-visual logic
        // must not run when a pending find trigger exists.
        // Start at position 0 ('a'). vim_enter_visual moves cursor to 1 ('v')
        // and creates selection (0, 1). Then `f` then `v` searches forward
        // from position 1, finding the next 'v' at position 4.
        let mut state = make_state("avcbvc", 0, None);
        state.vim_enter_visual();
        state.handle_vim_key("f", false, None);
        assert!(state.handle_vim_key("v", false, None)); // complete find, target 'v'
        assert_eq!(state.tabs[0].cursor, 4); // second 'v' is at index 4
        assert_eq!(state.tabs[0].vim_mode, VimMode::Visual); // still in Visual, not exited
    }

    #[test]
    fn test_apply_card_style_pocket_sets_bold_size_box_and_center() {
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.alignment, Alignment::Center);
        assert!(para.runs.iter().all(|r| r.bold));
        assert!(para.runs.iter().all(|r| r.size == 52));
        assert!(para.runs.iter().all(|r| r.box_format));
        assert_eq!(para.heading, 1);
    }

    #[test]
    fn test_apply_card_style_hat_sets_double_underline_not_box() {
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Hat);

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.alignment, Alignment::Center);
        assert!(para.runs.iter().all(|r| r.size == 44));
        assert!(para.runs.iter().all(|r| r.double_underline));
        assert!(para.runs.iter().all(|r| !r.box_format));
        assert_eq!(para.heading, 2);
    }

    #[test]
    fn test_apply_card_style_block_sets_underline_not_double() {
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Block);

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.alignment, Alignment::Center);
        assert!(para.runs.iter().all(|r| r.size == 32));
        assert!(para.runs.iter().all(|r| r.underline));
        assert!(para.runs.iter().all(|r| !r.double_underline));
        assert_eq!(para.heading, 3);
    }

    #[test]
    fn test_apply_card_style_tag_is_left_aligned_no_box_or_underline() {
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Tag);

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.alignment, Alignment::Left);
        assert!(para.runs.iter().all(|r| r.size == 26));
        assert!(para.runs.iter().all(|r| r.bold));
        assert!(para.runs.iter().all(|r| !r.box_format && !r.underline && !r.double_underline));
        assert_eq!(para.heading, 4);
    }

    #[test]
    fn test_apply_card_style_sets_heading_on_correct_line_when_cursor_on_second_line() {
        let paragraphs = vec![
            Paragraph { runs: vec![run_plain("first line")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("second line")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        // content is "first line\nsecond line" — byte 11 is the start of "second line".
        let mut state = make_state_with_paragraphs(paragraphs, 11);
        state.apply_card_style(CardStyleKind::Hat);

        assert_eq!(state.tabs[0].paragraphs[0].heading, 0, "first line untouched");
        assert_eq!(state.tabs[0].paragraphs[1].heading, 2, "second line marked Hat");
    }

    #[test]
    fn test_jump_to_line_moves_cursor_and_arms_scroll_flag() {
        let mut state = make_state("one\ntwo\nthree", 0, None);
        assert!(!state.tabs[0].pending_scroll_to_cursor);

        state.jump_to_line(2);

        assert_eq!(state.tabs[0].cursor, 8); // start of "three"
        assert!(state.tabs[0].pending_scroll_to_cursor);
        assert_eq!(state.tabs[0].selection, None);
    }

    #[test]
    fn test_apply_card_style_end_to_end_through_wikifi_export() {
        // End-to-end: applies each card style through the same
        // AppState::apply_card_style the ribbon/keybinds call, then feeds
        // the result straight into wikifi_export::export_to_markdown — the
        // whole pipeline this was silently broken for before apply_card_style
        // set Paragraph.heading (wikify_export.rs's own test covers the
        // export function in isolation with hand-built headings).
        let paragraphs = vec![
            Paragraph { runs: vec![run_plain("Case Title")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("Off-case Subtitle")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("Block heading")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("Tag text")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("plain body text")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);

        for (line, kind) in [
            (0, CardStyleKind::Pocket),
            (1, CardStyleKind::Hat),
            (2, CardStyleKind::Block),
            (3, CardStyleKind::Tag),
        ] {
            state.set_cursor_from_line_col(line, 0);
            state.apply_card_style(kind);
        }

        let tab = &state.tabs[0];
        let markdown = crate::wikifi_export::export_to_markdown(&tab.paragraphs, &tab.content);
        assert_eq!(
            markdown,
            "# Case Title\n## Off-case Subtitle\n### Block heading\n#### Tag text\nplain body text\n"
        );
    }
}
