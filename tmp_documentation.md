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

---

## Task C: Undo/Redo (vim_mode branch)

Implements Task C from `notes/vim_todo.md` — a per-tab undo/redo stack,
the prerequisite for vim's `u` / `Ctrl+r` (spec 5.5), done before vim
Normal-mode commands as the todo document specifies.

### What Was Built

**`src/state.rs`:**
- Added `undo_stack: Vec<String>`, `redo_stack: Vec<String>`, and
  `last_edit_at: Option<Instant>` to `Tab` (spec 4.5's undo stack is
  content-only, not a richer struct — cursor position is *not*
  snapshotted). Wired into both `Tab::new_empty` and `Tab::from_path`, and
  the `#[cfg(test)] make_state` helper.
- `push_undo_snapshot()` (private) — pushes `content.clone()` onto the
  active tab's undo stack before a mutation, coalescing rapid edits within
  a 300ms window (`UNDO_COALESCE_WINDOW`) into a single undo step by
  skipping the push entirely when the previous push was recent enough — so
  typing a whole word costs one Ctrl+Z, not one per keystroke. Any push
  clears the redo stack (a new edit invalidates whatever future those redo
  entries pointed to). Capped at 200 entries (`UNDO_STACK_CAP`), dropping
  the oldest snapshot once exceeded.
- `delete_selection_raw()` (private) — the actual selection-deletion
  mutation, split out from the now-public-facing `delete_selection()` so
  `insert_char`/`insert_str`/`backspace` can delegate to it internally
  *without* triggering a second, redundant undo push — those three already
  push their own snapshot up front, capturing the true pre-edit state
  (selection included) in one step rather than splitting "selection
  deleted" and "character inserted" into two separate undo steps.
- `insert_char`, `insert_str`, `backspace`, `delete_selection` (public) all
  now push an undo snapshot before mutating — but only when a mutation will
  actually happen: `backspace` at document start and `delete_selection`
  with no active selection are true no-ops and correctly push nothing, so
  Ctrl+Z never lands on an empty step. `insert_str("")` (e.g. pasting empty
  clipboard content) returns immediately for the same reason.
- `undo()` / `redo()` — pop from one stack, push the content being
  replaced onto the other. Since only `content` is snapshotted, the cursor
  is *not* restored to its exact pre-edit byte offset (that information was
  never kept) — instead it's clamped into the restored content's bounds and
  onto its nearest valid char boundary via the new `clamp_to_char_boundary`
  free function, since the old offset may not even be a valid boundary in
  the restored text. Both clear the active selection, mark the tab
  modified, and reset `last_edit_at` to `None` so the very next edit can't
  coalesce backwards into whatever was on the stack before the undo/redo.

**`src/text_editor.rs`:**
- Wired `Ctrl+Z` (undo), `Ctrl+Y` (redo), and `Ctrl+Shift+Z` (the common
  alternate redo binding alongside Ctrl+Y) into the existing Ctrl-modifier
  branch of `handle_key_down`, alongside copy/cut/paste/select-all. No new
  global `KeyBinding` registration needed in `main.rs` — like `c`/`x`/`v`/
  `a`, these arrive as raw key events with `modifiers.control` set and are
  handled directly in the Ctrl-combo match.

### Verification

- `cargo check`: clean (only the pre-existing dead-code warnings for
  vim-only motions/text-object helpers not wired to any key yet).
- `cargo test`: 160 passed, 0 failed (156 in the bin crate incl. 24 new for
  this task, 4 in `tests/parse_testing.rs`). All 24 new tests are pure
  `AppState`/free-function tests requiring no GPUI context — coalescing is
  tested deterministically by rewinding a tab's `last_edit_at` field
  directly rather than sleeping in the test, so the suite stays fast.
- `./run.sh`: builds and reaches window creation; same headless-sandbox EGL
  limitation as every prior task in this document
  (`MESA: error: ZINK: failed to choose pdev`) — Ctrl+Z/Y/Shift+Z could not
  be exercised interactively here. Confirm on a machine with a working
  display: type a burst of text (should undo as one step), pause 300ms+ and
  type more (should be a separate undo step), and undo/redo/undo again to
  confirm the stacks swap correctly.

## Task D: Vim Mode Switching + Indicator

Implements `notes/vim_todo.md` Task D: the Normal/Insert/Visual/VisualLine/
Command mode-entry/exit table from `notes/editor_instructions.md` §5.1, plus
a mode indicator.

**`src/state.rs`:**
- Added `VimMode` enum (`Normal`/`Insert`/`Visual`/`VisualLine`/`Command`,
  `#[default] Normal`), and `vim_mode: VimMode` + `vim_command_buf: String`
  fields on `Tab` (wired into `Tab::new_empty`, `Tab::from_path`, and the
  `make_state` test helper). Added `vim_enabled: bool` on `AppState`,
  hardcoded `true` in `AppState::new()` — wiring `settings.conf`'s `vim`
  flag into this (spec §2.3) remains an open, separate gap, not touched by
  this task.
