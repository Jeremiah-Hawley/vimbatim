use crate::document::TabId;
use crate::document_ops::FormatOp;
use crate::state::CardStyleKind;

#[derive(Debug, Clone, PartialEq)]
pub enum AppCommand {
    ApplyFormatting(FormatOp),
    ApplyCardStyle(CardStyleKind),
    ApplyCiteStyle,
    ApplyAnalyticStyle,
    ApplyEmphasisStyle,
    ClearFormatting,
    ToggleStrikethrough,
    ApplyCaseToSelection(crate::case_converter::CaseType),
    ApplyLineAlignment(crate::docx_parser::Alignment),
    ToggleFold,
    ToggleInvisibilityMode,
    ToggleSidebarMode,
    ToggleSidebar,
    SwitchTab(TabId),
    Undo,
    Redo,
    ClearToast,
    Backspace,
    DeleteForward,
    InsertChar(char),
    IndentListItem,
    OutdentListItem,
    MoveLeft,
    MoveRight,
    ExtendLeft,
    ExtendRight,
    Save,
    SaveAs,
    SaveTab(TabId),
    SaveTabAs(TabId),
    OpenFile,
    OpenFileAt(std::path::PathBuf),
    OpenFileInCurrentTab(std::path::PathBuf),
    OpenFileInSidePane(std::path::PathBuf),
    OpenFolder,
    RefreshFileTree,
    ReopenClosedTab,
    VimKey {
        key: String,
        shift: bool,
        key_char: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEffect {
    ReportError(crate::app::error::AppError),
    ShowError(String),
    WriteClipboard { text: String, metadata: String },
    DispatchKeybind(crate::keybinds::KeybindAction),
    PromptOpenFolder,
    PromptOpenFile,
    LoadDocument(std::path::PathBuf),
    LoadDocumentInCurrentTab(std::path::PathBuf),
    ScanWorkspace(std::path::PathBuf),
    PromptSaveAs(TabId),
    PerformSave(TabId),
}
