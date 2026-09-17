# Vimbatim Architecture and Hygiene Review

## Closeout — architecture revision milestones complete

The incremental revision and final encapsulation cleanup are complete on
`ARCHITECTURE`. The original assessment and migration checklist below are retained
as historical context, not a description of the current source tree.

### Delivered boundaries

- Steps 1–3: enforced quality/packaging checks and typed preferences.
- Steps 4–9: format-neutral document types, feature-oriented state modules,
  stable tab identity, `DocumentBuffer`, and explicit state aggregates.
- Steps 10–13: shared command/effect routing, background document operations,
  repository test seams, and typed errors with visible notifications.
- Steps 14–17: editor presentation separation, thin binary/library boundary,
  canonical rich text with a derived index, and strict hygiene gates.
- Post-plan cleanup: private `AppState` fields with read-only selectors and
  focused mutation methods; separate Vim dispatch/motion policy, settings
  presentation, and editor rendering; orphaned field comments removed.

The implementation follows the incremental modular-monolith decision in
[ADR 0001](docs/adr/0001-layered-modular-monolith.md), not the proposed directory
layout literally. `state.rs` remains a valid Rust module root; no renaming to
`state/mod.rs` is needed. Small session state stays private on `AppState` where
moving it would add churn without strengthening a boundary.

### Verified closeout baseline

- 57 Rust source files, 60,171 lines (including tests); `state.rs`: 1,618 lines.
- 1,312 unit tests and 4 integration tests pass.
- `cargo fmt --all --check` and strict all-target/all-feature clippy pass.
- `tools/check_packaging.sh` passes (static packaging contract checks).
- `cargo audit` reports no vulnerabilities, without advisory exceptions.
- Refreshed Graphify code graph reports no import cycles.
- [architecture_inventory.md](architecture_inventory.md) is tracked and generated
  recursively with `python3 tools/architecture_inventory.py`.

### Remaining maintenance, not additional extraction milestones

The original checklist describes a broader ideal as well as the migration.
This closeout does not claim that every filesystem operation is asynchronous or
that all settings/workspace failures have been converted to typed effects:
small settings writes, lazy directory expansion and some workspace operations
still use synchronous paths. UI substates also retain narrowly scoped mutable
accessors. These are explicit follow-up boundaries, not reasons to split files
further merely to reduce line counts.

Manual desktop interaction and fresh-package testing on each supported OS remain
release validation; local Rust tests and the packaging script do not replace them.
Further DOCX reader/writer or ribbon splits are optional and should follow a
concrete maintenance need.

Four informational unmaintained-dependency warnings remain: `paste`,
`proc-macro-error2`, `rustybuzz`, and `ttf-parser`. Upstream migrations are deferred
by decision; no local fork stack or audit exceptions are maintained. GPUI remains
pinned to Zed v1.9.0. Revisit these warnings when upstream replacements are usable.

---

## Original executive summary

Vimbatim is a **single-process desktop modular monolith** built around one GPUI `Entity<AppState>`. The basic shape is appropriate for the product: one process, one window, shared tabs, native file access, and no need for services, networking, or a database. Its strongest architectural qualities are a rich automated test suite, a mostly GPUI-free core state type, stable tab IDs in newer asynchronous paths, and explicit document/recovery models.

The main problem is not the choice of a monolith; it is that the monolith has lost its internal boundaries. `src/state.rs` is about 20,000 lines (40% of all Rust source), `src/text_editor.rs` is about 7,200, and `src/docx_parser.rs` is about 4,400. `AppState` mixes document editing, Vim interpretation, workspace filesystem operations, settings persistence, modal state, clipboard mailboxes, navigation, recovery coordination, and user preferences. Twenty-five of thirty source modules reference `AppState`. UI components can directly mutate public fields, while important invariants rely on callers remembering to update several related fields.

The optimal target is a **layered modular monolith with feature-oriented modules**, not a rewrite and not microservices. Keep GPUI and a single root `Entity<AppModel>`, but make it a composition root over smaller state aggregates. Make the rich document the canonical model, route behavior through commands, isolate filesystem/OOXML/settings behind repository/adaptor interfaces, and let views render selectors plus dispatch commands rather than edit fields directly.

