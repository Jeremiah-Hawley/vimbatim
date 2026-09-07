use crate::app::error::AppError;
use crate::document::{DocumentBuffer, Paragraph};
use crate::docx_parser::{DocxOrigin, NewDocStyle};
use std::path::{Path, PathBuf};

pub trait DocumentRepository {
    fn load_document(&self, path: &Path) -> Result<(Vec<Paragraph>, DocxOrigin), AppError>;
    fn save_new_docx(
        &self,
        paragraphs: &[Paragraph],
        path: &Path,
        doc_style: &NewDocStyle,
    ) -> Result<(), AppError>;
}

pub trait WorkspaceRepository {
    fn copy_file(&self, src: &Path, dest: &Path) -> Result<u64, AppError>;
    fn create_dir(&self, path: &Path) -> Result<(), AppError>;
    fn rename(&self, src: &Path, dest: &Path) -> Result<(), AppError>;
    fn remove_file(&self, path: &Path) -> Result<(), AppError>;
}

pub trait SettingsRepository {
    // Methods for loading/saving settings
}

pub trait RecoveryRepository {
    // Methods for listing/deleting/writing snapshots
}
