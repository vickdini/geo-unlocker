//! Built-in WireGuard: the protocol runs inside the app (boringtun) on our own
//! TUN adapter, so nothing has to be installed. One peer, all traffic.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tun2proxy::CancellationToken;

/// The parts of a WireGuard .conf we use.
#[derive(Debug, Clone)]
pub struct WgConfig {
    pub private_key: [u8; 32],
    pub address: Ipv4Addr,
    pub prefix: u8,
    pub dns: Option<Ipv4Addr>,
    pub mtu: u16,
    pub peer_public: [u8; 32],
    pub preshared: Option<[u8; 32]>,
    pub endpoint: SocketAddr,
    pub keepalive: Option<u16>,
}

fn key32(s: &str, what: &str) -> Result<[u8; 32]> {
    let b = base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|_| anyhow!("{what} isn't valid base64"))?;
    b.try_into().map_err(|_| anyhow!("{what} must be 32 bytes"))
}

impl WgConfig {
    pub fn parse(text: &str) -> Result<Self> {
        let (mut section, mut peers) = (String::new(), 0);
        let (mut private_key, mut address, mut prefix, mut dns, mut mtu) = (None, None, 32u8, None, 1420u16);
        let (mut peer_public, mut preshared, mut endpoint, mut keepalive) = (None, None, None, None);
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.starts_with('[') {
                section = line.to_ascii_lowercase();
                if section == "[peer]" {
                    peers += 1;
                }
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            match (section.as_str(), k.as_str()) {
                ("[interface]", "privatekey") => private_key = Some(key32(v, "PrivateKey")?),
                ("[interface]", "address") => {
                    for a in v.split(',') {
                        let (ip, p) = a.trim().split_once('/').unwrap_or((a.trim(), "32"));
                        if let Ok(ip) = ip.parse::<Ipv4Addr>() {
                            address = Some(ip);
                            prefix = p.parse().unwrap_or(32);
                            break;
                        }
                    }
                }
                ("[interface]", "dns") => dns = v.split(',').find_map(|d| d.trim().parse::<Ipv4Addr>().ok()),
                ("[interface]", "mtu") => mtu = v.parse().unwrap_or(1420),
                ("[peer]", "publickey") => peer_public = Some(key32(v, "PublicKey")?),
                ("[peer]", "presharedkey") => preshared = Some(key32(v, "PresharedKey")?),
                ("[peer]", "persistentkeepalive") => keepalive = v.parse().ok().filter(|k| *k > 0),
                ("[peer]", "endpoint") => endpoint = Some(v.to_string()),
                _ => {}
            }
        }
        if peers != 1 {
            bail!("This WireGuard profile has {peers} peers; only profiles with exactly one server are supported.");
        }
        let endpoint = endpoint.context("The profile has no Endpoint")?;
        let addrs: Vec<SocketAddr> = endpoint
            .to_socket_addrs()
            .with_context(|| format!("Can't find the VPN server {endpoint}"))?
            .collect();
        let endpoint = addrs
            .iter()
            .find(|a| a.is_ipv4())
            .copied()
            .context("The VPN server has no IPv4 address")?;
        Ok(Self {
            private_key: private_key.context("The profile has no PrivateKey")?,
            address: address.context("The profile has no IPv4 Address")?,
            prefix,
            dns,
            mtu: mtu.clamp(1280, 1500),
            peer_public: peer_public.context("The profile has no peer PublicKey")?,
            preshared,
            endpoint,
            keepalive: keepalive.or(Some(25)),
        })
    }

    pub fn endpoint_ip(&self) -> String {
        self.endpoint.ip().to_string()
    }
}

#[cfg(windows)]
const TUN_NAME: Option<&str> = Some("GeoUnlockerVPN");
#[cfg(target_os = "linux")]
const TUN_NAME: Option<&str> = Some("guwg0");
#[cfg(target_os = "macos")]
const TUN_NAME: Option<&str> = None;

#[derive(Default)]
struct Stats {
    rx: AtomicU64,
    tx: AtomicU64,
    /// seconds since the last handshake + 1 (0 = never)
    handshake: AtomicU64,
}

#[derive(Default)]
pub struct WgTunnel {
    running: Mutex<Option<(CancellationToken, JoinHandle<()>)>>,
    stats: Arc<Stats>,
}

impl WgTunnel {
    pub fn running(&self) -> bool {
        self.running.lock().unwrap().as_ref().is_some_and(|(_, h)| !h.is_finished())
    }

    /// (seconds since last handshake, received bytes, sent bytes)
    pub fn stats(&self) -> (Option<u64>, u64, u64) {
        let h = self.stats.handshake.load(Ordering::Relaxed);
        (h.checked_sub(1), self.stats.rx.load(Ordering::Relaxed), self.stats.tx.load(Ordering::Relaxed))
    }