The migration plan at the bottom is deliberately incremental. It starts with hygiene and mechanical extraction, preserves existing public behavior, and puts characterization tests around every boundary before changing it.

## Review scope and generated tooling

This review covered source layout, module dependencies, state ownership, UI composition, document editing, OOXML persistence, settings, recovery, filesystem operations, command/keybind routing, tests, and CI.

Added:

- `tools/architecture_inventory.py` — dependency-free source inventory generator.
- `architecture_inventory.md` — generated inventory (now tracked; regenerate it with the tool).
- `.gitignore` exception for this review so `ARCHITECTURE_REVIEW.md` can be committed.

Commands used:

```text
python tools/architecture_inventory.py
./tools/code_review_checks.sh
cargo clippy --all-targets --all-features
```

Current baseline:

- Approximately 50,000 lines under `src/*.rs`.
- `cargo test` passes: 1,288 binary unit tests and 4 integration tests.
- `cargo fmt --check` fails across many files.
- `cargo clippy --all-targets --all-features -- -D warnings` fails.
- The release workflow builds artifacts, but does not run tests, formatting, clippy, or a dependency advisory scan.

## Current architecture

### 1. Composition and lifecycle

`src/main.rs` is the executable composition root. It:

1. installs panic/recovery logging;
2. seeds the settings file;
3. starts GPUI;
4. loads bundled and imported fonts;
5. loads and registers keybindings;
6. opens one `MainWindow`;
7. intercepts native close requests.

`src/main_window.rs` constructs one shared `Entity<AppState>` and all GPUI views. It also owns the recovery timer and globally registers each GPUI action. This is effectively a **Presentation Model / shared mutable model** architecture using GPUI's entity observer mechanism.

### 2. Shared application model

`src/state.rs` defines:

- `Tab`, cursor, selection, undo/redo, Vim state, folding, and document metadata;
- `AppState`, containing tabs, pane state, workspace tree, menus/modals, preferences, keybinds, timer, registers, clipboard handoff, recovery entries, and more;
- most application use cases as inherent `AppState` methods;
- settings loading/saving helpers;
- file and directory operations;
- text motion and Vim command logic;
- the majority of tests.

Every view receives the same entity and commonly calls `state.read(cx)` or `state.update(cx, ...)` directly.

### 3. Domain/document model

`src/docx_parser.rs` currently owns the domain types `Paragraph`, `Run`, `Alignment`, `ListKind`, and `CardStyle`, in addition to parsing and serializing OOXML packages. `DocxOrigin` preserves source ZIP bytes and unsupported XML for round-trip saves.

A tab stores two representations of its document:

- `content: String`, used for byte-offset cursor/Vim/search operations;
- `paragraphs: Vec<Paragraph>`, used for formatting, rendering, and DOCX output.

`src/document_ops.rs` is the synchronization and rich-edit layer. It resolves flat byte offsets into paragraph/run positions and attempts to keep both representations aligned.

### 4. Input and command handling

- `src/keybinds.rs` defines configurable actions and GPUI action mappings.
- `src/vim_keybinds.rs` stores configurable Vim sequences.
- `AppState` contains the Vim state machine and editing behavior.
- `TextEditor` interprets GPUI key events and bridges clipboard/window-only effects.
- `MainWindow::register_global_actions` maps global actions to `AppState` calls.
- Ribbon, toolbar, context-menu, and command-palette components also invoke state methods directly.

There is therefore a command vocabulary, but no single command execution boundary.

### 5. Presentation

GPUI view modules include `text_editor`, `formatting_ribbon`, `settings_modal`, `file_explorer`, `tab_bar`, and smaller modal/panel modules. Most combine:

- event interpretation;
- direct state reads and mutations;
- view-model calculation;
- rendering;
- occasionally native dialogs, clipboard access, or filesystem-triggering calls.

`TextEditor` also owns layout, wrapping, hit testing, caches, scrolling, spellcheck overlays, selection painting, and input routing.

### 6. Infrastructure and persistence

