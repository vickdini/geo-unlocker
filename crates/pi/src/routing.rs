//! Policy routing + iptables rules that affect ONLY the selected devices.
//!
//! Everything lives in our own `GU_*` chains, routing table 4611 and rule
//! priorities 4610/4611, so "off" is just flushing them. Pi-hole, the Pi
//! itself and every other device are never touched.

use crate::config::Config;
use anyhow::Result;
use geo_common::sh::{run, try_run};

const TABLE: &str = "4611";
const PRIO_LAN: &str = "4610"; // device -> LAN stays local (casting, Pi-hole UI, ...)
const PRIO_VPN: &str = "4611"; // device -> everything else uses TABLE

const CHAINS: [(&str, &str, &str); 4] = [
    ("nat", "PREROUTING", "GU_PRE"),
    ("nat", "POSTROUTING", "GU_POST"),
    ("filter", "FORWARD", "GU_FWD"),
    ("mangle", "FORWARD", "GU_MSS"),
];

fn ipt(table: &str, args: &[&str]) -> Result<()> {
    let mut a = vec!["-w", "-t", table];
    a.extend_from_slice(args);
    run("iptables", &a).map(|_| ())
}

fn ipt_ok(table: &str, args: &[&str]) -> bool {
    let mut a = vec!["-w", "-t", table];
    a.extend_from_slice(args);
    try_run("iptables", &a).ok()
}

/// Enable forwarding and hook our chains in. Idempotent.
pub fn init(cfg: &Config) -> Result<()> {
    let redirects = format!("net.ipv4.conf.{}.send_redirects=0", cfg.lan_iface);
    for kv in [
        "net.ipv4.ip_forward=1",
        // Otherwise the Pi tells devices "talk to the router directly".
        "net.ipv4.conf.all.send_redirects=0",
        redirects.as_str(),
    ] {
        run("sysctl", ["-qw", kv])?;
    }
    for (table, parent, chain) in CHAINS {
        ipt_ok(table, &["-N", chain]);
        if !ipt_ok(table, &["-C", parent, "-j", chain]) {
            ipt(table, &["-I", parent, "1", "-j", chain])?;
        }
    }
    clear(cfg);
    Ok(())
}

/// Direct mode: devices use the Pi as a plain router to the home router.
pub fn clear(cfg: &Config) {
    for (table, _, chain) in CHAINS {
        ipt_ok(table, &["-F", chain]);
    }
    for prio in [PRIO_LAN, PRIO_VPN] {
        for _ in 0..512 {
            if !try_run("ip", ["rule", "del", "priority", prio]).ok() {
                break;
            }
        }
    }
    try_run("ip", ["route", "flush", "table", TABLE]);
    flush_conntrack(cfg);
}

/// Drop existing connections so they re-open through the new path.
fn flush_conntrack(cfg: &Config) {
    for ip in cfg.device_ips() {
        try_run("conntrack", ["-D", "-s", &ip]);
        try_run("conntrack", ["-D", "-d", &ip]);
    }
}

/// Send all traffic (and DNS) of the configured devices through `iface`, with a kill switch.
pub fn via_tunnel(cfg: &Config, iface: &str, dns: &str) -> Result<()> {
    clear(cfg);
    let lan = cfg.lan_subnet.as_str();
    run("ip", ["route", "replace", "default", "dev", iface, "table", TABLE])?;
    // Kill switch: if the tunnel interface disappears, the blackhole remains.
    run("ip", ["route", "add", "blackhole", "default", "metric", "32000", "table", TABLE])?;
    for dir in ["-o", "-i"] {
        ipt("mangle", &["-A", "GU_MSS", dir, iface, "-p", "tcp", "--tcp-flags", "SYN,RST", "SYN",
                        "-j", "TCPMSS", "--clamp-mss-to-pmtu"])?;
    }
    let dns_dest = format!("{dns}:53");
    for ip in cfg.device_ips() {
        let ip = ip.as_str();
        run("ip", ["rule", "add", "from", ip, "to", lan, "lookup", "main", "priority", PRIO_LAN])?;
        run("ip", ["rule", "add", "from", ip, "lookup", TABLE, "priority", PRIO_VPN])?;
        // DNS must resolve from the VPN's location, or CDNs serve the home region.
        for proto in ["udp", "tcp"] {
            ipt("nat", &["-A", "GU_PRE", "-s", ip, "-p", proto, "--dport", "53",
                         "-j", "DNAT", "--to-destination", &dns_dest])?;
        }
        ipt("nat", &["-A", "GU_POST", "-s", ip, "-o", iface, "-j", "MASQUERADE"])?;
        ipt("filter", &["-A", "GU_FWD", "-s", ip, "-o", iface, "-j", "ACCEPT"])?;
        ipt("filter", &["-A", "GU_FWD", "-d", ip, "-i", iface, "-m", "conntrack",
                        "--ctstate", "ESTABLISHED,RELATED", "-j", "ACCEPT"])?;
        ipt("filter", &["-A", "GU_FWD", "-s", ip, "-d", lan, "-j", "ACCEPT"])?;
        // Never leak to the home connection.
        ipt("filter", &["-A", "GU_FWD", "-s", ip, "-j", "REJECT"])?;
    }
    Ok(())
}
