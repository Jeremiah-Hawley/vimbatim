use super::MainWindow;
use crate::app::command::AppCommand;
use crate::state::AppState;
use gpui::{AppContext, TestAppContext};

#[gpui::test]
fn save_effects_can_be_requested_inside_a_state_update(cx: &mut TestAppContext) {
    let directory =
        std::env::temp_dir().join(format!("vimbatim-save-effects-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("saved.docx");
    let state = cx.new(|_| AppState::new());

    // Match the global action handlers: effects are submitted while the
    // entity is leased. Before deferral this panics before showing a dialog.
    state.update(cx, |st, cx| {
        st.insert_str("saved text");
        let effects = st.execute(AppCommand::SaveAs);
        MainWindow::handle_app_effects(state.clone(), effects, cx);
    });
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    cx.run_until_parked();
    assert!(path.exists());

    state.update(cx, |st, cx| {
        st.insert_str(" plus edit");
        let effects = st.execute(AppCommand::Save);
        MainWindow::handle_app_effects(state.clone(), effects, cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let st = state.read(cx);
        let tab = &st.workspace().tabs[st.workspace().active_tab];
        assert!(!tab.is_saving);
        assert!(!tab.document.is_modified);
        assert_eq!(tab.file_path.as_ref(), Some(&path));
    });
    let (paragraphs, _) = crate::docx_parser::parse_docx(&path).unwrap();
    assert_eq!(
        crate::docx_parser::paragraphs_to_plain_text(&paragraphs),
        "saved text plus edit"
    );
    std::fs::remove_dir_all(directory).unwrap();
}
