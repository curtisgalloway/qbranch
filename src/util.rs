// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Small helpers shared by every module: fatal exits, JSON file I/O, and
//! Python-compatible formatting for the strings the corpus pins. The
//! reference implementation is a Python script, and several of its messages
//! embed `repr()` output (`'name'`, `['a', 'b']`), so those forms are
//! reproduced here rather than approximated.

use serde_json::{Map, Value as Json};
use std::fs;
use std::io;
use std::path::Path;
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

/// `json.dumps(v, indent=2)`, including its default `ensure_ascii`: every
/// non-ASCII character becomes a `\uXXXX` escape (a surrogate pair above
/// the BMP), so files and reports match the reference byte for byte. The
/// serializer only ever emits non-ASCII inside string literals, so a pass
/// over the finished text is safe.
pub fn pretty(v: &Json) -> String {
    let raw = serde_json::to_string_pretty(v).unwrap_or_default();
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

pub fn write_json(p: &Path, v: &Json) -> io::Result<()> {
    fs::write(p, pretty(v) + "\n")
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

/// `repr()` of a str.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr()` of a value `json.loads` would have produced.
pub fn py_repr(v: &Json) -> String {
    match v {
        Json::Null => "None".to_string(),
        Json::Bool(true) => "True".to_string(),
        Json::Bool(false) => "False".to_string(),
        Json::Number(n) => n.to_string(),
        Json::String(s) => py_repr_str(s),
        Json::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Json::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `str()` of such a value: strings bare, everything else as `repr()`.
pub fn py_str(v: &Json) -> String {
    match v {
        Json::String(s) => s.clone(),
        other => py_repr(other),
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
