use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::case_converter;
use crate::docx_parser::{Alignment, CardStyle, DocxOrigin, Paragraph, Run, create_new_docx, paragraphs_to_plain_text, parse_docx};
use crate::document_ops::{apply_format_op, apply_formatting, apply_paragraph_alignment, is_uniformly_active, ranges_matching_format, reset_card_style_in_range, resolve_position, runs_in_range, sync_delete_range, sync_insert_char, sync_insert_str, sync_insert_str_with_runs, toggled_off, FormatOp};
use crate::recovery::RecoveryEntry;
use crate::wikifi_export;

/// Rapid edits within this window of the previous undo-stack push are
/// coalesced into the same undo step (spec 4.5), so e.g. typing a whole
/// word doesn't need one Ctrl+Z per character.
const UNDO_COALESCE_WINDOW: Duration = Duration::from_millis(300);
/// Maximum number of snapshots kept on a tab's undo stack (spec 4.5) — the
/// ceiling for small/typical documents. Large documents are capped further
/// by `UNDO_STACK_BYTE_BUDGET` instead (see `undo_stack_cap_for_snapshot_size`).
const UNDO_STACK_CAP: usize = 200;
/// Total approximate bytes `undo_stack`/`redo_stack` may hold at once.
/// `performance_plan.md`'s "undo/redo stack memory" finding: at
/// `UNDO_STACK_CAP` full `(content, paragraphs)` clones each, a large,
/// heavily-formatted document is a real multi-hundred-MB steady state, not
/// a hypothetical. Chosen so a ~5MB document (a large real debate case
/// file) still keeps dozens of undo levels, not just `UNDO_STACK_MIN_CAP`.
const UNDO_STACK_BYTE_BUDGET: usize = 100_000_000;
/// Even a huge document keeps at least this many undo levels — a fixed
/// memory budget alone would otherwise let one very large document shrink
/// undo depth to an unusably small number.
const UNDO_STACK_MIN_CAP: usize = 10;

/// What `condense_selection` (no pilcrows) leaves behind at each collapsed
/// newline: a real space, so condensed text still reads like one, plus a
/// zero-width space that renders as nothing but is real text —
/// `uncondense_selection`'s marker to find exactly where a newline used to
/// be without also matching an ordinary space the user typed.
const CONDENSE_MARKER: &str = "\u{200B} ";

/// `uncondense_selection`'s core: turns every `CONDENSE_MARKER` and every
/// `¶` in `text` back into a real newline, whichever condense variant
/// produced it (or both, if the selection mixes text condensed both ways).
fn uncondense_markers(text: &str) -> String {
    text.replace(CONDENSE_MARKER, "\n").replace('¶', "\n")
}

/// Rough byte-size estimate of one undo/redo snapshot. Only used to keep
/// the stacks' total memory bounded on large documents, not for anything
/// content-accuracy-sensitive, so an approximation (content plus every
/// run's string fields) is enough — no need to walk anything not already
/// owned by the snapshot itself.
fn snapshot_byte_estimate(content: &str, paragraphs: &[Paragraph]) -> usize {
    content.len()
        + paragraphs
            .iter()
            .flat_map(|p| &p.runs)
            .map(|r| {
                r.text.len()
                    + r.highlight_color.len()
                    + r.font.as_deref().map_or(0, str::len)
                    + r.color.as_deref().map_or(0, str::len)
            })
            .sum::<usize>()
}

/// How many snapshots the undo/redo stacks should keep given the current
/// document's approximate snapshot size — `UNDO_STACK_CAP` for small
/// documents, shrinking proportionally as `snapshot_bytes` grows so total
/// stack memory stays within `UNDO_STACK_BYTE_BUDGET`, never below
/// `UNDO_STACK_MIN_CAP`.
fn undo_stack_cap_for_snapshot_size(snapshot_bytes: usize) -> usize {
    let budget_based = UNDO_STACK_BYTE_BUDGET / snapshot_bytes.max(1);
    budget_based.clamp(UNDO_STACK_MIN_CAP, UNDO_STACK_CAP)
}

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
    /// Bumped on every real mutation to `content`/`paragraphs` — starts at
    /// `push_undo_snapshot()` (this codebase's de facto "a real edit is
    /// about to happen" choke point, called by nearly every mutating
    /// method), plus `undo()`/`redo()` which replace both fields wholesale
    /// without going through it. Lets `TextEditor`'s row-wrap cache detect
    /// "did the text actually change since I last wrapped it" without
    /// comparing the full `content` string on every render.
    pub content_version: u64,
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
    /// The `content_version` most recently written to a crash-recovery
    /// snapshot. Equal to `content_version` means the snapshot on disk is
    /// current and no write is due. Left unchanged on a failed write so the
    /// next tick retries.
    pub last_snapshot_version: u64,
    /// How long this tab's last recovery snapshot took to write. Feeds
    /// `recovery::snapshot_interval`, so an expensive document snapshots
    /// less often than a cheap one. `None` until the first write.
    pub last_snapshot_cost: Option<Duration>,
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
    /// Checklist: Settings -> Vim Mode. Keystrokes typed so far toward a
    /// user-configured vim-keybind sequence (`AppState.vim_keybinds`),
    /// e.g. `"z"` while mid-typing `"zs"` for Save. Unrelated to
    /// `vim_command_buf` — that field is shaped for exactly one digit-run
    /// plus one trailing trigger char and is already fully claimed by real
    /// vim's own `g`/`f`/`F`/`t`/`T` bookkeeping; this is a separate,
    /// arbitrary-length buffer for a separate concern. Mutually exclusive
    /// with every other per-tab vim pending-state field by construction: it
    /// only ever becomes non-empty via `handle_vim_normal_key`'s final
    /// catch-all, which is only reached once every other pending state has
    /// already declined the keystroke — see that function's own comment.
    pub vim_keybind_seq: String,
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
    /// Indices of the heading paragraphs the user has collapsed.
    ///
    /// Kept on the tab rather than on `Paragraph` so the document model stays
    /// exactly what gets written to the .docx — folding is a view state and
    /// must never reach the file.
    ///
    /// ponytail: keyed by paragraph *index*, and cleared wholesale whenever the
    /// paragraph count changes (`sync_fold_state`). Typing inside a paragraph
    /// keeps folds; splitting or merging one drops them all rather than
    /// silently folding the wrong sections. The alternative — a stable id per
    /// paragraph — means a new field on `Paragraph` and touching all 79 of its
    /// struct literals, which is not worth it until someone actually edits
    /// heavily while folded.
    pub folded_headings: std::collections::HashSet<usize>,
    /// Paragraph count `folded_headings` was built against — see above.
    pub folded_para_count: usize,
    /// Bumped on every fold change so the editor's row cache invalidates.
    /// Folding is not a content edit, so `content_version` must not move.
    pub fold_version: u64,
    /// Byte ranges last matched by "Select similar formatting" (Doc Menu),
    /// drawn exactly like `selection` and used in its place by
    /// `apply_formatting_to_selection`.
    ///
    /// Deliberately *not* folded into `selection`: that field is a single
    /// (anchor, focus) pair driven by the caret, and ~80 call sites assume
    /// it. This is a separate, read-mostly overlay that only formatting
    /// commands consult, and any keystroke or click clears it (see
    /// `clear_similar_selection`) so it can never go stale against edits.
    ///
    /// ponytail: formatting ops only — copy/cut/delete still act on
    /// `selection`. Widen when someone actually wants to cut every tag at
    /// once.
    pub similar_ranges: Vec<(usize, usize)>,
}

/// A dirty tab reduced to exactly what a recovery snapshot needs, with no
/// GPUI `Entity` borrow involved.
///
/// Exists so the panic hook — which fires on the panicking thread, outside
/// any GPUI context — can still write snapshots. The background snapshot
/// task refreshes a global copy of this on each tick.
#[derive(Clone)]
pub struct TabSnapshot {
    pub id: usize,
    pub paragraphs: Vec<Paragraph>,
    pub origin: Option<Arc<DocxOrigin>>,
    pub file_path: Option<PathBuf>,
    pub title: String,
}

/// A single empty paragraph containing one default (unformatted) run — the
/// starting state for `Tab.paragraphs` before any docx has been parsed into
/// it. Never `vec![]`: every rich-text-aware function assumes at least one
/// paragraph and run always exist.
pub fn default_paragraphs() -> Vec<Paragraph> {
    vec![Paragraph { runs: vec![Run::default()], heading: 0, alignment: Alignment::default(), unsupported_xml: None }]
}

/// The file explorer sidebar's starting width in pixels — not persisted
/// across launches (deliberate: dragging the sidebar wider/narrower is a
/// per-session convenience, not a saved preference).
///
/// Sized so the header's whole control cluster — the Files/Nav pair plus the
/// refresh and new-file buttons, about 144px together — fits alongside a
/// readable folder name. At the previous 240 the last two were pushed off the
/// edge on launch.
pub const DEFAULT_SIDEBAR_WIDTH: f32 = 300.0;

/// Clamps a proposed sidebar width (from dragging its resize handle,
/// `main_window.rs`) to a usable range: never so narrow file names become
/// unreadable, never so wide it swallows the editor.
pub fn clamp_sidebar_width(width: f32) -> f32 {
    width.clamp(180.0, 480.0)
}

/// One of the editor's two side-by-side panes (`notes/split_view_plan.md`).
///
/// `Primary` is the only pane when the split is closed, so it is also the
/// default — every pre-split code path keeps behaving as it always did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pane {
    #[default]
    Primary,
    Secondary,
}

/// The primary pane's share of the editor width. Clamped so neither pane can
/// be dragged away to nothing — same reasoning (and same shape) as
/// `clamp_sidebar_width`.
pub fn clamp_split_ratio(ratio: f32) -> f32 {
    ratio.clamp(0.2, 0.8)
}

impl Tab {
    /// A never-saved, never-typed-in tab — the blank "New Tab" the app opens
    /// with, or one the user made and hasn't used yet.
    ///
    /// Opening a file reuses such a tab rather than leaving it stranded beside
    /// the document (`open_file`), and the editor paints its placeholder text
    /// on one. `is_modified` alone isn't enough: a tab can be modified back to
    /// empty by undo, and that still shouldn't be silently replaced.
    pub fn is_blank_new_tab(&self) -> bool {
        self.file_path.is_none() && self.content.is_empty() && !self.is_modified
    }

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
            content_version: 0,
            is_modified: false,
            paragraphs: default_paragraphs(),
            docx_origin: None,
            pending_format: None,
            cursor: 0,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
            last_snapshot_version: 0,
            last_snapshot_cost: None,
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
            vim_keybind_seq: String::new(),
            vim_search_direction: true,
            vim_jump_back: Vec::new(),
            vim_jump_forward: Vec::new(),
            pending_scroll_to_cursor: false,
            folded_headings: std::collections::HashSet::new(),
            folded_para_count: 0,
            fold_version: 0,
            similar_ranges: Vec::new(),
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
            content_version: 0,
            is_modified: false,
            paragraphs: default_paragraphs(),
            docx_origin: None,
            pending_format: None,
            cursor: 0,
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
            last_snapshot_version: 0,
            last_snapshot_cost: None,
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
            vim_keybind_seq: String::new(),
            vim_search_direction: true,
            vim_jump_back: Vec::new(),
            vim_jump_forward: Vec::new(),
            pending_scroll_to_cursor: false,
            folded_headings: std::collections::HashSet::new(),
            folded_para_count: 0,
            fold_version: 0,
            similar_ranges: Vec::new(),
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

/// What a file-explorer right-click landed on (`found_bugs.md`'s Forgotten
/// Implicit Feature: right-click to delete or create). Determines both what
/// "Delete" acts on and which directory "New File" creates into.
#[derive(Clone, Debug, PartialEq)]
pub enum FileContextMenuTarget {
    File(PathBuf),
    Dir(PathBuf),
    /// Right-click on empty space below the tree — "New File" creates at
    /// `working_directory`'s root; "Delete" has nothing to act on.
    Background,
}

/// State for the file explorer's right-click menu. `position` is a window-
/// relative `(x, y)` in pixels — a plain tuple, not a `gpui::Point`, since
/// `state.rs` is deliberately gpui-free (see the rest of this file); the
/// view layer (`file_explorer.rs`) converts at its own boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct FileContextMenu {
    pub position: (f32, f32),
    pub target: FileContextMenuTarget,
    /// Set by clicking "Delete" once, before the destructive
    /// `fs::remove_file` actually runs — a real filesystem delete has no
    /// undo, so the menu shows a "Delete <name>? Confirm / Cancel" step
    /// instead of deleting on the first click.
    pub confirming_delete: bool,
}

/// State for the text editor's right-click menu. `position` is window-relative
/// `(x, y)` in pixels, same plain-tuple convention (and same reason) as
/// `FileContextMenu` — `state.rs` stays gpui-free and the view converts at its
/// own boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorContextMenu {
    pub position: (f32, f32),
    /// The misspelled word under the click, or `None` if the right-click
    /// didn't land on one — which is what gates the suggestions and the
    /// "Add to Dictionary" item.
    pub spell_target: Option<SpellTarget>,
}

/// A misspelled word a right-click landed on, resolved once at click time.
///
/// `suggestions` is filled here rather than at render time on purpose:
/// `spellcheck::suggest` is a dictionary *search*, orders of magnitude slower
/// than the per-word `check` the squiggles use, so it must never run on a
/// frame. The `line`/`start_col`/`end_col` triple is what "replace with this
/// suggestion" feeds back to `set_cursor_from_line_col`/
/// `extend_selection_to_line_col`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpellTarget {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub word: String,
    pub suggestions: Vec<String>,
}

/// Which of the find bar's two text fields keystrokes go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindField {
    Query,
    Replace,
}

/// State for the find/replace bar (`src/find_bar.rs`), `None` when closed.
///
/// Deliberately app-wide rather than per-tab: the bar is a single floating
/// panel under the ribbon, and carrying the query across a tab switch is what
/// every editor does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindBar {
    pub query: String,
    pub replacement: String,
    pub focus: FindField,
    /// Match count and which one the cursor is on (1-based), recomputed after
    /// every query change or jump — drives the "3 of 12" readout.
    pub match_count: usize,
    pub current_match: usize,
}

impl Default for FindField {
    fn default() -> Self {
        FindField::Query
    }
}

/// A close action (tab-close `×` or the app-close `×`) awaiting the user's
/// answer to "save changes before closing?", set by `request_close_tab`/
/// `request_close_app` whenever the target has unsaved changes. `None` means
/// no confirmation is in flight — `close_confirm.rs` renders nothing in that
/// case, and `MainWindow` only mounts it at all while this is `Some`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PendingClose {
    Tab(usize),
    App,
}

/// The shared application state, owned as a GPUI Model and read/written by all views.
pub struct AppState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    /// Set whenever `active_tab` changes to a tab the user didn't get there
    /// by clicking directly into the editor (tab-bar click, file-open from
    /// the sidebar) — GPUI keyboard focus doesn't move on its own just
    /// because `active_tab` did, so without this the text editor's
    /// `FocusHandle` is left wherever it was (often nowhere), and Enter/keys
    /// silently stop reaching it until the user clicks into the editor
    /// again. `TextEditor::render` checks and clears this once per frame,
    /// mirroring `Tab::pending_scroll_to_cursor`'s same check-and-clear idiom.
    pub pending_focus_editor: Option<Pane>,
    pub next_tab_id: usize,
    /// Closed tabs' file paths, most-recently-closed last — a LIFO stack
    /// `reopen_closed_tab` pops from. Only file-backed tabs push onto it;
    /// a blank "New Tab" has nothing on disk to reopen. Deliberately not
    /// persisted — a fresh session starts with nothing to reopen.
    pub closed_tabs: Vec<PathBuf>,
    pub sidebar_visible: bool,
    /// File explorer sidebar width in pixels, changed by dragging its
    /// resize handle (`main_window.rs`). Deliberately not persisted to
    /// settings.conf — resets to `DEFAULT_SIDEBAR_WIDTH` every launch.
    pub sidebar_width: f32,
    /// Open state of the file explorer's right-click menu, `None` when
    /// closed. See `FileContextMenu`.
    pub file_context_menu: Option<FileContextMenu>,
    /// Open state of the text editor's right-click menu, `None` when closed.
    /// See `EditorContextMenu`.
    pub editor_context_menu: Option<EditorContextMenu>,
    /// Open state of the find/replace bar, `None` when closed. See `FindBar`.
    pub find_bar: Option<FindBar>,
    /// Whether the word-count panel (`src/word_count.rs`) is showing.
    pub word_count_visible: bool,
    /// The speech timer popup and its clock (`src/timer.rs`). Lives here
    /// rather than in the view so the `start_timer` keybind and the ribbon's
    /// Timer button reach the same state.
    pub timer: crate::timer::TimerState,
    /// settings.conf `[FORMATTING] spreading_wpm` — the reading rate the word
    /// count panel divides by for its time estimate. "Spreading" is debate's
    /// term for reading at speed, so this is deliberately not a prose-reading
    /// default.
    pub spreading_wpm: u32,
    /// Colors the user added from the Font Color and HL Color dropdowns'
    /// picker, oldest first, as `0xRRGGBB`. Persisted to settings.conf's
    /// `[FORMATTING]` section so they survive a restart, capped at
    /// `MAX_CUSTOM_COLORS`. Kept as two lists on purpose — see
    /// `CustomColorTarget`.
    pub custom_font_colors: Vec<u32>,
    pub custom_highlight_colors: Vec<u32>,
    /// Which view the left sidebar shows — the file tree, or (Nav) a
    /// heading outline of the active tab's Pocket/Hat/Block/Tag lines.
    /// Toggled from two places that both flip the same field: the ribbon's
    /// Nav button, and a Files/Nav button pair in the sidebar's own header.
    pub sidebar_mode: SidebarMode,
    pub settings_visible: bool,
    /// Set while a tab-close or app-close is waiting on the user's
    /// save/discard/cancel answer (`close_confirm.rs`). See `PendingClose`.
    pub pending_close: Option<PendingClose>,
    /// Documents recovered from a previous session that ended without a
    /// clean save, newest first. Non-empty makes `main_window.rs` mount
    /// `RecoveryPrompt`; each recovery action pops one entry.
    pub pending_recovery: Vec<RecoveryEntry>,
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
    /// Checklist: Settings -> Vim Mode. Only consulted while `vim_enabled`
    /// and the active tab's `vim_mode == VimMode::Normal` — see
    /// `handle_vim_normal_key`'s sequence-continuation check and its
    /// modified final catch-all.
    pub vim_keybinds: crate::vim_keybinds::VimKeybinds,
    /// A vim-keybind's resolved action, staged here because `state.rs` has
    /// no `cx`/`window` to actually dispatch it — same mailbox pattern as
    /// `pending_clipboard_sync` below. Drained by `text_editor.rs`'s
    /// `process_key_plain`, immediately after a vim keystroke is handled,
    /// via `take_pending_vim_action` + `window.dispatch_action`.
    pub pending_vim_action: Option<crate::keybinds::KeybindAction>,
    pub theme: crate::theme::ThemeKind,
    /// Light or dark variant of `theme`. Orthogonal to the theme itself —
    /// every `ThemeKind` ships both, so this only swaps the palette's
    /// lightness, keeping the user's chosen color family.
    pub theme_mode: crate::theme::ThemeMode,
    pub theme_color_mode: crate::theme::ThemeColorMode,
    /// The user's imported theme (Settings -> Themes -> Import Theme),
    /// `(dark, light)`, loaded from `custom_theme_path()` at startup if that
    /// file exists. `None` until an import happens; only one at a time —
    /// importing again replaces it wholesale. Selected via
    /// `theme == ThemeKind::Custom`; resolve colors through
    /// `current_palette()`, never the bare `theme::palette()` free function,
    /// which has no access to this runtime state.
    pub custom_theme: Option<(crate::theme::Palette, crate::theme::Palette)>,
    /// `normal_text_size` from settings.conf, in half-points (`Run.size`'s
    /// unit) — the default body text size: what any run with no explicit
    /// `FontSize` override renders at (`text_editor.rs`'s `normal_size_px`,
    /// which covers a brand-new document's single default run same as any
    /// other plain-typed text), and the size "Clear Formatting" resets a
    /// line back to. See `load_normal_text_size_half_points`.
    pub normal_text_size_half_points: u16,
    /// `pocket_size`/`block_size`/`tag_size`/`cite_size` from settings.conf,
    /// in half-points (`Run.size`'s unit) — the font sizes `apply_card_style`
    /// applies for those styles (Hat's stays fixed via
    /// `CardStyleKind::font_size`; not requested as configurable). See
    /// `load_font_size_half_points`.
    pub pocket_size_half_points: u16,
    pub block_size_half_points: u16,
    pub tag_size_half_points: u16,
    /// The size Cite applies alongside bold (`main_window.rs`'s `CiteAction`
    /// handler and the ribbon's Cite button, `formatting_ribbon.rs`) — Cite
    /// isn't a `CardStyleKind` (it targets the selection, not the whole
    /// line), so it keeps its own field rather than sharing the enum.
    pub cite_size_half_points: u16,
    /// `small_size` from settings.conf, in half-points (`Run.size`'s unit) —
    /// the size Shrink (`shrink_text`) sets non-underlined selected text to.
    pub small_size_half_points: u16,
    /// Editor text zoom multiplier (`found_bugs.md`'s Ctrl+=/Ctrl+-/Ctrl+0
    /// zoom, rebuilt from scratch — no trace of a prior implementation
    /// survived in git history). Applied only to the document text
    /// (`text_editor.rs`'s font size/line height/wrap and hit-testing
    /// math), not the surrounding app chrome — a deliberate scope
    /// narrowing the user confirmed. `1.0` is 100% (no zoom). Not
    /// persisted to settings.conf — resets to 100% each launch, matching
    /// how a fresh Word session doesn't remember its last zoom level either.
    pub zoom: f32,
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
    /// settings.conf `highlight_color` — the color the Highlight button and
    /// keybind apply. A Word highlight-color name (any of the six the ribbon's
    /// HL Color dropdown offers), or a bare 6-digit hex; resolved by
    /// `text_editor::highlight_color_hex`, same as everywhere else.
    ///
    /// Edited by hand in settings.conf, not in the settings modal — the
    /// dropdown is where colors get picked.
    pub highlight_color: String,
    /// settings.conf `analytic_color` — the text color the Analytic style
    /// applies, as a 6-digit hex (`Run.color`'s own form, no leading `#`).
    pub analytic_color: String,
    /// settings.conf `standardize_highlight_exception` — a highlight color
    /// that "Standardize highlighting with exception" leaves alone. Empty
    /// means no exception, and that command behaves like the plain one.
    pub standardize_highlight_exception: String,
    /// Which run formatting the Emphasis command applies. Independent, not
    /// mutually exclusive — Word's own "emphasis" is whatever combination a
    /// squad has standardised on.
    ///
    /// Stored and surfaced only; nothing reads these yet. The Emphasis button
    /// still applies plain bold until it is wired up separately.
    pub emphasis_bold: bool,
    pub emphasis_underline: bool,
    pub emphasis_box: bool,
    /// Whether the paste command (f2 / the ribbon's Paste button) condenses
    /// the pasted text, collapsing its newlines instead of keeping them.
    pub paste_condense: bool,
    /// When condensing, mark each collapsed newline with a pilcrow instead of
    /// a plain space. Only meaningful while `paste_condense` is on.
    pub paste_condense_pilcrow: bool,
    /// The settings.conf this state reads from and writes back to.
    ///
    /// Held as a field rather than calling `settings_conf_path()` at each
    /// write site so the test constructor can point at a temp file. Without
    /// it, running `cargo test` writes through the real
    /// `~/.vimbatim/settings.conf` — `persist_custom_colors` alone would
    /// overwrite a developer's actual saved swatches with a test fixture's.
    pub settings_path: PathBuf,
    /// settings.conf `[SPELLCHECK]`. When false the editor skips the whole
    /// spellcheck path for the cost of one bool check per row.
    pub spellcheck_enabled: bool,
    /// The squiggle color, kept as the raw settings.conf string (a Word
    /// color name like `red`, or a bare 6-digit hex) and resolved through
    /// `text_editor::highlight_color_hex` at paint time — that function
    /// already handles both forms, so there's nothing to parse here.
    pub spellcheck_underline_color: String,
    /// Words the user added via the right-click menu's "Add to Dictionary",
    /// lowercased. Backed by `user_dictionary.txt` next to settings.conf.
    ///
    /// `Rc`-wrapped so `TextEditor::render` can hand it to the `uniform_list`
    /// closure (which must be `'static`, so it can't borrow) for the price of
    /// a refcount bump instead of deep-cloning every word on every frame.
    pub user_dictionary: Rc<HashSet<String>>,
    pub invisibility_mode: bool,
    /// Renders the document inside an 8.5x11in page centered in the editing
    /// pane, wrapped to the page's text column instead of the viewport.
    /// Continuous scroll, not true pagination — no page breaks (see
    /// `PAGE_WIDTH_PX`/`PAGE_MARGIN_PX` in `text_editor.rs`). Not persisted,
    /// same as `invisibility_mode` above.
    pub print_layout: bool,
    pub split_view: bool,
    /// The tab shown in the secondary pane, as a stable `Tab.id` — never an
    /// index.
    ///
    /// `close_tab` shifts every later index, and this codebase has been bitten
    /// by that twice already (the Switch Tab menu and the recovery snapshot
    /// loop both key by id for the same reason). A stale index here would
    /// silently point the second pane at the wrong document.
    pub secondary_tab_id: Option<usize>,
    /// Which pane the user is editing in. `active_tab` always names this
    /// pane's tab — see `focus_pane`.
    pub focused_pane: Pane,
    /// The primary pane's tab, as a stable id, remembered while focus is in
    /// the *secondary* pane.
    ///
    /// Without it the primary pane has nowhere to point once `active_tab` has
    /// moved to the secondary's document: both panes would resolve to the same
    /// tab and render the same text, and focusing back would have nothing to
    /// restore. `None` before focus has ever left Primary, where `active_tab`
    /// is still the answer.
    pub primary_tab_id: Option<usize>,
    /// The primary pane's share of the editor width, `clamp_split_ratio`'d.
    /// Deliberately not persisted, matching `sidebar_width`.
    pub split_ratio: f32,
    /// Reading mode: the split pane and sidebar are hidden, and Left/Right
    /// page through the document a screenful at a time
    /// (`TextEditor::page_scroll`).
    pub read_mode: bool,
    /// Whether the sidebar was showing before read mode hid it, so leaving
    /// read mode puts it back rather than stranding the user without a file
    /// tree.
    sidebar_before_read_mode: bool,
    /// True only while the divider is actually being dragged.
    ///
    /// The editor's row cache is keyed on viewport width, so a drag
    /// invalidates it on every mouse-move — and a miss is a full re-wrap of the
    /// *whole document* (measured at ~0.9ms per 500 paragraphs in release,
    /// several times that in debug), paid twice because both panes re-render.
    /// At mouse-move rates that is a freeze on any real card file. While this
    /// is set, `TextEditor` keeps painting its existing row tables and re-wraps
    /// once on release instead.
    pub split_dragging: bool,
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
    /// The run marker this card style stamps onto its line — what identifies
    /// it afterwards, rather than re-deriving it from bold + font size.
    fn card_style(&self) -> CardStyle {
        match self {
            CardStyleKind::Pocket => CardStyle::Pocket,
            CardStyleKind::Hat => CardStyle::Hat,
            CardStyleKind::Block => CardStyle::Block,
            CardStyleKind::Tag => CardStyle::Tag,
        }
    }

    fn heading_level(&self) -> u8 {
        match self {
            CardStyleKind::Pocket => 1,
            CardStyleKind::Hat => 2,
            CardStyleKind::Block => 3,
            CardStyleKind::Tag => 4,
        }
    }
}

/// Resolves settings.conf's real path: inside the per-user application data
/// directory (`recovery::app_data_dir()` — `~/.vimbatim` on macOS/Linux,
/// `%APPDATA%\vimbatim` on Windows), the same place crash.log and the
/// recovery snapshots already live.
///
/// This used to resolve next to the running executable, which was wrong in
/// three separate ways on a packaged macOS `.app`:
///
/// 1. cargo-bundle puts the binary in `Contents/MacOS/` but everything in
///    `[package.metadata.bundle] resources` in `Contents/Resources/`, so the
///    shipped settings.conf was never in the directory being read.
/// 2. Writing settings back (the settings modal) meant writing *inside* the
///    app bundle, which invalidates its code signature.
/// 3. That write fails outright when the app runs from a read-only DMG or has
///    been Gatekeeper-translocated to a random read-only path.
///
/// It was also wrong for plain `cargo run`, where the executable lives in
/// `target/debug/` rather than the repo root — settings written by the modal
/// (to a CWD-relative path) and settings read at startup were two different
/// files, so every change appeared to revert on relaunch.
///
/// The user data directory has none of these problems, is writable on every
/// platform without extra permissions, and is already the one path
/// `FIRST_LAUNCH.txt` tells testers about. `ensure_settings_file` seeds it
/// from the bundled defaults on first launch.
pub fn settings_conf_path() -> PathBuf {
    crate::recovery::app_data_dir().join("settings.conf")
}

/// Locates the pristine `default_settings.conf` that ships *with the build* —
/// the seed for a first launch, and what "Reset to Defaults" restores from.
///
/// Unlike settings.conf this one is read-only and ships alongside the
/// executable, so it has to be hunted in the platform's install layout:
/// next to the binary (Windows/Linux, and `cargo build`'s `target/<profile>/`
/// once `run.sh` has placed it), then `../Resources/` (a macOS `.app`, where
/// cargo-bundle puts declared resources), and finally the bare relative path
/// so running from a source checkout works with no setup at all.
pub fn bundled_default_settings_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join("default_settings.conf");
            if beside.exists() {
                return beside;
            }
            // macOS .app: Contents/MacOS/vimbatim -> Contents/Resources/
            let resources = dir.join("../Resources/default_settings.conf");
            if resources.exists() {
                return resources;
            }
        }
    }
    PathBuf::from("default_settings.conf")
}

/// Creates the user data directory and seeds settings.conf from the bundled
/// defaults if it isn't there yet. Called once at startup, before anything
/// reads a setting.
///
/// Best-effort throughout: with no settings.conf every loader already falls
/// back to its own hardcoded default, so a failure here costs the user their
/// *preferred* defaults, never the ability to launch.
pub fn ensure_settings_file() {
    let path = settings_conf_path();
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::copy(bundled_default_settings_path(), &path) {
        log_line(&format!("[settings] couldn't seed {}: {e}", path.display()));
    }
}

/// Fixed crash-log location for the panic hook (`main.rs`) — always the
/// same path so a tester can find it without hunting, and so the "First
/// Launch" doc can just tell them the one path for their OS
/// (`closed_beta_plan.md` §5): `~/.vimbatim/crash.log` on macOS/Linux,
/// `%APPDATA%\vimbatim\crash.log` on Windows. Both are writable without
/// extra permissions, unlike the install directory a packaged `.app`/`.exe`
/// may live in.
pub fn crash_log_path() -> PathBuf {
    crate::recovery::app_data_dir().join("crash.log")
}

/// `println!`/`eprintln!` replacement for anything reachable outside
/// `#[cfg(test)]`. `windows_subsystem = "windows"` (main.rs, added so
/// double-clicking the .exe stops opening a console) means `GetStdHandle`
/// returns null with no console attached — every `print_to` call in std then
/// hits the write error and *panics* (`library/std/src/io/stdio.rs`'s
/// `print_to`: "failed printing to {label}"), turning a routine message like
/// "not a .docx" into a crash. Writes to the same `crash_log_path()` file
/// `main.rs`'s panic hook and `load_bundled_fonts` already use instead —
/// best-effort, a failure here must never itself panic.
pub fn log_line(msg: &str) {
    let path = crash_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(file, "{msg}");
    }
}

