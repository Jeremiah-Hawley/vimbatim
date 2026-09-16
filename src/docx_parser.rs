use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use zip::write::SimpleFileOptions;
use zip::ZipArchive;
use zip::ZipWriter;

pub use crate::document::{
    Alignment, CardStyle, DocDefaults, ListItem, ListKind, NewDocStyle, Paragraph, Run,
};

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
    /// This document's own `<w:docDefaults>`, so a file written in Word or
    /// Verbatim renders with the body font and size *it* declares rather than
    /// this app's settings. Empty for a document that declares none.
    pub doc_defaults: DocDefaults,
}

impl DocxOrigin {
    /// Saves `paragraphs` back to `path` as a .docx file, using this
    /// origin's preserved preamble/sectPr/raw ZIP as the template.
    pub fn save(
        &self,
        paragraphs: &[Paragraph],
        path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.save_with_compression(paragraphs, path, zip::CompressionMethod::Deflated)
    }

    /// `save` for a crash-recovery snapshot. Identical today; it exists as a
    /// separate entry point so the snapshot's compression can be changed
    /// (to `Stored`, skipping deflate) without touching any real-save path.
    /// See the recovery spec's Performance section — do not change this to
    /// `Stored` until Task 10's measurement justifies it.
    pub fn save_snapshot(
        &self,
        paragraphs: &[Paragraph],
        path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
        let numbering_xml = build_numbering_xml(paragraphs);
        write_docx(
            &self.raw_zip,
            &new_xml,
            numbering_xml.as_deref(),
            path,
            method,
        )
    }
}

