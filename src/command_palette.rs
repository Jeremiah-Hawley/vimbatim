/*
 * The command palette (checklist: Settings -> Command Palette): a small text
 * box under the ribbon that fuzzy-searches every command in Vimbatim and runs
 * the top one on Enter, or whichever row is clicked.
 *
 * This file holds both halves because they belong together: the registry is a
 * static table with no `AppState` coupling of its own, so putting it in
 * `state.rs` (which is deliberately gpui-free but is also 16k lines about
 * document state) would separate it from the only thing that reads it.
 */

use gpui::prelude::*;
use gpui::*;

use crate::document_ops::FormatOp;
use crate::docx_parser::Alignment;
use crate::keybinds::{action_for, KeybindAction};
use crate::state::AppState;
use crate::theme::{radius, space};

/// How a palette entry actually runs.
///
/// Three variants rather than one boxed closure because they need genuinely
/// different things at call time: a `KeybindAction` goes through GPUI's
/// dispatch (and can display its current key combo), a URL needs `App`, and
/// everything else is a plain function of `&mut AppState`.
///
/// `State` is what makes the "every command in the app" scope tractable:
/// non-capturing closures coerce to `fn` pointers, so a ribbon format op, a
/// Doc/Card menu row, and a settings toggle are all the same shape —
/// `AppState::delete_tags` already had it, in the ribbon's own menu tables.
#[derive(Clone, Copy)]
pub enum PaletteAction {
    Keybind(KeybindAction),
    State(fn(&mut AppState)),
    Url(&'static str),
}

pub struct PaletteCommand {
    pub label: &'static str,
    pub action: PaletteAction,
}

/// Every command the palette can run, in registry order.
///
/// Assembled in four blocks, **first-wins on duplicate labels**:
///   1. `KeybindAction`s — first so they keep their entry (they're the only
///      ones that can show a key combo beside the name).
///   2. The ribbon's Doc Menu and Card Menu rows.
///   3. Ribbon commands with no `KeybindAction` equivalent.
///   4. Settings toggles.
///
/// The duplicates are real and three deep: `Condense` is a `KeybindAction`, a
/// `FormatAction`, *and* the Card Menu's "Condense, no pilcrows" — all calling
/// `condense_selection`. `SelectSimilarFormatting` is both a keybind and a Doc
/// Menu row. `no_duplicate_labels` is the test that keeps this honest as the
/// ribbon grows.
///
/// Block 3 is a deliberate **allow-list**, not "every `FormatAction` minus
/// exclusions": an exclude-list would silently turn each newly added ribbon
/// variant into a dead palette entry, whereas this fails closed. Left out on
/// purpose:
///   • the six dropdown *openers* (Doc/Card/SwitchTab menus, Font Family,
///     Font Color, HL Color) — each is a container whose contents are the real
///     commands, and its panel is anchored to a ribbon button that the palette
///     isn't;
///   • `CollapseAll`, `Body`, `PocketCite`, `OpenBlock`, `CloseBlock` — enum
///     variants referenced nowhere in the codebase: no button, no handler;
///   • `PrintLayout` — deliberately inert (its ribbon button is commented out
///     pending the deferred print-layout work);
///   • `FontSize`, which is a typable spinner rather than a command;
///   • `Timer`, whose ribbon button flips the same `timer.visible` the
///     `StartTimer` keybind does — a duplicate the label dedupe cannot catch,
///     because the two spell the same command differently ("Timer" vs
///     "Toggle Timer"). Same-behaviour-different-label is the one duplication
///     this registry has to catch by reading, not by test.
pub fn registry() -> Vec<PaletteCommand> {
    let mut out: Vec<PaletteCommand> = Vec::new();

    // ── 1. Bindable actions ────────────────────────────────────────────────
    for action in KeybindAction::all() {
        // `CiteFromLink` is still a `println!` no-op, and the palette itself
        // is not a thing to run from inside the palette.
        if action.is_stub() || *action == KeybindAction::CommandPalette {
            continue;
        }
        out.push(PaletteCommand { label: action.label(), action: PaletteAction::Keybind(*action) });
    }

    // ── 2. Doc Menu / Card Menu rows ───────────────────────────────────────
    // Same labels and same `fn(&mut AppState)` the ribbon's own menus use.
    let menu_rows: &[(&'static str, fn(&mut AppState))] = &[
        ("Delete analytics", AppState::delete_analytics),
        ("Convert analytics to tags", AppState::convert_analytics_to_tags),
        ("Remove emphasis", AppState::remove_emphasis),
        ("Remove non highlighted underlining", AppState::remove_non_highlighted_underlining),
        ("Remove blank lines", AppState::remove_blank_lines),
        ("Remove pilcrows", AppState::remove_pilcrows),
        ("Condense, no pilcrows", AppState::condense_selection),
        ("Condense, pilcrows", AppState::condense_with_pilcrows),
        ("Uncondensed", AppState::uncondense_selection),
        ("Standardize highlighting", AppState::standardize_highlighting),
        ("Standardize highlighting with exception", AppState::standardize_highlighting_with_exception),
    ];
    for (label, run) in menu_rows {
        out.push(PaletteCommand { label, action: PaletteAction::State(*run) });
    }

    // ── 3. Ribbon commands with no keybind of their own ────────────────────
    let ribbon: &[(&'static str, fn(&mut AppState))] = &[
        ("Bullet List", AppState::apply_bullet_list),
        ("Numbered List", AppState::apply_numbered_list),
        ("Italics", |s| s.apply_formatting_to_selection(FormatOp::Italic(true))),
        ("Strikethrough", AppState::toggle_strikethrough),
        ("Normal Size", |s| s.apply_formatting_to_selection(FormatOp::FontSize(24))),
        ("Change Case: Sentence", |s| {
            s.apply_case_to_selection(crate::case_converter::CaseType::Sentence)
        }),
        ("Change Case: lower", |s| {
            s.apply_case_to_selection(crate::case_converter::CaseType::Lower)
        }),
        ("Change Case: UPPER", |s| {
            s.apply_case_to_selection(crate::case_converter::CaseType::Upper)
        }),
        ("Change Case: Capitalize Each Word", |s| {
            s.apply_case_to_selection(crate::case_converter::CaseType::Title)
        }),
        ("Change Case: tOGGLE cASE", |s| {
            s.apply_case_to_selection(crate::case_converter::CaseType::Toggle)
        }),
        ("Align Left", |s| s.apply_line_alignment(Alignment::Left)),
        ("Align Center", |s| s.apply_line_alignment(Alignment::Center)),
        ("Align Right", |s| s.apply_line_alignment(Alignment::Right)),
        ("Highlight Yellow", |s| {
            s.apply_formatting_to_selection(FormatOp::Highlight(Some("yellow".to_string())))
        }),
        ("Highlight Green", |s| {
            s.apply_formatting_to_selection(FormatOp::Highlight(Some("green".to_string())))
        }),
        ("Remove Highlight", |s| s.apply_formatting_to_selection(FormatOp::Highlight(None))),
        ("Fold / Unfold All", AppState::toggle_fold),
        ("Toggle Invisibility Mode", AppState::toggle_invisibility_mode),
        // Matches the ribbon's Nav button: switches the sidebar's mode *and*
        // makes sure the sidebar is actually showing.
        ("Toggle Navigation Sidebar", |s| {
            s.sidebar_mode = match s.sidebar_mode {
                crate::state::SidebarMode::Files => crate::state::SidebarMode::Nav,
                crate::state::SidebarMode::Nav => crate::state::SidebarMode::Files,
            };
            s.sidebar_visible = true;
        }),
        ("Toggle Split View", |s| {
            if s.split_view {
                s.close_split();
            } else {
                s.open_split();
            }
        }),
        ("Search From List", AppState::open_search_from_list),
    ];
    for (label, run) in ribbon {
        out.push(PaletteCommand { label, action: PaletteAction::State(*run) });
    }

    // ── 4. Settings toggles ────────────────────────────────────────────────
    // Every one of these routes through an `AppState` method that flips *and*
    // persists. Flipping the flag directly here would look right and silently
    // fail to survive a restart.
    let toggles: &[(&'static str, fn(&mut AppState))] = &[
        ("Toggle Vim Mode", AppState::toggle_vim),
        ("Toggle Spellcheck", AppState::toggle_spellcheck),
        ("Toggle Search From List Feature", AppState::toggle_search_from_list),
        ("Toggle Whole-Word List Search", AppState::toggle_search_list_whole_words),
        ("Toggle Navigation Heading Fold Buttons", AppState::toggle_nav_fold_buttons),
        ("Toggle Paragraph Integrity", AppState::toggle_paragraph_integrity),
        ("Toggle Pilcrows", AppState::toggle_pilcrows),
    ];
    for (label, run) in toggles {
        out.push(PaletteCommand { label, action: PaletteAction::State(*run) });
    }

    // ── 5. Links ───────────────────────────────────────────────────────────
    out.push(PaletteCommand { label: "Open opencaselist", action: PaletteAction::Url("https://opencaselist.com/") });
    out.push(PaletteCommand {
        label: "Open Tabroom",
        action: PaletteAction::Url("https://www.tabroom.com/index/index.mhtml"),
    });

    // First-wins dedupe. Order matters: block 1 gets to keep its entry so the
    // command shows its key combo.
    let mut seen: Vec<&'static str> = Vec::new();
    out.retain(|c| {
        let fresh = !seen.contains(&c.label);
        if fresh {
            seen.push(c.label);
        }
        fresh
    });
    out
}

/// Scores `label` against a fuzzy `query` — every query character must appear
/// in `label`, in order, case-insensitively (`cf` → "Clear Formatting").
/// `None` means no match.
///
/// Higher is better. The weighting, in descending influence:
///   • an exact prefix of the whole label (typing "sav" for "Save")
///   • each character landing at a word start ("cf" hitting *C*lear
///     *F*ormatting, which is the whole point of initials-style querying)
///   • runs of consecutive characters
///   • an early first match
///
/// Deliberately a small hand-rolled scorer rather than a fuzzy-match crate:
/// the whole registry is a few dozen short labels, so the ranking only has to
/// be *sensible*, and a dependency for that is not a trade worth making.
pub fn fuzzy_score(query: &str, label: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let l: Vec<char> = label.chars().collect();
    let lower: Vec<char> = label.chars().flat_map(|c| c.to_lowercase()).collect();
    // `to_lowercase` can yield more than one char for some scripts; the
    // labels here are ASCII, and this guard keeps the index pairing honest
    // rather than silently misaligning if that ever changes.
    if lower.len() != l.len() {
        return label.to_lowercase().contains(&query.to_lowercase()).then_some(0);
    }

    let mut score = 0;
    let mut qi = 0;
    let mut first_match: Option<usize> = None;
    let mut prev_hit: Option<usize> = None;

    for (i, &c) in lower.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if c != q[qi] {
            continue;
        }
        if first_match.is_none() {
            first_match = Some(i);
        }
        // Word start: the first character, or one preceded by a separator.
        let at_word_start = i == 0 || matches!(l[i - 1], ' ' | '-' | '/' | '(' | ',' | '&');
        if at_word_start {
            score += 12;
        }
        if prev_hit == Some(i.wrapping_sub(1)) {
            score += 6;
        }
        prev_hit = Some(i);
        qi += 1;
    }

    if qi < q.len() {
        return None;
    }
    if lower.starts_with(&q[..]) {
        score += 40;
    }
    // Earlier first match wins, and shorter labels win among equals — both
    // small enough not to override a word-start match.
    score -= first_match.unwrap_or(0) as i32;
    score -= (l.len() / 8) as i32;
    Some(score)
}

/// The registry filtered and ranked for `query`.
///
/// Sorted by `(score desc, label asc)` — a **total** order. Enter runs the top
/// row, so a tie that reordered between frames would make "the top command"
/// genuinely unpredictable.
pub fn filtered(query: &str) -> Vec<PaletteCommand> {
    let mut scored: Vec<(i32, PaletteCommand)> = registry()
        .into_iter()
        .filter_map(|c| fuzzy_score(query, c.label).map(|s| (s, c)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.label.cmp(b.1.label)));
    scored.into_iter().map(|(_, c)| c).collect()
}

/// How many rows the panel shows at once. Enter runs the first of them, so
/// this is a display cap, not a search cap — the "N more…" line below the
/// list reports everything the query actually matched.
const MAX_ROWS: usize = 5;

/// The command palette panel — a query box plus its ranked results, mounted
/// under the ribbon in the same slot as the find bar (the two are mutually
/// exclusive; see `AppState::open_command_palette`).
///
/// Like `find_bar.rs`, this claims focus and interprets key-down itself:
/// GPUI ships no text input, and this codebase's three existing text-capture
/// surfaces (settings keybind capture, vim's `:` line, the find bar) all take
/// the same approach.
pub struct CommandPaletteView {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
}

impl CommandPaletteView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        CommandPaletteView { state, focus_handle: cx.focus_handle() }
    }

    /// Runs `command` and closes the palette.
    ///
    /// Closing happens **first**: several commands (Toggle Settings, Find,
    /// Search From List) open a surface of their own, and running them from an
    /// open palette would otherwise leave that surface behind a panel still
    /// sitting on top of it.
    fn activate(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.close_command_palette();
            cx.notify();
        });
        match action {
            PaletteAction::Keybind(a) => window.dispatch_action(action_for(a), cx),
            PaletteAction::State(run) => {
                self.state.update(cx, |s, cx| {
                    run(s);
                    cx.notify();
                });
            }
            PaletteAction::Url(url) => cx.open_url(url),
        }
        cx.notify();
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let key = ks.key.as_str();

