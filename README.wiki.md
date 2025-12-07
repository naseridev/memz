# MEMZ Technical Wiki

This document provides comprehensive technical details about MEMZ's architecture, implementation, and Linux memory subsystem interactions.

---

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Module Breakdown](#module-breakdown)
3. [Data Flow](#data-flow)
4. [Linux Memory Subsystem](#linux-memory-subsystem)
5. [Implementation Details](#implementation-details)
6. [Design Decisions](#design-decisions)
7. [Performance Considerations](#performance-considerations)
8. [Limitations and Trade-offs](#limitations-and-trade-offs)

---

## Architecture Overview

MEMZ follows a three-layer architecture:

```
+------------------------------------------+
|         User Interface (ui.rs)           |
|    TUI rendering, input handling         |
+------------------------------------------+
                    |
                    | AnalyzedState
                    |
+------------------------------------------+
|       Analyzer (analyzer.rs)             |
|   Aggregation, computation, tracking     |
+------------------------------------------+
                    |
                    | MemorySnapshot
                    |
+------------------------------------------+
|      Collector (collector.rs)            |
|   /proc parsing, raw data extraction     |
+------------------------------------------+
                    |
                    |
              Linux procfs
```

**Separation of Concerns**:
- `collector`: Raw data acquisition from kernel
- `analyzer`: Computation and state management
- `ui`: Presentation and user interaction

This design allows independent testing and modification of each layer.

---

## Module Breakdown

### collector.rs

**Purpose**: Extract raw memory data from Linux procfs.

**Key Structures**:

- `MemorySnapshot`: Container for system, process, and NUMA data at a single point in time
- `ProcessMemory`: Per-process memory statistics from `smaps_rollup`
- `SystemMemory`: System-wide statistics from `/proc/meminfo`
- `NumaNode`: Per-node memory data from `/sys/devices/system/node/nodeN/meminfo`

**Data Sources**:

1. `/proc/meminfo`: System totals (MemTotal, MemFree, Cached, etc.)
2. `/proc/[pid]/smaps_rollup`: Aggregated per-process memory maps
3. `/proc/[pid]/comm`: Process names
4. `/sys/devices/system/node/nodeN/meminfo`: NUMA topology

**Why smaps_rollup?**

Traditional `/proc/[pid]/smaps` provides detailed per-VMA (Virtual Memory Area) breakdowns but requires parsing potentially thousands of lines per process. `smaps_rollup` (kernel 4.14+) provides the same aggregated data in ~10 lines, reducing I/O and parsing overhead by 100-1000x.

**Collection Strategy**:

- Single pass through `/proc` to enumerate PIDs
- Parallel reads would help on systems with 1000+ processes, but current implementation is sequential for simplicity
- Failed reads (process died mid-collection) are silently ignored
- PID tracking in `known_pids` enables future delta calculations (currently unused)

### analyzer.rs

**Purpose**: Transform raw snapshots into actionable statistics.

**Key Structures**:

- `AnalyzedState`: Final output combining all computed metrics
- `ProcessStats`: Per-process aggregations with deltas
- `SystemStats`: System-level totals and utilization
- `SharedMemoryStats`: Sharing efficiency analysis
- `MemoryMap`: Physical memory distribution breakdown

**Computations**:

1. **Process Analysis** (`analyze_processes`):
   - Aggregates `shared_clean + shared_dirty` into `shared_kb`
   - Aggregates `private_clean + private_dirty` into `private_kb`
   - Calculates PSS delta by comparing current snapshot with `process_history`
   - Delta enables trend detection (memory growth/shrinkage)

2. **System Analysis** (`analyze_system`):
   - Sums all process PSS values to get `total_process_pss_kb`
   - Sums all process RSS values to get `total_process_rss_kb`
   - Computes swap usage: `swap_total - swap_free`
   - Provides both used and available memory metrics

3. **Shared Memory Analysis** (`analyze_shared_memory`):
   - Sums shared clean/dirty across all processes
   - Calculates sharing efficiency: `(RSS_total - PSS_total) / RSS_total * 100`
   - High efficiency (>30%) indicates effective memory sharing (common in systems with many processes using shared libraries)

4. **Memory Map** (`build_memory_map`):
   - Accounts for process private, cache, buffers, free, slab, page tables
   - Kernel memory calculated as residual: `total - accounted`
   - This approximation is necessary because procfs doesn't expose detailed kernel allocations

**State Management**:

- `last_snapshot`: Stores previous collection for delta calculation
- `process_history`: HashMap tracking PSS values by PID across collections
- History is pruned to current PIDs each cycle to avoid unbounded growth

### ui.rs

**Purpose**: Render TUI and handle user input.

**Framework**: ratatui (formerly tui-rs)

**Key Structures**:

- `App`: Application state container
  - Current `AnalyzedState`
  - Sort mode (PSS, RSS, Shared, PID)
  - View mode (Processes, Memory Map, Shared Memory)
  - Scroll offset for process list navigation

**Rendering Pipeline**:

1. `draw()`: Main entry point, splits screen into three chunks
2. `draw_system_stats()`: Top panel with system overview
3. View-specific renderers:
   - `draw_process_list()`: Scrollable table with process data
   - `draw_memory_map()`: Bar chart of memory distribution
   - `draw_shared_view()`: Shared memory statistics
4. `draw_help()`: Bottom panel with keybindings

**Sorting**:

Processes are re-sorted on every data update according to `sort_mode`:
- PSS (default): Largest memory consumers first
- RSS: Traditional RSS-based sorting
- Shared: Processes with most shared memory
- PID: Numerical order

**Scrolling**:

- `visible_rows` calculated dynamically from terminal height
- Scroll bounds enforced to prevent out-of-range access
- Page up/down jumps by `visible_rows`

**Color Scheme**:

- `COLOR_PRIMARY` (white): General data
- `COLOR_SECONDARY` (yellow): Labels and emphasis
- Bold styling for processes with significant PSS deltas (>10 MB change)

### main.rs

**Purpose**: Application entry point, event loop, and system checks.

**Responsibilities**:

1. **Platform Validation**:
   - Compile-time check: `#[cfg(target_os = "linux")]`
   - Runtime check: `is_root()` using `libc::geteuid()`
   - Kernel version parsing from `/proc/sys/kernel/osrelease`

2. **Terminal Setup**:
   - Enable raw mode (disable line buffering, echo)
   - Switch to alternate screen (preserves existing terminal content)
   - Enable mouse capture (currently unused)

3. **Event Loop**:
   - Poll for keyboard input with timeout
   - Refresh data every 1 second (`tick_rate`)
   - Handle quit, sort, view, scroll events
   - Redraw on every iteration

4. **Cleanup**:
   - Restore terminal state on exit (disable raw mode, leave alternate screen)
   - Show cursor (hidden during runtime)
   - Print errors to stderr after terminal restoration

**Error Handling**:

Errors propagated via `anyhow::Result`. All errors trigger cleanup before exit.

---

## Data Flow

### Initialization

```
main()
  -> check_system_requirements()
  -> setup_terminal()
  -> Collector::new()
  -> Analyzer::new()
  -> App::new()
  -> initial collection
```

### Runtime Loop (every 1 second)

```
collector.collect()
  -> parse /proc/meminfo
  -> parse /sys/devices/system/node/*/meminfo
  -> enumerate /proc/[pid] directories
  -> parse /proc/[pid]/smaps_rollup for each PID
  -> return MemorySnapshot

analyzer.update(snapshot)
  -> store snapshot

analyzer.get_state()
  -> analyze_processes() -> ProcessStats[]
  -> analyze_system() -> SystemStats
  -> analyze_shared_memory() -> SharedMemoryStats
  -> build_memory_map() -> MemoryMap
  -> return AnalyzedState

app.update_data(state)
  -> sort processes by current mode
  -> store state

terminal.draw()
  -> ui::draw() -> renders all panels
```

---

## Linux Memory Subsystem

Understanding MEMZ requires knowledge of Linux memory management.

### Virtual Memory Areas (VMAs)

Each process has a set of VMAs representing memory regions (code, data, heap, stack, mmap'd files, shared libraries). The kernel tracks per-VMA statistics in `/proc/[pid]/smaps`.

### Page Categories

Pages in a VMA are classified as:

- **Private Clean**: Unmodified pages unique to the process (e.g., unused heap pages)
- **Private Dirty**: Modified pages unique to the process (e.g., stack, active heap)
- **Shared Clean**: Unmodified pages shared between processes (e.g., libc code)
- **Shared Dirty**: Modified pages shared between processes (e.g., System V shared memory)

### RSS vs PSS

**RSS**: Sum of all pages in physical memory for a process. Shared pages are fully counted for each process.

**PSS**: Adjusts for sharing. If a page is shared by N processes, each accounts for 1/N of that page.

Example:
- Process A: 100 MB private + 50 MB shared (shared by 2 processes)
- Process B: 200 MB private + 50 MB shared (same 50 MB)

RSS: A=150 MB, B=250 MB, Total=400 MB
PSS: A=125 MB, B=225 MB, Total=350 MB (matches physical usage)

### Kernel Memory

Not tracked in procfs at per-process granularity. MEMZ estimates kernel memory as:

```
kernel_kb = total_kb - (process_private + cache + buffers + free + slab + page_tables)
```

This includes:
- Kernel code/data
- DMA buffers
- Network buffers
- Unaccounted allocations

### Swap

Linux can swap anonymous pages (heap, stack) to disk. `Swap:` in `smaps_rollup` shows swapped-out memory for each process. Note: file-backed pages (mapped files) are not "swapped" - they're simply discarded and re-read on access.

### NUMA

On multi-socket systems, memory is divided across NUMA nodes. Each CPU socket has local memory. Accessing remote memory incurs latency penalty. MEMZ reads per-node statistics but doesn't track per-process NUMA affinity (would require parsing `/proc/[pid]/numa_maps`).

---

## Implementation Details

### PSS Delta Calculation

```rust
let last_pss = self.process_history.get(&proc.pid).copied().unwrap_or(proc.pss_kb);
let pss_delta = proc.pss_kb as i64 - last_pss as i64;
```

- On first observation, `unwrap_or` defaults to current PSS, resulting in delta = 0
- On subsequent observations, calculates change since last snapshot
- Negative deltas indicate memory release
- Large deltas (>10 MB) are highlighted in the UI

### Sharing Efficiency

```rust
let efficiency = ((total_rss - total_pss) as f64 / total_rss as f64) * 100.0;
```

Represents percentage of memory saved by sharing. Example:
- RSS total: 10 GB
- PSS total: 7 GB
- Efficiency: 30% (3 GB saved via sharing)

Typical values:
- Desktop: 20-40% (many processes use shared libraries)
- Server: 10-25% (depends on workload)
- Container: 5-15% (isolated environments reduce sharing)

### Memory Map Accounting

Residual calculation assumes all non-explicitly-accounted memory belongs to the kernel. This is a simplification - some memory may be in categories not exposed by procfs.

Known gaps:
- Hardware-reserved memory
- BIOS/UEFI regions
- Memory-mapped I/O
- GPU memory (on integrated graphics)

For most workloads, these are <1% of total memory.

---

## Design Decisions

### Why Rust?

1. Memory safety without garbage collection overhead
2. Excellent libraries for terminal UI (ratatui) and error handling (anyhow)
3. Zero-cost abstractions for efficient parsing
4. Compile-time platform checks

### Why smaps_rollup?

As mentioned, 100-1000x faster than parsing full smaps. Critical for systems with many processes.

### Why 1-second refresh?

Balance between responsiveness and CPU overhead. Faster refresh (e.g., 100ms) would consume more CPU parsing procfs. Slower refresh reduces UX quality.

### Why terminal UI?

- SSH-friendly (works over remote connections)
- Low resource usage (no GUI overhead)
- Traditional for system monitoring tools (htop, top, iotop)

### Why no historical graphing?

Would require:
1. Storing time-series data in memory or on disk
2. More complex UI (charts, zoom controls)
3. Increased memory footprint

Out of scope for an academic project focused on real-time analysis.

---

## Performance Considerations

### Collection Overhead

On a system with 300 processes:
- `/proc/meminfo`: ~1ms
- NUMA nodes: ~2ms
- Process enumeration: ~5ms
- smaps_rollup parsing: ~30ms (300 x 0.1ms)

Total: ~38ms per collection

### Memory Overhead

- Base application: ~3 MB
- Per-process tracking: ~200 bytes per PID
- Snapshot storage: ~(N_procs x 200 bytes) x 2 (current + history)

For 1000 processes: ~3.4 MB total overhead

### Terminal Rendering

Ratatui uses double-buffering to minimize screen updates. Only changed cells are redrawn. Typical render time: <5ms.

### Scaling Limits

- **Process count**: Tested up to 2000 processes; collection time ~100ms
- **NUMA nodes**: No practical limit (read scales linearly)
- **Terminal size**: Works down to 80x24; optimal at 120x30+

---

## Limitations and Trade-offs

### Accuracy

- PSS is a kernel estimate, not exact accounting
- Kernel memory calculation is residual-based approximation
- Race conditions: processes can start/exit during collection (handled gracefully)

### Completeness

- Doesn't track:
  - Per-process NUMA placement
  - Transparent huge pages details
  - Memory-mapped files attribution
  - Cgroup memory limits

- These would require additional procfs parsing and UI complexity

### Security

- Requires root for full system visibility
- No authentication/authorization layer
- Assumes trusted user environment

For production use, would need:
- Capability-based access (CAP_SYS_PTRACE instead of full root)
- Audit logging
- Read-only mode

### Portability

Linux-only by design. Porting to other Unix-like systems would require:
- FreeBSD: Parse `/usr/bin/procstat -v` output
- macOS: Use `task_info()` system calls
- Windows: WMI queries or PerfMon APIs

Each has different memory models and APIs - non-trivial effort.

---

## Future Enhancements

Potential improvements (not implemented):

1. **Per-cgroup accounting**: Track memory usage by container/cgroup
2. **Historical data**: Store snapshots to file, graph trends
3. **Filtering**: Show only processes matching name pattern
4. **Diff mode**: Compare two snapshots (e.g., before/after workload)
5. **Export**: CSV/JSON output for external analysis
6. **Alerting**: Threshold-based notifications (e.g., swap usage > 80%)
7. **Flame graphs**: Visualize memory hierarchy

---

## References

- Linux kernel documentation: /proc/[pid]/smaps
- Understanding Linux memory statistics (kernel.org)
- PSS vs RSS explanation (LWN.net)
- NUMA architecture documentation
- ratatui framework documentation

---

## Glossary

- **VMA**: Virtual Memory Area, a contiguous range of virtual addresses
- **RSS**: Resident Set Size, total physical memory used by a process
- **PSS**: Proportional Set Size, RSS adjusted for shared pages
- **smaps**: Per-VMA memory statistics
- **smaps_rollup**: Aggregated smaps data (kernel 4.14+)
- **NUMA**: Non-Uniform Memory Access, multi-socket memory topology
- **procfs**: Pseudo-filesystem exposing kernel/process information
- **Slab**: Kernel memory allocator cache
- **Page tables**: Kernel structures mapping virtual to physical addresses

---

## Development Notes

### Testing

Manual testing on various distributions:
- Ubuntu 22.04 (kernel 5.15)
- Fedora 38 (kernel 6.2)
- Debian 12 (kernel 6.1)

Automated testing would require:
- Mock procfs structures
- Integration tests with known process states
- Benchmarking on systems with 100-10000 processes

### Code Style

Follows Rust standard conventions:
- `rustfmt` for formatting
- `clippy` lints enabled
- Descriptive variable names where clarity matters
- Terse names (`proc`, `mem`, `pct`) for hot loops

### Dependencies

- `ratatui 0.20+`: TUI framework
- `crossterm 0.26+`: Terminal manipulation
- `anyhow 1.0+`: Error handling
- `libc 0.2+`: System call wrappers

No unsafe code except `libc::geteuid()` call (required for root check).

---

This wiki aims to provide sufficient depth for understanding MEMZ's implementation without requiring prior kernel internals knowledge. For specific questions or clarifications, refer to inline code comments or Linux kernel documentation.