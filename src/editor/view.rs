use gpui::prelude::*;
use gpui::*;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::auto_scroll::AutoScroller;
use crate::document_ops::paragraph_run_char_spans;
use crate::docx_parser::{Paragraph, Run};
use crate::editor::geometry::page_scroll_offset;
use crate::editor::layout::{
    build_visual_rows, column_for_x_in_row, display_line, document_lines, expand_rows_for_display,
    hidden_wrap_rows, line_for_y, line_height_px, list_item_ordinal, list_marker_text_for_level,
    nearest_wrap_row_for_display_row, paints_run_box, row_cache_is_valid_for, row_edge_target_col,
    row_slot_px, selection_span_for_line, sub_cursor_for_run, text_line_box_px,
    visual_row_for_line_col, x_for_col_in_row, RowCache, RowEdge,
};
#[cfg(test)]
use crate::editor::layout::{list_marker_text, slot_count_for_paragraph, to_letter, to_roman};
#[cfg(test)]
use crate::editor::layout::{CARD_BOX_EXTRA_PX, EMPHASIS_BOX_EXTRA_PX, ROW_SUBDIVISIONS};
pub(crate) use crate::editor::style::{
    all_curated_font_names, effective_char_font, effective_char_size_px, imported_font_names,
    is_curated_font, register_imported_font, run_is_hidden, unregister_imported_font,
    CURATED_SERIF_FONT, FONT_FAMILY,
};
use crate::editor::style::{heading_font_size_px, line_font_px, line_segments, SegmentStyle};
#[cfg(test)]
use crate::editor::style::{CHAR_ADVANCE_RATIO, SERIF_CHAR_ADVANCE_RATIO};
use crate::keybinds::{CopyAction, CutAction, PasteAction};
use crate::state::{AppState, EditorContextMenu, Pane, SpellTarget, VimMode};
use crate::theme::{Palette, ThemeMode};

#[path = "input_adapter.rs"]
mod input_adapter;

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
#[cfg(test)]
const LINE_HEIGHT_RATIO: f32 = LINE_HEIGHT_PX / FONT_SIZE_PX;

/// Matches the outer editor div's padding.
const CONTENT_PADDING_PX: f32 = 16.0;
/// Number of lines of buffer to keep visible above/below the cursor —
/// mirrors Vim's `scrolloff`. `scroll_to_cursor` starts scrolling once the
/// cursor comes within this many lines of the viewport edge, rather than
/// waiting until the cursor line itself is already clipped. Raised from 3
/// to 6 (found_bugs.md: auto-scroll at the bottom of the page only kicked
/// in once the cursor was already 3-4 lines from the edge — the old value
/// of this same constant — and needed to start earlier).
const SCROLL_MARGIN_LINES: f32 = 6.0;
/// Width of the document scrollbar's track, and the inset of the thumb inside
/// it. Beta feedback asked for "the little thingy you click and drag to go up
/// and down the doc".
const SCROLLBAR_WIDTH_PX: f32 = 10.0;
const SCROLLBAR_THUMB_INSET_PX: f32 = 2.0;
/// Shortest the thumb is allowed to get. Without a floor, a long document
/// shrinks it to a few unclickable pixels.
const SCROLLBAR_MIN_THUMB_PX: f32 = 24.0;
/// Horizontal space `usable_wrap_width` keeps clear on the right for the
/// scrollbar, so text never wraps underneath it (bug report: "it covers
/// text"). Wider than the track itself to leave a visible gap between the
/// last character and the bar. Because wrap width also drives click
/// hit-testing, reserving it in one place keeps both in agreement.
const SCROLLBAR_GUTTER_PX: f32 = SCROLLBAR_WIDTH_PX + 6.0;
/// How long the bar stays fully opaque after the last scroll, and how long it
/// then takes to fade out. Set `SCROLLBAR_IDLE_OPACITY` above 0.0 to leave it
/// faintly visible at rest instead of hiding completely.
const SCROLLBAR_HOLD_MS: u64 = 900;
const SCROLLBAR_FADE_MS: u64 = 400;
const SCROLLBAR_IDLE_OPACITY: f32 = 0.0;

/// Opacity of the scrollbar `delta` of the way through its hold-then-fade
/// animation. Flat at full opacity for the hold, then eased down.
///
/// Split out so the curve is testable — `compute` needs a live GPUI frame.
pub(crate) fn scrollbar_fade_opacity(delta: f32) -> f32 {
    let total = (SCROLLBAR_HOLD_MS + SCROLLBAR_FADE_MS) as f32;
    let hold = SCROLLBAR_HOLD_MS as f32 / total;
    if delta <= hold {
        return 1.0;
    }
    let t = ((delta - hold) / (1.0 - hold)).clamp(0.0, 1.0);
    1.0 + t * (SCROLLBAR_IDLE_OPACITY - 1.0)
}

/// Geometry captured when a scrollbar drag begins, carried as the drag's
/// payload so the move handler needs no live layout of its own.
///
/// The document cannot reflow mid-drag (no typing, no resize), so these stay
/// valid for the life of the gesture.
#[derive(Clone)]
pub struct ScrollbarDragPayload {
    /// Window-space Y of the top of the track.
    track_top: f32,
    /// Distance the thumb can travel: track height minus thumb height.
    travel: f32,
    /// Scrollable distance in content pixels: content height minus viewport.
    max_scroll: f32,
}

impl Render for ScrollbarDragPayload {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // GPUI wants a drag preview; a scrollbar drag has none — the thumb
        // itself is the feedback. Same empty-view trick `SidebarResizePayload`
        // uses in `file_explorer.rs`.
        div()
    }
}

/// The document's scrollbar, drawn as a `uniform_list` decoration.
///
/// Built on `gpui::UniformListDecoration` rather than Zed's `ui::Scrollbar`.
/// That component is not reachable here: `ui` is not a dependency (this crate
/// takes only `gpui`/`gpui_platform`), and it resolves its colors through
/// `theme::ActiveTheme`, which reads a `GlobalTheme` global this app never
/// installs — it has its own `Palette`. The decoration trait is the same hook
/// Zed's own scrollbar hangs off, and it lives in `gpui`, so this gets the
/// same integration with none of that dependency weight.
///
/// `compute` runs during the list's prepaint and is handed the geometry that
/// would otherwise have to be recovered by hand: the viewport `bounds`, the
/// measured `item_height`, and the total `item_count`.
struct ScrollbarDecoration {
    /// The list's own scroll handle — the same one `track_scroll` was given,
    /// so dragging and every other scroll path move the identical offset.
    scroll_handle: ScrollHandle,
    /// Where inside the thumb the pointer grabbed it, so the thumb doesn't
    /// jump under the cursor on the first move. Written on mouse-down, read
    /// by the drag-move handler in `render`; shared because the decoration is
    /// rebuilt from scratch every frame and cannot hold state itself.
    grab_offset: Rc<Cell<f32>>,
    /// Set the moment the pointer goes down on the bar, cleared on release.
    /// `cx.has_active_drag()` only becomes true once a drag has actually
    /// begun, which leaves the first move after mouse-down unguarded; this
    /// closes that window so not even a single character flashes selected.
    pressed: Rc<Cell<bool>>,
    /// Bumped by `render` whenever the scroll offset changes. It is only used
    /// as part of the fade animation's element id: a new id restarts the
    /// animation, which is how scrolling brings a faded-out bar back.
    activity: usize,
    track: u32,
    thumb: u32,
    thumb_hover: u32,
}

