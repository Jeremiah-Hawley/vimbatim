# Implementation Plan: `uniform_list` Lazy Row Rendering

Follow-up to `performance_plan.md`'s idea #2, bundled with idea #1 (row-table
caching) per review — `uniform_list` alone still needs the full-document wrap
to know its item count, so caching that wrap is what makes this actually pay
off on renders that don't touch the text (scrolling, cursor moves — the most
frequent render triggers in normal use).

Scope: `src/text_editor.rs`'s row rendering, plus a small addition to `Tab`
(`state.rs`) for cache invalidation. No changes to `document_ops.rs`'s
formatting logic, docx parsing/saving, or the sidebar/tab-bar work from
earlier in this session.

---

## Background: what I confirmed about `uniform_list`

Read `crates/gpui/src/elements/uniform_list.rs` in the vendored `gpui` source
directly (not going from memory):

- `gpui::uniform_list(id, item_count, |range: Range<usize>, window, cx| Vec<R>)`
  — GPUI calls the closure only with the currently-visible index range (plus
  a small overscan it manages internally), not the whole list. It measures
  row height once (by rendering `item_to_measure_index`, default `0`) and
  lays out the rest arithmetically — this is what makes it fast for large
  lists, and it's exactly this editor's shape: every row is already meant to
  be a fixed `LINE_HEIGHT_PX` (the render() doc comment at
  `text_editor.rs:628-632` says so explicitly, because click-to-position and
  scroll math already depend on it).
- `UniformListScrollHandle` (its own scroll-handle type, passed to
  `.track_scroll()`) is `Rc<RefCell<UniformListScrollState>>`, and
  `UniformListScrollState.base_handle` is a **plain `pub gpui::ScrollHandle`**
  — the exact same type `TextEditor.scroll_handle` already is
  (`ScrollHandle(Rc<RefCell<ScrollHandleState>>)`, confirmed in
  `elements/div.rs`). This means the existing click/drag/auto-scroll/
  scroll-to-cursor pixel math doesn't need to be rewritten — it needs to read
  its `ScrollHandle` from `uniform_list_scroll_handle.0.borrow().base_handle`
  instead of a separately-constructed one, since both are cheap
  clones of the same underlying Rc-shared state.