        // Ctrl/Cmd combos fall through to the global keymap, so the app's own
        // shortcuts still work while the palette has focus.
        if ks.modifiers.control || ks.modifiers.platform {
            return;
        }

        match key {
            "escape" => {
                self.state.update(cx, |s, cx| {
                    s.close_command_palette();
                    cx.notify();
                });
            }
            "enter" => {
                // Per the spec, Enter runs "the top recommended command" —
                // there is no separate selection to track.
                let query = self.query(cx);
                if let Some(top) = filtered(&query).into_iter().next() {
                    self.activate(top.action, window, cx);
                }
                return;
            }
            "backspace" => {
                self.state.update(cx, |s, cx| {
                    if let Some(p) = s.command_palette.as_mut() {
                        p.query.pop();
                    }
                    cx.notify();
                });
            }
            _ => {
                let Some(ch) = crate::state::vim_find_target_char(key, ks.modifiers.shift, ks.key_char.as_deref())
                else {
                    return;
                };
                self.state.update(cx, |s, cx| {
                    if let Some(p) = s.command_palette.as_mut() {
                        p.query.push(ch);
                    }
                    cx.notify();
                });
            }
        }
        self.focus_handle.clone().focus(window, cx);
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.state.read(cx).command_palette.as_ref().map(|p| p.query.clone()).unwrap_or_default()
    }
}