/// Returns all paragraph text joined by newlines. This is the plain-text
/// content loaded into `tab.document.content()` so the text editor can display it.
#[cfg(test)]
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
    let (styles, doc_defaults) = match archive.by_name("word/styles.xml") {
        Ok(mut file) => {
            let mut xml = String::new();
            file.read_to_string(&mut xml)?;
            (parse_styles_xml(&xml), parse_doc_defaults(&xml))
        }
        Err(_) => (HashMap::new(), DocDefaults::default()),
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

    Ok((
        paragraphs,
        DocxOrigin {
            raw_zip,
            preamble,
            sect_pr,
            has_unsupported_blocks,
            doc_defaults,
        },
    ))
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
///
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
    numbering_xml: Option<&str>,
    path: &Path,
    document_compression: zip::CompressionMethod,
) -> Result<(), Box<dyn std::error::Error>> {
    /*
     * Open the original ZIP from the in-memory byte slice, then stream each
     * entry into a new ZIP writer:
     *  - For word/document.xml: write the freshly generated XML using
     *    `document_compression` (the caller decides Deflated vs. Stored).
     *  - For word/numbering.xml: write the freshly generated numbering (or
     *    omit it) rather than copying the source's own — same "regenerate,
     *    don't preserve" policy as word/document.xml, safe because a real
     *    Verbatim-authored file's own numbering.xml is confirmed inert
     *    boilerplate when unused (see the design doc's "Ground truth"
     *    section).
     *  - For [Content_Types].xml / word/_rels/document.xml.rels: patch in
     *    the numbering override/relationship if the current document needs
     *    one and the source doesn't already declare it.
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

    let mut wrote_numbering = false;
    for i in 0..archive.len() {
        let file = archive.by_index_raw(i)?;
        let name = file.name().to_string();
        match name.as_str() {
            "word/document.xml" => {
                drop(file);
                let options = SimpleFileOptions::default().compression_method(document_compression);
                writer.start_file(&name, options)?;
                writer.write_all(new_xml.as_bytes())?;
            }
            "word/numbering.xml" => {
                drop(file);
                if let Some(xml) = numbering_xml {
                    writer.start_file(&name, SimpleFileOptions::default())?;
                    writer.write_all(xml.as_bytes())?;
                    wrote_numbering = true;
                }
                // Source had one but the current document has no list at
                // all: omit it (dropped, not copied) — matches
                // `create_new_docx`'s "no list -> no numbering.xml" policy.
            }
            "[Content_Types].xml" if numbering_xml.is_some() => {
                // `by_index_raw`'s `file` handle (bound above) yields the
                // still-compressed bytes, meant for `raw_copy_file`, not
                // text reading — re-open this same entry through the
                // decompressing `by_index` to actually read its content.
                drop(file);
                let mut content = String::new();
                std::io::Read::read_to_string(&mut archive.by_index(i)?, &mut content)?;
                if !content.contains("wordprocessingml.numbering+xml") {
                    content = content.replace(
                        "</Types>",
                        "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/></Types>",
                    );
                }
                writer.start_file(&name, SimpleFileOptions::default())?;
                writer.write_all(content.as_bytes())?;
            }
            "word/_rels/document.xml.rels" if numbering_xml.is_some() => {
                drop(file);
                let mut content = String::new();
                std::io::Read::read_to_string(&mut archive.by_index(i)?, &mut content)?;
                if !content.contains("relationships/numbering") {
                    content = content.replace(
                        "</Relationships>",
                        "<Relationship Id=\"rIdVimbatimNumbering\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/></Relationships>",
                    );
                }
                writer.start_file(&name, SimpleFileOptions::default())?;
                writer.write_all(content.as_bytes())?;
            }
            _ => {
                writer.raw_copy_file(file)?;
            }
        }
    }

    // Source never had word/numbering.xml at all, but the current document
    // needs one (a brand-new list added to a file that never had any).
    if let Some(xml) = numbering_xml {
        if !wrote_numbering {
            writer.start_file("word/numbering.xml", SimpleFileOptions::default())?;
            writer.write_all(xml.as_bytes())?;
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
/// `<w:docDefaults><w:rPrDefault><w:rPr>`'s font and size, if the document
/// declares any. A separate streaming pass rather than a branch inside
/// `parse_styles_xml`: that function is a per-`<w:style>` state machine, and
/// `docDefaults` sits outside every `<w:style>`, so folding it in would mean
/// threading an "am I inside docDefaults" flag through all of it to answer one
/// question asked once per file.
pub fn parse_doc_defaults(xml: &str) -> DocDefaults {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut out = DocDefaults::default();
    let mut in_defaults = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() == b"w:rPrDefault" => in_defaults = true,
            Ok(Event::End(ref e)) if e.name().as_ref() == b"w:rPrDefault" => break,
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) if in_defaults => {
                match e.name().as_ref() {
                    b"w:rFonts" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"w:ascii" {
                                out.font = Some(attr_value(&attr));
                            }
                        }
                    }
                    b"w:sz" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"w:val" {
                                out.size = attr_value(&attr).parse().unwrap_or(0);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_styles_xml(xml: &str) -> HashMap<String, StyleDefaults> {
    let mut styles = HashMap::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut current_id: Option<String> = None;
    let mut current: StyleDefaults = StyleDefaults::default();
    let mut scratch_run = Run::default();
    let mut in_ppr = false;
    let mut in_rpr = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"w:style" => {
                    current_id = e.attributes().flatten().find_map(|attr| {
                        (attr.key.as_ref() == b"w:styleId")
                            .then(|| String::from_utf8_lossy(&attr.value).into_owned())
                    });
                    current = StyleDefaults::default();
                    scratch_run = Run::default();
                }
                b"w:pPr" => {
                    in_ppr = true;
                }
                b"w:rPr" => {
                    in_rpr = true;
                }
                b"w:jc" if in_ppr => {
                    let mut para = Paragraph {
                        list: None,
                        runs: Vec::new(),
                        heading: 0,
                        alignment: Alignment::default(),
                        unsupported_xml: None,
                    };
                    apply_para_alignment(e, &mut para);
                    current.alignment = Some(para.alignment);
                }
                b"w:pBdr" if in_ppr => {
                    current.box_format = true;
                }
                _ if in_rpr => {
                    apply_run_prop(e, &mut scratch_run);
                }
                _ => {}
            },
            Ok(Event::End(ref e)) => match e.name().as_ref() {
                b"w:pPr" => {
                    in_ppr = false;
                }
                b"w:rPr" => {
                    in_rpr = false;
                }
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
            },
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
    reader.config_mut().trim_text(false);

    let mut current_abstract_id: Option<u32> = None;
    let mut current_num_id: Option<u32> = None;
    let mut in_lvl0 = false;
    let mut current_numfmt = String::new();
    let mut current_lvltext = String::new();
    let mut current_font: Option<String> = None;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => match e.name().as_ref() {
                b"w:abstractNum" => {
                    current_abstract_id = e.attributes().flatten().find_map(|attr| {
                        (attr.key.as_ref() == b"w:abstractNumId")
                            .then(|| std::str::from_utf8(&attr.value).ok()?.parse().ok())
                            .flatten()
                    });
                }
                b"w:lvl" => {
                    in_lvl0 = e
                        .attributes()
                        .flatten()
                        .any(|attr| attr.key.as_ref() == b"w:ilvl" && attr.value.as_ref() == b"0");
                    current_numfmt.clear();
                    current_lvltext.clear();
                    current_font = None;
                }
                b"w:numFmt" if in_lvl0 => {
                    if let Some(v) = e.attributes().flatten().find_map(|a| {
                        (a.key.as_ref() == b"w:val")
                            .then(|| String::from_utf8_lossy(&a.value).into_owned())
                    }) {
                        current_numfmt = v;
                    }
                }
                b"w:lvlText" if in_lvl0 => {
                    if let Some(v) = e.attributes().flatten().find_map(|a| {
                        (a.key.as_ref() == b"w:val")
                            .then(|| String::from_utf8_lossy(&a.value).into_owned())
                    }) {
                        current_lvltext = v;
                    }
                }
                b"w:rFonts" if in_lvl0 => {
                    current_font = e.attributes().flatten().find_map(|a| {
                        (a.key.as_ref() == b"w:ascii")
                            .then(|| String::from_utf8_lossy(&a.value).into_owned())
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
            },
            Ok(Event::End(ref e)) => match e.name().as_ref() {
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
                b"w:num" => {
                    current_num_id = None;
                }
                _ => {}
            },
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
    reader.config_mut().trim_text(false);

    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut current_para: Option<Paragraph> = None;
    let mut current_run: Option<Run> = None;

    let mut in_ppr = false; // inside <w:pPr>
    let mut in_rpr = false; // inside <w:rPr>
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
                        current_para = Some(Paragraph {
                            list: None,
                            runs: Vec::new(),
                            heading: 0,
                            alignment: Alignment::default(),
                            unsupported_xml: None,
                        });
                        para_has_box_border = false;
                        current_style_defaults = None;
                        para_has_unsupported_content = false;
                        current_para_num_id = None;
                        current_para_ilvl = 0;
                        para_start_pos = reader.buffer_position() as usize;
                    }
                    b"w:pPr" => {
                        in_ppr = true;
                    }
                    b"w:numPr" if in_ppr => {
                        in_numpr = true;
                    }
                    b"w:pStyle" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_style(e, para);
                            current_style_defaults = apply_paragraph_style_defaults(
                                e,
                                para,
                                styles,
                                &mut para_has_box_border,
                            );
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
                        let mut run = Run {
                            box_format: para_has_box_border,
                            ..Run::default()
                        };
                        if let Some(defaults) = &current_style_defaults {
                            run.bold = defaults.bold;
                            run.size = defaults.size;
                            run.underline = defaults.underline;
                            run.double_underline = defaults.double_underline;
                        }
                        current_run = Some(run);
                    }
                    b"w:rPr" => {
                        in_rpr = true;
                    }
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
                    // `<w:p/>` — how Word writes a blank line that carries no
                    // paragraph properties and no runs. There is no `End` event
                    // to close it, so without this arm the paragraph was never
                    // pushed at all and every blank line in a Word- or
                    // Verbatim-authored document silently vanished on open —
                    // and, since saving rewrites `document.xml` from this list,
                    // vanished from their file too on the next save.
                    b"w:p" => {
                        paragraphs.push(Paragraph {
                            list: None,
                            runs: vec![Run::default()],
                            heading: 0,
                            alignment: Alignment::default(),
                            unsupported_xml: None,
                        });
                    }
                    b"w:pStyle" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_style(e, para);
                            current_style_defaults = apply_paragraph_style_defaults(
                                e,
                                para,
                                styles,
                                &mut para_has_box_border,
                            );
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
                        if let Some(v) = e
                            .attributes()
                            .flatten()
                            .find_map(|a| {
                                (a.key.as_ref() == b"w:val")
                                    .then(|| std::str::from_utf8(&a.value).ok()?.parse().ok())
                            })
                            .flatten()
                        {
                            current_para_ilvl = v;
                        }
                    }
                    b"w:numId" if in_numpr => {
                        current_para_num_id = e
                            .attributes()
                            .flatten()
                            .find_map(|a| {
                                (a.key.as_ref() == b"w:val")
                                    .then(|| std::str::from_utf8(&a.value).ok()?.parse().ok())
                            })
                            .flatten();
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
                        run.text
                            .push_str(&quick_xml::escape::unescape(&e.decode()?)?);
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
                                let para_end_pos = reader.buffer_position() as usize - 6;
                                para.unsupported_xml =
                                    Some(xml[para_start_pos..para_end_pos].to_string());
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
                                    // Every run in the line, whitespace
                                    // included — `apply_card_style` marks the
                                    // whole line via `apply_formatting_to_line`,
                                    // so this has to reproduce that exactly.
                                    // Skipping blank runs left a whitespace run
                                    // between two marked ones unmarked, which
                                    // `format_key` counts as different
                                    // formatting: the merge below then couldn't
                                    // fuse the line back together, and "Select
                                    // similar formatting" treated the space as
                                    // unlike its own neighbours. The paragraph
                                    // tests that read this
                                    // (`tag_paragraph_test`,
                                    // `analytic_paragraph_test`) filter to runs
                                    // with real text anyway, so a blank line
                                    // still isn't a Tag however its runs are
                                    // marked.
                                    if run.style.is_none() {
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
                            crate::document::normalize::merge_adjacent_same_format_runs(
                                &mut para.runs,
                            );
                            // A blank line that *does* carry paragraph
                            // properties — `<w:p><w:pPr><w:jc .../></w:pPr></w:p>`,
                            // which Word writes constantly — closes with no runs
                            // at all. Every rich-text-aware function in this
                            // codebase assumes a paragraph holds at least one
                            // run (`default_paragraphs`), so leaving it empty
                            // panicked `sync_insert_char` on the first keystroke
                            // rather than misrendering.
                            if para.runs.is_empty() {
                                para.runs.push(Run::default());
                            }
                            paragraphs.push(para);
                        }
                        in_ppr = false;
                    }
                    b"w:pPr" => {
                        in_ppr = false;
                    }
                    b"w:numPr" => {
                        in_numpr = false;
                    }
                    b"w:r" => {
                        // Flush the completed run into the current paragraph.
                        if let (Some(run), Some(para)) = (current_run.take(), current_para.as_mut())
                        {
                            para.runs.push(run);
                        }
                        in_rpr = false;
                        in_text = false;
                    }
                    b"w:rPr" => {
                        in_rpr = false;
                    }
                    b"w:t" => {
                        in_text = false;
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    // Same invariant one level up: a body with no `<w:p>` this parser
    // recognised must still yield the one empty paragraph every caller
    // assumes, not an empty document that indexes out of bounds the moment
    // it is edited.
    if paragraphs.is_empty() {
        paragraphs.push(Paragraph {
            list: None,
            runs: vec![Run::default()],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        });
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
fn apply_run_character_style(
    e: &BytesStart,
    run: &mut Run,
    styles: &HashMap<String, StyleDefaults>,
) {
    let Some(style_id) = e.attributes().flatten().find_map(|attr| {
        (attr.key.as_ref() == b"w:val").then(|| String::from_utf8_lossy(&attr.value).into_owned())
    }) else {
        return;
    };
    let Some(defaults) = styles.get(&style_id) else {
        return;
    };
    run.bold = defaults.bold;
    run.size = defaults.size;
    run.underline = defaults.underline;
    run.double_underline = defaults.double_underline;
    run.box_format = run.box_format || defaults.box_format;
    if defaults.emphasis_boxed {
        run.emphasis_boxed = true;
        run.emphasis = true;
    }
    // Verbatim names its Emphasis card type with this exact style id, and so
    // does this app now. Read off the id rather than inferred from the style's
    // `<w:bdr>` (which is all `emphasis_boxed` above can see), because whether
    // Emphasis draws a box is a per-user setting here — an unboxed Emphasis is
    // still an Emphasis.
    if style_id == "Emphasis" {
        run.emphasis = true;
    }
}

/// Applies a run-property element to `run` based on the element's tag name.
/// The only values Word accepts for `<w:highlight w:val="…">` (ECMA-376
/// ST_HighlightColor). Anything else has to be written as shading instead —
/// see `run_props_xml`. `text_editor::highlight_color_hex` maps these same
/// names to their on-screen color, and a test there asserts the two agree.
pub(crate) const WORD_HIGHLIGHT_NAMES: [&str; 16] = [
    "yellow",
    "green",
    "cyan",
    "magenta",
    "blue",
    "red",
    "darkBlue",
    "darkCyan",
    "darkGreen",
    "darkMagenta",
    "darkRed",
    "darkYellow",
    "darkGray",
    "lightGray",
    "black",
    "white",
];

/// The `<w:rPr>` inner XML for one run. Split out of `write_docx` so the
/// highlight-vs-shading branch below can be tested directly.
fn run_props_xml(run: &Run) -> String {
    let mut out = String::new();
    // `<w:rStyle>` must lead `<w:rPr>` per the OOXML schema, so it is written
    // before any direct formatting.
    // `<w:rStyle>` is not repeatable in CT_RPr — a run gets exactly one
    // character style — so a run that is both a Cite/Analytic *and* emphasized
    // can only name one. The card marker wins: it is the structural identity,
    // and Emphasis's appearance is fully carried by the direct `<w:b>`/`<w:u>`/
    // `<w:bdr>` written below either way. `<w:vimbatimEmphasis/>` still records
    // the flag for this app's own round trip in that case.
    let rstyle = run
        .style
        .and_then(|s| s.docx_rstyle_id())
        .or(run.emphasis.then_some("Emphasis"));
    if let Some(style) = rstyle {
        out.push_str(&format!("<w:rStyle w:val=\"{style}\"/>",));
    }
    if run.bold {
        out.push_str("<w:b/>");
    }
    if run.italic {
        out.push_str("<w:i/>");
    }
    if run.strikethrough {
        out.push_str("<w:strike/>");
    }
    if run.double_underline {
        out.push_str("<w:u w:val=\"double\"/>");
    } else if run.underline {
        out.push_str("<w:u w:val=\"single\"/>");
    }
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
            out.push_str(&format!(
                "<w:highlight w:val=\"{}\"/>",
                escape_xml_attr(&run.highlight_color)
            ));
        } else {
            out.push_str(&format!(
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
                escape_xml_attr(&run.highlight_color),
            ));
        }
    }
    if run.size > 0 {
        out.push_str(&format!("<w:sz w:val=\"{}\"/>", run.size));
    }
    if let Some(font) = &run.font {
        out.push_str(&format!(
            "<w:rFonts w:ascii=\"{}\"/>",
            escape_xml_attr(font)
        ));
    }
    if let Some(color) = &run.color {
        out.push_str(&format!("<w:color w:val=\"{}\"/>", escape_xml_attr(color)));
    }
    // Custom markers, namespaced like `CardStyle::style_id` so they're
    // harmless to any other editor (Word ignores elements it doesn't know).
    // Kept alongside the `Emphasis` rStyle above: these two carry the flags
    // through this app's own round trip even when the rStyle slot was taken by
    // a card marker, and they distinguish "emphasized" from "emphasized and
    // boxed", which one character style cannot.
    if run.emphasis {
        out.push_str("<w:vimbatimEmphasis/>");
    }
    if run.emphasis_boxed {
        out.push_str("<w:vimbatimEmphasisBox/>");
    }
    out
}

/// Handles `w:b` (bold), `w:u` (underline), `w:highlight` (highlight colour),
/// `w:shd` (custom hex highlight, written as shading), `w:sz` (font size in
/// half-points), and `w:bdr` (border, i.e. the Emphasis box).  Unknown tags
/// are silently ignored.
/// One attribute's value, with XML entities resolved.
///
/// quick-xml hands back the *raw* source text between the quotes, so
/// `w:ascii="Foo &amp; Bar"` arrives as the literal seven-character string
/// `&amp;` embedded in the name. Every value that gets written straight back
/// into an attribute by `run_props_xml` has to come through here, because
/// that side now escapes what it emits: leaving the raw form in place would
/// re-escape it to `&amp;amp;` and grow the corruption by one round trip per
/// save. Decoding on the way in and encoding on the way out is the only
/// pairing that round-trips — and it is also what makes such a font name
/// display correctly in the picker instead of showing its own entity.
///
/// Falls back to the raw bytes when the value isn't decodable (a malformed
/// entity), which is what the parser did for every value before this.
fn attr_value(attr: &quick_xml::events::attributes::Attribute) -> String {
    attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).into_owned())
}

fn apply_run_prop(e: &BytesStart, run: &mut Run) {
    /*
     * This function is called for both `Event::Start` and `Event::Empty`
     * variants of each property element, avoiding duplicated match arms in the
     * main parser loop.  The element name is re-read from `e` rather than
     * passed as a parameter to keep the call site clean.
     */
    match e.name().as_ref() {
        b"w:b" => {
            run.bold = on_off_attr_is_true(e);
        }
        b"w:i" => {
            run.italic = on_off_attr_is_true(e);
        }
        b"w:strike" => {
            run.strikethrough = on_off_attr_is_true(e);
        }
        b"w:u" => {
            if !on_off_attr_is_true(e) {
                run.underline = false;
                run.double_underline = false;
            } else {
                let is_double = e
                    .attributes()
                    .flatten()
                    .any(|attr| attr.key.as_ref() == b"w:val" && attr.value.as_ref() == b"double");
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
                    run.highlight_color = attr_value(&attr);
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
                    let fill = attr_value(&attr);
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
                    run.font = Some(attr_value(&attr));
                }
            }
        }
        // `w:val="auto"` means "inherit the default" — treated the same as
        // the attribute being absent, so `color` stays `None`.
        b"w:color" => {
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:val" {
                    let val = attr_value(&attr);
                    if val != "auto" {
                        run.color = Some(val);
                    }
                }
            }
        }
        b"w:vimbatimEmphasis" => {
            run.emphasis = true;
        }
        b"w:vimbatimEmphasisBox" => {
            run.emphasis_boxed = true;
        }
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
    (
        ListKind::BulletSolidBox,
        "bullet",
        "\u{f0a7}",
        Some("Wingdings"),
    ),
    (
        ListKind::BulletDiamond,
        "bullet",
        "\u{f076}",
        Some("Wingdings"),
    ),
    (
        ListKind::BulletArrow,
        "bullet",
        "\u{f0d8}",
        Some("Wingdings"),
    ),
    (
        ListKind::BulletCheckmark,
        "bullet",
        "\u{f0fc}",
        Some("Wingdings"),
    ),
    (ListKind::NumberDecimalDot, "decimal", "%1.", None),
    (ListKind::NumberDecimalParen, "decimal", "%1)", None),
    (ListKind::NumberUpperRoman, "upperRoman", "%1.", None),
    (ListKind::NumberUpperLetter, "upperLetter", "%1.", None),
    (ListKind::NumberLowerLetterParen, "lowerLetter", "%1)", None),
    (ListKind::NumberLowerLetterDot, "lowerLetter", "%1.", None),
    (ListKind::NumberLowerRoman, "lowerRoman", "%1.", None),
];

