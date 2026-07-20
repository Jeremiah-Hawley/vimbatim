use gpui::prelude::*;
use gpui::*;

use crate::state::{AppState, PendingClose};
use crate::theme::{palette, Palette};

/// Save/Discard/Cancel confirmation for closing a dirty tab or the whole
/// app, shown whenever `AppState.pending_close` is `Some` (set by
/// `AppState::request_close_tab`/`request_close_app`).
///
/// Mirrors `settings_modal.rs`'s backdrop+centred-panel convention: this
/// view is always fully constructed, and `MainWindow` conditionally mounts
/// it (`.when(pending_close.is_some(), ...)`) rather than this view hiding
/// itself — so `render()` can assume `pending_close` is `Some` (the `None`
/// branch below is a defensive fallback, not an expected path).
pub struct CloseConfirm {
    state: Entity<AppState>,
}

impl CloseConfirm {
    pub fn new(state: Entity<AppState>) -> Self {
        CloseConfirm { state }
    }
}

impl Render for CloseConfirm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let p = palette(state.theme);

        let Some(pending) = state.pending_close else {
            return div();
        };

        let message = match pending {
            PendingClose::Tab(idx) => match state.tabs.get(idx) {
                Some(tab) => format!("Save changes to \u{201c}{}\u{201d} before closing?", tab.title),
                None => "Save changes before closing?".to_string(),
            },
            PendingClose::App => "Save changes before quitting?".to_string(),
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(black().opacity(0.55))
            // Backdrop click cancels, same as settings_modal.rs's backdrop.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, _window, cx| {
                this.state.update(cx, |s, cx| { s.cancel_close(); cx.notify(); });
                cx.notify();
            }))
            .child(
                div()
                    .id("close-confirm-panel")
                    // Stops the backdrop's cancel handler above from firing
                    // for clicks inside the panel itself — see
                    // settings_modal.rs's identical guard for why a no-op
                    // handler alone isn't enough (mouse events keep
                    // bubbling unless something calls stop_propagation).
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .p(px(20.0))
                    .w(px(360.0))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .bg(rgb(p.editor_bg_raised))
                    .border_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .text_color(rgb(p.text))
                            .font_weight(FontWeight::BOLD)
                            .child(message),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(close_confirm_button(
                                "close-confirm-cancel",
                                "Cancel",
                                p,
                                cx.listener(|this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| { s.cancel_close(); cx.notify(); });
                                    cx.notify();
                                }),
                            ))
                            .child(close_confirm_button(
                                "close-confirm-discard",
                                "Discard",
                                p,
                                cx.listener(|this, _ev, _window, cx| {
                                    // Read whether this resolves an app-close *before*
                                    // confirm_close_discard consumes pending_close —
                                    // this pure state layer has no way to call
                                    // cx.quit() itself, so the GPUI view here is the
                                    // one place that does it once the state settles.
                                    let was_app =
                                        matches!(this.state.read(cx).pending_close, Some(PendingClose::App));
                                    this.state.update(cx, |s, cx| { s.confirm_close_discard(); cx.notify(); });
                                    cx.notify();
                                    if was_app {
                                        cx.quit();
                                    }
                                }),
                            ))
                            .child(close_confirm_button(
                                "close-confirm-save",
                                "Save",
                                p,
                                cx.listener(|this, _ev, _window, cx| {
                                    let was_app =
                                        matches!(this.state.read(cx).pending_close, Some(PendingClose::App));
                                    // confirm_close_save reports whether everything
                                    // it touched actually persisted (a tab with no
                                    // file_path — no "Save As" flow to fall back to
                                    // — can't be saved, and is left open/dirty
                                    // rather than silently discarded). Only quit
                                    // when that's true; otherwise the dialog closes
                                    // but the app stays up with the dirty tab(s)
                                    // still open.
                                    let persisted = this.state.update(cx, |s, cx| {
                                        let persisted = s.confirm_close_save();
                                        cx.notify();
                                        persisted
                                    });
                                    cx.notify();
                                    if was_app && persisted {
                                        cx.quit();
                                    }
                                }),
                            )),
                    ),
            )
    }
}

/// One panel button. `on_click` is what `cx.listener(...)` produces at the
/// call site (`impl Fn(&ClickEvent, &mut Window, &mut App) + 'static`, i.e.
/// `Div::on_click`'s own parameter type) — not the raw
/// `Fn(&mut CloseConfirm, ...)` closure passed *into* `cx.listener`.
fn close_confirm_button(
    id: &'static str,
    label: &'static str,
    p: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(14.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .text_sm()
        .cursor_pointer()
        .bg(rgb(p.accent_muted))
        .text_color(rgb(p.text))
        .hover(move |s| s.bg(rgb(p.accent)))
        .on_click(on_click)
        .child(label)
}
