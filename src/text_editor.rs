use gpui::prelude::*;
use gpui::*;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::auto_scroll::AutoScroller;
use crate::docx_parser::{Paragraph, Run};
use crate::document_ops::paragraph_run_char_spans;
use crate::keybinds::{CopyAction, CutAction, PasteAction};
use crate::state::{
    matches_shifted_symbol, vim_find_target_char, AppState, EditorContextMenu, Pane, SpellTarget,
    VimMode,
};
use crate::theme::{palette, Palette, ThemeMode};

/// `CHAR_WIDTH_PX`/`FONT_SIZE_PX`/`LINE_HEIGHT_PX` below are the 100%-zoom
/// baseline (`AppState.zoom == 1.0`) — every call site multiplies by the
/// active tab's `zoom` before use, so document text, wrapping, and
/// click/scroll hit-testing all scale together (found_bugs.md's Ctrl+=/
/// Ctrl+-/Ctrl+0 zoom). App chrome (placeholder text, the mode-indicator
/// strip) deliberately keeps using GPUI's plain `.text_sm()` instead —
/// zoom is scoped to document text only, not the whole UI.
///
/// Approximate monospace glyph width, used only to convert a mouse click's
/// pixel X position into a character column within its row. This is an
/// estimate (0.6× font size, the typical monospace advance width), not real
/// glyph shaping — precise X hit-testing would require rendering lines
/// through GPUI's InteractiveText/ShapedLine APIs instead of plain divs,
/// which is a larger rework than click-to-position alone justifies right
/// now. Word-wrap decisions do *not* use this — see `char_width_fn`, which
/// measures each character's real rendered width instead, since a single
/// uniform estimate is wrong for narrow glyphs like '.' or '-' and folds
/// lines dominated by them far earlier than their actual on-screen width
/// would require.
const CHAR_WIDTH_PX: f32 = 8.4;
/// The editor's `text_sm()` resolves to 0.875rem, i.e. 14px at GPUI's
/// default 16px rem_size (this app never overrides rem_size). Used to query
/// real glyph widths for word-wrap via `TextSystem::layout_width`, which
/// needs an explicit font size rather than reading it from render()'s
/// ambient text style.
const FONT_SIZE_PX: f32 = 14.0;
/// Width of the separator painted where invisibility mode dropped text between
/// two visible fragments, at 100% zoom.
///
/// A fixed width rather than a space character on purpose: a space is
/// font-metric-dependent, so the gap after a 26pt cite would be far wider than
/// one in body text, and the row would read unevenly. This is one number to
/// tune, and it scales with zoom like everything else.
const HIDDEN_TEXT_GAP_PX: f32 = 4.0;
/// Hover-group name tying a heading row to the fold marker inside it, so the
/// marker appears when the cursor is anywhere on that line rather than only
/// over the marker itself.
const FOLD_ROW_GROUP: &str = "fold-row";

/// A monospace glyph's advance as a fraction of its font size, derived from
/// the two constants above (8.4px at 14px = 0.6) — `CHAR_WIDTH_PX` alone is
/// only correct when the document actually renders at `FONT_SIZE_PX`, which it
/// does not: body text renders at settings.conf's `normal_text_size`
/// (`AppState.normal_text_size_half_points`, 11pt by default, i.e. 11px).
/// Click-to-position multiplies this by the *real* rendered size so a click
/// maps to the character under the pointer rather than one computed for a
/// larger font.
const CHAR_ADVANCE_RATIO: f32 = CHAR_WIDTH_PX / FONT_SIZE_PX;
/// Matches the `.min_h(px(20.0))` set on each line div in render().
const LINE_HEIGHT_PX: f32 = 20.0;
/// A line's height as a multiple of its font size (20/14 ≈ 1.43), derived
/// the same way `CHAR_ADVANCE_RATIO` above is: `LINE_HEIGHT_PX` alone is
/// only correct at `FONT_SIZE_PX`, which — same gap `CHAR_ADVANCE_RATIO`'s
/// own comment already flags for character width — body text does not
/// actually render at. Real line spacing has to scale off the actual
/// configured size (`normal_text_size_half_points`) via `line_height_px`
/// below, not this constant directly; using it unscaled is what made 11pt
/// body text (the 22-half-point default) sit in a slot calibrated for 14pt,
/// reading as visibly too generous and never shrinking for anything smaller.
const LINE_HEIGHT_RATIO: f32 = LINE_HEIGHT_PX / FONT_SIZE_PX;

/// The real row height for one line of body text at `normal_size_px` — the
/// zoom-scalable replacement for using `LINE_HEIGHT_PX` directly. See
/// `LINE_HEIGHT_RATIO`.
fn line_height_px(normal_size_px: f32) -> f32 {
    normal_size_px * LINE_HEIGHT_RATIO
}
/// Matches the `.p(px(16.0))` set on the outer editor div in render().
const CONTENT_PADDING_PX: f32 = 16.0;
/// Number of lines of buffer to keep visible above/below the cursor —
/// mirrors Vim's `scrolloff`. `scroll_to_cursor` starts scrolling once the
/// cursor comes within this many lines of the viewport edge, rather than
/// waiting until the cursor line itself is already clipped. Raised from 3
/// to 6 (found_bugs.md: auto-scroll at the bottom of the page only kicked
/// in once the cursor was already 3-4 lines from the edge — the old value
/// of this same constant — and needed to start earlier).
const SCROLL_MARGIN_LINES: f32 = 6.0;
/// A literal, well-known monospace family name rather than the generic
/// CSS-style alias `"monospace"`. GPUI's font matching (`cosmic_text`'s
/// `load_family`) filters real system fonts by an *exact string* match
/// against each font file's own embedded family name — no font ever
/// declares its family as literally "monospace", so that name always
/// missed and fell through to GPUI's hardcoded fallback stack, which
/// resolves each candidate at default weight/style, discarding any
/// requested bold/italic before it ever reaches font matching. Separately,
/// `find_best_match` short-circuits (`candidates.len() == 1 => Ok(0)`)
/// without checking weight/style whenever the resolved family has only one
/// loaded face — together these silently dropped every bold/italic
/// request. "DejaVu Sans Mono" ships with separate Book/Bold/Oblique/Bold
/// Oblique faces under one family name on essentially all Linux/WSL
/// systems, giving `find_best_match` real candidates to choose between.
pub(crate) const FONT_FAMILY: &str = "DejaVu Sans Mono";

/// The one other font this app can vouch for the same way it vouches for
/// `FONT_FAMILY`: bundled (`main.rs`'s `load_bundled_fonts`) with all 4
/// weight/style faces, so GPUI's `find_best_match` short-circuit (see
/// `FONT_FAMILY`'s own doc comment) can't silently drop bold/italic for it.
/// Not literally "Georgia" (the font a tester actually asked for) — Georgia
/// is a proprietary Microsoft font with no redistribution rights, so it
/// can't be bundled the same way. `formatting_ribbon.rs`'s Font Family
/// picker only offers `CURATED_FONTS`, not every font installed on the
/// host, specifically so a selection always renders correctly; a `run.font`
/// read from a real imported `.docx` (naming e.g. actual Georgia, Calibri,
/// Times New Roman) still round-trips on save, it just isn't rendered as
/// that font on screen — see `apply_run_style`.
pub(crate) const CURATED_SERIF_FONT: &str = "DejaVu Serif";

pub(crate) const CURATED_FONTS: &[&str] = &[FONT_FAMILY, CURATED_SERIF_FONT];

fn is_curated_font(name: &str) -> bool {
    CURATED_FONTS.contains(&name)
}

/// `CURATED_SERIF_FONT`'s counterpart to `CHAR_ADVANCE_RATIO` below — same
/// "advance as a fraction of font size" idea, but DejaVu Serif is
/// proportional (unlike the monospace primary font), so no single ratio is
/// exact per-character the way `CHAR_ADVANCE_RATIO` is for a font where
/// every glyph is identically wide. Measured as the mean advance across
/// a-z/0-9 at the font's own units-per-em (via `ttf_parser`, same
/// empirical-constant approach `CHAR_ADVANCE_RATIO` itself already uses —
/// see its doc comment), which is close enough to `CHAR_ADVANCE_RATIO`
/// (0.6) that this remains a reasonable approximation for click-to-position
/// and Up/Down column math, the same "not real glyph shaping" tradeoff
/// `CHAR_ADVANCE_RATIO`'s own doc comment already accepts. Word-wrap
/// decisions do *not* use this — `visual_rows_for_viewport` measures each
/// character's real rendered width per font instead.
const SERIF_CHAR_ADVANCE_RATIO: f32 = 0.589;

/// Caches the word-wrapped row table (and the intermediate data it's built
/// from — split lines, per-line chars, line byte offsets, and the cloned
/// paragraph formatting) across renders that don't actually change the
/// document. Scrolling, cursor movement, and focus changes all trigger a
/// render but don't touch `content`/`paragraphs` — without this cache,
/// `render()` re-wraps the *entire* document on every single one of those,
/// regardless of how much is actually visible (see performance_plan.md /
/// uniform_list_plan.md). Invalidated by `Tab.content_version` (bumped on
/// every real edit — see its own doc comment in `state.rs`), not by
/// comparing `content` itself, which would defeat the point.
///
/// `Rc`-wrapped so a cache hit is a handful of cheap pointer clones, not a
/// deep clone of the document — also what lets this data be captured
/// cheaply into a `uniform_list` render closure later, instead of deep-
/// cloned into it on every render.
struct RowCache {
    tab_id: usize,
    content_version: u64,
    /// `f32` isn't `Eq`; comparing via `to_bits()` is the standard way to
    /// use a float as an exact cache key without pulling in an epsilon
    /// comparison that would need its own tuning.
    viewport_width_bits: u32,
    zoom_bits: u32,
    /// Invisibility mode and fold both drop rows from `display_to_wrap`, so
    /// toggling either changes the tables and must invalidate — otherwise the
    /// editor keeps painting the previous mode's row list.
    invisibility: bool,
    fold_version: u64,
    lines: Rc<Vec<String>>,
    line_chars: Rc<Vec<Vec<char>>>,
    line_byte_starts: Rc<Vec<usize>>,
    rows: Rc<Vec<(usize, usize, usize)>>,
    paragraphs: Rc<Vec<Paragraph>>,
    /// `uniform_list`-facing expansion of `rows` — see
    /// `expand_rows_for_display`'s doc comment. Cached alongside `rows`
    /// since it's derived from it plus `paragraphs`/`zoom`, all of which are
    /// already part of this cache's invalidation key.
    display_to_wrap: Rc<Vec<Option<usize>>>,
    wrap_to_display: Rc<Vec<usize>>,
}

/// Whether a run should be left unpainted in invisibility mode.
///
/// What survives is highlighted text plus every card style:
/// * `heading != 0` covers Pocket/Hat/Block/Tag, which are paragraph-level
///   (`CardStyleKind::heading_level`), so the whole line stays.
/// * Cite is run-level and has no marker of its own — `apply_cite_style` only
///   sets bold plus `cite_size_half_points` — so it is identified by exactly
///   that pair. Any other bold run at the configured cite size is
///   indistinguishable from a real cite and will also stay visible; that is a
///   limit of how cites are stored, not a choice made here.
///
/// Hiding is purely visual — the run keeps its space, so wrap points, click
/// mapping and cursor math are untouched, and the document itself is never
/// modified.
fn run_is_hidden(
    invisibility: bool,
    heading: u8,
    highlighted: bool,
    bold: bool,
    size_half_points: u16,
    cite_size_half_points: u16,
) -> bool {
    if !invisibility {
        return false;
    }
    let card_style_line = heading != 0;
    // `size == 0` means "inherit the body size" in this codebase, so it can
    // never be a cite even if the setting were somehow zero.
    let is_cite = bold && size_half_points != 0 && size_half_points == cite_size_half_points;
    !(card_style_line || highlighted || is_cite)
}

/// The scroll arithmetic behind reading mode's Left/Right paging, split out
/// from `TextEditor::page_scroll` so it is testable without a laid-out view.
///
/// `current` and the result are GPUI scroll offsets: `<= 0`, growing more
/// negative further down the document. `max_y` is the positive maximum scroll
/// distance. Returns `None` when the page would not move — already at that end.
fn page_scroll_offset(
    current: f32,
    viewport_h: f32,
    row_height: f32,
    max_y: f32,
    forward: bool,
) -> Option<f32> {
    // Whole rows only: a raw pixel jump lands mid-row and slices the line
    // straddling the fold, scrolling half of it past unread. At least one row
    // so a viewport shorter than a single line still advances.
    let rows_per_page = (viewport_h / row_height).floor().max(1.0);
    let delta = rows_per_page * row_height;
    let target = if forward { current - delta } else { current + delta };
    let clamped = target.clamp(-max_y.max(0.0), 0.0);
    ((clamped - current).abs() >= 0.5).then_some(clamped)
}

/// Pure cache-validity check, pulled out of `render()` so it's unit-testable
/// without a GPUI context — the one part of the row cache that isn't just
/// GPUI interaction glue. `ignore_width` optionally accepts a cache built at a
/// different width.
///
/// `ignore_width` is set only while the split divider is being dragged
/// (`AppState.split_dragging`). A width change normally *must* invalidate —
/// the wrap points depend on it — but rebuilding costs a full-document re-wrap
/// per pane per mouse-move, which locks the app up on a large file. Reusing
/// the stale tables leaves the text wrapped at the pre-drag width for the
/// duration of the drag; releasing clears the flag and the next render wraps
/// correctly, once.
fn row_cache_is_valid_for(
    cache: &RowCache,
    tab_id: usize,
    content_version: u64,
    viewport_width: f32,
    zoom: f32,
    ignore_width: bool,
    invisibility: bool,
    fold_version: u64,
) -> bool {
    cache.tab_id == tab_id
        && cache.content_version == content_version
        && (ignore_width || cache.viewport_width_bits == viewport_width.to_bits())
        && cache.zoom_bits == zoom.to_bits()
        && cache.invisibility == invisibility
        && cache.fold_version == fold_version
}

/// The main document editing area.
///
/// Renders the text content of the currently active tab inside a focused,
/// scrollable div. Keyboard input is routed here when the div holds focus.
///
/// Designed to be the extensible base for .docx support: content currently lives
/// as plain `String` in `AppState::Tab`, meaning callers can swap in a richer
/// document model without touching this view's rendering or focus plumbing.
pub struct TextEditor {
    state: Entity<AppState>,
    /// GPUI focus handle — required to receive raw keyboard events.
    focus_handle: FocusHandle,
    /// Tracks this editor's scroll state (see `.track_scroll()` in
    /// render()). Besides the scroll offset itself, `.bounds()` also gives
    /// the editor's fixed viewport box in window coordinates — GPUI's own
    /// layout bounds for the tracked div, computed before any scroll
    /// translation is applied, so it can't drift with scroll position the
    /// way a hand-rolled bounds capture could. Click/drag positioning uses
    /// both `.offset()` and `.bounds()` to convert screen-relative
    /// coordinates into document-relative ones; drag-to-edge auto-scroll
    /// uses `.bounds()` for its edge-trigger check.
    scroll_handle: ScrollHandle,
    /// The `uniform_list` element's own scroll-handle type, passed to its
    /// `.track_scroll()` in render(). Its `base_handle` field is a plain
    /// `gpui::ScrollHandle` — the *same* type `scroll_handle` above already
    /// is (confirmed against the vendored `gpui` source, `elements/div.rs`'s
    /// `ScrollHandle(Rc<RefCell<..>>)` and `elements/uniform_list.rs`'s
    /// `UniformListScrollState.base_handle: ScrollHandle`), so `scroll_handle`
    /// is initialized *from* this one's `base_handle` in `new()` below —
    /// both fields end up pointing at the same Rc-shared state. That's what
    /// lets every existing consumer of `scroll_handle` (click/drag pixel
    /// math, `AutoScroller`, `scroll_to_cursor`) keep working unchanged
    /// while `uniform_list` itself tracks the real scroll position.
    uniform_list_scroll_handle: UniformListScrollHandle,
    /// Drives continuous scrolling while a click-drag sits near the top/
    /// bottom edge of the viewport — see `auto_scroll::AutoScroller`.
    auto_scroller: AutoScroller,
    /// True right after a bare `@` was pressed in Normal mode, waiting for
    /// the register character that completes `@<register>` (user-requested
    /// macro replay — not part of editor_instructions.md). Kept here rather
    /// than in `AppState` since resolving it triggers `replay_macro`, which
    /// needs this struct's GPUI context.
    macro_at_pending: bool,
    /// The word-wrapped row table for the current render, memoized across
    /// renders that don't change the document — see `RowCache`. `None`
    /// before the first render.
    row_cache: Option<RowCache>,
    /// Per-tab scroll offsets, keyed by the tab's stable `id` (not its
    /// positional index — matches the convention `tab_bar.rs` already uses
    /// for keying GPUI element ids, so reordering/closing other tabs can't
    /// scramble which offset belongs to which tab). There is only one
    /// `scroll_handle`/`uniform_list_scroll_handle` pair for the whole
    /// window (see `main_window.rs`, the sole `TextEditor::new` call), so
    /// without this map every tab shares one scroll position and switching
    /// tabs makes the old tab's scroll position "leak" into the new one.
    /// Saved/restored at the top of `render()` whenever the active tab
    /// changes. A tab with no entry yet (never visited) falls back to
    /// `Point::default()`, i.e. scrolled to the top.
    tab_scroll_offsets: std::collections::HashMap<usize, Point<Pixels>>,
    /// The tab `id` seen on the previous render, used to detect a tab
    /// switch at the top of the next `render()` call. `None` only before
    /// the very first render.
    last_seen_active_tab: Option<usize>,
    /// Memoized spellcheck results, keyed by the *line's own text* rather
    /// than by tab id + `content_version`.
    ///
    /// Keying on the text is what makes this cheap and correct at once:
    /// editing one line leaves every other line's key untouched, so a
    /// keystroke re-checks exactly the line being typed instead of the whole
    /// document (which is what a `content_version` key would have forced).
    /// Scrolling, switching tabs, and resizing hit warm entries. There is no
    /// invalidation logic at all — a line that changed simply has a different
    /// key, and a stale entry is unreachable rather than wrong.
    ///
    /// `Rc<RefCell<..>>` because the `uniform_list` render closure must be
    /// `'static` and so cannot borrow `self`; `Rc<Vec<..>>` values so a cache
    /// hit clones a pointer, not the ranges. Cleared wholesale past
    /// `SPELL_CACHE_MAX_LINES` — see `spell_ranges_cached`.
    spell_cache: Rc<RefCell<SpellCache>>,
    /// Which pane this editor paints. Two `TextEditor` entities exist while
    /// the split is open (`notes/split_view_plan.md`); every tab read below
    /// goes through `tab_index` rather than `AppState.active_tab`, so each one
    /// shows its own document.
    pane: Pane,
}

/// The spellcheck memo plus the one thing besides line text that its results
/// depend on.
#[derive(Default)]
struct SpellCache {
    /// Size of the user dictionary the cached entries were computed against.
    ///
    /// Without this, "Add to Dictionary" would leave every *already-cached*
    /// line still squiggling that word until the line happened to be edited —
    /// the cache key is the line's text, which the dictionary edit doesn't
    /// change. `add_to_user_dictionary` only ever inserts (deduplicated), so
    /// the set's length is a monotonic generation counter and needs no
    /// separate plumbing between `AppState` and this view.
    dict_len: usize,
    entries: HashMap<String, Rc<Vec<(usize, usize)>>>,
}

/// How many distinct lines the spellcheck memo holds before it's cleared.
///
/// Entries are keyed by line text and nothing ever removes one individually,
/// so without a bound a long editing session would accumulate every
/// intermediate state of every line ever typed. 4096 comfortably covers any
/// document's live line set plus a long tail of edits, and a clear costs one
/// re-check of the ~40 visible rows.
const SPELL_CACHE_MAX_LINES: usize = 4096;

/// Looks up (or computes and stores) one line's misspelled char-column ranges.
///
/// Free function rather than a method: its only caller is the `'static`
/// `uniform_list` closure, which holds a clone of the `Rc` cache rather than
/// `self`.
fn spell_ranges_cached(
    cache: &Rc<RefCell<SpellCache>>,
    line: &str,
    user_dictionary: &HashSet<String>,
) -> Rc<Vec<(usize, usize)>> {
    {
        let mut cache = cache.borrow_mut();
        // A dictionary addition invalidates every entry, since any of them
        // could contain the newly-accepted word.
        if cache.dict_len != user_dictionary.len() {
            cache.entries.clear();
            cache.dict_len = user_dictionary.len();
        } else if let Some(hit) = cache.entries.get(line) {
            return hit.clone();
        }
    }

    let ranges = Rc::new(crate::spellcheck::misspelled_ranges(line, user_dictionary));
    let mut cache = cache.borrow_mut();
    // Clear rather than evict: there's no recency data to evict *by*, and the
    // refill cost is one screenful of checks.
    if cache.entries.len() >= SPELL_CACHE_MAX_LINES {
        cache.entries.clear();
    }
    cache.entries.insert(line.to_string(), ranges.clone());
    ranges
}

/// See `TextEditor::move_cursor_to_row_edge`'s doc comment.
#[derive(Clone, Copy)]
enum RowEdge {
    Start,
    End,
    FirstNonBlank,
}

/// Resolves a display row (see `expand_rows_for_display`) to the nearest
/// real content row at or before it. A display row can be a blank spacer
/// slot reserved by an earlier oversized card-style/heading row (which has
/// no content of its own to land on), so this walks backward to the
/// nearest one that does — shared by `line_col_from_mouse_position` (a
/// click landing on a spacer slot) and H/M/L's row resolution (bug report:
/// H/M/L landed on the wrong row whenever a card-style row sat above the
/// viewport, from assuming every row was the same pixel height instead of
/// going through this same display-row translation).
fn nearest_wrap_row_for_display_row(display_to_wrap: &[Option<usize>], display_row: usize) -> usize {
    (0..=display_row).rev().find_map(|i| display_to_wrap[i]).unwrap_or(0)
}

/// Pure resolution of `RowEdge` into a char column within `line_chars`,
/// given the current visual row's `[row_start, row_end)` char range —
/// factored out of `TextEditor::move_cursor_to_row_edge` so this (the part
/// with an actual branch worth testing) doesn't need a live GPUI context.
fn row_edge_target_col(edge: RowEdge, line_chars: &[char], row_start: usize, row_end: usize) -> usize {
    match edge {
        RowEdge::Start => row_start,
        RowEdge::End => row_end,
        RowEdge::FirstNonBlank => line_chars
            .get(row_start..row_end.min(line_chars.len()))
            .unwrap_or(&[])
            .iter()
            .position(|c| !c.is_whitespace())
            .map(|i| row_start + i)
            .unwrap_or(row_end), // an all-whitespace row: land at its end, matching real vim's `^` on a blank line
    }
}

