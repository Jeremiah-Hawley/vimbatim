//! Visual tokens for the Vimbatim GPUI chrome.
//!
//! The first theme is a dark, Word-aware document workbench: compact,
//! precise, and quiet enough that the document remains the primary surface.
//! Values are kept as named hex/spacing tokens so later settings-backed
//! themes can swap palettes without rewriting each view. When theme switching
//! lands, keep this module's names as the stable semantic contract and add
//! palette modules behind it (for example `workbench_dark`, `classic_light`,
//! or `terminal_dark`) rather than reading raw colors in each component.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    WorkbenchDark,
    CatppuccinMocha,
    TokyoNight,
    GruvboxDark,
    Nord,
    EverforestDark,
    RosePine,
    Kanagawa,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeColorMode {
    Minimal,
    Vivid,
}

impl ThemeColorMode {
    pub fn all() -> &'static [ThemeColorMode] {
        &[ThemeColorMode::Minimal, ThemeColorMode::Vivid]
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeColorMode::Minimal => "Minimal",
            ThemeColorMode::Vivid => "Vivid",
        }
    }

    pub fn conf_value(self) -> &'static str {
        match self {
            ThemeColorMode::Minimal => "minimal",
            ThemeColorMode::Vivid => "vivid",
        }
    }

    pub fn from_conf_value(value: &str) -> ThemeColorMode {
        match value.trim().to_ascii_lowercase().as_str() {
            "vivid" | "colorful" | "highlight" => ThemeColorMode::Vivid,
            _ => ThemeColorMode::Minimal,
        }
    }
}

