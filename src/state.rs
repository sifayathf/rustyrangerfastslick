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
use ratatui::layout::{Layout, Direction, Constraint, Rect};

// ── Directory listing cache ───────────────────────────────────────────────────
// TTL prevents re-reading the filesystem every frame when the preview pane
// hovers over a directory. Cap stops UI freezes on System32-sized folders.

const DIR_CACHE_TTL: Duration = Duration::from_secs(2);
const MAX_DIR_ENTRIES: usize  = 3_000;

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
}

#[derive(Clone, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    Rename,
    ConfirmDelete,
    ContextMenu,
    NewFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DividerType {
    NavToMiller,
    MillerToMiller(usize),
    MillerToPreview,
}

#[derive(Default, Clone)]
pub struct LayoutGeometry {
    pub header_rect: Rect,
    pub nav_rail_rect: Rect,
    pub miller_region_rect: Rect,
    pub pane_rects: Vec<Rect>,
    pub preview_outer_rect: Rect,
    pub preview_header_rect: Rect,
    pub preview_viewport_rect: Rect,
    pub preview_controls_rect: Rect,
    pub preview_metadata_rect: Rect,
    pub status_rect: Rect,
    pub divider_rects: Vec<Rect>,
    pub divider_types: Vec<DividerType>,
    pub row_rects: HashMap<usize, Vec<(usize, Rect)>>,
    pub nav_row_rects: Vec<(usize, Rect)>,
}

pub struct AppState {
    pub levels:        Vec<DirLevel>,
    pub current_level: usize,
    last_event_time:   Instant,

    // Preview interaction state
    pub mode:            AppMode,  // current application mode
    pub input_buffer:    String,   // used for Rename / New Folder
    pub clipboard:       Option<(PathBuf, bool)>, // (path, is_cut)
    pub image_zoom:      f32,      // 1.0 = 100%; +/- adjust
    pub image_rotation:  u32,      // 0 / 90 / 180 / 270 degrees
    pub image_flip_h:    bool,     // 'f' flips horizontally
    pub preview_scroll:  usize,    // manual scroll offset for preview pane
    
    // Mouse double-click tracking: (time, pane_idx, file_idx)
    pub last_click:      Option<(Instant, usize, usize)>,

    // Layout and resizing state
    pub dragging_divider: Option<usize>,
    pub column_ratios:    Vec<f32>,
    pub layout_geometry:  Arc<Mutex<LayoutGeometry>>,
    pub nav_rail_width:   u16,
    pub nav_rail_visible: bool,

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

        let level = DirLevel {
            path: home,
            files,
            selected: 0,
        };

