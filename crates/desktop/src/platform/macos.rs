use super::{bundled, unix, DefaultRoute, Platform};
use anyhow::Result;
use geo_common::sh::{find, run, try_run};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct MacOs;

const BREW: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

impl MacOs {
    fn services(&self) -> Vec<String> {
        try_run("networksetup", ["-listallnetworkservices"])
            .stdout
            .lines()
            .skip(1)
            .filter(|s| !s.is_empty() && !s.starts_with('*'))
            .map(String::from)
            .collect()
    }
}

impl Platform for MacOs {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn state_dir(&self) -> PathBuf {
        PathBuf::from("/Library/Application Support/GeoUnlocker")
    }

    fn tor(&self) -> Option<PathBuf> {
        // Shipped inside the app bundle (Contents/Resources/tor = Tor Expert Bundle), else Homebrew.
        bundled("tor/tor/tor").or_else(|| find("tor", &BREW.map(|b| Path::new(b).join("tor"))))
    }

    fn geoip_candidates(&self, tor: &Path) -> Vec<PathBuf> {
        let d = tor.parent().unwrap_or(Path::new("."));
        let up = d.parent().unwrap_or(d);
        vec![
            d.join("geoip"),
            up.join("data").join("geoip"),
            PathBuf::from("/opt/homebrew/share/tor/geoip"),
            PathBuf::from("/usr/local/share/tor/geoip"),
        ]
    }

    fn set_socks_proxy(&self, host: &str, port: u16) -> Result<Value> {
        let services = self.services();
        let port = port.to_string();
        for s in &services {
            run("networksetup", ["-setsocksfirewallproxy", s, host, &port])?;
            run("networksetup", ["-setsocksfirewallproxystate", s, "on"])?;
        }
        Ok(json!({ "services": services }))
    }

    fn restore_proxy(&self, saved: &Value) {
        for s in saved["services"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            try_run("networksetup", ["-setsocksfirewallproxystate", s, "off"]);
        }
    }

    fn open_url(&self, url: &str) {
        unix::spawn_detached(unix::as_user(&["open", url]));
    }

    fn default_route(&self) -> Result<DefaultRoute> {
        let out = run("route", ["-n", "get", "default"])?;
        let field = |k: &str| out.lines().find_map(|l| l.trim().strip_prefix(k).map(|v| v.trim().to_string()));
        let (Some(gateway), Some(iface)) = (field("gateway:"), field("interface:")) else {
            anyhow::bail!("couldn't find this computer's internet connection");
        };
        let local_ip = run("ipconfig", ["getifaddr", &iface])?.trim().to_string();
        Ok(DefaultRoute { gateway, iface, local_ip })
    }

    fn needs_guard_routes(&self) -> bool {
        true
    }

    fn add_bypass(&self, ips: &[String], route: &DefaultRoute) {
        for ip in ips {
            try_run("route", ["-n", "add", "-host", ip, &route.gateway]);
        }
    }

    fn tun_up(&self, tun: &str, route: &DefaultRoute, bypass: &[String], dns: &str) -> Result<Value> {
        // Bypass hosts keep the normal route; everything else goes into the adapter.
        self.add_bypass(bypass, route);
        for net in ["0.0.0.0/1", "128.0.0.0/1"] {
            run("route", ["-n", "add", "-net", net, "-interface", tun])?;
        }
        try_run("route", ["-n", "add", "-inet6", "-net", "2000::/3", "::1", "-reject"]);
        // DNS -> `dns` on every network service (restored afterwards).
        let mut saved_dns = serde_json::Map::new();
        for s in self.services() {
            let cur = try_run("networksetup", ["-getdnsservers", &s]).stdout;
            let servers: Vec<String> = cur
                .lines()
                .map(str::trim)
                .filter(|l| l.parse::<std::net::IpAddr>().is_ok())
                .map(String::from)
                .collect();
            try_run("networksetup", ["-setdnsservers", &s, dns]);
            saved_dns.insert(s, json!(servers));
        }
        try_run("dscacheutil", ["-flushcache"]);
        Ok(json!({ "os": "macos", "tun": tun, "bypass": bypass, "dns": saved_dns }))
    }

    fn tun_down(&self, saved: &Value) {
        if let Some(tun) = saved["tun"].as_str() {
            for net in ["0.0.0.0/1", "128.0.0.0/1"] {
                try_run("route", ["-n", "delete", "-net", net, "-interface", tun]);
            }
        }
        try_run("route", ["-n", "delete", "-inet6", "-net", "2000::/3"]);
        for ip in saved["bypass"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            try_run("route", ["-n", "delete", "-host", ip]);
        }
        if let Some(dns) = saved["dns"].as_object() {
            for (svc, servers) in dns {
                let list: Vec<&str> = servers.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let mut args = vec!["-setdnsservers", svc.as_str()];
                if list.is_empty() {
                    args.push("Empty");
                } else {
                    args.extend(list);
                }
                try_run("networksetup", &args);
            }
        }
        try_run("dscacheutil", ["-flushcache"]);
    }
}
