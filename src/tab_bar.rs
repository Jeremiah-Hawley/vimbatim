use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;

// Actions for the tab bar, registered globally in main.rs.
actions!(tab_bar, [NewTab, CloseActiveTab]);

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

    fn handle_new_tab(&mut self, _: &NewTab, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Appends a blank tab and switches to it.
         */
        self.state.update(cx, |s, cx| {
            s.new_tab();
            cx.notify();
        });
        cx.notify();
    }

    fn handle_close_active(&mut self, _: &CloseActiveTab, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Closes the currently active tab. AppState ensures at least one tab survives.
         */
        let idx = self.state.read(cx).active_tab;
        self.state.update(cx, |s, cx| {
            s.close_tab(idx);
            cx.notify();
        });
        cx.notify();
    }
}

impl Render for TabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the full tab bar:
         *
         *   [Tab 0] [Tab 1] … [+]  <── drag region ──>  [×]
         *
         * The drag region is an invisible flex-1 spacer marked as WindowControlArea::Drag
         * so clicking and dragging it moves the window on supported platforms.
         *
         * Tab elements require an `.id()` so GPUI can track hover/click state
         * across frames. We use named_usize IDs to ensure uniqueness.
         */
        let state = self.state.read(cx);
        let tabs = state.tabs.clone();
        let active_idx = state.active_tab;
        let _ = state;

        let bar = div()
            .on_action(cx.listener(Self::handle_new_tab))
            .on_action(cx.listener(Self::handle_close_active))
            .flex()
            .flex_row()
            .w_full()
            .h(px(36.0))
            .bg(rgb(0x2d2d2d))
            .border_b_1()
            .border_color(rgb(0x252526));

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

                let tab_bg   = if is_active { rgb(0x1e1e1e) } else { rgb(0x2d2d2d) };
                let tab_text = if is_active { rgb(0xd4d4d4) } else { rgb(0x858585) };

                // Each tab and its close button needs a distinct ID
                let tab_id   = ElementId::named_usize("tab", idx);
                let close_id = ElementId::named_usize("tab-close", idx);

                div()
                    .id(tab_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .px(px(12.0))
                    .gap(px(8.0))
                    .bg(tab_bg)
                    .cursor_pointer()
                    .border_r_1()
                    .border_color(rgb(0x464647))
                    .when(!is_active, |d| d.border_b_1().border_color(rgb(0x464647)))
                    // Click tab body → switch to this tab
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.state.update(cx, |s, cx| {
                            s.set_active_tab(idx);
                            cx.notify();
                        });
                        cx.notify();
                    }))
                    // Tab title label
                    .child(
                        div()
                            .text_sm()
                            .text_color(tab_text)
                            .child(title),
                    )
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
                            .rounded(px(2.0))
                            .text_sm()
                            .text_color(rgb(0x858585))
                            .cursor(CursorStyle::PointingHand)
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
            .text_color(rgb(0x858585))
            .cursor_pointer()
            .text_lg()
            .border_r_1()
            .border_color(rgb(0x464647))
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
        let drag_region = div()
            .flex_1()
            .h_full()
            .on_mouse_down(MouseButton::Left, |_ev, window, _cx| {
                window.start_window_move();
            });

        // "×" button on the far right closes the entire application
        let close_btn = div()
            .id("app-close-btn")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(46.0))
            .text_color(rgb(0x858585))
            .cursor_pointer()
            .text_lg()
            .border_l_1()
            .border_color(rgb(0x464647))
            .on_click(|_ev, _window, cx| {
                cx.quit();
            })
            .child("×");

        bar.children(tab_elements).child(new_btn).child(drag_region).child(close_btn)
    }
}
