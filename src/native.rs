// ================= src/native.rs =================
use image::DynamicImage;
use ratatui::layout::Rect as TuiRect;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::thread;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub enum PreviewCmd {
    ShowImage {
        generation: u64,
        img: Arc<DynamicImage>,
        path: std::path::PathBuf,
        rotation: u32,
        flip_h: bool,
        zoom: f32,
        cell_rect: TuiRect,
        term_cols: u16,
        term_rows: u16,
        background: u32,
        ultra_fast: bool,
    },
    Hide {
        generation: u64,
    },
    Quit,
}

pub struct NativePreviewManager {
    sender: Sender<PreviewCmd>,
    generation: Arc<AtomicU64>,
}

impl NativePreviewManager {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);
        thread::spawn(move || {
            native_window_thread(rx, worker_generation);
        });
        Self {
            sender: tx,
            generation,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &self,
        img: Arc<DynamicImage>,
        path: std::path::PathBuf,
        rotation: u32,
        flip_h: bool,
        zoom: f32,
        cell_rect: TuiRect,
        term_cols: u16,
        term_rows: u16,
        background: u32,
        ultra_fast: bool,
    ) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.sender.send(PreviewCmd::ShowImage {
            generation,
            img,
            path,
            rotation,
            flip_h,
            zoom,
            cell_rect,
            term_cols,
            term_rows,
            background,
            ultra_fast,
        });
    }

    pub fn hide(&self) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.sender.send(PreviewCmd::Hide { generation });
    }
}

