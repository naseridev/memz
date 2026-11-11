use crate::{
    analyzer::{PressureLevel, signed_diff},
    collector::MemoryBreakdown,
    state::{App, SortMode, TABLE_CONSTRAINTS, ViewMode},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Row, Sparkline, Table},
};

const COLOR_PRIMARY: Color = Color::White;
const COLOR_SECONDARY: Color = Color::Rgb(255, 115, 40);
const COLOR_ALERT: Color = Color::LightRed;
const COLOR_HEALTHY: Color = Color::LightGreen;

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
        app.modal_area = centered_rect(58, 52, f.area());
        draw_inspector_modal(f, app.modal_area, app);
    }
}

fn draw_system_stats(f: &mut Frame, area: Rect, app: &App) {
    let sys = &app.state.system;
    let label = Style::default().fg(COLOR_SECONDARY);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let (p_txt, p_style) = match sys.pressure_level {
        PressureLevel::Healthy => ("HEALTHY", Style::default().fg(COLOR_HEALTHY)),
        PressureLevel::Warning => ("WARNING", label.add_modifier(Modifier::BOLD)),
        PressureLevel::Critical => (
            "CRITICAL / SWAPPING",
            Style::default()
                .fg(COLOR_ALERT)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let swap_io = if sys.swap_rate_kb != 0 {
        format!(" ({:+.1} MiB/s)", sys.swap_rate_kb as f64 / 1024.0)
    } else {
        String::new()
    };
    let psi = if sys.psi_some_avg10 > 0.0 || sys.psi_full_avg10 > 0.0 {
        format!(
            " | PSI {:.1}%/{:.1}%",
            sys.psi_some_avg10, sys.psi_full_avg10
        )
    } else {
        String::new()
    };
    let base = app
        .baseline_system_pss_kb
        .map(|b| {
            format!(
                " | Base~: {:+.1} MiB",
                signed_diff(sys.total_process_pss_kb, b) as f64 / 1024.0
            )
        })
        .unwrap_or_default();

    let lines = vec![
        Line::from(vec![
            Span::styled("Memory: ", label),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%)",
                to_gib(sys.used_kb),
                to_gib(sys.total_kb),
                pct(sys.used_kb, sys.total_kb)
            )),
        ]),
        Line::from(vec![
            Span::styled("Available: ", label),
            Span::raw(format!(
                "{:.1} GiB | Cache/Buffers: {:.1} GiB",
                to_gib(sys.available_kb),
                to_gib(sys.cached_kb.saturating_add(sys.buffers_kb))
            )),
        ]),
        Line::from(vec![
            Span::styled("Swap: ", label),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%){}",
                to_gib(sys.swap_used_kb),
                to_gib(sys.swap_total_kb),
                pct(sys.swap_used_kb, sys.swap_total_kb),
                swap_io
            )),
        ]),
        Line::from(vec![
            Span::styled("Process PSS: ", label),
            Span::raw(format!(
                "{:.1} GiB | RSS: {:.1} GiB{}",
                to_gib(sys.total_process_pss_kb),
                to_gib(sys.total_process_rss_kb),
                base
            )),
        ]),
        Line::from(vec![
            Span::styled("Pressure: ", label),
            Span::styled(p_txt, p_style),
            Span::raw(psi),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("System Memory"),
        ),
        cols[0],
    );

    let spark_color = if sys.pressure_level == PressureLevel::Critical {
        COLOR_ALERT
    } else {
        COLOR_SECONDARY
    };
    let spark = Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Memory Trend (60s)"),
        )
        .data(&sys.history_used_kb)
        .max(sys.total_kb.max(1))
        .style(Style::default().fg(spark_color));
    f.render_widget(spark, cols[1]);
}

