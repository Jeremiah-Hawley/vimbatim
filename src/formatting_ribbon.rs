use gpui::prelude::*;
use gpui::*;

use crate::document_ops::FormatOp;
use crate::state::AppState;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum FormatAction {
    Paste, Condense, Pocket, Hat, Block, Tag, Cite, Underline, Emphasis, Highlight, Clear, FoldToggle,
    FontSize, FontFamily, NumberedList, Italics, Bold, BulletList, FontColor, Strikethrough, ChangeCase,
    Shrink, HighlightColorSelect, ToggleParagraphIntegrity, TogglePilcrows, DocMenu, CardMenu,
    Nav, InvisibilityMode, SwitchTabMenu, WindowSplit,
    OpenWiki, OpenTabroom, Wikifi,
    Body, PocketCite, HighlightYellow, HighlightGreen, RemoveHighlight, OpenBlock, CloseBlock, NormalSize,
}

impl FormatAction {
    pub fn to_format_op(&self) -> Option<FormatOp> {
        match self {
            FormatAction::Underline => Some(FormatOp::Underline(true)),
            FormatAction::Italics => Some(FormatOp::Italic(true)),
            FormatAction::Highlight => Some(FormatOp::Highlight(Some("yellow".to_string()))),
            FormatAction::HighlightYellow => Some(FormatOp::Highlight(Some("yellow".to_string()))),
            FormatAction::HighlightGreen => Some(FormatOp::Highlight(Some("green".to_string()))),
            FormatAction::RemoveHighlight => Some(FormatOp::Highlight(None)),
            FormatAction::Bold => Some(FormatOp::Bold(true)),
            FormatAction::Clear => Some(FormatOp::ClearAll),
            FormatAction::Shrink => Some(FormatOp::FontSize(20)),
            FormatAction::NormalSize => Some(FormatOp::FontSize(24)),
            // Card styles: each maps to Bold + custom font size
            FormatAction::Pocket => Some(FormatOp::Bold(true)),    // Size 26 = 52 half-points
            FormatAction::Hat => Some(FormatOp::Bold(true)),       // Size 22 = 44 half-points
            FormatAction::Block => Some(FormatOp::Bold(true)),     // Size 16 = 32 half-points
            FormatAction::Tag => Some(FormatOp::Bold(true)),       // Size 23 = 46 half-points
            FormatAction::Cite => Some(FormatOp::Bold(true)),      // Size 13 = 26 half-points
            FormatAction::Emphasis => Some(FormatOp::Bold(true)),  // Bold only
            _ => None,
        }
    }

    pub fn card_style_size(&self) -> Option<u16> {
        match self {
            FormatAction::Pocket => Some(52),   // 26pt
            FormatAction::Hat => Some(44),      // 22pt
            FormatAction::Block => Some(32),    // 16pt
            FormatAction::Tag => Some(46),      // 23pt
            FormatAction::Cite => Some(26),     // 13pt
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            FormatAction::Paste => "Paste", FormatAction::Condense => "Condense", FormatAction::Pocket => "Pocket",
            FormatAction::Hat => "Hat", FormatAction::Block => "Block", FormatAction::Tag => "Tag",
            FormatAction::Cite => "Cite", FormatAction::Underline => "Underline", FormatAction::Emphasis => "Emphasis",
            FormatAction::Highlight => "Highlight", FormatAction::Clear => "Clear", FormatAction::FoldToggle => "Fold Toggle",
            FormatAction::FontSize => "Font Size", FormatAction::FontFamily => "Font Family", FormatAction::NumberedList => "Numbered List",
            FormatAction::Italics => "Italics", FormatAction::Bold => "Bold", FormatAction::BulletList => "Bullet List",
            FormatAction::FontColor => "Font Color", FormatAction::Strikethrough => "Strikethrough", FormatAction::ChangeCase => "Change Case",
            FormatAction::Shrink => "Shrink", FormatAction::HighlightColorSelect => "HL Color", FormatAction::ToggleParagraphIntegrity => "Para Integrity",
            FormatAction::TogglePilcrows => "Pilcrows", FormatAction::DocMenu => "Doc Menu", FormatAction::CardMenu => "Card Menu",
            FormatAction::Nav => "Nav", FormatAction::InvisibilityMode => "Invisibility", FormatAction::SwitchTabMenu => "Switch Tab",
            FormatAction::WindowSplit => "Window Split", FormatAction::OpenWiki => "Open Wiki", FormatAction::OpenTabroom => "Open Tabroom",
            FormatAction::Wikifi => "Wikifi", FormatAction::Body => "Body", FormatAction::PocketCite => "Pkt Cite",
            FormatAction::HighlightYellow => "HLt", FormatAction::HighlightGreen => "HLg", FormatAction::RemoveHighlight => "Rm HL",
            FormatAction::OpenBlock => "Open Blk", FormatAction::CloseBlock => "Close Blk", FormatAction::NormalSize => "Normal",
        }
    }
}