impl TextEditor {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self::for_pane(state, Pane::Primary, cx)
    }

    /// Resolves this editor's tab. `None` means the pane has nothing to show —
    /// the secondary pane while the split is closed.
    ///
    /// Every tab read in this file goes through here. After the split-view
    /// refactor there are deliberately *no* remaining `active_tab` reads in
    /// this file: a missed one would silently paint the other pane's document
    /// with no error, so "zero matches" is the greppable invariant that
    /// replaces checking by eye.
    fn tab_index(&self, cx: &App) -> Option<usize> {
        self.state.read(cx).pane_tab_index(self.pane)
    }

    pub fn for_pane(state: Entity<AppState>, pane: Pane, cx: &mut Context<Self>) -> Self {
        /*
         * Creates the text editor and registers a focus handle. Focus is claimed
         * lazily the first time the user clicks inside the editor.
         *
         * The `cx.focus_handle()` call creates a new entry in GPUI's focus registry;
         * the handle must be passed to `.track_focus()` in render() so the element
         * participates in the focus tree.
         */
        let focus_handle = cx.focus_handle();
        let uniform_list_scroll_handle = UniformListScrollHandle::new();
        let scroll_handle = uniform_list_scroll_handle.0.borrow().base_handle.clone();
        let auto_scroller = AutoScroller::new(scroll_handle.clone(), uniform_list_scroll_handle.clone(), state.clone());
        TextEditor {
            state,
            focus_handle,
            scroll_handle,
            uniform_list_scroll_handle,
            auto_scroller,
            macro_at_pending: false,
            row_cache: None,
            tab_scroll_offsets: std::collections::HashMap::new(),
            last_seen_active_tab: None,
            spell_cache: Rc::new(RefCell::new(SpellCache::default())),
            pane,
        }
    }

    fn scroll_to_cursor(&self, cx: &Context<Self>) {
        /*
         * Scrolls vertically so the cursor's visual row stays at least
         * `SCROLL_MARGIN_LINES` rows inside the visible viewport. Called
         * after every key event that could move the cursor.
         *
         * A wrapped logical line spans several visual rows, so the cursor's
         * document-space Y position is resolved via the same
         * `document_lines`/`build_visual_rows`/`visual_row_for_line_col`
         * pipeline `render()` and `line_col_from_mouse_position` use —
         * keeping all three in agreement about where each row actually sits.
         *
         * GPUI scroll offsets are ≤ 0: 0 means scrolled to the top, and
         * more-negative values mean the document has been scrolled further down.
         *
         * All positions here are in the same "content space" that
         * `line_col_from_mouse_position` uses: display row `i`'s top sits at
         * `i * LINE_HEIGHT_PX` (see `expand_rows_for_display` — the cursor's
         * *wrap* row is translated into this display-row space first, since
         * an oversized card-style/heading row earlier in the document
         * reserves extra blank spacer rows that push everything after it
         * down), with no padding baked into per-row offsets — padding is
         * only ever a one-time inset when converting to/from screen space.
         * `bounds().size.height` is the div's full border-box
         * height, which includes the top *and* bottom padding, so the actual
         * visible content window is `viewport_h - 2 * CONTENT_PADDING_PX`,
         * not the raw bounds height — using the raw height here previously
         * overestimated how much content was visible and let the cursor
         * drift below the real bottom edge before scrolling kicked in.
         *
         * The trigger checks use `margin` so scrolling begins while the
         * cursor is still comfortably visible, not only once it's already
         * clipped — otherwise a single keystroke can move the cursor from
         * "just visible" to "off-screen" with nothing to catch it. The
         * target offsets then re-open exactly `margin` worth of space on the
         * side being scrolled toward, so the buffer is restored rather than
         * just barely satisfied.
         *
         * The method is a no-op when the scroll handle has not been laid out
         * yet (viewport_h <= 0), which can happen on the very first frame.
         */
        let Some((cursor_top, viewport_h, max_y, offset_x, zoom, normal_size_px)) = self.cursor_scroll_geometry(cx) else { return };
        let line_height    = line_height_px(normal_size_px) * zoom;
        let cursor_bottom  = cursor_top + line_height;
        let margin         = SCROLL_MARGIN_LINES * line_height;

        let offset         = self.scroll_handle.offset();
        let visible_top    = -offset.y.as_f32();
        let visible_bottom = visible_top + viewport_h;

        if cursor_top < visible_top + margin {
            // Cursor is within `margin` of the top edge (or above it) —
            // scroll up so `margin` worth of buffer opens above the line.
            // Clamped to 0 so this can't scroll past the top of the document
            // just because the margin asked for space that doesn't exist yet.
            let new_y = (margin - cursor_top).clamp(-max_y.max(0.0), 0.0);
            self.scroll_handle.set_offset(point(offset_x, px(new_y)));
        } else if cursor_bottom > visible_bottom - margin {
            // Cursor is within `margin` of the bottom edge (or below it) —
            // scroll down so `margin` worth of buffer opens below the line.
            let new_y = (viewport_h - margin - cursor_bottom).clamp(-max_y.max(0.0), 0.0);
            self.scroll_handle.set_offset(point(offset_x, px(new_y)));
        }
    }

    /// Shared setup for `scroll_to_cursor` and `scroll_to_cursor_centered`:
    /// resolves the cursor's current visual row into content-space Y (same
    /// space `line_col_from_mouse_position` uses), plus the viewport height
    /// and max scroll offset needed to clamp any new offset. `None` when the
    /// scroll handle hasn't been laid out yet (viewport_h <= 0), which can
    /// happen on the very first frame.
    fn cursor_scroll_geometry(&self, cx: &Context<Self>) -> Option<(f32, f32, f32, Pixels, f32, f32)> {
        let state = self.state.read(cx);
        let (cursor_line, cursor_col) = state.pane_cursor_line_col(self.pane);
        let zoom = state.zoom;
        let normal_size_px = state.normal_text_size_half_points as f32 / 2.0;
        let _ = state;

        // `scroll_to_cursor` calls this on essentially every key event, so
        // reusing `RowCache` here (populated by the last `render()`, almost
        // always still valid — nothing between renders changes tab_id/
        // content_version/viewport_width/zoom) avoids paying a full-
        // document rewrap on every single keystroke, on top of render()'s
        // own now-cached cost.
        let viewport_width = self.scroll_handle.bounds().size.width.as_f32();
        let (rows, _, wrap_to_display) = self.cached_or_fresh_row_tables(cx, viewport_width);
        let visual_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
        // Translate into display-row space (see `expand_rows_for_display`)
        // so an oversized card-style/heading row earlier in the document
        // pushes this pixel position down by however many blank spacer
        // rows it reserved, matching what `render()` actually paints.
        let display_row = wrap_to_display[visual_row];
        let cursor_top = display_row as f32 * line_height_px(normal_size_px) * zoom;

        let viewport_h = self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
        if viewport_h <= 0.0 { return None; }

        let max_y = self.scroll_handle.max_offset().y.as_f32();
        Some((cursor_top, viewport_h, max_y, self.scroll_handle.offset().x, zoom, normal_size_px))
    }

    /// Returns the row tables the most recent `render()` already computed
    /// (`RowCache`, a handful of cheap `Rc::clone`s) when they're still
    /// valid for the given viewport width, instead of re-running the
    /// full-document wrap — used by `cursor_scroll_geometry` and the
    /// click/drag hit-testing handlers below (`performance_plan.md`'s
    /// "route hit-testing through the cache" item). Falls back to a fresh,
    /// uncached computation on a miss (e.g. the very first render hasn't
    /// happened yet) — this method doesn't own populating `row_cache`,
    /// `render()` does, and it runs again on the next frame regardless.
    fn cached_or_fresh_row_tables(
        &self,
        cx: &Context<Self>,
        viewport_width: f32,
    ) -> (Rc<Vec<(usize, usize, usize)>>, Rc<Vec<Option<usize>>>, Rc<Vec<usize>>) {
        let idx = self.tab_index(cx);
        let state = self.state.read(cx);
        let dragging = state.split_dragging;
        let invisibility = state.invisibility_mode;
        let cite_size = state.cite_size_half_points;
        let fold_version = idx.and_then(|i| state.tabs.get(i)).map(|t| t.fold_version).unwrap_or(0);
        let folds = idx
            .and_then(|i| state.tabs.get(i))
            .map(|t| t.folded_headings.clone())
            .unwrap_or_default();
        let tab_id = idx.and_then(|i| state.tabs.get(i)).map(|t| t.id).unwrap_or(usize::MAX);
        let content_version = idx.and_then(|i| state.tabs.get(i)).map(|t| t.content_version).unwrap_or(0);
        let zoom = state.zoom;
        if let Some(cache) = self.row_cache.as_ref() {
            if row_cache_is_valid_for(cache, tab_id, content_version, viewport_width, zoom, dragging, invisibility, fold_version) {
                return (cache.rows.clone(), cache.display_to_wrap.clone(), cache.wrap_to_display.clone());
            }
        }
        let content = state.pane_content(self.pane).to_string();
        let paragraphs = idx.and_then(|i| state.tabs.get(i)).map(|t| t.paragraphs.clone()).unwrap_or_default();
        let normal_size_px = state.normal_text_size_half_points as f32 / 2.0;
        let lines = document_lines(&content);
        let rows = Rc::new(visual_rows_for_viewport(
            cx, &lines, viewport_width, zoom, &paragraphs, normal_size_px,
        ));
        let folded_paras = AppState::folded_paragraphs(&paragraphs, &folds);
        let hidden = hidden_wrap_rows(&rows, &paragraphs, invisibility, cite_size, &folded_paras);
        let (display_to_wrap, wrap_to_display) =
            expand_rows_for_display(&rows, &paragraphs, zoom, &hidden, normal_size_px);
        (rows, Rc::new(display_to_wrap), Rc::new(wrap_to_display))
    }

    /// Unlike `scroll_to_cursor` (which only nudges the viewport when the
    /// cursor is near an edge), this always repositions the cursor's line to
    /// the vertical center of the viewport. Used exclusively by the Nav
    /// menu's jump-to-heading (`AppState::jump_to_line`, consumed via
    /// `Tab.pending_scroll_to_cursor` in `render()` below) — landing back on
    /// an already-visible line with no scroll at all reads as "nothing
    /// happened" even though the cursor did move, which defeats the point
    /// of clicking a heading to jump to it.
    /// Reading mode's Left/Right paging: moves the viewport by exactly one
    /// screenful of whole rows.
    ///
    /// Advancing by `floor(viewport / row_height)` rows rather than by the raw
    /// viewport height is what makes the two guarantees hold together. A raw
    /// pixel jump lands mid-row, so the line straddling the fold would be
    /// sliced — half of it scrolled past unread. Rounding down to whole rows
    /// means the next page starts exactly at the first row that wasn't fully
    /// visible: nothing is skipped, and nothing fully-read is shown twice. A
    /// row that was only *partially* visible at the bottom reappears whole at
    /// the top, which is the safe direction to err.
    ///
    /// Returns false when there is nothing to scroll (not laid out yet, or
    /// already at the end in that direction), so the caller can let the key
    /// fall through to its normal meaning.
    fn page_scroll(&self, forward: bool, cx: &Context<Self>) -> bool {
        let state = self.state.read(cx);
        let zoom = state.zoom;
        let normal_size_px = state.normal_text_size_half_points as f32 / 2.0;
        let row_height = line_height_px(normal_size_px) * zoom;
        if row_height <= 0.0 {
            return false;
        }
        let viewport_h = self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
        if viewport_h <= 0.0 {
            return false;
        }
        let offset = self.scroll_handle.offset();
        let max_y = self.scroll_handle.max_offset().y.as_f32();
        let current = offset.y.as_f32();
        let Some(next) = page_scroll_offset(current, viewport_h, row_height, max_y, forward) else {
            return false; // already at that end
        };
        self.scroll_handle.set_offset(point(offset.x, px(next)));
        true
    }

    fn scroll_to_cursor_centered(&self, cx: &Context<Self>) {
        let Some((cursor_top, viewport_h, max_y, offset_x, zoom, normal_size_px)) = self.cursor_scroll_geometry(cx) else { return };
        let target_visible_top = cursor_top - (viewport_h - line_height_px(normal_size_px) * zoom) / 2.0;
        let new_y = (-target_visible_top).clamp(-max_y.max(0.0), 0.0);
        self.scroll_handle.set_offset(point(offset_x, px(new_y)));
    }

    /// Real vim's `zt`: scrolls so the cursor's line sits at the top edge
    /// of the viewport. Shares `cursor_scroll_geometry` with
    /// `scroll_to_cursor_centered` above — see that function's doc comment
    /// for why this needs live GPUI viewport geometry rather than living in
    /// `AppState`.
    fn scroll_to_cursor_top(&self, cx: &Context<Self>) {
        let Some((cursor_top, _viewport_h, max_y, offset_x, _zoom, _normal_size_px)) = self.cursor_scroll_geometry(cx) else { return };
        let new_y = (-cursor_top).clamp(-max_y.max(0.0), 0.0);
        self.scroll_handle.set_offset(point(offset_x, px(new_y)));
    }

    /// Real vim's `zb`: scrolls so the cursor's line sits at the bottom
    /// edge of the viewport.
    fn scroll_to_cursor_bottom(&self, cx: &Context<Self>) {
        let Some((cursor_top, viewport_h, max_y, offset_x, zoom, normal_size_px)) = self.cursor_scroll_geometry(cx) else { return };
        let target_visible_top = cursor_top - (viewport_h - line_height_px(normal_size_px) * zoom);
        let new_y = (-target_visible_top).clamp(-max_y.max(0.0), 0.0);
        self.scroll_handle.set_offset(point(offset_x, px(new_y)));
    }

    /// Which edge of the cursor's current *visual* row to jump to — shared
    /// by vim's bare `$`/`0`/`^` (Normal/Visual mode) and the plain
    /// `Home`/`End` keys (both vim-disabled and vim's Insert mode). Real
    /// vim's `$`/`0`/`^`/Home/End all target the *logical* line; this app
    /// deliberately inverts that, the same way `j`/`k` already do (see
    /// `move_cursor_visual_row`'s own doc comment) — debate case files are
    /// typically one long wrapped paragraph per card, so jumping to the
    /// literal start/end of the whole paragraph reads as the cursor
    /// teleporting off-screen instead of "go to the edge of this line."
    /// `g$`/`g0`/`g^` (`state.rs`'s `resolve_vim_motion`) are the escape
    /// hatch back to the true logical-line target when it's actually
    /// wanted.
    fn move_cursor_to_row_edge(&self, cx: &mut Context<Self>, edge: RowEdge, extend: bool) {
        let idx = self.tab_index(cx);
        let state = self.state.read(cx);
        let content = state.pane_content(self.pane).to_string();
        let (cursor_line, cursor_col) = state.pane_cursor_line_col(self.pane);
        let zoom = state.zoom;
        let normal_size_px = state.normal_text_size_half_points as f32 / 2.0;
        let paragraphs = idx.and_then(|i| state.tabs.get(i)).map(|t| t.paragraphs.clone()).unwrap_or_default();
        let _ = state;

        let lines = document_lines(&content);
        let rows = visual_rows_for_viewport(
            cx,
            &lines,
            self.scroll_handle.bounds().size.width.as_f32(),
            zoom,
            &paragraphs,
            normal_size_px,
        );
        let current_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
        let (line, row_start, row_end) = rows[current_row];
        let line_chars: Vec<char> = lines.get(line).map(|l| l.chars().collect()).unwrap_or_default();
        let target_col = row_edge_target_col(edge, &line_chars, row_start, row_end);

        self.state.update(cx, |state, cx| {
            if extend {
                state.extend_selection_to_line_col(line, target_col);
            } else {
                state.set_cursor_from_line_col(line, target_col);
            }
            cx.notify();
        });
        self.scroll_to_cursor(cx);
    }

    fn move_cursor_visual_row(&self, cx: &mut Context<Self>, delta: isize, extend: bool) {
        /*
         * Moves the cursor to the visual row `delta` rows above/below its
         * current one (-1/+1 for Up/Down), preserving its on-screen column
         * rather than its logical-line column.
         *
         * Without this, pressing Up from the row directly below a wrapped
         * line would jump to the very first character of the line above
         * (using that *logical* line's column), skipping right past its
         * wrapped continuation rows entirely — landing on the wrong visual
         * spot on screen. This rebuilds the same row table `render()`
         * paints from, so "the row above" here always matches what's
         * actually drawn one row up on screen.
         *
         * No-op past the first/last visual row. `extend` selects between
         * `set_cursor_from_line_col` (Up/Down) and
         * `extend_selection_to_line_col` (Shift+Up/Down), mirroring every
         * other motion's plain/extending pair.
         */
        let idx = self.tab_index(cx);
        let state = self.state.read(cx);
        let content = state.pane_content(self.pane).to_string();
        let (cursor_line, cursor_col) = state.pane_cursor_line_col(self.pane);
        let zoom = state.zoom;
        let normal_size_px = state.normal_text_size_half_points as f32 / 2.0;
        let paragraphs = idx.and_then(|i| state.tabs.get(i)).map(|t| t.paragraphs.clone()).unwrap_or_default();
        let _ = state;

        let lines = document_lines(&content);
        let rows = visual_rows_for_viewport(
            cx,
            &lines,
            self.scroll_handle.bounds().size.width.as_f32(),
            zoom,
            &paragraphs,
            normal_size_px,
        );

        let current_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
        let (_, row_start, _) = rows[current_row];
        let col_in_row = cursor_col - row_start;

        let Some((target_line, target_col)) = visual_row_step(&rows, current_row, col_in_row, delta, &paragraphs, normal_size_px, zoom) else {
            return; // no-op past the first/last visual row
        };

        self.state.update(cx, |state, cx| {
            if extend {
                state.extend_selection_to_line_col(target_line, target_col);
            } else {
                state.set_cursor_from_line_col(target_line, target_col);
            }
            cx.notify();
        });
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Dispatches raw key-down events to `process_key`, which does the
         * actual work — split out so macro replay (`@<register>`, a
         * user-requested feature not part of editor_instructions.md) can
         * re-invoke the exact same dispatch for a recorded keystroke
         * without a real `KeyDownEvent` to hand it.
         */
        // Any real keystroke dismisses the right-click menu — it's a mouse
        // gesture's transient state, and leaving it floating over text the
        // user has started typing into reads as a stuck overlay. The key is
        // still dispatched normally below, so this never swallows input.
        //
        // Here rather than in `process_key`: that one is also the macro-replay
        // path (`@<register>`), which has no live menu to dismiss and
        // shouldn't pay for the check per replayed keystroke.
        if self.state.read(cx).editor_context_menu.is_some() {
            self.state.update(cx, |s, cx| {
                s.editor_context_menu = None;
                cx.notify();
            });
        }

        // Likewise for a "Select similar formatting" highlight — its ranges are
        // raw byte offsets that can't follow an edit, so they must not outlive
        // the keystroke. Only *unbound* keys get here at all: GPUI dispatches
        // matched key bindings first and its action handlers stop propagation
        // (`App::on_action`'s bubble phase), so Bold/Highlight/etc. still act on
        // the whole match set, which is the entire point of the feature.
        if self
            .state
            .read(cx)
            .tabs
            .get(self.tab_index(cx).unwrap_or(usize::MAX))
            .is_some_and(|t| !t.similar_ranges.is_empty())
        {
            self.state.update(cx, |s, cx| {
                s.clear_similar_selection();
                cx.notify();
            });
        }

        let ks = &event.keystroke;

        // Reading mode: Left/Right page the viewport instead of moving the
        // caret. Only for unmodified presses, so Shift-select and any
        // Ctrl/Cmd combination keep their normal meaning; and only when the
        // scroll actually moved, so at the end of the document the key still
        // falls through to ordinary cursor movement.
        let plain_arrow = !ks.modifiers.shift && !ks.modifiers.control && !ks.modifiers.platform;
        if self.state.read(cx).read_mode && plain_arrow {
            let forward = match ks.key.as_str() {
                "right" => Some(true),
                "left" => Some(false),
                _ => None,
            };
            if let Some(forward) = forward {
                if self.page_scroll(forward, cx) {
                    cx.notify();
                    return;
                }
            }
        }

        self.process_key(&ks.key, ks.modifiers.shift, ks.modifiers.control, ks.modifiers.platform, ks.key_char.as_deref(), window, cx);
    }

    fn process_key(&mut self, key: &str, shift: bool, control: bool, platform: bool, key_char: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        /*
         * The actual key-handling logic `handle_key_down` used to contain
         * directly, now parameterized so both a live `KeyDownEvent` and a
         * replayed macro keystroke (which has no `KeyDownEvent` to unpack)
         * funnel through the same path.
         *
         * Platform-modifier (Ctrl/Cmd) combinations are deliberately passed
         * through so global actions (toggle-settings, new-tab, etc.) can
         * fire normally — and deliberately excluded from macro-recording
         * capture below, an explicit scope decision (macros cover vim's
         * own keystroke stream, not app-global shortcuts).
         * Only pure character input, space, enter, tab, and backspace are
         * consumed. scroll_to_cursor is called at every exit point so the
         * cursor line stays visible regardless of which key moved it.
         */
        if control || platform {
            self.process_key_ctrl_combo(key, shift, cx);
            return;
        }

        // Macro recording capture (user-requested `q`/`@` macros, not part
        // of the written spec): record this keystroke iff a
        // recording was already active *before* dispatch and is *still*
        // active *after* — excluding the `q<register>` pair that starts a
        // recording (not yet active beforehand) and the bare `q` that ends
        // one (no longer active afterward), so only the macro's actual
        // content is captured.
        let was_recording = self.state.read(cx).vim_is_recording_macro();
        // `.`-repeat change capture (spec 5.5) — unlike macro recording,
        // this appends *before* dispatch, since the keystroke that
        // completes the operator (ending the recording) must still be
        // captured; see `vim_is_recording_change`'s doc comment.
        if self.state.read(cx).vim_is_recording_change() {
            self.state.update(cx, |state, _cx| state.record_change_key(key, shift, key_char));
        }
        self.process_key_plain(key, shift, key_char, window, cx);
        if was_recording && self.state.read(cx).vim_is_recording_macro() {
            self.state.update(cx, |state, _cx| state.record_macro_key(key, shift, key_char));
        }
    }

    fn process_key_ctrl_combo(&mut self, key: &str, shift: bool, cx: &mut Context<Self>) {
        /*
         * Handles Ctrl/Cmd-modified keystrokes — split out of `process_key`
         * so its early `return` doesn't also need to skip macro-recording
         * capture (Ctrl combos are app-global shortcuts, not part of vim's
         * own keystroke stream, and are never recorded into a macro).
         *
         * Copy/Cut/Paste/Undo/Redo/SelectAll/Bold/Underline used to be
         * hardcoded here. They're now configurable GPUI actions
         * (`src/keybinds.rs`, handled in `main_window.rs`) — leaving them
         * here too would have them permanently shadowed anyway: GPUI stops
         * an event's propagation once a keybinding's action handler runs,
         * so this raw key-event path never actually fired for them once a
         * matching binding existed (confirmed the hard way — Ctrl+B here
         * never fired while Ctrl+B was also bound to ToggleSidebar).
         */
        match key {
            "o" => {
                // Ctrl+O: jump list back (spec 5.5). Vim-specific, out of
                // scope for the configurable keybind system.
                self.state.update(cx, |state, _cx| state.vim_jump_backward());
                cx.notify();
                self.scroll_to_cursor(cx);
            }
            "i" => {
                // Ctrl+I: jump list forward (spec 5.5). Vim-specific, out of
                // scope for the configurable keybind system.
                self.state.update(cx, |state, _cx| state.vim_jump_forward());
                cx.notify();
                self.scroll_to_cursor(cx);
            }
            // Ctrl+R: real vim's Redo, in Normal mode only — the app's own
            // configurable Redo keybind defaults to Ctrl+Y instead (see
            // `keybinds.rs`), so this doesn't collide with it; Ctrl+R itself
            // has no default binding and was previously a complete no-op
            // here. Gated the same way `process_key_plain` reads vim state,
            // just below, since this function otherwise reads none.
            "r" => {
                let (vim_enabled, vim_mode) = {
                    let state = self.state.read(cx);
                    let mode = self.tab_index(cx)
                        .and_then(|i| state.tabs.get(i))
                        .map(|t| t.vim_mode)
                        .unwrap_or(VimMode::Insert);
                    (state.vim_enabled, mode)
                };
                if vim_enabled && vim_mode == VimMode::Normal {
                    self.state.update(cx, |state, _cx| state.redo());
                    cx.notify();
                }
            }
            // Ctrl+Left/Right jump by word; Ctrl+Home/End jump to document start/end
            // (spec 4.1). Shift+Ctrl+<key> extends the selection instead of just
            // moving (spec 4.3). Plain (unmodified) arrow/Home/End are handled below.
            "left" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_word_backward() } else { state.move_word_backward() }
                });
                cx.notify();
            }
            "right" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_word_forward() } else { state.move_word_forward() }
                });
                cx.notify();
            }
            "home" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_doc_start() } else { state.move_doc_start() }
                });
                cx.notify();
            }
            "end" => {
                self.state.update(cx, |state, _cx| {
                    if shift { state.extend_doc_end() } else { state.move_doc_end() }
                });
                cx.notify();
            }
            // Ctrl+Backspace deletes the previous word (spec bugfix/QoL
            // task 8). Content-editing key, so it lives here rather than
            // the global keybind/action system (`src/keybinds.rs`) —
            // it only makes sense with editor focus, same reasoning as
            // Ctrl+Left/Right/Home/End/O/I above.
            "backspace" => {
                self.state.update(cx, |state, cx| {
                    state.delete_word_backward();
                    cx.notify();
                });
            }
            _ => {} // Ctrl+S, Ctrl+T, Ctrl+W, etc. handled by global actions
        }
        self.scroll_to_cursor(cx);
    }

    fn process_key_plain(&mut self, key: &str, shift: bool, key_char: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Handles every non-Ctrl/Cmd keystroke: vim-mode routing plus the
         * plain-editor fallback. Split out of `process_key` so macro
         * recording can wrap this call without also capturing Ctrl combos
         * (handled separately by `process_key_ctrl_combo`).
         */
        // Vim mode routing (Task D). Insert mode behaves like the plain
        // editor below except for Escape, which nothing in the plain-editor
        // match block otherwise handles. The other four modes route through
        // handle_vim_key first; it returns false only for Normal-mode
        // navigation keys it deliberately lets fall through (see its own
        // doc comment) — everything else it returns true for is fully
        // handled here and shouldn't reach the plain-editor logic below.
        let (vim_enabled, vim_mode) = {
            let idx = self.tab_index(cx);
            let state = self.state.read(cx);
            let mode = idx.and_then(|i| state.tabs.get(i)).map(|t| t.vim_mode).unwrap_or_default();
            (state.vim_enabled, mode)
        };
        if vim_enabled {
            if vim_mode == VimMode::Insert {
                if key == "escape" {
                    self.state.update(cx, |state, _cx| state.vim_exit_to_normal());
                    cx.notify();
                    self.scroll_to_cursor(cx);
                    return;
                }
                // else: fall through to the plain-editor handling below.
            } else {
                // 'j'/'k' need the current viewport's wrap layout (GPUI
                // context `handle_vim_key` doesn't have), so they're
                // special-cased here rather than dispatched through
                // AppState — mirroring how plain Up/Down are handled below,
                // and reusing the same visual-row-aware movement so j/k
                // feel identical to the arrow keys on this app's wrapped
                // content rather than vim's logical-line semantics (a
                // deliberate UX choice for this heavily-wrapping app).
                // Intercepted in Normal mode (moves the cursor) and Visual/
                // VisualLine (extends the selection, spec 5.6) with no
                // pending find/gg trigger *and* no pending `d`/`y`/`c`
                // operator — otherwise 'j'/'k' must reach `handle_vim_key`
                // so a pending `f`/`t` can treat them as a target character
                // (e.g. completing `fj`), or a pending operator can abandon
                // itself cleanly via `complete_vim_operator` (without this,
                // `dj` would silently move the cursor via
                // `move_cursor_visual_row` below and leave `d` dangling for
                // the *next* keystroke to complete instead). Also gated on
                // `!shift` (Task I) — shift+j is `J` (join lines, spec
                // 5.5), a completely different command that must reach
                // `handle_vim_key` instead of being swallowed as "move down".
                let no_pending_trigger = self.state.read(cx).vim_pending_trigger().is_none()
                    && self.state.read(cx).vim_pending_operator().is_none();
                let is_visual = matches!(vim_mode, VimMode::Visual | VimMode::VisualLine);
                if (vim_mode == VimMode::Normal || is_visual) && no_pending_trigger && !shift && (key == "j" || key == "k") {
                    let count = self.state.update(cx, |state, _cx| state.take_vim_count()).unwrap_or(1);
                    let delta: isize = if key == "k" { -1 } else { 1 };
                    for _ in 0..count {
                        self.move_cursor_visual_row(cx, delta, is_visual);
                    }
                    self.scroll_to_cursor(cx);
                    return;
                }

                // H/M/L: top/middle/bottom of the *visible* viewport (spec
                // 5.2) — needs the live scroll offset and visual-row
                // layout, same GPUI-context reason as j/k above. Resolves
                // down to a plain logical line number and hands off to
                // `vim_move_to_line_first_nonblank`, which doesn't need to
                // know anything about viewports.
                //
                // Bug report: landed on the wrong row whenever a card-style
                // row (Pocket/Block/Tag/Cite/heading) sat above the
                // viewport. Root cause: dividing the raw scroll offset by a
                // single `line_height` assumes every row is the same
                // height, but an oversized row reserves extra blank
                // *display* spacer rows (`expand_rows_for_display`) that
                // push everything after it down — `cursor_scroll_geometry`
                // already accounts for this by translating through
                // `display_to_wrap`; this now does the same translation in
                // the opposite direction (pixels -> display row -> real
                // content row), reusing the identical "a spacer slot
                // belongs to the nearest real row before it" rule
                // `line_col_from_mouse_position` already established for
                // clicks landing on one.
                if (vim_mode == VimMode::Normal || is_visual) && no_pending_trigger
                    && shift && matches!(key, "h" | "m" | "l")
                {
                    let zoom = self.state.read(cx).zoom;
                    let normal_size_px = self.state.read(cx).normal_text_size_half_points as f32 / 2.0;
                    let viewport_width = self.scroll_handle.bounds().size.width.as_f32();
                    let (rows, display_to_wrap, _) = self.cached_or_fresh_row_tables(cx, viewport_width);
                    if !rows.is_empty() && !display_to_wrap.is_empty() {
                        let line_height = line_height_px(normal_size_px) * zoom;
                        let viewport_h = self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
                        let offset = self.scroll_handle.offset();
                        let last_display_row = display_to_wrap.len() - 1;
                        let top_display = (((-offset.y.as_f32()) / line_height).floor().max(0.0) as usize).min(last_display_row);
                        let visible_count = ((viewport_h / line_height).floor().max(1.0)) as usize;
                        let bottom_display = (top_display + visible_count.saturating_sub(1)).min(last_display_row);
                        let top_row = nearest_wrap_row_for_display_row(&display_to_wrap, top_display);
                        let bottom_row = nearest_wrap_row_for_display_row(&display_to_wrap, bottom_display);
                        let target_row = match key {
                            "h" => top_row,
                            "l" => bottom_row,
                            "m" => top_row + bottom_row.saturating_sub(top_row) / 2,
                            _ => unreachable!(),
                        };
                        let target_line = rows[target_row].0;
                        self.state.update(cx, |state, cx| {
                            state.vim_move_to_line_first_nonblank(target_line, is_visual);
                            cx.notify();
                        });
                        cx.notify();
                        self.scroll_to_cursor(cx);
                        return;
                    }
                }

                // Real vim's `zz`/`zt`/`zb` (center/scroll-to-top/scroll-
                // to-bottom the viewport on the cursor's line) — needs live
                // scroll-handle geometry, same GPUI-context reason j/k and
                // H/M/L above are intercepted here rather than reaching
                // `AppState::handle_vim_key`. Piggybacks on the z-leader
                // custom-keybind buffer (`vim_keybind_seq`) that the first
                // `z` keystroke already starts via the ordinary catch-all
                // below — only *after* that buffer already holds exactly
                // "z" does this claim the second keystroke, so every other
                // zX default/custom binding (zs, zn, zy, ...) is completely
                // unaffected and still resolves through the normal path.
                // `zz`/`zt`/`zb` are deliberately no longer in
                // `VimKeybinds::defaults()` (see `vim_keybinds.rs`'s
                // `NATIVE_VIM_SEQUENCES`) — without this intercept they'd
                // fall through to `continue_vim_keybind_sequence`, resolve
                // to `VimLookup::None`, and silently do nothing, which is
                // exactly the reported bug.
                if vim_mode == VimMode::Normal && no_pending_trigger && !shift && matches!(key, "z" | "t" | "b") {
                    let pending_z = self
                        .tab_index(cx)
                        .and_then(|i| self.state.read(cx).tabs.get(i).map(|t| t.vim_keybind_seq == "z"))
                        .unwrap_or(false);
                    if pending_z {
                        self.state.update(cx, |state, _cx| {
                            if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                                tab.vim_keybind_seq.clear();
                            }
                        });
                        match key {
                            "z" => self.scroll_to_cursor_centered(cx),
                            "t" => self.scroll_to_cursor_top(cx),
                            "b" => self.scroll_to_cursor_bottom(cx),
                            _ => unreachable!(),
                        }
                        // Bug report: the view didn't move until the *next*
                        // keystroke. The scroll helpers only call
                        // `scroll_handle.set_offset` — they never request a
                        // repaint, and this is the one branch in this file
                        // that changes the offset without also mutating state.
                        // Every other caller gets its frame for free from an
                        // accompanying `state.update(.., cx.notify())` (the
                        // `scroll_to_cursor` sites) or notifies explicitly
                        // (read mode's `page_scroll`, `:879`); the
                        // `state.update` above takes `_cx` because it only
                        // clears the sequence buffer. So nothing scheduled a
                        // frame, and the new offset sat unpainted until an
                        // unrelated key (w/e/b) caused one.
                        //
                        // Deliberately notified here rather than inside the
                        // three helpers: `scroll_to_cursor_centered` is also
                        // called from inside `render()` (the
                        // `pending_scroll_to_cursor` drain), where notifying
                        // would dirty the view mid-paint and cost an extra
                        // frame every time the Nav menu jumps to a line.
                        cx.notify();
                        return;
                    }
                }

                // Bug report: `$` jumped to the end of the whole wrapped
                // paragraph instead of the current visual row — same
                // "logical line vs visual row" inversion `j`/`k` already
                // make for this heavily-wrapping app (see
                // `move_cursor_to_row_edge`'s doc comment). `0`/`^`/Home/End
                // get the identical treatment for consistency; `g$`/`g0`/
                // `g^` (`state.rs`) reach the original logical-line target.
                // Deliberately does NOT cover an operator's pending target
                // (`d$`/`c$`/`D`/`C`) — confirmed with the reporter that
                // those should keep deleting to the end of the paragraph,
                // which is exactly what gating this on `no_pending_trigger`
                // (already excludes a pending operator) achieves: with one
                // pending, this block is skipped and the key falls through
                // to `handle_vim_key` below, which still resolves `$`/`0`/
                // `^`/Home/End through the unchanged, logical-line
                // `resolve_vim_motion` arms.
                if (vim_mode == VimMode::Normal || is_visual) && no_pending_trigger {
                    // Bare "0" only means "start of row" when no count
                    // digits are already being typed — "10" must still
                    // accumulate as count 10, not treat its second "0" as
                    // a motion.
                    let buf_empty = self
                        .tab_index(cx)
                        .and_then(|i| self.state.read(cx).tabs.get(i).map(|t| t.vim_command_buf.is_empty()))
                        .unwrap_or(true);
                    let edge = if matches_shifted_symbol(key, shift, key_char, "4", "$") || key == "end" {
                        Some(RowEdge::End)
                    } else if matches_shifted_symbol(key, shift, key_char, "6", "^") {
                        Some(RowEdge::FirstNonBlank)
                    } else if key == "home" || (key == "0" && !shift && buf_empty) {
                        Some(RowEdge::Start)
                    } else {
                        None
                    };
                    if let Some(edge) = edge {
                        self.move_cursor_to_row_edge(cx, edge, is_visual);
                        return;
                    }
                }

                // `@`/`@<register>`/`@@` macro replay (user-requested, not
                // part of editor_instructions.md) — kept entirely here
                // rather than in `AppState::handle_vim_key`
                // since replaying re-enters `process_key` with full GPUI
                // context, which `AppState` doesn't have. Normal-mode only
                // (unlike `q` recording start/stop, which — being purely
                // state bookkeeping with no GPUI dependency — lives in
                // `AppState` and is reachable from Visual mode too via the
                // shared dispatcher; narrowing replay to Normal mode is a
                // deliberate, documented scope limit for this pass).
                if vim_mode == VimMode::Normal && no_pending_trigger {
                    if self.macro_at_pending {
                        self.macro_at_pending = false;
                        if let Some(register) = vim_find_target_char(key, shift, key_char) {
                            let register = if register == '@' {
                                self.state.read(cx).vim_last_macro_register
                            } else {
                                Some(register)
                            };
                            if let Some(register) = register {
                                self.replay_macro(register, window, cx);
                            }
                        }
                        self.scroll_to_cursor(cx);
                        return;
                    }
                    if matches_shifted_symbol(key, shift, key_char, "2", "@") {
                        self.macro_at_pending = true;
                        return;
                    }
                }

                // `"+p`/`"+P` (spec 5.8's clipboard register, read
                // direction): `state.rs` can't reach the OS clipboard
                // itself, so when the `+` register is about to be pasted
                // from, read it here (this is the only layer with `cx`)
                // and stage it into the register the ordinary,
                // GPUI-unaware paste path already knows how to read.
                if (key == "p") && self.state.read(cx).vim_selected_register() == Some('+') {
                    if let Some(item) = cx.read_from_clipboard() {
                        if let Some(text) = item.text() {
                            self.state.update(cx, |state, _cx| state.set_register('+', text.to_string()));
                        }
                    }
                }

                let (consumed, clipboard_sync, vim_action) = self.state.update(cx, |state, cx| {
                    let handled = state.handle_vim_key(key, shift, key_char);
                    if handled { cx.notify(); }
                    (handled, state.take_pending_clipboard_sync(), state.take_pending_vim_action())
                });
                // `"+y`/`"+d`/`"+c` (write direction): mirrors the read
                // direction above — `execute_vim_operator_range` stages the
                // text in `pending_clipboard_sync` when the `+` register
                // was targeted; this is the only place with `cx` to
                // actually push it onto the OS clipboard.
                if let Some(text) = clipboard_sync {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
                // Checklist: Settings -> Vim Mode. Same mailbox pattern as
                // `clipboard_sync` above — `state.rs` staged the resolved
                // `KeybindAction`, this is the one place with `window`+`cx`
                // to actually fire it, via the same `dispatch_action` call
                // `app_toolbar.rs`'s toolbar buttons already use.
                if let Some(action) = vim_action {
                    window.dispatch_action(crate::keybinds::action_for(action), cx);
                }
                if consumed {
                    cx.notify();
                    self.scroll_to_cursor(cx);
                    return;
                }
                // else: a Normal-mode navigation key fell through — continue
                // below to the same handling the plain editor uses.
            }
        }

        // Up/Down move by *visual* row (not logical line) so wrapped lines'
        // continuation rows are reachable — handled separately from the
        // match below since it needs the current viewport's wrap layout,
        // not just a plain AppState mutation. Extends instead of moving
        // when Shift is held (the plain editor's own convention) OR vim is
        // in Visual/VisualLine mode — reached here (rather than being
        // handled above) precisely when vim's own j/k branch let Up/Down
        // fall through, which requires extending too or it would silently
        // clear the active selection via a plain, non-extending move.
        if key == "up" || key == "down" {
            let vim_visual = vim_enabled && matches!(vim_mode, VimMode::Visual | VimMode::VisualLine);
            let delta = if key == "up" { -1 } else { 1 };
            self.move_cursor_visual_row(cx, delta, shift || vim_visual);
            self.scroll_to_cursor(cx);
            return;
        }

        // Home/End, reached here rather than above precisely when vim is
        // disabled or in Insert mode — the vim-Normal/Visual case is
        // already handled above (same visual-row logic, see
        // `move_cursor_to_row_edge`'s doc comment), and an operator-pending
        // Home/End (`dEnd`) is consumed before ever reaching this point.
        if key == "home" || key == "end" {
            let edge = if key == "home" { RowEdge::Start } else { RowEdge::End };
            self.move_cursor_to_row_edge(cx, edge, shift);
            return;
        }

        let consumed = self.state.update(cx, |state, cx| {
            match key {
                "backspace" => { state.backspace(); cx.notify(); true }
                "delete"    => { state.delete_forward(); cx.notify(); true }
                "enter"     => { state.insert_char('\n'); cx.notify(); true }
                "space"     => { state.insert_char(' '); cx.notify(); true }
                "tab"       => { state.insert_char('\t'); cx.notify(); true }
                // Shift+<key> extends the selection instead of moving plainly (spec 4.3).
                "left"      => { if shift { state.extend_left() } else { state.move_left() }; cx.notify(); true }
                "right"     => { if shift { state.extend_right() } else { state.move_right() }; cx.notify(); true }
                k if k.chars().count() == 1 => {
                    let mut ch = k.chars().next().unwrap();
                    // Apply shift for uppercase; GPUI gives lowercase key names
                    if shift && ch.is_alphabetic() {
                        ch = ch.to_uppercase().next().unwrap_or(ch);
                    }
                    state.insert_char(ch);
                    cx.notify();
                    true
                }
                _ => false,
            }
        });
        if consumed { cx.notify(); }
        self.scroll_to_cursor(cx);
    }

    fn replay_macro(&mut self, register: char, window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Replays a recorded macro (`@<register>`) by feeding
         * its captured keystrokes back through `process_key` one at a
         * time, in order — the same function a live keypress reaches, so
         * replay re-triggers the exact same mode-aware routing (Insert/
         * Normal/Visual, motions, H/M/L, j/k, etc.) a real keystroke would.
         *
         * The key vector is read and cloned *before* the loop starts, with
         * that borrow fully released before any `process_key` call — each
         * of those does its own `self.state.update`/`read`, and GPUI
         * panics if one of those runs while another is still open on the
         * same entity, which would happen if this loop were written inside
         * a `self.state.update(...)` closure instead.
         */
        self.state.update(cx, |state, _cx| { state.vim_last_macro_register = Some(register); });
        let Some(keys) = self.state.read(cx).macro_keys(register) else { return };
        for k in keys {
            self.process_key(&k.key, k.shift, false, false, k.key_char.as_deref(), window, cx);
        }
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the editor as a focusable, scrollable column.
         *
         * Content is split on '\n' into logical lines, then each logical
         * line is word-wrapped into one or more fixed-height visual rows via
         * `build_visual_rows` — this is what actually fixes long lines
         * running off the right edge instead of wrapping. One div is
         * painted per visual row, not per logical line, which keeps every
         * row exactly `LINE_HEIGHT_PX` tall so click-to-position and
         * scroll-to-cursor's pixel math (which assume a fixed row height)
         * stay correct even when lines wrap.
         *
         * The row `tab.cursor` actually points into is rendered as three
         * inline spans (text before / cursor cell / text after) so the cursor
         * marker sits at the real character position, rather than always
         * trailing the last line regardless of where the cursor is.
         *
         * Clicking anywhere in the editor reclaims keyboard focus.
         */
        // Tab switch (TabBar's on_click -> `set_active_tab`) and file-open
        // (sidebar, new-tab) never touch GPUI keyboard focus directly — they
        // only flip `active_tab` on the shared AppState. Left alone, the
        // text editor's FocusHandle stays wherever it was (often nowhere),
        // so Enter/keys silently stop reaching `handle_key_down` until the
        // user clicks into the editor again. Honor and clear the request
        // here, once per frame, mirroring `pending_scroll_to_cursor` below.
        // Only when *this* pane is the one being asked for — with two editors
        // mounted, an unqualified flag lets whichever renders first steal the
        // keyboard from the pane the user actually acted on.
        if self.state.read(cx).pending_focus_editor == Some(self.pane) {
            self.state.update(cx, |state, _cx| state.pending_focus_editor = None);
            self.focus_handle.clone().focus(window, cx);
        }

        // Nav menu jump (state.rs's `jump_to_line`): FileExplorer has no
        // direct reference to this view to call a scroll method on, so it
        // leaves a flag on the active tab instead. Honor and clear it here,
        // before laying out this frame — always centering (not the regular
        // edge-triggered scroll_to_cursor) so clicking an already-visible
        // heading still visibly does something.
        let pane_idx = self.tab_index(cx);
        let should_scroll = self.state.update(cx, |state, _cx| {
            let active = pane_idx.unwrap_or(usize::MAX);
            if let Some(tab) = state.tabs.get_mut(active) {
                if tab.pending_scroll_to_cursor {
                    tab.pending_scroll_to_cursor = false;
                    return true;
                }
            }
            false
        });
        if should_scroll {
            self.scroll_to_cursor_centered(cx);
        }

        // Tab-scroll isolation: this view has a single shared `scroll_handle`
        // for the whole window (see the struct-field doc comment above), so
        // without this check the previously active tab's scroll offset just
        // stays put when the user switches tabs, "leaking" into whichever
        // tab becomes active. Detect the switch by comparing the active
        // tab's stable `id` (not its positional index) against what was
        // seen last render: on a switch, stash the outgoing tab's current
        // offset under its old id, then restore the incoming tab's saved
        // offset — or `Point::default()` (scrolled to top) if this is the
        // first time that tab has ever been active.
        let active_tab_id = self.tab_index(cx).and_then(|i| self.state.read(cx).tabs.get(i)).map(|t| t.id);
        if self.last_seen_active_tab != active_tab_id {
            if let Some(prev_id) = self.last_seen_active_tab {
                self.tab_scroll_offsets.insert(prev_id, self.scroll_handle.offset());
            }
            let restore = active_tab_id.and_then(|id| self.tab_scroll_offsets.get(&id)).copied().unwrap_or_default();
            self.scroll_handle.set_offset(restore);
            self.last_seen_active_tab = active_tab_id;
        }

        let idx = pane_idx;
        let state = self.state.read(cx);
        let zoom = state.zoom;
        // The editor pane is themed like the rest of the chrome — every color
        // below comes from the palette so light mode reaches the document
        // surface too, not just the frame around it.
        let p = state.current_palette();
        let theme_mode = state.theme_mode;
        let cursor_style = if state.vim_enabled { CursorStyle::Block } else { CursorStyle::Line };
        let normal_size_px = state.normal_text_size_half_points as f32 / 2.0;
        let viewport_width = self.scroll_handle.bounds().size.width.as_f32();
        let dragging = state.split_dragging;
        let invisibility = state.invisibility_mode;
        let cite_size = state.cite_size_half_points;
        let fold_version = idx.and_then(|i| state.tabs.get(i)).map(|t| t.fold_version).unwrap_or(0);
        let folds = idx
            .and_then(|i| state.tabs.get(i))
            .map(|t| t.folded_headings.clone())
            .unwrap_or_default();
        let tab_id = idx.and_then(|i| state.tabs.get(i)).map(|t| t.id).unwrap_or(usize::MAX);
        let content_version = idx.and_then(|i| state.tabs.get(i)).map(|t| t.content_version).unwrap_or(0);
        let cache_valid = self
            .row_cache
            .as_ref()
            .is_some_and(|c| row_cache_is_valid_for(c, tab_id, content_version, viewport_width, zoom, dragging, invisibility, fold_version));
        // Only pay for the full content/paragraphs clone on a cache miss.
        // `document_lines`/word-wrap need `cx` free of `state`'s borrow (see
        // `let _ = state;` below), so the actual wrap happens further down —
        // this just captures the owned data a miss needs before that borrow ends.
        let fresh_content_and_paragraphs = (!cache_valid).then(|| {
            (
                state.pane_content(self.pane).to_string(),
                state.tabs.get(idx.unwrap_or(usize::MAX)).map(|t| t.paragraphs.clone()).unwrap_or_default(),
            )
        });
        let is_new_tab = state
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .map(|t| t.is_blank_new_tab())
            .unwrap_or(true);
        let show_unsupported_banner = state
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .map(|t| t.has_unsupported_blocks && !t.unsupported_banner_dismissed)
            .unwrap_or(false);
        let (cursor_line, cursor_col) = state.pane_cursor_line_col(self.pane);
        // Normalise (anchor, focus) into (min, max) once so per-line lookups
        // below don't each have to re-derive the ordering.
        // Flattened with "Select similar formatting"'s own matched ranges
        // (`Tab.similar_ranges`) — the two draw identically, so the whole
        // paint path below takes one list rather than knowing about both.
        // In practice only one is ever non-empty: selecting-similar clears
        // the caret selection, and the next keystroke or click clears the
        // similar ranges.
        let selections: Vec<(usize, usize)> = state
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .into_iter()
            .flat_map(|t| {
                t.selection
                    .map(|(a, f)| (a.min(f), a.max(f)))
                    .into_iter()
                    .chain(t.similar_ranges.iter().copied())
            })
            .collect();
        // Mode indicator text. Deviates from spec 5.1's literal "nothing
        // shown for Normal" — showing `-- NORMAL --` removes the ambiguity
        // between "vim is on and in Normal mode" and "vim mode is off
        // entirely", both of which otherwise render an identical blank
        // indicator strip.
        let mode_indicator_text: Option<&'static str> = if state.vim_enabled {
            idx.and_then(|i| state.tabs.get(i)).map(|t| match t.vim_mode {
                VimMode::Normal => "-- NORMAL --",
                VimMode::Insert => "-- INSERT --",
                VimMode::Visual => "-- VISUAL --",
                VimMode::VisualLine => "-- VISUAL LINE --",
                VimMode::Command => "-- COMMAND --",
                VimMode::Replace => "-- REPLACE --",
                VimMode::Search => "-- SEARCH --",
            })
        } else {
            None
        };
        // Echoes every in-progress "waiting for the next key" state next
        // to the mode label — not just `vim_command_buf`'s own count/
        // pending-trigger grammar (`3f`), but also the pending states
        // Task F/G added afterward that deliberately live in *separate*
        // fields rather than `vim_command_buf` (to avoid colliding with
        // its existing grammar — see e.g. `start_vim_operator`'s doc
        // comment): a pending `d`/`y`/`c`/`>`/`<`/`gU`/`gu` operator, an
        // `i`/`a` text-object prefix after one, and `q`/`@` macro
        // record/replay's own pending-register state. Concretely, this
        // string is a UI-only concern — it's built by concatenating
        // whichever of these happen to be active; the underlying
        // functionality (recording, replaying, running operators) already
        // worked correctly without it, confirmed by testing after this
        // fix was requested — this closes a *feedback* gap, not a
        // functional one, matching what "no visual on the command mode
        // line" while everything actually worked turned out to mean.
        // Also shows "recording @<register>" for the whole duration of an
        // active recording (real vim does this too), not just the initial
        // `q<register>` keystroke. In Command mode (Task H), shows the
        // live `:command` text instead of the Normal/Visual pending-state
        // echo, since the two are mutually exclusive by construction (only
        // one `vim_mode` is active at a time). A `vim_command_error` from
        // the last dispatched command (e.g. `:q` refused on unsaved
        // changes, or an unrecognized command) is appended in any mode
        // until the next `:` is opened, matching real vim's persistent
        // error line.
        let pending_command_text: Option<String> = state
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .map(|t| {
                let mut buf = if t.vim_mode == VimMode::Command {
                    format!(":{}", t.vim_command_line)
                } else if t.vim_mode == VimMode::Search {
                    let prefix = if t.vim_search_direction { '/' } else { '?' };
                    format!("{prefix}{}", t.vim_command_line)
                } else {
                    let mut buf = t.vim_command_buf.clone();
                    if let Some(operator) = t.vim_pending_operator {
                        buf.push(operator);
                        if let Some(inner) = t.vim_pending_text_object_prefix {
                            buf.push(if inner { 'i' } else { 'a' });
                        }
                    }
                    if state.vim_macro_record_pending() {
                        buf.push('q');
                    }
                    if self.macro_at_pending {
                        buf.push('@');
                    }
                    if let Some(register) = state.vim_recording_register() {
                        buf.push_str(&format!(" [recording @{register}]"));
                    }
                    buf
                };
                if let Some(err) = &t.vim_command_error {
                    if !buf.is_empty() { buf.push(' '); }
                    buf.push_str(err);
                }
                buf
            })
            .filter(|buf| !buf.is_empty());
        let _ = state;

        let is_focused = self.focus_handle.is_focused(window);

        // Rebuild the word-wrapped row table only on a cache miss (see
        // `RowCache`'s doc comment) — `state`'s borrow of `cx` has already
        // ended (`let _ = state;` above), so `cx` is free for
        // `visual_rows_for_viewport` again here.
        if let Some((content, paragraphs)) = fresh_content_and_paragraphs {
            let lines = document_lines(&content);
            let line_chars: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();

            // Byte offset of each logical line's start within `content`,
            // needed to test `selection` (a document-wide byte range)
            // against each line.
            let mut line_byte_starts: Vec<usize> = Vec::with_capacity(lines.len());
            let mut byte_offset = 0;
            for l in &lines {
                line_byte_starts.push(byte_offset);
                byte_offset += l.len() + 1; // +1 for the '\n' the split() consumed
            }

            // Word-wrap each logical line into fixed-height visual rows so
            // long lines reflow within the viewport instead of running off
            // the right edge. Click/drag hit-testing and scroll-to-cursor
            // rebuild this exact same row table (via the same helper
            // functions) so all three always agree on where each row's
            // boundaries fall.
            let rows = visual_rows_for_viewport(
                cx, &lines, viewport_width, zoom, &paragraphs, normal_size_px,
            );
            let folded_paras = AppState::folded_paragraphs(&paragraphs, &folds);
        let hidden = hidden_wrap_rows(&rows, &paragraphs, invisibility, cite_size, &folded_paras);
            let (display_to_wrap, wrap_to_display) =
                expand_rows_for_display(&rows, &paragraphs, zoom, &hidden, normal_size_px);

            self.row_cache = Some(RowCache {
                tab_id,
                content_version,
                invisibility,
                fold_version,
                viewport_width_bits: viewport_width.to_bits(),
                zoom_bits: zoom.to_bits(),
                lines: Rc::new(lines),
                line_chars: Rc::new(line_chars),
                line_byte_starts: Rc::new(line_byte_starts),
                rows: Rc::new(rows),
                paragraphs: Rc::new(paragraphs),
                display_to_wrap: Rc::new(display_to_wrap),
                wrap_to_display: Rc::new(wrap_to_display),
            });
        }
        let cache = self
            .row_cache
            .as_ref()
            .expect("populated just above on a miss; cache_valid guarantees it already existed on a hit");
        let lines = cache.lines.clone();
        let line_chars = cache.line_chars.clone();
        let line_byte_starts = cache.line_byte_starts.clone();
        let rows = cache.rows.clone();
        let paragraphs = cache.paragraphs.clone();
        let display_to_wrap = cache.display_to_wrap.clone();
        let wrap_to_display = cache.wrap_to_display.clone();

        // Display-space cursor row (see `expand_rows_for_display`) — the
        // `uniform_list` closure below iterates display indices, not raw
        // wrap-row indices, so the cursor-row comparison inside it needs to
        // be in the same space.
        let cursor_visual_row = is_focused.then(|| {
            let wrap_row = visual_row_for_line_col(&rows, cursor_line, cursor_col);
            wrap_to_display[wrap_row]
        });

        // Outer wrapper: takes the same slot in main_window's flex row the
        // scrollable editor div used to occupy directly (`.flex_1()`,
        // `.min_w_0()`, `.min_h_0()` all moved here from that div below), and
        // stacks [scrollable editor, mode indicator] as siblings in a column.
        // The indicator must be a *sibling* of the scrollable div, not nested
        // inside it — nesting it inside would make it scroll with content and
        // perturb `scroll_handle.bounds()`/`max_offset()`, which
        // `scroll_to_cursor` and the wrap math both depend on reflecting only
        // the editor's own viewport.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(show_unsupported_banner, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .px(px(12.0))
                        .py(px(6.0))
                        // A warning strip, so it keeps its amber identity
                        // rather than becoming palette chrome — but the dark
                        // amber is illegible on a light theme, so each mode
                        // gets its own pairing.
                        .bg(rgb(match theme_mode {
                            ThemeMode::Dark => 0x5a3d1a,
                            ThemeMode::Light => 0xfaecc8,
                        }))
                        .text_color(rgb(match theme_mode {
                            ThemeMode::Dark => 0xf0d9a8,
                            ThemeMode::Light => 0x6b4e10,
                        }))
                        .text_sm()
                        .child("This document contains a table — Vimbatim can't edit or preserve it; saving will remove it.")
                        .child(
                            div()
                                .id("dismiss-unsupported-banner")
                                .cursor_pointer()
                                .px(px(8.0))
                                .child("×")
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    // Resolved at click time, not captured from
                                    // render: this listener outlives the frame
                                    // and the pane's tab can change meanwhile.
                                    let pane = this.pane;
                                    this.state.update(cx, |s, cx| {
                                        let i = s.pane_tab_index(pane);
                                        if let Some(tab) = i.and_then(|i| s.tabs.get_mut(i)) {
                                            tab.unsupported_banner_dismissed = true;
                                        }
                                        cx.notify();
                                    });
                                })),
                        ),
                )
            })
            .child(
                div()
                    // `.id()` must come before `.overflow_y_scroll()` because GPUI tracks
                    // scroll position per unique element ID (requires Stateful<Div>).
                    .id("text-editor")
                    .key_context("TextEditor")
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::handle_key_down))
            // Clicking the editor area claims keyboard focus and moves the
            // cursor to the clicked position (spec 4.1 click-to-position).
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                // Claim the pane before anything reads a tab: `focus_pane`
                // repoints `active_tab`, and every state call below (cursor
                // placement, selection) resolves through it.
                let pane = this.pane;
                this.state.update(cx, |s, cx| { s.focus_pane(pane); cx.notify(); });
                this.focus_handle.clone().focus(window, cx);
                let bounds = this.scroll_handle.bounds();
                let scroll_y = this.scroll_handle.offset().y.as_f32();
                let zoom = this.state.read(cx).zoom;
                let font_size_px = this.state.read(cx).normal_text_size_half_points as f32 / 2.0;
                let paragraphs = {
                    let st = this.state.read(cx);
                    pane_idx.and_then(|i| st.tabs.get(i)).map(|t| t.paragraphs.clone()).unwrap_or_default()
                };
                let (rows, display_to_wrap, _) = this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                let row_height_px = real_row_height_px(&this.uniform_list_scroll_handle, display_to_wrap.len(), font_size_px, zoom);
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, &rows, &display_to_wrap, zoom, font_size_px, &paragraphs, row_height_px);
                let click_count = ev.click_count;
                this.state.update(cx, |state, cx| {
                    state.editor_context_menu = None;
                    state.clear_similar_selection();
                    // `set_cursor_from_line_col` does the line/col -> byte-offset
                    // conversion (there's no standalone public helper for it) and
                    // leaves the result in `tab.cursor`, so double/triple-click
                    // reuse that single call instead of re-deriving the byte
                    // position themselves.
                    state.set_cursor_from_line_col(line, col);
                    let byte_pos = pane_idx.and_then(|i| state.tabs.get(i)).map(|t| t.cursor).unwrap_or(0);
                    match click_count {
                        2 => state.select_word_at(byte_pos),
                        3 => state.select_line_at(byte_pos),
                        _ => {}
                    }
                    cx.notify();
                });
                cx.notify();
            }))
            // Right-click opens the Cut/Copy/Paste menu (rendered by
            // `render_context_menu` at the bottom of this wrapper).
            //
            // ponytail: an existing selection is left alone rather than
            // hit-tested against the click point — right-clicking *inside* a
            // selection must keep it (that's the whole point of the Copy
            // item), and right-clicking outside one is rare enough that
            // "menu opens, selection unchanged" beats redoing the byte-offset
            // math just to decide whether to clear it. Add the hit-test if
            // that ever bites.
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let pane = this.pane;
                this.state.update(cx, |s, cx| { s.focus_pane(pane); cx.notify(); });
                this.focus_handle.clone().focus(window, cx);
                let has_selection = {
                    let st = this.state.read(cx);
                    pane_idx.and_then(|i| st.tabs.get(i)).is_some_and(|t| t.selection.is_some())
                };
                // Resolve the click to a (line, col) whether or not there's a
                // selection — without a selection it also moves the caret,
                // but either way it's what locates a misspelled word below.
                let bounds = this.scroll_handle.bounds();
                let scroll_y = this.scroll_handle.offset().y.as_f32();
                let zoom = this.state.read(cx).zoom;
                let font_size_px = this.state.read(cx).normal_text_size_half_points as f32 / 2.0;
                let paragraphs = {
                    let st = this.state.read(cx);
                    pane_idx.and_then(|i| st.tabs.get(i)).map(|t| t.paragraphs.clone()).unwrap_or_default()
                };
                let (rows, display_to_wrap, _) = this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                let row_height_px = real_row_height_px(&this.uniform_list_scroll_handle, display_to_wrap.len(), font_size_px, zoom);
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, &rows, &display_to_wrap, zoom, font_size_px, &paragraphs, row_height_px);
                if !has_selection {
                    this.state.update(cx, |state, _cx| state.set_cursor_from_line_col(line, col));
                }

                // Did the click land on a squiggle? `suggest` runs here, once,
                // rather than during render — it's a dictionary search, far
                // slower than the per-word `check` the squiggles use.
                let spell_target = {
                    let st = this.state.read(cx);
                    if !st.spellcheck_enabled {
                        None
                    } else {
                        let content = pane_idx.and_then(|i| st.tabs.get(i)).map(|t| t.content.clone()).unwrap_or_default();
                        let lines = document_lines(&content);
                        lines.get(line).and_then(|text| {
                            crate::spellcheck::misspelled_ranges(text, &st.user_dictionary)
                                .into_iter()
                                .find(|&(s, e)| col >= s && col < e)
                                .map(|(start_col, end_col)| {
                                    let word: String =
                                        text.chars().skip(start_col).take(end_col - start_col).collect();
                                    let suggestions = crate::spellcheck::suggest(&word);
                                    SpellTarget { line, start_col, end_col, word, suggestions }
                                })
                        })
                    }
                };

                this.state.update(cx, |state, cx| {
                    state.editor_context_menu = Some(EditorContextMenu {
                        position: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                        spell_target,
                    });
                    cx.notify();
                });
            }))
            // Dragging with the left button held extends a selection from
            // wherever on_mouse_down landed (spec 4.3 "mouse click-drag
            // creates a selection"). `auto_scroller.notify` starts (or feeds)
            // a per-frame auto-scroll loop when the drag is near the top/
            // bottom edge of the viewport, so the selection can extend past
            // what's currently visible even if the mouse stops moving.
            // `on_mouse_move` only fires while the cursor is over this
            // element's own bounds, so a drag that exits the editor (e.g.
            // into the sidebar) stops updating until it re-enters —
            // acceptable for a first pass, not spec-required to track drags
            // that leave the editor.
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, window, cx| {
                if !ev.dragging() { return; }
                let bounds = this.scroll_handle.bounds();
                let scroll_y = this.scroll_handle.offset().y.as_f32();
                let zoom = this.state.read(cx).zoom;
                let font_size_px = this.state.read(cx).normal_text_size_half_points as f32 / 2.0;
                let paragraphs = {
                    let st = this.state.read(cx);
                    pane_idx.and_then(|i| st.tabs.get(i)).map(|t| t.paragraphs.clone()).unwrap_or_default()
                };
                let (rows, display_to_wrap, _) = this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                let row_height_px = real_row_height_px(&this.uniform_list_scroll_handle, display_to_wrap.len(), font_size_px, zoom);
                let (line, col) = line_col_from_mouse_position(ev.position, bounds, scroll_y, &rows, &display_to_wrap, zoom, font_size_px, &paragraphs, row_height_px);
                this.state.update(cx, |state, cx| {
                    state.extend_selection_to_line_col(line, col);
                    cx.notify();
                });
                this.auto_scroller.notify(ev.position, window);
                cx.notify();
            }))
            // Stop any in-progress auto-scroll loop on mouse-up, whether the
            // release happens over the editor (on_mouse_up) or elsewhere
            // (on_mouse_up_out, e.g. the user dragged into the sidebar and
            // released there) — otherwise a drag that ends while parked in
            // the edge zone would keep scrolling forever with nothing left
            // to stop it.
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _ev, _window, _cx| {
                this.auto_scroller.stop();
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _ev, _window, _cx| {
                this.auto_scroller.stop();
            }))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            // Critical (see main_window.rs's `min_h_0` comment for the same
            // pattern): this div is now a flex_1 child on a flex_col's main
            // axis (its parent wrapper, added by this task) rather than a
            // cross-axis-stretched flex_row child like before — on the main
            // axis a flex item's default min-height is its content size, so
            // without this a document taller than the viewport could grow
            // the div past the wrapper's allocated height instead of
            // scrolling internally.
            .min_h_0()
            .bg(rgb(p.editor_bg))
            // No `.overflow_y_scroll()`/`.track_scroll()`/`.p()`/`.border_1()`
            // here anymore — `uniform_list` below owns the actual scrolling
            // now (it sets its own vertical overflow internally and is
            // `.track_scroll()`ed to `uniform_list_scroll_handle`), and
            // padding/border move with it: `self.scroll_handle.bounds()`
            // (via the shared handle, see `TextEditor::new()`) reflects
            // *uniform_list's* box, and the click/scroll pixel math's
            // `CONTENT_PADDING_PX` subtraction assumes that box already
            // includes the padding inset, same as it did when this div was
            // both the padded box and the tracked scroll container at once.
            // This div is now just a plain flex_col wrapper stacking
            // [new-tab placeholder?, the scrollable row list].
            // Placeholder shown on an empty, unsaved tab — a plain sibling
            // above the row list now rather than uniform_list's first
            // "item" (uniform_list has no such prepend slot); low-risk since
            // a genuinely empty new tab has nothing to scroll to anyway.
            .when(is_new_tab, |d| {
                d.child(
                    div()
                        .text_sm()
                        .text_color(rgb(p.text_faint))
                        .font_family(FONT_FAMILY)
                        .p(px(16.0))
                        .child("Open a file from the sidebar, or start typing…"),
                )
            })
            .child(
                uniform_list("text-editor-rows", display_to_wrap.len(), {
                    let lines = lines.clone();
                    let selections = selections.clone();
                    let line_chars = line_chars.clone();
                    let line_byte_starts = line_byte_starts.clone();
                    let rows = rows.clone();
                    let paragraphs = paragraphs.clone();
                    let display_to_wrap = display_to_wrap.clone();
                    // Spellcheck inputs, read once per frame rather than per
                    // row. `user_dictionary` is `Rc` in `AppState` precisely
                    // so this is a refcount bump, not a deep clone.
                    let spellcheck_enabled = self.state.read(cx).spellcheck_enabled;
                    let user_dictionary = self.state.read(cx).user_dictionary.clone();
                    let spell_cache = self.spell_cache.clone();
                    let spellcheck_color =
                        highlight_color_hex(&self.state.read(cx).spellcheck_underline_color);
                    let invisibility_mode = self.state.read(cx).invisibility_mode;
                    let cite_size_half_points = self.state.read(cx).cite_size_half_points;
                    let folded_headings = {
                        let st = self.state.read(cx);
                        st.tabs.get(st.active_tab).map(|t| t.folded_headings.clone()).unwrap_or_default()
                    };
                    let fold_state = self.state.clone();
                    move |range: std::ops::Range<usize>, _window, _cx| {
                        range.map(|display_idx| {
                        // A `None` slot is a blank spacer reserved before an
                        // oversized (card-style/heading) row so its content
                        // has empty space to visually spill upward into
                        // instead of overlapping the row above — see
                        // `expand_rows_for_display`'s doc comment. Same
                        // fixed `.h()` as a real row, just no content, so
                        // `uniform_list`'s single-measurement layout still
                        // sees a uniform row height everywhere.
                        let Some(visual_idx) = display_to_wrap[display_idx] else {
                            return div().h(px(line_height_px(normal_size_px) * zoom)).into_any_element();
                        };
                            let (li, row_start, row_end) = rows[visual_idx];
                        let chars = &line_chars[li];
                        let row_text: String = chars[row_start..row_end].iter().collect();

                        // `.then(|| ...)` (lazy), not `.then_some(...)` — the latter's
                        // argument is a plain value, evaluated eagerly *before* the
                        // bool is even checked. With `then_some`, `cursor_col - row_start`
                        // was computed for every row regardless of the condition, and
                        // underflowed (panicked) on any row whose row_start exceeded the
                        // cursor's column — i.e. almost any row that isn't the cursor's own.
                        let row_cursor_col = (cursor_visual_row == Some(display_idx))
                            .then(|| cursor_col - row_start);

                        // Clip the logical line's selection char-range (if any) down
                        // to this row's own [row_start, row_end) sub-range, then
                        // rebase it to be relative to the row instead of the line.
                        let row_selections: Vec<(usize, usize)> = selections
                            .iter()
                            .filter_map(|&(s, e)| selection_span_for_line(&lines[li], line_byte_starts[li], s, e))
                            .filter_map(|(sel_start, sel_end)| {
                                let clipped_start = sel_start.max(row_start);
                                let clipped_end = sel_end.min(row_end);
                                // Same eager-vs-lazy pitfall as row_cursor_col above: use
                                // `.then(|| ...)` since clipped_end can be < row_start when
                                // the selection doesn't reach this row, which would
                                // underflow `clipped_end - row_start` if evaluated eagerly.
                                (clipped_start < clipped_end)
                                    .then(|| (clipped_start - row_start, clipped_end - row_start))
                            })
                            .collect();

                        // Rich-text formatting (Phase 1): clip this logical
                        // line's paragraph run boundaries down to this row's
                        // own [row_start, row_end) sub-range, same rebasing
                        // pattern as `row_selection` above — a wrapped row
                        // only needs to know about the runs it actually spans.
                        let row_run_spans: Vec<(usize, usize, usize)> = paragraphs
                            .get(li)
                            .map(|p| paragraph_run_char_spans(p))
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|(rs, re, run_idx)| {
                                let clipped_start = rs.max(row_start);
                                let clipped_end = re.min(row_end);
                                (clipped_start < clipped_end)
                                    .then(|| (clipped_start - row_start, clipped_end - row_start, run_idx))
                            })
                            .collect();

                        // Spellcheck: the logical line's misspelled ranges,
                        // clipped and rebased onto this row exactly like
                        // `row_selection` and `row_run_spans` above.
                        //
                        // Memoized per line text (see `spell_ranges_cached`),
                        // so a keystroke re-checks only the line being edited
                        // and scrolling is free. Measured uncached, for
                        // reference: ~10µs per realistic card paragraph in
                        // release, ~6x that in debug.
                        let row_misspelled: Vec<(usize, usize)> = if spellcheck_enabled {
                            spell_ranges_cached(&spell_cache, &lines[li], &user_dictionary)
                                .iter()
                                .filter_map(|&(ms, me)| {
                                    let clipped_start = ms.max(row_start);
                                    let clipped_end = me.min(row_end);
                                    (clipped_start < clipped_end)
                                        .then(|| (clipped_start - row_start, clipped_end - row_start))
                                })
                                .collect()
                        } else {
                            Vec::new()
                        };

                        // Check if previous paragraph also has box_format (for merging boxes)
                        let prev_has_box = li > 0 && paragraphs.get(li - 1)
                            .is_some_and(|p| p.runs.iter().any(|r| r.box_format));

                        // Fold marker, on a heading's *first* row only — a
                        // wrapped heading gets one marker, not one per visual
                        // row. Hidden until the row is hovered, so a folded
                        // outline reads as clean text rather than a column of
                        // arrows.
                        let row_heading = paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                        let fold_toggle = (row_heading != 0 && row_start == 0).then(|| {
                            let collapsed = folded_headings.contains(&li);
                            let state = fold_state.clone();
                            div()
                                .id(ElementId::named_usize("fold-toggle", li))
                                .w(px(12.0 * zoom))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                // Independent of the heading's own (possibly
                                // very large) font size, so markers stay a
                                // consistent size down the outline.
                                .text_size(px(9.0 * zoom))
                                .font_weight(FontWeight::NORMAL)
                                // Transparent rather than absent: the marker
                                // keeps its width at all times, so hovering a
                                // heading doesn't shift its text sideways.
                                .text_color(transparent_black())
                                .group_hover(FOLD_ROW_GROUP, move |s| s.text_color(rgb(p.text_muted)))
                                .cursor_pointer()
                                .hover(move |s| s.text_color(rgb(p.text)))
                                .on_mouse_down(MouseButton::Left, {
                                    // A plain closure over a cloned handle,
                                    // not `cx.listener` — the `uniform_list`
                                    // closure is `'static` and cannot borrow
                                    // the view's context.
                                    move |_ev, _window, cx: &mut App| {
                                        // Stops the click also placing the
                                        // caret in the heading.
                                        cx.stop_propagation();
                                        state.update(cx, |st, cx| {
                                            st.toggle_paragraph_fold(li);
                                            cx.notify();
                                        });
                                    }
                                })
                                .child(if collapsed { "▶" } else { "▼" })
                                .into_any_element()
                        });

                        let content_el = render_line(
                            &row_text,
                            row_cursor_col,
                            &row_selections,
                            &row_run_spans,
                            paragraphs.get(li),
                            prev_has_box,
                            zoom,
                            p,
                            cursor_style,
                            &row_misspelled,
                            spellcheck_color,
                            invisibility_mode,
                            cite_size_half_points,
                            fold_toggle,
                        );
                        // Heading styles (spec 6.5): a paragraph-wide default
                        // that per-run formatting (bold/size/etc., applied
                        // inside `content_el`'s own children) still overrides
                        // for the specific characters it covers, since GPUI's
                        // text style cascades to children and a child's own
                        // call wins. NOTE: a heading's larger font size can
                        // visually overflow this row's fixed `LINE_HEIGHT_PX`
                        // by design — `expand_rows_for_display` reserves
                        // blank spacer rows after this one sized for exactly
                        // that overflow (see `slot_count_for_paragraph`), so
                        // it spills into empty space rather than the next
                        // row's content; still needs real-hardware
                        // verification (this sandbox has no display) to
                        // confirm how it actually looks.
                        let heading = paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                        // `normal_size_px` (settings.conf's `normal_text_size`,
                        // read once above as `normal_text_size_half_points`)
                        // is the visual default for any run with no explicit
                        // `FontSize` override (`size == 0` — brand-new
                        // documents' single default run, and any plain-typed
                        // text) — a run-level or heading-level override still
                        // wins underneath, same as before this was configurable.
                        let row_div = div()
                            .font_family(FONT_FAMILY)
                            .text_size(px(normal_size_px * zoom))
                            .text_color(rgb(p.text));
                        let row_div = match heading_font_size_px(heading, zoom) {
                            Some(size) => row_div.text_size(px(size)).font_weight(FontWeight::BOLD),
                            None => row_div,
                        };
                        row_div
                            // Locks this row's height so wrapping stays fully
                            // decided by `wrap_line_into_rows` up front — nowrap
                            // stops GPUI from *also* word-wrapping this row's text
                            // internally if CHAR_WIDTH_PX's monospace estimate
                            // ever slightly overshoots the real glyph width, which
                            // would otherwise grow this div past one row and break
                            // the fixed-row-height assumption click/scroll math relies on.
                            .whitespace_nowrap()
                            // `.h()`, not `.min_h()` — uniform_list measures
                            // exactly one row (`measure_item`, always at
                            // `list_width: None` i.e. unconstrained/MinContent
                            // width — confirmed in the vendored gpui source,
                            // `elements/uniform_list.rs`'s `request_layout`/
                            // `prepaint`) and applies *that single row's*
                            // height to *every* row in the whole list
                            // (`item_top = item_height * item_index`, same
                            // file). `.min_h()` only floors the height, so
                            // any row whose content naturally measures taller
                            // than `LINE_HEIGHT_PX` under that unconstrained-
                            // width measurement pass — which any wrapped or
                            // multi-span row can — poisoned every row's
                            // spacing uniformly (found from a real bug
                            // report: lines rendering ~2x too far apart, and
                            // auto-scroll/scroll-to-cursor firing late since
                            // their pixel math assumes exactly
                            // `LINE_HEIGHT_PX` per row). An explicit `.h()`
                            // is a fixed layout size independent of content
                            // or measurement width, so `measure_item` always
                            // returns exactly `LINE_HEIGHT_PX * zoom`
                            // regardless of which row it happens to measure.
                            // A heading's larger font can still visually
                            // overflow this box (unchanged from before this
                            // fix, still not clipped since overflow stays
                            // visible — see the comment on `heading` above).
                            .h(px(line_height_px(normal_size_px) * zoom))
                            // Column direction + justify_end bottom-aligns
                            // `content_el` within this fixed-height slot when
                            // it's shorter (a Shrunk line next to normal-size
                            // ones) instead of the block-layout default of
                            // sitting flush at the top with empty space below.
                            // Column's *cross* axis is horizontal and defaults
                            // to `Stretch`, so this doesn't change width
                            // behavior — `content_el` still fills the row
                            // exactly as it did as a plain block child, which
                            // is what its own internal justify_center/
                            // justify_end (paragraph alignment) and the
                            // Pocket box's `w_full()` depend on.
                            //
                            // `.w_full()`: this row_div is a genuine Taffy
                            // *root* for this layout pass — uniform_list's
                            // paint loop calls `item.layout_as_root(available_space)`
                            // per row (`elements/uniform_list.rs`) — and
                            // Taffy's root-sizing carve-out that auto-stretches
                            // an unsized node to its available space only
                            // applies to `display: block` nodes
                            // (`compute_root_layout`, gated on
                            // `style.is_block()`); this is `display: flex`,
                            // so with no width of its own it fell through to
                            // ordinary flex content-sizing and hugged its
                            // widest child instead — the real reason
                            // alignment silently did nothing no matter what
                            // was set further down the tree (`line_div`'s own
                            // `w_full()`, `box_div`'s too, both resolve
                            // against *this* node's width, which was never
                            // definite). Confirmed by reading Taffy's actual
                            // `compute_root_layout`/`perform_child_layout`
                            // source, not guessed — this is the fourth
                            // reported attempt at this bug, and the first
                            // three all added width one or more levels too
                            // low to matter.
                            .w_full()
                            .flex()
                            .flex_col()
                            .justify_end()
                            // Marks this row as the hover group the fold
                            // marker inside it watches.
                            .group(FOLD_ROW_GROUP)
                            .child(content_el)
                            .into_any_element()
                        }).collect()
                    }
                })
                // `uniform_list` is the actual scrollable element now (it
                // sets vertical overflow internally); padding/border move
                // here from the old outer div for the reason explained
                // above this `.child(...)` block.
                .track_scroll(&self.uniform_list_scroll_handle)
                .flex_1()
                .min_w_0()
                .min_h_0()
                .w_full()
                // Prevent long lines from expanding the editor past its
                // flex_1 allocation — uniform_list only constrains the
                // vertical axis internally, same as the old div's own
                // `.overflow_y_scroll()` needed an explicit
                // `.overflow_x_hidden()` alongside it.
                .overflow_x_hidden()
                .p(px(16.0))
                // Thin focus ring so the user can tell where key input lands
                .border_1()
                .border_color(if is_focused { rgb(p.accent) } else { rgb(p.editor_bg) })
            ) // closes the "text-editor" div's .child(uniform_list...)
            ) // closes the scrollable-editor .child(...) on the wrapper
            .child({
                // Mode indicator (spec 5.1) — a sibling below the scrollable
                // editor div, at a fixed height so switching modes doesn't
                // resize (and re-wrap) the editor's own viewport. The
                // in-progress command/count buffer (e.g. "3f"), when
                // present, is appended after the mode label on the same
                // line, matching real vim's bottom-right pending-keys echo.
                let mut line = mode_indicator_text.unwrap_or("").to_string();
                if let Some(pending) = &pending_command_text {
                    if !line.is_empty() { line.push(' '); }
                    line.push_str(pending);
                }
                div()
                    .h(px(LINE_HEIGHT_PX))
                    .px(px(16.0))
                    .bg(rgb(p.editor_bg))
                    .font_family(FONT_FAMILY)
                    .text_sm()
                    .text_color(rgb(p.text))
                    .child(line)
            })
            .when_some(self.state.read(cx).editor_context_menu.clone(), |el, menu| {
                let has_selection = self
                    .state
                    .read(cx)
                    .tabs
                    .get(self.tab_index(cx).unwrap_or(usize::MAX))
                    .is_some_and(|t| t.selection.is_some());
                el.child(render_context_menu(menu, p, has_selection, &self.state))
            })
    }
}

