use crate::{
    analyzer::{AnalyzedState, ProcessStats, detect_leak, signed_diff},
    collector::MemoryBreakdown,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::TableState,
};
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

#[derive(Debug, Clone)]
pub struct DisplayRow {
    pub pid: u32,
    pub count: usize,
    pub name: Arc<str>,
    pub exe: Arc<str>,
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
        Self {
            visible_rows: 20,
            ..Default::default()
        }
    }

    pub fn update_data(&mut self, state: AnalyzedState) {
        self.state = state;
        self.rebuild_display_rows();
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
        self.baseline_pids.clear();
        self.baseline_groups.clear();
        if self.baseline_system_pss_kb.take().is_none() {
            for p in &self.state.processes {
                self.baseline_pids.insert(p.pid, p.pss_kb);
                let e = self.baseline_groups.entry(Arc::clone(&p.name)).or_insert(0);
                *e = e.saturating_add(p.pss_kb);
            }
            self.baseline_system_pss_kb = Some(self.state.system.total_process_pss_kb);
        }
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

    pub fn move_selection(&mut self, delta: isize) {
        if self.display_rows.is_empty() {
            return;
        }
        let max_idx = self.display_rows.len().saturating_sub(1);
        let next = self
            .selected_index
            .saturating_add_signed(delta)
            .min(max_idx);
        if next != self.selected_index {
            self.selected_index = next;
            self.inspector_breakdown = None;
            self.sync_table_state();
        }
    }

    pub fn handle_left_click(&mut self, col: u16, row: u16) -> bool {
        if self.inspecting {
            if !contains(self.modal_area, col, row) {
                self.close_inspector();
                return true;
            }
            return false;
        }
        if self.view_mode != ViewMode::Processes || !contains(self.table_area, col, row) {
            return false;
        }
        let header_y = self.table_area.y.saturating_add(1);
        if row == header_y {
            let inner = Rect::new(
                self.table_area.x.saturating_add(1),
                header_y,
                self.table_area.width.saturating_sub(2),
                1,
            );
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(TABLE_CONSTRAINTS)
                .spacing(1)
                .split(inner);
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
            for (r, mode) in cols.iter().zip(modes) {
                if col >= r.x && col < r.right() {
                    self.sort_mode = mode;
                    self.selected_index = 0;
                    self.rebuild_display_rows();
                    return true;
                }
            }
            return false;
        }
        if let Some(idx) = self.row_at(row) {
            if self.selected_index == idx {
                self.open_inspector();
            } else {
                self.selected_index = idx;
                self.inspector_breakdown = None;
                self.sync_table_state();
            }
            return true;
        }
        false
    }

    pub fn handle_right_click(&mut self, col: u16, row: u16) -> bool {
        if self.inspecting {
            self.close_inspector();
            return true;
        }
        if self.view_mode == ViewMode::Processes
            && contains(self.table_area, col, row)
            && let Some(idx) = self.row_at(row)
        {
            self.selected_index = idx;
            self.sync_table_state();
            self.open_inspector();
            return true;
        }
        false
    }

    fn row_at(&self, row: u16) -> Option<usize> {
        let (start, end) = (
            self.table_area.y.saturating_add(3),
            self.table_area.bottom().saturating_sub(1),
        );
        if row >= start && row < end {
            let idx = self
                .table_state
                .offset()
                .saturating_add((row - start) as usize);
            (idx < self.display_rows.len()).then_some(idx)
        } else {
            None
        }
    }

    pub fn rebuild_display_rows(&mut self) {
        let q = self.search_query.trim().to_lowercase();
        let has_base = self.baseline_system_pss_kb.is_some();
        let filtered: Vec<&ProcessStats> = self
            .state
            .processes
            .iter()
            .filter(|p| {
                q.is_empty() || p.name.to_lowercase().contains(&q) || p.pid.to_string().contains(&q)
            })
            .collect();

        let mut rows: Vec<DisplayRow> = if self.grouped {
            let mut groups: HashMap<Arc<str>, DisplayRow> = HashMap::new();
            for p in filtered {
                let e = groups
                    .entry(Arc::clone(&p.name))
                    .or_insert_with(|| DisplayRow {
                        pid: p.pid,
                        count: 0,
                        name: Arc::clone(&p.name),
                        exe: Arc::clone(&p.exe),
                        cmdline: Arc::clone(&p.cmdline),
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
                    });
                e.pid = e.pid.min(p.pid);
                e.count += 1;
                e.pss_kb = e.pss_kb.saturating_add(p.pss_kb);
                e.rss_kb = e.rss_kb.saturating_add(p.rss_kb);
                e.shared_kb = e.shared_kb.saturating_add(p.shared_kb);
                e.private_kb = e.private_kb.saturating_add(p.private_kb);
                e.swap_kb = e.swap_kb.saturating_add(p.swap_kb);
                e.pss_delta_kb = e.pss_delta_kb.saturating_add(p.pss_delta_kb);
                e.is_leaking |= p.is_leaking;
                e.leak_rate_kb = e.leak_rate_kb.saturating_add(p.leak_rate_kb);
                e.pids.push(p.pid);
                merge_history(&mut e.pss_history, &p.pss_history);
            }
            for r in groups.values_mut() {
                if has_base {
                    r.pss_delta_kb = signed_diff(
                        r.pss_kb,
                        self.baseline_groups.get(&r.name).copied().unwrap_or(0),
                    );
                }
                if !r.is_leaking {
                    let dq: VecDeque<u64> = r.pss_history.iter().copied().collect();
                    (r.is_leaking, r.leak_rate_kb) = detect_leak(&dq);
                }
            }
            groups.into_values().collect()
        } else {
            filtered
                .into_iter()
                .map(|p| DisplayRow {
                    pid: p.pid,
                    count: 1,
                    name: Arc::clone(&p.name),
                    exe: Arc::clone(&p.exe),
                    cmdline: Arc::clone(&p.cmdline),
                    pss_kb: p.pss_kb,
                    rss_kb: p.rss_kb,
                    shared_kb: p.shared_kb,
                    private_kb: p.private_kb,
                    swap_kb: p.swap_kb,
                    pss_delta_kb: if has_base {
                        signed_diff(
                            p.pss_kb,
                            self.baseline_pids.get(&p.pid).copied().unwrap_or(0),
                        )
                    } else {
                        p.pss_delta_kb
                    },
                    pss_history: p.pss_history.clone(),
                    is_leaking: p.is_leaking,
                    leak_rate_kb: p.leak_rate_kb,
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

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.right() && row >= r.y && row < r.bottom()
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
    for (t, &s) in target.iter_mut().skip(offset).zip(source) {
        *t = t.saturating_add(s);
    }
}
