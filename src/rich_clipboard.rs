use crate::docx_parser::{Alignment, CardStyle, Run};

/// One copied paragraph's *paragraph-level* formatting: `(heading, alignment)`.
///
/// Runs alone can't express a card style. Pocket/Hat/Block/Tag are run-level
/// bold/size/box **plus** `Paragraph.heading` and a centred `Paragraph.alignment`
/// (`AppState::apply_card_style`), and only the run half used to reach the
/// clipboard — so pasting four card-style lines produced four correctly-sized
/// but structurally plain paragraphs.
pub type ParagraphAttrs = (u8, Alignment);

/// Separates the paragraph-attribute section from the run section. A distinct
/// byte (ASCII group separator) rather than reusing `\x1e` so metadata written
/// by a build that predates paragraph attributes still decodes — it simply has
/// no attribute section, and paste falls back to leaving paragraphs as the
/// split left them.
const SECTION_SEP: char = '\x1d';

/// Serializes copied plain text alongside its per-run *and* per-paragraph
/// formatting so an in-app paste (Ctrl+V) can restore both. Stored as GPUI
/// clipboard *metadata* (`ClipboardItem::new_string_with_metadata`), riding
/// alongside the plain text every other app already sees — external apps that
/// read only `.text()` are unaffected.
///
/// Two `\x1d`-separated sections. First the paragraph attributes, one
/// `heading\x1falignment` record per copied paragraph, `\x1e`-separated. Then
/// the runs: one per record (`\x1e`-separated), fields within a record
/// separated by `\x1f`. A trailing text-per-run field would need escaping
/// (text can contain `\x1e`/`\x1f` in theory); instead the byte length of each
/// run's text is stored so `decode()` can slice the accompanying plain text
/// (the clipboard's own `.text()`) without ambiguity.
pub fn encode_with_lengths(runs: &[Run], paragraphs: &[ParagraphAttrs]) -> String {
    let paras = paragraphs
        .iter()
        .map(|(heading, alignment)| format!("{}\x1f{}", heading, encode_alignment(*alignment)))
        .collect::<Vec<_>>()
        .join("\x1e");
    let runs = runs
        .iter()
        .map(|r| format!("{}\x1f{}", encode_fields(r), r.text.len()))
        .collect::<Vec<_>>()
        .join("\x1e");
    format!("{paras}{SECTION_SEP}{runs}")
}

fn encode_alignment(a: Alignment) -> u8 {
    match a {
        Alignment::Left => 0,
        Alignment::Center => 1,
        Alignment::Right => 2,
        Alignment::Justify => 3,
    }
}

fn decode_alignment(v: u8) -> Option<Alignment> {
    match v {
        0 => Some(Alignment::Left),
        1 => Some(Alignment::Center),
        2 => Some(Alignment::Right),
        3 => Some(Alignment::Justify),
        _ => None,
    }
}

fn encode_fields(r: &Run) -> String {
    format!(
        "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
        r.bold, r.italic, r.underline, r.double_underline, r.strikethrough,
        r.highlight, r.highlight_color, r.size,
        r.font.as_deref().unwrap_or(""), r.color.as_deref().unwrap_or(""),
        r.box_format,
        // Carried so a copied Cite or Analytic is still one after pasting —
        // the marker is what every command identifying them reads.
        r.style.map(|s| s.style_id()).unwrap_or(""),
    )
}

