//! macOS/Linux only: the privileged service exposes `api::dispatch` to the app
//! window (which runs as the user) on 127.0.0.1, protected by a random per-launch
//! bearer token. Windows doesn't use this: the window calls `dispatch` directly.

use crate::api;
use crate::controller::Ctl;
use anyhow::Result;
use geo_common::web::{self, body_json, ct_eq, header, json, Resp};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tiny_http::Request;

pub const PORT: u16 = 8089;

/// Seconds-since-epoch of the last authenticated request (the window polls every second).
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

fn handle(ctl: &Arc<Ctl>, token: &str, req: &mut Request) -> Resp {
    let bearer = header(req, "Authorization").and_then(|h| h.strip_prefix("Bearer ").map(String::from));
    if !bearer.is_some_and(|b| ct_eq(&b, token)) {
        return web::text(403, "Forbidden");
    }
    LAST_REQUEST.store(geo_common::now(), Ordering::Relaxed);
    let method = req.method().to_string().to_ascii_uppercase();
    let path = req.url().split('?').next().unwrap_or("").to_string();
    let path = path.strip_prefix("/api").unwrap_or(&path).to_string();
    let result = body_json(req).and_then(|b| api::dispatch(ctl, &method, &path, &b));
    match result {
        Ok(v) => json(200, &v),
        Err(e) => web::error(400, format!("{e:#}")),
    }
}
