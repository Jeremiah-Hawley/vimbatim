use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::case_converter;
use crate::document::{DocumentBuffer, TabId};
use crate::document_ops::{
    apply_format_op, apply_formatting, apply_paragraph_alignment, buffer_delete_range,
    buffer_insert_char, buffer_insert_str, buffer_insert_str_with_runs, is_uniformly_active,
    ranges_matching_format, reset_card_style_in_range, runs_in_range, toggled_off, FormatOp,
};
use crate::docx_parser::{
    parse_docx, Alignment, CardStyle, DocxOrigin, ListItem, ListKind, Paragraph, Run,
};
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

/// Allocator bookkeeping attributable to one `Run`, on top of its struct size
/// and the bytes its strings hold.
///
/// Every run owns several separately-allocated `String`s (`text`,
/// `highlight_color`, and optionally `font`/`color`), and each carries its own
/// header and size-class rounding. Measured at ~30 bytes per run against real
/// RSS — see `snapshot_byte_estimate`'s table. A calibration figure, not a
/// derivation: it is allocator-specific, and re-measuring it is what
/// `bench_diagnostic_undo_memory_growth_per_tab` is for.
const PER_RUN_ALLOCATION_OVERHEAD: usize = 30;
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

/// Heap bytes one undo/redo snapshot really costs, used to keep the stacks
/// bounded (see `undo_stack_cap_for_snapshot_size`).
///
/// This used to count only string *payloads*, which made it wrong by 2.7x on
/// a real document — and it was wrong in the direction that matters, letting
/// the byte budget permit far more memory than it claimed. Measured on a
/// 243KB debate file (`bench_diagnostic_undo_memory_growth_per_tab`, which
/// reads RSS straight from `/proc/self/statm`):
///
/// ```text
///   content                  249KB
///   string payloads          254KB
///   Run/Paragraph structs    683KB   <- ignored entirely by the old estimate
///   allocator overhead       176KB   <- ~30 bytes per run
///   ------------------------------
///   total                   1.30MB   (old estimate said 0.48MB)
/// ```
///
/// The struct term dominates, because debate documents are run-dense — that
/// file carries 5864 runs across 476 paragraphs, so `Vec<Run>`'s own contents
/// outweigh the text they hold. Under-counting it let a single tab's stacks
/// reach ~512MB, and both the stacks and the budget are per tab, so a few
/// case files open at once could reach several GB. That is what the beta
/// report of "slows down after a while, fixed by restarting" actually was:
/// not a leak, but a ceiling that never held. Freed pages are not returned to
/// the OS either, so RSS never falls back within a session, which is why only
/// a restart cured it.
fn snapshot_byte_estimate(content: &str, paragraphs: &[Paragraph]) -> usize {
    let runs: usize = paragraphs.iter().map(|p| p.runs.len()).sum();
    content.len()
        + std::mem::size_of_val(paragraphs)
        + runs * (std::mem::size_of::<Run>() + PER_RUN_ALLOCATION_OVERHEAD)
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
/// Logs what one save actually cost, to `crash.log` via `log_line`.
///
/// Beta feedback reported the app degrading over a session — typing lagging,
/// scrolling stuttering, and saves taking up to ten seconds — cured by a
/// restart. Save is the most informative of those symptoms, because its cost
/// should be a pure function of document size: if the *same* document saves
/// quickly at launch and slowly an hour later, then what is being saved has
/// changed, not the machine.
///
/// `runs` is the number worth watching. Editing splits runs
/// (`split_run_at_position`) and merging fuses them back
/// (`merge_adjacent_same_format_runs`); if any edit path misses the merge,
/// run counts climb over a session while the text itself stays the same
/// size. Everything that walks runs — word wrap, painting, spellcheck, the
/// XML rebuild inside this very save — slows down together, which is exactly
/// the reported cluster of symptoms. Reopening the file re-parses it, and
/// parse merges adjacent identical runs, which would also explain why a
/// restart cures it.
///
/// So: bytes flat while runs climb means fragmentation, and this line says
/// where to look. Runs flat while `ms` climbs means the document model is
/// innocent and the cost is elsewhere (allocator pressure from the repeated
/// full-document clones, most likely).
fn log_save_cost(paragraphs: &[Paragraph], elapsed: std::time::Duration) {
    let runs: usize = paragraphs.iter().map(|p| p.runs.len()).sum();
    let bytes: usize = paragraphs
        .iter()
        .flat_map(|p| &p.runs)
        .map(|r| r.text.len())
        .sum();
    // Runs per KB is the actual signal — it holds steady on a healthy
    // document however much text is added, and climbs on a fragmenting one.
    let runs_per_kb = runs as f32 / (bytes as f32 / 1024.0).max(1.0);
    log_line(&format!(
        "[save] {:.1}ms  paragraphs={}  runs={}  bytes={}  runs/KB={:.1}",
        elapsed.as_secs_f32() * 1000.0,
        paragraphs.len(),
        runs,
        bytes,
        runs_per_kb,
    ));
}

fn undo_stack_cap_for_snapshot_size(snapshot_bytes: usize) -> usize {
    // Halved because the cap is applied to the undo *and* redo stacks
    // independently, and a tab can have both full at once. Without this the
    // budget silently permitted twice what its name promises.
    let budget_based = UNDO_STACK_BYTE_BUDGET / 2 / snapshot_bytes.max(1);
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
    pub id: TabId,
    pub document: DocumentBuffer,
    pub title: String,
    pub file_path: Option<PathBuf>,
    /// True from save preparation until that save's matching completion is applied.
    pub is_saving: bool,
    /// Content version captured by the in-flight save.
    pub saving_version: Option<u64>,
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
    /// True when this tab was opened from a `.docx` that held bytes this build
    /// couldn't parse, so `tab_from_docx` detached it from its path rather than
    /// let the first Ctrl+S replace the original with a blank document.
    ///
    /// The detachment on its own was a silent trap: the tab is still titled
    /// after the file and still looks like it opened it, but Save is the no-op
    /// every path-less tab is. This is what puts a warning strip on it saying
    /// so, and pointing at Save As.
    pub opened_detached: bool,
    /// True once the user has dismissed this tab's warning strip. View-level UI
    /// state, same as every other per-tab boolean already in this struct.
    pub banner_dismissed: bool,
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
    pub id: TabId,
    pub paragraphs: Vec<Paragraph>,
    pub origin: Option<Arc<DocxOrigin>>,
    pub file_path: Option<PathBuf>,
    pub title: String,
    /// Carried so a snapshot written from the panic hook — which runs on the
    /// dying thread with no `AppState` to read — still bakes the user's own
    /// sizes and spacing into the recovered document, exactly as a normal save
    /// would. See `AppState::new_doc_style`.
    pub doc_style: crate::docx_parser::NewDocStyle,
}

/// A single empty paragraph containing one default (unformatted) run — the
/// starting state for `Tab.paragraphs` before any docx has been parsed into
/// it. Never `vec![]`: every rich-text-aware function assumes at least one
/// paragraph and run always exist.
pub fn default_paragraphs() -> Vec<Paragraph> {
    vec![Paragraph {
        list: None,
        runs: vec![Run::default()],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }]
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
        self.file_path.is_none() && self.document.content().is_empty() && !self.document.is_modified
    }

    pub fn new_empty(id: TabId) -> Self {
        /*
         * Creates a blank "New Tab" with no associated file. This is the default
         * starting state when the application opens or the user creates a new tab.
         */
        Tab {
            id,
            title: "New Tab".to_string(),
            file_path: None,
            is_saving: false,
            saving_version: None,
            document: DocumentBuffer::default(),
            docx_origin: None,
            pending_format: None,
            cursor: 0,
            selection: None,

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
            opened_detached: false,
            banner_dismissed: false,
        }
    }

    /// The warning strip this tab should show, or `None`.
    ///
    /// One slot, not one per condition: a tab that failed to parse has no
    /// origin and therefore no unsupported blocks either, so the two cases are
    /// mutually exclusive by construction.
    pub fn banner_message(&self) -> Option<&'static str> {
        if self.banner_dismissed {
            return None;
        }
        if self.opened_detached {
            return Some(
                "Vimbatim couldn't read this file, so this tab isn't linked to it — \
                 Save will do nothing. Use Save As to write your changes somewhere else.",
            );
        }
        if self.has_unsupported_blocks {
            return Some(
                "This document contains a table — Vimbatim can't edit or preserve it; \
                 saving will remove it.",
            );
        }
        None
    }

    pub fn from_path(id: TabId, path: PathBuf) -> Self {
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
            is_saving: false,
            saving_version: None,
            document: DocumentBuffer::default(),
            docx_origin: None,
            pending_format: None,
            cursor: 0,
            selection: None,

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
            opened_detached: false,
            banner_dismissed: false,
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
    /// The menu's third mode (alongside normal and `confirming_delete`):
    /// `Some` swaps the item list for a single text input holding the
    /// in-progress new name, committed by Enter. Lives here rather than on
    /// `FileExplorer` so the menu owns every one of its own modes; the
    /// `FocusHandle` that feeds it keystrokes stays in the view, since
    /// `state.rs` is gpui-free.
    pub rename_buffer: Option<String>,
}

