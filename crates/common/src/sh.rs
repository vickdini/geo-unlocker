//! Cross-platform helpers for running external programs.

use anyhow::{bail, Result};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Result of a finished command.
pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    /// stderr + stdout, trimmed (for error messages).
    pub fn text(&self) -> String {
        format!("{}{}", self.stderr, self.stdout).trim().to_string()
    }
}

/// Find a program on PATH. On Windows, tries PATHEXT extensions (.exe, .cmd, ...).
pub fn which(prog: &str) -> Option<PathBuf> {
    let p = Path::new(prog);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .collect()
    } else {
        Vec::new()
    };
    for dir in std::env::split_paths(&path) {
        if cfg!(windows) {
            if p.extension().is_some() && dir.join(prog).is_file() {
                return Some(dir.join(prog));
            }
            for e in &exts {
                let c = dir.join(format!("{prog}{e}"));
                if c.is_file() {
                    return Some(c);
                }
            }
        } else {
            let c = dir.join(prog);
            if is_executable(&c) {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// First existing path from `extra`, else `which(prog)`.
pub fn find(prog: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    extra.iter().find(|p| p.is_file()).cloned().or_else(|| which(prog))
}

/// A `Command` with the program resolved via PATH and no console window on Windows.
pub fn command(prog: impl AsRef<OsStr>) -> Command {
    let prog = prog.as_ref();
    let resolved: OsString = prog
        .to_str()
        .and_then(which)
        .map(PathBuf::into_os_string)
        .unwrap_or_else(|| prog.to_os_string());
    #[allow(unused_mut)]
    let mut c = Command::new(resolved);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Run a program to completion. Never fails: a missing program gives code 127.
pub fn exec<I, S>(prog: impl AsRef<OsStr>, args: I, input: Option<&str>, env: &[(&str, &str)]) -> Out
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut c = command(&prog);
    c.args(args);
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            return Out {
                code: 127,
                stdout: String::new(),
                stderr: format!("{}: {e}", prog.as_ref().to_string_lossy()),
            }
        }
    };
    if let (Some(inp), Some(mut s)) = (input, child.stdin.take()) {
        let _ = s.write_all(inp.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => Out { code: -1, stdout: String::new(), stderr: e.to_string() },
    }
}

/// Run and return the result whatever the exit code.
pub fn try_run<I, S>(prog: impl AsRef<OsStr>, args: I) -> Out
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    exec(prog, args, None, &[])
}

/// Run and return stdout, or an error containing the command's output.
pub fn run<I, S>(prog: impl AsRef<OsStr>, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_with(prog, args, None, &[])
}

pub fn run_with<I, S>(prog: impl AsRef<OsStr>, args: I, input: Option<&str>, env: &[(&str, &str)]) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().to_os_string()).collect();
    let o = exec(&prog, &args, input, env);
    if !o.ok() {
        let cmdline = std::iter::once(prog.as_ref().to_string_lossy().into_owned())
            .chain(args.iter().map(|a| a.to_string_lossy().into_owned()))
            .collect::<Vec<_>>()
            .join(" ");
        let text = o.text();
        let tail: String = text.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect();
        bail!("`{cmdline}` failed ({}): {tail}", o.code);
    }
    Ok(o.stdout)
}
