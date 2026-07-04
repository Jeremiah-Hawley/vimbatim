# Vim Functionality — Spec Sheet & Completion Todo

This document narrows the full spec in `notes/editor_instructions.md` (section
5, plus the parts of sections 3–4 it depends on) down to exactly what's left
to build for Vim support, checked against the actual state of the code as of
commit `7524198` on `staging`. Read `notes/editor_instructions.md` section 9
(Implementation Notes and Constraints) before starting — those rules
(commenting standard, GPUI-only, tab identity by `id`, UTF-8 cursor safety,
commit message format) apply to everything below and are not repeated in
full here.

Rich text display (section 6) and formatting-ribbon operations (section 7)
are **out of scope** for this document — vim edits operate on the flat
`tab.content` string only (spec 3.2), so vim work does not require the
paragraph/run rendering pipeline to be finished first.

---

## 1. Current State (verified against source, not assumed)

| Area | File | State |
|------|------|-------|
| `Tab.cursor: usize`, `Tab.selection: Option<(usize,usize)>` | `src/state.rs:19-23` | Exists |
| `Tab.vim_mode`, `Tab.vim_command_buf` | `src/state.rs` | **Missing entirely** |
| `VimMode` enum | `src/state.rs` | **Missing entirely** |
| Arrow key / Home / End / word-jump cursor movement | `src/text_editor.rs` | **Missing** — `handle_key_down` only handles backspace/enter/space/tab/single printable chars (`src/text_editor.rs:84-105`); Left/Right/Up/Down/Home/End are not matched at all, so they currently no-op |
| Visible cursor tied to `tab.cursor` | `src/text_editor.rs:183-187` | **Fake** — renders a static `"_"` appended to the last line whenever focused; ignores `tab.cursor`'s actual value and line/column |
| Click-to-position cursor | `src/text_editor.rs` | **Missing** |
| Selection extend via Shift+arrow / drag | `src/text_editor.rs` | **Missing** (the data field exists, nothing sets it via keyboard/mouse) |
| Selection rendering (highlight) | `src/text_editor.rs` | **Missing** |
| Copy / Cut / Paste | `src/text_editor.rs:46-82`, `src/state.rs:319-360` | **Done** for Ctrl+C/X/V using GPUI clipboard |
| Undo / Redo | anywhere | **Missing entirely** — no undo stack field, no Ctrl+Z/Y handling |
| Find / Replace | anywhere | **Missing entirely** |
| Vim mode switching (i/I/a/A/o/O/v/V/:/Escape) | anywhere | **Missing entirely** |
| Vim motions (h/j/k/l/w/b/e/0/^/$/gg/G/f/t/…) | anywhere | **Missing entirely** |
| Vim operators (d/y/c/>/</gU/gu) + text objects | anywhere | **Missing entirely** |
| Vim command mode (`:w`, `:q`, `:%s/…`, …) | anywhere | **Missing entirely** |
| Vim registers | anywhere | **Missing entirely** |
| `settings.conf` → `Settings` parsing | `config_parsing/config_parsing.rs` | **Done**, has `vim: bool` field, tested in `tests/parse_testing.rs` |
| `config_parsing` wired into the binary | `src/main.rs`, `src/state.rs` | **Not wired at all.** `config_parsing` is only reachable as `vimbatim::config_parsing` via the `[lib]` shim in `src/lib.rs` for the test crate. `main.rs` never calls `mod config_parsing` or reads `settings.conf`. `AppState::new()` (`src/state.rs:118-139`) hardcodes tab/mode defaults and never sees `Settings.vim`. |

**Implication:** cursor movement, click-positioning, and a real cursor
renderer do not exist yet. Vim motions are the same underlying operation
(move `tab.cursor`) as plain-editor arrow keys, so build the movement
primitives once and drive them from both the non-vim key handler and the
vim Normal-mode motion table. Do not implement vim motions against a cursor
system that still only advances by insertion.

---

## 2. Data Model Additions

All in `src/state.rs`.

### 2.1 `VimMode` enum

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
}

impl Default for VimMode {
    fn default() -> Self { VimMode::Normal }
}
```

### 2.2 `Tab` fields

Add to the `Tab` struct (`src/state.rs:8-24`) and to every place that
constructs one — `Tab::new_empty` (`:27`), `Tab::from_path` (`:44`), and the
test helper `make_state` (`:442-461`):

```rust
pub vim_mode: VimMode,
pub vim_command_buf: String,   // accumulates counts, `:`-commands, and
                                // pending operator/text-object/f-t-char state
