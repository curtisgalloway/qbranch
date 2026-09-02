// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Path helpers that reproduce the pathlib and os.path behaviour the
//! reference script relies on: lexical cleaning, `~` and `$VAR` expansion,
//! non-strict resolution, and a PATH lookup. The plan the corpus compares
//! prints paths, so the string form of every path has to come out the way
//! `str(pathlib.Path(...))` would print it.

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// `Path.home()`.
pub fn home_dir() -> PathBuf {
    if let Some(h) = env::var_os("HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    #[cfg(windows)]
    if let Some(h) = env::var_os("USERPROFILE").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    PathBuf::from("/")
}

/// `pathlib.Path(s)`: drop empty and `.` components, keep `..`, drop a
/// trailing separator.
pub fn clean(s: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for c in Path::new(s).components() {
        if c != Component::CurDir {
            out.push(c.as_os_str());
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `Path.expanduser()` for a leading `~`.
pub fn expanduser(p: &Path, home: &Path) -> PathBuf {
    let mut comps = p.components();
    if let Some(Component::Normal(first)) = comps.next() {
        if first == "~" {
            let mut out = home.to_path_buf();
            for c in comps {
                out.push(c.as_os_str());
            }
            return out;
        }
    }
    p.to_path_buf()
}

/// `os.path.expandvars`: `$NAME` and `${NAME}` from the environment, an
/// unknown variable left as written.
pub fn expandvars(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() {
            if b[i + 1] == b'{' {
                if let Some(end) = s[i + 2..].find('}') {
                    let name = &s[i + 2..i + 2 + end];
                    match env::var_os(name) {
                        Some(v) => out.push_str(&v.to_string_lossy()),
                        None => out.push_str(&s[i..i + 3 + end]),
                    }
                    i += 3 + end;
                    continue;
                }
            } else {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if j > start {
                    match env::var_os(&s[start..j]) {
                        Some(v) => out.push_str(&v.to_string_lossy()),
                        None => out.push_str(&s[i..j]),
                    }
                    i = j;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `Path.resolve()` (non-strict): canonicalize the longest existing prefix
/// and append the rest lexically.
pub fn resolve(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        env::current_dir().unwrap_or_default().join(p)
    };
    if let Ok(c) = abs.canonicalize() {
        return c;
    }
    let comps: Vec<Component> = abs.components().collect();
    for k in (1..=comps.len()).rev() {
        let prefix: PathBuf = comps[..k].iter().map(|c| c.as_os_str()).collect();
        if let Ok(mut c) = prefix.canonicalize() {
            for comp in &comps[k..] {
                match comp {
                    Component::ParentDir => {
                        c.pop();
                    }
                    Component::CurDir => {}
                    other => c.push(other.as_os_str()),
                }
            }
            return c;
        }
    }
    abs
}

/// `p.relative_to(root)` succeeds: a lexical prefix test.
pub fn is_under(p: &Path, root: &Path) -> bool {
    p.starts_with(root)
}

pub fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// `os.readlink` cleaned the way `Path(...)` would clean it.
pub fn read_link(p: &Path) -> Option<PathBuf> {
    fs::read_link(p).ok().map(|t| clean(&t.to_string_lossy()))
}

/// `Path.name`.
pub fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `Path.parent` (the path itself at the root, as pathlib does).
pub fn parent(p: &Path) -> PathBuf {
    p.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| p.to_path_buf())
}

/// `shutil.which`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for candidate in candidates(&dir, name) {
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    vec![dir.join(name)]
}

#[cfg(windows)]
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    let exts = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut out = vec![dir.join(name)];
    for ext in exts.split(';').filter(|e| !e.is_empty()) {
        out.push(dir.join(format!("{name}{ext}")));
    }
    out
}

#[cfg(not(windows))]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Create a symbolic link at `dst` pointing at `src`, as written.
#[cfg(not(windows))]
pub fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
pub fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

/// Remove a symlink (or file) at `p` without following it.
pub fn unlink(p: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if fs::symlink_metadata(p)?.is_dir() {
            return fs::remove_dir(p);
        }
    }
    fs::remove_file(p)
}
