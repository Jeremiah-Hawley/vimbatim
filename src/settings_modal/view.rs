use super::*;

impl SettingsModal {
    /// The left-hand pane switcher. One row per `SettingsSection`, the active
    /// one washed with the accent color.
    fn render_sidebar(&self, p: crate::theme::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .w(px(180.0))
            .flex_none()
            .p(px(10.0))
            .border_r_1()
            .border_color(rgb(p.border_subtle))
            .bg(rgb(p.sidebar))
            .children(SettingsSection::all().into_iter().map(|section| {
                let is_current = section == self.section;
                div()
                    .id(ElementId::named_usize("settings-section", section as usize))
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_sm()
                    .when(is_current, |d| {
                        d.bg(rgb(p.accent_wash))
                            .text_color(rgb(p.text))
                            .font_weight(FontWeight::BOLD)
                    })
                    .when(!is_current, |d| {
                        d.text_color(rgb(p.text_muted))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.section = section;
                            this.cancel_capture(cx);
                            this.cancel_vim_capture();
                            this.cancel_word_list_edit();
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(crate::icons::icon(
                                section.icon(),
                                if is_current { p.text } else { p.text_muted },
                                14.0,
                            ))
                            .child(section.label()),
                    )
            }))
    }

    /// The actions a keybind list shows for `category`.
    ///
    /// Shared by both the Ctrl-combo list and the Vim Keybinds list so the two
    /// can't drift on what's visible. `CommandPalette` is hidden while its Toggle
    /// Features switch is off — the same "only show it once the feature is on"
    /// rule the Vim Keybinds section itself follows — so a disabled feature
    /// doesn't leave a bindable-looking dead row behind.
    pub(super) fn listed_actions(
        category: KeybindCategory,
        command_palette_enabled: bool,
    ) -> Vec<KeybindAction> {
        KeybindAction::all()
            .iter()
            .copied()
            .filter(|a| a.category() == category)
            .filter(|a| *a != KeybindAction::CommandPalette || command_palette_enabled)
            .collect()
    }

    /// A labelled on/off row with a description — the shared shape of every
    /// entry in the Toggle Features pane.
    fn toggle_row(
        id: &'static str,
        label: &'static str,
        description: &'static str,
        enabled: bool,
        p: crate::theme::Palette,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(p.text))
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(p.text_muted))
                            .max_w(px(400.0))
                            .child(description),
                    ),
            )
            .child(
                div()
                    .id(id)
                    .flex_none()
                    .cursor_pointer()
                    .px(px(12.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .text_xs()
                    .border_1()
                    .when(enabled, |d| {
                        d.bg(rgb(p.accent))
                            .border_color(rgb(p.accent_strong))
                            // The accent is a saturated mid-tone in both
                            // modes, so a fixed light label stays legible on
                            // it — unlike `p.text`, which inverts with the
                            // mode and would vanish on the light palette.
                            .text_color(rgb(0xffffff))
                    })
                    .when(!enabled, |d| {
                        d.bg(rgb(p.chrome_active))
                            .border_color(rgb(p.border))
                            .text_color(rgb(p.text_muted))
                    })
                    .on_mouse_down(MouseButton::Left, on_click)
                    .child(if enabled { "On" } else { "Off" }),
            )
    }

    /// The Toggle Features pane: every on/off feature in one group, per the
    /// bottom-of-settings grouping this sidebar's last entry names.
    fn render_toggle_features(
        &self,
        vim_enabled: bool,
        spellcheck_enabled: bool,
        spellcheck_color: String,
        spreading_wpm: u32,
        nav_fold_buttons: bool,
        search_from_list_enabled: bool,
        search_list_whole_words: bool,
        command_palette_enabled: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                Self::toggle_row(
                    "vim-mode-toggle",
                    "Vim Mode",
                    "Enables modal editing (Normal/Insert/Visual modes and motions), similar to the Vim text editor.",
                    vim_enabled,
                    p,
                    cx.listener(|this, _ev, _window, cx| this.toggle_vim(cx)),
                ),
            )
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(Self::toggle_row(
                        "spellcheck-toggle",
                        "Spellcheck",
                        "Underlines misspelled words. Right-click one for suggestions, or to add it to your dictionary.",
                        spellcheck_enabled,
                        p,
                        cx.listener(|this, _ev, _window, cx| this.toggle_spellcheck(cx)),
                    ))
                    // Underline color — only meaningful while spellcheck is on.
                    .when(spellcheck_enabled, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.text_muted))
                                        .child("Underline color"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(6.0))
                                        .children(SPELLCHECK_COLORS.iter().map(|&(name, hex)| {
                                            let selected = spellcheck_color == name;
                                            div()
                                                .id(SharedString::from(format!("spellcheck-color-{name}")))
                                                .w(px(20.0))
                                                .h(px(20.0))
                                                .rounded(px(4.0))
                                                .bg(rgb(hex))
                                                .cursor_pointer()
                                                .border_2()
                                                // The selected swatch gets a ring
                                                // in the page's own text color
                                                // (so it reads on both light and
                                                // dark); the rest get a border
                                                // matching their own fill, which
                                                // keeps every swatch the same
                                                // size either way.
                                                .border_color(rgb(if selected { p.text } else { hex }))
                                                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                                                    this.set_spellcheck_color(name, cx);
                                                }))
                                        })),
                                ),
                        )
                    }),
            )
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            // ── Search From List ──────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(Self::toggle_row(
                        "search-from-list-toggle",
                        "Search From List",
                        "Adds a \"Search From List\" button beside Find that steps through every occurrence of any word in your list, using the same Next/Previous controls.",
                        search_from_list_enabled,
                        p,
                        cx.listener(|this, _ev, _window, cx| this.toggle_search_from_list(cx)),
                    ))
                    // The list and its matching option are only meaningful
                    // while the feature is on — same gating the spellcheck
                    // colour row above uses.
                    .when(search_from_list_enabled, |d| {
                        d.child(self.render_word_list_box(p, cx)).child(Self::toggle_row(
                            "search-list-whole-words-toggle",
                            "Whole words only",
                            "On: \"war\" matches \"war\" but not \"warming\". Off: matches anywhere inside a word, the way the Find button does.",
                            search_list_whole_words,
                            p,
                            cx.listener(|this, _ev, _window, cx| this.toggle_search_list_whole_words(cx)),
                        ))
                    }),
            )
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(Self::toggle_row(
                "command-palette-toggle",
                "Command Palette",
                "Adds a searchable list of every command in Vimbatim, opened with Ctrl+P (rebindable under Keybindings once this is on). Type to filter, Enter runs the top result.",
                command_palette_enabled,
                p,
                cx.listener(|this, _ev, _window, cx| this.toggle_command_palette(cx)),
            ))
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(Self::toggle_row(
                "nav-fold-buttons-toggle",
                "Navigation Menu Heading Fold Buttons",
                "Adds 1/2/3/4 buttons under the folder name in the Navigation sidebar that collapse the outline to that heading level. The same levels are always available by right-clicking the outline.",
                nav_fold_buttons,
                p,
                cx.listener(|this, _ev, _window, cx| this.toggle_nav_fold_buttons(cx)),
            ))
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            // ── Spreading rate ────────────────────────────────────────────
            // Not a toggle, but it belongs with the other document-behaviour
            // settings rather than under Appearance.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Spreading rate"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(p.text_muted))
                                    .max_w(px(400.0))
                                    .child("Words per minute used for the Word Count panel's speech-time estimate."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .flex_none()
                            .child(Self::stepper_btn("spreading-wpm-down", "−", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_spreading_wpm(-10, cx)),
                            ))
                            .child(
                                div()
                                    .w(px(56.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(p.text))
                                    .child(format!("{spreading_wpm}")),
                            )
                            .child(Self::stepper_btn("spreading-wpm-up", "+", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_spreading_wpm(10, cx)),
                            )),
                    ),
            )
    }

    /// One −/+ button of a numeric stepper.
    fn stepper_btn(
        id: &'static str,
        label: &'static str,
        p: crate::theme::Palette,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .w(px(24.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .text_sm()
            .text_color(rgb(p.text))
            .bg(rgb(p.chrome_active))
            .border_1()
            .border_color(rgb(p.border))
            .hover(move |s| s.bg(rgb(p.chrome_hover)))
            .child(label)
    }

    /// A labelled checkbox. Used for the Emphasis trio, which are independent
    /// rather than a pick-one — a squad's "emphasis" is whatever combination
    /// they standardised on.
    fn checkbox(
        id: &'static str,
        label: &'static str,
        checked: bool,
        p: crate::theme::Palette,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, on_click)
            .child(
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .border_1()
                    .text_size(px(11.0))
                    .when(checked, |d| {
                        d.bg(rgb(p.accent))
                            .border_color(rgb(p.accent_strong))
                            .text_color(rgb(0xffffff))
                            .child("✓")
                    })
                    .when(!checked, |d| {
                        d.bg(rgb(p.chrome_active)).border_color(rgb(p.border))
                    }),
            )
            .child(div().text_sm().text_color(rgb(p.text)).child(label))
    }

    fn render_timer_settings(
        &self,
        speech_minutes: [u16; 3],
        prep_minutes: u16,
        timer_panel_enabled: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = |id: &'static str, label: &'static str, value: u16, speech: Option<usize>| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(div().text_sm().text_color(rgb(p.text)).child(label))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(Self::stepper_btn("timer-down", "−", p).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _window, cx| {
                                this.adjust_timer_default(speech, -1, cx)
                            }),
                        ))
                        .child(
                            div()
                                .w(px(56.0))
                                .text_center()
                                .child(format!("{value} min")),
                        )
                        .child(Self::stepper_btn(id, "+", p).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _ev, _window, cx| {
                                this.adjust_timer_default(speech, 1, cx)
                            }),
                        )),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(div().text_lg().font_weight(FontWeight::BOLD).child("Timer"))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(p.text_muted))
                    .child("Defaults used by the speech timer presets and prep timer."),
            )
            .child(row(
                "timer-speech-1",
                "Speech button 1",
                speech_minutes[0],
                Some(0),
            ))
            .child(row(
                "timer-speech-2",
                "Speech button 2",
                speech_minutes[1],
                Some(1),
            ))
            .child(row(
                "timer-speech-3",
                "Speech button 3",
                speech_minutes[2],
                Some(2),
            ))
            .child(row(
                "timer-prep-up",
                "Prep time per team",
                prep_minutes,
                None,
            ))
            .child(div().h(px(1.0)).bg(rgb(p.border_subtle)))
            .child(Self::toggle_row(
                "timer-panel-toggle", "Timer Panel",
                "Shows the timer as its own sidebar panel, next to Files and Nav, instead of docked above them.",
                timer_panel_enabled, p,
                cx.listener(|this, _ev, _window, cx| this.toggle_timer_panel_enabled(cx)),
            ))
    }

    /// The Text Settings pane: default highlight color, what Emphasis applies,
    /// and how the paste command treats newlines.
    fn render_text_settings(
        &self,
        emphasis: (bool, bool, bool),
        emphasis_change_size: bool,
        emphasis_size_points: u16,
        shrink_points: u16,
        // Pocket, Hat, Block, Tag, Cite — in points, in that order.
        card_size_points: [u16; 5],
        exception: String,
        custom_highlights: Vec<u32>,
        analytic_color: String,
        paste_condense: bool,
        paste_condense_pilcrow: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (bold, underline, boxed) = emphasis;
        let heading = |text: &'static str| {
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(p.text))
                .child(text)
        };
        let note = |text: &'static str| {
            div()
                .text_xs()
                .text_color(rgb(p.text_muted))
                .max_w(px(420.0))
                .child(text)
        };

        // One stepper row per card style. Built as a loop rather than five
        // copy-pasted blocks: the only thing that varies is the label, the
        // element ids, and which size it writes.
        let card_rows: [(
            &'static str,
            &'static str,
            &'static str,
            Option<CardStyleKind>,
        ); 5] = [
            (
                "card-size-pocket-down",
                "card-size-pocket-up",
                "Pocket",
                Some(CardStyleKind::Pocket),
            ),
            (
                "card-size-hat-down",
                "card-size-hat-up",
                "Hat",
                Some(CardStyleKind::Hat),
            ),
            (
                "card-size-block-down",
                "card-size-block-up",
                "Block",
                Some(CardStyleKind::Block),
            ),
            (
                "card-size-tag-down",
                "card-size-tag-up",
                "Tag",
                Some(CardStyleKind::Tag),
            ),
            ("card-size-cite-down", "card-size-cite-up", "Cite", None),
        ];

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            // ── Card style sizes ──────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Card style sizes"))
                    .child(note(
                        "The point size each card style applies. These also go into the \
                         Heading 1-4 style definitions of documents created here, so Word \
                         and Verbatim show the same sizes.",
                    ))
                    .children(card_rows.into_iter().zip(card_size_points).map(
                        |((id_down, id_up, label, kind), points)| {
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .w(px(64.0))
                                        .text_sm()
                                        .text_color(rgb(p.text))
                                        .child(label),
                                )
                                .child(Self::stepper_btn(id_down, "−", p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _window, cx| {
                                        this.adjust_card_size(kind, -1, cx)
                                    }),
                                ))
                                .child(
                                    div()
                                        .w(px(48.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_sm()
                                        .text_color(rgb(p.text))
                                        .child(format!("{points} pt")),
                                )
                                .child(Self::stepper_btn(id_up, "+", p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _ev, _window, cx| {
                                        this.adjust_card_size(kind, 1, cx)
                                    }),
                                ))
                        },
                    )),
            )
            // ── Shrink size ───────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Shrink size"))
                    .child(note("The point size Shrink drops text to. Underlined text is left alone."))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(Self::stepper_btn("shrink-size-down", "−", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_shrink_size(-1, cx)),
                            ))
                            .child(
                                div()
                                    .w(px(48.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_sm()
                                    .text_color(rgb(p.text))
                                    .child(format!("{shrink_points} pt")),
                            )
                            .child(Self::stepper_btn("shrink-size-up", "+", p).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _ev, _window, cx| this.adjust_shrink_size(1, cx)),
                            )),
                    ),
            )
            // ── Analytic color ────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Analytic color"))
                    .child(note("The text color the Analytic button applies. Same palette as the Font Color dropdown."))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.0))
                            .children(crate::formatting_ribbon::TEXT_COLORS.iter().map(|&(name, hex)| {
                                let selected = analytic_color == name;
                                div()
                                    .id(SharedString::from(format!("analytic-color-{name}")))
                                    .w(px(22.0))
                                    .h(px(22.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(hex))
                                    .cursor_pointer()
                                    .border_2()
                                    .border_color(rgb(if selected { p.text } else { hex }))
                                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                                        this.state.update(cx, |s, cx| {
                                            s.set_analytic_color(name);
                                            cx.notify();
                                        });
                                        cx.notify();
                                    }))
                            })),
                    ),
            )
            // ── Standardize exception ─────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Standardize highlight exception"))
                    .child(note(
                        "\"Standardize highlighting with exception\" leaves this color alone. \
                         Click the selected swatch again to clear it.",
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .w(px(220.0))
                            .gap(px(6.0))
                            .children(
                                crate::formatting_ribbon::HIGHLIGHT_COLORS
                                    .iter()
                                    .map(|&(name, _label, hex)| (name.to_string(), hex))
                                    .chain(
                                        custom_highlights
                                            .iter()
                                            .map(|&hex| (format!("{hex:06x}"), hex)),
                                    )
                                    .map(|(name, hex)| {
                                        let selected = exception == name;
                                        let pick = name.clone();
                                        div()
                                            .id(SharedString::from(format!("std-exception-{name}")))
                                            .w(px(22.0))
                                            .h(px(22.0))
                                            .rounded(px(4.0))
                                            .bg(rgb(hex))
                                            .cursor_pointer()
                                            .border_2()
                                            .border_color(rgb(if selected { p.text } else { hex }))
                                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _ev, _window, cx| {
                                                // Re-clicking the current one
                                                // clears it — there is no
                                                // separate "none" swatch to
                                                // find.
                                                let next = if selected { String::new() } else { pick.clone() };
                                                this.state.update(cx, |s, cx| {
                                                    s.set_standardize_exception(&next);
                                                    cx.notify();
                                                });
                                                cx.notify();
                                            }))
                                    }),
                            ),
                    ),
            )
            // ── Emphasis ──────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(heading("Emphasis"))
                    .child(note("Which formatting the Emphasis command applies. Any combination."))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(16.0))
                            .child(Self::checkbox("emphasis-bold", "Bold", bold, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis(!bold, underline, boxed);
                                        cx.notify();
                                    });
                                })))
                            .child(Self::checkbox("emphasis-underline", "Underline", underline, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis(bold, !underline, boxed);
                                        cx.notify();
                                    });
                                })))
                            .child(Self::checkbox("emphasis-box", "Box", boxed, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis(bold, underline, !boxed);
                                        cx.notify();
                                    });
                                })))
                            .child(Self::checkbox("emphasis-change-size", "Change size", emphasis_change_size, p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_emphasis_change_size(!emphasis_change_size);
                                        cx.notify();
                                    });
                                }))),
                    )
                    .when(emphasis_change_size, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(Self::stepper_btn("emphasis-size-down", "−", p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _window, cx| this.adjust_emphasis_size(-1, cx)),
                                ))
                                .child(
                                    div()
                                        .w(px(48.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_sm()
                                        .text_color(rgb(p.text))
                                        .child(format!("{emphasis_size_points} pt")),
                                )
                                .child(Self::stepper_btn("emphasis-size-up", "+", p).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _ev, _window, cx| this.adjust_emphasis_size(1, cx)),
                                )),
                        )
                    }),
            )
            // ── Paste ─────────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(heading("Paste"))
                    .child(note("The ribbon's Para Integrity and Pilcrows buttons drive these same two settings."))
                    .child(Self::checkbox("paste-condense", "Condense by default", paste_condense, p,
                        cx.listener(move |this, _ev, _window, cx| {
                            this.state.update(cx, |s, cx| {
                                s.set_paste_condense(!paste_condense);
                                cx.notify();
                            });
                        })))
                    // Only meaningful while condensing, so it appears only then
                    // rather than sitting inert.
                    .when(paste_condense, |d| {
                        d.child(
                            div().pl(px(22.0)).child(Self::checkbox(
                                "paste-condense-pilcrow",
                                "Mark collapsed newlines with ¶",
                                paste_condense_pilcrow,
                                p,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.set_paste_condense_pilcrow(!paste_condense_pilcrow);
                                        cx.notify();
                                    });
                                }),
                            )),
                        )
                    }),
            )
    }

    /// The Appearance pane: theme picker, then the Theme Color / Mode pair.
    /// Also what Theme Preview shows on its own.
    fn render_appearance(
        &self,
        current_theme: ThemeKind,
        current_theme_mode: ThemeMode,
        current_theme_color_mode: ThemeColorMode,
        theme_preview: bool,
        p: crate::theme::Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            // ── Theme selector ────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pb(px(12.0))
                    .border_b_1()
                    .border_color(rgb(p.border_subtle))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Theme"),
                            )
                            .when(!theme_preview, |d| {
                                d.child(
                                    div()
                                        .id("theme-preview-toggle")
                                        .cursor_pointer()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .rounded(px(4.0))
                                        .text_xs()
                                        .bg(rgb(p.chrome_active))
                                        .text_color(rgb(p.text))
                                        .border_1()
                                        .border_color(rgb(p.border))
                                        .hover(move |s| s.bg(rgb(p.chrome_hover)))
                                        .active(move |s| s.bg(rgb(p.chrome_active)))
                                        .on_click(cx.listener(|this, _ev, _window, cx| {
                                            this.enter_theme_preview(cx);
                                        }))
                                        .child("Preview"),
                                )
                            }),
                    )
                    .child(div().flex().flex_row().flex_wrap().gap(px(6.0)).children(
                        ThemeKind::all().iter().map(|theme| {
                            let theme = *theme;
                            let is_current = theme == current_theme;
                            let theme_palette = palette(theme, current_theme_mode);
                            div()
                                .id(ElementId::named_usize("theme-choice", theme as usize))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .cursor_pointer()
                                .pl(px(6.0))
                                .pr(px(10.0))
                                .py(px(4.0))
                                .rounded(px(4.0))
                                .text_xs()
                                .border_1()
                                .when(is_current, |d| {
                                    d.bg(rgb(p.accent_wash))
                                        .border_color(rgb(p.accent_muted))
                                        .text_color(rgb(p.text))
                                })
                                .when(!is_current, |d| {
                                    d.bg(rgb(p.chrome_active))
                                        .border_color(rgb(p.border_subtle))
                                        .text_color(rgb(p.text_muted))
                                })
                                .hover(move |s| s.bg(rgb(p.chrome_hover)))
                                .active(move |s| s.bg(rgb(p.chrome_active)))
                                .on_click(cx.listener(move |this, _ev, _window, cx| {
                                    this.set_theme(theme, cx);
                                }))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .w(px(8.0))
                                                .h(px(8.0))
                                                .rounded(px(2.0))
                                                .bg(rgb(theme_palette.accent)),
                                        )
                                        .child(
                                            div()
                                                .w(px(8.0))
                                                .h(px(8.0))
                                                .rounded(px(2.0))
                                                .bg(rgb(theme_palette.accent_alt)),
                                        )
                                        .child(
                                            div()
                                                .w(px(8.0))
                                                .h(px(8.0))
                                                .rounded(px(2.0))
                                                .bg(rgb(theme_palette.highlight)),
                                        ),
                                )
                                .child(theme.label())
                        }),
                    )),
            )
            // ── Theme Color │ Mode ────────────────────────────────────────
            // Two labelled groups side by side, split by a thin vertical rule.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Theme Color"),
                            )
                            .child(div().flex().flex_row().gap(px(6.0)).children(
                                ThemeColorMode::all().iter().map(|mode| {
                                    let mode = *mode;
                                    Self::mode_pill(
                                        ElementId::named_usize("theme-color-mode", mode as usize),
                                        mode.label(),
                                        mode == current_theme_color_mode,
                                        p,
                                        cx.listener(move |this, _ev, _window, cx| {
                                            this.set_theme_color_mode(mode, cx);
                                        }),
                                    )
                                }),
                            )),
                    )
                    // The separating rule. Height is fixed rather than
                    // stretched so it spans the label+buttons pair without
                    // pinning the row's height.
                    .child(div().w(px(1.0)).h(px(44.0)).bg(rgb(p.border_subtle)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p.text))
                                    .child("Mode"),
                            )
                            .child(div().flex().flex_row().gap(px(6.0)).children(
                                ThemeMode::all().iter().map(|mode| {
                                    let mode = *mode;
                                    Self::mode_pill(
                                        ElementId::named_usize("theme-mode", mode as usize),
                                        mode.label(),
                                        mode == current_theme_mode,
                                        p,
                                        cx.listener(move |this, _ev, _window, cx| {
                                            this.set_theme_mode(mode, cx);
                                        }),
                                    )
                                }),
                            )),
                    ),
            )
            // ── Custom Theme (TOML) ──────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .pt(px(12.0))
                    .border_t_1()
                    .border_color(rgb(p.border_subtle))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(p.text))
                            .child("Custom Theme"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.0))
                            .child(Self::mode_pill(
                                "download-theme-template".into(),
                                "Download Theme Template",
                                false,
                                p,
                                cx.listener(|this, _ev, window, cx| {
                                    this.download_theme_template(window, cx);
                                }),
                            ))
                            .child(Self::mode_pill(
                                "import-theme".into(),
                                "Import Theme",
                                false,
                                p,
                                cx.listener(|this, _ev, window, cx| {
                                    this.import_theme(window, cx);
                                }),
                            )),
                    )
                    .when_some(self.theme_import_error.as_ref(), |d, msg| {
                        // Same red/mode pairing as `capture_prompt`'s conflict
                        // warning (dark red is illegible on a light background).
                        d.child(
                            div()
                                .text_xs()
                                .text_color(rgb(match current_theme_mode {
                                    ThemeMode::Dark => 0xf48771,
                                    ThemeMode::Light => 0xb02a15,
                                }))
                                .child(msg.clone()),
                        )
                    }),
            )
    }

    /// Settings -> Fonts: lists every font imported via `font_import.rs`
    /// (each previewed in its own face) with a remove button per entry, plus
    /// an "Add Font" button that opens the same `font_import_modal.rs`
    /// popup the Font Family dropdown's "+ Add Font" row does.
    fn render_fonts(&self, p: crate::theme::Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let names = crate::text_editor::imported_font_names();
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(p.text))
                    .child("Imported Fonts"),
            )
            .child(Self::mode_pill(
                "add-font".into(),
                "Add Font",
                false,
                p,
                cx.listener(|this, _ev, _window, cx| {
                    this.state.update(cx, |s, cx| {
                        s.open_font_import_modal();
                        cx.notify();
                    });
                    cx.notify();
                }),
            ))
            .when(names.is_empty(), |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.text_faint))
                        .child("No fonts imported yet."),
                )
            })
            .children(names.into_iter().enumerate().map(|(idx, name)| {
                let remove_name = name.clone();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .bg(rgb(p.chrome_active))
                    .rounded(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_family(name.clone())
                            .text_color(rgb(p.text))
                            .child(name),
                    )
                    .child(
                        div()
                            .id(ElementId::named_usize("imported-font-remove", idx))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(rgb(p.text_faint))
                            .hover(move |s| s.text_color(rgb(p.text)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.state.update(cx, |s, cx| {
                                        s.remove_imported_font(&remove_name);
                                        cx.notify();
                                    });
                                    cx.notify();
                                }),
                            )
                            .child(crate::icons::icon(
                                crate::icons::Icon::TabClose,
                                p.text_faint,
                                12.0,
                            )),
                    )
                    .into_any_element()
            }))
    }
}

