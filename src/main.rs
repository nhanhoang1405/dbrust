mod app;
mod connection;
mod db;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyEventKind,
    },
    execute,
};
use std::{io, time::Duration};

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
    }
}
#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "dbrust — terminal SQL workbench\n\nUsage: cargo run --release\n\nConnect to PostgreSQL, MySQL, or SQLite using the startup form.\nF1 help · F5 execute · F2 connections · Ctrl+Q quit"
        );
        return Ok(());
    }
    let history = connection::History::load()?;
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let mut app = app::App::new(history);
    while !app.should_quit() {
        app.poll();
        terminal.draw(|frame| app.draw(frame))?;
        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Paste(value) => app.paste(&value),
                _ => {}
            }
        }
    }
    Ok(())
}
