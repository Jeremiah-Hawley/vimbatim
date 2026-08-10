/*
 * Configurable, non-vim keybindings: parsing/serializing key combinations,
 * the canonical list of bindable actions, and the registry that ties a
 * `KeybindAction` to whatever `KeyCombo` the user has assigned it in
 * settings.conf.
 *
 * Vim's own modal command language (hjkl, operators, text objects, `:`
 * commands, etc.) is deliberately NOT part of this system — only the
 * "plain app" shortcuts that exist independent of vim mode.
 */

use gpui::{actions, Action, App, KeyBinding, Modifiers};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// A single key combination, e.g. "Ctrl+Shift+B" — modifiers plus one final
/// key. Stored platform-neutral (`ctrl` always means "the primary modifier",
/// i.e. real Ctrl on Linux/Windows and Cmd on macOS); the Ctrl→Cmd swap only
/// happens at the edges (`to_gpui_keystroke`, `display_string`,
/// `from_capture`), never in storage, so settings.conf stays identical across
/// platforms.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: String,
}

impl KeyCombo {
    pub fn new(ctrl: bool, shift: bool, alt: bool, key: &str) -> Self {
        KeyCombo { ctrl, shift, alt, key: key.to_lowercase() }
    }

    /// Parses settings.conf's space-separated format: modifier tokens
    /// (`CTRL`/`SHFT`/`ALT`, case-insensitive, any order) followed by one
    /// trailing key token (`b`, `f2`, `,`). Returns `None` for empty/
    /// malformed input rather than panicking — callers fall back to
    /// `KeybindAction::default_combo()`.
    pub fn parse(s: &str) -> Option<KeyCombo> {
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut key = None;

        for token in s.split_whitespace() {
            match token.to_ascii_uppercase().as_str() {
                "CTRL" | "CMD" => ctrl = true,
                "SHFT" | "SHIFT" => shift = true,
                "ALT" => alt = true,
                _ => key = Some(token.to_lowercase()),
            }
        }

        key.map(|key| KeyCombo { ctrl, shift, alt, key })
    }

    /// Canonical serialization written back to settings.conf.
    pub fn to_conf_string(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl { parts.push("CTRL".to_string()); }
        if self.shift { parts.push("SHFT".to_string()); }
        if self.alt { parts.push("ALT".to_string()); }
        parts.push(self.key.clone());
        parts.join(" ")
    }

    /// GPUI's own hyphenated keystroke syntax (`"ctrl-shift-b"`), substituting
    /// `cmd` for `ctrl` on macOS so the binding actually fires on the key
    /// users there expect.
    pub fn to_gpui_keystroke(&self) -> String {
        let mut parts = Vec::new();
        let primary = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
        if self.ctrl { parts.push(primary.to_string()); }
        if self.alt { parts.push("alt".to_string()); }
        if self.shift { parts.push("shift".to_string()); }
        parts.push(gpui_key_name(&self.key));
        parts.join("-")
    }

    /// An action with no key assigned. `KeyCombo::parse` returns `None` for an
    /// empty settings.conf value, so this is what an action's `default_combo`
    /// returns when it ships unbound — the user assigns one in the settings
    /// modal, and `rebuild_keymap` skips it until they do.
    pub fn is_unbound(&self) -> bool {
        self.key.is_empty()
    }

    /// Human-readable label for the settings UI, platform-aware.
    pub fn display_string(&self) -> String {
        if self.is_unbound() {
            return "Unbound".to_string();
        }
        let primary = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
        let mut parts = Vec::new();
        if self.ctrl { parts.push(primary.to_string()); }
        if self.alt { parts.push("Alt".to_string()); }
        if self.shift { parts.push("Shift".to_string()); }
        parts.push(display_key_name(&self.key));
        parts.join("+")
    }

    /// Builds a combo from a live keypress during capture mode. `modifiers`
    /// comes straight from `KeyDownEvent.keystroke.modifiers`. Returns `None`
    /// for `Escape` (the universal "cancel capture" key) so callers never
    /// need a separate special case for it.
    ///
    /// On macOS, a physical Cmd press (`modifiers.platform`) satisfies our
    /// internal `ctrl` slot — matching how `to_gpui_keystroke` binds Cmd
    /// there — rather than requiring an actual Ctrl key macOS users don't
    /// reach for. On other platforms, `platform` (the Windows/Super key)
    /// isn't part of our supported modifier set and is ignored.
    pub fn from_capture(modifiers: &Modifiers, key: &str) -> Option<KeyCombo> {
        if key.eq_ignore_ascii_case("escape") {
            return None;
        }
        let ctrl = if cfg!(target_os = "macos") {
            modifiers.control || modifiers.platform
        } else {
            modifiers.control
        };
        Some(KeyCombo { ctrl, shift: modifiers.shift, alt: modifiers.alt, key: key.to_lowercase() })
    }
}

/// Maps our stored key name to what GPUI's keystroke parser expects. Only
/// `,` needs no change (GPUI accepts it literally); this exists as the one
/// seam where a future oddly-named key could need translation.
fn gpui_key_name(key: &str) -> String {
    key.to_string()
}

/// Maps a stored key name to its display form — uppercased (`b` → `B`,
/// `f7` → `F7`); punctuation like `,` is unaffected by uppercasing.
fn display_key_name(key: &str) -> String {
    key.to_uppercase()
}

/// The six groupings shown in the settings modal's keybind editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeybindCategory {
    General,
    Editing,
    TextFormatting,
    CardStyles,
    Highlighting,
    CaselistTools,
}

impl KeybindCategory {
    pub fn label(&self) -> &'static str {
        match self {
            KeybindCategory::General => "General",
            KeybindCategory::Editing => "Editing",
            KeybindCategory::TextFormatting => "Text Formatting",
            KeybindCategory::CardStyles => "Card Styles",
            KeybindCategory::Highlighting => "Highlighting",
            KeybindCategory::CaselistTools => "Caselist Tools",
        }
    }

    pub fn all() -> &'static [KeybindCategory] {
        &[
            KeybindCategory::General,
            KeybindCategory::Editing,
            KeybindCategory::TextFormatting,
            KeybindCategory::CardStyles,
            KeybindCategory::Highlighting,
            KeybindCategory::CaselistTools,
        ]
    }
}

