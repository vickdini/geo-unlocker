//! /etc/geo-unlocker/config.json

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::path::Path;

/// A LAN device whose traffic is sent through the VPN.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub ip: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// Pi interface on the home network, e.g. eth0
    pub lan_iface: String,
    /// Pi address on the home network
    pub pi_ip: String,
    /// e.g. 192.168.1.0/24
    pub lan_subnet: String,
    /// Devices routed through the VPN (all their traffic).
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default = "default_listen")]
    pub listen: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// HTTP basic-auth password for the web UI and the desktop app ("" = none).
    #[serde(default)]
    pub password: String,
    /// DNS used through the tunnel when the VPN provides none.
    #[serde(default = "default_dns")]
    pub fallback_dns: String,
    /// Install new releases from GitHub automatically.
    #[serde(default = "default_true")]
    pub auto_update: bool,
}

fn default_listen() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8088
}
fn default_true() -> bool {
    true
}
fn default_dns() -> String {
    "1.1.1.1".into()
}

/// "192.168.1.7/24" -> (network u32, mask u32, prefix)
fn parse_cidr(s: &str) -> Result<(u32, u32, u8)> {
    let (ip, prefix) = s.split_once('/').context("lan_subnet must look like 192.168.1.0/24")?;
    let ip: Ipv4Addr = ip.parse().context("bad lan_subnet address")?;
    let prefix: u8 = prefix.parse().context("bad lan_subnet prefix")?;
    if prefix > 32 {
        bail!("bad lan_subnet prefix");
    }
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    Ok((u32::from(ip) & mask, mask, prefix))
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut c: Config = serde_json::from_str(&text).context("parsing config.json")?;
        c.validate()?;
        Ok(c)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        geo_common::write_private(path, &serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    fn validate(&mut self) -> Result<()> {
        self.pi_ip.parse::<Ipv4Addr>().context("bad pi_ip")?;
        let (net, _, prefix) = parse_cidr(&self.lan_subnet)?;
        self.lan_subnet = format!("{}/{}", Ipv4Addr::from(net), prefix);
        self.fallback_dns.parse::<Ipv4Addr>().context("bad fallback_dns")?;
        let devices = std::mem::take(&mut self.devices);
        self.devices = self.check_devices(devices)?;
        Ok(())
    }

    /// Validate a device list: IPv4, inside the LAN, not the Pi, no duplicates.
    pub fn check_devices(&self, devices: Vec<Device>) -> Result<Vec<Device>> {
        let (net, mask, _) = parse_cidr(&self.lan_subnet)?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for d in devices {
            let ip: Ipv4Addr = d.ip.trim().parse().with_context(|| format!("'{}' is not an IPv4 address", d.ip))?;
            if u32::from(ip) & mask != net {
                bail!("{ip} is not on your home network ({})", self.lan_subnet);
            }
            if ip.to_string() == self.pi_ip {
                bail!("{ip} is the Pi itself");
            }
            if !seen.insert(ip) {
                bail!("{ip} is listed twice");
            }
            let name: String = d.name.trim().chars().take(40).collect();
            out.push(Device { ip: ip.to_string(), name });
        }
        Ok(out)
    }

    pub fn device_ips(&self) -> Vec<String> {
        self.devices.iter().map(|d| d.ip.clone()).collect()
    }
}
