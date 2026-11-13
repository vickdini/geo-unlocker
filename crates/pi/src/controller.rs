//! Pi controller: the selected devices are either direct ("off") or on one VPN profile.

use crate::config::{Config, Device};
use crate::{routing, tunnel::Tunnel};
use anyhow::Result;
use geo_common::jobs::{Job, JobRunner, JobView};
use geo_common::profiles::ProfileStore;
use geo_common::state::State;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

pub struct Ctl {
    cfg_path: PathBuf,
    pub state_dir: PathBuf,
    pub updater: crate::updater::Updater,
    pub cfg: RwLock<Config>,
    pub profiles: ProfileStore,
    pub state: State,
    tunnel: Mutex<Tunnel>,
    pub jobs: JobRunner,
}

impl Ctl {
    pub fn new(cfg_path: PathBuf, cfg: Config, state_dir: &Path) -> Result<Arc<Self>> {
        routing::init(&cfg)?;
        Ok(Arc::new(Self {
            cfg_path,
            state_dir: state_dir.to_path_buf(),
            updater: Default::default(),
            profiles: ProfileStore::new(state_dir.join("profiles"))?,
            state: State::open(state_dir.join("state.json")),
            tunnel: Mutex::new(Tunnel::new(state_dir.to_path_buf())),
            jobs: JobRunner::default(),
            cfg: RwLock::new(cfg),
        }))
    }

    fn activate(&self, job: &Job, name: Option<String>) -> Result<()> {
        let cfg = self.cfg.read().unwrap().clone();
        let mut t = self.tunnel.lock().unwrap();
        t.stop();
        routing::clear(&cfg);
        let Some(name) = name else {
            self.state.remove("active");
            return Ok(());
        };
        job.log(format!("Connecting '{name}'"));
        let result = self
            .profiles
            .get(&name)
            .and_then(|(text, user, pw)| t.start(&cfg, &text, &name, &user, &pw));
        if let Err(e) = result {
            t.stop();
            routing::clear(&cfg);
            self.state.remove("active");
            return Err(e);
        }
        self.state.set("active", &name);
        Ok(())
    }

    pub fn connect(self: &Arc<Self>, name: String) -> Result<JobView> {
        let me = self.clone();
        self.jobs.start(&format!("Connecting {name}"), move |job| me.activate(job, Some(name)))
    }

    pub fn off(self: &Arc<Self>) -> Result<JobView> {
        let me = self.clone();
        self.jobs.start("Turning off", move |job| me.activate(job, None))
    }

    pub fn restore(self: &Arc<Self>) {
        if let Some(name) = self.state.get::<String>("active") {
            let _ = self.connect(name);
        }
    }

    /// Replace the device list and re-apply routing if a VPN is connected.
    pub fn set_devices(&self, devices: Vec<Device>) -> Result<Vec<Device>> {
        let mut cfg = self.cfg.write().unwrap();
        let checked = cfg.check_devices(devices)?;
        let old = cfg.clone();
        cfg.devices = checked;
        cfg.save(&self.cfg_path)?;
        let t = self.tunnel.lock().unwrap();
        routing::clear(&old);
        if let (Some(iface), Some(dns)) = (t.iface, t.dns.as_deref()) {
            routing::via_tunnel(&cfg, iface, dns)?;
        }
        Ok(cfg.devices.clone())
    }

    pub fn set_auto_update(&self, on: bool) -> Result<()> {
        let mut cfg = self.cfg.write().unwrap();
        cfg.auto_update = on;
        cfg.save(&self.cfg_path)
    }

    pub fn status(&self) -> Value {
        let cfg = self.cfg.read().unwrap();
        let tunnel = self.tunnel.try_lock().map(|mut t| t.status()).unwrap_or_else(|_| json!({}));
        json!({
            "active": self.state.get::<String>("active"),
            "devices": cfg.devices,
            "pi_ip": cfg.pi_ip,
            "lan_subnet": cfg.lan_subnet,
            "tunnel": tunnel,
            "job": self.jobs.view(),
            "update": self.updater.status(),
            "auto_update": cfg.auto_update,
        })
    }
}