Persistence is function-based rather than behind interfaces:

- `docx_parser.rs`: OOXML ZIP parser/writer;
- `recovery.rs`: snapshots and metadata;
- `state.rs`, `theme.rs`, `keybinds.rs`, `vim_keybinds.rs`, and `config_parsing/`: settings parsing/writing;
- `font_import.rs`: font ZIP parsing, persistence, and GPUI activation;
- `state.rs`: workspace scanning and file operations;
- `wikifi_export.rs`: Markdown export.

Most functions call `std::fs` directly, and several are called while an `AppState` update is running on the UI thread.

### 7. Tests and build

The project has unusually strong regression coverage by count, but tests are concentrated inside production files: `state.rs` alone contains about 769 tests. `src/lib.rs` exports only the legacy config parser, while the actual application is declared as private modules under `main.rs`. Consequently, almost all application tests are binary-local unit tests rather than tests against a reusable library boundary.

The GitHub workflow is packaging-oriented and manually triggered. It does not establish a continuous quality gate.

## Architectural strengths worth preserving

1. **Single root state is appropriate.** A desktop editor benefits from one authoritative application model. Replace the god object, not the single-process model.
2. **The core state avoids GPUI types.** Plain tuples and data types allow substantial logic to be tested without launching a window.
3. **Regression coverage is extensive.** Existing tests should be treated as migration protection, not rewritten wholesale.
4. **Stable IDs are already used where races matter.** Recovery and split-pane work show awareness that vector indices are not identities.
5. **Document mutations have a partial choke point.** `document_ops` and undo snapshot methods provide a starting seam for encapsulation.
6. **Infrastructure is mostly function-based.** Free functions are easier to wrap behind repositories than deeply embedded framework objects.
7. **Recovery I/O is already moved off the UI executor.** Its collect-under-read/write-in-background pattern is a useful model for open/save/import.

## Flaws and risks in the current architecture

### A. `AppState` is a god object and service locator

`AppState` is both data and the implementation of nearly every use case. It owns editor, workspace, settings, filesystem, modal, recovery, theme, timer, and Vim concerns. Public fields allow any of 25 dependent modules to bypass intended methods.

Effects:

- unrelated changes collide in one file;
- invariants are implicit and distributed;
- code review requires global knowledge;
- small features increase compile and test scope;
- mocking I/O is difficult;
- UI code can create states that constructors/use cases would forbid.

Splitting the file alone is useful hygiene, but not sufficient architecture. The ownership boundaries must eventually become explicit types.

### B. The document has two writable sources of truth

`Tab.content` and `Tab.paragraphs` are both public and writable. Correctness depends on every mutation updating both, preserving UTF-8 boundaries, clearing unsupported XML, incrementing `content_version`, updating dirty state, managing folds, and recording undo consistently.

This is the highest long-term correctness risk. The code has good tests around known paths, but the type system does not prevent a new path from modifying only one representation. It also doubles snapshot memory and encourages repeated full-document reconstruction.

A `DocumentBuffer` aggregate should own these details. In the final design, rich paragraphs are canonical and flat text plus offset indexes are derived/cached. During migration, both can remain stored, but must become private behind invariant-preserving methods.

### C. Domain types are owned by the DOCX adapter

`Run`, `Paragraph`, `Alignment`, `CardStyle`, and `ListKind` are editor-domain concepts, yet they live in `docx_parser`. As a result, editing, clipboard, rendering, export, state, and recovery all depend on an infrastructure module. `docx_parser` also depends back on `document_ops` for normalization, creating an inverted/cyclic conceptual dependency.

The domain model should be format-neutral. DOCX should translate to/from it through a repository/adapter.

### D. Presentation, application logic, and effects are interleaved

Views call state methods and edit public flags directly. `MainWindow` handles clipboard, native dialogs, save/open, and formatting actions in one long registration function. Similar actions can enter through the ribbon, palette, toolbar, Vim, or keybind and do not all pass through one executor.

This creates behavioral drift: adding a new action currently requires edits in the action enum, GPUI action types, keymap building, global registration, palette/ribbon code, and sometimes Vim bridging. Notifications and error handling are also caller-dependent.

