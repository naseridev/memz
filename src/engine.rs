use crate::{
    analyzer::{AnalyzedState, Analyzer},
    collector::Collector,
};
use anyhow::Result;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

enum Command {
    Kill(Vec<u32>),
    Shutdown,
}

pub struct Engine {
    state_rx: Receiver<AnalyzedState>,
    cmd_tx: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn new(tick_rate: Duration) -> Result<Self> {
        let mut collector = Collector::new()?;
        let mut analyzer = Analyzer::new();

        let (state_tx, state_rx) = mpsc::sync_channel(1);
        let (cmd_tx, cmd_rx) = mpsc::channel();

        let worker = thread::spawn(move || {
            loop {
                if let Ok(snapshot) = collector.collect() {
                    analyzer.update(snapshot);
                    let _ = state_tx.try_send(analyzer.get_state());
                }

                match cmd_rx.recv_timeout(tick_rate) {
                    Ok(Command::Kill(pids)) => {
                        collector.kill_pids(&pids);
                    }
                    Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });

        Ok(Self {
            state_rx,
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

    pub fn kill_pids(&self, pids: Vec<u32>) {
        if !pids.is_empty()
            && let Some(tx) = &self.cmd_tx
        {
            let _ = tx.send(Command::Kill(pids));
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
