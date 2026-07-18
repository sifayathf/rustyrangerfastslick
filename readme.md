# 🤠 rusty-ranger-fast (Windows Edition)

`rusty-ranger-fast` is a blazing-fast terminal file manager written in Rust, built on top of `ratatui` and `crossterm`. It is designed with power-users in mind, featuring ultra-fast multi-pane navigation, custom directory caching, and a rich, multi-format file preview engine.

This edition features a cross-platform backend optimized for Windows terminal environments, rendering graphics inline using half-block terminal graphics (`viuer`), alongside native support for Linux/macOS with X11/Wayland pixel-perfect `ueberzugpp` overlay integration.

---

## ✨ Features

*   **⚡ High-Speed Navigation**: Uses a high-performance directory caching system with configurable time-to-live (TTL) and size caps.
*   **🖼️ Rich File Preview Engine**:
    *   **Images**: Supported formats: `jpg`, `jpeg`, `png`, `bmp`, `gif`, `webp`, `tiff`, `ico`. Includes zoom, rotate, and horizontal flip transforms.
    *   **Documents & Code**: Fully syntax-highlighted previews for code/text files with a beautiful **Catppuccin Mocha** palette.
    *   **PDFs & Office**: Text previews for `.pdf`, `.docx`, `.xlsx`, `.pptx`, `.ods`, `.xls`, `.ppt`.
    *   **Data & Structure**: Formatted rendering of Jupyter Notebooks (`.ipynb`), `.csv`, `.tsv`, and `.rtf`.
    *   **Archives & Media**: View directory listings of archives (`.zip`, `.tar`, `.gz`, etc.), metadata of audio files (duration, bitrates, etc.), and video files.
*   **✏️ Inline Text Editor**: Jump directly into any editable text/code file within the preview pane with one keypress, edit inline, and save changes immediately.
*   **💻 Windows & Unix Native**: Enumerates Windows drive letters (`A:\` through `Z:\`) natively, and supports fallback path configurations.

---

## ⌨️ Keybindings Reference

### 1. General Navigation
| Key | Action |
| :--- | :--- |
| `h` / `Left Arrow` | Navigate up to the parent directory (Left pane) |
| `l` / `Right Arrow` / `Enter` | Open folder or file (Right pane) |
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
*   *Optional (Unix-only)*: `ueberzugpp` for pixel-perfect overlays. On Windows, inline half-block rendering is used automatically.

### Running & Building
1.  **Clone the Repository**:
    ```bash
    git clone https://github.com/sifayathf/rustyrangerfastwindows.git
    cd rustyrangerfastwindows
    ```
2.  **Build in Release Mode**:
    ```bash
    cargo build --release
    ```
3.  **Run the application**:
    ```bash
    cargo run
    ```