/// What a Nav-mode right-click landed on. Unlike the file tree's own menu
/// this needs no path — a heading is identified by its line index into the
/// active tab's content, the same handle `jump_to_line` and
/// `heading_contents_range` already take.
#[derive(Clone, Debug, PartialEq)]
pub enum NavContextMenuTarget {
    Heading(usize),
    /// Right-click on empty space in the Nav list — only the "Show Heading
    /// Level 1–4" rows apply, since there's no heading to act on.
    Background,
}

/// State for the Nav outline's right-click menu, `None` when closed. Same
/// plain-tuple `position` convention (and same gpui-free reason) as
/// `FileContextMenu`.
#[derive(Clone, Debug, PartialEq)]
pub struct NavContextMenu {
    pub position: (f32, f32),
    pub target: NavContextMenuTarget,
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

/// State for the command palette (`src/command_palette.rs`), `None` when
/// closed. Just the query — the command list itself is a static registry, and
/// which row Enter runs is always the top result (per spec), so there is no
/// selection index to carry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandPaletteState {
    pub query: String,
}

/// Which of the find bar's two text fields keystrokes go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FindField {
    #[default]
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
    /// Search From List mode: Next/Previous walk every match of every word in
    /// `AppState.search_word_list` (merged, in document order) instead of the
    /// single `query`.
    ///
    /// A mode of this same panel rather than a second one, since "move through
    /// them the same way they can for the normal find button" is exactly this
    /// panel's traversal over a different needle set. `query` is left untouched
    /// while in list mode, so reopening with Ctrl+F restores whatever was last
    /// typed there.
    ///
    /// This struct is app-wide and reused across opens, so every open path must
    /// set this explicitly — see `open_find_bar` (clears) and
    /// `open_search_from_list` (sets).
    pub list_mode: bool,
}