fn abstract_num_id_for(kind: ListKind) -> u32 {
    LIST_KIND_TABLE
        .iter()
        .position(|(k, ..)| *k == kind)
        .unwrap() as u32
}

/// One `<w:lvl>` element. `ind_left`/`ind_hanging` are in twips (1/20 pt),
/// matching Word's own defaults confirmed from `Lists.docx`: `left`
/// increases 720 per level, `hanging` is 360 (180 at level 2 for the
/// numbered cascade specifically — see `cascade_level_xml`).
fn build_lvl_xml(
    ilvl: u8,
    num_fmt: &str,
    lvl_text: &str,
    font: Option<&str>,
    ind_left: u32,
    ind_hanging: u32,
) -> String {
    let font_xml = match font {
        Some(f) => {
            let f = escape_xml_attr(f);
            format!("<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\"/>")
        }
        None => String::new(),
    };
    format!(
        "<w:lvl w:ilvl=\"{ilvl}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{num_fmt}\"/><w:lvlText w:val=\"{lvl_text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{ind_left}\" w:hanging=\"{ind_hanging}\"/></w:pPr><w:rPr>{font_xml}</w:rPr></w:lvl>",
    )
}

/// The one fixed cascade every one of the 13 styles' levels 1-8 use,
/// independent of the level-0 style — confirmed by dumping the *complete*
/// ilvl 0-8 range (not just 0-2, an earlier gap in this function that
/// shipped a wrong 2-value cascade — see the fix history below) across
/// `Lists.docx`'s `BulletSolid` and `NumberDecimalDot`/`NumberUpperRoman`
/// abstractNums:
/// - Bullets: `o`/Courier New (level 1) -> `\u{f0a7}`/Wingdings (level 2) ->
///   `\u{f0b7}`/Symbol (level 3) -> repeats every 3 levels. `hanging` is 360
///   at every level.
/// - Numbers: `lowerLetter "%N."` (level 1) -> `lowerRoman "%N."` (level 2)
///   -> `decimal "%N."` (level 3) -> repeats every 3 levels, **independent
///   of the level-0 format** (confirmed against `NumberUpperRoman`'s own
///   abstractNum: its level 3 is `decimal`, not `upperRoman` again).
///   `hanging` is 360 except 180 at the `lowerRoman` position (level 2, 5, 8).
///
/// Fix history: the version that shipped with this function's own Task 3
/// only verified ilvl 0/1/2 and collapsed cascade positions 1 and 2 into
/// one `_ =>` match arm — silently correct for 2 of every 3 wrapped levels
/// and wrong on the third (bullets showed `\u{f0a7}` where real Word shows
/// `\u{f0b7}`; numbers showed `lowerRoman` where real Word shows `decimal`).
/// Caught by `test_list_marker_text_for_level_bullet_cascade_values`
/// (`text_editor.rs`, this fix's rendering-side counterpart) failing at
/// level 3 once that test was extended past level 2.
fn cascade_level_xml(ilvl: u8, is_bullet: bool) -> String {
    let ind_left = 720 * (ilvl as u32 + 1);
    let cascade_pos = (ilvl - 1) % 3; // ilvl 1,4,7 -> 0; 2,5,8 -> 1; 3,6 -> 2
    if is_bullet {
        let (lvl_text, font) = match cascade_pos {
            0 => ("o", "Courier New"),
            1 => ("\u{f0a7}", "Wingdings"),
            _ => ("\u{f0b7}", "Symbol"),
        };
        build_lvl_xml(ilvl, "bullet", lvl_text, Some(font), ind_left, 360)
    } else {
        let (num_fmt, hanging) = match cascade_pos {
            0 => ("lowerLetter", 360),
            1 => ("lowerRoman", 180),
            _ => ("decimal", 360),
        };
        build_lvl_xml(
            ilvl,
            num_fmt,
            &format!("%{}.", ilvl + 1),
            None,
            ind_left,
            hanging,
        )
    }
}

