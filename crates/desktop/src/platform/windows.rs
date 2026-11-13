use super::{bundled, DefaultRoute, Platform};
use anyhow::Result;
use geo_common::sh::{find, run};
use serde_json::{json, Value};
use std::path::PathBuf;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

pub struct Windows;

const IS_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const NRPT_TAG: &str = "GeoUnlocker";
const FW_RULE: &str = "GeoUnlocker block IPv6";

fn ps(script: &str) -> Result<String> {
    run("powershell", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
}

/// Tell running apps the proxy settings changed.
fn notify_proxy_change() {
    use windows_sys::Win32::Networking::WinInet::InternetSetOptionW;
    const SETTINGS_CHANGED: u32 = 39;
    const REFRESH: u32 = 37;
    unsafe {
        InternetSetOptionW(std::ptr::null(), SETTINGS_CHANGED, std::ptr::null(), 0);
        InternetSetOptionW(std::ptr::null(), REFRESH, std::ptr::null(), 0);
    }
}

fn host_route(ip: &str, route: &DefaultRoute) -> String {
    format!(
        "New-NetRoute -DestinationPrefix '{ip}/32' -InterfaceIndex {} -NextHop '{}' -RouteMetric 1 -PolicyStore ActiveStore -ErrorAction SilentlyContinue | Out-Null;",
        route.iface, route.gateway
    )
}

impl Platform for Windows {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn state_dir(&self) -> PathBuf {
        PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("GeoUnlocker")
    }

    fn tor(&self) -> Option<PathBuf> {
        // Shipped with the app as the unmodified Tor Expert Bundle (tor\tor\tor.exe, tor\data\geoip).
        bundled(r"tor\tor\tor.exe").or_else(|| find("tor", &[]))
    }

    fn set_socks_proxy(&self, host: &str, port: u16) -> Result<Value> {
        let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(IS_KEY, KEY_READ | KEY_WRITE)?;
        let enable: Option<u32> = key.get_value("ProxyEnable").ok();
        let server: Option<String> = key.get_value("ProxyServer").ok();
        key.set_value("ProxyEnable", &1u32)?;
        key.set_value("ProxyServer", &format!("socks={host}:{port}"))?;
        notify_proxy_change();
        Ok(json!({ "ProxyEnable": enable, "ProxyServer": server }))
    }

    fn restore_proxy(&self, saved: &Value) {
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(IS_KEY, KEY_WRITE) {
            let _ = key.set_value("ProxyEnable", &(saved["ProxyEnable"].as_u64().unwrap_or(0) as u32));
            match saved["ProxyServer"].as_str() {
                Some(s) => {
                    let _ = key.set_value("ProxyServer", &s.to_string());
                }
                None => {
                    let _ = key.delete_value("ProxyServer");
                }
            }
            notify_proxy_change();
        }
    }

    fn open_url(&self, url: &str) {
        // Via explorer so the browser doesn't inherit our administrator rights.
        let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    }

    fn default_route(&self) -> Result<DefaultRoute> {
        let out = ps("$r = Get-NetRoute -DestinationPrefix '0.0.0.0/0' -ErrorAction Stop | \
                      Where-Object { $_.InterfaceAlias -notlike 'GeoUnlocker*' } | \
                      Sort-Object { $_.RouteMetric + (Get-NetIPInterface -InterfaceIndex $_.ifIndex -AddressFamily IPv4).InterfaceMetric } | \
                      Select-Object -First 1; \
                      $ip = (Get-NetIPAddress -InterfaceIndex $r.ifIndex -AddressFamily IPv4 | Select-Object -First 1).IPAddress; \
                      \"$($r.NextHop)|$($r.ifIndex)|$ip\"")?;
        let f: Vec<&str> = out.trim().split('|').collect();
        match f.as_slice() {
            [gw, idx, ip] if !gw.is_empty() && !ip.is_empty() => {
                Ok(DefaultRoute { gateway: gw.to_string(), iface: idx.to_string(), local_ip: ip.to_string() })
            }
            _ => anyhow::bail!("couldn't find this computer's internet connection ({})", out.trim()),
        }
    }

    fn add_bypass(&self, ips: &[String], route: &DefaultRoute) {
        if !ips.is_empty() {
            let _ = ps(&ips.iter().map(|ip| host_route(ip, route)).collect::<String>());
        }
    }

    fn tun_up(&self, tun: &str, route: &DefaultRoute, bypass: &[String], dns: &str) -> Result<Value> {
        // Bypass routes first (e.g. the WireGuard server), then everything else into the
        // adapter with two /1 routes (more specific than the default route, which stays).
        // Tor binds its own sockets to the physical address; Windows' strong host model
        // keeps those on the physical interface.
        let script = format!(
            "$i = (Get-NetAdapter -Name '{tun}' -ErrorAction Stop).ifIndex; {bypass} \
             foreach ($p in '0.0.0.0/1','128.0.0.0/1') {{ New-NetRoute -DestinationPrefix $p -InterfaceIndex $i -NextHop '0.0.0.0' -RouteMetric 1 -PolicyStore ActiveStore -ErrorAction Stop | Out-Null }}; \
             Get-DnsClientNrptRule | Where-Object Comment -eq '{NRPT_TAG}' | Remove-DnsClientNrptRule -Force; \
             Add-DnsClientNrptRule -Namespace '.' -NameServers '{dns}' -Comment '{NRPT_TAG}' | Out-Null; \
             Remove-NetFirewallRule -DisplayName '{FW_RULE}' -ErrorAction SilentlyContinue; \
             New-NetFirewallRule -DisplayName '{FW_RULE}' -Direction Outbound -RemoteAddress 2000::/3 -Action Block | Out-Null; \
             Clear-DnsClientCache",
            bypass = bypass.iter().map(|ip| host_route(ip, route)).collect::<String>()
        );
        ps(&script)?;
        Ok(json!({ "os": "windows", "bypass": bypass }))
    }

    fn tun_down(&self, saved: &Value) {
        let bypass: String = saved["bypass"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|ip| format!("Remove-NetRoute -DestinationPrefix '{ip}/32' -Confirm:$false -ErrorAction SilentlyContinue;"))
            .collect();
        let _ = ps(&format!(
            "{bypass} \
             Get-NetRoute -DestinationPrefix '0.0.0.0/1','128.0.0.0/1' -ErrorAction SilentlyContinue | \
               Where-Object {{ $_.InterfaceAlias -like 'GeoUnlocker*' }} | Remove-NetRoute -Confirm:$false -ErrorAction SilentlyContinue; \
             Get-DnsClientNrptRule | Where-Object Comment -eq '{NRPT_TAG}' | Remove-DnsClientNrptRule -Force; \
             Remove-NetFirewallRule -DisplayName '{FW_RULE}' -ErrorAction SilentlyContinue; \
             Clear-DnsClientCache"
        ));
    }
}
