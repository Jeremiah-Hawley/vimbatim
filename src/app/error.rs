use std::fmt;

#[derive(Debug)]
pub enum AppError {
    DocumentParse(String),
    DocumentSave(String),
    Settings(String),
    Workspace(String),
    Font(String),
    Recovery(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::DocumentParse(e) => write!(f, "Failed to parse document: {}", e),
            AppError::DocumentSave(e) => write!(f, "Failed to save document: {}", e),
            AppError::Settings(e) => write!(f, "Settings error: {}", e),
            AppError::Workspace(e) => write!(f, "Workspace file error: {}", e),
            AppError::Font(e) => write!(f, "Font loading error: {}", e),
            AppError::Recovery(e) => write!(f, "Crash recovery error: {}", e),
        }
    }
}

impl std::error::Error for AppError {}