/// A close action (tab-close `×` or the app-close `×`) awaiting the user's
/// answer to "save changes before closing?", set by `request_close_tab`/
/// `request_close_app` whenever the target has unsaved changes. `None` means
/// no confirmation is in flight — `close_confirm.rs` renders nothing in that
/// case, and `MainWindow` only mounts it at all while this is `Some`.
/// Recovery work owned independently from tabs and UI state.
#[derive(Default)]
pub struct RecoveryState {
    pub pending_entries: Vec<RecoveryEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NotificationSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub severity: NotificationSeverity,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PendingClose {
    Tab(TabId),
    App,
}

/// Transient UI overlays and menus.
pub struct UiState {
    pub file_context_menu: Option<FileContextMenu>,
    pub nav_context_menu: Option<NavContextMenu>,
    pub editor_context_menu: Option<EditorContextMenu>,
    pub find_bar: Option<FindBar>,
    pub command_palette: Option<CommandPaletteState>,
    pub settings_visible: bool,
    pub font_import_modal_open: bool,
    pub pending_close: Option<PendingClose>,
    pub sidebar_visible: bool,
    pub word_count_visible: bool,
    pub timer: crate::timer::TimerState,
    pub read_mode: bool,
    pub sidebar_before_read_mode: bool,
    pub invisibility_mode: bool,
    pub print_layout: bool,
    pub notifications: Vec<Notification>,
    /// Window-scoped keybind dispatch requested by an app effect.
    pub pending_keybinds: Vec<crate::keybinds::KeybindAction>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            file_context_menu: None,
            nav_context_menu: None,
            editor_context_menu: None,
            find_bar: None,
            command_palette: None,
            settings_visible: false,
            font_import_modal_open: false,
            pending_close: None,
            sidebar_visible: true,
            word_count_visible: false,
            timer: crate::timer::TimerState::default(),
            read_mode: false,
            sidebar_before_read_mode: true,
            invisibility_mode: false,
            print_layout: false,
            notifications: Vec::new(),
            pending_keybinds: Vec::new(),
        }
    }
}

