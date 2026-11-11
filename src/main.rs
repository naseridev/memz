use anyhow::Result;
use memz::{engine::Engine, tui::Tui};

fn main() -> Result<()> {
    let engine = Engine::new()?;
    let mut tui = Tui::new(engine)?;
    tui.run()
}
