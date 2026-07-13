use gpui::prelude::*;
use gpui::*;

use crate::app_toolbar::AppToolbar;
use crate::document_ops::FormatOp;
use crate::file_explorer::{FileExplorer, SidebarResizePayload};
use crate::formatting_ribbon::FormattingRibbon;
use crate::keybinds::{
    BlockAction, BoldAction, CiteAction, CiteFromLinkAction, ClearFormattingAction, CloseTabAction,
    CondenseAction, CopyAction, CutAction, DeleteTagsAction, EmphasisAction, FindAction,
    FindReplaceAction, HatAction, HighlightAction, NewTabAction, OpenStatsAction, PasteAction,
    PasteSmartAction, PocketAction, RedoAction, SaveAction, SaveAsAction, SelectAllAction,
    ShrinkAction, StartTimerAction, TagAction, ToggleSettingsAction, ToggleSidebarAction,
    UndoAction, UnderlineAction, WikifiAction, ZoomInAction, ZoomOutAction, ZoomResetAction,
};
use crate::settings_modal::SettingsModal;
use crate::state::{clamp_sidebar_width, AppState, CardStyleKind};
use crate::tab_bar::TabBar;
use crate::text_editor::TextEditor;
use crate::theme::palette;

/// The root view of the application window.
///
/// Owns all child views and the shared `AppState` model, and composes the
/// full layout. Every configurable, non-vim keybind action (`src/keybinds.rs`)
/// is handled by a closure registered via `App::on_action` in `new()` below
/// — deliberately *not* `.on_action(cx.listener(...))` on a div in
/// `render()`, which was the original approach and turned out to be broken:
/// that form only fires when the specific div it's attached to is on the
/// *currently focused* dispatch path (computed from `Window.focus`), so
/// e.g. Ctrl+, silently did nothing unless the text editor specifically had
/// focus — clicking the sidebar, the ribbon, or nothing at all left no
/// path to this view's div at all. `App::on_action` is registered globally
/// (`App.global_action_listeners`) and fires for a matching action
/// regardless of `dispatch_path`/focus entirely (confirmed against GPUI's
/// own `Window::dispatch_action_on_node_inner` — its "Bubble phase for
/// global actions" block never reads `dispatch_path`), which is what these
/// need: they're meant to work everywhere in the app, not just inside one
/// specific view.
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
         * Global keybind action handlers are also registered here, once, via
         * `cx.on_action` (see the struct's doc comment for why) — each
         * closure captures its own clone of `state`.
         */
        let state = cx.new(|_cx| AppState::new());

        let tab_bar           = cx.new(|_cx| TabBar::new(state.clone()));
        let app_toolbar       = cx.new(|_cx| AppToolbar::new(state.clone()));
        let formatting_ribbon = cx.new(|_cx| FormattingRibbon::new(state.clone()));
        let text_editor       = cx.new(|cx|  TextEditor::new(state.clone(), cx));
        let file_explorer     = cx.new(|_cx| FileExplorer::new(state.clone()));
        let settings_modal    = cx.new(|cx|  SettingsModal::new(state.clone(), cx));

        Self::register_global_actions(state.clone(), cx);

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

    /// Registers one `App::on_action` handler per configurable keybind
    /// action. Takes `&mut App` specifically, not `&mut Context<Self>` —
    /// `Context<T>` has its own, differently-shaped `on_action` (window-
    /// scoped, tied to a specific view) that shadows `App::on_action` by
    /// name, so calling this through a `Context<MainWindow>` would silently
    /// resolve to the wrong method. `Context<Self>` derefs to `&mut App`,
    /// so callers just pass their `cx` straight through.
    ///
    /// Adding a future bindable action means: one enum variant in
    /// `keybinds.rs`, one action struct there, one keybinding arm in
    /// `rebuild_keymap`, and one `cx.on_action` call here.
    fn register_global_actions(state: Entity<AppState>, cx: &mut App) {
        let s = state.clone();
        cx.on_action(move |_: &NewTabAction, cx| {
            s.update(cx, |st, cx| { st.new_tab(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &CloseTabAction, cx| {
            let idx = s.read(cx).active_tab;
            s.update(cx, |st, cx| { st.close_tab(idx); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &ToggleSettingsAction, cx| {
            s.update(cx, |st, cx| {
                st.settings_visible = !st.settings_visible;
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &ToggleSidebarAction, cx| {
            s.update(cx, |st, cx| {
                st.sidebar_visible = !st.sidebar_visible;
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &SaveAction, cx| {
            s.update(cx, |st, _cx| {
                if let Err(e) = st.save_active_tab() {
                    eprintln!("[save] {}", e);
                }
            });
        });

        cx.on_action(move |_: &SaveAsAction, _cx| {
            // Stub — no Save As flow exists yet (bindable/remappable
            // regardless, matching this codebase's existing Doc Menu/Card
            // Menu convention).
            println!("[Save As] not yet implemented");
        });

        cx.on_action(move |_: &FindAction, _cx| {
            println!("[Find] not yet implemented");
        });

        cx.on_action(move |_: &FindReplaceAction, _cx| {
            println!("[Find & Replace] not yet implemented");
        });

        let s = state.clone();
        cx.on_action(move |_: &CopyAction, cx| {
            let text = s.read(cx).copy_selection();
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        });

        let s = state.clone();
        cx.on_action(move |_: &CutAction, cx| {
            let text = s.update(cx, |st, cx| {
                let result = st.cut_selection();
                if result.is_some() { cx.notify(); }
                result
            });
            if let Some(text) = text {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        });

        let s = state.clone();
        cx.on_action(move |_: &PasteAction, cx| {
            if let Some(item) = cx.read_from_clipboard() {
                if let Some(text) = item.text() {
                    s.update(cx, |st, cx| {
                        st.insert_str(&text);
                        cx.notify();
                    });
                }
            }
        });

        let s = state.clone();
        cx.on_action(move |_: &UndoAction, cx| {
            s.update(cx, |st, cx| { st.undo(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &RedoAction, cx| {
            s.update(cx, |st, cx| { st.redo(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &SelectAllAction, cx| {
            s.update(cx, |st, cx| { st.select_all(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &BoldAction, cx| {
            s.update(cx, |st, cx| {
                st.apply_formatting_to_selection(FormatOp::Bold(true));
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &UnderlineAction, cx| {
            s.update(cx, |st, cx| {
                st.apply_formatting_to_selection(FormatOp::Underline(true));
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &ShrinkAction, cx| {
            s.update(cx, |st, cx| { st.shrink_text(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &ClearFormattingAction, cx| {
            s.update(cx, |st, cx| {
                let default_size = st.large_size_half_points;
                st.apply_formatting_to_line(FormatOp::ClearAll { default_size });
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &ZoomInAction, cx| {
            s.update(cx, |st, cx| { st.zoom_in(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &ZoomOutAction, cx| {
            s.update(cx, |st, cx| { st.zoom_out(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &ZoomResetAction, cx| {
            s.update(cx, |st, cx| { st.zoom_reset(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &PasteSmartAction, cx| {
            if let Some(item) = cx.read_from_clipboard() {
                if let Some(text) = item.text() {
                    s.update(cx, |st, cx| {
                        st.paste_text(&text);
                        cx.notify();
                    });
                }
            }
        });

        let s = state.clone();
        cx.on_action(move |_: &CondenseAction, cx| {
            s.update(cx, |st, cx| { st.condense_selection(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &PocketAction, cx| {
            s.update(cx, |st, cx| { st.apply_card_style(CardStyleKind::Pocket); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &HatAction, cx| {
            s.update(cx, |st, cx| { st.apply_card_style(CardStyleKind::Hat); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &BlockAction, cx| {
            s.update(cx, |st, cx| { st.apply_card_style(CardStyleKind::Block); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &TagAction, cx| {
            s.update(cx, |st, cx| { st.apply_card_style(CardStyleKind::Tag); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &CiteAction, cx| {
            // Cite applies to the current selection only, not the whole
            // line (matching the ribbon's Cite button — formatting_ribbon.rs).
            s.update(cx, |st, cx| {
                st.apply_formatting_to_selection(FormatOp::Bold(true));
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &EmphasisAction, cx| {
            s.update(cx, |st, cx| {
                st.apply_formatting_to_selection(FormatOp::Bold(true));
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &HighlightAction, cx| {
            s.update(cx, |st, cx| {
                st.apply_formatting_to_selection(FormatOp::Highlight(Some("yellow".to_string())));
                cx.notify();
            });
        });

        cx.on_action(move |_: &DeleteTagsAction, _cx| {
            println!("[Delete Tags] not yet implemented");
        });

        cx.on_action(move |_: &StartTimerAction, _cx| {
            println!("[Start Timer] not yet implemented");
        });

        cx.on_action(move |_: &OpenStatsAction, _cx| {
            println!("[Open Stats] not yet implemented");
        });

        cx.on_action(move |_: &CiteFromLinkAction, _cx| {
            println!("[Cite From Link] not yet implemented");
        });

        let s = state.clone();
        cx.on_action(move |_: &WikifiAction, cx| {
            s.update(cx, |st, _cx| {
                match st.wikify_current_tab() {
                    Ok(_) => println!("Document exported to markdown"),
                    Err(e) => println!("Export failed: {}", e),
                }
            });
        });
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
        let theme = self.state.read(cx).theme;
        let p = palette(theme);

        let ctx_menu_state = self.state.clone();
        let resize_state = self.state.clone();
        div()
            // Closes the file explorer's right-click menu (found_bugs.md)
            // on any left-click elsewhere in the app — its own rows call
            // `cx.stop_propagation()` so a click inside the menu never
            // reaches here.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.blur();
                ctx_menu_state.update(cx, |s, cx| {
                    if s.file_context_menu.is_some() {
                        s.close_file_context_menu();
                        cx.notify();
                    }
                });
            })
            // Sidebar resize drag (FileExplorer's handle starts it via
            // `.on_drag(SidebarResizePayload, ...)`). Registered on this
            // root, window-spanning div — not on the sidebar itself, whose
            // handle is only 4px wide — so the drag keeps tracking even
            // when the cursor moves faster than the handle's own bounds.
            // Mirrors Zed's own `Workspace::on_drag_move` dock-resize
            // pattern (`workspace.rs`), the reference this was built from.
            .on_drag_move(move |e: &DragMoveEvent<SidebarResizePayload>, _window, cx| {
                let new_width = clamp_sidebar_width(e.event.position.x.as_f32());
                resize_state.update(cx, |s, cx| {
                    s.sidebar_width = new_width;
                    cx.notify();
                });
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.app_bg))
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
