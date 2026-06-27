# TOP LEVEL
Use the GPUI (gpui.rs) crate for all GUI tasks

Create the following things, each bullet point has further instructions below
 - Main Text Editoy
 - File Explorer Side Bar
 - Formatting Ribbon
 - toggleable floating settings

follow the three documentation rules:
1) When writing code, for each function have a large multi-line comment right underneath it's declaration line describing what it does.
2) When writing code, comment for any line of code that isn't self explanatory.
3) when you're done, add an in-depth description of what you did to tmp_documentation.md

## Main Text Editor
**NOTE: you should not make this be able to edit .docx files now, just make it extendable to do that in the future**
The majority of the screen should be takken up by an area to write to .docx files, it should work the way that you'd expect from a text editor (like VSCode) or a word editor (like Word) rolled into one.

## Tab Menu
On the top of the GUI window should be a tab system, each tab should either be a "new tab" that prompts the user to open a file, or a tab that contains a .docx file for the user to edit, think about how tabs work in Obsidian with .md files, but for .docx.

## File Explorer Side Bar
This should be a collapsable side bar on the right that shows the user the current folder that they opened the application in, and each .docx file in that directory tree (similar to VSCode).
The user should also be able to create new files and open files into new tabs by double clicking on them.

## Formatting Ribbon
underneath the tab menu, and above the main text editor should be a ribbon containing settings for formatting text in the word file, this should be inspired by the formatting ribbon in Microsoft Word. You do not need to impliment the formatting buttons now, but you should have it be extendable for future additions, and have atleast a 2x2 grid of buttons for demonstration.

## Toggleable floating settings
you should have it so that when the user presses a settings keybind declared in settings.conf (don't worry, about parsing the file now, just use a placeholder) a centered floating window should open for the user to change certain settings, you don't need to impliment this now, just have the window open, and have a button that prints to console when clicked for a demonstration.

