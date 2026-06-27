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
