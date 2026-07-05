# Ongoing Bug Fixes

## Pocket Box Merging Not Working

**Issue:** When pressing Enter on a Pocket line, the boxes should merge (second box has no top border), but this is still not working correctly.

**Current Implementation:**
- In text_editor.rs render_line(), check if `prev_has_box` is true
- If true, render box with only bottom/left/right borders: `border_b_1().border_l_1().border_r_1()`
- If false, render full box: `border_1()`

**Possible Root Causes & Ideas to Test:**

1. **GPUI Border API Limitation**
   - The border_b_1(), border_l_1(), border_r_1() approach might not work in GPUI
   - GPUI might not support selective border removal/application
   - Idea: Check GPUI source/docs for the correct API to render borders on specific sides
   - Alternative: Use custom styling or a different border approach

2. **Border Call Ordering**
   - The method chaining order might matter
   - Maybe setting border_color() after border_b_1() resets it
   - Idea: Try setting border_color() before the specific border calls
   - Idea: Try building the border configuration differently

3. **Box Wrapper Structure**
   - The current approach wraps the line_div in a box
   - The line_div itself might have conflicting border/styling
   - Idea: Check if line_div has any borders or styling applied
   - Idea: Inspect the full div hierarchy to see what's rendering

4. **Detection Logic Issue**
   - The prev_has_box detection might not be working correctly
   - Maybe li is not the correct paragraph index
   - Maybe paragraphs.get(li - 1) is not returning the right paragraph
   - Idea: Add debug logging to verify prev_has_box is being set correctly
   - Idea: Check if wrapped rows vs logical lines affects the index

5. **Rendering Context Issue**
   - The boxes might need to be rendered as a group instead of individually
   - The margin/padding between rows might be preventing visual merging
   - Idea: Check if there's spacing between rendered rows
   - Idea: Consider if boxes need to be merged at a higher level (not line-by-line)

6. **Alternative Implementation**
   - Instead of removing the top border, could add negative margin to second box
   - Could render both boxes as a single element when detected
   - Could use a different visual indicator for merged boxes
   - Idea: Try adding mb(px(-1.0)) to second box to overlap borders
   - Idea: Render merged boxes as a single div containing both lines

**Testing Strategy:**
1. Enable visual debugging to see what borders are actually rendering
2. Log the prev_has_box value for each Pocket line
3. Check if the selective border calls are even being invoked
4. Test if GPUI supports the border API being used
5. Verify the paragraph index (li) is correct for the previous line

**Next Steps:**
- Run the app and visually confirm what's currently rendering
- Check GPUI documentation for correct border API
- If selective borders don't work, try the negative margin approach
- Consider if this needs a higher-level rendering refactor
