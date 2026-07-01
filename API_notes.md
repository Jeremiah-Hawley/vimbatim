# GPUI API Notes

Notes from building the Vimbatim GUI against the Zed GPUI crate (git dependency, ~mid-2025).
Source lives at `crates/gpui` inside the Zed monorepo.

---

## Cargo setup

```toml
[dependencies]
gpui          = { git = "https://github.com/zed-industries/zed", package = "gpui" }
gpui_platform = { git = "https://github.com/zed-industries/zed", package = "gpui_platform" }
```

`gpui_platform` provides the OS-specific backend (`application()` constructor).
Without it you have to wire up the platform layer yourself.

---

## Launching the app

```rust
use gpui::prelude::*;
use gpui::*;
use gpui_platform::application;

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("My App".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| RootView::new(cx)),
        )
        .unwrap();
        cx.activate(true); // bring window to front (required on macOS)
    });
}
```

`open_window` callback signature: `|window: &mut Window, cx: &mut App| -> Entity<V>`.

---

## Core types

| Old name (pre-2025) | Current name | Notes |
|---------------------|--------------|-------|
| `View<T>` | `Entity<T>` | unified handle for views and data models |
| `Model<T>` | `Entity<T>` | same type — no distinction anymore |
| `ViewContext<T>` | `Context<T>` | derefs to `App` |
| `ModelContext<T>` | `Context<T>` | same type |
| `AppContext` | `App` | the top-level context |

`Entity<T>` is cheap to clone (reference-counted). Pass it around freely.

---

## Creating entities

```rust
// Inside a Context<Self> (e.g. in another view's new() or render())
let data_entity = cx.new(|_cx| MyDataStruct::new());
let view_entity = cx.new(|cx| MyView::new(data_entity.clone(), cx));

// Inside open_window callback (cx: &mut App)
let root = cx.new(|cx| RootView::new(cx));
```

Both views and plain data structs use the same `cx.new(...)` call.
If your struct implements `Render`, it acts as a view.

---

## Reading and writing entity state

```rust
// Read (immutable borrow of T)
let value = entity.read(cx).some_field;

// Update (mutable + notify)
entity.update(cx, |state, cx| {
    state.some_field = new_value;
    cx.notify(); // trigger re-render of all views watching this entity
});
```

`cx` here can be `&mut Context<T>`, `&mut App`, or any type that implements `AppContext`.

---

## The Render trait

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child("hello")
    }
}
```

The signature takes **three** parameters: `&mut self`, `&mut Window`, `&mut Context<Self>`.
Earlier versions only took two — this is a common source of compile errors when following old docs.

---

## Actions and keybindings

```rust
// 1. Declare action types (zero-sized structs)
actions!(my_namespace, [OpenSettings, ToggleSidebar]);

// 2. Register keybindings globally (in the run() callback)
cx.bind_keys([
    KeyBinding::new("ctrl-,", OpenSettings, None),
    KeyBinding::new("ctrl-b", ToggleSidebar, None),
]);

// 3. Handle actions in a view's render()
div()
    .on_action(cx.listener(Self::handle_open_settings))

// 4. Handler method signature
fn handle_open_settings(&mut self, _: &OpenSettings, _window: &mut Window, cx: &mut Context<Self>) {
    // do stuff
    cx.notify();
}
```

Actions bubble up the focus tree, so register handlers on the outermost div that should catch them.

---

## cx.listener — accessing view state in callbacks

```rust
// cx.listener wraps a closure so it can access &mut Self
div()
    .on_click(cx.listener(|this, _ev, _window, cx| {
        this.some_field = true;
        cx.notify();
    }))
```

`this` is `&mut Self` (the current view).
`cx` is `&mut Context<Self>`.
`window` is `&mut Window`.

The returned closure is `'static` — it captures a `WeakEntity<Self>` internally.
Call `cx.listener` as many times as needed in one render; each call is a separate capture.

---

## Div basics

```rust
use gpui::prelude::*; // required for when(), children(), FluentBuilder, etc.

div()
    .flex()
    .flex_col()          // or .flex_row()
    .flex_1()            // flex-grow: 1
    .min_h_0()           // critical inside flex children to prevent height overflow
    .size_full()         // width: 100%; height: 100%
    .w(px(240.0))        // fixed width
    .h(px(36.0))         // fixed height
    .bg(rgb(0x1e1e1e))
    .text_color(rgb(0xd4d4d4))
    .text_sm()           // also: text_xs, text_lg, text_xl
    .font_family("monospace")
    .font_weight(FontWeight::BOLD)
    .p(px(16.0))         // padding all sides; also: .px(), .py(), .pt(), etc.
    .gap(px(8.0))
    .border_1()
    .border_color(rgb(0x464647))
    .border_b_1()        // bottom border only; also: _t_, _l_, _r_
    .rounded(px(8.0))
    .cursor_pointer()    // also: cursor_text(), cursor_default(), etc.
    .items_center()      // align-items: center
    .justify_center()    // justify-content: center
    .justify_between()   // justify-content: space-between
    .relative()          // position: relative (positioning context for absolute children)
    .absolute()          // position: absolute
    .top_0()             // top: 0; also .left_0(), .right_0(), .bottom_0()
    .shadow_lg()         // box-shadow large; also .shadow_sm(), .shadow_md()
    .child("text")       // append a child element
    .children(iter)      // append multiple children from an iterator
    .when(bool, |d| d.child(...))          // conditional child/style
    .when_some(opt, |d, val| d.child(...)) // conditional on Option
```

`when` and `when_some` come from the `FluentBuilder` trait, available via `use gpui::prelude::*`.

---