/// The editor's right-click menu, pinned to the click position with the same
/// `deferred(anchored(...))` pair the file explorer's menu uses
/// (`file_explorer.rs::render_context_menu`) so it paints above the row list
/// regardless of tree position.
///
/// Always shows Cut / Copy / Paste. When the click landed on a misspelled word
/// (`EditorContextMenu.spell_target`), spelling suggestions are prepended and
/// "Add to Dictionary" is appended.
///
/// The clipboard items dispatch the *existing* keybind actions rather than
/// re-implementing clipboard handling — `main_window.rs` already owns those
/// three handlers (including rich-run metadata on copy/cut and rich paste),
/// and they're registered as global actions, so `Window::dispatch_action`
/// reaches them regardless of focus.
fn render_context_menu(
    menu: EditorContextMenu,
    p: Palette,
    has_selection: bool,
    state_handle: &Entity<AppState>,
) -> AnyElement {
    let (x, y) = menu.position;

    // Shared row chrome. Every item closes the menu; what it does *after* that
    // is the caller's closure.
    let row = |id: ElementId, label: String, enabled: bool, color: u32| {
        div()
            .id(id)
            .h(px(26.0))
            .px(px(10.0))
            .flex()
            .items_center()
            .text_sm()
            .text_color(rgb(if enabled { color } else { p.text_faint }))
            .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(rgb(p.chrome_hover))))
            .child(label)
    };
    let separator = || div().h(px(1.0)).my(px(4.0)).bg(rgb(p.border_subtle));

    // Cut/Copy are no-ops without a selection; show them muted rather than
    // hiding them so the menu doesn't change shape between clicks.
    let action_item = |id: &'static str, label: &'static str, enabled: bool, action: fn() -> Box<dyn Action>, state: Entity<AppState>| {
        row(id.into(), label.to_string(), enabled, p.text).when(enabled, |d| {
            d.on_click(move |_ev, window, cx| {
                state.update(cx, |s, cx| {
                    s.editor_context_menu = None;
                    cx.notify();
                });
                window.dispatch_action(action(), cx);
            })
        })
    };

    let mut panel = div()
        .flex()
        .flex_col()
        // Wide enough for "Add to Dictionary" without wrapping; suggestions
        // are single words and comfortably shorter.
        .w(px(180.0))
        .bg(rgb(p.chrome))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(px(6.0))
        .shadow_lg()
        .py(px(4.0))
        // Keeps the editor's own left-mouse-down (which closes this menu and
        // moves the caret) from firing before the item's on_click.
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation());

    // ── Suggestions ────────────────────────────────────────────────────────
    // Above the clipboard items, matching Word: they're the reason the user
    // right-clicked a squiggle, so they get the top of the menu.
    if let Some(target) = &menu.spell_target {
        if target.suggestions.is_empty() {
            panel = panel.child(
                div()
                    .h(px(26.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .text_sm()
                    .italic()
                    .text_color(rgb(p.text_faint))
                    .child("No suggestions"),
            );
        } else {
            for (i, suggestion) in target.suggestions.iter().enumerate() {
                let state = state_handle.clone();
                let target = target.clone();
                let replacement = suggestion.clone();
                panel = panel.child(
                    row(("editor-ctx-suggestion", i).into(), suggestion.clone(), true, p.text)
                        .on_click(move |_ev, _window, cx| {
                            state.update(cx, |s, cx| {
                                s.replace_spell_target(&target, &replacement);
                                s.editor_context_menu = None;
                                cx.notify();
                            });
                        }),
                );
            }
        }
        panel = panel.child(separator());
    }

    // ── Clipboard ──────────────────────────────────────────────────────────
    panel = panel
        .child(action_item("editor-ctx-cut", "Cut", has_selection, || Box::new(CutAction), state_handle.clone()))
        .child(action_item("editor-ctx-copy", "Copy", has_selection, || Box::new(CopyAction), state_handle.clone()))
        .child(action_item("editor-ctx-paste", "Paste", true, || Box::new(PasteAction), state_handle.clone()));

    // ── Add to Dictionary ──────────────────────────────────────────────────
    if let Some(target) = &menu.spell_target {
        let state = state_handle.clone();
        let word = target.word.clone();
        panel = panel.child(separator()).child(
            row("editor-ctx-add-to-dict".into(), "Add to Dictionary".to_string(), true, p.text)
                .on_click(move |_ev, _window, cx| {
                    state.update(cx, |s, cx| {
                        s.add_to_user_dictionary(&word);
                        s.editor_context_menu = None;
                        cx.notify();
                    });
                }),
        );
    }

    let dismiss_state = state_handle.clone();
    deferred(
        anchored().position(point(px(x), px(y))).snap_to_window().child(
            div()
                .id("editor-context-menu-dismiss")
                .on_mouse_down_out(move |_ev: &MouseDownEvent, _window, cx| {
                    dismiss_state.update(cx, |s, cx| {
                        if s.editor_context_menu.is_some() {
                            s.editor_context_menu = None;
                            cx.notify();
                        }
                    });
                })
                .child(panel),
        ),
    )
    .with_priority(1)
    .into_any_element()
}

/// One-char-for-one-char substitution of `'\t'` -> `' '` for display only —
/// `render_line`'s and `char_width_fn`'s shared reasoning for why a raw tab
/// can't be handed to GPUI's shaper (no glyph in the bundled fonts) and why
/// swapping it for a space here can't desync any offset-based computation
/// downstream (cursor/selection/misspelled ranges all index by position, not
/// content). Never touches the actual document model — callers pass in a
/// line already read out of `tab.content`/`paragraphs`, which still holds
/// the real `'\t'` for undo/.docx export/Verbatim round-trip fidelity.
fn display_line(line: &str) -> std::borrow::Cow<'_, str> {
    if line.contains('\t') {
        std::borrow::Cow::Owned(line.replace('\t', " "))
    } else {
        std::borrow::Cow::Borrowed(line)
    }
}

