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
}

impl TabBar {
    pub fn new(state: Entity<AppState>) -> Self {
        /*
         * Constructs a TabBar backed by the shared AppState entity. All tab data
         * lives in AppState so the bar is purely a rendering layer.
         */
        TabBar { state }
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
        let p = palette(state.theme);
        let tabs = state.tabs.clone();
        let active_idx = state.active_tab;
        let _ = state;

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
                    // Click tab body → switch to this tab (fires only when not dragging).
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.state.update(cx, |s, cx| {
                            s.set_active_tab(idx);
                            cx.notify();
                        });
                        cx.notify();
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
                    // Tab title label. `.truncate()` (overflow_hidden +
                    // whitespace_nowrap + text_ellipsis) clips the back of a
                    // long name with "…" instead of wrapping it onto a
                    // second line.
                    .child(div().min_w_0().flex_1().truncate().text_sm().text_color(tab_text).child(title))
                    // Close button (×) — stop_propagation prevents the click from
                    // bubbling to the parent tab div's on_click (set_active_tab).
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
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                cx.stop_propagation();
                                this.state.update(cx, |s, cx| {
                                    s.close_tab(idx);
                                    cx.notify();
                                });
                                cx.notify();
                            }))
                            .child("×"),
                    )
            })
            .collect();

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
            .on_click(cx.listener(|this, _ev, _window, cx| {
                this.state.update(cx, |s, cx| {
                    s.new_tab();
                    cx.notify();
                });
                cx.notify();
            }))
            .child("+");

        // Invisible spacer that fills remaining width. On mouse-down we call
        // start_window_move() directly because Linux (X11 + Wayland) implements
        // on_hit_test_window_control as a no-op, so WindowControlArea::Drag never fires.
        let drag_region =
            div()
                .flex_1()
                .h_full()
                .on_mouse_down(MouseButton::Left, |_ev, window, _cx| {
                    window.start_window_move();
                });

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
            .on_click(|_ev, window, _cx| {
                window.zoom_window();
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
            .on_click(|_ev, _window, cx| {
                cx.quit();
            })
            .child("×");

        bar.child(tab_scroll_area)
            .child(new_btn_fixed)
            .child(drag_region)
            .child(minimize_btn)
            .child(maximize_btn)
            .child(close_btn)
    }
}
