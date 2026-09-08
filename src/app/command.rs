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
    SwitchTab(usize),
    Undo,
    Redo,
    ClearToast,
    VimKey {
        key: String,
        shift: bool,
        key_char: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEffect {
    ShowError(String),
    WriteClipboard { text: String, metadata: String },
    DispatchKeybind(crate::keybinds::KeybindAction),
}