fn build_abstract_num_xml(
    id: u32,
    kind: ListKind,
    num_fmt: &str,
    lvl_text: &str,
    font: Option<&str>,
) -> String {
    let mut out = format!("<w:abstractNum w:abstractNumId=\"{id}\">");
    out.push_str(&build_lvl_xml(0, num_fmt, lvl_text, font, 720, 360));
    for ilvl in 1..=8u8 {
        out.push_str(&cascade_level_xml(ilvl, kind.is_bullet()));
    }
    out.push_str("</w:abstractNum>");
    out
}

/// Assigns a `numId` to every list paragraph's index, one fresh id per
/// contiguous run of list paragraphs *of the same `ListKind`* — a run ends
/// at the next paragraph with `list: None`, the next paragraph whose
/// `ListKind` differs from this run's, or the end of the document.
///
/// Confirmed against `Lists.docx` itself, not assumed: its six-different-
/// bullet-styles example (`Solid Bullet`, `Hollow Bullet`, ...) and its
/// seven-different-number-styles example are each six/seven *immediately
/// consecutive* paragraphs, and real Word still gives every single one its
/// own `numId` — a style change starts a new list even with no non-list
/// paragraph between them. Breaking only on `list.is_some()` becoming
/// false (an earlier version of this function) silently collapsed all six
/// bullet styles onto one shared `numId`, so only one of the six could
/// ever be looked up as that run's `ListKind` — caught by
/// `test_real_lists_docx_survives_save_and_reload` resolving a saved
/// `Checkmark` paragraph back as `BulletArrow` after a round trip.
///
/// Shared by `build_numbering_xml` (which needs the same numId->abstractNum
/// `<w:num>` entries) and `rebuild_document_xml` (which writes the numId
/// onto each paragraph's own `<w:numPr>`) so the two can never disagree
/// about which paragraph got which id.
fn assign_list_num_ids(paragraphs: &[Paragraph]) -> HashMap<usize, u32> {
    let mut assignment = HashMap::new();
    let mut next_num_id = 1u32;
    let mut i = 0;
    while i < paragraphs.len() {
        let Some(run_kind) = paragraphs[i].list.map(|item| item.kind) else {
            i += 1;
            continue;
        };
        while i < paragraphs.len() && paragraphs[i].list.map(|item| item.kind) == Some(run_kind) {
            assignment.insert(i, next_num_id);
            i += 1;
        }
        next_num_id += 1;
    }
    assignment
}

