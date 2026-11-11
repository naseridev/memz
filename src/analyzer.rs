use crate::collector::{MemorySnapshot, NumaNode, ProcessMemory, SystemMemory};
use std::collections::HashMap;

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
    pub pss_kb: u64,
    pub rss_kb: u64,
    pub shared_kb: u64,
    pub private_kb: u64,
    pub swap_kb: u64,
    pub pss_delta_kb: i64,
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
    process_history: HashMap<u32, u64>,
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
                let last_pss = self
                    .process_history
                    .get(&proc.pid)
                    .copied()
                    .unwrap_or(proc.pss_kb);

                let pss_delta_kb = if proc.pss_kb >= last_pss {
                    (proc.pss_kb - last_pss).min(i64::MAX as u64) as i64
                } else {
                    -((last_pss - proc.pss_kb).min(i64::MAX as u64) as i64)
                };

                new_history.insert(proc.pid, proc.pss_kb);

                ProcessStats {
                    pid: proc.pid,
                    name: proc.name.clone(),
                    pss_kb: proc.pss_kb,
                    rss_kb: proc.rss_kb,
                    shared_kb: proc.shared_clean_kb.saturating_add(proc.shared_dirty_kb),
                    private_kb: proc.private_clean_kb.saturating_add(proc.private_dirty_kb),
                    swap_kb: proc.swap_kb,
                    pss_delta_kb,
                }
            })
            .collect();

        self.process_history = new_history;
        stats
    }

    fn analyze_system(&self, system: &SystemMemory, processes: &[ProcessMemory]) -> SystemStats {
        let (total_pss, total_rss) = processes.iter().fold((0u64, 0u64), |(pss, rss), p| {
            (pss.saturating_add(p.pss_kb), rss.saturating_add(p.rss_kb))
        });

        SystemStats {
            total_kb: system.total_kb,
            used_kb: system.total_kb.saturating_sub(system.available_kb),
            available_kb: system.available_kb,
            cached_kb: system.cached_kb,
            buffers_kb: system.buffers_kb,
            swap_total_kb: system.swap_total_kb,
            swap_used_kb: system.swap_total_kb.saturating_sub(system.swap_free_kb),
            total_process_pss_kb: total_pss,
            total_process_rss_kb: total_rss,
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