### E. Synchronous I/O can run on the UI thread

Document open/save, workspace scanning, settings writes, dictionary writes, and some file operations are reachable inside synchronous state/view callbacks. Large documents and slow/network filesystems can freeze rendering. Recovery correctly demonstrates the safer asynchronous pattern, but it is not generalized.

### F. Settings have no single typed schema or repository

Settings are parsed independently by `state`, `theme`, `keybinds`, `vim_keybinds`, and legacy `config_parsing`. Defaults are repeated in code and `default_settings.conf`; writes replace individual lines from multiple modules.

There is already a concrete symptom: `default_settings.conf` defines `paragraph_integrity` and `pilcrows`, but `AppState::new()` hardcodes both to `false` rather than loading them. Schema drift is therefore not hypothetical.

A second concrete packaging mismatch exists: the Windows workflow copies `settings.conf`, while runtime seeding searches for `default_settings.conf`. Fresh Windows artifacts may fail to seed the intended defaults and silently use hardcoded fallbacks. The tracked `settings.conf` also contains developer preferences and should not be a release resource.

### G. Errors are strings/log lines rather than application state

Many failures are logged to `crash.log` and otherwise hidden. The detached-corrupt-document behavior is safer than overwriting a file, but the user receives no clear in-app explanation. Save/open/import/recovery errors need typed categories and a visible notification surface.

`Box<dyn Error>`, `String`, `Option`, and best-effort logging are mixed, making it difficult for callers to decide whether to retry, notify, or abort.

### H. Heavy operations rely on full clones and global rebuilds

Undo/redo and recovery clone full strings and paragraph trees. Row wrapping and some searches walk the whole document. A byte budget now limits the worst undo growth, which is good mitigation, but the architecture still scales by copying aggregates.

The target should retain the Memento pattern initially, then permit deltas/persistent data later. A piece table/rope is not required for this migration and would be an unnecessarily risky rewrite now.

### I. Identity is inconsistent

Some paths correctly use stable `Tab.id`; others expose and pass vector indices (`active_tab`, `PendingClose::Tab(usize)`, many methods). Reordering or asynchronous dialogs can make indices stale. Existing comments document several bugs from exactly this distinction.

Use a `TabId` newtype at boundaries and resolve to an index only immediately before synchronous access.

### J. Renderer modules are oversized and calculation-heavy

`text_editor.rs` combines controller, layout engine, cache management, hit testing, style resolution, and rendering. `settings_modal.rs` and `formatting_ribbon.rs` similarly combine large render trees with behavior. Pure calculations are testable, but their location obscures which code is presentation-independent.

Extracting `editor_layout`, `editor_input`, and `editor_style` modules would improve ownership without changing GPUI behavior.

### K. The crate boundary does not represent the application

`lib.rs` contains only `config_parsing`; `main.rs` declares the real application modules. This prevents clean external integration tests, makes the binary the effective library, and leaves a legacy parser as the only advertised API.

The application should be a library crate with a thin binary entry point. Public exposure can remain narrow through an `app::run()` facade.

### L. Hygiene and delivery controls are weak

- `cargo fmt --check` currently fails broadly.
- strict clippy fails; non-strict clippy reports many warnings.
- CI builds release packages but does not test or lint.
- no dependency advisory scan is present.
- git dependencies rely on the lockfile rather than explicit `rev` declarations.
- `.gitignore` ignored every Markdown file except README, including architecture/review documents.
- a misspelled tracked `.gitigore` exists.
- root contains duplicate/scratch artifacts (`bebas_neue.zip`, notes, bug CSV, local `settings.conf`) without a clear project-documentation policy.
- very long historical comments frequently explain bug chronology and refer to absent/ignored design documents. Valuable rationale should be tests, short invariants, or ADRs; source comments should explain current constraints.

## Recommended target architecture

### Architectural style

Use a **layered, feature-oriented modular monolith** with **unidirectional command flow**:

```text
GPUI events
    -> AppCommand
    -> Application Controller / command handler
    -> Domain aggregates
    -> Effect requests
    -> Infrastructure adapters
    -> AppEvent / updated model
    -> GPUI render via selectors
```

