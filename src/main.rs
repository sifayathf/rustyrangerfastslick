// ================= src/main.rs =================
use std::io;
use crossterm::{
    execute,
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    event::{EnableMouseCapture, DisableMouseCapture},
};
use ratatui::{Terminal, backend::CrosstermBackend};

mod state;
mod ui;
mod preview;
mod native;
mod overlay;
use state::AppState;

/// Profile name used in Windows Terminal settings.json
const WT_PROFILE_NAME: &str = "Rusty Ranger";
/// GUID for the dedicated Windows Terminal profile (stable, deterministic)
const WT_PROFILE_GUID: &str = "{a7b3c4d5-e6f7-4a8b-9c0d-1e2f3a4b5c6d}";
/// Desired font face for the file manager
// Match the original rounded-control implementation. Windows Terminal's
// Cascadia Code profile supplies the full-cell Powerline separator geometry
// used for pill caps while keeping the normal text grid monospace.
const FONT_FACE: &str = "Cascadia Code";
/// Desired font size (pt) — 9pt gives a compact, Windows-Explorer-like look
const FONT_SIZE: u32 = 9;

/// Detect Windows Terminal and relaunch in a dedicated profile with the right font.
/// Returns `true` if we relaunched (caller should exit), `false` to continue normally.
#[cfg(windows)]
fn try_relaunch_in_wt_profile() -> bool {
    use std::path::PathBuf;

    // Relaunch once into the compact dedicated profile. Users can still use
    // Ctrl+Plus in Windows Terminal when they prefer a larger layout.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--launched") {
        return false;
    }

    // Note: we deliberately do NOT gate this on WT_SESSION being set.
    // Launching by double-clicking the exe (or a shortcut) spawns a bare
    // conhost.exe with no WT_SESSION — that's the common case, and it's
    // exactly when we most need to relaunch into Windows Terminal, since
    // legacy conhost has much weaker Unicode/glyph coverage than WT with a
    // proper TrueType font. We still relaunch even if already inside WT, to
    // force our specific profile/font rather than trusting whatever the
    // user's current WT profile happens to use.

    // Find Windows Terminal settings.json
    let local_app_data = match std::env::var("LOCALAPPDATA") {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Try both Store and unpackaged paths
    let settings_paths = [
        PathBuf::from(&local_app_data)
            .join("Packages")
            .join("Microsoft.WindowsTerminal_8wekyb3d8bbwe")
            .join("LocalState")
            .join("settings.json"),
        PathBuf::from(&local_app_data)
            .join("Microsoft")
            .join("Windows Terminal")
            .join("settings.json"),
    ];

    let settings_path = match settings_paths.iter().find(|p| p.exists()) {
        Some(p) => p.clone(),
        None => return false,
    };

    // Read and parse settings.json
    let content = match std::fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let mut settings: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Get the exe path for the profile commandline
    let exe_path = match std::env::current_exe() {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return false,
    };

    // Build the profile JSON
    let profile = serde_json::json!({
        "guid": WT_PROFILE_GUID,
        "name": WT_PROFILE_NAME,
        "commandline": format!("\"{}\" --launched", exe_path),
        "hidden": false,
        "font": {
            "face": FONT_FACE,
            "size": FONT_SIZE
        },
        "colorScheme": "One Half Dark",
        "cursorShape": "filledBox",
        "padding": "2",
        "scrollbarState": "hidden"
    });

    // Insert or update the profile in profiles.list
    let needs_write;
    if let Some(profiles) = settings.get_mut("profiles") {
        if let Some(list) = profiles.get_mut("list") {
            if let Some(arr) = list.as_array_mut() {
                // Check if profile already exists
                if let Some(existing) = arr.iter_mut().find(|p| {
                    p.get("guid").and_then(|g| g.as_str()) == Some(WT_PROFILE_GUID)
                }) {
                    // Update existing profile's commandline and font
                    *existing = profile;
                    needs_write = true;
                } else {
                    arr.push(profile);
                    needs_write = true;
                }
            } else {
                return false;
            }
        } else {
            return false;
        }
    } else {
        return false;
    }

    if needs_write {
        // Write back settings.json
        let formatted = match serde_json::to_string_pretty(&settings) {
            Ok(s) => s,
            Err(_) => return false,
        };
        if std::fs::write(&settings_path, formatted).is_err() {
            return false;
        }
    }

    // Relaunch: open a new Windows Terminal window with our profile.
    // Only tell the caller to exit if this actually launched — otherwise
    // we'd vanish with no window at all if wt.exe isn't on PATH.
    match std::process::Command::new("wt.exe")
        .args(["-p", WT_PROFILE_NAME])
        .spawn()
    {
        Ok(_) => true,
        Err(_) => false,
    }
}

fn main() -> anyhow::Result<()> {
    // Install panic hook to ensure terminal state is always restored
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default_hook(info);
    }));

    // Try to relaunch in a dedicated Windows Terminal profile with the right font
    #[cfg(windows)]
    {
        if try_relaunch_in_wt_profile() {
            // We've spawned a new WT window with the right profile; exit this instance
            std::process::exit(0);
        }
    }

    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );

        // Fallback for legacy conhost.exe: set console font directly
        if std::env::var("WT_SESSION").is_err() {
            let handle = windows_sys::Win32::System::Console::GetStdHandle(
                windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE,
            );
            if handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
                && handle != std::ptr::null_mut()
            {
                let mut font_info: windows_sys::Win32::System::Console::CONSOLE_FONT_INFOEX =
                    std::mem::zeroed();
                font_info.cbSize = std::mem::size_of::<
                    windows_sys::Win32::System::Console::CONSOLE_FONT_INFOEX,
                >() as u32;
                font_info.nFont = 0;
                font_info.dwFontSize.X = 0;
                font_info.dwFontSize.Y = 12; // compact fallback matching the 9pt WT profile
                font_info.FontFamily = 54; // FF_DONTCARE | TMPF_TRUETYPE
                font_info.FontWeight = 400; // FW_NORMAL

                let name = "Consolas\0".encode_utf16().collect::<Vec<_>>();
                let len = name.len().min(32);
                for i in 0..len {
                    font_info.FaceName[i] = name[i];
                }

                windows_sys::Win32::System::Console::SetCurrentConsoleFontEx(
                    handle,
                    0,
                    &font_info,
                );
            }
        }
    }

    std::env::set_var("COLORTERM", "truecolor");
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = AppState::new()?;

    loop {
        app.maybe_refresh_drives();
        terminal.draw(|f| ui::draw(f, &app))?;

        if app.handle_events()? {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    overlay::shutdown();
    Ok(())
}

