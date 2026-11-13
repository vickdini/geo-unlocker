//! Web UI + JSON API. The desktop app uses the same API to push profiles:
//!   POST /api/vpn/profiles  {"name", "text", "username"?, "password"?}

use crate::config::Device;
use crate::controller::Ctl;
use anyhow::{bail, Result};
use geo_common::web::{self, basic_password, body_json, ct_eq, field, json, respond, Resp};
use std::sync::Arc;
use tiny_http::{Method, Request};

const INDEX: &str = include_str!("../static/index.html");

pub fn serve(addr: &str, ctl: Arc<Ctl>) -> Result<()> {
    web::serve(addr, Arc::new(move |req: &mut Request| handle(&ctl, req)))
}

fn handle(ctl: &Arc<Ctl>, req: &mut Request) -> Resp {
    let password = ctl.cfg.read().unwrap().password.clone();
    if !password.is_empty() && !basic_password(req).is_some_and(|p| ct_eq(&p, &password)) {
        return web::text(401, "Login required")
            .with_header(web::header_kv("WWW-Authenticate", "Basic realm=\"geo-unlocker\""));
    }
    respond(route(ctl, req))
}

fn route(ctl: &Arc<Ctl>, req: &mut Request) -> Result<Resp> {
    let (segs, _) = web::split_url(req.url());
    let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
    let method = req.method().clone();
    Ok(match (method, segs.as_slice()) {
        (Method::Get, []) => web::html(INDEX),
        (Method::Get, ["api", "status"]) => json(200, &ctl.status()),
        (Method::Post, ["api", "off"]) => json(200, &ctl.off()?),
        (Method::Get, ["api", "vpn", "profiles"]) => json(200, &ctl.profiles.list()),
        (Method::Post, ["api", "vpn", "profiles"]) => {
            let b = body_json(req)?;
            ctl.profiles.save(field(&b, "name").trim(), &field(&b, "text"),
                              field(&b, "username").trim(), &field(&b, "password"))?;
            json(200, &ctl.profiles.list())
        }
        (Method::Delete, ["api", "vpn", "profiles", name]) => {
            if ctl.state.get::<String>("active").as_deref() == Some(*name) {
                bail!("Disconnect before deleting the active profile");
            }
            ctl.profiles.delete(name)?;
            json(200, &ctl.profiles.list())
        }
        (Method::Post, ["api", "vpn", "connect"]) => {
            let b = body_json(req)?;
            json(200, &ctl.connect(field(&b, "name"))?)
        }
        (Method::Post, ["api", "update", "check"]) => {
            let me = ctl.clone();
            std::thread::spawn(move || {
                let _ = me.updater.check_and_install(&me);
            });
            json(200, &ctl.updater.status())
        }
        (Method::Post, ["api", "update", "auto"]) => {
            let b = body_json(req)?;
            ctl.set_auto_update(b["enabled"].as_bool().unwrap_or(true))?;
            json(200, &ctl.status())
        }
        (Method::Get, ["api", "devices"]) => json(200, &ctl.cfg.read().unwrap().devices),
        (Method::Put, ["api", "devices"]) => {
            let devices: Vec<Device> = serde_json::from_value(body_json(req)?)?;
            json(200, &ctl.set_devices(devices)?)
        }
        _ => web::error(404, "Not found"),
    })
}
