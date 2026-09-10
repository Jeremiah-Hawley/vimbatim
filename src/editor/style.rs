//! Pure presentation style calculations.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use crate::docx_parser::Paragraph;

/// Monospace glyph advance as a fraction of its font size.
pub(crate) const CHAR_ADVANCE_RATIO: f32 = 8.4 / 14.0;

/// Returns the zoomed font size for a heading; body text has no override.
pub(crate) fn heading_font_size_px(heading: u8, zoom: f32) -> Option<f32> {
    match heading {
        0 => None,
        1 => Some(24.0 * zoom),
        2 => Some(20.0 * zoom),
        3 => Some(18.0 * zoom),
        4..=6 => Some(16.0 * zoom),
        _ => Some(14.0 * zoom),
    }
}

/// Font size used to reserve a paragraph's visual row.
pub(crate) fn line_font_px(
    para: Option<&crate::docx_parser::Paragraph>,
    zoom: f32,
    normal_size_px: f32,
) -> f32 {
    let Some(para) = para else {
        return normal_size_px * zoom;
    };
    if para.heading == 4 {
        return normal_size_px * zoom;
    }
    // Emphasis runs are no longer excluded. They were, because the quantum
    // used to be a whole line: one emphasized word in a plain paragraph cost
    // the entire paragraph a full extra line of spacing (that bug report is
    // what added the filter). With `ROW_SUBDIVISIONS` the same run now costs
    // a fraction of a line, which is small enough to be the honest answer —
    // and excluding it is what left an emphasis box painting outside the row
    // reserved for it, overlapping the box on the line above.
    let run_max_px = para
        .runs
        .iter()
        .filter(|r| r.size > 0 && r.style != Some(crate::docx_parser::CardStyle::Cite))
        .map(|r| r.size as f32 / 2.0 * zoom)
        .fold(0.0_f32, f32::max);
    // An explicit run-level size (card styles always set one, covering the
    // whole line) already reflects what's actually drawn and wins over the
    // heading fallback below it — using both would over-reserve whenever the
    // card style's real size is smaller than its heading level's generic
    // default (e.g. Tag: 13px run size vs. heading level 4's 16px fallback),
    // padding in a spurious blank row under every Tag line. `heading_px` is
    // only relevant when no run carries an explicit size — plain document
    // headings with no card-style override.
    let heading_px = heading_font_size_px(para.heading, zoom).unwrap_or(0.0);
    if run_max_px > 0.0 {
        run_max_px
    } else {
        heading_px
    }
    .max(normal_size_px * zoom)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_sizes_are_zoomed() {
        assert_eq!(heading_font_size_px(0, 2.0), None);
        assert_eq!(heading_font_size_px(2, 1.5), Some(30.0));
    }
}

// Font selection and per-character metrics are presentation policy, not GPUI view code.
/// A literal, well-known monospace family name rather than the generic
/// CSS-style alias `"monospace"`. GPUI's font matching (`cosmic_text`'s
/// `load_family`) filters real system fonts by an *exact string* match
/// against each font file's own embedded family name — no font ever
/// declares its family as literally "monospace", so that name always
/// missed and fell through to GPUI's hardcoded fallback stack, which
/// resolves each candidate at default weight/style, discarding any
/// requested bold/italic before it ever reaches font matching. Separately,
/// `find_best_match` short-circuits (`candidates.len() == 1 => Ok(0)`)
/// without checking weight/style whenever the resolved family has only one
/// loaded face — together these silently dropped every bold/italic
/// request. "DejaVu Sans Mono" ships with separate Book/Bold/Oblique/Bold
/// Oblique faces under one family name on essentially all Linux/WSL
/// systems, giving `find_best_match` real candidates to choose between.
pub(crate) const FONT_FAMILY: &str = "DejaVu Sans Mono";

/// The one other font this app can vouch for the same way it vouches for
/// `FONT_FAMILY`: bundled (`main.rs`'s `load_bundled_fonts`) with all 4
/// weight/style faces, so GPUI's `find_best_match` short-circuit (see
/// `FONT_FAMILY`'s own doc comment) can't silently drop bold/italic for it.
/// Not literally "Georgia" (the font a tester actually asked for) — Georgia
/// is a proprietary Microsoft font with no redistribution rights, so it
/// can't be bundled the same way. `formatting_ribbon.rs`'s Font Family
/// picker only offers `CURATED_FONTS`, not every font installed on the
/// host, specifically so a selection always renders correctly; a `run.font`
/// read from a real imported `.docx` (naming e.g. actual Georgia, Calibri,
/// Times New Roman) still round-trips on save, it just isn't rendered as
/// that font on screen — see `apply_run_style`.
pub(crate) const CURATED_SERIF_FONT: &str = "DejaVu Serif";