/// Vim state shared across tabs: registers, macros, searches, and repeat state.
pub struct GlobalVimState {
    pub vim_enabled: bool,
    pub vim_keybinds: crate::vim_keybinds::VimKeybinds,
    pub pending_vim_action: Option<crate::keybinds::KeybindAction>,
    /// Platform work requested by a Vim command (for example `:e path`).
    pub pending_effects: Vec<crate::app::command::AppEffect>,
    pub vim_macros: HashMap<char, Vec<RecordedVimKey>>,
    vim_macro_recording: Option<(char, Vec<RecordedVimKey>)>,
    vim_macro_record_pending: bool,
    pub vim_last_macro_register: Option<char>,
    pub registers: HashMap<char, String>,
    pub register_formats: HashMap<char, String>,
    pub pending_clipboard_sync: Option<(String, String)>,
    pub last_search: Option<(String, bool)>,
    pub last_change: Option<VimChange>,
    pub(crate) vim_change_recording: Option<Vec<RecordedVimKey>>,
    vim_insertion_recording: Option<String>,
    vim_pending_change_before_insert: Option<(char, Vec<RecordedVimKey>)>,
}

impl Default for GlobalVimState {
    fn default() -> Self {
        Self {
            vim_enabled: false,
            vim_keybinds: crate::vim_keybinds::VimKeybinds::defaults(),
            pending_vim_action: None,
            pending_effects: Vec::new(),
            vim_macros: HashMap::new(),
            vim_macro_recording: None,
            vim_macro_record_pending: false,
            vim_last_macro_register: None,
            registers: HashMap::new(),
            register_formats: HashMap::new(),
            pending_clipboard_sync: None,
            last_search: None,
            last_change: None,
            vim_change_recording: None,
            vim_insertion_recording: None,
            vim_pending_change_before_insert: None,
        }
    }
}

/// Tab, pane, and filesystem-navigation state.
pub struct WorkspaceState {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub pending_focus_editor: Option<Pane>,
    pub next_tab_id: usize,
    pub closed_tabs: Vec<PathBuf>,
    pub working_directory: PathBuf,
    pub file_tree: Vec<FileNode>,
    pub split_view: bool,
    pub secondary_tab_id: Option<TabId>,
    pub focused_pane: Pane,
    pub primary_tab_id: Option<TabId>,
    pub split_ratio: f32,
    pub split_dragging: bool,
}

