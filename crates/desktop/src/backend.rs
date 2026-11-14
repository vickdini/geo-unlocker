//! Starting the networking core: in the window's process (Windows) or as the
//! privileged service (macOS/Linux).

use crate::{controller::Ctl, platform, service};
use anyhow::Result;
use base64::Engine;
use std::sync::Arc;

pub fn random_token() -> Result<String> {
    let mut raw = [0u8; 24];
    getrandom::getrandom(&mut raw).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

fn init() -> Result<Arc<Ctl>> {
    let plat = platform::current();
    let dir = plat.state_dir();
    std::fs::create_dir_all(&dir)?;
    geo_common::set_log_file(&dir.join("geo-unlocker.log"));
    let ctl = Ctl::new(plat)?;
    // Disconnect cleanly on Ctrl+C, logout or kill.
    let c = ctl.clone();
    let _ = ctrlc::set_handler(move || {
        c.shutdown();
        std::process::exit(0);
    });
    crate::updater::Updater::spawn(&ctl);
    ctl.spawn_watchdog();
    geo_common::log(format!("started (version {})", crate::updater::VERSION));
    Ok(ctl)
}

/// Same process as the window (Windows, or when started as root).
pub fn start_local() -> Result<Arc<Ctl>> {
    // A second copy would fight over routes: only one instance at a time.
    static LOCK: std::sync::OnceLock<std::net::TcpListener> = std::sync::OnceLock::new();
    let l = std::net::TcpListener::bind(("127.0.0.1", service::PORT))
        .map_err(|_| anyhow::anyhow!("Geo Unlocker is already running."))?;
    let _ = LOCK.set(l);
    init()
}

/// macOS/Linux privileged service: serve the window over 127.0.0.1 and exit when
/// the window stops talking to us.
pub fn start_service(token: String) -> Result<Arc<Ctl>> {
    let ctl = init()?;
    service::start(ctl.clone(), token)?;
    let c = ctl.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        if geo_common::now().saturating_sub(service::last_request()) > 45 {
            geo_common::log("app window is gone: disconnecting and exiting");
            c.shutdown();
            std::process::exit(0);
        }
    });
    Ok(ctl)
}