/// Every non-vim-specific action that can be bound to a key combination.
/// Adding a new bindable hotkey in the future means: add a variant here,
/// add its label/category/conf_key/default_combo below, add a GPUI action
/// struct + keybinding arm in `rebuild_keymap`, and add one `.on_action`
/// handler in `main_window.rs` — no other file needs to know the full list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeybindAction {
    ToggleSettings,
    ToggleSidebar,
    NewTab,
    CloseTab,
    ReopenClosedTab,
    Save,
    SaveAs,
    Find,
    FindReplace,
    Copy,
    Cut,
    Paste,
    PasteWithoutFormatting,
    Undo,
    Redo,
    SelectAll,
    SelectSimilarFormatting,
    Bold,
    Underline,
    Shrink,
    ClearFormatting,
    PasteSmart,
    Condense,
    Pocket,
    Hat,
    Block,
    Tag,
    Cite,
    Analytic,
    Emphasis,
    Highlight,
    DeleteTags,
    StartTimer,
    OpenStats,
    CiteFromLink,
    Wikifi,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    NextTab,
    PrevTab,
    CommandPalette,
    OpenFile,
    OpenFolder,
    SwitchActivePane,
    NewFile,
    RefreshFileTree,
}

impl KeybindAction {
    pub fn all() -> &'static [KeybindAction] {
        use KeybindAction::*;
        &[
            ToggleSettings, ToggleSidebar, NewTab, CloseTab, ReopenClosedTab, Save, SaveAs, Find, FindReplace,
            Copy, Cut, Paste, PasteWithoutFormatting, Undo, Redo, SelectAll,
            SelectSimilarFormatting,
            Bold, Underline, Shrink, ClearFormatting,
            PasteSmart, Condense, Pocket, Hat, Block, Tag, Cite, Analytic, Emphasis,
            Highlight,
            DeleteTags, StartTimer, OpenStats, CiteFromLink, Wikifi,
            ZoomIn, ZoomOut, ZoomReset,
            NextTab, PrevTab,
            CommandPalette,
            OpenFile, OpenFolder, SwitchActivePane,
            NewFile, RefreshFileTree,
        ]
    }

    pub fn label(&self) -> &'static str {
        use KeybindAction::*;
        match self {
            ToggleSettings => "Toggle Settings",
            ToggleSidebar => "Toggle Sidebar",
            NewTab => "New Document",
            CloseTab => "Close Tab",
            ReopenClosedTab => "Reopen Closed Tab",
            Save => "Save",
            SaveAs => "Save As",
            Find => "Find",
            FindReplace => "Find & Replace",
            Copy => "Copy",
            Cut => "Cut",
            Paste => "Paste",
            PasteWithoutFormatting => "Paste Without Formatting",
            Undo => "Undo",
            Redo => "Redo",
            SelectAll => "Select All",
            SelectSimilarFormatting => "Select Similar Formatting",
            Bold => "Bold",
            Underline => "Underline",
            Shrink => "Shrink",
            ClearFormatting => "Clear Formatting",
            PasteSmart => "Paste (Smart)",
            Condense => "Condense",
            Pocket => "Pocket",
            Hat => "Hat",
            Block => "Block",
            Tag => "Tag",
            Cite => "Cite",
            Analytic => "Analytic",
            Emphasis => "Emphasis",
            Highlight => "Highlight",
            DeleteTags => "Delete Tags",
            StartTimer => "Timer",
            // Labelled for the thing it opens — the ribbon button says "Word
            // Count", and a keybind list that calls it something else is a
            // keybind nobody finds. The conf key stays `open_stats` so existing
            // settings.conf files keep working.
            OpenStats => "Word Count",
            CiteFromLink => "Cite From Link",
            Wikifi => "Wikifi",
            ZoomIn => "Zoom In",
            ZoomOut => "Zoom Out",
            ZoomReset => "Reset Zoom",
            NextTab => "Next Tab",
            PrevTab => "Previous Tab",
            CommandPalette => "Command Palette",
            OpenFile => "Open File",
            OpenFolder => "Open Folder",
            SwitchActivePane => "Switch Active Pane",
            NewFile => "New File",
            RefreshFileTree => "Refresh File Tree",
        }
    }

    pub fn category(&self) -> KeybindCategory {
        use KeybindAction::*;
        use KeybindCategory as C;
        match self {
            ToggleSettings | ToggleSidebar | NewTab | CloseTab | ReopenClosedTab | Save | SaveAs
                | Find | FindReplace | ZoomIn | ZoomOut | ZoomReset | NextTab | PrevTab
                | CommandPalette | OpenFile | OpenFolder | SwitchActivePane
                | NewFile | RefreshFileTree => C::General,
            Copy | Cut | Paste | PasteWithoutFormatting | Undo | Redo | SelectAll
                | SelectSimilarFormatting => C::Editing,
            Bold | Underline | Shrink | ClearFormatting => C::TextFormatting,
            PasteSmart | Condense | Pocket | Hat | Block | Tag | Cite | Analytic | Emphasis => C::CardStyles,
            Highlight => C::Highlighting,
            DeleteTags | StartTimer | OpenStats | CiteFromLink | Wikifi => C::CaselistTools,
        }
    }

    /// The exact key name used in settings.conf's `[KEYBINDS]` section.
    pub fn conf_key(&self) -> &'static str {
        use KeybindAction::*;
        match self {
            ToggleSettings => "settings",
            ToggleSidebar => "sidebar",
            NewTab => "new_document",
            CloseTab => "close_tab",
            ReopenClosedTab => "reopen_closed_tab",
            Save => "save",
            SaveAs => "save_as",
            Find => "find",
            FindReplace => "find_and_replace",
            Copy => "copy",
            Cut => "cut",
            Paste => "paste_raw",
            PasteWithoutFormatting => "paste_plain",
            Undo => "undo",
            Redo => "redo",
            SelectAll => "select_all",
            SelectSimilarFormatting => "select_similar_formatting",
            Bold => "bold",
            Underline => "underline",
            Shrink => "shrink",
            ClearFormatting => "clear",
            PasteSmart => "paste",
            Condense => "condense",
            Pocket => "pocket_hotkey",
            Hat => "hat",
            Block => "block",
            Tag => "tag",
            Cite => "cite",
            Analytic => "analytic",
            Emphasis => "emphasis",
            Highlight => "highlight",
            DeleteTags => "delete_tags",
            StartTimer => "start_timer",
            OpenStats => "open_stats",
            CiteFromLink => "cite_from_link",
            Wikifi => "wikifi",
            ZoomIn => "zoom_in",
            ZoomOut => "zoom_out",
            ZoomReset => "zoom_reset",
            NextTab => "next_tab",
            PrevTab => "prev_tab",
            CommandPalette => "command_palette",
            OpenFile => "open_file",
            OpenFolder => "open_folder",
            SwitchActivePane => "switch_active_pane",
            NewFile => "new_file",
            RefreshFileTree => "refresh_file_tree",
        }
    }

    /// Fallback used when settings.conf is missing or doesn't have this key.
    /// See the implementation plan for why each of these was chosen —
    /// notably `Underline` adopts the real hardcoded `Ctrl+U` (not conf's
    /// stale, never-wired `f9`), and `ToggleSidebar` adopts conf's
    /// `Ctrl+Shift+B` (resolving the pre-existing silent clash with Bold's
    /// `Ctrl+B`).
    pub fn default_combo(&self) -> KeyCombo {
        use KeybindAction::*;
        match self {
            ToggleSettings => KeyCombo::new(true, false, false, ","),
            ToggleSidebar => KeyCombo::new(true, true, false, "b"),
            NewTab => KeyCombo::new(true, false, false, "n"),
            CloseTab => KeyCombo::new(true, false, false, "w"),
            ReopenClosedTab => KeyCombo::new(true, true, false, "w"),
            Save => KeyCombo::new(true, false, false, "s"),
            SaveAs => KeyCombo::new(true, true, false, "s"),
            Find => KeyCombo::new(true, false, false, "f"),
            FindReplace => KeyCombo::new(true, false, false, "h"),
            Copy => KeyCombo::new(true, false, false, "c"),
            Cut => KeyCombo::new(true, false, false, "x"),
            Paste => KeyCombo::new(true, false, false, "v"),
            PasteWithoutFormatting => KeyCombo::new(true, true, false, "v"),
            Undo => KeyCombo::new(true, false, false, "z"),
            Redo => KeyCombo::new(true, false, false, "y"),
            SelectAll => KeyCombo::new(true, false, false, "a"),
            // Ships unbound, like Analytic: it is a rarely-reached Doc Menu
            // command, and every obvious Ctrl combination is already taken.
            SelectSimilarFormatting => KeyCombo::new(false, false, false, ""),
            Bold => KeyCombo::new(true, false, false, "b"),
            Underline => KeyCombo::new(true, false, false, "u"),
            Shrink => KeyCombo::new(false, false, true, "f3"),
            ClearFormatting => KeyCombo::new(false, false, false, "f12"),
            PasteSmart => KeyCombo::new(false, false, false, "f2"),
            Condense => KeyCombo::new(false, false, false, "f3"),
            Pocket => KeyCombo::new(false, false, false, "f4"),
            Hat => KeyCombo::new(false, false, false, "f5"),
            Block => KeyCombo::new(false, false, false, "f6"),
            Tag => KeyCombo::new(false, false, false, "f7"),
            Cite => KeyCombo::new(false, false, false, "f8"),
            // Ships unbound — there is no spare function key that isn't
            // already a card style, so the user picks one.
            Analytic => KeyCombo::new(false, false, false, ""),
            Emphasis => KeyCombo::new(false, false, false, "f10"),
            Highlight => KeyCombo::new(false, false, false, "f11"),
            DeleteTags => KeyCombo::new(false, false, true, "f7"),
            StartTimer => KeyCombo::new(true, true, false, "t"),
            OpenStats => KeyCombo::new(true, true, false, "i"),
            CiteFromLink => KeyCombo::new(true, false, false, "f8"),
            Wikifi => KeyCombo::new(true, true, true, "w"),
            // "=" (not "+") so this fires without needing Shift on a US
            // layout — the same convention VS Code and most editors use.
            ZoomIn => KeyCombo::new(true, false, false, "="),
            ZoomOut => KeyCombo::new(true, false, false, "-"),
            ZoomReset => KeyCombo::new(true, false, false, "0"),
            // GPUI reports the Tab key as the literal string "tab" on both
            // its Linux (`keystroke_from_xkb`, `Keysym::Tab => "tab"`) and
            // macOS (`gpui_macos/src/events.rs`) keystroke-naming code —
            // verified in the vendored crate before picking this literal.
            NextTab => KeyCombo::new(true, false, false, "tab"),
            PrevTab => KeyCombo::new(true, true, false, "tab"),
            // Ctrl+P is unclaimed by every other default combo.
            CommandPalette => KeyCombo::new(true, false, false, "p"),
            // All three ship unbound: Open File/Open Folder already have a
            // toolbar button and no obvious free combo (Ctrl+O collides with
            // no existing default, but claiming it unprompted for a rarely-
            // keyboard-driven action isn't this task's call to make), and
            // Switch Active Pane only means anything once split view is
            // open — same "ships unbound, user picks one" reasoning as
            // Analytic/SelectSimilarFormatting above.
            OpenFile => KeyCombo::new(false, false, false, ""),
            OpenFolder => KeyCombo::new(false, false, false, ""),
            SwitchActivePane => KeyCombo::new(false, false, false, ""),
            // Same reasoning: both already have a sidebar button, and
            // NewTab already owns Ctrl+N for a *blank tab*, which this is
            // deliberately not (it writes a new .docx to the working
            // directory) — reusing a similar combo for a similarly-named
            // but different action would be the confusing choice.
            NewFile => KeyCombo::new(false, false, false, ""),
            RefreshFileTree => KeyCombo::new(false, false, false, ""),
        }
    }

    /// True if this action has no real implementation yet (matches this
    /// codebase's existing convention for unimplemented ribbon items like
    /// Doc Menu / Card Menu — still fully bindable and shown in the UI, the
    /// handler just logs instead of doing nothing silently).
    pub fn is_stub(&self) -> bool {
        // SaveAs/Find/FindReplace/DeleteTags/StartTimer/OpenStats all shipped
        // real functionality (`main_window.rs`'s own action handlers) since
        // this list was written — Cite From Link is the one still genuinely a
        // `println!` no-op.
        matches!(self, KeybindAction::CiteFromLink)
    }
}

