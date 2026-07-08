use gpui::prelude::*;
use gpui::*;

use crate::document_ops::FormatOp;
use crate::state::AppState;
use crate::theme::{palette, radius, space, Palette, ThemeColorMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum FormatAction {
    Paste,
    Condense,
    Pocket,
    Hat,
    Block,
    Tag,
    Cite,
    Underline,
    Emphasis,
    Highlight,
    Clear,
    FoldToggle,
    FontSize,
    FontFamily,
    NumberedList,
    Italics,
    Bold,
    BulletList,
    FontColor,
    Strikethrough,
    ChangeCase,
    Shrink,
    HighlightColorSelect,
    ToggleParagraphIntegrity,
    TogglePilcrows,
    DocMenu,
    CardMenu,
    Nav,
    InvisibilityMode,
    SwitchTabMenu,
    WindowSplit,
    CollapseAll,
    OpenWiki,
    OpenTabroom,
    Wikifi,
    Body,
    PocketCite,
    HighlightYellow,
    HighlightGreen,
    RemoveHighlight,
    OpenBlock,
    CloseBlock,
    NormalSize,
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
            FormatAction::Strikethrough => Some(FormatOp::Strikethrough(true)),
            FormatAction::Shrink => Some(FormatOp::FontSize(20)),
            FormatAction::NormalSize => Some(FormatOp::FontSize(24)),
            // Card styles: each maps to Bold + custom font size
            FormatAction::Pocket => Some(FormatOp::Bold(true)), // Size 26 = 52 half-points
            FormatAction::Hat => Some(FormatOp::Bold(true)),    // Size 22 = 44 half-points
            FormatAction::Block => Some(FormatOp::Bold(true)),  // Size 16 = 32 half-points
            FormatAction::Tag => Some(FormatOp::Bold(true)),    // Size 23 = 46 half-points
            FormatAction::Cite => Some(FormatOp::Bold(true)),   // Size 13 = 26 half-points
            FormatAction::Emphasis => Some(FormatOp::Bold(true)), // Bold only
            _ => None,
        }
    }
}

#[derive(Clone)]
struct RibbonBtn {
    label: &'static str,
    action: FormatAction,
    tone: RibbonTone,
}

impl RibbonBtn {
    fn primary(label: &'static str, action: FormatAction) -> Self {
        Self {
            label,
            action,
            tone: RibbonTone::Primary,
        }
    }

    fn secondary(label: &'static str, action: FormatAction) -> Self {
        Self {
            label,
            action,
            tone: RibbonTone::Secondary,
        }
    }

    fn quiet(label: &'static str, action: FormatAction) -> Self {
        Self {
            label,
            action,
            tone: RibbonTone::Quiet,
        }
    }
}

#[derive(Clone, Copy)]
enum RibbonTone {
    Primary,
    Secondary,
    Quiet,
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

    fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        for name in ["cards", "text", "document", "view", "caselist"] {
            self.collapsed.insert(name, collapsed);
        }
        cx.notify();
    }

    fn make_button(
        label: &'static str,
        action: FormatAction,
        tone: RibbonTone,
        p: Palette,
        color_mode: ThemeColorMode,
        state: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let action_id = action as usize;
        let (bg, text, border, hover_bg, hover_border, active_bg, min_width) =
            match (tone, color_mode) {
                (RibbonTone::Primary, ThemeColorMode::Minimal) => (
                    p.accent_wash,
                    p.text,
                    p.accent_muted,
                    p.selection,
                    p.accent,
                    p.accent_muted,
                    68.0,
                ),
                (RibbonTone::Primary, ThemeColorMode::Vivid) => (
                    p.accent_wash,
                    p.text,
                    p.accent_alt,
                    p.selection,
                    p.highlight,
                    p.accent_muted,
                    68.0,
                ),
                (RibbonTone::Secondary, ThemeColorMode::Vivid) => (
                    p.chrome_elevated,
                    p.text,
                    p.border_subtle,
                    p.selection,
                    p.accent_muted,
                    p.accent_wash,
                    68.0,
                ),
                (RibbonTone::Secondary, _) => (
                    p.chrome_elevated,
                    p.text,
                    p.border_subtle,
                    p.chrome_hover,
                    p.border,
                    p.chrome_active,
                    60.0,
                ),
                (RibbonTone::Quiet, ThemeColorMode::Vivid) => (
                    p.chrome_active,
                    p.text_muted,
                    p.border_subtle,
                    p.accent_wash,
                    p.accent_muted,
                    p.selection,
                    56.0,
                ),
                (RibbonTone::Quiet, _) => (
                    p.chrome_active,
                    p.text_muted,
                    p.border_subtle,
                    p.chrome_hover,
                    p.border,
                    p.chrome_active,
                    56.0,
                ),
            };

        div()
            .id(ElementId::named_usize("ribbon-btn", action_id))
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(min_width))
            .h(px(24.0))
            .px(px(space::SM))
            .rounded(px(radius::MD))
            .bg(rgb(bg))
            .text_color(rgb(text))
            .text_sm()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(border))
            .hover(move |s| {
                s.bg(rgb(hover_bg))
                    .border_color(rgb(hover_border))
                    .text_color(rgb(p.text))
            })
            .active(move |s| s.bg(rgb(active_bg)))
            .on_mouse_down(gpui::MouseButton::Left, {
                let label_text = label;
                let act = action;
                let st = state.clone();
                cx.listener(move |_this, _ev, _window, cx| {
                    println!("Button pressed: {}", label_text);
                    if !matches!(
                        act,
                        FormatAction::DocMenu
                            | FormatAction::CardMenu
                            | FormatAction::SwitchTabMenu
                    ) {
                        cx.stop_propagation();
                    }
                    match act {
                        FormatAction::Paste => {
                            if let Some(item) = cx.read_from_clipboard() {
                                if let Some(text) = item.text() {
                                    st.update(cx, |state, _cx| {
                                        state.paste_text(&text);
                                    });
                                    cx.notify();
                                }
                            }
                        }
                        FormatAction::Condense => {
                            st.update(cx, |state, _cx| {
                                state.condense_selection();
                            });
                            cx.notify();
                        }
                        FormatAction::BulletList => {
                            st.update(cx, |state, _cx| {
                                state.apply_bullet_list();
                            });
                            cx.notify();
                        }
                        FormatAction::NumberedList => {
                            st.update(cx, |state, _cx| {
                                state.apply_numbered_list();
                            });
                            cx.notify();
                        }
                        FormatAction::FontSize => {
                            st.update(cx, |state, _cx| {
                                state.cycle_font_size();
                            });
                            cx.notify();
                        }
                        FormatAction::FontColor => {
                            st.update(cx, |state, _cx| {
                                // Default to Black for now
                                state.apply_font_color(crate::color_picker::ColorChoice::Black);
                            });
                            cx.notify();
                        }
                        FormatAction::HighlightColorSelect => {
                            st.update(cx, |state, _cx| {
                                state.cycle_highlight_color();
                            });
                            cx.notify();
                        }
                        FormatAction::Shrink => {
                            st.update(cx, |state, _cx| {
                                state.shrink_text();
                            });
                            cx.notify();
                        }
                        FormatAction::ChangeCase => {
                            st.update(cx, |state, _cx| {
                                // Default to Title case for now
                                state.apply_case_to_selection(
                                    crate::case_converter::CaseType::Title,
                                );
                            });
                            cx.notify();
                        }
                        FormatAction::Strikethrough => {
                            st.update(cx, |state, _cx| {
                                state.toggle_strikethrough();
                            });
                            cx.notify();
                        }
                        FormatAction::FoldToggle => {
                            st.update(cx, |state, _cx| {
                                state.toggle_fold();
                            });
                            cx.notify();
                        }
                        FormatAction::ToggleParagraphIntegrity => {
                            st.update(cx, |state, _cx| {
                                state.toggle_paragraph_integrity();
                            });
                            cx.notify();
                        }
                        FormatAction::TogglePilcrows => {
                            st.update(cx, |state, _cx| {
                                state.toggle_pilcrows();
                            });
                            cx.notify();
                        }
                        FormatAction::DocMenu => {
                            println!("Doc Menu opened - options are placeholders for Phase 5");
                            // Menu items:
                            // - Fix Fake Tags
                            // - Convert analytics to tags
                            // - Fix Formatting Gaps
                            // - Revert to default styles
                            // - Remove emphasis
                            // - Remove non highlighted underlining
                            // - Remove blank lines
                            // - Remove pilcrows
                            // - Select similar formatting
                        }
                        FormatAction::CardMenu => {
                            println!("Card Menu opened - options are placeholders for Phase 5");
                            // Menu items:
                            // - Condense, no pilcrows
                            // - Condense, pilcrows
                            // - Uncondensed
                            // - Standardize highlighting
                            // - Standardize highlighting with exception
                            // - Auto emphasis first
                            // - Duplicate cite
                        }
                        FormatAction::OpenWiki => {
                            let url = "https://opencaselist.com/";
                            #[cfg(target_os = "macos")]
                            {
                                let _ = std::process::Command::new("open").arg(url).spawn();
                            }
                            #[cfg(target_os = "linux")]
                            {
                                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
                            }
                            #[cfg(target_os = "windows")]
                            {
                                let _ = std::process::Command::new("cmd")
                                    .args(&["/C", "start", url])
                                    .spawn();
                            }
                        }
                        FormatAction::OpenTabroom => {
                            let url = "https://www.tabroom.com/index/index.mhtml";
                            #[cfg(target_os = "macos")]
                            {
                                let _ = std::process::Command::new("open").arg(url).spawn();
                            }
                            #[cfg(target_os = "linux")]
                            {
                                let _ = std::process::Command::new("xdg-open").arg(url).spawn();
                            }
                            #[cfg(target_os = "windows")]
                            {
                                let _ = std::process::Command::new("cmd")
                                    .args(&["/C", "start", url])
                                    .spawn();
                            }
                        }
                        FormatAction::Nav => {
                            // Toggles the same AppState.sidebar_mode the
                            // file explorer's own Files/Nav header buttons
                            // control (file_explorer.rs). Also ensures the
                            // sidebar itself is visible — "open the
                            // navigation tab" (ribbon_instructions.md)
                            // implies making it visible, not just switching
                            // its mode while it might be collapsed.
                            st.update(cx, |state, _cx| {
                                state.sidebar_mode = match state.sidebar_mode {
                                    crate::state::SidebarMode::Files => crate::state::SidebarMode::Nav,
                                    crate::state::SidebarMode::Nav => crate::state::SidebarMode::Files,
                                };
                                state.sidebar_visible = true;
                            });
                            cx.notify();
                        }
                        FormatAction::InvisibilityMode => {
                            st.update(cx, |state, _cx| {
                                state.toggle_invisibility_mode();
                            });
                            cx.notify();
                        }
                        FormatAction::SwitchTabMenu => {
                            st.update(cx, |state, _cx| {
                                let tabs = state.get_tab_titles();
                                println!("Switch Tab Menu: {:?}", tabs);
                                // UI for selecting tab would go here
                            });
                            cx.notify();
                        }
                        FormatAction::WindowSplit => {
                            st.update(cx, |state, _cx| {
                                state.toggle_split_view();
                            });
                            cx.notify();
                        }
                        FormatAction::Wikifi => {
                            st.update(cx, |state, _cx| match state.wikify_current_tab() {
                                Ok(_) => println!("Document exported to markdown"),
                                Err(e) => println!("Export failed: {}", e),
                            });
                            cx.notify();
                        }
                        // Card styles: apply the shared AppState::apply_card_style,
                        // also used by the configurable keybind actions (src/keybinds.rs)
                        // so ribbon buttons and hotkeys behave identically.
                        FormatAction::Pocket
                        | FormatAction::Hat
                        | FormatAction::Block
                        | FormatAction::Tag => {
                            let kind = match act {
                                FormatAction::Pocket => crate::state::CardStyleKind::Pocket,
                                FormatAction::Hat => crate::state::CardStyleKind::Hat,
                                FormatAction::Block => crate::state::CardStyleKind::Block,
                                FormatAction::Tag => crate::state::CardStyleKind::Tag,
                                _ => unreachable!(),
                            };
                            st.update(cx, |state, _cx| state.apply_card_style(kind));
                            cx.notify();
                        }
                        // Clear: clear all formatting from the entire line.
                        FormatAction::Clear => {
                            st.update(cx, |state, _cx| {
                                state.apply_formatting_to_line(FormatOp::ClearAll);
                            });
                            cx.notify();
                        }
                        _ => {
                            if let Some(op) = act.to_format_op() {
                                st.update(cx, |state, _cx| {
                                    state.apply_formatting_to_selection(op);
                                });
                                cx.notify();
                            }
                        }
                    }
                })
            })
            .child(label)
    }

    fn render_group(
        name: &'static str,
        label: &'static str,
        buttons: &[Vec<RibbonBtn>],
        is_collapsed: bool,
        p: Palette,
        color_mode: ThemeColorMode,
        state: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let header_text = if color_mode == ThemeColorMode::Vivid {
            p.accent_strong
        } else {
            p.text_muted
        };
        let header_hover_text = if color_mode == ThemeColorMode::Vivid {
            p.accent_strong
        } else {
            p.text
        };

        div()
            .flex()
            .flex_col()
            .gap(px(space::SM))
            .border_r_1()
            .border_color(rgb(p.border_subtle))
            .px(px(space::MD))
            .py(px(space::XS))
            .h_full()
            .child(
                div()
                    .id(ElementId::from(format!("ribbon-group-toggle-{name}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(space::XS))
                    .cursor_pointer()
                    .px(px(space::XS))
                    .py(px(space::XXS))
                    .rounded(px(radius::MD))
                    .bg(rgb(p.chrome_active))
                    .text_color(rgb(header_text))
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(header_hover_text)))
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            let collapsed = this.collapsed.get(name).copied().unwrap_or(false);
                            this.collapsed.insert(name, !collapsed);
                            cx.notify();
                        }),
                    )
                    .child(label)
                    .child(if is_collapsed { "▶" } else { "▼" }),
            )
            .when(!is_collapsed, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::XS))
                        .flex_1()
                        .children(buttons.iter().map(|row| {
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(space::XS))
                                .children(row.iter().map(|btn| {
                                    Self::make_button(
                                        btn.label,
                                        btn.action,
                                        btn.tone,
                                        p,
                                        color_mode,
                                        state.clone(),
                                        cx,
                                    )
                                }))
                        })),
                )
            })
    }

    fn render_global_controls(
        all_collapsed: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_start()
            .gap(px(space::XS))
            .border_r_1()
            .border_color(rgb(p.border_subtle))
            .px(px(space::SM))
            .py(px(space::XS))
            .h_full()
            .child(
                div()
                    .id("ribbon-expand-all")
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(28.0))
                    .h(px(24.0))
                    .rounded(px(radius::MD))
                    .bg(rgb(p.chrome_active))
                    .text_color(rgb(p.text_muted))
                    .text_sm()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(p.border_subtle))
                    .hover(move |s| {
                        s.bg(rgb(p.chrome_hover))
                            .border_color(rgb(p.accent_muted))
                            .text_color(rgb(p.text))
                    })
                    .active(move |s| s.bg(rgb(p.chrome_active)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.set_all_collapsed(!all_collapsed, cx);
                        }),
                    )
                    .child(if all_collapsed { "▶" } else { "▼" }),
            )
    }
}

