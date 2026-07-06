# Potential Hotkeys for Vimbatim

Review and prioritize these common hotkeys from Word and VSCode for implementation.

## Already Implemented
- Ctrl+, : Settings modal ✓
- Ctrl+B : Toggle file explorer sidebar ✓
- Ctrl+T : New tab ✓
- Ctrl+W : Close tab ✓
- Ctrl+Z : Undo ✓
- Ctrl+Y : Redo ✓
- Ctrl+Shift+Z : Redo (alternate) ✓
- Ctrl+A : Select all ✓
- Ctrl+C : Copy ✓
- Ctrl+X : Cut ✓
- Ctrl+V : Paste ✓
- Ctrl+B : Bold ✓
- Ctrl+I : Italic ✓
- Vim motions (hjkl, w, b, e, f, F, t, T, etc.) ✓

## File Operations (Word-style)
- **Ctrl+N** : New document (could open new tab)
- **Ctrl+O** : Open file dialog
- **Ctrl+S** : Save current file (currently on window close)
- **Ctrl+Shift+S** : Save As / Save with different name
- **Ctrl+P** : Print document

## Navigation & Search (VSCode + Word hybrid)
- **Ctrl+F** : Find in current document (vi already has /, but single key is faster)
- **Ctrl+H** : Find and Replace (not currently available)
- **Ctrl+G** : Go to line/page number
- **Ctrl+Home** : Go to start of document
- **Ctrl+End** : Go to end of document
- **F5** : Navigator/Outline view (like VSCode's Go to Symbol)
- **Ctrl+Shift+F** : Find in all open tabs/documents (if multi-document)

## Text Formatting Shortcuts
- **Ctrl+U** : Underline toggle (currently only via ribbon)
- **Ctrl+Shift+U** : Remove formatting / Clear formatting (like Ctrl+M in some editors)
- **Ctrl+Shift+X** : Strikethrough toggle (currently only via ribbon)
- **Ctrl++** : Increase font size
- **Ctrl+-** : Decrease font size
- **Ctrl+]** : Increase indent
- **Ctrl+[** : Decrease indent

## Paragraph Formatting (Word-style)
- **Ctrl+E** : Center align
- **Ctrl+L** : Left align
- **Ctrl+R** : Right align
- **Ctrl+J** : Justify
- **Ctrl+1** : Single spacing
- **Ctrl+2** : Double spacing
- **Ctrl+5** : 1.5 spacing
- **Ctrl+0** : Paragraph spacing toggle

## Line Operations (VSCode-style)
- **Ctrl+Shift+K** : Delete entire line (dangerous - needs confirmation)
- **Ctrl+Shift+Enter** : Insert line above
- **Ctrl+Enter** : Insert line below (we have vim 'o')
- **Alt+Up** : Move line up
- **Alt+Down** : Move line down
- **Ctrl+D** : Duplicate line

## Card Style Shortcuts (Vimbatim-specific)
- **Ctrl+Shift+P** : Apply Pocket format (instead of ribbon)
- **Ctrl+Shift+H** : Apply Hat format
- **Ctrl+Shift+B** : Apply Block format
- **Ctrl+Shift+T** : Apply Tag format
- **Ctrl+Shift+C** : Apply Cite format

## Highlight/Color Shortcuts (Vimbatim-specific)
- **Ctrl+Alt+Y** : Highlight Yellow
- **Ctrl+Alt+G** : Highlight Green  
- **Ctrl+Alt+B** : Highlight Blue
- **Ctrl+Alt+C** : Highlight Custom
- **Ctrl+Alt+X** : Remove Highlight

## Window/View Management
- **Ctrl+Tab** : Next tab / Next window (we have vim gt)
- **Ctrl+Shift+Tab** : Previous tab (we have vim gT)
- **Ctrl+Shift+V** : Split view side-by-side (we have ribbon button)
- **Ctrl+J** : Toggle bottom panel (for properties/formatting options)

## Miscellaneous Useful
- **Ctrl+,** : Settings (already implemented)
- **Ctrl+K Ctrl+C** : Toggle comment (not applicable to docs)
- **F2** : Rename (for tab rename?)
- **F3** / **Ctrl+G** : Find next
- **Shift+F3** / **Ctrl+Shift+G** : Find previous
- **Escape** : Clear search / Close menus

## Vim-Specific Extensions (not Word/VSCode but useful)
- **:set number** : Show line numbers (toggle)
- **:set relativenumber** : Relative line numbers
- **gg** : Go to beginning (already have)
- **G** : Go to end (already have)
- **:%s/find/replace/g** : Find & replace (already available via vim)

## Priority Assessment (Recommended for Implementation)

### HIGH PRIORITY (Frequently used, significant UX improvement)
- [ ] Ctrl+F : Find (faster than vim /)
- [ ] Ctrl+H : Find & Replace (very useful, not in vim)
- [ ] Ctrl+G : Go to line/page (navigation)
- [ ] Alt+Up/Down : Move line up/down (common in modern editors)
- [ ] Ctrl+S : Save file (muscle memory from Word)
- [ ] Ctrl+Shift+K : Delete line (VSCode users expect this)

### MEDIUM PRIORITY (Useful but can be learned through ribbon/vim)
- [ ] Ctrl+Shift+P/H/B : Card style shortcuts (faster than ribbon)
- [ ] Ctrl+Alt+Y/G/B : Highlight color shortcuts
- [ ] Ctrl+U : Underline (formatting muscle memory)
- [ ] Ctrl+[ / Ctrl+] : Indent control
- [ ] Ctrl+E/L/R : Align shortcuts

### LOW PRIORITY (Nice to have, less critical)
- [ ] Ctrl+N : New document
- [ ] Ctrl+O : Open file dialog
- [ ] Ctrl+P : Print
- [ ] Ctrl+J : Toggle panel
- [ ] F5 : Navigator/Outline

## Notes for Implementation

1. **Conflict Resolution:**
   - Ctrl+B already bound to toggle sidebar (Word uses it for Bold, which we have via Vim)
   - Ctrl+J not yet used (could be useful)
   - Ctrl+F could coexist with vim's / (provide both)

2. **Vim Mode Considerations:**
   - Users in vim mode may prefer vim commands (`:` based)
   - These hotkeys help non-vim users
   - Could make some optional via settings

3. **Platform-Specific:**
   - Consider Cmd instead of Ctrl for Mac users
   - Alt vs Option on Mac

4. **Implementation Approach:**
   - Start with HIGH priority (6 items)
   - Test with actual users
   - Add MEDIUM priority if positive feedback
   - Leave LOW priority for future
