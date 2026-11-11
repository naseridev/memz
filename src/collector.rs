use crate::analyzer::{
    AnalyzedState, MAX_HISTORY_SAMPLES, ProcTotals, ProcessStats, RawSystemMemory, build_state,
    detect_leak, signed_diff,
};
use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use sysinfo::{
    MemoryRefreshKind, Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind,
};

#[derive(Debug, Clone, Default)]
pub struct MemoryBreakdown {
    pub heap_kb: u64,
    pub stack_kb: u64,
    pub anon_kb: u64,
    pub libs_kb: u64,
}

#[derive(Debug, Clone, Default)]
pub struct NumaNode {
    pub node_id: u32,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub mem_used_kb: u64,
}

#[derive(Default)]
struct SmapsRollup {
    rss_kb: u64,
    pss_kb: u64,
    shared_clean_kb: u64,
    shared_dirty_kb: u64,
    private_clean_kb: u64,
    private_dirty_kb: u64,
    swap_kb: u64,
}

#[derive(Clone, Copy)]
enum RegionCategory {
    Heap,
    Stack,
    Anonymous,
    SharedLibrary,
}

struct ProcTrack {
    name: Arc<str>,
    cmdline: Arc<str>,
    history: VecDeque<u64>,
}

pub struct Collector {
    sys: System,
    tracks: HashMap<u32, ProcTrack>,
    system_history: VecDeque<u64>,
    last_swap_used_kb: Option<u64>,
}

impl Collector {
    pub fn new() -> Result<Self> {
        Ok(Self {
            sys: System::new(),
            tracks: HashMap::new(),
            system_history: VecDeque::with_capacity(MAX_HISTORY_SAMPLES),
            last_swap_used_kb: None,
        })
    }

