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
mod render;

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
    /// goes through `tab_index` rather than `AppState.workspace().active_tab`, so each one
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
        let line_spacing = state.preferences().line_spacing;
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
        let dragging = state.workspace().split_dragging;
        let invisibility = state.ui().invisibility_mode;
        let cite_size = state.preferences().cite_size_half_points;
        let fold_version = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.fold_version)
            .unwrap_or(0);
        let folds = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.folded_headings.clone())
            .unwrap_or_default();
        let tab_id = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.id.0)
            .unwrap_or(usize::MAX);
        let content_version = idx
            .and_then(|i| state.workspace().tabs.get(i))
            .map(|t| t.document.content_version)
            .unwrap_or(0);
        let zoom = state.zoom;
        let line_spacing = state.preferences().line_spacing;
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
            .and_then(|i| state.workspace().tabs.get(i))
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
        let row_height = row_slot_px(normal_size_px, state.preferences().line_spacing, zoom);
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
            .and_then(|i| state.workspace().tabs.get(i))
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
            .and_then(|i| state.workspace().tabs.get(i))
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
        if self.state.read(cx).ui().editor_context_menu.is_some() {
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
            .workspace()
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
        if self.state.read(cx).ui().read_mode && plain_arrow {
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
                            if s.ui().editor_context_menu.is_some() {
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
    let font_size_px = st.preferences().normal_text_size_half_points as f32 / 2.0;
    let line_spacing = st.preferences().line_spacing;
    let content = st.active_content().to_string();
    let paragraphs = st
        .workspace()
        .tabs
        .get(st.workspace().active_tab)
        .map(|tab| tab.document.paragraphs().to_vec())
        .unwrap_or_default();
    let invisibility = st.ui().invisibility_mode;
    let cite_size = st.preferences().cite_size_half_points;
    let folds = st
        .workspace()
        .tabs
        .get(st.workspace().active_tab)
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