pub undo_stack: Vec<String>,   // snapshots of `content`; cap at 200 (spec 4.5)
pub redo_stack: Vec<String>,
```

Also add, needed by the register system (spec 5.8) and `.`-repeat (spec
5.5):

```rust
pub last_find: Option<(char, char)>, // (f/F/t/T variant, target char) for `;`/`,`
```

Registers and the last-change-for-`.` are **not** per-tab (real vim shares
them across buffers) — put them on `AppState` instead:

```rust
pub registers: std::collections::HashMap<char, String>, // 'a'..'z', '"', '0'
pub last_change: Option<VimChange>, // whatever representation `.` needs to replay
```

`VimChange` isn't specified further upstream — design it when you implement
`.` (task 8 below); a reasonable shape is an enum mirroring the operator +
count + motion/text-object that produced the last edit, since re-running
insertion text verbatim (for `i`/`a`/`c` changes) also has to be captured.

### 2.3 Wiring `settings.conf`'s `vim` flag into startup mode

This is a real gap, not covered by the priority list in
`editor_instructions.md` §10 because that list assumes config parsing is
already connected — it isn't (see table above).

1. In `src/main.rs`, add `mod config_parsing;` is wrong — `config_parsing`
   lives outside `src/` and is currently only exposed via the `#[path]` shim
   in `src/lib.rs`. Either:
   - (a) give the binary the same shim (`#[path = "../config_parsing/config_parsing.rs"] mod config_parsing;` in `main.rs`), or
   - (b) have `main.rs` depend on the `vimbatim` lib target (`use vimbatim::config_parsing;`) the way `tests/parse_testing.rs` already does.
   Prefer (b) — it reuses the existing `[lib]`-shimmed module instead of
   compiling the file twice under two different module paths.
2. Call `config_parsing::parse("settings.conf")` once at startup (in
   `main()` before `cx.open_window`, or inside `AppState::new()`) and thread
   the resulting `vim: bool` into `AppState::new()`.
3. `AppState::new()` (`src/state.rs:118-139`) must set the first tab's
   `vim_mode` to `VimMode::Normal` when `settings.vim == true`, or leave vim
   behavior disabled (i.e. the editor behaves as a plain text editor,
   ignoring the vim motion/operator tables) when `false`.
4. Settings file path: hardcode `"settings.conf"` resolved relative to
   `AppState.working_directory`, matching how `tests/parse_testing.rs`
   invokes it relative to the crate root. Do not add a settings-file-missing
   error path beyond what `config_parsing::Settings::parse` already does
   (it currently `.expect()`s on a missing file — leave that as-is unless
   you're asked to change it, it's out of scope here).
5. `:set vim` / `:set novim` (spec 5.7) must flip a *runtime* vim-enabled
   flag independent of the tab's `vim_mode` — add `pub vim_enabled: bool` to
   `AppState`, seeded from `settings.vim`, and gate all vim key handling on
   it. `vim_mode` continues to track Normal/Insert/etc. only while
   `vim_enabled` is true.

---

## 3. Key Handling Architecture

Everything currently funnels through `TextEditor::handle_key_down` in
`src/text_editor.rs:34-105`. That function will grow a lot; keep it thin and
move dispatch logic into `AppState` methods (following the existing
pattern where `insert_char`/`backspace`/`delete_selection` already live on
`AppState`, not the view) so `state.rs` stays unit-testable without a GPUI
context, matching the existing `#[cfg(test)] mod tests` block at
`src/state.rs:435-532`.

Recommended shape:

```rust
// state.rs
impl AppState {
    pub fn handle_vim_key(&mut self, key: &str, shift: bool, ctrl: bool) -> bool {
        // returns true if the key was consumed by vim handling
    }
}
```

`TextEditor::handle_key_down` should check `state.vim_enabled &&
tab.vim_mode != VimMode::Insert` (or always for Command/Normal/Visual) and
route to `handle_vim_key` first; if it returns `false` (or vim is disabled,
or mode is Insert), fall through to the existing insertion/backspace
handling.

**Watch the existing Ctrl-block early return.** The Ctrl/Cmd branch at
`src/text_editor.rs:46-82` runs *before* anything else in
`handle_key_down` and unconditionally `return`s at line 81, with unhandled
keys falling into `_ => {}` and still returning. Vim needs several
Ctrl-chords this block currently swallows silently: `Ctrl+r` (redo),
`Ctrl+d`/`Ctrl+u`/`Ctrl+f`/`Ctrl+b` (scroll), `Ctrl+o`/`Ctrl+i` (jump list),
`Ctrl+[` (Escape alias). Adding these only inside `handle_vim_key` will not
work — they'll never be reached. Restructure the `_ => {}` arm (or the
block's entry condition) to call into vim handling when `state.vim_enabled`
before falling through to the unconditional `return`.

