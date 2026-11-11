use crate::collector::{MemorySnapshot, NumaNode, ProcessMemory, SystemMemory};
use std::collections::{HashMap, VecDeque};

const MAX_HISTORY_SAMPLES: usize = 60;
const MIN_LEAK_SAMPLES: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct AnalyzedState {
    pub processes: Vec<ProcessStats>,
    pub system: SystemStats,
    pub shared_memory: SharedMemoryStats,
    pub numa_nodes: Vec<NumaNode>,
    pub memory_map: MemoryMap,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessStats {
    pub pid: u32,
    pub name: String,
    pub exe: String,
    pub cmdline: String,
    pub pss_kb: u64,
    pub rss_kb: u64,
    pub shared_kb: u64,
    pub private_kb: u64,
    pub swap_kb: u64,
    pub pss_delta_kb: i64,
    pub pss_history: Vec<u64>,
    pub is_leaking: bool,
    pub leak_rate_kb: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SystemStats {
    pub total_kb: u64,
    pub used_kb: u64,
    pub available_kb: u64,
    pub cached_kb: u64,
    pub buffers_kb: u64,
    pub swap_total_kb: u64,
    pub swap_used_kb: u64,
    pub total_process_pss_kb: u64,
    pub total_process_rss_kb: u64,
    pub history_used_kb: Vec<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct SharedMemoryStats {
    pub total_shared_kb: u64,
    pub total_shared_clean_kb: u64,
    pub total_shared_dirty_kb: u64,
    pub sharing_efficiency: f64,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryMap {
    pub kernel_kb: u64,
    pub process_private_kb: u64,
    pub process_shared_kb: u64,
    pub cache_kb: u64,
    pub buffers_kb: u64,
    pub free_kb: u64,
    pub slab_kb: u64,
    pub page_tables_kb: u64,
}

#[derive(Default)]
pub struct Analyzer {
    last_snapshot: Option<MemorySnapshot>,
    process_history: HashMap<u32, VecDeque<u64>>,
    system_history: VecDeque<u64>,
}

impl Analyzer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, snapshot: MemorySnapshot) {
        self.last_snapshot = Some(snapshot);
    }

    pub fn get_state(&mut self) -> AnalyzedState {
        let Some(snapshot) = self.last_snapshot.take() else {
            return AnalyzedState::default();
        };

        let processes = self.analyze_processes(&snapshot.processes);
        let system = self.analyze_system(&snapshot.system, &snapshot.processes);
        let shared_memory = self.analyze_shared_memory(&snapshot.processes);
        let memory_map = self.build_memory_map(&snapshot.system, &snapshot.processes);

        AnalyzedState {
            processes,
            system,
            shared_memory,
            numa_nodes: snapshot.numa_nodes,
            memory_map,
        }
    }

    fn analyze_processes(&mut self, processes: &[ProcessMemory]) -> Vec<ProcessStats> {
        let mut new_history = HashMap::with_capacity(processes.len());

        let stats = processes
            .iter()
            .map(|proc| {
                let mut history = self
                    .process_history
                    .remove(&proc.pid)
                    .unwrap_or_else(|| VecDeque::with_capacity(MAX_HISTORY_SAMPLES));

                let last_pss = history.back().copied().unwrap_or(proc.pss_kb);

                let pss_delta_kb = if proc.pss_kb >= last_pss {
                    (proc.pss_kb - last_pss).min(i64::MAX as u64) as i64
                } else {
                    -((last_pss - proc.pss_kb).min(i64::MAX as u64) as i64)
                };

                if history.len() == MAX_HISTORY_SAMPLES {
                    history.pop_front();
                }
                history.push_back(proc.pss_kb);

                let (is_leaking, leak_rate_kb) = detect_leak(&history);
                let pss_history = history.iter().copied().collect();

                new_history.insert(proc.pid, history);

                ProcessStats {
                    pid: proc.pid,
                    name: proc.name.clone(),
                    exe: proc.exe.clone(),
                    cmdline: proc.cmdline.clone(),
                    pss_kb: proc.pss_kb,
                    rss_kb: proc.rss_kb,
                    shared_kb: proc.shared_clean_kb.saturating_add(proc.shared_dirty_kb),
                    private_kb: proc.private_clean_kb.saturating_add(proc.private_dirty_kb),
                    swap_kb: proc.swap_kb,
                    pss_delta_kb,
                    pss_history,
                    is_leaking,
                    leak_rate_kb,
                }
            })
            .collect();

        self.process_history = new_history;
        stats
    }

    fn analyze_system(
        &mut self,
        system: &SystemMemory,
        processes: &[ProcessMemory],
    ) -> SystemStats {
        let (total_pss, total_rss) = processes.iter().fold((0u64, 0u64), |(pss, rss), p| {
            (pss.saturating_add(p.pss_kb), rss.saturating_add(p.rss_kb))
        });

        let used_kb = system.total_kb.saturating_sub(system.available_kb);
        if self.system_history.len() == MAX_HISTORY_SAMPLES {
            self.system_history.pop_front();
        }
        self.system_history.push_back(used_kb);

        SystemStats {
            total_kb: system.total_kb,
            used_kb,
            available_kb: system.available_kb,
            cached_kb: system.cached_kb,
            buffers_kb: system.buffers_kb,
            swap_total_kb: system.swap_total_kb,
            swap_used_kb: system.swap_total_kb.saturating_sub(system.swap_free_kb),
            total_process_pss_kb: total_pss,
            total_process_rss_kb: total_rss,
            history_used_kb: self.system_history.iter().copied().collect(),
        }
    }

    fn analyze_shared_memory(&self, processes: &[ProcessMemory]) -> SharedMemoryStats {
        let (clean, dirty, rss, pss) =
            processes
                .iter()
                .fold((0u64, 0u64, 0u64, 0u64), |(c, d, r, p), proc| {
                    (
                        c.saturating_add(proc.shared_clean_kb),
                        d.saturating_add(proc.shared_dirty_kb),
                        r.saturating_add(proc.rss_kb),
                        p.saturating_add(proc.pss_kb),
                    )
                });

        let efficiency = if rss > 0 {
            (rss.saturating_sub(pss) as f64 / rss as f64) * 100.0
        } else {
            0.0
        };

        SharedMemoryStats {
            total_shared_kb: clean.saturating_add(dirty),
            total_shared_clean_kb: clean,
            total_shared_dirty_kb: dirty,
            sharing_efficiency: efficiency,
        }
    }

    fn build_memory_map(&self, system: &SystemMemory, processes: &[ProcessMemory]) -> MemoryMap {
        let (total_private, total_shared) =
            processes
                .iter()
                .fold((0u64, 0u64), |(priv_acc, shar_acc), p| {
                    (
                        priv_acc
                            .saturating_add(p.private_clean_kb)
                            .saturating_add(p.private_dirty_kb),
                        shar_acc
                            .saturating_add(p.shared_clean_kb)
                            .saturating_add(p.shared_dirty_kb),
                    )
                });

        let accounted = total_private
            .saturating_add(system.cached_kb)
            .saturating_add(system.buffers_kb)
            .saturating_add(system.free_kb)
            .saturating_add(system.slab_kb)
            .saturating_add(system.page_tables_kb);

        MemoryMap {
            kernel_kb: system.total_kb.saturating_sub(accounted),
            process_private_kb: total_private,
            process_shared_kb: total_shared,
            cache_kb: system.cached_kb,
            buffers_kb: system.buffers_kb,
            free_kb: system.free_kb,
            slab_kb: system.slab_kb,
            page_tables_kb: system.page_tables_kb,
        }
    }
}

pub fn detect_leak(history: &VecDeque<u64>) -> (bool, u64) {
    if history.len() < MIN_LEAK_SAMPLES {
        return (false, 0);
    }

    let first = *history.front().unwrap_or(&0);
    let last = *history.back().unwrap_or(&0);
    if last <= first {
        return (false, 0);
    }

    let total_growth = last - first;
    if total_growth < 1024 {
        return (false, 0);
    }

    let mut increases = 0usize;
    let mut decreases = 0usize;
    let mut max_seen = first;
    let mut max_drop = 0u64;

    for (&prev, &curr) in history.iter().zip(history.iter().skip(1)) {
        if curr > prev {
            increases += 1;
        } else if curr < prev {
            decreases += 1;
        }
        if curr > max_seen {
            max_seen = curr;
        } else {
            max_drop = max_drop.max(max_seen - curr);
        }
    }

    if increases <= decreases * 2 || max_drop > total_growth / 3 {
        return (false, 0);
    }

    let n = history.len() as f64;
    let mean_x = (n - 1.0) / 2.0;
    let mean_y = history.iter().sum::<u64>() as f64 / n;

    let mut num = 0.0;
    let mut den_x = 0.0;
    let mut den_y = 0.0;

    for (i, &y_val) in history.iter().enumerate() {
        let dx = i as f64 - mean_x;
        let dy = y_val as f64 - mean_y;
        num += dx * dy;
        den_x += dx * dx;
        den_y += dy * dy;
    }

    if den_x <= 0.0 || den_y <= 0.0 || num <= 0.0 {
        return (false, 0);
    }

    let slope = num / den_x;
    let r_squared = (num * num) / (den_x * den_y);

    if slope >= 32.0 && r_squared >= 0.70 {
        (true, slope.round() as u64)
    } else {
        (false, 0)
    }
}
