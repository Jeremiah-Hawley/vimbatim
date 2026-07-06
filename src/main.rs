mod docx_parser;
mod document_ops;
mod keybinds;
mod state;
mod tab_bar;
mod app_toolbar;
mod formatting_ribbon;
mod text_editor;
mod auto_scroll;
mod case_converter;
mod color_picker;
mod file_explorer;
mod wikifi_export;
mod settings_modal;
mod main_window;

use gpui::prelude::*;
use gpui::*;
use gpui_platform::application;
use keybinds::{rebuild_keymap, Keybinds};
use main_window::MainWindow;
use std::path::Path;

fn main() {
    /*
     * Application entry point.
     *
     * Creates the GPUI application, loads every configurable keybinding from
     * settings.conf (src/keybinds.rs) and registers them, then opens a
     * 1280×768 centred window containing the MainWindow view.
     *
     * `cx.activate(true)` brings the window to the foreground on platforms that
     * require it (macOS).
     */
    application().run(|cx: &mut App| {
        // All non-vim keybindings (toggle-settings, toggle-sidebar, new-tab,
        // close-tab, save, copy/cut/paste, undo/redo, card styles, etc.) are
        // loaded from settings.conf and registered here. The settings modal
        // calls `rebuild_keymap` again at runtime whenever the user remaps
        // one, so this isn't the only place this ever runs.
        let keybinds = Keybinds::load(Path::new("settings.conf"));
        rebuild_keymap(cx, &keybinds);

        let bounds = Bounds::centered(
            None,
            size(px(1280.0), px(768.0)),
            cx,
        );

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Vimbatim".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| MainWindow::new(cx)),
        )
        .expect("Failed to open main window");

        cx.activate(true);
    });
}
