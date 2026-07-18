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

