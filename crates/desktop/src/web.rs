//! Local web UI + JSON API on 127.0.0.1, protected by a per-launch token cookie.

use crate::controller::{Ctl, Mode};
use crate::pilink;
use anyhow::Result;
use geo_common::web::{self, body_json, ct_eq, field, header, json, respond, Resp};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tiny_http::{Method, Request};

const INDEX: &str = include_str!("../static/index.html");
const MAP_JS: &str = include_str!("../static/map.js");
const FLAGS_FONT: &[u8] = include_bytes!("../static/flags.woff2");
pub const PORT: u16 = 8089;
const COOKIE: &str = "gu_token";

/// Seconds-since-epoch of the last authenticated request (the app window polls every second).
static LAST_REQUEST: AtomicU64 = AtomicU64::new(0);

pub fn last_request() -> u64 {
    LAST_REQUEST.load(Ordering::Relaxed)
}

/// Bind 127.0.0.1:PORT (fails if Geo Unlocker is already running) and serve in the background.
pub fn start(ctl: Arc<Ctl>, token: String) -> Result<()> {
    let server = web::bind(&format!("127.0.0.1:{PORT}"))
        .map_err(|_| anyhow::anyhow!("Geo Unlocker is already running (port {PORT} is in use)."))?;
    LAST_REQUEST.store(geo_common::now(), Ordering::Relaxed);
    std::thread::spawn(move || web::run(server, Arc::new(move |req: &mut Request| handle(&ctl, &token, req))));
    Ok(())
}

