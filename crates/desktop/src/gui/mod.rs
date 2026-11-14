//! Native desktop window (iced). No web engine: everything is drawn by the app.

mod flags;
mod icons;
mod map;
mod style;
mod view;

use crate::controller::Ctl;
use crate::{api, backend, service};
use iced::widget::canvas;
use iced::{window, Point, Size, Subscription, Task};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Canned backend answers: (method, path, body) -> JSON.
pub type MockFn = Arc<dyn Fn(&str, &str, &Value) -> Result<Value, String> + Send + Sync>;

/// How the window reaches the networking core.
#[derive(Clone)]
pub enum Backend {
    /// Same process (Windows, or when started as root).
    Local(Arc<Ctl>),
    /// The privileged service on macOS/Linux (127.0.0.1, bearer token).
    Remote { token: String },
    /// Canned answers (the `preview` example, for screenshots and UI work).
    Mock(MockFn),
}

impl Backend {
    fn call(&self, method: &str, path: &str, body: &Value) -> Result<Value, String> {
        match self {
            Backend::Local(ctl) => api::dispatch(ctl, method, path, body).map_err(|e| format!("{e:#}")),
            Backend::Mock(f) => f(method, path, body),
            Backend::Remote { token } => {
                let url = format!("http://127.0.0.1:{}/api{path}", service::PORT);
                let req = ureq::AgentBuilder::new()
                    .timeout(Duration::from_secs(120))
                    .build()
                    .request(method, &url)
                    .set("Authorization", &format!("Bearer {token}"));
                match req.send_json(body.clone()) {
                    Ok(r) => r.into_json().map_err(|e| e.to_string()),
                    Err(ureq::Error::Status(_, r)) => {
                        let v: Value = r.into_json().unwrap_or(Value::Null);
                        Err(v["error"].as_str().unwrap_or("request failed").to_string())
                    }
                    Err(e) => Err(format!("Geo Unlocker's background service isn't responding ({e})")),
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Tor,
    Vpn,
    Cloud,
}

/// Something that needs a "are you sure?" first.
#[derive(Clone, Debug)]
pub enum Pending {
    DeleteProfile(String),
    Deploy(String),
    Destroy,
    InstallUpdate,
    Quit,
}

#[derive(Clone, Debug)]
pub enum Overlay {
    Confirm { title: String, body: String, ok: String, danger: bool, action: Pending },
    AddProfile,
    Pi,
    Updates,
    Advanced,
}

#[derive(Clone, Debug, Default)]
pub struct Geo {
    pub ip: String,
    pub city: String,
    pub country: String,
    pub at: Option<Point>,
}

#[derive(Default, Clone)]
pub struct AddForm {
    pub file_name: String,
    pub text: String,
    pub name: String,
    pub user: String,
    pub pass: String,
}

/// Saved between runs (window-side preferences only).
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Prefs {
    tab: Option<Tab>,
    tor_system: Option<bool>,
    home_ip: Option<String>,
    home_city: Option<String>,
    home_country: Option<String>,
    home_lat: Option<f32>,
    home_lon: Option<f32>,
}

fn prefs_path() -> std::path::PathBuf {
    let env = |k: &str| std::env::var_os(k).map(std::path::PathBuf::from);
    #[cfg(windows)]
    let dir = env("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("GeoUnlocker");
    #[cfg(target_os = "macos")]
    let dir = env("HOME").unwrap_or_else(std::env::temp_dir).join("Library/Application Support/GeoUnlocker");
    #[cfg(target_os = "linux")]
    let dir = env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config"))).unwrap_or_else(std::env::temp_dir).join("geo-unlocker");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("window.json")
}

pub struct App {
    pub backend: Option<Backend>,
    pub waiting: Option<String>,
    pub fatal: Option<String>,
    pub status: Value,
    status_busy: bool,
    pub tab: Tab,
    pub search: String,
    pub tor_system: bool,
    pub tor_list: Option<Vec<(String, u64, u64)>>,
    pub profiles: Vec<(String, String, bool)>,
    pub cloud: Value,
    pub cloud_sub: Option<String>,
    pub pi_url: String,
    pub pi_pass: String,
    pub pi_info: Value,
    pub form: AddForm,
    pub overlay: Option<Overlay>,
    pub flash: Option<(String, bool, Instant)>,
    pub dismissed_job: Option<String>,
    pub home: Option<Geo>,
    pub exit: Option<Geo>,
    conn_key: String,
    pub view: map::View,
    view_target: Option<map::View>,
    pub map_size: Size,
    pub land: canvas::Cache,
    base: Option<(u64, u64, u64)>,
    prev: Option<(Instant, u64, u64)>,
    pub speeds: Option<(f32, f32)>,
    pub volume: Option<(u64, u64)>,
    pub samples: VecDeque<(f32, f32)>,
    restart_requested: bool,
    /// Whether a background job was running at the last status update.
    job_was_running: bool,
    /// Result of the last server create/delete, shown in the Cloud tab: (message, is_error).
    pub cloud_notice: Option<(String, bool)>,
    /// What was copied last and when (for the "Copied" confirmation).
    pub copied: Option<(String, Instant)>,
    /// Technical details (GET /advanced), refreshed while the panel is open.
    pub advanced: Value,
    ticks: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    ServiceReady(Result<(), String>),
    Tick,
    Status(Result<Value, String>),
    TorList(Result<Value, String>),
    Profiles(Result<Value, String>),
    Cloud(Result<Value, String>),
    PiInfo(Result<Value, String>),
    Ip(String, bool, Result<Value, String>),
    Done(Result<Value, String>),
    DoneNotify(String, Result<Value, String>),
    Tab(Tab),
    Search(String),
    TorScope(bool),
    MainButton,
    TorConnect(String),
    VpnConnect(String),
    VpnToPi(String),
    Ask(Pending),
    Confirmed,
    CloseOverlay,
    OpenAddProfile,
    PickFile,
    FilePicked(Option<(String, String)>),
    FormName(String),
    FormUser(String),
    FormPass(String),
    SaveProfile,
    CloudLogin,
    CloudLogout,
    CloudSub(String),
    CloudConnect,
    CloudToPi,
    OpenPi,
    OpenUpdates,
    OpenAdvanced,
    Advanced(Result<Value, String>),
    PiUrl(String),
    PiPass(String),
    PiSave,
    UpdCheck,
    UpdAuto(bool),
    OpenLink(String),
    /// Copy text to the clipboard (e.g. the Azure sign-in code).
    Copy(String),
    DismissToast,
    CloseRequested,
    Exit,
    MapView(map::View),
    MapZoom(f32),
    MapReset,
    MapClick(String),
    MapSize(Size),
    Frame,
}

pub fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Open a link in the user's normal browser.
pub fn open_external(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn(); // not elevated
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// What the connection looks like right now (derived from the status).
pub struct ConnView {
    pub state: &'static str, // off | wait | on
    pub title: String,
    pub proto: String,
    pub cc: String,
    pub traffic: Option<(u64, u64)>,
}

impl App {
    fn new(backend: Option<Backend>, waiting: Option<String>, fatal: Option<String>) -> Self {
        let prefs: Prefs = std::fs::read_to_string(prefs_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let home = prefs.home_ip.clone().map(|ip| Geo {
            ip,
            city: prefs.home_city.clone().unwrap_or_default(),
            country: prefs.home_country.clone().unwrap_or_default(),
            at: prefs.home_lat.zip(prefs.home_lon).map(|(a, o)| map::lat_lon(a, o)),
        });
        App {
            backend,
            waiting,
            fatal,
            status: Value::Null,
            status_busy: false,
            tab: prefs.tab.unwrap_or(Tab::Tor),
            search: String::new(),
            tor_system: prefs.tor_system.unwrap_or(true),
            tor_list: None,
            profiles: Vec::new(),
            cloud: Value::Null,
            cloud_sub: None,
            pi_url: String::new(),
            pi_pass: String::new(),
            pi_info: Value::Null,
            form: AddForm::default(),
            overlay: None,
            flash: None,
            dismissed_job: None,
            home,
            exit: None,
            conn_key: String::new(),
            view: map::View::default(),
            view_target: None,
            map_size: Size::new(800.0, 500.0),
            land: canvas::Cache::new(),
            base: None,
            prev: None,
            speeds: None,
            volume: None,
            samples: VecDeque::new(),
            restart_requested: false,
            job_was_running: false,
            cloud_notice: None,
            copied: None,
            advanced: Value::Null,
            ticks: 0,
        }
    }

    fn save_prefs(&self) {
        let h = self.home.clone().unwrap_or_default();
        let w = map::world();
        let (lat, lon) = h
            .at
            .map(|p| (w_lat(p, w), p.x / w.w * 360.0 - 180.0))
            .map(|(a, b)| (Some(a), Some(b)))
            .unwrap_or((None, None));
        let p = Prefs {
            tab: Some(self.tab),
            tor_system: Some(self.tor_system),
            home_ip: self.home.as_ref().map(|h| h.ip.clone()),
            home_city: Some(h.city),
            home_country: Some(h.country),
            home_lat: lat,
            home_lon: lon,
        };
        let _ = std::fs::write(prefs_path(), serde_json::to_string(&p).unwrap_or_default());
    }

    /// Run a backend command off the UI thread.
    fn call(&self, method: &'static str, path: impl Into<String>, body: Value, f: impl FnOnce(Result<Value, String>) -> Message + Send + 'static) -> Task<Message> {
        let Some(b) = self.backend.clone() else { return Task::none() };
        let path = path.into();
        Task::perform(
            async move { tokio::task::spawn_blocking(move || b.call(method, &path, &body)).await.unwrap_or_else(|e| Err(e.to_string())) },
            f,
        )
    }

    fn notify(&mut self, msg: impl Into<String>, error: bool) {
        self.flash = Some((msg.into(), error, Instant::now()));
    }

    pub fn conn(&self) -> ConnView {
        let st = &self.status;
        let m = &st["mode"];
        let kind = m["kind"].as_str().unwrap_or("off");
        let job = &st["job"];
        let switching = job["running"].as_bool() == Some(true) && job["title"].as_str() != Some("Disconnecting");
        let mut v = ConnView { state: "off", title: String::new(), proto: String::new(), cc: String::new(), traffic: None };
        let mut up = false;
        match kind {
            "tor" => {
                let t = &st["tor"];
                let system = m["system"].as_bool() != Some(false);
                up = t["running"].as_bool() == Some(true)
                    && t["bootstrap"].as_u64().unwrap_or(0) >= 100
                    && (!system || st["tor_tunnel"].as_bool() == Some(true));
                v.proto = if system { "Tor · whole computer".into() } else { "Tor · proxy apps".into() };
                v.cc = s(m, "country");
                if v.cc.is_empty() {
                    v.cc = self.exit.as_ref().map(|e| e.country.clone()).unwrap_or_default();
                }
                v.title = format!("{} » Tor", icons::country_name(&s(m, "country")));
                if system {
                    v.traffic = Some((st["tor_traffic"]["rx"].as_u64().unwrap_or(0), st["tor_traffic"]["tx"].as_u64().unwrap_or(0)));
                }
            }
            "vpn" | "cloud" => {
                let vp = &st["vpn"];
                up = vp["kind"].as_str() == Some("wireguard") && vp["handshake_age"].as_u64().is_some_and(|a| a < 180);
                v.proto = "WireGuard".into();
                if vp["kind"].as_str() == Some("wireguard") {
                    v.traffic = Some((vp["rx"].as_u64().unwrap_or(0), vp["tx"].as_u64().unwrap_or(0)));
                }
                if kind == "cloud" {
                    v.cc = s(&st["cloud"][s(m, "provider")], "country");
                    v.title = format!("{} » Azure", icons::country_name(&v.cc));
                } else {
                    v.cc = self.exit.as_ref().map(|e| e.country.clone()).unwrap_or_default();
                    v.title = s(m, "name");
                }
            }
            _ => {}
        }
        v.state = if kind == "off" {
            if switching { "wait" } else { "off" }
        } else if up {
            "on"
        } else {
            "wait"
        };
        v
    }

    fn markers(&self) -> Vec<String> {
        match self.tab {
            Tab::Tor => self.tor_list.as_ref().map(|l| l.iter().map(|c| c.0.clone()).collect()).unwrap_or_default(),
            Tab::Cloud if self.cloud["deployment"].is_null() && self.cloud["account"].is_string() => {
                self.cloud["regions"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn focus(&mut self, points: Vec<Point>) {
        self.view_target = Some(map::View::fit(&points, self.map_size));
    }

    fn after_status(&mut self) -> Task<Message> {
        let v = self.conn();
        let mut tasks = Vec::new();
        // Look up where the internet sees us once the connection settles.
        let key = format!("{}|{}", self.status["mode"], v.state);
        if key != self.conn_key {
            let was_on = self.conn_key.ends_with("|on");
            self.conn_key = key.clone();
            if v.state == "off" {
                self.exit = None;
                if was_on {
                    self.view_target = Some(map::View::default());
                }
            }
            if v.state != "wait" {
                let off = v.state == "off";
                let delay = if off { 300 } else { 1500 };
                if let Some(b) = self.backend.clone() {
                    tasks.push(Task::perform(
                        async move {
                            tokio::time::sleep(Duration::from_millis(delay)).await;
                            tokio::task::spawn_blocking(move || b.call("GET", "/ip", &Value::Null)).await.unwrap_or_else(|e| Err(e.to_string()))
                        },
                        move |r| Message::Ip(key.clone(), off, r),
                    ));
                }
            }
        }
        // When a job (create/delete server, connect) finishes, refresh the lists so the
        // Cloud tab doesn't keep showing a server that was just deleted.
        let job_running = self.status["job"]["running"].as_bool() == Some(true);
        if self.job_was_running && !job_running {
            let job = &self.status["job"];
            let title = s(job, "title");
            let country = icons::country_name(&s(&self.cloud["deployment"], "country"));
            let error = job["error"].as_str().map(String::from);
            if title.starts_with("Deleting your server") {
                self.cloud_notice = Some(match error {
                    None => (format!("✔ Your server in {country} was deleted. You're no longer being charged for it."), false),
                    Some(e) => (format!("Deleting the server failed: {e}"), true),
                });
            } else if title.starts_with("Creating your server") {
                self.cloud_notice = error.map(|e| (format!("Creating the server failed: {e}"), true));
            }
            tasks.push(self.refresh_lists());
        }
        self.job_was_running = job_running;
        // Traffic statistics.
        let now = Instant::now();
        let since = self.status["connected_at"].as_u64().unwrap_or(0);
        if since == 0 || v.state == "off" {
            self.base = None;
            self.prev = None;
            self.samples.clear();
        }
        let t = if v.state == "on" { v.traffic } else { None };
        self.speeds = None;
        self.volume = None;
        if let Some((rx, tx)) = t {
            if self.base.is_none_or(|b| b.0 != since || rx < b.1) {
                self.base = Some((since, rx, tx));
            }
            let (_, brx, btx) = self.base.unwrap();
            self.volume = Some((rx - brx, tx.saturating_sub(btx)));
            let mut sp = (0.0, 0.0);
            if let Some((pt, prx, ptx)) = self.prev {
                let dt = now.duration_since(pt).as_secs_f32().max(0.2);
                sp = (rx.saturating_sub(prx) as f32 / dt, tx.saturating_sub(ptx) as f32 / dt);
            }
            self.prev = Some((now, rx, tx));
            self.speeds = Some(sp);
            self.samples.push_back(sp);
            while self.samples.len() > 60 {
                self.samples.pop_front();
            }
        }
        // Finish an update on macOS/Linux by restarting (Windows' installer does it itself).
        if self.status["update"]["state"].as_str() == Some("restart") && !self.restart_requested {
            self.restart_requested = true;
            if cfg!(unix) {
                relaunch();
                tasks.push(self.shutdown_then_exit());
            }
        }
        Task::batch(tasks)
    }

    fn shutdown_then_exit(&self) -> Task<Message> {
        match self.backend.clone() {
            Some(Backend::Local(ctl)) => Task::perform(
                async move {
                    let _ = tokio::task::spawn_blocking(move || ctl.shutdown()).await;
                },
                |_| Message::Exit,
            ),
            Some(b) => Task::perform(
                async move {
                    let _ = tokio::task::spawn_blocking(move || b.call("POST", "/quit", &json!({}))).await;
                },
                |_| Message::Exit,
            ),
            None => iced::exit(),
        }
    }

    fn refresh_lists(&self) -> Task<Message> {
        Task::batch([
            self.call("GET", "/tor/countries", Value::Null, Message::TorList),
            self.call("GET", "/vpn/profiles", Value::Null, Message::Profiles),
            self.call("GET", "/cloud/azure", Value::Null, Message::Cloud),
        ])
    }
}

fn w_lat(p: Point, w: &map::World) -> f32 {
    // inverse of map::lat_lon for the y axis
    let (lat0, lat1) = (84.0f32, -58.0f32);
    lat0 - p.y / w.h * (lat0 - lat1)
}

pub fn update(app: &mut App, message: Message) -> Task<Message> {
    match message {
        Message::ServiceReady(Ok(())) => {
            app.waiting = None;
            return app.refresh_lists();
        }
        Message::ServiceReady(Err(e)) => {
            app.waiting = None;
            app.fatal = Some(e);
        }
        Message::Tick => {
            app.ticks += 1;
            let mut tasks = Vec::new();
            if matches!(app.overlay, Some(Overlay::Advanced)) && app.ticks.is_multiple_of(2) {
                tasks.push(app.call("GET", "/advanced", Value::Null, Message::Advanced));
            }
            if let Some((_, _, t)) = &app.flash {
                if t.elapsed() > Duration::from_secs(6) {
                    app.flash = None;
                }
            }
            if !app.status_busy && app.backend.is_some() && app.waiting.is_none() {
                app.status_busy = true;
                tasks.push(app.call("GET", "/status", Value::Null, Message::Status));
            }
            return Task::batch(tasks);
        }
        Message::Status(r) => {
            app.status_busy = false;
            match r {
                Ok(v) => {
                    app.status = v;
                    return app.after_status();
                }
                Err(e) => app.notify(e, true),
            }
        }
        Message::TorList(Ok(v)) => {
            let mut list: Vec<(String, u64, u64)> = v
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| (s(c, "code"), c["relays"].as_u64().unwrap_or(0), c["mbps"].as_u64().unwrap_or(0)))
                .collect();
            list.sort_by_key(|c| icons::country_name(&c.0));
            app.tor_list = Some(list);
        }
        Message::TorList(Err(_)) => {
            let mut list: Vec<(String, u64, u64)> =
                ["us", "de", "nl", "fr", "gb", "ca", "ch", "se", "fi", "at"].iter().map(|c| (c.to_string(), 0, 0)).collect();
            list.sort_by_key(|c| icons::country_name(&c.0));
            app.tor_list = Some(list);
        }
        Message::Profiles(Ok(v)) => {
            app.profiles = v
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| (s(p, "name"), s(p, "type"), p["auth"].as_bool().unwrap_or(false)))
                .collect();
        }
        Message::Cloud(Ok(v)) => {
            if app.cloud_sub.is_none() {
                app.cloud_sub = v["options"]["subscriptions"][0]["id"].as_str().map(String::from);
            }
            let login_pending = v["login"].is_object() && v["login"]["done"].as_bool() != Some(true) && v["account"].is_null();
            app.cloud = v;
            if login_pending {
                // Poll until the device-code sign-in finishes.
                return Task::perform(tokio::time::sleep(Duration::from_secs(3)), |_| Message::Tab(Tab::Cloud));
            }
        }
        Message::Profiles(Err(e)) | Message::Cloud(Err(e)) | Message::Done(Err(e)) | Message::DoneNotify(_, Err(e)) => app.notify(e, true),
        Message::PiInfo(r) => {
            app.pi_info = r.unwrap_or_else(|e| json!({ "error": e }));
            if let Some(u) = app.pi_info["url"].as_str() {
                if app.pi_url.is_empty() {
                    app.pi_url = u.trim_start_matches("http://").to_string();
                }
            }
        }
        Message::Ip(key, off, Ok(v)) => {
            if key == app.conn_key {
                let at = s(&v, "loc").split_once(',').and_then(|(a, o)| Some(map::lat_lon(a.parse().ok()?, o.parse().ok()?)));
                let geo = Geo { ip: s(&v, "ip"), city: s(&v, "city"), country: s(&v, "country").to_lowercase(), at };
                if off {
                    app.home = Some(geo);
                    app.save_prefs();
                } else {
                    app.exit = Some(geo);
                    let pts: Vec<Point> = [app.home.as_ref().and_then(|h| h.at), app.exit.as_ref().and_then(|e| e.at)].into_iter().flatten().collect();
                    app.focus(pts);
                }
            }
        }
        Message::Ip(..) => {}
        Message::Done(Ok(_)) => return app.refresh_lists(),
        Message::DoneNotify(msg, Ok(_)) => {
            app.notify(msg, false);
            return app.refresh_lists();
        }
        Message::Tab(t) => {
            app.tab = t;
            app.save_prefs();
            if t == Tab::Cloud {
                return app.call("GET", "/cloud/azure", Value::Null, Message::Cloud);
            }
        }
        Message::Search(q) => app.search = q,
        Message::TorScope(system) => {
            app.tor_system = system;
            app.save_prefs();
        }
        Message::MainButton => {
            if app.conn().state != "off" {
                return app.call("POST", "/off", json!({}), Message::Done);
            }
            let mode = if app.status["last_mode"].is_object() {
                app.status["last_mode"].clone()
            } else {
                json!({ "kind": "tor", "country": "", "system": true })
            };
            return app.call("POST", "/mode", mode, Message::Done);
        }
        Message::TorConnect(cc) | Message::MapClick(cc) if app.tab == Tab::Tor || cc.is_empty() => {
            let mode = json!({ "kind": "tor", "country": cc, "system": app.tor_system });
            return app.call("POST", "/mode", mode, Message::Done);
        }
        Message::TorConnect(_) => {}
        Message::MapClick(cc) => {
            if app.tab == Tab::Cloud && app.cloud["regions"][&cc].is_string() {
                return update(app, Message::Ask(Pending::Deploy(cc)));
            }
        }
        Message::VpnConnect(name) => return app.call("POST", "/mode", json!({ "kind": "vpn", "name": name }), Message::Done),
        Message::VpnToPi(name) => {
            let msg = format!("\"{name}\" is now on the Raspberry Pi.");
            return app.call("POST", format!("/vpn/profiles/{}/send-to-pi", encode(&name)), json!({}), move |r| Message::DoneNotify(msg, r));
        }
        Message::Ask(action) => {
            let (title, body, ok, danger) = match &action {
                Pending::DeleteProfile(n) => (format!("Delete \"{n}\"?"), "The profile is removed from this computer.".to_string(), "Delete", true),
                Pending::Deploy(cc) => (
                    format!("Create a server in {}?", icons::country_name(cc)),
                    format!(
                        "Geo Unlocker creates a small VM in Azure ({}) running WireGuard and connects this computer to it.\n\n{}",
                        s(&app.cloud["regions"], cc),
                        s(&app.cloud, "cost_note")
                    ),
                    "Create & connect",
                    false,
                ),
                Pending::Destroy => ("Delete your Azure server?".into(), "This stops the charges. If the Raspberry Pi uses this server, it loses it too.".into(), "Delete", true),
                Pending::InstallUpdate => ("Install the update now?".into(), "This disconnects you, installs the new version and restarts Geo Unlocker.".into(), "Install & restart", false),
                Pending::Quit => ("Quit Geo Unlocker?".into(), "This disconnects and restores your normal connection.".into(), "Quit", false),
            };
            app.overlay = Some(Overlay::Confirm { title, body, ok: ok.into(), danger, action });
        }
        Message::Confirmed => {
            let Some(Overlay::Confirm { action, .. }) = app.overlay.take() else { return Task::none() };
            return match action {
                Pending::DeleteProfile(n) => app.call("DELETE", format!("/vpn/profiles/{}", encode(&n)), Value::Null, Message::Done),
                Pending::Deploy(cc) => {
                    app.cloud_notice = None;
                    app.call("POST", "/cloud/azure/deploy", json!({ "country": cc, "subscription": app.cloud_sub }), Message::Done)
                }
                Pending::Destroy => {
                    app.cloud_notice = None;
                    app.call("POST", "/cloud/azure/destroy", json!({}), Message::Done)
                }
                Pending::InstallUpdate => app.call("POST", "/update/install", json!({}), Message::Done),
                Pending::Quit => app.shutdown_then_exit(),
            };
        }
        Message::CloseOverlay => app.overlay = None,
        Message::OpenAddProfile => {
            app.form = AddForm::default();
            app.overlay = Some(Overlay::AddProfile);
        }
        Message::PickFile => {
            return Task::perform(
                async {
                    let f = rfd::AsyncFileDialog::new()
                        .set_title("Choose a WireGuard or OpenVPN profile")
                        .add_filter("VPN profile", &["conf", "ovpn"])
                        .pick_file()
                        .await?;
                    let bytes = f.read().await;
                    Some((f.file_name(), String::from_utf8_lossy(&bytes).into_owned()))
                },
                Message::FilePicked,
            );
        }
        Message::FilePicked(Some((name, text))) => {
            if app.form.name.is_empty() {
                app.form.name = name.rsplit_once('.').map(|x| x.0).unwrap_or(&name).to_string();
            }
            app.form.file_name = name;
            app.form.text = text;
        }
        Message::FilePicked(None) => {}
        Message::FormName(v) => app.form.name = v,
        Message::FormUser(v) => app.form.user = v,
        Message::FormPass(v) => app.form.pass = v,
        Message::SaveProfile => {
            if app.form.text.is_empty() {
                app.notify("Choose a .conf or .ovpn file first", true);
                return Task::none();
            }
            let f = app.form.clone();
            app.overlay = None;
            return app.call("POST", "/vpn/profiles", json!({ "name": f.name.trim(), "text": f.text, "username": f.user, "password": f.pass }), Message::Done);
        }
        Message::CloudLogin => return app.call("POST", "/cloud/azure/login", json!({}), |_| Message::Tab(Tab::Cloud)),
        Message::CloudLogout => return app.call("POST", "/cloud/azure/logout", json!({}), |_| Message::Tab(Tab::Cloud)),
        Message::CloudSub(name) => {
            app.cloud_sub = app.cloud["options"]["subscriptions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| s(x, "name") == name)
                .map(|x| s(x, "id"));
        }
        Message::CloudConnect => return app.call("POST", "/mode", json!({ "kind": "cloud", "provider": "azure" }), Message::Done),
        Message::CloudToPi => {
            return app.call("POST", "/cloud/azure/send-to-pi", json!({}), |r| {
                Message::DoneNotify("The Raspberry Pi now routes your devices through your Azure server.".into(), r)
            })
        }
        Message::OpenPi => {
            app.overlay = Some(Overlay::Pi);
            return app.call("GET", "/pi", Value::Null, Message::PiInfo);
        }
        Message::OpenUpdates => app.overlay = Some(Overlay::Updates),
        Message::OpenAdvanced => {
            app.overlay = Some(Overlay::Advanced);
            return app.call("GET", "/advanced", Value::Null, Message::Advanced);
        }
        Message::Advanced(r) => app.advanced = r.unwrap_or_else(|e| json!({ "error": e })),
        Message::PiUrl(v) => app.pi_url = v,
        Message::PiPass(v) => app.pi_pass = v,
        Message::PiSave => {
            let pw = if app.pi_pass.is_empty() { Value::Null } else { json!(app.pi_pass) };
            app.pi_pass.clear();
            app.pi_info = json!({ "checking": true });
            return app.call("POST", "/pi", json!({ "url": app.pi_url, "password": pw }), Message::PiInfo);
        }
        Message::UpdCheck => return app.call("POST", "/update/check", json!({}), |_| Message::Tick),
        Message::UpdAuto(on) => return app.call("POST", "/update/auto", json!({ "enabled": on }), |_| Message::Tick),
        Message::OpenLink(url) => open_external(&url),
        Message::Copy(text) => {
            app.copied = Some((text.clone(), Instant::now()));
            return iced::clipboard::write(text);
        }
        Message::DismissToast => {
            app.flash = None;
            app.dismissed_job = Some(format!("{}|{}", app.status["job"]["title"], app.status["job"]["error"]));
        }
        Message::CloseRequested => return app.shutdown_then_exit(),
        Message::Exit => return iced::exit(),
        Message::MapView(v) => {
            app.view = v;
            app.view_target = None;
            app.land.clear();
        }
        Message::MapZoom(f) => {
            app.view = map::View { zoom: app.view.zoom * f, ..app.view }.clamped(app.map_size);
            app.view_target = None;
            app.land.clear();
        }
        Message::MapReset => app.view_target = Some(map::View::default()),
        Message::MapSize(sz) => {
            if sz != app.map_size {
                app.map_size = sz;
                app.view = app.view.clamped(sz);
                app.land.clear();
            }
        }
        Message::Frame => {
            if let Some(t) = app.view_target {
                let next = app.view.lerp(t, 0.18);
                let done = (next.cx - t.cx).abs() < 0.5 && (next.cy - t.cy).abs() < 0.5 && (next.zoom - t.zoom).abs() < 0.01;
                app.view = if done { t } else { next };
                if done {
                    app.view_target = None;
                }
                app.land.clear();
            }
        }
    }
    Task::none()
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn subscription(app: &App) -> Subscription<Message> {
    let mut subs = vec![
        iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick),
        window::close_requests().map(|_| Message::CloseRequested),
    ];
    if app.view_target.is_some() {
        subs.push(window::frames().map(|_| Message::Frame));
    }
    Subscription::batch(subs)
}

/// App icon: purple disc with a white globe (also embedded in the .exe).
fn icon() -> Option<window::Icon> {
    const N: u32 = 64;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = ((x as f32 - c) / c, (y as f32 - c) / c);
            let d = (dx * dx + dy * dy).sqrt();
            let w = 0.07;
            let white = d <= 0.98
                && ((d - 0.6).abs() < w
                    || ((dx.abs() < w || dy.abs() < w) && d < 0.6 + w)
                    || (((dx / 0.28).powi(2) + (dy / 0.6).powi(2)).sqrt() - 1.0).abs() < w * 2.2 && d < 0.62);
            let px = if d > 0.98 {
                [0, 0, 0, 0]
            } else if white {
                [255, 255, 255, 255]
            } else {
                let t = y as f32 / N as f32;
                [(155.0 - 50.0 * t) as u8, (133.0 - 70.0 * t) as u8, 255, 255]
            };
            rgba.extend_from_slice(&px);
        }
    }
    window::icon::from_rgba(rgba, N, N).ok()
}

/// Start the (updated) app again a moment after we exit (macOS/Linux).
fn relaunch() {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = std::env::current_exe().ok().and_then(|e| e.ancestors().nth(3).map(|p| p.to_path_buf())) {
        let _ = std::process::Command::new("sh").arg("-c").arg("sleep 2; open -n \"$0\"").arg(&bundle).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let exe = if std::path::Path::new("/usr/bin/geo-unlocker").exists() {
            "/usr/bin/geo-unlocker".to_string()
        } else {
            std::env::current_exe().map(|p| p.display().to_string().trim_end_matches(" (deleted)").to_string()).unwrap_or_default()
        };
        let _ = std::process::Command::new("sh").arg("-c").arg("sleep 2; exec \"$0\"").arg(exe).spawn();
    }
}

fn is_root() -> bool {
    #[cfg(unix)]
    return unsafe { libc::geteuid() } == 0;
    #[cfg(not(unix))]
    return false;
}

/// macOS/Linux: start the privileged service (system password prompt) and wait for it.
#[cfg(unix)]
fn launch_service(token: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let run = || -> anyhow::Result<()> {
        let exe = std::env::current_exe()?;
        let token_file = std::env::temp_dir().join(format!("geo-unlocker-{}.token", std::process::id()));
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&token_file)?;
        std::fs::write(&token_file, token)?;
        #[cfg(target_os = "macos")]
        {
            let q = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
            let shell = format!("{} --service --token-file {} >/dev/null 2>&1 &", q(&exe.to_string_lossy()), q(&token_file.to_string_lossy()));
            let script = format!("do shell script \"{}\" with administrator privileges", shell.replace('\\', "\\\\").replace('"', "\\\""));
            if !std::process::Command::new("osascript").args(["-e", &script]).status()?.success() {
                anyhow::bail!("Administrator permission was not granted.");
            }
        }
        #[cfg(target_os = "linux")]
        {
            if geo_common::sh::which("pkexec").is_none() {
                anyhow::bail!("pkexec is not installed. Start Geo Unlocker with: sudo {}", exe.display());
            }
            std::process::Command::new("pkexec").arg(&exe).args(["--service", "--token-file"]).arg(&token_file).spawn()?;
        }
        Ok(())
    };
    run().map_err(|e| format!("{e:#}"))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", service::PORT)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Err("Geo Unlocker's background service didn't start.".into())
}

pub fn run() -> anyhow::Result<()> {
    let in_process = cfg!(windows) || is_root();
    let boot = move || -> (App, Task<Message>) {
        if in_process {
            match backend::start_local() {
                Ok(ctl) => {
                    let app = App::new(Some(Backend::Local(ctl)), None, None);
                    let t = app.refresh_lists();
                    (app, t)
                }
                Err(e) => (App::new(None, None, Some(format!("{e:#}"))), Task::none()),
            }
        } else {
            let token = backend::random_token().unwrap_or_default();
            let app = App::new(
                Some(Backend::Remote { token: token.clone() }),
                Some("Waiting for administrator permission…\nGeo Unlocker needs it to change network settings.".into()),
                None,
            );
            #[cfg(unix)]
            let t = Task::perform(async move { tokio::task::spawn_blocking(move || launch_service(&token)).await.unwrap_or_else(|e| Err(e.to_string())) }, Message::ServiceReady);
            #[cfg(not(unix))]
            let t = Task::none();
            (app, t)
        }
    };
    launch(boot)
}

/// Open the window with a canned backend (see examples/preview.rs).
pub fn run_preview(backend: Backend) -> anyhow::Result<()> {
    launch(move || {
        let mut app = App::new(Some(backend.clone()), None, None);
        let mut t = app.refresh_lists();
        // GEO_PREVIEW_OPEN=details opens the Technical details panel (for screenshots).
        if std::env::var("GEO_PREVIEW_OPEN").as_deref() == Ok("details") {
            app.overlay = Some(Overlay::Advanced);
            t = Task::batch([t, app.call("GET", "/advanced", Value::Null, Message::Advanced)]);
        }
        (app, t)
    })
}

fn launch(boot: impl Fn() -> (App, Task<Message>) + 'static) -> anyhow::Result<()> {
    iced::application(boot, update, view::view)
        .title("Geo Unlocker")
        .theme(|_: &App| style::theme())
        .subscription(subscription)
        .window(window::Settings {
            size: Size::new(1240.0, 780.0),
            min_size: Some(Size::new(940.0, 620.0)),
            maximized: true,
            icon: icon(),
            exit_on_close_request: false,
            ..Default::default()
        })
        .antialiasing(true)
        .run()
        .map_err(|e| anyhow::anyhow!("can't open the window: {e}"))
}
