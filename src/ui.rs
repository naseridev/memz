use crate::{
    analyzer::{AnalyzedState, PressureLevel, ProcessStats, detect_leak},
    collector::MemoryBreakdown,
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Row, Sparkline, Table, TableState},
};
use std::collections::{HashMap, VecDeque};

const COLOR_PRIMARY: Color = Color::White;
const COLOR_SECONDARY: Color = Color::Rgb(255, 165, 0);
const COLOR_ALERT: Color = Color::LightRed;
const COLOR_HEALTHY: Color = Color::LightGreen;

const TABLE_CONSTRAINTS: [Constraint; 8] = [
    Constraint::Length(7),
    Constraint::Min(20),
    Constraint::Length(9),
    Constraint::Length(9),
    Constraint::Length(9),
    Constraint::Length(10),
    Constraint::Length(8),
    Constraint::Length(9),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    Pid,
    Name,
    #[default]
    Pss,
    Rss,
    Shared,
    Private,
    Swap,
    Delta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Processes,
    MemoryMap,
    SharedMemory,
}

#[derive(Debug, Clone, Default)]
struct DisplayRow {
    pid: u32,
    count: usize,
    name: String,
    exe: String,
    cmdline: String,
    pss_kb: u64,
    rss_kb: u64,
    shared_kb: u64,
    private_kb: u64,
    swap_kb: u64,
    pss_delta_kb: i64,
    pss_history: Vec<u64>,
    is_leaking: bool,
    leak_rate_kb: u64,
    pids: Vec<u32>,
}

#[derive(Default)]
pub struct App {
    state: AnalyzedState,
    display_rows: Vec<DisplayRow>,
    inspector_breakdown: Option<MemoryBreakdown>,
    baseline_pids: HashMap<u32, u64>,
    baseline_groups: HashMap<String, u64>,
    baseline_system_pss_kb: Option<u64>,
    sort_mode: SortMode,
    view_mode: ViewMode,
    grouped: bool,
    searching: bool,
    inspecting: bool,
    search_query: String,
    selected_index: usize,
    table_state: TableState,
    visible_rows: usize,
    table_area: Rect,
    modal_area: Rect,
}

impl App {
    pub fn new() -> Self {
        Self {
            visible_rows: 20,
            ..Default::default()
        }
    }

    pub fn update_data(&mut self, state: AnalyzedState) {
        self.state = state;
        self.rebuild_display_rows();
    }

    pub fn update_breakdown(&mut self, breakdown: MemoryBreakdown) {
        self.inspector_breakdown = Some(breakdown);
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    pub fn is_inspecting(&self) -> bool {
        self.inspecting
    }

    pub fn open_inspector(&mut self) {
        if !self.display_rows.is_empty() {
            self.view_mode = ViewMode::Processes;
            self.inspecting = true;
            self.inspector_breakdown = None;
        }
    }

    pub fn close_inspector(&mut self) {
        self.inspecting = false;
        self.inspector_breakdown = None;
    }

    pub fn toggle_baseline(&mut self) {
        if self.baseline_system_pss_kb.is_some() {
            self.baseline_pids.clear();
            self.baseline_groups.clear();
            self.baseline_system_pss_kb = None;
        } else {
            self.baseline_pids.clear();
            self.baseline_groups.clear();
            for proc in &self.state.processes {
                self.baseline_pids.insert(proc.pid, proc.pss_kb);
                let entry = self.baseline_groups.entry(proc.name.clone()).or_insert(0);
                *entry = entry.saturating_add(proc.pss_kb);
            }
            self.baseline_system_pss_kb = Some(self.state.system.total_process_pss_kb);
        }
        self.rebuild_display_rows();
    }

    pub fn start_search(&mut self) {
        self.view_mode = ViewMode::Processes;
        self.searching = true;
    }

    pub fn finish_search(&mut self) {
        self.searching = false;
    }

    pub fn cancel_search(&mut self) {
        self.searching = false;
        self.search_query.clear();
        self.selected_index = 0;
        self.rebuild_display_rows();
    }

    pub fn clear_search(&mut self) {
        if !self.search_query.is_empty() {
            self.search_query.clear();
            self.selected_index = 0;
            self.rebuild_display_rows();
        }
    }

    pub fn push_search_char(&mut self, c: char) {
        self.search_query.push(c);
        self.selected_index = 0;
        self.rebuild_display_rows();
    }

    pub fn pop_search_char(&mut self) {
        self.search_query.pop();
        self.selected_index = 0;
        self.rebuild_display_rows();
    }

    pub fn toggle_group(&mut self) {
        self.grouped = !self.grouped;
        self.selected_index = 0;
        self.rebuild_display_rows();
    }

    pub fn selected_pids(&self) -> Vec<u32> {
        self.display_rows
            .get(self.selected_index)
            .map(|r| r.pids.clone())
            .unwrap_or_default()
    }

    pub fn next_sort(&mut self) {
        self.sort_mode = match self.sort_mode {
            SortMode::Pss => SortMode::Rss,
            SortMode::Rss => SortMode::Shared,
            SortMode::Shared => SortMode::Private,
            SortMode::Private => SortMode::Swap,
            SortMode::Swap => SortMode::Delta,
            SortMode::Delta => SortMode::Pid,
            SortMode::Pid => SortMode::Name,
            SortMode::Name => SortMode::Pss,
        };
        self.selected_index = 0;
        self.rebuild_display_rows();
    }

    pub fn toggle_view(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Processes => ViewMode::MemoryMap,
            ViewMode::MemoryMap => ViewMode::SharedMemory,
            ViewMode::SharedMemory => ViewMode::Processes,
        };
    }

