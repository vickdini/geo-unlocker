//! WireGuard / OpenVPN tunnels that carry only the selected devices' traffic.
//! The Pi's own routes are never changed (WireGuard `Table = off`, OpenVPN
//! `--route-nopull` + stripped route directives).

use crate::{config::Config, routing};
use anyhow::{bail, Result};
use geo_common::profiles::{is_wireguard, strip_openvpn_directives, strip_wireguard_keys, wireguard_dns};
use geo_common::sh::{command, run, try_run};
use geo_common::write_private;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

pub const WG_IF: &str = "guwg0";
pub const OVPN_IF: &str = "gutun0";

const WG_DROP: &[&str] = &["dns", "table", "preup", "postup", "predown", "postdown", "saveconfig"];
const OVPN_DROP: &[&str] = &[
    "redirect-gateway", "route", "route-ipv6", "route-gateway", "dev", "dev-type", "dev-node", "up",
    "down", "script-security", "auth-user-pass", "log", "log-append", "daemon", "block-outside-dns",
    "register-dns", "writepid", "management",
];

#[derive(Default)]
pub struct Tunnel {
    dir: PathBuf,
    pub kind: Option<&'static str>,
    pub label: Option<String>,
    pub dns: Option<String>,
    pub iface: Option<&'static str>,
    proc_: Option<Child>,
}

impl Tunnel {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, ..Default::default() }
    }

    pub fn start(&mut self, cfg: &Config, text: &str, label: &str, user: &str, pw: &str) -> Result<()> {
        self.stop();
        if is_wireguard(text) {
            self.start_wireguard(cfg, text)?;
        } else {
            self.start_openvpn(cfg, text, user, pw)?;
        }
        self.label = Some(label.to_string());
        routing::via_tunnel(cfg, self.iface.unwrap(), self.dns.as_deref().unwrap())
    }

    fn wg_conf_path(&self) -> PathBuf {
        self.dir.join(format!("{WG_IF}.conf"))
    }

    fn start_wireguard(&mut self, cfg: &Config, text: &str) -> Result<()> {
        let path = self.wg_conf_path();
        write_private(&path, &strip_wireguard_keys(text, WG_DROP, &["Table = off"]))?;
        try_run("wg-quick", ["down".as_ref(), path.as_os_str()]);
        run("wg-quick", ["up".as_ref(), path.as_os_str()])?;
        self.kind = Some("wireguard");
        self.iface = Some(WG_IF);
        self.dns = Some(wireguard_dns(text).unwrap_or_else(|| cfg.fallback_dns.clone()));
        Ok(())
    }

    fn start_openvpn(&mut self, cfg: &Config, text: &str, user: &str, pw: &str) -> Result<()> {
        let conf = self.dir.join("gu-openvpn.conf");
        write_private(&conf, &strip_openvpn_directives(text, OVPN_DROP))?;
        let logf = self.dir.join("openvpn.log");
        std::fs::write(&logf, "")?;
        let mut c = command("openvpn");
        c.arg("--config").arg(&conf)
            .args(["--dev", OVPN_IF, "--dev-type", "tun", "--route-nopull", "--auth-nocache", "--verb", "3"])
            .arg("--log").arg(&logf);
        if !user.is_empty() {
            let auth = self.dir.join("gu-openvpn.auth");
            write_private(&auth, &format!("{user}\n{pw}\n"))?;
            c.arg("--auth-user-pass").arg(auth);
        }
        let child = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
            .map_err(|e| anyhow::anyhow!("can't start openvpn: {e}"))?;
        self.proc_ = Some(child);
        let deadline = Instant::now() + Duration::from_secs(60);
        let log = loop {
            let log = std::fs::read_to_string(&logf).unwrap_or_default();
            if log.contains("Initialization Sequence Completed") {
                break log;
            }
            let exited = self.proc_.as_mut().map(|p| matches!(p.try_wait(), Ok(Some(_)))).unwrap_or(true);
            if exited || Instant::now() > deadline {
                self.stop();
                let lines: Vec<&str> = log.trim().lines().collect();
                bail!("OpenVPN did not connect:\n{}", lines[lines.len().saturating_sub(8)..].join("\n"));
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        self.kind = Some("openvpn");
        self.iface = Some(OVPN_IF);
        self.dns = Some(pushed_dns(&log).unwrap_or_else(|| cfg.fallback_dns.clone()));
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.kind == Some("wireguard") {
            try_run("wg-quick", ["down".as_ref(), self.wg_conf_path().as_os_str()]);
        }
        if let Some(mut p) = self.proc_.take() {
            unsafe {
                libc::kill(p.id() as i32, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && matches!(p.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = p.kill();
            let _ = p.wait();
        }
        self.kind = None;
        self.label = None;
        self.dns = None;
        self.iface = None;
    }

    pub fn status(&mut self) -> Value {
        let mut st = json!({ "kind": self.kind, "label": self.label, "dns": self.dns });
        match self.kind {
            Some("wireguard") => {
                let dump = try_run("wg", ["show", WG_IF, "dump"]).stdout;
                if let Some(peer) = dump.lines().nth(1) {
                    let f: Vec<&str> = peer.split('\t').collect();
                    if f.len() > 6 {
                        let hs: u64 = f[4].parse().unwrap_or(0);
                        st["handshake_age"] = if hs > 0 { json!(geo_common::now().saturating_sub(hs)) } else { Value::Null };
                        st["rx"] = json!(f[5].parse::<u64>().unwrap_or(0));
                        st["tx"] = json!(f[6].parse::<u64>().unwrap_or(0));
                    }
                }
            }
            Some("openvpn") => {
                let alive = self.proc_.as_mut().is_some_and(|p| matches!(p.try_wait(), Ok(None)));
                st["alive"] = json!(alive);
            }
            _ => {}
        }
        st
    }
}

/// DNS server pushed by an OpenVPN server ("dhcp-option DNS a.b.c.d" in the log).
fn pushed_dns(log: &str) -> Option<String> {
    let i = log.find("dhcp-option DNS ")? + "dhcp-option DNS ".len();
    let ip: String = log[i..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    ip.parse::<std::net::Ipv4Addr>().ok().map(|_| ip)
}
