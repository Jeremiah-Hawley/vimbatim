/// Stable identity for a tab across reordering and asynchronous work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub usize);

/// The editable document state kept together so text, formatting, and history
/// cannot be independently replaced by a future caller.
#[derive(Clone, Debug)]
pub struct DocumentBuffer {
    pub content: String,
    pub paragraphs: Vec<Paragraph>,
    pub content_version: u64,
    pub is_modified: bool,
    pub undo_stack: Vec<(String, Vec<Paragraph>)>,
    pub redo_stack: Vec<(String, Vec<Paragraph>)>,
    pub last_edit_at: Option<std::time::Instant>,
}

impl DocumentBuffer {
    pub fn new(content: String, paragraphs: Vec<Paragraph>) -> Self {
        Self {
            content,
            paragraphs,
            content_version: 0,
            is_modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_at: None,
        }
    }

    #[allow(dead_code)] // Called by the next mutation-boundary extraction.
    pub fn debug_assert_valid(&self, cursor: usize, selection: Option<(usize, usize)>) {
        debug_assert!(!self.paragraphs.is_empty());
        debug_assert!(self
            .paragraphs
            .iter()
            .all(|paragraph| !paragraph.runs.is_empty()));
        debug_assert_eq!(
            self.paragraphs
                .iter()
                .map(|paragraph| paragraph
                    .runs
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
            self.content
        );
        debug_assert!(cursor <= self.content.len() && self.content.is_char_boundary(cursor));
        if let Some((anchor, focus)) = selection {
            debug_assert!(anchor <= self.content.len() && focus <= self.content.len());
            debug_assert!(
                self.content.is_char_boundary(anchor) && self.content.is_char_boundary(focus)
            );
        }
    }
}

impl Default for DocumentBuffer {
    fn default() -> Self {
        Self::new(
            String::new(),
            vec![Paragraph {
                list: None,
                runs: vec![Run::default()],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            }],
        )
    }
}

impl From<usize> for TabId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

/// A named debate style a run carries, independent of the visual formatting
/// that style happens to apply.
///
/// Pocket/Hat/Block/Tag were previously identified only by
/// `Paragraph.heading`, and Cite and Analytic by nothing at all — they were
/// recognised by pattern-matching bold + a configured font size + a color,
/// which mistakes any hand-formatted text that happens to match. A marker
/// makes the intent explicit and survives a round-trip through the .docx as a
/// `<w:rStyle>` reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardStyle {
    Pocket,
    Hat,
    Block,
    Tag,
    Cite,
    Analytic,
}

impl CardStyle {
    /// The `<w:rStyle w:val>` id written into the document.
    ///
    /// Namespaced so it cannot collide with a style a real Word document
    /// defines. Word ignores a reference to a style id it doesn't know, so
    /// these are harmless in any other editor — and this parser reads the id
    /// back directly rather than resolving it through `styles.xml`, so the
    /// marker survives even though nothing defines it there.
    pub fn style_id(&self) -> &'static str {
        match self {
            CardStyle::Pocket => "VimbatimPocket",
            CardStyle::Hat => "VimbatimHat",
            CardStyle::Block => "VimbatimBlock",
            CardStyle::Tag => "VimbatimTag",
            CardStyle::Cite => "VimbatimCite",
            CardStyle::Analytic => "VimbatimAnalytic",
        }
    }

    /// The `<w:rStyle>` id to write into a `.docx` for this style, or `None`
    /// when nothing should be written.
    ///
    /// Pocket/Hat/Block/Tag write nothing: real Verbatim puts no run-level
    /// style on them at all (verified against
    /// `Verbatim_Formatting_To_Compare_To.docx` — their identity is entirely
    /// `<w:pStyle w:val="HeadingN">`), and `from_heading` re-derives the marker
    /// from the heading level at parse. Emitting `VimbatimPocket` and friends
    /// only ever produced four `<w:rStyle>` references to styles that existed
    /// in no stylesheet.
    ///
    /// Cite and Emphasis take Verbatim's own ids, so a card written here is the
    /// same thing to Verbatim that one written there is. Analytic has no
    /// Verbatim equivalent and keeps this app's id — defined in
    /// `build_new_doc_styles_xml` so it resolves.
    pub fn docx_rstyle_id(&self) -> Option<&'static str> {
        match self {
            CardStyle::Pocket | CardStyle::Hat | CardStyle::Block | CardStyle::Tag => None,
            CardStyle::Cite => Some("Style13ptBold"),
            CardStyle::Analytic => Some("VimbatimAnalytic"),
        }
    }

    pub fn from_style_id(id: &str) -> Option<CardStyle> {
        match id {
            // Verbatim's own ids, and this app's for Analytic — what
            // `docx_rstyle_id` writes today.
            "Style13ptBold" => Some(CardStyle::Cite),
            "VimbatimAnalytic" => Some(CardStyle::Analytic),

            // LEGACY (remove once no file in circulation predates the switch
            // to Verbatim's style ids — see `docx_rstyle_id`). Read-only: none
            // of these four are written any more, and `VimbatimCite` is
            // superseded by `Style13ptBold`, so every file converts on its next
            // save. Dropping this arm early would silently strip the Cite
            // marker from every document this app has already written — the
            // direct bold and size would still look right, so nothing would
            // announce it.
            "VimbatimPocket" => Some(CardStyle::Pocket),
            "VimbatimHat" => Some(CardStyle::Hat),
            "VimbatimBlock" => Some(CardStyle::Block),
            "VimbatimTag" => Some(CardStyle::Tag),
            "VimbatimCite" => Some(CardStyle::Cite),
            _ => None,
        }
    }

    /// The card style a Word heading level corresponds to, for documents
    /// written elsewhere that carry `<w:pStyle w:val="Heading N"/>` but none
    /// of this app's own markers.
    pub fn from_heading(level: u8) -> Option<CardStyle> {
        match level {
            1 => Some(CardStyle::Pocket),
            2 => Some(CardStyle::Hat),
            3 => Some(CardStyle::Block),
            4 => Some(CardStyle::Tag),
            _ => None,
        }
    }
}