Keep one root GPUI entity. Do not introduce dependency injection frameworks, async runtimes beyond what GPUI already provides, an event bus, microservices, or a database.

### Proposed module layout

```text
src/
  lib.rs                    # narrow library surface; exports app::run
  main.rs                   # panic hook + vimbatim::app::run()
  app/
    mod.rs                  # composition root
    model.rs                # AppModel composed of sub-states
    command.rs              # AppCommand, AppEffect, command handler
    selectors.rs            # read-only view projections
    error.rs                # AppError and user-facing severity
  domain/
    document.rs             # Document, Paragraph, Run, styles
    buffer.rs               # DocumentBuffer invariant boundary
    edit.rs                 # edit/format commands
    history.rs              # Memento-based undo/redo
    selection.rs            # offsets, ranges, TabId newtypes
    vim.rs                  # Vim state machine/input strategy
  workspace/
    model.rs                # tabs, panes, file tree
    commands.rs             # open/close/move/rename behavior
  preferences/
    model.rs                # typed Settings with defaults/validation
    service.rs              # load/update/save orchestration
  infrastructure/
    docx.rs                 # DocumentRepository adapter
    settings_file.rs        # SettingsRepository adapter
    recovery_file.rs        # RecoveryRepository adapter
    workspace_fs.rs         # WorkspaceRepository adapter
    fonts.rs                # FontRepository/GPUI font adapter
  presentation/
    main_window.rs
    editor/
      view.rs
      input.rs
      layout.rs
      style.rs
    ...existing views...
```

Exact folders are less important than the dependency rule:

```text
presentation -> application -> domain
                         \-> ports (traits)
infrastructure implements ports and may depend on domain

domain must not depend on GPUI, ZIP, filesystem, settings format, or views
```

### Explicit design patterns

1. **Aggregate Root — `DocumentBuffer` / `DocumentSession`**
   - Own `Document`, selection, history, dirty/version state, and any temporary flat-text cache.
   - Expose operations such as `insert`, `delete`, `format`, `undo`, and `replace_document`.
   - Enforce “at least one paragraph/run,” UTF-8 boundary, content/paragraph consistency, version, and dirty-state invariants in one place.

2. **Command pattern — `AppCommand`**
   - Represent intent independently of its source: `Save(TabId)`, `ApplyFormat`, `OpenPath`, `ToggleSidebar`, etc.
   - Ribbon, keybind, palette, context menu, and Vim should dispatch the same command.
   - The handler returns `AppEffect` values for operations requiring GPUI/platform access, such as file dialogs and clipboard reads.

3. **Repository pattern / Ports and Adapters**
   - `DocumentRepository`: load/save a format-neutral `DocumentSession` payload.
   - `SettingsRepository`: load/save one typed `Settings` value.
   - `RecoveryRepository`: list/write/delete snapshots.
   - `WorkspaceRepository`: scan/copy/move/delete paths.
   - Production adapters use DOCX/filesystem; tests use in-memory fakes.

4. **Strategy pattern — input modes**
   - `PlainInputStrategy` and `VimInputStrategy` translate keystrokes into domain/app commands.
   - Vim state remains testable and no longer requires `AppState` to own every command implementation.

5. **Memento pattern — undo/redo**
   - Keep current snapshots first, but encapsulate them in `History` with the byte budget.
   - Delta-based history can be a later optimization without changing callers.

6. **Facade pattern — format and platform boundaries**
   - `DocxRepository` hides ZIP/XML/source-preservation details.
   - `AppRuntime` or `PlatformServices` hides GPUI dialogs/clipboard/font registration from application commands.

7. **Observer pattern — retain GPUI `Entity` notifications**
   - GPUI already provides observation. Avoid adding a second event system.

8. **Selectors / CQRS-lite for reads**
   - Views use small read-only queries such as `active_document_view()` or `toolbar_state()` instead of reading dozens of public fields.
   - This is not full CQRS; it simply separates mutation commands from render projections.

### Target state composition

