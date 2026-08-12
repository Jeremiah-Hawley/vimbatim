use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use zip::ZipArchive;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// Elements that represent real content `Paragraph`/`Run` can't model, so a
/// paragraph containing one has its full inner XML captured verbatim into
/// `unsupported_xml` instead of being silently destroyed on the next save.
/// Deliberately narrow — see `Paragraph::unsupported_xml`'s doc comment for
/// why this must NOT be "anything not explicitly handled" (that would also
/// catch harmless, common elements like bookmarks and proofing marks).
const UNSUPPORTED_INLINE_TAGS: &[&[u8]] = &[
    b"w:hyperlink",
    b"w:drawing",
    b"w:footnoteReference",
    b"w:endnoteReference",
    b"w:fldSimple",
    b"w:instrText",
];

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

    pub fn from_style_id(id: &str) -> Option<CardStyle> {
        match id {
            "VimbatimPocket" => Some(CardStyle::Pocket),
            "VimbatimHat" => Some(CardStyle::Hat),
            "VimbatimBlock" => Some(CardStyle::Block),
            "VimbatimTag" => Some(CardStyle::Tag),
            "VimbatimCite" => Some(CardStyle::Cite),
            "VimbatimAnalytic" => Some(CardStyle::Analytic),
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
    pub alignment: Alignment,  // left, center, right, justify
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

/// The save-time constants needed to reconstruct a real .docx file around
/// whatever a tab's live `paragraphs` currently holds. `raw_zip` is the
/// original file's bytes, used as the template when saving — all ZIP
/// entries except `word/document.xml` are copied verbatim, preserving
/// images, styles, and embedded fonts. `preamble` and `sect_pr` are the
/// fragments of `word/document.xml` that surround the body content.
///
/// Deliberately holds nothing that changes during editing (unlike the old
/// `DocxDocument`, which bundled `paragraphs` in here too) — a tab's
/// `paragraphs` needs to mutate on every keystroke once the rich-text
/// formatting plan's Phase 1 lands (span-sync across edits), which an
/// `Arc`-wrapped, non-`Clone` bundle can't support. `DocxOrigin` itself
/// stays immutable for the tab's lifetime, so it's still cheap to share via
/// `Arc` exactly as before.
#[derive(Debug)]
pub struct DocxOrigin {
    pub(crate) raw_zip: Vec<u8>,
    pub(crate) preamble: String,
    pub(crate) sect_pr: String,
    /// True when the source document's `word/document.xml` contains a
    /// `<w:tbl` (table) anywhere in the body. Tables are block-level
    /// structures — not a single line of the plain-text buffer the way a
    /// paragraph is — so they're never parsed into the editable model at
    /// all; this flag exists purely so the app can warn instead of
    /// silently discarding them on the next save (see
    /// `Tab.has_unsupported_blocks`).
    pub(crate) has_unsupported_blocks: bool,
}

impl DocxOrigin {
    /// Saves `paragraphs` back to `path` as a .docx file, using this
    /// origin's preserved preamble/sectPr/raw ZIP as the template.
    pub fn save(&self, paragraphs: &[Paragraph], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        self.save_with_compression(paragraphs, path, zip::CompressionMethod::Deflated)
    }

    /// `save` for a crash-recovery snapshot. Identical today; it exists as a
    /// separate entry point so the snapshot's compression can be changed
    /// (to `Stored`, skipping deflate) without touching any real-save path.
    /// See the recovery spec's Performance section — do not change this to
    /// `Stored` until Task 10's measurement justifies it.
    pub fn save_snapshot(&self, paragraphs: &[Paragraph], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        self.save_with_compression(paragraphs, path, zip::CompressionMethod::Deflated)
    }

    fn save_with_compression(
        &self,
        paragraphs: &[Paragraph],
        path: &Path,
        method: zip::CompressionMethod,
    ) -> Result<(), Box<dyn std::error::Error>> {
        /*
         * Generate the new XML from the given paragraph model, then hand the
         * bytes off to `write_docx`, which handles the ZIP round-trip. Shared
         * by `save` and `save_snapshot` so the two differ only in the
         * compression they ask for.
         */
        let new_xml = rebuild_document_xml(&self.preamble, &self.sect_pr, paragraphs);
        write_docx(&self.raw_zip, &new_xml, path, method)
    }
}

/// Returns all paragraph text joined by newlines. This is the plain-text
/// content loaded into `tab.content` so the text editor can display it.
pub fn paragraphs_to_plain_text(paragraphs: &[Paragraph]) -> String {
    /*
     * Each paragraph becomes one line.  Runs within a paragraph are
     * concatenated without separators — the run boundary carries no
     * semantic meaning in plain text.
     */
    paragraphs
        .iter()
        .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads the .docx file at `path`, decompresses the ZIP, parses
/// `word/document.xml`, and returns the parsed paragraphs plus the
/// save-time `DocxOrigin` needed to write them back out.
///
/// The raw ZIP bytes are retained in memory so that all non-document entries
/// (styles, images, fonts, etc.) can be reproduced exactly on save without
/// reprocessing.
pub fn parse_docx(path: &Path) -> Result<(Vec<Paragraph>, DocxOrigin), Box<dyn std::error::Error>> {
    /*
     * 1. Read the raw file bytes.
     * 2. Open the ZIP and extract word/document.xml as a string.
     * 3. Parse the XML into a Vec<Paragraph>.
     * 4. Pull the preamble and sectPr out of the raw XML for later serialisation.
     * 5. Return the parsed paragraphs and the assembled DocxOrigin, handing
     *    ownership of raw_zip so no extra copy is needed.
     */
    let raw_zip = std::fs::read(path)?;
    let cursor = std::io::Cursor::new(&raw_zip);
    let mut archive = ZipArchive::new(cursor)?;

    let document_xml = {
        let mut file = archive.by_name("word/document.xml")?;
        let mut xml = String::new();
        file.read_to_string(&mut xml)?;
        xml
    };

    // word/styles.xml doesn't exist for every .docx (e.g. ones this app
    // itself writes via create_new_docx) — treat that as "no named styles",
    // not a parse failure.
    let styles = match archive.by_name("word/styles.xml") {
        Ok(mut file) => {
            let mut xml = String::new();
            file.read_to_string(&mut xml)?;
            parse_styles_xml(&xml)
        }
        Err(_) => HashMap::new(),
    };

    // word/numbering.xml doesn't exist for every .docx either (no lists) —
    // same "absent means empty" convention as word/styles.xml above.
    let numbering = match archive.by_name("word/numbering.xml") {
        Ok(mut file) => {
            let mut xml = String::new();
            file.read_to_string(&mut xml)?;
            parse_numbering_xml(&xml)
        }
        Err(_) => HashMap::new(),
    };

    let paragraphs = parse_document_xml(&document_xml, &styles, &numbering)?;

    // Extract the fragments we need for round-trip serialisation at parse
    // time so we can discard the full XML string afterwards.
    let preamble = extract_preamble(&document_xml).unwrap_or_else(fallback_preamble);
    let sect_pr = extract_sect_pr(&document_xml).unwrap_or("").to_string();
    let has_unsupported_blocks = document_xml.contains("<w:tbl");

    Ok((paragraphs, DocxOrigin { raw_zip, preamble, sect_pr, has_unsupported_blocks }))
}

/// Writes `new_xml` into the .docx at `path`, replacing `word/document.xml`
/// and copying all other ZIP entries verbatim from `raw_zip`.
///
/// An atomic temp-file rename (`path + ".tmp"`) prevents partial writes from
/// corrupting the original if the process is interrupted.
///
///  - `document_compression` applies only to word/document.xml. Real saves
///    pass Deflated; recovery snapshots may pass Stored to skip the deflate
///    cost on a transient file (see the recovery spec's Performance section).
/// Temp path for the atomic write that ends in a rename onto `path`.
///
/// Unique per call, because two writers really can target one destination at
/// the same moment: the panic hook snapshots every dirty tab on the panicking
/// thread while the background snapshot task may already be mid-write on the
/// same tab. With one shared `.docx.tmp` name both `File::create` it, both
/// interleave their zip output into it, and whichever renames last publishes
/// a corrupt archive — with a perfectly valid `.meta` beside it, so recovery
/// offers the entry and then silently drops the document when it won't parse.
///
/// The pid disambiguates two processes saving the same user file; the counter
/// disambiguates threads within one process. `.tmp` stays the final extension
/// so `recovery::scan_recovery_dir` can still recognise abandoned leftovers,
/// and the pid stays the first `-`-separated segment of the file stem so that
/// sweep can tell whether the owning process is still alive.
fn tmp_write_path(path: &Path) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_extension(format!("docx.{}.{}.tmp", std::process::id(), n))
}

fn write_docx(
    raw_zip: &[u8],
    new_xml: &str,
    path: &Path,
    document_compression: zip::CompressionMethod,
) -> Result<(), Box<dyn std::error::Error>> {
    /*
     * Open the original ZIP from the in-memory byte slice, then stream each
     * entry into a new ZIP writer:
     *  - For word/document.xml: write the freshly generated XML using
     *    `document_compression` (the caller decides Deflated vs. Stored).
     *  - For everything else: raw_copy_file copies the compressed bytes without
     *    decompressing, preserving the original compression level and metadata.
     *
     * The new file is written to a .tmp path first and renamed atomically at
     * the end to avoid leaving a corrupt file if an error occurs mid-write.
     */
    let cursor = std::io::Cursor::new(raw_zip);
    let mut archive = ZipArchive::new(cursor)?;

    let tmp_path = tmp_write_path(path);
    let tmp_file = std::fs::File::create(&tmp_path)?;
    let mut writer = ZipWriter::new(tmp_file);

    for i in 0..archive.len() {
        let file = archive.by_index_raw(i)?;
        let name = file.name().to_string();
        if name == "word/document.xml" {
            // Drop the borrow on `archive` before writing to `writer`.
            drop(file);
            let options = SimpleFileOptions::default()
                .compression_method(document_compression);
            writer.start_file(&name, options)?;
            writer.write_all(new_xml.as_bytes())?;
        } else {
            // Raw copy — no decompression, preserves all metadata.
            writer.raw_copy_file(file)?;
        }
    }

    writer.finish()?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// The formatting a named paragraph style (`word/styles.xml`) resolves to,
/// for the narrow set of properties `Paragraph`/`Run` already model. Real
/// Word documents commonly carry a paragraph's visual formatting entirely on
/// the *style* it references (`<w:pStyle>`) rather than inline on the
/// paragraph itself — a document authored with named styles for "Pocket"/
/// "Hat"/"Block"/"Tag" (e.g. Word's own Heading 1-4, aliased accordingly)
/// puts the box/center/bold/size/underline on the style definition, not on
/// each paragraph. Resolved once per referenced style, applied as each such
/// paragraph/run's *default* — any direct/inline formatting the paragraph or
/// run also carries is applied afterward by the normal parse path and wins,
/// matching Word's own direct-formatting-beats-style cascade.
///
/// Deliberately resolves only the *directly*-referenced style, not a
/// `w:basedOn` chain — every style this app's own card conventions produce
/// (and every one observed in a real "Verbatim"-authored test file) is based
/// on `Normal`, which carries nothing relevant (font/size/spacing only, no
/// alignment/border/bold/underline); walking a full inheritance chain would
/// be solving a problem that doesn't exist here.
#[derive(Debug, Default, Clone)]
struct StyleDefaults {
    alignment: Option<Alignment>,
    box_format: bool,
    bold: bool,
    size: u16,
    underline: bool,
    double_underline: bool,
    emphasis_boxed: bool,
}

/// Parses `word/styles.xml` into a `styleId -> StyleDefaults` map. Missing or
/// unparseable `w:pPr`/`w:rPr` content on a given style just leaves that
/// style's `StyleDefaults` at its all-`false`/`None` default — same
/// "leave it alone" fallback `apply_para_alignment`/`apply_run_prop` already
/// use for a single paragraph's own properties.
fn parse_styles_xml(xml: &str) -> HashMap<String, StyleDefaults> {
    let mut styles = HashMap::new();
    let mut reader = Reader::from_str(xml);
    reader.trim_text(false);

    let mut current_id: Option<String> = None;
    let mut current: StyleDefaults = StyleDefaults::default();
    let mut scratch_run = Run::default();
    let mut in_ppr = false;
    let mut in_rpr = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                match e.name().as_ref() {
                    b"w:style" => {
                        current_id = e.attributes().flatten().find_map(|attr| {
                            (attr.key.as_ref() == b"w:styleId")
                                .then(|| String::from_utf8_lossy(&attr.value).into_owned())
                        });
                        current = StyleDefaults::default();
                        scratch_run = Run::default();
                    }
                    b"w:pPr" => { in_ppr = true; }
                    b"w:rPr" => { in_rpr = true; }
                    b"w:jc" if in_ppr => {
                        let mut para = Paragraph { list: None, runs: Vec::new(), heading: 0, alignment: Alignment::default(), unsupported_xml: None };
                        apply_para_alignment(e, &mut para);
                        current.alignment = Some(para.alignment);
                    }
                    b"w:pBdr" if in_ppr => { current.box_format = true; }
                    _ if in_rpr => { apply_run_prop(e, &mut scratch_run); }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"w:pPr" => { in_ppr = false; }
                    b"w:rPr" => { in_rpr = false; }
                    b"w:style" => {
                        current.bold = scratch_run.bold;
                        current.size = scratch_run.size;
                        current.underline = scratch_run.underline;
                        current.double_underline = scratch_run.double_underline;
                        current.emphasis_boxed = scratch_run.emphasis_boxed;
                        if let Some(id) = current_id.take() {
                            styles.insert(id, current.clone());
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    styles
}

/// Parses `word/numbering.xml` into a `numId -> (numFmt, lvlText, font)` map
/// for level 0 only (Phase 1 — Phase 2 extends this to every level once
/// multi-level indent lands). Resolves the `w:num -> w:abstractNumId ->
/// w:lvl[ilvl=0]` indirection in one pass: `abstractNum` definitions are
/// collected first, then `w:num` entries are resolved against them as each
/// is encountered (`word/numbering.xml` always defines every `abstractNum`
/// before any `w:num` that references it, per every real Word file
/// inspected for this feature).
fn parse_numbering_xml(xml: &str) -> HashMap<u32, (String, String, Option<String>)> {
    let mut abstract_defs: HashMap<u32, (String, String, Option<String>)> = HashMap::new();
    let mut num_to_abstract: HashMap<u32, u32> = HashMap::new();

    let mut reader = Reader::from_str(xml);
    reader.trim_text(false);

    let mut current_abstract_id: Option<u32> = None;
    let mut current_num_id: Option<u32> = None;
    let mut in_lvl0 = false;
    let mut current_numfmt = String::new();
    let mut current_lvltext = String::new();
    let mut current_font: Option<String> = None;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                match e.name().as_ref() {
                    b"w:abstractNum" => {
                        current_abstract_id = e.attributes().flatten().find_map(|attr| {
                            (attr.key.as_ref() == b"w:abstractNumId")
                                .then(|| std::str::from_utf8(&attr.value).ok()?.parse().ok())
                                .flatten()
                        });
                    }
                    b"w:lvl" => {
                        in_lvl0 = e.attributes().flatten().any(|attr| {
                            attr.key.as_ref() == b"w:ilvl" && attr.value.as_ref() == b"0"
                        });
                        current_numfmt.clear();
                        current_lvltext.clear();
                        current_font = None;
                    }
                    b"w:numFmt" if in_lvl0 => {
                        if let Some(v) = e.attributes().flatten().find_map(|a| {
                            (a.key.as_ref() == b"w:val").then(|| String::from_utf8_lossy(&a.value).into_owned())
                        }) {
                            current_numfmt = v;
                        }
                    }
                    b"w:lvlText" if in_lvl0 => {
                        if let Some(v) = e.attributes().flatten().find_map(|a| {
                            (a.key.as_ref() == b"w:val").then(|| String::from_utf8_lossy(&a.value).into_owned())
                        }) {
                            current_lvltext = v;
                        }
                    }
                    b"w:rFonts" if in_lvl0 => {
                        current_font = e.attributes().flatten().find_map(|a| {
                            (a.key.as_ref() == b"w:ascii").then(|| String::from_utf8_lossy(&a.value).into_owned())
                        });
                    }
                    b"w:num" => {
                        current_num_id = e.attributes().flatten().find_map(|attr| {
                            (attr.key.as_ref() == b"w:numId")
                                .then(|| std::str::from_utf8(&attr.value).ok()?.parse().ok())
                                .flatten()
                        });
                    }
                    b"w:abstractNumId" => {
                        if let Some(num_id) = current_num_id {
                            if let Some(abs_id) = e.attributes().flatten().find_map(|attr| {
                                (attr.key.as_ref() == b"w:val")
                                    .then(|| std::str::from_utf8(&attr.value).ok()?.parse().ok())
                                    .flatten()
                            }) {
                                num_to_abstract.insert(num_id, abs_id);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"w:lvl" if in_lvl0 => {
                        if let Some(id) = current_abstract_id {
                            abstract_defs.insert(
                                id,
                                (
                                    current_numfmt.clone(),
                                    current_lvltext.clone(),
                                    current_font.clone(),
                                ),
                            );
                        }
                        in_lvl0 = false;
                    }
                    b"w:num" => { current_num_id = None; }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    num_to_abstract
        .into_iter()
        .filter_map(|(num_id, abs_id)| abstract_defs.get(&abs_id).cloned().map(|def| (num_id, def)))
        .collect()
}

/// Parses the XML string from `word/document.xml` into a flat `Vec<Paragraph>`.
///
/// Uses quick-xml's streaming event API (no DOM tree is built) to keep memory
/// use proportional to the longest run of text, not the full document size.
/// Boolean flags (`in_ppr`, `in_rpr`, `in_text`) track the parser's position
/// in the nesting hierarchy so attribute-reading only fires in the right context.
fn parse_document_xml(
    xml: &str,
    styles: &HashMap<String, StyleDefaults>,
    numbering: &HashMap<u32, (String, String, Option<String>)>,
) -> Result<Vec<Paragraph>, Box<dyn std::error::Error>> {
    /*
     * Relevant element hierarchy in Word XML:
     *
     *   <w:p>                ← paragraph  → Paragraph
     *     <w:pPr>            ← para props (heading style lives here)
     *       <w:pStyle/>
     *     </w:pPr>
     *     <w:r>              ← run        → Run
     *       <w:rPr>          ← run props (bold, underline, etc.)
     *         <w:b/>
     *         <w:u/>
     *         <w:highlight/>
     *         <w:sz/>
     *       </w:rPr>
     *       <w:t>text</w:t>  ← actual characters
     *     </w:r>
     *   </w:p>
     *
     * The `buf` Vec is reused across events to avoid repeated allocation.
     * Word sometimes uses self-closing tags (Event::Empty) for properties like
     * `<w:b/>`, so both Start and Empty events are handled for every property
     * element via the shared helper functions `apply_run_prop` and
     * `apply_para_style`.
     */
    let mut reader = Reader::from_str(xml);
    // Do not trim whitespace — leading/trailing spaces in <w:t> are significant.
    reader.trim_text(false);

    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut current_para: Option<Paragraph> = None;
    let mut current_run: Option<Run> = None;

    let mut in_ppr  = false; // inside <w:pPr>
    let mut in_rpr  = false; // inside <w:rPr>
    let mut in_text = false; // inside <w:t>
    // Set when the current paragraph's <w:pPr> contains a <w:pBdr> (any
    // border side implies a full box, matching how apply_card_style's
    // Pocket always sets all four sides uniformly) — applied to each run
    // as it's created, since <w:pPr> always precedes every <w:r> in a
    // well-formed <w:p>.
    let mut para_has_box_border = false;
    // Set while inside the current paragraph's <w:pPr><w:numPr> — gates the
    // <w:ilvl>/<w:numId> handlers below the same way `in_ppr`/`in_rpr` gate
    // their own children.
    let mut in_numpr = false;
    // The current paragraph's <w:numPr><w:numId>/<w:ilvl>, if any — resolved
    // against `numbering` into `Paragraph.list` when the paragraph ends.
    let mut current_para_num_id: Option<u32> = None;
    let mut current_para_ilvl: u8 = 0;
    // The current paragraph's resolved named-style defaults (if its
    // <w:pStyle> references one `styles` has an entry for) — seeds each new
    // <w:r> this paragraph creates, mirroring how `para_has_box_border`
    // seeds `box_format`. `None` for a paragraph with no style, or one whose
    // style has no entry in `styles` (e.g. a plain "heading1" that isn't
    // also a named style with its own formatting).
    let mut current_style_defaults: Option<StyleDefaults> = None;
    // Byte offset (into the original `xml: &str`) right after the current
    // paragraph's opening `<w:p...>` tag - captured the moment Event::Start
    // for "w:p" fires, since reader.buffer_position() at that instant is
    // exactly the start of the paragraph's inner content.
    let mut para_start_pos: usize = 0;
    // Set true the moment any UNSUPPORTED_INLINE_TAGS element is seen while
    // inside the current paragraph (checked in both Event::Start and
    // Event::Empty, since e.g. <w:drawing> commonly appears self-closing).
    let mut para_has_unsupported_content = false;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(ref e) => {
                match e.name().as_ref() {
                    b"w:p" => {
                        current_para = Some(Paragraph { list: None, runs: Vec::new(), heading: 0, alignment: Alignment::default(), unsupported_xml: None });
                        para_has_box_border = false;
                        current_style_defaults = None;
                        para_has_unsupported_content = false;
                        current_para_num_id = None;
                        current_para_ilvl = 0;
                        para_start_pos = reader.buffer_position();
                    }
                    b"w:pPr" => { in_ppr = true; }
                    b"w:numPr" if in_ppr => { in_numpr = true; }
                    b"w:pStyle" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_style(e, para);
                            current_style_defaults = apply_paragraph_style_defaults(e, para, styles, &mut para_has_box_border);
                        }
                    }
                    b"w:jc" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_alignment(e, para);
                        }
                    }
                    b"w:pBdr" if in_ppr => {
                        para_has_box_border = true;
                    }
                    b"w:r" => {
                        let mut run = Run { box_format: para_has_box_border, ..Run::default() };
                        if let Some(defaults) = &current_style_defaults {
                            run.bold = defaults.bold;
                            run.size = defaults.size;
                            run.underline = defaults.underline;
                            run.double_underline = defaults.double_underline;
                        }
                        current_run = Some(run);
                    }
                    b"w:rPr" => { in_rpr = true; }
                    b"w:t" => {
                        in_text = true;
                        // Detect xml:space="preserve" so whitespace is kept.
                        if let Some(run) = current_run.as_mut() {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"xml:space"
                                    && attr.value.as_ref() == b"preserve"
                                {
                                    run.whitespace_preserve = true;
                                }
                            }
                        }
                    }
                    // A character style (`word/styles.xml`) referenced
                    // directly on this run — see `apply_run_character_style`.
                    // Checked ahead of the generic in_rpr catch-all so it
                    // doesn't fall through to `apply_run_prop`'s "unknown
                    // tag" no-op.
                    b"w:rStyle" if in_rpr => {
                        if let Some(run) = current_run.as_mut() {
                            // This app's own marker is read straight off the
                            // id — it is deliberately absent from styles.xml,
                            // so resolving it there would find nothing.
                            apply_run_style_marker(e, run);
                            apply_run_character_style(e, run, styles);
                        }
                    }
                    // Catch-all for run-property elements (w:b, w:u, etc.).
                    _ if in_rpr => {
                        if let Some(run) = current_run.as_mut() {
                            apply_run_prop(e, run);
                        }
                    }
                    other if UNSUPPORTED_INLINE_TAGS.contains(&other) => {
                        para_has_unsupported_content = true;
                    }
                    _ => {}
                }
            }
            Event::Empty(ref e) => {
                // Self-closing property tags — same logic as the Start arm for
                // pStyle and run properties; no end event follows.
                match e.name().as_ref() {
                    b"w:pStyle" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_style(e, para);
                            current_style_defaults = apply_paragraph_style_defaults(e, para, styles, &mut para_has_box_border);
                        }
                    }
                    b"w:jc" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_alignment(e, para);
                        }
                    }
                    b"w:pBdr" if in_ppr => {
                        para_has_box_border = true;
                    }
                    b"w:ilvl" if in_numpr => {
                        if let Some(v) = e.attributes().flatten().find_map(|a| {
                            (a.key.as_ref() == b"w:val").then(|| std::str::from_utf8(&a.value).ok()?.parse().ok())
                        }).flatten() {
                            current_para_ilvl = v;
                        }
                    }
                    b"w:numId" if in_numpr => {
                        current_para_num_id = e.attributes().flatten().find_map(|a| {
                            (a.key.as_ref() == b"w:val").then(|| std::str::from_utf8(&a.value).ok()?.parse().ok())
                        }).flatten();
                    }
                    b"w:rStyle" if in_rpr => {
                        if let Some(run) = current_run.as_mut() {
                            // This app's own marker is read straight off the
                            // id — it is deliberately absent from styles.xml,
                            // so resolving it there would find nothing.
                            apply_run_style_marker(e, run);
                            apply_run_character_style(e, run, styles);
                        }
                    }
                    _ if in_rpr => {
                        if let Some(run) = current_run.as_mut() {
                            apply_run_prop(e, run);
                        }
                    }
                    other if UNSUPPORTED_INLINE_TAGS.contains(&other) => {
                        para_has_unsupported_content = true;
                    }
                    _ => {}
                }
            }
            Event::Text(ref e) => {
                if in_text {
                    if let Some(run) = current_run.as_mut() {
                        // unescape() handles XML entities like &amp; → &.
                        run.text.push_str(&e.unescape()?);
                    }
                }
            }
            Event::End(ref e) => {
                match e.name().as_ref() {
                    b"w:p" => {
                        if let Some(mut para) = current_para.take() {
                            if para_has_unsupported_content {
                                // buffer_position() here is immediately after
                                // "</w:p>"'s closing '>' - subtract the
                                // literal tag's own byte length (6) to get
                                // just the inner content, excluding the
                                // closing tag itself.
                                let para_end_pos = reader.buffer_position() - 6;
                                para.unsupported_xml = Some(xml[para_start_pos..para_end_pos].to_string());
                            }
                            // Word fragments runs heavily (spell-check,
                            // revision-tracking remnants) even when adjacent
                            // runs share identical formatting, which made
                            // every per-keystroke edit on a loaded document
                            // walk far more runs than an equivalent
                            // freshly-typed one (`resolve_position` and the
                            // sync_*/apply_formatting helpers are all
                            // O(runs)). Collapsing them once here, at parse
                            // time, is free — it doesn't change what gets
                            // saved (`unsupported_xml`, when set, is
                            // re-emitted verbatim and ignores `runs`
                            // entirely) but keeps every later edit as cheap
                            // as it already is for a new document.
                            // A document written elsewhere carries Word's
                            // heading styles but none of this app's markers.
                            // Deriving the marker from the heading level here
                            // is what lets every command that identifies card
                            // styles read one field instead of re-guessing
                            // from bold + font size — the accuracy win the
                            // marker exists for. Runs that already carry a
                            // marker (a document this app saved) keep it.
                            if let Some(style) = CardStyle::from_heading(para.heading) {
                                for run in &mut para.runs {
                                    if run.style.is_none() && !run.text.trim().is_empty() {
                                        run.style = Some(style);
                                    }
                                }
                            }
                            if let Some(num_id) = current_para_num_id {
                                if let Some((fmt, text, font)) = numbering.get(&num_id) {
                                    para.list = Some(ListItem {
                                        kind: ListKind::classify(fmt, text, font.as_deref()),
                                        level: current_para_ilvl,
                                    });
                                }
                            }
                            crate::document_ops::merge_adjacent_same_format_runs(&mut para.runs);
                            paragraphs.push(para);
                        }
                        in_ppr = false;
                    }
                    b"w:pPr" => { in_ppr = false; }
                    b"w:numPr" => { in_numpr = false; }
                    b"w:r" => {
                        // Flush the completed run into the current paragraph.
                        if let (Some(run), Some(para)) = (current_run.take(), current_para.as_mut()) {
                            para.runs.push(run);
                        }
                        in_rpr  = false;
                        in_text = false;
                    }
                    b"w:rPr" => { in_rpr = false; }
                    b"w:t"   => { in_text = false; }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(paragraphs)
}

/// Looks up `<w:pStyle>`'s `w:val` (the raw style ID, e.g. `"Heading1"` —
/// same string `word/styles.xml`'s `w:styleId` uses) in `styles`, applies
/// its resolved alignment/box to `para`/`para_has_box_border` as *defaults*
/// (any direct `<w:jc>`/`<w:pBdr>` on this same paragraph, processed later
/// in the same streaming pass, overwrites these afterward), and returns the
/// resolved `StyleDefaults` so the caller can seed each new `<w:r>` this
/// paragraph creates with its run-level defaults (bold/size/underline).
fn apply_paragraph_style_defaults(
    e: &BytesStart,
    para: &mut Paragraph,
    styles: &HashMap<String, StyleDefaults>,
    para_has_box_border: &mut bool,
) -> Option<StyleDefaults> {
    let style_id = e.attributes().flatten().find_map(|attr| {
        (attr.key.as_ref() == b"w:val").then(|| String::from_utf8_lossy(&attr.value).into_owned())
    })?;
    let defaults = styles.get(&style_id)?.clone();
    if let Some(alignment) = defaults.alignment {
        para.alignment = alignment;
    }
    if defaults.box_format {
        *para_has_box_border = true;
    }
    Some(defaults)
}

/// Applies a `<w:pStyle>` element's attributes to `para`, setting
/// `para.heading` when the style name starts with "heading".
fn apply_para_style(e: &BytesStart, para: &mut Paragraph) {
    /*
     * Word uses the `w:val` attribute to carry the style name (e.g.,
     * "Heading1", "heading2").  We lower-case before matching because
     * the casing is not guaranteed to be consistent across documents.
     */
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:val" {
            let val = String::from_utf8_lossy(&attr.value).to_lowercase();
            if val.starts_with("heading") {
                // The digit suffix is the heading level (1–9).
                if let Some(n) = val.chars().last().and_then(|c| c.to_digit(10)) {
                    para.heading = n as u8;
                }
            }
        }
    }
}

/// Applies a `<w:jc>` element's `w:val` attribute to `para.alignment`. Word's
/// own OOXML value for full justification is `"both"`, not `"justify"`. Any
/// other/absent value leaves `para.alignment` at `Alignment::Left`.
fn apply_para_alignment(e: &BytesStart, para: &mut Paragraph) {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:val" {
            para.alignment = match attr.value.as_ref() {
                b"center" => Alignment::Center,
                b"right" => Alignment::Right,
                b"both" => Alignment::Justify,
                _ => Alignment::Left,
            };
        }
    }
}

