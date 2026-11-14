//! Desktop controller: one mode at a time (off / tor / vpn / cloud) for this computer.

use crate::platform::{DefaultRoute, Platform};
use crate::{cloud::CloudManager, pilink, systun::SysTunnel, tor::{TorClient, SOCKS_PORT}, vpn::DesktopVpn};
use anyhow::Result;
use geo_common::jobs::{Job, JobRunner, JobView};
use geo_common::profiles::ProfileStore;
use geo_common::state::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Off,
    /// `system`: all of this computer's traffic (default); otherwise only apps that
    /// follow the system SOCKS proxy.
    Tor {
        country: String,
        #[serde(default = "yes")]
        system: bool,
    },
    Vpn { name: String },
    Cloud { provider: String },
}

pub struct Ctl {
    pub plat: Box<dyn Platform>,
    pub dir: PathBuf,
    pub state: Arc<State>,
    pub profiles: ProfileStore,
    pub tor: TorClient,
    pub vpn: DesktopVpn,
    pub systun: SysTunnel,
    guard_watch: Mutex<Option<Arc<AtomicBool>>>,
    /// When the current connection was made (0 = not connected).
    connected_at: std::sync::atomic::AtomicU64,
    pub cloud: CloudManager,
    pub updater: crate::updater::Updater,
    pub jobs: JobRunner,
    switch: Mutex<()>,
}

impl Ctl {
    pub fn new(plat: Box<dyn Platform>) -> Result<Arc<Self>> {
        let dir = plat.state_dir();
        std::fs::create_dir_all(&dir)?;
        let ctl = Arc::new(Self {
            state: Arc::new(State::open(dir.join("state.json"))),
            profiles: ProfileStore::new(dir.join("profiles"))?,
            tor: TorClient::new(&dir),
            vpn: DesktopVpn::new(&dir),
            systun: SysTunnel::default(),
            guard_watch: Mutex::new(None),
            connected_at: Default::default(),
            cloud: CloudManager::default(),
            updater: Default::default(),
            jobs: JobRunner::default(),
            switch: Mutex::new(()),
            dir,
            plat,
        });
        // Clean up after a crash: restore the proxy / DNS / routes and drop a leftover tunnel.
        ctl.tor.restore_proxy(ctl.plat.as_ref(), &ctl.state);
        ctl.undo_tun_routes();
        ctl.state.set("mode", &Mode::Off);
        Ok(ctl)
    }

    pub fn mode(&self) -> Mode {
        self.state.get("mode").unwrap_or_default()
    }

    fn undo_tun_routes(&self) {
        if let Some(saved) = self.state.get::<Value>("tun_saved") {
            self.plat.tun_down(&saved);
            self.state.remove("tun_saved");
        }
    }

    fn stop_all(&self) {
        if let Some(flag) = self.guard_watch.lock().unwrap().take() {
            flag.store(true, Ordering::SeqCst);
        }
        self.undo_tun_routes();
        self.systun.stop();
        self.tor.stop(self.plat.as_ref(), &self.state);
        self.vpn.stop();
    }

    /// Tor for the whole computer: tor (bound to the physical address) + TUN adapter + routes.
    fn start_tor_system(self: &Arc<Self>, job: &Job, country: &str) -> Result<()> {
        let plat = self.plat.as_ref();
        let route = plat.default_route()?;
        job.log(format!("Normal connection: gateway {} (this computer {})", route.gateway, route.local_ip));
        let extra = vec![format!("OutboundBindAddress {}", route.local_ip), "ClientUseIPv6 0".to_string()];
        self.tor.start(plat, &self.state, country, job, &extra, false)?;
        let guards = if plat.needs_guard_routes() { self.tor.guard_ips()? } else { Vec::new() };
        job.log("Creating the virtual network adapter");
        let tun = self.systun.start(SOCKS_PORT, &self.dir)?;
        job.log(format!("Routing all traffic through Tor ({tun})"));
        let saved = plat.tun_up(&tun, &route, &guards, crate::platform::TUNNEL_DNS)?;
        self.state.set("tun_saved", &saved);
        if plat.needs_guard_routes() {
            self.watch_guards(route, guards);
        }
        Ok(())
    }

