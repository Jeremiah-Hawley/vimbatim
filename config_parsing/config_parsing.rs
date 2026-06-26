/*
This file should have a function that parses the setting.conf file
and output a struct containing the settings from the level
*/

use regex::Regex;
use std::fs;

pub struct Settings {
    // [FORMATTING]
    pub highlight_color: String,
    pub small_size: u8,
    pub large_size: u8,
    pub paragraph_integrity: bool,
    pub pilcrows: bool,
    // [KEYBINDS]
    pub vim: bool,
    pub paste: String,
    pub condense: String,
    pub pocket_hotkey: String,
    pub hat: String,
    pub block: String,
    pub tag: String,
    pub cite: String,
    pub underline: String,
    pub emphasis: String,
    pub highlight: String,
    pub clear: String,
    pub delete_tags: String,
    pub new_document: String,
    pub start_timer: String,
    pub open_stats: String,
    pub shrink: String,
    pub cite_from_link: String,
    pub wikifi: String,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            highlight_color: String::new(),
            small_size: 0,
            large_size: 0,
            paragraph_integrity: false,
            pilcrows: false,
            vim: false,
            paste: String::new(),
            condense: String::new(),
            pocket_hotkey: String::new(),
            hat: String::new(),
            block: String::new(),
            tag: String::new(),
            cite: String::new(),
            underline: String::new(),
            emphasis: String::new(),
            highlight: String::new(),
            clear: String::new(),
            delete_tags: String::new(),
            new_document: String::new(),
            start_timer: String::new(),
            open_stats: String::new(),
            shrink: String::new(),
            cite_from_link: String::new(),
            wikifi: String::new(),
        }
    }

    pub fn parse(filename: &str) -> Settings {
        let mut s = Settings::new();
        let content = fs::read_to_string(filename).expect("Failed to read config file");
        let re = Regex::new(r"^(\w+)=(.+)$").unwrap();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            if let Some(caps) = re.captures(line) {
                let key = &caps[1];
                let value = caps[2].trim().to_string();
                match key {
                    "highlight_color"    => s.highlight_color = value,
                    "small_size"         => s.small_size = value.parse().unwrap_or(0),
                    "large_size"         => s.large_size = value.parse().unwrap_or(0),
                    "paragraph_integrity"=> s.paragraph_integrity = value == "true",
                    "pilcrows"           => s.pilcrows = value == "true",
                    "vim"                => s.vim = value == "true",
                    "paste"              => s.paste = value,
                    "condense"           => s.condense = value,
                    "pocket_hotkey"      => s.pocket_hotkey = value,
                    "hat"                => s.hat = value,
                    "block"              => s.block = value,
                    "tag"                => s.tag = value,
                    "cite"               => s.cite = value,
                    "underline"          => s.underline = value,
                    "emphasis"           => s.emphasis = value,
                    "highlight"          => s.highlight = value,
                    "clear"              => s.clear = value,
                    "delete_tags"        => s.delete_tags = value,
                    "new_document"       => s.new_document = value,
                    "start_timer"        => s.start_timer = value,
                    "open_stats"         => s.open_stats = value,
                    "shrink"             => s.shrink = value,
                    "cite_from_link"     => s.cite_from_link = value,
                    "wikifi"             => s.wikifi = value,
                    _ => {}
                }
            }
        }
        s
    }
}

// parse function - called from main
pub fn parse(filename: &str) -> Settings {
    Settings::parse(filename)
}
