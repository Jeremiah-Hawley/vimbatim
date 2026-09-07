use crate::app::error::AppError;
use crate::app::repository::{DocumentRepository, WorkspaceRepository};
use crate::document::DocumentBuffer;
use std::path::{Path, PathBuf};

pub struct DocumentStore;

impl DocumentRepository for DocumentStore {
    // Currently synchronous facades to encapsulate fs interactions.
    // In next steps, these can be made async and scheduled on GPUI background executors.
    fn load_document(
        &self,
        path: &Path,
    ) -> Result<
        (
            Vec<crate::document::Paragraph>,
            crate::docx_parser::DocxOrigin,
        ),
        AppError,
    > {
        crate::docx_parser::parse_docx(path).map_err(|e| AppError::DocumentParse(e.to_string()))
    }

    fn save_new_docx(
        &self,
        paragraphs: &[crate::document::Paragraph],
        path: &Path,
        doc_style: &crate::docx_parser::NewDocStyle,
    ) -> Result<(), AppError> {
        crate::docx_parser::create_new_docx(paragraphs, path, *doc_style)
            .map_err(|e| AppError::DocumentParse(e.to_string()))
    }
}

pub struct WorkspaceFs;

impl WorkspaceRepository for WorkspaceFs {
    fn copy_file(&self, src: &Path, dest: &Path) -> Result<u64, AppError> {
        std::fs::copy(src, dest).map_err(|e| AppError::Workspace(e.to_string()))
    }

    fn create_dir(&self, path: &Path) -> Result<(), AppError> {
        std::fs::create_dir(path).map_err(|e| AppError::Workspace(e.to_string()))
    }

    fn rename(&self, src: &Path, dest: &Path) -> Result<(), AppError> {
        std::fs::rename(src, dest).map_err(|e| AppError::Workspace(e.to_string()))
    }

    fn remove_file(&self, path: &Path) -> Result<(), AppError> {
        std::fs::remove_file(path).map_err(|e| AppError::Workspace(e.to_string()))
    }
}

pub struct SettingsStore;

pub struct RecoveryStore;
