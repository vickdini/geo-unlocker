//! Tor on the desktop: a private tor process + the system SOCKS proxy pointed at it.

use crate::platform::Platform;
use anyhow::{bail, Result};
use geo_common::jobs::Job;
use geo_common::sh::command;
use geo_common::state::State;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Minimal Tor control-protocol client.
struct Control {
    r: BufReader<TcpStream>,
    w: TcpStream,
}

impl Control {
    /// Send a command; return the reply's lines (without status codes).
    fn cmd(&mut self, cmd: &str) -> Result<Vec<String>> {
        self.w.write_all(cmd.as_bytes())?;
        self.w.write_all(b"\r\n")?;
        let mut out = Vec::new();
        loop {
            let mut line = String::new();
            if self.r.read_line(&mut line)? == 0 {
                bail!("tor closed the control connection");
            }
            let line = line.trim_end().to_string();
            if line.len() < 4 {
                out.push(line);
                continue;
            }
            let (code, sep, rest) = (&line[..3], &line[3..4], &line[4..]);
            if !code.starts_with('2') && code.chars().all(|c| c.is_ascii_digit()) {
                bail!("tor control: {line}");
            }
            match sep {
                " " if code.chars().all(|c| c.is_ascii_digit()) => {
                    if rest != "OK" {
                        out.push(rest.to_string());
                    }
                    return Ok(out);
                }
                "-" | "+" if code.chars().all(|c| c.is_ascii_digit()) => {
                    if !rest.is_empty() {
                        out.push(rest.to_string());
                    }
                }
                _ => {
                    if line != "." {
                        out.push(line);
                    }
                }
            }
        }
    }
}

/// Not 9050/9150, so it doesn't clash with a system tor or Tor Browser.
pub const SOCKS_PORT: u16 = 9155;
const ONIONOO: &str =
    "https://onionoo.torproject.org/details?type=relay&running=true&flag=Exit&fields=country,advertised_bandwidth";

#[derive(Serialize, Clone)]
pub struct ExitCountry {
    code: String,
    relays: u32,
    mbps: u64,
}

pub struct TorClient {
    dir: PathBuf,
    proc_: Mutex<Option<Child>>,
    country: Mutex<Option<String>>,
    progress: Arc<AtomicU32>,
    log: Arc<Mutex<VecDeque<String>>>,
    countries: Mutex<Option<(Instant, Vec<ExitCountry>)>>,
}

