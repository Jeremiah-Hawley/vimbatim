use gpui::{px, rgb, svg, AssetSource, Result, SharedString, Styled};
use std::borrow::Cow;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Icon {
    AlignCenter,
    AlignLeft,
    AlignRight,
    CardMenu,
    ClearFormatting,
    Condense,
    DisclosureCollapsed,
    DisclosureExpanded,
    DocMenu,
    Emphasis,
    EyeClosed,
    EyeOpen,
    FileDocx,
    FileOpen,
    NewFile,
    Find,
    Fold,
    FolderOpen,
    Folder,
    FontSize,
    HighlightBucket,
    Highlight,
    ListBullet,
    ListNumbered,
    Nav,
    OpenWiki,
    Paste,
    Refresh,
    SaveAs,
    Save,
    SearchList,
    SettingsAppearance,
    SettingsFonts,
    SettingsKeybindings,
    SettingsText,
    SettingsToggles,
    Settings,
    Shrink,
    Sidebar,
    Split,
    SwitchTab,
    TabClose,
    TabNew,
    Tabroom,
    Timer,
    Wikifi,
    WindowClose,
    WindowMinimise,
    WordCount,
}

impl Icon {
    pub fn path(&self) -> &'static str {
        match self {
            Icon::AlignCenter => "icons/align-center.svg",
            Icon::AlignLeft => "icons/align-left.svg",
            Icon::AlignRight => "icons/align-right.svg",
            Icon::CardMenu => "icons/card_menu.svg",
            Icon::ClearFormatting => "icons/clear-formatting.svg",
            Icon::Condense => "icons/Fold_Icon.svg",
            Icon::DisclosureCollapsed => "icons/disclosure-collapsed.svg",
            Icon::DisclosureExpanded => "icons/disclosure-expanded.svg",
            Icon::DocMenu => "icons/document_menu.svg",
            Icon::Emphasis => "icons/emphasis.svg",
            Icon::EyeClosed => "icons/eye-closed.svg",
            Icon::EyeOpen => "icons/eye-open.svg",
            Icon::FileDocx => "icons/file-docx.svg",
            Icon::FileOpen => "icons/Open_File.svg",
            Icon::NewFile => "icons/New_File.svg",
            Icon::Find => "icons/search.svg",
            Icon::Fold => "icons/fold_icons.svg",
            Icon::FolderOpen => "icons/Open_Folder.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::FontSize => "icons/font-size.svg",
            Icon::HighlightBucket => "icons/highlight-bucket.svg",
            Icon::Highlight => "icons/Highlight.svg",
            Icon::ListBullet => "icons/Bullet_List.svg",
            Icon::ListNumbered => "icons/Numbered_List.svg",
            Icon::Nav => "icons/file_tree_menu.svg",
            Icon::OpenWiki => "icons/open_wiki.svg",
            Icon::Paste => "icons/paste.svg",
            Icon::Refresh => "icons/Refresh.svg",
            Icon::SaveAs => "icons/Save_As.svg",
            Icon::Save => "icons/save.svg",
            Icon::SearchList => "icons/search-list.svg",
            Icon::SettingsAppearance => "icons/settings-appearance.svg",
            Icon::SettingsFonts => "icons/settings-fonts.svg",
            Icon::SettingsKeybindings => "icons/settings-keybindings.svg",
            Icon::SettingsText => "icons/settings-text.svg",
            Icon::SettingsToggles => "icons/settings-toggles.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::Shrink => "icons/shrink.svg",
            Icon::Sidebar => "icons/split screen.svg",
            Icon::Split => "icons/Split_Icon.svg",
            Icon::SwitchTab => "icons/tab_menu.svg",
            Icon::TabClose => "icons/Close_Tab.svg",
            Icon::TabNew => "icons/New_Tab.svg",
            Icon::Tabroom => "icons/tabroom.svg",
            Icon::Timer => "icons/timer.svg",
            Icon::Wikifi => "icons/wikifi.svg",
            Icon::WindowClose => "icons/window-close.svg",
            Icon::WindowMinimise => "icons/window-minimise.svg",
            Icon::WordCount => "icons/document_stats.svg",
        }
    }

    pub fn all() -> &'static [Icon] {
        &[
            Icon::AlignCenter,
            Icon::AlignLeft,
            Icon::AlignRight,
            Icon::CardMenu,
            Icon::ClearFormatting,
            Icon::Condense,
            Icon::DisclosureCollapsed,
            Icon::DisclosureExpanded,
            Icon::DocMenu,
            Icon::Emphasis,
            Icon::EyeClosed,
            Icon::EyeOpen,
            Icon::FileDocx,
            Icon::FileOpen,
            Icon::NewFile,
            Icon::Find,
            Icon::Fold,
            Icon::FolderOpen,
            Icon::Folder,
            Icon::FontSize,
            Icon::HighlightBucket,
            Icon::Highlight,
            Icon::ListBullet,
            Icon::ListNumbered,
            Icon::Nav,
            Icon::OpenWiki,
            Icon::Paste,
            Icon::Refresh,
            Icon::SaveAs,
            Icon::Save,
            Icon::SearchList,
            Icon::SettingsAppearance,
            Icon::SettingsFonts,
            Icon::SettingsKeybindings,
            Icon::SettingsText,
            Icon::SettingsToggles,
            Icon::Settings,
            Icon::Shrink,
            Icon::Sidebar,
            Icon::Split,
            Icon::SwitchTab,
            Icon::TabClose,
            Icon::TabNew,
            Icon::Tabroom,
            Icon::Timer,
            Icon::Wikifi,
            Icon::WindowClose,
            Icon::WindowMinimise,
            Icon::WordCount,
        ]
    }
}

