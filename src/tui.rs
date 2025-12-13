use crate::{engine::Engine, ui};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;

pub struct Tui {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    engine: Engine,
    app: ui::App,
}

impl Tui {
    pub fn new(engine: Engine) -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;

        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        let mut tui = Self {
            terminal,
            engine,
            app: ui::App::new(),
        };

        let initial = tui.engine.initial_state()?;
        tui.app.update_data(initial);

        Ok(tui)
    }

    pub fn run(&mut self) -> Result<()> {
        loop {
            self.terminal.draw(|f| ui::draw(f, &mut self.app))?;

            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(k) = event::read()? {
                    match k.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('n') => self.app.next_sort(),
                        KeyCode::Char('v') => self.app.toggle_view(),
                        KeyCode::Up => self.app.scroll_up(),
                        KeyCode::Down => self.app.scroll_down(),
                        KeyCode::PageUp => self.app.page_up(),
                        KeyCode::PageDown => self.app.page_down(),
                        _ => {}
                    }
                }
            }

            if self.engine.should_tick() {
                let state = self.engine.tick()?;
                self.app.update_data(state);
            }
        }
    }
}

// REVIEW Maybe consider to log errors instead of printing to stderr
//        And refactor it to be an standalone method instead of Drop-impl.
// 
impl Drop for Tui {
    fn drop(&mut self) {
        if let Err(err) = disable_raw_mode() {
            eprintln!("Failed to disable raw mode: {:?}", err);
        }

        if let Err(err) = execute!(self.terminal.backend_mut(), LeaveAlternateScreen) {
            eprintln!("Failed to leave alternate screen: {:?}", err);
        }

        if let Err(err) = self.terminal.show_cursor() {
            eprintln!("Failed to show cursor: {:?}", err);
        }
    }
}
