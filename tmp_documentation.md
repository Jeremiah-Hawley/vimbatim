# Vimbatim GUI — Implementation Notes

## What Was Built

A full GPUI-based desktop application for editing `.docx` files, implemented on the `gui` git branch.
The app uses the **Zed GPUI framework** (`gpui` + `gpui_platform` from the Zed monorepo) for GPU-accelerated UI rendering.

---

## Architecture

All state lives in a single shared model (`Entity<AppState>`) that is created once in
`MainWindow::new()` and passed as a cloned handle to every child view. Because `Entity<T>` is
reference-counted and cheaply cloneable in GPUI, no message-passing or callback plumbing is needed
between views — each view reads and writes the same shared state directly through `entity.read(cx)`
and `entity.update(cx, |state, cx| { ... })`.

### Source Files

| File | Purpose |
|------|---------|
| `src/main.rs` | App entry point. Creates the GPUI Application, registers global key bindings (Ctrl+`,`, Ctrl+B, Ctrl+T, Ctrl+W), opens a 1200x800 window. |
| `src/main_window.rs` | Root view. Composes all child views into the tab-bar -> ribbon -> (editor | sidebar) layout. Handles ToggleSettings and ToggleSidebar actions. |
| `src/state.rs` | AppState model (tabs, active tab, sidebar visibility, settings visibility, working directory, file tree). Also contains Tab, FileNode, and scan_directory. |
| `src/tab_bar.rs` | Tab bar. Renders one button per open tab + a "+" new-tab button. Tabs have a close "x". Clicking a tab switches to it; clicking "x" closes it. |
| `src/formatting_ribbon.rs` | Two-row formatting ribbon below the tab bar. Currently 2x4 stub buttons that print to console on click. Adding new format actions requires only adding entries to button_rows(). |
| `src/text_editor.rs` | Main editor area. Focusable, scrollable, key-event-aware div. Inserts characters into AppState::Tab::content on key-down. Splits content by newline to render multi-line. |
| `src/file_explorer.rs` | Collapsible 240px sidebar. Scans the working directory for .docx files. Clicking a directory toggles expand/collapse; double-clicking a file opens it in a new tab. Has a "+" button to create new blank .docx files. |
| `src/settings_modal.rs` | Floating settings overlay. Renders as an absolute-positioned semi-transparent backdrop with a centred dialog panel. Toggle with Ctrl+,. Contains a demo button that prints to console. |
| `config_parsing/config_parsing.rs` | Pre-existing config parser (unchanged). Parses settings.conf into a Settings struct and a flat HashMap. |

---

## Layout

```
+--------------------------------------------------------+
| [Tab 1] [Tab 2 dot] [+]                               |  <- tab bar (36px)
+--------------------------------------------------------+
| [B] [I] [U] [S]   [=L] [=C] [=R] [*=]               |  <- ribbon (2 rows x 4 buttons)
+----------------------------------+---------------------+
|                                  |  WORKING DIR        |
|  (text editor - flex-1)          |  > subdir/          |
|                                  |  [doc] file.docx    |
|  Hello, World_                   |  [doc] notes.docx   |
|                                  |                     |
+----------------------------------+---------------------+
```

When settings are open, a dimmed backdrop covers everything and a centred dialog appears on top.

---

## Key Bindings

| Key | Action |
|-----|--------|
| Ctrl+, | Toggle settings modal (matches settings.conf: settings=CTRL ,) |
| Ctrl+B | Toggle file explorer sidebar |
| Ctrl+T | New blank tab |
| Ctrl+W | Close active tab |

---

## GPUI API Notes (for future contributors)

