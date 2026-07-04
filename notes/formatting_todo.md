# Rich Text Formatting — Spec Sheet & Completion Todo

This document narrows the full spec in `notes/editor_instructions.md` section
6 ("Required: Rich Text Display") and section 7 ("Required: Formatting
Operations") down to exactly what's left to build, checked against the
actual state of the code as of the end of the vim-mode work (`vim_todo.md`
Tasks A–I, all done). Read `notes/editor_instructions.md` section 9
(Implementation Notes and Constraints) before starting — the commenting
standard, GPUI-only rule, tab identity by `id`, UTF-8 cursor safety, and
commit message format all apply here and are not repeated in full.

`vim_todo.md` explicitly scoped rich text and formatting operations out of
the vim work: "vim edits operate on the flat `tab.content` string only." This
document is what changes that — every vim-mode edit path (Tasks A–I, ~435
tests) must keep working exactly as it does today; formatting sync is added
*alongside* it, not by replacing it.

---

## 0. Scope decisions (confirmed with the user before writing this document)

- **Two-phase build, in sequence.** Phase 1 (display + the infrastructure to
  keep formatting attached to text across edits) must be done and working
  before Phase 2 (the ribbon UI + actual formatting-editing operations)
  starts. Phase 2's `apply_formatting` depends entirely on Phase 1's
  synced-paragraph model existing.
- **Attribute scope extended beyond the literal spec text.** Section 6.2's
  style table and section 7.1's ribbon only cover bold/underline/highlight/
  size. This document adds **italic, font family, and text color** to both
  the data model and the ribbon, per explicit user decision — a real Word
  document commonly carries these and the feature would feel incomplete
  without them. Every place below that says "per spec 6.2/7.1" should be
  read as "plus italic/font/color, added the same way."
- **Formatting operations target `tab.selection`, vim-mode-aware.** Ribbon
  buttons and any keyboard shortcuts act on the active selection — which
  already means Visual/VisualLine mode's selection works with zero extra
  plumbing, since `tab.selection` is the same field vim mode already
  maintains. Normal mode with no selection is a smaller decision left to
  Phase 2's own task (§7 below).

---

## 1. Current State (verified against source, not assumed)

| Area | File | State |
|------|------|-------|
| `Run { text, bold, underline, highlight, highlight_color, size, whitespace_preserve }` | `src/docx_parser.rs:13-24` | Exists. No `italic`/`font`/`color` fields. |
| `Paragraph { runs: Vec<Run>, heading: u8 }` | `src/docx_parser.rs:28-32` | Exists. `heading` is the only paragraph-level attribute — no general "style name" field (relevant to §7.1's CARD STYLES group, see §8 below). |
| `DocxDocument { paragraphs, raw_zip, preamble, sect_pr }` | `src/docx_parser.rs:46-52` | Exists, wrapped in `Arc` on `Tab` (`Tab.document: Option<Arc<DocxDocument>>`) specifically so cloning it (done once per save, `src/state.rs:503`) is cheap. **This is incompatible with Phase 1** — see §2.1. |
| `parse_document_xml` reads `<w:b>`, `<w:u>`, `<w:highlight>`, `<w:sz>`, `<w:pStyle>` | `src/docx_parser.rs:185-372` | Done for bold/underline/highlight/size/heading. No `<w:i>`, `<w:rFonts>`, `<w:color>` handling — silently discarded today. |
| `to_plain_text()` | `src/docx_parser.rs:57-68` | One paragraph → one line, joined by `\n`; runs within a paragraph concatenated with **no separator**. Confirms: a `\n` in `tab.content` is *always* exactly a paragraph boundary, never a soft in-paragraph break (Word's `<w:br/>` isn't parsed at all) — no ambiguity to resolve here. |
| `save()` (unedited path) | `src/docx_parser.rs:73-80` | Serialises `self.paragraphs` verbatim — full formatting fidelity, but only reachable when the tab was never edited (see below). |
| `save_from_content()` (edited path) | `src/docx_parser.rs:86-96`, via `content_to_paragraphs` (`:377-391`) | **The bug this document exists to fix**: one plain `Run::default()` per line, regardless of what formatting existed. Any edit to a loaded docx silently drops all formatting on save — `editor_instructions.md` line 82 names this an intentional interim simplification. |
| Rendering | `src/text_editor.rs` (`render()`, `document_lines`, `render_line`/`line_segments`) | Splits `tab.content` on `\n` and draws every line with one uniform `.font_family("monospace").text_sm().text_color(rgb(0xd4d4d4))`. `line_segments` already splits a line into styled sub-spans, but only for the cursor/selection overlay (`SegmentStyle::Cursor`/`Selection`) — never for formatting. `tab.document`/`tab.paragraphs` is never read by `render()` today. |
| `formatting_ribbon.rs` | — | Does not exist. Marked "Stub" in `editor_instructions.md`'s codebase-state table. |
| `apply_formatting`/`FormatOp` | — | Does not exist anywhere. |
| GPUI per-run styling primitive | vendored `gpui` 0.2.2 | Confirmed available: `TextRun { len, font: Font, color: Hsla, background_color: Option<Hsla>, underline: Option<UnderlineStyle>, strikethrough: Option<StrikethroughStyle> }` plus `StyledText::new(text).with_runs(Vec<TextRun>)`. This is the mechanism §6 below builds on — nothing needs to be vendored or upgraded. |
| `settings.conf` `small_size`/`large_size` | `config_parsing/config_parsing.rs:13-14, 84-85` | Already parsed (`u8`, points). §7.1's SIZE group (`Shrink`/`Normal`) can read these directly — no gap. |

---

## 2. Data Model — Phase 1

All in `src/docx_parser.rs` and `src/state.rs` unless noted.

### 2.1 `Tab.document` must stop being `Arc`-wrapped

`Arc<DocxDocument>` exists today specifically so cloning it is cheap (its own
doc comment: "callers bump the refcount rather than deep-copying" — and it
deliberately does not derive `Clone` to discourage anyone from deep-copying
it). Phase 1 needs to *mutate* a tab's paragraphs on every keystroke, which
an `Arc` with no interior mutability can't support without `Arc::make_mut`
(which itself requires `Clone`, defeating the original design intent).

Split `DocxDocument` into two pieces with different lifetimes:

```rust
// src/docx_parser.rs
pub struct Paragraph { pub runs: Vec<Run>, pub heading: u8 }  // unchanged

/// The save-time constants — everything needed to reconstruct a real .docx
/// file around whatever `Tab.paragraphs` currently holds, but never
/// mutated during editing. Still cheap to clone via Arc since it's
/// genuinely immutable for the tab's lifetime.
pub struct DocxOrigin {
    pub(crate) raw_zip: Vec<u8>,
    pub(crate) preamble: String,
    pub(crate) sect_pr: String,
}
```

```rust
// src/state.rs, Tab struct
pub paragraphs: Vec<Paragraph>,          // live, mutated by every edit (Phase 1 §4)
pub docx_origin: Option<Arc<DocxOrigin>>, // None for brand-new/never-parsed tabs
```

`parse_docx` returns `(Vec<Paragraph>, DocxOrigin)` instead of one
`DocxDocument`. `open_file` (`src/state.rs:460-463`) sets both fields;
`Tab::new_empty`/`Tab::from_path` default `paragraphs` to a single empty
paragraph with one default run (never `vec![]` — Phase 1's mutation
primitives, §4, assume at least one paragraph/run always exists) and
`docx_origin: None`. Update the two existing `.document` call sites
(`src/state.rs:463`, `:503`) to the new fields — `save_tab` (§2.3 below)
changes shape anyway as part of Phase 1 Task 7.

### 2.2 `Run` gains italic/font/color

```rust
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,           // new
    pub underline: bool,
    pub highlight: bool,
    pub highlight_color: String,
    pub size: u16,
    pub font: Option<String>,   // new — None means "inherit document default"
    pub color: Option<String>,  // new — hex RRGGBB (docx's own format), None = default
    pub whitespace_preserve: bool,
}
```

`Run` already derives `Default`; the new fields default to `false`/`None`
for free, so `content_to_paragraphs`-style "plain run" construction
elsewhere doesn't need updating.

### 2.3 Undo/redo snapshots both `content` and `paragraphs` together

`Tab.undo_stack`/`redo_stack: Vec<String>` (content-only, `src/state.rs`)
becomes `Vec<(String, Vec<Paragraph>)>` (or a small `UndoSnapshot` struct if
a third field is ever needed — YAGNI for now, a tuple is fine). Every
`push_undo_snapshot` call site (there's exactly one function,
`src/state.rs:572`) already funnels every mutation through it — updating
its signature to snapshot `tab.paragraphs.clone()` alongside
`tab.content.clone()` is a single, localized change. Without this, undo
would restore old text while leaving stale (shifted-wrong or
already-deleted) formatting attached to it.

`Paragraph`/`Run` need to derive `Clone` for this (harmless — they don't
hold anything expensive; `raw_zip`, the actually-large field, lives on
`DocxOrigin` now and is never cloned per-edit).

---

## 3. Phase 1 — Display + Format-Sync Infrastructure

### Task 1: Data model refactor (§2.1–2.3 above)

Files: `src/docx_parser.rs`, `src/state.rs`. Foundational — nothing else in
Phase 1 can start until this lands and `cargo test` passes with the two
existing `.document` call sites updated.

### Task 2: Parser extension — italic/font/color

Files: `src/docx_parser.rs`. Extend `parse_document_xml`'s per-run property
handling (`apply_run_prop`, `:343+`) with `<w:i>` → `italic = true` (same
presence-means-true pattern as `<w:b>`), `<w:rFonts w:ascii="...">` →
`font = Some(...)` (Word's run-fonts element has several attributes for
different script ranges — read `w:ascii` only, document that East
Asian/complex-script font overrides are out of scope), `<w:color
w:val="RRGGBB">` → `color = Some(...)` (skip `w:val="auto"`, which means
"inherit", same as not being present). Extend `rebuild_document_xml`
(`:396+`) to re-emit `<w:i/>`, `<w:rFonts w:ascii="...">`, `<w:color
w:val="...">` when each field is set, mirroring how bold/underline are
already emitted. TDD: construct minimal `word/document.xml` fixtures with
each new attribute (inline strings are fine, matching how existing parser
tests are structured — check `tests/parse_testing.rs` for the pattern
before adding new fixtures) and assert round-trip through parse → rebuild.

### Task 3: Byte-offset ↔ (paragraph, run, char) resolution

Files: `src/docx_parser.rs` or a new `src/document_ops.rs` (recommended —
`state.rs` is already large; this is a self-contained, pure-function
module with its own natural boundary, matching how `docx_parser.rs` is
already separate).

```rust
pub fn resolve_position(paragraphs: &[Paragraph], byte_offset: usize) -> (usize, usize, usize);
// -> (para_idx, run_idx, char_offset_within_run)
```

Paragraph boundaries are exactly line boundaries (confirmed in §1) — this
is the same walk `line_start`/`line_end` in `state.rs` already do over
`content`, just also summing run-text lengths within the found paragraph
to land on the right run. Pure function, unit-test directly with
hand-built `Vec<Paragraph>` fixtures (edge cases: offset at a paragraph
boundary — lands at the *start* of the next paragraph, not the end of the
previous one, chosen for consistency with `insert_char`'s own
"cursor lands after what it inserted" convention; offset inside a
multi-byte UTF-8 character — should not occur if callers only ever pass
already-validated char-boundary offsets, same invariant every vim-mode
function already relies on).

### Task 4: Choke-point mutation sync

Files: `src/state.rs`. This is the largest task in Phase 1 — enumerate
every function that mutates `tab.content` directly and give each a paired
paragraph-mutation call. Thanks to this codebase's existing "single
mutation choke point" pattern (established across Tasks C/F/H), the actual
list is short:

- `insert_char`/`insert_str` (`:620-637`, `:752-771`) — insert into the run
  found by `resolve_position`, **inheriting that run's formatting** (real
  rich-text convention: typed text takes on the format of whatever it's
  typed inside). Typing `\n` is the one structural case: splits the
  current paragraph into two at the char offset (the run being split
  contributes its tail to a new run starting the new paragraph).
- `backspace`/`delete_selection_raw` (`:642-665`, `:667+`) and
  `delete_vim_range`/`replace_vim_range` (the vim operator choke points,
  Task F) — remove text from the run(s)/paragraph(s) spanning
  `[start, end)`; deleting a paragraph's trailing `\n` merges it with the
  next paragraph (its runs appended to the end of the first);
  empty runs left behind by a deletion are dropped, not kept as
  zero-length placeholders.
- `dispatch_vim_substitute` (`:%s`, Task H) is the one outlier — a regex
  substitution can change a line's text in a way that has no clean
  per-character mapping back to the original runs. **Scope limit,
  documented rather than solved generally**: after a `:%s` edit, any
  paragraph whose text actually changed has its runs replaced with a
  single default (unformatted) run — same behavior as today, but now
  scoped to *only* the lines the substitution actually touched, not the
  whole document. Paragraphs untouched by the substitution keep their
  existing runs exactly.

Every one of these needs its own unit test asserting `tab.paragraphs`
after the edit, not just `tab.content` — this is the core regression
surface for the whole feature and deserves the same TDD rigor Tasks A–I
used throughout.

### Task 5: Undo/redo integration

Covered by §2.3 — listed here as its own checkable task since it touches
`push_undo_snapshot` and both `undo()`/`redo()`'s restore logic
(`src/state.rs`), which need to restore `tab.paragraphs` alongside
`tab.content` on every pop.

### Task 6: Rendering (`src/text_editor.rs`)

Replace the uniform per-line rendering with `StyledText::new(line_text)
.with_runs(Vec<TextRun>)`, one call per paragraph/line. Build the
`Vec<TextRun>` by walking that paragraph's `runs` and mapping fields per
spec 6.2, extended:

| `Run` field | GPUI style |
|---|---|
| `bold` | `font.weight = FontWeight::BOLD` |
| `italic` | `font.style = FontStyle::Italic` |
| `underline` | `underline: Some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false })` |
| `highlight` + `highlight_color` | `background_color: Some(color_from_word_highlight(&run.highlight_color))` (15-entry table, spec 6.2, copied verbatim) |
| `size` (half-points) | override this run's rendered size: `px(run.size as f32 / 2.0)` |
| `font` | `font.family = run.font.clone().unwrap_or_default()` |
| `color` | `color: parse_hex_color(&run.color).unwrap_or(default)` |

Plus `para.heading` (spec 6.5's table, unchanged: 24/20/18/16/14px for
levels 1/2/3/4-6/7-9, bold) applied as a paragraph-wide override layered
underneath the per-run table above.

**Integration risk to solve explicitly, not by accident**: `line_segments`
already splits a line into sub-spans for the cursor/selection overlay.
This task needs ONE run-splitting pass that accounts for *both* formatting
run boundaries *and* cursor/selection boundaries — building two
independent split passes and trying to compose their output after the
fact will not produce correct results where they overlap. Design this as
a single function that takes both the paragraph's runs and the
cursor/selection range and produces one final `Vec<TextRun>`.

Fallback: when `tab.docx_origin` is `None` (brand-new tab, or `parse_docx`
failed) — `tab.paragraphs` still exists (§2.1's default: one paragraph,
one plain run) and renders through the exact same path with no special
casing needed, matching spec 6.1's "fall back to plain text" intent
without actually needing a separate code path.

### Task 7: Save integration — the actual bug fix

Files: `src/state.rs` (`save_tab`), `src/docx_parser.rs`. `save_tab`
currently branches on `tab.document` being `Some`/`None` to choose
`save_from_content` (lossy) vs. `create_new_docx`. Once `tab.paragraphs` is
always live and kept in sync (Task 4), `save_from_content` and
`content_to_paragraphs` are no longer needed for the edited-and-saved
case — `save_tab` calls a `save_paragraphs(paragraphs, origin, path)`
free function (using `docx_origin`'s `raw_zip`/`preamble`/`sect_pr` when
`Some`, or `create_new_docx`-style fresh-skeleton generation when `None`)
directly on `tab.paragraphs`, unconditionally. This is what actually fixes
`editor_instructions.md` line 82's documented simplification — worth its
own `tmp_documentation.md` writeup calling out explicitly that this bug is
now fixed, since it's been a known, named gap since Task H.

---

## 4. Phase 2 — Formatting Editing Operations

Do not start until Phase 1 Task 7 is done, tested, and its own
`tmp_documentation.md` entry is written — `apply_formatting` below is
built directly on `tab.paragraphs` being reliably in sync.

### Task 1: `apply_formatting` core

Files: `src/document_ops.rs` (or wherever Task 3 landed).

```rust
pub fn apply_formatting(paragraphs: &mut Vec<Paragraph>, start: usize, end: usize, op: FormatOp);

pub enum FormatOp {
    Bold(bool),
    Italic(bool),                  // extended scope
    Underline(bool),
    Highlight(Option<String>),     // None = remove highlight
    FontSize(u16),                 // 0 = remove explicit size
    FontFamily(Option<String>),    // extended scope; None = remove override
    Color(Option<String>),         // extended scope; None = remove override
    ClearAll,
}
```

Per spec 7.2: resolve `start`/`end` via Task 3's `resolve_position`, split
the run(s) at each boundary if the boundary falls mid-run (a new
`split_run_at(run, char_offset) -> (Run, Run)` primitive, reusable from
Task 3's module), then apply `op` to every run now fully contained in
`[start, end)`. Push an undo snapshot first (§2.3's now-paired
content+paragraphs snapshot) — formatting-only changes must be undoable
same as text edits.

### Task 2: Selection-target dispatch

Files: `src/state.rs`. A thin `apply_formatting_to_selection(op: FormatOp)`
on `AppState` resolving `tab.selection` (works in Visual/VisualLine mode
with zero extra work, per the vim-aware decision in §0) and calling Task
1's `apply_formatting`. **Open micro-decision, not yet resolved**: what
happens when `apply_formatting_to_selection` is called with no active
selection (plain Normal-mode cursor, nothing highlighted)? Real Word
toggles a "pending state" for whatever gets typed next (spec 7's own
intro line: "or (if no selection) toggles the property for subsequent
typing"). Implementing that pending-state mechanism is a meaningfully
different, second sub-feature (needs a new `Tab.pending_format:
Option<FormatOp>` consulted by `insert_char`) — decide at Phase 2's own
kickoff whether it's in scope for this pass or deferred (no-op on empty
selection) as its own follow-up.

### Task 3: Ribbon UI (`src/formatting_ribbon.rs`)

New file — currently doesn't exist. Build the CARD STYLES / MARKUP /
CLEAN / STRUCTURE / SIZE groups per spec 7.1, plus (extended scope) an
Italic button in MARKUP, and a font-family picker + color picker (new UI
elements — check what dropdown/picker primitives, if any, the sidebar or
settings modal already use in this codebase before building new ones from
scratch). Wire each button to `apply_formatting_to_selection` via the
`AppState` entity, mirroring how the settings modal already talks to
`AppState`.

**Real spec gap, flag rather than guess**: CARD STYLES (`Tag`/`Cite`/
`Body`/`Pkt`/`Pkt Cite`) and STRUCTURE (`Open Blk`/`Close Blk`) aren't
character-run formatting at all — `Paragraph` only has `heading: u8`
today, nothing resembling a named paragraph style, and "block markers"
aren't defined anywhere in the spec beyond their button names. Resolve
before building Task 3's UI: either (a) add a `Paragraph.style: Option<
String>` field (or an enum of the 5 named styles) with its own rendering
rules (not specified — sizes/weights for Tag/Cite/Body/Pkt/PktCite are
not given anywhere in `editor_instructions.md`), or (b) treat CARD STYLES
and STRUCTURE as out of scope for this pass and ship MARKUP/CLEAN/SIZE
only, documenting the gap the same way `vim_todo.md` documented the `R`
Replace-mode gap rather than silently guessing at unspecified behavior.

### Task 4: Keyboard shortcuts

Files: `src/text_editor.rs` (`process_key_ctrl_combo`, the existing
Ctrl-combo dispatch table Task I's jump-list wiring already extended).
Add `Ctrl+B`/`Ctrl+I`/`Ctrl+U` calling `apply_formatting_to_selection`
with the corresponding toggle — consistent with this app's existing
non-vim Ctrl-shortcut convention (spec 4), and independent of whichever
way Task 2's no-selection micro-decision goes.

---

## 5. Verification

- `cargo check` before every commit.
- `cargo test` must keep passing — all ~435 existing tests from Tasks A–I
  are the regression suite for Phase 1 Task 4 in particular (every vim
  operator/motion test implicitly exercises the choke-point functions
  being changed). Add new tests alongside every function touched, not at
  the end, matching the established convention.
- `./run.sh` for manual verification once Phase 1 Task 6 (rendering)
  lands — this is the one area (like vim mode's mode indicator) that
  needs real-keyboard/real-display confirmation beyond unit tests, since
  this sandbox has no working display.
- Commit format per §9.5: `feat:`/`fix:`/`refactor:`, no scope prefix.
- Every new function needs a multi-line block comment under its
  signature; finishing a task means appending a description to
  `tmp_documentation.md`, same as every vim-mode task did.

## 6. Suggested Branch

This is a large, separate feature from vim mode — suggest a new branch
(`rich-text-formatting` or similar) off whatever `vim_mode` merges into,
rather than continuing directly on `vim_mode`. Confirm with the user
before creating it (per this session's own git-safety conventions —
branch creation isn't something to do unprompted).
