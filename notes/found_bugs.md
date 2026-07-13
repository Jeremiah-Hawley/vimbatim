bugfixes_and_packaging# File Purpos0
This file contains descriptions of bugs I found while testing, visual bugs are just visual and don't impact functionality, functionality bugs are bugs that do impact functionality, and Forgitten Implicit Features are features that I meant to have inlcuded but didn't explicitly refer to them so they're small changes that need to be added. the order of priority for editing is
1) Functionality bugs
2) Forgotten Implicit Feaetures
3) Visual Bugs

When fixing bugs, if there is an lack of clarity ask the user to clarify the issue or how they'd like it fixed.
## Visual bugs


## Functionality bugs
### Slow when editing pre-existing .docx files
Curently editing docx files that have been loaded from another file takes far longer than creating and editing a new file from scratch. I would asome this has something to do with .docx parsing or saving, remember that it only matters if the docx portion of whats on screen is written properly at save time, so you should be able to move a lot of the computational workload to the saving process which is less time restricted than the editing process

## Forgotten Implicit Features


