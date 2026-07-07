use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;
use crate::theme::{palette, radius, space};

/// A toolbar row below the tab bar showing the app name, sidebar toggle,
/// future command hooks, and secondary app controls.
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

    fn future_command(
        id: &'static str,
        label: &'static str,
        p: crate::theme::Palette,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(24.0))
            .px(px(10.0))
            .rounded(px(radius::MD))
            .text_xs()
            .text_color(rgb(p.text_muted))
            .cursor_pointer()
            .border_1()
            .border_color(rgb(p.border_subtle))
            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            .active(move |s| s.bg(rgb(p.chrome_active)))
            .child(label)
    }
}

impl Render for AppToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the app toolbar row:
         *
         *   Vimbatim  |  [≡ Sidebar]                         [Settings]
         *
         * The sidebar toggle mutates AppState directly rather than dispatching an
         * action so it works regardless of which element has keyboard focus.
         *
         * This row gives users orientation without competing with the ribbon's
         * command surface.
         */
        let state = self.state.read(cx);
        let p = palette(state.theme);
        let sidebar_visible = state.sidebar_visible;
        let _ = state;

        let sidebar_label = if sidebar_visible {
            "≡  Hide Files"
        } else {
            "≡  Show Files"
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(36.0))
            .px(px(space::MD))
            .gap(px(space::SM))
            .bg(rgb(p.editor_bg_raised))
            .border_b_1()
            .border_color(rgb(p.border_subtle))
            // ── App name ──────────────────────────────────────────────────────
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.accent))
                    .pr(px(space::SM))
                    .border_r_1()
                    .border_color(rgb(p.border))
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
                    .rounded(px(radius::MD))
                    .text_xs()
                    .text_color(rgb(p.text))
                    .cursor_pointer()
                    .bg(rgb(p.accent_muted))
                    .border_1()
                    .border_color(rgb(p.accent))
                    .hover(move |s| s.bg(rgb(p.accent_strong)))
                    .active(move |s| s.bg(rgb(p.accent_muted)))
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
            .child(div().flex_1())
            // ── Future command hooks ─────────────────────────────────────────
            .child(Self::future_command("toolbar-find", "Find", p))
            .child(Self::future_command("toolbar-word-count", "Word Count", p))
            .child(Self::future_command("toolbar-export", "Export", p))
            .child(Self::future_command("toolbar-history", "History", p))
            // ── Secondary app controls ───────────────────────────────────────
            .child(
                div()
                    .id("toolbar-settings")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .w(px(32.0))
                    .rounded(px(radius::MD))
                    .text_lg()
                    .text_color(rgb(p.text_muted))
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(p.border))
                    .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(cx.listener(|this, _ev, _window, cx| {
                        this.state.update(cx, |s, cx| {
                            s.settings_visible = !s.settings_visible;
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    .child("⚙"),
            )
    }
}
