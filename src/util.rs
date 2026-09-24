// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Small helpers shared by every module: fatal exits, JSON file I/O, and
//! the Python-compatible JSON and `str()` formatting that keeps the port's
//! output byte-identical to the reference script's.

use serde_json::{Map, Value as Json};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub type JMap = Map<String, Json>;

/// `sys.exit(msg)`: print to stderr, exit 1.
pub fn die(msg: impl AsRef<str>) -> ! {
    eprintln!("{}", msg.as_ref());
    std::process::exit(1)
}

pub fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub fn read_json(p: &Path) -> Result<Json, String> {
    let text = fs::read_to_string(p).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub fn read_json_object(p: &Path) -> Result<JMap, String> {
    match read_json(p)? {
        Json::Object(m) => Ok(m),
        other => Err(format!("expected a JSON object, got {}", kind(&other))),
    }
}

pub fn kind(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Number(_) => "a number",
        Json::String(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    }
}

/// `json.dumps`'s default `ensure_ascii`: every non-ASCII character becomes
/// a `\uXXXX` escape (a surrogate pair above the BMP). The serializer only
/// ever emits non-ASCII inside string literals, so a pass over the finished
/// text is safe.
fn ascii_escape(raw: String) -> String {
    if raw.is_ascii() {
        return raw;
    }
    let mut out = String::with_capacity(raw.len() + 16);
    for c in raw.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

/// `json.dumps(v, indent=2)`.
pub fn pretty(v: &Json) -> String {
    ascii_escape(serde_json::to_string_pretty(v).unwrap_or_default())
}

/// `json.dumps(v)`: one line, with Python's `, ` and `: ` separators.
pub fn py_dumps(v: &Json) -> String {
    let mut out = String::new();
    write_py(v, &mut out);
    ascii_escape(out)
}

fn write_py(v: &Json, out: &mut String) {
    match v {
        Json::Null | Json::Bool(_) | Json::Number(_) => out.push_str(&v.to_string()),
        Json::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Json::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_py(x, out);
            }
            out.push(']');
        }
        Json::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push_str(": ");
                write_py(x, out);
            }
            out.push('}');
        }
    }
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// An owner-only directory beside a destination, removed when dropped.
pub struct PrivateStagingDir {
    path: PathBuf,
}

