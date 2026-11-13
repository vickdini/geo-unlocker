//! The desktop window (tao + wry, using the system web engine: WebView2 on Windows,
//! WKWebView on macOS, WebKitGTK on Linux).

use crate::{backend, web};
use anyhow::{anyhow, Result};
use std::time::{Duration, Instant};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Icon, WindowBuilder};
use wry::{NewWindowResponse, WebViewBuilder};

#[cfg_attr(windows, allow(dead_code))]
enum UserEvent {
    /// macOS/Linux: the privileged service is up.
    Ready,
    Failed(String),
    /// An update was installed: restart the app (sent by the page via IPC).
    Restart,
}

const BG: (u8, u8, u8, u8) = (23, 24, 28, 255);

const WAITING_HTML: &str = r#"<!doctype html><html><body style="margin:0;height:100vh;display:grid;place-items:center;
background:#17181c;color:#e8e8ee;font:15px system-ui,-apple-system,'Segoe UI',sans-serif">
<div style="text-align:center"><div style="font-size:40px">🌍</div><p><b>Geo Unlocker</b></p>
<p style="color:#9a9aa8">Waiting for administrator permission…<br>Geo Unlocker needs it to change network settings.</p></div></body></html>"#;

/// Open a link in the user's normal browser (not inside the app).
pub fn open_external(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    #[cfg(windows)]
    // Via explorer so the browser doesn't inherit our administrator rights.
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

fn is_root() -> bool {
    #[cfg(unix)]
    return unsafe { libc::geteuid() } == 0;
    #[cfg(not(unix))]
    return false;
}

/// Simple round app icon (purple disc with a white ring), drawn in code.
fn icon() -> Option<Icon> {
    const N: u32 = 64;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let (r, g, b, a) = if d > 31.5 {
                (0, 0, 0, 0)
            } else if (17.0..21.5).contains(&d) || ((x as f32 - c).abs() < 2.2 && d < 21.5) || ((y as f32 - c).abs() < 2.2 && d < 21.5) {
                (255, 255, 255, 255)
            } else {
                let t = y as f32 / N as f32;
                ((109.0 + 40.0 * t) as u8, (74.0 - 20.0 * t) as u8, 255, 255)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    Icon::from_rgba(rgba, N, N).ok()
}

#[cfg(unix)]
fn launch_privileged_service(token: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let exe = std::env::current_exe()?;
    let token_file = std::env::temp_dir().join(format!("geo-unlocker-{}.token", std::process::id()));
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&token_file)?;
    std::fs::write(&token_file, token)?;
    let q = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    #[cfg(target_os = "macos")]
    {
        let shell = format!(
            "{} --service --token-file {} >/dev/null 2>&1 &",
            q(&exe.to_string_lossy()),
            q(&token_file.to_string_lossy())
        );
        let script = format!(
            "do shell script \"{}\" with administrator privileges",
            shell.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let st = std::process::Command::new("osascript").args(["-e", &script]).status()?;
        if !st.success() {
            anyhow::bail!("Administrator permission was not granted.");
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = q;
        if geo_common::sh::which("pkexec").is_none() {
            anyhow::bail!("pkexec is not installed. Start Geo Unlocker with: sudo {}", exe.display());
        }
        std::process::Command::new("pkexec")
            .arg(&exe)
            .args(["--service", "--token-file"])
            .arg(&token_file)
            .spawn()?;
    }
    Ok(())
}

/// Start the (updated) app again a moment after we exit.
fn relaunch() {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = std::env::current_exe().ok().and_then(|e| e.ancestors().nth(3).map(|p| p.to_path_buf())) {
        // Wait for the old background service to free its port, then open the app again.
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
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn wait_for_server(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", web::PORT)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

pub fn run() -> Result<()> {
    let token = backend::random_token()?;
    let url = format!("http://127.0.0.1:{}/?t={token}", web::PORT);
    let origin = format!("http://127.0.0.1:{}", web::PORT);

    // Windows: this process already has administrator rights (exe manifest).
    // macOS/Linux: run the privileged part as a separate service unless we're root.
    let in_process = cfg!(windows) || is_root();
    let ctl = if in_process { Some(backend::start(token.clone(), false)?) } else { None };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Geo Unlocker")
        .with_inner_size(LogicalSize::new(1240.0, 780.0))
        .with_min_inner_size(LogicalSize::new(940.0, 620.0))
        .with_maximized(true)
        .with_window_icon(icon())
        .build(&event_loop)
        .map_err(|e| anyhow!("can't open the window: {e}"))?;

    let nav_origin = origin.clone();
    let ipc_proxy = event_loop.create_proxy();
    let builder = WebViewBuilder::new()
        .with_ipc_handler(move |req: wry::http::Request<String>| {
            if req.body() == "restart" {
                let _ = ipc_proxy.send_event(UserEvent::Restart);
            }
        })
        .with_background_color(BG)
        .with_devtools(cfg!(debug_assertions))
        // Stay on our own page; anything else (sign-in links, help) opens in the browser.
        .with_navigation_handler(move |u| {
            if u.starts_with(&nav_origin) || u.starts_with("about:") || u.starts_with("data:") {
                true
            } else {
                open_external(&u);
                false
            }
        })
        .with_new_window_req_handler(|u, _| {
            open_external(&u);
            NewWindowResponse::Deny
        });
    let builder = if in_process { builder.with_url(&url) } else { builder.with_html(WAITING_HTML) };

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let webview = builder.build(&window)?;
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().ok_or_else(|| anyhow!("no GTK container"))?)?
    };

    #[cfg(unix)]
    if !in_process {
        let proxy = event_loop.create_proxy();
        let token = token.clone();
        std::thread::spawn(move || {
            let ev = match launch_privileged_service(&token) {
                Err(e) => UserEvent::Failed(format!("{e:#}")),
                Ok(()) if wait_for_server(Duration::from_secs(180)) => UserEvent::Ready,
                Ok(()) => UserEvent::Failed("The Geo Unlocker service didn't start.".into()),
            };
            let _ = proxy.send_event(ev);
        });
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Ready) => {
                let _ = webview.load_url(&url);
            }
            Event::UserEvent(UserEvent::Failed(msg)) => {
                crate::fatal(&msg);
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } | Event::UserEvent(UserEvent::Restart) => {
                let restart = matches!(event, Event::UserEvent(UserEvent::Restart));
                // Closing the app disconnects and restores normal networking.
                match &ctl {
                    Some(c) => c.shutdown(),
                    None => {
                        let _ = ureq::post(&format!("{origin}/api/quit"))
                            .set("Authorization", &format!("Bearer {token}"))
                            .timeout(Duration::from_secs(20))
                            .send_string("{}");
                    }
                }
                if restart {
                    relaunch();
                }
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