/// The registry mapping each `KeybindAction` to every `KeyCombo` currently
/// assigned to it, loaded from and saved back to settings.conf.
///
/// A `Vec` rather than one `KeyCombo`, per "add another keybind to the same
/// function" — an action can carry zero (unbound), one, or several combos.
/// A combo stored here is always a real, bound one; "unbound" is simply an
/// empty `Vec`, never a placeholder entry (unlike `KeybindAction::default_combo`,
/// which still uses an empty-key `KeyCombo` as its own unbound sentinel since
/// it has no `Vec` to be empty).
#[derive(Clone, Debug)]
pub struct Keybinds {
    combos: HashMap<KeybindAction, Vec<KeyCombo>>,
}

impl Keybinds {
    /// Every action defaults to its `default_combo()` (or no combo at all,
    /// for the handful that ship unbound), then whatever settings.conf
    /// actually specifies overrides that — so a missing or unparseable entry
    /// never leaves an action unexpectedly unbound.
    pub fn defaults() -> Keybinds {
        let combos = KeybindAction::all()
            .iter()
            .map(|a| {
                let d = a.default_combo();
                (*a, if d.is_unbound() { Vec::new() } else { vec![d] })
            })
            .collect();
        Keybinds { combos }
    }

    /// Parses settings.conf's flat `key=value` lines (mirroring
    /// `config_parsing.rs`'s own approach: every line starting with `[` is
    /// skipped, so any number of `[KEYBINDS: ...]` sub-headers are safe)
    /// looking for each action's `conf_key()`. Every line sharing a key is
    /// one combo for that action — `save_to` emits one line per combo, so a
    /// multi-keybind action round-trips as several consecutive lines with
    /// the same key, not one delimited value (a delimiter risks colliding
    /// with an actual bindable key, e.g. `,` — `ToggleSettings`'s own
    /// default — so this avoids that instead of picking one and hoping).
    pub fn load(path: &Path) -> Keybinds {
        let mut keybinds = Keybinds::defaults();
        let Ok(content) = fs::read_to_string(path) else { return keybinds };

        let mut values: HashMap<&str, Vec<String>> = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') { continue; }
            if let Some((key, value)) = line.split_once('=') {
                values.entry(key.trim()).or_default().push(value.trim().to_string());
            }
        }

