# Tab Close and Reorder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the × button on each tab reliably close that tab, and allow tabs to be reordered by clicking and dragging them.

**Architecture:** Both features live entirely in `src/tab_bar.rs` and `src/state.rs`. The close fix adds mouse-down propagation stopping so drags on the × don't bleed to the parent tab. Reordering uses GPUI's built-in `on_drag` / `on_drop` API: a `TabDragPayload` struct (which itself implements `Render` to act as the drag ghost) carries the source index; each tab accepts a drop and calls a new `move_tab` method on `AppState`.

**Tech Stack:** Rust, GPUI (git dep from zed-industries/zed), no new dependencies.

## Global Constraints

- GPUI from git: `https://github.com/zed-industries/zed`, package `gpui`
- Rust edition 2021
- No new crate dependencies
- All GPUI API calls must be verified against the real source at `/root/.cargo/git/checkouts/zed-a70e2ad075855582/2c346f6/crates/gpui/src/`
- `on_drag` requires a stateful element (`.id()` called before it) — tab divs already have `.id(tab_id)`
- Drag payload type must implement `Render` (it IS the ghost view in GPUI's drag system)
- `cargo check` must pass with no new errors after each task

---

## File Map

| File | What changes |
|------|-------------|
| `src/state.rs` | Add `move_tab(from: usize, to: usize)` method to `AppState` |
| `src/tab_bar.rs` | Add `TabDragPayload` struct + `Render` impl; add `on_drag`, `on_drop`, `drag_over` to tab divs; add `on_mouse_down` stop-propagation to close button so it can't accidentally start a drag |

---

## Task 1: Fix the close button reliably

**Problem:** Clicking the × button can start a drag (mouse-down bubbles up to the parent tab div which has `on_drag`). Adding an `on_mouse_down` handler with `cx.stop_propagation()` on the × button prevents the mouse-down from reaching the tab's drag listener.

**Files:**
- Modify: `src/tab_bar.rs` (close button block, ~lines 125–148)

**Interfaces:**
- Produces: nothing new — purely a robustness fix

- [ ] **Step 1: Add `on_mouse_down` stop-propagation to the close button**

In `src/tab_bar.rs`, find the close button child div (the one with `.id(close_id)`) and add `.on_mouse_down` **before** the existing `.on_click`:

```rust
// Close button (×) — stop_propagation prevents the click from
// bubbling to the parent tab div's on_click (set_active_tab).
.child(
    div()
        .id(close_id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(16.0))
        .h(px(16.0))
        .rounded(px(2.0))
        .text_sm()
        .text_color(rgb(0x858585))
        .cursor(CursorStyle::PointingHand)
        // Stop mouse-down from bubbling to the parent tab div so it
        // cannot accidentally initiate a tab-drag via the × button.
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_click(cx.listener(move |this, _ev, _window, cx| {
            cx.stop_propagation();
            this.state.update(cx, |s, cx| {
                s.close_tab(idx);
                cx.notify();
            });
            cx.notify();
        }))
        .child("×"),
)
```

- [ ] **Step 2: Verify compile**

```bash
cargo check 2>&1 | grep "^error"
```
Expected: no output (no errors).

- [ ] **Step 3: Commit**

```bash
git add src/tab_bar.rs
git commit -m "fix: stop mouse-down propagation on tab close button"
```

---

## Task 2: Add `move_tab` to `AppState`

**Files:**
- Modify: `src/state.rs`

**Interfaces:**
- Produces: `AppState::move_tab(&mut self, from: usize, to: usize)` — swaps two tab positions and keeps `active_tab` pointing at the same logical tab.

- [ ] **Step 1: Add `move_tab` method to `AppState` in `src/state.rs`**

Add this method after `close_tab`:

```rust
pub fn move_tab(&mut self, from: usize, to: usize) {
    /*
     * Moves the tab at `from` to position `to`, shifting other tabs as needed.
     * Updates `active_tab` so the visually active tab does not change.
     */
    if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
        return;
    }
    let tab = self.tabs.remove(from);
    self.tabs.insert(to, tab);
    // Keep active_tab pointing at the same logical tab after the move.
    self.active_tab = if self.active_tab == from {
        to
    } else if from < self.active_tab && to >= self.active_tab {
        self.active_tab - 1
    } else if from > self.active_tab && to <= self.active_tab {
        self.active_tab + 1
    } else {
        self.active_tab
    };
}
```

- [ ] **Step 2: Verify compile**

```bash
cargo check 2>&1 | grep "^error"
```
Expected: no output.

- [ ] **Step 3: Commit**

```bash
git add src/state.rs
git commit -m "feat: add move_tab to AppState for tab reordering"
```

---

## Task 3: Implement tab drag-and-drop reordering

**Files:**
- Modify: `src/tab_bar.rs`

**Interfaces:**
- Consumes: `AppState::move_tab(from: usize, to: usize)` from Task 2
- Produces: draggable tab chips with a ghost overlay and highlighted drop zones

### Background: GPUI drag API

- `on_drag(value: T, constructor: |&T, Point<Pixels>, &mut Window, &mut App| -> Entity<W>)` — on a stateful element, initiates a drag when the user presses and moves. `value` is the payload; `constructor` produces the ghost entity (`W: Render`).
- `on_drop::<T>(listener)` — fires on any element when a drag of type `T` is released over it. Available on non-stateful elements too.
- `drag_over::<T>(|StyleRefinement, &T, &mut Window, &mut App| -> StyleRefinement)` — applies a style while a drag of type `T` is hovering over the element.
- The drag payload type must implement `Render` because GPUI uses the same value as the ghost view.

### Implementation

- [ ] **Step 1: Add `TabDragPayload` struct and its `Render` impl in `src/tab_bar.rs`**

Add this near the top of the file, after the `use` lines and the `actions!` macro:

```rust
/// Drag payload for tab reordering. Carries the source tab index and title.
/// Implements `Render` because GPUI uses the payload value as the ghost view
/// that floats under the cursor while dragging.
#[derive(Clone)]
struct TabDragPayload {
    from_idx: usize,
    title: String,
    /// Cursor offset within the dragged tab at the moment drag started.
    /// Used to position the ghost so it doesn't jump away from the cursor.
    offset: Point<Pixels>,
}

impl Render for TabDragPayload {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Render at the cursor offset so the ghost tracks the mouse naturally.
        div()
            .pl(self.offset.x)
            .pt(self.offset.y)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(36.0))
                    .px(px(12.0))
                    .bg(rgb(0x1e1e1e))
                    .text_sm()
                    .text_color(rgb(0xd4d4d4))
                    .border_1()
                    .border_color(rgb(0x569cd6))
                    .shadow_md()
                    .child(self.title.clone()),
            )
    }
}
```

- [ ] **Step 2: Verify `TabDragPayload` compiles alone**

```bash
cargo check 2>&1 | grep "^error"
```
Expected: no output.

- [ ] **Step 3: Add `on_drag`, `on_drop`, and `drag_over` to each tab div**

In the `tab_elements` iterator, the tab div currently ends with `.on_click(...)`. Replace the entire tab div construction with:

```rust
div()
    .id(tab_id)
    .flex()
    .flex_row()
    .items_center()
    .h_full()
    .px(px(12.0))
    .gap(px(8.0))
    .bg(tab_bg)
    .cursor_pointer()
    .border_r_1()
    .border_color(rgb(0x464647))
    .when(!is_active, |d| d.border_b_1().border_color(rgb(0x464647)))
    // Highlight this tab's left edge when a dragged tab hovers over it.
    .drag_over::<TabDragPayload>(|style, _, _, _| {
        style.border_l_2().border_color(rgb(0x569cd6))
    })
    // Receive a dropped tab — reorder it into this position.
    .on_drop(cx.listener(move |this, payload: &TabDragPayload, _window, cx| {
        if payload.from_idx != idx {
            this.state.update(cx, |s, cx| {
                s.move_tab(payload.from_idx, idx);
                cx.notify();
            });
            cx.notify();
        }
    }))
    // Click tab body → switch to this tab (fires only when not dragging).
    .on_click(cx.listener(move |this, _ev, _window, cx| {
        this.state.update(cx, |s, cx| {
            s.set_active_tab(idx);
            cx.notify();
        });
        cx.notify();
    }))
    // Begin drag — carry the source index and title as payload.
    // The constructor clones the payload and sets the cursor offset so
    // the ghost view positions itself under the cursor correctly.
    .on_drag(
        TabDragPayload { from_idx: idx, title: title.clone(), offset: Point::default() },
        cx.listener(move |_this, payload: &TabDragPayload, offset, _window, cx| {
            let ghost = TabDragPayload {
                from_idx: payload.from_idx,
                title: payload.title.clone(),
                offset,
            };
            cx.new(|_| ghost)
        }),
    )
    // Tab title label
    .child(
        div()
            .text_sm()
            .text_color(tab_text)
            .child(title),
    )
    // Close button (×) — see Task 1 for the full block with stop_propagation
    .child(
        div()
            .id(close_id)
            .flex()
            .items_center()
            .justify_center()
            .w(px(16.0))
            .h(px(16.0))
            .rounded(px(2.0))
            .text_sm()
            .text_color(rgb(0x858585))
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(move |this, _ev, _window, cx| {
                cx.stop_propagation();
                this.state.update(cx, |s, cx| {
                    s.close_tab(idx);
                    cx.notify();
                });
                cx.notify();
            }))
            .child("×"),
    )
```

**Note on `on_drag` constructor signature:** The `cx.listener` wrapper for `on_drag` uses a four-argument closure: `|this: &mut TabBar, payload: &TabDragPayload, offset: Point<Pixels>, window: &mut Window, cx: &mut Context<TabBar>|`. If the compiler complains about argument count, use the non-listener form instead (no access to `this` needed):

```rust
.on_drag(
    TabDragPayload { from_idx: idx, title: title.clone(), offset: Point::default() },
    |payload: &TabDragPayload, offset, _window, cx| {
        let ghost = TabDragPayload {
            from_idx: payload.from_idx,
            title: payload.title.clone(),
            offset,
        };
        cx.new(|_| ghost)
    },
)
```

- [ ] **Step 4: Add `Point` to the gpui imports at the top of `tab_bar.rs`**

`Point` is already re-exported by `use gpui::*;` so no import change should be needed. Verify with:

```bash
cargo check 2>&1 | grep "^error"
```

If `Point` is not found, add it explicitly: change `use gpui::*;` to also import `use gpui::Point;` (it is already glob-imported, so this is unlikely to be needed).

- [ ] **Step 5: Verify full compile**

```bash
cargo check 2>&1 | grep -E "^error"
```
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add src/tab_bar.rs src/state.rs
git commit -m "feat: draggable tab reordering with ghost preview and drop highlight"
```

---

## Verification Checklist (manual, after build)

Run `./run.sh` then:

1. **Close tab:** Open 2+ tabs. Click × on a non-active tab → that tab closes, others unchanged.
2. **Close active tab:** Click × on the active tab → active tab closes, an adjacent tab becomes active.
3. **Can't drag-start from ×:** Press and hold × then move mouse → no drag ghost appears; release still closes the tab.
4. **Tab drag ghost:** Press and hold on a tab's title area, then move → a semi-transparent ghost of the tab title appears under the cursor.
5. **Drop highlight:** While dragging, hover over another tab → its left border turns blue.
6. **Reorder:** Drag Tab A over Tab B and release → tabs swap positions, the dragged tab is now active.
7. **Click still works:** A quick click (no movement) on a non-active tab → switches to that tab; no drag occurs.