/// The list style a paragraph carries, if any. `level` is always 0 in
/// Phase 1 (single-level); Phase 2 uses 0-8, matching Word's own `w:ilvl`
/// range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListItem {
    pub kind: ListKind,
    pub level: u8,
}

/// One of the 13 real-Word list styles this app supports, exactly matching
/// the reference file `Lists.docx`'s four examples (a plain bulleted list,
/// a plain numbered list, six distinct bullet options, seven distinct
/// number options). No open-ended "custom" variant — an unrecognized
/// foreign list is classified to the nearest of these via
/// `ListKind::classify`, never dropped or stored raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    BulletSolid,
    BulletHollow,
    BulletSolidBox,
    BulletDiamond,
    BulletArrow,
    BulletCheckmark,
    NumberDecimalDot,
    NumberDecimalParen,
    NumberUpperRoman,
    NumberUpperLetter,
    NumberLowerLetterParen,
    NumberLowerLetterDot,
    NumberLowerRoman,
}

impl ListKind {
    /// `true` for the six bullet variants, `false` for the seven number
    /// variants — used by the gallery UI to pick which button's menu a
    /// style belongs in, and by marker rendering to decide whether to
    /// paint a fixed glyph or a computed ordinal.
    pub fn is_bullet(&self) -> bool {
        matches!(
            self,
            ListKind::BulletSolid
                | ListKind::BulletHollow
                | ListKind::BulletSolidBox
                | ListKind::BulletDiamond
                | ListKind::BulletArrow
                | ListKind::BulletCheckmark
        )
    }

    /// Resolves a parsed `(numFmt, lvlText, font)` triple to the nearest of
    /// the 13 supported styles, per the exact fallback order in
    /// `docs/superpowers/specs/2026-08-11-lists-design.md`'s "Data model"
    /// section — never drops or represents a foreign list as raw data.
    pub fn classify(num_fmt: &str, lvl_text: &str, font: Option<&str>) -> ListKind {
        match (num_fmt, lvl_text, font) {
            ("bullet", "\u{f0b7}", Some("Symbol")) => ListKind::BulletSolid,
            ("bullet", "o", Some("Courier New")) => ListKind::BulletHollow,
            ("bullet", "\u{f0a7}", Some("Wingdings")) => ListKind::BulletSolidBox,
            ("bullet", "\u{f076}", Some("Wingdings")) => ListKind::BulletDiamond,
            ("bullet", "\u{f0d8}", Some("Wingdings")) => ListKind::BulletArrow,
            ("bullet", "\u{f0fc}", Some("Wingdings")) => ListKind::BulletCheckmark,
            ("decimal", "%1.", _) => ListKind::NumberDecimalDot,
            ("decimal", "%1)", _) => ListKind::NumberDecimalParen,
            ("upperRoman", "%1.", _) => ListKind::NumberUpperRoman,
            ("upperLetter", "%1.", _) => ListKind::NumberUpperLetter,
            ("lowerLetter", "%1)", _) => ListKind::NumberLowerLetterParen,
            ("lowerLetter", "%1.", _) => ListKind::NumberLowerLetterDot,
            ("lowerRoman", "%1.", _) => ListKind::NumberLowerRoman,
            ("bullet", _, _) => ListKind::BulletSolid,
            ("decimal", _, _) => ListKind::NumberDecimalDot,
            ("upperRoman", _, _) => ListKind::NumberUpperRoman,
            ("upperLetter", _, _) => ListKind::NumberUpperLetter,
            ("lowerLetter", _, _) => ListKind::NumberLowerLetterDot,
            ("lowerRoman", _, _) => ListKind::NumberLowerRoman,
            _ => ListKind::NumberDecimalDot,
        }
    }
}

