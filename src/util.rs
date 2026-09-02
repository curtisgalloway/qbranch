// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Small helpers shared by every module: fatal exits, JSON file I/O, and
//! the Python-compatible JSON and `str()` formatting that keeps the port's
//! output byte-identical to the reference script's.

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