```rust
struct AppModel {
    workspace: WorkspaceState,
    preferences: Preferences,
    ui: UiState,
    vim: GlobalVimState,
    recovery: RecoveryState,
}

struct TabSession {
    id: TabId,
    title: String,
    path: Option<PathBuf>,
    document: DocumentBuffer,
    view: TabViewState,
    vim: VimSessionState,
}
```

Fields should be private by default. Views should not set dirty flags, versions, paths, or selections independently.

## Decisions and assumptions to confirm

1. The recommendation assumes preserving all current behavior is more important than adopting a rope/piece table immediately.
2. It assumes settings should be seeded from `default_settings.conf`, and tracked `settings.conf` is a developer-local sample rather than a release artifact.
3. It assumes a corrupt DOCX should produce a visible non-blocking error and remain detached unless the user explicitly chooses Save As.
4. It assumes no public Rust API compatibility is required; the current library surface appears accidental and limited to the legacy parser.

## Incremental migration plan for a smaller coding agent

### Rules for every step

- Make one numbered step per commit/PR.
- Do not combine mechanical file moves with behavior changes.
- Run `cargo test -q` after every step.
- For behavior-changing steps, add a failing regression test first.
- Preserve GPUI behavior and keyboard bindings unless a step explicitly says otherwise.
- Do not introduce a rope, ECS, event bus, dependency-injection framework, or new async runtime.
- Do not delete existing tests; move them only when their implementation moves.
- Before starting, run `python tools/architecture_inventory.py` and save the baseline locally.

### Step 1 — Establish enforceable hygiene

1. Run `cargo fmt` and commit formatting only.
2. Implement `Default` for `config_parsing::Settings` by delegating to `new()`.
3. Fix only low-risk compiler warnings: unused imports/variables and the tracked `.gitigore` typo. Do not refactor complex functions yet.
4. Add a `quality` CI job that runs `cargo fmt --check`, `cargo test --all-targets`, and `cargo clippy --all-targets --all-features`. Initially do not use `-D warnings`; add it only after warning cleanup.
5. Make packaging jobs depend on `quality`.

Acceptance: the three quality commands pass, except documented non-denied clippy warnings.

### Step 2 — Fix the settings packaging contract

1. In `.github/workflows/beta-build.yml`, package `default_settings.conf` for Windows instead of `settings.conf`.
2. Add a test around `bundled_default_settings_path` or a small packaging-validation script asserting every platform artifact specification includes `default_settings.conf`.
3. Stop treating root `settings.conf` as a shipping input. Keep it only as an explicitly named sample or remove it from version control in a separate commit.

Acceptance: a fresh unpacked Windows artifact can seed `%APPDATA%/vimbatim/settings.conf`.

### Step 3 — Create one typed preferences loader without changing `AppState`

1. Add `src/preferences.rs` with a `Preferences` struct containing all persisted non-keybind settings currently stored directly on `AppState`.
2. Implement `Default`, `load(&Path) -> Result<Preferences, PreferencesError>`, validation/clamping, and `save`/`update` methods.
3. Move existing parsing helpers from `state.rs`/`theme.rs` into this module; initially preserve the current line-based file format.
4. Add fixture tests proving every key in `default_settings.conf` is recognized.
5. Specifically load `paragraph_integrity` and `pilcrows`; add regression tests because they are currently hardcoded false.
6. Have `AppState::new()` load `Preferences` once, then copy values into its existing fields. Do not nest fields yet.

Acceptance: startup reads the settings file once through a typed schema, defaults come from one `Default` implementation, and all existing tests pass.

### Step 4 — Extract domain types from `docx_parser`

1. Add `src/document.rs`.
2. Move only `Run`, `Paragraph`, `Alignment`, `ListItem`, `ListKind`, `CardStyle`, `DocDefaults`, and `NewDocStyle` into it.
3. Re-export those names temporarily from `docx_parser` so existing imports keep compiling:
   `pub use crate::document::{...};`
4. Update `document_ops` to import from `document`, not `docx_parser`.
5. Keep parsing and serialization behavior unchanged.

Acceptance: the domain module has no `gpui`, `zip`, `quick_xml`, filesystem, or view imports; test output is unchanged.