fn cookie(req: &Request, name: &str) -> Option<String> {
    header(req, "Cookie")?.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn handle(ctl: &Arc<Ctl>, token: &str, req: &mut Request) -> Resp {
    // Only our own origin (blocks DNS rebinding), and only with the launch token.
    let host = header(req, "Host").unwrap_or_default();
    if host != format!("127.0.0.1:{PORT}") && host != format!("localhost:{PORT}") {
        return web::text(403, "Forbidden");
    }
    let (segs, query) = web::split_url(req.url());
    if segs.is_empty() && web::query_param(&query, "t").is_some_and(|t| ct_eq(&t, token)) {
        return web::text(302, "")
            .with_header(web::header_kv("Location", "/"))
            .with_header(web::header_kv("Set-Cookie", &format!("{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/")));
    }
    let bearer = header(req, "Authorization").and_then(|h| h.strip_prefix("Bearer ").map(String::from));
    let authed = cookie(req, COOKIE).is_some_and(|c| ct_eq(&c, token)) || bearer.is_some_and(|b| ct_eq(&b, token));
    if !authed {
        return web::text(403, "Open Geo Unlocker from its app window.");
    }
    LAST_REQUEST.store(geo_common::now(), Ordering::Relaxed);
    respond(route(ctl, req, &segs))
}

fn route(ctl: &Arc<Ctl>, req: &mut Request, segs: &[String]) -> Result<Resp> {
    let s: Vec<&str> = segs.iter().map(String::as_str).collect();
    let method = req.method().clone();
    Ok(match (method, s.as_slice()) {
        (Method::Get, []) => web::html(INDEX),
        (Method::Get, ["map.js"]) => web::asset(MAP_JS.as_bytes(), "text/javascript; charset=utf-8"),
        (Method::Get, ["flags.woff2"]) => web::asset(FLAGS_FONT, "font/woff2"),
        (Method::Get, ["api", "status"]) => json(200, &ctl.status()),
        (Method::Get, ["api", "ip"]) => json(200, &public_ip(ctl)?),
        (Method::Post, ["api", "mode"]) => {
            let mode: Mode = serde_json::from_value(body_json(req)?)?;
            json(200, &ctl.set_mode(mode)?)
        }
        (Method::Post, ["api", "off"]) => json(200, &ctl.set_mode(Mode::Off)?),
        (Method::Post, ["api", "quit"]) => {
            let ctl = ctl.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                ctl.shutdown();
                std::process::exit(0);
            });
            json(200, &json!({ "ok": true }))
        }

        // ---- Tor ----
        (Method::Get, ["api", "tor", "countries"]) => json(200, &ctl.tor.exit_countries()?),
        (Method::Post, ["api", "tor", "connect"]) => {
            let b = body_json(req)?;
            json(200, &ctl.set_mode(Mode::Tor { country: field(&b, "country"), system: b["system"].as_bool().unwrap_or(true) })?)
        }

        // ---- VPN profiles ----
        (Method::Get, ["api", "vpn", "profiles"]) => json(200, &ctl.profiles.list()),
        (Method::Post, ["api", "vpn", "profiles"]) => {
            let b = body_json(req)?;
            ctl.profiles.save(field(&b, "name").trim(), &field(&b, "text"), field(&b, "username").trim(), &field(&b, "password"))?;
            json(200, &ctl.profiles.list())
        }
        (Method::Delete, ["api", "vpn", "profiles", name]) => {
            ctl.profiles.delete(name)?;
            json(200, &ctl.profiles.list())
        }
        (Method::Post, ["api", "vpn", "profiles", name, "send-to-pi"]) => {
            ctl.send_profile_to_pi(name)?;
            json(200, &json!({ "ok": true }))
        }
        (Method::Post, ["api", "vpn", "connect"]) => {
            let b = body_json(req)?;
            json(200, &ctl.set_mode(Mode::Vpn { name: field(&b, "name") })?)
        }

        // ---- Cloud ----
        (Method::Get, ["api", "cloud", pid]) => json(200, &ctl.cloud.info(&ctl.state, pid)?),
        (Method::Post, ["api", "cloud", pid, "login"]) => json(200, &ctl.cloud.start_login(ctl.state.clone(), pid)?),
        (Method::Post, ["api", "cloud", pid, "logout"]) => {
            ctl.cloud.sign_out(&ctl.state, pid)?;
            json(200, &json!({ "ok": true }))
        }
        (Method::Post, ["api", "cloud", pid, "deploy"]) => {
            let b = body_json(req)?;
            let (pid, country) = (pid.to_string(), field(&b, "country"));
            let subscription = field(&b, "subscription");
            let connect = b["connect"].as_bool().unwrap_or(true);
            let me = ctl.clone();
            let title = format!("Creating {pid} server ({})", country.to_uppercase());
            json(200, &ctl.jobs.start(&title, move |job| {
                me.cloud.deploy(&me.state, &pid, &country, &subscription, &|m| job.log(m))?;
                if connect {
                    me.activate(job, Mode::Cloud { provider: pid })?;
                }
                Ok(())
            })?)
        }
        (Method::Post, ["api", "cloud", pid, "connect"]) => {
            json(200, &ctl.set_mode(Mode::Cloud { provider: pid.to_string() })?)
        }
        (Method::Post, ["api", "cloud", pid, "send-to-pi"]) => {
            let b = body_json(req)?;
            let name = ctl.send_cloud_to_pi(pid, b["connect"].as_bool().unwrap_or(true))?;
            json(200, &json!({ "name": name }))
        }
        (Method::Post, ["api", "cloud", pid, "destroy"]) => {
            let pid = pid.to_string();
            let me = ctl.clone();
            json(200, &ctl.jobs.start(&format!("Deleting {pid} server"), move |job| {
                if matches!(me.mode(), Mode::Cloud { provider } if provider == pid) {
                    me.activate(job, Mode::Off)?;
                }
                me.cloud.destroy(&me.state, &pid, &|m| job.log(m))?;
                job.log("If the Pi was using this server, switch it to another profile.");
                Ok(())
            })?)
        }

        // ---- Updates ----
        (Method::Get, ["api", "update"]) => json(200, &ctl.updater.status(ctl)),
        (Method::Post, ["api", "update", "check"]) => {
            let me = ctl.clone();
            std::thread::spawn(move || {
                if me.updater.check_and_download(&me).unwrap_or(false) {
                    me.updater.install_if_idle(&me);
                }
            });
            json(200, &ctl.updater.status(ctl))
        }
        (Method::Post, ["api", "update", "install"]) => {
            ctl.updater.install(ctl)?;
            json(200, &ctl.updater.status(ctl))
        }
        (Method::Post, ["api", "update", "auto"]) => {
            let b = body_json(req)?;
            ctl.state.set("auto_update", &b["enabled"].as_bool().unwrap_or(true));
            json(200, &ctl.updater.status(ctl))
        }

        // ---- Pi ----
        (Method::Get, ["api", "pi"]) => json(200, &pi_info(ctl)),
        (Method::Post, ["api", "pi"]) => {
            let b = body_json(req)?;
            let password = b["password"].as_str().map(String::from);
            pilink::configure(&ctl.state, &field(&b, "url"), password)?;
            json(200, &pi_info(ctl))
        }
        _ => web::error(404, "Not found"),
    })
}

/// Public IP and approximate location as the internet sees this computer right now
/// (through the tunnel when connected). Used for the map.
fn public_ip(ctl: &Ctl) -> Result<serde_json::Value> {
    let mut agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(12));
    if matches!(ctl.mode(), Mode::Tor { system: false, .. }) {
        let proxy = ureq::Proxy::new(format!("socks5://127.0.0.1:{}", crate::tor::SOCKS_PORT))?;
        agent = agent.proxy(proxy);
    }
    let v: serde_json::Value = agent
        .build()
        .get("https://ipinfo.io/json")
        .set("Accept", "application/json")
        .call()
        .map_err(|e| anyhow::anyhow!("can't look up the public IP: {e}"))?
        .into_json()?;
    Ok(v)
}

fn pi_info(ctl: &Ctl) -> serde_json::Value {
    let mut info = pilink::public(&ctl.state);
    if info["url"].is_string() {
        match pilink::status(&ctl.state) {
            Ok(s) => info["status"] = s,
            Err(e) => info["error"] = json!(format!("{e:#}")),
        }
    }
    info
}
