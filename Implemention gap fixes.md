TASK: COMPLETE UI/UX, INPUT, FILE OPERATIONS, AND PREVIEW OVERHAUL
FOR THE RUST WINDOWS FILE MANAGER

Do not apply isolated patches.

Audit the entire application architecture and fix all related issues across:
- main.rs
- state.rs
- ui.rs
- preview.rs
- overlay/native preview integration
- Cargo.toml/dependencies
- any supporting modules

The goal is to turn the current prototype into a polished, responsive,
mouse-first + keyboard-friendly Windows file manager.

============================================================
1. CRITICAL: FIX MOUSE INTERACTION
============================================================

Mouse interaction is currently fundamentally incomplete.

Implement proper hit-testing for every visible UI region.

LEFT CLICK:
- Clicking a file/folder row must select exactly that row.
- Clicking a row in another directory column must activate that column.
- Selection must update preview immediately.
- Clicking empty space must not accidentally select another item.
- Clicking preview must not alter directory selection.
- Clicking breadcrumb segments should navigate to that directory.
- Clicking drive entries should navigate correctly.

DOUBLE CLICK:
- Folder -> open folder.
- File -> open with Windows default application.
- Drive -> open drive.
- Double-click detection must be reliable and not conflict with single-click.

RIGHT CLICK:
- Select item under cursor first.
- Open context menu at mouse position.
- Support:
  Open
  Open With
  Cut
  Copy
  Paste
  Rename
  Delete
  Properties
  Copy Path
  Open in Terminal

MOUSE WHEEL:
- Scroll the pane currently under the mouse.
- Do NOT always move the current selection.
- Directory wheel -> scroll directory.
- Preview wheel -> scroll preview.
- Image preview -> optionally zoom with Ctrl+wheel.

DIVIDER DRAGGING:
- Keep existing resizable columns but fix hitboxes.
- Show resize cursor/visual indicator if possible.
- Prevent accidental divider drag when selecting nearby files.
- Clamp minimum/maximum pane widths.
- Persist sensible ratios during navigation.

============================================================
2. CRITICAL: IMPLEMENT REAL RENAME
============================================================

Rename currently needs to behave like a real file manager.

Implement F2 rename.

Requirements:
- F2 starts inline rename on selected file/folder.
- Show editable filename directly over/in selected row.
- For files, select basename by default but not extension.
- Arrow/Home/End/Delete/Backspace work normally.
- Ctrl+A selects filename text.
- Enter commits.
- Esc cancels.
- Clicking elsewhere commits or cancels consistently.
- Validate Windows-invalid filename characters.
- Prevent empty names.
- Handle duplicate destination names.
- Display filesystem errors visibly.
- Preserve selection after rename.
- Refresh directory immediately.
- Preview must update to renamed path.

Do NOT confuse rename with the current text-file "edit mode".

Create explicit application modes/state:
Normal
Rename
ContextMenu
Search
ConfirmDelete
TextEdit (only if genuinely implemented)

============================================================
3. FILE OPERATIONS
============================================================

Implement proper Windows file-manager operations:

Ctrl+C -> Copy
Ctrl+X -> Cut
Ctrl+V -> Paste
Delete -> Delete with confirmation
Shift+Delete -> permanent delete with stronger confirmation
F2 -> Rename
Ctrl+Shift+N -> New Folder
Enter -> Open
Alt+Enter -> Properties

Support:
- files
- folders
- multiple selection eventually
- collision handling
- copy/move progress
- filesystem errors
- permission errors
- locked files

After every mutation:
- invalidate directory caches
- refresh affected panes
- maintain selection intelligently
- refresh preview

Never leave stale cached directory contents after rename/delete/create/paste.

============================================================
4. REMOVE ANSI HALF-BLOCK IMAGE RENDERING
============================================================

The current image preview using characters such as:

▀

with foreground/background RGB colors is NOT acceptable as the primary
image renderer.

Do not render normal images as ANSI/Unicode pixel approximations.

Implement a high-quality graphics preview backend.

Preferred behavior:
- detect terminal capabilities
- use native terminal graphics protocols where supported:
  - Kitty graphics protocol
  - Sixel where supported
  - iTerm-style inline images where applicable
- for Windows Terminal, investigate and use the best supported image
  rendering strategy