**Verified GPUI key strings** (from the vendored `gpui` crate at
`~/.cargo/git/checkouts/zed-*/*/crates/gpui/src/platform/keystroke.rs`,
since these aren't documented in `editor_instructions.md`): `"left"`,
`"right"`, `"up"`, `"down"`, `"home"`, `"end"`, `"pageup"`, `"pagedown"`,
`"delete"`, `"escape"`, `"backspace"` — lowercase, matching the convention
already used for `"enter"`/`"space"`/`"tab"` in the current code
(`src/text_editor.rs:87-90`). Single printable characters arrive as
lowercase with `event.keystroke.modifiers.shift` set separately (see the
existing uppercase-on-shift handling at `src/text_editor.rs:91-100`) — vim
key matching (e.g. distinguishing `w`/`W`, `f`/`F`) must read the shift
modifier the same way, not expect an uppercase character in `key`.

---

## 4. Task Breakdown (build in this order — each phase is independently testable)

### Task A — Cursor movement primitives (prerequisite, not vim-specific)

Files: `src/state.rs`, `src/text_editor.rs`

- Add `AppState` methods: `move_left`, `move_right`, `move_up`, `move_down`,
  `move_line_start`, `move_line_first_nonblank`, `move_line_end`,
  `move_word_forward`, `move_word_end`, `move_word_backward`,
  `move_doc_start`, `move_doc_end`, `move_to_line(n: usize)`. Every one must
  clamp to a valid UTF-8 boundary via `content.is_char_boundary(offset)` or
  `char_indices()` (spec 9.4) — never `+=1`/`-=1` on the byte offset
  directly for multi-byte-safe movement.
- Wire `"left"`/`"right"`/`"up"`/`"down"`/`"home"`/`"end"` plus
  `Ctrl+Left`/`Ctrl+Right` (word) and `Ctrl+Home`/`Ctrl+End` (doc) in
  `handle_key_down` for the non-vim path (spec 4.1).
- Replace the fake cursor renderer (`src/text_editor.rs:183-187`, the
  `"{}_"` append) with one that maps `tab.cursor` (byte offset) to a
  `(line_index, column_within_line)` pair against the same `lines` split
  already computed at `src/text_editor.rs:132-136`, and renders a real
  marker at that position — not just the end of the last line.
- Click-to-position: **done**, but with two known limitations recorded in
  `tmp_documentation.md`'s "Task A" section — (1) column math uses an
  estimated monospace char width, not real glyph shaping, and (2) it
  currently ignores scroll offset: `content_bounds` is captured from the
  `.overflow_y_scroll()` viewport, so after scrolling down N lines a click
  resolves N lines too high. Fixing (2) requires reading the scroll offset
  for the `.id("text-editor")` element out of GPUI's scroll-handle state and
  adding it to `local_y` before calling `line_for_y` in
  `src/text_editor.rs`. Worth fixing before or alongside Task B, since
  Task B's selection-drag and selection-render work touches the same
  scrollable area and will have the identical scroll-offset bug if not
  accounted for up front.

### Task B — Selection extend + render (prerequisite)

Files: `src/text_editor.rs`

- Shift+arrow variants set/extend `tab.selection` using the movement
  primitives from Task A (anchor = old cursor if no selection yet, focus =
  new cursor). **Done** — `extend_left`/`right`/`up`/`down`/`word_forward`/
  `word_backward`/`line_start`/`line_end`/`doc_start`/`doc_end` in
  `src/state.rs`, wired to Shift+Left/Right/Up/Down/Home/End and
  Shift+Ctrl+Left/Right/Home/End in `src/text_editor.rs`.
- `Ctrl+A` selects all (spec 4.3). **Done** — `AppState::select_all`.
- Render selection as a background overlay per spec 6.4's color
  (`#264F78`) even though full rich-text rendering (section 6) is out of
  scope — this is just a positional overlay on the current plain-text line
  divs. **Done** — `render_line`/`line_segments`/`selection_span_for_line`
  in `src/text_editor.rs`, using `rgba(0x264F7880)`.
- Spec 4.3 also lists "Mouse click-drag creates a selection," which this
  bullet list omitted when it was originally written (an oversight, not a
  deliberate cut). **Done as a follow-up** —
  `AppState::extend_selection_to_line_col` (+ the shared
  `byte_offset_for_line_col` it and `set_cursor_from_line_col` both call)
  in `src/state.rs`; `on_mouse_move` + the shared
  `line_col_from_mouse_position` helper in `src/text_editor.rs`. No
  explicit drag-state field — the drag naturally starts/stops via GPUI's
  `MouseMoveEvent::dragging()` and `extend_selection`'s existing
  anchor-fallback logic.
  - **New known limitation:** a drag that starts outside the editor (e.g.
    in the sidebar) and moves into it will still extend a selection, since
    there's no "did this drag start here" flag. Worth adding if it turns
    out to matter.
  - **Manually verified working** (by the user, on a real display) at
    scroll offset 0 — cursor movement, click-to-position, click-drag
    selection, and the selection overlay all confirmed correct.
- **Auto-scroll while dragging: done, including continuing while the drag
  holds still at an edge — but unverified in the one way that matters.**
  `auto_scroll_delta`/`clamp_scroll_offset` now live in their own dedicated
  module, `src/auto_scroll.rs` (per explicit instruction), alongside a new
  `AutoScroller` struct that self-reschedules via `Window::on_next_frame`
  so scrolling continues independent of further mouse-move events — the
  "holds still at the edge" gap noted below is now closed. `AutoScroller`
  is wired into `TextEditor` (`src/text_editor.rs`): `notify()` from
  `on_mouse_move`, `stop()` from both `on_mouse_up` and `on_mouse_up_out`
  (covering a release outside the editor's bounds, which would otherwise
  leave the tick loop running forever).

  This required changing `line_col_from_mouse_position` (used by *both*
  click and drag) to subtract the scroll offset from the mouse position —
  necessary for auto-scroll to behave sensibly at all, but it rests on an
  **assumption that was never tested**: that `content_bounds` (the Task A
  `canvas()` capture) stays pinned to the viewport as the document scrolls,
  rather than moving with the scrolled content. The user's manual test of
  Task A/B happened to run entirely at scroll offset 0, where this
  subtraction is a no-op either way — auto-scroll is actually the *first*
  thing in this codebase that runs at a non-zero offset by construction, so
  it's the first real test of that assumption, not an independent risk
  stacked on a confirmed-working base.

  **Required next test, not optional — and how it fails is diagnostic:**
  scroll to the middle of a long document, click-drag from mid-screen
  downward past the bottom edge, and hold still.
  - Selection lands wrong immediately, before even reaching the edge → the
    coordinate math itself is wrong (switch to sourcing the viewport from
    `scroll_handle.bounds()` instead of the canvas capture — `ScrollHandle`
    guarantees `.bounds()`/`.offset()` share a coordinate system, removing
    the ambiguity; don't make this change speculatively before the test
    shows it's needed).
  - **Scrolling up works but down does nothing** → almost certainly the
    `max_offset().y` sign assumption: `clamp_scroll_offset`'s
    `-max_offset_y.max(0.0)` collapses to a zero-width range if GPUI
    reports that value as zero/negative, which would silently pin downward
    scrolling while leaving upward scrolling unaffected. This is the most
    likely concrete failure — test the downward edge specifically, not
    just any edge.
  - Selection visibly lags a frame behind the scroll → benign, ignore.
  - Everything above works → the frame-pump, coordinate math, and
    `max_offset` sign are all confirmed at once by this one gesture.

  See `tmp_documentation.md`'s "Continuous Auto-Scroll" section for the
  full writeup.
  - **Known limitation, unchanged:** a drag starting outside the editor
    still extends a selection once it enters (no drag-origin flag) — not
    addressed by this pass.

### Task C — Undo/redo (prerequisite) — **Done**

Files: `src/state.rs`, `src/text_editor.rs`. See `tmp_documentation.md`'s
"Task C: Undo/Redo" section for the full writeup; 24 new tests, all passing.

- Add `undo_stack`/`redo_stack` per Tab (section 2.2 above). **Done** —
  plus a `last_edit_at: Option<Instant>` field (needed by the coalescing
  bullet below but not explicitly listed in section 2.2).
- Push `content.clone()` before each edit op (`insert_char`, `insert_str`,
  `backspace`, `delete_selection`), batched within a 300ms window per spec
  4.5 — track last-edit `Instant` per tab to decide whether to push a new
  snapshot or coalesce into the top-of-stack one. **Done** — private
  `push_undo_snapshot()`; no-op edits (backspace at document start,
  delete_selection with nothing selected, insert_str("")) correctly push
  nothing, so Ctrl+Z can't land on an empty step.
- `undo()`/`redo()` pop/push between the two stacks, cap at 200 entries.
  **Done** — only `content` is snapshotted (no cursor), so both clamp the
  cursor into the restored content's bounds/char-boundaries via a new
  `clamp_to_char_boundary` free function rather than restoring its exact
  pre-edit position.
- Wire `Ctrl+Z` / `Ctrl+Y` / `Ctrl+Shift+Z`. **Done** — in the existing
  Ctrl-modifier branch of `handle_key_down`, alongside copy/cut/paste.
- This also directly implements vim's `u` / `Ctrl+r` (spec 5.5), so do this
  before vim Normal-mode commands (Task F).

### Task D — Vim mode switching + indicator — **Done**

Files: `src/state.rs`, `src/text_editor.rs`. See `tmp_documentation.md`'s
"Task D: Vim Mode Switching + Indicator" section for the full writeup; 34 new
tests, all passing.

- Implement the mode-entry/exit table exactly as in
  `notes/editor_instructions.md` §5.1 (`i`/`I`/`a`/`A`/`o`/`O`/`v`/`V`/`:`/`Escape`).
  `o`/`O` insert a new line and must reuse the undo-stack push from Task C.
  **Done** — `VimMode` enum + `vim_mode`/`vim_command_buf` on `Tab`,
  `vim_enabled: bool` on `AppState` (hardcoded `true`; §2.3's config-wiring
  gap is still open, unaddressed by this task). Ten `vim_enter_*`/
  `vim_open_line_*` methods plus `handle_vim_key` dispatch, all in
  `src/state.rs`, unit-tested directly.
- Command mode's keystroke-accumulation into `vim_command_buf` was
  deliberately **not** implemented here — Task D only does the entry/exit
  transition (`:` in, Escape/Enter out with nothing executed). Reason:
  accumulating typed characters correctly requires distinguishing shifted
  punctuation (`%`, `/`, etc.), which is Task H's problem to solve properly
  (see the `key_char` note below); a partial buffer that mangles those
  characters would look done while being subtly broken. `vim_command_buf`
  itself is still added now since later tasks (E's count prefixes, H's
  command text) need the field to exist.
- **GPUI key-reporting finding:** `Keystroke.key` reports the *unshifted*
  base glyph printed on the physical key (`";"` for the semicolon key
  whether or not shift is held) — the shifted character, when GPUI supplies
  one, comes separately via `Keystroke.key_char: Option<String>`. Untested
  against a real keyboard which of the two vim's `:` detection should trust,
  so `handle_vim_normal_key` matches *either*: `key == ";" && shift` OR
  `key_char == Some(":")`. This is also the first place this codebase reads
  `key_char` at all — the existing plain-text insertion arm in
  `handle_key_down` only handles alphabetic shift-to-uppercase and has no
  shifted-punctuation mapping, a pre-existing gap (not introduced by this
  task) that means typing `%`, `!`, `@`, etc. into document content doesn't
  work correctly anywhere in the app yet. Worth fixing generally before
  Task H needs full command-text fidelity (`:%s/foo/bar/g` requires a
  correct `%`).
- Normal mode lets navigation keys (arrows, Home, End) fall through to the
  plain-editor cursor movement rather than swallowing them — a deliberate
  choice so the editor stays usable for moving around before Task E's real
  vim motions land, safe because Normal mode has no active selection for a
  plain move to corrupt. Visual/VisualLine/Command swallow everything except
  their own listed exit keys, since letting navigation fall through in
  Visual mode would clear the selection via `move_left`/etc.'s
  selection-clearing behavior instead of extending it.
- Render a mode indicator. There is no existing status-bar component — add
  one small `div` at the bottom of the editor's own render tree in
  `src/text_editor.rs` (inside the outer `div()` built at
  `src/text_editor.rs:138`, after the line children), showing `-- INSERT --`
  / `-- VISUAL --` / `-- VISUAL LINE --` / `-- COMMAND --` / nothing for
  Normal, per spec 5.1. Don't build a separate status-bar view/file for this
  — it's a few lines of conditional text, not a new component. **Done, with
  one deviation from the literal instruction:** the indicator is a *sibling*
  below the scrollable editor div, not nested inside it — nesting it inside
  would have made it scroll with content and shrink/grow
  `scroll_handle.bounds()`/`max_offset()` every time the mode changed (the
  outer div's structure changed since this task list was written, when the
  wrap system didn't exist yet). `render()`'s top-level div is now a
  `flex_col` wrapper holding [scrollable editor div, indicator div] as
  siblings; `scroll_to_cursor` needed no changes since it already reads
  `.bounds()` off the tracked scroll handle, which GPUI recomputes
  automatically for the shrunk viewport.

### Task E — Vim motions (Normal mode) — **Done (both passes)**

Files: `src/state.rs`, `src/text_editor.rs`. See `tmp_documentation.md`'s
"Task E (pass 1 of 2)" and "Task E (pass 2 of 2)" sections for the full
writeups; 290 tests passing as of pass 2's completion (up from 256 at the
end of pass 1).

Pass 2 also picked up several things the user asked for directly, beyond
this task's original scope: a visible command/count buffer next to the
mode indicator, Visual mode actually extending the selection via motions
(the pure-motion half of Task G — see that section below, still open for
Visual-mode *operators*), and `q`/`@` macro recording/replay (not in
`editor_instructions.md` at all — an addition beyond the written spec).
Macros were initially reported as broken on real hardware; root-cause
investigation (post–Task G) found the functionality actually worked — the
only real gap was the mode indicator giving no visual feedback while a
`q`/`@` sequence was pending. See `tmp_documentation.md`'s "Mode-Indicator
Feedback Fix" section for the full story and the fix.

Implement the full motion table in `notes/editor_instructions.md` §5.2,
built on Task A's primitives plus new ones this table needs that Task A
didn't require: `w`/`W`/`b`/`B`/`e`/`E` (word vs WORD — WORD is
whitespace-delimited only, word additionally breaks on punctuation).
**Done.** `{`/`}` (paragraph = blank-line-delimited block). **Done.**
`f`/`F`/`t`/`T` + `;`/`,` repeat (store `last_find` per tab per §2.2).
**Done**, including real vim's repeat-nudge for `t`/`T` (`;` after a `t`
skips the already-adjacent match instead of no-op'ing). `gg`/`G`/`NG`.
**Done** — corrected to vim's real first-non-blank landing (not literal
column 0), composed from `move_to_line` + `move_line_first_nonblank`
rather than a new method. `h`/`l`/`w`/`b`/`e`/`0`/`^`/`$`/count prefixes.
**Done** — full 3-state dispatcher rewrite in `handle_vim_normal_key`
(digit accumulation, pending two-keystroke commands, single-key dispatch).
`j`/`k`. **Done**, but deliberately *not* vim's logical-line semantics —
reuses this app's existing visual-row-aware movement (same as the arrow
keys) so they don't skip a wrapped paragraph's continuation rows, a
conscious UX call for this heavily-wrapping-prose app, flagged in
`tmp_documentation.md`.

