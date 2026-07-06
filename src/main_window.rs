use gpui::prelude::*;
use gpui::*;

use crate::app_toolbar::AppToolbar;
use crate::document_ops::FormatOp;
use crate::file_explorer::FileExplorer;
use crate::formatting_ribbon::FormattingRibbon;
use crate::keybinds::{
    BoldAction, CiteAction, CiteFromLinkAction, ClearFormattingAction, CondenseAction, CopyAction,
    CutAction, DeleteTagsAction, EmphasisAction, FindAction, FindReplaceAction, HatAction,
    HighlightAction, OpenStatsAction, PasteAction, PasteSmartAction, PocketAction, RedoAction,
    SaveAction, SaveAsAction, SelectAllAction, ShrinkAction, StartTimerAction, TagAction,
    ToggleSettingsAction, ToggleSidebarAction, UndoAction, UnderlineAction, WikifiAction,
    BlockAction,
};
use crate::settings_modal::SettingsModal;
use crate::state::{AppState, CardStyleKind};
use crate::tab_bar::TabBar;
use crate::text_editor::TextEditor;

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
        let settings_modal    = cx.new(|cx|  SettingsModal::new(state.clone(), cx));

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

    fn toggle_settings(&mut self, _: &ToggleSettingsAction, _window: &mut Window, cx: &mut Context<Self>) {
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

    fn toggle_sidebar(&mut self, _: &ToggleSidebarAction, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Flips AppState.sidebar_visible to collapse/expand the file explorer panel.
         */
        self.state.update(cx, |s, cx| {
            s.sidebar_visible = !s.sidebar_visible;
            cx.notify();
        });
        cx.notify();
    }

    pub fn save(&mut self, _: &SaveAction, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Save handler. Delegates to AppState::save_active_tab and logs any error
         * to stderr — a future iteration should surface the error in the UI.
         */
        self.state.update(cx, |s, _cx| {
            if let Err(e) = s.save_active_tab() {
                eprintln!("[save] {}", e);
            }
        });
        cx.notify();
    }

    // ── Configurable keybind actions (src/keybinds.rs) ──────────────────────
    //
    // Every non-vim, non-tab-bar hotkey the user can remap through the
    // settings modal dispatches one of these. Registered on the root div in
    // render() below so they fire regardless of which child view has focus,
    // matching the pattern already established by toggle_settings/
    // toggle_sidebar/save above. Adding a future bindable action means: one
    // enum variant in keybinds.rs, one action struct there, one keybinding
    // arm in `rebuild_keymap`, and one small handler + `.on_action` line here.

    fn save_as(&mut self, _: &SaveAsAction, _window: &mut Window, _cx: &mut Context<Self>) {
        // Stub — no Save As flow exists yet (bindable/remappable regardless,
        // matching this codebase's existing Doc Menu/Card Menu convention).
        println!("[Save As] not yet implemented");
    }

    fn find(&mut self, _: &FindAction, _window: &mut Window, _cx: &mut Context<Self>) {
        println!("[Find] not yet implemented");
    }

    fn find_replace(&mut self, _: &FindReplaceAction, _window: &mut Window, _cx: &mut Context<Self>) {
        println!("[Find & Replace] not yet implemented");
    }

    fn copy(&mut self, _: &CopyAction, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.state.read(cx).copy_selection();
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &CutAction, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.state.update(cx, |state, cx| {
            let result = state.cut_selection();
            if result.is_some() { cx.notify(); }
            result
        });
        if let Some(text) = text {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.notify();
    }

    fn paste(&mut self, _: &PasteAction, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(text) = item.text() {
                self.state.update(cx, |state, cx| {
                    state.insert_str(&text);
                    cx.notify();
                });
            }
        }
        cx.notify();
    }

    fn undo(&mut self, _: &UndoAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.undo());
        cx.notify();
    }

    fn redo(&mut self, _: &RedoAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.redo());
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAllAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.select_all());
        cx.notify();
    }

    fn bold(&mut self, _: &BoldAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_formatting_to_selection(FormatOp::Bold(true)));
        cx.notify();
    }

    fn underline(&mut self, _: &UnderlineAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_formatting_to_selection(FormatOp::Underline(true)));
        cx.notify();
    }

    fn shrink(&mut self, _: &ShrinkAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.shrink_text());
        cx.notify();
    }

    fn clear_formatting(&mut self, _: &ClearFormattingAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_formatting_to_line(FormatOp::ClearAll));
        cx.notify();
    }

    fn paste_smart(&mut self, _: &PasteSmartAction, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(text) = item.text() {
                self.state.update(cx, |state, _cx| state.paste_text(&text));
            }
        }
        cx.notify();
    }

    fn condense(&mut self, _: &CondenseAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.condense_selection());
        cx.notify();
    }

    fn pocket(&mut self, _: &PocketAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_card_style(CardStyleKind::Pocket));
        cx.notify();
    }

    fn hat(&mut self, _: &HatAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_card_style(CardStyleKind::Hat));
        cx.notify();
    }

    fn block(&mut self, _: &BlockAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_card_style(CardStyleKind::Block));
        cx.notify();
    }

    fn tag(&mut self, _: &TagAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_card_style(CardStyleKind::Tag));
        cx.notify();
    }

    fn cite(&mut self, _: &CiteAction, _window: &mut Window, cx: &mut Context<Self>) {
        // Cite applies to the current selection only, not the whole line
        // (matching the ribbon's Cite button — see formatting_ribbon.rs).
        self.state.update(cx, |state, _cx| state.apply_formatting_to_selection(FormatOp::Bold(true)));
        cx.notify();
    }

    fn emphasis(&mut self, _: &EmphasisAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| state.apply_formatting_to_selection(FormatOp::Bold(true)));
        cx.notify();
    }

    fn highlight(&mut self, _: &HighlightAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| {
            state.apply_formatting_to_selection(FormatOp::Highlight(Some("yellow".to_string())));
        });
        cx.notify();
    }

    fn delete_tags(&mut self, _: &DeleteTagsAction, _window: &mut Window, _cx: &mut Context<Self>) {
        println!("[Delete Tags] not yet implemented");
    }

    fn start_timer(&mut self, _: &StartTimerAction, _window: &mut Window, _cx: &mut Context<Self>) {
        println!("[Start Timer] not yet implemented");
    }

    fn open_stats(&mut self, _: &OpenStatsAction, _window: &mut Window, _cx: &mut Context<Self>) {
        println!("[Open Stats] not yet implemented");
    }

    fn cite_from_link(&mut self, _: &CiteFromLinkAction, _window: &mut Window, _cx: &mut Context<Self>) {
        println!("[Cite From Link] not yet implemented");
    }

    fn wikifi(&mut self, _: &WikifiAction, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| {
            match state.wikify_current_tab() {
                Ok(_) => println!("Document exported to markdown"),
                Err(e) => println!("Export failed: {}", e),
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
            // Baseline key context so the dispatch path always has *some*
            // KeyContext tag on it, regardless of what currently has focus
            // (or whether anything does). Without this, GPUI's context-
            // predicate evaluator (`KeyBindingContextPredicate::eval_inner`)
            // treats a completely empty context stack as an automatic
            // non-match for *every* predicate — including negations like
            // keybinds::NOT_CAPTURING ("!KeybindCapturing") — since it
            // short-circuits to `false` before ever looking at the
            // predicate itself. TextEditor is the only other view that sets
            // a key_context ("TextEditor"), so every configured keybind
            // (Ctrl+, included) silently stopped matching the moment focus
            // was anywhere else (sidebar, ribbon, or nothing at all).
            .key_context("App")
            // Wire up global action handlers for this window
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::find_replace))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::bold))
            .on_action(cx.listener(Self::underline))
            .on_action(cx.listener(Self::shrink))
            .on_action(cx.listener(Self::clear_formatting))
            .on_action(cx.listener(Self::paste_smart))
            .on_action(cx.listener(Self::condense))
            .on_action(cx.listener(Self::pocket))
            .on_action(cx.listener(Self::hat))
            .on_action(cx.listener(Self::block))
            .on_action(cx.listener(Self::tag))
            .on_action(cx.listener(Self::cite))
            .on_action(cx.listener(Self::emphasis))
            .on_action(cx.listener(Self::highlight))
            .on_action(cx.listener(Self::delete_tags))
            .on_action(cx.listener(Self::start_timer))
            .on_action(cx.listener(Self::open_stats))
            .on_action(cx.listener(Self::cite_from_link))
            .on_action(cx.listener(Self::wikifi))
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
