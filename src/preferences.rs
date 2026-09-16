use crate::theme::{ThemeColorMode, ThemeKind, ThemeMode};
use std::{collections::HashMap, io, path::Path};

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub paragraph_integrity: bool,
    pub pilcrows: bool,
    pub theme: ThemeKind,
    pub theme_mode: ThemeMode,
    pub theme_color_mode: ThemeColorMode,
    pub highlight_color: String,
    pub analytic_color: String,
    pub spellcheck_enabled: bool,
    pub spellcheck_underline_color: String,
    pub line_spacing: f32,
    pub spreading_wpm: u32,
    pub normal_text_size_half_points: u16,
    pub small_size_half_points: u16,
    pub pocket_size_half_points: u16,
    pub hat_size_half_points: u16,
    pub block_size_half_points: u16,
    pub tag_size_half_points: u16,
    pub cite_size_half_points: u16,
    pub emphasis_size_half_points: u16,
    pub emphasis_bold: bool,
    pub emphasis_underline: bool,
    pub emphasis_box: bool,
    pub emphasis_change_size: bool,
    pub paste_condense: bool,
    pub paste_condense_pilcrow: bool,
    pub standardize_highlight_exception: String,
    pub custom_font_colors: Vec<u32>,
    pub custom_highlight_colors: Vec<u32>,
    pub nav_fold_buttons: bool,
    pub search_from_list_enabled: bool,
    pub search_list_whole_words: bool,
    pub command_palette_enabled: bool,
    pub working_directory: Option<std::path::PathBuf>,
    pub expanded_dirs: Vec<std::path::PathBuf>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            paragraph_integrity: false,
            pilcrows: false,
            theme: ThemeKind::WorkbenchDark,
            theme_mode: ThemeMode::Dark,
            theme_color_mode: ThemeColorMode::Minimal,
            highlight_color: "yellow".into(),
            analytic_color: "0000ff".into(),
            spellcheck_enabled: true,
            spellcheck_underline_color: "red".into(),
            line_spacing: 1.0,
            spreading_wpm: 300,
            normal_text_size_half_points: 22,
            small_size_half_points: 12,
            pocket_size_half_points: 52,
            hat_size_half_points: 44,
            block_size_half_points: 32,
            tag_size_half_points: 26,
            cite_size_half_points: 26,
            emphasis_size_half_points: 24,
            emphasis_bold: true,
            emphasis_underline: false,
            emphasis_box: false,
            emphasis_change_size: false,
            paste_condense: false,
            paste_condense_pilcrow: false,
            standardize_highlight_exception: String::new(),
            custom_font_colors: Vec::new(),
            custom_highlight_colors: Vec::new(),
            nav_fold_buttons: false,
            search_from_list_enabled: false,
            search_list_whole_words: true,
            command_palette_enabled: false,
            working_directory: None,
            expanded_dirs: Vec::new(),
        }
    }
}

