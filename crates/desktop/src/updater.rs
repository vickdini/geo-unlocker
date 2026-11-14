//! Automatic updates from GitHub Releases.
//!
//! Checks shortly after start and every 6 hours. A newer release is downloaded,
//! its checksum list's signature is verified with the public key built into the
//! app, and the installer's checksum is checked. It installs silently as soon as
//! the computer isn't connected (so an update never drops a VPN/Tor connection),
//! then the app restarts.

use crate::controller::{Ctl, Mode};
use anyhow::{bail, Context, Result};
use geo_common::update::{check_file, github_latest_url, is_newer, verify_sums, Release, SIG, SUMS};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

/// GitHub "owner/repo", set by the release build (CI); local builds don't auto-update.
pub const REPO: Option<&str> = option_env!("GEO_UPDATE_REPO");
/// Version of this build (CI sets it from the release tag).
pub const VERSION: &str = match option_env!("GEO_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const PUBLIC_KEY: &str = include_str!("../../../packaging/update-public-key.txt");
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Serialize, Clone, Default)]
pub struct UpdateStatus {
    pub current: &'static str,
    pub enabled: bool,
    pub auto: bool,
    /// idle | checking | downloading | ready | installing | restart | error
    pub state: &'static str,
    pub latest: Option<String>,
    pub notes_url: Option<String>,
    pub error: Option<String>,
    pub checked_at: u64,
}

pub struct Updater {
    status: Mutex<UpdateStatus>,
    /// Verified installer waiting to be installed.
    ready: Mutex<Option<PathBuf>>,
    busy: Mutex<()>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            status: Mutex::new(UpdateStatus { current: VERSION, enabled: REPO.is_some(), state: "idle", ..Default::default() }),
            ready: Mutex::new(None),
            busy: Mutex::new(()),
        }
    }
}

/// The installer file for this computer in a release.
fn asset_for_this_os(r: &Release) -> Option<(String, String)> {
    #[cfg(windows)]
    return r.asset("GeoUnlockerSetup-", "-x64.exe");
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return r.asset("GeoUnlocker-", "-macos-arm64.dmg");
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return r.asset("GeoUnlocker-", "-macos-x64.dmg");
    #[cfg(target_os = "linux")]
    return r.asset("geo-unlocker_", "_amd64.deb");
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(120))
        .user_agent(concat!("geo-unlocker/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let resp = agent().get(url).call().map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    let tmp = dest.with_extension("part");
    let mut f = std::fs::File::create(&tmp)?;
    std::io::copy(&mut resp.into_reader(), &mut f)?;
    drop(f);
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

fn text(url: &str) -> Result<String> {
    agent().get(url).call().map_err(|e| anyhow::anyhow!("download failed: {e}"))?.into_string().map_err(Into::into)
}

impl Updater {
    pub fn status(&self, ctl: &Ctl) -> UpdateStatus {
        let mut s = self.status.lock().unwrap().clone();
        s.auto = auto_enabled(ctl);
        s
    }

    fn set(&self, f: impl FnOnce(&mut UpdateStatus)) {
        f(&mut self.status.lock().unwrap());
    }

    /// Background loop: check now (after a short delay) and every few hours.
    pub fn spawn(ctl: &Arc<Ctl>) {
        if REPO.is_none() {
            return;
        }
        let weak: Weak<Ctl> = Arc::downgrade(ctl);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(20));
            loop {
                let Some(ctl) = weak.upgrade() else { return };
                if auto_enabled(&ctl) {
                    let _ = ctl.updater.check_and_download(&ctl);
                    ctl.updater.install_if_idle(&ctl);
                }
                drop(ctl);
                std::thread::sleep(CHECK_EVERY);
            }
        });
    }

    /// Look for a newer release; download and verify it. Returns true if one is ready.
    pub fn check_and_download(&self, ctl: &Ctl) -> Result<bool> {
        let Some(repo) = REPO else { bail!("This build doesn't have automatic updates (it wasn't built by the release pipeline).") };
        let _g = self.busy.lock().unwrap();
        if self.ready.lock().unwrap().is_some() {
            return Ok(true);
        }
        self.set(|s| {
            s.state = "checking";
            s.error = None;
        });
        let result = (|| -> Result<bool> {
            let v: Value = agent()
                .get(&github_latest_url(repo))
                .set("Accept", "application/vnd.github+json")
                .call()
                .map_err(|e| anyhow::anyhow!("can't reach GitHub: {e}"))?
                .into_json()?;
            let release = Release::from_github(&v)?;
            self.set(|s| {
                s.latest = Some(release.version.clone());
                s.notes_url = Some(release.notes_url.clone());
                s.checked_at = geo_common::now();
            });
            if !is_newer(&release.version, VERSION) {
                return Ok(false);
            }
            let (name, url) = asset_for_this_os(&release).context("the new release has no installer for this computer")?;
            let sums_url = release.assets.get(SUMS).context("the release isn't signed (no SHA256SUMS.txt)")?;
            let sig_url = release.assets.get(SIG).context("the release isn't signed (no signature)")?;
            self.set(|s| s.state = "downloading");
            geo_common::log(format!("update {}: downloading {name}", release.version));
            let sums = verify_sums(&text(sums_url)?, &text(sig_url)?, PUBLIC_KEY)?;
            let dir = ctl.dir.join("updates");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)?;
            let file = dir.join(&name);
            download(&url, &file)?;
            check_file(&file, &name, &sums)?;
            geo_common::log(format!("update {}: verified", release.version));
            *self.ready.lock().unwrap() = Some(file);
            Ok(true)
        })();
        match &result {
            Ok(true) => self.set(|s| s.state = "ready"),
            Ok(false) => self.set(|s| s.state = "idle"),
            Err(e) => {
                geo_common::log(format!("update check failed: {e:#}"));
                self.set(|s| {
                    s.state = "error";
                    s.error = Some(format!("{e:#}"));
                });
            }
        }
        result
    }

    /// Install a downloaded update now if nothing is connected (called after checks
    /// and whenever the user disconnects).
    pub fn install_if_idle(&self, ctl: &Ctl) {
        if self.ready.lock().unwrap().is_some() && matches!(ctl.mode(), Mode::Off) && !ctl.jobs.busy() && auto_enabled(ctl) {
            if let Err(e) = self.install(ctl) {
                self.set(|s| {
                    s.state = "error";
                    s.error = Some(format!("{e:#}"));
                });
            }
        }
    }

    /// Install the verified update (disconnects first).
    pub fn install(&self, ctl: &Ctl) -> Result<()> {
        let file = self.ready.lock().unwrap().clone().context("no update has been downloaded yet")?;
        self.set(|s| s.state = "installing");
        geo_common::log(format!("installing update from {}", file.display()));
        // Give the window time to show "Updating…" before the app closes.
        std::thread::sleep(std::time::Duration::from_secs(3));
        ctl.shutdown();
        install_file(&file)?;
        // Windows: the installer replaces the app and starts it again; we exit now.
        // macOS/Linux: files are replaced; the window restarts the app.
        self.set(|s| s.state = "restart");
        #[cfg(windows)]
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            std::process::exit(0);
        });
        Ok(())
    }
}