### Step 5 — Break the conceptual `docx_parser`/`document_ops` cycle

1. Move run-normalization primitives required by parsing into `document` or a new `document::normalize` module.
2. Make both `docx_parser` and `document_ops` depend on that module.
3. Confirm `docx_parser` no longer imports `document_ops`.

Acceptance: dependency direction is `document_ops -> document` and `docx_parser -> document`, never the reverse.

### Step 6 — Mechanically split `state.rs`

Do not redesign fields in this step. Convert `state.rs` to `state/mod.rs`, then move inherent `impl AppState` blocks and their tests into child modules:

- `state/editing.rs` — insert/delete/format/undo/search;
- `state/vim.rs` — Vim modes, motions, operators, registers, macros;
- `state/workspace.rs` — tabs, panes, file tree, open/save/file operations;
- `state/preferences.rs` — preference setters and persistence delegates;
- `state/ui.rs` — modal/menu/timer visibility and view state.

Use `pub(super)` for shared helpers rather than making everything `pub`. Keep data types and `AppState` declaration in `state/mod.rs`.

Acceptance: no behavior changes, `state/mod.rs` is under 3,000 lines, and all moved tests still pass.

### Step 7 — Introduce stable IDs at asynchronous and destructive boundaries

1. Add `TabId(usize)` as a newtype in `document.rs` or `workspace/model.rs`.
2. Change `Tab.id`, `secondary_tab_id`, `primary_tab_id`, recovery snapshot IDs, and pending-close targets to `TabId`.
3. Keep `active_tab` as an internal index temporarily.
4. Require dialogs/background tasks to capture `TabId`, then resolve it immediately before mutation.
5. Add tests for tab reorder/close while Save As and recovery operations are pending.

Acceptance: no asynchronous callback stores a tab vector index.

### Step 8 — Add `DocumentBuffer` as an invariant boundary

1. Add `DocumentBuffer` containing the existing `content`, `paragraphs`, `content_version`, dirty flag, undo/redo stacks, and edit timestamp.
2. Initially preserve both text representations; this step is encapsulation, not canonicalization.
3. Move edit methods from `AppState`/`document_ops` onto `DocumentBuffer` or make them private helpers called only by it.
4. Add `debug_assert_valid()` that checks:
   - at least one paragraph and run;
   - `paragraphs_to_plain_text(paragraphs) == content`;
   - cursor/selection bounds and UTF-8 boundaries when passed a selection;
   - history byte-budget limits.
5. Call validation after every mutation in debug/test builds.
6. Replace `Tab` fields with `document: DocumentBuffer`; provide temporary read-only accessors to reduce call-site churn.

Acceptance: no module outside `document`/`document_ops` can independently write flat content or paragraphs.

### Step 9 — Compose `AppState` from sub-states

After Step 8 has reduced direct field access, introduce:

- `WorkspaceState` for tabs/panes/tree;
- `UiState` for menus/modals/sidebar/layout;
- `Preferences` from Step 3;
- `GlobalVimState` for registers/macros/global search;
- `RecoveryState` for pending entries.

Move one group at a time and add temporary forwarding accessors. Make fields private once all callers use methods/selectors.

Acceptance: `AppState` is primarily composition plus cross-feature orchestration, not hundreds of unrelated fields.

### Step 10 — Create a unified command boundary

1. Add `app/command.rs` with `AppCommand` for framework-independent intent.
2. Start with duplicate entry points: formatting, tab switching, sidebar toggles, undo/redo, and card styles.
3. Implement `AppState::execute(command) -> Vec<AppEffect>`.
4. Add `AppEffect` only for platform work (`PromptOpen`, `PromptSaveAs(TabId)`, `ReadClipboard`, `WriteClipboard`, `ShowError`); do not model ordinary state changes as effects.
5. Change ribbon, command palette, Vim custom mappings, and global actions to create the same `AppCommand`.
6. Keep GPUI action registration, but make each closure a thin adapter to the command handler.

Acceptance: each user intent has one behavior implementation regardless of its UI/keybind source.

### Step 11 — Move blocking I/O behind application services

