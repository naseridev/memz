use crate::{
    analyzer::{AnalyzedState, ProcessStats, detect_leak, signed_diff},
    collector::MemoryBreakdown,
    config::{THEME_PRESETS, ThemePreset, TickMode, UserConfig},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
    widgets::TableState,
};
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub const TABLE_CONSTRAINTS: [Constraint; 8] = [
    Constraint::Length(7),
    Constraint::Min(20),
    Constraint::Length(9),
    Constraint::Length(9),
    Constraint::Length(9),
    Constraint::Length(10),
    Constraint::Length(8),
    Constraint::Length(9),
];

const SORT_COLUMNS: [SortMode; 8] = [
    SortMode::Pid,
    SortMode::Name,
    SortMode::Pss,
    SortMode::Rss,
    SortMode::Shared,
    SortMode::Private,
    SortMode::Swap,
    SortMode::Delta,
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

impl SortMode {
    pub fn label(self, grouped: bool) -> &'static str {
        match self {
            Self::Pid => {
                if grouped {
                    "Count"
                } else {
                    "PID"
                }
            }
            Self::Name => "Name",
            Self::Pss => "PSS",
            Self::Rss => "RSS",
            Self::Shared => "Shared",
            Self::Private => "Private",
            Self::Swap => "Swap",
            Self::Delta => "Delta",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Pss => Self::Rss,
            Self::Rss => Self::Shared,
            Self::Shared => Self::Private,
            Self::Private => Self::Swap,
            Self::Swap => Self::Delta,
            Self::Delta => Self::Pid,
            Self::Pid => Self::Name,
            Self::Name => Self::Pss,
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Pid => "pid",
            Self::Name => "name",
            Self::Pss => "pss",
            Self::Rss => "rss",
            Self::Shared => "shared",
            Self::Private => "private",
            Self::Swap => "swap",
            Self::Delta => "delta",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "pid" => Self::Pid,
            "name" => Self::Name,
            "rss" => Self::Rss,
            "shared" => Self::Shared,
            "private" => Self::Private,
            "swap" => Self::Swap,
            "delta" => Self::Delta,
            _ => Self::Pss,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Processes,
    MemoryMap,
    SharedMemory,
}

impl ViewMode {
    pub fn next(self) -> Self {
        match self {
            Self::Processes => Self::MemoryMap,
            Self::MemoryMap => Self::SharedMemory,
            Self::SharedMemory => Self::Processes,
        }
    }

    pub fn next_label(self) -> &'static str {
        match self {
            Self::Processes => "map",
            Self::MemoryMap => "shared",
            Self::SharedMemory => "process",
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Processes => "processes",
            Self::MemoryMap => "memory_map",
            Self::SharedMemory => "shared_memory",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "memory_map" => Self::MemoryMap,
            "shared_memory" => Self::SharedMemory,
            _ => Self::Processes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DisplayRow {
    pub pid: u32,
    pub count: usize,
    pub name: Arc<str>,
    pub cmdline: Arc<str>,
    pub pss_kb: u64,
    pub rss_kb: u64,
    pub shared_kb: u64,
    pub private_kb: u64,
    pub swap_kb: u64,
    pub pss_delta_kb: i64,
    pub pss_history: Vec<u64>,
    pub is_leaking: bool,
    pub leak_rate_kb: u64,
    pub pids: Vec<u32>,
}

impl DisplayRow {
    fn from_process(proc_stat: &ProcessStats, delta_kb: i64) -> Self {
        Self {
            pid: proc_stat.pid,
            count: 1,
            name: Arc::clone(&proc_stat.name),
            cmdline: Arc::clone(&proc_stat.cmdline),
            pss_kb: proc_stat.pss_kb,
            rss_kb: proc_stat.rss_kb,
            shared_kb: proc_stat.shared_kb,
            private_kb: proc_stat.private_kb,
            swap_kb: proc_stat.swap_kb,
            pss_delta_kb: delta_kb,
            pss_history: proc_stat.pss_history.clone(),
            is_leaking: proc_stat.is_leaking,
            leak_rate_kb: proc_stat.leak_rate_kb,
            pids: vec![proc_stat.pid],
        }
    }

    fn empty_group_from(proc_stat: &ProcessStats) -> Self {
        Self {
            pid: proc_stat.pid,
            count: 0,
            name: Arc::clone(&proc_stat.name),
            cmdline: Arc::clone(&proc_stat.cmdline),
            pss_kb: 0,
            rss_kb: 0,
            shared_kb: 0,
            private_kb: 0,
            swap_kb: 0,
            pss_delta_kb: 0,
            pss_history: Vec::new(),
            is_leaking: false,
            leak_rate_kb: 0,
            pids: Vec::new(),
        }
    }

    fn merge_process(&mut self, proc_stat: &ProcessStats) {
        self.pid = self.pid.min(proc_stat.pid);
        self.count += 1;
        self.pss_kb = self.pss_kb.saturating_add(proc_stat.pss_kb);
        self.rss_kb = self.rss_kb.saturating_add(proc_stat.rss_kb);
        self.shared_kb = self.shared_kb.saturating_add(proc_stat.shared_kb);
        self.private_kb = self.private_kb.saturating_add(proc_stat.private_kb);
        self.swap_kb = self.swap_kb.saturating_add(proc_stat.swap_kb);
        self.pss_delta_kb = self.pss_delta_kb.saturating_add(proc_stat.pss_delta_kb);
        self.is_leaking |= proc_stat.is_leaking;
        self.leak_rate_kb = self.leak_rate_kb.saturating_add(proc_stat.leak_rate_kb);
        self.pids.push(proc_stat.pid);
        merge_history(&mut self.pss_history, &proc_stat.pss_history);
    }
}

#[derive(Default)]
pub struct App {
    pub state: AnalyzedState,
    pub display_rows: Vec<DisplayRow>,
    pub inspector_breakdown: Option<MemoryBreakdown>,
    pub baseline_pids: HashMap<u32, u64>,
    pub baseline_groups: HashMap<Arc<str>, u64>,
    pub baseline_system_pss_kb: Option<u64>,
    pub sort_mode: SortMode,
    pub view_mode: ViewMode,
    pub tick_mode: TickMode,
    pub theme: ThemePreset,
    pub theme_index: usize,
    pub choosing_color: bool,
    pub grouped: bool,
    pub searching: bool,
    pub inspecting: bool,
    pub search_query: String,
    pub selected_index: usize,
    pub table_state: TableState,
    pub visible_rows: usize,
    pub table_area: Rect,
    pub modal_area: Rect,
}

impl App {
    pub fn new() -> Self {
        let cfg = UserConfig::load();
        Self {
            sort_mode: cfg.sort_mode,
            view_mode: cfg.view_mode,
            tick_mode: cfg.tick_mode,
            theme: cfg.theme,
            theme_index: cfg.theme.index(),
            grouped: cfg.grouped,
            visible_rows: 20,
            ..Default::default()
        }
    }

    pub fn accent_color(&self) -> Color {
        self.theme.color()
    }

    pub fn save_config(&self) {
        let cfg = UserConfig {
            theme: self.theme,
            view_mode: self.view_mode,
            sort_mode: self.sort_mode,
            tick_mode: self.tick_mode,
            grouped: self.grouped,
        };
        cfg.save();
    }

    pub fn faster_tick(&mut self) -> TickMode {
        self.tick_mode = self.tick_mode.step_faster();
        self.save_config();
        self.tick_mode
    }

    pub fn slower_tick(&mut self) -> TickMode {
        self.tick_mode = self.tick_mode.step_slower();
        self.save_config();
        self.tick_mode
    }

    pub fn open_color_picker(&mut self) {
        self.close_inspector();
        self.searching = false;
        self.theme_index = self.theme.index();
        self.choosing_color = true;
    }

    pub fn close_color_picker(&mut self) {
        self.choosing_color = false;
        self.save_config();
    }

    pub fn move_color_selection(&mut self, delta: isize) {
        let len = THEME_PRESETS.len() as isize;
        let next = (self.theme_index as isize + delta).rem_euclid(len) as usize;

        self.theme_index = next;
        self.theme = THEME_PRESETS[next];
        self.save_config();
    }

    pub fn toggle_group(&mut self) {
        if !self.is_process_view() {
            return;
        }

        self.grouped = !self.grouped;
        self.selected_index = 0;
        self.rebuild_display_rows();
        self.save_config();
    }

    pub fn is_process_view(&self) -> bool {
        self.view_mode == ViewMode::Processes
    }

    pub fn update_data(&mut self, state: AnalyzedState) {
        self.state = state;
        self.rebuild_display_rows();
    }

    pub fn open_inspector(&mut self) {
        if self.is_process_view() && !self.display_rows.is_empty() {
            self.inspecting = true;
            self.inspector_breakdown = None;
        }
    }

    pub fn close_inspector(&mut self) {
        self.inspecting = false;
        self.inspector_breakdown = None;
    }

    pub fn toggle_baseline(&mut self) {
        self.baseline_pids.clear();
        self.baseline_groups.clear();

        if self.baseline_system_pss_kb.take().is_none() {
            for proc_stat in &self.state.processes {
                self.baseline_pids.insert(proc_stat.pid, proc_stat.pss_kb);
                let group_total = self
                    .baseline_groups
                    .entry(Arc::clone(&proc_stat.name))
                    .or_insert(0);
                *group_total = group_total.saturating_add(proc_stat.pss_kb);
            }
            self.baseline_system_pss_kb = Some(self.state.system.total_process_pss_kb);
        }

        self.rebuild_display_rows();
    }

    pub fn selected_pids(&self) -> Vec<u32> {
        if !self.is_process_view() {
            return Vec::new();
        }
        self.display_rows
            .get(self.selected_index)
            .map(|row| row.pids.clone())
            .unwrap_or_default()
    }

    pub fn next_sort(&mut self) {
        if !self.is_process_view() {
            return;
        }
        self.sort_mode = self.sort_mode.next();
        self.selected_index = 0;
        self.rebuild_display_rows();
        self.save_config();
    }

    pub fn toggle_view(&mut self) {
        self.close_inspector();
        self.choosing_color = false;
        self.searching = false;
        self.view_mode = self.view_mode.next();
        self.save_config();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if !self.is_process_view() || self.display_rows.is_empty() {
            return;
        }
        let max_index = self.display_rows.len().saturating_sub(1);
        let next_index = self
            .selected_index
            .saturating_add_signed(delta)
            .min(max_index);

        if next_index != self.selected_index {
            self.selected_index = next_index;
            self.inspector_breakdown = None;
            self.sync_table_state();
        }
    }

    pub fn handle_left_click(&mut self, column: u16, row: u16) -> bool {
        if self.choosing_color {
            if !rect_contains(self.modal_area, column, row) {
                self.close_color_picker();
                return true;
            }

            let first_item_row = self.modal_area.y.saturating_add(2);

            if row >= first_item_row {
                let idx = (row - first_item_row) as usize;

                if idx < THEME_PRESETS.len() {
                    self.theme_index = idx;
                    self.theme = THEME_PRESETS[idx];
                    self.close_color_picker();

                    return true;
                }
            }
            return false;
        }

        if self.inspecting {
            if !rect_contains(self.modal_area, column, row) {
                self.close_inspector();
                return true;
            }

            return false;
        }

        if !self.is_process_view() || !rect_contains(self.table_area, column, row) {
            return false;
        }

        let header_row = self.table_area.y.saturating_add(1);
        if row == header_row {
            return self.handle_header_click(column, header_row);
        }

        let Some(clicked_index) = self.row_index_at(row) else {
            return false;
        };

        if self.selected_index == clicked_index {
            self.open_inspector();
        } else {
            self.selected_index = clicked_index;
            self.inspector_breakdown = None;
            self.sync_table_state();
        }
        true
    }

    pub fn handle_right_click(&mut self, column: u16, row: u16) -> bool {
        if self.choosing_color {
            self.close_color_picker();
            return true;
        }

        if self.inspecting {
            self.close_inspector();
            return true;
        }

        if !self.is_process_view() || !rect_contains(self.table_area, column, row) {
            return false;
        }

        let Some(clicked_index) = self.row_index_at(row) else {
            return false;
        };

        self.selected_index = clicked_index;
        self.sync_table_state();
        self.open_inspector();

        true
    }

    fn handle_header_click(&mut self, column: u16, header_row: u16) -> bool {
        let inner_header = Rect::new(
            self.table_area.x.saturating_add(1),
            header_row,
            self.table_area.width.saturating_sub(2),
            1,
        );

        let column_rects = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(TABLE_CONSTRAINTS)
            .spacing(1)
            .split(inner_header);

        for (col_rect, mode) in column_rects.iter().zip(SORT_COLUMNS) {
            if column >= col_rect.x && column < col_rect.right() {
                self.sort_mode = mode;
                self.selected_index = 0;
                self.rebuild_display_rows();
                self.save_config();
                return true;
            }
        }

        false
    }

    fn row_index_at(&self, row: u16) -> Option<usize> {
        let first_data_row = self.table_area.y.saturating_add(3);
        let last_data_row = self.table_area.bottom().saturating_sub(1);

        if row < first_data_row || row >= last_data_row {
            return None;
        }

        let relative_row = (row - first_data_row) as usize;
        let index = self.table_state.offset().saturating_add(relative_row);

        (index < self.display_rows.len()).then_some(index)
    }

    pub fn rebuild_display_rows(&mut self) {
        let query = self.search_query.trim().to_lowercase();
        let filtered: Vec<&ProcessStats> = self
            .state
            .processes
            .iter()
            .filter(|proc_stat| matches_query(proc_stat, &query))
            .collect();

        let mut rows = if self.grouped {
            self.build_grouped_rows(&filtered)
        } else {
            self.build_individual_rows(&filtered)
        };

        let sort_mode = self.sort_mode;
        let grouped = self.grouped;

        rows.sort_unstable_by(|left, right| compare_rows(left, right, sort_mode, grouped));

        self.display_rows = rows;
        self.sync_table_state();
    }

    fn build_individual_rows(&self, processes: &[&ProcessStats]) -> Vec<DisplayRow> {
        let has_baseline = self.baseline_system_pss_kb.is_some();

        processes
            .iter()
            .map(|proc_stat| {
                let delta_kb = if has_baseline {
                    let base = self.baseline_pids.get(&proc_stat.pid).copied().unwrap_or(0);
                    signed_diff(proc_stat.pss_kb, base)
                } else {
                    proc_stat.pss_delta_kb
                };
                DisplayRow::from_process(proc_stat, delta_kb)
            })
            .collect()
    }

    fn build_grouped_rows(&self, processes: &[&ProcessStats]) -> Vec<DisplayRow> {
        let has_baseline = self.baseline_system_pss_kb.is_some();
        let dt_secs = (self.state.system.tick_ms.max(100) as f64) / 1000.0;
        let mut groups: HashMap<Arc<str>, DisplayRow> = HashMap::new();

        for proc_stat in processes {
            let entry = groups
                .entry(Arc::clone(&proc_stat.name))
                .or_insert_with(|| DisplayRow::empty_group_from(proc_stat));
            entry.merge_process(proc_stat);
        }

        for group in groups.values_mut() {
            if has_baseline {
                let base = self.baseline_groups.get(&group.name).copied().unwrap_or(0);
                group.pss_delta_kb = signed_diff(group.pss_kb, base);
            }

            if !group.is_leaking {
                let history_queue: VecDeque<u64> = group.pss_history.iter().copied().collect();
                let (is_leaking, leak_rate_kb) = detect_leak(&history_queue, dt_secs);
                group.is_leaking = is_leaking;
                group.leak_rate_kb = leak_rate_kb;
            }
        }

        groups.into_values().collect()
    }

    pub fn sync_table_state(&mut self) {
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

fn matches_query(proc_stat: &ProcessStats, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    proc_stat.name.to_lowercase().contains(query) || proc_stat.pid.to_string().contains(query)
}

fn compare_rows(
    left: &DisplayRow,
    right: &DisplayRow,
    sort_mode: SortMode,
    grouped: bool,
) -> Ordering {
    match sort_mode {
        SortMode::Pss => right
            .pss_kb
            .cmp(&left.pss_kb)
            .then_with(|| left.pid.cmp(&right.pid)),
        SortMode::Rss => right
            .rss_kb
            .cmp(&left.rss_kb)
            .then_with(|| left.pid.cmp(&right.pid)),
        SortMode::Shared => right
            .shared_kb
            .cmp(&left.shared_kb)
            .then_with(|| left.pid.cmp(&right.pid)),
        SortMode::Private => right
            .private_kb
            .cmp(&left.private_kb)
            .then_with(|| left.pid.cmp(&right.pid)),
        SortMode::Swap => right
            .swap_kb
            .cmp(&left.swap_kb)
            .then_with(|| left.pid.cmp(&right.pid)),
        SortMode::Delta => right
            .pss_delta_kb
            .saturating_abs()
            .cmp(&left.pss_delta_kb.saturating_abs())
            .then_with(|| right.pss_kb.cmp(&left.pss_kb)),
        SortMode::Name => left
            .name
            .cmp(&right.name)
            .then_with(|| right.pss_kb.cmp(&left.pss_kb)),
        SortMode::Pid => {
            if grouped {
                right
                    .count
                    .cmp(&left.count)
                    .then_with(|| left.name.cmp(&right.name))
            } else {
                left.pid.cmp(&right.pid)
            }
        }
    }
}

fn rect_contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x && column < rect.right() && row >= rect.y && row < rect.bottom()
}

fn merge_history(target: &mut Vec<u64>, source: &[u64]) {
    if source.is_empty() {
        return;
    }

    if target.len() < source.len() {
        let mut padded = vec![0u64; source.len() - target.len()];
        padded.append(target);
        *target = padded;
    }

    let offset = target.len() - source.len();

    for (target_val, &source_val) in target.iter_mut().skip(offset).zip(source) {
        *target_val = target_val.saturating_add(source_val);
    }
}