impl ThemeKind {
    pub fn all() -> &'static [ThemeKind] {
        &[
            ThemeKind::WorkbenchDark,
            ThemeKind::CatppuccinMocha,
            ThemeKind::TokyoNight,
            ThemeKind::GruvboxDark,
            ThemeKind::Nord,
            ThemeKind::EverforestDark,
            ThemeKind::RosePine,
            ThemeKind::Kanagawa,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::WorkbenchDark => "Workbench Dark",
            ThemeKind::CatppuccinMocha => "Catppuccin Mocha",
            ThemeKind::TokyoNight => "Tokyo Night",
            ThemeKind::GruvboxDark => "Gruvbox Dark",
            ThemeKind::Nord => "Nord",
            ThemeKind::EverforestDark => "Everforest Dark",
            ThemeKind::RosePine => "Rose Pine",
            ThemeKind::Kanagawa => "Kanagawa",
        }
    }

    pub fn conf_value(self) -> &'static str {
        match self {
            ThemeKind::WorkbenchDark => "workbench-dark",
            ThemeKind::CatppuccinMocha => "catppuccin-mocha",
            ThemeKind::TokyoNight => "tokyo-night",
            ThemeKind::GruvboxDark => "gruvbox-dark",
            ThemeKind::Nord => "nord",
            ThemeKind::EverforestDark => "everforest-dark",
            ThemeKind::RosePine => "rose-pine",
            ThemeKind::Kanagawa => "kanagawa",
        }
    }

    pub fn from_conf_value(value: &str) -> ThemeKind {
        match value.trim().to_ascii_lowercase().as_str() {
            "catppuccin-mocha" | "catppuccin" => ThemeKind::CatppuccinMocha,
            "tokyo-night" | "tokyonight" => ThemeKind::TokyoNight,
            "gruvbox-dark" | "gruvbox" => ThemeKind::GruvboxDark,
            "nord" => ThemeKind::Nord,
            "everforest-dark" | "everforest" => ThemeKind::EverforestDark,
            "rose-pine" | "rosepine" => ThemeKind::RosePine,
            "kanagawa" => ThemeKind::Kanagawa,
            _ => ThemeKind::WorkbenchDark,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub app_bg: u32,
    pub editor_bg: u32,
    pub editor_bg_raised: u32,
    pub chrome: u32,
    pub chrome_elevated: u32,
    pub chrome_hover: u32,
    pub chrome_active: u32,
    pub sidebar: u32,
    pub border: u32,
    pub border_subtle: u32,
    pub text: u32,
    pub text_muted: u32,
    pub text_faint: u32,
    pub accent: u32,
    pub accent_strong: u32,
    pub accent_muted: u32,
    pub accent_wash: u32,
    pub accent_alt: u32,
    pub highlight: u32,
    pub selection: u32,
}

pub const fn palette(kind: ThemeKind) -> Palette {
    match kind {
        ThemeKind::WorkbenchDark => Palette {
            app_bg: color::APP_BG,
            editor_bg: color::EDITOR_BG,
            editor_bg_raised: color::EDITOR_BG_RAISED,
            chrome: color::CHROME,
            chrome_elevated: color::CHROME_ELEVATED,
            chrome_hover: color::CHROME_HOVER,
            chrome_active: color::CHROME_ACTIVE,
            sidebar: color::SIDEBAR,
            border: color::BORDER,
            border_subtle: color::BORDER_SUBTLE,
            text: color::TEXT,
            text_muted: color::TEXT_MUTED,
            text_faint: color::TEXT_FAINT,
            accent: color::ACCENT,
            accent_strong: color::ACCENT_STRONG,
            accent_muted: color::ACCENT_MUTED,
            accent_wash: color::ACCENT_WASH,
            accent_alt: color::ACCENT_ALT,
            highlight: color::HIGHLIGHT,
            selection: color::SELECTION,
        },
        ThemeKind::CatppuccinMocha => Palette {
            app_bg: 0x11111b,
            editor_bg: 0x1e1e2e,
            editor_bg_raised: 0x242437,
            chrome: 0x181825,
            chrome_elevated: 0x313244,
            chrome_hover: 0x45475a,
            chrome_active: 0x1e1e2e,
            sidebar: 0x181825,
            border: 0x45475a,
            border_subtle: 0x313244,
            text: 0xcdd6f4,
            text_muted: 0xa6adc8,
            text_faint: 0x6c7086,
            accent: 0x89b4fa,
            accent_strong: 0x74c7ec,
            accent_muted: 0x45475a,
            accent_wash: 0x27324d,
            accent_alt: 0xf5c2e7,
            highlight: 0xf9e2af,
            selection: 0x313f5f,
        },
        ThemeKind::TokyoNight => Palette {
            app_bg: 0x16161e,
            editor_bg: 0x1a1b26,
            editor_bg_raised: 0x1f2335,
            chrome: 0x1f2335,
            chrome_elevated: 0x292e42,
            chrome_hover: 0x3b4261,
            chrome_active: 0x16161e,
            sidebar: 0x1f2335,
            border: 0x3b4261,
            border_subtle: 0x292e42,
            text: 0xc0caf5,
            text_muted: 0x9aa5ce,
            text_faint: 0x565f89,
            accent: 0x7aa2f7,
            accent_strong: 0x2ac3de,
            accent_muted: 0x2f426f,
            accent_wash: 0x1d2d4f,
            accent_alt: 0xbb9af7,
            highlight: 0xe0af68,
            selection: 0x283457,
        },
        ThemeKind::GruvboxDark => Palette {
            app_bg: 0x1d2021,
            editor_bg: 0x282828,
            editor_bg_raised: 0x32302f,
            chrome: 0x282828,
            chrome_elevated: 0x3c3836,
            chrome_hover: 0x504945,
            chrome_active: 0x1d2021,
            sidebar: 0x242321,
            border: 0x504945,
            border_subtle: 0x3c3836,
            text: 0xebdbb2,
            text_muted: 0xbdae93,
            text_faint: 0x7c6f64,
            accent: 0x83a598,
            accent_strong: 0x8ec07c,
            accent_muted: 0x3f5f58,
            accent_wash: 0x2c3f3a,
            accent_alt: 0xd3869b,
            highlight: 0xfabd2f,
            selection: 0x3f4f46,
        },
        ThemeKind::Nord => Palette {
            app_bg: 0x242933,
            editor_bg: 0x2e3440,
            editor_bg_raised: 0x343c4a,
            chrome: 0x2b303b,
            chrome_elevated: 0x3b4252,
            chrome_hover: 0x434c5e,
            chrome_active: 0x242933,
            sidebar: 0x2b303b,
            border: 0x4c566a,
            border_subtle: 0x3b4252,
            text: 0xe5e9f0,
            text_muted: 0xd8dee9,
            text_faint: 0x8793a8,
            accent: 0x88c0d0,
            accent_strong: 0x81a1c1,
            accent_muted: 0x3f5d6b,
            accent_wash: 0x314853,
            accent_alt: 0xb48ead,
            highlight: 0xebcb8b,
            selection: 0x405766,
        },
        ThemeKind::EverforestDark => Palette {
            app_bg: 0x1e2326,
            editor_bg: 0x272e33,
            editor_bg_raised: 0x2e383c,
            chrome: 0x232a2e,
            chrome_elevated: 0x374145,
            chrome_hover: 0x485258,
            chrome_active: 0x1e2326,
            sidebar: 0x232a2e,
            border: 0x4f5b58,
            border_subtle: 0x374145,
            text: 0xd3c6aa,
            text_muted: 0xa7c080,
            text_faint: 0x7a8478,
            accent: 0x7fbbb3,
            accent_strong: 0xa7c080,
            accent_muted: 0x3f5d5a,
            accent_wash: 0x2c4441,
            accent_alt: 0xe67e80,
            highlight: 0xdbbc7f,
            selection: 0x3a5450,
        },
        ThemeKind::RosePine => Palette {
            app_bg: 0x191724,
            editor_bg: 0x1f1d2e,
            editor_bg_raised: 0x26233a,
            chrome: 0x1f1d2e,
            chrome_elevated: 0x2a273f,
            chrome_hover: 0x403d52,
            chrome_active: 0x191724,
            sidebar: 0x1f1d2e,
            border: 0x524f67,
            border_subtle: 0x403d52,
            text: 0xe0def4,
            text_muted: 0x908caa,
            text_faint: 0x6e6a86,
            accent: 0x9ccfd8,
            accent_strong: 0xc4a7e7,
            accent_muted: 0x3a5060,
            accent_wash: 0x293947,
            accent_alt: 0xebbcba,
            highlight: 0xf6c177,
            selection: 0x393552,
        },
        ThemeKind::Kanagawa => Palette {
            app_bg: 0x16161d,
            editor_bg: 0x1f1f28,
            editor_bg_raised: 0x252535,
            chrome: 0x181820,
            chrome_elevated: 0x2a2a37,
            chrome_hover: 0x363646,
            chrome_active: 0x16161d,
            sidebar: 0x181820,
            border: 0x54546d,
            border_subtle: 0x363646,
            text: 0xdcd7ba,
            text_muted: 0xc8c093,
            text_faint: 0x727169,
            accent: 0x7e9cd8,
            accent_strong: 0x7aa89f,
            accent_muted: 0x31445f,
            accent_wash: 0x26364f,
            accent_alt: 0xd27e99,
            highlight: 0xe6c384,
            selection: 0x2d4f67,
        },
    }
}

pub fn load_theme(path: &Path) -> ThemeKind {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return ThemeKind::WorkbenchDark;
    };

    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(key, value)| (key.trim() == "theme").then(|| ThemeKind::from_conf_value(value)))
        .unwrap_or(ThemeKind::WorkbenchDark)
}

