use gpui::prelude::*;
use gpui::*;

use crate::keybinds::{rebuild_keymap, KeyCombo, KeybindAction, KeybindCategory, Keybinds};
use crate::state::{bundled_default_settings_path, settings_conf_path, AppState};
use crate::theme::{palette, save_theme, save_theme_color_mode, save_theme_mode, ThemeColorMode, ThemeKind, ThemeMode};

/// Where this modal *writes* every setting it changes.
///
/// Must stay `settings_conf_path()` — the exact path `AppState::new()` uses to
/// *read* settings at startup. This was previously a bare relative
/// `"settings.conf"`, resolved against the process's current working
/// directory, while startup read from next to the executable: the modal wrote
/// one file and startup read another, so every change made here — vim toggle,
/// theme, Reset to Defaults — silently reverted on the next launch.
fn settings_path() -> std::path::PathBuf {
    settings_conf_path()
}

/// The pristine copy `Reset to Defaults` restores from — the read-only one
/// shipped with the build, not a sibling of the user's settings.conf (which
/// now lives in the user data directory and has no defaults file beside it).
fn default_settings_path() -> std::path::PathBuf {
    bundled_default_settings_path()
}

/// Underline colors offered for spellcheck, as `(settings.conf value, swatch
/// hex)`. The names are Word highlight-color names, which is what
/// `text_editor::highlight_color_hex` resolves at paint time — so what's
/// written here stays hand-editable in settings.conf rather than becoming an
/// opaque hex blob.
///
/// ponytail: a fixed set, not the ribbon's full HSL picker. Reusing that would
/// mean generifying `color_picker::render_picker`, which is hardcoded to
/// `Context<FormattingRibbon>` and whose drag listeners reach back into that
/// view's own `picker` field — real work, for a setting nobody changes twice.
/// `highlight_color_hex` already accepts a raw 6-digit hex, so anyone who
/// wants an exact shade can still type it into settings.conf directly.
const SPELLCHECK_COLORS: [(&str, u32); 5] = [
    ("red", 0xFF0000),
    ("darkRed", 0x8B0000),
    ("blue", 0x0000FF),
    ("green", 0x00FF00),
    ("magenta", 0xFF00FF),
];

/// Which pane the settings sidebar is showing.
///
/// The sidebar switches panes rather than scrolling one long page to an
/// anchor: GPUI has no scroll-to-element primitive, so a true jump would mean
/// measuring every section's laid-out offset and driving the scroll handle by
/// hand. Switching panes is what VS Code and macOS System Settings do anyway,
/// and it keeps each pane short enough to not need scrolling at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsSection {
    Appearance,
    TextSettings,
    Keybindings,
    ToggleFeatures,
}

impl SettingsSection {
    /// Sidebar order, top to bottom. `ToggleFeatures` is deliberately last —
    /// the toggles belong at the bottom of the settings list.
    fn all() -> [SettingsSection; 4] {
        [
            SettingsSection::Appearance,
            SettingsSection::TextSettings,
            SettingsSection::Keybindings,
            SettingsSection::ToggleFeatures,
        ]
    }

    fn label(&self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::TextSettings => "Text Settings",
            SettingsSection::Keybindings => "Keybindings",
            SettingsSection::ToggleFeatures => "Toggle Features",
        }
    }
}

/// `{version} ({git_sha})`, e.g. `0.1.0-beta.1 (a1b2c3d)` — both baked in at
/// compile time (`Cargo.toml`'s version, and `build.rs`'s
/// `VIMBATIM_GIT_SHA`). Shown in the settings modal so a beta tester can
/// read off exactly what build a bug report came from
/// (`closed_beta_plan.md` §3).
fn build_version_string() -> String {
    format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("VIMBATIM_GIT_SHA"))
}

/// The floating settings modal. Renders as a centred overlay on top of the
/// main window whenever `AppState.settings_visible` is true.
///
/// Lets the user toggle vim mode and remap every configurable, non-vim
/// keybinding (`src/keybinds.rs`) by pressing a new key combination.
/// Changes take effect immediately (the GPUI keymap is rebuilt on the spot)
/// and are persisted to settings.conf right away — there's no separate
/// "Save" step for keybind changes.
pub struct SettingsModal {
    state: Entity<AppState>,
    /// Needed so this view can claim keyboard focus while capturing a key
    /// combination — see `start_capture`.
    focus_handle: FocusHandle,
    /// The action (and slot) currently awaiting a keypress, if any. The slot
    /// is `Some(index)` when re-capturing an existing combo (clicking its
    /// chip), or `None` when adding a brand new one (the "+" button) —
    /// `handle_capture_key` routes to `Keybinds::set_at` or `Keybinds::add`
    /// accordingly.
    capturing: Option<(KeybindAction, Option<usize>)>,
    /// Set when a captured combo collides with another action's existing
    /// binding — shown inline on the capturing row. Capture stays active
    /// (rather than closing) so the user can just try a different key.
    conflict_message: Option<String>,
    /// Per-category collapse state for the keybind list, mirroring
    /// `formatting_ribbon.rs`'s own collapsible-group pattern.
    collapsed: std::collections::HashMap<KeybindCategory, bool>,
    /// The Vim Keybinds sub-list's own collapse state — kept separate from
    /// `collapsed` above so collapsing "General" in one list doesn't also
    /// collapse it in the other.
    vim_collapsed: std::collections::HashMap<KeybindCategory, bool>,
    /// The vim-keybind counterpart of `capturing` — checklist: Settings ->
    /// Vim Mode. The second element is the *existing sequence being
    /// replaced*, if any (re-capturing a chip), rather than an index: a
    /// `VimKeybinds` binding is keyed by its sequence string, not by
    /// position, so there's no natural slot index the way `Keybinds`' own
    /// `Vec<KeyCombo>` has one.
    vim_capturing: Option<(KeybindAction, Option<String>)>,
    /// What's been typed so far this capture — a vim sequence can be
    /// several keystrokes (unlike a Ctrl+key combo, which resolves in one),
    /// so this accumulates until Enter commits or Escape cancels.
    vim_capture_buffer: String,
    /// The vim-keybind counterpart of `conflict_message`.
    vim_conflict_message: Option<String>,
    /// Lightweight mode for cycling themes against the real app chrome
    /// without the dimmed backdrop or the full keybind settings list.
    theme_preview: bool,
    /// Set when Import Theme's picked file fails to parse as a valid
    /// custom-theme TOML — shown inline under the Import Theme button,
    /// mirroring `conflict_message`'s pattern. Cleared on the next attempt.
    theme_import_error: Option<String>,
    /// True while the Search From List word box is accepting typing — the
    /// third mode `handle_capture_key` routes between (see its doc comment).
    ///
    /// The buffer is the box's live text, one word per line, only written back
    /// to `AppState` (and disk) on each keystroke via `set_search_word_list`.
    editing_word_list: bool,
    word_list_buffer: String,
    /// Which sidebar pane is showing. See `SettingsSection`.
    section: SettingsSection,
}

