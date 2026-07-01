# Phase 3 — Clipboard: Todo

## Spec (editor_instructions.md §4.4)

- `Ctrl+C` — copy selection to system clipboard (no-op if no selection)
- `Ctrl+X` — cut selection to system clipboard; deletes selected range
- `Ctrl+V` — paste from system clipboard at cursor; replaces active selection first

GPUI clipboard API (available on any `cx` that derefs to `App`):
- `cx.write_to_clipboard(ClipboardItem::new_string(text: String))`
- `cx.read_from_clipboard() -> Option<ClipboardItem>`
- `ClipboardItem::text(&self) -> Option<String>` — extracts the string payload

`ClipboardItem` is re-exported via `use gpui::*;`, so no new imports needed in
`text_editor.rs`.

---

## Files to touch

| File | Change |
|------|--------|
| `src/state.rs` | Add 3 methods to `impl AppState`: `copy_selection`, `cut_selection`, `insert_str` |
| `src/text_editor.rs` | Add `"c"`, `"x"`, `"v"` arms to the Ctrl branch of `handle_key_down` |
| `tmp_documentation.md` | Add Ctrl+C/X/V to key bindings table; update state.rs row |

---

## 1. `src/state.rs` — new methods on `AppState`

### `copy_selection`

Read-only (`&self`). Returns the text in the active selection, or `None` if
there is no selection. Called from `text_editor.rs` via `self.state.read(cx)`.

```rust
pub fn copy_selection(&self) -> Option<String> {
    /*
     * Returns the selected text as an owned String, or None when there is no
     * active selection. Does not modify state; safe to call via entity.read(cx).
     */
    let tab = self.tabs.get(self.active_tab)?;
    let (a, f) = tab.selection?;
    let (start, end) = (a.min(f), a.max(f));
    Some(tab.content[start..end].to_string())
}
```

### `cut_selection`

Mutable (`&mut self`). Calls `delete_selection` and returns the deleted text,
or `None` if there was no selection.

```rust
pub fn cut_selection(&mut self) -> Option<String> {
    /*
     * Extracts the selected text and deletes it. Returns the text so the caller
     * can write it to the clipboard. Returns None when there is no selection.
     * Delegates deletion to delete_selection to keep cursor/is_modified logic
     * in one place.
     */
    let tab = self.tabs.get(self.active_tab)?;
    let (a, f) = tab.selection?;
    let (start, end) = (a.min(f), a.max(f));
    let text = tab.content[start..end].to_string();
    self.delete_selection();
    Some(text)
}
```

### `insert_str`

Mutable (`&mut self`). If a selection is active, deletes it first (reusing
`delete_selection`), then inserts the string at the cursor. Advances the
cursor past the inserted text.

```rust
pub fn insert_str(&mut self, text: &str) {
    /*
     * Inserts a string at the current cursor, replacing any active selection
     * first. Advances the cursor to the end of the inserted text. Mirrors the
     * single-char path in insert_char but handles multi-char payloads from
     * clipboard paste.
     */
    if self.tabs.get(self.active_tab).map(|t| t.selection.is_some()).unwrap_or(false) {
        self.delete_selection();
    }
    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
        tab.content.insert_str(tab.cursor, text);
        tab.cursor += text.len(); // text is valid UTF-8 so len() == byte length
        tab.goal_col = None;
        tab.is_modified = true;
    }
}
```

---

## 2. `src/text_editor.rs` — Ctrl branch additions

The Ctrl branch in `handle_key_down` currently returns early after handling
`"left"/"right"/"home"/"end"` and `"a"`. Add three more arms **before** the
`_ => {}` fallthrough.

**Important:** clipboard calls (`cx.write_to_clipboard`, `cx.read_from_clipboard`)
must be made on `cx: &mut Context<TextEditor>` — the outer handle_key_down cx —
NOT on the inner `cx: &mut Context<AppState>` inside a `state.update(...)` closure.
Read state first, then do the clipboard call, then call notify.

```rust
"c" => {
    // Copy: read-only, use entity.read(cx) so no update closure is needed
    let text = self.state.read(cx).copy_selection();
    if let Some(text) = text {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}
"x" => {
    // Cut: delete selection inside update, return the text to write to clipboard
    let text = self.state.update(cx, |state, cx| {
        let result = state.cut_selection();
        if result.is_some() { cx.notify(); }
        result
    });
    if let Some(text) = text {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.notify();
    }
}
"v" => {
    // Paste: read clipboard first (needs outer cx), then insert inside update
    if let Some(item) = cx.read_from_clipboard() {
        if let Some(text) = item.text() {
            self.state.update(cx, |state, cx| {
                state.insert_str(&text);
                cx.notify();
            });
            cx.notify();
        }
    }
}
```

These go inside the outer `match key { ... }` in the Ctrl branch, alongside the
existing `"left"/"right"/"home"/"end"` and `"a"` arms.

---

## 3. Tests to add in `src/state.rs`

Add to the `#[cfg(test)]` block. Use the existing `make_state()` helper.

```
test_copy_selection_basic          — normal forward selection → correct string
test_copy_selection_backward       — anchor > focus (reversed) → still correct
test_copy_selection_no_selection   — None when selection is None
test_cut_selection_basic           — deletes range, returns text, cursor at start
test_cut_selection_no_selection    — returns None, content unchanged
test_insert_str_no_selection       — inserts at cursor, advances cursor
test_insert_str_replaces_selection — selection deleted first, then text inserted
test_insert_str_empty              — inserting "" is a no-op (no crash)
```

---

## 4. `tmp_documentation.md` updates

- Add to key bindings table:

```
| Ctrl+C | Copy selection to system clipboard |
| Ctrl+X | Cut selection to system clipboard  |
| Ctrl+V | Paste from system clipboard        |
```

- Update the `src/state.rs` row to mention `copy_selection`, `cut_selection`,
  `insert_str`, and the 8 new unit tests (total ~44).

---

## Known constraints / gotchas

- `cx.write_to_clipboard` / `cx.read_from_clipboard` are on `App`, which
  `Context<T>` derefs to. They are available on both the outer
  `Context<TextEditor>` and the inner `Context<AppState>`, but clipboard I/O
  should happen on the **outer** cx so the clipboard call and the state update
  are not interleaved.
- `ClipboardItem::text()` returns `Option<String>` — it is `None` when the
  clipboard holds only image or file entries. A no-op paste on non-text clipboard
  is correct behaviour.
- `insert_str` uses `tab.cursor += text.len()` (byte advance). This is correct
  because `text` came from the clipboard as a valid UTF-8 `String`, so
  `text.len()` equals the number of bytes inserted.
- Ctrl+C with no selection: `copy_selection` returns `None` → no clipboard
  write. This matches most editor behaviour (no-op).
- The Ctrl branch currently has `return;` unconditionally at its end, so any
  key handled here does NOT propagate to global actions. Ctrl+C/X/V have no
  competing global bindings in `main.rs`, so this is fine.