## Stateful elements — required for on_click and overflow_scroll

Calling `.id(...)` on a `Div` promotes it to `Stateful<Div>`, which is required for:
- `.on_click(...)`
- `.overflow_y_scroll()` / `.overflow_scroll()`
- Any other method on `StatefulInteractiveElement`

**`.id()` must be the first method called** — it changes the type from `Div` to `Stateful<Div>`.

```rust
div()
    .id("my-button")          // promotes to Stateful<Div>
    .on_click(|ev, _win, cx| { ... })  // now available

div()
    .id("scroll-area")
    .overflow_y_scroll()      // now available
```

`ElementId` accepts several types:
```rust
.id("static-str")
.id(42usize)               // From<usize>
.id(some_string)           // From<String>
.id(ElementId::named_usize("tab", idx))  // namespaced usize
.id(ElementId::from(path_string))        // From<String>
```

---

## on_click vs on_mouse_down

```rust
// on_click — requires .id(), fires on full press+release
div()
    .id("btn")
    .on_click(|ev: &ClickEvent, window, cx| { ... })

// on_mouse_down — no id required, fires on press
div()
    .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, cx| { ... })
```

`ClickEvent` is an enum — use the method, not direct field access:
```rust
ev.click_count()  // 1 = single click, 2 = double click, etc.
ev.modifiers()    // Modifiers held during click
ev.position()     // Point<Pixels>
```

---

## Keyboard focus

```rust
// In a view constructor
let focus_handle = cx.focus_handle();

// In render() — attach to element
div()
    .id("editor")
    .track_focus(&self.focus_handle)
    .key_context("EditorCtx")   // optional: scopes keybinding context
    .on_key_down(cx.listener(Self::handle_key))

// Focus programmatically (needs both Window and App)
focus_handle.focus(window, cx);           // method on FocusHandle
// or
cx.focus_view(&entity, window);           // focus an Entity<T: Focusable>

// Check focus state in render
let is_focused = self.focus_handle.is_focused(window);
```

Implement `Focusable` to let external code focus your view:
```rust
impl Focusable for MyView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
```

---

## KeyDownEvent

```rust
fn handle_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
    let ks = &event.keystroke;
    let key: &str = ks.key.as_str();   // "a", "enter", "backspace", "space", etc.

    // Modifier fields on Modifiers struct:
    ks.modifiers.control   // Ctrl
    ks.modifiers.shift     // Shift
    ks.modifiers.alt       // Alt / Option
    ks.modifiers.platform  // Cmd on macOS, Win key on Windows, Super on Linux
    ks.modifiers.function  // Fn key
    // NOTE: there is no .command field — it was renamed to .platform
}
```

---

## Colors

```rust
rgb(0xRRGGBB)           // Rgb from hex
rgba(0xRRGGBBAA)        // Rgba from hex (alpha in last byte)
black()                 // Hsla constant
white()
red() / green() / blue() / yellow()

// Opacity on any color type
black().opacity(0.55)   // returns Hsla with alpha applied
rgb(0xff0000).opacity(0.5)
```

---

## Returning mixed element types from a function

When a function returns `impl IntoElement` but the branches return different concrete types
(e.g. `Div` vs `Stateful<Div>`), call `.into_any_element()` on each branch to unify them:

```rust
fn render_node(node: &Node) -> AnyElement {
    match node {
        Node::Dir { .. } => div().flex()./* ... */.into_any_element(),
        Node::File { .. } => div().id("file").on_click(/* ... */).into_any_element(),
    }
}
```

---

## Absolute overlay (modal pattern)

To render a floating modal over all other content:

```rust
// Parent container must be .relative()
div()
    .relative()
    .size_full()
    .child(/* main content */)
    .when(modal_visible, |d| {
        d.child(
            div()
                .absolute()
                .top_0().left_0().right_0().bottom_0()
                .flex().items_center().justify_center()
                .bg(black().opacity(0.55))
                // backdrop click closes modal
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _ev, w, cx| {
                    this.close(w, cx);
                }))
                .child(
                    div()
                        .id("modal-panel")
                        .w(px(440.0))
                        .bg(rgb(0x2d2d2d))
                        .rounded(px(8.0))
                        .shadow_lg()
                        // stop click propagating to backdrop
                        .on_mouse_down(MouseButton::Left, |_ev, _w, _cx| {})
                        .child(/* modal content */)
                )
        )
    })
```

---

## Common pitfalls

| Mistake | Fix |
|---------|-----|
| Using `View<T>`, `Model<T>`, `ViewContext` | Use `Entity<T>` and `Context<T>` |
| `render(&mut self, cx)` with 2 params | Must be `render(&mut self, window: &mut Window, cx: &mut Context<Self>)` |
| `on_click` on a plain `div()` | Add `.id(...)` before `.on_click(...)` |
| `overflow_y_scroll()` on a plain `div()` | Add `.id(...)` first |
| `drop(entity.read(cx))` | `entity.read(cx)` returns a borrow, not an owned value; use `let _ = val;` or just let it drop at end of scope |
| `modifiers.command` | Field is now `modifiers.platform` |
| `ev.up.click_count` on `ClickEvent` | Use `ev.click_count()` (method on the enum) |
| `App::new()` | Use `gpui_platform::application()` |
| `cx.new_view(...)` | Use `cx.new(...)` |
| `cx.new_model(...)` | Use `cx.new(...)` |
| `cx.focus(&handle)` | Use `handle.focus(window, cx)` or `cx.focus_view(&entity, window)` |
| Forgetting `use gpui::prelude::*` | Required for `when`, `children`, `FluentBuilder`, `InteractiveElement`, etc. |
