# Performance Investigation: Lag on Large/Heavily-Formatted Documents

Code review only — no changes made. Findings below are traced to specific
functions with line references, not guesses.

## Headline finding: the editor re-does full-document work on every render

`TextEditor::render()` (`src/text_editor.rs:621`) runs on effectively every
interaction — every keystroke, every cursor move, every scroll tick, every
mouse hover that triggers `cx.notify()`. Right now it does this, unconditionally,
**regardless of how much of the document is actually visible on screen**:

1. `text_editor.rs:663` — clones the entire document `content` into a new `String`.
2. `text_editor.rs:667` — clones the entire `paragraphs: Vec<Paragraph>` — every
   `Run` in the document, including every `String` field (`text`, `highlight_color`,
   `font`, `color`).
3. `text_editor.rs:769-770` — splits `content` into *every* logical line and
   collects *every character of every line* into a fresh `Vec<Vec<char>>`.
4. `text_editor.rs:788` (`visual_rows_for_viewport`, `text_editor.rs:1395`) —
   despite the name, this word-wraps **every line in the document**, not just
   the visible ones, into visual rows.
5. `text_editor.rs:943` — `.children(rows.iter().map(...))` builds a real GPUI
   element (via `render_line`) for **every visual row in the document**. Each
   one clips paragraph run-spans (`paragraph_run_char_spans`, proportional to
   how many formatting runs that line has) and does box-merge lookups against
   the previous line.

None of this is scoped to the visible scroll window. A document with 2,000
visual rows pays the same per-render cost whether you're looking at row 1 or
row 1,900. GPUI then has to lay out and paint every one of those row elements
even though the scrollable viewport (`overflow_y_scroll`, `text_editor.rs:913`)
only ever shows ~40-60 of them at once.

**This is why both symptoms line up with the code:** document *length* drives
steps 3-5 linearly, and formatting *density* drives step 5's per-row cost
(more runs per line = more span-clipping and more child elements per row). A
large, heavily-formatted file hits both multipliers at once — exactly the
combination you described.

There's already a partial fix in this exact spot: `char_width_fn` caches glyph
width lookups per unique character (see the comment at
`text_editor.rs:2296-2317`, and the existing
`bench_diagnostic_large_document_per_keystroke_costs` test at
`text_editor.rs:2257`) so the wrap pass itself doesn't re-measure the same
glyph over and over. That helped the *wrap* step's constant factor, but it
doesn't change the fact that the wrap still runs over the whole document, and
it does nothing for the clone or element-construction cost.

## Compounding finding: click/drag hit-testing repeats the same full-document work

`on_mouse_down` (`text_editor.rs:846-861`) and `on_mouse_move`
(`text_editor.rs:873-888`) each independently re-clone `content`, re-run
`document_lines`, and re-run `visual_rows_for_viewport` over the **entire
document** — separately from `render()`'s own copy of the same work.
`on_mouse_move` fires on every pixel of mouse movement during a drag, so a
click-drag text selection in a large document is doing full-document re-wrap
dozens of times per second, on top of the render-triggered cost above. This
is very likely the single most noticeable "laggy while doing anything" moment
a user would hit.

## Secondary finding: undo/redo stack memory

`Tab.undo_stack`/`redo_stack` (`state.rs:156-161`) store up to
`UNDO_STACK_CAP = 200` (`state.rs:17`) full `(String, Vec<Paragraph>)`
snapshots — a complete clone of the document on every non-coalesced edit
(`push_undo_snapshot`, `state.rs:814`, 300ms coalescing window). For a large,
heavily-formatted document this is a real memory multiplier: 200 snapshots of
a multi-MB document is not a hypothetical, it's the actual steady state after
a long editing session on a big file. This is more of a memory-pressure /
long-session-degradation issue than a per-keystroke lag cause, but it's a
concrete, fixable contributor and worth listing separately since the fix
shape is different from the render-path issues above.

## Minor finding: `resolve_position` is a linear scan from document start

