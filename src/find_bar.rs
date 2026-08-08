use gpui::prelude::*;
use gpui::*;

use crate::state::{AppState, FindBar as FindBarState, FindField};
use crate::theme::{radius, space, Palette};

/// The find/replace panel (spec 4.6), floating under the ribbon at the top
/// right of the editor area whenever `AppState.find_bar` is `Some`.
///
/// Two text fields (Find, Replace) and four buttons (Next, Previous, Replace,
/// Replace All), plus an "N of M" match readout.
///
/// The fields are not real text inputs — GPUI ships none, and this codebase
/// has no reusable one (the settings modal captures raw keystrokes for keybind
/// capture, vim's `:`/`/` line captures its own). So this view claims focus and
/// interprets key-down itself, the same approach both of those already take.
/// Tab switches fields, Enter finds the next match, Shift+Enter the previous,
/// Escape closes and hands focus back to the editor.
pub struct FindBarView {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
}

impl FindBarView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        FindBarView { state, focus_handle: cx.focus_handle() }
    }

    /// Applies one keystroke to whichever field has focus.
    fn handle_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let key = ks.key.as_str();
        let shift = ks.modifiers.shift;

        // Let Ctrl/Cmd combinations through to the global keymap so the app's
        // own shortcuts still work while the bar has focus.
        if ks.modifiers.control || ks.modifiers.platform {
            return;
        }

        // Search From List mode has no text field: the words live in Settings.
        // The panel still claims focus every frame (below, in `render`) so
        // Enter/Shift+Enter reach it the way they do for a normal find — which
        // means without this guard the catch-all arm would push every
        // keystroke into the now-hidden `bar.query`, and typing into the
        // document would silently stop working while the panel is open.
        if self.state.read(cx).find_bar.as_ref().is_some_and(|b| b.list_mode) {
            match key {
                "escape" => {
                    self.state.update(cx, |s, cx| {
                        s.close_find_bar();
                        cx.notify();
                    });
                }
                "enter" => {
                    self.state.update(cx, |s, cx| {
                        s.find_next(!shift);
                        cx.notify();
                    });
                }
                // Everything else — including Tab, which has no second field
                // to switch to here — is deliberately inert.
                _ => return,
            }
            self.focus_handle.clone().focus(window, cx);
            cx.notify();
            return;
        }

        match key {
            "escape" => {
                self.state.update(cx, |s, cx| {
                    s.close_find_bar();
                    cx.notify();
                });
            }
            "enter" => {
                self.state.update(cx, |s, cx| {
                    s.find_next(!shift);
                    cx.notify();
                });
            }
            "tab" => {
                self.state.update(cx, |s, cx| {
                    if let Some(bar) = s.find_bar.as_mut() {
                        bar.focus = match bar.focus {
                            FindField::Query => FindField::Replace,
                            FindField::Replace => FindField::Query,
                        };
                    }
                    cx.notify();
                });
            }
            "backspace" => {
                self.state.update(cx, |s, cx| {
                    if let Some(bar) = s.find_bar.as_mut() {
                        match bar.focus {
                            FindField::Query => { bar.query.pop(); }
                            FindField::Replace => { bar.replacement.pop(); }
                        }
                    }
                    s.refresh_find_matches();
                    cx.notify();
                });
            }
            _ => {
                // `vim_find_target_char` is this codebase's proven
                // keystroke-to-character resolver (it handles shifted
                // punctuation correctly on this GPUI backend, where `key`
                // alone doesn't) and returns `None` for named keys like
                // "left"/"f1", which simply do nothing here.
                let Some(ch) = crate::state::vim_find_target_char(key, shift, ks.key_char.as_deref())
                else {
                    return;
                };
                self.state.update(cx, |s, cx| {
                    if let Some(bar) = s.find_bar.as_mut() {
                        match bar.focus {
                            FindField::Query => bar.query.push(ch),
                            FindField::Replace => bar.replacement.push(ch),
                        }
                    }
                    s.refresh_find_matches();
                    cx.notify();
                });
            }
        }
        // Every branch above changed something this view paints.
        self.focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    /// One text field. Clicking it moves the typing focus to that field; the
    /// focused one gets an accent border so it's obvious where keys land.
    fn field(
        &self,
        label: &'static str,
        value: &str,
        field: FindField,
        focused: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id: ElementId = match field {
            FindField::Query => "find-bar-query".into(),
            FindField::Replace => "find-bar-replace".into(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(space::SM))
            .child(
                div()
                    .w(px(56.0))
                    .flex_none()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child(label),
            )
            .child(
                div()
                    .id(id)
                    .w(px(220.0))
                    .h(px(24.0))
                    .px(px(space::SM))
                    .flex()
                    .items_center()
                    .rounded(px(radius::MD))
                    .bg(rgb(p.editor_bg))
                    .border_1()
                    .border_color(rgb(if focused { p.accent } else { p.border_subtle }))
                    .cursor_pointer()
                    .text_xs()
                    .text_color(rgb(p.text))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, window, cx| {
                        this.state.update(cx, |s, cx| {
                            if let Some(bar) = s.find_bar.as_mut() {
                                bar.focus = field;
                            }
                            cx.notify();
                        });
                        this.focus_handle.clone().focus(window, cx);
                    }))
                    // A block caret on the focused field, so an empty field
                    // still shows where typing goes.
                    .child(value.to_string())
                    .when(focused, |d| {
                        d.child(div().w(px(1.0)).h(px(14.0)).ml(px(1.0)).bg(rgb(p.text)))
                    }),
            )
    }

    /// One of the four action buttons. The caller chains its own `.on_click`;
    /// this only owns the shared chrome and the disabled-when-empty styling.
    fn button(id: &'static str, label: &'static str, enabled: bool, p: Palette) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .h(px(24.0))
            .px(px(10.0))
            .rounded(px(radius::MD))
            .text_xs()
            .border_1()
            .when(enabled, |d| {
                d.cursor_pointer()
                    .text_color(rgb(p.text))
                    .border_color(rgb(p.border))
                    .hover(move |s| s.bg(rgb(p.chrome_hover)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
            })
            .when(!enabled, |d| {
                d.text_color(rgb(p.text_faint)).border_color(rgb(p.border_subtle))
            })
            .child(label)
    }
}

