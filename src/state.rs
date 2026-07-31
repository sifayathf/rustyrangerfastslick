// ================= src/state.rs =================
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use ratatui::layout::Rect;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::{Duration, Instant, SystemTime},
};
use unicode_segmentation::UnicodeSegmentation;

// ── Directory listing cache ───────────────────────────────────────────────────
// TTL prevents re-reading the filesystem every frame when the preview pane
// hovers over a directory. Rendering is virtualized, so listings are never
// silently truncated.

const DIR_CACHE_TTL: Duration = Duration::from_secs(2);

#[inline]
fn point_in(col: u16, row: u16, r: &Rect) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

#[derive(Clone)]
pub struct DirEntryInfo {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

struct DirCacheEntry {
    entries: Vec<DirEntryInfo>,
    at: Instant,
}

static DIR_CACHE: Lazy<Mutex<HashMap<PathBuf, DirCacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
pub struct DirLevel {
    pub path: PathBuf,
    pub files: Vec<DirEntryInfo>,
    pub selected: usize,
    /// First row shown in this pane. Kept separate from `selected` so the
    /// mouse wheel scrolls the list without changing or opening a file.
    pub scroll: usize,
    /// Multi-selected (marked) entries, keyed by absolute path.
    pub marked: std::collections::HashSet<PathBuf>,
    /// Anchor index for Shift+Click range selection.
    pub select_anchor: usize,
}

impl DirLevel {
    pub fn new(path: PathBuf, files: Vec<DirEntryInfo>) -> Self {
        Self {
            path,
            files,
            selected: 0,
            scroll: 0,
            marked: std::collections::HashSet::new(),
            select_anchor: 0,
        }
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
    NewFile,
    Properties,
}

// ── Drive sidebar ──────────────────────────────────────────────────────────
#[derive(Clone)]
pub struct DriveInfo {
    pub path: PathBuf,
    pub label: String,
    pub fs: String,
    pub kind: String, // "Fixed" | "Removable" | "Network" | "CD-ROM" | "Unknown"
    pub total: u64,
    pub free: u64,
}

// ── Right-click context menu ─────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Open,
    Play,
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
    NewFile,
}

impl ContextAction {
    pub fn label(&self) -> &'static str {
        match self {
            ContextAction::Open => "Open",
            ContextAction::Play => "Play",
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
            ContextAction::NewFile => "New File...",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Miller,
    Explorer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OfficeRenderMode {
    Text,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PdfRenderMode {
    Text,
    Visual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewMode {
    Normal,
    Full,
    Blitz,
}

impl PreviewMode {
    pub fn is_blitz(self) -> bool {
        self == PreviewMode::Blitz
    }

    pub fn office_policy(self) -> OfficeRenderMode {
        match self {
            PreviewMode::Full => OfficeRenderMode::Full,
            PreviewMode::Normal | PreviewMode::Blitz => OfficeRenderMode::Text,
        }
    }

    pub fn pdf_policy(self) -> PdfRenderMode {
        match self {
            PreviewMode::Full => PdfRenderMode::Visual,
            PreviewMode::Normal | PreviewMode::Blitz => PdfRenderMode::Text,
        }
    }
}

fn preview_settle_delay(mode: PreviewMode) -> Duration {
    match mode {
        PreviewMode::Normal => Duration::from_millis(80),
        PreviewMode::Full => Duration::from_millis(160),
        PreviewMode::Blitz => Duration::from_millis(40),
    }
}

#[derive(Clone, Default)]
pub struct FolderStats {
    pub bytes: u64,
    pub files: u64,
    pub folders: u64,
    pub inaccessible: u64,
    pub complete: bool,
}

struct PropertyScan {
    receiver: Receiver<FolderStats>,
    cancel: Arc<AtomicBool>,
}

fn grapheme_byte_index(value: &str, index: usize) -> usize {
    value
        .grapheme_indices(true)
        .nth(index)
        .map(|(byte, _)| byte)
        .unwrap_or(value.len())
}

fn grapheme_count(value: &str) -> usize {
    value.graphemes(true).count()
}

#[cfg(windows)]
fn system_double_click_interval() -> Duration {
    let milliseconds =
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
    Duration::from_millis(milliseconds.max(100) as u64)
}

#[cfg(not(windows))]
fn system_double_click_interval() -> Duration {
    Duration::from_millis(500)
}

/// Terminal mouse events can arrive noticeably later than native window messages,
/// especially when a redraw falls between the two button presses. Honour the OS
/// preference while allowing enough headroom for that delivery latency.
fn primary_double_click_interval() -> Duration {
    system_double_click_interval().max(Duration::from_millis(750))
}

fn is_repeated_primary_click(
    last_click: Option<(Instant, usize, usize)>,
    now: Instant,
    pane: usize,
    file: usize,
    interval: Duration,
) -> bool {
    matches!(last_click,
        Some((last_time, last_pane, last_file))
            if last_pane == pane
                && last_file == file
                && now.checked_duration_since(last_time).is_some_and(|elapsed| elapsed <= interval))
}

enum DirectoryWatchEvent {
    Snapshot {
        path: PathBuf,
        entries: Vec<DirEntryInfo>,
    },
    Error {
        path: PathBuf,
        message: String,
    },
}

struct DirectoryWatch {
    path: PathBuf,
    receiver: Receiver<DirectoryWatchEvent>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
enum NavigationAction {
    Replace { select_path: Option<PathBuf> },
    Push { origin_level: usize },
}

enum NavigationEvent {
    Complete(Vec<DirEntryInfo>),
    Error(String),
}

struct NavigationTask {
    generation: u64,
    path: PathBuf,
    action: NavigationAction,
    receiver: Receiver<NavigationEvent>,
}

#[derive(Clone)]
pub struct OperationStatus {
    pub label: String,
    pub done: usize,
    pub total: usize,
}

enum OperationEvent {
    Progress {
        label: String,
        done: usize,
        total: usize,
    },
    Complete {
        message: String,
        is_error: bool,
        clear_clipboard: bool,
        snapshots: Vec<(PathBuf, Vec<DirEntryInfo>)>,
    },
}

struct OperationTask {
    receiver: Receiver<OperationEvent>,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Modified,
    Size,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Name => "Name",
            SortMode::Modified => "Date",
            SortMode::Size => "Size",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToggleAction {
    Theme,
    LayoutMode,
    PreviewNormal,
    PreviewFull,
    PreviewBlitz,
    EditMode,
    DirPreviewClick,
    SortName,
    SortModified,
    SortSize,
    SortOrder,
    Details,
    SelectionStyle,
    FontFamily,
    FontSize,
    FontWeight,
    Hover,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewRequestKey {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    rotation: u32,
    flip_h: bool,
    page_index: usize,
    preview_mode: PreviewMode,
}

struct PreviewRequest {
    generation: u64,
    key: PreviewRequestKey,
}

struct PreviewWorkerEvent {
    generation: u64,
    key: PreviewRequestKey,
    page_count: Option<usize>,
    content: crate::preview::PreviewContent,
}

struct PreviewWorker {
    sender: mpsc::Sender<PreviewRequest>,
    receiver: Receiver<PreviewWorkerEvent>,
}

fn spawn_preview_worker() -> PreviewWorker {
    let (request_sender, request_receiver) = mpsc::channel::<PreviewRequest>();
    let (result_sender, result_receiver) = mpsc::channel::<PreviewWorkerEvent>();
    std::thread::spawn(move || {
        while let Ok(mut request) = request_receiver.recv() {
            // Navigation can outrun preview parsing. Discard queued obsolete
            // requests before doing any work and prepare only the newest file.
            while let Ok(newer) = request_receiver.try_recv() {
                request = newer;
            }
            let page_count = crate::preview::page_count(&request.key.path);
            let page_index = page_count
                .map(|count| request.key.page_index.min(count.saturating_sub(1)))
                .unwrap_or(request.key.page_index);
            let content = crate::preview::render(
                &request.key.path,
                request.key.rotation,
                request.key.flip_h,
                page_index,
                request.key.preview_mode,
            );
            if result_sender
                .send(PreviewWorkerEvent {
                    generation: request.generation,
                    key: request.key,
                    page_count,
                    content,
                })
                .is_err()
            {
                break;
            }
        }
    });
    PreviewWorker {
        sender: request_sender,
        receiver: result_receiver,
    }
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
    pub pane_level_indices: HashMap<usize, usize>,
    pub preview_rect: Option<Rect>,
    // Maps visible pane index to a list of (file_index, Rect)
    pub row_rects: HashMap<usize, Vec<(usize, Rect)>>,
    pub tile_columns: usize,
    pub tile_visible_items: usize,
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
    pub search_rect: Option<Rect>,
    pub pane_sort_rect: Option<Rect>,

    // Right-click context menu
    pub context_menu_rect: Option<Rect>,
    pub context_menu_item_rects: Vec<(Rect, ContextAction)>,
}

pub struct AppState {
    pub levels: Vec<DirLevel>,
    pub current_level: usize,
    last_event_time: Instant,
    event_revision: u64,
    last_selection_changed_at: Instant,

    // Settings & Toggles
    pub theme_mode: ThemeMode,
    pub layout_mode: LayoutMode,
    pub preview_mode: PreviewMode,
    pub office_mode: OfficeRenderMode,
    pub pdf_mode: PdfRenderMode,
    pub edit_preview_mode: bool,
    pub dir_preview_clickable: bool,
    pub preview_page_index: usize,
    pub preview_page_count: Option<usize>,
    pub preview_focused: bool,
    pub sort_mode: SortMode,
    pub sort_descending: bool,
    pub show_file_details: bool,
    pub rounded_selection: bool,
    pub font_face: String,
    pub font_size: u16,
    pub font_weight: u16,
    pub hover_enabled: bool,
    pub search_active: bool,
    pub search_query: String,
    pub search_original_selection: Option<usize>,

    // Interactive Preview Editor state
    pub edit_buffer: Vec<String>,
    pub edit_cursor_row: usize,
    pub edit_cursor_col: usize,
    pub edit_dirty: bool,
    pub edit_path: Option<PathBuf>,
    pub edit_line_ending: String,

    // Preview interaction state
    pub mode: AppMode,                           // current application mode
    pub input_buffer: String,                    // used for Rename / New Folder
    pub input_cursor: usize,                     // cursor position within input_buffer (chars)
    pub input_sel_start: usize, // selection start (chars) — for Ctrl+A / basename select
    pub clipboard: Option<(Vec<PathBuf>, bool)>, // (paths, is_cut)
    pub image_zoom: f32,        // 1.0 = 100%; +/- adjust
    pub image_rotation: u32,    // 0 / 90 / 180 / 270 degrees
    pub image_flip_h: bool,     // 'f' flips horizontally
    pub preview_scroll: usize,  // manual scroll offset for preview pane

    preview_worker: PreviewWorker,
    preview_generation: u64,
    preview_requested: Option<PreviewRequestKey>,
    prepared_preview: Option<(PreviewRequestKey, crate::preview::PreviewContent)>,
    preview_retry_at: Option<Instant>,

    // Mouse double-click tracking: (time, pane_idx, file_idx)
    pub last_click: Option<(Instant, usize, usize)>,

    // Layout and resizing state
    pub dragging_divider: Option<usize>,
    pub dragging_sidebar: bool,
    pub column_ratios: Vec<f32>,
    pub layout_geometry: Arc<Mutex<LayoutGeometry>>,

    // Left sidebar: drives + quick access
    pub drives: Vec<DriveInfo>,
    pub quick_access: Vec<(&'static str, PathBuf)>,
    drives_refreshed_at: Instant,
    pub sidebar_width: u16,

    // Right-click context menu
    pub context_menu_target: Option<PathBuf>,
    context_menu_targets: Vec<PathBuf>,
    pending_delete_targets: Option<Vec<PathBuf>>,
    pub context_menu_items: Vec<ContextAction>,
    pub context_menu_hover: Option<usize>,
    pub hovered_row: Option<(usize, usize)>,
    pending_event: Option<Event>,
    pub pending_menu_pos: (u16, u16),

    // Stable Properties target and non-blocking recursive folder totals.
    pub properties_path: Option<PathBuf>,
    pub properties_stats: Option<FolderStats>,
    property_scan: Option<PropertyScan>,
    directory_watch: Option<DirectoryWatch>,

    navigation_generation: u64,
    navigation_task: Option<NavigationTask>,
    pub navigation_loading: Option<PathBuf>,

    operation_task: Option<OperationTask>,
    pub operation_status: Option<OperationStatus>,

    // Non-blocking status notifications (message, shown_at)
    pub notice: Option<(String, Instant, bool)>, // bool = is_error

    // Windows Native preview overlay manager
    pub native_preview: crate::native::NativePreviewManager,
}

impl AppState {
    pub fn new() -> anyhow::Result<Self> {
        let user_settings = crate::settings::load();
        let home = dirs::home_dir().unwrap_or_else(|| {
            #[cfg(windows)]
            {
                PathBuf::from("C:\\")
            }
            #[cfg(not(windows))]
            {
                PathBuf::from("/")
            }
        });
        let start_path = user_settings
            .last_location
            .as_deref()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .unwrap_or_else(|| home.clone());
        let initial_sort_mode = match user_settings.sort_mode.as_str() {
            "modified" => SortMode::Modified,
            "size" => SortMode::Size,
            _ => SortMode::Name,
        };
        let mut files = list_dir(&start_path)?;
        sort_dir_entries(&mut files, initial_sort_mode, user_settings.sort_descending);

        let level = DirLevel::new(start_path, files);

        let mut quick_access: Vec<(&'static str, PathBuf)> = Vec::new();
        // Store labels without embedded emoji. Emoji cell widths vary between
        // terminal/font combinations and used to leave the final character
        // outside the active-row highlight on other machines.
        if let Some(h) = dirs::home_dir() {
            quick_access.push(("Home", h));
        }
        if let Some(d) = dirs::desktop_dir() {
            quick_access.push(("Desktop", d));
        }
        if let Some(d) = dirs::document_dir() {
            quick_access.push(("Documents", d));
        }
        if let Some(d) = dirs::download_dir() {
            quick_access.push(("Downloads", d));
        }
        if let Some(d) = dirs::picture_dir() {
            quick_access.push(("Pictures", d));
        }

        Ok(Self {
            levels: vec![level],
            current_level: 0,
            last_event_time: Instant::now(),
            event_revision: 0,
            last_selection_changed_at: Instant::now(),
            theme_mode: if user_settings.theme_light {
                ThemeMode::Light
            } else {
                ThemeMode::Dark
            },
            layout_mode: if user_settings.explorer_view {
                LayoutMode::Explorer
            } else {
                LayoutMode::Miller
            },
            preview_mode: match user_settings.preview_mode.as_str() {
                "full" => PreviewMode::Full,
                "blitz" => PreviewMode::Blitz,
                _ => PreviewMode::Normal,
            },
            office_mode: if user_settings.office_full {
                OfficeRenderMode::Full
            } else {
                OfficeRenderMode::Text
            },
            pdf_mode: if user_settings.pdf_visual {
                PdfRenderMode::Visual
            } else {
                PdfRenderMode::Text
            },
            edit_preview_mode: false,
            dir_preview_clickable: user_settings.dir_preview_clickable,
            preview_page_index: 0,
            preview_page_count: None,
            preview_focused: false,
            sort_mode: initial_sort_mode,
            sort_descending: user_settings.sort_descending,
            show_file_details: user_settings.show_file_details,
            rounded_selection: user_settings.rounded_selection,
            font_face: user_settings.font_face,
            font_size: user_settings.font_size,
            font_weight: user_settings.font_weight,
            hover_enabled: user_settings.hover_enabled,
            search_active: false,
            search_query: String::new(),
            search_original_selection: None,
            edit_buffer: Vec::new(),
            edit_cursor_row: 0,
            edit_cursor_col: 0,
            edit_dirty: false,
            edit_path: None,
            edit_line_ending: "\n".to_string(),
            mode: AppMode::Normal,
            input_buffer: String::new(),
            input_cursor: 0,
            input_sel_start: 0,
            clipboard: None,
            last_click: None,
            dragging_divider: None,
            dragging_sidebar: false,
            column_ratios: user_settings.column_ratios,
            layout_geometry: Arc::new(Mutex::new(LayoutGeometry::default())),
            preview_scroll: 0,
            preview_worker: spawn_preview_worker(),
            preview_generation: 0,
            preview_requested: None,
            prepared_preview: None,
            preview_retry_at: None,
            image_zoom: 1.0,
            image_rotation: 0,
            image_flip_h: false,
            drives: list_drives_info(),
            quick_access,
            drives_refreshed_at: Instant::now(),
            sidebar_width: user_settings.sidebar_width,
            context_menu_target: None,
            context_menu_targets: Vec::new(),
            pending_delete_targets: None,
            context_menu_items: Vec::new(),
            context_menu_hover: None,
            hovered_row: None,
            pending_event: None,
            pending_menu_pos: (0, 0),
            properties_path: None,
            properties_stats: None,
            property_scan: None,
            directory_watch: None,
            navigation_generation: 0,
            navigation_task: None,
            navigation_loading: None,
            operation_task: None,
            operation_status: None,
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

    /// Poll background services once per frame. Keeping this separate from
    /// input handling lets filesystem and preview work complete even while
    /// the user is not pressing a key.
    pub fn tick_background(&mut self) {
        self.maybe_refresh_drives();
        self.refresh_property_scan();
        self.poll_navigation();
        self.sync_directory_watch();
        let mut events = Vec::new();
        let mut watch_disconnected = false;
        if let Some(watch) = self.directory_watch.as_ref() {
            loop {
                match watch.receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        watch_disconnected = true;
                        break;
                    }
                }
            }
        }
        if watch_disconnected {
            self.directory_watch = None;
        }
        for event in events {
            match event {
                DirectoryWatchEvent::Snapshot { path, entries } => {
                    self.apply_directory_snapshot(&path, entries)
                }
                DirectoryWatchEvent::Error { path, message } => {
                    if self.current().path == path {
                        self.set_notice(message, true);
                    }
                }
            }
        }
        self.poll_operation();
        self.poll_preview_worker();
        self.refresh_preview_request();
    }

    fn current_preview_request_key(&self) -> Option<PreviewRequestKey> {
        let path = self.selected_file()?;
        if path.is_dir() {
            return None;
        }
        let metadata = fs::metadata(&path).ok();
        Some(PreviewRequestKey {
            path,
            modified: metadata.as_ref().and_then(|value| value.modified().ok()),
            len: metadata.as_ref().map_or(0, |value| value.len()),
            rotation: self.image_rotation,
            flip_h: self.image_flip_h,
            page_index: self.preview_page_index,
            preview_mode: self.preview_mode,
        })
    }

    fn invalidate_preview_pipeline(&mut self, reset_page: bool) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview_requested = None;
        self.prepared_preview = None;
        self.preview_retry_at = None;
        self.preview_page_count = None;
        if reset_page {
            self.preview_page_index = 0;
        }
        self.native_preview.hide();
    }

    fn poll_preview_worker(&mut self) {
        let mut newest = None;
        while let Ok(event) = self.preview_worker.receiver.try_recv() {
            newest = Some(event);
        }
        let Some(event) = newest else {
            return;
        };
        if event.generation != self.preview_generation {
            return;
        }
        self.preview_requested = None;
        self.preview_page_count = event.page_count;
        if let Some(count) = event.page_count {
            let clamped = self.preview_page_index.min(count.saturating_sub(1));
            if clamped != self.preview_page_index {
                self.preview_page_index = clamped;
                self.preview_generation = self.preview_generation.wrapping_add(1);
                self.prepared_preview = None;
                self.preview_retry_at = None;
                return;
            }
        }
        let loading = matches!(
            &event.content,
            crate::preview::PreviewContent::Status(info)
                if info.kind == crate::preview::PreviewStatusKind::Loading
        );
        self.prepared_preview = Some((event.key, event.content));
        self.preview_retry_at = loading.then(|| Instant::now() + Duration::from_millis(300));
    }

    fn refresh_preview_request(&mut self) {
        if self.preview_debounce_active() {
            return;
        }
        let Some(key) = self.current_preview_request_key() else {
            self.preview_requested = None;
            self.prepared_preview = None;
            return;
        };
        if self
            .prepared_preview
            .as_ref()
            .is_some_and(|(prepared, _)| prepared != &key)
        {
            self.prepared_preview = None;
            self.preview_page_count = None;
        }
        if self.preview_requested.as_ref() == Some(&key) {
            return;
        }
        let prepared_loading = self.prepared_preview.as_ref().is_some_and(|(prepared, content)| {
            prepared == &key
                && matches!(content, crate::preview::PreviewContent::Status(info) if info.kind == crate::preview::PreviewStatusKind::Loading)
        });
        if self
            .prepared_preview
            .as_ref()
            .is_some_and(|(prepared, _)| prepared == &key)
            && !prepared_loading
        {
            return;
        }
        if self.preview_retry_at.is_some_and(|at| Instant::now() < at) {
            return;
        }
        self.preview_generation = self.preview_generation.wrapping_add(1);
        let generation = self.preview_generation;
        if self
            .preview_worker
            .sender
            .send(PreviewRequest {
                generation,
                key: key.clone(),
            })
            .is_ok()
        {
            self.preview_requested = Some(key);
            self.preview_retry_at = None;
        }
    }

    pub fn prepared_preview(&self) -> Option<&crate::preview::PreviewContent> {
        self.prepared_preview.as_ref().map(|(_, content)| content)
    }

    pub fn effective_office_mode(&self) -> OfficeRenderMode {
        self.preview_mode.office_policy()
    }

    pub fn effective_pdf_mode(&self) -> PdfRenderMode {
        self.preview_mode.pdf_policy()
    }

    pub fn is_paged_visual_selected(&self) -> bool {
        let extension = self
            .selected_file()
            .and_then(|path| path.extension().map(|value| value.to_os_string()))
            .and_then(|value| value.to_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        matches!(extension.as_str(), "ppt" | "pptx" | "odp" | "pdf")
    }

    pub fn step_preview_page(&mut self, delta: isize) {
        if !self.is_paged_visual_selected() {
            return;
        }
        let next = if delta.is_negative() {
            self.preview_page_index.saturating_sub(delta.unsigned_abs())
        } else {
            self.preview_page_index.saturating_add(delta as usize)
        };
        let next = self
            .preview_page_count
            .map(|count| next.min(count.saturating_sub(1)))
            .unwrap_or(next);
        if next != self.preview_page_index {
            self.preview_page_index = next;
            self.preview_generation = self.preview_generation.wrapping_add(1);
            self.preview_requested = None;
            self.prepared_preview = None;
            self.preview_retry_at = None;
            self.preview_scroll = 0;
            self.native_preview.hide();
        }
    }

    fn poll_navigation(&mut self) {
        let event = self
            .navigation_task
            .as_ref()
            .and_then(|task| match task.receiver.try_recv() {
                Ok(event) => Some(event),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(NavigationEvent::Error(
                    "Folder scan stopped unexpectedly".to_string(),
                )),
            });
        let Some(event) = event else {
            return;
        };
        let Some(task) = self.navigation_task.take() else {
            return;
        };
        if task.generation != self.navigation_generation {
            return;
        }
        self.navigation_loading = None;
        match event {
            NavigationEvent::Complete(entries) => {
                self.apply_navigation(task.path, entries, task.action);
            }
            NavigationEvent::Error(message) => self.set_notice(message, true),
        }
    }

    fn cancel_navigation(&mut self) {
        self.navigation_generation = self.navigation_generation.wrapping_add(1);
        self.navigation_task = None;
        self.navigation_loading = None;
    }

    fn cached_sorted_dir(&self, path: &std::path::Path) -> Option<Vec<DirEntryInfo>> {
        let mut entries = DIR_CACHE
            .lock()
            .get(path)
            .map(|entry| entry.entries.clone())?;
        sort_dir_entries(&mut entries, self.sort_mode, self.sort_descending);
        Some(entries)
    }

    fn request_navigation(&mut self, path: PathBuf, action: NavigationAction) {
        self.cancel_navigation();
        if let Some(entries) = self.cached_sorted_dir(&path) {
            self.apply_navigation(path, entries, action);
            return;
        }

        self.navigation_generation = self.navigation_generation.wrapping_add(1);
        let generation = self.navigation_generation;
        let (sender, receiver) = mpsc::channel();
        let worker_path = path.clone();
        let sort_mode = self.sort_mode;
        let sort_descending = self.sort_descending;
        std::thread::spawn(move || {
            let result = list_dir(&worker_path).map(|mut entries| {
                sort_dir_entries(&mut entries, sort_mode, sort_descending);
                entries
            });
            let event = match result {
                Ok(entries) => NavigationEvent::Complete(entries),
                Err(error) => NavigationEvent::Error(format!(
                    "Can't open {}: {}",
                    worker_path.display(),
                    error
                )),
            };
            let _ = sender.send(event);
        });
        self.navigation_loading = Some(path.clone());
        self.navigation_task = Some(NavigationTask {
            generation,
            path,
            action,
            receiver,
        });
    }

    fn apply_navigation(
        &mut self,
        path: PathBuf,
        entries: Vec<DirEntryInfo>,
        action: NavigationAction,
    ) {
        match action {
            NavigationAction::Replace { select_path } => {
                let mut level = DirLevel::new(path, entries);
                if let Some(target) = select_path {
                    if let Some(index) = level.files.iter().position(|entry| entry.path == target) {
                        level.selected = index;
                        level.select_anchor = index;
                    }
                }
                self.levels = vec![level];
                self.current_level = 0;
            }
            NavigationAction::Push { origin_level } => {
                if origin_level >= self.levels.len() {
                    return;
                }
                self.levels.truncate(origin_level + 1);
                self.levels.push(DirLevel::new(path, entries));
                self.current_level = origin_level + 1;
            }
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, _, is_error)| *is_error)
        {
            self.notice = None;
        }
        self.reset_image_state();
    }

    fn poll_operation(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(task) = self.operation_task.as_ref() {
            loop {
                match task.receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let mut completed = false;
        for event in events {
            match event {
                OperationEvent::Progress { label, done, total } => {
                    self.operation_status = Some(OperationStatus { label, done, total });
                }
                OperationEvent::Complete {
                    message,
                    is_error,
                    clear_clipboard,
                    snapshots,
                } => {
                    for (path, entries) in snapshots {
                        self.apply_directory_snapshot(&path, entries);
                    }
                    if clear_clipboard {
                        self.clipboard = None;
                    }
                    self.operation_status = None;
                    self.set_notice(message, is_error);
                    completed = true;
                }
            }
        }
        if completed {
            self.operation_task = None;
        } else if disconnected {
            self.operation_task = None;
            self.operation_status = None;
            self.set_notice("File operation stopped unexpectedly", true);
        }
    }

    fn sync_directory_watch(&mut self) {
        let path = self.current().path.clone();
        let replace = self
            .directory_watch
            .as_ref()
            .map(|watch| watch.path != path)
            .unwrap_or(true);
        if !replace {
            return;
        }
        if let Some(watch) = self.directory_watch.take() {
            watch.cancel.store(true, Ordering::Release);
        }
        self.directory_watch = Some(start_directory_watch(path));
    }

    fn apply_directory_snapshot(&mut self, path: &std::path::Path, mut entries: Vec<DirEntryInfo>) {
        sort_dir_entries(&mut entries, self.sort_mode, self.sort_descending);
        DIR_CACHE.lock().insert(
            path.to_path_buf(),
            DirCacheEntry {
                entries: entries.clone(),
                at: Instant::now(),
            },
        );

        for level in self.levels.iter_mut().filter(|level| level.path == path) {
            if directory_fingerprint(&level.files) == directory_fingerprint(&entries) {
                continue;
            }
            let selected_path = level
                .files
                .get(level.selected)
                .map(|entry| entry.path.clone());
            level.files = entries.clone();
            level
                .marked
                .retain(|marked| level.files.iter().any(|entry| &entry.path == marked));
            level.selected = selected_path
                .as_ref()
                .and_then(|selected| level.files.iter().position(|entry| &entry.path == selected))
                .unwrap_or_else(|| level.selected.min(level.files.len().saturating_sub(1)));
            level.select_anchor = level.selected;
            level.scroll = level.scroll.min(level.files.len().saturating_sub(1));
            crate::preview::invalidate_cached_dir(Some(path));
        }
    }

    pub fn set_notice(&mut self, msg: impl Into<String>, is_error: bool) {
        self.notice = Some((msg.into(), Instant::now(), is_error));
    }

    /// Active (non-expired) notice text, if any.
    pub fn active_notice(&self) -> Option<(&str, bool)> {
        self.notice.as_ref().and_then(|(msg, at, err)| {
            // Errors remain visible until the next successful action or an
            // explicit Escape. Transient success messages still expire.
            if *err || at.elapsed() < Duration::from_secs(4) {
                Some((msg.as_str(), *err))
            } else {
                None
            }
        })
    }

    pub fn open_properties(&mut self, path: PathBuf) {
        if let Some(scan) = self.property_scan.take() {
            scan.cancel.store(true, Ordering::Relaxed);
        }
        self.properties_path = Some(path.clone());
        self.properties_stats = None;
        self.mode = AppMode::Properties;

        if !path.is_dir() {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        std::thread::spawn(move || scan_folder_stats(path, sender, worker_cancel));
        self.properties_stats = Some(FolderStats::default());
        self.property_scan = Some(PropertyScan { receiver, cancel });
    }

    pub fn close_properties(&mut self) {
        if let Some(scan) = self.property_scan.take() {
            scan.cancel.store(true, Ordering::Relaxed);
        }
        self.properties_path = None;
        self.properties_stats = None;
        self.mode = AppMode::Normal;
    }

    fn refresh_property_scan(&mut self) {
        let mut finished = false;
        if let Some(scan) = self.property_scan.as_ref() {
            while let Ok(stats) = scan.receiver.try_recv() {
                finished = stats.complete;
                self.properties_stats = Some(stats);
            }
        }
        if finished {
            self.property_scan = None;
        }
    }

    /// Navigate the active pane directly to an arbitrary directory path
    /// (used by sidebar clicks, breadcrumb clicks, and drive navigation).
    pub fn navigate_to(&mut self, path: &std::path::Path) {
        self.navigate_to_select(path, None);
    }

    fn navigate_to_select(&mut self, path: &std::path::Path, select_path: Option<PathBuf>) {
        self.request_navigation(
            path.to_path_buf(),
            NavigationAction::Replace { select_path },
        );
    }

    pub fn current(&self) -> &DirLevel {
        &self.levels[self.current_level]
    }

    pub fn current_mut(&mut self) -> &mut DirLevel {
        &mut self.levels[self.current_level]
    }

    fn load_sorted_dir(&self, path: &std::path::Path) -> anyhow::Result<Vec<DirEntryInfo>> {
        let mut files = list_dir(path)?;
        sort_dir_entries(&mut files, self.sort_mode, self.sort_descending);
        Ok(files)
    }

    fn resort_levels(&mut self) {
        let mode = self.sort_mode;
        let descending = self.sort_descending;
        for level in &mut self.levels {
            let selected_path = level
                .files
                .get(level.selected)
                .map(|entry| entry.path.clone());
            sort_dir_entries(&mut level.files, mode, descending);
            if let Some(selected_path) = selected_path {
                if let Some(index) = level
                    .files
                    .iter()
                    .position(|entry| entry.path == selected_path)
                {
                    level.selected = index;
                    level.select_anchor = index;
                }
            }
            level.scroll = level.scroll.min(level.files.len().saturating_sub(1));
        }
    }

    pub fn handle_events(&mut self) -> anyhow::Result<bool> {
        self.refresh_property_scan();

        // Preserve the first non-movement event found while coalescing pointer
        // movement. This keeps keys/clicks lossless without replaying hundreds
        // of stale Windows Terminal mouse coordinates.
        let next_event = if let Some(pending) = self.pending_event.take() {
            pending
        } else {
            let input_wait = if self.preview_mode.is_blitz() {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(16)
            };
            if !event::poll(input_wait)? {
                return Ok(false);
            }
            event::read()?
        };

        self.event_revision = self.event_revision.wrapping_add(1);
        match next_event {
            Event::Key(key) => {
                // Releases are duplicates on Windows. Repeats are deliberate
                // input and make held navigation keys feel native.
                if key.kind == KeyEventKind::Release {
                    return Ok(false);
                }

                self.last_event_time = Instant::now();

                if self.edit_preview_mode
                    && self.mode == AppMode::Normal
                    && self.handle_edit_key(key.code, key.modifiers)
                {
                    return Ok(false);
                }

                if self.search_active {
                    match (key.code, key.modifiers) {
                        (KeyCode::Esc, _) => {
                            self.cancel_search();
                        }
                        (KeyCode::Enter, _) => {
                            self.accept_search();
                        }
                        (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                            self.accept_search();
                        }
                        (KeyCode::Backspace, _) => {
                            self.search_query.pop();
                            self.select_first_search_match();
                        }
                        (KeyCode::Down, _) | (KeyCode::Tab, _) => {
                            self.select_next_search_match(true);
                        }
                        (KeyCode::Up, _) | (KeyCode::BackTab, _) => {
                            self.select_next_search_match(false);
                        }
                        (KeyCode::Char(c), modifiers)
                            if modifiers == KeyModifiers::NONE
                                || modifiers == KeyModifiers::SHIFT =>
                        {
                            self.search_query.push(c);
                            self.select_first_search_match();
                        }
                        _ => {}
                    }
                    return Ok(false);
                }

                match (key.code, key.modifiers) {
                    // ── Quit ────────────────────────────────────────────────
                    // Only allow quit when in Normal mode
                    (KeyCode::Char('q'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        return Ok(true)
                    }
                    (KeyCode::Esc, _) if self.mode == AppMode::Properties => {
                        self.close_properties();
                    }
                    (KeyCode::Esc, _) if self.mode != AppMode::Normal => {
                        // Escape cancels modes
                        self.mode = AppMode::Normal;
                    }
                    (KeyCode::Esc, _) if self.operation_task.is_some() => {
                        self.cancel_operation();
                    }
                    (KeyCode::Esc, _)
                        if self
                            .notice
                            .as_ref()
                            .is_some_and(|(_, _, is_error)| *is_error) =>
                    {
                        self.notice = None;
                    }
                    (KeyCode::Esc, _) => return Ok(true),

                    // ── Modes ───────────────────────────────────────
                    (KeyCode::F(2), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.start_rename();
                    }
                    (KeyCode::Delete, KeyModifiers::SHIFT) if self.mode == AppMode::Normal => {
                        if !self.current().files.is_empty() {
                            self.pending_delete_targets = Some(self.selected_paths());
                            self.mode = AppMode::ConfirmDeletePermanent;
                        }
                    }
                    (KeyCode::Delete, _) if self.mode == AppMode::Normal => {
                        if !self.current().files.is_empty() {
                            self.pending_delete_targets = Some(self.selected_paths());
                            self.mode = AppMode::ConfirmDelete;
                        }
                    }
                    (KeyCode::F(7), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.mode = AppMode::NewFolder;
                        self.input_buffer.clear();
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                    }
                    (KeyCode::Char('N'), m)
                        if self.mode == AppMode::Normal
                            && m.contains(KeyModifiers::CONTROL)
                            && m.contains(KeyModifiers::SHIFT) =>
                    {
                        self.mode = AppMode::NewFolder;
                        self.input_buffer.clear();
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                    }
                    (KeyCode::Char('n'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.mode = AppMode::NewFile;
                        self.input_buffer.clear();
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                    }
                    (KeyCode::Enter, KeyModifiers::ALT) if self.mode == AppMode::Normal => {
                        if let Some(path) = self.selected_entry_path() {
                            self.open_properties(path);
                        }
                    }
                    (KeyCode::Char('f'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.begin_search();
                    }
                    (KeyCode::Char('/'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.begin_search();
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
                        self.input_cursor = self.input_grapheme_len();
                    }
                    (KeyCode::Char(c), m)
                        if self.mode == AppMode::Rename
                            && (m == KeyModifiers::NONE || m == KeyModifiers::SHIFT) =>
                    {
                        self.rename_input_replace_selection_with(&c.to_string());
                        return Ok(false);
                    }
                    (KeyCode::Char(c), _)
                        if self.mode == AppMode::NewFolder || self.mode == AppMode::NewFile =>
                    {
                        self.insert_input_char(c);
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
                    (KeyCode::Backspace, _)
                        if self.mode == AppMode::NewFolder || self.mode == AppMode::NewFile =>
                    {
                        if self.input_cursor > 0 {
                            let idx = self.byte_idx(self.input_cursor - 1);
                            let end = self.byte_idx(self.input_cursor);
                            self.input_buffer.replace_range(idx..end, "");
                            self.input_cursor -= 1;
                        }
                        return Ok(false);
                    }
                    (KeyCode::Delete, _)
                        if matches!(
                            self.mode,
                            AppMode::Rename | AppMode::NewFolder | AppMode::NewFile
                        ) =>
                    {
                        let len = self.input_grapheme_len();
                        if self.input_cursor < len {
                            let idx = self.byte_idx(self.input_cursor);
                            let end = self.byte_idx(self.input_cursor + 1);
                            self.input_buffer.replace_range(idx..end, "");
                        }
                        self.input_sel_start = self.input_cursor;
                        return Ok(false);
                    }
                    (KeyCode::Left, _)
                        if matches!(
                            self.mode,
                            AppMode::Rename | AppMode::NewFolder | AppMode::NewFile
                        ) =>
                    {
                        self.input_cursor = self.input_cursor.saturating_sub(1);
                        self.input_sel_start = self.input_cursor;
                        return Ok(false);
                    }
                    (KeyCode::Right, _)
                        if matches!(
                            self.mode,
                            AppMode::Rename | AppMode::NewFolder | AppMode::NewFile
                        ) =>
                    {
                        let len = self.input_grapheme_len();
                        self.input_cursor = (self.input_cursor + 1).min(len);
                        self.input_sel_start = self.input_cursor;
                        return Ok(false);
                    }
                    (KeyCode::Home, _)
                        if matches!(
                            self.mode,
                            AppMode::Rename | AppMode::NewFolder | AppMode::NewFile
                        ) =>
                    {
                        self.input_cursor = 0;
                        self.input_sel_start = 0;
                        return Ok(false);
                    }
                    (KeyCode::End, _)
                        if matches!(
                            self.mode,
                            AppMode::Rename | AppMode::NewFolder | AppMode::NewFile
                        ) =>
                    {
                        let len = self.input_grapheme_len();
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
                    (KeyCode::Enter, _) if self.mode == AppMode::NewFile => {
                        self.commit_new_file();
                        return Ok(false);
                    }
                    (KeyCode::Char('y'), _) | (KeyCode::Char('Y'), _)
                        if self.mode == AppMode::ConfirmDelete =>
                    {
                        self.do_delete(false);
                        return Ok(false);
                    }
                    (KeyCode::Char('y'), _) | (KeyCode::Char('Y'), _)
                        if self.mode == AppMode::ConfirmDeletePermanent =>
                    {
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

                    // Once the preview has been clicked, paged visual content
                    // owns these keys. Folder navigation keeps them otherwise.
                    (KeyCode::Left, _)
                        if self.mode == AppMode::Normal
                            && self.preview_focused
                            && self.is_paged_visual_selected() =>
                    {
                        self.step_preview_page(-1)
                    }
                    (KeyCode::Right, _)
                        if self.mode == AppMode::Normal
                            && self.preview_focused
                            && self.is_paged_visual_selected() =>
                    {
                        self.step_preview_page(1)
                    }
                    (KeyCode::PageUp, _)
                        if self.mode == AppMode::Normal
                            && self.preview_focused
                            && self.is_paged_visual_selected() =>
                    {
                        self.step_preview_page(-1)
                    }
                    (KeyCode::PageDown, _)
                        if self.mode == AppMode::Normal
                            && self.preview_focused
                            && self.is_paged_visual_selected() =>
                    {
                        self.step_preview_page(1)
                    }

                    // ── Navigation — only when Normal mode ──────────────────
                    (KeyCode::Down, _) if self.mode == AppMode::Normal => self.move_down(),
                    (KeyCode::Up, _) if self.mode == AppMode::Normal => self.move_up(),
                    (KeyCode::Left, _) if self.mode == AppMode::Normal => {
                        if self.layout_mode == LayoutMode::Explorer {
                            self.move_tile_horizontal(false);
                        } else {
                            self.go_left()?;
                            self.reset_image_state();
                        }
                    }
                    (KeyCode::Right, _) if self.mode == AppMode::Normal => {
                        if self.layout_mode == LayoutMode::Explorer {
                            self.move_tile_horizontal(true);
                        } else {
                            self.go_right()?;
                            self.reset_image_state();
                        }
                    }
                    (KeyCode::Enter, _) if self.mode == AppMode::Normal => {
                        self.open_selected();
                    }
                    (KeyCode::Char(' '), KeyModifiers::NONE)
                        if self.mode == AppMode::Normal
                            && self
                                .selected_entry_path()
                                .is_some_and(|path| crate::preview::is_media_path(&path)) =>
                    {
                        self.open_selected();
                    }

                    // ── Navigation — Vim keys ────────────────────────────────
                    (KeyCode::Char('j'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.move_down()
                    }
                    (KeyCode::Char('k'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.move_up()
                    }
                    (KeyCode::Char('h'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        if self.layout_mode == LayoutMode::Explorer {
                            self.move_tile_horizontal(false);
                        } else {
                            self.go_left()?;
                            self.reset_image_state();
                        }
                    }
                    (KeyCode::Char('l'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        if self.layout_mode == LayoutMode::Explorer {
                            self.move_tile_horizontal(true);
                        } else {
                            self.go_right()?;
                            self.reset_image_state();
                        }
                    }

                    // ── Jump top / bottom ────────────────────────────────────
                    (KeyCode::Char('g'), KeyModifiers::NONE) if self.mode == AppMode::Normal => {
                        self.jump_top()
                    }
                    (KeyCode::Char('G'), KeyModifiers::SHIFT) if self.mode == AppMode::Normal => {
                        self.jump_bottom()
                    }

                    // ── Page navigation ──────────────────────────────────────
                    (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL)
                        if self.mode == AppMode::Normal =>
                    {
                        self.page_down()
                    }
                    (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL)
                        if self.mode == AppMode::Normal =>
                    {
                        self.page_up()
                    }

                    // ── Preview scroll ───────────────────────────────────────
                    (KeyCode::Char(']'), KeyModifiers::NONE) => {
                        self.preview_scroll = self.preview_scroll.saturating_add(5);
                    }
                    (KeyCode::Char('['), KeyModifiers::NONE)
                    | (KeyCode::Char('?'), KeyModifiers::NONE)
                    | (KeyCode::Char('/'), KeyModifiers::SHIFT) => {
                        self.preview_scroll = self.preview_scroll.saturating_sub(5);
                    }

                    // ── Home directory ───────────────────────────────────────
                    (KeyCode::Char('~'), KeyModifiers::SHIFT) if self.mode == AppMode::Normal => {
                        self.go_home()?
                    }

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
            Event::Mouse(mut mouse) => {
                self.last_event_time = Instant::now();
                match mouse.kind {
                    MouseEventKind::Moved => {
                        // Mouse tracking can arrive much faster than a full TUI
                        // frame. Render only the newest position and retain the
                        // first meaningful event for the next iteration.
                        while event::poll(Duration::ZERO)? {
                            match event::read()? {
                                Event::Mouse(next) if next.kind == MouseEventKind::Moved => {
                                    mouse = next;
                                }
                                other => {
                                    self.pending_event = Some(other);
                                    break;
                                }
                            }
                        }
                        let geo = self.layout_geometry.lock().clone();
                        self.hovered_row = self
                            .hover_enabled
                            .then(|| {
                                geo.row_rects.iter().find_map(|(pane_idx, rows)| {
                                    rows.iter()
                                        .find(|(_, rect)| point_in(mouse.column, mouse.row, rect))
                                        .map(|(file_idx, _)| (*pane_idx, *file_idx))
                                })
                            })
                            .flatten();
                        if self.mode == AppMode::ContextMenu {
                            self.context_menu_hover = geo
                                .context_menu_item_rects
                                .iter()
                                .position(|(rect, _)| point_in(mouse.column, mouse.row, rect));
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        let mut steps = 1usize;
                        while steps < 32 && event::poll(Duration::ZERO)? {
                            match event::read()? {
                                Event::Mouse(next) if next.kind == MouseEventKind::ScrollDown => {
                                    steps += 1;
                                }
                                other => {
                                    self.pending_event = Some(other);
                                    break;
                                }
                            }
                        }
                        let geo = self.layout_geometry.lock().clone();
                        self.handle_scroll(&geo, mouse.column, mouse.row, true, steps);
                    }
                    MouseEventKind::ScrollUp => {
                        let mut steps = 1usize;
                        while steps < 32 && event::poll(Duration::ZERO)? {
                            match event::read()? {
                                Event::Mouse(next) if next.kind == MouseEventKind::ScrollUp => {
                                    steps += 1;
                                }
                                other => {
                                    self.pending_event = Some(other);
                                    break;
                                }
                            }
                        }
                        let geo = self.layout_geometry.lock().clone();
                        self.handle_scroll(&geo, mouse.column, mouse.row, false, steps);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let geo = self.layout_geometry.lock().clone();

                        if geo
                            .search_rect
                            .as_ref()
                            .is_some_and(|rect| point_in(mouse.column, mouse.row, rect))
                        {
                            self.begin_search();
                            return Ok(false);
                        }
                        if self.search_active {
                            self.accept_search();
                        }

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
                            if self.mode == AppMode::Rename {
                                return Ok(false);
                            }
                        } else if self.mode == AppMode::NewFolder {
                            self.commit_new_folder();
                            if self.mode == AppMode::NewFolder {
                                return Ok(false);
                            }
                        } else if self.mode == AppMode::NewFile {
                            self.commit_new_file();
                            if self.mode == AppMode::NewFile {
                                return Ok(false);
                            }
                        } else if self.mode == AppMode::ConfirmDelete
                            || self.mode == AppMode::ConfirmDeletePermanent
                            || self.mode == AppMode::Properties
                        {
                            if self.mode == AppMode::Properties {
                                self.close_properties();
                            } else {
                                self.mode = AppMode::Normal;
                            }
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
                        if handled {
                            return Ok(false);
                        }

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
                                self.preview_focused = true;
                                self.step_preview_page(-1);
                                return Ok(false);
                            }
                        }
                        if let Some(rect) = geo.slide_next_rect {
                            if point_in(mouse.column, mouse.row, &rect) {
                                self.preview_focused = true;
                                self.step_preview_page(1);
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
                                        self.navigate_to_select(parent, Some(path.clone()));
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
                        if handled {
                            return Ok(false);
                        }

                        // ── Breadcrumb: clickable path segments ──
                        for (rect, path) in geo.breadcrumb_segment_rects.iter() {
                            if point_in(mouse.column, mouse.row, rect) {
                                self.navigate_to(path);
                                handled = true;
                                break;
                            }
                        }
                        if handled {
                            return Ok(false);
                        }

                        if geo
                            .pane_sort_rect
                            .as_ref()
                            .is_some_and(|rect| point_in(mouse.column, mouse.row, rect))
                        {
                            self.select_sort_mode(self.sort_mode);
                            return Ok(false);
                        }

                        // ── Divider drag ──
                        for (idx, dr) in geo.divider_rects.iter().enumerate() {
                            if point_in(mouse.column, mouse.row, dr) {
                                self.dragging_divider = Some(idx);
                                handled = true;
                                break;
                            }
                        }
                        if handled {
                            return Ok(false);
                        }

                        // ── File/folder rows ──
                        let hit_row = self.handle_row_click(
                            &geo,
                            mouse.column,
                            mouse.row,
                            mouse.modifiers,
                            false,
                        );
                        if hit_row {
                            self.preview_focused = false;
                        } else if geo
                            .preview_rect
                            .is_some_and(|rect| point_in(mouse.column, mouse.row, &rect))
                        {
                            self.preview_focused = true;
                        }
                    }
                    MouseEventKind::Down(MouseButton::Right) => {
                        let geo = self.layout_geometry.lock().clone();
                        if self.mode == AppMode::ContextMenu {
                            self.mode = AppMode::Normal;
                        }
                        // Right-click selects the item under the cursor first, then opens
                        // the context menu at the mouse position.
                        let hit_row = self.handle_row_click(
                            &geo,
                            mouse.column,
                            mouse.row,
                            KeyModifiers::NONE,
                            true,
                        );
                        // Empty pane/preview space gets the background menu. It
                        // must never apply actions to a stale highlighted row.
                        let target = hit_row.then(|| self.selected_entry_path()).flatten();
                        self.open_context_menu(target);
                        self.pending_menu_pos = (mouse.column, mouse.row);
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if self.dragging_sidebar {
                            let term_w = crossterm::terminal::size().unwrap_or((100, 30)).0;
                            let maximum = term_w.saturating_sub(48).clamp(18, 36);
                            self.sidebar_width = mouse.column.clamp(18, maximum);
                        } else if let Some(idx) = self.dragging_divider {
                            let geo = self.layout_geometry.lock().clone();
                            let mut columns = geo.pane_rects.clone();
                            if let Some(preview) = geo.preview_rect {
                                columns.push(preview);
                            }
                            if let (Some(left), Some(right)) =
                                (columns.get(idx), columns.get(idx + 1))
                            {
                                let combined = left.width.saturating_add(right.width);
                                let minimum = 8u16.min(combined / 2);
                                if combined > minimum.saturating_mul(2) {
                                    let new_left = mouse
                                        .column
                                        .saturating_sub(left.x)
                                        .clamp(minimum, combined.saturating_sub(minimum));
                                    let n_cols = columns.len().min(self.column_ratios.len());
                                    let start_ratio_idx = self.column_ratios.len() - n_cols;
                                    let ratio_idx_left = start_ratio_idx + idx;
                                    let ratio_idx_right = ratio_idx_left + 1;
                                    if ratio_idx_right < self.column_ratios.len() {
                                        let pair_total = self.column_ratios[ratio_idx_left]
                                            + self.column_ratios[ratio_idx_right];
                                        let left_ratio =
                                            pair_total * new_left as f32 / combined as f32;
                                        self.column_ratios[ratio_idx_left] = left_ratio;
                                        self.column_ratios[ratio_idx_right] =
                                            pair_total - left_ratio;
                                    }
                                }
                            }
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        let changed_layout =
                            self.dragging_divider.is_some() || self.dragging_sidebar;
                        self.dragging_divider = None;
                        self.dragging_sidebar = false;
                        if changed_layout {
                            self.persist_user_settings();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_scroll(
        &mut self,
        geo: &LayoutGeometry,
        col: u16,
        row: u16,
        down: bool,
        steps: usize,
    ) {
        let rows = 3usize.saturating_mul(steps.max(1));
        if let Some(pr) = geo.preview_rect {
            if point_in(col, row, &pr) {
                let selected = self.selected_file();
                // Visual presentations and PDFs are paged content, not
                // scrollable text blocks. The wheel requests the adjacent
                // rendered page instead of moving a metadata header.
                if self.is_paged_visual_selected() {
                    self.preview_focused = true;
                    self.step_preview_page(if down {
                        steps as isize
                    } else {
                        -(steps as isize)
                    });
                } else if crate::preview::is_visual_preview(
                    selected.as_ref(),
                    self.effective_office_mode(),
                    self.effective_pdf_mode(),
                ) {
                    // Static visual previews have nothing textual to scroll.
                    self.preview_scroll = 0;
                } else if down {
                    self.preview_scroll = self.preview_scroll.saturating_add(rows);
                } else {
                    self.preview_scroll = self.preview_scroll.saturating_sub(rows);
                }
                return;
            }
        }
        for (idx, pr) in geo.pane_rects.iter().enumerate() {
            if point_in(col, row, pr) {
                if self.search_active
                    && !self.search_query.is_empty()
                    && idx + 1 == geo.pane_rects.len()
                {
                    self.select_next_search_match(down);
                    return;
                }
                let num = self.levels.len();
                let start = num.saturating_sub(4);
                let real_idx = geo
                    .pane_level_indices
                    .get(&idx)
                    .copied()
                    .unwrap_or(start + idx);
                if real_idx < self.levels.len() {
                    let visible = pr.height.saturating_sub(2) as usize;
                    let level = &mut self.levels[real_idx];
                    let max_scroll = level.files.len().saturating_sub(visible);
                    if down {
                        level.scroll = level.scroll.saturating_add(rows).min(max_scroll);
                    } else {
                        level.scroll = level.scroll.saturating_sub(rows);
                    }
                }
                return;
            }
        }
    }

    /// Shared row hit-testing for left-click selection and right-click.
    /// A primary click selects and a repeated primary click opens. Rename is
    /// deliberately reserved for F2 and the context menu so it can never steal
    /// a slightly delayed double-click.
    fn handle_row_click(
        &mut self,
        geo: &LayoutGeometry,
        col: u16,
        row: u16,
        mods: KeyModifiers,
        for_context_menu: bool,
    ) -> bool {
        for (pane_idx, rows) in geo.row_rects.iter() {
            for (file_idx, rect) in rows {
                if point_in(col, row, rect) {
                    let num = self.levels.len();
                    let start = num.saturating_sub(4);
                    let real_level_idx = geo
                        .pane_level_indices
                        .get(pane_idx)
                        .copied()
                        .unwrap_or(start + pane_idx);
                    if real_level_idx >= self.levels.len() {
                        return false;
                    }

                    self.current_level = real_level_idx;
                    self.levels.truncate(self.current_level + 1);

                    if *file_idx >= self.current().files.len() {
                        return false;
                    }

                    if for_context_menu {
                        self.last_click = None;
                        // Right-click: select without clobbering an existing multi-selection.
                        let cur = self.current_mut();
                        if !cur.marked.contains(&cur.files[*file_idx].path) {
                            cur.marked.clear();
                            cur.selected = *file_idx;
                            cur.select_anchor = *file_idx;
                        }
                        self.reset_image_state();
                        self.mode = AppMode::Normal;
                        return true;
                    }

                    if mods.contains(KeyModifiers::SHIFT) {
                        self.last_click = None;
                        self.mark_range(*file_idx);
                        self.reset_image_state();
                        return true;
                    }
                    if mods.contains(KeyModifiers::CONTROL) {
                        self.last_click = None;
                        self.toggle_mark(*file_idx);
                        self.reset_image_state();
                        return true;
                    }

                    let now = Instant::now();
                    let is_double_click = is_repeated_primary_click(
                        self.last_click,
                        now,
                        *pane_idx,
                        *file_idx,
                        primary_double_click_interval(),
                    );

                    self.clear_marks();
                    self.current_mut().selected = *file_idx;
                    self.current_mut().select_anchor = *file_idx;

                    if is_double_click {
                        let path = self.current().files[*file_idx].path.clone();
                        self.last_click = None;
                        self.open_path(path);
                    } else {
                        self.last_click = Some((now, *pane_idx, *file_idx));
                        self.reset_image_state();
                    }
                    return true;
                }
            }
        }
        // Only directory-pane background clicks clear marks. Preview, sidebar,
        // breadcrumb, and status clicks must not mutate directory selection.
        if geo.pane_rects.iter().any(|rect| point_in(col, row, rect)) {
            self.last_click = None;
            self.clear_marks();
        }
        false
    }

    fn keep_selection_visible(&mut self) {
        let num = self.levels.len();
        let start = num.saturating_sub(4);
        if self.current_level < start {
            return;
        }
        let pane_idx = self.current_level - start;
        let geometry = self.layout_geometry.lock().clone();
        let (visible, columns) = if self.layout_mode == LayoutMode::Explorer {
            (
                geometry.tile_visible_items.max(1),
                geometry.tile_columns.max(1),
            )
        } else {
            (
                geometry
                    .pane_rects
                    .get(pane_idx)
                    .map(|r| r.height.saturating_sub(2) as usize)
                    .unwrap_or(1)
                    .max(1),
                1,
            )
        };
        let cur = self.current_mut();
        if cur.selected < cur.scroll {
            cur.scroll = (cur.selected / columns) * columns;
        } else if cur.selected >= cur.scroll.saturating_add(visible) {
            let selected_row_start = (cur.selected / columns) * columns;
            cur.scroll = selected_row_start
                .saturating_add(columns)
                .saturating_sub(visible);
        }
        let max_scroll = cur.files.len().saturating_sub(visible);
        cur.scroll = cur.scroll.min(max_scroll).div_euclid(columns) * columns;
    }

    // ── Movement helpers ──────────────────────────────────────────────────────

    fn move_down(&mut self) {
        let step = if self.layout_mode == LayoutMode::Explorer {
            self.layout_geometry.lock().tile_columns.max(1)
        } else {
            1
        };
        let current = self.current_mut();
        if current.files.is_empty() {
            return;
        }
        if current.selected + step < current.files.len() {
            current.selected += step;
            self.keep_selection_visible();
            self.reset_image_state();
        } else if step > 1 && current.selected + 1 < current.files.len() {
            current.selected = current.files.len() - 1;
            self.keep_selection_visible();
            self.reset_image_state();
        }
    }

    fn move_up(&mut self) {
        let step = if self.layout_mode == LayoutMode::Explorer {
            self.layout_geometry.lock().tile_columns.max(1)
        } else {
            1
        };
        let current = self.current_mut();
        if current.selected > 0 {
            current.selected = current.selected.saturating_sub(step);
            self.keep_selection_visible();
            self.reset_image_state();
        }
    }

    fn move_tile_horizontal(&mut self, right: bool) {
        if self.layout_mode != LayoutMode::Explorer {
            return;
        }
        let columns = self.layout_geometry.lock().tile_columns.max(1);
        let current = self.current_mut();
        if right {
            if current.selected + 1 < current.files.len()
                && current.selected % columns + 1 < columns
            {
                current.selected += 1;
            } else {
                return;
            }
        } else if current.selected > 0 && !current.selected.is_multiple_of(columns) {
            current.selected -= 1;
        } else {
            return;
        }
        self.keep_selection_visible();
        self.reset_image_state();
    }

    fn go_right(&mut self) -> anyhow::Result<()> {
        let current = self.current();
        if current.files.is_empty() {
            return Ok(());
        }

        let selected_entry = &current.files[current.selected];

        if selected_entry.is_dir {
            self.request_navigation(
                selected_entry.path.clone(),
                NavigationAction::Push {
                    origin_level: self.current_level,
                },
            );
        }
        Ok(())
    }

    fn go_left(&mut self) -> anyhow::Result<()> {
        self.cancel_navigation();
        if self.current_level > 0 {
            self.current_level -= 1;
            // Truncate the trailing levels so the right-hand preview pane
            // immediately reflects the currently selected item in the new level.
            self.levels.truncate(self.current_level + 1);
        } else {
            // At the leftmost level, open the parent and keep the directory
            // we came from selected. The scan stays off the input thread.
            let child = self.current().path.clone();
            if let Some(parent) = child.parent() {
                let parent_path = parent.to_path_buf();
                self.request_navigation(
                    parent_path,
                    NavigationAction::Replace {
                        select_path: Some(child),
                    },
                );
            }
        }
        Ok(())
    }

    fn jump_top(&mut self) {
        let current = self.current_mut();
        if current.selected != 0 {
            current.selected = 0;
            current.scroll = 0;
            self.reset_image_state();
        }
    }

    fn jump_bottom(&mut self) {
        let current = self.current_mut();
        if !current.files.is_empty() {
            let last = current.files.len() - 1;
            if current.selected != last {
                current.selected = last;
                self.keep_selection_visible();
                self.reset_image_state();
            }
        }
    }

    pub fn open_selected(&mut self) {
        if let Some(path) = self.selected_entry_path() {
            self.open_path(path);
        }
    }

    fn focus_path(&mut self, path: &std::path::Path) {
        if let Some(index) = self
            .current()
            .files
            .iter()
            .position(|entry| entry.path == path)
        {
            let current = self.current_mut();
            current.selected = index;
            current.select_anchor = index;
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        if path.is_dir() {
            if self.current().files.iter().any(|entry| entry.path == path) {
                self.focus_path(&path);
                let _ = self.go_right();
            } else {
                self.navigate_to(&path);
            }
        } else {
            #[cfg(windows)]
            {
                let path_str = path.to_string_lossy().to_string();
                // /C start "" "path"
                let _ = std::process::Command::new("cmd")
                    .args(["/C", "start", "", &path_str])
                    .spawn();
            }
            #[cfg(not(windows))]
            {
                let path_str = path.to_string_lossy().to_string();
                let _ = std::process::Command::new("xdg-open")
                    .arg(&path_str)
                    .spawn();
            }
        }
    }

    fn page_down(&mut self) {
        let page = if self.layout_mode == LayoutMode::Explorer {
            self.layout_geometry.lock().tile_visible_items.max(1)
        } else {
            10
        };
        let current = self.current_mut();
        if current.files.is_empty() {
            return;
        }
        let target = (current.selected + page).min(current.files.len() - 1);
        if current.selected != target {
            current.selected = target;
            self.keep_selection_visible();
            self.reset_image_state();
        }
    }

    fn page_up(&mut self) {
        let page = if self.layout_mode == LayoutMode::Explorer {
            self.layout_geometry.lock().tile_visible_items.max(1)
        } else {
            10
        };
        let current = self.current_mut();
        let target = current.selected.saturating_sub(page);
        if current.selected != target {
            current.selected = target;
            self.keep_selection_visible();
            self.reset_image_state();
        }
    }

    fn go_home(&mut self) -> anyhow::Result<()> {
        if let Some(home) = dirs::home_dir() {
            self.navigate_to(&home);
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn go_root(&mut self) -> anyhow::Result<()> {
        let root = PathBuf::from("/");
        self.navigate_to(&root);
        Ok(())
    }

    /// Windows: enumerate available drive letters and present them as a
    /// synthetic top-level directory listing.
    #[cfg(windows)]
    fn go_drives(&mut self) -> anyhow::Result<()> {
        self.cancel_navigation();
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
        if cur.files.is_empty() {
            return None;
        }
        let p = &cur.files[cur.selected];
        if !p.is_dir {
            Some(p.path.clone())
        } else {
            None
        }
    }

    /// Path of the currently highlighted entry, whether it's a file or a directory.
    /// Use this for context-menu / rename / delete / copy-path targets;
    /// use `selected_file` where only *files* are meaningful (e.g. image preview).
    pub fn selected_entry_path(&self) -> Option<PathBuf> {
        let cur = self.current();
        if cur.files.is_empty() {
            return None;
        }
        Some(cur.files[cur.selected].path.clone())
    }

    pub fn begin_search(&mut self) {
        if !self.search_active {
            self.search_original_selection = Some(self.current().selected);
            self.search_query.clear();
        }
        self.search_active = true;
    }

    fn accept_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.search_original_selection = None;
    }

    fn cancel_search(&mut self) {
        if let Some(index) = self.search_original_selection.take() {
            let last = self.current().files.len().saturating_sub(1);
            self.current_mut().selected = index.min(last);
            self.keep_selection_visible();
        }
        self.search_active = false;
        self.search_query.clear();
        self.preview_scroll = 0;
    }

    pub fn search_matches(&self) -> Vec<usize> {
        if self.search_query.is_empty() {
            return Vec::new();
        }
        self.current()
            .files
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                filename_matches(entry, &self.search_query).then_some(index)
            })
            .collect()
    }

    pub fn search_match_position(&self) -> (usize, usize) {
        let matches = self.search_matches();
        let position = matches
            .iter()
            .position(|index| *index == self.current().selected)
            .map(|index| index + 1)
            .unwrap_or(0);
        (position, matches.len())
    }

    fn select_first_search_match(&mut self) {
        let Some(index) = self.search_matches().first().copied() else {
            if self.search_query.is_empty() {
                if let Some(original) = self.search_original_selection {
                    self.current_mut().selected =
                        original.min(self.current().files.len().saturating_sub(1));
                    self.keep_selection_visible();
                }
            }
            return;
        };
        self.current_mut().selected = index;
        self.keep_selection_visible();
        self.preview_scroll = 0;
        self.last_selection_changed_at = Instant::now();
        self.invalidate_preview_pipeline(true);
    }

    fn select_next_search_match(&mut self, forward: bool) {
        let matches = self.search_matches();
        if matches.is_empty() {
            return;
        }
        let selected = self.current().selected;
        let current_position = matches.iter().position(|index| *index == selected);
        let next_position = match (current_position, forward) {
            (Some(position), true) => (position + 1) % matches.len(),
            (Some(0), false) | (None, false) => matches.len() - 1,
            (Some(position), false) => position - 1,
            (None, true) => 0,
        };
        self.current_mut().selected = matches[next_position];
        self.keep_selection_visible();
        self.preview_scroll = 0;
        self.last_selection_changed_at = Instant::now();
        self.invalidate_preview_pipeline(true);
    }

    /// True if the selected file is an image
    fn is_image_selected(&self) -> bool {
        self.selected_file().is_some_and(|p| {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" | "tiff" | "tif" | "ico"
            )
        })
    }

    /// Reset image transform state (called when navigating away)
    pub fn reset_image_state(&mut self) {
        self.last_selection_changed_at = Instant::now();
        if self.edit_preview_mode {
            self.edit_preview_mode = false;
            if self.edit_dirty {
                self.set_notice("Left edit mode; unsaved changes are kept", false);
            }
        }
        self.image_zoom = 1.0;
        self.image_rotation = 0;
        self.image_flip_h = false;
        self.mode = AppMode::Normal;
        self.preview_scroll = 0;
        self.preview_focused = false;
        self.invalidate_preview_pipeline(true);
        self.search_active = false;
        self.search_query.clear();
        self.search_original_selection = None;
    }

    pub fn preview_debounce_active(&self) -> bool {
        self.last_selection_changed_at.elapsed() < preview_settle_delay(self.preview_mode)
    }

    pub fn event_revision(&self) -> u64 {
        self.event_revision
    }

    // ── Text-editing helpers for Rename / New Folder input ───────────────────

    fn byte_idx(&self, grapheme_idx: usize) -> usize {
        self.input_buffer
            .grapheme_indices(true)
            .nth(grapheme_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.input_buffer.len())
    }

    fn input_grapheme_len(&self) -> usize {
        self.input_buffer.graphemes(true).count()
    }

    fn insert_input_char(&mut self, value: char) {
        let byte = self.byte_idx(self.input_cursor);
        self.input_buffer.insert(byte, value);
        let after = byte + value.len_utf8();
        self.input_cursor = self.input_buffer[..after].graphemes(true).count();
        self.input_sel_start = self.input_cursor;
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
        let new_pos = self.input_buffer[..bl + s.len()].graphemes(true).count();
        self.input_cursor = new_pos;
        self.input_sel_start = new_pos;
    }

    /// Windows filenames may not contain: \ / : * ? " < > | and may not be "." or "..".
    fn validate_filename(name: &str) -> Result<(), String> {
        if name.is_empty() || name.chars().all(char::is_whitespace) {
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
        // Windows reserves these DOS device names even when an extension is
        // present (for example, "CON.txt").
        let stem = name
            .split('.')
            .next()
            .unwrap_or(name)
            .trim_end()
            .to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'));
        if reserved {
            return Err("That name is reserved by Windows".into());
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
        if cur.files.is_empty() {
            return Vec::new();
        }
        vec![cur.files[cur.selected].path.clone()]
    }

    pub fn delete_target_summary(&self) -> (usize, Vec<String>) {
        let paths = self
            .pending_delete_targets
            .as_ref()
            .cloned()
            .unwrap_or_else(|| self.selected_paths());
        let count = paths.len();
        let mut examples = paths
            .iter()
            .take(3)
            .map(|path| {
                let kind = if path.is_dir() { "Folder" } else { "File" };
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                format!("[{kind}] {name}")
            })
            .collect::<Vec<_>>();
        if count > examples.len() {
            examples.push(format!("… and {} more", count - examples.len()));
        }
        if examples.is_empty() {
            examples.push(
                paths
                    .first()
                    .and_then(|path| path.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if count == 1 && examples[0].is_empty() {
            examples[0] = paths[0]
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
        }
        (count, examples)
    }

    pub fn toggle_mark(&mut self, idx: usize) {
        let cur = self.current_mut();
        if idx >= cur.files.len() {
            return;
        }
        let path = cur.files[idx].path.clone();
        if !cur.marked.remove(&path) {
            cur.marked.insert(path);
        }
        cur.selected = idx;
        cur.select_anchor = idx;
    }

    pub fn mark_range(&mut self, to: usize) {
        let cur = self.current_mut();
        if cur.files.is_empty() {
            return;
        }
        let to = to.min(cur.files.len() - 1);
        let (lo, hi) = if cur.select_anchor <= to {
            (cur.select_anchor, to)
        } else {
            (to, cur.select_anchor)
        };
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
        if cur.files.is_empty() {
            return;
        }
        let name = cur.files[cur.selected]
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let is_dir = cur.files[cur.selected].is_dir;
        self.input_buffer = name.clone();
        // Select the basename but not the extension (files only).
        let sel_end = if is_dir {
            name.graphemes(true).count()
        } else {
            match name.rfind('.') {
                Some(byte_pos) if byte_pos > 0 => name[..byte_pos].graphemes(true).count(),
                _ => name.graphemes(true).count(),
            }
        };
        self.input_sel_start = 0;
        self.input_cursor = sel_end;
        self.mode = AppMode::Rename;
    }

    fn commit_rename(&mut self) {
        let new_name = self.input_buffer.clone();
        if let Err(e) = Self::validate_filename(&new_name) {
            self.set_notice(e, true);
            return;
        }
        let cur = self.current();
        if cur.files.is_empty() {
            self.mode = AppMode::Normal;
            return;
        }
        let old_path = cur.files[cur.selected].path.clone();
        let mut new_path = old_path.clone();
        new_path.set_file_name(&new_name);

        if new_path == old_path {
            self.mode = AppMode::Normal;
            return;
        }
        if new_path.exists() {
            self.set_notice(format!("\"{}\" already exists", new_name), true);
            return;
        }
        match fs::rename(&old_path, &new_path) {
            Ok(_) => {
                if let Some(parent) = old_path.parent() {
                    DIR_CACHE.lock().remove(parent);
                    crate::preview::invalidate_cached_dir(Some(parent));
                    if let Ok(files) = self.load_sorted_dir(parent) {
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
            Err(e) => {
                self.set_notice(format!("Rename failed: {}", e), true);
                return;
            }
        }
        self.mode = AppMode::Normal;
    }

    // ── New folder ─────────────────────────────────────────────────────────

    fn commit_new_folder(&mut self) {
        let name = self.input_buffer.clone();
        if let Err(e) = Self::validate_filename(&name) {
            self.set_notice(e, true);
            return;
        }
        let parent = self.current().path.clone();
        let new_dir = parent.join(&name);
        if new_dir.exists() {
            self.set_notice(format!("\"{}\" already exists", name), true);
            return;
        }
        match fs::create_dir(&new_dir) {
            Ok(_) => {
                DIR_CACHE.lock().remove(&parent);
                crate::preview::invalidate_cached_dir(Some(&parent));
                if let Ok(files) = self.load_sorted_dir(&parent) {
                    let cur = self.current_mut();
                    cur.files = files;
                    if let Some(pos) = cur.files.iter().position(|f| f.path == new_dir) {
                        cur.selected = pos;
                    }
                }
                self.set_notice(format!("Created folder \"{}\"", name), false);
            }
            Err(e) => {
                self.set_notice(format!("Couldn't create folder: {}", e), true);
                return;
            }
        }
        self.mode = AppMode::Normal;
    }

    fn commit_new_file(&mut self) {
        let name = self.input_buffer.clone();
        if let Err(e) = Self::validate_filename(&name) {
            self.set_notice(e, true);
            return;
        }
        let parent = self.current().path.clone();
        let new_file = parent.join(&name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_file)
        {
            Ok(_) => {
                DIR_CACHE.lock().remove(&parent);
                crate::preview::invalidate_cached_dir(Some(&parent));
                if let Ok(files) = self.load_sorted_dir(&parent) {
                    let cur = self.current_mut();
                    cur.files = files;
                    if let Some(pos) = cur.files.iter().position(|entry| entry.path == new_file) {
                        cur.selected = pos;
                        cur.select_anchor = pos;
                    }
                }
                self.set_notice(format!("Created file \"{}\"", name), false);
                self.mode = AppMode::Normal;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.set_notice(format!("\"{}\" already exists", name), true);
            }
            Err(e) => {
                self.set_notice(format!("Couldn't create file: {}", e), true);
            }
        }
    }

    // ── Delete ─────────────────────────────────────────────────────────────

    fn do_delete(&mut self, permanent: bool) {
        let paths = self
            .pending_delete_targets
            .take()
            .unwrap_or_else(|| self.selected_paths());
        if paths.is_empty() {
            self.mode = AppMode::Normal;
            return;
        }
        if self.operation_task.is_some() {
            self.set_notice("Another file operation is still running", true);
            self.mode = AppMode::Normal;
            return;
        }
        let total = paths.len();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.operation_status = Some(OperationStatus {
            label: if permanent {
                "Deleting permanently".to_string()
            } else {
                "Moving to Recycle Bin".to_string()
            },
            done: 0,
            total,
        });
        self.operation_task = Some(OperationTask { receiver, cancel });
        std::thread::spawn(move || {
            let mut deleted = 0usize;
            let mut errors = Vec::new();
            let label = if permanent {
                "Deleting permanently"
            } else {
                "Moving to Recycle Bin"
            };
            for (index, path) in paths.iter().enumerate() {
                if worker_cancel.load(Ordering::Acquire) {
                    break;
                }
                #[cfg(windows)]
                let result = if permanent {
                    remove_path(path)
                } else {
                    move_to_recycle_bin(std::slice::from_ref(path))
                };
                #[cfg(not(windows))]
                let result = remove_path(path);
                match result {
                    Ok(()) => deleted += 1,
                    Err(error) => errors.push(format!("{}: {}", path.display(), error)),
                }
                let _ = sender.send(OperationEvent::Progress {
                    label: label.to_string(),
                    done: index + 1,
                    total,
                });
            }
            let snapshots = snapshots_for_parents(&paths);
            let failed = errors.len();
            let cancelled = worker_cancel.load(Ordering::Acquire);
            let destination = if !permanent && cfg!(windows) {
                "to Recycle Bin"
            } else {
                "permanently"
            };
            let message = if cancelled {
                format!("Cancelled after deleting {deleted} of {total} item(s)")
            } else if failed == 0 {
                format!("Deleted {} item(s) {}", deleted, destination)
            } else {
                format!(
                    "Deleted {} item(s); {} failed: {}",
                    deleted,
                    failed,
                    errors.first().cloned().unwrap_or_default()
                )
            };
            let _ = sender.send(OperationEvent::Complete {
                message,
                is_error: failed > 0,
                clear_clipboard: false,
                snapshots,
            });
        });
        self.mode = AppMode::Normal;
    }

    // ── Paste ──────────────────────────────────────────────────────────────

    pub fn do_paste(&mut self) {
        let Some((srcs, is_cut)) = self.clipboard.clone() else {
            return;
        };
        if self.operation_task.is_some() {
            self.set_notice("Another file operation is still running", true);
            return;
        }
        let dest_dir = self.current().path.clone();
        let total = srcs.len();
        let label = if is_cut { "Moving" } else { "Copying" }.to_string();
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.operation_status = Some(OperationStatus {
            label: label.clone(),
            done: 0,
            total,
        });
        self.operation_task = Some(OperationTask { receiver, cancel });
        std::thread::spawn(move || {
            let mut ok = 0usize;
            let mut errors = Vec::new();
            for (index, src_path) in srcs.iter().enumerate() {
                if worker_cancel.load(Ordering::Acquire) {
                    break;
                }
                match paste_one(src_path, &dest_dir, is_cut, &worker_cancel) {
                    Ok(PasteOutcome::Completed) => ok += 1,
                    Ok(PasteOutcome::Skipped) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => break,
                    Err(error) => errors.push(format!("{}: {}", src_path.display(), error)),
                }
                let _ = sender.send(OperationEvent::Progress {
                    label: label.clone(),
                    done: index + 1,
                    total,
                });
            }
            let mut refresh = srcs.clone();
            refresh.push(dest_dir.clone());
            let snapshots = snapshots_for_parents_and_directories(&refresh, &dest_dir);
            let failed = errors.len();
            let cancelled = worker_cancel.load(Ordering::Acquire);
            let message = if cancelled {
                format!("Cancelled after {} of {} item(s)", ok, total)
            } else if failed == 0 {
                format!("{} {} item(s)", if is_cut { "Moved" } else { "Copied" }, ok)
            } else {
                format!(
                    "{} {} item(s); {} failed: {}",
                    if is_cut { "Moved" } else { "Copied" },
                    ok,
                    failed,
                    errors.first().cloned().unwrap_or_default()
                )
            };
            let _ = sender.send(OperationEvent::Complete {
                message,
                is_error: failed > 0,
                clear_clipboard: is_cut && failed == 0 && !cancelled,
                snapshots,
            });
        });
    }

    fn cancel_operation(&mut self) {
        if let Some(task) = self.operation_task.as_ref() {
            task.cancel.store(true, Ordering::Release);
            if let Some(status) = self.operation_status.as_mut() {
                status.label = "Cancelling".to_string();
            }
        }
    }

    // ── Context menu ──────────────────────────────────────────────────────

    pub fn open_context_menu(&mut self, path: Option<PathBuf>) {
        self.context_menu_hover = None;
        self.context_menu_target = path.clone();
        self.context_menu_targets = path
            .as_ref()
            .map(|target| {
                if self.current().marked.contains(target) {
                    self.selected_paths()
                } else {
                    vec![target.clone()]
                }
            })
            .unwrap_or_default();
        let has_selection = path.is_some();
        let has_clipboard = self.clipboard.is_some();
        let mut items = Vec::new();
        if has_selection {
            items.push(ContextAction::Open);
            if path.as_deref().is_some_and(crate::preview::is_media_path) {
                items.push(ContextAction::Play);
            }
            items.push(ContextAction::OpenWith);
            items.push(ContextAction::Cut);
            items.push(ContextAction::Copy);
        }
        if has_clipboard {
            items.push(ContextAction::Paste);
        }
        if has_selection {
            items.push(ContextAction::Rename);
            items.push(ContextAction::Delete);
            items.push(ContextAction::CopyPath);
        }
        items.push(ContextAction::OpenInTerminal);
        items.push(ContextAction::NewFile);
        items.push(ContextAction::NewFolder);
        if has_selection {
            items.push(ContextAction::Properties);
        }
        self.context_menu_items = items;
        self.mode = AppMode::ContextMenu;
    }

    pub fn apply_context_action(&mut self, action: ContextAction) {
        let target = self.context_menu_target.clone();
        let targets = self.context_menu_targets.clone();
        self.mode = AppMode::Normal;
        match action {
            ContextAction::Open => {
                if let Some(path) = targets.first() {
                    self.open_path(path.clone());
                }
            }
            ContextAction::Play => {
                if let Some(path) = targets.first() {
                    self.open_path(path.clone());
                }
            }
            ContextAction::OpenWith => self.do_open_with(target),
            ContextAction::Cut => {
                if !targets.is_empty() {
                    self.clipboard = Some((targets, true));
                }
            }
            ContextAction::Copy => {
                if !targets.is_empty() {
                    self.clipboard = Some((targets, false));
                }
            }
            ContextAction::Paste => self.do_paste(),
            ContextAction::Rename => {
                if let Some(path) = targets.first() {
                    self.focus_path(path);
                }
                self.start_rename();
            }
            ContextAction::Delete => {
                self.pending_delete_targets = Some(targets);
                self.mode = AppMode::ConfirmDelete;
            }
            ContextAction::Properties => {
                if let Some(path) = target.or_else(|| self.selected_entry_path()) {
                    self.open_properties(path);
                }
            }
            ContextAction::CopyPath => self.do_copy_path(target),
            ContextAction::OpenInTerminal => self.do_open_in_terminal(),
            ContextAction::NewFolder => {
                self.mode = AppMode::NewFolder;
                self.input_buffer.clear();
                self.input_cursor = 0;
                self.input_sel_start = 0;
            }
            ContextAction::NewFile => {
                self.mode = AppMode::NewFile;
                self.input_buffer.clear();
                self.input_cursor = 0;
                self.input_sel_start = 0;
            }
        }
    }

    fn do_copy_path(&mut self, target: Option<PathBuf>) {
        let Some(path) = target
            .or_else(|| self.selected_entry_path())
            .or_else(|| Some(self.current().path.clone()))
        else {
            return;
        };
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
            if std::process::Command::new("wt")
                .arg("-d")
                .arg(&dir_str)
                .spawn()
                .is_err()
            {
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
        let Some(path) = target.or_else(|| self.selected_entry_path()) else {
            return;
        };
        #[cfg(windows)]
        {
            let path_str = path.to_string_lossy().to_string();
            let _ = std::process::Command::new("rundll32")
                .arg("shell32.dll,OpenAs_RunDLL")
                .arg(&path_str)
                .spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = path;
        }
    }

    fn persist_user_settings(&self) {
        let _ = crate::settings::save(&crate::settings::UserSettings {
            explorer_view: self.layout_mode == LayoutMode::Explorer,
            theme_light: self.theme_mode == ThemeMode::Light,
            office_full: self.office_mode == OfficeRenderMode::Full,
            pdf_visual: self.pdf_mode == PdfRenderMode::Visual,
            dir_preview_clickable: self.dir_preview_clickable,
            sort_mode: match self.sort_mode {
                SortMode::Name => "name",
                SortMode::Modified => "modified",
                SortMode::Size => "size",
            }
            .to_string(),
            sort_descending: self.sort_descending,
            show_file_details: self.show_file_details,
            rounded_selection: self.rounded_selection,
            hover_enabled: self.hover_enabled,
            font_face: self.font_face.clone(),
            font_size: self.font_size,
            font_weight: self.font_weight,
            preview_mode: match self.preview_mode {
                PreviewMode::Normal => "normal",
                PreviewMode::Full => "full",
                PreviewMode::Blitz => "blitz",
            }
            .to_string(),
            sidebar_width: self.sidebar_width,
            column_ratios: self.column_ratios.clone(),
            last_location: Some(self.current().path.to_string_lossy().into_owned()),
        });
    }

    pub fn toggle_setting(&mut self, action: ToggleAction) {
        match action {
            ToggleAction::Theme => {
                self.theme_mode = match self.theme_mode {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                };
                self.set_notice(
                    format!(
                        "Theme: {}",
                        if self.theme_mode == ThemeMode::Dark {
                            "Dark Mode"
                        } else {
                            "Light Mode"
                        }
                    ),
                    false,
                );
                self.persist_user_settings();
            }
            ToggleAction::LayoutMode => {
                self.layout_mode = match self.layout_mode {
                    LayoutMode::Miller => LayoutMode::Explorer,
                    LayoutMode::Explorer => LayoutMode::Miller,
                };
                self.hovered_row = None;
                self.persist_user_settings();
                self.set_notice(
                    format!(
                        "File view: {}",
                        if self.layout_mode == LayoutMode::Explorer {
                            "Windows tiles + details preview"
                        } else {
                            "Column view"
                        },
                    ),
                    false,
                );
            }
            ToggleAction::PreviewNormal
            | ToggleAction::PreviewFull
            | ToggleAction::PreviewBlitz => {
                self.preview_mode = match action {
                    ToggleAction::PreviewNormal => PreviewMode::Normal,
                    ToggleAction::PreviewFull => PreviewMode::Full,
                    ToggleAction::PreviewBlitz => PreviewMode::Blitz,
                    _ => unreachable!(),
                };
                self.last_selection_changed_at = Instant::now();
                self.invalidate_preview_pipeline(true);
                self.persist_user_settings();
                self.set_notice(
                    format!(
                        "Preview mode: {}",
                        match self.preview_mode {
                            PreviewMode::Normal => "Normal — responsive staged previews",
                            PreviewMode::Full => "Full — complete page and slide rendering",
                            PreviewMode::Blitz => "Blitz — cached and lightweight previews",
                        }
                    ),
                    false,
                );
            }
            ToggleAction::EditMode => {
                if self.edit_preview_mode {
                    self.edit_preview_mode = false;
                    self.set_notice(
                        if self.edit_dirty {
                            "Preview Edit Mode: OFF (unsaved changes kept)"
                        } else {
                            "Preview Edit Mode: OFF"
                        },
                        false,
                    );
                } else {
                    match self.sync_edit_buffer_with_selected() {
                        Ok(()) => {
                            self.edit_preview_mode = true;
                            self.set_notice("Preview Edit Mode: ON — Esc exits", false);
                        }
                        Err(e) => {
                            self.edit_preview_mode = false;
                            self.set_notice(e, true);
                        }
                    }
                }
            }
            ToggleAction::DirPreviewClick => {
                self.dir_preview_clickable = !self.dir_preview_clickable;
                self.set_notice(
                    format!(
                        "Directory Preview Clickable: {}",
                        if self.dir_preview_clickable {
                            "ON"
                        } else {
                            "OFF"
                        }
                    ),
                    false,
                );
                self.persist_user_settings();
            }
            ToggleAction::SortName => self.select_sort_mode(SortMode::Name),
            ToggleAction::SortModified => self.select_sort_mode(SortMode::Modified),
            ToggleAction::SortSize => self.select_sort_mode(SortMode::Size),
            ToggleAction::SortOrder => {
                self.sort_descending = !self.sort_descending;
                self.resort_levels();
                self.set_notice(
                    format!(
                        "Sort order: {}",
                        if self.sort_descending {
                            "Descending"
                        } else {
                            "Ascending"
                        }
                    ),
                    false,
                );
                self.persist_user_settings();
            }
            ToggleAction::Details => {
                self.show_file_details = !self.show_file_details;
                self.set_notice(
                    format!(
                        "File size and timestamp: {}",
                        if self.show_file_details {
                            "Shown"
                        } else {
                            "Hidden"
                        }
                    ),
                    false,
                );
                self.persist_user_settings();
            }
            ToggleAction::SelectionStyle => {
                self.rounded_selection = !self.rounded_selection;
                self.set_notice(
                    format!(
                        "Global control shape: {}",
                        if self.rounded_selection {
                            "Pill"
                        } else {
                            "Flat"
                        }
                    ),
                    false,
                );
                self.persist_user_settings();
            }
            ToggleAction::FontFamily => {
                const FACES: [&str; 4] =
                    ["Cascadia Code", "Nirmala UI", "Consolas", "Lucida Console"];
                let current = FACES
                    .iter()
                    .position(|value| *value == self.font_face)
                    .unwrap_or(0);
                self.font_face = FACES[(current + 1) % FACES.len()].to_string();
                self.persist_user_settings();
                self.set_notice(
                    format!("Terminal font: {} (applies on next launch)", self.font_face),
                    false,
                );
            }
            ToggleAction::FontSize => {
                const SIZES: [u16; 6] = [8, 9, 10, 11, 12, 14];
                let current = SIZES
                    .iter()
                    .position(|value| *value == self.font_size)
                    .unwrap_or(0);
                self.font_size = SIZES[(current + 1) % SIZES.len()];
                self.persist_user_settings();
                self.set_notice(
                    format!(
                        "Terminal font size: {} pt (applies fully on next launch)",
                        self.font_size
                    ),
                    false,
                );
            }
            ToggleAction::FontWeight => {
                const WEIGHTS: [u16; 6] = [300, 400, 500, 600, 700, 800];
                let current = WEIGHTS
                    .iter()
                    .position(|value| *value == self.font_weight)
                    .unwrap_or(0);
                self.font_weight = WEIGHTS[(current + 1) % WEIGHTS.len()];
                self.persist_user_settings();
                self.set_notice(
                    format!(
                        "Terminal font weight: {}{}",
                        self.font_weight,
                        if self.font_weight >= 600 {
                            " (bold now; exact weight on next launch)"
                        } else {
                            " (exact weight on next launch)"
                        },
                    ),
                    false,
                );
            }
            ToggleAction::Hover => {
                self.hover_enabled = !self.hover_enabled;
                if !self.hover_enabled {
                    self.hovered_row = None;
                }
                self.set_notice(
                    format!(
                        "Pointer hover highlighting: {}",
                        if self.hover_enabled { "ON" } else { "OFF" }
                    ),
                    false,
                );
                self.persist_user_settings();
            }
        }
    }

    fn select_sort_mode(&mut self, mode: SortMode) {
        if self.sort_mode == mode {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_mode = mode;
            self.sort_descending = false;
        }
        self.resort_levels();
        self.persist_user_settings();
        self.set_notice(
            format!(
                "Sorted by {} ({})",
                self.sort_mode.label(),
                if self.sort_descending {
                    "descending"
                } else {
                    "ascending"
                },
            ),
            false,
        );
    }

    pub fn sync_edit_buffer_with_selected(&mut self) -> Result<(), String> {
        let p = self
            .selected_file()
            .ok_or_else(|| "Select a text file before enabling Edit".to_string())?;
        if !p.is_file() {
            return Err("Only files can be edited".to_string());
        }

        if self.edit_dirty && self.edit_path.as_ref() == Some(&p) {
            // Resume the in-memory draft rather than silently replacing it.
            return Ok(());
        }
        if self.edit_dirty && self.edit_path.as_ref() != Some(&p) {
            let old_name = self
                .edit_path
                .as_ref()
                .and_then(|path| path.file_name())
                .unwrap_or_default()
                .to_string_lossy();
            return Err(format!(
                "Unsaved changes for {} are kept; reselect it to resume or save",
                old_name
            ));
        }

        let content =
            fs::read_to_string(&p).map_err(|e| format!("Can't edit {}: {}", p.display(), e))?;
        let (lines, line_ending) = split_edit_text(&content);
        self.edit_buffer = lines;
        self.edit_line_ending = line_ending.to_string();
        self.edit_cursor_row = 0;
        self.edit_cursor_col = 0;
        self.preview_scroll = 0;
        self.edit_dirty = false;
        self.edit_path = Some(p);
        Ok(())
    }

    pub fn save_edited_preview(&mut self) {
        if let Some(path) = &self.edit_path {
            let text = self.edit_buffer.join(&self.edit_line_ending);
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
        if !self.edit_preview_mode {
            return false;
        }

        if code == KeyCode::Esc {
            self.edit_preview_mode = false;
            self.set_notice(
                if self.edit_dirty {
                    "Left edit mode; unsaved changes are kept"
                } else {
                    "Left edit mode"
                },
                false,
            );
            return true;
        }

        if self.edit_buffer.is_empty() {
            return true;
        }

        if modifiers.contains(KeyModifiers::CONTROL)
            && (code == KeyCode::Char('s') || code == KeyCode::Char('S'))
        {
            self.save_edited_preview();
            return true;
        }

        match code {
            KeyCode::Char(c)
                if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT =>
            {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let line = &mut self.edit_buffer[self.edit_cursor_row];
                    let idx = grapheme_byte_index(line, self.edit_cursor_col);
                    line.insert(idx, c);
                    self.edit_cursor_col = grapheme_count(&line[..idx + c.len_utf8()]);
                    self.edit_dirty = true;
                    return true;
                }
            }
            KeyCode::Enter => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let line = self.edit_buffer[self.edit_cursor_row].clone();
                    let idx = grapheme_byte_index(&line, self.edit_cursor_col);
                    let (head, tail) = line.split_at(idx);
                    self.edit_buffer[self.edit_cursor_row] = head.to_string();
                    self.edit_buffer
                        .insert(self.edit_cursor_row + 1, tail.to_string());
                    self.edit_cursor_row += 1;
                    self.edit_cursor_col = 0;
                    self.edit_dirty = true;
                    return true;
                }
            }
            KeyCode::Backspace => {
                if self.edit_cursor_col > 0 {
                    let line = &mut self.edit_buffer[self.edit_cursor_row];
                    let idx = grapheme_byte_index(line, self.edit_cursor_col - 1);
                    let end = grapheme_byte_index(line, self.edit_cursor_col);
                    line.replace_range(idx..end, "");
                    self.edit_cursor_col -= 1;
                    self.edit_dirty = true;
                    return true;
                } else if self.edit_cursor_row > 0 {
                    let curr = self.edit_buffer.remove(self.edit_cursor_row);
                    self.edit_cursor_row -= 1;
                    let prev = &mut self.edit_buffer[self.edit_cursor_row];
                    self.edit_cursor_col = grapheme_count(prev);
                    prev.push_str(&curr);
                    self.edit_dirty = true;
                    return true;
                }
            }
            KeyCode::Delete => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let len = grapheme_count(&self.edit_buffer[self.edit_cursor_row]);
                    if self.edit_cursor_col < len {
                        let line = &mut self.edit_buffer[self.edit_cursor_row];
                        let idx = grapheme_byte_index(line, self.edit_cursor_col);
                        let end = grapheme_byte_index(line, self.edit_cursor_col + 1);
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
                    let len = grapheme_count(&self.edit_buffer[self.edit_cursor_row]);
                    self.edit_cursor_col = self.edit_cursor_col.min(len);
                    return true;
                }
            }
            KeyCode::Down => {
                if self.edit_cursor_row + 1 < self.edit_buffer.len() {
                    self.edit_cursor_row += 1;
                    let len = grapheme_count(&self.edit_buffer[self.edit_cursor_row]);
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
                    self.edit_cursor_col = grapheme_count(&self.edit_buffer[self.edit_cursor_row]);
                    return true;
                }
            }
            KeyCode::Right => {
                if self.edit_cursor_row < self.edit_buffer.len() {
                    let len = grapheme_count(&self.edit_buffer[self.edit_cursor_row]);
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
                    self.edit_cursor_col = grapheme_count(&self.edit_buffer[self.edit_cursor_row]);
                    return true;
                }
            }
            KeyCode::Tab if self.edit_cursor_row < self.edit_buffer.len() => {
                let line = &mut self.edit_buffer[self.edit_cursor_row];
                let idx = grapheme_byte_index(line, self.edit_cursor_col);
                line.insert_str(idx, "    ");
                self.edit_cursor_col += 4;
                self.edit_dirty = true;
                return true;
            }
            _ => {}
        }
        false
    }
}

// ── Text/code extension check ─────────────────────────────────────────────────

impl Drop for AppState {
    fn drop(&mut self) {
        self.persist_user_settings();
        if let Some(watch) = self.directory_watch.take() {
            watch.cancel.store(true, Ordering::Release);
        }
        if let Some(scan) = self.property_scan.take() {
            scan.cancel.store(true, Ordering::Release);
        }
        if let Some(operation) = self.operation_task.take() {
            operation.cancel.store(true, Ordering::Release);
        }
    }
}

fn scan_folder_stats(
    root: PathBuf,
    sender: std::sync::mpsc::Sender<FolderStats>,
    cancel: Arc<AtomicBool>,
) {
    let mut stats = FolderStats::default();
    let mut pending = vec![root];
    let mut last_update = Instant::now();

    while let Some(directory) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                stats.inaccessible = stats.inaccessible.saturating_add(1);
                continue;
            }
        };
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    stats.inaccessible = stats.inaccessible.saturating_add(1);
                    continue;
                }
            };
            let metadata = match fs::symlink_metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(_) => {
                    stats.inaccessible = stats.inaccessible.saturating_add(1);
                    continue;
                }
            };
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                // Do not follow links: doing so can loop or leave the selected tree.
                stats.inaccessible = stats.inaccessible.saturating_add(1);
            } else if file_type.is_dir() {
                stats.folders = stats.folders.saturating_add(1);
                pending.push(entry.path());
            } else {
                stats.files = stats.files.saturating_add(1);
                stats.bytes = stats.bytes.saturating_add(metadata.len());
            }

            if last_update.elapsed() >= Duration::from_millis(120) {
                if sender.send(stats.clone()).is_err() {
                    return;
                }
                last_update = Instant::now();
            }
        }
    }

    stats.complete = true;
    let _ = sender.send(stats);
}

pub fn filename_matches(entry: &DirEntryInfo, query: &str) -> bool {
    !query.is_empty()
        && entry.path.file_name().is_some_and(|name| {
            name.to_string_lossy()
                .to_lowercase()
                .contains(&query.to_lowercase())
        })
}

fn split_edit_text(content: &str) -> (Vec<String>, &'static str) {
    let line_ending = if content.contains("\r\n") {
        "\r\n"
    } else if content.contains('\r') {
        "\r"
    } else {
        "\n"
    };
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    // `split` intentionally keeps a final empty element. That represents the
    // newline at EOF as a real editable boundary, so Backspace/Delete/Enter
    // behave naturally at the bottom of the file.
    let mut lines = normalized
        .split('\n')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(String::new());
    }
    (lines, line_ending)
}

// ── Directory listing ─────────────────────────────────────────────────────────

fn scan_dir_uncached(path: &std::path::Path) -> std::io::Result<Vec<DirEntryInfo>> {
    fs::read_dir(path)?
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let path = entry.path();
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let metadata = entry.metadata().ok();
            let size = metadata
                .as_ref()
                .filter(|meta| !meta.is_dir())
                .map(|meta| meta.len())
                .unwrap_or(0);
            let modified = metadata.and_then(|meta| meta.modified().ok());
            Ok(DirEntryInfo {
                path,
                is_dir,
                size,
                modified,
            })
        })
        .collect()
}

fn directory_fingerprint(entries: &[DirEntryInfo]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut stable: Vec<&DirEntryInfo> = entries.iter().collect();
    stable.sort_by(|left, right| left.path.cmp(&right.path));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in stable {
        entry.path.hash(&mut hasher);
        entry.is_dir.hash(&mut hasher);
        entry.size.hash(&mut hasher);
        entry.modified.hash(&mut hasher);
    }
    hasher.finish()
}

fn start_directory_watch(path: PathBuf) -> DirectoryWatch {
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker_path = path.clone();
    std::thread::spawn(move || {
        let mut last_fingerprint = None;
        let mut last_error = None;
        let mut last_directory_modified = None;
        let mut last_full_scan = Instant::now()
            .checked_sub(Duration::from_secs(3))
            .unwrap_or_else(Instant::now);
        while !worker_cancel.load(Ordering::Acquire) {
            let directory_modified = fs::metadata(&worker_path)
                .and_then(|meta| meta.modified())
                .ok();
            let must_scan = directory_modified != last_directory_modified
                || last_full_scan.elapsed() >= Duration::from_secs(2);
            if must_scan {
                last_directory_modified = directory_modified;
                last_full_scan = Instant::now();
                match scan_dir_uncached(&worker_path) {
                    Ok(entries) => {
                        let fingerprint = directory_fingerprint(&entries);
                        if last_fingerprint != Some(fingerprint) {
                            last_fingerprint = Some(fingerprint);
                            last_error = None;
                            if sender
                                .send(DirectoryWatchEvent::Snapshot {
                                    path: worker_path.clone(),
                                    entries,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        let message = format!("Can't refresh {}: {}", worker_path.display(), error);
                        if last_error.as_deref() != Some(message.as_str()) {
                            last_error = Some(message.clone());
                            if sender
                                .send(DirectoryWatchEvent::Error {
                                    path: worker_path.clone(),
                                    message,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            }
            for _ in 0..10 {
                if worker_cancel.load(Ordering::Acquire) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    });
    DirectoryWatch {
        path,
        receiver,
        cancel,
    }
}

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

    let mut entries = scan_dir_uncached(path_ref)?;

    // Sort: directories first, then files, both alphabetically (case-insensitive)
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => {
            let a_name = a
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            let b_name = b
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            a_name.cmp(&b_name)
        }
    });

    // Store in cache
    DIR_CACHE.lock().insert(
        path_buf,
        DirCacheEntry {
            entries: entries.clone(),
            at: Instant::now(),
        },
    );

    Ok(entries)
}

pub(crate) fn sort_dir_entries(entries: &mut [DirEntryInfo], mode: SortMode, descending: bool) {
    entries.sort_by(|a, b| {
        let directory_order = match (a.is_dir, b.is_dir) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        };
        if directory_order != std::cmp::Ordering::Equal {
            return directory_order;
        }

        let name_a = a
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let name_b = b
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let order = match mode {
            SortMode::Name => name_a.cmp(&name_b),
            SortMode::Modified => a
                .modified
                .cmp(&b.modified)
                .then_with(|| name_a.cmp(&name_b)),
            SortMode::Size => a.size.cmp(&b.size).then_with(|| name_a.cmp(&name_b)),
        };
        if descending {
            order.reverse()
        } else {
            order
        }
    });
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
            DirEntryInfo {
                path,
                is_dir: true,
                size: 0,
                modified: None,
            }
        })
        .collect()
}

// ── Recursive copy helper ─────────────────────────────────────────────────────

#[cfg(windows)]
fn move_to_recycle_bin(paths: &[PathBuf]) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
    };

    // SHFileOperation expects a double-NUL-terminated list of paths.
    let mut from: Vec<u16> = Vec::new();
    for path in paths {
        from.extend(path.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);

    let mut operation = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let status = unsafe { SHFileOperationW(&mut operation) };
    if status == 0 && operation.fAnyOperationsAborted == 0 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "Recycle Bin operation failed (code {status})"
        )))
    }
}

fn destination_is_inside_source(src: &std::path::Path, destination_dir: &std::path::Path) -> bool {
    // Both paths normally exist at paste time. Canonicalization also resolves
    // `..`, drive-letter casing, and junction/symlink aliases on Windows.
    match (fs::canonicalize(src), fs::canonicalize(destination_dir)) {
        (Ok(src), Ok(destination)) => destination.starts_with(src),
        _ => destination_dir.starts_with(src),
    }
}

fn remove_path(path: &std::path::Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[derive(Debug)]
enum PasteOutcome {
    Completed,
    Skipped,
}

fn paste_one(
    source: &std::path::Path,
    destination_dir: &std::path::Path,
    is_cut: bool,
    cancel: &AtomicBool,
) -> std::io::Result<PasteOutcome> {
    check_cancelled(cancel)?;
    let file_name = source.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source has no file name")
    })?;
    if source.is_dir() && destination_is_inside_source(source, destination_dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "can't copy or move a folder into itself",
        ));
    }

    let mut destination = destination_dir.join(file_name);
    if destination.exists() && destination != source {
        destination = unique_dest_path(destination_dir, file_name);
    }
    if destination == source {
        return Ok(PasteOutcome::Skipped);
    }

    if is_cut && fs::rename(source, &destination).is_ok() {
        return Ok(PasteOutcome::Completed);
    }

    transactional_copy(source, &destination, cancel)?;
    check_cancelled(cancel)?;
    if is_cut {
        remove_path(source)?;
    }
    Ok(PasteOutcome::Completed)
}

fn transactional_copy(
    source: &std::path::Path,
    destination: &std::path::Path,
    cancel: &AtomicBool,
) -> std::io::Result<()> {
    check_cancelled(cancel)?;
    let parent = destination.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination has no parent",
        )
    })?;
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let mut temporary = None;
    for sequence in 0..10_000u32 {
        let candidate = parent.join(format!(
            ".rusty-ranger-partial-{}-{}-{}",
            name,
            std::process::id(),
            sequence
        ));
        if !candidate.exists() {
            temporary = Some(candidate);
            break;
        }
    }
    let temporary = temporary.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a temporary destination",
        )
    })?;

    let copied = if source.is_dir() {
        copy_dir_all(source, &temporary, cancel)
    } else {
        fs::copy(source, &temporary).map(|_| ())
    };
    if let Err(error) = copied {
        let _ = remove_path(&temporary);
        return Err(error);
    }
    if let Err(error) = check_cancelled(cancel) {
        let _ = remove_path(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = remove_path(&temporary);
        return Err(error);
    }
    Ok(())
}

fn snapshots_for_parents(paths: &[PathBuf]) -> Vec<(PathBuf, Vec<DirEntryInfo>)> {
    let directories: std::collections::HashSet<PathBuf> = paths
        .iter()
        .filter_map(|path| path.parent().map(std::path::Path::to_path_buf))
        .collect();
    snapshot_directories(directories)
}

fn snapshots_for_parents_and_directories(
    paths: &[PathBuf],
    destination: &std::path::Path,
) -> Vec<(PathBuf, Vec<DirEntryInfo>)> {
    let mut directories: std::collections::HashSet<PathBuf> = paths
        .iter()
        .filter_map(|path| path.parent().map(std::path::Path::to_path_buf))
        .collect();
    directories.insert(destination.to_path_buf());
    snapshot_directories(directories)
}

fn snapshot_directories(
    directories: std::collections::HashSet<PathBuf>,
) -> Vec<(PathBuf, Vec<DirEntryInfo>)> {
    directories
        .into_iter()
        .filter_map(|path| scan_dir_uncached(&path).ok().map(|entries| (path, entries)))
        .collect()
}

fn copy_dir_all(
    src: impl AsRef<std::path::Path>,
    dst: impl AsRef<std::path::Path>,
    cancel: &AtomicBool,
) -> std::io::Result<()> {
    check_cancelled(cancel)?;
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        check_cancelled(cancel)?;
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "symbolic link or reparse point is not followed: {}",
                    entry.path().display()
                ),
            ));
        } else if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()), cancel)?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn check_cancelled(cancel: &AtomicBool) -> std::io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "operation cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Generate a non-colliding destination path by appending " (2)", " (3)", …
/// before the extension, matching Windows Explorer's paste-collision behavior.
fn unique_dest_path(dir: &std::path::Path, file_name: &std::ffi::OsStr) -> PathBuf {
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
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_modes_have_distinct_settle_policies() {
        assert_eq!(
            preview_settle_delay(PreviewMode::Normal),
            Duration::from_millis(80)
        );
        assert_eq!(
            preview_settle_delay(PreviewMode::Full),
            Duration::from_millis(160)
        );
        assert_eq!(
            preview_settle_delay(PreviewMode::Blitz),
            Duration::from_millis(40)
        );
        assert_eq!(PreviewMode::Normal.office_policy(), OfficeRenderMode::Text);
        assert_eq!(PreviewMode::Normal.pdf_policy(), PdfRenderMode::Text);
        assert_eq!(PreviewMode::Full.office_policy(), OfficeRenderMode::Full);
        assert_eq!(PreviewMode::Full.pdf_policy(), PdfRenderMode::Visual);
        assert_eq!(PreviewMode::Blitz.office_policy(), OfficeRenderMode::Text);
        assert_eq!(PreviewMode::Blitz.pdf_policy(), PdfRenderMode::Text);
    }

    #[test]
    fn repeated_primary_click_opens_only_the_same_row_within_the_window() {
        let now = Instant::now();
        let interval = Duration::from_millis(750);
        let recent = now.checked_sub(Duration::from_millis(700)).unwrap();
        let stale = now.checked_sub(Duration::from_millis(751)).unwrap();

        assert!(is_repeated_primary_click(
            Some((recent, 2, 9)),
            now,
            2,
            9,
            interval
        ));
        assert!(!is_repeated_primary_click(
            Some((stale, 2, 9)),
            now,
            2,
            9,
            interval
        ));
        assert!(!is_repeated_primary_click(
            Some((recent, 1, 9)),
            now,
            2,
            9,
            interval
        ));
        assert!(!is_repeated_primary_click(
            Some((recent, 2, 8)),
            now,
            2,
            9,
            interval
        ));
    }

    #[test]
    fn rejects_windows_reserved_and_malformed_names() {
        for name in [
            "", "   ", ".", "..", "bad.", "bad ", "a/b", "CON", "con.txt", "LPT9.log",
        ] {
            assert!(
                AppState::validate_filename(name).is_err(),
                "{name:?} should be invalid"
            );
        }
        for name in [
            "report.txt",
            ".gitignore",
            "COM10",
            " leading space.txt",
            "résumé.md",
        ] {
            assert!(
                AppState::validate_filename(name).is_ok(),
                "{name:?} should be valid"
            );
        }
    }

    #[test]
    fn detects_destination_inside_source() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-path-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = base.join("source");
        let child = source.join("child");
        let sibling = base.join("sibling");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&sibling).unwrap();

        assert!(destination_is_inside_source(&source, &source));
        assert!(destination_is_inside_source(&source, &child));
        assert!(!destination_is_inside_source(&source, &sibling));

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn unique_destination_preserves_extensions() {
        let base =
            std::env::temp_dir().join(format!("rusty-ranger-dest-test-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("photo.jpg"), b"one").unwrap();
        fs::write(base.join("photo (2).jpg"), b"two").unwrap();

        assert_eq!(
            unique_dest_path(&base, std::ffi::OsStr::new("photo.jpg")),
            base.join("photo (3).jpg")
        );

        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn editor_preserves_lines_and_line_endings() {
        let (lines, ending) = split_edit_text("one\r\ntwo\r\n\r\n");
        assert_eq!(lines, ["one", "two", "", ""]);
        assert_eq!(ending, "\r\n");
        assert_eq!(lines.join(ending), "one\r\ntwo\r\n\r\n");

        let (lines, ending) = split_edit_text("single line");
        assert_eq!(lines, ["single line"]);
        assert_eq!(ending, "\n");
    }

    #[test]
    fn editor_expands_tabs_without_changing_the_buffer() {
        let source = "\tlet\tx = 1;";
        assert_eq!(crate::preview::expand_editor_tabs(source), "    let x = 1;");
        assert_eq!(source, "\tlet\tx = 1;");
    }

    #[test]
    fn sorting_keeps_folders_first_and_supports_metadata() {
        let mut entries = vec![
            DirEntryInfo {
                path: PathBuf::from("large.txt"),
                is_dir: false,
                size: 500,
                modified: Some(std::time::UNIX_EPOCH + Duration::from_secs(10)),
            },
            DirEntryInfo {
                path: PathBuf::from("folder"),
                is_dir: true,
                size: 0,
                modified: Some(std::time::UNIX_EPOCH + Duration::from_secs(5)),
            },
            DirEntryInfo {
                path: PathBuf::from("small.txt"),
                is_dir: false,
                size: 10,
                modified: Some(std::time::UNIX_EPOCH + Duration::from_secs(20)),
            },
        ];

        sort_dir_entries(&mut entries, SortMode::Size, false);
        assert_eq!(entries[0].path, PathBuf::from("folder"));
        assert_eq!(entries[1].path, PathBuf::from("small.txt"));
        assert_eq!(entries[2].path, PathBuf::from("large.txt"));

        sort_dir_entries(&mut entries, SortMode::Modified, true);
        assert_eq!(entries[0].path, PathBuf::from("folder"));
        assert_eq!(entries[1].path, PathBuf::from("small.txt"));
        assert_eq!(entries[2].path, PathBuf::from("large.txt"));
    }

    #[test]
    fn folder_scan_reports_recursive_totals() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-properties-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(base.join("nested").join("deeper")).unwrap();
        fs::write(base.join("one.bin"), [1_u8, 2, 3]).unwrap();
        fs::write(base.join("nested").join("two.bin"), [4_u8, 5]).unwrap();

        let (sender, receiver) = mpsc::channel();
        scan_folder_stats(base.clone(), sender, Arc::new(AtomicBool::new(false)));
        let stats = receiver.try_iter().last().expect("final folder totals");

        assert!(stats.complete);
        assert_eq!(stats.files, 2);
        assert_eq!(stats.folders, 2);
        assert_eq!(stats.bytes, 5);
        assert_eq!(stats.inaccessible, 0);

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn filename_filter_is_case_insensitive_and_literal() {
        let entry = DirEntryInfo {
            path: PathBuf::from("Quarterly Report [FINAL].PDF"),
            is_dir: false,
            size: 0,
            modified: None,
        };
        assert!(filename_matches(&entry, "report"));
        assert!(filename_matches(&entry, "[FINAL]"));
        assert!(filename_matches(&entry, ".pdf"));
        assert!(!filename_matches(&entry, "draft"));
        assert!(!filename_matches(&entry, ""));
    }

    #[test]
    fn transactional_copy_publishes_only_the_complete_destination() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-copy-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = base.join("source.txt");
        let destination_dir = base.join("destination");
        fs::create_dir_all(&destination_dir).unwrap();
        fs::write(&source, b"complete payload").unwrap();
        let cancel = AtomicBool::new(false);

        assert!(matches!(
            paste_one(&source, &destination_dir, false, &cancel).unwrap(),
            PasteOutcome::Completed
        ));
        assert_eq!(
            fs::read(destination_dir.join("source.txt")).unwrap(),
            b"complete payload"
        );
        assert!(fs::read_dir(&destination_dir)
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains("partial")));

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cancelled_copy_does_not_publish_a_destination() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-cancel-copy-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = base.join("source.txt");
        let destination_dir = base.join("destination");
        fs::create_dir_all(&destination_dir).unwrap();
        fs::write(&source, b"payload").unwrap();
        let cancel = AtomicBool::new(true);

        let error = paste_one(&source, &destination_dir, false, &cancel).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(!destination_dir.join("source.txt").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn grapheme_cursor_treats_tamil_combining_sequence_as_one_unit() {
        let value = "கொ";
        assert_eq!(grapheme_count(value), 1);
        assert_eq!(grapheme_byte_index(value, 1), value.len());
    }

    #[test]
    fn directory_listing_is_not_silently_limited_to_three_thousand_entries() {
        let base = std::env::temp_dir().join(format!(
            "rusty-ranger-listing-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        for index in 0..3_005 {
            fs::write(base.join(format!("item-{index:04}")), []).unwrap();
        }
        assert_eq!(scan_dir_uncached(&base).unwrap().len(), 3_005);
        fs::remove_dir_all(base).unwrap();
    }
}
