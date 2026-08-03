/*
 * The vim-mode keybind layer (checklist: Settings -> Vim Mode) — lets a user
 * bind a `KeybindAction` (the same action list `keybinds.rs`'s Ctrl+key
 * system uses) to a raw vim-Normal-mode keystroke sequence, e.g. "zs" for
 * Save. Kept in its own file rather than folded into `keybinds.rs`: that
 * file's own doc comment states vim's modal command language is
 * deliberately NOT part of the Ctrl+key system, and this feature — while it
 * reuses `KeybindAction` — is exactly that command language.
 *
 * A sequence is stored as the literal characters typed (case *is*
 * significant: "S" means shift+s, not "s"). Only the first character of a
 * sequence is ever checked against vim's own reserved keyspace
 * (`state::is_vim_reserved_normal_key`) — every key after the first is
 * consumed by the caller's own sequence buffer before it could ever reach
 * vim's native dispatcher, so it's free to be anything.
 */

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::keybinds::KeybindAction;
use crate::state::is_vim_reserved_normal_key;

/// Result of matching a candidate sequence (built up one keystroke at a
/// time) against the bound table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimLookup {
    /// The sequence exactly matches a bound action.
    Exact(KeybindAction),
    /// Not a complete match yet, but a valid prefix of at least one bound
    /// sequence — keep buffering.
    Prefix,
    /// Not a prefix of anything bound.
    None,
}

/// A sequence -> action map, the inverse of how `Keybinds` is keyed
/// (action -> combos) — sequence uniqueness/prefix-safety is what needs
/// checking on every capture, so sequence is the natural map key here.
#[derive(Clone, Debug, Default)]
pub struct VimKeybinds {
    bindings: HashMap<String, KeybindAction>,
}

impl VimKeybinds {
    /// The z-leader default table: every default lives under `z` (confirmed
    /// unclaimed by vim's own Normal-mode dispatch — see
    /// `is_vim_reserved_normal_key`'s doc comment). `Undo`/`Redo` are
    /// deliberately absent — real vim's `u`/`Ctrl+r` already cover them
    /// natively (`state.rs`'s `handle_vim_normal_key`,
    /// `text_editor.rs`'s `process_key_ctrl_combo`), and giving them a
    /// second binding here would be exactly the duplication this feature
    /// was asked to avoid. Also absent, for lack of a clean mnemonic or
    /// because they're already single-keypress-reachable: `ToggleSettings`,
    /// `SelectSimilarFormatting`, `Shrink`, `PasteSmart`, the seven Card
    /// Styles (already on F-key defaults), and `CiteFromLink` (still a
    /// `println!` stub, no point defaulting a binding to a no-op). All of
    /// these remain fully capturable manually through the Settings UI.
    pub fn defaults() -> VimKeybinds {
        use KeybindAction::*;
        let table: &[(KeybindAction, &str)] = &[
            (NewTab, "zn"),
            (CloseTab, "zq"),
            (ReopenClosedTab, "zo"),
            (Save, "zs"),
            (SaveAs, "zS"),
            (Find, "zf"),
            (FindReplace, "zr"),
            (ToggleSidebar, "zb"),
            (ZoomIn, "z="),
            (ZoomOut, "z-"),
            (ZoomReset, "z0"),
            (NextTab, "zl"),
            (PrevTab, "zh"),
            (Copy, "zy"),
            (Cut, "zd"),
            (Paste, "zp"),
            (PasteWithoutFormatting, "zP"),
            (SelectAll, "za"),
            (Bold, "zB"),
            (Underline, "zu"),
            (ClearFormatting, "zc"),
            (Highlight, "zH"),
            (DeleteTags, "zt"),
            (StartTimer, "zT"),
            (OpenStats, "zi"),
            (Wikifi, "zw"),
        ];
        let bindings = table.iter().map(|(action, seq)| (seq.to_string(), *action)).collect();
        VimKeybinds { bindings }
    }