pub fn auto_enabled(ctl: &Ctl) -> bool {
    ctl.state.get::<bool>("auto_update").unwrap_or(true)
}

#[cfg(windows)]
fn install_file(file: &Path) -> Result<()> {
    // Silent Inno Setup install; /relaunch=1 starts the new version when done.
    std::process::Command::new(file)
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/relaunch=1"])
        .spawn()
        .context("can't start the installer")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_file(file: &Path) -> Result<()> {
    use geo_common::sh::{run, try_run};
    // Replace the running app bundle (…/Geo Unlocker.app/Contents/MacOS/geo-unlocker).
    let exe = std::env::current_exe()?;
    let bundle = exe.ancestors().nth(3).context("not running from an app bundle")?.to_path_buf();
    if bundle.extension().and_then(|e| e.to_str()) != Some("app") {
        bail!("not running from an app bundle; install the update manually");
    }
    let mount = file.with_extension("mnt");
    std::fs::create_dir_all(&mount)?;
    run("hdiutil", ["attach".as_ref(), "-nobrowse".as_ref(), "-readonly".as_ref(), "-mountpoint".as_ref(), mount.as_os_str(), file.as_os_str()])?;
    let result = (|| -> Result<()> {
        let src = std::fs::read_dir(&mount)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
            .context("the update has no app inside")?;
        let staged = bundle.with_extension("app.new");
        let _ = std::fs::remove_dir_all(&staged);
        run("ditto", [src.as_os_str(), staged.as_os_str()])?;
        let old = bundle.with_extension("app.old");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&bundle, &old)?;
        std::fs::rename(&staged, &bundle)?;
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    })();
    try_run("hdiutil", ["detach".as_ref(), mount.as_os_str(), "-force".as_ref()]);
    result
}

#[cfg(target_os = "linux")]
fn install_file(file: &Path) -> Result<()> {
    geo_common::sh::run_with(
        "apt-get",
        ["install".as_ref(), "-y".as_ref(), "--allow-downgrades".as_ref(), file.as_os_str()],
        None,
        &[("DEBIAN_FRONTEND", "noninteractive")],
    )?;
    Ok(())
}