- if terminal-native high-quality rendering is impossible, use a native
  overlay/window rendering surface aligned with the preview pane

Images must appear:
- sharp
- correct aspect ratio
- centered
- non-pixelated
- no stretched geometry
- no ANSI half-block artifacts

Support:
JPG/JPEG
PNG
GIF
WEBP
BMP
TIFF
ICO

GIF:
- animate GIFs rather than showing only a bad static representation
  when the rendering backend supports animation.

Controls:
+/- zoom
0 fit
1 actual size
R rotate
F flip
mouse wheel zoom where appropriate
drag/pan when zoomed

Cache decoded/resized images intelligently.

============================================================
5. REAL VIDEO PREVIEW
============================================================

Do NOT treat video preview as merely a static ffmpeg thumbnail.

Implement a proper video preview experience.

At minimum show:
- high-quality frame/thumbnail
- filename
- resolution
- duration
- codec
- frame rate
- file size

Preferred:
- embedded/native playback surface
- play/pause
- seek
- volume/mute
- timeline
- current time / total duration

Supported formats where codecs permit:
MP4
MKV
MOV
AVI
WEBM
WMV
M4V

If embedded playback is impossible inside the TUI:
create/use a native preview overlay aligned with the preview pane.

Never block the main UI while decoding video.

============================================================
6. AUDIO PREVIEW
============================================================

Improve audio preview.

Show:
- album art
- title
- artist
- album
- duration
- bitrate
- sample rate
- channels
- format

Optional playback:
- play/pause
- seek
- volume
- timeline

Use a polished media-card layout instead of raw metadata lines.

============================================================
7. PDF PREVIEW
============================================================

PDF preview should visually render pages.

Do NOT primarily dump extracted text.

Implement:
- rendered first page
- page navigation
- page count
- zoom
- scroll
- fit width
- fit page

Lazy-render pages and cache them.

Text extraction may be supplementary, not the primary visual preview.

============================================================
8. WORD / DOCX PREVIEW
============================================================

Current text extraction produces a crude approximation and duplicate-looking
content in some documents.

Fix DOC/DOCX preview.

Desired:
- preserve headings
- paragraphs
- lists
- tables where possible
- spacing
- page-like presentation

Best option:
render document pages to images/PDF using an available backend such as
LibreOffice headless conversion, then display rendered pages.

Fallback:
structured text preview.

Do not duplicate document content.

============================================================
9. EXCEL / XLSX PREVIEW
============================================================

Current spreadsheet preview is visually broken:
- raw serial date values
- columns misaligned
- text clipped arbitrarily
- rows wrap incorrectly
- table does not resemble a spreadsheet

Implement a real grid preview.

Requirements:
- column headers A/B/C...
- row numbers
- proper cell boundaries
- column sizing
- horizontal scrolling
- vertical scrolling
- selected sheet indication
- sheet switching
- dates formatted as dates rather than Excel serial numbers
- numbers formatted appropriately
- merged-cell handling where practical
- freeze header visually

Never construct spreadsheet tables using fragile fixed-width string slicing.

============================================================
10. JUPYTER NOTEBOOK PREVIEW
============================================================

Improve .ipynb rendering.

Render:
- Markdown cells distinctly
- Code cells with syntax highlighting
- execution counts
- text outputs
- tables/dataframes
- error outputs
- image outputs when possible

Use notebook-style cell separators/cards.

Do not present the notebook as a generic flat text dump.

============================================================
11. SOURCE CODE PREVIEW
============================================================

Keep syntax highlighting but improve it substantially.

Current custom token parsing should eventually be replaced with a proper
syntax-highlighting library if practical.

Requirements:
- accurate syntax highlighting
- line numbers
- tabs handled correctly
- horizontal scrolling
- vertical scrolling
- no unwanted line wrapping for source code
- current file metadata
- large-file lazy loading

Support Rust, Python, JS/TS, Java, C/C++, C#, Go, SQL, JSON, YAML,
Markdown, HTML/CSS, shell, etc.

Do not cap usability at an arbitrary first 400 lines.
Use viewport/lazy rendering.

============================================================
12. FIX FAKE TEXT EDITING
============================================================

The UI currently advertises an EDITING mode and Ctrl+S behavior.

