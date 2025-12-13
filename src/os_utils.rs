use anyhow::Result;

#[cfg(target_os = "linux")]
const KERNEL_RELEASE_PATH: &str = "/proc/sys/kernel/osrelease";

#[cfg(target_os = "linux")]
pub(crate) fn check_kernel_version() -> Result<()> {
    let (major, minor) = read_kernel_version();

    if kernel_too_old(major, minor) {
        eprintln!("Warning: Kernel version {}.{} detected", major, minor);
        eprintln!("This tool requires Linux kernel 4.14+ for smaps_rollup support");
        eprintln!("Some features may not work correctly\n");
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn read_kernel_version() -> (u32, u32) {
    let raw = std::fs::read_to_string(KERNEL_RELEASE_PATH).unwrap_or_else(|_| "0.0.0".into());

    let mut parts = raw.trim().split('.');

    let major = parts
        .next()
        .and_then(|x| x.parse::<u32>().ok())
        .unwrap_or(0);

    let minor = parts
        .next()
        .and_then(|x| x.parse::<u32>().ok())
        .unwrap_or(0);

    (major, minor)
}

#[cfg(target_os = "linux")]
pub(crate) fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[cfg(target_os = "linux")]
fn kernel_too_old(major: u32, minor: u32) -> bool {
    major < 4 || (major == 4 && minor < 14)
}

#[cfg(not(target_os = "linux"))]
fn check_kernel_version() -> Result<()> {
    Ok(())
}
