use gpui::prelude::*;
use gpui::*;

use crate::docx_parser::Alignment;
use crate::document_ops::FormatOp;
use crate::state::AppState;
use crate::theme::{palette, radius, space, Palette, ThemeColorMode, ThemeMode};

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
    Analytic,
    AlignLeft,
    AlignCenter,
    AlignRight,
    Body,
    PocketCite,
    HighlightYellow,
    HighlightGreen,
    RemoveHighlight,
    OpenBlock,
    CloseBlock,
    NormalSize,
    Timer,
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

/// The highlight colors the HL Color dropdown offers, as
/// `(Word highlight name written into the document, label, swatch hex)`.
///
/// The names are Word's own, which is what `Run.highlight_color` stores and
/// what `text_editor::highlight_color_hex` resolves — so a document written
/// here still opens correctly in Word, and settings.conf's `highlight_color`
/// can name any of them.
/// The text colors the Font Color dropdown offers, and the same list the
/// Analytic color setting picks from — one palette, so a color chosen for an
/// analytic is always reachable from the dropdown too.
///
/// Stored as bare 6-digit hex, which is `Run.color`'s own form. Deeper tones
/// than the highlight palette: these *are* the type, so they have to stay
/// readable at body size rather than sitting behind black text.
pub(crate) const TEXT_COLORS: [(&str, u32); 6] = [
    ("000000", 0x000000),
    ("0000ff", 0x0000FF),
    ("c00000", 0xC00000),
    ("007000", 0x007000),
    ("7030a0", 0x7030A0),
    ("b36b00", 0xB36B00),
];

pub(crate) const HIGHLIGHT_COLORS: [(&str, &str, u32); 6] = [
    ("yellow", "Yellow", 0xFFD700),
    ("green", "Green", 0x00FF00),
    ("cyan", "Cyan", 0x00FFFF),
    ("magenta", "Magenta", 0xFF00FF),
    ("blue", "Blue", 0x0000FF),
    ("red", "Red", 0xFF0000),
];

#[derive(Clone)]
struct RibbonBtn {
    label: &'static str,
    action: FormatAction,
    tone: RibbonTone,
    /// When set, the button paints this icon instead of `label` and shrinks
    /// to icon width. `label` is still carried — it is what the existing click
    /// handler logs, and it is the accessible name.
    icon: Option<RibbonIcon>,
    /// Renders as pressed-in. For buttons that toggle a mode rather than
    /// perform an action, so the ribbon shows what is currently on.
    engaged: bool,
    /// Paints the button in this color instead of its tone's. Used by HL
    /// Color to show which highlight is currently selected.
    tint: Option<u32>,
}

impl RibbonBtn {
    fn primary(label: &'static str, action: FormatAction) -> Self {
        Self {
            label,
            action,
            tone: RibbonTone::Primary,
            icon: None,
            engaged: false,
            tint: None,
        }
    }

    fn secondary(label: &'static str, action: FormatAction) -> Self {
        Self {
            label,
            action,
            tone: RibbonTone::Secondary,
            icon: None,
            engaged: false,
            tint: None,
        }
    }

    /// Marks this button as showing an active mode.
    fn engaged(mut self, engaged: bool) -> Self {
        self.engaged = engaged;
        self
    }

    /// Paints this button in `hex` rather than its tone's usual colors.
    fn tint(mut self, hex: u32) -> Self {
        self.tint = Some(hex);
        self
    }

    /// A compact icon button.
    fn icon(label: &'static str, action: FormatAction, icon: RibbonIcon) -> Self {
        Self {
            label,
            action,
            tone: RibbonTone::Secondary,
            icon: Some(icon),
            engaged: false,
            tint: None,
        }
    }
}

/// The marks a ribbon button can paint instead of a text label.
///
/// Drawn from divs rather than glyphs — there is no icon font or SVG asset in
/// this project, and the Unicode characters that come closest (alignment
/// marks, `•`, `1.`) render inconsistently across platform UI fonts. This app
/// has already been bitten once by assuming a font name resolves
/// (`text_editor.rs`'s `FONT_FAMILY` fix), so shapes that are guaranteed to
/// paint are worth the handful of extra lines.
#[derive(Clone, Copy)]
enum RibbonIcon {
    /// Four stacked bars justified to the alignment, as in Word.
    Align(Alignment),
    /// Three rows, each a dot and a bar.
    BulletList,
    /// Three rows, each a small numeral and a bar.
    NumberedList,
    /// A single letterform carrying the formatting it applies — `B` in bold,
    /// `I` italic, `U` underlined, `S` struck through. Unlike the marks above
    /// these are real text: they are plain ASCII letters styled by the very
    /// property the button toggles, so there is no font-coverage risk and the
    /// icon can't drift out of sync with what the button does.
    Bold,
    Italic,
    Underline,
    Strikethrough,
}