impl Render for FindBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(bar) = self.state.read(cx).find_bar.clone() else {
            return div().into_any_element();
        };
        let p = self.state.read(cx).current_palette();

        // The bar is only useful with the keyboard, and it opens in response
        // to a keybind — so it takes focus on the frame it appears.
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.clone().focus(window, cx);
        }

        let FindBarState { query, replacement, focus, match_count, current_match, list_mode } = bar;
        let word_count = self.state.read(cx).search_word_list.len();
        // What "there is something to search for" means differs by mode — and
        // four separate things below key off it (both fields' visibility, the
        // buttons' enabled styling, the readout, and the Replace pair). One
        // guard so they can't disagree.
        let active = if list_mode { word_count > 0 } else { !query.is_empty() };
        let readout = if !active {
            String::new()
        } else if match_count == 0 {
            "No results".to_string()
        } else if current_match == 0 {
            format!("{match_count} matches")
        } else {
            format!("{current_match} of {match_count}")
        };

        div()
            .id("find-bar")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            // Clicks inside must not reach the editor underneath, which would
            // move the caret and steal focus back.
            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .p(px(space::SM))
            .bg(rgb(p.chrome))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(px(radius::MD))
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(space::SM))
                    // List mode has nothing to type into — the words live in
                    // Settings — so the query field is replaced by a label
                    // naming what's being searched. When the list is empty
                    // that label is also the only signpost pointing at where
                    // to write one, which is why the panel opens anyway.
                    .child(if list_mode {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Search From List"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(if active { p.text_muted } else { p.text_faint }))
                                    .child(match word_count {
                                        0 => "No words — add them in Settings → Toggle Features".to_string(),
                                        1 => "1 word".to_string(),
                                        n => format!("{n} words"),
                                    }),
                            )
                            .into_any_element()
                    } else {
                        self.field("Find", &query, FindField::Query, focus == FindField::Query, p, cx)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .child(readout),
                    )
                    .child(
                        div()
                            .id("find-bar-close")
                            .w(px(20.0))
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius::SM))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.state.update(cx, |s, cx| {
                                    s.close_find_bar();
                                    cx.notify();
                                });
                            }))
                            .child("×"),
                    ),
            )
            // Replace has no meaning against a word list — replacing every hit
            // of any of N words with one string was not asked for and is
            // destructive — so the field and both buttons are list-mode-hidden.
            // `replace_current`/`replace_all` already early-return on an empty
            // query, so hiding is enough; they need no extra guard.
            .when(!list_mode, |d| {
                d.child(self.field("Replace", &replacement, FindField::Replace, focus == FindField::Replace, p, cx))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(space::XS))
                    .child(
                        Self::button("find-bar-next", "Next", active, p)
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.state.update(cx, |s, cx| { s.find_next(true); cx.notify(); });
                            })),
                    )
                    .child(
                        Self::button("find-bar-prev", "Previous", active, p)
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.state.update(cx, |s, cx| { s.find_next(false); cx.notify(); });
                            })),
                    )
                    .when(!list_mode, |d| {
                        d.child(
                            Self::button("find-bar-replace", "Replace", active, p)
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| { s.replace_current(); cx.notify(); });
                                })),
                        )
                        .child(
                            Self::button("find-bar-replace-all", "Replace All", active, p)
                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| { s.replace_all(); cx.notify(); });
                                })),
                        )
                    }),
            )
            .into_any_element()
    }
}
