//! User-imported fonts (formatting_ribbon.rs's Font Family "+ Add Font"
//! row): accepts a `.ttf`/`.otf` file, or a `.zip` containing one or more,
//! registers the faces with GPUI so they render, and persists them to
//! `recovery::app_data_dir()/fonts/<slug>/` so they survive a restart.
//!
//! No manifest file: a family's name is always re-derived by re-parsing one
//! of its own face files with `ttf_parser` (the exact family name Word
//! itself would write into `<w:rFonts>`, and the same check
//! `main.rs`'s `assert_family_has_four_distinct_faces` already relies on for
//! the bundled fonts) — nothing to fall out of sync.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::App;

/// One face's raw bytes plus the (family, bold, italic) identity read from
/// its own name/OS-2 tables.
#[derive(Debug)]
struct Face {
    family: String,
    bold: bool,
    italic: bool,
    bytes: Vec<u8>,
}

fn parse_face(bytes: Vec<u8>) -> Option<Face> {
    let face = ttf_parser::Face::parse(&bytes, 0).ok()?;
    let family = face
        .names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::FAMILY && n.is_unicode())
        .and_then(|n| n.to_string())?;
    let bold = face.is_bold();
    let italic = face.is_italic();
    drop(face);
    Some(Face { family, bold, italic, bytes })
}

fn is_font_entry(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".ttf") || lower.ends_with(".otf")
}

/// Reads every usable face out of `path`: the file itself if it's a
/// `.ttf`/`.otf`, or every `.ttf`/`.otf` entry inside it if it's a `.zip`.
/// Entries that don't parse as a font (a `.zip`'s README/license files, a
/// corrupt face) are silently skipped, not errors — only an empty result is.
/// A `.zip` shipping the same face twice (e.g. both `.ttf` and `.otf` of the
/// same weight, as real font distributions commonly do) keeps only the
/// first: registering both would just be two identical faces under one
/// (bold, italic) slot, which GPUI's own weight/style matching can't tell
/// apart anyway (see `main.rs`'s `assert_family_has_four_distinct_faces`).
fn candidate_faces(path: &Path) -> Result<Vec<Face>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let raw_faces: Vec<Vec<u8>> = match ext.as_str() {
        "zip" => {
            let file = std::fs::File::open(path).map_err(|e| format!("Couldn't open that file: {e}"))?;
            let mut archive =
                zip::ZipArchive::new(file).map_err(|e| format!("Not a valid .zip file: {e}"))?;
            let mut out = Vec::new();
            for i in 0..archive.len() {
                let mut entry = match archive.by_index(i) {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                if entry.is_dir() || !is_font_entry(entry.name()) {
                    continue;
                }
                let mut bytes = Vec::new();
                if std::io::Read::read_to_end(&mut entry, &mut bytes).is_ok() {
                    out.push(bytes);
                }
            }
            out
        }
        "ttf" | "otf" => {
            vec![std::fs::read(path).map_err(|e| format!("Couldn't open that file: {e}"))?]
        }
        _ => return Err("Unsupported file type — choose a .ttf, .otf, or .zip file.".to_string()),
    };

    let mut seen = std::collections::HashSet::new();
    let mut faces = Vec::new();
    for bytes in raw_faces {
        let Some(face) = parse_face(bytes) else { continue };
        if seen.insert((face.family.clone(), face.bold, face.italic)) {
            faces.push(face);
        }
    }
    if faces.is_empty() {
        return Err("No usable .ttf/.otf font found in that file.".to_string());
    }
    Ok(faces)
}

fn fonts_root() -> PathBuf {
    crate::recovery::app_data_dir().join("fonts")
}

fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect()
}