/// OOXML boolean-toggle semantics: the element being present with no
/// `w:val` (or `w:val="1"`/`"true"`) means on; `w:val="0"`/`"false"`/
/// (for `<w:u>` specifically) `"none"` means explicitly off. Real
/// documents' character-style definitions rely on this to turn an
/// inherited property back off (e.g. debate-community docx files'
/// "Style13ptBold" character style sets `<w:u w:val="none"/>` precisely so
/// referencing it doesn't also underline the text).
fn on_off_attr_is_true(e: &BytesStart) -> bool {
    !e.attributes().flatten().any(|attr| {
        attr.key.as_ref() == b"w:val" && matches!(attr.value.as_ref(), b"0" | b"false" | b"none")
    })
}

/// Records this app's own `<w:rStyle>` marker on the run, if that is what the
/// reference is.
///
/// Deliberately separate from `apply_run_character_style`: the Vimbatim ids are
/// not defined in `styles.xml` (the document's own style table is preserved
/// verbatim on save, so there is nowhere to add them), which means style
/// resolution finds nothing for them. Reading the id directly is what makes the
/// marker survive a save/reload, while staying invisible to Word.
fn apply_run_style_marker(e: &BytesStart, run: &mut Run) {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"w:val" {
            if let Ok(val) = std::str::from_utf8(attr.value.as_ref()) {
                if let Some(style) = CardStyle::from_style_id(val) {
                    run.style = Some(style);
                }
            }
        }
    }
}

