//! Shared code for the Geo Unlocker Pi and desktop apps.

pub mod jobs;
pub mod profiles;
pub mod sh;
pub mod state;
pub mod update;
pub mod web;

static LOG_FILE: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
static RECENT: std::sync::Mutex<std::collections::VecDeque<String>> = std::sync::Mutex::new(std::collections::VecDeque::new());

/// The most recent log lines (newest last), for showing in an app.
pub fn recent_log() -> Vec<String> {
    RECENT.lock().unwrap().iter().cloned().collect()
}

/// Also append log lines to this file (the windowed desktop app has no console).
pub fn set_log_file(path: &std::path::Path) {
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        *LOG_FILE.lock().unwrap() = Some(f);
    }
}

/// Log a line to stderr (systemd's journal adds timestamps) and the log file, if set.
pub fn log(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("[geo-unlocker] {msg}");
    {
        let mut r = RECENT.lock().unwrap();
        r.push_back(format!("{} {msg}", clock()));
        if r.len() > 400 {
            r.pop_front();
        }
    }
    if let Some(f) = LOG_FILE.lock().unwrap().as_mut() {
        use std::io::Write;
        let _ = writeln!(f, "{} {msg}", now());
    }
}

/// Write a file readable only by its owner (on Unix).
pub fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Current UTC time as HH:MM:SS.
pub fn clock() -> String {
    let t = now() % 86_400;
    format!("{:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60)
}

/// Replace secret values in WireGuard configs / setup scripts with <hidden>.
pub fn redact(text: &str) -> String {
    text.lines()
        .map(|l| {
            let key = l.split('=').next().unwrap_or("").trim().to_ascii_lowercase();
            if ["privatekey", "presharedkey"].contains(&key.as_str()) {
                format!("{} = <hidden>", l.split('=').next().unwrap_or("").trim_end())
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
