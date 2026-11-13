//! Geo Unlocker for Raspberry Pi: routes all traffic from selected LAN devices
//! through an OpenVPN or WireGuard server. Linux only; runs as root.

#[cfg(target_os = "linux")]
mod config;
#[cfg(target_os = "linux")]
mod controller;
#[cfg(target_os = "linux")]
mod routing;
#[cfg(target_os = "linux")]
mod tunnel;
#[cfg(target_os = "linux")]
mod updater;
#[cfg(target_os = "linux")]
mod web;

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> anyhow::Result<()> {
    use std::path::PathBuf;
    if unsafe { libc::geteuid() } != 0 {
        anyhow::bail!("geo-unlocker-pi must run as root");
    }
    let cfg_path = PathBuf::from(
        std::env::var("GEOUNLOCK_CONFIG").unwrap_or_else(|_| "/etc/geo-unlocker/config.json".into()),
    );
    let state_dir =
        PathBuf::from(std::env::var("GEOUNLOCK_STATE").unwrap_or_else(|_| "/var/lib/geo-unlocker".into()));
    std::fs::create_dir_all(&state_dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let cfg = config::Config::load(&cfg_path)?;
    let addr = format!("{}:{}", cfg.listen, cfg.port);
    let ctl = controller::Ctl::new(cfg_path, cfg, &state_dir)?;
    ctl.restore();
    updater::Updater::spawn(&ctl);
    geo_common::log(format!("version {}", updater::VERSION));
    geo_common::log(format!("listening on http://{addr}"));
    web::serve(&addr, ctl)
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("geo-unlocker-pi only runs on Linux (Raspberry Pi OS / Debian).");
    std::process::exit(1);
}