    /// Create the adapter and run WireGuard. Returns the adapter's interface name.
    #[allow(unused_variables)]
    pub fn start(&self, cfg: &WgConfig, state_dir: &Path) -> Result<String> {
        self.stop();
        for a in [&self.stats.rx, &self.stats.tx, &self.stats.handshake] {
            a.store(0, Ordering::Relaxed);
        }
        let mask = if cfg.prefix == 0 { 0 } else { u32::MAX << (32 - cfg.prefix.min(32)) };
        let mut tc = tun::Configuration::default();
        tc.address(cfg.address).netmask(Ipv4Addr::from(mask)).destination(cfg.address).mtu(cfg.mtu).up();
        if let Some(n) = TUN_NAME {
            tc.tun_name(n);
        }
        #[cfg(windows)]
        {
            let dll = crate::systun::wintun_path(state_dir)?;
            tc.platform_config(|c| {
                c.wintun_file(dll.clone());
                c.device_guid(0x6765_6f75_6e6c_6f63_6b65_722d_7667_7701);
                c.wait_for_interfaces(true, false, Duration::from_secs(15));
            });
        }
        let tunn = Tunn::new(
            StaticSecret::from(cfg.private_key),
            PublicKey::from(cfg.peer_public),
            cfg.preshared,
            cfg.keepalive,
            rand_index(),
            None,
        );
        let tunn = Arc::new(Mutex::new(tunn));
        let (endpoint, stats) = (cfg.endpoint, self.stats.clone());
        let token = CancellationToken::new();
        let stop = token.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Result<String>>();
        let handle = std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(e.into()));
                    return;
                }
            };
            rt.block_on(async move {
                let device = match tun::create_as_async(&tc) {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow!("can't create the virtual network adapter: {e}")));
                        return;
                    }
                };
                let name = tun::AbstractDevice::tun_name(&*device).unwrap_or_default();
                let udp = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
                    Ok(u) => u,
                    Err(e) => {
                        let _ = tx.send(Err(e.into()));
                        return;
                    }
                };
                let _ = tx.send(Ok(name));
                tokio::select! {
                    r = run_tunnel(device, udp, endpoint, tunn, stats) => {
                        if let Err(e) = r { geo_common::log(format!("WireGuard stopped: {e:#}")); }
                    }
                    _ = stop.cancelled() => {}
                }
            });
        });
        let name = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| anyhow!("timed out creating the virtual network adapter"))??;
        *self.running.lock().unwrap() = Some((token, handle));
        Ok(name)
    }

    pub fn stop(&self) {
        if let Some((token, handle)) = self.running.lock().unwrap().take() {
            token.cancel();
            let _ = handle.join();
        }
    }
}

fn rand_index() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_le_bytes(b) >> 8
}

const BUF: usize = 65_536;