1. Introduce concrete facades first: `DocumentStore`, `WorkspaceFs`, `SettingsStore`, and `RecoveryStore`.
2. Move `std::fs` calls out of state/view modules into those facades.
3. Run document open/save, recursive scans, font ZIP reads, and recovery writes on GPUI's background executor.
4. Capture immutable request data and `TabId`; apply completion on the UI entity only after awaiting.
5. Add loading/saving state so duplicate saves and close-during-save are deterministic.
6. Add file/ZIP size limits while touching document/font loading.

Acceptance: no potentially large file or ZIP operation runs inside a synchronous render/event state update.

### Step 12 — Add repository traits only where tests need substitution

Define narrow traits around the facades:

```rust
trait DocumentRepository { fn load(...); fn save(...); }
trait SettingsRepository { fn load(...); fn save(...); }
trait WorkspaceRepository { fn scan(...); fn move_path(...); }
trait RecoveryRepository { fn list(...); fn write(...); fn delete(...); }
```

Use simple generic parameters or `Arc<dyn Trait>` at the application boundary. Add in-memory fakes for command tests. Do not make every helper a trait.

Acceptance: application command tests can cover load/save/failure flows without touching the real filesystem.

### Step 13 — Add typed errors and visible notifications

1. Add `AppError` variants for document parse, save, settings, workspace, font, and recovery failures.
2. Add `UiState.notifications: Vec<Notification>` with severity and optional action.
3. Convert user-relevant `log_line`-only failures into notifications while retaining diagnostic logging.
4. Show an explicit notification when a corrupt document opens detached.
5. Keep panic/crash logging separate from recoverable application errors.

Acceptance: open/save/import failures are visible and tests assert the correct error category.

### Step 14 — Split the editor presentation module

Move pure code without changing algorithms:

- wrapping/row calculations/cache keys -> `presentation/editor/layout.rs`;
- style/color/font calculations -> `presentation/editor/style.rs`;
- key and mouse translation -> `presentation/editor/input.rs`;
- GPUI element construction -> `presentation/editor/view.rs`.

Then introduce plain and Vim input strategies that return `AppCommand`s. Keep scroll handles and GPUI focus in the view.

Acceptance: layout and input modules are GPUI-light or GPUI-free and retain their existing unit tests.

### Step 15 — Make the application a library with a thin binary

1. Move module declarations from `main.rs` to `lib.rs`.
2. Add `pub fn run()` under `app`, keeping most modules crate-private.
3. Reduce `main.rs` to platform attributes, panic-hook installation if necessary, and `vimbatim::app::run()`.
4. Move selected end-to-end state tests into `tests/` using the command/repository boundaries.
5. Remove the legacy standalone config parser after all consumers/tests use `Preferences`.

Acceptance: `cargo test --lib`, `cargo test --bins`, and integration tests each exercise intentional targets; `lib.rs` represents the real application.

### Step 16 — Canonicalize document text after the safe migration

1. Make rich `Document` paragraphs the sole persisted source of truth.
2. Add a cached `PlainTextIndex` containing flat text and paragraph/run offset mappings.
3. Invalidate/update that index only through `DocumentBuffer` edits.
4. Change Vim/search/render reads to use the cache.
5. Remove the independently writable flat `String` and stop storing duplicate text in undo snapshots.
6. Measure before considering delta history or a rope.

Acceptance: there is one writable document representation, byte-offset lookup remains correct for Unicode, and performance is no worse on existing diagnostic fixtures.

### Step 17 — Final hygiene gate

1. Resolve or narrowly allow remaining clippy warnings with comments.
2. Enable `-D warnings` in CI.
3. Add `cargo audit` or `cargo deny` to CI.
4. Pin git dependencies with explicit revisions.
5. Move durable design rationale into `docs/adr/` and permit those files in `.gitignore`.
6. Shorten source comments to current invariants; preserve bug history in tests/ADRs/git.
7. Regenerate `architecture_inventory.md` and verify no feature module has become a replacement god object.

Acceptance: format, tests, strict clippy, advisory scan, and packaging all pass before artifacts are published.
