use gpui::prelude::*;
use gpui::*;

use crate::document_ops::FormatOp;
use crate::state::AppState;

/// Formatting operations mirroring the Verbatim debate-speech Word extension.
///
/// Each variant corresponds to one ribbon button. MARKUP/CLEAN/SIZE actions
/// are wired to real `apply_formatting_to_selection` calls (rich-text
/// formatting plan, Phase 2) via `to_format_op`. CARD STYLES/STRUCTURE
/// remain a `println!` stub — **documented, out-of-scope gap**: they aren't
/// character formatting at all (they're paragraph-level named styles and
/// structural markers), and `editor_instructions.md` never defines what a
/// named card style actually looks like or what a "block marker" inserts
/// beyond the button labels themselves. Wiring them would mean inventing
/// that design from scratch rather than implementing a written spec —
/// deferred, same as `vim_todo.md`'s documented `R` (Replace mode) gap was
/// before it got its own explicit design decision.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum FormatAction {
    // ── Card style presets ──────────────────────────────────────────────────
    Tag,        // Bold header label for the evidence card
    Cite,       // Author / date / source citation line
    Body,       // Standard card body text
    Pocket,     // Condensed format for pocket-round documents
    PocketCite, // Citation line in pocket format
    // ── Run-level markup ────────────────────────────────────────────────────
    Underline,       // Sub-mark: underline key words to read in round
    Italic,          // Italic emphasis (extended scope beyond spec 7.1's literal button list)
    HighlightYellow, // Primary in-round read mark
    HighlightGreen,  // Best-evidence emphasis mark
    RemoveHighlight, // Strip all highlight from selection
    Bold,            // Bold emphasis
    Clean,           // Remove all character formatting from selection
    // ── Document structure ──────────────────────────────────────────────────
    OpenBlock,  // Begin a labelled speech block
    CloseBlock, // End a speech block
    // ── Size utilities ──────────────────────────────────────────────────────
    Shrink,     // Decrement font size by one step
    NormalSize, // Reset to default card body size
}

impl FormatAction {
    /// Maps this action to the `FormatOp` `apply_formatting_to_selection`
    /// understands, or `None` for the still-stubbed CARD STYLES/STRUCTURE
    /// actions (see this enum's own doc comment).
    ///
    /// `Shrink`/`NormalSize`'s point sizes should come from
    /// `settings.conf`'s `small_size`/`large_size` fields (spec 7.1) —
    /// those aren't wired into `AppState` yet (a separate, pre-existing,
    /// already-tracked gap — settings.conf's `vim` flag has the same
    /// problem), so these use reasonable fixed defaults instead
    /// (documented here, not silently invented) until that wiring exists.
    fn to_format_op(&self) -> Option<FormatOp> {
        match self {
            FormatAction::Underline => Some(FormatOp::Underline(true)),
            FormatAction::Italic => Some(FormatOp::Italic(true)),
            FormatAction::HighlightYellow => Some(FormatOp::Highlight(Some("yellow".to_string()))),
            FormatAction::HighlightGreen => Some(FormatOp::Highlight(Some("green".to_string()))),
            FormatAction::RemoveHighlight => Some(FormatOp::Highlight(None)),
            FormatAction::Bold => Some(FormatOp::Bold(true)),
            FormatAction::Clean => Some(FormatOp::ClearAll),
            FormatAction::Shrink => Some(FormatOp::FontSize(20)),     // 10pt, in half-points
            FormatAction::NormalSize => Some(FormatOp::FontSize(24)), // 12pt, in half-points
            FormatAction::Tag
            | FormatAction::Cite
            | FormatAction::Body
            | FormatAction::Pocket
            | FormatAction::PocketCite
            | FormatAction::OpenBlock
            | FormatAction::CloseBlock => None,
        }
    }
}