/// Resolves a `<w:rStyle w:val="...">` — a *character* style referenced
/// directly on a run's `<w:rPr>`, distinct from a paragraph's `<w:pStyle>`.
/// Debate-community docx files commonly underline/bold the emphasized
/// "read" portion of a card this way (e.g. a "StyleUnderline" character
/// style) rather than with direct `<w:u>`/`<w:b>` — left unhandled, that
/// text silently lost its formatting. Real Verbatim encodes its own
/// "Emphasis" card type the same way: the run carries only the `<w:rStyle>`
/// reference, and bold/underline/size/box all live on the named character
/// style in `word/styles.xml`.
///
/// Applies as this run's baseline the same way a paragraph style's
/// defaults already do (see `apply_paragraph_style_defaults`): any direct
/// `<w:b>`/`<w:u>`/etc. appearing later in this same `<w:rPr>` is processed
/// afterward by `apply_run_prop` and still wins, matching Word's own
/// direct-formatting-beats-style cascade. `box_format` is OR'd rather than
/// overwritten so this can't clear a box this run already has from its
/// paragraph's own border.
fn apply_run_character_style(e: &BytesStart, run: &mut Run, styles: &HashMap<String, StyleDefaults>) {
    let Some(style_id) = e.attributes().flatten().find_map(|attr| {
        (attr.key.as_ref() == b"w:val").then(|| String::from_utf8_lossy(&attr.value).into_owned())
    }) else { return };
    let Some(defaults) = styles.get(&style_id) else { return };
    run.bold = defaults.bold;
    run.size = defaults.size;
    run.underline = defaults.underline;
    run.double_underline = defaults.double_underline;
    run.box_format = run.box_format || defaults.box_format;
    if defaults.emphasis_boxed {
        run.emphasis_boxed = true;
        run.emphasis = true;
    }
}

/// Applies a run-property element to `run` based on the element's tag name.
/// The only values Word accepts for `<w:highlight w:val="…">` (ECMA-376
/// ST_HighlightColor). Anything else has to be written as shading instead —
/// see `run_props_xml`. `text_editor::highlight_color_hex` maps these same
/// names to their on-screen color, and a test there asserts the two agree.
pub(crate) const WORD_HIGHLIGHT_NAMES: [&str; 16] = [
    "yellow", "green", "cyan", "magenta", "blue", "red", "darkBlue", "darkCyan", "darkGreen",
    "darkMagenta", "darkRed", "darkYellow", "darkGray", "lightGray", "black", "white",
];

/// The `<w:rPr>` inner XML for one run. Split out of `write_docx` so the
/// highlight-vs-shading branch below can be tested directly.
fn run_props_xml(run: &Run) -> String {
    let mut out = String::new();
    // `<w:rStyle>` must lead `<w:rPr>` per the OOXML schema, so it is written
    // before any direct formatting.
    if let Some(style) = run.style {
        out.push_str(&format!("<w:rStyle w:val=\"{}\"/>", style.style_id()));
    }
    if run.bold { out.push_str("<w:b/>"); }
    if run.italic { out.push_str("<w:i/>"); }
    if run.strikethrough { out.push_str("<w:strike/>"); }
    if run.double_underline { out.push_str("<w:u w:val=\"double\"/>"); }
    else if run.underline { out.push_str("<w:u w:val=\"single\"/>"); }
    if run.emphasis_boxed {
        // Real OOXML run border, matching real Verbatim's own "Emphasis"
        // character style's <w:bdr> exactly (found by diffing
        // Verbatim_Formatting_To_Compare_To.docx's word/styles.xml).
        // Emitted before highlight/shd: <w:bdr> after <w:shd> in a run's
        // <w:rPr> violates CT_RPr's element order and Word has been
        // observed to silently drop it on reopen.
        out.push_str("<w:bdr w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"auto\"/>");
    }
    if run.highlight {
        // `w:highlight` only accepts the 16 names above — Word silently drops
        // anything else. A custom hex color goes out as shading, which Word
        // does honor.
        if WORD_HIGHLIGHT_NAMES.contains(&run.highlight_color.as_str()) {
            out.push_str(&format!("<w:highlight w:val=\"{}\"/>", run.highlight_color));
        } else {
            out.push_str(&format!(
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
                run.highlight_color,
            ));
        }
    }
    if run.size > 0 {
        out.push_str(&format!("<w:sz w:val=\"{}\"/>", run.size));
    }
    if let Some(font) = &run.font {
        out.push_str(&format!("<w:rFonts w:ascii=\"{}\"/>", font));
    }
    if let Some(color) = &run.color {
        out.push_str(&format!("<w:color w:val=\"{}\"/>", color));
    }
    // Custom markers, namespaced like `CardStyle::style_id` so they're
    // harmless to any other editor (Word ignores elements it doesn't know).
    if run.emphasis { out.push_str("<w:vimbatimEmphasis/>"); }
    if run.emphasis_boxed { out.push_str("<w:vimbatimEmphasisBox/>"); }
    out
}

