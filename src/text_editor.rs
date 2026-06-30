use gpui::prelude::*;
use gpui::*;

use crate::state::AppState;

/// The main document editing area.
///
/// Renders the text content of the currently active tab inside a focused,
/// scrollable div. Keyboard input is routed here when the div holds focus.
///
/// Designed to be the extensible base for .docx support: content currently lives
/// as plain `String` in `AppState::Tab`, meaning callers can swap in a richer
/// document model without touching this view's rendering or focus plumbing.
pub struct TextEditor {
    state: Entity<AppState>,
    /// GPUI focus handle — required to receive raw keyboard events.
    focus_handle: FocusHandle,
}

impl TextEditor {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        /*
         * Creates the text editor and registers a focus handle. Focus is claimed
         * lazily the first time the user clicks inside the editor.
         *
         * The `cx.focus_handle()` call creates a new entry in GPUI's focus registry;
         * the handle must be passed to `.track_focus()` in render() so the element
         * participates in the focus tree.
         */
        let focus_handle = cx.focus_handle();
        TextEditor { state, focus_handle }
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        /*
         * Dispatches raw key-down events to the AppState so text content is updated.
         *
         * Platform-modifier (Ctrl/Cmd) combinations are deliberately passed through
         * so global actions (toggle-settings, new-tab, etc.) can fire normally.
         * Only pure character input, space, enter, tab, and backspace are consumed.
         */
        let ks = &event.keystroke;

        // Pass Ctrl / platform-modifier combos to the global action dispatcher
        if ks.modifiers.control || ks.modifiers.platform {
            return;
        }

        let key = ks.key.as_str();
        let consumed = self.state.update(cx, |state, cx| {
            match key {
                "backspace" => { state.backspace(); cx.notify(); true }
                "enter"     => { state.insert_char('\n'); cx.notify(); true }
                "space"     => { state.insert_char(' '); cx.notify(); true }
                "tab"       => { state.insert_char('\t'); cx.notify(); true }
                k if k.chars().count() == 1 => {
                    let mut ch = k.chars().next().unwrap();
                    // Apply shift for uppercase; GPUI gives lowercase key names
                    if ks.modifiers.shift && ch.is_alphabetic() {
                        ch = ch.to_uppercase().next().unwrap_or(ch);
                    }
                    state.insert_char(ch);
                    cx.notify();
                    true
                }
                _ => false,
            }
        });
        if consumed { cx.notify(); }
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the editor as a focusable, scrollable column.
         *
         * Content is split on '\n' so each line is its own div — this preserves
         * blank lines and avoids GPUI collapsing inline text across newlines.
         *
         * A cursor marker ("_") is appended to the last line when the editor is
         * focused, giving visual feedback that key input is active.
         *
         * Clicking anywhere in the editor reclaims keyboard focus.
         */
        let state = self.state.read(cx);
        let content = state.active_content().to_string();
        let is_new_tab = state
            .tabs
            .get(state.active_tab)
            .map(|t| t.file_path.is_none() && t.content.is_empty())
            .unwrap_or(true);
        let _ = state;

        let is_focused = self.focus_handle.is_focused(window);

        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.split('\n').map(|l| l.to_string()).collect()
        };

        div()
            // `.id()` must come before `.overflow_y_scroll()` because GPUI tracks
            // scroll position per unique element ID (requires Stateful<Div>).
            .id("text-editor")
            .key_context("TextEditor")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            // Clicking the editor area claims keyboard focus
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, window, cx| {
                this.focus_handle.clone().focus(window, cx);
            }))
            .flex_1()
            .min_w_0()
            .bg(rgb(0x1e1e1e))
            .overflow_y_scroll()
            .p(px(16.0))
            // Thin focus ring so the user can tell where key input lands
            .border_1()
            .border_color(if is_focused { rgb(0x007acc) } else { rgb(0x1e1e1e) })
            .child(
                div()
                    .flex()
                    .flex_col()
                    // w_full constrains each line to the editor width so text wraps
                    // rather than extending off-screen to the right.
                    .w_full()
                    // Placeholder shown on an empty, unsaved tab
                    .when(is_new_tab, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x555555))
                                .font_family("monospace")
                                .child("Open a file from the sidebar, or start typing…"),
                        )
                    })
                    // One div per line of content
                    .children(lines.iter().enumerate().map(|(i, line)| {
                        let is_last = i == lines.len() - 1;
                        div()
                            .font_family("monospace")
                            .text_sm()
                            .text_color(rgb(0xd4d4d4))
                            // min_h keeps empty lines visually present
                            .min_h(px(20.0))
                            .child(if is_last && is_focused {
                                format!("{}_", line)
                            } else {
                                line.clone()
                            })
                    }))
            )
    }
}
