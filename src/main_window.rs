use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::*;

use crate::app_toolbar::AppToolbar;
use crate::close_confirm::CloseConfirm;
use crate::docx_parser::{DocxOrigin, Paragraph};
use crate::document_ops::FormatOp;
use crate::file_explorer::{FileExplorer, SidebarResizePayload};
use crate::find_bar::FindBarView;
use crate::formatting_ribbon::FormattingRibbon;
use crate::keybinds::{
    BlockAction, BoldAction, CiteAction, CiteFromLinkAction, ClearFormattingAction, CloseTabAction,
    AnalyticAction, CondenseAction, CopyAction, CutAction, DeleteTagsAction, EmphasisAction,
    FindAction,
    FindReplaceAction, HatAction, HighlightAction, NewTabAction, NextTabAction, OpenStatsAction, PasteAction,
    PasteSmartAction, PasteWithoutFormattingAction, PocketAction, PrevTabAction, RedoAction, SaveAction, SaveAsAction, SelectAllAction, SelectSimilarFormattingAction,
    ShrinkAction, StartTimerAction, TagAction, ToggleSettingsAction, ToggleSidebarAction,
    UndoAction, UnderlineAction, WikifiAction, ZoomInAction, ZoomOutAction, ZoomResetAction,
};
use crate::recovery_prompt::RecoveryPrompt;
use crate::settings_modal::SettingsModal;
use crate::state::{clamp_sidebar_width, clamp_split_ratio, AppState, CardStyleKind};
use crate::tab_bar::TabBar;
use crate::text_editor::TextEditor;
use crate::theme::palette;
use crate::timer::Timer;
use crate::word_count::WordCount;

/// Carried by the split-view divider's drag, and picked up by this window's
/// `on_drag_move` handler.
///
/// A distinct type rather than a flag on the sidebar's payload: GPUI
/// discriminates `on_drag_move` listeners *by payload type*, which is the whole
/// reason `SidebarResizePayload` is its own struct. Two drags, two types, no
/// interference.
#[derive(Clone)]
pub struct SplitResizePayload;

impl Render for SplitResizePayload {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

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
    pub state: Entity<AppState>,
    tab_bar: Entity<TabBar>,
    app_toolbar: Entity<AppToolbar>,
    formatting_ribbon: Entity<FormattingRibbon>,
    text_editor: Entity<TextEditor>,
    text_editor_secondary: Entity<TextEditor>,
    file_explorer: Entity<FileExplorer>,
    find_bar: Entity<FindBarView>,
    settings_modal: Entity<SettingsModal>,
    close_confirm: Entity<CloseConfirm>,
    recovery_prompt: Entity<RecoveryPrompt>,
    word_count: Entity<WordCount>,
    timer: Entity<Timer>,
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

        let tab_bar           = cx.new(|cx| TabBar::new(state.clone(), cx));
        let app_toolbar       = cx.new(|_cx| AppToolbar::new(state.clone()));
        let formatting_ribbon = cx.new(|cx| FormattingRibbon::new(state.clone(), cx));
        let text_editor       = cx.new(|cx|  TextEditor::new(state.clone(), cx));
        // Constructed always, mounted only while the split is open. Each pane
        // needs its own scroll handle, row cache, spell cache and focus handle
        // — sharing one editor would make the panes fight over all four.
        let text_editor_secondary =
            cx.new(|cx| TextEditor::for_pane(state.clone(), crate::state::Pane::Secondary, cx));
        let file_explorer     = cx.new(|_cx| FileExplorer::new(state.clone()));
        let find_bar          = cx.new(|cx|  FindBarView::new(state.clone(), cx));
        let settings_modal    = cx.new(|cx|  SettingsModal::new(state.clone(), cx));
        let close_confirm     = cx.new(|_cx| CloseConfirm::new(state.clone()));
        let recovery_prompt   = cx.new(|_cx| RecoveryPrompt::new(state.clone()));
        let word_count        = cx.new(|_cx| WordCount::new(state.clone()));
        let timer             = cx.new(|cx|  Timer::new(state.clone(), cx));