/// Visual accent applied to a button's background and foreground colours.
#[derive(Clone, Copy)]
enum BtnTheme {
    Default,  // Standard dark button
    Tag,      // Blue — card label
    Cite,     // Muted gray — citation
    YellowHL, // Gold — yellow highlight
    GreenHL,  // Green — second-pass highlight
}

/// Static metadata for a single ribbon button.
#[allow(dead_code)]
struct RibbonBtn {
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    action: FormatAction,
    theme: BtnTheme,
}

/// The debate-focused Verbatim-style formatting ribbon.
///
/// Displays five labelled button groups in a single horizontal bar:
///   CARD STYLES | MARKUP | CLEAN | STRUCTURE | SIZE
///
/// Layout mirrors the Verbatim Word macro ribbon used in competitive policy
/// and parliamentary debate to format evidence cards and speech documents.
/// The ribbon height is fixed at 52 px to accommodate category labels above buttons.
pub struct FormattingRibbon {
    state: Entity<AppState>,
}

impl FormattingRibbon {
    pub fn new(state: Entity<AppState>) -> Self {
        /*
         * Buttons call into the shared AppState to actually apply
         * formatting (rich-text formatting plan, Phase 2), matching how
         * every other view (TabBar, AppToolbar, FileExplorer,
         * SettingsModal) holds the same shared entity.
         */
        FormattingRibbon { state }
    }

    /// Returns the (background_hex, foreground_hex) colour pair for a button theme.
    fn btn_colors(theme: BtnTheme) -> (u32, u32) {
        /*
         * Each theme maps to a (background, text) hex colour pair. Accent colours
         * give visual affordance about the operation each button performs:
         *   Tag      → blue,  matching the common blue card-tag convention
         *   Cite     → muted gray, de-emphasised since citations are secondary
         *   YellowHL → gold,  previewing the yellow highlight it applies
         *   GreenHL  → green, previewing the green highlight it applies
         */
        match theme {
            BtnTheme::Default  => (0x3c3c3c, 0xd4d4d4),
            BtnTheme::Tag      => (0x1a3c5c, 0x569cd6),
            BtnTheme::Cite     => (0x363636, 0x909090),
            BtnTheme::YellowHL => (0x4a3c00, 0xffd700),
            BtnTheme::GreenHL  => (0x1a3c1a, 0x4ec94e),
        }
    }

    /// Renders a labelled ribbon group: a small category header above a row of buttons.
    fn render_group(label: &'static str, btns: Vec<RibbonBtn>, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Builds a flex-col div containing:
         *   1. A tiny all-caps category label in muted gray (e.g. "CARD STYLES")
         *   2. A flex-row of styled, clickable buttons
         *
         * `justify_center` vertically centres the label + button pair within the
         * ribbon's fixed 52 px height, matching the visual balance of Word's ribbon.
         *
         * All string data in RibbonBtn is &'static, so no heap allocations are
         * needed for labels or the on_click closure capture.
         *
         * A button whose action maps to a real `FormatOp` (MARKUP/CLEAN/
         * SIZE) calls `apply_formatting_to_selection` via `cx.listener`,
         * same pattern every other view's clickable elements already use.
         * One that doesn't (`to_format_op` returns `None` — CARD STYLES/
         * STRUCTURE, this enum's own doc comment explains why) keeps the
         * original `println!` stub.
         */
        let button_els: Vec<_> = btns.into_iter().map(|btn| {
            let (bg, fg) = Self::btn_colors(btn.theme);
            // &'static str is Copy, so this moves into the closure without borrowing.
            let tip = btn.tooltip;
            let op = btn.action.to_format_op();
            div()
                .id(btn.id)
                .flex()
                .items_center()
                .justify_center()
                .px(px(7.0))
                .h(px(24.0))
                .bg(rgb(bg))
                .rounded(px(3.0))
                .text_color(rgb(fg))
                .text_xs()
                .cursor_pointer()
                .border_1()
                .border_color(rgb(0x505050))
                .mr(px(3.0))
                .on_click(cx.listener(move |this, _ev, _window, cx| {
                    match &op {
                        Some(op) => {
                            this.state.update(cx, |s, cx| {
                                s.apply_formatting_to_selection(op.clone());
                                cx.notify();
                            });
                            cx.notify();
                        }
                        None => println!("[Ribbon] {}", tip),
                    }
                }))
                .child(btn.label)
        }).collect();

        div()
            .flex()
            .flex_col()
            .justify_center()
            .px(px(6.0))
            .gap(px(2.0))
            // Small muted category label above the button row
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x606060))
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .children(button_els),
            )
    }

    /// Renders a 1 px vertical divider between ribbon groups.
    fn separator() -> impl IntoElement {
        /*
         * The outer ribbon uses `items_stretch`, which causes this div to grow
         * to the ribbon's full height automatically — no explicit height needed.
         */
        div()
            .w(px(1.0))
            .mx(px(4.0))
            .bg(rgb(0x464647))
    }
}