- **Entity<T>** unified handle for both view-like and data-only objects (replaces the old View<T> / Model<T> split).
- **Context<T>** per-view context that derefs to App; call cx.notify() to trigger a re-render.
- **Render::render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement** the render method signature.
- **cx.listener(|this, event, window, cx| { ... })** converts a method-style closure into a 'static callback, capturing a weak reference to the current view.
- **.id("unique-id")** promotes a Div to a Stateful<Div>, required for on_click, overflow_y_scroll, and other stateful interactions.
- **actions!(namespace, [ActionName])** declares zero-sized action structs usable with cx.bind_keys and .on_action.

---

## Extensibility Notes

- **.docx editing**: swap Tab::content: String for a document model that holds a parsed OOXML tree; the TextEditor view only calls state.active_content() so the change is localised to AppState.
- **Formatting ribbon**: add buttons by appending to the button_rows() array in FormattingRibbon. Hook each button's on_click to a new GPUI action dispatched to the focused TextEditor.
- **Settings modal**: add rows between the description text and the demo button in SettingsModal::render(). The Settings struct in config_parsing.rs already holds all keybind and formatting preferences from settings.conf.
- **File explorer**: the scan_directory function in state.rs already handles nested directories; adding support for other file types (.md, .txt) requires only loosening the extension filter.

---

## Task A: Cursor Movement Primitives (vim_mode branch)

Implements Task A from `notes/vim_todo.md` — the cursor-movement foundation
that both the plain (non-vim) editor and, later, vim Normal-mode motions
build on. All work is TDD'd: 41 new unit tests were added, each written
before its implementation and verified to fail for the right reason first.

### What Was Built

**`src/state.rs` — pure motion logic on `AppState`:**
- `move_left` / `move_right` / `move_up` / `move_down` — byte-offset-safe
  single-step cursor movement. Up/down preserve the character column
  (not byte column) and clamp to shorter lines, matching standard editor
  behaviour rather than vim's separate "desired column" memory (out of
  scope for this task).
- `move_line_start` / `move_line_first_nonblank` / `move_line_end` —
  line-boundary jumps (`Home`/`End` today; `0`/`^`/`$` once vim motions land).
- `move_word_forward` / `move_word_end` / `move_word_backward` — vim-style
  `w`/`e`/`b` word motions, built on a 3-class char classifier (word /
  punctuation / whitespace) so `foo.bar` treats `.` as its own word,
  matching vim rather than a naive whitespace-only split.
- `move_doc_start` / `move_doc_end` / `move_to_line` — document-level jumps.
- `cursor_line_col()` — maps the byte-offset cursor to a `(line, char_column)`
  pair for rendering.
- `set_cursor_from_line_col()` — the inverse, used by click-to-position;
  clamps both line and column to valid document bounds.
- Private helpers backing all of the above: `line_start`, `line_end`,
  `line_offset`, `byte_offset_for_col`, `char_class`, `skip_whitespace`,
  `word_forward`, `word_end`, `word_backward`.

**`src/text_editor.rs` — wiring and rendering:**
- `handle_key_down` now handles `left`/`right`/`up`/`down`/`home`/`end`
  (plain movement) and `Ctrl+Left`/`Ctrl+Right`/`Ctrl+Home`/`Ctrl+End`
  (word/document jumps), added to the existing Ctrl-modifier branch
  alongside copy/cut/paste.
- Replaced the old fake cursor (a static `"_"` always appended to the last
  line) with `render_cursor_line()`, which renders the line the cursor is
  actually on as three inline spans — text before, a highlighted cursor
  cell, text after — using `cursor_line_col()` to find the right line.
- Added click-to-position: an invisible `canvas()` element captures the
  editor's painted bounds into `content_bounds` (an `Rc<Cell<Bounds<Pixels>>>`
  field) every frame; `on_mouse_down` converts the click's window-space
  position into a line/column via `column_for_x`/`line_for_y` (pure,
  tested pixel-math helpers using an estimated monospace character width)
  and calls `set_cursor_from_line_col`.

### Known Limitations: Click Positioning

