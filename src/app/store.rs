use crate::document::DocumentBuffer;
use std::path::{Path, PathBuf};

pub struct DocumentStore;

impl DocumentStore {
    // Currently synchronous facades to encapsulate fs interactions.
    // In next steps, these can be made async and scheduled on GPUI background executors.
    pub fn load_document(
        path: &Path,
    ) -> Result<
        (
            Vec<crate::document::Paragraph>,
            crate::docx_parser::DocxOrigin,
        ),
        String,
    > {
        crate::docx_parser::parse_docx(path).map_err(|e| e.to_string())
    }

    pub fn save_new_docx(
        paragraphs: &[crate::document::Paragraph],
        path: &Path,
        doc_style: &crate::docx_parser::NewDocStyle,
    ) -> Result<(), String> {
        crate::docx_parser::create_new_docx(paragraphs, path, *doc_style).map_err(|e| e.to_string())
    }
}

pub struct WorkspaceFs;

impl WorkspaceFs {
    pub fn copy_file(src: &Path, dest: &Path) -> std::io::Result<u64> {
        std::fs::copy(src, dest)
    }

    pub fn create_dir(path: &Path) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }

    pub fn rename(src: &Path, dest: &Path) -> std::io::Result<()> {
        std::fs::rename(src, dest)
    }

    pub fn remove_file(path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }
}

pub struct SettingsStore;

pub struct RecoveryStore;