impl SettingsModal {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        /*
         * Constructs the SettingsModal. Visibility is controlled externally via
         * `AppState.settings_visible`; the modal itself is always fully constructed
         * and only conditionally rendered by MainWindow.
         */
        SettingsModal {
            state,
            focus_handle: cx.focus_handle(),
            capturing: None,
            conflict_message: None,
            collapsed: std::collections::HashMap::new(),
            vim_collapsed: std::collections::HashMap::new(),
            vim_capturing: None,
            vim_capture_buffer: String::new(),
            vim_conflict_message: None,
            theme_preview: false,
            theme_import_error: None,
            editing_word_list: false,
            word_list_buffer: String::new(),
            section: SettingsSection::Appearance,
        }
    }

    /// Enters the Search From List word box, seeding the buffer from the saved
    /// list. Cancels both keybind captures — all three modes share one
    /// `on_key_down`, so only one may be live at a time.
    fn start_word_list_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_capture(cx);
        self.cancel_vim_capture();
        self.word_list_buffer = self.state.read(cx).search_word_list_text();
        self.editing_word_list = true;
        self.focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    fn cancel_word_list_edit(&mut self) {
        self.editing_word_list = false;
        self.word_list_buffer.clear();
    }

    /// Applies one keystroke to the word box. Enter inserts a real newline —
    /// this is a multi-line list, and separating words is the box's whole
    /// purpose — so it is not a commit key here; Escape leaves the box, and
    /// Tab leaves it rather than inserting an indent nothing would read.
    ///
    /// Every edit writes straight through to `AppState` (and the word-list
    /// file), so there is no unsaved-buffer state to lose and an open Search
    /// From List panel's readout follows the typing live.
    fn handle_word_list_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        match ks.key.as_str() {
            "escape" | "tab" => {
                self.cancel_word_list_edit();
                cx.notify();
                return;
            }
            "enter" => self.word_list_buffer.push('\n'),
            "backspace" => {
                self.word_list_buffer.pop();
            }
            key => {
                let Some(c) = crate::state::vim_find_target_char(key, ks.modifiers.shift, ks.key_char.as_deref())
                else {
                    return;
                };
                self.word_list_buffer.push(c);
            }
        }
        let text = self.word_list_buffer.clone();
        self.state.update(cx, |s, cx| {
            s.set_search_word_list(&text);
            cx.notify();
        });
        cx.notify();
    }

    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Hides the modal by setting `AppState.settings_visible` to false.
         * Both the backdrop click and the explicit Close / × buttons call this.
         * Also cancels any in-progress key capture so closing the modal
         * never leaves the keymap cleared.
         */
        self.cancel_capture(cx);
        self.cancel_vim_capture();
        self.cancel_word_list_edit();
        self.theme_preview = false;
        self.state.update(cx, |s, cx| {
            s.settings_visible = false;
            cx.notify();
        });
        cx.notify();
    }

    /// Arms capture mode for `action`: the next keystroke (after this call)
    /// is interpreted as the candidate new binding by `handle_capture_key`.
    ///
    /// Clears every registered keybinding for the duration of the capture
    /// (`cancel_capture`/successful capture restores them via
    /// `rebuild_keymap`), so an already-bound combo still reaches
    /// `handle_capture_key` below instead of firing whatever it's currently
    /// bound to. Two other approaches were tried and don't work: (1)
    /// stop-propagation inside `App::intercept_keystrokes` — GPUI's raw-key
    /// dispatch checks the same propagate-event flag an interceptor sets,
    /// so suppressing an action that way also suppresses the raw event this
    /// view depends on; (2) a `KeyContext` predicate requiring this panel's
    /// tag to be *absent* — GPUI's context-predicate evaluator treats a
    /// dispatch path with no context tags on it as an automatic non-match
    /// for every predicate (including negations), and not every focus state
    /// in this app's tree guarantees a tagged ancestor is on the path.
    /// Clearing the keymap outright sidesteps both problems: with nothing
    /// registered, there's nothing for any keystroke to match, regardless
    /// of focus or context.
    fn start_capture(&mut self, action: KeybindAction, slot: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_word_list_edit();
        self.capturing = Some((action, slot));
        self.conflict_message = None;
        cx.clear_key_bindings();
        self.focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    fn cancel_capture(&mut self, cx: &mut Context<Self>) {
        self.capturing = None;
        self.conflict_message = None;
        let keybinds = self.state.read(cx).keybinds.clone();
        rebuild_keymap(cx, &keybinds);
    }

    /// Resolves a captured keystroke into a candidate `KeyCombo`, applying
    /// it (and persisting + rebuilding the live keymap) if it doesn't
    /// The panel's single `on_key_down` entry point — routes to whichever of
    /// its **three** typing modes is active: the Search From List word box,
    /// vim-sequence capture, or Ctrl-combo capture.
    ///
    /// All three are mutually exclusive by construction — starting any one
    /// cancels the other two, and the section-switch / close / reset-to-
    /// defaults call sites cancel all three — so this ordered fall-through is
    /// unambiguous. **Any future mode must be cancelled in those same places**;
    /// that invariant is the only thing keeping this router honest.
    fn handle_capture_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_word_list {
            self.handle_word_list_key(event, window, cx);
            return;
        }
        if self.vim_capturing.is_some() {
            self.handle_vim_capture_key(event, window, cx);
            return;
        }
        self.handle_ctrl_capture_key(event, window, cx);
    }

    /// Resolves a captured keystroke into a candidate `KeyCombo`, applying
    /// it (and persisting + rebuilding the live keymap) if it doesn't
    /// collide with another action, or showing an inline conflict message
    /// and staying in capture mode if it does.
    fn handle_ctrl_capture_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((action, slot)) = self.capturing else { return };
        let ks = &event.keystroke;

        let Some(combo) = KeyCombo::from_capture(&ks.modifiers, &ks.key) else {
            // Escape: cancel capture, keeping the existing binding.
            self.cancel_capture(cx);
            cx.notify();
            return;
        };

        let conflict = self.state.read(cx).keybinds.find_conflict(&combo, (action, slot));
        if let Some(other) = conflict {
            self.conflict_message = Some(format!(
                "{} is already used by \"{}\". Press a different combination, or Esc to keep the current binding.",
                combo.display_string(),
                other.label(),
            ));
            cx.notify();
            return;
        }

        self.state.update(cx, |s, _cx| {
            match slot {
                Some(index) => s.keybinds.set_at(action, index, combo.clone()),
                None => s.keybinds.add(action, combo.clone()),
            }
            let _ = s.keybinds.save_to(&settings_path(), s.vim_enabled, &[]);
        });
        self.cancel_capture(cx); // restores the keymap, now including the new binding
        cx.notify();
    }

    /// Arms vim-keybind capture for `action`. Unlike `start_capture`, this
    /// does *not* call `cx.clear_key_bindings()` — a raw, unmodified
    /// letter keystroke never matches any GPUI `KeyBinding` (those are all
    /// registered as Ctrl/Cmd combos or F-keys via `to_gpui_keystroke()`),
    /// so there's nothing here for a plain "s" or "z" to collide with.
    /// `existing` is the sequence being replaced, if any — `None` means
    /// adding a fresh one via the "+" button, same distinction
    /// `start_capture`'s `slot` makes for the Ctrl+key system.
    fn start_vim_capture(&mut self, action: KeybindAction, existing: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_word_list_edit();
        self.vim_capturing = Some((action, existing));
        self.vim_capture_buffer.clear();
        self.vim_conflict_message = None;
        self.focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    fn cancel_vim_capture(&mut self) {
        self.vim_capturing = None;
        self.vim_capture_buffer.clear();
        self.vim_conflict_message = None;
    }

    /// Accumulates keystrokes into `vim_capture_buffer` until Enter commits
    /// it (after both capture-time hard-block checks — see
    /// `VimKeybinds::is_reserved_first_key`/`find_overlap_conflict`) or
    /// Escape cancels. Unlike `handle_capture_key`'s single-keystroke
    /// combo, a vim sequence is typed over several keystrokes, so this
    /// can't resolve on the first one.
    fn handle_vim_capture_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((action, existing)) = self.vim_capturing.clone() else { return };
        let ks = &event.keystroke;

        match ks.key.as_str() {
            "escape" => {
                self.cancel_vim_capture();
                cx.notify();
                return;
            }
            "backspace" => {
                self.vim_capture_buffer.pop();
                self.vim_conflict_message = None;
                cx.notify();
                return;
            }
            "enter" => {
                if self.vim_capture_buffer.is_empty() {
                    return;
                }
                let candidate = self.vim_capture_buffer.clone();
                let exclude = existing.as_deref();

                if crate::vim_keybinds::VimKeybinds::is_reserved_first_key(&candidate) {
                    self.vim_conflict_message = Some(format!(
                        "{candidate:?} starts with a key vim's own Normal mode already uses. Try a different sequence, or Esc to keep the current binding."
                    ));
                    self.vim_capture_buffer.clear();
                    cx.notify();
                    return;
                }
                let conflict = self.state.read(cx).vim_keybinds.find_overlap_conflict(&candidate, exclude);
                if let Some((other, other_seq)) = conflict {
                    self.vim_conflict_message = Some(format!(
                        "{candidate:?} overlaps with {other_seq:?}, already used by \"{}\". Try a different sequence, or Esc to keep the current binding.",
                        other.label(),
                    ));
                    self.vim_capture_buffer.clear();
                    cx.notify();
                    return;
                }
                if let Some(native) = crate::vim_keybinds::VimKeybinds::find_native_vim_conflict(&candidate) {
                    self.vim_conflict_message = Some(format!(
                        "{candidate:?} overlaps with {native:?}, which is vim's own scroll command. Try a different sequence, or Esc to keep the current binding."
                    ));
                    self.vim_capture_buffer.clear();
                    cx.notify();
                    return;
                }

                self.state.update(cx, |s, _cx| {
                    if let Some(old) = &existing {
                        s.vim_keybinds.remove(old);
                    }
                    s.vim_keybinds.add(action, candidate.clone());
                    let _ = s.vim_keybinds.save_to(&settings_path());
                });
                self.cancel_vim_capture();
                cx.notify();
            }
            _ => {
                // `vim_find_target_char` handles the same shift/key_char
                // normalization the real vim dispatcher uses, so what's
                // typed here matches exactly what `VimKeybinds` will later
                // be asked to look up at runtime.
                if let Some(c) = crate::state::vim_find_target_char(&ks.key, ks.modifiers.shift, ks.key_char.as_deref()) {
                    self.vim_capture_buffer.push(c);
                    self.vim_conflict_message = None;
                    cx.notify();
                }
            }
        }
    }

    // Each of these delegates to the `AppState` method that flips *and*
    // persists. The write used to live here, which made this modal the only
    // thing that could save a toggle — so the command palette, which can run
    // every one of them, would have flipped the flag and silently lost it on
    // restart.
    fn toggle_vim(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.toggle_vim();
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_spellcheck(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.toggle_spellcheck();
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_search_from_list(&mut self, cx: &mut Context<Self>) {
        self.cancel_word_list_edit();
        self.state.update(cx, |s, cx| {
            s.toggle_search_from_list();
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_search_list_whole_words(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.toggle_search_list_whole_words();
            cx.notify();
        });
        cx.notify();
    }

    /// The Search From List word box: a click-to-focus multi-line text area,
    /// built the same way every other text input in this app is (no GPUI text
    /// input exists — see `find_bar.rs`'s own note), with the panel's shared
    /// `focus_handle` and `handle_word_list_key` doing the typing.
    fn render_word_list_box(&self, p: crate::theme::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let words = self.state.read(cx).search_word_list.clone();
        let editing = self.editing_word_list;
        // While editing, paint the live buffer (which can hold a trailing
        // blank line the saved list deliberately drops); otherwise the saved
        // list, so the box still shows its contents after focus moves away.
        let lines: Vec<String> = if editing {
            self.word_list_buffer.split('\n').map(ToString::to_string).collect()
        } else {
            words.clone()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child(if editing {
                        "One word per line. Enter starts a new line; Esc when you're done."
                    } else {
                        "One word per line. Click to edit."
                    }),
            )
            .child(
                div()
                    .id("search-word-list-box")
                    .w_full()
                    .min_h(px(96.0))
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .p(px(8.0))
                    .rounded(px(4.0))
                    .bg(rgb(p.editor_bg))
                    .border_1()
                    .border_color(rgb(if editing { p.accent } else { p.border_subtle }))
                    .cursor_pointer()
                    .text_sm()
                    .text_color(rgb(p.text))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, window, cx| {
                        this.start_word_list_edit(window, cx);
                    }))
                    .when(lines.iter().all(|l| l.is_empty()) && !editing, |d| {
                        d.child(
                            div()
                                .text_color(rgb(p.text_faint))
                                .child("No words yet — click here and type one per line."),
                        )
                    })
                    .children(lines.into_iter().enumerate().map(|(i, line)| {
                        // A block caret on the last line while editing, so an
                        // empty box still shows where typing lands — the same
                        // stand-in the find bar's fields use.
                        div().flex().flex_row().items_center().child(line).when(
                            editing && i + 1 == self.word_list_buffer.split('\n').count(),
                            |d| d.child(div().w(px(1.0)).h(px(14.0)).ml(px(1.0)).bg(rgb(p.text))),
                        )
                    })),
            )
    }

    fn toggle_nav_fold_buttons(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.toggle_nav_fold_buttons();
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_command_palette(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.toggle_command_palette_enabled();
            cx.notify();
        });
        cx.notify();
    }

    fn set_spellcheck_color(&mut self, name: &'static str, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.spellcheck_underline_color = name.to_string();
            let _ = crate::theme::save_setting_line(
                &settings_path(),
                "spellcheck_underline_color",
                name,
            );
            cx.notify();
        });
        cx.notify();
    }

    /// Nudges the spreading rate by `delta` wpm, clamped and persisted.
    ///
    /// A stepper rather than a text field: GPUI has no text input, and a
    /// hand-rolled numeric one would be more code than the setting is worth
    /// (typing an exact value is still possible — settings.conf is plain text).
    /// Nudges the shrink size by `delta` points, clamped and persisted. Same
    /// stepper reasoning as `adjust_spreading_wpm` — no text input exists, and
    /// settings.conf stays hand-editable for an exact value.
    fn adjust_shrink_size(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let current = (s.small_size_half_points / 2) as i32;
            s.set_shrink_size_points((current + delta).max(0) as u16);
            cx.notify();
        });
        cx.notify();
    }

    fn adjust_emphasis_size(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let current = (s.emphasis_size_half_points / 2) as i32;
            s.set_emphasis_size_points((current + delta).max(0) as u16);
            cx.notify();
        });
        cx.notify();
    }

    fn adjust_spreading_wpm(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let next = crate::state::clamp_spreading_wpm(
                (s.spreading_wpm as i32 + delta).max(0) as u32,
            );
            s.spreading_wpm = next;
            let _ = crate::theme::save_setting_line(
                &settings_path(),
                "spreading_wpm",
                &next.to_string(),
            );
            cx.notify();
        });
        cx.notify();
    }

    /// Settings -> Appearance -> Download Theme Template: native save dialog
    /// (same `prompt_for_new_path` gpui uses for the app's own Save As, see
    /// `main_window.rs`), writes `theme::custom_theme_template()` verbatim —
    /// a blank starting point the user edits and re-imports.
    fn download_theme_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.state.read(cx).working_directory.clone();
        let path_rx = cx.prompt_for_new_path(&dir, Some("theme_template.toml"));
        cx.spawn_in(window, async move |_this, cx| {
            let Ok(Ok(Some(path))) = path_rx.await else { return };
            let _ = cx.background_spawn(async move {
                std::fs::write(path, crate::theme::custom_theme_template())
            }).await;
        })
        .detach();
    }

    /// Settings -> Appearance -> Import Theme: native open dialog, then
    /// `AppState::import_custom_theme` parses+adopts it. Invalid TOML shows
    /// `theme_import_error` inline rather than silently doing nothing.
    fn import_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths_rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        let state = self.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths_rx.await else { return };
            let Some(path) = paths.pop() else { return };
            let Ok(content) = std::fs::read_to_string(&path) else {
                let _ = this.update(cx, |this, cx| {
                    this.theme_import_error = Some("Couldn't read that file.".to_string());
                    cx.notify();
                });
                return;
            };
            let ok = state.update(cx, |s, cx| {
                let ok = s.import_custom_theme(&content);
                if ok { cx.notify(); }
                ok
            });
            let _ = this.update(cx, |this, cx| {
                this.theme_import_error = if ok {
                    None
                } else {
                    Some("Not a valid theme file — missing a [dark]/[light] section or a color.".to_string())
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn set_theme(&mut self, theme: ThemeKind, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.theme = theme;
            let _ = save_theme(&settings_path(), theme);
            cx.notify();
        });
        cx.notify();
    }

    fn set_theme_color_mode(&mut self, mode: ThemeColorMode, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.theme_color_mode = mode;
            let _ = save_theme_color_mode(&settings_path(), mode);
            cx.notify();
        });
        cx.notify();
    }

    /// One selectable pill in the Theme Color / Mode rows. Both groups render
    /// identically and differ only in what their click writes, so the styling
    /// lives here once.
    fn mode_pill(
        id: ElementId,
        label: &'static str,
        is_current: bool,
        p: crate::theme::Palette,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .cursor_pointer()
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .text_xs()
            .border_1()
            .when(is_current, |d| {
                d.bg(rgb(p.accent_wash))
                    .border_color(rgb(p.accent_muted))
                    .text_color(rgb(p.text))
            })
            .when(!is_current, |d| {
                d.bg(rgb(p.chrome_active))
                    .border_color(rgb(p.border_subtle))
                    .text_color(rgb(p.text_muted))
            })
            .hover(move |s| s.bg(rgb(p.chrome_hover)))
            .active(move |s| s.bg(rgb(p.chrome_active)))
            .on_click(on_click)
            .child(label)
    }

    fn set_theme_mode(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.theme_mode = mode;
            let _ = save_theme_mode(&settings_path(), mode);
            cx.notify();
        });
        cx.notify();
    }

    fn enter_theme_preview(&mut self, cx: &mut Context<Self>) {
        self.theme_preview = true;
        self.cancel_capture(cx);
        self.cancel_vim_capture();
        self.cancel_word_list_edit();
        cx.notify();
    }

    fn exit_theme_preview(&mut self, cx: &mut Context<Self>) {
        self.theme_preview = false;
        cx.notify();
    }

    /// Copies default_settings.conf over settings.conf, reloads both the
    /// keybind registry and the vim flag from the now-reset file, rebuilds
    /// the live keymap, and cancels any in-progress capture.
    fn reset_to_defaults(&mut self, cx: &mut Context<Self>) {
        if std::fs::copy(default_settings_path(), settings_path()).is_err() {
            return;
        }
        let path = settings_path();
        let path = path.as_path();
        let keybinds = Keybinds::load(path);
        let vim_keybinds = crate::vim_keybinds::VimKeybinds::load(path);
        let vim_enabled = crate::keybinds::load_vim_enabled(path);
        let theme = crate::theme::load_theme(path);
        let theme_mode = crate::theme::load_theme_mode(path);
        let theme_color_mode = crate::theme::load_theme_color_mode(path);

        self.state.update(cx, |s, _cx| {
            s.keybinds = keybinds;
            s.vim_keybinds = vim_keybinds;
            s.vim_enabled = vim_enabled;
            s.theme = theme;
            s.theme_mode = theme_mode;
            s.theme_color_mode = theme_color_mode;
        });
        self.cancel_capture(cx); // also rebuilds the keymap from the now-reset keybinds
        self.cancel_vim_capture();
        self.cancel_word_list_edit();
        cx.notify();
    }

    /// One bound combo's own chip: the combo pill, a "Change" link that
    /// re-captures that exact slot, and a "×" that removes it outright (no
    /// capture needed for a removal). Replaced by the live capture prompt
    /// while this specific slot is the one being captured.
    fn render_combo_chip(
        &self,
        action: KeybindAction,
        index: usize,
        combo: &KeyCombo,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.capturing == Some((action, Some(index))) {
            return self.capture_prompt(p, theme_mode).into_any_element();
        }
        // `action as usize` alone collides across an action's own slots — a
        // wide stride (any single action realistically has a handful of
        // combos at most) keeps `(action, index)` pairs unique per element.
        let base_id = (action as usize) * 64 + index;
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(p.text))
                    .px(px(8.0))
                    .py(px(2.0))
                    .bg(rgb(p.chrome_active))
                    .rounded(px(4.0))
                    .child(combo.display_string()),
            )
            .child(
                div()
                    .id(ElementId::named_usize("keybind-change", base_id))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(p.accent))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, window, cx| {
                        this.start_capture(action, Some(index), window, cx);
                    }))
                    .child("Change"),
            )
            .child(
                div()
                    .id(ElementId::named_usize("keybind-remove", base_id))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(p.text_faint))
                    .hover(move |s| s.text_color(rgb(p.text)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                        this.state.update(cx, |s, _cx| {
                            s.keybinds.remove_at(action, index);
                            let _ = s.keybinds.save_to(&settings_path(), s.vim_enabled, &[]);
                        });
                        this.cancel_capture(cx); // rebuilds the keymap without the removed combo
                        cx.notify();
                    }))
                    .child("×"),
            )
            .into_any_element()
    }

    /// The live "press a key…" prompt (or an inline conflict message),
    /// shared by whichever slot — an existing chip being re-captured, or the
    /// "+" add slot — is the one currently active.
    fn capture_prompt(&self, p: crate::theme::Palette, theme_mode: ThemeMode) -> AnyElement {
        match &self.conflict_message {
            // A conflict warning keeps its own red identity rather than
            // becoming palette chrome, but the dark red is illegible on a
            // light background — same per-mode pairing the editor's
            // unsupported-document banner uses (`text_editor.rs`).
            Some(msg) => div()
                .text_xs()
                .text_color(rgb(match theme_mode {
                    ThemeMode::Dark => 0xf48771,
                    ThemeMode::Light => 0xb02a15,
                }))
                .max_w(px(220.0))
                .child(msg.clone())
                .into_any_element(),
            None => div()
                .text_xs()
                .text_color(rgb(p.accent))
                .child("Press a key… (Esc to cancel)")
                .into_any_element(),
        }
    }

    /// Renders one action's row: its label on the left, and on the right one
    /// chip per bound combo plus a small square "+" to add another — or,
    /// while a specific slot is being captured, the live prompt in its place.
    fn render_action_row(
        &self,
        action: KeybindAction,
        combos: Vec<KeyCombo>,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut slots: Vec<AnyElement> = combos
            .iter()
            .enumerate()
            .map(|(i, combo)| self.render_combo_chip(action, i, combo, p, theme_mode, cx))
            .collect();

        // The "add another" slot: the capture prompt while adding, else the
        // small square "+" button that starts it. `None` as the slot marks
        // this as an addition (not a re-capture of an existing index) to
        // `start_capture`/`handle_capture_key`.
        slots.push(if self.capturing == Some((action, None)) {
            self.capture_prompt(p, theme_mode)
        } else {
            div()
                .id(ElementId::named_usize("keybind-add", action as usize))
                .flex()
                .items_center()
                .justify_center()
                .w(px(18.0))
                .h(px(18.0))
                .rounded(px(4.0))
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(p.text_faint))
                .bg(rgb(p.chrome_active))
                .hover(move |s| s.text_color(rgb(p.text)).bg(rgb(p.chrome_hover)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, window, cx| {
                    this.start_capture(action, None, window, cx);
                }))
                .child("+")
                .into_any_element()
        });

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .py(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().text_sm().text_color(rgb(p.text)).child(action.label()))
                    .when(action.is_stub(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.text_faint))
                                .child("(not yet implemented)"),
                        )
                    })
                    .when(combos.is_empty() && self.capturing != Some((action, None)), |d| {
                        d.child(div().text_xs().text_color(rgb(p.text_faint)).child("Unbound"))
                    }),
            )
            .child(div().flex().flex_row().items_center().gap(px(8.0)).children(slots))
    }

    /// Renders one collapsible category section (its header + every action
    /// row belonging to it), mirroring `formatting_ribbon.rs`'s own
    /// collapse-arrow convention.
    fn render_category(
        &self,
        category: KeybindCategory,
        keybinds: &Keybinds,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_collapsed = *self.collapsed.get(&category).unwrap_or(&false);
        let actions = Self::listed_actions(category, self.state.read(cx).command_palette_enabled);

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(p.border_subtle))
            .child(
                div()
                    .id(ElementId::named_usize("keybind-category", category as u8 as usize))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .py(px(2.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.text))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                        let collapsed = this.collapsed.get(&category).copied().unwrap_or(false);
                        this.collapsed.insert(category, !collapsed);
                        cx.notify();
                    }))
                    .child(if is_collapsed { "▶" } else { "▼" })
                    .child(category.label()),
            )
            .when(!is_collapsed, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(16.0))
                        .children(actions.into_iter().map(|action| {
                            self.render_action_row(action, keybinds.get_all(action), p, theme_mode, cx)
                        })),
                )
            })
    }

    // ── Vim Keybinds (checklist: Settings -> Vim Mode) ────────────────────
    // Mirrors render_combo_chip/capture_prompt/render_action_row/
    // render_category above closely, but keyed by sequence string (a
    // `VimKeybinds` binding has no `Vec`-index slot the way a `KeyCombo`
    // does) and without any `cx.clear_key_bindings()` dance, since a raw
    // vim keystroke never collides with a registered GPUI `KeyBinding`.

    fn render_vim_combo_chip(
        &self,
        action: KeybindAction,
        sequence: &str,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.vim_capturing.as_ref().is_some_and(|(a, s)| *a == action && s.as_deref() == Some(sequence)) {
            return self.vim_capture_prompt(p, theme_mode).into_any_element();
        }
        let base_id = format!("{action:?}-{sequence}");
        let sequence_owned = sequence.to_string();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(p.text))
                    .px(px(8.0))
                    .py(px(2.0))
                    .bg(rgb(p.chrome_active))
                    .rounded(px(4.0))
                    .child(sequence.to_string()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("vim-keybind-change-{base_id}")))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(p.accent))
                    .on_mouse_down(MouseButton::Left, {
                        let sequence_owned = sequence_owned.clone();
                        cx.listener(move |this, _ev, window, cx| {
                            this.start_vim_capture(action, Some(sequence_owned.clone()), window, cx);
                        })
                    })
                    .child("Change"),
            )
            .child(
                div()
                    .id(SharedString::from(format!("vim-keybind-remove-{base_id}")))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(p.text_faint))
                    .hover(move |s| s.text_color(rgb(p.text)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                        this.state.update(cx, |s, _cx| {
                            s.vim_keybinds.remove(&sequence_owned);
                            let _ = s.vim_keybinds.save_to(&settings_path());
                        });
                        this.cancel_vim_capture();
                        cx.notify();
                    }))
                    .child("×"),
            )
            .into_any_element()
    }

    fn vim_capture_prompt(&self, p: crate::theme::Palette, theme_mode: ThemeMode) -> AnyElement {
        match &self.vim_conflict_message {
            Some(msg) => div()
                .text_xs()
                .text_color(rgb(match theme_mode {
                    ThemeMode::Dark => 0xf48771,
                    ThemeMode::Light => 0xb02a15,
                }))
                .max_w(px(260.0))
                .child(msg.clone())
                .into_any_element(),
            None => div()
                .text_xs()
                .text_color(rgb(p.accent))
                .max_w(px(180.0))
                .child(format!(
                    "Type a sequence, Enter to save ({}), Esc to cancel",
                    if self.vim_capture_buffer.is_empty() { "…".to_string() } else { self.vim_capture_buffer.clone() }
                ))
                .into_any_element(),
        }
    }

    fn render_vim_action_row(
        &self,
        action: KeybindAction,
        sequences: Vec<String>,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut slots: Vec<AnyElement> = sequences
            .iter()
            .map(|seq| self.render_vim_combo_chip(action, seq, p, theme_mode, cx))
            .collect();

        let is_adding = self.vim_capturing.as_ref().is_some_and(|(a, s)| *a == action && s.is_none());
        slots.push(if is_adding {
            self.vim_capture_prompt(p, theme_mode)
        } else {
            div()
                .id(ElementId::named_usize("vim-keybind-add", action as usize))
                .flex()
                .items_center()
                .justify_center()
                .w(px(18.0))
                .h(px(18.0))
                .rounded(px(4.0))
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(p.text_faint))
                .bg(rgb(p.chrome_active))
                .hover(move |s| s.text_color(rgb(p.text)).bg(rgb(p.chrome_hover)))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, window, cx| {
                    this.start_vim_capture(action, None, window, cx);
                }))
                .child("+")
                .into_any_element()
        });

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .py(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().text_sm().text_color(rgb(p.text)).child(action.label()))
                    .when(sequences.is_empty() && !is_adding, |d| {
                        d.child(div().text_xs().text_color(rgb(p.text_faint)).child("Unbound"))
                    }),
            )
            .child(div().flex().flex_row().items_center().gap(px(8.0)).children(slots))
    }

    fn render_vim_category(
        &self,
        category: KeybindCategory,
        vim_keybinds: &crate::vim_keybinds::VimKeybinds,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_collapsed = *self.vim_collapsed.get(&category).unwrap_or(&false);
        let actions = Self::listed_actions(category, self.state.read(cx).command_palette_enabled);

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(p.border_subtle))
            .child(
                div()
                    .id(ElementId::named_usize("vim-keybind-category", category as u8 as usize))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .py(px(2.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.text))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                        let collapsed = this.vim_collapsed.get(&category).copied().unwrap_or(false);
                        this.vim_collapsed.insert(category, !collapsed);
                        cx.notify();
                    }))
                    .child(if is_collapsed { "▶" } else { "▼" })
                    .child(category.label()),
            )
            .when(!is_collapsed, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .px(px(16.0))
                        .children(actions.into_iter().map(|action| {
                            self.render_vim_action_row(action, vim_keybinds.get_all(action), p, theme_mode, cx)
                        })),
                )
            })
    }

    /// The Vim Keybinds sub-list — appended to the Keybindings pane, gated
    /// on `vim_enabled` at the call site, rather than a whole separate
    /// `SettingsSection`: that would need `SettingsSection::all()` to
    /// become conditional, plus handling "what's the active section when
    /// vim gets toggled off while it's showing" — real state-machine
    /// surface a plain `when(vim_enabled, ...)` block doesn't need at all.
    fn render_vim_keybinds_section(
        &self,
        vim_keybinds: &crate::vim_keybinds::VimKeybinds,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .pt(px(16.0))
            .mt(px(8.0))
            .border_t_1()
            .border_color(rgb(p.border_subtle))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .pb(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(p.text))
                            .child("Vim Keybinds"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .max_w(px(500.0))
                            .child(
                                "Bind an app action to a vim-Normal-mode keystroke sequence. \
                                 Only fires while Vim Mode is on and the active tab is in Normal \
                                 mode. A sequence can't start with a key vim's own commands \
                                 already use (h, d, g, f, and so on) — every default here lives \
                                 under \"z\", which vim leaves free.",
                            ),
                    ),
            )
            .children(KeybindCategory::all().iter().map(|category| {
                self.render_vim_category(*category, vim_keybinds, p, theme_mode, cx)
            }))
    }
}

