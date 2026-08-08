use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;
use crate::theme::{color, palette, radius, space};

/// Drag payload for tab reordering. Carries the source tab index and title.
/// Implements `Render` because GPUI uses the payload value as the ghost view
/// that floats under the cursor while dragging.
#[derive(Clone)]
struct TabDragPayload {
    from_idx: usize,
    title: String,
    /// Cursor offset within the dragged tab at the moment drag started.
    /// Used to position the ghost so it doesn't jump away from the cursor.
    offset: Point<Pixels>,
}

impl Render for TabDragPayload {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Render at the cursor offset so the ghost tracks the mouse naturally.
        div().pl(self.offset.x).pt(self.offset.y).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(36.0))
                .px(px(space::MD))
                .bg(rgb(color::EDITOR_BG))
                .text_sm()
                .text_color(rgb(color::TEXT))
                .border_1()
                .border_color(rgb(color::ACCENT))
                .shadow_md()
                .child(self.title.clone()),
        )
    }
}

/// The tab bar rendered at the top of the window.
///
/// Shows one styled button per open tab, a "+" new-tab button immediately after
/// the last tab, an empty drag region, and an "×" close-app button on the far right.
pub struct TabBar {
    state: Entity<AppState>,
    /// Id of the tab currently being renamed via double-click, if any.
    /// `None` means every tab renders its plain (non-editable) title.
    renaming_tab_id: Option<usize>,
    /// The in-progress new title while `renaming_tab_id` is `Some` — not
    /// written back to `AppState` until Enter commits it (`Escape` discards
    /// it instead).
    rename_buffer: String,
    /// Claims keyboard focus for the inline rename input so `on_key_down`
    /// actually receives keystrokes — mirrors `settings_modal.rs`'s
    /// `focus_handle`/`track_focus`/`on_key_down` capture pattern, the only
    /// working precedent for a focus-driven text-capture input in this
    /// codebase (see `handle_rename_key`).
    rename_focus: FocusHandle,
    /// Open state of the tab bar's right-click menu, `None` when closed.
    ///
    /// View state, not `AppState` — unlike the file explorer's menu nothing
    /// outside this bar needs to know it's open, and its own
    /// `on_mouse_down_out` dismisses it, so `main_window.rs`'s root handler
    /// stays untouched.
    context_menu: Option<TabContextMenu>,
}

/// What a tab-bar right-click landed on.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TabContextTarget {
    /// A tab, held by its stable `Tab.id` rather than an index: the "Save As"
    /// item awaits a native file dialog, during which tabs can be reordered
    /// or closed out from under a stored index.
    Tab(usize),
    /// The "+" button — only "Open Last Closed Tab" applies.
    NewTabButton,
}

/// Position (window-relative pixels, matching `MouseDownEvent.position`) and
/// target of the open right-click menu.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TabContextMenu {
    position: (f32, f32),
    target: TabContextTarget,
}