/// A single formatting run within a paragraph — the smallest unit of text with
/// consistent styling. Word documents split paragraphs into runs whenever
/// formatting changes (e.g., switching from plain to bold text).
///
/// Derives `Clone` so a tab's live `paragraphs` can be snapshotted into
/// `undo_stack`/`redo_stack` alongside `content` (rich-text formatting plan,
/// Phase 1) — none of these fields are expensive to clone.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    /// `<w:i/>` (rich-text formatting plan, Phase 1).
    pub italic: bool,
    pub underline: bool,
    pub double_underline: bool,
    pub strikethrough: bool,
    pub highlight: bool,
    pub highlight_color: String,
    pub size: u16,
    /// `<w:rFonts w:ascii="...">` — `None` means "inherit the document
    /// default", same convention as `color` below.
    pub font: Option<String>,
    /// `<w:color w:val="RRGGBB">`, Word's own hex format. `None` (or
    /// `w:val="auto"`, parsed the same as absent) means "inherit".
    pub color: Option<String>,
    pub box_format: bool,
    /// True when `xml:space="preserve"` is set on `<w:t>` — required to keep
    /// leading/trailing whitespace that XML parsers would otherwise strip.
    pub whitespace_preserve: bool,
    /// The debate style this run was given, if any. See `CardStyle`.
    pub style: Option<CardStyle>,
    /// Set by `AppState::apply_emphasis_style` — the Emphasis button's own
    /// marker, independent of `bold`/`underline`/`box_format` (which
    /// combination it applied is user-configurable and not itself proof the
    /// text is "emphasized"). `remove_emphasis` reads this directly rather
    /// than guessing from formatting.
    pub emphasis: bool,
    /// The small inline emphasis box (rendered as a border directly on the
    /// run's span — see `text_editor::apply_run_style`), distinct from
    /// `box_format`'s paragraph-wide Pocket box.
    pub emphasis_boxed: bool,
}

/// One paragraph of the document, composed of zero or more runs.
/// `heading` is 0 for body text, or 1–9 mirroring Word's Heading 1–9 styles.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    pub heading: u8,
    pub alignment: Alignment, // left, center, right, justify
    /// The list style/level this paragraph carries, if any. See `ListItem`.
    pub list: Option<ListItem>,
    /// Raw inner XML (everything between `<w:p...>` and `</w:p>`), captured
    /// at parse time only when this paragraph contains one of a narrow,
    /// explicit list of elements the app doesn't model (hyperlinks, inline
    /// drawings, footnote/endnote references, field codes) — see
    /// `parse_document_xml`'s `UNSUPPORTED_INLINE_TAGS`. `Some` means
    /// `rebuild_document_xml` re-emits this verbatim instead of rebuilding
    /// from `runs`/`heading`/`alignment`. Cleared to `None` the instant this
    /// paragraph is actually edited (`document_ops.rs`'s mutation choke
    /// points), at which point whatever exotic content it had is
    /// permanently, deliberately dropped — there's no way to keep e.g. a
    /// hyperlink's target in sync with retyped text.
    pub unsupported_xml: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

/// for everything that doesn't override them.
///
/// Read so a document written in Word or Verbatim renders here with *its*
/// defaults rather than this app's settings. It is deliberately kept as a
/// document-level fallback and never folded into `Run`s: `run.size == 0` and
/// `run.font == None` mean "inherit", and that is what keeps a saved file free
/// of a redundant `<w:sz>` and `<w:rFonts>` on every single run. Baking these
/// in would write them back out and freeze the document's default at whatever
/// it happened to be the first time it was opened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocDefaults {
    /// `<w:rPrDefault>`'s `<w:rFonts w:ascii>`, when the document names one.
    pub font: Option<String>,
    /// `<w:rPrDefault>`'s `<w:sz>`, half-points. `0` means the document
    /// declares no default size.
    pub size: u16,
}