impl TorClient {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join("tor"),
            proc_: Mutex::new(None),
            country: Mutex::new(None),
            progress: Arc::new(AtomicU32::new(0)),
            log: Arc::new(Mutex::new(VecDeque::new())),
            countries: Mutex::new(None),
        }
    }

    /// Start tor and wait until it's connected. `extra` adds torrc lines; with
    /// `set_proxy` the system SOCKS proxy is pointed at it (proxy-only mode).
    pub fn start(&self, plat: &dyn Platform, state: &State, country: &str, job: &Job, extra: &[String], set_proxy: bool) -> Result<()> {
        let country = country.to_ascii_lowercase();
        if !country.is_empty() && !(country.len() == 2 && country.chars().all(|c| c.is_ascii_lowercase())) {
            bail!("country must be a 2-letter code");
        }
        let Some(tor) = plat.tor() else { bail!("Tor is missing from this installation. Reinstall Geo Unlocker.") };
        self.stop(plat, state);

        let data = self.dir.join("data");
        std::fs::create_dir_all(&data)?;
        let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
        let mut lines = vec![
            format!("DataDirectory {}", slash(&data)),
            format!("SocksPort 127.0.0.1:{SOCKS_PORT}"),
            "Log notice stdout".to_string(),
            // Control port (cookie-authenticated) to look up guard relays.
            "ControlPort auto".to_string(),
            format!("ControlPortWriteToFile {}", slash(&self.dir.join("control_port"))),
            "CookieAuthentication 1".to_string(),
            format!("CookieAuthFile {}", slash(&self.dir.join("control_auth_cookie"))),
        ];
        lines.extend(extra.iter().cloned());
        let geoip = plat.geoip_candidates(&tor).into_iter().find(|p| p.is_file());
        if let Some(g) = &geoip {
            lines.push(format!("GeoIPFile {}", slash(g)));
            let v6 = g.with_file_name("geoip6");
            if v6.is_file() {
                lines.push(format!("GeoIPv6File {}", slash(&v6)));
            }
        }
        if !country.is_empty() {
            if geoip.is_none() {
                bail!("Tor's country database (geoip) is missing, so an exit country can't be chosen. \
                       Choose Fastest country, or reinstall Geo Unlocker.");
            }
            lines.push(format!("ExitNodes {{{country}}}"));
            lines.push("StrictNodes 1".into());
        }
        let torrc = self.dir.join("torrc");
        std::fs::write(&torrc, lines.join("\n") + "\n")?;

        self.progress.store(0, Ordering::SeqCst);
        self.log.lock().unwrap().clear();
        *self.country.lock().unwrap() = (!country.is_empty()).then(|| country.clone());
        let mut child = command(&tor)
            .arg("-f")
            .arg(&torrc)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("can't start tor: {e}"))?;
        let out = child.stdout.take().unwrap();
        *self.proc_.lock().unwrap() = Some(child);
        let (progress, log) = (self.progress.clone(), self.log.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                if let Some(i) = line.find("Bootstrapped ") {
                    let n: String = line[i + 13..].chars().take_while(char::is_ascii_digit).collect();
                    if let Ok(n) = n.parse() {
                        progress.store(n, Ordering::SeqCst);
                    }
                }
                let mut l = log.lock().unwrap();
                l.push_back(line);
                if l.len() > 50 {
                    l.pop_front();
                }
            }
        });

        job.log("Connecting to the Tor network…");
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut last = u32::MAX;
        loop {
            let p = self.progress.load(Ordering::SeqCst);
            if p != last {
                last = p;
                job.log(format!("Bootstrapped {p}%"));
            }
            if p >= 100 {
                break;
            }
            if !self.running() {
                let tail: Vec<String> = self.log.lock().unwrap().iter().rev().take(5).cloned().collect();
                bail!("tor exited: {}", tail.into_iter().rev().collect::<Vec<_>>().join(" | "));
            }
            if Instant::now() > deadline {
                self.stop(plat, state);
                bail!("Tor didn't finish connecting in 2 minutes{}",
                      if country.is_empty() { "" } else { " - the chosen country may have no usable exits" });
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        if !set_proxy {
            return Ok(());
        }
        job.log(format!("Setting the system SOCKS proxy to 127.0.0.1:{SOCKS_PORT}"));
        let saved = plat.set_socks_proxy("127.0.0.1", SOCKS_PORT)?;
        state.set("proxy_saved", &saved); // lets a crash be cleaned up on the next start
        if saved["manual"].as_bool() == Some(true) {
            job.log(format!("Couldn't set the proxy automatically: set SOCKS5 127.0.0.1:{SOCKS_PORT} in your apps"));
        }
        Ok(())
    }

    /// IP addresses of Tor's entry guards (the only relays it connects to directly).
    pub fn guard_ips(&self) -> Result<Vec<String>> {
        let port = std::fs::read_to_string(self.dir.join("control_port"))?;
        let addr = port.trim().strip_prefix("PORT=").unwrap_or(port.trim()).to_string();
        let cookie: String = std::fs::read(self.dir.join("control_auth_cookie"))?
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let stream = TcpStream::connect(&addr)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut ctl = Control { r: BufReader::new(stream.try_clone()?), w: stream };
        ctl.cmd(&format!("AUTHENTICATE {cookie}"))?;
        let fingerprints: Vec<String> = ctl
            .cmd("GETINFO entry-guards")?
            .iter()
            .filter_map(|l| {
                let l = l.trim_start_matches("entry-guards=");
                let fp = l.strip_prefix('$')?;
                Some(fp.split(['~', '=', ' ']).next()?.to_string())
            })
            .collect();
        let mut ips = Vec::new();
        for fp in fingerprints {
            // "r nickname identity digest date time IP ORPort DirPort"
            if let Ok(lines) = ctl.cmd(&format!("GETINFO ns/id/{fp}")) {
                if let Some(ip) = lines.iter().find_map(|l| l.strip_prefix("r ")?.split_whitespace().nth(5)) {
                    if ip.parse::<std::net::Ipv4Addr>().is_ok() && !ips.iter().any(|x: &String| x == ip) {
                        ips.push(ip.to_string());
                    }
                }
            }
        }
        Ok(ips)
    }

    pub fn running(&self) -> bool {
        self.proc_.lock().unwrap().as_mut().is_some_and(|p| matches!(p.try_wait(), Ok(None)))
    }

    pub fn restore_proxy(&self, plat: &dyn Platform, state: &State) {
        if let Some(saved) = state.get::<Value>("proxy_saved") {
            plat.restore_proxy(&saved);
            state.remove("proxy_saved");
        }
    }

    pub fn stop(&self, plat: &dyn Platform, state: &State) {
        self.restore_proxy(plat, state);
        if let Some(mut p) = self.proc_.lock().unwrap().take() {
            let _ = p.kill();
            let _ = p.wait();
        }
    }

    pub fn status(&self) -> Value {
        json!({
            "running": self.running(),
            "country": *self.country.lock().unwrap(),
            "bootstrap": self.progress.load(Ordering::SeqCst),
            "socks": format!("127.0.0.1:{SOCKS_PORT}"),
        })
    }

    /// Countries that currently have running exit relays (cached for an hour).
    pub fn exit_countries(&self) -> Result<Vec<ExitCountry>> {
        if let Some((t, list)) = &*self.countries.lock().unwrap() {
            if t.elapsed() < Duration::from_secs(3600) {
                return Ok(list.clone());
            }
        }
        let v: Value = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(20))
            .user_agent("geo-unlocker")
            .build()
            .get(ONIONOO)
            .call()
            .map_err(|e| anyhow::anyhow!("can't load the Tor relay list: {e}"))?
            .into_json()?;
        let mut agg: BTreeMap<String, (u32, f64)> = BTreeMap::new();
        for r in v["relays"].as_array().into_iter().flatten() {
            if let Some(c) = r["country"].as_str() {
                let e = agg.entry(c.to_string()).or_default();
                e.0 += 1;
                e.1 += r["advertised_bandwidth"].as_f64().unwrap_or(0.0) * 8.0 / 1e6;
            }
        }
        let mut list: Vec<ExitCountry> = agg
            .into_iter()
            .map(|(code, (relays, mbps))| ExitCountry { code, relays, mbps: mbps.round() as u64 })
            .collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.mbps));
        *self.countries.lock().unwrap() = Some((Instant::now(), list.clone()));
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A fake tor control port speaking the real reply format.
    fn fake_tor(listener: TcpListener) {
        let (s, _) = listener.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut w = s;
        let mut line = String::new();
        while r.read_line(&mut line).unwrap_or(0) > 0 {
            let reply = match line.trim_end() {
                l if l.starts_with("AUTHENTICATE 0a0b") => "250 OK\r\n".to_string(),
                "GETINFO entry-guards" => "250+entry-guards=\r\n$AAAA~guardA up\r\n$BBBB=guardB never-connected\r\n.\r\n250 OK\r\n".into(),
                "GETINFO ns/id/AAAA" => "250+ns/id/AAAA=\r\nr guardA aWQ ZGln 2026-11-13 07:00:00 192.0.2.10 9001 0\r\ns Fast Guard Running Stable Valid\r\n.\r\n250 OK\r\n".into(),
                "GETINFO ns/id/BBBB" => "250+ns/id/BBBB=\r\nr guardB aWQ ZGln 2026-11-13 07:00:00 198.51.100.7 443 0\r\n.\r\n250 OK\r\n".into(),
                _ => "552 Unrecognized\r\n".into(),
            };
            w.write_all(reply.as_bytes()).unwrap();
            line.clear();
        }
    }

    #[test]
    fn guard_ips_from_control_port() {
        let tmp = std::env::temp_dir().join(format!("gu-tor-test-{}", std::process::id()));
        let client = TorClient::new(&tmp);
        std::fs::create_dir_all(&client.dir).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(client.dir.join("control_port"), format!("PORT=127.0.0.1:{port}\n")).unwrap();
        std::fs::write(client.dir.join("control_auth_cookie"), [0x0a, 0x0b]).unwrap();
        let server = std::thread::spawn(move || fake_tor(listener));
        let ips = client.guard_ips().unwrap();
        assert_eq!(ips, ["192.0.2.10", "198.51.100.7"]);
        drop(client);
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = server;
    }
}
