use crate::{engine::Engine, state::App, ui};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
};
use ratatui::DefaultTerminal;
use std::io;
use std::time::Duration;

pub struct Tui {
    terminal: DefaultTerminal,
    engine: Engine,
    app: App,
}

impl Tui {
    pub fn new(engine: Engine) -> Result<Self> {
        let mut app = App::new();
        if let Some(initial) = engine.try_recv() {
            app.update_data(initial);
        }
        let terminal = ratatui::init();
        let _ = execute!(io::stdout(), EnableMouseCapture);
        Ok(Self {
            terminal,
            engine,
            app,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        let mut redraw = true;
        loop {
            if let Some(state) = self.engine.try_recv() {
                self.app.update_data(state);
                if self.app.inspecting {
                    self.engine.inspect_pids(self.app.selected_pids());
                }
                redraw = true;
            }
            if let Some(breakdown) = self.engine.try_recv_inspect() {
                self.app.inspector_breakdown = Some(breakdown);
                redraw = true;
            }
            if redraw {
                self.terminal.draw(|f| ui::draw(f, &mut self.app))?;
                redraw = false;
            }
            if event::poll(Duration::from_millis(16))? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            if self.handle_key(k) {
                                return Ok(());
                            }
                            redraw = true;
                        }
                        Event::Mouse(m) => {
                            if self.handle_mouse(m) {
                                redraw = true;
                            }
                        }
                        Event::Resize(_, _) => redraw = true,
                        _ => {}
                    }
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, k: KeyEvent) -> bool {
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        let page = self.app.visible_rows.max(1) as isize;
        if self.app.searching {
            match k.code {
                KeyCode::Esc => {
                    self.app.searching = false;
                    self.app.search_query.clear();
                    self.app.selected_index = 0;
                    self.app.rebuild_display_rows();
                }
                KeyCode::Enter => self.app.searching = false,
                KeyCode::Backspace => {
                    self.app.search_query.pop();
                    self.app.selected_index = 0;
                    self.app.rebuild_display_rows();
                }
                KeyCode::Up => self.app.move_selection(-1),
                KeyCode::Down => self.app.move_selection(1),
                KeyCode::PageUp => self.app.move_selection(-page),
                KeyCode::PageDown => self.app.move_selection(page),
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::ALT) => {
                    self.app.search_query.push(c);
                    self.app.selected_index = 0;
                    self.app.rebuild_display_rows();
                }
                _ => {}
            }
            return false;
        }

        match k.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return true,
            KeyCode::Esc => {
                if self.app.inspecting {
                    self.app.close_inspector();
                    self.engine.inspect_pids(Vec::new());
                } else if !self.app.search_query.is_empty() {
                    self.app.search_query.clear();
                    self.app.selected_index = 0;
                    self.app.rebuild_display_rows();
                }
            }
            KeyCode::Enter | KeyCode::Char('i') | KeyCode::Char('I') => {
                if self.app.inspecting {
                    self.app.close_inspector();
                    self.engine.inspect_pids(Vec::new());
                } else {
                    self.app.open_inspector();
                    self.sync_inspector();
                }
            }
            KeyCode::Char('/') if !self.app.inspecting => {
                self.app.view_mode = crate::state::ViewMode::Processes;
                self.app.searching = true;
            }
            KeyCode::Char('g') | KeyCode::Char('G') if !self.app.inspecting => {
                self.app.grouped = !self.app.grouped;
                self.app.selected_index = 0;
                self.app.rebuild_display_rows();
            }
            KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Char(' ') => {
                self.app.toggle_baseline()
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                self.engine.kill_pids(self.app.selected_pids())
            }
            KeyCode::Char('n') | KeyCode::Char('N') if !self.app.inspecting => self.app.next_sort(),
            KeyCode::Char('v') | KeyCode::Char('V') if !self.app.inspecting => {
                self.app.toggle_view()
            }
            KeyCode::Up => {
                self.app.move_selection(-1);
                self.sync_inspector();
            }
            KeyCode::Down => {
                self.app.move_selection(1);
                self.sync_inspector();
            }
            KeyCode::PageUp if !self.app.inspecting => self.app.move_selection(-page),
            KeyCode::PageDown if !self.app.inspecting => self.app.move_selection(page),
            _ => {}
        }
        false
    }

    fn handle_mouse(&mut self, m: MouseEvent) -> bool {
        match m.kind {
            MouseEventKind::ScrollUp => {
                self.app.move_selection(-1);
                self.sync_inspector();
                true
            }
            MouseEventKind::ScrollDown => {
                self.app.move_selection(1);
                self.sync_inspector();
                true
            }
            MouseEventKind::Down(MouseButton::Left)
                if self.app.handle_left_click(m.column, m.row) =>
            {
                self.sync_inspector();
                true
            }
            MouseEventKind::Down(MouseButton::Right)
                if self.app.handle_right_click(m.column, m.row) =>
            {
                self.sync_inspector();
                true
            }
            _ => false,
        }
    }

    fn sync_inspector(&self) {
        let pids = if self.app.inspecting {
            self.app.selected_pids()
        } else {
            Vec::new()
        };
        self.engine.inspect_pids(pids);
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        ratatui::restore();
    }
}
