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
    Keybindings,
    ToggleFeatures,
}

impl SettingsSection {
    /// Sidebar order, top to bottom. `ToggleFeatures` is deliberately last —
    /// the toggles belong at the bottom of the settings list.
    fn all() -> [SettingsSection; 3] {
        [
            SettingsSection::Appearance,
            SettingsSection::Keybindings,
            SettingsSection::ToggleFeatures,
        ]
    }

    fn label(&self) -> &'static str {
        match self {
            SettingsSection::Appearance => "Appearance",
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
    /// The action currently awaiting a keypress, if any (armed by clicking
    /// a row's "Change" button).
    capturing: Option<KeybindAction>,
    /// Set when a captured combo collides with another action's existing
    /// binding — shown inline on the capturing row. Capture stays active
    /// (rather than closing) so the user can just try a different key.
    conflict_message: Option<String>,
    /// Per-category collapse state for the keybind list, mirroring
    /// `formatting_ribbon.rs`'s own collapsible-group pattern.
    collapsed: std::collections::HashMap<KeybindCategory, bool>,
    /// Lightweight mode for cycling themes against the real app chrome
    /// without the dimmed backdrop or the full keybind settings list.
    theme_preview: bool,
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
            theme_preview: false,
            section: SettingsSection::Appearance,
        }
    }

    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Hides the modal by setting `AppState.settings_visible` to false.
         * Both the backdrop click and the explicit Close / × buttons call this.
         * Also cancels any in-progress key capture so closing the modal
         * never leaves the keymap cleared.
         */
        self.cancel_capture(cx);
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
    fn start_capture(&mut self, action: KeybindAction, window: &mut Window, cx: &mut Context<Self>) {
        self.capturing = Some(action);
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
    /// collide with another action, or showing an inline conflict message
    /// and staying in capture mode if it does.
    fn handle_capture_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.capturing else { return };
        let ks = &event.keystroke;

        let Some(combo) = KeyCombo::from_capture(&ks.modifiers, &ks.key) else {
            // Escape: cancel capture, keeping the existing binding.
            self.cancel_capture(cx);
            cx.notify();
            return;
        };

        let conflict = self.state.read(cx).keybinds.find_conflict(&combo, action);
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
            s.keybinds.set(action, combo.clone());
            let _ = s.keybinds.save_to(&settings_path(), s.vim_enabled, &[]);
        });
        self.cancel_capture(cx); // restores the keymap, now including the new binding
        cx.notify();
    }

    fn toggle_vim(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _cx| {
            s.vim_enabled = !s.vim_enabled;
            let _ = s.keybinds.save_to(&settings_path(), s.vim_enabled, &[]);
        });
        cx.notify();
    }

    fn toggle_spellcheck(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.spellcheck_enabled = !s.spellcheck_enabled;
            // `save_setting_line` is the generic single-key writer the theme
            // saves already use — it updates the key in place if present and
            // appends it otherwise, so there's no spellcheck-specific writer.
            let _ = crate::theme::save_setting_line(
                &settings_path(),
                "spellcheck",
                if s.spellcheck_enabled { "true" } else { "false" },
            );
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
        let vim_enabled = crate::keybinds::load_vim_enabled(path);
        let theme = crate::theme::load_theme(path);
        let theme_mode = crate::theme::load_theme_mode(path);
        let theme_color_mode = crate::theme::load_theme_color_mode(path);

        self.state.update(cx, |s, _cx| {
            s.keybinds = keybinds;
            s.vim_enabled = vim_enabled;
            s.theme = theme;
            s.theme_mode = theme_mode;
            s.theme_color_mode = theme_color_mode;
        });
        self.cancel_capture(cx); // also rebuilds the keymap from the now-reset keybinds
        cx.notify();
    }

    /// Renders one action's row: its label on the left, and on the right
    /// either its current combo + a "Change" button, or (while this
    /// specific action is being captured) a live prompt / conflict message.
    fn render_action_row(
        &self,
        action: KeybindAction,
        combo: KeyCombo,
        p: crate::theme::Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_capturing = self.capturing == Some(action);

        let right_side: AnyElement = if is_capturing {
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
        } else {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
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
                        .id(ElementId::named_usize("keybind-change", action as usize))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(p.accent))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, window, cx| {
                            this.start_capture(action, window, cx);
                        }))
                        .child("Change"),
                )
                .into_any_element()
        };

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
                    }),
            )
            .child(right_side)
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
        let actions: Vec<KeybindAction> = KeybindAction::all()
            .iter()
            .copied()
            .filter(|a| a.category() == category)
            .collect();

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
                            self.render_action_row(action, keybinds.get(action), p, theme_mode, cx)
                        })),
                )
            })
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
                        cx.notify();
                    }))
                    .child(section.label())
            }))
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
        let current_theme = self.state.read(cx).theme;
        let current_theme_mode = self.state.read(cx).theme_mode;
        let current_theme_color_mode = self.state.read(cx).theme_color_mode;
        let keybinds = self.state.read(cx).keybinds.clone();
        let p = palette(current_theme, current_theme_mode);
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
                                    .when(!theme_preview && section == SettingsSection::Keybindings, |d| {
                                        d.children(KeybindCategory::all().iter().map(|category| {
                                            self.render_category(*category, &keybinds, p, current_theme_mode, cx)
                                        }))
                                    })
                                    .when(!theme_preview && section == SettingsSection::ToggleFeatures, |d| {
                                        d.child(self.render_toggle_features(
                                            vim_enabled,
                                            spellcheck_enabled,
                                            spellcheck_color.clone(),
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
