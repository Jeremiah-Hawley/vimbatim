use gpui::prelude::*;
use gpui::*;

use crate::keybinds::{FindAction, OpenStatsAction, SaveAction, SaveAsAction};
use crate::state::AppState;
use crate::theme::{radius, space};

/// A toolbar row below the tab bar showing the app name, sidebar toggle,
/// the file/find/word-count/save-as commands, and secondary app controls.
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
        let p = state.current_palette();
        let sidebar_visible = state.ui().sidebar_visible;
        // Read Mode's button is disabled below pending a real rework — see
        // that `.child` block's comment.
        // let read_mode = state.read_mode;
        let search_from_list_enabled = state.preferences().search_from_list_enabled;
        let _ = state;

        let sidebar_label = if sidebar_visible {
            "Hide Files"
        } else {
            "Show Files"
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
                    .on_click(cx.listener(|_this, _ev, window, cx| {
                        window.dispatch_action(Box::new(crate::keybinds::ToggleSidebarAction), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(crate::icons::icon(
                                crate::icons::Icon::Sidebar,
                                p.text,
                                14.0,
                            ))
                            .child(sidebar_label),
                    ),
            )
            // ── Open folder ───────────────────────────────────────────────────
            .child(
                div()
                    .id("toolbar-open-folder")
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
                    // Native OS folder picker (gpui's own prompt_for_paths) —
                    // no dialog UI of our own to build or maintain.
                    .on_click(cx.listener(|_this, _ev, window, cx| {
                        window.dispatch_action(Box::new(crate::keybinds::OpenFolderAction), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(crate::icons::icon(
                                crate::icons::Icon::FolderOpen,
                                p.text,
                                14.0,
                            ))
                            .child("Open Folder"),
                    ),
            )
            // ── Open file ─────────────────────────────────────────────────────
            // Same native picker as Open Folder, flipped to files, feeding
            // `AppState::open_file` — which already parses the .docx, opens a
            // new tab, focuses the editor, and switches to the existing tab
            // instead if the file is already open.
            .child(
                div()
                    .id("toolbar-open-file")
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
                    .on_click(cx.listener(|_this, _ev, window, cx| {
                        window.dispatch_action(Box::new(crate::keybinds::OpenFileAction), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(crate::icons::icon(
                                crate::icons::Icon::FileOpen,
                                p.text,
                                14.0,
                            ))
                            .child("Open File"),
                    ),
            )
            .child(div().flex_1())
            // ── Future command hooks ─────────────────────────────────────────
            // ── Read Mode ─────────────────────────────────────────────────────
            // Disabled pending a real rework (checklist: needs to be "an
            // entirely different screen", not a flag on the current editor —
            // the codebase has no variable-height layout model to build true
            // pagination on, so this is deferred rather than shipped half-done).
            // Button hidden; the supporting state/behavior (AppState.read_mode,
            // toggle_read_mode, the ribbon's collapse-on-entry, the editor's
            // arrow-key paging) is left in place, just unreachable, for when
            // this is picked back up.
            // .child(
            //     div()
            //         .id("toolbar-read-mode")
            //         .flex()
            //         .items_center()
            //         .justify_center()
            //         .h(px(24.0))
            //         .px(px(10.0))
            //         .rounded(px(radius::MD))
            //         .text_xs()
            //         .cursor_pointer()
            //         .border_1()
            //         // Reads as engaged while active — it changes what the
            //         // arrow keys do, so it must not look like a momentary
            //         // button.
            //         .when(read_mode, |d| {
            //             d.bg(rgb(p.accent))
            //                 .text_color(rgb(0xffffff))
            //                 .border_color(rgb(p.accent_strong))
            //         })
            //         .when(!read_mode, |d| {
            //             d.text_color(rgb(p.text_muted))
            //                 .border_color(rgb(p.border_subtle))
            //                 .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            //                 .active(move |s| s.bg(rgb(p.chrome_active)))
            //         })
            //         .on_click(cx.listener(|this, _ev, _window, cx| {
            //             this.state.update(cx, |s, cx| {
            //                 s.toggle_read_mode();
            //                 cx.notify();
            //             });
            //             cx.notify();
            //         }))
            //         .child("Read Mode"),
            // )
            // ── Search From List ──────────────────────────────────────────────
            // Sits immediately left of Find (Read Mode's own slot, commented
            // out above), and only while the Toggle Features switch is on.
            //
            // Updates AppState directly rather than dispatching an action: no
            // keybind was requested for this, and a `KeybindAction` variant
            // would drag in the enum, its defaults, `rebuild_keymap`, a
            // `main_window` handler and a row in Settings → Keybinds for
            // nothing. Same direct-state approach the Settings ⚙ button uses.
            .when(search_from_list_enabled, |d| {
                d.child(
                    div()
                        .id("toolbar-search-from-list")
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(24.0))
                        .px(px(6.0))
                        .rounded(px(radius::MD))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(p.chrome_hover)))
                        .active(move |s| s.bg(rgb(p.chrome_active)))
                        .on_click(cx.listener(|this, _ev, _window, cx| {
                            this.state.update(cx, |s, cx| {
                                s.open_search_from_list();
                                cx.notify();
                            });
                            cx.notify();
                        }))
                        .tooltip(move |_window, cx| {
                            cx.new(|_| crate::formatting_ribbon::RibbonTooltip {
                                label: "Search From List".into(),
                                palette: p,
                            })
                            .into()
                        })
                        .child(crate::icons::icon(
                            crate::icons::Icon::SearchList,
                            p.text_muted,
                            16.0,
                        )),
                )
            })
            // Find opens the same panel Ctrl+F does, via the shared action.
            .child(
                div()
                    .id("toolbar-find")
                    .tooltip(move |_window, cx| {
                        cx.new(|_| crate::formatting_ribbon::RibbonTooltip {
                            label: "Search".into(),
                            palette: p,
                        })
                        .into()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px(px(6.0))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(|_ev, window, cx| {
                        window.dispatch_action(Box::new(FindAction), cx);
                    })
                    .child(crate::icons::icon(
                        crate::icons::Icon::Find,
                        p.text_muted,
                        16.0,
                    )),
            )
            // Word Count opens the stats panel, via the same action the
            // `open_stats` keybind already dispatches.
            .child(
                div()
                    .id("toolbar-word-count")
                    .tooltip(move |_window, cx| {
                        cx.new(|_| crate::formatting_ribbon::RibbonTooltip {
                            label: "Word Count".into(),
                            palette: p,
                        })
                        .into()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px(px(6.0))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(|_ev, window, cx| {
                        window.dispatch_action(Box::new(OpenStatsAction), cx);
                    })
                    .child(crate::icons::icon(
                        crate::icons::Icon::WordCount,
                        p.text_muted,
                        16.0,
                    )),
            )
            // ── Save As ───────────────────────────────────────────────────────
            // Dispatches the existing `SaveAsAction` rather than opening the
            // dialog here, so this button and its Ctrl+Shift+S keybind run the
            // identical handler (registered globally in `main_window.rs`).
            .child(
                div()
                    .id("toolbar-save-as")
                    .tooltip(move |_window, cx| {
                        cx.new(|_| crate::formatting_ribbon::RibbonTooltip {
                            label: "Save As".into(),
                            palette: p,
                        })
                        .into()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px(px(6.0))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(|_ev, window, cx| {
                        window.dispatch_action(Box::new(SaveAsAction), cx);
                    })
                    .child(crate::icons::icon(
                        crate::icons::Icon::SaveAs,
                        p.text_muted,
                        16.0,
                    )),
            )
            // ── Save ──────────────────────────────────────────────────────────
            // Dispatches the existing `SaveAction` (already live behind
            // Ctrl+S) — this button was the only thing missing, not the save
            // path itself.
            .child(
                div()
                    .id("toolbar-save")
                    .tooltip(move |_window, cx| {
                        cx.new(|_| crate::formatting_ribbon::RibbonTooltip {
                            label: "Save".into(),
                            palette: p,
                        })
                        .into()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.0))
                    .px(px(6.0))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(|_ev, window, cx| {
                        window.dispatch_action(Box::new(SaveAction), cx);
                    })
                    .child(crate::icons::icon(
                        crate::icons::Icon::Save,
                        p.text_muted,
                        16.0,
                    )),
            )
            // ── Secondary app controls ───────────────────────────────────────
            .child(
                div()
                    .id("toolbar-settings")
                    .tooltip(move |_window, cx| {
                        cx.new(|_| crate::formatting_ribbon::RibbonTooltip {
                            label: "Settings".into(),
                            palette: p,
                        })
                        .into()
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(28.0))
                    .w(px(32.0))
                    .rounded(px(radius::MD))
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(p.border))
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_click(cx.listener(|_this, _ev, window, cx| {
                        window.dispatch_action(Box::new(crate::keybinds::ToggleSettingsAction), cx);
                    }))
                    .child(crate::icons::icon(
                        crate::icons::Icon::Settings,
                        p.text_muted,
                        18.0,
                    )),
            )
    }
}