/// Registers every face's bytes with GPUI, measures the family's click-math
/// ratio (`text_editor::measure_advance_ratio`), and records it in the
/// runtime registry (`text_editor::register_imported_font`) so it renders
/// and appears in the picker immediately.
fn activate_family(cx: &App, family: &str, dir: PathBuf, faces: &[Face]) -> Result<(), String> {
    let font_bytes = faces.iter().map(|f| std::borrow::Cow::Owned(f.bytes.clone())).collect();
    cx.text_system().add_fonts(font_bytes).map_err(|e| format!("Couldn't load that font: {e}"))?;
    let ratio = crate::text_editor::measure_advance_ratio(cx, family);
    crate::text_editor::register_imported_font(family.to_string(), ratio, dir);
    Ok(())
}

/// One imported family's result: its name, and whether it actually shipped
/// a bold and/or italic face. `font_import_modal.rs` uses `has_bold`/
/// `has_italic` to warn the user up front — a family missing one renders
/// fine at its base weight, but pressing Bold/Italic on it won't visibly do
/// anything, since there's no such face to switch to (not a bug, just an
/// inherent limit of what the uploaded file contains).
pub struct FontImportOutcome {
    pub family: String,
    pub has_bold: bool,
    pub has_italic: bool,
}

/// One line per family missing a bold and/or italic face, or `None` if
/// everything imported shipped both. Phrased as a heads-up, not an error —
/// the import still succeeded.
pub fn missing_style_warning(outcomes: &[FontImportOutcome]) -> Option<String> {
    let lines: Vec<String> = outcomes
        .iter()
        .filter_map(|o| {
            let missing = match (o.has_bold, o.has_italic) {
                (true, true) => return None,
                (false, true) => "a bold",
                (true, false) => "an italic",
                (false, false) => "bold or italic",
            };
            Some(format!(
                "\u{201c}{}\u{201d} has no {missing} face — that button won't visibly change it.",
                o.family
            ))
        })
        .collect();
    if lines.is_empty() { None } else { Some(lines.join("\n")) }
}

/// Imports `path` (a `.ttf`, `.otf`, or `.zip`): parses it, writes each
/// distinct family's faces to disk, and activates them. Returns one
/// `FontImportOutcome` per family that became available. Re-importing the
/// same family overwrites its stored faces (matches "import" reading as
/// "make this the current version of that font", not "add a duplicate").
pub fn install_from_path(cx: &App, path: &Path) -> Result<Vec<FontImportOutcome>, String> {
    let faces = candidate_faces(path)?;

    let mut by_family: HashMap<String, Vec<Face>> = HashMap::new();
    for face in faces {
        by_family.entry(face.family.clone()).or_default().push(face);
    }

    let mut installed = Vec::new();
    for (family, faces) in by_family {
        let dir = fonts_root().join(slugify(&family));
        std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't save that font: {e}"))?;
        // Clear out a previous version's face files first — otherwise a
        // family that shrinks from 4 faces to 1 across two imports leaves
        // the stale extra files behind for `load_persisted` to pick back up.
        if let Ok(existing) = std::fs::read_dir(&dir) {
            for entry in existing.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        for (i, face) in faces.iter().enumerate() {
            let file = dir.join(format!("{}_{}_{}.font", face.bold, face.italic, i));
            std::fs::write(&file, &face.bytes).map_err(|e| format!("Couldn't save that font: {e}"))?;
        }
        let has_bold = faces.iter().any(|f| f.bold);
        let has_italic = faces.iter().any(|f| f.italic);
        activate_family(cx, &family, dir, &faces)?;
        installed.push(FontImportOutcome { family, has_bold, has_italic });
    }
    installed.sort_by(|a, b| a.family.cmp(&b.family));
    Ok(installed)
}

/// Re-registers every font previously imported, called once at startup
/// (`main.rs`, right after `load_bundled_fonts`) so imports persist across
/// restarts. Best-effort per family: a directory that fails to read/parse is
/// skipped rather than aborting the rest.
pub fn load_persisted(cx: &App) {
    let root = fonts_root();
    let Ok(entries) = std::fs::read_dir(&root) else { return };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        let faces: Vec<Face> = files
            .flatten()
            .filter_map(|f| std::fs::read(f.path()).ok())
            .filter_map(parse_face)
            .collect();
        let Some(family) = faces.first().map(|f| f.family.clone()) else { continue };
        let _ = activate_family(cx, &family, dir, &faces);
    }
}

