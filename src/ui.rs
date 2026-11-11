use crate::analyzer::{AnalyzedState, ProcessStats};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Row, Table, TableState},
};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Default)]
struct DisplayRow {
    pid: u32,
    count: usize,
    name: String,
    pss_kb: u64,
    rss_kb: u64,
    shared_kb: u64,
    private_kb: u64,
    swap_kb: u64,
    pss_delta_kb: i64,
    pids: Vec<u32>,
}

#[derive(Default)]
pub struct App {
    state: AnalyzedState,
    display_rows: Vec<DisplayRow>,
    sort_mode: SortMode,
    view_mode: ViewMode,
    grouped: bool,
    searching: bool,
    search_query: String,
    selected_index: usize,
    table_state: TableState,
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
        self.rebuild_display_rows();
    }

    pub fn is_searching(&self) -> bool {
        self.searching
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
            SortMode::Shared => SortMode::Pid,
            SortMode::Pid => SortMode::Pss,
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
        self.sync_table_state();
    }

    pub fn scroll_down(&mut self) {
        if !self.display_rows.is_empty() && self.selected_index + 1 < self.display_rows.len() {
            self.selected_index += 1;
            self.sync_table_state();
        }
    }

    pub fn page_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(self.visible_rows.max(1));
        self.sync_table_state();
    }

    pub fn page_down(&mut self) {
        if !self.display_rows.is_empty() {
            self.selected_index = self
                .selected_index
                .saturating_add(self.visible_rows.max(1))
                .min(self.display_rows.len() - 1);
            self.sync_table_state();
        }
    }

    fn rebuild_display_rows(&mut self) {
        let query = self.search_query.trim().to_lowercase();

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
                        ..Default::default()
                    });
                entry.pid = entry.pid.min(proc.pid);
                entry.count += 1;
                entry.pss_kb = entry.pss_kb.saturating_add(proc.pss_kb);
                entry.rss_kb = entry.rss_kb.saturating_add(proc.rss_kb);
                entry.shared_kb = entry.shared_kb.saturating_add(proc.shared_kb);
                entry.private_kb = entry.private_kb.saturating_add(proc.private_kb);
                entry.swap_kb = entry.swap_kb.saturating_add(proc.swap_kb);
                entry.pss_delta_kb = entry.pss_delta_kb.saturating_add(proc.pss_delta_kb);
                entry.pids.push(proc.pid);
            }
            groups.into_values().collect()
        } else {
            filtered
                .into_iter()
                .map(|p| DisplayRow {
                    pid: p.pid,
                    count: 1,
                    name: p.name.clone(),
                    pss_kb: p.pss_kb,
                    rss_kb: p.rss_kb,
                    shared_kb: p.shared_kb,
                    private_kb: p.private_kb,
                    swap_kb: p.swap_kb,
                    pss_delta_kb: p.pss_delta_kb,
                    pids: vec![p.pid],
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
        } else {
            self.selected_index = self.selected_index.min(self.display_rows.len() - 1);
            self.table_state.select(Some(self.selected_index));
        }
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
    app.sync_table_state();

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

fn draw_process_list(f: &mut Frame, area: Rect, app: &mut App) {
    let header_style = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);

    let first_col = if app.grouped { "Count" } else { "PID" };

    let header = Row::new(
        [
            first_col, "Name", "PSS", "RSS", "Shared", "Private", "Swap", "Delta",
        ]
        .into_iter()
        .map(|h| Text::from(h).style(header_style)),
    )
    .height(1)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .display_rows
        .iter()
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

            let id_col = if app.grouped {
                format!("{}x", proc.count)
            } else {
                proc.pid.to_string()
            };

            Row::new([
                id_col,
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
        SortMode::Pid => {
            if app.grouped {
                " [Sort: Count]"
            } else {
                " [Sort: PID]"
            }
        }
    };

    let group_indicator = if app.grouped { " [Grouped]" } else { "" };
    let filter_indicator = if !app.search_query.is_empty() {
        format!(" [Filter: {}]", app.search_query)
    } else {
        String::new()
    };

    let current_pos = if app.display_rows.is_empty() {
        0
    } else {
        app.selected_index + 1
    };

    let title = format!(
        "Processes ({}/{}){}{}{}",
        current_pos,
        app.display_rows.len(),
        sort_indicator,
        group_indicator,
        filter_indicator
    );

    let highlight_style = Style::default()
        .bg(COLOR_SECONDARY)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

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
    .highlight_style(highlight_style)
    .block(Block::default().borders(Borders::ALL).title(title));

    f.render_stateful_widget(table, area, &mut app.table_state);
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
        Span::raw("q: quit | /: search | g: group | k: kill | n: sort | v: "),
        Span::styled(view_name, secondary),
        Span::raw(" | up/down: select"),
    ])];

    let para =
        Paragraph::new(help_text).block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}