    /// If Tor switches to a new guard relay, give it a bypass route too.
    fn watch_guards(self: &Arc<Self>, route: DefaultRoute, mut known: Vec<String>) {
        let stop = Arc::new(AtomicBool::new(false));
        *self.guard_watch.lock().unwrap() = Some(stop.clone());
        let me = self.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(30));
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(ips) = me.tor.guard_ips() else { continue };
                let new: Vec<String> = ips.into_iter().filter(|ip| !known.contains(ip)).collect();
                if new.is_empty() {
                    continue;
                }
                me.plat.add_bypass(&new, &route);
                known.extend(new);
                if let Some(mut saved) = me.state.get::<Value>("tun_saved") {
                    saved["bypass"] = json!(known);
                    me.state.set("tun_saved", &saved);
                }
            }
        });
    }

    pub fn activate(self: &Arc<Self>, job: &Job, mode: Mode) -> Result<()> {
        let _g = self.switch.lock().unwrap();
        self.stop_all();
        self.connected_at.store(0, Ordering::SeqCst);
        let plat = self.plat.as_ref();
        let r = match &mode {
            Mode::Off => Ok(()),
            Mode::Tor { country, system } => {
                job.log(format!("Starting Tor (exit: {})", if country.is_empty() { "any country".into() } else { country.to_uppercase() }));
                if *system {
                    self.start_tor_system(job, country)
                } else {
                    self.tor.start(plat, &self.state, country, job, &[], true)
                }
            }
            Mode::Vpn { name } => self
                .profiles
                .get(name)
                .and_then(|(text, _, _)| self.vpn.start(plat, &self.state, &text, name, job)),
            Mode::Cloud { provider } => self
                .cloud
                .client_config(&self.state, provider, "desktop")
                .and_then(|conf| self.vpn.start(plat, &self.state, &conf, &format!("cloud:{provider}"), job)),
        };
        if r.is_err() {
            self.stop_all();
            self.state.set("mode", &Mode::Off);
            return r;
        }
        self.state.set("mode", &mode);
        if !matches!(mode, Mode::Off) {
            self.state.set("last_mode", &mode);
            self.connected_at.store(geo_common::now(), Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn set_mode(self: &Arc<Self>, mode: Mode) -> Result<JobView> {
        let title = match &mode {
            Mode::Off => "Disconnecting",
            Mode::Tor { .. } => "Starting Tor",
            Mode::Vpn { .. } => "Connecting VPN",
            Mode::Cloud { .. } => "Connecting to cloud server",
        };
        let me = self.clone();
        let off = matches!(mode, Mode::Off);
        let view = self.jobs.start(title, move |job| me.activate(job, mode))?;
        if off {
            // A downloaded update installs as soon as nothing is connected.
            let me = self.clone();
            std::thread::spawn(move || {
                while me.jobs.busy() {
                    std::thread::sleep(Duration::from_millis(500));
                }
                me.updater.install_if_idle(&me);
            });
        }
        Ok(view)
    }

    /// Watch the active connection; if its tunnel dies unexpectedly (crash, adapter
    /// removed, tor exited), reconnect instead of silently leaving the computer on
    /// its normal connection while the window says "Connecting…".
    pub fn spawn_watchdog(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut next_try = 0u64;
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let Some(ctl) = me.upgrade() else { return };
                // Only for connections that were up (connected_at set) and not mid-switch.
                if ctl.jobs.busy() || ctl.connected_at.load(Ordering::SeqCst) == 0 {
                    continue;
                }
                let mode = ctl.mode();
                let alive = match &mode {
                    Mode::Off => true,
                    Mode::Vpn { .. } | Mode::Cloud { .. } => ctl.vpn.running(),
                    Mode::Tor { system: true, .. } => ctl.tor.running() && ctl.systun.running(),
                    Mode::Tor { system: false, .. } => ctl.tor.running(),
                };
                let now = geo_common::now();
                if alive || now < next_try {
                    continue;
                }
                geo_common::log("connection lost: reconnecting");
                next_try = now + 30; // don't hammer if reconnecting keeps failing
                let _ = ctl.set_mode(mode);
            }
        });
    }

    pub fn shutdown(&self) {
        self.stop_all();
        self.state.set("mode", &Mode::Off);
    }

    // ----- Pi -----
    pub fn send_profile_to_pi(&self, name: &str) -> Result<()> {
        let (text, user, pw) = self.profiles.get(name)?;
        pilink::send_profile(&self.state, name, &text, &user, &pw)
    }

    pub fn send_cloud_to_pi(&self, provider: &str, connect: bool) -> Result<String> {
        let d = self.cloud.deployments(&self.state).remove(provider)
            .ok_or_else(|| anyhow::anyhow!("No server deployed for this provider"))?;
        let name = format!("cloud-{provider}-{}", d.country);
        let conf = self.cloud.client_config(&self.state, provider, "pi")?;
        pilink::send_profile(&self.state, &name, &conf, "", "")?;
        if connect {
            pilink::connect(&self.state, &name)?;
        }
        Ok(name)
    }

    pub fn status(&self) -> Value {
        let plat = self.plat.as_ref();
        // Everything is built in or bundled; Tor is only missing if the bundle was removed.
        let tools = json!({ "wireguard": true, "tor": plat.tor().is_some() });
        let hints = json!({ "tor": "Tor is missing from this installation. Reinstall Geo Unlocker." });
        let cloud: serde_json::Map<String, Value> = self
            .cloud
            .deployments(&self.state)
            .iter()
            .map(|(k, d)| (k.clone(), d.public()))
            .collect();
        json!({
            "mode": self.mode(),
            "platform": plat.name(),
            "tools": tools,
            "hints": hints,
            "tor": self.tor.status(),
            "vpn": self.vpn.status(),
            "tor_tunnel": self.systun.running(),
            "tor_traffic": { "rx": crate::systun::traffic().0, "tx": crate::systun::traffic().1 },
            "connected_at": self.connected_at.load(Ordering::SeqCst),
            "last_mode": self.state.get::<Mode>("last_mode"),
            "job": self.jobs.view(),
            "cloud": cloud,
            "pi": pilink::public(&self.state),
            "update": self.updater.status(self),
        })
    }
}