    pub fn scroll_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
        self.inspector_breakdown = None;
        self.sync_table_state();
    }

    pub fn scroll_down(&mut self) {
        if !self.display_rows.is_empty() && self.selected_index + 1 < self.display_rows.len() {
            self.selected_index += 1;
            self.inspector_breakdown = None;
            self.sync_table_state();
        }
    }

    pub fn page_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(self.visible_rows.max(1));
        self.inspector_breakdown = None;
        self.sync_table_state();
    }

    pub fn page_down(&mut self) {
        if !self.display_rows.is_empty() {
            self.selected_index = self
                .selected_index
                .saturating_add(self.visible_rows.max(1))
                .min(self.display_rows.len() - 1);
            self.inspector_breakdown = None;
            self.sync_table_state();
        }
    }

    pub fn handle_left_click(&mut self, col: u16, row: u16) -> bool {
        if self.inspecting {
            if !rect_contains(self.modal_area, col, row) {
                self.close_inspector();
                return true;
            }
            return false;
        }

        if self.view_mode != ViewMode::Processes || !rect_contains(self.table_area, col, row) {
            return false;
        }

        let header_y = self.table_area.y.saturating_add(1);
        if row == header_y {
            let inner_header = Rect {
                x: self.table_area.x.saturating_add(1),
                y: header_y,
                width: self.table_area.width.saturating_sub(2),
                height: 1,
            };
            let col_rects = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(TABLE_CONSTRAINTS)
                .spacing(1)
                .split(inner_header);

            let modes = [
                SortMode::Pid,
                SortMode::Name,
                SortMode::Pss,
                SortMode::Rss,
                SortMode::Shared,
                SortMode::Private,
                SortMode::Swap,
                SortMode::Delta,
            ];

            for (rect, mode) in col_rects.iter().zip(modes) {
                if col >= rect.x && col < rect.right() {
                    self.sort_mode = mode;
                    self.selected_index = 0;
                    self.rebuild_display_rows();
                    return true;
                }
            }
            return false;
        }

        let data_start_y = self.table_area.y.saturating_add(3);
        let data_end_y = self.table_area.bottom().saturating_sub(1);
        if row >= data_start_y && row < data_end_y {
            let visual_idx = (row - data_start_y) as usize;
            let target_idx = self.table_state.offset().saturating_add(visual_idx);
            if target_idx < self.display_rows.len() {
                if self.selected_index == target_idx {
                    self.open_inspector();
                } else {
                    self.selected_index = target_idx;
                    self.inspector_breakdown = None;
                    self.sync_table_state();
                }
                return true;
            }
        }

        false
    }

    pub fn handle_right_click(&mut self, col: u16, row: u16) -> bool {
        if self.inspecting {
            self.close_inspector();
            return true;
        }

        if self.view_mode == ViewMode::Processes && rect_contains(self.table_area, col, row) {
            let data_start_y = self.table_area.y.saturating_add(3);
            let data_end_y = self.table_area.bottom().saturating_sub(1);
            if row >= data_start_y && row < data_end_y {
                let visual_idx = (row - data_start_y) as usize;
                let target_idx = self.table_state.offset().saturating_add(visual_idx);
                if target_idx < self.display_rows.len() {
                    self.selected_index = target_idx;
                    self.sync_table_state();
                    self.open_inspector();
                    return true;
                }
            }
        }

        false
    }

    fn rebuild_display_rows(&mut self) {
        let query = self.search_query.trim().to_lowercase();
        let has_baseline = self.baseline_system_pss_kb.is_some();

        let filtered: Vec<&ProcessStats> = self
            .state
            .processes
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.name.to_lowercase().contains(&query)
                    || p.pid.to_string().contains(&query)
            })
            .collect();

        let mut rows: Vec<DisplayRow> = if self.grouped {
            let mut groups: HashMap<String, DisplayRow> = HashMap::new();
            for proc in filtered {
                let entry = groups
                    .entry(proc.name.clone())
                    .or_insert_with(|| DisplayRow {
                        pid: proc.pid,
                        count: 0,
                        name: proc.name.clone(),
                        exe: proc.exe.clone(),
                        cmdline: proc.cmdline.clone(),
                        ..Default::default()
                    });
                entry.pid = entry.pid.min(proc.pid);
                entry.count += 1;
                if entry.exe.is_empty() && !proc.exe.is_empty() {
                    entry.exe = proc.exe.clone();
                }
                if entry.cmdline.is_empty() && !proc.cmdline.is_empty() {
                    entry.cmdline = proc.cmdline.clone();
                }
                entry.pss_kb = entry.pss_kb.saturating_add(proc.pss_kb);
                entry.rss_kb = entry.rss_kb.saturating_add(proc.rss_kb);
                entry.shared_kb = entry.shared_kb.saturating_add(proc.shared_kb);
                entry.private_kb = entry.private_kb.saturating_add(proc.private_kb);
                entry.swap_kb = entry.swap_kb.saturating_add(proc.swap_kb);
                entry.pss_delta_kb = entry.pss_delta_kb.saturating_add(proc.pss_delta_kb);
                entry.is_leaking |= proc.is_leaking;
                entry.leak_rate_kb = entry.leak_rate_kb.saturating_add(proc.leak_rate_kb);
                entry.pids.push(proc.pid);

                merge_history(&mut entry.pss_history, &proc.pss_history);
            }

            for row in groups.values_mut() {
                if has_baseline {
                    let base = self.baseline_groups.get(&row.name).copied().unwrap_or(0);
                    row.pss_delta_kb = diff_kb(row.pss_kb, base);
                }
                if !row.is_leaking {
                    let deque: VecDeque<u64> = row.pss_history.iter().copied().collect();
                    let (leaking, rate) = detect_leak(&deque);
                    row.is_leaking = leaking;
                    row.leak_rate_kb = rate;
                }
            }

            groups.into_values().collect()
        } else {
            filtered
                .into_iter()
                .map(|p| {
                    let pss_delta_kb = if has_baseline {
                        let base = self.baseline_pids.get(&p.pid).copied().unwrap_or(0);
                        diff_kb(p.pss_kb, base)
                    } else {
                        p.pss_delta_kb
                    };

                    DisplayRow {
                        pid: p.pid,
                        count: 1,
                        name: p.name.clone(),
                        exe: p.exe.clone(),
                        cmdline: p.cmdline.clone(),
                        pss_kb: p.pss_kb,
                        rss_kb: p.rss_kb,
                        shared_kb: p.shared_kb,
                        private_kb: p.private_kb,
                        swap_kb: p.swap_kb,
                        pss_delta_kb,
                        pss_history: p.pss_history.clone(),
                        is_leaking: p.is_leaking,
                        leak_rate_kb: p.leak_rate_kb,
                        pids: vec![p.pid],
                    }
                })
                .collect()
        };

        rows.sort_unstable_by(|a, b| match self.sort_mode {
            SortMode::Pss => b.pss_kb.cmp(&a.pss_kb).then_with(|| a.pid.cmp(&b.pid)),
            SortMode::Rss => b.rss_kb.cmp(&a.rss_kb).then_with(|| a.pid.cmp(&b.pid)),
            SortMode::Shared => b
                .shared_kb
                .cmp(&a.shared_kb)
                .then_with(|| a.pid.cmp(&b.pid)),
            SortMode::Private => b
                .private_kb
                .cmp(&a.private_kb)
                .then_with(|| a.pid.cmp(&b.pid)),
            SortMode::Swap => b.swap_kb.cmp(&a.swap_kb).then_with(|| a.pid.cmp(&b.pid)),
            SortMode::Delta => b
                .pss_delta_kb
                .saturating_abs()
                .cmp(&a.pss_delta_kb.saturating_abs())
                .then_with(|| b.pss_kb.cmp(&a.pss_kb)),
            SortMode::Name => a.name.cmp(&b.name).then_with(|| b.pss_kb.cmp(&a.pss_kb)),
            SortMode::Pid => {
                if self.grouped {
                    b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name))
                } else {
                    a.pid.cmp(&b.pid)
                }
            }
        });

        self.display_rows = rows;
        self.sync_table_state();
    }

    fn sync_table_state(&mut self) {
        if self.display_rows.is_empty() {
            self.selected_index = 0;
            self.table_state.select(None);
            self.inspecting = false;
        } else {
            self.selected_index = self.selected_index.min(self.display_rows.len() - 1);
            self.table_state.select(Some(self.selected_index));
        }
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.right() && row >= r.y && row < r.bottom()
}

