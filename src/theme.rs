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
    /// Settings -> Themes -> Import Theme. Deliberately absent from `all()`
    /// (the settings modal only offers it as a pill once `AppState.custom_theme`
    /// is actually loaded) and from `dark_palette`/`light_palette` in any
    /// meaningful sense — its real colors live in `AppState.custom_theme`
    /// at runtime, not in this `const fn`'s compiled-in table, so resolving
    /// a palette for it must go through `AppState::current_palette`, never
    /// the bare `palette()` free function.
    Custom,
}

/// Light or dark variant of whichever `ThemeKind` is selected. Orthogonal to
/// the theme itself: every theme ships both, so switching mode keeps the user's
/// chosen palette family and only swaps its lightness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

impl ThemeMode {
    pub fn all() -> &'static [ThemeMode] {
        &[ThemeMode::Light, ThemeMode::Dark]
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }

    pub fn conf_value(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    pub fn from_conf_value(value: &str) -> ThemeMode {
        match value.trim().to_ascii_lowercase().as_str() {
            "light" => ThemeMode::Light,
            _ => ThemeMode::Dark,
        }
    }
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
            ThemeKind::Custom => "Custom",
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
            ThemeKind::Custom => "custom",
        }
    }

    /// `"custom"` must resolve to `ThemeKind::Custom` explicitly rather than
    /// falling through the `_` arm — that arm means "unrecognized", and
    /// silently downgrading a saved "custom" choice to Workbench Dark on
    /// every launch would make Import Theme un-persistable.
    pub fn from_conf_value(value: &str) -> ThemeKind {
        match value.trim().to_ascii_lowercase().as_str() {
            "catppuccin-mocha" | "catppuccin" => ThemeKind::CatppuccinMocha,
            "tokyo-night" | "tokyonight" => ThemeKind::TokyoNight,
            "gruvbox-dark" | "gruvbox" => ThemeKind::GruvboxDark,
            "nord" => ThemeKind::Nord,
            "everforest-dark" | "everforest" => ThemeKind::EverforestDark,
            "rose-pine" | "rosepine" => ThemeKind::RosePine,
            "kanagawa" => ThemeKind::Kanagawa,
            "custom" => ThemeKind::Custom,
            _ => ThemeKind::WorkbenchDark,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
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

pub const fn palette(kind: ThemeKind, mode: ThemeMode) -> Palette {
    match mode {
        ThemeMode::Dark => dark_palette(kind),
        ThemeMode::Light => light_palette(kind),
    }
}

const fn dark_palette(kind: ThemeKind) -> Palette {
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
        // Never actually resolved through here — `AppState::current_palette`
        // intercepts `ThemeKind::Custom` and returns the loaded
        // `AppState.custom_theme` instead, since this `const fn` has no
        // access to runtime state. This arm exists only so the match stays
        // exhaustive; Workbench Dark is a harmless, visibly-wrong-if-ever-hit
        // placeholder.
        ThemeKind::Custom => dark_palette(ThemeKind::WorkbenchDark),
    }
}

/// The light counterpart of each theme, taken from that palette family's own
/// published light variant rather than computed — Catppuccin Latte, Tokyo Night
/// Day, Gruvbox light, Nord's Snow Storm, Everforest light, Rosé Pine Dawn, and
/// Kanagawa Lotus. Mechanically inverting a dark palette's lightness gives
/// accents that were tuned against a dark background and wash out on a light
/// one, so these are authored, not derived. Workbench Light has no upstream and
/// mirrors Workbench Dark's structure.
const fn light_palette(kind: ThemeKind) -> Palette {
    match kind {
        ThemeKind::WorkbenchDark => Palette {
            app_bg: 0xf3f3f3,
            editor_bg: 0xffffff,
            editor_bg_raised: 0xf7f7f7,
            chrome: 0xececec,
            chrome_elevated: 0xe4e4e4,
            chrome_hover: 0xdcdcdc,
            chrome_active: 0xd2d2d2,
            sidebar: 0xf0f0f0,
            border: 0xc4c4c4,
            border_subtle: 0xe0e0e0,
            text: 0x1f2023,
            text_muted: 0x55585f,
            text_faint: 0x8a8f98,
            accent: 0x0a66c2,
            accent_strong: 0x09549f,
            accent_muted: 0xbcd8f5,
            accent_wash: 0xe3effb,
            accent_alt: 0x8250df,
            highlight: 0xc79100,
            selection: 0xcfe3fb,
        },
        // Catppuccin Latte
        ThemeKind::CatppuccinMocha => Palette {
            app_bg: 0xdce0e8,
            editor_bg: 0xeff1f5,
            editor_bg_raised: 0xe6e9ef,
            chrome: 0xe6e9ef,
            chrome_elevated: 0xccd0da,
            chrome_hover: 0xbcc0cc,
            chrome_active: 0xdce0e8,
            sidebar: 0xe6e9ef,
            border: 0xbcc0cc,
            border_subtle: 0xccd0da,
            text: 0x4c4f69,
            text_muted: 0x6c6f85,
            text_faint: 0x9ca0b0,
            accent: 0x1e66f5,
            accent_strong: 0x209fb5,
            accent_muted: 0xc5d3f8,
            accent_wash: 0xdce4fb,
            accent_alt: 0x8839ef,
            highlight: 0xdf8e1d,
            selection: 0xbfd0f5,
        },
        // Tokyo Night Day
        ThemeKind::TokyoNight => Palette {
            app_bg: 0xd0d5e3,
            editor_bg: 0xe1e2e7,
            editor_bg_raised: 0xd9dae3,
            chrome: 0xd6d8e0,
            chrome_elevated: 0xc4c8da,
            chrome_hover: 0xb7bcd1,
            chrome_active: 0xd0d5e3,
            sidebar: 0xd6d8e0,
            border: 0xa8aecb,
            border_subtle: 0xc4c8da,
            text: 0x3760bf,
            text_muted: 0x6172b0,
            text_faint: 0x848cb5,
            accent: 0x2e7de9,
            accent_strong: 0x007197,
            accent_muted: 0xb8cdf2,
            accent_wash: 0xd5e2f8,
            accent_alt: 0x9854f1,
            highlight: 0xb15c00,
            selection: 0xb6bfe2,
        },
        // Gruvbox light
        ThemeKind::GruvboxDark => Palette {
            app_bg: 0xf2e5bc,
            editor_bg: 0xfbf1c7,
            editor_bg_raised: 0xf9f5d7,
            chrome: 0xebdbb2,
            chrome_elevated: 0xd5c4a1,
            chrome_hover: 0xc8b795,
            chrome_active: 0xebdbb2,
            sidebar: 0xebdbb2,
            border: 0xbdae93,
            border_subtle: 0xd5c4a1,
            text: 0x3c3836,
            text_muted: 0x504945,
            text_faint: 0x7c6f64,
            accent: 0x076678,
            accent_strong: 0x427b58,
            accent_muted: 0xadc6cc,
            accent_wash: 0xd4e2e5,
            accent_alt: 0x8f3f71,
            highlight: 0xb57614,
            selection: 0xd5c4a1,
        },
        // Nord — Snow Storm backgrounds, Polar Night text
        ThemeKind::Nord => Palette {
            app_bg: 0xd8dee9,
            editor_bg: 0xeceff4,
            editor_bg_raised: 0xe5e9f0,
            chrome: 0xe5e9f0,
            chrome_elevated: 0xd8dee9,
            chrome_hover: 0xc9d2e0,
            chrome_active: 0xd8dee9,
            sidebar: 0xe5e9f0,
            border: 0xc0cadb,
            border_subtle: 0xd8dee9,
            text: 0x2e3440,
            text_muted: 0x4c566a,
            text_faint: 0x7b88a1,
            accent: 0x5e81ac,
            accent_strong: 0x4c7095,
            accent_muted: 0xbdd0e3,
            accent_wash: 0xdbe6f1,
            accent_alt: 0xb48ead,
            highlight: 0xbf8b30,
            selection: 0xc4d4e6,
        },
        // Everforest light
        ThemeKind::EverforestDark => Palette {
            app_bg: 0xf4f0d9,
            editor_bg: 0xfdf6e3,
            editor_bg_raised: 0xf4f0d9,
            chrome: 0xefebd4,
            chrome_elevated: 0xe6e2cc,
            chrome_hover: 0xdcd8c0,
            chrome_active: 0xefebd4,
            sidebar: 0xefebd4,
            border: 0xd8d3ba,
            border_subtle: 0xe6e2cc,
            text: 0x5c6a72,
            text_muted: 0x829181,
            text_faint: 0x939f91,
            accent: 0x3a94c5,
            accent_strong: 0x35a77c,
            accent_muted: 0xbcd9e8,
            accent_wash: 0xdcebf3,
            accent_alt: 0xdf69ba,
            highlight: 0xdfa000,
            selection: 0xe1e7c8,
        },
        // Rosé Pine Dawn
        ThemeKind::RosePine => Palette {
            app_bg: 0xf2e9e1,
            editor_bg: 0xfaf4ed,
            editor_bg_raised: 0xfffaf3,
            chrome: 0xf2e9e1,
            chrome_elevated: 0xdfdad9,
            chrome_hover: 0xcecacd,
            chrome_active: 0xf4ede8,
            sidebar: 0xf2e9e1,
            border: 0xcecacd,
            border_subtle: 0xdfdad9,
            text: 0x575279,
            text_muted: 0x797593,
            text_faint: 0x9893a5,
            accent: 0x286983,
            accent_strong: 0x56949f,
            accent_muted: 0xbcd4dc,
            accent_wash: 0xdde9ee,
            accent_alt: 0x907aa9,
            highlight: 0xea9d34,
            selection: 0xdfdad9,
        },
        // Kanagawa Lotus
        ThemeKind::Kanagawa => Palette {
            app_bg: 0xe5ddb0,
            editor_bg: 0xf2ecbc,
            editor_bg_raised: 0xe7dba0,
            chrome: 0xe5ddb0,
            chrome_elevated: 0xd5cea3,
            chrome_hover: 0xc9c093,
            chrome_active: 0xe5ddb0,
            sidebar: 0xe5ddb0,
            border: 0xc7bf94,
            border_subtle: 0xd5cea3,
            text: 0x545464,
            text_muted: 0x716e61,
            text_faint: 0x8a8980,
            accent: 0x4d699b,
            accent_strong: 0x597b75,
            accent_muted: 0xb3c0d5,
            accent_wash: 0xd6dfeb,
            accent_alt: 0x766b90,
            highlight: 0xcc6d00,
            selection: 0xc9cbd1,
        },
        // See the matching arm in `dark_palette` above — never actually
        // resolved through here in practice.
        ThemeKind::Custom => light_palette(ThemeKind::WorkbenchDark),
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

pub fn load_theme_mode(path: &Path) -> ThemeMode {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return ThemeMode::Dark;
    };

    contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(key, value)| (key.trim() == "theme_mode").then(|| ThemeMode::from_conf_value(value)))
        .unwrap_or(ThemeMode::Dark)
}

pub fn save_theme_mode(path: &Path, mode: ThemeMode) -> std::io::Result<()> {
    save_setting_line(path, "theme_mode", mode.conf_value())
}

pub fn save_theme(path: &Path, theme: ThemeKind) -> std::io::Result<()> {
    save_setting_line(path, "theme", theme.conf_value())
}

pub fn save_theme_color_mode(path: &Path, mode: ThemeColorMode) -> std::io::Result<()> {
    save_setting_line(path, "theme_color_mode", mode.conf_value())
}

/// `Palette`'s 20 fields, always in this same order — every writer/reader of
/// theme TOML in this module uses it, so a template, an export, and the
/// parser's expectations can never quietly drift apart.
const PALETTE_FIELDS: [&str; 20] = [
    "app_bg", "editor_bg", "editor_bg_raised", "chrome", "chrome_elevated",
    "chrome_hover", "chrome_active", "sidebar", "border", "border_subtle",
    "text", "text_muted", "text_faint", "accent", "accent_strong",
    "accent_muted", "accent_wash", "accent_alt", "highlight", "selection",
];

fn palette_field(p: &Palette, name: &str) -> Option<u32> {
    Some(match name {
        "app_bg" => p.app_bg,
        "editor_bg" => p.editor_bg,
        "editor_bg_raised" => p.editor_bg_raised,
        "chrome" => p.chrome,
        "chrome_elevated" => p.chrome_elevated,
        "chrome_hover" => p.chrome_hover,
        "chrome_active" => p.chrome_active,
        "sidebar" => p.sidebar,
        "border" => p.border,
        "border_subtle" => p.border_subtle,
        "text" => p.text,
        "text_muted" => p.text_muted,
        "text_faint" => p.text_faint,
        "accent" => p.accent,
        "accent_strong" => p.accent_strong,
        "accent_muted" => p.accent_muted,
        "accent_wash" => p.accent_wash,
        "accent_alt" => p.accent_alt,
        "highlight" => p.highlight,
        "selection" => p.selection,
        _ => return None,
    })
}

/// Builds a `Palette` from a `field name -> hex value` map, requiring every
/// one of `PALETTE_FIELDS` to be present — a template with a missing or
/// misspelled key fails the whole import rather than silently substituting
/// a color nobody asked for.
fn palette_from_fields(values: &std::collections::HashMap<&str, u32>) -> Option<Palette> {
    let mut p = Palette {
        app_bg: 0, editor_bg: 0, editor_bg_raised: 0, chrome: 0, chrome_elevated: 0,
        chrome_hover: 0, chrome_active: 0, sidebar: 0, border: 0, border_subtle: 0,
        text: 0, text_muted: 0, text_faint: 0, accent: 0, accent_strong: 0,
        accent_muted: 0, accent_wash: 0, accent_alt: 0, highlight: 0, selection: 0,
    };
    for field in PALETTE_FIELDS {
        let value = *values.get(field)?;
        match field {
            "app_bg" => p.app_bg = value,
            "editor_bg" => p.editor_bg = value,
            "editor_bg_raised" => p.editor_bg_raised = value,
            "chrome" => p.chrome = value,
            "chrome_elevated" => p.chrome_elevated = value,
            "chrome_hover" => p.chrome_hover = value,
            "chrome_active" => p.chrome_active = value,
            "sidebar" => p.sidebar = value,
            "border" => p.border = value,
            "border_subtle" => p.border_subtle = value,
            "text" => p.text = value,
            "text_muted" => p.text_muted = value,
            "text_faint" => p.text_faint = value,
            "accent" => p.accent = value,
            "accent_strong" => p.accent_strong = value,
            "accent_muted" => p.accent_muted = value,
            "accent_wash" => p.accent_wash = value,
            "accent_alt" => p.accent_alt = value,
            "highlight" => p.highlight = value,
            "selection" => p.selection = value,
            _ => unreachable!("PALETTE_FIELDS is the only source of `field`"),
        }
    }
    Some(p)
}

fn palette_to_toml_lines(p: &Palette) -> String {
    PALETTE_FIELDS
        .iter()
        .map(|field| format!("{field} = \"{:06x}\"", palette_field(p, field).unwrap_or(0)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses one `[dark]`/`[light]` section's `key = "hex"` lines (quotes
/// optional, `#` comments and blank lines ignored) into a `Palette` — `None`
/// if any of `PALETTE_FIELDS` is missing or unparseable.
fn parse_palette_section(lines: &[&str]) -> Option<Palette> {
    let mut values = std::collections::HashMap::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim().trim_matches('"');
        if let Some(hex) = crate::color_picker::parse_hex(value) {
            values.insert(key.trim(), hex);
        }
    }
    palette_from_fields(&values)
}

/// Downloadable starting point for Settings -> Themes -> Import Theme: a
/// fully valid, directly-reimportable file (Workbench Dark's own colors,
/// not placeholder gibberish) with `[dark]` and `[light]` sections the user
/// edits in place — matching `default_settings.conf`'s own role as a
/// pristine, complete example rather than an empty shell.
pub fn custom_theme_template() -> String {
    format!(
        "# Vimbatim custom theme.\n\
         # Edit the hex values below (no leading '#') and re-import this\n\
         # file from Settings -> Themes -> Import Theme.\n\n\
         [dark]\n{}\n\n[light]\n{}\n",
        palette_to_toml_lines(&dark_palette(ThemeKind::WorkbenchDark)),
        palette_to_toml_lines(&light_palette(ThemeKind::WorkbenchDark)),
    )
}

/// Parses an imported theme file into its `(dark, light)` pair. `None` on
/// anything malformed — a missing section or an incomplete/bad palette —
/// so the caller can fall back rather than half-apply a broken theme.
pub fn parse_custom_theme_toml(content: &str) -> Option<(Palette, Palette)> {
    let mut dark_lines = Vec::new();
    let mut light_lines = Vec::new();
    let mut section: Option<&str> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("[dark]") { section = Some("dark"); continue; }
        if trimmed.eq_ignore_ascii_case("[light]") { section = Some("light"); continue; }
        match section {
            Some("dark") => dark_lines.push(line),
            Some("light") => light_lines.push(line),
            _ => {}
        }
    }
    let dark = parse_palette_section(&dark_lines)?;
    let light = parse_palette_section(&light_lines)?;
    Some((dark, light))
}

pub(crate) fn save_setting_line(path: &Path, key: &str, value: &str) -> std::io::Result<()> {
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

/// Perceived luminance, 0.0 (black) to 1.0 (white), ITU-R BT.709 weighting.
fn luminance(hex: u32) -> f32 {
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Blends `hex` toward `target` by `amount` (0.0 = unchanged, 1.0 = target).
fn blend(hex: u32, target: u32, amount: f32) -> u32 {
    let mix = |shift: u32| {
        let from = ((hex >> shift) & 0xFF) as f32;
        let to = ((target >> shift) & 0xFF) as f32;
        (from + (to - from) * amount).round().clamp(0.0, 255.0) as u32
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// A variant of `hex` that stays visible against the given mode's chrome.
///
/// A highlight color is chosen for how it looks behind black text on white
/// paper, which is not the same thing as being readable as a swatch on app
/// chrome: yellow all but disappears on a light panel, and a saturated blue
/// disappears on a dark one. Only colors that would actually vanish are
/// touched, and the blend keeps the hue recognisable — the point is still to
/// show *which* color is selected.
pub fn visible_on_chrome(hex: u32, mode: ThemeMode) -> u32 {
    const TOO_DARK: f32 = 0.35;
    const TOO_LIGHT: f32 = 0.6;
    match mode {
        ThemeMode::Dark if luminance(hex) < TOO_DARK => blend(hex, 0xFFFFFF, 0.45),
        ThemeMode::Light if luminance(hex) > TOO_LIGHT => blend(hex, 0x000000, 0.35),
        _ => hex,
    }
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

#[cfg(test)]
mod visibility_tests {
    use super::*;

    /// Yellow is the default highlight and the worst case on a light panel —
    /// it must come back darker there, and be left alone on dark chrome.
    #[test]
    fn yellow_is_darkened_only_for_light_mode() {
        const YELLOW: u32 = 0xFFD700;
        assert_eq!(visible_on_chrome(YELLOW, ThemeMode::Dark), YELLOW);

        let light = visible_on_chrome(YELLOW, ThemeMode::Light);
        assert_ne!(light, YELLOW);
        assert!(luminance(light) < luminance(YELLOW), "should have darkened");
    }

    /// A saturated blue is the mirror case: invisible on dark chrome, fine on
    /// light.
    #[test]
    fn blue_is_lightened_only_for_dark_mode() {
        const BLUE: u32 = 0x0000FF;
        assert_eq!(visible_on_chrome(BLUE, ThemeMode::Light), BLUE);

        let dark = visible_on_chrome(BLUE, ThemeMode::Dark);
        assert_ne!(dark, BLUE);
        assert!(luminance(dark) > luminance(BLUE), "should have lightened");
    }

    /// A colour comfortably between the thresholds is passed through
    /// untouched in both modes.
    #[test]
    fn mid_tones_are_left_alone() {
        const MID_GREY: u32 = 0x808080;
        assert_eq!(visible_on_chrome(MID_GREY, ThemeMode::Dark), MID_GREY);
        assert_eq!(visible_on_chrome(MID_GREY, ThemeMode::Light), MID_GREY);
    }

    /// Adjusting must keep the colour recognisable — the button's whole job is
    /// to say *which* highlight is selected, so the dominant channels have to
    /// stay dominant. Magenta is the awkward case: it carries no green, so its
    /// luminance (0.28) reads as dark even though it looks vivid.
    #[test]
    fn adjusting_preserves_hue() {
        let channels = |hex: u32| (hex >> 16 & 0xFF, hex >> 8 & 0xFF, hex & 0xFF);
        for (name, color) in [("magenta", 0xFF00FFu32), ("blue", 0x0000FF), ("yellow", 0xFFD700)] {
            for mode in [ThemeMode::Dark, ThemeMode::Light] {
                let (r0, g0, b0) = channels(color);
                let (r1, g1, b1) = channels(visible_on_chrome(color, mode));
                assert_eq!(
                    (r0 > g0, g0 > b0, r0 > b0),
                    (r1 > g1, g1 > b1, r1 > b1),
                    "{name} lost its hue ordering in {mode:?} mode"
                );
            }
        }
    }

    #[test]
    fn blend_hits_both_ends_and_the_middle() {
        assert_eq!(blend(0x000000, 0xFFFFFF, 0.0), 0x000000);
        assert_eq!(blend(0x000000, 0xFFFFFF, 1.0), 0xFFFFFF);
        assert_eq!(blend(0x000000, 0xFFFFFF, 0.5), 0x808080);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Perceived brightness, 0.0 (black) to 1.0 (white). Same BT.601 weighting
    /// `text_editor::relative_luminance` uses, duplicated here rather than made
    /// public — this is a test-only sanity check, not a rendering path.
    fn brightness(hex: u32) -> f32 {
        let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
        let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
        let b = (hex & 0xFF) as f32 / 255.0;
        r * 0.299 + g * 0.587 + b * 0.114
    }

    #[test]
    fn custom_theme_template_round_trips_through_parse() {
        let template = custom_theme_template();
        let (dark, light) = parse_custom_theme_toml(&template).expect("template must parse");
        assert_eq!(dark, dark_palette(ThemeKind::WorkbenchDark));
        assert_eq!(light, light_palette(ThemeKind::WorkbenchDark));
    }

    #[test]
    fn parse_custom_theme_toml_rejects_a_missing_section() {
        let dark_only = format!("[dark]\n{}\n", palette_to_toml_lines(&dark_palette(ThemeKind::WorkbenchDark)));
        assert_eq!(parse_custom_theme_toml(&dark_only), None);
    }

    #[test]
    fn parse_custom_theme_toml_rejects_a_missing_field() {
        let mut broken = custom_theme_template();
        // Drop the `app_bg` line from the [dark] section entirely.
        broken = broken.lines().filter(|l| !l.trim().starts_with("app_bg")).collect::<Vec<_>>().join("\n");
        assert_eq!(parse_custom_theme_toml(&broken), None);
    }

    #[test]
    fn parse_custom_theme_toml_accepts_unquoted_hex_too() {
        // The template quotes values, but a hand-edited file without quotes
        // (still valid per `color_picker::parse_hex`) should work too.
        let unquoted = custom_theme_template().replace('"', "");
        let (dark, light) = parse_custom_theme_toml(&unquoted).expect("unquoted hex must still parse");
        assert_eq!(dark, dark_palette(ThemeKind::WorkbenchDark));
        assert_eq!(light, light_palette(ThemeKind::WorkbenchDark));
    }

    #[test]
    fn from_conf_value_resolves_custom_explicitly_not_via_fallback() {
        assert_eq!(ThemeKind::from_conf_value("custom"), ThemeKind::Custom);
    }

    #[test]
    fn test_theme_mode_conf_values_round_trip() {
        for mode in ThemeMode::all() {
            assert_eq!(ThemeMode::from_conf_value(mode.conf_value()), *mode);
        }
        // Tolerant of case and padding, like every other conf reader here.
        assert_eq!(ThemeMode::from_conf_value("  LIGHT "), ThemeMode::Light);
        // Unknown or missing falls back to the mode every theme shipped as.
        assert_eq!(ThemeMode::from_conf_value("sepia"), ThemeMode::Dark);
        assert_eq!(ThemeMode::from_conf_value(""), ThemeMode::Dark);
    }

    /// Guards the 160 hand-authored light hex values: a digit slipped in any
    /// background or text entry shows up here as a light palette that isn't
    /// actually light, or one whose text would vanish into its own background.
    #[test]
    fn test_light_palettes_are_light_and_dark_palettes_are_dark() {
        for kind in ThemeKind::all() {
            let light = palette(*kind, ThemeMode::Light);
            let dark = palette(*kind, ThemeMode::Dark);

            assert!(
                brightness(light.editor_bg) > 0.7,
                "{}'s light editor_bg is not light: {:06x}",
                kind.label(),
                light.editor_bg,
            );
            assert!(
                brightness(dark.editor_bg) < 0.3,
                "{}'s dark editor_bg is not dark: {:06x}",
                kind.label(),
                dark.editor_bg,
            );
            assert!(
                brightness(light.text) < brightness(light.editor_bg),
                "{}'s light text is not darker than its background",
                kind.label(),
            );
            assert!(
                brightness(dark.text) > brightness(dark.editor_bg),
                "{}'s dark text is not lighter than its background",
                kind.label(),
            );
        }
    }

    /// Chrome has to sit on the same side of the light/dark divide as the
    /// editor, or the app frame fights the page it surrounds.
    #[test]
    fn test_light_palette_chrome_stays_light() {
        for kind in ThemeKind::all() {
            let light = palette(*kind, ThemeMode::Light);
            for (name, value) in [
                ("app_bg", light.app_bg),
                ("chrome", light.chrome),
                ("chrome_elevated", light.chrome_elevated),
                ("sidebar", light.sidebar),
            ] {
                assert!(
                    brightness(value) > 0.55,
                    "{}'s light {name} is too dark: {value:06x}",
                    kind.label(),
                );
            }
        }
    }

    #[test]
    fn test_save_then_load_theme_mode_round_trips() {
        let dir = std::env::temp_dir()
            .join(format!("vimbatim-theme-mode-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.conf");
        std::fs::write(&path, "[FORMATTING]\ntheme=nord\ntheme_mode=dark\n").unwrap();

        save_theme_mode(&path, ThemeMode::Light).unwrap();
        assert_eq!(load_theme_mode(&path), ThemeMode::Light);
        // The neighbouring key is untouched.
        assert_eq!(load_theme(&path), ThemeKind::Nord);

        save_theme_mode(&path, ThemeMode::Dark).unwrap();
        assert_eq!(load_theme_mode(&path), ThemeMode::Dark);

        // A file with no theme_mode line at all reads as Dark.
        std::fs::write(&path, "theme=nord\n").unwrap();
        assert_eq!(load_theme_mode(&path), ThemeMode::Dark);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
