# How Foxit PDF Editor handles editing

2026-11-06. Read from Foxit's own help pages only; **Foxit wasn't installed or run**, so this describes what its documentation says. Most pages state no app version, and Windows and Mac pages are mixed below, so menu names may differ between them.

## Sources

1. [Organize PDF Pages, Mac user guide v13](https://help.foxit.com/manuals/pdf-editor/mac/en-us/v13/Organize_PDF_Pages.html) (the page was cut off near its end)
2. [Paragraph Editing, Windows, 2 Dec 2025](https://resource-cms.foxit.com/foxit-pdf-editor-for-windows/paragraph-editing)
3. [Reflow Editing (Pro Only), Windows, 5 Dec 2025](https://resource-cms.foxit.com/foxit-pdf-editor-for-windows/reflow-editing-pro-only)
4. [How to link and join text objects, blog, 8 Jul 2021](https://foxit.com/blog/how-to-link-and-join-text-objects)
5. [Edit text objects, Windows guide](https://www.foxit.com/resource-hub/user-guide/foxit-pdf-editor-for-windows/edit-text-objects/)
6. [Add, edit and rearrange pages, tutorial, 15 Aug 2026](https://resource-cms.foxit.com/tutorials/how-to-easily-add-edit-and-rearrange-pages-in-a-pdf-with-foxit-pdf-editor)
7. [Complex layouts and text reflow, tutorial, 9 May 2025](https://resource-cms.foxit.com/tutorials/how-to-edit-complex-pdf-layouts-and-use-text-reflow-in-foxit-pdf-editor)
8. [Foxit PDF Editor for Mac, product page, July 2026](https://www.foxit.com/phantompdf/edit-pdf-mac)

## What Foxit does

### Text comes in three levels

| Level | Reach | Behaviour | Source |
|---|---|---|---|
| **Edit Text** | One text block on one page | Edits "texts in a paragraph like Microsoft Office Word"; lines wrap "when the text comes to the margin" | 2, 3 |
| **Link and Join** | Linked blocks, also across pages | Text flows between linked blocks and doesn't cover what lies between them, but overflow past the last block covers what follows. Join merges blocks on one page into a paragraph | 4 |
| **Reflow Editing** | The whole document | "text will reflow automatically in pages". Its own mode, entered through a dialog; **Apply Changes** saves it and **Exit Reflow Editing** discards it. Labelled Pro only | 3 |

On a scanned PDF, Edit Text prompts to recognize the text first (2).

### Editing tools appear when needed
Edit Text and Reflow bring a Format tab into the right panel, and Link and Join bring a context tab to the ribbon (2, 3, 4). The overall interface is a "familiar ribbon-style interface" (8), which is the kind of permanent bar RectoPDF avoids.

### Fonts
Foxit says "you can add, paste or replace text only if the font used for that text is installed on your system" (5). The pages don't say what happens with missing characters or fonts that aren't embedded.

### Pages
- **Rearrange:** drag selected thumbnails, or use a Move dialog with a page range and destination. Swap also exists (1).
- **Add a PDF:** drag a file onto the Pages panel or Thumbnail window and "all the pages from the file will be inserted". The docs don't say where they land. Insert also takes files, folders, open files, a scanner, the clipboard or a blank page (1).
- **Between open documents:** drag thumbnails to "the target place in the Pages panel of the target file" (1).
- **Placement:** in Thumbnail View, an Insert icon sits before or after each thumbnail (1).
- **Rotate one page:** select it and use Rotate Pages, left or right (1).
- **Hover buttons:** rotate, delete and insert, when one page is selected (1).
- **Selection:** Command or Shift click; Command-drag duplicates (1).
- **Thumbnail View** hides all panels (1).

### Not covered by these pages
Undo, the effect of edits on file size, how the Mac Edit toolbar opens, and which edition includes Reflow.

## What RectoPDF takes from this

| Finding | Call | Why |
|---|---|---|
| Edit Text works on a whole text block, wrapping within it | **ADOPT.** Replaces our line-only plan | A line-only editor would be well below the baseline people know. It needs block detection and line breaking with the PDF's font metrics |
| Drop a PDF to add pages | **ADOPT-ADAPTED** | Ours shows where it will land, which Foxit's docs don't state |
| Drag pages between open documents | **ADOPT-ADAPTED** | Drop onto a tab in our sidebar |
| Hover buttons for rotate, delete, insert | **ADOPT** | Already drawn, plus insert |
| Insert point before or after a thumbnail | **ADOPT** | Precise and easy to find |
| Command-drag to duplicate; Command and Shift selection | **ADOPT** | Standard on the Mac |
| Prompt to recognize text on a scan | **ADOPT** | We already have OCR in scope |
| Tools appear only while editing | **KEEP OURS** | The same idea, in the sidebar rather than a ribbon |
| Thumbnail View hides all panels | **KEEP OURS** | Our sidebar stays, because it holds the edit tools |
| Fonts must be installed on the system | **KEEP OURS** | We reuse the PDF's own font and embed a fallback for missing characters, so no installed font is needed. Harder, and untested |
| Link and Join | Later | Useful, but only once block editing is solid |
| Document-wide reflow | Later, and unproven | Foxit makes it a separate mode with explicit Apply, which suggests how heavy it is |