fn render_line(
    line: &str,
    cursor_col: Option<usize>,
    // Row-relative char-column ranges to paint as selected. More than one
    // because "Select similar formatting" highlights every matching run in
    // the document at once; an ordinary caret selection is just the
    // single-element case.
    selections: &[(usize, usize)],
    run_spans: &[(usize, usize, usize)],
    para: Option<&Paragraph>,
    prev_has_box: bool,
    zoom: f32,
    pal: Palette,
    cursor_style: CursorStyle,
    // Row-relative char-column ranges to draw a spellcheck squiggle under,
    // and the color to draw it in. Empty when spellcheck is off or this row
    // is clean.
    misspelled: &[(usize, usize)],
    misspelled_color: u32,
    // Invisibility mode (ribbon VIEW group): paint only the parts of the
    // document that get read aloud — highlighted runs, and every run of a Tag
    // line. Everything else keeps its space and its layout and is simply not
    // drawn, so wrap points, click mapping and cursor math all stay exactly as
    // they are. Nothing about the document itself changes.
    invisibility: bool,
    cite_size_half_points: u16,
    // The fold marker for this row, when it is a heading's first row.
    //
    // Placed here rather than by the caller because a Pocket's content is
    // wrapped in a border box, and the marker belongs *inside* that box. It
    // also has to sit beside a `flex_1` wrapper around the line, or the line
    // stops filling the row and its own `justify_center` has nothing to centre
    // within — which is what silently un-centred every card style the first
    // time this marker was added.
    fold_toggle: Option<AnyElement>,
) -> AnyElement {
    /*
     * Renders one (visual-row-clipped) line of text. Splits into
     * `(run_start, run_end, run_idx)` chunks per the paragraph's formatting
     * runs (spec 6.2, rich-text formatting plan Phase 1), then further
     * splits *within* each chunk via the existing `line_segments` wherever
     * the cursor and/or selection touch it — the two concerns are
     * orthogonal (which run a character's formatting comes from vs.
     * whether it's under the cursor/selection), so composing them as an
     * outer-run/inner-cursor split avoids needing one function that
     * understands both at once.
     *
     * Falls back to a single plain-text child when there's exactly one run
     * and no cursor/selection touches this row, matching the cheap path
     * every untouched line already took before formatting existed. An
     * empty `run_spans` (formatting not available for this row, e.g. a
     * brand-new tab with no parsed paragraphs) is treated as one big
     * unformatted run spanning the whole line, so cursor/selection
     * rendering never silently breaks when formatting data is absent.
     */
    // Bug report: pressing Tab appeared to do nothing at all. Root cause
    // (confirmed via `ttf_parser::Face::glyph_index('\t')` against the
    // bundled assets/DejaVuSansMono*.ttf, which returns `None`): this app's
    // fonts have no glyph for U+0009, so GPUI's shaper paints it with zero
    // visible width — a `'\t'` reaches the model and the document really
    // does contain it (`state.rs`'s `insert_char`/`indent_vim_range`), it
    // just never became visible or advanced the on-screen cursor. See
    // `display_line`'s own comment for why substituting it here is safe.
    let line = display_line(line);
    let chars: Vec<char> = line.chars().collect();

    // Tag is the card style at heading level 4 (`CardStyleKind::heading_level`),
    // and a Tag line is read aloud in full.
    let heading = para.map(|p| p.heading).unwrap_or(0);
    // Cheap pre-check: a card-style line hides nothing, so it keeps the fast
    // path. Passing a non-bold, size-0 run means "could plain body text hide
    // here?" — if not, nothing on this line can.
    let hides_anything = run_is_hidden(invisibility, heading, false, false, 0, cite_size_half_points);

    // The fast paths below emit one element for the whole row, which cannot
    // express "some runs drawn, some not" — fall through to the per-run path
    // whenever anything might be hidden.
    if !hides_anything && cursor_col.is_none() && selections.is_empty() && misspelled.is_empty() {
        // Don't take any fast path if alignment or a box border is needed —
        // both are only drawn by the full path below (the box wrapper in
        // particular: a bare-div return skips it entirely, which was the bug
        // where a Pocket's box only drew once the cursor — or a selection,
        // forcing the same fall through — put the line on the slow path).
        // Also the bug where a paragraph's alignment (e.g. Center, read from
        // a real docx or set by a button click) was silently ignored on any
        // row that reached one of these fast returns.
        use crate::docx_parser::Alignment;
        let needs_alignment = para.is_some_and(|p| !matches!(p.alignment, Alignment::Left));
        if run_spans.is_empty() {
            if !needs_alignment {
                return line.to_string().into_any_element();
            }
        } else if let [(start, end, run_idx)] = run_spans {
            if *start == 0 && *end == chars.len() {
                let run = para.and_then(|p| p.runs.get(*run_idx));
                if run.is_none() && !needs_alignment {
                    return line.to_string().into_any_element();
                }
                let needs_box = run.is_some_and(|r| r.box_format);
                if !needs_alignment && !needs_box {
                    return apply_run_style(div(), run, zoom, pal).child(line.to_string()).into_any_element();
                }
            }
        }
    }

    let effective_spans: Vec<(usize, usize, usize)> = if run_spans.is_empty() {
        vec![(0, chars.len(), usize::MAX)]
    } else {
        run_spans.to_vec()
    };

    // Built as a fold rather than a filter/map chain so a single separator can
    // be carried across run *and* segment boundaries — hidden text between two
    // highlights is usually its own run, so the state has to outlive one run's
    // iteration.
    let mut spans: Vec<AnyElement> = Vec::new();
    // Something was dropped since the last painted fragment.
    let mut pending_gap = false;
    // A fragment has already been painted on this row, so a gap would sit
    // between two things rather than indenting the row.
    let mut emitted_any = false;

    for (run_start, run_end, run_idx) in effective_spans {
        let run = para.and_then(|p| p.runs.get(run_idx));
        let sub_len = run_end - run_start;
        let sub_cursor = cursor_col.filter(|&c| c >= run_start && c <= run_end).map(|c| c - run_start);
        let sub_selections: Vec<(usize, usize)> = selections
            .iter()
            .filter_map(|&(s, e)| {
                let (clipped_start, clipped_end) = (s.max(run_start), e.min(run_end));
                (clipped_start < clipped_end)
                    .then(|| (clipped_start - run_start, clipped_end - run_start))
            })
            .collect();
        // Same clip-and-rebase as `sub_selection`, for each squiggle
        // range that overlaps this run.
        let sub_misspelled: Vec<(usize, usize)> = misspelled
            .iter()
            .filter_map(|&(s, e)| {
                let (clipped_start, clipped_end) = (s.max(run_start), e.min(run_end));
                (clipped_start < clipped_end)
                    .then(|| (clipped_start - run_start, clipped_end - run_start))
            })
            .collect();
        // A run survives invisibility only by being highlighted; the
        // whole-line exemption for Tag lines is folded into
        // `hides_anything` above.
        let hidden = run_is_hidden(
            invisibility,
            heading,
            run.is_some_and(|r| r.highlight),
            run.is_some_and(|r| r.bold),
            run.map(|r| r.size).unwrap_or(0),
            cite_size_half_points,
        );

        for (start, end, style, is_misspelled) in
            line_segments(sub_len, sub_cursor, &sub_selections, &sub_misspelled)
        {
            // Hidden text is dropped from the layout rather than painted
            // transparently, so visible fragments close up instead of sitting
            // in gaps the width of the words that aren't there.
            //
            // The cursor's own cell survives even inside hidden text — it is
            // the only thing showing where typing would land, and losing it
            // makes the mode impossible to navigate. Everything else hidden
            // (including a selection over it) is simply not emitted, and only
            // sets the flag below.
            if hidden && style != SegmentStyle::Cursor {
                pending_gap = true;
                continue;
            }

            // Flush one separator for however much was skipped — one space or
            // three sentences both read as "something was here", which is the
            // useful signal, and keeps the row from running words together.
            // Deferring to just before the next fragment is what makes leading
            // and trailing hidden text cost nothing.
            if pending_gap && emitted_any {
                spans.push(
                    div()
                        .flex_shrink_0()
                        .w(px(HIDDEN_TEXT_GAP_PX * zoom))
                        .into_any_element(),
                );
            }
            pending_gap = false;

            // A zero-width segment only ever occurs for the cursor
            // sitting past the last character (end of line) — render
            // it as a single space so the highlighted cell still has
            // visible width.
            let text: String = if start == end {
                " ".to_string()
            } else {
                chars[run_start + start..run_start + end].iter().collect()
            };
            spans.push(render_segment(
                text,
                run,
                style,
                zoom,
                pal,
                cursor_style,
                is_misspelled.then_some(misspelled_color),
                hidden,
            ));
            emitted_any = true;
        }
    }

    // `items_end()`: when a line mixes run sizes (e.g. Shrink applied to only
    // part of it), each span's own div is as tall as its own font's line
    // height — cross-axis alignment decides where a shorter span sits inside
    // the row, and the default (effectively top) is what made smaller text
    // "float" above the baseline the surrounding text sits on.
    // `.w_full()`: justify_center()/justify_end() below only have room to
    // move content within if this row has a definite width of its own to
    // distribute — without it the flex row hugs its content's natural size
    // and every alignment reads as flush-left regardless of what's set below.
    // Row_div's own cross-axis stretch (see its comment) is meant to already
    // provide that, but real hardware testing found alignment silently
    // failing in every scenario, so this stops depending on that chain.
    let mut line_div = div().flex().flex_row().w_full().items_end().children(spans);
    // Apply paragraph-level alignment if available (Phase 4.3)
    if let Some(p) = para {
        use crate::docx_parser::Alignment;
        line_div = match p.alignment {
            Alignment::Center => line_div.justify_center(),
            Alignment::Right => line_div.justify_end(),
            Alignment::Justify => line_div.justify_between(), // approximate for now
            Alignment::Left => line_div.justify_start(),
        };

        // Check if any run has box_format (Pocket formatting)
        // Wrap in full-width box container so box stays at full width while content is aligned
        // Increased vertical padding to create visual separation between consecutive Pockets
        let has_box = p.runs.iter().any(|r| r.box_format);
        if has_box {
            let mut box_div = div()
                .w_full()
                .border_color(rgb(pal.text))
                .px(px(8.0))
                .py(px(8.0));

            box_div = match fold_toggle {
                // Inside the border, so a Pocket's marker reads as part of the
                // box rather than floating outside it.
                Some(toggle) => box_div
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(toggle)
                    .child(div().flex_1().min_w_0().child(line_div)),
                None => box_div.child(line_div),
            };

            // If previous line also has a box, merge them by removing top border.
            // Width bumped 1px -> 2px to match Verbatim's own bolder Pocket box
            // (see the sz=24 note on the docx writer) — kept in sync with
            // CARD_BOX_EXTRA_PX below, which reserves row height for exactly
            // this border weight.
            if prev_has_box {
                box_div = box_div.border_b_2().border_l_2().border_r_2();
            } else {
                box_div = box_div.border_2();
            }

            return box_div.into_any_element();
        }
    }
    match fold_toggle {
        Some(toggle) => div()
            .flex()
            .flex_row()
            .items_center()
            .child(toggle)
            .child(div().flex_1().min_w_0().child(line_div))
            .into_any_element(),
        None => line_div.into_any_element(),
    }
}