impl SettingsModal {
    /// The left-hand pane switcher. One row per `SettingsSection`, the active
    /// one washed with the accent color.
    fn render_sidebar(&self, p: crate::theme::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .w(px(180.0))
            .flex_none()
            .p(px(10.0))
            .border_r_1()
            .border_color(rgb(p.border_subtle))
            .bg(rgb(p.sidebar))
            .children(SettingsSection::all().into_iter().map(|section| {
                let is_current = section == self.section;
                div()
                    .id(ElementId::named_usize("settings-section", section as usize))
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_sm()
                    .when(is_current, |d| {
                        d.bg(rgb(p.accent_wash))
                            .text_color(rgb(p.text))
                            .font_weight(FontWeight::BOLD)
                    })
                    .when(!is_current, |d| {
                        d.text_color(rgb(p.text_muted))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                    })
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                        this.section = section;
                        this.cancel_capture(cx);
                        this.cancel_vim_capture();
                        this.cancel_word_list_edit();
                        cx.notify();
                    }))
                    .child(section.label())
            }))
    }

    /// The actions a keybind list shows for `category`.
///
/// Shared by both the Ctrl-combo list and the Vim Keybinds list so the two
/// can't drift on what's visible. `CommandPalette` is hidden while its Toggle
/// Features switch is off — the same "only show it once the feature is on"
/// rule the Vim Keybinds section itself follows — so a disabled feature
/// doesn't leave a bindable-looking dead row behind.
fn listed_actions(category: KeybindCategory, command_palette_enabled: bool) -> Vec<KeybindAction> {
    KeybindAction::all()
        .iter()
        .copied()
        .filter(|a| a.category() == category)
        .filter(|a| *a != KeybindAction::CommandPalette || command_palette_enabled)
        .collect()
}

