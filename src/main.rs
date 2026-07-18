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

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        
        let handle = windows_sys::Win32::System::Console::GetStdHandle(windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE);
        if handle != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE && handle != std::ptr::null_mut() {
            let mut font_info: windows_sys::Win32::System::Console::CONSOLE_FONT_INFOEX = std::mem::zeroed();
            font_info.cbSize = std::mem::size_of::<windows_sys::Win32::System::Console::CONSOLE_FONT_INFOEX>() as u32;
            font_info.nFont = 0;
            font_info.dwFontSize.X = 0;
            font_info.dwFontSize.Y = 14; // 14px height (~10.5pt)
            font_info.FontFamily = 54; // FF_DONTCARE | TMPF_TRUETYPE
            font_info.FontWeight = 400; // FW_NORMAL
            
            let name = "Segoe UI Variable Text\0".encode_utf16().collect::<Vec<_>>();
            let len = name.len().min(32);
            for i in 0..len {
                font_info.FaceName[i] = name[i];
            }
            
            windows_sys::Win32::System::Console::SetCurrentConsoleFontEx(handle, 0, &font_info);
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
        terminal.draw(|f| ui::draw(f, &app))?;

        if app.handle_events()? {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    overlay::shutdown();
    Ok(())
}

