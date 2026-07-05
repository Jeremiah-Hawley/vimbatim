# Bug Fix Plan: Card Styles on Empty Lines

## Problem
When clicking Pocket/Hat/Block on an empty line, only center alignment is applied. The bold, font size, and special formatting (box/underline/double-underline) appear to be ignored.

## Root Cause
In `apply_formatting_to_line()` (state.rs:763), the formatting IS being applied to the empty line's paragraph. However, the formatting isn't visible because the run is empty (no text to display).

More critically, `apply_formatting_to_line()` doesn't set `pending_format`, unlike `apply_formatting_to_selection()` (state.rs:800). When the user types after applying card style formatting:
- `insert_char()` checks for `pending_format` to apply retroactively to newly typed characters
- Since `pending_format` is not set, the new characters don't inherit the formatting
- Only the center alignment persists (because it's paragraph-level, not run-level)

## Solution
Modify `apply_formatting_to_line()` to set `pending_format` when formatting is applied to an empty line. This mirrors the behavior in `apply_formatting_to_selection()` for the no-selection case.

### Implementation Details

**File: src/state.rs, function apply_formatting_to_line()**

1. After applying formatting to the line, check if the line is empty:
   ```rust
   let is_line_empty = line_start >= line_end;
   ```

2. If the line is empty AND formatting was applied (not toggled off), set pending_format:
   ```rust
   if is_line_empty && !toggled {
       if let Some(tab) = self.tabs.get_mut(self.active_tab) {
           tab.pending_format = Some(effective_op);
       }
   }
   ```

3. Special handling for center-alignment-only formatting:
   - Card styles apply both run-level formatting AND paragraph-level alignment
   - Only the run-level formatting needs to go into `pending_format` (alignment applies at paragraph level)
   - Extract just the bold/size/box/underline/double-underline part for `pending_format`

### Changes Required

**src/state.rs - apply_formatting_to_line() method**

After line 796 (after `apply_formatting()` call):

```rust
// If applying to an empty line, arm pending_format so subsequent typing gets the formatting
if is_line_empty {
    // Determine which operation to set as pending (the run-level formatting, not alignment)
    let pending_op = match &effective_op {
        FormatOp::Box(_) | FormatOp::Bold(_) | FormatOp::FontSize(_) | 
        FormatOp::Underline(_) | FormatOp::DoubleUnderline(_) => Some(effective_op.clone()),
        _ => None,
    };
    
    if let Some(op) = pending_op {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.pending_format = Some(op);
        }
    }
}
```

## Testing Strategy

1. **Test case 1: Empty line card style application**
   - Click on empty line
   - Click Pocket button
   - Type text
   - Expected: Text should be bold, size 26, centered, with box

2. **Test case 2: Non-empty line (should not set pending)**
   - Type "Hello"
   - With cursor in middle of text
   - Click Pocket button
   - Type "!"
   - Expected: Only "Hello" has Pocket formatting, "!" does not

3. **Test case 3: Multiple card styles**
   - Test same behavior with Hat and Block

4. **Test case 4: Toggle behavior**
   - Apply Pocket to empty line
   - Type text (should be Pocket formatted)
   - Click Pocket again on that text
   - Type more (should NOT be Pocket formatted - toggled off)

## Expected Outcome
After applying card style formatting to an empty line, subsequently typed characters will inherit the formatting, making the card style fully visible and functional immediately.
