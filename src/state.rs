// ================= src/state.rs =================
use std::{
    path::PathBuf, fs,
    time::{Duration, Instant},
    collections::HashMap,
    sync::Arc,
};
use parking_lot::Mutex;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, KeyEventKind, MouseEventKind, MouseButton};
use once_cell::sync::Lazy;
use ratatui::layout::Rect;

// ── Directory listing cache ───────────────────────────────────────────────────
// TTL prevents re-reading the filesystem every frame when the preview pane
// hovers over a directory. Cap stops UI freezes on System32-sized folders.

const DIR_CACHE_TTL: Duration = Duration::from_secs(2);
const MAX_DIR_ENTRIES: usize  = 3_000;

#[inline]
fn point_in(col: u16, row: u16, r: &Rect) -> bool {
    col >= r.x && col < r.x.saturating_add(r.width) && row >= r.y && row < r.y.saturating_add(r.height)
}

#[derive(Clone)]
pub struct DirEntryInfo {
    pub path: PathBuf,
    pub is_dir: bool,
}

struct DirCacheEntry {
    entries: Vec<DirEntryInfo>,
    at:      Instant,
}

static DIR_CACHE: Lazy<Mutex<HashMap<PathBuf, DirCacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
pub struct DirLevel {
    pub path: PathBuf,
    pub files: Vec<DirEntryInfo>,
    pub selected: usize,
    /// Multi-selected (marked) entries, keyed by absolute path.
    pub marked: std::collections::HashSet<PathBuf>,
    /// Anchor index for Shift+Click range selection.
    pub select_anchor: usize,
}

impl DirLevel {
    pub fn new(path: PathBuf, files: Vec<DirEntryInfo>) -> Self {
        Self { path, files, selected: 0, marked: std::collections::HashSet::new(), select_anchor: 0 }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    Rename,
    ConfirmDelete,
    ConfirmDeletePermanent,
    ContextMenu,
    NewFolder,
    Properties,
}

// ── Drive sidebar ──────────────────────────────────────────────────────────
#[derive(Clone)]
pub struct DriveInfo {
    pub path:  PathBuf,
    pub label: String,
    pub fs:    String,
    pub kind:  String,   // "Fixed" | "Removable" | "Network" | "CD-ROM" | "Unknown"
    pub total: u64,
    pub free:  u64,
}

// ── Right-click context menu ─────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Open,
    OpenWith,
    Cut,
    Copy,
    Paste,
    Rename,
    Delete,
    Properties,
    CopyPath,
    OpenInTerminal,
    NewFolder,
}

