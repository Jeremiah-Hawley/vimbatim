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

use gpui::{actions, App, KeyBinding, Modifiers};
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

    /// Human-readable label for the settings UI, platform-aware.
    pub fn display_string(&self) -> String {
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
    Save,
    SaveAs,
    Find,
    FindReplace,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
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
    Emphasis,
    Highlight,
    DeleteTags,
    StartTimer,
    OpenStats,
    CiteFromLink,
    Wikifi,
}

impl KeybindAction {
    pub fn all() -> &'static [KeybindAction] {
        use KeybindAction::*;
        &[
            ToggleSettings, ToggleSidebar, NewTab, CloseTab, Save, SaveAs, Find, FindReplace,
            Copy, Cut, Paste, Undo, Redo, SelectAll,
            Bold, Underline, Shrink, ClearFormatting,
            PasteSmart, Condense, Pocket, Hat, Block, Tag, Cite, Emphasis,
            Highlight,
            DeleteTags, StartTimer, OpenStats, CiteFromLink, Wikifi,
        ]
    }

    pub fn label(&self) -> &'static str {
        use KeybindAction::*;
        match self {
            ToggleSettings => "Toggle Settings",
            ToggleSidebar => "Toggle Sidebar",
            NewTab => "New Document",
            CloseTab => "Close Tab",
            Save => "Save",
            SaveAs => "Save As",
            Find => "Find",
            FindReplace => "Find & Replace",
            Copy => "Copy",
            Cut => "Cut",
            Paste => "Paste",
            Undo => "Undo",
            Redo => "Redo",
            SelectAll => "Select All",
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
            Emphasis => "Emphasis",
            Highlight => "Highlight",
            DeleteTags => "Delete Tags",
            StartTimer => "Start Timer",
            OpenStats => "Open Stats",
            CiteFromLink => "Cite From Link",
            Wikifi => "Wikifi",
        }
    }

    pub fn category(&self) -> KeybindCategory {
        use KeybindAction::*;
        use KeybindCategory as C;
        match self {
            ToggleSettings | ToggleSidebar | NewTab | CloseTab | Save | SaveAs | Find | FindReplace => C::General,
            Copy | Cut | Paste | Undo | Redo | SelectAll => C::Editing,
            Bold | Underline | Shrink | ClearFormatting => C::TextFormatting,
            PasteSmart | Condense | Pocket | Hat | Block | Tag | Cite | Emphasis => C::CardStyles,
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
            Save => "save",
            SaveAs => "save_as",
            Find => "find",
            FindReplace => "find_and_replace",
            Copy => "copy",
            Cut => "cut",
            Paste => "paste_raw",
            Undo => "undo",
            Redo => "redo",
            SelectAll => "select_all",
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
            Emphasis => "emphasis",
            Highlight => "highlight",
            DeleteTags => "delete_tags",
            StartTimer => "start_timer",
            OpenStats => "open_stats",
            CiteFromLink => "cite_from_link",
            Wikifi => "wikifi",
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
            Save => KeyCombo::new(true, false, false, "s"),
            SaveAs => KeyCombo::new(true, true, false, "s"),
            Find => KeyCombo::new(true, false, false, "f"),
            FindReplace => KeyCombo::new(true, false, false, "h"),
            Copy => KeyCombo::new(true, false, false, "c"),
            Cut => KeyCombo::new(true, false, false, "x"),
            Paste => KeyCombo::new(true, false, false, "v"),
            Undo => KeyCombo::new(true, false, false, "z"),
            Redo => KeyCombo::new(true, false, false, "y"),
            SelectAll => KeyCombo::new(true, false, false, "a"),
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
            Emphasis => KeyCombo::new(false, false, false, "f10"),
            Highlight => KeyCombo::new(false, false, false, "f11"),
            DeleteTags => KeyCombo::new(false, false, true, "f7"),
            StartTimer => KeyCombo::new(true, true, false, "t"),
            OpenStats => KeyCombo::new(true, true, false, "i"),
            CiteFromLink => KeyCombo::new(true, false, false, "f8"),
            Wikifi => KeyCombo::new(true, true, true, "w"),
        }
    }

    /// True if this action has no real implementation yet (matches this
    /// codebase's existing convention for unimplemented ribbon items like
    /// Doc Menu / Card Menu — still fully bindable and shown in the UI, the
    /// handler just logs instead of doing nothing silently).
    pub fn is_stub(&self) -> bool {
        matches!(
            self,
            KeybindAction::SaveAs | KeybindAction::Find | KeybindAction::FindReplace
                | KeybindAction::DeleteTags | KeybindAction::StartTimer
                | KeybindAction::OpenStats | KeybindAction::CiteFromLink
        )
    }
}