impl Render for SettingsModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders a semi-transparent full-screen backdrop with a centred dialog
         * panel on top.
         *
         * Layout:
         *   • Full-screen dimmed backdrop — clicking it closes the modal
         *   • Centred panel containing:
         *       – Title bar with "Settings" heading and a × close button
         *       – A content row: the section sidebar on the left, the selected
         *         section's pane on the right (see `SettingsSection`)
         *       – Reset to Defaults / Close button row
         *
         * Every color comes from the active theme's `Palette`. It used to be
         * hardcoded VS-Code-dark hex with a `theme_preview ? palette : hex`
         * split, which meant the modal stayed dark-on-dark in light mode —
         * unreadable — unless you happened to be in Theme Preview. The flag
         * now controls layout only (narrower panel, no backdrop dim, no
         * sidebar or keybind list), never color.
         *
         * The panel tracks its own focus handle and listens for key-down
         * events so `start_capture` can claim focus and `handle_capture_key`
         * receives the very next keystroke, regardless of which button was
         * clicked to arm capture.
         */
        let vim_enabled = self.state.read(cx).global_vim().vim_enabled;
        let spellcheck_enabled = self.state.read(cx).preferences().spellcheck_enabled;
        let spellcheck_color = self
            .state
            .read(cx)
            .preferences()
            .spellcheck_underline_color
            .clone();
        let spreading_wpm = self.state.read(cx).preferences().spreading_wpm;
        let speech_time_minutes = {
            let prefs = self.state.read(cx).preferences();
            [
                prefs.speech_time_minutes,
                prefs.speech_time_2_minutes,
                prefs.speech_time_3_minutes,
            ]
        };
        let prep_time_minutes = self.state.read(cx).preferences().prep_time_minutes;
        let nav_fold_buttons = self.state.read(cx).preferences().nav_fold_buttons;
        let timer_panel_enabled = self.state.read(cx).preferences().timer_panel_enabled;
        let search_from_list_enabled = self.state.read(cx).preferences().search_from_list_enabled;
        let search_list_whole_words = self.state.read(cx).preferences().search_list_whole_words;
        let command_palette_enabled = self.state.read(cx).preferences().command_palette_enabled;
        let shrink_points = self.state.read(cx).preferences().small_size_half_points / 2;
        let exception = self
            .state
            .read(cx)
            .preferences()
            .standardize_highlight_exception
            .clone();
        let analytic_color = self.state.read(cx).preferences().analytic_color.clone();
        // The same colors the HL Color dropdown offers — built-ins plus
        // whatever the user has saved — so the exception can name any highlight
        // actually reachable in the document.
        let custom_highlights: Vec<u32> = self
            .state
            .read(cx)
            .custom_colors(crate::state::CustomColorTarget::Highlight)
            .to_vec();
        let card_size_points: [u16; 5] = {
            let st = self.state.read(cx);
            [
                st.card_size_half_points(CardStyleKind::Pocket) / 2,
                st.card_size_half_points(CardStyleKind::Hat) / 2,
                st.card_size_half_points(CardStyleKind::Block) / 2,
                st.card_size_half_points(CardStyleKind::Tag) / 2,
                st.preferences().cite_size_half_points / 2,
            ]
        };
        let (
            emphasis,
            emphasis_change_size,
            emphasis_size_points,
            paste_condense,
            paste_condense_pilcrow,
        ) = {
            let st = self.state.read(cx);
            (
                (
                    st.preferences().emphasis_bold,
                    st.preferences().emphasis_underline,
                    st.preferences().emphasis_box,
                ),
                st.preferences().emphasis_change_size,
                st.preferences().emphasis_size_half_points / 2,
                st.preferences().paste_condense,
                st.preferences().paste_condense_pilcrow,
            )
        };
        let current_theme = self.state.read(cx).preferences().theme;
        let current_theme_mode = self.state.read(cx).preferences().theme_mode;
        let current_theme_color_mode = self.state.read(cx).preferences().theme_color_mode;
        let keybinds = self.state.read(cx).keybinds().clone();
        let vim_keybinds = self.state.read(cx).global_vim().vim_keybinds.clone();
        let p = self.state.read(cx).current_palette();
        let theme_preview = self.theme_preview;
        let section = self.section;

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(black().opacity(if theme_preview { 0.0 } else { 0.55 }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _ev, window, cx| {
                    this.close(window, cx);
                }),
            )
            // Stops wheel events over the modal (including its padding, not
            // just the inner scrollable list) from bubbling to the document
            // editor underneath.
            .on_scroll_wheel(|_ev, _window, cx| cx.stop_propagation())
            .child(
                div()
                    .id("settings-panel")
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::handle_capture_key))
                    // Wide enough for the sidebar plus a keybind row's
                    // label/combo/Change trio without the combo chip
                    // wrapping.
                    .w(px(if theme_preview { 380.0 } else { 860.0 }))
                    .h(px(if theme_preview { 420.0 } else { 620.0 }))
                    .bg(rgb(p.chrome))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    // Stops the mouse-down from bubbling up to the backdrop's
                    // close handler above. A plain no-op handler here does
                    // NOT do this by itself — GPUI mouse events keep bubbling
                    // through every ancestor's on_mouse_down unless one of
                    // them explicitly calls stop_propagation, exactly like
                    // keyboard dispatch. Without this, every click anywhere
                    // in the panel (Change buttons, category headers, the
                    // vim toggle, Reset) closed the modal.
                    .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
                    // ── Title bar ────────────────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .flex_none()
                            .px(px(20.0))
                            .py(px(14.0))
                            .border_b_1()
                            .border_color(rgb(p.border_subtle))
                            .when(theme_preview, |d| {
                                d.child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(10.0))
                                        .child(
                                            div()
                                                .id("settings-preview-back")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .w(px(28.0))
                                                .h(px(28.0))
                                                .rounded(px(4.0))
                                                .cursor_pointer()
                                                .text_color(rgb(p.text_muted))
                                                .bg(rgb(p.chrome_active))
                                                .border_1()
                                                .border_color(rgb(p.border_subtle))
                                                .hover(move |s| {
                                                    s.bg(rgb(p.chrome_hover))
                                                        .text_color(rgb(p.text))
                                                })
                                                .active(move |s| s.bg(rgb(p.chrome_active)))
                                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                                    this.exit_theme_preview(cx);
                                                }))
                                                .child("‹"),
                                        )
                                        .child(
                                            div()
                                                .text_color(rgb(p.text))
                                                .font_weight(FontWeight::BOLD)
                                                .child("Theme Preview"),
                                        ),
                                )
                            })
                            .when(!theme_preview, |d| {
                                d.child(
                                    div()
                                        .text_color(rgb(p.text))
                                        .font_weight(FontWeight::BOLD)
                                        .child("Settings"),
                                )
                                .child(
                                    div()
                                        .id("settings-close-x")
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .w(px(28.0))
                                        .h(px(28.0))
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .text_color(rgb(p.text_muted))
                                        .bg(rgb(p.chrome_active))
                                        .hover(move |s| {
                                            s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text))
                                        })
                                        .on_click(cx.listener(|this, _ev, window, cx| {
                                            this.close(window, cx);
                                        }))
                                        .child(crate::icons::icon(
                                            crate::icons::Icon::TabClose,
                                            p.text_muted,
                                            16.0,
                                        )),
                                )
                            }),
                    )
                    // ── Sidebar │ selected pane ──────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_1()
                            .min_h_0()
                            .when(!theme_preview, |d| d.child(self.render_sidebar(p, cx)))
                            .child(
                                div()
                                    .id("settings-body-scroll")
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.0))
                                    .p(px(20.0))
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    // Theme Preview is Appearance-only, with
                                    // no sidebar to choose anything else.
                                    .when(
                                        theme_preview || section == SettingsSection::Appearance,
                                        |d| {
                                            d.child(self.render_appearance(
                                                current_theme,
                                                current_theme_mode,
                                                current_theme_color_mode,
                                                theme_preview,
                                                p,
                                                cx,
                                            ))
                                        },
                                    )
                                    .when(
                                        !theme_preview && section == SettingsSection::TextSettings,
                                        |d| {
                                            d.child(self.render_text_settings(
                                                emphasis,
                                                emphasis_change_size,
                                                emphasis_size_points,
                                                shrink_points,
                                                card_size_points,
                                                exception.clone(),
                                                custom_highlights.clone(),
                                                analytic_color.clone(),
                                                paste_condense,
                                                paste_condense_pilcrow,
                                                p,
                                                cx,
                                            ))
                                        },
                                    )
                                    .when(
                                        !theme_preview && section == SettingsSection::Fonts,
                                        |d| d.child(self.render_fonts(p, cx)),
                                    )
                                    .when(
                                        !theme_preview && section == SettingsSection::Keybindings,
                                        |d| {
                                            d.children(KeybindCategory::all().iter().map(
                                                |category| {
                                                    self.render_category(
                                                        *category,
                                                        &keybinds,
                                                        p,
                                                        current_theme_mode,
                                                        cx,
                                                    )
                                                },
                                            ))
                                            .when(
                                                vim_enabled,
                                                |d| {
                                                    d.child(self.render_vim_keybinds_section(
                                                        &vim_keybinds,
                                                        p,
                                                        current_theme_mode,
                                                        cx,
                                                    ))
                                                },
                                            )
                                        },
                                    )
                                    .when(
                                        !theme_preview && section == SettingsSection::Timer,
                                        |d| {
                                            d.child(self.render_timer_settings(
                                                speech_time_minutes,
                                                prep_time_minutes,
                                                timer_panel_enabled,
                                                p,
                                                cx,
                                            ))
                                        },
                                    )
                                    .when(
                                        !theme_preview
                                            && section == SettingsSection::ToggleFeatures,
                                        |d| {
                                            d.child(self.render_toggle_features(
                                                vim_enabled,
                                                spellcheck_enabled,
                                                spellcheck_color.clone(),
                                                spreading_wpm,
                                                nav_fold_buttons,
                                                search_from_list_enabled,
                                                search_list_whole_words,
                                                command_palette_enabled,
                                                p,
                                                cx,
                                            ))
                                        },
                                    ),
                            ),
                    )
                    // ── Bottom button row ────────────────────────────────────
                    .when(!theme_preview, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_between()
                                .flex_none()
                                .px(px(20.0))
                                .py(px(12.0))
                                .border_t_1()
                                .border_color(rgb(p.border_subtle))
                                // closed_beta_plan.md §3: always-visible build
                                // string so a tester reporting a bug can read
                                // off exactly what build they're on.
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.text_faint))
                                        .child(build_version_string()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .gap(px(8.0))
                                        .child(
                                            div()
                                                .id("settings-reset-btn")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .px(px(16.0))
                                                .py(px(6.0))
                                                .bg(rgb(p.chrome_active))
                                                .rounded(px(4.0))
                                                .cursor_pointer()
                                                .text_sm()
                                                .text_color(rgb(p.text))
                                                .border_1()
                                                .border_color(rgb(p.border))
                                                .hover(move |s| s.bg(rgb(p.chrome_hover)))
                                                .on_click(cx.listener(|this, _ev, _window, cx| {
                                                    this.reset_to_defaults(cx);
                                                }))
                                                .child("Reset to Defaults"),
                                        )
                                        .child(
                                            div()
                                                .id("settings-close-btn")
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .px(px(16.0))
                                                .py(px(6.0))
                                                .bg(rgb(p.accent))
                                                .rounded(px(4.0))
                                                .cursor_pointer()
                                                .text_sm()
                                                // Fixed light label: the accent
                                                // is a saturated mid-tone in
                                                // both modes, while `p.text`
                                                // inverts and would disappear
                                                // on it in light mode.
                                                .text_color(rgb(0xffffff))
                                                .hover(move |s| s.bg(rgb(p.accent_strong)))
                                                .on_click(cx.listener(|this, _ev, window, cx| {
                                                    this.close(window, cx);
                                                }))
                                                .child("Close"),
                                        ),
                                ),
                        )
                    }),
            )
    }
}