`resolve_position` (`document_ops.rs:20`) — called by `sync_insert_char`,
`apply_formatting`, `apply_paragraph_alignment`, etc., i.e. on every single
keystroke and every formatting action — walks paragraphs from index 0,
summing run lengths, until it finds the byte offset it's looking for. This is
O(paragraphs before the cursor) per call. It's cheaper than the render-path
issues above (no cloning, no element construction, just arithmetic over
lengths), but it's paid on every keystroke in addition to everything else,
and it gets worse the further into a large document you're editing.

## Checked, and not a primary concern: `docx_parser.rs` load/save

Skimmed `parse_docx` (`docx_parser.rs:152`) and the save path
(`docx_parser.rs:~700-780`). Parsing uses `quick-xml` (a real streaming
parser, not a naive regex pass), and serialization builds the output with a
single growable `String` + `push_str` calls, not repeated `+`/`format!`
concatenation (which would be the classic O(n²) trap). Nothing here jumped
out as a bottleneck on this read. If file *open* specifically still feels
slow on a huge file after fixing the render path, this would be the next
place to profile — but I wouldn't start here.

---

## Ideas, roughly ordered by leverage vs. risk

### 1. Cache the wrapped-row table across renders (do this first)
Right now steps 3-4 above (line-splitting + full-document wrapping) re-run on
*every* render, even ones triggered by something that didn't change the text
at all (e.g. plain cursor movement, scrolling, focus change). Memoize the row
table in `TextEditor`'s own struct, keyed on a cheap signal (e.g. a
generation counter bumped only when `tab.content` actually changes, plus
viewport width and zoom) — recompute only when one of those actually changed
since the last render. This alone would make cursor-move/scroll/focus
renders on a large document nearly free, without touching the editing model
at all. Low risk: it's a pure caching layer around functions that already
exist and are already unit-tested.

### 2. Virtualize row rendering with GPUI's own `uniform_list`
GPUI ships a primitive built exactly for this:
`gpui::uniform_list(id, item_count, |visible_range, window, cx| { ... })`
(`crates/gpui/src/elements/uniform_list.rs`) — "lazy rendering for a set of
items that are of uniform height... will only render the visible subset of
items." Every row this editor renders is already fixed-height by design (the
render() doc comment at `text_editor.rs:628-632` says as much, specifically
*because* click-to-position and scroll-to-cursor math depend on it) — this is
close to the textbook use case `uniform_list` was built for. Swapping the
`.children(rows.iter().map(...))` loop for `uniform_list` would turn "build
and paint an element for every row in the document" into "build and paint an
element for the ~40-60 rows actually on screen," independent of document
size. This is the single biggest lever on raw render/paint lag.

