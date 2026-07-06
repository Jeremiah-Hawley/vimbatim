use gpui::prelude::*;
use gpui::*;
use std::path::Path;

use crate::keybinds::{rebuild_keymap, KeyCombo, KeybindAction, KeybindCategory, Keybinds};
use crate::state::AppState;

const SETTINGS_PATH: &str = "settings.conf";
const DEFAULT_SETTINGS_PATH: &str = "default_settings.conf";

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
    /// Suppresses normal app-wide action dispatch for every keystroke while
    /// `capturing` is armed. Without this, pressing a combo that's already
    /// globally bound elsewhere (e.g. Ctrl+S while remapping something
    /// else) would fire *that* action instead of ever reaching
    /// `handle_capture_key` — GPUI resolves an action's keybinding before
    /// delivering a raw key event to this view. Dropped (unsubscribing)
    /// the instant capture ends, successfully or not.
    capture_subscription: Option<Subscription>,
    /// Per-category collapse state for the keybind list, mirroring
    /// `formatting_ribbon.rs`'s own collapsible-group pattern.
    collapsed: std::collections::HashMap<KeybindCategory, bool>,
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
            capture_subscription: None,
            collapsed: std::collections::HashMap::new(),
        }
    }

    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Hides the modal by setting `AppState.settings_visible` to false.
         * Both the backdrop click and the explicit Close / × buttons call this.
         * Also cancels any in-progress key capture so closing the modal
         * never leaves a dangling keystroke interceptor active.
         */
        self.cancel_capture();
        self.state.update(cx, |s, cx| {
            s.settings_visible = false;
            cx.notify();
        });
        cx.notify();
    }

    /// Arms capture mode for `action`: the next keystroke (after this call)
    /// is interpreted as the candidate new binding by `handle_capture_key`.
    fn start_capture(&mut self, action: KeybindAction, window: &mut Window, cx: &mut Context<Self>) {
        self.capturing = Some(action);
        self.conflict_message = None;
        // See the field's doc comment: unconditionally swallow every
        // keystroke's normal action dispatch while capturing, so an
        // already-bound combo still reaches handle_capture_key below
        // instead of firing whatever it's currently bound to.
        self.capture_subscription = Some(cx.intercept_keystrokes(|_event, _window, cx| {
            cx.stop_propagation();
        }));
        self.focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    fn cancel_capture(&mut self) {
        self.capturing = None;
        self.conflict_message = None;
        self.capture_subscription = None; // dropping unsubscribes
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
            self.cancel_capture();
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

        let keybinds = self.state.update(cx, |s, _cx| {
            s.keybinds.set(action, combo.clone());
            let _ = s.keybinds.save_to(Path::new(SETTINGS_PATH), s.vim_enabled, &[]);
            s.keybinds.clone()
        });
        rebuild_keymap(cx, &keybinds);
        self.cancel_capture();
        cx.notify();
    }

    fn toggle_vim(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _cx| {
            s.vim_enabled = !s.vim_enabled;
            let _ = s.keybinds.save_to(Path::new(SETTINGS_PATH), s.vim_enabled, &[]);
        });
        cx.notify();
    }

    /// Copies default_settings.conf over settings.conf, reloads both the
    /// keybind registry and the vim flag from the now-reset file, rebuilds
    /// the live keymap, and cancels any in-progress capture.
    fn reset_to_defaults(&mut self, cx: &mut Context<Self>) {
        if std::fs::copy(DEFAULT_SETTINGS_PATH, SETTINGS_PATH).is_err() {
            return;
        }
        let path = Path::new(SETTINGS_PATH);
        let keybinds = Keybinds::load(path);
        let vim_enabled = crate::keybinds::load_vim_enabled(path);

        self.state.update(cx, |s, _cx| {
            s.keybinds = keybinds.clone();
            s.vim_enabled = vim_enabled;
        });
        rebuild_keymap(cx, &keybinds);
        self.cancel_capture();
        cx.notify();
    }

    /// Renders one action's row: its label on the left, and on the right
    /// either its current combo + a "Change" button, or (while this
    /// specific action is being captured) a live prompt / conflict message.
    fn render_action_row(&self, action: KeybindAction, combo: KeyCombo, cx: &mut Context<Self>) -> impl IntoElement {
        let is_capturing = self.capturing == Some(action);

        let right_side: AnyElement = if is_capturing {
            match &self.conflict_message {
                Some(msg) => div()
                    .text_xs()
                    .text_color(rgb(0xf48771))
                    .max_w(px(220.0))
                    .child(msg.clone())
                    .into_any_element(),
                None => div()
                    .text_xs()
                    .text_color(rgb(0x569cd6))
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
                        .text_color(rgb(0xd4d4d4))
                        .px(px(8.0))
                        .py(px(2.0))
                        .bg(rgb(0x3c3c3c))
                        .rounded(px(4.0))
                        .child(combo.display_string()),
                )
                .child(
                    div()
                        .id(ElementId::named_usize("keybind-change", action as usize))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(0x569cd6))
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
                    .child(div().text_sm().text_color(rgb(0xd4d4d4)).child(action.label()))
                    .when(action.is_stub(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(0x858585))
                                .child("(not yet implemented)"),
                        )
                    }),
            )
            .child(right_side)
    }

    /// Renders one collapsible category section (its header + every action
    /// row belonging to it), mirroring `formatting_ribbon.rs`'s own
    /// collapse-arrow convention.
    fn render_category(&self, category: KeybindCategory, keybinds: &Keybinds, cx: &mut Context<Self>) -> impl IntoElement {
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
            .border_color(rgb(0x3d3d3d))
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
                    .text_color(rgb(0xd4d4d4))
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
                            self.render_action_row(action, keybinds.get(action), cx)
                        })),
                )
            })
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
         *       – Vim Mode on/off toggle row
         *       – One collapsible section per KeybindCategory, each listing
         *         its actions' current binding + a "Change" (capture) button
         *       – Reset to Defaults / Close button row
         *
         * The panel tracks its own focus handle and listens for key-down
         * events so `start_capture` can claim focus and `handle_capture_key`
         * receives the very next keystroke, regardless of which button was
         * clicked to arm capture.
         */
        let vim_enabled = self.state.read(cx).vim_enabled;
        let keybinds = self.state.read(cx).keybinds.clone();

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(black().opacity(0.55))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, window, cx| {
                this.close(window, cx);
            }))
            .child(
                div()
                    .id("settings-panel")
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::handle_capture_key))
                    .w(px(520.0))
                    .max_h(px(640.0))
                    .bg(rgb(0x2d2d2d))
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
                    // ── Title bar ──────────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px(px(20.0))
                            .py(px(14.0))
                            .border_b_1()
                            .border_color(rgb(0x464647))
                            .child(
                                div()
                                    .text_color(rgb(0xd4d4d4))
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
                                    .text_color(rgb(0x858585))
                                    .bg(rgb(0x3c3c3c))
                                    .on_click(cx.listener(|this, _ev, window, cx| {
                                        this.close(window, cx);
                                    }))
                                    .child("×"),
                            ),
                    )
                    // ── Scrollable body ──────────────────────────────────────────
                    .child(
                        div()
                            .id("settings-body-scroll")
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .p(px(20.0))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            // ── Vim Mode toggle row ──────────────────────────────
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .pb(px(8.0))
                                    .border_b_1()
                                    .border_color(rgb(0x464647))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xd4d4d4))
                                            .child("Vim Mode"),
                                    )
                                    .child(
                                        div()
                                            .id("vim-mode-toggle")
                                            .cursor_pointer()
                                            .px(px(10.0))
                                            .py(px(4.0))
                                            .rounded(px(4.0))
                                            .text_xs()
                                            .when(vim_enabled, |d| d.bg(rgb(0x007acc)).text_color(rgb(0xffffff)))
                                            .when(!vim_enabled, |d| d.bg(rgb(0x3c3c3c)).text_color(rgb(0x999999)))
                                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, _window, cx| {
                                                this.toggle_vim(cx);
                                            }))
                                            .child(if vim_enabled { "On" } else { "Off" }),
                                    ),
                            )
                            // ── Keybind categories ───────────────────────────────
                            .children(
                                KeybindCategory::all()
                                    .iter()
                                    .map(|category| self.render_category(*category, &keybinds, cx)),
                            ),
                    )
                    // ── Bottom button row ────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px(px(20.0))
                            .py(px(12.0))
                            .border_t_1()
                            .border_color(rgb(0x464647))
                            .child(
                                div()
                                    .id("settings-reset-btn")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px(px(16.0))
                                    .py(px(6.0))
                                    .bg(rgb(0x3c3c3c))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(0xd4d4d4))
                                    .border_1()
                                    .border_color(rgb(0x555555))
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
                                    .bg(rgb(0x007acc))
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .text_sm()
                                    .text_color(rgb(0xffffff))
                                    .on_click(cx.listener(|this, _ev, window, cx| {
                                        this.close(window, cx);
                                    }))
                                    .child("Close"),
                            ),
                    ),
            )
    }
}
