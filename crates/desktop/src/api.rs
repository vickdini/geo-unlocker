//! The app's command interface. The native window calls `dispatch` directly on
//! Windows; on macOS/Linux the window runs as the user and reaches the privileged
//! service through a token-protected channel on 127.0.0.1 (see `service.rs`).
//! Both go through this one function, so behaviour is identical.

use crate::controller::{Ctl, Mode};
use crate::pilink;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::sync::Arc;

fn field(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

pub fn dispatch(ctl: &Arc<Ctl>, method: &str, path: &str, b: &Value) -> Result<Value> {
    let segs: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(geo_common::web::percent_decode).collect();
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    Ok(match (method, s.as_slice()) {
        ("GET", ["status"]) => ctl.status(),
        ("GET", ["ip"]) => public_ip(ctl)?,
        ("POST", ["mode"]) => serde_json::to_value(ctl.set_mode(serde_json::from_value::<Mode>(b.clone())?)?)?,
        ("POST", ["off"]) => serde_json::to_value(ctl.set_mode(Mode::Off)?)?,
        ("POST", ["quit"]) => {
            let ctl = ctl.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                ctl.shutdown();
                std::process::exit(0);
            });
            json!({ "ok": true })
        }

        // ---- Tor ----
        ("GET", ["tor", "countries"]) => serde_json::to_value(ctl.tor.exit_countries()?)?,

        // ---- VPN profiles ----
        ("GET", ["vpn", "profiles"]) => serde_json::to_value(ctl.profiles.list())?,
        ("POST", ["vpn", "profiles"]) => {
            ctl.profiles.save(field(b, "name").trim(), &field(b, "text"), field(b, "username").trim(), &field(b, "password"))?;
            serde_json::to_value(ctl.profiles.list())?
        }
        ("DELETE", ["vpn", "profiles", name]) => {
            ctl.profiles.delete(name)?;
            serde_json::to_value(ctl.profiles.list())?
        }
        ("POST", ["vpn", "profiles", name, "send-to-pi"]) => {
            ctl.send_profile_to_pi(name)?;
            json!({ "ok": true })
        }

        // ---- Cloud ----
        ("GET", ["cloud", pid]) => ctl.cloud.info(&ctl.state, pid)?,
        ("POST", ["cloud", pid, "login"]) => ctl.cloud.start_login(ctl.state.clone(), pid)?,
        ("POST", ["cloud", pid, "logout"]) => {
            ctl.cloud.sign_out(&ctl.state, pid)?;
            json!({ "ok": true })
        }
        ("POST", ["cloud", pid, "deploy"]) => {
            let (pid, country, subscription) = (pid.to_string(), field(b, "country"), field(b, "subscription"));
            let me = ctl.clone();
            let title = format!("Creating your server ({})", country.to_uppercase());
            serde_json::to_value(ctl.jobs.start(&title, move |job| {
                me.cloud.deploy(&me.state, &pid, &country, &subscription, &|m| job.log(m))?;
                me.activate(job, Mode::Cloud { provider: pid })
            })?)?
        }
        ("POST", ["cloud", pid, "send-to-pi"]) => json!({ "name": ctl.send_cloud_to_pi(pid, true)? }),
        ("POST", ["cloud", pid, "destroy"]) => {
            let pid = pid.to_string();
            let me = ctl.clone();
            serde_json::to_value(ctl.jobs.start("Deleting your server", move |job| {
                if matches!(me.mode(), Mode::Cloud { provider } if provider == pid) {
                    me.activate(job, Mode::Off)?;
                }
                me.cloud.destroy(&me.state, &pid, &|m| job.log(m))?;
                job.log("If the Pi was using this server, switch it to another profile.");
                Ok(())
            })?)?
        }

        // ---- Updates ----
        ("POST", ["update", "check"]) => {
            let me = ctl.clone();
            std::thread::spawn(move || {
                if me.updater.check_and_download(&me).unwrap_or(false) {
                    me.updater.install_if_idle(&me);
                }
            });
            serde_json::to_value(ctl.updater.status(ctl))?
        }
        ("POST", ["update", "install"]) => {
            ctl.updater.install(ctl)?;
            serde_json::to_value(ctl.updater.status(ctl))?
        }
        ("POST", ["update", "auto"]) => {
            ctl.state.set("auto_update", &b["enabled"].as_bool().unwrap_or(true));
            serde_json::to_value(ctl.updater.status(ctl))?
        }

        // ---- Raspberry Pi ----
        ("GET", ["pi"]) => pi_info(ctl),
        ("POST", ["pi"]) => {
            pilink::configure(&ctl.state, &field(b, "url"), b["password"].as_str().map(String::from))?;
            pi_info(ctl)
        }
        _ => bail!("unknown command {method} {path}"),
    })
}

/// Public IP and approximate location as the internet sees this computer right now
/// (through the tunnel when connected). Used for the map.
fn public_ip(ctl: &Ctl) -> Result<Value> {
    let mut agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(12));
    if matches!(ctl.mode(), Mode::Tor { system: false, .. }) {
        agent = agent.proxy(ureq::Proxy::new(format!("socks5://127.0.0.1:{}", crate::tor::SOCKS_PORT))?);
    }
    agent
        .build()
        .get("https://ipinfo.io/json")
        .set("Accept", "application/json")
        .call()
        .map_err(|e| anyhow::anyhow!("can't look up the public IP: {e}"))?
        .into_json()
        .map_err(Into::into)
}

fn pi_info(ctl: &Ctl) -> Value {
    let mut info = pilink::public(&ctl.state);
    if info["url"].is_string() {
        match pilink::status(&ctl.state) {
            Ok(s) => info["status"] = s,
            Err(e) => info["error"] = json!(format!("{e:#}")),
        }
    }
    info
}
