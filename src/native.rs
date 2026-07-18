// ================= src/native.rs =================
use std::sync::{Arc, Mutex};
use std::thread;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::collections::HashMap;
use image::DynamicImage;
use windows_sys::Win32::Foundation::{HWND, RECT, POINT, LPARAM, WPARAM, LRESULT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use ratatui::layout::Rect as TuiRect;

pub enum PreviewCmd {
    ShowImage {
        img: Arc<DynamicImage>,
        cell_rect: TuiRect,
        term_cols: u16,
        term_rows: u16,
    },
    Hide,
    Quit,
}

pub struct NativePreviewManager {
    sender: Sender<PreviewCmd>,
}

impl NativePreviewManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        thread::spawn(move || {
            native_window_thread(rx);
        });
        Self { sender: tx }
    }

    pub fn show(&self, img: Arc<DynamicImage>, cell_rect: TuiRect, term_cols: u16, term_rows: u16) {
        let _ = self.sender.send(PreviewCmd::ShowImage { img, cell_rect, term_cols, term_rows });
    }

    pub fn hide(&self) {
        let _ = self.sender.send(PreviewCmd::Hide);
    }
}

impl Drop for NativePreviewManager {
    fn drop(&mut self) {
        let _ = self.sender.send(PreviewCmd::Quit);
    }
}

// ── Debug Logging ─────────────────────────────────────────────────────────────
fn log_debug(msg: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(true)
        .open("debug.log")
    {
        use std::io::Write;
        let _ = writeln!(file, "{}", msg);
    }
}

// ── Robust HWND & Process Ancestry resolution ─────────────────────────────────

fn get_ancestor_pids(start_pid: u32) -> Vec<u32> {
    let mut ancestors = Vec::new();
    let mut current = start_pid;
    
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return ancestors;
        }
        
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        
        let mut parent_map = HashMap::new();
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                parent_map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        windows_sys::Win32::Foundation::CloseHandle(snapshot);
        
        while let Some(&parent) = parent_map.get(&current) {
            if parent == 0 || parent == current {
                break;
            }
            ancestors.push(parent);
            current = parent;
        }
    }
    
    ancestors
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: isize) -> i32 {
    let list = lparam as *mut Vec<(HWND, u32, String)>;
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);

    let mut class_buf = [0u16; 256];
    let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
    let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);

    (*list).push((hwnd, pid, class_name));
    1
}

fn find_terminal_window() -> HWND {
    let console_hwnd = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    if console_hwnd.is_null() {
        return std::ptr::null_mut();
    }

    let mut console_pid = 0;
    unsafe { GetWindowThreadProcessId(console_hwnd, &mut console_pid); }

    let mut class_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(console_hwnd, class_buf.as_mut_ptr(), 256) };
    let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);

    if class_name == "ConsoleWindowClass" {
        return console_hwnd;
    }

    // Process walking for ConPTY sessions (e.g. Windows Terminal)
    let ancestor_pids = get_ancestor_pids(console_pid);
    let mut all_windows: Vec<(HWND, u32, String)> = Vec::new();
    unsafe {
        EnumWindows(Some(enum_windows_proc), &mut all_windows as *mut _ as isize);
    }

    let mut candidates = Vec::new();
    for &(hwnd, pid, ref c_name) in &all_windows {
        if c_name == "CASCADIA_HOSTING_WINDOW_CLASS" && ancestor_pids.contains(&pid) {
            candidates.push(hwnd);
        }
    }

    if candidates.is_empty() {
        // Fallback to any CASCADIA_HOSTING_WINDOW_CLASS window
        for &(hwnd, _pid, ref c_name) in &all_windows {
            if c_name == "CASCADIA_HOSTING_WINDOW_CLASS" {
                candidates.push(hwnd);
            }
        }
    }

    if candidates.is_empty() {
        return console_hwnd;
    }

    if candidates.len() == 1 {
        return candidates[0];
    }

    // Prioritize active or visible window
    let fg_hwnd = unsafe { GetForegroundWindow() };
    for &cand in &candidates {
        if cand == fg_hwnd {
            return cand;
        }
    }

    for &cand in &candidates {
        if unsafe { IsWindowVisible(cand) } != 0 {
            return cand;
        }
    }

    candidates[0]
}

