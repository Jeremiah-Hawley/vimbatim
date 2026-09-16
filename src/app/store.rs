use crate::app::error::AppError;
use crate::app::repository::{DocumentRepository, WorkspaceRepository};
use crate::state::FileNode;
use std::path::Path;

/// Reject inputs large enough to make a synchronous ZIP parse unsafe. The
/// background migration keeps this boundary and moves the work off the UI.
pub const MAX_DOCUMENT_BYTES: u64 = 128 * 1024 * 1024;

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
        if std::fs::metadata(path)
            .map_err(|e| AppError::DocumentParse(e.to_string()))?
            .len()
            > MAX_DOCUMENT_BYTES
        {
            return Err(AppError::DocumentParse(format!(
                "{} exceeds the {} MiB document limit",
                path.display(),
                MAX_DOCUMENT_BYTES / 1024 / 1024
            )));
        }
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

impl DocumentStore {
    pub fn save_document(
        paragraphs: &[crate::document::Paragraph],
        origin: Option<&crate::docx_parser::DocxOrigin>,
        path: &Path,
        doc_style: crate::docx_parser::NewDocStyle,
    ) -> Result<(), AppError> {
        match origin {
            Some(origin) => origin.save(paragraphs, path),
            None => crate::docx_parser::create_new_docx(paragraphs, path, doc_style),
        }
        .map_err(|e| AppError::DocumentSave(e.to_string()))
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
    fn scan_directory(&self, dir: &Path) -> Result<Vec<FileNode>, AppError> {
        let mut nodes: Vec<FileNode> = Vec::new();

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => return Err(AppError::Workspace(e.to_string())),
        };

        let mut dirs: Vec<FileNode> = Vec::new();
        let mut files: Vec<FileNode> = Vec::new();

        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_dir() {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                // Omit hidden directories (like .git or OS metadata folders)
                if name.starts_with('.') {
                    continue;
                }
                // Recursively scan the subdirectory
                if let Ok(children) = self.scan_directory(&path) {
                    if !children.is_empty() {
                        dirs.push(FileNode::Dir {
                            name,
                            path,
                            children,
                            expanded: false, // populated later by `refresh_file_tree`
                        });
                    }
                }
            } else if path.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
            {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                // Omit backup/temp files that Word and LaTeX leave behind,
                // even if they somehow got a .docx extension.
                if name.starts_with("~$") || name.starts_with('.') {
                    continue;
                }
                files.push(FileNode::File { name, path });
            }
        }

        dirs.sort_by_key(|a| a.name().to_lowercase());
        files.sort_by_key(|a| a.name().to_lowercase());

        nodes.extend(dirs);
        nodes.extend(files);

        Ok(nodes)
    }
}

pub struct SettingsStore {
    path: std::path::PathBuf,
}

impl SettingsStore {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }
}

impl crate::app::repository::SettingsRepository for SettingsStore {
    fn load_preferences(&self) -> Result<crate::preferences::Preferences, AppError> {
        crate::preferences::Preferences::load(&self.path)
            .map_err(|e| AppError::Settings(e.to_string()))
    }

    fn save_preferences(
        &self,
        preferences: &crate::preferences::Preferences,
    ) -> Result<(), AppError> {
        preferences
            .save(&self.path)
            .map_err(|e| AppError::Settings(e.to_string()))
    }
}

pub struct RecoveryStore;

impl crate::app::repository::RecoveryRepository for RecoveryStore {
    fn list_entries(&self) -> Result<Vec<crate::recovery::RecoveryEntry>, AppError> {
        Ok(crate::recovery::scan_recovery_dir(
            &crate::recovery::recovery_dir(),
        ))
    }

    fn write_snapshot(&self, snapshot: &crate::state::TabSnapshot) -> Result<(), AppError> {
        crate::recovery::write_snapshot(
            snapshot.id,
            &snapshot.paragraphs,
            snapshot.origin.as_deref(),
            snapshot.file_path.as_deref(),
            &snapshot.title,
            snapshot.doc_style,
        )
        .map(|_| ())
        .map_err(|e| AppError::Recovery(e.to_string()))
    }

    fn delete_entry(&self, entry: &crate::recovery::RecoveryEntry) -> Result<(), AppError> {
        crate::recovery::delete_entry(entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_store_rejects_files_over_the_limit() {
        let path = std::env::temp_dir().join(format!("vimbatim-too-large-{}", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_DOCUMENT_BYTES + 1).unwrap();

        let error = DocumentStore.load_document(&path).unwrap_err();
        assert!(error.to_string().contains("document limit"));
        let _ = std::fs::remove_file(path);
    }
}