/// The shared application state, owned as a GPUI Model and read/written by all views.
pub struct AppState {
    workspace: WorkspaceState,
    ui: UiState,
    global_vim: GlobalVimState,
    /// Canonical persisted preferences; no writable scalar mirrors.
    preferences: crate::preferences::Preferences,
    /// Session-only sidebar width, clamped by its setter.
    sidebar_width: f32,
    /// Pending file-tree paste: source path and whether to cut rather than copy.
    copied_file: Option<(PathBuf, bool)>,
    /// Trimmed words persisted separately in search_word_list.txt.
    search_word_list: Vec<String>,
    sidebar_mode: SidebarMode,
    recovery: RecoveryState,
    keybinds: crate::keybinds::Keybinds,
    /// Imported (dark, light) palettes; resolve through current_palette().
    custom_theme: Option<(crate::theme::Palette, crate::theme::Palette)>,
    /// Session-only document zoom, not application chrome zoom.
    zoom: f32,
    /// Instance-specific path so tests never write the user's settings.
    settings_path: PathBuf,
    /// Lowercase words; Rc avoids cloning the dictionary on every render.
    user_dictionary: Rc<HashSet<String>>,
}

impl AppState {
    pub(crate) fn sidebar_width(&self) -> f32 {
        self.sidebar_width
    }
    pub(crate) fn sidebar_mode(&self) -> SidebarMode {
        self.sidebar_mode
    }
    pub(crate) fn copied_file(&self) -> Option<&(PathBuf, bool)> {
        self.copied_file.as_ref()
    }
    pub(crate) fn search_word_list(&self) -> &[String] {
        &self.search_word_list
    }
    pub(crate) fn recovery(&self) -> &RecoveryState {
        &self.recovery
    }
    pub(crate) fn keybinds(&self) -> &crate::keybinds::Keybinds {
        &self.keybinds
    }
    pub(crate) fn zoom(&self) -> f32 {
        self.zoom
    }
    pub(crate) fn user_dictionary(&self) -> &Rc<HashSet<String>> {
        &self.user_dictionary
    }

    pub(crate) fn workspace(&self) -> &WorkspaceState {
        &self.workspace
    }

    pub(crate) fn ui(&self) -> &UiState {
        &self.ui
    }

    pub(crate) fn preferences(&self) -> &crate::preferences::Preferences {
        &self.preferences
    }

