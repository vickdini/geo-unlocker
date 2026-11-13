//! Open the app window with sample data (no admin rights, no network changes).
//!   cargo run -p geo-unlocker --example preview -- [connected|off|signin]

use geo_unlocker::gui::{run_preview, Backend};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let connected = arg != "off" && arg != "signin";
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
                "account": null, "regions": {"us": "eastus"}, "cost_note": "", "deployment": null,
                "login": if signin { json!({"url": "https://microsoft.com/devicelogin", "device_code": "FJ3K8QLM2", "done": false, "ok": false}) } else { Value::Null },
            }),
            ("GET", "/ip") if connected => json!({"ip": "185.220.101.4", "city": "Amsterdam", "country": "NL", "loc": "52.37,4.89"}),
            ("GET", "/ip") => json!({"ip": "93.41.12.7", "city": "Milan", "country": "IT", "loc": "45.46,9.19"}),
            _ => json!({}),
        })
    })))
}