Do not expose an editor unless editing actually works.

Either:

A. implement a real text editor:
- editable buffer
- cursor
- selection
- insert/delete
- undo/redo
- Ctrl+S save
- dirty-state indicator

OR

B. remove "e:edit", EDITING and Ctrl+S from the UI completely.

Never show functionality that is only a no-op.

============================================================
13. FIX COLUMN/PANE LAYOUT
============================================================

The screenshots show:
- very narrow directory columns
- filenames heavily truncated
- excessive preview width in some states
- unused blank areas
- inconsistent pane proportions
- awkward deep-directory navigation

Redesign sizing.

Rules:
- active directory pane gets more width.
- ancestor panes may be narrower.
- preview gets useful width but must not starve filename panes.
- establish sane minimum widths.
- dynamically adapt based on terminal width.
- hide/collapse oldest ancestor panes on smaller screens.
- allow user resize.
- remember user ratios.

Do not blindly force five percentage ratios for every layout.

============================================================
14. FIX LARGE EMPTY / BLACK UI AREAS
============================================================

Some screenshots show huge empty black areas.

The UI should use available space intentionally.

For an empty folder show a polished empty state:
"Folder is empty"

For no preview:
"No preview available"

For unsupported file:
show:
- icon
- filename
- type
- size
- modified date
- "Open externally" hint

Never leave unexplained giant blank regions.

============================================================
15. FIX FILE/FOLDER ROW RENDERING
============================================================

Improve list row rendering.

Problems:
- inconsistent icon widths
- Unicode emoji width assumptions
- filenames clipped unpredictably
- selection pills may overflow
- Powerline characters may render incorrectly depending on font
- selected background widths are inconsistent

Implement Unicode display-width-aware rendering.

Use unicode-width or equivalent.

Do NOT assume:
emoji width == 2
character count == terminal display width

Truncate using ellipsis:
very-long-filename-example...

Never cut UTF-8 incorrectly.

Selection should be clean and rectangular.
Avoid decorative Powerline edges if they create clipping/alignment problems.

============================================================
16. FILE ICON SYSTEM
============================================================

Create consistent file-type icons.

Categories:
folder
drive
image
video
audio
PDF
Word
Excel
PowerPoint
archive
code
text
database
executable
unknown

Do not depend entirely on emoji because Windows terminal font fallback causes
different widths.

Prefer Nerd Font icons when available with a safe fallback.

============================================================
17. PREVIEW HEADER REDESIGN
============================================================

Current preview title crams too much information into the border.

Do not put:

filename | size | type | controls | zoom | rotation | etc.

all into one border title.

Create:

Preview
────────────────────────

filename.ext
TYPE • SIZE • DIMENSIONS/MODIFIED DATE

Then content.

Put contextual controls in a bottom action/status bar.

Long filenames must ellipsize cleanly.

============================================================
18. BREADCRUMB REDESIGN
============================================================

Make breadcrumb interactive:

C: > Users > sifay > Downloads > Project

Requirements:
- clickable segments
- truncate intelligently
- never overflow screen
- active segment visually distinct
- root/drives button
- optionally allow Ctrl+L path entry

============================================================
19. STATUS BAR REDESIGN
============================================================

Current status bar is crowded with cryptic shortcuts.

Replace with contextual status/action bar.

Example normal mode:

12/104   ↑↓ Navigate   Enter Open   F2 Rename   Del Delete   Ctrl+C Copy

Image:

PNG 1920×1080   1.4 MB   +/- Zoom   0 Fit   R Rotate

Video:

MP4 1080p   03:24   Space Play   ←→ Seek

Shortcuts shown must match the current context.

============================================================
20. SCROLLBARS
============================================================

Add visible scroll indicators/scrollbars for:
- long directory lists
- text preview
- code preview
- spreadsheets
- documents/PDF

The user must know:
- current position
- whether more content exists

============================================================
21. SELECTION MODEL
============================================================

Separate these concepts:

focused pane
selected row
hovered row
preview target
multi-selected files

Do not overload one current_level/selected state for every interaction.

Implement:
- single click selection
- Ctrl+click multi-select
- Shift+click range select
- Ctrl+A select all
- clear selection appropriately

Selected items should remain stable while scrolling.

