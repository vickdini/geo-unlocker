//! Whole-computer Tor: a virtual network adapter (TUN) whose traffic is handed to
//! Tor's SOCKS port by tun2proxy. DNS is answered inside the tunnel ("virtual DNS"),
//! so names are resolved by Tor exits, never by the local network.
//!
//! Routes/DNS that point the computer at the adapter are set by the platform layer
//! (`Platform::tun_up`). The adapter disappears when this process exits, taking its
//! routes with it, so a crash cannot leave the computer offline.

use anyhow::{anyhow, Result};
use std::net::Ipv4Addr;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;
use tun2proxy::{ArgDns, Args, CancellationToken, ProxyParameters};

/// Point-to-point addresses for the adapter (an unusual /30 to avoid clashing with home LANs).
pub const TUN_ADDR: Ipv4Addr = Ipv4Addr::new(10, 255, 233, 2);
pub const TUN_PEER: Ipv4Addr = Ipv4Addr::new(10, 255, 233, 1);
const TUN_MASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 252);
const MTU: u16 = 1500;

#[cfg(target_os = "linux")]
const TUN_NAME: Option<&str> = Some("gutor0");
#[cfg(windows)]
const TUN_NAME: Option<&str> = Some("GeoUnlockerTor");
#[cfg(target_os = "macos")]
const TUN_NAME: Option<&str> = None; // macOS assigns utunN

#[cfg(all(windows, target_arch = "x86_64"))]
const WINTUN_DLL: &[u8] = include_bytes!("../assets/wintun/amd64/wintun.dll");
#[cfg(all(windows, target_arch = "aarch64"))]
const WINTUN_DLL: &[u8] = include_bytes!("../assets/wintun/arm64/wintun.dll");

/// Write the bundled, signed wintun.dll next to our state (unchanged bytes).
#[cfg(windows)]
pub(crate) fn wintun_path(dir: &Path) -> Result<std::path::PathBuf> {
    let p = dir.join("wintun.dll");
    if std::fs::read(&p).map(|b| b != WINTUN_DLL).unwrap_or(true) {
        std::fs::write(&p, WINTUN_DLL)?;
    }
    Ok(p)
}

/// Bytes carried by the Tor adapter (cumulative, reported by tun2proxy every second).
static TX: AtomicU64 = AtomicU64::new(0);
static RX: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn on_traffic(status: *const tun2proxy::TrafficStatus, _ctx: *mut std::ffi::c_void) {
    if let Some(s) = unsafe { status.as_ref() } {
        TX.store(s.tx, Ordering::Relaxed);
        RX.store(s.rx, Ordering::Relaxed);
    }
}

/// (received, sent) bytes through the Tor adapter since the app started.
pub fn traffic() -> (u64, u64) {
    (RX.load(Ordering::Relaxed), TX.load(Ordering::Relaxed))
}

#[derive(Default)]
pub struct SysTunnel {
    running: Mutex<Option<(CancellationToken, JoinHandle<()>)>>,
}

impl SysTunnel {
    pub fn running(&self) -> bool {
        self.running.lock().unwrap().as_ref().is_some_and(|(_, h)| !h.is_finished())
    }

    /// Create the adapter and start forwarding to socks5://127.0.0.1:`socks_port`.
    /// Returns the adapter's interface name.
    #[allow(unused_variables)]
    pub fn start(&self, socks_port: u16, state_dir: &Path) -> Result<String> {
        self.stop();
        let mut cfg = tun::Configuration::default();
        cfg.address(TUN_ADDR).netmask(TUN_MASK).destination(TUN_PEER).mtu(MTU).up();
        if let Some(name) = TUN_NAME {
            cfg.tun_name(name);
        }
        #[cfg(windows)]
        {
            let dll = wintun_path(state_dir)?;
            cfg.platform_config(|c| {
                c.wintun_file(dll.clone());
                c.device_guid(0x6765_6f75_6e6c_6f63_6b65_722d_746f_7201);
                c.wait_for_interfaces(true, false, Duration::from_secs(15));
            });
        }

        unsafe { tun2proxy::tun2proxy_set_traffic_status_callback(1, Some(on_traffic), std::ptr::null_mut()) };
        let mut args = Args::default();
        let proxy = ProxyParameters::from_str(&format!("socks5://127.0.0.1:{socks_port}"))
            .map_err(|e| anyhow!("bad proxy address: {e}"))?;
        args.proxy(proxy).dns(ArgDns::Virtual).setup(false);

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
                let device = match tun::create_as_async(&cfg) {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tx.send(Err(anyhow!("can't create the virtual network adapter: {e}")));
                        return;
                    }
                };
                let name = tun::AbstractDevice::tun_name(&*device).unwrap_or_default();
                let _ = tx.send(Ok(name));
                if let Err(e) = tun2proxy::run(device, MTU, args, stop).await {
                    geo_common::log(format!("tor tunnel stopped: {e}"));
                }
            });
        });
        let name = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| anyhow!("timed out creating the virtual network adapter"))??;
        *self.running.lock().unwrap() = Some((token, handle));
        Ok(name)
    }

    /// Stop forwarding; dropping the device removes the adapter and its routes.
    pub fn stop(&self) {
        if let Some((token, handle)) = self.running.lock().unwrap().take() {
            token.cancel();
            let _ = handle.join();
        }
    }
}
