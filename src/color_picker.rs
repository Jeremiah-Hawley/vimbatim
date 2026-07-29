use gpui::prelude::*;
use gpui::*;

use crate::theme::{radius, space, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorChoice {
    Black,
    Red,
    Blue,
    Custom(u32), // RGB as hex
}

impl ColorChoice {
    pub fn hex_value(&self) -> u32 {
        match self {
            ColorChoice::Black => 0x000000,
            ColorChoice::Red => 0xFF0000,
            ColorChoice::Blue => 0x0000FF,
            ColorChoice::Custom(hex) => *hex,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ColorChoice::Black => "Black",
            ColorChoice::Red => "Red",
            ColorChoice::Blue => "Blue",
            ColorChoice::Custom(_) => "Custom",
        }
    }
}

/// `Hsla` -> packed `0xRRGGBB`. gpui already owns the conversion math
/// (`From<Hsla> for Rgba`); this only packs the channels.
pub fn hsla_to_hex(color: Hsla) -> u32 {
    let rgba = Rgba::from(color);
    let to_byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    (to_byte(rgba.r) << 16) | (to_byte(rgba.g) << 8) | to_byte(rgba.b)
}

/// Packed `0xRRGGBB` -> `Hsla`, again via gpui's own conversion.
pub fn hex_to_hsla(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

/// Parses a pasted or typed color. Accepts surrounding whitespace and one
/// optional leading `#`; everything else must be exactly six hex digits.
pub fn parse_hex(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// Which strip a drag started on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerDrag {
    SatLight,
    Hue,
}

/// State for the interactive custom-color picker: an HSL saturation/lightness
/// square, a hue strip, and a hex field, all views onto one `Hsla`.
pub struct CustomColorPicker {
    /// The single source of truth. `hex_text` is derived from it on every drag.
    pub color: Hsla,
    /// What the hex field shows. Diverges from `color` only while the user is
    /// mid-typing an incomplete value.
    pub hex_text: String,
    /// On-screen bounds of each strip, captured in prepaint by a `canvas`
    /// child — GPUI has no other way to ask an element where it ended up.
    pub(crate) sv_bounds: Bounds<Pixels>,
    pub(crate) hue_bounds: Bounds<Pixels>,
    pub(crate) drag: Option<PickerDrag>,
}

impl CustomColorPicker {
    pub fn new() -> Self {
        CustomColorPicker {
            color: hex_to_hsla(0xFFD700),
            hex_text: "FFD700".to_string(),
            sv_bounds: Bounds::default(),
            hue_bounds: Bounds::default(),
            drag: None,
        }
    }

    pub fn hex(&self) -> u32 {
        hsla_to_hex(self.color)
    }

    /// Jumps the picker to `hex` — used when the user types or pastes one.
    pub fn set_color(&mut self, hex: u32) {
        self.color = hex_to_hsla(hex);
        self.sync_hex_text();
    }

    pub fn set_sat_light(&mut self, pos: Point<Pixels>) {
        let s = fraction(pos.x, self.sv_bounds.origin.x, self.sv_bounds.size.width);
        let l = 1.0 - fraction(pos.y, self.sv_bounds.origin.y, self.sv_bounds.size.height);
        self.color = hsla(self.color.h, s, l, 1.0);
        self.sync_hex_text();
    }

    pub fn set_hue(&mut self, pos: Point<Pixels>) {
        let h = fraction(pos.x, self.hue_bounds.origin.x, self.hue_bounds.size.width);
        self.color = hsla(h, self.color.s, self.color.l, 1.0);
        self.sync_hex_text();
    }

    fn sync_hex_text(&mut self) {
        self.hex_text = format!("{:06X}", self.hex());
    }
}

/// Where `pos` falls along a strip, as 0.0..=1.0. Clamped, so dragging past
/// either end saturates instead of wrapping, and a zero-width strip (one that
/// hasn't been painted yet) reads 0.0 rather than NaN.
fn fraction(pos: Pixels, origin: Pixels, size: Pixels) -> f32 {
    if size.as_f32() <= 0.0 {
        return 0.0;
    }
    ((pos.as_f32() - origin.as_f32()) / size.as_f32()).clamp(0.0, 1.0)
}

/// Black or white, whichever reads against `hex`. Standard perceived-brightness
/// weighting; extracted from `color_button` so it can be unit tested (the
/// button itself returns an opaque `impl IntoElement`).
pub fn contrast_text(hex: u32) -> u32 {
    let r = ((hex >> 16) & 0xFF) as f32;
    let g = ((hex >> 8) & 0xFF) as f32;
    let b = (hex & 0xFF) as f32;
    if r * 0.299 + g * 0.587 + b * 0.114 > 128.0 {
        0x000000
    } else {
        0xFFFFFF
    }
}

/// One swatch row: a block of `hex` with `label` written on it in a readable
/// color. The label is passed in rather than derived, because the same hex
/// means different things in different menus — `0x0000FF` is "Blue" in the HL
/// Color menu and "#0000FF" as a saved custom color.
pub fn color_button(hex: u32, label: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .p(px(4.0))
        .rounded(px(2.0))
        .bg(rgb(hex))
        .text_color(rgb(contrast_text(hex)))
        .child(label.into())
}

/// The picker's visuals. Concretely typed to `FormattingRibbon` because that's
/// the only view that owns one; generalize only if a second one appears.
pub fn render_picker(
    picker: &CustomColorPicker,
    p: Palette,
    cx: &mut Context<crate::formatting_ribbon::FormattingRibbon>,
) -> AnyElement {
    let hue = picker.color.h;
    let current = picker.hex();
    let entity = cx.entity();

    // Saturation/lightness square. No color math: the base is the pure hue,
    // one horizontal overlay desaturates leftward, and two stacked half-height
    // overlays lighten upward and darken downward. That's HSL space, and it
    // maps straight onto `set_sat_light`'s (s, 1 - l).
    let sv_entity = entity.clone();
    let square = div()
        .id("picker-sv")
        .relative()
        .w(px(200.0))
        .h(px(120.0))
        .rounded(px(radius::SM))
        .bg(hsla(hue, 1.0, 0.5, 1.0))
        .cursor_pointer()
        .child(div().absolute().inset_0().bg(linear_gradient(
            90.0,
            linear_color_stop(hsla(0.0, 0.0, 0.5, 1.0), 0.0),
            linear_color_stop(hsla(0.0, 0.0, 0.5, 0.0), 1.0),
        )))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(60.0))
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(hsla(0.0, 0.0, 1.0, 1.0), 0.0),
                    linear_color_stop(hsla(0.0, 0.0, 1.0, 0.0), 1.0),
                )),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .right_0()
                .h(px(60.0))
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(hsla(0.0, 0.0, 0.0, 0.0), 0.0),
                    linear_color_stop(hsla(0.0, 0.0, 0.0, 1.0), 1.0),
                )),
        )
        // Marker.
        .child(
            div()
                .absolute()
                .left(px(200.0 * picker.color.s - 5.0))
                .top(px(120.0 * (1.0 - picker.color.l) - 5.0))
                .w(px(10.0))
                .h(px(10.0))
                .rounded(px(5.0))
                .border_2()
                .border_color(rgb(contrast_text(current))),
        )
        // Bounds capture. No `cx.notify()` here — that would re-render forever.
        .child(
            canvas(
                move |bounds, _window, cx| {
                    sv_entity.update(cx, |this, _cx| this.picker.sv_bounds = bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.picker.drag = Some(PickerDrag::SatLight);
                this.picker.set_sat_light(ev.position);
                cx.notify();
            }),
        );

    // Hue strip: six segments, each a gradient between adjacent 60-degree
    // anchors. `Hsla.h` is 0..1, so the anchors are i/6.
    let hue_entity = entity.clone();
    let hue_strip = div()
        .id("picker-hue")
        .relative()
        .flex()
        .flex_row()
        .w(px(200.0))
        .h(px(14.0))
        .rounded(px(radius::SM))
        .overflow_hidden()
        .cursor_pointer()
        .children((0..6).map(|i| {
            div().flex_1().h_full().bg(linear_gradient(
                90.0,
                linear_color_stop(hsla(i as f32 / 6.0, 1.0, 0.5, 1.0), 0.0),
                linear_color_stop(hsla((i + 1) as f32 / 6.0, 1.0, 0.5, 1.0), 1.0),
            ))
        }))
        .child(
            div()
                .absolute()
                .left(px(200.0 * hue - 2.0))
                .top_0()
                .w(px(4.0))
                .h_full()
                .border_1()
                .border_color(rgb(0xFFFFFF)),
        )
        .child(
            canvas(
                move |bounds, _window, cx| {
                    hue_entity.update(cx, |this, _cx| this.picker.hue_bounds = bounds);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                this.picker.drag = Some(PickerDrag::Hue);
                this.picker.set_hue(ev.position);
                cx.notify();
            }),
        );

    // Move/up live on the picker root so a drag keeps tracking when the cursor
    // leaves the strip it started on.
    //
    // ponytail: a drag that leaves the whole picker stops updating until it
    // comes back (values clamp, nothing breaks). Promote these to window-level
    // listeners only if that actually annoys someone.
    div()
        .id("custom-color-picker")
        .flex()
        .flex_col()
        .gap(px(space::XS))
        .p(px(space::XS))
        .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _window, cx| {
            match this.picker.drag {
                Some(PickerDrag::SatLight) => this.picker.set_sat_light(ev.position),
                Some(PickerDrag::Hue) => this.picker.set_hue(ev.position),
                None => return,
            }
            cx.notify();
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                if this.picker.drag.take().is_some() {
                    cx.notify();
                }
            }),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _ev: &MouseUpEvent, _window, cx| {
                if this.picker.drag.take().is_some() {
                    cx.notify();
                }
            }),
        )
        .child(square)
        .child(hue_strip)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(space::XS))
                .child(div().w(px(20.0)).h(px(20.0)).rounded(px(radius::SM)).bg(rgb(current)))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(p.text))
                        .child(format!("#{}", picker.hex_text)),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Import only what's under test, not `super::*` — color_picker.rs has
    // `use gpui::*;` at module scope, and gpui exports its own `test`
    // attribute macro (for async GPUI tests) that shadows std's `#[test]` and
    // sends the test-attribute expansion into infinite recursion if it's in
    // scope here. Same reason as `text_editor.rs`'s test module.
    use super::{contrast_text, hex_to_hsla, hsla_to_hex, parse_hex, CustomColorPicker};
    use gpui::{point, px, Bounds, Pixels, Size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: Size { width: px(w), height: px(h) },
        }
    }

    #[test]
    fn test_hex_hsla_round_trip() {
        for hex in [0xFF0000, 0x00FF00, 0x0000FF, 0x000000, 0xFFFFFF, 0x808080, 0x00FF88, 0xFFD700]
        {
            assert_eq!(hsla_to_hex(hex_to_hsla(hex)), hex, "round trip failed for {hex:06X}");
        }
    }

    #[test]
    fn test_set_color_syncs_hex_text() {
        let mut picker = CustomColorPicker::new();
        picker.set_color(0x00FF88);
        assert_eq!(picker.hex_text, "00FF88");
        assert_eq!(picker.hex(), 0x00FF88);
    }

    #[test]
    fn test_set_sat_light_maps_position_to_saturation_and_lightness() {
        let mut picker = CustomColorPicker::new();
        picker.sv_bounds = bounds(100.0, 100.0, 200.0, 100.0);
        picker.set_color(0xFF0000); // hue 0

        // Top-right corner: full saturation, full lightness => white.
        picker.set_sat_light(point(px(300.0), px(100.0)));
        assert!((picker.color.s - 1.0).abs() < 0.01);
        assert!((picker.color.l - 1.0).abs() < 0.01);

        // Bottom-left corner: no saturation, no lightness => black.
        picker.set_sat_light(point(px(100.0), px(200.0)));
        assert!(picker.color.s.abs() < 0.01);
        assert!(picker.color.l.abs() < 0.01);

        // Middle.
        picker.set_sat_light(point(px(200.0), px(150.0)));
        assert!((picker.color.s - 0.5).abs() < 0.01);
        assert!((picker.color.l - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_set_sat_light_clamps_outside_the_square() {
        let mut picker = CustomColorPicker::new();
        picker.sv_bounds = bounds(100.0, 100.0, 200.0, 100.0);
        picker.set_sat_light(point(px(-500.0), px(-500.0)));
        assert_eq!(picker.color.s, 0.0);
        assert_eq!(picker.color.l, 1.0);

        picker.set_sat_light(point(px(9999.0), px(9999.0)));
        assert_eq!(picker.color.s, 1.0);
        assert_eq!(picker.color.l, 0.0);
    }

    #[test]
    fn test_set_hue_maps_position_and_preserves_sat_light() {
        let mut picker = CustomColorPicker::new();
        picker.hue_bounds = bounds(0.0, 0.0, 360.0, 12.0);
        picker.sv_bounds = bounds(0.0, 0.0, 100.0, 100.0);
        picker.set_sat_light(point(px(50.0), px(50.0)));
        let (s, l) = (picker.color.s, picker.color.l);

        picker.set_hue(point(px(180.0), px(6.0)));
        assert!((picker.color.h - 0.5).abs() < 0.01);
        assert_eq!(picker.color.s, s);
        assert_eq!(picker.color.l, l);
    }

    #[test]
    fn test_zero_sized_bounds_do_not_divide_by_zero() {
        let mut picker = CustomColorPicker::new();
        picker.sv_bounds = bounds(0.0, 0.0, 0.0, 0.0);
        picker.set_sat_light(point(px(10.0), px(10.0)));
        assert!(picker.color.s.is_finite() && picker.color.l.is_finite());
    }

    #[test]
    fn test_parse_hex_accepts_valid_forms() {
        assert_eq!(parse_hex("00ff88"), Some(0x00ff88));
        assert_eq!(parse_hex("#00FF88"), Some(0x00ff88));
        assert_eq!(parse_hex("  #00ff88  "), Some(0x00ff88));
        assert_eq!(parse_hex("#00ff88\n"), Some(0x00ff88));
    }

    #[test]
    fn test_parse_hex_rejects_everything_else() {
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#"), None);
        assert_eq!(parse_hex("00ff8"), None, "5 digits");
        assert_eq!(parse_hex("00ff888"), None, "7 digits");
        assert_eq!(parse_hex("zzzzzz"), None);
        assert_eq!(parse_hex("##00ff88"), None);
        assert_eq!(parse_hex("rgb(0,255,136)"), None);
    }

    #[test]
    fn test_contrast_text_picks_black_on_light_colors() {
        assert_eq!(contrast_text(0xFFFFFF), 0x000000);
        assert_eq!(contrast_text(0xFFD700), 0x000000); // yellow
        assert_eq!(contrast_text(0x00FF00), 0x000000); // green
    }

    #[test]
    fn test_contrast_text_picks_white_on_dark_colors() {
        assert_eq!(contrast_text(0x000000), 0xFFFFFF);
        assert_eq!(contrast_text(0x0000FF), 0xFFFFFF); // blue
        assert_eq!(contrast_text(0x8B0000), 0xFFFFFF); // darkRed
    }
}
