//! Minimal helpers on top of tiny_http (one thread per request).

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use std::io::{Cursor, Read};
use std::sync::Arc;
use tiny_http::{Header, Request, Response, Server};

pub type Resp = Response<Cursor<Vec<u8>>>;
pub type Handler = Arc<dyn Fn(&mut Request) -> Resp + Send + Sync>;

pub fn header_kv(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("valid header")
}

pub fn json<T: Serialize>(code: u16, v: &T) -> Resp {
    Response::from_data(serde_json::to_vec(v).unwrap_or_default())
        .with_status_code(code)
        .with_header(header_kv("Content-Type", "application/json"))
        .with_header(header_kv("Cache-Control", "no-store"))
}

pub fn error(code: u16, msg: impl std::fmt::Display) -> Resp {
    json(code, &serde_json::json!({ "error": msg.to_string() }))
}

pub fn html(body: &str) -> Resp {
    Response::from_data(body.as_bytes().to_vec())
        .with_header(header_kv("Content-Type", "text/html; charset=utf-8"))
        .with_header(header_kv("Cache-Control", "no-store"))
}

/// Static bytes with a content type (fonts, scripts).
pub fn asset(data: &[u8], content_type: &str) -> Resp {
    Response::from_data(data.to_vec())
        .with_header(header_kv("Content-Type", content_type))
        .with_header(header_kv("Cache-Control", "max-age=86400"))
}

pub fn text(code: u16, msg: &str) -> Resp {
    Response::from_data(msg.as_bytes().to_vec())
        .with_status_code(code)
        .with_header(header_kv("Content-Type", "text/plain; charset=utf-8"))
}

/// Turn a handler result into a response (errors -> 400 JSON).
pub fn respond(r: Result<Resp>) -> Resp {
    r.unwrap_or_else(|e| error(400, format!("{e:#}")))
}

pub fn header(req: &Request, name: &str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

pub fn body(req: &mut Request, limit: usize) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    req.as_reader().take(limit as u64 + 1).read_to_end(&mut buf)?;
    if buf.len() > limit {
        bail!("Request too large");
    }
    Ok(buf)
}

pub fn body_json(req: &mut Request) -> Result<Value> {
    let b = body(req, 256 * 1024)?;
    if b.iter().all(|c| c.is_ascii_whitespace()) {
        return Ok(Value::Object(Default::default()));
    }
    Ok(serde_json::from_slice(&b)?)
}

/// Get a string field from a JSON object ("" if missing).
pub fn field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// "/api/x/y?z=1" -> (["api","x","y"], "z=1")
pub fn split_url(url: &str) -> (Vec<String>, String) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let segs = path.split('/').filter(|s| !s.is_empty()).map(percent_decode).collect();
    (segs, query.to_string())
}

pub fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then(|| percent_decode(v))
    })
}

/// Password from an `Authorization: Basic ...` header.
pub fn basic_password(req: &Request) -> Option<String> {
    let h = header(req, "Authorization")?;
    let enc = h.strip_prefix("Basic ")?;
    let dec = base64::engine::general_purpose::STANDARD.decode(enc.trim()).ok()?;
    let s = String::from_utf8(dec).ok()?;
    Some(s.split_once(':')?.1.to_string())
}

/// Constant-time string comparison.
pub fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bind the listening socket (fails if the port is taken).
pub fn bind(addr: &str) -> Result<Server> {
    Server::http(addr).map_err(|e| anyhow!("can't listen on {addr}: {e}"))
}

/// Serve forever, one thread per request.
pub fn serve(addr: &str, handler: Handler) -> Result<()> {
    run(bind(addr)?, handler);
    Ok(())
}

/// Handle requests on an already-bound server until it closes.
pub fn run(server: Server, handler: Handler) {
    for mut req in server.incoming_requests() {
        let h = handler.clone();
        std::thread::spawn(move || {
            let resp = h(&mut req);
            let _ = req.respond(resp);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls() {
        let (s, q) = split_url("/api/vpn/profiles/US%20East/send-to-pi?t=abc");
        assert_eq!(s, ["api", "vpn", "profiles", "US East", "send-to-pi"]);
        assert_eq!(query_param(&q, "t").as_deref(), Some("abc"));
        assert_eq!(percent_decode("a%2"), "a%2");
        assert!(ct_eq("abc", "abc") && !ct_eq("abc", "abd") && !ct_eq("a", "ab"));
    }
}