impl Render for FormattingRibbon {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let (p, color_mode) = {
            let state_read = state.read(cx);
            (palette(state_read.theme), state_read.theme_color_mode)
        };
        let ribbon_groups = ["cards", "text", "document", "view", "caselist"];
        let all_collapsed = ribbon_groups
            .iter()
            .all(|name| self.collapsed.get(name).copied().unwrap_or(false));
        div()
            .flex()
            .flex_row()
            .w_full()
            .gap(px(0.0))
            .p(px(0.0))
            .bg(rgb(p.chrome))
            .child(Self::render_global_controls(all_collapsed, p, cx))
            .child(Self::render_group(
                "cards",
                "CARDS",
                &[
                    vec![
                        RibbonBtn::primary("Paste", FormatAction::Paste),
                        RibbonBtn::primary("Condense", FormatAction::Condense),
                        RibbonBtn::primary("Pocket", FormatAction::Pocket),
                        RibbonBtn::primary("Hat", FormatAction::Hat),
                    ],
                    vec![
                        RibbonBtn::primary("Block", FormatAction::Block),
                        RibbonBtn::primary("Tag", FormatAction::Tag),
                        RibbonBtn::primary("Cite", FormatAction::Cite),
                        RibbonBtn::secondary("Emphasis", FormatAction::Emphasis),
                    ],
                    vec![
                        RibbonBtn::secondary("Highlight", FormatAction::Highlight),
                        RibbonBtn::secondary("Shrink", FormatAction::Shrink),
                        RibbonBtn::secondary("Clear", FormatAction::Clear),
                        RibbonBtn::quiet("Fold", FormatAction::FoldToggle),
                    ],
                ],
                *self.collapsed.get("cards").unwrap_or(&false),
                p,
                color_mode,
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "text",
                "TEXT",
                &[
                    vec![
                        RibbonBtn::secondary("Bold", FormatAction::Bold),
                        RibbonBtn::secondary("Italics", FormatAction::Italics),
                        RibbonBtn::secondary("Underline", FormatAction::Underline),
                    ],
                    vec![
                        RibbonBtn::secondary("Font Size", FormatAction::FontSize),
                        RibbonBtn::quiet("Font Family", FormatAction::FontFamily),
                        RibbonBtn::secondary("Font Color", FormatAction::FontColor),
                    ],
                    vec![
                        RibbonBtn::secondary("HL Color", FormatAction::HighlightColorSelect),
                        RibbonBtn::secondary("Strike", FormatAction::Strikethrough),
                        RibbonBtn::secondary("Case", FormatAction::ChangeCase),
                    ],
                ],
                *self.collapsed.get("text").unwrap_or(&false),
                p,
                color_mode,
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "document",
                "DOCUMENT",
                &[
                    vec![
                        RibbonBtn::secondary("Bullets", FormatAction::BulletList),
                        RibbonBtn::secondary("Numbered", FormatAction::NumberedList),
                    ],
                    vec![
                        RibbonBtn::secondary(
                            "Para Integrity",
                            FormatAction::ToggleParagraphIntegrity,
                        ),
                        RibbonBtn::secondary("Pilcrows", FormatAction::TogglePilcrows),
                    ],
                    vec![
                        RibbonBtn::quiet("Doc Menu", FormatAction::DocMenu),
                        RibbonBtn::quiet("Card Menu", FormatAction::CardMenu),
                    ],
                ],
                *self.collapsed.get("document").unwrap_or(&false),
                p,
                color_mode,
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "view",
                "VIEW",
                &[
                    vec![
                        RibbonBtn::quiet("Nav", FormatAction::Nav),
                        RibbonBtn::secondary("Invisibility", FormatAction::InvisibilityMode),
                    ],
                    vec![
                        RibbonBtn::secondary("Switch Tab", FormatAction::SwitchTabMenu),
                        RibbonBtn::secondary("Split", FormatAction::WindowSplit),
                    ],
                ],
                *self.collapsed.get("view").unwrap_or(&false),
                p,
                color_mode,
                state.clone(),
                cx,
            ))
            .child(Self::render_group(
                "caselist",
                "CASELIST",
                &[
                    vec![RibbonBtn::primary("Wikifi", FormatAction::Wikifi)],
                    vec![RibbonBtn::secondary("Open Wiki", FormatAction::OpenWiki)],
                    vec![RibbonBtn::secondary("Tabroom", FormatAction::OpenTabroom)],
                ],
                *self.collapsed.get("caselist").unwrap_or(&false),
                p,
                color_mode,
                state.clone(),
                cx,
            ))
    }
}
