//! The terminal layer — the only part that touches a real TTY: raw mode, the alternate screen, and the
//! crossterm event loop. Deliberately thin. All decisions live in [`crate::app`] (state) and
//! [`crate::ui`] (render), which are terminal-free and unit-tested; this module only wires crossterm
//! events onto [`App::on_key`] and draws each tick, and it restores the terminal even on panic.

use std::io::{self, Stdout};
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use tessera_core::Result;

use crate::app::{App, Key};
use crate::config::Layout;
use crate::ui;

/// Open `path` under `layout` and run the interactive explorer until the user quits. Sets up raw mode
/// + the alternate screen, restores them on exit (and on panic, via [`TerminalGuard`]).
pub fn run(path: &Path, layout: Layout) -> Result<()> {
    let app = App::open(path, layout)?;
    let mut guard = TerminalGuard::enter()?;
    let res = event_loop(&mut guard.terminal, app);
    guard.restore()?; // explicit restore so a clean exit isn't relying on drop ordering
    res
}

/// The draw/read loop: render the frame, block for the next key, feed it to the app, repeat until
/// [`App::should_quit`].
fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, &app))?;
        if let Event::Key(key) = event::read()? {
            // Ignore key-release / repeat events (Windows emits both); act only on press.
            if key.kind == KeyEventKind::Press {
                app.on_key(map_key(key.code));
                // Load Data-mode block data / the Verify verdict after input (the I/O the pure on_key
                // deliberately omits — both are lazy + cached).
                app.sync_data();
                app.sync_verify();
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

/// Map a crossterm [`KeyCode`] onto the terminal-independent [`Key`] the app understands.
fn map_key(code: KeyCode) -> Key {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => Key::Quit,
        KeyCode::Down | KeyCode::Char('j') => Key::Down,
        KeyCode::Up | KeyCode::Char('k') => Key::Up,
        KeyCode::Right | KeyCode::Char('l') => Key::Expand,
        KeyCode::Left | KeyCode::Char('h') => Key::Collapse,
        KeyCode::Enter | KeyCode::Char(' ') => Key::Enter,
        KeyCode::Tab => Key::NextMode,
        KeyCode::Char('m') => Key::ToggleImage,
        KeyCode::Char(c @ '1'..='9') => Key::Mode(c as u8 - b'0'),
        _ => Key::Other,
    }
}

/// RAII guard that owns the terminal in raw mode + the alternate screen and restores it on drop —
/// so a panic in the render/event loop never leaves the user's terminal wedged.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl TerminalGuard {
    /// Enter raw mode + the alternate screen and build the ratatui terminal.
    fn enter() -> Result<TerminalGuard> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(TerminalGuard {
            terminal,
            active: true,
        })
    }

    /// Restore the terminal (idempotent — safe to call explicitly and again on drop).
    fn restore(&mut self) -> Result<()> {
        if self.active {
            self.active = false;
            disable_raw_mode()?;
            execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
            self.terminal.show_cursor()?;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Best-effort restore on panic/early return; ignore errors during unwind.
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_mapping_covers_navigation_modes_and_quit() {
        assert_eq!(map_key(KeyCode::Char('q')), Key::Quit);
        assert_eq!(map_key(KeyCode::Esc), Key::Quit);
        assert_eq!(map_key(KeyCode::Char('j')), Key::Down);
        assert_eq!(map_key(KeyCode::Down), Key::Down);
        assert_eq!(map_key(KeyCode::Char('k')), Key::Up);
        assert_eq!(map_key(KeyCode::Char('l')), Key::Expand);
        assert_eq!(map_key(KeyCode::Char('h')), Key::Collapse);
        assert_eq!(map_key(KeyCode::Enter), Key::Enter);
        assert_eq!(map_key(KeyCode::Tab), Key::NextMode);
        assert_eq!(map_key(KeyCode::Char('3')), Key::Mode(3));
        assert_eq!(map_key(KeyCode::Char('z')), Key::Other);
    }
}