#[derive(Clone)]
struct RibbonBtn {
    label: &'static str,
    action: FormatAction,
}

pub struct FormattingRibbon {
    #[allow(dead_code)]
    state: Entity<AppState>,
    collapsed: std::collections::HashMap<&'static str, bool>,
}

impl FormattingRibbon {
    pub fn new(state: Entity<AppState>) -> Self {
        FormattingRibbon {
            state,
            collapsed: std::collections::HashMap::new(),
        }
    }

    fn make_button(label: &'static str, action: FormatAction, state: Entity<AppState>, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(60.0))
            .h(px(24.0))
            .px(px(4.0))
            .rounded(px(2.0))
            .bg(rgb(0x3d3d3d))
            .text_color(rgb(0xd4d4d4))
            .text_sm()
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, {
                let label_text = label;
                let act = action.clone();
                let st = state.clone();
                cx.listener(move |_this, _ev, _window, cx| {
                    println!("Button pressed: {}", label_text);
                    match act {
                        FormatAction::Paste => {
                            if let Some(item) = cx.read_from_clipboard() {
                                if let Some(text) = item.text() {
                                    st.update(cx, |state, _cx| {
                                        state.paste_text(&text);
                                    });
                                }
                            }
                        }
                        FormatAction::Condense => {
                            st.update(cx, |state, _cx| {
                                state.condense_selection();
                            });
                        }
                        // Card styles: apply bold + custom font size
                        FormatAction::Pocket | FormatAction::Hat | FormatAction::Block |
                        FormatAction::Tag | FormatAction::Cite => {
                            if let Some(op) = act.to_format_op() {
                                st.update(cx, |state, _cx| {
                                    state.apply_formatting_to_selection(op);
                                });
                            }
                            if let Some(size) = act.card_style_size() {
                                st.update(cx, |state, _cx| {
                                    state.apply_formatting_to_selection(FormatOp::FontSize(size));
                                });
                            }
                        }
                        _ => {
                            if let Some(op) = act.to_format_op() {
                                st.update(cx, |state, _cx| {
                                    state.apply_formatting_to_selection(op);
                                });
                            }
                        }
                    }
                })
            })
            .child(label)
    }

    fn render_group(name: &'static str, label: &'static str, buttons: &[Vec<RibbonBtn>], is_collapsed: bool, state: Entity<AppState>, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .border_r_1()
            .border_color(rgb(0x3d3d3d))
            .px(px(8.0))
            .h_full()
            .when(!is_collapsed, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .flex_1()
                        .children(buttons.iter().map(|row| {
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(4.0))
                                .children(row.iter().map(|btn| {
                                    Self::make_button(btn.label, btn.action.clone(), state.clone(), cx)
                                }))
                        }))
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(2.0))
                    .cursor_pointer()
                    .px(px(4.0))
                    .py(px(2.0))
                    .rounded(px(2.0))
                    .bg(rgb(0x3d3d3d))
                    .text_color(rgb(0x999999))
                    .text_xs()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                        let collapsed = this.collapsed.get(name).copied().unwrap_or(false);
                        this.collapsed.insert(name, !collapsed);
                        cx.notify();
                    }))
                    .child(label)
                    .child(if is_collapsed { "▶" } else { "▼" })
            )
    }
}