pub(crate) const CURATED_FONTS: &[&str] = &[FONT_FAMILY, CURATED_SERIF_FONT];

/// A user-imported font family (`font_import.rs`), registered at runtime
/// rather than baked into `CURATED_FONTS`. `ratio` is `char_advance_ratio`'s
/// equivalent for this family — same "advance as a fraction of font size"
/// idea as `SERIF_CHAR_ADVANCE_RATIO`, but measured automatically (via
/// `measure_advance_ratio`, real glyph shaping) instead of hand-derived,
/// since there's no fixed set of imported fonts to hand-tune for. `dir` is
/// where its face files live under `recovery::app_data_dir()`, kept so
/// removing the font can delete them.
struct ImportedFontMeta {
    ratio: f32,
    dir: PathBuf,
}

/// Fonts the user has imported this run (or reloaded from disk at startup —
/// see `font_import::load_persisted`), keyed by family name. Lives here
/// rather than on `AppState`: every consumer (`is_curated_font`,
/// `effective_char_font`, `effective_char_advance_ratio`,
/// `visual_rows_for_viewport`) is a free function taking `Option<&Paragraph>`
/// data, not an `AppState` handle, so threading it through as a parameter
/// would touch every call site for no benefit over one process-wide table.
static IMPORTED_FONTS: OnceLock<RwLock<HashMap<String, ImportedFontMeta>>> = OnceLock::new();

fn imported_fonts() -> &'static RwLock<HashMap<String, ImportedFontMeta>> {
    IMPORTED_FONTS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn is_imported_font(name: &str) -> bool {
    imported_fonts().read().unwrap().contains_key(name)
}

fn imported_font_ratio(name: &str) -> Option<f32> {
    imported_fonts().read().unwrap().get(name).map(|m| m.ratio)
}

/// Names of every imported font, sorted for a stable picker order.
pub(crate) fn imported_font_names() -> Vec<String> {
    let mut names: Vec<String> = imported_fonts().read().unwrap().keys().cloned().collect();
    names.sort();
    names
}

/// `CURATED_FONTS` plus every imported font — what the Font Family picker
/// (`formatting_ribbon.rs`) actually offers.
pub(crate) fn all_curated_font_names() -> Vec<String> {
    let mut names: Vec<String> = CURATED_FONTS.iter().map(|s| s.to_string()).collect();
    names.extend(imported_font_names());
    names
}

/// Registers a newly-imported (or freshly-reloaded) font family so it starts
/// rendering and appears in the picker. Called by `font_import.rs` after the
/// faces are already handed to `cx.text_system().add_fonts`.
pub(crate) fn register_imported_font(name: String, ratio: f32, dir: PathBuf) {
    imported_fonts()
        .write()
        .unwrap()
        .insert(name, ImportedFontMeta { ratio, dir });
}

/// Drops a font from the picker/render path and returns where its face
/// files live on disk, for the caller to delete. The face bytes already
/// handed to `cx.text_system().add_fonts` have no unload API and stay
/// resident until restart — harmless, since nothing can reach them by name
/// once this returns, but a run still explicitly naming this font (e.g. a
/// freshly-reopened `.docx`) falls back to `FONT_FAMILY` immediately, not
/// just after restart.
pub(crate) fn unregister_imported_font(name: &str) -> Option<PathBuf> {
    imported_fonts()
        .write()
        .unwrap()
        .remove(name)
        .map(|m| m.dir)
}

pub(crate) fn is_curated_font(name: &str) -> bool {
    CURATED_FONTS.contains(&name) || is_imported_font(name)
}

/// `CURATED_SERIF_FONT`'s counterpart to `CHAR_ADVANCE_RATIO` below — same
/// "advance as a fraction of font size" idea, but DejaVu Serif is
/// proportional (unlike the monospace primary font), so no single ratio is
/// exact per-character the way `CHAR_ADVANCE_RATIO` is for a font where
/// every glyph is identically wide. Measured as the mean advance across
/// a-z/0-9 at the font's own units-per-em (via `ttf_parser`, same
/// empirical-constant approach `CHAR_ADVANCE_RATIO` itself already uses —
/// see its doc comment), which is close enough to `CHAR_ADVANCE_RATIO`
/// (0.6) that this remains a reasonable approximation for click-to-position
/// and Up/Down column math, the same "not real glyph shaping" tradeoff
/// `CHAR_ADVANCE_RATIO`'s own doc comment already accepts. Word-wrap
/// decisions do *not* use this — `visual_rows_for_viewport` measures each
/// character's real rendered width per font instead.
pub(crate) const SERIF_CHAR_ADVANCE_RATIO: f32 = 0.589;

