use crate::{
    analyzer::{PressureLevel, signed_diff},
    collector::MemoryBreakdown,
    config::THEME_PRESETS,
    state::{App, DisplayRow, SHORTCUT_ITEMS, TABLE_CONSTRAINTS, ViewMode},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Row, Sparkline, Table},
};

const COLOR_PRIMARY: Color = Color::White;
const COLOR_ALERT: Color = Color::LightRed;
const COLOR_HEALTHY: Color = Color::LightGreen;
const SIGNIFICANT_DELTA_KB: i64 = 10_240;

fn to_gib(kb: u64) -> f64 {
    kb as f64 / 1024.0 / 1024.0
}

fn to_mib(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

fn percentage(part: u64, total: u64) -> f64 {
    if total > 0 {
        (part as f64 / total as f64) * 100.0
    } else {
        0.0
    }
}

fn ascii_bar(percent: f64, max_width: usize) -> String {
    let ratio = percent.clamp(0.0, 100.0) / 100.0;
    let filled = (ratio * max_width as f64) as usize;
    "#".repeat(filled)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    app.table_area = layout[1];
    app.visible_rows = layout[1].height.saturating_sub(4) as usize;
    app.sync_table_state();

    draw_system_stats(frame, layout[0], app);

    match app.view_mode {
        ViewMode::Processes => draw_process_list(frame, layout[1], app),
        ViewMode::MemoryMap => draw_memory_map(frame, layout[1], app),
        ViewMode::SharedMemory => draw_shared_view(frame, layout[1], app),
    }

    draw_footer(frame, layout[2], app);

    if app.showing_shortcuts {
        app.modal_area = centered_rect(48, 48, frame.area());
        draw_shortcuts_modal(frame, app.modal_area, app);
    } else if app.choosing_color {
        app.modal_area = centered_rect(36, 44, frame.area());
        draw_color_modal(frame, app.modal_area, app);
    } else if app.inspecting && app.is_process_view() {
        app.modal_area = centered_rect(58, 52, frame.area());
        draw_inspector_modal(frame, app.modal_area, app);
    }
}

fn draw_system_stats(frame: &mut Frame, area: Rect, app: &App) {
    let sys = &app.state.system;
    let accent = app.accent_color();
    let label_style = Style::default().fg(accent);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    let (pressure_text, pressure_style) = match sys.pressure_level {
        PressureLevel::Healthy => ("HEALTHY", Style::default().fg(COLOR_HEALTHY)),
        PressureLevel::Warning => ("WARNING", label_style.add_modifier(Modifier::BOLD)),
        PressureLevel::Critical => (
            "CRITICAL / SWAPPING",
            Style::default()
                .fg(COLOR_ALERT)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let swap_rate_suffix = if sys.swap_rate_kb != 0 {
        format!(" ({:+.1} MiB/s)", sys.swap_rate_kb as f64 / 1024.0)
    } else {
        String::new()
    };

    let psi_suffix = if sys.psi_some_avg10 > 0.0 || sys.psi_full_avg10 > 0.0 {
        format!(
            " | PSI {:.1}%/{:.1}%",
            sys.psi_some_avg10, sys.psi_full_avg10
        )
    } else {
        String::new()
    };

    let baseline_suffix = app
        .baseline_system_pss_kb
        .map(|base_kb| {
            let diff_mib = signed_diff(sys.total_process_pss_kb, base_kb) as f64 / 1024.0;
            format!(" | Base~: {diff_mib:+.1} MiB")
        })
        .unwrap_or_default();

    let cache_and_buffers_kb = sys.cached_kb.saturating_add(sys.buffers_kb);

    let summary_lines = vec![
        Line::from(vec![
            Span::styled("Memory: ", label_style),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%)",
                to_gib(sys.used_kb),
                to_gib(sys.total_kb),
                percentage(sys.used_kb, sys.total_kb)
            )),
        ]),
        Line::from(vec![
            Span::styled("Available: ", label_style),
            Span::raw(format!(
                "{:.1} GiB | Cache/Buffers: {:.1} GiB",
                to_gib(sys.available_kb),
                to_gib(cache_and_buffers_kb)
            )),
        ]),
        Line::from(vec![
            Span::styled("Swap: ", label_style),
            Span::raw(format!(
                "{:.1} / {:.1} GiB ({:.1}%){}",
                to_gib(sys.swap_used_kb),
                to_gib(sys.swap_total_kb),
                percentage(sys.swap_used_kb, sys.swap_total_kb),
                swap_rate_suffix
            )),
        ]),
        Line::from(vec![
            Span::styled("Process PSS: ", label_style),
            Span::raw(format!(
                "{:.1} GiB | RSS: {:.1} GiB{}",
                to_gib(sys.total_process_pss_kb),
                to_gib(sys.total_process_rss_kb),
                baseline_suffix
            )),
        ]),
        Line::from(vec![
            Span::styled("Pressure: ", label_style),
            Span::styled(pressure_text, pressure_style),
            Span::raw(psi_suffix),
            Span::raw(" | "),
            Span::styled("Tick: ", label_style),
            Span::raw(app.tick_mode.label(sys.tick_ms)),
        ]),
    ];

    let summary_block = Block::default()
        .borders(Borders::ALL)
        .title("System Memory");
    frame.render_widget(
        Paragraph::new(summary_lines).block(summary_block),
        columns[0],
    );

    let spark_color = if sys.pressure_level == PressureLevel::Critical {
        COLOR_ALERT
    } else {
        accent
    };
    let trend_block = Block::default()
        .borders(Borders::ALL)
        .title("Memory Trend (60 Samples)");
    let sparkline = Sparkline::default()
        .block(trend_block)
        .data(&sys.history_used_kb)
        .max(sys.total_kb.max(1))
        .style(Style::default().fg(spark_color));
    frame.render_widget(sparkline, columns[1]);
}