**Real integration cost, not hand-waved:** this editor has a fair amount of
hand-built scroll/pixel machinery already — `ScrollHandle`, the
click/drag/auto-scroll pixel math (`line_col_from_mouse_position`,
`auto_scroll.rs`), and `scroll_to_cursor`. `uniform_list` has its own
scroll-handle story. Reconciling the two (or replacing the custom scroll
math with `uniform_list`'s) is real work and needs its own design pass, not
a drop-in swap — flagging this now so it isn't underestimated later.

### 3. Route hit-testing through the same cached table
Once (1) exists, `on_mouse_down`/`on_mouse_move` should read the cached row
table instead of independently recomputing it. This removes the
compounding finding above almost for free once the cache exists.

### 4. Shrink undo/redo's memory footprint
Two independent options, either is enough on its own:
- Lower `UNDO_STACK_CAP` (or make it size-aware — cap by total bytes
  snapshotted, not just entry count) for large documents specifically.
- Bigger change: store diffs between snapshots instead of full clones. More
  invasive, more payoff, and not needed unless (1)-(3) don't already make
  the memory picture acceptable.

### 5. Make `resolve_position` sub-linear
Maintain a cached prefix-sum of paragraph lengths (invalidated on edits,
same generation-counter idea as (1)) and binary-search it, instead of
re-summing from paragraph 0 every call. Smallest, lowest-risk item on this
list; worth doing but not where the user-visible lag is coming from.

### 6. The "load a couple pages at a time" idea (real pagination)
This is the big swing, matching how Word/other production word processors
handle huge documents: never hold the whole parsed document in memory at
once, only a window of pages, loading more as the user scrolls and evicting
what scrolls far out of view.

**What it would actually require**, to be concrete about the size of this:
- A chunked/streaming variant of `parse_docx` instead of one whole-file parse.
- A windowed `paragraphs` model in `Tab`, replacing the current "always the
  whole document" assumption — which `AppState`, `document_ops.rs`, undo/redo,
  Find/Find-and-Replace, the Nav outline (`file_explorer.rs`'s heading walk),
  and Wikifi export (`wikifi_export.rs`) **all currently depend on directly**.
  Every one of those would need to either become window-aware or force a full
  load when invoked (defeating some of the purpose).
- Save-path logic to merge an edited window back into the full document
  without having the rest of it in memory, without corrupting formatting
  outside the edited window.

**My honest read:** I would not start here. The evidence above points at the
*render path* re-doing full-document work on every frame — not at memory
pressure from holding the whole parsed document. Even a large debate case
file's text-plus-formatting model realistically runs low tens of MB, which
is nothing for modern RAM; the lag is from re-wrapping and re-painting all of
it every frame, not from it existing in memory. Items 1-3 directly target
the actual measured cost, are far lower risk, and don't touch save/round-trip
correctness — which this codebase just got working end-to-end (per the
recent "docx file editing round trip done" commit). Real pagination would put
that at risk for a problem items 1-3 would likely already solve. I'd only
reach for this if, after 1-3 are in and measured on a real large file, lag is
still unacceptable.

---

## Suggested next step (still just planning)

Before implementing anything, extend the existing diagnostic-benchmark
pattern (`bench_diagnostic_large_document_per_keystroke_costs`,
`text_editor.rs:2257`) with one that measures `render()`'s *actual* end-to-end
cost on a synthetic large, heavily-formatted document (many paragraphs ×
many runs per paragraph, not one giant single-paragraph string like the
existing bench uses) — both scrolled to the top and scrolled deep into the
document. That gives real before/after numbers to validate items 1-3 against,
the same way this codebase already validated the `char_width_fn` cache.

---

## Status (closed-beta gate)

- **1. Row-table caching** — Done (`uniform_list_plan.md` steps 0-3;
  `RowCache`).
- **2. `uniform_list` virtualization** — Done (`uniform_list_plan.md` step
  4, plus the multi-slot fix in step 4.5 for card-style/heading rows that
  step 4 initially broke — see `handoff.md`). Still needs real-hardware
  verification (this sandbox has no GPU/Vulkan driver).
- **3. Route hit-testing through the cache** — Done (`uniform_list_plan.md`
  step 5; `TextEditor::cached_or_fresh_row_tables`, used by
  `on_mouse_down`/`on_mouse_move`/`cursor_scroll_geometry`).
  `AutoScroller::tick` deliberately left uncached — see its own `ponytail:`
  comment in `auto_scroll.rs`.
- **4. Undo/redo memory footprint** — Done. `state.rs`'s
  `undo_stack_cap_for_snapshot_size`/`snapshot_byte_estimate`: the 200-entry
  cap now shrinks for large documents so total undo/redo memory stays
  under a fixed budget (`UNDO_STACK_BYTE_BUDGET`), never below
  `UNDO_STACK_MIN_CAP`.
- **5. Sub-linear `resolve_position`** — Explicitly deferred, not required
  for the closed-beta gate (user decision, 2026-07-12): real but minor
  per-keystroke scan cost, not where user-visible lag comes from per this
  doc's own original assessment. Revisit if profiling on a real large
  document after the beta still shows it as a hot path.
- **6. Real pagination** — Not planned; this doc's own original
  recommendation was not to start here, and nothing since has changed that.
