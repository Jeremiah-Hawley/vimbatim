use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;
use crate::theme::{palette, radius, space, Palette};

/// The word-count panel: a centred dialog showing the active document's word
/// counts and an estimated speech time, opened from the toolbar's Word Count
/// button or the `open_stats` keybind.
///
/// Mounted by `MainWindow` only while `AppState.word_count_visible` is true.
/// Counts are read live from `AppState::document_stats` on each render rather
/// than cached — the panel is open for seconds at a time and the count is one
/// pass over the document.
pub struct WordCount {
    state: Entity<AppState>,
}

impl WordCount {
    pub fn new(state: Entity<AppState>) -> Self {
        WordCount { state }
    }

    /// One label/value row. `detail` is the optional smaller line underneath,
    /// used to break the spoken-word figure into its two sources.
    fn row(label: &'static str, value: String, detail: Option<String>, p: Palette) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(space::MD))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_sm().text_color(rgb(p.text)).child(label))
                    .when_some(detail, |d, detail| {
                        d.child(div().text_xs().text_color(rgb(p.text_muted)).child(detail))
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.text))
                    .child(value),
            )
    }
}

impl Render for WordCount {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let p = state.current_palette();
        let wpm = state.spreading_wpm;
        let stats = state.document_stats();
        let (minutes, seconds) = stats.estimated_time(wpm);

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
            // Clicking the backdrop closes, matching the settings modal.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, _window, cx| {
                this.state.update(cx, |s, cx| {
                    s.word_count_visible = false;
                    cx.notify();
                });
            }))
            .child(
                div()
                    .w(px(360.0))
                    .bg(rgb(p.chrome))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    // Without this, a click anywhere inside the panel bubbles
                    // to the backdrop handler above and closes it — the same
                    // trap documented in `settings_modal.rs`.
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    // ── Title bar ────────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px(px(16.0))
                            .py(px(12.0))
                            .border_b_1()
                            .border_color(rgb(p.border_subtle))
                            .child(
                                div()
                                    .text_color(rgb(p.text))
                                    .font_weight(FontWeight::BOLD)
                                    .child("Word Count"),
                            )
                            .child(
                                div()
                                    .id("word-count-close-x")
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .rounded(px(radius::MD))
                                    .cursor_pointer()
                                    .text_color(rgb(p.text_muted))
                                    .bg(rgb(p.chrome_active))
                                    .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                                    .on_click(cx.listener(|this, _ev, _window, cx| {
                                        this.state.update(cx, |s, cx| {
                                            s.word_count_visible = false;
                                            cx.notify();
                                        });
                                    }))
                                    .child("×"),
                            ),
                    )
                    // ── Stats ────────────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::MD))
                            .p(px(16.0))
                            .child(Self::row("Total words", stats.total_words.to_string(), None, p))
                            .child(Self::row(
                                "Words read aloud",
                                stats.spoken_words.to_string(),
                                Some(format!(
                                    "{} tag + {} highlighted",
                                    stats.tag_words, stats.highlighted_words
                                )),
                                p,
                            ))
                            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
                            .child(Self::row(
                                "Est. time",
                                format!("{minutes}:{seconds:02}"),
                                Some(format!("at {wpm} wpm")),
                                p,
                            )),
                    ),
            )
    }
}