/// A labelled on/off row with a description — the shared shape of every
    /// entry in the Toggle Features pane.
    fn toggle_row(
        id: &'static str,
        label: &'static str,
        description: &'static str,
        enabled: bool,
        p: crate::theme::Palette,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(p.text))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .max_w(px(400.0))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .id(id)
                    .flex_none()
                    .cursor_pointer()
                    .px(px(12.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .border_1()
                    .when(enabled, |d| {
                        d.bg(rgb(p.accent))
                            .border_color(rgb(p.accent_strong))
                            // The accent is a saturated mid-tone in both
                            // modes, so a fixed light label stays legible on
                            // it — unlike `p.text`, which inverts with the
                            // mode and would vanish on the light palette.
                            .text_color(rgb(0xffffff))
                    })
                    .when(!enabled, |d| {
                        d.bg(rgb(p.chrome_active))
                            .border_color(rgb(p.border))
                            .text_color(rgb(p.text_muted))
                    })
                    .on_mouse_down(MouseButton::Left, on_click)
                    .child(if enabled { "On" } else { "Off" }),
            )
    }

    /// The Toggle Features pane: every on/off feature in one group, per the
    /// bottom-of-settings grouping this sidebar's last entry names.
    fn render_toggle_features(
        &self,
        vim_enabled: bool,
        spellcheck_enabled: bool,
        spellcheck_color: String,
        spreading_wpm: u32,
        nav_fold_buttons: bool,
        search_from_list_enabled: bool,
        search_list_whole_words: bool,
        command_palette_enabled: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                Self::toggle_row(
                    "vim-mode-toggle",
                    "Vim Mode",
                    "Enables modal editing (Normal/Insert/Visual modes and motions), similar to the Vim text editor.",
                    vim_enabled,
                    p,
                    cx.listener(|this, _ev, _window, cx| this.toggle_vim(cx)),
                ),
            )
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(Self::toggle_row(
                        "spellcheck-toggle",
                        "Spellcheck",
                        "Underlines misspelled words. Right-click one for suggestions, or to add it to your dictionary.",
                        spellcheck_enabled,
                        p,
                        cx.listener(|this, _ev, _window, cx| this.toggle_spellcheck(cx)),
                    ))
                    // Underline color — only meaningful while spellcheck is on.
                    .when(spellcheck_enabled, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.text_muted))
                                        .child("Underline color"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(6.0))
                                        .children(SPELLCHECK_COLORS.iter().map(|&(name, hex)| {
                                            let selected = spellcheck_color == name;
                                            div()
                                                .id(SharedString::from(format!("spellcheck-color-{name}")))
                                                .w(px(20.0))
                                                .h(px(20.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(hex))
                                                .cursor_pointer()
                                                .border_2()
                                                // The selected swatch gets a ring
                                                // in the page's own text color
                                                // (so it reads on both light and
                                                // dark); the rest get a border
                                                // matching their own fill, which
                                                // keeps every swatch the same
                                                // size either way.
                                                .border_color(rgb(if selected { p.text } else { hex }))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                                                    this.set_spellcheck_color(name, cx);
                                                }))
                                        })),
                                ),
                        )
                    }),
            )
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            // ── Search From List ──────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(Self::toggle_row(
                        "search-from-list-toggle",
                        "Search From List",
                        "Adds a \"Search From List\" button beside Find that steps through every occurrence of any word in your list, using the same Next/Previous controls.",
                        search_from_list_enabled,
                        p,
                        cx.listener(|this, _ev, _window, cx| this.toggle_search_from_list(cx)),
                    ))
                    // The list and its matching option are only meaningful
                    // while the feature is on — same gating the spellcheck
                    // colour row above uses.
                    .when(search_from_list_enabled, |d| {
                        d.child(self.render_word_list_box(p, cx)).child(Self::toggle_row(
                            "search-list-whole-words-toggle",
                            "Whole words only",
                            "On: \"war\" matches \"war\" but not \"warming\". Off: matches anywhere inside a word, the way the Find button does.",
                            search_list_whole_words,
                            p,
                            cx.listener(|this, _ev, _window, cx| this.toggle_search_list_whole_words(cx)),
                        ))
                    }),
            )
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(Self::toggle_row(
                "command-palette-toggle",
                "Command Palette",
                "Adds a searchable list of every command in Vimbatim, opened with Ctrl+P (rebindable under Keybindings once this is on). Type to filter, Enter runs the top result.",
                command_palette_enabled,
                p,
                cx.listener(|this, _ev, _window, cx| this.toggle_command_palette(cx)),
            ))
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(Self::toggle_row(
                "nav-fold-buttons-toggle",
                "Navigation Menu Heading Fold Buttons",
                "Adds 1/2/3/4 buttons under the folder name in the Navigation sidebar that collapse the outline to that heading level. The same levels are always available by right-clicking the outline.",
                nav_fold_buttons,
                p,
                cx.listener(|this, _ev, _window, cx| this.toggle_nav_fold_buttons(cx)),
            ))
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            // ── Spreading rate ────────────────────────────────────────────
            // Not a toggle, but it belongs with the other document-behaviour
            // settings rather than under Appearance.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Spreading rate"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.text_muted))
                                    .max_w(px(400.0))
                                    .child("Words per minute used for the Word Count panel's speech-time estimate."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .flex_none()
                            .child(Self::stepper_btn("spreading-wpm-down", "−", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_spreading_wpm(-10, cx)),
                            ))
                            .child(
                                div()
                                    .w(px(56.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(p.text))
                                    .child(format!("{spreading_wpm}")),
                            )
                            .child(Self::stepper_btn("spreading-wpm-up", "+", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_spreading_wpm(10, cx)),
                            )),
                    ),
            )
    }

    /// One −/+ button of a numeric stepper.
    fn stepper_btn(id: &'static str, label: &'static str, p: crate::theme::Palette) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .text_sm()
            .text_color(rgb(p.text))
            .bg(rgb(p.chrome_active))
            .border_1()
            .border_color(rgb(p.border))
            .hover(move |s| s.bg(rgb(p.chrome_hover)))
            .child(label)
    }

    /// A labelled checkbox. Used for the Emphasis trio, which are independent
    /// rather than a pick-one — a squad's "emphasis" is whatever combination
    /// they standardised on.
    fn checkbox(
        id: &'static str,
        label: &'static str,
        checked: bool,
        p: crate::theme::Palette,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, on_click)
            .child(
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .border_1()
                    .text_size(px(11.0))
                    .when(checked, |d| {
                        d.bg(rgb(p.accent))
                            .border_color(rgb(p.accent_strong))
                            .text_color(rgb(0xffffff))
                            .child("✓")
                    })
                    .when(!checked, |d| {
                        d.bg(rgb(p.chrome_active)).border_color(rgb(p.border))
                    }),
            )
            .child(div().text_sm().text_color(rgb(p.text)).child(label))
    }

    /// The Text Settings pane: default highlight color, what Emphasis applies,
    /// and how the paste command treats newlines.
    fn render_text_settings(
        &self,
        emphasis: (bool, bool, bool),
        emphasis_change_size: bool,
        emphasis_size_points: u16,
        shrink_points: u16,
        exception: String,
        custom_highlights: Vec<u32>,
        analytic_color: String,
        paste_condense: bool,
        paste_condense_pilcrow: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (bold, underline, boxed) = emphasis;
        let heading = |text: &'static str| {
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(p.text))
                .child(text)
        };
        let note = |text: &'static str| {
            div().text_xs().text_color(rgb(p.text_muted)).max_w(px(420.0)).child(text)
        };

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            // ── Shrink size ───────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Shrink size"))
                    .child(note("The point size Shrink drops text to. Underlined text is left alone."))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(Self::stepper_btn("shrink-size-down", "−", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_shrink_size(-1, cx)),
                            ))
                            .child(
                                div()
                                    .w(px(48.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(p.text))
                                    .child(format!("{shrink_points} pt")),
                            )
                            .child(Self::stepper_btn("shrink-size-up", "+", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_shrink_size(1, cx)),
                            )),
                    ),
            )
            // ── Analytic color ────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Analytic color"))
                    .child(note("The text color the Analytic button applies. Same palette as the Font Color dropdown."))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.0))
                            .children(crate::formatting_ribbon::TEXT_COLORS.iter().map(|&(name, hex)| {
                                let selected = analytic_color == name;
                                div()
                                    .id(SharedString::from(format!("analytic-color-{name}")))
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(hex))
                                    .cursor_pointer()
                                    .border_2()
                                    .border_color(rgb(if selected { p.text } else { hex }))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                                        this.state.update(cx, |s, cx| {
                                            s.set_analytic_color(name);
                                            cx.notify();
                                        });
                                        cx.notify();
                                    }))
                            })),
                    ),
            )
            // ── Standardize exception ─────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Standardize highlight exception"))
                    .child(note(
                        "\"Standardize highlighting with exception\" leaves this color alone. \
                         Click the selected swatch again to clear it.",
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .w(px(220.0))
                            .gap(px(6.0))
                            .children(
                                crate::formatting_ribbon::HIGHLIGHT_COLORS
                                    .iter()
                                    .map(|&(name, _label, hex)| (name.to_string(), hex))
                                    .chain(
                                        custom_highlights
                                            .iter()
                                            .map(|&hex| (format!("{hex:06x}"), hex)),
                                    )
                                    .map(|(name, hex)| {
                                        let selected = exception == name;
                                        let pick = name.clone();
                                        div()
                                            .id(SharedString::from(format!("std-exception-{name}")))
                                            .w(px(22.0))
                                            .h(px(22.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(hex))
                                            .cursor_pointer()
                                            .border_2()
                                            .border_color(rgb(if selected { p.text } else { hex }))
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                                                // Re-clicking the current one
                                                // clears it — there is no
                                                // separate "none" swatch to
                                                // find.
                                                let next = if selected { String::new() } else { pick.clone() };
                                                this.state.update(cx, |s, cx| {
                                                    s.set_standardize_exception(&next);
                                                    cx.notify();
                                                });
                                                cx.notify();
                                            }))
                                    }),
                            ),
                    ),
            )
            // ── Emphasis ──────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Emphasis"))
                    .child(note("Which formatting the Emphasis command applies. Any combination."))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(16.0))
                            .child(Self::checkbox("emphasis-bold", "Bold", bold, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis(!bold, underline, boxed);
                                        cx.notify();
                                    });
                                })))
                            .child(Self::checkbox("emphasis-underline", "Underline", underline, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis(bold, !underline, boxed);
                                        cx.notify();
                                    });
                                })))
                            .child(Self::checkbox("emphasis-box", "Box", boxed, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis(bold, underline, !boxed);
                                        cx.notify();
                                    });
                                })))
                            .child(Self::checkbox("emphasis-change-size", "Change size", emphasis_change_size, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis_change_size(!emphasis_change_size);
                                        cx.notify();
                                    });
                                }))),
                    )
                    .when(emphasis_change_size, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(Self::stepper_btn("emphasis-size-down", "−", p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _window, cx| this.adjust_emphasis_size(-1, cx)),
                                ))
                                .child(
                                    div()
                                        .w(px(48.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_sm()
                                        .text_color(rgb(p.text))
                                        .child(format!("{emphasis_size_points} pt")),
                                )
                                .child(Self::stepper_btn("emphasis-size-up", "+", p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _window, cx| this.adjust_emphasis_size(1, cx)),
                                )),
                        )
                    }),
            )
            // ── Paste ─────────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(heading("Paste"))
                    .child(note("The ribbon's Para Integrity and Pilcrows buttons drive these same two settings."))
                    .child(Self::checkbox("paste-condense", "Condense by default", paste_condense, p,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.state.update(cx, |s, cx| {
                                s.set_paste_condense(!paste_condense);
                                cx.notify();
                            });
                        })))
                    // Only meaningful while condensing, so it appears only then
                    // rather than sitting inert.
                    .when(paste_condense, |d| {
                        d.child(
                            div().pl(px(22.0)).child(Self::checkbox(
                                "paste-condense-pilcrow",
                                "Mark collapsed newlines with ¶",
                                paste_condense_pilcrow,
                                p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_paste_condense_pilcrow(!paste_condense_pilcrow);
                                        cx.notify();
                                    });
                                }),
                            )),
                        )
                    }),
            )
    }

    /// The Appearance pane: theme picker, then the Theme Color / Mode pair.
    /// Also what Theme Preview shows on its own.
    fn render_appearance(
        &self,
        current_theme: ThemeKind,
        current_theme_mode: ThemeMode,
        current_theme_color_mode: ThemeColorMode,
        theme_preview: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            // ── Theme selector ────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Theme"),
                            )
                            .when(!theme_preview, |d| {
                                d.child(
                                    div()
                                        .id("theme-preview-toggle")
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .text_xs()
                                        .bg(rgb(p.chrome_active))
                                        .text_color(rgb(p.text))
                                        .border_1()
                                        .border_color(rgb(p.border))
                                        .hover(move |s| s.bg(rgb(p.chrome_hover)))
                                        .active(move |s| s.bg(rgb(p.chrome_active)))
                                        .on_click(cx.listener(|this, _ev, _window, cx| {
                                            this.enter_theme_preview(cx);
                                        }))
                                        .child("Preview"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(6.0))
                            .children(ThemeKind::all().iter().map(|theme| {
                                let theme = *theme;
                                let is_current = theme == current_theme;
                                let theme_palette = palette(theme, current_theme_mode);
                                div()
                                    .id(ElementId::named_usize("theme-choice", theme as usize))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .cursor_pointer()
                                    .pl(px(6.0))
                                    .pr(px(10.0))
                                    .py(px(4.0))
                                    .rounded(px(4.0))
                                    .text_xs()
                                    .border_1()
                                    .when(is_current, |d| {
                                        d.bg(rgb(p.accent_wash))
                                            .border_color(rgb(p.accent_muted))
                                            .text_color(rgb(p.text))
                                    })
                                    .when(!is_current, |d| {
                                        d.bg(rgb(p.chrome_active))
                                            .border_color(rgb(p.border_subtle))
                                            .text_color(rgb(p.text_muted))
                                    })
                                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                                    .active(move |s| s.bg(rgb(p.chrome_active)))
                                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                                        this.set_theme(theme, cx);
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .gap(px(2.0))
                                            .child(div().w(px(8.0)).h(px(8.0)).rounded(px(2.0)).bg(rgb(theme_palette.accent)))
                                            .child(div().w(px(8.0)).h(px(8.0)).rounded(px(2.0)).bg(rgb(theme_palette.accent_alt)))
                                            .child(div().w(px(8.0)).h(px(8.0)).rounded(px(2.0)).bg(rgb(theme_palette.highlight))),
                                    )
                                    .child(theme.label())
                            })),
                    ),
            )
            // ── Theme Color │ Mode ────────────────────────────────────────
            // Two labelled groups side by side, split by a thin vertical rule.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Theme Color"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .children(ThemeColorMode::all().iter().map(|mode| {
                                        let mode = *mode;
                                        Self::mode_pill(
                                            ElementId::named_usize("theme-color-mode", mode as usize),
                                            mode.label(),
                                            mode == current_theme_color_mode,
                                            p,
                                            cx.listener(move |this, _ev, _window, cx| {
                                                this.set_theme_color_mode(mode, cx);
                                            }),
                                        )
                                    })),
                            ),
                    )
                    // The separating rule. Height is fixed rather than
                    // stretched so it spans the label+buttons pair without
                    // pinning the row's height.
                    .child(div().w(px(1.0)).h(px(44.0)).bg(rgb(p.border_subtle)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Mode"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap(px(6.0))
                                    .children(ThemeMode::all().iter().map(|mode| {
                                        let mode = *mode;
                                        Self::mode_pill(
                                            ElementId::named_usize("theme-mode", mode as usize),
                                            mode.label(),
                                            mode == current_theme_mode,
                                            p,
                                            cx.listener(move |this, _ev, _window, cx| {
                                                this.set_theme_mode(mode, cx);
                                            }),
                                        )
                                    })),
                            ),
                    ),
            )
            // ── Custom Theme (TOML) ──────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pt(px(12.0))
                    .border_t_1()
                    .border_color(rgb(p.border_subtle))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(p.text))
                            .child("Custom Theme"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.0))
                            .child(Self::mode_pill(
                                "download-theme-template".into(),
                                "Download Theme Template",
                                false,
                                p,
                                cx.listener(|this, _ev, window, cx| {
                                    this.download_theme_template(window, cx);
                                }),
                            ))
                            .child(Self::mode_pill(
                                "import-theme".into(),
                                "Import Theme",
                                false,
                                p,
                                cx.listener(|this, _ev, window, cx| {
                                    this.import_theme(window, cx);
                                }),
                            )),
                    )
                    .when_some(self.theme_import_error.as_ref(), |d, msg| {
                        // Same red/mode pairing as `capture_prompt`'s conflict
                        // warning (dark red is illegible on a light background).
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(match current_theme_mode {
                                    ThemeMode::Dark => 0xf48771,
                                    ThemeMode::Light => 0xb02a15,
                                }))
                                .child(msg.clone()),
                        )
                    }),
            )
    }
}

