//! Helpers shared by macOS and Linux.

/// Re-exec through sudo when not root.
pub fn ensure_root() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    use std::os::unix::process::CommandExt;
    eprintln!("Geo Unlocker needs administrator rights to manage VPN connections; asking sudo…");
    let exe = std::env::current_exe().expect("current exe");
    let err = std::process::Command::new("sudo")
        .arg("-E")
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("failed to run sudo: {err}");
    std::process::exit(1);
}

/// Wrap a command so it runs as the desktop user who started sudo
/// (needed for opening the browser and changing desktop settings).
pub fn as_user(cmd: &[&str]) -> Vec<String> {
    // Started via sudo (SUDO_*) or via the graphical admin prompt pkexec (PKEXEC_UID).
    let uid = std::env::var("SUDO_UID").or_else(|_| std::env::var("PKEXEC_UID")).ok();
    let user = std::env::var("SUDO_USER").ok().or_else(|| {
        let uid = uid.as_deref()?;
        let o = geo_common::sh::try_run("id", ["-nu", uid]);
        o.ok().then(|| o.stdout.trim().to_string())
    });
    let Some(user) = user.filter(|u| !u.is_empty() && u != "root") else {
        return cmd.iter().map(|s| s.to_string()).collect();
    };
    let mut out: Vec<String> = vec!["sudo".into(), "-u".into(), user.clone(), "env".into()];
    if let Some(uid) = uid {
        out.push(format!("XDG_RUNTIME_DIR=/run/user/{uid}"));
        out.push(format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"));
    }
    for var in ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"] {
        if let Ok(v) = std::env::var(var) {
            out.push(format!("{var}={v}"));
        }
    }
    out.extend(cmd.iter().map(|s| s.to_string()));
    out
}

pub fn spawn_detached(cmd: Vec<String>) {
    if let Some((prog, args)) = cmd.split_first() {
        let _ = std::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

pub fn chmod_private(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}
