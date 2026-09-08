use crate::app::error::AppError;
use crate::document::Paragraph;
use crate::docx_parser::{DocxOrigin, NewDocStyle};
use std::path::Path;

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

#[cfg(test)]
pub struct InMemoryDocumentRepository {
    pub load_error: AppError,
    pub save_result: Result<(), AppError>,
    pub saved_paths: std::cell::RefCell<Vec<std::path::PathBuf>>,
}

#[cfg(test)]
impl InMemoryDocumentRepository {
    pub fn failing(error: AppError) -> Self {
        Self {
            load_error: error,
            save_result: Ok(()),
            saved_paths: std::cell::RefCell::new(Vec::new()),
        }
    }
}

#[cfg(test)]
impl DocumentRepository for InMemoryDocumentRepository {
    fn load_document(&self, _path: &Path) -> Result<(Vec<Paragraph>, DocxOrigin), AppError> {
        Err(self.load_error.clone())
    }

    fn save_new_docx(
        &self,
        _paragraphs: &[Paragraph],
        path: &Path,
        _doc_style: &NewDocStyle,
    ) -> Result<(), AppError> {
        self.saved_paths.borrow_mut().push(path.to_path_buf());
        self.save_result.clone()
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct InMemoryWorkspaceRepository {
    pub files: std::cell::RefCell<std::collections::HashMap<std::path::PathBuf, Vec<u8>>>,
    pub directories: std::cell::RefCell<std::collections::HashSet<std::path::PathBuf>>,
}

#[cfg(test)]
impl WorkspaceRepository for InMemoryWorkspaceRepository {
    fn copy_file(&self, src: &Path, dest: &Path) -> Result<u64, AppError> {
        let bytes = self
            .files
            .borrow()
            .get(src)
            .cloned()
            .ok_or_else(|| AppError::Workspace(format!("{} does not exist", src.display())))?;
        let len = bytes.len() as u64;
        self.files.borrow_mut().insert(dest.to_path_buf(), bytes);
        Ok(len)
    }

    fn create_dir(&self, path: &Path) -> Result<(), AppError> {
        self.directories.borrow_mut().insert(path.to_path_buf());
        Ok(())
    }

    fn rename(&self, src: &Path, dest: &Path) -> Result<(), AppError> {
        let bytes = self
            .files
            .borrow_mut()
            .remove(src)
            .ok_or_else(|| AppError::Workspace(format!("{} does not exist", src.display())))?;
        self.files.borrow_mut().insert(dest.to_path_buf(), bytes);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), AppError> {
        self.files
            .borrow_mut()
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| AppError::Workspace(format!("{} does not exist", path.display())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_fake_returns_the_configured_failure() {
        let repo = InMemoryDocumentRepository::failing(AppError::DocumentParse("bad zip".into()));
        assert!(matches!(
            repo.load_document(Path::new("broken.docx")),
            Err(AppError::DocumentParse(message)) if message == "bad zip"
        ));
    }

    #[test]
    fn workspace_fake_exercises_file_flow_without_disk() {
        let repo = InMemoryWorkspaceRepository::default();
        let source = Path::new("source.docx");
        let copy = Path::new("copy.docx");
        let renamed = Path::new("renamed.docx");
        repo.files
            .borrow_mut()
            .insert(source.to_path_buf(), vec![1, 2, 3]);

        assert_eq!(repo.copy_file(source, copy).unwrap(), 3);
        repo.rename(copy, renamed).unwrap();
        repo.remove_file(renamed).unwrap();
        assert!(!repo.files.borrow().contains_key(renamed));
    }
}
