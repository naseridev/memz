use anyhow::Result;

#[cfg(target_os = "linux")]
use crate::os_utils::is_root;

pub(crate) fn check_system_requirements() -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("Error: This tool only runs on Linux");
        eprintln!("Current OS: {}", std::env::consts::OS);
        eprintln!("");
        eprintln!("This tool requires:");
        eprintln!("  - Linux kernel 4.14+");
        eprintln!("  - /proc filesystem");
        eprintln!("  - Root privileges");
        return Err(anyhow::anyhow!(
            "Unsupported operating system: {}",
            std::env::consts::OS
        ));
    }

    #[cfg(target_os = "linux")]
    if !is_root() {
        return Err(anyhow::anyhow!(
            "This tool requires root privileges. Please run with sudo."
        ));
    }

    Ok(())
}
