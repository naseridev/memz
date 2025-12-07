use crate::analyzer::AnalyzedState;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};

const COLOR_PRIMARY: Color = Color::White;
const COLOR_SECONDARY: Color = Color::Yellow;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortMode {
    Pss,
    Rss,
    Shared,
    Pid,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewMode {
    Processes,
    MemoryMap,
    SharedMemory,
}

pub struct App {
    state: AnalyzedState,
    sort_mode: SortMode,
    view_mode: ViewMode,
    scroll_offset: usize,
    visible_rows: usize,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AnalyzedState {
                processes: Vec::new(),
                system: crate::analyzer::SystemStats {
                    total_kb: 0,
                    used_kb: 0,
                    available_kb: 0,
                    cached_kb: 0,
                    buffers_kb: 0,
                    swap_total_kb: 0,
                    swap_used_kb: 0,
                    total_process_pss_kb: 0,
                    total_process_rss_kb: 0,
                },
                shared_memory: crate::analyzer::SharedMemoryStats {
                    total_shared_kb: 0,
                    total_shared_clean_kb: 0,
                    total_shared_dirty_kb: 0,
                    sharing_efficiency: 0.0,
                },
                numa_nodes: Vec::new(),
                memory_map: crate::analyzer::MemoryMap {
                    kernel_kb: 0,
                    process_private_kb: 0,
                    process_shared_kb: 0,
                    cache_kb: 0,
                    buffers_kb: 0,
                    free_kb: 0,
                    slab_kb: 0,
                    page_tables_kb: 0,
                },
            },
            sort_mode: SortMode::Pss,
            view_mode: ViewMode::Processes,
            scroll_offset: 0,
            visible_rows: 20,
        }
    }

    pub fn update_data(&mut self, mut state: AnalyzedState) {
        match self.sort_mode {
            SortMode::Pss => state.processes.sort_by(|a, b| b.pss_kb.cmp(&a.pss_kb)),
            SortMode::Rss => state.processes.sort_by(|a, b| b.rss_kb.cmp(&a.rss_kb)),
            SortMode::Shared => state.processes.sort_by(|a, b| b.shared_kb.cmp(&a.shared_kb)),
            SortMode::Pid => state.processes.sort_by_key(|p| p.pid),
        }

        self.state = state;
    }

    pub fn next_sort(&mut self) {
        self.sort_mode = match self.sort_mode {
            SortMode::Pss => SortMode::Rss,
            SortMode::Rss => SortMode::Shared,
            SortMode::Shared => SortMode::Pid,
            SortMode::Pid => SortMode::Pss,
        };
        self.scroll_offset = 0;
    }

    pub fn toggle_view(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Processes => ViewMode::MemoryMap,
            ViewMode::MemoryMap => ViewMode::SharedMemory,
            ViewMode::SharedMemory => ViewMode::Processes,
        };
        self.scroll_offset = 0;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset + self.visible_rows < self.state.processes.len() {
            self.scroll_offset += 1;
        }
    }

    pub fn page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.visible_rows);
    }

    pub fn page_down(&mut self) {
        let max_offset = self.state.processes.len().saturating_sub(self.visible_rows);
        self.scroll_offset = (self.scroll_offset + self.visible_rows).min(max_offset);
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    app.visible_rows = chunks[1].height.saturating_sub(3) as usize;

    draw_system_stats(f, chunks[0], app);

    match app.view_mode {
        ViewMode::Processes => draw_process_list(f, chunks[1], app),
        ViewMode::MemoryMap => draw_memory_map(f, chunks[1], app),
        ViewMode::SharedMemory => draw_shared_view(f, chunks[1], app),
    }

    draw_help(f, chunks[2], app);
}

fn draw_system_stats(f: &mut Frame, area: Rect, app: &App) {
    let sys = &app.state.system;

    let used_pct = if sys.total_kb > 0 {
        (sys.used_kb as f64 / sys.total_kb as f64) * 100.0
    } else {
        0.0
    };

    let swap_pct = if sys.swap_total_kb > 0 {
        (sys.swap_used_kb as f64 / sys.swap_total_kb as f64) * 100.0
    } else {
        0.0
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Memory: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%)",
                sys.used_kb as f64 / 1024.0 / 1024.0,
                sys.total_kb as f64 / 1024.0 / 1024.0,
                used_pct
            )),
        ]),
        Line::from(vec![
            Span::styled("Available: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!("{:.1} GiB", sys.available_kb as f64 / 1024.0 / 1024.0)),
        ]),
        Line::from(vec![
            Span::styled("Cache/Buffers: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!(
                "{:.1} GiB",
                (sys.cached_kb + sys.buffers_kb) as f64 / 1024.0 / 1024.0
            )),
        ]),
        Line::from(vec![
            Span::styled("Swap: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%)",
                sys.swap_used_kb as f64 / 1024.0 / 1024.0,
                sys.swap_total_kb as f64 / 1024.0 / 1024.0,
                swap_pct
            )),
        ]),
        Line::from(vec![
            Span::styled("Process PSS: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!(
                "{:.1} GiB (accurate) | RSS: {:.1} GiB (overcounted)",
                sys.total_process_pss_kb as f64 / 1024.0 / 1024.0,
                sys.total_process_rss_kb as f64 / 1024.0 / 1024.0,
            )),
        ]),
    ];

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("System Memory"));
    f.render_widget(para, area);
}