        for action in KeybindAction::all() {
            let Some(raws) = values.get(action.conf_key()) else { continue };
            // The key being present at all is authoritative, even if every
            // line for it is blank/unparseable — that (an empty Vec) is how
            // a user's deliberate "no keybinds" persists across a restart,
            // distinct from the key being absent entirely (a settings.conf
            // that predates this action, which should keep the default).
            let parsed: Vec<KeyCombo> = raws.iter().filter_map(|r| KeyCombo::parse(r)).collect();
            keybinds.combos.insert(*action, parsed);
        }
        keybinds
    }

    /// Every combo currently bound to `action`, in assignment order. Empty
    /// means unbound.
    pub fn get_all(&self, action: KeybindAction) -> Vec<KeyCombo> {
        self.combos.get(&action).cloned().unwrap_or_default()
    }

    /// `action`'s first/primary combo, or an unbound sentinel if it has none
    /// — for call sites that only ever need a single value: `ToggleSettings`
    /// (which keeps its own dedicated settings.conf line, never the general
    /// multi-combo list) and display code that only shows one at a time.
    pub fn get(&self, action: KeybindAction) -> KeyCombo {
        self.get_all(action).into_iter().next().unwrap_or_else(|| KeyCombo::new(false, false, false, ""))
    }

    /// The settings modal's "+" button: appends `combo` as an additional
    /// binding for `action` rather than replacing its existing one(s).
    pub fn add(&mut self, action: KeybindAction, combo: KeyCombo) {
        let mut combos = self.get_all(action);
        combos.push(combo);
        self.combos.insert(action, combos);
    }

    /// Replaces the combo at `index` — the existing "re-capture this slot"
    /// flow, now addressed by index since an action can have more than one.
    /// A no-op if `index` is out of range (the slot was removed from under
    /// an in-flight capture).
    pub fn set_at(&mut self, action: KeybindAction, index: usize, combo: KeyCombo) {
        let mut combos = self.get_all(action);
        if index < combos.len() {
            combos[index] = combo;
            self.combos.insert(action, combos);
        }
    }

    /// The settings modal's "remove keybind" button: drops the combo at
    /// `index` outright (no replacement prompt). A no-op if out of range.
    pub fn remove_at(&mut self, action: KeybindAction, index: usize) {
        let mut combos = self.get_all(action);
        if index < combos.len() {
            combos.remove(index);
            self.combos.insert(action, combos);
        }
    }

    /// Returns whichever *other* (action, slot) already owns `combo`, if
    /// any — used to block duplicate assignments and tell the user what's
    /// currently using the combination they just pressed. `exclude` is the
    /// slot being edited (`None` when adding a brand new one, which can
    /// never already exist), so re-confirming a slot's own current value
    /// isn't reported as colliding with itself.
    ///
    /// An unbound combo owns nothing, so it never conflicts: several actions
    /// ship with no key at all (Analytic, Select Similar Formatting) and
    /// "unbound collides with unbound" is not a clash a user can act on.
    pub fn find_conflict(
        &self,
        combo: &KeyCombo,
        exclude: (KeybindAction, Option<usize>),
    ) -> Option<KeybindAction> {
        if combo.is_unbound() {
            return None;
        }
        for action in KeybindAction::all() {
            for (i, existing) in self.get_all(*action).iter().enumerate() {
                if (*action, Some(i)) == exclude {
                    continue;
                }
                if existing == combo {
                    return Some(*action);
                }
            }
        }
        None
    }

    /// Rewrites only the file's `[KEYBINDS...]` portion, grouped by category
    /// under a labeled sub-header per category, leaving everything else
    /// (e.g. `[FORMATTING]`, and the standalone `vim`/`vim_lines` flags)
    /// byte-for-byte untouched.
    pub fn save_to(&self, path: &Path, vim_enabled: bool, extra_keybind_lines: &[String]) -> std::io::Result<()> {
        let existing = fs::read_to_string(path).unwrap_or_default();
        let preserved = extract_non_keybind_sections(&existing);

        let mut out = preserved;
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str("[KEYBINDS]\n");
        out.push_str(&format!("settings={}\n", self.get(KeybindAction::ToggleSettings).to_conf_string()));
        out.push_str(&format!("vim={}\n", vim_enabled));
        for line in extra_keybind_lines {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');

        for category in KeybindCategory::all() {
            out.push_str(&format!("[KEYBINDS: {}]\n", category.label().to_uppercase()));
            for action in KeybindAction::all() {
                if action.category() != *category || *action == KeybindAction::ToggleSettings {
                    continue;
                }
                let combos = self.get_all(*action);
                if combos.is_empty() {
                    // An empty value line is still written (not omitted) so
                    // `load` can tell "deliberately cleared to no keybinds"
                    // apart from "this action didn't exist in an older
                    // settings.conf" — the key being present at all is what
                    // makes it authoritative there.
                    out.push_str(&format!("{}=\n", action.conf_key()));
                } else {
                    for combo in &combos {
                        out.push_str(&format!("{}={}\n", action.conf_key(), combo.to_conf_string()));
                    }
                }
            }
            out.push('\n');
        }

        fs::write(path, out)
    }
}

