# 🤠 rusty-ranger-fast (Windows Edition)

`rusty-ranger-fast` is a Windows-first terminal file manager written in Rust with `ratatui` and `crossterm`. It combines column navigation with a Windows-style Tiles view, asynchronous file operations, live directory watching, and a multi-format preview engine.

On Windows, decoded images and rendered document pages are displayed in a mouse-transparent native overlay aligned to the exact preview-cell rectangle. Text and code remain in the terminal grid.

---

## ✨ Features

*   **⚡ Responsive Navigation**: Coalesced pointer input, event-driven redraws, asynchronous directory scans/file operations/previews, live file-system watching, and three explicit preview policies: Normal, Full, and Blitz.
*   **🗂️ Two Views**: Toggle between Columns and a responsive Windows-style Tiles grid with icon, name, type, size, modified date, hover, selection, and a details preview.
*   **🖼️ Rich File Preview Engine**:
    *   **Images**: Supported formats: `jpg`, `jpeg`, `png`, `bmp`, `gif`, `webp`, `tiff`, `ico`. Includes zoom, rotate, and horizontal flip transforms.
    *   **Documents & Code**: Fully syntax-highlighted previews for code/text files with a beautiful **Catppuccin Mocha** palette.
    *   **PDFs & Office**: Normal provides quiet text immediately and upgrades to a cached visual in the background; Full uses LibreOffice/OpenOffice, Microsoft Office automation, Poppler, or MuPDF and reports renderer failures honestly; Blitz stays lightweight.
    *   **Data & Structure**: Formatted rendering of Jupyter Notebooks (`.ipynb`), `.csv`, `.tsv`, and `.rtf`.
    *   **Archives & Media**: Archive listings, audio metadata, animated-GIF external playback, and asynchronous video thumbnails. Press `Enter` or `Space` to play media in the default Windows application.
*   **✏️ Inline Text Editor**: Jump directly into any editable text/code file within the preview pane with one keypress, edit inline, and save changes immediately.
*   **💻 Windows & Unix Native**: Enumerates Windows drive letters (`A:\` through `Z:\`) natively, and supports fallback path configurations.

---

## ⌨️ Keybindings Reference

### 1. General Navigation
| Key | Action |
| :--- | :--- |
| `h` / `Left Arrow` | Navigate to the parent in Columns view; move one tile left in Tiles view |
| `l` / `Right Arrow` | Open a child pane in Columns view; move one tile right in Tiles view |
| `Enter` | Open the selected folder or file |
| `k` / `Up Arrow` | Scroll up in the directory list |
| `j` / `Down Arrow` | Scroll down in the directory list |
| `g` | Jump to the **top** of the directory list |
| `G` | Jump to the **bottom** of the directory list |
| `Ctrl + d` / `PageDown` | Page down through directory |
| `Ctrl + u` / `PageUp` | Page up through directory |
| `~` | Quick jump to your **Home** directory |
| `\` | Show Windows Drives list (`This PC`) on Windows, or jump to root `/` on Unix |

### 2. File Preview Interaction
| Key | Action |
| :--- | :--- |
| `[` / `]` or `?` / `/` | Scroll preview text up / down |
| `e` | **Edit mode** (Enter inline code/text editor on supported text files) |
| `Ctrl + s` | Save edited file (Only when in Edit mode) |
| `Esc` | Exit Edit mode without saving / Quit application (from general mode) |
| `q` | Quit application (Only when not in edit mode) |
| `Space` | Play the selected video, audio, or animated GIF externally |
| Click preview, then `Left` / `Right` or `PageUp` / `PageDown` | Move between rendered PDF pages or presentation slides |
| Mouse wheel over paged preview | Move between pages/slides without scrolling the header |

Preview modes are selected in the sidebar: **Normal** shows useful content without refresh banners while a rich preview is prepared, **Full** forces complete Office/PDF page rendering, and **Blitz** uses cached visuals or quiet lightweight metadata without blocking navigation. The main mode is authoritative; legacy Office/PDF settings cannot silently turn Normal into Full.

### 3. Image Transform Controls
*(Active when hovering over any image format)*
| Key | Action |
| :--- | :--- |
| `+` / `=` | Zoom **in** |
| `-` | Zoom **out** |
| `0` | Fit image to preview pane |
| `r` | Rotate image **90° clockwise** |
| `R` | Rotate image **90° counter-clockwise** |
| `f` | Flip image **horizontally** |

---

## 🛠️ Requirements & Installation

### Requirements
*   **Rust Compiler** (v1.56 or later). [Install Rust](https://www.rust-lang.org/tools/install).
*   **Windows Terminal** is recommended and configured automatically when available.
*   Optional visual backends: LibreOffice/OpenOffice or Microsoft Office; Poppler or MuPDF for PDF rasterization; FFmpeg for video thumbnails.

### 🚀 Quick Start (Precompiled Binary)
If you are on Windows, you can run the application immediately without installing Rust:
1. Download or clone this repository.
2. Double-click or run `rusty_ranger_fast.exe` from the command line in the repository root.

### Running & Building from Source
1.  **Clone the Repository**:
    ```bash
    git clone https://github.com/sifayathf/rustyrangerfastslick.git
    cd rustyrangerfastslick
    ```
2.  **Build in Release Mode**:
    ```bash
    cargo build --release
    ```
3.  **Run the application**:
    ```bash
    cargo run
    ```
