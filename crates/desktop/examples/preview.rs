//! Open the app window with sample data (no admin rights, no network changes).
//!   cargo run -p geo-unlocker --example preview -- [connected|off|signin|cloud]

use geo_unlocker::gui::{run_preview, Backend};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let connected = !matches!(arg.as_str(), "off" | "signin" | "cloud");
    let cloud = arg == "cloud";
    let signin = arg == "signin";
    let rx = Arc::new(AtomicU64::new(2_200_000));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    run_preview(Backend::Mock(Arc::new(move |method: &str, path: &str, _body: &Value| -> Result<Value, String> {
        let r = rx.fetch_add(60_000 + (now % 7) * 9_000, Ordering::Relaxed);
        Ok(match (method, path) {
            ("GET", "/status") => json!({
                "mode": if connected { json!({"kind": "tor", "country": "nl", "system": true}) } else { json!({"kind": "off"}) },
                "tools": {"wireguard": true, "tor": true},
                "tor": {"running": connected, "country": "nl", "bootstrap": 100},
                "tor_tunnel": connected, "tor_traffic": {"rx": r, "tx": r / 9},
                "vpn": {"kind": null}, "job": null, "cloud": {},
                "pi": {"url": "http://192.168.1.10:8088"},
                "connected_at": if connected { now - 29_679 } else { 0 },
                "last_mode": {"kind": "tor", "country": "nl", "system": true},
                "update": {"current": "0.2.0", "enabled": true, "auto": true, "state": "idle", "latest": "0.2.0", "checked_at": now - 600},
            }),
            ("GET", "/tor/countries") => json!([
                {"code": "de", "relays": 412, "mbps": 9800}, {"code": "us", "relays": 301, "mbps": 7600},
                {"code": "nl", "relays": 190, "mbps": 6100}, {"code": "fr", "relays": 120, "mbps": 3300},
                {"code": "ch", "relays": 80, "mbps": 2500}, {"code": "se", "relays": 60, "mbps": 1900},
                {"code": "fi", "relays": 44, "mbps": 1200}, {"code": "gb", "relays": 40, "mbps": 1100},
                {"code": "ca", "relays": 30, "mbps": 900}, {"code": "jp", "relays": 9, "mbps": 200}
            ]),
            ("GET", "/vpn/profiles") => json!([{"name": "Proton-NL", "type": "wireguard", "auth": false}]),
            ("GET", "/cloud/azure") => json!({
                "account": if cloud { json!("vick@example.com") } else { Value::Null },
                "regions": {"us": "eastus", "gb": "uksouth", "de": "germanywestcentral"}, "cost_note": "",
                "deployment": if cloud { json!({"country": "us", "region": "eastus", "ip": "20.127.93.202", "created": now - 5 * 3600 - 1260, "data_bytes": 112_400_000_000u64}) } else { Value::Null },
                "options": {"subscriptions": [{"id": "a", "name": "Azure subscription 1"}]},
                "usd_per_hour": 0.0154,
                "costs": {"vm_per_hour": 0.0104, "ip_per_hour": 0.005, "data_per_gb": 0.087, "data_free_gb": 100},
                "login": if signin { json!({"url": "https://microsoft.com/devicelogin", "device_code": "FJ3K8QLM2", "done": false, "ok": false}) } else { Value::Null },
            }),
            ("GET", "/advanced") => json!({
                "mode": {"kind": "cloud", "provider": "azure"},
                "tunnel": {"kind": "wireguard", "label": "cloud:azure", "handshake_age": 14, "rx": r, "tx": r / 9},
                "tor": {"running": false, "bootstrap": 0, "socks": "127.0.0.1:9155"}, "tor_tunnel": false,
                "network_changes": {"os": "windows", "bypass": ["20.127.93.202"]},
                "wireguard_config": "[Interface]
PrivateKey = <hidden>
Address = 10.66.66.2/32
DNS = 1.1.1.1

[Peer]
PublicKey = 7c3Hk2Qe1m+VtLx0f4m9zYb0sDqgk8x8mPq3W2rUk1E=
Endpoint = 20.127.93.202:51820
AllowedIPs = 0.0.0.0/0
PersistentKeepalive = 25",
                "cloud": {
                    "server": {"region": "eastus", "ip": "20.127.93.202"},
                    "setup_script": "#!/bin/bash
set -e
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y wireguard iptables
...",
                    "delete": "Delete removes the whole resource group (VM, disk, network, public IP) with one API call: DELETE /subscriptions/<subscription>/resourceGroups/<group>",
                    "api_calls": [
                        {"time": "09:41:03", "method": "PUT", "url": "/subscriptions/1234/resourceGroups/geo-unlocker-12a2f3?api-version=2021-04-01", "status": "201", "body": {"location": "eastus"}},
                        {"time": "09:41:06", "method": "PUT", "url": "/subscriptions/1234/resourceGroups/geo-unlocker-12a2f3/providers/Microsoft.Network/networkSecurityGroups/gu-nsg?api-version=2023-09-01", "status": "201", "body": {"location": "eastus", "properties": {"securityRules": [{"name": "wireguard"}]}}}
                    ]
                },
                "tor_output": [],
                "log": ["09:41:03 [Creating your server (US)] Creating resource group geo-unlocker-12a2f3 in eastus", "09:42:16 [Creating your server (US)] Routing all traffic through WireGuard (GeoUnlockerVPN)", "09:42:30 [Creating your server (US)] Done"],
                "data_folder": r"C:\ProgramData\GeoUnlocker"
            }),
            ("GET", "/ip") if connected => json!({"ip": "185.220.101.4", "city": "Amsterdam", "country": "NL", "loc": "52.37,4.89"}),
            ("GET", "/ip") => json!({"ip": "93.41.12.7", "city": "Milan", "country": "IT", "loc": "45.46,9.19"}),
            _ => json!({}),
        })
    })))
}
