use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    pub processes: Vec<ProcessMemory>,
    pub system: SystemMemory,
    pub numa_nodes: Vec<NumaNode>,
}

#[derive(Debug, Clone)]
pub struct ProcessMemory {
    pub pid: u32,
    pub name: String,
    pub rss_kb: u64,
    pub pss_kb: u64,
    pub shared_clean_kb: u64,
    pub shared_dirty_kb: u64,
    pub private_clean_kb: u64,
    pub private_dirty_kb: u64,
    pub swap_kb: u64,
}

#[derive(Debug, Clone)]
pub struct SystemMemory {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub slab_kb: u64,
    pub page_tables_kb: u64,
}

#[derive(Debug, Clone)]
pub struct NumaNode {
    pub node_id: u32,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub mem_used_kb: u64,
}

pub struct Collector {
    known_pids: HashSet<u32>,
    proc_path: PathBuf,
}

impl Collector {
    pub fn new() -> Result<Self> {
        Ok(Self {
            known_pids: HashSet::new(),
            proc_path: PathBuf::from("/proc"),
        })
    }

    pub fn collect(&mut self) -> Result<MemorySnapshot> {
        let system = self.collect_system_memory()?;
        let numa_nodes = self.collect_numa_info()?;
        let processes = self.collect_process_memory()?;

        Ok(MemorySnapshot {
            processes,
            system,
            numa_nodes,
        })
    }

    fn collect_system_memory(&self) -> Result<SystemMemory> {
        let content = fs::read_to_string("/proc/meminfo")
            .context("Failed to read /proc/meminfo")?;

        let mut mem = SystemMemory {
            total_kb: 0,
            free_kb: 0,
            available_kb: 0,
            buffers_kb: 0,
            cached_kb: 0,
            swap_total_kb: 0,
            swap_free_kb: 0,
            slab_kb: 0,
            page_tables_kb: 0,
        };

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }

            let value = parts[1].parse::<u64>().unwrap_or(0);

            match parts[0] {
                "MemTotal:" => mem.total_kb = value,
                "MemFree:" => mem.free_kb = value,
                "MemAvailable:" => mem.available_kb = value,
                "Buffers:" => mem.buffers_kb = value,
                "Cached:" => mem.cached_kb = value,
                "SwapTotal:" => mem.swap_total_kb = value,
                "SwapFree:" => mem.swap_free_kb = value,
                "Slab:" => mem.slab_kb = value,
                "PageTables:" => mem.page_tables_kb = value,
                _ => {}
            }
        }

        Ok(mem)
    }

    fn collect_numa_info(&self) -> Result<Vec<NumaNode>> {
        let mut nodes = Vec::new();
        let sys_node_path = PathBuf::from("/sys/devices/system/node");

        if !sys_node_path.exists() {
            return Ok(nodes);
        }

        let entries = fs::read_dir(&sys_node_path).context("Failed to read NUMA nodes")?;

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if !name_str.starts_with("node") {
                continue;
            }

            if let Some(node_id) = name_str.strip_prefix("node").and_then(|s| s.parse::<u32>().ok()) {
                let meminfo_path = entry.path().join("meminfo");
                if let Ok(content) = fs::read_to_string(meminfo_path) {
                    let mut node = NumaNode {
                        node_id,
                        mem_total_kb: 0,
                        mem_free_kb: 0,
                        mem_used_kb: 0,
                    };

                    for line in content.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() < 4 {
                            continue;
                        }

                        let value = parts[3].parse::<u64>().unwrap_or(0);

                        if line.contains("MemTotal:") {
                            node.mem_total_kb = value;
                        } else if line.contains("MemFree:") {
                            node.mem_free_kb = value;
                        } else if line.contains("MemUsed:") {
                            node.mem_used_kb = value;
                        }
                    }

                    if node.mem_used_kb == 0 {
                        node.mem_used_kb = node.mem_total_kb.saturating_sub(node.mem_free_kb);
                    }

                    nodes.push(node);
                }
            }
        }

        nodes.sort_by_key(|n| n.node_id);
        Ok(nodes)
    }

    fn collect_process_memory(&mut self) -> Result<Vec<ProcessMemory>> {
        let mut processes = Vec::new();
        let mut current_pids = HashSet::new();

        let entries = fs::read_dir(&self.proc_path).context("Failed to read /proc")?;

        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if let Ok(pid) = name_str.parse::<u32>() {
                current_pids.insert(pid);

                let smaps_path = entry.path().join("smaps_rollup");

                if let Ok(proc_mem) = self.parse_smaps_rollup(pid, &smaps_path) {
                    processes.push(proc_mem);
                }
            }
        }

        self.known_pids = current_pids;

        Ok(processes)
    }

    fn parse_smaps_rollup(&self, pid: u32, path: &PathBuf) -> Result<ProcessMemory> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read smaps_rollup for PID {}", pid))?;

        let mut mem = ProcessMemory {
            pid,
            name: self.get_process_name(pid),
            rss_kb: 0,
            pss_kb: 0,
            shared_clean_kb: 0,
            shared_dirty_kb: 0,
            private_clean_kb: 0,
            private_dirty_kb: 0,
            swap_kb: 0,
        };

        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }

            let value = parts[1].parse::<u64>().unwrap_or(0);

            match parts[0] {
                "Rss:" => mem.rss_kb = value,
                "Pss:" => mem.pss_kb = value,
                "Shared_Clean:" => mem.shared_clean_kb = value,
                "Shared_Dirty:" => mem.shared_dirty_kb = value,
                "Private_Clean:" => mem.private_clean_kb = value,
                "Private_Dirty:" => mem.private_dirty_kb = value,
                "Swap:" => mem.swap_kb = value,
                _ => {}
            }
        }

        Ok(mem)
    }

    fn get_process_name(&self, pid: u32) -> String {
        let comm_path = self.proc_path.join(pid.to_string()).join("comm");
        fs::read_to_string(comm_path)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| format!("[{}]", pid))
    }
}