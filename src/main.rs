use crate::{
    engine::Engine, os_utils::check_kernel_version, sysreq::check_system_requirements, tui::Tui,
};
use anyhow::Result;
use std::time::Duration;

pub(crate) mod analyzer;
pub(crate) mod collector;
mod engine;
mod os_utils;
mod sysreq;
mod tui;
mod ui;

const TICK_RATE: Duration = Duration::from_millis(1000);

fn main() -> Result<()> {
    check_system_requirements()?;
    check_kernel_version()?;

    let engine = Engine::new(TICK_RATE)?;
    let mut tui = Tui::new(engine)?;
    tui.run()?;

    Ok(())
}
