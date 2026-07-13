# Handoff: Editor Performance Work (uniform_list rollout)

For a new agent picking this up cold. Read this first, then the two plan
docs it points to — don't re-derive context that's already written down.

## What this work stream is

The user reported the app going slow/laggy on large, heavily-formatted
`.docx` files. That turned into two docs, read in this order:

1. **`performance_plan.md`** — the investigation. Root cause: `TextEditor::
   render()` (`src/text_editor.rs`) redid full-document work (clone content,
   clone every paragraph/run, re-split every line, re-wrap every line) on
   *every* render, regardless of how much was actually visible on screen —
   and every keystroke/scroll/cursor-move triggers a render.
2. **`uniform_list_plan.md`** — the fix plan and its execution log. Two
   complementary pieces: (a) cache the wrapped-row table across renders that
   don't change the text, (b) use GPUI's `uniform_list` primitive so only
   the visible rows get built into real elements, not the whole document.
   Steps 0-4 are implemented; **step 4 introduced a real regression, caught
   during real-hardware testing, that is the current blocker** (see below).

Both docs have a running "done/what happened" log per step — that log *is*
the project history for this work. Don't skip it.

## Status as of this handoff

- Steps 0-3 (undo-gap fix, `content_version` cache-invalidation counter,
  `RowCache`, a benchmark proving the cache works — 7.78ms → 2.2µs on a
  cache hit) are done, tested, and — critically — have actually **run**
  (either as unit tests or at a point where `render()` genuinely executed).
- Step 4 (swap the row loop for `gpui::uniform_list`) compiles and passes
  all 643 tests, but **could not be exercised at all in this sandbox** — it
  has no working GPU/Vulkan driver, so no window ever opens and `render()`
  never runs here. Every claim about step 4's correctness rests entirely on
  the user's own real-hardware testing.
- That real-hardware test surfaced two connected bugs (both since fixed) and
  flagged one more still under investigation (ribbon hover lag — see
  `uniform_list_plan.md`'s "Post-step-4 real-hardware bug report" section
  for the full trace; not repeated here).
- Fixing those two bugs (row height forced to a fixed `.h()` instead of
  `.min_h()`) **caused the new regression this handoff exists for.**

## Update: the overlap bug below was fixed (multi-slot rows, option 1)

