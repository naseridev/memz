# MEMZ Technical Wiki

A detailed breakdown of how `memz` collects kernel telemetry, computes regression metrics, and isolates background workloads from the terminal rendering loop.

## Table of Contents

1. [Architecture and Concurrency](#architecture-and-concurrency)
2. [Kernel Data Acquisition](#kernel-data-acquisition)
3. [Memory Math and Accounting](#memory-math-and-accounting)
4. [Statistical Leak Detection](#statistical-leak-detection)
5. [Adaptive Polling Engine](#adaptive-polling-engine)
6. [Data Pipeline and Table Projections](#data-pipeline-and-table-projections)
7. [Configuration Storage](#configuration-storage)
8. [Performance Characteristics](#performance-characteristics)

---

## Architecture and Concurrency

`memz` separates raw data extraction from terminal rendering using an asynchronous worker model connected by bounded channels.

```text
┌─────────────────────────────────────────────────────────────────┐
│                    Terminal Layer (terminal/)                   │
│                                                                 │
│   tui.rs  ───► 60 FPS Event Loop (Non-blocking crossterm poll)  │
│                Keyboard and Mouse Event Router                  │
│                                                                 │
│   ui.rs   ───► Ratatui Render Engine                            │
│                Stateless Frame Generator                        │
└────────────────────────────────┬────────────────────────────────┘
                                 │ MPSC Channels
                                 │ State Rx (sync_channel(1))
                                 │ Inspect Rx (sync_channel(1))
                                 │ Command Tx (channel())
┌────────────────────────────────┴────────────────────────────────┐
│                        App Layer (app/)                         │
│                                                                 │
│   state.rs  ──► Table Navigation, Sort and Group Projections    │
│                 Baseline Differential Computations              │
│                                                                 │
│   config.rs ──► Session State and Theme Storage (XDG / AppData) │
│                                                                 │
│   engine.rs ──► Background Worker Controller                    │
└────────────────────────────────┬────────────────────────────────┘
                                 │ Thread Handoff
                                 │ Runtime Commands (Kill, Inspect, Mode)
                                 │ String Caching with Arc<str>
┌────────────────────────────────┴────────────────────────────────┐
│                       Core Layer (core/)                        │
│                                                                 │
│   collector.rs ──► Linux procfs Parser and sysinfo Backend      │
│                    Single-Pass smaps_rollup and NUMA Reader     │
│                                                                 │
│   analyzer.rs  ──► Linear Regression Engine                     │
│                    PSI Pressure Evaluator and dt Normalization  │
└─────────────────────────────────────────────────────────────────┘
```

### Threading Model

1. **Terminal Render Loop (Main Thread)**
   - Listens for terminal events using `crossterm::event::poll` with a 16ms timeout (targetting 60 FPS).
   - Only runs full layout redraws when new telemetry data arrives or when the user interacts with the keyboard or mouse.
   - Never performs blocking file I/O or procfs reads directly on the main thread.

2. **Telemetry Engine (Worker Thread)**
   - Runs in a background OS thread spawned during initialization.
   - Periodically walks procfs, computes NUMA usage, and executes leak detection math.
   - Passes fresh `AnalyzedState` frames via a bounded `sync_channel(1)`. If the main thread is busy drawing, older frames are dropped automatically to prevent stale telemetry buildup.
   - Listens for incoming commands (`Kill`, `Inspect`, `SetTickMode`, `Shutdown`) on an unbounded control channel.

3. **String Allocation Strategy**
   - Process names and command lines are wrapped in `Arc<str>`.
   - Repeated sampling loops reuse the existing heap pointer for known processes, preventing thousands of tiny string allocations each second.

---

## Kernel Data Acquisition

### Procfs Interfaces

`memz` gathers memory metrics through several standard Linux virtual files:

- **`/proc/meminfo`**: Used for overall system metrics (such as `MemTotal`, `MemFree`, `MemAvailable`, `Cached`, `Buffers`, `Slab`, `PageTables`, `SwapTotal`, and `SwapFree`).
- **`/proc/[pid]/smaps_rollup`**: Provides aggregated memory counters for a process in roughly ten lines of text. Using `smaps_rollup` instead of parsing thousands of lines in `/proc/[pid]/smaps` reduces file I/O overhead by two to three orders of magnitude.
- **`/proc/[pid]/smaps`**: Read only when you select a process and hit `Enter`. This extracts category details (heap, stack, anonymous memory, and dynamic libraries).
- **`/proc/pressure/memory`**: Kernel Pressure Stall Information (PSI). Evaluates `avg10` numbers for both `some` (at least one process delayed by memory reclaim) and `full` (all runnable threads stalled waiting for memory).
- **`/sys/devices/system/node/node[N]/meminfo`**: Gathers hardware NUMA node capacities and usage figures.

---

## Memory Math and Accounting

### PSS vs RSS

Resident Set Size (RSS) simply adds together all physical pages mapped into a process:

$$\text{RSS}_i = \text{Private Pages}_i + \sum_{j \in \text{Shared}_i} 1$$

When $N$ processes map a shared library (such as `glibc`), standard RSS counts that library $N$ times. As a result, summing RSS across all tasks frequently exceeds total physical RAM:

$$\sum_{i=1}^{M} \text{RSS}_i \gg \text{Total System Memory}$$

Proportional Set Size (PSS) corrects for this by dividing shared pages among all processes holding an active reference:

$$\text{PSS}_i = \text{Private Pages}_i + \sum_{j \in \text{Shared}_i} \frac{1}{\text{Ref Count}_j}$$

$$\sum_{i=1}^{M} \text{PSS}_i \approx \text{Real Process RAM Usage}$$

### Calculating Kernel Memory Residuals

Linux does not provide a single counter for total kernel memory allocations. `memz` estimates kernel space by subtracting accounted user and system structures from total RAM:

$$\text{Kernel}_{\text{KB}} = \text{MemTotal} - \left( \text{Private} + \text{Shared} + \text{Cached} + \text{Buffers} + \text{Slab} + \text{PageTables} + \text{Free} \right)$$

This residual approximation covers in-kernel structures, DMA buffers, network socket queues, and device driver allocations.

---

## Statistical Leak Detection

Instead of watching for small step increases that could just be regular heap fragmentation, `memz` evaluates the slope of a rolling sample history ($n \ge 8$) using Ordinary Least Squares (OLS) regression.

### Regression Math

Given sample points $(t_i, y_i)$ where $t_i = i$ and $y_i = \text{PSS}_i$:

$$\bar{t} = \frac{n - 1}{2}, \quad \bar{y} = \frac{1}{n} \sum_{i=0}^{n-1} y_i$$

$$\text{Cov}(t, y) = \sum_{i=0}^{n-1} (t_i - \bar{t})(y_i - \bar{y})$$

$$\text{Var}(t) = \sum_{i=0}^{n-1} (t_i - \bar{t})^2, \quad \text{Var}(y) = \sum_{i=0}^{n-1} (y_i - \bar{y})^2$$

The growth slope ($m_{\text{sample}}$) and fit quality ($R^2$) are:

$$m_{\text{sample}} = \frac{\text{Cov}(t, y)}{\text{Var}(t)}, \quad R^2 = \frac{[\text{Cov}(t, y)]^2}{\text{Var}(t) \cdot \text{Var}(y)}$$

### Normalizing for Real Elapsed Time

Because the sampling rate can speed up or slow down based on system activity, the raw sample slope is normalized against the actual elapsed time ($\Delta t$):

$$\text{Rate}_{\text{growth}} = \frac{m_{\text{sample}}}{\Delta t_{\text{actual}}} \quad [\text{KB/s}]$$

### Trigger Conditions

A process is marked with a red `[LEAK?]` tag only when all four checks pass:
1. At least 8 continuous samples are on record.
2. The normalized growth rate is at least $32\text{ KB/s}$.
3. Linear correlation $R^2$ is $0.70$ or higher.
4. Temporary downward dips do not exceed one third of the overall growth span.

---

## Adaptive Polling Engine

`memz` automatically adjusts how often it queries the kernel based on memory pressure:

| Mode | Polling Interval | Why It Switches |
| :--- | :--- | :--- |
| **Eco** | $2000\text{ ms}$ | Normal conditions, no thrashing, healthy PSI indicators, and no leaks. |
| **Normal** | $1000\text{ ms}$ | User has a process inspector popup open. |
| **Fast** | $500\text{ ms}$ | Memory thrashing ($\ge 4\text{ MiB/s}$ swap delta), PSI $\text{full} \ge 1.0\%$, free RAM $< 15\%$, or active leak detected. |

You can use `+` or `-` to manually lock the engine into Fast, Normal, or Eco speeds, or return it to Auto.

---

## Data Pipeline and Table Projections

Data travels through a multi-stage pipeline before reaching the screen:

```text
Kernel procfs / sysinfo
         │
         ▼
AnalyzedState (ProcessStats[])
         │
         ├─► Substring Search (matching name or PID)
         │
         ├─► Process Grouping (combining matching binary names)
         │
         ├─► Snapshot Differentials (relative to saved baseline)
         │
         ▼
DisplayRow[] (Projected Table Model)
         │
         ▼
Ratatui Table Rendering (viewport offset and selection)
```

### Process Grouping Details

When grouping (`g`) is switched on:
- All processes sharing the same binary name are grouped into a single row.
- PSS, RSS, Shared, Private, and Swap counters are added together.
- Individual history buffers are merged chronologically to allow leak detection across the entire group.
- The PID column shows the count of running instances (for example, `12x`).

---

## Configuration Storage

Preferences are loaded at startup and saved upon exit using a simple key-value file without pulling in heavy serialization crates:

- **Linux and macOS**: `$XDG_CONFIG_HOME/memz/state.conf` (falls back to `~/.config/memz/state.conf`).
- **Windows**: `%APPDATA%\memz\state.conf` (falls back to `%USERPROFILE%\.config\memz\state.conf`).

Stored format:
```ini
theme=ember_orange
view_mode=processes
sort_mode=pss
tick_mode=auto
grouped=false
```

---

## Performance Characteristics

- **CPU Utilization**: Stays under $0.8\%$ of a single core on modern systems at 1-second polling intervals with 500 active processes.
- **Resident Memory**: Takes up between $4.5\text{ MiB}$ and $8.5\text{ MiB}$ of RAM.
- **Kernel I/O Speed**: Sequential reads of `/proc/[pid]/smaps_rollup` generally finish within $15\text{ to }30\text{ ms}$ per cycle.
- **Terminal Rendering**: Uses Ratatui double-buffering to write only changed screen cells, completing frame draws in less than $2\text{ ms}$.