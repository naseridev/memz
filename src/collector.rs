use anyhow::Result;
use std::fs;
use std::path::Path;
use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug, Clone, Default)]
pub struct MemorySnapshot {
    pub processes: Vec<ProcessMemory>,
    pub system: SystemMemory,
    pub numa_nodes: Vec<NumaNode>,
}

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone, Default)]
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

#[derive(Debug, Clone, Default)]
pub struct NumaNode {
    pub node_id: u32,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub mem_used_kb: u64,
}

pub struct Collector {
    sys: System,
}

impl Collector {
    pub fn new() -> Result<Self> {
        Ok(Self { sys: System::new() })
    }

    pub fn collect(&mut self) -> Result<MemorySnapshot> {
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::everything());
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );

        let system = self.collect_system_memory();
        let processes = self.collect_processes();
        let numa_nodes = self.collect_numa_info();

        Ok(MemorySnapshot {
            processes,
            system,
            numa_nodes,
        })
    }

    fn collect_system_memory(&self) -> SystemMemory {
        let mut mem = SystemMemory {
            total_kb: self.sys.total_memory() / 1024,
            free_kb: self.sys.free_memory() / 1024,
            available_kb: self.sys.available_memory() / 1024,
            swap_total_kb: self.sys.total_swap() / 1024,
            swap_free_kb: self.sys.free_swap() / 1024,
            ..Default::default()
        };

        if mem.available_kb == 0 {
            mem.available_kb = mem.free_kb;
        }

        if let Ok(content) = fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                let mut parts = line.split_whitespace();
                if let (Some(key), Some(val_str)) = (parts.next(), parts.next()) {
                    let val = val_str.parse::<u64>().unwrap_or(0);
                    match key {
                        "Buffers:" => mem.buffers_kb = val,
                        "Cached:" => mem.cached_kb = val,
                        "Slab:" => mem.slab_kb = val,
                        "PageTables:" => mem.page_tables_kb = val,
                        _ => {}
                    }
                }
            }
        } else {
            let used_kb = self.sys.used_memory() / 1024;
            mem.cached_kb = mem
                .total_kb
                .saturating_sub(used_kb)
                .saturating_sub(mem.free_kb);
        }

        mem
    }

    fn collect_processes(&self) -> Vec<ProcessMemory> {
        self.sys
            .processes()
            .iter()
            .filter_map(|(&pid, proc_info)| {
                if proc_info.thread_kind().is_some() {
                    return None;
                }

                let rss_kb = proc_info.memory() / 1024;
                if rss_kb == 0 {
                    return None;
                }

                let upid = pid.as_u32();
                let vmem_kb = proc_info.virtual_memory() / 1024;

                let raw_name = proc_info.name().to_string_lossy();
                let name = raw_name
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect::<String>();

                let proc_name = if name.is_empty() {
                    format!("[{}]", upid)
                } else {
                    name
                };

                if let Some(detailed) = Self::read_smaps(upid, proc_name.clone()) {
                    return Some(detailed);
                }

                let swap_kb = vmem_kb.saturating_sub(rss_kb);

                Some(ProcessMemory {
                    pid: upid,
                    name: proc_name,
                    rss_kb,
                    pss_kb: rss_kb,
                    shared_clean_kb: 0,
                    shared_dirty_kb: 0,
                    private_clean_kb: 0,
                    private_dirty_kb: rss_kb,
                    swap_kb,
                })
            })
            .collect()
    }

    fn read_smaps(pid: u32, name: String) -> Option<ProcessMemory> {
        let content = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;

        let mut mem = ProcessMemory {
            pid,
            name,
            ..Default::default()
        };

        for line in content.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(key), Some(val_str)) = (parts.next(), parts.next()) {
                let val = val_str.parse::<u64>().unwrap_or(0);
                match key {
                    "Rss:" => mem.rss_kb = val,
                    "Pss:" => mem.pss_kb = val,
                    "Shared_Clean:" => mem.shared_clean_kb = val,
                    "Shared_Dirty:" => mem.shared_dirty_kb = val,
                    "Private_Clean:" => mem.private_clean_kb = val,
                    "Private_Dirty:" => mem.private_dirty_kb = val,
                    "Swap:" => mem.swap_kb = val,
                    _ => {}
                }
            }
        }

        if mem.rss_kb == 0 && mem.pss_kb == 0 && mem.swap_kb == 0 {
            None
        } else {
            Some(mem)
        }
    }

    fn collect_numa_info(&self) -> Vec<NumaNode> {
        let Ok(entries) = fs::read_dir(Path::new("/sys/devices/system/node")) else {
            return Vec::new();
        };

        let mut nodes: Vec<NumaNode> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name();
                let node_id = name
                    .to_string_lossy()
                    .strip_prefix("node")?
                    .parse::<u32>()
                    .ok()?;
                let content = fs::read_to_string(entry.path().join("meminfo")).ok()?;

                let mut node = NumaNode {
                    node_id,
                    ..Default::default()
                };
                let mut has_used = false;

                for line in content.lines() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        let val = parts[3].parse::<u64>().unwrap_or(0);
                        match parts[2] {
                            "MemTotal:" => node.mem_total_kb = val,
                            "MemFree:" => node.mem_free_kb = val,
                            "MemUsed:" => {
                                node.mem_used_kb = val;
                                has_used = true;
                            }
                            _ => {}
                        }
                    }
                }

                if !has_used {
                    node.mem_used_kb = node.mem_total_kb.saturating_sub(node.mem_free_kb);
                }

                Some(node)
            })
            .collect();

        nodes.sort_by_key(|n| n.node_id);
        nodes
    }
}