User picked candidate fix direction 1 ("expand oversized lines into
multiple uniform-height slots") over options 2/3. Implemented in
`src/text_editor.rs`:

- `slot_count_for_paragraph(para, zoom)` — pure function computing how many
  `LINE_HEIGHT_PX` slots a paragraph's line actually needs, from the max
  run-level `FontSize` (card styles) or `heading_font_size_px` (document
  headings), plus a fixed `CARD_BOX_EXTRA_PX` (18px) for Pocket's box
  padding/border. Zoom-aware (the box padding doesn't scale with zoom, so it
  doesn't cancel out of the ratio the way the font-size term does).
- `expand_rows_for_display(rows, paragraphs, zoom)` — expands the wrap-rows
  table into a `uniform_list`-facing "display rows" table: an oversized
  row's own entry stays first, followed by `slot_count - 1` blank spacer
  entries. Returns `(display_to_wrap, wrap_to_display)` for translating
  between the two index spaces.
- `RowCache` now caches both vectors alongside `rows` (same invalidation
  key, no new cache-miss cases).
- The `uniform_list` item count is `display_to_wrap.len()`, not
  `rows.len()`; a `None` slot renders as a plain blank `.h(px(LINE_HEIGHT_PX
  * zoom))` div. Real content rows are unchanged — still a fixed `.h()`
  (not `.min_h()`, which would re-poison `uniform_list`'s single-item
  measurement the way it did before). The oversized content visually spills
  into the reserved blank slot(s) below it via ordinary CSS overflow
  (nothing clips it), which is exactly what those slots exist to make room
  for.
- Every place that turned a wrap-row index into a pixel Y position had to
  move to display-row space too, or it would drift by however many spacer
  rows preceded it: `cursor_visual_row` in `render()`, `cursor_scroll_geometry`
  (feeds `scroll_to_cursor`/`scroll_to_cursor_centered`), and
  `line_col_from_mouse_position` (click-to-position, click-drag-select, and
  `AutoScroller`'s edge-scroll tick — all three call sites updated,
  including the one in `auto_scroll.rs`). Cursor Up/Down
  (`visual_row_for_line_col`/`visual_row_step`) deliberately stayed in
  wrap-row space — spacer rows are invisible to logical cursor movement.
  A click landing on a spacer slot resolves to the real content row above
  it (the oversized line the spacer belongs to), not whatever comes after.

7 new unit tests (`slot_count_for_paragraph`/`expand_rows_for_display`),
650/650 tests pass, clean build. **Still needs the same real-hardware
verification as every other rendering change in this plan** — this sandbox
has no GPU/Vulkan driver, so nothing here has actually been seen on screen.
Specifically worth checking: a Pocket/Hat/Block line no longer overlapping
the line below it, clicking into the blank space below an oversized line
still landing the cursor on that line, and scroll-to-cursor/auto-scroll
still tracking correctly past an oversized line.

## The open bug: card-styled lines now overlap adjacent lines

**Symptom:** Pocket/Hat/Block-styled lines (and any other line using a
larger font via `heading_font_size_px`) now visually overlap the line(s)
below them, wherever they appear in a document.

**Why:** `gpui::uniform_list` requires every item to be the *same* height —
confirmed by reading the vendored `gpui` source
(`elements/uniform_list.rs`): it measures exactly one row and applies that
one measurement's height to every row in the list uniformly
(`item_top = item_height * item_index`). The row-height bug fixed just
before this handoff (`uniform_list_plan.md`'s "Post-step-4" section)
switched each row div from `.min_h(px(LINE_HEIGHT_PX * zoom))` (a floor —
content could grow past it) to `.h(px(LINE_HEIGHT_PX * zoom))` (a fixed
size — content can't grow the box at all, only visually spill past it).

That fix was correct for the *previous* bug (it's what makes `measure_item`
return a consistent value regardless of which row it samples), but it
exposed the real problem underneath: **this document model does not have
uniform-height rows.** A Pocket line renders at `size: 52` (half-points,
≈26pt ≈ 35-40px tall) against a `LINE_HEIGHT_PX` of `20.0` built for plain
14px body text — roughly 2x the box height, sometimes more for Pocket
specifically (Hat/Block are smaller but still oversized relative to body
text). With a *floor* (`.min_h()`), the row's own box grew to fit that
text, so the *next* row was naturally pushed down and nothing overlapped.
With a *fixed* height, the next row is still positioned at exactly
`LINE_HEIGHT_PX * (index + 1)` regardless of what the previous row actually
needed — so the oversized text visually spills into where the next row's
content already sits.

This isn't a narrow edge case: card styles (Pocket/Hat/Block/Tag) are the
structural backbone of the debate case files this app is for. Expect this
to show up in essentially every real document, not just unusual ones.

**Why this wasn't caught by the fix that introduced it:** the fix's own
comment (now stale, should be corrected alongside whatever fixes this)
assumed a heading's overflow would be a minor cosmetic detail — "visually
overflow this box... not clipped since overflow stays visible" — reasoning
that held for a *slightly* larger font, not for card styles' actual font
sizes (2-3x body text), which overflow by whole line-heights' worth, not a
few pixels.

### Candidate fix directions (not evaluated in depth — pick one deliberately, don't default to the first without weighing the others)

1. **Expand oversized lines into multiple uniform-height "slots".** Compute
   how many `LINE_HEIGHT_PX` units a line's font size actually needs
   (`ceil(font_height / LINE_HEIGHT_PX)`), have that line occupy that many
   consecutive entries in the `rows` table (first slot = real content,
   remaining slots = blank spacers reserved so nothing else renders there),
   keep `uniform_list`'s one-height-for-everything intact. Real work: the
   `rows` table stops being a 1:1 map of "visual row" to "wrapped line
   segment" — cursor positioning, click-to-position, and selection math all
   currently assume that 1:1 mapping and would need to account for spacer
   slots.
2. **Make `LINE_HEIGHT_PX` big enough for the largest card style
   (Pocket, ~26pt).** Simplest, zero architecture change — but every line
   of plain body text (the majority of any real document) would get
   Pocket-sized vertical spacing too. Almost certainly an unacceptable
   look, but the cheapest thing to try first if a quick visual check is
   wanted before committing to option 1 or 3.
3. **Drop `uniform_list`, keep the caching (steps 1-3).** Revert just the
   step-4 swap back to a plain `.children(rows.iter().map(...))` loop (which
   naturally supports variable-height rows, the way the pre-step-4 code
   already did correctly), while keeping `RowCache`'s win intact. Gives up
   `uniform_list`'s paint-cost savings (not building/painting off-screen
   rows) — per `uniform_list_plan.md`'s original framing, that was flagged
   as *possibly* the single biggest lever on raw render lag, so this
   trades away a real, if unquantified, amount of the win — but it's a
   small, safe, well-understood diff versus option 1's real complexity.

No recommendation is being made here on purpose — this is a real tradeoff
(architecture complexity vs. performance ceiling vs. implementation risk)
the user should weigh in on, not something to default into silently.

## Things to know before touching this code

- **This sandbox cannot render a GPUI window at all** (no working Vulkan/EGL
  driver — fails at `MESA: error: ZINK: failed to choose pdev` before any
  window opens). Every rendering-level change in this file needs the user's
  own real-hardware testing to actually verify — `cargo build`/`cargo test`
  passing proves compilation and pure-logic correctness only, nothing about
  actual on-screen behavior. Say this explicitly when reporting work, don't
  let a clean test run imply more confidence than it earns.
- `TestAppContext`/`#[gpui::test]` (GPUI's own test-context macro) are not
  available to this crate — `Cargo.toml`'s `gpui` dependency doesn't enable
  the `test-support` feature. Don't reach for them without first checking
  whether enabling that feature is in scope for the task at hand.
- Key files: `src/text_editor.rs` (`RowCache`, `row_cache_is_valid`, the
  `uniform_list` wiring, `scroll_to_cursor`, the row-div construction with
  `LINE_HEIGHT_PX`/`heading_font_size_px`), `src/state.rs`
  (`content_version`, `push_undo_snapshot`), `src/auto_scroll.rs`
  (`AutoScroller`, shares `TextEditor`'s `scroll_handle`).
- Established pattern for GPUI-only-verifiable work this whole session:
  investigate via reading the vendored `gpui` source directly (checked out
  at `~/.cargo/git/checkouts/zed-a70e2ad075855582/<rev>/crates/gpui/src/`)
  rather than guessing at its behavior — that's what found both the
  original `uniform_list` measurement bug and the ribbon-hover full-window-
  refresh behavior. It's real, primary-source evidence, not speculation.
- `uniform_list_plan.md`'s remaining steps (5: route click/drag hit-testing
  through the cache; 6: full manual verification pass) should probably wait
  until the overlap regression above is resolved — testing step 5/6 against
  a visibly broken render would produce noisy, hard-to-interpret results.
