// Suppresses the console window Windows otherwise opens alongside the GUI
// window on every launch (the default "console" subsystem always allocates
// one, regardless of whether anything is ever printed — commenting out
// println!/eprintln! calls alone would not have stopped it). No effect on
// macOS/Linux. Must stay the first item in this file: an inner attribute
// placed after any item is a compile error, not a silent no-op.
#![windows_subsystem = "windows"]

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

/// Embeds `text_editor::FONT_FAMILY`'s 4 faces (`assets/DejaVuSansMono*.ttf`,
/// Bitstream Vera license — see `assets/DejaVuSansMono.LICENSE`, free to
/// redistribute) into the binary and registers them with GPUI's text
/// system, so bold/italic render correctly regardless of what's actually
/// installed on the host machine.
///
/// Without this, whether bold/italic show up at all silently depends on the
/// host having "DejaVu Sans Mono" installed with *all four* weight/style
/// faces present — this app's own click/cursor math already assumes one
/// consistent monospace font everywhere (see `FONT_FAMILY`'s own doc
/// comment), so relying on the system to provide it was never sound. Real
/// hardware testing found this reproduced identically on Windows and (via a
/// separate but related bug, since fixed — `apply_run_style` was letting a
/// docx's own `<w:rFonts>` override this font per run) on Linux too: GPUI's
/// font matching only resolves weight/style correctly when more than one
/// face of the requested family is actually loaded (`candidates.len() == 1`
/// short-circuits past weight/style selection — see the same note on
/// `formatting_ribbon.rs`'s icon rendering, which hit this identically for
/// the ribbon's own B/I letters).
///
/// Additive, not a replacement: on a machine that already has DejaVu Sans
/// Mono installed, GPUI's family lookup will now find both the system's
/// faces and these embedded ones under the same name — harmless, since
/// `find_best_match`'s scoring picks whichever scores best regardless of
/// where it came from, but expected, not a bug, if a debugger ever notices
/// duplicate candidates.
fn load_bundled_fonts(cx: &mut App) {
    let fonts = vec![
        std::borrow::Cow::Borrowed(include_bytes!("../assets/DejaVuSansMono.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/DejaVuSansMono-Bold.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/DejaVuSansMono-Oblique.ttf").as_slice()),
        std::borrow::Cow::Borrowed(include_bytes!("../assets/DejaVuSansMono-BoldOblique.ttf").as_slice()),
    ];
    // Deliberately not `let _ =`: a silently-failing font load is exactly
    // the failure class that produced this bug in the first place, and a
    // double-clicked GUI app has no visible console to report it to (same
    // reasoning as `install_panic_hook`'s crash log) — so a failure here
    // gets written to that same file instead of vanishing.
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        state::log_line(&format!("\n--- vimbatim: failed to load bundled fonts: {e} ---"));
    }
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
        load_bundled_fonts(cx);

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
                    // tab_bar.rs draws its own drag region + minimize/maximize/close
                    // buttons. `false` here leaves GPUI's Windows backend showing the
                    // native OS caption too (gpui_windows/window.rs maps this straight
                    // to `hide_title_bar`), stacking a second, native set of window
                    // chrome above the app's own — the "two menus" bug. macOS shows its
                    // native traffic lights regardless of this flag (different style
                    // mask), so scoping to Windows avoids trading a stacked duplicate
                    // there for an overlapping one.
                    appears_transparent: cfg!(target_os = "windows"),
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

#[cfg(test)]
mod tests {
    // GPUI's own font matching (`gpui_wgpu`'s cosmic-text path, and
    // DirectWrite's `GetMatchingFonts` on Windows) both resolve weight/style
    // by exact family-name string equality plus the font's own declared
    // weight/style metadata — not by which file the bytes came from. If
    // these 4 faces ever registered under *different* family strings (an
    // old-style font naming a bold face "DejaVu Sans Mono Bold" as its own
    // family, rather than family="DejaVu Sans Mono" + a Bold subfamily, is a
    // real, common failure mode for older TTFs), `load_bundled_fonts` would
    // silently load 4 unrelated one-off "families" instead of one family
    // with 4 faces, and every fix riding on this bundling would silently do
    // nothing — this is the check that rules that out for the actual bytes
    // being shipped, not just this one machine's copy of the font.
    #[test]
    fn test_bundled_fonts_register_as_one_family_with_four_distinct_faces() {
        let fonts: [(&str, &[u8]); 4] = [
            ("Book", include_bytes!("../assets/DejaVuSansMono.ttf")),
            ("Bold", include_bytes!("../assets/DejaVuSansMono-Bold.ttf")),
            ("Oblique", include_bytes!("../assets/DejaVuSansMono-Oblique.ttf")),
            ("BoldOblique", include_bytes!("../assets/DejaVuSansMono-BoldOblique.ttf")),
        ];
        let mut seen = std::collections::HashSet::new();
        for (label, bytes) in fonts {
            let face = ttf_parser::Face::parse(bytes, 0)
                .unwrap_or_else(|e| panic!("{label}: not a valid font file: {e:?}"));
            let family = face
                .names()
                .into_iter()
                .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
                .and_then(|n| n.to_string())
                .unwrap_or_else(|| panic!("{label}: no Unicode family name record"));
            assert_eq!(
                family, "DejaVu Sans Mono",
                "{label}: bundled font must register under the exact family FONT_FAMILY requests"
            );
            let bold = face.is_bold();
            let italic = face.is_italic();
            assert!(
                seen.insert((bold, italic)),
                "{label}: (bold={bold}, italic={italic}) duplicates a face already seen — \
                 GPUI's weight/style matching can't tell these two apart"
            );
        }
        assert_eq!(seen.len(), 4, "expected 4 distinct (bold, italic) combinations, got {seen:?}");
    }
}