- `vim_command_buf` is added now (needed by Task E's count prefixes and
  Task H's command text) but Task D itself never writes to it — Command
  mode's keystroke accumulation was deliberately left out (see below).
- Ten mode-transition methods on `AppState`, one per spec 5.1 table row:
  `vim_enter_insert_before_cursor` (`i`), `vim_enter_insert_line_start`
  (`I`, via `move_line_first_nonblank` — vim's `^`, not literal byte 0),
  `vim_enter_insert_after_cursor` (`a`, via `move_right`),
  `vim_enter_insert_line_end` (`A`, via `move_line_end`),
  `vim_open_line_below` (`o`), `vim_open_line_above` (`O`),
  `vim_enter_visual` (`v`, selects the character under the cursor —
  zero-width at document end), `vim_enter_visual_line` (`V`, selects the
  current line including its trailing `\n` when one exists),
  `vim_enter_command` (`:`), and `vim_exit_to_normal` (shared by every
  "-> Normal" transition in the table). `o`/`O` both insert their newline
  via `insert_char`, reusing Task C's undo-stack push as required by the
  task spec. `O` needs one extra correction `o` doesn't: `insert_char`
  always advances the cursor past what it inserted, which for `O` (newline
  inserted *before* the existing line) leaves the cursor one line too far
  forward — the method resets `tab.cursor` back to the byte offset it
  captured before inserting.
- `handle_vim_key(&mut self, key: &str, shift: bool, key_char: Option<&str>) -> bool`
  dispatches on the active tab's `vim_mode` to one of three private
  sub-handlers, returning whether the key was consumed:
  - `handle_vim_normal_key` — matches Task D's mode-switch keys, plus a
    `':'` check described below. Deliberately lets named navigation keys
    (`left`/`right`/`up`/`down`/`home`/`end`) fall through (returns
    `false`) so the editor stays usable for moving around before Task E's
    real vim motions exist — safe in Normal mode specifically because
    there's no active selection a plain cursor move could corrupt.
    Everything else is swallowed (returns `true` without inserting text),
    matching real vim's Normal mode never falling through to text
    insertion for an unrecognized key.
  - `handle_vim_visual_key` — used for both Visual and VisualLine. Swallows
    every key unconditionally (unlike Normal mode) since letting navigation
    fall through here would clear the selection via `move_left`/etc.'s
    selection-clearing side effect instead of extending it. Only
    implements the exact exits spec 5.1 lists: lowercase `v` closes
    Visual, shifted `V` closes VisualLine — real vim's additional
    Visual<->VisualLine direct-switch behavior (pressing the *other* key
    switches variant instead of exiting) isn't in the spec table and is
    out of scope here; the mismatched key/shift combination is just a
    no-op.
  - `handle_vim_command_key` — Escape or Enter both exit to Normal without
    executing anything; every other key is swallowed. This is the one
    place this task deliberately implements less than the literal task
    text's example indicator string might suggest: no keystroke
    accumulation into `vim_command_buf`. Reason: composing correct command
    text requires distinguishing shifted-punctuation characters (`%`, `/`,
    etc.) from their unshifted base key, which is Task H's problem — a
    partial buffer that silently mangled those characters would look done
    while being subtly broken, which is worse than an honest stub.
- **`:` detection and a pre-existing app-wide gap it surfaced:** GPUI's
  `Keystroke.key` field reports the *unshifted* base glyph printed on the
  physical key — for the semicolon key, `key` is `";"` whether or not
  shift is held. The actual shifted character, when GPUI supplies one
  directly, comes via `Keystroke.key_char: Option<String>` (confirmed by
  reading the vendored `gpui` crate's `keystroke.rs`, not previously used
  anywhere in this codebase). Since this hadn't been confirmed against a
  real keyboard at the time of writing, `handle_vim_normal_key`'s `:` match
  checks *both*: `(key == ";" && shift) || key_char == Some(":")` — robust
  to either behavior GPUI might actually exhibit. Tracing this also
  confirmed a **pre-existing, not-introduced-here gap**: the plain-text
  insertion arm in `handle_key_down` (`k if k.chars().count() == 1 => ...`)
  only maps shift to uppercase for alphabetic characters; there is no
  shifted-punctuation mapping anywhere in the app, so typing `%`, `!`,
  `@`, `"`, etc. into document content does not currently produce the
  shifted character. Worth fixing generally (probably by using `key_char`
  when present) before Task H needs full command-text fidelity — `:%s/foo/
  bar/g` requires a correct `%`.

**`src/text_editor.rs`:**
- `handle_key_down` gained a vim-routing block, positioned after the
  Ctrl-combo branch (which still returns unconditionally and is unaffected)
  and before the existing Up/Down visual-row handling and the main
  plain-editor match. Reads `vim_enabled` and the active tab's `vim_mode`
  once via `self.state.read(cx)`. In Insert mode, only Escape is
  intercepted here (routed to `vim_exit_to_normal`) since nothing else in
  the plain-editor path below handles Escape at all; every other key falls
  through unchanged. In the other four modes, every key routes through
  `state.handle_vim_key(key, shift, key_char)` first; when it returns
  `true` the function returns immediately (already fully handled), when it
  returns `false` (Normal-mode navigation) execution continues down into
  the same Up/Down check and match block the plain editor uses.
- `render()` restructured: the top-level element returned used to be the
  scrollable/focusable editor div itself. It's now a `flex_col` wrapper
  holding two `flex_col` siblings — the scrollable editor div (unchanged
  internally, still the one `.track_scroll(&self.scroll_handle)` tracks)
  and a new fixed-height mode-indicator div below it, showing
  `-- NORMAL --` / `-- INSERT --` / `-- VISUAL --` / `-- VISUAL LINE --` /
  `-- COMMAND --`, or an empty string when vim mode is off entirely. Spec
  5.1 literally says "nothing shown for Normal" — deviated from that
  (per explicit user request after the initial Task D pass) since a blank
  indicator can't distinguish "vim on, Normal mode" from "vim off", both of
  which otherwise render identically. The indicator div is always present
  at a fixed height rather than conditionally added/removed, so switching
  modes doesn't resize (and force a re-wrap of) the editor's own viewport.
  `.flex_1()`/`.min_w_0()`/`.min_h_0()` moved from the old top-level div to
  the new wrapper, since the wrapper is now what sits in `main_window.rs`'s
  flex row; the inner scrollable div keeps its own `.flex_1()` to claim the
  wrapper's remaining vertical space after the indicator, and also needs
  its own `.min_h_0()` (added alongside its existing `.flex_1()`/
  `.min_w_0()`) — it went from a cross-axis-stretched flex_row child (fixed
  height guaranteed) to a main-axis flex_1 child of the new flex_col
  wrapper, where a flex item's default min-height is its *content* size;
  without this, a document taller than the viewport could grow the div past
  the wrapper's allocated height instead of scrolling internally, exactly
  the failure mode `main_window.rs:140`'s own `min_h_0` comment describes
  for the same pattern one level up.
  - **Deliberately not** nested inside the scrollable div, despite the
    task text's literal wording ("inside the outer div... after the line
    children") — that wording predates the wrap-system rework earlier in
    this branch. Nesting it inside would make the indicator scroll with
    content and shrink/grow `scroll_handle.bounds()`/`max_offset()` every
    time the mode changed, since both are derived from that div's own
    child content plus its box size. As a sibling instead, `scroll_handle`
    still tracks only the scrollable div, and GPUI's own layout
    recomputes that div's `.bounds()` correctly reflecting the reduced
    viewport height (indicator's height subtracted) automatically —
    `scroll_to_cursor` needed no code changes for this.

### Verification

- `cargo check`: clean (only pre-existing dead-code warnings for
  vim-motion-prep functions from Tasks A-C not yet wired to any key, plus
  the newly-added `VimMode` variants/fields not yet used by anything beyond
  Task D — expected, since Tasks E-I aren't implemented).
- `cargo test`: 194 passed, 0 failed (190 in the bin crate, incl. 34 new
  for this task: 17 for the ten mode-transition methods (including boundary
  cases — `o`/`O` on the first/last line and on an empty document) and 17
  for `handle_vim_key`'s dispatch across all five modes; 4 in
  `tests/parse_testing.rs`).
- `./run.sh`: launched and ran for 5s without a panic or crash (same
  headless-sandbox EGL/MESA warnings as every prior task in this document —
  `MESA: error: ZINK: failed to choose pdev`), confirming the `render()`
  layout restructure doesn't break at runtime. **Not visually verified** —
  no screenshot capability was available to confirm the indicator actually
  renders the right text at the right position, or that scrolling still
  feels correct with the slightly-shrunk viewport. Confirm on a machine
  with a working display: press `i`/`Escape`, `v`/`Escape`, `V`/`Escape`,
  `:`/`Escape` and check the indicator text at each step; confirm the
  cursor still tracks correctly near the bottom edge of a scrolled long
  document (Task B/C's scroll-margin fix depends on `scroll_handle.bounds()`
  being correct post-restructure, which is reasoned through above but
  untested on real hardware).

### Mode indicator follow-up: Normal mode now shows `-- NORMAL --`

Per explicit user request after the initial Task D pass, deviated further
from spec 5.1's literal "nothing shown for Normal": the indicator now shows
`-- NORMAL --` in Normal mode instead of an empty string, since a blank
indicator couldn't distinguish "vim on, Normal mode" from "vim off
entirely" — both rendered identically. `src/text_editor.rs`'s
`mode_indicator_text` match arm changed from `VimMode::Normal => None` to
`VimMode::Normal => "-- NORMAL --"` (the whole match's return type dropped
the `Option` wrapper on the match arms accordingly, keeping only the outer
`if state.vim_enabled { ... } else { None }` as the option boundary).
`cargo check`/`cargo test`: unaffected (194 passed).

## Task E (pass 1 of 2): Vim Motions — Pure Text Motions

Implements the text-navigation half of `notes/vim_todo.md` Task E / spec
5.2's motion table. Deliberately split into two passes (flagged explicitly
by the advisor consulted before starting): this pass covers every motion
that's pure text math over `AppState`/`Tab.content`, fully unit-testable
without a GPUI context. **Pass 2** — `H`/`M`/`L`, `Ctrl+D`/`U`/`F`/`B`,
`zz`/`zt`/`zb` — needs live viewport/scroll state that only exists in
`text_editor.rs`'s `ScrollHandle`, is architecturally a different shape of
work, and is largely unverifiable in this sandbox (no display) — **not yet
implemented**, tracked as its own follow-up. `%` (spec 5.2: "future:
matching bracket/paren") is explicitly out of scope per the spec itself.

**`src/state.rs`:**
- Added `last_find: Option<(char, char)>` to `Tab` (variant + target char,
  spec §2.2), wired into both constructors and `make_state`.
- `vim_command_buf` (added empty/inert in Task D) now does real work: it
  holds an optional leading run of digit characters (a `[count]` prefix)
  followed by an optional single trailing "pending trigger" character for a
  two-keystroke command still awaiting its second key (`g` awaiting a
  second `g`, or `f`/`F`/`t`/`T` awaiting a target). New free function
  `split_vim_command_buf(buf: &str) -> (Option<usize>, Option<char>)`
  parses this out (pure, unit-tested); `AppState::take_vim_count()` (pub —
  needed by `text_editor.rs`'s j/k, see below) and `vim_pending_trigger()`
  (pub, same reason) expose it; `push_vim_command_buf_char`/
  `clear_vim_command_buf` (private) mutate it.
- **Stale-count-leak fix:** every mode-transition method that enters a new
  mode (`vim_enter_insert_before_cursor`, `vim_enter_visual`,
  `vim_enter_visual_line`, `vim_enter_command`) now clears
  `vim_command_buf` on entry. Without this, a count typed in Normal mode
  (e.g. `3`) then abandoned by switching modes (`v`, `i`, etc.) would sit in
  the buffer and silently apply to an unrelated motion pressed after
  returning to Normal mode later — Task D never hit this since it never
  gave the buffer real content to leak.
- **Word motions (`w`/`b`/`e` already existed; added `W`/`B`/`E`):**
  `word_forward`/`word_end`/`word_backward` refactored to thin wrappers
  over new `*_classified(content, pos, classify: fn(char) -> CharClass)`
  functions, parameterized on the classifier instead of hardcoding
  `char_class`. New `big_word_class` (whitespace vs. everything else, no
  word/punctuation split) drives new `word_forward_big`/`word_end_big`/
  `word_backward_big`, exposed as `AppState::move_word_forward_big`/
  `move_word_end_big`/`move_word_backward_big` (`W`/`E`/`B`). The refactor
  changed no observable behavior — all pre-existing `w`/`b`/`e` tests still
  pass unmodified.
- **Paragraph motions (`{`/`}`, new):** `is_blank_line` + `paragraph_forward`/
  `paragraph_backward` free functions (a paragraph boundary is a
  zero-length line) + `AppState::move_paragraph_forward`/
  `move_paragraph_backward`. Both always search strictly past the current
  line, even when the cursor already sits on a blank line — matching vim,
  `}`/`{` never stay put.
- **Find-char motions (`f`/`F`/`t`/`T` + `;`/`,`, new):**
  `find_char_forward`/`find_char_backward`/`till_char_forward`/
  `till_char_backward` free functions (line-scoped — never cross a `\n`) +
  `resolve_find` dispatcher + `AppState::move_find_char_forward`/
  `_backward`/`move_till_char_forward`/`_backward` (all delegate to a
  private `apply_find(kind, target, remember)`) + `repeat_last_find` (`;`)
  /`repeat_last_find_reverse` (`,`). A failed find (target not on the
  line) is a true no-op — cursor doesn't move *and* `last_find` isn't
  updated, so `;` can't be left repeating a search that never actually
  landed anywhere. `;`/`,` themselves never update `last_find` — repeating
  leaves the original find remembered, so `;` after a `,` still repeats the
  *original* direction (verified by
  `test_repeat_last_find_reverse_after_reverse_still_repeats_original`).
  `apply_find` nudges the search start one character further in the search
  direction when repeating a `t`/`T` specifically (not `f`/`F`, which don't
  need it) — without this, `;` after a `t` would immediately re-find the
  same adjacent match it already stopped before and no-op forever; real vim
  has this same nudge.
- **`gg`/`G`/`N`+`gg`/`NG` (new):** no new method — composed from the
  existing `move_to_line` + `move_line_first_nonblank` directly in the
  dispatcher (see below), correcting `move_to_line`'s literal-column-0
  landing to vim's real first-non-blank semantics. `gg` with no count goes
  to line 1; `G` with no count goes to the *last* line via
  `move_to_line(usize::MAX)`, which `line_offset`'s existing clamp-on-
  overrun logic already handles correctly (breaks out as soon as it hits
  the real last line, not a `usize::MAX`-iteration loop).
- **`handle_vim_normal_key` rewritten** as a 3-state dispatcher (full
  reasoning in its own doc comment): (1) if a two-keystroke command is
  already pending, this key completes or abandons it — checked *first* so
  e.g. a pending `f` correctly treats `;` as a literal target character
  instead of the `:` check below hijacking it; (2) no pending command, but
  this key starts/extends a `[count]` digit prefix or starts a new
  two-keystroke command; (3) a complete single-key command, consuming
  whatever count was accumulated. `$`/`^`/`{`/`}` (shifted number/bracket
  keys) are checked *before* state 2's digit accumulation specifically
  because their unshifted base keys (`"4"`, `"6"`) are themselves valid
  count digits — a real bug caught by `cargo test`, not just reasoning:
  `test_handle_vim_key_normal_dollar_via_key_char` initially failed because
  `"4"` was being swallowed as a count digit before ever reaching the `$`
  check. Deviations worth flagging explicitly (not bugs, deliberate scope
  choices):
  - Count is **not** applied to `$`/`^` — real vim's `2$` means "end of the
    *next* line," which isn't implemented; `2$` here just goes to the end
    of the current line, same as plain `$`.
  - `j`/`k` are **not** matched in this dispatcher at all — see
    `text_editor.rs` below. `Shift+J`/`Shift+K` fall into the same j/k path
    (no shift check), so they currently move like plain `j`/`k` rather than
    being unmapped — deliberate: real vim's `J` (join lines) and `K` aren't
    implemented, and making them fall through instead would leak a literal
    "J"/"K" character into the document via the plain-editor insertion arm
    (traced and confirmed during review — the `("j",_)|("k",_) => false` arm
    in state 3 exists specifically so this doesn't happen for `j`/`k`
    themselves, but adding a shift-guard would re-open exactly that leak).
  - `Escape` while a find is pending abandons the pending find and returns
    to a clean Normal state, without inserting anything — falls out
    naturally from `vim_find_target_char` returning `None` for the
    multi-character `"escape"` key name, rather than needing special-case
    code.

**`src/text_editor.rs`:**
- `handle_key_down`'s vim-routing `else` branch (Normal/Visual/VisualLine/
  Command) gained a `j`/`k` special case *before* the `handle_vim_key` call:
  when `vim_mode == Normal` and no two-keystroke command is pending
  (`state.vim_pending_trigger().is_none()`), `j`/`k` are resolved via
  `take_vim_count()` + a loop over the existing `move_cursor_visual_row`
  (the same visual-row-aware movement Up/Down arrows already use), instead
  of going through `AppState::handle_vim_key` at all. Reused rather than
  reimplemented in `state.rs` because visual-row movement needs the current
  viewport's wrap layout — GPUI context `state.rs`'s pure methods don't
  have.
  - **Deliberate UX choice, not strict vim semantics:** real vim's `j`/`k`
    move by *logical* line, wrapping is a display-only concern. This app
    reuses the same visual-row movement the arrow keys already use instead,
    so `j`/`k` don't jump over a wrapped paragraph's continuation rows —
    matching how the rest of this editor already treats wrapped lines, at
    the cost of deviating from vim's literal logical-line `j`/`k`. Flagged
    per the pattern established for other deliberate spec deviations in
    this document.
  - The `vim_pending_trigger().is_none()` guard exists so a pending `f`/`t`
    still correctly treats a `j`/`k` keypress as its target character
    (`fj`) rather than this special case intercepting it as a motion first.

### Verification

- `cargo check`: clean (only pre-existing dead-code warnings for
  not-yet-wired vim-prep functions from earlier tasks).
- `cargo test`: 256 passed, 0 failed (all in the bin crate: 46 new tests for
  this task — word/WORD motions, paragraph motions, find-char motions +
  repeat, and the full dispatcher rewrite including the count/pending-
  trigger state machine, the `$`/digit-collision regression, and the
  stale-count-leak fix — plus the 210 carried over from Tasks A-D unchanged;
  4 in `tests/parse_testing.rs`). Every new pure function/method has direct
  test coverage; manual byte-offset traces were done by hand for the
  paragraph and find-char tests before running them, and all matched.
- **`j`/`k` is the one piece of this task with no automated test** — the
  `take_vim_count`/`vim_pending_trigger` logic it depends on is fully
  tested, but the `text_editor.rs` wiring itself needs a GPUI context this
  test suite doesn't spin up. `./run.sh` launched and ran 5s without a
  panic (same sandbox EGL/MESA warnings as every prior task), confirming no
  startup-time breakage, but this doesn't exercise keypresses. **Needs
  manual verification on a machine with a working display**, specifically:
  plain `j`/`k`; `3j`/`3k` (count actually loops); `fj` (must find a
  literal `j` character and *not* move down — the pending-trigger guard);
  and `gg`/`G` (composed from existing, already-tested primitives, but
  never exercised through the live dispatcher before now).
- Reminder for context: `vim_enabled` is still hardcoded `true` in
  `AppState::new()` (§2.3's config-wiring gap, unaddressed since Task D) —
  the app boots straight into vim Normal mode, where typing does nothing
  until `i`/`a`/`o`/etc. is pressed. Not new to this task, but worth
  restating since Task E is what makes Normal mode's motions actually do
  something for the first time.

## Task E (pass 2 of 2): Command Echo, Visual-Mode Motions, Remaining Motions, Macros

Closes out Task E / spec 5.2's viewport-relative motions plus several
adjacent gaps the user flagged directly: a visible command/count buffer,
Visual mode actually doing something (spec 5.6), the `_` motion, `H`/`M`/`L`,
and `q`/`@` macro recording/replay (user-requested directly, not part of
`editor_instructions.md`). `Ctrl+D`/`U`/`F`/`B` and
`zz`/`zt`/`zb` remain out of scope — not requested this pass, tracked
separately if wanted later.

### Command/count buffer echo

`text_editor.rs`'s `render()` now reads the active tab's `vim_command_buf`
(when non-empty and mode is Normal/Visual/VisualLine) and appends it,
space-separated, to the mode-indicator line — so a half-typed `3f` or `g`
is visible next to `-- NORMAL --` instead of silently pending.

### `$`/`^`/`{`/`}`/`:` detection fix (real-hardware bug)

Pass 1's `$` detection (`key_char == Some("$")` or `key == "4" && shift`)
did nothing on real hardware. Root cause: this app's GPUI/WSLg/X11 backend
sometimes reports the shifted symbol **directly** in `key` (e.g. `key ==
"$"` for a literal Shift+4), contradicting the vendored `Keystroke` docs.
New free function `matches_shifted_symbol` in `state.rs` checks all three
possible reporting forms and is now used for `$`/`^`/`{`/`}`/`:` uniformly.
Regression tests added for each ("reported directly as symbol" case) plus a
negative test confirming a plain `4` digit isn't misdetected as `$`.

### Visual-mode motion extension (spec 5.6)

Visual mode previously had zero functionality beyond entry/exit. `state.rs`
now has a shared `handle_vim_motion_key(key, shift, key_char, extend) ->
Option<bool>` containing the *entire* Normal-mode count/pending-trigger
state machine and motion table; `extend: false` (Normal) moves the cursor,
`extend: true` (Visual/VisualLine) grows the selection via the same
resolved target through a new `apply_vim_motion`/`extend_selection` path.
`handle_vim_normal_key` and the rewritten `handle_vim_visual_key` (now
returns `bool`, takes `key_char`) both route through it. Also fixed a bug
caught by this pass's own tests (not user-reported): `vim_enter_visual`/
`vim_enter_visual_line` set `tab.selection` but never `tab.cursor`, so the
visible cursor and any following motion's starting point stayed at the
pre-Visual position instead of the selection's far edge.

### `_` motion and `H`/`M`/`L`

`_` (first non-blank of the `[count]`-th line down) is pure text math —
new free function `underscore_motion` in `state.rs`, wired into the shared
dispatcher like every other pass-1 motion. `H`/`M`/`L` (top/middle/bottom
of the *visible* viewport) are viewport-relative: `text_editor.rs` computes
the visible row range from `scroll_handle.offset()`/`.bounds()` and the
existing visual-row layout pipeline, resolves it down to a plain logical
line number, then hands off to a new GPUI-context-free `AppState::
vim_move_to_line_first_nonblank(line, extend)` — mirroring the existing
j/k split between GPUI-bound row math and pure line-targeting.

### Macro recording/replay (`q` / `@` — user-requested, not in the spec)

`editor_instructions.md` has no macro section (§5.8 "Vim Registers" covers
only yank/paste registers, not macros) — `q`/`@` were requested directly by
the user ("q/@ macros, etc.") and implemented as an addition beyond the
written spec.

- **Recording (`q`/`q<register>`)** lives entirely in `AppState`
  (`state.rs`), no GPUI context needed: `vim_macro_recording: Option<(char,
  Vec<RecordedVimKey>)>` holds the in-progress capture, `vim_macros:
  HashMap<char, Vec<RecordedVimKey>>` holds saved recordings.
  `handle_vim_normal_key` checks for a pending `q<register>` completion or a
  bare `q` (start if idle, stop-and-save if recording) *before* the shared
  motion dispatcher, but only when `vim_pending_trigger().is_none()` — so
  `fq` still resolves as "find the literal character `q`", not a macro
  command. Reachable from Visual mode too (it shares the same dispatcher
  entry), a deliberate, minor scope widening beyond what was asked.
- **Replay (`@<register>`/`@@`)** lives entirely in `text_editor.rs`
  (Normal mode only — a deliberate scope narrowing, since replaying a
  logical-line-relative macro from mid-selection has unclear semantics and
  wasn't asked for) because it needs to re-enter GPUI-context-dependent key
  handling (`j`/`k`/`H`/`M`/`L`) that `AppState` can't reach on its own.
  `handle_key_down` was split into `process_key`/`process_key_ctrl_combo`/
  `process_key_plain` specifically so replay can call `process_key` in a
  loop with a recorded `(key, shift, key_char)` triple, exactly re-running
  the same dispatch a live keystroke takes. The register's keys are read
  and cloned *before* the replay loop starts, with that borrow released
  first — each `process_key` call does its own `state.update`/`read`, and
  GPUI panics if one of those runs while another is already open on the
  same entity.
- **Recording capture** wraps every non-Ctrl keystroke (`process_key`
  checks `vim_is_recording_macro()` before and after dispatching to
  `process_key_plain`, recording the key only if recording was already
  active *before* and is *still* active *after* — which naturally excludes
  the `q`/register keystrokes that start a recording and the `q` that ends
  one, while still capturing Insert-mode text typed mid-recording, since
  the wrap is around the whole dispatch, not just the vim-specific branch).
- `3@a` (count-repeat replay) is **not implemented** — real vim supports
  it, but it wasn't asked for and was flagged as an optional, skippable
  extra during design review; `@a` always replays exactly once.

### Known Limitations

- **Resolved**: `q`/`@` macros were initially reported as not working on
  real hardware. Root-cause investigation (see `superpowers:systematic-
  debugging`-guided session) confirmed via GPUI's actual X11 source
  (`gpui_linux/src/linux/platform.rs`, `keystroke_from_xkb`) that the
  shifted-symbol-reported-directly behavior `matches_shifted_symbol` was
  built to handle is real and consistent — `Keysym::at => "@"`,
  `Keysym::less => "<"`, etc. — and that macro's control flow was correct
  against that confirmed behavior. Asking the user what they'd actually
  observed revealed the true issue: **the functionality worked; there was
  just no visual feedback** on the mode-indicator line while `q`/`@`
  sequences were pending or a recording was active — because
  `vim_macro_record_pending`/`macro_at_pending`/the active-recording state
  deliberately live in fields separate from `vim_command_buf` (to avoid
  colliding with its own pending-trigger grammar), and the pending-command
  echo built in this pass only ever read `vim_command_buf`. Fixed by
  extending that echo to also reflect these fields — see the dedicated
  "Mode-Indicator Feedback Fix" section further down, added after Task G.
- `@<register>` replay only intercepts in Normal mode; Visual-mode `@`
  falls through to the shared motion dispatcher and is silently swallowed
  (no crash, just a no-op).
- Macro playback doesn't preserve Ctrl/Cmd-modified keystrokes — if one was
  recorded (unusual; Ctrl combos are app-global shortcuts, not part of
  vim's own keystroke stream), replay reruns it as a plain, unmodified key
  instead. Deliberate, narrow scope decision, not a bug.
- `Ctrl+D`/`U`/`F`/`B` (half/full-page scroll) and `zz`/`zt`/`zb` (re-center
  cursor in viewport) remain unimplemented — never part of this pass's
  request.

### Verification

- `cargo check`: clean (only pre-existing dead-code warnings for
  not-yet-wired vim-prep functions, unrelated to this pass).
- `cargo test`: 290 passed, 0 failed, up from 256 at the end of pass 1 (34
  new tests: command-buf echo has no direct tests of its own since it's a
  `render()` string concern, covered instead by the underlying
  `vim_command_buf` tests already in place; ~17 for Visual-mode motion
  extension including the `vim_enter_visual`/`vim_enter_visual_line`
  cursor-fix regressions; 8 for `_`/`vim_move_to_line_first_nonblank`; 6 for
  macro recording start/stop/capture/register-collision-with-pending-find).
- **`H`/`M`/`L`'s viewport-row math and all of `@` replay have no automated
  tests** — both live in `text_editor.rs` and need a live GPUI context/
  `ScrollHandle` this test suite doesn't spin up, same limitation already
  noted for `j`/`k` in pass 1. `./run.sh`/`timeout 5 ./target/debug/
  vimbatim` launched and ran the full timeout without a panic (same
  sandbox EGL/MESA warnings as every prior task) after every change in this
  pass, confirming no startup-time breakage, but this doesn't exercise
  keypresses. **Needs manual verification on a machine with a working
  display**, specifically: `H`/`M`/`L` after scrolling partway through a
  long document; `q a <keys> q` then `@ a` reproducing those keys exactly;
  `@@` repeating the last-replayed register; `q` while already recording
  correctly stops rather than starting a nested recording; and a macro
  recorded across an Insert-mode excursion (`qa` → `i` → type text →
  `Escape` → `q`) replaying the typed text correctly.

## Task F: Vim Operators + Text Objects

Implements `notes/editor_instructions.md` §5.3 (operators) and §5.4 (text
objects). **Unit-tested and self-consistent, but NOT hardware-verified** —
unlike earlier tasks in this document, this one is flagged explicitly as
such rather than marked plain "Done", because the macro feature (previous
section) was marked done after the same kind of unit-test-only coverage
and turned out broken on real hardware. Task F carries a real, narrower
version of that same risk — see "Hardware verification" below before
trusting this beyond the test suite.

### Architecture: `MotionKind` and the resolve/apply split

The foundational change, done first and validated with an advisor before
building operators on top of it: `handle_vim_motion_key` (Task E's shared
Normal/Visual motion dispatcher) was split into a thin wrapper plus a new
`resolve_vim_motion(&mut self, key, shift, key_char) -> MotionResolution`,
which resolves a keystroke to a target position *and* a `MotionKind`
(`ExclusiveChar`/`InclusiveChar`/`Linewise` — vim's own `:help exclusive`/
`inclusive`/`linewise`) without applying it anywhere. `MotionKind` is the
piece a bare `usize` target loses: `dw` and `de` must produce different
ranges from the same cursor position even when they'd otherwise look
similar, and only the kind distinguishes them. Three consumers now share
one motion table: Normal-mode cursor movement, Visual-mode selection
extension (both via `handle_vim_motion_key`, unchanged from Task E), and
Task F's operators (via `resolve_vim_motion` directly). This refactor was
mechanical enough that all 301 pre-existing tests passed unchanged
immediately after it, confirmed before any operator code was written.

One quirk preserved carefully: Normal mode's historical "let `left`/
`right`/`home`/`end` fall through to the plain editor" convenience is now
implemented as a check in the thin wrapper *before* calling
`resolve_vim_motion` — the resolver itself always resolves those four keys
locally (as h/l/0/$ equivalents), since operators need a real target for
`dh`/`d<end>`/etc. and have no such fallthrough concept.

### Operators: `d`/`y`/`c` + `dd`/`yy`/`cc`, `>`/`<` + `>>`/`<<`, `gU`/`gu`

- **Pending-operator state**: new `Tab.vim_pending_operator: Option<char>`
  (plus `vim_pending_text_object_prefix: Option<bool>` for the `i`/`a`
  stage — see below), deliberately *not* reusing `vim_command_buf`'s
  pending-trigger mechanism (used by `f`/`g`/etc.) since an operator
  pressed key does not behave like a motion trigger — `handle_vim_normal_key`
  checks it first, ahead of even the macro `q`/`@` check, so a pending `d`
  correctly claims the *next* keystroke (e.g. `d` then `q` abandons `d`
  rather than starting macro recording into register `q` — a regression
  this session's own tests caught and fixed via ordering, not a
  hypothetical).
- **`complete_vim_operator`** resolves, in order: (1) a pending text-object
  prefix, (2) the doubled-operator case (same key pressed again — `dd`/
  `yy`/`cc`/`>>`/`<<`), (3) an `i`/`a` prefix starting a text object, (4)
  otherwise delegates to `resolve_vim_motion`. Its `MotionKind::Linewise`
  vs `ExclusiveChar`/`InclusiveChar` result becomes the operator's range
  via `vim_operator_motion_range`/`vim_operator_doubled_range`; `c` gets a
  documented special case (linewise change excludes the trailing newline,
  emptying the line in place rather than deleting it, matching real vim);
  `>`/`<` get a similar override forcing `Linewise` regardless of the
  motion's own kind (vim's own rule — `>w` indents the *line*, even though
  `w` is charwise), which `gU`/`gu` deliberately do **not** share (they
  respect the motion's real kind, so `gUw` only uppercases the word).
- **Registers**: minimal, write-only `AppState.registers: HashMap<char,
  String>` — `d`/`c` write to `'"'`, `y` writes to both `'"'` and `'0'`.
  No `"a`-prefix register selection, no `+` clipboard routing, and nothing
  reads a register back yet (`p`/`P` paste is Task H) — pulling forward
  only the minimum slice of Task H's register design that operators need.
- **`gU`/`gu`** are two-keystroke commands (`g` + `u`, disambiguated by
  `shift` on the second key, same convention as every other letter in this
  file) intercepted in `handle_vim_normal_key` *before*
  `handle_vim_motion_key` would otherwise claim the second key as `gg`'s
  (failed) pending completion and silently abandon it. Internally
  represented as operator ids `'U'`/`'u'` (not `'g'`) since the actual
  operator identity isn't known until the second key arrives.
- **Indent (`>`/`<`)**: no shiftwidth setting exists anywhere in this app,
  so a literal tab is the indent unit (matching the plain editor's own Tab
  key), and unindent removes one leading tab if present, else up to 4
  leading spaces — a stand-in for "one shiftwidth" of space-indented
  content, not an exact match to any configured width.
- **Scope limits, all deliberate and documented in code comments**:
  count-*before*-an-operator (`3dd`) isn't supported, only count-*after*
  (`d3w`) or count-*between* a doubled operator's two keys (`d2d`) —
  combining both would need multiplying two separate counts, deliberately
  deferred. `dj`/`dk`/`d<up>`/`d<down>` (and the `>`/`gU` equivalents)
  don't work — `j`/`k` always resolve to `MotionResolution::NeedsGpui`
  (no viewport context in `AppState`), so they cleanly abandon a pending
  operator rather than doing nothing useful; **this is the same class of
  gap the advisor caught for macros** (`d@`/`dj` bypassing
  `complete_vim_operator` entirely via `text_editor.rs`'s j/k/H/M/L/`@`
  interceptions) — fixed by adding a `vim_pending_operator()` guard
  alongside the existing `vim_pending_trigger()` one at all three
  interception sites, *before* any operator code was called done. `gUU`/
  `guu` (doubled-key form of the case operators) isn't implemented — the
  generic doubled-key check assumes an unshifted single-char key, which
  doesn't fit `gU`'s second keystroke; abandons cleanly rather than
  misfiring.

### Text objects: `iw`/`aw`, `is`/`as`, `ip`/`ap`, quotes, brackets

All five resolvers are free functions returning `Option<(usize, usize)>`
(`text_object_word` returns a plain tuple — it degenerates to a
zero-width object rather than failing), dispatched by
`resolve_vim_text_object` from a target character resolved via the
existing `vim_find_target_char` (reused rather than duplicated, so
shifted-punctuation object keys like `"`/`(`/`{` get the same robust
detection `f`/`F`/`t`/`T` targets already have).

- `iw`/`aw`: contiguous run of the same `CharClass` (word/punctuation/
  whitespace — reusing Task E's own word-motion classification) as the
  character under the cursor; `aw` additionally swallows one adjacent
  whitespace run, trailing preferred, falling back to leading.
- `is`/`as`: **simplified** sentence boundary — ends at the first `.`/`!`/
  `?` followed by whitespace or end-of-content, with no handling of
  abbreviations, decimal numbers, or quote/paren-wrapped punctuation. A
  documented simplification of vim's own more elaborate sentence grammar,
  chosen because precise sentence detection is the least load-bearing text
  object for this app's actual use case (debate-card editing leans much
  harder on word/paragraph/quote/bracket objects).
- `ip`/`ap`: reuses `is_blank_line` (the same paragraph-boundary
  definition `{`/`}` already use) — contiguous run of lines sharing the
  cursor line's blank/non-blank status; `ap` swallows one adjacent block
  of the *opposite* status, trailing preferred, falling back to leading —
  the same inclusion pattern as `aw`, one level up.
- `i"`/`a"`, `i'`/`a'`: **current line only** (matching real vim — quote
  objects never cross lines), picks the first quote pair that contains or
  starts at/after the cursor.
- Brackets (`i(`/`a(`, `i[`/`a[`, `i{`/`a{`, either half of the pair
  selects the same region): unlike quotes, these search the **whole
  document** and are nesting-aware via a single forward scan with a stack
  of open positions — among all matched pairs enclosing the cursor, the
  smallest is the innermost, matching real vim. Unmatched brackets are
  ignored rather than erroring.

**Known gap, not fixed**: `>`/`<`'s forced-linewise override lives in
`vim_operator_motion_range` (the motion path) but the text-object path
(`resolve_vim_text_object`) always builds an `ExclusiveChar` range — so
`>iw` would insert a tab mid-line instead of indenting, rather than being
rejected or redirected. Not fixed because it's a nonsensical command in
practice (nobody actually types `>iw`); `>ip`/`>ap` — the combinations
that matter — work correctly since paragraph ranges are already
line-aligned by construction.

### Verification

- `cargo check`: clean (only pre-existing dead-code warnings, unrelated to
  this task).
- `cargo test`: 345 passed, 0 failed, up from 290 at the end of Task E
  (55 new tests spanning the `MotionKind`/`resolve_vim_motion` split, every
  operator × several motion kinds, `dd`/`yy`/`cc` and their counted forms,
  every text object's inner/around variant plus a couple of edge cases
  each, `>>`/`<<`/`gU`/`gu`, and regression tests for both bugs an advisor
  review caught before they shipped: the `d`-then-`q` macro-collision
  ordering, and the `dj`/`d@`-bypasses-the-operator gap in
  `text_editor.rs`'s GPUI-bound key interceptions).
- **Hardware verification checklist, in priority order** (none of this is
  exercised by the test suite — GPUI isn't spun up in tests). Update: the
  `matches_shifted_symbol` risk flagged below for `>>`/`<<` turned out to
  be unfounded — root-cause investigation of the (mis-)reported macro
  breakage confirmed via GPUI's actual X11 source that this detection
  mechanism is sound (see Task E pass 2's Known Limitations); `>>`/`<<`
  should work correctly by the same confirmed mechanism. Left as the first
  item to check anyway, since it's still unexercised by the test suite:
  1. `>>` and `<<` — `matches_shifted_symbol` (shift+`.`/shift+`,`) is now
     confirmed sound against GPUI's actual `keystroke_from_xkb` source
     (`Keysym::greater => ">"`, `Keysym::less => "<"`, matching this
     helper's checks exactly), but still worth a quick real-keystroke
     confirmation since nothing in this task has been.
  2. `dw` vs `de` from the same cursor position — must produce visibly
     different results (the entire point of `MotionKind`).
  3. `dd`/`yy`/`cc`, plus `d2d` (count between doubled keys) and `d3w`
     (count after the operator).
  4. `ciw`/`di(`/`ci"` — the text-object completions whose *object*
     character is shifted punctuation (`"`/`(`), the second-highest
     shifted-symbol reporting risk in this task after `>>`/`<<`.
  5. `gUw`/`guiw` — confirm charwise (only the word changes), not the
     whole line.
  6. `dj`/`dk` — confirm they're a clean no-op (operator abandoned,
     content unchanged), not a dangling operator that misfires on the
     *next* keystroke (the exact bug the advisor caught and this session
     fixed before it could ship).

## Task G: Visual-Mode Operators

Closes out Task G (spec 5.6) — the motion-extension half was already done
in Task E pass 2; this adds the operator row: `d`/`x`, `y`, `c`, `>`, `<`,
`~` (toggle case), `gU`, `gu`, and `o` (swap selection ends). A separate
`notes/editor_instructions.md` §11 "Optional Features" section was added
first, at the user's direction, to record one explicit scope decision:
using a text object (`iw`, `i"`, etc.) to directly set the Visual
selection (real vim's `viw`) is **not** in spec 5.6's table and was left
out of this pass — tracked there (§11.1) as an optional fast-follow, not
built.

### Architecture: immediate execution, not a pending sequence

Unlike Normal mode's operators (Task F), which start a *pending* sequence
waiting for a motion/text-object/doubled-key, Visual-mode operators act
**immediately** on the selection that already exists — there's no
"waiting for the next key" state to manage. `handle_vim_visual_key` gained
three checks, all placed before the shared motion dispatcher:

1. `gU`/`gu` — checked first (mirroring Task F's identical ordering
   concern): a pending `g` (from `gg`) would otherwise let
   `handle_vim_motion_key` claim the following `u`/`U` as `gg`'s failed
   completion and silently abandon it.
2. The rest of the operator set (`resolve_vim_visual_operator_key`, a new
   free function reusing `matches_shifted_symbol` for `>`/`<`/`~`'s
   shifted-punctuation detection) and `o`.
3. Both (2) are gated on `vim_pending_trigger().is_none()` — **a real
   regression this session's own pre-existing test suite caught**: `f`
   then `d` (find target `'d'`) was misfiring as "start the delete
   operator" before this guard was added, since the new operator check ran
   ahead of `handle_vim_motion_key`'s own pending-find completion. Same
   collision class the advisor flagged for Task F's Normal-mode operators
   and macros — caught here by `cargo test` failing immediately, not
   shipped and found later.

`vim_visual_operator_range(operator) -> Option<(usize, usize, MotionKind)>`
resolves the active selection into the range an operator needs — the
Visual-mode counterpart to Task F's `vim_operator_motion_range`, except
the range is already given by the selection rather than built from a
cursor/target pair. `VisualLine` selections are always linewise; so are
`>`/`<` even from a plain (charwise) `Visual` selection (vim's own rule —
indent always affects whole lines). `c` on a linewise range excludes the
trailing newline, matching Normal mode's `cc` (empties the line in place).
The line-aligned bounds are recomputed from the selection's current
min/max rather than trusted to already sit on line boundaries, since
`VisualLine`'s selection is only guaranteed line-aligned at entry — a
charwise motion extending it afterward isn't specially re-snapped (a
separate, pre-existing gap, not fixed here; being defensive here is what
keeps this method correct regardless).

`execute_vim_visual_operator(operator)` runs the operator and returns to
Normal mode afterward via `vim_exit_to_normal` — except `c`, which already
transitions to Insert mode on its own (reusing Task F's
`execute_vim_operator_range`, unchanged), so calling `vim_exit_to_normal`
afterward would wrongly revert that. `~` (no Normal-mode equivalent exists
yet — that's Task I's single-character `~`) gets its own small
`toggle_case_vim_range`, since it flips each character's case
independently rather than pushing everything one direction like `gU`/`gu`.

`o` (`vim_visual_swap_ends`) just swaps the selection's anchor/focus and
moves the cursor to the new focus — the highlighted range itself doesn't
change, and it stays in Visual mode (doesn't execute anything or exit).

### Verification

- `cargo check`: clean (only pre-existing dead-code warnings, unrelated).
- `cargo test`: 359 passed, 0 failed, up from 345 at the end of Task F (14
  new tests: every operator in both Visual and VisualLine modes,
  `>`/`<`'s forced-linewise-even-from-charwise-selection rule, `gU`'s
  charwise-not-linewise distinction, `~`, `o`, and the `f`-then-`d`
  regression above).
- **No automated tests for the actual GPUI-bound keystroke path** — same
  limitation as Tasks E/F, this suite never spins up a live GPUI event
  loop. `>`/`<`'s Visual-mode entry points reuse the same
  `matches_shifted_symbol`-based detection Task F's `>>`/`<<` use, which
  a later root-cause investigation confirmed sound against GPUI's actual
  X11 source (see Task E pass 2's Known Limitations) — so this is no
  longer an open risk the way it was when this task was first written,
  but still worth a real-keystroke pass since nothing here has had one.
  `./run.sh`/`timeout 5 ./target/debug/vimbatim` launched and ran the full
  timeout without a panic after every change in this task, confirming no
  startup-time breakage only. **Needs manual verification**, in priority
  order: `d`/`x`/`y`/`c` on both a charwise and a `VisualLine` selection;
  `gU`/`gu`/`~` (shifted-`~` specifically); `>`/`<` in Visual mode; `o`
  after extending a selection in both directions; and the `f`-then-`d`
  (or any pending find) interaction, confirming the fix actually holds on
  a live keyboard, not just in this test suite.

## Mode-Indicator Feedback Fix (post–Task G)

Root-cause investigation of the "`q`/`@` macros are broken" report (using
`superpowers:systematic-debugging`) traced the actual GPUI X11 source
(`gpui_linux/src/linux/platform.rs`'s `keystroke_from_xkb`, from the
vendored `zed` git dependency) rather than continuing to guess. Confirmed
facts from that source: the keysym table really does map shifted
punctuation directly to the resolved symbol (`Keysym::at => "@"`,
`Keysym::less => "<"`, `Keysym::greater => ">"`, `Keysym::asciitilde =>
"~"` — exactly matching every `matches_shifted_symbol` call site in this
codebase), and GPUI *deliberately* clears `modifiers.shift` for symbol
keys ("we only include shift for upper-case letters... not for numbers
and symbols") — a real quirk, but one `matches_shifted_symbol`'s first
check (`key == symbol`, independent of `shift`) already tolerates. Tracing
the full `q`→register→content→`q`→`@`→register control flow against this
confirmed behavior turned up no logic bug.

Asking the user what they'd actually observed (rather than continuing to
guess) resolved it: **the functionality was working correctly** — the
only problem was the mode-indicator line showing no feedback while a
`q`/`@` sequence was pending or a recording was active. Root cause: Task
E pass 2's pending-command echo (`pending_command_text` in
`text_editor.rs`) only ever read `Tab.vim_command_buf`, but the pending
states Task F/G added afterward (`vim_pending_operator`,
`vim_pending_text_object_prefix`, and macros' own
`vim_macro_record_pending`/`macro_at_pending`) all deliberately live in
*separate* fields — precisely to avoid colliding with `vim_command_buf`'s
own count/pending-trigger grammar (see `start_vim_operator`'s doc
comment) — so the echo never saw them.

**Fix**: two new `AppState` accessors, `vim_recording_register() ->
Option<char>` and `vim_macro_record_pending() -> bool` (mirroring
`vim_is_recording_macro()`, which already existed), and
`pending_command_text`'s composition extended to append: a pending
operator character (plus its `i`/`a` text-object prefix, if any), a
pending `q` or `@` waiting for its register, and — for the whole duration
of an active recording, not just the initial keystroke, matching real
vim's own status-line behavior — `[recording @<register>]`.

This was a **feedback gap, not a functional bug** — `d`/`y`/`c`/`>`/`<`/
`gU`/`gu`/text objects/macros all worked before this fix; the fix only
makes their in-progress state visible. No behavior change to any
operator, motion, or macro logic.

### Verification

- `cargo check`: clean.
- `cargo test`: 361 passed, 0 failed, up from 359 (2 new tests for the two
  accessors).
- The indicator string construction itself has no direct test (it's a
  `render()` concern, same as the original Task E pass 2 echo) — covered
  indirectly via the two new accessor tests plus every existing
  operator/macro test that already exercises the underlying state these
  accessors read. `timeout 5 ./target/debug/vimbatim` launched and ran
  the full timeout without a panic.
- **Still needs real-keyboard confirmation** that the indicator actually
  renders these strings correctly in the live UI (font, spacing,
  `[recording @x]` not clipping) — this sandbox has no working display.

## Task H: Command Mode (`:`) + Registers

Implements spec §5.7 (Command mode) and §5.8 (registers) in full. Six
sub-tasks, each TDD'd independently; 392 tests passing (up from 382 at the
end of Task G's `/simplify` pass).

### Command-mode text capture (`src/state.rs`)

Task D left Command mode entry/exit only. Added a new, dedicated
`tab.vim_command_line: String` field — deliberately *not* a reuse of
`vim_command_buf`, since that buffer has its own digit+single-trailing-
trigger-char parser (`split_vim_command_buf`) that arbitrary text like
`%s/foo/bar/g` would break; the same collision-avoidance reasoning Task F
used to keep `vim_pending_operator` in its own field.

`handle_vim_command_key` now: discards and exits on `Escape`; dispatches
via `dispatch_vim_command` then exits on `Enter`; pops the last character
(or exits to Normal if already empty, matching real vim) on `Backspace`;
and otherwise resolves the keystroke to a literal character via
`vim_find_target_char` — the same resolver already proven correct for
shifted punctuation (f/F/t/T targets, macro registers) on this GPUI
backend, which sidesteps the separately-tracked "shifted punctuation
doesn't type right anywhere in the app" gap for this one feature without
needing to fix it app-wide.

The mode indicator (`text_editor.rs`) now shows `:<command_line>` while in
Command mode, and appends a new `tab.vim_command_error` (see below) in any
mode until the next `:` is opened — mirroring real vim's persistent error
line.

### Command dispatcher (`dispatch_vim_command`, `src/state.rs`)

Every command in spec §5.7:

- `:w`/`:wa` — `save_active_tab`/loop `save_tab(idx)` (the latter is a new
  index-taking core `save_active_tab` now wraps, so `:wa` doesn't need to
  juggle `active_tab`).
- `:q` — refuses with `E37: No write since last change` (set into
  `vim_command_error`) if the active tab is modified; `:q!` force-closes
  regardless. **Scope decision, confirmed with the user before
  implementing**: spec §5.7 literally says "prompt if unsaved", but real
  vim doesn't pop a confirmation dialog by default — it just refuses with
  an error unless `:q!` overrides it. Implementing the error-refusal
  behavior matches real vim and needed no new prompt/modal UI; building an
  actual confirm dialog was the rejected alternative.
- `:wq`/`:x` — save then close. Both behave identically (`save_tab`
  already no-ops when `!is_modified`, which is exactly `:x`'s "only write
  if there were changes" semantics — no special-casing needed).
- `:e <path>` — resolves relative to `working_directory`, calls the
  existing `open_file`.
- `:set vim`/`:set novim` — toggles `vim_enabled`.
- `:<n>` — jumps to line `n` (1-indexed) via the existing
  `vim_move_to_line_first_nonblank`.
- `:%s/pattern/replacement/[g][i]` — `dispatch_vim_substitute`, using the
  `regex` crate (already a dependency). Per-line: without `g`, only the
  first match per line is replaced (`Regex::replace`), matching real vim's
  default; `i` prepends `(?i)` to the pattern for case-insensitivity. A bad
  pattern sets `vim_command_error` instead of panicking.
- `:noh` — accepted but a documented no-op; nothing to clear until Task I
  builds search highlighting.
- Anything else sets `vim_command_error` to `E492: Not an editor command:
  <cmd>`.

### Registers (§5.8, `src/state.rs` + `src/text_editor.rs`)

The `HashMap<char, String>` on `AppState` (from Task F, write-only until
now) is read from as well as written to:

- **`"<register>` prefix syntax**: `tab.vim_pending_register_select: bool`
  arms on a bare `"`; the next keystroke (`a`-`z`, `0`, `+`) selects
  `tab.vim_selected_register: Option<char>`, a *one-shot* selection
  consumed by `take_vim_selected_register` the next time a register-
  writing (`d`/`y`/`c`) or register-reading (`p`/`P`) action runs. Checked
  in `handle_vim_normal_key` and `handle_vim_visual_key` via a shared
  `try_handle_vim_register_prefix`, in the same "claims its key before
  anything else gets a chance" position as the existing macro-register-
  pending check — same pattern, different trigger key, no collision.
- **Write path**: `write_vim_register(text, also_yank)` replaces the old
  direct `registers.insert('"', ...)` calls in `execute_vim_operator_range`
  — always writes `'"'` (and `'0'` for yanks), plus the selected named
  register if one was given, mirroring real vim's "named register writes
  always also update the unnamed one" behavior. Visual mode's operators
  already route through `execute_vim_operator_range`, so register support
  there came for free — only the prefix-detection needed adding.
- **`p`/`P` paste**: new `vim_paste_register(before: bool)`. Linewise vs.
  charwise is read directly off the register text — "does it end with
  `\n`" — rather than tracked as separate state, since every linewise
  operator range already ends with a trailing newline by construction
  (`linewise_bounds_for_operator`). Linewise pastes as a new line below
  (`p`)/above (`P`) the cursor's line, landing on the first non-blank;
  charwise inserts right after (`p`)/at (`P`) the cursor, landing on the
  last pasted character (via `char_indices` for UTF-8 safety, never raw
  byte arithmetic).
- **`'+'` clipboard register**: stored in `registers` like any other named
  register — `state.rs` stays entirely GPUI-unaware. `text_editor.rs`
  handles both directions, mirroring the existing Ctrl+C/V clipboard
  pattern: before dispatching a `p`/`P` keystroke, if
  `state.vim_selected_register() == Some('+')` (a new peeking accessor,
  non-consuming — distinct from the internal consuming
  `take_vim_selected_register`), it reads `cx.read_from_clipboard()` and
  stages the text into register `'+'` via a new `set_register` setter, so
  the ordinary paste path then just works unmodified. In the write
  direction, `execute_vim_operator_range` stages written text into a new
  `AppState.pending_clipboard_sync: Option<String>` mailbox whenever the
  target register was `'+'`; `text_editor.rs` drains it
  (`take_pending_clipboard_sync`) right after every vim keystroke and
  pushes it onto the OS clipboard via `cx.write_to_clipboard`.

### Verification

- `cargo check`/`cargo build`: clean.
- `cargo test`: 392 passed, 0 failed, up from 382 (31 new tests across all
  six sub-tasks: command-mode capture, the dispatcher's ~15 commands,
  `:%s` substitution's flag combinations, register-prefix selection and
  one-shot consumption, and all four paste variants).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: the `'+'` clipboard round-trip
  (`cx.read_from_clipboard`/`cx.write_to_clipboard`) and the mode
  indicator's live rendering of `:command` text and `vim_command_error`
  strings have no GPUI test harness in this sandbox (no `test-support`
  feature wired in, no working display) — needs a real-keyboard pass.

## Task I: Remaining Normal-Mode Commands + `.` Repeat

Implements the rest of spec §5.5 not already covered by earlier tasks:
`x`/`X`/`r<char>`/`R`/`s`/`S`/`~`/`.`/`J`/`/`/`?`/`n`/`N`/`*`/`#`/`Ctrl+o`/
`Ctrl+i`. Six sub-tasks, each TDD'd independently; 435 tests passing (up
from 427 at the end of Task H).

### x/X/s/S/~/J (`src/state.rs`, `src/text_editor.rs`)

Convenience single-key commands reusing existing operator machinery:
`vim_delete_char_forward`/`_backward` (`x`/`X`, clamped to the current
line so they never delete a trailing newline), `vim_substitute_char`/
`_line` (`s`/`S`, `x`/line-clear + `vim_enter_insert_before_cursor`),
`vim_toggle_case_char` (`~`, reuses Task G's `toggle_case_vim_range`),
`vim_join_lines` (`J`, collapses the next line's leading spaces/tabs to a
single space).

**Real bug found and fixed along the way**: `text_editor.rs`'s `j`/`k`
visual-row-movement interception (built for Task E) checked `key == "j" ||
key == "k"` without checking `shift` — meaning `J` (shift+j, "join lines")
would have been silently swallowed as "move down one line" before ever
reaching `handle_vim_key`. Caught by writing `J`'s own test before the
fix existed to make it pass. Fixed by adding `&& !shift` to that
interception's guard.

### r<char> (`src/state.rs`)

A one-shot `tab.vim_pending_replace: bool`, mirroring the existing
macro-register-pending pattern, checked at the same priority as a pending
operator (must claim its next keystroke unconditionally). Doesn't touch
any register — matches real vim.

### R — Replace mode (`src/state.rs`, `src/text_editor.rs`)

**Scope decision, confirmed with the user before implementing** (this
section of `vim_todo.md` flagged a real spec gap — `editor_instructions.md`
never defines a Replace mode): added a real `VimMode::Replace` variant
rather than treating `R` as out of scope. Typing overwrites the character
under the cursor (falling back to `insert_char`, i.e. appending, once the
cursor reaches the line's end — matches real vim's ability to extend a
line's length in Replace mode). `Backspace` moves the cursor back without
restoring the overwritten character — a documented simplification (real
vim tracks per-position originals).

### Search mode: `/`, `?`, `n`, `N`, `*`, `#` (`src/state.rs`, `src/text_editor.rs`)

A new `VimMode::Search`. Rather than duplicating Command mode's text
capture, extracted a shared `capture_vim_line_input` state machine (an
enum `VimLineInput { Consumed, Dispatch(String), Cancelled }`) that both
`handle_vim_command_key` and the new `handle_vim_search_key` call —
`Escape`/`Enter`/`Backspace`/character-capture are identical between the
two modes; only what happens with the finished text differs (dispatched
as a command vs. a search). `dispatch_vim_search`/`jump_to_search_match_from`
implement a minimal `content.find`/`rfind`-with-wraparound (not a regex
search — a full inline find-bar with highlighting is spec §4.6 territory,
explicitly out of scope here). `AppState.last_search: Option<(String,
bool)>` remembers the pattern+direction for `n`/`N` (`N` reverses it).
`*`/`#` extract the word under the cursor via Task F's `text_object_word`,
searching from just past/before its bounds so the word the cursor is
already standing in doesn't match itself.

### Jump list: `Ctrl+o`/`Ctrl+i` (`src/state.rs`, `src/text_editor.rs`)

A per-tab back/forward stack pair (`vim_jump_back`/`vim_jump_forward:
Vec<usize>`, the same shape as `undo_stack`/`redo_stack`). Rather than
special-casing every "large jump" command individually, the push check
lives in the single application point *every* Normal-mode motion already
passes through — `apply_vim_motion` — comparing the old and new cursor's
line numbers (via a new `line_index_for` helper) and pushing whenever the
delta exceeds one line. This automatically covers `gg`/`G`/`:<n>`/every
search dispatch (all of which call `apply_vim_motion` internally) with no
per-call-site wiring. Visual-mode selection extension never pushes.

### `.` repeat (`src/state.rs`, `src/text_editor.rs`)

The most involved piece. `AppState.last_change: Option<VimChange>`, a
semantic enum:

```rust
enum VimChange {
    Operator(char, Vec<RecordedVimKey>),
    OperatorInsert(char, Vec<RecordedVimKey>, String),
    Insertion(String),
}
```

Deliberately **not** a raw full-keystroke-sequence replay (unlike macros)
— storing the operator char plus just its *completion* keystrokes means
`.` can call `start_vim_operator`/`complete_vim_operator` programmatically
at the new cursor position and let the existing motion/text-object
resolution machinery re-resolve fresh, which is what makes `dw` at a new
position correctly delete a *different* word rather than replaying a
stale byte range.

Capture mechanics:
- `start_vim_operator` begins a `vim_change_recording: Vec<RecordedVimKey>`
  for every operator except `y` (yanking isn't a "change").
  `text_editor.rs`'s `process_key` appends each keystroke to it *before*
  dispatch (unlike macro recording's after-the-fact check — the keystroke
  that *completes* the operator must still be captured).
- `execute_vim_operator_range` commits the recording to `last_change` once
  the operator actually runs — except `c`, which stashes it in
  `vim_pending_change_before_insert` instead, since the Insert session it
  leads into needs to be combined into the same change (real vim's `.`
  after `cw<text><Esc>` repeats both the deletion and the retyped text).
  The `NotAMotion`/`NeedsGpui` abandon path in `complete_vim_operator`
  discards the recording instead of committing it.
- `vim_enter_insert_before_cursor` (the single choke point every Insert
  entry — `i`/`I`/`a`/`A`/`o`/`O`/`c` — funnels through) starts a
  `vim_insertion_recording: String`; `insert_char`/`insert_str`/`backspace`
  append/pop it. `vim_exit_to_normal` commits it on exit from Insert mode,
  combining with `vim_pending_change_before_insert` if present.

**Scope limitation, matching this section's original guidance** ("i/a/c-
style insertions... don't try to replay arbitrary multi-command
sequences"): `o`/`O` also start an insertion recording (since they share
the same entry point), but replaying it via `.` inserts the text inline
rather than reopening a new line first — real vim's `.` after `o<text>
<Esc>` needs to remember *that a line was opened*, not just what was
typed, which this design doesn't capture. Documented rather than silently
wrong.

**A real regression caught by the test suite before shipping**: the first
version of `clear_vim_pending_operator` discarded `vim_change_recording`
unconditionally — but that function runs on *every* operator-completion
path (success *and* abandonment), always *before*
`execute_vim_operator_range`, which needs the recording still intact to
commit it. Every successful operator would have silently failed to set
`last_change`. Fixed by moving the discard to specifically the
`NotAMotion`/`NeedsGpui` abandon branch in `complete_vim_operator`, leaving
`clear_vim_pending_operator` itself untouched.

### Verification

- `cargo check`/`cargo build`: clean.
- `cargo test`: 435 passed, 0 failed, up from 427 (43 new tests across all
  six sub-tasks).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: `R`/Search mode's live indicator rendering,
  and the real-keyboard feel of `.` repeat, have no GPUI test harness in
  this sandbox — needs a real-keyboard pass, same caveat as every other
  UI-facing piece this session.

## Rich Text Formatting — Phase 1 (Display + Format-Sync Infrastructure)

Full plan in `notes/formatting_todo.md`. All of vim_todo.md's Tasks A-I
were done first; this is a separate feature area (spec §6/§7) planned via
`superpowers:brainstorming` with three explicit scope decisions confirmed
with the user: (1) two-phase build, Phase 1 (this) before Phase 2
(editing operations); (2) attribute scope extended beyond the literal spec
text to include italic/font-family/color, not just bold/underline/
highlight/size; (3) Phase 2's formatting operations will target
`tab.selection`, vim-mode-aware. Seven sub-tasks, each TDD'd
independently; 490 tests passing (up from 435 at the end of Task I).

### Data model refactor (`src/docx_parser.rs`, `src/state.rs`)

`DocxDocument` (which bundled `paragraphs` inside an `Arc`, deliberately
non-`Clone` for cheap sharing) couldn't support the core requirement of
this feature — mutating paragraphs on every keystroke. Split into:
`DocxOrigin { raw_zip, preamble, sect_pr }` (still `Arc`-wrapped, genuinely
immutable for a tab's lifetime) and a new, always-populated
`Tab.paragraphs: Vec<Paragraph>` (live, mutated in sync with every edit).
`parse_docx` now returns `(Vec<Paragraph>, DocxOrigin)`. `Run` gained
`italic: bool`, `font: Option<String>`, `color: Option<String>` (docx hex),
plus `Clone`/`PartialEq` derives (needed for undo/redo snapshots).
`default_paragraphs()` (one empty paragraph, one default run) is the
starting state for tabs with no parsed docx.

### Parser extension (`src/docx_parser.rs`)

`apply_run_prop` gained `<w:i>` → italic, `<w:rFonts w:ascii="...">` →
font (East Asian/complex-script overrides out of scope), `<w:color
w:val="...">` → color (`"auto"` treated as absent). `rebuild_document_xml`
re-emits all three. First tests ever written directly against
`docx_parser.rs`'s internals (`parse_document_xml`/`rebuild_document_xml`
had none before) — hand-built minimal XML fixtures, round-trip assertions.

### `resolve_position` (`src/document_ops.rs`, new file)

The core primitive: resolves a byte offset into `content` into
`(paragraph_index, run_index, char_offset)` against the live `paragraphs`.
Paragraph boundaries are exactly line boundaries (confirmed from
`paragraphs_to_plain_text`'s own join-by-`\n` behavior — no soft-break
ambiguity to resolve). A position landing exactly at a paragraph/run
boundary resolves to the *end of the earlier* one, not the start of the
next — so typing right before a paragraph break continues that
paragraph's formatting rather than adopting the next one's.

### Choke-point mutation sync (`src/state.rs`, `src/document_ops.rs`)

`sync_insert_char`/`sync_insert_str`/`sync_delete_range` (plus
`split_paragraph_at` and `merge_adjacent_same_format_runs`, all in
`document_ops.rs`) wired into the small set of functions every higher-level
edit already funnels through: `insert_char`, `insert_str`, `backspace`,
`delete_selection_raw`, and — since `indent_vim_range`/`change_case_vim_range`/
`toggle_case_vim_range`/`delete_vim_range` all already call
`replace_vim_range` — that single function covers `d`/`c`/`x`/`s`/`>`/`</gU`/
`gu`/`~`/`r`/`J` at once. `vim_paste_register` (raw `content.insert_str`
calls, bypassing the choke points) needed its own explicit sync calls.
`dispatch_vim_substitute` (`:%s`) is a documented scope limit: a regex
substitution has no clean per-character mapping back to original runs, so
only paragraphs whose text actually changed get replaced with a single
default run — untouched paragraphs keep their formatting exactly.
Adjacent same-format runs are merged after every deletion (otherwise
repeated edits would accumulate more and more needlessly-split runs).

### Undo/redo integration (`src/state.rs`)

`undo_stack`/`redo_stack` changed from `Vec<String>` to
`Vec<(String, Vec<Paragraph>)>` — paired snapshots, so undo can't restore
old text while leaving stale/shifted formatting attached to it. Every
pre-existing undo/redo test needed migrating from comparing
`tab.undo_stack` directly to a new `undo_contents()`/`redo_contents()` test
helper that extracts just the content half (most of those tests predate
this feature and only care about content, not formatting).

### Rendering (`src/text_editor.rs`)

Replaced uniform per-line styling with per-run styling via GPUI's
`Styled` trait (`font_weight`/`italic`/`underline`/`bg`/`text_size`/
`font_family`/`text_color` — all confirmed to exist directly on `Div`,
so no need to introduce GPUI's separate `TextRun`/`StyledText` API
alongside this codebase's existing div-per-segment rendering style).
The real integration risk (flagged explicitly in the plan before writing
any code): formatting-run boundaries and the *existing*
cursor/selection-overlay splitting (`line_segments`, unchanged) both want
to split the same line into spans. Resolved by composing them as an outer
run-level split (via new `paragraph_run_char_spans`, char-column space to
match `line_segments`' existing coordinate system) with `line_segments`
called *within* each run's own sub-range — the two concerns stay
orthogonal instead of needing one function that understands both.
`highlight_color_hex` (spec 6.2's 15-entry table) and
`heading_font_size_px` (spec 6.5) are both pure and unit-tested directly;
the actual GPUI div-building is smoke-tested only (`timeout 5
./target/debug/vimbatim`), consistent with every other rendering-adjacent
piece this session, since this sandbox has no display. **Known
open risk, not yet hardware-verified**: a heading's larger font size could
visually overflow the fixed `LINE_HEIGHT_PX` every row's click/scroll pixel
math assumes — row height doesn't adjust for it.

### Save integration — the actual bug fix (`src/state.rs`, `src/docx_parser.rs`)

`save_tab` now saves `tab.paragraphs` directly via `DocxOrigin::save`
(renamed from the old `DocxDocument::save`) instead of regenerating plain
unstyled paragraphs from `content` via the now-deleted
`save_from_content`/`content_to_paragraphs`. This is the fix for
`editor_instructions.md` line 82's named simplification ("open preserves
formatting read-only; any edit produces plain paragraphs on save") — an
edited, loaded docx now keeps its formatting on save. `create_new_docx`
was also changed to take `&[Paragraph]` instead of `&str`, so a brand-new
tab's formatting (once Phase 2 exists) round-trips too; its one other call
site (`file_explorer.rs`'s "new file" button) updated to pass
`default_paragraphs()`.

### Verification

- `cargo check`/`cargo build`: clean.
- `cargo test`: 490 passed, 0 failed, up from 435 (55 new tests: parser
  round-trips, `resolve_position`/sync-primitive unit tests, choke-point
  integration tests asserting `tab.paragraphs` stays correct through real
  editor operations — not just `tab.content` — undo/redo pairing, and the
  two pure rendering-helper functions).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: the actual visual rendering (bold looking
  bold, highlight colors, heading sizes, the cursor/selection-over-
  formatting layering) has no GPUI test harness in this sandbox — needs a
  real-keyboard/real-display pass, same caveat as every other UI-facing
  piece this session.

## Rich Text Formatting — Phase 2 (Formatting Editing Operations)

Full plan in `notes/formatting_todo.md`. Builds directly on Phase 1's
synced-`paragraphs` model. Two open questions the plan document flagged
explicitly (not guessed at) were resolved with the user before
implementing: (1) the no-selection case implements a real pending-format-
for-next-typing mechanism, not a no-op; (2) CARD STYLES/STRUCTURE stayed
explicitly out of scope (documented gap, same pattern as `vim_todo.md`'s
`R` gap) — only MARKUP/CLEAN/SIZE got wired. Four sub-tasks; 512 tests
passing (up from 490 at the end of Phase 1).

### `apply_formatting` core + `FormatOp` (`src/document_ops.rs`)

`FormatOp` (`Bold`/`Italic`/`Underline`/`Highlight`/`FontSize`/
`FontFamily`/`Color`/`ClearAll`) and `apply_formatting(paragraphs, start,
end, op)` per spec 7.2. Splits runs at both boundaries first (`split_run_
at_position`, END before START so START's already-resolved indices don't
get invalidated by a run being inserted ahead of it), then re-walks every
run's absolute byte range and formats whichever ones fall fully inside
`[start, end)` — simpler and more robust than trying to track exactly
which indices the two splits shifted. Reuses Phase 1's `merge_adjacent_
same_format_runs` afterward so formatting a range that happens to match
its neighbors' styling doesn't leave needlessly-fragmented runs behind.

### Selection-target dispatch + pending format (`src/state.rs`)

`apply_formatting_to_selection(op)`: with an active selection, applies
`op` directly (its own undo snapshot, paired content+paragraphs per Phase
1). With no selection, arms/disarms a new `Tab.pending_format: Option<
FormatOp>` instead — pressing the *same* action again while already
pending turns it back off (a toggle button's usual behavior). `insert_char`
consumes it: after each typed character, if `pending_format` is `Some`,
applies it to that character's exact range. It persists across multiple
keystrokes (not just one character) until explicitly toggled off again.
**Documented simplification**: a single slot, not a set — arming a second
format while one is already pending replaces it (real Word can have
several pending toggles at once, e.g. bold *and* italic simultaneously;
this can only have one). Only `insert_char` consults it — `insert_str`
(paste) does not.

### Ribbon UI (`src/formatting_ribbon.rs`)

**Real discovery, not assumed**: this file already existed in full — the
CARD STYLES/MARKUP/CLEAN/STRUCTURE/SIZE button layout, `FormatAction` enum,
and all rendering were already built; only every button's `on_click`
was a `println!` stub. `FormatAction::to_format_op()` maps the character-
formatting actions (Und/HLt/HLg/Bold/Rm HL/Clean/Shrink/Normal, plus a new
Ital button added for the italic scope extension) to a `FormatOp`,
`None` for the still-stubbed CARD STYLES/STRUCTURE actions. `render_group`
now takes `cx: &mut Context<Self>` and uses `cx.listener` (the same
pattern every other clickable view element in this codebase already uses)
to call `apply_formatting_to_selection` for mapped actions, falling back
to the original stub for unmapped ones. `FormattingRibbon` gained a
`state: Entity<AppState>` field (didn't have one at all before) and its
constructor signature changed accordingly — `main_window.rs` updated.
`Shrink`/`Normal`'s point sizes use fixed defaults (10pt/12pt) rather than
`settings.conf`'s `small_size`/`large_size` fields, since those were never
wired into `AppState` in the first place — a separate, pre-existing gap
(same class as the `vim` flag never being read at startup), not silently
expanded here. Font-family and color pickers (the other two extended-scope
attributes) are not yet in the ribbon — a real UI primitive this codebase
has no precedent for, deferred rather than built hastily.

### Keyboard shortcuts (`src/text_editor.rs`)

`Ctrl+B` (Bold) and `Ctrl+U` (Underline) alongside the ribbon, in
`process_key_ctrl_combo`. **`Ctrl+I` is deliberately not italic** — the
conventional shortcut collides with real vim's own `Ctrl+I` (jump list
forward, spec 5.5, added in Task I), which takes priority since it's
existing, tested vim functionality; italic stays ribbon-only.

### Verification

- `cargo check`/`cargo build`: clean.
- `cargo test`: 512 passed, 0 failed, up from 490 (22 new tests: `apply_
  formatting`'s split/merge/multi-paragraph behavior, `apply_formatting_
  to_selection`'s selection and pending-format paths including the toggle-
  on/toggle-off/replace cases, and the ribbon's `FormatAction`→`FormatOp`
  mapping).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: clicking ribbon buttons, the shortcuts, and
  pending-format's visual feedback (there is none yet — no UI indicator
  shows a pending format is armed, unlike vim mode's own indicator line)
  have no GPUI test harness in this sandbox — needs a real mouse/keyboard
  pass. The missing pending-format indicator is itself worth flagging as a
  small, real UX gap for a future pass.

All of `notes/formatting_todo.md`'s planned work (Phases 1 and 2) is now
complete.

## Rich-text formatting bug fixes (post-hardware-testing)

Three issues reported after real-hardware testing of the above feature,
investigated via `systematic-debugging` (root cause traced through the
actual vendored GPUI/cosmic-text/fontdb source before any fix, not guessed
by analogy) and fixed:

### 1. Bold/Italic weren't visible

**Root cause**: `text_editor.rs` requested the font family `"monospace"` —
a generic CSS-style alias, not any real font file's own declared family
name. GPUI's `cosmic_text_system.rs::load_family` filters real system
fonts by an *exact literal string match* against each font's embedded
family metadata, so `"monospace"` never matches directly. Additionally
(and independent of the above), `find_best_match`'s
`if candidates.len() == 1 { return Ok(0); }` short-circuit skips
weight/style matching entirely whenever the resolved family has only one
loaded face — silently returning that one face regardless of the
requested bold/italic. Together, this explains why `.bg()`-based styling
(highlight, cursor, selection) worked while `.font_weight()`/`.italic()`
didn't: only the latter depends on more than one face actually resolving.
**Fix**: added `FONT_FAMILY = "DejaVu Sans Mono"` (a literal, real font
family name that ships with separate Book/Bold/Oblique/Bold Oblique faces
on essentially all Linux/WSL systems) and replaced all 4 hardcoded
`"monospace"` call sites with it, so `find_best_match` has real candidates
to choose between. **Not independently hardware-verified** — this
sandbox has no display; needs a real run to confirm bold/italic now
render and that regular text still looks correct/monospaced.

### 2. Highlight (and Bold/Italic/Underline) didn't toggle off on re-click

**Root cause**: not a data bug — `RemoveHighlight`'s
`FormatOp::Highlight(None)` mapping was already correct and tested.
`apply_formatting_to_selection` simply had no toggle-detection logic for
the has-a-selection path (only the no-selection/`pending_format` path
toggled) — every ribbon markup button unconditionally re-applied its "on"
state even when the whole selection was already in it, unlike a toolbar
button's expected press-again-to-release behavior.
**Fix**: added `is_uniformly_active(paragraphs, start, end, &op)` and
`toggled_off(&op)` to `document_ops.rs` — the former reads whether every
run touching `[start, end)` is already in `op`'s "on" state (checking
highlight *color* equality, not just presence, so clicking green on
yellow-highlighted text still applies green rather than toggling it off);
the latter maps `Bold/Italic/Underline/Highlight`'s "on" variant to its
"off" one. `apply_formatting_to_selection` now applies `toggled_off(&op)`
instead of `op` when the selection is already uniform.

### 3. White text on a light highlight (e.g. yellow) was illegible

New feature request, not a bug: darken a highlight color when both it and
the effective text color are perceptually light, matching Word's own
dark-mode highlight behavior. Added `relative_luminance` (BT.709
0.2126/0.7152/0.0722 weighting), `is_light_color` (luminance > 0.5), and
`darken_for_light_text` (scales each RGB channel by 0.4, preserving hue)
to `text_editor.rs`. Wired into `apply_run_style`'s highlight branch: the
effective text color is `run.color` if set, else the editor's default
`0xd4d4d4`; if both it and the resolved highlight color are light, the
highlight is darkened before being applied via `.bg()`. Unconditional
(not gated on a light/dark-mode setting) since this app has no such
toggle and is always dark-themed.

### Verification

- `cargo build`: clean, same 6 pre-existing dead-code warnings as before
  this change (none new).
- `cargo test`: 530 passed, 0 failed, up from 512 (18 new tests: luminance/
  toggle-detection pure-function tests plus one state.rs integration test
  for the toggle-off path).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: the font-family fix (item 1) and the visual
  appearance of the darkened highlight (item 3) both need a real display
  to confirm — this sandbox cannot render GPUI's window.

## Configurable Keybinds (settings modal, formatting branch)

Lets the user remap every non-vim hotkey through a GUI in the settings
modal (Ctrl+,), persisted to settings.conf, with duplicate detection and
a reset-to-defaults option. Built per an explicit requirement: extendable
to any future keybind with minimal touch points.

### What Was Built

**`src/keybinds.rs` (new module):**
- `KeyCombo` — a modifier+key struct, stored platform-neutral (`ctrl`
  always means "the primary modifier"; Ctrl→Cmd substitution happens only
  at the edges: `to_gpui_keystroke()` (GPUI's hyphenated syntax, e.g.
  `"ctrl-shift-b"`, substituting `cmd` on macOS), `display_string()` (UI
  label, e.g. `"Ctrl+Shift+B"` or `"Cmd+Shift+B"`), and `from_capture()`
  (builds a combo from a live `KeyDownEvent`, treating a Mac's Cmd key as
  satisfying the `ctrl` slot). `parse()`/`to_conf_string()` handle
  settings.conf's existing space-separated format (`"CTRL SHFT b"`).
- `KeybindCategory` — the six groupings shown in the settings UI: General,
  Editing, Text Formatting, Card Styles, Highlighting, Caselist Tools.
- `KeybindAction` — the canonical enum of every bindable, non-vim action
  (32 variants). Each has `label()`, `category()`, `conf_key()` (the exact
  settings.conf key name), `default_combo()`, and `is_stub()` (true for
  actions with no real implementation yet — Save As, Find, Find & Replace,
  Delete Tags, Start Timer, Open Stats, Cite From Link — matching this
  codebase's existing Doc Menu/Card Menu placeholder convention).
- `Keybinds` — the registry mapping each action to its current combo:
  `load()`/`defaults()`, `get()`/`set()`, `find_conflict()` (duplicate
  detection), `save_to()` (rewrites only the file's `[KEYBINDS...]`
  portion, leaving `[FORMATTING]` and the standalone `vim_lines` flag
  byte-for-byte untouched).
- `load_vim_enabled()` — reads the standalone `vim` boolean (not a
  KeybindAction — a mode toggle, not a key combo).
- `rebuild_keymap(cx, keybinds)` — clears and rebuilds the entire GPUI
  keymap from a `Keybinds` instance. Callable at runtime (confirmed via
  `App::clear_key_bindings`/`App::bind_keys`, not just at startup), so the
  settings modal calls it immediately after every remap.
- 32 zero-sized GPUI action structs (`actions!` macro) — one per
  `KeybindAction`, the single place all of them are declared.

**Real, pre-existing bug fixed as a side effect:** GPUI stops an event's
propagation once a keybinding's action fires, so the raw `KeyDownEvent`
never reaches a view's own `on_key_down` after a match. `Ctrl+B` was bound
both to `ToggleSidebar` (a real GPUI action) and hardcoded "Bold" (a raw
key match in `text_editor.rs`) — the action always won silently, so
**Ctrl+B never actually bolded text**, only toggled the sidebar. Moving
Bold/Underline/Copy/Cut/Paste/Undo/Redo/SelectAll into real, independently
configurable GPUI actions removes the shadowing; adopting settings.conf's
own `sidebar=CTRL SHFT b` resolves the clash (Bold keeps Ctrl+B, Sidebar
becomes Ctrl+Shift+B).

**`src/state.rs`:**
- Added `CardStyleKind` enum (Pocket/Hat/Block/Tag) + `AppState::
  apply_card_style(kind)`, extracted from `formatting_ribbon.rs`'s
  previously-inline card-style button logic so both the ribbon and the
  new keybind actions share identical behavior instead of duplicating it.
- Added `pub keybinds: Keybinds` field; `vim_enabled` is now loaded from
  settings.conf's `vim` flag via `keybinds::load_vim_enabled` instead of
  being hardcoded `true` (a gap flagged repeatedly since Task D of the vim
  mode work).

**`src/formatting_ribbon.rs`:** card-style button handlers (Pocket/Hat/
Block/Tag) now call `state.apply_card_style(kind)` instead of inline
per-button logic; `card_style_size()` removed (dead — superseded by
`CardStyleKind::font_size()`).

**`src/main.rs`:** loads `Keybinds::load("settings.conf")` and calls
`rebuild_keymap` once at startup, replacing the old hardcoded
`cx.bind_keys([...])` block of 5 actions.

**`src/main_window.rs`:** ~30 new `.on_action` handlers (Copy, Cut, Paste,
Undo, Redo, SelectAll, Bold, Underline, Shrink, ClearFormatting,
PasteSmart, Condense, Pocket, Hat, Block, Tag, Cite, Emphasis, Highlight,
SaveAs/Find/FindReplace/DeleteTags/StartTimer/OpenStats/CiteFromLink
stubs, Wikifi), registered on the root div alongside the existing
ToggleSettings/ToggleSidebar/Save (now retargeted to the new action
structs from `keybinds.rs` instead of a locally-declared, now-removed
`actions!` block).

**`src/tab_bar.rs`:** its own local `NewTab`/`CloseActiveTab` actions
removed; handlers retargeted to `keybinds::NewTabAction`/`CloseTabAction`.

**`src/text_editor.rs`:** removed the now-dead hardcoded `"c"|"x"|"v"|"a"|
"z"|"y"|"b"|"u"` match arms from `process_key_ctrl_combo` (they were
already permanently shadowed by GPUI actions per the bug above, wherever
one existed — now made explicit and correct instead of accidental). Vim
jump-list (`Ctrl+O`/`Ctrl+I`) and word/doc-jump Ctrl-combos stay
hardcoded — vim-specific, explicitly out of scope for the configurable
system.

**`src/settings_modal.rs`:** rebuilt from its placeholder body into a real
keybind editor:
- A Vim Mode on/off toggle row (off by default in `default_settings.conf`;
  unchanged — still `true` — in the user's live `settings.conf`, so this
  wiring introduces no behavior change today, only going forward).
- Six collapsible category sections (mirroring `formatting_ribbon.rs`'s
  own collapse-arrow convention), each listing its actions' current combo
  + a "Change" button.
- Capture flow: clicking "Change" arms `capturing: Option<KeybindAction>`
  and claims keyboard focus; the next keystroke resolves via
  `KeyCombo::from_capture`. **Non-obvious fix required here:** GPUI
  resolves an action's keybinding *before* delivering a raw `KeyDownEvent`
  to a view's `on_key_down` — so pressing an already-globally-bound combo
  while capturing (e.g. Ctrl+S while remapping something else) would
  silently fire *that* action (Save) instead of ever reaching the capture
  handler. Fixed via `App::intercept_keystrokes`, registered only while
  capturing, unconditionally calling `cx.stop_propagation()` for every
  keystroke — this suppresses normal action dispatch for that one event
  and routes it to `on_key_down` instead (confirmed against GPUI's own
  `Window::dispatch_key_event` source: an interceptor's `stop_propagation`
  short-circuits straight to `finish_dispatch_key_event`, skipping the
  `match_result.bindings` action-dispatch step entirely).
- Duplicate detection: a captured combo already in use shows "already used
  by "X"" inline on the row and stays in capture mode so the user can just
  try again; Escape cancels, keeping the existing binding.
- "Reset to Defaults" copies `default_settings.conf` over `settings.conf`,
  reloads both `Keybinds` and the vim flag, and rebuilds the live keymap.

**`settings.conf` / `default_settings.conf`:** reorganized into six
`[KEYBINDS: CATEGORY]` sub-headers (safe — the existing flat `key=value`
parser in `config_parsing.rs` skips every line starting with `[`
regardless of its exact text). Two typos fixed (`CRTL` → `CTRL`),
`Find_and_Replace` renamed `find_and_replace`. Six new keys added with
defaults matching their previously-hardcoded-only behavior (`close_tab`,
`copy`, `cut`, `paste_raw`, `undo`, `redo`, `select_all`).
`default_settings.conf`'s `vim` is now `false` (the user's stated default
preference); `settings.conf`'s stays `true` (no live behavior change).
`underline`'s value corrected from the stale, never-wired `f9` to the
real, working `CTRL u`.

### Verification

- `cargo test`: 555 passed, 0 failed (551 in the bin crate, incl. ~35 new
  for `keybinds.rs` — parsing, conflict detection, capture, default/real-
  file consistency checks — plus 4 for `apply_card_style`; 4 in
  `tests/parse_testing.rs`, updated to match the reorganized conf values).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic (same sandbox EGL/MESA limitation as every prior task
  in this document) — confirms no startup-time breakage from the keymap
  rebuild, `AppState`'s new fields, or the settings modal rewrite.
- **Not hardware-verified**: the settings modal's capture flow (clicking
  "Change", pressing a key, seeing the conflict message, Reset to
  Defaults) is GPUI interaction glue this sandbox's headless environment
  can't exercise. Confirm on a machine with a working display: open
  Settings, remap Bold to something else, confirm it applies immediately
  without restarting; try to bind an already-used combo and confirm the
  conflict message names the right action; toggle Vim Mode off/on; Reset
  to Defaults and confirm every binding reverts.

## Bugfix: Keybind Capture Never Actually Worked

The settings modal's "Change" button correctly armed capture mode (showed
"Press a key…"), but pressing a key never did anything — reported by the
user after the previous session's implementation.

### Root Cause

The original design used `App::intercept_keystrokes` + `cx.stop_propagation()`
to suppress an already-bound combo's action from firing while capturing,
intending the raw `KeyDownEvent` to still reach `on_key_down` afterward.
This doesn't work: GPUI's `Window::dispatch_key_event` uses the *same*
`propagate_event` flag for both action dispatch short-circuiting and the
subsequent `finish_dispatch_key_event`/`dispatch_key_down_up_event` raw-key
delivery, without resetting it in between. Setting it `false` in an
interceptor causes `dispatch_key_down_up_event`'s own capture-phase loop
(`if !cx.propagate_event { return; }`, checked after processing the first
node with any key listener) to bail out immediately — before ever reaching
the bubble-phase pass where `.on_key_down`'s wrapped listener actually
checks `phase == DispatchPhase::Bubble` and invokes the real callback. So
the interceptor broke *every* capture attempt, not just the already-bound-
combo edge case it was meant to handle.

Separately (confirmed while diagnosing): GPUI's action-dispatch loop
(`window.rs`'s `dispatch_key_event`) returns immediately once any matched
binding's action handler stops propagation (the default) — it never calls
`finish_dispatch_key_event` in that case. This means a keystroke already
bound to a live action can *never* reach `on_key_down` through propagation
tricks alone; the only way to let it fall through is to make the binding
itself not match in the first place.

### Fix

Switched to GPUI's `KeyContext` predicate system instead. `settings_modal.rs`
now conditionally tags its panel div with `.key_context("KeybindCapturing")`
only while `capturing` is armed. Every one of `rebuild_keymap`'s 32
`KeyBinding`s (`src/keybinds.rs`) now requires `Some("!KeybindCapturing")`
(previously `None`) — i.e. that context's *absence* — to match. So while
capturing, none of the app's keybindings match at all, regardless of which
key is pressed, and the keystroke falls through to `on_key_down` normally
via the ordinary "no binding matched" path. The broken
`intercept_keystrokes`/`Subscription` field was removed entirely.

### Also Fixed: Test Suite Fragility

Discovered while re-running tests after this fix: several tests asserted
*exact literal values* from the real, live `settings.conf` — but that file
is deliberately user-editable at runtime (the whole point of this feature).
The failure that surfaced it: `settings.conf`'s `vim` value had genuinely
changed to `false` (someone using the working Vim Mode toggle on a real
display), which is correct, expected behavior — not a bug — yet it broke
`real_settings_conf_has_vim_true` and two of `tests/parse_testing.rs`'s
`config_parsing`-based tests, which all hardcoded values against that same
mutable file.

- `keybinds.rs`: replaced `real_settings_conf_matches_defaults` (exact
  per-action value checks) and removed `real_settings_conf_has_vim_true`
  entirely; added `real_settings_conf_is_internally_consistent`, which only
  checks structural invariants (every action resolves to a combo, no two
  collide) that hold regardless of what's been customized.
  `default_settings.conf`'s equivalent tests (`real_default_settings_conf_
  matches_except_vim`/`_has_vim_false`) are kept as exact-value checks —
  that file is never written to by the running app (only read, or copied
  wholesale *into* settings.conf on Reset), so its content is a legitimate
  stable invariant.
- `tests/parse_testing.rs`: now reads a new fixture,
  `tests/fixtures/settings.conf` (a frozen snapshot, not the live file),
  instead of the project root's real `settings.conf` — fully decoupling
  this test suite from whatever the app has since persisted at runtime.

### Verification

- `cargo test`: 554 passed, 0 failed (550 bin + 4 integration).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: confirm on a real display that clicking
  "Change", pressing a new key, and seeing it apply immediately now
  actually works — this was the entire point of the fix and is exactly
  the kind of GPUI dispatch-order interaction this sandbox can't exercise.

## Bugfix: Ctrl+, (and every other configured keybind) only worked with the text editor focused

### Root Cause

`KeyBindingContextPredicate::eval_inner` (GPUI's own source,
`crates/gpui/src/keymap/context.rs`) short-circuits to `false` for *any*
predicate — including negations like `Not(...)` — the moment the context
stack passed to it is empty (`contexts.last()` is `None`), before it ever
looks at which predicate variant it's evaluating. Since every one of the
32 keybindings registered in `rebuild_keymap` uses the context predicate
`NOT_CAPTURING` ("!KeybindCapturing", added in the previous fix), each one
requires the context stack to be *non-empty* just to be evaluated at all
— `TextEditor` was the only view anywhere in the app with a
`.key_context(...)` call, so the stack was only ever non-empty while the
editor had focus. With focus anywhere else (sidebar, ribbon) or nowhere at
all (fresh app launch, before any click), the context stack was empty and
every configured keybind — Ctrl+, included — silently failed to match.

### Fix

Added `.key_context("App")` to `MainWindow`'s root div (`main_window.rs`).
This guarantees the dispatch path always contributes at least one
`KeyContext` tag regardless of what currently has focus, since the root
div is an ancestor of everything (or the fallback target when nothing has
focus at all) — so `!KeybindCapturing` (and any future context predicate)
now evaluates correctly everywhere, not just inside the text editor.

### Verification

- `cargo test`: 554 passed, 0 failed.
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- Also cleaned up four `clippy::bool_assert_comparison` lints in
  `tests/parse_testing.rs` (`assert_eq!(x, true/false)` → `assert!(x)`/
  `assert!(!x)`), flagged during this fix.
- **Not hardware-verified**: confirm on a real display that Ctrl+, now
  opens Settings when the sidebar, ribbon, or nothing at all has focus,
  not just when the text editor does.

## Bugfix (real root cause): Ctrl+, and every configured keybind only fired with the text editor focused

The previous "empty context stack" fix (adding `.key_context("App")` to
`MainWindow`'s root div) did not actually solve the problem — confirmed by
the user testing on a real display with that fix in place. Traced further
and found the true root cause.

### Root Cause

`.on_action(cx.listener(Self::handler))`, registered on a specific `div()`
in `render()`, is a **window-scoped** action listener — GPUI only invokes
it when that div's node is part of the *currently focused* dispatch path
(`Window::dispatch_action_on_node_inner` iterates `dispatch_path`, computed
from `Window.focus`). `TextEditor` was the only view in the app that ever
called `FocusHandle::focus()` to actually claim focus (on click); nothing
else — the sidebar, the ribbon, the tab bar — ever did. So the moment
focus was anywhere other than the text editor (or nowhere at all, e.g.
right after launch), `MainWindow`'s root div — and every `.on_action`
handler registered on it, all ~30 of them, Ctrl+, included — was simply
not on the dispatch path and never ran. The earlier `.key_context("App")`
fix targeted a real, separate quirk (an empty context stack fails every
context predicate, even negations) but didn't address this — the div
itself was never being visited during dispatch when focus was elsewhere,
regardless of what context tags it carried.

### Fix

Converted every configurable keybind action handler from
`.on_action(cx.listener(Self::handler))` (view/div-scoped) to
`App::on_action(...)` (registered globally on `App.global_action_listeners`,
once, in `MainWindow::new`). Confirmed against GPUI's own
`Window::dispatch_action_on_node_inner` source: its "Bubble phase for
global actions" block never reads `dispatch_path` at all — a global
listener fires for a matching action regardless of focus state,
unconditionally. This is architecturally the correct mechanism for
app-wide keyboard shortcuts (as opposed to view-local ones like
`TextEditor`'s own raw `on_key_down`, which legitimately *should* depend
on that view having focus).

One subtlety hit along the way: `Context<T>::on_action` has a completely
different, window-scoped signature (`(TypeId, &mut Window, listener)`)
that shadows `App::on_action` by name when called through a
`Context<MainWindow>` — even though `Context<T>` derefs to `App`, Rust
resolves the inherent method on the concrete type first and errors on
arity mismatch rather than falling through to the deref'd version. Fixed
by giving `register_global_actions` a `&mut App` parameter explicitly
(callers just pass their `cx`, which coerces).

All 32 action handlers (Copy/Cut/Paste/Undo/Redo/SelectAll/Bold/Underline/
card styles/etc., previously spread across `main_window.rs` and
`tab_bar.rs`'s `NewTab`/`CloseTab`) are now registered in one place —
`MainWindow::register_global_actions`, called once from `new()` — each
closure capturing its own clone of the shared `Entity<AppState>`. Removed
the now-pointless `.key_context("App")` tag and the earlier `NOT_CAPTURING`
context-predicate mechanism entirely (superseded by the previous commit's
switch to `cx.clear_key_bindings()` during capture, which needed no
context-tree cooperation to begin with).

### Verification

- `cargo test`: 554 passed, 0 failed.
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: this is the fix that should finally resolve
  the user's repeated real-display reports — confirm Ctrl+, (and ideally
  a couple of others, e.g. Ctrl+Z/Ctrl+B) now work with focus on the
  sidebar, the ribbon, or nothing at all, not just the text editor.

## Nav Menu (working navigation, formatting/main branches)

Implements `notes/ribbon_instructions.md`'s Nav feature per the design doc
at `docs/superpowers/specs/2026-07-07-nav-menu-design.md` — the ribbon's
Nav button previously printed to console and did nothing else; a Files/Nav
button pair also used to exist in the file explorer's own header (as a
Phase 1 placeholder) but was silently dropped in the `gui-polish` theme
redesign. Both are now real.

### What Was Built

**`src/state.rs`:**
- `SidebarMode` enum (`Files` default / `Nav`) + `AppState.sidebar_mode`
  field — which view the left sidebar (`FileExplorer`) currently shows.
- `AppState::apply_card_style` now sets `Paragraph.heading` (1=Pocket,
  2=Hat, 3=Block, 4=Tag) on the cursor's line, in addition to the run
  formatting it already applied. This closes a real, pre-existing gap:
  neither Wikifi export nor heading-based font sizing ever actually worked
  for a card style applied through this app, since both already read that
  same field and it was never being set. `content`/`paragraphs` are kept
  1:1 (one paragraph per line), so the paragraph index is just the count
  of newlines before the cursor.
- `Tab.pending_scroll_to_cursor: bool` + `AppState::jump_to_line(line)` —
  the mechanism that lets a click in `FileExplorer` (which has no direct
  reference to `TextEditor`, only the shared `AppState`) still scroll an
  off-screen heading into view. `jump_to_line` moves the cursor and arms
  the flag; `TextEditor::render()` checks and clears it on its next paint,
  calling its own (otherwise-private) `scroll_to_cursor()`. Ordinary
  in-editor navigation is unaffected — it already calls `scroll_to_cursor()`
  directly and never touches this flag.

**`src/file_explorer.rs`:**
- Restored the Files/Nav button pair in the header (left of refresh),
  wired to `AppState.sidebar_mode` for real this time (previous version
  just printed to console). `render_mode_toggle_btn` is a small shared
  helper for both halves — highlighted when active, click sets the mode.
- `render()` branches the whole body on `sidebar_mode`: `Files` is
  unchanged from before; `Nav` calls new `render_nav_tree()`, header
  title/subtitle switch to "Navigation"/active tab title, refresh/+
  buttons hidden (not applicable in Nav mode).
- `render_nav_tree()` walks the active tab's `content`/`paragraphs`
  together (same pairing `wikifi_export.rs` uses), collects every line
  with `heading` 1–4, and renders each indented `(heading - 1) * 16px`
  (Pocket flush left, Tag deepest) — nested by *type*, not document
  position, per the design doc. Text truncates with an ellipsis past the
  240px panel width (`.truncate()`). Clicking a row calls
  `AppState::jump_to_line`. Shows "No headings yet" instead of an empty
  scroll area when the active tab has none.

**`src/formatting_ribbon.rs`:** `FormatAction::Nav`'s handler toggles the
same `AppState.sidebar_mode` the file explorer's own buttons control, and
also sets `sidebar_visible = true` — "open the navigation tab" implies
making the sidebar visible if it's currently collapsed, not just switching
its internal mode while hidden.

**`src/wikifi_export.rs` / `src/state.rs` tests:** this function had zero
test coverage before. Added a dependency-free unit test in
`wikifi_export.rs` (hand-built headings 1–4 + body text) and an end-to-end
integration test in `state.rs` that drives the real
`AppState::apply_card_style` across a 5-line document and feeds the result
straight into `export_to_markdown` — proving the whole ribbon/keybind →
export pipeline works now, not just the export function in isolation.

### Verification

- `cargo test`: 558 passed, 0 failed (554 unit incl. 7 new for this
  feature — heading assignment per card style, correct-line targeting on
  a multi-line document, `jump_to_line`'s cursor+flag behavior, and the
  two Wikifi export tests above; 4 in `tests/parse_testing.rs`).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: this sandbox has no display. Confirm on a
  real machine: click the ribbon's Nav button and the sidebar's own Nav
  button (both should show the same heading outline); apply Pocket/Hat/
  Block/Tag to a few lines and confirm they appear correctly indented;
  click a heading that's scrolled off-screen and confirm the editor
  actually scrolls to it, not just moves the cursor invisibly; confirm
  Wikifi export now produces real markdown headings.

## Nav Menu Follow-up: Collapse Arrows + Always-Center Jump

Two refinements requested after the initial Nav implementation.

### 1. Collapse arrows

This required a real design change, not just an addition: the original
Nav tree indented purely by *type* (Pocket always depth 0, Hat always
depth 1, etc., "nested by type, not document position" — an explicit
choice in the original design doc). But "collapse the headings underneath
it" only has a well-defined meaning for *actual* document-structure
nesting — a Hat's real children are whatever Blocks/Tags follow it before
the next Hat or Pocket, not "every Block/Tag in the document." So this
follow-up switches Nav to real tree nesting.

**`src/file_explorer.rs`:**
- New free function `build_nav_entries(headings, collapsed) -> Vec<NavEntry>`
  — the classic "a heading's children are everything up to the next
  heading at an equal-or-shallower level" algorithm (same one Markdown/
  VSCode outline views use for a flat heading list), implemented with a
  single pass and a small level stack. `NavEntry.depth` is now actual tree
  depth (an orphan Tag with no preceding Pocket/Hat/Block sits at depth 0,
  not depth 3); `NavEntry.has_children` gates whether a row gets an arrow
  at all.
- New `FileExplorer.nav_collapsed: HashSet<usize>` (line indices) — view-
  only UI state, deliberately *not* threaded through `AppState` (unlike
  the file tree's own `FileNode::Dir.expanded`, which lives in `AppState`
  because file-tree structure is itself shared state — collapse state
  here isn't shared with or meaningful to anything else). Persists across
  re-renders naturally as a struct field; collapsing an outer heading
  doesn't clear an inner heading's own collapsed flag, so re-expanding the
  outer one doesn't silently re-expand the inner one too.
- Each row is now two separate sibling elements (an optional arrow div +
  the text div), not one div with a single click handler — the arrow
  toggles `nav_collapsed` via `cx.listener` (needs `&mut self`), the text
  calls `jump_to_line` via the same plain-closure-on-`state_handle`
  pattern used elsewhere. Siblings don't bubble into each other, so no
  `stop_propagation` juggling was needed.
- 7 new unit tests directly on `build_nav_entries` (pure, no GPUI
  context): flat siblings, strictly-nested levels, an orphan Tag's depth,
  a sibling correctly popping back to depth 0 after a deeper subtree,
  collapse hiding only its own subtree, nested collapse state surviving
  an outer re-expand, and leaf headings getting no arrow.
- Hit the same `gpui::*` / `#[test]` shadowing recursion-limit gotcha
  `text_editor.rs`'s test module already has a comment about — `use
  super::*` inside `mod tests` pulls in `gpui::*`'s own `test` macro,
  which shadows `std::test` and blows the recursion limit expanding it.
  Fixed the same way: import only the specific items needed, not `super::*`.

### 2. Always center on jump, don't just scroll-into-view

`scroll_to_cursor` (used by all *other* cursor movement) only nudges the
viewport when the cursor is near an edge — clicking a heading that's
already visible would previously do nothing, which reads as "the click
didn't work" even though the cursor did move.

**`src/text_editor.rs`:** extracted the shared setup both scroll methods
need (cursor's content-space Y, viewport height, max scroll offset) into
`cursor_scroll_geometry`, then added `scroll_to_cursor_centered`, which
always repositions the cursor's line to the vertical middle of the
viewport rather than only correcting when it's near an edge. The
`pending_scroll_to_cursor` flag (exclusively set by `jump_to_line`) now
calls this instead of the regular `scroll_to_cursor` — ordinary in-editor
navigation (arrow keys, vim motions, click-to-position) is completely
unaffected, since none of those touch this flag or call the centered
variant.

### Verification

- `cargo test`: 565 passed, 0 failed (561 unit incl. 7 new; 4 integration).
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- **Not hardware-verified**: confirm on a real display — a heading with
  nested headings shows an arrow, clicking it hides/shows exactly its own
  subtree (not siblings'), and collapsing/re-expanding a parent doesn't
  disturb a separately-collapsed child's own state; clicking an
  already-visible heading now visibly re-centers the viewport instead of
  doing nothing.

## Docx Round-Trip Fidelity

Fixed every silent formatting-loss bug in `src/docx_parser.rs`: opening a
real `.docx`, editing it, and saving previously dropped several
attributes the app's own UI already set — this plan closes each gap
symmetrically (parse *and* emit) rather than just one direction, plus adds
preservation for content types the app doesn't model at all instead of
silently destroying them on save.

### The five formatting round-trip fixes (`src/docx_parser.rs`)

All five follow the same shape: a missing parse arm and/or missing emit
logic, made symmetric.

- **Alignment + heading** (`<w:pPr>` wrapper): `apply_para_style` already
  parsed `<w:pStyle>` (heading level) correctly, but `rebuild_document_xml`
  never emitted `<w:pPr>`/`<w:pStyle>` **at all**, for any paragraph — every
  Pocket/Hat/Block/Tag heading was silently lost on save. New
  `apply_para_alignment` reads `<w:jc w:val="...">` (`"center"`/`"right"`/
  `"both"` — Word's own OOXML name for full justification, not
  `"justify"`). Both now share one conditionally-emitted `<w:pPr>` block.
- **Double underline**: `apply_run_prop`'s `w:u` arm set `underline = true`
  unconditionally, collapsing Hat's double-underline card style to single
  underline on save. Now reads `w:val="double"` into
  `run.double_underline`, kept mutually exclusive with `underline`.
- **Strikethrough**: `<w:strike/>` had zero XML representation in either
  direction, despite `Run.strikethrough` already being wired through the
  app's own formatting model.
- **Pocket box**: no Word equivalent exists at the run level, but a
  paragraph border (`<w:pBdr>`) is the native equivalent and renders as an
  actual box in Word. Parsed onto every run in the paragraph (matching how
  `apply_card_style` already applies `box_format` uniformly), emitted as a
  4-sided single-line border when any run has it set.

### Preserving content the app can't model (`Paragraph.unsupported_xml`)

New field: `Paragraph.unsupported_xml: Option<String>`. At parse time, a
paragraph containing one of a **narrow, explicit** list of elements
(`<w:hyperlink>`, `<w:drawing>`, `<w:footnoteReference>`,
`<w:endnoteReference>`, `<w:fldSimple>`, `<w:instrText>`) has its full raw
inner XML captured verbatim. On save, `rebuild_document_xml` re-emits that
verbatim instead of rebuilding from `runs`/`heading`/`alignment` — so
editing text *elsewhere* in the document no longer silently deletes a
hyperlink or image the app can't itself represent.

Deliberately narrow rather than "anything unhandled": incidental tags like
`<w:bookmarkStart>`/`<w:proofErr>` must keep being silently dropped exactly
as before, not freeze the paragraph from editing.

**Invalidation**: cleared to `None` the instant the paragraph is actually
touched, at `document_ops.rs`'s existing mutation choke points
(`sync_insert_char`, `sync_delete_range`, `apply_formatting`) — editing the
exotic paragraph directly honestly drops its content rather than
pretending to keep a hyperlink's target in sync with retyped text.

### Warning about content the app can't preserve at all (tables)

Tables are block-level (not a single line the way a paragraph is), so
`Vec<Paragraph>` can't represent them without a real block-model redesign
— explicitly out of scope. Instead: `parse_docx` scans raw
`word/document.xml` for `<w:tbl` and sets `DocxOrigin.has_unsupported_blocks`;
`open_file` copies this onto `Tab.has_unsupported_blocks`.
`text_editor.rs`'s `render()` shows a dismissible banner ("This document
contains a table — Vimbatim can't edit or preserve it; saving will remove
it.") above the editor when set and not yet dismissed
(`Tab.unsupported_banner_dismissed`). Not a blocking modal — the file still
opens and saves; the risk is just no longer silent.

### Testing

Two layers, per the design spec: XML-string tests (`rebuild_document_xml`
→ `parse_document_xml` directly, one pair per fixed attribute) plus a new
real-file round-trip test (`create_new_docx` → `parse_docx` →
`DocxOrigin::save` → `parse_docx` again) exercising the actual ZIP/file
code path the running app uses — no prior test in this file touched
`parse_docx`/`write_docx`/`DocxOrigin::save` at all.

### Verification

- `cargo test`: 597 passed, 0 failed (593 unit incl. ~30 new; 4
  integration), across all 7 tasks.
- `timeout 5 ./target/debug/vimbatim`: launched and ran the full timeout
  without a panic.
- No new dead-code warnings vs. the pre-plan baseline.
- **Not verifiable in this sandbox, needs the user's own machine**: open a
  real `.docx` containing at least one Pocket/Hat/Block/Tag heading, a
  centered paragraph, and (if available) a hyperlink or table, in this
  app; make an unrelated text edit; save; reopen the saved file in actual
  Microsoft Word (or LibreOffice/Google Docs as a fallback) and confirm
  the heading level, alignment, double underline/strikethrough/box, and
  (for the hyperlink case) the hyperlink itself are all still correct.
  This is the one thing no test in this plan can substitute for.