        // ── Crash-recovery snapshots ────────────────────────────────────
        // One task for the whole app, not one per tab: it wakes on a fixed
        // 1s tick and asks each tab whether it is due, using that tab's own
        // interval (derived from what its last snapshot actually cost). So
        // a cheap tab and an expensive one in the same window snapshot at
        // 3s and 30s off the same tick.
        //
        // The write itself runs on the background executor so a slow zip
        // never blocks the tick, and never blocks the UI thread.
        //
        // Held as a *weak* handle: this task must not be the thing keeping
        // AppState alive. When the window (and with it `state`) is dropped,
        // `read_with`/`update` start returning `Err` and the loop exits
        // instead of spinning forever on a dead entity.
        let snapshot_state = state.downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;

                // Collect the work under one short read, then release it
                // before doing any (potentially slow) I/O. Keyed by the
                // tab's stable `id` rather than its index into `tabs`: a tab
                // can close while an earlier entry in this same batch is
                // being written (each write below is awaited in turn), which
                // would shift every later index — the id is immune to that.
                let due: Vec<(usize, u64, Vec<Paragraph>, Option<Arc<DocxOrigin>>, Option<PathBuf>, String)> =
                    match snapshot_state.read_with(cx, |s, _| {
                        let now = Instant::now();
                        s.tabs
                            .iter()
                            .filter(|t| {
                                crate::recovery::needs_snapshot(
                                    t.is_modified,
                                    t.content_version,
                                    t.last_snapshot_version,
                                    t.last_edit_at,
                                    now,
                                    crate::recovery::snapshot_interval(t.last_snapshot_cost),
                                )
                            })
                            .map(|t| {
                                (
                                    t.id,
                                    t.content_version,
                                    t.paragraphs.clone(),
                                    t.docx_origin.clone(),
                                    t.file_path.clone(),
                                    t.title.clone(),
                                )
                            })
                            .collect()
                    }) {
                        Ok(due) => due,
                        // The AppState entity is gone: the app is shutting
                        // down, so stop ticking.
                        Err(_) => return,
                    };

                // Keep the panic hook's view of the dirty tabs current. Cheap
                // relative to the snapshot writes below, and it must be fresh
                // at the instant of a panic, which we cannot predict.
                if let Ok(mirror) = snapshot_state.read_with(cx, |s, _| s.dirty_tab_snapshots()) {
                    if let Some(slot) = crate::recovery::PANIC_SNAPSHOT.get() {
                        if let Ok(mut guard) = slot.lock() {
                            *guard = mirror;
                        }
                    }
                }