impl Render for FormattingRibbon {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        div()
            .flex()
            .flex_row()
            .w_full()
            .gap(px(0.0))
            .p(px(0.0))
            .bg(rgb(0x2d2d2d))
            .child(Self::render_group(
                "organize",
                "ORGANIZE",
                &[
                    vec![RibbonBtn { label: "Paste", action: FormatAction::Paste }, RibbonBtn { label: "Condense", action: FormatAction::Condense }, RibbonBtn { label: "Pocket", action: FormatAction::Pocket }, RibbonBtn { label: "Hat", action: FormatAction::Hat }],
                    vec![RibbonBtn { label: "Block", action: FormatAction::Block }, RibbonBtn { label: "Tag", action: FormatAction::Tag }, RibbonBtn { label: "Cite", action: FormatAction::Cite }, RibbonBtn { label: "Underline", action: FormatAction::Underline }],
                    vec![RibbonBtn { label: "Emphasis", action: FormatAction::Emphasis }, RibbonBtn { label: "Highlight", action: FormatAction::Highlight }, RibbonBtn { label: "Clear", action: FormatAction::Clear }, RibbonBtn { label: "Fold Toggle", action: FormatAction::FoldToggle }],
                ],
                *self.collapsed.get("organize").unwrap_or(&false),
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "document",
                "DOCUMENT",
                &[
                    vec![RibbonBtn { label: "Font Size", action: FormatAction::FontSize }, RibbonBtn { label: "Font Family", action: FormatAction::FontFamily }, RibbonBtn { label: "Numbered List", action: FormatAction::NumberedList }],
                    vec![RibbonBtn { label: "Italics", action: FormatAction::Italics }, RibbonBtn { label: "Bold", action: FormatAction::Bold }, RibbonBtn { label: "Bullet List", action: FormatAction::BulletList }],
                    vec![RibbonBtn { label: "Font Color", action: FormatAction::FontColor }, RibbonBtn { label: "Strikethrough", action: FormatAction::Strikethrough }, RibbonBtn { label: "Change Case", action: FormatAction::ChangeCase }],
                ],
                *self.collapsed.get("document").unwrap_or(&false),
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "card_format",
                "CARD FORMAT",
                &[
                    vec![RibbonBtn { label: "Shrink", action: FormatAction::Shrink }, RibbonBtn { label: "HL Color", action: FormatAction::HighlightColorSelect }],
                    vec![RibbonBtn { label: "Para Integrity", action: FormatAction::ToggleParagraphIntegrity }, RibbonBtn { label: "Pilcrows", action: FormatAction::TogglePilcrows }],
                    vec![RibbonBtn { label: "Doc Menu", action: FormatAction::DocMenu }, RibbonBtn { label: "Card Menu", action: FormatAction::CardMenu }],
                ],
                *self.collapsed.get("card_format").unwrap_or(&false),
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "view",
                "VIEW",
                &[
                    vec![RibbonBtn { label: "Nav", action: FormatAction::Nav }, RibbonBtn { label: "Invisibility", action: FormatAction::InvisibilityMode }],
                    vec![RibbonBtn { label: "Switch Tab", action: FormatAction::SwitchTabMenu }, RibbonBtn { label: "Window Split", action: FormatAction::WindowSplit }],
                ],
                *self.collapsed.get("view").unwrap_or(&false),
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "caselist",
                "CASELIST",
                &[
                    vec![RibbonBtn { label: "Open Wiki", action: FormatAction::OpenWiki }],
                    vec![RibbonBtn { label: "Open Tabroom", action: FormatAction::OpenTabroom }],
                    vec![RibbonBtn { label: "Wikifi", action: FormatAction::Wikifi }],
                ],
                *self.collapsed.get("caselist").unwrap_or(&false),
                state.clone(),
                cx,
            ))
    }
}