pub fn load_theme_color_mode(path: &Path) -> ThemeColorMode {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return ThemeColorMode::Minimal;
    };

    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(key, value)| {
            (key.trim() == "theme_color_mode").then(|| ThemeColorMode::from_conf_value(value))
        })
        .unwrap_or(ThemeColorMode::Minimal)
}

pub fn save_theme(path: &Path, theme: ThemeKind) -> std::io::Result<()> {
    save_setting_line(path, "theme", theme.conf_value())
}

pub fn save_theme_color_mode(path: &Path, mode: ThemeColorMode) -> std::io::Result<()> {
    save_setting_line(path, "theme_color_mode", mode.conf_value())
}

fn save_setting_line(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
    let mut lines: Vec<String> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(ToString::to_string)
        .collect();

    if let Some(line) = lines.iter_mut().find(|line| {
        line.split_once('=')
            .map(|(existing_key, _)| existing_key.trim() == key)
            .unwrap_or(false)
    }) {
        *line = format!("{key}={value}");
    } else {
        let insert_at = lines
            .iter()
            .position(|line| line.trim() == "[KEYBINDS]")
            .unwrap_or(lines.len());
        if insert_at > 0 && !lines[insert_at.saturating_sub(1)].trim().is_empty() {
            lines.insert(insert_at, String::new());
            lines.insert(insert_at, format!("{key}={value}"));
        } else {
            lines.insert(insert_at, format!("{key}={value}"));
        }
    }

    std::fs::write(path, format!("{}\n", lines.join("\n")))
}

pub mod color {
    pub const APP_BG: u32 = 0x1b1d20;
    pub const EDITOR_BG: u32 = 0x1f2023;
    pub const EDITOR_BG_RAISED: u32 = 0x24262a;
    pub const CHROME: u32 = 0x27292d;
    pub const CHROME_ELEVATED: u32 = 0x303238;
    pub const CHROME_HOVER: u32 = 0x383b42;
    pub const CHROME_ACTIVE: u32 = 0x202226;
    pub const SIDEBAR: u32 = 0x24262a;
    pub const BORDER: u32 = 0x3f424a;
    pub const BORDER_SUBTLE: u32 = 0x31343a;
    pub const TEXT: u32 = 0xd7d9de;
    pub const TEXT_MUTED: u32 = 0x9ca0aa;
    pub const TEXT_FAINT: u32 = 0x676c76;
    pub const ACCENT: u32 = 0x6aa6df;
    pub const ACCENT_STRONG: u32 = 0x2f7fc1;
    pub const ACCENT_MUTED: u32 = 0x254967;
    pub const ACCENT_WASH: u32 = 0x1d3344;
    pub const ACCENT_ALT: u32 = 0xc58edb;
    pub const HIGHLIGHT: u32 = 0xe0c36e;
    pub const SELECTION: u32 = 0x2b4e69;
}

pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
}

pub mod radius {
    pub const XS: f32 = 2.0;
    pub const SM: f32 = 3.0;
    pub const MD: f32 = 4.0;
}