============================================================
22. KEYBOARD NAVIGATION
============================================================

Maintain keyboard support.

Arrow Up/Down -> selection
Left -> parent/previous column
Right/Enter -> open folder
Enter on file -> open file
Backspace -> parent folder
Home/End
PageUp/PageDown
F2 rename
Delete delete
Ctrl+C/X/V
Ctrl+A
Ctrl+L path entry
Ctrl+F search
Esc cancel modal/action

Vim keys may remain optional but should not interfere with normal typing.

============================================================
23. SEARCH / FILTER
============================================================

Add Ctrl+F quick filter/search for current folder.

- incremental search
- highlight matches
- Esc clears
- mouse selectable results

Optional:
global recursive search mode.

============================================================
24. PERFORMANCE / ASYNC WORK
============================================================

Never block the rendering/event thread for:
- image decoding
- ffmpeg
- PDF rendering
- Office conversion
- directory enumeration
- huge files
- thumbnails

Use background workers/tasks.

Show:
Loading preview...

Cancel stale preview work when selection changes rapidly.

Use cache keyed by:
path
mtime
preview size
render options

Avoid using one global temporary filename such as a single video thumbnail
for every video.

============================================================
25. CACHE INVALIDATION
============================================================

Current directory/preview caching needs a coherent invalidation strategy.

Invalidate after:
rename
delete
new folder
paste
move
external filesystem change

Cache keys should include modification timestamp where appropriate.

Do not show stale preview after filesystem mutation.

============================================================
26. ERROR HANDLING
============================================================

Never silently fail.

Display non-blocking notifications/toasts for:
- permission denied
- rename failed
- file already exists
- codec unavailable
- preview unavailable
- corrupted file
- path too long
- file disappeared
- access denied

Avoid panics and unsafe unwrap assumptions for filesystem paths.

============================================================
27. WINDOWS-SPECIFIC BEHAVIOR
============================================================

Correctly handle:
- C:\ drive roots
- UNC paths
- hidden files
- system files
- symlinks/junctions
- inaccessible directories
- OneDrive placeholders
- long paths
- removable drives
- network drives

Drive view should optionally show:
- label
- drive letter
- filesystem
- used/free space

============================================================
28. OPEN FILES PROPERLY
============================================================

Enter/double-click on a non-directory should launch the Windows default app.

Do not simply do nothing when Enter is pressed on a file.

Implement Windows ShellExecute/open equivalent safely.

============================================================
29. RESPONSIVE UI
============================================================

Test at:
80x24
100x30
120x40
160x50
full-screen Windows Terminal

No:
- panic
- overlapping columns
- title overflow
- negative/saturated geometry bugs
- giant unusable blank regions

Provide responsive breakpoints.

============================================================
30. ARCHITECTURAL CLEANUP
============================================================

Separate:

AppState
NavigationState
SelectionState
LayoutState
RenameState
ClipboardState
PreviewState
PreviewCache
AsyncPreviewWorker
InputController
FileOperations

UI rendering should not contain business logic.

Mouse hit-testing must use the EXACT Rects generated by the UI layout.

Do not independently recalculate approximate column boundaries in state.rs.

During draw/layout, produce/store a LayoutGeometry structure:

breadcrumb_rect
pane_rects[]
preview_rect
status_rect
row_rects / row mapping
divider_rects

Mouse handling must consume this geometry.

This is critical for reliable mouse selection.

============================================================
31. PREVIEW BACKEND ARCHITECTURE
============================================================

Create a trait/interface concept such as:

PreviewProvider

providers:
TextPreview
CodePreview
ImagePreview
VideoPreview
AudioPreview
PdfPreview
OfficePreview
SpreadsheetPreview
NotebookPreview
ArchivePreview
BinaryPreview

Each provider should return a structured preview model rather than arbitrary
preformatted terminal strings.

UI decides how to render the model.

============================================================
32. VISUAL QUALITY TARGET
============================================================

Target a polished aesthetic inspired by:

- Yazi
- modern terminal applications
- Windows 11 File Explorer
- VS Code sidebar/preview
- Midnight Commander concepts, but modernized

Keep:
- dark modern theme
- rounded subtle borders where appropriate
- cyan/blue focus accent
- green only where semantically useful

