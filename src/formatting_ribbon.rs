use gpui::prelude::*;
use gpui::*;

use crate::docx_parser::Alignment;
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
    AlignLeft,
    AlignCenter,
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
            // Clear is always intercepted by its own match arm below (it
            // needs AppState's configured default size, which this method
            // has no access to) before ever reaching this generic mapping.
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
    open_menu: Option<FormatAction>,
    /// Which menu the panel's `on_mouse_down_out` just closed, valid only for
    /// the remainder of that one mouse-down dispatch — `render` clears it, the
    /// same check-and-clear-once-per-frame idiom as
    /// `AppState::pending_focus_editor`.
    ///
    /// ponytail: exists solely for the reopen race. `on_mouse_down_out` fires
    /// in the capture phase, so clicking an *open* menu's own button closes it
    /// before the button's bubble-phase toggle runs, and that toggle would see
    /// `open_menu == None` and reopen it — the menu could never be closed by
    /// clicking its own button. If GPUI ever grows a "was this click inside
    /// element X" test, delete this field and ask that instead.
    dismissed: Option<FormatAction>,
    editing_custom: Option<FormatAction>,
    custom_hex_buffer: String,
    custom_color_focus: FocusHandle,
    /// `pub(crate)` so `color_picker::render_picker`'s listeners can reach it.
    pub(crate) picker: crate::color_picker::CustomColorPicker,
}

