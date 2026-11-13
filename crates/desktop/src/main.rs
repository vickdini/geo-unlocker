//! Geo Unlocker desktop app (Windows, macOS, Linux): Tor, VPN and self-hosted
//! cloud VPN for this computer, plus sending profiles to the Geo Unlocker Pi.
//!
//!   geo-unlocker             open the app window (default)
//!   geo-unlocker --browser   no window: serve the UI and open it in the browser
//!   geo-unlocker --service   privileged background part (started by the window on macOS/Linux)
//!   geo-unlocker --cleanup   undo leftover network changes (used by the uninstaller)

// Release builds on Windows are window-only apps (no console).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use geo_unlocker::{app, backend, platform, web};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();

    let result = if has("--cleanup") {
        // Used by the uninstaller: undo any leftover network changes, then exit.
        platform::ensure_admin();
        geo_unlocker::controller::Ctl::new(platform::current()).map(|c| c.shutdown())
    } else if has("--service") {
        run_service(value("--token-file"))
    } else if has("--browser") {
        platform::ensure_admin();
        run_browser()
    } else {
        app::run()
    };
    if let Err(e) = result {
        geo_unlocker::fatal(&format!("{e:#}"));
        std::process::exit(1);
    }
}

/// Privileged service for the macOS/Linux window: reads its token from a file the
/// window wrote, serves the API and exits when the window goes away.
fn run_service(token_file: Option<String>) -> anyhow::Result<()> {
    let path = token_file.ok_or_else(|| anyhow::anyhow!("--service needs --token-file"))?;
    let token = std::fs::read_to_string(&path)?.trim().to_string();
    let _ = std::fs::remove_file(&path);
    if token.len() < 16 {
        anyhow::bail!("bad token");
    }
    backend::start(token, true)?;
    loop {
        std::thread::park();
    }
}

/// Headless mode: serve the UI and open it in the default browser.
fn run_browser() -> anyhow::Result<()> {
    let token = backend::random_token()?;
    let ctl = backend::start(token.clone(), false)?;
    let url = format!("http://127.0.0.1:{}/?t={token}", web::PORT);
    println!("\n  Geo Unlocker is running. Open:\n\n    {url}\n\n  Keep this window open. Closing it disconnects.\n");
    ctl.plat.open_url(&url);
    loop {
        std::thread::park();
    }
}