/// The file explorer's starting directory when there's no prior working
/// directory to restore — the fallback `AppState::new()` uses when
/// `load_working_directory` finds no `working_directory` line in
/// settings.conf (a fresh install, or a settings.conf predating this
/// setting). Prefers the user's home directory
/// (`Documents` on Windows, matching Explorer's own default save location)
/// over the process's CWD: a packaged `.app`/`.exe` launched by
/// double-click has no guaranteed CWD, and opening the file tree at some
/// unrelated system directory (e.g. `/` on macOS) reads as broken on first
/// launch (`closed_beta_plan.md` §0). Falls back to `current_dir()`, then
/// `.`, only if the platform's home-directory env var is unset.
fn default_working_directory() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|home| home.join("Documents"))
    } else {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Documents"))
    };
    let path = base
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("Vimbatim");
    let _ = std::fs::create_dir_all(&path);
    path
}

/// Reads `working_directory` from settings.conf — mirrors
/// `load_normal_text_size_half_points`'s tolerant flat key=value scan. `None` when
/// the file or key is missing, so callers can fall back to
/// `default_working_directory()` rather than trusting a nonexistent path.
fn load_working_directory(path: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_to_string(path).ok().and_then(|contents| {
        contents.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "working_directory").then(|| PathBuf::from(value.trim()))
        })
    })
}

/// Persists `working_directory` to settings.conf via `theme`'s generic
/// key=value upsert helper, so the file explorer reopens to the last folder
/// the user picked (`set_working_directory`) instead of always resetting to
/// `default_working_directory()`.
fn save_working_directory(path: &std::path::Path, dir: &std::path::Path) -> std::io::Result<()> {
    crate::theme::save_setting_line(path, "working_directory", &dir.display().to_string())
}

/// Reads `expanded_dirs` from settings.conf — a `|`-joined list of directory
/// paths the nav-pane tree had expanded at last save (`save_expanded_dirs`).
/// Empty when the file or key is missing.
fn load_expanded_dirs(path: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_to_string(path)
        .ok()
        .map(|contents| {
            contents
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once('=')?;
                    (key.trim() == "expanded_dirs").then(|| {
                        value.split('|').filter(|s| !s.is_empty()).map(PathBuf::from).collect()
                    })
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Persists every currently expanded nav-pane directory to settings.conf as
/// a single `|`-joined `expanded_dirs` line, so `AppState::new()` can
/// restore the same folders expanded on the next launch
/// (`file_explorer::restore_expanded_dirs`).
pub(crate) fn save_expanded_dirs(path: &std::path::Path, dirs: &[PathBuf]) -> std::io::Result<()> {
    let joined = dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("|");
    crate::theme::save_setting_line(path, "expanded_dirs", &joined)
}

/// Guarantees `path` ends in `.docx` — every Save As destination goes through
/// this, both the name suggested to the native picker and whatever the user
/// actually types back.
///
/// Appends rather than using `Path::set_extension`, which would rewrite
/// anything after the last dot: `set_extension` turns "neg.v2" into "neg.docx",
/// silently eating part of the name. An existing `.docx` (in any case — Windows
/// pickers hand back `.DOCX`) is left exactly as the user wrote it.
/// Reading rate used when settings.conf has no `spreading_wpm`. Conversational
/// speech is ~150 wpm and prose reading ~250; competitive debate "spreading"
/// sits far above both, and 300 is a common mid-range figure to start from.
pub const DEFAULT_SPREADING_WPM: u32 = 300;

/// Clamps the shrink size (points) to something a document can actually use.
/// Wide enough for any real "small text" convention, narrow enough that the
/// stepper can't walk it somewhere unreadable.
pub fn clamp_shrink_size_points(points: u16) -> u16 {
    points.clamp(4, 48)
}

/// Clamps a words-per-minute value to something that can't produce a nonsense
/// estimate. Zero would divide by zero; the upper bound is well past any human
/// rate and only exists to keep the settings stepper from running away.
pub fn clamp_spreading_wpm(wpm: u32) -> u32 {
    wpm.clamp(50, 1000)
}

fn load_spreading_wpm(path: &std::path::Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (k, value) = line.split_once('=')?;
                (k.trim() == "spreading_wpm").then(|| value.trim().parse::<u32>().ok()).flatten()
            })
        })
        .map(clamp_spreading_wpm)
        .unwrap_or(DEFAULT_SPREADING_WPM)
}

/// What the word-count panel shows. `spoken` is the figure the time estimate
/// divides — in a debate doc the parts actually read aloud are the tag lines
/// plus the highlighted body text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentStats {
    pub total_words: usize,
    pub tag_words: usize,
    pub highlighted_words: usize,
    pub spoken_words: usize,
}

impl DocumentStats {
    /// `spoken_words` at `wpm`, as `(minutes, seconds)`.
    pub fn estimated_time(&self, wpm: u32) -> (u64, u64) {
        let wpm = clamp_spreading_wpm(wpm) as f64;
        let seconds = (self.spoken_words as f64 / wpm * 60.0).round() as u64;
        (seconds / 60, seconds % 60)
    }
}

/// Whitespace-delimited word count — the same rule Word's own counter uses,
/// and the only one that matches what a user eyeballing a page expects.
fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// First byte offset at or after `from` where `needle` occurs, matched
/// ASCII-case-insensitively. `None` if there is no match.
///
/// ASCII-only case folding on purpose. Full Unicode folding via
/// `to_lowercase()` is not length-preserving (`İ` lowercases to two chars),
/// which would break the byte offsets every caller here feeds straight into
/// `content[..]` slicing and selection ranges. Comparing bytes with
/// `eq_ignore_ascii_case` keeps offsets exact and covers the English prose
/// this editor is for; non-ASCII letters simply match case-sensitively.
pub(crate) fn find_from(content: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > content.len() {
        return None;
    }
    let last = content.len() - needle.len();
    (from.min(content.len())..=last)
        .filter(|i| content.is_char_boundary(*i) && content.is_char_boundary(i + needle.len()))
        .find(|i| content[*i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Last occurrence of `needle` that *starts* strictly before `before`. The
/// backward counterpart of `find_from`, with the same ASCII-folding caveat.
pub(crate) fn rfind_before(content: &str, needle: &str, before: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > content.len() || before == 0 {
        return None;
    }
    // `before - 1`, not `saturating_sub(1)`: at `before == 0` no index can be
    // strictly before it, and saturating would have collapsed that to the same
    // bound as `before == 1` — making a backward search from the very start of
    // the document match position 0 instead of wrapping to the end.
    let last = (content.len() - needle.len()).min(before - 1);
    (0..=last)
        .rev()
        .filter(|i| content.is_char_boundary(*i) && content.is_char_boundary(i + needle.len()))
        .find(|i| content[*i..i + needle.len()].eq_ignore_ascii_case(needle))
}

pub fn with_docx_extension(path: &Path) -> PathBuf {
    let already_docx = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"));
    if already_docx {
        return path.to_path_buf();
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".docx");
    path.with_file_name(name)
}

/// Which dropdown a custom color belongs to. The two lists are deliberately
/// separate: a highlight color added while highlighting shouldn't turn up as a
/// font color option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomColorTarget {
    Font,
    Highlight,
}

impl CustomColorTarget {
    /// The settings.conf `[FORMATTING]` key this list is stored under.
    pub fn settings_key(self) -> &'static str {
        match self {
            CustomColorTarget::Font => "custom_font_colors",
            CustomColorTarget::Highlight => "custom_highlight_colors",
        }
    }
}

/// ponytail: hard cap on saved custom colors, oldest dropped first — keeps
/// settings.conf and the dropdown from growing without bound. Raise it (or add
/// a "manage colors" UI) if users ask for more slots.
pub const MAX_CUSTOM_COLORS: usize = 16;

/// Reads one pipe-separated list of `RRGGBB` hex colors from settings.conf.
/// Same `|` convention as `expanded_dirs`, and the same tolerant flat scan as
/// `load_font_size_half_points`: a missing file, missing key, or unparseable
/// entry costs that entry only, never an error.
pub(crate) fn load_custom_colors(path: &std::path::Path, key: &str) -> Vec<u32> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (k, value) = line.split_once('=')?;
                (k.trim() == key).then(|| value.trim().to_string())
            })
        })
        .map(|value| {
            value
                .split('|')
                .map(str::trim)
                .filter(|entry| entry.len() == 6)
                .filter_map(|entry| u32::from_str_radix(entry, 16).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn save_custom_colors(
    path: &std::path::Path,
    key: &str,
    colors: &[u32],
) -> std::io::Result<()> {
    let joined = colors.iter().map(|c| format!("{c:06x}")).collect::<Vec<_>>().join("|");
    crate::theme::save_setting_line(path, key, &joined)
}

/// Reads `normal_text_size` (points, `[FORMATTING]` section) from settings.conf —
/// the size "Clear Formatting" (`found_bugs.md`) resets a line back to.
/// Mirrors `keybinds::load_vim_enabled`'s tolerant flat key=value scan
/// rather than pulling in the unused `config_parsing` crate (which panics
/// on a missing file). Converted to half-points, `Run.size`'s own unit.
/// Falls back to 22 (11pt) when the file or key is missing/unparseable,
/// matching settings.conf's own shipped default.
fn load_normal_text_size_half_points(path: &std::path::Path) -> u16 {
    load_font_size_half_points(path, "normal_text_size", 22)
}

/// Reads a `[FORMATTING]` font-size setting (points) from settings.conf,
/// converting to half-points (`Run.size`'s unit) — the shared scan behind
/// `load_normal_text_size_half_points` and the `pocket_size`/`block_size`/
/// `tag_size`/`cite_size` loads in `AppState::new()`. `default_half_points`
/// is returned as-is (already in half-points) when the file or key is
/// missing/unparseable.
fn load_font_size_half_points(path: &std::path::Path, key: &str, default_half_points: u16) -> u16 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (k, value) = line.split_once('=')?;
                (k.trim() == key).then(|| value.trim().parse::<u16>().ok()).flatten()
            })
        })
        .map(|points| points * 2)
        .unwrap_or(default_half_points)
}

/// Reads a `true`/`false` settings.conf key, mirroring
/// `keybinds::load_vim_enabled`'s tolerant flat key=value scan. `default` is
/// returned when the file or key is missing.
///
/// Note the scan is *section-agnostic* — every loader in this file is, and
/// the `[...]` headers are cosmetic to it. That's why the spellcheck keys are
/// prefixed (`spellcheck`, `spellcheck_underline_color`) rather than relying
/// on `[SPELLCHECK]` to disambiguate a bare `enabled=`.
fn load_bool_setting(path: &std::path::Path, key: &str, default: bool) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (k, value) = line.split_once('=')?;
                (k.trim() == key).then(|| value.trim() == "true")
            })
        })
        .unwrap_or(default)
}

/// Reads a free-form string settings.conf key. Same flat scan as above; an
/// empty value counts as missing so a blank line falls back to the default.
fn load_string_setting(path: &std::path::Path, key: &str, default: &str) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let (k, value) = line.split_once('=')?;
                let value = value.trim();
                (k.trim() == key && !value.is_empty()).then(|| value.to_string())
            })
        })
        .unwrap_or_else(|| default.to_string())
}

/// Where the user's added-word list lives: alongside settings.conf, one
/// lowercased word per line.
///
/// A separate file rather than a settings.conf key because it grows without
/// bound as the user clicks "Add to Dictionary", and `theme::save_setting_line`
/// (settings.conf's writer) works a whole line at a time — a thousand-word
/// value on one line is not a config file anyone can edit by hand.
pub fn user_dictionary_path() -> PathBuf {
    settings_conf_path().with_file_name("user_dictionary.txt")
}

/// Settings -> Themes -> Import Theme's destination: the picked file is
/// copied here (next to settings.conf, same "beside `current_exe()`, not
/// the original upload path" reasoning as `user_dictionary_path()`) so a
/// custom theme survives even if the original file the user picked is later
/// moved or deleted.
pub fn custom_theme_path() -> PathBuf {
    settings_conf_path().with_file_name("custom_theme.toml")
}