#[derive(Clone, Copy)]
enum RibbonTone {
    Primary,
    Secondary,
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
    /// What the user is typing into the font-size box, `None` when not
    /// editing (the box then shows the selection's actual size). Mirrors
    /// `custom_hex_buffer`'s arrangement.
    font_size_buffer: Option<String>,
    font_size_focus: FocusHandle,
    /// What the user has typed into the Switch Tab menu's search box. Empty
    /// means "show every open tab". Cleared whenever that menu opens.
    tab_search_buffer: String,
    tab_search_focus: FocusHandle,
    /// `pub(crate)` so `color_picker::render_picker`'s listeners can reach it.
    pub(crate) picker: crate::color_picker::CustomColorPicker,
    /// `AppState.read_mode` as of the last render, so the transition into it
    /// can be acted on once. Same check-and-update-per-frame idiom as
    /// `TextEditor.last_seen_active_tab`.
    last_seen_read_mode: bool,
    /// The per-group collapse state read mode replaced, restored on the way
    /// out. Collapsing is a one-shot action on entering rather than an
    /// override held for the duration, so a group can still be expanded while
    /// reading — but the user's own layout shouldn't be lost to have done so.
    collapsed_before_read_mode: Option<std::collections::HashMap<&'static str, bool>>,
}

impl FormattingRibbon {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // The ribbon reads live document state (the font-size box shows the
        // size at the cursor), but GPUI doesn't track cross-entity reads: a
        // view only re-renders when something calls `notify` on *it*. Moving
        // the cursor notifies AppState and the text editor, so without this
        // the size box kept whatever value it had at the ribbon's last render.
        cx.observe(&state, |_this, _state, cx| cx.notify()).detach();