fn diff_kb(current: u64, base: u64) -> i64 {
    if current >= base {
        (current - base).min(i64::MAX as u64) as i64
    } else {
        -((base - current).min(i64::MAX as u64) as i64)
    }
}

fn merge_history(target: &mut Vec<u64>, source: &[u64]) {
    if source.is_empty() {
        return;
    }
    if target.len() < source.len() {
        let diff = source.len() - target.len();
        let mut new_target = vec![0u64; diff];
        new_target.append(target);
        *target = new_target;
    }
    let offset = target.len() - source.len();
    for (t, &s) in target.iter_mut().skip(offset).zip(source.iter()) {
        *t = t.saturating_add(s);
    }
}

fn to_gib(kb: u64) -> f64 {
    kb as f64 / 1024.0 / 1024.0
}

fn to_mib(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

fn pct(part: u64, total: u64) -> f64 {
    if total > 0 {
        (part as f64 / total as f64) * 100.0
    } else {
        0.0
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    app.table_area = chunks[1];
    app.visible_rows = chunks[1].height.saturating_sub(4) as usize;
    app.sync_table_state();

    draw_system_stats(f, chunks[0], app);

    match app.view_mode {
        ViewMode::Processes => draw_process_list(f, chunks[1], app),
        ViewMode::MemoryMap => draw_memory_map(f, chunks[1], app),
        ViewMode::SharedMemory => draw_shared_view(f, chunks[1], app),
    }

    draw_help(f, chunks[2], app);

    if app.inspecting {
        app.modal_area = centered_rect(76, 82, f.area());
        draw_inspector_modal(f, app.modal_area, app);
    }
}

fn draw_system_stats(f: &mut Frame, area: Rect, app: &App) {
    let sys = &app.state.system;
    let label_style = Style::default().fg(COLOR_SECONDARY);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let (pressure_text, pressure_style) = match sys.pressure_level {
        PressureLevel::Healthy => ("HEALTHY", Style::default().fg(COLOR_HEALTHY)),
        PressureLevel::Warning => (
            "WARNING",
            Style::default()
                .fg(COLOR_SECONDARY)
                .add_modifier(Modifier::BOLD),
        ),
        PressureLevel::Critical => (
            "CRITICAL / SWAPPING",
            Style::default()
                .fg(COLOR_ALERT)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let swap_io_str = if sys.swap_rate_kb != 0 {
        format!(" ({:+.1} MiB/s)", sys.swap_rate_kb as f64 / 1024.0)
    } else {
        String::new()
    };

    let psi_str = if sys.psi_some_avg10 > 0.0 || sys.psi_full_avg10 > 0.0 {
        format!(
            " | PSI some: {:.1}% full: {:.1}%",
            sys.psi_some_avg10, sys.psi_full_avg10
        )
    } else {
        String::new()
    };

    let baseline_str = if let Some(base_pss) = app.baseline_system_pss_kb {
        let diff_mib = diff_kb(sys.total_process_pss_kb, base_pss) as f64 / 1024.0;
        format!(" | BaseΔ: {:+.1} MiB", diff_mib)
    } else {
        String::new()
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Memory: ", label_style),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%)",
                to_gib(sys.used_kb),
                to_gib(sys.total_kb),
                pct(sys.used_kb, sys.total_kb)
            )),
        ]),
        Line::from(vec![
            Span::styled("Available: ", label_style),
            Span::raw(format!(
                "{:.1} GiB | Cache/Buffers: {:.1} GiB",
                to_gib(sys.available_kb),
                to_gib(sys.cached_kb.saturating_add(sys.buffers_kb))
            )),
        ]),
        Line::from(vec![
            Span::styled("Swap: ", label_style),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%){}",
                to_gib(sys.swap_used_kb),
                to_gib(sys.swap_total_kb),
                pct(sys.swap_used_kb, sys.swap_total_kb),
                swap_io_str
            )),
        ]),
        Line::from(vec![
            Span::styled("Process PSS: ", label_style),
            Span::raw(format!(
                "{:.1} GiB (accurate) | RSS: {:.1} GiB{}",
                to_gib(sys.total_process_pss_kb),
                to_gib(sys.total_process_rss_kb),
                baseline_str
            )),
        ]),
        Line::from(vec![
            Span::styled("Pressure: ", label_style),
            Span::styled(pressure_text, pressure_style),
            Span::raw(psi_str),
        ]),
    ];

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("System Memory"),
    );
    f.render_widget(para, cols[0]);

    let spark_color = match sys.pressure_level {
        PressureLevel::Critical => COLOR_ALERT,
        PressureLevel::Warning => COLOR_SECONDARY,
        PressureLevel::Healthy => COLOR_SECONDARY,
    };

    let sparkline = Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Memory Trend (60s)"),
        )
        .data(&sys.history_used_kb)
        .max(sys.total_kb.max(1))
        .style(Style::default().fg(spark_color));
    f.render_widget(sparkline, cols[1]);
}

