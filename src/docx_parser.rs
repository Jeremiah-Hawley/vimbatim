use std::io::{Read, Write};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use zip::ZipArchive;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

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
    /// True when `xml:space="preserve"` is set on `<w:t>` — required to keep
    /// leading/trailing whitespace that XML parsers would otherwise strip.
    pub whitespace_preserve: bool,
}

/// One paragraph of the document, composed of zero or more runs.
/// `heading` is 0 for body text, or 1–9 mirroring Word's Heading 1–9 styles.
#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub runs: Vec<Run>,
    pub heading: u8,
    pub alignment: Alignment,  // left, center, right, justify
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
}

impl DocxOrigin {
    /// Saves `paragraphs` back to `path` as a .docx file, using this
    /// origin's preserved preamble/sectPr/raw ZIP as the template.
    pub fn save(&self, paragraphs: &[Paragraph], path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        /*
         * Generate the new XML from the given paragraph model, then hand
         * the bytes off to `write_docx`, which handles the ZIP round-trip.
         */
        let new_xml = rebuild_document_xml(&self.preamble, &self.sect_pr, paragraphs);
        write_docx(&self.raw_zip, &new_xml, path)
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

    let paragraphs = parse_document_xml(&document_xml)?;

    // Extract the fragments we need for round-trip serialisation at parse
    // time so we can discard the full XML string afterwards.
    let preamble = extract_preamble(&document_xml).unwrap_or_else(fallback_preamble);
    let sect_pr = extract_sect_pr(&document_xml).unwrap_or("").to_string();

    Ok((paragraphs, DocxOrigin { raw_zip, preamble, sect_pr }))
}

/// Writes `new_xml` into the .docx at `path`, replacing `word/document.xml`
/// and copying all other ZIP entries verbatim from `raw_zip`.
///
/// An atomic temp-file rename (`path + ".tmp"`) prevents partial writes from
/// corrupting the original if the process is interrupted.
fn write_docx(raw_zip: &[u8], new_xml: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    /*
     * Open the original ZIP from the in-memory byte slice, then stream each
     * entry into a new ZIP writer:
     *  - For word/document.xml: write the freshly generated XML with Deflate.
     *  - For everything else: raw_copy_file copies the compressed bytes without
     *    decompressing, preserving the original compression level and metadata.
     *
     * The new file is written to a .tmp path first and renamed atomically at
     * the end to avoid leaving a corrupt file if an error occurs mid-write.
     */
    let cursor = std::io::Cursor::new(raw_zip);
    let mut archive = ZipArchive::new(cursor)?;

    let tmp_path = path.with_extension("docx.tmp");
    let tmp_file = std::fs::File::create(&tmp_path)?;
    let mut writer = ZipWriter::new(tmp_file);

    for i in 0..archive.len() {
        let file = archive.by_index_raw(i)?;
        let name = file.name().to_string();
        if name == "word/document.xml" {
            // Drop the borrow on `archive` before writing to `writer`.
            drop(file);
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
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

/// Parses the XML string from `word/document.xml` into a flat `Vec<Paragraph>`.
///
/// Uses quick-xml's streaming event API (no DOM tree is built) to keep memory
/// use proportional to the longest run of text, not the full document size.
/// Boolean flags (`in_ppr`, `in_rpr`, `in_text`) track the parser's position
/// in the nesting hierarchy so attribute-reading only fires in the right context.
fn parse_document_xml(xml: &str) -> Result<Vec<Paragraph>, Box<dyn std::error::Error>> {
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

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(ref e) => {
                match e.name().as_ref() {
                    b"w:p" => {
                        current_para = Some(Paragraph { runs: Vec::new(), heading: 0, alignment: Alignment::default() });
                    }
                    b"w:pPr" => { in_ppr = true; }
                    b"w:pStyle" if in_ppr => {
                        if let Some(para) = current_para.as_mut() {
                            apply_para_style(e, para);
                        }
                    }
                    b"w:r" => {
                        current_run = Some(Run::default());
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
                    // Catch-all for run-property elements (w:b, w:u, etc.).
                    _ if in_rpr => {
                        if let Some(run) = current_run.as_mut() {
                            apply_run_prop(e, run);
                        }
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
                        }
                    }
                    _ if in_rpr => {
                        if let Some(run) = current_run.as_mut() {
                            apply_run_prop(e, run);
                        }
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
                        if let Some(para) = current_para.take() {
                            paragraphs.push(para);
                        }
                        in_ppr = false;
                    }
                    b"w:pPr" => { in_ppr = false; }
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

/// Applies a run-property element to `run` based on the element's tag name.
/// Handles `w:b` (bold), `w:u` (underline), `w:highlight` (highlight colour),
/// and `w:sz` (font size in half-points).  Unknown tags are silently ignored.
fn apply_run_prop(e: &BytesStart, run: &mut Run) {
    /*
     * This function is called for both `Event::Start` and `Event::Empty`
     * variants of each property element, avoiding duplicated match arms in the
     * main parser loop.  The element name is re-read from `e` rather than
     * passed as a parameter to keep the call site clean.
     */
    match e.name().as_ref() {
        b"w:b" => { run.bold = true; }
        b"w:i" => { run.italic = true; }
        b"w:u" => { run.underline = true; }
        b"w:highlight" => {
            run.highlight = true;
            for attr in e.attributes().flatten() {
                if attr.key.as_ref() == b"w:val" {
                    run.highlight_color = String::from_utf8_lossy(&attr.value).into_owned();
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
        _ => {}
    }
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

    for para in paragraphs {
        out.push_str("<w:p>");
        for run in &para.runs {
            out.push_str("<w:r>");
            let has_props = run.bold || run.italic || run.underline || run.highlight
                || run.size > 0 || run.font.is_some() || run.color.is_some();
            if has_props {
                out.push_str("<w:rPr>");
                if run.bold      { out.push_str("<w:b/>"); }
                if run.italic    { out.push_str("<w:i/>"); }
                if run.underline { out.push_str("<w:u w:val=\"single\"/>"); }
                if run.highlight {
                    out.push_str(&format!("<w:highlight w:val=\"{}\"/>", run.highlight_color));
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

    let tmp_path = path.with_extension("docx.tmp");
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

    fn wrap_run_xml(run_xml: &str) -> String {
        format!(
            "<w:document><w:body><w:p><w:r>{}<w:t>hi</w:t></w:r></w:p></w:body></w:document>",
            run_xml
        )
    }

    // ── italic/font/color parsing (rich-text formatting plan, Phase 1) ──────

    #[test]
    fn test_parses_italic_run_property() {
        let xml = wrap_run_xml("<w:rPr><w:i/></w:rPr>");
        let paragraphs = parse_document_xml(&xml).unwrap();
        assert!(paragraphs[0].runs[0].italic);
    }

    #[test]
    fn test_parses_run_font_ascii_attribute() {
        let xml = wrap_run_xml(r#"<w:rPr><w:rFonts w:ascii="Georgia"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml).unwrap();
        assert_eq!(paragraphs[0].runs[0].font, Some("Georgia".to_string()));
    }

    #[test]
    fn test_parses_run_color_value() {
        let xml = wrap_run_xml(r#"<w:rPr><w:color w:val="FF0000"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml).unwrap();
        assert_eq!(paragraphs[0].runs[0].color, Some("FF0000".to_string()));
    }

    #[test]
    fn test_color_val_auto_is_treated_as_none() {
        let xml = wrap_run_xml(r#"<w:rPr><w:color w:val="auto"/></w:rPr>"#);
        let paragraphs = parse_document_xml(&xml).unwrap();
        assert_eq!(paragraphs[0].runs[0].color, None);
    }

    #[test]
    fn test_run_without_new_properties_defaults_to_none() {
        let xml = wrap_run_xml("");
        let paragraphs = parse_document_xml(&xml).unwrap();
        assert!(!paragraphs[0].runs[0].italic);
        assert_eq!(paragraphs[0].runs[0].font, None);
        assert_eq!(paragraphs[0].runs[0].color, None);
    }

    // ── italic/font/color re-emission (rebuild_document_xml) ────────────────

    #[test]
    fn test_rebuild_emits_italic() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), italic: true, ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains("<w:i/>"));
    }

    #[test]
    fn test_rebuild_emits_font_ascii() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), font: Some("Georgia".into()), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:rFonts w:ascii="Georgia"/>"#));
    }

    #[test]
    fn test_rebuild_emits_color() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), color: Some("FF0000".into()), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(xml.contains(r#"<w:color w:val="FF0000"/>"#));
    }

    #[test]
    fn test_rebuild_omits_rpr_entirely_when_no_properties_set() {
        let paragraphs = vec![Paragraph {
            runs: vec![Run { text: "hi".into(), ..Run::default() }],
            heading: 0,
            alignment: Alignment::default(),
        }];
        let xml = rebuild_document_xml("<w:document>", "", &paragraphs);
        assert!(!xml.contains("<w:rPr>"));
    }

    #[test]
    fn test_italic_font_color_round_trip_through_parse_and_rebuild() {
        let original = vec![Paragraph {
            runs: vec![Run {
                text: "hi".into(),
                italic: true,
                font: Some("Georgia".into()),
                color: Some("00FF00".into()),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
        }];
        let xml = rebuild_document_xml("<w:document>", "", &original);
        // rebuild_document_xml wraps in <w:body>...</w:body></w:document>,
        // matching what parse_document_xml expects to find.
        let reparsed = parse_document_xml(&xml).unwrap();
        assert_eq!(reparsed[0].runs[0].italic, true);
        assert_eq!(reparsed[0].runs[0].font, Some("Georgia".to_string()));
        assert_eq!(reparsed[0].runs[0].color, Some("00FF00".to_string()));
    }
}