1. **Approximate character width.** `column_for_x`/`line_for_y` use a
   hardcoded monospace character-width estimate (`CHAR_WIDTH_PX = 8.4`, i.e.
   0.6× the 14px `text_sm()` font size), not real glyph shaping. Accurate
   enough for a monospace font in practice but will drift on unusual font
   configurations. Precise hit-testing would require rendering lines
   through GPUI's `InteractiveText`/`ShapedLine` APIs (which expose
   `index_for_x`) instead of plain `div()`s with string children — a larger
   rework noted in `notes/vim_todo.md` as future work.
2. **Breaks when the document is scrolled.** `content_bounds` is captured
   from a `canvas()` sized to the `.overflow_y_scroll()` viewport, so its
   origin is the visible top of the editor, not the top of the document.
   `line_for_y` counts lines from 0 relative to that origin — it has no
   knowledge of how many lines have scrolled out of view above it. After
   scrolling down N lines, a click will resolve to a line index N lower
   than the line actually clicked. This matters for this app in particular:
   debate evidence files are long and get scrolled constantly. Fixing this
   requires reading the scroll offset out of GPUI's scroll-handle state for
   the `.id("text-editor")` element and adding it to `local_y` before
   calling `line_for_y` — not done here; flagging as the next thing to fix
   before this is click-accurate on any file longer than one screen.

### Verification

- `cargo check`: clean (only pre-existing dead-code warnings for
  `move_line_first_nonblank`/`move_word_end`/`move_to_line`, which are vim
  Normal-mode motions not wired to any key yet — that's the next task).
- `cargo test`: 63 passed, 0 failed (59 in the bin crate incl. 41 new,
  4 in `tests/parse_testing.rs`).
- `./run.sh`: builds and reaches window creation; this sandbox has no GPU/
  EGL driver available (`MESA: error: ZINK: failed to choose pdev`), so the
  window itself could not be visually confirmed here. The failure is in the
  headless container's graphics stack, not in the build — confirm visually
  on a machine with a working display before merging.

---

## Task B: Selection Extend + Render (vim_mode branch)

Implements Task B from `notes/vim_todo.md` — selection extension via
Shift+motion keys, `Ctrl+A` select-all, and a real selection background
overlay, building directly on Task A's cursor primitives. 33 new unit tests,
all TDD'd (written before their implementation, verified to fail for the
right reason first).

### What Was Built

**`src/state.rs`:**
- Refactored `move_left`/`move_right`/`move_up`/`move_down` to delegate to
  new pure free functions `char_left`, `char_right`, `line_up`, `line_down`
  (pure refactor — no behavior change, verified by the existing Task A
  tests staying green throughout).
- Added `extend_left` / `extend_right` / `extend_up` / `extend_down` /
  `extend_word_forward` / `extend_word_backward` / `extend_line_start` /
  `extend_line_end` / `extend_doc_start` / `extend_doc_end` — the
  Shift-modified counterpart to each existing `move_*` method, all sharing
  one helper, `extend_selection(tab, new_cursor)`. That helper's anchor
  logic: reuse the existing selection's anchor if one is active, otherwise
  anchor at the cursor's pre-move position — so repeated Shift+motions grow
  the same selection, and reversing direction shrinks it back toward the
  anchor instead of resetting. A selection is kept as `Some((anchor,
  anchor))` (zero-width) rather than `None` when a Shift+motion returns
  exactly to its start, so the anchor survives.
- Added `select_all` (`Ctrl+A`) — selects the whole document, cursor to the
  end, matching standard (non-vim) editor convention.