fn render_segment(
    text: String,
    run: Option<&Run>,
    style: SegmentStyle,
    zoom: f32,
    pal: Palette,
    cursor_style: CursorStyle,
    misspelled: Option<u32>,
    hidden: bool,
) -> AnyElement {
    /*
     * Applies the run's formatting first, then layers the cursor/selection
     * overlay on top — each of GPUI's style calls simply overwrites the
     * previous value for that field (confirmed against `Styled`'s own
     * implementation), so applying the overlay's `.bg()`/`.text_color()`
     * *after* the run's own correctly makes it win, matching real editors
     * drawing the cursor/selection on top of a highlight rather than
     * underneath it.
     *
     * Use flex_shrink(0.0) to prevent the div from expanding beyond the text width,
     * so highlights only extend as far as the text itself.
     */
    let el = apply_run_style(div().flex_shrink(0.0), run, zoom, pal);
    let el = match style {
        SegmentStyle::Cursor => match cursor_style {
            // Inverted block cursor: the page's text color as the block, the
            // page's background as the glyph on top of it.
            CursorStyle::Block => el.bg(rgb(pal.text)).text_color(rgb(pal.editor_bg)),
            // A caret belongs *between* two characters, so it's an overlay on
            // the left edge of the character the cursor is on, not a background
            // on the character itself. Absolutely positioned so the character
            // keeps its own colors and the line never shifts by the caret's
            // width as the cursor moves through it.
            CursorStyle::Line => el.relative().child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px((2.0 * zoom).max(1.0)))
                    .bg(rgb(pal.text)),
            ),
        },
        // The theme's selection color at ~50% opacity (spec 6.4 fixed this at
        // #264F78; it now follows the palette so it stays visible on light
        // backgrounds). `<< 8 | 0x80` packs the RGB into RGBA's high bits.
        SegmentStyle::Selection => el.bg(rgba((pal.selection << 8) | 0x80)),
        SegmentStyle::Plain => el,
    };
    /*
     * Spellcheck squiggle.
     *
     * A GPUI div has exactly one `underline` field, so a run that is *both*
     * formatting-underlined (`apply_run_style` above) and misspelled can't
     * express both decorations on one element — whichever is set last wins.
     * Word draws both, so on that collision the text is painted twice: the
     * real child keeps the formatting underline, and a second copy sits on
     * top with transparent glyphs and only the wavy decoration visible.
     *
     * The overlay is a child of this same element, so it inherits font
     * family/size/weight/style and its glyph advances match the real text
     * exactly — the squiggle can't drift out of alignment with the word.
     * `left_0().top_0()` (not `inset_0()`) lets it size to its own content
     * instead of stretching to the parent box.
     *
     * The common cases stay single-element: no squiggle, or a squiggle with
     * no competing formatting underline.
     */
    // Applied after the run style and the cursor/selection overlay, so it wins
    // the glyph color — but deliberately *not* over their backgrounds: the
    // caret and the selection stay visible while reading, which is what makes
    // the mode navigable rather than a blank page.
    let el = if hidden { el.text_color(transparent_black()) } else { el };

    let formatting_underline = run.is_some_and(|r| r.underline || r.double_underline);
    // Hidden text's real underline already renders transparent via the
    // `text_color(transparent_black())` above (`.underline()`'s own color
    // defaults to the glyph color — see the note below); gating out here
    // stops a hidden run from growing a *visible* second stroke where the
    // first one is invisible.
    let double_underline = !hidden && run.is_some_and(|r| r.double_underline);

    // GPUI's `UnderlineStyle` (style.rs) is thickness/color/wavy only — no
    // "double" variant — so `apply_run_style`'s `.underline()` can only ever
    // paint one stroke, which is why a Hat's double underline was rendering
    // as a single line even though the docx itself round-trips `w:u
    // w:val="double"` correctly (this is purely a rendering gap, not a
    // parse/save one). Fakes the second stroke with the same trick as the
    // misspelled-squiggle overlay just below: a transparent duplicate of the
    // glyphs carries its own `.underline()`, offset a few px below the real
    // line so two strokes paint. `.underline()` alone defaults its color to
    // the glyph color (line.rs's `unwrap_or(style_run.color)`), which here
    // is transparent — so the color must be set explicitly, same as the
    // squiggle overlay's `.text_decoration_color()` a few lines down.
    let underline_hex = run.and_then(|r| r.color.as_deref())
        .and_then(|c| u32::from_str_radix(c, 16).ok())
        .unwrap_or(pal.text);
    let double_underline_overlay = double_underline.then(|| {
        div()
            .absolute()
            .left_0()
            .top(px(3.0 * zoom))
            .text_color(transparent_black())
            .underline()
            .text_decoration_color(rgb(underline_hex))
            .child(text.clone())
    });

    match misspelled {
        Some(color) if formatting_underline => el
            .relative()
            .child(text.clone())
            .children(double_underline_overlay)
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .text_color(transparent_black())
                    .underline()
                    .text_decoration_wavy()
                    .text_decoration_color(rgb(color))
                    .child(text),
            )
            .into_any_element(),
        Some(color) => el
            .underline()
            .text_decoration_wavy()
            .text_decoration_color(rgb(color))
            .child(text)
            .into_any_element(),
        None if double_underline => el
            .relative()
            .child(text.clone())
            .children(double_underline_overlay)
            .into_any_element(),
        None => el.child(text).into_any_element(),
    }
}

fn apply_run_style(el: Div, run: Option<&Run>, zoom: f32, pal: Palette) -> Div {
    /*
     * Maps a `Run`'s fields onto GPUI style calls per spec 6.2 (extended
     * with italic/font/color, rich-text formatting plan Phase 1's scope
     * decision). `run: None` (formatting data unavailable for this
     * position) leaves `el` untouched, rendering as plain text.
     */
    let Some(run) = run else { return el };
    let mut el = el;
    if run.bold { el = el.font_weight(FontWeight::BOLD); }
    if run.italic { el = el.italic(); }
    if run.underline { el = el.underline(); }
    if run.double_underline { el = el.underline(); }
    if run.strikethrough { el = el.line_through(); }
    // Note: box_format is applied at the line level in render_line(), not here at the run level
    if run.emphasis_boxed { el = el.border_1().border_color(rgb(pal.text)); }
    if run.highlight {
        let base_hex = highlight_color_hex(&run.highlight_color);
        let text_hex = run.color.as_deref()
            .and_then(|c| u32::from_str_radix(c, 16).ok())
            .unwrap_or(pal.text);
        // Word darkens a light highlight sitting under light text so the text
        // stays legible (white on yellow is otherwise unreadable). This used
        // to be unconditional because the app was dark-only; now that the
        // default text color follows the theme, the condition does the gating
        // by itself — in light mode `p.text` is dark, so a yellow highlight
        // is left alone, which is what Word does there too.
        let highlight_hex = if is_light_color(base_hex) && is_light_color(text_hex) {
            darken_for_light_text(base_hex)
        } else {
            base_hex
        };
        el = el.bg(rgb(highlight_hex));
    }
    if run.size > 0 {
        el = el.text_size(px(run.size as f32 / 2.0 * zoom));
    }
    // `run.font` is only applied to rendering when it's one of
    // `CURATED_FONTS` — this app bundles all 4 weight/style faces for those
    // (`main.rs`'s `load_bundled_fonts`) so GPUI's font resolution always
    // has real bold/italic candidates to match against. An arbitrary
    // `run.font` (a real docx's own `<w:rFonts>`, e.g. "Calibri" or "Times
    // New Roman" from an imported document) is deliberately left applying
    // `FONT_FAMILY` on screen instead, but still kept as data so re-saving
    // round-trips it: every such font tested opened with bold/italic
    // silently not rendering, because GPUI's font resolution
    // (`gpui_wgpu::cosmic_text_system`'s `find_best_match`) short-circuits
    // (`candidates.len() == 1 => Ok(0)`, skipping weight/style matching)
    // whenever only one face of a family is actually loaded — the same bug
    // already diagnosed for the ribbon's B/I icons (`formatting_ribbon.rs`)
    // and for `FONT_FAMILY` itself (`main.rs`'s `load_bundled_fonts`).
    // Fonts the app can't vouch for having a full face set (anything not in
    // `CURATED_FONTS`) would hit this identically, so they don't get
    // applied here at all.
    if let Some(font_name) = run.font.as_deref() {
        if is_curated_font(font_name) {
            el = el.font_family(font_name.to_string());
        }
    }
    if let Some(color) = &run.color {
        if let Ok(value) = u32::from_str_radix(color, 16) {
            el = el.text_color(rgb(value));
        }
    }
    el
}

pub(crate) fn highlight_color_hex(name: &str) -> u32 {
    /*
     * Maps Word's highlight color names to their GPUI hex value (spec
     * 6.2's 15-entry table, plus a fallback for anything unrecognized).
     * Falls back to parsing `name` as a raw 6-digit hex string before
     * giving up — the HL Color dropdown's Custom option stores colors
     * this way since there's no name for an arbitrary RGB value.
     */
    match name {
        "yellow" => 0xFFD700,
        "green" => 0x00FF00,
        "blue" => 0x0000FF,
        "cyan" => 0x00FFFF,
        "magenta" => 0xFF00FF,
        "red" => 0xFF0000,
        "darkBlue" => 0x00008B,
        "darkCyan" => 0x008B8B,
        "darkGreen" => 0x006400,
        "darkMagenta" => 0x8B008B,
        "darkRed" => 0x8B0000,
        "darkYellow" => 0x8B8B00,
        "darkGray" => 0xA9A9A9,
        "lightGray" => 0xD3D3D3,
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        _ => u32::from_str_radix(name, 16).unwrap_or(0x888888),
    }
}

fn relative_luminance(hex: u32) -> f32 {
    /*
     * Standard perceived-luminance weighting (ITU-R BT.709 coefficients),
     * used to decide whether a color reads as "light" (spec: bug fix,
     * darken highlight under light text, matching Word's dark-mode
     * behavior).
     */
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn is_light_color(hex: u32) -> bool {
    relative_luminance(hex) > 0.5
}

fn darken_for_light_text(hex: u32) -> u32 {
    /*
     * Scales each channel down uniformly (preserving hue) so a light
     * highlight color stops washing out light-colored text on top of it.
     */
    const SCALE: f32 = 0.4;
    let r = (((hex >> 16) & 0xFF) as f32 * SCALE) as u32;
    let g = (((hex >> 8) & 0xFF) as f32 * SCALE) as u32;
    let b = ((hex & 0xFF) as f32 * SCALE) as u32;
    (r << 16) | (g << 8) | b
}

fn heading_font_size_px(heading: u8, zoom: f32) -> Option<f32> {
    /*
     * Spec 6.5's heading-level font size table, scaled by `zoom`. `None`
     * for `heading == 0` (body text — no override).
     */
    match heading {
        0 => None,
        1 => Some(24.0 * zoom),
        2 => Some(20.0 * zoom),
        3 => Some(18.0 * zoom),
        4..=6 => Some(16.0 * zoom),
        _ => Some(14.0 * zoom), // 7-9
    }
}

/// Vertical padding (`py(8.0)`, top+bottom) plus border (`border_2()`,
/// top+bottom) that `render_line`'s box wrapper (`FormatOp::Box`, used by
/// the Pocket card style) adds around the text itself, on top of the font's
/// own height — not scaled by `zoom` (matches the fixed `px(8.0)`/
/// `border_2()` calls in `render_line`). Used by `slot_count_for_paragraph`
/// to reserve enough uniform_list slots for a boxed line's real height.
/// Must move in lockstep with `render_line`'s border width (16px padding +
/// 2px*2 border sides = 20) — under-reserving here reproduces the box
/// clipping/overlap bug already fixed once (see this const's own history).
const CARD_BOX_EXTRA_PX: f32 = 20.0;

/// How many uniform-height `LINE_HEIGHT_PX` slots a paragraph's rendered
/// line actually needs. `gpui::uniform_list` (see `RowCache`/`render()`)
/// measures exactly one row and forces every item in the list to that same
/// height — a paragraph using a larger font (card styles Pocket/Hat/Block/
/// Tag via a run-level `FontSize`, or a document heading via
/// `heading_font_size_px`) would otherwise visually spill into the row
/// above it (`row_div` bottom-aligns its content — see its own `justify_end`
/// comment). `expand_rows_for_display` reserves `slot_count - 1` extra
/// blank rows *before* this paragraph's row so that overflow has somewhere
/// empty to land instead (see handoff.md's "card-styled lines now overlap"
/// writeup for the earlier, fillers-after history of this bug).
///
/// `font_px` and the real line height both scale by `zoom` the same way,
/// but `CARD_BOX_EXTRA_PX` doesn't, so `zoom` still has to be threaded
/// through rather than cancelling out. `normal_size_px` is the floor a run
/// with no explicit size falls back to — the actual configured default
/// (`normal_text_size_half_points`), not the stale `FONT_SIZE_PX` reference.
fn slot_count_for_paragraph(para: Option<&Paragraph>, zoom: f32, normal_size_px: f32) -> usize {
    let Some(para) = para else { return 1 };
    let run_max_px = para
        .runs
        .iter()
        .filter(|r| r.size > 0)
        .map(|r| r.size as f32 / 2.0 * zoom)
        .fold(0.0_f32, f32::max);
    // An explicit run-level size (card styles always set one, covering the
    // whole line) already reflects what's actually drawn and wins over the
    // heading fallback below it — using both would over-reserve whenever the
    // card style's real size is smaller than its heading level's generic
    // default (e.g. Tag: 13px run size vs. heading level 4's 16px fallback),
    // padding in a spurious blank row under every Tag line. `heading_px` is
    // only relevant when no run carries an explicit size — plain document
    // headings with no card-style override.
    let heading_px = heading_font_size_px(para.heading, zoom).unwrap_or(0.0);
    let font_px = if run_max_px > 0.0 { run_max_px } else { heading_px }.max(normal_size_px * zoom);
    let has_box = para.runs.iter().any(|r| r.box_format);
    let line_height = line_height_px(normal_size_px) * zoom;
    let needed_px = font_px * LINE_HEIGHT_RATIO + if has_box { CARD_BOX_EXTRA_PX } else { 0.0 };
    ((needed_px / line_height).ceil() as usize).max(1)
}

/// Expands the word-wrapped `rows` table (one entry per visual row) into a
/// `uniform_list`-facing "display rows" table that reserves extra blank
/// slots *before* any oversized paragraph (see `slot_count_for_paragraph`).
/// Before, not after: `row_div` bottom-aligns its content (`justify_end`),
/// so an oversized paragraph's real overflow spills upward out of its slot,
/// not downward — reserving the blank space after it left the overflow with
/// nowhere to go but into the row above (or the ribbon toolbar, for the
/// first line in the file).
///
/// Returns `(display_to_wrap, wrap_to_display)`:
/// - `display_to_wrap[display_idx]` is `Some(wrap_idx)` for the row that
///   holds real content (the last of its reserved slots), or `None` for a
///   blank spacer slot reserved before it.
/// - `wrap_to_display[wrap_idx]` is the display index a given wrap-table
///   row starts at — needed anywhere pixel math is keyed off a wrap-row
///   index (cursor position, scroll-to-cursor) so it accounts for spacer
///   rows inserted earlier in the document.
/// Which wrap rows paint nothing, so they can be dropped from the display list
/// entirely rather than left as blank lines.
///
/// Two independent reasons a row disappears:
///
/// * **Fold** hides whatever sits under a collapsed heading. `folded_paras` is
///   the per-paragraph map `AppState::folded_paragraphs` computed, which is
///   level-aware — collapsing a Pocket takes its Hats, Blocks and Tags with it,
///   not just its prose.
/// * **Invisibility** hides individual runs, and a row whose every run is
///   hidden has nothing left to paint.
///
/// Fold is checked first because it is coarser: a folded body row is gone
/// regardless of what it contains, including highlights.
pub(crate) fn hidden_wrap_rows(
    rows: &[(usize, usize, usize)],
    paragraphs: &[Paragraph],
    invisibility: bool,
    cite_size_half_points: u16,
    folded_paras: &[bool],
) -> Vec<bool> {
    if !invisibility && folded_paras.iter().all(|f| !f) {
        return vec![false; rows.len()];
    }
    rows.iter()
        .map(|&(li, row_start, row_end)| {
            let Some(para) = paragraphs.get(li) else { return true };
            if folded_paras.get(li).copied().unwrap_or(false) {
                return true;
            }
            if !invisibility {
                return false;
            }
            if para.heading != 0 {
                return false; // a card-style line stays whole
            }
            !paragraph_run_char_spans(para).into_iter().any(|(s, e, run_idx)| {
                // Only runs actually on this row decide it.
                if s.max(row_start) >= e.min(row_end) {
                    return false;
                }
                let run = para.runs.get(run_idx);
                !run_is_hidden(
                    true,
                    para.heading,
                    run.is_some_and(|r| r.highlight),
                    run.is_some_and(|r| r.bold),
                    run.map(|r| r.size).unwrap_or(0),
                    cite_size_half_points,
                )
            })
        })
        .collect()
}

pub(crate) fn expand_rows_for_display(
    rows: &[(usize, usize, usize)],
    paragraphs: &[Paragraph],
    zoom: f32,
    hidden: &[bool],
    normal_size_px: f32,
) -> (Vec<Option<usize>>, Vec<usize>) {
    let mut display_to_wrap = Vec::with_capacity(rows.len());
    let mut wrap_to_display = Vec::with_capacity(rows.len());
    for (wrap_idx, (li, _, _)) in rows.iter().enumerate() {
        // A fully hidden row gets no display slot, which is what closes up the
        // vertical gap. It still records a `wrap_to_display` entry pointing at
        // wherever the next visible row lands, so cursor scrolling on a hidden
        // row resolves to the nearest thing actually on screen.
        if hidden.get(wrap_idx).copied().unwrap_or(false) {
            wrap_to_display.push(display_to_wrap.len());
            continue;
        }
        // Blank filler slots go *before* the content row, not after: `row_div`
        // bottom-aligns its content (`justify_end`, for aligning mixed-size
        // text on the bottom rather than the top — see its own comment), so
        // a paragraph too tall for one slot overflows *upward* out of its
        // slot, not downward. Filler reserved after it left nothing above to
        // absorb that overflow — the box of the next Pocket/Hat/heading would
        // bleed into whatever sat above it (the previous line's text, or the
        // ribbon toolbar if it was the first line in the file) while the
        // filler itself just added unused space below. `wrap_to_display`
        // still has to point at the content slot specifically (not the first
        // filler), since cursor/scroll pixel math is keyed off it.
        let slots = slot_count_for_paragraph(paragraphs.get(*li), zoom, normal_size_px);
        for _ in 1..slots {
            display_to_wrap.push(None);
        }
        wrap_to_display.push(display_to_wrap.len());
        display_to_wrap.push(Some(wrap_idx));
    }
    (display_to_wrap, wrap_to_display)
}

fn usable_wrap_width(viewport_width_px: f32) -> f32 {
    /*
     * Computes how many pixels of width are available for wrapping text,
     * given the current viewport pixel width. Subtracts the left+right
     * content padding so it matches the actual usable text area (mirrors
     * CONTENT_PADDING_PX's use elsewhere).
     *
     * Returns a sentinel of `f32::MAX` when the viewport hasn't been laid
     * out yet (width <= 0, which happens on the very first frame before
     * `scroll_handle.bounds()` has real numbers) so lines render unwrapped
     * for that one frame instead of collapsing to almost nothing.
     */
    let usable = viewport_width_px - 2.0 * CONTENT_PADDING_PX;
    if usable <= 0.0 { f32::MAX } else { usable }
}

fn char_width_fn(cx: &App, font: Font, font_size_px: f32) -> impl FnMut(char) -> f32 {
    /*
     * Builds a closure that returns a character's real, rendered pixel
     * width for `font` at `font_size_px` (the zoomed font size — see
     * `AppState.zoom`), backed by GPUI's own `TextSystem::layout_width`
     * (the same glyph-shaping measurement GPUI itself uses to paint text,
     * cached internally per character).
     *
     * This replaces the old approach of assuming every character is
     * `CHAR_WIDTH_PX` wide for wrap purposes: that uniform estimate is
     * systematically wrong for narrow glyphs like '.' or '-', which render
     * much thinner than the average — folding lines dominated by them
     * (e.g. citation ellipses, en-dashes) far earlier than their actual
     * on-screen width warrants.
     *
     * The returned closure owns its own `Arc<TextSystem>` clone (cheap —
     * it's a refcount bump) and a resolved `FontId`, so it doesn't borrow
     * `cx` and can be passed freely into the pure wrap functions below.
     *
     * `layout_width` itself goes through GPUI's frame-based `LineLayoutCache`
     * (a locked, hash-keyed cache) on every call — cheap once per unique
     * character, but real documents call this once per *occurrence*, not
     * once per unique character. GPUI's own `LineWrapper::width_for_char`
     * (text_system/line_wrapper.rs) avoids exactly this cost with a local
     * per-char cache (`&mut self`, ASCII array + HashMap fallback); this
     * closure mirrors that technique and signature exactly (`FnMut`, not
     * `Fn`-plus-`RefCell` — an earlier version tried `RefCell` to keep a
     * `Fn` bound and hit a real "already borrowed" panic from an `if let
     * ... else { borrow_mut() }` whose immutable borrow's temporary lives
     * across the whole if/else; `FnMut` sidesteps that class of bug
     * entirely, same as the reference implementation) so a wrap pass over a
     * document with thousands of characters but only ~60-100 distinct ones
     * pays the expensive lookup only ~60-100 times, not once per character.
     * Scoped to a single closure instance (recreated on each wrap pass), so
     * it needs no invalidation logic.
     */
    let text_system = cx.text_system().clone();
    let font_id = text_system.resolve_font(&font);
    let mut ascii_cache: [Option<f32>; 128] = [None; 128];
    let mut other_cache: std::collections::HashMap<char, f32> = std::collections::HashMap::new();
    move |c: char| {
        // Matches render_line's tab->space substitution (see its comment):
        // the bundled fonts have no glyph for '\t' at all, so measuring it
        // directly would report ~0 width — wrap/scroll math needs the same
        // width the renderer actually paints, or the cursor's visual
        // column and the wrap point silently disagree.
        let c = if c == '\t' { ' ' } else { c };
        if (c as u32) < 128 {
            let idx = c as usize;
            if let Some(w) = ascii_cache[idx] {
                return w;
            }
            let w = text_system.layout_width(font_id, px(font_size_px), c).as_f32();
            ascii_cache[idx] = Some(w);
            w
        } else if let Some(&w) = other_cache.get(&c) {
            w
        } else {
            let w = text_system.layout_width(font_id, px(font_size_px), c).as_f32();
            other_cache.insert(c, w);
            w
        }
    }
}

pub(crate) fn visual_rows_for_viewport(
    cx: &App,
    lines: &[String],
    viewport_width_px: f32,
    zoom: f32,
    paragraphs: &[Paragraph],
    normal_size_px: f32,
) -> Vec<(usize, usize, usize)> {
    /*
     * Convenience wrapper combining `char_width_fn` + `usable_wrap_width` +
     * `build_visual_rows` — the single entry point every cx-having call
     * site (render, click/drag hit-testing, scroll-to-cursor, Up/Down,
     * auto-scroll) uses to build the row table, so they can never disagree
     * about wrap width or glyph metrics. `zoom` (`AppState.zoom`) scales
     * the font size wrapping is measured against, so a zoomed-in document
     * re-wraps at the same visual width it renders at.
     */
    let reference_px = FONT_SIZE_PX * zoom;
    let mut mono_measure = char_width_fn(cx, font(FONT_FAMILY), reference_px);
    let mut serif_measure = char_width_fn(cx, font(CURATED_SERIF_FONT), reference_px);

    // Wrapping has to measure each character at the size (and, since a run
    // can now pick a curated font — bug report: font selection didn't
    // visibly do anything — the font) it actually paints at: enlarging a
    // run makes its characters wider, and a row wrapped for the old
    // size/font then overflows the right edge instead of breaking.
    //
    // `build_visual_rows` walks lines in order, so a single-slot cache of the
    // current line's run-span table is enough to avoid rebuilding it per
    // character.
    let mut cached_spans: Option<(usize, Vec<(usize, usize, usize)>)> = None;
    let mut width_at = |line_idx: usize, char_idx: usize, ch: char| {
        if cached_spans.as_ref().map(|(i, _)| *i) != Some(line_idx) {
            let spans = paragraphs
                .get(line_idx)
                .map(paragraph_run_char_spans)
                .unwrap_or_default();
            cached_spans = Some((line_idx, spans));
        }
        let spans = cached_spans.as_ref().map(|(_, s)| s.as_slice()).unwrap_or(&[]);
        let para = paragraphs.get(line_idx);
        let size = effective_char_size_px(para, spans, char_idx, normal_size_px, zoom);
        let measured = match effective_char_font(para, spans, char_idx) {
            CURATED_SERIF_FONT => serif_measure(ch),
            _ => mono_measure(ch),
        };
        // A glyph's advance scales linearly with font size (true of any
        // font, not just a monospace one — vector outlines scale uniformly
        // with point size), so one real glyph measurement at the reference
        // size covers every size on the row.
        if reference_px > 0.0 { measured * (size / reference_px) } else { measured }
    };

    build_visual_rows(lines, usable_wrap_width(viewport_width_px), &mut width_at)
}

fn wrap_line_into_rows(
    chars: &[char],
    wrap_width_px: f32,
    width_of: &mut impl FnMut(usize, char) -> f32,
) -> Vec<(usize, usize)> {
    /*
     * Word-wraps one logical line's characters into visual rows whose
     * accumulated real glyph width (via `width_of`) stays within
     * `wrap_width_px`, breaking at the last space within budget when one
     * exists, or hard-breaking mid-word when a single word exceeds the
     * budget on its own. The space a word-boundary break lands on is
     * consumed (not repeated as a leading space on the next row), matching
     * normal word-wrap behaviour.
     *
     * Pure and independent of any real font/GPUI context — callers supply
     * `width_of`, so this stays unit-testable with synthetic width
     * functions (see the tests below for one that exercises variable-width
     * characters directly).
     *
     * Always returns at least one row, even for an empty line, so every
     * logical line still occupies its own visual slot — this is what lets
     * click/scroll math treat "row index" as a stable, always-present
     * coordinate.
     */
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let mut rows = Vec::new();
    let mut row_start = 0;
    while row_start < chars.len() {
        let mut width = 0.0f32;
        let mut i = row_start;
        let mut last_space: Option<usize> = None;
        while i < chars.len() {
            let char_width = width_of(i, chars[i]);
            // `i > row_start` forces at least one character onto every row,
            // even one whose width alone exceeds the budget — otherwise a
            // very narrow viewport (or a single unusually wide glyph) could
            // produce a zero-width row and loop forever.
            if width + char_width > wrap_width_px && i > row_start {
                break;
            }
            width += char_width;
            if chars[i] == ' ' && i > row_start {
                last_space = Some(i);
            }
            i += 1;
        }
        if i >= chars.len() {
            rows.push((row_start, chars.len()));
            break;
        }
        let row_end = last_space.unwrap_or(i);
        rows.push((row_start, row_end));
        // Skip the space itself when we broke on one, so it doesn't reappear
        // as a leading character on the next row.
        row_start = if last_space.is_some() { row_end + 1 } else { row_end };
    }
    rows
}

fn build_visual_rows(
    lines: &[String],
    wrap_width_px: f32,
    width_at: &mut impl FnMut(usize, usize, char) -> f32,
) -> Vec<(usize, usize, usize)> {
    /*
     * Flattens every logical line into an ordered list of visual rows, each
     * tagged with its owning logical line index and its [start, end) char
     * range within that line. Shared by rendering (which paints one
     * fixed-height div per row) and click/scroll math (which maps pixel
     * positions to/from this same row table) so all three always agree on
     * where each row's boundaries fall.
     */
    let mut rows = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let mut width_of = |idx: usize, ch: char| width_at(li, idx, ch);
        for (start, end) in wrap_line_into_rows(&chars, wrap_width_px, &mut width_of) {
            rows.push((li, start, end));
        }
    }
    rows
}

fn visual_row_for_line_col(rows: &[(usize, usize, usize)], logical_line: usize, char_col: usize) -> usize {
    /*
     * Finds the visual row that a (logical_line, char_col) cursor position
     * belongs to.
     *
     * A column sitting exactly at a row's end is ambiguous, and is resolved
     * differently depending on *why* the row ended:
     *   - Hard break (a long word forced mid-word, no space consumed): the
     *     next row starts exactly where this one ends, so the column is
     *     redirected to the *start* of that next row — matching how text
     *     editors visually carry the cursor onto the next wrapped row
     *     rather than trailing behind the break.
     *   - Soft break (`wrap_line_into_rows` consumed a space at the wrap
     *     point): the next row starts one character *past* this row's end,
     *     leaving a one-character gap for the consumed space. A column
     *     equal to this row's end is the space itself — not a valid
     *     position on the next row — so it stays here, trailing the last
     *     visible character. (Redirecting it forward regardless of this gap
     *     was the original bug: the next row's `row_start` could then
     *     exceed `char_col`, underflowing any `char_col - row_start` a
     *     caller computed downstream.)
     *   - Last row of the line: there's no next row to redirect to, so it
     *     stays here regardless.
     */
    let mut last_row_of_line = 0;
    for (idx, &(li, start, end)) in rows.iter().enumerate() {
        if li != logical_line { continue; }
        last_row_of_line = idx;
        if char_col >= start && char_col < end {
            return idx;
        }
        if char_col == end {
            let next_is_contiguous = rows.get(idx + 1)
                .map(|&(next_li, next_start, _)| next_li == li && next_start == end)
                .unwrap_or(false);
            if !next_is_contiguous {
                return idx;
            }
            // else: a hard break — fall through so the next iteration's
            // `char_col >= start && char_col < end` check picks it up.
        }
    }
    last_row_of_line
}

