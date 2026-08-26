use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;
use crate::theme::Palette;

/// "Add Font" popup, opened from the Font Family dropdown's "+ Add Font"
/// row and the Fonts settings section. Mirrors `close_confirm.rs`'s
/// backdrop+centred-panel convention: always constructed, mounted only
/// while `AppState.font_import_modal_open` is true.
///
/// `error`/`warning` are view-local (not on `AppState`) the same way
/// `settings_modal.rs`'s `theme_import_error` is — only this view ever
/// reads or writes them.
pub struct FontImportModal {
    state: Entity<AppState>,
    error: Option<String>,
    /// Set after a *successful* import that's missing a bold and/or italic
    /// face (`font_import::missing_style_warning`) — the font is imported
    /// and usable, this just tells the user up front that Bold/Italic won't
    /// visibly do anything for it, rather than them discovering that later.
    /// Keeps the modal open (unlike a clean success) so the message is
    /// actually seen instead of flashing shut immediately.
    warning: Option<String>,
}

impl FontImportModal {
    pub fn new(state: Entity<AppState>) -> Self {
        FontImportModal { state, error: None, warning: None }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.close_font_import_modal();
            cx.notify();
        });
        cx.notify();
    }

    /// Native open dialog (same `prompt_for_paths` used elsewhere — it
    /// can't filter by extension, so an unsupported pick surfaces as an
    /// inline error instead of being filtered out of the picker), then
    /// `font_import::install_from_path` does the actual parse+register.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths_rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let state = self.state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = paths_rx.await else { return };
            let Some(path) = paths.pop() else { return };
            let result = state.update(cx, |s, cx| {
                let result = crate::font_import::install_from_path(cx, &path);
                cx.notify();
                result
            });
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(outcomes) => {
                        this.error = None;
                        this.warning = crate::font_import::missing_style_warning(&outcomes);
                        // Only auto-close on a clean success — a warning
                        // stays on screen until the user dismisses it, or
                        // this defeats the point of warning them at all.
                        if this.warning.is_none() {
                            this.state.update(cx, |s, cx| {
                                s.close_font_import_modal();
                                cx.notify();
                            });
                        }
                    }
                    Err(e) => {
                        this.error = Some(e);
                        this.warning = None;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for FontImportModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let p = state.current_palette();
        let theme_mode = state.theme_mode;

        if !state.font_import_modal_open {
            return div();
        }

        let alert_color = match theme_mode {
            crate::theme::ThemeMode::Dark => 0xf48771,
            crate::theme::ThemeMode::Light => 0xb02a15,
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
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, _window, cx| this.cancel(cx)))
            .child(
                div()
                    .id("font-import-panel")
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .p(px(20.0))
                    .w(px(380.0))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .bg(rgb(p.editor_bg_raised))
                    .border_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .text_color(rgb(p.text))
                            .font_weight(FontWeight::BOLD)
                            .child("Import Font"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(p.text_muted))
                            .child("Upload a .ttf, .otf, or a .zip containing one."),
                    )
                    .when_some(self.error.clone(), |d, err| {
                        d.child(div().text_sm().text_color(rgb(alert_color)).child(err))
                    })
                    .when_some(self.warning.clone(), |d, warning| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .children(warning.lines().map(|line| {
                                    div()
                                        .text_sm()
                                        .text_color(rgb(alert_color))
                                        .child(line.to_string())
                                        .into_any_element()
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .child(font_import_button(
                                "font-import-cancel",
                                "Cancel",
                                p,
                                cx.listener(|this, _ev, _window, cx| this.cancel(cx)),
                            ))
                            .child(font_import_button(
                                "font-import-browse",
                                "Browse...",
                                p,
                                cx.listener(|this, _ev, window, cx| this.browse(window, cx)),
                            )),
                    ),
            )
    }
}

/// Same button shape as `close_confirm.rs`'s `close_confirm_button` — kept
/// as its own copy rather than shared, since the two panels' button rows
/// have no other coupling and a shared helper would just be an extra
/// indirection for four lines of styling.
fn font_import_button(
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