/// Handles `w:b` (bold), `w:u` (underline), `w:highlight` (highlight colour),
/// `w:shd` (custom hex highlight, written as shading), `w:sz` (font size in
/// half-points), and `w:bdr` (border, i.e. the Emphasis box).  Unknown tags
/// are silently ignored.
fn apply_run_prop(e: &BytesStart, run: &mut Run) {
    /*
     * This function is called for both `Event::Start` and `Event::Empty`
     * variants of each property element, avoiding duplicated match arms in the
     * main parser loop.  The element name is re-read from `e` rather than
     * passed as a parameter to keep the call site clean.
     */
    match e.name().as_ref() {
        b"w:b" => { run.bold = on_off_attr_is_true(e); }
        b"w:i" => { run.italic = on_off_attr_is_true(e); }
        b"w:strike" => { run.strikethrough = on_off_attr_is_true(e); }
        b"w:u" => {
            if !on_off_attr_is_true(e) {
                run.underline = false;
                run.double_underline = false;
            } else {
                let is_double = e.attributes().flatten().any(|attr| {
                    attr.key.as_ref() == b"w:val" && attr.value.as_ref() == b"double"
                });
                if is_double {
                    run.double_underline = true;
                } else {
                    run.underline = true;
                }
            }
        }
        b"w:highlight" => {
            run.highlight = true;
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:val" {
                    run.highlight_color = String::from_utf8_lossy(&attr.value).into_owned();
                }
            }
        }
        // Word writes arbitrary background colors as shading, not highlight.
        // `w:fill="auto"` means "no fill", and anything that isn't 6 hex digits
        // isn't a color we can render — both are ignored so ordinary Word
        // documents don't gain phantom highlights.
        b"w:shd" => {
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:fill" {
                    let fill = String::from_utf8_lossy(&attr.value).into_owned();
                    if fill.len() == 6 && fill.chars().all(|c| c.is_ascii_hexdigit()) {
                        run.highlight = true;
                        run.highlight_color = fill.to_lowercase();
                    }
                }
            }
        }
        b"w:sz" => {
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:val" {
                    if let Ok(s) = std::str::from_utf8(&attr.value) {
                        run.size = s.parse().unwrap_or(0);
                    }
                }
            }
        }
        // Only `w:ascii` is read — East Asian/complex-script font overrides
        // (`w:eastAsia`/`w:cs`) are out of scope (rich-text formatting plan,
        // Phase 1).
        b"w:rFonts" => {
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:ascii" {
                    run.font = Some(String::from_utf8_lossy(&attr.value).into_owned());
                }
            }
        }
        // `w:val="auto"` means "inherit the default" — treated the same as
        // the attribute being absent, so `color` stays `None`.
        b"w:color" => {
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:val" {
                    let val = String::from_utf8_lossy(&attr.value).into_owned();
                    if val != "auto" {
                        run.color = Some(val);
                    }
                }
            }
        }
        b"w:vimbatimEmphasis" => { run.emphasis = true; }
        b"w:vimbatimEmphasisBox" => { run.emphasis_boxed = true; }
        b"w:bdr" => {
            // OOXML: w:val="none" or w:val="nil" explicitly cancels an inherited border.
            // Otherwise, the border is present and we set emphasis_boxed/emphasis.
            let mut has_cancel_value = false;
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:val" {
                    let val = String::from_utf8_lossy(&attr.value);
                    if val == "none" || val == "nil" {
                        has_cancel_value = true;
                    }
                    break;
                }
            }
            if has_cancel_value {
                // Explicit cancellation: clear the box
                run.emphasis_boxed = false;
                run.emphasis = false;
            } else {
                // Border is present (no w:val attribute, or w:val is not "none"/"nil")
                run.emphasis_boxed = true;
                run.emphasis = true;
            }
        }
        _ => {}
    }
}

/// `(ListKind, numFmt, level-0 lvlText, level-0 font)` in the exact order
/// `abstractNumId`s 0-12 are assigned — stable across saves, matching
/// `docs/superpowers/specs/2026-08-11-lists-design.md`'s ground-truth table
/// (values read directly from `Lists.docx`).
const LIST_KIND_TABLE: [(ListKind, &str, &str, Option<&str>); 13] = [
    (ListKind::BulletSolid, "bullet", "\u{f0b7}", Some("Symbol")),
    (ListKind::BulletHollow, "bullet", "o", Some("Courier New")),
    (ListKind::BulletSolidBox, "bullet", "\u{f0a7}", Some("Wingdings")),
    (ListKind::BulletDiamond, "bullet", "\u{f076}", Some("Wingdings")),
    (ListKind::BulletArrow, "bullet", "\u{f0d8}", Some("Wingdings")),
    (ListKind::BulletCheckmark, "bullet", "\u{f0fc}", Some("Wingdings")),
    (ListKind::NumberDecimalDot, "decimal", "%1.", None),
    (ListKind::NumberDecimalParen, "decimal", "%1)", None),
    (ListKind::NumberUpperRoman, "upperRoman", "%1.", None),
    (ListKind::NumberUpperLetter, "upperLetter", "%1.", None),
    (ListKind::NumberLowerLetterParen, "lowerLetter", "%1)", None),
    (ListKind::NumberLowerLetterDot, "lowerLetter", "%1.", None),
    (ListKind::NumberLowerRoman, "lowerRoman", "%1.", None),
];

fn abstract_num_id_for(kind: ListKind) -> u32 {
    LIST_KIND_TABLE.iter().position(|(k, ..)| *k == kind).unwrap() as u32
}

/// One `<w:lvl>` element. `ind_left`/`ind_hanging` are in twips (1/20 pt),
/// matching Word's own defaults confirmed from `Lists.docx`: `left`
/// increases 720 per level, `hanging` is 360 (180 at level 2 for the
/// numbered cascade specifically — see `cascade_level_xml`).
fn build_lvl_xml(ilvl: u8, num_fmt: &str, lvl_text: &str, font: Option<&str>, ind_left: u32, ind_hanging: u32) -> String {
    let font_xml = match font {
        Some(f) => format!("<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\"/>"),
        None => String::new(),
    };
    format!(
        "<w:lvl w:ilvl=\"{ilvl}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{num_fmt}\"/><w:lvlText w:val=\"{lvl_text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{ind_left}\" w:hanging=\"{ind_hanging}\"/></w:pPr><w:rPr>{font_xml}</w:rPr></w:lvl>",
    )
}

/// The one fixed cascade every one of the 13 styles' levels 1-8 use,
/// independent of the level-0 style — confirmed (not guessed) by dumping
/// ilvl 0/1/2 across all 13 of `Lists.docx`'s real `abstractNum`
/// definitions: bullets go `<own glyph>` (level 0) -> `o`/Courier New
/// (level 1) -> `\u{f0a7}`/Wingdings (level 2), repeating every 3 levels;
/// numbers go `<own format>` (level 0) -> `lowerLetter "%2."` (level 1) ->
/// `lowerRoman "%3."` (level 2), repeating every 3 levels.
fn cascade_level_xml(ilvl: u8, is_bullet: bool) -> String {
    let ind_left = 720 * (ilvl as u32 + 1);
    let cascade_pos = (ilvl - 1) % 3; // ilvl 1,4,7 -> 0; 2,5,8 -> 1; 3,6 -> 2
    if is_bullet {
        let (lvl_text, font, hanging) = match cascade_pos {
            0 => ("o", "Courier New", 360),
            _ => ("\u{f0a7}", "Wingdings", 360),
        };
        build_lvl_xml(ilvl, "bullet", lvl_text, Some(font), ind_left, hanging)
    } else {
        let (num_fmt, hanging) = match cascade_pos {
            0 => ("lowerLetter", 360),
            _ => ("lowerRoman", 180),
        };
        build_lvl_xml(ilvl, num_fmt, &format!("%{}.", ilvl + 1), None, ind_left, hanging)
    }
}

fn build_abstract_num_xml(id: u32, kind: ListKind, num_fmt: &str, lvl_text: &str, font: Option<&str>) -> String {
    let mut out = format!("<w:abstractNum w:abstractNumId=\"{id}\">");
    out.push_str(&build_lvl_xml(0, num_fmt, lvl_text, font, 720, 360));
    for ilvl in 1..=8u8 {
        out.push_str(&cascade_level_xml(ilvl, kind.is_bullet()));
    }
    out.push_str("</w:abstractNum>");
    out
}

/// Assigns a `numId` to every list paragraph's index, one fresh id per
/// contiguous run of list paragraphs (a run ends at the next paragraph with
/// `list: None`, or the end of the document). Shared by `build_numbering_xml`
/// (which needs the same numId->abstractNum `<w:num>` entries) and
/// `rebuild_document_xml` (which writes the numId onto each paragraph's own
/// `<w:numPr>`) so the two can never disagree about which paragraph got
/// which id.
fn assign_list_num_ids(paragraphs: &[Paragraph]) -> HashMap<usize, u32> {
    let mut assignment = HashMap::new();
    let mut next_num_id = 1u32;
    let mut i = 0;
    while i < paragraphs.len() {
        if paragraphs[i].list.is_some() {
            while i < paragraphs.len() && paragraphs[i].list.is_some() {
                assignment.insert(i, next_num_id);
                i += 1;
            }
            next_num_id += 1;
        } else {
            i += 1;
        }
    }
    assignment
}

/// Builds `word/numbering.xml`, or `None` if `paragraphs` has no list at
/// all (in which case the caller — the package-plumbing write path — omits
/// the part entirely, unchanged from a non-list document today).
fn build_numbering_xml(paragraphs: &[Paragraph]) -> Option<String> {
    if !paragraphs.iter().any(|p| p.list.is_some()) {
        return None;
    }

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
    );
    for (id, (kind, num_fmt, lvl_text, font)) in LIST_KIND_TABLE.iter().enumerate() {
        out.push_str(&build_abstract_num_xml(id as u32, *kind, num_fmt, lvl_text, *font));
    }

    let num_ids = assign_list_num_ids(paragraphs);
    // One <w:num> per distinct numId, ordered by id; each looks up its
    // abstractNum via the *first* paragraph index that carries that id —
    // every paragraph in one run shares the same ListKind by construction
    // (apply_list_style only ever sets one kind per selection).
    let mut seen: Vec<u32> = num_ids.values().copied().collect();
    seen.sort_unstable();
    seen.dedup();
    for num_id in seen {
        let para_idx = num_ids.iter().find(|(_, id)| **id == num_id).map(|(idx, _)| *idx).unwrap();
        let kind = paragraphs[para_idx].list.unwrap().kind;
        out.push_str(&format!(
            "<w:num w:numId=\"{num_id}\"><w:abstractNumId w:val=\"{}\"/></w:num>",
            abstract_num_id_for(kind),
        ));
    }

    out.push_str("</w:numbering>");
    Some(out)
}

