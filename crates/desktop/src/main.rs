//! Geo Unlocker desktop app (Windows, macOS, Linux): Tor, built-in WireGuard and
//! Azure cloud VPN for this computer, plus managing the Geo Unlocker Pi.
//!
//!   geo-unlocker             open the app window (default)
//!   geo-unlocker --service   privileged background part (started by the window on macOS/Linux)
//!   geo-unlocker --cleanup   undo leftover network changes (used by the uninstaller)

// Release builds on Windows are window-only apps (no console).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use geo_unlocker::{backend, gui, platform};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();

    let result = if has("--cleanup") {
        platform::ensure_admin();
        geo_unlocker::controller::Ctl::new(platform::current()).map(|c| c.shutdown())
    } else if has("--service") {
        run_service(value("--token-file"))
    } else {
        gui::run()
    };
    if let Err(e) = result {
        geo_unlocker::fatal(&format!("{e:#}"));
        std::process::exit(1);
    }
}

/// Privileged service for the macOS/Linux window: reads its token from a file the
/// window wrote, serves the window and exits when the window goes away.
fn run_service(token_file: Option<String>) -> anyhow::Result<()> {
    let path = token_file.ok_or_else(|| anyhow::anyhow!("--service needs --token-file"))?;
    let token = std::fs::read_to_string(&path)?.trim().to_string();
    let _ = std::fs::remove_file(&path);
    if token.len() < 16 {
        anyhow::bail!("bad token");
    }
    backend::start_service(token)?;
    loop {
        std::thread::park();
    }
}