`H`/`M`/`L` (viewport-relative — top/middle/bottom of the *visible*
viewport). **Done in pass 2** — `text_editor.rs` resolves the visible row
range from `scroll_handle.offset()`/`.bounds()`, then hands off to a
GPUI-context-free `AppState::vim_move_to_line_first_nonblank`. `_` (first
non-blank of the `[count]`-th line down). **Done in pass 2** — pure text
math, `underscore_motion`.

**Still not implemented:** `Ctrl+D`/`Ctrl+U`/`Ctrl+F`/`Ctrl+B` (half/full
page scroll), `zz`/`zt`/`zb` (scroll without moving cursor line-relative
position) — never requested in either pass, tracked here if wanted later.
`%` (spec 5.2: "future: matching bracket/paren") is out of scope per the
spec itself.

Count prefixes (`3w`, `5j`) accumulate digits into `tab.vim_command_buf`
before the motion key arrives; parse and clear the buffer once a
non-digit key completes the command. **Done** — see `split_vim_command_buf`/
`take_vim_count` in `tmp_documentation.md`. Note one implemented deviation:
count is *not* applied to `$`/`^` (real vim's `2$` means "end of the next
line," not implemented here — `2$` just goes to the end of the current
line).

### Task F — Vim operators + text objects — **Done, unit-tested only (not hardware-verified)**