    pub(crate) fn global_vim(&self) -> &GlobalVimState {
        &self.global_vim
    }
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
    fn is_centered(&self) -> bool {
        matches!(
            self,
            CardStyleKind::Pocket | CardStyleKind::Hat | CardStyleKind::Block
        )
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
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
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
/// Persists `working_directory` to settings.conf via `theme`'s generic
/// key=value upsert helper, so the file explorer reopens to the last folder
/// the user picked (`set_working_directory`) instead of always resetting to
/// `default_working_directory()`.
fn save_working_directory(path: &std::path::Path, dir: &std::path::Path) -> std::io::Result<()> {
    crate::preferences::Preferences::update(path, "working_directory", &dir.display().to_string())
}
/// Persists every currently expanded nav-pane directory to settings.conf as
/// a single `|`-joined `expanded_dirs` line, so `AppState::new()` can
/// restore the same folders expanded on the next launch
/// (`state::workspace::restore_expanded_dirs`).
pub(crate) fn save_expanded_dirs(path: &std::path::Path, dirs: &[PathBuf]) -> std::io::Result<()> {
    let joined = dirs
        .iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join("|");
    crate::preferences::Preferences::update(path, "expanded_dirs", &joined)
}

/// Guarantees `path` ends in `.docx` — every Save As destination goes through
/// this, both the name suggested to the native picker and whatever the user
/// actually types back.
///
/// Appends rather than using `Path::set_extension`, which would rewrite
/// anything after the last dot: `set_extension` turns "neg.v2" into "neg.docx",
/// silently eating part of the name. An existing `.docx` (in any case — Windows
/// pickers hand back `.DOCX`) is left exactly as the user wrote it.
/// Clamps the shrink size (points) to something a document can actually use.
/// Wide enough for any real "small text" convention, narrow enough that the
/// stepper can't walk it somewhere unreadable.
pub fn clamp_shrink_size_points(points: u16) -> u16 {
    points.clamp(4, 48)
}

/// Clamps the Emphasis size (points) to the same bounds as Shrink size —
/// same rationale: wide enough for any real convention, narrow enough that
/// the stepper can't walk it somewhere unreadable.
pub fn clamp_emphasis_size_points(points: u16) -> u16 {
    points.clamp(4, 48)
}

/// Clamps a card style's size (points) to the same bounds as Shrink and
/// Emphasis — wide enough for any real convention (a Pocket runs 26pt), narrow
/// enough that the stepper can't walk it somewhere unreadable.
pub fn clamp_card_size_points(points: u16) -> u16 {
    points.clamp(4, 48)
}

/// Clamps a words-per-minute value to something that can't produce a nonsense
/// estimate. Zero would divide by zero; the upper bound is well past any human
/// rate and only exists to keep the settings stepper from running away.
pub fn clamp_spreading_wpm(wpm: u32) -> u32 {
    wpm.clamp(50, 1000)
}

/// Line spacing used when settings.conf has no `line_spacing`. 1.0 means
/// "whatever `LINE_HEIGHT_RATIO` already produced", so an existing install
/// that never sets the key renders exactly as it did before the setting
/// existed.
pub const DEFAULT_LINE_SPACING: f32 = 1.0;

/// Clamps a line-spacing multiplier to a range a document can actually use.
///
/// The floor is below 1.0 on purpose — tightening past single spacing is a
/// real thing debate docs do — but not so far that rows collapse into each
/// other and click-to-position (which divides a click's Y by row height)
/// loses the resolution to tell two rows apart. The ceiling covers Word's
/// own double spacing with room above it.
///
/// A non-finite input (NaN from a malformed settings.conf value) falls back
/// to the default rather than propagating: NaN survives `clamp` in Rust's
/// f32 and would poison every row-height calculation downstream into NaN,
/// which lays out as a zero-height row rather than failing loudly.
pub fn clamp_line_spacing(spacing: f32) -> f32 {
    if !spacing.is_finite() {
        return DEFAULT_LINE_SPACING;
    }
    spacing.clamp(0.5, 3.0)
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

// ── Search From List matching core ──────────────────────────────────────────
//
// The word-list counterparts of `find_from`/`rfind_before`. Both return a
// `(start, end)` pair rather than a bare start, because a list's matches have
// *differing* lengths — every existing find call site assumes one fixed
// `query.len()`, which is exactly the assumption that breaks here.
//
// Both are built on `find_from`/`rfind_before` per word rather than a new
// scanner, so they inherit those functions' char-boundary guards and their
// ASCII case folding — which settles case sensitivity by reuse: list search is
// case-insensitive, same as Find, with no second convention to remember.

/// True when `[start, end)` in `content` is bounded by non-word characters on
/// both sides — the "whole words only" test.
///
/// Char-based, never byte indexing. Deliberately just `is_alphanumeric`: that
/// makes "don" match inside "don't" (an apostrophe isn't alphanumeric), which
/// is the standard trade every editor's whole-word search makes and not worth
/// a word-character table to avoid.
fn is_whole_word_at(content: &str, start: usize, end: usize) -> bool {
    let before_ok = content[..start]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric());
    let after_ok = content[end..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric());
    before_ok && after_ok
}

/// Earliest match of *any* word in `words` at or after `from`, as `(start, end)`.
///
/// "Earliest" is by start position, so repeated Next walks the merged match set
/// in document order — which is what "move through them the same way they can
/// for the normal find button" means for a list of several words.
pub(crate) fn find_list_from(
    content: &str,
    words: &[String],
    whole_words: bool,
    from: usize,
) -> Option<(usize, usize)> {
    words
        .iter()
        .filter_map(|word| {
            // A word can match before the first *whole-word* match, so the
            // scan has to keep stepping past rejected hits rather than
            // giving up on the first one.
            let mut at = from;
            loop {
                let start = find_from(content, word, at)?;
                let end = start + word.len();
                if !whole_words || is_whole_word_at(content, start, end) {
                    return Some((start, end));
                }
                at = start + 1;
            }
        })
        .min()
}

/// Last match of any word in `words` that *starts* strictly before `before`,
/// as `(start, end)`. The backward counterpart of `find_list_from`.
pub(crate) fn rfind_list_before(
    content: &str,
    words: &[String],
    whole_words: bool,
    before: usize,
) -> Option<(usize, usize)> {
    words
        .iter()
        .filter_map(|word| {
            let mut bound = before;
            loop {
                let start = rfind_before(content, word, bound)?;
                let end = start + word.len();
                if !whole_words || is_whole_word_at(content, start, end) {
                    return Some((start, end));
                }
                bound = start; // strictly-before, so this can't loop forever
            }
        })
        .max()
}

/// Every list match in the document, in the exact order Next steps through
/// them — the whole traversal in one pass.
///
/// Exists for complexity, not convenience. Counting by calling
/// `find_list_from` once per match is O(matches × words × len): each call
/// re-scans from its start position for *every* word, and a word with no
/// further occurrences scans to the end of the document every single time.
/// (The single-needle loop in `refresh_find_matches` has no such problem —
/// its successive scans partition the document, so it's linear overall.)
/// Collecting each word's matches once first is O(words × len), which is
/// what keeps a long list on a large card file from stalling the Next button.
///
/// The greedy walk at the end reproduces `find_list_from`'s own `min()`
/// choice — smallest start, then smallest end — and steps by the match's end
/// exactly as `find_next` does, so the "N of M" readout can never count a
/// match that Next would never stop on.
pub(crate) fn list_matches(
    content: &str,
    words: &[String],
    whole_words: bool,
) -> Vec<(usize, usize)> {
    let mut all: Vec<(usize, usize)> = Vec::new();
    for word in words {
        let mut at = 0;
        while let Some(start) = find_from(content, word, at) {
            let end = start + word.len();
            if !whole_words || is_whole_word_at(content, start, end) {
                all.push((start, end));
            }
            at = start + 1;
        }
    }
    all.sort_unstable();

    let mut out = Vec::new();
    let mut at = 0;
    for (start, end) in all {
        if start >= at {
            out.push((start, end));
            at = end.max(start + 1);
        }
    }
    out
}

/// Splits the user's word-list text into the words the search actually uses.
///
/// Trims each line and drops blanks so the panel's "N words" readout is honest
/// — `find_from` already refuses an empty needle, so a blank line's real cost
/// is a wrong count, not a spin.
pub(crate) fn parse_word_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(ToString::to_string)
        .collect()
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
pub(crate) fn save_custom_colors(
    path: &std::path::Path,
    key: &str,
    colors: &[u32],
) -> std::io::Result<()> {
    let joined = colors
        .iter()
        .map(|c| format!("{c:06x}"))
        .collect::<Vec<_>>()
        .join("|");
    crate::theme::save_setting_line(path, key, &joined)
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

/// Where Search From List keeps its words — one per line, beside settings.conf.
/// See `AppState.search_word_list` for why this isn't a settings.conf key.
pub fn search_word_list_path() -> PathBuf {
    settings_conf_path().with_file_name("search_word_list.txt")
}

/// Reads the word list back. A missing file is the normal state before the
/// user has written one, not an error.
pub(crate) fn load_word_list(path: &std::path::Path) -> Vec<String> {
    parse_word_list(&std::fs::read_to_string(path).unwrap_or_default())
}

/// Writes the word list out, one word per line. Errors are logged, not
/// propagated — the in-memory list is what the current session searches, and
/// failing to persist it must not lose it.
pub(crate) fn save_word_list(path: &std::path::Path, words: &[String]) {
    if let Err(e) = std::fs::write(path, words.join("\n")) {
        log_line(&format!("[settings] failed to save search word list: {e}"));
    }
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

mod editing;
mod preferences;
mod ui;
mod vim;
mod vim_dispatch;
mod vim_motion;
mod workspace;

pub(crate) use vim_motion::{
    is_vim_reserved_normal_key, matches_shifted_symbol, vim_find_target_char,
};