impl Render for CommandPaletteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(palette) = self.state.read(cx).command_palette.clone() else {
            return div().into_any_element();
        };
        let p = self.state.read(cx).current_palette();
        let keybinds = self.state.read(cx).keybinds.clone();

        // Opened by a keybind and usable only from the keyboard, so it takes
        // focus on the frame it appears — same as the find bar.
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.clone().focus(window, cx);
        }

        let results = filtered(&palette.query);
        let total = results.len();

        div()
            .id("command-palette")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::handle_key_down))
            // Clicks inside must not reach the editor underneath, which would
            // move the caret and take focus back.
            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            // Width comes from the wrapper in `main_window.rs` (half the
            // ribbon), not from a constant here — the panel just fills what
            // it's given, so the two can't disagree about how wide it is.
            .w_full()
            .bg(rgb(p.chrome))
            .border_1()
            .border_color(rgb(p.border))
            .rounded(px(radius::MD))
            .shadow_lg()
            .p(px(space::SM))
            .gap(px(space::XS))
            // ── Query box ──────────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(26.0))
                    .px(px(space::SM))
                    .rounded(px(radius::MD))
                    .bg(rgb(p.editor_bg))
                    .border_1()
                    .border_color(rgb(p.accent))
                    .text_sm()
                    .text_color(rgb(p.text))
                    .child(if palette.query.is_empty() {
                        div().text_color(rgb(p.text_faint)).child("Search commands…").into_any_element()
                    } else {
                        div().child(palette.query.clone()).into_any_element()
                    })
                    // Block caret, so an empty box still shows where typing lands.
                    .child(div().w(px(1.0)).h(px(14.0)).ml(px(1.0)).bg(rgb(p.text))),
            )
            .when(total == 0, |d| {
                d.child(
                    div()
                        .px(px(space::SM))
                        .py(px(space::XS))
                        .text_xs()
                        .text_color(rgb(p.text_faint))
                        .child("No matching command"),
                )
            })
            // ── Results ────────────────────────────────────────────────────
            .children(results.into_iter().take(MAX_ROWS).enumerate().map(|(i, command)| {
                // The top row is what Enter runs, so it reads as selected.
                let is_top = i == 0;
                let action = command.action;
                // Only a bindable action has a combo to show.
                let combo = match action {
                    PaletteAction::Keybind(a) => {
                        let c = keybinds.get(a);
                        (!c.is_unbound()).then(|| c.display_string())
                    }
                    _ => None,
                };

                div()
                    .id(ElementId::named_usize("palette-row", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(space::SM))
                    .h(px(24.0))
                    .px(px(space::SM))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .text_sm()
                    .when(is_top, |d| d.bg(rgb(p.accent_wash)).text_color(rgb(p.text)))
                    .when(!is_top, |d| {
                        d.text_color(rgb(p.text_muted))
                            .hover(move |s| s.bg(rgb(p.chrome_hover)).text_color(rgb(p.text)))
                    })
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.activate(action, window, cx);
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(command.label))
                    .when_some(combo, |d, combo| {
                        d.child(div().flex_none().text_xs().text_color(rgb(p.text_faint)).child(combo))
                    })
            }))
            .when(total > MAX_ROWS, |d| {
                d.child(
                    div()
                        .px(px(space::SM))
                        .text_xs()
                        .text_color(rgb(p.text_faint))
                        .child(format!("{} more…", total - MAX_ROWS)),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Only what's needed — `use gpui::*` at module scope shadows std's
    // `#[test]` with gpui's own async-test macro (same trap `text_editor.rs`
    // and `file_explorer.rs` document in their own test modules).
    use super::{filtered, fuzzy_score, registry, PaletteAction};
    use crate::keybinds::KeybindAction;

    #[test]
    fn registry_has_no_duplicate_labels() {
        // The dedupe is load-bearing: Condense reaches the registry three
        // separate ways (keybind, ribbon, Card Menu row) and would otherwise
        // appear three times.
        let commands = registry();
        let mut labels: Vec<&str> = commands.iter().map(|c| c.label).collect();
        labels.sort_unstable();
        let mut deduped = labels.clone();
        deduped.dedup();
        assert_eq!(labels, deduped, "the palette registry lists a command twice");
    }

    #[test]
    fn registry_is_populated_and_every_keybind_entry_is_real() {
        let commands = registry();
        assert!(commands.len() > 40, "registry looks truncated: {}", commands.len());
        for c in &commands {
            if let PaletteAction::Keybind(a) = c.action {
                assert!(KeybindAction::all().contains(&a), "{a:?} is not a real action");
                assert!(!a.is_stub(), "{a:?} is a stub and must not be listed");
                assert_ne!(a, KeybindAction::CommandPalette, "the palette must not list itself");
            }
            assert!(!c.label.is_empty());
        }
    }

    #[test]
    fn registry_covers_all_four_buckets() {
        // Guards against a block being dropped wholesale in a refactor —
        // "everything in Vimbatim" is the point of this registry.
        let commands = registry();
        let has = |label: &str| commands.iter().any(|c| c.label == label);
        assert!(has("Save"), "bindable actions missing");
        assert!(has("Remove blank lines"), "Doc/Card menu rows missing");
        assert!(has("Align Center"), "ribbon-only commands missing");
        assert!(has("Toggle Spellcheck"), "settings toggles missing");
        assert!(has("Open Tabroom"), "links missing");
        // Bug report: Open File, Open Folder and Switch Active Pane were
        // reachable only from the toolbar button / not reachable at all
        // (pane switching had no method), missing from both the palette and
        // Settings → Keybinds. Fixed by making them real `KeybindAction`
        // variants, which block 1's loop over `KeybindAction::all()` above
        // already surfaces automatically — this pins that down.
        assert!(has("Open File"), "Open File missing from the palette");
        assert!(has("Open Folder"), "Open Folder missing from the palette");
        assert!(has("Switch Active Pane"), "Switch Active Pane missing from the palette");
        assert!(has("New File"), "New File missing from the palette");
        assert!(has("Refresh File Tree"), "Refresh File Tree missing from the palette");
    }

    #[test]
    fn fuzzy_matches_initials_and_ranks_them_first() {
        // The headline case: "cf" should find Clear Formatting, and rank it
        // above a label that merely happens to contain a c before an f.
        let ranked = filtered("cf");
        assert_eq!(ranked.first().map(|c| c.label), Some("Clear Formatting"));
    }

    #[test]
    fn fuzzy_matches_a_run_inside_one_word() {
        // A contiguous run beats scattered initials: "hili" sits inside
        // "Highlight" as one block.
        assert_eq!(filtered("hili").first().map(|c| c.label), Some("Highlight"));
    }

    #[test]
    fn fuzzy_prefers_an_exact_prefix() {
        // "save" is a prefix of both "Save" and "Save As"; the exact one wins
        // on the length tiebreak.
        assert_eq!(filtered("save").first().map(|c| c.label), Some("Save"));
    }

    #[test]
    fn fuzzy_requires_the_letters_in_order() {
        assert!(fuzzy_score("fc", "Clear Formatting").is_none());
        assert!(fuzzy_score("cf", "Clear Formatting").is_some());
    }

    #[test]
    fn empty_query_returns_the_whole_registry() {
        assert_eq!(filtered("").len(), registry().len());
    }

    #[test]
    fn a_query_matching_nothing_returns_no_rows() {
        assert!(filtered("zzzqqqxxx").is_empty());
    }

    #[test]
    fn ranking_is_a_total_order_so_the_top_row_is_stable() {
        // Enter runs row 0; if ties reordered between frames, "the top
        // recommended command" would be unpredictable.
        let a: Vec<&str> = filtered("to").iter().map(|c| c.label).collect();
        let b: Vec<&str> = filtered("to").iter().map(|c| c.label).collect();
        assert_eq!(a, b);
        assert!(a.windows(2).all(|w| w[0] != w[1]));
    }
}