fn draw_process_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let accent = app.accent_color();
    let header_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);

    let id_header = if app.grouped { "Count" } else { "PID" };
    let delta_header = if app.baseline_system_pss_kb.is_some() {
        "Base~"
    } else {
        "Delta"
    };

    let column_titles = [
        id_header,
        "Name",
        "PSS",
        "RSS",
        "Shared",
        "Private",
        "Swap",
        delta_header,
    ];
    let header_cells = column_titles
        .into_iter()
        .map(|title| Text::from(title).style(header_style));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let mut leaking_count = 0usize;
    let mut table_rows = Vec::with_capacity(app.display_rows.len());

    for row in &app.display_rows {
        if row.is_leaking {
            leaking_count += 1;
        }
        table_rows.push(format_process_row(row, app.grouped));
    }

    let title = format_process_table_title(app, leaking_count);
    let highlight_style = Style::default()
        .bg(accent)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);

    let table = Table::new(table_rows, TABLE_CONSTRAINTS)
        .header(header)
        .highlight_style(highlight_style)
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn format_process_row(row: &DisplayRow, grouped: bool) -> Row<'static> {
    let row_style = if row.is_leaking {
        Style::default()
            .fg(COLOR_ALERT)
            .add_modifier(Modifier::BOLD)
    } else if row.pss_delta_kb.saturating_abs() > SIGNIFICANT_DELTA_KB {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let id_cell = if grouped {
        format!("{}x", row.count)
    } else {
        row.pid.to_string()
    };

    let name_cell = if row.is_leaking {
        format!("{} [LEAK?]", row.name)
    } else {
        row.name.to_string()
    };

    let delta_mib = row.pss_delta_kb / 1024;
    let delta_cell = if delta_mib != 0 {
        format!("{delta_mib:+}")
    } else {
        String::from("-")
    };

    Row::new([
        id_cell,
        name_cell,
        format!("{} M", row.pss_kb / 1024),
        format!("{} M", row.rss_kb / 1024),
        format!("{} M", row.shared_kb / 1024),
        format!("{} M", row.private_kb / 1024),
        format!("{} M", row.swap_kb / 1024),
        delta_cell,
    ])
    .style(row_style)
}

fn format_process_table_title(app: &App, leaking_count: usize) -> String {
    let position = if app.display_rows.is_empty() {
        0
    } else {
        app.selected_index + 1
    };
    let total = app.display_rows.len();
    let sort_label = app.sort_mode.label(app.grouped);

    let mut title = format!("Processes ({position}/{total}) [Sort: {sort_label}]");

    if app.grouped {
        title.push_str(" [Grouped]");
    }
    if app.baseline_system_pss_kb.is_some() {
        title.push_str(" [Base~]");
    }
    if !app.search_query.is_empty() {
        title.push_str(&format!(" [Filter: {}]", app.search_query));
    }
    if leaking_count > 0 {
        title.push_str(&format!(" [Leaks: {leaking_count}]"));
    }

    title
}

