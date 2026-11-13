//! VPN for this computer: built-in WireGuard carrying all traffic.

use crate::platform::Platform;
use crate::wgtun::{WgConfig, WgTunnel};
use anyhow::{bail, Result};
use geo_common::jobs::Job;
use geo_common::profiles::is_wireguard;
use geo_common::state::State;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct DesktopVpn {
    dir: PathBuf,
    tunnel: WgTunnel,
    label: Mutex<Option<String>>,
}

impl DesktopVpn {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf(), tunnel: WgTunnel::default(), label: Mutex::new(None) }
    }

    /// Connect: all IPv4 traffic and DNS go through the WireGuard server.
    /// Route changes are saved in `state` ("tun_saved") so they can be undone after a crash.
    pub fn start(&self, plat: &dyn Platform, state: &State, text: &str, label: &str, job: &Job) -> Result<()> {
        if !is_wireguard(text) {
            bail!("This is an OpenVPN profile. Geo Unlocker connects computers with WireGuard; \
                   send OpenVPN profiles to the Raspberry Pi instead (→ Pi), or download a \
                   WireGuard profile from your VPN provider.");
        }
        let cfg = WgConfig::parse(text)?;
        let route = plat.default_route()?;
        job.log(format!("Server {} · this computer {}", cfg.endpoint, cfg.address));
        let tun = self.tunnel.start(&cfg, &self.dir)?;
        job.log(format!("Routing all traffic through WireGuard ({tun})"));
        let dns = cfg.dns.map(|d| d.to_string()).unwrap_or_else(|| crate::platform::TUNNEL_DNS.to_string());
        match plat.tun_up(&tun, &route, &[cfg.endpoint_ip()], &dns) {
            Ok(saved) => state.set("tun_saved", &saved),
            Err(e) => {
                self.tunnel.stop();
                return Err(e);
            }
        }
        *self.label.lock().unwrap() = Some(label.to_string());
        Ok(())
    }

    /// Stop the tunnel (routes are undone by the caller via "tun_saved").
    pub fn stop(&self) {
        self.tunnel.stop();
        *self.label.lock().unwrap() = None;
    }

    pub fn status(&self) -> Value {
        if !self.tunnel.running() {
            return json!({ "kind": null });
        }
        let (hs, rx, tx) = self.tunnel.stats();
        json!({
            "kind": "wireguard",
            "label": *self.label.lock().unwrap(),
            "handshake_age": hs,
            "rx": rx,
            "tx": tx,
        })
    }
}
