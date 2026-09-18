# SVG Icon Integration Plan

## Goal

Use the SVG files currently in `icons/` throughout Vimbatim instead of text labels, Unicode symbols, and hand-built CSS/div icons where a matching asset exists. Preserve every existing action, tooltip, focus behavior, theme state, and platform-specific window-control behavior.

The SVGs are monochrome 24×24 assets with `fill="#000"`. GPUI renders SVGs as alpha masks and applies the element's `text_color`, so do **not** create light/dark copies or rewrite their fills. Tint each icon from the active `Palette`.

## Constraints

- Keep the icons embedded in the executable. Do not depend on the source tree or installation directory at runtime.
- Add no dependency: `include_bytes!`, GPUI's `AssetSource`, and `svg()` are sufficient.
- Keep labels as tooltips/accessibility context where the current UI already provides them. Do not remove visible text where it conveys useful information or where no matching SVG exists.
- Do not change action dispatch, IDs, click handlers, window hit-test areas, keyboard shortcuts, disabled states, or modal behavior.
- Keep the existing generated `Bold`, `Italic`, `Underline`, `Strikethrough`, `Case`, and font-color marks because no corresponding SVG assets exist.
- Keep the maximize/restore glyphs for now because `icons/` has no maximize/restore SVG pair.
- Treat missing assets as a test/build failure, not a silent runtime fallback.

## Phase 1 — Add one embedded asset boundary

Create `src/icons.rs` with:

1. `VimbatimAssets`, implementing `gpui::AssetSource`.
   - `load(path)` must return `Cow::Borrowed(include_bytes!(...))` for every SVG below.
   - Use stable logical paths such as `icons/save.svg`.
   - Return `Ok(None)` for unknown paths.
   - `list("icons")` should return the known logical paths or names; other paths can return an empty vector.
   - Keep the mapping explicit. Forty-eight match arms are boring but compile-time checked and avoid a new embedding crate/build script.

2. A small `Icon` enum containing one variant per file, with `Icon::path() -> &'static str`.

3. One rendering helper, for example:

   ```rust
   pub(crate) fn icon(icon: Icon, color: u32, size: f32) -> gpui::Svg {
       gpui::svg()
           .path(icon.path())
           .size(gpui::px(size))
           .text_color(gpui::rgb(color))
   }
   ```

   If this pinned GPUI revision does not accept `.size(px(...))`, use matching `.w(px(...)).h(px(...))`; do not wrap every icon in a custom view.

4. Register the module in `src/lib.rs` and change startup from:

   ```rust
   application().run(...)
   ```

   to:

   ```rust
   application().with_assets(VimbatimAssets).run(...)
   ```

5. Keep font embedding unchanged. The icon `AssetSource` is only for GPUI SVG loading.

### Asset inventory

Embed all of these exact files:

- `align-center.svg`
- `align-left.svg`
- `align-right.svg`
- `card-menu.svg`
- `clear-formatting.svg`
- `condense.svg`
- `disclosure-collapsed.svg`
- `disclosure-expanded.svg`
- `doc-menu.svg`
- `emphasis.svg`
- `eye-closed.svg`
- `eye-open.svg`
- `file-docx.svg`
- `file-open.svg`
- `find.svg`
- `fold.svg`
- `folder-open.svg`
- `folder.svg`
- `font-size.svg`
- `highlight-bucket.svg`
- `highlight.svg`
- `list-bullet.svg`
- `list-numbered.svg`
- `nav.svg`
- `open-wiki.svg`
- `paste.svg`
- `refresh.svg`
- `save-as.svg`
- `save.svg`
- `search-list.svg`
- `settings-appearance.svg`
- `settings-fonts.svg`
- `settings-keybindings.svg`
- `settings-text.svg`
- `settings-toggles.svg`
- `settings.svg`
- `shrink.svg`
- `sidebar.svg`
- `split.svg`
- `switch-tab.svg`
- `tab-close.svg`
- `tab-new.svg`
- `tabroom.svg`
- `timer.svg`
- `wikifi.svg`
- `window-close.svg`
- `window-minimise.svg`
- `word-count.svg`

The directory is currently untracked; add all SVGs to Git in the implementation commit.

## Phase 2 — Convert shared/high-visibility controls first

### `src/app_toolbar.rs`

Retain each existing button's ID, action, hover state, border, and text tooltip. Replace or augment the visible content as follows:

| Existing control | SVG | Presentation |
|---|---|---|
| Show/Hide Files | `sidebar.svg` | Icon plus the existing short label, because the state-dependent text remains useful |
| Open Folder | `folder-open.svg` | Icon plus label initially |
| Open File | `file-open.svg` | Icon plus label initially |
| Search From List | `search-list.svg` | 16px icon; keep tooltip/label if space permits |
| Find | `find.svg` | 16px icon |
| Word Count | `word-count.svg` | 16px icon |
| Save As | `save-as.svg` | 16px icon |
| Save | `save.svg` | 16px icon |
| Settings `⚙` | `settings.svg` | Replace glyph with 18px icon |

Use `p.text_muted` at rest, `p.text` on hover, and the current selected/accent color where the control already has selected state. Do not hard-code black or white.

### `src/tab_bar.rs`

| Existing control | SVG |
|---|---|
| New tab `+` | `tab-new.svg` |
| Tab close `×` | `tab-close.svg` |
| App close `×` | `window-close.svg` |
| Minimize `−` | `window-minimise.svg` |
| Tab switch/menu control, if present | `switch-tab.svg` |

Preserve `WindowControlArea::Drag` and `WindowControlArea::Max` exactly. Leave maximize/restore as `□`/`❐` because no supplied SVG represents those states. Keep the close-button destructive hover color if one exists.