Files: `src/state.rs`

Implements `notes/editor_instructions.md` §5.3 and §5.4. Full writeup in
`tmp_documentation.md`'s "Task F" section; 345 tests passing (up from 290).
Everything here is unit-tested but was never exercised through a live
GPUI event loop when first written. A post–Task G root-cause
investigation (see `tmp_documentation.md`'s "Mode-Indicator Feedback Fix")
traced GPUI's actual X11 source and confirmed `matches_shifted_symbol` —
which `>>`/`<<` depend on — is sound; the `q`/`@` macro report that
originally motivated the caution here turned out to be a missing UI
feedback issue, not a functional bug. Still worth a real-keystroke pass
since nothing in this task has had one — see Task F's own "Hardware
verification checklist".

Delivered: `d`/`y`/`c` operators + `dd`/`yy`/`cc` doubled-key current-line
forms; `>`/`<` indent/unindent + `>>`/`<<`; `gU`/`gu` case-change
operators; all of §5.4's text objects (`iw`/`aw`, `is`/`as`, `ip`/`ap`,
`i"`/`a"`, `i'`/`a'`, brackets `(`/`[`/`{` and their `a` variants) as pure
free functions returning `Option<(usize, usize)>`. `d`/`c` write to
register `"`; `y` writes to both `"` and `0` (a minimal, write-only slice
of Task H's register design pulled forward — no `"a` prefix syntax, no
`+` clipboard, no `p`/`P` paste yet).

