use gpui::prelude::*;
use gpui::*;

use crate::keybinds::{rebuild_keymap, KeyCombo, KeybindAction, KeybindCategory, Keybinds};
use crate::state::{bundled_default_settings_path, settings_conf_path, AppState, CardStyleKind};
use crate::theme::{palette, ThemeColorMode, ThemeKind, ThemeMode};

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
    Fonts,
    Keybindings,
    ToggleFeatures,
    Timer,
}

impl SettingsSection {
    /// Sidebar order, top to bottom. `ToggleFeatures` is deliberately last —
    /// the toggles belong at the bottom of the settings list.
    fn all() -> [SettingsSection; 6] {
        [
            SettingsSection::Appearance,
            SettingsSection::TextSettings,
            SettingsSection::Fonts,
            SettingsSection::Keybindings,
            SettingsSection::ToggleFeatures,
            SettingsSection::Timer,
        ]
    }

    fn label(&self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
            SettingsSection::TextSettings => "Text Settings",
            SettingsSection::Fonts => "Fonts",
            SettingsSection::Keybindings => "Keybindings",
            SettingsSection::ToggleFeatures => "Toggle Features",
            SettingsSection::Timer => "Timer",
        }
    }

    pub fn icon(&self) -> crate::icons::Icon {
        match self {
            SettingsSection::Appearance => crate::icons::Icon::SettingsAppearance,
            SettingsSection::TextSettings => crate::icons::Icon::SettingsText,
            SettingsSection::Fonts => crate::icons::Icon::SettingsFonts,
            SettingsSection::Keybindings => crate::icons::Icon::SettingsKeybindings,
            SettingsSection::ToggleFeatures => crate::icons::Icon::SettingsToggles,
            SettingsSection::Timer => crate::icons::Icon::Timer,
        }
    }
}

