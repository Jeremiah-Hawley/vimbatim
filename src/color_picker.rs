use gpui::prelude::*;
use gpui::*;

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

pub fn color_button(color: ColorChoice) -> impl IntoElement {
    let hex = color.hex_value();
    let r = (hex >> 16) & 0xFF;
    let g = (hex >> 8) & 0xFF;
    let b = hex & 0xFF;

    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .p(px(4.0))
        .rounded(px(2.0))
        .bg(rgb(hex))
        .text_color(if (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) > 128.0 {
            rgb(0x000000)
        } else {
            rgb(0xFFFFFF)
        })
        .child(color.label())
}
