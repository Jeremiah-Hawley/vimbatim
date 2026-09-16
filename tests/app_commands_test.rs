use vimbatim::testing::*;

#[test]
fn fake_document_failure_reaches_the_open_notification_flow() {
    let mut state = AppState::new();
    let repo = InMemoryDocumentRepository::failing(AppError::DocumentParse("bad zip".into()));
    let result = repo.load_document(std::path::Path::new("broken.docx"));
    state.complete_open_file("broken.docx".into(), result);

    assert!(state
        .notifications()
        .iter()
        .any(|notification| notification.message.contains("bad zip")));
}

#[test]
fn show_error_effect_creates_a_visible_notification() {
    let mut state = AppState::new();
    state.apply_effect(AppEffect::ShowError("save failed".into()));
    assert_eq!(state.notifications().len(), 1);
    assert_eq!(state.notifications()[0].message, "save failed");
    state.execute(AppCommand::ClearToast);
    assert!(state.notifications().is_empty());
}