/// Builds `word/styles.xml` for a brand-new (`create_new_docx`) file:
/// `Normal`/`DefaultParagraphFont` (the base every other style needs to
/// exist, per spec and per every real Word file) plus `Heading1`-`Heading4`
/// carrying the `Pocket`/`Hat`/`Block`/`Tag` aliases Verbatim itself uses,
/// each with the exact `sz`/`b`/`u`/`pBdr`/`jc` values `AppState::apply_card_
/// style`'s current defaults already write directly onto a card-style
/// paragraph's own runs (`CardStyleKind::font_size`, `is_centered`) —
/// confirmed against `tests/fixtures/verbatim_emphasis_reference.docx`'s own
/// `word/styles.xml`, with the theme-font references (`*Theme` attrs,
/// `w:themeColor`), `w:rsid`, `w:link` (points at character styles this
/// minimal file never defines), and `w:pageBreakBefore` (a real behavior
/// change Verbatim's own reference file happens to carry, not something
/// `apply_card_style` does) stripped — none of those affect whether Word
/// resolves the style, and a dangling theme reference with no `theme1.xml`
/// A document's own `<w:docDefaults>` — the body font and size it declares
impl NewDocStyle {
    /// `<w:docDefaults>` built from the user's own settings, not Verbatim's
    /// numbers. Verbatim hardcodes 11pt with 1.15 line spacing and 8pt
    /// paragraph spacing; copying that would make Word render spacing the
    /// Vimbatim editor never shows, and would ignore anyone who changed either
    /// setting. Deliberately carries no `<w:rFonts>`: this app has no
    /// default-font setting, so it has no opinion to record and Word's own
    /// default font is the honest answer.
    ///
    /// `w:line` is in 240ths (Word's "auto" line rule), `w:after="0"` because
    /// this app has no paragraph-spacing concept to express.
    fn doc_defaults_xml(&self) -> String {
        let line = (self.line_spacing * 240.0).round().max(1.0) as u32;
        format!(
            "<w:docDefaults>\
<w:rPrDefault><w:rPr><w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/></w:rPr></w:rPrDefault>\
<w:pPrDefault><w:pPr><w:spacing w:after=\"0\" w:line=\"{line}\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault>\
</w:docDefaults>",
            sz = self.normal_size,
        )
    }
}

