# Color Dropdown Menus Implementation Plan

## Goal
Add dropdown menus for Font Color and Highlight Color in the ribbon, allowing users to select from predefined colors instead of cycling through them.

## Current State
- FontColor button: applies black color only
- HighlightColorSelect button: cycles through colors via `state.cycle_highlight_color()`

## Implementation Strategy

### Approach: Dedicated Dropdown Rendering
Instead of using regular buttons, create custom dropdown menu rendering for colors.

### Required Changes

#### 1. Add State to Track Dropdown Menu Visibility
**File: src/formatting_ribbon.rs**

Add to FormattingRibbon struct:
```rust
pub struct FormattingRibbon {
    ...
    font_color_menu_open: bool,
    highlight_color_menu_open: bool,
}
```

Initialize in `new()`:
```rust
font_color_menu_open: false,
highlight_color_menu_open: false,
```

#### 2. Create Dropdown Color Button Renderer
**File: src/formatting_ribbon.rs**

Create method to render color dropdown button:
```rust
fn render_color_dropdown(
    label: &str,
    current_color: String,
    colors: &[(&str, u32)],
    menu_open: bool,
    on_click: impl Fn(&str, u32),
    cx: &mut Context<Self>
) -> impl IntoElement
```

The dropdown should:
- Show a colored square representing the current color
- Show a small dropdown arrow
- When clicked, toggle menu visibility
- When menu is open, display color options
- Each color option shows a colored square with label

#### 3. Add Color Selection Actions
**File: src/state.rs**

Add methods to apply specific colors:
```rust
pub fn apply_font_color(&mut self, color: ColorChoice) {
    // existing implementation
}

pub fn apply_highlight_color(&mut self, color_name: &str) {
    // apply specific highlight color
}
```

#### 4. Modify Ribbon Rendering
**File: src/formatting_ribbon.rs**

In the `render()` method, replace:
- FontColor button with dropdown menu
- HighlightColorSelect button with dropdown menu

In DOCUMENT section (line ~513):
- Replace FontColor button with color dropdown

In CARD FORMAT section (line ~523):
- Replace HL Color button with color dropdown

#### 5. Add Click Handlers
In the dropdown rendering, add handlers that:
- Toggle menu visibility when clicked
- Call the appropriate color application method when a color is selected
- Call `cx.notify()` to re-render

### Color Options

**Font Colors:**
- Black (0x000000) - default
- Red (0xFF0000)
- Blue (0x0000FF)
- Custom option (opens text input for hex)

**Highlight Colors:**
- Yellow (existing default)
- Green (existing)
- Blue (existing)
- Custom option (opens text input for hex)

### UI Layout

**Font Color Dropdown:**
```
┌─────────┐
│ ◼ ▼     │  "Font Color" button with colored square
└─────────┘
  │
  └─ Current color shown as small square
  └─ Arrow indicating dropdown
  
When open:
┌──────────────┐
│ ◼ Black      │
│ ◼ Red        │
│ ◼ Blue       │
│ ◼ Custom...  │
└──────────────┘
```

### Testing Strategy

1. Click Font Color button - menu should appear
2. Select Black - text color should change to black
3. Select Red - text color should change to red
4. Click away - menu should close
5. Same for Highlight Color dropdown
6. Test Custom color option opens input dialog

### Files to Modify
1. src/formatting_ribbon.rs - dropdown rendering and state
2. src/state.rs - color application methods (may already exist)
3. src/color_picker.rs - may need enhancements for UI display

### Potential Challenges
1. GPUI menu positioning - dropdown needs to appear below button
2. Click outside to close - need to handle clicking elsewhere to close menu
3. Custom color input - need to add text input field
4. State management - tracking which menu is open

### Implementation Order
1. Add menu visibility state to FormattingRibbon
2. Create dropdown rendering function
3. Create color menu rendering in render() method
4. Add click handlers for color selection
5. Test and refine styling