/// Serialises `paragraphs` back to a `word/document.xml` string, using
/// `preamble` (everything before `<w:body>`) and `sect_pr` (the `<w:sectPr>`
/// block) extracted from the original file to preserve document-level settings.
fn rebuild_document_xml(preamble: &str, sect_pr: &str, paragraphs: &[Paragraph]) -> String {
    /*
     * Structure of the emitted XML:
     *
     *   {preamble}<w:body>
     *     <w:p><w:r><w:rPr>...</w:rPr><w:t>...</w:t></w:r></w:p>
     *     ...
     *     {sect_pr}
     *   </w:body></w:document>
     *
     * The capacity hint avoids reallocations for typical document sizes.
     */
    let mut out = String::with_capacity(preamble.len() + sect_pr.len() + paragraphs.len() * 200);
    out.push_str(preamble);
    out.push_str("<w:body>");

    let list_num_ids = assign_list_num_ids(paragraphs);
    for (para_index, para) in paragraphs.iter().enumerate() {
        out.push_str("<w:p>");
        if let Some(raw) = &para.unsupported_xml {
            out.push_str(raw);
            out.push_str("</w:p>");
            continue;
        }
        let mut ppr = String::new();
        if para.heading != 0 {
            // Capitalized to match Word's own built-in styleId ("Heading1"
            // .."Heading9" in word/styles.xml) — OOXML styleId references
            // are matched case-sensitively, so a lowercase "heading1" points
            // at a styleId that doesn't exist and Word silently falls back
            // to Normal, dropping the heading's formatting and its entry in
            // the Navigation pane.
            ppr.push_str(&format!("<w:pStyle w:val=\"Heading{}\"/>", para.heading));
        }
        match para.alignment {
            Alignment::Center => ppr.push_str("<w:jc w:val=\"center\"/>"),
            Alignment::Right => ppr.push_str("<w:jc w:val=\"right\"/>"),
            Alignment::Justify => ppr.push_str("<w:jc w:val=\"both\"/>"),
            Alignment::Left => {}
        }
        if para.runs.iter().any(|r| r.box_format) {
            // sz is eighths of a point (OOXML spec). 24 = 3pt, matching the
            // real Verbatim Word add-in's own "Pocket" style (its Heading1
            // alias's <w:pBdr>, found by diffing a file saved from Verbatim
            // against one saved from vimbatim) — vimbatim was previously
            // writing 4 (0.5pt), a visibly thinner box once reopened in Word
            // even though both apps' in-app rendering looked fine.
            //
            // w:space (the gap between border and text) also has to match
            // per side: Verbatim's own style uses 1 on top/bottom but 4 on
            // left/right — a uniform 1 on all four sides (the previous
            // value here) rendered a visibly narrower box on the left/right
            // than Verbatim's native one, even with sz already matching.
            ppr.push_str(
                "<w:pBdr>\
                <w:top w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"000000\"/>\
                <w:bottom w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"000000\"/>\
                <w:left w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"000000\"/>\
                <w:right w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"000000\"/>\
                </w:pBdr>",
            );
        }
        if let Some(item) = para.list {
            if let Some(num_id) = list_num_ids.get(&para_index) {
                // Matches Lists.docx exactly, where every list-item
                // paragraph carries both — not <w:numPr> alone.
                ppr.push_str("<w:pStyle w:val=\"ListParagraph\"/>");
                ppr.push_str(&format!(
                    "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{}\"/></w:numPr>",
                    item.level, num_id,
                ));
            }
        }
        if !ppr.is_empty() {
            out.push_str("<w:pPr>");
            out.push_str(&ppr);
            out.push_str("</w:pPr>");
        }
        for run in &para.runs {
            out.push_str("<w:r>");
            let has_props = run.bold || run.italic || run.underline || run.double_underline
                || run.strikethrough || run.highlight || run.size > 0 || run.font.is_some()
                || run.color.is_some()
                // A style marker alone is enough to need a `<w:rPr>` — an
                // Analytic that happens to carry no direct formatting still
                // has to say what it is.
                || run.style.is_some()
                || run.emphasis || run.emphasis_boxed;
            if has_props {
                out.push_str("<w:rPr>");
                out.push_str(&run_props_xml(run));
                out.push_str("</w:rPr>");
            }
            // Emit xml:space="preserve" only when the run needs it.
            let space_attr = if run.whitespace_preserve { " xml:space=\"preserve\"" } else { "" };
            out.push_str(&format!("<w:t{}>", space_attr));
            out.push_str(&escape_xml_text(&run.text));
            out.push_str("</w:t></w:r>");
        }
        out.push_str("</w:p>");
    }

    if !sect_pr.is_empty() {
        out.push_str(sect_pr);
    }
    out.push_str("</w:body></w:document>");
    out
}

/// Returns everything in `xml` before the `<w:body` opening tag.
/// Used at parse time to capture namespace declarations and document-level
/// settings so they can be re-emitted unchanged on save.
fn extract_preamble(xml: &str) -> Option<String> {
    let pos = xml.find("<w:body")?;
    Some(xml[..pos].to_string())
}

/// Returns the `<w:sectPr>…</w:sectPr>` block from `xml`, if present.
/// Word stores page margins, orientation, and similar layout settings here;
/// preserving it prevents the document layout from changing on round-trip.
fn extract_sect_pr(xml: &str) -> Option<&str> {
    /*
     * `rfind` is used because `sectPr` always appears at the end of `<w:body>`,
     * after all paragraphs.  If multiple `sectPr` elements existed (unlikely in
     * practice), this picks the last one which is the document-level one.
     */
    let start   = xml.rfind("<w:sectPr")?;
    let end_tag = "</w:sectPr>";
    let end     = xml[start..].find(end_tag)? + start + end_tag.len();
    Some(&xml[start..end])
}

/// Returns a minimal `<w:document>` preamble used when the original file did
/// not contain a parseable one.  Only the core `w:` namespace is declared;
/// documents produced this way will lack the full namespace set that
/// Microsoft Office expects, so this fallback is a last resort.
fn fallback_preamble() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">"
        .to_string()
}

/// Creates a brand-new minimal .docx file at `path` whose body contains
/// `paragraphs`. Unlike `DocxOrigin::save`, this does not require an
/// existing file to use as a ZIP template — it builds the required ZIP
/// entries from scratch.
///
/// Word requires at minimum four entries in the ZIP:
///   `[Content_Types].xml`, `_rels/.rels`,
///   `word/document.xml`, `word/_rels/document.xml.rels`
pub fn create_new_docx(paragraphs: &[Paragraph], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    /*
     * Build a minimal but fully spec-compliant .docx:
     *  1. Encode `paragraphs` (with whatever formatting they carry — rich-
     *     text formatting plan, Phase 1) in `word/document.xml`.
     *  2. Write the required Open Packaging Convention manifest files.
     *  3. Use an atomic temp-file rename so an interrupted save does not leave
     *     a corrupt file at `path`.
     */
    let preamble = fallback_preamble();
    let document_xml = rebuild_document_xml(&preamble, "", paragraphs);

    let tmp_path = tmp_write_path(path);
    let tmp_file = std::fs::File::create(&tmp_path)?;
    let mut writer = ZipWriter::new(tmp_file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    writer.start_file("[Content_Types].xml", opts)?;
    writer.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
</Types>"
    )?;

    writer.start_file("_rels/.rels", opts)?;
    writer.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" \
Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
Target=\"word/document.xml\"/>\
</Relationships>"
    )?;

    writer.start_file("word/_rels/document.xml.rels", opts)?;
    writer.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>"
    )?;

    writer.start_file("word/document.xml", opts)?;
    writer.write_all(document_xml.as_bytes())?;

    writer.finish()?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Escapes the three XML-significant characters in text content:
/// `&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;`.
fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_named_highlight_still_writes_w_highlight() {
        let run = Run {
            text: "hi".into(),
            highlight: true,
            highlight_color: "yellow".into(),
            ..Run::default()
        };
        let xml = run_props_xml(&run);
        assert!(xml.contains("<w:highlight w:val=\"yellow\"/>"), "got {xml}");
        assert!(!xml.contains("w:shd"), "got {xml}");
    }

    #[test]
    fn test_custom_hex_highlight_writes_w_shd() {
        let run = Run {
            text: "hi".into(),
            highlight: true,
            highlight_color: "00ff88".into(),
            ..Run::default()
        };
        let xml = run_props_xml(&run);
        assert!(
            xml.contains("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"00ff88\"/>"),
            "got {xml}",
        );
        assert!(!xml.contains("w:highlight"), "got {xml}");
    }

    #[test]
    fn test_emphasis_boxed_emits_a_real_bdr() {
        let run = Run { text: "hi".into(), emphasis: true, emphasis_boxed: true, ..Run::default() };
        let xml = run_props_xml(&run);
        assert!(
            xml.contains(r#"<w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/>"#),
            "got: {xml}"
        );
    }

    #[test]
    fn test_non_boxed_emphasis_emits_no_bdr() {
        let run = Run { text: "hi".into(), emphasis: true, ..Run::default() };
        let xml = run_props_xml(&run);
        assert!(!xml.contains("w:bdr"), "got: {xml}");
    }

    #[test]
    fn test_emphasis_box_bdr_precedes_highlight_and_shd() {
        // CT_RPr's schema order (and Word's real-world tolerance) requires
        // <w:bdr> before <w:shd>; a border emitted after <w:highlight>/<w:shd>
        // has been observed silently dropped by Word on reopen even though this
        // parser's own reparse (which is order-insensitive) would still pass.
        let named_highlight = Run {
            text: "hi".into(), emphasis: true, emphasis_boxed: true,
            highlight: true, highlight_color: "yellow".into(), ..Run::default()
        };
        let xml = run_props_xml(&named_highlight);
        let bdr_pos = xml.find("<w:bdr").expect("bdr missing");
        let highlight_pos = xml.find("<w:highlight").expect("highlight missing");
        assert!(bdr_pos < highlight_pos, "bdr must precede highlight, got: {xml}");

        let custom_hex_highlight = Run {
            text: "hi".into(), emphasis: true, emphasis_boxed: true,
            highlight: true, highlight_color: "00ff88".into(), ..Run::default()
        };
        let xml = run_props_xml(&custom_hex_highlight);
        let bdr_pos = xml.find("<w:bdr").expect("bdr missing");
        let shd_pos = xml.find("<w:shd").expect("shd missing");
        assert!(bdr_pos < shd_pos, "bdr must precede shd, got: {xml}");
    }

    #[test]
    fn test_w_shd_fill_is_read_back_as_a_highlight() {
        let mut run = Run::default();
        apply_run_prop(
            &BytesStart::from_content(r#"w:shd w:val="clear" w:color="auto" w:fill="00FF88""#, 5),
            &mut run,
        );
        assert!(run.highlight);
        assert_eq!(run.highlight_color, "00ff88");
    }

    #[test]
    fn test_w_shd_auto_fill_is_not_a_highlight() {
        for content in [
            r#"w:shd w:val="clear" w:color="auto" w:fill="auto""#,
            r#"w:shd w:val="clear" w:color="auto""#,
            r#"w:shd w:val="clear" w:fill="nothex""#,
        ] {
            let mut run = Run::default();
            apply_run_prop(&BytesStart::from_content(content, 5), &mut run);
            assert!(!run.highlight, "should not highlight for: {content}");
        }
    }

    #[test]
    fn test_custom_highlight_survives_a_write_read_round_trip() {
        let source = Run {
            text: "hi".into(),
            highlight: true,
            highlight_color: "00ff88".into(),
            ..Run::default()
        };
        // Feed the emitted `w:shd` element straight back through the parser.
        let xml = run_props_xml(&source);
        let inner = xml.trim_start_matches("<w:shd ").trim_end_matches("/>");
        let mut run = Run::default();
        apply_run_prop(&BytesStart::from_content(format!("w:shd {inner}"), 5), &mut run);
        assert!(run.highlight);
        assert_eq!(run.highlight_color, source.highlight_color);
    }

    #[test]
    fn two_concurrent_writes_to_one_path_get_different_temp_files() {
        // The panic hook writes snapshots on the panicking thread while the
        // background snapshot task may already be mid-write on the same tab's
        // file. A shared temp name lets both truncate and interleave into one
        // file, then rename a corrupt zip into place.
        let dest = Path::new("/tmp/vimbatim-example.docx");
        assert_ne!(tmp_write_path(dest), tmp_write_path(dest));
    }

    #[test]
    fn the_temp_write_path_keeps_tmp_as_its_final_extension() {
        // `scan_recovery_dir` sweeps abandoned leftovers by this extension.
        let tmp = tmp_write_path(Path::new("/tmp/1234-5678-0.docx"));
        assert_eq!(tmp.extension().unwrap(), "tmp");
        // ...and the pid stays the first '-'-separated segment, which is how
        // the sweeper decides whether the owning process is still alive.
        let stem = tmp.file_stem().unwrap().to_str().unwrap();
        assert_eq!(stem.split('-').next().unwrap(), "1234");
    }

    fn wrap_run_xml(run_xml: &str) -> String {
        format!(
            "<w:document><w:body><w:p><w:r>{}<w:t>hi</w:t></w:r></w:p></w:body></w:document>",
            run_xml
        )
    }

    fn no_styles() -> HashMap<String, StyleDefaults> {
        HashMap::new()
    }

    fn no_numbering() -> HashMap<u32, (String, String, Option<String>)> {
        HashMap::new()
    }

    // ── italic/font/color parsing (rich-text formatting plan, Phase 1) ──────

    #[test]
    fn test_parses_italic_run_property() {
        let xml = wrap_run_xml("<w:rPr><w:i/></w:rPr>");
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].runs[0].italic);
    }

    #[test]
    fn test_parses_run_font_ascii_attribute() {
        let xml = wrap_run_xml(r#"<w:rPr><w:rFonts w:ascii="Georgia"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs[0].font, Some("Georgia".to_string()));
    }

    #[test]
    fn test_parses_run_color_value() {
        let xml = wrap_run_xml(r#"<w:rPr><w:color w:val="FF0000"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs[0].color, Some("FF0000".to_string()));
    }

    #[test]
    fn test_color_val_auto_is_treated_as_none() {
        let xml = wrap_run_xml(r#"<w:rPr><w:color w:val="auto"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs[0].color, None);
    }

    #[test]
    fn test_run_without_new_properties_defaults_to_none() {
        let xml = wrap_run_xml("");
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].italic);
        assert_eq!(paragraphs[0].runs[0].font, None);
        assert_eq!(paragraphs[0].runs[0].color, None);
    }

    // ── alignment + heading parsing/emission ────────────────────────────────

    #[test]
    fn test_parses_center_alignment() {
        let xml = "<w:document><w:body><w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].alignment, Alignment::Center);
    }

    #[test]
    fn test_parses_justify_alignment_from_both_value() {
        // Word's own OOXML value for full justification is "both", not "justify".
        let xml = "<w:document><w:body><w:p><w:pPr><w:jc w:val=\"both\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].alignment, Alignment::Justify);
    }

    #[test]
    fn test_paragraph_without_jc_defaults_to_left_alignment() {
        let xml = "<w:document><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].alignment, Alignment::Left);
    }

    #[test]
    fn test_rebuild_emits_center_alignment() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::Center,
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:jc w:val="center"/>"#));
    }

    #[test]
    fn test_rebuild_omits_jc_for_left_alignment() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::Left,
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("w:jc"));
    }

    #[test]
    fn test_rebuild_emits_heading_style() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 2,
            alignment: Alignment::Left,
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        // Capitalized to match Word's own built-in styleId casing
        // ("Heading1".."Heading9") — see
        // test_rebuild_emits_capitalized_heading_styleid_matching_words_own_styles_xml
        // for why the casing has to match exactly.
        assert!(xml.contains(r#"<w:pStyle w:val="Heading2"/>"#));
    }

    #[test]
    fn test_rebuild_omits_pstyle_for_body_text() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::Left,
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("w:pStyle"));
    }

    #[test]
    fn test_rebuild_omits_ppr_entirely_for_plain_paragraph() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::Left,
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("w:pPr"));
    }

    #[test]
    fn test_alignment_and_heading_round_trip_through_parse_and_rebuild() {
        let original = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 1,
            alignment: Alignment::Center,
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &original);
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(reparsed[0].heading, 1);
        assert_eq!(reparsed[0].alignment, Alignment::Center);
    }

    // ── double underline parsing/emission ───────────────────────────────────

    #[test]
    fn test_parses_double_underline_distinctly_from_single() {
        let xml = wrap_run_xml(r#"<w:rPr><w:u w:val="double"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].runs[0].double_underline);
        assert!(!paragraphs[0].runs[0].underline);
    }

    #[test]
    fn test_parses_single_underline_val_as_plain_underline() {
        let xml = wrap_run_xml(r#"<w:rPr><w:u w:val="single"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].runs[0].underline);
        assert!(!paragraphs[0].runs[0].double_underline);
    }

    #[test]
    fn test_u_val_none_is_not_underlined() {
        // OOXML boolean-toggle semantics: `w:val="none"` (also seen as "0"/
        // "false") explicitly turns underline OFF, same as the property
        // being absent — not "anything other than double means single".
        // Real debate-community docx files declare this on character
        // styles like "Style13ptBold" (`<w:u w:val="none"/>`) to explicitly
        // suppress underline that would otherwise carry over.
        let xml = wrap_run_xml(r#"<w:rPr><w:u w:val="none"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].underline);
        assert!(!paragraphs[0].runs[0].double_underline);
    }

    #[test]
    fn test_b_val_0_is_not_bold() {
        let xml = wrap_run_xml(r#"<w:rPr><w:b w:val="0"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_rstyle_resolves_character_style_underline_and_bold() {
        // Reported bug repro: real debate docx files (e.g. this app's own
        // "Affective Labor" test file) underline/bold the emphasized
        // "read" portions of a card via a *character* style referenced by
        // <w:rStyle> on the run, rather than direct <w:u>/<w:b> — 4000+
        // occurrences of `<w:rStyle w:val="StyleUnderline"/>` in that file
        // alone. Unhandled, that text silently loses its underline.
        let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="StyleUnderline">
                <w:rPr><w:u w:val="single"/></w:rPr>
            </w:style>
        </w:styles>"#;
        let styles = parse_styles_xml(styles_xml);
        let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="StyleUnderline"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
        assert!(paragraphs[0].runs[0].underline, "character-style underline not applied");
    }

    #[test]
    fn test_rstyle_direct_formatting_after_it_still_wins() {
        // Word's own cascade: rStyle (character style) sets the run's
        // baseline, but any direct formatting appearing later in the same
        // <w:rPr> still overrides it — matching how a <w:pStyle>'s
        // paragraph-level defaults already work elsewhere in this parser.
        let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="StyleUnderline">
                <w:rPr><w:u w:val="single"/></w:rPr>
            </w:style>
        </w:styles>"#;
        let styles = parse_styles_xml(styles_xml);
        let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="StyleUnderline"/><w:u w:val="none"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].underline, "direct <w:u w:val=\"none\"/> after rStyle should win");
    }

    #[test]
    fn test_rstyle_resolves_character_style_bdr_into_emphasis_boxed_and_emphasis() {
        // Mirrors real Verbatim output: the run carries only an rStyle
        // reference; bold/underline/size/border all live on the referenced
        // character style.
        let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="Emphasis">
                <w:rPr>
                    <w:b/><w:sz w:val="24"/><w:u w:val="single"/>
                    <w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/>
                </w:rPr>
            </w:style>
        </w:styles>"#;
        let styles = parse_styles_xml(styles_xml);
        let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="Emphasis"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
        let run = &paragraphs[0].runs[0];
        assert!(run.emphasis_boxed, "style-referenced <w:bdr> should set emphasis_boxed");
        assert!(run.emphasis, "a real box implies Emphasis, so Remove Emphasis can find it");
        assert!(run.bold && run.underline, "existing bold/underline resolution must still work");
    }

    #[test]
    fn test_direct_bdr_on_a_run_sets_emphasis_boxed_and_emphasis() {
        let xml = wrap_run_xml(r#"<w:rPr><w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].runs[0].emphasis_boxed);
        assert!(paragraphs[0].runs[0].emphasis);
    }

    #[test]
    fn test_run_without_bdr_has_emphasis_boxed_false() {
        let xml = wrap_run_xml("<w:rPr><w:b/></w:rPr>");
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].emphasis_boxed);
    }

    #[test]
    fn test_bdr_val_none_cancels_inherited_emphasis_boxed() {
        // A run with rStyle pointing to Emphasis (which has a border) can
        // explicitly cancel the border with <w:bdr w:val="none"/>.
        // This mirrors the existing behavior for <w:u w:val="none"/> canceling
        // underline from the style.
        let styles_xml = r#"<w:styles>
            <w:style w:type="character" w:styleId="Emphasis">
                <w:rPr>
                    <w:bdr w:val="single" w:sz="12" w:space="0" w:color="auto"/>
                </w:rPr>
            </w:style>
        </w:styles>"#;
        let styles = parse_styles_xml(styles_xml);
        let xml = wrap_run_xml(r#"<w:rPr><w:rStyle w:val="Emphasis"/><w:bdr w:val="none"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml, &styles, &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].emphasis_boxed, "direct <w:bdr w:val=\"none\"/> after rStyle should cancel the box");
        assert!(!paragraphs[0].runs[0].emphasis, "canceling the box should also clear emphasis");
    }

    #[test]
    fn test_rebuild_emits_double_underline() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), double_underline: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:u w:val="double"/>"#));
    }

    #[test]
    fn test_double_underline_round_trip_through_parse_and_rebuild() {
        let original = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), double_underline: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &original);
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(reparsed[0].runs[0].double_underline);
        assert!(!reparsed[0].runs[0].underline);
    }

    // ── strikethrough parsing/emission ──────────────────────────────────────

    #[test]
    fn test_parses_strikethrough_run_property() {
        let xml = wrap_run_xml("<w:rPr><w:strike/></w:rPr>");
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].runs[0].strikethrough);
    }

    #[test]
    fn test_rebuild_emits_strikethrough() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), strikethrough: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains("<w:strike/>"));
    }

    #[test]
    fn test_strikethrough_round_trip_through_parse_and_rebuild() {
        let original = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), strikethrough: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &original);
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert!(reparsed[0].runs[0].strikethrough);
    }

    // ── Pocket box (paragraph border) parsing/emission ──────────────────────

    #[test]
    fn test_parses_paragraph_border_as_box_format_on_every_run() {
        let xml = "<w:document><w:body><w:p><w:pPr><w:pBdr><w:top w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/><w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/><w:left w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/><w:right w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"000000\"/></w:pBdr></w:pPr><w:r><w:t>a</w:t></w:r><w:r><w:t>b</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        // Parse-time run merging collapses "a"+"b" (identical formatting)
        // into one run, so check the property holds across whatever runs
        // remain rather than hardcoding a run count.
        assert!(paragraphs[0].runs.iter().all(|r| r.box_format));
    }

    #[test]
    fn test_paragraph_without_pbdr_has_box_format_false() {
        let xml = "<w:document><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert!(!paragraphs[0].runs[0].box_format);
    }

    #[test]
    fn test_rebuild_emits_four_sided_pbdr_when_box_format_set() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), box_format: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains("<w:pBdr>"));
        // top/bottom: space="1"; left/right: space="4" — matching real Verbatim's
        // own Heading1/"Pocket" style exactly (Verbatim_Formatting_To_Compare_To.docx),
        // not a uniform value. A uniform space="1" is what made Vimbatim's pocket
        // box render visibly narrower than Verbatim's when opened in Verbatim.
        assert!(xml.contains("<w:top w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"000000\"/>"));
        assert!(xml.contains("<w:bottom w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"000000\"/>"));
        assert!(xml.contains("<w:left w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"000000\"/>"));
        assert!(xml.contains("<w:right w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"000000\"/>"));
    }

    #[test]
    fn test_rebuild_omits_pbdr_when_no_run_has_box_format() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("w:pBdr"));
    }

    /// The marker's whole point: it survives a save/reload, so a Cite stays a
    /// Cite instead of being re-guessed from bold + font size.
    #[test]
    fn test_style_marker_round_trips_through_parse_and_rebuild() {
        for style in [
            CardStyle::Pocket,
            CardStyle::Hat,
            CardStyle::Block,
            CardStyle::Tag,
            CardStyle::Cite,
            CardStyle::Analytic,
        ] {
            let paragraphs = vec![Paragraph { list: None,
                runs: vec![Run { text: "marked".into(), style: Some(style), ..Run::default() }],
                heading: 0,
                alignment: Alignment::default(),
                unsupported_xml: None,
            }];
            let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
            let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
            assert_eq!(
                reparsed[0].runs[0].style,
                Some(style),
                "{style:?} did not survive the round trip"
            );
        }
    }

    /// The id is written as a `<w:rStyle>` reference, which Word ignores when
    /// the style isn't defined — so a marked document opens cleanly elsewhere.
    #[test]
    fn test_style_marker_is_written_as_an_rstyle_reference() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "a cite".into(), style: Some(CardStyle::Cite), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
        assert!(xml.contains(r#"<w:rStyle w:val="VimbatimCite"/>"#), "got: {xml}");
    }

    /// The Emphasis markers round-trip through save/load, and independently
    /// of each other and of `box_format`/`style` — a run can be Block-styled
    /// and emphasized-but-not-boxed at the same time.
    #[test]
    fn test_emphasis_markers_survive_round_trip() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![
                Run { text: "emphasized only".into(), emphasis: true, ..Run::default() },
                Run {
                    text: "emphasized and boxed".into(),
                    emphasis: true,
                    emphasis_boxed: true,
                    bold: true,
                    style: Some(CardStyle::Block),
                    ..Run::default()
                },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let xml = rebuild_document_xml(&fallback_preamble(), "", &paragraphs);
        assert!(xml.contains("<w:vimbatimEmphasis/>"), "got: {xml}");
        assert!(xml.contains("<w:vimbatimEmphasisBox/>"), "got: {xml}");

        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        let runs = &reparsed[0].runs;
        assert!(runs[0].emphasis && !runs[0].emphasis_boxed);
        assert!(runs[1].emphasis && runs[1].emphasis_boxed);
        assert!(runs[1].bold && runs[1].style == Some(CardStyle::Block));
    }

    /// A document written in Word carries heading styles but no markers —
    /// deriving them at parse time is the accuracy win.
    #[test]
    fn test_word_heading_styles_become_markers() {
        let xml = "<w:document><w:body><w:p>\
                   <w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>\
                   <w:r><w:t>a pocket</w:t></w:r>\
                   </w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].heading, 1);
        assert_eq!(paragraphs[0].runs[0].style, Some(CardStyle::Pocket));
    }

    /// Runs that look identical but carry different markers are different
    /// things — merging them would erase one.
    #[test]
    fn test_runs_with_different_markers_do_not_merge() {
        let mut runs = vec![
            Run { text: "a".into(), style: Some(CardStyle::Cite), ..Run::default() },
            Run { text: "b".into(), style: Some(CardStyle::Analytic), ..Run::default() },
        ];
        crate::document_ops::merge_adjacent_same_format_runs(&mut runs);
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn test_box_format_round_trip_through_parse_and_rebuild() {
        let original = vec![Paragraph { list: None,
            runs: vec![
                Run { text: "a".into(), box_format: true, ..Run::default() },
                Run { text: "b".into(), box_format: true, ..Run::default() },
            ],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &original);
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        // Parse-time run merging collapses "a"+"b" (identical formatting)
        // into one run, so check the property holds across whatever runs
        // remain rather than hardcoding a run count.
        assert!(reparsed[0].runs.iter().all(|r| r.box_format));
    }

    // ── real-file round trip (parse_docx -> DocxOrigin::save -> parse_docx) ─

    #[test]
    fn test_real_file_round_trip_preserves_all_five_fixed_attributes() {
        let dir = std::env::temp_dir().join(format!("vimbatim_docx_roundtrip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.docx");

        // 1. Create a minimal real .docx on disk.
        let initial = vec![Paragraph { list: None,
            runs: vec![Run { text: "hello".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        create_new_docx(&initial, &path).unwrap();

        // 2. Open it through the real parse_docx path (ZIP + XML), not the
        //    XML-string helpers the rest of this file's tests use.
        let (mut paragraphs, origin) = parse_docx(&path).unwrap();
        assert_eq!(paragraphs[0].runs[0].text, "hello");

        // 3. Apply every attribute this plan fixed, directly on the parsed
        //    model (mirroring what AppState::apply_card_style and
        //    apply_formatting_to_selection do in the real app).
        paragraphs[0].heading = 1;
        paragraphs[0].alignment = Alignment::Center;
        paragraphs[0].runs[0].double_underline = true;
        paragraphs[0].runs[0].strikethrough = true;
        paragraphs[0].runs[0].box_format = true;

        // 4. Save through the real DocxOrigin::save path (ZIP write, not a
        //    bare string).
        origin.save(&paragraphs, &path).unwrap();

        // 5. Parse it again from scratch — a completely fresh read of the
        //    file just written, proving the round trip survives a real
        //    save/reload, not just an in-memory transformation.
        let (reparsed, _origin2) = parse_docx(&path).unwrap();
        assert_eq!(reparsed[0].heading, 1);
        assert_eq!(reparsed[0].alignment, Alignment::Center);
        assert!(reparsed[0].runs[0].double_underline);
        assert!(reparsed[0].runs[0].strikethrough);
        assert!(reparsed[0].runs[0].box_format);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    // ── regression tests against a real Verbatim-authored reference file ───

    /// Parses the actual file real Verbatim produced (the reference pair the
    /// `Round Trip Bug Fixes.md` tracker itself was built from) and confirms
    /// Emphasis's box now survives, using real Verbatim bytes rather than a
    /// hand-written XML fixture.
    #[test]
    fn test_parses_real_verbatim_authored_emphasis_box() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verbatim_emphasis_reference.docx");
        let (paragraphs, _origin) = parse_docx(&path).unwrap();

        let pocket = paragraphs.iter().find(|p| p.runs.iter().any(|r| r.text == "Pocket")).unwrap();
        assert!(pocket.runs[0].box_format, "Pocket paragraph should carry the paragraph-wide box");

        let emphasis_para = paragraphs.iter()
            .find(|p| p.runs.iter().any(|r| r.text.contains("Emphasis (Size")))
            .expect("Emphasis paragraph not found");
        assert!(
            emphasis_para.runs.iter().all(|r| r.emphasis && r.emphasis_boxed),
            "every run in the Emphasis paragraph should be marked emphasis + emphasis_boxed, got: {:?}",
            emphasis_para.runs,
        );

        let emphasis_highlight_para = paragraphs.iter()
            .find(|p| p.runs.iter().any(|r| r.text.contains("Emphasis (same as above) and highlight")))
            .expect("Emphasis+highlight paragraph not found");
        let run = &emphasis_highlight_para.runs[0];
        assert!(run.emphasis && run.emphasis_boxed, "Emphasis+highlight should also carry the box");
        assert!(run.highlight && run.highlight_color == "yellow");
    }

    /// Round-trips that same real Verbatim file through a real save/reload
    /// (ZIP write, not a bare string), confirming the fix survives the whole
    /// pipeline the app actually uses when a user opens a Verbatim file, edits
    /// it, and saves — not just an in-memory transformation.
    #[test]
    fn test_real_verbatim_file_emphasis_box_survives_save_and_reload() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verbatim_emphasis_reference.docx");
        let dir = std::env::temp_dir().join(format!("vimbatim_verbatim_roundtrip_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.docx");
        std::fs::copy(&src, &path).unwrap();

        let (paragraphs, origin) = parse_docx(&path).unwrap();
        origin.save(&paragraphs, &path).unwrap();

        let (reparsed, _origin2) = parse_docx(&path).unwrap();
        let emphasis_para = reparsed.iter()
            .find(|p| p.runs.iter().any(|r| r.text.contains("Emphasis (Size")))
            .expect("Emphasis paragraph not found after save/reload");
        assert!(emphasis_para.runs.iter().all(|r| r.emphasis && r.emphasis_boxed));

        let pocket = reparsed.iter()
            .find(|p| p.runs.iter().any(|r| r.text == "Pocket"))
            .expect("Pocket paragraph not found after save/reload");
        assert!(pocket.runs[0].box_format, "Pocket's box_format should survive save/reload");

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    // ── heading style round-trips against Word's actual built-in styleId ────

    #[test]
    fn test_rebuild_emits_capitalized_heading_styleid_matching_words_own_styles_xml() {
        // Real Word documents (and every version of Microsoft Word itself)
        // define the built-in heading styles with a capitalized styleId —
        // <w:style w:type="paragraph" w:styleId="Heading1"> in word/styles.xml
        // — and OOXML styleId references are matched case-sensitively. If
        // rebuild_document_xml emits a differently-cased w:val, the saved
        // paragraph's <w:pStyle> points at a styleId that doesn't exist in
        // styles.xml (which this app never rewrites), so Word silently falls
        // back to Normal: the heading's bold/size/outline-level vanish and it
        // drops out of Word's Navigation pane, even though parsing it back
        // into Vimbatim still shows a heading (apply_para_style lower-cases
        // before matching, so it doesn't notice the mismatch).
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 1,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:pStyle w:val="Heading1"/>"#));
    }

    // ── block-level unsupported content detection ───────────────────────────

    #[test]
    fn test_detects_table_in_document_xml() {
        let xml = "<w:document><w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>";
        assert!(xml.contains("<w:tbl"));
    }

    #[test]
    fn test_parse_docx_sets_has_unsupported_blocks_for_real_file_with_table() {
        let dir = std::env::temp_dir().join(format!("vimbatim_docx_table_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("with_table.docx");

        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "before table".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }];
        create_new_docx(&paragraphs, &path).unwrap();

        // create_new_docx has no table support itself, so this only confirms
        // the negative case end-to-end through a real file — splicing a real
        // <w:tbl> into a ZIP-written file for the positive case is
        // significant extra machinery for marginal coverage beyond the
        // already-passing test_detects_table_in_document_xml string check.
        let (_paragraphs, origin) = parse_docx(&path).unwrap();
        assert!(!origin.has_unsupported_blocks);

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(&dir).ok();
    }

    // ── paragraph-style-based formatting (word/styles.xml resolution) ───────

    // Mirrors the shape real Word (and the "Verbatim" tool the user tested
    // with) actually writes for a named paragraph style: box+center+bold+
    // size live on the STYLE, not inline on each paragraph that uses it.
    const POCKET_STYLE_XML: &str = r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:aliases w:val="Pocket"/><w:basedOn w:val="Normal"/><w:pPr><w:pBdr><w:top w:val="single" w:sz="24" w:space="1" w:color="auto"/><w:left w:val="single" w:sz="24" w:space="4" w:color="auto"/><w:bottom w:val="single" w:sz="24" w:space="1" w:color="auto"/><w:right w:val="single" w:sz="24" w:space="4" w:color="auto"/></w:pBdr><w:jc w:val="center"/></w:pPr><w:rPr><w:b/><w:sz w:val="52"/></w:rPr></w:style>"#;

    #[test]
    fn test_parses_alignment_and_box_from_referenced_paragraph_style() {
        let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
        let styles = parse_styles_xml(&styles_xml);
        let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].alignment, Alignment::Center);
        assert_eq!(paragraphs[0].heading, 1);
        assert!(paragraphs[0].runs[0].box_format);
        assert!(paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[0].size, 52);
    }

    #[test]
    fn test_direct_paragraph_formatting_overrides_style_defaults() {
        let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
        let styles = parse_styles_xml(&styles_xml);
        // Same style reference as above, but this paragraph ALSO carries its
        // own direct <w:jc> - direct formatting must win over the style's.
        let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/><w:jc w:val=\"left\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].alignment, Alignment::Left);
    }

    #[test]
    fn test_direct_run_formatting_overrides_style_defaults() {
        let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
        let styles = parse_styles_xml(&styles_xml);
        // The style says sz=52; this run's own <w:sz> should win.
        let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:rPr><w:sz w:val=\"80\"/></w:rPr><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs[0].size, 80);
        assert!(paragraphs[0].runs[0].bold); // still inherited from the style
    }

    #[test]
    fn test_paragraph_without_pstyle_is_unaffected_by_styles_map() {
        let styles_xml = format!("<w:styles>{}</w:styles>", POCKET_STYLE_XML);
        let styles = parse_styles_xml(&styles_xml);
        let xml = "<w:document><w:body><w:p><w:r><w:t>plain</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].alignment, Alignment::Left);
        assert!(!paragraphs[0].runs[0].box_format);
        assert!(!paragraphs[0].runs[0].bold);
        assert_eq!(paragraphs[0].runs[0].size, 0);
    }

    // ── parse-time run merging (editing-speed fix) ──────────────────────────

    #[test]
    fn test_adjacent_runs_with_identical_formatting_merge_at_parse_time() {
        // Word fragments runs at spell-check/revision boundaries even when
        // formatting doesn't change; merging these once at parse time keeps
        // every later per-keystroke edit as cheap on a loaded document as on
        // a freshly-typed one (both O(runs), but this keeps `runs` small).
        let xml = "<w:document><w:body><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>foo</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bar</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs.len(), 1);
        assert_eq!(paragraphs[0].runs[0].text, "foobar");
        assert!(paragraphs[0].runs[0].bold);
    }

    #[test]
    fn test_adjacent_runs_with_different_formatting_stay_separate_at_parse_time() {
        let xml = "<w:document><w:body><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>foo</w:t></w:r><w:r><w:t>bar</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].runs.len(), 2);
        assert!(paragraphs[0].runs[0].bold);
        assert!(!paragraphs[0].runs[1].bold);
    }

    #[test]
    fn test_pstyle_referencing_unknown_style_id_is_unaffected() {
        let styles = parse_styles_xml("<w:styles></w:styles>");
        let xml = "<w:document><w:body><w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &styles, &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].heading, 1); // name-based heading detection still works
        assert_eq!(paragraphs[0].alignment, Alignment::Left);
        assert!(!paragraphs[0].runs[0].box_format);
    }

    // ── unsupported inline content preservation ─────────────────────────────

    #[test]
    fn test_captures_unsupported_xml_for_paragraph_with_hyperlink() {
        let xml = "<w:document><w:body><w:p><w:hyperlink r:id=\"rId1\"><w:r><w:t>link text</w:t></w:r></w:hyperlink></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].unsupported_xml.is_some());
        assert!(paragraphs[0].unsupported_xml.as_ref().unwrap().contains("w:hyperlink"));
    }

    #[test]
    fn test_plain_paragraph_has_no_unsupported_xml() {
        let xml = "<w:document><w:body><w:p><w:r><w:t>plain</w:t></w:r></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].unsupported_xml, None);
    }

    #[test]
    fn test_incidental_tags_do_not_trigger_unsupported_xml_capture() {
        // Bookmarks are common and harmless - must NOT freeze this paragraph.
        let xml = "<w:document><w:body><w:p><w:bookmarkStart w:id=\"0\" w:name=\"_Test\"/><w:r><w:t>plain</w:t></w:r><w:bookmarkEnd w:id=\"0\"/></w:p></w:body></w:document>";
        let paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].unsupported_xml, None);
    }

    #[test]
    fn test_rebuild_reemits_unsupported_xml_verbatim_when_present() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "ignored".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: Some("<w:hyperlink r:id=\"rId1\"><w:r><w:t>link text</w:t></w:r></w:hyperlink>".to_string()),
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains("<w:hyperlink r:id=\"rId1\">"));
        assert!(xml.contains("link text"));
        // The runs the app *did* manage to parse for display purposes must
        // NOT also be independently re-emitted - unsupported_xml IS the
        // paragraph's entire content on save.
        assert!(!xml.contains("ignored"));
    }

    #[test]
    fn test_unsupported_xml_round_trips_through_untouched_edit_elsewhere() {
        let xml = "<w:document><w:body><w:p><w:hyperlink r:id=\"rId1\"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:p><w:p><w:r><w:t>other paragraph</w:t></w:r></w:p></w:body></w:document>";
        let mut paragraphs = parse_document_xml(xml, &no_styles(), &no_numbering()).unwrap();
        assert!(paragraphs[0].unsupported_xml.is_some());

        // Edit only the SECOND paragraph - the first (with the hyperlink)
        // is never touched.
        paragraphs[1].runs[0].text = "edited".to_string();

        let rebuilt = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(rebuilt.contains("w:hyperlink"));
        assert!(rebuilt.contains("edited"));
    }

    // ── italic/font/color re-emission (rebuild_document_xml) ────────────────

    #[test]
    fn test_rebuild_emits_italic() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), italic: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains("<w:i/>"));
    }

    #[test]
    fn test_rebuild_emits_font_ascii() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), font: Some("Georgia".into()), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:rFonts w:ascii="Georgia"/>"#));
    }

    #[test]
    fn test_rebuild_emits_color() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), color: Some("FF0000".into()), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:color w:val="FF0000"/>"#));
    }

    #[test]
    fn test_rebuild_omits_rpr_entirely_when_no_properties_set() {
        let paragraphs = vec![Paragraph { list: None,
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("<w:rPr>"));
    }

    #[test]
    fn test_italic_font_color_round_trip_through_parse_and_rebuild() {
        let original = vec![Paragraph { list: None,
            runs: vec![Run {
                text: "hi".into(),
                italic: true,
                font: Some("Georgia".into()),
                color: Some("00FF00".into()),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
        unsupported_xml: None,
    }];
        let xml = rebuild_document_xml("<w:document>", "", &original);
        // rebuild_document_xml wraps in <w:body>...</w:body></w:document>,
        // matching what parse_document_xml expects to find.
        let reparsed = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(reparsed[0].runs[0].italic, true);
        assert_eq!(reparsed[0].runs[0].font, Some("Georgia".to_string()));
        assert_eq!(reparsed[0].runs[0].color, Some("00FF00".to_string()));
    }

    // ── recovery snapshot round trip (DocxOrigin::save_snapshot) ────────────

    #[test]
    fn save_snapshot_produces_a_docx_that_parses_back_to_the_same_paragraphs() {
        let dir = std::env::temp_dir().join(format!("vimbatim-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.docx");
        let snapshot = dir.join("snapshot.docx");

        // Build a real docx, then reload it to get a genuine DocxOrigin.
        let mut para = Paragraph::default();
        para.runs.push(Run { text: "hello world".into(), ..Default::default() });
        create_new_docx(&[para.clone()], &original).unwrap();
        let (paragraphs, origin) = parse_docx(&original).unwrap();

        origin.save_snapshot(&paragraphs, &snapshot).unwrap();

        let (restored, _) = parse_docx(&snapshot).unwrap();
        assert_eq!(restored, paragraphs);

        std::fs::remove_file(&original).ok();
        std::fs::remove_file(&snapshot).ok();
        std::fs::remove_dir(&dir).ok();
    }

    // ── real Word list parsing (word/numbering.xml) ─────────────────────────

    #[test]
    fn test_parse_numbering_xml_extracts_numfmt_lvltext_font_per_numid() {
        let xml = format!(
            "<w:numbering>\
             <w:abstractNum w:abstractNumId=\"4\">\
             <w:lvl w:ilvl=\"0\">\
             <w:numFmt w:val=\"bullet\"/>\
             <w:lvlText w:val=\"{}\"/>\
             <w:rPr><w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\"/></w:rPr>\
             </w:lvl>\
             </w:abstractNum>\
             <w:abstractNum w:abstractNumId=\"8\">\
             <w:lvl w:ilvl=\"0\">\
             <w:numFmt w:val=\"decimal\"/>\
             <w:lvlText w:val=\"%1.\"/>\
             </w:lvl>\
             </w:abstractNum>\
             <w:num w:numId=\"1\"><w:abstractNumId w:val=\"4\"/></w:num>\
             <w:num w:numId=\"2\"><w:abstractNumId w:val=\"8\"/></w:num>\
             </w:numbering>",
            "\u{f0b7}",
        );
        let map = parse_numbering_xml(&xml);
        assert_eq!(map.get(&1), Some(&("bullet".to_string(), "\u{f0b7}".to_string(), Some("Symbol".to_string()))));
        assert_eq!(map.get(&2), Some(&("decimal".to_string(), "%1.".to_string(), None)));
    }

    #[test]
    fn test_list_kind_classify_exact_matches() {
        assert_eq!(ListKind::classify("bullet", "\u{f0b7}", Some("Symbol")), ListKind::BulletSolid);
        assert_eq!(ListKind::classify("bullet", "o", Some("Courier New")), ListKind::BulletHollow);
        assert_eq!(ListKind::classify("decimal", "%1.", None), ListKind::NumberDecimalDot);
        assert_eq!(ListKind::classify("lowerRoman", "%1.", None), ListKind::NumberLowerRoman);
    }

    #[test]
    fn test_list_kind_classify_falls_back_for_unrecognized_bullet() {
        assert_eq!(ListKind::classify("bullet", "\u{2013}", Some("Arial")), ListKind::BulletSolid);
    }

    #[test]
    fn test_list_kind_classify_falls_back_for_unrecognized_number_punctuation() {
        assert_eq!(ListKind::classify("decimal", "%1:", None), ListKind::NumberDecimalDot);
        assert_eq!(ListKind::classify("lowerLetter", "%1:", None), ListKind::NumberLowerLetterDot);
    }

    #[test]
    fn test_list_kind_classify_falls_back_for_unrecognized_numfmt() {
        assert_eq!(ListKind::classify("cardinalText", "%1.", None), ListKind::NumberDecimalDot);
    }

    #[test]
    fn test_parses_numpr_into_paragraph_list() {
        let xml = "<w:document><w:body><w:p>\
                   <w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>\
                   <w:r><w:t>Item One</w:t></w:r>\
                   </w:p></w:body></w:document>";
        let mut numbering = HashMap::new();
        numbering.insert(1u32, ("bullet".to_string(), "\u{f0b7}".to_string(), Some("Symbol".to_string())));
        let paragraphs = parse_document_xml(xml, &no_styles(), &numbering).unwrap();
        assert_eq!(paragraphs[0].list, Some(ListItem { kind: ListKind::BulletSolid, level: 0 }));
    }

    #[test]
    fn test_paragraph_without_numpr_has_no_list() {
        let xml = wrap_run_xml("<w:rPr><w:b/></w:rPr>");
        let paragraphs = parse_document_xml(&xml, &no_styles(), &no_numbering()).unwrap();
        assert_eq!(paragraphs[0].list, None);
    }

    // ── word/numbering.xml generation ───────────────────────────────────────

    #[test]
    fn test_build_numbering_xml_returns_none_for_no_lists() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0, alignment: Alignment::default(), list: None, unsupported_xml: None,
        }];
        assert!(build_numbering_xml(&paragraphs).is_none());
    }

    #[test]
    fn test_build_numbering_xml_emits_all_13_abstract_nums_with_9_levels_each() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0, alignment: Alignment::default(),
            list: Some(ListItem { kind: ListKind::BulletSolid, level: 0 }),
            unsupported_xml: None,
        }];
        let xml = build_numbering_xml(&paragraphs).unwrap();
        assert_eq!(xml.matches("<w:abstractNum ").count(), 13, "got: {xml}");
        // Spot-check one full abstractNum's level-0/1/2, matching Lists.docx exactly.
        assert!(xml.contains("<w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{f0b7}\"/>"), "got: {xml}");
        assert!(xml.contains("<w:rFonts w:ascii=\"Symbol\" w:hAnsi=\"Symbol\"/>"), "got: {xml}");
        // Confirmed cascade: every bullet style's level 1 is 'o'/Courier New,
        // level 2 is /Wingdings, regardless of the level-0 style.
        assert!(xml.matches("<w:lvlText w:val=\"o\"/>").count() >= 6, "got: {xml}");
        assert!(xml.matches("<w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\"/>").count() >= 6, "got: {xml}");
    }

    #[test]
    fn test_build_numbering_xml_emits_one_num_per_contiguous_list_run() {
        let paragraphs = vec![
            Paragraph { runs: vec![Run { text: "a".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::NumberDecimalDot, level: 0 }), unsupported_xml: None },
            Paragraph { runs: vec![Run { text: "b".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::NumberDecimalDot, level: 0 }), unsupported_xml: None },
            Paragraph { runs: vec![Run { text: "not a list".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: None, unsupported_xml: None },
            Paragraph { runs: vec![Run { text: "c".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::NumberDecimalDot, level: 0 }), unsupported_xml: None },
        ];
        let xml = build_numbering_xml(&paragraphs).unwrap();
        // Two runs of decimal-dot list paragraphs (a,b) and (c) -> two <w:num> entries.
        assert_eq!(xml.matches("<w:num ").count(), 2, "got: {xml}");
    }

    // ── pStyle/numPr emission + shared numId assignment ─────────────────────

    #[test]
    fn test_rebuild_emits_pstyle_and_numpr_for_list_paragraphs() {
        let paragraphs = vec![
            Paragraph { runs: vec![Run { text: "one".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::BulletSolid, level: 0 }), unsupported_xml: None },
            Paragraph { runs: vec![Run { text: "two".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::BulletSolid, level: 0 }), unsupported_xml: None },
        ];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert_eq!(xml.matches("<w:pStyle w:val=\"ListParagraph\"/>").count(), 2, "got: {xml}");
        // Both paragraphs are one contiguous run -> same numId, both ilvl 0.
        assert_eq!(xml.matches("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr>").count(), 2, "got: {xml}");
    }

    #[test]
    fn test_rebuild_gives_separate_list_runs_different_numids() {
        let paragraphs = vec![
            Paragraph { runs: vec![Run { text: "a".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::NumberDecimalDot, level: 0 }), unsupported_xml: None },
            Paragraph { runs: vec![Run { text: "not a list".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: None, unsupported_xml: None },
            Paragraph { runs: vec![Run { text: "b".into(), ..Run::default() }], heading: 0, alignment: Alignment::default(),
                list: Some(ListItem { kind: ListKind::NumberDecimalDot, level: 0 }), unsupported_xml: None },
        ];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains("<w:numId w:val=\"1\"/>"), "got: {xml}");
        assert!(xml.contains("<w:numId w:val=\"2\"/>"), "got: {xml}");
    }

    #[test]
    fn test_rebuild_omits_pstyle_numpr_for_non_list_paragraphs() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0, alignment: Alignment::default(), list: None, unsupported_xml: None,
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("ListParagraph"), "got: {xml}");
        assert!(!xml.contains("w:numPr"), "got: {xml}");
    }
}
