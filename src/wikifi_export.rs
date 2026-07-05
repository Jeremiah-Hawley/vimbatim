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
