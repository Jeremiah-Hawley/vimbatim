# File Purpose
This file contains descriptions of bugs I found while testing
## Visual bugs


## Functionality bugs
### File Saving
When creating a new file in the text editor, after writing text in it, saving just returns "Save failed: tab has no parsed document", this likely requires two things to be changed
1) When creating a new .docx file it needs to do more than create an empty file with the .docx extension, it needs to create a zip file containing multiple xml files formatted the way word would and text should be put in the document.xml file which is the one that contains document text
2) Parsing a file shouldn't be a requirement for saving a file, after creating a file in vimbatim, and then saving that file it should just create a new file with the properties in the editor rather than failing

### Directory and Document Searching
In the file explorer on the right, it only updates available files and folders on open, so if a file or folder is created in another application (like Microsoft File Explorer, or Microsoft Word) it will not show until vimbatim is restarted.



