use gpui::prelude::*;
use gpui::*;

use crate::app_toolbar::AppToolbar;
use crate::file_explorer::FileExplorer;
use crate::formatting_ribbon::FormattingRibbon;
use crate::settings_modal::SettingsModal;
use crate::state::AppState;
use crate::tab_bar::TabBar;
use crate::text_editor::TextEditor;

actions!(vimbatim, [ToggleSettings, ToggleSidebar, Save]);

/// The root view of the application window.
///
/// Owns all child views and the shared `AppState` model. Handles the two global
/// actions (toggle-settings, toggle-sidebar) and composes the full layout.
pub struct MainWindow {
    state: Entity<AppState>,
    tab_bar: Entity<TabBar>,
    app_toolbar: Entity<AppToolbar>,
    formatting_ribbon: Entity<FormattingRibbon>,
    text_editor: Entity<TextEditor>,
    file_explorer: Entity<FileExplorer>,
    settings_modal: Entity<SettingsModal>,
}

impl MainWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        /*
         * Constructs the MainWindow and all child views. A single shared AppState entity
         * is created here and passed (cloned as a handle) to every child view so they
         * all read/write the same state without explicit message-passing.
         *
         * Key bindings are registered on the App in main.rs; action handlers are wired
         * up in render() via `.on_action(cx.listener(...))`.
         */
        let state = cx.new(|_cx| AppState::new());

        let tab_bar           = cx.new(|_cx| TabBar::new(state.clone()));
        let app_toolbar       = cx.new(|_cx| AppToolbar::new(state.clone()));
        let formatting_ribbon = cx.new(|_cx| FormattingRibbon::new(state.clone()));
        let text_editor       = cx.new(|cx|  TextEditor::new(state.clone(), cx));
        let file_explorer     = cx.new(|_cx| FileExplorer::new(state.clone()));
        let settings_modal    = cx.new(|_cx| SettingsModal::new(state.clone()));

        MainWindow {
            state,
            tab_bar,
            app_toolbar,
            formatting_ribbon,
            text_editor,
            file_explorer,
            settings_modal,
        }
    }

    fn toggle_settings(&mut self, _: &ToggleSettings, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Flips AppState.settings_visible to show/hide the floating settings modal.
         * Notifying cx triggers a re-render of all views that read settings_visible.
         */
        self.state.update(cx, |s, cx| {
            s.settings_visible = !s.settings_visible;
            cx.notify();
        });
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Flips AppState.sidebar_visible to collapse/expand the file explorer panel.
         */
        self.state.update(cx, |s, cx| {
            s.sidebar_visible = !s.sidebar_visible;
            cx.notify();
        });
        cx.notify();
    }

    pub fn save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Ctrl+S handler. Delegates to AppState::save_active_tab and logs any error
         * to stderr — a future iteration should surface the error in the UI.
         */
        self.state.update(cx, |s, _cx| {
            if let Err(e) = s.save_active_tab() {
                eprintln!("[save] {}", e);
            }
        });
        cx.notify();
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Lays out the full application chrome:
         *
         *   ┌─────────────────────────────────────────┐
         *   │ Tab bar                                 │
         *   ├─────────────────────────────────────────┤
         *   │ Formatting ribbon (2 rows of buttons)   │
         *   ├────────────────────────────┬────────────┤
         *   │ Text editor (flex-1)       │ Sidebar    │
         *   └────────────────────────────┴────────────┘
         *
         * When settings_visible is true, SettingsModal is rendered as an absolute-
         * positioned child that overlays everything else.
         *
         * The outer container has `.relative()` so the modal's `.absolute()` is
         * scoped to this window rather than the display.
         */
        let sidebar_visible  = self.state.read(cx).sidebar_visible;
        let settings_visible = self.state.read(cx).settings_visible;

        div()
            // Wire up global action handlers for this window
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::save))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x1e1e1e))
            // Needed so the modal overlay's `absolute` is relative to this container
            .relative()
            // ── Tab bar ────────────────────────────────────────────────────
            .child(self.tab_bar.clone())
            // ── App toolbar (Vimbatim label, sidebar toggle, placeholders) ──
            .child(self.app_toolbar.clone())
            // ── Formatting ribbon ──────────────────────────────────────────
            .child(self.formatting_ribbon.clone())
            // ── Main content row ───────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    // min_h_0 is critical: without it a flex child won't respect
                    // the parent's height and will overflow.
                    .min_h_0()
                    .when(sidebar_visible, |d| d.child(self.file_explorer.clone()))
                    .child(self.text_editor.clone())
            )
            // ── Settings modal overlay ─────────────────────────────────────
            // Added last so it paints on top of all other children
            .when(settings_visible, |d| d.child(self.settings_modal.clone()))
    }
}