/// Loads the user dictionary, lowercasing as it goes so lookups can be
/// case-insensitive without normalizing at every call site. A missing file is
/// the normal first-launch state, not an error.
fn load_user_dictionary(path: &std::path::Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|contents| {
            contents
                .lines()
                .map(|l| l.trim().to_lowercase())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
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
        let working_directory =
            load_working_directory(settings_path).unwrap_or_else(default_working_directory);

        let mut file_tree = scan_directory(&working_directory);
        crate::file_explorer::restore_expanded_dirs(
            &mut file_tree,
            &load_expanded_dirs(settings_path),
        );
        let keybinds = crate::keybinds::Keybinds::load(settings_path);
        let vim_keybinds = crate::vim_keybinds::VimKeybinds::load(settings_path);
        let vim_enabled = crate::keybinds::load_vim_enabled(settings_path);
        let theme = crate::theme::load_theme(settings_path);
        let theme_mode = crate::theme::load_theme_mode(settings_path);
        let theme_color_mode = crate::theme::load_theme_color_mode(settings_path);
        // Present only if the user has actually imported one — a missing
        // file is the normal "never imported" state, not an error. Like
        // `user_dictionary_path()`, always the real global path regardless
        // of the `settings_path` this constructor was given.
        let custom_theme = std::fs::read_to_string(custom_theme_path())
            .ok()
            .and_then(|s| crate::theme::parse_custom_theme_toml(&s));
        let normal_text_size_half_points = load_normal_text_size_half_points(settings_path);
        let pocket_size_half_points = load_font_size_half_points(settings_path, "pocket_size", 52);
        let block_size_half_points = load_font_size_half_points(settings_path, "block_size", 32);
        let tag_size_half_points = load_font_size_half_points(settings_path, "tag_size", 26);
        let cite_size_half_points = load_font_size_half_points(settings_path, "cite_size", 26);
        let small_size_half_points =
            clamp_shrink_size_points(load_font_size_half_points(settings_path, "small_size", 12) / 2) * 2;
        let custom_font_colors =
            load_custom_colors(settings_path, CustomColorTarget::Font.settings_key());
        let custom_highlight_colors =
            load_custom_colors(settings_path, CustomColorTarget::Highlight.settings_key());

        AppState {
            tabs: vec![Tab::new_empty(0)],
            active_tab: 0,
            pending_focus_editor: None,
            next_tab_id: 1,
            closed_tabs: Vec::new(),
            sidebar_visible: true,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            file_context_menu: None,
            editor_context_menu: None,
            find_bar: None,
            word_count_visible: false,
            timer: crate::timer::TimerState::default(),
            spreading_wpm: load_spreading_wpm(settings_path),
            custom_font_colors,
            custom_highlight_colors,
            sidebar_mode: SidebarMode::default(),
            settings_visible: false,
            pending_close: None,
            pending_recovery: crate::recovery::scan_recovery_dir(&crate::recovery::recovery_dir()),
            working_directory,
            file_tree,
            vim_enabled,
            keybinds,
            vim_keybinds,
            pending_vim_action: None,
            theme,
            theme_mode,
            theme_color_mode,
            custom_theme,
            normal_text_size_half_points,
            pocket_size_half_points,
            block_size_half_points,
            tag_size_half_points,
            cite_size_half_points,
            small_size_half_points,
            zoom: 1.0,
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
            highlight_color: load_string_setting(settings_path, "highlight_color", "yellow"),
            analytic_color: load_string_setting(settings_path, "analytic_color", "0000ff"),
            standardize_highlight_exception:
                load_string_setting(settings_path, "standardize_highlight_exception", ""),
            emphasis_bold: load_bool_setting(settings_path, "emphasis_bold", true),
            emphasis_underline: load_bool_setting(settings_path, "emphasis_underline", false),
            emphasis_box: load_bool_setting(settings_path, "emphasis_box", false),
            paste_condense: load_bool_setting(settings_path, "paste_condense", false),
            paste_condense_pilcrow: load_bool_setting(settings_path, "paste_condense_pilcrow", false),
            settings_path: settings_path.to_path_buf(),
            spellcheck_enabled: load_bool_setting(settings_path, "spellcheck", true),
            spellcheck_underline_color: load_string_setting(
                settings_path,
                "spellcheck_underline_color",
                "red",
            ),
            user_dictionary: Rc::new(load_user_dictionary(&user_dictionary_path())),
            invisibility_mode: false,
            print_layout: false,
            split_view: false,
            secondary_tab_id: None,
            focused_pane: Pane::Primary,
            primary_tab_id: None,
            split_ratio: 0.5,
            split_dragging: false,
            read_mode: false,
            sidebar_before_read_mode: true,
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
            log_line(&format!("[spellcheck] couldn't write {}: {e}", path.display()));
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return DocumentStats::default() };

        let mut stats = DocumentStats {
            total_words: count_words(&tab.content),
            ..DocumentStats::default()
        };
        for para in &tab.paragraphs {
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
        let tab = self.tabs.get(self.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return None;
        }
        Some(
            runs_in_range(&tab.paragraphs, start, end)
                .iter()
                .filter(|r| {
                    r.highlight
                        || matches!(r.style, Some(CardStyle::Tag) | Some(CardStyle::Cite))
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
            .tabs
            .get(self.active_tab)
            .map(|t| t.paragraphs.iter().any(&is_tag))
            .unwrap_or(false);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        let default_size = self.normal_text_size_half_points;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for para in &mut tab.paragraphs {
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
            tab.is_modified = true;
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
    pub fn open_find_bar(&mut self) {
        let selected = self.copy_selection().filter(|s| !s.contains('\n'));
        let bar = self.find_bar.get_or_insert_with(FindBar::default);
        if let Some(text) = selected {
            bar.query = text;
        }
        bar.focus = FindField::Query;
        self.refresh_find_matches();
    }

    pub fn close_find_bar(&mut self) {
        self.find_bar = None;
        self.pending_focus_editor = Some(self.focused_pane);
    }

    /// Recomputes the "N of M" readout. Cheap enough to run on every
    /// keystroke: it is one pass over the document with a byte comparison per
    /// candidate position.
    pub fn refresh_find_matches(&mut self) {
        let Some(bar) = self.find_bar.as_ref() else { return };
        let query = bar.query.clone();
        let (count, current) = match self.tabs.get(self.active_tab) {
            Some(tab) if !query.is_empty() => {
                let cursor = tab.cursor;
                let mut count = 0;
                let mut current = 0;
                let mut at = 0;
                while let Some(pos) = find_from(&tab.content, &query, at) {
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
        if let Some(bar) = self.find_bar.as_mut() {
            bar.match_count = count;
            bar.current_match = current;
        }
    }

    /// Jumps to and selects the next (or previous) match, wrapping around the
    /// document like the vim `/` search this shares its wraparound semantics
    /// with. Returns false when there's nothing to find.
    pub fn find_next(&mut self, forward: bool) -> bool {
        let Some(query) = self.find_bar.as_ref().map(|b| b.query.clone()) else { return false };
        if query.is_empty() { return false; }
        let Some(tab) = self.tabs.get(self.active_tab) else { return false };

        // Search from the current selection's far edge so repeated Next walks
        // forward instead of re-finding the match already highlighted.
        let from = match tab.selection {
            Some((a, f)) if forward => a.max(f),
            Some((a, f)) => a.min(f),
            None => tab.cursor,
        };
        let found = if forward {
            find_from(&tab.content, &query, from).or_else(|| find_from(&tab.content, &query, 0))
        } else {
            rfind_before(&tab.content, &query, from)
                .or_else(|| rfind_before(&tab.content, &query, tab.content.len()))
        };

        let Some(pos) = found else { return false };
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.selection = Some((pos, pos + query.len()));
            tab.cursor = pos + query.len();
            tab.pending_scroll_to_cursor = true;
        }
        self.refresh_find_matches();
        true
    }

    /// Replaces the currently-selected match, then advances to the next one.
    /// A no-op unless the selection actually *is* a match — otherwise Replace
    /// pressed straight after opening the bar would overwrite arbitrary text.
    pub fn replace_current(&mut self) {
        let Some(bar) = self.find_bar.as_ref() else { return };
        let (query, replacement) = (bar.query.clone(), bar.replacement.clone());
        if query.is_empty() { return; }

        let selection_is_match = self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.selection.map(|(a, f)| (t, a.min(f), a.max(f))))
            .is_some_and(|(tab, start, end)| {
                end <= tab.content.len()
                    && end - start == query.len()
                    && tab.content[start..end].eq_ignore_ascii_case(&query)
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
        let Some(bar) = self.find_bar.as_ref() else { return 0 };
        let (query, replacement) = (bar.query.clone(), bar.replacement.clone());
        if query.is_empty() { return 0; }

        let mut replaced = 0;
        let mut at = 0;
        loop {
            let Some(tab) = self.tabs.get(self.active_tab) else { break };
            let Some(pos) = find_from(&tab.content, &query, at) else { break };
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
    pub fn current_palette(&self) -> crate::theme::Palette {
        if self.theme == crate::theme::ThemeKind::Custom {
            if let Some((dark, light)) = self.custom_theme {
                return match self.theme_mode {
                    crate::theme::ThemeMode::Dark => dark,
                    crate::theme::ThemeMode::Light => light,
                };
            }
        }
        crate::theme::palette(self.theme, self.theme_mode)
    }

    /// Settings -> Themes -> Import Theme: parses `content` (the picked
    /// file's own text) as a custom theme, and if it's valid, adopts it —
    /// replacing any previously imported one — copies it to
    /// `custom_theme_path()` so it survives a restart even if the original
    /// file moves, switches `theme` to `Custom`, and persists that choice.
    /// Returns whether the import succeeded; a caller can show an error on
    /// `false` without touching any existing custom theme still in effect.
    pub fn import_custom_theme(&mut self, content: &str) -> bool {
        let Some(parsed) = crate::theme::parse_custom_theme_toml(content) else { return false };
        self.custom_theme = Some(parsed);
        self.theme = crate::theme::ThemeKind::Custom;
        let _ = std::fs::write(custom_theme_path(), content);
        let _ = crate::theme::save_theme(&settings_conf_path(), self.theme);
        true
    }

    pub fn new_tab(&mut self) {
        /*
         * Appends a blank tab and makes it the active tab. Used when the user
         * clicks the "+" button in the tab bar or presses the new-tab keybind.
         */
        self.push_empty_tab();
        self.show_in_focused_pane(self.tabs.len() - 1);
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
    fn show_in_focused_pane(&mut self, idx: usize) {
        self.active_tab = idx;
        let id = self.tabs.get(idx).map(|t| t.id);
        match self.focused_pane {
            Pane::Primary => self.primary_tab_id = id,
            Pane::Secondary => self.secondary_tab_id = id,
        }
        self.pending_focus_editor = Some(self.focused_pane);
    }

    /// Appends a blank tab and returns its stable id, without touching focus
    /// or `active_tab`. Shared by `new_tab` and `open_split`, which then do
    /// their own focusing — the two differ only in which pane ends up on it.
    fn push_empty_tab(&mut self) -> usize {
        let id = self.next_tab_id;
        self.tabs.push(Tab::new_empty(id));
        self.next_tab_id += 1;
        id
    }

    // ── Split view (notes/split_view_plan.md) ───────────────────────────────

    /// Enters or leaves reading mode.
    ///
    /// Entering collapses the split (the tab stays open — only the pane goes
    /// away, same as `close_split`) and hides the sidebar, so the document has
    /// the whole window. Leaving restores the sidebar to whatever it was.
    pub fn toggle_read_mode(&mut self) {
        self.read_mode = !self.read_mode;
        if self.read_mode {
            self.sidebar_before_read_mode = self.sidebar_visible;
            self.close_split();
            self.sidebar_visible = false;
        } else {
            self.sidebar_visible = self.sidebar_before_read_mode;
        }
    }

    /// Resolves a pane to a live index into `tabs`.
    ///
    /// The single place the secondary pane's stored `Tab.id` is turned into an
    /// index. `None` for `Secondary` when the split is closed, or when its tab
    /// has since been closed.
    pub fn pane_tab_index(&self, pane: Pane) -> Option<usize> {
        match pane {
            // While the secondary pane holds focus, `active_tab` names *its*
            // document, so the primary pane resolves through its remembered id
            // instead — otherwise both panes paint the same text.
            Pane::Primary => {
                if self.focused_pane == Pane::Primary || self.primary_tab_id.is_none() {
                    (self.active_tab < self.tabs.len()).then_some(self.active_tab)
                } else {
                    let id = self.primary_tab_id?;
                    self.tabs.iter().position(|t| t.id == id)
                }
            }
            Pane::Secondary => {
                if !self.split_view {
                    return None;
                }
                let id = self.secondary_tab_id?;
                self.tabs.iter().position(|t| t.id == id)
            }
        }
    }

    /// Moves editing focus to `pane`, pointing `active_tab` at that pane's tab.
    ///
    /// This is the whole mechanism that keeps split view from touching the 200+
    /// `self.active_tab` reads elsewhere in this file: "the active tab" and
    /// "the focused pane's tab" are the same thing, so every existing method
    /// acts on the right document without knowing panes exist.
    pub fn focus_pane(&mut self, pane: Pane) {
        match pane {
            Pane::Secondary => {
                let Some(idx) = self.pane_tab_index(Pane::Secondary) else { return };
                // Remember what the primary pane was on before `active_tab`
                // moves off it, so focusing back lands on the same document.
                if self.focused_pane == Pane::Primary {
                    self.primary_tab_id = self.tabs.get(self.active_tab).map(|t| t.id);
                }
                self.active_tab = idx;
            }
            Pane::Primary => {
                if let Some(idx) = self.pane_tab_index(Pane::Primary) {
                    self.active_tab = idx;
                }
            }
        }
        self.focused_pane = pane;
        self.pending_focus_editor = Some(pane);
    }

    /// Opens the split with a fresh blank tab in the new pane.
    ///
    /// Idempotent: with the split already open this only focuses the secondary
    /// pane, rather than stacking up blank tabs on repeated clicks.
    pub fn open_split(&mut self) {
        if self.split_view {
            self.focus_pane(Pane::Secondary);
            return;
        }
        let id = self.push_empty_tab();
        self.secondary_tab_id = Some(id);
        self.split_view = true;
        self.focus_pane(Pane::Secondary);
    }

    /// Closes the split. The secondary tab stays *open* — only the pane goes
    /// away, so nothing the user typed into it is lost or hidden from the tab
    /// bar.
    pub fn close_split(&mut self) {
        if !self.split_view {
            return;
        }
        self.split_view = false;
        self.secondary_tab_id = None;
        self.focused_pane = Pane::Primary;
        self.pending_focus_editor = Some(Pane::Primary);
    }

    pub fn open_file(&mut self, path: PathBuf) {
        /*
         * Opens a file in a new tab, parsing its docx content immediately.
         * If the file is already open, switches to the existing tab instead.
         *
         * When `parse_docx` fails (e.g., the file is corrupt or a 0-byte placeholder),
         * the tab still opens with empty content and `docx_origin = None`
         * (`paragraphs` stays at its default single empty paragraph/run).
         *
         * Anything that isn't a .docx is refused outright. The guard lives here
         * rather than at the toolbar's file picker because that isn't the only
         * way an arbitrary path gets in: vim's `:e <path>` reaches this same
         * method, and GPUI's `PathPromptOptions` has no extension filter to set
         * on the native dialog. The sidebar's own tree is already .docx-only
         * (`scan_directory`), so this changes nothing for it.
         */
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("docx")) {
            // stderr, matching how every other non-fatal file error in this
            // file reports itself — there's no in-app notification surface.
            log_line(&format!("[open] not a .docx, refusing to open: {}", path.display()));
            return;
        }
        if let Some(idx) = self.tabs.iter().position(|t| t.file_path.as_deref() == Some(&path)) {
            // Already open in the *other* pane: focus that pane rather than
            // pulling the same document into this one (split-view decision 1).
            if self.split_view && self.pane_tab_index(Pane::Secondary) == Some(idx) {
                self.focus_pane(Pane::Secondary);
            } else if self.split_view && self.pane_tab_index(Pane::Primary) == Some(idx) {
                self.focus_pane(Pane::Primary);
            } else {
                self.show_in_focused_pane(idx);
            }
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

        // An untouched "New Tab" is a placeholder, not work — opening a file
        // takes its slot instead of leaving a blank tab stranded beside the
        // document. Replacing *in place* keeps every other tab's index stable,
        // so the other pane's tab and any in-flight indices stay valid; the
        // replacement carries a fresh id, and `show_in_focused_pane` re-reads
        // it so this pane points at the new document rather than the discarded
        // placeholder.
        let reuse = self
            .pane_tab_index(self.focused_pane)
            .filter(|&i| self.tabs.get(i).is_some_and(|t| t.is_blank_new_tab()));
        match reuse {
            Some(idx) => {
                self.tabs[idx] = tab;
                self.show_in_focused_pane(idx);
            }
            None => {
                self.tabs.push(tab);
                self.show_in_focused_pane(self.tabs.len() - 1);
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
        self.save_tab(self.active_tab)
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
        // Same funnel-level forcing the recovery Save As already does: a
        // picker (or a user typing a name) can hand back a bare or
        // wrong-extension path, and saving a docx there produces a file
        // `open_file` will refuse to reopen.
        let path = with_docx_extension(&path);

        let idx = self.active_tab;
        let tab = self.tabs.get_mut(idx).ok_or("No active tab")?;
        tab.file_path = Some(path.clone());
        tab.title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        tab.is_modified = true;
        self.save_tab(idx)
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
        let tab_id = if let Some(tab) = self.tabs.get_mut(idx) {
            tab.is_modified = false;
            tab.last_snapshot_version = tab.content_version;
            Some(tab.id)
        } else {
            None
        };
        // Persisted for real — the recovery snapshot is now redundant, and
        // leaving it would prompt on next launch about work already saved.
        if let Some(id) = tab_id {
            crate::recovery::delete_snapshot(id);
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
        // A deliberately closed tab has no unsaved work worth recovering.
        let closed_id = self.tabs.get(idx).map(|t| t.id);
        if let Some(id) = closed_id {
            crate::recovery::delete_snapshot(id);
        }
        // Only a file-backed tab can be reopened — a blank "New Tab" has
        // nothing on disk for `reopen_closed_tab` to load back.
        if let Some(path) = self.tabs.get(idx).and_then(|t| t.file_path.clone()) {
            self.closed_tabs.push(path);
        }
        self.tabs.remove(idx);

        // Two panes need two tabs. Collapse the split when the closed tab was
        // the secondary pane's own, or when only one tab is left for both to
        // share — either way the pane has nothing legal left to show.
        if self.split_view
            && (closed_id == self.secondary_tab_id || self.tabs.len() < 2)
        {
            self.close_split();
        }
        // If a tab to the left of the active one was removed, shift active_tab left.
        if idx < self.active_tab {
            self.active_tab -= 1;
        }
        // clamp active tab to valid range
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        // Closing a tab (via its close button, not a click into the editor)
        // can leave a different tab active — same focus-loss bug as
        // `set_active_tab`/`open_file`, so request the same reclaim.
        // Harmless when the active tab didn't actually change: GPUI's
        // `focus()` is a no-op if the handle is already focused.
        self.pending_focus_editor = Some(self.focused_pane);
    }

    /// Settings → Keybinds "open a closed tab" (`Ctrl+Shift+W` by default):
    /// reopens the most recently closed file-backed tab, popping it off
    /// `closed_tabs`. Repeating the keybind walks back through however many
    /// tabs were closed, most-recent first. A no-op with nothing on the
    /// stack.
    pub fn reopen_closed_tab(&mut self) {
        if let Some(path) = self.closed_tabs.pop() {
            self.open_file(path);
        }
    }

    /// Entry point for the tab-bar's `×` button. Closes the tab immediately
    /// when it has no unsaved changes (unchanged behavior); otherwise arms
    /// `pending_close` so `close_confirm.rs` can ask Save/Discard/Cancel
    /// instead of silently dropping edits.
    pub fn request_close_tab(&mut self, idx: usize) {
        if self.tabs.get(idx).map(|t| t.is_modified).unwrap_or(false) {
            self.pending_close = Some(PendingClose::Tab(idx));
        } else {
            self.close_tab(idx);
        }
    }

    /// Entry point for the app-close `×`. When any tab has unsaved changes,
    /// arms `pending_close` so the confirm dialog can show. Otherwise
    /// resolves it straight back to `None` via `confirm_close_discard` —
    /// the GPUI caller reads "pending_close is None right after this call
    /// returns" as its own signal to call `cx.quit()` immediately, since
    /// this GPUI-free layer has no way to quit the app itself.
    pub fn request_close_app(&mut self) {
        self.pending_close = Some(PendingClose::App);
        if !self.tabs.iter().any(|t| t.is_modified) {
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
        match self.pending_close.take() {
            Some(PendingClose::Tab(idx)) => {
                let _ = self.save_tab(idx);
                let persisted = self.tabs.get(idx).map(|t| !t.is_modified).unwrap_or(true);
                if persisted {
                    self.close_tab(idx);
                }
                persisted
            }
            Some(PendingClose::App) => {
                for idx in 0..self.tabs.len() {
                    let _ = self.save_tab(idx);
                }
                self.tabs.iter().all(|t| !t.is_modified)
            }
            None => true,
        }
    }

    /// Resolves the pending close by discarding unsaved changes: closes the
    /// target tab without saving (or, for an app-close, just clears
    /// `pending_close` — no tabs to remove, the caller quits).
    pub fn confirm_close_discard(&mut self) {
        match self.pending_close.take() {
            Some(PendingClose::Tab(idx)) => self.close_tab(idx),
            Some(PendingClose::App) => {
                // Quitting with changes deliberately discarded: nothing here
                // is worth recovering, so clear every snapshot rather than
                // prompting about it on next launch.
                for tab in &self.tabs {
                    crate::recovery::delete_snapshot(tab.id);
                }
            }
            None => {}
        }
    }

    /// Backs out of a pending close (Cancel button, or the confirm dialog's
    /// backdrop click) — leaves everything untouched.
    pub fn cancel_close(&mut self) {
        self.pending_close = None;
    }

    /// The dirty tabs, flattened for the panic hook.
    pub fn dirty_tab_snapshots(&self) -> Vec<TabSnapshot> {
        self.tabs
            .iter()
            .filter(|t| t.is_modified)
            .map(|t| TabSnapshot {
                id: t.id,
                paragraphs: t.paragraphs.clone(),
                origin: t.docx_origin.clone(),
                file_path: t.file_path.clone(),
                title: t.title.clone(),
            })
            .collect()
    }

    /// Recovery option 1: throw the recovered changes away and delete the
    /// temporary file.
    pub fn discard_recovery(&mut self) {
        if self.pending_recovery.is_empty() {
            return;
        }
        let entry = self.pending_recovery.remove(0);
        crate::recovery::delete_entry(&entry);
    }

    /// Recovery option 2, part 1: hands the entry to the view so it can run
    /// the native save-file picker (which this GPUI-free layer cannot
    /// await), without popping it yet — a cancelled picker must leave the
    /// entry in place. The view calls `complete_recovery_save_as` once it
    /// has a destination.
    pub fn take_recovery_for_save_as(&mut self) -> Option<RecoveryEntry> {
        self.pending_recovery.first().cloned()
    }

    /// Recovery option 2, part 2: copies the snapshot to the user's chosen
    /// path, then pops and deletes it.
    ///
    /// A plain file copy, not a re-save: the snapshot is already a valid
    /// .docx carrying the original template, so copying is both cheaper and
    /// lossless compared with parse-then-write.
    pub fn complete_recovery_save_as(&mut self, entry: &RecoveryEntry, dest: &Path) -> Result<(), String> {
        // Forced here, at the funnel, rather than trusting the picker: a
        // never-saved tab's title is literally "New Tab" (`Tab::new_empty`),
        // so accepting the suggested name verbatim would otherwise write an
        // extension-less file that nothing will reopen as a document.
        let dest = with_docx_extension(dest);
        std::fs::copy(&entry.snapshot, &dest).map_err(|e| format!("Save failed: {e}"))?;
        self.pending_recovery.retain(|e| e.snapshot != entry.snapshot);
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
    pub fn resume_recovery(&mut self) {
        if self.pending_recovery.is_empty() {
            return;
        }
        let entry = self.pending_recovery.remove(0);

        // A snapshot that won't parse has nothing to restore. Drop it rather
        // than opening an empty tab that looks like the recovery succeeded —
        // `scan_recovery_dir` deliberately doesn't parse every zip at launch,
        // so this is where a corrupt snapshot is caught.
        let Ok((paragraphs, origin)) = parse_docx(&entry.snapshot) else {
            crate::recovery::delete_entry(&entry);
            return;
        };

        let mut tab = match &entry.original_path {
            Some(path) => Tab::from_path(self.next_tab_id, path.clone()),
            // Never-saved tab: reopen untitled, exactly as it was pre-crash.
            None => Tab::new_empty(self.next_tab_id),
        };
        tab.content = paragraphs_to_plain_text(&paragraphs);
        tab.paragraphs = paragraphs;
        tab.has_unsupported_blocks = origin.has_unsupported_blocks;
        tab.docx_origin = Some(Arc::new(origin));
        if entry.original_path.is_none() {
            tab.title = entry.title.clone();
        }
        // The whole point of Resume: changes are present but unsaved.
        tab.is_modified = true;
        // `delete_entry` below removes the only on-disk copy of this content,
        // so the tab must be eligible for a fresh snapshot without waiting
        // for the user to type. A freshly built Tab has content_version ==
        // last_snapshot_version == 0, which `needs_snapshot` reads as "this
        // version is already written"; bumping the version clears that. The
        // edit stamp then just starts the normal idle debounce from now, so
        // the rewrite lands one interval after the resume rather than on the
        // very next tick.
        tab.content_version = 1;
        tab.last_edit_at = Some(Instant::now());

        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.show_in_focused_pane(self.tabs.len() - 1);

        crate::recovery::delete_entry(&entry);
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
        if idx >= self.tabs.len() {
            return;
        }
        if self.split_view {
            let other = match self.focused_pane {
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
        if self.tabs.is_empty() { return; }
        self.set_active_tab((self.active_tab + 1) % self.tabs.len());
    }

    pub fn prev_tab(&mut self) {
        /*
         * Cycles to the previous tab (Ctrl+Shift+Tab), wrapping from the
         * first tab back to the last.
         */
        if self.tabs.is_empty() { return; }
        self.set_active_tab((self.active_tab + self.tabs.len() - 1) % self.tabs.len());
    }

    pub fn rename_tab(&mut self, id: usize, new_title: String) {
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
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.title = new_title;
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
         *
         * This is also this codebase's de facto "a real content mutation is
         * about to happen" choke point, so `content_version` bumps here
         * unconditionally, *before* the coalescing check below — a fast
         * typing burst must still bump it on every keystroke (uniform_list_plan.md
         * Part 1), even though only the first keystroke of the burst pushes
         * an actual undo entry.
         */
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        tab.content_version += 1;
        let now = Instant::now();
        let within_coalesce_window = tab.last_edit_at
            .map(|t| now.duration_since(t) < UNDO_COALESCE_WINDOW)
            .unwrap_or(false);
        tab.last_edit_at = Some(now);
        if within_coalesce_window {
            return;
        }
        tab.undo_stack.push((tab.content.clone(), tab.paragraphs.clone()));
        let cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(&tab.content, &tab.paragraphs));
        while tab.undo_stack.len() > cap {
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

    /// The Delete key: deletes the character immediately *after* the cursor —
    /// `backspace`'s forward counterpart. If a selection is active the whole
    /// selection is deleted instead, same as `backspace`. Doesn't touch
    /// `vim_insertion_recording`: that tracks characters just typed for `.`
    /// repeat, and this removes a character ahead of the cursor, never one of
    /// those.
    pub fn delete_forward(&mut self) {
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection(); // already pushes its own undo snapshot
            return;
        }
        let at_document_end = self
            .tabs
            .get(self.active_tab)
            .map(|t| t.cursor >= t.content.len())
            .unwrap_or(true);
        if at_document_end { return; }
        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let next = char_right(&tab.content, tab.cursor);
            sync_delete_range(&mut tab.paragraphs, tab.cursor, next);
            tab.content.remove(tab.cursor);
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
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection();
            return;
        }
        let Some((start, cursor)) = self.tabs.get(self.active_tab).map(|t| (word_backward(&t.content, t.cursor), t.cursor)) else { return };
        if start == cursor { return; }
        self.push_undo_snapshot();
        let mut deleted_chars = 0;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            deleted_chars = tab.content[start..cursor].chars().count();
            sync_delete_range(&mut tab.paragraphs, start, cursor);
            tab.content.replace_range(start..cursor, "");
            tab.cursor = start;
            tab.selection = None;
            tab.is_modified = true;
        }
        // Mirrors `backspace`'s per-char `rec.pop()`, scaled to a whole
        // word: truncate the recording by the same number of chars actually
        // removed, so vim's `.`-repeat (`VimChange::Insertion`) doesn't
        // replay text this deletion already erased. Capped at the
        // recording's own length — the deleted range can reach back past
        // where the current insertion segment started, into text that was
        // never part of this recording (same as `backspace`'s `pop()`
        // being a no-op once `rec` runs out).
        if let Some(rec) = self.vim_insertion_recording.as_mut() {
            let rec_chars = rec.chars().count();
            let new_len = rec_chars.saturating_sub(deleted_chars);
            let byte_idx = rec.char_indices().nth(new_len).map(|(i, _)| i).unwrap_or(rec.len());
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
            reset_card_style_in_range(&mut tab.paragraphs, line_start, line_end);
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
            let (para_idx, _, _) = resolve_position(&tab.paragraphs, line_start);
            if let Some(para) = tab.paragraphs.get_mut(para_idx) {
                for run in para.runs.iter_mut() {
                    apply_format_op(run, &effective_op);
                }
            }
        }
        tab.is_modified = true;
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
        let ranges: Option<Vec<(usize, usize)>> = self.tabs.get(self.active_tab).and_then(|t| {
            if !t.similar_ranges.is_empty() {
                Some(t.similar_ranges.clone())
            } else {
                t.selection.map(|(a, f)| vec![(a.min(f), a.max(f))])
            }
        });
        match ranges {
            Some(ranges) => {
                self.push_undo_snapshot();
                if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                    // Toggle off only when *every* range is already in that
                    // state, so one already-bold match can't block bolding all
                    // the others. Identical to the old single-range rule when
                    // there is only one range.
                    let effective_op = if ranges
                        .iter()
                        .all(|&(s, e)| is_uniformly_active(&tab.paragraphs, s, e, &op))
                    {
                        toggled_off(&op)
                    } else {
                        op.clone()
                    };
                    for &(start, end) in &ranges {
                        apply_formatting(&mut tab.paragraphs, start, end, effective_op.clone());
                        // Mirrors apply_formatting_to_line's own ClearAll special
                        // case (see its comment above `reset_card_style_in_range`):
                        // apply_formatting above only ever mutates run-level
                        // fields, never a Pocket/Hat/Block paragraph's own
                        // `heading`/`alignment` — left alone, Clear Formatting
                        // through an active selection silently kept a card-styled
                        // paragraph boxed/centered while only stripping bold/size.
                        if let FormatOp::ClearAll { .. } = effective_op {
                            reset_card_style_in_range(&mut tab.paragraphs, start, end);
                            tab.pending_format = None;
                        }
                    }
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
                    self.push_undo_snapshot();
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
        let default_size = self.normal_text_size_half_points;
        let has_selection = self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false);
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
        let processed = if self.paste_condense {
            let replacement = if self.paste_condense_pilcrow { "¶" } else { " " };
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
        let exception = self.standardize_highlight_exception.clone();
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
        let color = self.highlight_color.clone();
        let repaints = |run: &Run| {
            run.highlight
                && run.highlight_color != color
                && except.is_none_or(|e| run.highlight_color != e)
        };

        let nothing_to_do = self
            .tabs
            .get(self.active_tab)
            .map(|t| !t.paragraphs.iter().flat_map(|para| &para.runs).any(repaints))
            .unwrap_or(true);
        // Pushing an undo entry for a no-op would make Ctrl+Z appear broken.
        if nothing_to_do {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for para in &mut tab.paragraphs {
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
            tab.is_modified = true;
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return;
        }

        let selected_text = tab.content[start..end].to_string();
        let condensed = selected_text.replace('\n', replacement);

        if condensed == selected_text {
            return;
        }

        // `runs_in_range` emits a dedicated unformatted `"\n"` run for every
        // paragraph boundary the selection crosses (same contract copy/paste
        // relies on) — replacing `\n` inside each run's own text turns those
        // into the replacement too, matching `condensed`, without touching any
        // real run's formatting.
        let condensed_runs: Vec<Run> = runs_in_range(&tab.paragraphs, start, end)
            .into_iter()
            .map(|mut r| { r.text = r.text.replace('\n', replacement); r })
            .collect();

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_delete_range(&mut tab.paragraphs, start, end);
            tab.content.drain(start..end);
            sync_insert_str_with_runs(&mut tab.paragraphs, start, &condensed, &condensed_runs);
            tab.content.insert_str(start, &condensed);
            tab.cursor = start;
            tab.selection = Some((start, start + condensed.len()));
            tab.is_modified = true;
        }
    }

    /// Card Menu → "Uncondense": undoes condensing by turning each marker it
    /// left behind back into a real newline — `¶` if the selection was
    /// condensed with pilcrows, `CONDENSE_MARKER` if it was condensed
    /// without them. Whichever produced the selected text, this reverses it;
    /// a no-op if neither marker is present. Same run-preserving mechanics as
    /// `condense_selection_with`, just inverted.
    pub fn uncondense_selection(&mut self) {
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let Some((a, f)) = tab.selection else { return };

        let (start, end) = (a.min(f), a.max(f));
        if start >= end {
            return;
        }

        let selected_text = tab.content[start..end].to_string();
        let uncondensed = uncondense_markers(&selected_text);

        if uncondensed == selected_text {
            return;
        }

        let uncondensed_runs: Vec<Run> = runs_in_range(&tab.paragraphs, start, end)
            .into_iter()
            .map(|mut r| { r.text = uncondense_markers(&r.text); r })
            .collect();

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_delete_range(&mut tab.paragraphs, start, end);
            tab.content.drain(start..end);
            sync_insert_str_with_runs(&mut tab.paragraphs, start, &uncondensed, &uncondensed_runs);
            tab.content.insert_str(start, &uncondensed);
            tab.cursor = start;
            tab.selection = Some((start, start + uncondensed.len()));
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
        let tab = self.tabs.get(self.active_tab)?;
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
        for para in &tab.paragraphs {
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

    pub fn shrink_text(&mut self) {
        /*
         * Sets the font size of every non-underlined run in the selection to
         * settings.conf's `small_size` (user-requested: underlined text is
         * left alone — e.g. a debate card's underlined emphasis shouldn't
         * shrink along with the rest of the tag/cite). Runs are only
         * touched when they fall fully inside the selection (no splitting
         * at partial overlaps), matching this method's pre-existing scan.
         */
        let small_size = self.small_size_half_points;
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
                            if run_start >= start && run_end <= end && !run.underline {
                                run.size = small_size;
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if tab.folded_para_count != tab.paragraphs.len() {
            tab.folded_headings.clear();
            tab.folded_para_count = tab.paragraphs.len();
            tab.fold_version = tab.fold_version.wrapping_add(1);
        }
    }

    /// Collapses or expands the heading paragraph at `idx`. A no-op on body
    /// text — there is nothing under it to fold.
    pub fn toggle_paragraph_fold(&mut self, idx: usize) {
        self.sync_fold_state();
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        if tab.paragraphs.get(idx).map(|p| p.heading).unwrap_or(0) == 0 {
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
        self.tabs
            .get(self.active_tab)
            .is_some_and(|t| t.folded_para_count == t.paragraphs.len() && !t.folded_headings.is_empty())
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        tab.folded_headings.clear();
        if !expand {
            for (i, para) in tab.paragraphs.iter().enumerate() {
                if para.heading != 0 {
                    tab.folded_headings.insert(i);
                }
            }
        }
        tab.folded_para_count = tab.paragraphs.len();
        tab.fold_version = tab.fold_version.wrapping_add(1);
    }



    /// Paragraph integrity: keep a paste's paragraph breaks intact.
    ///
    /// Turning it on switches condense-on-paste off — the two are opposites,
    /// and leaving both on meant the ribbon claimed to be preserving
    /// paragraphs while the paste collapsed them anyway.
    pub fn toggle_paragraph_integrity(&mut self) {
        self.paragraph_integrity = !self.paragraph_integrity;
        if self.paragraph_integrity {
            self.set_paste_condense(false);
        }
    }

    /// Pilcrows: mark collapsed newlines with `¶`.
    ///
    /// Drives the `paste_condense_pilcrow` setting so the ribbon toggle and
    /// the settings modal are the same switch rather than two that disagree.
    pub fn toggle_pilcrows(&mut self) {
        self.pilcrows = !self.pilcrows;
        self.set_paste_condense_pilcrow(self.pilcrows);
    }

    /// Setters for the text settings that persist to settings.conf, so a
    /// change made from the ribbon survives a restart exactly like one made in
    /// the settings modal.
    pub fn set_paste_condense(&mut self, on: bool) {
        self.paste_condense = on;
        self.save_setting("paste_condense", if on { "true" } else { "false" });
    }

    pub fn set_paste_condense_pilcrow(&mut self, on: bool) {
        self.paste_condense_pilcrow = on;
        self.save_setting("paste_condense_pilcrow", if on { "true" } else { "false" });
    }

    /// The size Shrink drops text to, in points. Stored as half-points
    /// (`Run.size`'s unit) but written to settings.conf as points, which is
    /// what `small_size` has always held and what the user reads.
    pub fn set_shrink_size_points(&mut self, points: u16) {
        let points = clamp_shrink_size_points(points);
        self.small_size_half_points = points * 2;
        self.save_setting("small_size", &points.to_string());
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
        self.highlight_color = name.to_string();
        self.save_setting("highlight_color", name);
    }

    pub fn set_analytic_color(&mut self, hex: &str) {
        self.analytic_color = hex.to_string();
        self.save_setting("analytic_color", hex);
    }

    /// The highlight color "Standardize highlighting with exception" spares.
    /// An empty string clears it.
    pub fn set_standardize_exception(&mut self, name: &str) {
        self.standardize_highlight_exception = name.to_string();
        self.save_setting("standardize_highlight_exception", name);
    }

    pub fn set_emphasis(&mut self, bold: bool, underline: bool, boxed: bool) {
        self.emphasis_bold = bold;
        self.emphasis_underline = underline;
        self.emphasis_box = boxed;
        self.save_setting("emphasis_bold", if bold { "true" } else { "false" });
        self.save_setting("emphasis_underline", if underline { "true" } else { "false" });
        self.save_setting("emphasis_box", if boxed { "true" } else { "false" });
    }

    /// Writes one key to this state's settings.conf. Best-effort, matching
    /// every other settings write in this file — an unwritable directory must
    /// not break the in-memory change.
    fn save_setting(&self, key: &str, value: &str) {
        if let Err(e) = crate::theme::save_setting_line(&self.settings_path, key, value) {
            log_line(&format!("[settings] couldn't save {key}: {e}"));
        }
    }

    pub fn toggle_invisibility_mode(&mut self) {
        /*
         * Toggles invisibility mode. When on, only highlighted text,
         * tags, and citations are shown.
         */
        self.invisibility_mode = !self.invisibility_mode;
    }

    pub fn toggle_print_layout(&mut self) {
        self.print_layout = !self.print_layout;
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

    pub fn apply_line_alignment(&mut self, alignment: Alignment) {
        /*
         * Sets the alignment of the line containing the cursor (ribbon's
         * Align Left/Align Center buttons) — line-scoped like
         * `apply_formatting_to_line`/`apply_card_style`, not selection-
         * spanning like `apply_center_alignment`. Not a toggle: `Alignment`
         * is a single-valued paragraph field, so setting one value
         * inherently supersedes whatever was there before.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let cursor = tab.cursor;
        let line_start = tab.content[..cursor].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
        let line_end = tab.content[cursor..].find('\n').map(|pos| cursor + pos).unwrap_or(tab.content.len());

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            apply_paragraph_alignment(&mut tab.paragraphs, line_start, line_end, alignment);
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
        // Pocket/Block/Tag read their configured size from settings.conf;
        // Hat isn't user-configurable (not requested), so it keeps
        // `CardStyleKind::font_size`'s fixed value.
        let size = match kind {
            CardStyleKind::Pocket => self.pocket_size_half_points,
            CardStyleKind::Hat => kind.font_size(),
            CardStyleKind::Block => self.block_size_half_points,
            CardStyleKind::Tag => self.tag_size_half_points,
        };

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
        let size = self.tag_size_half_points;
        let color = self.analytic_color.clone();
        self.apply_formatting_to_line(FormatOp::Bold(true));
        self.apply_formatting_to_line(FormatOp::FontSize(size));
        self.apply_formatting_to_line(FormatOp::Color(Some(color)));
        self.apply_formatting_to_line(FormatOp::Style(Some(CardStyle::Analytic)));

        if let Some(tab) = self.tabs.get(self.active_tab) {
            let line_idx = tab.content[..tab.cursor].matches('\n').count();
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                if let Some(para) = tab.paragraphs.get_mut(line_idx) {
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
        let size = self.tag_size_half_points;
        let color = self.analytic_color.clone();
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
                && substantive().all(|r| {
                    r.bold && r.size == size && r.color.as_deref() == Some(color.as_str())
                })
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
            .tabs
            .get(self.active_tab)
            .map(|t| t.paragraphs.iter().any(&is_analytic))
            .unwrap_or(false);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.paragraphs.retain(|para| !is_analytic(para));
            // Every rich-text-aware function assumes at least one paragraph and
            // one run always exist (`default_paragraphs`).
            if tab.paragraphs.is_empty() {
                tab.paragraphs = default_paragraphs();
            }
            tab.content = paragraphs_to_plain_text(&tab.paragraphs);
            // The cursor and any selection pointed into text that is gone.
            tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor.min(tab.content.len()));
            tab.selection = None;
            tab.is_modified = true;
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
            .tabs
            .get(self.active_tab)
            .map(|t| t.paragraphs.iter().any(&is_analytic))
            .unwrap_or(false);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        let tag_heading = CardStyleKind::Tag.heading_level();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            for para in &mut tab.paragraphs {
                if !is_analytic(para) {
                    continue;
                }
                for run in &mut para.runs {
                    run.color = None;
                }
                para.heading = tag_heading;
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.is_modified = true;
        }
    }

    pub fn apply_cite_style(&mut self) {
        self.apply_formatting_to_selection(FormatOp::Bold(true));
        let size = self.cite_size_half_points;
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
        let Some(previous) = tab.undo_stack.pop() else { return };
        tab.content_version += 1;
        let current_content = std::mem::replace(&mut tab.content, previous.0);
        let current_paragraphs = std::mem::replace(&mut tab.paragraphs, previous.1);
        tab.redo_stack.push((current_content, current_paragraphs));
        // Same size-aware cap as `push_undo_snapshot` — repeatedly undoing
        // a huge document without any new edit would otherwise let
        // `redo_stack` grow past what `undo_stack` was ever bounded to.
        let cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(&tab.content, &tab.paragraphs));
        while tab.redo_stack.len() > cap {
            tab.redo_stack.remove(0);
        }
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
        tab.content_version += 1;
        let current_content = std::mem::replace(&mut tab.content, next.0);
        let current_paragraphs = std::mem::replace(&mut tab.paragraphs, next.1);
        tab.undo_stack.push((current_content, current_paragraphs));
        let cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(&tab.content, &tab.paragraphs));
        while tab.undo_stack.len() > cap {
            tab.undo_stack.remove(0);
        }
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

    /// Sibling of `copy_selection` that also returns the selection's
    /// per-run formatting, for `rich_clipboard::encode_with_lengths` to ride
    /// alongside the plain text on copy/cut. `None` under the same
    /// conditions `copy_selection` returns `None`.
    pub fn copy_selection_runs(&self) -> Option<Vec<Run>> {
        let tab = self.tabs.get(self.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));
        Some(crate::document_ops::runs_in_range(&tab.paragraphs, start, end))
    }

    /// The `(heading, alignment)` of every paragraph the selection touches, in
    /// document order — the paragraph-level half of a copy.
    ///
    /// Separate from `copy_selection_runs` because runs cannot express a card
    /// style on their own: Pocket/Hat/Block/Tag are run-level bold/size/box
    /// *plus* these two paragraph fields (`apply_card_style`). Copying only the
    /// runs is what made a pasted card come back correctly sized but
    /// structurally plain.
    pub fn copy_selection_paragraph_attrs(&self) -> Option<Vec<crate::rich_clipboard::ParagraphAttrs>> {
        let tab = self.tabs.get(self.active_tab)?;
        let (a, f) = tab.selection?;
        let (start, end) = (a.min(f), a.max(f));

        // Walk paragraphs by their byte spans in `content`, +1 per separating
        // '\n', and keep every one the selection overlaps. A zero-width
        // selection is already ruled out by the callers, but an end-exclusive
        // touch (selection stopping exactly at a paragraph's first byte) must
        // not pull that paragraph in.
        let mut attrs = Vec::new();
        let mut para_start = 0usize;
        for para in &tab.paragraphs {
            let text_len: usize = para.runs.iter().map(|r| r.text.len()).sum();
            let para_end = para_start + text_len;
            if start <= para_end && end > para_start || (start == end && start == para_start) {
                attrs.push((para.heading, para.alignment));
            }
            para_start = para_end + 1; // the '\n' between paragraphs
        }
        Some(attrs)
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
            tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor);
            sync_insert_str(&mut tab.paragraphs, tab.cursor, text);
            tab.content.insert_str(tab.cursor, text);
            tab.cursor += text.len(); // text is valid UTF-8 so len() == byte count
            tab.is_modified = true;
        }
        if let Some(rec) = self.vim_insertion_recording.as_mut() {
            rec.push_str(text);
        }
    }

    /// Like `insert_str`, but also stitches `runs` (already boundary-aligned
    /// to `text`, per `rich_clipboard::decode`'s own guarantee) into
    /// `tab.paragraphs` at the insertion point instead of leaving the
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
        if text.is_empty() { return; }
        self.push_undo_snapshot();
        if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
            self.delete_selection_raw();
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor);
            // Which paragraph the paste starts in, resolved *before* the
            // insert — afterwards the offsets have all moved.
            let first_para = crate::document_ops::resolve_position(&tab.paragraphs, tab.cursor).0;
            crate::document_ops::sync_insert_str_with_runs(&mut tab.paragraphs, tab.cursor, text, runs);
            tab.content.insert_str(tab.cursor, text);
            tab.cursor += text.len();
            tab.is_modified = true;

            // Only apply when the attribute list actually describes the text
            // being inserted, one entry per paragraph it spans. A mismatch
            // means the caller composed text and attributes from different
            // places, and applying them positionally anyway would stamp each
            // paragraph with its neighbour's card style — silent, and worse
            // than leaving the split's defaults alone.
            let spanned = text.matches('\n').count() + 1;
            if spanned == paragraph_attrs.len() {
                for (i, (heading, alignment)) in paragraph_attrs.iter().enumerate() {
                    if let Some(para) = tab.paragraphs.get_mut(first_para + i) {
                        para.heading = *heading;
                        para.alignment = *alignment;
                    }
                }
            }
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
        let Some(tab) = self.tabs.get_mut(self.active_tab) else { return };
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
        let (para_idx, run_idx, _) = resolve_position(&tab.paragraphs, probe);
        let Some(target) = tab.paragraphs.get(para_idx).and_then(|p| p.runs.get(run_idx)) else {
            return;
        };
        tab.similar_ranges = ranges_matching_format(&tab.paragraphs, &target.clone());
        tab.selection = None;
    }

    /// The range a Doc Menu cleanup command acts on: the active selection
    /// when there is one, else the whole document. Shared by every Doc Menu
    /// cleanup command below.
    fn selection_or_whole_document(&self) -> Option<(usize, usize)> {
        let tab = self.tabs.get(self.active_tab)?;
        Some(match tab.selection {
            Some((a, f)) => (a.min(f), a.max(f)),
            None => (0, tab.content.len()),
        })
    }

    /// Doc Menu → Remove emphasis: clears bold/underline/box from any run
    /// whose formatting matches the Emphasis button's own configured
    /// combination (`emphasis_bold`/`emphasis_underline`/`emphasis_box`)
    /// *exactly* — highlight doesn't factor in, so a highlighted emphasis
    /// run is fair game exactly like a plain one. Scope is the active
    /// selection, or the whole document with none, same as its three
    /// siblings below.
    ///
    /// Runs carrying a card-style marker (Tag/Cite/Analytic — Pocket/Hat/
    /// Block too, via `apply_card_style`'s `FormatOp::Style`) are excluded
    /// even when their formatting happens to coincide: Emphasis itself has
    /// no marker of its own, so "exactly that formatting" has to mean
    /// *unstyled* text, or emphasis configured to bold-only would eat every
    /// Tag in the document.
    pub fn remove_emphasis(&mut self) {
        let (want_bold, want_underline, want_box) =
            (self.emphasis_bold, self.emphasis_underline, self.emphasis_box);
        // All three off means Emphasis applies no formatting at all — matching
        // "exactly that" would otherwise mean every plain unstyled run in the
        // document, which is not what this command is for.
        if !want_bold && !want_underline && !want_box {
            return;
        }
        let matches = |r: &Run| {
            r.style.is_none()
                && r.bold == want_bold
                && r.underline == want_underline
                && r.box_format == want_box
        };

        let Some((start, end)) = self.selection_or_whole_document() else { return };
        if start >= end {
            return;
        }
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let any = runs_in_range(&tab.paragraphs, start, end).iter().any(&matches);
        // No undo entry for a no-op — Ctrl+Z should undo what the user did.
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let (start_para, start_run, start_char) = resolve_position(&tab.paragraphs, start);
            let (end_para, end_run, end_char) = resolve_position(&tab.paragraphs, end);
            // Same end-then-start split order as `apply_formatting`, so the
            // already-resolved start position isn't shifted by the end split.
            crate::document_ops::split_run_at_position(&mut tab.paragraphs, end_para, end_run, end_char);
            crate::document_ops::split_run_at_position(&mut tab.paragraphs, start_para, start_run, start_char);

            let mut cumulative = 0usize;
            for para in tab.paragraphs.iter_mut() {
                for run in para.runs.iter_mut() {
                    let run_start = cumulative;
                    let run_end = cumulative + run.text.len();
                    if run_start >= start && run_end <= end && matches(run) {
                        run.bold = false;
                        run.underline = false;
                        run.box_format = false;
                    }
                    cumulative = run_end;
                }
                cumulative += 1;
                crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
            }
            tab.is_modified = true;
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
        let Some((start, end)) = self.selection_or_whole_document() else { return };
        if start >= end {
            return;
        }
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let any = runs_in_range(&tab.paragraphs, start, end)
            .iter()
            .any(|r| r.underline && !r.highlight);
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let (start_para, start_run, start_char) = resolve_position(&tab.paragraphs, start);
            let (end_para, end_run, end_char) = resolve_position(&tab.paragraphs, end);
            // Same end-then-start split order as `apply_formatting`, so the
            // already-resolved start position isn't shifted by the end split.
            crate::document_ops::split_run_at_position(&mut tab.paragraphs, end_para, end_run, end_char);
            crate::document_ops::split_run_at_position(&mut tab.paragraphs, start_para, start_run, start_char);

            let mut cumulative = 0usize;
            for para in tab.paragraphs.iter_mut() {
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
            tab.is_modified = true;
        }
    }

    /// Doc Menu → Remove blank lines: deletes every paragraph with no text
    /// (blank however it's styled), across the whole document, or — with an
    /// active selection — only those whose line falls inside it.
    pub fn remove_blank_lines(&mut self) {
        let is_blank = |p: &Paragraph| p.runs.iter().all(|r| r.text.trim().is_empty());
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        // Inclusive [first_line, last_line] the selection's bytes fall
        // across, in the same "count '\n's before the offset" terms every
        // other line-index lookup here uses (see `cursor_line_col`). `None`
        // means no selection: every paragraph is in scope.
        let line_range = tab.selection.map(|(a, f)| {
            let (start, end) = (a.min(f), a.max(f));
            (
                tab.content[..start].matches('\n').count(),
                tab.content[..end].matches('\n').count(),
            )
        });
        let in_scope = |idx: usize| line_range.is_none_or(|(first, last)| idx >= first && idx <= last);
        let any = tab
            .paragraphs
            .iter()
            .enumerate()
            .any(|(i, p)| in_scope(i) && is_blank(p));
        if !any {
            return;
        }

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let mut idx = 0usize;
            tab.paragraphs.retain(|p| {
                let drop = in_scope(idx) && is_blank(p);
                idx += 1;
                !drop
            });
            // Every rich-text-aware function assumes at least one paragraph
            // and one run always exist (`default_paragraphs`).
            if tab.paragraphs.is_empty() {
                tab.paragraphs = default_paragraphs();
            }
            tab.content = paragraphs_to_plain_text(&tab.paragraphs);
            tab.cursor = clamp_to_char_boundary(&tab.content, tab.cursor.min(tab.content.len()));
            tab.selection = None;
            tab.is_modified = true;
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
        let Some((start, end)) = self.selection_or_whole_document() else { return };
        if start >= end {
            return;
        }
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        if !tab.content[start..end].contains('¶') {
            return;
        }

        let stripped = tab.content[start..end].replace('¶', "");
        // Mirrors `condense_selection_with`: capture the range's own runs
        // first, strip `¶` out of each run's text, then delete-and-reinsert
        // so every surviving character keeps its original formatting. A run
        // that was only a `¶` strips down to an empty string, which
        // `sync_insert_str_with_runs` silently contributes zero characters
        // for — nothing else to special-case.
        let stripped_runs: Vec<Run> = runs_in_range(&tab.paragraphs, start, end)
            .into_iter()
            .map(|mut r| { r.text = r.text.replace('¶', ""); r })
            .collect();

        self.push_undo_snapshot();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            sync_delete_range(&mut tab.paragraphs, start, end);
            tab.content.drain(start..end);
            sync_insert_str_with_runs(&mut tab.paragraphs, start, &stripped, &stripped_runs);
            tab.content.insert_str(start, &stripped);
            tab.cursor = start + stripped.len();
            tab.selection = None;
            tab.is_modified = true;
        }
    }

    /// Drops any "select similar formatting" highlight. Called from the
    /// editor's key-down and mouse-down handlers, the same two choke points
    /// that dismiss the right-click menu: the ranges are byte offsets with no
    /// way to follow an edit, so they must not outlive the next input event.
    pub fn clear_similar_selection(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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

    pub fn select_word_at(&mut self, byte_pos: usize) {
        /*
         * Double-click word selection: selects the same contiguous
         * char-class run vim's `iw` text object would (an alphanumeric
         * word run, a punctuation run, or a whitespace run — see
         * `text_object_word`), reusing its boundary math rather than
         * reimplementing word-boundary detection for the mouse gesture.
         */
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        let (start, end) = text_object_word(&tab.content, byte_pos, true);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return };
        if let Some((start, end)) = text_object_paragraph(&tab.content, byte_pos, true) {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
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
        let Some(tab) = self.tabs.get(self.active_tab) else { return (0, 0) };
        let start = line_start(&tab.content, tab.cursor);
        let col = tab.content[start..tab.cursor].chars().count();
        let line_idx = tab.content[..start].matches('\n').count();
        (line_idx, col)
    }

    /// `active_content` for a specific pane. The secondary pane is showing a
    /// different document than `active_tab` names, so it cannot go through the
    /// `active_*` helpers — those all mean "the focused pane's tab".
    pub fn pane_content(&self, pane: Pane) -> &str {
        self.pane_tab_index(pane)
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.content.as_str())
            .unwrap_or("")
    }

    /// `cursor_line_col` for a specific pane — see `pane_content`.
    pub fn pane_cursor_line_col(&self, pane: Pane) -> (usize, usize) {
        let Some(tab) = self.pane_tab_index(pane).and_then(|i| self.tabs.get(i)) else {
            return (0, 0);
        };
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

    pub fn set_working_directory(&mut self, dir: PathBuf) {
        /*
         * Re-roots the file explorer at `dir` (the "Open Folder" button) and
         * shows the sidebar, since picking a folder implies the user wants to
         * see it. Open tabs are untouched — this only affects the tree.
         * Persists the new `working_directory` to settings.conf so the app
         * reopens here next launch instead of resetting to the default.
         */
        self.working_directory = dir;
        self.sidebar_visible = true;
        self.refresh_file_tree();
        let _ = save_working_directory(&self.settings_path, &self.working_directory);
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
        self.file_context_menu = Some(FileContextMenu { position, target, confirming_delete: false });
    }

    pub fn close_file_context_menu(&mut self) {
        self.file_context_menu = None;
    }

    pub fn custom_colors(&self, target: CustomColorTarget) -> &[u32] {
        match target {
            CustomColorTarget::Font => &self.custom_font_colors,
            CustomColorTarget::Highlight => &self.custom_highlight_colors,
        }
    }

    /// Appends `hex` to the target list and persists it. Re-adding an existing
    /// color moves it to the end (most recent) rather than duplicating it.
    pub fn add_custom_color(&mut self, target: CustomColorTarget, hex: u32) {
        let list = match target {
            CustomColorTarget::Font => &mut self.custom_font_colors,
            CustomColorTarget::Highlight => &mut self.custom_highlight_colors,
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
            CustomColorTarget::Font => &mut self.custom_font_colors,
            CustomColorTarget::Highlight => &mut self.custom_highlight_colors,
        };
        let Some(pos) = list.iter().position(|c| *c == hex) else { return };
        list.remove(pos);
        self.persist_custom_colors(target);
    }

    /// Writes one custom color list back to settings.conf. A failed write is
    /// logged, not propagated — losing a saved swatch must never take the
    /// applied color (or the user's edit) with it.
    fn persist_custom_colors(&self, target: CustomColorTarget) {
        if let Err(e) = save_custom_colors(
            &self.settings_path,
            target.settings_key(),
            self.custom_colors(target),
        ) {
            log_line(&format!("[settings] failed to save custom colors: {e}"));
        }
    }

    pub fn request_context_menu_delete_confirmation(&mut self) {
        /*
         * Arms the "Delete <name>? Confirm / Cancel" step — see
         * `FileContextMenu.confirming_delete`'s doc comment for why deletion
         * isn't one click.
         */
        if let Some(menu) = self.file_context_menu.as_mut() {
            menu.confirming_delete = true;
        }
    }

    pub fn confirm_context_menu_delete(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        /*
         * Deletes the file the open context menu targets from disk and
         * refreshes the tree. A no-op (not an error) for a Dir or
         * Background target — deletion is scoped to files only, since
         * deleting a whole directory tree needs stronger confirmation than
         * this menu offers. Closes the menu either way.
         */
        let result: Result<(), Box<dyn std::error::Error>> = match self.file_context_menu.take() {
            Some(FileContextMenu { target: FileContextMenuTarget::File(path), .. }) => {
                std::fs::remove_file(&path).map_err(Into::into)
            }
            _ => Ok(()),
        };
        self.refresh_file_tree();
        result
    }

    pub fn create_file_at_context_menu_location(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        /*
         * Creates a new blank .docx in the context menu's target directory
         * (a File target's parent directory, a Dir target itself, or
         * `working_directory` for Background) and opens it — the
         * right-click counterpart to the sidebar's own "+" button
         * (`create_new_docx_in`), just targeting wherever was clicked
         * instead of always the tree's root.
         */
        let Some(menu) = self.file_context_menu.take() else { return Ok(()) };
        let dir = match menu.target {
            FileContextMenuTarget::File(path) => {
                path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.working_directory.clone())
            }
            FileContextMenuTarget::Dir(path) => path,
            FileContextMenuTarget::Background => self.working_directory.clone(),
        };
        self.create_new_docx_in(&dir)
    }

    pub fn create_new_docx_in(&mut self, dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        /*
         * Creates a new blank .docx file in `dir`, named the first unused
         * "Untitled.docx" / "Untitled 1.docx" / ... in sequence, opens it,
         * and refreshes the file tree. Shared by the sidebar's "+" button
         * (dir = working_directory) and the right-click menu's "New File"
         * (dir = wherever was clicked).
         */
        let mut name = "Untitled.docx".to_string();
        let mut counter = 1;
        while dir.join(&name).exists() {
            name = format!("Untitled {}.docx", counter);
            counter += 1;
        }
        let path = dir.join(&name);
        create_new_docx(&default_paragraphs(), &path)?;
        self.refresh_file_tree();
        self.open_file(path);
        Ok(())
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

    /// Drains the vim-keybind mailbox. `text_editor.rs` calls this right
    /// after dispatching every vim keystroke, same as
    /// `take_pending_clipboard_sync`, and if it returns `Some`, dispatches
    /// the action via `window.dispatch_action` — the one step this file
    /// can't do itself (no `window`/`cx` here).
    pub fn take_pending_vim_action(&mut self) -> Option<crate::keybinds::KeybindAction> {
        self.pending_vim_action.take()
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
        // Checklist: Settings -> Vim Mode. A vim-keybind sequence already
        // in progress (`Tab.vim_keybind_seq` non-empty) claims this key
        // unconditionally, checked before even the pending operator below.
        // Safe to check first because it's mutually exclusive with every
        // other pending state in this function by construction: a sequence
        // only ever *starts* via this function's final catch-all, which is
        // only reached once every other pending state has already declined
        // the keystroke — so if the buffer is non-empty, nothing else
        // could be racing it.
        if self.tabs.get(self.active_tab).is_some_and(|t| !t.vim_keybind_seq.is_empty()) {
            return self.continue_vim_keybind_sequence(key, shift, key_char);
        }

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
            // Real vim's Undo — this app's vim mode never wired it up before
            // (only `g`+`u`/`U`, the case-change operator, used the letter).
            // Calls the exact same `undo()` the app-level Undo keybind does,
            // so vim's `u` and the configurable Ctrl+Z stay in sync rather
            // than tracking two separate undo stacks. Shifted `U` (real
            // vim's "undo whole line") is out of scope, left unbound.
            ("u", false) => { self.undo(); true }
            // Checklist: Settings -> Vim Mode. The only place a *fresh*
            // vim-keybind sequence can start — every real vim command above
            // has already had first refusal, so a key that reaches here is
            // genuinely free to be claimed. A single-key binding fires
            // immediately; a longer one starts `vim_keybind_seq` for
            // `continue_vim_keybind_sequence` (checked at the very top of
            // this function) to pick up on the next keystroke. No match:
            // silently swallowed, exactly like this catch-all always has.
            _ => {
                if let Some(c) = vim_find_target_char(key, shift, key_char) {
                    self.dispatch_fresh_vim_keybind_key(c);
                }
                true
            }
        }
    }

    /// The continuation half of the vim-keybind sequence state machine —
    /// see `handle_vim_normal_key`'s own top-of-function check, which is
    /// what routes here. `Escape`, or any key that isn't a literal
    /// character (an arrow key, say), abandons the in-progress sequence
    /// rather than silently absorbing something unrelated to it.
    fn continue_vim_keybind_sequence(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool {
        if key == "escape" {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_keybind_seq.clear(); }
            return true;
        }
        let Some(c) = vim_find_target_char(key, shift, key_char) else {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_keybind_seq.clear(); }
            return true;
        };
        let seq = {
            let Some(tab) = self.tabs.get_mut(self.active_tab) else { return true };
            tab.vim_keybind_seq.push(c);
            tab.vim_keybind_seq.clone()
        };
        match self.vim_keybinds.lookup(&seq) {
            crate::vim_keybinds::VimLookup::Exact(action) => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_keybind_seq.clear(); }
                self.pending_vim_action = Some(action);
            }
            crate::vim_keybinds::VimLookup::Prefix => {}
            crate::vim_keybinds::VimLookup::None => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_keybind_seq.clear(); }
            }
        }
        true
    }

    /// A single fresh character, unclaimed by every real vim command ahead
    /// of it in `handle_vim_normal_key`'s dispatch order — checks whether it
    /// starts (or, for a one-character binding, completes) a vim-keybind
    /// sequence. Shares its match-and-branch logic with
    /// `continue_vim_keybind_sequence` but starts from an empty buffer
    /// rather than an in-progress one, so it's kept as its own small
    /// function rather than forcing one path to pretend it has a buffer to
    /// continue.
    fn dispatch_fresh_vim_keybind_key(&mut self, c: char) {
        let seq = c.to_string();
        match self.vim_keybinds.lookup(&seq) {
            crate::vim_keybinds::VimLookup::Exact(action) => {
                self.pending_vim_action = Some(action);
            }
            crate::vim_keybinds::VimLookup::Prefix => {
                if let Some(tab) = self.tabs.get_mut(self.active_tab) { tab.vim_keybind_seq = seq; }
            }
            crate::vim_keybinds::VimLookup::None => {}
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
                    // `g$`/`g0`/`g^`: the escape hatch to real vim's original
                    // (logical-line) meaning of `$`/`0`/`^`, now that bare
                    // `$`/`0`/`^` resolve to the current *visual* row instead
                    // (`text_editor.rs`'s own interception, ahead of this
                    // dispatcher, for a heavily-wrapping document editor —
                    // see its doc comment). Deliberately the exact same
                    // `line_end`/`line_start`/`first_nonblank` calls the
                    // plain (non-`g`) arms below already use — this is only
                    // reachable from a pending `g`, so it never overlaps
                    // with them.
                    else if matches_shifted_symbol(key, shift, key_char, "4", "$") {
                        let target = self.tabs.get(self.active_tab).map(|tab| line_end(&tab.content, tab.cursor)).unwrap_or(0);
                        return MotionResolution::Resolved { target, kind: MotionKind::InclusiveChar };
                    } else if key == "0" && !shift {
                        let target = self.tabs.get(self.active_tab).map(|tab| line_start(&tab.content, tab.cursor)).unwrap_or(0);
                        return MotionResolution::Resolved { target, kind: MotionKind::ExclusiveChar };
                    } else if matches_shifted_symbol(key, shift, key_char, "6", "^") {
                        let target = self.tabs.get(self.active_tab).map(|tab| first_nonblank(&tab.content, tab.cursor)).unwrap_or(0);
                        return MotionResolution::Resolved { target, kind: MotionKind::ExclusiveChar };
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
            dirs.push(FileNode::Dir {
                name,
                path,
                children: Vec::new(),
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

    /// Makes a unique temp dir for one test. Mirrors `recovery.rs`'s helper —
    /// no `tempfile` dependency.
    fn custom_color_temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("vimbatim-color-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_load_custom_colors_parses_pipe_separated_hex() {
        let dir = custom_color_temp_dir("load");
        let path = dir.join("settings.conf");
        std::fs::write(&path, "[FORMATTING]\ncustom_highlight_colors=00ff88|aabbcc\n").unwrap();
        assert_eq!(load_custom_colors(&path, "custom_highlight_colors"), vec![0x00ff88, 0xaabbcc]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_custom_colors_tolerates_missing_and_garbage() {
        let dir = custom_color_temp_dir("tolerant");
        let path = dir.join("settings.conf");

        // Missing file.
        assert!(load_custom_colors(&path, "custom_font_colors").is_empty());

        // Missing key, empty value, and unparseable entries mixed with good ones.
        std::fs::write(
            &path,
            "[FORMATTING]\ncustom_font_colors=\ncustom_highlight_colors=00ff88|zzzzzz|1234567|aabbcc\n",
        )
        .unwrap();
        assert!(load_custom_colors(&path, "custom_font_colors").is_empty());
        assert!(load_custom_colors(&path, "nonexistent_key").is_empty());
        assert_eq!(
            load_custom_colors(&path, "custom_highlight_colors"),
            vec![0x00ff88, 0xaabbcc],
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_then_load_custom_colors_round_trips() {
        let dir = custom_color_temp_dir("roundtrip");
        let path = dir.join("settings.conf");
        std::fs::write(&path, "[FORMATTING]\ntheme=nord\n\n[KEYBINDS]\nvim=false\n").unwrap();

        save_custom_colors(&path, "custom_font_colors", &[0x00ff88, 0x000000]).unwrap();
        assert_eq!(load_custom_colors(&path, "custom_font_colors"), vec![0x00ff88, 0x000000]);

        // An unrelated existing key survives the write.
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("theme=nord"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_add_custom_color_dedups_by_moving_to_the_end() {
        let mut state = make_state("", 0, None);
        state.custom_highlight_colors = vec![0x111111, 0x222222, 0x333333];
        state.add_custom_color(CustomColorTarget::Highlight, 0x222222);
        assert_eq!(state.custom_highlight_colors, vec![0x111111, 0x333333, 0x222222]);
    }

    #[test]
    fn test_add_custom_color_caps_the_list_dropping_oldest() {
        let mut state = make_state("", 0, None);
        for i in 0..MAX_CUSTOM_COLORS as u32 {
            state.add_custom_color(CustomColorTarget::Font, i);
        }
        assert_eq!(state.custom_font_colors.len(), MAX_CUSTOM_COLORS);
        assert_eq!(state.custom_font_colors[0], 0);

        state.add_custom_color(CustomColorTarget::Font, 0xFFFFFF);
        assert_eq!(state.custom_font_colors.len(), MAX_CUSTOM_COLORS);
        assert_eq!(state.custom_font_colors[0], 1, "oldest entry should be dropped");
        assert_eq!(*state.custom_font_colors.last().unwrap(), 0xFFFFFF);
    }

    #[test]
    fn test_remove_custom_color_drops_it_and_leaves_the_rest() {
        let mut state = make_state("", 0, None);
        state.custom_highlight_colors = vec![0x111111, 0x222222, 0x333333];
        state.remove_custom_color(CustomColorTarget::Highlight, 0x222222);
        assert_eq!(state.custom_highlight_colors, vec![0x111111, 0x333333]);
    }

    #[test]
    fn test_remove_custom_color_is_a_noop_for_an_absent_color() {
        let mut state = make_state("", 0, None);
        state.custom_font_colors = vec![0x111111];
        state.remove_custom_color(CustomColorTarget::Font, 0x999999);
        assert_eq!(state.custom_font_colors, vec![0x111111]);
    }

    #[test]
    fn test_remove_custom_color_only_touches_its_own_list() {
        let mut state = make_state("", 0, None);
        state.custom_font_colors = vec![0x00ff88];
        state.custom_highlight_colors = vec![0x00ff88];
        state.remove_custom_color(CustomColorTarget::Highlight, 0x00ff88);
        assert!(state.custom_highlight_colors.is_empty());
        assert_eq!(state.custom_font_colors, vec![0x00ff88]);
    }

    #[test]
    fn test_add_custom_color_keeps_the_two_lists_separate() {
        let mut state = make_state("", 0, None);
        state.add_custom_color(CustomColorTarget::Highlight, 0x00ff88);
        assert_eq!(state.custom_highlight_colors, vec![0x00ff88]);
        assert!(state.custom_font_colors.is_empty());
    }

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
                content_version: 0,
                is_modified: false,
                paragraphs: default_paragraphs(),
                docx_origin: None,
                pending_format: None,
                cursor,
                selection,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
                last_edit_at: None,
                last_snapshot_version: 0,
                last_snapshot_cost: None,
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
                vim_keybind_seq: String::new(),
                vim_search_direction: true,
                vim_jump_back: Vec::new(),
                vim_jump_forward: Vec::new(),
                pending_scroll_to_cursor: false,
            folded_headings: std::collections::HashSet::new(),
            folded_para_count: 0,
            fold_version: 0,
                similar_ranges: Vec::new(),
                has_unsupported_blocks: false,
                unsupported_banner_dismissed: false,
            }],
            active_tab: 0,
            pending_focus_editor: None,
            next_tab_id: 1,
            closed_tabs: Vec::new(),
            sidebar_visible: false,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            file_context_menu: None,
            editor_context_menu: None,
            find_bar: None,
            word_count_visible: false,
            timer: crate::timer::TimerState::default(),
            spreading_wpm: DEFAULT_SPREADING_WPM,
            custom_font_colors: Vec::new(),
            custom_highlight_colors: Vec::new(),
            sidebar_mode: SidebarMode::default(),
            settings_visible: false,
            pending_close: None,
            pending_recovery: Vec::new(),
            working_directory: std::path::PathBuf::from("."),
            file_tree: vec![],
            vim_enabled: true,
            keybinds: crate::keybinds::Keybinds::defaults(),
            vim_keybinds: crate::vim_keybinds::VimKeybinds::defaults(),
            pending_vim_action: None,
            theme: crate::theme::ThemeKind::WorkbenchDark,
            theme_mode: crate::theme::ThemeMode::Dark,
            theme_color_mode: crate::theme::ThemeColorMode::Minimal,
            custom_theme: None,
            normal_text_size_half_points: 22,
            pocket_size_half_points: 52,
            block_size_half_points: 32,
            tag_size_half_points: 26,
            cite_size_half_points: 26,
            small_size_half_points: 12,
            zoom: 1.0,
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
            highlight_color: "yellow".to_string(),
            analytic_color: "0000ff".to_string(),
            standardize_highlight_exception: String::new(),
            emphasis_bold: true,
            emphasis_underline: false,
            emphasis_box: false,
            paste_condense: false,
            paste_condense_pilcrow: false,
            // A temp file, never the real ~/.vimbatim/settings.conf — see
            // the field's doc comment.
            settings_path: std::env::temp_dir().join("vimbatim_test_settings.conf"),
            // Off in tests: on would mean every fixture string gets run
            // through the bundled dictionary, for no assertion's benefit.
            spellcheck_enabled: false,
            spellcheck_underline_color: "red".to_string(),
            user_dictionary: Rc::new(HashSet::new()),
            invisibility_mode: false,
            print_layout: false,
            split_view: false,
            secondary_tab_id: None,
            focused_pane: Pane::Primary,
            primary_tab_id: None,
            split_ratio: 0.5,
            split_dragging: false,
            read_mode: false,
            sidebar_before_read_mode: true,
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

    // ── Opening a file reuses a blank "New Tab" ────────────────────────────

    fn temp_docx(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("vimbatim_reuse_{}_{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("card.docx");
        create_new_docx(&default_paragraphs(), &path).unwrap();
        (dir, path)
    }

    #[test]
    fn opening_a_file_replaces_an_untouched_new_tab() {
        let (dir, path) = temp_docx("replace");
        let mut state = make_state("", 0, None);
        state.tabs[0] = Tab::new_empty(0); // a pristine "New Tab"
        assert_eq!(state.tabs.len(), 1);

        state.open_file(path.clone());

        assert_eq!(state.tabs.len(), 1, "a blank tab was left stranded");
        assert_eq!(state.tabs[0].file_path.as_deref(), Some(path.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_a_file_keeps_a_tab_that_has_been_typed_in() {
        let (dir, path) = temp_docx("keep-typed");
        let mut state = make_state("", 0, None);
        state.tabs[0] = Tab::new_empty(0);
        state.insert_str("draft"); // now it holds work

        state.open_file(path.clone());

        assert_eq!(state.tabs.len(), 2, "unsaved work was overwritten");
        assert_eq!(state.tabs[0].content, "draft");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Undoing back to empty leaves a tab that *looks* blank but has history
    /// and a dirty flag. Replacing it would silently discard that.
    #[test]
    fn a_tab_emptied_by_undo_is_not_treated_as_a_blank_new_tab() {
        let mut state = make_state("", 0, None);
        state.tabs[0] = Tab::new_empty(0);
        state.insert_str("typed");
        state.undo();

        assert!(state.tabs[0].content.is_empty());
        assert!(!state.tabs[0].is_blank_new_tab(), "undo-emptied tab looked pristine");
    }

    #[test]
    fn opening_a_file_in_the_split_replaces_only_that_panes_blank_tab() {
        let (dir, path) = temp_docx("split");
        let mut state = make_state("primary work", 0, None);
        state.open_split(); // secondary gets a blank tab, and focus
        let tabs_before = state.tabs.len();

        state.open_file(path.clone());

        assert_eq!(state.tabs.len(), tabs_before, "split's blank tab was not reused");
        let secondary = state.pane_tab_index(Pane::Secondary).unwrap();
        assert_eq!(state.tabs[secondary].file_path.as_deref(), Some(path.as_path()));
        // The other pane is untouched.
        assert_eq!(state.pane_content(Pane::Primary), "primary work");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Level-aware folding ────────────────────────────────────────────────

    fn outline() -> Vec<Paragraph> {
        let card = |text: &str, heading: u8| Paragraph {
            runs: vec![Run { text: text.into(), ..Run::default() }],
            heading,
            alignment: Alignment::default(),
            unsupported_xml: None,
        };
        vec![
            card("pocket A", 1),   // 0
            para_plain("body A"),  // 1
            card("hat A", 2),      // 2
            para_plain("body B"),  // 3
            card("tag A", 4),      // 4
            para_plain("body C"),  // 5
            card("pocket B", 1),   // 6
            para_plain("body D"),  // 7
        ]
    }

    /// Collapsing a Pocket takes everything under it — including the Hats and
    /// Tags nested beneath — until the next heading at its level or higher.
    #[test]
    fn folding_a_heading_hides_lower_levels_beneath_it() {
        let paragraphs = outline();
        let folded = std::collections::HashSet::from([0usize]);

        let hidden = AppState::folded_paragraphs(&paragraphs, &folded);

        assert_eq!(
            hidden,
            vec![false, true, true, true, true, true, false, false],
            "a collapsed Pocket must swallow its Hats and Tags, and stop at the next Pocket"
        );
    }

    /// Collapsing a lower-level heading takes only its own section.
    #[test]
    fn folding_a_nested_heading_leaves_its_parents_alone() {
        let paragraphs = outline();
        let folded = std::collections::HashSet::from([2usize]); // the Hat

        let hidden = AppState::folded_paragraphs(&paragraphs, &folded);

        // Hat's section runs to the next heading of level <= 2, i.e. Pocket B.
        assert_eq!(hidden, vec![false, false, false, true, true, true, false, false]);
    }

    #[test]
    fn folding_nothing_hides_nothing() {
        let paragraphs = outline();
        let hidden = AppState::folded_paragraphs(&paragraphs, &std::collections::HashSet::new());
        assert_eq!(hidden, vec![false; 8]);
    }

    /// A heading that closes one collapsed section can open another in the
    /// same step — the two Pockets here are both collapsed.
    #[test]
    fn a_heading_can_end_one_section_and_start_the_next() {
        let paragraphs = outline();
        let folded = std::collections::HashSet::from([0usize, 6usize]);

        let hidden = AppState::folded_paragraphs(&paragraphs, &folded);

        assert_eq!(hidden, vec![false, true, true, true, true, true, false, true]);
    }

    #[test]
    fn toggle_fold_collapses_every_heading_then_expands_all() {
        let mut state = make_state_with_paragraphs(outline(), 0);

        state.toggle_fold();
        assert!(state.any_folded());
        let hidden = AppState::folded_paragraphs(
            &state.tabs[0].paragraphs,
            &state.tabs[0].folded_headings,
        );
        // Only the two Pockets survive: each swallows everything beneath it.
        assert_eq!(hidden, vec![false, true, true, true, true, true, false, true]);

        state.toggle_fold();
        assert!(!state.any_folded(), "second press should expand everything");
    }

    #[test]
    fn toggling_one_heading_leaves_the_others_alone() {
        let mut state = make_state_with_paragraphs(outline(), 0);
        state.toggle_fold(); // collapse all

        state.toggle_paragraph_fold(0); // expand just Pocket A

        assert!(!state.tabs[0].folded_headings.contains(&0));
        assert!(state.tabs[0].folded_headings.contains(&6));
    }

    #[test]
    fn body_paragraphs_cannot_be_folded() {
        let mut state = make_state_with_paragraphs(outline(), 0);
        state.toggle_paragraph_fold(1); // a body line
        assert!(state.tabs[0].folded_headings.is_empty());
    }

    /// Fold state is keyed by paragraph index, so a structural edit drops it
    /// rather than folding the wrong sections — see `Tab.folded_headings`.
    #[test]
    fn fold_state_is_dropped_when_the_paragraph_count_changes() {
        let mut state = make_state_with_paragraphs(outline(), 0);
        state.toggle_fold();
        assert!(state.any_folded());

        // Split a paragraph in two.
        state.tabs[0].cursor = 0;
        state.insert_str("\n");

        assert!(!state.any_folded(), "stale folds survived a structural edit");
    }

    // ── Text settings ──────────────────────────────────────────────────────
    // ── Standardize highlighting ───────────────────────────────────────────
    // ── Analytic style ─────────────────────────────────────────────────────
    fn analytic_para(text: &str, size: u16, color: &str) -> Paragraph {
        Paragraph {
            runs: vec![Run {
                text: text.into(),
                bold: true,
                size,
                color: Some(color.into()),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }
    }

    #[test]
    fn delete_analytics_removes_whole_lines() {
        let paragraphs = vec![
            para_plain("keep this"),
            analytic_para("an analytic", 26, "0000ff"),
            para_plain("and this"),
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.delete_analytics();

        assert_eq!(state.tabs[0].paragraphs.len(), 2, "the line should be gone, not blanked");
        // `content` is rebuilt from the survivors — the 1:1 line/paragraph
        // invariant the rest of the editor depends on.
        assert_eq!(state.tabs[0].content, "keep this\nand this");
    }

    #[test]
    fn delete_analytics_leaves_other_formatting_alone() {
        let paragraphs = vec![analytic_para("wrong color", 26, "c00000")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.delete_analytics();

        assert_eq!(state.tabs[0].paragraphs.len(), 1);
    }

    /// A document that is nothing but analytics must still end up with one
    /// paragraph — every rich-text function assumes at least one exists.
    #[test]
    fn deleting_every_paragraph_leaves_a_blank_one() {
        let paragraphs = vec![
            analytic_para("one", 26, "0000ff"),
            analytic_para("two", 26, "0000ff"),
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.delete_analytics();

        assert_eq!(state.tabs[0].paragraphs.len(), 1);
        assert!(state.tabs[0].content.is_empty());
        assert!(!state.tabs[0].paragraphs[0].runs.is_empty(), "a paragraph always has a run");
    }

    /// The cursor pointed into text that no longer exists.
    #[test]
    fn delete_analytics_clamps_the_cursor() {
        let paragraphs = vec![para_plain("ab"), analytic_para("long analytic", 26, "0000ff")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");
        state.tabs[0].cursor = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, state.tabs[0].content.len()));

        state.delete_analytics();

        assert!(state.tabs[0].cursor <= state.tabs[0].content.len());
        assert_eq!(state.tabs[0].selection, None, "a selection into deleted text must clear");
    }

    #[test]
    fn delete_analytics_is_a_no_op_when_there_are_none() {
        let mut state = make_state_with_paragraphs(vec![para_plain("just text")], 0);
        state.set_analytic_color("0000ff");
        let version_before = state.tabs[0].content_version;

        state.delete_analytics();

        assert_eq!(state.tabs[0].content_version, version_before);
        assert!(!state.tabs[0].is_modified);
    }

    /// The marker is authoritative: a reformatted analytic is still one, even
    /// though its bold/size/color no longer match the signature.
    #[test]
    fn a_marked_analytic_is_recognised_regardless_of_its_formatting() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run {
                text: "reformatted".into(),
                bold: false,
                size: 99,
                color: Some("00ff00".into()),
                style: Some(CardStyle::Analytic),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.set_analytic_color("0000ff");

        state.convert_analytics_to_tags();

        assert_eq!(state.tabs[0].paragraphs[0].heading, 4);
    }

    /// ...and the converse: text that coincidentally matches the old
    /// signature but is marked as something else is left alone. This is the
    /// misidentification the marker exists to prevent.
    #[test]
    fn a_marked_cite_is_not_mistaken_for_an_analytic() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run {
                text: "a cite".into(),
                bold: true,
                size: 26,
                color: Some("0000ff".into()),
                style: Some(CardStyle::Cite),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.convert_analytics_to_tags();
        state.delete_analytics();

        assert_eq!(state.tabs[0].paragraphs.len(), 1, "a marked Cite was deleted as an analytic");
        assert_eq!(state.tabs[0].paragraphs[0].heading, 0);
    }

    #[test]
    fn applying_a_card_style_stamps_its_marker() {
        let mut state = make_state_with_paragraphs(vec![para_plain("a line")], 0);
        state.apply_card_style(CardStyleKind::Block);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].style, Some(CardStyle::Block));
    }

    #[test]
    fn applying_cite_and_analytic_stamps_their_markers() {
        let mut state = make_state_with_paragraphs(vec![para_plain("some text")], 0);
        state.tabs[0].selection = Some((0, 9));
        state.apply_cite_style();
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].style, Some(CardStyle::Cite));

        let mut state = make_state_with_paragraphs(vec![para_plain("some text")], 0);
        state.apply_analytic_style();
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].style, Some(CardStyle::Analytic));
    }

    /// Clearing formatting clears what the run *was*, not just how it looked —
    /// otherwise a cleared line still answers to "is this an analytic?".
    #[test]
    fn clearing_formatting_clears_the_marker() {
        let mut state = make_state_with_paragraphs(vec![para_plain("a line")], 0);
        state.apply_analytic_style();
        state.tabs[0].selection = Some((0, 6));

        state.clear_formatting();

        assert_eq!(state.tabs[0].paragraphs[0].runs[0].style, None);
    }

    #[test]
    fn convert_analytics_promotes_them_to_tags() {
        let paragraphs = vec![
            analytic_para("an analytic", 26, "0000ff"),
            para_plain("ordinary body text"),
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.convert_analytics_to_tags();

        let converted = &state.tabs[0].paragraphs[0];
        assert_eq!(converted.heading, 4, "should now be a Tag");
        assert_eq!(converted.runs[0].color, None, "a Tag is plain-colored");
        assert!(converted.runs[0].bold);
        assert_eq!(state.tabs[0].paragraphs[1].heading, 0, "body text untouched");
    }

    /// Only paragraphs matching the analytic signature convert — bold text at
    /// the same size in a *different* color is someone else's formatting.
    #[test]
    fn convert_analytics_ignores_other_colored_text() {
        let paragraphs = vec![analytic_para("not an analytic", 26, "c00000")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.convert_analytics_to_tags();

        assert_eq!(state.tabs[0].paragraphs[0].heading, 0);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].color.as_deref(), Some("c00000"));
    }

    #[test]
    fn convert_analytics_is_a_no_op_when_there_are_none() {
        let mut state = make_state_with_paragraphs(vec![para_plain("just text")], 0);
        state.set_analytic_color("0000ff");
        let version_before = state.tabs[0].content_version;

        state.convert_analytics_to_tags();

        assert_eq!(state.tabs[0].content_version, version_before);
        assert!(!state.tabs[0].is_modified);
    }

    /// Round-trip: what `apply_analytic_style` produces is exactly what the
    /// converter recognises, so the two cannot drift apart.
    #[test]
    fn a_freshly_applied_analytic_converts() {
        let mut state = make_state_with_paragraphs(vec![para_plain("some analysis")], 0);
        state.tag_size_half_points = 26;
        state.set_analytic_color("0000ff");

        state.apply_analytic_style();
        assert_eq!(state.tabs[0].paragraphs[0].heading, 0, "analytic must not be a heading");

        state.convert_analytics_to_tags();

        assert_eq!(state.tabs[0].paragraphs[0].heading, 4);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].color, None);
    }

    /// A blank line is never an analytic — converting one would put an empty
    /// Tag in the Nav outline.
    #[test]
    fn convert_analytics_skips_blank_lines() {
        let mut state = make_state_with_paragraphs(vec![para_plain("")], 0);
        state.set_analytic_color("0000ff");

        state.convert_analytics_to_tags();

        assert_eq!(state.tabs[0].paragraphs[0].heading, 0);
    }


    #[test]
    fn analytic_applies_tag_weight_and_size_in_the_configured_color() {
        let mut state = make_state("an analytic", 0, None);
        state.tag_size_half_points = 26;
        state.set_analytic_color("c00000");

        state.apply_analytic_style();

        let run = &state.tabs[0].paragraphs[0].runs[0];
        assert!(run.bold);
        assert_eq!(run.size, 26, "should match the configured Tag size");
        assert_eq!(run.color.as_deref(), Some("c00000"));
    }

    /// The whole point of Analytic over Tag: it is the debater's own argument,
    /// not a structural marker, so it must stay out of the Nav outline, the
    /// fold hierarchy, and Wikifi's heading levels — all of which read
    /// `Paragraph.heading`.
    #[test]
    fn analytic_is_not_a_heading() {
        let mut state = make_state("an analytic", 0, None);
        state.apply_analytic_style();
        assert_eq!(state.tabs[0].paragraphs[0].heading, 0);
    }

    /// Converting a Tag line to an Analytic has to clear the heading it
    /// already carried, or the line stays in the outline while no longer
    /// looking like a card style.
    #[test]
    fn analytic_clears_an_existing_card_style_heading() {
        let mut state = make_state("was a tag", 0, None);
        state.apply_card_style(CardStyleKind::Tag);
        assert_eq!(state.tabs[0].paragraphs[0].heading, 4);

        state.apply_analytic_style();

        assert_eq!(state.tabs[0].paragraphs[0].heading, 0);
    }

    #[test]
    fn setting_the_highlight_color_is_what_later_operations_read() {
        let mut state = make_state("", 0, None);
        assert_eq!(state.highlight_color, "yellow");

        state.set_highlight_color("cyan");
        assert_eq!(state.highlight_color, "cyan");

        // A custom color is stored as a bare hex, which
        // `text_editor::highlight_color_hex` also parses.
        state.set_highlight_color("86f2ef");
        assert_eq!(state.highlight_color, "86f2ef");
    }

    /// Picking a color and then standardizing must use the picked one — the
    /// two features share `highlight_color` rather than each having their own
    /// idea of "current".
    #[test]
    fn standardize_follows_the_most_recently_picked_color() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl("marked", "green")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);

        state.set_highlight_color("cyan");
        state.standardize_highlighting();

        assert_eq!(state.tabs[0].paragraphs[0].runs[0].highlight_color, "cyan");
    }


    fn hl(text: &str, color: &str) -> Run {
        Run { text: text.into(), highlight: true, highlight_color: color.into(), ..Run::default() }
    }

    #[test]
    fn standardize_repaints_every_highlight_to_the_current_color() {
        let paragraphs = vec![
            Paragraph {
                runs: vec![hl("green bit", "green"), run_plain(" plain "), hl("cyan bit", "cyan")],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            Paragraph {
                runs: vec![hl("magenta bit", "magenta")],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.highlight_color = "yellow".to_string();

        state.standardize_highlighting();

        for para in &state.tabs[0].paragraphs {
            for run in &para.runs {
                if run.highlight {
                    assert_eq!(run.highlight_color, "yellow");
                }
            }
        }
    }

    /// Only the color changes — nothing gains or loses a highlight, and no
    /// text moves.
    #[test]
    fn standardize_leaves_unhighlighted_text_alone() {
        let paragraphs = vec![Paragraph {
            runs: vec![run_plain("before "), hl("marked", "green"), run_plain(" after")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let content_before = state.tabs[0].content.clone();
        state.highlight_color = "yellow".to_string();

        state.standardize_highlighting();

        assert_eq!(state.tabs[0].content, content_before, "text must not move");
        let runs = &state.tabs[0].paragraphs[0].runs;
        assert!(!runs[0].highlight, "plain text gained a highlight");
        assert!(!runs.last().unwrap().highlight);
    }

    /// Runs that differed only by highlight color are identical afterwards, so
    /// they fuse rather than leaving the document split on a distinction that
    /// no longer exists.
    #[test]
    fn standardize_merges_runs_that_now_match() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl("one ", "green"), hl("two", "cyan")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.highlight_color = "yellow".to_string();

        state.standardize_highlighting();

        assert_eq!(state.tabs[0].paragraphs[0].runs.len(), 1);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].text, "one two");
    }

    #[test]
    fn standardize_with_exception_spares_the_chosen_color() {
        let paragraphs = vec![Paragraph {
            runs: vec![
                hl("ordinary", "green"),
                run_plain(" "),
                hl("meaningful", "cyan"),
                run_plain(" "),
                hl("also ordinary", "magenta"),
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.set_highlight_color("yellow");
        state.set_standardize_exception("cyan");

        state.standardize_highlighting_with_exception();

        let colors: Vec<&str> = state.tabs[0].paragraphs[0]
            .runs
            .iter()
            .filter(|r| r.highlight)
            .map(|r| r.highlight_color.as_str())
            .collect();
        assert_eq!(colors, vec!["yellow", "cyan", "yellow"]);
    }

    /// With nothing configured, the exception command is the plain one.
    #[test]
    fn standardize_with_no_exception_repaints_everything() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl("a", "green"), hl("b", "cyan")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.set_highlight_color("yellow");
        assert!(state.standardize_highlight_exception.is_empty());

        state.standardize_highlighting_with_exception();

        // Both repainted, and now identical, so they fused.
        assert_eq!(state.tabs[0].paragraphs[0].runs.len(), 1);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].highlight_color, "yellow");
    }

    /// A document where only the excepted color differs has nothing to do —
    /// no undo entry, so Ctrl+Z still undoes whatever the user actually did.
    #[test]
    fn standardize_with_exception_is_a_no_op_when_only_the_exception_differs() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl("kept", "cyan"), hl(" done", "yellow")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.set_highlight_color("yellow");
        state.set_standardize_exception("cyan");
        let version_before = state.tabs[0].content_version;

        state.standardize_highlighting_with_exception();

        assert_eq!(state.tabs[0].content_version, version_before);
        assert!(!state.tabs[0].is_modified);
    }

    #[test]
    fn standardize_is_undoable() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl("marked", "green")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.highlight_color = "yellow".to_string();
        let version_before = state.tabs[0].content_version;

        state.standardize_highlighting();

        assert!(state.tabs[0].is_modified);
        assert!(state.tabs[0].content_version > version_before, "row cache would go stale");
    }

    /// A document already in the right color must not push an undo entry —
    /// Ctrl+Z afterwards should undo whatever the user actually did last.
    #[test]
    fn standardize_is_a_no_op_when_already_uniform() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl("marked", "yellow")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.highlight_color = "yellow".to_string();
        let version_before = state.tabs[0].content_version;

        state.standardize_highlighting();

        assert_eq!(state.tabs[0].content_version, version_before);
        assert!(!state.tabs[0].is_modified);
    }

    #[test]
    fn shrink_size_setter_stores_half_points_and_clamps() {
        let mut state = make_state("", 0, None);

        state.set_shrink_size_points(8);
        assert_eq!(state.small_size_half_points, 16, "stored in half-points");

        // Clamped at both ends rather than walking somewhere unusable.
        state.set_shrink_size_points(0);
        assert_eq!(state.small_size_half_points, 8); // 4pt floor
        state.set_shrink_size_points(999);
        assert_eq!(state.small_size_half_points, 96); // 48pt ceiling
    }

    /// Shrink applies whatever the setting currently says, not a fixed size.
    #[test]
    fn shrink_uses_the_configured_size() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "shrink me".into(), size: 44, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 9));
        state.set_shrink_size_points(7);

        state.shrink_text();

        assert_eq!(state.tabs[0].paragraphs[0].runs[0].size, 14, "7pt = 14 half-points");
    }

    #[test]
    fn text_settings_load_from_settings_conf() {
        let dir = std::env::temp_dir().join(format!("vimbatim_textset_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");
        std::fs::write(
            &path,
            "[FORMATTING]\nhighlight_color=cyan\n\n[TEXT]\nemphasis_bold=false\n\
             emphasis_underline=true\nemphasis_box=true\npaste_condense=true\n\
             paste_condense_pilcrow=true\n",
        )
        .unwrap();

        assert_eq!(load_string_setting(&path, "highlight_color", "yellow"), "cyan");
        assert!(!load_bool_setting(&path, "emphasis_bold", true));
        assert!(load_bool_setting(&path, "emphasis_underline", false));
        assert!(load_bool_setting(&path, "emphasis_box", false));
        assert!(load_bool_setting(&path, "paste_condense", false));
        assert!(load_bool_setting(&path, "paste_condense_pilcrow", false));

        let _ = std::fs::remove_dir_all(&dir);
    }



    #[test]
    fn paste_keeps_newlines_when_condense_is_off() {
        let mut state = make_state("", 0, None);
        state.paste_condense = false;
        state.paste_text("one\ntwo");
        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    #[test]
    fn paste_condenses_newlines_to_spaces() {
        let mut state = make_state("", 0, None);
        state.paste_condense = true;
        state.paste_text("one\ntwo");
        assert_eq!(state.tabs[0].content, "one two");
    }

    #[test]
    fn paste_condense_marks_newlines_with_a_pilcrow_when_asked() {
        let mut state = make_state("", 0, None);
        state.paste_condense = true;
        state.paste_condense_pilcrow = true;
        state.paste_text("one\ntwo");
        assert_eq!(state.tabs[0].content, "one¶two");
    }

    /// The pilcrow sub-setting is meaningless on its own — condensing off means
    /// the newline is kept, not replaced with a mark.
    #[test]
    fn the_pilcrow_setting_does_nothing_while_condense_is_off() {
        let mut state = make_state("", 0, None);
        state.paste_condense = false;
        state.paste_condense_pilcrow = true;
        state.paste_text("one\ntwo");
        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    /// Paragraph integrity and condense-on-paste are opposites: turning the
    /// first on has to switch the second off, or the ribbon claims to preserve
    /// paragraphs while the paste collapses them.
    #[test]
    fn paragraph_integrity_turns_condense_off() {
        let mut state = make_state("", 0, None);
        state.paste_condense = true;

        state.toggle_paragraph_integrity();

        assert!(state.paragraph_integrity);
        assert!(!state.paste_condense);
    }

    #[test]
    fn turning_paragraph_integrity_back_off_leaves_condense_alone() {
        let mut state = make_state("", 0, None);
        state.toggle_paragraph_integrity(); // on -> condense forced off
        state.set_paste_condense(true); // user turns it back on deliberately

        state.toggle_paragraph_integrity(); // off again

        assert!(!state.paragraph_integrity);
        assert!(state.paste_condense, "toggling integrity off should not undo a deliberate choice");
    }

    /// The Pilcrows ribbon button and the settings checkbox are the same
    /// switch, not two that can disagree.
    #[test]
    fn the_pilcrow_button_drives_the_pilcrow_setting() {
        let mut state = make_state("", 0, None);
        assert!(!state.paste_condense_pilcrow);

        state.toggle_pilcrows();
        assert!(state.pilcrows);
        assert!(state.paste_condense_pilcrow);

        state.toggle_pilcrows();
        assert!(!state.paste_condense_pilcrow);
    }

    #[test]
    fn emphasis_options_are_independent() {
        let mut state = make_state("", 0, None);
        state.set_emphasis(true, true, false);
        assert!(state.emphasis_bold && state.emphasis_underline && !state.emphasis_box);

        state.set_emphasis(false, true, true);
        assert!(!state.emphasis_bold && state.emphasis_underline && state.emphasis_box);
    }

    // ── Read mode ──────────────────────────────────────────────────────────

    #[test]
    fn read_mode_hides_the_sidebar_and_collapses_the_split() {
        let mut state = make_state("doc", 0, None);
        state.sidebar_visible = true;
        state.open_split();
        let tabs_before = state.tabs.len();

        state.toggle_read_mode();

        assert!(state.read_mode);
        assert!(!state.sidebar_visible);
        assert!(!state.split_view);
        // The split's tab is only un-shown, never closed.
        assert_eq!(state.tabs.len(), tabs_before, "read mode closed a tab");
    }

    #[test]
    fn leaving_read_mode_restores_the_sidebar_it_hid() {
        let mut state = make_state("doc", 0, None);
        state.sidebar_visible = true;

        state.toggle_read_mode();
        state.toggle_read_mode();

        assert!(!state.read_mode);
        assert!(state.sidebar_visible);
    }

    /// A sidebar the user had already hidden must stay hidden on exit —
    /// restoring it would be read mode turning something on that wasn't.
    #[test]
    fn leaving_read_mode_does_not_reveal_a_sidebar_that_was_already_hidden() {
        let mut state = make_state("doc", 0, None);
        state.sidebar_visible = false;

        state.toggle_read_mode();
        state.toggle_read_mode();

        assert!(!state.sidebar_visible);
    }

    // ── Split view (notes/split_view_plan.md) ──────────────────────────────

    /// The index-vs-id trap this feature is most exposed to: closing a tab
    /// positioned *before* the secondary pane's shifts every later index, and
    /// an index-based `secondary_tab_id` would silently retarget the pane.
    #[test]
    fn secondary_pane_survives_a_tab_closing_before_it() {
        let mut state = make_state("", 0, None);
        state.new_tab();
        state.new_tab();
        state.open_split(); // 4 tabs; secondary is the newest
        let secondary_id = state.secondary_tab_id.unwrap();

        state.close_tab(0);

        assert!(state.split_view, "split should survive an unrelated close");
        assert_eq!(state.secondary_tab_id, Some(secondary_id));
        let idx = state.pane_tab_index(Pane::Secondary).unwrap();
        assert_eq!(state.tabs[idx].id, secondary_id, "pane followed the wrong tab");
    }

    /// Decision 1: one document is never in two panes. Asking for the tab the
    /// secondary pane holds focuses that pane instead.
    #[test]
    fn activating_the_secondary_panes_tab_focuses_that_pane() {
        let mut state = make_state("", 0, None);
        state.open_split();
        let secondary_idx = state.pane_tab_index(Pane::Secondary).unwrap();
        state.focus_pane(Pane::Primary);

        state.set_active_tab(secondary_idx);

        assert_eq!(state.focused_pane, Pane::Secondary);
        assert_eq!(state.active_tab, secondary_idx);
    }

    /// Clicking a tab shows it in whichever pane is live — this is the only
    /// way to get an existing document into the split.
    #[test]
    fn activating_a_tab_opens_it_in_the_focused_pane() {
        let mut state = make_state("tab zero", 0, None);
        state.new_tab(); // idx 1 — the one we'll click; in neither pane
        state.new_tab(); // idx 2 — what the primary pane ends up showing
        state.open_split(); // idx 3 — secondary, and now focused
        assert_eq!(state.focused_pane, Pane::Secondary);
        let primary_before = state.pane_tab_index(Pane::Primary);

        state.set_active_tab(1);

        assert_eq!(state.focused_pane, Pane::Secondary, "click yanked focus to the other pane");
        assert_eq!(state.pane_tab_index(Pane::Secondary), Some(1));
        // ...and the primary pane kept whatever it was already showing.
        assert_eq!(state.pane_tab_index(Pane::Primary), primary_before);
    }

    /// The other half of decision 1: clicking the tab the *other* pane is
    /// already showing focuses that pane instead of duplicating it.
    #[test]
    fn activating_the_other_panes_tab_focuses_that_pane_from_either_side() {
        let mut state = make_state("", 0, None);
        state.open_split();
        let primary_idx = state.pane_tab_index(Pane::Primary).unwrap();

        // Focused pane is Secondary; click the primary's tab.
        state.set_active_tab(primary_idx);
        assert_eq!(state.focused_pane, Pane::Primary);

        // And back the other way.
        let secondary_idx = state.pane_tab_index(Pane::Secondary).unwrap();
        state.set_active_tab(secondary_idx);
        assert_eq!(state.focused_pane, Pane::Secondary);
    }

    #[test]
    fn closing_the_secondary_panes_tab_collapses_the_split() {
        let mut state = make_state("", 0, None);
        state.new_tab();
        state.open_split();
        let idx = state.pane_tab_index(Pane::Secondary).unwrap();

        state.close_tab(idx);

        assert!(!state.split_view);
        assert_eq!(state.secondary_tab_id, None);
        assert_eq!(state.focused_pane, Pane::Primary);
    }

    /// Two panes need two tabs; dropping to one has to collapse the split
    /// even when the tab closed was not the secondary pane's own.
    #[test]
    fn closing_down_to_one_tab_collapses_the_split() {
        let mut state = make_state("", 0, None);
        state.open_split(); // 2 tabs
        state.close_tab(0);

        assert!(!state.split_view);
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn open_split_is_idempotent() {
        let mut state = make_state("", 0, None);
        state.open_split();
        let tabs_after_first = state.tabs.len();
        let id = state.secondary_tab_id;

        state.open_split();

        assert_eq!(state.tabs.len(), tabs_after_first, "second open stacked a blank tab");
        assert_eq!(state.secondary_tab_id, id);
    }

    #[test]
    fn focus_pane_repoints_active_tab() {
        let mut state = make_state("", 0, None);
        state.open_split();
        let secondary_idx = state.pane_tab_index(Pane::Secondary).unwrap();

        state.focus_pane(Pane::Primary);
        assert_ne!(state.active_tab, secondary_idx);

        state.focus_pane(Pane::Secondary);
        assert_eq!(state.active_tab, secondary_idx);
    }

    /// With the split closed the secondary pane has nothing to show, and its
    /// editor must render blank rather than mirroring the primary.
    #[test]
    fn secondary_pane_has_no_tab_while_the_split_is_closed() {
        let state = make_state("hello", 0, None);
        assert_eq!(state.pane_tab_index(Pane::Secondary), None);
        assert_eq!(state.pane_content(Pane::Secondary), "");
        assert_eq!(state.pane_content(Pane::Primary), "hello");
    }

    /// The bug this exists to prevent: with focus in the secondary pane,
    /// `active_tab` names the *secondary's* document, so a primary pane that
    /// resolved through `active_tab` would paint the same text in both halves.
    #[test]
    fn the_two_panes_never_resolve_to_the_same_tab() {
        let mut state = make_state("primary text", 0, None);
        state.open_split();
        state.insert_str("secondary text");

        // Focus is in the secondary pane right after open_split.
        assert_eq!(state.focused_pane, Pane::Secondary);
        let primary = state.pane_tab_index(Pane::Primary).unwrap();
        let secondary = state.pane_tab_index(Pane::Secondary).unwrap();
        assert_ne!(primary, secondary, "both panes resolved to one tab");
        assert_eq!(state.pane_content(Pane::Primary), "primary text");
        assert_eq!(state.pane_content(Pane::Secondary), "secondary text");

        // ...and the same holds with focus back in the primary pane.
        state.focus_pane(Pane::Primary);
        assert_ne!(
            state.pane_tab_index(Pane::Primary),
            state.pane_tab_index(Pane::Secondary)
        );
        assert_eq!(state.pane_content(Pane::Primary), "primary text");
        assert_eq!(state.pane_content(Pane::Secondary), "secondary text");
    }

    /// Opening a file with the secondary pane focused must put it in *that*
    /// pane. Before `show_in_focused_pane` it moved `active_tab` while both
    /// panes kept their stored ids, so the document appeared in neither.
    #[test]
    fn opening_a_tab_with_the_secondary_pane_focused_lands_there() {
        let mut state = make_state("primary", 0, None);
        state.open_split();
        assert_eq!(state.focused_pane, Pane::Secondary);

        state.new_tab();

        let secondary = state.pane_tab_index(Pane::Secondary).unwrap();
        assert_eq!(secondary, state.active_tab, "new tab did not land in the focused pane");
        assert_ne!(state.pane_tab_index(Pane::Primary), Some(secondary));
        assert_eq!(state.pane_content(Pane::Primary), "primary");
    }

    /// The reported bug: double-clicking a file in the sidebar with the split
    /// pane focused opened a new tab that appeared in *neither* pane. The
    /// new-tab path in `open_file` still assigned `active_tab` directly and
    /// never updated the focused pane's stored id.
    #[test]
    fn opening_a_file_with_the_secondary_pane_focused_shows_it_there() {
        let dir = std::env::temp_dir().join(format!("vimbatim_split_open_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("card.docx");
        create_new_docx(&default_paragraphs(), &path).unwrap();

        let mut state = make_state("primary", 0, None);
        state.open_split();
        assert_eq!(state.focused_pane, Pane::Secondary);
        let primary_before = state.pane_tab_index(Pane::Primary);

        state.open_file(path.clone());

        let secondary = state.pane_tab_index(Pane::Secondary).expect("split collapsed");
        assert_eq!(
            state.tabs[secondary].file_path.as_deref(),
            Some(path.as_path()),
            "opened file did not land in the focused pane"
        );
        assert_eq!(state.pane_tab_index(Pane::Primary), primary_before);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Recovery reopens a document the same way, and had the same defect.
    #[test]
    fn resumed_recovery_lands_in_the_focused_pane() {
        let (mut state, _dir) = make_state_with_recovery("split-resume");
        state.open_split();
        assert_eq!(state.focused_pane, Pane::Secondary);

        state.resume_recovery();

        let secondary = state.pane_tab_index(Pane::Secondary).expect("split collapsed");
        assert_eq!(state.active_tab, secondary, "recovered tab bypassed the focused pane");
    }

    #[test]
    fn split_ratio_is_clamped_away_from_collapsing_a_pane() {
        assert_eq!(clamp_split_ratio(0.0), 0.2);
        assert_eq!(clamp_split_ratio(0.5), 0.5);
        assert_eq!(clamp_split_ratio(1.0), 0.8);
    }

    // ── pending_focus_editor (tab switch / open / close / new-tab) ──────────

    #[test]
    fn set_active_tab_requests_editor_focus() {
        // Root-cause regression test for the intermittent Enter/keyboard
        // lockout (notes/feedback.md): clicking a tab only ever called
        // `set_active_tab`, which never touched GPUI keyboard focus, so the
        // text editor's FocusHandle was left stale until the user clicked
        // back into it. `pending_focus_editor` is the flag `TextEditor::render`
        // checks-and-clears to reclaim focus once per frame.
        let mut state = make_state("hello", 0, None);
        state.tabs.push(Tab::new_empty(1));
        state.pending_focus_editor = None;

        state.set_active_tab(1);

        assert_eq!(state.pending_focus_editor, Some(Pane::Primary));
    }

    #[test]
    fn set_active_tab_out_of_range_does_not_request_focus() {
        let mut state = make_state("hello", 0, None);
        state.pending_focus_editor = None;

        state.set_active_tab(99);

        assert_eq!(state.pending_focus_editor, None);
    }

    // ── rename_tab (double-click tab rename) ────────────────────────────

    #[test]
    fn rename_tab_updates_title_by_id() {
        let mut state = make_state("hello", 0, None);
        let id = state.tabs[state.active_tab].id;

        state.rename_tab(id, "My Renamed Tab".to_string());

        assert_eq!(state.tabs[state.active_tab].title, "My Renamed Tab");
    }

    #[test]
    fn rename_tab_ignores_empty_title() {
        let mut state = make_state("hello", 0, None);
        let id = state.tabs[state.active_tab].id;
        let original = state.tabs[state.active_tab].title.clone();

        state.rename_tab(id, "".to_string());

        assert_eq!(state.tabs[state.active_tab].title, original);
    }

    #[test]
    fn new_tab_requests_editor_focus() {
        let mut state = make_state("hello", 0, None);
        state.pending_focus_editor = None;

        state.new_tab();

        assert_eq!(state.pending_focus_editor, Some(Pane::Primary));
    }

    /// The guard is in `open_file` rather than at the toolbar's picker, so it
    /// has to hold for every entry point — vim's `:e <path>` included.
    #[test]
    fn open_file_refuses_anything_that_is_not_a_docx() {
        let dir = std::env::temp_dir().join(format!("vimbatim_ext_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, "plain text").unwrap();

        let mut state = make_state("hello", 0, None);
        let tabs_before = state.tabs.len();
        state.open_file(txt);
        assert_eq!(state.tabs.len(), tabs_before, "a .txt must not open a tab");

        // Case-insensitive: Windows hands back .DOCX from the native picker.
        let upper = dir.join("doc.DOCX");
        create_new_docx(&default_paragraphs(), &upper).unwrap();
        state.open_file(upper);
        assert_eq!(state.tabs.len(), tabs_before + 1, ".DOCX must still open");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_as_writes_the_file_and_repoints_the_tab() {
        let dir = std::env::temp_dir().join(format!("vimbatim_saveas_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("renamed.docx");

        let mut state = make_state("hello world", 0, None);
        state.save_active_tab_as(dest.clone()).unwrap();

        assert!(dest.exists(), "Save As must write the file");
        // Re-pointed, not merely copied — a later plain Ctrl+S goes here too.
        assert_eq!(state.tabs[0].file_path.as_deref(), Some(dest.as_path()));
        assert_eq!(state.tabs[0].title, "renamed.docx");
        assert!(!state.tabs[0].is_modified, "a successful save clears the dirty flag");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A picker (or a user typing a name) can hand back a path with no
    /// extension — saving there would produce a file `open_file` then refuses
    /// to reopen.
    #[test]
    fn save_as_appends_docx_when_the_chosen_name_lacks_it() {
        let dir = std::env::temp_dir().join(format!("vimbatim_saveas_ext_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut state = make_state("hello", 0, None);
        state.save_active_tab_as(dir.join("no_extension")).unwrap();

        assert_eq!(state.tabs[0].title, "no_extension.docx");
        assert!(dir.join("no_extension.docx").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_file_new_tab_requests_editor_focus() {
        let dir = std::env::temp_dir().join(format!(
            "vimbatim_focus_test_{}_new",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.docx");
        create_new_docx(&default_paragraphs(), &path).unwrap();

        let mut state = make_state("hello", 0, None);
        state.pending_focus_editor = None;

        state.open_file(path);

        assert_eq!(state.pending_focus_editor, Some(Pane::Primary));
    }

    #[test]
    fn open_file_existing_tab_requests_editor_focus() {
        let dir = std::env::temp_dir().join(format!(
            "vimbatim_focus_test_{}_existing",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.docx");
        create_new_docx(&default_paragraphs(), &path).unwrap();

        let mut state = make_state("hello", 0, None);
        state.open_file(path.clone());
        state.pending_focus_editor = None; // clear what the first open set

        state.open_file(path); // already open -> switches to existing tab

        assert_eq!(state.pending_focus_editor, Some(Pane::Primary));
    }

    #[test]
    fn close_tab_requests_editor_focus() {
        let mut state = make_state("hello", 0, None);
        state.tabs.push(Tab::new_empty(1));
        state.pending_focus_editor = None;

        state.close_tab(0);

        assert_eq!(state.pending_focus_editor, Some(Pane::Primary));
    }

    #[test]
    fn close_tab_pushes_a_file_backed_tabs_path_onto_closed_tabs() {
        let mut state = make_state("hello", 0, None);
        let path = PathBuf::from("/tmp/vimbatim_reopen_test_a.docx");
        state.tabs.push(Tab::from_path(1, path.clone()));

        state.close_tab(1);

        assert_eq!(state.closed_tabs, vec![path]);
    }

    #[test]
    fn close_tab_does_not_stack_a_blank_new_tab() {
        let mut state = make_state("hello", 0, None);
        state.tabs.push(Tab::new_empty(1)); // no file_path
        state.closed_tabs.clear();

        state.close_tab(1);

        assert!(state.closed_tabs.is_empty());
    }

    #[test]
    fn reopen_closed_tab_reopens_most_recently_closed_first() {
        let mut state = make_state("hello", 0, None);
        let a = PathBuf::from("/tmp/vimbatim_reopen_test_a.docx");
        let b = PathBuf::from("/tmp/vimbatim_reopen_test_b.docx");
        state.tabs.push(Tab::from_path(1, a.clone()));
        state.tabs.push(Tab::from_path(2, b.clone()));
        state.close_tab(1); // closes a
        state.close_tab(1); // closes b (shifted down after a's removal)

        state.reopen_closed_tab();
        assert!(state.tabs.iter().any(|t| t.file_path.as_ref() == Some(&b)), "b (closed last) reopens first");

        state.reopen_closed_tab();
        assert!(state.tabs.iter().any(|t| t.file_path.as_ref() == Some(&a)), "a reopens second");

        assert!(state.closed_tabs.is_empty());
    }

    #[test]
    fn reopen_closed_tab_is_a_noop_with_nothing_closed() {
        let mut state = make_state("hello", 0, None);
        let tabs_before = state.tabs.len();

        state.reopen_closed_tab();

        assert_eq!(state.tabs.len(), tabs_before);
    }

    // ── next_tab / prev_tab (Task 9: Ctrl+Tab / Ctrl+Shift+Tab cycling) ─────

    #[test]
    fn next_tab_wraps_around() {
        let mut state = make_state("hello", 0, None);
        state.new_tab();
        state.new_tab(); // 3 tabs, active_tab at the last-created (index 2)
        state.set_active_tab(2);

        state.next_tab();

        assert_eq!(state.active_tab, 0);
    }

    #[test]
    fn prev_tab_wraps_around() {
        let mut state = make_state("hello", 0, None);
        state.new_tab();
        state.new_tab();
        state.set_active_tab(0);

        state.prev_tab();

        assert_eq!(state.active_tab, 2);
    }

    // ── pending_close (Task 6: confirm before closing a dirty tab/app) ──────

    #[test]
    fn request_close_tab_shows_confirm_when_modified() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].is_modified = true;

        state.request_close_tab(0);

        assert_eq!(state.pending_close, Some(PendingClose::Tab(0)));
        assert_eq!(state.tabs.len(), 1); // not yet closed
    }

    #[test]
    fn request_close_tab_closes_immediately_when_unmodified() {
        let mut state = make_state("hello", 0, None);
        state.new_tab();
        state.tabs[0].is_modified = false;

        state.request_close_tab(0);

        assert_eq!(state.pending_close, None);
        assert_eq!(state.tabs.len(), 1); // closed immediately
    }

    #[test]
    fn confirm_close_discard_closes_tab_without_saving() {
        let mut state = make_state("hello", 0, None);
        state.new_tab();
        state.tabs[0].is_modified = true;
        state.request_close_tab(0);

        state.confirm_close_discard();

        assert_eq!(state.pending_close, None);
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn cancel_close_leaves_tab_open() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].is_modified = true;

        state.request_close_tab(0);
        state.cancel_close();

        assert_eq!(state.pending_close, None);
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn request_close_app_shows_confirm_when_any_tab_modified() {
        let mut state = make_state("hello", 0, None);
        state.new_tab();
        state.tabs[0].is_modified = true; // the *other* (inactive) tab is dirty

        state.request_close_app();

        assert_eq!(state.pending_close, Some(PendingClose::App));
    }

    #[test]
    fn request_close_app_clears_pending_when_nothing_modified() {
        // No modified tabs: request_close_app resolves pending_close back to
        // None on its own (via confirm_close_discard) rather than leaving it
        // Some(App) — the GPUI caller reads this "None after the call" as
        // its signal to quit immediately without ever mounting the dialog.
        let mut state = make_state("hello", 0, None);

        state.request_close_app();

        assert_eq!(state.pending_close, None);
    }

    #[test]
    fn confirm_close_save_tab_clears_pending_and_closes() {
        // Tab 0 needs a real file_path here — save_tab only actually
        // persists (and confirm_close_save only actually closes) a tab that
        // has somewhere to write to. See
        // `confirm_close_save_does_not_discard_a_never_saved_tab` below for
        // the no-file_path case.
        let dir = std::env::temp_dir().join(format!(
            "vimbatim_confirm_close_save_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.docx");
        create_new_docx(&default_paragraphs(), &path).unwrap();

        let mut state = make_state("hello", 0, None);
        state.tabs[0].file_path = Some(path);
        state.new_tab();
        state.tabs[0].is_modified = true;
        state.request_close_tab(0);

        let persisted = state.confirm_close_save();

        assert!(persisted);
        assert_eq!(state.pending_close, None);
        assert_eq!(state.tabs.len(), 1);
    }

    #[test]
    fn confirm_close_save_does_not_discard_a_never_saved_tab() {
        // A tab with no file_path (a plain "New Tab" that was never saved to
        // disk) has nowhere for save_tab to persist to — this app has no
        // "Save As" flow to fall back to. Before this fix, confirm_close_save
        // ignored save_tab's no-op result and closed the tab anyway, silently
        // discarding its content exactly as Discard would have, defeating the
        // whole point of the confirmation dialog. Correct behavior: leave the
        // tab open and dirty.
        let mut state = make_state("unsaved scratch content", 0, None);
        assert!(state.tabs[0].file_path.is_none());
        state.tabs[0].is_modified = true;
        state.new_tab(); // second tab so a wrongful close_tab would be observable
        state.request_close_tab(0);

        let persisted = state.confirm_close_save();

        assert!(!persisted);
        assert_eq!(state.pending_close, None); // dialog still resolves/closes
        assert_eq!(state.tabs.len(), 2); // but the tab itself was NOT closed
        assert_eq!(state.tabs[0].content, "unsaved scratch content"); // content preserved
        assert!(state.tabs[0].is_modified); // still dirty, still needs a Save
    }

    #[test]
    fn confirm_close_save_app_clears_pending_without_closing_tabs() {
        let mut state = make_state("hello", 0, None);
        state.new_tab();
        state.tabs[0].is_modified = true;
        state.request_close_app();
        assert_eq!(state.pending_close, Some(PendingClose::App));

        let persisted = state.confirm_close_save();

        // tab 0 has no file_path, so the app-wide save can't fully persist —
        // the caller (close_confirm.rs) reads this `false` as "don't
        // cx.quit(), a dirty tab is still unsaved".
        assert!(!persisted);
        assert_eq!(state.pending_close, None);
        assert_eq!(state.tabs.len(), 2); // saving the app doesn't remove tabs
    }

    #[test]
    fn cancel_close_is_a_no_op_when_nothing_pending() {
        let mut state = make_state("hello", 0, None);
        state.cancel_close();
        assert_eq!(state.pending_close, None);
    }

    // ── clamp_sidebar_width ──────────────────────────────────────────────────

    #[test]
    fn test_clamp_sidebar_width_within_range_is_unchanged() {
        assert_eq!(clamp_sidebar_width(300.0), 300.0);
    }

    #[test]
    fn test_clamp_sidebar_width_below_min_clamps_to_min() {
        assert_eq!(clamp_sidebar_width(50.0), 180.0);
    }

    #[test]
    fn test_clamp_sidebar_width_above_max_clamps_to_max() {
        assert_eq!(clamp_sidebar_width(900.0), 480.0);
    }

    #[test]
    fn test_clamp_sidebar_width_default_is_within_range() {
        assert_eq!(clamp_sidebar_width(DEFAULT_SIDEBAR_WIDTH), DEFAULT_SIDEBAR_WIDTH);
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

    // ── Font size box (ribbon spinner) ──────────────────────────────────────

    fn sized_run(text: &str, size: u16) -> Run {
        Run { text: text.into(), size, ..Run::default() }
    }

    fn sized_para(runs: Vec<Run>) -> Paragraph {
        Paragraph { runs, ..Paragraph::default() }
    }

    #[test]
    fn test_selection_font_size_reports_a_uniform_selection() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("hello world", 32)])],
            0,
        );
        state.tabs[0].selection = Some((0, 11));
        assert_eq!(state.selection_font_size_half_points(), Some(32));
    }

    #[test]
    fn test_selection_font_size_is_none_when_sizes_are_mixed() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("big", 48), sized_run("small", 20)])],
            0,
        );
        state.tabs[0].selection = Some((0, 8));
        assert_eq!(state.selection_font_size_half_points(), None);
    }

    #[test]
    fn test_selection_font_size_reads_the_run_under_the_cursor_with_no_selection() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("abc", 48), sized_run("defgh", 20)])],
            4, // inside the second run
        );
        state.tabs[0].selection = None;
        assert_eq!(state.selection_font_size_half_points(), Some(20));
    }

    /// The old inline detector in `cycle_font_size` restarted its byte offset
    /// at every paragraph and never counted the separating newline, so on a
    /// multi-paragraph document it read the size off the wrong runs. Offsets
    /// now accumulate the way `document_ops::is_uniformly_active` does.
    #[test]
    fn test_selection_font_size_offsets_accumulate_across_paragraphs() {
        let mut state = make_state_with_paragraphs(
            vec![
                sized_para(vec![sized_run("first", 48)]),   // bytes 0..5, newline at 5
                sized_para(vec![sized_run("second", 20)]),  // bytes 6..12
            ],
            0,
        );
        // A selection wholly inside the *second* paragraph.
        state.tabs[0].selection = Some((6, 12));
        assert_eq!(state.selection_font_size_half_points(), Some(20));

        // Spanning both paragraphs is mixed.
        state.tabs[0].selection = Some((0, 12));
        assert_eq!(state.selection_font_size_half_points(), None);
    }

    /// `size == 0` means "no explicit override" — the box turns that into the
    /// configured body size, but the getter reports it verbatim.
    #[test]
    fn test_selection_font_size_reports_zero_for_an_unstyled_run() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("plain", 0)])],
            0,
        );
        state.tabs[0].selection = Some((0, 5));
        assert_eq!(state.selection_font_size_half_points(), Some(0));
    }

    /// With the caret at the very end of the text there is no character
    /// *after* it, so reading forward blanked the box. It reports the
    /// character before the caret instead — what typing there would inherit.
    #[test]
    fn test_selection_font_size_at_end_of_text_reports_the_preceding_run() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("hello", 48)])],
            5, // one past the last character
        );
        state.tabs[0].selection = None;
        assert_eq!(state.selection_font_size_half_points(), Some(48));
    }

    /// At the very start there is nothing before the caret, so it falls
    /// forward to the first character rather than reporting nothing.
    #[test]
    fn test_selection_font_size_at_start_of_text_reports_the_first_run() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("hello", 48)])],
            0,
        );
        state.tabs[0].selection = None;
        assert_eq!(state.selection_font_size_half_points(), Some(48));
    }

    /// Between two differently-sized runs the caret inherits the left one,
    /// matching Word.
    #[test]
    fn test_selection_font_size_between_runs_reports_the_left_one() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("abc", 48), sized_run("defgh", 20)])],
            3, // exactly on the boundary
        );
        state.tabs[0].selection = None;
        assert_eq!(state.selection_font_size_half_points(), Some(48));
    }

    /// An empty (zero-width) selection is a caret, not a range.
    #[test]
    fn test_selection_font_size_treats_a_collapsed_selection_as_a_caret() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("hello", 48)])],
            5,
        );
        state.tabs[0].selection = Some((5, 5));
        assert_eq!(state.selection_font_size_half_points(), Some(48));
    }

    #[test]
    fn test_set_font_size_applies_to_the_selection() {
        let mut state = make_state_with_paragraphs(
            vec![sized_para(vec![sized_run("hello", 0)])],
            0,
        );
        state.tabs[0].selection = Some((0, 5));
        state.set_font_size_half_points(36); // 18pt
        assert_eq!(state.selection_font_size_half_points(), Some(36));
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

    // ── Rich clipboard round-trip: copy_selection_runs -> encode_with_lengths
    // -> decode -> insert_str_with_runs, end-to-end, no GPUI involved ────────

    /// End-to-end against the user's reported repro: the Pocket/Hat/Block/Tag
    /// document is copied whole, pasted below itself, saved as a real .docx,
    /// and re-parsed. Guards the whole clipboard -> paragraphs -> docx chain,
    /// which is where the corruption actually surfaced (it survived a
    /// save+reopen, so a state-level assertion alone would not have caught it).
    #[test]
    fn copy_paste_card_styles_survives_a_docx_save_and_reload() {
        let card = |text: &str, heading: u8, size: u16| Paragraph {
            runs: vec![Run { text: text.into(), bold: true, size, ..Run::default() }],
            heading,
            alignment: Alignment::Center,
            unsupported_xml: None,
        };
        // Five styled lines, then the blank line the user pressed Enter to
        // reach before pasting.
        let mut state = make_state_with_paragraphs(
            vec![
                card("pocket", 1, 52),
                card("hat", 2, 44),
                card("block", 3, 32),
                card("tag", 4, 26),
                para_plain("test"),
                para_plain(""),
            ],
            0,
        );

        // Select the five styled lines (not the trailing blank) and copy
        // through the real clipboard encoding.
        let doc_len = state.tabs[0].content.len();
        let copy_end = doc_len - 1; // excludes the trailing blank paragraph
        state.tabs[0].selection = Some((0, copy_end));
        let plain = state.copy_selection().unwrap();
        let runs = state.copy_selection_runs().unwrap();
        let attrs = state.copy_selection_paragraph_attrs().unwrap();
        let meta = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
        let (runs, attrs) = crate::rich_clipboard::decode(&meta, &plain).unwrap();

        // Paste into the blank line at the end, as the user did.
        state.tabs[0].selection = None;
        state.tabs[0].cursor = doc_len;
        state.insert_str_with_runs_and_paragraphs(&plain, &runs, &attrs);

        let dir = std::env::temp_dir().join(format!("vimbatim_paste_e2e_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Paste_Test.docx");
        state.save_active_tab_as(path.clone()).unwrap();

        let (reloaded, _) = crate::docx_parser::parse_docx(&path).unwrap();
        let headings: Vec<u8> = reloaded.iter().map(|p| p.heading).collect();
        assert_eq!(
            headings,
            vec![1, 2, 3, 4, 0, 1, 2, 3, 4, 0],
            "card-style headings must survive copy -> paste -> save -> reload"
        );
        for i in [5usize, 6, 7, 8] {
            assert_eq!(reloaded[i].alignment, Alignment::Center, "alignment lost on pasted paragraph {i}");
        }
        // The pasted body line must not inherit the card styles' leftovers.
        let last = reloaded.last().unwrap();
        assert_eq!(last.runs.len(), 1, "leftover empty runs: {:?}", last.runs);
        assert!(!last.runs[0].box_format, "spurious border on the pasted body line");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Repro of the user-reported "copy/paste jumbles formatting": write
    /// Pocket/Hat/Block/Tag lines, select all, paste below.
    ///
    /// The sibling test above only ever used `heading: 0` and default
    /// alignment, which is exactly why it never caught this — card styles are
    /// the one thing that carries *paragraph-level* state.
    #[test]
    fn copy_paste_preserves_card_style_heading_and_alignment() {
        let card = |text: &str, heading: u8, size: u16| Paragraph {
            runs: vec![Run { text: text.into(), bold: true, size, ..Run::default() }],
            heading,
            alignment: Alignment::Center,
            unsupported_xml: None,
        };
        let paragraphs = vec![
            card("pocket", 1, 52),
            card("hat", 2, 44),
            card("block", 3, 32),
            card("tag", 4, 26),
            para_plain("test"),
        ];
        let mut source = make_state_with_paragraphs(paragraphs, 0);
        let doc_len = source.tabs[0].content.len();
        source.tabs[0].selection = Some((0, doc_len));

        let plain = source.copy_selection().unwrap();
        let runs = source.copy_selection_runs().unwrap();
        let attrs = source.copy_selection_paragraph_attrs().unwrap();
        let meta = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
        let (decoded, decoded_attrs) = crate::rich_clipboard::decode(&meta, &plain)
            .expect("decode rejected a multi-paragraph card-style copy");

        let mut dest = make_state("", 0, None);
        dest.insert_str_with_runs_and_paragraphs(&plain, &decoded, &decoded_attrs);

        let paras = &dest.tabs[0].paragraphs;
        assert_eq!(paras.len(), 5, "expected one paragraph per copied line");

        // Paragraph-level card-style markers must survive the paste.
        assert_eq!(
            paras.iter().map(|p| p.heading).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 0],
            "heading (Pocket/Hat/Block/Tag) lost on paste"
        );
        for (i, para) in paras[..4].iter().enumerate() {
            assert_eq!(para.alignment, Alignment::Center, "alignment lost on pasted paragraph {i}");
        }

        // No empty leftover runs: they carry stale bold/size/box_format that
        // shows up as a spurious border and wrong inherited formatting.
        for (i, para) in paras.iter().enumerate() {
            assert!(
                para.runs.iter().all(|r| !r.text.is_empty()),
                "paragraph {i} kept empty leftover runs: {:?}",
                para.runs.iter().map(|r| (&r.text, r.size, r.box_format)).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn test_multi_paragraph_copy_paste_round_trip_preserves_per_line_formatting() {
        // Regression test: runs_in_range never emitted a run for the
        // paragraph-separating '\n', so the summed run lengths came up one
        // byte short of the plain text for every multi-paragraph selection,
        // and rich_clipboard::decode (which requires an exact match) rejected
        // it outright -- silently falling back to plain-text paste and
        // losing all per-line formatting. That defeats this app's primary
        // "copy a formatted card spanning several lines" use case; only
        // single-paragraph copies worked before this fix.
        let paragraphs = vec![
            Paragraph {
                runs: vec![Run { text: "bold line".into(), bold: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            para_plain("plain line"),
            Paragraph {
                runs: vec![Run { text: "hi line".into(), highlight: true, highlight_color: "yellow".into(), ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ];
        let mut source = make_state_with_paragraphs(paragraphs, 0);
        let doc_len = source.tabs[0].content.len();
        source.tabs[0].selection = Some((0, doc_len)); // whole doc, crossing both paragraph boundaries

        let plain_text = source.copy_selection().unwrap();
        let runs = source.copy_selection_runs().unwrap();
        let attrs = source.copy_selection_paragraph_attrs().unwrap();
        let metadata = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
        let decoded = crate::rich_clipboard::decode(&metadata, &plain_text);
        assert!(decoded.is_some(), "decode() rejected a multi-paragraph copy -- the bug this test guards against");
        let (decoded_runs, _) = decoded.unwrap();

        let mut dest = make_state("", 0, None);
        dest.insert_str_with_runs(&plain_text, &decoded_runs);

        assert_eq!(dest.tabs[0].content, plain_text);
        assert_eq!(dest.tabs[0].paragraphs.len(), 3);
        assert_eq!(dest.tabs[0].paragraphs[0].runs[0].text, "bold line");
        assert!(dest.tabs[0].paragraphs[0].runs[0].bold);
        assert_eq!(dest.tabs[0].paragraphs[1].runs[0].text, "plain line");
        assert!(!dest.tabs[0].paragraphs[1].runs[0].bold);
        assert_eq!(dest.tabs[0].paragraphs[2].runs[0].text, "hi line");
        assert!(dest.tabs[0].paragraphs[2].runs[0].highlight);
        assert_eq!(dest.tabs[0].paragraphs[2].runs[0].highlight_color, "yellow");
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

    #[test]
    fn insert_str_does_not_panic_on_stale_out_of_bounds_cursor() {
        // Simulate a stale cursor left over from before an undo/redo or tab
        // swap shortened the content out from under it.
        let mut state = make_state("hi", 9999, None);
        state.insert_str("x"); // must not panic
        assert!(state.tabs[0].content.contains('x'));
    }

    #[test]
    fn insert_str_does_not_panic_on_mid_char_cursor() {
        // 'é' is 2 bytes; cursor at position 2 lands inside 'é', not on a boundary
        let mut state = make_state("héllo", 2, None);
        state.insert_str("x"); // must not panic
        assert!(state.tabs[0].content.contains('x'));
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

    #[test]
    fn test_delete_word_backward_removes_preceding_word() {
        let mut state = make_state("hello world", 11, None); // cursor at end
        state.delete_word_backward();
        // Deletes "world" back to its start (6); the space before it
        // belongs to the gap *preceding* "world", not to "world" itself,
        // so it's left behind — same as vim's `b` landing on index 6.
        assert_eq!(state.tabs[0].content, "hello ");
        assert_eq!(state.tabs[0].cursor, 6);
    }

    #[test]
    fn test_delete_word_backward_at_start_of_line_is_a_noop() {
        let mut state = make_state("hello", 0, None);
        state.delete_word_backward();
        assert_eq!(state.tabs[0].content, "hello");
        assert_eq!(state.tabs[0].cursor, 0);
    }

    #[test]
    fn test_delete_word_backward_deletes_selection_instead_when_active() {
        let mut state = make_state("hello world", 11, Some((6, 11)));
        state.delete_word_backward();
        assert_eq!(state.tabs[0].content, "hello ");
        assert!(state.tabs[0].selection.is_none());
    }

    #[test]
    fn test_delete_word_backward_across_paragraph_break_merges_lines() {
        // Minor finding from the task-8 review: Ctrl+Backspace at the start
        // of a line should walk back over the preceding newline (word_backward
        // treats '\n' as whitespace, same as any other blank gap) and merge
        // into the previous line, exactly like backspace already does.
        let mut state = make_state("hello\nworld", 11, None); // cursor at end
        state.delete_word_backward();
        assert_eq!(state.tabs[0].content, "hello\n");
        assert_eq!(state.tabs[0].cursor, 6);
    }

    #[test]
    fn test_delete_word_backward_truncates_vim_insertion_recording() {
        // Task-8 review bug: `backspace` pops one char off
        // `vim_insertion_recording` per char deleted (so vim's `.`-repeat
        // replays only what's actually still in the document), but
        // `delete_word_backward` deleted a whole word without touching the
        // recording at all — so a Ctrl+Backspace mid-insert left the
        // deleted word's text stranded in the recording buffer, and `.`
        // would incorrectly replay it too.
        let mut state = make_state("", 0, None);
        vim_key_recorded(&mut state, "i", false, None);
        state.insert_str("hello world");
        state.delete_word_backward();
        assert_eq!(state.tabs[0].content, "hello ");
        state.insert_str("there");
        state.vim_exit_to_normal();
        assert_eq!(state.tabs[0].content, "hello there");

        // Repeat the insertion at the end of the document: if the deleted
        // "world" text were still sitting in the recording, this would
        // replay "hello worldthere" instead of just "hello there".
        state.tabs[0].cursor = state.tabs[0].content.len();
        state.vim_repeat_last_change();
        assert_eq!(state.tabs[0].content, "hello therehello there");
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

    // ── select_word_at / select_line_at (double/triple-click) ──────────────

    #[test]
    fn select_word_at_selects_the_word_under_the_position() {
        let mut state = make_state("hello world foo", 0, None);
        state.select_word_at(7); // inside "world"
        assert_eq!(state.tabs[0].selection, Some((6, 11))); // "world"
        assert_eq!(state.tabs[0].cursor, 11);
    }

    #[test]
    fn select_word_at_on_punctuation_selects_just_the_punctuation_run() {
        // Matches vim `iw`'s classification: a punctuation run is its own
        // "word", distinct from the alphanumeric runs around it.
        let mut state = make_state("foo, bar", 0, None);
        state.select_word_at(3); // the ","
        assert_eq!(state.tabs[0].selection, Some((3, 4)));
    }

    #[test]
    fn select_line_at_selects_the_whole_paragraph() {
        // No blank line separates these two lines, so per `ip` semantics
        // (a paragraph is a blank-line-delimited block) they're one
        // paragraph and both get selected.
        let mut state = make_state("first line\nsecond line", 2, None);
        state.select_line_at(2); // inside "first line"
        assert_eq!(state.tabs[0].selection, Some((0, 22)));
        assert_eq!(state.tabs[0].cursor, 22);
    }

    #[test]
    fn select_line_at_stops_at_blank_line_boundary() {
        // `ip`'s range includes the line's trailing newline (its usual
        // linewise convention), so the end lands just past it rather than
        // at the last content byte.
        let mut state = make_state("first\n\nsecond", 0, None);
        state.select_line_at(0); // inside "first"
        assert_eq!(state.tabs[0].selection, Some((0, 6)));
        assert_eq!(state.tabs[0].cursor, 6);
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

    // ── content_version (uniform_list_plan.md Part 1: row-wrap cache key) ───

    #[test]
    fn test_content_version_bumps_on_insert() {
        let mut state = make_state("ab", 2, None);
        let before = state.tabs[0].content_version;
        state.insert_char('c');
        assert!(state.tabs[0].content_version > before);
    }

    #[test]
    fn test_content_version_bumps_on_every_keystroke_even_within_coalesce_window() {
        // The version must bump on *every* real edit, not just the first
        // keystroke of a coalesced burst — otherwise the row-wrap cache
        // would serve stale text for every keystroke inside the 300ms
        // window except the first. Only one undo-stack entry gets pushed
        // (coalesced), but content_version still climbs every time.
        let mut state = make_state("a", 1, None);
        let v0 = state.tabs[0].content_version;
        state.insert_char('b'); // within the coalesce window of itself, but v0 -> v1 regardless
        let v1 = state.tabs[0].content_version;
        state.insert_char('c'); // still within the window as v1's edit
        let v2 = state.tabs[0].content_version;
        assert!(v1 > v0, "version did not bump on first keystroke");
        assert!(v2 > v1, "version did not bump on second (coalesced) keystroke");
        assert_eq!(state.tabs[0].undo_stack.len(), 1, "sanity check: still just one coalesced undo entry");
    }

    #[test]
    fn test_content_version_does_not_bump_on_true_noop() {
        // Backspace at document start is a true no-op (state.rs's own
        // documented convention: no mutation, no undo push) — the cache key
        // must not churn for it either.
        let mut state = make_state("", 0, None);
        let before = state.tabs[0].content_version;
        state.backspace();
        assert_eq!(state.tabs[0].content_version, before);
    }

    #[test]
    fn test_content_version_bumps_on_undo_and_redo() {
        let mut state = make_state("ab", 2, None);
        state.insert_char('c');
        let after_insert = state.tabs[0].content_version;
        state.undo();
        assert!(state.tabs[0].content_version > after_insert, "undo did not bump version");
        let after_undo = state.tabs[0].content_version;
        state.redo();
        assert!(state.tabs[0].content_version > after_undo, "redo did not bump version");
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
    fn test_apply_formatting_to_selection_with_no_selection_is_undoable() {
        // Bug: pressing a formatting hotkey (Ctrl+B, etc.) with the cursor on
        // a character but no active selection mutated the document without
        // ever pushing an undo snapshot, so Ctrl+Z couldn't undo it. Cursor
        // at 0 formats the 'h' under it.
        let paragraphs = vec![para_plain("hello")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = None;
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert!(state.tabs[0].paragraphs[0].runs[0].bold, "formatting wasn't applied");
        state.undo();
        assert!(!state.tabs[0].paragraphs[0].runs[0].bold, "undo did not revert the no-selection formatting");
    }

    #[test]
    fn test_apply_formatting_to_selection_with_cursor_at_document_end_is_true_noop() {
        // Cursor at the very end of the document has no character under it
        // to format — this must stay a true no-op (no undo step created),
        // matching the "true no-op = don't push" convention every other
        // mutation entry point in this file already follows.
        let paragraphs = vec![para_plain("hello")];
        let mut state = make_state_with_paragraphs(paragraphs, 5);
        state.tabs[0].selection = None;
        let undo_depth_before = state.tabs[0].undo_stack.len();
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        assert_eq!(state.tabs[0].undo_stack.len(), undo_depth_before);
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

    // ── clear_formatting: route to selection vs. current line ──────────────

    #[test]
    fn clear_formatting_spans_a_multi_paragraph_selection() {
        // Bug: ClearFormattingAction always called apply_formatting_to_line,
        // which only ever clears the cursor's own line — a selection
        // spanning multiple paragraphs left the other paragraphs bold.
        let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let content_len = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, content_len));
        state.apply_formatting_to_selection(FormatOp::Bold(true));
        for para in &state.tabs[0].paragraphs {
            for run in &para.runs {
                assert!(run.bold, "expected bold applied across every paragraph in the selection");
            }
        }

        state.tabs[0].selection = Some((0, content_len));
        state.clear_formatting();

        for para in &state.tabs[0].paragraphs {
            for run in &para.runs {
                assert!(!run.bold, "expected bold cleared across every paragraph in the selection");
            }
        }
    }

    #[test]
    fn clear_formatting_falls_back_to_current_line_with_no_selection() {
        let paragraphs = vec![para_plain("one"), para_plain("two")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = None;
        state.tabs[0].cursor = 5; // inside "two"
        state.clear_formatting(); // should not panic, should behave like today's single-line clear
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

    // ── closed_beta_plan.md §0: settings.conf / working_directory resolve
    // against the executable's location, not CWD ─────────────────────────────

    #[test]
    fn test_settings_conf_path_resolves_under_the_user_data_dir() {
        let path = settings_conf_path();
        assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("settings.conf")));
        // Deliberately NOT next to the executable: a packaged macOS .app is
        // read-only under Gatekeeper translocation, and writing inside one
        // breaks its code signature. Must be the same writable per-user
        // directory crash.log and the recovery snapshots already use.
        assert_eq!(path.parent(), Some(crate::recovery::app_data_dir().as_path()));
    }

    #[test]
    fn test_crash_log_path_resolves_under_a_fixed_dir_named_for_the_os() {
        let path = crash_log_path();
        assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("crash.log")));
        let parent_name = path.parent().unwrap().file_name().unwrap();
        if cfg!(target_os = "windows") {
            assert_eq!(parent_name, "vimbatim");
        } else {
            assert_eq!(parent_name, ".vimbatim");
        }
        assert!(path.is_absolute());
    }

    #[test]
    fn test_default_working_directory_is_not_bare_cwd_dot() {
        // Before this fix, working_directory always came from
        // std::env::current_dir() — a double-clicked packaged .app/.exe has
        // no guaranteed CWD (e.g. macOS Finder launches at "/"), so this
        // guards against silently regressing back to that.
        let dir = default_working_directory();
        assert_ne!(dir, PathBuf::from("."));
        assert!(dir.is_absolute());
    }

    // ── working_directory / expanded_dirs persistence (Task 4) ──────────────

    #[test]
    fn working_directory_round_trips_through_settings_conf() {
        let dir = temp_test_dir("working_directory_round_trip");
        let conf_path = dir.join("settings.conf");
        std::fs::write(&conf_path, "").unwrap();
        let target = PathBuf::from("/some/nested/dir");
        save_working_directory(&conf_path, &target).unwrap();
        assert_eq!(load_working_directory(&conf_path), Some(target));
    }

    #[test]
    fn expanded_dirs_round_trip_through_settings_conf() {
        let dir = temp_test_dir("expanded_dirs_round_trip");
        let conf_path = dir.join("settings.conf");
        std::fs::write(&conf_path, "").unwrap();
        let dirs = vec![PathBuf::from("/a/b"), PathBuf::from("/a/c")];
        save_expanded_dirs(&conf_path, &dirs).unwrap();
        assert_eq!(load_expanded_dirs(&conf_path), dirs);
    }

    #[test]
    fn load_working_directory_returns_none_when_key_missing() {
        let dir = temp_test_dir("working_directory_missing_key");
        let conf_path = dir.join("settings.conf");
        std::fs::write(&conf_path, "theme=dark\n").unwrap();
        assert_eq!(load_working_directory(&conf_path), None);
    }

    #[test]
    fn load_expanded_dirs_returns_empty_when_key_missing() {
        let dir = temp_test_dir("expanded_dirs_missing_key");
        let conf_path = dir.join("settings.conf");
        std::fs::write(&conf_path, "theme=dark\n").unwrap();
        assert_eq!(load_expanded_dirs(&conf_path), Vec::<PathBuf>::new());
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

    // ── undo/redo byte-budget cap (performance_plan.md's "undo/redo stack
    // memory" finding) ──────────────────────────────────────────────────────

    #[test]
    fn test_snapshot_byte_estimate_sums_content_and_run_strings() {
        let content = "hello world"; // 11 bytes
        let paragraphs = vec![Paragraph {
            runs: vec![Run {
                text: "hello world".into(), // 11 bytes, counted again (runs are the source of truth for formatted text)
                highlight_color: "yellow".into(), // 6 bytes
                font: Some("Arial".into()), // 5 bytes
                color: Some("FF0000".into()), // 6 bytes
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        assert_eq!(snapshot_byte_estimate(content, &paragraphs), 11 + 11 + 6 + 5 + 6);
    }

    #[test]
    fn test_undo_stack_cap_full_for_small_snapshots() {
        assert_eq!(undo_stack_cap_for_snapshot_size(100), UNDO_STACK_CAP);
    }

    #[test]
    fn test_undo_stack_cap_shrinks_for_large_snapshots() {
        // 1MB snapshot: 100_000_000 / 1_000_000 == 100, well under UNDO_STACK_CAP.
        assert_eq!(undo_stack_cap_for_snapshot_size(1_000_000), 100);
    }

    #[test]
    fn test_undo_stack_cap_never_below_minimum() {
        // A snapshot bigger than the whole budget would compute to 0 without
        // the floor — must still keep at least UNDO_STACK_MIN_CAP levels.
        assert_eq!(undo_stack_cap_for_snapshot_size(UNDO_STACK_BYTE_BUDGET * 10), UNDO_STACK_MIN_CAP);
    }

    #[test]
    fn test_undo_stack_shrinks_below_200_for_large_document() {
        // Proves push_undo_snapshot actually applies the size-aware cap
        // (not just that the pure function exists): a 1MB document's
        // snapshot size caps it well below the usual 200. Computed from
        // the actual final content/paragraphs rather than hardcoded, since
        // formatting-sync (insert_char keeps `paragraphs` textually in
        // step with `content`) means the snapshot is larger than
        // `content.len()` alone.
        let big_content = "a".repeat(1_000_000);
        let mut state = make_state(&big_content, big_content.len(), None);
        for _ in 0..150 {
            state.insert_char('x');
            break_coalesce_window(&mut state);
        }
        let tab = &state.tabs[0];
        let expected_cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(&tab.content, &tab.paragraphs));
        assert!(expected_cap < 200);
        assert_eq!(tab.undo_stack.len(), expected_cap);
    }

    #[test]
    fn test_redo_stack_shrinks_below_200_for_large_document() {
        // Undoing repeatedly without any new edit must cap redo_stack the
        // same way push_undo_snapshot caps undo_stack — otherwise a user
        // holding Ctrl+Z on a huge document could still blow past the byte
        // budget via redo_stack alone.
        let big_content = "a".repeat(1_000_000);
        let mut state = make_state(&big_content, big_content.len(), None);
        for _ in 0..150 {
            state.insert_char('x');
            break_coalesce_window(&mut state);
        }
        for _ in 0..150 {
            state.undo();
        }
        let tab = &state.tabs[0];
        let expected_cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(&tab.content, &tab.paragraphs));
        assert!(expected_cap < 200);
        assert_eq!(tab.redo_stack.len(), expected_cap);
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
    fn delete_forward_removes_the_character_after_the_cursor() {
        let mut state = make_state("abc", 1, None);
        state.delete_forward();
        assert_eq!(state.tabs[0].content, "ac");
        assert_eq!(state.tabs[0].cursor, 1, "cursor doesn't move for a forward delete");
    }

    #[test]
    fn delete_forward_deletes_the_selection_when_one_is_active() {
        let mut state = make_state("hello world", 5, Some((0, 5)));
        state.delete_forward();
        assert_eq!(state.tabs[0].content, " world");
    }

    #[test]
    fn delete_forward_is_a_no_op_at_document_end() {
        let mut state = make_state("abc", 3, None);
        state.delete_forward();
        assert_eq!(state.tabs[0].content, "abc");
        assert!(state.tabs[0].undo_stack.is_empty());
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

    // ── bare `u` = real vim Undo (checklist: previously unwired) ──────────────

    #[test]
    fn test_bare_u_undoes_the_last_edit() {
        let mut state = make_state("abc", 1, None);
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("z", false, None); // "abc" -> "azc"
        assert_eq!(state.tabs[0].content, "azc");
        state.handle_vim_key("u", false, None);
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_bare_u_is_a_noop_with_nothing_to_undo() {
        let mut state = make_state("abc", 1, None);
        assert!(state.handle_vim_key("u", false, None));
        assert_eq!(state.tabs[0].content, "abc");
    }

    #[test]
    fn test_shifted_u_is_still_unbound() {
        // Real vim's `U` ("undo whole line") is explicitly out of scope —
        // shifted U must not accidentally alias to plain undo.
        let mut state = make_state("abc", 1, None);
        state.handle_vim_key("r", false, None);
        state.handle_vim_key("z", false, None); // "abc" -> "azc"
        state.handle_vim_key("u", true, None);
        assert_eq!(state.tabs[0].content, "azc", "shift+u must not undo");
    }

    // ── Vim-keybind checklist item: reserved-key registry ─────────────────────
    // (`is_vim_reserved_normal_key`) — the parity test below is the actual
    // safety net; these spot-checks pin the specific asymmetric cases (m/M,
    // k/K, and the deliberately-unclaimed shifted D/Y/C/U/K/Q) that a purely
    // exhaustive test would prove but not clearly document as intentional.

    #[test]
    fn test_z_and_lowercase_m_are_unreserved() {
        assert!(!is_vim_reserved_normal_key("z", false, None));
        assert!(!is_vim_reserved_normal_key("z", true, None));
        assert!(!is_vim_reserved_normal_key("m", false, None));
    }

    #[test]
    fn test_uppercase_m_and_k_are_reserved_despite_lowercase_being_free() {
        assert!(is_vim_reserved_normal_key("m", true, None), "M is the visual screen-jump");
        assert!(!is_vim_reserved_normal_key("k", false, None), "bare k is free of a built-in Normal-mode meaning at this layer");
        assert!(is_vim_reserved_normal_key("k", true, None), "K is reserved (H/M/L jump family)");
    }

    #[test]
    fn test_shifted_dycuqk_are_deliberately_unclaimed() {
        for key in ["d", "y", "c", "u", "q"] {
            assert!(!is_vim_reserved_normal_key(key, true, None), "shift+{key} should be free");
        }
    }

    /// One frozen snapshot of every field a keystroke could observably
    /// change, used by the exhaustive parity test below to prove a
    /// non-reserved key is a true no-op, not just "didn't crash."
    fn vim_dispatch_snapshot(state: &AppState) -> String {
        let tab = &state.tabs[0];
        format!(
            "{:?}|{}|{:?}|{}|{:?}|{:?}|{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
            tab.cursor, tab.content, tab.selection, tab.vim_command_buf,
            tab.vim_mode, tab.vim_pending_operator, tab.vim_command_line,
            tab.vim_command_error, tab.vim_pending_register_select,
            tab.vim_selected_register, tab.vim_pending_replace,
            tab.vim_pending_text_object_prefix, tab.last_find,
            state.registers,
        )
    }

    /// The registry's real safety net: for every ASCII letter/digit key this
    /// app's vim-keybind first-key check (`is_vim_reserved_normal_key`) does
    /// *not* claim, replaying it through a fresh, otherwise-idle Normal-mode
    /// `AppState` must produce *zero* observable change. If this ever fails,
    /// either a real vim command was added without updating the reserved-key
    /// list (fix the list), or the list over-claims a key vim doesn't
    /// actually use (fine to leave reserved, but worth knowing).
    ///
    /// Scoped to letters + digits with `key_char: None` — the exact
    /// representation the new vim-keybind dispatcher actually receives
    /// keystrokes in (see `text_editor.rs`), and the only keyspace the
    /// z-leader system's sequences are ever built from. Symbol keys are
    /// covered by `is_vim_reserved_normal_key`'s own reuse of
    /// `matches_shifted_symbol` (already exercised by every existing vim
    /// test that types a symbol), not re-proven exhaustively here.
    #[test]
    fn test_every_non_reserved_letter_or_digit_is_a_true_vim_noop() {
        let mut candidates: Vec<char> = ('a'..='z').collect();
        candidates.extend('0'..='9');

        for key_char in candidates {
            let key = key_char.to_string();
            for shift in [false, true] {
                if is_vim_reserved_normal_key(&key, shift, None) {
                    continue;
                }
                let mut state = make_state("hello world", 5, None);
                let before = vim_dispatch_snapshot(&state);
                state.handle_vim_key(&key, shift, None);
                let after = vim_dispatch_snapshot(&state);
                assert_eq!(
                    before, after,
                    "key {key:?} (shift={shift}) is claimed to be unreserved but changed observable state — reserve it in is_vim_reserved_normal_key"
                );
            }
        }
    }

    // ── Vim-keybind runtime dispatch (checklist: Settings -> Vim Mode) ────────

    #[test]
    fn test_z_then_s_fires_save_via_pending_vim_action() {
        // Exercises the real default table (`VimKeybinds::defaults()`,
        // `make_state`'s vim_keybinds), not a hand-inserted binding — this
        // is the exact sequence a user gets out of the box.
        let mut state = make_state("hello", 0, None);
        assert!(state.handle_vim_key("z", false, None));
        assert!(!state.tabs[0].vim_keybind_seq.is_empty(), "z should start buffering (it's a prefix of zs)");
        assert!(state.handle_vim_key("s", false, None));
        assert_eq!(state.take_pending_vim_action(), Some(crate::keybinds::KeybindAction::Save));
        assert!(state.tabs[0].vim_keybind_seq.is_empty(), "buffer must clear once resolved");
    }

    #[test]
    fn test_d_then_z_abandons_the_pending_operator_instead_of_starting_a_sequence() {
        // The exact interaction the plan flagged as the highest-risk case:
        // `d` starts a real pending delete operator; `z` is not a valid
        // motion, so real vim's own rule ("an invalid motion cancels the
        // operator") must still apply — `z` must NOT be hijacked into
        // starting a vim-keybind sequence instead.
        let mut state = make_state("hello world", 0, None);
        assert!(state.handle_vim_key("d", false, None));
        assert_eq!(state.tabs[0].vim_pending_operator, Some('d'));
        assert!(state.handle_vim_key("z", false, None));
        assert_eq!(state.tabs[0].vim_pending_operator, None, "invalid motion must cancel the pending operator");
        assert_eq!(state.tabs[0].vim_keybind_seq, "", "z must not have started a vim-keybind sequence here");
        assert_eq!(state.tabs[0].content, "hello world", "nothing should have been deleted");
        assert_eq!(state.take_pending_vim_action(), None);
    }

    #[test]
    fn test_z_then_d_fires_cut_not_a_real_delete_operator() {
        // The mirror image: once `z` has already started a sequence, `d`
        // (which would normally start a delete operator) must be consumed
        // as the sequence's second key instead — `zd` is Cut in the default
        // table.
        let mut state = make_state("hello world", 0, None);
        assert!(state.handle_vim_key("z", false, None));
        assert!(state.handle_vim_key("d", false, None));
        assert_eq!(state.take_pending_vim_action(), Some(crate::keybinds::KeybindAction::Cut));
        assert_eq!(state.tabs[0].vim_pending_operator, None, "d must not also have started a real delete operator");
        assert_eq!(state.tabs[0].content, "hello world", "no direct deletion — Cut fires via dispatch_action, out of this function's reach");
    }

    #[test]
    fn test_z_then_escape_clears_the_sequence_with_no_side_effects() {
        let mut state = make_state("hello", 0, None);
        assert!(state.handle_vim_key("z", false, None));
        assert!(state.handle_vim_key("escape", false, None));
        assert_eq!(state.tabs[0].vim_keybind_seq, "");
        assert_eq!(state.take_pending_vim_action(), None);
        assert_eq!(state.tabs[0].content, "hello");
    }

    #[test]
    fn test_z_then_an_unbound_key_resets_silently() {
        let mut state = make_state("hello", 0, None);
        assert!(state.handle_vim_key("z", false, None));
        // "j" isn't the second key of any default binding.
        assert!(state.handle_vim_key("j", false, None));
        assert_eq!(state.tabs[0].vim_keybind_seq, "", "an unbound continuation must reset, not stay pending forever");
        assert_eq!(state.take_pending_vim_action(), None);
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

    /// The escape hatch back to real vim's original (paragraph-wide)
    /// `$`/`0`/`^` now that the bare keys resolve to the current visual row
    /// instead (`text_editor.rs`'s interception, ahead of
    /// `resolve_vim_motion`) — same targets `resolve_vim_motion`'s own
    /// non-`g` `$`/`0`/`^` arms already compute, just reached via a
    /// pending `g` instead.
    #[test]
    fn test_resolve_vim_motion_g_dollar_g_zero_g_caret() {
        let mut state = make_state("  hi", 2, None);
        state.handle_vim_key("g", false, None); // pending
        let g_dollar = state.resolve_vim_motion("4", true, Some("$"));
        assert_eq!(g_dollar, MotionResolution::Resolved { target: 4, kind: MotionKind::InclusiveChar });

        let mut state2 = make_state("  hi", 2, None);
        state2.handle_vim_key("g", false, None);
        let g_zero = state2.resolve_vim_motion("0", false, None);
        assert_eq!(g_zero, MotionResolution::Resolved { target: 0, kind: MotionKind::ExclusiveChar });

        let mut state3 = make_state("  hi", 3, None);
        state3.handle_vim_key("g", false, None);
        let g_caret = state3.resolve_vim_motion("6", true, Some("^"));
        assert_eq!(g_caret, MotionResolution::Resolved { target: 2, kind: MotionKind::ExclusiveChar });
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

    // ── Zoom (found_bugs.md: Ctrl+=/Ctrl+-/Ctrl+0, rebuilt from scratch) ────

    #[test]
    fn test_zoom_in_increases_by_step() {
        let mut state = make_state("", 0, None);
        state.zoom_in();
        assert!((state.zoom - 1.1).abs() < f32::EPSILON);
    }

    #[test]
    fn test_zoom_out_decreases_by_step() {
        let mut state = make_state("", 0, None);
        state.zoom_out();
        assert!((state.zoom - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_zoom_in_clamps_at_max() {
        let mut state = make_state("", 0, None);
        state.zoom = AppState::ZOOM_MAX;
        state.zoom_in();
        assert_eq!(state.zoom, AppState::ZOOM_MAX);
    }

    #[test]
    fn test_zoom_out_clamps_at_min() {
        let mut state = make_state("", 0, None);
        state.zoom = AppState::ZOOM_MIN;
        state.zoom_out();
        assert_eq!(state.zoom, AppState::ZOOM_MIN);
    }

    #[test]
    fn test_zoom_reset_returns_to_100_percent() {
        let mut state = make_state("", 0, None);
        state.zoom = 1.8;
        state.zoom_reset();
        assert_eq!(state.zoom, 1.0);
    }

    #[test]
    fn test_apply_line_alignment_center_sets_current_line() {
        let mut state = make_state("hello world", 0, None);
        state.apply_line_alignment(Alignment::Center);

        assert_eq!(state.tabs[0].paragraphs[0].alignment, Alignment::Center);
    }

    #[test]
    fn test_apply_line_alignment_left_sets_current_line() {
        // Start centered, then switch back to left — the two buttons
        // should behave as a mutually exclusive pair, not independent
        // on/off toggles.
        let mut state = make_state("hello world", 0, None);
        state.apply_line_alignment(Alignment::Center);
        state.apply_line_alignment(Alignment::Left);

        assert_eq!(state.tabs[0].paragraphs[0].alignment, Alignment::Left);
    }

    #[test]
    fn test_apply_line_alignment_only_affects_current_line_not_whole_document() {
        let paragraphs = vec![
            Paragraph { runs: vec![run_plain("first line")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("second line")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.apply_line_alignment(Alignment::Center);

        assert_eq!(state.tabs[0].paragraphs[0].alignment, Alignment::Center);
        assert_eq!(state.tabs[0].paragraphs[1].alignment, Alignment::Left);
    }

    #[test]
    fn test_apply_line_alignment_targets_line_under_cursor_not_first_line() {
        let paragraphs = vec![
            Paragraph { runs: vec![run_plain("first line")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
            Paragraph { runs: vec![run_plain("second line")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        let cursor = "first line\n".len(); // start of "second line"
        let mut state = make_state_with_paragraphs(paragraphs, cursor);
        state.apply_line_alignment(Alignment::Center);

        assert_eq!(state.tabs[0].paragraphs[0].alignment, Alignment::Left);
        assert_eq!(state.tabs[0].paragraphs[1].alignment, Alignment::Center);
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
    fn test_apply_card_style_pocket_on_empty_line_then_typed_text_is_boxed_and_centered() {
        // Reported bug repro: press the Pocket style FIRST on a blank
        // line/tab, THEN type the card's text (the natural authoring order,
        // vs. the already-covered "type first, then style" case above).
        let mut state = make_state("", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        for ch in "hello".chars() {
            state.insert_char(ch);
        }

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.alignment, Alignment::Center);
        assert!(para.runs.iter().all(|r| r.bold), "bold lost: {:?}", para.runs);
        assert!(para.runs.iter().all(|r| r.size == 52), "size lost: {:?}", para.runs);
        assert!(para.runs.iter().all(|r| r.box_format), "box lost: {:?}", para.runs);
    }

    #[test]
    fn test_clear_formatting_resets_pocket_heading_alignment_and_size() {
        // found_bugs.md: "Clear Formatting failing to remove all
        // formatting" — clicking Clear on a Pocket-styled line left it
        // visually still boxed/centered/oversized, because the run-level
        // ClearAll never reset the paragraph-level heading/alignment fields
        // apply_card_style also sets.
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        state.normal_text_size_half_points = 22; // 11pt, settings.conf's default

        let default_size = state.normal_text_size_half_points;
        state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.heading, 0, "heading not cleared");
        assert_eq!(para.alignment, Alignment::Left, "not left-aligned");
        assert!(para.runs.iter().all(|r| !r.bold), "bold not cleared");
        assert!(para.runs.iter().all(|r| !r.box_format), "box not cleared");
        assert!(para.runs.iter().all(|r| r.size == 22), "size not reset to normal_text_size");
    }

    #[test]
    fn test_clear_formatting_via_selection_resets_pocket_heading_and_alignment() {
        // Same bug as test_clear_formatting_resets_pocket_heading_alignment_
        // and_size, but through the SELECTION path (clear_formatting() with
        // tab.selection.is_some()), which routes to
        // apply_formatting_to_selection instead of apply_formatting_to_line.
        // That branch only ever called document_ops::apply_formatting for
        // run-level fields (bold/size/box) and never reset the paragraph-
        // level heading/alignment apply_card_style also sets — so clicking
        // Clear Formatting with an ordinary, single-paragraph selection
        // inside a Pocket-styled line left it still boxed/centered.
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        state.normal_text_size_half_points = 22; // 11pt, settings.conf's default

        // Selection entirely inside the one (Pocket) paragraph.
        state.tabs[0].selection = Some((0, 5));
        state.clear_formatting();

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.heading, 0, "heading not cleared via selection path");
        assert_eq!(para.alignment, Alignment::Left, "not left-aligned via selection path");
        assert!(para.runs.iter().all(|r| !r.bold), "bold not cleared via selection path");
        assert!(para.runs.iter().all(|r| !r.box_format), "box not cleared via selection path");
        assert_eq!(state.tabs[0].pending_format, None, "stale pending_format should be cleared via selection path too");
    }

    #[test]
    fn test_clear_formatting_on_empty_line_clears_stale_pending_format() {
        // Repro: Pocket (F4) on an *empty* line, then Clear Formatting
        // (F12) on that same still-empty line, then type. The newly typed
        // text kept getting boxed, because ClearAll wasn't in
        // apply_formatting_to_line's pending-format-arming match — whatever
        // card-style op (Box(true), the last of apply_card_style's
        // Bold+FontSize+Box sequence) armed `pending_format` earlier was
        // never cleared, so it kept force-applying to every character
        // typed afterward (see insert_char's own doc comment: a pending
        // format applies "to every character typed... not just this one").
        // The existing Clear Formatting tests above don't catch this since
        // they start from a non-empty line, where the pending-format-arming
        // branch (gated on `is_line_empty`) never even runs.
        let mut state = make_state("", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        let default_size = state.normal_text_size_half_points;
        state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

        assert_eq!(state.tabs[0].pending_format, None, "stale pending_format should be cleared by Clear Formatting");

        state.insert_char('a');
        let para = &state.tabs[0].paragraphs[0];
        assert!(para.runs.iter().all(|r| !r.box_format), "newly typed text should not inherit the cleared Pocket box");
    }

    #[test]
    fn test_clear_formatting_on_empty_line_does_not_leak_box_across_newline() {
        // Second half of the same repro: without the fix, the stale
        // pending_format kept applying on *every* keystroke, including
        // across an Enter — so a new paragraph created after typing on the
        // "cleared" line still ended up boxed too.
        let mut state = make_state("", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        let default_size = state.normal_text_size_half_points;
        state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

        state.insert_char('a');
        state.insert_char('\n');
        state.insert_char('b');

        assert!(state.tabs[0].paragraphs.iter().all(|p| p.runs.iter().all(|r| !r.box_format)),
            "no paragraph should carry the cleared Pocket box after typing across a newline: {:?}",
            state.tabs[0].paragraphs);
    }

    #[test]
    fn test_pocket_on_empty_line_does_not_leave_pending_format_after_enter() {
        // Broader repro (no Clear Formatting involved this time): Pocket
        // (F4) an empty line, press Enter, keep typing. The new paragraph
        // correctly lost bold/size/heading/alignment — split_paragraph_at
        // (document_ops.rs) already reverts all of that for a
        // heading-marked split — but still ended up boxed, because
        // apply_card_style's internal apply_formatting_to_line(Box(true))
        // call left `pending_format` armed indefinitely (nothing but an
        // explicit re-toggle or Clear Formatting ever cleared it). That
        // stale pending format then kept re-applying Box(true) to
        // whatever got typed next — including the freshly-split,
        // already-correctly-reset tail run — and would keep doing so on
        // every subsequent line too.
        //
        // The run is already seeded directly for the very next keystroke
        // (see apply_formatting_to_line's own `is_line_empty` seeding
        // above), so arming `pending_format` here serves no purpose for
        // apply_card_style and should not happen at all.
        let mut state = make_state("", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        assert_eq!(state.tabs[0].pending_format, None, "apply_card_style should not leave a sticky pending format armed");

        state.insert_char('a');
        state.insert_char('\n');
        state.insert_char('b');

        // Paragraph 0 ("a") is legitimately still a real Pocket line — it
        // should keep its box. Only paragraph 1 ("b", created by the
        // Enter split) should have reverted to plain.
        let paragraphs = &state.tabs[0].paragraphs;
        assert!(paragraphs[0].runs.iter().all(|r| r.box_format), "the original Pocket line should keep its box: {:?}", paragraphs);
        assert!(paragraphs[1].runs.iter().all(|r| !r.box_format), "box should not leak onto the new paragraph after Enter: {:?}", paragraphs);
    }

    #[test]
    fn test_backspace_through_pocket_line_and_trailing_newline_clears_all_formatting() {
        // Reported bug: Pocket a line, press Enter (new empty plain line
        // below it), then backspace repeatedly to erase the new line AND
        // all of the pocket text. Box/center-align visibly disappeared
        // already, but bold/font size stuck around — because nothing ever
        // reset the surviving paragraph's `heading` (text_editor.rs applies
        // a heading-driven bold+oversized font at the paragraph level,
        // independent of the run's own now-cleared bold/size) once the
        // paragraph's actual pocket-formatted text was fully deleted.
        let mut state = make_state("", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        state.insert_char('a');
        state.insert_char('\n');
        // Backspace away the new empty line, then the pocket text itself.
        state.backspace(); // removes the newline, merges back into the pocket paragraph
        state.backspace(); // removes "a"

        assert_eq!(state.tabs[0].paragraphs.len(), 1);
        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.heading, 0, "heading not cleared once pocket text is fully deleted: {:?}", para);
        assert_eq!(para.alignment, Alignment::Left, "not left-aligned: {:?}", para);
        assert!(para.runs.iter().all(|r| !r.bold), "bold not cleared: {:?}", para);
        assert!(para.runs.iter().all(|r| !r.box_format), "box not cleared: {:?}", para);
        assert!(para.runs.iter().all(|r| r.size == 0), "size not cleared: {:?}", para);
    }

    #[test]
    fn test_backspace_merging_empty_line_into_pocket_line_keeps_center_alignment() {
        // Narrower case: only ONE backspace (undoing the Enter, no pocket
        // text deleted yet) should leave the pocket paragraph exactly as it
        // was — still centered — not reset alignment just because a merge
        // across paragraphs happened.
        let mut state = make_state("", 0, None);
        state.apply_card_style(CardStyleKind::Pocket);
        state.insert_char('a');
        state.insert_char('\n');
        state.backspace();

        assert_eq!(state.tabs[0].paragraphs.len(), 1);
        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.alignment, Alignment::Center, "pocket line's center alignment should survive merging back an empty trailing line: {:?}", para);
        assert_eq!(para.heading, 1);
        assert!(para.runs.iter().all(|r| r.box_format));
    }

    #[test]
    fn test_clear_formatting_on_hat_line_removes_double_underline_and_heading() {
        let mut state = make_state("hello world", 0, None);
        state.apply_card_style(CardStyleKind::Hat);

        let default_size = state.normal_text_size_half_points;
        state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

        let para = &state.tabs[0].paragraphs[0];
        assert_eq!(para.heading, 0);
        assert_eq!(para.alignment, Alignment::Left);
        assert!(para.runs.iter().all(|r| !r.double_underline));
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
    fn test_apply_card_style_pocket_block_tag_use_configured_sizes_not_hardcoded() {
        // Regression: pocket_size/block_size/tag_size are now read from
        // settings.conf (AppState::pocket_size_half_points etc.) rather than
        // CardStyleKind::font_size()'s fixed table — a changed setting must
        // actually take effect the next time the style is applied.
        let mut state = make_state("hello world", 0, None);
        state.pocket_size_half_points = 60;
        state.block_size_half_points = 40;
        state.tag_size_half_points = 20;

        state.apply_card_style(CardStyleKind::Pocket);
        assert!(state.tabs[0].paragraphs[0].runs.iter().all(|r| r.size == 60));

        state.apply_card_style(CardStyleKind::Block);
        assert!(state.tabs[0].paragraphs[0].runs.iter().all(|r| r.size == 40));

        state.apply_card_style(CardStyleKind::Tag);
        assert!(state.tabs[0].paragraphs[0].runs.iter().all(|r| r.size == 20));
    }

    #[test]
    fn test_apply_cite_style_applies_bold_and_configured_size_to_selection() {
        let paragraphs = vec![para_plain("hello")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 5));
        state.cite_size_half_points = 30;
        state.apply_cite_style();

        let para = &state.tabs[0].paragraphs[0];
        assert!(para.runs.iter().all(|r| r.bold));
        assert!(para.runs.iter().all(|r| r.size == 30));
    }

    #[test]
    fn test_condense_selection_replaces_newlines_with_spaces() {
        let paragraphs = vec![para_plain("one"), para_plain("two")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));
        state.condense_selection();
        // Reads exactly like "one two" — the zero-width space renders as
        // nothing — but the marker is real text, which is what makes
        // uncondense_selection able to find it.
        assert_eq!(state.tabs[0].content, "one\u{200B} two");
    }

    #[test]
    fn test_uncondense_reverses_a_plain_condense() {
        let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));
        state.condense_selection();
        state.tabs[0].selection = Some((0, state.tabs[0].content.len()));
        state.uncondense_selection();
        assert_eq!(state.tabs[0].content, "one\ntwo\nthree");
    }

    #[test]
    fn test_uncondense_reverses_a_pilcrow_condense() {
        let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));
        state.condense_with_pilcrows();
        state.tabs[0].selection = Some((0, state.tabs[0].content.len()));
        state.uncondense_selection();
        assert_eq!(state.tabs[0].content, "one\ntwo\nthree");
    }

    #[test]
    fn test_uncondense_is_a_no_op_without_either_marker() {
        let mut state = make_state("one two", 0, None);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));
        state.uncondense_selection();
        assert_eq!(state.tabs[0].content, "one two");
    }

    #[test]
    fn test_condense_with_pilcrows_marks_each_break() {
        let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));

        state.condense_with_pilcrows();

        assert_eq!(state.tabs[0].content, "one¶two¶three");
        assert_eq!(state.tabs[0].paragraphs.len(), 1, "should be one paragraph now");
    }

    /// The pilcrow variant must keep per-character formatting exactly as the
    /// plain one does — they share a core, and this pins that they stay shared.
    #[test]
    fn test_condense_with_pilcrows_preserves_run_formatting() {
        let paragraphs = vec![
            Paragraph {
                runs: vec![Run { text: "bold".into(), bold: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            Paragraph {
                runs: vec![run_plain("plain")],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));

        state.condense_with_pilcrows();

        assert_eq!(state.tabs[0].content, "bold¶plain");
        let runs = &state.tabs[0].paragraphs[0].runs;
        assert!(runs.iter().find(|r| r.text.contains("bold")).unwrap().bold);
        assert!(!runs.iter().find(|r| r.text.contains("plain")).unwrap().bold);
    }

    // ── Doc Menu cleanup commands ───────────────────────────────────────────

    #[test]
    fn remove_emphasis_strips_bold_from_unstyled_runs_only() {
        let paragraphs = vec![
            para_plain("plain"),
            Paragraph {
                runs: vec![Run { text: "bold".into(), bold: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            tag_para("a tag"),
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        // Default settings.conf: emphasis_bold=true, underline/box=false.
        state.remove_emphasis();

        assert!(!state.tabs[0].paragraphs[1].runs[0].bold, "bold-only run should be cleared");
        assert!(state.tabs[0].paragraphs[2].runs[0].bold, "a Tag's own bold must survive");
    }

    #[test]
    fn remove_emphasis_is_a_no_op_when_nothing_is_configured() {
        let paragraphs = vec![para_plain("plain text")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.emphasis_bold = false;
        state.emphasis_underline = false;
        state.emphasis_box = false;
        let before = state.tabs[0].undo_stack.len();

        state.remove_emphasis();

        assert_eq!(state.tabs[0].undo_stack.len(), before, "no formatting is defined, so nothing to undo");
    }

    #[test]
    fn remove_emphasis_respects_an_active_selection() {
        let paragraphs = vec![
            Paragraph {
                runs: vec![Run { text: "one".into(), bold: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            Paragraph {
                runs: vec![Run { text: "two".into(), bold: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        // Selection covers only the first line ("one").
        state.tabs[0].selection = Some((0, 3));

        state.remove_emphasis();

        assert!(!state.tabs[0].paragraphs[0].runs[0].bold);
        assert!(state.tabs[0].paragraphs[1].runs[0].bold, "unselected line must survive");
    }

    #[test]
    fn remove_non_highlighted_underlining_skips_highlighted_runs() {
        let paragraphs = vec![Paragraph {
            runs: vec![
                Run { text: "plain-underline".into(), underline: true, ..Run::default() },
                Run { text: "highlighted-underline".into(), underline: true, highlight: true, ..Run::default() },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);

        state.remove_non_highlighted_underlining();

        let runs = &state.tabs[0].paragraphs[0].runs;
        assert!(!runs.iter().find(|r| r.text.contains("plain")).unwrap().underline);
        assert!(runs.iter().find(|r| r.text.contains("highlighted")).unwrap().underline);
    }

    #[test]
    fn remove_non_highlighted_underlining_respects_an_active_selection() {
        let paragraphs = vec![
            Paragraph {
                runs: vec![Run { text: "one".into(), underline: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            Paragraph {
                runs: vec![Run { text: "two".into(), underline: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        // Selection covers only the first line ("one").
        state.tabs[0].selection = Some((0, 3));

        state.remove_non_highlighted_underlining();

        assert!(!state.tabs[0].paragraphs[0].runs[0].underline);
        assert!(state.tabs[0].paragraphs[1].runs[0].underline, "unselected line must survive");
    }

    #[test]
    fn remove_blank_lines_deletes_only_empty_paragraphs() {
        let paragraphs = vec![para_plain("one"), para_plain(""), para_plain("two")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);

        state.remove_blank_lines();

        assert_eq!(state.tabs[0].content, "one\ntwo");
    }

    #[test]
    fn remove_blank_lines_with_a_selection_only_touches_selected_lines() {
        let paragraphs = vec![para_plain(""), para_plain("kept"), para_plain("")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        // Select only the middle + last line, leaving the leading blank alone.
        let start = state.tabs[0].content.find("kept").unwrap();
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((start, end));

        state.remove_blank_lines();

        assert_eq!(state.tabs[0].content, "\nkept");
    }

    #[test]
    fn remove_pilcrows_strips_the_marker_and_keeps_formatting() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "bold¶text".into(), bold: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);

        state.remove_pilcrows();

        assert_eq!(state.tabs[0].content, "boldtext");
        assert!(state.tabs[0].paragraphs[0].runs.iter().all(|r| r.bold));
    }

    #[test]
    fn remove_pilcrows_is_a_no_op_without_any() {
        let paragraphs = vec![para_plain("no marker here")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let before = state.tabs[0].undo_stack.len();

        state.remove_pilcrows();

        assert_eq!(state.tabs[0].undo_stack.len(), before);
    }

    // ── Delete tags / spoken-word counting ────────────────────────────────

    fn tag_para(text: &str) -> Paragraph {
        Paragraph {
            runs: vec![Run {
                text: text.into(),
                bold: true,
                size: 26,
                style: Some(CardStyle::Tag),
                ..Run::default()
            }],
            heading: CardStyleKind::Tag.heading_level(),
            alignment: Alignment::default(),
            unsupported_xml: None,
        }
    }

    /// The words survive; only the formatting goes.
    #[test]
    fn delete_tags_strips_formatting_but_keeps_the_line() {
        let mut state = make_state_with_paragraphs(
            vec![para_plain("body"), tag_para("A tag"), para_plain("more body")],
            0,
        );

        state.delete_tags();

        assert_eq!(state.tabs[0].content, "body\nA tag\nmore body");
        let tag = &state.tabs[0].paragraphs[1];
        assert_eq!(tag.heading, 0, "the heading marker is what made it a tag");
        assert_eq!(tag.runs[0].text, "A tag");
        assert!(!tag.runs[0].bold);
        assert_eq!(tag.runs[0].style, None);
        assert_eq!(tag.runs[0].size, state.normal_text_size_half_points);
    }

    /// The marker is authoritative, exactly as it is for analytics: a
    /// reformatted tag is still a tag.
    #[test]
    fn delete_tags_finds_a_marked_tag_whose_formatting_was_changed() {
        let mut para = tag_para("odd tag");
        para.runs[0].bold = false;
        para.runs[0].size = 99;
        para.heading = 0;
        let mut state = make_state_with_paragraphs(vec![para], 0);

        state.delete_tags();

        assert_eq!(state.tabs[0].paragraphs[0].runs[0].style, None);
        assert_eq!(state.tabs[0].paragraphs[0].runs[0].size, state.normal_text_size_half_points);
    }

    /// ...and a marked *cite* that happens to sit at a heading level is not a
    /// tag. This is the misidentification the marker exists to prevent.
    #[test]
    fn delete_tags_leaves_a_marked_cite_alone() {
        let mut para = tag_para("a cite");
        para.runs[0].style = Some(CardStyle::Cite);
        let mut state = make_state_with_paragraphs(vec![para], 0);

        state.delete_tags();

        assert_eq!(state.tabs[0].paragraphs[0].runs[0].style, Some(CardStyle::Cite));
        assert!(!state.tabs[0].is_modified, "nothing matched, so nothing changed");
    }

    #[test]
    fn delete_tags_is_a_no_op_when_there_are_none() {
        let mut state = make_state_with_paragraphs(vec![para_plain("just text")], 0);
        let version_before = state.tabs[0].content_version;

        state.delete_tags();

        assert_eq!(state.tabs[0].content_version, version_before);
        assert!(!state.tabs[0].is_modified);
    }

    /// The timer's WPM readout counts what actually gets read aloud —
    /// highlighted runs plus tags and cites — and nothing else.
    #[test]
    fn spoken_words_in_selection_counts_only_read_aloud_text() {
        let paragraphs = vec![Paragraph {
            runs: vec![
                run_plain("skip these four words "),
                Run { text: "two highlighted ".into(), highlight: true, ..Run::default() },
                Run { text: "one tag ".into(), style: Some(CardStyle::Tag), ..Run::default() },
                Run { text: "a cite".into(), style: Some(CardStyle::Cite), ..Run::default() },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, state.tabs[0].content.len()));

        assert_eq!(state.spoken_words_in_selection(), Some(6));
    }

    /// `None`, not `Some(0)` — the timer shows a "select text" hint for the
    /// first and a different message for the second.
    #[test]
    fn spoken_words_in_selection_distinguishes_no_selection_from_no_spoken_text() {
        let mut state = make_state_with_paragraphs(vec![para_plain("plain words only")], 0);
        assert_eq!(state.spoken_words_in_selection(), None);

        state.tabs[0].selection = Some((0, 16));
        assert_eq!(state.spoken_words_in_selection(), Some(0));
    }

    // ── Select similar formatting ─────────────────────────────────────────

    fn tagged(text: &str) -> Run {
        Run { text: text.into(), style: Some(CardStyle::Tag), ..Run::default() }
    }

    /// Two paragraphs, each "TAG" + " body". Cursor inside the first tag.
    fn tagged_doc(cursor: usize) -> AppState {
        let para = || Paragraph {
            runs: vec![tagged("TAG"), run_plain(" body")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        };
        make_state_with_paragraphs(vec![para(), para()], cursor)
    }

    #[test]
    fn test_select_similar_formatting_matches_every_run_like_the_cursors() {
        // "TAG body\nTAG body" — tags at 0..3 and 9..12.
        let mut state = tagged_doc(1);

        state.select_similar_formatting();

        assert_eq!(state.tabs[0].similar_ranges, vec![(0, 3), (9, 12)]);
        // Blanked so the caret selection and the matches can't both be drawn.
        assert_eq!(state.tabs[0].selection, None);
    }

    /// With a selection, the run at its *start* is the template — and the
    /// result replaces the selection rather than adding to it.
    #[test]
    fn test_select_similar_formatting_uses_the_selections_first_run() {
        let mut state = tagged_doc(0);
        // Spans the plain " body" into the second paragraph's tag; the start
        // sits in the plain run, so plain text is what gets matched.
        state.tabs[0].selection = Some((3, 11));

        state.select_similar_formatting();

        assert_eq!(state.tabs[0].similar_ranges, vec![(3, 8), (12, 17)]);
    }

    /// The payoff: one formatting command restyles every match at once.
    #[test]
    fn test_formatting_applies_to_every_similar_range() {
        let mut state = tagged_doc(1);
        state.select_similar_formatting();

        state.apply_formatting_to_selection(FormatOp::Bold(true));

        let tags: Vec<&Run> = state.tabs[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter())
            .filter(|r| r.text == "TAG")
            .collect();
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().all(|r| r.bold), "both tags should be bold");
        // Everything else untouched.
        assert!(state.tabs[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter())
            .filter(|r| r.text == " body")
            .all(|r| !r.bold));
    }

    /// A second click of the same button toggles off, exactly as it does for a
    /// single selection — but only because *every* match was already bold.
    #[test]
    fn test_formatting_toggles_off_only_when_every_similar_range_matches() {
        let mut state = tagged_doc(1);
        state.select_similar_formatting();
        state.apply_formatting_to_selection(FormatOp::Bold(true));

        // One match un-bolded by hand: the next apply must bold *it*, not
        // un-bold the other one.
        state.tabs[0].paragraphs[1].runs[0].bold = false;
        state.apply_formatting_to_selection(FormatOp::Bold(true));

        assert!(state.tabs[0]
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter())
            .filter(|r| r.text == "TAG")
            .all(|r| r.bold));
    }

    #[test]
    fn test_clear_similar_selection_drops_the_matches() {
        let mut state = tagged_doc(1);
        state.select_similar_formatting();
        assert!(!state.tabs[0].similar_ranges.is_empty());

        state.clear_similar_selection();

        assert!(state.tabs[0].similar_ranges.is_empty());
    }

    /// Both variants need a selection, and neither should touch a selection
    /// with no newlines in it — no undo entry for a no-op.
    #[test]
    fn test_condense_is_a_no_op_without_newlines() {
        let mut state = make_state_with_paragraphs(vec![para_plain("single line")], 0);
        state.tabs[0].selection = Some((0, 11));
        let version_before = state.tabs[0].content_version;

        state.condense_with_pilcrows();
        state.condense_selection();

        assert_eq!(state.tabs[0].content, "single line");
        assert_eq!(state.tabs[0].content_version, version_before);
    }

    #[test]
    fn test_condense_selection_preserves_run_formatting() {
        // Regression: condense used to delete the selection and reinsert it
        // as plain text (`sync_insert_str`), flattening every condensed
        // character down to a single unformatted run and losing bold/
        // highlight/size/etc. Each character's original formatting must
        // survive condensing, just with '\n' swapped for ' '.
        let paragraphs = vec![
            Paragraph {
                runs: vec![Run { text: "bold".into(), bold: true, ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            Paragraph { runs: vec![run_plain("plain")], heading: 0, alignment: Alignment::default(), unsupported_xml: None },
        ];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        let end = state.tabs[0].content.len();
        state.tabs[0].selection = Some((0, end));
        state.condense_selection();

        assert_eq!(state.tabs[0].content, "bold\u{200B} plain");
        let runs = &state.tabs[0].paragraphs[0].runs;
        let bold_run = runs.iter().find(|r| r.text.contains("bold")).unwrap();
        assert!(bold_run.bold, "bold formatting should survive condensing");
        let plain_run = runs.iter().find(|r| r.text.contains("plain")).unwrap();
        assert!(!plain_run.bold, "plain run shouldn't pick up bold");
    }

    #[test]
    fn test_shrink_text_sets_non_underlined_selection_to_small_size() {
        let paragraphs = vec![para_plain("hello world")];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 11));
        state.small_size_half_points = 8;
        state.shrink_text();

        let para = &state.tabs[0].paragraphs[0];
        assert!(para.runs.iter().all(|r| r.size == 8));
    }

    #[test]
    fn test_shrink_text_leaves_underlined_runs_untouched() {
        // User-clarified spec: Shrink sets non-underlined selected text to
        // settings.conf's `small_size`, but skips underlined runs entirely
        // (e.g. a debate card's underlined emphasis shouldn't shrink along
        // with the rest of the tag).
        let paragraphs = vec![Paragraph {
            runs: vec![
                Run { text: "under".into(), underline: true, size: 24, ..Run::default() },
                Run { text: "plain".into(), size: 24, ..Run::default() },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let mut state = make_state_with_paragraphs(paragraphs, 0);
        state.tabs[0].selection = Some((0, 10)); // whole line: "underplain"
        state.small_size_half_points = 8;
        state.shrink_text();

        let runs = &state.tabs[0].paragraphs[0].runs;
        assert_eq!(runs[0].size, 24, "underlined run must be left alone");
        assert_eq!(runs[1].size, 8, "non-underlined run should shrink to small_size");
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

    // ── File explorer right-click menu (found_bugs.md) ──────────────────────

    /// A fresh, unique temp directory for a filesystem-touching test —
    /// mirrors `docx_parser.rs`'s own `test_real_file_round_trip_...`
    /// pattern, with `test_name` added so parallel test threads (cargo
    /// test's default) never collide on the same directory.
    fn temp_test_dir(test_name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vimbatim_ctx_menu_test_{}_{}", std::process::id(), test_name));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ── Word count ────────────────────────────────────────────────────────

    #[test]
    fn document_stats_counts_total_tag_and_highlighted_words() {
        let tag_para = Paragraph {
            runs: vec![Run { text: "extinction comes first".into(), bold: true, ..Run::default() }],
            heading: 4, // Tag — CardStyleKind::heading_level
            alignment: Alignment::default(),
            unsupported_xml: None,
        };
        let body = Paragraph {
            runs: vec![
                Run { text: "unread lead in ".into(), ..Run::default() },
                Run { text: "three highlighted words".into(), highlight: true, highlight_color: "yellow".into(), ..Run::default() },
                Run { text: " unread tail".into(), ..Run::default() },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        };
        let state = make_state_with_paragraphs(vec![tag_para, body], 0);
        let stats = state.document_stats();

        // 3 in the tag line + 8 in the body line.
        assert_eq!(stats.total_words, 11);
        assert_eq!(stats.tag_words, 3);
        assert_eq!(stats.highlighted_words, 3);
        assert_eq!(stats.spoken_words, 6);
    }

    #[test]
    fn document_stats_ignores_heading_levels_that_are_not_tag() {
        // Pocket/Hat/Block are headings 1..3 and are not read aloud.
        let pocket = Paragraph {
            runs: vec![Run { text: "not a tag".into(), ..Run::default() }],
            heading: 1,
            alignment: Alignment::default(),
            unsupported_xml: None,
        };
        let state = make_state_with_paragraphs(vec![pocket], 0);
        assert_eq!(state.document_stats().tag_words, 0);
    }

    #[test]
    fn estimated_time_formats_as_minutes_and_seconds() {
        let stats = DocumentStats { spoken_words: 300, ..DocumentStats::default() };
        assert_eq!(stats.estimated_time(300), (1, 0));
        assert_eq!(stats.estimated_time(600), (0, 30));

        // 450 words at 300 wpm is 1.5 minutes.
        let stats = DocumentStats { spoken_words: 450, ..DocumentStats::default() };
        assert_eq!(stats.estimated_time(300), (1, 30));
    }

    /// A zero rate would divide by zero and produce an infinite estimate; the
    /// clamp is what stops a hand-edited settings.conf from doing that.
    #[test]
    fn estimated_time_survives_a_nonsense_wpm() {
        let stats = DocumentStats { spoken_words: 100, ..DocumentStats::default() };
        let (m, s) = stats.estimated_time(0);
        assert!(m < 10 && s < 60, "expected a finite estimate, got {m}:{s}");
    }

    #[test]
    fn spreading_wpm_is_clamped_to_a_usable_range() {
        assert_eq!(clamp_spreading_wpm(0), 50);
        assert_eq!(clamp_spreading_wpm(300), 300);
        assert_eq!(clamp_spreading_wpm(99_999), 1000);
    }

    // ── Find / Replace ────────────────────────────────────────────────────

    #[test]
    fn find_from_is_ascii_case_insensitive_and_respects_start() {
        assert_eq!(find_from("Hello hello", "hello", 0), Some(0));
        assert_eq!(find_from("Hello hello", "hello", 1), Some(6));
        assert_eq!(find_from("Hello hello", "HELLO", 0), Some(0));
        assert_eq!(find_from("Hello", "bye", 0), None);
        // Empty needle must never match, or find_next would spin.
        assert_eq!(find_from("Hello", "", 0), None);
    }

    #[test]
    fn find_from_never_splits_a_multibyte_char() {
        // "é" is two bytes: a naive byte scan would test an offset inside it.
        let content = "café cafe";
        assert_eq!(find_from(content, "cafe", 0), Some(6));
        assert_eq!(find_from(content, "café", 0), Some(0));
    }

    #[test]
    fn rfind_before_finds_the_last_match_that_starts_earlier() {
        assert_eq!(rfind_before("a b a b a", "a", 9), Some(8));
        assert_eq!(rfind_before("a b a b a", "a", 8), Some(4));
        // "strictly before": offset 0 still qualifies when `before` is 1...
        assert_eq!(rfind_before("a b a b a", "a", 1), Some(0));
        // ...and nothing qualifies at 0, which is what makes find_next(false)
        // wrap instead of re-finding the match it is already sitting on.
        assert_eq!(rfind_before("a b a b a", "a", 0), None);
    }

    #[test]
    fn find_next_selects_the_match_and_wraps_around() {
        let mut state = make_state("one two one", 0, None);
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "one".to_string();

        assert!(state.find_next(true));
        assert_eq!(state.tabs[0].selection, Some((0, 3)));

        assert!(state.find_next(true));
        assert_eq!(state.tabs[0].selection, Some((8, 11)));

        // Past the last match, wrap back to the first.
        assert!(state.find_next(true));
        assert_eq!(state.tabs[0].selection, Some((0, 3)));
    }

    #[test]
    fn find_next_backward_walks_in_reverse() {
        let mut state = make_state("one two one", 11, None);
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "one".to_string();

        assert!(state.find_next(false));
        assert_eq!(state.tabs[0].selection, Some((8, 11)));
        assert!(state.find_next(false));
        assert_eq!(state.tabs[0].selection, Some((0, 3)));
    }

    /// Replace must only act on a selection that actually *is* a match —
    /// otherwise pressing it right after opening the bar overwrites whatever
    /// text happened to be selected.
    #[test]
    fn replace_current_ignores_a_selection_that_is_not_a_match() {
        let mut state = make_state("one two", 0, None);
        state.tabs[0].selection = Some((4, 7)); // "two"
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "one".to_string();
        state.find_bar.as_mut().unwrap().replacement = "X".to_string();

        state.replace_current();
        assert!(state.tabs[0].content.contains("two"), "unrelated selection was overwritten");
    }

    #[test]
    fn replace_current_swaps_the_found_match_then_advances() {
        let mut state = make_state("one two one", 0, None);
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "one".to_string();
        state.find_bar.as_mut().unwrap().replacement = "X".to_string();

        state.find_next(true);
        state.replace_current();
        assert_eq!(state.tabs[0].content, "X two one");
        // ...and moved on to the remaining match.
        assert_eq!(state.tabs[0].selection, Some((6, 9)));
    }

    #[test]
    fn replace_all_replaces_every_match_case_insensitively() {
        let mut state = make_state("One one ONE", 0, None);
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "one".to_string();
        state.find_bar.as_mut().unwrap().replacement = "two".to_string();

        assert_eq!(state.replace_all(), 3);
        assert_eq!(state.tabs[0].content, "two two two");
    }

    /// A replacement containing the query would loop forever if the scan
    /// resumed at the match position instead of past the inserted text.
    #[test]
    fn replace_all_terminates_when_the_replacement_contains_the_query() {
        let mut state = make_state("a a a", 0, None);
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "a".to_string();
        state.find_bar.as_mut().unwrap().replacement = "aa".to_string();

        assert_eq!(state.replace_all(), 3);
        assert_eq!(state.tabs[0].content, "aa aa aa");
    }

    #[test]
    fn open_find_bar_seeds_the_query_from_the_selection() {
        let mut state = make_state("hello world", 0, None);
        state.tabs[0].selection = Some((6, 11));
        state.open_find_bar();
        assert_eq!(state.find_bar.as_ref().unwrap().query, "world");
    }

    #[test]
    fn refresh_find_matches_counts_every_occurrence() {
        let mut state = make_state("one one one", 0, None);
        state.open_find_bar();
        state.find_bar.as_mut().unwrap().query = "one".to_string();
        state.refresh_find_matches();
        assert_eq!(state.find_bar.as_ref().unwrap().match_count, 3);

        state.find_next(true);
        assert_eq!(state.find_bar.as_ref().unwrap().current_match, 1);
        state.find_next(true);
        assert_eq!(state.find_bar.as_ref().unwrap().current_match, 2);
    }

    // ── Spellcheck ────────────────────────────────────────────────────────

    /// The one branch worth protecting here: `replace_spell_target` composes
    /// three existing methods, and getting the (line, col) span wrong would
    /// silently eat the wrong characters.
    #[test]
    fn test_replace_spell_target_swaps_only_the_flagged_word() {
        let mut state = make_state("hello wrold there", 0, None);
        let target = SpellTarget {
            line: 0,
            start_col: 6,
            end_col: 11,
            word: "wrold".to_string(),
            suggestions: vec!["world".to_string()],
        };
        state.replace_spell_target(&target, "world");
        assert_eq!(state.tabs[0].content, "hello world there");
    }

    /// On a later line, so the line→byte-offset conversion is actually
    /// exercised rather than trivially passing at offset 0.
    #[test]
    fn test_replace_spell_target_on_second_line() {
        let mut state = make_state("first line\nsecond teh line", 0, None);
        let target = SpellTarget {
            line: 1,
            start_col: 7,
            end_col: 10,
            word: "teh".to_string(),
            suggestions: vec![],
        };
        state.replace_spell_target(&target, "the");
        assert_eq!(state.tabs[0].content, "first line\nsecond the line");
    }

    #[test]
    fn test_add_to_user_dictionary_is_lowercased_and_deduped() {
        // Explicit temp path — the no-arg `add_to_user_dictionary` appends to
        // the *real* ~/.vimbatim/user_dictionary.txt, which a test must never
        // touch.
        let path = std::env::temp_dir().join("vimbatim_test_user_dict.txt");
        let _ = std::fs::remove_file(&path);

        let mut state = make_state("", 0, None);
        state.add_to_user_dictionary_at("Kritik", &path);
        state.add_to_user_dictionary_at("kritik", &path);
        assert!(state.user_dictionary.contains("kritik"));
        assert_eq!(state.user_dictionary.len(), 1);

        // The dedup must reach the file too, not just the in-memory set —
        // otherwise the list grows without bound across sessions.
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written.lines().collect::<Vec<_>>(), vec!["kritik"]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_open_file_context_menu_sets_state() {
        let mut state = make_state("", 0, None);
        state.open_file_context_menu((10.0, 20.0), FileContextMenuTarget::Background);

        let menu = state.file_context_menu.as_ref().unwrap();
        assert_eq!(menu.position, (10.0, 20.0));
        assert_eq!(menu.target, FileContextMenuTarget::Background);
        assert!(!menu.confirming_delete);
    }

    #[test]
    fn test_close_file_context_menu_clears_state() {
        let mut state = make_state("", 0, None);
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Background);
        state.close_file_context_menu();
        assert!(state.file_context_menu.is_none());
    }

    #[test]
    fn test_request_delete_confirmation_arms_flag_without_deleting() {
        let dir = temp_test_dir("request_confirmation");
        let path = dir.join("keep-me.docx");
        std::fs::write(&path, b"placeholder").unwrap();

        let mut state = make_state("", 0, None);
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::File(path.clone()));
        state.request_context_menu_delete_confirmation();

        assert!(state.file_context_menu.as_ref().unwrap().confirming_delete);
        assert!(path.exists(), "delete must not happen until confirmed");
    }

    #[test]
    fn test_confirm_delete_removes_the_targeted_file() {
        let dir = temp_test_dir("confirm_delete");
        let path = dir.join("delete-me.docx");
        std::fs::write(&path, b"placeholder").unwrap();

        let mut state = make_state("", 0, None);
        state.working_directory = dir;
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::File(path.clone()));
        state.request_context_menu_delete_confirmation();

        state.confirm_context_menu_delete().unwrap();

        assert!(!path.exists());
        assert!(state.file_context_menu.is_none());
    }

    #[test]
    fn test_confirm_delete_is_noop_for_dir_and_background_targets() {
        let mut state = make_state("", 0, None);
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Dir(std::path::PathBuf::from("/some/dir")));
        assert!(state.confirm_context_menu_delete().is_ok());

        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Background);
        assert!(state.confirm_context_menu_delete().is_ok());
    }

    #[test]
    fn test_create_new_docx_in_picks_first_available_untitled_name() {
        let dir = temp_test_dir("naming_sequence");
        let mut state = make_state("", 0, None);
        state.working_directory = dir.clone();

        state.create_new_docx_in(&dir).unwrap();
        assert!(dir.join("Untitled.docx").exists());

        state.create_new_docx_in(&dir).unwrap();
        assert!(dir.join("Untitled 1.docx").exists());
    }

    #[test]
    fn test_create_file_at_context_menu_location_targets_files_parent_dir() {
        let dir = temp_test_dir("file_target_parent_dir");
        let subdir = dir.join("cards");
        std::fs::create_dir_all(&subdir).unwrap();
        let existing_file = subdir.join("existing.docx");
        std::fs::write(&existing_file, b"placeholder").unwrap();

        let mut state = make_state("", 0, None);
        state.working_directory = dir.clone();
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::File(existing_file));

        state.create_file_at_context_menu_location().unwrap();

        // New file lands next to the right-clicked file (in `cards/`), not
        // at the tree's root (`dir`).
        assert!(subdir.join("Untitled.docx").exists());
        assert!(!dir.join("Untitled.docx").exists());
    }

    #[test]
    fn test_create_file_at_context_menu_location_targets_dir_itself() {
        let dir = temp_test_dir("dir_target");
        let subdir = dir.join("cards");
        std::fs::create_dir_all(&subdir).unwrap();

        let mut state = make_state("", 0, None);
        state.working_directory = dir.clone();
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Dir(subdir.clone()));

        state.create_file_at_context_menu_location().unwrap();

        assert!(subdir.join("Untitled.docx").exists());
    }

    #[test]
    fn test_create_file_at_context_menu_location_background_targets_working_directory() {
        let dir = temp_test_dir("background_target");
        let mut state = make_state("", 0, None);
        state.working_directory = dir.clone();
        state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Background);

        state.create_file_at_context_menu_location().unwrap();

        assert!(dir.join("Untitled.docx").exists());
    }

    // ── Document recovery ───────────────────────────────────────────────────

    /// Builds a state with one dirty tab plus a fake pending recovery entry
    /// pointing at real files in a temp dir, so the recovery actions have
    /// something to act on.
    fn make_state_with_recovery(tag: &str) -> (AppState, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("vimbatim-state-rec-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let snapshot = dir.join("111-0.docx");
        let meta = dir.join("111-0.meta");
        let original = dir.join("case.docx");

        let mut para = crate::docx_parser::Paragraph::default();
        para.runs.push(crate::docx_parser::Run { text: "recovered text".into(), ..Default::default() });
        crate::docx_parser::create_new_docx(&[para], &snapshot).unwrap();
        std::fs::write(&meta, crate::recovery::format_meta(Some(&original), "case.docx", 1)).unwrap();

        let mut state = make_state("", 0, None);
        state.pending_recovery = vec![crate::recovery::RecoveryEntry {
            snapshot,
            meta,
            original_path: Some(original),
            title: "case.docx".into(),
            saved_at: 1,
        }];
        (state, dir)
    }

    #[test]
    fn discard_recovery_pops_the_entry_and_deletes_both_files() {
        let (mut state, _dir) = make_state_with_recovery("discard");
        let entry = state.pending_recovery[0].clone();

        state.discard_recovery();

        assert!(state.pending_recovery.is_empty());
        assert!(!entry.snapshot.exists());
        assert!(!entry.meta.exists());
    }

    #[test]
    fn resume_recovery_opens_a_modified_tab_pointed_at_the_original_path() {
        let (mut state, _dir) = make_state_with_recovery("resume");
        let entry = state.pending_recovery[0].clone();
        let tabs_before = state.tabs.len();

        state.resume_recovery();

        assert!(state.pending_recovery.is_empty());
        assert_eq!(state.tabs.len(), tabs_before + 1);
        let tab = state.tabs.last().unwrap();
        assert_eq!(tab.file_path, entry.original_path);
        assert!(tab.is_modified, "resumed changes are unsaved by design");
        assert!(tab.content.contains("recovered text"));
        assert_eq!(state.active_tab, state.tabs.len() - 1);
        // The snapshot is consumed — the content now lives in the editor.
        assert!(!entry.snapshot.exists());
    }

    #[test]
    fn a_resumed_tab_is_eligible_for_a_fresh_snapshot_without_any_further_typing() {
        let (mut state, _dir) = make_state_with_recovery("resume-snapshot-eligible");

        state.resume_recovery();

        let tab = state.tabs.last().unwrap();
        // Simulate the background task looking at this tab one interval later.
        let later = tab.last_edit_at.unwrap() + crate::recovery::MIN_SNAPSHOT_INTERVAL;
        assert!(
            crate::recovery::needs_snapshot(
                tab.is_modified,
                tab.content_version,
                tab.last_snapshot_version,
                tab.last_edit_at,
                later,
                crate::recovery::MIN_SNAPSHOT_INTERVAL,
            ),
            "a resumed tab must be snapshot-eligible — its only on-disk copy was just deleted"
        );
    }

    #[test]
    fn resume_recovery_of_a_never_saved_tab_opens_an_untitled_modified_tab() {
        let (mut state, _dir) = make_state_with_recovery("resume-untitled");
        state.pending_recovery[0].original_path = None;
        state.pending_recovery[0].title = "New Tab".into();

        state.resume_recovery();

        let tab = state.tabs.last().unwrap();
        assert_eq!(tab.file_path, None);
        assert!(tab.is_modified);
        assert!(tab.content.contains("recovered text"));
    }

    #[test]
    fn complete_recovery_save_as_copies_the_snapshot_and_deletes_it() {
        let (mut state, dir) = make_state_with_recovery("save-as");
        let entry = state.take_recovery_for_save_as().unwrap();
        let dest = dir.join("saved-elsewhere.docx");

        state.complete_recovery_save_as(&entry, &dest).unwrap();

        assert!(dest.exists());
        assert!(state.pending_recovery.is_empty());
        assert!(!entry.snapshot.exists());
        // The copy is a real docx, not a truncated one.
        assert!(crate::docx_parser::parse_docx(&dest).is_ok());
    }

    #[test]
    fn test_with_docx_extension_appends_when_missing() {
        assert_eq!(with_docx_extension(Path::new("New Tab")), Path::new("New Tab.docx"));
        assert_eq!(with_docx_extension(Path::new("/tmp/aff")), Path::new("/tmp/aff.docx"));
    }

    #[test]
    fn test_with_docx_extension_leaves_an_existing_one_alone() {
        assert_eq!(with_docx_extension(Path::new("card.docx")), Path::new("card.docx"));
        // Case-insensitive: Windows pickers hand back `.DOCX`.
        assert_eq!(with_docx_extension(Path::new("card.DOCX")), Path::new("card.DOCX"));
        assert_eq!(with_docx_extension(Path::new("card.Docx")), Path::new("card.Docx"));
    }

    #[test]
    fn test_with_docx_extension_appends_rather_than_replacing_other_suffixes() {
        // `set_extension` would turn these into "neg.docx" and "1ac.docx",
        // silently eating part of the name the user typed.
        assert_eq!(with_docx_extension(Path::new("neg.v2")), Path::new("neg.v2.docx"));
        assert_eq!(with_docx_extension(Path::new("1ac.txt")), Path::new("1ac.txt.docx"));
    }

    #[test]
    fn complete_recovery_save_as_forces_the_docx_extension() {
        let (mut state, dir) = make_state_with_recovery("save-as-no-ext");
        let entry = state.take_recovery_for_save_as().unwrap();
        // What the picker hands back when the user accepts the "New Tab"
        // suggestion verbatim.
        let typed = dir.join("New Tab");

        state.complete_recovery_save_as(&entry, &typed).unwrap();

        assert!(!typed.exists(), "must not write an extension-less file");
        let expected = dir.join("New Tab.docx");
        assert!(expected.exists(), "should have written {expected:?}");
        assert!(crate::docx_parser::parse_docx(&expected).is_ok());
    }

    #[test]
    fn resume_recovery_drops_a_snapshot_that_will_not_parse_instead_of_opening_an_empty_tab() {
        let (mut state, _dir) = make_state_with_recovery("resume-corrupt");
        let entry = state.pending_recovery[0].clone();
        std::fs::write(&entry.snapshot, b"not a zip at all").unwrap();
        let tabs_before = state.tabs.len();

        state.resume_recovery();

        assert!(state.pending_recovery.is_empty());
        assert_eq!(state.tabs.len(), tabs_before, "no tab for an unreadable snapshot");
        assert!(!entry.snapshot.exists());
    }

    #[test]
    fn take_recovery_for_save_as_returns_none_when_nothing_is_pending() {
        let mut state = make_state("", 0, None);
        assert!(state.take_recovery_for_save_as().is_none());
    }

    #[test]
    fn recovery_actions_walk_through_multiple_entries_one_at_a_time() {
        let (mut state, _dir) = make_state_with_recovery("multi");
        let first = state.pending_recovery[0].clone();
        state.pending_recovery.push(crate::recovery::RecoveryEntry {
            title: "second.docx".into(),
            saved_at: 0,
            ..first
        });
        assert_eq!(state.pending_recovery.len(), 2);

        state.discard_recovery();
        assert_eq!(state.pending_recovery.len(), 1);
        assert_eq!(state.pending_recovery[0].title, "second.docx");

        state.discard_recovery();
        assert!(state.pending_recovery.is_empty());
    }

    #[test]
    fn close_tab_deletes_that_tabs_snapshot() {
        let mut state = make_state("hello", 0, None);
        state.tabs.push(Tab::new_empty(1));
        let (docx, meta) = crate::recovery::snapshot_paths(state.tabs[1].id);
        std::fs::create_dir_all(crate::recovery::recovery_dir()).unwrap();
        std::fs::write(&docx, b"x").unwrap();
        std::fs::write(&meta, b"x").unwrap();

        state.close_tab(1);

        assert!(!docx.exists());
        assert!(!meta.exists());
    }

    #[test]
    fn confirm_close_discard_for_an_app_close_deletes_every_snapshot() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].is_modified = true;
        let (docx, meta) = crate::recovery::snapshot_paths(state.tabs[0].id);
        std::fs::create_dir_all(crate::recovery::recovery_dir()).unwrap();
        std::fs::write(&docx, b"x").unwrap();
        std::fs::write(&meta, b"x").unwrap();

        state.request_close_app();
        state.confirm_close_discard();

        assert!(!docx.exists());
        assert!(!meta.exists());
    }

    #[test]
    fn dirty_tab_snapshots_includes_only_modified_tabs() {
        let mut state = make_state("hello", 0, None);
        state.tabs[0].is_modified = true;
        state.tabs[0].title = "dirty".into();
        let mut clean = Tab::new_empty(1);
        clean.title = "clean".into();
        state.tabs.push(clean);

        let mirror = state.dirty_tab_snapshots();

        assert_eq!(mirror.len(), 1);
        assert_eq!(mirror[0].title, "dirty");
        assert_eq!(mirror[0].id, state.tabs[0].id);
    }

    #[test]
    fn dirty_tab_snapshots_is_empty_when_nothing_is_modified() {
        let state = make_state("hello", 0, None);
        assert!(state.dirty_tab_snapshots().is_empty());
    }
}
