use crate::docx_parser::Paragraph;

pub fn export_to_markdown(paragraphs: &[Paragraph], content: &str) -> String {
    /*
     * Converts document to markdown with heading hierarchy:
     * Pockets → H1 (#), Hats → H2 (##), Blocks → H3 (###), Tags → H4 (####)
     * All other formatting ignored.
     */
    let lines = content.split('\n');
    let mut markdown = String::new();

    for (para_idx, line) in lines.enumerate() {
        if let Some(para) = paragraphs.get(para_idx) {
            match para.heading {
                1 => markdown.push_str(&format!("# {}\n", line)),
                2 => markdown.push_str(&format!("## {}\n", line)),
                3 => markdown.push_str(&format!("### {}\n", line)),
                4 => markdown.push_str(&format!("#### {}\n", line)),
                _ => markdown.push_str(&format!("{}\n", line)),
            }
        } else {
            markdown.push_str(&format!("{}\n", line));
        }
    }
    markdown
}

pub fn save_markdown_file(path: &std::path::PathBuf, markdown: &str) -> std::io::Result<()> {
    /*
     * Saves markdown string to file with .md extension.
     */
    use std::fs;

    let md_path = path.with_extension("md");
    fs::write(md_path, markdown)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx_parser::{Alignment, Run};

    /// Direct, dependency-free check of the export function itself — the
    /// end-to-end version (building the document through the real
    /// `AppState::apply_card_style`, proving the whole ribbon/keybind →
    /// export pipeline works now that `apply_card_style` sets
    /// `Paragraph.heading`) lives in `state.rs`, where the hermetic
    /// `make_state_with_paragraphs` test helper already exists.
    #[test]
    fn export_to_markdown_maps_heading_1_through_4_and_leaves_body_text_alone() {
        let para = |heading: u8, text: &str| Paragraph {
            runs: vec![Run { text: text.to_string(), ..Default::default() }],
            heading,
            alignment: Alignment::default(),
        unsupported_xml: None,
    };
        let paragraphs = vec![
            para(1, "Case Title"),
            para(2, "Off-case Subtitle"),
            para(3, "Block heading"),
            para(4, "Tag text"),
            para(0, "plain body text"),
        ];
        let content = "Case Title\nOff-case Subtitle\nBlock heading\nTag text\nplain body text";

        let markdown = export_to_markdown(&paragraphs, content);

        assert_eq!(
            markdown,
            "# Case Title\n## Off-case Subtitle\n### Block heading\n#### Tag text\nplain body text\n"
        );
    }
}
