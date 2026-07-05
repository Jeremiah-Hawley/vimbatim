mod docx_parser;
mod document_ops;
mod state;
mod tab_bar;
mod app_toolbar;
mod formatting_ribbon;
mod text_editor;
mod auto_scroll;
mod case_converter;
mod file_explorer;
mod settings_modal;
mod main_window;

use gpui::prelude::*;
use gpui::*;
use gpui_platform::application;
use main_window::{MainWindow, ToggleSidebar, ToggleSettings, Save};
use tab_bar::{CloseActiveTab, NewTab};

fn main() {
    /*
     * Application entry point.
     *
     * Creates the GPUI application, registers global keybindings for toggle-settings
     * (Ctrl+,), toggle-sidebar (Ctrl+B), new-tab (Ctrl+T), and close-tab (Ctrl+W),
     * then opens a 1200×800 centred window containing the MainWindow view.
     *
     * `cx.activate(true)` brings the window to the foreground on platforms that
     * require it (macOS).
     */
    application().run(|cx: &mut App| {
        // Register application-wide keybindings.
        // Actions are dispatched to whichever view has the keyboard focus.
        cx.bind_keys([
            KeyBinding::new("ctrl-,", ToggleSettings, None),
            KeyBinding::new("ctrl-b", ToggleSidebar, None),
            KeyBinding::new("ctrl-t", NewTab, None),
            KeyBinding::new("ctrl-w", CloseActiveTab, None),
            KeyBinding::new("ctrl-s", Save, None),
        ]);

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