impl Preferences {
    pub fn load(path: &Path) -> Result<Self, io::Error> {
        let contents = std::fs::read_to_string(path)?;
        let values: HashMap<_, _> = contents
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim(), value.trim()))
            .collect();
        let mut p = Self::default();
        let bool = |key, default| values.get(key).map_or(default, |v| *v == "true");
        let string = |key: &str, default: &str| {
            values
                .get(key)
                .filter(|v| !v.is_empty())
                .unwrap_or(&default)
                .to_string()
        };
        p.paragraph_integrity = bool("paragraph_integrity", p.paragraph_integrity);
        p.pilcrows = bool("pilcrows", p.pilcrows);
        p.theme = values
            .get("theme")
            .map_or(p.theme, |v| ThemeKind::from_conf_value(v));
        p.theme_mode = values
            .get("theme_mode")
            .map_or(p.theme_mode, |v| ThemeMode::from_conf_value(v));
        p.theme_color_mode = values
            .get("theme_color_mode")
            .map_or(p.theme_color_mode, |v| ThemeColorMode::from_conf_value(v));
        p.highlight_color = string("highlight_color", &p.highlight_color);
        p.analytic_color = string("analytic_color", &p.analytic_color);
        p.spellcheck_enabled = bool("spellcheck", p.spellcheck_enabled);
        p.spellcheck_underline_color =
            string("spellcheck_underline_color", &p.spellcheck_underline_color);
        p.line_spacing = values
            .get("line_spacing")
            .and_then(|v| v.parse().ok())
            .filter(|v: &f32| v.is_finite())
            .map_or(1.0, |v| v.clamp(0.5, 3.0));
        p.spreading_wpm = values
            .get("spreading_wpm")
            .and_then(|v| v.parse().ok())
            .map_or(300, |v: u32| v.clamp(50, 1000));
        let size = |key, default| {
            values
                .get(key)
                .and_then(|v| v.parse::<u16>().ok())
                .map_or(default, |v| v.clamp(4, 48) * 2)
        };
        p.normal_text_size_half_points = size("normal_text_size", 22);
        p.small_size_half_points = size("small_size", 12);
        p.pocket_size_half_points = size("pocket_size", 52);
        p.hat_size_half_points = size("hat_size", 44);
        p.block_size_half_points = size("block_size", 32);
        p.tag_size_half_points = size("tag_size", 26);
        p.cite_size_half_points = size("cite_size", 26);
        p.emphasis_size_half_points = size("emphasis_size", 24);
        p.emphasis_bold = bool("emphasis_bold", p.emphasis_bold);
        p.emphasis_underline = bool("emphasis_underline", p.emphasis_underline);
        p.emphasis_box = bool("emphasis_box", p.emphasis_box);
        p.emphasis_change_size = bool("emphasis_change_size", p.emphasis_change_size);
        p.paste_condense = bool("paste_condense", p.paste_condense);
        p.paste_condense_pilcrow = bool("paste_condense_pilcrow", p.paste_condense_pilcrow);
        p.standardize_highlight_exception = string("standardize_highlight_exception", "");
        let colors = |key: &str| {
            values
                .get(key)
                .map(|v| {
                    v.split('|')
                        .map(str::trim)
                        .filter(|c| c.len() == 6)
                        .filter_map(|c| u32::from_str_radix(c, 16).ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        p.custom_font_colors = colors("custom_font_colors");
        p.custom_highlight_colors = colors("custom_highlight_colors");
        p.nav_fold_buttons = bool("nav_fold_buttons", p.nav_fold_buttons);
        p.search_from_list_enabled = bool("search_from_list", p.search_from_list_enabled);
        p.search_list_whole_words = bool("search_list_whole_words", p.search_list_whole_words);
        p.command_palette_enabled = bool("command_palette", p.command_palette_enabled);
        p.working_directory = values
            .get("working_directory")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from);
        p.expanded_dirs = values
            .get("expanded_dirs")
            .map(|v| {
                v.split('|')
                    .filter(|v| !v.is_empty())
                    .map(std::path::PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        Ok(p)
    }

    pub fn update(path: &Path, key: &str, value: &str) -> io::Result<()> {
        crate::theme::save_setting_line(path, key, value)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        for (key, value) in [
            ("paragraph_integrity", self.paragraph_integrity.to_string()),
            ("pilcrows", self.pilcrows.to_string()),
            ("theme", self.theme.conf_value().into()),
            ("theme_mode", self.theme_mode.conf_value().into()),
            (
                "theme_color_mode",
                self.theme_color_mode.conf_value().into(),
            ),
            ("highlight_color", self.highlight_color.clone()),
            ("analytic_color", self.analytic_color.clone()),
            ("spellcheck", self.spellcheck_enabled.to_string()),
            (
                "spellcheck_underline_color",
                self.spellcheck_underline_color.clone(),
            ),
            ("line_spacing", self.line_spacing.to_string()),
            ("spreading_wpm", self.spreading_wpm.to_string()),
        ] {
            Self::update(path, key, &value)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shipped_defaults_are_loaded() {
        let p = Preferences::load(Path::new("default_settings.conf")).unwrap();
        assert!(p.paragraph_integrity);
        assert!(!p.pilcrows);
        assert_eq!(p.line_spacing, 1.0);
        assert_eq!(p.normal_text_size_half_points, 22);
        assert_eq!(p.pocket_size_half_points, 52);
        assert_eq!(p.highlight_color, "yellow");
        assert!(p.spellcheck_enabled);
        assert!(p.emphasis_bold);
        assert!(!p.paste_condense);
        assert!(p.custom_font_colors.is_empty());
        assert!(p.search_list_whole_words);
    }
}