/// `{version} ({git_sha})`, e.g. `0.1.0-beta.1 (a1b2c3d)` — both baked in at
/// compile time (`Cargo.toml`'s version, and `build.rs`'s
/// `VIMBATIM_GIT_SHA`). Shown in the settings modal so a beta tester can
/// read off exactly what build a bug report came from
/// (`closed_beta_plan.md` §3).
fn build_version_string() -> String {
    format!(
        "{} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("VIMBATIM_GIT_SHA")
    )
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
    fn handle_word_list_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                let Some(c) = crate::state::vim_find_target_char(
                    key,
                    ks.modifiers.shift,
                    ks.key_char.as_deref(),
                ) else {
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
            s.close_settings();
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
    fn start_capture(
        &mut self,
        action: KeybindAction,
        slot: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let keybinds = self.state.read(cx).keybinds().clone();
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
    fn handle_capture_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    fn handle_ctrl_capture_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((action, slot)) = self.capturing else {
            return;
        };
        let ks = &event.keystroke;

        let Some(combo) = KeyCombo::from_capture(&ks.modifiers, &ks.key) else {
            // Escape: cancel capture, keeping the existing binding.
            self.cancel_capture(cx);
            cx.notify();
            return;
        };

        let conflict = self
            .state
            .read(cx)
            .keybinds()
            .find_conflict(&combo, (action, slot));
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
            s.set_keybind(action, slot, combo.clone());
            let _ = s
                .keybinds()
                .save_to(&settings_path(), s.global_vim().vim_enabled, &[]);
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
    fn start_vim_capture(
        &mut self,
        action: KeybindAction,
        existing: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    fn handle_vim_capture_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((action, existing)) = self.vim_capturing.clone() else {
            return;
        };
        let ks = &event.keystroke;

        match ks.key.as_str() {
            "escape" => {
                self.cancel_vim_capture();
                cx.notify();
            }
            "backspace" => {
                self.vim_capture_buffer.pop();
                self.vim_conflict_message = None;
                cx.notify();
            }
            "enter" => {
                if self.vim_capture_buffer.is_empty() {
                    return;
                }
                let candidate = self.vim_capture_buffer.clone();
                let exclude = existing.as_deref();

                if crate::vim_keybinds::VimKeybinds::is_reserved_first_key(&candidate) {
                    self.vim_conflict_message = Some(format!(
                        "{candidate:?} is reserved or invalid. Use an unreserved sequence or :name (letters, digits, underscore; letter first). Esc keeps the current binding."
                    ));
                    self.vim_capture_buffer.clear();
                    cx.notify();
                    return;
                }
                let conflict = self
                    .state
                    .read(cx)
                    .global_vim()
                    .vim_keybinds
                    .find_overlap_conflict(&candidate, exclude);
                if let Some((other, other_seq)) = conflict {
                    self.vim_conflict_message = Some(format!(
                        "{candidate:?} overlaps with {other_seq:?}, already used by \"{}\". Try a different sequence, or Esc to keep the current binding.",
                        other.label(),
                    ));
                    self.vim_capture_buffer.clear();
                    cx.notify();
                    return;
                }
                if let Some(native) =
                    crate::vim_keybinds::VimKeybinds::find_native_vim_conflict(&candidate)
                {
                    self.vim_conflict_message = Some(format!(
                        "{candidate:?} overlaps with {native:?}, which is vim's own scroll command. Try a different sequence, or Esc to keep the current binding."
                    ));
                    self.vim_capture_buffer.clear();
                    cx.notify();
                    return;
                }

                self.state.update(cx, |s, _cx| {
                    s.set_vim_keybind(
                        action,
                        existing.as_deref(),
                        candidate.clone(),
                        &settings_path(),
                    );
                });
                self.cancel_vim_capture();
                cx.notify();
            }
            _ => {
                // `vim_find_target_char` handles the same shift/key_char
                // normalization the real vim dispatcher uses, so what's
                // typed here matches exactly what `VimKeybinds` will later
                // be asked to look up at runtime.
                if let Some(c) = crate::state::vim_find_target_char(
                    &ks.key,
                    ks.modifiers.shift,
                    ks.key_char.as_deref(),
                ) {
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
    fn render_word_list_box(
        &self,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let words = self.state.read(cx).search_word_list().to_vec();
        let editing = self.editing_word_list;
        // While editing, paint the live buffer (which can hold a trailing
        // blank line the saved list deliberately drops); otherwise the saved
        // list, so the box still shows its contents after focus moves away.
        let lines: Vec<String> = if editing {
            self.word_list_buffer
                .split('\n')
                .map(ToString::to_string)
                .collect()
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _ev, window, cx| {
                            this.start_word_list_edit(window, cx);
                        }),
                    )
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

    fn adjust_timer_default(
        &mut self,
        speech_slot: Option<usize>,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            let prefs = s.preferences();
            let mut speech = [
                prefs.speech_time_minutes,
                prefs.speech_time_2_minutes,
                prefs.speech_time_3_minutes,
            ];
            let prep = prefs.prep_time_minutes;
            if let Some(slot) = speech_slot {
                speech[slot] = (speech[slot] as i32 + delta).clamp(1, 120) as u16
            }
            let prep = if speech_slot.is_none() {
                (prep as i32 + delta).clamp(1, 120) as u16
            } else {
                prep
            };
            s.set_timer_defaults(speech, prep);
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
            s.set_spellcheck_underline_color(name);
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
            let current = (s.preferences().small_size_half_points / 2) as i32;
            s.set_shrink_size_points((current + delta).max(0) as u16);
            cx.notify();
        });
        cx.notify();
    }

    /// One stepper click on a card style's size. `None` means Cite, which
    /// isn't a `CardStyleKind` (it targets the selection, not the whole line)
    /// and so has its own setter — see `AppState::set_cite_size_points`.
    fn adjust_card_size(
        &mut self,
        kind: Option<CardStyleKind>,
        delta: i32,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            let current = match kind {
                Some(kind) => s.card_size_half_points(kind),
                None => s.preferences().cite_size_half_points,
            } as i32
                / 2;
            let points = (current + delta).max(0) as u16;
            match kind {
                Some(kind) => s.set_card_size_points(kind, points),
                None => s.set_cite_size_points(points),
            }
            cx.notify();
        });
        cx.notify();
    }

    fn adjust_emphasis_size(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let current = (s.preferences().emphasis_size_half_points / 2) as i32;
            s.set_emphasis_size_points((current + delta).max(0) as u16);
            cx.notify();
        });
        cx.notify();
    }

    fn adjust_spreading_wpm(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            let next = crate::state::clamp_spreading_wpm(
                (s.preferences().spreading_wpm as i32 + delta).max(0) as u32,
            );
            s.set_spreading_wpm(next);
            cx.notify();
        });
        cx.notify();
    }

    /// Settings -> Appearance -> Download Theme Template: native save dialog
    /// (same `prompt_for_new_path` gpui uses for the app's own Save As, see
    /// `main_window.rs`), writes `theme::custom_theme_template()` verbatim —
    /// a blank starting point the user edits and re-imports.
    fn download_theme_template(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.state.read(cx).workspace().working_directory.clone();
        let path_rx = cx.prompt_for_new_path(&dir, Some("theme_template.toml"));
        let state = self.state.clone();

        let (dark, light) = state.read(cx).theme_palettes();

        cx.spawn_in(window, async move |_this, cx| {
            let Ok(Ok(Some(path))) = path_rx.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    std::fs::write(path, crate::theme::custom_theme_template(&dark, &light))
                })
                .await;
            if let Err(error) = result {
                state.update(cx, |state, cx| {
                    state.apply_effect(crate::app::command::AppEffect::ShowError(format!(
                        "Could not save theme template: {error}"
                    )));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Settings -> Appearance -> Import Theme: native open dialog, then
    /// `AppState::import_custom_theme` parses+adopts it. Invalid TOML shows
    /// `theme_import_error` inline rather than silently doing nothing.
    fn import_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths_rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let state = self.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths_rx.await else {
                return;
            };
            let Some(path) = paths.pop() else { return };
            let content = cx
                .background_executor()
                .spawn(async move { std::fs::read_to_string(path) })
                .await;
            let Ok(content) = content else {
                state.update(cx, |state, cx| {
                    state.apply_effect(crate::app::command::AppEffect::ShowError(
                        "Could not read theme file.".to_string(),
                    ));
                    cx.notify();
                });
                let _ = this.update(cx, |this, cx| {
                    this.theme_import_error = Some("Couldn't read that file.".to_string());
                    cx.notify();
                });
                return;
            };
            let ok = state.update(cx, |s, cx| {
                let ok = s.import_custom_theme(&content);
                if ok {
                    cx.notify();
                }
                ok
            });
            let _ = this.update(cx, |this, cx| {
                this.theme_import_error = if ok {
                    None
                } else {
                    Some(
                        "Not a valid theme file — missing a [dark]/[light] section or a color."
                            .to_string(),
                    )
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn set_theme(&mut self, theme: ThemeKind, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.set_theme(theme);
            cx.notify();
        });
        cx.notify();
    }

    fn set_theme_color_mode(&mut self, mode: ThemeColorMode, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.set_theme_color_mode(mode);
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
            s.set_theme_mode(mode);
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
            s.replace_keybinds(keybinds);
            s.replace_vim_settings(vim_keybinds, vim_enabled);
            s.apply_theme_preferences(theme, theme_mode, theme_color_mode);
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, window, cx| {
                            this.start_capture(action, Some(index), window, cx);
                        }),
                    )
                    .child("Change"),
            )
            .child(
                div()
                    .id(ElementId::named_usize("keybind-remove", base_id))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(p.text_faint))
                    .hover(move |s| s.text_color(rgb(p.text)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.state.update(cx, |s, _cx| {
                                s.remove_keybind(action, index);
                                let _ = s.keybinds().save_to(
                                    &settings_path(),
                                    s.global_vim().vim_enabled,
                                    &[],
                                );
                            });
                            this.cancel_capture(cx); // rebuilds the keymap without the removed combo
                            cx.notify();
                        }),
                    )
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
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, window, cx| {
                        this.start_capture(action, None, window, cx);
                    }),
                )
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.text))
                            .child(action.label()),
                    )
                    .when(action.is_stub(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.text_faint))
                                .child("(not yet implemented)"),
                        )
                    })
                    .when(
                        combos.is_empty() && self.capturing != Some((action, None)),
                        |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.text_faint))
                                    .child("Unbound"),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .children(slots),
            )
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
        let actions = Self::listed_actions(
            category,
            self.state.read(cx).preferences().command_palette_enabled,
        );

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(p.border_subtle))
            .child(
                div()
                    .id(ElementId::named_usize(
                        "keybind-category",
                        category as u8 as usize,
                    ))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .py(px(2.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.text))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            let collapsed = this.collapsed.get(&category).copied().unwrap_or(false);
                            this.collapsed.insert(category, !collapsed);
                            cx.notify();
                        }),
                    )
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
                            self.render_action_row(
                                action,
                                keybinds.get_all(action),
                                p,
                                theme_mode,
                                cx,
                            )
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
        if self
            .vim_capturing
            .as_ref()
            .is_some_and(|(a, s)| *a == action && s.as_deref() == Some(sequence))
        {
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
                            this.start_vim_capture(
                                action,
                                Some(sequence_owned.clone()),
                                window,
                                cx,
                            );
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
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.state.update(cx, |s, _cx| {
                                s.remove_vim_keybind(&sequence_owned, &settings_path());
                            });
                            this.cancel_vim_capture();
                            cx.notify();
                        }),
                    )
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
                    if self.vim_capture_buffer.is_empty() {
                        "…".to_string()
                    } else {
                        self.vim_capture_buffer.clone()
                    }
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

        let is_adding = self
            .vim_capturing
            .as_ref()
            .is_some_and(|(a, s)| *a == action && s.is_none());
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
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _ev, window, cx| {
                        this.start_vim_capture(action, None, window, cx);
                    }),
                )
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.text))
                            .child(action.label()),
                    )
                    .when(sequences.is_empty() && !is_adding, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.text_faint))
                                .child("Unbound"),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .children(slots),
            )
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
        let actions = Self::listed_actions(
            category,
            self.state.read(cx).preferences().command_palette_enabled,
        );

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .py(px(6.0))
            .border_b_1()
            .border_color(rgb(p.border_subtle))
            .child(
                div()
                    .id(ElementId::named_usize(
                        "vim-keybind-category",
                        category as u8 as usize,
                    ))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .py(px(2.0))
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.text))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            let collapsed =
                                this.vim_collapsed.get(&category).copied().unwrap_or(false);
                            this.vim_collapsed.insert(category, !collapsed);
                            cx.notify();
                        }),
                    )
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
                            self.render_vim_action_row(
                                action,
                                vim_keybinds.get_all(action),
                                p,
                                theme_mode,
                                cx,
                            )
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
                                "Bind an action to a Normal/Visual-mode sequence or a command such as :myhighlight. \
                                 Command names start with an ASCII letter and contain only letters, digits or _. \
                                 Press Enter to save or execute a command. Native commands are reserved.",
                            ),
                    ),
            )
            .children(KeybindCategory::all().iter().map(|category| {
                self.render_vim_category(*category, vim_keybinds, p, theme_mode, cx)
            }))
    }
}

mod view;