        Ok(Self {
            levels:         vec![level],
            current_level:  0,
            last_event_time: Instant::now(),
            mode:           AppMode::Normal,
            input_buffer:   String::new(),
            clipboard:      None,
            last_click:     None,
            dragging_divider: None,
            column_ratios:    vec![0.10, 0.10, 0.12, 0.18, 0.50],
            layout_geometry:  Arc::new(Mutex::new(LayoutGeometry::default())),
            nav_rail_width:   28,
            nav_rail_visible: false,
            preview_scroll: 0,
            image_zoom: 1.0,
            image_rotation: 0,
            image_flip_h: false,
            native_preview: crate::native::NativePreviewManager::new(),
        })
    }

    pub fn current(&self) -> &DirLevel {
        &self.levels[self.current_level]
    }

    pub fn current_mut(&mut self) -> &mut DirLevel {
        &mut self.levels[self.current_level]
    }

    pub fn has_preview(&self) -> bool {
        let cur = self.current();
        !cur.files.is_empty()
    }

    pub fn calculate_layout(&self, area: Rect) -> LayoutGeometry {
        let mut geo = LayoutGeometry::default();
        
        let vertical_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // breadcrumb
                Constraint::Min(0),    // panes
                Constraint::Length(1), // status bar
            ])
            .split(area);
            
        geo.header_rect = vertical_chunks[0];
        geo.status_rect = vertical_chunks[2];
        
        let panes_area = vertical_chunks[1];
        
        let rail_width = if self.nav_rail_visible {
            if panes_area.width < 100 {
                0
            } else {
                self.nav_rail_width
            }
        } else {
            0
        };
        
        let preview_width = if self.has_preview() {
            let min_preview = 50;
            let mut pw = (panes_area.width as f32 * self.column_ratios[4]) as u16;
            if pw < min_preview {
                pw = min_preview;
            }
            if pw >= panes_area.width.saturating_sub(rail_width + 30) {
                pw = panes_area.width.saturating_sub(rail_width + 30);
            }
            pw
        } else {
            0
        };
        
        let miller_width = panes_area.width.saturating_sub(rail_width + preview_width);
        
        let horizontal_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(rail_width),
                Constraint::Length(miller_width),
                Constraint::Length(preview_width),
            ])
            .split(panes_area);
            
        geo.nav_rail_rect = horizontal_chunks[0];
        geo.miller_region_rect = horizontal_chunks[1];
        geo.preview_outer_rect = horizontal_chunks[2];
        
        let num = self.levels.len();
        let start = if num > 4 { num - 4 } else { 0 };
        let panes = &self.levels[start..];
        let np = panes.len();
        
        if np > 0 {
            let has_prev = preview_width > 0;
            let total_cols = np + if has_prev { 1 } else { 0 };
            let start_idx = 5 - total_cols;
            let mut sub_ratios = self.column_ratios[start_idx..].to_vec();
            
            let sum: f32 = sub_ratios.iter().take(np).sum();
            if sum > 0.0 {
                for r in sub_ratios.iter_mut().take(np) {
                    *r /= sum;
                }
            }
            
            let mut constraints: Vec<Constraint> = sub_ratios.iter().take(np.saturating_sub(1)).map(|&r| {
                Constraint::Percentage((r * 100.0) as u16)
            }).collect();
            constraints.push(Constraint::Min(0));
            
            let miller_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(constraints)
                .split(geo.miller_region_rect);
                
            for i in 0..np {
                geo.pane_rects.push(miller_chunks[i]);
            }
        }
        
        if preview_width > 0 {
            let pr = geo.preview_outer_rect;
            let has_metadata_space = pr.height > 20;
            let metadata_height = if has_metadata_space { 12 } else { 0 };
            
            let preview_vertical = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(4), // Header
                    Constraint::Min(0),    // Viewport
                    Constraint::Length(3), // Controls/separator
                    Constraint::Length(metadata_height), // Metadata
                ])
                .split(pr);
                
            geo.preview_header_rect = preview_vertical[0];
            geo.preview_viewport_rect = preview_vertical[1];
            geo.preview_controls_rect = preview_vertical[2];
            geo.preview_metadata_rect = preview_vertical[3];
        }
        
        if rail_width > 0 {
            geo.divider_rects.push(Rect {
                x: geo.nav_rail_rect.x + geo.nav_rail_rect.width - 1,
                y: panes_area.y,
                width: 2,
                height: panes_area.height,
            });
            geo.divider_types.push(DividerType::NavToMiller);
        }
        
        for i in 0..geo.pane_rects.len().saturating_sub(1) {
            let current_pane = geo.pane_rects[i];
            geo.divider_rects.push(Rect {
                x: current_pane.x + current_pane.width - 1,
                y: panes_area.y,
                width: 2,
                height: panes_area.height,
            });
            geo.divider_types.push(DividerType::MillerToMiller(i));
        }
        
        if preview_width > 0 {
            geo.divider_rects.push(Rect {
                x: geo.miller_region_rect.x + geo.miller_region_rect.width - 1,
                y: panes_area.y,
                width: 2,
                height: panes_area.height,
            });
            geo.divider_types.push(DividerType::MillerToPreview);
        }
        
        geo
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
                        if let Some(p) = self.selected_file().or_else(|| {
                            let cur = self.current();
                            if !cur.files.is_empty() {
                                Some(cur.files[cur.selected].path.clone())
                            } else {
                                None
                            }
                        }) {
                            self.mode = AppMode::Rename;
                            self.input_buffer = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                        }
                    }
                    (KeyCode::Delete, _) if self.mode == AppMode::Normal => {
                        if !self.current().files.is_empty() {
                            self.mode = AppMode::ConfirmDelete;
                        }
                    }
                    (KeyCode::F(7), KeyModifiers::NONE) | (KeyCode::Char('n'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        self.mode = AppMode::NewFolder;
                        self.input_buffer.clear();
                    }

                    // ── Clipboard Operations (Copy, Cut, Paste) ─────────────
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        let cur = self.current();
                        if !cur.files.is_empty() {
                            self.clipboard = Some((cur.files[cur.selected].path.clone(), false));
                        }
                    }
                    (KeyCode::Char('x'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        let cur = self.current();
                        if !cur.files.is_empty() {
                            self.clipboard = Some((cur.files[cur.selected].path.clone(), true));
                        }
                    }
                    (KeyCode::Char('v'), KeyModifiers::CONTROL) if self.mode == AppMode::Normal => {
                        if let Some((src_path, is_cut)) = self.clipboard.clone() {
                            let dest_dir = self.current().path.clone();
                            let dest_path = dest_dir.join(src_path.file_name().unwrap());
                            
                            // A simple copy/move. For directories, recursive copy is needed.
                            // To keep it simple, we use a custom recursive copy for dirs.
                            if is_cut {
                                if fs::rename(&src_path, &dest_path).is_ok() {
                                    self.clipboard = None;
                                    DIR_CACHE.lock().remove(src_path.parent().unwrap());
                                    DIR_CACHE.lock().remove(&dest_dir);
                                    if let Ok(files) = list_dir(&dest_dir) {
                                        self.current_mut().files = files;
                                    }
                                }
                            } else {
                                if src_path.is_dir() {
                                    let _ = copy_dir_all(&src_path, &dest_path);
                                } else {
                                    let _ = fs::copy(&src_path, &dest_path);
                                }
                                DIR_CACHE.lock().remove(&dest_dir);
                                if let Ok(files) = list_dir(&dest_dir) {
                                    self.current_mut().files = files;
                                }
                            }
                        }
                    }

                    // ── Typing in Input Modes ────────────────────────────────
                    (KeyCode::Char(c), _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        self.input_buffer.push(c);
                        return Ok(false);
                    }
                    (KeyCode::Backspace, _) if self.mode == AppMode::Rename || self.mode == AppMode::NewFolder => {
                        self.input_buffer.pop();
                        return Ok(false);
                    }
                    (KeyCode::Enter, _) if self.mode == AppMode::Rename => {
                        if !self.input_buffer.is_empty() {
                            let cur = self.current();
                            if !cur.files.is_empty() {
                                let old_path = cur.files[cur.selected].path.clone();
                                let mut new_path = old_path.clone();
                                new_path.set_file_name(&self.input_buffer);
                                
                                if let Ok(_) = fs::rename(&old_path, &new_path) {
                                    // Invalidate cache and reload
                                    if let Some(parent) = old_path.parent() {
                                        DIR_CACHE.lock().remove(parent);
                                        // A quick refresh of current level
                                        if let Ok(files) = list_dir(parent) {
                                            self.current_mut().files = files;
                                        }
                                    }
                                }
                            }
                        }
                        self.mode = AppMode::Normal;
                        return Ok(false);
                    }
                    (KeyCode::Enter, _) if self.mode == AppMode::NewFolder => {
                        if !self.input_buffer.is_empty() {
                            let parent = self.current().path.clone();
                            let new_dir = parent.join(&self.input_buffer);
                            if let Ok(_) = fs::create_dir(&new_dir) {
                                DIR_CACHE.lock().remove(&parent);
                                if let Ok(files) = list_dir(&parent) {
                                    self.current_mut().files = files;
                                }
                            }
                        }
                        self.mode = AppMode::Normal;
                        return Ok(false);
                    }
                    (KeyCode::Char('y'), _) | (KeyCode::Char('Y'), _) if self.mode == AppMode::ConfirmDelete => {
                        let cur = self.current();
                        if !cur.files.is_empty() {
                            let path = cur.files[cur.selected].path.clone();
                            if path.is_dir() {
                                let _ = fs::remove_dir_all(&path);
                            } else {
                                let _ = fs::remove_file(&path);
                            }
                            if let Some(parent) = path.parent() {
                                DIR_CACHE.lock().remove(parent);
                                if let Ok(files) = list_dir(parent) {
                                    self.current_mut().files = files;
                                    // Adjust selection if we deleted the last item
                                    let c = self.current_mut();
                                    if c.selected >= c.files.len() && !c.files.is_empty() {
                                        c.selected = c.files.len() - 1;
                                    } else if c.files.is_empty() {
                                        c.selected = 0;
                                    }
                                }
                            }
                        }
                        self.mode = AppMode::Normal;
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
                    MouseEventKind::ScrollDown => {
                        let geo = self.layout_geometry.lock().clone();
                        let mut over_preview = false;
                        let pr = geo.preview_outer_rect;
                        if pr.width > 0 && mouse.column >= pr.x && mouse.column < pr.x + pr.width && mouse.row >= pr.y && mouse.row < pr.y + pr.height {
                            over_preview = true;
                        }
                        if over_preview {
                            self.preview_scroll = self.preview_scroll.saturating_add(5);
                        } else {
                            // Find pane
                            for (idx, pr) in geo.pane_rects.iter().enumerate() {
                                if mouse.column >= pr.x && mouse.column < pr.x + pr.width && mouse.row >= pr.y && mouse.row < pr.y + pr.height {
                                    // Make this pane active and scroll down
                                    let num = self.levels.len();
                                    let start = if num > 4 { num - 4 } else { 0 };
                                    let real_idx = start + idx;
                                    if real_idx < self.levels.len() {
                                        self.current_level = real_idx;
                                        self.levels.truncate(self.current_level + 1);
                                        self.move_down();
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        let geo = self.layout_geometry.lock().clone();
                        let mut over_preview = false;
                        let pr = geo.preview_outer_rect;
                        if pr.width > 0 && mouse.column >= pr.x && mouse.column < pr.x + pr.width && mouse.row >= pr.y && mouse.row < pr.y + pr.height {
                            over_preview = true;
                        }
                        if over_preview {
                            self.preview_scroll = self.preview_scroll.saturating_sub(5);
                        } else {
                            for (idx, pr) in geo.pane_rects.iter().enumerate() {
                                if mouse.column >= pr.x && mouse.column < pr.x + pr.width && mouse.row >= pr.y && mouse.row < pr.y + pr.height {
                                    let num = self.levels.len();
                                    let start = if num > 4 { num - 4 } else { 0 };
                                    let real_idx = start + idx;
                                    if real_idx < self.levels.len() {
                                        self.current_level = real_idx;
                                        self.levels.truncate(self.current_level + 1);
                                        self.move_up();
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let geo = self.layout_geometry.lock().clone();
                        let mut divider_found = false;
                        
                        // Check dividers first
                        for (idx, dr) in geo.divider_rects.iter().enumerate() {
                            if mouse.column >= dr.x && mouse.column < dr.x + dr.width {
                                self.dragging_divider = Some(idx);
                                divider_found = true;
                                break;
                            }
                        }
                        
                        if !divider_found {
                            // Check breadcrumbs
                            if mouse.row == geo.header_rect.y && mouse.column >= geo.header_rect.x && mouse.column < geo.header_rect.x + geo.header_rect.width {
                                // TODO: Breadcrumb navigation
                            }
                            // Check row clicks
                            else {
                                for (pane_idx, rows) in geo.row_rects.iter() {
                                    for (file_idx, rect) in rows {
                                        if mouse.column >= rect.x && mouse.column < rect.x + rect.width && mouse.row == rect.y {
                                            // Clicked a row!
                                            let num = self.levels.len();
                                            let start = if num > 4 { num - 4 } else { 0 };
                                            let real_level_idx = start + pane_idx;
                                            
                                            if real_level_idx < self.levels.len() {
                                                let was_active_pane = self.current_level == real_level_idx;
                                                self.current_level = real_level_idx;
                                                self.levels.truncate(self.current_level + 1);
                                                
                                                let now = Instant::now();
                                                let last_click_val = self.last_click;
                                                let mut new_last_click = None;
                                                let mut new_mode = None;
                                                let mut new_input = None;
                                                let mut is_double_click = false;
                                                
                                                let cur = self.current_mut();
                                                if *file_idx < cur.files.len() {
                                                    let old_selected = cur.selected;
                                                    
                                                    is_double_click = if let Some((last_time, last_pane, last_file)) = last_click_val {
                                                        last_pane == *pane_idx && last_file == *file_idx && now.duration_since(last_time).as_millis() < 500
                                                    } else {
                                                        false
                                                    };
                                                    
                                                    if is_double_click {
                                                        cur.selected = *file_idx;
                                                    } else {
                                                        new_last_click = Some((now, *pane_idx, *file_idx));
                                                        
                                                        if was_active_pane && old_selected == *file_idx {
                                                            new_mode = Some(AppMode::Rename);
                                                            new_input = Some(cur.files[*file_idx].path.file_name().unwrap_or_default().to_string_lossy().to_string());
                                                        } else {
                                                            cur.selected = *file_idx;
                                                        }
                                                    }
                                                }
                                                
                                                if is_double_click {
                                                    self.last_click = None;
                                                    self.open_selected();
                                                } else if new_last_click.is_some() {
                                                    self.last_click = new_last_click;
                                                    if let Some(mode) = new_mode {
                                                        self.mode = mode;
                                                        self.input_buffer = new_input.unwrap();
                                                    } else {
                                                        self.reset_image_state();
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if let Some(idx) = self.dragging_divider {
                            let geo = self.layout_geometry.lock().clone();
                            if let Some(&div_type) = geo.divider_types.get(idx) {
                                let term_w = crossterm::terminal::size().unwrap_or((100, 30)).0;
                                let old_bound = geo.divider_rects.get(idx).map(|r| r.x).unwrap_or(0);
                                let new_bound = mouse.column;
                                
                                if old_bound > 0 && new_bound != old_bound && new_bound > 0 && new_bound < term_w {
                                    let delta_px = new_bound as i32 - old_bound as i32;
                                    let delta_ratio = delta_px as f32 / term_w as f32;
                                    
                                    match div_type {
                                        DividerType::NavToMiller => {
                                            let new_w = (self.nav_rail_width as i32 + delta_px).clamp(10, 45);
                                            self.nav_rail_width = new_w as u16;
                                        }
                                        DividerType::MillerToMiller(m_idx) => {
                                            let num = self.levels.len();
                                            let start = if num > 4 { num - 4 } else { 0 };
                                            let panes = &self.levels[start..];
                                            let np = panes.len();
                                            let has_prev = self.has_preview();
                                            let total_cols = np + if has_prev { 1 } else { 0 };
                                            
                                            let start_ratio_idx = 5 - total_cols;
                                            let ratio_idx_left = start_ratio_idx + m_idx;
                                            let ratio_idx_right = ratio_idx_left + 1;
                                            
                                            if ratio_idx_left < self.column_ratios.len() && ratio_idx_right < self.column_ratios.len() {
                                                let val_left = self.column_ratios[ratio_idx_left] + delta_ratio;
                                                let val_right = self.column_ratios[ratio_idx_right] - delta_ratio;
                                                if val_left > 0.05 && val_right > 0.05 {
                                                    self.column_ratios[ratio_idx_left] = val_left;
                                                    self.column_ratios[ratio_idx_right] = val_right;
                                                }
                                            }
                                        }
                                        DividerType::MillerToPreview => {
                                            let num = self.levels.len();
                                            let start = if num > 4 { num - 4 } else { 0 };
                                            let panes = &self.levels[start..];
                                            let np = panes.len();
                                            let total_cols = np + 1;
                                            
                                            let start_ratio_idx = 5 - total_cols;
                                            let ratio_idx_left = start_ratio_idx + np - 1;
                                            let ratio_idx_right = 5 - 1;
                                            
                                            if ratio_idx_left < self.column_ratios.len() && ratio_idx_right < self.column_ratios.len() {
                                                let val_left = self.column_ratios[ratio_idx_left] + delta_ratio;
                                                let val_right = self.column_ratios[ratio_idx_right] - delta_ratio;
                                                if val_left > 0.05 && val_right > 0.05 {
                                                    self.column_ratios[ratio_idx_left] = val_left;
                                                    self.column_ratios[ratio_idx_right] = val_right;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        self.dragging_divider = None;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(false)
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

            let new_level = DirLevel {
                path: selected_entry.path.clone(),
                files,
                selected: 0,
            };

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

                let new_level = DirLevel {
                    path: parent_path,
                    files,
                    selected,
                };

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
            self.levels = vec![DirLevel {
                path: home,
                files,
                selected: 0,
            }];
            self.current_level = 0;
        }
        Ok(())
    }

    #[cfg(not(windows))]
    fn go_root(&mut self) -> anyhow::Result<()> {
        let root = PathBuf::from("/");
        let files = list_dir(&root)?;
        self.levels = vec![DirLevel {
            path: root,
            files,
            selected: 0,
        }];
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
        self.levels = vec![DirLevel {
            path: drives_root,
            files: drives,
            selected: 0,
        }];
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