// ── Child Window Search ───────────────────────────────────────────────────────

struct EnumChildData {
    target_class: String,
    found_hwnd: HWND,
}

unsafe extern "system" fn enum_child_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    let data = lparam as *mut EnumChildData;
    let mut class_buf = [0u16; 256];
    let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
    let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);
    
    if class_name == (*data).target_class {
        (*data).found_hwnd = hwnd;
        return 0; // stop enumeration
    }
    1 // continue
}

fn find_child_window_by_class(parent: HWND, class_name: &str) -> HWND {
    let mut data = EnumChildData {
        target_class: class_name.to_string(),
        found_hwnd: std::ptr::null_mut(),
    };
    unsafe {
        EnumChildWindows(parent, Some(enum_child_proc), &mut data as *mut _ as LPARAM);
    }
    data.found_hwnd
}

// ── Safe Window State ──────────────────────────────────────────────────────────

struct WindowState {
    img_bgra: Vec<u8>,
    img_w: u32,
    img_h: u32,
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = lparam as *const CREATESTRUCTW;
            if !cs.is_null() {
                let state_ptr = (*cs).lpCreateParams;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_PAINT => {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            
            let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Mutex<WindowState>;
            if !state_ptr.is_null() {
                if let Ok(state) = (*state_ptr).try_lock() {
                    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                    GetClientRect(hwnd, &mut rect);
                    
                    // Dark background fill (#1e1e2e equivalent)
                    let brush = CreateSolidBrush(30 | (30 << 8) | (46 << 16));
                    FillRect(hdc, &rect, brush);
                    DeleteObject(brush);

                    if !state.img_bgra.is_empty() {
                        let mut bi: BITMAPINFO = std::mem::zeroed();
                        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                        bi.bmiHeader.biWidth = state.img_w as i32;
                        bi.bmiHeader.biHeight = -(state.img_h as i32); // top-down
                        bi.bmiHeader.biPlanes = 1;
                        bi.bmiHeader.biBitCount = 32;
                        bi.bmiHeader.biCompression = BI_RGB as u32;
                        
                        // Center within the popup window
                        let win_w = rect.right - rect.left;
                        let win_h = rect.bottom - rect.top;
                        let cx = (win_w - state.img_w as i32) / 2;
                        let cy = (win_h - state.img_h as i32) / 2;

                        StretchDIBits(
                            hdc,
                            cx, cy, state.img_w as i32, state.img_h as i32,
                            0, 0, state.img_w as i32, state.img_h as i32,
                            state.img_bgra.as_ptr() as *const _,
                            &bi,
                            DIB_RGB_COLORS,
                            SRCCOPY
                        );
                    }
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_DESTROY => {
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── Overlay window thread ─────────────────────────────────────────────────────

fn native_window_thread(rx: Receiver<PreviewCmd>) {
    unsafe {
        let state_ptr = Box::into_raw(Box::new(Mutex::new(WindowState {
            img_bgra: Vec::new(),
            img_w: 0,
            img_h: 0,
        })));

        let class_name = "RustyRangerPreview\0".encode_utf16().collect::<Vec<_>>();
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpfnWndProc = Some(wnd_proc);
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassExW(&wc);

        let term_hwnd = find_terminal_window();

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0, 0, 0, 0,
            term_hwnd, // Owned window of Windows Terminal/conhost
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            state_ptr as *const _,
        );

        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);

        loop {
            // Process Win32 messages
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    let _ = Box::from_raw(state_ptr);
                    return;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // Coalesce incoming commands
            let mut latest_cmd = None;
            while let Ok(cmd) = rx.try_recv() {
                latest_cmd = Some(cmd);
            }

            if let Some(cmd) = latest_cmd {
                match cmd {
                    PreviewCmd::ShowImage { img, cell_rect, term_cols, term_rows } => {
                        let term_hwnd = find_terminal_window();
                        if !term_hwnd.is_null() && term_cols > 0 && term_rows > 0 {
                            let mut class_buf = [0u16; 256];
                            let class_len = GetClassNameW(term_hwnd, class_buf.as_mut_ptr(), 256);
                            let term_class = String::from_utf16_lossy(&class_buf[..class_len as usize]);

                            let (bridge_hwnd, padding) = if term_class == "CASCADIA_HOSTING_WINDOW_CLASS" {
                                let bridge = find_child_window_by_class(term_hwnd, "Windows.UI.Composition.DesktopWindowContentBridge");
                                if !bridge.is_null() {
                                    let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(term_hwnd);
                                    let pad = (8.0f32 * (dpi as f32) / 96.0f32).round() as i32;
                                    (bridge, pad)
                                } else {
                                    (term_hwnd, 0)
                                }
                            } else {
                                (term_hwnd, 0)
                            };

                            let mut client_rect: RECT = std::mem::zeroed();
                            GetClientRect(bridge_hwnd, &mut client_rect);
                            
                            let width = client_rect.right - client_rect.left;
                            let height = client_rect.bottom - client_rect.top;
                            
                            let grid_width = (width - 2 * padding).max(1);
                            let grid_height = (height - 2 * padding).max(1);
                            
                            let cw = grid_width / term_cols as i32;
                            let ch = grid_height / term_rows as i32;
                            
                            let mut pt = POINT { x: 0, y: 0 };
                            ClientToScreen(bridge_hwnd, &mut pt);
                            
                            let px = pt.x + padding + cell_rect.x as i32 * cw;
                            let py = pt.y + padding + cell_rect.y as i32 * ch;
                            let pw = cell_rect.width as i32 * cw;
                            let ph = cell_rect.height as i32 * ch;
                            
                            let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(term_hwnd);
                            log_debug(&format!(
                                "[OVERLAY]\n\
                                 terminal hwnd = {:?}\n\
                                 terminal client rect = L: {}, T: {}, R: {}, B: {}\n\
                                 terminal cols/rows = {}/{}\n\
                                 preview cell rect = x: {}, y: {}, w: {}, h: {}\n\
                                 calculated pixel rect = x: {}, y: {}, w: {}, h: {}\n\
                                 DPI = {}\n\
                                 image source dimensions = {} × {}\n",
                                term_hwnd, client_rect.left, client_rect.top, client_rect.right, client_rect.bottom,
                                term_cols, term_rows, cell_rect.x, cell_rect.y, cell_rect.width, cell_rect.height,
                                px, py, pw, ph, dpi, img.width(), img.height()
                            ));

                            // Fit image inside pw x ph
                            let img_w = img.width() as f32;
                            let img_h = img.height() as f32;
                            
                            let scale = (pw as f32 / img_w).min(ph as f32 / img_h).min(1.0);
                            let fw = (img_w * scale) as u32;
                            let fh = (img_h * scale) as u32;

                            // Native window is sized and positioned to match the viewport EXACTLY
                            SetWindowPos(hwnd, std::ptr::null_mut(), px, py, pw, ph, SWP_SHOWWINDOW);

                            // Decode/resize outside of WM_PAINT
                            let resized = img.resize_exact(fw, fh, image::imageops::FilterType::Lanczos3);
                            let bgra: Vec<u8> = resized.to_rgba8().pixels().flat_map(|p| vec![p[2], p[1], p[0], 255]).collect();
                            
                            if let Ok(mut state) = (*state_ptr).lock() {
                                state.img_bgra = bgra;
                                state.img_w = fw;
                                state.img_h = fh;
                            }
                            
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    }
                    PreviewCmd::Hide => {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                    PreviewCmd::Quit => {
                        let _ = Box::from_raw(state_ptr);
                        DestroyWindow(hwnd);
                        return;
                    }
                }
            }
            thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}
