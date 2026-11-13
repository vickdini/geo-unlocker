//! Per-OS details: bundled Tor, routing traffic into our tunnels, system proxy, admin rights.

use anyhow::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// The computer's normal route to the internet (captured before a tunnel is up).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DefaultRoute {
    pub gateway: String,
    /// Interface name (Linux/macOS) or interface index (Windows).
    pub iface: String,
    /// This computer's address on that interface; Tor binds its own connections to it
    /// so they keep using the normal route instead of looping into the Tor adapter.
    pub local_ip: String,
}

/// DNS server used through Tor (answered inside the tunnel) and for WireGuard profiles
/// without a DNS line. If settings are left behind after a crash it still works directly.
pub const TUNNEL_DNS: &str = "1.1.1.1";

/// A file shipped with the app: next to the executable, or in the macOS app bundle's
/// Resources folder.
pub fn bundled(rel: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join(rel), dir.join("..").join("Resources").join(rel)].into_iter().find(|p| p.is_file())
}

pub trait Platform: Send + Sync {
    fn name(&self) -> &'static str;
    fn state_dir(&self) -> PathBuf;

    /// The Tor program (bundled with the app, or installed on the system).
    fn tor(&self) -> Option<PathBuf>;

    /// Point the system SOCKS proxy at host:port; returns what's needed to restore it.
    fn set_socks_proxy(&self, host: &str, port: u16) -> Result<Value>;
    fn restore_proxy(&self, saved: &Value);

    fn open_url(&self, url: &str);

    // ----- whole-computer tunnels (Tor and WireGuard) -----
    fn default_route(&self) -> Result<DefaultRoute>;
    /// Send all IPv4 traffic into the adapter `tun`, DNS to `dns`, and block global IPv6.
    /// `bypass` are IPs that must keep using the normal route (the WireGuard server,
    /// Tor's guard relays). Returns what `tun_down` needs to undo it (also used for
    /// crash recovery).
    fn tun_up(&self, tun: &str, route: &DefaultRoute, bypass: &[String], dns: &str) -> Result<Value>;
    fn tun_down(&self, saved: &Value);
    /// Whether Tor's guard relays need explicit bypass routes (when binding Tor to the
    /// physical address isn't enough to keep its traffic off the adapter).
    fn needs_guard_routes(&self) -> bool {
        false
    }
    /// Add more bypass routes later (new Tor guards).
    fn add_bypass(&self, ips: &[String], route: &DefaultRoute);

    /// Where Tor's geoip file may be (needed for choosing exit countries).
    fn geoip_candidates(&self, tor: &Path) -> Vec<PathBuf> {
        let d = tor.parent().unwrap_or(Path::new("."));
        let up = d.parent().unwrap_or(d);
        vec![d.join("geoip"), up.join("data").join("geoip"), up.join("Data").join("Tor").join("geoip")]
    }
}

pub fn current() -> Box<dyn Platform> {
    #[cfg(windows)]
    return Box::new(windows::Windows);
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacOs);
    #[cfg(target_os = "linux")]
    return Box::new(linux::Linux);
}

/// Re-launch with admin rights if needed (Windows gets them from the exe manifest).
pub fn ensure_admin() {
    #[cfg(unix)]
    unix::ensure_root();
}
