use crate::collector::NumaNode;
use std::collections::VecDeque;
use std::sync::Arc;

pub const MAX_HISTORY_SAMPLES: usize = 60;
const MIN_LEAK_SAMPLES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PressureLevel {
    #[default]
    Healthy,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Default)]
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
    pub swap_rate_kb: i64,
    pub total_process_pss_kb: u64,
    pub total_process_rss_kb: u64,
    pub history_used_kb: Vec<u64>,
    pub psi_some_avg10: f64,
    pub psi_full_avg10: f64,
    pub pressure_level: PressureLevel,
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

#[derive(Debug, Clone, Default)]
pub struct ProcTotals {
    pub pss_kb: u64,
    pub rss_kb: u64,
    pub shared_clean_kb: u64,
    pub shared_dirty_kb: u64,
    pub private_kb: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RawSystemMemory {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub buffers_kb: u64,
    pub cached_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
    pub slab_kb: u64,
    pub page_tables_kb: u64,
    pub psi_some_avg10: f64,
    pub psi_full_avg10: f64,
}

pub fn build_state(
    processes: Vec<ProcessStats>,
    totals: ProcTotals,
    sys: RawSystemMemory,
    numa_nodes: Vec<NumaNode>,
    system_history: &mut VecDeque<u64>,
    last_swap_used_kb: &mut Option<u64>,
) -> AnalyzedState {
    let used_kb = sys.total_kb.saturating_sub(sys.available_kb);
    if system_history.len() == MAX_HISTORY_SAMPLES {
        system_history.pop_front();
    }
    system_history.push_back(used_kb);

    let swap_used = sys.swap_total_kb.saturating_sub(sys.swap_free_kb);
    let prev_swap = last_swap_used_kb.replace(swap_used).unwrap_or(swap_used);
    let swap_rate_kb = signed_diff(swap_used, prev_swap);

    let avail_pct = if sys.total_kb > 0 {
        (sys.available_kb as f64 / sys.total_kb as f64) * 100.0
    } else {
        100.0
    };
    let thrashing = swap_rate_kb.unsigned_abs() >= 4096;
    let pressure_level = if sys.psi_full_avg10 >= 5.0
        || sys.psi_some_avg10 >= 25.0
        || avail_pct < 5.0
        || (avail_pct < 12.0 && thrashing)
    {
        PressureLevel::Critical
    } else if sys.psi_full_avg10 >= 1.0
        || sys.psi_some_avg10 >= 5.0
        || avail_pct < 15.0
        || thrashing
    {
        PressureLevel::Warning
    } else {
        PressureLevel::Healthy
    };

    let total_shared_kb = totals
        .shared_clean_kb
        .saturating_add(totals.shared_dirty_kb);
    let sharing_efficiency = if totals.rss_kb > 0 {
        (totals.rss_kb.saturating_sub(totals.pss_kb) as f64 / totals.rss_kb as f64) * 100.0
    } else {
        0.0
    };

    let accounted = totals
        .private_kb
        .saturating_add(sys.cached_kb)
        .saturating_add(sys.buffers_kb)
        .saturating_add(sys.free_kb)
        .saturating_add(sys.slab_kb)
        .saturating_add(sys.page_tables_kb);

    AnalyzedState {
        processes,
        system: SystemStats {
            total_kb: sys.total_kb,
            used_kb,
            available_kb: sys.available_kb,
            cached_kb: sys.cached_kb,
            buffers_kb: sys.buffers_kb,
            swap_total_kb: sys.swap_total_kb,
            swap_used_kb: swap_used,
            swap_rate_kb,
            total_process_pss_kb: totals.pss_kb,
            total_process_rss_kb: totals.rss_kb,
            history_used_kb: system_history.iter().copied().collect(),
            psi_some_avg10: sys.psi_some_avg10,
            psi_full_avg10: sys.psi_full_avg10,
            pressure_level,
        },
        shared_memory: SharedMemoryStats {
            total_shared_kb,
            total_shared_clean_kb: totals.shared_clean_kb,
            total_shared_dirty_kb: totals.shared_dirty_kb,
            sharing_efficiency,
        },
        memory_map: MemoryMap {
            kernel_kb: sys.total_kb.saturating_sub(accounted),
            process_private_kb: totals.private_kb,
            process_shared_kb: total_shared_kb,
            cache_kb: sys.cached_kb,
            buffers_kb: sys.buffers_kb,
            free_kb: sys.free_kb,
            slab_kb: sys.slab_kb,
            page_tables_kb: sys.page_tables_kb,
        },
        numa_nodes,
    }
}

pub fn signed_diff(curr: u64, base: u64) -> i64 {
    if curr >= base {
        (curr - base).min(i64::MAX as u64) as i64
    } else {
        -((base - curr).min(i64::MAX as u64) as i64)
    }
}

pub fn detect_leak(history: &VecDeque<u64>) -> (bool, u64) {
    let (Some(&first), Some(&last)) = (history.front(), history.back()) else {
        return (false, 0);
    };
    if history.len() < MIN_LEAK_SAMPLES || last <= first || last - first < 1024 {
        return (false, 0);
    }
    let (mut inc, mut dec, mut peak, mut max_drop) = (0usize, 0usize, first, 0u64);
    for (&prev, &curr) in history.iter().zip(history.iter().skip(1)) {
        if curr > prev {
            inc += 1;
        } else if curr < prev {
            dec += 1;
        }
        if curr > peak {
            peak = curr;
        } else {
            max_drop = max_drop.max(peak - curr);
        }
    }
    if inc <= dec * 2 || max_drop > (last - first) / 3 {
        return (false, 0);
    }
    let n = history.len() as f64;
    let (mean_x, mean_y) = ((n - 1.0) / 2.0, history.iter().sum::<u64>() as f64 / n);
    let (mut num, mut den_x, mut den_y) = (0.0, 0.0, 0.0);
    for (i, &y) in history.iter().enumerate() {
        let (dx, dy) = (i as f64 - mean_x, y as f64 - mean_y);
        num += dx * dy;
        den_x += dx * dx;
        den_y += dy * dy;
    }
    if den_x <= 0.0 || den_y <= 0.0 || num <= 0.0 {
        return (false, 0);
    }
    let (slope, r2) = (num / den_x, (num * num) / (den_x * den_y));
    if slope >= 32.0 && r2 >= 0.70 {
        (true, slope.round() as u64)
    } else {
        (false, 0)
    }
}