                for (tab_id, version, paragraphs, origin, path, title) in due {
                    // `version` is the content_version captured above, before
                    // the write — not re-read afterwards, or an edit landing
                    // mid-write would be silently marked as snapshotted.
                    let cost = cx
                        .background_executor()
                        .spawn(async move {
                            crate::recovery::write_snapshot(
                                tab_id,
                                &paragraphs,
                                origin.as_deref(),
                                path.as_deref(),
                                &title,
                            )
                        })
                        .await;

                    // Only record success. A failed write leaves
                    // last_snapshot_version untouched so the next tick
                    // retries — snapshotting is best-effort and never
                    // surfaces an error to the user mid-edit.
                    if let Ok(cost) = cost {
                        let _ = snapshot_state.update(cx, |s, _| {
                            // Look up by id, not index: the tab may have
                            // moved, been saved, or closed while this write
                            // was in flight.
                            match s.tabs.iter_mut().find(|t| t.id == tab_id) {
                                Some(tab) if tab.is_modified => {
                                    tab.last_snapshot_version = version;
                                    tab.last_snapshot_cost = Some(cost);
                                }
                                /*
                                 * Saved (Ctrl+S) or closed during the await.
                                 * Both call `delete_snapshot` on the
                                 * foreground — but they ran before the write
                                 * landed, so they deleted files that did not
                                 * exist yet and the write has just recreated
                                 * them. Nothing else would ever remove them:
                                 * a clean tab is never due again, and a
                                 * closed tab's id is swept by no quit path,
                                 * so the next launch would prompt to recover
                                 * work the user had already saved or
                                 * deliberately discarded. Delete here, where
                                 * both cases converge.
                                 */
                                _ => crate::recovery::delete_snapshot(tab_id),
                            }
                        });
                    }
                }
            }
        })
        .detach();

        Self::register_global_actions(state.clone(), cx);

        MainWindow {
            state,
            tab_bar,
            app_toolbar,
            formatting_ribbon,
            text_editor,
            text_editor_secondary,
            file_explorer,
            find_bar,
            settings_modal,
            close_confirm,
            recovery_prompt,
            word_count,
            timer,
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
            // Routes through request_close_tab (not close_tab directly) so
            // the Ctrl+W keybind gets the same Save/Discard/Cancel dialog
            // as the tab bar's × button (tab_bar.rs) when the tab is dirty
            // — otherwise this keybind would be a silent-discard backdoor
            // around the whole point of this confirmation flow.
            let idx = s.read(cx).active_tab;
            s.update(cx, |st, cx| { st.request_close_tab(idx); cx.notify(); });
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

        let s = state.clone();
        cx.on_action(move |_: &SaveAsAction, cx| {
            // Native save dialog (gpui's `prompt_for_new_path`), seeded with
            // the tab's own folder and filename when it has one so "Save As"
            // on an opened document starts beside the original rather than at
            // some unrelated default.
            let (dir, suggested) = {
                let st = s.read(cx);
                let tab = st.tabs.get(st.active_tab);
                let dir = tab
                    .and_then(|t| t.file_path.as_ref())
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| st.working_directory.clone());
                let suggested = tab
                    .map(|t| t.title.clone())
                    .filter(|t| t.ends_with(".docx"))
                    .unwrap_or_else(|| "Untitled.docx".to_string());
                (dir, suggested)
            };

            let path_rx = cx.prompt_for_new_path(&dir, Some(&suggested));
            let state = s.clone();
            cx.spawn(async move |cx| {
                let Ok(Ok(Some(path))) = path_rx.await else {
                    return; // cancelled, or the platform couldn't open a picker
                };
                let _ = state.update(cx, |st, cx| {
                    if let Err(e) = st.save_active_tab_as(path) {
                        eprintln!("[save as] {}", e);
                    }
                    cx.notify();
                });
            })
            .detach();
        });

        // Ctrl+F and Ctrl+H open the same panel — it always carries both a
        // Find and a Replace field, so there is nothing for the two bindings
        // to differ on beyond which field starts focused.
        let s = state.clone();
        cx.on_action(move |_: &FindAction, cx| {
            s.update(cx, |st, cx| { st.open_find_bar(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &FindReplaceAction, cx| {
            s.update(cx, |st, cx| {
                st.open_find_bar();
                if let Some(bar) = st.find_bar.as_mut() {
                    bar.focus = crate::state::FindField::Replace;
                }
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &CopyAction, cx| {
            let state = s.read(cx);
            let Some(text) = state.copy_selection() else { return };
            let runs = state.copy_selection_runs().unwrap_or_default();
            let paras = state.copy_selection_paragraph_attrs().unwrap_or_default();
            let metadata = crate::rich_clipboard::encode_with_lengths(&runs, &paras);
            cx.write_to_clipboard(ClipboardItem::new_string_with_metadata(text, metadata));
        });

        let s = state.clone();
        cx.on_action(move |_: &CutAction, cx| {
            let runs = s.read(cx).copy_selection_runs().unwrap_or_default();
            let paras = s.read(cx).copy_selection_paragraph_attrs().unwrap_or_default();
            let text = s.update(cx, |st, cx| {
                let result = st.cut_selection();
                if result.is_some() { cx.notify(); }
                result
            });
            if let Some(text) = text {
                let metadata = crate::rich_clipboard::encode_with_lengths(&runs, &paras);
                cx.write_to_clipboard(ClipboardItem::new_string_with_metadata(text, metadata));
            }
        });

        let s = state.clone();
        cx.on_action(move |_: &PasteAction, cx| {
            let Some(item) = cx.read_from_clipboard() else { return };
            let Some(text) = item.text() else { return };
            let rich = item.metadata().and_then(|m| crate::rich_clipboard::decode(m, &text));
            s.update(cx, |st, cx| {
                match rich {
                    Some((runs, paras)) => {
                        st.insert_str_with_runs_and_paragraphs(&text, &runs, &paras)
                    }
                    None => st.insert_str(&text),
                }
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &PasteWithoutFormattingAction, cx| {
            if let Some(item) = cx.read_from_clipboard() {
                if let Some(text) = item.text() {
                    s.update(cx, |st, cx| { st.insert_str(&text); cx.notify(); });
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
        cx.on_action(move |_: &SelectSimilarFormattingAction, cx| {
            s.update(cx, |st, cx| { st.select_similar_formatting(); cx.notify(); });
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
                st.clear_formatting();
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
        cx.on_action(move |_: &NextTabAction, cx| {
            s.update(cx, |st, cx| { st.next_tab(); cx.notify(); });
        });

        let s = state.clone();
        cx.on_action(move |_: &PrevTabAction, cx| {
            s.update(cx, |st, cx| { st.prev_tab(); cx.notify(); });
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
                st.apply_cite_style();
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &AnalyticAction, cx| {
            s.update(cx, |st, cx| {
                st.apply_analytic_style();
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
                // settings.conf's `highlight_color`, not a hardcoded yellow —
                // the setting existed but nothing had ever read it.
                let color = st.highlight_color.clone();
                st.apply_formatting_to_selection(FormatOp::Highlight(Some(color)));
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &DeleteTagsAction, cx| {
            s.update(cx, |st, cx| { st.delete_tags(); cx.notify(); });
        });

        // Opens the timer popup rather than starting the clock directly: Start
        // lives on the panel, and starting an invisible timer is not something
        // anyone can act on.
        let s = state.clone();
        cx.on_action(move |_: &StartTimerAction, cx| {
            s.update(cx, |st, cx| {
                st.timer.visible = !st.timer.visible;
                cx.notify();
            });
        });

        let s = state.clone();
        cx.on_action(move |_: &OpenStatsAction, cx| {
            s.update(cx, |st, cx| {
                st.word_count_visible = !st.word_count_visible;
                cx.notify();
            });
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
        let pending_close    = self.state.read(cx).pending_close;
        let has_recovery     = !self.state.read(cx).pending_recovery.is_empty();
        let word_count_visible = self.state.read(cx).word_count_visible;
        let timer_visible    = self.state.read(cx).timer.visible;
        let split_view       = self.state.read(cx).split_view;
        let split_ratio      = self.state.read(cx).split_ratio;
        let sidebar_width    = self.state.read(cx).sidebar_width;
        let find_bar_visible = self.state.read(cx).find_bar.is_some();
        let theme = self.state.read(cx).theme;
        let theme_mode = self.state.read(cx).theme_mode;
        let p = palette(theme, theme_mode);

        let ctx_menu_state = self.state.clone();
        let resize_state = self.state.clone();
        let split_state = self.state.clone();
        let drag_end_state = self.state.clone();
        // The divider drag reports a window-absolute X; converting it to a
        // ratio needs the window's own width, sampled here rather than inside
        // the handler (which has no view to measure from).
        let window_width = _window.viewport_size().width.as_f32();
        let _ = sidebar_width;
        div()
            // Closes the file explorer's right-click menu (found_bugs.md)
            // on any left-click elsewhere in the app — its own rows call
            // `cx.stop_propagation()` so a click inside the menu never
            // reaches here.
            // Ends a divider drag. `on_mouse_up` fires on release wherever the
            // cursor happens to be, and this root spans the window, so the flag
            // can't be stranded on — which would otherwise leave the document
            // wrapped at a stale width indefinitely.
            .on_mouse_up(MouseButton::Left, {
                let s = drag_end_state.clone();
                move |_ev, _window, cx| {
                    s.update(cx, |st, cx| {
                        if st.split_dragging {
                            st.split_dragging = false;
                            cx.notify();
                        }
                    });
                }
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.blur();
                ctx_menu_state.update(cx, |s, cx| {
                    if s.file_context_menu.is_some() || s.editor_context_menu.is_some() {
                        s.close_file_context_menu();
                        s.editor_context_menu = None;
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
            // Split-view divider. Registered on this same window-spanning
            // root, and for the same reason as the sidebar's: a 4px handle
            // cannot track a cursor moving faster than its own width.
            .on_drag_move(move |e: &DragMoveEvent<SplitResizePayload>, _window, cx| {
                let x = e.event.position.x.as_f32();
                split_state.update(cx, |s, cx| {
                    // Measured against the editor area, which starts after the
                    // sidebar when one is showing.
                    let left = if s.sidebar_visible { s.sidebar_width } else { 0.0 };
                    let width = (window_width - left).max(1.0);
                    let next = clamp_split_ratio((x - left) / width);
                    if s.split_ratio == next && s.split_dragging {
                        return; // no movement worth a repaint
                    }
                    s.split_ratio = next;
                    // Suspends the editors' full-document re-wrap for the
                    // duration of the drag — see `AppState.split_dragging`.
                    s.split_dragging = true;
                    cx.notify();
                });
            })
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
            // Wrapped in its own stacking context so the timer popup can be
            // positioned against the *ribbon* ("in the middle of the ribbon")
            // rather than against the window, with no height constants to keep
            // in sync. `deferred` so it paints over the editor below it — the
            // same trick the ribbon's own dropdown menus use, and the reason it
            // can't simply be a later sibling like the modals are.
            .child(
                div()
                    .relative()
                    .child(self.formatting_ribbon.clone())
                    .when(timer_visible, |d| {
                        d.child(deferred(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .flex()
                                .justify_center()
                                .child(self.timer.clone()),
                        ).with_priority(150))
                    }),
            )
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
                    // The editor and the find bar share a stacking context so
                    // the bar can float over the top-left of the editor area
                    // (spec 4.6) without taking layout space from it — an
                    // inline sibling would reflow the document every time the
                    // bar opened, re-wrapping every line.
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            // Primary pane. `flex_grow` carries the ratio so
                            // the two panes always fill the row exactly, with
                            // no leftover gap to reconcile.
                            .child(
                                div()
                                    .flex()
                                    .flex_grow(if split_view { split_ratio } else { 1.0 })
                                    .flex_basis(px(0.0))
                                    .min_w_0()
                                    .min_h_0()
                                    .child(self.text_editor.clone()),
                            )
                            .when(split_view, |d| {
                                d.child(
                                    div()
                                        .id("split-divider")
                                        .w(px(4.0))
                                        .h_full()
                                        .flex_none()
                                        .bg(rgb(p.border))
                                        .cursor_col_resize()
                                        .occlude()
                                        .on_drag(SplitResizePayload, |payload: &SplitResizePayload, _offset, _window, cx| {
                                            cx.new(|_| payload.clone())
                                        }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_grow(1.0 - split_ratio)
                                        .flex_basis(px(0.0))
                                        .min_w_0()
                                        .min_h_0()
                                        .child(self.text_editor_secondary.clone()),
                                )
                            })
                            .when(find_bar_visible, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(8.0))
                                        .right(px(8.0))
                                        .child(self.find_bar.clone()),
                                )
                            }),
                    )
            )
            // ── Settings modal overlay ─────────────────────────────────────
            // Added last so it paints on top of all other children
            .when(settings_visible, |d| d.child(self.settings_modal.clone()))
            // ── Close-confirm overlay ───────────────────────────────────────
            // Mounted only while a tab-close or app-close is awaiting a
            // Save/Discard/Cancel answer; painted after the settings modal
            // so it's still on top even if somehow both were open at once.
            .when(pending_close.is_some(), |d| d.child(self.close_confirm.clone()))
            // ── Recovery prompt overlay ─────────────────────────────────────
            // Mounted at launch when a previous session left unsaved work
            // behind. Painted last so it sits above both other modals — it
            // must be answered before anything else is usable.
            .when(has_recovery, |d| d.child(self.recovery_prompt.clone()))
            // ── Word count panel ────────────────────────────────────────────
            .when(word_count_visible, |d| d.child(self.word_count.clone()))
    }
}