    pub fn kill_pids(&mut self, pids: &[u32]) {
        for &pid in pids {
            let sys_pid = Pid::from_u32(pid);
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[sys_pid]),
                true,
                ProcessRefreshKind::nothing(),
            );
            if let Some(process) = self.sys.process(sys_pid) {
                let _ = process.kill();
            }
        }
    }

    pub fn inspect_pids(&self, pids: &[u32]) -> MemoryBreakdown {
        let mut total = MemoryBreakdown::default();

        for &pid in pids {
            if let Some(breakdown) = Self::parse_smaps_breakdown(pid) {
                total.heap_kb = total.heap_kb.saturating_add(breakdown.heap_kb);
                total.stack_kb = total.stack_kb.saturating_add(breakdown.stack_kb);
                total.anon_kb = total.anon_kb.saturating_add(breakdown.anon_kb);
                total.libs_kb = total.libs_kb.saturating_add(breakdown.libs_kb);
            } else if let Some(process) = self.sys.process(Pid::from_u32(pid)) {
                let rss_kb = process.memory() / 1024;
                total.anon_kb = total.anon_kb.saturating_add(rss_kb);
            }
        }

        total
    }

    pub fn collect(&mut self) -> Result<AnalyzedState> {
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::everything());

        let refresh_kind = ProcessRefreshKind::nothing()
            .with_memory()
            .with_cmd(UpdateKind::OnlyIfNotSet);

        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

        let system_memory = self.collect_system_memory();
        let (processes, totals) = self.collect_and_analyze_processes();
        let numa_nodes = Self::collect_numa_info();

        Ok(build_state(
            processes,
            totals,
            system_memory,
            numa_nodes,
            &mut self.system_history,
            &mut self.last_swap_used_kb,
        ))
    }

    fn collect_system_memory(&self) -> RawSystemMemory {
        let free_kb = self.sys.free_memory() / 1024;
        let available_kb = (self.sys.available_memory() / 1024).max(free_kb);

        let mut memory = RawSystemMemory {
            total_kb: self.sys.total_memory() / 1024,
            free_kb,
            available_kb,
            swap_total_kb: self.sys.total_swap() / 1024,
            swap_free_kb: self.sys.free_swap() / 1024,
            ..Default::default()
        };

        if let Ok(content) = fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                let mut parts = line.split_whitespace();
                let (Some(key), Some(val_str)) = (parts.next(), parts.next()) else {
                    continue;
                };
                let value = val_str.parse::<u64>().unwrap_or(0);
                match key {
                    "Buffers:" => memory.buffers_kb = value,
                    "Cached:" => memory.cached_kb = value,
                    "Slab:" => memory.slab_kb = value,
                    "PageTables:" => memory.page_tables_kb = value,
                    _ => {}
                }
            }
        } else {
            let used_kb = self.sys.used_memory() / 1024;
            memory.cached_kb = memory
                .total_kb
                .saturating_sub(used_kb)
                .saturating_sub(memory.free_kb);
        }

        if let Ok(psi_content) = fs::read_to_string("/proc/pressure/memory") {
            Self::parse_psi_metrics(&psi_content, &mut memory);
        }

        memory
    }

    fn parse_psi_metrics(content: &str, memory: &mut RawSystemMemory) {
        for line in content.lines() {
            let mut parts = line.split_whitespace();
            let Some(kind) = parts.next() else {
                continue;
            };
            for token in parts {
                let Some(avg_str) = token.strip_prefix("avg10=") else {
                    continue;
                };
                let Ok(value) = avg_str.parse::<f64>() else {
                    continue;
                };
                match kind {
                    "some" => memory.psi_some_avg10 = value,
                    "full" => memory.psi_full_avg10 = value,
                    _ => {}
                }
            }
        }
    }

    fn collect_and_analyze_processes(&mut self) -> (Vec<ProcessStats>, ProcTotals) {
        let process_map = self.sys.processes();
        let mut next_tracks = HashMap::with_capacity(process_map.len());
        let mut totals = ProcTotals::default();
        let mut stats = Vec::with_capacity(process_map.len());

        for (&sys_pid, process) in process_map {
            if process.thread_kind().is_some() || process.memory() == 0 {
                continue;
            }

            let pid = sys_pid.as_u32();
            let metrics = Self::read_process_metrics(pid, process);

            let mut track = self
                .tracks
                .remove(&pid)
                .unwrap_or_else(|| Self::create_track(pid, process));

            let previous_pss = track.history.back().copied().unwrap_or(metrics.pss_kb);
            let pss_delta_kb = signed_diff(metrics.pss_kb, previous_pss);

            if track.history.len() == MAX_HISTORY_SAMPLES {
                track.history.pop_front();
            }
            track.history.push_back(metrics.pss_kb);

            let (is_leaking, leak_rate_kb) = detect_leak(&track.history);
            let pss_history = track.history.iter().copied().collect();

            let shared_kb = metrics
                .shared_clean_kb
                .saturating_add(metrics.shared_dirty_kb);
            let private_kb = metrics
                .private_clean_kb
                .saturating_add(metrics.private_dirty_kb);

            totals.pss_kb = totals.pss_kb.saturating_add(metrics.pss_kb);
            totals.rss_kb = totals.rss_kb.saturating_add(metrics.rss_kb);
            totals.shared_clean_kb = totals
                .shared_clean_kb
                .saturating_add(metrics.shared_clean_kb);
            totals.shared_dirty_kb = totals
                .shared_dirty_kb
                .saturating_add(metrics.shared_dirty_kb);
            totals.private_kb = totals.private_kb.saturating_add(private_kb);

            stats.push(ProcessStats {
                pid,
                name: Arc::clone(&track.name),
                cmdline: Arc::clone(&track.cmdline),
                pss_kb: metrics.pss_kb,
                rss_kb: metrics.rss_kb,
                shared_kb,
                private_kb,
                swap_kb: metrics.swap_kb,
                pss_delta_kb,
                pss_history,
                is_leaking,
                leak_rate_kb,
            });

            next_tracks.insert(pid, track);
        }

        self.tracks = next_tracks;
        (stats, totals)
    }

    fn create_track(pid: u32, process: &Process) -> ProcTrack {
        let raw_name = sanitize(&process.name().to_string_lossy());
        let name = if raw_name.is_empty() {
            format!("[{pid}]")
        } else {
            raw_name
        };

        let joined_cmd = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let cmdline = sanitize(&joined_cmd);

        ProcTrack {
            name: Arc::from(name),
            cmdline: Arc::from(cmdline),
            history: VecDeque::with_capacity(MAX_HISTORY_SAMPLES),
        }
    }

    fn read_process_metrics(pid: u32, process: &Process) -> SmapsRollup {
        if let Some(rollup) = Self::read_smaps_rollup(pid) {
            return rollup;
        }

        let rss_kb = process.memory() / 1024;
        let virtual_kb = process.virtual_memory() / 1024;
        let swap_kb = virtual_kb.saturating_sub(rss_kb);

        SmapsRollup {
            rss_kb,
            pss_kb: rss_kb,
            shared_clean_kb: 0,
            shared_dirty_kb: 0,
            private_clean_kb: 0,
            private_dirty_kb: rss_kb,
            swap_kb,
        }
    }

    fn read_smaps_rollup(pid: u32) -> Option<SmapsRollup> {
        let path = format!("/proc/{pid}/smaps_rollup");
        let content = fs::read_to_string(path).ok()?;
        let mut rollup = SmapsRollup::default();

        for line in content.lines() {
            let mut parts = line.split_whitespace();
            let (Some(key), Some(val_str)) = (parts.next(), parts.next()) else {
                continue;
            };
            let value = val_str.parse::<u64>().unwrap_or(0);
            match key {
                "Rss:" => rollup.rss_kb = value,
                "Pss:" => rollup.pss_kb = value,
                "Shared_Clean:" => rollup.shared_clean_kb = value,
                "Shared_Dirty:" => rollup.shared_dirty_kb = value,
                "Private_Clean:" => rollup.private_clean_kb = value,
                "Private_Dirty:" => rollup.private_dirty_kb = value,
                "Swap:" => rollup.swap_kb = value,
                _ => {}
            }
        }

        if rollup.rss_kb > 0 || rollup.pss_kb > 0 || rollup.swap_kb > 0 {
            Some(rollup)
        } else {
            None
        }
    }

    fn parse_smaps_breakdown(pid: u32) -> Option<MemoryBreakdown> {
        let path = format!("/proc/{pid}/smaps");
        let content = fs::read_to_string(path).ok()?;
        let mut breakdown = MemoryBreakdown::default();
        let mut category = RegionCategory::Anonymous;

        for line in content.lines() {
            if line.contains('-') && line.split_whitespace().count() >= 5 {
                let mapping_name = line.split_whitespace().nth(5).unwrap_or("");
                category = if mapping_name == "[heap]" {
                    RegionCategory::Heap
                } else if mapping_name.starts_with("[stack") {
                    RegionCategory::Stack
                } else if mapping_name.starts_with('/') {
                    RegionCategory::SharedLibrary
                } else {
                    RegionCategory::Anonymous
                };
                continue;
            }

            let Some(rest) = line.strip_prefix("Pss:") else {
                continue;
            };
            let Some(val_str) = rest.split_whitespace().next() else {
                continue;
            };
            let Ok(pss_kb) = val_str.parse::<u64>() else {
                continue;
            };

            match category {
                RegionCategory::Heap => {
                    breakdown.heap_kb = breakdown.heap_kb.saturating_add(pss_kb);
                }
                RegionCategory::Stack => {
                    breakdown.stack_kb = breakdown.stack_kb.saturating_add(pss_kb);
                }
                RegionCategory::Anonymous => {
                    breakdown.anon_kb = breakdown.anon_kb.saturating_add(pss_kb);
                }
                RegionCategory::SharedLibrary => {
                    breakdown.libs_kb = breakdown.libs_kb.saturating_add(pss_kb);
                }
            }
        }

        Some(breakdown)
    }

    fn collect_numa_info() -> Vec<NumaNode> {
        let node_root = Path::new("/sys/devices/system/node");
        let Ok(entries) = fs::read_dir(node_root) else {
            return Vec::new();
        };

        let mut nodes = Vec::new();
        for entry in entries.flatten() {
            if let Some(node) = Self::parse_numa_node(&entry.path()) {
                nodes.push(node);
            }
        }

        nodes.sort_by_key(|node| node.node_id);
        nodes
    }

    fn parse_numa_node(node_path: &Path) -> Option<NumaNode> {
        let file_name = node_path.file_name()?.to_string_lossy();
        let node_id = file_name.strip_prefix("node")?.parse::<u32>().ok()?;
        let meminfo = fs::read_to_string(node_path.join("meminfo")).ok()?;

        let mut node = NumaNode {
            node_id,
            ..Default::default()
        };
        let mut has_explicit_used = false;

        for line in meminfo.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 {
                continue;
            }
            let value = parts[3].parse::<u64>().unwrap_or(0);
            match parts[2] {
                "MemTotal:" => node.mem_total_kb = value,
                "MemFree:" => node.mem_free_kb = value,
                "MemUsed:" => {
                    node.mem_used_kb = value;
                    has_explicit_used = true;
                }
                _ => {}
            }
        }

        if !has_explicit_used {
            node.mem_used_kb = node.mem_total_kb.saturating_sub(node.mem_free_kb);
        }

        Some(node)
    }
}

fn sanitize(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}