fn draw_process_list(f: &mut Frame, area: Rect, app: &mut App) {
    let header_style = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);

    let first_col = if app.grouped { "Count" } else { "PID" };
    let delta_col = if app.baseline_system_pss_kb.is_some() {
        "BaseΔ"
    } else {
        "Delta"
    };

    let header = Row::new(
        [
            first_col, "Name", "PSS", "RSS", "Shared", "Private", "Swap", delta_col,
        ]
        .into_iter()
        .map(|h| Text::from(h).style(header_style)),
    )
    .height(1)
    .bottom_margin(1);

    let mut leak_count = 0usize;

    let rows: Vec<Row> = app
        .display_rows
        .iter()
        .map(|proc| {
            if proc.is_leaking {
                leak_count += 1;
            }

            let delta_mb = proc.pss_delta_kb / 1024;
            let delta_str = if delta_mb != 0 {
                format!("{:+}", delta_mb)
            } else {
                String::from("-")
            };

            let style = if proc.is_leaking {
                Style::default()
                    .fg(COLOR_ALERT)
                    .add_modifier(Modifier::BOLD)
            } else if proc.pss_delta_kb.saturating_abs() > 10240 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let id_col = if app.grouped {
                format!("{}x", proc.count)
            } else {
                proc.pid.to_string()
            };

            let display_name = if proc.is_leaking {
                format!("{} [LEAK?]", proc.name)
            } else {
                proc.name.clone()
            };

            Row::new([
                id_col,
                display_name,
                format!("{} M", proc.pss_kb / 1024),
                format!("{} M", proc.rss_kb / 1024),
                format!("{} M", proc.shared_kb / 1024),
                format!("{} M", proc.private_kb / 1024),
                format!("{} M", proc.swap_kb / 1024),
                delta_str,
            ])
            .style(style)
        })
        .collect();

    let sort_indicator = match app.sort_mode {
        SortMode::Pss => " [Sort: PSS]",
        SortMode::Rss => " [Sort: RSS]",
        SortMode::Shared => " [Sort: Shared]",
        SortMode::Private => " [Sort: Private]",
        SortMode::Swap => " [Sort: Swap]",
        SortMode::Delta => " [Sort: Delta]",
        SortMode::Name => " [Sort: Name]",
        SortMode::Pid => {
            if app.grouped {
                " [Sort: Count]"
            } else {
                " [Sort: PID]"
            }
        }
    };

    let group_indicator = if app.grouped { " [Grouped]" } else { "" };
    let baseline_indicator = if app.baseline_system_pss_kb.is_some() {
        " [Baseline]"
    } else {
        ""
    };
    let filter_indicator = if !app.search_query.is_empty() {
        format!(" [Filter: {}]", app.search_query)
    } else {
        String::new()
    };
    let leak_indicator = if leak_count > 0 {
        format!(" [Leaks: {}]", leak_count)
    } else {
        String::new()
    };

    let current_pos = if app.display_rows.is_empty() {
        0
    } else {
        app.selected_index + 1
    };

    let title = format!(
        "Processes ({}/{}){}{}{}{}{}",
        current_pos,
        app.display_rows.len(),
        sort_indicator,
        group_indicator,
        baseline_indicator,
        filter_indicator,
        leak_indicator
    );

    let highlight_style = Style::default()
        .bg(COLOR_SECONDARY)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    let table = Table::new(rows, TABLE_CONSTRAINTS)
        .header(header)
        .highlight_style(highlight_style)
        .block(Block::default().borders(Borders::ALL).title(title));

    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_inspector_modal(f: &mut Frame, modal_area: Rect, app: &App) {
    let Some(proc) = app.display_rows.get(app.selected_index) else {
        return;
    };

    f.render_widget(Clear, modal_area);

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_SECONDARY))
        .title(
            "Process Memory Inspector (Enter/Esc/RightClick: close | Up/Down: switch | k: kill)",
        );

    let inner_area = outer_block.inner(modal_area);
    f.render_widget(outer_block, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(11), Constraint::Length(6)])
        .split(inner_area);

    let label = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);

    let exe_str = if proc.exe.is_empty() {
        "N/A"
    } else {
        &proc.exe
    };
    let cmd_str = if proc.cmdline.is_empty() {
        &proc.name
    } else {
        &proc.cmdline
    };

    let leak_span = if proc.is_leaking {
        Span::styled(
            format!("LEAK DETECTED (+{} KB/s)", proc.leak_rate_kb),
            Style::default()
                .fg(COLOR_ALERT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("Stable", Style::default().fg(COLOR_HEALTHY))
    };

    let breakdown = app.inspector_breakdown.clone().unwrap_or(MemoryBreakdown {
        heap_kb: 0,
        stack_kb: 0,
        anon_kb: proc.private_kb,
        libs_kb: proc.shared_kb,
    });

    let total_pss = proc.pss_kb.max(1);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Process: ", label),
            Span::raw(format!(
                "{} (PID: {}, Instances: {})  ",
                proc.name, proc.pid, proc.count
            )),
            Span::styled("Status: ", label),
            leak_span,
        ]),
        Line::from(vec![
            Span::styled("Executable: ", label),
            Span::raw(exe_str),
        ]),
        Line::from(vec![Span::styled("Cmdline: ", label), Span::raw(cmd_str)]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Summary: ", label),
            Span::raw(format!(
                "PSS: {:.1} MiB | RSS: {:.1} MiB | Private: {:.1} MiB | Shared: {:.1} MiB | Swap: {:.1} MiB",
                to_mib(proc.pss_kb),
                to_mib(proc.rss_kb),
                to_mib(proc.private_kb),
                to_mib(proc.shared_kb),
                to_mib(proc.swap_kb),
            )),
        ]),
        Line::from(""),
        Line::from(Span::styled("Memory Region Breakdown (PSS):", label)),
    ];

    let segments = [
        ("Heap", breakdown.heap_kb, COLOR_SECONDARY),
        ("Stack", breakdown.stack_kb, COLOR_PRIMARY),
        ("Anonymous", breakdown.anon_kb, COLOR_PRIMARY),
        ("Shared Libs", breakdown.libs_kb, COLOR_SECONDARY),
        ("Swap", proc.swap_kb, COLOR_PRIMARY),
    ];

    for (seg_name, kb, color) in segments {
        let seg_pct = pct(kb, total_pss);
        let bar_width = ((seg_pct.clamp(0.0, 100.0) / 100.0) * 32.0) as usize;
        let style = Style::default().fg(color);

        lines.push(Line::from(vec![
            Span::styled(format!("  {:13} ", seg_name), style),
            Span::raw(format!("{:8.1} MiB ({:5.1}%) ", to_mib(kb), seg_pct)),
            Span::styled("#".repeat(bar_width), style),
        ]));
    }

    let info_para = Paragraph::new(lines);
    f.render_widget(info_para, chunks[0]);

    let max_pss = proc.pss_history.iter().copied().max().unwrap_or(1).max(1);
    let spark_color = if proc.is_leaking {
        COLOR_ALERT
    } else {
        COLOR_SECONDARY
    };

    let sparkline = Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Process PSS Trend (last 60s)"),
        )
        .data(&proc.pss_history)
        .max(max_pss)
        .style(Style::default().fg(spark_color));
    f.render_widget(sparkline, chunks[1]);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn draw_memory_map(f: &mut Frame, area: Rect, app: &App) {
    let map = &app.state.memory_map;
    let total_kb = app.state.system.total_kb;
    let header_style = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(Span::styled("Physical Memory Distribution:", header_style)),
        Line::from(""),
    ];

    let items = [
        ("Kernel", map.kernel_kb, COLOR_SECONDARY),
        ("Process Private", map.process_private_kb, COLOR_PRIMARY),
        ("Process Shared", map.process_shared_kb, COLOR_PRIMARY),
        ("Page Cache", map.cache_kb, COLOR_PRIMARY),
        ("Buffers", map.buffers_kb, COLOR_PRIMARY),
        ("Slab", map.slab_kb, COLOR_PRIMARY),
        ("Page Tables", map.page_tables_kb, COLOR_PRIMARY),
        ("Free", map.free_kb, COLOR_SECONDARY),
    ];

    for (label, kb, color) in items {
        let item_pct = pct(kb, total_kb);
        let bar_width = ((item_pct.clamp(0.0, 100.0) / 100.0) * 50.0) as usize;
        let style = Style::default().fg(color);

        lines.push(Line::from(vec![
            Span::styled(format!("{:16} ", label), style),
            Span::raw(format!("{:7.1} GiB ({:5.1}%) ", to_gib(kb), item_pct)),
            Span::styled("#".repeat(bar_width), style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Total: ", Style::default().fg(COLOR_SECONDARY)),
        Span::raw(format!("{:.1} GiB", to_gib(total_kb))),
    ]));

    if !app.state.numa_nodes.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("NUMA Nodes:", header_style)));

        for node in &app.state.numa_nodes {
            lines.push(Line::from(vec![
                Span::raw(format!("  Node {}: ", node.node_id)),
                Span::raw(format!(
                    "{:.1} / {:.1} GiB ({:.1}%)",
                    to_gib(node.mem_used_kb),
                    to_gib(node.mem_total_kb),
                    pct(node.mem_used_kb, node.mem_total_kb)
                )),
            ]));
        }
    }

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Physical Memory Map"),
    );
    f.render_widget(para, area);
}