    /// Every sequence bound to `action`, alphabetically — a stable order,
    /// not map-iteration order, so the settings UI and `save_to`'s written
    /// file don't jitter between runs with the same bindings.
    pub fn get_all(&self, action: KeybindAction) -> Vec<String> {
        let mut seqs: Vec<String> = self
            .bindings
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(seq, _)| seq.clone())
            .collect();
        seqs.sort();
        seqs
    }

    pub fn add(&mut self, action: KeybindAction, sequence: String) {
        self.bindings.insert(sequence, action);
    }

    pub fn remove(&mut self, sequence: &str) {
        self.bindings.remove(sequence);
    }

    /// The runtime entry point: does `sequence` (accumulated so far) match,
    /// or could still match, something bound?
    pub fn lookup(&self, sequence: &str) -> VimLookup {
        if let Some(action) = self.bindings.get(sequence) {
            return VimLookup::Exact(*action);
        }
        if self.bindings.keys().any(|bound| bound.starts_with(sequence)) {
            return VimLookup::Prefix;
        }
        VimLookup::None
    }

    /// Capture-time hard-block rule 2 (rule 1 is `is_reserved_first_key`
    /// below): does `candidate` collide with an already-bound sequence,
    /// either as an exact duplicate or as a prefix in either direction?
    /// Either overlap would make at least one of the two sequences
    /// permanently unreachable (whichever is the prefix always completes
    /// first), so both directions are rejected, not just exact matches.
    /// `exclude` is the sequence currently being re-captured, if any — the
    /// same "don't conflict with yourself" exemption
    /// `Keybinds::find_conflict` gives its own `(action, slot)` exclusion.
    pub fn find_overlap_conflict(&self, candidate: &str, exclude: Option<&str>) -> Option<(KeybindAction, String)> {
        for (existing, action) in &self.bindings {
            if Some(existing.as_str()) == exclude {
                continue;
            }
            if existing == candidate || existing.starts_with(candidate) || candidate.starts_with(existing.as_str()) {
                return Some((*action, existing.clone()));
            }
        }
        None
    }

    /// Capture-time hard-block rule 1: is `candidate`'s *first* character
    /// already meaningful to vim's own Normal-mode dispatch? Only the first
    /// character matters — everything after it is consumed by this
    /// system's own sequence buffer before it could ever reach the native
    /// dispatcher (see the module doc comment).
    pub fn is_reserved_first_key(candidate: &str) -> bool {
        let Some(c) = candidate.chars().next() else { return true };
        let (key, shift) = char_to_key_and_shift(c);
        is_vim_reserved_normal_key(&key, shift, None)
    }

    /// Loads `[VIM_KEYBINDS]` from `path`, falling back to `defaults()` for
    /// any action whose namespaced key (`vim_<conf_key()>`) is absent —
    /// same "missing means keep the default, present-but-empty means
    /// deliberately cleared" contract `Keybinds::load` uses, and for the
    /// same reason: an older settings.conf that predates this feature
    /// should still get sensible bindings, not silently end up unbound.
    pub fn load(path: &Path) -> VimKeybinds {
        let mut vim_keybinds = VimKeybinds::defaults();
        let Ok(content) = fs::read_to_string(path) else { return vim_keybinds };

        let mut values: HashMap<&str, Vec<String>> = HashMap::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                values.entry(key.trim()).or_default().push(value.trim().to_string());
            }
        }

        for action in KeybindAction::all() {
            let conf_key = format!("vim_{}", action.conf_key());
            let Some(raws) = values.get(conf_key.as_str()) else { continue };
            vim_keybinds.bindings.retain(|_, a| a != action);
            for raw in raws {
                if !raw.is_empty() {
                    vim_keybinds.bindings.insert(raw.clone(), *action);
                }
            }
        }
        vim_keybinds
    }

    /// Rewrites only `[VIM_KEYBINDS]`, leaving everything else (including
    /// `keybinds.rs`'s own `[KEYBINDS...]` sections) byte-for-byte
    /// untouched — the same split-section discipline `Keybinds::save_to`
    /// uses for its own part of the file. Namespaced `vim_<conf_key()>`
    /// keys, not bare `conf_key()`: `Keybinds::load` is itself header-blind
    /// (it flat-scans every line in the whole file for its own action
    /// keys, regardless of which `[...]` section it's under) — a bare
    /// `save=zs` line here would be silently picked up by `Keybinds::load`
    /// too and replace whatever Ctrl+key combo Save actually has.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let existing = fs::read_to_string(path).unwrap_or_default();
        let preserved = extract_non_vim_keybind_sections(&existing);

        let mut out = preserved;
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str("[VIM_KEYBINDS]\n");
        for action in KeybindAction::all() {
            let seqs = self.get_all(*action);
            if seqs.is_empty() {
                // Written even when empty, same reason `Keybinds::save_to`
                // always writes a line per action: presence of the key at
                // all is what lets `load` tell "deliberately cleared" apart
                // from "predates this feature, keep the default."
                out.push_str(&format!("vim_{}=\n", action.conf_key()));
            } else {
                for seq in seqs {
                    out.push_str(&format!("vim_{}={}\n", action.conf_key(), seq));
                }
            }
        }
        fs::write(path, out)
    }
}

