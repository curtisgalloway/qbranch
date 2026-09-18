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
use std::sync::mpsc;
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
    let mut command = Command::new(program(first));
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Keep descendants in a group that can be terminated if they retain
        // one of the captured pipes after the direct child has exited.
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let child_id = child.id();
    let mut so = child.stdout.take().expect("piped stdout");
    let mut se = child.stderr.take().expect("piped stderr");
    let (sender, receiver) = mpsc::channel();
    let stdout_sender = sender.clone();
    thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        let _ = stdout_sender.send((true, b));
    });
    thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        let _ = sender.send((false, b));
    });
    let deadline = timeout.map(|t| Instant::now() + t);
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    loop {
        if status.is_none() {
            status = child.try_wait().map_err(|e| e.to_string())?;
        }
        if status.is_some() && stdout.is_some() && stderr.is_some() {
            break;
        }
        let wait = match deadline {
            Some(d) => {
                let Some(remaining) = d.checked_duration_since(Instant::now()) else {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child_id as libc::pid_t), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "Command '{}' timed out after {} seconds",
                        argv.join(" "),
                        timeout.map(|t| t.as_secs()).unwrap_or(0)
                    ));
                };
                remaining.min(Duration::from_millis(20))
            }
            None => Duration::from_millis(20),
        };
        match receiver.recv_timeout(wait) {
            Ok((true, bytes)) => stdout = Some(bytes),
            Ok((false, bytes)) => stderr = Some(bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(wait),
        }
    }
    let status = match status {
        Some(status) => status,
        None => child.wait().map_err(|e| e.to_string())?,
    };
    Ok(Output {
        code: status.code(),
        stdout: String::from_utf8_lossy(stdout.as_deref().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(stderr.as_deref().unwrap_or_default()).into_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> Vec<String> {
        #[cfg(windows)]
        return vec![
            "cmd.exe".to_string(),
            "/D".to_string(),
            "/C".to_string(),
            command.to_string(),
        ];
        #[cfg(not(windows))]
        return vec!["/bin/sh".to_string(), "-c".to_string(), command.to_string()];
    }

    #[test]
    fn captures_both_output_streams_exactly() {
        #[cfg(windows)]
        let argv = shell("echo stdout exact&1>&2 echo stderr exact&exit /b 0");
        #[cfg(not(windows))]
        let argv = shell("printf 'stdout exact'; printf 'stderr exact' >&2");
        let output = run_capture(&argv, None).unwrap();
        assert_eq!(output.code, Some(0));
        let newline = if cfg!(windows) { "\r\n" } else { "" };
        assert_eq!(output.stdout, format!("stdout exact{newline}"));
        assert_eq!(output.stderr, format!("stderr exact{newline}"));
    }

    #[test]
    fn reports_nonzero_exit() {
        let output = run_capture(&shell("exit 7"), None).unwrap();
        assert_eq!(output.code, Some(7));
        assert!(!output.ok());
        assert_eq!(output.code_str(), "7");
    }

    #[test]
    fn direct_child_timeout_is_enforced() {
        let started = Instant::now();
        #[cfg(windows)]
        let argv = shell("ping -n 3 127.0.0.1 >nul");
        #[cfg(not(windows))]
        let argv = shell("sleep 2");
        let error = match run_capture(&argv, Some(Duration::from_millis(100))) {
            Err(error) => error,
            Ok(_) => panic!("command unexpectedly completed"),
        };
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_survives_parent_exit_while_descendant_holds_pipe() {
        let argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "sleep 2 & exit 0".to_string(),
        ];
        let started = Instant::now();
        let error = match run_capture(&argv, Some(Duration::from_millis(100))) {
            Err(error) => error,
            Ok(_) => panic!("command unexpectedly completed"),
        };
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_survives_closed_pipes_while_child_keeps_running() {
        let argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "exec >/dev/null 2>&1; sleep 2".to_string(),
        ];
        let started = Instant::now();
        let error = match run_capture(&argv, Some(Duration::from_millis(100))) {
            Err(error) => error,
            Ok(_) => panic!("command unexpectedly completed"),
        };
        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