pub struct VimbatimAssets;

impl AssetSource for VimbatimAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "icons/align-center.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/align-center.svg"
            )))),
            "icons/align-left.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/align-left.svg"
            )))),
            "icons/align-right.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/align-right.svg"
            )))),
            "icons/card_menu.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/card_menu.svg"
            )))),
            "icons/clear-formatting.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/clear-formatting.svg"
            )))),
            "icons/Fold_Icon.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Fold_Icon.svg"
            )))),
            "icons/disclosure-collapsed.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/disclosure-collapsed.svg"
            )))),
            "icons/disclosure-expanded.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/disclosure-expanded.svg"
            )))),
            "icons/document_menu.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/document_menu.svg"
            )))),
            "icons/emphasis.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/emphasis.svg"))))
            }
            "icons/eye-closed.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/eye-closed.svg"
            )))),
            "icons/eye-open.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/eye-open.svg"))))
            }
            "icons/file-docx.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/file-docx.svg"
            )))),
            "icons/Open_File.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Open_File.svg"
            )))),
            "icons/New_File.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/New_File.svg"))))
            }
            "icons/search.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/search.svg")))),
            "icons/fold_icons.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/fold_icons.svg"
            )))),
            "icons/Open_Folder.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Open_Folder.svg"
            )))),
            "icons/folder.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/folder.svg")))),
            "icons/font-size.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/font-size.svg"
            )))),
            "icons/highlight-bucket.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/highlight-bucket.svg"
            )))),
            "icons/Highlight.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Highlight.svg"
            )))),
            "icons/Bullet_List.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Bullet_List.svg"
            )))),
            "icons/Numbered_List.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Numbered_List.svg"
            )))),
            "icons/file_tree_menu.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/file_tree_menu.svg"
            )))),
            "icons/open_wiki.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/open_wiki.svg"
            )))),
            "icons/paste.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/paste.svg")))),
            "icons/Refresh.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/Refresh.svg")))),
            "icons/Save_As.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/Save_As.svg")))),
            "icons/save.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/save.svg")))),
            "icons/search-list.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/search-list.svg"
            )))),
            "icons/settings-appearance.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-appearance.svg"
            )))),
            "icons/settings-fonts.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-fonts.svg"
            )))),
            "icons/settings-keybindings.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-keybindings.svg"
            )))),
            "icons/settings-text.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-text.svg"
            )))),
            "icons/settings-toggles.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/settings-toggles.svg"
            )))),
            "icons/settings.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/settings.svg"))))
            }
            "icons/shrink.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/shrink.svg")))),
            "icons/split screen.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/split screen.svg"
            )))),
            "icons/Split_Icon.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Split_Icon.svg"
            )))),
            "icons/tab_menu.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/tab_menu.svg"))))
            }
            "icons/Close_Tab.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/Close_Tab.svg"
            )))),
            "icons/New_Tab.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/New_Tab.svg")))),
            "icons/tabroom.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/tabroom.svg")))),
            "icons/timer.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/timer.svg")))),
            "icons/wikifi.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/wikifi.svg")))),
            "icons/window-close.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/window-close.svg"
            )))),
            "icons/window-minimise.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/window-minimise.svg"
            )))),
            "icons/document_stats.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/document_stats.svg"
            )))),
            _ => Ok(None),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path == "icons" {
            Ok(Icon::all().iter().map(|i| i.path().into()).collect())
        } else {
            Ok(vec![])
        }
    }
}

pub(crate) fn icon(icon: Icon, color: u32, size: f32) -> gpui::Svg {
    svg()
        .path(icon.path())
        .w(px(size))
        .h(px(size))
        .text_color(rgb(color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_completeness() {
        let assets = VimbatimAssets;
        let mut paths = std::collections::HashSet::new();

        for i in Icon::all() {
            let path = i.path();
            assert!(paths.insert(path), "Duplicate path: {}", path);

            let bytes = assets.load(path).unwrap().unwrap();
            assert!(!bytes.is_empty(), "Empty asset: {}", path);

            // Verify it looks like an SVG.
            let s = std::str::from_utf8(&bytes).unwrap();
            assert!(s.contains("<svg"), "Missing <svg in {}", path);
            assert!(
                s.contains("viewBox=\"0 0 24 24\"") || s.contains("viewBox="),
                "Missing viewBox in {}",
                path
            );
        }

        // Test that list("icons") contains everything
        let listed = assets.list("icons").unwrap();
        assert_eq!(listed.len(), Icon::all().len());

        if let Ok(entries) = std::fs::read_dir("icons") {
            let mut file_count = 0;
            for entry in entries {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.extension().map(|s| s == "svg").unwrap_or(false) {
                    file_count += 1;
                }
            }
            assert_eq!(
                file_count,
                Icon::all().len(),
                "Mismatch between files in icons/ and Icon enum"
            );
        }
    }
}
