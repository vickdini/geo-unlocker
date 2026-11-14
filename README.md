# Geo Unlocker

Two native apps written in Rust. Each is a single binary with its web page built in,
and needs no interpreter or runtime.

| App | Runs on | Options | Routes |
|---|---|---|---|
| **geo-unlocker-pi** | Raspberry Pi (next to Pi-hole) | OpenVPN, WireGuard | **All traffic from the devices you list** (TVs, monitors, sticks) |
| **geo-unlocker** | Windows, macOS, Linux | Tor, WireGuard VPN (built in), Azure cloud VPN | **All of that computer's traffic** |

Use the desktop app on its own, or together with the Pi for devices that can't run a VPN
(smart TVs, monitors, streaming sticks). They work together: both use the same WireGuard `.conf` / OpenVPN `.ovpn` profiles, and the
desktop app can **send profiles to the Pi**, including a cloud server it created. Each cloud
server has a separate key for the Pi, so the computer and the TVs can use it at the same time.

```
crates/common    shared: profiles, jobs, state file, HTTP helpers, process helpers
crates/pi        Pi app (Linux only)
crates/desktop   desktop app
pi/              Pi installer + systemd unit
```

---

## Pi app

Every device on the list (by IP) has **all** its traffic sent through the VPN the Pi is
connected to. Everything else on your network, including the Pi itself and Pi-hole, keeps
using your normal connection.

```
                                ┌─ Off ──► eth0 ─► home router ─► internet
Listed devices ─► Pi (gateway) ─┤
                                └─ VPN ──► guwg0 / gutun0 ─► VPN server ─► internet
Other devices ──────────────────────────► home router (unchanged)
```

* Each listed device uses the Pi as its **gateway** (set once in the device's network settings).
* Source-based `ip rule`s send the listed IPs to a private routing table (4611) pointing at the
  tunnel. The tunnel never changes the Pi's own routes (WireGuard `Table = off`, OpenVPN
  `--route-nopull`, route directives stripped).
