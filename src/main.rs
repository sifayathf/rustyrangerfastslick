// ================= src/main.rs =================
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

mod native;
mod overlay;
mod preview;
mod settings;
mod state;
mod ui;
use state::AppState;

/// Profile name used in Windows Terminal settings.json
const WT_PROFILE_NAME: &str = "Rusty Ranger";
/// GUID for the dedicated Windows Terminal profile (stable, deterministic)
const WT_PROFILE_GUID: &str = "{a7b3c4d5-e6f7-4a8b-9c0d-1e2f3a4b5c6d}";
// Match the original rounded-control implementation. Windows Terminal's
// Cascadia Code profile supplies the full-cell Powerline separator geometry
// used for pill caps while keeping the normal text grid monospace.
/// Detect Windows Terminal and relaunch in a dedicated profile with the right font.
/// Returns `true` if we relaunched (caller should exit), `false` to continue normally.
#[cfg(windows)]
fn try_relaunch_in_wt_profile(user_settings: &settings::UserSettings) -> bool {
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
            "face": user_settings.font_face.as_str(),
            "size": user_settings.font_size,
            "weight": user_settings.font_weight
        },
        "colorScheme": "One Half Dark",
        "cursorShape": "filledBox",
        "padding": "2",
        "scrollbarState": "hidden"
    });

    // Insert or update only the fields owned by Rusty Ranger. Recursively
    // merging keeps any profile customizations the user added themselves.
    let needs_write;
    if let Some(profiles) = settings.get_mut("profiles") {
        if let Some(list) = profiles.get_mut("list") {
            if let Some(arr) = list.as_array_mut() {
                // Check if profile already exists
                if let Some(existing) = arr
                    .iter_mut()
                    .find(|p| p.get("guid").and_then(|g| g.as_str()) == Some(WT_PROFILE_GUID))
                {
                    let before = existing.clone();
                    merge_json(existing, profile);
                    needs_write = *existing != before;
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
        // Keep a one-time recovery copy and atomically publish the update.
        // A crash can therefore leave either the old or new valid JSON, never
        // a partially-written Windows Terminal configuration.
        let formatted = match serde_json::to_string_pretty(&settings) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let backup = settings_path.with_extension("json.rusty-ranger.bak");
        if !backup.exists() && std::fs::copy(&settings_path, &backup).is_err() {
            return false;
        }
        if settings::write_atomic(&settings_path, formatted.as_bytes()).is_err() {
            return false;
        }
    }

    // Relaunch: open a new Windows Terminal window with our profile.
    // Only tell the caller to exit if this actually launched — otherwise
    // we'd vanish with no window at all if wt.exe isn't on PATH.
    std::process::Command::new("wt.exe")
        .args(["-p", WT_PROFILE_NAME])
        .spawn()
        .is_ok()
}

#[cfg(windows)]
fn merge_json(destination: &mut serde_json::Value, source: serde_json::Value) {
    let serde_json::Value::Object(source) = source else {
        *destination = source;
        return;
    };
    if !destination.is_object() {
        *destination = serde_json::Value::Object(serde_json::Map::new());
    }
    let destination = destination.as_object_mut().expect("object initialized");
    for (key, source_value) in source {
        if let Some(destination_value) = destination.get_mut(&key) {
            if destination_value.is_object() && source_value.is_object() {
                merge_json(destination_value, source_value);
                continue;
            }
        }
        destination.insert(key, source_value);
    }
}

fn main() -> anyhow::Result<()> {
    preview::cleanup_render_cache();
    // Install panic hook to ensure terminal state is always restored
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default_hook(info);
    }));

    let user_settings = settings::load();

    // Try to relaunch in a dedicated Windows Terminal profile with the right font
    #[cfg(windows)]
    {
        if try_relaunch_in_wt_profile(&user_settings) {
            // We've spawned a new WT window with the right profile; exit this instance
            std::process::exit(0);
        }
    }

    #[cfg(windows)]
    unsafe {
        // Keep the console transport in UTF-8 even when Windows launches the
        // executable through legacy conhost. This prevents Tamil and other
        // complex-script text from being converted through an ANSI code page
        // before Windows Terminal/font fallback can shape it.
        windows_sys::Win32::System::Console::SetConsoleCP(65001);
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);

        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );

        // Fallback for legacy conhost.exe: set console font directly
        if std::env::var("WT_SESSION").is_err() {
            let handle = windows_sys::Win32::System::Console::GetStdHandle(
                windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE,
            );
            if handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE && !handle.is_null() {
                let mut font_info: windows_sys::Win32::System::Console::CONSOLE_FONT_INFOEX =
                    std::mem::zeroed();
                font_info.cbSize = std::mem::size_of::<
                    windows_sys::Win32::System::Console::CONSOLE_FONT_INFOEX,
                >() as u32;
                font_info.nFont = 0;
                font_info.dwFontSize.X = 0;
                font_info.dwFontSize.Y = ((user_settings.font_size as i16) + 3).clamp(11, 19);
                font_info.FontFamily = 54; // FF_DONTCARE | TMPF_TRUETYPE
                font_info.FontWeight = user_settings.font_weight as u32;

                let name = format!("{}\0", user_settings.font_face)
                    .encode_utf16()
                    .collect::<Vec<_>>();
                let len = name.len().min(32);
                font_info.FaceName[..len].copy_from_slice(&name[..len]);

                windows_sys::Win32::System::Console::SetCurrentConsoleFontEx(handle, 0, &font_info);
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
    let mut drawn_revision = u64::MAX;
    loop {
        app.tick_background();
        let revision = app.event_revision();
        if revision != drawn_revision {
            terminal.draw(|f| ui::draw(f, &app))?;
            drawn_revision = revision;
        }

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

#[cfg(all(test, windows))]
mod tests {
    use super::merge_json;

    #[test]
    fn terminal_profile_merge_preserves_user_fields() {
        let mut destination = serde_json::json!({
            "guid": "old",
            "opacity": 73,
            "font": { "face": "Old", "features": { "liga": 0 } }
        });
        merge_json(
            &mut destination,
            serde_json::json!({
                "guid": "new",
                "font": { "face": "Cascadia Code", "size": 9 }
            }),
        );
        assert_eq!(destination["guid"], "new");
        assert_eq!(destination["opacity"], 73);
        assert_eq!(destination["font"]["face"], "Cascadia Code");
        assert_eq!(destination["font"]["features"]["liga"], 0);
    }
}
