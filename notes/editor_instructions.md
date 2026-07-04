# Vimbatim — Full .docx Editor Requirements

This document is the authoritative specification for completing the Vimbatim editor. It is written for agents implementing the remaining functionality. Read the entire document before writing any code.

---

## 1. Project Overview

Vimbatim is a `.docx` editor built in Rust using the [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) UI framework. Its primary audience is competitive policy debaters who use `.docx` files as their evidence cards. The design is inspired by the "Verbatim" Word macro suite — debaters need fast formatting operations (tagging, underlining, highlighting cards) and the ability to navigate large documents quickly. Vim keybind support is a first-class requirement because many debaters use Vim-style editing.

The editor should feel like VS Code (tab system, sidebar, keyboard-driven) with the formatting capabilities of Microsoft Word and Vim navigation layered on top.

---

## 2. Current Codebase State

All source files live in `src/`. Consult each file directly — this section lists what is already implemented so you do not duplicate work.

| File | Status | What it does |
|------|--------|-------------|
| `main.rs` | Done | App entry point, keybinding registration |
| `main_window.rs` | Done | Root layout view, action dispatch |
| `state.rs` | Done | `AppState`, `Tab`, `FileNode`, `scan_directory` |
| `docx_parser.rs` | Done | `.docx` ZIP decompression, XML parsing, round-trip save |
| `tab_bar.rs` | Done | Tab strip with open/close/drag-reorder |
| `file_explorer.rs` | Done | Sidebar file tree, double-click to open |
| `app_toolbar.rs` | Done | Top toolbar strip |
| `text_editor.rs` | Stub | Displays `tab.content` as plain text; character-append and backspace only; **no cursor positioning, no selection, no formatting display** |
| `formatting_ribbon.rs` | Stub | Renders buttons that `println!` on click; **actions not wired to editor** |
| `settings_modal.rs` | Stub | Floating modal opens/closes; **settings not wired** |
| `config_parsing/` | Done | Parses `settings.conf` into a `Settings` struct |

### Current `Tab` struct (in `state.rs`)

```rust
pub struct Tab {
    pub id: usize,
    pub title: String,
    pub file_path: Option<PathBuf>,
    pub content: String,                     // flat plain-text string
    pub document: Option<Arc<DocxDocument>>, // parsed .docx structure
    pub is_modified: bool,
}
```

### Current `DocxDocument` struct (in `docx_parser.rs`)

```rust
pub struct DocxDocument {
    pub paragraphs: Vec<Paragraph>,   // parsed content
    pub(crate) raw_zip: Vec<u8>,      // original file bytes for round-trip save
    pub(crate) preamble: String,      // XML before <w:body>
    pub(crate) sect_pr: String,       // <w:sectPr> block (page layout)
}
```

`DocxDocument` does **not** derive `Clone` — it is always accessed through `Arc<DocxDocument>`.

---

## 3. Required: Document Model

The current model stores content in a flat `String` (`tab.content`) for display and a separate `Vec<Paragraph>` in `tab.document` for save. These diverge after any edit. The full implementation must reconcile them into a single source of truth.

### 3.1 Editing State per Tab

Add the following fields to `Tab` in `state.rs`:

```rust
pub cursor: usize,          // byte offset into the flat document string
pub selection: Option<(usize, usize)>, // (anchor, focus) byte offsets; None = no selection
pub vim_mode: VimMode,      // current vim mode for this tab
pub vim_command_buf: String, // accumulation buffer for : commands and operator+count+motion combos
```

The `cursor` and `selection` fields operate on **the flat string** (`tab.content`). This keeps the text editor simple — it works on a `String` and does not need to understand the paragraph/run structure. The paragraph model (`tab.document.paragraphs`) is only used for parsing (load) and serialisation (save).

### 3.2 Flat String as Source of Truth for Editing

After loading, `tab.content` is populated by `DocxDocument::to_plain_text()`, which joins paragraphs with `\n`. From that point on, all edits are mutations on `tab.content`. The paragraph model is not kept in sync during editing — on save, `save_from_content` rebuilds paragraphs from the flat string by splitting on `\n`.

This is an intentional simplification. A future pass can maintain rich-text run structure through edits; for now the contract is: **open preserves formatting read-only; any edit produces plain paragraphs on save.**