* Their DNS is redirected through the tunnel so streaming CDNs see the VPN location
  (those devices skip Pi-hole's ad blocking while connected).
* **Kill switch:** if the tunnel drops, listed devices are blocked rather than leaking.
* Devices can be added or removed in the web page at any time. Rules are reapplied immediately.

### Install

From a release (`geo-unlocker-pi-arm64.tar.gz` for 64-bit Raspberry Pi OS, `-armhf` for 32-bit):

```bash
tar xzf geo-unlocker-pi-arm64.tar.gz && cd geo-unlocker-pi-arm64
sudo ./install.sh --device 192.168.1.50=TV --device 192.168.1.51=Monitor --password 'pick-one'
```

Or from source on the Pi (with Rust installed from https://rustup.rs): `sudo ./pi/install.sh ...`
builds it first.

Then on **each listed device**: network settings → manual: its IP, **Gateway = Pi IP**,
DNS = Pi IP. **Turn IPv6 off** on those devices.

Web page: `http://<pi-ip>:8088`. Config: `/etc/geo-unlocker/config.json`; profiles/state:
`/var/lib/geo-unlocker` (root-only). Logs: `journalctl -u geo-unlocker -f`.

HTTP API (basic auth with the Pi password), also used by the desktop app:

| | |
|---|---|
| `GET /api/status` | active profile, devices, tunnel stats |
| `GET` / `POST /api/vpn/profiles` | list / add `{name, text, username?, password?}` |
| `DELETE /api/vpn/profiles/<name>` | remove |
| `POST /api/vpn/connect` `{name}` · `POST /api/off` | switch |
| `GET` / `PUT /api/devices` | `[{ip, name}]` |

---

## Desktop app

### Install (nothing else needed)

| | Download | Notes |
|---|---|---|
| Windows 10/11 | `GeoUnlockerSetup-<version>-x64.exe` | Start menu + optional desktop shortcut; uninstall from Settings › Apps |
| macOS 11+ | `GeoUnlocker-<version>-macos-arm64.dmg` / `-x64.dmg` | Drag to Applications. Not notarized yet: right-click › Open the first time |
| Linux (Debian/Ubuntu) | `geo-unlocker_<version>_amd64.deb` | `sudo apt install ./geo-unlocker_*.deb` pulls in Tor, iptables etc. automatically |

Everything the features need is included:

* **WireGuard** runs inside the app (Cloudflare's [boringtun](https://github.com/cloudflare/boringtun)) on
  the app's own virtual network adapter (Windows: WireGuard's signed Wintun driver, embedded).
* **Tor**: the official Tor Expert Bundle ships with the app (Linux: the distro's `tor` package).
* **Azure** is driven through its web APIs; sign-in is Microsoft's standard device code. No `az` CLI.
* The window is fully native (Rust + [iced](https://iced.rs), drawn on the GPU); no web engine.
  Windows: the C runtime is linked in.

### Use

The app opens a native, ProtonVPN-style window: a connection card with **Quick connect / Disconnect**,
Tor / VPN / Cloud lists with search, a world map showing your location and exit, and a live
session-traffic chart. The **Raspberry Pi** button (top right) links the Pi.

* **🧅 Tor**: choose an exit country. **Whole computer** (default) sends every app's traffic and
  DNS through Tor; **Proxy apps only** sets the system SOCKS proxy (`127.0.0.1:9155`). Tor carries
  TCP only, so some games/calls stop working while connected. Not suitable for streaming.
* **🔐 VPN**: import a WireGuard `.conf` from any provider; it carries **all** of the computer's
  traffic. OpenVPN `.ovpn` profiles can be stored and sent to the Raspberry Pi (**→ Pi**), which
  runs OpenVPN; they don't connect the computer itself.
* **☁️ Cloud (Azure)**: sign in with your Microsoft account (the app shows a code to enter at
  microsoft.com/devicelogin), pick a country, and the app creates a small Ubuntu VM running
  WireGuard in your subscription and connects. **Use on the Pi** sends the same server to the Pi.
  **Delete** removes the whole resource group. A free Azure account works.

Behaviour:

* **Windows** asks for administrator rights (UAC) at launch. **macOS / Linux**: the window runs as
  you; a small background service runs as administrator (system password prompt) and exits when
  the window closes.
* Closing the window disconnects and restores your normal connection. Uninstalling also undoes any
  leftover network change (`geo-unlocker --cleanup`).
* The map shows where the internet sees you, via `ipinfo.io` (through the tunnel when connected).
* Log file: `geo-unlocker.log` in `%ProgramData%\GeoUnlocker` (Windows),
  `/Library/Application Support/GeoUnlocker` (macOS), `/var/lib/geo-unlocker-desktop` (Linux).
* Third-party components and licenses: `crates/desktop/static/NOTICE.md`.

Cloud costs (Azure, US East; you pay only for what you use):

| Cost | Price | Charged |
|---|---|---|
| Virtual machine (B1s) | US$0.010 / hour | while the server exists |
| Public IP address | US$0.005 / hour | while the server exists |
| **Total** | **≈ US$0.015 / hour** | |
| Data downloaded | US$0.087 / GB | after the first 100 GB each month |

HD video uses about 3 GB per hour. The server card shows the running cost; **Delete** stops all
charges. Big streaming services block many data-centre addresses, so a cloud server won't unlock
every service.

---

## Automatic updates

Both apps update themselves from this repository's **GitHub Releases**:

* **Desktop**: checks 20 s after start and every 6 hours. A newer release is downloaded and
  verified, then installed silently **as soon as you're disconnected** (it never interrupts a
  VPN/Tor connection), and the app restarts. *About & updates* (top bar) shows the version, has
  **Check now / Install now**, and an **Install updates automatically** switch.
* **Pi**: checks a minute after start and every 12 hours, replaces its binary and restarts the
  service (the active profile reconnects within seconds). Toggle under *Updates* on the Pi page or
  `"auto_update": false` in `/etc/geo-unlocker/config.json`.

Updates are **signed**: each release has `SHA256SUMS.txt` plus `SHA256SUMS.txt.sig`, an Ed25519
signature made in CI. The apps embed the public key (`packaging/update-public-key.txt`) and only
install a file whose checksum is in a correctly signed list. A tampered or corrupted download is
refused.

### Publishing a release

1. One-time: add the repository secret **`UPDATE_SIGNING_KEY`** (Settings → Secrets and variables
   → Actions) with the contents of your private key file. Keep a backup of that file somewhere
   safe; without it you can't ship updates to existing installs.
2. Tag and push: `git tag v0.3.0 && git push origin v0.3.0`.
3. The workflow builds every installer and Pi bundle with that version and the repository name
   built in, signs the checksum list, verifies it like the apps will, and publishes the release.
   Installed apps pick it up automatically.

Local builds (without the release pipeline) don't auto-update. To rotate keys:
`cargo run -p update-sign -- keygen <new-private-key-file>` and put the printed public key in
`packaging/update-public-key.txt` (apps built before the change keep trusting the old key).

## Building

```bash
cargo test --workspace --lib
cargo run -p geo-unlocker --example preview   # the window with sample data (no admin, no network changes)
cargo build --release -p geo-unlocker                                           # desktop app
cross build --release -p geo-unlocker-pi --target aarch64-unknown-linux-musl   # Pi (64-bit)
```

Installers:

```bash
# Windows (needs Inno Setup 6)
python packaging/fetch_tor.py windows-x86_64 dist/windows/tor   # official Tor Expert Bundle, checksum-verified
iscc /DAppVersion=0.2.0 packaging/windows/geo-unlocker.iss      # -> dist/GeoUnlockerSetup-0.2.0-x64.exe
# macOS
bash packaging/macos/make-dmg.sh aarch64-apple-darwin macos-aarch64 0.2.0
# Linux
cargo install cargo-deb && cargo deb -p geo-unlocker
```

`.github/workflows/build.yml` builds all installers plus the Pi bundles on every push, and attaches
them to a GitHub release for `v*` tags. The Pi binaries are static (musl).

### How whole-computer Tor works

```
apps ─► Tor adapter (10.255.233.2) ─► tun2proxy ─► Tor SOCKS 127.0.0.1:9155 ─► Tor network
                                                    Tor's own connections ─► normal route
```

* Two `/1` routes send all IPv4 into the adapter; your LAN stays reachable.
* Tor's own connections must not loop back into the adapter: Tor binds them to the computer's
  real address (`OutboundBindAddress`). Windows (strong host model) then keeps them on the real
  interface; on Linux a source-address rule sends them via the real gateway; on macOS the app also
  adds host routes for Tor's guard relays (read from Tor's control port, refreshed every 30 s).
* DNS: Windows uses an NRPT rule for every name, Linux redirects port 53 with iptables, and macOS
  sets each network service's DNS (restored afterwards). All point to `1.1.1.1`, which is answered
  inside the tunnel.
* The adapter disappears when the app exits, taking its routes with it. Leftover DNS/firewall
  settings are harmless (1.1.1.1 still works directly) and are removed on the next start.
* Windows: the signed `wintun.dll` from wintun.net is embedded unmodified and written to
  `%ProgramData%\GeoUnlocker` (see `crates/desktop/assets/wintun`).

## Limitations

* Big streaming services block Tor and many VPN/cloud IP ranges; using a VPN may break their terms.
* Desktop: no kill switch yet. If the VPN or Tor tunnel stops, traffic falls back to the normal
  connection. IPv6 is blocked while connected; tunnels carry IPv4 only.
* Desktop VPN: WireGuard profiles with a single server (what providers hand out).
* Each routed device needs a fixed IP and the Pi as its gateway.
