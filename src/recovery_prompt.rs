use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;
use crate::theme::{palette, Palette};

/// Launch-time "we recovered unsaved changes" prompt, shown whenever
/// `AppState.pending_recovery` is non-empty (filled by `AppState::new` from
/// `recovery::scan_recovery_dir`).
///
/// Mirrors `close_confirm.rs`'s backdrop+centred-panel convention: this view
/// is always fully constructed and `MainWindow` conditionally mounts it, so
/// `render()` can assume the list is non-empty (the empty branch below is a
/// defensive fallback, not an expected path).
///
/// Entries are shown one at a time rather than as a list: acting on one pops
/// it and the next renders, which reuses a single panel and needs no per-row
/// UI. See the recovery spec.
///
/// Deliberately has no backdrop-click-to-dismiss, unlike close_confirm:
/// dismissing this prompt would silently strand the recovered work, and
/// there is no "ask me later" option. The user must pick one of the three.
pub struct RecoveryPrompt {
    state: Entity<AppState>,
}

impl RecoveryPrompt {
    pub fn new(state: Entity<AppState>) -> Self {
        RecoveryPrompt { state }
    }
}

impl Render for RecoveryPrompt {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let p = palette(state.theme);

        let Some(entry) = state.pending_recovery.first() else {
            return div();
        };
        let title = entry.title.clone();
        let remaining = state.pending_recovery.len();
        let origin_line = match &entry.original_path {
            Some(path) => format!("Was editing: {}", path.display()),
            None => "This document had never been saved.".to_string(),
        };
        let heading = if remaining > 1 {
            format!("Unsaved changes recovered \u{2014} \u{201c}{title}\u{201d} ({remaining} remaining)")
        } else {
            format!("Unsaved changes recovered \u{2014} \u{201c}{title}\u{201d}")
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
            // Swallows backdrop clicks without dismissing — see the struct
            // doc for why this modal has no cancel path.
            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
            .child(
                div()
                    .id("recovery-prompt-panel")
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .p(px(20.0))
                    .w(px(440.0))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .bg(rgb(p.editor_bg_raised))
                    .border_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .text_color(rgb(p.text))
                            .font_weight(FontWeight::BOLD)
                            .child(heading),
                    )
                    .child(div().text_sm().text_color(rgb(p.text)).child(origin_line))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(recovery_button(
                                "recovery-discard",
                                "Discard",
                                p,
                                cx.listener(|this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| { s.discard_recovery(); cx.notify(); });
                                    cx.notify();
                                }),
                            ))
                            .child(recovery_button(
                                "recovery-save-as",
                                "Save As\u{2026}",
                                p,
                                cx.listener(|this, _ev, window, cx| {
                                    // The native picker is async, so the state
                                    // layer hands the entry out and takes it
                                    // back with a destination — a cancelled
                                    // picker leaves the entry in place (this
                                    // closure simply returns without ever
                                    // calling complete_recovery_save_as, so
                                    // pending_recovery is untouched and the
                                    // modal keeps showing the same entry).
                                    let Some(entry) = this.state.update(cx, |s, _| s.take_recovery_for_save_as())
                                    else { return };
                                    // gpui's prompt_for_new_path (unlike
                                    // prompt_for_paths, used elsewhere for
                                    // folder selection) wants a starting
                                    // directory plus a suggested filename —
                                    // reuse the app's own working directory
                                    // rather than inventing a new default.
                                    let directory = this.state.read(cx).working_directory.clone();
                                    let dest_rx = cx.prompt_for_new_path(&directory, Some(&entry.title));
                                    let state = this.state.clone();
                                    cx.spawn_in(window, async move |_this, cx| {
                                        let Ok(Ok(Some(dest))) = dest_rx.await else { return };
                                        let _ = state.update(cx, |s, cx| {
                                            let _ = s.complete_recovery_save_as(&entry, &dest);
                                            cx.notify();
                                        });
                                    })
                                    .detach();
                                }),
                            ))
                            .child(recovery_button(
                                "recovery-resume",
                                "Resume Editing",
                                p,
                                cx.listener(|this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| { s.resume_recovery(); cx.notify(); });
                                    cx.notify();
                                }),
                            )),
                    ),
            )
    }
}

/// One panel button. Same shape as `close_confirm.rs`'s own helper —
/// `on_click` is what `cx.listener(...)` produces at the call site, not the
/// raw closure passed into it.
fn recovery_button(
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