/// The registry mapping each `KeybindAction` to its currently-assigned
/// `KeyCombo`, loaded from and saved back to settings.conf.
#[derive(Clone, Debug)]
pub struct Keybinds {
    combos: HashMap<KeybindAction, KeyCombo>,
}

impl Keybinds {
    /// Every action defaults to its `default_combo()`, then whatever
    /// settings.conf actually specifies overrides that — so a missing or
    /// unparseable entry never leaves an action unbound.
    pub fn defaults() -> Keybinds {
        let combos = KeybindAction::all().iter().map(|a| (*a, a.default_combo())).collect();
        Keybinds { combos }
    }

    /// Parses settings.conf's flat `key=value` lines (mirroring
    /// `config_parsing.rs`'s own approach: every line starting with `[` is
    /// skipped, so any number of `[KEYBINDS: ...]` sub-headers are safe)
    /// looking for each action's `conf_key()`.
    pub fn load(path: &Path) -> Keybinds {
        let mut keybinds = Keybinds::defaults();
        let Ok(content) = fs::read_to_string(path) else { return keybinds };

        let mut values: HashMap<&str, String> = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') { continue; }
            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim(), value.trim().to_string());
            }
        }

        for action in KeybindAction::all() {
            if let Some(raw) = values.get(action.conf_key()) {
                if let Some(combo) = KeyCombo::parse(raw) {
                    keybinds.combos.insert(*action, combo);
                }
            }
        }
        keybinds
    }

    pub fn get(&self, action: KeybindAction) -> KeyCombo {
        self.combos.get(&action).cloned().unwrap_or_else(|| action.default_combo())
    }

    pub fn set(&mut self, action: KeybindAction, combo: KeyCombo) {
        self.combos.insert(action, combo);
    }

    /// Returns whichever *other* action already owns `combo`, if any —
    /// used to block duplicate assignments and tell the user what's
    /// currently using the combination they just pressed.
    pub fn find_conflict(&self, combo: &KeyCombo, exclude: KeybindAction) -> Option<KeybindAction> {
        KeybindAction::all()
            .iter()
            .find(|a| **a != exclude && self.get(**a) == *combo)
            .copied()
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
                out.push_str(&format!("{}={}\n", action.conf_key(), self.get(*action).to_conf_string()));
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
// is the one place all 32 are declared. `main_window.rs` registers a small
// `.on_action` handler per struct; `rebuild_keymap` below is the only place
// that needs to know which struct corresponds to which `KeybindAction`.
actions!(
    keybinds,
    [
        ToggleSettingsAction, ToggleSidebarAction, NewTabAction, CloseTabAction, SaveAction,
        SaveAsAction, FindAction, FindReplaceAction,
        CopyAction, CutAction, PasteAction, UndoAction, RedoAction, SelectAllAction,
        BoldAction, UnderlineAction, ShrinkAction, ClearFormattingAction,
        PasteSmartAction, CondenseAction, PocketAction, HatAction, BlockAction, TagAction,
        CiteAction, EmphasisAction,
        HighlightAction,
        DeleteTagsAction, StartTimerAction, OpenStatsAction, CiteFromLinkAction, WikifiAction,
    ]
);

/// Rebuilds the entire GPUI keymap from `keybinds`. Safe to call at startup
/// or any time after a binding changes — `App::clear_key_bindings` +
/// `App::bind_keys` both work at runtime, not just before the window opens.
pub fn rebuild_keymap(cx: &mut App, keybinds: &Keybinds) {
    use KeybindAction::*;

    cx.clear_key_bindings();

    let ks = |action: KeybindAction| keybinds.get(action).to_gpui_keystroke();

    cx.bind_keys([
        KeyBinding::new(&ks(ToggleSettings), ToggleSettingsAction, None),
        KeyBinding::new(&ks(ToggleSidebar), ToggleSidebarAction, None),
        KeyBinding::new(&ks(NewTab), NewTabAction, None),
        KeyBinding::new(&ks(CloseTab), CloseTabAction, None),
        KeyBinding::new(&ks(Save), SaveAction, None),
        KeyBinding::new(&ks(SaveAs), SaveAsAction, None),
        KeyBinding::new(&ks(Find), FindAction, None),
        KeyBinding::new(&ks(FindReplace), FindReplaceAction, None),
        KeyBinding::new(&ks(Copy), CopyAction, None),
        KeyBinding::new(&ks(Cut), CutAction, None),
        KeyBinding::new(&ks(Paste), PasteAction, None),
        KeyBinding::new(&ks(Undo), UndoAction, None),
        KeyBinding::new(&ks(Redo), RedoAction, None),
        KeyBinding::new(&ks(SelectAll), SelectAllAction, None),
        KeyBinding::new(&ks(Bold), BoldAction, None),
        KeyBinding::new(&ks(Underline), UnderlineAction, None),
        KeyBinding::new(&ks(Shrink), ShrinkAction, None),
        KeyBinding::new(&ks(ClearFormatting), ClearFormattingAction, None),
        KeyBinding::new(&ks(PasteSmart), PasteSmartAction, None),
        KeyBinding::new(&ks(Condense), CondenseAction, None),
        KeyBinding::new(&ks(Pocket), PocketAction, None),
        KeyBinding::new(&ks(Hat), HatAction, None),
        KeyBinding::new(&ks(Block), BlockAction, None),
        KeyBinding::new(&ks(Tag), TagAction, None),
        KeyBinding::new(&ks(Cite), CiteAction, None),
        KeyBinding::new(&ks(Emphasis), EmphasisAction, None),
        KeyBinding::new(&ks(Highlight), HighlightAction, None),
        KeyBinding::new(&ks(DeleteTags), DeleteTagsAction, None),
        KeyBinding::new(&ks(StartTimer), StartTimerAction, None),
        KeyBinding::new(&ks(OpenStats), OpenStatsAction, None),
        KeyBinding::new(&ks(CiteFromLink), CiteFromLinkAction, None),
        KeyBinding::new(&ks(Wikifi), WikifiAction, None),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

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
                keybinds.find_conflict(&combo, *action),
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
        keybinds.set(KeybindAction::Underline, combo.clone());
        assert_eq!(
            keybinds.find_conflict(&combo, KeybindAction::Underline),
            Some(KeybindAction::Bold)
        );
    }

    #[test]
    fn find_conflict_ignores_self() {
        let keybinds = Keybinds::defaults();
        let combo = keybinds.get(KeybindAction::Bold);
        assert_eq!(keybinds.find_conflict(&combo, KeybindAction::Bold), None);
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
    fn real_settings_conf_matches_defaults() {
        // Confirms settings.conf's actual on-disk values (post-reorganization)
        // agree with every action's default_combo() — they're meant to be
        // identical right now; this test breaks loudly the moment they drift.
        let keybinds = Keybinds::load(Path::new("settings.conf"));
        for action in KeybindAction::all() {
            assert_eq!(
                keybinds.get(*action), action.default_combo(),
                "{:?} in settings.conf doesn't match its default_combo()", action,
            );
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
    fn real_settings_conf_has_vim_true() {
        assert!(load_vim_enabled(Path::new("settings.conf")));
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