**Deviations from this original plan, decided during implementation**:
count is supported *after* an operator (`d3w`) or *between* a doubled
operator's two keys (`d2d`), but *not before* the operator (`3dd` doesn't
work) — combining both would need multiplying two separate counts,
deliberately deferred rather than reusing `vim_command_buf` for the whole
sequence as originally sketched here (a pending operator is tracked as its
own `Tab.vim_pending_operator`/`vim_pending_text_object_prefix` fields
instead, since `vim_command_buf`'s existing pending-trigger grammar
couldn't cleanly represent "waiting for either a motion, a doubled key, or
an `i`/`a` prefix" without colliding with `f`/`g`'s own pending-trigger
completion). `dj`/`dk`/`d<up>`/`d<down>` (and their `>`/`gU` equivalents)
are a documented gap — `j`/`k` need GPUI viewport context no part of this
system has.

### Task G — Visual / VisualLine mode — **Done, unit-tested only (not hardware-verified)**

Files: `src/state.rs`

Spec §5.6, both halves now implemented. Full writeup in
`tmp_documentation.md`'s "Task G" section; 359 tests passing (up from
345 at the end of Task F).

Motions in this mode extend `tab.selection` instead of moving an
unselected cursor — done as part of Task E pass 2: `handle_vim_key`'s
Normal-mode motion dispatcher (`handle_vim_motion_key`) was made shared
between Normal and Visual/VisualLine via an `extend: bool` parameter,
rather than duplicating the motion table.

