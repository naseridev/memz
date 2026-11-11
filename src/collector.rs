use crate::analyzer::{
    AnalyzedState, MAX_HISTORY_SAMPLES, ProcTotals, ProcessStats, RawSystemMemory, build_state,
    detect_leak, signed_diff,
};
use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use sysinfo::{MemoryRefreshKind, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

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

struct ProcTrack {
    name: Arc<str>,
    exe: Arc<str>,
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
            if let Some(p) = self.sys.process(sys_pid) {
                let _ = p.kill();
            }
        }
    }

    pub fn inspect_pids(&self, pids: &[u32]) -> MemoryBreakdown {
        let mut total = MemoryBreakdown::default();
        for &pid in pids {
            if let Some(b) = Self::parse_smaps_breakdown(pid) {
                total.heap_kb = total.heap_kb.saturating_add(b.heap_kb);
                total.stack_kb = total.stack_kb.saturating_add(b.stack_kb);
                total.anon_kb = total.anon_kb.saturating_add(b.anon_kb);
                total.libs_kb = total.libs_kb.saturating_add(b.libs_kb);
            } else if let Some(p) = self.sys.process(Pid::from_u32(pid)) {
                total.anon_kb = total.anon_kb.saturating_add(p.memory() / 1024);
            }
        }
        total
    }

    pub fn collect(&mut self) -> Result<AnalyzedState> {
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::everything());
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_memory()
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cmd(UpdateKind::OnlyIfNotSet),
        );
        let sys_mem = self.collect_system_memory();
        let (processes, totals) = self.collect_and_analyze_processes();
        let numa_nodes = Self::collect_numa_info();

        Ok(build_state(
            processes,
            totals,
            sys_mem,
            numa_nodes,
            &mut self.system_history,
            &mut self.last_swap_used_kb,
        ))
    }

    fn collect_system_memory(&self) -> RawSystemMemory {
        let mut mem = RawSystemMemory {
            total_kb: self.sys.total_memory() / 1024,
            free_kb: self.sys.free_memory() / 1024,
            available_kb: (self.sys.available_memory() / 1024).max(self.sys.free_memory() / 1024),
            swap_total_kb: self.sys.total_swap() / 1024,
            swap_free_kb: self.sys.free_swap() / 1024,
            ..Default::default()
        };
        if let Ok(content) = fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                let mut parts = line.split_whitespace();
                if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                    let val = v.parse::<u64>().unwrap_or(0);
                    match k {
                        "Buffers:" => mem.buffers_kb = val,
                        "Cached:" => mem.cached_kb = val,
                        "Slab:" => mem.slab_kb = val,
                        "PageTables:" => mem.page_tables_kb = val,
                        _ => {}
                    }
                }
            }
        } else {
            let used = self.sys.used_memory() / 1024;
            mem.cached_kb = mem
                .total_kb
                .saturating_sub(used)
                .saturating_sub(mem.free_kb);
        }
        if let Ok(psi) = fs::read_to_string("/proc/pressure/memory") {
            for line in psi.lines() {
                let mut parts = line.split_whitespace();
                let Some(kind) = parts.next() else { continue };
                for tok in parts {
                    if let Some(v) = tok
                        .strip_prefix("avg10=")
                        .and_then(|s| s.parse::<f64>().ok())
                    {
                        match kind {
                            "some" => mem.psi_some_avg10 = v,
                            "full" => mem.psi_full_avg10 = v,
                            _ => {}
                        }
                    }
                }
            }
        }
        mem
    }

    fn collect_and_analyze_processes(&mut self) -> (Vec<ProcessStats>, ProcTotals) {
        let mut next_tracks = HashMap::with_capacity(self.sys.processes().len());
        let mut totals = ProcTotals::default();
        let mut stats = Vec::with_capacity(self.sys.processes().len());

        for (&pid, p) in self.sys.processes() {
            if p.thread_kind().is_some() || p.memory() == 0 {
                continue;
            }
            let upid = pid.as_u32();
            let fallback_rss = p.memory() / 1024;
            let fallback_swap = (p.virtual_memory() / 1024).saturating_sub(fallback_rss);
            let (rss_kb, pss_kb, sh_clean, sh_dirty, pr_clean, pr_dirty, swap_kb) =
                Self::read_smaps(upid).unwrap_or((
                    fallback_rss,
                    fallback_rss,
                    0,
                    0,
                    0,
                    fallback_rss,
                    fallback_swap,
                ));

            let mut track = self.tracks.remove(&upid).unwrap_or_else(|| {
                let raw_name = sanitize(&p.name().to_string_lossy());
                let name = if raw_name.is_empty() {
                    format!("[{upid}]")
                } else {
                    raw_name
                };
                let exe = p
                    .exe()
                    .map(|e| sanitize(&e.to_string_lossy()))
                    .unwrap_or_default();
                let cmd = sanitize(
                    &p.cmd()
                        .iter()
                        .map(|s| s.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                ProcTrack {
                    name: Arc::from(name),
                    exe: Arc::from(exe),
                    cmdline: Arc::from(cmd),
                    history: VecDeque::with_capacity(MAX_HISTORY_SAMPLES),
                }
            });

            let last_pss = track.history.back().copied().unwrap_or(pss_kb);
            let delta = signed_diff(pss_kb, last_pss);
            if track.history.len() == MAX_HISTORY_SAMPLES {
                track.history.pop_front();
            }
            track.history.push_back(pss_kb);
            let (is_leaking, leak_rate_kb) = detect_leak(&track.history);
            let pss_history = track.history.iter().copied().collect();

            let shared_kb = sh_clean.saturating_add(sh_dirty);
            let private_kb = pr_clean.saturating_add(pr_dirty);
            totals.pss_kb = totals.pss_kb.saturating_add(pss_kb);
            totals.rss_kb = totals.rss_kb.saturating_add(rss_kb);
            totals.shared_clean_kb = totals.shared_clean_kb.saturating_add(sh_clean);
            totals.shared_dirty_kb = totals.shared_dirty_kb.saturating_add(sh_dirty);
            totals.private_kb = totals.private_kb.saturating_add(private_kb);

            stats.push(ProcessStats {
                pid: upid,
                name: Arc::clone(&track.name),
                exe: Arc::clone(&track.exe),
                cmdline: Arc::clone(&track.cmdline),
                pss_kb,
                rss_kb,
                shared_kb,
                private_kb,
                swap_kb,
                pss_delta_kb: delta,
                pss_history,
                is_leaking,
                leak_rate_kb,
            });
            next_tracks.insert(upid, track);
        }
        self.tracks = next_tracks;
        (stats, totals)
    }

    fn read_smaps(pid: u32) -> Option<(u64, u64, u64, u64, u64, u64, u64)> {
        let content = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
        let (mut rss, mut pss, mut sc, mut sd, mut pc, mut pd, mut sw) = (0, 0, 0, 0, 0, 0, 0);
        for line in content.lines() {
            let mut parts = line.split_whitespace();
            if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                let val = v.parse::<u64>().unwrap_or(0);
                match k {
                    "Rss:" => rss = val,
                    "Pss:" => pss = val,
                    "Shared_Clean:" => sc = val,
                    "Shared_Dirty:" => sd = val,
                    "Private_Clean:" => pc = val,
                    "Private_Dirty:" => pd = val,
                    "Swap:" => sw = val,
                    _ => {}
                }
            }
        }
        (rss > 0 || pss > 0 || sw > 0).then_some((rss, pss, sc, sd, pc, pd, sw))
    }

    fn parse_smaps_breakdown(pid: u32) -> Option<MemoryBreakdown> {
        let content = fs::read_to_string(format!("/proc/{pid}/smaps")).ok()?;
        let (mut b, mut bucket) = (MemoryBreakdown::default(), 2u8);
        for line in content.lines() {
            if line.contains('-') && line.split_whitespace().count() >= 5 {
                let path = line.split_whitespace().nth(5).unwrap_or("");
                bucket = if path == "[heap]" {
                    0
                } else if path.starts_with("[stack") {
                    1
                } else if path.starts_with('/') {
                    3
                } else {
                    2
                };
            } else if let Some(val) = line
                .strip_prefix("Pss:")
                .and_then(|r| r.split_whitespace().next())
                .and_then(|v| v.parse::<u64>().ok())
            {
                match bucket {
                    0 => b.heap_kb = b.heap_kb.saturating_add(val),
                    1 => b.stack_kb = b.stack_kb.saturating_add(val),
                    3 => b.libs_kb = b.libs_kb.saturating_add(val),
                    _ => b.anon_kb = b.anon_kb.saturating_add(val),
                }
            }
        }
        Some(b)
    }

    fn collect_numa_info() -> Vec<NumaNode> {
        let Ok(entries) = fs::read_dir(Path::new("/sys/devices/system/node")) else {
            return Vec::new();
        };
        let mut nodes: Vec<NumaNode> = entries
            .flatten()
            .filter_map(|e| {
                let id = e
                    .file_name()
                    .to_string_lossy()
                    .strip_prefix("node")?
                    .parse::<u32>()
                    .ok()?;
                let text = fs::read_to_string(e.path().join("meminfo")).ok()?;
                let (mut n, mut has_used) = (
                    NumaNode {
                        node_id: id,
                        ..Default::default()
                    },
                    false,
                );
                for l in text.lines() {
                    let p: Vec<&str> = l.split_whitespace().collect();
                    if p.len() >= 4 {
                        let v = p[3].parse::<u64>().unwrap_or(0);
                        match p[2] {
                            "MemTotal:" => n.mem_total_kb = v,
                            "MemFree:" => n.mem_free_kb = v,
                            "MemUsed:" => {
                                n.mem_used_kb = v;
                                has_used = true;
                            }
                            _ => {}
                        }
                    }
                }
                if !has_used {
                    n.mem_used_kb = n.mem_total_kb.saturating_sub(n.mem_free_kb);
                }
                Some(n)
            })
            .collect();
        nodes.sort_by_key(|n| n.node_id);
        nodes
    }
}

fn sanitize(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
