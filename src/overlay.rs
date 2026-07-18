// ================= src/overlay.rs =================
// On Unix/Linux: use ueberzugpp for pixel-perfect image overlays in X11/Wayland terminals.
// On Windows: this module is a no-op — images are rendered inline via viuer in preview.rs.

#[cfg(unix)]
use once_cell::sync::Lazy;
#[cfg(unix)]
use parking_lot::Mutex;
#[cfg(unix)]
use std::{
    io::Write,
    process::{Child, Command, Stdio},
};

#[cfg(unix)]
static OVERLAY: Lazy<Mutex<Option<Child>>> = Lazy::new(|| Mutex::new(None));

#[cfg(unix)]
pub fn start() {
    let mut guard = OVERLAY.lock();
    if guard.is_some() {
        return;
    }
    if let Ok(child) = Command::new("ueberzugpp")
        .args(["layer", "--parser", "json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        *guard = Some(child);
    }
}

#[cfg(unix)]
pub fn show_image(path: &str, x: u16, y: u16, w: u16, h: u16) {
    start();
    if let Some(child) = OVERLAY.lock().as_mut() {
        if let Some(stdin) = child.stdin.as_mut() {
            let cmd = format!(
                "{{\"action\":\"add\",\"identifier\":\"preview\",\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"path\":\"{}\"}}\\n",
                x, y, w, h, path
            );
            let _ = stdin.write_all(cmd.as_bytes());
            let _ = stdin.flush();
        }
    }
}

#[cfg(unix)]
pub fn clear() {
    if let Some(child) = OVERLAY.lock().as_mut() {
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(b"{\"action\":\"remove\",\"identifier\":\"preview\"}\n");
            let _ = stdin.flush();
        }
    }
}

#[cfg(unix)]
pub fn shutdown() {
    if let Some(mut child) = OVERLAY.lock().take() {
        let _ = child.kill();
    }
}

// ── Windows stubs ────────────────────────────────────────────────────────────

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn start() {}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn show_image(_path: &str, _x: u16, _y: u16, _w: u16, _h: u16) {}

#[cfg(not(unix))]
#[allow(dead_code)]
pub fn clear() {}

#[cfg(not(unix))]
pub fn shutdown() {}