Operators (`d`/`x`, `y`, `c`, `>`, `<`, `~`, `gU`, `gu`) act on
`tab.selection` immediately (no pending-sequence state needed, unlike Task
F's Normal-mode operators — the selection already exists) and reuse Task
F's `execute_vim_operator_range` where possible. `o` swaps which end of
the selection the cursor is on. Unit-tested but not exercised through a
live GPUI event loop; `>`/`<`'s `matches_shifted_symbol` dependency was
confirmed sound against GPUI's actual X11 source during the mode-indicator
root-cause investigation (see `tmp_documentation.md`'s "Mode-Indicator
Feedback Fix") — still worth a real-keystroke pass since nothing here has
had one.

**Explicit scope decision, recorded in `notes/editor_instructions.md` §11.1
("Optional Features")**: text objects (`iw`, `i"`, etc.) directly setting
the Visual selection (real vim's `viw`) is not in spec 5.6's table and was
left out of this pass as an optional, not-yet-built fast-follow.

### Task H — Command mode (`:`) + registers — **Done**

Files: `src/state.rs`, `src/text_editor.rs`

Full writeup in `tmp_documentation.md`'s "Task H" section; 392 tests passing
(up from 382 at the end of Task G's `/simplify` pass).

- Command-mode keystrokes append to a new, dedicated `tab.vim_command_line`
  — deliberately *not* `vim_command_buf` (that buffer has its own
  digit+single-trigger-char parser, `split_vim_command_buf`, not built for
  arbitrary text like `%s/foo/bar/g`; same reasoning Task F used to keep
  `vim_pending_operator` separate from it). Characters resolve via
  `vim_find_target_char`, reusing the resolver already proven correct for
  shifted punctuation on this GPUI backend, sidestepping the app-wide
  shift-punctuation gap for this feature. `Enter` dispatches via
  `dispatch_vim_command` then returns to Normal; `Escape` discards;
  `Backspace` deletes the last char or exits to Normal if already empty.
- Every command in spec §5.7 is implemented, including
  `:%s/pattern/replacement/[g][i]` via the `regex` crate (per-line, first
  match unless `g`; `i` flag via `(?i)`). `:noh` is accepted but a
  documented no-op — nothing to clear until Task I's search highlighting
  exists.
- **Scope decision**: `:q`'s spec text says "prompt if unsaved". Real vim
  doesn't pop a confirmation dialog by default — it refuses with an error
  (`E37: No write since last change`) unless `:q!` forces it. Implemented
  that way (via a new `tab.vim_command_error: Option<String>`, shown in the
  mode indicator) rather than building new modal/prompt UI; confirmed with
  the user before implementing.
- `:e <path>`/`:w`/`:wq`/`:x`/`:wa`/`:q`/`:q!` dispatch to the existing
  `open_file`/`save_active_tab`/`close_tab` — `save_active_tab` was split
  into an index-taking `save_tab(idx)` core so `:wa` can loop every tab.
- Registers (§5.8): the `HashMap<char, String>` on `AppState` from Task F is
  now read from as well as written to. `"<letter>`/`"+`/`"0`/`"\"` prefix
  syntax (`tab.vim_pending_register_select` + `vim_selected_register`, a
  one-shot selection consumed by the next `d`/`y`/`c`/`p`/`P`, same pattern
  as the existing macro-register-pending flow) works in both Normal and
  Visual mode. `p`/`P` added, using "does the register text end with `\n`"
  as the linewise-vs-charwise signal — free, since every linewise operator
  range already ends with a trailing newline by construction. `'+'` is
  stored in `registers` like any other named register; `text_editor.rs`
  mirrors it to/from the real OS clipboard around dispatch (peeking
  `vim_selected_register() == Some('+')` before a `p`/`P` to read the
  clipboard in, draining a `pending_clipboard_sync` mailbox after any
  keystroke to push a `"+y`/`"+d`/`"+c` out) — the only two places this
  needed a GPUI `cx`, mirroring the existing Ctrl+C/V pattern.

### Task I — Remaining Normal-mode commands + `.` repeat — **Done**

Files: `src/state.rs`, `src/text_editor.rs`

Full writeup in `tmp_documentation.md`'s "Task I" section; 435 tests
passing (up from 427 at the end of Task H).

Spec §5.5's list minus what earlier tasks already cover (`u`/`Ctrl+r` from
Task C, `>>`/`<<` from Task F, `p`/`P` from Task H's registers). Implemented:
`x`/`X`/`r<char>`/`R`/`s`/`S`/`~`/`.`/`J`/`/`/`?`/`n`/`N`/`*`/`#`/`Ctrl+o`/
`Ctrl+i`.

**`R` (Replace mode) — user decision: added a real `VimMode::Replace`**
variant (overwrite-in-place semantics, `Escape` back to Normal, `--
REPLACE --` indicator), rather than treating it as out of scope. Backspace
moves the cursor back but doesn't restore the overwritten character (a
documented simplification vs. real vim's per-position undo tracking).

**`/`/`?`/`n`/`N`/`*`/`#`**: a new `VimMode::Search`, sharing the exact
text-capture state machine Task H built for `:` (extracted into a shared
`capture_vim_line_input` helper so the two don't duplicate escape/
backspace/char-capture logic) — dispatches to a minimal `content.find`/
`rfind`-with-wraparound (plain substring, not regex, per this section's
original guidance). `*`/`#` extract the word under the cursor via Task F's
`text_object_word` as the literal search text.

**`Ctrl+o`/`Ctrl+i`**: a per-tab jump-list back/forward stack pair
(`vim_jump_back`/`vim_jump_forward: Vec<usize>`, same shape as
`undo_stack`/`redo_stack`), pushed to from the single application point
every Normal-mode motion already goes through (`apply_vim_motion`) whenever
a motion crosses more than one line — covers `gg`/`G`/`:<n>`/searches
automatically without special-casing each call site.

**`.` repeat**: `AppState.last_change: Option<VimChange>`, a semantic enum
(`Operator(char, Vec<RecordedVimKey>)` / `OperatorInsert(..., String)` /
`Insertion(String)`) rather than raw keystroke replay of the *entire*
sequence — re-invokes `start_vim_operator`/`complete_vim_operator` with the
stored completion keystrokes at the *new* cursor position (correctly
re-resolving `dw`-style motions fresh each time), and replays typed text
via `insert_str` for `i`/`a`/`c`-style insertions. `y` never starts a
recording (yanking isn't a "change"). `o`/`O`-opened insertions are
captured as plain text too, but replaying them via `.` inserts inline
rather than reopening a new line — an explicitly out-of-scope
simplification (this section's original guidance only promised `i`/`a`/`c`).

---

## 5. Verification

No automated UI tests are required at this stage (per
`notes/editor_instructions.md` §9.8), but:

- `cargo check` before every commit.
- `cargo test` must keep passing — this exercises `config_parsing`
  (`tests/parse_testing.rs`) and whatever you add to
  `#[cfg(test)] mod tests` in `src/state.rs`. Every free function added in
  Task F (text-object resolvers) and the pure `AppState` motion methods from
  Task A/E are unit-testable the same way `copy_selection`/`cut_selection`/
  `insert_str` already are (`src/state.rs:466-532`) — write tests there as
  you go, not at the end.
- `./run.sh` for manual verification (sets `XCURSOR_SIZE=24` for WSL cursor
  scaling). Manually check each task's key bindings against the tables in
  `notes/editor_instructions.md` §5 before considering a task done.
- Commit format per §9.5: `feat: <description>` / `fix: <description>` /
  `refactor: <description>`, no scope prefix.
- Per `notes/INSTRUCTIONS.md`'s documentation rules (also restated in
  `editor_instructions.md` §9.6): every new function needs a multi-line
  block comment under its signature, non-obvious lines get inline comments,
  and finishing a task means appending a description to
  `tmp_documentation.md`.

## 6. Suggested Branch

Per `editor_instructions.md` §9.7, create a feature branch (`vim-mode`) off
`staging` for this work rather than committing directly to `staging`.
