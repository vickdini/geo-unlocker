use super::{bundled, unix, DefaultRoute, Platform};
use anyhow::Result;
use geo_common::sh::{find, run, try_run, which};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Linux;

const BYPASS_TABLE: &str = "4613";

impl Linux {
    fn gsettings(&self, args: &[&str]) -> geo_common::sh::Out {
        let mut cmd = vec!["gsettings"];
        cmd.extend_from_slice(args);
        let full = unix::as_user(&cmd);
        try_run(&full[0], &full[1..])
    }
}

/// Create (or empty) our chain and hook it into `parent`.
fn chain(tool: &str, table: &str, parent: &str, name: &str) -> Result<()> {
    try_run(tool, ["-w", "-t", table, "-N", name]);
    run(tool, ["-w", "-t", table, "-F", name])?;
    if !try_run(tool, ["-w", "-t", table, "-C", parent, "-j", name]).ok() {
        run(tool, ["-w", "-t", table, "-I", parent, "1", "-j", name])?;
    }
    Ok(())
}

impl Platform for Linux {
    fn name(&self) -> &'static str {
        "linux"
    }

    fn state_dir(&self) -> PathBuf {
        PathBuf::from("/var/lib/geo-unlocker-desktop")
    }

    fn tor(&self) -> Option<PathBuf> {
        // The .deb depends on the distribution's tor package.
        bundled("tor/tor").or_else(|| find("tor", &[PathBuf::from("/usr/bin/tor"), PathBuf::from("/usr/sbin/tor")]))
    }

    fn geoip_candidates(&self, tor: &Path) -> Vec<PathBuf> {
        let d = tor.parent().unwrap_or(Path::new("."));
        vec![d.join("geoip"), PathBuf::from("/usr/share/tor/geoip")]
    }

    fn set_socks_proxy(&self, host: &str, port: u16) -> Result<Value> {
        if which("gsettings").is_none() {
            return Ok(json!({ "manual": true }));
        }
        let prev = self.gsettings(&["get", "org.gnome.system.proxy", "mode"]).stdout;
        let prev = prev.trim().trim_matches('\'');
        let port = port.to_string();
        self.gsettings(&["set", "org.gnome.system.proxy.socks", "host", host]);
        self.gsettings(&["set", "org.gnome.system.proxy.socks", "port", &port]);
        self.gsettings(&["set", "org.gnome.system.proxy", "mode", "manual"]);
        Ok(json!({ "mode": if prev.is_empty() { "none" } else { prev } }))
    }

    fn restore_proxy(&self, saved: &Value) {
        if let Some(mode) = saved["mode"].as_str() {
            self.gsettings(&["set", "org.gnome.system.proxy", "mode", mode]);
        }
    }

    fn open_url(&self, url: &str) {
        unix::spawn_detached(unix::as_user(&["xdg-open", url]));
    }

    fn default_route(&self) -> Result<DefaultRoute> {
        // "1.1.1.1 via 192.168.1.1 dev wlan0 src 192.168.1.23 uid 0"
        let out = run("ip", ["-4", "route", "get", "1.1.1.1"])?;
        let toks: Vec<&str> = out.split_whitespace().collect();
        let after = |k: &str| toks.iter().position(|t| *t == k).and_then(|i| toks.get(i + 1)).map(|s| s.to_string());
        match (after("via"), after("dev"), after("src")) {
            (Some(gateway), Some(iface), Some(local_ip)) => Ok(DefaultRoute { gateway, iface, local_ip }),
            _ => anyhow::bail!("couldn't find this computer's internet connection ({})", out.trim()),
        }
    }

    fn add_bypass(&self, ips: &[String], route: &DefaultRoute) {
        for ip in ips {
            try_run("ip", ["route", "replace", &format!("{ip}/32"), "via", &route.gateway, "dev", &route.iface]);
        }
    }

    fn tun_up(&self, tun: &str, route: &DefaultRoute, bypass: &[String], dns: &str) -> Result<Value> {
        self.add_bypass(bypass, route);
        // All IPv4 into the adapter (two /1 routes beat the default route; LAN routes stay).
        for net in ["0.0.0.0/1", "128.0.0.0/1"] {
            run("ip", ["route", "replace", net, "dev", tun])?;
        }
        // Tor binds its connections to the physical address: send those via the real gateway.
        run("ip", ["route", "replace", "default", "via", &route.gateway, "dev", &route.iface, "table", BYPASS_TABLE])?;
        run("ip", ["rule", "add", "from", &route.local_ip, "lookup", BYPASS_TABLE, "priority", BYPASS_TABLE])?;
        // DNS: whatever server the system uses, send queries to `dns` through the tunnel.
        let dest = format!("{dns}:53");
        chain("iptables", "nat", "OUTPUT", "GU_DNS")?;
        run("iptables", ["-w", "-t", "nat", "-A", "GU_DNS", "-d", "127.0.0.0/8", "-j", "RETURN"])?;
        for proto in ["udp", "tcp"] {
            run("iptables", ["-w", "-t", "nat", "-A", "GU_DNS", "-p", proto, "--dport", "53", "-j", "DNAT", "--to-destination", &dest])?;
        }
        // Block global IPv6 so nothing bypasses the IPv4 tunnel (ip6tables may be missing).
        if chain("ip6tables", "filter", "OUTPUT", "GU_V6").is_ok() {
            try_run("ip6tables", ["-w", "-A", "GU_V6", "-d", "2000::/3", "-j", "REJECT"]);
        }
        Ok(json!({ "os": "linux", "tun": tun, "bypass": bypass }))
    }

    fn tun_down(&self, saved: &Value) {
        if let Some(tun) = saved["tun"].as_str() {
            for net in ["0.0.0.0/1", "128.0.0.0/1"] {
                try_run("ip", ["route", "del", net, "dev", tun]);
            }
        }
        for ip in saved["bypass"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            try_run("ip", ["route", "del", &format!("{ip}/32")]);
        }
        for _ in 0..16 {
            if !try_run("ip", ["rule", "del", "priority", BYPASS_TABLE]).ok() {
                break;
            }
        }
        try_run("ip", ["route", "flush", "table", BYPASS_TABLE]);
        try_run("iptables", ["-w", "-t", "nat", "-F", "GU_DNS"]);
        try_run("ip6tables", ["-w", "-F", "GU_V6"]);
    }
}
