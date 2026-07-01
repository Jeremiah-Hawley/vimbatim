use std::cell::Cell;
use std::rc::Rc;

use gpui::{point, px, App, Entity, Pixels, Point, ScrollHandle, Window};

use crate::state::AppState;
use crate::text_editor::{document_lines, line_col_from_mouse_position, visual_rows_for_viewport};

/// How close a click-drag has to get to the top/bottom of the viewport
/// before auto-scroll kicks in.
const EDGE_MARGIN_PX: f32 = 24.0;
/// How far to scroll per tick (either a real `on_mouse_move` event or a
/// re-armed animation frame) while a drag sits in the trigger zone.
const SCROLL_STEP_PX: f32 = 12.0;

fn auto_scroll_delta(mouse_y: f32, viewport_top: f32, viewport_height: f32, edge_margin: f32, scroll_step: f32) -> f32 {
    /*
     * Returns how much to adjust the scroll offset by this tick, when a
     * drag's mouse position sits within `edge_margin` pixels of the top or
     * bottom of the viewport: `scroll_step` (scroll up, i.e. reveal content
     * above) near the top edge, `-scroll_step` (scroll down) near the
     * bottom edge, or 0.0 outside both trigger zones. The caller adds this
     * to the current scroll offset and clamps it with `clamp_scroll_offset`.
     */
    if viewport_height <= 0.0 { return 0.0; }
    let viewport_bottom = viewport_top + viewport_height;
    if mouse_y < viewport_top + edge_margin {
        scroll_step
    } else if mouse_y > viewport_bottom - edge_margin {
        -scroll_step
    } else {
        0.0
    }
}

fn clamp_scroll_offset(offset_y: f32, max_offset_y: f32) -> f32 {
    /*
     * Clamps a proposed scroll offset to GPUI's valid range for
     * `ScrollHandle::set_offset`: never positive (that would scroll past
     * the top of the document), never more negative than `max_offset_y`
     * (that would scroll past the bottom). `max_offset_y.max(0.0)` guards
     * against a not-yet-laid-out handle reporting a negative max.
     */
    offset_y.clamp(-max_offset_y.max(0.0), 0.0)
}

/// Drives auto-scroll for a click-drag selection that sits near the top or
/// bottom edge of the editor's viewport.
///
/// `on_mouse_move` alone isn't enough: it only fires while the mouse is
/// actually moving, but the natural "keep scrolling" gesture is to drag to
/// the edge and hold still. `AutoScroller` re-arms itself once per frame via
/// `Window::on_next_frame` while a drag is parked in the trigger zone, so
/// scrolling continues independent of further mouse movement, and stops the
/// instant the mouse leaves the zone or `stop()` is called (drag ended).
///
/// All fields are shared handles (`Rc`/`ScrollHandle`/`Entity`), so cloning
/// an `AutoScroller` — which the internal frame-rescheduling does — doesn't
/// duplicate any state; every clone drives the same underlying editor.
#[derive(Clone)]
pub struct AutoScroller {
    scroll_handle: ScrollHandle,
    state: Entity<AppState>,
    last_mouse_position: Rc<Cell<Point<Pixels>>>,
    running: Rc<Cell<bool>>,
}

impl AutoScroller {
    pub fn new(scroll_handle: ScrollHandle, state: Entity<AppState>) -> Self {
        /*
         * Constructs an idle AutoScroller. `scroll_handle` should be the
         * same handle the owning TextEditor tracks via `.track_scroll()`,
         * so `.bounds()`/`.offset()` here always reflect the editor's real,
         * current viewport and scroll position rather than a stale copy —
         * and, since `.bounds()` is GPUI's own layout bounds for that div
         * (computed before scroll translation, not a hand-rolled capture),
         * it can't drift with scroll position the way a separately tracked
         * bounds value could.
         */
        AutoScroller {
            scroll_handle,
            state,
            last_mouse_position: Rc::new(Cell::new(Point::default())),
            running: Rc::new(Cell::new(false)),
        }
    }

    pub fn notify(&self, position: Point<Pixels>, window: &mut Window) {
        /*
         * Call on every `on_mouse_move` while a drag is active. Records the
         * latest mouse position (used by ticks that fire with no new mouse
         * event), and starts the per-frame tick loop if the position is
         * within the edge trigger zone and a loop isn't already running —
         * an already-running loop picks up the new position on its own on
         * the next tick, so this doesn't re-arm.
         */
        self.last_mouse_position.set(position);
        if self.running.get() { return; }
        let bounds = self.scroll_handle.bounds();
        let delta = auto_scroll_delta(
            position.y.as_f32(),
            bounds.origin.y.as_f32(),
            bounds.size.height.as_f32(),
            EDGE_MARGIN_PX,
            SCROLL_STEP_PX,
        );
        if delta != 0.0 {
            self.running.set(true);
            self.arm(window);
        }
    }

