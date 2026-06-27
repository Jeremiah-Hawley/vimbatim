use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;

/// The floating settings modal. Renders as a centred overlay on top of the
/// main window whenever `AppState.settings_visible` is true.
///
/// Currently shows a title, a placeholder description, and a demo button that
/// prints to the console. Adding real settings rows in the future only requires
/// inserting new children inside the modal body div.
pub struct SettingsModal {
    state: Entity<AppState>,
}

impl SettingsModal {
    pub fn new(state: Entity<AppState>) -> Self {
        /*
         * Constructs the SettingsModal. Visibility is controlled externally via
         * `AppState.settings_visible`; the modal itself is always fully constructed
         * and only conditionally rendered by MainWindow.
         */
        SettingsModal { state }
    }

    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Hides the modal by setting `AppState.settings_visible` to false.
         * Both the backdrop click and the explicit Close / × buttons call this.
         */
        self.state.update(cx, |s, cx| {
            s.settings_visible = false;
            cx.notify();
        });
        cx.notify();
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
         *   • Centred white-ish panel containing:
         *       – Title bar with "Settings" heading and a × close button
         *       – Placeholder description text
         *       – Demo button row (prints to console when clicked)
         *       – Close button row at the bottom
         *
         * Future settings rows go between the description and the demo row.
         * Each row should follow the pattern: label on the left, control on the right.
         */

        // Full-screen dimmed backdrop (absolute, so it covers the main content)
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            // Semi-transparent black overlay
            .bg(black().opacity(0.55))
            // Clicking the backdrop closes the modal
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, window, cx| {
                this.close(window, cx);
            }))
            // ── Centred dialog panel ─────────────────────────────────────────
            .child(
                div()
                    .id("settings-panel")
                    .w(px(440.0))
                    .bg(rgb(0x2d2d2d))
                    .rounded(px(8.0))
                    .shadow_lg()
                    // Absorb clicks so they don't reach the backdrop handler
                    .on_mouse_down(MouseButton::Left, |_ev, _window, _cx| { /* no-op */ })
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
                    // ── Modal body ──────────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(16.0))
                            .p(px(20.0))
                            // Placeholder description
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0x858585))
                                    .child(
                                        "Settings are loaded from settings.conf in the working \
                                         directory. Additional controls will appear here as they \
                                         are implemented.",
                                    ),
                            )
                            // ── Demo button row ───────────────────────────────────
                            // Remove or replace once real settings controls are implemented.
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .py(px(8.0))
                                    .border_t_1()
                                    .border_color(rgb(0x464647))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0xd4d4d4))
                                            .child("Demo setting"),
                                    )
                                    .child(
                                        div()
                                            .id("settings-demo-btn")
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
                                            .on_click(|_ev, _window, _cx| {
                                                // Demo: prints to stdout when clicked.
                                                // Replace with real setting logic.
                                                println!("[SettingsModal] demo button clicked");
                                            })
                                            .child("Click me"),
                                    ),
                            )
                            // ── Bottom close button ──────────────────────────────
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .child(
                                        div()
                                            .id("settings-close-btn")
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
                                            .on_click(cx.listener(|this, _ev, window, cx| {
                                                this.close(window, cx);
                                            }))
                                            .child("Close"),
                                    ),
                            ),
                    ),
            )
    }
}