impl TabBar {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        /*
         * Constructs a TabBar backed by the shared AppState entity. All tab data
         * lives in AppState so the bar is purely a rendering layer. Takes `cx`
         * (matching `FormattingRibbon::new`'s and `SettingsModal::new`'s existing
         * precedent) so it can mint its own `rename_focus` handle up front.
         */
        TabBar {
            state,
            renaming_tab_id: None,
            rename_buffer: String::new(),
            rename_focus: cx.focus_handle(),
            context_menu: None,
        }
    }

    /// One clickable row of the right-click menu.
    fn menu_item(
        id: &'static str,
        label: &'static str,
        p: crate::theme::Palette,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(26.0))
            .px(px(space::SM))
            .flex()
            .items_center()
            .cursor_pointer()
            .text_sm()
            .text_color(rgb(p.text))
            .hover(move |s| s.bg(rgb(p.chrome_hover)))
            .on_click(on_click)
            .child(label)
    }

    /// The tab bar's right-click menu.
    ///
    /// Rendered by `render` as a sibling of the whole bar, deliberately *not*
    /// inside `tab_scroll_area` — that container is `overflow_x_scroll`, which
    /// would clip the panel.
    ///
    /// Save/Save As take a tab index directly (`save_tab`/`save_tab_as`)
    /// rather than switching tabs and dispatching the global actions: the
    /// Save As action resolves `active_tab` *after* awaiting the file dialog,
    /// so a tab switch while the dialog is open would save the wrong
    /// document. The id→index lookup happens after the await here for the
    /// same reason.
    fn render_context_menu(
        &self,
        menu: TabContextMenu,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (x, y) = menu.position;
        let state = self.state.clone();

        let panel = div()
            .flex()
            .flex_col()
            .w(px(190.0))
            .bg(rgb(p.chrome))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(px(radius::MD))
            .shadow_lg()
            .py(px(space::XXS))
            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
            .map(|d| match menu.target {
                TabContextTarget::NewTabButton => d.child(Self::menu_item(
                    "tab-ctx-reopen",
                    "Open Last Closed Tab",
                    p,
                    cx.listener(|this, _ev, _window, cx| {
                        this.context_menu = None;
                        this.state.update(cx, |s, cx| {
                            s.reopen_closed_tab();
                            cx.notify();
                        });
                        cx.notify();
                    }),
                )),
                TabContextTarget::Tab(tab_id) => {
                    let save_as_state = state.clone();
                    d.child(Self::menu_item("tab-ctx-save", "Save", p, cx.listener(move |this, _ev, _window, cx| {
                        this.context_menu = None;
                        this.state.update(cx, |s, cx| {
                            if let Some(idx) = s.tabs.iter().position(|t| t.id == tab_id) {
                                if let Err(e) = s.save_tab(idx) {
                                    crate::state::log_line(&format!("[save] {e}"));
                                }
                            }
                            cx.notify();
                        });
                        cx.notify();
                    })))
                    .child(Self::menu_item("tab-ctx-save-as", "Save As", p, {
                        let s = save_as_state.clone();
                        cx.listener(move |this, _ev, _window, cx| {
                            this.context_menu = None;
                            cx.notify();
                            // Seed the dialog from *this* tab's own folder and
                            // name, the same way the toolbar's Save As seeds
                            // from the active one.
                            let (dir, suggested) = {
                                let st = s.read(cx);
                                let tab = st.tabs.iter().find(|t| t.id == tab_id);
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
                            let s = s.clone();
                            cx.spawn(async move |_this, cx| {
                                let Ok(Ok(Some(path))) = path_rx.await else {
                                    return; // cancelled, or no picker available
                                };
                                let _ = s.update(cx, |st, cx| {
                                    // Resolved *after* the await: tabs can be
                                    // reordered or closed while the dialog is up.
                                    if let Some(idx) = st.tabs.iter().position(|t| t.id == tab_id) {
                                        if let Err(e) = st.save_tab_as(idx, path) {
                                            crate::state::log_line(&format!("[save as] {e}"));
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                            .detach();
                        })
                    }))
                    .child(div().h(px(1.0)).my(px(space::XXS)).bg(rgb(p.border_subtle)))
                    .child(Self::menu_item("tab-ctx-close", "Close", p, cx.listener(move |this, _ev, _window, cx| {
                        this.context_menu = None;
                        this.state.update(cx, |s, cx| {
                            // Through `request_close_tab`, so a dirty tab gets
                            // the same Save/Discard/Cancel dialog the × does.
                            if let Some(idx) = s.tabs.iter().position(|t| t.id == tab_id) {
                                s.request_close_tab(idx);
                            }
                            cx.notify();
                        });
                        cx.notify();
                    })))
                    .child(Self::menu_item("tab-ctx-close-left", "Close Tabs to the Left", p, cx.listener(move |this, _ev, _window, cx| {
                        this.context_menu = None;
                        this.state.update(cx, |s, cx| {
                            if let Some(idx) = s.tabs.iter().position(|t| t.id == tab_id) {
                                s.close_tabs_to_left(idx);
                            }
                            cx.notify();
                        });
                        cx.notify();
                    })))
                    .child(Self::menu_item("tab-ctx-close-right", "Close Tabs to the Right", p, cx.listener(move |this, _ev, _window, cx| {
                        this.context_menu = None;
                        this.state.update(cx, |s, cx| {
                            if let Some(idx) = s.tabs.iter().position(|t| t.id == tab_id) {
                                s.close_tabs_to_right(idx);
                            }
                            cx.notify();
                        });
                        cx.notify();
                    })))
                }
            })
            .into_any_element();

        deferred(
            anchored().position(point(px(x), px(y))).snap_to_window().child(
                div()
                    .id("tab-context-menu-dismiss")
                    .on_mouse_down_out(cx.listener(|this, _ev: &MouseDownEvent, _window, cx| {
                        if this.context_menu.take().is_some() {
                            cx.notify();
                        }
                    }))
                    .child(panel),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// Shared right-click handler for both menu targets — records where the
    /// click landed and what it landed on.
    fn open_context_menu(
        &mut self,
        target: TabContextTarget,
        ev: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        // A right-click abandons any in-progress inline rename, same as
        // switching tabs does.
        self.renaming_tab_id = None;
        self.rename_buffer.clear();
        self.context_menu = Some(TabContextMenu {
            position: (ev.position.x.as_f32(), ev.position.y.as_f32()),
            target,
        });
        cx.notify();
    }

    /// Key handler for the inline rename input, armed via `track_focus` +
    /// `on_key_down` once double-click sets `renaming_tab_id`. Mirrors
    /// `state.rs`'s `capture_vim_line_input` state machine (Escape cancels,
    /// Enter commits, Backspace pops, everything else resolves to a literal
    /// char via `vim_find_target_char` — the same helper vim's own
    /// Command/Search line-input uses, so shifted punctuation etc. behaves
    /// identically here) — reimplemented locally rather than shared since
    /// that state machine lives on `AppState`/per-tab fields, and this
    /// buffer is transient UI state that belongs on `TabBar` itself.
    fn handle_rename_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        match ks.key.as_str() {
            "escape" => {
                self.renaming_tab_id = None;
                self.rename_buffer.clear();
            }
            "enter" => {
                if let Some(id) = self.renaming_tab_id.take() {
                    let title = std::mem::take(&mut self.rename_buffer);
                    self.state.update(cx, |s, cx| {
                        s.rename_tab(id, title);
                        cx.notify();
                    });
                }
            }
            "backspace" => {
                self.rename_buffer.pop();
            }
            _ => {
                if let Some(c) = crate::state::vim_find_target_char(&ks.key, ks.modifiers.shift, ks.key_char.as_deref()) {
                    self.rename_buffer.push(c);
                }
            }
        }
        cx.notify();
    }
}

impl Render for TabBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the full tab bar:
         *
         *   [Tab 0] [Tab 1] … [+]  <── drag region ──>  [—] [□] [×]
         *
         * The drag region is an invisible flex-1 spacer marked as WindowControlArea::Drag
         * so clicking and dragging it moves the window on supported platforms.
         * It shrinks automatically as fixed-width siblings (new-tab, minimize,
         * maximize, close) are added — flexbox reflows the flex_1 spacer
         * rather than letting any fixed-width sibling get covered or overlap
         * another, so the minimize/maximize buttons need no special-case
         * layout code of their own (found_bugs.md's own note to "make sure
         * the two new buttons don't cover" the new-tab button/tab-scroll
         * area is already satisfied by this existing flex arrangement).
         *
         * Tab elements require an `.id()` so GPUI can track hover/click state
         * across frames. We use named_usize IDs to ensure uniqueness.
         */
        let is_maximized = window.is_maximized();
        let state = self.state.read(cx);
        let p = state.current_palette();
        let accent_alt = p.accent_alt;
        let tabs = state.tabs.clone();
        let active_idx = state.active_tab;
        // The secondary pane's tab is marked so it's clear which half of a
        // split a document lives in — it is not `active_tab` unless that pane
        // also has focus.
        let secondary_idx = state.pane_tab_index(crate::state::Pane::Secondary);
        let _ = state;
        let renaming_tab_id = self.renaming_tab_id;
        let rename_buffer = self.rename_buffer.clone();
        let rename_focus = self.rename_focus.clone();

        let bar = div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(36.0))
            .bg(rgb(p.app_bg))
            .border_b_1()
            .border_color(rgb(p.border_subtle));

        let tab_elements: Vec<_> = tabs
            .iter()
            .enumerate()
            .map(|(idx, tab)| {
                let is_active = idx == active_idx;
                let title = if tab.is_modified {
                    format!("● {}", tab.title)
                } else {
                    tab.title.clone()
                };

                let tab_bg = if is_active {
                    rgb(p.editor_bg)
                } else {
                    rgb(p.app_bg)
                };
                let tab_text = if is_active {
                    rgb(p.text)
                } else {
                    rgb(p.text_muted)
                };
                let border = p.border;
                let chrome_hover = p.chrome_hover;
                let chrome_active = p.chrome_active;
                let text = p.text;
                let accent = p.accent;

                // Use stable tab.id (not loop idx) so GPUI doesn't confuse element
                // state when tabs are removed and remaining ones shift positions.
                let tab_id = ElementId::named_usize("tab", tab.id);
                let close_id = ElementId::named_usize("tab-close", tab.id);
                let rename_input_id = ElementId::named_usize("tab-rename-input", tab.id);
                // Raw (un-prefixed) title — `title` above may carry a "● "
                // modified-indicator prefix, which the rename buffer should
                // never start pre-populated with.
                let tab_id_for_rename = tab.id;
                let tab_title_for_rename = tab.title.clone();

                // While this tab is being renamed, swap its title label for
                // a focused, editable input; every other tab keeps the
                // plain (unclickable-for-typing) title div. Both arms are
                // boxed into AnyElement so the if/else can return one type —
                // mirrors settings_modal.rs's `render_action_row` (its
                // `right_side: AnyElement` local), the working precedent for
                // this exact "swap a label for a focus-driven input" shape.
                let is_renaming = renaming_tab_id == Some(tab.id);
                let title_area: AnyElement = if is_renaming {
                    div()
                        .id(rename_input_id)
                        .track_focus(&rename_focus)
                        .on_key_down(cx.listener(Self::handle_rename_key))
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .text_color(tab_text)
                        // Stop the mouse-down from bubbling to the parent tab
                        // div's own on_mouse_down above, which would
                        // otherwise immediately re-trigger set_active_tab or
                        // (on a stray double-click) re-arm renaming.
                        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                        .child(format!("{rename_buffer}\u{258f}")) // trailing "▏" as a simple text-cursor stand-in
                        .into_any_element()
                } else {
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .text_color(tab_text)
                        .child(title.clone())
                        .into_any_element()
                };

                div()
                    .id(tab_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .min_w(px(96.0))
                    .max_w(px(220.0))
                    .px(px(space::MD))
                    .gap(px(space::SM))
                    .bg(tab_bg)
                    .cursor_pointer()
                    .rounded(px(radius::SM))
                    .border_r_1()
                    .border_color(rgb(border))
                    .when(!is_active, move |d| {
                        d.border_b_1()
                            .border_color(rgb(border))
                            .hover(move |s| s.bg(rgb(chrome_hover)).text_color(rgb(text)))
                            .active(move |s| s.bg(rgb(chrome_active)))
                    })
                    .when(is_active, move |d| d.border_t_1().border_color(rgb(accent)))
                    // Tab shown in the split's second pane: a bottom rule in
                    // the alternate accent, so it reads as "open elsewhere"
                    // rather than competing with the active tab's top rule.
                    .when(Some(idx) == secondary_idx, move |d| {
                        d.border_b_2().border_color(rgb(accent_alt))
                    })
                    // Highlight this tab's left edge when a dragged tab hovers over it.
                    .drag_over::<TabDragPayload>(move |style, _, _, _| {
                        style.border_l_2().border_color(rgb(accent))
                    })
                    // Receive a dropped tab — reorder it into this position.
                    .on_drop(
                        cx.listener(move |this, payload: &TabDragPayload, _window, cx| {
                            if payload.from_idx != idx {
                                this.state.update(cx, |s, cx| {
                                    s.move_tab(payload.from_idx, idx);
                                    cx.notify();
                                });
                                cx.notify();
                            }
                        }),
                    )
                    // Click tab body → switch to this tab; double-click (or
                    // more — matches file_explorer.rs's own `>= 2` double-click
                    // threshold) instead arms inline rename mode and claims
                    // keyboard focus for `rename_focus` so `handle_rename_key`
                    // starts receiving keystrokes.
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        if ev.click_count >= 2 {
                            this.renaming_tab_id = Some(tab_id_for_rename);
                            this.rename_buffer = tab_title_for_rename.clone();
                            this.rename_focus.clone().focus(window, cx);
                        } else {
                            // Switching tabs abandons any in-progress rename
                            // on another tab (no separate blur-tracking
                            // machinery — this covers the actual reachable
                            // case, since the rename input's own
                            // stop_propagation keeps clicks inside it from
                            // ever reaching here).
                            this.renaming_tab_id = None;
                            this.rename_buffer.clear();
                            this.state.update(cx, |s, cx| {
                                s.set_active_tab(idx);
                                cx.notify();
                            });
                        }
                        cx.notify();
                    }))
                    // Right-click a tab → save / save as / close / close left
                    // / close right. Carries the tab's stable id, not `idx`.
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_context_menu(TabContextTarget::Tab(tab_id_for_rename), ev, cx);
                    }))
                    // Begin drag — carry the source index and title as payload.
                    // Plain closure (not cx.listener): on_drag constructor signature is
                    // Fn(&T, Point<Pixels>, &mut Window, &mut App) -> Entity<W>, which does
                    // not match cx.listener's output signature.
                    .on_drag(
                        TabDragPayload {
                            from_idx: idx,
                            title: title.clone(),
                            offset: Point::default(),
                        },
                        |payload: &TabDragPayload, offset, _window, cx| {
                            let ghost = TabDragPayload {
                                from_idx: payload.from_idx,
                                title: payload.title.clone(),
                                offset,
                            };
                            cx.new(|_| ghost)
                        },
                    )
                    // Tab title label (or, mid-rename, the inline input
                    // built above). `.truncate()` (overflow_hidden +
                    // whitespace_nowrap + text_ellipsis) clips the back of a
                    // long name with "…" instead of wrapping it onto a
                    // second line.
                    .child(title_area)
                    // Close button (×) — stop_propagation prevents the click from
                    // bubbling to the parent tab div's on_click (set_active_tab).
                    // GPUI's on_click registers its own internal MouseDownEvent
                    // listener that does NOT stop propagation (it ignores cx),
                    // so the mouse-down phase alone would otherwise still reach
                    // the parent tab div's on_mouse_down (switch/rename) before
                    // this on_click's request_close_tab fires. Explicit
                    // on_mouse_down + stop_propagation here closes that gap —
                    // same pattern as the rename-input div above.
                    .child(
                        div()
                            .id(close_id)
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(16.0))
                            .h(px(16.0))
                            .rounded(px(radius::XS))
                            .text_sm()
                            .text_color(rgb(p.text_muted))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                            .active(move |s| s.bg(rgb(p.chrome_active)))
                            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                cx.stop_propagation();
                                this.state.update(cx, |s, cx| {
                                    s.request_close_tab(idx);
                                    cx.notify();
                                });
                                cx.notify();
                            }))
                            .child("×"),
                    )
            })
            .collect();

        // Number of tabs at render time — the insert-before position that
        // means "past the last tab", which is what the two trailing drop
        // targets below pass to `move_tab` (see its own doc comment: refusing
        // this value is what made the last slot unreachable by dragging).
        let tab_count = tabs.len();

        // "+" button sits immediately after the last tab
        let new_btn = div()
            .id("new-tab-btn")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(36.0))
            .text_color(rgb(p.text_muted))
            .cursor_pointer()
            .text_lg()
            .border_r_1()
            .border_color(rgb(p.border))
            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            .active(move |s| s.bg(rgb(p.chrome_active)))
            // Dropping a tab on "+" moves it to the end.
            .drag_over::<TabDragPayload>(move |style, _, _, _| {
                style.border_l_2().border_color(rgb(p.accent))
            })
            .on_drop(cx.listener(move |this, payload: &TabDragPayload, _window, cx| {
                this.state.update(cx, |s, cx| {
                    s.move_tab(payload.from_idx, tab_count);
                    cx.notify();
                });
                cx.notify();
            }))
            .on_click(cx.listener(|this, _ev, _window, cx| {
                this.state.update(cx, |s, cx| {
                    s.new_tab();
                    cx.notify();
                });
                cx.notify();
            }))
            // Right-click "+" → open the most recently closed tab.
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.open_context_menu(TabContextTarget::NewTabButton, ev, cx);
            }))
            .child("+");

        // Invisible spacer that fills remaining width. macOS/Linux drag the window via
        // start_window_move() on mouse-down. On Windows that call is a no-op — dragging
        // instead requires this region to report WindowControlArea::Drag so WM_NCHITTEST
        // returns HTCAPTION, which is also what gives real Aero-snap and
        // double-click-to-maximize. Harmless to set on every platform: macOS/Linux never
        // consult on_hit_test_window_control's result for the drag case, so the
        // start_window_move() path below still does the work there.
        //
        // It doubles as the second "drop a tab here to send it to the end"
        // target (the "+" button is the first). The drop target is a child
        // div rather than this one: `window_control_area(Drag)` makes Windows
        // hit-test the region as HTCAPTION, which is exactly what a drop needs
        // *not* to be swallowed by. A plain, unmarked child covering the same
        // area gives GPUI's own drag-and-drop something ordinary to land on.
        let drag_region = div()
            .flex_1()
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |_ev, window, _cx| {
                window.start_window_move();
            })
            .child(
                div()
                    .id("tab-drop-end")
                    .size_full()
                    .drag_over::<TabDragPayload>(move |style, _, _, _| {
                        style.border_l_2().border_color(rgb(p.accent))
                    })
                    .on_drop(cx.listener(move |this, payload: &TabDragPayload, _window, cx| {
                        this.state.update(cx, |s, cx| {
                            s.move_tab(payload.from_idx, tab_count);
                            cx.notify();
                        });
                        cx.notify();
                    })),
            );

        // Scrollable container for tabs only. min_w_0 lets it shrink so the
        // fixed "+" and "×" buttons are always visible regardless of tab count.
        let tab_scroll_area = div()
            .id("tab-scroll-area")
            .flex()
            .flex_row()
            .h_full()
            .min_w_0()
            .overflow_x_scroll()
            .children(tab_elements);

        // "+" sits outside the scroll area as a flex_none sibling so it is
        // never squeezed or scrolled away when many tabs are open.
        let new_btn_fixed = new_btn.flex_none();

        // Minimize/Maximize (found_bugs.md Forgotten Implicit Feature) —
        // real platform-level window controls, not a fullscreen toggle:
        // `Window::minimize_window`/`zoom_window` call straight through to
        // the platform window (`zoom_window` is GPUI's real maximize/restore
        // toggle, named after macOS's own "zoom" term for it). Styled
        // identically to `close_btn` below for a consistent three-button
        // cluster.
        let minimize_btn = div()
            .id("window-minimize-btn")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(46.0))
            .flex_none()
            .text_color(rgb(p.text_muted))
            .cursor_pointer()
            .text_lg()
            .border_l_1()
            .border_color(rgb(p.border))
            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            .active(move |s| s.bg(rgb(p.chrome_active)))
            .on_click(|_ev, window, _cx| {
                window.minimize_window();
            })
            .child("−");

        // Icon reflects current state: "□" to maximize, "❐" (restore) once
        // already maximized — same convention Windows/most Linux DEs use.
        //
        // GPUI's Windows backend (`zoom()`, gpui_windows/src/window.rs) is
        // unconditionally SW_MAXIMIZE — it never restores, unlike macOS/X11/
        // Wayland's `zoom()`, which genuinely toggle. Marking this div
        // WindowControlArea::Max routes Windows clicks through its native
        // HTMAXBUTTON hit-test path instead (events.rs's
        // handle_nc_mouse_up_msg), which *does* check is_maximized() and
        // toggles correctly — same trick as the drag region above. The
        // on_click below still fires alongside that native path (GPUI
        // doesn't stop propagation for it), so it's gated off on Windows to
        // avoid firing the broken always-maximize on top of the correct
        // native toggle; macOS/Linux never consult the window-control hit
        // test for clicks, so they still need on_click to actually work.
        let maximize_btn = div()
            .id("window-maximize-btn")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(46.0))
            .flex_none()
            .text_color(rgb(p.text_muted))
            .cursor_pointer()
            .text_lg()
            .border_l_1()
            .border_color(rgb(p.border))
            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            .active(move |s| s.bg(rgb(p.chrome_active)))
            .window_control_area(WindowControlArea::Max)
            .on_click(|_ev, window, _cx| {
                if !cfg!(target_os = "windows") {
                    window.zoom_window();
                }
            })
            .child(if is_maximized { "❐" } else { "□" });

        // "×" button on the far right closes the entire application.
        let close_btn = div()
            .id("app-close-btn")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(46.0))
            .flex_none()
            .text_color(rgb(p.text_muted))
            .cursor_pointer()
            .text_lg()
            .border_l_1()
            .border_color(rgb(p.border))
            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
            .active(move |s| s.bg(rgb(p.chrome_active)))
            // Routes through AppState::request_close_app rather than
            // quitting directly, so a dirty tab gets a Save/Discard/Cancel
            // dialog (close_confirm.rs) instead of silently losing edits.
            // request_close_app resolves pending_close back to None on its
            // own (via confirm_close_discard) when nothing is unsaved — that
            // "still None right after the call" is this GPUI layer's own
            // signal to quit immediately, since the pure state layer has no
            // way to call cx.quit() itself.
            .on_click(cx.listener(|this, _ev, _window, cx| {
                let should_quit = this.state.update(cx, |s, cx| {
                    s.request_close_app();
                    cx.notify();
                    s.pending_close.is_none()
                });
                cx.notify();
                if should_quit {
                    cx.quit();
                }
            }))
            .child("×");

        // The context menu is a child of `bar`, not of `tab_scroll_area` —
        // that container is `overflow_x_scroll` and would clip the panel.
        bar.child(tab_scroll_area)
            .child(new_btn_fixed)
            .child(drag_region)
            .child(minimize_btn)
            .child(maximize_btn)
            .child(close_btn)
            .when_some(self.context_menu, |el, menu| {
                el.child(self.render_context_menu(menu, p, cx))
            })
    }
}