impl Render for FormattingRibbon {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        /*
         * Renders the full ribbon as one 52 px horizontal bar divided into five
         * labelled groups separated by 1 px vertical lines.
         *
         * `items_stretch` on the outer flex row causes both group divs and
         * separators to fill the ribbon height without needing explicit heights.
         *
         * Group order mirrors the Verbatim Word ribbon:
         *   CARD STYLES — per-card formatting presets (Tag, Cite, Body, Pocket)
         *   MARKUP      — run-level marks applied during evidence preparation
         *   CLEAN       — remove markup from the current selection
         *   STRUCTURE   — open and close speech blocks
         *   SIZE        — quick font-size step utilities
         */
        div()
            .flex()
            .flex_row()
            // items_stretch lets all children (groups + separators) fill the 52 px height.
            .items_stretch()
            .w_full()
            .h(px(52.0))
            .px(px(8.0))
            .bg(rgb(0x2d2d2e))
            .border_b_1()
            .border_color(rgb(0x252526))
            // ── CARD STYLES ────────────────────────────────────────────────
            .child(Self::render_group("CARD STYLES", vec![
                RibbonBtn { id: "fmt-tag",     label: "Tag",      tooltip: "Tag — card label (bold)",     action: FormatAction::Tag,        theme: BtnTheme::Tag      },
                RibbonBtn { id: "fmt-cite",    label: "Cite",     tooltip: "Cite — citation line",        action: FormatAction::Cite,       theme: BtnTheme::Cite     },
                RibbonBtn { id: "fmt-body",    label: "Body",     tooltip: "Body — card body text",      action: FormatAction::Body,       theme: BtnTheme::Default  },
                RibbonBtn { id: "fmt-pkt",     label: "Pkt",      tooltip: "Pocket — condensed format",  action: FormatAction::Pocket,     theme: BtnTheme::Default  },
                RibbonBtn { id: "fmt-pktcite", label: "Pkt Cite", tooltip: "Pocket Cite",                action: FormatAction::PocketCite, theme: BtnTheme::Cite     },
            ], cx))
            .child(Self::separator())
            // ── MARKUP ─────────────────────────────────────────────────────
            .child(Self::render_group("MARKUP", vec![
                RibbonBtn { id: "fmt-und",  label: "Und",  tooltip: "Underline (sub-mark evidence)",  action: FormatAction::Underline,       theme: BtnTheme::Default  },
                RibbonBtn { id: "fmt-hly",  label: "HLt",  tooltip: "Yellow highlight (in-round)",    action: FormatAction::HighlightYellow, theme: BtnTheme::YellowHL },
                RibbonBtn { id: "fmt-hlg",  label: "HLg",  tooltip: "Green highlight (best evid.)",   action: FormatAction::HighlightGreen,  theme: BtnTheme::GreenHL  },
                RibbonBtn { id: "fmt-bold", label: "Bold", tooltip: "Bold",                           action: FormatAction::Bold,            theme: BtnTheme::Default  },
                RibbonBtn { id: "fmt-italic", label: "Ital", tooltip: "Italic",                        action: FormatAction::Italic,          theme: BtnTheme::Default  },
            ], cx))
            .child(Self::separator())
            // ── CLEAN ──────────────────────────────────────────────────────
            .child(Self::render_group("CLEAN", vec![
                RibbonBtn { id: "fmt-rmhl",  label: "Rm HL", tooltip: "Remove highlight from selection", action: FormatAction::RemoveHighlight, theme: BtnTheme::Default },
                RibbonBtn { id: "fmt-clean", label: "Clean", tooltip: "Remove all character formatting", action: FormatAction::Clean,           theme: BtnTheme::Default },
            ], cx))
            .child(Self::separator())
            // ── STRUCTURE ──────────────────────────────────────────────────
            .child(Self::render_group("STRUCTURE", vec![
                RibbonBtn { id: "fmt-openblk",  label: "Open Blk",  tooltip: "Open speech block",  action: FormatAction::OpenBlock,  theme: BtnTheme::Default },
                RibbonBtn { id: "fmt-closeblk", label: "Close Blk", tooltip: "Close speech block", action: FormatAction::CloseBlock, theme: BtnTheme::Default },
            ], cx))
            .child(Self::separator())
            // ── SIZE ───────────────────────────────────────────────────────
            .child(Self::render_group("SIZE", vec![
                RibbonBtn { id: "fmt-shrink", label: "Shrink", tooltip: "Decrease font size by one step", action: FormatAction::Shrink,     theme: BtnTheme::Default },
                RibbonBtn { id: "fmt-norm",   label: "Normal", tooltip: "Reset to standard font size",    action: FormatAction::NormalSize, theme: BtnTheme::Default },
            ], cx))
    }
}