Improve:
- spacing
- typography hierarchy
- selected/focused distinction
- consistent padding
- restrained icon usage

Avoid visual noise.

============================================================
33. SPECIFIC BUGS VISIBLE IN CURRENT SCREENSHOTS
============================================================

Fix all of these:

- Mouse click does not select files.
- Clicking another pane does not reliably activate it.
- Rename does not work.
- Images/GIFs appear heavily pixelated.
- Image rendering uses terminal half-block approximation.
- Video does not provide real useful playback/preview.
- Excel dates appear as raw serial numbers like 46122.
- Excel table columns wrap/misalign badly.
- Long filenames are clipped without useful ellipsis.
- Pane titles get truncated.
- Preview header can overflow horizontally.
- Large unused black regions appear.
- DOCX extraction can produce duplicated/poorly structured content.
- Directory columns become too narrow.
- Unicode icons cause inconsistent alignment.
- Selection background width is inconsistent.
- Preview scrolling UX is obscure.
- File editing is advertised despite save/edit functionality being incomplete.
- Directory preview is just a raw child list and lacks useful metadata.
- No polished unsupported-file state.
- No visible loading state for expensive previews.
- Potential UI blocking during preview generation.
- No true file context menu.
- No complete clipboard/file-operation workflow.
- No inline rename editor.
- No proper mouse hover state.
- No multi-selection.
- No clickable breadcrumb navigation.
- No proper scrollbars.

============================================================
34. IMPLEMENTATION PRIORITY
============================================================

PHASE 1 — MUST FIX FIRST
1. Correct layout geometry model.
2. Correct mouse hit-testing.
3. Single-click selection.
4. Double-click open.
5. Pane focus.
6. F2 inline rename.
7. Copy/cut/paste/delete/new-folder.
8. Cache invalidation.
9. Fix filename clipping/alignment.
10. Responsive pane sizing.

PHASE 2 — PREVIEW ENGINE
1. Replace ANSI half-block image rendering.
2. High-quality image backend.
3. PDF page rendering.
4. Video preview/player backend.
5. Better DOCX/PPTX rendering.
6. Spreadsheet grid.
7. Notebook rendering.
8. Async preview workers.

PHASE 3 — POLISH
1. Context menus.
2. Multi-selection.
3. Search.
4. Breadcrumb interaction.
5. Scrollbars.
6. Notifications.
7. Properties.
8. Better icons.
9. Theme cleanup.
10. Accessibility/keyboard polish.

============================================================
35. ACCEPTANCE TESTS
============================================================

Do not declare the task complete until these work:

TEST 1:
Click the 20th file in a directory.
Exactly that file becomes selected and preview changes.

TEST 2:
Click a file in a different visible column.
That pane becomes focused and the clicked row is selected.

TEST 3:
Double-click folder.
Folder opens.

TEST 4:
Double-click .txt/.docx/.png.
Windows default application opens it.

TEST 5:
Select file -> F2 -> rename -> Enter.
Filesystem filename changes and UI refreshes immediately.

TEST 6:
F2 -> type -> Esc.
Original filename remains unchanged.

TEST 7:
PNG/JPG preview is visually sharp and not rendered using ▀ ANSI blocks.

TEST 8:
GIF renders properly/animates where supported.

TEST 9:
MP4 displays high-quality preview and metadata; playback if backend enabled.

TEST 10:
PDF visually renders pages.

TEST 11:
XLSX displays spreadsheet-style grid and formatted dates.

TEST 12:
DOCX does not duplicate paragraphs and has readable document layout.

TEST 13:
Resize terminal repeatedly.
No overlapping, clipping corruption or panic.

TEST 14:
Drag column divider.
Columns resize smoothly without accidentally selecting files.

TEST 15:
Mouse wheel over directory scrolls that directory.
Mouse wheel over preview scrolls preview.

TEST 16:
Copy/paste/rename/delete refresh all affected panes immediately.

TEST 17:
Open directory containing thousands of files.
UI remains responsive.

TEST 18:
Rapidly arrow through image/video/PDF files.
Old preview jobs are cancelled/ignored and UI does not freeze.

IMPORTANT:
Do not merely patch the screenshots individually.
Refactor the interaction/layout/preview architecture so these classes of
bugs cannot recur.