fn visual_row_step(
    rows: &[(usize, usize, usize)],
    current_row: usize,
    col_in_row: usize,
    delta: isize,
    paragraphs: &[Paragraph],
    normal_size_px: f32,
    zoom: f32,
) -> Option<(usize, usize)> {
    /*
     * Steps `delta` visual rows away from `current_row` (-1/+1 for Up/Down),
     * carrying `col_in_row` — the cursor's on-screen column within its
     * current row — over onto the target row.
     *
     * Not a raw index carry: `col_in_row` is first converted to a pixel X
     * position at the *current* row's own sizes (`x_for_col_in_row`), then
     * re-resolved into a column on the *target* row via that row's own
     * sizes (`column_for_x_in_row`) — two rows can render at different
     * sizes (a heading/Cite/Pocket run beside plain body text), and only
     * the pixel position is actually preserved by real "move up/down"
     * behavior; the same character index lands at very different on-screen
     * columns on rows of different sizes. Degenerates to a plain index
     * carry (clamped to the target row's width) when every character is
     * the same size, which is why the pre-existing tests below still hold.
     *
     * Returns `None` past the first/last visual row, i.e. Up on the first
     * row or Down on the last, matching the no-op behaviour of every other
     * boundary motion in this editor.
     */
    let target_row = current_row as isize + delta;
    if target_row < 0 || target_row as usize >= rows.len() {
        return None;
    }
    let (cur_line, cur_row_start, cur_row_end) = rows[current_row];
    let cur_spans = paragraphs.get(cur_line).map(paragraph_run_char_spans).unwrap_or_default();
    let target_x = x_for_col_in_row(col_in_row, paragraphs.get(cur_line), &cur_spans, cur_row_start, cur_row_end, normal_size_px, zoom);

    let (target_line, target_row_start, target_row_end) = rows[target_row as usize];
    let target_spans = paragraphs.get(target_line).map(paragraph_run_char_spans).unwrap_or_default();
    let target_col_in_row = column_for_x_in_row(target_x, paragraphs.get(target_line), &target_spans, target_row_start, target_row_end, normal_size_px, zoom);
    Some((target_line, target_row_start + target_col_in_row))
}

pub(crate) fn document_lines(content: &str) -> Vec<String> {
    /*
     * Splits document content into logical lines on '\n', matching the model
     * used throughout rendering and click/scroll math. An empty document is
     * still one (empty) line so the editor always has somewhere to place
     * the cursor.
     */
    if content.is_empty() {
        vec![String::new()]
    } else {
        content.split('\n').map(|l| l.to_string()).collect()
    }
}

/// The font size one character actually paints at, mirroring exactly how
/// `render()` layers its three sources: a run-level `FontSize` wins (card
/// styles Pocket/Hat/Block/Tag/Cite and Shrink all set one), otherwise the
/// paragraph's heading level, otherwise the configured body size. Already
/// multiplied by `zoom`.
fn effective_char_size_px(
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    char_idx: usize,
    normal_size_px: f32,
    zoom: f32,
) -> f32 {
    let run = spans
        .iter()
        .find(|(start, end, _)| char_idx >= *start && char_idx < *end)
        .and_then(|(_, _, run_idx)| para.and_then(|p| p.runs.get(*run_idx)));
    if let Some(run) = run {
        if run.size > 0 {
            return run.size as f32 / 2.0 * zoom;
        }
    }
    if let Some(size) = para.and_then(|p| heading_font_size_px(p.heading, zoom)) {
        return size;
    }
    normal_size_px * zoom
}

/// The font family one character actually paints at — mirrors
/// `apply_run_style`'s own rule exactly (a curated `run.font` wins,
/// anything else falls back to `FONT_FAMILY`), so wrap/click/scroll math
/// always measures against the same font that's actually painted. `'static`
/// since both possible results are the compile-time constants
/// `FONT_FAMILY`/`CURATED_SERIF_FONT`.
fn effective_char_font(para: Option<&Paragraph>, spans: &[(usize, usize, usize)], char_idx: usize) -> &'static str {
    let run = spans
        .iter()
        .find(|(start, end, _)| char_idx >= *start && char_idx < *end)
        .and_then(|(_, _, run_idx)| para.and_then(|p| p.runs.get(*run_idx)));
    match run.and_then(|r| r.font.as_deref()) {
        Some(CURATED_SERIF_FONT) => CURATED_SERIF_FONT,
        _ => FONT_FAMILY,
    }
}

/// `column_for_x_in_row`/`x_for_col_in_row`'s per-character advance-ratio
/// lookup: `CHAR_ADVANCE_RATIO` for the monospace primary font,
/// `SERIF_CHAR_ADVANCE_RATIO` for the curated serif font, mirroring
/// `effective_char_font`'s own font selection exactly (so click-mapping and
/// wrap agree on which font a character belongs to, even though wrap uses
/// real glyph measurement and this stays the cheaper ratio approximation —
/// see `SERIF_CHAR_ADVANCE_RATIO`'s doc comment for why that's an
/// acceptable tradeoff here).
fn effective_char_advance_ratio(para: Option<&Paragraph>, spans: &[(usize, usize, usize)], char_idx: usize) -> f32 {
    match effective_char_font(para, spans, char_idx) {
        CURATED_SERIF_FONT => SERIF_CHAR_ADVANCE_RATIO,
        _ => CHAR_ADVANCE_RATIO,
    }
}

/// Converts an x pixel offset (relative to the start of the text, i.e. after
/// subtracting the container's left padding) into a character column *within
/// one visual row*, rounding to the nearest character boundary and clamping
/// negative input to 0.
///
/// Walks the row character by character rather than dividing by a single
/// width: a row can mix font sizes (a Cite run inside a body line, a Shrunk
/// span, a heading), so no one width describes it. Dividing by a uniform
/// estimate put the cursor increasingly far from the pointer the further into
/// a differently-sized line the user clicked.
fn column_for_x_in_row(
    x: f32,
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    row_start: usize,
    row_end: usize,
    normal_size_px: f32,
    zoom: f32,
) -> usize {
    if x <= 0.0 || row_end <= row_start {
        return 0;
    }
    let mut left = 0.0f32;
    for char_idx in row_start..row_end {
        let width = effective_char_size_px(para, spans, char_idx, normal_size_px, zoom)
            * effective_char_advance_ratio(para, spans, char_idx);
        if x < left + width {
            // Past this character's midpoint means the nearer boundary is the
            // one after it — same round-to-nearest feel as clicking in Word.
            let col = char_idx - row_start;
            return if x - left > width / 2.0 { col + 1 } else { col };
        }
        left += width;
    }
    row_end - row_start
}

/// Inverse of `column_for_x_in_row`: the pixel X position `col_in_row`
/// characters into a row, summing each character's own effective size.
///
/// Bug report: pressing `k`/`j` (`visual_row_step`) to move between two
/// rows of different font size (e.g. off the end of an 11pt line onto a
/// larger-sized one above, or vice versa) landed the cursor far off to one
/// side — it was carrying the raw character *index* across rows, but two
/// rows at different sizes don't share one width-per-character, so the
/// same index sits at very different on-screen X positions on each. This
/// is the piece that lets `visual_row_step` convert the current row's
/// column to a real pixel X *before* re-resolving it against the target
/// row's own sizes via `column_for_x_in_row` — only pixel position is
/// actually preserved by real "move up/down" behavior.
fn x_for_col_in_row(
    col_in_row: usize,
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    row_start: usize,
    row_end: usize,
    normal_size_px: f32,
    zoom: f32,
) -> f32 {
    let end = row_start + col_in_row.min(row_end - row_start);
    let mut x = 0.0f32;
    for char_idx in row_start..end {
        x += effective_char_size_px(para, spans, char_idx, normal_size_px, zoom)
            * effective_char_advance_ratio(para, spans, char_idx);
    }
    x
}

fn line_for_y(y: f32, line_height: f32, num_rows: usize) -> usize {
    /*
     * Converts a y pixel offset (relative to the start of the text) into a
     * 0-indexed visual row number, clamped to `num_rows - 1` so a click
     * below the last row still lands on it rather than panicking on an
     * out-of-range row index.
     */
    if line_height <= 0.0 || num_rows == 0 { return 0; }
    if y <= 0.0 { return 0; }
    ((y / line_height) as usize).min(num_rows - 1)
}

/// GPUI's own measured per-row pixel height — `UniformListScrollHandle`'s
/// `last_item_size`, populated every `prepaint` from `measure_item`'s real
/// Taffy layout of one row div — rather than independently recomputing
/// `line_height_px(font_size_px) * zoom` and trusting it matches. Device
/// logging from a real bug report showed those two diverging by a fraction
/// of a pixel on the reporter's hardware (consistent with device-pixel
/// snapping), which is exactly what `line_col_from_mouse_position` divides
/// local Y by — see its own comment on the `row_height_px` param.
///
/// `ItemSize.item` is *not* one row's height — it's `padded_bounds.size`,
/// the whole viewport box (confirmed against the vendored
/// `uniform_list.rs`'s `prepaint`, `item: padded_bounds.size`). The real
/// per-row height only recovers by dividing `ItemSize.contents.height`
/// (`longest_item_size.height * item_count`, i.e. total content extent) by
/// `item_count` — the same value this file's own `content_size.height =
/// longest_item_size.height * self.item_count` computation is built from.
/// `item_count` must be the same `display_to_wrap.len()` passed to
/// `uniform_list(...)`, or this divides by the wrong count.
///
/// Shared by `TextEditor`'s own click/drag handlers and
/// `AutoScroller::tick` (which resolves a cursor position during
/// edge-scrolling), so neither can drift from the other. Falls back to the
/// computed value before any layout has run yet (`last_item_size` is
/// `None` until then) or when `item_count` is 0, same "not laid out yet"
/// sentinel pattern `usable_wrap_width` already uses.
pub(crate) fn real_row_height_px(handle: &UniformListScrollHandle, item_count: usize, font_size_px: f32, zoom: f32) -> f32 {
    if item_count == 0 {
        return line_height_px(font_size_px) * zoom;
    }
    handle
        .0
        .borrow()
        .last_item_size
        .map(|s| s.contents.height.as_f32() / item_count as f32)
        .filter(|h| *h > 0.0)
        .unwrap_or_else(|| line_height_px(font_size_px) * zoom)
}

pub(crate) fn line_col_from_mouse_position(
    position: Point<Pixels>,
    content_bounds: Bounds<Pixels>,
    scroll_offset_y: f32,
    rows: &[(usize, usize, usize)],
    display_to_wrap: &[Option<usize>],
    zoom: f32,
    font_size_px: f32,
    paragraphs: &[Paragraph],
    // GPUI's own measured per-row pixel height (`real_row_height_px`),
    // not `line_height_px(font_size_px) * zoom` recomputed here — bug report:
    // clicking near the bottom of a large file landed the cursor several
    // lines above the pointer. Root cause, confirmed via device logging: on
    // the reporter's hardware, GPUI's `uniform_list` actually measured and
    // positioned every row at a slightly different pixel height than this
    // formula computes (consistent with device-pixel snapping) — a
    // fraction-of-a-pixel-per-row error that's invisible near the top of a
    // document and compounds, several rows deep, near the bottom of a long
    // one. `font_size_px`/`zoom` are still used below for horizontal
    // (per-character) sizing, which is unrelated and unaffected.
    row_height_px: f32,
) -> (usize, usize) {
    /*
     * Converts a window-space mouse position into a (logical_line,
     * char_column) pair. Shared by on_mouse_down (plain click) and
     * on_mouse_move (click-drag, including `AutoScroller`'s edge-scroll
     * ticks) so all three can never disagree about where a given pixel
     * position maps to.
     *
     * Takes the same visual-row table `render()` paints from (built via
     * `visual_rows_for_viewport`, which needs a live GPUI context for real
     * glyph-width measurement) rather than rebuilding it internally — this
     * function itself stays plain and cx-free. A pixel Y is first resolved
     * to a *display* row (see `expand_rows_for_display` — a wrapped
     * logical line spans several visual rows, and an oversized card-style/
     * heading row reserves extra blank spacer rows after it) and only then
     * translated back to the logical (line, column) pair that `AppState`
     * understands.
     *
     * `content_bounds` is the editor's fixed viewport box — GPUI's own
     * layout bounds for the tracked div (`ScrollHandle::bounds()`), which
     * doesn't move when the document scrolls, so a position relative to it
     * alone would describe screen position, not document position, on any
     * document taller than one screen. `scroll_offset_y` is
     * `ScrollHandle::offset().y`, which goes more negative the further the
     * document has been scrolled down — subtracting it converts
     * screen-relative Y into document-relative Y.
     */
    // Subtract the container's padding (spec: `.p(px(16.0))` in render())
    // so (0, 0) lines up with the first character of the text.
    let local_x = position.x.as_f32() - content_bounds.origin.x.as_f32() - CONTENT_PADDING_PX;
    let local_y = position.y.as_f32() - content_bounds.origin.y.as_f32() - CONTENT_PADDING_PX - scroll_offset_y;
    let display_row = line_for_y(local_y, row_height_px, display_to_wrap.len());

    // A click landing on a blank spacer slot (the empty space an oversized
    // row's content visually spills into) belongs to that row, not to
    // whatever comes after it — walk back to the nearest real content row.
    let visual_row = nearest_wrap_row_for_display_row(display_to_wrap, display_row);

    // The row has to be resolved *before* the column: which characters are on
    // this row determines what they're sized at, and therefore how wide each
    // one is.
    let (logical_line, row_start, row_end) = rows[visual_row];
    let para = paragraphs.get(logical_line);
    let spans = para.map(paragraph_run_char_spans).unwrap_or_default();
    // Center/Right alignment (`render_line`'s `justify_center`/`justify_end`)
    // indents a row's text from the row's raw left edge — `local_x` measures
    // from that same raw edge, so without this every click on centered or
    // right-aligned text resolved to a column shifted right by exactly the
    // alignment's own indent. `x_for_col_in_row` is the same real-width
    // summation `column_for_x_in_row` uses, so this indent always matches
    // what actually got laid out, mixed run sizes included. `avail_width`
    // mirrors `usable_wrap_width`, the same available width word-wrap itself
    // assumed the row's content fills (`render()`'s `content_el` sits in a
    // `.w_full()` row with the same padding subtracted).
    use crate::docx_parser::Alignment;
    let text_width = x_for_col_in_row(row_end - row_start, para, &spans, row_start, row_end, font_size_px, zoom);
    let avail_width = usable_wrap_width(content_bounds.size.width.as_f32());
    let indent = match para.map(|p| p.alignment) {
        Some(Alignment::Center) => ((avail_width - text_width) / 2.0).max(0.0),
        Some(Alignment::Right) => (avail_width - text_width).max(0.0),
        // Left is flush already; Justify only stretches inter-word gaps
        // (render()'s own comment calls it an approximation), which doesn't
        // move the row's first character, so no indent applies there either.
        _ => 0.0,
    };
    let col_in_row = column_for_x_in_row(
        local_x - indent, para, &spans, row_start, row_end, font_size_px, zoom,
    );
    let col = row_start + col_in_row.min(row_end - row_start);
    (logical_line, col)
}

fn selection_span_for_line(line: &str, line_byte_start: usize, sel_start: usize, sel_end: usize) -> Option<(usize, usize)> {
    /*
     * Maps a selection's document-wide byte range onto the char-column range
     * of a single line, or None if the selection doesn't touch this line at
     * all (including the boundary case where the selection ends exactly at
     * this line's first byte, or starts exactly at its last byte — those
     * describe a selection that stops at the newline, not one that includes
     * this line's visible characters). `sel_start`/`sel_end` must already be
     * normalized so `sel_start <= sel_end`.
     */
    if sel_start == sel_end { return None; } // nothing selected
    let line_byte_end = line_byte_start + line.len();
    if sel_end <= line_byte_start || sel_start >= line_byte_end { return None; }

    // Clamp each selection edge into this line's byte range, then convert
    // that relative byte offset into a char column (not byte column).
    let to_col = |byte: usize| -> usize {
        let rel = byte.saturating_sub(line_byte_start).min(line.len());
        line[..rel].chars().count()
    };
    let start_col = to_col(sel_start.max(line_byte_start));
    let end_col = to_col(sel_end.min(line_byte_end));
    if start_col == end_col { return None; } // e.g. an empty line fully inside the selection
    Some((start_col, end_col))
}

/// How a single rendered line segment should be styled — plain text, the
/// cursor's highlighted cell, or the selection's background overlay.
/// How the caret is drawn. Vim users expect the block cursor that sits *on* a
/// character (that's what vim's own motions address); everyone else expects the
/// thin line *between* characters that Word and every other editor uses.
/// Follows the `vim` setting itself, not the current vim mode.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum CursorStyle {
    Block,
    Line,
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum SegmentStyle {
    Plain,
    Cursor,
    Selection,
}