/// Whether a run should be left unpainted in invisibility mode.
///
/// What survives is highlighted text plus every card style:
/// * `heading != 0` covers Pocket/Hat/Block/Tag, which are paragraph-level
///   (`CardStyleKind::heading_level`), so the whole line stays.
/// * Cite is run-level and has no marker of its own — `apply_cite_style` only
///   sets bold plus `cite_size_half_points` — so it is identified by exactly
///   that pair. Any other bold run at the configured cite size is
///   indistinguishable from a real cite and will also stay visible; that is a
///   limit of how cites are stored, not a choice made here.
///
/// Hiding is purely visual — the run keeps its space, so wrap points, click
/// mapping and cursor math are untouched, and the document itself is never
/// modified.
pub(crate) fn run_is_hidden(
    invisibility: bool,
    heading: u8,
    highlighted: bool,
    bold: bool,
    size_half_points: u16,
    cite_size_half_points: u16,
) -> bool {
    if !invisibility {
        return false;
    }
    let card_style_line = heading != 0;
    // `size == 0` means "inherit the body size" in this codebase, so it can
    // never be a cite even if the setting were somehow zero.
    let is_cite = bold && size_half_points != 0 && size_half_points == cite_size_half_points;
    !(card_style_line || highlighted || is_cite)
}

/// The font size one character actually paints at, mirroring exactly how
/// `render()` layers its three sources: a run-level `FontSize` wins (card
/// styles Pocket/Hat/Block/Tag/Cite and Shrink all set one), otherwise the
/// paragraph's heading level, otherwise the configured body size. Already
/// multiplied by `zoom`.
pub(crate) fn effective_char_size_px(
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    char_idx: usize,
    normal_size_px: f32,
    zoom: f32,
) -> f32 {
    let run = spans
        .iter()
        .find(|(start, end, _)| char_idx >= *start && char_idx < *end)
        .and_then(|(_, _, run_idx)| para.and_then(|p| p.runs.get(*run_idx)));
    if let Some(run) = run {
        if run.size > 0 {
            return run.size as f32 / 2.0 * zoom;
        }
    }
    if let Some(size) = para.and_then(|p| heading_font_size_px(p.heading, zoom)) {
        return size;
    }
    normal_size_px * zoom
}

/// The font family one character actually paints at — mirrors
/// `apply_run_style`'s own rule exactly (a curated `run.font` wins,
/// anything else falls back to `FONT_FAMILY`), so wrap/click/scroll math
/// always measures against the same font that's actually painted.
/// `Cow::Borrowed` for the two compile-time-known outcomes (no allocation on
/// the hot path); `Cow::Owned` only when the run names an imported font,
/// whose name isn't `'static`.
pub(crate) fn effective_char_font(
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    char_idx: usize,
) -> Cow<'static, str> {
    let run = spans
        .iter()
        .find(|(start, end, _)| char_idx >= *start && char_idx < *end)
        .and_then(|(_, _, run_idx)| para.and_then(|p| p.runs.get(*run_idx)));
    match run.and_then(|r| r.font.as_deref()) {
        Some(CURATED_SERIF_FONT) => Cow::Borrowed(CURATED_SERIF_FONT),
        Some(name) if is_imported_font(name) => Cow::Owned(name.to_string()),
        _ => Cow::Borrowed(FONT_FAMILY),
    }
}

/// `column_for_x_in_row`/`x_for_col_in_row`'s per-character advance-ratio
/// lookup: `CHAR_ADVANCE_RATIO` for the monospace primary font,
/// `SERIF_CHAR_ADVANCE_RATIO` for the curated serif font, an imported font's
/// own `measure_advance_ratio` result for anything registered via
/// `font_import.rs`, mirroring `effective_char_font`'s own font selection
/// exactly (so click-mapping and wrap agree on which font a character
/// belongs to, even though wrap uses real glyph measurement and this stays
/// the cheaper ratio approximation — see `SERIF_CHAR_ADVANCE_RATIO`'s doc
/// comment for why that's an acceptable tradeoff here).
pub(crate) fn effective_char_advance_ratio(
    para: Option<&Paragraph>,
    spans: &[(usize, usize, usize)],
    char_idx: usize,
) -> f32 {
    match effective_char_font(para, spans, char_idx).as_ref() {
        CURATED_SERIF_FONT => SERIF_CHAR_ADVANCE_RATIO,
        FONT_FAMILY => CHAR_ADVANCE_RATIO,
        name => imported_font_ratio(name).unwrap_or(CHAR_ADVANCE_RATIO),
    }
}