impl FormattingRibbon {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        FormattingRibbon {
            state,
            collapsed: std::collections::HashMap::new(),
            open_menu: None,
            dismissed: None,
            editing_custom: None,
            custom_hex_buffer: String::new(),
            custom_color_focus: cx.focus_handle(),
            picker: crate::color_picker::CustomColorPicker::new(),
        }
    }

    fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        for name in ["cards", "text", "document", "view", "caselist"] {
            self.collapsed.insert(name, collapsed);
        }
        cx.notify();
    }

    fn make_button(
        &self,
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

        let button = div()
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
                cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
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
                        FormatAction::DocMenu
                        | FormatAction::CardMenu
                        | FormatAction::FontFamily
                        | FormatAction::FontColor
                        | FormatAction::HighlightColorSelect => {
                            // `dismissed` means the panel's capture-phase
                            // out-handler already closed this menu during this
                            // same click — treat it as "was open" so clicking
                            // an open menu's own button closes it.
                            if this.open_menu == Some(act) || this.dismissed == Some(act) {
                                this.open_menu = None;
                            } else {
                                this.open_menu = Some(act);
                                this.editing_custom = None;
                            }
                            cx.notify();
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
                        // Cite: apply the shared AppState::apply_cite_style,
                        // also used by the `f8` keybind (main_window.rs) so
                        // the ribbon button and hotkey behave identically.
                        FormatAction::Cite => {
                            st.update(cx, |state, _cx| state.apply_cite_style());
                            cx.notify();
                        }
                        // Align Left / Align Center: set the current line's
                        // alignment. Not a toggle — see AppState::apply_line_alignment.
                        FormatAction::AlignLeft | FormatAction::AlignCenter => {
                            let alignment = match act {
                                FormatAction::AlignLeft => Alignment::Left,
                                FormatAction::AlignCenter => Alignment::Center,
                                _ => unreachable!(),
                            };
                            st.update(cx, |state, _cx| state.apply_line_alignment(alignment));
                            cx.notify();
                        }
                        // Clear: clear all formatting from the entire line.
                        FormatAction::Clear => {
                            st.update(cx, |state, _cx| {
                                state.clear_formatting();
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
            .child(label);

        // The menu is a child of its own button, and `anchored()` with no
        // `.position()` resolves to the element's own laid-out origin (gpui's
        // `AnchoredPositionMode::Window` falls back to `bounds.origin`). That's
        // what makes the panel open under the button rather than under
        // wherever inside the button the user happened to click.
        div()
            .relative()
            .child(button)
            .when(self.open_menu == Some(action), |d| {
                let panel = self.render_menu_panel(action, p, cx);
                d.child(
                    deferred(
                        anchored()
                            .snap_to_window()
                            // Button height (24px) + a 2px gap.
                            .offset(point(px(0.0), px(26.0)))
                            .child(panel),
                    )
                    .with_priority(100),
                )
            })
    }

    fn render_menu_panel(
        &self,
        action: FormatAction,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = match action {
            FormatAction::DocMenu => Self::text_menu_rows(
                "Doc Menu",
                &[
                    "Fix Fake Tags",
                    "Convert analytics to tags",
                    "Fix Formatting Gaps",
                    "Revert to default styles",
                    "Remove emphasis",
                    "Remove non highlighted underlining",
                    "Remove blank lines",
                    "Remove pilcrows",
                    "Select similar formatting",
                ],
                p,
                cx,
            ),
            FormatAction::CardMenu => Self::text_menu_rows(
                "Card Menu",
                &[
                    "Condense, no pilcrows",
                    "Condense, pilcrows",
                    "Uncondensed",
                    "Standardize highlighting",
                    "Standardize highlighting with exception",
                    "Auto emphasis first",
                    "Duplicate cite",
                ],
                p,
                cx,
            ),
            FormatAction::FontColor => {
                let mut rows: Vec<AnyElement> = [
                    crate::color_picker::ColorChoice::Black,
                    crate::color_picker::ColorChoice::Red,
                    crate::color_picker::ColorChoice::Blue,
                ]
                .into_iter()
                .map(|choice| {
                    div()
                        .id(ElementId::named_usize("font-color-choice", choice.hex_value() as usize))
                        .cursor_pointer()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _ev, _window, cx| {
                                cx.stop_propagation();
                                this.state.update(cx, |state, _cx| state.apply_font_color(choice));
                                this.open_menu = None;
                                cx.notify();
                            }),
                        )
                        .child(crate::color_picker::color_button(
                            choice.hex_value(),
                            choice.label(),
                        ))
                        .into_any_element()
                })
                .collect();
                rows.extend(self.custom_color_rows(FormatAction::FontColor, p, cx));
                rows.push(self.render_custom_color_row(FormatAction::FontColor, p, cx));
                rows
            }
            FormatAction::HighlightColorSelect => {
                // First element is the Word highlight name written into the
                // document; second is what the user reads.
                let mut rows: Vec<AnyElement> = [
                    ("blue", "Blue", 0x0000FFu32),
                    ("green", "Green", 0x00FF00u32),
                    ("yellow", "Yellow", 0xFFD700u32),
                ]
                .into_iter()
                .map(|(name, label, hex)| {
                    div()
                        .id(ElementId::named_usize("highlight-color-choice", hex as usize))
                        .cursor_pointer()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _ev, _window, cx| {
                                cx.stop_propagation();
                                this.state.update(cx, |state, _cx| {
                                    state.apply_formatting_to_selection(FormatOp::Highlight(Some(
                                        name.to_string(),
                                    )));
                                });
                                this.open_menu = None;
                                cx.notify();
                            }),
                        )
                        .child(crate::color_picker::color_button(hex, label))
                        .into_any_element()
                })
                .collect();
                rows.extend(self.custom_color_rows(FormatAction::HighlightColorSelect, p, cx));
                rows.push(self.render_custom_color_row(FormatAction::HighlightColorSelect, p, cx));
                rows
            }
            FormatAction::FontFamily => {
                let mut names = cx.text_system().all_font_names();
                names.retain(|n| !n.starts_with('.'));
                names
                    .into_iter()
                    .enumerate()
                    .map(|(idx, name)| {
                        let applied_name = name.clone();
                        div()
                            .id(ElementId::named_usize("font-family-choice", idx))
                            .px(px(space::SM))
                            .py(px(space::XXS))
                            .rounded(px(radius::SM))
                            .text_color(rgb(p.text))
                            .text_sm()
                            .font_family(name.clone())
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(p.chrome_hover)))
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _ev, _window, cx| {
                                    cx.stop_propagation();
                                    this.state.update(cx, |state, _cx| {
                                        state.apply_formatting_to_selection(FormatOp::FontFamily(
                                            Some(applied_name.clone()),
                                        ));
                                    });
                                    this.open_menu = None;
                                    cx.notify();
                                }),
                            )
                            .child(name)
                            .into_any_element()
                    })
                    .collect()
            }
            _ => vec![],
        };

        div()
            .id("ribbon-menu-panel")
            // Dispatched in the *capture* phase, so it fires no matter who
            // stops propagation later in the bubble phase — which is the whole
            // point: `text_editor.rs` stops propagation on its own mouse-down,
            // which is why the root-level handler in `main_window.rs` never
            // sees editor clicks.
            .on_mouse_down_out(cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                this.open_menu = None;
                this.editing_custom = None;
                this.custom_hex_buffer.clear();
                this.dismissed = Some(action);
                cx.notify();
            }))
            .flex()
            .flex_col()
            .min_w(px(200.0))
            .max_h(px(320.0))
            .overflow_y_scroll()
            .p(px(space::XS))
            .gap(px(space::XXS))
            .rounded(px(radius::MD))
            .bg(rgb(p.chrome_elevated))
            .border_1()
            .border_color(rgb(p.border))
            .shadow_lg()
            .children(rows)
            .into_any_element()
    }

    fn text_menu_rows(
        menu_label: &'static str,
        items: &[&'static str],
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let item = *item;
                div()
                    .id(ElementId::named_usize("ribbon-menu-item", idx))
                    .px(px(space::SM))
                    .py(px(space::XXS))
                    .rounded(px(radius::SM))
                    .text_color(rgb(p.text))
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.chrome_hover)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            cx.stop_propagation();
                            println!("{menu_label}: {item}");
                            this.open_menu = None;
                            cx.notify();
                        }),
                    )
                    .child(item)
                    .into_any_element()
            })
            .collect()
    }

    /// Which saved-color list a menu writes to. `None` for the menus that have
    /// no colors at all (Doc/Card/Font Family).
    fn custom_color_target(action: FormatAction) -> Option<crate::state::CustomColorTarget> {
        match action {
            FormatAction::FontColor => Some(crate::state::CustomColorTarget::Font),
            FormatAction::HighlightColorSelect => Some(crate::state::CustomColorTarget::Highlight),
            _ => None,
        }
    }

    /// One row per saved custom color, oldest first, labelled by its hex —
    /// there's no name for an arbitrary RGB value, and the hex is what the user
    /// typed in the first place. Each row carries a delete button on its right
    /// that drops the color from the list and from settings.conf.
    fn custom_color_rows(
        &self,
        target: FormatAction,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(storage) = Self::custom_color_target(target) else {
            return vec![];
        };
        let colors: Vec<u32> = self.state.read(cx).custom_colors(storage).to_vec();
        colors
            .into_iter()
            .map(|hex| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(space::XXS))
                    // The swatch and the delete button are siblings, not
                    // nested — so clicking delete can't also apply the color,
                    // no `stop_propagation` gymnastics needed.
                    .child(
                        div()
                            .id(ElementId::named_usize("custom-color-row", hex as usize))
                            .flex_1()
                            .cursor_pointer()
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _ev, _window, cx| {
                                    cx.stop_propagation();
                                    this.apply_custom_color(target, hex, cx);
                                    this.open_menu = None;
                                    this.editing_custom = None;
                                    cx.notify();
                                }),
                            )
                            .child(crate::color_picker::color_button(
                                hex,
                                format!("#{hex:06X}"),
                            )),
                    )
                    .child(
                        div()
                            .id(ElementId::named_usize("custom-color-delete", hex as usize))
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded(px(radius::SM))
                            .text_color(rgb(p.text_muted))
                            .text_sm()
                            .cursor_pointer()
                            .hover(move |s| {
                                s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text))
                            })
                            // Deleting leaves the menu open — you're usually
                            // tidying several swatches at once, not picking one.
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _ev, _window, cx| {
                                    cx.stop_propagation();
                                    this.state.update(cx, |state, _cx| {
                                        state.remove_custom_color(storage, hex);
                                    });
                                    cx.notify();
                                }),
                            )
                            // ponytail: `×`, not a trash can — no font on this
                            // system covers U+1F5D1, and it's the same glyph
                            // every other delete/close in this app uses. Swap
                            // to "🗑" once an emoji font ships with the build.
                            .child("×"),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn handle_custom_hex_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.editing_custom else { return };
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;

        // Paste: accept the clipboard only if it really is a color.
        if (mods.control || mods.platform) && key == "v" {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                if let Some(hex) = crate::color_picker::parse_hex(&text) {
                    self.picker.set_color(hex);
                    self.custom_hex_buffer = format!("{hex:06X}");
                    cx.notify();
                }
            }
            return;
        }

        match key {
            "backspace" => {
                self.custom_hex_buffer.pop();
            }
            "escape" => {
                self.editing_custom = None;
                self.custom_hex_buffer.clear();
            }
            "enter" => {
                let hex = crate::color_picker::parse_hex(&self.custom_hex_buffer)
                    .unwrap_or_else(|| self.picker.hex());
                self.apply_custom_color(target, hex, cx);
                self.editing_custom = None;
                self.custom_hex_buffer.clear();
                self.open_menu = None;
            }
            // A leading `#` is accepted and ignored — people paste and type it
            // out of habit.
            "#" => {}
            k if k.len() == 1
                && self.custom_hex_buffer.len() < 6
                && k.chars().next().unwrap().is_ascii_hexdigit() =>
            {
                self.custom_hex_buffer.push_str(&k.to_uppercase());
                if let Some(hex) = crate::color_picker::parse_hex(&self.custom_hex_buffer) {
                    self.picker.set_color(hex);
                }
            }
            _ => return,
        }
        cx.notify();
    }

    /// Applies `hex` to the selection **and** saves it to the matching custom
    /// color list, so it comes back as its own row next time — and after a
    /// restart, since `add_custom_color` persists to settings.conf.
    fn apply_custom_color(&mut self, target: FormatAction, hex: u32, cx: &mut Context<Self>) {
        let Some(storage) = Self::custom_color_target(target) else { return };
        self.state.update(cx, |state, _cx| {
            match target {
                FormatAction::FontColor => {
                    state.apply_font_color(crate::color_picker::ColorChoice::Custom(hex));
                }
                FormatAction::HighlightColorSelect => {
                    state.apply_formatting_to_selection(FormatOp::Highlight(Some(format!(
                        "{hex:06x}"
                    ))));
                }
                _ => {}
            }
            state.add_custom_color(storage, hex);
        });
    }

    fn render_custom_color_row(&self, target: FormatAction, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.editing_custom == Some(target);
        if !editing {
            return div()
                .id("custom-color-trigger")
                .px(px(space::SM))
                .py(px(space::XXS))
                .rounded(px(radius::SM))
                .text_color(rgb(p.text))
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(p.chrome_hover)))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _ev, window, cx| {
                        cx.stop_propagation();
                        this.editing_custom = Some(target);
                        this.custom_hex_buffer.clear();
                        this.custom_color_focus.clone().focus(window, cx);
                        cx.notify();
                    }),
                )
                .child("Custom…")
                .into_any_element();
        }

        // An empty buffer means the user hasn't typed — show what the picker is
        // currently on rather than six blanks.
        let typed = if self.custom_hex_buffer.is_empty() {
            self.picker.hex_text.clone()
        } else {
            format!("{:_<6}", self.custom_hex_buffer)
        };

        div()
            .id("custom-color-input")
            .track_focus(&self.custom_color_focus)
            .on_key_down(cx.listener(Self::handle_custom_hex_key))
            .flex()
            .flex_col()
            .gap(px(space::XS))
            .px(px(space::XS))
            .py(px(space::XXS))
            .rounded(px(radius::SM))
            .bg(rgb(p.chrome_active))
            .text_color(rgb(p.text))
            .text_sm()
            .child(crate::color_picker::render_picker(&self.picker, p, cx))
            .child(div().text_sm().child(format!("#{typed}")))
            .child(
                div()
                    .id("custom-color-add")
                    .px(px(space::SM))
                    .py(px(space::XXS))
                    .rounded(px(radius::SM))
                    .bg(rgb(p.accent_muted))
                    .text_color(rgb(p.text))
                    .text_sm()
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.accent_strong)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            cx.stop_propagation();
                            let hex = crate::color_picker::parse_hex(&this.custom_hex_buffer)
                                .unwrap_or_else(|| this.picker.hex());
                            this.apply_custom_color(target, hex, cx);
                            this.editing_custom = None;
                            this.custom_hex_buffer.clear();
                            this.open_menu = None;
                            cx.notify();
                        }),
                    )
                    .child("Add"),
            )
            .child(
                div()
                    .text_color(rgb(p.text_muted))
                    .text_xs()
                    .child("Drag, or type/paste a hex. Enter to add, Esc to cancel"),
            )
            .into_any_element()
    }

    fn render_group(
        &self,
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
                                    self.make_button(
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
        // Valid only for the duration of one mouse-down dispatch (see the
        // field's doc comment) — this is the clear half of that contract.
        self.dismissed = None;

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
            .id("ribbon-root")
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .gap(px(0.0))
            .p(px(0.0))
            .bg(rgb(p.chrome))
            .child(Self::render_global_controls(all_collapsed, p, cx))
            .child(self.render_group(
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
            .child(self.render_group(
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
            .child(self.render_group(
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
                    vec![
                        RibbonBtn::secondary("Align Left", FormatAction::AlignLeft),
                        RibbonBtn::secondary("Align Center", FormatAction::AlignCenter),
                    ],
                ],
                *self.collapsed.get("document").unwrap_or(&false),
                p,
                color_mode,
                state.clone(),
                cx,
            ))
            .child(self.render_group(
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
            .child(self.render_group(
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