fn draw_process_list(f: &mut Frame, area: Rect, app: &mut App) {
    let h_style = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);
    let id_hdr = if app.grouped { "Count" } else { "PID" };
    let d_hdr = if app.baseline_system_pss_kb.is_some() {
        "Base~"
    } else {
        "Delta"
    };
    let header = Row::new(
        [
            id_hdr, "Name", "PSS", "RSS", "Shared", "Private", "Swap", d_hdr,
        ]
        .into_iter()
        .map(|h| Text::from(h).style(h_style)),
    )
    .height(1)
    .bottom_margin(1);

    let mut leaks = 0usize;
    let rows: Vec<Row> = app
        .display_rows
        .iter()
        .map(|p| {
            if p.is_leaking {
                leaks += 1;
            }
            let d_mb = p.pss_delta_kb / 1024;
            let style = if p.is_leaking {
                Style::default()
                    .fg(COLOR_ALERT)
                    .add_modifier(Modifier::BOLD)
            } else if p.pss_delta_kb.saturating_abs() > 10240 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let id = if app.grouped {
                format!("{}x", p.count)
            } else {
                p.pid.to_string()
            };
            let name = if p.is_leaking {
                format!("{} [LEAK?]", p.name)
            } else {
                p.name.to_string()
            };
            Row::new([
                id,
                name,
                format!("{} M", p.pss_kb / 1024),
                format!("{} M", p.rss_kb / 1024),
                format!("{} M", p.shared_kb / 1024),
                format!("{} M", p.private_kb / 1024),
                format!("{} M", p.swap_kb / 1024),
                if d_mb != 0 {
                    format!("{d_mb:+}")
                } else {
                    "-".into()
                },
            ])
            .style(style)
        })
        .collect();

    let sort_lbl = match app.sort_mode {
        SortMode::Pss => "PSS",
        SortMode::Rss => "RSS",
        SortMode::Shared => "Shared",
        SortMode::Private => "Private",
        SortMode::Swap => "Swap",
        SortMode::Delta => "Delta",
        SortMode::Name => "Name",
        SortMode::Pid => {
            if app.grouped {
                "Count"
            } else {
                "PID"
            }
        }
    };
    let pos = if app.display_rows.is_empty() {
        0
    } else {
        app.selected_index + 1
    };
    let title = format!(
        "Processes ({}/{}) [Sort: {}]{}{}{}{}",
        pos,
        app.display_rows.len(),
        sort_lbl,
        if app.grouped { " [Grouped]" } else { "" },
        if app.baseline_system_pss_kb.is_some() {
            " [Base~]"
        } else {
            ""
        },
        if app.search_query.is_empty() {
            String::new()
        } else {
            format!(" [Filter: {}]", app.search_query)
        },
        if leaks > 0 {
            format!(" [Leaks: {leaks}]")
        } else {
            String::new()
        }
    );
    let hl = Style::default()
        .bg(COLOR_SECONDARY)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let table = Table::new(rows, TABLE_CONSTRAINTS)
        .header(header)
        .highlight_style(hl)
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_inspector_modal(f: &mut Frame, area: Rect, app: &App) {
    let Some(p) = app.display_rows.get(app.selected_index) else {
        return;
    };
    f.render_widget(Clear, area);
    let title = if p.count > 1 {
        format!(" {} ({}x) ", p.name, p.count)
    } else {
        format!(" {} [PID {}] ", p.name, p.pid)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_SECONDARY))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(inner);
    let label = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);
    let status = if p.is_leaking {
        Span::styled(
            format!("LEAK (+{} KB/s)", p.leak_rate_kb),
            Style::default()
                .fg(COLOR_ALERT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("Healthy", Style::default().fg(COLOR_HEALTHY))
    };
    let b = app.inspector_breakdown.clone().unwrap_or(MemoryBreakdown {
        heap_kb: 0,
        stack_kb: 0,
        anon_kb: p.private_kb,
        libs_kb: p.shared_kb,
    });
    let cmd = if p.cmdline.is_empty() {
        p.name.as_ref()
    } else {
        p.cmdline.as_ref()
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Status: ", label),
            status,
            Span::raw("   "),
            Span::styled("Cmd: ", label),
            Span::raw(cmd),
        ]),
        Line::from(vec![
            Span::styled("Memory: ", label),
            Span::raw(format!(
                "PSS {:.1}M | Private {:.1}M | Shared {:.1}M | Swap {:.1}M",
                to_mib(p.pss_kb),
                to_mib(p.private_kb),
                to_mib(p.shared_kb),
                to_mib(p.swap_kb)
            )),
        ]),
        Line::from(""),
    ];
    for (name, kb, col) in [
        ("Heap", b.heap_kb, COLOR_SECONDARY),
        ("Stack", b.stack_kb, COLOR_PRIMARY),
        ("Anon", b.anon_kb, COLOR_PRIMARY),
        ("Libs", b.libs_kb, COLOR_SECONDARY),
    ] {
        let ratio = pct(kb, p.pss_kb.max(1));
        let bar = "#".repeat(((ratio.clamp(0.0, 100.0) / 100.0) * 24.0) as usize);
        lines.push(Line::from(vec![
            Span::styled(format!("{name:7} "), Style::default().fg(col)),
            Span::raw(format!("{:7.1}M ({:4.1}%) ", to_mib(kb), ratio)),
            Span::styled(bar, Style::default().fg(col)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), chunks[0]);

    let max_pss = p.pss_history.iter().copied().max().unwrap_or(1).max(1);
    let spark = Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title("60s Trend (Esc: close | k: kill)"),
        )
        .data(&p.pss_history)
        .max(max_pss)
        .style(Style::default().fg(if p.is_leaking {
            COLOR_ALERT
        } else {
            COLOR_SECONDARY
        }));
    f.render_widget(spark, chunks[1]);
}

fn draw_memory_map(f: &mut Frame, area: Rect, app: &App) {
    let (m, total) = (&app.state.memory_map, app.state.system.total_kb);
    let hdr = Style::default()
        .fg(COLOR_SECONDARY)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![
        Line::from(Span::styled("Physical Memory Distribution:", hdr)),
        Line::from(""),
    ];
    for (lbl, kb, col) in [
        ("Kernel", m.kernel_kb, COLOR_SECONDARY),
        ("Process Private", m.process_private_kb, COLOR_PRIMARY),
        ("Process Shared", m.process_shared_kb, COLOR_PRIMARY),
        ("Page Cache", m.cache_kb, COLOR_PRIMARY),
        ("Buffers", m.buffers_kb, COLOR_PRIMARY),
        ("Slab", m.slab_kb, COLOR_PRIMARY),
        ("Page Tables", m.page_tables_kb, COLOR_PRIMARY),
        ("Free", m.free_kb, COLOR_SECONDARY),
    ] {
        let p = pct(kb, total);
        let bar = "#".repeat(((p.clamp(0.0, 100.0) / 100.0) * 45.0) as usize);
        lines.push(Line::from(vec![
            Span::styled(format!("{lbl:16} "), Style::default().fg(col)),
            Span::raw(format!("{:7.1} GiB ({:5.1}%) ", to_gib(kb), p)),
            Span::styled(bar, Style::default().fg(col)),
        ]));
    }
    if !app.state.numa_nodes.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("NUMA Nodes:", hdr)));
        for n in &app.state.numa_nodes {
            lines.push(Line::from(format!(
                "  Node {}: {:.1} / {:.1} GiB ({:.1}%)",
                n.node_id,
                to_gib(n.mem_used_kb),
                to_gib(n.mem_total_kb),
                pct(n.mem_used_kb, n.mem_total_kb)
            )));
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Physical Memory Map"),
        ),
        area,
    );
}