/// Reconstructs the runs and paragraph attributes from `metadata` (this
/// module's own encoding) and `plain_text` (the clipboard's own `.text()`, the
/// source of truth for characters — metadata carries only formatting, per-run
/// byte lengths, and per-paragraph markers).
///
/// Returns `None` on any malformed/foreign metadata (e.g. another app wrote
/// unrelated metadata) so the caller can fall back to plain paste. Metadata
/// with no paragraph section (written by a build predating it) decodes fine,
/// with an empty attribute list.
pub fn decode(metadata: &str, plain_text: &str) -> Option<(Vec<Run>, Vec<ParagraphAttrs>)> {
    let (para_section, run_section) = match metadata.split_once(SECTION_SEP) {
        Some((paras, runs)) => (paras, runs),
        None => ("", metadata),
    };

    let mut paragraphs = Vec::new();
    if !para_section.is_empty() {
        for record in para_section.split('\x1e') {
            let (heading, alignment) = record.split_once('\x1f')?;
            paragraphs.push((
                heading.parse().ok()?,
                decode_alignment(alignment.parse().ok()?)?,
            ));
        }
    }

    let mut runs = Vec::new();
    let mut offset = 0usize;
    for record in run_section.split('\x1e') {
        let fields: Vec<&str> = record.split('\x1f').collect();
        if fields.len() != 13 { return None; }
        let len: usize = fields[12].parse().ok()?;
        if offset + len > plain_text.len() || !plain_text.is_char_boundary(offset)
            || !plain_text.is_char_boundary(offset + len)
        {
            return None;
        }
        runs.push(Run {
            text: plain_text[offset..offset + len].to_string(),
            bold: fields[0].parse().ok()?,
            italic: fields[1].parse().ok()?,
            underline: fields[2].parse().ok()?,
            double_underline: fields[3].parse().ok()?,
            strikethrough: fields[4].parse().ok()?,
            highlight: fields[5].parse().ok()?,
            highlight_color: fields[6].to_string(),
            size: fields[7].parse().ok()?,
            font: (!fields[8].is_empty()).then(|| fields[8].to_string()),
            color: (!fields[9].is_empty()).then(|| fields[9].to_string()),
            box_format: fields[10].parse().ok()?,
            whitespace_preserve: false,
            style: CardStyle::from_style_id(fields[11]),
        });
        offset += len;
    }
    (offset == plain_text.len()).then_some((runs, paragraphs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_single_run() {
        let run = Run { text: "hello".into(), bold: true, size: 24, ..Run::default() };
        let encoded = encode_with_lengths(&[run.clone()], &[(0, Alignment::Left)]);
        let (decoded, paras) = decode(&encoded, "hello").unwrap();
        assert_eq!(decoded, vec![run]);
        assert_eq!(paras, vec![(0, Alignment::Left)]);
    }

    #[test]
    fn round_trips_multiple_runs_with_multibyte_text() {
        let runs = vec![
            Run { text: "héllo ".into(), bold: true, ..Run::default() },
            Run { text: "world".into(), italic: true, ..Run::default() },
        ];
        let encoded = encode_with_lengths(&runs, &[(0, Alignment::Left)]);
        let plain = "héllo world";
        let (decoded, _) = decode(&encoded, plain).unwrap();
        assert_eq!(decoded, runs);
    }

    #[test]
    fn rejects_malformed_metadata() {
        assert_eq!(decode("garbage", "hello"), None);
    }

    /// Metadata written before the paragraph section existed has no `\x1d`,
    /// and must still paste (runs only) rather than being rejected outright —
    /// otherwise a copy made in an older build, still sitting on the
    /// clipboard, silently degrades to plain text.
    #[test]
    fn decodes_legacy_metadata_with_no_paragraph_section() {
        let run = Run { text: "hello".into(), bold: true, ..Run::default() };
        let legacy = format!("{}\x1f{}", encode_fields(&run), run.text.len());
        let (runs, paras) = decode(&legacy, "hello").unwrap();
        assert_eq!(runs, vec![run]);
        assert!(paras.is_empty());
    }

    #[test]
    fn round_trips_paragraph_attrs_for_every_alignment() {
        let run = Run { text: "x".into(), ..Run::default() };
        let attrs = vec![
            (1, Alignment::Center),
            (2, Alignment::Right),
            (3, Alignment::Justify),
            (0, Alignment::Left),
        ];
        let encoded = encode_with_lengths(&[run], &attrs);
        let (_, decoded) = decode(&encoded, "x").unwrap();
        assert_eq!(decoded, attrs);
    }

    #[test]
    fn rejects_metadata_whose_lengths_dont_match_the_text() {
        // "hi" is a different byte length than "hello" (decode only checks
        // total length, not content, so a same-length swap would slip
        // through undetected — that's an inherent, documented limitation,
        // not what this test is exercising).
        let run = Run { text: "hello".into(), ..Run::default() };
        let encoded = encode_with_lengths(&[run], &[(0, Alignment::Left)]);
        assert_eq!(decode(&encoded, "hi"), None);
    }
}