fn line_segments(
    len: usize,
    cursor_col: Option<usize>,
    selections: &[(usize, usize)],
    misspelled: &[(usize, usize)],
) -> Vec<(usize, usize, SegmentStyle, bool)> {
    /*
     * Splits a line of `len` characters into styled segments by merging the
     * cursor position (if this is the cursor's line) and the selection's
     * char-column range (if any) into one ordered list of breakpoints, then
     * classifying each resulting [start, end) run. The cursor always gets
     * its own single-character segment even where it sits inside a
     * selection — real editors draw the block cursor on top of selection
     * highlighting, not the other way around. A cursor sitting past the
     * last character (end of line) produces a synthetic zero-width segment
     * (`len, len`) that the renderer turns into a single highlighted space.
     *
     * Misspelled ranges are a *third*, orthogonal overlay rather than another
     * `SegmentStyle` variant: a misspelled word can also be selected, or sit
     * under the cursor, and those have to keep their own background. So they
     * contribute breakpoints like the others but come back as the trailing
     * `bool` on each segment — "paint a squiggle under this run too".
     */
    if cursor_col.is_none() && selections.is_empty() && misspelled.is_empty() {
        return vec![(0, len, SegmentStyle::Plain, false)];
    }

    let mut breaks: Vec<usize> = vec![0, len];
    if let Some(c) = cursor_col {
        let c = c.min(len);
        breaks.push(c);
        if c < len { breaks.push(c + 1); }
    }
    for &(s, e) in selections {
        breaks.push(s.min(len));
        breaks.push(e.min(len));
    }
    for &(s, e) in misspelled {
        breaks.push(s.min(len));
        breaks.push(e.min(len));
    }
    breaks.sort_unstable();
    breaks.dedup();

    let mut segments = Vec::new();
    for w in breaks.windows(2) {
        let (start, end) = (w[0], w[1]);
        let is_cursor = cursor_col.map(|c| c.min(len) == start && end == start + 1).unwrap_or(false);
        let in_selection = selections
            .iter()
            .any(|&(s, e)| start >= s.min(len) && end <= e.min(len));
        let style = if is_cursor {
            SegmentStyle::Cursor
        } else if in_selection {
            SegmentStyle::Selection
        } else {
            SegmentStyle::Plain
        };
        let is_misspelled = misspelled
            .iter()
            .any(|&(s, e)| start >= s.min(len) && end <= e.min(len));
        segments.push((start, end, style, is_misspelled));
    }

    // A cursor at (or past) the end of the line has no character to occupy,
    // so the main loop above never produces a segment for it — append one.
    if let Some(c) = cursor_col {
        if c >= len {
            segments.push((len, len, SegmentStyle::Cursor, false));
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    // Import only the two functions under test, not `super::*` — text_editor.rs
    // has `use gpui::*;` at module scope, and gpui exports its own `test`
    // attribute macro (for async GPUI tests) that shadows std's `#[test]` and
    // sends the test-attribute expansion into infinite recursion if it's in
    // scope here.
    use super::{
        column_for_x_in_row, x_for_col_in_row, effective_char_size_px, effective_char_font,
        effective_char_advance_ratio, line_for_y, selection_span_for_line, row_edge_target_col, RowEdge,
        nearest_wrap_row_for_display_row,
        line_segments, SegmentStyle, CHAR_ADVANCE_RATIO, SERIF_CHAR_ADVANCE_RATIO,
        FONT_FAMILY, CURATED_SERIF_FONT,
        usable_wrap_width, wrap_line_into_rows, build_visual_rows, visual_row_for_line_col,
        visual_row_step, document_lines, highlight_color_hex, heading_font_size_px,
        relative_luminance, is_light_color, darken_for_light_text,
        hidden_wrap_rows, page_scroll_offset, run_is_hidden, row_cache_is_valid_for, RowCache, slot_count_for_paragraph, expand_rows_for_display,
        spell_ranges_cached, SpellCache, line_height_px, LINE_HEIGHT_PX, display_line,
        line_col_from_mouse_position, real_row_height_px,
    };
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;
    use crate::state::AppState;
    use crate::docx_parser::{Paragraph, Run, Alignment};
    use std::time::Instant;

    /// A plain body-text row: 11px text (settings.conf's `normal_text_size`)
    /// means 6.6px characters, so a click N characters in resolves to column N.
    /// The old uniform-width math divided by 8.4px and landed short.
    /// The reported bug: bumping the font size on text near the right edge let
    /// it run off the editor instead of re-wrapping. Wrapping measured every
    /// character at one fixed size, so a row wrapped for 11px text still held
    /// the same characters after they grew to 24px — and the extra width spilled
    /// past the right border. Widths now come from each character's own size,
    /// so a larger run breaks sooner.
    #[test]
    fn test_wrap_breaks_sooner_when_characters_are_larger() {
        let chars: Vec<char> = "aaaa aaaa aaaa aaaa".chars().collect();

        // 6.6px characters (11pt body) — plenty fits in 100px.
        let mut small = |_: usize, _: char| 6.6f32;
        let small_rows = wrap_line_into_rows(&chars, 100.0, &mut small);

        // 14.4px characters (24pt) — the same text needs more rows.
        let mut large = |_: usize, _: char| 14.4f32;
        let large_rows = wrap_line_into_rows(&chars, 100.0, &mut large);

        assert!(
            large_rows.len() > small_rows.len(),
            "larger text must wrap into more rows: {} vs {}",
            large_rows.len(),
            small_rows.len(),
        );
        // And no row may exceed the budget at the larger size.
        for (start, end) in &large_rows {
            let width = (end - start) as f32 * 14.4;
            assert!(width <= 100.0, "row {start}..{end} is {width}px, over budget");
        }
    }

    /// Bug report: pressing Tab while editing appeared to do nothing —
    /// no visible gap, cursor didn't move. Root cause: the bundled fonts
    /// have no glyph for U+0009, so GPUI paints a raw '\t' with zero
    /// width. `display_line` substitutes it for a space at render time
    /// only, which must be a strict one-char-for-one-char swap — anything
    /// else would desync every offset-based cursor/selection computation
    /// that indexes into the line by character position.
    #[test]
    fn test_display_line_swaps_tab_for_space_one_for_one() {
        assert_eq!(display_line("a\tb"), "a b");
        assert_eq!(
            "a\tb".chars().count(),
            display_line("a\tb").chars().count(),
            "substitution must not change the char count offsets are computed against"
        );
        // No tab present: no allocation-worthy change, and (implementation
        // detail worth locking in) no unnecessary copy.
        assert!(matches!(display_line("plain text"), std::borrow::Cow::Borrowed(_)));
    }

    /// A size change partway along a row has to be respected mid-row, which is
    /// why the width callback receives the character's index.
    #[test]
    fn test_wrap_respects_a_size_change_partway_through_a_row() {
        let chars: Vec<char> = "aaaaaaaaaaaaaaaaaaaa".chars().collect(); // 20
        // First 10 characters small, the rest large.
        let mut width_of = |i: usize, _: char| if i < 10 { 5.0f32 } else { 20.0 };
        let rows = wrap_line_into_rows(&chars, 100.0, &mut width_of);

        // 10 small chars fill exactly 50px, then two large ones reach 90px and
        // a third would overflow — so the first row must stop before char 13.
        assert!(rows[0].1 <= 13, "first row ran to {} despite the larger tail", rows[0].1);
        assert!(rows.len() > 1, "the larger tail must be pushed onto another row");
    }

    #[test]
    fn test_column_in_row_uses_the_rendered_body_size() {
        let char_width = 11.0 * CHAR_ADVANCE_RATIO;
        assert!((char_width - 6.6).abs() < 0.01, "got {char_width}");
        for col in [0usize, 1, 5, 20, 60] {
            let x = col as f32 * char_width;
            assert_eq!(
                column_for_x_in_row(x, None, &[], 0, 80, 11.0, 1.0),
                col,
                "click at column {col}",
            );
        }
    }

    #[test]
    fn test_column_in_row_rounds_to_the_nearest_boundary() {
        let w = 11.0 * CHAR_ADVANCE_RATIO; // 6.6
        assert_eq!(column_for_x_in_row(w * 0.4, None, &[], 0, 10, 11.0, 1.0), 0);
        assert_eq!(column_for_x_in_row(w * 0.6, None, &[], 0, 10, 11.0, 1.0), 1);
    }

    #[test]
    fn test_column_in_row_clamps_negative_and_past_the_end() {
        assert_eq!(column_for_x_in_row(-30.0, None, &[], 0, 10, 11.0, 1.0), 0);
        assert_eq!(column_for_x_in_row(9999.0, None, &[], 0, 10, 11.0, 1.0), 10);
        // An empty row has no column but 0.
        assert_eq!(column_for_x_in_row(50.0, None, &[], 4, 4, 11.0, 1.0), 0);
    }

    /// Bug report: clicking a centered line placed the cursor to the right of
    /// the pointer by roughly the centering indent. `line_col_from_mouse_position`
    /// used to measure `local_x` from the row's raw left edge regardless of
    /// alignment, which is only correct for `Alignment::Left`.
    #[test]
    fn line_col_from_mouse_position_accounts_for_center_alignment() {
        use gpui::{point, px, size, Bounds};
        let para = Paragraph { alignment: Alignment::Center, ..Paragraph::default() };
        let paragraphs = vec![para];
        let rows = vec![(0usize, 0usize, 10usize)]; // one row, 10 chars
        let display_to_wrap = vec![Some(0usize)];
        let content_bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(232.0), px(100.0)));
        // avail_width = 232 - 2*16 = 200; text_width = 10 * (11*CHAR_ADVANCE_RATIO) = 66;
        // indent = (200 - 66) / 2 = 67.
        let indent = 67.0;
        // Clicking exactly at the text's real (indented) left edge must land on
        // column 0. Without accounting for alignment, this same pixel — 67px
        // right of the row's raw edge — would resolve as if it were 67px into
        // a row that starts flush left, landing at the row's *last* column
        // instead (67 / 6.6 ≈ 10, clamped to row_end - row_start).
        let position = point(px(16.0 + indent), px(0.0));
        let (line, col) = line_col_from_mouse_position(
            position, content_bounds, 0.0, &rows, &display_to_wrap, 1.0, 11.0, &paragraphs,
            line_height_px(11.0),
        );
        assert_eq!((line, col), (0, 0));
    }

    /// Bug report: clicking near the bottom of a large file landed the
    /// cursor several lines above the pointer. Root cause, confirmed via
    /// device logging: GPUI's actual measured per-row height can differ
    /// (by a fraction of a pixel — device-pixel snapping) from
    /// `line_height_px(font_size_px) * zoom` recomputed independently, and
    /// that per-row error compounds with row index. `real_row_height_px`
    /// must prefer GPUI's own measurement once one exists.
    #[test]
    fn real_row_height_px_prefers_gpuis_measured_height_over_the_computed_one() {
        let handle = gpui::UniformListScrollHandle::new();
        let item_count = 100usize;
        // Before any layout has run, there's nothing measured yet — falls
        // back to the computed value.
        assert_eq!(real_row_height_px(&handle, item_count, 11.0, 1.0), line_height_px(11.0));

        // `ItemSize.item` is the *viewport's* box (`padded_bounds.size` in
        // GPUI's own `prepaint`, confirmed against the vendored source), not
        // one row's height — a stub that (wrongly) set `item.height` to the
        // real row height would pass even if `real_row_height_px` read the
        // wrong field, which is exactly the bug this test needs to catch.
        // The real per-row height only recovers from `contents.height /
        // item_count` (`contents.height = longest_item_size.height *
        // item_count`), matching this reported bug's device data (measured
        // 15.5 vs. computed ~15.71).
        handle.0.borrow_mut().last_item_size = Some(gpui::ItemSize {
            item: gpui::size(gpui::px(999.0), gpui::px(524.5)), // a viewport-sized box
            contents: gpui::size(gpui::px(999.0), gpui::px(15.5 * item_count as f32)),
        });
        assert_eq!(real_row_height_px(&handle, item_count, 11.0, 1.0), 15.5);
    }

    #[test]
    fn test_column_in_row_tracks_zoom() {
        for zoom in [0.5f32, 1.0, 1.5, 2.0] {
            let w = 11.0 * CHAR_ADVANCE_RATIO * zoom;
            assert_eq!(column_for_x_in_row(12.0 * w, None, &[], 0, 40, 11.0, zoom), 12, "zoom {zoom}");
        }
    }

    /// The reported regression: a line rendered at a *different* size than the
    /// body default. A Block card style sets a run-level 16pt (32 half-points),
    /// so its characters are 9.6px, not 6.6px — clicking 10 characters in must
    /// still land on column 10.
    #[test]
    fn test_column_in_row_follows_a_run_level_font_size() {
        let para = Paragraph {
            runs: vec![Run { text: "0123456789abcdef".into(), size: 32, ..Run::default() }],
            ..Paragraph::default()
        };
        let spans = crate::document_ops::paragraph_run_char_spans(&para);
        let block_char = 16.0 * CHAR_ADVANCE_RATIO; // 9.6px
        for col in [1usize, 5, 10] {
            assert_eq!(
                column_for_x_in_row(col as f32 * block_char, Some(&para), &spans, 0, 16, 11.0, 1.0),
                col,
                "block-sized column {col}",
            );
        }
        // Read through the body-size estimate the same pixel lands far right.
        let body_char = 11.0 * CHAR_ADVANCE_RATIO;
        assert!((10.0 * block_char / body_char).round() as usize > 10);
    }

    /// `x_for_col_in_row` is `column_for_x_in_row`'s inverse — round-tripping
    /// a column through both must return the same column, both on a plain
    /// uniform-size row and on a run-level-sized one (`visual_row_step`
    /// relies on exactly this to convert a column to pixels on one row and
    /// back to a column on another).
    #[test]
    fn test_x_for_col_in_row_round_trips_with_column_for_x_in_row() {
        for col in [0usize, 1, 5, 10] {
            let x = x_for_col_in_row(col, None, &[], 0, 10, 11.0, 1.0);
            assert_eq!(column_for_x_in_row(x, None, &[], 0, 10, 11.0, 1.0), col);
        }

        let para = Paragraph {
            runs: vec![Run { text: "0123456789abcdef".into(), size: 32, ..Run::default() }],
            ..Paragraph::default()
        };
        let spans = crate::document_ops::paragraph_run_char_spans(&para);
        for col in [1usize, 5, 10] {
            let x = x_for_col_in_row(col, Some(&para), &spans, 0, 16, 11.0, 1.0);
            assert_eq!(column_for_x_in_row(x, Some(&para), &spans, 0, 16, 11.0, 1.0), col);
        }
    }

    #[test]
    fn test_x_for_col_in_row_clamps_past_the_end() {
        assert_eq!(x_for_col_in_row(50, None, &[], 0, 10, 11.0, 1.0), x_for_col_in_row(10, None, &[], 0, 10, 11.0, 1.0));
    }

    /// Bug report: `$`/`0`/`^`/Home/End jumped to the edge of the whole
    /// wrapped paragraph instead of the current visual row. This is the
    /// pure column-resolution `move_cursor_to_row_edge` is built on —
    /// exercised directly against a *row* range that's narrower than the
    /// full line, which is exactly the wrapped-continuation-row case.
    #[test]
    fn test_row_edge_target_col_start_and_end_use_the_row_not_the_line() {
        // "hello world" wrapped into rows [0,6) ("hello ") and [6,11)
        // ("world") — row 2 is the one under test.
        let chars: Vec<char> = "hello world".chars().collect();
        assert_eq!(row_edge_target_col(RowEdge::Start, &chars, 6, 11), 6, "row start, not line start (0)");
        assert_eq!(row_edge_target_col(RowEdge::End, &chars, 6, 11), 11, "row end, not line end");
        assert_eq!(row_edge_target_col(RowEdge::End, &chars, 0, 6), 6, "the *first* row's own end, not the whole line's");
    }

    #[test]
    fn test_row_edge_target_col_first_non_blank_skips_leading_whitespace_within_the_row() {
        let chars: Vec<char> = "one   two".chars().collect();
        // Row [3, 9) is "   two" — first non-blank is 't' at index 6.
        assert_eq!(row_edge_target_col(RowEdge::FirstNonBlank, &chars, 3, 9), 6);
    }

    #[test]
    fn test_row_edge_target_col_first_non_blank_all_whitespace_row_lands_at_its_end() {
        let chars: Vec<char> = "one    ".chars().collect();
        // Row [3, 7) is all spaces — matches real vim's `^` on a blank line.
        assert_eq!(row_edge_target_col(RowEdge::FirstNonBlank, &chars, 3, 7), 7);
    }

    /// Bug report: H/M/L landed on the wrong row whenever a card-style row
    /// sat above the viewport, from dividing raw scroll offset by a single
    /// `line_height` (assumes every row is the same height). This is the
    /// piece that translates a *display* row (uniform height, includes
    /// spacer slots an oversized row reserves) back to the nearest real
    /// content row — the same rule a mouse click landing on a spacer slot
    /// already used.
    #[test]
    fn test_nearest_wrap_row_for_display_row_walks_back_over_spacer_slots() {
        // Display rows: 0 = real row 0, 1/2 = spacer slots (an oversized
        // row 0 reserved 3 display slots total), 3 = real row 1.
        let display_to_wrap = vec![Some(0), None, None, Some(1)];
        assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 0), 0);
        assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 1), 0, "spacer slot belongs to the row before it");
        assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 2), 0);
        assert_eq!(nearest_wrap_row_for_display_row(&display_to_wrap, 3), 1);
    }

    /// A row mixing sizes — a Cite-sized run after body text — has no single
    /// character width at all, which is why the column is walked rather than
    /// divided.
    #[test]
    fn test_column_in_row_handles_mixed_sizes_within_one_row() {
        let para = Paragraph {
            runs: vec![
                Run { text: "aaaaa".into(), ..Run::default() },          // body 11px
                Run { text: "bbbbb".into(), size: 26, ..Run::default() }, // 13pt
            ],
            ..Paragraph::default()
        };
        let spans = crate::document_ops::paragraph_run_char_spans(&para);
        let body = 11.0 * CHAR_ADVANCE_RATIO;
        let cite = 13.0 * CHAR_ADVANCE_RATIO;

        // Boundary between the two runs.
        assert_eq!(column_for_x_in_row(5.0 * body, Some(&para), &spans, 0, 10, 11.0, 1.0), 5);
        // Three characters into the larger run.
        let x = 5.0 * body + 3.0 * cite;
        assert_eq!(column_for_x_in_row(x, Some(&para), &spans, 0, 10, 11.0, 1.0), 8);
    }

    #[test]
    fn test_effective_char_size_prefers_run_then_heading_then_body() {
        let body = Paragraph { runs: vec![Run { text: "ab".into(), ..Run::default() }], ..Paragraph::default() };
        let spans = crate::document_ops::paragraph_run_char_spans(&body);
        assert_eq!(effective_char_size_px(Some(&body), &spans, 0, 11.0, 1.0), 11.0);

        let heading = Paragraph { runs: vec![Run { text: "ab".into(), ..Run::default() }], heading: 1, ..Paragraph::default() };
        let hspans = crate::document_ops::paragraph_run_char_spans(&heading);
        assert_eq!(effective_char_size_px(Some(&heading), &hspans, 0, 11.0, 1.0), 24.0);

        // A run-level size wins over the heading level.
        let both = Paragraph { runs: vec![Run { text: "ab".into(), size: 32, ..Run::default() }], heading: 1, ..Paragraph::default() };
        let bspans = crate::document_ops::paragraph_run_char_spans(&both);
        assert_eq!(effective_char_size_px(Some(&both), &bspans, 0, 11.0, 1.0), 16.0);

        // No paragraph data at all falls back to the body size.
        assert_eq!(effective_char_size_px(None, &[], 0, 11.0, 2.0), 22.0);
    }

    /// Bug report: choosing a font from the Font Family picker didn't
    /// visibly change anything. `effective_char_font` is the piece that
    /// tells wrap/click/scroll math which font a character actually paints
    /// at — it must mirror `apply_run_style`'s own rule (a curated
    /// `run.font` wins, anything else falls back to `FONT_FAMILY`) or wrap
    /// decisions and what's on screen would disagree.
    #[test]
    fn test_effective_char_font_follows_run_font_only_when_curated() {
        let none = Paragraph { runs: vec![Run { text: "ab".into(), ..Run::default() }], ..Paragraph::default() };
        let none_spans = crate::document_ops::paragraph_run_char_spans(&none);
        assert_eq!(effective_char_font(Some(&none), &none_spans, 0), FONT_FAMILY);

        let serif = Paragraph {
            runs: vec![Run { text: "ab".into(), font: Some(CURATED_SERIF_FONT.to_string()), ..Run::default() }],
            ..Paragraph::default()
        };
        let serif_spans = crate::document_ops::paragraph_run_char_spans(&serif);
        assert_eq!(effective_char_font(Some(&serif), &serif_spans, 0), CURATED_SERIF_FONT);

        // An uncurated font (e.g. read from a real imported .docx naming
        // "Georgia" or "Calibri") must not be applied — falls back to
        // FONT_FAMILY exactly like `apply_run_style` does.
        let uncurated = Paragraph {
            runs: vec![Run { text: "ab".into(), font: Some("Georgia".to_string()), ..Run::default() }],
            ..Paragraph::default()
        };
        let uncurated_spans = crate::document_ops::paragraph_run_char_spans(&uncurated);
        assert_eq!(effective_char_font(Some(&uncurated), &uncurated_spans, 0), FONT_FAMILY);

        // No paragraph data at all falls back to FONT_FAMILY too.
        assert_eq!(effective_char_font(None, &[], 0), FONT_FAMILY);
    }

    #[test]
    fn test_effective_char_advance_ratio_matches_effective_char_font() {
        let serif = Paragraph {
            runs: vec![Run { text: "ab".into(), font: Some(CURATED_SERIF_FONT.to_string()), ..Run::default() }],
            ..Paragraph::default()
        };
        let spans = crate::document_ops::paragraph_run_char_spans(&serif);
        assert_eq!(effective_char_advance_ratio(Some(&serif), &spans, 0), SERIF_CHAR_ADVANCE_RATIO);
        assert_eq!(effective_char_advance_ratio(None, &[], 0), CHAR_ADVANCE_RATIO);
    }

    /// `column_for_x_in_row` must resolve a serif-font run at the serif
    /// ratio, not the monospace one — using the wrong ratio for a
    /// proportional font's run would put click-to-cursor consistently off
    /// for any document that uses the curated serif font at all.
    #[test]
    fn test_column_in_row_uses_serif_ratio_for_a_serif_run() {
        let para = Paragraph {
            runs: vec![Run { text: "0123456789".into(), font: Some(CURATED_SERIF_FONT.to_string()), ..Run::default() }],
            ..Paragraph::default()
        };
        let spans = crate::document_ops::paragraph_run_char_spans(&para);
        let serif_char = 11.0 * SERIF_CHAR_ADVANCE_RATIO;
        for col in [1usize, 5, 9] {
            assert_eq!(
                column_for_x_in_row(col as f32 * serif_char, Some(&para), &spans, 0, 10, 11.0, 1.0),
                col,
            );
        }
    }

    #[test]
    fn test_line_for_y_top_is_first_line() {
        assert_eq!(line_for_y(0.0, 20.0, 3), 0);
        assert_eq!(line_for_y(19.9, 20.0, 3), 0);
    }

    #[test]
    fn test_line_for_y_advances_per_line_height() {
        assert_eq!(line_for_y(20.0, 20.0, 3), 1);
        assert_eq!(line_for_y(45.0, 20.0, 3), 2);
    }

    #[test]
    fn test_line_for_y_clamps_past_last_line() {
        assert_eq!(line_for_y(1000.0, 20.0, 3), 2);
    }

    #[test]
    fn test_line_for_y_clamps_negative_to_zero() {
        assert_eq!(line_for_y(-5.0, 20.0, 3), 0);
    }

    #[test]
    fn test_line_for_y_no_lines_is_zero() {
        assert_eq!(line_for_y(50.0, 20.0, 0), 0);
    }

    // ── selection_span_for_line ─────────────────────────────────────────────

    #[test]
    fn test_selection_span_before_line_is_none() {
        // Line "world" starts at byte 6; selection (0, 4) ends before it.
        assert_eq!(selection_span_for_line("world", 6, 0, 4), None);
    }

    #[test]
    fn test_selection_span_after_line_is_none() {
        // Line "hello" spans bytes [0, 5); selection (6, 10) starts after it.
        assert_eq!(selection_span_for_line("hello", 0, 6, 10), None);
    }

    #[test]
    fn test_selection_span_touching_line_start_boundary_is_none() {
        // Selection (0, 6) covers "hello" plus its newline, but not "world" itself.
        assert_eq!(selection_span_for_line("world", 6, 0, 6), None);
    }

    #[test]
    fn test_selection_span_touching_line_end_boundary_is_none() {
        assert_eq!(selection_span_for_line("hello", 0, 5, 9), None);
    }

    #[test]
    fn test_selection_span_within_single_line() {
        assert_eq!(selection_span_for_line("hello world", 0, 0, 5), Some((0, 5)));
    }

    #[test]
    fn test_selection_span_first_line_of_multiline_selection() {
        // Selection continues past this line's end; highlight runs to end of line.
        assert_eq!(selection_span_for_line("hello", 0, 2, 20), Some((2, 5)));
    }

    #[test]
    fn test_selection_span_last_line_of_multiline_selection() {
        // Selection started before this line; highlight runs from its start.
        assert_eq!(selection_span_for_line("world", 6, 0, 9), Some((0, 3)));
    }

    #[test]
    fn test_selection_span_middle_line_fully_covered() {
        assert_eq!(selection_span_for_line("middle", 10, 0, 30), Some((0, 6)));
    }

    #[test]
    fn test_selection_span_zero_width_is_none() {
        assert_eq!(selection_span_for_line("hello", 0, 2, 2), None);
    }

    #[test]
    fn test_selection_span_counts_chars_not_bytes() {
        // "café" is 5 bytes but 4 characters ('é' is 2 bytes).
        assert_eq!(selection_span_for_line("café", 0, 0, 5), Some((0, 4)));
    }

    // ── line_segments ────────────────────────────────────────────────────────

    #[test]
    fn test_line_segments_no_cursor_no_selection() {
        assert_eq!(line_segments(5, None, &[], &[]), vec![(0, 5, SegmentStyle::Plain, false)]);
    }

    #[test]
    fn test_line_segments_cursor_mid_line() {
        assert_eq!(
            line_segments(5, Some(2), &[], &[]),
            vec![
                (0, 2, SegmentStyle::Plain, false),
                (2, 3, SegmentStyle::Cursor, false),
                (3, 5, SegmentStyle::Plain, false),
            ]
        );
    }

    #[test]
    fn test_line_segments_cursor_at_line_start() {
        assert_eq!(
            line_segments(5, Some(0), &[], &[]),
            vec![(0, 1, SegmentStyle::Cursor, false), (1, 5, SegmentStyle::Plain, false)]
        );
    }

    #[test]
    fn test_line_segments_cursor_past_end_of_line() {
        assert_eq!(
            line_segments(5, Some(5), &[], &[]),
            vec![(0, 5, SegmentStyle::Plain, false), (5, 5, SegmentStyle::Cursor, false)]
        );
    }

    #[test]
    fn test_line_segments_selection_only() {
        assert_eq!(
            line_segments(6, None, &[(1, 4)], &[]),
            vec![
                (0, 1, SegmentStyle::Plain, false),
                (1, 4, SegmentStyle::Selection, false),
                (4, 6, SegmentStyle::Plain, false),
            ]
        );
    }

    #[test]
    fn test_line_segments_selection_covers_full_line() {
        assert_eq!(
            line_segments(6, None, &[(0, 6)], &[]),
            vec![(0, 6, SegmentStyle::Selection, false)]
        );
    }

    #[test]
    fn test_line_segments_cursor_inside_selection_wins_its_own_cell() {
        assert_eq!(
            line_segments(6, Some(2), &[(2, 5)], &[]),
            vec![
                (0, 2, SegmentStyle::Plain, false),
                (2, 3, SegmentStyle::Cursor, false),
                (3, 5, SegmentStyle::Selection, false),
                (5, 6, SegmentStyle::Plain, false),
            ]
        );
    }

    #[test]
    fn test_line_segments_empty_line_with_cursor() {
        assert_eq!(
            line_segments(0, Some(0), &[], &[]),
            vec![(0, 0, SegmentStyle::Cursor, false)]
        );
    }

    #[test]
    fn test_line_segments_misspelled_range_splits_and_flags() {
        assert_eq!(
            line_segments(9, None, &[], &[(4, 9)]),
            vec![(0, 4, SegmentStyle::Plain, false), (4, 9, SegmentStyle::Plain, true)]
        );
    }

    #[test]
    fn test_line_segments_misspelled_word_keeps_selection_and_cursor_styles() {
        // A selected, cursor-bearing, misspelled word must keep all three:
        // the squiggle is an overlay, not a competing SegmentStyle.
        assert_eq!(
            line_segments(6, Some(1), &[(0, 4)], &[(0, 4)]),
            vec![
                (0, 1, SegmentStyle::Selection, true),
                (1, 2, SegmentStyle::Cursor, true),
                (2, 4, SegmentStyle::Selection, true),
                (4, 6, SegmentStyle::Plain, false),
            ]
        );
    }

    // ── usable_wrap_width ────────────────────────────────────────────────────

    #[test]
    fn test_usable_wrap_width_basic() {
        assert_eq!(usable_wrap_width(100.0), 68.0); // 100 - 2*16
    }

    #[test]
    fn test_usable_wrap_width_unlaid_out_viewport_is_unbounded() {
        // width <= 0 happens before the scroll handle's first layout pass.
        assert_eq!(usable_wrap_width(0.0), f32::MAX);
        assert_eq!(usable_wrap_width(-5.0), f32::MAX);
    }

    // ── wrap_line_into_rows ─────────────────────────────────────────────────────
    // Tests use a uniform 8.0px-per-char width function to exercise the wrap
    // algorithm itself (spacing/hard-break logic), independent of any real
    // font metrics — see the dedicated variable-width test below for the
    // narrow-vs-wide-glyph behaviour this was rewritten to fix.

    #[test]
    fn test_wrap_line_into_rows_empty_line_is_one_row() {
        assert_eq!(wrap_line_into_rows(&[], 80.0, &mut |_, _| 8.0), vec![(0, 0)]);
    }

    #[test]
    fn test_wrap_line_into_rows_fits_in_one_row() {
        let chars: Vec<char> = "hello".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 80.0, &mut |_, _| 8.0), vec![(0, 5)]);
    }

    #[test]
    fn test_wrap_line_into_rows_breaks_on_word_boundary() {
        // "hello world" (11 chars) at 8px/char, budget=64px covers "hello wo"
        // (8 chars); last space within budget is at index 5, so row 1 is
        // [0,5)="hello", the space at 5 is consumed, row 2 starts at 6: "world".
        let chars: Vec<char> = "hello world".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 64.0, &mut |_, _| 8.0), vec![(0, 5), (6, 11)]);
    }

    #[test]
    fn test_wrap_line_into_rows_hard_breaks_long_word() {
        // No spaces at all within budget -> hard break exactly at the pixel
        // budget (32px / 8px-per-char = 4 chars per row).
        let chars: Vec<char> = "abcdefghij".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 32.0, &mut |_, _| 8.0), vec![(0, 4), (4, 8), (8, 10)]);
    }

    #[test]
    fn test_wrap_line_into_rows_exact_multiple_of_width() {
        let chars: Vec<char> = "abcdefgh".chars().collect();
        assert_eq!(wrap_line_into_rows(&chars, 32.0, &mut |_, _| 8.0), vec![(0, 4), (4, 8)]);
    }

    #[test]
    fn test_wrap_line_into_rows_trailing_space_at_break_not_repeated() {
        // Two words separated by exactly one space at the wrap point: the
        // space must not reappear as a leading character on the next row.
        let chars: Vec<char> = "aaaa bbbb".chars().collect();
        let rows = wrap_line_into_rows(&chars, 40.0, &mut |_, _| 8.0);
        for (start, end) in &rows {
            let text: String = chars[*start..*end].iter().collect();
            assert!(!text.starts_with(' '), "row {:?} starts with a space", text);
        }
    }

    #[test]
    fn test_wrap_line_into_rows_forces_progress_when_single_char_exceeds_budget() {
        // A viewport (or a single unusually wide glyph) narrower than one
        // character's width must still advance one character per row rather
        // than looping forever or producing an empty row.
        let chars: Vec<char> = "ab".chars().collect();
        let rows = wrap_line_into_rows(&chars, 10.0, &mut |_, _| 100.0);
        assert_eq!(rows, vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn test_wrap_line_into_rows_narrow_chars_pack_more_per_row_than_a_uniform_estimate_would() {
        // This is the actual bug: a uniform per-character width estimate
        // folds lines of narrow glyphs (like '.' or '-') far earlier than
        // their real on-screen width warrants. With a real per-char width
        // function, 20 narrow (2px) dots should fit 10 to a row within a
        // 20px budget — a uniform 8px/char estimate would have wrapped
        // after only 2.
        let chars: Vec<char> = vec!['.'; 20];
        let mut width_of = |_: usize, c: char| if c == '.' { 2.0 } else { 8.0 };
        let rows = wrap_line_into_rows(&chars, 20.0, &mut width_of);
        assert_eq!(rows[0], (0, 10));
    }

    #[test]
    fn test_wrap_line_into_rows_stateful_width_fn_handles_repeated_non_ascii() {
        // Regression: an earlier version of char_width_fn's per-char cache
        // used `Fn` + `RefCell` to satisfy this function's old `&impl Fn`
        // bound; a cache miss on any non-ASCII character (an `if let ...
        // else { borrow_mut() }` whose immutable borrow's temporary lived
        // across the whole if/else) panicked with "already borrowed" the
        // first time a real .docx with non-ASCII text (smart quotes,
        // accents) was opened. This exercises the exact shape that broke:
        // a stateful width-lookup closure (cache populated on first sight,
        // read back on repeats) driven across a line with repeated
        // non-ASCII characters, through the same `&mut impl FnMut` this
        // function now requires.
        let chars: Vec<char> = "caf\u{e9} caf\u{e9} \u{201c}word\u{201d}".chars().collect();
        let mut cache: std::collections::HashMap<char, f32> = std::collections::HashMap::new();
        let mut width_of = |_: usize, c: char| *cache.entry(c).or_insert(8.0);
        let rows = wrap_line_into_rows(&chars, 200.0, &mut width_of);
        assert_eq!(rows, vec![(0, chars.len())]);
    }

    // ── build_visual_rows / document_lines ──────────────────────────────────

    #[test]
    fn test_document_lines_empty_content_is_one_empty_line() {
        assert_eq!(document_lines(""), vec![String::new()]);
    }

    #[test]
    fn test_document_lines_splits_on_newline() {
        assert_eq!(document_lines("a\nb\nc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn test_build_visual_rows_one_row_per_short_line() {
        let lines = document_lines("hi\nthere");
        let rows = build_visual_rows(&lines, 800.0, &mut |_, _, _| 8.0);
        assert_eq!(rows, vec![(0, 0, 2), (1, 0, 5)]);
    }

    #[test]
    fn test_build_visual_rows_wraps_long_line_into_multiple_rows() {
        let lines = document_lines("hello world");
        let rows = build_visual_rows(&lines, 64.0, &mut |_, _, _| 8.0);
        assert_eq!(rows, vec![(0, 0, 5), (0, 6, 11)]);
    }

    // ── visual_row_for_line_col ──────────────────────────────────────────────

    #[test]
    fn test_visual_row_for_line_col_within_first_row() {
        // Line 0 wraps into rows [(0,5), (6,11)]; col 2 is inside the first.
        let rows = vec![(0, 0, 5), (0, 6, 11)];
        assert_eq!(visual_row_for_line_col(&rows, 0, 2), 0);
    }

    #[test]
    fn test_visual_row_for_line_col_hard_break_boundary_lands_on_next_row_start() {
        // Rows are CONTIGUOUS (row 0 ends at 4, row 1 starts at 4) — a hard
        // mid-word break, no space consumed. col 4 should be carried onto
        // the start of row 1, matching how text editors visually continue
        // the cursor onto the next wrapped row rather than trailing behind.
        let rows = vec![(0, 0, 4), (0, 4, 8)];
        assert_eq!(visual_row_for_line_col(&rows, 0, 4), 1);
    }

    #[test]
    fn test_visual_row_for_line_col_soft_break_boundary_stays_on_current_row() {
        // Row 0 ends at 5, but row 1 starts at 6 (not 5) — a one-character
        // gap for the space `wrap_line_into_rows` consumed at the break.
        // col 5 *is* that consumed space, not a position on row 1, so it
        // must stay on row 0 (trailing the last visible character) rather
        // than being redirected to row 1's row_start (6) — redirecting it
        // was the original bug: row_start(6) > char_col(5) underflowed any
        // `char_col - row_start` a caller computed downstream.
        let rows = vec![(0, 0, 5), (0, 6, 11)];
        assert_eq!(visual_row_for_line_col(&rows, 0, 5), 0);
    }

    #[test]
    fn test_visual_row_for_line_col_true_end_of_line_stays_on_last_row() {
        // col 11 is the true end of the (single) logical line — no next row
        // exists, so it must resolve to the line's last row.
        let rows = vec![(0, 0, 5), (0, 6, 11)];
        assert_eq!(visual_row_for_line_col(&rows, 0, 11), 1);
    }

    #[test]
    fn test_visual_row_for_line_col_second_logical_line() {
        let rows = vec![(0, 0, 5), (1, 0, 3)];
        assert_eq!(visual_row_for_line_col(&rows, 1, 1), 1);
    }

    // ── visual_row_step ──────────────────────────────────────────────────────

    #[test]
    fn test_visual_row_step_up_into_wrapped_continuation_row() {
        // Line 0 wraps into two rows: [0,5) and [6,11) ("hello"/"world").
        // Line 1 is short: [0,3). Standing at the start of line 1 (row 2,
        // col 0) and pressing Up must land on line 0's *second* row (the
        // wrapped continuation), not jump to the very start of line 0.
        // No paragraph data (`&[]`): every character falls back to the same
        // uniform size, so pixel-preserving and index-preserving resolve
        // identically here — this is exercising the row-boundary logic, not
        // the font-size-aware column math (covered separately below).
        let rows = vec![(0, 0, 5), (0, 6, 11), (1, 0, 3)];
        assert_eq!(visual_row_step(&rows, 2, 0, -1, &[], 11.0, 1.0), Some((0, 6)));
    }

    #[test]
    fn test_visual_row_step_down_into_wrapped_continuation_row() {
        let rows = vec![(0, 0, 5), (0, 6, 11), (1, 0, 3)];
        assert_eq!(visual_row_step(&rows, 0, 3, 1, &[], 11.0, 1.0), Some((0, 9)));
    }

    #[test]
    fn test_visual_row_step_preserves_screen_column() {
        let rows = vec![(0, 0, 10), (1, 0, 10)];
        assert_eq!(visual_row_step(&rows, 0, 4, 1, &[], 11.0, 1.0), Some((1, 4)));
    }

    #[test]
    fn test_visual_row_step_clamps_to_shorter_target_row() {
        let rows = vec![(0, 0, 10), (1, 0, 3)];
        assert_eq!(visual_row_step(&rows, 0, 8, 1, &[], 11.0, 1.0), Some((1, 3)));
    }

    #[test]
    fn test_visual_row_step_up_past_first_row_is_none() {
        let rows = vec![(0, 0, 5)];
        assert_eq!(visual_row_step(&rows, 0, 2, -1, &[], 11.0, 1.0), None);
    }

    /// The reported bug: cursor near the end of an 11pt line, pressing `k`
    /// to move up onto a larger-sized (Block-style, 16pt) row landed the
    /// cursor "half way across the screen leftward" — carrying the raw
    /// character index (18) onto the larger row put it at column 18 there
    /// too, but 16pt characters are wider, so column 18 on that row sits
    /// far past where column 18 sat on the narrower 11pt row. The fix must
    /// land at the *pixel-equivalent* column instead: 18 * 6.6px (11pt) /
    /// 9.6px (16pt) rounds to column 12, not 18.
    #[test]
    fn test_visual_row_step_lands_on_pixel_equivalent_column_across_a_font_size_change() {
        let paragraphs = vec![
            Paragraph {
                runs: vec![Run { text: "0123456789abcdefghij".into(), size: 32, ..Run::default() }],
                ..Paragraph::default()
            },
            Paragraph {
                runs: vec![Run { text: "0123456789abcdefghij".into(), ..Run::default() }],
                ..Paragraph::default()
            },
        ];
        let rows = vec![(0, 0, 20), (1, 0, 20)];
        // Moving up (line 1, col 18) -> (line 0, the 16pt row).
        assert_eq!(
            visual_row_step(&rows, 1, 18, -1, &paragraphs, 11.0, 1.0),
            Some((0, 12)),
            "must preserve on-screen X position, not the raw character index"
        );
    }

    #[test]
    fn test_visual_row_step_down_past_last_row_is_none() {
        let rows = vec![(0, 0, 5)];
        assert_eq!(visual_row_step(&rows, 0, 2, 1, &[], 11.0, 1.0), None);
    }

    // ── highlight_color_hex / heading_font_size_px ──────────────────────────

    #[test]
    fn test_highlight_color_hex_known_names() {
        assert_eq!(highlight_color_hex("yellow"), 0xFFD700);
        assert_eq!(highlight_color_hex("green"), 0x00FF00);
        assert_eq!(highlight_color_hex("black"), 0x000000);
        assert_eq!(highlight_color_hex("white"), 0xFFFFFF);
    }

    #[test]
    fn test_highlight_color_hex_unknown_name_falls_back() {
        assert_eq!(highlight_color_hex("nonexistent"), 0x888888);
    }

    /// The 16 names `docx_parser` writes as `w:highlight` must all render as a
    /// real color here — otherwise a document Word accepts would come back
    /// grey. This is what keeps the two lists from drifting apart.
    /// `render_segment` packs the palette's selection color into RGBA as
    /// `(rgb << 8) | 0x80`. Spec 6.4 fixed the selection at #264F78 at ~50%
    /// opacity, so packing that exact value must still reproduce the literal
    /// this refactor replaced — that's what proves the theming didn't quietly
    /// change how a selection looks.
    #[test]
    fn test_selection_alpha_packing_preserves_the_original_color() {
        assert_eq!((0x264F78u32 << 8) | 0x80, 0x264F7880);
        // Every theme's selection keeps its RGB intact and gains the alpha.
        for kind in crate::theme::ThemeKind::all() {
            for mode in crate::theme::ThemeMode::all() {
                let selection = crate::theme::palette(*kind, *mode).selection;
                let packed = (selection << 8) | 0x80;
                assert_eq!(packed >> 8, selection, "{} lost its RGB", kind.label());
                assert_eq!(packed & 0xFF, 0x80, "{} lost its alpha", kind.label());
            }
        }
    }

    #[test]
    fn test_every_word_highlight_name_has_a_color() {
        for name in crate::docx_parser::WORD_HIGHLIGHT_NAMES {
            assert_ne!(
                highlight_color_hex(name),
                0x888888,
                "{name} falls through to the unknown-color fallback",
            );
        }
    }

    #[test]
    fn test_highlight_color_hex_raw_hex_string() {
        assert_eq!(highlight_color_hex("00ff88"), 0x00ff88);
        assert_eq!(highlight_color_hex("0000FF"), 0x0000ff);
    }

    #[test]
    fn test_highlight_color_hex_blue_named() {
        assert_eq!(highlight_color_hex("blue"), 0x0000ff);
    }

    #[test]
    fn test_heading_font_size_body_text_has_no_override() {
        assert_eq!(heading_font_size_px(0, 1.0), None);
    }

    #[test]
    fn test_heading_font_size_levels_1_through_3_each_distinct() {
        assert_eq!(heading_font_size_px(1, 1.0), Some(24.0));
        assert_eq!(heading_font_size_px(2, 1.0), Some(20.0));
        assert_eq!(heading_font_size_px(3, 1.0), Some(18.0));
    }

    #[test]
    fn test_heading_font_size_levels_4_to_6_share_one_size() {
        assert_eq!(heading_font_size_px(4, 1.0), Some(16.0));
        assert_eq!(heading_font_size_px(5, 1.0), Some(16.0));
        assert_eq!(heading_font_size_px(6, 1.0), Some(16.0));
    }

    #[test]
    fn test_heading_font_size_levels_7_to_9_share_one_size() {
        assert_eq!(heading_font_size_px(7, 1.0), Some(14.0));
        assert_eq!(heading_font_size_px(9, 1.0), Some(14.0));
    }

    #[test]
    fn test_heading_font_size_scales_with_zoom() {
        assert_eq!(heading_font_size_px(1, 2.0), Some(48.0));
        assert_eq!(heading_font_size_px(0, 2.0), None);
    }

    // ── relative_luminance / is_light_color / darken_for_light_text ────────

    #[test]
    fn test_relative_luminance_white_is_one() {
        assert!((relative_luminance(0xFFFFFF) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_relative_luminance_black_is_zero() {
        assert!((relative_luminance(0x000000) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_relative_luminance_yellow_is_high() {
        // 0.2126*1 + 0.7152*1 + 0.0722*0 = 0.9278
        assert!((relative_luminance(0xFFFF00) - 0.9278).abs() < 0.001);
    }

    #[test]
    fn test_is_light_color_white_is_light() {
        assert!(is_light_color(0xFFFFFF));
    }

    #[test]
    fn test_is_light_color_black_is_not_light() {
        assert!(!is_light_color(0x000000));
    }

    #[test]
    fn test_is_light_color_yellow_highlight_is_light() {
        assert!(is_light_color(highlight_color_hex("yellow")));
    }

    #[test]
    fn test_is_light_color_dark_blue_highlight_is_not_light() {
        assert!(!is_light_color(highlight_color_hex("darkBlue")));
    }

    #[test]
    fn test_darken_for_light_text_reduces_each_channel() {
        let darkened = darken_for_light_text(0xFFD700); // yellow highlight
        let r = (darkened >> 16) & 0xFF;
        let g = (darkened >> 8) & 0xFF;
        let b = darkened & 0xFF;
        assert!(r < 0xFF);
        assert!(g < 0xD7);
        assert!(b < 0x01 || b == 0);
    }

    #[test]
    fn test_darken_for_light_text_preserves_hue_ratio() {
        // Darkening scales channels uniformly, so a pure-red channel stays
        // proportionally larger than a zero channel.
        let darkened = darken_for_light_text(0xFFFF00);
        let r = (darkened >> 16) & 0xFF;
        let g = (darkened >> 8) & 0xFF;
        let b = darkened & 0xFF;
        assert!(r > 0);
        assert!(g > 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn test_darken_for_light_text_result_is_no_longer_light() {
        assert!(is_light_color(0xFFD700));
        assert!(!is_light_color(darken_for_light_text(0xFFD700)));
    }

    // ── Diagnostic: isolate the per-keystroke cost on a large loaded document ──
    //
    // Reproduces the "editing a loaded .docx is slower than a blank tab"
    // report on a single ~15k-char paragraph (the realistic worst case: a
    // debate card is one giant paragraph). Times three independent things
    // that all run on every keystroke of a real edit, with GPUI's own
    // (expensive, locked, hashed) `layout_width` deliberately excluded from
    // (3) via a synthetic width closure — so this isolates the mutation
    // path (1), the undo-snapshot clone (2), and the wrap algorithm's own
    // cost (3) from the one thing this sandbox can't measure (real font
    // shaping, which needs a live GPUI `App`). Run with
    // `cargo test bench_diagnostic -- --nocapture` to see the printed
    // numbers; not a pass/fail regression test.
    #[test]
    fn bench_diagnostic_large_document_per_keystroke_costs() {
        let big_text: String = "the quick brown fox jumps over the lazy dog "
            .repeat(340); // ~15,300 chars, one giant paragraph
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: big_text.clone(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];

        let mut state = AppState::new();
        state.tabs[0].content = big_text.clone();
        state.tabs[0].paragraphs = paragraphs;
        state.tabs[0].cursor = big_text.len();

        // (1) 100x insert_char: covers push_undo_snapshot + sync_insert_char
        //     (which calls resolve_position) — the whole mutation path.
        let t0 = Instant::now();
        for _ in 0..100 {
            state.insert_char('a');
        }
        let insert_100_elapsed = t0.elapsed();

        // (2) One paragraphs.clone(), the same clone push_undo_snapshot and
        //     TextEditor::render() both pay on every non-coalesced keystroke
        //     and every frame respectively.
        let t1 = Instant::now();
        let _cloned = state.tabs[0].paragraphs.clone();
        let clone_elapsed = t1.elapsed();

        // (3) build_visual_rows over the full document with a synthetic,
        //     branch-free width closure — isolates the wrap algorithm's own
        //     cost from real font-shaping cost (which this headless sandbox
        //     cannot measure without a live GPUI App).
        let lines = document_lines(&state.tabs[0].content);
        let mut synthetic_width_of = |_: usize, _: usize, c: char| if c == ' ' { 4.0 } else { 8.4 };
        let t2 = Instant::now();
        let _rows = build_visual_rows(&lines, usable_wrap_width(800.0), &mut synthetic_width_of);
        let wrap_elapsed = t2.elapsed();

        // How many times a wrap pass over this document calls the real,
        // expensive text_system.layout_width path before vs. after the
        // per-call cache added to char_width_fn: before, once per character
        // occurrence; after, once per *unique* character (the cache's own
        // hit path never reaches layout_width again for a repeat).
        let occurrences = big_text.chars().count();
        let unique: std::collections::HashSet<char> = big_text.chars().collect();

        eprintln!(
            "bench_diagnostic: 100x insert_char = {:?} ({:?}/keystroke), \
             paragraphs.clone() = {:?}, build_visual_rows (synthetic width) = {:?}, \
             expensive layout_width calls per wrap pass: before={} (one per char) \
             after={} (one per unique char) = {:.0}x fewer",
            insert_100_elapsed,
            insert_100_elapsed / 100,
            clone_elapsed,
            wrap_elapsed,
            occurrences,
            unique.len(),
            occurrences as f64 / unique.len() as f64,
        );

        // Sanity bound only (catches an accidental infinite loop / O(n^3)
        // blowup) — not the diagnostic signal itself, which is the printed
        // numbers above.
        assert!(insert_100_elapsed.as_secs() < 5);
    }

    #[test]
    fn bench_diagnostic_row_cache_hit_vs_miss_on_large_heavily_formatted_document() {
        // Unlike the bench above (one giant single-run paragraph), this
        // builds a document shaped like a real heavily-formatted case file:
        // many paragraphs, each with several runs (mixed bold/italic/
        // highlight spans within the same line, the way Bold+Highlight+Cite
        // formatting actually looks) — the shape performance_plan.md flagged
        // as multiplying render cost on top of raw document length, since
        // more runs per line means more span-clipping/element work per row.
        //
        // Measures what `RowCache` (uniform_list_plan.md Part 1, now wired
        // into `render()`) actually buys: a cache MISS pays the full
        // clone-and-rewrap cost `render()` used to pay on *every* frame; a
        // cache HIT is just `Rc::clone`ing the same 5 fields. Deliberately
        // doesn't vary scroll position (top vs. deep in the document) the
        // way the plan originally sketched — under the caching built so far,
        // the whole document is wrapped either way regardless of scroll
        // offset; a scroll-position-dependent cost only appears once
        // uniform_list (Part 2 / step 4) limits work to the visible range.
        let paragraph_count = 500;
        let runs_per_paragraph = 6;
        let mut paragraphs = Vec::with_capacity(paragraph_count);
        for i in 0..paragraph_count {
            let mut runs = Vec::with_capacity(runs_per_paragraph);
            for r in 0..runs_per_paragraph {
                runs.push(Run {
                    text: format!("segment {i}-{r} of a heavily formatted line "),
                    bold: r % 2 == 0,
                    italic: r % 3 == 0,
                    highlight: r % 4 == 0,
                    highlight_color: if r % 4 == 0 { "yellow".to_string() } else { String::new() },
                    size: 24,
                    ..Run::default()
                });
            }
            paragraphs.push(Paragraph { runs, heading: 0, alignment: Alignment::default(), unsupported_xml: None });
        }
        let content: String = paragraphs
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");

        let mut synthetic_width_of = |_: usize, _: usize, c: char| if c == ' ' { 4.0 } else { 8.4 };
        let lines = document_lines(&content);

        // (1) Cache MISS: the full rebuild render() pays without a cached
        // row table — clone paragraphs, collect per-line chars, word-wrap.
        let t0 = Instant::now();
        let _miss_paragraphs = paragraphs.clone();
        let _miss_line_chars: Vec<Vec<char>> = lines.iter().map(|l| l.chars().collect()).collect();
        let _miss_rows = build_visual_rows(&lines, usable_wrap_width(800.0), &mut synthetic_width_of);
        let miss_elapsed = t0.elapsed();

        // (2) Cache HIT: the same data, but already `Rc`-wrapped the way
        // `RowCache` stores it — a hit is just cloning these 5 handles.
        let rc_paragraphs = Rc::new(paragraphs.clone());
        let rc_lines = Rc::new(lines.clone());
        let rc_line_chars: Rc<Vec<Vec<char>>> = Rc::new(lines.iter().map(|l| l.chars().collect()).collect());
        let rc_rows = Rc::new(build_visual_rows(&lines, usable_wrap_width(800.0), &mut synthetic_width_of));
        let t1 = Instant::now();
        let _hit_paragraphs = rc_paragraphs.clone();
        let _hit_lines = rc_lines.clone();
        let _hit_line_chars = rc_line_chars.clone();
        let _hit_rows = rc_rows.clone();
        let hit_elapsed = t1.elapsed();

        eprintln!(
            "bench_diagnostic_row_cache: {} paragraphs x {} runs each ({} bytes) — \
             cache MISS (full rebuild) = {:?}, cache HIT (Rc clones) = {:?}, {:.0}x faster on hit",
            paragraph_count,
            runs_per_paragraph,
            content.len(),
            miss_elapsed,
            hit_elapsed,
            miss_elapsed.as_nanos() as f64 / (hit_elapsed.as_nanos().max(1) as f64),
        );

        // The actual diagnostic signal is the printed numbers above; this is
        // just a sanity bound confirming the cache is doing its job at all,
        // not a specific performance target.
        assert!(hit_elapsed < miss_elapsed, "a cache hit must be cheaper than a full rebuild");
    }

    // ── row_cache_is_valid (uniform_list_plan.md Part 1) ─────────────────────

    fn test_row_cache(tab_id: usize, content_version: u64, viewport_width: f32, zoom: f32) -> RowCache {
        RowCache {
            invisibility: false,
            fold_version: 0,
            tab_id,
            content_version,
            viewport_width_bits: viewport_width.to_bits(),
            zoom_bits: zoom.to_bits(),
            lines: Rc::new(Vec::new()),
            line_chars: Rc::new(Vec::new()),
            line_byte_starts: Rc::new(Vec::new()),
            rows: Rc::new(Vec::new()),
            paragraphs: Rc::new(Vec::new()),
            display_to_wrap: Rc::new(Vec::new()),
            wrap_to_display: Rc::new(Vec::new()),
        }
    }

    // ── invisibility mode ────────────────────────────────────────────────────

    /// (invisibility, heading, highlighted, bold, size, cite_size)
    #[test]
    fn test_invisibility_keeps_highlights_and_every_card_style() {
        const CITE: u16 = 26; // settings.conf cite_size=13pt

        // Off: nothing is ever hidden.
        assert!(!run_is_hidden(false, 0, false, false, 0, CITE));

        // Plain body text hides.
        assert!(run_is_hidden(true, 0, false, false, 0, CITE));
        // Highlighted text stays.
        assert!(!run_is_hidden(true, 0, true, false, 0, CITE));

        // Pocket/Hat/Block/Tag are heading levels 1..4 — all stay, whole line.
        for heading in 1..=4 {
            assert!(!run_is_hidden(true, heading, false, false, 0, CITE), "heading {heading} hidden");
        }

        // Cite: bold at the configured cite size.
        assert!(!run_is_hidden(true, 0, false, true, CITE, CITE));
        // Emphasis is bold at body size (0 = inherit) and is *not* a cite.
        assert!(run_is_hidden(true, 0, false, true, 0, CITE));
        // Bold at some other explicit size is not a cite either.
        assert!(run_is_hidden(true, 0, false, true, 52, CITE));
        // Cite size without bold is not a cite.
        assert!(run_is_hidden(true, 0, false, false, CITE, CITE));
    }

    fn run_plain(text: &str) -> Run {
        Run { text: text.into(), ..Run::default() }
    }

    fn para_plain(text: &str) -> Paragraph {
        Paragraph {
            runs: vec![run_plain(text)],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }
    }

    fn hl_run(text: &str) -> Run {
        Run { text: text.into(), highlight: true, highlight_color: "yellow".into(), ..Run::default() }
    }

    fn no_folds(paragraphs: &[Paragraph]) -> Vec<bool> {
        vec![false; paragraphs.len()]
    }

    /// What "collapse every heading" produces: each body paragraph hidden.
    fn all_body_folded(paragraphs: &[Paragraph]) -> Vec<bool> {
        paragraphs.iter().map(|p| p.heading == 0).collect()
    }

    fn card_para(text: &str, heading: u8) -> Paragraph {
        Paragraph {
            runs: vec![run_plain(text)],
            heading,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }
    }

    /// Fold leaves every card-style line and hides every body paragraph — the
    /// document's outline, Word's collapse-under-headings.
    #[test]
    fn test_fold_hides_body_and_keeps_every_heading_level() {
        let paragraphs = vec![
            card_para("pocket", 1),
            para_plain("body under pocket"),
            card_para("hat", 2),
            para_plain("body under hat"),
            card_para("block", 3),
            card_para("tag", 4),
            para_plain("body under tag"),
        ];
        let rows: Vec<(usize, usize, usize)> =
            (0..paragraphs.len()).map(|i| (i, 0usize, 1usize)).collect();

        let hidden = hidden_wrap_rows(&rows, &paragraphs, false, 26, &all_body_folded(&paragraphs));
        assert_eq!(hidden, vec![false, true, false, true, false, false, true]);

        // Off, nothing folds.
        assert_eq!(hidden_wrap_rows(&rows, &paragraphs, false, 26, &no_folds(&paragraphs)), vec![false; 7]);
    }

    /// Fold is the coarser rule: a folded body row goes even if it holds
    /// highlighted text that invisibility mode would have kept.
    #[test]
    fn test_fold_hides_body_rows_that_invisibility_would_keep() {
        let paragraphs = vec![Paragraph {
            runs: vec![hl_run("highlighted body")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let rows = vec![(0usize, 0usize, 16usize)];

        // Invisibility alone keeps it (it is highlighted)...
        assert_eq!(hidden_wrap_rows(&rows, &paragraphs, true, 26, &no_folds(&paragraphs)), vec![false]);
        // ...but folding hides it regardless.
        assert_eq!(hidden_wrap_rows(&rows, &paragraphs, true, 26, &all_body_folded(&paragraphs)), vec![true]);
    }

    #[test]
    fn test_hidden_wrap_rows_marks_only_fully_hidden_rows() {
        let paragraphs = vec![
            // 0: body text with a highlight in it — stays.
            Paragraph {
                runs: vec![run_plain("plain "), hl_run("read this")],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
            // 1: body text with nothing marked — goes.
            para_plain("unread body text"),
            // 2: a Tag line — stays whole.
            Paragraph {
                runs: vec![run_plain("a tag")],
                heading: 4,
                alignment: Alignment::default(),
                unsupported_xml: None,
            },
        ];
        let rows = vec![(0usize, 0usize, 15usize), (1, 0, 16), (2, 0, 5)];

        let hidden = hidden_wrap_rows(&rows, &paragraphs, true, 26, &no_folds(&paragraphs));
        assert_eq!(hidden, vec![false, true, false]);

        // Off, nothing hides.
        assert_eq!(hidden_wrap_rows(&rows, &paragraphs, false, 26, &no_folds(&paragraphs)), vec![false; 3]);
    }

    /// Only the runs actually on a row decide it: a highlight later in a
    /// wrapped paragraph must not keep an earlier all-plain row visible.
    #[test]
    fn test_hidden_wrap_rows_judges_each_wrapped_row_separately() {
        let paragraphs = vec![Paragraph {
            runs: vec![run_plain("aaaaa"), hl_run("bbbbb")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        // Row 0 covers only the plain run, row 1 only the highlighted one.
        let rows = vec![(0usize, 0usize, 5usize), (0, 5, 10)];
        assert_eq!(hidden_wrap_rows(&rows, &paragraphs, true, 26, &no_folds(&paragraphs)), vec![true, false]);
    }

    #[test]
    fn test_hidden_rows_get_no_display_slot() {
        let paragraphs = vec![para_plain("a"), para_plain("b"), para_plain("c")];
        let rows = vec![(0usize, 0usize, 1usize), (1, 0, 1), (2, 0, 1)];

        let (display_to_wrap, wrap_to_display) =
            expand_rows_for_display(&rows, &paragraphs, 1.0, &[false, true, false], 14.0);

        // The middle row is gone from the paint list entirely — that is the
        // vertical gap closing.
        assert_eq!(display_to_wrap, vec![Some(0), Some(2)]);
        // ...and the hidden row points at where the next visible one landed,
        // so scroll-to-cursor still resolves.
        assert_eq!(wrap_to_display, vec![0, 1, 1]);
    }

    // ── read mode paging ─────────────────────────────────────────────────────

    /// The two guarantees together: a page advances by whole rows only, so
    /// nothing is skipped, and by a *full* screenful of them, so nothing
    /// fully-read repeats.
    #[test]
    fn test_page_scroll_advances_by_whole_rows() {
        // 100px viewport, 24px rows -> 4 whole rows fit (96px), not 100.
        assert_eq!(page_scroll_offset(0.0, 100.0, 24.0, 1000.0, true), Some(-96.0));
        // ...and back up by the same amount.
        assert_eq!(page_scroll_offset(-96.0, 100.0, 24.0, 1000.0, false), Some(0.0));
    }

    #[test]
    fn test_page_scroll_stops_at_the_document_ends() {
        // Already at the top: nothing above to page to.
        assert_eq!(page_scroll_offset(0.0, 100.0, 24.0, 1000.0, false), None);
        // Already at the bottom.
        assert_eq!(page_scroll_offset(-1000.0, 100.0, 24.0, 1000.0, true), None);
        // A partial page remaining still moves, clamped to the end rather
        // than overshooting into blank space.
        assert_eq!(page_scroll_offset(-950.0, 100.0, 24.0, 1000.0, true), Some(-1000.0));
    }

    /// A viewport shorter than one row must still advance, or the keys lock up.
    #[test]
    fn test_page_scroll_advances_at_least_one_row() {
        assert_eq!(page_scroll_offset(0.0, 10.0, 24.0, 1000.0, true), Some(-24.0));
    }

    /// Rows scale with zoom, so a page must too — otherwise zoomed-in text
    /// would page by more lines than are on screen and skip content.
    #[test]
    fn test_page_scroll_follows_zoom() {
        let zoomed_row = 24.0 * 2.0;
        assert_eq!(page_scroll_offset(0.0, 100.0, zoomed_row, 1000.0, true), Some(-96.0));
    }

    // ── spell cache ──────────────────────────────────────────────────────────

    #[test]
    fn test_spell_cache_returns_same_allocation_on_hit() {
        let cache = Rc::new(RefCell::new(SpellCache::default()));
        let dict = HashSet::new();
        let first = spell_ranges_cached(&cache, "hello wrold", &dict);
        let second = spell_ranges_cached(&cache, "hello wrold", &dict);
        assert_eq!(*first, vec![(6, 11)]);
        // Same `Rc`, i.e. the second call didn't re-run the checker.
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(cache.borrow().entries.len(), 1);
    }

    #[test]
    fn test_spell_cache_keyed_on_line_text_not_position() {
        let cache = Rc::new(RefCell::new(SpellCache::default()));
        let dict = HashSet::new();
        spell_ranges_cached(&cache, "hello wrold", &dict);
        spell_ranges_cached(&cache, "a different line", &dict);
        // Editing one line must not evict the other — that's the whole reason
        // this is keyed on text rather than on `content_version`.
        assert_eq!(cache.borrow().entries.len(), 2);
        assert!(cache.borrow().entries.contains_key("hello wrold"));
    }

    /// The staleness path: adding a word to the user dictionary has to drop
    /// cached entries, or already-checked lines keep squiggling a word the
    /// user just accepted. The cache key is the line's text, which a
    /// dictionary edit doesn't change, so nothing else would catch this.
    #[test]
    fn test_spell_cache_invalidated_by_user_dictionary_growth() {
        let cache = Rc::new(RefCell::new(SpellCache::default()));
        let mut dict = HashSet::new();

        let before = spell_ranges_cached(&cache, "hello wrold", &dict);
        assert_eq!(*before, vec![(6, 11)]);

        dict.insert("wrold".to_string());
        let after = spell_ranges_cached(&cache, "hello wrold", &dict);
        assert!(after.is_empty(), "squiggle should clear after Add to Dictionary");
    }

    #[test]
    fn test_row_cache_is_valid_when_everything_matches() {
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(row_cache_is_valid_for(&cache, 1, 5, 800.0, 1.0, false, false, 0));
    }

    #[test]
    fn test_row_cache_is_valid_false_when_tab_id_differs() {
        // A different tab could coincidentally share the same
        // content_version/width/zoom — tab_id must be checked, or a tab
        // switch could serve another tab's stale wrapped rows.
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(!row_cache_is_valid_for(&cache, 2, 5, 800.0, 1.0, false, false, 0));
    }

    #[test]
    fn test_row_cache_is_valid_false_when_content_version_differs() {
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(!row_cache_is_valid_for(&cache, 1, 6, 800.0, 1.0, false, false, 0));
    }

    /// The divider-drag freeze fix: a width change normally invalidates, but
    /// while dragging the stale tables are reused rather than paying a
    /// full-document re-wrap per pane per mouse-move.
    #[test]
    fn test_row_cache_survives_a_width_change_while_the_divider_is_dragging() {
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(!row_cache_is_valid_for(&cache, 1, 5, 640.0, 1.0, false, false, 0));
        assert!(row_cache_is_valid_for(&cache, 1, 5, 640.0, 1.0, true, false, 0));
    }

    /// Dragging must not make the cache accept a *different document* or a
    /// stale edit — only a different width.
    #[test]
    fn test_dragging_still_invalidates_on_content_or_tab_change() {
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(!row_cache_is_valid_for(&cache, 2, 5, 640.0, 1.0, true, false, 0), "wrong tab accepted");
        assert!(!row_cache_is_valid_for(&cache, 1, 6, 640.0, 1.0, true, false, 0), "stale content accepted");
        assert!(!row_cache_is_valid_for(&cache, 1, 5, 640.0, 1.25, true, false, 0), "stale zoom accepted");
    }

    #[test]
    fn test_row_cache_is_valid_false_when_viewport_width_differs() {
        // A window resize must invalidate the cache — the old wrap width no
        // longer matches where lines should actually break.
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(!row_cache_is_valid_for(&cache, 1, 5, 801.0, 1.0, false, false, 0));
    }

    #[test]
    fn test_row_cache_is_valid_false_when_zoom_differs() {
        let cache = test_row_cache(1, 5, 800.0, 1.0);
        assert!(!row_cache_is_valid_for(&cache, 1, 5, 800.0, 1.25, false, false, 0));
    }

    // ── slot_count_for_paragraph / expand_rows_for_display ────────────────────
    // (card-style row-overlap fix — handoff.md)

    fn plain_paragraph() -> Paragraph {
        Paragraph { runs: vec![Run::default()], heading: 0, alignment: Alignment::default(), unsupported_xml: None }
    }

    fn pocket_paragraph() -> Paragraph {
        // Mirrors AppState::apply_card_style(CardStyleKind::Pocket): bold +
        // FontSize(52 half-points = 26px) + Box(true), heading level 1.
        Paragraph {
            runs: vec![Run { size: 52, bold: true, box_format: true, ..Run::default() }],
            heading: 1,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }
    }

    /// The actual bug fix: settings.conf's real 11pt default
    /// (`normal_text_size_half_points: 22` — `AppState::new`'s own default)
    /// must produce a *shorter* row than the old fixed 20px, not the same
    /// one — 20px was calibrated for a stale 14px reference that predates
    /// the configurable default-size setting, which is why 11pt body text
    /// read as "far too much space between lines."
    #[test]
    fn test_line_height_tracks_normal_size_not_the_stale_14px_reference() {
        let default_normal_size_px = 22.0 / 2.0; // AppState::new's normal_text_size_half_points
        assert!(line_height_px(default_normal_size_px) < LINE_HEIGHT_PX);
        // And it keeps scaling in both directions with the configured size,
        // rather than flooring at the old hardcoded reference.
        assert!(line_height_px(9.0) < line_height_px(default_normal_size_px));
        assert!(line_height_px(default_normal_size_px) < line_height_px(18.0));
    }

    // `normal_size_px: 14.0` in these tests matches the pre-fix hardcoded
    // `FONT_SIZE_PX` reference these numeric expectations were tuned
    // against — see `line_height_px`/`LINE_HEIGHT_RATIO`.
    #[test]
    fn test_slot_count_plain_paragraph_is_one_slot() {
        assert_eq!(slot_count_for_paragraph(Some(&plain_paragraph()), 1.0, 14.0), 1);
    }

    #[test]
    fn test_slot_count_no_paragraph_data_is_one_slot() {
        // A brand-new tab has no parsed paragraphs yet — must not panic or
        // under/over-count when formatting data is simply absent.
        assert_eq!(slot_count_for_paragraph(None, 1.0, 14.0), 1);
    }

    #[test]
    fn test_slot_count_pocket_needs_multiple_slots() {
        // 26px font (~1.86x LINE_HEIGHT_PX/FONT_SIZE_PX ratio) plus the box's
        // padding/border comfortably needs more than one 20px slot.
        let slots = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 14.0);
        assert!(slots > 1, "expected Pocket line to need multiple slots, got {slots}");
    }

    #[test]
    fn test_slot_count_scales_with_zoom() {
        // CARD_BOX_EXTRA_PX doesn't scale with zoom, so at very low zoom it
        // dominates and needs relatively more slots than at 1x.
        let at_1x = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 14.0);
        let at_half = slot_count_for_paragraph(Some(&pocket_paragraph()), 0.5, 14.0);
        assert!(at_half >= at_1x);
    }

    #[test]
    fn test_slot_count_tag_needs_only_one_slot() {
        // Mirrors AppState::apply_card_style(CardStyleKind::Tag): bold +
        // FontSize(26 half-points = 13px), heading level 4. 13px is smaller
        // than body text (14px), so despite heading level 4's generic 16px
        // fallback, the actual rendered line fits comfortably in one slot —
        // it must not reserve a spurious blank row underneath.
        let para = Paragraph {
            runs: vec![Run { size: 26, bold: true, ..Run::default() }],
            heading: 4,
            alignment: Alignment::default(),
            unsupported_xml: None,
        };
        assert_eq!(slot_count_for_paragraph(Some(&para), 1.0, 14.0), 1);
    }

    #[test]
    fn test_slot_count_heading_without_box_still_oversized() {
        // heading_font_size_px(1, 1.0) == 24px, no box — still needs 2 slots
        // (24 * 20/14 == 34.3px > 20px, <= 40px).
        let para = Paragraph { runs: vec![Run::default()], heading: 1, alignment: Alignment::default(), unsupported_xml: None };
        assert_eq!(slot_count_for_paragraph(Some(&para), 1.0, 14.0), 2);
    }

    #[test]
    fn test_expand_rows_for_display_plain_rows_are_untouched() {
        let rows = vec![(0, 0, 5), (1, 0, 5)];
        let paragraphs = vec![plain_paragraph(), plain_paragraph()];
        let (display_to_wrap, wrap_to_display) = expand_rows_for_display(&rows, &paragraphs, 1.0, &vec![false; rows.len()], 14.0);
        assert_eq!(display_to_wrap, vec![Some(0), Some(1)]);
        assert_eq!(wrap_to_display, vec![0, 1]);
    }

    #[test]
    fn test_expand_rows_for_display_inserts_spacers_before_oversized_row() {
        let rows = vec![(0, 0, 5), (1, 0, 5)];
        let paragraphs = vec![pocket_paragraph(), plain_paragraph()];
        let slots = slot_count_for_paragraph(Some(&pocket_paragraph()), 1.0, 14.0);
        let (display_to_wrap, wrap_to_display) = expand_rows_for_display(&rows, &paragraphs, 1.0, &vec![false; rows.len()], 14.0);

        // Row 0 (Pocket) occupies `slots` display rows: blanks first, so the
        // box's real overflow direction (upward, out of a bottom-aligned
        // row) has somewhere empty to land, then the content itself.
        let mut expected = std::iter::repeat(None).take(slots - 1).collect::<Vec<_>>();
        expected.push(Some(0));
        expected.push(Some(1));
        assert_eq!(display_to_wrap, expected);

        // Row 0's content now sits at display index `slots - 1`, after its
        // own leading blanks; row 1 immediately follows at `slots`.
        assert_eq!(wrap_to_display, vec![slots - 1, slots]);
    }
}
