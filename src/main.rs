mod docx_parser;
mod document_ops;
mod keybinds;
mod vim_keybinds;
mod rich_clipboard;
mod recovery;
mod recovery_prompt;
mod state;
mod tab_bar;
mod app_toolbar;
mod formatting_ribbon;
mod text_editor;
mod auto_scroll;
mod case_converter;
mod color_picker;
mod file_explorer;
mod find_bar;
mod wikifi_export;
mod settings_modal;
mod close_confirm;
mod main_window;
mod spellcheck;
mod theme;
mod timer;
mod word_count;

use gpui::prelude::*;
use gpui::*;
use gpui_platform::application;
use keybinds::{rebuild_keymap, Keybinds};
use main_window::MainWindow;
use std::io::Write;

/// closed_beta_plan.md §5: a double-clicked GUI app has no visible console,
/// so an unhandled panic is otherwise completely silent to the tester (the
/// app just vanishes) and unreportable. Wraps the default panic behavior
/// (still prints to stderr, in case a console *is* attached) with an
/// append to a fixed crash-log file, tagged with the exact build so a bug
/// report can be tied back to a commit.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);

        // Unsaved work is not replaceable; the crash log is. Write snapshots
        // first, from the mirror the background task keeps current.
        if let Some(slot) = recovery::PANIC_SNAPSHOT.get() {
            if let Ok(tabs) = slot.lock() {
                recovery::write_all_snapshots(&tabs);
            }
        }

        let build = format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("VIMBATIM_GIT_SHA"));
        let backtrace = std::backtrace::Backtrace::force_capture();
        let entry = format!("\n--- vimbatim crash: build {build} ---\n{info}\n{backtrace}\n");

        let path = state::crash_log_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(entry.as_bytes());
        }
    }));
}

fn main() {
    install_panic_hook();

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
    // Must run before the first read of any setting: settings.conf now lives
    // in the per-user data directory (see `state::settings_conf_path`), which
    // on a fresh install contains nothing at all until this seeds it from the
    // bundled defaults.
    state::ensure_settings_file();

    application().run(|cx: &mut App| {
        // All non-vim keybindings (toggle-settings, toggle-sidebar, new-tab,
        // close-tab, save, copy/cut/paste, undo/redo, card styles, etc.) are
        // loaded from settings.conf and registered here. The settings modal
        // calls `rebuild_keymap` again at runtime whenever the user remaps
        // one, so this isn't the only place this ever runs.
        let keybinds = Keybinds::load(&state::settings_conf_path());
        rebuild_keymap(cx, &keybinds);

        let bounds = Bounds::centered(
            None,
            size(px(1280.0), px(768.0)),
            cx,
        );

        let _ = recovery::PANIC_SNAPSHOT
            .set(std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));

        let window = cx.open_window(
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

        // The native titlebar close button previously bypassed the
        // Save/Discard/Cancel prompt entirely: every other quit path routes
        // through AppState::request_close_app (see tab_bar.rs), but the OS
        // button routed nowhere. Returning `false` here keeps the window
        // open and lets close_confirm.rs drive the decision, exactly as the
        // in-app × already does.
        window
            .update(cx, |view, window, cx| {
                let state = view.state.clone();
                window.on_window_should_close(cx, move |_window, cx| {
                    let quit_now = state.update(cx, |s, cx| {
                        s.request_close_app();
                        cx.notify();
                        s.pending_close.is_none()
                    });
                    // Returning true only tells the platform not to veto the
                    // close — it does not terminate the app. GPUI's default
                    // QuitMode auto-quits when the last window closes on
                    // Linux/Windows but NOT on macOS, so quit explicitly here,
                    // exactly as tab_bar.rs and close_confirm.rs already do.
                    if quit_now {
                        cx.quit();
                    }
                    quit_now
                });
            })
            .expect("Failed to install window close handler");

        cx.activate(true);
    });
}
