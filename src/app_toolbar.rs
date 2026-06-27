use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;

/// A toolbar row below the tab bar showing the app name, sidebar toggle, and
/// placeholder buttons reserved for future features.
pub struct AppToolbar {
    state: Entity<AppState>,
}

impl AppToolbar {
    pub fn new(state: Entity<AppState>) -> Self {
        /*
         * Constructs the AppToolbar. Sidebar visibility is read from and written
         * to the shared AppState, matching the Ctrl+B keybinding behaviour.
         */
        AppToolbar { state }
    }

    fn toolbar_btn(id: &'static str, label: &'static str) -> impl IntoElement {
        /*
         * Renders a generic placeholder button with a consistent style.
         * The id must be unique across all buttons in one render pass.
         */
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(24.0))
            .px(px(10.0))
            .rounded(px(4.0))
            .text_xs()
            .text_color(rgb(0x858585))
            .cursor_pointer()
            .border_1()
            .border_color(rgb(0x464647))
            .child(label)
    }
}

impl Render for AppToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the app toolbar row:
         *
         *   Vimbatim  |  [≡ Sidebar]  [Btn 1]  [Btn 2]  [Btn 3]  [Btn 4]
         *
         * The sidebar toggle mutates AppState directly rather than dispatching an
         * action so it works regardless of which element has keyboard focus.
         *
         * The placeholder buttons (Btn 1–4) are stubs for future features such as
         * find-in-files, word count, export, or version history.
         */
        let sidebar_visible = self.state.read(cx).sidebar_visible;
        let sidebar_label = if sidebar_visible { "≡  Hide Files" } else { "≡  Show Files" };

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(36.0))
            .px(px(12.0))
            .gap(px(8.0))
            .bg(rgb(0x252526))
            .border_b_1()
            .border_color(rgb(0x2d2d2d))
            // ── App name ──────────────────────────────────────────────────────
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x569cd6))
                    .pr(px(8.0))
                    .border_r_1()
                    .border_color(rgb(0x464647))
                    .child("Vimbatim"),
            )
            // ── Sidebar toggle ────────────────────────────────────────────────
            .child(
                div()
                    .id("toolbar-sidebar-toggle")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px(px(10.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .text_color(rgb(0xd4d4d4))
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(0x569cd6))
                    // Directly mutate AppState so the button works regardless of focus
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.state.update(cx, |s, cx| {
                            s.sidebar_visible = !s.sidebar_visible;
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    .child(sidebar_label),
            )
            // ── Placeholder buttons for future features ───────────────────────
            .child(Self::toolbar_btn("toolbar-btn-1", "Find"))
            .child(Self::toolbar_btn("toolbar-btn-2", "Word Count"))
            .child(Self::toolbar_btn("toolbar-btn-3", "Export"))
            .child(Self::toolbar_btn("toolbar-btn-4", "History"))
    }
}
