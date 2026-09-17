/// Effective text/background colors, also inverted by the Vim block cursor.
pub(crate) fn run_colors(
    run: Option<&crate::docx_parser::Run>,
    text: u32,
    background: u32,
) -> (u32, u32) {
    let foreground = run
        .and_then(|r| r.color.as_deref())
        .and_then(|c| u32::from_str_radix(c, 16).ok())
        .unwrap_or(text);
    let background = match run.filter(|r| r.highlight) {
        Some(run) => {
            let base = highlight_color_hex(&run.highlight_color);
            if is_light_color(base) && is_light_color(foreground) {
                darken_for_light_text(base)
            } else {
                base
            }
        }
        None => background,
    };
    (foreground, background)
}

pub(crate) fn highlight_color_hex(name: &str) -> u32 {
    /*
     * Maps Word's highlight color names to their GPUI hex value (spec
     * 6.2's 15-entry table, plus a fallback for anything unrecognized).
     * Falls back to parsing `name` as a raw 6-digit hex string before
     * giving up — the HL Color dropdown's Custom option stores colors
     * this way since there's no name for an arbitrary RGB value.
     */
    match name {
        "yellow" => 0xFFD700,
        "green" => 0x00FF00,
        "blue" => 0x0000FF,
        "cyan" => 0x00FFFF,
        "magenta" => 0xFF00FF,
        "red" => 0xFF0000,
        "darkBlue" => 0x00008B,
        "darkCyan" => 0x008B8B,
        "darkGreen" => 0x006400,
        "darkMagenta" => 0x8B008B,
        "darkRed" => 0x8B0000,
        "darkYellow" => 0x8B8B00,
        "darkGray" => 0xA9A9A9,
        "lightGray" => 0xD3D3D3,
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        _ => u32::from_str_radix(name, 16).unwrap_or(0x888888),
    }
}

pub(crate) fn relative_luminance(hex: u32) -> f32 {
    /*
     * Standard perceived-luminance weighting (ITU-R BT.709 coefficients),
     * used to decide whether a color reads as "light" (spec: bug fix,
     * darken highlight under light text, matching Word's dark-mode
     * behavior).
     */
    let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
    let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
    let b = (hex & 0xFF) as f32 / 255.0;
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub(crate) fn is_light_color(hex: u32) -> bool {
    relative_luminance(hex) > 0.5
}

pub(crate) fn darken_for_light_text(hex: u32) -> u32 {
    /*
     * Scales each channel down uniformly (preserving hue) so a light
     * highlight color stops washing out light-colored text on top of it.
     */
    const SCALE: f32 = 0.4;
    let r = (((hex >> 16) & 0xFF) as f32 * SCALE) as u32;
    let g = (((hex >> 8) & 0xFF) as f32 * SCALE) as u32;
    let b = ((hex & 0xFF) as f32 * SCALE) as u32;
    (r << 16) | (g << 8) | b
}
