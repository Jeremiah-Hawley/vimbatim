use vimbatim::config_parsing;

// These tests read a fixture (tests/fixtures/settings.conf), not the real
// project-root settings.conf. That file is a live, user-editable runtime
// file now that keybinds are configurable through the settings modal —
// every value here (including `vim`) can legitimately change the moment
// someone remaps a key or toggles vim mode, which would otherwise make
// these tests fail for reasons that have nothing to do with a real bug.
const FIXTURE: &str = "tests/fixtures/settings.conf";

#[test]
fn test_formatting_section() {
    let s = config_parsing::parse(FIXTURE);
    assert_eq!(s.highlight_color, "yellow");
    assert_eq!(s.small_size, 6);
    assert_eq!(s.large_size, 11);
    assert_eq!(s.paragraph_integrity, true);
    assert_eq!(s.pilcrows, false);
}

#[test]
fn test_keybinds_section() {
    let s = config_parsing::parse(FIXTURE);
    assert_eq!(s.vim, false);
    assert_eq!(s.paste, "f2");
    assert_eq!(s.condense, "f3");
    assert_eq!(s.pocket_hotkey, "f4");
    assert_eq!(s.hat, "f5");
    assert_eq!(s.block, "f6");
    assert_eq!(s.tag, "f7");
    assert_eq!(s.cite, "f8");
    assert_eq!(s.underline, "CTRL u");
    assert_eq!(s.emphasis, "f10");
    assert_eq!(s.highlight, "f11");
    assert_eq!(s.clear, "f12");
    assert_eq!(s.delete_tags, "ALT f7");
    assert_eq!(s.new_document, "CTRL n");
    assert_eq!(s.start_timer, "CTRL SHFT t");
    assert_eq!(s.open_stats, "CTRL SHFT i");
    assert_eq!(s.shrink, "ALT f3");
    assert_eq!(s.cite_from_link, "CTRL f8");
    assert_eq!(s.wikifi, "CTRL SHFT ALT w");
}

#[test]
fn test_parsing_dict() {
    let map = vimbatim::config_parsing::parsing_dict(FIXTURE);
    assert_eq!(map.get("highlight_color").map(String::as_str), Some("yellow"));
    assert_eq!(map.get("small_size").map(String::as_str),      Some("6"));
    assert_eq!(map.get("large_size").map(String::as_str),      Some("11"));
    assert_eq!(map.get("paragraph_integrity").map(String::as_str), Some("true"));
    assert_eq!(map.get("pilcrows").map(String::as_str),        Some("false"));
    assert_eq!(map.get("vim").map(String::as_str),             Some("false"));
    assert_eq!(map.get("paste").map(String::as_str),           Some("f2"));
    assert_eq!(map.get("wikifi").map(String::as_str),          Some("CTRL SHFT ALT w"));
}

#[test]
fn test_defaults_before_parse() {
    let s = vimbatim::config_parsing::Settings::new();
    assert_eq!(s.highlight_color, "");
    assert_eq!(s.small_size, 0);
    assert_eq!(s.large_size, 0);
    assert_eq!(s.vim, false);
}
