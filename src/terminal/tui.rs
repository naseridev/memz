use crate::app::{engine::Engine, state::App};
use crate::terminal::ui;
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

const POLL_INTERVAL: Duration = Duration::from_millis(16);

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
        let mut needs_redraw = true;

        loop {
            if self.poll_telemetry() {
                needs_redraw = true;
            }

            if needs_redraw {
                self.terminal.draw(|frame| ui::draw(frame, &mut self.app))?;
                needs_redraw = false;
            }

            if event::poll(POLL_INTERVAL)? {
                loop {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            if self.handle_key(key) {
                                return Ok(());
                            }
                            needs_redraw = true;
                        }
                        Event::Mouse(mouse) => {
                            if self.handle_mouse(mouse) {
                                needs_redraw = true;
                            }
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

    fn poll_telemetry(&mut self) -> bool {
        let mut updated = false;

        if let Some(state) = self.engine.try_recv() {
            self.app.update_data(state);

            if self.app.inspecting {
                self.engine.inspect_pids(self.app.selected_pids());
            }

            updated = true;
        }

        if let Some(breakdown) = self.engine.try_recv_inspect() {
            self.app.inspector_breakdown = Some(breakdown);
            updated = true;
        }

        updated
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }

        let is_shift_tab = key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));

        if is_shift_tab {
            self.app.toggle_view();
            self.sync_inspector();
            return false;
        }

        let is_ctrl_x = (key.code == KeyCode::Char('x') || key.code == KeyCode::Char('X'))
            && key.modifiers.contains(KeyModifiers::CONTROL);

        if is_ctrl_x {
            self.app.toggle_shortcuts();
            return false;
        }

        if self.app.showing_shortcuts {
            return self.handle_shortcuts_key(key);
        }

        if self.app.choosing_color {
            return self.handle_color_picker_key(key);
        }

        if self.app.searching {
            self.handle_search_key(key);
            return false;
        }

        if self.app.inspecting {
            return self.handle_inspector_key(key);
        }

        if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q') {
            return true;
        }

        match key.code {
            KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Char(' ') => {
                self.app.toggle_baseline();
                return false;
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.app.open_color_picker();
                return false;
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let mode = self.app.faster_tick();
                self.engine.set_tick_mode(mode);
                return false;
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                let mode = self.app.slower_tick();
                self.engine.set_tick_mode(mode);
                return false;
            }
            _ => {}
        }

        if self.app.is_process_view() {
            self.handle_process_view_key(key);
        }

        false
    }

    fn handle_shortcuts_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return true,
            KeyCode::Esc => {
                self.app.close_shortcuts();
            }
            KeyCode::Up => {
                self.app.move_shortcuts_selection(-1);
            }
            KeyCode::Down => {
                self.app.move_shortcuts_selection(1);
            }
            KeyCode::Enter => {
                return self.execute_active_shortcut();
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                self.app.toggle_group();
                self.app.close_shortcuts();
            }
            KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Char(' ') => {
                self.app.toggle_baseline();
                self.app.close_shortcuts();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.app.next_sort();
                self.app.close_shortcuts();
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                self.engine.kill_pids(self.app.selected_pids());
                self.app.close_shortcuts();
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.app.close_shortcuts();
                self.app.open_color_picker();
            }
            KeyCode::Char('+') | KeyCode::Char('=') => {
                let mode = self.app.faster_tick();
                self.engine.set_tick_mode(mode);
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                let mode = self.app.slower_tick();
                self.engine.set_tick_mode(mode);
            }
            _ => {}
        }
        false
    }

    fn execute_active_shortcut(&mut self) -> bool {
        match self.app.shortcuts_index {
            0 => {
                self.app.toggle_group();
                self.app.close_shortcuts();
            }
            1 => {
                self.app.toggle_baseline();
                self.app.close_shortcuts();
            }
            2 => {
                self.app.next_sort();
                self.app.close_shortcuts();
            }
            3 => {
                self.engine.kill_pids(self.app.selected_pids());
                self.app.close_shortcuts();
            }
            4 => {
                self.app.close_shortcuts();
                self.app.open_color_picker();
            }
            5 => {
                let mode = self.app.faster_tick();
                self.engine.set_tick_mode(mode);
            }
            6 => {
                let mode = self.app.slower_tick();
                self.engine.set_tick_mode(mode);
            }
            7 => return true,
            _ => {}
        }
        false
    }

    fn handle_color_picker_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return true,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('C') => {
                self.app.close_color_picker();
            }
            KeyCode::Up => self.app.move_color_selection(-1),
            KeyCode::Down => self.app.move_color_selection(1),
            _ => {}
        }
        false
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        let page_step = self.app.visible_rows.max(1) as isize;

        match key.code {
            KeyCode::Esc => {
                self.app.searching = false;
                self.app.search_query.clear();
                self.app.selected_index = 0;
                self.app.rebuild_display_rows();
            }
            KeyCode::Enter => {
                self.app.searching = false;
            }
            KeyCode::Backspace => {
                self.app.search_query.pop();
                self.app.selected_index = 0;
                self.app.rebuild_display_rows();
            }
            KeyCode::Up => self.app.move_selection(-1),
            KeyCode::Down => self.app.move_selection(1),
            KeyCode::PageUp => self.app.move_selection(-page_step),
            KeyCode::PageDown => self.app.move_selection(page_step),
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.app.search_query.push(ch);
                self.app.selected_index = 0;
                self.app.rebuild_display_rows();
            }
            _ => {}
        }
    }

    fn handle_inspector_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return true,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('i') | KeyCode::Char('I') => {
                self.app.close_inspector();
                self.sync_inspector();
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                self.engine.kill_pids(self.app.selected_pids());
            }
            KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Char(' ') => {
                self.app.toggle_baseline();
            }
            KeyCode::Up => {
                self.app.move_selection(-1);
                self.sync_inspector();
            }
            KeyCode::Down => {
                self.app.move_selection(1);
                self.sync_inspector();
            }
            _ => {}
        }
        false
    }

    fn handle_process_view_key(&mut self, key: KeyEvent) {
        let page_step = self.app.visible_rows.max(1) as isize;

        match key.code {
            KeyCode::Esc => {
                if !self.app.search_query.is_empty() {
                    self.app.search_query.clear();
                    self.app.selected_index = 0;
                    self.app.rebuild_display_rows();
                }
            }
            KeyCode::Enter => {
                self.app.open_inspector();
                self.sync_inspector();
            }
            KeyCode::Char('/') => {
                self.app.searching = true;
            }
            KeyCode::Char('g') | KeyCode::Char('G') => {
                self.app.toggle_group();
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.app.next_sort();
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                self.engine.kill_pids(self.app.selected_pids());
            }
            KeyCode::Up => self.app.move_selection(-1),
            KeyCode::Down => self.app.move_selection(1),
            KeyCode::PageUp => self.app.move_selection(-page_step),
            KeyCode::PageDown => self.app.move_selection(page_step),
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self.app.showing_shortcuts {
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.app.move_shortcuts_selection(-1);
                    true
                }
                MouseEventKind::ScrollDown => {
                    self.app.move_shortcuts_selection(1);
                    true
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(idx) = self.app.shortcut_index_at(mouse.column, mouse.row) {
                        self.app.shortcuts_index = idx;
                        self.execute_active_shortcut();
                        true
                    } else if !self.app.is_inside_modal(mouse.column, mouse.row) {
                        self.app.close_shortcuts();
                        true
                    } else {
                        false
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.app.close_shortcuts();
                    true
                }
                _ => false,
            };
        }

        if self.app.choosing_color {
            return match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.app.move_color_selection(-1);
                    true
                }
                MouseEventKind::ScrollDown => {
                    self.app.move_color_selection(1);
                    true
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    self.app.handle_left_click(mouse.column, mouse.row)
                }
                MouseEventKind::Down(MouseButton::Right) => {
                    self.app.handle_right_click(mouse.column, mouse.row)
                }
                _ => false,
            };
        }

        if !self.app.is_process_view() {
            return false;
        }

        match mouse.kind {
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
                if self.app.handle_left_click(mouse.column, mouse.row) =>
            {
                self.sync_inspector();
                true
            }
            MouseEventKind::Down(MouseButton::Right)
                if self.app.handle_right_click(mouse.column, mouse.row) =>
            {
                self.sync_inspector();
                true
            }
            _ => false,
        }
    }

    fn sync_inspector(&self) {
        let pids = if self.app.inspecting && self.app.is_process_view() {
            self.app.selected_pids()
        } else {
            Vec::new()
        };

        self.engine.inspect_pids(pids);
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        self.app.save_config();
        let _ = execute!(io::stdout(), DisableMouseCapture);

        ratatui::restore();
    }
}