- `ListSizingBehavior::default()` is `Auto` ("the list should not calculate a
  fixed size") — matches how the editor already fills its `flex_1()`
  allocation and scrolls internally. No non-default sizing config needed.

---

## Audit results

Traced every mutation call site (see grep list above) to its containing
function:

**Clean — already covered by `push_undo_snapshot` in the same or an ancestor
call, so bumping inside it (as planned) is correct as-is:**
`insert_char`, `backspace`, `delete_selection`/`delete_selection_raw`,
`apply_formatting_to_line` (calls it unconditionally, even on an empty line —
covers the direct `para.heading`/`para.alignment`/`apply_format_op` mutations
in the same function too), `apply_center_alignment_with_selection`,
`apply_line_alignment`, `apply_case_to_selection` (direct `run.text`/
`tab.content` mutation, no `sync_*` call at all — still covered because it
pushes undo before mutating), `condense_selection`, `apply_bullet_list`,
`apply_numbered_list`, `insert_str` (covers `paste_text` by delegation),
`vim_paste_register`, `replace_vim_range` (the vim operator choke point —
covers `d`/`c`/`x`/`s`/`>`/`<`/`gU`/`gu`/`~`/`r`/`J` transitively), the `:%s`
vim substitute command (guarded by an actual-change check, correctly skips
the bump on a no-op substitution), and `apply_card_style` (its direct
`para.heading =` assignment happens *after* it's already called
`apply_formatting_to_line` at least once in the same invocation, which has
already pushed). `open_file`'s direct `tab.content =`/`tab.paragraphs =` is
safe by construction, not by audit — it's initializing a brand-new tab before
it's pushed into `self.tabs`, so there's no existing cache entry for that
`tab_id` to go stale; the first render is always a cache miss regardless.

**Found a real gap:** `apply_formatting_to_selection`'s no-selection branch
(`state.rs`, the `None` arm — formats the single character under the cursor
and arms `pending_format`) calls `apply_formatting` directly and **never
calls `push_undo_snapshot` anywhere in that branch.** This isn't just a
caching problem — it's a pre-existing, currently-shipping undo bug: pressing
a formatting hotkey (Ctrl+B, the ribbon's Bold button, etc.) with the cursor
sitting on a character but *no active selection* mutates the document and
**cannot be undone with Ctrl+Z**. `cycle_font_size`, `apply_font_color`, and
the highlight/strikethrough cyclers all delegate to
`apply_formatting_to_selection`, so they inherit the same gap whenever
there's no selection.

This was invisible until now because the only existing coverage,
`test_apply_formatting_to_selection_is_undoable`, only exercises the
*with-selection* path (`state.tabs[0].selection = Some((0, 5))`) — there's no
equivalent test for the cursor-only case. The gap and the missing test are
the same root cause.

**Recommended fix, one line:** add `self.push_undo_snapshot();` at the top of
the `None` branch, mirroring every other formatting entry point. This fixes
the real undo bug *and* closes the caching gap in the same change — it should
land as its own small fix before (or as part of) step 1 below, with a new
test for the no-selection undo case sitting next to the existing
with-selection one.

---

## Part 1: Row-table caching (prerequisite)

### The correctness-critical piece: cache invalidation

Add `pub content_version: u64` to `Tab` (`state.rs`), starting at `0`.

**Bump it unconditionally at the very top of `push_undo_snapshot()`
(`state.rs:814`), before its 300ms coalescing check.** This matters: that
function is already this codebase's de facto "a real content mutation is
about to happen" choke point — nearly every mutating `AppState` method calls
it first (`insert_char`, `backspace`, `delete_selection`,
`apply_formatting_to_line`, `apply_formatting_to_selection`,
`apply_line_alignment`, etc.). But it *skips* pushing an actual undo entry
when called again within the 300ms coalescing window — if the version bump
were placed after that check instead of before it, a fast typing burst would
leave the cache serving stale wrapped text for every keystroke inside the
window except the first. Bumping unconditionally at the top avoids this.

Also bump it in `undo()` and `redo()` (`state.rs`, ~1609/1634) — both replace
`tab.content`/`tab.paragraphs` wholesale via `mem::replace`, bypassing
`push_undo_snapshot` entirely.

**Audit completed** — every call site of `sync_insert_char`, `sync_insert_str`,
`sync_delete_range`, `apply_formatting`, `apply_paragraph_alignment`,
`apply_format_op`, plus every direct `tab.content =` / `tab.paragraphs =` /
`para.heading =` / `para.alignment =` / `para.runs =` / `run.text =`
assignment in `state.rs`, traced to its containing function and checked
against `push_undo_snapshot`. Full results below ("Audit results").

**Recommended way to keep this verified going forward, not just a one-time
manual pass:** a test that calls every public mutating `AppState` method once
each and asserts `content_version` increased — mechanical, catches a future
call site being added without the bump immediately, instead of relying on a
manual audit staying exhaustive as the codebase grows.

### The cache itself

New struct in `text_editor.rs`, held as a `TextEditor` field:

```rust
struct RowCache {
    tab_id: usize,
    content_version: u64,
    viewport_width_bits: u32,  // f32 isn't Eq; compare via to_bits() or an epsilon
    zoom_bits: u32,
    lines: Rc<Vec<String>>,
    line_chars: Rc<Vec<Vec<char>>>,
    line_byte_starts: Rc<Vec<usize>>,
    rows: Rc<Vec<(usize, usize, usize)>>,   // (logical_line_idx, row_start_char, row_end_char)
    paragraphs: Rc<Vec<Paragraph>>,
}
```

`render()` checks the cache against the active tab's current
`(tab_id, content_version, viewport_width, zoom)` at the top; on a match,
reuses the `Rc`s (cheap clone, no re-wrap); on a miss, recomputes exactly
what `render()` does today (`document_lines`, the `Vec<Vec<char>>` collection,
`visual_rows_for_viewport`, `tab.paragraphs.clone()`) and stores the result.
Wrapping everything in `Rc` here isn't just for the cache's own sake — it's
what lets the `uniform_list` closure in Part 2 capture this data cheaply
instead of deep-cloning it into every render's closure.

`content.to_string()` at `text_editor.rs:663` goes away too — `lines`/
`line_chars` are now sourced from the cache, so the full content clone isn't
needed on cache hits either.

---

## Part 2: Swap the row loop for `uniform_list`

### Structural change to the editor div tree

Today (`text_editor.rs:836-919`), the **outer** `"text-editor"` div owns
`.overflow_y_scroll()` + `.track_scroll(&self.scroll_handle)` *and* all the
focus/key/mouse-event handling, with a plain `div().flex_col()` child holding
one div per row.

`uniform_list` needs to own the actual scrolling itself (it sets
`overflow.y: Scroll` internally and has its own `.track_scroll()`) — nesting
it inside another scrollable container would be broken. So:

- Outer `"text-editor"` div: **keeps** `.track_focus()`, `on_key_down`,
  `on_mouse_down`, `on_mouse_move`, `on_mouse_up(_out)`, padding, border,
  focus-ring styling — **loses** `.overflow_y_scroll()` and
  `.track_scroll(&self.scroll_handle)`. Becomes a plain flex container.
- New child: `uniform_list("text-editor-rows", rows.len(), move |range, window, cx| { ... })`
  with `.track_scroll(&self.uniform_list_scroll_handle)`, `.flex_1()`,
  `.min_w_0()`, `.min_h_0()`, `.w_full()` — takes over the slot the old
  `div().flex_col().children(rows...)` occupied.
- The unsupported-file banner and the mode-indicator strip stay exactly
  where they are (outside this div entirely, per the existing structure) —
  unaffected by this change.

### The closure

```rust
let rows = cache.rows.clone();          // Rc clone, cheap
let line_chars = cache.line_chars.clone();
let paragraphs = cache.paragraphs.clone();
let lines = cache.lines.clone();
let line_byte_starts = cache.line_byte_starts.clone();
// selection, cursor_visual_row, cursor_col, zoom: Copy or cheap to clone

uniform_list("text-editor-rows", rows.len(), move |range, _window, _cx| {
    range
        .map(|visual_idx| {
            let (li, row_start, row_end) = rows[visual_idx];
            // identical body to today's per-row closure (text_editor.rs:944-1010):
            // row_text, row_cursor_col, row_selection, row_run_spans, prev_has_box,
            // render_line(...), heading-style wrapper — unchanged logic, just reading
            // from the Rc'd cache instead of the render()-local variables it uses today.
        })
        .collect()
})
```

Everything inside the per-row body (`render_line`, `paragraph_run_char_spans`,
box-merge lookup, cursor/selection clipping) is unchanged — this is a
relocation, not a rewrite of that logic.

### Scroll handle unification

- New `TextEditor` field: `uniform_list_scroll_handle: UniformListScrollHandle`.
- In `TextEditor::new()`: construct it first, then initialize the *existing*
  `scroll_handle: ScrollHandle` field from
  `uniform_list_scroll_handle.0.borrow().base_handle.clone()` instead of
  `ScrollHandle::new()`. Both fields now point at the same Rc-shared state.
- Nothing else that reads `self.scroll_handle` (`scroll_to_cursor`,
  `scroll_to_cursor_centered`, `AutoScroller`, the click/drag pixel math)
  needs to change *in principle* — they keep calling `.bounds()`/`.offset()`/
  `.set_offset()` exactly as today, now reading/writing the same state
  `uniform_list` itself tracks.

**Flagging, not asserting:** whether `ScrollHandle::bounds()` sourced through
a *tracked* `uniform_list` element behaves identically to today's plain
`overflow_y_scroll()` div for every existing consumer (click-to-position,
drag-select, auto-scroll edge detection, scroll-to-cursor centering) is an
assumption about GPUI internals, not something I've run and watched. This
codebase has hit exactly this kind of unverified-GPUI-behavior gap before
(the scroll-offset-in-`content_bounds` question from earlier `TextEditor`
work) — needs real-hardware verification before this is called done, same as
that history.

### Hit-testing reuses the cache

`on_mouse_down` (`text_editor.rs:846`) and `on_mouse_move` (`text_editor.rs:873`)
currently each independently re-clone `content` and re-run
`document_lines` + `visual_rows_for_viewport` over the whole document, just
to hit-test one click or drag tick. Once the cache from Part 1 exists, both
should read `rows`/`line_chars` from it (same tab_id/content_version/width/
zoom check `render()` uses) instead of recomputing — this directly closes
`performance_plan.md`'s "compounding finding" (full-document rewrap on every
mouse-move during a drag).

---

## Sequencing (each step independently verifiable before the next)

0. **Done.** Fixed the `apply_formatting_to_selection` no-selection undo gap:
   `push_undo_snapshot()` now called right before the character-under-cursor
   mutation in the `None` branch (state.rs), guarded by the same
   `cursor < content_len` check so a true no-op (cursor at document end)
   still pushes nothing. Two new tests: undo now correctly reverts a
   no-selection format, and the document-end case stays a true no-op (undo
   stack depth unchanged). 633/633 tests pass, clean build.
1. **Done.** Added `content_version: u64` to `Tab` (all 3 constructors:
   `new_empty`, `from_path`, the `make_state` test helper), bumped at the 3
   identified sites — unconditionally at the top of `push_undo_snapshot()`
   (before its coalescing check), and in `undo()`/`redo()` after their
   empty-stack no-op guards. 4 new tests: bumps on a real edit, bumps on
   *every* keystroke of a coalesced typing burst even though only one undo
   entry gets pushed (the specific behavior the "before the coalescing
   check" placement exists for), does *not* bump on a true no-op
   (backspace at document start), and bumps on both undo and redo.
   637/637 tests pass, clean build. No rendering change yet — purely
   additive, `content_version` isn't read anywhere outside its own tests
   until step 2.
2. **Done.** Added `RowCache`/`row_cache_is_valid` and a `row_cache: Option<RowCache>`
   field on `TextEditor`. `render()` now checks `(tab_id, content_version,
   viewport_width, zoom)` against the cache before doing anything else —
   `content`/`paragraphs` are only cloned, and `document_lines`/word-wrap
   only re-run, on a miss; a hit is 5 cheap `Rc::clone`s. Still uses today's
   `.children(rows.iter().map(...))` loop for actual painting — that's
   step 4. 5 new tests on the pure `row_cache_is_valid` key comparison
   (match, and each of tab_id/content_version/width/zoom independently
   causing a miss). 642/642 tests pass, clean build, app launches and stays
   alive with no panic (same sandbox GPU/EGL limitation as every other GUI
   check this session — could not visually confirm rendering itself).
3. **Done.** Added `bench_diagnostic_row_cache_hit_vs_miss_on_large_heavily_formatted_document`,
   extending the existing bench's pattern with a document actually shaped
   like a heavily-formatted case file — 500 paragraphs × 6 runs each (mixed
   bold/italic/highlight spans within the same line), not one giant single-
   run paragraph. Measured cache MISS (full clone + rewrap, what `render()`
   paid on every frame before step 2) against cache HIT (5 `Rc::clone`s, what
   it pays now): **7.78ms vs. 2.2µs — ~3,470× faster on a hit.** Dropped the
   "scrolled to top vs. scrolled deep" comparison the plan originally
   sketched here: under the caching built so far, the whole document is
   wrapped regardless of scroll position either way — a scroll-position-
   dependent cost only appears once step 4's `uniform_list` limits work to
   the visible range, so that comparison belongs there instead. 643/643
   tests pass, clean build.
4. **Done, but unverified beyond compiling — read this before trusting it.**
   Swapped `.children(rows.iter().map(...))` for
   `uniform_list("text-editor-rows", rows.len(), move |range, _, _| ...)`,
   using the cached `Rc`-wrapped `rows`/`lines`/`line_chars`/
   `line_byte_starts`/`paragraphs` from step 2 as cheap clones into the
   closure. Scroll handles unified as planned: `TextEditor.scroll_handle`
   is now initialized from `uniform_list_scroll_handle.0.borrow().base_handle
   .clone()` instead of `ScrollHandle::new()`, so `AutoScroller`/
   `scroll_to_cursor`/the click-drag pixel math read/write the same
   Rc-shared state `uniform_list` itself tracks — no changes needed to any
   of them. `.overflow_y_scroll()`/`.track_scroll()`/`.p(px(16.0))`/
   `.border_1()`/`.border_color()` all moved from the old outer div onto
   `uniform_list` itself, deliberately — `self.scroll_handle.bounds()`
   needs to keep including the padding inset the same way it always did,
   or the click/scroll pixel math's `CONTENT_PADDING_PX` subtraction would
   silently double-count it. The new-tab placeholder moved from being
   uniform_list's implicit "first item" (no such slot exists) to a plain
   sibling above it, given its own `.p(px(16.0))` since it's no longer
   inside the padded box.
   
   643/643 tests pass, clean build, app launches and stays alive with no
   panic — but **that check exercises none of this.** GPUI never gets far
   enough to create a window in this sandbox (fails at
   `MESA: error: ZINK: failed to choose pdev`, before any view's `render()`
   ever runs), so `render()` — the only place any of this step's code
   executes — has literally never run. This is a materially weaker
   confidence level than every other step in this plan: steps 0-3 either
   ran directly in the test suite or were smoke-tested at a point where
   `render()` genuinely had executed. This step's correctness rests
   entirely on step 6's manual pass, not on anything checked so far.

### Post-step-4 real-hardware bug report and fix

First real usage after step 4 surfaced exactly the kind of gap flagged
above. Two symptoms, confirmed to share one root cause by reading the
vendored `gpui` source directly (`elements/uniform_list.rs`):

- Every line rendered roughly 2x too far apart (not just around edits —
  uniformly, everywhere).
- Auto-scroll and scroll-to-cursor triggered far too late — the cursor
  could sit half a screen below the visible area before scrolling caught up.

**Root cause:** `uniform_list` measures exactly *one* row
(`measure_item`, default index 0, always called with `list_width: None` —
i.e. unconstrained/`MinContent` width) and applies *that single
measurement's height* to *every* row in the whole list uniformly
(`item_top = item_height * item_index` in `prepaint`). The row divs used
`.min_h(px(LINE_HEIGHT_PX * zoom))` — a floor, not a fixed size — so any
row whose content measured taller than `LINE_HEIGHT_PX` under that
unconstrained-width measurement pass (plausible for any wrapped/multi-span
row) poisoned the spacing of the *entire* document. Bug 2 was a direct,
mechanical consequence of bug 1, not a separate issue: `scroll_to_cursor`'s
pixel math (`cursor_top = row_index * LINE_HEIGHT_PX`) assumes exactly
`LINE_HEIGHT_PX` per row — confirmed by reading `scroll_to_cursor`'s own
code — so once the real rendered row height diverged from that assumption,
the estimated cursor position fell further and further behind its true
on-screen position as the user scrolled/typed.

**Fix:** `.min_h(...)` → `.h(px(LINE_HEIGHT_PX * zoom))` on the row div
(`text_editor.rs`). An explicit height is a fixed layout size independent
of content or measurement width, so `measure_item` now always returns
exactly `LINE_HEIGHT_PX * zoom` regardless of which row it happens to
measure. Deliberately did *not* add `.overflow_hidden()` alongside it — a
heading's larger font can still visually overflow this box exactly as it
already could before this fix (a pre-existing, already-documented,
accepted limitation, not something this fix should newly introduce by
clipping heading text).

Could not write an automated test for this — it's a real GPUI layout
behavior (Taffy measurement + positioning) that only exists once a live
window renders, and `TestAppContext`/`#[gpui::test]` aren't available to
this crate (the `gpui` dependency in `Cargo.toml` doesn't enable
`test-support`). 643/643 tests pass and the app still launches without a
panic, but neither of those exercises this fix — **needs the same
real-hardware re-test that caught the bug in the first place.**

A third, separate observation from the same test session: ribbon buttons
still feel laggy on hover. Traced this architecturally (not yet fixed):
GPUI's `.hover()` calls `window.refresh()` on a hover-state change, which
sets the *whole window's* dirty flag (`Window.invalidator`, confirmed in
`elements/div.rs`/`window.rs`) — not scoped to the hovered element's
subtree. This means every ribbon hover re-runs `Render::render()` for
*every* visible view in the window, `TextEditor` included, regardless of
whether the ribbon and the editor have anything to do with each other.
`TextEditor::render()`'s cost is now far smaller than before this plan
(cache hits are ~2µs instead of full-document rewrap), but `uniform_list`'s
`measure_item` still runs on every single frame (twice — once in
`request_layout`, once in `prepaint`), and the row-height bug above may
have also been inflating how many rows `uniform_list` considered "visible"
and built per frame. Whether the residual cost is now small enough not to
matter, or whether `formatting_ribbon.rs`'s own render has an independent
inefficiency never audited in this plan, is unconfirmed — needs
re-verification after the row-height fix before deciding whether it's
worth its own investigation.

**Further real-hardware testing after the `.min_h()` → `.h()` fix above
surfaced a new regression it caused: card-styled lines (Pocket/Hat/Block —
larger font sizes) now visually overlap adjacent lines**, since a fixed
row height can no longer grow to fit oversized text the way `.min_h()`
used to. This is a real architecture mismatch (`uniform_list` requires
uniform row heights; this document model doesn't have them, because of
card styles) with no fix applied yet — full writeup, root cause, and
candidate fix directions moved to **`handoff.md`** rather than duplicated
here, since this is where the next agent should start.

4.5. **Done.** Fixed the card-style row-overlap regression from the
   `.min_h()` → `.h()` fix above (full root cause and options were in
   `handoff.md`; user picked option 1, "multi-slot rows"). Added
   `slot_count_for_paragraph`/`expand_rows_for_display` in
   `text_editor.rs`: an oversized paragraph (card style or heading) now
   reserves `slot_count - 1` blank spacer rows after its own row in
   `uniform_list`'s item list, instead of a bare fixed height it could only
   overflow into the next row's content. `RowCache` caches the expansion;
   `cursor_scroll_geometry`, the render closure's `cursor_visual_row`, and
   `line_col_from_mouse_position` (all 3 call sites: `on_mouse_down`,
   `on_mouse_move`, `AutoScroller`) were all updated to work in this
   "display row" index space rather than the raw wrap-row space, since
   spacer rows shift every pixel-position calculation after them. See
   `handoff.md`'s "Update: the overlap bug below was fixed" section for the
   full writeup. 650/650 tests pass, clean build — **unverified on real
   hardware**, same sandbox limitation as step 4 itself.
5. **Done.** Added `TextEditor::cached_or_fresh_row_tables` — reuses
   `RowCache`'s `rows`/`display_to_wrap`/`wrap_to_display` (cheap
   `Rc::clone`s) when still valid for the current viewport width, falling
   back to a fresh computation only on a genuine miss (e.g. before the
   first render). `on_mouse_down`, `on_mouse_move`, and
   `cursor_scroll_geometry` (called by `scroll_to_cursor` on essentially
   every key event — a bigger win than the mouse handlers alone, since it
   fires far more often) all route through it now. `AutoScroller::tick`
   (`auto_scroll.rs`) deliberately still recomputes locally — it has no
   reference to `TextEditor`'s cache, and only runs once per animation
   frame during an edge-drag rather than on every mouse-move pixel;
   flagged with a `ponytail:` comment rather than silently left unfixed.
   650/650 tests pass, clean build — no new pure logic to unit-test here
   (the cache-hit path is just `Rc::clone`s already covered by
   `row_cache_is_valid`'s tests; the miss-path fallback is the same
   full-document-wrap code `render()`/`cursor_scroll_geometry` always ran,
   unchanged).
6. Manual real-hardware verification (this sandbox can't render a window at
   all, per every prior GUI check this session): typing, scrolling,
   click-to-position, click-drag-select near the top/bottom edges
   (exercises `AutoScroller`), Nav-menu jump-to-heading (exercises
   `scroll_to_cursor_centered`) — on an actual large, heavily-formatted
   document, not just a short test file.

## Testing strategy

Same split this codebase already uses elsewhere (auto-scroll, click-drag):
pure logic gets real unit tests, GPUI glue doesn't because it can't be
spun up here.

- **Tested:** `content_version` bump coverage (step 1's call-site-audit
  test), the cache invalidation-key comparison if extracted as its own pure
  function, and the benchmark from step 3.
- **Not unit-testable, needs manual verification:** the `uniform_list`
  wiring itself, scroll-handle sharing behaving as expected, `measure_item`
  producing exactly `LINE_HEIGHT_PX` (if it doesn't, click-to-position and
  scroll-to-cursor's row-height-based pixel math would drift) — flagging
  this specific one explicitly since it's a silent-drift failure mode, not a
  crash, and easy to miss without deliberately checking it.
