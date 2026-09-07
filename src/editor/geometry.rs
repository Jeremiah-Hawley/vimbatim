pub(crate) struct ScrollbarGeometry {
    pub thumb_h: f32,
    pub travel: f32,
    pub thumb_top: f32,
}

pub(crate) fn scrollbar_geometry(
    viewport_h: f32,
    content_h: f32,
    scrolled: f32,
    min_thumb_px: f32,
) -> ScrollbarGeometry {
    let thumb_h = (viewport_h * (viewport_h / content_h))
        .max(min_thumb_px)
        .min(viewport_h);
    let travel = viewport_h - thumb_h;
    let max_scroll = content_h - viewport_h;
    let thumb_top = if max_scroll > 0.0 {
        (scrolled / max_scroll).clamp(0.0, 1.0) * travel
    } else {
        0.0
    };
    ScrollbarGeometry {
        thumb_h,
        travel,
        thumb_top,
    }
}