    pub fn stop(&self) {
        /*
         * Stops any active tick loop. Call on mouse-up (both the "released
         * over the editor" and "released elsewhere" cases — see the
         * on_mouse_up/on_mouse_up_out wiring in text_editor.rs) so a drag
         * that ends while parked in the edge zone doesn't keep scrolling
         * forever with no way to stop it.
         */
        self.running.set(false);
    }

    fn arm(&self, window: &Window) {
        let this = self.clone();
        window.on_next_frame(move |window, cx| this.tick(window, cx));
    }

    fn tick(&self, window: &mut Window, cx: &mut App) {
        /*
         * Runs once per animation frame while active. Recomputes the
         * auto-scroll delta fresh from the last known mouse position each
         * time (rather than trusting a stored value), so a scroll that
         * carries the mouse out of the trigger zone — or a `stop()` call —
         * is picked up on the very next tick without needing a separate
         * signal. Re-arms itself for the next frame only if still active.
         */
        if !self.running.get() { return; }
        let bounds = self.scroll_handle.bounds();
        let position = self.last_mouse_position.get();
        let delta = auto_scroll_delta(
            position.y.as_f32(),
            bounds.origin.y.as_f32(),
            bounds.size.height.as_f32(),
            EDGE_MARGIN_PX,
            SCROLL_STEP_PX,
        );
        if delta == 0.0 {
            self.running.set(false);
            return;
        }

        let current = self.scroll_handle.offset();
        let max_y = self.scroll_handle.max_offset().y.as_f32();
        let new_y = clamp_scroll_offset(current.y.as_f32() + delta, max_y);
        self.scroll_handle.set_offset(point(current.x, px(new_y)));

        let scroll_y = self.scroll_handle.offset().y.as_f32();
        let content = self.state.read(cx).active_content().to_string();
        let lines = document_lines(&content);
        let rows = visual_rows_for_viewport(cx, &lines, bounds.size.width.as_f32());
        let (line, col) = line_col_from_mouse_position(position, bounds, scroll_y, &rows);
        self.state.update(cx, |state, cx| {
            state.extend_selection_to_line_col(line, col);
            cx.notify();
        });

        self.arm(window);
    }
}

#[cfg(test)]
mod tests {
    // Import only the functions under test, not `super::*` — this module
    // pulls in gpui types, and gpui exports its own `test` attribute macro
    // that shadows std's `#[test]` if brought into scope here (see the same
    // note in text_editor.rs's test module).
    use super::{auto_scroll_delta, clamp_scroll_offset};

    // ── auto_scroll_delta ────────────────────────────────────────────────────

    #[test]
    fn test_auto_scroll_delta_middle_of_viewport_is_zero() {
        assert_eq!(auto_scroll_delta(100.0, 0.0, 200.0, 20.0, 12.0), 0.0);
    }

    #[test]
    fn test_auto_scroll_delta_near_top_edge_scrolls_up() {
        assert_eq!(auto_scroll_delta(10.0, 0.0, 200.0, 20.0, 12.0), 12.0);
    }

    #[test]
    fn test_auto_scroll_delta_near_bottom_edge_scrolls_down() {
        assert_eq!(auto_scroll_delta(195.0, 0.0, 200.0, 20.0, 12.0), -12.0);
    }

    #[test]
    fn test_auto_scroll_delta_at_top_margin_boundary_is_zero() {
        // Exactly at the margin boundary: not yet inside the trigger zone.
        assert_eq!(auto_scroll_delta(20.0, 0.0, 200.0, 20.0, 12.0), 0.0);
    }

    #[test]
    fn test_auto_scroll_delta_at_bottom_margin_boundary_is_zero() {
        assert_eq!(auto_scroll_delta(180.0, 0.0, 200.0, 20.0, 12.0), 0.0);
    }

    #[test]
    fn test_auto_scroll_delta_respects_viewport_top_offset() {
        // Viewport starts at y=50 (not 0); top edge trigger zone is [50, 70).
        assert_eq!(auto_scroll_delta(55.0, 50.0, 200.0, 20.0, 12.0), 12.0);
    }

    #[test]
    fn test_auto_scroll_delta_zero_height_viewport_is_zero() {
        assert_eq!(auto_scroll_delta(10.0, 0.0, 0.0, 20.0, 12.0), 0.0);
    }

    // ── clamp_scroll_offset ──────────────────────────────────────────────────

    #[test]
    fn test_clamp_scroll_offset_within_range_unchanged() {
        assert_eq!(clamp_scroll_offset(-50.0, 100.0), -50.0);
    }

    #[test]
    fn test_clamp_scroll_offset_positive_clamps_to_zero() {
        // Offset can never be positive — that would scroll past the top.
        assert_eq!(clamp_scroll_offset(10.0, 100.0), 0.0);
    }

    #[test]
    fn test_clamp_scroll_offset_past_max_clamps_to_max() {
        assert_eq!(clamp_scroll_offset(-150.0, 100.0), -100.0);
    }

    #[test]
    fn test_clamp_scroll_offset_no_scrollable_content_forces_zero() {
        assert_eq!(clamp_scroll_offset(-10.0, 0.0), 0.0);
    }
}
