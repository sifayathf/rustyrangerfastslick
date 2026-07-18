use std::sync::{Arc, Mutex};
use std::thread;
use std::sync::mpsc::{channel, Sender, Receiver};
use image::DynamicImage;
use windows_sys::Win32::Foundation::{HWND, RECT, POINT, LPARAM, WPARAM, LRESULT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::Win32::Graphics::Gdi::*;
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

unsafe extern "system" fn enum_func(hwnd: HWND, lparam: isize) -> i32 {
    let mut class_name = [0u16; 256];
    let len = GetClassNameW(hwnd, class_name.as_mut_ptr(), 256);
    let name = String::from_utf16_lossy(&class_name[..len as usize]);
    if name == "CASCADIA_HOSTING_WINDOW_CLASS" || name == "ConsoleWindowClass" {
        *(lparam as *mut HWND) = hwnd;
        return 0;
    }
    1
}

fn get_terminal_hwnd() -> HWND {
    let mut hwnd: HWND = std::ptr::null_mut();
    unsafe {
        EnumWindows(Some(enum_func), &mut hwnd as *mut _ as isize);
        if hwnd.is_null() {
            hwnd = windows_sys::Win32::System::Console::GetConsoleWindow();
        }
    }
    hwnd
}

struct WindowState {
    img_bgra: Vec<u8>,
    img_w: u32,
    img_h: u32,
}

static mut STATE: Option<Mutex<WindowState>> = None;

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            
            if let Some(state_mutex) = &STATE {
                if let Ok(state) = state_mutex.try_lock() {
                    if !state.img_bgra.is_empty() {
                        let mut bi: BITMAPINFO = std::mem::zeroed();
                        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                        bi.bmiHeader.biWidth = state.img_w as i32;
                        bi.bmiHeader.biHeight = -(state.img_h as i32); // top-down
                        bi.bmiHeader.biPlanes = 1;
                        bi.bmiHeader.biBitCount = 32;
                        bi.bmiHeader.biCompression = BI_RGB as u32;
                        
                        StretchDIBits(
                            hdc,
                            0, 0, state.img_w as i32, state.img_h as i32,
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
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn native_window_thread(rx: Receiver<PreviewCmd>) {
    unsafe {
        STATE = Some(Mutex::new(WindowState {
            img_bgra: Vec::new(),
            img_w: 0,
            img_h: 0,
        }));

        let class_name = "RustyRangerPreview\0".encode_utf16().collect::<Vec<_>>();
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpfnWndProc = Some(wnd_proc);
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0, 0, 0, 0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );

        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);

        loop {
            // Process Win32 messages
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // Check commands
            if let Ok(cmd) = rx.try_recv() {
                match cmd {
                    PreviewCmd::ShowImage { img, cell_rect, term_cols, term_rows } => {
                        let term_hwnd = get_terminal_hwnd();
                        if !term_hwnd.is_null() && term_cols > 0 && term_rows > 0 {
                            let mut client_rect: RECT = std::mem::zeroed();
                            GetClientRect(term_hwnd, &mut client_rect);
                            
                            let mut pt = POINT { x: 0, y: 0 };
                            ClientToScreen(term_hwnd, &mut pt);
                            
                            let cw = (client_rect.right - client_rect.left) as f32 / term_cols as f32;
                            let ch = (client_rect.bottom - client_rect.top) as f32 / term_rows as f32;
                            
                            let px = pt.x + (cell_rect.x as f32 * cw) as i32;
                            let py = pt.y + (cell_rect.y as f32 * ch) as i32;
                            let pw = (cell_rect.width as f32 * cw) as i32;
                            let ph = (cell_rect.height as f32 * ch) as i32;
                            
                            // Fit image inside pw x ph
                            let img_w = img.width() as f32;
                            let img_h = img.height() as f32;
                            
                            let scale = (pw as f32 / img_w).min(ph as f32 / img_h).min(1.0);
                            let fw = (img_w * scale) as u32;
                            let fh = (img_h * scale) as u32;
                            
                            let cx = px + (pw - fw as i32) / 2;
                            let cy = py + (ph - fh as i32) / 2;
                            
                            let resized = img.resize_exact(fw, fh, image::imageops::FilterType::Lanczos3);
                            let bgra: Vec<u8> = resized.to_rgba8().pixels().flat_map(|p| vec![p[2], p[1], p[0], 255]).collect();
                            
                            if let Some(state_mutex) = &STATE {
                                if let Ok(mut state) = state_mutex.lock() {
                                    state.img_bgra = bgra;
                                    state.img_w = fw;
                                    state.img_h = fh;
                                }
                            }
                            
                            SetWindowPos(hwnd, std::ptr::null_mut(), cx, cy, fw as i32, fh as i32, SWP_SHOWWINDOW);
                            InvalidateRect(hwnd, std::ptr::null(), 0);
                        }
                    }
                    PreviewCmd::Hide => {
                        ShowWindow(hwnd, SW_HIDE);
                    }
                    PreviewCmd::Quit => {
                        break;
                    }
                }
            }
            thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}