### 3.3 `VimMode` Enum

Add to `state.rs`:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,      // character-wise visual selection
    VisualLine,  // line-wise visual selection
    Command,     // : command line
}

impl Default for VimMode {
    fn default() -> Self { VimMode::Normal }
}
```

Vim mode is per-tab so switching tabs restores the previous mode.

---

## 4. Required: Core Text Editing

Replace the current stub in `text_editor.rs` entirely. The new implementation must:

### 4.1 Cursor Movement

All movements must keep `cursor` within valid UTF-8 character boundaries (never split a multi-byte character).

| Operation | Key(s) |
|-----------|--------|
| Move right one char | `→` / (vim: `l`) |
| Move left one char | `←` / (vim: `h`) |
| Move down one line | `↓` / (vim: `j`) |
| Move up one line | `↑` / (vim: `k`) |
| Move to line start | `Home` / (vim: `0`) |
| Move to first non-whitespace | (vim: `^`) |
| Move to line end | `End` / (vim: `$`) |
| Move forward one word | `Ctrl+→` / (vim: `w`) |
| Move backward one word | `Ctrl+←` / (vim: `b`) |
| Move to end of word | (vim: `e`) |
| Move to document start | `Ctrl+Home` / (vim: `gg`) |
| Move to document end | `Ctrl+End` / (vim: `G`) |
| Move to line N | (vim: `Ng` where N is a number, or `NG`) |
| Page down | `PgDn` / `Ctrl+D` (vim half-page) |
| Page up | `PgUp` / `Ctrl+U` (vim half-page) |

### 4.2 Text Insertion

- In **Normal mode** (vim): no character insertion; keys dispatch motion/operator commands
- In **Insert mode** (vim) or when vim is disabled: all printable characters insert at `cursor`; `cursor` advances after each insert
- `Enter` inserts `\n` at `cursor`
- `Tab` inserts a tab character (`\t`) at `cursor`
- `Backspace` deletes the character immediately before `cursor`; cursor moves left
- `Delete` deletes the character at `cursor`; cursor stays

All insertions and deletions must update `tab.is_modified = true`.

### 4.3 Selection

- `Shift+arrow` keys extend or create a selection
- `Shift+Ctrl+arrow` extends by word
- `Shift+Home`/`End`, `Shift+Ctrl+Home`/`End` extend to line/document boundaries
- Mouse click-drag creates a selection
- `Ctrl+A` selects all

When a non-empty selection exists and the user types any character, the selection is deleted first, then the character is inserted.

`Backspace` or `Delete` with an active selection deletes the selected range.

### 4.4 Clipboard

- `Ctrl+C` — copy selection to system clipboard
- `Ctrl+X` — cut selection to system clipboard
- `Ctrl+V` — paste from system clipboard at cursor (replaces selection if present)

GPUI provides clipboard access via `cx.write_to_clipboard(ClipboardItem::new(text))` and `cx.read_from_clipboard()`.

### 4.5 Undo / Redo

- `Ctrl+Z` — undo last edit
- `Ctrl+Y` / `Ctrl+Shift+Z` — redo

Maintain a `Vec<String>` undo stack per tab. Push the current `content` onto the stack before each edit operation (or batch rapid keystrokes within a 300ms window into one undo entry). Cap the stack at 200 entries.

### 4.6 Find / Replace

- `Ctrl+F` — open an inline find bar at the bottom of the editor
- `Enter` / `F3` — next match
- `Shift+Enter` / `Shift+F3` — previous match
- `Ctrl+H` — open find-and-replace bar
- `Escape` — close find bar

Highlight all matches in the editor text. This requires the render pass to know the match positions.

---

## 5. Required: Vim Mode

Vim mode is toggled by the `vim` setting in `settings.conf` (parsed by `config_parsing`). It can also be toggled at runtime via the settings modal. When disabled, the editor behaves as a standard text editor with only the non-vim shortcuts in Section 4.

### 5.1 Mode Entry and Exit

| From | Key | To |
|------|-----|----|
| Normal | `i` | Insert (before cursor) |
| Normal | `I` | Insert (at line start) |
| Normal | `a` | Insert (after cursor) |
| Normal | `A` | Insert (at line end) |
| Normal | `o` | Insert (new line below, cursor at start) |
| Normal | `O` | Insert (new line above, cursor at start) |
| Normal | `v` | Visual (character-wise) |
| Normal | `V` | VisualLine |
| Normal | `Ctrl+v` | (future: VisualBlock — not required in first pass) |
| Normal | `:` | Command |
| Insert | `Escape` or `Ctrl+[` | Normal |
| Visual | `Escape` or `v` | Normal |
| VisualLine | `Escape` or `V` | Normal |
| Command | `Escape` | Normal |
| Command | `Enter` | Execute command, return to Normal |

The current mode must be visible to the user. Display a mode indicator in the status bar or at the bottom of the editor area (e.g. `-- INSERT --`, `-- VISUAL --`, `-- COMMAND --`; nothing shown for Normal).

### 5.2 Normal Mode — Motions

Motions move the cursor. All motions accept an optional count prefix (e.g. `3w` = move forward 3 words, `5j` = move down 5 lines). The count is accumulated in `vim_command_buf`.

| Key | Motion |
|-----|--------|
| `h` | left one character |
| `l` | right one character |
| `j` | down one line |
| `k` | up one line |
| `w` | forward to start of next word |
| `W` | forward to start of next WORD (whitespace-delimited) |
| `b` | backward to start of current/previous word |
| `B` | backward to start of current/previous WORD |
| `e` | forward to end of current/next word |
| `E` | forward to end of current/next WORD |
| `0` | start of line |
| `^` | first non-whitespace of line |
| `$` | end of line |
| `gg` | first line of document |
| `G` | last line of document |
| `{` | backward to start of paragraph (empty line boundary) |
| `}` | forward to start of next paragraph |
| `%` | (future: matching bracket/paren) |
| `f<char>` | forward to next occurrence of `<char>` on current line |
| `F<char>` | backward to previous occurrence of `<char>` on current line |
| `t<char>` | forward to character before next `<char>` |
| `T<char>` | backward to character after previous `<char>` |
| `;` | repeat last `f/F/t/T` |
| `,` | repeat last `f/F/t/T` in reverse |
| `H` | move cursor to top of visible window |
| `M` | move cursor to middle of visible window |
| `L` | move cursor to bottom of visible window |
| `Ctrl+D` | scroll down half page |
| `Ctrl+U` | scroll up half page |
| `Ctrl+F` | scroll down full page |
| `Ctrl+B` | scroll up full page |
| `zz` | center current line in window |
| `zt` | scroll so current line is at top |
| `zb` | scroll so current line is at bottom |

### 5.3 Normal Mode — Operators

Operators act on a motion or text object. The pattern is `[count]operator[count]motion` or `[count]operator[text object]`.

| Operator | Effect |
|----------|--------|
| `d` | delete (cut) |
| `y` | yank (copy) |
| `c` | change (delete then enter Insert mode) |
| `>` | indent right |
| `<` | indent left |
| `=` | auto-indent (future) |
| `gU` | uppercase |
| `gu` | lowercase |

**Doubled operator acts on the current line:** `dd` deletes the current line, `yy` yanks the current line, `cc` changes the current line.

### 5.4 Normal Mode — Text Objects

Used after an operator: `[operator][i/a][object]`.

- `i` = "inner" (excluding surrounding whitespace/delimiters)
- `a` = "a" / "around" (including surrounding whitespace/delimiter)

| Object | Description |
|--------|-------------|
| `w` | word |
| `W` | WORD |
| `s` | sentence |
| `p` | paragraph |
| `"` | double-quoted string |
| `'` | single-quoted string |
| `(` or `)` | parentheses |
| `[` or `]` | square brackets |
| `{` or `}` | curly braces |

Example: `diw` = delete inner word, `ci"` = change inside double quotes.

### 5.5 Normal Mode — Other Commands

| Key | Action |
|-----|--------|
| `x` | delete character under cursor (forward) |
| `X` | delete character before cursor (backward) |
| `r<char>` | replace character under cursor with `<char>` |
| `R` | enter Replace mode (overwrite characters) |
| `s` | delete character under cursor and enter Insert mode |
| `S` | delete current line and enter Insert mode |
| `p` | paste after cursor |
| `P` | paste before cursor |
| `u` | undo |
| `Ctrl+r` | redo |
| `~` | toggle case of character under cursor |
| `.` | repeat last change |
| `>>` | indent current line |
| `<<` | unindent current line |
| `J` | join current line with the next (replace `\n` with space) |
| `/` | enter search (forward) |
| `?` | enter search (backward) |
| `n` | next search match |
| `N` | previous search match |
| `*` | search forward for word under cursor |
| `#` | search backward for word under cursor |
| `Ctrl+o` | jump to previous cursor position (jump list) |
| `Ctrl+i` | jump to next cursor position (jump list) |

### 5.6 Visual Mode

In Visual mode, motions extend the selection. Operators act on the selection.

| Key | Action |
|-----|--------|
| All Normal motions | extend selection |
| `d` or `x` | delete selection |
| `y` | yank selection |
| `c` | change selection (delete + Insert mode) |
| `>` | indent selection |
| `<` | unindent selection |
| `~` | toggle case of selection |
| `gU` | uppercase selection |
| `gu` | lowercase selection |
| `o` | move cursor to other end of selection |

### 5.7 Command Mode (`:` commands)

| Command | Action |
|---------|--------|
| `:w` | save active tab |
| `:q` | close active tab (prompt if unsaved) |
| `:wq` or `:x` | save and close |
| `:q!` | close without saving |
| `:wa` | save all tabs |
| `:e <path>` | open file at path |
| `:set vim` | enable vim mode |
| `:set novim` | disable vim mode |
| `:<n>` | go to line number N |
| `:%s/<pattern>/<replacement>/[g][i]` | find and replace (g = all occurrences, i = case-insensitive) |
| `:noh` | clear search highlight |

### 5.8 Vim Registers

Maintain a simple register system:
- `"` — default register (used by `d`, `y`, `c`, `x`, `s`, `p`)
- `0` — yank register (last explicit `y` operation)
- `a`–`z` — named registers: `"ay` yanks into register a, `"ap` pastes from register a
- `+` — system clipboard register (`"+y` yanks to clipboard, `"+p` pastes from clipboard)

### 5.9 Vim and the Settings File

The `vim` key in `settings.conf` controls whether vim mode is active on startup:
```
vim=true
```
Read this via the existing `config_parsing` module. The `Settings` struct already has a `vim: bool` field. Pass it into `AppState` at startup so the first tab starts in the correct mode.

---

## 6. Required: Rich Text Display

The current editor renders `tab.content` as monospace plain text. The full implementation must render formatting visually.

### 6.1 Rendering Model

Instead of splitting `tab.content` on `\n` and rendering lines, render from `tab.document.paragraphs` directly. Each `Paragraph` produces one block-level div; each `Run` within it produces an inline span with styling applied.

When `tab.document` is `None` (new unsaved tab), fall back to rendering `tab.content` as plain text.

### 6.2 Run Styling

Apply the following GPUI style calls based on `Run` fields:

| `Run` field | GPUI style |
|-------------|------------|
| `bold` | `.font_weight(FontWeight::BOLD)` |
| `underline` | `.underline(UnderlineStyle { thickness: px(1.0), color: None, wavy: false })` |
| `highlight` with `highlight_color` | `.bg(color_from_word_highlight(&run.highlight_color))` |
| `size` (half-points) | `.text_size(px(run.size as f32 / 2.0))` |

Implement `color_from_word_highlight(name: &str) -> Hsla` mapping Word's highlight color names to GPUI colors:

| Word name | Color |
|-----------|-------|
| `yellow` | `#FFD700` |
| `green` | `#00FF00` |
| `cyan` | `#00FFFF` |
| `magenta` | `#FF00FF` |
| `red` | `#FF0000` |
| `darkBlue` | `#00008B` |
| `darkCyan` | `#008B8B` |
| `darkGreen` | `#006400` |
| `darkMagenta` | `#8B008B` |
| `darkRed` | `#8B0000` |
| `darkYellow` | `#8B8B00` |
| `darkGray` | `#A9A9A9` |
| `lightGray` | `#D3D3D3` |
| `black` | `#000000` |
| `white` | `#FFFFFF` |
| any other | `#888888` |

### 6.3 Cursor Rendering

The cursor is a position within the flat string (`tab.cursor`). Map it back to a `(para_idx, char_offset_within_para)` pair when rendering. Render the cursor as either:
- **Block cursor** (Normal mode): a highlighted background on the character cell at the cursor
- **Bar cursor** (Insert mode): a 2px vertical line before the character at the cursor

### 6.4 Selection Rendering

Render the selected range with a semi-transparent blue background overlay (`#264F78` at ~50% opacity), drawn behind the text of each affected run.

### 6.5 Paragraph Heading Styles

When `para.heading > 0`, apply a larger font size and bold weight:

| Heading level | Font size |
|---------------|-----------|
| 1 | 24px |
| 2 | 20px |
| 3 | 18px |
| 4–6 | 16px |
| 7–9 | 14px |

---

## 7. Required: Formatting Operations

The formatting ribbon buttons must be wired to actual editor operations. Each operation applies to the current selection, or (if no selection) toggles the property for subsequent typing (pending state).

Define GPUI actions for each operation and register them with keybindings. The ribbon buttons dispatch the same actions.

### 7.1 Verbatim Debate Ribbon Layout

The ribbon should match the Verbatim Word extension layout. Organise buttons into groups separated by vertical dividers:

**CARD STYLES** — apply a named paragraph/character style to the selection:
- `Tag` — applies Tag style (brief summary line)
- `Cite` — applies Cite style (citation line)
- `Body` — applies Body/normal style
- `Pkt` — applies Pocket style (abbreviated card)
- `Pkt Cite` — applies Pocket Cite style

**MARKUP** — character-level formatting:
- `Und` — underline
- `HLt` (yellow) — yellow highlight
- `HLg` (green) — green highlight
- `Bold` — bold

**CLEAN** — remove formatting:
- `Rm HL` — remove highlight from selection
- `Clean` — remove all character formatting from selection (underline, bold, highlight)

**STRUCTURE** — paragraph structure:
- `Open Blk` — insert a block-open marker
- `Close Blk` — insert a block-close marker

**SIZE** — font size:
- `Shrink` — apply the "small" font size from `settings.conf` (`small_size` field, in points)
- `Normal` — apply the "large" font size from `settings.conf` (`large_size` field, in points)

### 7.2 Applying Formatting to the Document Model

When the user applies a formatting operation to a selection:

1. Determine the selection range `(start, end)` as byte offsets into `tab.content`.
2. Map those offsets to `(para_idx, run_idx, char_offset)` pairs in `tab.document.paragraphs`.
3. Split runs at the selection boundaries if needed (a run that partially overlaps the boundary becomes two runs: one inside, one outside).
4. Apply the formatting property to all runs fully inside the selection.
5. Mark `tab.is_modified = true`.

This requires a helper function in `docx_parser.rs` or a new `src/document_ops.rs` file:

```rust
pub fn apply_formatting(doc: &mut DocxDocument, start: usize, end: usize, op: FormatOp);

pub enum FormatOp {
    Bold(bool),
    Underline(bool),
    Highlight(Option<String>), // None = remove highlight
    FontSize(u16),             // 0 = remove explicit size
    ClearAll,
}
```

After applying formatting, regenerate `tab.content` from `doc.to_plain_text()` (plain text doesn't change) and keep `tab.cursor` valid.

---

## 8. Required: File Operations

### 8.1 New File

`Ctrl+N` opens a new empty tab. If the user types content and presses `Ctrl+S`, prompt for a save path with a file dialog. On confirm, create a new `.docx` file using the fallback preamble from `docx_parser`.

### 8.2 Open File

`Ctrl+O` opens a file picker dialog. On confirm, call `AppState::open_file`. File explorer double-click already calls this.

### 8.3 Save

`Ctrl+S` — calls `AppState::save_active_tab`. Errors must be shown to the user (status bar message or modal — not just `eprintln!`).

### 8.4 Save As

`Ctrl+Shift+S` — same as save but always prompts for a path.

### 8.5 Close Tab

`Ctrl+W` — already implemented in `tab_bar.rs`. If `tab.is_modified`, show a confirmation dialog before closing.

### 8.6 Modified Indicator

Display a dot or asterisk in the tab title when `tab.is_modified` is true (e.g. `• filename.docx`). Already wired — `tab.is_modified` exists, just needs to be read in `tab_bar.rs` render.

---

## 9. Implementation Notes and Constraints

### 9.1 Framework

GPUI is the only UI framework. Do not introduce `gtk`, `winit`, `egui`, or any other UI library. GPUI runs the event loop; all rendering is declarative via `Render::render`.

Use `gpui::actions!` to define new actions and `cx.bind_keys` in `main.rs` to register keybindings. Action handlers are registered on elements via `.on_action(cx.listener(...))`.

### 9.2 State Architecture

`AppState` is a GPUI `Entity<AppState>`. All views hold a clone of the same `Entity<AppState>` handle. Mutations go through `state.update(cx, |s, cx| { ... })`. Never hold a raw `&mut AppState` across an await or GPUI frame boundary.

### 9.3 Tab Identity

Tabs are identified by `tab.id: usize` (a stable integer, never reused). Do **not** use the tab's position in `self.tabs` as an identifier — tabs can be reordered and closed, causing indices to shift. This is critical for GPUI element IDs (see `tab_bar.rs` for the existing pattern).

### 9.4 Cursor Position Safety

Always use `tab.content.char_indices()` or `tab.content.is_char_boundary(offset)` before slicing the content string. Rust strings are UTF-8; indexing by byte offset into a multi-byte character panics.

### 9.5 Commit Message Format

Use: `feat: <description>` for new features, `fix: <description>` for bug fixes, `refactor: <description>` for refactors. No scope prefix.

### 9.6 Commenting Standard (from INSTRUCTIONS.md)

1. Every function must have a multi-line block comment immediately below its signature describing what it does.
2. Comment any line that is not self-explanatory.
3. When a feature is complete, append a description of the changes made to `tmp_documentation.md`.

### 9.7 Branch Strategy

- `gui` — current main development branch for UI work
- `docx_parsing` — parser and save logic (already merged into `gui` conceptually; ensure changes go to the right branch)
- Create feature branches for large additions (e.g. `vim-mode`, `rich-text-render`)

### 9.8 Testing

- Run `cargo check` before every commit
- Run `./run.sh` to test the application manually; the script sets `XCURSOR_SIZE=24` to fix cursor scaling on WSL
- No automated UI tests are required at this stage, but `cargo test` must pass (existing config_parsing tests live in `tests/parse_testing.rs`)

---

## 10. Priority Order for Implementation

Implement in this order so each phase produces a working, testable state:

1. **Cursor and basic movement** — add `cursor: usize` to `Tab`; update `text_editor.rs` to handle arrow keys and click-to-position; render a visible cursor
2. **Selection** — add `selection` to `Tab`; implement shift-select, click-drag; render highlight
3. **Clipboard** — cut/copy/paste using GPUI clipboard APIs
4. **Undo/redo** — undo stack per tab
5. **Vim Normal mode** — motions only (no operators yet); mode indicator display
6. **Vim operators and text objects** — `d`, `y`, `c`, visual mode
7. **Vim Command mode** — `:w`, `:q`, `:e`, line jump, search
8. **Rich text display** — render from `doc.paragraphs` with run styling; cursor/selection overlay
9. **Formatting operations** — `apply_formatting` in document model; wire ribbon buttons
10. **Find/replace** — inline bar
11. **File dialogs** — new file, open, save-as
12. **Polish** — modified indicator in tab title, save error UI, vim registers

---

## 11. Optional Features

Not required by the sections above — real vim behaviors worth having, but
out of scope for the corresponding required task unless explicitly picked
up. Each entry names the task it would extend.

### 11.1 Text objects as Visual-mode motions (extends 5.6 / Task G)

Section 5.6's table only specifies "all Normal motions extend selection"
plus its own operator row (`d`/`x`, `y`, `c`, `>`, `<`, `~`, `gU`, `gu`,
`o`) — it does not specify using a text object (`iw`, `i"`, `ip`, etc.,
spec 5.4) to directly set or extend the Visual selection while already in
Visual mode. Real vim supports this (`viw` while in Visual mode selects
the inner word under the cursor, replacing whatever selection existed).
Not required for Task G; a small addition on top of the text-object
resolvers Task F already built (`resolve_vim_text_object`) if picked up
later — each resolver already returns the exact `(start, end)` range
Visual mode would set `tab.selection` to.
