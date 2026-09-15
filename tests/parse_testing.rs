use std::path::Path;
use vimbatim::testing::Preferences;

const FIXTURE: &str = "tests/fixtures/settings.conf";

#[test]
fn preferences_loads_the_packaged_settings_fixture() {
    let preferences = Preferences::load(Path::new(FIXTURE)).unwrap();
    assert_eq!(preferences.highlight_color, "yellow");
    assert_eq!(preferences.small_size_half_points, 12);
    assert!(preferences.paragraph_integrity);
    assert!(!preferences.pilcrows);
}

#[test]
fn preferences_uses_defaults_for_absent_values() {
    let preferences = Preferences::load(Path::new(FIXTURE)).unwrap();
    assert_eq!(preferences.normal_text_size_half_points, 22);
    assert_eq!(preferences.line_spacing, 1.0);
}
