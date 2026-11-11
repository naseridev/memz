use crate::{
    analyzer::AnalyzedState,
    collector::{Collector, MemoryBreakdown},
};
use anyhow::Result;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

enum Command {
    Kill(Vec<u32>),
    Inspect(Vec<u32>),
    Shutdown,
}

pub struct Engine {
    state_rx: Receiver<AnalyzedState>,
    inspect_rx: Receiver<MemoryBreakdown>,
    cmd_tx: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new(tick_rate: Duration) -> Result<Self> {
        let mut collector = Collector::new()?;

        let (state_tx, state_rx) = mpsc::sync_channel(1);
        let (inspect_tx, inspect_rx) = mpsc::sync_channel(1);
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            let mut inspected_pids: Vec<u32> = Vec::new();
            let mut next_tick = Instant::now();

            loop {
                if Instant::now() >= next_tick {
                    if let Ok(state) = collector.collect() {
                        let _ = state_tx.try_send(state);
                    }
                    if !inspected_pids.is_empty() {
                        let _ = inspect_tx.try_send(collector.inspect_pids(&inspected_pids));
                    }
                    next_tick = Instant::now() + tick_rate;
                }

                let timeout = next_tick.saturating_duration_since(Instant::now());
                match cmd_rx.recv_timeout(timeout) {
                    Ok(Command::Kill(pids)) => {
                        collector.kill_pids(&pids);
                    }
                    Ok(Command::Inspect(pids)) => {
                        inspected_pids = pids;
                        if !inspected_pids.is_empty() {
                            let _ = inspect_tx.try_send(collector.inspect_pids(&inspected_pids));
                        }
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
