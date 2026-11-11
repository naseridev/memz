use crate::app::config::{TickMode, UserConfig};
use crate::core::{
    analyzer::{AnalyzedState, PressureLevel},
    collector::{Collector, MemoryBreakdown},
};
use anyhow::Result;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

enum Command {
    Kill(Vec<u32>),
    Inspect(Vec<u32>),
    SetTickMode(TickMode),
    Shutdown,
}

pub struct Engine {
    state_rx: Receiver<AnalyzedState>,
    inspect_rx: Receiver<MemoryBreakdown>,
    cmd_tx: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new() -> Result<Self> {
        let mut collector = Collector::new()?;
        let initial_mode = UserConfig::load().tick_mode;

        let (state_tx, state_rx) = mpsc::sync_channel(1);
        let (inspect_tx, inspect_rx) = mpsc::sync_channel(1);
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            let mut inspected_pids: Vec<u32> = Vec::new();
            let mut tick_mode = initial_mode;
            let mut current_interval = Duration::from_millis(1000);
            let mut next_tick = Instant::now();

            loop {
                if Instant::now() >= next_tick {
                    if let Ok(mut state) = collector.collect() {
                        current_interval =
                            compute_interval(tick_mode, &state, !inspected_pids.is_empty());

                        state.system.tick_ms = current_interval.as_millis() as u64;
                        let _ = state_tx.try_send(state);
                    }

                    if !inspected_pids.is_empty() {
                        let _ = inspect_tx.try_send(collector.inspect_pids(&inspected_pids));
                    }

                    next_tick = Instant::now() + current_interval;
                }

                let timeout = next_tick.saturating_duration_since(Instant::now());

                match cmd_rx.recv_timeout(timeout) {
                    Ok(Command::Kill(pids)) => {
                        collector.kill_pids(&pids);
                        next_tick = Instant::now();
                    }

                    Ok(Command::Inspect(pids)) => {
                        inspected_pids = pids;
                        if !inspected_pids.is_empty() {
                            let _ = inspect_tx.try_send(collector.inspect_pids(&inspected_pids));

                            if tick_mode == TickMode::Auto {
                                current_interval =
                                    current_interval.min(Duration::from_millis(1000));
                                next_tick = next_tick.min(Instant::now() + current_interval);
                            }
                        }
                    }

                    Ok(Command::SetTickMode(mode)) => {
                        tick_mode = mode;
                        next_tick = Instant::now();
                    }

                    Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,

                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });

        Ok(Self {
            state_rx,
            inspect_rx,
            cmd_tx: Some(cmd_tx),
            worker: Some(worker),
        })
    }

    pub fn try_recv(&self) -> Option<AnalyzedState> {
        let mut latest = None;

        while let Ok(state) = self.state_rx.try_recv() {
            latest = Some(state);
        }

        latest
    }

    pub fn try_recv_inspect(&self) -> Option<MemoryBreakdown> {
        let mut latest = None;

        while let Ok(breakdown) = self.inspect_rx.try_recv() {
            latest = Some(breakdown);
        }

        latest
    }

    pub fn kill_pids(&self, pids: Vec<u32>) {
        if !pids.is_empty()
            && let Some(tx) = &self.cmd_tx
        {
            let _ = tx.send(Command::Kill(pids));
        }
    }

    pub fn inspect_pids(&self, pids: Vec<u32>) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::Inspect(pids));
        }
    }

    pub fn set_tick_mode(&self, mode: TickMode) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Command::SetTickMode(mode));
        }
    }
}

fn compute_interval(mode: TickMode, state: &AnalyzedState, inspecting: bool) -> Duration {
    match mode {
        TickMode::Fast => Duration::from_millis(500),
        TickMode::Normal => Duration::from_millis(1000),
        TickMode::Eco => Duration::from_millis(2000),
        TickMode::Auto => {
            let any_leaking = state.processes.iter().any(|p| p.is_leaking);

            if state.system.pressure_level != PressureLevel::Healthy || any_leaking {
                Duration::from_millis(500)
            } else if inspecting {
                Duration::from_millis(1000)
            } else {
                Duration::from_millis(2000)
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Command::Shutdown);
        }

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