**`src/text_editor.rs`:**
- Wired Shift+Left/Right/Up/Down/Home/End (plain movement's Shift variants)
  and Shift+Ctrl+Left/Right/Home/End (word/doc jump Shift variants) to the
  new `extend_*` methods; `Ctrl+A` to `select_all`.
- Replaced `render_cursor_line` (which only handled the cursor) with a more
  general `render_line`, built on two new pure functions:
  - `selection_span_for_line(line, line_byte_start, sel_start, sel_end)` —
    maps a selection's document-wide byte range onto a single line's
    char-column range, or `None` if the selection doesn't touch that line.
  - `line_segments(len, cursor_col, selection)` — merges the cursor
    position and/or selection range for one line into an ordered list of
    `(start, end, SegmentStyle)` runs (`Plain` / `Cursor` / `Selection`),
    handling every overlap case (cursor inside a selection always gets its
    own cell, drawn on top — matching how real editors layer a block cursor
    over selection highlighting).
  - `render_line` turns those segments into GPUI divs, styling `Selection`
    segments with `rgba(0x264F7880)` (spec 6.4's `#264F78` at ~50% opacity)
    and falling back to a single plain-text child (no extra divs) when a
    line has neither a cursor nor a selection on it, to avoid needlessly
    changing the render output for the common case.

### Verification

- `cargo check`: clean (same pre-existing dead-code warnings as Task A, for
  the vim-only motions still unused until vim Normal mode lands).
- `cargo test`: 96 passed, 0 failed (92 in the bin crate incl. 33 new since
  Task A, 4 in `tests/parse_testing.rs`).
- `./run.sh`: builds and reaches window creation; same headless-sandbox EGL
  limitation as Task A (`MESA: error: ZINK: failed to choose pdev`) —
  Shift+selection and the visual overlay could not be confirmed on screen
  here. Confirm visually on a machine with a working display.

---

## Task B Follow-up: Click-Drag Selection (vim_mode branch)

Implements the click-drag gap flagged at the end of the Task B writeup
above (`notes/editor_instructions.md` §4.3: "Mouse click-drag creates a
selection").

### What Was Built

**`src/state.rs`:**
- Extracted `byte_offset_for_line_col(content, line, col)` — a free
  function mapping a (line, char_column) pair to a byte offset, factored
  out of `set_cursor_from_line_col` so its exact clamping behavior is
  shared rather than duplicated.
- Added `extend_selection_to_line_col(line, col)` — the click-drag
  counterpart to `set_cursor_from_line_col`: same byte-offset math, but
  calls the existing `extend_selection` helper instead of clearing the
  selection. 4 new tests, TDD'd.

**`src/text_editor.rs`:**
- Extracted `line_col_from_mouse_position(position, content_bounds,
  num_lines)`, factoring the pixel-to-line/column math that used to be
  inlined in `on_mouse_down` so `on_mouse_move` can share it exactly.
- Added an `on_mouse_move` handler: if `event.dragging()` (GPUI's own
  "left button currently held" check) is false, it's a no-op; otherwise it
  calls `extend_selection_to_line_col`. No new drag-state field was needed
  — the first dragging move naturally anchors at wherever `on_mouse_down`
  left the cursor, because `extend_selection`'s anchor logic already falls
  back to the current cursor when there's no active selection yet, and the
  drag naturally stops the instant `event.dragging()` goes false (the
  button being released), with no explicit `on_mouse_up` handler required.

### Known Limitations

1. **A drag that starts outside the editor still extends a selection once
   it enters.** Because there's no explicit "did this drag start in the
   editor" flag — a deliberate simplification, see above — pressing the
   mouse button in the sidebar, holding, and dragging into the editor will
   fire `on_mouse_move` with `dragging() == true` and extend a selection
   from whatever the cursor's old position was, even though the user never
   clicked inside the editor. Minor, but a real "wrong output today," not
   just a cosmetic gap — worth a drag-origin flag if it turns out to matter
   in practice.
2. **No auto-scroll while dragging, compounding the existing scroll-offset
   bug.** `on_mouse_move` only fires while the cursor is over the editor's
   own bounds (GPUI semantics, not a choice made here), so there's no way
   to drag-select past the currently visible screen at all — combined with
   the scroll-offset bug already flagged in the Task A section, this means
   drag-select is only reliable within the first screenful of a document.
   For this app specifically (long debate evidence cards), that's a real
   gap: fixing the scroll-offset bug and adding scroll-while-dragging
   should be treated as one piece of follow-up work, not two.

### Verification

- `cargo check`: clean (same pre-existing dead-code warnings as before).
- `cargo test`: 100 passed, 0 failed (96 in the bin crate incl. 4 new,
  4 in `tests/parse_testing.rs`).
- `./run.sh`: builds and reaches window creation; same headless-sandbox EGL
  limitation as before — **this feature could not be visually confirmed at
  all.** Unlike Task A/B's keyboard-driven logic (which is mostly real,
  unit-tested code), click-drag is almost entirely GPUI interaction glue:
  whether `on_mouse_move` actually fires as expected, whether
  `event.dragging()` behaves as documented, and critically whether the
  `canvas().absolute().inset_0()` bounds-capture (added in Task A for
  click-to-position) actually yields the right origin. None of that is
  exercised by any unit test — it can't be. Click-to-position, and now
  click-drag, both depend on that one unverified assumption. **A real run
  on a display is the highest-priority next step before building anything
  else on top of mouse interaction in this editor** — if the bounds capture
  is subtly wrong, both features are wrong in the same way, silently.

**Update after manual testing:** the user ran the app on a real display and
confirmed cursor movement, click-to-position, click-drag selection, and the
selection overlay all work correctly at the top of a document (scroll
offset 0). The one gap found: no auto-scroll while dragging. See the next
section.

---

## Task B Follow-up: Auto-Scroll During Drag (vim_mode branch)

Adds auto-scroll to the click-drag selection built above, so dragging near
the top/bottom edge of the visible viewport scrolls the document and
extends the selection into the newly revealed content — the missing piece
identified by the user's manual test.

### What Was Built

**`src/text_editor.rs`:**
- `auto_scroll_delta(mouse_y, viewport_top, viewport_height, edge_margin,
  scroll_step)` — pure function returning how much to adjust the scroll
  offset by, when a drag position sits within `edge_margin` of the
  viewport's top or bottom edge. 7 tests, TDD'd.
- `clamp_scroll_offset(offset_y, max_offset_y)` — pure function clamping a
  proposed scroll offset into GPUI's valid `[-max_offset_y, 0]` range.
  4 tests, TDD'd.
- Added a `scroll_handle: ScrollHandle` field to `TextEditor`, wired via
  `.track_scroll(&self.scroll_handle)` on the editor's outer div (alongside
  the existing `.overflow_y_scroll()`). `on_mouse_move` now calls
  `auto_scroll_delta` + `clamp_scroll_offset` and, when non-zero, applies
  the adjustment via `scroll_handle.set_offset(...)` before computing the
  drag's (line, column).
- **Also changed `line_col_from_mouse_position`** (used by both
  `on_mouse_down` and `on_mouse_move`) to subtract the current scroll
  offset (`scroll_handle.offset().y`) from the mouse position before
  converting to a line number. This was necessary for auto-scroll itself to
  behave sensibly — without it, scrolling during a drag would immediately
  make every subsequent position calculation wrong by the scrolled amount.

### Unverified Assumption This Rests On — Read Before Trusting "Scroll Works"

The scroll-offset subtraction above assumes `content_bounds` (captured by
the Task A `canvas()` trick, from the *editor's own* paint bounds) stays
fixed to the viewport as the document scrolls, and that GPUI's
`ScrollHandle::offset().y` is the correct, matching quantity to subtract
from it. That assumption was **never true by construction** — it's an
inference about how GPUI's `.overflow_y_scroll()` + absolutely-positioned
children interact, and it wasn't tested at scroll offset 0 (the user's
manual test happened to exercise everything *except* this, since scroll
offset 0 makes the new subtraction a no-op). If the assumption is wrong —
if `content_bounds` actually moves with scrolled content instead of
staying pinned — the fix here doesn't just fail to help, it **regresses
already-confirmed-working click positioning** on any scrolled document, by
double-counting the scroll offset.

**This needs one specific manual test before it can be called done:**
scroll down partway through a long document, then click somewhere in the
middle of the visible screen — does the cursor land where you actually
clicked, or is it off by roughly the scrolled amount? Then try
click-drag-select while scrolled. If that test fails, the fix is to source
the viewport bounds from `scroll_handle.bounds()` instead of the `canvas()`
capture — `ScrollHandle` guarantees `.bounds()` and `.offset()` are in the
same coordinate system, which removes the ambiguity entirely — but that
change should wait until the test shows it's actually needed, not be done
speculatively.

Separately, `clamp_scroll_offset`'s clamp assumes `ScrollHandle::max_offset().y`
is a positive magnitude (the total scrollable distance). If it's actually
already negative, bottom-edge auto-scroll will silently never trigger
(`.max(0.0)` would zero out the clamp range). Untested for the same reason.

### Known Limitation: Auto-Scroll Stops If the Drag Holds Still at the Edge

Auto-scroll only advances inside the `on_mouse_move` handler, so it only
scrolls when the mouse is *moving* — dragging to the bottom edge and then
holding still (the natural motion for "I want to keep scrolling down") will
scroll a little on the events fired by reaching the edge and then stop,
rather than continuing to scroll while held. Real auto-scroll needs a frame
timer (e.g. re-arming via `window.on_next_frame` while a drag is active) to
keep advancing without further mouse movement. Not implemented here — for
this app's long debate-card documents, users dragging to an edge and
waiting is a realistic first-minute interaction, so this is a real gap to
close, not a cosmetic one.

### Verification

- `cargo check`: clean (same pre-existing warnings as before).
- `cargo test`: 111 passed, 0 failed (107 in the bin crate incl. 11 new,
  4 in `tests/parse_testing.rs`).
- `./run.sh`: builds and reaches window creation; same headless-sandbox EGL
  limitation — **the one thing that actually needs checking (scroll-offset
  correctness) could not be checked here at all**, since it only manifests
  at a non-zero scroll offset. Do not treat this as "done" until the
  scroll-then-click test above has been run for real.

---

## Task B Follow-up: Continuous Auto-Scroll (vim_mode branch)

Closes the "holds still at the edge" gap flagged at the end of the previous
section — auto-scroll now continues while a drag is parked in the edge
trigger zone, not just on each `on_mouse_move` event.

### What Was Built

Per explicit instruction, this got its own dedicated module,
**`src/auto_scroll.rs`** (registered via `mod auto_scroll;` in `main.rs`):

- `auto_scroll_delta` and `clamp_scroll_offset` — moved here unchanged from
  `text_editor.rs` (their 11 tests moved with them; still passing from the
  new location, confirming the move didn't change behavior).
- **`AutoScroller`** — a small struct holding shared handles
  (`content_bounds: Rc<Cell<Bounds<Pixels>>>`, `scroll_handle: ScrollHandle`,
  `state: Entity<AppState>`, all cloned from the same instances
  `TextEditor` already owns) plus `Cell`-based tracking of the last known
  mouse position, line count, and a `running` flag.
  - `notify(position, num_lines, window)` — called from `on_mouse_move`.
    Records the latest position; if not already running and the position is
    in the edge trigger zone, starts the tick loop.
  - `tick(window, cx)` — runs once per animation frame via
    `window.on_next_frame`. Recomputes the scroll delta fresh from the last
    known mouse position (not a cached value) each time, so it naturally
    stops the instant the position falls outside the trigger zone or
    `running` goes false — no separate "did the mouse leave the zone"
    signal needed. Applies the scroll offset, repositions the selection at
    the newly-revealed content under the (possibly stationary) mouse, then
    re-arms itself for the next frame if still active.
  - `stop()` — sets `running` false. Wired to **both** `on_mouse_up` (drag
    released over the editor) and `on_mouse_up_out` (released elsewhere,
    e.g. the user dragged into the sidebar and let go there) in
    `text_editor.rs`, so a drag that ends while parked in the edge zone has
    a way to actually stop scrolling — without this, a release outside the
    editor's bounds would leave the tick loop running indefinitely, since
    `on_mouse_move`/`on_mouse_up` only fire while hovering the element.
- `TextEditor` gained an `auto_scroller: AutoScroller` field, constructed in
  `new()` from clones of the same `content_bounds`/`scroll_handle` it
  already tracks (so `AutoScroller` always sees the editor's real, current
  state — never a stale copy).

**Why the frame-chain should self-sustain:** `tick` calls `cx.notify()`
(inside the `state.update` that repositions the selection), which dirties
the view and triggers a repaint; painting is what flushes GPUI's queued
`on_next_frame` callbacks, including the one `tick` just re-armed via
`arm()`. The very first tick is kicked off by the `cx.notify()` already at
the end of `on_mouse_move`. This is the standard "self-rescheduling
animation frame" idiom — reasoned through against GPUI's source, not run.

### What This Actually Tests, and How Each Failure Mode Reads

Auto-scroll is the first thing in this codebase that runs *at a non-zero
scroll offset by construction* — dragging to an edge and continuing to
scroll is the only way to get there. The user's manual test of Task A/B
confirmed cursor/click/drag/selection all work, but entirely at scroll
offset 0, where the scroll-offset subtraction added for auto-scroll
(previous section) is a no-op either way. So this isn't a new risk stacked
on a verified base — **it's the first real exercise of the coordinate-math
assumption that manual test never touched.**

**One gesture settles multiple open questions at once, and how it fails is
diagnostic.** Scroll to the middle of a long document, then click-drag from
mid-screen downward past the bottom edge and hold still:

- **Selection lands on the wrong line immediately, even before reaching the
  edge** → the scroll-offset coordinate math itself is wrong (the
  `content_bounds`-pinned-vs-scrolls-with-content question from the
  previous section).
- **Scrolling up works but scrolling down does nothing** → almost certainly
  the `max_offset().y` sign assumption flagged in the previous section:
  `clamp_scroll_offset`'s `-max_offset_y.max(0.0)` collapses the valid range
  to just `0` if GPUI actually reports that value as zero or negative,
  which would silently pin downward scrolling while leaving upward
  scrolling (which only depends on reaching `0`, not `max_offset_y`)
  unaffected. This is the single most likely concrete failure — it's why
  testing the *downward* edge specifically matters, not just any edge.
- **Scrolling happens but the selection visibly lags a frame behind** →
  benign ordering artifact, not a bug worth chasing.
- **Everything above works** → the frame-pump, the coordinate math, and the
  `max_offset` sign are all confirmed at once.

### Known Limitations (carried over, still real)

The two flagged in the "Click-Drag Selection" section above still apply
unchanged: a drag starting outside the editor still extends a selection
once it enters (no drag-origin flag), and this doesn't fix that. What *is*
now fixed is "no way to keep scrolling while held at an edge" — that was
the specific gap this follow-up targeted.

### Verification

- `cargo check`: clean (same pre-existing warnings; `AutoScroller` and its
  fields are all used, no new dead-code warnings introduced).
- `cargo test`: 111 passed, 0 failed — same count as before this change,
  since it's a relocation of existing tests plus new glue code that (like
  every other GPUI event-handler/frame-callback in this codebase) isn't
  itself unit-testable.
- `./run.sh`: builds and reaches window creation; same headless-sandbox EGL
  limitation as every prior mouse-interaction task. **This is implemented,
  not confirmed** — the scroll-down-and-hold test above is the one thing
  that would actually confirm it, and it hasn't been run.