/// Removes an imported font: drops it from the runtime registry (so it
/// leaves the picker and stops rendering immediately — see
/// `text_editor::unregister_imported_font`'s own doc comment for why the
/// GPUI-side face bytes themselves linger, harmlessly, until restart) and
/// deletes its face files from disk.
pub fn remove(name: &str) -> Result<(), String> {
    if let Some(dir) = crate::text_editor::unregister_imported_font(name) {
        std::fs::remove_dir_all(dir).map_err(|e| format!("Couldn't delete that font's files: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bebas_neue.zip")
    }

    #[test]
    fn test_candidate_faces_extracts_only_ttf_and_otf_from_zip() {
        let faces = candidate_faces(&fixture_path()).unwrap();
        // The real archive also ships .eot/.woff/.woff2/.md/.txt entries —
        // this asserts those are all filtered out, leaving exactly the one
        // (ttf, otf) pair collapsed to one face by the (family, bold,
        // italic) dedupe.
        assert_eq!(faces.len(), 1, "expected the .ttf/.otf duplicate to collapse to one face");
        assert_eq!(faces[0].family, "Bebas Neue");
        assert!(!faces[0].bold);
        assert!(!faces[0].italic);
    }

    #[test]
    fn test_candidate_faces_rejects_unsupported_extension() {
        let err = candidate_faces(Path::new("notes.txt")).unwrap_err();
        assert!(err.contains(".ttf"), "got: {err}");
    }

    #[test]
    fn test_candidate_faces_rejects_zip_with_no_fonts() {
        let dir = std::env::temp_dir().join("vimbatim_font_import_test_empty_zip");
        let _ = std::fs::create_dir_all(&dir);
        let zip_path = dir.join("empty.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer.start_file("README.md", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut writer, b"no fonts here").unwrap();
            writer.finish().unwrap();
        }
        let err = candidate_faces(&zip_path).unwrap_err();
        assert!(err.contains("No usable"), "got: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_slugify_is_filesystem_safe() {
        assert_eq!(slugify("Bebas Neue"), "bebas_neue");
        assert_eq!(slugify("Times New Roman"), "times_new_roman");
    }

    #[test]
    fn test_missing_style_warning_names_exactly_the_missing_faces() {
        let complete = FontImportOutcome { family: "Complete".to_string(), has_bold: true, has_italic: true };
        let no_bold = FontImportOutcome { family: "NoBold".to_string(), has_bold: false, has_italic: true };
        let no_italic = FontImportOutcome { family: "NoItalic".to_string(), has_bold: true, has_italic: false };
        let regular_only =
            FontImportOutcome { family: "RegularOnly".to_string(), has_bold: false, has_italic: false };

        assert_eq!(missing_style_warning(&[complete]), None);

        let warning = missing_style_warning(&[no_bold]).unwrap();
        assert!(warning.contains("NoBold") && warning.contains("bold"), "got: {warning}");

        let warning = missing_style_warning(&[no_italic]).unwrap();
        assert!(warning.contains("NoItalic") && warning.contains("italic"), "got: {warning}");

        let warning = missing_style_warning(&[regular_only]).unwrap();
        assert!(warning.contains("RegularOnly") && warning.contains("bold or italic"), "got: {warning}");

        // Multiple families, only the incomplete ones produce a line.
        let no_bold2 = FontImportOutcome { family: "NoBold".to_string(), has_bold: false, has_italic: true };
        let complete2 = FontImportOutcome { family: "Complete".to_string(), has_bold: true, has_italic: true };
        let warning = missing_style_warning(&[no_bold2, complete2]).unwrap();
        assert_eq!(warning.lines().count(), 1, "got: {warning}");
    }
}