/// Shuttle packets: adapter -> encrypt -> UDP, UDP -> decrypt -> adapter, plus timers.
async fn run_tunnel(
    device: tun::AsyncDevice,
    udp: tokio::net::UdpSocket,
    endpoint: SocketAddr,
    tunn: Arc<Mutex<Tunn>>,
    stats: Arc<Stats>,
) -> Result<()> {
    let udp = Arc::new(udp);
    let (mut tun_rd, mut tun_wr) = tokio::io::split(device);

    // Start the handshake right away.
    let init = {
        let mut out = vec![0u8; 256];
        match tunn.lock().unwrap().format_handshake_initiation(&mut out, false) {
            TunnResult::WriteToNetwork(p) => Some(p.to_vec()),
            _ => None,
        }
    };
    if let Some(p) = init {
        udp.send_to(&p, endpoint).await?;
    }

    // Adapter -> network.
    let outbound = {
        let (udp, tunn) = (udp.clone(), tunn.clone());
        async move {
            let (mut buf, mut out) = (vec![0u8; BUF], vec![0u8; BUF + 64]);
            loop {
                let n = tun_rd.read(&mut buf).await?;
                if n == 0 {
                    return Ok::<_, anyhow::Error>(());
                }
                let pkt = match tunn.lock().unwrap().encapsulate(&buf[..n], &mut out) {
                    TunnResult::WriteToNetwork(p) => Some(p.to_vec()),
                    _ => None,
                };
                if let Some(p) = pkt {
                    udp.send_to(&p, endpoint).await?;
                }
            }
        }
    };

    // Network -> adapter.
    let inbound = {
        let (udp, tunn) = (udp.clone(), tunn.clone());
        async move {
            let (mut buf, mut out) = (vec![0u8; BUF], vec![0u8; BUF]);
            loop {
                let (n, from) = udp.recv_from(&mut buf).await?;
                if from != endpoint {
                    continue;
                }
                let (mut replies, mut to_tun) = (Vec::new(), None);
                {
                    let mut t = tunn.lock().unwrap();
                    match t.decapsulate(Some(from.ip()), &buf[..n], &mut out) {
                        TunnResult::WriteToNetwork(p) => {
                            replies.push(p.to_vec());
                            // Flush packets queued while the handshake was in progress.
                            while let TunnResult::WriteToNetwork(p) = t.decapsulate(None, &[], &mut out) {
                                replies.push(p.to_vec());
                            }
                        }
                        TunnResult::WriteToTunnelV4(p, _) => to_tun = Some(p.to_vec()),
                        TunnResult::WriteToTunnelV6(p, _) => to_tun = Some(p.to_vec()),
                        _ => {}
                    }
                }
                for r in replies {
                    udp.send_to(&r, endpoint).await?;
                }
                if let Some(p) = to_tun {
                    tun_wr.write_all(&p).await?;
                }
            }
        }
    };

    // Timers: handshakes, keepalives, stats.
    let timers = {
        let (udp, tunn) = (udp.clone(), tunn.clone());
        async move {
            let mut out = vec![0u8; BUF];
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            loop {
                tick.tick().await;
                let pkt = {
                    let mut t = tunn.lock().unwrap();
                    let (hs, tx, rx, _, _) = t.stats();
                    stats.tx.store(tx as u64, Ordering::Relaxed);
                    stats.rx.store(rx as u64, Ordering::Relaxed);
                    stats.handshake.store(hs.map(|d| d.as_secs() + 1).unwrap_or(0), Ordering::Relaxed);
                    match t.update_timers(&mut out) {
                        TunnResult::WriteToNetwork(p) => Some(p.to_vec()),
                        _ => None,
                    }
                };
                if let Some(p) = pkt {
                    udp.send_to(&p, endpoint).await?;
                }
            }
            #[allow(unreachable_code)]
            Ok::<_, anyhow::Error>(())
        }
    };

    tokio::select! {
        r = outbound => r,
        r = inbound => r,
        r = timers => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_provider_config() {
        let conf = "[Interface]\n# Key for test\nPrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=\n\
                    Address = 10.2.0.2/32, fd00::2/128\nDNS = 10.2.0.1\n\n[Peer]\n\
                    PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nAllowedIPs = 0.0.0.0/0, ::/0\n\
                    Endpoint = 127.0.0.1:51820\n";
        let c = WgConfig::parse(conf).unwrap();
        assert_eq!(c.address, Ipv4Addr::new(10, 2, 0, 2));
        assert_eq!(c.prefix, 32);
        assert_eq!(c.dns, Some(Ipv4Addr::new(10, 2, 0, 1)));
        assert_eq!(c.endpoint, "127.0.0.1:51820".parse().unwrap());
        assert_eq!(c.keepalive, Some(25));
        let two = format!("{conf}[Peer]\nPublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=\nEndpoint = 127.0.0.1:1\n");
        assert!(WgConfig::parse(&two).is_err());
        assert!(WgConfig::parse("[Interface]\nPrivateKey = nope\n[Peer]\n").is_err());
    }

    /// Two boringtun peers complete a handshake and exchange a packet through
    /// the same encapsulate/decapsulate calls the tunnel loop uses.
    #[test]
    fn handshake_between_two_peers() {
        let a_priv = StaticSecret::from([1u8; 32]);
        let b_priv = StaticSecret::from([2u8; 32]);
        let (a_pub, b_pub) = (PublicKey::from(&a_priv), PublicKey::from(&b_priv));
        let mut a = Tunn::new(a_priv, b_pub, None, None, 1, None);
        let mut b = Tunn::new(b_priv, a_pub, None, None, 2, None);
        let (mut buf_a, mut buf_b) = (vec![0u8; 2048], vec![0u8; 2048]);
        let init = match a.format_handshake_initiation(&mut buf_a, false) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            _ => panic!("no initiation"),
        };
        let resp = match b.decapsulate(None, &init, &mut buf_b) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            _ => panic!("no response"),
        };
        let keepalive = match a.decapsulate(None, &resp, &mut buf_a) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            _ => panic!("no keepalive"),
        };
        assert!(matches!(b.decapsulate(None, &keepalive, &mut buf_b), TunnResult::Done));
        // A minimal IPv4 header (20 bytes) addressed 10.0.0.2 -> 1.1.1.1.
        let mut ip = vec![0x45, 0, 0, 20, 0, 0, 0, 0, 64, 17, 0, 0, 10, 0, 0, 2, 1, 1, 1, 1];
        ip.resize(20, 0);
        let enc = match a.encapsulate(&ip, &mut buf_a) {
            TunnResult::WriteToNetwork(p) => p.to_vec(),
            _ => panic!("no data packet"),
        };
        match b.decapsulate(None, &enc, &mut buf_b) {
            TunnResult::WriteToTunnelV4(p, src) => {
                assert_eq!(p, &ip[..]);
                assert_eq!(src, Ipv4Addr::new(10, 0, 0, 2));
            }
            _ => panic!("packet not delivered"),
        }
    }
}