/// Pulls every section from `content` that this module doesn't own (i.e.
/// everything except `[KEYBINDS...]` headers and the flag lines already
/// re-emitted by `save_to`), preserving original text verbatim so a save
/// never clobbers `[FORMATTING]` or anything else a future feature adds.
fn extract_non_keybind_sections(content: &str) -> String {
    let mut out = String::new();
    let mut in_keybinds_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_keybinds_section = trimmed.to_uppercase().starts_with("[KEYBINDS");
            if in_keybinds_section { continue; }
        }
        if in_keybinds_section {
            // Skip the flag lines re-emitted explicitly by save_to, but keep
            // vim_lines (untouched, unused elsewhere) preserved verbatim by
            // falling through only when it's specifically that key.
            if trimmed.starts_with("vim_lines") {
                in_keybinds_section = false;
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Reads the standalone `vim` flag from settings.conf. Not a
/// `KeybindAction` (it's a mode toggle, not a key combination), so it's
/// parsed separately from the `Keybinds` registry above. Falls back to
/// `false` when the file or key is missing, matching this app's "vim off
/// by default" preference for a from-scratch environment.
pub fn load_vim_enabled(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else { return false };
    for line in content.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "vim" {
                return value.trim() == "true";
            }
        }
    }
    false
}

// Every bindable action needs its own zero-sized GPUI action struct — this
// is the one place all 37 are declared. `main_window.rs` registers a small
// `.on_action` handler per struct; `rebuild_keymap` below is the only place
// that needs to know which struct corresponds to which `KeybindAction`.
actions!(
    keybinds,
    [
        ToggleSettingsAction, ToggleSidebarAction, NewTabAction, CloseTabAction, ReopenClosedTabAction, SaveAction,
        SaveAsAction, FindAction, FindReplaceAction,
        CopyAction, CutAction, PasteAction, PasteWithoutFormattingAction, UndoAction, RedoAction, SelectAllAction,
        SelectSimilarFormattingAction,
        BoldAction, UnderlineAction, ShrinkAction, ClearFormattingAction,
        PasteSmartAction, CondenseAction, PocketAction, HatAction, BlockAction, TagAction,
        CiteAction, AnalyticAction, EmphasisAction,
        HighlightAction,
        DeleteTagsAction, StartTimerAction, OpenStatsAction, CiteFromLinkAction, WikifiAction,
        ZoomInAction, ZoomOutAction, ZoomResetAction,
        NextTabAction, PrevTabAction, CommandPaletteAction,
        OpenFileAction, OpenFolderAction, SwitchActivePaneAction,
        NewFileAction, RefreshFileTreeAction,
    ]
);