#[cfg(test)]
mod tests {
    // Import only the item under test, not `super::*` — this file has
    // `use gpui::*;` at module scope, and gpui exports its own `test`
    // attribute macro that shadows std's `#[test]` if it's in scope here
    // (same issue text_editor.rs's test module documents).
    use super::{FormatAction, FormatOp};

    #[test]
    fn test_markup_actions_map_to_format_ops() {
        assert_eq!(FormatAction::Bold.to_format_op(), Some(FormatOp::Bold(true)));
        assert_eq!(FormatAction::Italic.to_format_op(), Some(FormatOp::Italic(true)));
        assert_eq!(FormatAction::Underline.to_format_op(), Some(FormatOp::Underline(true)));
        assert_eq!(
            FormatAction::HighlightYellow.to_format_op(),
            Some(FormatOp::Highlight(Some("yellow".to_string())))
        );
        assert_eq!(
            FormatAction::HighlightGreen.to_format_op(),
            Some(FormatOp::Highlight(Some("green".to_string())))
        );
    }

    #[test]
    fn test_clean_actions_map_to_format_ops() {
        assert_eq!(FormatAction::RemoveHighlight.to_format_op(), Some(FormatOp::Highlight(None)));
        assert_eq!(FormatAction::Clean.to_format_op(), Some(FormatOp::ClearAll));
    }

    #[test]
    fn test_size_actions_map_to_format_ops() {
        assert_eq!(FormatAction::Shrink.to_format_op(), Some(FormatOp::FontSize(20)));
        assert_eq!(FormatAction::NormalSize.to_format_op(), Some(FormatOp::FontSize(24)));
    }

    #[test]
    fn test_card_style_and_structure_actions_are_unmapped() {
        assert_eq!(FormatAction::Tag.to_format_op(), None);
        assert_eq!(FormatAction::Cite.to_format_op(), None);
        assert_eq!(FormatAction::Body.to_format_op(), None);
        assert_eq!(FormatAction::Pocket.to_format_op(), None);
        assert_eq!(FormatAction::PocketCite.to_format_op(), None);
        assert_eq!(FormatAction::OpenBlock.to_format_op(), None);
        assert_eq!(FormatAction::CloseBlock.to_format_op(), None);
    }
}