/// part is itself the kind of thing that triggers a repair prompt.
///
/// Bug report: applying a card style to a paragraph in a file created fresh
/// in Vimbatim showed all the right direct formatting in Word but was
/// missing its actual heading level (Navigation pane / Styles dropdown) —
/// `create_new_docx` referenced `<w:pStyle w:val="Heading1"/>` but the
/// package never had a `word/styles.xml` for that id to resolve against, so
/// Word fell back to Normal for the *style association* while still
/// painting the paragraph's own direct bold/size/box/underline. Always
/// emitted (not conditional on whether a card style is used yet): the
/// resave path (`write_docx`) has no equivalent of `build_numbering_xml`'s
/// "wrote_numbering" on-demand fallback, so a style applied *after* file
/// creation would have nowhere to ever gain this part otherwise.
fn build_new_doc_styles_xml(style: NewDocStyle) -> String {
    // `<w:docDefaults>` leads `<w:styles>` per CT_Styles' declared sequence
    // (docDefaults, latentStyles, style*). `Normal` itself stays bare — this
    // app has no default-font setting to put in it, and the document-wide
    // defaults above already carry the size and spacing it would otherwise
    // duplicate.
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
{doc_defaults}\
<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>\
<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\">\
<w:name w:val=\"Default Paragraph Font\"/><w:semiHidden/><w:unhideWhenUsed/></w:style>",
        doc_defaults = style.doc_defaults_xml(),
    );
    // (alias, heading level, font size half-points, centered, underline).
    // The sizes come from the caller's settings rather than being restated
    // here: they used to be hardcoded copies of `CardStyleKind::font_size`
    // while `apply_card_style` applied the configurable values, so the style
    // definition and the runs it described could disagree.
    //
    // `page_break` mirrors Verbatim exactly: Pocket, Hat and Block each start a
    // new page in Word, Tag does not. Vimbatim's own editor is continuous and
    // never reads `<w:pageBreakBefore/>`, so this changes nothing here and
    // makes a card file paginate in Word the way a Verbatim-authored one does.
    let kinds: [(&str, u8, u16, bool, Option<&str>, bool, u16); 4] = [
        ("Pocket", 1, style.pocket_size, true, None, true, 240),
        ("Hat", 2, style.hat_size, true, Some("double"), true, 40),
        ("Block", 3, style.block_size, true, Some("single"), true, 40),
        ("Tag", 4, style.tag_size, false, None, false, 40),
    ];
    for (alias, level, size, centered, underline, page_break, space_before) in kinds {
        let mut ppr = String::from("<w:keepNext/><w:keepLines/>");
        if page_break {
            ppr.push_str("<w:pageBreakBefore/>");
        }
        if alias == "Pocket" {
            ppr.push_str(
                // CT_PBdr's declared sequence is top, left, bottom, right,
                // between, bar — not the top/bottom/left/right this used to
                // emit. Word has already been observed in this codebase to
                // silently drop an out-of-order border child (see
                // `run_props_xml`'s `<w:bdr>` note), and real Verbatim writes
                // the schema order, so there is nothing to gain by deviating.
                "<w:pBdr>\
                <w:top w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:left w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                <w:bottom w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:right w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                </w:pBdr>",
            );
        }
        // `w:after="0"` with a small `w:before`, as Verbatim has it: card
        // styles sit tight against the text they head.
        ppr.push_str(&format!(
            "<w:spacing w:before=\"{space_before}\" w:after=\"0\"/>"
        ));
        if centered {
            ppr.push_str("<w:jc w:val=\"center\"/>");
        }
        ppr.push_str(&format!("<w:outlineLvl w:val=\"{}\"/>", level - 1));

        let mut rpr = format!("<w:b/><w:sz w:val=\"{size}\"/>");
        if let Some(u) = underline {
            rpr.push_str(&format!("<w:u w:val=\"{u}\"/>"));
        }

        // `<w:link>` pairs the paragraph style with a character style of the
        // same formatting, which is how Word's Styles pane offers a card style
        // for a run selection rather than a whole paragraph. It has to name a
        // style that exists — a dangling `<w:link>` is the same defect as the
        // dangling `<w:rStyle>` markers this app used to write — so each
        // `HeadingNChar` is emitted right alongside.
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\">\
            <w:name w:val=\"heading {level}\"/><w:aliases w:val=\"{alias}\"/>\
            <w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>\
            <w:link w:val=\"Heading{level}Char\"/><w:qFormat/>\
            <w:pPr>{ppr}</w:pPr><w:rPr>{rpr}</w:rPr></w:style>\
            <w:style w:type=\"character\" w:styleId=\"Heading{level}Char\">\
            <w:name w:val=\"Heading {level} Char\"/><w:aliases w:val=\"{alias} Char\"/>\
            <w:basedOn w:val=\"DefaultParagraphFont\"/><w:link w:val=\"Heading{level}\"/>\
            <w:rPr>{rpr}</w:rPr></w:style>",
        ));
    }

    // Cite — Verbatim's own id and alias. `<w:u w:val="none"/>` matches
    // Verbatim's definition: a Cite is bold at its size and explicitly *not*
    // underlined, so it does not inherit an underline from anything around it.
    out.push_str(&format!(
        "<w:style w:type=\"character\" w:styleId=\"Style13ptBold\">\
        <w:name w:val=\"Style 13 pt Bold\"/><w:aliases w:val=\"Cite\"/>\
        <w:basedOn w:val=\"DefaultParagraphFont\"/><w:qFormat/>\
        <w:rPr><w:b/><w:bCs/><w:sz w:val=\"{cite}\"/><w:u w:val=\"none\"/></w:rPr></w:style>",
        cite = style.cite_size,
    ));

    // Emphasis — Verbatim's id, but defined from *this* app's Emphasis
    // settings rather than copying Verbatim's fixed bold+underline+box. What
    // Emphasis means is a user preference here (Settings -> Text Settings), and
    // a style that disagreed with the direct formatting on the run would show
    // one thing in Word's Styles pane and another on the page.
    let mut emphasis_rpr = String::new();
    if style.emphasis_bold {
        emphasis_rpr.push_str("<w:b/>");
    }
    if style.emphasis_underline {
        emphasis_rpr.push_str("<w:u w:val=\"single\"/>");
    }
    if let Some(size) = style.emphasis_size {
        emphasis_rpr.push_str(&format!("<w:sz w:val=\"{size}\"/>"));
    }
    if style.emphasis_box {
        // Same run border Verbatim's own Emphasis style carries, and the same
        // one `run_props_xml` writes directly onto the run.
        emphasis_rpr
            .push_str("<w:bdr w:val=\"single\" w:sz=\"12\" w:space=\"0\" w:color=\"auto\"/>");
    }
    out.push_str(&format!(
        "<w:style w:type=\"character\" w:styleId=\"Emphasis\">\
        <w:name w:val=\"Emphasis\"/><w:basedOn w:val=\"DefaultParagraphFont\"/><w:qFormat/>\
        <w:rPr>{emphasis_rpr}</w:rPr></w:style>",
    ));

    // Analytic has no Verbatim equivalent, so it keeps this app's own id — and
    // therefore has to be defined here, or it is the one remaining `<w:rStyle>`
    // pointing at nothing. Colour is left to the direct formatting on the run:
    // it is a per-document choice (`analytic_color`), not part of what the
    // style *is*.
    out.push_str(&format!(
        "<w:style w:type=\"character\" w:styleId=\"VimbatimAnalytic\">\
        <w:name w:val=\"Analytic\"/><w:basedOn w:val=\"DefaultParagraphFont\"/><w:qFormat/>\
        <w:rPr><w:b/><w:sz w:val=\"{tag}\"/></w:rPr></w:style>",
        tag = style.tag_size,
    ));

    out.push_str("</w:styles>");
    out
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
        out.push_str(&build_abstract_num_xml(
            id as u32, *kind, num_fmt, lvl_text, *font,
        ));
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
        let para_idx = num_ids
            .iter()
            .find(|(_, id)| **id == num_id)
            .map(|(idx, _)| *idx)
            .unwrap();
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
            //
            // Children in CT_PBdr's declared order (top, left, bottom, right)
            // and `w:color="auto"` rather than a hard `000000`: `auto` follows
            // the text colour the way Verbatim's own style does, so the box
            // stays visible against a dark document theme or coloured text.
            ppr.push_str(
                "<w:pBdr>\
                <w:top w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:left w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                <w:bottom w:val=\"single\" w:sz=\"24\" w:space=\"1\" w:color=\"auto\"/>\
                <w:right w:val=\"single\" w:sz=\"24\" w:space=\"4\" w:color=\"auto\"/>\
                </w:pBdr>",
            );
        }
        // `para.heading != 0` guard: defense in depth against a paragraph
        // that (through some path other than `AppState::apply_card_style`,
        // which now clears `.list` itself) still carries both a heading and
        // a list — `<w:pStyle>` isn't repeatable per CT_PPrBase, and the
        // heading one above already went out, so a second here would make
        // this `<w:pPr>` invalid OOXML. Heading wins: a card-style line was
        // never conceptually still a list item.
        if para.heading == 0 {
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
                // has to say what it is. Only the markers that actually emit
                // an `<w:rStyle>` count, or a Pocket run with no direct
                // formatting would open an empty `<w:rPr>` for nothing.
                || run.style.and_then(|s| s.docx_rstyle_id()).is_some()
                || run.emphasis || run.emphasis_boxed;
            if has_props {
                out.push_str("<w:rPr>");
                out.push_str(&run_props_xml(run));
                out.push_str("</w:rPr>");
            }
            // Emit xml:space="preserve" whenever the run's own text has
            // leading/trailing whitespace — derived from the text itself
            // rather than trusting `run.whitespace_preserve` (a stale
            // parse-time flag that a later split/edit, e.g. carving a
            // highlighted word out of a longer run, never recomputes for
            // the new boundary). Without this, Word silently trims an edge
            // space that vimbatim's own renderer — which reads `run.text`
            // directly — still showed.
            let needs_preserve = run.text.starts_with(char::is_whitespace)
                || run.text.ends_with(char::is_whitespace);
            let space_attr = if needs_preserve {
                " xml:space=\"preserve\""
            } else {
                ""
            };
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
    let start = xml.rfind("<w:sectPr")?;
    let end_tag = "</w:sectPr>";
    let end = xml[start..].find(end_tag)? + start + end_tag.len();
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
pub fn create_new_docx(
    paragraphs: &[Paragraph],
    path: &Path,
    style: NewDocStyle,
) -> Result<(), Box<dyn std::error::Error>> {
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
    let numbering_xml = build_numbering_xml(paragraphs);
    // Always built, unlike numbering_xml: `write_docx` (the resave path)
    // has no on-demand fallback for a missing word/styles.xml the way it
    // does for numbering (`wrote_numbering`), so a style applied *after*
    // creation would have no part to ever land in if this were conditional
    // — see `build_new_doc_styles_xml`'s own doc comment.
    let styles_xml = build_new_doc_styles_xml(style);

    let tmp_path = tmp_write_path(path);
    let tmp_file = std::fs::File::create(&tmp_path)?;
    let mut writer = ZipWriter::new(tmp_file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let content_types_numbering_override = if numbering_xml.is_some() {
        "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>"
    } else {
        ""
    };
    writer.start_file("[Content_Types].xml", opts)?;
    writer.write_all(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>\
<Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.openxmlformats-package.core-properties+xml\"/>\
<Override PartName=\"/docProps/app.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.extended-properties+xml\"/>\
{content_types_numbering_override}\
</Types>"
    ).as_bytes())?;

    writer.start_file("_rels/.rels", opts)?;
    writer.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" \
Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" \
Target=\"word/document.xml\"/>\
<Relationship Id=\"rIdCore\" \
Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" \
Target=\"docProps/core.xml\"/>\
<Relationship Id=\"rIdApp\" \
Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" \
Target=\"docProps/app.xml\"/>\
</Relationships>",
    )?;

    // rId2 is reserved for numbering (below, when present) — styles always
    // gets rId3 regardless, so the two can never collide.
    let rels_numbering = if numbering_xml.is_some() {
        "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering\" Target=\"numbering.xml\"/>"
    } else {
        ""
    };
    writer.start_file("word/_rels/document.xml.rels", opts)?;
    writer.write_all(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>\
<Relationship Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/>\
{rels_numbering}</Relationships>"
    ).as_bytes())?;

    // The optional parts Word supplies defaults for, but which every real
    // document carries — a Vimbatim file used to have five parts where a
    // Verbatim one has eighteen. `settings.xml` gives Word an explicit
    // `defaultTabStop` instead of an implied one; `docProps` is where a title
    // and timestamps live, and without it every tool that reads document
    // metadata showed blanks.
    //
    // Deliberately still absent: `fontTable.xml`, `webSettings.xml`,
    // `theme1.xml`, `footnotes.xml`, `endnotes.xml`. Nothing here references
    // them (no style uses a theme colour or font), so they would be parts that
    // exist only to look complete.
    writer.start_file("word/settings.xml", opts)?;
    writer.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:settings xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:defaultTabStop w:val=\"720\"/>\
</w:settings>",
    )?;

    writer.start_file("docProps/core.xml", opts)?;
    writer.write_all(
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<cp:coreProperties \
xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
xmlns:dcterms=\"http://purl.org/dc/terms/\" \
xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
<dc:title>{title}</dc:title>\
<cp:revision>1</cp:revision>\
</cp:coreProperties>",
            // The file's own name, escaped: it is the only title this app knows,
            // and an unescaped `&` in a filename would produce a part Word rejects.
            title = escape_xml_text(
                path.file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Untitled"),
            ),
        )
        .as_bytes(),
    )?;

    writer.start_file("docProps/app.xml", opts)?;
    writer.write_all(
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<Properties \
xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" \
xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\
<Application>Vimbatim</Application>\
</Properties>",
    )?;

    writer.start_file("word/document.xml", opts)?;
    writer.write_all(document_xml.as_bytes())?;

    writer.start_file("word/styles.xml", opts)?;
    writer.write_all(styles_xml.as_bytes())?;

    if let Some(numbering_xml) = &numbering_xml {
        writer.start_file("word/numbering.xml", opts)?;
        writer.write_all(numbering_xml.as_bytes())?;
    }

    writer.finish()?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Escapes the three XML-significant characters in text content:
/// `&` → `&amp;`, `<` → `&lt;`, `>` → `&gt;`.
/// The attribute-value counterpart to `escape_xml_text`.
///
/// Text nodes only have to hide `&`, `<` and `>`; an attribute value sits
/// inside quotes and must hide those too, or the attribute simply ends early.
/// Without this, a font family carrying a `"` — `font_import` takes the name
/// verbatim from the TTF `name` table, and a hand-edited `settings.conf`
/// colour reaches `w:color` the same way — produced
/// `<w:rFonts w:ascii="Ev"il"/>`, a document Word refuses to open. Paired
/// with `attr_value` on the read side; see the note there for why neither
/// half works alone.
fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
#[path = "docx_parser_tests.rs"]
mod tests;
