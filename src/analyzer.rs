use crate::collector::{MemorySnapshot, ProcessMemory, SystemMemory, NumaNode};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AnalyzedState {
    pub processes: Vec<ProcessStats>,
    pub system: SystemStats,
    pub shared_memory: SharedMemoryStats,
    pub numa_nodes: Vec<NumaNode>,
    pub memory_map: MemoryMap,
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
pub struct SharedMemoryStats {
    pub total_shared_kb: u64,
    pub total_shared_clean_kb: u64,
    pub total_shared_dirty_kb: u64,
    pub sharing_efficiency: f64,
}

#[derive(Debug, Clone)]
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

pub struct Analyzer {
    last_snapshot: Option<MemorySnapshot>,
    process_history: HashMap<u32, u64>,
}

impl Analyzer {
    pub fn new() -> Self {
        Self {
            last_snapshot: None,
            process_history: HashMap::new(),
        }
    }

    pub fn update(&mut self, snapshot: MemorySnapshot) {
        self.last_snapshot = Some(snapshot);
    }

    pub fn get_state(&mut self) -> AnalyzedState {
        let snapshot = match &self.last_snapshot {
            Some(s) => s.clone(),
            None => return self.empty_state(),
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
        let mut stats = Vec::with_capacity(processes.len());
        let mut new_history = HashMap::new();

        for proc in processes {
            let last_pss = self.process_history.get(&proc.pid).copied().unwrap_or(proc.pss_kb);
            let pss_delta = proc.pss_kb as i64 - last_pss as i64;

            stats.push(ProcessStats {
                pid: proc.pid,
                name: proc.name.clone(),
                pss_kb: proc.pss_kb,
                rss_kb: proc.rss_kb,
                shared_kb: proc.shared_clean_kb + proc.shared_dirty_kb,
                private_kb: proc.private_clean_kb + proc.private_dirty_kb,
                swap_kb: proc.swap_kb,
                pss_delta_kb: pss_delta,
            });

            new_history.insert(proc.pid, proc.pss_kb);
        }

        self.process_history = new_history;
        stats
    }

    fn analyze_system(&self, system: &SystemMemory, processes: &[ProcessMemory]) -> SystemStats {
        let total_pss: u64 = processes.iter().map(|p| p.pss_kb).sum();
        let total_rss: u64 = processes.iter().map(|p| p.rss_kb).sum();
        let swap_used = system.swap_total_kb.saturating_sub(system.swap_free_kb);

        SystemStats {
            total_kb: system.total_kb,
            used_kb: system.total_kb.saturating_sub(system.available_kb),
            available_kb: system.available_kb,
            cached_kb: system.cached_kb,
            buffers_kb: system.buffers_kb,
            swap_total_kb: system.swap_total_kb,
            swap_used_kb: swap_used,
            total_process_pss_kb: total_pss,
            total_process_rss_kb: total_rss,
        }
    }

    fn analyze_shared_memory(&self, processes: &[ProcessMemory]) -> SharedMemoryStats {
        let total_shared: u64 = processes
            .iter()
            .map(|p| p.shared_clean_kb + p.shared_dirty_kb)
            .sum();

        let total_shared_clean: u64 = processes.iter().map(|p| p.shared_clean_kb).sum();
        let total_shared_dirty: u64 = processes.iter().map(|p| p.shared_dirty_kb).sum();

        let total_rss: u64 = processes.iter().map(|p| p.rss_kb).sum();
        let total_pss: u64 = processes.iter().map(|p| p.pss_kb).sum();

        let efficiency = if total_rss > 0 {
            ((total_rss - total_pss) as f64 / total_rss as f64) * 100.0
        } else {
            0.0
        };

        SharedMemoryStats {
            total_shared_kb: total_shared,
            total_shared_clean_kb: total_shared_clean,
            total_shared_dirty_kb: total_shared_dirty,
            sharing_efficiency: efficiency,
        }
    }

    fn build_memory_map(&self, system: &SystemMemory, processes: &[ProcessMemory]) -> MemoryMap {
        let total_private: u64 = processes
            .iter()
            .map(|p| p.private_clean_kb + p.private_dirty_kb)
            .sum();

        let total_shared: u64 = processes
            .iter()
            .map(|p| p.shared_clean_kb + p.shared_dirty_kb)
            .sum();

        let accounted = total_private + system.cached_kb + system.buffers_kb + system.free_kb + system.slab_kb + system.page_tables_kb;
        let kernel = system.total_kb.saturating_sub(accounted);

        MemoryMap {
            kernel_kb: kernel,
            process_private_kb: total_private,
            process_shared_kb: total_shared,
            cache_kb: system.cached_kb,
            buffers_kb: system.buffers_kb,
            free_kb: system.free_kb,
            slab_kb: system.slab_kb,
            page_tables_kb: system.page_tables_kb,
        }
    }

    fn empty_state(&self) -> AnalyzedState {
        AnalyzedState {
            processes: Vec::new(),
            system: SystemStats {
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
            shared_memory: SharedMemoryStats {
                total_shared_kb: 0,
                total_shared_clean_kb: 0,
                total_shared_dirty_kb: 0,
                sharing_efficiency: 0.0,
            },
            numa_nodes: Vec::new(),
            memory_map: MemoryMap {
                kernel_kb: 0,
                process_private_kb: 0,
                process_shared_kb: 0,
                cache_kb: 0,
                buffers_kb: 0,
                free_kb: 0,
                slab_kb: 0,
                page_tables_kb: 0,
            },
        }
    }
}