fn draw_shortcuts_modal(frame: &mut Frame, area: Rect, app: &App) {
    let accent = app.accent_color();
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(" Shortcuts ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::with_capacity(SHORTCUT_ITEMS.len() + 3);
    lines.push(Line::from(""));

    for (idx, (key, desc)) in SHORTCUT_ITEMS.iter().enumerate() {
        let is_selected = idx == app.shortcuts_index;
        let marker = if is_selected { " > " } else { "   " };
        let (key_style, desc_style) = if is_selected {
            (
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(COLOR_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                Style::default().fg(accent),
                Style::default().fg(COLOR_PRIMARY),
            )
        };

        lines.push(Line::from(vec![
            Span::styled(
                marker,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{key:<7}"), key_style),
            Span::raw(" "),
            Span::styled(*desc, desc_style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Up/Down: select | Enter: run | Esc: close",
        Style::default().fg(accent),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_inspector_modal(frame: &mut Frame, area: Rect, app: &App) {
    let Some(proc_row) = app.display_rows.get(app.selected_index) else {
        return;
    };

    let accent = app.accent_color();
    frame.render_widget(Clear, area);

    let title = if proc_row.count > 1 {
        format!(" {} ({}x) ", proc_row.name, proc_row.count)
    } else {
        format!(" {} [PID {}] ", proc_row.name, proc_row.pid)
    };

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(title);
    let inner_area = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(4)])
        .split(inner_area);

    let label_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);

    let status_span = if proc_row.is_leaking {
        Span::styled(
            format!("LEAK (+{} KB/s)", proc_row.leak_rate_kb),
            Style::default()
                .fg(COLOR_ALERT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled("Healthy", Style::default().fg(COLOR_HEALTHY))
    };

    let breakdown = app.inspector_breakdown.clone().unwrap_or(MemoryBreakdown {
        heap_kb: 0,
        stack_kb: 0,
        anon_kb: proc_row.private_kb,
        libs_kb: proc_row.shared_kb,
    });

    let command_text = if proc_row.cmdline.is_empty() {
        proc_row.name.as_ref()
    } else {
        proc_row.cmdline.as_ref()
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Status: ", label_style),
            status_span,
            Span::raw("   "),
            Span::styled("Cmd: ", label_style),
            Span::raw(command_text),
        ]),
        Line::from(vec![
            Span::styled("Memory: ", label_style),
            Span::raw(format!(
                "PSS {:.1}M | Private {:.1}M | Shared {:.1}M | Swap {:.1}M",
                to_mib(proc_row.pss_kb),
                to_mib(proc_row.private_kb),
                to_mib(proc_row.shared_kb),
                to_mib(proc_row.swap_kb)
            )),
        ]),
        Line::from(""),
    ];

    let segments = [
        ("Heap", breakdown.heap_kb, accent),
        ("Stack", breakdown.stack_kb, COLOR_PRIMARY),
        ("Anon", breakdown.anon_kb, COLOR_PRIMARY),
        ("Libs", breakdown.libs_kb, accent),
    ];

    let pss_base = proc_row.pss_kb.max(1);
    for (segment_name, segment_kb, color) in segments {
        let ratio = percentage(segment_kb, pss_base);
        let bar = ascii_bar(ratio, 24);
        lines.push(Line::from(vec![
            Span::styled(format!("{segment_name:7} "), Style::default().fg(color)),
            Span::raw(format!("{:7.1}M ({:4.1}%) ", to_mib(segment_kb), ratio)),
            Span::styled(bar, Style::default().fg(color)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), modal_chunks[0]);

    let max_pss = proc_row
        .pss_history
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .max(1);
    let spark_color = if proc_row.is_leaking {
        COLOR_ALERT
    } else {
        accent
    };
    let sparkline = Sparkline::default()
        .block(
            Block::default()
                .borders(Borders::TOP)
                .title("60s Trend (Esc: close | k: kill | b: base~)"),
        )
        .data(&proc_row.pss_history)
        .max(max_pss)
        .style(Style::default().fg(spark_color));

    frame.render_widget(sparkline, modal_chunks[1]);
}

fn draw_color_modal(frame: &mut Frame, area: Rect, app: &App) {
    let accent = app.accent_color();
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(" Theme Color ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::with_capacity(THEME_PRESETS.len() + 3);
    lines.push(Line::from(""));

    for (idx, preset) in THEME_PRESETS.iter().enumerate() {
        let is_selected = idx == app.theme_index;
        let marker = if is_selected { " > " } else { "   " };
        let row_style = if is_selected {
            Style::default()
                .fg(preset.color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_PRIMARY)
        };

        lines.push(Line::from(vec![
            Span::styled(marker, row_style),
            Span::styled("### ", Style::default().fg(preset.color())),
            Span::styled(preset.name(), row_style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Up/Down: select | Enter/Esc: apply",
        Style::default().fg(accent),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_memory_map(frame: &mut Frame, area: Rect, app: &App) {
    let map = &app.state.memory_map;
    let total_kb = app.state.system.total_kb;
    let accent = app.accent_color();
    let header_style = Style::default().fg(accent).add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(Span::styled("Physical Memory Distribution:", header_style)),
        Line::from(""),
    ];

    let categories = [
        ("Kernel", map.kernel_kb, accent),
        ("Process Private", map.process_private_kb, COLOR_PRIMARY),
        ("Process Shared", map.process_shared_kb, COLOR_PRIMARY),
        ("Page Cache", map.cache_kb, COLOR_PRIMARY),
        ("Buffers", map.buffers_kb, COLOR_PRIMARY),
        ("Slab", map.slab_kb, COLOR_PRIMARY),
        ("Page Tables", map.page_tables_kb, COLOR_PRIMARY),
        ("Free", map.free_kb, accent),
    ];

    for (label, kb, color) in categories {
        let pct = percentage(kb, total_kb);
        let bar = ascii_bar(pct, 45);
        lines.push(Line::from(vec![
            Span::styled(format!("{label:16} "), Style::default().fg(color)),
            Span::raw(format!("{:7.1} GiB ({:5.1}%) ", to_gib(kb), pct)),
            Span::styled(bar, Style::default().fg(color)),
        ]));
    }

    if !app.state.numa_nodes.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("NUMA Nodes:", header_style)));
        for node in &app.state.numa_nodes {
            let used_pct = percentage(node.mem_used_kb, node.mem_total_kb);
            lines.push(Line::from(format!(
                "  Node {}: {:.1} / {:.1} GiB ({:.1}%)",
                node.node_id,
                to_gib(node.mem_used_kb),
                to_gib(node.mem_total_kb),
                used_pct
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Physical Memory Map");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_shared_view(frame: &mut Frame, area: Rect, app: &App) {
    let shared = &app.state.shared_memory;
    let primary = Style::default().fg(COLOR_PRIMARY);
    let secondary = Style::default().fg(app.accent_color());

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
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Shared Memory Analysis");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let accent = app.accent_color();

    if app.searching {
        let search_line = Line::from(vec![
            Span::styled(
                "Search",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(":", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(&app.search_query, Style::default().fg(COLOR_PRIMARY)),
            Span::styled("_", Style::default().fg(accent)),
            Span::styled("  |  ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                "Enter",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(":lock", Style::default().fg(COLOR_PRIMARY)),
            Span::styled("  |  ", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(
                "Esc",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(":clear", Style::default().fg(COLOR_PRIMARY)),
        ]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_PRIMARY))
            .title("Filter");
        frame.render_widget(Paragraph::new(vec![search_line]).block(block), area);
        return;
    }

    let key_style = Style::default()
        .fg(COLOR_PRIMARY)
        .add_modifier(Modifier::BOLD);
    let colon_style = Style::default().fg(COLOR_PRIMARY);
    let desc_style = Style::default().fg(COLOR_PRIMARY);
    let sep_style = Style::default().fg(COLOR_PRIMARY);

    let mut spans = Vec::new();

    if app.is_process_view() && !app.inspecting {
        spans.push(Span::styled("Enter", key_style));
        spans.push(Span::styled(":", colon_style));
        spans.push(Span::styled("inspect", desc_style));
        spans.push(Span::styled("  |  ", sep_style));

        spans.push(Span::styled("/", key_style));
        spans.push(Span::styled(":", colon_style));
        spans.push(Span::styled("search", desc_style));
        spans.push(Span::styled("  |  ", sep_style));
    } else if app.inspecting {
        spans.push(Span::styled("Esc", key_style));
        spans.push(Span::styled(":", colon_style));
        spans.push(Span::styled("close", desc_style));
        spans.push(Span::styled("  |  ", sep_style));

        spans.push(Span::styled("Up/Down", key_style));
        spans.push(Span::styled(":", colon_style));
        spans.push(Span::styled("navigate", desc_style));
        spans.push(Span::styled("  |  ", sep_style));
    }

    spans.push(Span::styled("Shift+Tab", key_style));
    spans.push(Span::styled(":", colon_style));
    spans.push(Span::styled("mode", desc_style));
    spans.push(Span::styled("  |  ", sep_style));

    spans.push(Span::styled("Ctrl+x", key_style));
    spans.push(Span::styled(":", colon_style));
    spans.push(Span::styled("shortcuts", desc_style));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_PRIMARY))
        .title("Controls");
    frame.render_widget(Paragraph::new(vec![Line::from(spans)]).block(block), area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let margin_y = (100 - percent_y) / 2;
    let margin_x = (100 - percent_x) / 2;

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(margin_y),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(margin_y),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(margin_x),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(margin_x),
        ])
        .split(vertical[1])[1]
}
