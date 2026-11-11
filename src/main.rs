use anyhow::Result;
use std::time::Duration;

mod analyzer;
mod collector;
mod engine;
mod state;
mod tui;
mod ui;

const TICK_RATE: Duration = Duration::from_millis(1000);

fn main() -> Result<()> {
    let engine = engine::Engine::new(TICK_RATE)?;
    let mut tui = tui::Tui::new(engine)?;
    tui.run()
}