        FormattingRibbon {
            state,
            collapsed: std::collections::HashMap::new(),
            open_menu: None,
            dismissed: None,
            editing_custom: None,
            custom_hex_buffer: String::new(),
            custom_color_focus: cx.focus_handle(),
            font_size_buffer: None,
            font_size_focus: cx.focus_handle(),
            tab_search_buffer: String::new(),
            tab_search_focus: cx.focus_handle(),
            picker: crate::color_picker::CustomColorPicker::new(),
            last_seen_read_mode: false,
            collapsed_before_read_mode: None,
        }
    }

    fn set_all_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        for name in ["cards", "text", "document", "view", "caselist"] {
            self.collapsed.insert(name, collapsed);
        }
        cx.notify();
    }

    /// Paints a `RibbonIcon` at roughly 14x16px — the size Word uses for the
    /// same marks at this button height.
    fn render_icon(icon: RibbonIcon, color: u32) -> AnyElement {
        match icon {
            RibbonIcon::Align(alignment) => {
                let justify = |d: Div| match alignment {
                    Alignment::Center => d.items_center(),
                    Alignment::Right => d.items_end(),
                    // Justify has no button of its own; it falls in with Left
                    // rather than silently painting as something else.
                    _ => d.items_start(),
                };
                justify(div().flex().flex_col())
                    .w(px(14.0))
                    .gap(px(2.0))
                    .children([14.0_f32, 9.0, 14.0, 9.0].into_iter().enumerate().map(|(i, w)| {
                        div()
                            .id(ElementId::named_usize("align-icon-bar", i))
                            .h(px(2.0))
                            .w(px(w))
                            .bg(rgb(color))
                    }))
                    .into_any_element()
            }
            RibbonIcon::Bold
            | RibbonIcon::Italic
            | RibbonIcon::Underline
            | RibbonIcon::Strikethrough => {
                let letter = match icon {
                    RibbonIcon::Bold => "B",
                    RibbonIcon::Italic => "I",
                    RibbonIcon::Underline => "U",
                    _ => "S",
                };
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(color))
                    // Without an explicit real family name the bold and
                    // italic requests below are silently dropped: GPUI
                    // resolves the default UI font to a single face, and
                    // `find_best_match` short-circuits past weight/style
                    // selection whenever only one face is loaded. Same bug,
                    // same fix as `text_editor.rs`'s FONT_FAMILY — reusing
                    // that constant keeps the choice in one place. Applied to
                    // all four letters so the row doesn't mix typefaces.
                    .font_family(crate::text_editor::FONT_FAMILY)
                    .when(matches!(icon, RibbonIcon::Bold), |d| d.font_weight(FontWeight::BOLD))
                    .when(matches!(icon, RibbonIcon::Italic), |d| d.italic())
                    .when(matches!(icon, RibbonIcon::Underline), |d| d.underline())
                    // GPUI does paint this (`text_system/line.rs` calls
                    // `window.paint_strikethrough`) — see the note in
                    // `text_editor.rs::apply_run_style`, which still claims
                    // otherwise for document text.
                    .when(matches!(icon, RibbonIcon::Strikethrough), |d| d.line_through())
                    .child(letter)
                    .into_any_element()
            }
            RibbonIcon::BulletList | RibbonIcon::NumberedList => {
                let numbered = matches!(icon, RibbonIcon::NumberedList);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .children((0..3).map(|row| {
                        let marker = if numbered {
                            // `line_height` pins the numeral to the row height
                            // so three of them still fit inside the button —
                            // without it each row grows to the font's natural
                            // line box and the icon overflows.
                            div()
                                .w(px(4.0))
                                .flex()
                                .justify_center()
                                .text_size(px(6.0))
                                .line_height(px(4.0))
                                .text_color(rgb(color))
                                .child(format!("{}", row + 1))
                                .into_any_element()
                        } else {
                            div()
                                .w(px(4.0))
                                .flex()
                                .justify_center()
                                .child(div().w(px(3.0)).h(px(3.0)).rounded_full().bg(rgb(color)))
                                .into_any_element()
                        };
                        div()
                            .id(ElementId::named_usize("list-icon-row", row))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(2.0))
                            .h(px(4.0))
                            .child(marker)
                            .child(div().w(px(9.0)).h(px(2.0)).bg(rgb(color)))
                    }))
                    .into_any_element()
            }
        }
    }

    fn make_button(
        &self,
        label: &'static str,
        action: FormatAction,
        tone: RibbonTone,
        icon: Option<RibbonIcon>,
        engaged: bool,
        tint: Option<u32>,
        p: Palette,
        color_mode: ThemeColorMode,
        state: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Font Size is a spinner (box + steppers), not a button — it occupies a
        // normal button slot in the TEXT group so the ribbon's layout and
        // grouping stay untouched.
        if action == FormatAction::FontSize {
            return self.render_font_size_control(p, cx);
        }
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
            };

        // An icon button is sized to its mark, not to the label minimum every
        // text button uses — three of them side by side then occupy no more
        // width than the two-button row above them, which is what keeps the
        // DOCUMENT group from getting wider.
        let is_icon = icon.is_some();
        // An engaged toggle takes the accent fill, overriding its tone's
        // normal colors — a mode that changes what the editor shows has to be
        // readable as on at a glance.
        let (bg, text, border) = if engaged {
            (p.accent, 0xffffff, p.accent_strong)
        } else if let Some(tint) = tint {
            // The label rides on the tint, so it needs contrast against *that*
            // rather than against the ribbon's chrome.
            (tint, crate::color_picker::contrast_text(tint), tint)
        } else {
            (bg, text, border)
        };
        let button = div()
            .id(ElementId::named_usize("ribbon-btn", action_id))
            .flex()
            .items_center()
            .justify_center()
            .when(!is_icon, |d| d.min_w(px(min_width)).px(px(space::SM)))
            .when(is_icon, |d| d.w(px(38.0)))
            .h(px(24.0))
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
                        | FormatAction::SwitchTabMenu
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
                                // Every open starts from the full tab list.
                                this.tab_search_buffer.clear();
                            }
                            cx.notify();
                        }
                        // GPUI's own opener rather than spawning `xdg-open`
                        // ourselves: on Linux it tries every opener the `open`
                        // crate knows about (including the WSL-aware ones) and
                        // falls back to the XDG desktop portal, instead of
                        // failing silently on a machine with no `xdg-open`.
                        // It also logs failures, which a bare
                        // `let _ = Command::spawn()` cannot.
                        FormatAction::OpenWiki => {
                            cx.open_url("https://opencaselist.com/");
                        }
                        FormatAction::OpenTabroom => {
                            cx.open_url("https://www.tabroom.com/index/index.mhtml");
                        }
                        FormatAction::Timer => {
                            st.update(cx, |state, _cx| {
                                state.timer.visible = !state.timer.visible;
                            });
                            cx.notify();
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
                        FormatAction::WindowSplit => {
                            st.update(cx, |state, _cx| {
                                if state.split_view {
                                    state.close_split();
                                } else {
                                    state.open_split();
                                }
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
                        // Analytic: Tag's weight and size in the configured
                        // analytic color, without the heading marker — see
                        // AppState::apply_analytic_style.
                        FormatAction::Analytic => {
                            st.update(cx, |state, _cx| {
                                state.apply_analytic_style();
                            });
                            cx.notify();
                        }
                        // Cite: apply the shared AppState::apply_cite_style,
                        // also used by the `f8` keybind (main_window.rs) so
                        // the ribbon button and hotkey behave identically.
                        FormatAction::Cite => {
                            st.update(cx, |state, _cx| state.apply_cite_style());
                            cx.notify();
                        }
                        // Align Left / Center / Right: set the current line's
                        // alignment. Not a toggle — see AppState::apply_line_alignment.
                        FormatAction::AlignLeft
                        | FormatAction::AlignCenter
                        | FormatAction::AlignRight => {
                            let alignment = match act {
                                FormatAction::AlignLeft => Alignment::Left,
                                FormatAction::AlignCenter => Alignment::Center,
                                FormatAction::AlignRight => Alignment::Right,
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
                                    // The generic Highlight button follows
                                    // settings.conf's `highlight_color`; the
                                    // explicit HighlightYellow/Green buttons
                                    // keep naming their own color.
                                    let op = match (act, op) {
                                        (FormatAction::Highlight, FormatOp::Highlight(_)) => {
                                            FormatOp::Highlight(Some(state.highlight_color.clone()))
                                        }
                                        (_, op) => op,
                                    };
                                    state.apply_formatting_to_selection(op);
                                });
                                cx.notify();
                            }
                        }
                    }
                })
            })
            .map(|d| match icon {
                Some(icon) => d.child(Self::render_icon(icon, text)),
                None => d.child(label),
            });

        // The menu is a child of its own button's wrapper, and `anchored()`
        // with no `.position()` resolves to the element's own laid-out origin
        // (gpui's `AnchoredPositionMode::Window` falls back to
        // `bounds.origin`). That's what makes the panel open under the button
        // rather than under wherever inside the button the user happened to
        // click.
        //
        // That origin is the anchored element's *own* slot in the wrapper —
        // i.e. already below the button, since the button is the first child.
        // The offset below is therefore only the gap, not a gap plus a button
        // height: adding the latter put every menu one full button too low.
        // It went unnoticed while Doc Menu / Card Menu had another row beneath
        // them; once the ribbon dropped from four rows to three they became
        // the last row and the menus opened into empty space below it.
        div()
            .relative()
            .child(button)
            .when(self.open_menu == Some(action), |d| {
                let panel = self.render_menu_panel(action, p, cx);
                d.child(
                    deferred(
                        anchored()
                            .snap_to_window()
                            .offset(point(px(0.0), px(2.0)))
                            .child(panel),
                    )
                    .with_priority(100),
                )
            })
            .into_any_element()
    }

    /// Word's font-size control: a typable box with a small up and a small down
    /// stepper. Replaces the old single button that cycled 24 -> 32 -> 48pt,
    /// which could neither show the current size nor reach any other value.
    fn render_font_size_control(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        // What the box shows: whatever the user is mid-typing, else the
        // selection's actual size in points, else blank when the selection
        // mixes sizes (same as Word). A run size of 0 carries no override, so
        // it displays as the configured body size, which is what it paints at.
        let current_points = {
            let state = self.state.read(cx);
            let default_points = state.normal_text_size_half_points as f32 / 2.0;
            state.selection_font_size_half_points().map(|half| {
                if half == 0 { default_points } else { half as f32 / 2.0 }
            })
        };

        let shown = match &self.font_size_buffer {
            Some(buffer) => buffer.clone(),
            None => match current_points {
                Some(points) if points.fract() == 0.0 => format!("{}", points as u32),
                Some(points) => format!("{points}"),
                None => String::new(),
            },
        };

        let stepper = |id: &'static str, glyph: &'static str, delta: i32, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .w(px(14.0))
                .h(px(11.0))
                .rounded(px(radius::XS))
                .bg(rgb(p.chrome_elevated))
                .text_color(rgb(p.text_muted))
                .text_xs()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(p.border_subtle))
                .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                .active(move |s| s.bg(rgb(p.chrome_active)))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _ev, _window, cx| {
                        cx.stop_propagation();
                        this.step_font_size(delta, cx);
                    }),
                )
                .child(glyph)
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(space::XXS))
            // ── the typable box ──────────────────────────────────────────
            .child(
                div()
                    .id("font-size-box")
                    .track_focus(&self.font_size_focus)
                    .on_key_down(cx.listener(Self::handle_font_size_key))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(34.0))
                    .h(px(24.0))
                    .rounded(px(radius::MD))
                    .bg(rgb(p.chrome_elevated))
                    .text_color(rgb(p.text))
                    .text_sm()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(if self.font_size_buffer.is_some() {
                        p.accent
                    } else {
                        p.border_subtle
                    }))
                    .hover(move |s| s.border_color(rgb(p.border)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, window, cx| {
                            cx.stop_propagation();
                            // Start from empty so typing replaces the size
                            // rather than appending to it, the way clicking
                            // Word's box selects what's already there.
                            this.font_size_buffer = Some(String::new());
                            this.font_size_focus.clone().focus(window, cx);
                            cx.notify();
                        }),
                    )
                    .child(shown),
            )
            // ── the steppers, stacked like Word's ────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(stepper("font-size-up", "\u{25b4}", 1, cx))
                    .child(stepper("font-size-down", "\u{25be}", -1, cx)),
            )
            .into_any_element()
    }

    /// Nudges the size by `delta` points, starting from whatever the selection
    /// currently is (or the configured body size when it's mixed or unset).
    /// Clamped to 1..=409pt, Word's own range.
    fn step_font_size(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _cx| {
            let default_half = state.normal_text_size_half_points;
            let current_half = match state.selection_font_size_half_points() {
                Some(0) | None => default_half,
                Some(half) => half,
            };
            let points = (current_half as i32 / 2 + delta).clamp(1, 409);
            state.set_font_size_half_points((points * 2) as u16);
        });
        // Typing then stepping should continue from the stepped value, not the
        // half-finished text.
        self.font_size_buffer = None;
        cx.notify();
    }

    fn handle_font_size_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(buffer) = self.font_size_buffer.as_mut() else { return };
        match event.keystroke.key.as_str() {
            "backspace" => {
                buffer.pop();
            }
            "escape" => {
                self.font_size_buffer = None;
            }
            "enter" => {
                let points = buffer.parse::<i32>().ok().map(|p| p.clamp(1, 409));
                self.font_size_buffer = None;
                if let Some(points) = points {
                    self.state.update(cx, |state, _cx| {
                        state.set_font_size_half_points((points * 2) as u16);
                    });
                }
            }
            // Three digits is enough for Word's 409pt ceiling.
            k if k.len() == 1
                && buffer.len() < 3
                && k.chars().next().unwrap().is_ascii_digit() =>
            {
                buffer.push_str(k);
            }
            _ => return,
        }
        cx.notify();
    }

    fn render_menu_panel(
        &self,
        action: FormatAction,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme_mode = self.state.read(cx).theme_mode;
        let rows: Vec<AnyElement> = match action {
            FormatAction::DocMenu => Self::text_menu_rows(
                "Doc Menu",
                // `None` = not implemented yet, and rendered red. Give an
                // entry its `AppState` method as it lands.
                &[
                    ("Delete tags", Some(AppState::delete_tags)),
                    ("Delete analytics", Some(AppState::delete_analytics)),
                    (
                        "Convert analytics to tags",
                        Some(AppState::convert_analytics_to_tags),
                    ),
                    ("Remove emphasis", Some(AppState::remove_emphasis)),
                    (
                        "Remove non highlighted underlining",
                        Some(AppState::remove_non_highlighted_underlining),
                    ),
                    ("Remove blank lines", Some(AppState::remove_blank_lines)),
                    ("Remove pilcrows", Some(AppState::remove_pilcrows)),
                    (
                        "Select similar formatting",
                        Some(AppState::select_similar_formatting),
                    ),
                ],
                p,
                theme_mode,
                cx,
            ),
            FormatAction::SwitchTabMenu => self.switch_tab_rows(p, cx),
            FormatAction::CardMenu => Self::text_menu_rows(
                "Card Menu",
                &[
                    ("Condense, no pilcrows", Some(AppState::condense_selection)),
                    ("Condense, pilcrows", Some(AppState::condense_with_pilcrows)),
                    ("Uncondensed", None),
                    ("Standardize highlighting", Some(AppState::standardize_highlighting)),
                    (
                        "Standardize highlighting with exception",
                        Some(AppState::standardize_highlighting_with_exception),
                    ),
                ],
                p,
                theme_mode,
                cx,
            ),
            FormatAction::FontColor => {
                let mut swatches: Vec<AnyElement> = TEXT_COLORS
                    .into_iter()
                    .map(|(name, hex)| {
                        Self::color_swatch(
                            ElementId::named_usize("font-color-choice", hex as usize),
                            hex,
                            p,
                            cx,
                            move |this, cx| {
                                this.state.update(cx, |state, _cx| {
                                    state.apply_formatting_to_selection(FormatOp::Color(Some(
                                        name.to_string(),
                                    )));
                                });
                                this.open_menu = None;
                                cx.notify();
                            },
                            None,
                        )
                    })
                    .collect();
                swatches.extend(self.custom_color_swatches(FormatAction::FontColor, p, cx));
                vec![
                    Self::color_grid(swatches),
                    self.render_custom_color_row(FormatAction::FontColor, p, cx),
                ]
            }
            FormatAction::HighlightColorSelect => {
                let mut swatches: Vec<AnyElement> = HIGHLIGHT_COLORS
                    .into_iter()
                    .map(|(name, _label, hex)| {
                        Self::color_swatch(
                            ElementId::named_usize("highlight-color-choice", hex as usize),
                            hex,
                            p,
                            cx,
                            move |this, cx| {
                                this.state.update(cx, |state, _cx| {
                                    // Picking here is what makes a color "the
                                    // current highlight" — it drives the
                                    // Highlight button and keybind, Standardize
                                    // Highlighting, and this button's own tint,
                                    // not just the selection under the cursor.
                                    state.set_highlight_color(name);
                                    state.apply_formatting_to_selection(FormatOp::Highlight(Some(
                                        name.to_string(),
                                    )));
                                });
                                this.open_menu = None;
                                cx.notify();
                            },
                            None,
                        )
                    })
                    .collect();
                swatches
                    .extend(self.custom_color_swatches(FormatAction::HighlightColorSelect, p, cx));
                vec![
                    Self::color_grid(swatches),
                    self.render_custom_color_row(FormatAction::HighlightColorSelect, p, cx),
                ]
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

    /// The Switch Tab dropdown: a search box over the open tabs' titles, then
    /// a scrollable list of whatever still matches. Clicking a row activates
    /// that tab.
    ///
    /// Rows carry the tab's stable `id`, not its position in the filtered
    /// list — filtering reorders nothing but does remove entries, so a
    /// positional index would activate the wrong document as soon as the
    /// search box had anything in it.
    fn switch_tab_rows(&self, p: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let query = self.tab_search_buffer.to_lowercase();
        let (tabs, active_tab) = {
            let state = self.state.read(cx);
            let tabs: Vec<(usize, usize, String)> = state
                .tabs
                .iter()
                .enumerate()
                .map(|(idx, t)| (idx, t.id, t.title.clone()))
                .filter(|(_, _, title)| query.is_empty() || title.to_lowercase().contains(&query))
                .collect();
            (tabs, state.active_tab)
        };

        let typed = self.tab_search_buffer.clone();
        let search_box = div()
            .id("switch-tab-search")
            .track_focus(&self.tab_search_focus)
            .on_key_down(cx.listener(Self::handle_tab_search_key))
            .w_full()
            .h(px(24.0))
            .px(px(space::SM))
            .mb(px(space::XXS))
            .flex()
            .items_center()
            .rounded(px(radius::SM))
            .bg(rgb(p.editor_bg))
            .border_1()
            .border_color(rgb(p.accent))
            .text_sm()
            .text_color(rgb(if typed.is_empty() { p.text_faint } else { p.text }))
            .child(if typed.is_empty() { "Search tabs…".to_string() } else { typed })
            .into_any_element();

        let mut rows = vec![search_box];

        if tabs.is_empty() {
            rows.push(
                div()
                    .px(px(space::SM))
                    .py(px(space::XXS))
                    .text_sm()
                    .text_color(rgb(p.text_faint))
                    .child("No matching tabs")
                    .into_any_element(),
            );
            return rows;
        }

        // Scroll rather than grow: a debater can have dozens of files open,
        // and an unbounded dropdown would run off the bottom of the window.
        let list = div()
            .id("switch-tab-list")
            .max_h(px(220.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .children(tabs.into_iter().map(|(idx, id, title)| {
                let is_active = idx == active_tab;
                div()
                    .id(ElementId::named_usize("switch-tab-row", id))
                    .px(px(space::SM))
                    .py(px(space::XXS))
                    .rounded(px(radius::SM))
                    .text_sm()
                    .cursor_pointer()
                    .when(is_active, |d| {
                        d.bg(rgb(p.accent_wash)).text_color(rgb(p.text)).font_weight(FontWeight::BOLD)
                    })
                    .when(!is_active, |d| {
                        d.text_color(rgb(p.text)).hover(move |s| s.bg(rgb(p.chrome_hover)))
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            cx.stop_propagation();
                            this.state.update(cx, |state, cx| {
                                // Re-resolve by id: the list was built on a
                                // previous frame and a tab could have closed
                                // since, which would shift every later index.
                                if let Some(pos) = state.tabs.iter().position(|t| t.id == id) {
                                    state.set_active_tab(pos);
                                }
                                cx.notify();
                            });
                            this.open_menu = None;
                            this.tab_search_buffer.clear();
                            cx.notify();
                        }),
                    )
                    .child(title)
            }))
            .into_any_element();

        rows.push(list);
        rows
    }

    /// Keystrokes for the Switch Tab search box. Escape closes the menu,
    /// Enter jumps to the single remaining match (the common case after
    /// typing a few letters).
    fn handle_tab_search_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_menu != Some(FormatAction::SwitchTabMenu) {
            return;
        }
        let ks = &event.keystroke;
        match ks.key.as_str() {
            "escape" => {
                self.open_menu = None;
                self.tab_search_buffer.clear();
            }
            "backspace" => {
                self.tab_search_buffer.pop();
            }
            "enter" => {
                let query = self.tab_search_buffer.to_lowercase();
                let hit = {
                    let state = self.state.read(cx);
                    state
                        .tabs
                        .iter()
                        .position(|t| query.is_empty() || t.title.to_lowercase().contains(&query))
                };
                if let Some(pos) = hit {
                    self.state.update(cx, |state, cx| {
                        state.set_active_tab(pos);
                        cx.notify();
                    });
                }
                self.open_menu = None;
                self.tab_search_buffer.clear();
            }
            key => {
                // Same resolver the find bar and vim's `f` use — it handles
                // shifted punctuation correctly on this GPUI backend.
                let Some(ch) = crate::state::vim_find_target_char(
                    key,
                    ks.modifiers.shift,
                    ks.key_char.as_deref(),
                ) else {
                    return;
                };
                self.tab_search_buffer.push(ch);
            }
        }
        cx.notify();
    }

    /// Rows for the Doc Menu / Card Menu dropdowns.
    ///
    /// Each item carries the `AppState` method it runs, or `None` when the
    /// command doesn't exist yet — those render red, so the menus stop
    /// advertising things that silently do nothing. One field expresses both
    /// the behaviour and the colour: giving an entry an action is what makes
    /// it go black.
    fn text_menu_rows(
        menu_label: &'static str,
        items: &[(&'static str, Option<fn(&mut AppState)>)],
        p: Palette,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // A warning red that stays legible on both palettes — the dark tone is
        // unreadable on a light background, the same per-mode pairing the
        // settings modal's keybind-conflict message and the editor's
        // unsupported-document banner already use.
        let not_implemented = match theme_mode {
            ThemeMode::Dark => 0xf48771,
            ThemeMode::Light => 0xb02a15,
        };
        items
            .iter()
            .enumerate()
            .map(|(idx, (item, action))| {
                let item = *item;
                let action = *action;
                div()
                    .id(ElementId::named_usize("ribbon-menu-item", idx))
                    .px(px(space::SM))
                    .py(px(space::XXS))
                    .rounded(px(radius::SM))
                    .text_color(rgb(if action.is_some() { p.text } else { not_implemented }))
                    .text_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p.chrome_hover)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            cx.stop_propagation();
                            match action {
                                Some(run) => this.state.update(cx, |state, cx| {
                                    run(state);
                                    cx.notify();
                                }),
                                None => println!("{menu_label}: {item}"),
                            }
                            this.open_menu = None;
                            cx.notify();
                        }),
                    )
                    .child(item)
                    .into_any_element()
            })
            .collect()
    }
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
    /// One swatch in a color grid. Unlabelled — the color is the label.
    ///
    /// `on_delete` is `Some` only for user-added custom colors: it renders a
    /// small × in the corner, invisible until the swatch is hovered so a full
    /// grid isn't a field of delete buttons. The × is a sibling overlay rather
    /// than a nested child, so clicking it cannot also apply the color.
    fn color_swatch(
        id: ElementId,
        hex: u32,
        p: Palette,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        on_delete: Option<Box<dyn Fn(&mut Self, &mut Context<Self>)>>,
    ) -> AnyElement {
        let group = SharedString::from(format!("swatch-{hex:06X}"));
        let mut swatch = div()
            .id(id)
            .group(group.clone())
            .relative()
            .w(px(22.0))
            .h(px(22.0))
            .flex_none()
            .rounded(px(radius::SM))
            .bg(rgb(hex))
            .border_1()
            .border_color(rgb(p.border))
            .cursor_pointer()
            .hover(move |s| s.border_color(rgb(p.text)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _ev, _window, cx| {
                    cx.stop_propagation();
                    on_pick(this, cx);
                }),
            );

        if let Some(on_delete) = on_delete {
            swatch = swatch.child(
                div()
                    .id(SharedString::from(format!("swatch-del-{hex:06X}")))
                    .absolute()
                    .top(px(-4.0))
                    .right(px(-4.0))
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgb(p.chrome_elevated))
                    .border_1()
                    .border_color(rgb(p.border))
                    .text_size(px(8.0))
                    .text_color(transparent_black())
                    .group_hover(group, move |s| s.text_color(rgb(p.text_muted)))
                    .hover(move |s| s.text_color(rgb(p.text)))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            cx.stop_propagation();
                            on_delete(this, cx);
                        }),
                    )
                    // ponytail: `×`, not a trash can — no font on this system
                    // covers U+1F5D1, and it matches every other delete in the
                    // app. Swap once an emoji font ships with the build.
                    .child("×"),
            );
        }
        swatch.into_any_element()
    }

    /// Wraps swatches into a grid, as one panel row.
    ///
    /// Replaces the old column of full-width labelled bars: with six built-in
    /// colors plus however many custom ones, that list was taller than the
    /// ribbon it dropped out of, and the names added nothing the swatch itself
    /// doesn't already say.
    fn color_grid(swatches: Vec<AnyElement>) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .w(px(160.0))
            .gap(px(space::XS))
            .children(swatches)
            .into_any_element()
    }

    /// The user's saved custom colors, as grid swatches with a hover-delete.
    fn custom_color_swatches(
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
                Self::color_swatch(
                    ElementId::named_usize("custom-color-swatch", hex as usize),
                    hex,
                    p,
                    cx,
                    move |this, cx| {
                        this.apply_custom_color(target, hex, cx);
                        this.open_menu = None;
                        this.editing_custom = None;
                        cx.notify();
                    },
                    Some(Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                        // Deleting leaves the menu open — you're usually
                        // tidying several swatches at once, not picking one.
                        this.state.update(cx, |state, _cx| {
                            state.remove_custom_color(storage, hex);
                        });
                        cx.notify();
                    })),
                )
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
                    // Same path the built-in swatches take — `Run.color` is a
                    // bare 6-digit hex either way.
                    state.apply_formatting_to_selection(FormatOp::Color(Some(format!(
                        "{hex:06x}"
                    ))));
                }
                FormatAction::HighlightColorSelect => {
                    // `highlight_color_hex` parses a bare 6-digit hex, so a
                    // custom color can be the current highlight just like a
                    // named one.
                    let hex_name = format!("{hex:06x}");
                    state.set_highlight_color(&hex_name);
                    state.apply_formatting_to_selection(FormatOp::Highlight(Some(hex_name)));
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
                                        btn.icon,
                                        btn.engaged,
                                        btn.tint,
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Valid only for the duration of one mouse-down dispatch (see the
        // field's doc comment) — this is the clear half of that contract.
        self.dismissed = None;

        // Entering read mode collapses every group, so the document gets the
        // window; leaving puts the previous layout back. Acted on once per
        // transition rather than held as an override, so a group can still be
        // expanded by hand while reading.
        let read_mode = self.state.read(cx).read_mode;
        if read_mode != self.last_seen_read_mode {
            self.last_seen_read_mode = read_mode;
            if read_mode {
                self.collapsed_before_read_mode = Some(self.collapsed.clone());
                for name in ["cards", "text", "document", "view", "caselist"] {
                    self.collapsed.insert(name, true);
                }
            } else if let Some(previous) = self.collapsed_before_read_mode.take() {
                self.collapsed = previous;
            }
        }

        // The Switch Tab menu is search-first, so its box takes focus for as
        // long as the menu is open — otherwise the first keystroke after
        // opening it would go to the document instead.
        if self.open_menu == Some(FormatAction::SwitchTabMenu)
            && !self.tab_search_focus.is_focused(window)
        {
            self.tab_search_focus.clone().focus(window, cx);
        }

        let state = self.state.clone();
        let (p, color_mode) = {
            let state_read = state.read(cx);
            (palette(state_read.theme, state_read.theme_mode), state_read.theme_color_mode)
        };
        let invisibility_mode = self.state.read(cx).invisibility_mode;
        let any_folded = self.state.read(cx).any_folded();
        let timer_visible = self.state.read(cx).timer.visible;
        // The button wears the current highlight color, nudged toward
        // visibility against this theme's chrome — see `visible_on_chrome`.
        let highlight_tint = crate::theme::visible_on_chrome(
            crate::text_editor::highlight_color_hex(&self.state.read(cx).highlight_color),
            self.state.read(cx).theme_mode,
        );
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
                        RibbonBtn::primary("Analytic", FormatAction::Analytic),
                    ],
                    vec![
                        RibbonBtn::secondary("Emphasis", FormatAction::Emphasis),
                        RibbonBtn::secondary("Highlight", FormatAction::Highlight),
                        RibbonBtn::secondary("Shrink", FormatAction::Shrink),
                        RibbonBtn::secondary("Clear", FormatAction::Clear),
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
                    // The four character-formatting icons plus the size
                    // spinner lead the group: they are the controls reached
                    // most often, and four 38px icons alongside the spinner
                    // still fit the width the three-label rows below already
                    // need.
                    vec![
                        RibbonBtn::icon("Bold", FormatAction::Bold, RibbonIcon::Bold),
                        RibbonBtn::icon("Italics", FormatAction::Italics, RibbonIcon::Italic),
                        RibbonBtn::icon("Underline", FormatAction::Underline, RibbonIcon::Underline),
                        RibbonBtn::icon("Strike", FormatAction::Strikethrough, RibbonIcon::Strikethrough),
                        RibbonBtn::secondary("Font Size", FormatAction::FontSize),
                    ],
                    vec![
                        RibbonBtn::secondary("Font Family", FormatAction::FontFamily),
                        RibbonBtn::secondary("Font Color", FormatAction::FontColor),
                    ],
                    vec![
                        RibbonBtn::secondary("HL Color", FormatAction::HighlightColorSelect)
                            .tint(highlight_tint),
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
                    // All five icon buttons share one row: at 38px each they
                    // fit in the width the Doc Menu / Card Menu row already
                    // needs, and folding the old fourth row in here drops
                    // DOCUMENT — the only four-row group — to three, which is
                    // what sets the ribbon's height.
                    vec![
                        RibbonBtn::icon("Bullets", FormatAction::BulletList, RibbonIcon::BulletList),
                        RibbonBtn::icon("Numbered", FormatAction::NumberedList, RibbonIcon::NumberedList),
                        RibbonBtn::icon("Align Left", FormatAction::AlignLeft, RibbonIcon::Align(Alignment::Left)),
                        RibbonBtn::icon("Align Center", FormatAction::AlignCenter, RibbonIcon::Align(Alignment::Center)),
                        RibbonBtn::icon("Align Right", FormatAction::AlignRight, RibbonIcon::Align(Alignment::Right)),
                    ],
                    vec![
                        RibbonBtn::secondary(
                            "Para Integrity",
                            FormatAction::ToggleParagraphIntegrity,
                        ),
                        RibbonBtn::secondary("Pilcrows", FormatAction::TogglePilcrows),
                    ],
                    vec![
                        RibbonBtn::secondary("Doc Menu", FormatAction::DocMenu),
                        RibbonBtn::secondary("Card Menu", FormatAction::CardMenu),
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
                        RibbonBtn::secondary("Nav", FormatAction::Nav),
                        RibbonBtn::secondary("Invisibility", FormatAction::InvisibilityMode)
                            .engaged(invisibility_mode),
                        RibbonBtn::secondary("Timer", FormatAction::Timer)
                            .engaged(timer_visible),
                    ],
                    vec![
                        RibbonBtn::secondary("Switch Tab", FormatAction::SwitchTabMenu),
                        RibbonBtn::secondary("Split", FormatAction::WindowSplit),
                        RibbonBtn::secondary("Fold", FormatAction::FoldToggle).engaged(any_folded),
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
