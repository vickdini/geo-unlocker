//! VPN profiles (WireGuard `.conf` / OpenVPN `.ovpn`) shared by both apps.
//! Both apps accept exactly the same files, so profiles move freely between them.

use anyhow::{bail, Result};
use serde::Serialize;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name.chars().all(|c| c.is_ascii_alphanumeric() || " _.-".contains(c))
}

pub fn is_wireguard(text: &str) -> bool {
    text.lines().any(|l| l.trim().eq_ignore_ascii_case("[interface]"))
}

pub fn is_openvpn(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("remote ") || l.starts_with("remote\t")
    })
}

fn key_of(line: &str) -> Option<String> {
    line.split_once('=').map(|(k, _)| k.trim().to_ascii_lowercase())
}

/// First IPv4 DNS server in the WireGuard `[Interface]` section.
pub fn wireguard_dns(text: &str) -> Option<String> {
    let mut in_iface = false;
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') {
            in_iface = s.eq_ignore_ascii_case("[interface]");
            continue;
        }
        if in_iface && key_of(s).as_deref() == Some("dns") {
            let val = s.split_once('=').map(|x| x.1).unwrap_or("");
            for item in val.split(',') {
                if let Ok(ip) = item.trim().parse::<Ipv4Addr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
}

/// Remove `[Interface]` keys (case-insensitive) and optionally add lines to it.
pub fn strip_wireguard_keys(text: &str, keys: &[&str], add_to_interface: &[&str]) -> String {
    let mut out = Vec::new();
    let mut in_iface = false;
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') {
            in_iface = s.eq_ignore_ascii_case("[interface]");
            out.push(s.to_string());
            if in_iface {
                out.extend(add_to_interface.iter().map(|l| l.to_string()));
            }
            continue;
        }
        if in_iface {
            if let Some(k) = key_of(s) {
                if keys.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    out.join("\n") + "\n"
}

/// Remove OpenVPN directives (matched on the first word of each line).
pub fn strip_openvpn_directives(text: &str, directives: &[&str]) -> String {
    text.lines()
        .filter(|l| {
            let first = l.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            !directives.contains(&first.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Make a single-peer WireGuard config carry all traffic (IPv4 and IPv6).
/// Configs with several peers are left alone (their AllowedIPs split traffic on purpose).
pub fn force_full_tunnel_wireguard(text: &str) -> String {
    let peers = text.lines().filter(|l| l.trim().eq_ignore_ascii_case("[peer]")).count();
    if peers != 1 {
        return text.to_string();
    }
    let mut out = Vec::new();
    let mut in_peer = false;
    for line in text.lines() {
        let s = line.trim();
        if s.starts_with('[') {
            in_peer = s.eq_ignore_ascii_case("[peer]");
            out.push(line.to_string());
            if in_peer {
                out.push("AllowedIPs = 0.0.0.0/0, ::/0".to_string());
            }
            continue;
        }
        if in_peer && key_of(s).as_deref() == Some("allowedips") {
            continue;
        }
        out.push(line.to_string());
    }
    out.join("
") + "
"
}

#[derive(Serialize, Clone)]
pub struct ProfileInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub auth: bool,
}

pub struct ProfileStore {
    dir: PathBuf,
}

impl ProfileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn check(name: &str) -> Result<()> {
        if !valid_name(name) {
            bail!("Profile name: letters, numbers, space, . _ - (max 40)");
        }
        Ok(())
    }

    fn path(&self, name: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{name}.{ext}"))
    }

    pub fn list(&self) -> Vec<ProfileInfo> {
        let mut out: Vec<ProfileInfo> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension()? != "conf" {
                    return None;
                }
                let name = p.file_stem()?.to_str()?.to_string();
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                Some(ProfileInfo {
                    kind: if is_wireguard(&text) { "wireguard" } else { "openvpn" },
                    auth: self.path(&name, "auth").exists(),
                    name,
                })
            })
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    pub fn save(&self, name: &str, text: &str, username: &str, password: &str) -> Result<()> {
        Self::check(name)?;
        if !(is_wireguard(text) || is_openvpn(text)) {
            bail!("That doesn't look like a WireGuard .conf or OpenVPN .ovpn file");
        }
        crate::write_private(&self.path(name, "conf"), text)?;
        let auth = self.path(name, "auth");
        if username.is_empty() {
            let _ = std::fs::remove_file(auth);
        } else {
            crate::write_private(&auth, &format!("{username}\n{password}\n"))?;
        }
        Ok(())
    }

    /// -> (text, username, password)
    pub fn get(&self, name: &str) -> Result<(String, String, String)> {
        Self::check(name)?;
        let conf = self.path(name, "conf");
        if !conf.exists() {
            bail!("No profile '{name}'");
        }
        let text = std::fs::read_to_string(conf)?;
        let (mut user, mut pw) = (String::new(), String::new());
        if let Ok(a) = std::fs::read_to_string(self.path(name, "auth")) {
            let mut it = a.lines();
            user = it.next().unwrap_or("").to_string();
            pw = it.next().unwrap_or("").to_string();
        }
        Ok((text, user, pw))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        Self::check(name)?;
        for ext in ["conf", "auth"] {
            let _ = std::fs::remove_file(self.path(name, ext));
        }
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WG: &str = "[Interface]\nPrivateKey = abc\nAddress = 10.2.0.2/32\nDNS = 10.2.0.1, 1.1.1.1\nPostUp = iptables -A x\n[Peer]\nPublicKey = xyz\nAllowedIPs = 0.0.0.0/0\n";

    #[test]
    fn wireguard_helpers() {
        assert!(is_wireguard(WG));
        assert_eq!(wireguard_dns(WG).as_deref(), Some("10.2.0.1"));
        let s = strip_wireguard_keys(WG, &["dns", "postup"], &["Table = off"]);
        assert!(s.contains("Table = off") && !s.contains("DNS") && !s.contains("PostUp"));
        assert!(s.contains("AllowedIPs = 0.0.0.0/0"));
    }

    #[test]
    fn full_tunnel() {
        let split = "[Interface]
PrivateKey = a
[Peer]
PublicKey = b
AllowedIPs = 10.0.0.0/8
Endpoint = 1.2.3.4:51820
";
        let full = force_full_tunnel_wireguard(split);
        assert!(full.contains("AllowedIPs = 0.0.0.0/0, ::/0") && !full.contains("10.0.0.0/8") && full.contains("Endpoint"));
        let two = format!("{split}[Peer]
PublicKey = c
AllowedIPs = 10.1.0.0/16
");
        assert_eq!(force_full_tunnel_wireguard(&two), two);
    }

    #[test]
    fn openvpn_helpers() {
        let ov = "client\ndev tun\nremote x.com 1194\nredirect-gateway def1\nroute-nopull\nroute 10.0.0.0 255.0.0.0\n<ca>\nABC\n</ca>\n";
        assert!(is_openvpn(ov) && !is_wireguard(ov));
        let s = strip_openvpn_directives(ov, &["dev", "redirect-gateway", "route"]);
        assert!(!s.contains("dev tun") && !s.contains("redirect-gateway") && !s.contains("route 10."));
        assert!(s.contains("route-nopull") && s.contains("<ca>"));
    }

    #[test]
    fn names() {
        assert!(valid_name("US-East 1.x_y"));
        assert!(!valid_name("bad/name") && !valid_name("") && !valid_name(&"a".repeat(41)));
    }
}
