use crate::analyzer::AnalyzedState;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Row, Table},
};

const COLOR_PRIMARY: Color = Color::White;
const COLOR_SECONDARY: Color = Color::Rgb(255, 165, 0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortMode {
    #[default]
    Pss,
    Rss,
    Shared,
    Pid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Processes,
    MemoryMap,
    SharedMemory,
}

#[derive(Default)]
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
            visible_rows: 20,
            ..Default::default()
        }
    }

    pub fn update_data(&mut self, state: AnalyzedState) {
        self.state = state;
        self.sort_processes();
        self.clamp_scroll();
    }

    pub fn next_sort(&mut self) {
        self.sort_mode = match self.sort_mode {
            SortMode::Pss => SortMode::Rss,
            SortMode::Rss => SortMode::Shared,
            SortMode::Shared => SortMode::Pid,
            SortMode::Pid => SortMode::Pss,
        };
        self.sort_processes();
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
        if self.scroll_offset < self.max_scroll_offset() {
            self.scroll_offset += 1;
        }
    }

    pub fn page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.visible_rows);
    }

    pub fn page_down(&mut self) {
        self.scroll_offset = self
            .scroll_offset
            .saturating_add(self.visible_rows)
            .min(self.max_scroll_offset());
    }

    fn sort_processes(&mut self) {
        self.state
            .processes
            .sort_unstable_by(|a, b| match self.sort_mode {
                SortMode::Pss => b.pss_kb.cmp(&a.pss_kb).then_with(|| a.pid.cmp(&b.pid)),
                SortMode::Rss => b.rss_kb.cmp(&a.rss_kb).then_with(|| a.pid.cmp(&b.pid)),
                SortMode::Shared => b
                    .shared_kb
                    .cmp(&a.shared_kb)
                    .then_with(|| a.pid.cmp(&b.pid)),
                SortMode::Pid => a.pid.cmp(&b.pid),
            });
    }

    fn max_scroll_offset(&self) -> usize {
        self.state.processes.len().saturating_sub(self.visible_rows)
    }

    fn clamp_scroll(&mut self) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset());
    }
}

fn to_gib(kb: u64) -> f64 {
    kb as f64 / 1024.0 / 1024.0
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
            Constraint::Length(7),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    app.visible_rows = chunks[1].height.saturating_sub(4) as usize;
    app.clamp_scroll();

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
    let label_style = Style::default().fg(COLOR_SECONDARY);

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
            Span::raw(format!("{:.1} GiB", to_gib(sys.available_kb))),
        ]),
        Line::from(vec![
            Span::styled("Cache/Buffers: ", label_style),
            Span::raw(format!(
                "{:.1} GiB",
                to_gib(sys.cached_kb.saturating_add(sys.buffers_kb))
            )),
        ]),
        Line::from(vec![
            Span::styled("Swap: ", label_style),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%)",
                to_gib(sys.swap_used_kb),
                to_gib(sys.swap_total_kb),
                pct(sys.swap_used_kb, sys.swap_total_kb)
            )),
        ]),
        Line::from(vec![
            Span::styled("Process PSS: ", label_style),
            Span::raw(format!(
                "{:.1} GiB (accurate) | RSS: {:.1} GiB (overcounted)",
                to_gib(sys.total_process_pss_kb),
                to_gib(sys.total_process_rss_kb),
            )),
        ]),
    ];

    let para = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title("System Memory"),
    );
    f.render_widget(para, area);
}

fn draw_process_list(f: &mut Frame, area: Rect, app: &App) {
    let header_style = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);

    let header = Row::new(
        [
            "PID", "Name", "PSS", "RSS", "Shared", "Private", "Swap", "Delta",
        ]
        .into_iter()
        .map(|h| Text::from(h).style(header_style)),
    )
    .height(1)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .state
        .processes
        .iter()
        .skip(app.scroll_offset)
        .take(app.visible_rows)
        .map(|proc| {
            let delta_mb = proc.pss_delta_kb / 1024;
            let delta_str = if delta_mb != 0 {
                format!("{:+}", delta_mb)
            } else {
                String::from("-")
            };

            let style = if proc.pss_delta_kb.saturating_abs() > 10240 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new([
                proc.pid.to_string(),
                proc.name.clone(),
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
    let view_name = match app.view_mode {
        ViewMode::Processes => "map",
        ViewMode::MemoryMap => "shared",
        ViewMode::SharedMemory => "process",
    };

    let help_text = vec![Line::from(vec![
        Span::raw("q: quit | n: next sort | v: "),
        Span::styled(view_name, Style::default().fg(COLOR_SECONDARY)),
        Span::raw(" view | up/down: scroll | PgUp/PgDn: page"),
    ])];

    let para =
        Paragraph::new(help_text).block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}