fn draw_shared_view(f: &mut Frame, area: Rect, app: &App) {
    let shared = &app.state.shared_memory;
    let primary = Style::default().fg(COLOR_PRIMARY);
    let secondary = Style::default().fg(COLOR_SECONDARY);

    let lines = vec![
        Line::from(vec![
            Span::styled("Total Shared Memory: ", secondary),
            Span::raw(format!("{:.1} GiB", to_gib(shared.total_shared_kb))),
        ]),
        Line::from(vec![
            Span::styled("  Clean: ", primary),
            Span::raw(format!("{:.1} GiB", to_gib(shared.total_shared_clean_kb))),
        ]),
        Line::from(vec![
            Span::styled("  Dirty: ", primary),
            Span::raw(format!("{:.1} GiB", to_gib(shared.total_shared_dirty_kb))),
        ]),
        Line::from(vec![
            Span::styled("Sharing Efficiency: ", secondary),
            Span::raw(format!("{:.1}%", shared.sharing_efficiency)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Memory saved by sharing pages across processes",
            primary,
        )),
    ];

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Shared Memory Analysis"),
    );
    f.render_widget(para, area);
}

fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    let secondary = Style::default().fg(COLOR_SECONDARY);

    if app.searching {
        let search_line = vec![Line::from(vec![
            Span::styled("Search: ", secondary.add_modifier(Modifier::BOLD)),
            Span::raw(&app.search_query),
            Span::styled("_", secondary),
            Span::raw("  (Enter: lock filter | Esc: clear)"),
        ])];

        let para = Paragraph::new(search_line)
            .block(Block::default().borders(Borders::ALL).title("Filter"));
        f.render_widget(para, area);
        return;
    }

    let view_name = match app.view_mode {
        ViewMode::Processes => "map",
        ViewMode::MemoryMap => "shared",
        ViewMode::SharedMemory => "process",
    };

    let help_text = vec![Line::from(vec![
        Span::raw(
            "q: quit | Enter/Click: inspect | b/Space: baseline | /: search | g: group | k: kill | n: sort | v: ",
        ),
        Span::styled(view_name, secondary),
    ])];

    let para =
        Paragraph::new(help_text).block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}