/// The settings-derived numbers a freshly created `.docx` bakes into its
/// `word/styles.xml`: the document-wide defaults, each card style's size, and
/// what Emphasis means here.
///
/// Passed in rather than read from `AppState`, because this module
/// deliberately doesn't depend on `state`. `Default` holds the same constants
/// `CardStyleKind::font_size` falls back to, so a caller with no settings to
/// hand — a test, or a crash snapshot — still writes a conventional document.
///
/// The point of routing these through here at all is that Word had no idea
/// what a Vimbatim document's defaults were: with no `<w:docDefaults>` it
/// applied its own, which differ by Word version, so a blank document created
/// here and one created in Verbatim didn't agree on body size or line spacing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NewDocStyle {
    /// Body text size, half-points (`normal_text_size`).
    pub normal_size: u16,
    /// Line spacing multiplier (`line_spacing`): 1.0 single, 2.0 double.
    pub line_spacing: f32,
    /// Card style sizes, half-points.
    pub pocket_size: u16,
    pub hat_size: u16,
    pub block_size: u16,
    pub tag_size: u16,
    pub cite_size: u16,
    /// What Emphasis applies, so the `Emphasis` character style written into
    /// the document matches what this app actually does rather than copying
    /// Verbatim's fixed definition.
    pub emphasis_bold: bool,
    pub emphasis_underline: bool,
    pub emphasis_box: bool,
    /// `Some(half_points)` when Emphasis also resizes, `None` when it leaves
    /// size alone (`emphasis_change_size`).
    pub emphasis_size: Option<u16>,
}

impl Default for NewDocStyle {
    fn default() -> Self {
        Self {
            normal_size: 22,
            line_spacing: 1.0,
            pocket_size: 52,
            hat_size: 44,
            block_size: 32,
            tag_size: 26,
            cite_size: 26,
            emphasis_bold: true,
            emphasis_underline: false,
            emphasis_box: false,
            emphasis_size: None,
        }
    }
}

/// Document-model normalization shared by editing and OOXML parsing.
pub mod normalize {
    use super::Run;

    /// Merges adjacent runs that share identical formatting into one, comparing
    /// every `Run` field except `text`. Deletion can make two runs that
    /// previously had unrelated text become textually adjacent — without this,
    /// repeated edits would let paragraphs accumulate more and more same-format
    /// runs indefinitely.
    pub fn merge_adjacent_same_format_runs(runs: &mut Vec<Run>) {
        /*
         * Empty runs go first, before the merge — they render nothing but are not
         * harmless. `sync_insert_str_with_runs` inserts one character at a time
         * and splits paragraphs as it goes, which strands an empty remnant of each
         * source run in the paragraph the split left behind. A multi-line paste of
         * Pocket/Hat/Block/Tag lines therefore ended with a paragraph holding
         * `[("test", 0), ("", 26), ("", 32), ("", 44), ("", 52)]` — four invisible
         * runs whose sizes and `box_format` still fed paragraph-level rendering
         * (a spurious border) and got written straight back out to the .docx.
         *
         * The all-empty case is a genuinely blank paragraph: keep exactly one run,
         * both to hold the invariant that a paragraph always has at least one and
         * to preserve the formatting typing there should inherit.
         */
        if runs.iter().all(|r| r.text.is_empty()) {
            runs.truncate(1);
        } else {
            runs.retain(|r| !r.text.is_empty());
        }

        let mut i = 0;
        while i + 1 < runs.len() {
            let same_format = runs[i].bold == runs[i + 1].bold
            && runs[i].italic == runs[i + 1].italic
            && runs[i].underline == runs[i + 1].underline
            && runs[i].double_underline == runs[i + 1].double_underline
            && runs[i].strikethrough == runs[i + 1].strikethrough
            && runs[i].highlight == runs[i + 1].highlight
            && runs[i].highlight_color == runs[i + 1].highlight_color
            && runs[i].size == runs[i + 1].size
            && runs[i].font == runs[i + 1].font
            && runs[i].color == runs[i + 1].color
            && runs[i].box_format == runs[i + 1].box_format
            && runs[i].whitespace_preserve == runs[i + 1].whitespace_preserve
            && runs[i].emphasis == runs[i + 1].emphasis
            && runs[i].emphasis_boxed == runs[i + 1].emphasis_boxed
            // Two runs that look identical but carry different markers are
            // different things — fusing them would erase one.
            && runs[i].style == runs[i + 1].style;
            if same_format {
                let next_text = runs[i + 1].text.clone();
                runs[i].text.push_str(&next_text);
                runs.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_validation_accepts_synced_text_and_runs() {
        let buffer = DocumentBuffer::new(
            "hello".into(),
            vec![Paragraph {
                list: None,
                runs: vec![Run {
                    text: "hello".into(),
                    ..Run::default()
                }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            }],
        );
        buffer.debug_assert_valid(5, Some((0, 5)));
    }
}
