//! Automatic updates for the Pi app from GitHub Releases (signed checksums, like
//! the desktop app). Checks a minute after start and every 12 hours; installs the
//! new binary and restarts the service, which reconnects the active profile.

use crate::controller::Ctl;
use anyhow::{bail, Context, Result};
use geo_common::sh::run;
use geo_common::update::{check_file, github_latest_url, is_newer, verify_sums, Release, SIG, SUMS};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

pub const REPO: Option<&str> = option_env!("GEO_UPDATE_REPO");
pub const VERSION: &str = match option_env!("GEO_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const PUBLIC_KEY: &str = include_str!("../../../packaging/update-public-key.txt");

#[derive(Serialize, Clone, Default)]
pub struct UpdateStatus {
    pub current: &'static str,
    pub enabled: bool,
    /// idle | checking | installing | error
    pub state: &'static str,
    pub latest: Option<String>,
    pub error: Option<String>,
    pub checked_at: u64,
}

pub struct Updater {
    status: Mutex<UpdateStatus>,
    busy: Mutex<()>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Mutex::new(UpdateStatus { current: VERSION, enabled: REPO.is_some(), state: "idle", ..Default::default() }),
            busy: Mutex::new(()),
        }
    }
}

fn arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("arm64"),
        "arm" => Ok("armhf"),
        a => bail!("no Pi release for {a}"),
    }
}

fn curl(url: &str) -> Result<String> {
    run("curl", ["-fsSL", "--max-time", "60", "-A", "geo-unlocker-pi", "-H", "Accept: application/vnd.github+json", url])
}

impl Updater {
    pub fn status(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, f: impl FnOnce(&mut UpdateStatus)) {
        f(&mut self.status.lock().unwrap());
    }

    pub fn spawn(ctl: &Arc<Ctl>) {
        if REPO.is_none() {
            return;
        }
        let weak: Weak<Ctl> = Arc::downgrade(ctl);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(60));
            loop {
                let Some(ctl) = weak.upgrade() else { return };
                if ctl.cfg.read().unwrap().auto_update {
                    let _ = ctl.updater.check_and_install(&ctl);
                }
                drop(ctl);
                std::thread::sleep(Duration::from_secs(12 * 3600));
            }
        });
    }

    /// Check GitHub; if a newer signed release exists, install it and restart.
    pub fn check_and_install(&self, ctl: &Ctl) -> Result<bool> {
        let Some(repo) = REPO else { bail!("This build doesn't have automatic updates.") };
        let _g = self.busy.lock().unwrap();
        self.set(|s| {
            s.state = "checking";
            s.error = None;
        });
        let result = (|| -> Result<bool> {
            let v: Value = serde_json::from_str(&curl(&github_latest_url(repo))?)?;
            let release = Release::from_github(&v)?;
            self.set(|s| {
                s.latest = Some(release.version.clone());
                s.checked_at = geo_common::now();
            });
            if !is_newer(&release.version, VERSION) {
                return Ok(false);
            }
            let name = format!("geo-unlocker-pi-{}.tar.gz", arch()?);
            let url = release.assets.get(&name).with_context(|| format!("the release has no {name}"))?;
            let sums_url = release.assets.get(SUMS).context("the release isn't signed")?;
            let sig_url = release.assets.get(SIG).context("the release isn't signed")?;
            self.set(|s| s.state = "installing");
            let sums = verify_sums(&curl(sums_url)?, &curl(sig_url)?, PUBLIC_KEY)?;
            let dir = ctl.state_dir.join("updates");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let file = dir.join(&name);
            run("curl", ["-fsSL".as_ref(), "--max-time".as_ref(), "900".as_ref(), "-o".as_ref(), file.as_os_str(), url.as_ref()])?;
            check_file(&file, &name, &sums)?;
            run("tar", ["xzf".as_ref(), file.as_os_str(), "-C".as_ref(), dir.as_os_str()])?;
            let new_bin = dir.join(name.trim_end_matches(".tar.gz")).join("geo-unlocker-pi");
            install_binary(&new_bin)?;
            geo_common::log(format!("updated to {}; restarting", release.version));
            // systemd restarts us with the new binary; the active profile reconnects.
            std::process::Command::new("systemctl").args(["restart", "geo-unlocker"]).spawn()?;
            Ok(true)
        })();
        if let Err(e) = &result {
            geo_common::log(format!("update failed: {e:#}"));
            self.set(|s| {
                s.state = "error";
                s.error = Some(format!("{e:#}"));
            });
        } else if result.as_ref().is_ok_and(|r| !r) {
            self.set(|s| s.state = "idle");
        }
        result
    }
}

/// Atomically replace the running binary.
fn install_binary(new_bin: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let target = std::env::current_exe()?;
    let target = Path::new(target.to_str().unwrap_or("").trim_end_matches(" (deleted)")).to_path_buf();
    let staged = target.with_extension("new");
    std::fs::copy(new_bin, &staged)?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&staged, &target)?;
    Ok(())
}
