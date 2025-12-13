use anyhow::Result;
use std::time::{Duration, Instant};
use crate::collector;
use crate::analyzer;

pub struct Engine {
    collector: collector::Collector,
    analyzer: analyzer::Analyzer,
    tick_rate: Duration,
    last_tick: Instant,
}

impl Engine {
    pub fn new(tick_rate: Duration) -> Result<Self> {
        Ok(Self {
            collector: collector::Collector::new()?,
            analyzer: analyzer::Analyzer::new(),
            tick_rate,
            last_tick: Instant::now(),
        })
    }

    pub fn should_tick(&self) -> bool {
        self.last_tick.elapsed() >= self.tick_rate
    }

    pub fn tick(&mut self) -> Result<analyzer::AnalyzedState> {
        let data = self.collector.collect()?;
        self.analyzer.update(data);
        self.last_tick = Instant::now();
        Ok(self.analyzer.get_state())
    }

    pub fn initial_state(&mut self) -> Result<analyzer::AnalyzedState> {
        let data = self.collector.collect()?;
        self.analyzer.update(data);
        Ok(self.analyzer.get_state())
    }
}