impl ContextAction {
    pub fn label(&self) -> &'static str {
        match self {
            ContextAction::Open => "Open",
            ContextAction::OpenWith => "Open With...",
            ContextAction::Cut => "Cut",
            ContextAction::Copy => "Copy",
            ContextAction::Paste => "Paste",
            ContextAction::Rename => "Rename",
            ContextAction::Delete => "Delete",
            ContextAction::Properties => "Properties",
            ContextAction::CopyPath => "Copy Path",
            ContextAction::OpenInTerminal => "Open in Terminal",
            ContextAction::NewFolder => "New Folder",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OfficeRenderMode {
    Text,
    Full,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PdfRenderMode {
    Text,
    Visual,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToggleAction {
    Theme,
    OfficeMode,
    PdfMode,
    EditMode,
    DirPreviewClick,
}

pub struct AppTheme {
    pub bg_panel: ratatui::style::Color,
    pub bg_root: ratatui::style::Color,
    pub border: ratatui::style::Color,
    pub border_lo: ratatui::style::Color,
    pub text: ratatui::style::Color,
    pub text_soft: ratatui::style::Color,
    pub muted: ratatui::style::Color,
    pub accent: ratatui::style::Color,
    pub accent2: ratatui::style::Color,
    pub sel_bg: ratatui::style::Color,
    pub sel_bg_inactive: ratatui::style::Color,
    pub folder: ratatui::style::Color,
    pub ok: ratatui::style::Color,
    pub warn: ratatui::style::Color,
    pub err: ratatui::style::Color,
    pub mark: ratatui::style::Color,
}

impl AppTheme {
    pub fn dark() -> Self {
        Self {
            bg_panel: ratatui::style::Color::Rgb(24, 26, 34),
            bg_root: ratatui::style::Color::Rgb(18, 20, 26),
            border: ratatui::style::Color::Rgb(38, 40, 50),
            border_lo: ratatui::style::Color::Rgb(28, 30, 37),
            text: ratatui::style::Color::Rgb(214, 218, 230),
            text_soft: ratatui::style::Color::Rgb(160, 166, 185),
            muted: ratatui::style::Color::Rgb(120, 126, 145),
            accent: ratatui::style::Color::Rgb(65, 166, 166),
            accent2: ratatui::style::Color::Rgb(137, 180, 250),
            sel_bg: ratatui::style::Color::Rgb(32, 64, 66),
            sel_bg_inactive: ratatui::style::Color::Rgb(40, 42, 52),
            folder: ratatui::style::Color::Rgb(229, 192, 123),
            ok: ratatui::style::Color::Rgb(137, 220, 165),
            warn: ratatui::style::Color::Rgb(240, 198, 116),
            err: ratatui::style::Color::Rgb(240, 120, 120),
            mark: ratatui::style::Color::Rgb(210, 160, 90),
        }
    }

    pub fn light() -> Self {
        Self {
            bg_panel: ratatui::style::Color::Rgb(245, 247, 250),
            bg_root: ratatui::style::Color::Rgb(235, 238, 242),
            border: ratatui::style::Color::Rgb(210, 215, 225),
            border_lo: ratatui::style::Color::Rgb(225, 228, 235),
            text: ratatui::style::Color::Rgb(30, 35, 45),
            text_soft: ratatui::style::Color::Rgb(70, 80, 95),
            muted: ratatui::style::Color::Rgb(120, 130, 145),
            accent: ratatui::style::Color::Rgb(0, 122, 204),
            accent2: ratatui::style::Color::Rgb(30, 144, 255),
            sel_bg: ratatui::style::Color::Rgb(215, 232, 255),
            sel_bg_inactive: ratatui::style::Color::Rgb(230, 236, 245),
            folder: ratatui::style::Color::Rgb(218, 165, 32),
            ok: ratatui::style::Color::Rgb(40, 160, 80),
            warn: ratatui::style::Color::Rgb(210, 130, 20),
            err: ratatui::style::Color::Rgb(220, 50, 50),
            mark: ratatui::style::Color::Rgb(180, 100, 20),
        }
    }
}

#[derive(Default, Clone)]
pub struct LayoutGeometry {
    pub breadcrumb_rect: Rect,
    pub status_rect: Rect,
    pub pane_rects: Vec<Rect>,
    pub preview_rect: Option<Rect>,
    // Maps visible pane index to a list of (file_index, Rect)
    pub row_rects: HashMap<usize, Vec<(usize, Rect)>>,
    pub divider_rects: Vec<Rect>,

    // Left drive/quick-access sidebar
    pub sidebar_rect: Rect,
    pub sidebar_item_rects: Vec<(Rect, PathBuf)>,
    pub sidebar_divider_rect: Rect,
    pub toggle_rects: Vec<(Rect, ToggleAction)>,

    // Clickable directory preview items
    pub preview_dir_item_rects: Vec<(Rect, PathBuf)>,

    // Preview editor & slide controls
    pub edit_save_btn_rect: Option<Rect>,
    pub slide_prev_rect: Option<Rect>,
    pub slide_next_rect: Option<Rect>,

    // Clickable breadcrumb path segments
    pub breadcrumb_segment_rects: Vec<(Rect, PathBuf)>,

    // Right-click context menu
    pub context_menu_rect: Option<Rect>,
    pub context_menu_item_rects: Vec<(Rect, ContextAction)>,
}

pub struct AppState {
    pub levels:        Vec<DirLevel>,
    pub current_level: usize,
    last_event_time:   Instant,

    // Settings & Toggles
    pub theme_mode:            ThemeMode,
    pub office_mode:           OfficeRenderMode,
    pub pdf_mode:              PdfRenderMode,
    pub edit_preview_mode:     bool,
    pub dir_preview_clickable: bool,
    pub pptx_slide_index:      usize,

    // Interactive Preview Editor state
    pub edit_buffer:           Vec<String>,
    pub edit_cursor_row:       usize,
    pub edit_cursor_col:       usize,
    pub edit_dirty:            bool,
    pub edit_path:             Option<PathBuf>,

    // Preview interaction state
    pub mode:            AppMode,  // current application mode
    pub input_buffer:    String,   // used for Rename / New Folder
    pub input_cursor:    usize,    // cursor position within input_buffer (chars)
    pub input_sel_start: usize,    // selection start (chars) — for Ctrl+A / basename select
    pub clipboard:       Option<(Vec<PathBuf>, bool)>, // (paths, is_cut)
    pub image_zoom:      f32,      // 1.0 = 100%; +/- adjust
    pub image_rotation:  u32,      // 0 / 90 / 180 / 270 degrees
    pub image_flip_h:    bool,     // 'f' flips horizontally
    pub preview_scroll:  usize,    // manual scroll offset for preview pane

    // Mouse double-click tracking: (time, pane_idx, file_idx)
    pub last_click:      Option<(Instant, usize, usize)>,

    // Layout and resizing state
    pub dragging_divider: Option<usize>,
    pub dragging_sidebar: bool,
    pub column_ratios:    Vec<f32>,
    pub layout_geometry:  Arc<Mutex<LayoutGeometry>>,

    // Left sidebar: drives + quick access
    pub drives:          Vec<DriveInfo>,
    pub quick_access:    Vec<(&'static str, PathBuf)>,
    drives_refreshed_at: Instant,
    pub sidebar_width:   u16,

    // Right-click context menu
    pub context_menu_target: Option<PathBuf>,
    pub context_menu_items:  Vec<ContextAction>,
    pub context_menu_hover:  Option<usize>,
    pub pending_menu_pos:    (u16, u16),

    // Non-blocking status notifications (message, shown_at)
    pub notice: Option<(String, Instant, bool)>, // bool = is_error

    // Windows Native preview overlay manager
    pub native_preview: crate::native::NativePreviewManager,
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let home = dirs::home_dir().unwrap_or_else(|| {
            #[cfg(windows)]
            { PathBuf::from("C:\\") }
            #[cfg(not(windows))]
            { PathBuf::from("/") }
        });
        let files = list_dir(&home)?;

        let level = DirLevel::new(home.clone(), files);

        let mut quick_access: Vec<(&'static str, PathBuf)> = Vec::new();
        if let Some(h) = dirs::home_dir() { quick_access.push(("🏠 Home", h)); }
        if let Some(d) = dirs::desktop_dir() { quick_access.push(("🖥 Desktop", d)); }
        if let Some(d) = dirs::document_dir() { quick_access.push(("📄 Documents", d)); }
        if let Some(d) = dirs::download_dir() { quick_access.push(("⬇ Downloads", d)); }
        if let Some(d) = dirs::picture_dir() { quick_access.push(("🖼 Pictures", d)); }

        Ok(Self {
            levels:         vec![level],
            current_level:  0,
            last_event_time: Instant::now(),
            theme_mode:            ThemeMode::Dark,
            office_mode:           OfficeRenderMode::Text,
            pdf_mode:              PdfRenderMode::Text,
            edit_preview_mode:     false,
            dir_preview_clickable: true,
            pptx_slide_index:      0,
            edit_buffer:           Vec::new(),
            edit_cursor_row:       0,
            edit_cursor_col:       0,
            edit_dirty:            false,
            edit_path:             None,
            mode:           AppMode::Normal,
            input_buffer:   String::new(),
            input_cursor:   0,
            input_sel_start: 0,
            clipboard:      None,
            last_click:     None,
            dragging_divider: None,
            dragging_sidebar: false,
            column_ratios:    vec![0.10, 0.10, 0.12, 0.18, 0.50],
            layout_geometry:  Arc::new(Mutex::new(LayoutGeometry::default())),
            preview_scroll: 0,
            image_zoom: 1.0,
            image_rotation: 0,
            image_flip_h: false,
            drives: list_drives_info(),
            quick_access,
            drives_refreshed_at: Instant::now(),
            sidebar_width: 26,
            context_menu_target: None,
            context_menu_items: Vec::new(),
            context_menu_hover: None,
            pending_menu_pos: (0, 0),
            notice: None,
            native_preview: crate::native::NativePreviewManager::new(),
        })
    }

    pub fn theme(&self) -> AppTheme {
        match self.theme_mode {
            ThemeMode::Dark => AppTheme::dark(),
            ThemeMode::Light => AppTheme::light(),
        }
    }

    /// Re-scan drive list at most every 5s (used/free space can change).
    pub fn maybe_refresh_drives(&mut self) {
        if self.drives_refreshed_at.elapsed() > Duration::from_secs(5) {
            self.drives = list_drives_info();
            self.drives_refreshed_at = Instant::now();
        }
    }

    pub fn set_notice(&mut self, msg: impl Into<String>, is_error: bool) {
        self.notice = Some((msg.into(), Instant::now(), is_error));
    }

    /// Active (non-expired) notice text, if any.
    pub fn active_notice(&self) -> Option<(&str, bool)> {
        self.notice.as_ref().and_then(|(msg, at, err)| {
            if at.elapsed() < Duration::from_secs(4) { Some((msg.as_str(), *err)) } else { None }
        })
    }

    /// Navigate the active pane directly to an arbitrary directory path
    /// (used by sidebar clicks, breadcrumb clicks, and drive navigation).
    pub fn navigate_to(&mut self, path: &std::path::Path) {
        match list_dir(path) {
            Ok(files) => {
                self.levels = vec![DirLevel::new(path.to_path_buf(), files)];
                self.current_level = 0;
                self.reset_image_state();
            }
            Err(e) => self.set_notice(format!("Can't open {}: {}", path.display(), e), true),
        }
    }

    pub fn current(&self) -> &DirLevel {
        &self.levels[self.current_level]
    }

    pub fn current_mut(&mut self) -> &mut DirLevel {
        &mut self.levels[self.current_level]
    }

    pub fn handle_events(&mut self) -> anyhow::Result<bool> {
        // Poll with timeout to prevent blocking forever
        if !event::poll(Duration::from_millis(100))? {
            return Ok(false);
        }

        match event::read()? {
            Event::Key(key) => {
                // Only process KeyPress events — ignore KeyRelease/KeyRepeat.
                // Critical on Windows: prevents double-triggering.
                if key.kind != KeyEventKind::Press {
                    return Ok(false);
                }

                self.last_event_time = Instant::now();

                if self.edit_preview_mode && self.mode == AppMode::Normal {
                    if self.handle_edit_key(key.code, key.modifiers) {
                        return Ok(false);
                    }
                }

                match (key.code, key.modifiers) {
                    // ── Quit ────────────────────────────────────────────────
                    // Only allow quit when in Normal mode
                    (KeyCode::Char('q'), KeyModifiers::NONE) if self.mode == AppMode::Normal => return Ok(true),
                    (KeyCode::Esc, _) if self.mode != AppMode::Normal => {
                        // Escape cancels modes
                        self.mode = AppMode::Normal;
                    }
                    (KeyCode::Esc, _) => return Ok(true),

                    // ── Modes ───────────────────────────────────────
                    (KeyCode::F(2), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.start_rename();
                    }
                    (KeyCode::Delete, KeyModifiers::SHIFT) if self.mode == AppMode::Normal => {
                        if !self.current().files.is_empty() {
                            self.mode = AppMode::ConfirmDeletePermanent;
                        }
                    }
                    (KeyCode::Delete, _) if self.mode == AppMode::Normal => {
                        if !self.current().files.is_empty() {
                            self.mode = AppMode::ConfirmDelete;
                        }
                    }
                    (KeyCode::F(7), KeyModifiers::NONE)
                    | (KeyCode::Char('n'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.mode = AppMode::NewFolder;
                        self.input_buffer.clear();
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                    }
                    (KeyCode::Char('N'), m)
                        if self.mode == AppMode::Normal && m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) => {
                        self.mode = AppMode::NewFolder;
                        self.input_buffer.clear();
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                    }
                    (KeyCode::Enter, KeyModifiers::ALT) if self.mode == AppMode::Normal => {
                        if !self.current().files.is_empty() {
                            self.mode = AppMode::Properties;
                        }
                    }

                    // ── Clipboard Operations (Copy, Cut, Paste) ─────────────
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        let paths = self.selected_paths();
                        if !paths.is_empty() {
                            let n = paths.len();
                            self.clipboard = Some((paths, false));
                            self.set_notice(format!("Copied {} item(s)", n), false);
                        }
                    }
                    (KeyCode::Char('x'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        let paths = self.selected_paths();
                        if !paths.is_empty() {
                            let n = paths.len();
                            self.clipboard = Some((paths, true));
                            self.set_notice(format!("Cut {} item(s)", n), false);
                        }
                    }
                    (KeyCode::Char('v'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.do_paste();
                    }
                    (KeyCode::Char('a'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        // Select-all (mark every entry in the active pane)
                        let cur = self.current_mut();
                        let all: std::collections::HashSet<PathBuf> =
                            cur.files.iter().map(|f| f.path.clone()).collect();
                        cur.marked = all;
                    }

                    // ── Rename input editing ─────────────────────────────────
                    (KeyCode::Char('a'), KeyModifiers::CONTROL) if self.mode == AppMode::Rename => {
                        self.input_sel_start = 0;
                        self.input_cursor = self.input_buffer.chars().count();
                    }
                    (KeyCode::Char(c), m) if self.mode == AppMode::Rename && (m == KeyModifiers::NONE || m == KeyModifiers::SHIFT) => {
                        self.rename_input_replace_selection_with(&c.to_string());
                        return Ok(false);
                    }
                    (KeyCode::Char(c), _) if self.mode == AppMode::NewFolder => {
                        self.input_buffer.insert(self.byte_idx(self.input_cursor), c);
                        self.input_cursor += 1;
                        return Ok(false);
                    }
                    (KeyCode::Backspace, _) if self.mode == AppMode::Rename => {
                        if self.input_sel_start != self.input_cursor {
                            self.rename_input_replace_selection_with("");
                        } else if self.input_cursor > 0 {
                            let idx = self.byte_idx(self.input_cursor - 1);
                            let end = self.byte_idx(self.input_cursor);
                            self.input_buffer.replace_range(idx..end, "");
                            self.input_cursor -= 1;
                            self.input_sel_start = self.input_cursor;
                        }
                        return Ok(false);
                    }
                    (KeyCode::Backspace, _) if self.mode == AppMode::NewFolder => {
                        if self.input_cursor > 0 {
                            let idx = self.byte_idx(self.input_cursor - 1);
                            let end = self.byte_idx(self.input_cursor);
                            self.input_buffer.replace_range(idx..end, "");
                            self.input_cursor -= 1;
                        }
                        return Ok(false);
                    }
                    (KeyCode::Delete, _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        let len = self.input_buffer.chars().count();
                        if self.input_cursor < len {
                            let idx = self.byte_idx(self.input_cursor);
                            let end = self.byte_idx(self.input_cursor + 1);
                            self.input_buffer.replace_range(idx..end, "");
                        }
                        self.input_sel_start = self.input_cursor;
                        return Ok(false);
                    }
                    (KeyCode::Left, _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        self.input_cursor = self.input_cursor.saturating_sub(1);
                        self.input_sel_start = self.input_cursor;
                        return Ok(false);
                    }
                    (KeyCode::Right, _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        let len = self.input_buffer.chars().count();
                        self.input_cursor = (self.input_cursor + 1).min(len);
                        self.input_sel_start = self.input_cursor;
                        return Ok(false);
                    }
                    (KeyCode::Home, _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                        return Ok(false);
                    }
                    (KeyCode::End, _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        let len = self.input_buffer.chars().count();
                        self.input_cursor = len;
                        self.input_sel_start = len;
                        return Ok(false);
                    }
                    (KeyCode::Enter, _) if self.mode == AppMode::Rename => {
                        self.commit_rename();
                        return Ok(false);
                    }
                    (KeyCode::Enter, _) if self.mode == AppMode::NewFolder => {
                        self.commit_new_folder();
                        return Ok(false);
                    }
                    (KeyCode::Char('y'), _) | (KeyCode::Char('Y'), _) if self.mode == AppMode::ConfirmDelete => {
                        self.do_delete(false);
                        return Ok(false);
                    }
                    (KeyCode::Char('y'), _) | (KeyCode::Char('Y'), _) if self.mode == AppMode::ConfirmDeletePermanent => {
                        self.do_delete(true);
                        return Ok(false);
                    }

                    // ── Image controls ───────────────────────────────────────
                    // Only active when hovering an image file
                    (KeyCode::Char('+'), KeyModifiers::NONE)
                    | (KeyCode::Char('='), KeyModifiers::NONE) => {
                        if self.is_image_selected() {
                            self.image_zoom = (self.image_zoom * 1.25).min(8.0);
                        }
                    }
                    (KeyCode::Char('-'), KeyModifiers::NONE) => {
                        if self.is_image_selected() {
                            self.image_zoom = (self.image_zoom / 1.25).max(0.1);
                        }
                    }
                    (KeyCode::Char('0'), KeyModifiers::NONE) => {
                        // Fit to pane (reset zoom)
                        self.image_zoom = 1.0;
                    }
                    (KeyCode::Char('r'), KeyModifiers::NONE) => {
                        // Rotate 90° clockwise
                        if self.is_image_selected() {
                            self.image_rotation = (self.image_rotation + 90) % 360;
                        }
                    }
                    (KeyCode::Char('R'), KeyModifiers::SHIFT) => {
                        // Rotate 90° counter-clockwise
                        if self.is_image_selected() {
                            self.image_rotation = (self.image_rotation + 270) % 360;
                        }
                    }
                    (KeyCode::Char('f'), KeyModifiers::NONE) => {
                        // Flip horizontal
                        if self.is_image_selected() {
                            self.image_flip_h = !self.image_flip_h;
                        }
                    }

                    // ── Preview scroll (Shift+Up/Down) ───────────────────────
                    (KeyCode::Down, KeyModifiers::SHIFT) if self.mode == AppMode::Normal => {
                        self.preview_scroll = self.preview_scroll.saturating_add(3);
                    }
                    (KeyCode::Up, KeyModifiers::SHIFT) if self.mode == AppMode::Normal => {
                        self.preview_scroll = self.preview_scroll.saturating_sub(3);
                    }

                    // ── Navigation — only when Normal mode ──────────────────
                    (KeyCode::Down, _) if self.mode == AppMode::Normal => self.move_down(),
                    (KeyCode::Up, _)   if self.mode == AppMode::Normal => self.move_up(),
                    (KeyCode::Left, _) if self.mode == AppMode::Normal => { self.go_left()?; self.reset_image_state(); }
                    (KeyCode::Right, _) if self.mode == AppMode::Normal => { self.go_right()?; self.reset_image_state(); }
                    (KeyCode::Enter, _) if self.mode == AppMode::Normal => { self.open_selected(); }

                    // ── Navigation — Vim keys ────────────────────────────────
                    (KeyCode::Char('j'), KeyModifiers::NONE) if self.mode == AppMode::Normal => self.move_down(),
                    (KeyCode::Char('k'), KeyModifiers::NONE) if self.mode == AppMode::Normal => self.move_up(),
                    (KeyCode::Char('h'), KeyModifiers::NONE) if self.mode == AppMode::Normal => { self.go_left()?; self.reset_image_state(); }
                    (KeyCode::Char('l'), KeyModifiers::NONE) if self.mode == AppMode::Normal => { self.go_right()?; self.reset_image_state(); }

                    // ── Jump top / bottom ────────────────────────────────────
                    (KeyCode::Char('g'), KeyModifiers::NONE) if self.mode == AppMode::Normal => self.jump_top(),
                    (KeyCode::Char('G'), KeyModifiers::SHIFT) if self.mode == AppMode::Normal => self.jump_bottom(),

                    // ── Page navigation ──────────────────────────────────────
                    (KeyCode::PageDown, _)
                    | (KeyCode::Char('d'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.page_down()
                    }
                    (KeyCode::PageUp, _)
                    | (KeyCode::Char('u'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.page_up()
                    }

                    // ── Preview scroll ───────────────────────────────────────
                    (KeyCode::Char(']'), KeyModifiers::NONE) | (KeyCode::Char('/'), KeyModifiers::NONE) => {
                        self.preview_scroll = self.preview_scroll.saturating_add(5);
                    }
                    (KeyCode::Char('['), KeyModifiers::NONE) | (KeyCode::Char('?'), KeyModifiers::NONE) | (KeyCode::Char('/'), KeyModifiers::SHIFT) => {
                        self.preview_scroll = self.preview_scroll.saturating_sub(5);
                    }

                    // ── Home directory ───────────────────────────────────────
                    (KeyCode::Char('~'), KeyModifiers::SHIFT) if self.mode == AppMode::Normal => self.go_home()?,

                    // ── Root / Drives ────────────────────────────────────────
                    (KeyCode::Char('\\'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        #[cfg(windows)]
                        self.go_drives()?;
                        #[cfg(not(windows))]
                        self.go_root()?;
                    }

                    _ => {}
                }
            }
            Event::Mouse(mouse) => {
                self.last_event_time = Instant::now();
                match mouse.kind {
                    MouseEventKind::Moved => {
                        if self.mode == AppMode::ContextMenu {
                            let geo = self.layout_geometry.lock().clone();
                            self.context_menu_hover = geo.context_menu_item_rects.iter()
                                .position(|(rect, _)| point_in(mouse.column, mouse.row, rect));
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        let geo = self.layout_geometry.lock().clone();
                        self.handle_scroll(&geo, mouse.column, mouse.row, true);
                    }
                    MouseEventKind::ScrollUp => {
                        let geo = self.layout_geometry.lock().clone();
                        self.handle_scroll(&geo, mouse.column, mouse.row, false);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let geo = self.layout_geometry.lock().clone();

                        // ── Context menu open: any click either selects an item or closes it ──
                        if self.mode == AppMode::ContextMenu {
                            let mut action = None;
                            for (rect, act) in geo.context_menu_item_rects.iter() {
                                if point_in(mouse.column, mouse.row, rect) {
                                    action = Some(*act);
                                    break;
                                }
                            }
                            if let Some(act) = action {
                                self.apply_context_action(act);
                            } else {
                                self.mode = AppMode::Normal; // clicked outside the menu
                            }
                            return Ok(false);
                        }

                        // Clicking elsewhere while editing text commits/cancels the input.
                        if self.mode == AppMode::Rename {
                            self.commit_rename();
                        } else if self.mode == AppMode::NewFolder {
                            self.commit_new_folder();
                        } else if self.mode == AppMode::ConfirmDelete
                            || self.mode == AppMode::ConfirmDeletePermanent
                            || self.mode == AppMode::Properties
                        {
                            self.mode = AppMode::Normal;
                        }

                        // ── Sidebar resize divider ──
                        if point_in(mouse.column, mouse.row, &geo.sidebar_divider_rect) {
                            self.dragging_sidebar = true;
                            return Ok(false);
                        }

                        // ── Settings Toggles ──
                        let mut handled = false;
                        for (rect, action) in geo.toggle_rects.iter() {
                            if point_in(mouse.column, mouse.row, rect) {
                                self.toggle_setting(*action);
                                handled = true;
                                break;
                            }
                        }
                        if handled { return Ok(false); }

                        // ── Save button in Preview Editor ──
                        if let Some(rect) = geo.edit_save_btn_rect {
                            if point_in(mouse.column, mouse.row, &rect) {
                                self.save_edited_preview();
                                return Ok(false);
                            }
                        }

                        // ── Slide Prev / Next in visual preview ──
                        if let Some(rect) = geo.slide_prev_rect {
                            if point_in(mouse.column, mouse.row, &rect) {
                                self.pptx_slide_index = self.pptx_slide_index.saturating_sub(1);
                                return Ok(false);
                            }
                        }
                        if let Some(rect) = geo.slide_next_rect {
                            if point_in(mouse.column, mouse.row, &rect) {
                                self.pptx_slide_index = self.pptx_slide_index.saturating_add(1);
                                return Ok(false);
                            }
                        }

                        // ── Clickable Directory Preview Items ──
                        if self.dir_preview_clickable {
                            for (rect, path) in geo.preview_dir_item_rects.iter() {
                                if point_in(mouse.column, mouse.row, rect) {
                                    if path.is_dir() {
                                        self.navigate_to(path);
                                    } else if let Some(parent) = path.parent() {
                                        let file_name = path.file_name();
                                        self.navigate_to(parent);
                                        if let Some(name) = file_name {
                                            if let Some(cur) = self.levels.last_mut() {
                                                if let Some(idx) = cur.files.iter().position(|f| f.path.file_name() == Some(name)) {
                                                    cur.selected = idx;
                                                    cur.select_anchor = idx;
                                                }
                                            }
                                        }
                                    }
                                    return Ok(false);
                                }
                            }
                        }

                        // ── Sidebar: drives / quick access ──
                        for (rect, path) in geo.sidebar_item_rects.iter() {
                            if point_in(mouse.column, mouse.row, rect) {
                                self.navigate_to(path);
                                handled = true;
                                break;
                            }
                        }
                        if handled { return Ok(false); }

                        // ── Breadcrumb: clickable path segments ──
                        for (rect, path) in geo.breadcrumb_segment_rects.iter() {
                            if point_in(mouse.column, mouse.row, rect) {
                                self.navigate_to(path);
                                handled = true;
                                break;
                            }
                        }
                        if handled { return Ok(false); }

                        // ── Divider drag ──
                        for (idx, dr) in geo.divider_rects.iter().enumerate() {
                            if point_in(mouse.column, mouse.row, dr) {
                                self.dragging_divider = Some(idx);
                                handled = true;
                                break;
                            }
                        }
                        if handled { return Ok(false); }

                        // ── File/folder rows ──
                        self.handle_row_click(&geo, mouse.column, mouse.row, mouse.modifiers, false);
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        let geo = self.layout_geometry.lock().clone();
                        if self.mode == AppMode::ContextMenu {
                            self.mode = AppMode::Normal;
                        }
                        // Right-click selects the item under the cursor first, then opens
                        // the context menu at the mouse position.
                        self.handle_row_click(&geo, mouse.column, mouse.row, KeyModifiers::NONE, true);
                        let target = self.selected_entry_path();
                        self.open_context_menu(target);
                        self.pending_menu_pos = (mouse.column, mouse.row);
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if self.dragging_sidebar {
                            let term_w = crossterm::terminal::size().unwrap_or((100, 30)).0;
                            self.sidebar_width = mouse.column.clamp(16, term_w.saturating_sub(30));
                        } else if let Some(idx) = self.dragging_divider {
                            let geo = self.layout_geometry.lock().clone();
                            if let Some(_first_pane) = geo.pane_rects.first() {
                                let term_w = crossterm::terminal::size().unwrap_or((100, 30)).0;
                                let old_bound = geo.divider_rects.get(idx).map(|r| r.x).unwrap_or(0);
                                let new_bound = mouse.column;

                                if old_bound > 0 && new_bound != old_bound && new_bound > 0 && new_bound < term_w {
                                    let delta_ratio = (new_bound as f32 - old_bound as f32) / term_w as f32;

                                    let num = self.levels.len();
                                    let start = if num > 4 { num - 4 } else { 0 };
                                    let panes = &self.levels[start..];
                                    let np = panes.len();
                                    let has_preview = panes.last().map_or(false, |l| !l.files.is_empty());
                                    let n_cols = if has_preview { np + 1 } else { np };

                                    let start_ratio_idx = 5 - n_cols;
                                    let ratio_idx_left = start_ratio_idx + idx;
                                    let ratio_idx_right = ratio_idx_left + 1;

                                    if ratio_idx_left < self.column_ratios.len() && ratio_idx_right < self.column_ratios.len() {
                                        let val_left = self.column_ratios[ratio_idx_left] + delta_ratio;
                                        let val_right = self.column_ratios[ratio_idx_right] - delta_ratio;

                                        if val_left > 0.06 && val_right > 0.06 {
                                            self.column_ratios[ratio_idx_left] = val_left;
                                            self.column_ratios[ratio_idx_right] = val_right;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        self.dragging_divider = None;
                        self.dragging_sidebar = false;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_scroll(&mut self, geo: &LayoutGeometry, col: u16, row: u16, down: bool) {
        if let Some(pr) = geo.preview_rect {
            if point_in(col, row, &pr) {
                if down { self.preview_scroll = self.preview_scroll.saturating_add(3); }
                else { self.preview_scroll = self.preview_scroll.saturating_sub(3); }
                return;
            }
        }
        for (idx, pr) in geo.pane_rects.iter().enumerate() {
            if point_in(col, row, pr) {
                let num = self.levels.len();
                let start = if num > 4 { num - 4 } else { 0 };
                let real_idx = start + idx;
                if real_idx < self.levels.len() {
                    self.current_level = real_idx;
                    self.levels.truncate(self.current_level + 1);
                    if down { self.move_down(); } else { self.move_up(); }
                }
                return;
            }
        }
    }

    /// Shared row hit-testing for left-click selection and right-click.
    /// `for_context_menu` skips double-click/rename detection since a right
    /// click should just select, not toggle rename or open.
    fn handle_row_click(&mut self, geo: &LayoutGeometry, col: u16, row: u16, mods: KeyModifiers, for_context_menu: bool) {
        for (pane_idx, rows) in geo.row_rects.iter() {
            for (file_idx, rect) in rows {
                if point_in(col, row, rect) {
                    let num = self.levels.len();
                    let start = if num > 4 { num - 4 } else { 0 };
                    let real_level_idx = start + pane_idx;
                    if real_level_idx >= self.levels.len() { return; }

                    let was_active_pane = self.current_level == real_level_idx;
                    self.current_level = real_level_idx;
                    self.levels.truncate(self.current_level + 1);

                    if *file_idx >= self.current().files.len() { return; }

                    if for_context_menu {
                        // Right-click: select without clobbering an existing multi-selection.
                        let cur = self.current_mut();
                        if !cur.marked.contains(&cur.files[*file_idx].path) {
                            cur.marked.clear();
                            cur.selected = *file_idx;
                            cur.select_anchor = *file_idx;
                        }
                        self.reset_image_state();
                        self.mode = AppMode::Normal;
                        return;
                    }

                    if mods.contains(KeyModifiers::SHIFT) {
                        self.mark_range(*file_idx);
                        self.reset_image_state();
                        return;
                    }
                    if mods.contains(KeyModifiers::CONTROL) {
                        self.toggle_mark(*file_idx);
                        self.reset_image_state();
                        return;
                    }

                    let now = Instant::now();
                    let last_click_val = self.last_click;
                    let old_selected = self.current().selected;
                    let had_marks = !self.current().marked.is_empty();

                    let is_double_click = matches!(last_click_val,
                        Some((last_time, last_pane, last_file))
                            if last_pane == *pane_idx && last_file == *file_idx
                                && now.duration_since(last_time).as_millis() < 450);

                    // A slower second click on an already-selected, already-active row
                    // (Explorer-style click-click-pause) starts an inline rename.
                    let is_slow_rename_click = !is_double_click && was_active_pane && !had_marks
                        && old_selected == *file_idx
                        && matches!(last_click_val,
                            Some((last_time, last_pane, last_file))
                                if last_pane == *pane_idx && last_file == *file_idx
                                    && now.duration_since(last_time).as_millis() >= 450
                                    && now.duration_since(last_time).as_millis() < 1800);

                    self.clear_marks();
                    self.current_mut().selected = *file_idx;
                    self.current_mut().select_anchor = *file_idx;

                    if is_double_click {
                        self.last_click = None;
                        self.open_selected();
                    } else if is_slow_rename_click {
                        self.last_click = None;
                        self.start_rename();
                    } else {
                        self.last_click = Some((now, *pane_idx, *file_idx));
                        self.reset_image_state();
                    }
                    return;
                }
            }
        }
        // Clicked empty space inside a pane area but not on any row: just clear marks.
        self.clear_marks();
    }

    // ── Movement helpers ──────────────────────────────────────────────────────

    fn move_down(&mut self) {
        let current = self.current_mut();
        if current.files.is_empty() {
            return;
        }
        if current.selected + 1 < current.files.len() {
            current.selected += 1;
            self.reset_image_state();
        }
    }

    fn move_up(&mut self) {
        let current = self.current_mut();
        if current.selected > 0 {
            current.selected -= 1;
            self.reset_image_state();
        }
    }

    fn go_right(&mut self) -> anyhow::Result<()> {
        let current = self.current();
        if current.files.is_empty() {
            return Ok(());
        }

        let selected_entry = &current.files[current.selected];

        if selected_entry.is_dir {
            let files = list_dir(&selected_entry.path)?;

            let new_level = DirLevel::new(selected_entry.path.clone(), files);

            // Discard levels to the right when branching
            self.levels.truncate(self.current_level + 1);
            self.levels.push(new_level);
            self.current_level += 1;
        }
        Ok(())
    }

    fn go_left(&mut self) -> anyhow::Result<()> {
        if self.current_level > 0 {
            self.current_level -= 1;
            // Truncate the trailing levels so the right-hand preview pane
            // immediately reflects the currently selected item in the new level.
            self.levels.truncate(self.current_level + 1);
        } else {
            // At leftmost level — navigate up to parent
            let current = self.current();
            if let Some(parent) = current.path.parent() {
                let parent_path = parent.to_path_buf();
                let files = list_dir(&parent_path)?;

                // Keep the cursor on the directory we came from
                let selected = files.iter()
                    .position(|e| e.path == current.path)
                    .unwrap_or(0);

                let mut new_level = DirLevel::new(parent_path, files);
                new_level.selected = selected;
                new_level.select_anchor = selected;

                self.levels.insert(0, new_level);
                self.current_level += 1; // adjust index after insert
            }
        }
        Ok(())
    }

    fn jump_top(&mut self) {
        let current = self.current_mut();
        if current.selected != 0 {
            current.selected = 0;
            self.reset_image_state();
        }
    }

    fn jump_bottom(&mut self) {
        let current = self.current_mut();
        if !current.files.is_empty() {
            let last = current.files.len() - 1;
            if current.selected != last {
                current.selected = last;
                self.reset_image_state();
            }
        }
    }
    
    pub fn open_selected(&mut self) {
        let current = self.current();
        if current.files.is_empty() { return; }
        
        let selected_entry = &current.files[current.selected];
        
        if selected_entry.is_dir {
            // It's a directory, go right
            let _ = self.go_right();
        } else {
            // It's a file, open with default app
            #[cfg(windows)]
            {
                let path_str = selected_entry.path.to_string_lossy().to_string();
                // /C start "" "path"
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", &path_str])
                    .spawn();
            }
            #[cfg(not(windows))]
            {
                let path_str = selected_entry.path.to_string_lossy().to_string();
                let _ = std::process::Command::new("xdg-open")
                    .arg(&path_str)
                    .spawn();
            }
        }
    }

    fn page_down(&mut self) {
        let current = self.current_mut();
        if current.files.is_empty() {
            return;
        }
        let target = (current.selected + 10).min(current.files.len() - 1);
        if current.selected != target {
            current.selected = target;
            self.reset_image_state();
        }
    }

    fn page_up(&mut self) {
        let current = self.current_mut();
        let target = current.selected.saturating_sub(10);
        if current.selected != target {
            current.selected = target;
            self.reset_image_state();
        }
    }

    fn go_home(&mut self) -> anyhow::Result<()> {
        if let Some(home) = dirs::home_dir() {
            let files = list_dir(&home)?;
            self.levels = vec![DirLevel::new(home, files)];
            self.current_level = 0;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn go_root(&mut self) -> anyhow::Result<()> {
        let root = PathBuf::from("/");
        let files = list_dir(&root)?;
        self.levels = vec![DirLevel::new(root, files)];
        self.current_level = 0;
        Ok(())
    }

    /// Windows: enumerate available drive letters and present them as a
    /// synthetic top-level directory listing.
    #[cfg(windows)]
    fn go_drives(&mut self) -> anyhow::Result<()> {
        let drives = list_windows_drives();
        // Use a sentinel path to indicate "drives root"
        let drives_root = PathBuf::from("\\\\drives");
        self.levels = vec![DirLevel::new(drives_root, drives)];
        self.current_level = 0;
        Ok(())
    }

    // ── Preview helper methods ────────────────────────────────────────────────

    pub fn selected_file(&self) -> Option<PathBuf> {
        let cur = self.current();
        if cur.files.is_empty() { return None; }
        let p = &cur.files[cur.selected];
        if !p.is_dir { Some(p.path.clone()) } else { None }
    }

    /// Path of the currently highlighted entry, whether it's a file or a directory.
    /// Use this for context-menu / rename / delete / copy-path targets;
    /// use `selected_file` where only *files* are meaningful (e.g. image preview).
    pub fn selected_entry_path(&self) -> Option<PathBuf> {
        let cur = self.current();
        if cur.files.is_empty() { return None; }
        Some(cur.files[cur.selected].path.clone())
    }

    /// True if the selected file is an image
    fn is_image_selected(&self) -> bool {
        self.selected_file().map_or(false, |p| {
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
            matches!(ext.as_str(), "jpg"|"jpeg"|"png"|"bmp"|"gif"|"webp"|"tiff"|"tif"|"ico")
        })
    }

    /// Reset image transform state (called when navigating away)
    pub fn reset_image_state(&mut self) {
        self.image_zoom     = 1.0;
        self.image_rotation = 0;
        self.image_flip_h   = false;
        self.mode           = AppMode::Normal;
        self.preview_scroll = 0;
    }

    /// Calculate the visual boundaries of all columns in character cells
    pub fn get_column_boundaries(&self, term_width: u16) -> Vec<u16> {
        let num = self.levels.len();
        let start = if num > 4 { num - 4 } else { 0 };
        let panes = &self.levels[start..];
        let np = panes.len();
        let has_preview = panes.last().map_or(false, |l| !l.files.is_empty());
        let n_cols = if has_preview { np + 1 } else { np };

        if n_cols == 0 {
            return Vec::new();
        }

        // Get default or custom ratios for the visible columns
        let start_idx = 5 - n_cols;
        let mut sub_ratios = self.column_ratios[start_idx..].to_vec();
        let sum: f32 = sub_ratios.iter().sum();
        if sum > 0.0 {
            for r in sub_ratios.iter_mut() {
                *r /= sum;
            }
        }

        let mut boundaries = Vec::new();
        let mut current_x = 0.0;
        for r in sub_ratios.iter() {
            let col_width = r * term_width as f32;
            current_x += col_width;
            boundaries.push(current_x.round() as u16);
        }
        boundaries
    }

    // ── Text-editing helpers for Rename / New Folder input ───────────────────

    fn byte_idx(&self, char_idx: usize) -> usize {
        self.input_buffer.char_indices().nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.input_buffer.len())
    }

    fn rename_input_replace_selection_with(&mut self, s: &str) {
        let (lo, hi) = if self.input_sel_start <= self.input_cursor {
            (self.input_sel_start, self.input_cursor)
        } else {
            (self.input_cursor, self.input_sel_start)
        };
        let bl = self.byte_idx(lo);
        let bh = self.byte_idx(hi);
        self.input_buffer.replace_range(bl..bh, s);
        let new_pos = lo + s.chars().count();
        self.input_cursor = new_pos;
        self.input_sel_start = new_pos;
    }

    /// Windows filenames may not contain: \ / : * ? " < > | and may not be "." or "..".
    fn validate_filename(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("Name can't be empty".into());
        }
        if name == "." || name == ".." {
            return Err("Invalid name".into());
        }
        const BAD: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        if name.chars().any(|c| BAD.contains(&c) || c.is_control()) {
            return Err("Name contains an invalid character (\\ / : * ? \" < > |)".into());
        }
        if name.trim_end_matches('.') != name || name.trim_end() != name {
            return Err("Name can't end with a space or a period".into());
        }
        Ok(())
    }

    /// The set of paths an operation (copy/cut/delete) should act on:
    /// the marked (multi-)selection if any, otherwise just the highlighted row.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        let cur = self.current();
        if !cur.marked.is_empty() {
            return cur.marked.iter().cloned().collect();
        }
        if cur.files.is_empty() { return Vec::new(); }
        vec![cur.files[cur.selected].path.clone()]
    }

    pub fn toggle_mark(&mut self, idx: usize) {
        let cur = self.current_mut();
        if idx >= cur.files.len() { return; }
        let path = cur.files[idx].path.clone();
        if !cur.marked.remove(&path) {
            cur.marked.insert(path);
        }
        cur.selected = idx;
        cur.select_anchor = idx;
    }

    pub fn mark_range(&mut self, to: usize) {
        let cur = self.current_mut();
        if cur.files.is_empty() { return; }
        let to = to.min(cur.files.len() - 1);
        let (lo, hi) = if cur.select_anchor <= to { (cur.select_anchor, to) } else { (to, cur.select_anchor) };
        for i in lo..=hi {
            cur.marked.insert(cur.files[i].path.clone());
        }
        cur.selected = to;
    }

    fn clear_marks(&mut self) {
        self.current_mut().marked.clear();
    }

    // ── Rename ─────────────────────────────────────────────────────────────

    pub fn start_rename(&mut self) {
        let cur = self.current();
        if cur.files.is_empty() { return; }
        let name = cur.files[cur.selected].path.file_name()
            .unwrap_or_default().to_string_lossy().to_string();
        let is_dir = cur.files[cur.selected].is_dir;
        self.input_buffer = name.clone();
        // Select the basename but not the extension (files only).
        let sel_end = if is_dir {
            name.chars().count()
        } else {
            match name.rfind('.') {
                Some(byte_pos) if byte_pos > 0 => name[..byte_pos].chars().count(),
                _ => name.chars().count(),
            }
        };
        self.input_sel_start = 0;
        self.input_cursor = sel_end;
        self.mode = AppMode::Rename;
    }

    fn commit_rename(&mut self) {
        let new_name = self.input_buffer.trim().to_string();
        if let Err(e) = Self::validate_filename(&new_name) {
            self.set_notice(e, true);
            self.mode = AppMode::Normal;
            return;
        }
        let cur = self.current();
        if cur.files.is_empty() { self.mode = AppMode::Normal; return; }
        let old_path = cur.files[cur.selected].path.clone();
        let mut new_path = old_path.clone();
        new_path.set_file_name(&new_name);

        if new_path == old_path {
            self.mode = AppMode::Normal;
            return;
        }
        if new_path.exists() {
            self.set_notice(format!("\"{}\" already exists", new_name), true);
            self.mode = AppMode::Normal;
            return;
        }
        match fs::rename(&old_path, &new_path) {
            Ok(_) => {
                if let Some(parent) = old_path.parent() {
                    DIR_CACHE.lock().remove(parent);
                    if let Ok(files) = list_dir(parent) {
                        let cur = self.current_mut();
                        cur.files = files;
                        // Preserve selection on the renamed item.
                        if let Some(pos) = cur.files.iter().position(|f| f.path == new_path) {
                            cur.selected = pos;
                        }
                        cur.marked.clear();
                    }
                }
                self.set_notice(format!("Renamed to \"{}\"", new_name), false);
            }
            Err(e) => self.set_notice(format!("Rename failed: {}", e), true),
        }
        self.mode = AppMode::Normal;
    }

    // ── New folder ─────────────────────────────────────────────────────────

    fn commit_new_folder(&mut self) {
        let name = self.input_buffer.trim().to_string();
        if let Err(e) = Self::validate_filename(&name) {
            self.set_notice(e, true);
            self.mode = AppMode::Normal;
            return;
        }
        let parent = self.current().path.clone();
        let new_dir = parent.join(&name);
        if new_dir.exists() {
            self.set_notice(format!("\"{}\" already exists", name), true);
            self.mode = AppMode::Normal;
            return;
        }
        match fs::create_dir(&new_dir) {
            Ok(_) => {
                DIR_CACHE.lock().remove(&parent);
                if let Ok(files) = list_dir(&parent) {
                    let cur = self.current_mut();
                    cur.files = files;
                    if let Some(pos) = cur.files.iter().position(|f| f.path == new_dir) {
                        cur.selected = pos;
                    }
                }
                self.set_notice(format!("Created folder \"{}\"", name), false);
            }
            Err(e) => self.set_notice(format!("Couldn't create folder: {}", e), true),
        }
        self.mode = AppMode::Normal;
    }

    // ── Delete ─────────────────────────────────────────────────────────────

    fn do_delete(&mut self, permanent: bool) {
        let paths = self.selected_paths();
        if paths.is_empty() { self.mode = AppMode::Normal; return; }
        let mut errors = 0usize;
        let mut deleted = 0usize;
        for path in &paths {
            let result = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
            match result {
                Ok(_) => deleted += 1,
                Err(_) => errors += 1,
            }
        }
        if let Some(parent) = paths[0].parent().map(|p| p.to_path_buf()) {
            DIR_CACHE.lock().remove(&parent);
            if let Ok(files) = list_dir(&parent) {
                let cur = self.current_mut();
                cur.files = files;
                cur.marked.clear();
                if cur.selected >= cur.files.len() {
                    cur.selected = cur.files.len().saturating_sub(1);
                }
            }
        }
        let _ = permanent;
        if errors > 0 {
            self.set_notice(format!("Deleted {} item(s), {} failed (in use or access denied)", deleted, errors), errors == deleted);
        } else {
            self.set_notice(format!("Deleted {} item(s)", deleted), false);
        }
        self.mode = AppMode::Normal;
    }

    // ── Paste ──────────────────────────────────────────────────────────────

    pub fn do_paste(&mut self) {
        let Some((srcs, is_cut)) = self.clipboard.clone() else { return; };
        let dest_dir = self.current().path.clone();
        let mut ok = 0usize;
        let mut errors: Vec<String> = Vec::new();

        for src_path in &srcs {
            let Some(file_name) = src_path.file_name() else { continue; };
            let mut dest_path = dest_dir.join(file_name);

            // Collision handling: never silently overwrite — auto-suffix instead.
            if dest_path.exists() && dest_path != *src_path {
                dest_path = unique_dest_path(&dest_dir, file_name);
            }
            if dest_path == *src_path {
                continue; // pasting into the same location as source; skip
            }

            let result = if is_cut {
                fs::rename(src_path, &dest_path)
                    .or_else(|_| {
                        // Cross-drive move: rename fails, fall back to copy+delete.
                        if src_path.is_dir() {
                            copy_dir_all(src_path, &dest_path)
                                .and_then(|_| fs::remove_dir_all(src_path))
                        } else {
                            fs::copy(src_path, &dest_path).map(|_| ())
                                .and_then(|_| fs::remove_file(src_path))
                        }
                    })
            } else if src_path.is_dir() {
                copy_dir_all(src_path, &dest_path)
            } else {
                fs::copy(src_path, &dest_path).map(|_| ())
            };

            match result {
                Ok(_) => {
                    ok += 1;
                    if let Some(p) = src_path.parent() { DIR_CACHE.lock().remove(p); }
                }
                Err(e) => errors.push(format!("{}: {}", file_name.to_string_lossy(), e)),
            }
        }

        DIR_CACHE.lock().remove(&dest_dir);
        if let Ok(files) = list_dir(&dest_dir) {
            self.current_mut().files = files;
        }
        if is_cut && errors.is_empty() {
            self.clipboard = None;
        }
        if errors.is_empty() {
            self.set_notice(format!("Pasted {} item(s)", ok), false);
        } else {
            self.set_notice(format!("Pasted {} item(s), {} failed", ok, errors.len()), true);
        }
    }

    // ── Context menu ──────────────────────────────────────────────────────

    pub fn open_context_menu(&mut self, path: Option<PathBuf>) {
        self.context_menu_hover = None;
        self.context_menu_target = path.clone();
        let has_selection = path.is_some();
        let has_clipboard = self.clipboard.is_some();
        let mut items = Vec::new();
        if has_selection {
            items.push(ContextAction::Open);
            items.push(ContextAction::OpenWith);
            items.push(ContextAction::Cut);
            items.push(ContextAction::Copy);
        }
        if has_clipboard { items.push(ContextAction::Paste); }
        if has_selection {
            items.push(ContextAction::Rename);
            items.push(ContextAction::Delete);
            items.push(ContextAction::CopyPath);
        }
        items.push(ContextAction::OpenInTerminal);
        items.push(ContextAction::NewFolder);
        if has_selection { items.push(ContextAction::Properties); }
        self.context_menu_items = items;
        self.mode = AppMode::ContextMenu;
    }

    pub fn apply_context_action(&mut self, action: ContextAction) {
        let target = self.context_menu_target.clone();
        self.mode = AppMode::Normal;
        match action {
            ContextAction::Open => { self.open_selected(); }
            ContextAction::OpenWith => self.do_open_with(target),
            ContextAction::Cut => {
                let paths = self.selected_paths();
                if !paths.is_empty() { self.clipboard = Some((paths, true)); }
            }
            ContextAction::Copy => {
                let paths = self.selected_paths();
                if !paths.is_empty() { self.clipboard = Some((paths, false)); }
            }
            ContextAction::Paste => self.do_paste(),
            ContextAction::Rename => self.start_rename(),
            ContextAction::Delete => self.mode = AppMode::ConfirmDelete,
            ContextAction::Properties => self.mode = AppMode::Properties,
            ContextAction::CopyPath => self.do_copy_path(target),
            ContextAction::OpenInTerminal => self.do_open_in_terminal(),
            ContextAction::NewFolder => {
                self.mode = AppMode::NewFolder;
                self.input_buffer.clear();
                self.input_cursor = 0;
                self.input_sel_start = 0;
            }
        }
    }

    fn do_copy_path(&mut self, target: Option<PathBuf>) {
        let Some(path) = target.or_else(|| self.selected_entry_path()).or_else(|| Some(self.current().path.clone())) else { return; };
        let text = path.to_string_lossy().to_string();
        #[cfg(windows)]
        {
            use std::io::Write;
            if let Ok(mut child) = std::process::Command::new("cmd")
                .args(["/C", "clip"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        }
        self.set_notice(format!("Copied path: {}", text), false);
    }

    fn do_open_in_terminal(&mut self) {
        let dir = self.current().path.clone();
        let dir_str = dir.to_string_lossy().to_string();
        #[cfg(windows)]
        {
            if std::process::Command::new("wt").arg("-d").arg(&dir_str).spawn().is_err() {
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "cmd", "/K", "cd", "/d"])
                    .arg(&dir_str)
                    .spawn();
            }
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("x-terminal-emulator")
                .current_dir(&dir)
                .spawn();
        }
    }

    fn do_open_with(&mut self, target: Option<PathBuf>) {
        let Some(path) = target.or_else(|| self.selected_entry_path()) else { return; };
        #[cfg(windows)]
        {
            let path_str = path.to_string_lossy().to_string();
            let _ = std::process::Command::new("rundll32")
                .arg("shell32.dll,OpenAs_RunDLL")
                .arg(&path_str)
                .spawn();
        }
        #[cfg(not(windows))]
        { let _ = path; }
    }

    pub fn toggle_setting(&mut self, action: ToggleAction) {
        match action {
            ToggleAction::Theme => {
                self.theme_mode = match self.theme_mode {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                };
                self.set_notice(format!("Theme: {}", if self.theme_mode == ThemeMode::Dark { "Dark Mode" } else { "Light Mode" }), false);
            }
            ToggleAction::OfficeMode => {
                self.office_mode = match self.office_mode {
                    OfficeRenderMode::Text => OfficeRenderMode::Full,
                    OfficeRenderMode::Full => OfficeRenderMode::Text,
                };
                self.set_notice(format!("Office mode: {}", if self.office_mode == OfficeRenderMode::Text { "Text" } else { "Full (Visual)" }), false);
            }
            ToggleAction::PdfMode => {
                self.pdf_mode = match self.pdf_mode {
                    PdfRenderMode::Text => PdfRenderMode::Visual,
                    PdfRenderMode::Visual => PdfRenderMode::Text,
                };
                self.set_notice(format!("PDF mode: {}", if self.pdf_mode == PdfRenderMode::Text { "Text" } else { "Visual" }), false);
            }
            ToggleAction::EditMode => {
                self.edit_preview_mode = !self.edit_preview_mode;
                if self.edit_preview_mode {
                    self.sync_edit_buffer_with_selected();
                }
                self.set_notice(format!("Preview Edit Mode: {}", if self.edit_preview_mode { "ON" } else { "OFF" }), false);
            }
            ToggleAction::DirPreviewClick => {
                self.dir_preview_clickable = !self.dir_preview_clickable;
                self.set_notice(format!("Directory Preview Clickable: {}", if self.dir_preview_clickable { "ON" } else { "OFF" }), false);
            }
        }
    }

    pub fn sync_edit_buffer_with_selected(&mut self) {
        if let Some(p) = self.selected_file() {
            if p.is_file() {
                if let Ok(content) = fs::read_to_string(&p) {
                    self.edit_buffer = content.lines().map(|s| s.to_string()).collect();
                    if self.edit_buffer.is_empty() {
                        self.edit_buffer.push(String::new());
                    }
                    self.edit_cursor_row = 0;
                    self.edit_cursor_col = 0;
                    self.edit_dirty = false;
                    self.edit_path = Some(p);
                }
            }
        }
    }

    pub fn save_edited_preview(&mut self) {
        if let Some(path) = &self.edit_path {
            let text = self.edit_buffer.join("\n");
            match fs::write(path, text) {
                Ok(_) => {
                    self.edit_dirty = false;
                    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.set_notice(format!("✓ Saved {}", file_name), false);
                }
                Err(e) => {
                    self.set_notice(format!("⚠ Failed to save: {}", e), true);
                }
            }
        }
    }

    pub fn handle_edit_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if !self.edit_preview_mode || self.edit_buffer.is_empty() {
            return false;
        }

        if modifiers.contains(KeyModifiers::CONTROL) {
            if code == KeyCode::Char('s') || code == KeyCode::Char('S') {
                self.save_edited_preview();
                return true;
            }
        }

        match code {
            KeyCode::Char(c) if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let line = &mut self.edit_buffer[self.edit_cursor_row];
                    let idx = line.char_indices().nth(self.edit_cursor_col).map(|(i, _)| i).unwrap_or(line.len());
                    line.insert(idx, c);
                    self.edit_cursor_col += 1;
                    self.edit_dirty = true;
                    return true;
                }
            }
            KeyCode::Enter => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let line = self.edit_buffer[self.edit_cursor_row].clone();
                    let idx = line.char_indices().nth(self.edit_cursor_col).map(|(i, _)| i).unwrap_or(line.len());
                    let (head, tail) = line.split_at(idx);
                    self.edit_buffer[self.edit_cursor_row] = head.to_string();
                    self.edit_buffer.insert(self.edit_cursor_row + 1, tail.to_string());
                    self.edit_cursor_row += 1;
                    self.edit_cursor_col = 0;
                    self.edit_dirty = true;
                    return true;
                }
            }
            KeyCode::Backspace => {
                if self.edit_cursor_col > 0 {
                    let line = &mut self.edit_buffer[self.edit_cursor_row];
                    let idx = line.char_indices().nth(self.edit_cursor_col - 1).map(|(i, _)| i).unwrap_or(0);
                    let end = line.char_indices().nth(self.edit_cursor_col).map(|(i, _)| i).unwrap_or(line.len());
                    line.replace_range(idx..end, "");
                    self.edit_cursor_col -= 1;
                    self.edit_dirty = true;
                    return true;
                } else if self.edit_cursor_row > 0 {
                    let curr = self.edit_buffer.remove(self.edit_cursor_row);
                    self.edit_cursor_row -= 1;
                    let prev = &mut self.edit_buffer[self.edit_cursor_row];
                    self.edit_cursor_col = prev.chars().count();
                    prev.push_str(&curr);
                    self.edit_dirty = true;
                    return true;
                }
            }
            KeyCode::Delete => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let len = self.edit_buffer[self.edit_cursor_row].chars().count();
                    if self.edit_cursor_col < len {
                        let line = &mut self.edit_buffer[self.edit_cursor_row];
                        let idx = line.char_indices().nth(self.edit_cursor_col).map(|(i, _)| i).unwrap_or(0);
                        let end = line.char_indices().nth(self.edit_cursor_col + 1).map(|(i, _)| i).unwrap_or(line.len());
                        line.replace_range(idx..end, "");
                        self.edit_dirty = true;
                        return true;
                    } else if self.edit_cursor_row + 1 < self.edit_buffer.len() {
                        let next = self.edit_buffer.remove(self.edit_cursor_row + 1);
                        self.edit_buffer[self.edit_cursor_row].push_str(&next);
                        self.edit_dirty = true;
                        return true;
                    }
                }
            }
            KeyCode::Up => {
                if self.edit_cursor_row > 0 {
                    self.edit_cursor_row -= 1;
                    let len = self.edit_buffer[self.edit_cursor_row].chars().count();
                    self.edit_cursor_col = self.edit_cursor_col.min(len);
                    return true;
                }
            }
            KeyCode::Down => {
                if self.edit_cursor_row + 1 < self.edit_buffer.len() {
                    self.edit_cursor_row += 1;
                    let len = self.edit_buffer[self.edit_cursor_row].chars().count();
                    self.edit_cursor_col = self.edit_cursor_col.min(len);
                    return true;
                }
            }
            KeyCode::Left => {
                if self.edit_cursor_col > 0 {
                    self.edit_cursor_col -= 1;
                    return true;
                } else if self.edit_cursor_row > 0 {
                    self.edit_cursor_row -= 1;
                    self.edit_cursor_col = self.edit_buffer[self.edit_cursor_row].chars().count();
                    return true;
                }
            }
            KeyCode::Right => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let len = self.edit_buffer[self.edit_cursor_row].chars().count();
                    if self.edit_cursor_col < len {
                        self.edit_cursor_col += 1;
                        return true;
                    } else if self.edit_cursor_row + 1 < self.edit_buffer.len() {
                        self.edit_cursor_row += 1;
                        self.edit_cursor_col = 0;
                        return true;
                    }
                }
            }
            KeyCode::Home => {
                self.edit_cursor_col = 0;
                return true;
            }
            KeyCode::End => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    self.edit_cursor_col = self.edit_buffer[self.edit_cursor_row].chars().count();
                    return true;
                }
            }
            KeyCode::Tab => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let line = &mut self.edit_buffer[self.edit_cursor_row];
                    let idx = line.char_indices().nth(self.edit_cursor_col).map(|(i, _)| i).unwrap_or(line.len());
                    line.insert_str(idx, "    ");
                    self.edit_cursor_col += 4;
                    self.edit_dirty = true;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

// ── Text/code extension check ─────────────────────────────────────────────────

pub fn is_text_or_code(p: &PathBuf) -> bool {
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    matches!(ext.as_str(),
        "rs"|"py"|"js"|"ts"|"tsx"|"jsx"|"html"|"htm"|"css"|"scss"|
        "json"|"toml"|"yaml"|"yml"|"sh"|"bash"|"zsh"|"ps1"|"bat"|
        "c"|"cpp"|"h"|"hpp"|"java"|"kt"|"go"|"rb"|"php"|"cs"|"lua"|
        "sql"|"xml"|"md"|"markdown"|"txt"|"log"|"ini"|"cfg"|"conf"|
        "swift"|"dart"|"r"|"jl"|"hs"|"ex"|"exs"|"vue"|"svelte"|
        "astro"|"rtf"|"csv"|"tsv"|"rst"
    )
}

// ── Directory listing ─────────────────────────────────────────────────────────

pub fn list_dir<P: AsRef<std::path::Path>>(path: P) -> anyhow::Result<Vec<DirEntryInfo>> {
    let path_ref = path.as_ref();
    let path_buf = path_ref.to_path_buf();

    // Special sentinel for the Windows drives list
    #[cfg(windows)]
    if path_ref == std::path::Path::new("\\\\drives") {
        return Ok(list_windows_drives());
    }

    // Return cached result if still fresh
    {
        let cache = DIR_CACHE.lock();
        if let Some(entry) = cache.get(&path_buf) {
            if entry.at.elapsed() < DIR_CACHE_TTL {
                return Ok(entry.entries.clone());
            }
        }
    }

    let mut entries: Vec<_> = fs::read_dir(path_ref)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let path = e.path();
            let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            DirEntryInfo { path, is_dir }
        })
        .take(MAX_DIR_ENTRIES) // cap to prevent Sort + UI lag on huge dirs
        .collect();

    // Sort: directories first, then files, both alphabetically (case-insensitive)
    entries.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let a_name = a.path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                let b_name = b.path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                a_name.cmp(&b_name)
            }
        }
    });

    // Store in cache
    DIR_CACHE.lock().insert(path_buf, DirCacheEntry {
        entries: entries.clone(),
        at:      Instant::now(),
    });

    Ok(entries)
}

/// Enumerate all available Windows drive letters (A:\ … Z:\).
#[cfg(windows)]
pub fn list_windows_drives() -> Vec<DirEntryInfo> {
    // GetLogicalDrives() returns a bitmask — bit 0 = A, bit 1 = B, …, bit 25 = Z
    extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    let mask = unsafe { GetLogicalDrives() };
    (0u32..26)
        .filter(|&i| mask & (1 << i) != 0)
        .map(|i| {
            let letter = (b'A' + i as u8) as char;
            let path = PathBuf::from(format!("{}:\\", letter));
            DirEntryInfo { path, is_dir: true }
        })
        .collect()
}

// ── Recursive copy helper ─────────────────────────────────────────────────────

fn copy_dir_all(src: impl AsRef<std::path::Path>, dst: impl AsRef<std::path::Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Generate a non-colliding destination path by appending " (2)", " (3)", …
/// before the extension, matching Windows Explorer's paste-collision behavior.
fn unique_dest_path(dir: &PathBuf, file_name: &std::ffi::OsStr) -> PathBuf {
    let name = file_name.to_string_lossy().to_string();
    let (stem, ext) = match name.rfind('.') {
        Some(pos) if pos > 0 => (name[..pos].to_string(), name[pos..].to_string()),
        _ => (name.clone(), String::new()),
    };
    for n in 2..10_000 {
        let candidate = dir.join(format!("{} ({}){}", stem, n, ext));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

// ── Drive info (left sidebar) ─────────────────────────────────────────────────

#[cfg(windows)]
pub fn list_drives_info() -> Vec<DriveInfo> {
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(lp_root_path_name: *const u16) -> u32;
        fn GetDiskFreeSpaceExW(
            lp_directory_name: *const u16,
            lp_free_bytes_available: *mut u64,
            lp_total_number_of_bytes: *mut u64,
            lp_total_number_of_free_bytes: *mut u64,
        ) -> i32;
        fn GetVolumeInformationW(
            lp_root_path_name: *const u16,
            lp_volume_name_buffer: *mut u16,
            n_volume_name_size: u32,
            lp_volume_serial_number: *mut u32,
            lp_maximum_component_length: *mut u32,
            lp_file_system_flags: *mut u32,
            lp_file_system_name_buffer: *mut u16,
            n_file_system_name_size: u32,
        ) -> i32;
    }

    use std::os::windows::ffi::OsStrExt;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    fn drive_kind(t: u32) -> &'static str {
        match t {
            2 => "Removable",
            3 => "Fixed",
            4 => "Network",
            5 => "CD-ROM",
            6 => "RAM Disk",
            _ => "Unknown",
        }
    }

    let mask = unsafe { GetLogicalDrives() };
    (0u32..26)
        .filter(|&i| mask & (1 << i) != 0)
        .map(|i| {
            let letter = (b'A' + i as u8) as char;
            let root = format!("{}:\\", letter);
            let root_w = wide(&root);
            let path = PathBuf::from(&root);

            let kind_code = unsafe { GetDriveTypeW(root_w.as_ptr()) };
            let kind = drive_kind(kind_code).to_string();

            let mut free_avail: u64 = 0;
            let mut total: u64 = 0;
            let mut total_free: u64 = 0;
            let has_space = kind_code != 5 /* skip empty CD-ROM trays */
                && unsafe { GetDiskFreeSpaceExW(root_w.as_ptr(), &mut free_avail, &mut total, &mut total_free) } != 0;

            let mut vol_name = [0u16; 128];
            let mut fs_name = [0u16; 32];
            let got_vol = unsafe {
                GetVolumeInformationW(
                    root_w.as_ptr(),
                    vol_name.as_mut_ptr(), vol_name.len() as u32,
                    std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(),
                    fs_name.as_mut_ptr(), fs_name.len() as u32,
                )
            } != 0;

            let label = if got_vol {
                let end = vol_name.iter().position(|&c| c == 0).unwrap_or(0);
                let s = String::from_utf16_lossy(&vol_name[..end]);
                if s.is_empty() { format!("Local Disk ({}:)", letter) } else { format!("{} ({}:)", s, letter) }
            } else {
                format!("{} ({}:)", kind, letter)
            };
            let fs = if got_vol {
                let end = fs_name.iter().position(|&c| c == 0).unwrap_or(0);
                String::from_utf16_lossy(&fs_name[..end])
            } else { String::new() };

            DriveInfo {
                path,
                label,
                fs,
                kind,
                total: if has_space { total } else { 0 },
                free: if has_space { total_free } else { 0 },
            }
        })
        .collect()
}

#[cfg(not(windows))]
pub fn list_drives_info() -> Vec<DriveInfo> {
    vec![DriveInfo {
        path: PathBuf::from("/"),
        label: "Root".to_string(),
        fs: String::new(),
        kind: "Fixed".to_string(),
        total: 0,
        free: 0,
    }]
}
