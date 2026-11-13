//! Talks to the Geo Unlocker Pi app's HTTP API (send profiles, connect).

use anyhow::{bail, Result};
use base64::Engine;
use geo_common::state::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct PiSettings {
    pub url: String,
    pub password: String,
}

pub fn settings(state: &State) -> Option<PiSettings> {
    state.get::<PiSettings>("pi").filter(|s| !s.url.is_empty())
}

pub fn public(state: &State) -> Value {
    match settings(state) {
        Some(s) => json!({ "url": s.url, "has_password": !s.password.is_empty() }),
        None => json!({ "url": null, "has_password": false }),
    }
}

fn normalize(url: &str) -> String {
    let mut u = url.trim().trim_end_matches('/').to_string();
    if !u.contains("://") {
        u = format!("http://{u}");
    }
    let host_part = u.split_once("://").map(|x| x.1).unwrap_or(&u);
    if !host_part.contains(':') {
        u.push_str(":8088"); // the Pi app's default port
    }
    u
}

fn request(s: &PiSettings, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let mut req = agent.request(method, &format!("{}{path}", s.url)).set("Accept", "application/json");
    if !s.password.is_empty() {
        let token = base64::engine::general_purpose::STANDARD.encode(format!("geo:{}", s.password));
        req = req.set("Authorization", &format!("Basic {token}"));
    }
    let resp = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match resp {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(401, _)) => bail!("The Pi rejected the password"),
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_json::<Value>().ok().and_then(|v| v["error"].as_str().map(String::from));
            bail!("Pi error: {}", msg.unwrap_or_else(|| format!("HTTP {code}")))
        }
        Err(e) => bail!("Can't reach the Pi at {}: {e}", s.url),
    }
}

fn need(state: &State) -> Result<PiSettings> {
    settings(state).ok_or_else(|| anyhow::anyhow!("Set up the Raspberry Pi connection first (Pi tab)"))
}

/// Save the Pi address/password after checking they work. `password: None` keeps the saved one.
pub fn configure(state: &State, url: &str, password: Option<String>) -> Result<()> {
    if url.trim().is_empty() {
        state.remove("pi");
        return Ok(());
    }
    let url = normalize(url);
    let old = settings(state).unwrap_or_default();
    let password = password.unwrap_or_else(|| if old.url == url { old.password } else { String::new() });
    let s = PiSettings { url, password };
    request(&s, "GET", "/api/status", None)?;
    state.set("pi", &s);
    Ok(())
}

pub fn status(state: &State) -> Result<Value> {
    request(&need(state)?, "GET", "/api/status", None)
}

pub fn send_profile(state: &State, name: &str, text: &str, user: &str, pw: &str) -> Result<()> {
    let body = json!({ "name": name, "text": text, "username": user, "password": pw });
    request(&need(state)?, "POST", "/api/vpn/profiles", Some(body)).map(|_| ())
}

pub fn connect(state: &State, name: &str) -> Result<()> {
    request(&need(state)?, "POST", "/api/vpn/connect", Some(json!({ "name": name }))).map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn urls() {
        assert_eq!(super::normalize("192.168.1.10"), "http://192.168.1.10:8088");
        assert_eq!(super::normalize("http://pi.lan:9000/"), "http://pi.lan:9000");
    }
}