### `src/file_explorer.rs`

Use:

- `folder.svg` for collapsed directories.
- `folder-open.svg` for expanded directories.
- `file-docx.svg` for DOCX files.
- `disclosure-collapsed.svg` / `disclosure-expanded.svg` for tree and Nav disclosure controls.
- `refresh.svg` for refresh.
- `tab-new.svg` for the existing add/new control.
- `sidebar.svg` for Files mode and `nav.svg` for Navigation mode.
- `doc-menu.svg` and `card-menu.svg` for the matching document/card context-menu triggers, where those triggers exist.

Keep row text and indentation. Icons should be fixed-width/flex-none so labels align and long filenames still truncate as before.

## Phase 3 — Replace matching ribbon drawings

In `src/formatting_ribbon.rs`, replace only `RibbonIcon` cases backed by supplied SVGs:

| `RibbonIcon` / action | SVG |
|---|---|
| Align Left | `align-left.svg` |
| Align Center | `align-center.svg` |
| Align Right | `align-right.svg` |
| Highlight bucket | `highlight-bucket.svg` |
| Eye/open visibility | `eye-open.svg` |
| Eye/closed visibility | `eye-closed.svg` |
| Fold | `fold.svg` |
| Split | `split.svg` |
| Bullet list | `list-bullet.svg` |
| Numbered list | `list-numbered.svg` |

Also use the following assets on their existing text buttons/menu triggers where applicable:

- `clear-formatting.svg`
- `condense.svg`
- `emphasis.svg`
- `highlight.svg`
- `shrink.svg`
- `font-size.svg`
- `nav.svg`
- `paste.svg`
- `card-menu.svg`
- `doc-menu.svg`

Do not redesign the ribbon API. The minimum change is to let `RibbonIcon` map to either the existing generated mark or `crate::icons::icon(...)`. Preserve active/selected colors and the current tooltip mechanism (`label` remains the tooltip source).

## Phase 4 — Secondary views

### Settings

In `src/settings_modal/view.rs`, put an icon before each existing sidebar label:

- Appearance → `settings-appearance.svg`
- Text Settings → `settings-text.svg`
- Fonts → `settings-fonts.svg`
- Keybindings → `settings-keybindings.svg`
- Toggle Features → `settings-toggles.svg`

Keep labels visible. Use `p.text` for the selected section and `p.text_muted` otherwise. Replace modal close glyphs with `tab-close.svg` only where doing so does not alter their size/hit target.

### Feature surfaces

Use matching assets at the primary trigger/header controls, without changing behavior:

- `src/timer.rs` → `timer.svg`
- `src/word_count.rs` → `word-count.svg`
- Wiki-open action → `open-wiki.svg`
- Wikifi action/export → `wikifi.svg`
- Tabroom action → `tabroom.svg`
- Find/search-list surfaces → `find.svg` / `search-list.svg`
- Split-pane switch action → `switch-tab.svg` or `split.svg`, according to whether the command switches panes or creates/removes a split

Use repository search to locate the real existing control before editing; do not invent new buttons merely to consume every asset.

## Phase 5 — Tests and packaging gates

Add focused tests in the fewest files possible:

1. **Asset completeness test (`src/icons.rs`)**
   - Iterate `Icon::all()`.
   - Assert every path is unique.
   - Assert `VimbatimAssets.load(icon.path())` returns non-empty bytes.
   - Assert each payload starts with or contains `<svg` and includes `viewBox="0 0 24 24"`.
   - Assert all files in `icons/` are represented by the enum. A small test-only `std::fs::read_dir` comparison is acceptable; runtime code remains embedded.

2. **No external runtime dependency**
   - Temporarily move/rename `icons/` after compiling and run the built binary or an asset-source test against the embedded bytes. The application must not read SVGs from disk.

3. **Update `tools/check_packaging.sh`**
   - Assert all `icons/*.svg` are tracked by Git.
   - Assert the binary uses embedded assets (for example, grep for `.with_assets(VimbatimAssets)`).
   - Do not copy the `icons/` directory into Windows/macOS/Linux distributions unless the implementation deliberately chooses external assets instead; embedding is the preferred plan.

4. Run:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-targets --all-features
   bash tools/check_packaging.sh
   cargo audit
   graphify update .
   ```

5. Manually verify on a desktop build:
   - Every icon appears on first launch (no blank controls before cache warm-up).
   - Icons remain legible in every theme and light/dark mode.
   - Hover, active, selected, disabled, and destructive states tint correctly.
   - 100%, 125%, 150%, and 200% display scaling remain crisp.
   - Tab close/new, native minimize/maximize/close, drag region, Save/Save As, file tree, ribbon, settings navigation, and tooltips still behave identically.

## Implementation order and commits

Use small commits so regressions are easy to isolate:

1. `embed SVG icon assets and add icon helper`
2. `use SVG icons in toolbar and tab chrome`
3. `use SVG icons in file explorer and ribbon`
4. `use SVG icons in settings and feature views`
5. `add icon completeness and packaging checks`

After each UI conversion, run formatting, the focused tests, and `cargo check`. Run the full gate only after the final conversion.

## Acceptance criteria

- All 48 SVG files are tracked, embedded, and covered by the asset completeness test.
- Every existing control with a clear matching asset uses that SVG.
- No action, keybinding, menu, hit target, or platform window behavior changes.
- Icons inherit theme colors; no icon is permanently black.
- The packaged application works without an external `icons/` directory.
- Strict clippy, all tests, packaging checks, and audit complete with no new failures.
- Manual desktop verification is reported separately from automated checks; do not claim visual success based only on unit tests.