fn draw_process_list(f: &mut Frame, area: Rect, app: &App) {
    let header_cells = ["PID", "Name", "PSS", "RSS", "Shared", "Private", "Swap", "Delta"]
        .iter()
        .map(|h| {
            ratatui::text::Text::from(*h).style(
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            )
        });

    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = app.state.processes
        .iter()
        .skip(app.scroll_offset)
        .take(app.visible_rows)
        .map(|proc| {
            let delta_str = if proc.pss_delta_kb != 0 {
                format!("{:+}", proc.pss_delta_kb / 1024)
            } else {
                String::from("-")
            };

            Row::new(vec![
                proc.pid.to_string(),
                proc.name.clone(),
                format!("{} M", proc.pss_kb / 1024),
                format!("{} M", proc.rss_kb / 1024),
                format!("{} M", proc.shared_kb / 1024),
                format!("{} M", proc.private_kb / 1024),
                format!("{} M", proc.swap_kb / 1024),
                delta_str,
            ])
            .style(if proc.pss_delta_kb.abs() > 10240 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect();

    let sort_indicator = match app.sort_mode {
        SortMode::Pss => " [Sort: PSS]",
        SortMode::Rss => " [Sort: RSS]",
        SortMode::Shared => " [Sort: Shared]",
        SortMode::Pid => " [Sort: PID]",
    };

    let title = format!(
        "Processes ({}/{}){}",
        app.scroll_offset.min(app.state.processes.len()),
        app.state.processes.len(),
        sort_indicator
    );

    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Min(20),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(table, area);
}

fn draw_memory_map(f: &mut Frame, area: Rect, app: &App) {
    let map = &app.state.memory_map;
    let sys = &app.state.system;

    let total = sys.total_kb as f64;

    let mut lines = vec![
        Line::from(Span::styled(
            "Physical Memory Distribution:",
            Style::default().fg(COLOR_SECONDARY).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    let items = vec![
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
        let gb = kb as f64 / 1024.0 / 1024.0;
        let pct = if total > 0.0 {
            (kb as f64 / total) * 100.0
        } else {
            0.0
        };

        let bar_width = ((pct / 100.0) * 50.0) as usize;
        let bar = "#".repeat(bar_width);

        lines.push(Line::from(vec![
            Span::styled(format!("{:16} ", label), Style::default().fg(color)),
            Span::raw(format!("{:7.1} GiB ({:5.1}%) ", gb, pct)),
            Span::styled(bar, Style::default().fg(color)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Total: ", Style::default().fg(COLOR_SECONDARY)),
        Span::raw(format!("{:.1} GiB", total / 1024.0 / 1024.0)),
    ]));

    if !app.state.numa_nodes.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "NUMA Nodes:",
            Style::default().fg(COLOR_SECONDARY).add_modifier(Modifier::BOLD),
        )));

        for node in &app.state.numa_nodes {
            let used_pct = if node.mem_total_kb > 0 {
                (node.mem_used_kb as f64 / node.mem_total_kb as f64) * 100.0
            } else {
                0.0
            };

            lines.push(Line::from(vec![
                Span::raw(format!("  Node {}: ", node.node_id)),
                Span::raw(format!(
                    "{:.1} / {:.1} GiB ({:.1}%)",
                    node.mem_used_kb as f64 / 1024.0 / 1024.0,
                    node.mem_total_kb as f64 / 1024.0 / 1024.0,
                    used_pct
                )),
            ]));
        }
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Physical Memory Map"));
    f.render_widget(para, area);
}

fn draw_shared_view(f: &mut Frame, area: Rect, app: &App) {
    let shared = &app.state.shared_memory;

    let lines = vec![
        Line::from(vec![
            Span::styled("Total Shared Memory: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!("{:.1} GiB", shared.total_shared_kb as f64 / 1024.0 / 1024.0)),
        ]),
        Line::from(vec![
            Span::styled("  Clean: ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{:.1} GiB", shared.total_shared_clean_kb as f64 / 1024.0 / 1024.0)),
        ]),
        Line::from(vec![
            Span::styled("  Dirty: ", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(format!("{:.1} GiB", shared.total_shared_dirty_kb as f64 / 1024.0 / 1024.0)),
        ]),
        Line::from(vec![
            Span::styled("Sharing Efficiency: ", Style::default().fg(COLOR_SECONDARY)),
            Span::raw(format!("{:.1}%", shared.sharing_efficiency)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Memory saved by sharing pages across processes",
            Style::default().fg(COLOR_PRIMARY),
        )),
    ];

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Shared Memory Analysis"));
    f.render_widget(para, area);
}

fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    let view_name = match app.view_mode {
        ViewMode::Processes => "map",
        ViewMode::MemoryMap => "shared",
        ViewMode::SharedMemory => "process",
    };

    let help_text = vec![Line::from(vec![
        Span::raw("q: quit | n: next sort | v: "),
        Span::styled(
            view_name,
            Style::default().fg(COLOR_SECONDARY),
        ),
        Span::raw(" view | up/down: scroll | PgUp/PgDn: page"),
    ])];

    let para = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}