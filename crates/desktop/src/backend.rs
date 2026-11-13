//! Starting the privileged part: controller + local API server.

use crate::{controller::Ctl, platform, web};
use anyhow::Result;
use base64::Engine;
use std::sync::Arc;

pub fn random_token() -> Result<String> {
    let mut raw = [0u8; 24];
    getrandom::getrandom(&mut raw).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw))
}

/// Start the controller and API server. With `idle_exit`, disconnect and exit when
/// the app window stops talking to us (used by the separate macOS/Linux service).
pub fn start(token: String, idle_exit: bool) -> Result<Arc<Ctl>> {
    let plat = platform::current();
    let dir = plat.state_dir();
    std::fs::create_dir_all(&dir)?;
    geo_common::set_log_file(&dir.join("geo-unlocker.log"));
    let ctl = Ctl::new(plat)?;
    web::start(ctl.clone(), token)?;

    // Disconnect cleanly on Ctrl+C, logout or kill.
    let c = ctl.clone();
    let _ = ctrlc::set_handler(move || {
        c.shutdown();
        std::process::exit(0);
    });

    if idle_exit {
        let c = ctl.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            if geo_common::now().saturating_sub(web::last_request()) > 45 {
                geo_common::log("app window is gone: disconnecting and exiting");
                c.shutdown();
                std::process::exit(0);
            }
        });
    }
    crate::updater::Updater::spawn(&ctl);
    geo_common::log(format!("backend started (version {})", crate::updater::VERSION));
    Ok(ctl)
}