impl PrivateStagingDir {
    pub fn beside(destination: &Path) -> io::Result<Self> {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let stem = destination
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("qbranch"))
            .to_string_lossy();
        for _ in 0..1000 {
            let sequence = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(".{stem}.qbranch-{}-{sequence}", std::process::id()));
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700).create(&path)
            };
            #[cfg(not(unix))]
            let created = fs::create_dir(&path);
            match created {
                Ok(()) => {
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create unique staging directory",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Leave the directory behind for manual recovery after a failed rollback.
    pub fn preserve(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

impl Drop for PrivateStagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn resolved_write_destination(p: &Path) -> PathBuf {
    if fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        crate::paths::resolve(p)
    } else {
        p.to_path_buf()
    }
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    type Bool = i32;
    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "MoveFileExW"]
        fn move_file_ex_w(existing: *const u16, new: *const u16, flags: u32) -> Bool;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let ok = unsafe {
        move_file_ex_w(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn write_atomic(p: &Path, bytes: &[u8], private: bool, follow_symlink: bool) -> io::Result<()> {
    write_atomic_with_replace(p, bytes, private, follow_symlink, atomic_replace)
}

fn write_atomic_with_replace(
    p: &Path,
    bytes: &[u8],
    private: bool,
    follow_symlink: bool,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let destination = if follow_symlink {
        resolved_write_destination(p)
    } else {
        p.to_path_buf()
    };
    let staging = PrivateStagingDir::beside(&destination)?;
    let staged = staging.path().join("new");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staged)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if private {
            0o600
        } else {
            fs::metadata(&destination)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0o600)
        };
        file.set_permissions(fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = private;
    drop(file);
    replace(&staged, &destination)
}

pub fn write_json(p: &Path, v: &Json) -> io::Result<()> {
    let mut bytes = serde_json::to_string_pretty(v)
        .map(ascii_escape)
        .map_err(io::Error::other)?
        .into_bytes();
    bytes.push(b'\n');
    write_atomic(p, &bytes, false, true)
}

/// Atomically write an owner-only JSON file without following a destination link.
pub fn write_json_private(p: &Path, v: &Json) -> io::Result<()> {
    let mut bytes = serde_json::to_string_pretty(v)
        .map(ascii_escape)
        .map_err(io::Error::other)?
        .into_bytes();
    bytes.push(b'\n');
    write_atomic(p, &bytes, true, false)
}

/// Atomically write bytes, keeping an existing file's mode.
pub fn write_bytes(p: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(p, bytes, false, true)
}

/// Atomically write owner-only bytes without following a destination link.
pub fn write_private(p: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(p, bytes, true, false)
}

pub fn obj(v: Option<&Json>) -> Option<&JMap> {
    match v {
        Some(Json::Object(m)) => Some(m),
        _ => None,
    }
}

pub fn arr(v: Option<&Json>) -> Option<&Vec<Json>> {
    match v {
        Some(Json::Array(a)) => Some(a),
        _ => None,
    }
}

pub fn string(v: Option<&Json>) -> Option<&str> {
    match v {
        Some(Json::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Python truthiness of a JSON value (`if x:`).
pub fn truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Json::String(s)) => !s.is_empty(),
        Some(Json::Array(a)) => !a.is_empty(),
        Some(Json::Object(m)) => !m.is_empty(),
    }
}

/// `m.get(k) or {}` as an owned map.
pub fn obj_or_empty(m: &JMap, k: &str) -> JMap {
    obj(m.get(k)).cloned().unwrap_or_default()
}

/// `m.get(k, [])` / `m.get(k) or []` as an owned list.
pub fn arr_or_empty(m: &JMap, k: &str) -> Vec<Json> {
    arr(m.get(k)).cloned().unwrap_or_default()
}

/// `str()` of a value `json.loads` produced: strings bare, `None`, `True`,
/// numbers as written; containers as compact JSON.
pub fn py_str(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        Json::Null => "None".to_string(),
        Json::Bool(true) => "True".to_string(),
        Json::Bool(false) => "False".to_string(),
        other => py_dumps(other),
    }
}

/// `str(m.get(k))`: "None" when absent.
pub fn py_get_str(m: &JMap, k: &str) -> String {
    m.get(k).map(py_str).unwrap_or_else(|| "None".to_string())
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
pub fn utc_now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use serde_json::json;

    fn test_dir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "qbranch-util-{}-{tag}-{}",
            std::process::id(),
            STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        p
    }

    #[test]
    fn failed_json_write_preserves_existing_bytes() {
        let dir = test_dir("write-failure");
        let path = dir.join("settings.json");
        fs::write(&path, b"old bytes\n").unwrap();
        let result = write_atomic_with_replace(&path, b"new bytes\n", false, true, |_, _| {
            Err(io::Error::other("injected publish failure"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"old bytes\n");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn json_permissions_and_symlink_policy() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = test_dir("permissions");
        let new_private = dir.join("new-private.json");
        write_json_private(&new_private, &json!({"new": true})).unwrap();
        assert_eq!(
            fs::metadata(&new_private).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let existing_private = dir.join("existing-private.json");
        fs::write(&existing_private, "{}\n").unwrap();
        fs::set_permissions(&existing_private, fs::Permissions::from_mode(0o644)).unwrap();
        write_json_private(&existing_private, &json!({"private": true})).unwrap();
        assert_eq!(
            fs::metadata(&existing_private)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let target = dir.join("target.json");
        fs::write(&target, "{}\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.join("link.json");
        symlink(&target, &link).unwrap();

        write_json(&link, &json!({"ordinary": true})).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );

        write_json_private(&link, &json!({"private": true})).unwrap();
        assert!(!fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::metadata(&link).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_json(&target).unwrap(), json!({"ordinary": true}));

        let raw_link = dir.join("raw.json");
        symlink(&target, &raw_link).unwrap();
        let raw = b"{ \"legacy\" : true }\n";
        write_private(&raw_link, raw).unwrap();
        assert_eq!(fs::read(&raw_link).unwrap(), raw);
        assert!(!fs::symlink_metadata(&raw_link)
            .unwrap()
            .file_type()
            .is_symlink());
        fs::remove_dir_all(dir).unwrap();
    }
}