impl Drop for NativePreviewManager {
    fn drop(&mut self) {
        let _ = self.sender.send(PreviewCmd::Quit);
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
    unsafe {
        GetWindowThreadProcessId(console_hwnd, &mut console_pid);
    }

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
    background: u32,
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    GetClientRect(hwnd, &mut rect);

                    let brush = CreateSolidBrush(state.background);
                    FillRect(hdc, &rect, brush);
                    DeleteObject(brush);

                    if !state.img_bgra.is_empty() {
                        let mut bi: BITMAPINFO = std::mem::zeroed();
                        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                        bi.bmiHeader.biWidth = state.img_w as i32;
                        bi.bmiHeader.biHeight = -(state.img_h as i32); // top-down
                        bi.bmiHeader.biPlanes = 1;
                        bi.bmiHeader.biBitCount = 32;
                        bi.bmiHeader.biCompression = BI_RGB;

                        // Center within the popup window
                        let win_w = rect.right - rect.left;
                        let win_h = rect.bottom - rect.top;
                        let cx = (win_w - state.img_w as i32) / 2;
                        let cy = (win_h - state.img_h as i32) / 2;

                        StretchDIBits(
                            hdc,
                            cx,
                            cy,
                            state.img_w as i32,
                            state.img_h as i32,
                            0,
                            0,
                            state.img_w as i32,
                            state.img_h as i32,
                            state.img_bgra.as_ptr() as *const _,
                            &bi,
                            DIB_RGB_COLORS,
                            SRCCOPY,
                        );
                    }
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_DESTROY => 0,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── Overlay window thread ─────────────────────────────────────────────────────

fn native_window_thread(rx: Receiver<PreviewCmd>, newest_generation: Arc<AtomicU64>) {
    unsafe {
        let state_ptr = Box::into_raw(Box::new(Mutex::new(WindowState {
            img_bgra: Vec::new(),
            img_w: 0,
            img_h: 0,
            background: 30 | (30 << 8) | (46 << 16),
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
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            term_hwnd, // Owned window of Windows Terminal/conhost
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            state_ptr as *const _,
        );

        if hwnd.is_null() {
            let _ = Box::from_raw(state_ptr);
            UnregisterClassW(class_name.as_ptr(), std::ptr::null_mut());
            return;
        }

        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);

        // ── Caches to keep the per-tick work cheap ──────────────────────────
        // Resolving the terminal window used to call EnumWindows over every
        // top-level window on the system, plus a full process-tree walk, on
        // *every* ShowImage message (i.e. every redraw). That single call
        // could easily cost several milliseconds; done 60x/sec it was the
        // main cause of visible flicker and the preview "hanging" under any
        // mouse movement. Now we resolve it once and only re-resolve if it
        // goes stale (window closed/replaced) or periodically as a safety net.
        let mut cached_term_hwnd: HWND = term_hwnd;
        let mut cached_bridge: Option<(HWND, i32)> = None; // (bridge_hwnd, padding)
        let mut term_cache_at = std::time::Instant::now();
        const TERM_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(3);

        // Last successfully-applied state, so unchanged frames are a no-op.
        // NOTE: we key on (path, rotation, flip_h) rather than the Arc's
        // pointer — the source cache holds only one entry at a time, so a
        // freed image's allocation can be immediately reused by the next
        // one. That's especially likely for office-doc thumbnails (PDF/DOCX/
        // PPTX), which are all near-identically-sized generated PNGs — using
        // pointer identity there caused the overlay to wrongly think a
        // brand-new preview was "the same image" and skip repainting.
        let mut last_key: Option<(std::path::PathBuf, u32, bool, u32, u32)> = None;
        let mut last_cell_rect: TuiRect = TuiRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };
        let mut last_term_cols: u16 = 0;
        let mut last_term_rows: u16 = 0;
        let mut last_applied_rect: (i32, i32, i32, i32) = (i32::MIN, 0, 0, 0);
        let mut last_shown: Option<(Arc<DynamicImage>, f32, TuiRect, u16, u16)> = None;
        let mut is_visible = false;
        let mut ultra_fast = false;
        let mut last_resync_at = std::time::Instant::now();
        const RESYNC_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);

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

            // Coalesce incoming commands — only the newest matters.
            let mut latest_cmd = None;
            while let Ok(cmd) = rx.try_recv() {
                latest_cmd = Some(cmd);
            }

            let due_for_resync = is_visible && last_resync_at.elapsed() >= RESYNC_INTERVAL;

            if latest_cmd.is_none() && due_for_resync {
                // Nothing new to show, but periodically re-check the terminal's
                // on-screen position so the overlay follows a dragged/moved
                // window. This is position-only: no image decode, no resize.
                if let Some((img, zoom, cell_rect, cols, rows)) = last_shown.clone() {
                    reposition_overlay(
                        hwnd,
                        &img,
                        zoom,
                        cell_rect,
                        cols,
                        rows,
                        &mut cached_term_hwnd,
                        &mut cached_bridge,
                        &mut term_cache_at,
                        TERM_CACHE_TTL,
                        &mut last_applied_rect,
                        state_ptr,
                        false,
                    );
                }
                last_resync_at = std::time::Instant::now();
            }

            if let Some(cmd) = latest_cmd {
                match cmd {
                    PreviewCmd::ShowImage {
                        generation,
                        img,
                        path,
                        rotation,
                        flip_h,
                        zoom,
                        cell_rect,
                        term_cols,
                        term_rows,
                        background,
                        ultra_fast: requested_ultra,
                    } => {
                        if generation < newest_generation.load(Ordering::Acquire) {
                            continue;
                        }
                        ultra_fast = requested_ultra;
                        let zoom = zoom.clamp(0.1, 8.0);
                        let key = (path, rotation, flip_h, zoom.to_bits(), background);
                        let unchanged = Some(&key) == last_key.as_ref()
                            && cell_rect == last_cell_rect
                            && term_cols == last_term_cols
                            && term_rows == last_term_rows
                            && is_visible;

                        if !unchanged {
                            // Hide the previous bitmap before doing any resize
                            // work so it can never linger over a newer file.
                            ShowWindow(hwnd, SW_HIDE);
                            is_visible = false;
                            if let Ok(mut state) = (*state_ptr).lock() {
                                state.background = background;
                            }
                            last_cell_rect = cell_rect;
                            last_term_cols = term_cols;
                            last_term_rows = term_rows;

                            reposition_overlay(
                                hwnd,
                                &img,
                                zoom,
                                cell_rect,
                                term_cols,
                                term_rows,
                                &mut cached_term_hwnd,
                                &mut cached_bridge,
                                &mut term_cache_at,
                                TERM_CACHE_TTL,
                                &mut last_applied_rect,
                                state_ptr,
                                true,
                            );
                            if generation == newest_generation.load(Ordering::Acquire) {
                                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                                is_visible = true;
                                last_shown = Some((img, zoom, cell_rect, term_cols, term_rows));
                                last_key = Some(key);
                                last_resync_at = std::time::Instant::now();
                            }
                        }
                    }
                    PreviewCmd::Hide { generation } => {
                        if generation < newest_generation.load(Ordering::Acquire) {
                            continue;
                        }
                        ultra_fast = false;
                        if is_visible {
                            ShowWindow(hwnd, SW_HIDE);
                            is_visible = false;
                            last_shown = None;
                            last_key = None;
                            last_applied_rect = (i32::MIN, 0, 0, 0);
                        }
                    }
                    PreviewCmd::Quit => {
                        let _ = Box::from_raw(state_ptr);
                        DestroyWindow(hwnd);
                        UnregisterClassW(class_name.as_ptr(), std::ptr::null_mut());
                        return;
                    }
                }
            }
            if ultra_fast {
                thread::yield_now();
            } else {
                thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }
}

/// Resolve (with caching) where the overlay should sit on screen and apply
/// it. `allow_resize` gates the expensive Lanczos3 decode/resize + repaint;
/// periodic resyncs pass `false` and only move the window if its position
/// actually changed.
#[allow(clippy::too_many_arguments)]
unsafe fn reposition_overlay(
    hwnd: HWND,
    img: &Arc<DynamicImage>,
    zoom: f32,
    cell_rect: TuiRect,
    term_cols: u16,
    term_rows: u16,
    cached_term_hwnd: &mut HWND,
    cached_bridge: &mut Option<(HWND, i32)>,
    term_cache_at: &mut std::time::Instant,
    term_cache_ttl: std::time::Duration,
    last_applied_rect: &mut (i32, i32, i32, i32),
    state_ptr: *mut Mutex<WindowState>,
    allow_resize: bool,
) {
    if term_cols == 0 || term_rows == 0 {
        return;
    }

    // Re-resolve the terminal/bridge window only occasionally, or if the
    // cached handle has gone stale — never on every tick.
    let stale = IsWindow(*cached_term_hwnd) == 0 || term_cache_at.elapsed() > term_cache_ttl;
    if stale || cached_bridge.is_none() {
        let resolved = find_terminal_window();
        if !resolved.is_null() {
            *cached_term_hwnd = resolved;
        }
        *term_cache_at = std::time::Instant::now();

        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(*cached_term_hwnd, class_buf.as_mut_ptr(), 256);
        let term_class = String::from_utf16_lossy(&class_buf[..class_len as usize]);

        *cached_bridge = Some(if term_class == "CASCADIA_HOSTING_WINDOW_CLASS" {
            let bridge = find_child_window_by_class(
                *cached_term_hwnd,
                "Windows.UI.Composition.DesktopWindowContentBridge",
            );
            if !bridge.is_null() {
                let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(*cached_term_hwnd);
                let pad = (8.0f32 * (dpi as f32) / 96.0f32).round() as i32;
                (bridge, pad)
            } else {
                (*cached_term_hwnd, 0)
            }
        } else {
            (*cached_term_hwnd, 0)
        });
    }

    let Some((bridge_hwnd, padding)) = *cached_bridge else {
        return;
    };

    let mut client_rect: RECT = std::mem::zeroed();
    GetClientRect(bridge_hwnd, &mut client_rect);

    let width = client_rect.right - client_rect.left;
    let height = client_rect.bottom - client_rect.top;

    let grid_width = (width - 2 * padding).max(1);
    let grid_height = (height - 2 * padding).max(1);

    let cw = (grid_width / term_cols as i32).max(1);
    let ch = (grid_height / term_rows as i32).max(1);

    let mut pt = POINT { x: 0, y: 0 };
    ClientToScreen(bridge_hwnd, &mut pt);

    let px = pt.x + padding + cell_rect.x as i32 * cw;
    let py = pt.y + padding + cell_rect.y as i32 * ch;
    let pw = cell_rect.width as i32 * cw;
    let ph = cell_rect.height as i32 * ch;

    let rect_changed = (px, py, pw, ph) != *last_applied_rect;

    if !allow_resize {
        // Periodic position-only resync: move the window if it drifted
        // (e.g. the terminal was dragged), but never touch image content.
        if rect_changed {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                px,
                py,
                pw,
                ph,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
            *last_applied_rect = (px, py, pw, ph);
        }
        return;
    }

    if rect_changed {
        SetWindowPos(hwnd, std::ptr::null_mut(), px, py, pw, ph, SWP_NOACTIVATE);
        *last_applied_rect = (px, py, pw, ph);
    } else {
        // The caller shows the window only after confirming that this render
        // still belongs to the newest preview generation.
    }

    // Fit at 100%, then apply user zoom. The overlay window remains clipped
    // to the preview pane, so zooming never paints over navigation/UI chrome.
    let Some((fw, fh)) = fitted_zoom_dimensions(img.width(), img.height(), pw, ph, zoom) else {
        return;
    };

    // Decode/resize outside of WM_PAINT — this is the expensive step
    // (Lanczos3), so it only ever runs when the image or its target size
    // actually changed, not on every redraw tick.
    let resized = img.resize_exact(fw, fh, image::imageops::FilterType::Lanczos3);
    let bgra: Vec<u8> = resized
        .to_rgba8()
        .pixels()
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();

    if let Ok(mut state) = (*state_ptr).lock() {
        state.img_bgra = bgra;
        state.img_w = fw;
        state.img_h = fh;
    }

    InvalidateRect(hwnd, std::ptr::null(), 0);
}

fn fitted_zoom_dimensions(
    image_width: u32,
    image_height: u32,
    pane_width: i32,
    pane_height: i32,
    zoom: f32,
) -> Option<(u32, u32)> {
    if image_width == 0 || image_height == 0 || pane_width <= 0 || pane_height <= 0 {
        return None;
    }
    let image_width = image_width as f32;
    let image_height = image_height as f32;
    let fit_scale = (pane_width as f32 / image_width).min(pane_height as f32 / image_height);
    let requested_scale = fit_scale * zoom.clamp(0.1, 8.0);
    // Bound the decoded bitmap even at 800% so a large monitor cannot trigger
    // a hundreds-of-megabytes transient allocation while rapidly zooming.
    const MAX_RENDER_DIM: f32 = 4096.0;
    let scale = requested_scale
        .min(MAX_RENDER_DIM / image_width)
        .min(MAX_RENDER_DIM / image_height);
    Some((
        ((image_width * scale).round() as u32).max(1),
        ((image_height * scale).round() as u32).max(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::fitted_zoom_dimensions;

    #[test]
    fn native_preview_zoom_fits_scales_and_caps_allocations() {
        assert_eq!(
            fitted_zoom_dimensions(400, 200, 200, 200, 1.0),
            Some((200, 100))
        );
        assert_eq!(
            fitted_zoom_dimensions(400, 200, 200, 200, 2.0),
            Some((400, 200))
        );
        let huge_zoom = fitted_zoom_dimensions(10, 10, 2_000, 2_000, 8.0).unwrap();
        assert_eq!(huge_zoom, (4096, 4096));
        assert_eq!(fitted_zoom_dimensions(0, 200, 200, 200, 1.0), None);
    }
}
