use crate::{engine::Engine, ui};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::Duration;

pub struct Tui {
    terminal: DefaultTerminal,
    engine: Engine,
    app: ui::App,
}

impl Tui {
    pub fn new(engine: Engine) -> Result<Self> {
        let mut app = ui::App::new();
        if let Some(initial) = engine.try_recv() {
            app.update_data(initial);
        }

        let terminal = ratatui::init();

        Ok(Self {
            terminal,
            engine,
            app,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        let mut needs_redraw = true;

        loop {
            if let Some(state) = self.engine.try_recv() {
                self.app.update_data(state);
                needs_redraw = true;
            }

            if needs_redraw {
                self.terminal.draw(|f| ui::draw(f, &mut self.app))?;
                needs_redraw = false;
            }

            if event::poll(Duration::from_millis(16))? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            if k.code == KeyCode::Char('c')
                                && k.modifiers.contains(KeyModifiers::CONTROL)
                            {
                                return Ok(());
                            }

                            if self.app.is_searching() {
                                match k.code {
                                    KeyCode::Esc => self.app.cancel_search(),
                                    KeyCode::Enter => self.app.finish_search(),
                                    KeyCode::Backspace => self.app.pop_search_char(),
                                    KeyCode::Up => self.app.scroll_up(),
                                    KeyCode::Down => self.app.scroll_down(),
                                    KeyCode::PageUp => self.app.page_up(),
                                    KeyCode::PageDown => self.app.page_down(),
                                    KeyCode::Char(c)
                                        if !k.modifiers.contains(KeyModifiers::ALT) =>
                                    {
                                        self.app.push_search_char(c);
                                    }
                                    _ => {}
                                }
                            } else {
                                match k.code {
                                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                                    KeyCode::Esc => self.app.clear_search(),
                                    KeyCode::Char('/') => self.app.start_search(),
                                    KeyCode::Char('g') | KeyCode::Char('G') => {
                                        self.app.toggle_group();
                                    }
                                    KeyCode::Char('k') | KeyCode::Char('K') => {
                                        let pids = self.app.selected_pids();
                                        self.engine.kill_pids(pids);
                                    }
                                    KeyCode::Char('n') | KeyCode::Char('N') => {
                                        self.app.next_sort();
                                    }
                                    KeyCode::Char('v') | KeyCode::Char('V') => {
                                        self.app.toggle_view();
                                    }
                                    KeyCode::Up => self.app.scroll_up(),
                                    KeyCode::Down => self.app.scroll_down(),
                                    KeyCode::PageUp => self.app.page_up(),
                                    KeyCode::PageDown => self.app.page_down(),
                                    _ => {}
                                }
                            }
                            needs_redraw = true;
                        }
                        Event::Resize(_, _) => {
                            needs_redraw = true;
                        }
                        _ => {}
                    }

                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
