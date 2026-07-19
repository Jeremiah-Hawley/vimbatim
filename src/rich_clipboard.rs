use crate::docx_parser::Run;

/// Serializes copied plain text alongside its per-run formatting so an
/// in-app paste (Ctrl+V) can restore formatting. Stored as GPUI clipboard
/// *metadata* (`ClipboardItem::new_string_with_metadata`), riding alongside
/// the plain text every other app already sees — external apps that read
/// only `.text()` are unaffected.
///
/// One run per line (`\x1e`-separated), fields within a line separated by
/// `\x1f`. A trailing text-per-run field would need escaping (text can
/// contain `\x1e`/`\x1f` in theory); instead the byte length of each run's
/// text is stored so `decode()` can slice the accompanying plain text (the
/// clipboard's own `.text()`) without ambiguity.
pub fn encode_with_lengths(runs: &[Run]) -> String {
    runs.iter()
        .map(|r| format!("{}\x1f{}", encode_fields(r), r.text.len()))
        .collect::<Vec<_>>()
        .join("\x1e")
}

fn encode_fields(r: &Run) -> String {
    format!(
        "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
        r.bold, r.italic, r.underline, r.double_underline, r.strikethrough,
        r.highlight, r.highlight_color, r.size,
        r.font.as_deref().unwrap_or(""), r.color.as_deref().unwrap_or(""),
        r.box_format
    )
}

/// Reconstructs `Vec<Run>` from `metadata` (this module's own encoding) and
/// `plain_text` (the clipboard's own `.text()`, the source of truth for
/// characters — metadata carries only formatting + per-run byte lengths).
/// Returns `None` on any malformed/foreign metadata (e.g. another app wrote
/// unrelated metadata) so the caller can fall back to plain paste.
pub fn decode(metadata: &str, plain_text: &str) -> Option<Vec<Run>> {
    let mut runs = Vec::new();
    let mut offset = 0usize;
    for record in metadata.split('\x1e') {
        let fields: Vec<&str> = record.split('\x1f').collect();
        if fields.len() != 12 { return None; }
        let len: usize = fields[11].parse().ok()?;
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
        });
        offset += len;
    }
    (offset == plain_text.len()).then_some(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_single_run() {
        let run = Run { text: "hello".into(), bold: true, size: 24, ..Run::default() };
        let encoded = encode_with_lengths(&[run.clone()]);
        let decoded = decode(&encoded, "hello").unwrap();
        assert_eq!(decoded, vec![run]);
    }

    #[test]
    fn round_trips_multiple_runs_with_multibyte_text() {
        let runs = vec![
            Run { text: "héllo ".into(), bold: true, ..Run::default() },
            Run { text: "world".into(), italic: true, ..Run::default() },
        ];
        let encoded = encode_with_lengths(&runs);
        let plain = "héllo world";
        let decoded = decode(&encoded, plain).unwrap();
        assert_eq!(decoded, runs);
    }

    #[test]
    fn rejects_malformed_metadata() {
        assert_eq!(decode("garbage", "hello"), None);
    }

    #[test]
    fn rejects_metadata_whose_lengths_dont_match_the_text() {
        // "hi" is a different byte length than "hello" (decode only checks
        // total length, not content, so a same-length swap would slip
        // through undetected — that's an inherent, documented limitation,
        // not what this test is exercising).
        let run = Run { text: "hello".into(), ..Run::default() };
        let encoded = encode_with_lengths(&[run]);
        assert_eq!(decode(&encoded, "hi"), None);
    }
}
