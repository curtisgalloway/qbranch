// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Subprocess helpers: `subprocess.run(capture_output=True, timeout=...)`
//! with stdin closed, and the inherit-everything form used for `git clone`.
//! On Windows a bare program name is resolved through PATH and PATHEXT
//! first, so an npm-installed `claude.cmd` runs the way `claude.exe` would.

use crate::paths;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// `returncode` for messages: the exit code, or -1 when killed by a signal.
    pub fn code_str(&self) -> String {
        self.code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-1".to_string())
    }
}

/// The program to hand to `Command::new`: on Windows, a bare name found on
/// PATH (with PATHEXT), so `.cmd` shims run through the standard library's
/// batch-file handling; elsewhere, the name as given.
fn program(name: &str) -> PathBuf {
    if cfg!(windows) && !name.contains(['/', '\\']) {
        if let Some(found) = paths::which(name) {
            return found;
        }
    }
    PathBuf::from(name)
}

/// Run `argv`, capturing both streams, with stdin closed. A timeout of
/// `None` waits forever. Errors cover a failed spawn and the timeout.
pub fn run_capture(argv: &[String], timeout: Option<Duration>) -> Result<Output, String> {
    let (first, rest) = argv.split_first().ok_or("empty command")?;
    let mut child = Command::new(program(first))
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut so = child.stdout.take().expect("piped stdout");
    let mut se = child.stderr.take().expect("piped stderr");
    let t_out = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let deadline = timeout.map(|t| Instant::now() + t);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "Command '{}' timed out after {} seconds",
                        argv.join(" "),
                        timeout.map(|t| t.as_secs()).unwrap_or(0)
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    Ok(Output {
        code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Run `argv` with inherited stdio and return whether it exited 0.
pub fn run_inherit(argv: &[String]) -> Result<bool, String> {
    let (first, rest) = argv.split_first().ok_or("empty command")?;
    Command::new(program(first))
        .args(rest)
        .status()
        .map(|s| s.success())
        .map_err(|e| e.to_string())
}

/// `os.execvp` on Unix; spawn-wait-exit elsewhere.
pub fn exec(argv: &[String]) -> ! {
    let (first, rest) = argv.split_first().expect("non-empty command");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(program(first)).args(rest).exec();
        eprintln!("{first}: {err}");
        std::process::exit(1)
    }
    #[cfg(not(unix))]
    {
        match Command::new(program(first)).args(rest).status() {
            Ok(s) => std::process::exit(s.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("{first}: {e}");
                std::process::exit(1)
            }
        }
    }
}
