use gpui::prelude::*;
use gpui::*;

/// A formatting action that a ribbon button will eventually dispatch to the text editor.
///
/// Adding new formatting operations in the future only requires adding a variant here
/// and a corresponding button in `button_rows()`.
#[derive(Clone, Debug)]
pub enum FormatAction {
    Bold,
    Italic,
    Underline,
    StrikeThrough,
    AlignLeft,
    AlignCenter,
    AlignRight,
    BulletList,
}

/// Metadata for a single ribbon button (label, hover tooltip text, action variant).
#[derive(Clone)]
struct RibbonButton {
    label: &'static str,
    tooltip: &'static str,
    action: FormatAction,
}

/// The formatting ribbon rendered between the tab bar and the main editor.
///
/// Displays a 2-row × 4-column grid of formatting buttons. Currently the buttons
/// are demonstration stubs that print to the console. The structure is designed to
/// be extended: adding more rows or buttons requires only changes to `button_rows()`.
pub struct FormattingRibbon;

impl FormattingRibbon {
    pub fn new() -> Self {
        /*
         * No state required at construction. The ribbon contains only static button
         * definitions and dispatches actions that the text editor will implement.
         */
        FormattingRibbon
    }

    /// Returns the two rows of buttons displayed in the ribbon.
    fn button_rows() -> [Vec<RibbonButton>; 2] {
        /*
         * Defines the ribbon layout as two rows:
         *   Row 1 — character-level formatting (Bold, Italic, Underline, Strikethrough)
         *   Row 2 — paragraph-level formatting (alignment, lists)
         *
         * Future rows can be appended here without touching the render code.
         */
        [
            vec![
                RibbonButton { label: "B",   tooltip: "Bold",          action: FormatAction::Bold },
                RibbonButton { label: "I",   tooltip: "Italic",        action: FormatAction::Italic },
                RibbonButton { label: "U",   tooltip: "Underline",     action: FormatAction::Underline },
                RibbonButton { label: "S",   tooltip: "Strikethrough", action: FormatAction::StrikeThrough },
            ],
            vec![
                RibbonButton { label: "≡L",  tooltip: "Align Left",   action: FormatAction::AlignLeft },
                RibbonButton { label: "≡C",  tooltip: "Align Center", action: FormatAction::AlignCenter },
                RibbonButton { label: "≡R",  tooltip: "Align Right",  action: FormatAction::AlignRight },
                RibbonButton { label: "•≡",  tooltip: "Bullet List",  action: FormatAction::BulletList },
            ],
        ]
    }

    /// Renders one ribbon button. The `id` parameter must be unique across all buttons.
    fn render_button(btn: &RibbonButton, id: &'static str) -> impl IntoElement {
        /*
         * Creates a styled, clickable button element. Click handling is a console-print
         * stub; future work should dispatch the corresponding FormatAction to the
         * focused TextEditor via the GPUI action system.
         */
        let tooltip = btn.tooltip;
        div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(32.0))
            .h(px(28.0))
            .bg(rgb(0x3c3c3c))
            .rounded(px(3.0))
            .text_color(rgb(0xd4d4d4))
            .text_sm()
            .cursor_pointer()
            .mr(px(4.0))
            .border_1()
            .border_color(rgb(0x555555))
            .on_click(move |_ev, _window, _cx| {
                // Stub: print the action name.
                // Replace with: cx.dispatch_action(Box::new(the_action)) once the
                // TextEditor implements the corresponding action handlers.
                println!("[FormattingRibbon] clicked: {}", tooltip);
            })
            .child(btn.label)
    }
}

impl Render for FormattingRibbon {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the ribbon as a two-row flex layout. Row indices 0..7 are used as
         * element IDs with a row prefix so each button has a globally unique ID.
         */
        let rows = Self::button_rows();

        // Static IDs for each button: "fmt-0" .. "fmt-7"
        let btn_ids: [&'static str; 8] = [
            "fmt-0", "fmt-1", "fmt-2", "fmt-3",
            "fmt-4", "fmt-5", "fmt-6", "fmt-7",
        ];
        let mut id_idx = 0usize;

        div()
            .flex()
            .flex_col()
            .w_full()
            .py(px(4.0))
            .px(px(8.0))
            .gap(px(4.0))
            .bg(rgb(0x3c3c3c))
            .border_b_1()
            .border_color(rgb(0x252526))
            .children(rows.iter().map(|row| {
                let mut row_div = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(32.0));
                for btn in row.iter() {
                    let id = btn_ids[id_idx];
                    id_idx += 1;
                    row_div = row_div.child(Self::render_button(btn, id));
                }
                row_div
            }))
    }
}
