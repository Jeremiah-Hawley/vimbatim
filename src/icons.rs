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
            Icon::CardMenu => "icons/card-menu.svg",
            Icon::ClearFormatting => "icons/clear-formatting.svg",
            Icon::Condense => "icons/condense.svg",
            Icon::DisclosureCollapsed => "icons/disclosure-collapsed.svg",
            Icon::DisclosureExpanded => "icons/disclosure-expanded.svg",
            Icon::DocMenu => "icons/doc-menu.svg",
            Icon::Emphasis => "icons/emphasis.svg",
            Icon::EyeClosed => "icons/eye-closed.svg",
            Icon::EyeOpen => "icons/eye-open.svg",
            Icon::FileDocx => "icons/file-docx.svg",
            Icon::FileOpen => "icons/file-open.svg",
            Icon::Find => "icons/find.svg",
            Icon::Fold => "icons/fold.svg",
            Icon::FolderOpen => "icons/folder-open.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::FontSize => "icons/font-size.svg",
            Icon::HighlightBucket => "icons/highlight-bucket.svg",
            Icon::Highlight => "icons/highlight.svg",
            Icon::ListBullet => "icons/list-bullet.svg",
            Icon::ListNumbered => "icons/list-numbered.svg",
            Icon::Nav => "icons/nav.svg",
            Icon::OpenWiki => "icons/open-wiki.svg",
            Icon::Paste => "icons/paste.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::SaveAs => "icons/save-as.svg",
            Icon::Save => "icons/save.svg",
            Icon::SearchList => "icons/search-list.svg",
            Icon::SettingsAppearance => "icons/settings-appearance.svg",
            Icon::SettingsFonts => "icons/settings-fonts.svg",
            Icon::SettingsKeybindings => "icons/settings-keybindings.svg",
            Icon::SettingsText => "icons/settings-text.svg",
            Icon::SettingsToggles => "icons/settings-toggles.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::Shrink => "icons/shrink.svg",
            Icon::Sidebar => "icons/sidebar.svg",
            Icon::Split => "icons/split.svg",
            Icon::SwitchTab => "icons/switch-tab.svg",
            Icon::TabClose => "icons/tab-close.svg",
            Icon::TabNew => "icons/tab-new.svg",
            Icon::Tabroom => "icons/tabroom.svg",
            Icon::Timer => "icons/timer.svg",
            Icon::Wikifi => "icons/wikifi.svg",
            Icon::WindowClose => "icons/window-close.svg",
            Icon::WindowMinimise => "icons/window-minimise.svg",
            Icon::WordCount => "icons/word-count.svg",
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
            "icons/card-menu.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/card-menu.svg"
            )))),
            "icons/clear-formatting.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/clear-formatting.svg"
            )))),
            "icons/condense.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/condense.svg"))))
            }
            "icons/disclosure-collapsed.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/disclosure-collapsed.svg"
            )))),
            "icons/disclosure-expanded.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/disclosure-expanded.svg"
            )))),
            "icons/doc-menu.svg" => {
                Ok(Some(Cow::Borrowed(include_bytes!("../icons/doc-menu.svg"))))
            }
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
            "icons/file-open.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/file-open.svg"
            )))),
            "icons/find.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/find.svg")))),
            "icons/fold.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/fold.svg")))),
            "icons/folder-open.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/folder-open.svg"
            )))),
            "icons/folder.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/folder.svg")))),
            "icons/font-size.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/font-size.svg"
            )))),
            "icons/highlight-bucket.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/highlight-bucket.svg"
            )))),
            "icons/highlight.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/highlight.svg"
            )))),
            "icons/list-bullet.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/list-bullet.svg"
            )))),
            "icons/list-numbered.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/list-numbered.svg"
            )))),
            "icons/nav.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/nav.svg")))),
            "icons/open-wiki.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/open-wiki.svg"
            )))),
            "icons/paste.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/paste.svg")))),
            "icons/refresh.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/refresh.svg")))),
            "icons/save-as.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/save-as.svg")))),
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
            "icons/sidebar.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/sidebar.svg")))),
            "icons/split.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/split.svg")))),
            "icons/switch-tab.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/switch-tab.svg"
            )))),
            "icons/tab-close.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/tab-close.svg"
            )))),
            "icons/tab-new.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/tab-new.svg")))),
            "icons/tabroom.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/tabroom.svg")))),
            "icons/timer.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/timer.svg")))),
            "icons/wikifi.svg" => Ok(Some(Cow::Borrowed(include_bytes!("../icons/wikifi.svg")))),
            "icons/window-close.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/window-close.svg"
            )))),
            "icons/window-minimise.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/window-minimise.svg"
            )))),
            "icons/word-count.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../icons/word-count.svg"
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