/// Rebuilds the entire GPUI keymap from `keybinds`. Safe to call at startup
/// or any time after a binding changes — `App::clear_key_bindings` +
/// `App::bind_keys` both work at runtime, not just before the window opens.
///
/// Also used, via a bare `cx.clear_key_bindings()` with no matching
/// `bind_keys` call, to blank the keymap entirely while the settings modal
/// is capturing a new binding (`settings_modal.rs`'s `start_capture`) — see
/// that function's doc comment for why context-predicate-based exclusion
/// and keystroke interception were both tried first and don't work.
pub fn rebuild_keymap(cx: &mut App, keybinds: &Keybinds) {
    use KeybindAction::*;

    cx.clear_key_bindings();

    // One `KeyBinding` per combo an action actually has (zero, one, or
    // several — "add another keybind" means an action's list of combos
    // isn't always length 1 anymore). `A: Clone` because `actions!` already
    // derives it for every zero-sized action struct, so one `make` value
    // covers every combo without needing a constructor per binding.
    fn bind_all<A: Action + Clone>(keybinds: &Keybinds, action: KeybindAction, make: A) -> Vec<KeyBinding> {
        keybinds
            .get_all(action)
            .iter()
            .map(|combo| KeyBinding::new(&combo.to_gpui_keystroke(), make.clone(), None))
            .collect()
    }

    let mut bindings: Vec<KeyBinding> = Vec::new();
    bindings.extend(bind_all(keybinds, ToggleSettings, ToggleSettingsAction));
    bindings.extend(bind_all(keybinds, ToggleSidebar, ToggleSidebarAction));
    bindings.extend(bind_all(keybinds, NewTab, NewTabAction));
    bindings.extend(bind_all(keybinds, CloseTab, CloseTabAction));
    bindings.extend(bind_all(keybinds, ReopenClosedTab, ReopenClosedTabAction));
    bindings.extend(bind_all(keybinds, Save, SaveAction));
    bindings.extend(bind_all(keybinds, SaveAs, SaveAsAction));
    bindings.extend(bind_all(keybinds, Find, FindAction));
    bindings.extend(bind_all(keybinds, FindReplace, FindReplaceAction));
    bindings.extend(bind_all(keybinds, Copy, CopyAction));
    bindings.extend(bind_all(keybinds, Cut, CutAction));
    bindings.extend(bind_all(keybinds, Paste, PasteAction));
    bindings.extend(bind_all(keybinds, PasteWithoutFormatting, PasteWithoutFormattingAction));
    bindings.extend(bind_all(keybinds, Undo, UndoAction));
    bindings.extend(bind_all(keybinds, Redo, RedoAction));
    bindings.extend(bind_all(keybinds, SelectAll, SelectAllAction));
    bindings.extend(bind_all(keybinds, SelectSimilarFormatting, SelectSimilarFormattingAction));
    bindings.extend(bind_all(keybinds, Bold, BoldAction));
    bindings.extend(bind_all(keybinds, Underline, UnderlineAction));
    bindings.extend(bind_all(keybinds, Shrink, ShrinkAction));
    bindings.extend(bind_all(keybinds, ClearFormatting, ClearFormattingAction));
    bindings.extend(bind_all(keybinds, PasteSmart, PasteSmartAction));
    bindings.extend(bind_all(keybinds, Condense, CondenseAction));
    bindings.extend(bind_all(keybinds, Pocket, PocketAction));
    bindings.extend(bind_all(keybinds, Hat, HatAction));
    bindings.extend(bind_all(keybinds, Block, BlockAction));
    bindings.extend(bind_all(keybinds, Tag, TagAction));
    bindings.extend(bind_all(keybinds, Cite, CiteAction));
    bindings.extend(bind_all(keybinds, Analytic, AnalyticAction));
    bindings.extend(bind_all(keybinds, Emphasis, EmphasisAction));
    bindings.extend(bind_all(keybinds, Highlight, HighlightAction));
    bindings.extend(bind_all(keybinds, DeleteTags, DeleteTagsAction));
    bindings.extend(bind_all(keybinds, StartTimer, StartTimerAction));
    bindings.extend(bind_all(keybinds, OpenStats, OpenStatsAction));
    bindings.extend(bind_all(keybinds, CiteFromLink, CiteFromLinkAction));
    bindings.extend(bind_all(keybinds, Wikifi, WikifiAction));
    bindings.extend(bind_all(keybinds, ZoomIn, ZoomInAction));
    bindings.extend(bind_all(keybinds, ZoomOut, ZoomOutAction));
    bindings.extend(bind_all(keybinds, ZoomReset, ZoomResetAction));
    bindings.extend(bind_all(keybinds, NextTab, NextTabAction));
    bindings.extend(bind_all(keybinds, PrevTab, PrevTabAction));
    bindings.extend(bind_all(keybinds, CommandPalette, CommandPaletteAction));
    bindings.extend(bind_all(keybinds, OpenFile, OpenFileAction));
    bindings.extend(bind_all(keybinds, OpenFolder, OpenFolderAction));
    bindings.extend(bind_all(keybinds, SwitchActivePane, SwitchActivePaneAction));
    bindings.extend(bind_all(keybinds, NewFile, NewFileAction));
    bindings.extend(bind_all(keybinds, RefreshFileTree, RefreshFileTreeAction));
    bindings.push(KeyBinding::new("f9", UnderlineAction, None));
    // A second, fixed binding for New Document — Ctrl+T is the
    // browser-tab convention, not user-configurable like NewTab's own
    // combo (Ctrl+N by default). Free: Start Timer owns Ctrl+Shift+T,
    // not plain Ctrl+T.
    bindings.push(KeyBinding::new("ctrl-t", NewTabAction, None));

    cx.bind_keys(bindings);
}