impl UniformListDecoration for ScrollbarDecoration {
    fn compute(
        &self,
        _visible_range: std::ops::Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        /*
         * `item_count` is the *display-row* count (`display_to_wrap.len()`),
         * not the number of logical lines — every line is ROW_SUBDIVISIONS
         * rows, and most of them are blank spacers. Sizing the thumb off
         * anything else would make it disagree with the real scroll extent.
         * Using the measured `item_height` GPUI passes in (rather than
         * recomputing `row_slot_px`) keeps it exact on hardware that snaps
         * rows to device pixels — the same reason `real_row_height_px`
         * exists.
         */
        let viewport_h = bounds.size.height.as_f32();
        let content_h = item_height.as_f32() * item_count as f32;
        let max_scroll = content_h - viewport_h;
        if max_scroll <= 0.0 || viewport_h <= 0.0 {
            return div().into_any_element(); // nothing to scroll
        }

        // `scroll_offset.y` grows more negative the further down the document
        // is scrolled, which is why this negates before taking a fraction.
        let scrolled = (-scroll_offset.y.as_f32()).clamp(0.0, max_scroll);
        let crate::editor::geometry::ScrollbarGeometry {
            thumb_h,
            travel,
            thumb_top,
        } = crate::editor::geometry::scrollbar_geometry(
            viewport_h,
            content_h,
            scrolled,
            SCROLLBAR_MIN_THUMB_PX,
        );

        // The decoration is prepainted at `padded_bounds.origin + scroll_offset`
        // (gpui's `uniform_list`), i.e. in *scrolled content* space, so
        // subtracting `scroll_offset` again is what pins the bar to the
        // viewport. That compensation has to be applied to a child of this
        // element, not to this element itself: the root is positioned by
        // `prepaint_at(bounds.origin)` and laid out via `layout_as_root`,
        // where its own `absolute` inset has no containing block to resolve
        // against and is ignored — which left the bar sitting at a fixed spot
        // in the document and scrolling away with the text (bug report: "it
        // statically renders on one part of the page, if you scroll down far
        // enough it will go away"). A plain relative root gives the track a
        // containing block, and the inset resolves normally.
        let pin_x = -scroll_offset.x.as_f32();
        let pin_y = -scroll_offset.y.as_f32();
        // Hug the editor's true right edge. `bounds` is the list's *padded*
        // box, and the content mask is its *outer* bounds, so painting back
        // out across the padding is visible rather than clipped.
        let track_left =
            bounds.size.width.as_f32() + CONTENT_PADDING_PX - SCROLLBAR_WIDTH_PX + pin_x;
        // ...and in window space, which is what the pointer is compared against.
        let track_top_window = bounds.origin.y.as_f32() + pin_y;

        let payload = ScrollbarDragPayload {
            track_top: track_top_window,
            travel,
            max_scroll,
        };
        let grab = self.grab_offset.clone();
        let thumb_top_window = track_top_window + thumb_top;
        let track_handle = self.scroll_handle.clone();
        let (track_pressed, thumb_pressed) = (self.pressed.clone(), self.pressed.clone());
        let (thumb_color, thumb_hover) = (self.thumb, self.thumb_hover);

        let track = div()
            .id("editor-scrollbar-track")
            .absolute()
            .left(px(track_left))
            .top(px(pin_y))
            .w(px(SCROLLBAR_WIDTH_PX))
            .h(px(viewport_h))
            .bg(rgb(self.track))
            // Pointing at the bar cancels the fade for as long as the pointer
            // stays there. Opacity is a paint property in gpui, so a
            // fully-faded bar still hit-tests and can be picked back up.
            .hover(|st| st.opacity(1.0))
            // Clicking the track jumps there, centring the thumb on the
            // click. The thumb's own handler stops propagation, so grabbing
            // the thumb never also jumps.
            .on_mouse_down(
                MouseButton::Left,
                move |ev: &MouseDownEvent, _window, cx| {
                    // Same reason the thumb stops propagation: without this the
                    // click also reaches the editor underneath and moves the text
                    // cursor to wherever the pointer happened to be.
                    cx.stop_propagation();
                    track_pressed.set(true);
                    if travel <= 0.0 {
                        return;
                    }
                    let want_top = ev.position.y.as_f32() - track_top_window - thumb_h / 2.0;
                    let fraction = (want_top / travel).clamp(0.0, 1.0);
                    let offset = track_handle.offset();
                    track_handle.set_offset(point(offset.x, px(-(fraction * max_scroll))));
                    cx.refresh_windows();
                },
            )
            .child(
                div()
                    .id("editor-scrollbar-thumb")
                    .absolute()
                    .top(px(thumb_top))
                    .left(px(SCROLLBAR_THUMB_INSET_PX))
                    .w(px(SCROLLBAR_WIDTH_PX - 2.0 * SCROLLBAR_THUMB_INSET_PX))
                    .h(px(thumb_h))
                    .rounded(px(
                        (SCROLLBAR_WIDTH_PX - 2.0 * SCROLLBAR_THUMB_INSET_PX) / 2.0
                    ))
                    .bg(rgb(thumb_color))
                    .cursor_pointer()
                    .hover(move |st| st.bg(rgb(thumb_hover)))
                    // Records where in the thumb the grab landed. Mouse-down
                    // always precedes the first drag-move, so the offset is
                    // set before anything reads it.
                    .on_mouse_down(
                        MouseButton::Left,
                        move |ev: &MouseDownEvent, _window, cx| {
                            // Grabbing the thumb must not also register as a
                            // track click, which would teleport it to the cursor
                            // before the drag even starts.
                            cx.stop_propagation();
                            thumb_pressed.set(true);
                            grab.set(ev.position.y.as_f32() - thumb_top_window);
                        },
                    )
                    .on_drag(payload, |p: &ScrollbarDragPayload, _offset, _window, cx| {
                        cx.new(|_| p.clone())
                    }),
            )
            // Hold, then fade. The id carries `activity`, which `render`
            // bumps on every scroll — a new id restarts the animation, so
            // scrolling brings the bar back and re-arms the fade.
            .with_animation(
                ElementId::named_usize("editor-scrollbar-fade", self.activity),
                Animation::new(std::time::Duration::from_millis(
                    SCROLLBAR_HOLD_MS + SCROLLBAR_FADE_MS,
                )),
                |el, delta| el.opacity(scrollbar_fade_opacity(delta)),
            );

        // A plain relative root, sized to the viewport, purely so the track's
        // absolute inset above has something to resolve against.
        div()
            .w(px(bounds.size.width.as_f32()))
            .h(px(viewport_h))
            .child(track)
            .into_any_element()
    }
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
    /// Where inside the scrollbar thumb the pointer grabbed it. Lives here,
    /// not on the decoration, because the decoration is rebuilt every frame —
    /// see `ScrollbarDecoration::grab_offset`.
    scrollbar_grab: Rc<Cell<f32>>,
    /// Whether the pointer is currently down on the scrollbar — see
    /// `ScrollbarDecoration::pressed`.
    scrollbar_pressed: Rc<Cell<bool>>,
    /// Counts scroll movements, to re-arm the scrollbar's fade — see
    /// `ScrollbarDecoration::activity`.
    scrollbar_activity: usize,
    /// Scroll offset as of the last render, for spotting that movement.
    last_scrollbar_offset_y: Option<f32>,
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
    /// goes through `tab_index` rather than `AppState.workspace.active_tab`, so each one
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
        let auto_scroll_state = state.clone();
        let auto_scroll_list = uniform_list_scroll_handle.clone();
        let auto_scroller = AutoScroller::new(
            scroll_handle.clone(),
            Rc::new(move |position, bounds, scroll_y, cx| {
                extend_auto_scroll_selection(
                    &auto_scroll_state,
                    &auto_scroll_list,
                    position,
                    bounds,
                    scroll_y,
                    cx,
                );
            }),
        );
        TextEditor {
            state,
            focus_handle,
            scroll_handle,
            uniform_list_scroll_handle,
            auto_scroller,
            macro_at_pending: false,
            row_cache: None,
            scrollbar_grab: Rc::new(Cell::new(0.0)),
            scrollbar_pressed: Rc::new(Cell::new(false)),
            scrollbar_activity: 0,
            last_scrollbar_offset_y: None,
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
        let Some((cursor_top, viewport_h, max_y, offset_x, zoom, normal_size_px, line_spacing)) =
            self.cursor_scroll_geometry(cx)
        else {
            return;
        };
        let line_height = line_height_px(normal_size_px, line_spacing) * zoom;
        let cursor_bottom = cursor_top + line_height;
        let margin = SCROLL_MARGIN_LINES * line_height;

        let offset = self.scroll_handle.offset();
        let visible_top = -offset.y.as_f32();
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
    fn cursor_scroll_geometry(
        &self,
        cx: &Context<Self>,
    ) -> Option<(f32, f32, f32, Pixels, f32, f32, f32)> {
        let state = self.state.read(cx);
        let (cursor_line, cursor_col) = state.pane_cursor_line_col(self.pane);
        let zoom = state.zoom;
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        let line_spacing = state.preferences.line_spacing;
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
        // `display_row` is the row's *content* slot, the last of the slots
        // reserved for it — its glyphs are bottom-aligned there and painted
        // upward through the blank slots before it. So the top of the line is
        // one slot past that index, less a line's height. With a single slot
        // per line (`ROW_SUBDIVISIONS == 1`) this reduces exactly to the old
        // `display_row * line_height`.
        let slot_px = row_slot_px(normal_size_px, line_spacing, zoom);
        let cursor_top = (display_row + 1) as f32 * slot_px
            - line_height_px(normal_size_px, line_spacing) * zoom;

        let viewport_h =
            self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
        if viewport_h <= 0.0 {
            return None;
        }

        let max_y = self.scroll_handle.max_offset().y.as_f32();
        Some((
            cursor_top,
            viewport_h,
            max_y,
            self.scroll_handle.offset().x,
            zoom,
            normal_size_px,
            line_spacing,
        ))
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
    ) -> (
        Rc<Vec<(usize, usize, usize)>>,
        Rc<Vec<Option<usize>>>,
        Rc<Vec<usize>>,
    ) {
        let idx = self.tab_index(cx);
        let state = self.state.read(cx);
        let dragging = state.workspace.split_dragging;
        let invisibility = state.ui.invisibility_mode;
        let cite_size = state.preferences.cite_size_half_points;
        let fold_version = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.fold_version)
            .unwrap_or(0);
        let folds = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.folded_headings.clone())
            .unwrap_or_default();
        let tab_id = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.id.0)
            .unwrap_or(usize::MAX);
        let content_version = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.document.content_version)
            .unwrap_or(0);
        let zoom = state.zoom;
        let line_spacing = state.preferences.line_spacing;
        if let Some(cache) = self.row_cache.as_ref() {
            if row_cache_is_valid_for(
                cache,
                tab_id,
                content_version,
                viewport_width,
                zoom,
                line_spacing,
                dragging,
                invisibility,
                fold_version,
            ) {
                return (
                    cache.rows.clone(),
                    cache.display_to_wrap.clone(),
                    cache.wrap_to_display.clone(),
                );
            }
        }
        let content = state.pane_content(self.pane).to_string();
        let paragraphs = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.document.paragraphs().to_vec())
            .unwrap_or_default();
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        let lines = document_lines(&content);
        let rows = Rc::new(visual_rows_for_viewport(
            cx,
            &lines,
            viewport_width,
            zoom,
            &paragraphs,
            normal_size_px,
        ));
        let folded_paras = AppState::folded_paragraphs(&paragraphs, &folds);
        let hidden = hidden_wrap_rows(&rows, &paragraphs, invisibility, cite_size, &folded_paras);
        let (display_to_wrap, wrap_to_display) = expand_rows_for_display(
            &rows,
            &paragraphs,
            zoom,
            &hidden,
            normal_size_px,
            line_spacing,
        );
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
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        let row_height = row_slot_px(normal_size_px, state.preferences.line_spacing, zoom);
        if row_height <= 0.0 {
            return false;
        }
        let viewport_h =
            self.scroll_handle.bounds().size.height.as_f32() - 2.0 * CONTENT_PADDING_PX;
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
        let Some((cursor_top, viewport_h, max_y, offset_x, zoom, normal_size_px, line_spacing)) =
            self.cursor_scroll_geometry(cx)
        else {
            return;
        };
        let target_visible_top =
            cursor_top - (viewport_h - line_height_px(normal_size_px, line_spacing) * zoom) / 2.0;
        let new_y = (-target_visible_top).clamp(-max_y.max(0.0), 0.0);
        self.scroll_handle.set_offset(point(offset_x, px(new_y)));
    }

    /// Real vim's `zt`: scrolls so the cursor's line sits at the top edge
    /// of the viewport. Shares `cursor_scroll_geometry` with
    /// `scroll_to_cursor_centered` above — see that function's doc comment
    /// for why this needs live GPUI viewport geometry rather than living in
    /// `AppState`.
    fn scroll_to_cursor_top(&self, cx: &Context<Self>) {
        let Some((cursor_top, _viewport_h, max_y, offset_x, _zoom, _normal_size_px, _line_spacing)) =
            self.cursor_scroll_geometry(cx)
        else {
            return;
        };
        let new_y = (-cursor_top).clamp(-max_y.max(0.0), 0.0);
        self.scroll_handle.set_offset(point(offset_x, px(new_y)));
    }

    /// Real vim's `zb`: scrolls so the cursor's line sits at the bottom
    /// edge of the viewport.
    fn scroll_to_cursor_bottom(&self, cx: &Context<Self>) {
        let Some((cursor_top, viewport_h, max_y, offset_x, zoom, normal_size_px, line_spacing)) =
            self.cursor_scroll_geometry(cx)
        else {
            return;
        };
        let target_visible_top =
            cursor_top - (viewport_h - line_height_px(normal_size_px, line_spacing) * zoom);
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
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        let paragraphs = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.document.paragraphs().to_vec())
            .unwrap_or_default();
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
        let line_chars: Vec<char> = lines
            .get(line)
            .map(|l| l.chars().collect())
            .unwrap_or_default();
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
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        let paragraphs = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.document.paragraphs().to_vec())
            .unwrap_or_default();
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

        let Some((target_line, target_col)) = visual_row_step(
            &rows,
            current_row,
            col_in_row,
            delta,
            &paragraphs,
            normal_size_px,
            zoom,
        ) else {
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

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if self.state.read(cx).ui.editor_context_menu.is_some() {
            self.state.update(cx, |s, cx| {
                s.close_editor_context_menu();
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
            .workspace
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
        if self.state.read(cx).ui.read_mode && plain_arrow {
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

        self.process_key(
            &ks.key,
            ks.modifiers.shift,
            ks.modifiers.control,
            ks.modifiers.platform,
            ks.key_char.as_deref(),
            window,
            cx,
        );
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
        if self
            .state
            .update(cx, |state, _cx| state.take_pending_editor_focus(self.pane))
        {
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
            state.take_pending_scroll_to_cursor(self.pane)
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
        let active_tab_id = self
            .tab_index(cx)
            .and_then(|i| self.state.read(cx).workspace.tabs.get(i))
            .map(|t| t.id.0);
        if self.last_seen_active_tab != active_tab_id {
            if let Some(prev_id) = self.last_seen_active_tab {
                self.tab_scroll_offsets
                    .insert(prev_id, self.scroll_handle.offset());
            }
            let restore = active_tab_id
                .and_then(|id| self.tab_scroll_offsets.get(&id))
                .copied()
                .unwrap_or_default();
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
        let theme_mode = state.preferences.theme_mode;
        let cursor_style = if state.global_vim.vim_enabled {
            CursorStyle::Block
        } else {
            CursorStyle::Line
        };
        let normal_size_px = state.effective_normal_size_half_points() as f32 / 2.0;
        // The document's own `<w:docDefaults>` font, when it names one this app
        // can actually render. `is_curated_font` is the same gate `run.font`
        // already passes through — it covers the bundled families and any the
        // user has imported, and everything else keeps falling back to
        // `FONT_FAMILY` for the reason `apply_run_style` documents (GPUI can't
        // match bold/italic within a family whose faces aren't all loaded).
        let body_font: SharedString = state
            .effective_body_font()
            .filter(|name| is_curated_font(name))
            .map(|name| SharedString::from(name.to_string()))
            .unwrap_or_else(|| SharedString::from(FONT_FAMILY));
        let line_spacing = state.preferences.line_spacing;
        let viewport_width = self.scroll_handle.bounds().size.width.as_f32();
        let dragging = state.workspace.split_dragging;
        // Scroll movement re-arms the scrollbar's fade. Compared with a small
        // tolerance so sub-pixel jitter in the offset can't hold the bar
        // permanently visible by restarting the animation every frame.
        let scroll_y_now = self.scroll_handle.offset().y.as_f32();
        if self
            .last_scrollbar_offset_y
            .is_none_or(|prev| (prev - scroll_y_now).abs() > 0.5)
        {
            self.last_scrollbar_offset_y = Some(scroll_y_now);
            self.scrollbar_activity = self.scrollbar_activity.wrapping_add(1);
        }
        let scrollbar_activity = self.scrollbar_activity;
        let invisibility = state.ui.invisibility_mode;
        let cite_size = state.preferences.cite_size_half_points;
        let fold_version = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.fold_version)
            .unwrap_or(0);
        let folds = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.folded_headings.clone())
            .unwrap_or_default();
        let tab_id = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.id.0)
            .unwrap_or(usize::MAX);
        let content_version = idx
            .and_then(|i| state.workspace.tabs.get(i))
            .map(|t| t.document.content_version)
            .unwrap_or(0);
        let cache_valid = self.row_cache.as_ref().is_some_and(|c| {
            row_cache_is_valid_for(
                c,
                tab_id,
                content_version,
                viewport_width,
                zoom,
                line_spacing,
                dragging,
                invisibility,
                fold_version,
            )
        });
        // Only pay for the full content/paragraphs clone on a cache miss.
        // `document_lines`/word-wrap need `cx` free of `state`'s borrow (see
        // `let _ = state;` below), so the actual wrap happens further down —
        // this just captures the owned data a miss needs before that borrow ends.
        let fresh_content_and_paragraphs = (!cache_valid).then(|| {
            (
                state.pane_content(self.pane).to_string(),
                state
                    .workspace
                    .tabs
                    .get(idx.unwrap_or(usize::MAX))
                    .map(|t| t.document.paragraphs().to_vec())
                    .unwrap_or_default(),
            )
        });
        let is_new_tab = state
            .workspace
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .map(|t| t.is_blank_new_tab())
            .unwrap_or(true);
        let banner_message = state
            .workspace
            .tabs
            .get(idx.unwrap_or(usize::MAX))
            .and_then(|t| t.banner_message());
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
            .workspace
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
        let mode_indicator_text: Option<&'static str> = if state.global_vim.vim_enabled {
            idx.and_then(|i| state.workspace.tabs.get(i))
                .map(|t| match t.vim_mode {
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
            .workspace
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
                    if !buf.is_empty() {
                        buf.push(' ');
                    }
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
                cx,
                &lines,
                viewport_width,
                zoom,
                &paragraphs,
                normal_size_px,
            );
            let folded_paras = AppState::folded_paragraphs(&paragraphs, &folds);
            let hidden =
                hidden_wrap_rows(&rows, &paragraphs, invisibility, cite_size, &folded_paras);
            let (display_to_wrap, wrap_to_display) = expand_rows_for_display(
                &rows,
                &paragraphs,
                zoom,
                &hidden,
                normal_size_px,
                line_spacing,
            );

            self.row_cache = Some(RowCache {
                tab_id,
                content_version,
                invisibility,
                fold_version,
                viewport_width_bits: viewport_width.to_bits(),
                zoom_bits: zoom.to_bits(),
                line_spacing_bits: line_spacing.to_bits(),
                lines: Rc::new(lines),
                line_chars: Rc::new(line_chars),
                line_byte_starts: Rc::new(line_byte_starts),
                rows: Rc::new(rows),
                paragraphs: Rc::new(paragraphs.to_vec()),
                display_to_wrap: Rc::new(display_to_wrap),
                wrap_to_display: Rc::new(wrap_to_display),
            });
        }
        let cache = self.row_cache.as_ref().expect(
            "populated just above on a miss; cache_valid guarantees it already existed on a hit",
        );
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
            .when_some(banner_message, |d, message| {
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
                        .child(message)
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
                                        s.dismiss_unsupported_banner(pane);
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            // Claim the pane before anything reads a tab: `focus_pane`
                            // repoints `active_tab`, and every state call below (cursor
                            // placement, selection) resolves through it.
                            let pane = this.pane;
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(pane);
                                cx.notify();
                            });
                            this.focus_handle.clone().focus(window, cx);
                            let bounds = this.scroll_handle.bounds();
                            let scroll_y = this.scroll_handle.offset().y.as_f32();
                            let zoom = this.state.read(cx).zoom;
                            let font_size_px =
                                this.state.read(cx).effective_normal_size_half_points() as f32
                                    / 2.0;
                            let line_spacing = this.state.read(cx).preferences.line_spacing;
                            let paragraphs = {
                                let st = this.state.read(cx);
                                pane_idx
                                    .and_then(|i| st.workspace.tabs.get(i))
                                    .map(|t| t.document.paragraphs().to_vec())
                                    .unwrap_or_default()
                            };
                            let (rows, display_to_wrap, _) =
                                this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                            let row_height_px = real_row_height_px(
                                &this.uniform_list_scroll_handle,
                                display_to_wrap.len(),
                                font_size_px,
                                zoom,
                                line_spacing,
                            );
                            let (line, col) = line_col_from_mouse_position(
                                ev.position,
                                bounds,
                                scroll_y,
                                &rows,
                                &display_to_wrap,
                                zoom,
                                font_size_px,
                                &paragraphs,
                                row_height_px,
                            );
                            let click_count = ev.click_count;
                            let shift_click = ev.modifiers.shift && click_count == 1;
                            this.state.update(cx, |state, cx| {
                                state.close_editor_context_menu();
                                state.clear_similar_selection();
                                if shift_click {
                                    // Shift+Click: extend the selection from wherever the
                                    // cursor already is to the click point — the same
                                    // `extend_selection_to_line_col` a click-drag calls on
                                    // every `on_mouse_move`, just driven by one click
                                    // instead of a series of move events. Anchors at the
                                    // current cursor position when there's no selection
                                    // yet (see that function's own doc comment).
                                    state.extend_selection_to_line_col(line, col);
                                } else {
                                    // `set_cursor_from_line_col` does the line/col -> byte-offset
                                    // conversion (there's no standalone public helper for it) and
                                    // leaves the result in `tab.cursor`, so double/triple-click
                                    // reuse that single call instead of re-deriving the byte
                                    // position themselves.
                                    state.set_cursor_from_line_col(line, col);
                                    let byte_pos = pane_idx
                                        .and_then(|i| state.workspace.tabs.get(i))
                                        .map(|t| t.cursor)
                                        .unwrap_or(0);
                                    match click_count {
                                        2 => state.select_word_at(byte_pos),
                                        3 => state.select_line_at(byte_pos),
                                        _ => {}
                                    }
                                }
                                cx.notify();
                            });
                            cx.notify();
                        }),
                    )
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
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let pane = this.pane;
                            this.state.update(cx, |s, cx| {
                                s.focus_pane(pane);
                                cx.notify();
                            });
                            this.focus_handle.clone().focus(window, cx);
                            let has_selection = {
                                let st = this.state.read(cx);
                                pane_idx
                                    .and_then(|i| st.workspace.tabs.get(i))
                                    .is_some_and(|t| t.selection.is_some())
                            };
                            // Resolve the click to a (line, col) whether or not there's a
                            // selection — without a selection it also moves the caret,
                            // but either way it's what locates a misspelled word below.
                            let bounds = this.scroll_handle.bounds();
                            let scroll_y = this.scroll_handle.offset().y.as_f32();
                            let zoom = this.state.read(cx).zoom;
                            let font_size_px =
                                this.state.read(cx).effective_normal_size_half_points() as f32
                                    / 2.0;
                            let line_spacing = this.state.read(cx).preferences.line_spacing;
                            let paragraphs = {
                                let st = this.state.read(cx);
                                pane_idx
                                    .and_then(|i| st.workspace.tabs.get(i))
                                    .map(|t| t.document.paragraphs().to_vec())
                                    .unwrap_or_default()
                            };
                            let (rows, display_to_wrap, _) =
                                this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                            let row_height_px = real_row_height_px(
                                &this.uniform_list_scroll_handle,
                                display_to_wrap.len(),
                                font_size_px,
                                zoom,
                                line_spacing,
                            );
                            let (line, col) = line_col_from_mouse_position(
                                ev.position,
                                bounds,
                                scroll_y,
                                &rows,
                                &display_to_wrap,
                                zoom,
                                font_size_px,
                                &paragraphs,
                                row_height_px,
                            );
                            if !has_selection {
                                this.state.update(cx, |state, _cx| {
                                    state.set_cursor_from_line_col(line, col)
                                });
                            }

                            // Did the click land on a squiggle? `suggest` runs here, once,
                            // rather than during render — it's a dictionary search, far
                            // slower than the per-word `check` the squiggles use.
                            let spell_target = {
                                let st = this.state.read(cx);
                                if !st.preferences.spellcheck_enabled {
                                    None
                                } else {
                                    let content = pane_idx
                                        .and_then(|i| st.workspace.tabs.get(i))
                                        .map(|t| t.document.content())
                                        .unwrap_or_default();
                                    let lines = document_lines(&content);
                                    lines.get(line).and_then(|text| {
                                        crate::spellcheck::misspelled_ranges(
                                            text,
                                            &st.user_dictionary,
                                        )
                                        .into_iter()
                                        .find(|&(s, e)| col >= s && col < e)
                                        .map(
                                            |(start_col, end_col)| {
                                                let word: String = text
                                                    .chars()
                                                    .skip(start_col)
                                                    .take(end_col - start_col)
                                                    .collect();
                                                let suggestions = crate::spellcheck::suggest(&word);
                                                SpellTarget {
                                                    line,
                                                    start_col,
                                                    end_col,
                                                    word,
                                                    suggestions,
                                                }
                                            },
                                        )
                                    })
                                }
                            };

                            this.state.update(cx, |state, cx| {
                                state.open_editor_context_menu(EditorContextMenu {
                                    position: (ev.position.x.as_f32(), ev.position.y.as_f32()),
                                    spell_target,
                                });
                                cx.notify();
                            });
                        }),
                    )
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
                        if !ev.dragging() {
                            // Self-heal. If a release ever escapes both mouse-up
                            // handlers, a stuck flag would kill text selection for
                            // the rest of the session; the first move with no button
                            // held proves the pointer is up and clears it.
                            this.scrollbar_pressed.set(false);
                            return;
                        }
                        // A drag that belongs to something else is not a text
                        // selection. Dragging the scrollbar holds the left button
                        // down and moves the pointer across the document, which is
                        // indistinguishable from a click-drag at this level — so it
                        // selected everything it passed over, and fed the edge
                        // auto-scroller besides (bug report: "scrolling down
                        // highlights text because it requires LMB down"). The same
                        // guard covers the tab, sidebar-resize and split-resize
                        // drags, none of which should extend a selection either.
                        if cx.has_active_drag() || this.scrollbar_pressed.get() {
                            return;
                        }
                        let bounds = this.scroll_handle.bounds();
                        let scroll_y = this.scroll_handle.offset().y.as_f32();
                        let zoom = this.state.read(cx).zoom;
                        let font_size_px =
                            this.state.read(cx).effective_normal_size_half_points() as f32 / 2.0;
                        let line_spacing = this.state.read(cx).preferences.line_spacing;
                        let paragraphs = {
                            let st = this.state.read(cx);
                            pane_idx
                                .and_then(|i| st.workspace.tabs.get(i))
                                .map(|t| t.document.paragraphs().to_vec())
                                .unwrap_or_default()
                        };
                        let (rows, display_to_wrap, _) =
                            this.cached_or_fresh_row_tables(cx, bounds.size.width.as_f32());
                        let row_height_px = real_row_height_px(
                            &this.uniform_list_scroll_handle,
                            display_to_wrap.len(),
                            font_size_px,
                            zoom,
                            line_spacing,
                        );
                        let (line, col) = line_col_from_mouse_position(
                            ev.position,
                            bounds,
                            scroll_y,
                            &rows,
                            &display_to_wrap,
                            zoom,
                            font_size_px,
                            &paragraphs,
                            row_height_px,
                        );
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
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _window, _cx| {
                            this.auto_scroller.stop();
                            this.scrollbar_pressed.set(false);
                        }),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _ev, _window, _cx| {
                            this.auto_scroller.stop();
                            this.scrollbar_pressed.set(false);
                        }),
                    )
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
                            let spellcheck_enabled =
                                self.state.read(cx).preferences.spellcheck_enabled;
                            let user_dictionary = self.state.read(cx).user_dictionary.clone();
                            let spell_cache = self.spell_cache.clone();
                            let spellcheck_color = crate::editor::color::highlight_color_hex(
                                &self.state.read(cx).preferences.spellcheck_underline_color,
                            );
                            let invisibility_mode = self.state.read(cx).ui.invisibility_mode;
                            let cite_size_half_points =
                                self.state.read(cx).preferences.cite_size_half_points;
                            let folded_headings = {
                                let st = self.state.read(cx);
                                st.workspace
                                    .tabs
                                    .get(st.workspace.active_tab)
                                    .map(|t| t.folded_headings.clone())
                                    .unwrap_or_default()
                            };
                            let fold_state = self.state.clone();
                            move |range: std::ops::Range<usize>, _window, _cx| {
                                range
                                    .map(|display_idx| {
                                        // A `None` slot is a blank spacer reserved before an
                                        // oversized (card-style/heading) row so its content
                                        // has empty space to visually spill upward into
                                        // instead of overlapping the row above — see
                                        // `expand_rows_for_display`'s doc comment. Same
                                        // fixed `.h()` as a real row, just no content, so
                                        // `uniform_list`'s single-measurement layout still
                                        // sees a uniform row height everywhere.
                                        let Some(visual_idx) = display_to_wrap[display_idx] else {
                                            return div()
                                                .h(px(row_slot_px(
                                                    normal_size_px,
                                                    line_spacing,
                                                    zoom,
                                                )))
                                                .into_any_element();
                                        };
                                        let (li, row_start, row_end) = rows[visual_idx];
                                        let chars = &line_chars[li];
                                        let row_text: String =
                                            chars[row_start..row_end].iter().collect();

                                        // `.then(|| ...)` (lazy), not `.then_some(...)` — the latter's
                                        // argument is a plain value, evaluated eagerly *before* the
                                        // bool is even checked. With `then_some`, `cursor_col - row_start`
                                        // was computed for every row regardless of the condition, and
                                        // underflowed (panicked) on any row whose row_start exceeded the
                                        // cursor's column — i.e. almost any row that isn't the cursor's own.
                                        let row_cursor_col = (cursor_visual_row
                                            == Some(display_idx))
                                        .then(|| cursor_col - row_start);

                                        // Clip the logical line's selection char-range (if any) down
                                        // to this row's own [row_start, row_end) sub-range, then
                                        // rebase it to be relative to the row instead of the line.
                                        let row_selections: Vec<(usize, usize)> = selections
                                            .iter()
                                            .filter_map(|&(s, e)| {
                                                selection_span_for_line(
                                                    &lines[li],
                                                    line_byte_starts[li],
                                                    s,
                                                    e,
                                                )
                                            })
                                            .filter_map(|(sel_start, sel_end)| {
                                                let clipped_start = sel_start.max(row_start);
                                                let clipped_end = sel_end.min(row_end);
                                                // Same eager-vs-lazy pitfall as row_cursor_col above: use
                                                // `.then(|| ...)` since clipped_end can be < row_start when
                                                // the selection doesn't reach this row, which would
                                                // underflow `clipped_end - row_start` if evaluated eagerly.
                                                (clipped_start < clipped_end).then(|| {
                                                    (
                                                        clipped_start - row_start,
                                                        clipped_end - row_start,
                                                    )
                                                })
                                            })
                                            .collect();

                                        // Rich-text formatting (Phase 1): clip this logical
                                        // line's paragraph run boundaries down to this row's
                                        // own [row_start, row_end) sub-range, same rebasing
                                        // pattern as `row_selection` above — a wrapped row
                                        // only needs to know about the runs it actually spans.
                                        let row_run_spans: Vec<(usize, usize, usize)> = paragraphs
                                            .get(li)
                                            .map(paragraph_run_char_spans)
                                            .unwrap_or_default()
                                            .into_iter()
                                            .filter_map(|(rs, re, run_idx)| {
                                                let clipped_start = rs.max(row_start);
                                                let clipped_end = re.min(row_end);
                                                (clipped_start < clipped_end).then(|| {
                                                    (
                                                        clipped_start - row_start,
                                                        clipped_end - row_start,
                                                        run_idx,
                                                    )
                                                })
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
                                        let row_misspelled: Vec<(usize, usize)> =
                                            if spellcheck_enabled {
                                                spell_ranges_cached(
                                                    &spell_cache,
                                                    &lines[li],
                                                    &user_dictionary,
                                                )
                                                .iter()
                                                .filter_map(|&(ms, me)| {
                                                    let clipped_start = ms.max(row_start);
                                                    let clipped_end = me.min(row_end);
                                                    (clipped_start < clipped_end).then(|| {
                                                        (
                                                            clipped_start - row_start,
                                                            clipped_end - row_start,
                                                        )
                                                    })
                                                })
                                                .collect()
                                            } else {
                                                Vec::new()
                                            };

                                        // Check if previous paragraph also has box_format (for merging boxes)
                                        let prev_has_box = li > 0
                                            && paragraphs.get(li - 1).is_some_and(|p| {
                                                p.runs.iter().any(|r| r.box_format)
                                            });

                                        // Fold marker, on a heading's *first* row only — a
                                        // wrapped heading gets one marker, not one per visual
                                        // row. Hidden until the row is hovered, so a folded
                                        // outline reads as clean text rather than a column of
                                        // arrows.
                                        let row_heading =
                                            paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                                        let fold_toggle = (row_heading != 0 && row_start == 0)
                                            .then(|| {
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
                                                    .group_hover(FOLD_ROW_GROUP, move |s| {
                                                        s.text_color(rgb(p.text_muted))
                                                    })
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

                                        // List marker (bullet glyph or number), on a list
                                        // paragraph's *first* row only — see
                                        // `LIST_GUTTER_PX`'s doc comment on why wrapped
                                        // continuation rows don't get one. `list_item_ordinal`
                                        // uses the same contiguous-run rule
                                        // `docx_parser::assign_list_num_ids` uses on the
                                        // write side, so what's on screen always matches
                                        // what gets saved. `(level + 1)` widths: level 0
                                        // gets one gutter's worth of indent (unchanged from
                                        // Phase 1), each deeper level shifts the marker one
                                        // more gutter-width right — approximates Word's real
                                        // per-level indent step (720 twips per
                                        // `cascade_level_xml`) as a single fixed pixel step
                                        // rather than modeling twips.
                                        let list_marker = (row_start == 0)
                                            .then(|| paragraphs.get(li).and_then(|para| para.list))
                                            .flatten()
                                            .map(|item| {
                                                let ordinal = list_item_ordinal(&paragraphs, li);
                                                div()
                                                    .id(ElementId::named_usize("list-marker", li))
                                                    .w(px(LIST_GUTTER_PX
                                                        * (item.level as f32 + 1.0)
                                                        * zoom))
                                                    .flex_none()
                                                    .flex()
                                                    .justify_end()
                                                    .pr(px(4.0 * zoom))
                                                    .text_color(rgb(p.text))
                                                    .child(list_marker_text_for_level(
                                                        item.kind, item.level, ordinal,
                                                    ))
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
                                            list_marker,
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
                                        let heading =
                                            paragraphs.get(li).map(|p| p.heading).unwrap_or(0);
                                        // `normal_size_px` (settings.conf's `normal_text_size`,
                                        // read once above as `normal_text_size_half_points`)
                                        // is the visual default for any run with no explicit
                                        // `FontSize` override (`size == 0` — brand-new
                                        // documents' single default run, and any plain-typed
                                        // text) — a run-level or heading-level override still
                                        // wins underneath, same as before this was configurable.
                                        // The row's base font size, and the line box that
                                        // goes with it. `.line_height()` writes into the
                                        // cascading text style, so every span in this row —
                                        // including the ones carrying a highlight background
                                        // or an emphasis ring — is laid out in a box this
                                        // tall instead of GPUI's default golden-ratio one.
                                        // See `text_line_box_px` for why that default is
                                        // what made highlights cover each other.
                                        // The same size the row's space was reserved against
                                        // (`line_font_px`), not just the heading size — a
                                        // manually enlarged run in a plain paragraph grows the
                                        // reservation, so its line box has to grow with it or
                                        // its highlight would be painted shorter than its own
                                        // glyphs.
                                        let row_font_px =
                                            line_font_px(paragraphs.get(li), zoom, normal_size_px);
                                        let row_div = div()
                                            .font_family(body_font.clone())
                                            .text_size(px(normal_size_px * zoom))
                                            .line_height(px(text_line_box_px(row_font_px)))
                                            .text_color(rgb(p.text));
                                        let row_div = match heading_font_size_px(heading, zoom) {
                                            Some(size) => row_div
                                                .text_size(px(size))
                                                .font_weight(FontWeight::BOLD),
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
                                            .h(px(row_slot_px(normal_size_px, line_spacing, zoom)))
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
                                    })
                                    .collect()
                            }
                        })
                        // `uniform_list` is the actual scrollable element now (it
                        // sets vertical overflow internally); padding/border move
                        // here from the old outer div for the reason explained
                        // above this `.child(...)` block.
                        .with_decoration(ScrollbarDecoration {
                            scroll_handle: self.scroll_handle.clone(),
                            grab_offset: self.scrollbar_grab.clone(),
                            pressed: self.scrollbar_pressed.clone(),
                            activity: scrollbar_activity,
                            track: p.editor_bg_raised,
                            thumb: p.border,
                            thumb_hover: p.text_muted,
                        })
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
                        // Scrollbar thumb drag. `on_drag_move` dispatches in the
                        // capture phase and checks only the drag's *type*, not the
                        // pointer's position (gpui's `Interactivity::on_drag_move`),
                        // so this keeps tracking after the cursor leaves the thumb —
                        // or the editor entirely — which is what dragging a scrollbar
                        // demands. Registered here rather than on the thumb because
                        // the thumb is rebuilt every frame by the decoration.
                        .on_drag_move({
                            let scroll_handle = self.scroll_handle.clone();
                            let grab = self.scrollbar_grab.clone();
                            move |e: &DragMoveEvent<ScrollbarDragPayload>, _window, cx| {
                                let d = e.drag(cx);
                                if d.travel <= 0.0 {
                                    return;
                                }
                                // Where the thumb's *top* would sit if it followed the
                                // pointer, keeping the grab point under the cursor.
                                let thumb_top =
                                    e.event.position.y.as_f32() - grab.get() - d.track_top;
                                let fraction = (thumb_top / d.travel).clamp(0.0, 1.0);
                                let offset = scroll_handle.offset();
                                // Negative Y scrolls down, matching every other scroll
                                // path in this file.
                                scroll_handle
                                    .set_offset(point(offset.x, px(-(fraction * d.max_scroll))));
                                cx.refresh_windows();
                            }
                        })
                        .p(px(16.0))
                        // Thin focus ring so the user can tell where key input lands
                        .border_1()
                        .border_color(if is_focused {
                            rgb(p.accent)
                        } else {
                            rgb(p.editor_bg)
                        }),
                    ), // closes the "text-editor" div's .child(uniform_list...)
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
                    if !line.is_empty() {
                        line.push(' ');
                    }
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
            .when_some(
                self.state.read(cx).ui.editor_context_menu.clone(),
                |el, menu| {
                    let has_selection = self
                        .state
                        .read(cx)
                        .workspace
                        .tabs
                        .get(self.tab_index(cx).unwrap_or(usize::MAX))
                        .is_some_and(|t| t.selection.is_some());
                    el.child(render_context_menu(menu, p, has_selection, &self.state))
                },
            )
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
            .when(enabled, |d| {
                d.cursor_pointer().hover(move |s| s.bg(rgb(p.chrome_hover)))
            })
            .child(label)
    };
    let separator = || div().h(px(1.0)).my(px(4.0)).bg(rgb(p.border_subtle));

    // Cut/Copy are no-ops without a selection; show them muted rather than
    // hiding them so the menu doesn't change shape between clicks.
    let action_item = |id: &'static str,
                       label: &'static str,
                       enabled: bool,
                       action: fn() -> Box<dyn Action>,
                       state: Entity<AppState>| {
        row(id.into(), label.to_string(), enabled, p.text).when(enabled, |d| {
            d.on_click(move |_ev, window, cx| {
                state.update(cx, |s, cx| {
                    s.close_editor_context_menu();
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
                    row(
                        ("editor-ctx-suggestion", i).into(),
                        suggestion.clone(),
                        true,
                        p.text,
                    )
                    .on_click(move |_ev, _window, cx| {
                        state.update(cx, |s, cx| {
                            s.replace_spell_target(&target, &replacement);
                            s.close_editor_context_menu();
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
        .child(action_item(
            "editor-ctx-cut",
            "Cut",
            has_selection,
            || Box::new(CutAction),
            state_handle.clone(),
        ))
        .child(action_item(
            "editor-ctx-copy",
            "Copy",
            has_selection,
            || Box::new(CopyAction),
            state_handle.clone(),
        ))
        .child(action_item(
            "editor-ctx-paste",
            "Paste",
            true,
            || Box::new(PasteAction),
            state_handle.clone(),
        ));

    // ── Add to Dictionary ──────────────────────────────────────────────────
    if let Some(target) = &menu.spell_target {
        let state = state_handle.clone();
        let word = target.word.clone();
        panel = panel.child(separator()).child(
            row(
                "editor-ctx-add-to-dict".into(),
                "Add to Dictionary".to_string(),
                true,
                p.text,
            )
            .on_click(move |_ev, _window, cx| {
                state.update(cx, |s, cx| {
                    s.add_to_user_dictionary(&word);
                    s.close_editor_context_menu();
                    cx.notify();
                });
            }),
        );
    }

    let dismiss_state = state_handle.clone();
    deferred(
        anchored()
            .position(point(px(x), px(y)))
            .snap_to_window()
            .child(
                div()
                    .id("editor-context-menu-dismiss")
                    .on_mouse_down_out(move |_ev: &MouseDownEvent, _window, cx| {
                        dismiss_state.update(cx, |s, cx| {
                            if s.ui.editor_context_menu.is_some() {
                                s.close_editor_context_menu();
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
    // The bullet/number marker for this row, when it is a list paragraph's
    // first row (`row_start == 0` — see `LIST_GUTTER_PX`'s doc comment on
    // why wrapped continuation rows don't get one). Composed the same way
    // as `fold_toggle` above and for the same reason: it has to sit beside
    // the `flex_1` line wrapper, inside a Pocket's box if this line
    // happens to also be boxed (not a realistic combination in practice,
    // but composed correctly regardless rather than assumed away).
    list_marker: Option<AnyElement>,
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
    let hides_anything = run_is_hidden(
        invisibility,
        heading,
        false,
        false,
        0,
        cite_size_half_points,
    );

    // The fast paths below emit one element for the whole row, which cannot
    // express "some runs drawn, some not" — fall through to the per-run path
    // whenever anything might be hidden. `list_marker.is_none()` for the
    // same reason as the alignment/box checks a few lines down: a fast
    // return here is a bare div with no room to compose a leading gutter
    // element beside it, which would silently drop the marker on exactly
    // the most common case (a plain, single-run, unformatted list item).
    if !hides_anything
        && cursor_col.is_none()
        && selections.is_empty()
        && misspelled.is_empty()
        && list_marker.is_none()
    {
        // Don't take any fast path if alignment or a box-shaped visual is
        // needed — both are only drawn correctly by the full path below (the
        // box wrapper in particular: a bare-div return skips it entirely,
        // which was the bug where a Pocket's box only drew once the cursor —
        // or a selection, forcing the same fall through — put the line on
        // the slow path). Also the bug where a paragraph's alignment (e.g.
        // Center, read from a real docx or set by a button click) was
        // silently ignored on any row that reached one of these fast
        // returns.
        //
        // `paints_run_box`'s three properties hit the same bug for a
        // different reason: a bare-div return here isn't wrapped in
        // `line_div`, so the div carrying the highlight/box paint has no
        // constraint against its `flex_col` parent's cross-axis stretch and
        // visibly fills the whole row even when the line is one short word —
        // the slow path avoids this via each span's own `flex_shrink(0.0)`.
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
                if !needs_alignment && !paints_run_box(run) {
                    return apply_run_style(div(), run, zoom, pal)
                        .child(line.to_string())
                        .into_any_element();
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
        let sub_cursor = sub_cursor_for_run(cursor_col, run_start, run_end, chars.len());
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

            // Inside the border, so a fold marker/list marker reads as part
            // of the box rather than floating outside it. Not a realistic
            // combination in practice (list paragraphs don't carry
            // box_format), but composed generally rather than assumed away.
            let leading: Vec<AnyElement> = fold_toggle.into_iter().chain(list_marker).collect();
            box_div = if leading.is_empty() {
                box_div.child(line_div)
            } else {
                box_div
                    .flex()
                    .flex_row()
                    .items_center()
                    .children(leading)
                    .child(div().flex_1().min_w_0().child(line_div))
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
    let leading: Vec<AnyElement> = fold_toggle.into_iter().chain(list_marker).collect();
    match leading.is_empty() {
        false => div()
            .flex()
            .flex_row()
            .items_center()
            .children(leading)
            .child(div().flex_1().min_w_0().child(line_div))
            .into_any_element(),
        true => line_div.into_any_element(),
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
        //
        // A GPUI div has one `bg` field, so a plain `.bg()` here would
        // replace rather than blend with a run's own formatting-highlight
        // background (`apply_run_style`'s `run.highlight` case a few lines
        // up) — the highlight would vanish under the selection instead of
        // showing through it. Same overlay technique as `CursorStyle::Line`
        // just above: leave `el`'s own bg (the highlight, if any) as the
        // base layer and paint the translucent selection tint as a
        // full-size absolutely positioned child on top of it, added before
        // the text child below so the glyphs still paint on top of both.
        SegmentStyle::Selection => el.relative().child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .w_full()
                .h_full()
                .bg(rgba((pal.selection << 8) | 0x80)),
        ),
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
    let el = if hidden {
        el.text_color(transparent_black())
    } else {
        el
    };

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
    let underline_hex = run
        .and_then(|r| r.color.as_deref())
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
    if run.bold {
        el = el.font_weight(FontWeight::BOLD);
    }
    if run.italic {
        el = el.italic();
    }
    if run.underline {
        el = el.underline();
    }
    if run.double_underline {
        el = el.underline();
    }
    if run.strikethrough {
        el = el.line_through();
    }
    // Note: box_format is applied at the line level in render_line(), not here at the run level
    //
    // An inset box-shadow, not `border_1()`: a real border is part of
    // Taffy's box model and adds to this span's layout height (confirmed
    // against GPUI's own `taffy.rs`, which feeds `border` into the layout
    // style but never touches `box_shadow`). A box-shadow is paint only —
    // `inset: true` draws the ring just inside the span's existing bounds
    // instead of growing them, so the emphasis box gets slightly smaller
    // rather than the row around it getting taller (unverified visually —
    // no display in this sandbox — but structurally this is the one option
    // GPUI offers that can't touch layout at all).
    if run.emphasis_boxed {
        el = el.shadow(vec![BoxShadow {
            color: rgb(pal.text).into(),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.0),
            inset: true,
        }]);
    }
    if run.highlight {
        let base_hex = crate::editor::color::highlight_color_hex(&run.highlight_color);
        let text_hex = run
            .color
            .as_deref()
            .and_then(|c| u32::from_str_radix(c, 16).ok())
            .unwrap_or(pal.text);
        // Word darkens a light highlight sitting under light text so the text
        // stays legible (white on yellow is otherwise unreadable). This used
        // to be unconditional because the app was dark-only; now that the
        // default text color follows the theme, the condition does the gating
        // by itself — in light mode `p.text` is dark, so a yellow highlight
        // is left alone, which is what Word does there too.
        let highlight_hex = if crate::editor::color::is_light_color(base_hex)
            && crate::editor::color::is_light_color(text_hex)
        {
            crate::editor::color::darken_for_light_text(base_hex)
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

/// Fixed pixel width reserved for a list paragraph's marker gutter — wide
/// enough for the longest ordinal this app's practical list lengths need
/// ("99." or "iii.") at the default zoom, scaled by `zoom` like every other
/// row-metric constant in this file. Only the *first* visual row of a
/// (possibly word-wrapped) list paragraph gets a gutter and a marker —
/// wrapped continuation rows render flush-left, not Word's true hanging
/// indent.
/// ponytail: simplification, not Word's hanging-indent-on-every-wrapped-row;
/// upgrade if wrapped list items turn out to need it visually.
pub(crate) const LIST_GUTTER_PX: f32 = 28.0;
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
///
/// `r.emphasis` runs are excluded from the max: unlike a card style (which
/// applies its size to every run on the line via `apply_formatting_to_line`,
/// so the whole line really is that size), Emphasis applies to just the
/// selected span via `apply_formatting_to_selection` — one bigger word
/// inside an otherwise-normal paragraph. Letting it into this max reserved
/// a blank spacer row for the *entire* paragraph over one emphasized word
/// (bug report: "applying emphasis to one word increases the line spacing
/// of the entire paragraph"). Emphasis is meant to have no influence on
/// line spacing at all — whatever it needs beyond the reserved slot just
/// renders within the existing row instead of growing it.
///
/// Tag and Cite are excluded from ever needing more than one slot — bug
/// report: "Tags and Cites increase line spacing in vimbatim far more than
/// they do in word". Confirmed against a real Verbatim reference file
/// (`Verbatim_Formatting_To_Compare_To.docx`): Tag's own style
/// (`Heading4`/"Tag" in `word/styles.xml`) carries only `w:spacing
/// w:before="40" w:after="0"` (2pt, effectively nothing) with no `w:line`
/// override of its own — it inherits Normal's, same as any body paragraph —
/// and Cite (`Style13ptBold`/"Cite") is a *character* style with no
/// paragraph-spacing properties at all. Real Word never gives either a
/// visually reserved extra line; it just renders that one line's glyphs a
/// little taller within its own continuous flow. At this app's real default
/// settings (13px Tag/Cite vs. 11px normal text), the plain size-ratio math
/// below rounds that modest, sub-single-line difference up to a *whole*
/// extra reserved slot — doubling the line's height where Word shows next
/// to nothing extra. Pocket/Hat/Block are unaffected: they're genuinely
/// ~1.45-2.36x normal size (vs. Tag/Cite's ~1.18x) and Word's own reference
/// styles give them real `w:spacing w:before` (12pt/2pt/2pt) on top of that,
/// so their existing multi-slot reservation stays as-is.
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
    // The scrollbar's gutter comes off the top, unconditionally — including
    // when the document is short enough that no bar is drawn. Reserving it
    // only while the bar is visible would re-wrap the whole document (and
    // invalidate the row cache) the moment a document grew past one screen.
    let usable = viewport_width_px - 2.0 * CONTENT_PADDING_PX - SCROLLBAR_GUTTER_PX;
    if usable <= 0.0 {
        f32::MAX
    } else {
        usable
    }
}

pub(crate) fn char_width_fn(cx: &App, font: Font, font_size_px: f32) -> impl FnMut(char) -> f32 {
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
        // Matches display_line's control-character->space substitution (see
        // its comment): the bundled fonts have no glyph for a control
        // character, so measuring it directly would report ~0 width —
        // wrap/scroll math needs the same width the renderer actually
        // paints, or the cursor's visual column and the wrap point silently
        // disagree.
        let c = if c.is_control() { ' ' } else { c };
        if (c as u32) < 128 {
            let idx = c as usize;
            if let Some(w) = ascii_cache[idx] {
                return w;
            }
            let w = text_system
                .layout_width(font_id, px(font_size_px), c)
                .as_f32();
            ascii_cache[idx] = Some(w);
            w
        } else if let Some(&w) = other_cache.get(&c) {
            w
        } else {
            let w = text_system
                .layout_width(font_id, px(font_size_px), c)
                .as_f32();
            other_cache.insert(c, w);
            w
        }
    }
}

/// `SERIF_CHAR_ADVANCE_RATIO`'s automated equivalent for an imported font:
/// real-measures (via `char_width_fn`, the same glyph shaping GPUI paints
/// with) the mean advance of a-z/0-9 at `FONT_SIZE_PX`, then expresses it as
/// a fraction of font size — same derivation `SERIF_CHAR_ADVANCE_RATIO`'s own
/// doc comment describes doing by hand for one font, just computed at import
/// time so it works for whatever family the user brings in. `family` must
/// already be registered with `cx.text_system().add_fonts` (font_import.rs
/// calls this immediately after doing so) or GPUI's fallback stack measures
/// instead, giving a meaningless ratio.
pub(crate) fn measure_advance_ratio(cx: &App, family: &str) -> f32 {
    const SAMPLE: &str = "abcdefghijklmnopqrstuvwxyz0123456789";
    let mut measure = char_width_fn(cx, font(family.to_string()), FONT_SIZE_PX);
    let total: f32 = SAMPLE.chars().map(&mut measure).sum();
    total / (SAMPLE.chars().count() as f32 * FONT_SIZE_PX)
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
    // Mono/serif are pre-seeded (as before this map existed) so the common
    // case never pays a HashMap lookup to insert; an imported font's
    // measurement closure is built lazily, the first time a run actually
    // names it, and reused for the rest of this wrap pass.
    let mut measures: HashMap<String, Box<dyn FnMut(char) -> f32>> = HashMap::new();
    measures.insert(
        FONT_FAMILY.to_string(),
        Box::new(char_width_fn(cx, font(FONT_FAMILY), reference_px)),
    );
    measures.insert(
        CURATED_SERIF_FONT.to_string(),
        Box::new(char_width_fn(cx, font(CURATED_SERIF_FONT), reference_px)),
    );

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
        let spans = cached_spans
            .as_ref()
            .map(|(_, s)| s.as_slice())
            .unwrap_or(&[]);
        let para = paragraphs.get(line_idx);
        let size = effective_char_size_px(para, spans, char_idx, normal_size_px, zoom);
        let font_name = effective_char_font(para, spans, char_idx);
        let measure = measures.entry(font_name.to_string()).or_insert_with(|| {
            Box::new(char_width_fn(
                cx,
                font(font_name.into_owned()),
                reference_px,
            ))
        });
        let measured = measure(ch);
        // A glyph's advance scales linearly with font size (true of any
        // font, not just a monospace one — vector outlines scale uniformly
        // with point size), so one real glyph measurement at the reference
        // size covers every size on the row.
        if reference_px > 0.0 {
            measured * (size / reference_px)
        } else {
            measured
        }
    };

    build_visual_rows(lines, usable_wrap_width(viewport_width_px), &mut width_at)
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
    let cur_spans = paragraphs
        .get(cur_line)
        .map(paragraph_run_char_spans)
        .unwrap_or_default();
    let target_x = x_for_col_in_row(
        col_in_row,
        paragraphs.get(cur_line),
        &cur_spans,
        cur_row_start,
        cur_row_end,
        normal_size_px,
        zoom,
    );

    let (target_line, target_row_start, target_row_end) = rows[target_row as usize];
    let target_spans = paragraphs
        .get(target_line)
        .map(paragraph_run_char_spans)
        .unwrap_or_default();
    let target_col_in_row = column_for_x_in_row(
        target_x,
        paragraphs.get(target_line),
        &target_spans,
        target_row_start,
        target_row_end,
        normal_size_px,
        zoom,
    );
    Some((target_line, target_row_start + target_col_in_row))
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
fn extend_auto_scroll_selection(
    state: &Entity<AppState>,
    list_handle: &UniformListScrollHandle,
    position: Point<Pixels>,
    bounds: Bounds<Pixels>,
    scroll_y: f32,
    cx: &mut App,
) {
    // ponytail: uncached full-document rewrap once per animation frame while
    // edge-dragging; thread RowCache through only if this measures as a cost.
    let st = state.read(cx);
    let zoom = st.zoom;
    let font_size_px = st.preferences.normal_text_size_half_points as f32 / 2.0;
    let line_spacing = st.preferences.line_spacing;
    let content = st.active_content().to_string();
    let paragraphs = st
        .workspace
        .tabs
        .get(st.workspace.active_tab)
        .map(|tab| tab.document.paragraphs().to_vec())
        .unwrap_or_default();
    let invisibility = st.ui.invisibility_mode;
    let cite_size = st.preferences.cite_size_half_points;
    let folds = st
        .workspace
        .tabs
        .get(st.workspace.active_tab)
        .map(|tab| tab.folded_headings.clone())
        .unwrap_or_default();

    let lines = document_lines(&content);
    let rows = visual_rows_for_viewport(
        cx,
        &lines,
        bounds.size.width.as_f32(),
        zoom,
        &paragraphs,
        font_size_px,
    );
    let folded_paras = AppState::folded_paragraphs(&paragraphs, &folds);
    let hidden = hidden_wrap_rows(&rows, &paragraphs, invisibility, cite_size, &folded_paras);
    let (display_to_wrap, _) = expand_rows_for_display(
        &rows,
        &paragraphs,
        zoom,
        &hidden,
        font_size_px,
        line_spacing,
    );
    let row_height_px = real_row_height_px(
        list_handle,
        display_to_wrap.len(),
        font_size_px,
        zoom,
        line_spacing,
    );
    let (line, col) = line_col_from_mouse_position(
        position,
        bounds,
        scroll_y,
        &rows,
        &display_to_wrap,
        zoom,
        font_size_px,
        &paragraphs,
        row_height_px,
    );
    state.update(cx, |state, cx| {
        state.extend_selection_to_line_col(line, col);
        cx.notify();
    });
}

pub(crate) fn real_row_height_px(
    handle: &UniformListScrollHandle,
    item_count: usize,
    font_size_px: f32,
    zoom: f32,
    line_spacing: f32,
) -> f32 {
    if item_count == 0 {
        return row_slot_px(font_size_px, line_spacing, zoom);
    }
    handle
        .0
        .borrow()
        .last_item_size
        .map(|s| s.contents.height.as_f32() / item_count as f32)
        .filter(|h| *h > 0.0)
        .unwrap_or_else(|| row_slot_px(font_size_px, line_spacing, zoom))
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
    let local_y = position.y.as_f32()
        - content_bounds.origin.y.as_f32()
        - CONTENT_PADDING_PX
        - scroll_offset_y;
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
    let text_width = x_for_col_in_row(
        row_end - row_start,
        para,
        &spans,
        row_start,
        row_end,
        font_size_px,
        zoom,
    );
    let avail_width = usable_wrap_width(content_bounds.size.width.as_f32());
    let indent = match para.map(|p| p.alignment) {
        Some(Alignment::Center) => ((avail_width - text_width) / 2.0).max(0.0),
        Some(Alignment::Right) => (avail_width - text_width).max(0.0),
        // Left is flush already; Justify only stretches inter-word gaps
        // (render()'s own comment calls it an approximation), which doesn't
        // move the row's first character, so no indent applies there either.
        _ => 0.0,
    };
    // A list paragraph's *first* row (`row_start == 0`, matching where the
    // marker gutter actually renders — see `LIST_GUTTER_PX`'s own doc
    // comment on wrapped continuation rows) eats into the row the same way
    // an alignment indent does: text starts `LIST_GUTTER_PX * (level + 1) *
    // zoom` further right than the row's raw edge — same per-level widening
    // the renderer itself uses (Phase 2 multi-level indent).
    let gutter = match para.and_then(|p| p.list) {
        Some(item) if row_start == 0 => LIST_GUTTER_PX * (item.level as f32 + 1.0) * zoom,
        _ => 0.0,
    };
    let col_in_row = column_for_x_in_row(
        local_x - indent - gutter,
        para,
        &spans,
        row_start,
        row_end,
        font_size_px,
        zoom,
    );
    let col = row_start + col_in_row.min(row_end - row_start);
    (logical_line, col)
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

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
