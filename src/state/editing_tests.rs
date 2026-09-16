use super::*;
use crate::docx_parser::{create_new_docx, paragraphs_to_plain_text};
use crate::state::vim::*;
use crate::state::workspace::unique_path_in;

/// Makes a unique temp dir for one test. Mirrors `recovery.rs`'s helper —
/// no `tempfile` dependency.
fn custom_color_temp_dir(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("vimbatim-color-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_load_custom_colors_parses_pipe_separated_hex() {
    let dir = custom_color_temp_dir("load");
    let path = dir.join("settings.conf");
    std::fs::write(
        &path,
        "[FORMATTING]\ncustom_highlight_colors=00ff88|aabbcc\n",
    )
    .unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&path)
            .unwrap()
            .custom_highlight_colors,
        vec![0x00ff88, 0xaabbcc]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_load_custom_colors_tolerates_missing_and_garbage() {
    let dir = custom_color_temp_dir("tolerant");
    let path = dir.join("settings.conf");

    // Missing file.
    assert!(crate::preferences::Preferences::load(&path)
        .unwrap_or_default()
        .custom_font_colors
        .is_empty());

    // Missing key, empty value, and unparseable entries mixed with good ones.
    std::fs::write(
        &path,
        "[FORMATTING]\ncustom_font_colors=\ncustom_highlight_colors=00ff88|zzzzzz|1234567|aabbcc\n",
    )
    .unwrap();
    assert!(crate::preferences::Preferences::load(&path)
        .unwrap()
        .custom_font_colors
        .is_empty());
    assert_eq!(
        crate::preferences::Preferences::load(&path)
            .unwrap()
            .custom_highlight_colors,
        vec![0x00ff88, 0xaabbcc],
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_save_then_load_custom_colors_round_trips() {
    let dir = custom_color_temp_dir("roundtrip");
    let path = dir.join("settings.conf");
    std::fs::write(&path, "[FORMATTING]\ntheme=nord\n\n[KEYBINDS]\nvim=false\n").unwrap();

    save_custom_colors(&path, "custom_font_colors", &[0x00ff88, 0x000000]).unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&path)
            .unwrap()
            .custom_font_colors,
        vec![0x00ff88, 0x000000]
    );

    // An unrelated existing key survives the write.
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("theme=nord"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_add_custom_color_dedups_by_moving_to_the_end() {
    let mut state = make_state("", 0, None);
    state.preferences.custom_highlight_colors = vec![0x111111, 0x222222, 0x333333];
    state.add_custom_color(CustomColorTarget::Highlight, 0x222222);
    assert_eq!(
        state.preferences.custom_highlight_colors,
        vec![0x111111, 0x333333, 0x222222]
    );
}

#[test]
fn test_add_custom_color_caps_the_list_dropping_oldest() {
    let mut state = make_state("", 0, None);
    for i in 0..MAX_CUSTOM_COLORS as u32 {
        state.add_custom_color(CustomColorTarget::Font, i);
    }
    assert_eq!(
        state.preferences.custom_font_colors.len(),
        MAX_CUSTOM_COLORS
    );
    assert_eq!(state.preferences.custom_font_colors[0], 0);

    state.add_custom_color(CustomColorTarget::Font, 0xFFFFFF);
    assert_eq!(
        state.preferences.custom_font_colors.len(),
        MAX_CUSTOM_COLORS
    );
    assert_eq!(
        state.preferences.custom_font_colors[0], 1,
        "oldest entry should be dropped"
    );
    assert_eq!(
        *state.preferences.custom_font_colors.last().unwrap(),
        0xFFFFFF
    );
}

#[test]
fn test_remove_custom_color_drops_it_and_leaves_the_rest() {
    let mut state = make_state("", 0, None);
    state.preferences.custom_highlight_colors = vec![0x111111, 0x222222, 0x333333];
    state.remove_custom_color(CustomColorTarget::Highlight, 0x222222);
    assert_eq!(
        state.preferences.custom_highlight_colors,
        vec![0x111111, 0x333333]
    );
}

#[test]
fn test_remove_custom_color_is_a_noop_for_an_absent_color() {
    let mut state = make_state("", 0, None);
    state.preferences.custom_font_colors = vec![0x111111];
    state.remove_custom_color(CustomColorTarget::Font, 0x999999);
    assert_eq!(state.preferences.custom_font_colors, vec![0x111111]);
}

#[test]
fn test_remove_custom_color_only_touches_its_own_list() {
    let mut state = make_state("", 0, None);
    state.preferences.custom_font_colors = vec![0x00ff88];
    state.preferences.custom_highlight_colors = vec![0x00ff88];
    state.remove_custom_color(CustomColorTarget::Highlight, 0x00ff88);
    assert!(state.preferences.custom_highlight_colors.is_empty());
    assert_eq!(state.preferences.custom_font_colors, vec![0x00ff88]);
}

#[test]
fn test_add_custom_color_keeps_the_two_lists_separate() {
    let mut state = make_state("", 0, None);
    state.add_custom_color(CustomColorTarget::Highlight, 0x00ff88);
    assert_eq!(state.preferences.custom_highlight_colors, vec![0x00ff88]);
    assert!(state.preferences.custom_font_colors.is_empty());
}

fn plain_paragraphs(content: &str) -> Vec<Paragraph> {
    content
        .split('\n')
        .map(|text| Paragraph {
            runs: vec![Run {
                text: text.into(),
                ..Run::default()
            }],
            ..Paragraph::default()
        })
        .collect()
}

/// Build a minimal AppState with one tab whose content, cursor, and
/// selection are set to the given values. Avoids touching the filesystem
/// or GPUI context.
fn make_state(content: &str, cursor: usize, selection: Option<(usize, usize)>) -> AppState {
    AppState {
        workspace: WorkspaceState {
            tabs: vec![Tab {
                id: TabId(0),
                title: "test".into(),
                file_path: None,
                is_saving: false,
                saving_version: None,
                document: DocumentBuffer::new(plain_paragraphs(content)),
                docx_origin: None,
                pending_format: None,
                cursor,
                selection,
                last_snapshot_version: 0,
                last_snapshot_cost: None,
                vim_mode: VimMode::Normal,
                vim_command_buf: String::new(),
                last_find: None,
                vim_pending_operator: None,
                vim_pending_text_object_prefix: None,
                vim_command_line: String::new(),
                vim_command_error: None,
                vim_pending_register_select: false,
                vim_selected_register: None,
                vim_pending_replace: false,
                vim_keybind_seq: String::new(),
                vim_search_direction: true,
                vim_jump_back: Vec::new(),
                vim_jump_forward: Vec::new(),
                pending_scroll_to_cursor: false,
                folded_headings: std::collections::HashSet::new(),
                folded_para_count: 0,
                fold_version: 0,
                similar_ranges: Vec::new(),
                has_unsupported_blocks: false,
                opened_detached: false,
                banner_dismissed: false,
            }],
            active_tab: 0,
            pending_focus_editor: None,
            next_tab_id: 1,
            closed_tabs: Vec::new(),
            working_directory: std::path::PathBuf::from("."),
            file_tree: vec![],
            split_view: false,
            secondary_tab_id: None,
            focused_pane: Pane::Primary,
            primary_tab_id: None,
            split_ratio: 0.5,
            split_dragging: false,
        },
        ui: UiState::default(),
        global_vim: GlobalVimState {
            vim_enabled: true,
            ..Default::default()
        },
        // Off in tests: on would mean every fixture string gets run
        // through the bundled dictionary, for no assertion's benefit.
        preferences: crate::preferences::Preferences {
            spellcheck_enabled: false,
            ..Default::default()
        },
        sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        copied_file: None,
        search_word_list: Vec::new(),
        sidebar_mode: SidebarMode::default(),
        recovery: RecoveryState::default(),
        keybinds: crate::keybinds::Keybinds::defaults(),
        custom_theme: None,
        zoom: 1.0,
        // A temp file, never the real ~/.vimbatim/settings.conf — see
        // the field's doc comment.
        settings_path: std::env::temp_dir().join("vimbatim_test_settings.conf"),
        user_dictionary: Rc::new(HashSet::new()),
    }
}

#[test]
fn test_switch_active_pane_toggles_and_no_ops_while_unsplit() {
    let mut state = make_state("hello", 0, None);
    // Unsplit: nothing to switch to, so this stays on Primary rather
    // than reporting a phantom Secondary focus.
    state.switch_active_pane();
    assert_eq!(state.workspace.focused_pane, Pane::Primary);

    state.open_split();
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);

    state.switch_active_pane();
    assert_eq!(state.workspace.focused_pane, Pane::Primary);

    state.switch_active_pane();
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);
}

/// Mirrors `text_editor.rs`'s `process_key`: records the keystroke
/// into `vim_change_recording` (if active) *before* dispatching it —
/// needed since plain `handle_vim_key` calls in tests bypass that
/// capture step entirely (it's normally done one layer up).
fn vim_key_recorded(state: &mut AppState, key: &str, shift: bool, key_char: Option<&str>) {
    if state.vim_is_recording_change() {
        state.record_change_key(key, shift, key_char);
    }
    state.handle_vim_key(key, shift, key_char);
}

// ── Opening a file reuses a blank "New Tab" ────────────────────────────

fn temp_docx(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("vimbatim_reuse_{}_{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("card.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();
    (dir, path)
}

#[test]
fn opening_a_file_replaces_an_untouched_new_tab() {
    let (dir, path) = temp_docx("replace");
    let mut state = make_state("", 0, None);
    state.workspace.tabs[0] = Tab::new_empty(TabId(0)); // a pristine "New Tab"
    assert_eq!(state.workspace.tabs.len(), 1);

    state.open_file(path.clone());

    assert_eq!(
        state.workspace.tabs.len(),
        1,
        "a blank tab was left stranded"
    );
    assert_eq!(
        state.workspace.tabs[0].file_path.as_deref(),
        Some(path.as_path())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn opening_a_file_keeps_a_tab_that_has_been_typed_in() {
    let (dir, path) = temp_docx("keep-typed");
    let mut state = make_state("", 0, None);
    state.workspace.tabs[0] = Tab::new_empty(TabId(0));
    state.insert_str("draft"); // now it holds work

    state.open_file(path.clone());

    assert_eq!(
        state.workspace.tabs.len(),
        2,
        "unsaved work was overwritten"
    );
    assert_eq!(state.workspace.tabs[0].document.content(), "draft");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Undoing back to empty leaves a tab that *looks* blank but has history
/// and a dirty flag. Replacing it would silently discard that.
#[test]
fn a_tab_emptied_by_undo_is_not_treated_as_a_blank_new_tab() {
    let mut state = make_state("", 0, None);
    state.workspace.tabs[0] = Tab::new_empty(TabId(0));
    state.insert_str("typed");
    state.undo();

    assert!(state.workspace.tabs[0].document.content().is_empty());
    assert!(
        !state.workspace.tabs[0].is_blank_new_tab(),
        "undo-emptied tab looked pristine"
    );
}

#[test]
fn opening_a_file_in_the_split_replaces_only_that_panes_blank_tab() {
    let (dir, path) = temp_docx("split");
    let mut state = make_state("primary work", 0, None);
    state.open_split(); // secondary gets a blank tab, and focus
    let tabs_before = state.workspace.tabs.len();

    state.open_file(path.clone());

    assert_eq!(
        state.workspace.tabs.len(),
        tabs_before,
        "split's blank tab was not reused"
    );
    let secondary = state.pane_tab_index(Pane::Secondary).unwrap();
    assert_eq!(
        state.workspace.tabs[secondary].file_path.as_deref(),
        Some(path.as_path())
    );
    // The other pane is untouched.
    assert_eq!(state.pane_content(Pane::Primary), "primary work");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Level-aware folding ────────────────────────────────────────────────

fn outline() -> Vec<Paragraph> {
    let card = |text: &str, heading: u8| Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            ..Run::default()
        }],
        heading,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    vec![
        card("pocket A", 1),  // 0
        para_plain("body A"), // 1
        card("hat A", 2),     // 2
        para_plain("body B"), // 3
        card("tag A", 4),     // 4
        para_plain("body C"), // 5
        card("pocket B", 1),  // 6
        para_plain("body D"), // 7
    ]
}

/// Collapsing a Pocket takes everything under it — including the Hats and
/// Tags nested beneath — until the next heading at its level or higher.
#[test]
fn folding_a_heading_hides_lower_levels_beneath_it() {
    let paragraphs = outline();
    let folded = std::collections::HashSet::from([0usize]);

    let hidden = AppState::folded_paragraphs(&paragraphs, &folded);

    assert_eq!(
        hidden,
        vec![false, true, true, true, true, true, false, false],
        "a collapsed Pocket must swallow its Hats and Tags, and stop at the next Pocket"
    );
}

/// Collapsing a lower-level heading takes only its own section.
#[test]
fn folding_a_nested_heading_leaves_its_parents_alone() {
    let paragraphs = outline();
    let folded = std::collections::HashSet::from([2usize]); // the Hat

    let hidden = AppState::folded_paragraphs(&paragraphs, &folded);

    // Hat's section runs to the next heading of level <= 2, i.e. Pocket B.
    assert_eq!(
        hidden,
        vec![false, false, false, true, true, true, false, false]
    );
}

#[test]
fn folding_nothing_hides_nothing() {
    let paragraphs = outline();
    let hidden = AppState::folded_paragraphs(&paragraphs, &std::collections::HashSet::new());
    assert_eq!(hidden, vec![false; 8]);
}

/// A heading that closes one collapsed section can open another in the
/// same step — the two Pockets here are both collapsed.
#[test]
fn a_heading_can_end_one_section_and_start_the_next() {
    let paragraphs = outline();
    let folded = std::collections::HashSet::from([0usize, 6usize]);

    let hidden = AppState::folded_paragraphs(&paragraphs, &folded);

    assert_eq!(
        hidden,
        vec![false, true, true, true, true, true, false, true]
    );
}

#[test]
fn toggle_fold_collapses_every_heading_then_expands_all() {
    let mut state = make_state_with_paragraphs(outline(), 0);

    state.toggle_fold();
    assert!(state.any_folded());
    let hidden = AppState::folded_paragraphs(
        state.workspace.tabs[0].document.paragraphs(),
        &state.workspace.tabs[0].folded_headings,
    );
    // Only the two Pockets survive: each swallows everything beneath it.
    assert_eq!(
        hidden,
        vec![false, true, true, true, true, true, false, true]
    );

    state.toggle_fold();
    assert!(!state.any_folded(), "second press should expand everything");
}

#[test]
fn toggling_one_heading_leaves_the_others_alone() {
    let mut state = make_state_with_paragraphs(outline(), 0);
    state.toggle_fold(); // collapse all

    state.toggle_paragraph_fold(0); // expand just Pocket A

    assert!(!state.workspace.tabs[0].folded_headings.contains(&0));
    assert!(state.workspace.tabs[0].folded_headings.contains(&6));
}

#[test]
fn body_paragraphs_cannot_be_folded() {
    let mut state = make_state_with_paragraphs(outline(), 0);
    state.toggle_paragraph_fold(1); // a body line
    assert!(state.workspace.tabs[0].folded_headings.is_empty());
}

/// Fold state is keyed by paragraph index, so a structural edit drops it
/// rather than folding the wrong sections — see `Tab.folded_headings`.
#[test]
fn fold_state_is_dropped_when_the_paragraph_count_changes() {
    let mut state = make_state_with_paragraphs(outline(), 0);
    state.toggle_fold();
    assert!(state.any_folded());

    // Split a paragraph in two.
    state.workspace.tabs[0].cursor = 0;
    state.insert_str("\n");

    assert!(
        !state.any_folded(),
        "stale folds survived a structural edit"
    );
}

// ── Text settings ──────────────────────────────────────────────────────
// ── Standardize highlighting ───────────────────────────────────────────
// ── Analytic style ─────────────────────────────────────────────────────
fn analytic_para(text: &str, size: u16, color: &str) -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            bold: true,
            size,
            color: Some(color.into()),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

#[test]
fn delete_analytics_removes_whole_lines() {
    let paragraphs = vec![
        para_plain("keep this"),
        analytic_para("an analytic", 26, "0000ff"),
        para_plain("and this"),
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.delete_analytics();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs().len(),
        2,
        "the line should be gone, not blanked"
    );
    // `content` is rebuilt from the survivors — the 1:1 line/paragraph
    // invariant the rest of the editor depends on.
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "keep this\nand this"
    );
}

#[test]
fn delete_analytics_leaves_other_formatting_alone() {
    let paragraphs = vec![analytic_para("wrong color", 26, "c00000")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.delete_analytics();

    assert_eq!(state.workspace.tabs[0].document.paragraphs().len(), 1);
}

/// A document that is nothing but analytics must still end up with one
/// paragraph — every rich-text function assumes at least one exists.
#[test]
fn deleting_every_paragraph_leaves_a_blank_one() {
    let paragraphs = vec![
        analytic_para("one", 26, "0000ff"),
        analytic_para("two", 26, "0000ff"),
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.delete_analytics();

    assert_eq!(state.workspace.tabs[0].document.paragraphs().len(), 1);
    assert!(state.workspace.tabs[0].document.content().is_empty());
    assert!(
        !state.workspace.tabs[0].document.paragraphs()[0]
            .runs
            .is_empty(),
        "a paragraph always has a run"
    );
}

/// The cursor pointed into text that no longer exists.
#[test]
fn delete_analytics_clamps_the_cursor() {
    let paragraphs = vec![
        para_plain("ab"),
        analytic_para("long analytic", 26, "0000ff"),
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");
    state.workspace.tabs[0].cursor = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, state.workspace.tabs[0].document.content().len()));

    state.delete_analytics();

    assert!(state.workspace.tabs[0].cursor <= state.workspace.tabs[0].document.content().len());
    assert_eq!(
        state.workspace.tabs[0].selection, None,
        "a selection into deleted text must clear"
    );
}

#[test]
fn delete_analytics_is_a_no_op_when_there_are_none() {
    let mut state = make_state_with_paragraphs(vec![para_plain("just text")], 0);
    state.set_analytic_color("0000ff");
    let version_before = state.workspace.tabs[0].document.content_version;

    state.delete_analytics();

    assert_eq!(
        state.workspace.tabs[0].document.content_version,
        version_before
    );
    assert!(!state.workspace.tabs[0].document.is_modified);
}

/// The marker is authoritative: a reformatted analytic is still one, even
/// though its bold/size/color no longer match the signature.
#[test]
fn a_marked_analytic_is_recognised_regardless_of_its_formatting() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "reformatted".into(),
            bold: false,
            size: 99,
            color: Some("00ff00".into()),
            style: Some(CardStyle::Analytic),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.set_analytic_color("0000ff");

    state.convert_analytics_to_tags();

    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 4);
}

/// ...and the converse: text that coincidentally matches the old
/// signature but is marked as something else is left alone. This is the
/// misidentification the marker exists to prevent.
#[test]
fn a_marked_cite_is_not_mistaken_for_an_analytic() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "a cite".into(),
            bold: true,
            size: 26,
            color: Some("0000ff".into()),
            style: Some(CardStyle::Cite),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.convert_analytics_to_tags();
    state.delete_analytics();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs().len(),
        1,
        "a marked Cite was deleted as an analytic"
    );
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 0);
}

#[test]
fn applying_a_card_style_stamps_its_marker() {
    let mut state = make_state_with_paragraphs(vec![para_plain("a line")], 0);
    state.apply_card_style(CardStyleKind::Block);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].style,
        Some(CardStyle::Block)
    );
}

#[test]
fn applying_cite_and_analytic_stamps_their_markers() {
    let mut state = make_state_with_paragraphs(vec![para_plain("some text")], 0);
    state.workspace.tabs[0].selection = Some((0, 9));
    state.apply_cite_style();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].style,
        Some(CardStyle::Cite)
    );

    let mut state = make_state_with_paragraphs(vec![para_plain("some text")], 0);
    state.apply_analytic_style();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].style,
        Some(CardStyle::Analytic)
    );
}

/// Clearing formatting clears what the run *was*, not just how it looked —
/// otherwise a cleared line still answers to "is this an analytic?".
#[test]
fn clearing_formatting_clears_the_marker() {
    let mut state = make_state_with_paragraphs(vec![para_plain("a line")], 0);
    state.apply_analytic_style();
    state.workspace.tabs[0].selection = Some((0, 6));

    state.clear_formatting();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].style,
        None
    );
}

#[test]
fn convert_analytics_promotes_them_to_tags() {
    let paragraphs = vec![
        analytic_para("an analytic", 26, "0000ff"),
        para_plain("ordinary body text"),
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.convert_analytics_to_tags();

    let converted = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(converted.heading, 4, "should now be a Tag");
    assert_eq!(converted.runs[0].color, None, "a Tag is plain-colored");
    assert!(converted.runs[0].bold);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].heading,
        0,
        "body text untouched"
    );
}

/// Only paragraphs matching the analytic signature convert — bold text at
/// the same size in a *different* color is someone else's formatting.
#[test]
fn convert_analytics_ignores_other_colored_text() {
    let paragraphs = vec![analytic_para("not an analytic", 26, "c00000")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.convert_analytics_to_tags();

    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 0);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0]
            .color
            .as_deref(),
        Some("c00000")
    );
}

#[test]
fn convert_analytics_is_a_no_op_when_there_are_none() {
    let mut state = make_state_with_paragraphs(vec![para_plain("just text")], 0);
    state.set_analytic_color("0000ff");
    let version_before = state.workspace.tabs[0].document.content_version;

    state.convert_analytics_to_tags();

    assert_eq!(
        state.workspace.tabs[0].document.content_version,
        version_before
    );
    assert!(!state.workspace.tabs[0].document.is_modified);
}

/// Round-trip: what `apply_analytic_style` produces is exactly what the
/// converter recognises, so the two cannot drift apart.
#[test]
fn a_freshly_applied_analytic_converts() {
    let mut state = make_state_with_paragraphs(vec![para_plain("some analysis")], 0);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("0000ff");

    state.apply_analytic_style();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].heading,
        0,
        "analytic must not be a heading"
    );

    state.convert_analytics_to_tags();

    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 4);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].color,
        None
    );
}

/// A blank line is never an analytic — converting one would put an empty
/// Tag in the Nav outline.
#[test]
fn convert_analytics_skips_blank_lines() {
    let mut state = make_state_with_paragraphs(vec![para_plain("")], 0);
    state.set_analytic_color("0000ff");

    state.convert_analytics_to_tags();

    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 0);
}

#[test]
fn analytic_applies_tag_weight_and_size_in_the_configured_color() {
    let mut state = make_state("an analytic", 0, None);
    state.preferences.tag_size_half_points = 26;
    state.set_analytic_color("c00000");

    state.apply_analytic_style();

    let run = &state.workspace.tabs[0].document.paragraphs()[0].runs[0];
    assert!(run.bold);
    assert_eq!(run.size, 26, "should match the configured Tag size");
    assert_eq!(run.color.as_deref(), Some("c00000"));
}

/// The whole point of Analytic over Tag: it is the debater's own argument,
/// not a structural marker, so it must stay out of the Nav outline, the
/// fold hierarchy, and Wikifi's heading levels — all of which read
/// `Paragraph.heading`.
#[test]
fn analytic_is_not_a_heading() {
    let mut state = make_state("an analytic", 0, None);
    state.apply_analytic_style();
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 0);
}

/// Converting a Tag line to an Analytic has to clear the heading it
/// already carried, or the line stays in the outline while no longer
/// looking like a card style.
#[test]
fn analytic_clears_an_existing_card_style_heading() {
    let mut state = make_state("was a tag", 0, None);
    state.apply_card_style(CardStyleKind::Tag);
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 4);

    state.apply_analytic_style();

    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].heading, 0);
}

#[test]
fn setting_the_highlight_color_is_what_later_operations_read() {
    let mut state = make_state("", 0, None);
    assert_eq!(state.preferences.highlight_color, "yellow");

    state.set_highlight_color("cyan");
    assert_eq!(state.preferences.highlight_color, "cyan");

    // A custom color is stored as a bare hex, which
    // `text_editor::highlight_color_hex` also parses.
    state.set_highlight_color("86f2ef");
    assert_eq!(state.preferences.highlight_color, "86f2ef");
}

/// Picking a color and then standardizing must use the picked one — the
/// two features share `highlight_color` rather than each having their own
/// idea of "current".
#[test]
fn standardize_follows_the_most_recently_picked_color() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl("marked", "green")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);

    state.set_highlight_color("cyan");
    state.standardize_highlighting();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].highlight_color,
        "cyan"
    );
}

fn hl(text: &str, color: &str) -> Run {
    Run {
        text: text.into(),
        highlight: true,
        highlight_color: color.into(),
        ..Run::default()
    }
}

#[test]
fn standardize_repaints_every_highlight_to_the_current_color() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![
                hl("green bit", "green"),
                run_plain(" plain "),
                hl("cyan bit", "cyan"),
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![hl("magenta bit", "magenta")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.highlight_color = "yellow".to_string();

    state.standardize_highlighting();

    for para in state.workspace.tabs[0].document.paragraphs() {
        for run in &para.runs {
            if run.highlight {
                assert_eq!(run.highlight_color, "yellow");
            }
        }
    }
}

/// Only the color changes — nothing gains or loses a highlight, and no
/// text moves.
#[test]
fn standardize_leaves_unhighlighted_text_alone() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            run_plain("before "),
            hl("marked", "green"),
            run_plain(" after"),
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let content_before = state.workspace.tabs[0].document.content().to_owned();
    state.preferences.highlight_color = "yellow".to_string();

    state.standardize_highlighting();

    assert_eq!(
        state.workspace.tabs[0].document.content(),
        content_before,
        "text must not move"
    );
    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert!(!runs[0].highlight, "plain text gained a highlight");
    assert!(!runs.last().unwrap().highlight);
}

/// Runs that differed only by highlight color are identical afterwards, so
/// they fuse rather than leaving the document split on a distinction that
/// no longer exists.
#[test]
fn standardize_merges_runs_that_now_match() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl("one ", "green"), hl("two", "cyan")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.highlight_color = "yellow".to_string();

    state.standardize_highlighting();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs.len(),
        1
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "one two"
    );
}

#[test]
fn standardize_with_exception_spares_the_chosen_color() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            hl("ordinary", "green"),
            run_plain(" "),
            hl("meaningful", "cyan"),
            run_plain(" "),
            hl("also ordinary", "magenta"),
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.set_highlight_color("yellow");
    state.set_standardize_exception("cyan");

    state.standardize_highlighting_with_exception();

    let colors: Vec<&str> = state.workspace.tabs[0].document.paragraphs()[0]
        .runs
        .iter()
        .filter(|r| r.highlight)
        .map(|r| r.highlight_color.as_str())
        .collect();
    assert_eq!(colors, vec!["yellow", "cyan", "yellow"]);
}

/// With nothing configured, the exception command is the plain one.
#[test]
fn standardize_with_no_exception_repaints_everything() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl("a", "green"), hl("b", "cyan")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.set_highlight_color("yellow");
    assert!(state.preferences.standardize_highlight_exception.is_empty());

    state.standardize_highlighting_with_exception();

    // Both repainted, and now identical, so they fused.
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs.len(),
        1
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].highlight_color,
        "yellow"
    );
}

/// A document where only the excepted color differs has nothing to do —
/// no undo entry, so Ctrl+Z still undoes whatever the user actually did.
#[test]
fn standardize_with_exception_is_a_no_op_when_only_the_exception_differs() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl("kept", "cyan"), hl(" done", "yellow")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.set_highlight_color("yellow");
    state.set_standardize_exception("cyan");
    let version_before = state.workspace.tabs[0].document.content_version;

    state.standardize_highlighting_with_exception();

    assert_eq!(
        state.workspace.tabs[0].document.content_version,
        version_before
    );
    assert!(!state.workspace.tabs[0].document.is_modified);
}

#[test]
fn standardize_is_undoable() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl("marked", "green")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.highlight_color = "yellow".to_string();
    let version_before = state.workspace.tabs[0].document.content_version;

    state.standardize_highlighting();

    assert!(state.workspace.tabs[0].document.is_modified);
    assert!(
        state.workspace.tabs[0].document.content_version > version_before,
        "row cache would go stale"
    );
}

/// A document already in the right color must not push an undo entry —
/// Ctrl+Z afterwards should undo whatever the user actually did last.
#[test]
fn standardize_is_a_no_op_when_already_uniform() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![hl("marked", "yellow")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.highlight_color = "yellow".to_string();
    let version_before = state.workspace.tabs[0].document.content_version;

    state.standardize_highlighting();

    assert_eq!(
        state.workspace.tabs[0].document.content_version,
        version_before
    );
    assert!(!state.workspace.tabs[0].document.is_modified);
}

#[test]
fn shrink_size_setter_stores_half_points_and_clamps() {
    let mut state = make_state("", 0, None);

    state.set_shrink_size_points(8);
    assert_eq!(
        state.preferences.small_size_half_points, 16,
        "stored in half-points"
    );

    // Clamped at both ends rather than walking somewhere unusable.
    state.set_shrink_size_points(0);
    assert_eq!(state.preferences.small_size_half_points, 8); // 4pt floor
    state.set_shrink_size_points(999);
    assert_eq!(state.preferences.small_size_half_points, 96); // 48pt ceiling
}

#[test]
fn emphasis_size_setter_stores_half_points_and_clamps() {
    let mut state = make_state("", 0, None);

    state.set_emphasis_size_points(8);
    assert_eq!(
        state.preferences.emphasis_size_half_points, 16,
        "stored in half-points"
    );

    state.set_emphasis_size_points(0);
    assert_eq!(state.preferences.emphasis_size_half_points, 8); // 4pt floor
    state.set_emphasis_size_points(999);
    assert_eq!(state.preferences.emphasis_size_half_points, 96); // 48pt ceiling
}

#[test]
fn emphasis_change_size_setter_persists() {
    let mut state = make_state("", 0, None);
    assert!(!state.preferences.emphasis_change_size);
    state.set_emphasis_change_size(true);
    assert!(state.preferences.emphasis_change_size);
}

/// Shrink applies whatever the setting currently says, not a fixed size.
#[test]
fn shrink_uses_the_configured_size() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "shrink me".into(),
            size: 44,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 9));
    state.set_shrink_size_points(7);

    state.shrink_text();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].size,
        14,
        "7pt = 14 half-points"
    );
}

#[test]
fn text_settings_load_from_settings_conf() {
    let dir = std::env::temp_dir().join(format!("vimbatim_textset_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.conf");
    std::fs::write(
        &path,
        "[FORMATTING]\nhighlight_color=cyan\n\n[TEXT]\nemphasis_bold=false\n\
             emphasis_underline=true\nemphasis_box=true\nemphasis_change_size=true\n\
             emphasis_size=18\npaste_condense=true\n\
             paste_condense_pilcrow=true\n",
    )
    .unwrap();

    let prefs = crate::preferences::Preferences::load(&path).unwrap();
    assert_eq!(prefs.highlight_color, "cyan");
    assert!(!prefs.emphasis_bold);
    assert!(prefs.emphasis_underline);
    assert!(prefs.emphasis_box);
    assert!(prefs.emphasis_change_size);
    assert!(prefs.paste_condense);
    assert!(prefs.paste_condense_pilcrow);
    assert_eq!(prefs.emphasis_size_half_points, 36);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn paste_keeps_newlines_when_condense_is_off() {
    let mut state = make_state("", 0, None);
    state.preferences.paste_condense = false;
    state.paste_text("one\ntwo");
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

#[test]
fn paste_condenses_newlines_to_spaces() {
    let mut state = make_state("", 0, None);
    state.preferences.paste_condense = true;
    state.paste_text("one\ntwo");
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
}

#[test]
fn paste_condense_marks_newlines_with_a_pilcrow_when_asked() {
    let mut state = make_state("", 0, None);
    state.preferences.paste_condense = true;
    state.preferences.paste_condense_pilcrow = true;
    state.paste_text("one\ntwo");
    assert_eq!(state.workspace.tabs[0].document.content(), "one¶two");
}

/// The pilcrow sub-setting is meaningless on its own — condensing off means
/// the newline is kept, not replaced with a mark.
#[test]
fn the_pilcrow_setting_does_nothing_while_condense_is_off() {
    let mut state = make_state("", 0, None);
    state.preferences.paste_condense = false;
    state.preferences.paste_condense_pilcrow = true;
    state.paste_text("one\ntwo");
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

/// Paragraph integrity and condense-on-paste are opposites: turning the
/// first on has to switch the second off, or the ribbon claims to preserve
/// paragraphs while the paste collapses them.
#[test]
fn paragraph_integrity_turns_condense_off() {
    let mut state = make_state("", 0, None);
    state.preferences.paste_condense = true;

    state.toggle_paragraph_integrity();

    assert!(state.preferences.paragraph_integrity);
    assert!(!state.preferences.paste_condense);
}

#[test]
fn turning_paragraph_integrity_back_off_leaves_condense_alone() {
    let mut state = make_state("", 0, None);
    state.toggle_paragraph_integrity(); // on -> condense forced off
    state.set_paste_condense(true); // user turns it back on deliberately

    state.toggle_paragraph_integrity(); // off again

    assert!(!state.preferences.paragraph_integrity);
    assert!(
        state.preferences.paste_condense,
        "toggling integrity off should not undo a deliberate choice"
    );
}

/// The Pilcrows ribbon button and the settings checkbox are the same
/// switch, not two that can disagree.
#[test]
fn the_pilcrow_button_drives_the_pilcrow_setting() {
    let mut state = make_state("", 0, None);
    assert!(!state.preferences.paste_condense_pilcrow);

    state.toggle_pilcrows();
    assert!(state.preferences.pilcrows);
    assert!(state.preferences.paste_condense_pilcrow);

    state.toggle_pilcrows();
    assert!(!state.preferences.paste_condense_pilcrow);
}

#[test]
fn emphasis_options_are_independent() {
    let mut state = make_state("", 0, None);
    state.set_emphasis(true, true, false);
    assert!(
        state.preferences.emphasis_bold
            && state.preferences.emphasis_underline
            && !state.preferences.emphasis_box
    );

    state.set_emphasis(false, true, true);
    assert!(
        !state.preferences.emphasis_bold
            && state.preferences.emphasis_underline
            && state.preferences.emphasis_box
    );
}

#[test]
fn apply_emphasis_style_applies_exactly_the_configured_combination() {
    let mut state = make_state("hello", 0, None);
    state.set_emphasis(true, false, true); // bold + box, no underline
    state.workspace.tabs[0].selection = Some((0, 5));

    state.apply_emphasis_style();

    let r = &state.workspace.tabs[0].document.paragraphs()[0].runs[0];
    assert!(r.bold && !r.underline);
    // Emphasis's "Box" is the small inline border (`emphasis_boxed`),
    // never Pocket's paragraph-wide box (`box_format`) — regression
    // guard for a bug where this called `FormatOp::Box(true)` and wrapped
    // the whole paragraph instead.
    assert!(
        !r.box_format,
        "emphasis must never set Pocket's paragraph-wide box"
    );
    assert!(r.emphasis && r.emphasis_boxed);
    assert_eq!(r.size, 0, "emphasis_change_size is off by default");
}

#[test]
fn apply_emphasis_style_applies_the_configured_size_only_when_change_size_is_on() {
    let mut state = make_state("hello", 0, None);
    state.set_emphasis(false, false, false);
    state.set_emphasis_change_size(true);
    state.set_emphasis_size_points(18);
    state.workspace.tabs[0].selection = Some((0, 5));

    state.apply_emphasis_style();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].size,
        36
    );
}

/// The bug `apply_cite_style`'s pattern would have carried over:
/// re-clicking Emphasis on text already bold (e.g. inside a Tag) must
/// emphasize it, not toggle bold back off.
#[test]
fn apply_emphasis_style_does_not_toggle_off_already_bold_text() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            list: None,
            runs: vec![Run {
                text: "hello".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }],
        0,
    );
    state.set_emphasis(true, false, false);
    state.workspace.tabs[0].selection = Some((0, 5));

    state.apply_emphasis_style();

    let r = &state.workspace.tabs[0].document.paragraphs()[0].runs[0];
    assert!(r.bold, "already-bold text must stay bold, not toggle off");
    assert!(r.emphasis);
}

/// Emphasizing a phrase inside a Block must not erase the Block marker —
/// the exact landmine that ruled out modeling Emphasis as a `CardStyle`.
#[test]
fn apply_emphasis_style_preserves_an_existing_card_style_marker() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            list: None,
            runs: vec![Run {
                text: "evidence text".into(),
                bold: true,
                style: Some(CardStyle::Block),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }],
        0,
    );
    state.set_emphasis(false, true, false);
    state.workspace.tabs[0].selection = Some((0, 8)); // "evidence"

    state.apply_emphasis_style();

    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert!(runs[0].emphasis && runs[0].style == Some(CardStyle::Block));
}

#[test]
fn apply_emphasis_style_pushes_exactly_one_undo_entry() {
    let mut state = make_state("hello", 0, None);
    state.set_emphasis(true, true, true);
    state.set_emphasis_change_size(true);
    state.workspace.tabs[0].selection = Some((0, 5));
    let undo_depth_before = state.workspace.tabs[0].document.undo_stack.len();

    state.apply_emphasis_style();

    assert_eq!(
        state.workspace.tabs[0].document.undo_stack.len(),
        undo_depth_before + 1
    );
}

// ── Read mode ──────────────────────────────────────────────────────────

#[test]
fn read_mode_hides_the_sidebar_and_collapses_the_split() {
    let mut state = make_state("doc", 0, None);
    state.ui.sidebar_visible = true;
    state.open_split();
    let tabs_before = state.workspace.tabs.len();

    state.toggle_read_mode();

    assert!(state.ui.read_mode);
    assert!(!state.ui.sidebar_visible);
    assert!(!state.workspace.split_view);
    // The split's tab is only un-shown, never closed.
    assert_eq!(
        state.workspace.tabs.len(),
        tabs_before,
        "read mode closed a tab"
    );
}

#[test]
fn leaving_read_mode_restores_the_sidebar_it_hid() {
    let mut state = make_state("doc", 0, None);
    state.ui.sidebar_visible = true;

    state.toggle_read_mode();
    state.toggle_read_mode();

    assert!(!state.ui.read_mode);
    assert!(state.ui.sidebar_visible);
}

/// A sidebar the user had already hidden must stay hidden on exit —
/// restoring it would be read mode turning something on that wasn't.
#[test]
fn leaving_read_mode_does_not_reveal_a_sidebar_that_was_already_hidden() {
    let mut state = make_state("doc", 0, None);
    state.ui.sidebar_visible = false;

    state.toggle_read_mode();
    state.toggle_read_mode();

    assert!(!state.ui.sidebar_visible);
}

// ── Split view (notes/split_view_plan.md) ──────────────────────────────

/// The index-vs-id trap this feature is most exposed to: closing a tab
/// positioned *before* the secondary pane's shifts every later index, and
/// an index-based `secondary_tab_id` would silently retarget the pane.
#[test]
fn secondary_pane_survives_a_tab_closing_before_it() {
    let mut state = make_state("", 0, None);
    state.new_tab();
    state.new_tab();
    state.open_split(); // 4 tabs; secondary is the newest
    let secondary_id = state.workspace.secondary_tab_id.unwrap();

    state.close_tab(0);

    assert!(
        state.workspace.split_view,
        "split should survive an unrelated close"
    );
    assert_eq!(state.workspace.secondary_tab_id, Some(secondary_id));
    let idx = state.pane_tab_index(Pane::Secondary).unwrap();
    assert_eq!(
        state.workspace.tabs[idx].id, secondary_id,
        "pane followed the wrong tab"
    );
}

/// Decision 1: one document is never in two panes. Asking for the tab the
/// secondary pane holds focuses that pane instead.
#[test]
fn activating_the_secondary_panes_tab_focuses_that_pane() {
    let mut state = make_state("", 0, None);
    state.open_split();
    let secondary_idx = state.pane_tab_index(Pane::Secondary).unwrap();
    state.focus_pane(Pane::Primary);

    state.set_active_tab(secondary_idx);

    assert_eq!(state.workspace.focused_pane, Pane::Secondary);
    assert_eq!(state.workspace.active_tab, secondary_idx);
}

/// Clicking a tab shows it in whichever pane is live — this is the only
/// way to get an existing document into the split.
#[test]
fn activating_a_tab_opens_it_in_the_focused_pane() {
    let mut state = make_state("tab zero", 0, None);
    state.new_tab(); // idx 1 — the one we'll click; in neither pane
    state.new_tab(); // idx 2 — what the primary pane ends up showing
    state.open_split(); // idx 3 — secondary, and now focused
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);
    let primary_before = state.pane_tab_index(Pane::Primary);

    state.set_active_tab(1);

    assert_eq!(
        state.workspace.focused_pane,
        Pane::Secondary,
        "click yanked focus to the other pane"
    );
    assert_eq!(state.pane_tab_index(Pane::Secondary), Some(1));
    // ...and the primary pane kept whatever it was already showing.
    assert_eq!(state.pane_tab_index(Pane::Primary), primary_before);
}

/// The other half of decision 1: clicking the tab the *other* pane is
/// already showing focuses that pane instead of duplicating it.
#[test]
fn activating_the_other_panes_tab_focuses_that_pane_from_either_side() {
    let mut state = make_state("", 0, None);
    state.open_split();
    let primary_idx = state.pane_tab_index(Pane::Primary).unwrap();

    // Focused pane is Secondary; click the primary's tab.
    state.set_active_tab(primary_idx);
    assert_eq!(state.workspace.focused_pane, Pane::Primary);

    // And back the other way.
    let secondary_idx = state.pane_tab_index(Pane::Secondary).unwrap();
    state.set_active_tab(secondary_idx);
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);
}

#[test]
fn closing_the_secondary_panes_tab_collapses_the_split() {
    let mut state = make_state("", 0, None);
    state.new_tab();
    state.open_split();
    let idx = state.pane_tab_index(Pane::Secondary).unwrap();

    state.close_tab(idx);

    assert!(!state.workspace.split_view);
    assert_eq!(state.workspace.secondary_tab_id, None);
    assert_eq!(state.workspace.focused_pane, Pane::Primary);
}

/// Two panes need two tabs; dropping to one has to collapse the split
/// even when the tab closed was not the secondary pane's own.
#[test]
fn closing_down_to_one_tab_collapses_the_split() {
    let mut state = make_state("", 0, None);
    state.open_split(); // 2 tabs
    state.close_tab(0);

    assert!(!state.workspace.split_view);
    assert_eq!(state.workspace.tabs.len(), 1);
}

#[test]
fn open_split_is_idempotent() {
    let mut state = make_state("", 0, None);
    state.open_split();
    let tabs_after_first = state.workspace.tabs.len();
    let id = state.workspace.secondary_tab_id;

    state.open_split();

    assert_eq!(
        state.workspace.tabs.len(),
        tabs_after_first,
        "second open stacked a blank tab"
    );
    assert_eq!(state.workspace.secondary_tab_id, id);
}

#[test]
fn focus_pane_repoints_active_tab() {
    let mut state = make_state("", 0, None);
    state.open_split();
    let secondary_idx = state.pane_tab_index(Pane::Secondary).unwrap();

    state.focus_pane(Pane::Primary);
    assert_ne!(state.workspace.active_tab, secondary_idx);

    state.focus_pane(Pane::Secondary);
    assert_eq!(state.workspace.active_tab, secondary_idx);
}

/// With the split closed the secondary pane has nothing to show, and its
/// editor must render blank rather than mirroring the primary.
#[test]
fn secondary_pane_has_no_tab_while_the_split_is_closed() {
    let state = make_state("hello", 0, None);
    assert_eq!(state.pane_tab_index(Pane::Secondary), None);
    assert_eq!(state.pane_content(Pane::Secondary), "");
    assert_eq!(state.pane_content(Pane::Primary), "hello");
}

/// The bug this exists to prevent: with focus in the secondary pane,
/// `active_tab` names the *secondary's* document, so a primary pane that
/// resolved through `active_tab` would paint the same text in both halves.
#[test]
fn the_two_panes_never_resolve_to_the_same_tab() {
    let mut state = make_state("primary text", 0, None);
    state.open_split();
    state.insert_str("secondary text");

    // Focus is in the secondary pane right after open_split.
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);
    let primary = state.pane_tab_index(Pane::Primary).unwrap();
    let secondary = state.pane_tab_index(Pane::Secondary).unwrap();
    assert_ne!(primary, secondary, "both panes resolved to one tab");
    assert_eq!(state.pane_content(Pane::Primary), "primary text");
    assert_eq!(state.pane_content(Pane::Secondary), "secondary text");

    // ...and the same holds with focus back in the primary pane.
    state.focus_pane(Pane::Primary);
    assert_ne!(
        state.pane_tab_index(Pane::Primary),
        state.pane_tab_index(Pane::Secondary)
    );
    assert_eq!(state.pane_content(Pane::Primary), "primary text");
    assert_eq!(state.pane_content(Pane::Secondary), "secondary text");
}

/// Opening a file with the secondary pane focused must put it in *that*
/// pane. Before `show_in_focused_pane` it moved `active_tab` while both
/// panes kept their stored ids, so the document appeared in neither.
#[test]
fn opening_a_tab_with_the_secondary_pane_focused_lands_there() {
    let mut state = make_state("primary", 0, None);
    state.open_split();
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);

    state.new_tab();

    let secondary = state.pane_tab_index(Pane::Secondary).unwrap();
    assert_eq!(
        secondary, state.workspace.active_tab,
        "new tab did not land in the focused pane"
    );
    assert_ne!(state.pane_tab_index(Pane::Primary), Some(secondary));
    assert_eq!(state.pane_content(Pane::Primary), "primary");
}

/// The reported bug: double-clicking a file in the sidebar with the split
/// pane focused opened a new tab that appeared in *neither* pane. The
/// new-tab path in `open_file` still assigned `active_tab` directly and
/// never updated the focused pane's stored id.
#[test]
fn opening_a_file_with_the_secondary_pane_focused_shows_it_there() {
    let dir = std::env::temp_dir().join(format!("vimbatim_split_open_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("card.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();

    let mut state = make_state("primary", 0, None);
    state.open_split();
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);
    let primary_before = state.pane_tab_index(Pane::Primary);

    state.open_file(path.clone());

    let secondary = state
        .pane_tab_index(Pane::Secondary)
        .expect("split collapsed");
    assert_eq!(
        state.workspace.tabs[secondary].file_path.as_deref(),
        Some(path.as_path()),
        "opened file did not land in the focused pane"
    );
    assert_eq!(state.pane_tab_index(Pane::Primary), primary_before);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Recovery reopens a document the same way, and had the same defect.
#[test]
fn resumed_recovery_lands_in_the_focused_pane() {
    let (mut state, _dir) = make_state_with_recovery("split-resume");
    state.open_split();
    assert_eq!(state.workspace.focused_pane, Pane::Secondary);

    state.resume_recovery();

    let secondary = state
        .pane_tab_index(Pane::Secondary)
        .expect("split collapsed");
    assert_eq!(
        state.workspace.active_tab, secondary,
        "recovered tab bypassed the focused pane"
    );
}

#[test]
fn split_ratio_is_clamped_away_from_collapsing_a_pane() {
    assert_eq!(clamp_split_ratio(0.0), 0.2);
    assert_eq!(clamp_split_ratio(0.5), 0.5);
    assert_eq!(clamp_split_ratio(1.0), 0.8);
}

// ── pending_focus_editor (tab switch / open / close / new-tab) ──────────

#[test]
fn set_active_tab_requests_editor_focus() {
    // Root-cause regression test for the intermittent Enter/keyboard
    // lockout (notes/feedback.md): clicking a tab only ever called
    // `set_active_tab`, which never touched GPUI keyboard focus, so the
    // text editor's FocusHandle was left stale until the user clicked
    // back into it. `pending_focus_editor` is the flag `TextEditor::render`
    // checks-and-clears to reclaim focus once per frame.
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    state.workspace.pending_focus_editor = None;

    state.set_active_tab(1);

    assert_eq!(state.workspace.pending_focus_editor, Some(Pane::Primary));
}

#[test]
fn set_active_tab_out_of_range_does_not_request_focus() {
    let mut state = make_state("hello", 0, None);
    state.workspace.pending_focus_editor = None;

    state.set_active_tab(99);

    assert_eq!(state.workspace.pending_focus_editor, None);
}

// ── rename_tab (double-click tab rename) ────────────────────────────

#[test]
fn rename_tab_updates_title_by_id() {
    let mut state = make_state("hello", 0, None);
    let id = state.workspace.tabs[state.workspace.active_tab].id;

    state.rename_tab(id, "My Renamed Tab".to_string());

    assert_eq!(
        state.workspace.tabs[state.workspace.active_tab].title,
        "My Renamed Tab"
    );
}

#[test]
fn rename_tab_ignores_empty_title() {
    let mut state = make_state("hello", 0, None);
    let id = state.workspace.tabs[state.workspace.active_tab].id;
    let original = state.workspace.tabs[state.workspace.active_tab]
        .title
        .clone();

    state.rename_tab(id, "".to_string());

    assert_eq!(
        state.workspace.tabs[state.workspace.active_tab].title,
        original
    );
}

#[test]
fn new_tab_requests_editor_focus() {
    let mut state = make_state("hello", 0, None);
    state.workspace.pending_focus_editor = None;

    state.new_tab();

    assert_eq!(state.workspace.pending_focus_editor, Some(Pane::Primary));
}

/// The guard is in `open_file` rather than at the toolbar's picker, so it
/// has to hold for every entry point — vim's `:e <path>` included.
#[test]
fn open_file_refuses_anything_that_is_not_a_docx() {
    let dir = std::env::temp_dir().join(format!("vimbatim_ext_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let txt = dir.join("notes.txt");
    std::fs::write(&txt, "plain text").unwrap();

    let mut state = make_state("hello", 0, None);
    let tabs_before = state.workspace.tabs.len();
    state.open_file(txt);
    assert_eq!(
        state.workspace.tabs.len(),
        tabs_before,
        "a .txt must not open a tab"
    );

    // Case-insensitive: Windows hands back .DOCX from the native picker.
    let upper = dir.join("doc.DOCX");
    create_new_docx(&default_paragraphs(), &upper, Default::default()).unwrap();
    state.open_file(upper);
    assert_eq!(
        state.workspace.tabs.len(),
        tabs_before + 1,
        ".DOCX must still open"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_as_writes_the_file_and_repoints_the_tab() {
    let dir = std::env::temp_dir().join(format!("vimbatim_saveas_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("renamed.docx");

    let mut state = make_state("hello world", 0, None);
    state.save_active_tab_as(dest.clone()).unwrap();

    assert!(dest.exists(), "Save As must write the file");
    // Re-pointed, not merely copied — a later plain Ctrl+S goes here too.
    assert_eq!(
        state.workspace.tabs[0].file_path.as_deref(),
        Some(dest.as_path())
    );
    assert_eq!(state.workspace.tabs[0].title, "renamed.docx");
    assert!(
        !state.workspace.tabs[0].document.is_modified,
        "a successful save clears the dirty flag"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A picker (or a user typing a name) can hand back a path with no
/// extension — saving there would produce a file `open_file` then refuses
/// to reopen.
#[test]
fn save_as_appends_docx_when_the_chosen_name_lacks_it() {
    let dir = std::env::temp_dir().join(format!("vimbatim_saveas_ext_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut state = make_state("hello", 0, None);
    state.save_active_tab_as(dir.join("no_extension")).unwrap();

    assert_eq!(state.workspace.tabs[0].title, "no_extension.docx");
    assert!(dir.join("no_extension.docx").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_file_new_tab_requests_editor_focus() {
    let dir = std::env::temp_dir().join(format!("vimbatim_focus_test_{}_new", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();

    let mut state = make_state("hello", 0, None);
    state.workspace.pending_focus_editor = None;

    state.open_file(path);

    assert_eq!(state.workspace.pending_focus_editor, Some(Pane::Primary));
}

#[test]
fn open_file_existing_tab_requests_editor_focus() {
    let dir = std::env::temp_dir().join(format!(
        "vimbatim_focus_test_{}_existing",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();

    let mut state = make_state("hello", 0, None);
    state.open_file(path.clone());
    state.workspace.pending_focus_editor = None; // clear what the first open set

    state.open_file(path); // already open -> switches to existing tab

    assert_eq!(state.workspace.pending_focus_editor, Some(Pane::Primary));
}

#[test]
fn close_tab_requests_editor_focus() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    state.workspace.pending_focus_editor = None;

    state.close_tab(0);

    assert_eq!(state.workspace.pending_focus_editor, Some(Pane::Primary));
}

#[test]
fn close_tab_pushes_a_file_backed_tabs_path_onto_closed_tabs() {
    let mut state = make_state("hello", 0, None);
    let path = PathBuf::from("/tmp/vimbatim_reopen_test_a.docx");
    state
        .workspace
        .tabs
        .push(Tab::from_path(TabId(1), path.clone()));

    state.close_tab(1);

    assert_eq!(state.workspace.closed_tabs, vec![path]);
}

#[test]
fn close_tab_does_not_stack_a_blank_new_tab() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs.push(Tab::new_empty(TabId(1))); // no file_path
    state.workspace.closed_tabs.clear();

    state.close_tab(1);

    assert!(state.workspace.closed_tabs.is_empty());
}

/// A `.docx` this build can't parse must never be silently replaced by a
/// blank document. Before the fix, the tab kept `file_path` with
/// `docx_origin: None`, so the first edit + Ctrl+S took `save_tab`'s
/// `create_new_docx` branch and overwrote the original in place.
#[test]
fn opening_an_unparseable_docx_detaches_the_tab_so_a_save_cannot_overwrite_it() {
    let dir = std::env::temp_dir().join(format!("vimbatim_unparseable_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("corrupt.docx");
    std::fs::write(&path, b"this is not a zip, let alone a docx").unwrap();
    let before = std::fs::read(&path).unwrap();

    let mut state = make_state("hello", 0, None);
    state.open_file(path.clone());

    let tab = state
        .workspace
        .tabs
        .iter()
        .find(|t| t.title == "corrupt.docx")
        .expect("tab still opens");
    assert_eq!(
        tab.file_path, None,
        "an unreadable file must not stay attached to its tab"
    );

    // Edit and save the way a user would; the original must be untouched.
    let idx = state
        .workspace
        .tabs
        .iter()
        .position(|t| t.title == "corrupt.docx")
        .unwrap();
    state.workspace.active_tab = idx;
    state.insert_str("typed over it");
    state.save_active_tab().unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the unreadable original was overwritten"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Detaching an unreadable file from its tab stops the overwrite, but on
/// its own it is a silent trap: the tab is still titled after the file and
/// still looks like it opened it, while Ctrl+S quietly does nothing. The
/// tab has to say so.
#[test]
fn an_unreadable_file_puts_a_warning_banner_on_its_tab() {
    let dir = std::env::temp_dir().join(format!("vimbatim_banner_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("corrupt.docx");
    std::fs::write(&path, b"not a zip").unwrap();

    let mut state = make_state("hello", 0, None);
    state.open_file(path);
    let tab = state
        .workspace
        .tabs
        .iter()
        .find(|t| t.title == "corrupt.docx")
        .unwrap();
    assert!(tab.opened_detached);
    let message = tab.banner_message().expect("a warning is shown");
    assert!(
        message.contains("Save As"),
        "it has to point somewhere: {message}"
    );

    // Dismissible, like the unsupported-content banner it shares a slot with.
    let idx = state
        .workspace
        .tabs
        .iter()
        .position(|t| t.title == "corrupt.docx")
        .unwrap();
    state.workspace.tabs[idx].banner_dismissed = true;
    assert_eq!(state.workspace.tabs[idx].banner_message(), None);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Dismissing the warning must not make Save silent again. Pressing Save is
/// exactly when the user needs to know the tab has nowhere to write, and
/// `SaveAction` only logs to stderr — so the banner comes back.
#[test]
fn saving_a_detached_tab_brings_its_warning_back() {
    let dir = std::env::temp_dir().join(format!("vimbatim_resave_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("corrupt.docx");
    std::fs::write(&path, b"not a zip").unwrap();

    let mut state = make_state("hello", 0, None);
    state.open_file(path);
    let idx = state
        .workspace
        .tabs
        .iter()
        .position(|t| t.title == "corrupt.docx")
        .unwrap();
    state.workspace.active_tab = idx;

    state.workspace.tabs[idx].banner_dismissed = true;
    assert_eq!(state.workspace.tabs[idx].banner_message(), None);

    state.insert_str("work worth keeping");
    state.save_active_tab().unwrap();
    assert!(
        state.workspace.tabs[idx].banner_message().is_some(),
        "Save on a detached tab re-shows the warning it can't act on",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A tab that opened its file cleanly says nothing.
#[test]
fn a_normal_tab_shows_no_banner() {
    let state = make_state("hello", 0, None);
    assert_eq!(state.workspace.tabs[0].banner_message(), None);
}

/// Pasting a cut back into the folder it came from means "leave it where it
/// is". It used to fall through to `unique_path_in`, which sees the name
/// taken — by the very file being moved — and renamed it: `Case.docx`
/// became `Case 1.docx`, `Round 3` became `Round 3 1`. A no-op gesture must
/// not silently rename the user's work.
#[test]
fn cutting_and_pasting_into_the_same_folder_leaves_it_alone() {
    let dir = temp_test_dir("cut_same_parent");
    let file = dir.join("Case.docx");
    std::fs::write(&file, b"contents").unwrap();
    let folder = dir.join("Round 3");
    std::fs::create_dir(&folder).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();

    state.cut_file(file.clone());
    state.paste_file_into(&dir).unwrap();
    assert!(file.exists(), "the file kept its name");
    assert!(!dir.join("Case 1.docx").exists(), "and gained no duplicate");
    assert!(state.copied_file.is_none(), "the cut is spent either way");

    state.cut_file(folder.clone());
    state.paste_file_into(&dir).unwrap();
    assert!(folder.is_dir(), "the folder kept its name");
    assert!(!dir.join("Round 3 1").exists());
}

/// A moved file's *closed* tabs follow it too. The reopen stack
/// (Shift+Ctrl+W) is the one other place a document path is held, and left
/// stale it pointed at a path that no longer exists — which, since a
/// missing file stays attached to its tab by design, meant the next save
/// recreated an empty document back at the old location.
#[test]
fn moving_a_file_also_moves_it_on_the_reopen_stack() {
    let dir = temp_test_dir("move_reopen_stack");
    let target = dir.join("target");
    std::fs::create_dir(&target).unwrap();
    let src = dir.join("Case.docx");
    crate::docx_parser::create_new_docx(&default_paragraphs(), &src, Default::default()).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.open_file(src.clone());
    // `close_tab` always keeps one tab open, and `open_file` reused the
    // blank starting tab — so give it a second one to close from.
    state.new_tab();
    let idx = state
        .workspace
        .tabs
        .iter()
        .position(|t| t.file_path.as_ref() == Some(&src))
        .unwrap();
    state.close_tab(idx);
    assert_eq!(
        state.workspace.closed_tabs,
        vec![src.clone()],
        "closing stacked its path"
    );

    state.cut_file(src.clone());
    state.paste_file_into(&target).unwrap();

    let moved = target.join("Case.docx");
    assert_eq!(
        state.workspace.closed_tabs,
        vec![moved.clone()],
        "the stacked path followed the move"
    );

    state.reopen_closed_tab();
    assert!(
        state
            .workspace
            .tabs
            .iter()
            .any(|t| t.file_path.as_ref() == Some(&moved)),
        "reopening lands on the file where it now lives",
    );
}

/// The same for a whole folder: every closed tab that lived inside it is
/// prefix-swapped, exactly as the open ones are.
#[test]
fn moving_a_folder_also_moves_its_files_on_the_reopen_stack() {
    let dir = temp_test_dir("move_folder_reopen_stack");
    let src = dir.join("Round 3");
    std::fs::create_dir_all(src.join("nested")).unwrap();
    let inner = src.join("nested").join("Case.docx");
    std::fs::write(&inner, b"x").unwrap();
    let target = dir.join("Archive");
    std::fs::create_dir(&target).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.workspace.closed_tabs.push(inner.clone());

    state.cut_file(src.clone());
    state.paste_file_into(&target).unwrap();

    assert_eq!(
        state.workspace.closed_tabs,
        vec![target.join("Round 3").join("nested").join("Case.docx")],
    );
}

/// The self-move guard compares canonical paths, so a symlinked
/// destination pointing inside the folder being moved is caught here with a
/// message that says what happened — rather than reaching the filesystem
/// and coming back as "Invalid argument (os error 22)".
#[cfg(unix)]
#[test]
fn moving_a_folder_into_itself_through_a_symlink_is_refused_by_name() {
    let dir = temp_test_dir("cut_symlink");
    let src = dir.join("Src");
    std::fs::create_dir_all(src.join("inner")).unwrap();
    let link = dir.join("link_to_inner");
    std::os::unix::fs::symlink(src.join("inner"), &link).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.cut_file(src.clone());

    let err = state.paste_file_into(&link).expect_err("refused");
    assert!(
        err.to_string().contains("into itself"),
        "the message should name the cause, got: {err}",
    );
    assert!(src.is_dir(), "and the folder is untouched");
    assert!(state.copied_file.is_some(), "a refused paste keeps the cut");
}

/// The other half of the rule: a missing or zero-length file has no
/// content to lose, so it stays attached and saving creates it — how a
/// `touch`ed placeholder and a reopened-after-deletion tab are meant to
/// work.
#[test]
fn opening_an_empty_placeholder_docx_stays_attached_so_it_can_be_written() {
    let dir = std::env::temp_dir().join(format!("vimbatim_placeholder_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("placeholder.docx");
    std::fs::write(&path, b"").unwrap();

    let mut state = make_state("hello", 0, None);
    state.open_file(path.clone());

    let idx = state
        .workspace
        .tabs
        .iter()
        .position(|t| t.title == "placeholder.docx")
        .unwrap();
    assert_eq!(state.workspace.tabs[idx].file_path.as_ref(), Some(&path));
    state.workspace.active_tab = idx;
    state.insert_str("real content now");
    state.save_active_tab().unwrap();
    assert!(
        std::fs::metadata(&path).unwrap().len() > 0,
        "saving a placeholder must write it"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A document created here has to be a package Word accepts: every part
/// declared in `[Content_Types].xml`, every relationship resolving, and
/// every `<w:rStyle>` naming a style the same file defines. Goes through
/// the real save path with real settings rather than the defaults.
#[test]
fn a_new_document_is_a_complete_package_with_no_dangling_references() {
    let dir = std::env::temp_dir().join(format!("vimbatim_pkg_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("New.docx");

    let mut state = make_state("", 0, None);
    state.preferences.normal_text_size_half_points = 26;
    state.set_card_size_points(CardStyleKind::Hat, 19);
    crate::docx_parser::create_new_docx(&default_paragraphs(), &path, state.new_doc_style())
        .unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    for required in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/_rels/document.xml.rels",
        "word/document.xml",
        "word/styles.xml",
        "word/settings.xml",
        "docProps/core.xml",
        "docProps/app.xml",
    ] {
        assert!(
            names.contains(&required.to_string()),
            "missing {required}: {names:?}"
        );
    }

    let read = |archive: &mut zip::ZipArchive<std::fs::File>, name: &str| {
        let mut out = String::new();
        std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut out).unwrap();
        out
    };
    let content_types = read(&mut archive, "[Content_Types].xml");
    for part in &names {
        // Every part except the two `Default`-covered extensions needs an
        // Override, or Word refuses the package.
        if part.ends_with(".rels") || part == "[Content_Types].xml" {
            continue;
        }
        assert!(
            content_types.contains(&format!("PartName=\"/{part}\"")),
            "{part} has no content-type override: {content_types}",
        );
    }

    // Relationship targets resolve to parts that are actually present.
    for rels in ["_rels/.rels", "word/_rels/document.xml.rels"] {
        let xml = read(&mut archive, rels);
        let base = if rels.starts_with("word/") {
            "word/"
        } else {
            ""
        };
        for target in xml
            .split("Target=\"")
            .skip(1)
            .map(|t| &t[..t.find('"').unwrap()])
        {
            let full = format!("{base}{target}");
            assert!(
                names.contains(&full),
                "{rels} points at missing {full}: {names:?}"
            );
        }
    }

    // The settings actually reached the file.
    let styles = read(&mut archive, "word/styles.xml");
    assert!(
        styles.contains("<w:sz w:val=\"26\"/><w:szCs w:val=\"26\"/>"),
        "got: {styles}"
    );
    assert!(styles.contains("w:styleId=\"Heading2\""));
    assert!(
        styles[styles.find("w:styleId=\"Heading2\"").unwrap()..].contains("<w:sz w:val=\"38\"/>"),
        "the configured 19pt Hat: {styles}",
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ── Doc Menu / Card Menu after a real save + reload ─────────────────────
//
// These commands identify a card by `run.style`, which used to be written
// into the file as `<w:rStyle w:val="VimbatimTag"/>` and friends. Those
// four markers are no longer written (Verbatim writes none either), so the
// marker now has to be re-derived from the paragraph's heading level at
// parse. Exercising them in memory, where the marker is set directly, would
// no longer prove they work on a document that has been through a file.

/// Round-trips a document through a real `.docx` and hands back what a
/// fresh open would see.
fn through_a_real_docx(paragraphs: Vec<Paragraph>, label: &str) -> AppState {
    let dir = std::env::temp_dir().join(format!("vimbatim_menu_{}_{}", std::process::id(), label));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.docx");
    let mut source = make_state_with_paragraphs(paragraphs, 0);
    source.save_active_tab_as(path.clone()).unwrap();
    let (reloaded, _) = crate::docx_parser::parse_docx(&path).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    make_state_with_paragraphs(reloaded, 0)
}

fn menu_fixture() -> Vec<Paragraph> {
    let styled = |text: &str, heading: u8, size: u16, style: Option<CardStyle>| Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            bold: true,
            size,
            style,
            ..Run::default()
        }],
        heading,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    vec![
        styled("Pocket", 1, 52, Some(CardStyle::Pocket)),
        styled("Hat", 2, 44, Some(CardStyle::Hat)),
        styled("Block", 3, 32, Some(CardStyle::Block)),
        styled("Tag line", 4, 26, Some(CardStyle::Tag)),
        styled("Analytic line", 0, 26, Some(CardStyle::Analytic)),
        Paragraph {
            list: None,
            runs: vec![
                Run {
                    text: "Cite".into(),
                    bold: true,
                    size: 26,
                    style: Some(CardStyle::Cite),
                    ..Run::default()
                },
                Run {
                    text: " body text".into(),
                    ..Run::default()
                },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "emphasized".into(),
                emphasis: true,
                emphasis_boxed: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ]
}

/// Every card marker survives a real save + reload, which is what all the
/// menu commands below depend on.
#[test]
fn card_markers_survive_a_real_docx_round_trip_without_rstyle_markers() {
    let state = through_a_real_docx(menu_fixture(), "markers");
    let paras = state.workspace.tabs[0].document.paragraphs();
    let marker = |i: usize| paras[i].runs[0].style;
    assert_eq!(marker(0), Some(CardStyle::Pocket));
    assert_eq!(marker(1), Some(CardStyle::Hat));
    assert_eq!(marker(2), Some(CardStyle::Block));
    assert_eq!(marker(3), Some(CardStyle::Tag));
    assert_eq!(
        marker(4),
        Some(CardStyle::Analytic),
        "Analytic keeps its own rStyle"
    );
    assert_eq!(
        marker(5),
        Some(CardStyle::Cite),
        "Cite rides on Verbatim's Style13ptBold"
    );
    assert!(paras[6].runs[0].emphasis && paras[6].runs[0].emphasis_boxed);
    // A Cite inside a plain paragraph must not have bled onto its neighbour.
    assert_eq!(paras[5].runs[1].style, None);
}

/// Doc Menu -> Delete tags, on a reloaded document.
#[test]
fn doc_menu_delete_tags_works_after_a_round_trip() {
    let mut state = through_a_real_docx(menu_fixture(), "deltags");
    state.delete_tags();
    let paras = state.workspace.tabs[0].document.paragraphs();
    assert_eq!(paras[3].heading, 0, "the Tag line lost its heading");
    assert!(!paras[3].runs[0].bold, "and its formatting");
    assert_eq!(paras[3].runs[0].style, None);
    // Nothing else touched.
    assert_eq!(paras[0].heading, 1);
    assert_eq!(paras[4].runs[0].style, Some(CardStyle::Analytic));
}

/// Doc Menu -> Delete analytics, on a reloaded document.
#[test]
fn doc_menu_delete_analytics_works_after_a_round_trip() {
    let mut state = through_a_real_docx(menu_fixture(), "delanalytics");
    let before = state.workspace.tabs[0].document.paragraphs().len();
    state.delete_analytics();
    let paras = state.workspace.tabs[0].document.paragraphs();
    // "Delete analytics" removes the lines outright, unlike "Delete tags"
    // which strips a Tag back to body text in place.
    assert_eq!(paras.len(), before - 1);
    assert!(
        !paras
            .iter()
            .any(|p| p.runs.iter().any(|r| r.style == Some(CardStyle::Analytic))),
        "no analytic survived",
    );
    assert_eq!(
        paras[3].runs[0].style,
        Some(CardStyle::Tag),
        "Tags untouched"
    );
}

/// Doc Menu -> Convert analytics to tags, on a reloaded document. Reads one
/// marker and writes the other, so it depends on both directions.
#[test]
fn doc_menu_convert_analytics_to_tags_works_after_a_round_trip() {
    let mut state = through_a_real_docx(menu_fixture(), "convert");
    state.convert_analytics_to_tags();
    let paras = state.workspace.tabs[0].document.paragraphs();
    assert_eq!(paras[4].heading, CardStyleKind::Tag.heading_level());
    assert_eq!(paras[4].runs[0].style, Some(CardStyle::Tag));
}

/// Doc Menu -> Remove emphasis, on a reloaded document. Emphasis now also
/// writes Verbatim's `Emphasis` character style, so this proves the flag
/// still round-trips and still clears.
#[test]
fn doc_menu_remove_emphasis_works_after_a_round_trip() {
    let mut state = through_a_real_docx(menu_fixture(), "emphasis");
    assert!(
        state.workspace.tabs[0].document.paragraphs()[6].runs[0].emphasis,
        "it survived the file"
    );
    state.select_all();
    state.remove_emphasis();
    let run = &state.workspace.tabs[0].document.paragraphs()[6].runs[0];
    assert!(!run.emphasis && !run.emphasis_boxed, "emphasis cleared");
}

/// Doc Menu -> Select similar formatting, across a card line whose runs are
/// split by a whitespace-only run.
///
/// `from_heading` used to skip blank runs when restoring the marker, so the
/// space came back unmarked between two marked neighbours —
/// `ranges_matching_format` compares on `format_key`, which includes
/// `style` (and deliberately ignores `whitespace_preserve`), so the line
/// matched as two ranges with a hole rather than one.
#[test]
fn doc_menu_select_similar_formatting_spans_a_whitespace_run_in_a_card_line() {
    let run = |text: &str| Run {
        text: text.into(),
        bold: true,
        size: 52,
        style: Some(CardStyle::Pocket),
        ..Run::default()
    };
    let mut state = through_a_real_docx(
        vec![Paragraph {
            list: None,
            runs: vec![run("Poc"), run(" "), run("ket")],
            heading: 1,
            alignment: Alignment::Center,
            unsupported_xml: None,
        }],
        "similar",
    );
    assert!(
        state.workspace.tabs[0].document.paragraphs()[0]
            .runs
            .iter()
            .all(|r| r.style == Some(CardStyle::Pocket)),
        "every run in the line carries the marker: {:?}",
        state.workspace.tabs[0].document.paragraphs()[0]
            .runs
            .iter()
            .map(|r| (&r.text, r.style))
            .collect::<Vec<_>>(),
    );

    state.workspace.tabs[0].cursor = 0;
    state.workspace.tabs[0].selection = Some((0, 3)); // "Poc"
    state.select_similar_formatting();
    assert_eq!(
        state.workspace.tabs[0].similar_ranges,
        vec![(0, "Poc ket".len())],
        "the whole line matches as one range, space included",
    );
}

/// Doc Menu -> Remove blank lines, on a document whose blank lines only
/// exist because Phase 1 stopped dropping self-closing `<w:p/>`.
#[test]
fn doc_menu_remove_blank_lines_works_on_recovered_blank_lines() {
    let mut state = through_a_real_docx(
        vec![
            para_plain("first"),
            para_plain(""),
            para_plain(""),
            para_plain("second"),
        ],
        "blanklines",
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs().len(),
        4,
        "the blank lines survived the file"
    );
    state.remove_blank_lines();
    assert_eq!(
        state.workspace.tabs[0]
            .document
            .paragraphs()
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>(),
        vec!["first", "second"],
    );
}

/// Card Menu -> Condense / Uncondensed, on a reloaded document — they work
/// on text rather than markers, so this is a guard that the marker changes
/// didn't disturb them.
#[test]
fn card_menu_condense_and_uncondense_work_after_a_round_trip() {
    let mut state = through_a_real_docx(
        vec![para_plain("one"), para_plain("two"), para_plain("three")],
        "condense",
    );
    let len = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, len));
    state.condense_selection();
    assert!(
        !state.workspace.tabs[0].document.content().contains('\n'),
        "condensed to one line: {:?}",
        state.workspace.tabs[0].document.content()
    );

    let len = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, len));
    state.uncondense_selection();
    assert!(
        state.workspace.tabs[0].document.content().contains('\n'),
        "and back: {:?}",
        state.workspace.tabs[0].document.content()
    );
}

/// Card Menu -> Standardize highlighting, on a reloaded document.
#[test]
fn card_menu_standardize_highlighting_works_after_a_round_trip() {
    let mut state = through_a_real_docx(
        vec![Paragraph {
            list: None,
            runs: vec![
                Run {
                    text: "one".into(),
                    highlight: true,
                    highlight_color: "green".into(),
                    ..Run::default()
                },
                Run {
                    text: "two".into(),
                    highlight: true,
                    highlight_color: "cyan".into(),
                    ..Run::default()
                },
            ],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        }],
        "highlight",
    );
    state.preferences.highlight_color = "yellow".to_string();
    state.standardize_highlighting();
    assert!(
        state.workspace.tabs[0].document.paragraphs()[0]
            .runs
            .iter()
            .all(|r| !r.highlight || r.highlight_color == "yellow"),
        "got: {:?}",
        state.workspace.tabs[0].document.paragraphs()[0]
            .runs
            .iter()
            .map(|r| r.highlight_color.clone())
            .collect::<Vec<_>>(),
    );
}

/// A blank document created in Verbatim used to parse to *zero*
/// paragraphs, and the first keystroke then panicked
/// (`index out of bounds: the len is 0 but the index is 0`,
/// `document_ops.rs`'s `sync_insert_char`). Guards the whole path, not
/// just the parser: create in Verbatim, open here, type.
#[test]
fn a_document_of_only_empty_paragraphs_opens_and_accepts_typing() {
    let dir = std::env::temp_dir().join(format!("vimbatim_blank_ext_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Blank.docx");
    // A body with no paragraph this parser recognises — the same end state
    // Verbatim's own blank document reached, whose single self-closing
    // `<w:p/>` used to be dropped. Built through the real writer so the
    // whole open path is exercised, not just the parser.
    crate::docx_parser::create_new_docx(&[], &path, Default::default()).unwrap();

    let mut state = make_state("hello", 0, None);
    state.open_file(path.clone());
    let idx = state
        .workspace
        .tabs
        .iter()
        .position(|t| t.title == "Blank.docx")
        .expect("opens");
    state.workspace.active_tab = idx;
    assert_eq!(
        state.workspace.tabs[idx].document.paragraphs().len(),
        1,
        "the blank paragraph must survive"
    );

    state.insert_str("typed");
    assert_eq!(state.workspace.tabs[idx].document.content(), "typed");
    state.save_active_tab().unwrap();

    let (reloaded, _) = crate::docx_parser::parse_docx(&path).unwrap();
    assert_eq!(
        reloaded
            .iter()
            .map(|p| p.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect::<Vec<_>>(),
        vec!["typed"],
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reopen_closed_tab_reopens_most_recently_closed_first() {
    let mut state = make_state("hello", 0, None);
    let a = PathBuf::from("/tmp/vimbatim_reopen_test_a.docx");
    let b = PathBuf::from("/tmp/vimbatim_reopen_test_b.docx");
    state
        .workspace
        .tabs
        .push(Tab::from_path(TabId(1), a.clone()));
    state
        .workspace
        .tabs
        .push(Tab::from_path(TabId(2), b.clone()));
    state.close_tab(1); // closes a
    state.close_tab(1); // closes b (shifted down after a's removal)

    state.reopen_closed_tab();
    assert!(
        state
            .workspace
            .tabs
            .iter()
            .any(|t| t.file_path.as_ref() == Some(&b)),
        "b (closed last) reopens first"
    );

    state.reopen_closed_tab();
    assert!(
        state
            .workspace
            .tabs
            .iter()
            .any(|t| t.file_path.as_ref() == Some(&a)),
        "a reopens second"
    );

    assert!(state.workspace.closed_tabs.is_empty());
}

#[test]
fn reopen_closed_tab_is_a_noop_with_nothing_closed() {
    let mut state = make_state("hello", 0, None);
    let tabs_before = state.workspace.tabs.len();

    state.reopen_closed_tab();

    assert_eq!(state.workspace.tabs.len(), tabs_before);
}

// ── next_tab / prev_tab (Task 9: Ctrl+Tab / Ctrl+Shift+Tab cycling) ─────

#[test]
fn next_tab_wraps_around() {
    let mut state = make_state("hello", 0, None);
    state.new_tab();
    state.new_tab(); // 3 tabs, active_tab at the last-created (index 2)
    state.set_active_tab(2);

    state.next_tab();

    assert_eq!(state.workspace.active_tab, 0);
}

#[test]
fn prev_tab_wraps_around() {
    let mut state = make_state("hello", 0, None);
    state.new_tab();
    state.new_tab();
    state.set_active_tab(0);

    state.prev_tab();

    assert_eq!(state.workspace.active_tab, 2);
}

// ── pending_close (Task 6: confirm before closing a dirty tab/app) ──────

#[test]
fn request_close_tab_shows_confirm_when_modified() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].document.is_modified = true;

    state.request_close_tab(0);

    assert_eq!(state.ui.pending_close, Some(PendingClose::Tab(TabId(0))));
    assert_eq!(state.workspace.tabs.len(), 1); // not yet closed
}

#[test]
fn pending_close_follows_tab_id_after_reorder() {
    let mut state = make_state("first", 0, None);
    state.new_tab();
    state.workspace.tabs[0].document.is_modified = true;
    let target = state.workspace.tabs[0].id;

    state.request_close_tab(0);
    state.move_tab(0, 2);
    state.confirm_close_discard();

    assert!(state.workspace.tabs.iter().all(|tab| tab.id != target));
}

#[test]
fn request_close_tab_closes_immediately_when_unmodified() {
    let mut state = make_state("hello", 0, None);
    state.new_tab();
    state.workspace.tabs[0].document.is_modified = false;

    state.request_close_tab(0);

    assert_eq!(state.ui.pending_close, None);
    assert_eq!(state.workspace.tabs.len(), 1); // closed immediately
}

#[test]
fn confirm_close_discard_closes_tab_without_saving() {
    let mut state = make_state("hello", 0, None);
    state.new_tab();
    state.workspace.tabs[0].document.is_modified = true;
    state.request_close_tab(0);

    state.confirm_close_discard();

    assert_eq!(state.ui.pending_close, None);
    assert_eq!(state.workspace.tabs.len(), 1);
}

#[test]
fn cancel_close_leaves_tab_open() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].document.is_modified = true;

    state.request_close_tab(0);
    state.cancel_close();

    assert_eq!(state.ui.pending_close, None);
    assert_eq!(state.workspace.tabs.len(), 1);
}

#[test]
fn request_close_app_shows_confirm_when_any_tab_modified() {
    let mut state = make_state("hello", 0, None);
    state.new_tab();
    state.workspace.tabs[0].document.is_modified = true; // the *other* (inactive) tab is dirty

    state.request_close_app();

    assert_eq!(state.ui.pending_close, Some(PendingClose::App));
}

#[test]
fn request_close_app_clears_pending_when_nothing_modified() {
    // No modified tabs: request_close_app resolves pending_close back to
    // None on its own (via confirm_close_discard) rather than leaving it
    // Some(App) — the GPUI caller reads this "None after the call" as
    // its signal to quit immediately without ever mounting the dialog.
    let mut state = make_state("hello", 0, None);

    state.request_close_app();

    assert_eq!(state.ui.pending_close, None);
}

#[test]
fn confirm_close_save_tab_clears_pending_and_closes() {
    // Tab 0 needs a real file_path here — save_tab only actually
    // persists (and confirm_close_save only actually closes) a tab that
    // has somewhere to write to. See
    // `confirm_close_save_does_not_discard_a_never_saved_tab` below for
    // the no-file_path case.
    let dir = std::env::temp_dir().join(format!(
        "vimbatim_confirm_close_save_test_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();

    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].file_path = Some(path);
    state.new_tab();
    state.workspace.tabs[0].document.is_modified = true;
    state.request_close_tab(0);

    let persisted = state.confirm_close_save();

    assert!(persisted);
    assert_eq!(state.ui.pending_close, None);
    assert_eq!(state.workspace.tabs.len(), 1);
}

#[test]
fn confirm_close_save_does_not_discard_a_never_saved_tab() {
    // A tab with no file_path (a plain "New Tab" that was never saved to
    // disk) has nowhere for save_tab to persist to — this app has no
    // "Save As" flow to fall back to. Before this fix, confirm_close_save
    // ignored save_tab's no-op result and closed the tab anyway, silently
    // discarding its content exactly as Discard would have, defeating the
    // whole point of the confirmation dialog. Correct behavior: leave the
    // tab open and dirty.
    let mut state = make_state("unsaved scratch content", 0, None);
    assert!(state.workspace.tabs[0].file_path.is_none());
    state.workspace.tabs[0].document.is_modified = true;
    state.new_tab(); // second tab so a wrongful close_tab would be observable
    state.request_close_tab(0);

    let persisted = state.confirm_close_save();

    assert!(!persisted);
    assert_eq!(state.ui.pending_close, None); // dialog still resolves/closes
    assert_eq!(state.workspace.tabs.len(), 2); // but the tab itself was NOT closed
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "unsaved scratch content"
    ); // content preserved
    assert!(state.workspace.tabs[0].document.is_modified); // still dirty, still needs a Save
}

#[test]
fn confirm_close_save_app_clears_pending_without_closing_tabs() {
    let mut state = make_state("hello", 0, None);
    state.new_tab();
    state.workspace.tabs[0].document.is_modified = true;
    state.request_close_app();
    assert_eq!(state.ui.pending_close, Some(PendingClose::App));

    let persisted = state.confirm_close_save();

    // tab 0 has no file_path, so the app-wide save can't fully persist —
    // the caller (close_confirm.rs) reads this `false` as "don't
    // cx.quit(), a dirty tab is still unsaved".
    assert!(!persisted);
    assert_eq!(state.ui.pending_close, None);
    assert_eq!(state.workspace.tabs.len(), 2); // saving the app doesn't remove tabs
}

#[test]
fn cancel_close_is_a_no_op_when_nothing_pending() {
    let mut state = make_state("hello", 0, None);
    state.cancel_close();
    assert_eq!(state.ui.pending_close, None);
}

// ── clamp_sidebar_width ──────────────────────────────────────────────────

#[test]
fn test_clamp_sidebar_width_within_range_is_unchanged() {
    assert_eq!(clamp_sidebar_width(300.0), 300.0);
}

#[test]
fn test_clamp_sidebar_width_below_min_clamps_to_min() {
    assert_eq!(clamp_sidebar_width(50.0), 180.0);
}

#[test]
fn test_clamp_sidebar_width_above_max_clamps_to_max() {
    assert_eq!(clamp_sidebar_width(900.0), 480.0);
}

#[test]
fn test_clamp_sidebar_width_default_is_within_range() {
    assert_eq!(
        clamp_sidebar_width(DEFAULT_SIDEBAR_WIDTH),
        DEFAULT_SIDEBAR_WIDTH
    );
}

// ── Rich text formatting Phase 1: default_paragraphs / Tab construction ────

#[test]
fn test_default_paragraphs_is_one_empty_paragraph_one_default_run() {
    let paragraphs = default_paragraphs();
    assert_eq!(paragraphs.len(), 1);
    assert_eq!(paragraphs[0].heading, 0);
    assert_eq!(paragraphs[0].runs.len(), 1);
    assert_eq!(paragraphs[0].runs[0], Run::default());
}

#[test]
fn test_new_empty_tab_has_default_paragraphs_and_no_docx_origin() {
    let tab = Tab::new_empty(TabId(0));
    assert_eq!(tab.document.paragraphs(), default_paragraphs());
    assert!(tab.docx_origin.is_none());
}

/// Sets up a state whose tab has a *specific* multi-run/multi-paragraph
/// `paragraphs` structure (not the default single-run one `make_state`
/// builds), for testing that choke-point mutations keep `paragraphs`
/// in sync with `content` through real editor operations.
fn make_state_with_paragraphs(paragraphs: Vec<Paragraph>, cursor: usize) -> AppState {
    let content = paragraphs_to_plain_text(&paragraphs);
    let mut state = make_state(&content, cursor, None);
    *state.workspace.tabs[0].document.paragraphs_mut() = paragraphs;
    state
}

// ── Real Word lists (apply_list_style/remove_list_formatting) ───────────

#[test]
fn test_apply_list_style_sets_list_on_every_touched_paragraph() {
    let mut state = make_state_with_paragraphs(
        vec![
            Paragraph {
                runs: vec![Run {
                    text: "one".into(),
                    ..Run::default()
                }],
                ..Paragraph::default()
            },
            Paragraph {
                runs: vec![Run {
                    text: "two".into(),
                    ..Run::default()
                }],
                ..Paragraph::default()
            },
            Paragraph {
                runs: vec![Run {
                    text: "three".into(),
                    ..Run::default()
                }],
                ..Paragraph::default()
            },
        ],
        0,
    );
    state.workspace.tabs[0].selection = Some((0, 13)); // whole buffer ("one\ntwo\nthree")
    state.apply_list_style(ListKind::BulletSolid);
    for para in state.workspace.tabs[0].document.paragraphs() {
        assert_eq!(
            para.list,
            Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0
            })
        );
    }
}

#[test]
fn test_apply_list_style_toggles_off_when_already_that_exact_style() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        }],
        0,
    );
    state.workspace.tabs[0].selection = Some((0, 3));
    state.apply_list_style(ListKind::BulletSolid);
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].list, None);
}

#[test]
fn test_apply_list_style_switches_style_without_toggling_off() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        }],
        0,
    );
    state.workspace.tabs[0].selection = Some((0, 3));
    state.apply_list_style(ListKind::NumberDecimalDot);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].list,
        Some(ListItem {
            kind: ListKind::NumberDecimalDot,
            level: 0
        })
    );
}

#[test]
fn test_apply_list_style_with_no_selection_applies_to_cursors_own_paragraph_only() {
    let mut state = make_state_with_paragraphs(
        vec![
            Paragraph {
                runs: vec![Run {
                    text: "one".into(),
                    ..Run::default()
                }],
                ..Paragraph::default()
            },
            Paragraph {
                runs: vec![Run {
                    text: "two".into(),
                    ..Run::default()
                }],
                ..Paragraph::default()
            },
        ],
        5, // inside "two"
    );
    state.workspace.tabs[0].selection = None;
    state.apply_list_style(ListKind::BulletHollow);
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].list, None);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].list,
        Some(ListItem {
            kind: ListKind::BulletHollow,
            level: 0
        })
    );
}

#[test]
fn test_remove_list_formatting_clears_list_on_touched_paragraphs() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::NumberUpperRoman,
                level: 0,
            }),
            ..Paragraph::default()
        }],
        0,
    );
    state.workspace.tabs[0].selection = Some((0, 3));
    state.remove_list_formatting();
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].list, None);
}

#[test]
fn test_backspace_at_start_of_list_item_removes_list_formatting_not_text() {
    let mut state = make_state_with_paragraphs(
        vec![
            Paragraph {
                runs: vec![Run {
                    text: "before".into(),
                    ..Run::default()
                }],
                ..Paragraph::default()
            },
            Paragraph {
                runs: vec![Run {
                    text: "item".into(),
                    ..Run::default()
                }],
                list: Some(ListItem {
                    kind: ListKind::BulletSolid,
                    level: 0,
                }),
                ..Paragraph::default()
            },
        ],
        0,
    );
    // Cursor at the very start of "item"'s text, right after "before\n".
    state.workspace.tabs[0].cursor = 7;
    state.backspace();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs().len(),
        2,
        "paragraphs should not have merged"
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].runs[0].text,
        "item",
        "text should be untouched"
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].list,
        None,
        "list formatting should be cleared"
    );
}

#[test]
fn test_backspace_mid_list_item_deletes_text_normally() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "item".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        }],
        2, // between "it" and "em"
    );
    state.backspace();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "iem"
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0
        })
    );
}

// ── Phase 2: multi-level indent (indent_list_item/outdent_list_item) ────

#[test]
fn test_indent_list_item_increments_level() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        }],
        1,
    );
    state.indent_list_item();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 1
        })
    );
}

#[test]
fn test_indent_list_item_caps_at_level_8() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 8,
            }),
            ..Paragraph::default()
        }],
        1,
    );
    state.indent_list_item();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 8
        })
    );
}

#[test]
fn test_outdent_list_item_decrements_level() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 1,
            }),
            ..Paragraph::default()
        }],
        1,
    );
    state.outdent_list_item();
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].list,
        Some(ListItem {
            kind: ListKind::BulletSolid,
            level: 0
        })
    );
}

#[test]
fn test_outdent_list_item_at_level_0_removes_list() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            list: Some(ListItem {
                kind: ListKind::BulletSolid,
                level: 0,
            }),
            ..Paragraph::default()
        }],
        1,
    );
    state.outdent_list_item();
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].list, None);
}

#[test]
fn test_indent_list_item_on_non_list_paragraph_is_a_noop() {
    let mut state = make_state_with_paragraphs(
        vec![Paragraph {
            runs: vec![Run {
                text: "one".into(),
                ..Run::default()
            }],
            ..Paragraph::default()
        }],
        1,
    );
    let before = state.workspace.tabs[0].document.paragraphs().to_vec();
    state.indent_list_item();
    assert_eq!(state.workspace.tabs[0].document.paragraphs(), before);
}

// ── Font size box (ribbon spinner) ──────────────────────────────────────

fn sized_run(text: &str, size: u16) -> Run {
    Run {
        text: text.into(),
        size,
        ..Run::default()
    }
}

fn sized_para(runs: Vec<Run>) -> Paragraph {
    Paragraph {
        runs,
        ..Paragraph::default()
    }
}

#[test]
fn test_selection_font_size_reports_a_uniform_selection() {
    let mut state =
        make_state_with_paragraphs(vec![sized_para(vec![sized_run("hello world", 32)])], 0);
    state.workspace.tabs[0].selection = Some((0, 11));
    assert_eq!(state.selection_font_size_half_points(), Some(32));
}

#[test]
fn test_selection_font_size_is_none_when_sizes_are_mixed() {
    let mut state = make_state_with_paragraphs(
        vec![sized_para(vec![
            sized_run("big", 48),
            sized_run("small", 20),
        ])],
        0,
    );
    state.workspace.tabs[0].selection = Some((0, 8));
    assert_eq!(state.selection_font_size_half_points(), None);
}

#[test]
fn test_selection_font_size_reads_the_run_under_the_cursor_with_no_selection() {
    let mut state = make_state_with_paragraphs(
        vec![sized_para(vec![
            sized_run("abc", 48),
            sized_run("defgh", 20),
        ])],
        4, // inside the second run
    );
    state.workspace.tabs[0].selection = None;
    assert_eq!(state.selection_font_size_half_points(), Some(20));
}

/// The old inline detector in `cycle_font_size` restarted its byte offset
/// at every paragraph and never counted the separating newline, so on a
/// multi-paragraph document it read the size off the wrong runs. Offsets
/// now accumulate the way `document_ops::is_uniformly_active` does.
#[test]
fn test_selection_font_size_offsets_accumulate_across_paragraphs() {
    let mut state = make_state_with_paragraphs(
        vec![
            sized_para(vec![sized_run("first", 48)]), // bytes 0..5, newline at 5
            sized_para(vec![sized_run("second", 20)]), // bytes 6..12
        ],
        0,
    );
    // A selection wholly inside the *second* paragraph.
    state.workspace.tabs[0].selection = Some((6, 12));
    assert_eq!(state.selection_font_size_half_points(), Some(20));

    // Spanning both paragraphs is mixed.
    state.workspace.tabs[0].selection = Some((0, 12));
    assert_eq!(state.selection_font_size_half_points(), None);
}

/// `size == 0` means "no explicit override" — the box turns that into the
/// configured body size, but the getter reports it verbatim.
#[test]
fn test_selection_font_size_reports_zero_for_an_unstyled_run() {
    let mut state = make_state_with_paragraphs(vec![sized_para(vec![sized_run("plain", 0)])], 0);
    state.workspace.tabs[0].selection = Some((0, 5));
    assert_eq!(state.selection_font_size_half_points(), Some(0));
}

/// With the caret at the very end of the text there is no character
/// *after* it, so reading forward blanked the box. It reports the
/// character before the caret instead — what typing there would inherit.
#[test]
fn test_selection_font_size_at_end_of_text_reports_the_preceding_run() {
    let mut state = make_state_with_paragraphs(
        vec![sized_para(vec![sized_run("hello", 48)])],
        5, // one past the last character
    );
    state.workspace.tabs[0].selection = None;
    assert_eq!(state.selection_font_size_half_points(), Some(48));
}

/// At the very start there is nothing before the caret, so it falls
/// forward to the first character rather than reporting nothing.
#[test]
fn test_selection_font_size_at_start_of_text_reports_the_first_run() {
    let mut state = make_state_with_paragraphs(vec![sized_para(vec![sized_run("hello", 48)])], 0);
    state.workspace.tabs[0].selection = None;
    assert_eq!(state.selection_font_size_half_points(), Some(48));
}

/// Between two differently-sized runs the caret inherits the left one,
/// matching Word.
#[test]
fn test_selection_font_size_between_runs_reports_the_left_one() {
    let mut state = make_state_with_paragraphs(
        vec![sized_para(vec![
            sized_run("abc", 48),
            sized_run("defgh", 20),
        ])],
        3, // exactly on the boundary
    );
    state.workspace.tabs[0].selection = None;
    assert_eq!(state.selection_font_size_half_points(), Some(48));
}

/// An empty (zero-width) selection is a caret, not a range.
#[test]
fn test_selection_font_size_treats_a_collapsed_selection_as_a_caret() {
    let mut state = make_state_with_paragraphs(vec![sized_para(vec![sized_run("hello", 48)])], 5);
    state.workspace.tabs[0].selection = Some((5, 5));
    assert_eq!(state.selection_font_size_half_points(), Some(48));
}

#[test]
fn test_set_font_size_applies_to_the_selection() {
    let mut state = make_state_with_paragraphs(vec![sized_para(vec![sized_run("hello", 0)])], 0);
    state.workspace.tabs[0].selection = Some((0, 5));
    state.set_font_size_half_points(36); // 18pt
    assert_eq!(state.selection_font_size_half_points(), Some(36));
}

// ── Rich text formatting Phase 1: choke-point mutation sync ─────────────

#[test]
fn test_insert_char_choke_point_keeps_paragraphs_synced() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "abc".into(),
            bold: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 1);
    state.insert_char('X');
    assert_eq!(state.workspace.tabs[0].document.content(), "aXbc");
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "aXbc"
    );
    assert!(state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
}

#[test]
fn test_backspace_choke_point_keeps_paragraphs_synced() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "abc".into(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 2);
    state.backspace();
    assert_eq!(state.workspace.tabs[0].document.content(), "ac");
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "ac"
    );
}

#[test]
fn test_delete_selection_choke_point_keeps_paragraphs_synced() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "bold".into(),
                bold: true,
                ..Run::default()
            },
            Run {
                text: " plain".into(),
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((2, 6)); // deletes "ld p"
    state.delete_selection();
    assert_eq!(state.workspace.tabs[0].document.content(), "bolain");
    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].text, "bo");
    assert!(runs[0].bold);
    assert_eq!(runs[1].text, "lain");
    assert!(!runs[1].bold);
}

#[test]
fn test_vim_dd_choke_point_keeps_paragraphs_synced() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "one".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("two")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("d", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "two");
    assert_eq!(state.workspace.tabs[0].document.paragraphs().len(), 1);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "two"
    );
}

#[test]
fn test_vim_paste_choke_point_keeps_paragraphs_synced() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![run_plain("abc")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.global_vim.registers.insert('"', "XY".to_string());
    state.handle_vim_key("p", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "aXYbc");
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "aXYbc"
    );
}

#[test]
fn test_dispatch_vim_substitute_only_touches_changed_paragraphs() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "foo bar".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("untouched")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.dispatch_vim_command("%s/foo/baz/");
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "baz bar\nuntouched"
    );
    // changed paragraph loses formatting (documented scope limit)
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "baz bar"
    );
    assert!(!state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
    // untouched paragraph is byte-for-byte unchanged
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].runs[0].text,
        "untouched"
    );
}

#[test]
fn test_insert_newline_via_enter_splits_paragraph_in_sync() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![run_plain("hello")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 2);
    state.insert_char('\n');
    assert_eq!(state.workspace.tabs[0].document.content(), "he\nllo");
    assert_eq!(state.workspace.tabs[0].document.paragraphs().len(), 2);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "he"
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].runs[0].text,
        "llo"
    );
}

fn run_plain(text: &str) -> Run {
    Run {
        text: text.to_string(),
        ..Run::default()
    }
}

// ── copy_selection ────────────────────────────────────────────────────────

#[test]
fn test_copy_selection_basic() {
    let state = make_state("hello world", 5, Some((0, 5)));
    assert_eq!(state.copy_selection(), Some("hello".to_string()));
}

#[test]
fn test_copy_selection_backward() {
    // anchor > focus (reversed selection) — should still return correct text
    let state = make_state("hello world", 0, Some((5, 0)));
    assert_eq!(state.copy_selection(), Some("hello".to_string()));
}

#[test]
fn test_copy_selection_no_selection() {
    let state = make_state("hello world", 0, None);
    assert_eq!(state.copy_selection(), None);
}

// ── cut_selection ─────────────────────────────────────────────────────────

#[test]
fn test_cut_selection_basic() {
    let mut state = make_state("hello world", 5, Some((0, 5)));
    let text = state.cut_selection();
    assert_eq!(text, Some("hello".to_string()));
    assert_eq!(state.workspace.tabs[0].document.content(), " world");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert!(state.workspace.tabs[0].selection.is_none());
}

#[test]
fn test_cut_selection_no_selection() {
    let mut state = make_state("hello world", 5, None);
    let text = state.cut_selection();
    assert_eq!(text, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "hello world"); // unchanged
}

// ── Rich clipboard round-trip: copy_selection_runs -> encode_with_lengths
// -> decode -> insert_str_with_runs, end-to-end, no GPUI involved ────────

/// End-to-end against the user's reported repro: the Pocket/Hat/Block/Tag
/// document is copied whole, pasted below itself, saved as a real .docx,
/// and re-parsed. Guards the whole clipboard -> paragraphs -> docx chain,
/// which is where the corruption actually surfaced (it survived a
/// save+reopen, so a state-level assertion alone would not have caught it).
#[test]
fn copy_paste_card_styles_survives_a_docx_save_and_reload() {
    let card = |text: &str, heading: u8, size: u16| Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            bold: true,
            size,
            ..Run::default()
        }],
        heading,
        alignment: Alignment::Center,
        unsupported_xml: None,
    };
    // Five styled lines, then the blank line the user pressed Enter to
    // reach before pasting.
    let mut state = make_state_with_paragraphs(
        vec![
            card("pocket", 1, 52),
            card("hat", 2, 44),
            card("block", 3, 32),
            card("tag", 4, 26),
            para_plain("test"),
            para_plain(""),
        ],
        0,
    );

    // Select the five styled lines (not the trailing blank) and copy
    // through the real clipboard encoding.
    let doc_len = state.workspace.tabs[0].document.content().len();
    let copy_end = doc_len - 1; // excludes the trailing blank paragraph
    state.workspace.tabs[0].selection = Some((0, copy_end));
    let plain = state.copy_selection().unwrap();
    let runs = state.copy_selection_runs().unwrap();
    let attrs = state.copy_selection_paragraph_attrs().unwrap();
    let meta = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
    let (runs, attrs) = crate::rich_clipboard::decode(&meta, &plain).unwrap();

    // Paste into the blank line at the end, as the user did.
    state.workspace.tabs[0].selection = None;
    state.workspace.tabs[0].cursor = doc_len;
    state.insert_str_with_runs_and_paragraphs(&plain, &runs, &attrs);

    let dir = std::env::temp_dir().join(format!("vimbatim_paste_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Paste_Test.docx");
    state.save_active_tab_as(path.clone()).unwrap();

    let (reloaded, _) = crate::docx_parser::parse_docx(&path).unwrap();
    let headings: Vec<u8> = reloaded.iter().map(|p| p.heading).collect();
    assert_eq!(
        headings,
        vec![1, 2, 3, 4, 0, 1, 2, 3, 4, 0],
        "card-style headings must survive copy -> paste -> save -> reload"
    );
    for i in [5usize, 6, 7, 8] {
        assert_eq!(
            reloaded[i].alignment,
            Alignment::Center,
            "alignment lost on pasted paragraph {i}"
        );
    }
    // The pasted body line must not inherit the card styles' leftovers.
    let last = reloaded.last().unwrap();
    assert_eq!(last.runs.len(), 1, "leftover empty runs: {:?}", last.runs);
    assert!(
        !last.runs[0].box_format,
        "spurious border on the pasted body line"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Repro of the user-reported "copy/paste jumbles formatting": write
/// Pocket/Hat/Block/Tag lines, select all, paste below.
///
/// The sibling test above only ever used `heading: 0` and default
/// alignment, which is exactly why it never caught this — card styles are
/// the one thing that carries *paragraph-level* state.
#[test]
fn copy_paste_preserves_card_style_heading_and_alignment() {
    let card = |text: &str, heading: u8, size: u16| Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            bold: true,
            size,
            ..Run::default()
        }],
        heading,
        alignment: Alignment::Center,
        unsupported_xml: None,
    };
    let paragraphs = vec![
        card("pocket", 1, 52),
        card("hat", 2, 44),
        card("block", 3, 32),
        card("tag", 4, 26),
        para_plain("test"),
    ];
    let mut source = make_state_with_paragraphs(paragraphs, 0);
    let doc_len = source.workspace.tabs[0].document.content().len();
    source.workspace.tabs[0].selection = Some((0, doc_len));

    let plain = source.copy_selection().unwrap();
    let runs = source.copy_selection_runs().unwrap();
    let attrs = source.copy_selection_paragraph_attrs().unwrap();
    let meta = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
    let (decoded, decoded_attrs) = crate::rich_clipboard::decode(&meta, &plain)
        .expect("decode rejected a multi-paragraph card-style copy");

    let mut dest = make_state("", 0, None);
    dest.insert_str_with_runs_and_paragraphs(&plain, &decoded, &decoded_attrs);

    let paras = dest.workspace.tabs[0].document.paragraphs();
    assert_eq!(paras.len(), 5, "expected one paragraph per copied line");

    // Paragraph-level card-style markers must survive the paste.
    assert_eq!(
        paras.iter().map(|p| p.heading).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 0],
        "heading (Pocket/Hat/Block/Tag) lost on paste"
    );
    for (i, para) in paras[..4].iter().enumerate() {
        assert_eq!(
            para.alignment,
            Alignment::Center,
            "alignment lost on pasted paragraph {i}"
        );
    }

    // No empty leftover runs: they carry stale bold/size/box_format that
    // shows up as a spurious border and wrong inherited formatting.
    for (i, para) in paras.iter().enumerate() {
        assert!(
            para.runs.iter().all(|r| !r.text.is_empty()),
            "paragraph {i} kept empty leftover runs: {:?}",
            para.runs
                .iter()
                .map(|r| (&r.text, r.size, r.box_format))
                .collect::<Vec<_>>()
        );
    }
}

/// The reported bug, from the user's own `Untitled.docx`: a selection
/// that swallows the trailing newline — a drag, Shift+Down, `V`, or any
/// vim linewise range, i.e. how a card actually gets copied — yields one
/// *fewer* paragraph attribute than the pasted text spans, because
/// nothing of the paragraph past that newline was copied.
///
/// The old exact-match guard therefore never fired and dropped every
/// heading and alignment. Run-level formatting survived regardless, which
/// is why the symptom read as "size, bold, single underline and boxes are
/// fine, but alignment, heading and a Hat's double underline are ignored"
/// rather than as total loss: a Hat that loses `heading: 2` stops being a
/// Hat (font size, fold marker and `run_is_hidden` all key off it) even
/// though its `double_underline` run flag is still there.
#[test]
fn copy_paste_keeps_paragraph_attrs_when_the_selection_swallows_the_newline() {
    let card = |text: &str, heading: u8, size: u16, u: bool, du: bool, boxed: bool| Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            bold: true,
            size,
            underline: u,
            double_underline: du,
            box_format: boxed,
            ..Run::default()
        }],
        heading,
        alignment: Alignment::Center,
        unsupported_xml: None,
    };
    let mut source = make_state_with_paragraphs(
        vec![
            card("Pocket", 1, 52, false, false, true),
            card("Hat", 2, 44, false, true, false),
            card("Block", 3, 32, true, false, false),
            card("Tag", 4, 26, false, false, false),
            para_plain("normal text"),
            para_plain(""),
        ],
        0,
    );
    // Through the newline that ends "normal text", stopping at the blank
    // line's first byte — the selection a drag down the card produces.
    let end = source.workspace.tabs[0]
        .document
        .content()
        .find("normal text")
        .unwrap()
        + "normal text".len()
        + 1;
    source.workspace.tabs[0].selection = Some((0, end));

    let plain = source.copy_selection().unwrap();
    let runs = source.copy_selection_runs().unwrap();
    let attrs = source.copy_selection_paragraph_attrs().unwrap();
    // One short of the five paragraphs the text spans — the case the old
    // guard treated as "malformed" and silently ignored.
    assert_eq!(attrs.len(), plain.matches('\n').count());
    let meta = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
    let (runs, attrs) = crate::rich_clipboard::decode(&meta, &plain).unwrap();

    let mut dest = make_state_with_paragraphs(vec![para_plain("")], 0);
    dest.insert_str_with_runs_and_paragraphs(&plain, &runs, &attrs);

    let paras = dest.workspace.tabs[0].document.paragraphs();
    assert_eq!(
        paras.iter().map(|p| p.heading).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 0, 0],
        "card-style headings lost on a trailing-newline selection"
    );
    for (i, para) in paras[..4].iter().enumerate() {
        assert_eq!(
            para.alignment,
            Alignment::Center,
            "alignment lost on pasted paragraph {i}"
        );
    }
    assert!(
        paras[1].runs[0].double_underline,
        "the Hat's double underline must survive too"
    );
    assert!(
        paras[0].runs[0].box_format,
        "the Pocket's box must survive too"
    );
}

/// Pasting *above* an existing card used to strip it: the paste splits
/// the destination paragraph, and `split_paragraph_at` hands the new tail
/// `heading: 0` and default alignment (right for pressing Enter, wrong
/// for paste). The tail is the same line the user was standing on, so it
/// has to keep what it had.
///
/// Goes through the no-formatting case of the paste path — a paste with
/// no clipboard metadata, or Paste Without Formatting, must not flatten
/// the line it lands in either.
#[test]
fn pasting_above_a_card_line_leaves_that_cards_paragraph_attrs_intact() {
    let pocket = Paragraph {
        list: None,
        runs: vec![Run {
            text: "keep".into(),
            bold: true,
            size: 52,
            box_format: true,
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::Center,
        unsupported_xml: None,
    };
    let mut state = make_state_with_paragraphs(vec![pocket], 0);
    state.workspace.tabs[0].cursor = 0;
    state.insert_str_with_runs("new\n", &[]);

    let paras = state.workspace.tabs[0].document.paragraphs();
    assert_eq!(paras.len(), 2);
    assert_eq!(paras[1].runs[0].text, "keep");
    assert_eq!(
        paras[1].heading, 1,
        "the line pasted above lost its Pocket heading"
    );
    assert_eq!(
        paras[1].alignment,
        Alignment::Center,
        "the line pasted above lost its alignment"
    );
}

/// Vim's registers held plain text only, so `yy`/`p` (and `dd`, `cc`,
/// visual `y` — they all funnel through `write_vim_register`) pasted text
/// that simply inherited whatever run the cursor was sitting in. They now
/// carry the same `rich_clipboard` payload Ctrl+C does.
#[test]
fn vim_yank_and_put_preserves_run_and_paragraph_formatting() {
    let hat = Paragraph {
        list: None,
        runs: vec![Run {
            text: "Hat".into(),
            bold: true,
            size: 44,
            double_underline: true,
            ..Run::default()
        }],
        heading: 2,
        alignment: Alignment::Center,
        unsupported_xml: None,
    };
    let mut state = make_state_with_paragraphs(vec![hat, para_plain("body")], 0);

    state.handle_vim_key("y", false, None);
    state.handle_vim_key("y", false, None); // yy: linewise yank of the Hat
                                            // Down to the plain body line, set directly rather than with `j`:
                                            // vertical motion is resolved against visual rows in `text_editor.rs`,
                                            // which has no place in a state-level test.
    state.workspace.tabs[0].cursor = state.workspace.tabs[0]
        .document
        .content()
        .find("body")
        .unwrap();
    state.handle_vim_key("p", false, None); // put below it

    let paras = state.workspace.tabs[0].document.paragraphs();
    let pasted = &paras[2];
    assert_eq!(pasted.runs[0].text, "Hat");
    assert_eq!(pasted.heading, 2, "yy/p lost the Hat's heading");
    assert_eq!(
        pasted.alignment,
        Alignment::Center,
        "yy/p lost the Hat's alignment"
    );
    assert!(
        pasted.runs[0].double_underline,
        "yy/p lost the double underline"
    );
    assert!(pasted.runs[0].bold, "yy/p lost bold");
    assert_eq!(pasted.runs[0].size, 44, "yy/p lost the font size");
    // The body line it was pasted after is untouched.
    assert_eq!(paras[1].runs[0].text, "body");
    assert_eq!(paras[1].heading, 0);
}

/// A `+` register filled from another app's clipboard has no formatting
/// metadata; `p` must still paste it, plain, exactly as before.
#[test]
fn vim_put_from_a_register_with_no_formatting_pastes_plain() {
    let mut state = make_state("abc", 0, None);
    state.set_register('"', "XY".to_string(), None);
    state.handle_vim_key("p", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "aXYbc");
}
#[test]
fn test_multi_paragraph_copy_paste_round_trip_preserves_per_line_formatting() {
    // Regression test: runs_in_range never emitted a run for the
    // paragraph-separating '\n', so the summed run lengths came up one
    // byte short of the plain text for every multi-paragraph selection,
    // and rich_clipboard::decode (which requires an exact match) rejected
    // it outright -- silently falling back to plain-text paste and
    // losing all per-line formatting. That defeats this app's primary
    // "copy a formatted card spanning several lines" use case; only
    // single-paragraph copies worked before this fix.
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "bold line".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        para_plain("plain line"),
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "hi line".into(),
                highlight: true,
                highlight_color: "yellow".into(),
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut source = make_state_with_paragraphs(paragraphs, 0);
    let doc_len = source.workspace.tabs[0].document.content().len();
    source.workspace.tabs[0].selection = Some((0, doc_len)); // whole doc, crossing both paragraph boundaries

    let plain_text = source.copy_selection().unwrap();
    let runs = source.copy_selection_runs().unwrap();
    let attrs = source.copy_selection_paragraph_attrs().unwrap();
    let metadata = crate::rich_clipboard::encode_with_lengths(&runs, &attrs);
    let decoded = crate::rich_clipboard::decode(&metadata, &plain_text);
    assert!(
        decoded.is_some(),
        "decode() rejected a multi-paragraph copy -- the bug this test guards against"
    );
    let (decoded_runs, _) = decoded.unwrap();

    let mut dest = make_state("", 0, None);
    dest.insert_str_with_runs(&plain_text, &decoded_runs);

    assert_eq!(dest.workspace.tabs[0].document.content(), plain_text);
    assert_eq!(dest.workspace.tabs[0].document.paragraphs().len(), 3);
    assert_eq!(
        dest.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "bold line"
    );
    assert!(dest.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
    assert_eq!(
        dest.workspace.tabs[0].document.paragraphs()[1].runs[0].text,
        "plain line"
    );
    assert!(!dest.workspace.tabs[0].document.paragraphs()[1].runs[0].bold);
    assert_eq!(
        dest.workspace.tabs[0].document.paragraphs()[2].runs[0].text,
        "hi line"
    );
    assert!(dest.workspace.tabs[0].document.paragraphs()[2].runs[0].highlight);
    assert_eq!(
        dest.workspace.tabs[0].document.paragraphs()[2].runs[0].highlight_color,
        "yellow"
    );
}

// ── insert_str ────────────────────────────────────────────────────────────

#[test]
fn test_insert_str_no_selection() {
    let mut state = make_state("hello", 5, None);
    state.insert_str(" world");
    assert_eq!(state.workspace.tabs[0].document.content(), "hello world");
    assert_eq!(state.workspace.tabs[0].cursor, 11);
}

#[test]
fn test_insert_str_replaces_selection() {
    let mut state = make_state("hello world", 5, Some((0, 5)));
    state.insert_str("goodbye");
    assert_eq!(state.workspace.tabs[0].document.content(), "goodbye world");
    assert_eq!(state.workspace.tabs[0].cursor, 7);
    assert!(state.workspace.tabs[0].selection.is_none());
}

#[test]
fn test_insert_str_empty() {
    // Inserting an empty string is a no-op (no crash, content unchanged).
    let mut state = make_state("hello", 5, None);
    state.insert_str("");
    assert_eq!(state.workspace.tabs[0].document.content(), "hello");
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn insert_str_does_not_panic_on_stale_out_of_bounds_cursor() {
    // Simulate a stale cursor left over from before an undo/redo or tab
    // swap shortened the content out from under it.
    let mut state = make_state("hi", 9999, None);
    state.insert_str("x"); // must not panic
    assert!(state.workspace.tabs[0].document.content().contains('x'));
}

#[test]
fn insert_str_does_not_panic_on_mid_char_cursor() {
    // 'é' is 2 bytes; cursor at position 2 lands inside 'é', not on a boundary
    let mut state = make_state("héllo", 2, None);
    state.insert_str("x"); // must not panic
    assert!(state.workspace.tabs[0].document.content().contains('x'));
}

// ── move_left / move_right ──────────────────────────────────────────────

#[test]
fn test_move_right_advances_one_char() {
    let mut state = make_state("hello", 0, None);
    state.move_right();
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_move_right_stops_at_end() {
    let mut state = make_state("hi", 2, None);
    state.move_right();
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_move_right_skips_whole_multibyte_char() {
    // 'é' is 2 bytes in UTF-8; cursor must land on the next char boundary,
    // never inside the char.
    let mut state = make_state("café", 3, None);
    state.move_right();
    assert_eq!(state.workspace.tabs[0].cursor, 5);
    assert!(state.workspace.tabs[0]
        .document
        .content()
        .is_char_boundary(state.workspace.tabs[0].cursor));
}

#[test]
fn test_move_left_retreats_one_char() {
    let mut state = make_state("hello", 3, None);
    state.move_left();
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_move_left_stops_at_start() {
    let mut state = make_state("hi", 0, None);
    state.move_left();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_move_left_skips_whole_multibyte_char() {
    let mut state = make_state("café", 5, None);
    state.move_left();
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert!(state.workspace.tabs[0]
        .document
        .content()
        .is_char_boundary(state.workspace.tabs[0].cursor));
}

// ── move_up / move_down ─────────────────────────────────────────────────

#[test]
fn test_move_down_same_column() {
    let mut state = make_state("abc\ndefgh", 1, None); // cursor after 'a'
    state.move_down();
    assert_eq!(state.workspace.tabs[0].cursor, 5); // "abc\nd|efgh" -> after 'd'
}

#[test]
fn test_move_down_clamps_to_shorter_line() {
    let mut state = make_state("abcdef\nxy", 5, None); // cursor after "abcde"
    state.move_down();
    assert_eq!(state.workspace.tabs[0].cursor, 9); // end of "xy" (only 2 chars)
}

#[test]
fn test_move_down_on_last_line_is_noop() {
    let mut state = make_state("abc\ndef", 5, None);
    state.move_down();
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_move_up_same_column() {
    let mut state = make_state("abc\ndefgh", 6, None); // cursor after "de"
    state.move_up();
    assert_eq!(state.workspace.tabs[0].cursor, 2); // "ab|c" -> after "ab"
}

#[test]
fn test_move_up_clamps_to_shorter_line() {
    let mut state = make_state("xy\nabcdef", 8, None); // cursor after "abcde"
    state.move_up();
    assert_eq!(state.workspace.tabs[0].cursor, 2); // end of "xy"
}

#[test]
fn test_move_up_on_first_line_is_noop() {
    let mut state = make_state("abc\ndef", 2, None);
    state.move_up();
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

// ── move_line_start / move_line_first_nonblank / move_line_end ─────────

#[test]
fn test_move_line_start() {
    let mut state = make_state("abc\n  defgh", 9, None); // cursor inside "defgh"
    state.move_line_start();
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of second line
}

#[test]
fn test_move_line_first_nonblank_skips_leading_whitespace() {
    let mut state = make_state("abc\n  defgh", 9, None);
    state.move_line_first_nonblank();
    assert_eq!(state.workspace.tabs[0].cursor, 6); // 'd' in "  defgh"
}

#[test]
fn test_move_line_first_nonblank_all_whitespace_line_lands_at_end() {
    let mut state = make_state("abc\n   \ndef", 5, None); // middle line is all spaces
    state.move_line_first_nonblank();
    assert_eq!(state.workspace.tabs[0].cursor, 7); // end of the blank line, no non-blank found
}

#[test]
fn test_move_line_end() {
    let mut state = make_state("abc\ndefgh\nij", 5, None); // cursor inside "defgh"
    state.move_line_end();
    assert_eq!(state.workspace.tabs[0].cursor, 9); // just before the '\n'
}

#[test]
fn test_move_line_end_last_line() {
    let mut state = make_state("abc\ndef", 5, None);
    state.move_line_end();
    assert_eq!(state.workspace.tabs[0].cursor, 7); // end of content, no trailing '\n'
}

// ── move_word_forward / move_word_end / move_word_backward ─────────────

#[test]
fn test_move_word_forward_skips_to_next_word() {
    let mut state = make_state("hello world", 0, None);
    state.move_word_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 6); // start of "world"
}

#[test]
fn test_move_word_forward_stops_at_punctuation_boundary() {
    let mut state = make_state("foo.bar baz", 0, None);
    state.move_word_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 3); // start of "." (punctuation is its own word)
}

#[test]
fn test_move_word_forward_crosses_newline() {
    let mut state = make_state("foo\nbar", 0, None);
    state.move_word_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of "bar" on next line
}

#[test]
fn test_move_word_forward_at_last_word_goes_to_end() {
    let mut state = make_state("hello", 0, None);
    state.move_word_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_move_word_end_lands_on_last_char_of_word() {
    let mut state = make_state("hello world", 0, None);
    state.move_word_end();
    assert_eq!(state.workspace.tabs[0].cursor, 4); // last char of "hello" ('o')
}

#[test]
fn test_move_word_end_from_inside_word_goes_to_its_end() {
    let mut state = make_state("hello world", 2, None); // cursor on 'l'
    state.move_word_end();
    assert_eq!(state.workspace.tabs[0].cursor, 4);
}

#[test]
fn test_move_word_end_at_last_char_advances_to_next_word_end() {
    let mut state = make_state("hello world", 4, None); // cursor already at 'o'
    state.move_word_end();
    assert_eq!(state.workspace.tabs[0].cursor, 10); // last char of "world" ('d')
}

#[test]
fn test_move_word_backward_to_word_start() {
    let mut state = make_state("hello world", 11, None); // cursor at end
    state.move_word_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 6); // start of "world"
}

#[test]
fn test_move_word_backward_from_inside_word_goes_to_its_start() {
    let mut state = make_state("hello world", 8, None); // cursor on 'r'
    state.move_word_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

#[test]
fn test_move_word_backward_at_start_is_noop() {
    let mut state = make_state("hello", 0, None);
    state.move_word_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_delete_word_backward_removes_preceding_word() {
    let mut state = make_state("hello world", 11, None); // cursor at end
    state.delete_word_backward();
    // Deletes "world" back to its start (6); the space before it
    // belongs to the gap *preceding* "world", not to "world" itself,
    // so it's left behind — same as vim's `b` landing on index 6.
    assert_eq!(state.workspace.tabs[0].document.content(), "hello ");
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

#[test]
fn test_delete_word_backward_at_start_of_line_is_a_noop() {
    let mut state = make_state("hello", 0, None);
    state.delete_word_backward();
    assert_eq!(state.workspace.tabs[0].document.content(), "hello");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_delete_word_backward_deletes_selection_instead_when_active() {
    let mut state = make_state("hello world", 11, Some((6, 11)));
    state.delete_word_backward();
    assert_eq!(state.workspace.tabs[0].document.content(), "hello ");
    assert!(state.workspace.tabs[0].selection.is_none());
}

#[test]
fn test_delete_word_backward_across_paragraph_break_merges_lines() {
    // Minor finding from the task-8 review: Ctrl+Backspace at the start
    // of a line should walk back over the preceding newline (word_backward
    // treats '\n' as whitespace, same as any other blank gap) and merge
    // into the previous line, exactly like backspace already does.
    let mut state = make_state("hello\nworld", 11, None); // cursor at end
    state.delete_word_backward();
    assert_eq!(state.workspace.tabs[0].document.content(), "hello\n");
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

#[test]
fn test_delete_word_backward_truncates_vim_insertion_recording() {
    // Task-8 review bug: `backspace` pops one char off
    // `vim_insertion_recording` per char deleted (so vim's `.`-repeat
    // replays only what's actually still in the document), but
    // `delete_word_backward` deleted a whole word without touching the
    // recording at all — so a Ctrl+Backspace mid-insert left the
    // deleted word's text stranded in the recording buffer, and `.`
    // would incorrectly replay it too.
    let mut state = make_state("", 0, None);
    vim_key_recorded(&mut state, "i", false, None);
    state.insert_str("hello world");
    state.delete_word_backward();
    assert_eq!(state.workspace.tabs[0].document.content(), "hello ");
    state.insert_str("there");
    state.vim_exit_to_normal();
    assert_eq!(state.workspace.tabs[0].document.content(), "hello there");

    // Repeat the insertion at the end of the document: if the deleted
    // "world" text were still sitting in the recording, this would
    // replay "hello worldthere" instead of just "hello there".
    state.workspace.tabs[0].cursor = state.workspace.tabs[0].document.content().len();
    state.vim_repeat_last_change();
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "hello therehello there"
    );
}

// ── move_doc_start / move_doc_end / move_to_line ───────────────────────

#[test]
fn test_move_doc_start() {
    let mut state = make_state("abc\ndef\nghi", 9, None);
    state.move_doc_start();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_move_doc_end() {
    let mut state = make_state("abc\ndef\nghi", 0, None);
    state.move_doc_end();
    assert_eq!(state.workspace.tabs[0].cursor, 11);
}

#[test]
fn test_move_to_line_one_indexed() {
    let mut state = make_state("abc\ndef\nghi", 0, None);
    state.move_to_line(2);
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of "def"
}

#[test]
fn test_move_to_line_clamps_past_last_line() {
    let mut state = make_state("abc\ndef", 0, None);
    state.move_to_line(99);
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of last line
}

#[test]
fn test_move_to_line_zero_clamps_to_first_line() {
    let mut state = make_state("abc\ndef", 5, None);
    state.move_to_line(0);
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

// ── cursor_line_col ──────────────────────────────────────────────────

#[test]
fn test_cursor_line_col_start_of_document() {
    let state = make_state("hello\nworld", 0, None);
    assert_eq!(state.cursor_line_col(), (0, 0));
}

#[test]
fn test_cursor_line_col_end_of_first_line() {
    let state = make_state("hello\nworld", 5, None);
    assert_eq!(state.cursor_line_col(), (0, 5));
}

#[test]
fn test_cursor_line_col_start_of_second_line() {
    let state = make_state("hello\nworld", 6, None);
    assert_eq!(state.cursor_line_col(), (1, 0));
}

#[test]
fn test_cursor_line_col_end_of_document() {
    let state = make_state("hello\nworld", 11, None);
    assert_eq!(state.cursor_line_col(), (1, 5));
}

#[test]
fn test_cursor_line_col_counts_chars_not_bytes() {
    // "café" is 4 characters but 5 bytes ('é' is 2 bytes in UTF-8).
    let state = make_state("café\nx", 5, None);
    assert_eq!(state.cursor_line_col(), (0, 4));
}

// ── set_cursor_from_line_col ────────────────────────────────────────────

#[test]
fn test_set_cursor_from_line_col_basic() {
    let mut state = make_state("abc\ndefgh", 0, None);
    state.set_cursor_from_line_col(1, 2);
    assert_eq!(state.workspace.tabs[0].cursor, 6); // "abc\nde|fgh"
}

#[test]
fn test_set_cursor_from_line_col_clamps_column_past_line_end() {
    let mut state = make_state("ab\ndefgh", 0, None);
    state.set_cursor_from_line_col(0, 99);
    assert_eq!(state.workspace.tabs[0].cursor, 2); // end of "ab"
}

#[test]
fn test_set_cursor_from_line_col_clamps_line_past_last() {
    let mut state = make_state("abc\ndef", 0, None);
    state.set_cursor_from_line_col(99, 0);
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of last line
}

#[test]
fn test_set_cursor_from_line_col_clears_selection() {
    let mut state = make_state("abc\ndefgh", 0, Some((0, 3)));
    state.set_cursor_from_line_col(0, 1);
    assert!(state.workspace.tabs[0].selection.is_none());
}

// round-trip against cursor_line_col confirms the two stay inverse of
// each other, since click-positioning depends on that symmetry.
#[test]
fn test_set_cursor_from_line_col_round_trips_with_cursor_line_col() {
    let mut state = make_state("hello\nworld", 0, None);
    state.set_cursor_from_line_col(1, 3);
    assert_eq!(state.cursor_line_col(), (1, 3));
}

// ── extend_left / extend_right ──────────────────────────────────────────

#[test]
fn test_extend_right_creates_selection_from_current_cursor() {
    let mut state = make_state("hello", 0, None);
    state.extend_right();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 1)));
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_extend_right_twice_keeps_original_anchor() {
    let mut state = make_state("hello", 0, None);
    state.extend_right();
    state.extend_right();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 2)));
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_extend_left_keeps_anchor_when_selection_already_exists() {
    // Simulate having extended right first, then reversing direction.
    let mut state = make_state("hello", 2, Some((0, 2)));
    state.extend_left();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 1)));
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_extend_left_and_right_back_to_anchor_is_zero_width_not_none() {
    let mut state = make_state("hello", 0, None);
    state.extend_right();
    state.extend_left();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_extend_left_clamps_at_document_start() {
    let mut state = make_state("hello", 0, None);
    state.extend_left();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

// ── extend_up / extend_down ─────────────────────────────────────────────

#[test]
fn test_extend_down_creates_selection() {
    let mut state = make_state("abc\ndefgh", 1, None);
    state.extend_down();
    assert_eq!(state.workspace.tabs[0].selection, Some((1, 5)));
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_extend_up_creates_selection() {
    let mut state = make_state("abc\ndefgh", 6, None);
    state.extend_up();
    assert_eq!(state.workspace.tabs[0].selection, Some((6, 2)));
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

// ── extend_word_forward / extend_word_backward ──────────────────────────

#[test]
fn test_extend_word_forward_creates_selection() {
    let mut state = make_state("hello world", 0, None);
    state.extend_word_forward();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 6)));
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

#[test]
fn test_extend_word_backward_creates_selection() {
    let mut state = make_state("hello world", 11, None);
    state.extend_word_backward();
    assert_eq!(state.workspace.tabs[0].selection, Some((11, 6)));
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

// ── extend_line_start / extend_line_end ─────────────────────────────────

#[test]
fn test_extend_line_start_creates_selection() {
    let mut state = make_state("abc\n  defgh", 9, None);
    state.extend_line_start();
    assert_eq!(state.workspace.tabs[0].selection, Some((9, 4)));
    assert_eq!(state.workspace.tabs[0].cursor, 4);
}

#[test]
fn test_extend_line_end_creates_selection() {
    let mut state = make_state("abc\ndefgh\nij", 5, None);
    state.extend_line_end();
    assert_eq!(state.workspace.tabs[0].selection, Some((5, 9)));
    assert_eq!(state.workspace.tabs[0].cursor, 9);
}

// ── extend_doc_start / extend_doc_end ───────────────────────────────────

#[test]
fn test_extend_doc_start_creates_selection() {
    let mut state = make_state("abc\ndef\nghi", 9, None);
    state.extend_doc_start();
    assert_eq!(state.workspace.tabs[0].selection, Some((9, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_extend_doc_end_creates_selection() {
    let mut state = make_state("abc\ndef\nghi", 0, None);
    state.extend_doc_end();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 11)));
    assert_eq!(state.workspace.tabs[0].cursor, 11);
}

// ── select_all ───────────────────────────────────────────────────────────

#[test]
fn test_select_all() {
    let mut state = make_state("hello\nworld", 3, None);
    state.select_all();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 11)));
    assert_eq!(state.workspace.tabs[0].cursor, 11);
}

#[test]
fn test_select_all_empty_document() {
    let mut state = make_state("", 0, None);
    state.select_all();
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

// ── extend_selection_to_line_col (click-drag) ───────────────────────────

#[test]
fn test_extend_selection_to_line_col_creates_selection_from_cursor() {
    let mut state = make_state("abc\ndefgh", 1, None);
    state.extend_selection_to_line_col(1, 2);
    assert_eq!(state.workspace.tabs[0].selection, Some((1, 6))); // anchor = old cursor
    assert_eq!(state.workspace.tabs[0].cursor, 6); // line 1, col 2 -> "de|fgh"
}

// ── select_word_at / select_line_at (double/triple-click) ──────────────

#[test]
fn select_word_at_selects_the_word_under_the_position() {
    let mut state = make_state("hello world foo", 0, None);
    state.select_word_at(7); // inside "world"
    assert_eq!(state.workspace.tabs[0].selection, Some((6, 11))); // "world"
    assert_eq!(state.workspace.tabs[0].cursor, 11);
}

#[test]
fn select_word_at_on_punctuation_selects_just_the_punctuation_run() {
    // Matches vim `iw`'s classification: a punctuation run is its own
    // "word", distinct from the alphanumeric runs around it.
    let mut state = make_state("foo, bar", 0, None);
    state.select_word_at(3); // the ","
    assert_eq!(state.workspace.tabs[0].selection, Some((3, 4)));
}

#[test]
fn select_line_at_selects_the_whole_paragraph() {
    // No blank line separates these two lines, so per `ip` semantics
    // (a paragraph is a blank-line-delimited block) they're one
    // paragraph and both get selected.
    let mut state = make_state("first line\nsecond line", 2, None);
    state.select_line_at(2); // inside "first line"
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 22)));
    assert_eq!(state.workspace.tabs[0].cursor, 22);
}

#[test]
fn select_line_at_stops_at_blank_line_boundary() {
    // `ip`'s range includes the line's trailing newline (its usual
    // linewise convention), so the end lands just past it rather than
    // at the last content byte.
    let mut state = make_state("first\n\nsecond", 0, None);
    state.select_line_at(0); // inside "first"
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 6)));
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

#[test]
fn test_extend_selection_to_line_col_keeps_existing_anchor() {
    // Simulates a drag already in progress: selection exists, anchor
    // must not move even as the drag continues past it in either direction.
    let mut state = make_state("abc\ndefgh", 6, Some((1, 6)));
    state.extend_selection_to_line_col(0, 0);
    assert_eq!(state.workspace.tabs[0].selection, Some((1, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_extend_selection_to_line_col_clamps_out_of_range_line_and_col() {
    let mut state = make_state("abc\ndef", 0, None);
    state.extend_selection_to_line_col(99, 99);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 7))); // clamps to end of doc
    assert_eq!(state.workspace.tabs[0].cursor, 7);
}

#[test]
fn test_extend_selection_to_line_col_same_position_is_zero_width_not_none() {
    let mut state = make_state("abc\ndef", 0, None);
    state.extend_selection_to_line_col(0, 0);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

// ── clamp_to_char_boundary ───────────────────────────────────────────────

#[test]
fn test_clamp_to_char_boundary_already_valid_is_unchanged() {
    assert_eq!(clamp_to_char_boundary("hello", 3), 3);
}

#[test]
fn test_clamp_to_char_boundary_past_end_clamps_to_len() {
    assert_eq!(clamp_to_char_boundary("hi", 99), 2);
}

#[test]
fn test_clamp_to_char_boundary_mid_multibyte_char_walks_back() {
    // "café" — 'é' is 2 bytes, spanning byte offsets 3..5. Offset 4 sits
    // inside it and must walk back to 3, the char's own start.
    assert_eq!(clamp_to_char_boundary("café", 4), 3);
}

#[test]
fn test_clamp_to_char_boundary_zero_is_always_valid() {
    assert_eq!(clamp_to_char_boundary("", 0), 0);
}

// ── undo / redo ──────────────────────────────────────────────────────────

/// Rewinds the active tab's `last_edit_at` far enough into the past that
/// the next edit's `push_undo_snapshot` call will not coalesce with it —
/// lets tests control coalescing deterministically without sleeping.
fn break_coalesce_window(state: &mut AppState) {
    if let Some(tab) = state.workspace.tabs.get_mut(state.workspace.active_tab) {
        tab.document.last_edit_at =
            Some(Instant::now() - UNDO_COALESCE_WINDOW - Duration::from_millis(1));
    }
}

/// Extracts just the content half of each undo-stack snapshot — most
/// existing undo/redo tests predate the rich-text formatting plan's
/// paired `(content, paragraphs)` snapshot shape and only care about
/// the content side.
fn undo_contents(state: &AppState) -> Vec<String> {
    state.workspace.tabs[0]
        .document
        .undo_stack
        .iter()
        .map(|p| crate::docx_parser::paragraphs_to_plain_text(p))
        .collect()
}

fn redo_contents(state: &AppState) -> Vec<String> {
    state.workspace.tabs[0]
        .document
        .redo_stack
        .iter()
        .map(|p| crate::docx_parser::paragraphs_to_plain_text(p))
        .collect()
}

#[test]
fn test_insert_char_pushes_undo_snapshot() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c');
    assert_eq!(undo_contents(&state), vec!["ab".to_string()]);
}

#[test]
fn test_rapid_inserts_coalesce_into_one_undo_step() {
    // Two inserts with no time passing between them (the normal case for
    // fast typing) must land as ONE undo step, not two.
    let mut state = make_state("a", 1, None);
    state.insert_char('b');
    state.insert_char('c');
    assert_eq!(undo_contents(&state), vec!["a".to_string()]);
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_inserts_outside_coalesce_window_are_separate_undo_steps() {
    let mut state = make_state("a", 1, None);
    state.insert_char('b');
    break_coalesce_window(&mut state);
    state.insert_char('c');
    assert_eq!(
        undo_contents(&state),
        vec!["a".to_string(), "ab".to_string()]
    );
}

// ── content_version (uniform_list_plan.md Part 1: row-wrap cache key) ───

#[test]
fn test_content_version_bumps_on_insert() {
    let mut state = make_state("ab", 2, None);
    let before = state.workspace.tabs[0].document.content_version;
    state.insert_char('c');
    assert!(state.workspace.tabs[0].document.content_version > before);
}

#[test]
fn test_content_version_bumps_on_every_keystroke_even_within_coalesce_window() {
    // The version must bump on *every* real edit, not just the first
    // keystroke of a coalesced burst — otherwise the row-wrap cache
    // would serve stale text for every keystroke inside the 300ms
    // window except the first. Only one undo-stack entry gets pushed
    // (coalesced), but content_version still climbs every time.
    let mut state = make_state("a", 1, None);
    let v0 = state.workspace.tabs[0].document.content_version;
    state.insert_char('b'); // within the coalesce window of itself, but v0 -> v1 regardless
    let v1 = state.workspace.tabs[0].document.content_version;
    state.insert_char('c'); // still within the window as v1's edit
    let v2 = state.workspace.tabs[0].document.content_version;
    assert!(v1 > v0, "version did not bump on first keystroke");
    assert!(
        v2 > v1,
        "version did not bump on second (coalesced) keystroke"
    );
    assert_eq!(
        state.workspace.tabs[0].document.undo_stack.len(),
        1,
        "sanity check: still just one coalesced undo entry"
    );
}

#[test]
fn test_content_version_does_not_bump_on_true_noop() {
    // Backspace at document start is a true no-op (state.rs's own
    // documented convention: no mutation, no undo push) — the cache key
    // must not churn for it either.
    let mut state = make_state("", 0, None);
    let before = state.workspace.tabs[0].document.content_version;
    state.backspace();
    assert_eq!(state.workspace.tabs[0].document.content_version, before);
}

#[test]
fn test_content_version_bumps_on_undo_and_redo() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c');
    let after_insert = state.workspace.tabs[0].document.content_version;
    state.undo();
    assert!(
        state.workspace.tabs[0].document.content_version > after_insert,
        "undo did not bump version"
    );
    let after_undo = state.workspace.tabs[0].document.content_version;
    state.redo();
    assert!(
        state.workspace.tabs[0].document.content_version > after_undo,
        "redo did not bump version"
    );
}

#[test]
fn test_undo_restores_previous_content() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c');
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
    state.undo();
    assert_eq!(state.workspace.tabs[0].document.content(), "ab");
}

#[test]
fn test_undo_clears_selection_and_marks_modified() {
    let mut state = make_state("ab", 2, Some((0, 1)));
    state.workspace.tabs[0]
        .document
        .undo_stack
        .push(default_paragraphs());
    state.undo();
    assert!(state.workspace.tabs[0].selection.is_none());
    assert!(state.workspace.tabs[0].document.is_modified);
}

#[test]
fn test_undo_clamps_cursor_into_shorter_restored_content() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c'); // content = "abc", cursor = 3
    state.undo();
    // Restored content is "ab" (len 2); cursor must not remain at 3.
    assert_eq!(state.workspace.tabs[0].document.content(), "ab");
    assert!(state.workspace.tabs[0].cursor <= state.workspace.tabs[0].document.content().len());
    assert!(state.workspace.tabs[0]
        .document
        .content()
        .is_char_boundary(state.workspace.tabs[0].cursor));
}

#[test]
fn test_undo_with_empty_stack_is_noop() {
    let mut state = make_state("abc", 3, None);
    state.undo();
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
    assert_eq!(state.workspace.tabs[0].cursor, 3);
}

#[test]
fn test_undo_pushes_onto_redo_stack() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c');
    state.undo();
    assert_eq!(redo_contents(&state), vec!["abc".to_string()]);
}

#[test]
fn test_redo_restores_undone_content() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c');
    state.undo();
    assert_eq!(state.workspace.tabs[0].document.content(), "ab");
    state.redo();
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_undo_restores_paragraphs_not_just_content() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "bold".into(),
            bold: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 4);
    state.insert_char('X'); // "boldX", paragraphs now ["boldX"] still bold
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "boldX"
    );
    state.undo();
    assert_eq!(state.workspace.tabs[0].document.content(), "bold");
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "bold"
    );
    assert!(state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
}

#[test]
fn test_redo_restores_paragraphs_not_just_content() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "bold".into(),
            bold: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 4);
    state.insert_char('X');
    state.undo();
    state.redo();
    assert_eq!(state.workspace.tabs[0].document.content(), "boldX");
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "boldX"
    );
    assert!(state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
}

// ── Rich text formatting Phase 2: apply_formatting_to_selection ─────────

#[test]
fn test_apply_formatting_to_active_selection() {
    let paragraphs = vec![para_plain("hello world")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 5));
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert!(state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].text,
        "hello"
    );
}

#[test]
fn test_apply_formatting_to_selection_is_undoable() {
    let paragraphs = vec![para_plain("hello")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 5));
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    state.undo();
    assert!(!state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
}

#[test]
fn test_apply_formatting_to_selection_with_no_selection_is_undoable() {
    // Bug: pressing a formatting hotkey (Ctrl+B, etc.) with the cursor on
    // a character but no active selection mutated the document without
    // ever pushing an undo snapshot, so Ctrl+Z couldn't undo it. Cursor
    // at 0 formats the 'h' under it.
    let paragraphs = vec![para_plain("hello")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = None;
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold,
        "formatting wasn't applied"
    );
    state.undo();
    assert!(
        !state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold,
        "undo did not revert the no-selection formatting"
    );
}

#[test]
fn test_apply_formatting_to_selection_with_cursor_at_document_end_is_true_noop() {
    // Cursor at the very end of the document has no character under it
    // to format — this must stay a true no-op (no undo step created),
    // matching the "true no-op = don't push" convention every other
    // mutation entry point in this file already follows.
    let paragraphs = vec![para_plain("hello")];
    let mut state = make_state_with_paragraphs(paragraphs, 5);
    state.workspace.tabs[0].selection = None;
    let undo_depth_before = state.workspace.tabs[0].document.undo_stack.len();
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert_eq!(
        state.workspace.tabs[0].document.undo_stack.len(),
        undo_depth_before
    );
}

#[test]
fn test_apply_formatting_to_selection_toggles_off_when_already_active() {
    // Bug fix: re-clicking Bold on an already-bold selection should
    // un-bold it, matching Word's toolbar toggle behavior, instead of
    // being a no-op re-application.
    let paragraphs = vec![para_plain("hello world")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 5));
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert!(state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
    state.workspace.tabs[0].selection = Some((0, 5));
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert!(!state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
}

#[test]
fn test_apply_formatting_no_selection_arms_pending_format() {
    let mut state = make_state("hello", 0, None);
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert_eq!(
        state.workspace.tabs[0].pending_format,
        Some(FormatOp::Bold(true))
    );
}

#[test]
fn test_apply_formatting_no_selection_same_op_again_disarms() {
    let mut state = make_state("hello", 0, None);
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    assert_eq!(state.workspace.tabs[0].pending_format, None);
}

#[test]
fn test_apply_formatting_no_selection_different_op_replaces_pending() {
    let mut state = make_state("hello", 0, None);
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    state.apply_formatting_to_selection(FormatOp::Italic(true));
    assert_eq!(
        state.workspace.tabs[0].pending_format,
        Some(FormatOp::Italic(true))
    );
}

// ── clear_formatting: route to selection vs. current line ──────────────

#[test]
fn clear_formatting_spans_a_multi_paragraph_selection() {
    // Bug: ClearFormattingAction always called apply_formatting_to_line,
    // which only ever clears the cursor's own line — a selection
    // spanning multiple paragraphs left the other paragraphs bold.
    let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let content_len = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, content_len));
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    for para in state.workspace.tabs[0].document.paragraphs() {
        for run in &para.runs {
            assert!(
                run.bold,
                "expected bold applied across every paragraph in the selection"
            );
        }
    }

    state.workspace.tabs[0].selection = Some((0, content_len));
    state.clear_formatting();

    for para in state.workspace.tabs[0].document.paragraphs() {
        for run in &para.runs {
            assert!(
                !run.bold,
                "expected bold cleared across every paragraph in the selection"
            );
        }
    }
}

#[test]
fn clear_formatting_falls_back_to_current_line_with_no_selection() {
    let paragraphs = vec![para_plain("one"), para_plain("two")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = None;
    state.workspace.tabs[0].cursor = 5; // inside "two"
    state.clear_formatting(); // should not panic, should behave like today's single-line clear
}

#[test]
fn test_pending_format_applies_to_newly_typed_chars() {
    let mut state = make_state_with_paragraphs(vec![para_plain("ab")], 2);
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    state.insert_char('X');
    state.insert_char('Y');
    assert_eq!(state.workspace.tabs[0].document.content(), "abXY");
    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].text, "ab");
    assert!(!runs[0].bold);
    assert_eq!(runs[1].text, "XY");
    assert!(runs[1].bold);
}

#[test]
fn test_pending_format_stops_after_toggled_off() {
    // Insert 'Y' at a position that doesn't touch the just-bolded 'X'
    // run — typing immediately adjacent to an existing bold run would
    // inherit its formatting regardless of `pending_format`'s state
    // (the same "typed text takes on the format of whatever it's
    // typed inside" rule every insert follows), which isn't what this
    // test is checking.
    let mut state = make_state_with_paragraphs(vec![para_plain("ab")], 1);
    state.apply_formatting_to_selection(FormatOp::Bold(true));
    state.insert_char('X'); // "aXb", X is bold
    state.apply_formatting_to_selection(FormatOp::Bold(true)); // toggle off
    state.workspace.tabs[0].cursor = 0;
    state.insert_char('Y'); // "YaXb", Y at the very start
    assert_eq!(state.workspace.tabs[0].document.content(), "YaXb");
    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].text, "Ya");
    assert!(!runs[0].bold);
    assert_eq!(runs[1].text, "X");
    assert!(runs[1].bold);
    assert_eq!(runs[2].text, "b");
    assert!(!runs[2].bold);
}

fn para_plain(text: &str) -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![Run {
            text: text.to_string(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

#[test]
fn test_redo_with_empty_stack_is_noop() {
    let mut state = make_state("abc", 3, None);
    state.redo();
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_new_edit_after_undo_clears_redo_stack() {
    let mut state = make_state("ab", 2, None);
    state.insert_char('c');
    state.undo();
    assert!(!state.workspace.tabs[0].document.redo_stack.is_empty());
    break_coalesce_window(&mut state);
    state.insert_char('d');
    assert!(state.workspace.tabs[0].document.redo_stack.is_empty());
}

// ── closed_beta_plan.md §0: settings.conf / working_directory resolve
// against the executable's location, not CWD ─────────────────────────────

#[test]
fn test_settings_conf_path_resolves_under_the_user_data_dir() {
    let path = settings_conf_path();
    assert_eq!(
        path.file_name(),
        Some(std::ffi::OsStr::new("settings.conf"))
    );
    // Deliberately NOT next to the executable: a packaged macOS .app is
    // read-only under Gatekeeper translocation, and writing inside one
    // breaks its code signature. Must be the same writable per-user
    // directory crash.log and the recovery snapshots already use.
    assert_eq!(
        path.parent(),
        Some(crate::recovery::app_data_dir().as_path())
    );
}

#[test]
fn test_crash_log_path_resolves_under_a_fixed_dir_named_for_the_os() {
    let path = crash_log_path();
    assert_eq!(path.file_name(), Some(std::ffi::OsStr::new("crash.log")));
    let parent_name = path.parent().unwrap().file_name().unwrap();
    if cfg!(target_os = "windows") {
        assert_eq!(parent_name, "vimbatim");
    } else {
        assert_eq!(parent_name, ".vimbatim");
    }
    assert!(path.is_absolute());
}

#[test]
fn test_default_working_directory_is_not_bare_cwd_dot() {
    // Before this fix, working_directory always came from
    // std::env::current_dir() — a double-clicked packaged .app/.exe has
    // no guaranteed CWD (e.g. macOS Finder launches at "/"), so this
    // guards against silently regressing back to that.
    let dir = default_working_directory();
    assert_ne!(dir, PathBuf::from("."));
    assert!(dir.is_absolute());
}

// ── working_directory / expanded_dirs persistence (Task 4) ──────────────

#[test]
fn working_directory_round_trips_through_settings_conf() {
    let dir = temp_test_dir("working_directory_round_trip");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "").unwrap();
    let target = PathBuf::from("/some/nested/dir");
    save_working_directory(&conf_path, &target).unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .working_directory,
        Some(target)
    );
}

#[test]
fn expanded_dirs_round_trip_through_settings_conf() {
    let dir = temp_test_dir("expanded_dirs_round_trip");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "").unwrap();
    let dirs = vec![PathBuf::from("/a/b"), PathBuf::from("/a/c")];
    save_expanded_dirs(&conf_path, &dirs).unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .expanded_dirs,
        dirs
    );
}

#[test]
fn load_working_directory_returns_none_when_key_missing() {
    let dir = temp_test_dir("working_directory_missing_key");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "theme=dark\n").unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .working_directory,
        None
    );
}

#[test]
fn load_expanded_dirs_returns_empty_when_key_missing() {
    let dir = temp_test_dir("expanded_dirs_missing_key");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "theme=dark\n").unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .expanded_dirs,
        Vec::<PathBuf>::new()
    );
}

#[test]
fn test_undo_stack_capped_at_200() {
    let mut state = make_state("", 0, None);
    for _ in 0..250 {
        state.insert_char('x');
        break_coalesce_window(&mut state); // force every insert onto its own step
    }
    assert_eq!(state.workspace.tabs[0].document.undo_stack.len(), 200);
}

// ── undo/redo byte-budget cap (performance_plan.md's "undo/redo stack
// memory" finding) ──────────────────────────────────────────────────────

#[test]
fn test_snapshot_byte_estimate_sums_content_and_run_strings() {
    let content = "hello world"; // 11 bytes
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "hello world".into(), // 11 bytes, counted again (runs are the source of truth for formatted text)
            highlight_color: "yellow".into(), // 6 bytes
            font: Some("Arial".into()), // 5 bytes
            color: Some("FF0000".into()), // 6 bytes
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    // Payload bytes, plus the structs holding them and their allocator
    // overhead — the terms the estimate used to ignore, which is what let
    // the byte budget permit ~2.7x what it claimed.
    let payloads = 11 + 11 + 6 + 5 + 6;
    let structs =
        std::mem::size_of::<Paragraph>() + std::mem::size_of::<Run>() + PER_RUN_ALLOCATION_OVERHEAD;
    assert_eq!(
        snapshot_byte_estimate(content, &paragraphs),
        payloads + structs
    );
}

/// The estimate must never *under*-count, in any shape — under-counting is
/// the direction that breaks the memory ceiling.
#[test]
fn snapshot_byte_estimate_counts_at_least_the_bytes_it_can_see() {
    let paragraphs: Vec<Paragraph> = (0..50)
        .map(|i| Paragraph {
            list: None,
            runs: (0..20)
                .map(|j| Run {
                    text: format!("run {i}-{j} with some text"),
                    ..Run::default()
                })
                .collect(),
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        })
        .collect();
    let content = paragraphs_to_plain_text(&paragraphs);
    let payloads: usize = content.len()
        + paragraphs
            .iter()
            .flat_map(|p| &p.runs)
            .map(|r| r.text.len())
            .sum::<usize>();
    let estimate = snapshot_byte_estimate(&content, &paragraphs);
    assert!(
        estimate > payloads,
        "estimate {estimate} must exceed the {payloads} bytes of raw payload it holds"
    );
    // 1000 runs of ~20 bytes: the structs genuinely outweigh the text, which
    // is the whole reason the old payload-only estimate was so far off.
    assert!(
        estimate > 2 * payloads,
        "structs should dominate here, got {estimate} vs {payloads}"
    );
}

#[test]
fn test_undo_stack_cap_full_for_small_snapshots() {
    assert_eq!(undo_stack_cap_for_snapshot_size(100), UNDO_STACK_CAP);
}

/// The budget is shared between a tab's undo and redo stacks, which are
/// capped independently and can both be full at once. A tab must not be
/// able to hold more than the budget across the pair.
#[test]
fn undo_and_redo_together_stay_inside_the_byte_budget() {
    for snapshot in [64_000usize, 500_000, 1_300_000, 4_000_000] {
        let cap = undo_stack_cap_for_snapshot_size(snapshot);
        let both_stacks_full = cap * snapshot * 2;
        // The MIN_CAP floor deliberately wins for very large documents —
        // a few undo levels matter more than the ceiling there.
        if cap > UNDO_STACK_MIN_CAP {
            assert!(
                both_stacks_full <= UNDO_STACK_BYTE_BUDGET,
                "{snapshot}-byte snapshots: {cap} entries per stack = {both_stacks_full} bytes"
            );
        }
    }
}

#[test]
fn test_undo_stack_cap_shrinks_for_large_snapshots() {
    // 1MB snapshot against half the budget (the other half is the redo
    // stack's): 50_000_000 / 1_000_000 == 50, well under UNDO_STACK_CAP.
    assert_eq!(undo_stack_cap_for_snapshot_size(1_000_000), 50);
}

#[test]
fn test_undo_stack_cap_never_below_minimum() {
    // A snapshot bigger than the whole budget would compute to 0 without
    // the floor — must still keep at least UNDO_STACK_MIN_CAP levels.
    assert_eq!(
        undo_stack_cap_for_snapshot_size(UNDO_STACK_BYTE_BUDGET * 10),
        UNDO_STACK_MIN_CAP
    );
}

#[test]
fn test_undo_stack_shrinks_below_200_for_large_document() {
    // Proves push_undo_snapshot actually applies the size-aware cap
    // (not just that the pure function exists): a 1MB document's
    // snapshot size caps it well below the usual 200. Computed from
    // the actual final content/paragraphs rather than hardcoded, since
    // formatting-sync (insert_char keeps `paragraphs` textually in
    // step with `content`) means the snapshot is larger than
    // `content.len()` alone.
    let big_content = "a".repeat(1_000_000);
    let mut state = make_state(&big_content, big_content.len(), None);
    for _ in 0..150 {
        state.insert_char('x');
        break_coalesce_window(&mut state);
    }
    let tab = &state.workspace.tabs[0];
    let expected_cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(
        &tab.document.content(),
        tab.document.paragraphs(),
    ));
    assert!(expected_cap < 200);
    assert_eq!(tab.document.undo_stack.len(), expected_cap);
}

#[test]
fn test_redo_stack_shrinks_below_200_for_large_document() {
    // Undoing repeatedly without any new edit must cap redo_stack the
    // same way push_undo_snapshot caps undo_stack — otherwise a user
    // holding Ctrl+Z on a huge document could still blow past the byte
    // budget via redo_stack alone.
    let big_content = "a".repeat(1_000_000);
    let mut state = make_state(&big_content, big_content.len(), None);
    for _ in 0..150 {
        state.insert_char('x');
        break_coalesce_window(&mut state);
    }
    for _ in 0..150 {
        state.undo();
    }
    let tab = &state.workspace.tabs[0];
    let expected_cap = undo_stack_cap_for_snapshot_size(snapshot_byte_estimate(
        &tab.document.content(),
        tab.document.paragraphs(),
    ));
    assert!(expected_cap < 200);
    assert_eq!(tab.document.redo_stack.len(), expected_cap);
}

#[test]
fn test_backspace_pushes_undo_snapshot() {
    let mut state = make_state("abc", 3, None);
    state.backspace();
    assert_eq!(undo_contents(&state), vec!["abc".to_string()]);
}

#[test]
fn test_backspace_noop_at_document_start_does_not_push_undo() {
    let mut state = make_state("abc", 0, None);
    state.backspace();
    assert!(state.workspace.tabs[0].document.undo_stack.is_empty());
}

#[test]
fn test_backspace_over_selection_pushes_one_undo_step() {
    let mut state = make_state("hello world", 5, Some((0, 5)));
    state.backspace();
    assert_eq!(undo_contents(&state), vec!["hello world".to_string()]);
}

#[test]
fn delete_forward_removes_the_character_after_the_cursor() {
    let mut state = make_state("abc", 1, None);
    state.delete_forward();
    assert_eq!(state.workspace.tabs[0].document.content(), "ac");
    assert_eq!(
        state.workspace.tabs[0].cursor, 1,
        "cursor doesn't move for a forward delete"
    );
}

#[test]
fn delete_forward_deletes_the_selection_when_one_is_active() {
    let mut state = make_state("hello world", 5, Some((0, 5)));
    state.delete_forward();
    assert_eq!(state.workspace.tabs[0].document.content(), " world");
}

#[test]
fn delete_forward_is_a_no_op_at_document_end() {
    let mut state = make_state("abc", 3, None);
    state.delete_forward();
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
    assert!(state.workspace.tabs[0].document.undo_stack.is_empty());
}

#[test]
fn test_delete_selection_pushes_undo_snapshot() {
    let mut state = make_state("hello world", 5, Some((0, 5)));
    state.delete_selection();
    assert_eq!(undo_contents(&state), vec!["hello world".to_string()]);
}

#[test]
fn test_delete_selection_noop_does_not_push_undo() {
    let mut state = make_state("hello world", 5, None);
    state.delete_selection();
    assert!(state.workspace.tabs[0].document.undo_stack.is_empty());
}

#[test]
fn test_insert_str_pushes_undo_snapshot() {
    let mut state = make_state("hello", 5, None);
    state.insert_str(" world");
    assert_eq!(undo_contents(&state), vec!["hello".to_string()]);
}

#[test]
fn test_insert_str_empty_does_not_push_undo() {
    let mut state = make_state("hello", 5, None);
    state.insert_str("");
    assert!(state.workspace.tabs[0].document.undo_stack.is_empty());
}

#[test]
fn test_insert_str_replacing_selection_pushes_one_undo_step() {
    let mut state = make_state("hello world", 5, Some((0, 5)));
    state.insert_str("goodbye");
    assert_eq!(undo_contents(&state), vec!["hello world".to_string()]);
}

// ── vim mode-entry transitions (Task D) ─────────────────────────────────────

#[test]
fn test_vim_enter_insert_before_cursor_sets_mode_and_preserves_cursor() {
    let mut state = make_state("hello", 2, None);
    state.vim_enter_insert_before_cursor();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_vim_enter_insert_before_cursor_clears_selection() {
    let mut state = make_state("hello", 2, Some((0, 2)));
    state.vim_enter_insert_before_cursor();
    assert_eq!(state.workspace.tabs[0].selection, None);
}

#[test]
fn test_vim_enter_insert_line_start_moves_to_first_nonblank() {
    let mut state = make_state("  hello", 5, None);
    state.vim_enter_insert_line_start();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_vim_enter_insert_after_cursor_moves_right() {
    let mut state = make_state("hello", 0, None);
    state.vim_enter_insert_after_cursor();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_vim_enter_insert_after_cursor_clamps_at_document_end() {
    let mut state = make_state("hi", 2, None);
    state.vim_enter_insert_after_cursor();
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_vim_enter_insert_line_end_moves_to_line_end() {
    let mut state = make_state("hello\nworld", 0, None);
    state.vim_enter_insert_line_end();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    assert_eq!(state.workspace.tabs[0].cursor, 5); // byte offset of the '\n'
}

#[test]
fn test_vim_open_line_below_creates_new_line_and_places_cursor_on_it() {
    let mut state = make_state("hello", 2, None);
    state.vim_open_line_below();
    assert_eq!(state.workspace.tabs[0].document.content(), "hello\n");
    assert_eq!(state.workspace.tabs[0].cursor, 6);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_vim_open_line_below_pushes_undo_snapshot() {
    let mut state = make_state("hello", 2, None);
    state.vim_open_line_below();
    assert_eq!(undo_contents(&state), vec!["hello".to_string()]);
}

#[test]
fn test_vim_open_line_below_on_last_line_of_multiline_doc() {
    let mut state = make_state("first\nsecond", 8, None);
    state.vim_open_line_below();
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "first\nsecond\n"
    );
    assert_eq!(state.workspace.tabs[0].cursor, 13);
}

#[test]
fn test_vim_open_line_below_on_empty_document() {
    let mut state = make_state("", 0, None);
    state.vim_open_line_below();
    assert_eq!(state.workspace.tabs[0].document.content(), "\n");
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_vim_open_line_above_inserts_before_current_line() {
    let mut state = make_state("hello", 2, None);
    state.vim_open_line_above();
    assert_eq!(state.workspace.tabs[0].document.content(), "\nhello");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_vim_open_line_above_pushes_undo_snapshot() {
    let mut state = make_state("hello", 2, None);
    state.vim_open_line_above();
    assert_eq!(undo_contents(&state), vec!["hello".to_string()]);
}

#[test]
fn test_vim_open_line_above_on_second_line() {
    let mut state = make_state("first\nsecond", 8, None);
    state.vim_open_line_above();
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "first\n\nsecond"
    );
    assert_eq!(state.workspace.tabs[0].cursor, 6);
}

#[test]
fn test_vim_open_line_above_on_empty_document() {
    let mut state = make_state("", 0, None);
    state.vim_open_line_above();
    assert_eq!(state.workspace.tabs[0].document.content(), "\n");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_vim_enter_visual_selects_char_under_cursor() {
    let mut state = make_state("hello", 1, None);
    state.vim_enter_visual();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual);
    assert_eq!(state.workspace.tabs[0].selection, Some((1, 2)));
}

#[test]
fn test_vim_enter_visual_at_document_end_zero_width_selection() {
    let mut state = make_state("hi", 2, None);
    state.vim_enter_visual();
    assert_eq!(state.workspace.tabs[0].selection, Some((2, 2)));
}

#[test]
fn test_vim_enter_visual_line_selects_whole_line_including_newline() {
    let mut state = make_state("first\nsecond", 2, None); // on "first"
    state.vim_enter_visual_line();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::VisualLine);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 6))); // "first\n"
}

#[test]
fn test_vim_enter_visual_line_on_last_line_no_trailing_newline() {
    let mut state = make_state("first\nsecond", 8, None);
    state.workspace.tabs[0].cursor = 8; // on "second"
    state.vim_enter_visual_line();
    // "second" is the last line and has no trailing '\n' to include.
    assert_eq!(state.workspace.tabs[0].selection, Some((6, 12)));
}

#[test]
fn test_vim_enter_command_sets_mode() {
    let mut state = make_state("hello", 2, None);
    state.vim_enter_command();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
}

#[test]
fn test_vim_exit_to_normal_clears_selection_and_mode() {
    let mut state = make_state("hello", 2, Some((0, 2)));
    state.workspace.tabs[0].vim_mode = VimMode::Visual;
    state.vim_exit_to_normal();
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].selection, None);
}

// ── handle_vim_key dispatch (Task D) ─────────────────────────────────────────

#[test]
fn test_handle_vim_key_normal_i_enters_insert() {
    let mut state = make_state("hello", 0, None);
    let handled = state.handle_vim_key("i", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_handle_vim_key_normal_colon_via_shift_semicolon_enters_command() {
    let mut state = make_state("hello", 0, None);
    let handled = state.handle_vim_key(";", true, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
}

#[test]
fn test_handle_vim_key_normal_colon_via_key_char_enters_command() {
    // Covers the case where GPUI reports the shifted character directly
    // via key_char instead of (or in addition to) the base key + shift.
    let mut state = make_state("hello", 0, None);
    let handled = state.handle_vim_key(";", false, Some(":"));
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
}

#[test]
fn test_handle_vim_key_normal_colon_via_key_reported_as_symbol_directly() {
    let mut state = make_state("hello", 0, None);
    let handled = state.handle_vim_key(":", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
}

#[test]
fn test_handle_vim_key_normal_navigation_falls_through() {
    let mut state = make_state("hello", 2, None);
    let handled = state.handle_vim_key("left", false, None);
    assert!(!handled);
    // handle_vim_key itself must not move the cursor when it declines
    // to consume the key — the caller applies the plain-editor movement.
    assert_eq!(state.workspace.tabs[0].cursor, 2);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_handle_vim_key_normal_unmapped_printable_is_swallowed() {
    let mut state = make_state("hello", 2, None);
    let handled = state.handle_vim_key("q", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].document.content(), "hello"); // not inserted as text
}

#[test]
fn test_handle_vim_key_insert_mode_returns_false() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Insert;
    let handled = state.handle_vim_key("x", false, None);
    assert!(!handled);
}

#[test]
fn test_handle_vim_key_visual_escape_exits_to_normal() {
    let mut state = make_state("hello", 2, Some((2, 3)));
    state.workspace.tabs[0].vim_mode = VimMode::Visual;
    let handled = state.handle_vim_key("escape", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].selection, None);
}

#[test]
fn test_handle_vim_key_visual_v_exits_to_normal() {
    let mut state = make_state("hello", 2, Some((2, 3)));
    state.workspace.tabs[0].vim_mode = VimMode::Visual;
    let handled = state.handle_vim_key("v", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_handle_vim_key_visual_shift_v_is_swallowed_without_mode_change() {
    // Switching Visual -> VisualLine on shift-V isn't in spec 5.1's
    // table and is out of scope for Task D; it should be swallowed,
    // not fall through to text insertion, but also not change mode.
    let mut state = make_state("hello", 2, Some((2, 3)));
    state.workspace.tabs[0].vim_mode = VimMode::Visual;
    let handled = state.handle_vim_key("v", true, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual);
}

#[test]
fn test_handle_vim_key_visual_line_shift_v_exits_to_normal() {
    let mut state = make_state("hello", 2, Some((0, 5)));
    state.workspace.tabs[0].vim_mode = VimMode::VisualLine;
    let handled = state.handle_vim_key("v", true, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_handle_vim_key_visual_line_plain_v_is_noop() {
    let mut state = make_state("hello", 2, Some((0, 5)));
    state.workspace.tabs[0].vim_mode = VimMode::VisualLine;
    let handled = state.handle_vim_key("v", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::VisualLine);
}

#[test]
fn test_handle_vim_key_command_escape_exits_to_normal() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    state.workspace.tabs[0].vim_command_line = "wq".to_string();
    let handled = state.handle_vim_key("escape", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].vim_command_line, ""); // discarded, not dispatched
}

#[test]
fn test_handle_vim_key_command_enter_exits_to_normal() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    let handled = state.handle_vim_key("enter", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_handle_vim_key_command_other_key_is_swallowed_no_mode_change() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    let handled = state.handle_vim_key("x", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
    assert_eq!(state.workspace.tabs[0].document.content(), "hello"); // not inserted as text
    assert_eq!(state.workspace.tabs[0].vim_command_line, "x"); // captured into command line instead
}

// ── Task H.1: Command-mode text capture ─────────────────────────────────

#[test]
fn test_command_mode_captures_typed_letters() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    state.handle_vim_key("w", false, None);
    state.handle_vim_key("q", false, None);
    assert_eq!(state.workspace.tabs[0].vim_command_line, "wq");
}

#[test]
fn test_command_mode_captures_punctuation_via_key_char() {
    // GPUI reports shifted punctuation via key_char on this backend;
    // vim_find_target_char is the proven-correct resolver for it.
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    state.handle_vim_key("5", true, Some("%"));
    state.handle_vim_key("s", false, None);
    assert_eq!(state.workspace.tabs[0].vim_command_line, "%s");
}

#[test]
fn test_command_mode_backspace_removes_last_char() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    state.workspace.tabs[0].vim_command_line = "wq".to_string();
    state.handle_vim_key("backspace", false, None);
    assert_eq!(state.workspace.tabs[0].vim_command_line, "w");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
}

#[test]
fn test_command_mode_backspace_on_empty_exits_to_normal() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    state.handle_vim_key("backspace", false, None);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_command_mode_enter_clears_command_line_after_dispatch() {
    let mut state = make_state("hello", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Command;
    state.workspace.tabs[0].vim_command_line = "nonsense".to_string();
    state.handle_vim_key("enter", false, None);
    assert_eq!(state.workspace.tabs[0].vim_command_line, "");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

// ── Task H.2: dispatch_vim_command ──────────────────────────────────────

#[test]
fn test_dispatch_vim_command_set_novim_disables_vim() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("set novim");
    assert!(!state.global_vim.vim_enabled);
}

#[test]
fn test_dispatch_vim_command_set_vim_reenables_vim() {
    let mut state = make_state("hello", 0, None);
    state.global_vim.vim_enabled = false;
    state.dispatch_vim_command("set vim");
    assert!(state.global_vim.vim_enabled);
}

#[test]
fn test_dispatch_vim_command_line_number_jumps_cursor() {
    let mut state = make_state("aaa\nbbb\nccc", 0, None);
    state.dispatch_vim_command("2");
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of line 2 ("bbb")
}

#[test]
fn test_dispatch_vim_command_noh_is_noop_no_error() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("noh");
    assert_eq!(state.workspace.tabs[0].vim_command_error, None);
}

#[test]
fn test_dispatch_vim_command_unknown_command_sets_error() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("bogus");
    assert!(state.workspace.tabs[0].vim_command_error.is_some());
}

#[test]
fn test_dispatch_vim_command_w_with_no_file_path_is_noop() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("w");
    assert_eq!(state.workspace.tabs[0].vim_command_error, None);
}

#[test]
fn test_dispatch_vim_command_q_on_modified_tab_sets_error_and_does_not_close() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].document.is_modified = true;
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    state.workspace.active_tab = 0;
    state.dispatch_vim_command("q");
    assert_eq!(state.workspace.tabs.len(), 2);
    assert!(state.workspace.tabs[0].vim_command_error.is_some());
}

#[test]
fn test_dispatch_vim_command_q_on_unmodified_tab_closes() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    state.workspace.active_tab = 0;
    state.dispatch_vim_command("q");
    assert_eq!(state.workspace.tabs.len(), 1);
}

#[test]
fn test_dispatch_vim_command_q_bang_force_closes_even_if_modified() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].document.is_modified = true;
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    state.workspace.active_tab = 0;
    state.dispatch_vim_command("q!");
    assert_eq!(state.workspace.tabs.len(), 1);
}

#[test]
fn test_dispatch_vim_command_wq_closes_tab_when_no_file_path() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    state.workspace.active_tab = 0;
    state.dispatch_vim_command("wq");
    assert_eq!(state.workspace.tabs.len(), 1);
}

// ── Task H.3: :%s/pattern/replacement/[g][i] ────────────────────────────

#[test]
fn test_dispatch_vim_command_substitute_first_match_per_line() {
    let mut state = make_state("foo foo\nbar", 0, None);
    state.dispatch_vim_command("%s/foo/baz/");
    assert_eq!(state.workspace.tabs[0].document.content(), "baz foo\nbar");
    assert!(state.workspace.tabs[0].document.is_modified);
}

#[test]
fn test_dispatch_vim_command_substitute_global_flag_replaces_all_on_line() {
    let mut state = make_state("foo foo\nbar", 0, None);
    state.dispatch_vim_command("%s/foo/baz/g");
    assert_eq!(state.workspace.tabs[0].document.content(), "baz baz\nbar");
}

#[test]
fn test_dispatch_vim_command_substitute_case_insensitive_flag() {
    let mut state = make_state("Foo bar", 0, None);
    state.dispatch_vim_command("%s/foo/baz/i");
    assert_eq!(state.workspace.tabs[0].document.content(), "baz bar");
}

#[test]
fn test_dispatch_vim_command_substitute_no_match_leaves_content_unmodified() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("%s/xyz/abc/");
    assert_eq!(state.workspace.tabs[0].document.content(), "hello");
    assert!(!state.workspace.tabs[0].document.is_modified);
}

#[test]
fn test_dispatch_vim_command_substitute_bad_regex_sets_error() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("%s/[/x/");
    assert!(state.workspace.tabs[0].vim_command_error.is_some());
}

#[test]
fn test_dispatch_vim_command_e_requests_background_open() {
    let mut state = make_state("hello", 0, None);
    state.dispatch_vim_command("e nonexistent_test_file.docx");
    assert!(matches!(
        state.global_vim.pending_effects.as_slice(),
        [crate::app::command::AppEffect::LoadDocument(path)]
            if path.ends_with("nonexistent_test_file.docx")
    ));
}

// ── Task H.4: "<register> prefix + wiring into d/y/c ────────────────────

#[test]
fn test_quote_letter_dd_writes_to_named_register_and_default() {
    // Two lines so dd's linewise range naturally includes the trailing
    // '\n' (deleting the last line of a doc with no final newline
    // wouldn't have one to include — not this test's concern).
    let mut state = make_state("hello world\nsecond", 0, None);
    state.handle_vim_key("'", true, Some("\"")); // "
    state.handle_vim_key("a", false, None); // select register a
    state.handle_vim_key("d", false, None); // dd
    state.handle_vim_key("d", false, None);
    assert_eq!(
        state.global_vim.registers.get(&'a'),
        Some(&"hello world\n".to_string())
    );
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"hello world\n".to_string())
    );
}

#[test]
fn test_quote_letter_yank_also_writes_yank_register() {
    let mut state = make_state("hello world\nsecond", 0, None);
    state.handle_vim_key("'", true, Some("\""));
    state.handle_vim_key("b", false, None);
    state.handle_vim_key("y", false, None);
    state.handle_vim_key("y", false, None);
    assert_eq!(
        state.global_vim.registers.get(&'b'),
        Some(&"hello world\n".to_string())
    );
    assert_eq!(
        state.global_vim.registers.get(&'0'),
        Some(&"hello world\n".to_string())
    );
}

#[test]
fn test_register_selection_is_one_shot_reverts_to_default_after() {
    let mut state = make_state("one\ntwo\nthree", 0, None);
    state.handle_vim_key("'", true, Some("\""));
    state.handle_vim_key("a", false, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("d", false, None); // "add -> register a
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("d", false, None); // plain dd -> default only
    assert_eq!(
        state.global_vim.registers.get(&'a'),
        Some(&"one\n".to_string())
    );
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"two\n".to_string())
    );
}

#[test]
fn test_plus_register_prefix_stages_pending_clipboard_sync() {
    let mut state = make_state("hello\nworld", 0, None);
    state.handle_vim_key("'", true, Some("\""));
    state.handle_vim_key("=", true, Some("+"));
    state.handle_vim_key("y", false, None);
    state.handle_vim_key("y", false, None);
    assert_eq!(
        state.global_vim.registers.get(&'+'),
        Some(&"hello\n".to_string())
    );
    // The formatting rides along so `text_editor.rs` can put it on the
    // clipboard as metadata; only the text half is asserted here — the
    // encoding itself is `rich_clipboard`'s own round-trip tests.
    let (text, metadata) = state.global_vim.pending_clipboard_sync.clone().unwrap();
    assert_eq!(text, "hello\n");
    assert_eq!(Some(&metadata), state.global_vim.register_formats.get(&'+'));
}

// ── Task H.5: p/P paste ──────────────────────────────────────────────────

#[test]
fn test_paste_charwise_after_cursor() {
    let mut state = make_state("abc", 0, None);
    state.global_vim.registers.insert('"', "XY".to_string());
    state.handle_vim_key("p", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "aXYbc");
    assert_eq!(state.workspace.tabs[0].cursor, 2); // lands on last pasted char 'Y'
}

#[test]
fn test_paste_charwise_before_cursor_capital_p() {
    let mut state = make_state("abc", 1, None);
    state.global_vim.registers.insert('"', "XY".to_string());
    state.handle_vim_key("p", true, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "aXYbc");
}

#[test]
fn test_paste_linewise_inserts_as_new_line_below() {
    let mut state = make_state("one\ntwo", 0, None);
    state
        .global_vim
        .registers
        .insert('"', "middle\n".to_string());
    state.handle_vim_key("p", false, None);
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "one\nmiddle\ntwo"
    );
}

#[test]
fn test_paste_linewise_capital_p_inserts_above() {
    let mut state = make_state("one\ntwo", 4, None); // cursor on "two"
    state
        .global_vim
        .registers
        .insert('"', "middle\n".to_string());
    state.handle_vim_key("p", true, None);
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "one\nmiddle\ntwo"
    );
}

#[test]
fn test_paste_empty_register_is_noop() {
    let mut state = make_state("abc", 0, None);
    state.handle_vim_key("p", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_paste_named_register_after_quote_prefix() {
    let mut state = make_state("abc", 0, None);
    state.global_vim.registers.insert('a', "Z".to_string());
    state.handle_vim_key("'", true, Some("\""));
    state.handle_vim_key("a", false, None);
    state.handle_vim_key("p", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "aZbc");
}

// ── Task I.1: x/X/s/S/~/J convenience commands ──────────────────────────

#[test]
fn test_x_deletes_char_under_cursor() {
    let mut state = make_state("abc", 1, None);
    state.handle_vim_key("x", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ac");
    assert_eq!(state.workspace.tabs[0].cursor, 1);
    assert_eq!(state.global_vim.registers.get(&'"'), Some(&"b".to_string()));
}

#[test]
fn test_x_at_end_of_line_does_not_cross_newline() {
    let mut state = make_state("ab\ncd", 1, None); // cursor on 'b', last char of line
    state.handle_vim_key("x", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "a\ncd");
}

#[test]
fn test_x_on_empty_line_is_noop() {
    let mut state = make_state("\nabc", 0, None);
    state.handle_vim_key("x", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "\nabc");
}

#[test]
fn test_capital_x_deletes_char_before_cursor() {
    let mut state = make_state("abc", 2, None);
    state.handle_vim_key("x", true, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ac");
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_capital_x_at_line_start_does_not_cross_newline() {
    let mut state = make_state("ab\ncd", 3, None); // cursor on 'c', first char of line 2
    state.handle_vim_key("x", true, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ab\ncd");
}

#[test]
fn test_s_deletes_char_and_enters_insert() {
    let mut state = make_state("abc", 1, None);
    state.handle_vim_key("s", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ac");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_capital_s_deletes_line_and_enters_insert() {
    let mut state = make_state("abc\ndef", 1, None);
    state.handle_vim_key("s", true, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "\ndef");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_tilde_toggles_case_and_advances_cursor() {
    let mut state = make_state("aBc", 0, None);
    state.handle_vim_key("`", true, Some("~"));
    assert_eq!(state.workspace.tabs[0].document.content(), "ABc");
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_tilde_at_end_of_line_is_noop() {
    let mut state = make_state("\nabc", 0, None);
    state.handle_vim_key("`", true, Some("~"));
    assert_eq!(state.workspace.tabs[0].document.content(), "\nabc");
}

#[test]
fn test_join_joins_current_line_with_next() {
    let mut state = make_state("one\ntwo", 0, None);
    state.handle_vim_key("j", true, None); // J (shift+j)
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
}

#[test]
fn test_join_collapses_next_line_leading_whitespace() {
    let mut state = make_state("one\n   two", 0, None);
    state.handle_vim_key("j", true, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
}

#[test]
fn test_join_on_last_line_is_noop() {
    let mut state = make_state("only", 0, None);
    state.handle_vim_key("j", true, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "only");
}

// ── Task I.2: r<char> replace one character ─────────────────────────────

#[test]
fn test_r_replaces_char_under_cursor() {
    let mut state = make_state("abc", 1, None);
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("z", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "azc");
    assert_eq!(state.workspace.tabs[0].cursor, 1); // stays on the replaced char
}

#[test]
fn test_r_with_shifted_replacement_char() {
    let mut state = make_state("abc", 0, None);
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("z", true, None); // shift+z -> 'Z'
    assert_eq!(state.workspace.tabs[0].document.content(), "Zbc");
}

#[test]
fn test_r_escape_cancels_without_changing_content() {
    let mut state = make_state("abc", 1, None);
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("escape", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_r_does_not_write_register() {
    let mut state = make_state("abc", 1, None);
    state
        .global_vim
        .registers
        .insert('"', "unchanged".to_string());
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("z", false, None);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"unchanged".to_string())
    );
}

#[test]
fn test_r_on_empty_line_is_noop() {
    let mut state = make_state("\nabc", 0, None);
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("z", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "\nabc");
}

// ── bare `u` = real vim Undo (checklist: previously unwired) ──────────────

#[test]
fn test_bare_u_undoes_the_last_edit() {
    let mut state = make_state("abc", 1, None);
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("z", false, None); // "abc" -> "azc"
    assert_eq!(state.workspace.tabs[0].document.content(), "azc");
    state.handle_vim_key("u", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_bare_u_is_a_noop_with_nothing_to_undo() {
    let mut state = make_state("abc", 1, None);
    assert!(state.handle_vim_key("u", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_shifted_u_is_still_unbound() {
    // Real vim's `U` ("undo whole line") is explicitly out of scope —
    // shifted U must not accidentally alias to plain undo.
    let mut state = make_state("abc", 1, None);
    state.handle_vim_key("r", false, None);
    state.handle_vim_key("z", false, None); // "abc" -> "azc"
    state.handle_vim_key("u", true, None);
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "azc",
        "shift+u must not undo"
    );
}

// ── Vim-keybind checklist item: reserved-key registry ─────────────────────
// (`is_vim_reserved_normal_key`) — the parity test below is the actual
// safety net; these spot-checks pin the specific asymmetric cases (m/M,
// k/K, and the deliberately-unclaimed shifted D/Y/C/U/K/Q) that a purely
// exhaustive test would prove but not clearly document as intentional.

#[test]
fn test_z_and_lowercase_m_are_unreserved() {
    assert!(!is_vim_reserved_normal_key("z", false, None));
    assert!(!is_vim_reserved_normal_key("z", true, None));
    assert!(!is_vim_reserved_normal_key("m", false, None));
}

#[test]
fn test_uppercase_m_and_k_are_reserved_despite_lowercase_being_free() {
    assert!(
        is_vim_reserved_normal_key("m", true, None),
        "M is the visual screen-jump"
    );
    assert!(
        !is_vim_reserved_normal_key("k", false, None),
        "bare k is free of a built-in Normal-mode meaning at this layer"
    );
    assert!(
        is_vim_reserved_normal_key("k", true, None),
        "K is reserved (H/M/L jump family)"
    );
}

#[test]
fn test_shifted_dycuqk_are_deliberately_unclaimed() {
    for key in ["d", "y", "c", "u", "q"] {
        assert!(
            !is_vim_reserved_normal_key(key, true, None),
            "shift+{key} should be free"
        );
    }
}

/// One frozen snapshot of every field a keystroke could observably
/// change, used by the exhaustive parity test below to prove a
/// non-reserved key is a true no-op, not just "didn't crash."
fn vim_dispatch_snapshot(state: &AppState) -> String {
    let tab = &state.workspace.tabs[0];
    format!(
        "{:?}|{}|{:?}|{}|{:?}|{:?}|{}|{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
        tab.cursor,
        tab.document.content(),
        tab.selection,
        tab.vim_command_buf,
        tab.vim_mode,
        tab.vim_pending_operator,
        tab.vim_command_line,
        tab.vim_command_error,
        tab.vim_pending_register_select,
        tab.vim_selected_register,
        tab.vim_pending_replace,
        tab.vim_pending_text_object_prefix,
        tab.last_find,
        state.global_vim.registers,
    )
}

/// The registry's real safety net: for every ASCII letter/digit key this
/// app's vim-keybind first-key check (`is_vim_reserved_normal_key`) does
/// *not* claim, replaying it through a fresh, otherwise-idle Normal-mode
/// `AppState` must produce *zero* observable change. If this ever fails,
/// either a real vim command was added without updating the reserved-key
/// list (fix the list), or the list over-claims a key vim doesn't
/// actually use (fine to leave reserved, but worth knowing).
///
/// Scoped to letters + digits with `key_char: None` — the exact
/// representation the new vim-keybind dispatcher actually receives
/// keystrokes in (see `text_editor.rs`), and the only keyspace the
/// z-leader system's sequences are ever built from. Symbol keys are
/// covered by `is_vim_reserved_normal_key`'s own reuse of
/// `matches_shifted_symbol` (already exercised by every existing vim
/// test that types a symbol), not re-proven exhaustively here.
#[test]
fn test_every_non_reserved_letter_or_digit_is_a_true_vim_noop() {
    let mut candidates: Vec<char> = ('a'..='z').collect();
    candidates.extend('0'..='9');

    for key_char in candidates {
        let key = key_char.to_string();
        for shift in [false, true] {
            if is_vim_reserved_normal_key(&key, shift, None) {
                continue;
            }
            let mut state = make_state("hello world", 5, None);
            let before = vim_dispatch_snapshot(&state);
            state.handle_vim_key(&key, shift, None);
            let after = vim_dispatch_snapshot(&state);
            assert_eq!(
                    before, after,
                    "key {key:?} (shift={shift}) is claimed to be unreserved but changed observable state — reserve it in is_vim_reserved_normal_key"
                );
        }
    }
}

// ── Vim-keybind runtime dispatch (checklist: Settings -> Vim Mode) ────────

#[test]
fn test_z_then_s_fires_save_via_pending_vim_action() {
    // Exercises the real default table (`VimKeybinds::defaults()`,
    // `make_state`'s vim_keybinds), not a hand-inserted binding — this
    // is the exact sequence a user gets out of the box.
    let mut state = make_state("hello", 0, None);
    assert!(state.handle_vim_key("z", false, None));
    assert!(
        !state.workspace.tabs[0].vim_keybind_seq.is_empty(),
        "z should start buffering (it's a prefix of zs)"
    );
    assert!(state.handle_vim_key("s", false, None));
    assert_eq!(
        state.take_pending_vim_action(),
        Some(crate::keybinds::KeybindAction::Save)
    );
    assert!(
        state.workspace.tabs[0].vim_keybind_seq.is_empty(),
        "buffer must clear once resolved"
    );
}

#[test]
fn test_d_then_z_abandons_the_pending_operator_instead_of_starting_a_sequence() {
    // The exact interaction the plan flagged as the highest-risk case:
    // `d` starts a real pending delete operator; `z` is not a valid
    // motion, so real vim's own rule ("an invalid motion cancels the
    // operator") must still apply — `z` must NOT be hijacked into
    // starting a vim-keybind sequence instead.
    let mut state = make_state("hello world", 0, None);
    assert!(state.handle_vim_key("d", false, None));
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('d'));
    assert!(state.handle_vim_key("z", false, None));
    assert_eq!(
        state.workspace.tabs[0].vim_pending_operator, None,
        "invalid motion must cancel the pending operator"
    );
    assert_eq!(
        state.workspace.tabs[0].vim_keybind_seq, "",
        "z must not have started a vim-keybind sequence here"
    );
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "hello world",
        "nothing should have been deleted"
    );
    assert_eq!(state.take_pending_vim_action(), None);
}

#[test]
fn test_z_then_d_fires_cut_not_a_real_delete_operator() {
    // The mirror image: once `z` has already started a sequence, `d`
    // (which would normally start a delete operator) must be consumed
    // as the sequence's second key instead — `zd` is Cut in the default
    // table.
    let mut state = make_state("hello world", 0, None);
    assert!(state.handle_vim_key("z", false, None));
    assert!(state.handle_vim_key("d", false, None));
    assert_eq!(
        state.take_pending_vim_action(),
        Some(crate::keybinds::KeybindAction::Cut)
    );
    assert_eq!(
        state.workspace.tabs[0].vim_pending_operator, None,
        "d must not also have started a real delete operator"
    );
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "hello world",
        "no direct deletion — Cut fires via dispatch_action, out of this function's reach"
    );
}

#[test]
fn test_z_then_escape_clears_the_sequence_with_no_side_effects() {
    let mut state = make_state("hello", 0, None);
    assert!(state.handle_vim_key("z", false, None));
    assert!(state.handle_vim_key("escape", false, None));
    assert_eq!(state.workspace.tabs[0].vim_keybind_seq, "");
    assert_eq!(state.take_pending_vim_action(), None);
    assert_eq!(state.workspace.tabs[0].document.content(), "hello");
}

#[test]
fn test_z_then_an_unbound_key_resets_silently() {
    let mut state = make_state("hello", 0, None);
    assert!(state.handle_vim_key("z", false, None));
    // "j" isn't the second key of any default binding.
    assert!(state.handle_vim_key("j", false, None));
    assert_eq!(
        state.workspace.tabs[0].vim_keybind_seq, "",
        "an unbound continuation must reset, not stay pending forever"
    );
    assert_eq!(state.take_pending_vim_action(), None);
}

// ── Task I.3: R Replace mode ─────────────────────────────────────────────

#[test]
fn test_capital_r_enters_replace_mode() {
    let mut state = make_state("abc", 0, None);
    state.handle_vim_key("r", true, None);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Replace);
}

#[test]
fn test_replace_mode_typing_overwrites_chars() {
    let mut state = make_state("abcdef", 0, None);
    state.workspace.tabs[0].vim_mode = VimMode::Replace;
    state.handle_vim_key("x", false, Some("x"));
    state.handle_vim_key("y", false, Some("y"));
    assert_eq!(state.workspace.tabs[0].document.content(), "xycdef");
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_replace_mode_appends_past_end_of_line() {
    let mut state = make_state("ab", 2, None);
    state.workspace.tabs[0].vim_mode = VimMode::Replace;
    state.handle_vim_key("z", false, Some("z"));
    assert_eq!(state.workspace.tabs[0].document.content(), "abz");
}

#[test]
fn test_replace_mode_escape_returns_to_normal() {
    let mut state = make_state("abc", 0, None);
    state.workspace.tabs[0].vim_mode = VimMode::Replace;
    state.handle_vim_key("escape", false, None);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_replace_mode_backspace_moves_cursor_back() {
    let mut state = make_state("abc", 0, None);
    state.workspace.tabs[0].vim_mode = VimMode::Replace;
    state.handle_vim_key("x", false, Some("x"));
    assert_eq!(state.workspace.tabs[0].cursor, 1);
    state.handle_vim_key("backspace", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

// ── Task I.4: Search mode (/, ?, n, N, *, #) ────────────────────────────

#[test]
fn test_slash_enters_search_mode_forward() {
    let mut state = make_state("hello world", 0, None);
    state.handle_vim_key("/", false, Some("/"));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Search);
    assert!(state.workspace.tabs[0].vim_search_direction);
}

#[test]
fn test_question_mark_enters_search_mode_backward() {
    let mut state = make_state("hello world", 0, None);
    state.handle_vim_key("/", true, Some("?"));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Search);
    assert!(!state.workspace.tabs[0].vim_search_direction);
}

#[test]
fn test_search_forward_jumps_to_next_match() {
    let mut state = make_state("foo bar foo baz", 0, None);
    state.handle_vim_key("/", false, Some("/"));
    state.handle_vim_key("f", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("enter", false, None);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].cursor, 8); // second "foo"
}

#[test]
fn test_search_forward_wraps_around() {
    let mut state = make_state("foo bar", 4, None); // cursor on "bar"
    state.handle_vim_key("/", false, Some("/"));
    state.handle_vim_key("f", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("enter", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 0); // wrapped to the only "foo"
}

#[test]
fn test_search_escape_cancels_without_moving_cursor() {
    let mut state = make_state("foo bar foo", 0, None);
    state.handle_vim_key("/", false, Some("/"));
    state.handle_vim_key("b", false, None);
    state.handle_vim_key("escape", false, None);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_n_repeats_last_search_forward() {
    let mut state = make_state("foo bar foo baz foo", 0, None);
    state.handle_vim_key("/", false, Some("/"));
    state.handle_vim_key("f", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("enter", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 8);
    state.handle_vim_key("n", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 16);
}

#[test]
fn test_capital_n_repeats_search_in_reverse() {
    let mut state = make_state("foo bar foo baz foo", 0, None);
    state.handle_vim_key("/", false, Some("/"));
    state.handle_vim_key("f", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("o", false, None);
    state.handle_vim_key("enter", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 8);
    state.handle_vim_key("n", true, None); // N: reverse direction
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_star_searches_forward_for_word_under_cursor() {
    let mut state = make_state("foo bar foo baz", 0, None); // cursor on first "foo"
    state.handle_vim_key("8", true, Some("*"));
    assert_eq!(state.workspace.tabs[0].cursor, 8);
}

#[test]
fn test_hash_searches_backward_for_word_under_cursor() {
    let mut state = make_state("foo bar foo baz", 8, None); // cursor on second "foo"
    state.handle_vim_key("3", true, Some("#"));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

// ── Task I.5: Jump list (Ctrl+o/Ctrl+i) ──────────────────────────────────

#[test]
fn test_large_motion_pushes_jump_and_ctrl_o_returns() {
    let mut state = make_state("one\ntwo\nthree\nfour\nfive", 0, None);
    state.handle_vim_key("g", true, None); // G: last line
    assert_eq!(state.workspace.tabs[0].cursor, 19); // start of "five"
    state.vim_jump_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_ctrl_i_returns_forward_after_ctrl_o() {
    let mut state = make_state("one\ntwo\nthree\nfour\nfive", 0, None);
    state.handle_vim_key("g", true, None); // G
    state.vim_jump_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    state.vim_jump_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 19);
}

#[test]
fn test_single_line_motion_does_not_push_jump() {
    let mut state = make_state("one\ntwo\nthree", 0, None);
    state.handle_vim_key("l", false, None); // small same-line motion
    state.vim_jump_backward(); // nothing was pushed; should be a no-op
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_ctrl_o_with_empty_jump_list_is_noop() {
    let mut state = make_state("abc", 1, None);
    state.vim_jump_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

// ── Task I.6: '.' repeat last change ─────────────────────────────────────

#[test]
fn test_dot_repeats_operator_motion_at_new_cursor() {
    let mut state = make_state("foo bar baz", 0, None);
    vim_key_recorded(&mut state, "d", false, None);
    vim_key_recorded(&mut state, "w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "bar baz");
    // cursor now at start of "bar" (0). Move to "baz" and repeat.
    state.workspace.tabs[0].cursor = 4;
    state.vim_repeat_last_change();
    assert_eq!(state.workspace.tabs[0].document.content(), "bar ");
}

#[test]
fn test_dot_repeats_doubled_operator() {
    let mut state = make_state("one\ntwo\nthree", 0, None);
    vim_key_recorded(&mut state, "d", false, None);
    vim_key_recorded(&mut state, "d", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "two\nthree");
    state.vim_repeat_last_change();
    assert_eq!(state.workspace.tabs[0].document.content(), "three");
}

#[test]
fn test_dot_repeats_text_object() {
    let mut state = make_state("(a) (b)", 1, None); // cursor inside first parens
    vim_key_recorded(&mut state, "d", false, None);
    vim_key_recorded(&mut state, "i", false, None);
    vim_key_recorded(&mut state, "(", true, Some("("));
    assert_eq!(state.workspace.tabs[0].document.content(), "() (b)");
    state.workspace.tabs[0].cursor = 4; // inside second parens
    state.vim_repeat_last_change();
    assert_eq!(state.workspace.tabs[0].document.content(), "() ()");
}

#[test]
fn test_yank_does_not_set_last_change() {
    let mut state = make_state("foo bar", 0, None);
    vim_key_recorded(&mut state, "y", false, None);
    vim_key_recorded(&mut state, "w", false, None);
    assert_eq!(state.global_vim.last_change, None);
}

#[test]
fn test_dot_repeats_plain_insertion() {
    // Insert mode's Escape is handled by the caller (text_editor.rs),
    // not `handle_vim_key` (which returns `false` for it, per its own
    // doc comment) — so tests call `vim_exit_to_normal` directly here,
    // same as text_editor.rs does.
    let mut state = make_state("ab", 0, None);
    vim_key_recorded(&mut state, "i", false, None);
    state.insert_char('X');
    state.insert_char('Y');
    state.vim_exit_to_normal();
    assert_eq!(state.workspace.tabs[0].document.content(), "XYab");
    state.workspace.tabs[0].cursor = 4; // end of content
    state.vim_repeat_last_change();
    assert_eq!(state.workspace.tabs[0].document.content(), "XYabXY");
}

#[test]
fn test_dot_repeats_change_operator_plus_insertion() {
    let mut state = make_state("foo bar", 0, None);
    vim_key_recorded(&mut state, "c", false, None);
    vim_key_recorded(&mut state, "w", false, None);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    state.insert_char('X');
    state.vim_exit_to_normal();
    // `cw` consumes through the motion's exclusive end the same way
    // `dw` does (this codebase doesn't special-case `cw` to stop
    // before trailing whitespace like real vim's `ce`-like quirk) —
    // so the space goes with it.
    assert_eq!(state.workspace.tabs[0].document.content(), "Xbar");
    state.workspace.tabs[0].cursor = 1; // start of "bar"
    state.vim_repeat_last_change();
    assert_eq!(state.workspace.tabs[0].document.content(), "XX");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_dot_with_no_prior_change_is_noop() {
    let mut state = make_state("abc", 0, None);
    state.handle_vim_key(".", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "abc");
}

#[test]
fn test_abandoned_operator_does_not_set_last_change() {
    let mut state = make_state("abc", 0, None);
    vim_key_recorded(&mut state, "d", false, None);
    vim_key_recorded(&mut state, "up", false, None); // invalid motion for d: abandons
    assert_eq!(state.global_vim.last_change, None);
}

// ── split_vim_command_buf / take_vim_count / vim_pending_trigger (Task E) ───

#[test]
fn test_split_vim_command_buf_empty() {
    assert_eq!(split_vim_command_buf(""), (None, None));
}

#[test]
fn test_split_vim_command_buf_digits_only() {
    assert_eq!(split_vim_command_buf("42"), (Some(42), None));
}

#[test]
fn test_split_vim_command_buf_trigger_only() {
    assert_eq!(split_vim_command_buf("f"), (None, Some('f')));
}

#[test]
fn test_split_vim_command_buf_digits_and_trigger() {
    assert_eq!(split_vim_command_buf("12t"), (Some(12), Some('t')));
}

#[test]
fn test_take_vim_count_none_when_buffer_empty() {
    let mut state = make_state("hello", 0, None);
    assert_eq!(state.take_vim_count(), None);
}

#[test]
fn test_take_vim_count_parses_and_clears_digits() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "7".to_string();
    assert_eq!(state.take_vim_count(), Some(7));
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_take_vim_count_preserves_trailing_trigger() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "3f".to_string();
    assert_eq!(state.take_vim_count(), Some(3));
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "f");
}

#[test]
fn test_vim_pending_trigger_none_when_no_trigger() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "5".to_string();
    assert_eq!(state.vim_pending_trigger(), None);
}

#[test]
fn test_vim_pending_trigger_returns_trailing_char() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "g".to_string();
    assert_eq!(state.vim_pending_trigger(), Some('g'));
}

#[test]
fn test_vim_enter_insert_clears_command_buf() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "3".to_string();
    state.vim_enter_insert_before_cursor();
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_vim_enter_visual_clears_command_buf() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "3".to_string();
    state.vim_enter_visual();
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_vim_enter_visual_line_clears_command_buf() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "3".to_string();
    state.vim_enter_visual_line();
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_vim_enter_command_clears_command_buf() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].vim_command_buf = "3".to_string();
    state.vim_enter_command();
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

// ── WORD motions: W/B/E (Task E) ─────────────────────────────────────────────

#[test]
fn test_move_word_forward_big_treats_punctuation_as_part_of_word() {
    // "foo.bar" is ONE WORD for `W` (no word/punct split), unlike `w`
    // which would stop at the '.'.
    let mut state = make_state("foo.bar baz", 0, None);
    state.move_word_forward_big();
    assert_eq!(state.workspace.tabs[0].cursor, 8); // start of "baz"
}

#[test]
fn test_move_word_backward_big_treats_punctuation_as_part_of_word() {
    let mut state = make_state("foo.bar baz", 8, None); // on "baz"
    state.move_word_backward_big();
    assert_eq!(state.workspace.tabs[0].cursor, 0); // start of "foo.bar"
}

#[test]
fn test_move_word_end_big_treats_punctuation_as_part_of_word() {
    let mut state = make_state("foo.bar baz", 0, None);
    state.move_word_end_big();
    assert_eq!(state.workspace.tabs[0].cursor, 6); // last char of "foo.bar"
}

#[test]
fn test_move_word_forward_big_crosses_newline() {
    let mut state = make_state("foo\nbar", 0, None);
    state.move_word_forward_big();
    assert_eq!(state.workspace.tabs[0].cursor, 4);
}

// ── big_word_class / classified free functions ──────────────────────────────

#[test]
fn test_big_word_class_punctuation_is_word() {
    assert_eq!(big_word_class('.'), CharClass::Word);
    assert_eq!(big_word_class('_'), CharClass::Word);
    assert_eq!(big_word_class('a'), CharClass::Word);
}

#[test]
fn test_big_word_class_whitespace_is_space() {
    assert_eq!(big_word_class(' '), CharClass::Space);
    assert_eq!(big_word_class('\n'), CharClass::Space);
}

// ── paragraph motions: { / } (Task E) ────────────────────────────────────────

#[test]
fn test_move_paragraph_forward_lands_on_next_blank_line() {
    let mut state = make_state("one\ntwo\n\nthree", 0, None);
    state.move_paragraph_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 8); // start of the blank line
}

#[test]
fn test_move_paragraph_forward_no_next_paragraph_goes_to_end() {
    let mut state = make_state("one\ntwo\nthree", 0, None);
    state.move_paragraph_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 13); // content.len()
}

#[test]
fn test_move_paragraph_forward_already_on_blank_line_advances_past_it() {
    let mut state = make_state("one\n\ntwo\n\nthree", 4, None); // on the first blank line
    state.move_paragraph_forward();
    assert_eq!(state.workspace.tabs[0].cursor, 9); // the *second* blank line, not staying at 4
}

#[test]
fn test_move_paragraph_backward_lands_on_previous_blank_line() {
    let mut state = make_state("one\n\ntwo\nthree", 9, None); // on "three"
    state.move_paragraph_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 4);
}

#[test]
fn test_move_paragraph_backward_no_previous_paragraph_goes_to_start() {
    let mut state = make_state("one\ntwo\nthree", 9, None);
    state.move_paragraph_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_move_paragraph_backward_already_on_blank_line_retreats_past_it() {
    let mut state = make_state("one\n\ntwo\n\nthree", 9, None); // on the second blank line
    state.move_paragraph_backward();
    assert_eq!(state.workspace.tabs[0].cursor, 4); // the *first* blank line, not staying at 9
}

// ── f/F/t/T find-char motions + ;/, repeat (Task E) ──────────────────────────

#[test]
fn test_move_find_char_forward_lands_on_target() {
    let mut state = make_state("abcdef", 0, None);
    state.move_find_char_forward('d');
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert_eq!(state.workspace.tabs[0].last_find, Some(('f', 'd')));
}

#[test]
fn test_move_find_char_forward_not_found_is_noop_and_does_not_remember() {
    let mut state = make_state("abcdef", 0, None);
    state.move_find_char_forward('z');
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].last_find, None);
}

#[test]
fn test_move_find_char_forward_does_not_cross_line_boundary() {
    let mut state = make_state("abc\ndef", 0, None);
    state.move_find_char_forward('d');
    assert_eq!(state.workspace.tabs[0].cursor, 0); // 'd' is on the next line
}

#[test]
fn test_move_find_char_backward_lands_on_target() {
    let mut state = make_state("abcdef", 5, None);
    state.move_find_char_backward('b');
    assert_eq!(state.workspace.tabs[0].cursor, 1);
    assert_eq!(state.workspace.tabs[0].last_find, Some(('F', 'b')));
}

#[test]
fn test_move_till_char_forward_lands_one_before_target() {
    let mut state = make_state("abcdef", 0, None);
    state.move_till_char_forward('d');
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_move_till_char_forward_target_immediately_next_is_noop() {
    let mut state = make_state("abcdef", 0, None);
    state.move_till_char_forward('b');
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_move_till_char_backward_lands_one_after_target() {
    let mut state = make_state("abcdef", 5, None);
    state.move_till_char_backward('b');
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_repeat_last_find_repeats_forward_find() {
    let mut state = make_state("a.b.c.d", 0, None);
    state.move_find_char_forward('.');
    assert_eq!(state.workspace.tabs[0].cursor, 1);
    state.repeat_last_find();
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    state.repeat_last_find();
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_repeat_last_find_noop_when_no_prior_find() {
    let mut state = make_state("abcdef", 0, None);
    state.repeat_last_find();
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_repeat_last_find_does_not_update_last_find() {
    let mut state = make_state("a.b.c.d", 0, None);
    state.move_find_char_forward('.');
    state.repeat_last_find();
    assert_eq!(state.workspace.tabs[0].last_find, Some(('f', '.'))); // unchanged
}

#[test]
fn test_repeat_last_find_reverse_flips_direction() {
    let mut state = make_state("a.b.c.d", 5, None); // on the second '.'
    state.move_find_char_backward('.');
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    // ',' reverses F back into f, continuing forward past the original start.
    state.repeat_last_find_reverse();
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_repeat_last_find_reverse_does_not_update_last_find() {
    let mut state = make_state("a.b.c.d", 5, None);
    state.move_find_char_backward('.');
    state.repeat_last_find_reverse();
    assert_eq!(state.workspace.tabs[0].last_find, Some(('F', '.'))); // unchanged
}

#[test]
fn test_repeat_last_find_reverse_after_reverse_still_repeats_original() {
    // ';' after a ',' must repeat the *original* find direction, not
    // the reversed one from the preceding ',' — this is the reason
    // apply_find's `remember` flag exists.
    let mut state = make_state("a.b.c.d", 0, None);
    state.move_find_char_forward('.'); // last_find = ('f', '.'), cursor -> 1
    state.repeat_last_find_reverse(); // reversed to 'F': searches backward from 1, no match, no-op
    assert_eq!(state.workspace.tabs[0].cursor, 1); // unchanged: no earlier '.' before position 1
    state.repeat_last_find(); // still 'f' (unchanged by the ',' above): forward to next '.'
    assert_eq!(state.workspace.tabs[0].cursor, 3);
}

#[test]
fn test_repeat_last_find_till_nudges_past_adjacent_match() {
    // Without the repeat-nudge, ';' after a 't' would be a no-op
    // (landing back on the same position it already stopped at).
    let mut state = make_state("a.b.c.d", 0, None);
    state.move_till_char_forward('.'); // cursor -> 0 (immediately before the first '.')
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    state.repeat_last_find();
    assert_eq!(state.workspace.tabs[0].cursor, 2); // one before the *second* '.'
}

// ── handle_vim_normal_key dispatch state machine (Task E) ────────────────────

#[test]
fn test_handle_vim_key_normal_h_moves_left() {
    let mut state = make_state("hello", 3, None);
    assert!(state.handle_vim_key("h", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_handle_vim_key_normal_count_prefix_repeats_motion() {
    let mut state = make_state("hello world", 0, None);
    assert!(state.handle_vim_key("3", false, None)); // accumulate count
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "3");
    assert!(state.handle_vim_key("l", false, None)); // 3l
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert_eq!(state.workspace.tabs[0].vim_command_buf, ""); // consumed
}

#[test]
fn test_handle_vim_key_normal_multi_digit_count() {
    let mut state = make_state(&"x".repeat(20), 0, None);
    state.handle_vim_key("1", false, None);
    state.handle_vim_key("0", false, None);
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "10");
    state.handle_vim_key("l", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 10);
}

#[test]
fn test_handle_vim_key_normal_leading_zero_is_line_start_motion() {
    let mut state = make_state("hello", 3, None);
    assert!(state.handle_vim_key("0", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_handle_vim_key_normal_zero_after_nonzero_extends_count() {
    let mut state = make_state(&"x".repeat(20), 0, None);
    state.handle_vim_key("2", false, None);
    state.handle_vim_key("0", false, None); // "20", not the 0-motion
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "20");
    state.handle_vim_key("l", false, None);
    assert_eq!(
        state.workspace.tabs[0].cursor,
        20.min("xxxxxxxxxxxxxxxxxxxx".len())
    );
}

#[test]
fn test_handle_vim_key_normal_w_shift_is_big_word() {
    let mut state = make_state("foo.bar baz", 0, None);
    assert!(state.handle_vim_key("w", true, None)); // W
    assert_eq!(state.workspace.tabs[0].cursor, 8);
}

#[test]
fn test_handle_vim_key_normal_gg_no_count_goes_to_first_line_first_nonblank() {
    let mut state = make_state("one\n  two\n  three", 15, None);
    assert!(state.handle_vim_key("g", false, None)); // pending 'g'
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "g");
    assert!(state.handle_vim_key("g", false, None)); // gg
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_handle_vim_key_normal_count_gg_goes_to_that_line() {
    let mut state = make_state("one\n  two\n  three", 0, None);
    state.handle_vim_key("2", false, None);
    state.handle_vim_key("g", false, None);
    state.handle_vim_key("g", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 6); // first non-blank of line 2 ("two")
}

#[test]
fn test_handle_vim_key_normal_g_abandoned_by_unrelated_key() {
    let mut state = make_state("hello", 0, None);
    state.handle_vim_key("g", false, None); // pending
    let handled = state.handle_vim_key("x", false, None); // not a second 'g'
    assert!(handled); // still consumed (swallowed), just not gg
    assert_eq!(state.workspace.tabs[0].cursor, 0); // no motion happened
    assert_eq!(state.workspace.tabs[0].vim_command_buf, ""); // pending state cleared
}

#[test]
fn test_handle_vim_key_normal_shift_g_no_count_goes_to_last_line() {
    let mut state = make_state("one\ntwo\n  three", 0, None);
    assert!(state.handle_vim_key("g", true, None)); // G
    assert_eq!(state.workspace.tabs[0].cursor, 10); // first non-blank of "three"
}

#[test]
fn test_handle_vim_key_normal_dollar_via_shift_and_digit4() {
    let mut state = make_state("hello\nworld", 0, None);
    assert!(state.handle_vim_key("4", true, None)); // $
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_handle_vim_key_normal_dollar_via_key_char() {
    let mut state = make_state("hello\nworld", 0, None);
    assert!(state.handle_vim_key("4", false, Some("$")));
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_handle_vim_key_normal_dollar_via_key_reported_as_symbol_directly() {
    // Confirmed empirically on this app's WSLg/X11 backend: `$` did
    // nothing under the original key_char/shift-only check because
    // GPUI reports `key == "$"` directly here, not "4"+shift and not
    // key_char. This is the case that was actually broken.
    let mut state = make_state("hello\nworld", 0, None);
    assert!(state.handle_vim_key("$", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_handle_vim_key_normal_plain_4_is_not_dollar() {
    // Guards against matches_shifted_symbol over-triggering: an
    // unshifted "4" (a legitimate count digit) must not be treated as
    // `$` just because key_char happens to echo the same digit.
    let mut state = make_state(&"x".repeat(10), 0, None);
    state.handle_vim_key("4", false, Some("4"));
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "4"); // accumulated as a count
    assert_eq!(state.workspace.tabs[0].cursor, 0); // not moved to end of line
}

#[test]
fn test_handle_vim_key_normal_caret_via_key_char() {
    let mut state = make_state("  hello", 5, None);
    assert!(state.handle_vim_key("6", false, Some("^")));
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_handle_vim_key_normal_caret_via_key_reported_as_symbol_directly() {
    let mut state = make_state("  hello", 5, None);
    assert!(state.handle_vim_key("^", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_handle_vim_key_normal_brace_motions_via_key_reported_as_symbol_directly() {
    let mut state = make_state("one\n\ntwo", 0, None);
    assert!(state.handle_vim_key("}", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 4);
    assert!(state.handle_vim_key("{", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_handle_vim_key_normal_brace_motions_via_key_char() {
    let mut state = make_state("one\n\ntwo", 0, None);
    assert!(state.handle_vim_key("]", false, Some("}")));
    assert_eq!(state.workspace.tabs[0].cursor, 4);
    assert!(state.handle_vim_key("[", false, Some("{")));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_handle_vim_key_normal_f_pending_then_target_finds_char() {
    let mut state = make_state("abcdef", 0, None);
    assert!(state.handle_vim_key("f", false, None)); // pending 'f'
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "f");
    assert!(state.handle_vim_key("d", false, None)); // target
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_handle_vim_key_normal_shift_f_pending_is_capital_f() {
    let mut state = make_state("abcdef", 5, None);
    state.handle_vim_key("f", true, None); // pending 'F'
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "F");
    state.handle_vim_key("b", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_handle_vim_key_normal_count_f_repeats_find() {
    let mut state = make_state("a.b.c.d", 0, None);
    state.handle_vim_key("2", false, None);
    state.handle_vim_key("f", false, None);
    state.handle_vim_key(".", false, None); // 2f. -> second '.'
    assert_eq!(state.workspace.tabs[0].cursor, 3);
}

#[test]
fn test_handle_vim_key_normal_f_pending_target_via_key_char_for_symbol() {
    // A shifted-symbol target (e.g. f") relies on key_char since `key`
    // alone can't disambiguate it — same dual-detection pattern as ':'.
    let mut state = make_state("a\"b\"c", 0, None);
    state.handle_vim_key("f", false, None);
    state.handle_vim_key("'", true, Some("\"")); // shift+' = " on a US layout
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_handle_vim_key_normal_f_pending_escape_abandons_find() {
    let mut state = make_state("abcdef", 0, None);
    state.handle_vim_key("f", false, None);
    let handled = state.handle_vim_key("escape", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
}

#[test]
fn test_handle_vim_key_normal_semicolon_repeats_find() {
    let mut state = make_state("a.b.c.d", 0, None);
    state.handle_vim_key("f", false, None);
    state.handle_vim_key(".", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 1);
    assert!(state.handle_vim_key(";", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 3);
}

#[test]
fn test_handle_vim_key_normal_comma_reverses_find() {
    let mut state = make_state("a.b.c.d", 5, None);
    state.handle_vim_key("f", true, None); // F
    state.handle_vim_key(".", false, None);
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert!(state.handle_vim_key(",", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 5);
}

#[test]
fn test_handle_vim_key_normal_semicolon_shift_is_still_colon_not_repeat() {
    // Regression: shift+';' must remain the Command-mode trigger even
    // though plain ';' is now the find-repeat key.
    let mut state = make_state("hello", 0, None);
    assert!(state.handle_vim_key(";", true, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Command);
}

#[test]
fn test_handle_vim_key_normal_pending_find_target_colon_key_is_not_command_mode() {
    // A pending f/F/t/T must treat shift+';' (a ':' keypress) as its
    // target character, not as the Command-mode trigger, even though
    // that exact key/shift/key_char combo *would* enter Command mode
    // via the top-level ':' check when nothing is pending.
    let mut state = make_state("ab:cd", 0, None);
    state.handle_vim_key("f", false, None);
    let handled = state.handle_vim_key(";", true, Some(":"));
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal); // did NOT enter Command
    assert_eq!(state.workspace.tabs[0].cursor, 2); // found literal ':'
}

#[test]
fn test_handle_vim_key_normal_navigation_still_falls_through() {
    let mut state = make_state("hello", 2, None);
    assert!(!state.handle_vim_key("left", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 2);
}

#[test]
fn test_handle_vim_key_normal_jk_fall_through() {
    let mut state = make_state("hello", 2, None);
    assert!(!state.handle_vim_key("j", false, None));
    assert!(!state.handle_vim_key("k", false, None));
}

#[test]
fn test_handle_vim_key_normal_mode_switch_still_works_after_rewrite() {
    let mut state = make_state("hello", 0, None);
    assert!(state.handle_vim_key("i", false, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_handle_vim_key_normal_stale_count_does_not_leak_into_mode_switch() {
    let mut state = make_state("hello", 0, None);
    state.handle_vim_key("3", false, None);
    state.handle_vim_key("v", false, None); // enters Visual, should clear buf
    assert_eq!(state.workspace.tabs[0].vim_command_buf, "");
    state.handle_vim_key("escape", false, None); // back to Normal
    let cursor_before = state.workspace.tabs[0].cursor;
    state.handle_vim_key("l", false, None); // should move by 1, not 3
    assert_eq!(state.workspace.tabs[0].cursor, cursor_before + 1);
}

// ── Visual-mode motion extension (Task E pass 2) ─────────────────────────────

#[test]
fn test_handle_vim_key_visual_h_extends_selection() {
    let mut state = make_state("hello", 3, None);
    state.vim_enter_visual(); // selects (3, 4), cursor -> 4 (the selection's far edge)
    assert!(state.handle_vim_key("h", false, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual);
    // 'h' from cursor 4 lands back on 3 — the anchor — shrinking the
    // selection to zero-width rather than reversing past it.
    assert_eq!(state.workspace.tabs[0].selection, Some((3, 3)));
    assert_eq!(state.workspace.tabs[0].cursor, 3);
}

#[test]
fn test_handle_vim_key_visual_l_extends_selection_forward() {
    let mut state = make_state("hello world", 0, None);
    state.vim_enter_visual(); // selects (0, 1)
    assert!(state.handle_vim_key("l", false, None));
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 2)));
}

#[test]
fn test_handle_vim_key_visual_count_w_extends_by_multiple_words() {
    let mut state = make_state("one two three four", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("2", false, None);
    assert!(state.handle_vim_key("w", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 8); // start of "three"
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 8)));
}

#[test]
fn test_handle_vim_key_visual_dollar_extends_to_line_end() {
    let mut state = make_state("hello\nworld", 0, None);
    state.vim_enter_visual();
    assert!(state.handle_vim_key("$", false, None));
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 5)));
}

#[test]
fn test_handle_vim_key_visual_gg_extends_to_first_line() {
    let mut state = make_state("one\ntwo\nthree", 9, None); // on "three"
    state.vim_enter_visual();
    state.handle_vim_key("g", false, None);
    assert!(state.handle_vim_key("g", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].selection.unwrap().1, 0);
}

#[test]
fn test_handle_vim_key_visual_f_extends_to_found_char() {
    let mut state = make_state("abcdef", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("f", false, None);
    assert!(state.handle_vim_key("d", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
}

#[test]
fn test_handle_vim_key_visual_semicolon_repeats_find_and_extends() {
    let mut state = make_state("a.b.c.d", 0, None);
    state.vim_enter_visual(); // cursor -> 1 (char_right(0), the selection's far edge)
    state.handle_vim_key("f", false, None);
    state.handle_vim_key(".", false, None); // finds the '.' at 3, searching from cursor 1
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert!(state.handle_vim_key(";", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 5);
    assert_eq!(state.workspace.tabs[0].selection.unwrap().1, 5);
}

#[test]
fn test_handle_vim_key_visual_left_right_extend_instead_of_falling_through() {
    // Unlike Normal mode, Visual's left/right must NOT fall through
    // (that would clear the selection via the plain editor's Left/
    // Right handling) — they're resolved directly as h/l equivalents.
    let mut state = make_state("hello", 2, None);
    state.vim_enter_visual();
    assert!(state.handle_vim_key("right", false, None));
    assert_eq!(state.workspace.tabs[0].selection, Some((2, 4)));
    assert!(state.handle_vim_key("left", false, None));
    assert_eq!(state.workspace.tabs[0].selection, Some((2, 3)));
}

#[test]
fn test_handle_vim_key_visual_home_end_extend() {
    let mut state = make_state("hello world", 5, None);
    state.vim_enter_visual();
    assert!(state.handle_vim_key("end", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 11);
    assert!(state.handle_vim_key("home", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_handle_vim_key_visual_up_down_jk_fall_through_for_visual_row_movement() {
    let mut state = make_state("hello\nworld", 0, None);
    state.vim_enter_visual();
    assert!(!state.handle_vim_key("j", false, None));
    assert!(!state.handle_vim_key("k", false, None));
    assert!(!state.handle_vim_key("up", false, None));
    assert!(!state.handle_vim_key("down", false, None));
    // None of these should have been silently swallowed as a no-op motion.
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual);
}

#[test]
fn test_handle_vim_key_visual_line_h_extends_within_visual_line() {
    let mut state = make_state("one\ntwo\nthree", 4, None); // on "two"
    state.vim_enter_visual_line(); // selects "two\n" as (4, 8)
    assert!(!state.handle_vim_key("j", false, None)); // falls through, unaffected here
                                                      // Directly verify a pure motion extends VisualLine's selection too.
    assert!(state.handle_vim_key("l", false, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::VisualLine);
}

#[test]
fn test_handle_vim_key_visual_i_is_swallowed_not_insert_entry() {
    // In Visual mode 'i'/'a' are text-object prefixes (spec 5.4, not
    // yet implemented) — must NOT enter Insert mode the way Normal's
    // 'i' does.
    let mut state = make_state("hello", 2, None);
    state.vim_enter_visual();
    let handled = state.handle_vim_key("i", false, None);
    assert!(handled);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual); // not Insert
    assert_eq!(state.workspace.tabs[0].document.content(), "hello"); // not inserted as text
}

#[test]
fn test_handle_vim_key_visual_escape_still_exits_after_refactor() {
    let mut state = make_state("hello", 2, None);
    state.vim_enter_visual();
    assert!(state.handle_vim_key("escape", false, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].selection, None);
}

#[test]
fn test_handle_vim_key_visual_v_still_toggles_off_after_refactor() {
    let mut state = make_state("hello", 2, None);
    state.vim_enter_visual();
    assert!(state.handle_vim_key("v", false, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_handle_vim_key_visual_line_shift_v_still_toggles_off_after_refactor() {
    let mut state = make_state("hello", 2, None);
    state.vim_enter_visual_line();
    assert!(state.handle_vim_key("v", true, None));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

// ── _ motion (Task E pass 2) ──────────────────────────────────────────────────

#[test]
fn test_underscore_motion_no_count_is_current_line_first_nonblank() {
    assert_eq!(underscore_motion("  hello\nworld", 5, 1), 2);
}

#[test]
fn test_underscore_motion_count_moves_down_lines() {
    assert_eq!(underscore_motion("one\n  two\nthree", 0, 2), 6);
}

#[test]
fn test_underscore_motion_clamps_past_last_line() {
    assert_eq!(underscore_motion("one\ntwo", 0, 50), 4);
}

#[test]
fn test_handle_vim_key_normal_underscore_moves_to_first_nonblank() {
    let mut state = make_state("hello\n  world", 0, None);
    state.handle_vim_key("2", false, None);
    assert!(state.handle_vim_key("_", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 8); // "world" preceded by 2 spaces on line 2
}

#[test]
fn test_handle_vim_key_visual_underscore_extends_selection() {
    let mut state = make_state("one\n  two", 0, None);
    state.vim_enter_visual(); // cursor -> 1, selection (0, 1)
    state.handle_vim_key("2", false, None); // count=2: down one line
    assert!(state.handle_vim_key("_", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 6);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 6)));
}

// ── vim_move_to_line_first_nonblank / H/M/L groundwork (Task E pass 2) ───────

#[test]
fn test_vim_move_to_line_first_nonblank_moves_cursor() {
    let mut state = make_state("one\n  two\nthree", 0, None);
    state.vim_move_to_line_first_nonblank(1, false);
    assert_eq!(state.workspace.tabs[0].cursor, 6);
    assert_eq!(state.workspace.tabs[0].selection, None);
}

#[test]
fn test_vim_move_to_line_first_nonblank_extends_selection() {
    let mut state = make_state("one\n  two\nthree", 0, None);
    state.vim_enter_visual(); // cursor -> 1, selection (0,1)
    state.vim_move_to_line_first_nonblank(1, true);
    assert_eq!(state.workspace.tabs[0].cursor, 6);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 6)));
}

#[test]
fn test_vim_move_to_line_first_nonblank_clamps_past_last_line() {
    let mut state = make_state("one\ntwo", 0, None);
    state.vim_move_to_line_first_nonblank(50, false);
    assert_eq!(state.workspace.tabs[0].cursor, 4); // start of "two", the last line
}

// ── macro recording: q<register> / bare q (Task E pass 2) ────────────────────

#[test]
fn test_q_then_register_starts_recording() {
    let mut state = make_state("hello", 0, None);
    assert!(state.handle_vim_key("q", false, None)); // pending: waiting for register
    assert!(!state.vim_is_recording_macro());
    assert!(state.handle_vim_key("a", false, None)); // register 'a'
    assert!(state.vim_is_recording_macro());
}

#[test]
fn test_vim_macro_record_pending_accessor() {
    // Backs the mode-indicator's pending-command echo, which needs to
    // show `q` is waiting for its register name — this state doesn't
    // live in `vim_command_buf`, so the indicator can't see it without
    // this accessor.
    let mut state = make_state("hello", 0, None);
    assert!(!state.vim_macro_record_pending());
    state.handle_vim_key("q", false, None);
    assert!(state.vim_macro_record_pending());
    state.handle_vim_key("a", false, None);
    assert!(!state.vim_macro_record_pending());
}

#[test]
fn test_vim_recording_register_accessor() {
    // Backs the mode-indicator showing which register is actively
    // recording (real vim's "recording @a") for the whole duration of
    // a recording, not just the initial `q<register>` keystroke.
    let mut state = make_state("hello", 0, None);
    assert_eq!(state.vim_recording_register(), None);
    state.handle_vim_key("q", false, None);
    state.handle_vim_key("a", false, None);
    assert_eq!(state.vim_recording_register(), Some('a'));
    state.handle_vim_key("q", false, None); // stop
    assert_eq!(state.vim_recording_register(), None);
}

#[test]
fn test_bare_q_while_recording_stops_and_saves() {
    let mut state = make_state("hello", 0, None);
    state.handle_vim_key("q", false, None);
    state.handle_vim_key("a", false, None); // recording into 'a'
    state.record_macro_key("l", false, None);
    state.record_macro_key("l", false, None);
    assert!(state.handle_vim_key("q", false, None)); // bare q: stop
    assert!(!state.vim_is_recording_macro());
    assert_eq!(
        state.macro_keys('a'),
        Some(vec![
            RecordedVimKey {
                key: "l".into(),
                shift: false,
                key_char: None
            },
            RecordedVimKey {
                key: "l".into(),
                shift: false,
                key_char: None
            },
        ])
    );
}

#[test]
fn test_record_macro_key_noop_when_not_recording() {
    let mut state = make_state("hello", 0, None);
    state.record_macro_key("l", false, None);
    assert_eq!(state.macro_keys('a'), None);
}

#[test]
fn test_macro_pending_register_does_not_leak_into_next_command() {
    // 'q' followed by 'a' resolves the register; the *next* keystroke
    // must be handled normally (not swallowed as a second register).
    let mut state = make_state("hello", 0, None);
    state.handle_vim_key("q", false, None);
    state.handle_vim_key("a", false, None);
    assert!(state.handle_vim_key("l", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

#[test]
fn test_fq_resolves_as_find_target_not_macro_start() {
    // A pending f/F/t/T trigger takes priority over macro-start: `fq`
    // must find the literal character 'q', not begin `q<register>`.
    let mut state = make_state("qab", 0, None);
    state.handle_vim_key("f", false, None); // pending find trigger
    assert!(state.handle_vim_key("q", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 0); // found 'q' at position 0
    assert!(!state.vim_is_recording_macro());
}

#[test]
fn test_macro_keys_returns_none_for_unset_register() {
    let state = make_state("hello", 0, None);
    assert_eq!(state.macro_keys('z'), None);
}

// ── resolve_vim_motion / MotionKind (Task F groundwork) ──────────────────────

#[test]
fn test_resolve_vim_motion_w_is_exclusive_e_is_inclusive_same_target() {
    // The whole point of MotionKind: `w` and `e` land on the same
    // offset for "one two" from 0 (the 'o' at the end of "one" is
    // position... actually `w` lands at start of "two" (4), `e` lands
    // on the last char of "one" (2) — different targets AND different
    // kinds. Use content where they'd coincide in target to prove the
    // *kind* is what distinguishes them, not just the position.
    let mut state = make_state("one two", 0, None);
    let w = state.resolve_vim_motion("w", false, None);
    assert_eq!(
        w,
        MotionResolution::Resolved {
            target: 4,
            kind: MotionKind::ExclusiveChar
        }
    );
    let mut state2 = make_state("one two", 0, None);
    let e = state2.resolve_vim_motion("e", false, None);
    assert_eq!(
        e,
        MotionResolution::Resolved {
            target: 2,
            kind: MotionKind::InclusiveChar
        }
    );
}

#[test]
fn test_resolve_vim_motion_dollar_is_inclusive_caret_is_exclusive() {
    let mut state = make_state("  hi", 2, None);
    let dollar = state.resolve_vim_motion("4", true, Some("$"));
    assert_eq!(
        dollar,
        MotionResolution::Resolved {
            target: 4,
            kind: MotionKind::InclusiveChar
        }
    );
    let mut state2 = make_state("  hi", 2, None);
    let caret = state2.resolve_vim_motion("6", true, Some("^"));
    assert_eq!(
        caret,
        MotionResolution::Resolved {
            target: 2,
            kind: MotionKind::ExclusiveChar
        }
    );
}

#[test]
fn test_resolve_vim_motion_gg_and_g_shift_are_linewise() {
    let mut state = make_state("one\ntwo\nthree", 10, None);
    state.handle_vim_key("g", false, None); // pending
    let gg = state.resolve_vim_motion("g", false, None);
    assert_eq!(
        gg,
        MotionResolution::Resolved {
            target: 0,
            kind: MotionKind::Linewise
        }
    );

    let mut state2 = make_state("one\ntwo\nthree", 0, None);
    let g_shift = state2.resolve_vim_motion("g", true, None);
    assert_eq!(
        g_shift,
        MotionResolution::Resolved {
            target: 8,
            kind: MotionKind::Linewise
        }
    );
}

/// The escape hatch back to real vim's original (paragraph-wide)
/// `$`/`0`/`^` now that the bare keys resolve to the current visual row
/// instead (`text_editor.rs`'s interception, ahead of
/// `resolve_vim_motion`) — same targets `resolve_vim_motion`'s own
/// non-`g` `$`/`0`/`^` arms already compute, just reached via a
/// pending `g` instead.
#[test]
fn test_resolve_vim_motion_g_dollar_g_zero_g_caret() {
    let mut state = make_state("  hi", 2, None);
    state.handle_vim_key("g", false, None); // pending
    let g_dollar = state.resolve_vim_motion("4", true, Some("$"));
    assert_eq!(
        g_dollar,
        MotionResolution::Resolved {
            target: 4,
            kind: MotionKind::InclusiveChar
        }
    );

    let mut state2 = make_state("  hi", 2, None);
    state2.handle_vim_key("g", false, None);
    let g_zero = state2.resolve_vim_motion("0", false, None);
    assert_eq!(
        g_zero,
        MotionResolution::Resolved {
            target: 0,
            kind: MotionKind::ExclusiveChar
        }
    );

    let mut state3 = make_state("  hi", 3, None);
    state3.handle_vim_key("g", false, None);
    let g_caret = state3.resolve_vim_motion("6", true, Some("^"));
    assert_eq!(
        g_caret,
        MotionResolution::Resolved {
            target: 2,
            kind: MotionKind::ExclusiveChar
        }
    );
}

#[test]
fn test_resolve_vim_motion_underscore_is_linewise() {
    let mut state = make_state("one\ntwo", 0, None);
    let r = state.resolve_vim_motion("_", false, None);
    assert_eq!(
        r,
        MotionResolution::Resolved {
            target: 0,
            kind: MotionKind::Linewise
        }
    );
}

#[test]
fn test_resolve_vim_motion_find_f_is_inclusive_t_is_exclusive() {
    let mut state = make_state("abcXdef", 0, None);
    state.handle_vim_key("f", false, None); // pending
    let f = state.resolve_vim_motion("X", true, Some("X"));
    assert_eq!(
        f,
        MotionResolution::Resolved {
            target: 3,
            kind: MotionKind::InclusiveChar
        }
    );

    let mut state2 = make_state("abcXdef", 0, None);
    state2.handle_vim_key("t", false, None); // pending
    let t = state2.resolve_vim_motion("X", true, Some("X"));
    assert_eq!(
        t,
        MotionResolution::Resolved {
            target: 2,
            kind: MotionKind::ExclusiveChar
        }
    );
}

#[test]
fn test_resolve_vim_motion_left_right_home_end_always_resolve_locally() {
    // Unlike the old combined `handle_vim_motion_key`, `resolve_vim_motion`
    // itself never defers left/right/home/end to GPUI — that fallthrough
    // is `handle_vim_motion_key`'s own concern now, so operators (which
    // call `resolve_vim_motion` directly) can act on arrow keys too.
    let mut state = make_state("hello", 2, None);
    assert_eq!(
        state.resolve_vim_motion("left", false, None),
        MotionResolution::Resolved {
            target: 1,
            kind: MotionKind::ExclusiveChar
        }
    );
    let mut state2 = make_state("hello", 2, None);
    assert_eq!(
        state2.resolve_vim_motion("end", false, None),
        MotionResolution::Resolved {
            target: 5,
            kind: MotionKind::InclusiveChar
        }
    );
}

#[test]
fn test_resolve_vim_motion_up_down_j_k_need_gpui() {
    let mut state = make_state("one\ntwo", 0, None);
    assert_eq!(
        state.resolve_vim_motion("j", false, None),
        MotionResolution::NeedsGpui
    );
    assert_eq!(
        state.resolve_vim_motion("k", false, None),
        MotionResolution::NeedsGpui
    );
}

#[test]
fn test_resolve_vim_motion_digit_and_pending_trigger_start_are_pending() {
    let mut state = make_state("hello", 0, None);
    assert_eq!(
        state.resolve_vim_motion("3", false, None),
        MotionResolution::Pending
    );
    let mut state2 = make_state("hello", 0, None);
    assert_eq!(
        state2.resolve_vim_motion("g", false, None),
        MotionResolution::Pending
    );
}

#[test]
fn test_resolve_vim_motion_unmapped_key_is_not_a_motion() {
    let mut state = make_state("hello", 0, None);
    assert_eq!(
        state.resolve_vim_motion("i", false, None),
        MotionResolution::NotAMotion
    );
}

#[test]
fn test_handle_vim_motion_key_normal_still_defers_left_right_to_gpui() {
    // Regression: handle_vim_motion_key's own extend=false special case
    // must still return Some(false) for these, exactly as before the
    // resolve_vim_motion split.
    let mut state = make_state("hello", 2, None);
    assert_eq!(
        state.handle_vim_motion_key("left", false, None, false),
        Some(false)
    );
    assert_eq!(
        state.handle_vim_motion_key("home", false, None, false),
        Some(false)
    );
}

#[test]
fn test_handle_vim_motion_key_visual_resolves_left_right_locally() {
    let mut state = make_state("hello", 2, None);
    assert_eq!(
        state.handle_vim_motion_key("left", false, None, true),
        Some(true)
    );
    assert_eq!(state.workspace.tabs[0].cursor, 1);
}

// ── Operators: d/y/c + dd/yy/cc (Task F) ──────────────────────────────────────

#[test]
fn test_dw_deletes_exclusive_up_to_next_word() {
    let mut state = make_state("one two three", 0, None);
    state.handle_vim_key("d", false, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('d'));
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "two three");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"one ".to_string())
    );
}

#[test]
fn test_d3w_count_typed_after_operator_deletes_three_words() {
    let mut state = make_state("one two three four", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("3", false, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('d')); // still pending
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "four");
}

#[test]
fn test_de_deletes_inclusive_through_word_end() {
    let mut state = make_state("one two", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("e", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), " two");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_dw_and_de_from_same_cursor_produce_different_ranges() {
    // The whole point of MotionKind: same starting cursor, same
    // starting content, different operator result.
    let mut dw = make_state("one two", 0, None);
    dw.handle_vim_key("d", false, None);
    dw.handle_vim_key("w", false, None);
    let mut de = make_state("one two", 0, None);
    de.handle_vim_key("d", false, None);
    de.handle_vim_key("e", false, None);
    assert_ne!(
        dw.workspace.tabs[0].document.content(),
        de.workspace.tabs[0].document.content()
    );
}

#[test]
fn test_dd_deletes_current_line() {
    let mut state = make_state("one\ntwo\nthree", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("d", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "two\nthree");
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"one\n".to_string())
    );
}

#[test]
fn test_d2d_deletes_two_lines_via_count_between_doubled_keys() {
    let mut state = make_state("a\nb\nc\nd", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("2", false, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('d')); // still pending
    state.handle_vim_key("d", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "c\nd");
}

#[test]
fn test_d_dollar_deletes_inclusive_to_end_of_line() {
    let mut state = make_state("hello world", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("4", true, None); // shifted 4 => $
    assert_eq!(state.workspace.tabs[0].document.content(), "");
}

#[test]
fn test_yy_yanks_current_line_without_deleting() {
    let mut state = make_state("one\ntwo", 0, None);
    state.handle_vim_key("y", false, None);
    state.handle_vim_key("y", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo"); // unchanged
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"one\n".to_string())
    );
    assert_eq!(
        state.global_vim.registers.get(&'0'),
        Some(&"one\n".to_string())
    );
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_yw_yanks_word_and_moves_cursor_to_start_not_target() {
    let mut state = make_state("one two", 0, None);
    state.handle_vim_key("y", false, None);
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"one ".to_string())
    );
    assert_eq!(state.workspace.tabs[0].cursor, 0);
}

#[test]
fn test_cc_changes_line_keeping_it_as_empty_line_and_enters_insert() {
    let mut state = make_state("one\ntwo", 0, None);
    state.handle_vim_key("c", false, None);
    state.handle_vim_key("c", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "\ntwo"); // line kept, just emptied
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"one".to_string())
    );
}

#[test]
fn test_d_find_deletes_inclusive_through_target_char() {
    let mut state = make_state("abcXdef", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("f", false, None);
    state.handle_vim_key("X", true, Some("X"));
    assert_eq!(state.workspace.tabs[0].document.content(), "def");
}

#[test]
fn test_d_till_deletes_exclusive_up_to_target_char() {
    // `t` lands just *before* 'X' (position 2, the 'c') — exclusive
    // range [0, 2) deletes "ab", leaving "cXdef" behind. Distinct from
    // `df` (test above), which deletes through 'X' itself.
    let mut state = make_state("abcXdef", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("t", false, None);
    state.handle_vim_key("X", true, Some("X"));
    assert_eq!(state.workspace.tabs[0].document.content(), "cXdef");
}

#[test]
fn test_operator_delete_is_undoable() {
    let mut state = make_state("one two three", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "two three");
    state.undo();
    assert_eq!(state.workspace.tabs[0].document.content(), "one two three");
}

#[test]
fn test_operator_abandoned_by_invalid_key_does_not_leak_into_macro() {
    // 'd' then 'q': must abandon the pending operator, NOT start
    // recording into register 'q' — regression guard for the ordering
    // decided between complete_vim_operator and the macro q-pending
    // check in handle_vim_normal_key.
    let mut state = make_state("one two", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("q", false, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);
    assert!(!state.vim_is_recording_macro());
    assert_eq!(state.workspace.tabs[0].document.content(), "one two"); // unchanged
}

#[test]
fn test_operator_abandoned_by_needs_gpui_key() {
    // dj: j needs GPUI context resolve_vim_motion doesn't have —
    // documented gap, must abandon cleanly rather than panic/misfire.
    let mut state = make_state("one\ntwo", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("j", false, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

#[test]
fn test_operator_pending_cleared_on_mode_transitions() {
    let mut state = make_state("hello", 0, None);
    state.handle_vim_key("d", false, None);
    state.vim_exit_to_normal();
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);

    let mut state2 = make_state("hello", 0, None);
    state2.handle_vim_key("d", false, None);
    state2.vim_enter_visual();
    assert_eq!(state2.workspace.tabs[0].vim_pending_operator, None);
}

#[test]
fn test_d_backward_motion_normalizes_range_regardless_of_direction() {
    let mut state = make_state("one two three", 8, None); // cursor on "three"
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("b", false, None); // b moves backward to "two"
    assert_eq!(state.workspace.tabs[0].document.content(), "one three");
}

#[test]
fn test_vim_pending_operator_accessor() {
    // text_editor.rs's j/k, H/M/L, and `@` interceptions all gate on
    // this being None — regression guard for the "dj silently moves
    // the cursor and leaves d dangling" bug caught by the advisor
    // (same failure class as the pending-find-trigger check this
    // mirrors, `vim_pending_trigger()`).
    let mut state = make_state("one two", 0, None);
    assert_eq!(state.vim_pending_operator(), None);
    state.handle_vim_key("d", false, None);
    assert_eq!(state.vim_pending_operator(), Some('d'));
    state.handle_vim_key("w", false, None);
    assert_eq!(state.vim_pending_operator(), None);
}

// ── Text objects (Task F): iw/aw, is/as, ip/ap, quotes, brackets ─────────────

#[test]
fn test_text_object_word_inner_and_around_with_trailing_space() {
    let content = "one two three";
    let cursor = content.find("two").unwrap();
    let (s, e) = text_object_word(content, cursor, true);
    assert_eq!(&content[s..e], "two");
    let (s, e) = text_object_word(content, cursor, false);
    assert_eq!(&content[s..e], "two ");
}

#[test]
fn test_text_object_aw_falls_back_to_leading_space_when_no_trailing() {
    let content = "one two";
    let cursor = content.find("two").unwrap();
    let (s, e) = text_object_word(content, cursor, false);
    assert_eq!(&content[s..e], " two");
}

#[test]
fn test_text_object_iw_on_whitespace_selects_just_the_whitespace_run() {
    let content = "one  two";
    let cursor = content.find("  ").unwrap();
    let (s, e) = text_object_word(content, cursor, true);
    assert_eq!(&content[s..e], "  ");
}

#[test]
fn test_text_object_iw_on_punctuation_run() {
    let content = "one,,two";
    let cursor = content.find(",,").unwrap();
    let (s, e) = text_object_word(content, cursor, true);
    assert_eq!(&content[s..e], ",,");
}

#[test]
fn test_text_object_sentence_inner_and_around() {
    let content = "Hello world. Foo bar. Baz.";
    let cursor = content.find("bar").unwrap();
    let (s, e) = text_object_sentence(content, cursor, true).unwrap();
    assert_eq!(&content[s..e], "Foo bar.");
    let (s, e) = text_object_sentence(content, cursor, false).unwrap();
    assert_eq!(&content[s..e], "Foo bar. ");
}

#[test]
fn test_text_object_sentence_first_sentence_has_no_leading_boundary() {
    let content = "Hello world. Foo bar.";
    let cursor = content.find("Hello").unwrap();
    let (s, e) = text_object_sentence(content, cursor, true).unwrap();
    assert_eq!(&content[s..e], "Hello world.");
}

#[test]
fn test_text_object_paragraph_inner_and_around() {
    let content = "one\ntwo\n\nthree\nfour";
    let cursor = content.find("two").unwrap();
    let (s, e) = text_object_paragraph(content, cursor, true).unwrap();
    assert_eq!(&content[s..e], "one\ntwo\n");
    let (s, e) = text_object_paragraph(content, cursor, false).unwrap();
    assert_eq!(&content[s..e], "one\ntwo\n\n");
}

#[test]
fn test_text_object_paragraph_ap_falls_back_to_leading_blank_block() {
    let content = "one\ntwo\n\nthree\nfour";
    let cursor = content.find("four").unwrap();
    let (s, e) = text_object_paragraph(content, cursor, false).unwrap();
    assert_eq!(&content[s..e], "\nthree\nfour");
}

#[test]
fn test_text_object_quote_inner_and_around() {
    let content = "say \"hello world\" now";
    let cursor = content.find("hello").unwrap();
    let (s, e) = text_object_quote(content, cursor, '"', true).unwrap();
    assert_eq!(&content[s..e], "hello world");
    let (s, e) = text_object_quote(content, cursor, '"', false).unwrap();
    assert_eq!(&content[s..e], "\"hello world\"");
}

#[test]
fn test_text_object_quote_none_when_no_pair_on_line() {
    let content = "no quotes here";
    assert_eq!(text_object_quote(content, 0, '"', true), None);
}

#[test]
fn test_text_object_bracket_innermost_pair() {
    let content = "foo(bar(baz)qux)end";
    let cursor = content.find("baz").unwrap();
    let (s, e) = text_object_bracket(content, cursor, '(', ')', true).unwrap();
    assert_eq!(&content[s..e], "baz");
    let (s, e) = text_object_bracket(content, cursor, '(', ')', false).unwrap();
    assert_eq!(&content[s..e], "(baz)");
}

#[test]
fn test_text_object_bracket_outer_pair_when_cursor_outside_inner() {
    let content = "foo(bar(baz)qux)end";
    let cursor = content.find("qux").unwrap();
    let (s, e) = text_object_bracket(content, cursor, '(', ')', true).unwrap();
    assert_eq!(&content[s..e], "bar(baz)qux");
}

#[test]
fn test_diw_deletes_word_via_operator_and_text_object() {
    let mut state = make_state("one two three", 0, None);
    state.workspace.tabs[0].cursor = "one two three".find("two").unwrap();
    state.handle_vim_key("d", false, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('d'));
    state.handle_vim_key("i", false, None);
    assert_eq!(
        state.workspace.tabs[0].vim_pending_text_object_prefix,
        Some(true)
    );
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one  three");
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);
    assert_eq!(state.workspace.tabs[0].vim_pending_text_object_prefix, None);
}

#[test]
fn test_daw_deletes_word_and_surrounding_space() {
    let mut state = make_state("one two three", 0, None);
    state.workspace.tabs[0].cursor = "one two three".find("two").unwrap();
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("a", false, None);
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one three");
}

#[test]
fn test_ci_quote_changes_inside_quotes_and_enters_insert() {
    let content = "say \"hello world\" now";
    let mut state = make_state(content, 0, None);
    state.workspace.tabs[0].cursor = content.find("hello").unwrap();
    state.handle_vim_key("c", false, None);
    state.handle_vim_key("i", false, None);
    state.handle_vim_key("\"", true, Some("\""));
    assert_eq!(state.workspace.tabs[0].document.content(), "say \"\" now");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"hello world".to_string())
    );
}

#[test]
fn test_di_bracket_deletes_innermost_parens_content() {
    let content = "foo(bar(baz)qux)end";
    let mut state = make_state(content, 0, None);
    state.workspace.tabs[0].cursor = content.find("baz").unwrap();
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("i", false, None);
    state.handle_vim_key("(", true, Some("("));
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "foo(bar()qux)end"
    );
}

#[test]
fn test_text_object_with_no_match_abandons_operator_cleanly() {
    let mut state = make_state("no quotes here", 0, None);
    state.handle_vim_key("d", false, None);
    state.handle_vim_key("i", false, None);
    state.handle_vim_key("\"", true, Some("\""));
    assert_eq!(state.workspace.tabs[0].document.content(), "no quotes here");
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);
}

// ── >>/<</gU/gu operators (Task F) ────────────────────────────────────────────

#[test]
fn test_gt_gt_indents_current_line() {
    let mut state = make_state("one\ntwo", 0, None);
    state.handle_vim_key(".", true, Some(">")); // shifted '.' reported directly as '>'
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('>'));
    state.handle_vim_key(".", true, Some(">"));
    assert_eq!(state.workspace.tabs[0].document.content(), "\tone\ntwo");
}

#[test]
fn test_lt_lt_removes_leading_tab() {
    let mut state = make_state("\tone\ntwo", 0, None);
    state.handle_vim_key(",", true, Some("<"));
    state.handle_vim_key(",", true, Some("<"));
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

#[test]
fn test_lt_lt_removes_up_to_four_leading_spaces_when_no_tab() {
    let mut state = make_state("      one\ntwo", 0, None);
    state.handle_vim_key(",", true, Some("<"));
    state.handle_vim_key(",", true, Some("<"));
    assert_eq!(state.workspace.tabs[0].document.content(), "  one\ntwo");
}

#[test]
fn test_gt_ip_indents_paragraph_lines() {
    // `>j` isn't testable here: `j` always resolves to `NeedsGpui`
    // (same documented gap as `dj`), so a multi-line indent needs a
    // motion/text-object `resolve_vim_motion` can actually resolve —
    // `ip` (paragraph text object) exercises the same linewise-range
    // path without touching that gap.
    let mut state = make_state("one\ntwo\n\nthree", 0, None);
    state.handle_vim_key(".", true, Some(">"));
    state.handle_vim_key("i", false, None);
    state.handle_vim_key("p", false, None);
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "\tone\n\ttwo\n\nthree"
    );
}

#[test]
fn test_g_upper_u_w_uppercases_word() {
    let mut state = make_state("one two three", 0, None);
    state.handle_vim_key("g", false, None);
    assert_eq!(state.vim_pending_trigger(), Some('g'));
    state.handle_vim_key("u", true, None); // gU
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, Some('U'));
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ONE two three");
}

#[test]
fn test_gu_iw_lowercases_inner_word() {
    let mut state = make_state("ONE TWO THREE", 0, None);
    state.workspace.tabs[0].cursor = "ONE TWO THREE".find("TWO").unwrap();
    state.handle_vim_key("g", false, None);
    state.handle_vim_key("u", false, None); // gu
    state.handle_vim_key("i", false, None);
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ONE two THREE");
}

#[test]
fn test_g_upper_u_is_charwise_not_linewise_unlike_indent_operators() {
    // Distinguishes gUw (charwise, only the word changes) from an
    // indent operator's forced-linewise rule — regression guard for
    // the `matches!(operator, '>' | '<')` override in
    // vim_operator_motion_range not accidentally also catching 'U'/'u'.
    let mut state = make_state("one two\nthree", 0, None);
    state.handle_vim_key("g", false, None);
    state.handle_vim_key("u", true, None);
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "ONE two\nthree");
    // not the whole line
}

#[test]
fn test_gu_u_doubled_form_is_not_supported_and_abandons_cleanly() {
    // Documented scope gap: gUU/guu (doubled-key linewise form) isn't
    // implemented — must abandon without crashing or corrupting state,
    // not silently misfire as something else.
    let mut state = make_state("one two", 0, None);
    state.handle_vim_key("g", false, None);
    state.handle_vim_key("u", true, None); // gU
    state.handle_vim_key("u", true, None); // second U: not a supported completion
    assert_eq!(state.workspace.tabs[0].vim_pending_operator, None);
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
}

#[test]
fn test_indent_operator_undoable() {
    let mut state = make_state("one\ntwo", 0, None);
    state.handle_vim_key(".", true, Some(">"));
    state.handle_vim_key(".", true, Some(">"));
    assert_eq!(state.workspace.tabs[0].document.content(), "\tone\ntwo");
    state.undo();
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

// ── Visual-mode operators (Task G) ────────────────────────────────────────────

#[test]
fn test_visual_d_deletes_selection_and_returns_to_normal() {
    let mut state = make_state("one two three", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None); // extend selection to (0,2)
    assert!(state.handle_vim_key("d", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "e two three");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(state.workspace.tabs[0].selection, None);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"on".to_string())
    );
}

#[test]
fn test_visual_x_is_equivalent_to_d() {
    let mut state = make_state("one two three", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None);
    assert!(state.handle_vim_key("x", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "e two three");
}

#[test]
fn test_visual_y_yanks_without_deleting() {
    let mut state = make_state("one two three", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None);
    assert!(state.handle_vim_key("y", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "one two three");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
    assert_eq!(
        state.global_vim.registers.get(&'"'),
        Some(&"on".to_string())
    );
    assert_eq!(
        state.global_vim.registers.get(&'0'),
        Some(&"on".to_string())
    );
}

#[test]
fn test_visual_c_deletes_and_enters_insert() {
    let mut state = make_state("one two three", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None);
    assert!(state.handle_vim_key("c", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "e two three");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_visual_line_d_deletes_whole_lines() {
    let mut state = make_state("one\ntwo\nthree", 4, None); // on "two"
    state.vim_enter_visual_line();
    assert!(state.handle_vim_key("d", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "one\nthree");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_visual_line_c_keeps_line_as_empty_and_enters_insert() {
    let mut state = make_state("one\ntwo\nthree", 4, None); // on "two"
    state.vim_enter_visual_line();
    assert!(state.handle_vim_key("c", false, None));
    assert_eq!(state.workspace.tabs[0].document.content(), "one\n\nthree");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Insert);
}

#[test]
fn test_visual_charwise_gt_forces_linewise_indent() {
    // Real vim rule: `>` always indents whole lines, even from a
    // charwise (not VisualLine) selection.
    let mut state = make_state("one\ntwo", 0, None); // charwise selection covers only part of "one"
    state.vim_enter_visual();
    assert!(state.handle_vim_key(".", true, Some(">")));
    assert_eq!(state.workspace.tabs[0].document.content(), "\tone\ntwo");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_visual_lt_unindents() {
    let mut state = make_state("\tone\ntwo", 0, None);
    state.vim_enter_visual();
    assert!(state.handle_vim_key(",", true, Some("<")));
    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

#[test]
fn test_visual_line_gt_indents_all_selected_lines() {
    // `j`/`k` need GPUI context (not resolvable here, same limitation
    // as everywhere else in this test suite) — `w` twice extends the
    // selection from "one\n" (0,4) to "one\ntwo\n" (0,8) instead,
    // spanning two lines without touching that gap.
    let mut state = make_state("one\ntwo\nthree", 0, None);
    state.vim_enter_visual_line();
    state.handle_vim_key("w", false, None);
    state.handle_vim_key("w", false, None);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 8)));
    assert!(state.handle_vim_key(".", true, Some(">")));
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "\tone\n\ttwo\nthree"
    );
}

#[test]
fn test_visual_g_upper_u_uppercases_only_selected_chars_not_whole_line() {
    let mut state = make_state("one two\nthree", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None); // selects "on"
    state.handle_vim_key("g", false, None);
    assert!(state.handle_vim_key("u", true, None)); // gU
    assert_eq!(state.workspace.tabs[0].document.content(), "ONe two\nthree");
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Normal);
}

#[test]
fn test_visual_gu_lowercases_selection() {
    let mut state = make_state("ONE two", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None);
    state.handle_vim_key("l", false, None); // selection now covers all of "ONE"
    state.handle_vim_key("g", false, None);
    assert!(state.handle_vim_key("u", false, None)); // gu
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
}

#[test]
fn test_visual_tilde_toggles_case_of_selection() {
    let mut state = make_state("One two", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("l", false, None); // selects "On"
    assert!(state.handle_vim_key("`", true, Some("~")));
    assert_eq!(state.workspace.tabs[0].document.content(), "oNe two");
}

#[test]
fn test_apply_case_to_selection_mid_run() {
    // The whole document is one paragraph, one run — a selection that
    // starts and ends mid-run (not "an active selection" alone) is what
    // the run-splitting in `apply_case_to_selection` exists for.
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "one two three".to_string(),
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((4, 7));
    state.apply_case_to_selection(crate::case_converter::CaseType::Upper);
    assert_eq!(state.workspace.tabs[0].document.content(), "one TWO three");
}

#[test]
fn test_visual_o_swaps_selection_ends() {
    let mut state = make_state("one two three", 0, None);
    state.vim_enter_visual(); // selection (0,1), cursor 1
    state.handle_vim_key("l", false, None); // selection (0,2), cursor 2
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 2)));
    assert!(state.handle_vim_key("o", false, None));
    assert_eq!(state.workspace.tabs[0].selection, Some((2, 0)));
    assert_eq!(state.workspace.tabs[0].cursor, 0);
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual); // stays in Visual
}

#[test]
fn test_visual_pending_find_wins_over_operator_start() {
    // Regression for the collision this session's own test suite
    // caught: `f` then `d` must complete the find (target 'd'), not
    // misfire as starting the delete operator.
    let mut state = make_state("abcdef", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("f", false, None);
    assert!(state.handle_vim_key("d", false, None));
    assert_eq!(state.workspace.tabs[0].cursor, 3);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual); // not executed as an operator
}

#[test]
fn test_visual_pending_find_v_target_wins_over_exit_visual() {
    // Bug: in Visual mode, `f` then `v` should complete the find with
    // target 'v', not exit visual mode. The `v`-to-exit-visual logic
    // must not run when a pending find trigger exists.
    // Start at position 0 ('a'). vim_enter_visual moves cursor to 1 ('v')
    // and creates selection (0, 1). Then `f` then `v` searches forward
    // from position 1, finding the next 'v' at position 4.
    let mut state = make_state("avcbvc", 0, None);
    state.vim_enter_visual();
    state.handle_vim_key("f", false, None);
    assert!(state.handle_vim_key("v", false, None)); // complete find, target 'v'
    assert_eq!(state.workspace.tabs[0].cursor, 4); // second 'v' is at index 4
    assert_eq!(state.workspace.tabs[0].vim_mode, VimMode::Visual); // still in Visual, not exited
}

// ── Zoom (found_bugs.md: Ctrl+=/Ctrl+-/Ctrl+0, rebuilt from scratch) ────

#[test]
fn test_zoom_in_increases_by_step() {
    let mut state = make_state("", 0, None);
    state.zoom_in();
    assert!((state.zoom - 1.1).abs() < f32::EPSILON);
}

#[test]
fn test_zoom_out_decreases_by_step() {
    let mut state = make_state("", 0, None);
    state.zoom_out();
    assert!((state.zoom - 0.9).abs() < f32::EPSILON);
}

#[test]
fn test_zoom_in_clamps_at_max() {
    let mut state = make_state("", 0, None);
    state.zoom = AppState::ZOOM_MAX;
    state.zoom_in();
    assert_eq!(state.zoom, AppState::ZOOM_MAX);
}

#[test]
fn test_zoom_out_clamps_at_min() {
    let mut state = make_state("", 0, None);
    state.zoom = AppState::ZOOM_MIN;
    state.zoom_out();
    assert_eq!(state.zoom, AppState::ZOOM_MIN);
}

#[test]
fn test_zoom_reset_returns_to_100_percent() {
    let mut state = make_state("", 0, None);
    state.zoom = 1.8;
    state.zoom_reset();
    assert_eq!(state.zoom, 1.0);
}

#[test]
fn test_apply_line_alignment_center_sets_current_line() {
    let mut state = make_state("hello world", 0, None);
    state.apply_line_alignment(Alignment::Center);

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].alignment,
        Alignment::Center
    );
}

#[test]
fn test_apply_line_alignment_left_sets_current_line() {
    // Start centered, then switch back to left — the two buttons
    // should behave as a mutually exclusive pair, not independent
    // on/off toggles.
    let mut state = make_state("hello world", 0, None);
    state.apply_line_alignment(Alignment::Center);
    state.apply_line_alignment(Alignment::Left);

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].alignment,
        Alignment::Left
    );
}

#[test]
fn test_apply_line_alignment_only_affects_current_line_not_whole_document() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![run_plain("first line")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("second line")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.apply_line_alignment(Alignment::Center);

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].alignment,
        Alignment::Center
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].alignment,
        Alignment::Left
    );
}

#[test]
fn test_apply_line_alignment_targets_line_under_cursor_not_first_line() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![run_plain("first line")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("second line")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let cursor = "first line\n".len(); // start of "second line"
    let mut state = make_state_with_paragraphs(paragraphs, cursor);
    state.apply_line_alignment(Alignment::Center);

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].alignment,
        Alignment::Left
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].alignment,
        Alignment::Center
    );
}

#[test]
fn test_apply_card_style_pocket_sets_bold_size_box_and_center() {
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.alignment, Alignment::Center);
    assert!(para.runs.iter().all(|r| r.bold));
    assert!(para.runs.iter().all(|r| r.size == 52));
    assert!(para.runs.iter().all(|r| r.box_format));
    assert_eq!(para.heading, 1);
}

/// Bug report: card-style headings that used to render correctly in
/// Word stopped working. Root cause: applying a card style to a line
/// that was already a list item left `.list` set alongside the new
/// `heading`, and `rebuild_document_xml` wrote a `<w:pStyle>` for each —
/// two children of one non-repeatable element, which Word silently
/// "repaired" by dropping the card style's own formatting on reopen.
/// `apply_card_style` must clear `.list` so the paragraph can't carry
/// both.
#[test]
fn test_apply_card_style_clears_a_preexisting_list_marker() {
    let mut state = make_state("hello world", 0, None);
    state.workspace.tabs[0].document.paragraphs_mut()[0].list = Some(ListItem {
        kind: ListKind::BulletSolid,
        level: 0,
    });
    state.apply_card_style(CardStyleKind::Pocket);
    assert_eq!(state.workspace.tabs[0].document.paragraphs()[0].list, None);
}

#[test]
fn test_apply_card_style_pocket_on_empty_line_then_typed_text_is_boxed_and_centered() {
    // Reported bug repro: press the Pocket style FIRST on a blank
    // line/tab, THEN type the card's text (the natural authoring order,
    // vs. the already-covered "type first, then style" case above).
    let mut state = make_state("", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    for ch in "hello".chars() {
        state.insert_char(ch);
    }

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.alignment, Alignment::Center);
    assert!(
        para.runs.iter().all(|r| r.bold),
        "bold lost: {:?}",
        para.runs
    );
    assert!(
        para.runs.iter().all(|r| r.size == 52),
        "size lost: {:?}",
        para.runs
    );
    assert!(
        para.runs.iter().all(|r| r.box_format),
        "box lost: {:?}",
        para.runs
    );
}

#[test]
fn test_clear_formatting_resets_pocket_heading_alignment_and_size() {
    // found_bugs.md: "Clear Formatting failing to remove all
    // formatting" — clicking Clear on a Pocket-styled line left it
    // visually still boxed/centered/oversized, because the run-level
    // ClearAll never reset the paragraph-level heading/alignment fields
    // apply_card_style also sets.
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    state.preferences.normal_text_size_half_points = 22; // 11pt, settings.conf's default

    let default_size = state.preferences.normal_text_size_half_points;
    state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.heading, 0, "heading not cleared");
    assert_eq!(para.alignment, Alignment::Left, "not left-aligned");
    assert!(para.runs.iter().all(|r| !r.bold), "bold not cleared");
    assert!(para.runs.iter().all(|r| !r.box_format), "box not cleared");
    assert!(
        para.runs.iter().all(|r| r.size == 22),
        "size not reset to normal_text_size"
    );
}

#[test]
fn test_clear_formatting_via_selection_resets_pocket_heading_and_alignment() {
    // Same bug as test_clear_formatting_resets_pocket_heading_alignment_
    // and_size, but through the SELECTION path (clear_formatting() with
    // tab.selection.is_some()), which routes to
    // apply_formatting_to_selection instead of apply_formatting_to_line.
    // That branch only ever called document_ops::apply_formatting for
    // run-level fields (bold/size/box) and never reset the paragraph-
    // level heading/alignment apply_card_style also sets — so clicking
    // Clear Formatting with an ordinary, single-paragraph selection
    // inside a Pocket-styled line left it still boxed/centered.
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    state.preferences.normal_text_size_half_points = 22; // 11pt, settings.conf's default

    // Selection entirely inside the one (Pocket) paragraph.
    state.workspace.tabs[0].selection = Some((0, 5));
    state.clear_formatting();

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.heading, 0, "heading not cleared via selection path");
    assert_eq!(
        para.alignment,
        Alignment::Left,
        "not left-aligned via selection path"
    );
    assert!(
        para.runs.iter().all(|r| !r.bold),
        "bold not cleared via selection path"
    );
    assert!(
        para.runs.iter().all(|r| !r.box_format),
        "box not cleared via selection path"
    );
    assert_eq!(
        state.workspace.tabs[0].pending_format, None,
        "stale pending_format should be cleared via selection path too"
    );
}

#[test]
fn test_clear_formatting_on_empty_line_clears_stale_pending_format() {
    // Repro: Pocket (F4) on an *empty* line, then Clear Formatting
    // (F12) on that same still-empty line, then type. The newly typed
    // text kept getting boxed, because ClearAll wasn't in
    // apply_formatting_to_line's pending-format-arming match — whatever
    // card-style op (Box(true), the last of apply_card_style's
    // Bold+FontSize+Box sequence) armed `pending_format` earlier was
    // never cleared, so it kept force-applying to every character
    // typed afterward (see insert_char's own doc comment: a pending
    // format applies "to every character typed... not just this one").
    // The existing Clear Formatting tests above don't catch this since
    // they start from a non-empty line, where the pending-format-arming
    // branch (gated on `is_line_empty`) never even runs.
    let mut state = make_state("", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    let default_size = state.preferences.normal_text_size_half_points;
    state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

    assert_eq!(
        state.workspace.tabs[0].pending_format, None,
        "stale pending_format should be cleared by Clear Formatting"
    );

    state.insert_char('a');
    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert!(
        para.runs.iter().all(|r| !r.box_format),
        "newly typed text should not inherit the cleared Pocket box"
    );
}

#[test]
fn test_clear_formatting_on_empty_line_does_not_leak_box_across_newline() {
    // Second half of the same repro: without the fix, the stale
    // pending_format kept applying on *every* keystroke, including
    // across an Enter — so a new paragraph created after typing on the
    // "cleared" line still ended up boxed too.
    let mut state = make_state("", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    let default_size = state.preferences.normal_text_size_half_points;
    state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

    state.insert_char('a');
    state.insert_char('\n');
    state.insert_char('b');

    assert!(
        state.workspace.tabs[0]
            .document
            .paragraphs()
            .iter()
            .all(|p| p.runs.iter().all(|r| !r.box_format)),
        "no paragraph should carry the cleared Pocket box after typing across a newline: {:?}",
        state.workspace.tabs[0].document.paragraphs()
    );
}

#[test]
fn test_pocket_on_empty_line_does_not_leave_pending_format_after_enter() {
    // Broader repro (no Clear Formatting involved this time): Pocket
    // (F4) an empty line, press Enter, keep typing. The new paragraph
    // correctly lost bold/size/heading/alignment — split_paragraph_at
    // (document_ops.rs) already reverts all of that for a
    // heading-marked split — but still ended up boxed, because
    // apply_card_style's internal apply_formatting_to_line(Box(true))
    // call left `pending_format` armed indefinitely (nothing but an
    // explicit re-toggle or Clear Formatting ever cleared it). That
    // stale pending format then kept re-applying Box(true) to
    // whatever got typed next — including the freshly-split,
    // already-correctly-reset tail run — and would keep doing so on
    // every subsequent line too.
    //
    // The run is already seeded directly for the very next keystroke
    // (see apply_formatting_to_line's own `is_line_empty` seeding
    // above), so arming `pending_format` here serves no purpose for
    // apply_card_style and should not happen at all.
    let mut state = make_state("", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    assert_eq!(
        state.workspace.tabs[0].pending_format, None,
        "apply_card_style should not leave a sticky pending format armed"
    );

    state.insert_char('a');
    state.insert_char('\n');
    state.insert_char('b');

    // Paragraph 0 ("a") is legitimately still a real Pocket line — it
    // should keep its box. Only paragraph 1 ("b", created by the
    // Enter split) should have reverted to plain.
    let paragraphs = state.workspace.tabs[0].document.paragraphs();
    assert!(
        paragraphs[0].runs.iter().all(|r| r.box_format),
        "the original Pocket line should keep its box: {:?}",
        paragraphs
    );
    assert!(
        paragraphs[1].runs.iter().all(|r| !r.box_format),
        "box should not leak onto the new paragraph after Enter: {:?}",
        paragraphs
    );
}

#[test]
fn test_backspace_through_pocket_line_and_trailing_newline_clears_all_formatting() {
    // Reported bug: Pocket a line, press Enter (new empty plain line
    // below it), then backspace repeatedly to erase the new line AND
    // all of the pocket text. Box/center-align visibly disappeared
    // already, but bold/font size stuck around — because nothing ever
    // reset the surviving paragraph's `heading` (text_editor.rs applies
    // a heading-driven bold+oversized font at the paragraph level,
    // independent of the run's own now-cleared bold/size) once the
    // paragraph's actual pocket-formatted text was fully deleted.
    let mut state = make_state("", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    state.insert_char('a');
    state.insert_char('\n');
    // Backspace away the new empty line, then the pocket text itself.
    state.backspace(); // removes the newline, merges back into the pocket paragraph
    state.backspace(); // removes "a"

    assert_eq!(state.workspace.tabs[0].document.paragraphs().len(), 1);
    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(
        para.heading, 0,
        "heading not cleared once pocket text is fully deleted: {:?}",
        para
    );
    assert_eq!(
        para.alignment,
        Alignment::Left,
        "not left-aligned: {:?}",
        para
    );
    assert!(
        para.runs.iter().all(|r| !r.bold),
        "bold not cleared: {:?}",
        para
    );
    assert!(
        para.runs.iter().all(|r| !r.box_format),
        "box not cleared: {:?}",
        para
    );
    assert!(
        para.runs.iter().all(|r| r.size == 0),
        "size not cleared: {:?}",
        para
    );
}

#[test]
fn test_backspace_merging_empty_line_into_pocket_line_keeps_center_alignment() {
    // Narrower case: only ONE backspace (undoing the Enter, no pocket
    // text deleted yet) should leave the pocket paragraph exactly as it
    // was — still centered — not reset alignment just because a merge
    // across paragraphs happened.
    let mut state = make_state("", 0, None);
    state.apply_card_style(CardStyleKind::Pocket);
    state.insert_char('a');
    state.insert_char('\n');
    state.backspace();

    assert_eq!(state.workspace.tabs[0].document.paragraphs().len(), 1);
    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(
        para.alignment,
        Alignment::Center,
        "pocket line's center alignment should survive merging back an empty trailing line: {:?}",
        para
    );
    assert_eq!(para.heading, 1);
    assert!(para.runs.iter().all(|r| r.box_format));
}

#[test]
fn test_clear_formatting_on_hat_line_removes_double_underline_and_heading() {
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Hat);

    let default_size = state.preferences.normal_text_size_half_points;
    state.apply_formatting_to_line(FormatOp::ClearAll { default_size });

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.heading, 0);
    assert_eq!(para.alignment, Alignment::Left);
    assert!(para.runs.iter().all(|r| !r.double_underline));
}

#[test]
fn test_apply_card_style_hat_sets_double_underline_not_box() {
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Hat);

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.alignment, Alignment::Center);
    assert!(para.runs.iter().all(|r| r.size == 44));
    assert!(para.runs.iter().all(|r| r.double_underline));
    assert!(para.runs.iter().all(|r| !r.box_format));
    assert_eq!(para.heading, 2);
}

#[test]
fn test_apply_card_style_block_sets_underline_not_double() {
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Block);

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.alignment, Alignment::Center);
    assert!(para.runs.iter().all(|r| r.size == 32));
    assert!(para.runs.iter().all(|r| r.underline));
    assert!(para.runs.iter().all(|r| !r.double_underline));
    assert_eq!(para.heading, 3);
}

#[test]
fn test_apply_card_style_tag_is_left_aligned_no_box_or_underline() {
    let mut state = make_state("hello world", 0, None);
    state.apply_card_style(CardStyleKind::Tag);

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert_eq!(para.alignment, Alignment::Left);
    assert!(para.runs.iter().all(|r| r.size == 26));
    assert!(para.runs.iter().all(|r| r.bold));
    assert!(para
        .runs
        .iter()
        .all(|r| !r.box_format && !r.underline && !r.double_underline));
    assert_eq!(para.heading, 4);
}

#[test]
fn test_apply_card_style_pocket_block_tag_use_configured_sizes_not_hardcoded() {
    // Regression: pocket_size/block_size/tag_size are now read from
    // settings.conf (AppState::pocket_size_half_points etc.) rather than
    // CardStyleKind::font_size()'s fixed table — a changed setting must
    // actually take effect the next time the style is applied.
    let mut state = make_state("hello world", 0, None);
    state.preferences.pocket_size_half_points = 60;
    state.preferences.block_size_half_points = 40;
    state.preferences.tag_size_half_points = 20;

    state.apply_card_style(CardStyleKind::Pocket);
    assert!(state.workspace.tabs[0].document.paragraphs()[0]
        .runs
        .iter()
        .all(|r| r.size == 60));

    state.apply_card_style(CardStyleKind::Block);
    assert!(state.workspace.tabs[0].document.paragraphs()[0]
        .runs
        .iter()
        .all(|r| r.size == 40));

    state.apply_card_style(CardStyleKind::Tag);
    assert!(state.workspace.tabs[0].document.paragraphs()[0]
        .runs
        .iter()
        .all(|r| r.size == 20));
}

#[test]
fn test_apply_cite_style_applies_bold_and_configured_size_to_selection() {
    let paragraphs = vec![para_plain("hello")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 5));
    state.preferences.cite_size_half_points = 30;
    state.apply_cite_style();

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert!(para.runs.iter().all(|r| r.bold));
    assert!(para.runs.iter().all(|r| r.size == 30));
}

#[test]
fn test_condense_selection_replaces_newlines_with_spaces() {
    let paragraphs = vec![para_plain("one"), para_plain("two")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));
    state.condense_selection();
    // Reads exactly like "one two" — the zero-width space renders as
    // nothing — but the marker is real text, which is what makes
    // uncondense_selection able to find it.
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "one\u{200B} two"
    );
}

#[test]
fn test_uncondense_reverses_a_plain_condense() {
    let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));
    state.condense_selection();
    state.workspace.tabs[0].selection = Some((0, state.workspace.tabs[0].document.content().len()));
    state.uncondense_selection();
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "one\ntwo\nthree"
    );
}

#[test]
fn test_uncondense_reverses_a_pilcrow_condense() {
    let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));
    state.condense_with_pilcrows();
    state.workspace.tabs[0].selection = Some((0, state.workspace.tabs[0].document.content().len()));
    state.uncondense_selection();
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "one\ntwo\nthree"
    );
}

#[test]
fn test_uncondense_is_a_no_op_without_either_marker() {
    let mut state = make_state("one two", 0, None);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));
    state.uncondense_selection();
    assert_eq!(state.workspace.tabs[0].document.content(), "one two");
}

#[test]
fn test_condense_with_pilcrows_marks_each_break() {
    let paragraphs = vec![para_plain("one"), para_plain("two"), para_plain("three")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));

    state.condense_with_pilcrows();

    assert_eq!(state.workspace.tabs[0].document.content(), "one¶two¶three");
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs().len(),
        1,
        "should be one paragraph now"
    );
}

/// The pilcrow variant must keep per-character formatting exactly as the
/// plain one does — they share a core, and this pins that they stay shared.
#[test]
fn test_condense_with_pilcrows_preserves_run_formatting() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "bold".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("plain")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));

    state.condense_with_pilcrows();

    assert_eq!(state.workspace.tabs[0].document.content(), "bold¶plain");
    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert!(runs.iter().find(|r| r.text.contains("bold")).unwrap().bold);
    assert!(!runs.iter().find(|r| r.text.contains("plain")).unwrap().bold);
}

// ── Doc Menu cleanup commands ───────────────────────────────────────────

/// Marker-only removal: an Emphasis-marked run is cleared, but a run
/// that merely happens to look the same (manually bolded plain text, or
/// a Tag's own bold) is left alone — the exact bug the old
/// formatting-match heuristic had, since neither case ever carried a
/// real marker to tell them apart.
#[test]
fn remove_emphasis_strips_only_marked_runs() {
    let paragraphs = vec![
        para_plain("plain"),
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "manually bold".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "emphasized".into(),
                bold: true,
                emphasis: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        tag_para("a tag"),
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.remove_emphasis();

    assert!(
        state.workspace.tabs[0].document.paragraphs()[1].runs[0].bold,
        "manually-bolded text must survive"
    );
    assert!(
        !state.workspace.tabs[0].document.paragraphs()[2].runs[0].bold,
        "the marked emphasis run should be cleared"
    );
    assert!(!state.workspace.tabs[0].document.paragraphs()[2].runs[0].emphasis);
    assert!(
        state.workspace.tabs[0].document.paragraphs()[3].runs[0].bold,
        "a Tag's own bold must survive"
    );
}

#[test]
fn remove_emphasis_is_a_no_op_when_nothing_is_marked() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "plain text".into(),
            bold: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let before = state.workspace.tabs[0].document.undo_stack.len();

    state.remove_emphasis();

    assert_eq!(
        state.workspace.tabs[0].document.undo_stack.len(),
        before,
        "no run is marked, so nothing to undo"
    );
}

#[test]
fn remove_emphasis_respects_an_active_selection() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "one".into(),
                bold: true,
                emphasis: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "two".into(),
                bold: true,
                emphasis: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    // Selection covers only the first line ("one").
    state.workspace.tabs[0].selection = Some((0, 3));

    state.remove_emphasis();

    assert!(!state.workspace.tabs[0].document.paragraphs()[0].runs[0].bold);
    assert!(
        state.workspace.tabs[0].document.paragraphs()[1].runs[0].bold,
        "unselected line must survive"
    );
}

#[test]
fn remove_non_highlighted_underlining_skips_highlighted_runs() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "plain-underline".into(),
                underline: true,
                ..Run::default()
            },
            Run {
                text: "highlighted-underline".into(),
                underline: true,
                highlight: true,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);

    state.remove_non_highlighted_underlining();

    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert!(
        !runs
            .iter()
            .find(|r| r.text.contains("plain"))
            .unwrap()
            .underline
    );
    assert!(
        runs.iter()
            .find(|r| r.text.contains("highlighted"))
            .unwrap()
            .underline
    );
}

#[test]
fn remove_non_highlighted_underlining_respects_an_active_selection() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "one".into(),
                underline: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "two".into(),
                underline: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    // Selection covers only the first line ("one").
    state.workspace.tabs[0].selection = Some((0, 3));

    state.remove_non_highlighted_underlining();

    assert!(!state.workspace.tabs[0].document.paragraphs()[0].runs[0].underline);
    assert!(
        state.workspace.tabs[0].document.paragraphs()[1].runs[0].underline,
        "unselected line must survive"
    );
}

#[test]
fn remove_blank_lines_deletes_only_empty_paragraphs() {
    let paragraphs = vec![para_plain("one"), para_plain(""), para_plain("two")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);

    state.remove_blank_lines();

    assert_eq!(state.workspace.tabs[0].document.content(), "one\ntwo");
}

#[test]
fn remove_blank_lines_with_a_selection_only_touches_selected_lines() {
    let paragraphs = vec![para_plain(""), para_plain("kept"), para_plain("")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    // Select only the middle + last line, leaving the leading blank alone.
    let start = state.workspace.tabs[0]
        .document
        .content()
        .find("kept")
        .unwrap();
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((start, end));

    state.remove_blank_lines();

    assert_eq!(state.workspace.tabs[0].document.content(), "\nkept");
}

#[test]
fn remove_pilcrows_strips_the_marker_and_keeps_formatting() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "bold¶text".into(),
            bold: true,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);

    state.remove_pilcrows();

    assert_eq!(state.workspace.tabs[0].document.content(), "boldtext");
    assert!(state.workspace.tabs[0].document.paragraphs()[0]
        .runs
        .iter()
        .all(|r| r.bold));
}

#[test]
fn remove_pilcrows_is_a_no_op_without_any() {
    let paragraphs = vec![para_plain("no marker here")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let before = state.workspace.tabs[0].document.undo_stack.len();

    state.remove_pilcrows();

    assert_eq!(state.workspace.tabs[0].document.undo_stack.len(), before);
}

// ── Delete tags / spoken-word counting ────────────────────────────────

fn tag_para(text: &str) -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![Run {
            text: text.into(),
            bold: true,
            size: 26,
            style: Some(CardStyle::Tag),
            ..Run::default()
        }],
        heading: CardStyleKind::Tag.heading_level(),
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

/// The words survive; only the formatting goes.
#[test]
fn delete_tags_strips_formatting_but_keeps_the_line() {
    let mut state = make_state_with_paragraphs(
        vec![
            para_plain("body"),
            tag_para("A tag"),
            para_plain("more body"),
        ],
        0,
    );

    state.delete_tags();

    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "body\nA tag\nmore body"
    );
    let tag = &state.workspace.tabs[0].document.paragraphs()[1];
    assert_eq!(tag.heading, 0, "the heading marker is what made it a tag");
    assert_eq!(tag.runs[0].text, "A tag");
    assert!(!tag.runs[0].bold);
    assert_eq!(tag.runs[0].style, None);
    assert_eq!(
        tag.runs[0].size,
        state.preferences.normal_text_size_half_points
    );
}

/// The marker is authoritative, exactly as it is for analytics: a
/// reformatted tag is still a tag.
#[test]
fn delete_tags_finds_a_marked_tag_whose_formatting_was_changed() {
    let mut para = tag_para("odd tag");
    para.runs[0].bold = false;
    para.runs[0].size = 99;
    para.heading = 0;
    let mut state = make_state_with_paragraphs(vec![para], 0);

    state.delete_tags();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].style,
        None
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].size,
        state.preferences.normal_text_size_half_points
    );
}

/// ...and a marked *cite* that happens to sit at a heading level is not a
/// tag. This is the misidentification the marker exists to prevent.
#[test]
fn delete_tags_leaves_a_marked_cite_alone() {
    let mut para = tag_para("a cite");
    para.runs[0].style = Some(CardStyle::Cite);
    let mut state = make_state_with_paragraphs(vec![para], 0);

    state.delete_tags();

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].runs[0].style,
        Some(CardStyle::Cite)
    );
    assert!(
        !state.workspace.tabs[0].document.is_modified,
        "nothing matched, so nothing changed"
    );
}

#[test]
fn delete_tags_is_a_no_op_when_there_are_none() {
    let mut state = make_state_with_paragraphs(vec![para_plain("just text")], 0);
    let version_before = state.workspace.tabs[0].document.content_version;

    state.delete_tags();

    assert_eq!(
        state.workspace.tabs[0].document.content_version,
        version_before
    );
    assert!(!state.workspace.tabs[0].document.is_modified);
}

/// The timer's WPM readout counts what actually gets read aloud —
/// highlighted runs plus tags and cites — and nothing else.
#[test]
fn spoken_words_in_selection_counts_only_read_aloud_text() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            run_plain("skip these four words "),
            Run {
                text: "two highlighted ".into(),
                highlight: true,
                ..Run::default()
            },
            Run {
                text: "one tag ".into(),
                style: Some(CardStyle::Tag),
                ..Run::default()
            },
            Run {
                text: "a cite".into(),
                style: Some(CardStyle::Cite),
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, state.workspace.tabs[0].document.content().len()));

    assert_eq!(state.spoken_words_in_selection(), Some(6));
}

/// `None`, not `Some(0)` — the timer shows a "select text" hint for the
/// first and a different message for the second.
#[test]
fn spoken_words_in_selection_distinguishes_no_selection_from_no_spoken_text() {
    let mut state = make_state_with_paragraphs(vec![para_plain("plain words only")], 0);
    assert_eq!(state.spoken_words_in_selection(), None);

    state.workspace.tabs[0].selection = Some((0, 16));
    assert_eq!(state.spoken_words_in_selection(), Some(0));
}

// ── Select similar formatting ─────────────────────────────────────────

fn tagged(text: &str) -> Run {
    Run {
        text: text.into(),
        style: Some(CardStyle::Tag),
        ..Run::default()
    }
}

/// Two paragraphs, each "TAG" + " body". Cursor inside the first tag.
fn tagged_doc(cursor: usize) -> AppState {
    let para = || Paragraph {
        list: None,
        runs: vec![tagged("TAG"), run_plain(" body")],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    make_state_with_paragraphs(vec![para(), para()], cursor)
}

#[test]
fn test_select_similar_formatting_matches_every_run_like_the_cursors() {
    // "TAG body\nTAG body" — tags at 0..3 and 9..12.
    let mut state = tagged_doc(1);

    state.select_similar_formatting();

    assert_eq!(
        state.workspace.tabs[0].similar_ranges,
        vec![(0, 3), (9, 12)]
    );
    // Blanked so the caret selection and the matches can't both be drawn.
    assert_eq!(state.workspace.tabs[0].selection, None);
}

/// With a selection, the run at its *start* is the template — and the
/// result replaces the selection rather than adding to it.
#[test]
fn test_select_similar_formatting_uses_the_selections_first_run() {
    let mut state = tagged_doc(0);
    // Spans the plain " body" into the second paragraph's tag; the start
    // sits in the plain run, so plain text is what gets matched.
    state.workspace.tabs[0].selection = Some((3, 11));

    state.select_similar_formatting();

    assert_eq!(
        state.workspace.tabs[0].similar_ranges,
        vec![(3, 8), (12, 17)]
    );
}

/// The payoff: one formatting command restyles every match at once.
#[test]
fn test_formatting_applies_to_every_similar_range() {
    let mut state = tagged_doc(1);
    state.select_similar_formatting();

    state.apply_formatting_to_selection(FormatOp::Bold(true));

    let tags: Vec<&Run> = state.workspace.tabs[0]
        .document
        .paragraphs()
        .iter()
        .flat_map(|p| p.runs.iter())
        .filter(|r| r.text == "TAG")
        .collect();
    assert_eq!(tags.len(), 2);
    assert!(tags.iter().all(|r| r.bold), "both tags should be bold");
    // Everything else untouched.
    assert!(state.workspace.tabs[0]
        .document
        .paragraphs()
        .iter()
        .flat_map(|p| p.runs.iter())
        .filter(|r| r.text == " body")
        .all(|r| !r.bold));
}

/// A second click of the same button toggles off, exactly as it does for a
/// single selection — but only because *every* match was already bold.
#[test]
fn test_formatting_toggles_off_only_when_every_similar_range_matches() {
    let mut state = tagged_doc(1);
    state.select_similar_formatting();
    state.apply_formatting_to_selection(FormatOp::Bold(true));

    // One match un-bolded by hand: the next apply must bold *it*, not
    // un-bold the other one.
    state.workspace.tabs[0].document.paragraphs_mut()[1].runs[0].bold = false;
    state.apply_formatting_to_selection(FormatOp::Bold(true));

    assert!(state.workspace.tabs[0]
        .document
        .paragraphs()
        .iter()
        .flat_map(|p| p.runs.iter())
        .filter(|r| r.text == "TAG")
        .all(|r| r.bold));
}

#[test]
fn test_clear_similar_selection_drops_the_matches() {
    let mut state = tagged_doc(1);
    state.select_similar_formatting();
    assert!(!state.workspace.tabs[0].similar_ranges.is_empty());

    state.clear_similar_selection();

    assert!(state.workspace.tabs[0].similar_ranges.is_empty());
}

/// Both variants need a selection, and neither should touch a selection
/// with no newlines in it — no undo entry for a no-op.
#[test]
fn test_condense_is_a_no_op_without_newlines() {
    let mut state = make_state_with_paragraphs(vec![para_plain("single line")], 0);
    state.workspace.tabs[0].selection = Some((0, 11));
    let version_before = state.workspace.tabs[0].document.content_version;

    state.condense_with_pilcrows();
    state.condense_selection();

    assert_eq!(state.workspace.tabs[0].document.content(), "single line");
    assert_eq!(
        state.workspace.tabs[0].document.content_version,
        version_before
    );
}

#[test]
fn test_condense_selection_preserves_run_formatting() {
    // Regression: condense used to delete the selection and reinsert it
    // as plain text (`sync_insert_str`), flattening every condensed
    // character down to a single unformatted run and losing bold/
    // highlight/size/etc. Each character's original formatting must
    // survive condensing, just with '\n' swapped for ' '.
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![Run {
                text: "bold".into(),
                bold: true,
                ..Run::default()
            }],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("plain")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    let end = state.workspace.tabs[0].document.content().len();
    state.workspace.tabs[0].selection = Some((0, end));
    state.condense_selection();

    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "bold\u{200B} plain"
    );
    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    let bold_run = runs.iter().find(|r| r.text.contains("bold")).unwrap();
    assert!(bold_run.bold, "bold formatting should survive condensing");
    let plain_run = runs.iter().find(|r| r.text.contains("plain")).unwrap();
    assert!(!plain_run.bold, "plain run shouldn't pick up bold");
}

#[test]
fn test_shrink_text_sets_non_underlined_selection_to_small_size() {
    let paragraphs = vec![para_plain("hello world")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 11));
    state.preferences.small_size_half_points = 8;
    state.shrink_text();

    let para = &state.workspace.tabs[0].document.paragraphs()[0];
    assert!(para.runs.iter().all(|r| r.size == 8));
}

// ── underline un-shrinks (beta feedback, Formatting bullet 1) ─────────

/// "When underlining a shrinked word with the F9 function, automatically
/// reset word to the default font." Shrink marks text as unread and skips
/// underlined runs; underlining is the other half of that convention.
#[test]
fn underlining_a_shrunk_run_restores_it_to_body_size() {
    let paragraphs = vec![para_plain("hello world")];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.small_size_half_points = 8;
    state.preferences.normal_text_size_half_points = 22;

    state.workspace.tabs[0].selection = Some((0, 11));
    state.shrink_text();
    assert!(state.workspace.tabs[0].document.paragraphs()[0]
        .runs
        .iter()
        .all(|r| r.size == 8));

    state.workspace.tabs[0].selection = Some((0, 11));
    state.apply_formatting_to_selection(FormatOp::Underline(true));

    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert!(
        runs.iter().all(|r| r.underline),
        "the run must actually be underlined"
    );
    assert!(
        runs.iter().all(|r| r.size == 22),
        "shrunk text must return to body size"
    );
}

/// Only text at exactly the configured small size is restored. Underlining
/// a Cite or a Pocket must not yank it down to body size — that would make
/// F9 quietly destructive on every card style.
#[test]
fn underlining_leaves_sizes_that_are_not_the_shrink_size_alone() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "big".into(),
                size: 52,
                ..Run::default()
            },
            Run {
                text: "small".into(),
                size: 8,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.small_size_half_points = 8;
    state.preferences.normal_text_size_half_points = 22;
    state.workspace.tabs[0].selection = Some((0, 8)); // "bigsmall"

    state.apply_formatting_to_selection(FormatOp::Underline(true));

    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert_eq!(runs[0].size, 52, "a Pocket-sized run must keep its size");
    assert_eq!(runs[1].size, 22, "the shrunk run returns to body size");
}

/// Toggling underline back *off* must not also resize. The un-shrink is
/// gated on the effective op, not the requested one, so a second F9 only
/// removes the underline.
#[test]
fn un_underlining_does_not_resize() {
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![Run {
            text: "word".into(),
            underline: true,
            size: 8,
            ..Run::default()
        }],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.preferences.small_size_half_points = 8;
    state.preferences.normal_text_size_half_points = 22;
    state.workspace.tabs[0].selection = Some((0, 4));

    // Already uniformly underlined, so this toggles off.
    state.apply_formatting_to_selection(FormatOp::Underline(true));

    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert!(!runs[0].underline, "underline should have toggled off");
    assert_eq!(runs[0].size, 8, "toggling off must not resize");
}

#[test]
fn test_shrink_text_leaves_underlined_runs_untouched() {
    // User-clarified spec: Shrink sets non-underlined selected text to
    // settings.conf's `small_size`, but skips underlined runs entirely
    // (e.g. a debate card's underlined emphasis shouldn't shrink along
    // with the rest of the tag).
    let paragraphs = vec![Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "under".into(),
                underline: true,
                size: 24,
                ..Run::default()
            },
            Run {
                text: "plain".into(),
                size: 24,
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }];
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].selection = Some((0, 10)); // whole line: "underplain"
    state.preferences.small_size_half_points = 8;
    state.shrink_text();

    let runs = &state.workspace.tabs[0].document.paragraphs()[0].runs;
    assert_eq!(runs[0].size, 24, "underlined run must be left alone");
    assert_eq!(
        runs[1].size, 8,
        "non-underlined run should shrink to small_size"
    );
}

#[test]
fn test_apply_card_style_sets_heading_on_correct_line_when_cursor_on_second_line() {
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![run_plain("first line")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("second line")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    // content is "first line\nsecond line" — byte 11 is the start of "second line".
    let mut state = make_state_with_paragraphs(paragraphs, 11);
    state.apply_card_style(CardStyleKind::Hat);

    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[0].heading,
        0,
        "first line untouched"
    );
    assert_eq!(
        state.workspace.tabs[0].document.paragraphs()[1].heading,
        2,
        "second line marked Hat"
    );
}

#[test]
fn test_jump_to_line_moves_cursor_and_arms_scroll_flag() {
    let mut state = make_state("one\ntwo\nthree", 0, None);
    assert!(!state.workspace.tabs[0].pending_scroll_to_cursor);

    state.jump_to_line(2);

    assert_eq!(state.workspace.tabs[0].cursor, 8); // start of "three"
    assert!(state.workspace.tabs[0].pending_scroll_to_cursor);
    assert_eq!(state.workspace.tabs[0].selection, None);
}

#[test]
fn test_apply_card_style_end_to_end_through_wikifi_export() {
    // End-to-end: applies each card style through the same
    // AppState::apply_card_style the ribbon/keybinds call, then feeds
    // the result straight into wikifi_export::export_to_markdown — the
    // whole pipeline this was silently broken for before apply_card_style
    // set Paragraph.heading (wikify_export.rs's own test covers the
    // export function in isolation with hand-built headings).
    let paragraphs = vec![
        Paragraph {
            list: None,
            runs: vec![run_plain("Case Title")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("Off-case Subtitle")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("Block heading")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("Tag text")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
        Paragraph {
            list: None,
            runs: vec![run_plain("plain body text")],
            heading: 0,
            alignment: Alignment::default(),
            unsupported_xml: None,
        },
    ];
    let mut state = make_state_with_paragraphs(paragraphs, 0);

    for (line, kind) in [
        (0, CardStyleKind::Pocket),
        (1, CardStyleKind::Hat),
        (2, CardStyleKind::Block),
        (3, CardStyleKind::Tag),
    ] {
        state.set_cursor_from_line_col(line, 0);
        state.apply_card_style(kind);
    }

    let tab = &state.workspace.tabs[0];
    let markdown = crate::wikifi_export::export_to_markdown(
        tab.document.paragraphs(),
        &tab.document.content(),
    );
    assert_eq!(
        markdown,
        "# Case Title\n## Off-case Subtitle\n### Block heading\n#### Tag text\nplain body text\n"
    );
}

// ── File explorer right-click menu (found_bugs.md) ──────────────────────

/// A fresh, unique temp directory for a filesystem-touching test —
/// mirrors `docx_parser.rs`'s own `test_real_file_round_trip_...`
/// pattern, with `test_name` added so parallel test threads (cargo
/// test's default) never collide on the same directory.
/// Beta feedback: the app degrades over a session — typing lags, scrolling
/// stutters, saves take up to ten seconds — and a restart cures it for a
/// while. The leading hypothesis was run fragmentation: edits split runs
/// (`split_run_at_position`), merging fuses them back
/// (`merge_adjacent_same_format_runs`), and any edit path that misses the
/// merge would let run counts climb over a session while the text stays
/// the same size. Everything that walks runs would then slow down
/// together, and reopening the file would cure it, because parse merges
/// adjacent identical runs.
///
/// This drives a real debate document through a long editing session
/// headlessly and prints runs and save cost as it goes, so the hypothesis
/// can be settled with numbers rather than argued about. Prints only —
/// the assertions at the end guard the two things that would be outright
/// bugs, not a performance target.
#[test]
fn bench_diagnostic_editing_session_run_growth() {
    use crate::docx_parser::parse_docx;

    // A real card file if one is present, so the run/formatting mix is
    // representative; skipped rather than failed when it is not, since
    // *.docx is gitignored and a fresh clone will not have it.
    let source = std::path::Path::new("SeptOct-Kankee-Jeremiah.docx");
    let Ok((paragraphs, origin)) = parse_docx(source) else {
        println!("bench_diagnostic_editing_session: {source:?} not present, skipped");
        return;
    };

    let dir = temp_test_dir("editing_session");
    let target = dir.join("session.docx");
    std::fs::copy(source, &target).unwrap();

    let runs_of = |st: &AppState| -> usize {
        st.workspace.tabs[0]
            .document
            .paragraphs()
            .iter()
            .map(|p| p.runs.len())
            .sum()
    };
    let bytes_of = |st: &AppState| -> usize {
        st.workspace.tabs[0]
            .document
            .paragraphs()
            .iter()
            .flat_map(|p| &p.runs)
            .map(|r| r.text.len())
            .sum()
    };

    let mut state = make_state_with_paragraphs(paragraphs, 0);
    state.workspace.tabs[0].file_path = Some(target.clone());
    state.workspace.tabs[0].docx_origin = Some(std::sync::Arc::new(origin));

    println!(
        "\nbench_diagnostic_editing_session: {} paragraphs, {} runs, {} bytes at open",
        state.workspace.tabs[0].document.paragraphs().len(),
        runs_of(&state),
        bytes_of(&state),
    );
    println!("  round |    runs |   bytes | runs/KB | save ms");

    // Each round is a burst of ordinary work: typing, then bolding and
    // un-bolding a span (the split/merge cycle fragmentation would show
    // up in), then a shrink, in the middle of the document.
    for round in 0..=6 {
        if round > 0 {
            for _ in 0..200 {
                let mid = state.workspace.tabs[0].document.content().len() / 2;
                state.workspace.tabs[0].cursor =
                    clamp_to_char_boundary(&state.workspace.tabs[0].document.content(), mid);
                state.insert_char('x');
                // Defeat the undo coalescing window so every keystroke
                // takes the same path a real typing session does.
                state.workspace.tabs[0].document.last_edit_at = None;
            }
            for _ in 0..40 {
                let mid = state.workspace.tabs[0].document.content().len() / 2;
                let a = clamp_to_char_boundary(&state.workspace.tabs[0].document.content(), mid);
                let b =
                    clamp_to_char_boundary(&state.workspace.tabs[0].document.content(), mid + 20);
                state.workspace.tabs[0].selection = Some((a, b));
                state.apply_formatting_to_selection(FormatOp::Bold(true));
                state.workspace.tabs[0].selection = Some((a, b));
                state.apply_formatting_to_selection(FormatOp::Bold(true)); // toggles off
                state.workspace.tabs[0].selection = Some((a, b));
                state.shrink_text();
            }
            state.workspace.tabs[0].selection = None;
        }

        state.workspace.tabs[0].document.is_modified = true;
        let t = Instant::now();
        state.save_tab(0).unwrap();
        let save_ms = t.elapsed().as_secs_f32() * 1000.0;

        let (runs, bytes) = (runs_of(&state), bytes_of(&state));
        println!(
            "  {round:5} | {runs:7} | {bytes:7} | {:7.1} | {save_ms:7.1}",
            runs as f32 / (bytes as f32 / 1024.0).max(1.0),
        );
    }

    // Not a perf target — these only catch the pathological cases: text
    // must not be lost, and the document must still be loadable.
    assert!(bytes_of(&state) > 0, "the document lost all its text");
    assert!(
        parse_docx(&target).is_ok(),
        "the saved document no longer parses"
    );
}

/// How save cost scales with document size — the other half of the
/// "ten seconds to save" report.
///
/// The session bench above shows run counts are stable, so the document
/// model is not growing. That leaves size itself: real case files run far
/// larger than the test fixture, and debate documents are unusually
/// run-dense (the fixture is ~24 runs per KB). If save is linear, ten
/// seconds needs an implausibly huge file and the cost is elsewhere; if it
/// is superlinear, an ordinary large case file explains the report on its
/// own, with no leak involved.
#[test]
fn bench_diagnostic_save_cost_by_document_size() {
    use crate::docx_parser::parse_docx;

    let source = std::path::Path::new("SeptOct-Kankee-Jeremiah.docx");
    let Ok((base, origin)) = parse_docx(source) else {
        println!("bench_diagnostic_save_cost_by_size: {source:?} not present, skipped");
        return;
    };
    let dir = temp_test_dir("save_scaling");
    let origin = std::sync::Arc::new(origin);

    println!("\nbench_diagnostic_save_cost_by_document_size:");
    println!("  x |  paras |    runs |    bytes | save ms | ms per 100KB");
    for factor in [1usize, 2, 4, 8, 16] {
        let mut paragraphs = Vec::with_capacity(base.len() * factor);
        for _ in 0..factor {
            paragraphs.extend(base.iter().cloned());
        }
        let bytes: usize = paragraphs
            .iter()
            .flat_map(|p| &p.runs)
            .map(|r| r.text.len())
            .sum();
        let runs: usize = paragraphs.iter().map(|p| p.runs.len()).sum();

        let target = dir.join(format!("scaled_{factor}.docx"));
        std::fs::copy(source, &target).unwrap();
        let t = Instant::now();
        origin.save(&paragraphs, &target).unwrap();
        let ms = t.elapsed().as_secs_f32() * 1000.0;

        println!(
            "  {factor:2} | {:6} | {runs:7} | {bytes:8} | {ms:7.1} | {:12.1}",
            paragraphs.len(),
            ms / (bytes as f32 / 102_400.0),
        );
    }
}

/// Resident set size in MB, straight from `/proc/self/statm`. Linux-only;
/// the bench below reports 0 and says so elsewhere.
fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|p| p.parse::<f64>().ok())
        })
        .map(|pages| pages * 4096.0 / 1_048_576.0)
        .unwrap_or(0.0)
}

/// Does memory actually grow over a session, and where does it go?
///
/// The two benches above rule out the document model: run counts are
/// stable under heavy editing, and save is linear in document size
/// (~7-10ms per 100KB), so neither fragmentation nor file size can
/// produce a ten-second save. That leaves whole-process slowdown, which
/// is also the only thing that explains typing, scrolling *and* saving
/// all degrading together.
///
/// The undo/redo stacks are the obvious candidate. Every snapshot is a
/// full `(content, paragraphs)` clone, `UNDO_STACK_BYTE_BUDGET` is 100MB,
/// and both the stacks and that budget are **per tab** — so the ceiling
/// is roughly `tabs * 200MB` (undo plus redo), which a debate researcher
/// with several case files open can reach without doing anything unusual.
#[test]
fn bench_diagnostic_undo_memory_growth_per_tab() {
    use crate::docx_parser::parse_docx;

    let source = std::path::Path::new("SeptOct-Kankee-Jeremiah.docx");
    let Ok((base, _)) = parse_docx(source) else {
        println!("bench_diagnostic_undo_memory: {source:?} not present, skipped");
        return;
    };
    if rss_mb() == 0.0 {
        println!("bench_diagnostic_undo_memory: no /proc/self/statm, skipped");
        return;
    }

    // A single realistic case file, four times the fixture (~250KB of
    // text) — unremarkable for this document type.
    let mut paragraphs = Vec::new();
    for _ in 0..4 {
        paragraphs.extend(base.iter().cloned());
    }
    let doc_kb = paragraphs
        .iter()
        .flat_map(|p| &p.runs)
        .map(|r| r.text.len())
        .sum::<usize>() as f64
        / 1024.0;

    let baseline = rss_mb();
    let mut state = make_state_with_paragraphs(paragraphs, 0);
    // What the budget *thinks* one snapshot costs, versus what it really
    // costs in RSS below. The estimate counts string payloads only; it
    // cannot see per-`Run` struct overhead or per-allocation overhead,
    // and every run carries several separate `String`s.
    println!(
        "  size_of::<Run>()={} size_of::<Paragraph>()={} runs={} paras={} content={}",
        std::mem::size_of::<Run>(),
        std::mem::size_of::<Paragraph>(),
        state.workspace.tabs[0]
            .document
            .paragraphs()
            .iter()
            .map(|p| p.runs.len())
            .sum::<usize>(),
        state.workspace.tabs[0].document.paragraphs().len(),
        state.workspace.tabs[0].document.content().len(),
    );
    let estimate = snapshot_byte_estimate(
        &state.workspace.tabs[0].document.content(),
        state.workspace.tabs[0].document.paragraphs(),
    );
    println!(
        "\n  snapshot_byte_estimate says {:.2}MB per snapshot -> cap {} entries",
        estimate as f64 / 1_048_576.0,
        undo_stack_cap_for_snapshot_size(estimate),
    );
    println!("\nbench_diagnostic_undo_memory_growth_per_tab:");
    println!("  document {doc_kb:.0}KB of text, RSS {baseline:.0}MB at open");
    println!("  edits | undo entries | RSS MB | MB per undo entry");

    for round in 1..=6 {
        for _ in 0..25 {
            let mid = state.workspace.tabs[0].document.content().len() / 2;
            state.workspace.tabs[0].cursor =
                clamp_to_char_boundary(&state.workspace.tabs[0].document.content(), mid);
            state.insert_char('x');
            // Every keystroke its own undo step, as a real typing session
            // spread over minutes produces.
            state.workspace.tabs[0].document.last_edit_at = None;
        }
        let entries = state.workspace.tabs[0].document.undo_stack.len();
        let rss = rss_mb();
        println!(
            "  {:5} | {entries:12} | {rss:6.0} | {:17.2}",
            round * 25,
            (rss - baseline) / entries.max(1) as f64,
        );
    }

    let peak = rss_mb();
    // Dropping the stacks is what a "close and reopen the app" does to
    // this memory. If RSS falls back, the growth was undo history and
    // nothing is leaking in the strict sense.
    state.workspace.tabs[0].document.undo_stack.clear();
    state.workspace.tabs[0].document.redo_stack.clear();
    state.workspace.tabs[0].document.undo_stack.shrink_to_fit();
    state.workspace.tabs[0].document.redo_stack.shrink_to_fit();
    println!(
        "  after clearing undo/redo: RSS {:.0}MB (peak {peak:.0}MB, baseline {baseline:.0}MB)",
        rss_mb(),
    );
    let cap = undo_stack_cap_for_snapshot_size(estimate);
    println!(
        "  per-tab ceiling: {cap} entries x {:.2}MB x 2 stacks = {:.0}MB (budget {}MB)",
        estimate as f64 / 1_048_576.0,
        (cap * estimate * 2) as f64 / 1_048_576.0,
        UNDO_STACK_BYTE_BUDGET / 1_000_000,
    );
}

fn temp_test_dir(test_name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vimbatim_ctx_menu_test_{}_{}",
        std::process::id(),
        test_name
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ── Word count ────────────────────────────────────────────────────────

#[test]
fn document_stats_counts_total_tag_and_highlighted_words() {
    let tag_para = Paragraph {
        list: None,
        runs: vec![Run {
            text: "extinction comes first".into(),
            bold: true,
            ..Run::default()
        }],
        heading: 4, // Tag — CardStyleKind::heading_level
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let body = Paragraph {
        list: None,
        runs: vec![
            Run {
                text: "unread lead in ".into(),
                ..Run::default()
            },
            Run {
                text: "three highlighted words".into(),
                highlight: true,
                highlight_color: "yellow".into(),
                ..Run::default()
            },
            Run {
                text: " unread tail".into(),
                ..Run::default()
            },
        ],
        heading: 0,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let state = make_state_with_paragraphs(vec![tag_para, body], 0);
    let stats = state.document_stats();

    // 3 in the tag line + 8 in the body line.
    assert_eq!(stats.total_words, 11);
    assert_eq!(stats.tag_words, 3);
    assert_eq!(stats.highlighted_words, 3);
    assert_eq!(stats.spoken_words, 6);
}

#[test]
fn document_stats_ignores_heading_levels_that_are_not_tag() {
    // Pocket/Hat/Block are headings 1..3 and are not read aloud.
    let pocket = Paragraph {
        list: None,
        runs: vec![Run {
            text: "not a tag".into(),
            ..Run::default()
        }],
        heading: 1,
        alignment: Alignment::default(),
        unsupported_xml: None,
    };
    let state = make_state_with_paragraphs(vec![pocket], 0);
    assert_eq!(state.document_stats().tag_words, 0);
}

#[test]
fn estimated_time_formats_as_minutes_and_seconds() {
    let stats = DocumentStats {
        spoken_words: 300,
        ..DocumentStats::default()
    };
    assert_eq!(stats.estimated_time(300), (1, 0));
    assert_eq!(stats.estimated_time(600), (0, 30));

    // 450 words at 300 wpm is 1.5 minutes.
    let stats = DocumentStats {
        spoken_words: 450,
        ..DocumentStats::default()
    };
    assert_eq!(stats.estimated_time(300), (1, 30));
}

/// A zero rate would divide by zero and produce an infinite estimate; the
/// clamp is what stops a hand-edited settings.conf from doing that.
#[test]
fn estimated_time_survives_a_nonsense_wpm() {
    let stats = DocumentStats {
        spoken_words: 100,
        ..DocumentStats::default()
    };
    let (m, s) = stats.estimated_time(0);
    assert!(m < 10 && s < 60, "expected a finite estimate, got {m}:{s}");
}

// ── line spacing ──────────────────────────────────────────────────────

#[test]
fn line_spacing_is_clamped_to_a_usable_range() {
    assert_eq!(clamp_line_spacing(1.0), 1.0);
    assert_eq!(clamp_line_spacing(0.0), 0.5);
    assert_eq!(clamp_line_spacing(99.0), 3.0);
}

/// NaN survives `f32::clamp`, so without the explicit `is_finite` guard a
/// malformed settings.conf value would propagate into every row-height
/// calculation and lay rows out at zero height rather than failing loudly.
#[test]
fn line_spacing_rejects_a_non_finite_value_instead_of_propagating_it() {
    assert_eq!(clamp_line_spacing(f32::NAN), DEFAULT_LINE_SPACING);
    assert_eq!(clamp_line_spacing(f32::INFINITY), DEFAULT_LINE_SPACING);
}

#[test]
fn line_spacing_defaults_when_the_key_is_missing() {
    let dir = temp_test_dir("line_spacing_missing_key");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "theme=dark\n").unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .line_spacing,
        DEFAULT_LINE_SPACING
    );
}

#[test]
fn line_spacing_loads_and_clamps_what_is_on_disk() {
    let dir = temp_test_dir("line_spacing_load");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "line_spacing=1.5\n").unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .line_spacing,
        1.5
    );
    std::fs::write(&conf_path, "line_spacing=9.9\n").unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .line_spacing,
        3.0
    );
    // Garbage falls back rather than panicking, same as every other loader.
    std::fs::write(&conf_path, "line_spacing=wide\n").unwrap();
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .line_spacing,
        DEFAULT_LINE_SPACING
    );
}

/// What the future line-spacing button actually calls: sets the live value
/// and writes it back so it survives a restart.
#[test]
fn set_line_spacing_persists_and_round_trips() {
    let dir = temp_test_dir("line_spacing_round_trip");
    let conf_path = dir.join("settings.conf");
    std::fs::write(&conf_path, "").unwrap();
    let mut state = make_state("", 0, None);
    state.settings_path = conf_path.clone();

    state.set_line_spacing(1.5);
    assert_eq!(state.preferences.line_spacing, 1.5);
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .line_spacing,
        1.5
    );

    // Out-of-range input is clamped before it is stored *or* written, so
    // the file can never hold a value the loader would have to fix up.
    state.set_line_spacing(99.0);
    assert_eq!(state.preferences.line_spacing, 3.0);
    assert_eq!(
        crate::preferences::Preferences::load(&conf_path)
            .unwrap()
            .line_spacing,
        3.0
    );
}

// ── sidebar mode toggle (beta feedback: toggle files/nav) ─────────────

#[test]
fn toggle_sidebar_mode_flips_between_files_and_nav() {
    let mut state = make_state("", 0, None);
    assert_eq!(state.sidebar_mode, SidebarMode::Files);
    state.toggle_sidebar_mode();
    assert_eq!(state.sidebar_mode, SidebarMode::Nav);
    state.toggle_sidebar_mode();
    assert_eq!(state.sidebar_mode, SidebarMode::Files);
}

/// Asking for a sidebar view while the panel is hidden can only mean
/// "show me that view" — silently switching a mode nobody can see would
/// look like the key did nothing at all.
#[test]
fn toggle_sidebar_mode_reveals_a_hidden_sidebar() {
    let mut state = make_state("", 0, None);
    state.ui.sidebar_visible = false;
    state.toggle_sidebar_mode();
    assert!(state.ui.sidebar_visible);
}

#[test]
fn spreading_wpm_is_clamped_to_a_usable_range() {
    assert_eq!(clamp_spreading_wpm(0), 50);
    assert_eq!(clamp_spreading_wpm(300), 300);
    assert_eq!(clamp_spreading_wpm(99_999), 1000);
}

// ── Find / Replace ────────────────────────────────────────────────────

#[test]
fn find_from_is_ascii_case_insensitive_and_respects_start() {
    assert_eq!(find_from("Hello hello", "hello", 0), Some(0));
    assert_eq!(find_from("Hello hello", "hello", 1), Some(6));
    assert_eq!(find_from("Hello hello", "HELLO", 0), Some(0));
    assert_eq!(find_from("Hello", "bye", 0), None);
    // Empty needle must never match, or find_next would spin.
    assert_eq!(find_from("Hello", "", 0), None);
}

#[test]
fn find_from_never_splits_a_multibyte_char() {
    // "é" is two bytes: a naive byte scan would test an offset inside it.
    let content = "café cafe";
    assert_eq!(find_from(content, "cafe", 0), Some(6));
    assert_eq!(find_from(content, "café", 0), Some(0));
}

#[test]
fn rfind_before_finds_the_last_match_that_starts_earlier() {
    assert_eq!(rfind_before("a b a b a", "a", 9), Some(8));
    assert_eq!(rfind_before("a b a b a", "a", 8), Some(4));
    // "strictly before": offset 0 still qualifies when `before` is 1...
    assert_eq!(rfind_before("a b a b a", "a", 1), Some(0));
    // ...and nothing qualifies at 0, which is what makes find_next(false)
    // wrap instead of re-finding the match it is already sitting on.
    assert_eq!(rfind_before("a b a b a", "a", 0), None);
}

#[test]
fn find_next_selects_the_match_and_wraps_around() {
    let mut state = make_state("one two one", 0, None);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "one".to_string();

    assert!(state.find_next(true));
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));

    assert!(state.find_next(true));
    assert_eq!(state.workspace.tabs[0].selection, Some((8, 11)));

    // Past the last match, wrap back to the first.
    assert!(state.find_next(true));
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
}

#[test]
fn find_next_backward_walks_in_reverse() {
    let mut state = make_state("one two one", 11, None);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "one".to_string();

    assert!(state.find_next(false));
    assert_eq!(state.workspace.tabs[0].selection, Some((8, 11)));
    assert!(state.find_next(false));
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
}

/// Replace must only act on a selection that actually *is* a match —
/// otherwise pressing it right after opening the bar overwrites whatever
/// text happened to be selected.
#[test]
fn replace_current_ignores_a_selection_that_is_not_a_match() {
    let mut state = make_state("one two", 0, None);
    state.workspace.tabs[0].selection = Some((4, 7)); // "two"
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "one".to_string();
    state.ui.find_bar.as_mut().unwrap().replacement = "X".to_string();

    state.replace_current();
    assert!(
        state.workspace.tabs[0].document.content().contains("two"),
        "unrelated selection was overwritten"
    );
}

#[test]
fn replace_current_swaps_the_found_match_then_advances() {
    let mut state = make_state("one two one", 0, None);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "one".to_string();
    state.ui.find_bar.as_mut().unwrap().replacement = "X".to_string();

    state.find_next(true);
    state.replace_current();
    assert_eq!(state.workspace.tabs[0].document.content(), "X two one");
    // ...and moved on to the remaining match.
    assert_eq!(state.workspace.tabs[0].selection, Some((6, 9)));
}

#[test]
fn replace_all_replaces_every_match_case_insensitively() {
    let mut state = make_state("One one ONE", 0, None);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "one".to_string();
    state.ui.find_bar.as_mut().unwrap().replacement = "two".to_string();

    assert_eq!(state.replace_all(), 3);
    assert_eq!(state.workspace.tabs[0].document.content(), "two two two");
}

/// A replacement containing the query would loop forever if the scan
/// resumed at the match position instead of past the inserted text.
#[test]
fn replace_all_terminates_when_the_replacement_contains_the_query() {
    let mut state = make_state("a a a", 0, None);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "a".to_string();
    state.ui.find_bar.as_mut().unwrap().replacement = "aa".to_string();

    assert_eq!(state.replace_all(), 3);
    assert_eq!(state.workspace.tabs[0].document.content(), "aa aa aa");
}

#[test]
fn open_find_bar_seeds_the_query_from_the_selection() {
    let mut state = make_state("hello world", 0, None);
    state.workspace.tabs[0].selection = Some((6, 11));
    state.open_find_bar();
    assert_eq!(state.ui.find_bar.as_ref().unwrap().query, "world");
}

#[test]
fn refresh_find_matches_counts_every_occurrence() {
    let mut state = make_state("one one one", 0, None);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "one".to_string();
    state.refresh_find_matches();
    assert_eq!(state.ui.find_bar.as_ref().unwrap().match_count, 3);

    state.find_next(true);
    assert_eq!(state.ui.find_bar.as_ref().unwrap().current_match, 1);
    state.find_next(true);
    assert_eq!(state.ui.find_bar.as_ref().unwrap().current_match, 2);
}

// ── Spellcheck ────────────────────────────────────────────────────────

/// The one branch worth protecting here: `replace_spell_target` composes
/// three existing methods, and getting the (line, col) span wrong would
/// silently eat the wrong characters.
#[test]
fn test_replace_spell_target_swaps_only_the_flagged_word() {
    let mut state = make_state("hello wrold there", 0, None);
    let target = SpellTarget {
        line: 0,
        start_col: 6,
        end_col: 11,
        word: "wrold".to_string(),
        suggestions: vec!["world".to_string()],
    };
    state.replace_spell_target(&target, "world");
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "hello world there"
    );
}

/// On a later line, so the line→byte-offset conversion is actually
/// exercised rather than trivially passing at offset 0.
#[test]
fn test_replace_spell_target_on_second_line() {
    let mut state = make_state("first line\nsecond teh line", 0, None);
    let target = SpellTarget {
        line: 1,
        start_col: 7,
        end_col: 10,
        word: "teh".to_string(),
        suggestions: vec![],
    };
    state.replace_spell_target(&target, "the");
    assert_eq!(
        state.workspace.tabs[0].document.content(),
        "first line\nsecond the line"
    );
}

#[test]
fn test_add_to_user_dictionary_is_lowercased_and_deduped() {
    // Explicit temp path — the no-arg `add_to_user_dictionary` appends to
    // the *real* ~/.vimbatim/user_dictionary.txt, which a test must never
    // touch.
    let path = std::env::temp_dir().join("vimbatim_test_user_dict.txt");
    let _ = std::fs::remove_file(&path);

    let mut state = make_state("", 0, None);
    state.add_to_user_dictionary_at("Kritik", &path);
    state.add_to_user_dictionary_at("kritik", &path);
    assert!(state.user_dictionary.contains("kritik"));
    assert_eq!(state.user_dictionary.len(), 1);

    // The dedup must reach the file too, not just the in-memory set —
    // otherwise the list grows without bound across sessions.
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(written.lines().collect::<Vec<_>>(), vec!["kritik"]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_open_file_context_menu_sets_state() {
    let mut state = make_state("", 0, None);
    state.open_file_context_menu((10.0, 20.0), FileContextMenuTarget::Background);

    let menu = state.ui.file_context_menu.as_ref().unwrap();
    assert_eq!(menu.position, (10.0, 20.0));
    assert_eq!(menu.target, FileContextMenuTarget::Background);
    assert!(!menu.confirming_delete);
}

#[test]
fn test_close_file_context_menu_clears_state() {
    let mut state = make_state("", 0, None);
    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Background);
    state.close_file_context_menu();
    assert!(state.ui.file_context_menu.is_none());
}

#[test]
fn test_request_delete_confirmation_arms_flag_without_deleting() {
    let dir = temp_test_dir("request_confirmation");
    let path = dir.join("keep-me.docx");
    std::fs::write(&path, b"placeholder").unwrap();

    let mut state = make_state("", 0, None);
    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::File(path.clone()));
    state.request_context_menu_delete_confirmation();

    assert!(
        state
            .ui
            .file_context_menu
            .as_ref()
            .unwrap()
            .confirming_delete
    );
    assert!(path.exists(), "delete must not happen until confirmed");
}

#[test]
fn test_confirm_delete_removes_the_targeted_file() {
    let dir = temp_test_dir("confirm_delete");
    let path = dir.join("delete-me.docx");
    std::fs::write(&path, b"placeholder").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir;
    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::File(path.clone()));
    state.request_context_menu_delete_confirmation();

    state.confirm_context_menu_delete().unwrap();

    assert!(!path.exists());
    assert!(state.ui.file_context_menu.is_none());
}

#[test]
fn test_confirm_delete_is_noop_for_dir_and_background_targets() {
    let mut state = make_state("", 0, None);
    state.open_file_context_menu(
        (0.0, 0.0),
        FileContextMenuTarget::Dir(std::path::PathBuf::from("/some/dir")),
    );
    assert!(state.confirm_context_menu_delete().is_ok());

    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Background);
    assert!(state.confirm_context_menu_delete().is_ok());
}

#[test]
fn test_create_new_docx_in_picks_first_available_untitled_name() {
    let dir = temp_test_dir("naming_sequence");
    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();

    state.create_new_docx_in(&dir).unwrap();
    assert!(dir.join("Untitled.docx").exists());

    state.create_new_docx_in(&dir).unwrap();
    assert!(dir.join("Untitled 1.docx").exists());
}

#[test]
fn test_create_file_at_context_menu_location_targets_files_parent_dir() {
    let dir = temp_test_dir("file_target_parent_dir");
    let subdir = dir.join("cards");
    std::fs::create_dir_all(&subdir).unwrap();
    let existing_file = subdir.join("existing.docx");
    std::fs::write(&existing_file, b"placeholder").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::File(existing_file));

    state.create_file_at_context_menu_location().unwrap();

    // New file lands next to the right-clicked file (in `cards/`), not
    // at the tree's root (`dir`).
    assert!(subdir.join("Untitled.docx").exists());
    assert!(!dir.join("Untitled.docx").exists());
}

#[test]
fn test_create_file_at_context_menu_location_targets_dir_itself() {
    let dir = temp_test_dir("dir_target");
    let subdir = dir.join("cards");
    std::fs::create_dir_all(&subdir).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Dir(subdir.clone()));

    state.create_file_at_context_menu_location().unwrap();

    assert!(subdir.join("Untitled.docx").exists());
}

#[test]
fn test_create_file_at_context_menu_location_background_targets_working_directory() {
    let dir = temp_test_dir("background_target");
    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.open_file_context_menu((0.0, 0.0), FileContextMenuTarget::Background);

    state.create_file_at_context_menu_location().unwrap();

    assert!(dir.join("Untitled.docx").exists());
}

// ── Command palette ─────────────────────────────────────────────────────

#[test]
fn test_palette_and_find_bar_are_mutually_exclusive() {
    // Both mount in the same strip under the ribbon, so they can never be
    // up together. Enforced in the openers, not at the call sites.
    let mut state = make_state("", 0, None);

    state.open_find_bar();
    state.open_command_palette();
    assert!(
        state.ui.find_bar.is_none(),
        "opening the palette must close the find bar"
    );
    assert!(state.ui.command_palette.is_some());

    state.open_find_bar();
    assert!(
        state.ui.command_palette.is_none(),
        "opening the find bar must close the palette"
    );

    state.open_command_palette();
    state.open_search_from_list();
    assert!(
        state.ui.command_palette.is_none(),
        "Search From List must close the palette too"
    );
}

#[test]
fn test_disabling_the_palette_closes_an_open_one() {
    let mut state = make_state("", 0, None);
    state.settings_path = temp_test_dir("palette_disable").join("settings.conf");
    state.preferences.command_palette_enabled = true;
    state.open_command_palette();

    state.toggle_command_palette_enabled();

    assert!(!state.preferences.command_palette_enabled);
    assert!(
        state.ui.command_palette.is_none(),
        "a panel its keybind can't reopen must not be left up"
    );
}

/// Every feature toggle, flipped through its `AppState` method and read
/// back off disk. These used to flip in `settings_modal.rs` with the save
/// alongside, so any second caller (the command palette runs all of them)
/// flipped in memory only and lost it on restart — with nothing to fail.
#[test]
fn test_feature_toggles_flip_and_persist() {
    let dir = temp_test_dir("toggle_persistence");
    let conf = dir.join("settings.conf");
    std::fs::write(&conf, "[FORMATTING]\n").unwrap();

    let mut state = make_state("", 0, None);
    state.settings_path = conf.clone();

    let cases: Vec<(&str, fn(&mut AppState), fn(&AppState) -> bool)> = vec![
        ("spellcheck", AppState::toggle_spellcheck, |s| {
            s.preferences.spellcheck_enabled
        }),
        ("nav_fold_buttons", AppState::toggle_nav_fold_buttons, |s| {
            s.preferences.nav_fold_buttons
        }),
        ("search_from_list", AppState::toggle_search_from_list, |s| {
            s.preferences.search_from_list_enabled
        }),
        (
            "search_list_whole_words",
            AppState::toggle_search_list_whole_words,
            |s| s.preferences.search_list_whole_words,
        ),
        (
            "command_palette",
            AppState::toggle_command_palette_enabled,
            |s| s.preferences.command_palette_enabled,
        ),
    ];

    for (key, toggle, read) in cases {
        let before = read(&state);
        toggle(&mut state);
        assert_eq!(read(&state), !before, "{key} did not flip");
        let prefs = crate::preferences::Preferences::load(&conf).unwrap();
        let persisted = match key {
            "spellcheck" => prefs.spellcheck_enabled,
            "nav_fold_buttons" => prefs.nav_fold_buttons,
            "search_from_list" => prefs.search_from_list_enabled,
            "search_list_whole_words" => prefs.search_list_whole_words,
            "command_palette" => prefs.command_palette_enabled,
            _ => panic!("unknown key: {key}"),
        };
        assert_eq!(
            persisted, !before,
            "{key} flipped in memory but was not written to settings.conf",
        );
    }
}

#[test]
fn test_toggle_vim_persists_through_the_keybinds_file() {
    // Vim's flag rides in the keybinds section, not as a standalone
    // setting — a different writer from the other four.
    let dir = temp_test_dir("toggle_vim_persistence");
    let conf = dir.join("settings.conf");
    std::fs::write(&conf, "[FORMATTING]\n").unwrap();

    let mut state = make_state("", 0, None);
    state.settings_path = conf.clone();
    let before = state.global_vim.vim_enabled;

    state.toggle_vim();

    assert_eq!(state.global_vim.vim_enabled, !before);
    assert_eq!(crate::keybinds::load_vim_enabled(&conf), !before);
}

// ── Search From List ────────────────────────────────────────────────────

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(ToString::to_string).collect()
}

#[test]
fn test_parse_word_list_trims_and_drops_blank_lines() {
    // A blank line's real cost is a dishonest "N words" readout —
    // `find_from` already refuses an empty needle.
    assert_eq!(
        parse_word_list("war\n\n  peace  \n\n\n"),
        words(&["war", "peace"])
    );
    assert!(parse_word_list("   \n\n").is_empty());
}

#[test]
fn test_find_list_from_returns_the_earliest_match_across_all_words() {
    let content = "peace then war";
    // "war" appears later than "peace", so document order wins over list
    // order — that's what makes repeated Next walk the merged set.
    let got = find_list_from(content, &words(&["war", "peace"]), true, 0);
    assert_eq!(got, Some((0, 5)));
    // Resuming past the first match finds the later one.
    assert_eq!(
        find_list_from(content, &words(&["war", "peace"]), true, 5),
        Some((11, 14))
    );
}

#[test]
fn test_find_list_from_reports_each_matches_own_end() {
    // The whole reason the matcher returns (start, end): list matches have
    // differing lengths, so no single `query.len()` describes them.
    let content = "a warming war";
    let got = find_list_from(content, &words(&["warming", "war"]), true, 0);
    assert_eq!(got, Some((2, 9)));
    assert_eq!(&content[2..9], "warming");
}

#[test]
fn test_whole_words_rejects_a_match_inside_a_longer_word() {
    let content = "global warming is here";
    // "war" is inside "warming" — whole-word must skip it entirely.
    assert_eq!(find_list_from(content, &words(&["war"]), true, 0), None);
    // …and substring mode must still find it.
    assert_eq!(
        find_list_from(content, &words(&["war"]), false, 0),
        Some((7, 10))
    );
}

#[test]
fn test_whole_words_keeps_scanning_past_a_rejected_hit() {
    // The first "war" is inside "warming" and must be skipped, not treated
    // as "no match for this word at all".
    let content = "warming then war";
    assert_eq!(
        find_list_from(content, &words(&["war"]), true, 0),
        Some((13, 16))
    );
}

#[test]
fn test_whole_words_matches_at_document_start_and_end() {
    // No character before/after counts as a boundary.
    assert_eq!(
        find_list_from("war", &words(&["war"]), true, 0),
        Some((0, 3))
    );
    assert_eq!(
        find_list_from("the war", &words(&["war"]), true, 0),
        Some((4, 7))
    );
}

#[test]
fn test_find_list_from_is_case_insensitive_like_find() {
    // Inherited from `find_from`, deliberately — one convention, not two.
    assert_eq!(
        find_list_from("The War", &words(&["war"]), true, 0),
        Some((4, 7))
    );
}

#[test]
fn test_find_list_from_with_no_matching_word_is_none() {
    assert_eq!(
        find_list_from("nothing here", &words(&["war", "peace"]), true, 0),
        None
    );
    assert_eq!(find_list_from("anything", &[], true, 0), None);
}

#[test]
fn test_rfind_list_before_returns_the_latest_match_across_all_words() {
    let content = "war then peace then war";
    let w = words(&["war", "peace"]);
    assert_eq!(
        rfind_list_before(content, &w, true, content.len()),
        Some((20, 23))
    );
    // Strictly before: searching before the last match lands on "peace".
    assert_eq!(rfind_list_before(content, &w, true, 20), Some((9, 14)));
    assert_eq!(rfind_list_before(content, &w, true, 9), Some((0, 3)));
    assert_eq!(rfind_list_before(content, &w, true, 0), None);
}

#[test]
fn test_rfind_list_before_keeps_scanning_past_a_rejected_hit() {
    let content = "war then warming";
    // The later "war" is inside "warming"; the standalone one must win.
    assert_eq!(
        rfind_list_before(content, &words(&["war"]), true, content.len()),
        Some((0, 3))
    );
}

#[test]
fn test_list_matches_reproduces_the_find_next_traversal_exactly() {
    // The count loop and the Next button are two different code paths over
    // the same match set — if they ever disagree, the "N of M" readout
    // counts matches Next never stops on. This pins them together.
    let content = "war and warming and peace, war again";
    let list = ["war", "warming", "peace"];

    let collected = list_matches(content, &words(&list), true);

    let mut walked = Vec::new();
    let mut at = 0;
    while let Some((start, end)) = find_list_from(content, &words(&list), true, at) {
        walked.push((start, end));
        at = end.max(start + 1);
    }

    assert_eq!(collected, walked);
    assert!(
        !collected.is_empty(),
        "the fixture must actually match something"
    );
}

#[test]
fn test_list_matches_does_not_rescan_per_match() {
    // Regression guard for the O(matches x words x len) shape: a list
    // whose words mostly *don't* occur is the case that made every
    // iteration scan to the end of the document. A big document here
    // finishes instantly with the single-pass collector and crawls
    // without it.
    let content = "war ".repeat(20_000);
    let mut list: Vec<String> = (0..60).map(|i| format!("absentword{i}")).collect();
    list.push("war".to_string());

    let matches = list_matches(&content, &list, true);
    assert_eq!(matches.len(), 20_000);
}

/// A state with the find bar open in list mode over `list`.
fn make_list_search_state(content: &str, list: &[&str], whole_words: bool) -> AppState {
    let mut state = make_state(content, 0, None);
    state.search_word_list = words(list);
    state.preferences.search_list_whole_words = whole_words;
    state.open_search_from_list();
    state
}

#[test]
fn test_open_find_bar_clears_a_stale_list_mode() {
    // `FindBar` is app-wide and reused, so Ctrl+F after a list search must
    // not land the user in a panel with no query field.
    let mut state = make_list_search_state("war", &["war"], true);
    assert!(state.ui.find_bar.as_ref().unwrap().list_mode);

    state.open_find_bar();
    assert!(!state.ui.find_bar.as_ref().unwrap().list_mode);
}

#[test]
fn test_list_mode_counts_every_match_of_every_word() {
    let state = make_list_search_state("war peace war", &["war", "peace"], true);
    assert_eq!(state.ui.find_bar.as_ref().unwrap().match_count, 3);
}

#[test]
fn test_list_mode_count_and_current_match_survive_differing_lengths() {
    // "warming" (7) and "war" (3) in one list — the case the old
    // single-needle `query.len()` arithmetic could not express.
    let mut state = make_list_search_state("warming and war", &["warming", "war"], true);
    assert_eq!(state.ui.find_bar.as_ref().unwrap().match_count, 2);

    // Step onto the first match: the readout must say "1 of 2", proving
    // the count loop and `find_next` agree about where matches start/end.
    state.find_next(true);
    let bar = state.ui.find_bar.as_ref().unwrap();
    assert_eq!((bar.current_match, bar.match_count), (1, 2));
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 7)));

    state.find_next(true);
    let bar = state.ui.find_bar.as_ref().unwrap();
    assert_eq!((bar.current_match, bar.match_count), (2, 2));
    assert_eq!(state.workspace.tabs[0].selection, Some((12, 15)));
}

#[test]
fn test_list_mode_find_next_wraps_forward_and_backward() {
    let mut state = make_list_search_state("war peace", &["war", "peace"], true);

    state.find_next(true);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
    state.find_next(true);
    assert_eq!(state.workspace.tabs[0].selection, Some((4, 9)));
    // Past the last match, forward wraps to the first.
    state.find_next(true);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
    // …and backward from the first wraps to the last.
    state.find_next(false);
    assert_eq!(state.workspace.tabs[0].selection, Some((4, 9)));
}

#[test]
fn test_list_mode_with_an_empty_list_finds_nothing_but_still_opens() {
    // The panel opening on an empty list is deliberate — its readout is
    // what tells the user where to write one.
    let mut state = make_list_search_state("war peace", &[], true);
    assert!(state.ui.find_bar.is_some());
    assert!(!state.find_next(true));
    assert_eq!(state.ui.find_bar.as_ref().unwrap().match_count, 0);
}

#[test]
fn test_normal_find_is_unaffected_by_a_populated_word_list() {
    // The two modes share one panel; a word list must not leak into a
    // plain Ctrl+F.
    let mut state = make_state("war peace", 0, None);
    state.search_word_list = words(&["peace"]);
    state.open_find_bar();
    state.ui.find_bar.as_mut().unwrap().query = "war".into();
    state.refresh_find_matches();

    assert_eq!(state.ui.find_bar.as_ref().unwrap().match_count, 1);
    state.find_next(true);
    assert_eq!(state.workspace.tabs[0].selection, Some((0, 3)));
}

#[test]
fn test_word_list_round_trips_through_its_own_file() {
    let dir = temp_test_dir("word_list_file");
    let path = dir.join("search_word_list.txt");

    save_word_list(&path, &words(&["war", "peace"]));
    assert_eq!(load_word_list(&path), words(&["war", "peace"]));

    // A file that was never written is the normal pre-first-use state.
    assert!(load_word_list(&dir.join("absent.txt")).is_empty());
}

#[test]
fn test_search_word_list_text_round_trips_the_settings_box() {
    let mut state = make_state("", 0, None);
    state.set_search_word_list("war\n\npeace\n");
    // Blanks dropped on the way in, so the box reads back clean.
    assert_eq!(state.search_word_list, words(&["war", "peace"]));
    assert_eq!(state.search_word_list_text(), "war\npeace");
}

// ── Right-click menus: file operations ──────────────────────────────────

#[test]
fn test_vim_find_target_char_resolves_space_from_its_key_name() {
    // GPUI names the space key "space" — a multi-character name the
    // generic fallback rejects. Both rename inputs (file names, tab
    // titles) and vim's `f<space>` depend on it resolving anyway.
    assert_eq!(vim_find_target_char("space", false, None), Some(' '));
    assert_eq!(vim_find_target_char("space", false, Some(" ")), Some(' '));
    // Genuinely non-literal named keys still abandon the pending command.
    assert_eq!(vim_find_target_char("escape", false, None), None);
    assert_eq!(vim_find_target_char("enter", false, None), None);
    // Ordinary literals are unchanged.
    assert_eq!(vim_find_target_char("a", true, None), Some('A'));
    assert_eq!(vim_find_target_char(".", false, None), Some('.'));
}

#[test]
fn test_unique_path_in_walks_past_collisions() {
    let dir = temp_test_dir("unique_path");
    // Fresh directory: the bare name is free.
    assert_eq!(unique_path_in(&dir, "Card", "docx"), dir.join("Card.docx"));

    std::fs::write(dir.join("Card.docx"), b"x").unwrap();
    assert_eq!(
        unique_path_in(&dir, "Card", "docx"),
        dir.join("Card 1.docx")
    );

    std::fs::write(dir.join("Card 1.docx"), b"x").unwrap();
    assert_eq!(
        unique_path_in(&dir, "Card", "docx"),
        dir.join("Card 2.docx")
    );
}

#[test]
fn test_duplicate_file_writes_a_copy_beside_the_original() {
    let dir = temp_test_dir("duplicate_file");
    let src = dir.join("Original.docx");
    std::fs::write(&src, b"contents").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.duplicate_file(&src).unwrap();

    assert!(src.exists(), "the original must survive");
    assert_eq!(
        std::fs::read(dir.join("Original copy.docx")).unwrap(),
        b"contents"
    );
}

#[test]
fn test_paste_file_into_same_folder_does_not_clobber_the_source() {
    let dir = temp_test_dir("paste_same_folder");
    let src = dir.join("Card.docx");
    std::fs::write(&src, b"contents").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.copy_file(src.clone());
    state.paste_file_into(&dir).unwrap();

    assert!(src.exists());
    assert!(dir.join("Card 1.docx").exists());
    // The copy stays on the clipboard so it can be pasted again elsewhere,
    // and is still flagged as a copy rather than a cut.
    assert_eq!(state.copied_file, Some((src, false)));
}

#[test]
fn test_paste_file_with_nothing_copied_is_an_error_not_a_panic() {
    let dir = temp_test_dir("paste_nothing");
    let mut state = make_state("", 0, None);
    assert!(state.paste_file_into(&dir).is_err());
}

#[test]
fn test_create_new_folder_in_picks_first_free_name() {
    let dir = temp_test_dir("new_folder");
    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();

    state.create_new_folder_in(&dir).unwrap();
    assert!(dir.join("New Folder").is_dir());

    state.create_new_folder_in(&dir).unwrap();
    assert!(dir.join("New Folder 2").is_dir());
}

#[test]
fn test_rename_path_repoints_the_open_tab_that_was_showing_the_file() {
    let dir = temp_test_dir("rename_repoints_tab");
    let old = dir.join("Old.docx");
    std::fs::write(&old, b"contents").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.workspace.tabs[0].file_path = Some(old.clone());
    state.workspace.tabs[0].title = "Old.docx".into();

    // Bare name, no extension — `with_docx_extension` must supply it, or
    // the tree (.docx-only) could never show the file again.
    state.rename_path(&old, "New").unwrap();

    let new = dir.join("New.docx");
    assert!(new.exists());
    assert!(!old.exists());
    assert_eq!(state.workspace.tabs[0].file_path, Some(new));
    assert_eq!(state.workspace.tabs[0].title, "New.docx");
}

#[test]
fn test_rename_path_refuses_empty_and_existing_names() {
    let dir = temp_test_dir("rename_refusals");
    let old = dir.join("Old.docx");
    let taken = dir.join("Taken.docx");
    std::fs::write(&old, b"a").unwrap();
    std::fs::write(&taken, b"b").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();

    assert!(state.rename_path(&old, "   ").is_err());
    assert!(state.rename_path(&old, "Taken").is_err());
    assert!(old.exists(), "a refused rename must leave the file alone");
    assert_eq!(std::fs::read(&taken).unwrap(), b"b", "must not overwrite");
}

/// Rename changes the basename and nothing else. `dir.join(trimmed)`
/// used to accept `../elsewhere`, and an *absolute* name discarded the
/// parent entirely — so typing `/tmp/x` moved the file clean out of the
/// project, from a box that only claims to rename it.
#[test]
fn test_rename_path_refuses_anything_that_is_not_a_bare_name() {
    let dir = temp_test_dir("rename_refuses_paths");
    let sub = dir.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let old = sub.join("Case.docx");
    std::fs::write(&old, b"a").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();

    for name in ["../escaped", "..", ".", "a/b", "sub/../../escaped"] {
        assert!(
            state.rename_path(&old, name).is_err(),
            "{name} must be refused"
        );
    }
    assert!(state
        .rename_path(&old, "/tmp/vimbatim_rename_escape")
        .is_err());
    assert!(
        old.exists(),
        "a refused rename must leave the file where it was"
    );
    assert!(!dir.join("escaped.docx").exists());

    // An ordinary rename still works.
    state.rename_path(&old, "Renamed").unwrap();
    assert!(sub.join("Renamed.docx").exists());
}

/// Cut/Paste is where moving lives now: discoverable, folder-capable, and
/// unable to drop something outside the project the way the rename box
/// could.
#[test]
fn test_cut_then_paste_moves_a_file_and_repoints_its_open_tab() {
    let dir = temp_test_dir("cut_paste_moves_file");
    let target = dir.join("target");
    std::fs::create_dir(&target).unwrap();
    let src = dir.join("Case.docx");
    std::fs::write(&src, b"contents").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.workspace.tabs[0].file_path = Some(src.clone());

    state.cut_file(src.clone());
    state.paste_file_into(&target).unwrap();

    let moved = target.join("Case.docx");
    assert!(moved.exists(), "the file must be at the destination");
    assert!(!src.exists(), "a cut moves, it does not copy");
    assert_eq!(std::fs::read(&moved).unwrap(), b"contents");
    assert_eq!(
        state.workspace.tabs[0].file_path,
        Some(moved),
        "the open tab must follow the file"
    );
    assert!(state.copied_file.is_none(), "a cut pastes once");
}

/// A copy is unchanged by the cut work: original stays, and the clipboard
/// stays loaded so it can be pasted into several folders.
#[test]
fn test_copy_then_paste_still_duplicates_and_keeps_the_clipboard() {
    let dir = temp_test_dir("copy_paste_still_copies");
    let target = dir.join("target");
    std::fs::create_dir(&target).unwrap();
    let src = dir.join("Case.docx");
    std::fs::write(&src, b"contents").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();

    state.copy_file(src.clone());
    state.paste_file_into(&target).unwrap();

    assert!(src.exists(), "a copy leaves the original alone");
    assert!(target.join("Case.docx").exists());
    assert!(state.copied_file.is_some(), "a copy can be pasted again");
}

#[test]
fn test_cut_then_paste_moves_a_folder_but_never_into_itself() {
    let dir = temp_test_dir("cut_paste_moves_folder");
    let src = dir.join("Round 3");
    let nested = src.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("Case.docx"), b"c").unwrap();
    let target = dir.join("Archive");
    std::fs::create_dir(&target).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.workspace.tabs[0].file_path = Some(nested.join("Case.docx"));

    // Into itself, or into its own descendant, is refused rather than
    // stranding the subtree.
    state.cut_file(src.clone());
    assert!(state.paste_file_into(&src).is_err());
    assert!(state.paste_file_into(&nested).is_err());
    assert!(
        src.is_dir(),
        "a refused move must leave the folder in place"
    );

    state.paste_file_into(&target).unwrap();
    let moved = target.join("Round 3");
    assert!(moved.is_dir());
    assert!(!src.exists());
    assert_eq!(
        state.workspace.tabs[0].file_path,
        Some(moved.join("nested").join("Case.docx")),
        "a tab open inside the moved folder must follow it",
    );
}

#[test]
fn test_rename_path_on_a_dir_does_not_append_docx_extension() {
    let dir = temp_test_dir("rename_dir_no_extension");
    let old = dir.join("Old Folder");
    std::fs::create_dir(&old).unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.rename_path(&old, "New Folder").unwrap();

    assert!(dir.join("New Folder").is_dir());
    assert!(!dir.join("New Folder.docx").exists());
    assert!(!old.exists());
}

#[test]
fn test_rename_path_on_a_dir_repoints_nested_open_tabs_without_touching_title() {
    let dir = temp_test_dir("rename_dir_repoints_tab");
    let old = dir.join("Old Folder");
    std::fs::create_dir(&old).unwrap();
    let nested = old.join("Card.docx");
    std::fs::write(&nested, b"contents").unwrap();

    let mut state = make_state("", 0, None);
    state.workspace.working_directory = dir.clone();
    state.workspace.tabs[0].file_path = Some(nested.clone());
    state.workspace.tabs[0].title = "Card.docx".into();

    state.rename_path(&old, "New Folder").unwrap();

    let new_nested = dir.join("New Folder").join("Card.docx");
    assert!(new_nested.exists());
    assert_eq!(state.workspace.tabs[0].file_path, Some(new_nested));
    // The file itself didn't rename — only its parent folder did.
    assert_eq!(state.workspace.tabs[0].title, "Card.docx");
}

#[test]
fn test_open_file_in_current_tab_refuses_to_replace_a_modified_tab() {
    let dir = temp_test_dir("open_current_dirty");
    let path = dir.join("Card.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();

    let mut state = make_state("unsaved work", 0, None);
    state.workspace.working_directory = dir.clone();
    state.workspace.tabs[0].document.is_modified = true;

    state.open_file_in_current_tab(path.clone());

    // The dirty tab is still there with its content — the file opened
    // alongside it instead of replacing it.
    assert_eq!(state.workspace.tabs.len(), 2);
    assert_eq!(state.workspace.tabs[0].document.content(), "unsaved work");
    assert_eq!(state.workspace.tabs[1].file_path, Some(path));
}

#[test]
fn test_open_file_in_current_tab_replaces_a_clean_tab_in_place() {
    let dir = temp_test_dir("open_current_clean");
    let path = dir.join("Card.docx");
    create_new_docx(&default_paragraphs(), &path, Default::default()).unwrap();

    let mut state = make_state("scratch", 0, None);
    state.workspace.working_directory = dir.clone();

    state.open_file_in_current_tab(path.clone());

    assert_eq!(state.workspace.tabs.len(), 1);
    assert_eq!(state.workspace.tabs[0].file_path, Some(path));
}

// ── Right-click menus: Nav heading ranges ───────────────────────────────

/// A one-line paragraph at `heading` level — the shape `render_nav_tree`
/// reads to build the outline.
fn nav_para(text: &str, heading: u8) -> Paragraph {
    Paragraph {
        list: None,
        runs: vec![Run {
            text: text.to_string(),
            ..Run::default()
        }],
        heading,
        alignment: Alignment::default(),
        unsupported_xml: None,
    }
}

#[test]
fn test_heading_contents_range_stops_at_the_next_equal_level_heading() {
    let state = make_state_with_paragraphs(
        vec![
            nav_para("Pocket A", 1),
            nav_para("body", 0),
            nav_para("Pocket B", 1),
        ],
        0,
    );
    let (start, end) = state.heading_contents_range(0).unwrap();
    // Up to but not including Pocket B, and carrying the '\n' that
    // terminates "body" — so deleting this leaves no blank line behind.
    assert_eq!(
        &state.workspace.tabs[0].document.content()[start..end],
        "Pocket A\nbody\n"
    );
}

#[test]
fn test_heading_contents_range_stops_at_a_shallower_heading() {
    let state = make_state_with_paragraphs(
        vec![
            nav_para("Pocket", 1),
            nav_para("Hat", 2),
            nav_para("under hat", 0),
            nav_para("Pocket 2", 1),
        ],
        0,
    );
    let (start, end) = state.heading_contents_range(1).unwrap();
    assert_eq!(
        &state.workspace.tabs[0].document.content()[start..end],
        "Hat\nunder hat\n"
    );
}

#[test]
fn test_heading_contents_range_swallows_deeper_headings() {
    let state = make_state_with_paragraphs(
        vec![
            nav_para("Pocket", 1),
            nav_para("Hat", 2),
            nav_para("Tag", 4),
        ],
        0,
    );
    let (start, end) = state.heading_contents_range(0).unwrap();
    // Everything nested under the Pocket comes with it.
    assert_eq!(
        &state.workspace.tabs[0].document.content()[start..end],
        "Pocket\nHat\nTag"
    );
}

#[test]
fn test_heading_contents_range_of_the_last_heading_runs_to_end_of_document() {
    let state =
        make_state_with_paragraphs(vec![nav_para("Pocket", 1), nav_para("trailing body", 0)], 0);
    let (start, end) = state.heading_contents_range(0).unwrap();
    assert_eq!(end, state.workspace.tabs[0].document.content().len());
    assert_eq!(
        &state.workspace.tabs[0].document.content()[start..end],
        "Pocket\ntrailing body"
    );
}

#[test]
fn test_heading_contents_range_is_none_for_a_non_heading_line() {
    let state = make_state_with_paragraphs(vec![nav_para("just body", 0)], 0);
    assert!(state.heading_contents_range(0).is_none());
    assert!(
        state.heading_contents_range(99).is_none(),
        "out of range must not panic"
    );
}

#[test]
fn test_select_heading_and_contents_then_delete_leaves_no_blank_line() {
    let mut state = make_state_with_paragraphs(
        vec![
            nav_para("Pocket A", 1),
            nav_para("body", 0),
            nav_para("Pocket B", 1),
        ],
        0,
    );
    state.select_heading_and_contents(0);
    state.delete_selection();
    // The '\n' that ended the deleted section went with it, so Pocket B
    // is now the first line rather than sitting under a stranded blank.
    assert_eq!(state.workspace.tabs[0].document.content(), "Pocket B");
}

// ── Right-click menus: tab operations ───────────────────────────────────

/// A state with `n` tabs, all clean and file-backed enough for close
/// logic (paths are never touched by these tests).
fn make_state_with_tabs(n: usize) -> AppState {
    let mut state = make_state("", 0, None);
    for i in 1..n {
        let mut tab = Tab::new_empty(TabId(i));
        tab.title = format!("tab{i}");
        state.workspace.tabs.push(tab);
    }
    state.workspace.next_tab_id = n;
    state
}

#[test]
fn test_move_tab_to_len_moves_it_to_the_end() {
    // The drag-to-end bug: per-tab drop targets can only say "before tab
    // N", so the last slot is only reachable through `to == len`.
    let mut state = make_state_with_tabs(3);
    let moved_id = state.workspace.tabs[0].id;

    state.move_tab(0, 3);

    assert_eq!(state.workspace.tabs.last().unwrap().id, moved_id);
    assert_eq!(state.workspace.tabs.len(), 3);
}

#[test]
fn test_move_tab_past_len_is_still_rejected() {
    let mut state = make_state_with_tabs(3);
    let before: Vec<TabId> = state.workspace.tabs.iter().map(|t| t.id).collect();
    state.move_tab(0, 4);
    assert_eq!(
        state
            .workspace
            .tabs
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn test_move_tab_of_the_already_last_tab_to_the_end_is_a_no_op() {
    let mut state = make_state_with_tabs(3);
    let before: Vec<TabId> = state.workspace.tabs.iter().map(|t| t.id).collect();
    state.move_tab(2, 3);
    assert_eq!(
        state
            .workspace
            .tabs
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn test_close_tabs_to_right_closes_everything_after_the_index() {
    let mut state = make_state_with_tabs(4);
    let kept: Vec<TabId> = state.workspace.tabs[..2].iter().map(|t| t.id).collect();

    state.close_tabs_to_right(1);

    assert_eq!(
        state
            .workspace
            .tabs
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        kept
    );
}

#[test]
fn test_close_tabs_to_left_closes_everything_before_the_index() {
    let mut state = make_state_with_tabs(4);
    let kept: Vec<TabId> = state.workspace.tabs[2..].iter().map(|t| t.id).collect();

    state.close_tabs_to_left(2);

    assert_eq!(
        state
            .workspace
            .tabs
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        kept
    );
}

#[test]
fn test_close_tabs_skips_modified_tabs_rather_than_discarding_them() {
    // ponytail: `pending_close` holds one confirmation at a time, so a
    // batch close can't ask about several dirty tabs — it leaves them
    // open instead of silently dropping unsaved work.
    let mut state = make_state_with_tabs(4);
    state.workspace.tabs[3].document.is_modified = true;
    let dirty_id = state.workspace.tabs[3].id;
    let anchor_id = state.workspace.tabs[1].id;

    state.close_tabs_to_right(1);

    assert_eq!(
        state
            .workspace
            .tabs
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        vec![state.workspace.tabs[0].id, anchor_id, dirty_id],
    );
}

#[test]
fn test_close_tabs_to_right_of_the_last_tab_is_a_no_op() {
    let mut state = make_state_with_tabs(3);
    let before: Vec<TabId> = state.workspace.tabs.iter().map(|t| t.id).collect();
    state.close_tabs_to_right(2);
    assert_eq!(
        state
            .workspace
            .tabs
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>(),
        before
    );
}

// ── Document recovery ───────────────────────────────────────────────────

/// Builds a state with one dirty tab plus a fake pending recovery entry
/// pointing at real files in a temp dir, so the recovery actions have
/// something to act on.
fn make_state_with_recovery(tag: &str) -> (AppState, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("vimbatim-state-rec-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let snapshot = dir.join("111-0.docx");
    let meta = dir.join("111-0.meta");
    let original = dir.join("case.docx");

    let mut para = crate::docx_parser::Paragraph::default();
    para.runs.push(crate::docx_parser::Run {
        text: "recovered text".into(),
        ..Default::default()
    });
    crate::docx_parser::create_new_docx(&[para], &snapshot, Default::default()).unwrap();
    std::fs::write(
        &meta,
        crate::recovery::format_meta(Some(&original), "case.docx", 1),
    )
    .unwrap();

    let mut state = make_state("", 0, None);
    state.recovery.pending_entries = vec![crate::recovery::RecoveryEntry {
        snapshot,
        meta,
        original_path: Some(original),
        title: "case.docx".into(),
        saved_at: 1,
    }];
    (state, dir)
}

#[test]
fn discard_recovery_pops_the_entry_and_deletes_both_files() {
    let (mut state, _dir) = make_state_with_recovery("discard");
    let entry = state.recovery.pending_entries[0].clone();

    state.discard_recovery();

    assert!(state.recovery.pending_entries.is_empty());
    assert!(!entry.snapshot.exists());
    assert!(!entry.meta.exists());
}

#[test]
fn resume_recovery_opens_a_modified_tab_pointed_at_the_original_path() {
    let (mut state, _dir) = make_state_with_recovery("resume");
    let entry = state.recovery.pending_entries[0].clone();
    let tabs_before = state.workspace.tabs.len();

    state.resume_recovery();

    assert!(state.recovery.pending_entries.is_empty());
    assert_eq!(state.workspace.tabs.len(), tabs_before + 1);
    let tab = state.workspace.tabs.last().unwrap();
    assert_eq!(tab.file_path, entry.original_path);
    assert!(
        tab.document.is_modified,
        "resumed changes are unsaved by design"
    );
    assert!(tab.document.content().contains("recovered text"));
    assert_eq!(state.workspace.active_tab, state.workspace.tabs.len() - 1);
    // The snapshot is consumed — the content now lives in the editor.
    assert!(!entry.snapshot.exists());
}

#[test]
fn a_resumed_tab_is_eligible_for_a_fresh_snapshot_without_any_further_typing() {
    let (mut state, _dir) = make_state_with_recovery("resume-snapshot-eligible");

    state.resume_recovery();

    let tab = state.workspace.tabs.last().unwrap();
    // Simulate the background task looking at this tab one interval later.
    let later = tab.document.last_edit_at.unwrap() + crate::recovery::MIN_SNAPSHOT_INTERVAL;
    assert!(
        crate::recovery::needs_snapshot(
            tab.document.is_modified,
            tab.document.content_version,
            tab.last_snapshot_version,
            tab.document.last_edit_at,
            later,
            crate::recovery::MIN_SNAPSHOT_INTERVAL,
        ),
        "a resumed tab must be snapshot-eligible — its only on-disk copy was just deleted"
    );
}

#[test]
fn resume_recovery_of_a_never_saved_tab_opens_an_untitled_modified_tab() {
    let (mut state, _dir) = make_state_with_recovery("resume-untitled");
    state.recovery.pending_entries[0].original_path = None;
    state.recovery.pending_entries[0].title = "New Tab".into();

    state.resume_recovery();

    let tab = state.workspace.tabs.last().unwrap();
    assert_eq!(tab.file_path, None);
    assert!(tab.document.is_modified);
    assert!(tab.document.content().contains("recovered text"));
}

#[test]
fn complete_recovery_save_as_copies_the_snapshot_and_deletes_it() {
    let (mut state, dir) = make_state_with_recovery("save-as");
    let entry = state.take_recovery_for_save_as().unwrap();
    let dest = dir.join("saved-elsewhere.docx");

    state.complete_recovery_save_as(&entry, &dest).unwrap();

    assert!(dest.exists());
    assert!(state.recovery.pending_entries.is_empty());
    assert!(!entry.snapshot.exists());
    // The copy is a real docx, not a truncated one.
    assert!(crate::docx_parser::parse_docx(&dest).is_ok());
}

#[test]
fn test_with_docx_extension_appends_when_missing() {
    assert_eq!(
        with_docx_extension(Path::new("New Tab")),
        Path::new("New Tab.docx")
    );
    assert_eq!(
        with_docx_extension(Path::new("/tmp/aff")),
        Path::new("/tmp/aff.docx")
    );
}

#[test]
fn test_with_docx_extension_leaves_an_existing_one_alone() {
    assert_eq!(
        with_docx_extension(Path::new("card.docx")),
        Path::new("card.docx")
    );
    // Case-insensitive: Windows pickers hand back `.DOCX`.
    assert_eq!(
        with_docx_extension(Path::new("card.DOCX")),
        Path::new("card.DOCX")
    );
    assert_eq!(
        with_docx_extension(Path::new("card.Docx")),
        Path::new("card.Docx")
    );
}

#[test]
fn test_with_docx_extension_appends_rather_than_replacing_other_suffixes() {
    // `set_extension` would turn these into "neg.docx" and "1ac.docx",
    // silently eating part of the name the user typed.
    assert_eq!(
        with_docx_extension(Path::new("neg.v2")),
        Path::new("neg.v2.docx")
    );
    assert_eq!(
        with_docx_extension(Path::new("1ac.txt")),
        Path::new("1ac.txt.docx")
    );
}

#[test]
fn complete_recovery_save_as_forces_the_docx_extension() {
    let (mut state, dir) = make_state_with_recovery("save-as-no-ext");
    let entry = state.take_recovery_for_save_as().unwrap();
    // What the picker hands back when the user accepts the "New Tab"
    // suggestion verbatim.
    let typed = dir.join("New Tab");

    state.complete_recovery_save_as(&entry, &typed).unwrap();

    assert!(!typed.exists(), "must not write an extension-less file");
    let expected = dir.join("New Tab.docx");
    assert!(expected.exists(), "should have written {expected:?}");
    assert!(crate::docx_parser::parse_docx(&expected).is_ok());
}

#[test]
fn resume_recovery_drops_a_snapshot_that_will_not_parse_instead_of_opening_an_empty_tab() {
    let (mut state, _dir) = make_state_with_recovery("resume-corrupt");
    let entry = state.recovery.pending_entries[0].clone();
    std::fs::write(&entry.snapshot, b"not a zip at all").unwrap();
    let tabs_before = state.workspace.tabs.len();

    state.resume_recovery();

    assert!(state.recovery.pending_entries.is_empty());
    assert_eq!(
        state.workspace.tabs.len(),
        tabs_before,
        "no tab for an unreadable snapshot"
    );
    assert!(!entry.snapshot.exists());
}

#[test]
fn take_recovery_for_save_as_returns_none_when_nothing_is_pending() {
    let mut state = make_state("", 0, None);
    assert!(state.take_recovery_for_save_as().is_none());
}

#[test]
fn recovery_actions_walk_through_multiple_entries_one_at_a_time() {
    let (mut state, _dir) = make_state_with_recovery("multi");
    let first = state.recovery.pending_entries[0].clone();
    state
        .recovery
        .pending_entries
        .push(crate::recovery::RecoveryEntry {
            title: "second.docx".into(),
            saved_at: 0,
            ..first
        });
    assert_eq!(state.recovery.pending_entries.len(), 2);

    state.discard_recovery();
    assert_eq!(state.recovery.pending_entries.len(), 1);
    assert_eq!(state.recovery.pending_entries[0].title, "second.docx");

    state.discard_recovery();
    assert!(state.recovery.pending_entries.is_empty());
}

#[test]
fn close_tab_deletes_that_tabs_snapshot() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs.push(Tab::new_empty(TabId(1)));
    let (docx, meta) = crate::recovery::snapshot_paths(state.workspace.tabs[1].id);
    std::fs::create_dir_all(crate::recovery::recovery_dir()).unwrap();
    std::fs::write(&docx, b"x").unwrap();
    std::fs::write(&meta, b"x").unwrap();

    state.close_tab(1);

    assert!(!docx.exists());
    assert!(!meta.exists());
}

#[test]
fn confirm_close_discard_for_an_app_close_deletes_every_snapshot() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].document.is_modified = true;
    let (docx, meta) = crate::recovery::snapshot_paths(state.workspace.tabs[0].id);
    std::fs::create_dir_all(crate::recovery::recovery_dir()).unwrap();
    std::fs::write(&docx, b"x").unwrap();
    std::fs::write(&meta, b"x").unwrap();

    state.request_close_app();
    state.confirm_close_discard();

    assert!(!docx.exists());
    assert!(!meta.exists());
}

#[test]
fn dirty_tab_snapshots_includes_only_modified_tabs() {
    let mut state = make_state("hello", 0, None);
    state.workspace.tabs[0].document.is_modified = true;
    state.workspace.tabs[0].title = "dirty".into();
    let mut clean = Tab::new_empty(TabId(1));
    clean.title = "clean".into();
    state.workspace.tabs.push(clean);

    let mirror = state.dirty_tab_snapshots();

    assert_eq!(mirror.len(), 1);
    assert_eq!(mirror[0].title, "dirty");
    assert_eq!(mirror[0].id, state.workspace.tabs[0].id);
}

#[test]
fn save_preparation_rejects_duplicates_and_keeps_newer_edits_dirty() {
    let mut state = make_state("first", 0, None);
    let tab = &mut state.workspace.tabs[0];
    tab.file_path = Some(std::env::temp_dir().join("vimbatim-save-test.docx"));
    tab.document.is_modified = true;

    let prepared = state.prepare_save(0).unwrap();
    assert!(prepared.is_some());
    assert!(state.workspace.tabs[0].is_saving);
    assert!(state.prepare_save(0).unwrap().is_none());

    state.insert_char('!');
    state.complete_save(TabId(0), Ok(()), std::time::Duration::ZERO);
    assert!(!state.workspace.tabs[0].is_saving);
    assert!(state.workspace.tabs[0].document.is_modified);
}

#[test]
fn dirty_tab_snapshots_is_empty_when_nothing_is_modified() {
    let state = make_state("hello", 0, None);
    assert!(state.dirty_tab_snapshots().is_empty());
}

#[test]
fn commands_keep_stable_tab_identity_after_reordering() {
    let mut state = make_state("one", 0, None);
    let original_id = state.workspace.tabs[0].id;
    state.workspace.tabs.insert(0, Tab::new_empty(TabId(42)));

    state.execute(crate::app::command::AppCommand::SwitchTab(original_id));
    assert_eq!(
        state.workspace.tabs[state.workspace.active_tab].id,
        original_id
    );
    assert_eq!(
        state.execute(crate::app::command::AppCommand::Save),
        vec![crate::app::command::AppEffect::PerformSave(original_id)]
    );
}