/// Splits a stored sequence character into what `is_vim_reserved_normal_key`
/// expects: an uppercase letter is shift+lowercase; anything else (a
/// lowercase letter, digit, or symbol) is itself with no shift, which is
/// the only representation this app's default table ever needs (no default
/// uses a shifted symbol).
fn char_to_key_and_shift(c: char) -> (String, bool) {
    if c.is_ascii_uppercase() {
        (c.to_ascii_lowercase().to_string(), true)
    } else {
        (c.to_string(), false)
    }
}

/// The `[VIM_KEYBINDS]` counterpart of `keybinds.rs`'s own
/// `extract_non_keybind_sections` — strips only that one section (so this
/// module's own `save_to` doesn't duplicate it on every save), preserving
/// every other section (including `Keybinds`' `[KEYBINDS...]` ones)
/// verbatim.
fn extract_non_vim_keybind_sections(content: &str) -> String {
    let mut out = String::new();
    let mut in_vim_keybinds_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_vim_keybinds_section = trimmed.eq_ignore_ascii_case("[VIM_KEYBINDS]");
            if in_vim_keybinds_section {
                continue;
            }
        }
        if in_vim_keybinds_section {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_has_no_duplicate_sequences() {
        // The literal source of truth for "does the default table collide
        // with itself" — if two actions defaulted to the same sequence,
        // the later one in the table would silently win, and `add`ing them
        // to a `HashMap` would already have lost one during `defaults()`
        // itself. Comparing action count to binding count catches that.
        let defaults = VimKeybinds::defaults();
        let action_count = KeybindAction::all()
            .iter()
            .filter(|a| !defaults.get_all(**a).is_empty())
            .count();
        assert_eq!(defaults.bindings.len(), action_count, "a default sequence was silently overwritten by another");
    }

    #[test]
    fn defaults_never_collide_or_prefix_each_other() {
        let defaults = VimKeybinds::defaults();
        let all: Vec<&String> = defaults.bindings.keys().collect();
        for a in &all {
            for b in &all {
                if a == b {
                    continue;
                }
                assert!(!a.starts_with(b.as_str()), "{a} is prefixed by {b}");
            }
        }
    }

    #[test]
    fn defaults_first_keys_are_all_unreserved() {
        for seq in VimKeybinds::defaults().bindings.keys() {
            assert!(!VimKeybinds::is_reserved_first_key(seq), "default {seq} starts with a reserved key");
        }
    }

    #[test]
    fn defaults_exclude_undo_and_redo() {
        let defaults = VimKeybinds::defaults();
        assert!(defaults.get_all(KeybindAction::Undo).is_empty());
        assert!(defaults.get_all(KeybindAction::Redo).is_empty());
    }

    #[test]
    fn lookup_finds_exact_and_prefix_matches() {
        let mut vk = VimKeybinds::default();
        vk.add(KeybindAction::Save, "zs".to_string());
        assert_eq!(vk.lookup("z"), VimLookup::Prefix);
        assert_eq!(vk.lookup("zs"), VimLookup::Exact(KeybindAction::Save));
        assert_eq!(vk.lookup("zx"), VimLookup::None);
    }

    #[test]
    fn overlap_conflict_catches_exact_duplicate() {
        let mut vk = VimKeybinds::default();
        vk.add(KeybindAction::Save, "zs".to_string());
        assert_eq!(vk.find_overlap_conflict("zs", None), Some((KeybindAction::Save, "zs".to_string())));
    }

    #[test]
    fn overlap_conflict_catches_either_prefix_direction() {
        let mut vk = VimKeybinds::default();
        vk.add(KeybindAction::Save, "zs".to_string());
        // A shorter candidate that would swallow the existing binding.
        assert!(vk.find_overlap_conflict("z", None).is_some());
        // A longer candidate the existing binding would swallow instead.
        assert!(vk.find_overlap_conflict("zsx", None).is_some());
    }

    #[test]
    fn overlap_conflict_ignores_the_excluded_sequence() {
        let mut vk = VimKeybinds::default();
        vk.add(KeybindAction::Save, "zs".to_string());
        assert_eq!(vk.find_overlap_conflict("zs", Some("zs")), None);
    }

    #[test]
    fn reserved_first_key_matches_state_rs_registry() {
        assert!(VimKeybinds::is_reserved_first_key("d"));
        assert!(VimKeybinds::is_reserved_first_key("dx"));
        assert!(!VimKeybinds::is_reserved_first_key("z"));
        assert!(!VimKeybinds::is_reserved_first_key("zs"));
        // Uppercase in the first slot means shift+key, per
        // `char_to_key_and_shift` — "M" is reserved (H/M/L visual jump)
        // even though lowercase "m" (marks, unimplemented) is free.
        assert!(VimKeybinds::is_reserved_first_key("M"));
        assert!(!VimKeybinds::is_reserved_first_key("m"));
    }

    #[test]
    fn round_trips_through_settings_conf_without_corrupting_keybinds() {
        let dir = std::env::temp_dir().join(format!("vimbatim_vim_keybinds_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("settings.conf");

        // Seed a real [KEYBINDS] section the way `Keybinds::save_to` would,
        // so this test can prove `VimKeybinds::save_to` doesn't corrupt it.
        fs::write(&path, "[KEYBINDS]\nsave=CTRL s\n\n").unwrap();

        let mut vk = VimKeybinds::default();
        vk.add(KeybindAction::Save, "zs".to_string());
        vk.save_to(&path).unwrap();

        let reloaded = VimKeybinds::load(&path);
        assert_eq!(reloaded.get_all(KeybindAction::Save), vec!["zs".to_string()]);

        // The namespacing is what's actually under test: a bare `save=`
        // line here would have been picked up by `Keybinds::load`'s own
        // flat, header-blind scan and silently replaced Ctrl+S.
        let keybinds = crate::keybinds::Keybinds::load(&path);
        assert_eq!(
            keybinds.get(KeybindAction::Save).to_conf_string(),
            "CTRL s",
            "VimKeybinds::save_to must not corrupt Keybinds::load's own [KEYBINDS] section"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_preserves_an_explicitly_emptied_action() {
        let dir = std::env::temp_dir().join(format!("vimbatim_vim_keybinds_empty_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("settings.conf");

        let mut vk = VimKeybinds::defaults();
        // Deliberately clear a default — presence of the (empty) key on
        // disk must mean "stays cleared", not "predates this feature".
        for seq in vk.get_all(KeybindAction::Save) {
            vk.remove(&seq);
        }
        vk.save_to(&path).unwrap();

        let reloaded = VimKeybinds::load(&path);
        assert!(reloaded.get_all(KeybindAction::Save).is_empty());

        let _ = fs::remove_dir_all(&dir);
    }
}