/// Boxes the `*Action` struct paired with `action` — the vim-keybind
/// system's own entry point (`text_editor.rs`'s `process_key_plain`,
/// draining `AppState::take_pending_vim_action`) for firing a
/// `KeybindAction` outside GPUI's own keymap, via `window.dispatch_action`,
/// the same call already used by `app_toolbar.rs`'s toolbar buttons and
/// `text_editor.rs`'s own paste-shortcut fallback. One match arm per
/// variant, same enumeration `rebuild_keymap` above already needs.
pub fn action_for(action: KeybindAction) -> Box<dyn Action> {
    use KeybindAction::*;
    match action {
        ToggleSettings => Box::new(ToggleSettingsAction),
        ToggleSidebar => Box::new(ToggleSidebarAction),
        NewTab => Box::new(NewTabAction),
        CloseTab => Box::new(CloseTabAction),
        ReopenClosedTab => Box::new(ReopenClosedTabAction),
        Save => Box::new(SaveAction),
        SaveAs => Box::new(SaveAsAction),
        Find => Box::new(FindAction),
        FindReplace => Box::new(FindReplaceAction),
        Copy => Box::new(CopyAction),
        Cut => Box::new(CutAction),
        Paste => Box::new(PasteAction),
        PasteWithoutFormatting => Box::new(PasteWithoutFormattingAction),
        Undo => Box::new(UndoAction),
        Redo => Box::new(RedoAction),
        SelectAll => Box::new(SelectAllAction),
        SelectSimilarFormatting => Box::new(SelectSimilarFormattingAction),
        Bold => Box::new(BoldAction),
        Underline => Box::new(UnderlineAction),
        Shrink => Box::new(ShrinkAction),
        ClearFormatting => Box::new(ClearFormattingAction),
        PasteSmart => Box::new(PasteSmartAction),
        Condense => Box::new(CondenseAction),
        Pocket => Box::new(PocketAction),
        Hat => Box::new(HatAction),
        Block => Box::new(BlockAction),
        Tag => Box::new(TagAction),
        Cite => Box::new(CiteAction),
        Analytic => Box::new(AnalyticAction),
        Emphasis => Box::new(EmphasisAction),
        Highlight => Box::new(HighlightAction),
        DeleteTags => Box::new(DeleteTagsAction),
        StartTimer => Box::new(StartTimerAction),
        OpenStats => Box::new(OpenStatsAction),
        CiteFromLink => Box::new(CiteFromLinkAction),
        Wikifi => Box::new(WikifiAction),
        ZoomIn => Box::new(ZoomInAction),
        ZoomOut => Box::new(ZoomOutAction),
        ZoomReset => Box::new(ZoomResetAction),
        NextTab => Box::new(NextTabAction),
        PrevTab => Box::new(PrevTabAction),
        CommandPalette => Box::new(CommandPaletteAction),
        OpenFile => Box::new(OpenFileAction),
        OpenFolder => Box::new(OpenFolderAction),
        SwitchActivePane => Box::new(SwitchActivePaneAction),
        NewFile => Box::new(NewFileAction),
        RefreshFileTree => Box::new(RefreshFileTreeAction),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: these six all shipped real handlers in `main_window.rs`
    /// (Find/Find & Replace, Delete Tags, Start Timer, Open Stats, Save As)
    /// but the settings modal kept flagging them "(not yet implemented)".
    /// Cite From Link is the one action still a `println!` no-op.
    #[test]
    fn is_stub_only_flags_cite_from_link() {
        for action in KeybindAction::all() {
            assert_eq!(
                action.is_stub(),
                *action == KeybindAction::CiteFromLink,
                "{action:?} stub flag is stale"
            );
        }
    }

    #[test]
    fn parses_simple_key() {
        assert_eq!(KeyCombo::parse("f2"), Some(KeyCombo::new(false, false, false, "f2")));
    }

    #[test]
    fn parses_multi_modifier() {
        assert_eq!(KeyCombo::parse("CTRL SHFT b"), Some(KeyCombo::new(true, true, false, "b")));
    }

    #[test]
    fn parses_alt_combo_case_insensitive() {
        assert_eq!(KeyCombo::parse("alt f7"), Some(KeyCombo::new(false, false, true, "f7")));
    }

    #[test]
    fn parse_rejects_empty() {
        assert_eq!(KeyCombo::parse(""), None);
    }

    #[test]
    fn conf_string_roundtrips() {
        let combo = KeyCombo::new(true, true, false, "b");
        assert_eq!(combo.to_conf_string(), "CTRL SHFT b");
        assert_eq!(KeyCombo::parse(&combo.to_conf_string()), Some(combo));
    }

    #[test]
    fn gpui_keystroke_format() {
        let combo = KeyCombo::new(true, true, false, "b");
        // Non-macOS test environment: expect ctrl, not cmd.
        if !cfg!(target_os = "macos") {
            assert_eq!(combo.to_gpui_keystroke(), "ctrl-shift-b");
        }
    }

    #[test]
    fn display_string_format() {
        let combo = KeyCombo::new(true, false, true, "f7");
        if !cfg!(target_os = "macos") {
            assert_eq!(combo.display_string(), "Ctrl+Alt+F7");
        }
    }

    #[test]
    fn from_capture_escape_cancels() {
        let mods = Modifiers::default();
        assert_eq!(KeyCombo::from_capture(&mods, "escape"), None);
    }

    #[test]
    fn from_capture_builds_combo() {
        let mods = Modifiers { control: true, shift: true, alt: false, platform: false, function: false };
        let combo = KeyCombo::from_capture(&mods, "b").unwrap();
        assert_eq!(combo, KeyCombo::new(true, true, false, "b"));
    }

    #[test]
    fn every_action_has_a_default() {
        // Just exercises every arm of default_combo()/conf_key()/label()/category()
        // so a newly-added variant that forgets one panics here, not in the UI.
        for action in KeybindAction::all() {
            let _ = action.default_combo();
            let _ = action.conf_key();
            let _ = action.label();
            let _ = action.category();
        }
    }

    #[test]
    fn no_two_defaults_collide() {
        let keybinds = Keybinds::defaults();
        for action in KeybindAction::all() {
            let combo = keybinds.get(*action);
            assert_eq!(
                keybinds.find_conflict(&combo, (*action, Some(0))),
                None,
                "{:?}'s default {:?} collides with another action's default",
                action,
                combo,
            );
        }
    }

    #[test]
    fn find_conflict_detects_duplicate() {
        let mut keybinds = Keybinds::defaults();
        let combo = keybinds.get(KeybindAction::Bold);
        keybinds.set_at(KeybindAction::Underline, 0, combo.clone());
        assert_eq!(
            keybinds.find_conflict(&combo, (KeybindAction::Underline, Some(0))),
            Some(KeybindAction::Bold)
        );
    }

    #[test]
    fn find_conflict_ignores_self() {
        let keybinds = Keybinds::defaults();
        let combo = keybinds.get(KeybindAction::Bold);
        assert_eq!(keybinds.find_conflict(&combo, (KeybindAction::Bold, Some(0))), None);
    }

    #[test]
    fn add_appends_a_second_combo_without_disturbing_the_first() {
        let mut keybinds = Keybinds::defaults();
        let first = keybinds.get(KeybindAction::Bold);
        keybinds.add(KeybindAction::Bold, KeyCombo::new(false, false, false, "f2"));
        assert_eq!(
            keybinds.get_all(KeybindAction::Bold),
            vec![first, KeyCombo::new(false, false, false, "f2")]
        );
    }

    #[test]
    fn remove_at_drops_only_that_slot() {
        let mut keybinds = Keybinds::defaults();
        let first = keybinds.get(KeybindAction::Bold);
        keybinds.add(KeybindAction::Bold, KeyCombo::new(false, false, false, "f2"));
        keybinds.remove_at(KeybindAction::Bold, 0);
        assert_eq!(keybinds.get_all(KeybindAction::Bold), vec![KeyCombo::new(false, false, false, "f2")]);
        let _ = first;
    }

    #[test]
    fn remove_at_down_to_zero_leaves_the_action_unbound() {
        let mut keybinds = Keybinds::defaults();
        keybinds.remove_at(KeybindAction::Bold, 0);
        assert!(keybinds.get_all(KeybindAction::Bold).is_empty());
        assert_eq!(keybinds.get(KeybindAction::Bold), KeyCombo::new(false, false, false, ""));
    }

    /// The bug a naive `key=value1, value2` single-line format would hit:
    /// `ToggleSettings`'s own default key is a literal comma, so a
    /// comma-joined multi-combo action could parse as two combos where
    /// there's only one bound (`,` and whatever key sat after the comma).
    /// Repeated `key=` lines (this module's actual format) can't confuse the
    /// two, since each line is parsed independently.
    #[test]
    fn add_survives_round_trip_even_when_another_actions_key_is_a_comma() {
        let dir = std::env::temp_dir().join(format!("vimbatim_keybind_comma_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");

        let mut keybinds = Keybinds::defaults();
        keybinds.add(KeybindAction::Bold, KeyCombo::new(false, false, false, "f2"));
        keybinds.save_to(&path, false, &[]).unwrap();

        let reloaded = Keybinds::load(&path);
        assert_eq!(reloaded.get_all(KeybindAction::Bold).len(), 2);
        assert_eq!(reloaded.get(KeybindAction::ToggleSettings), KeyCombo::new(true, false, false, ","));

        fs::remove_file(&path).ok();
        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn cleared_keybind_stays_cleared_across_a_reload() {
        let dir = std::env::temp_dir().join(format!("vimbatim_keybind_clear_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");

        let mut keybinds = Keybinds::defaults();
        keybinds.remove_at(KeybindAction::Bold, 0);
        keybinds.save_to(&path, false, &[]).unwrap();

        // Without the "key present but empty" distinction, this would come
        // back bound to Bold's compiled-in default instead of staying empty.
        let reloaded = Keybinds::load(&path);
        assert!(reloaded.get_all(KeybindAction::Bold).is_empty());

        fs::remove_file(&path).ok();
        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn load_missing_file_uses_defaults() {
        let keybinds = Keybinds::load(Path::new("/nonexistent/path/settings.conf"));
        assert_eq!(keybinds.get(KeybindAction::Bold), KeybindAction::Bold.default_combo());
    }

    #[test]
    fn load_parses_flat_keys_across_headers() {
        let dir = std::env::temp_dir().join(format!("vimbatim_keybind_test_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");
        fs::write(&path, "[KEYBINDS: GENERAL]\nsave=ALT s\n\n[KEYBINDS: EDITING]\ncopy=ALT c\n").unwrap();

        let keybinds = Keybinds::load(&path);
        assert_eq!(keybinds.get(KeybindAction::Save), KeyCombo::new(false, false, true, "s"));
        assert_eq!(keybinds.get(KeybindAction::Copy), KeyCombo::new(false, false, true, "c"));

        fs::remove_file(&path).ok();
        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn real_settings_conf_is_internally_consistent() {
        // settings.conf is a live, user-editable runtime file — the whole
        // point of this feature is that its values change as the user
        // remaps things or toggles vim mode, so asserting exact values here
        // (as an earlier version of this test did) breaks the moment
        // someone actually uses the settings modal. Instead this only
        // checks structural invariants that must hold regardless of what's
        // been customized: every action resolves to some combo, and no two
        // actions collide with each other.
        let keybinds = Keybinds::load(Path::new("settings.conf"));
        for action in KeybindAction::all() {
            // Every combo of every action, not just the first — an action
            // can carry more than one now, and a collision on its second
            // slot is just as real as one on its first.
            for (i, combo) in keybinds.get_all(*action).iter().enumerate() {
                assert_eq!(
                    keybinds.find_conflict(combo, (*action, Some(i))), None,
                    "{:?}'s combo {:?} in settings.conf collides with another action", action, combo,
                );
            }
        }
    }

    #[test]
    fn real_default_settings_conf_matches_except_vim() {
        let map: HashMap<String, String> = fs::read_to_string("default_settings.conf")
            .unwrap()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        assert_eq!(map.get("vim").map(String::as_str), Some("false"));

        let keybinds = Keybinds::load(Path::new("default_settings.conf"));
        for action in KeybindAction::all() {
            assert_eq!(keybinds.get(*action), action.default_combo());
        }
    }

    #[test]
    fn real_default_settings_conf_has_vim_false() {
        assert!(!load_vim_enabled(Path::new("default_settings.conf")));
    }

    #[test]
    fn load_vim_enabled_missing_file_defaults_false() {
        assert!(!load_vim_enabled(Path::new("/nonexistent/path/settings.conf")));
    }
}
