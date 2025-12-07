mod collector;
mod analyzer;
mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::{Duration, Instant};

use collector::Collector;
use analyzer::Analyzer;
use ui::App;

fn main() -> Result<()> {
    check_system_requirements()?;
    check_kernel_version()?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> Result<()> {
    let mut collector = Collector::new()?;
    let mut analyzer = Analyzer::new();
    let mut app = App::new();

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(1000);

    let data = collector.collect()?;
    analyzer.update(data);
    app.update_data(analyzer.get_state());

    loop {
        terminal.draw(|f| ui::draw(f, &mut app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('n') => app.next_sort(),
                    KeyCode::Char('v') => app.toggle_view(),
                    KeyCode::Up => app.scroll_up(),
                    KeyCode::Down => app.scroll_down(),
                    KeyCode::PageUp => app.page_up(),
                    KeyCode::PageDown => app.page_down(),
                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            let data = collector.collect()?;
            analyzer.update(data);
            app.update_data(analyzer.get_state());
            last_tick = Instant::now();
        }
    }
}

fn check_system_requirements() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("Error: This tool only runs on Linux");
        eprintln!("Current OS: {}", std::env::consts::OS);
        eprintln!("");
        eprintln!("This tool requires:");
        eprintln!("  - Linux kernel 4.14+");
        eprintln!("  - /proc filesystem");
        eprintln!("  - Root privileges");
        return Err(anyhow::anyhow!("Unsupported operating system: {}", std::env::consts::OS));
    }

    #[cfg(target_os = "linux")]
    if !is_root() {
        return Err(anyhow::anyhow!("This tool requires root privileges. Please run with sudo."));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(target_os = "linux")]
fn check_kernel_version() -> Result<()> {
    let sys_info = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| String::from("0.0.0"));

    let version_parts: Vec<&str> = sys_info.trim().split('.').collect();

    if version_parts.len() >= 2 {
        if let (Ok(major), Ok(minor)) = (
            version_parts[0].parse::<u32>(),
            version_parts[1].parse::<u32>()
        ) {
            if major < 4 || (major == 4 && minor < 14) {
                eprintln!("Warning: Kernel version {}.{} detected", major, minor);
                eprintln!("This tool requires Linux kernel 4.14+ for smaps_rollup support");
                eprintln!("Some features may not work correctly");
                eprintln!("");
            }
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn check_kernel_version() -> Result<()> {
    Ok(())
}