fn draw_shared_view(f: &mut Frame, area: Rect, app: &App) {
    let s = &app.state.shared_memory;
    let (pri, sec) = (
        Style::default().fg(COLOR_PRIMARY),
        Style::default().fg(COLOR_SECONDARY),
    );
    let lines = vec![
        Line::from(vec![
            Span::styled("Total Shared Memory: ", sec),
            Span::raw(format!("{:.1} GiB", to_gib(s.total_shared_kb))),
        ]),
        Line::from(vec![
            Span::styled("  Clean: ", pri),
            Span::raw(format!("{:.1} GiB", to_gib(s.total_shared_clean_kb))),
        ]),
        Line::from(vec![
            Span::styled("  Dirty: ", pri),
            Span::raw(format!("{:.1} GiB", to_gib(s.total_shared_dirty_kb))),
        ]),
        Line::from(vec![
            Span::styled("Sharing Efficiency: ", sec),
            Span::raw(format!("{:.1}%", s.sharing_efficiency)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Shared Memory Analysis"),
        ),
        area,
    );
}

fn draw_help(f: &mut Frame, area: Rect, app: &App) {
    let sec = Style::default().fg(COLOR_SECONDARY);
    if app.searching {
        let line = Line::from(vec![
            Span::styled("Search: ", sec.add_modifier(Modifier::BOLD)),
            Span::raw(&app.search_query),
            Span::styled("_", sec),
            Span::raw("  (Enter: lock | Esc: clear)"),
        ]);
        f.render_widget(
            Paragraph::new(vec![line])
                .block(Block::default().borders(Borders::ALL).title("Filter")),
            area,
        );
        return;
    }
    let next_v = match app.view_mode {
        ViewMode::Processes => "map",
        ViewMode::MemoryMap => "shared",
        ViewMode::SharedMemory => "process",
    };
    let line = Line::from(vec![
        Span::raw(
            "q: quit | Enter: inspect | b/Space: base~ | /: search | g: group | k: kill | n: sort | v: ",
        ),
        Span::styled(next_v, sec),
    ]);
    f.render_widget(
        Paragraph::new(vec![line]).block(Block::default().borders(Borders::ALL).title("Controls")),
        area,
    );
}

fn centered_rect(px: u16, py: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - py) / 2),
            Constraint::Percentage(py),
            Constraint::Percentage((100 - py) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - px) / 2),
            Constraint::Percentage(px),
            Constraint::Percentage((100 - px) / 2),
        ])
        .split(v[1])[1]
}