impl Render for SettingsModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders a semi-transparent full-screen backdrop with a centred dialog
         * panel on top.
         *
         * Layout:
         *   • Full-screen dimmed backdrop — clicking it closes the modal
         *   • Centred panel containing:
         *       – Title bar with "Settings" heading and a × close button
         *       – A content row: the section sidebar on the left, the selected
         *         section's pane on the right (see `SettingsSection`)
         *       – Reset to Defaults / Close button row
         *
         * Every color comes from the active theme's `Palette`. It used to be
         * hardcoded VS-Code-dark hex with a `theme_preview ? palette : hex`
         * split, which meant the modal stayed dark-on-dark in light mode —
         * unreadable — unless you happened to be in Theme Preview. The flag
         * now controls layout only (narrower panel, no backdrop dim, no
         * sidebar or keybind list), never color.
         *
         * The panel tracks its own focus handle and listens for key-down
         * events so `start_capture` can claim focus and `handle_capture_key`
         * receives the very next keystroke, regardless of which button was
         * clicked to arm capture.
         */
        let vim_enabled = self.state.read(cx).vim_enabled;
        let spellcheck_enabled = self.state.read(cx).spellcheck_enabled;
        let spellcheck_color = self.state.read(cx).spellcheck_underline_color.clone();
        let spreading_wpm = self.state.read(cx).spreading_wpm;
        let nav_fold_buttons = self.state.read(cx).nav_fold_buttons;
        let search_from_list_enabled = self.state.read(cx).search_from_list_enabled;
        let search_list_whole_words = self.state.read(cx).search_list_whole_words;
        let command_palette_enabled = self.state.read(cx).command_palette_enabled;
        let shrink_points = self.state.read(cx).small_size_half_points / 2;
        let exception = self.state.read(cx).standardize_highlight_exception.clone();
        let analytic_color = self.state.read(cx).analytic_color.clone();
        // The same colors the HL Color dropdown offers — built-ins plus
        // whatever the user has saved — so the exception can name any highlight
        // actually reachable in the document.
        let custom_highlights: Vec<u32> = self
            .state
            .read(cx)
            .custom_colors(crate::state::CustomColorTarget::Highlight)
            .to_vec();
        let (emphasis, emphasis_change_size, emphasis_size_points, paste_condense, paste_condense_pilcrow) = {
            let st = self.state.read(cx);
            (
                (st.emphasis_bold, st.emphasis_underline, st.emphasis_box),
                st.emphasis_change_size,
                st.emphasis_size_half_points / 2,
                st.paste_condense,
                st.paste_condense_pilcrow,
            )
        };
        let current_theme = self.state.read(cx).theme;
        let current_theme_mode = self.state.read(cx).theme_mode;
        let current_theme_color_mode = self.state.read(cx).theme_color_mode;
        let keybinds = self.state.read(cx).keybinds.clone();
        let vim_keybinds = self.state.read(cx).vim_keybinds.clone();
        let p = self.state.read(cx).current_palette();
        let theme_preview = self.theme_preview;
        let section = self.section;

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(black().opacity(if theme_preview { 0.0 } else { 0.55 }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, window, cx| {
                this.close(window, cx);
            }))
            // Stops wheel events over the modal (including its padding, not
            // just the inner scrollable list) from bubbling to the document
            // editor underneath.
            .on_scroll_wheel(|_ev, _window, cx| cx.stop_propagation())
            .child(
                div()
                    .id("settings-panel")
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::handle_capture_key))
                    // Wide enough for the sidebar plus a keybind row's
                    // label/combo/Change trio without the combo chip
                    // wrapping.
                    .w(px(if theme_preview { 380.0 } else { 860.0 }))
                    .h(px(if theme_preview { 420.0 } else { 620.0 }))
                    .bg(rgb(p.chrome))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    // Stops the mouse-down from bubbling up to the backdrop's
                    // close handler above. A plain no-op handler here does
                    // NOT do this by itself — GPUI mouse events keep bubbling
                    // through every ancestor's on_mouse_down unless one of
                    // them explicitly calls stop_propagation, exactly like
                    // keyboard dispatch. Without this, every click anywhere
                    // in the panel (Change buttons, category headers, the
                    // vim toggle, Reset) closed the modal.
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    // ── Title bar ────────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .flex_none()
                            .px(px(20.0))
                            .py(px(14.0))
                            .border_b_1()
                            .border_color(rgb(p.border_subtle))
                            .when(theme_preview, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .id("settings-preview-back")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .rounded(px(4.0))
                                                .cursor_pointer()
                                                .text_color(rgb(p.text_muted))
                                                .bg(rgb(p.chrome_active))
                                                .border_1()
                                                .border_color(rgb(p.border_subtle))
                                                .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                                                .active(move |s| s.bg(rgb(p.chrome_active)))
                                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                                    this.exit_theme_preview(cx);
                                                }))
                                                .child("‹"),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(p.text))
                                                .font_weight(FontWeight::BOLD)
                                                .child("Theme Preview"),
                                        ),
                                )
                            })
                            .when(!theme_preview, |d| {
                                d.child(
                                    div()
                                        .text_color(rgb(p.text))
                                        .font_weight(FontWeight::BOLD)
                                        .child("Settings"),
                                )
                                .child(
                                    div()
                                        .id("settings-close-x")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .text_color(rgb(p.text_muted))
                                        .bg(rgb(p.chrome_active))
                                        .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                                        .on_click(cx.listener(|this, _ev, window, cx| {
                                            this.close(window, cx);
                                        }))
                                        .child("×"),
                                )
                            }),
                    )
                    // ── Sidebar │ selected pane ──────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .when(!theme_preview, |d| d.child(self.render_sidebar(p, cx)))
                            .child(
                                div()
                                    .id("settings-body-scroll")
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .p(px(20.0))
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    // Theme Preview is Appearance-only, with
                                    // no sidebar to choose anything else.
                                    .when(theme_preview || section == SettingsSection::Appearance, |d| {
                                        d.child(self.render_appearance(
                                            current_theme,
                                            current_theme_mode,
                                            current_theme_color_mode,
                                            theme_preview,
                                            p,
                                            cx,
                                        ))
                                    })
                                    .when(!theme_preview && section == SettingsSection::TextSettings, |d| {
                                        d.child(self.render_text_settings(
                                            emphasis,
                                            emphasis_change_size,
                                            emphasis_size_points,
                                            shrink_points,
                                            exception.clone(),
                                            custom_highlights.clone(),
                                            analytic_color.clone(),
                                            paste_condense,
                                            paste_condense_pilcrow,
                                            p,
                                            cx,
                                        ))
                                    })
                                    .when(!theme_preview && section == SettingsSection::Keybindings, |d| {
                                        d.children(KeybindCategory::all().iter().map(|category| {
                                            self.render_category(*category, &keybinds, p, current_theme_mode, cx)
                                        }))
                                        .when(vim_enabled, |d| {
                                            d.child(self.render_vim_keybinds_section(&vim_keybinds, p, current_theme_mode, cx))
                                        })
                                    })
                                    .when(!theme_preview && section == SettingsSection::ToggleFeatures, |d| {
                                        d.child(self.render_toggle_features(
                                            vim_enabled,
                                            spellcheck_enabled,
                                            spellcheck_color.clone(),
                                            spreading_wpm,
                                            nav_fold_buttons,
                                            search_from_list_enabled,
                                            search_list_whole_words,
                                            command_palette_enabled,
                                            p,
                                            cx,
                                        ))
                                    }),
                            ),
                    )
                    // ── Bottom button row ────────────────────────────────────
                    .when(!theme_preview, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .flex_none()
                                .px(px(20.0))
                                .py(px(12.0))
                                .border_t_1()
                                .border_color(rgb(p.border_subtle))
                                // closed_beta_plan.md §3: always-visible build
                                // string so a tester reporting a bug can read
                                // off exactly what build they're on.
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.text_faint))
                                        .child(build_version_string()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .id("settings-reset-btn")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .px(px(16.0))
                                                .py(px(6.0))
                                                .bg(rgb(p.chrome_active))
                                                .rounded(px(4.0))
                                                .cursor_pointer()
                                                .text_sm()
                                                .text_color(rgb(p.text))
                                                .border_1()
                                                .border_color(rgb(p.border))
                                                .hover(move |s| s.bg(rgb(p.chrome_hover)))
                                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                                    this.reset_to_defaults(cx);
                                                }))
                                                .child("Reset to Defaults"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-close-btn")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .px(px(16.0))
                                                .py(px(6.0))
                                                .bg(rgb(p.accent))
                                                .rounded(px(4.0))
                                                .cursor_pointer()
                                                .text_sm()
                                                // Fixed light label: the accent
                                                // is a saturated mid-tone in
                                                // both modes, while `p.text`
                                                // inverts and would disappear
                                                // on it in light mode.
                                                .text_color(rgb(0xffffff))
                                                .hover(move |s| s.bg(rgb(p.accent_strong)))
                                                .on_click(cx.listener(|this, _ev, window, cx| {
                                                    this.close(window, cx);
                                                }))
                                                .child("Close"),
                                        ),
                                ),
                        )
                    }),
            )
    }
}
