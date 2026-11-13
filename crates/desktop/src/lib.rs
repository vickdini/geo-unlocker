//! Geo Unlocker desktop app library (the binary is in main.rs).

pub mod cloud;
pub mod controller;
pub mod pilink;
pub mod platform;
pub mod systun;
pub mod tor;
pub mod vpn;
pub mod updater;
pub mod gui;
pub mod service;
pub mod wgtun;
pub mod api;
pub mod backend;

/// Show a fatal error: a message box on Windows (no console there), stderr elsewhere.
pub fn fatal(msg: &str) {
    geo_common::log(format!("fatal: {msg}"));
    eprintln!("error: {msg}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(msg), wide("Geo Unlocker"));
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
    }
}
