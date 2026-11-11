use anyhow::Result;
use memz::{app::engine::Engine, terminal::tui::Tui};

fn main() -> Result<()> {
    let engine = Engine::new()?;
    let mut tui = Tui::new(engine)?;
    tui.run()
}
