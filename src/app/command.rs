use crate::document_ops::FormatOp;
use crate::state::CardStyleKind;

#[derive(Debug, Clone, PartialEq)]
pub enum AppCommand {
    ApplyFormatting(FormatOp),
    ApplyCardStyle(CardStyleKind),
    ToggleSidebar,
    SwitchTab(usize),
    Undo,
    Redo,
    ClearToast,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEffect {
    ShowError(String),
}
