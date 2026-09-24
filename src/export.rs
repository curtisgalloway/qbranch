// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! --export-zips: the manifest's skills as one zip per skill, for a harness
//! that installs skills by upload (the Claude web and desktop apps) rather
//! than from a directory qbranch can link into. The uploader keeps a copy of
//! each zip it has uploaded under uploaded/, which is how the next export
//! tells a changed skill from a current one without state of its own.

use crate::ctx::Ctx;
use crate::skills::{collect_repo_skills, resolve_skill_src};
use crate::util::{self, die, display, JMap};
use serde_json::{json, Value as Json};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const EXPORT_INDEX: &str = "export.json";
const EXPORT_UPLOADED: &str = "uploaded";
// 1980-01-01 00:00, the earliest DOS timestamp. Every entry carries it, so a
// zip's bytes depend on the skill's files alone and an unchanged skill
// exports byte for byte the same.
const ZIP_DOS_DATE: u16 = (1 << 5) | 1;
const ZIP_DOS_TIME: u16 = 0;
const ZIP_LIMIT: usize = 0xFFFF_FFFF;

/// (path in the zip, contents or None for a directory, executable)
type Entry = (String, Option<Vec<u8>>, bool);

/// Whether a path inside a skill directory goes into its export zip.
///
/// `rel` is the path's components relative to the skill directory; a
/// directory that is left out is not descended into. Dotfiles go (.git, a
/// plugin's .claude-plugin), and so does build output, which changes without
/// the skill changing and would otherwise force a re-upload.
fn export_includes(rel: &[String]) -> bool {
    let last = rel.last().map(String::as_str).unwrap_or("");
    !(last.starts_with('.')
        || last == "__pycache__"
        || last == "node_modules"
        || last.ends_with(".pyc"))
}

/// Every entry of skill directory `src`, under a top directory `name`/,
/// sorted by path. Symbolic links to files are followed; links to
/// directories, and anything that is neither file nor directory, are left
/// out. So is a directory holding a CACHEDIR.TAG, the marker build tools
/// such as cargo put in their output directories.
fn skill_zip_entries(name: &str, src: &Path) -> io::Result<Vec<Entry>> {
    fn walk(name: &str, d: &Path, rel: &[String], entries: &mut Vec<Entry>) -> io::Result<()> {
        for child in fs::read_dir(d)? {
            let child = child?.path();
            let mut parts = rel.to_vec();
            parts.push(
                child
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
            if !export_includes(&parts) {
                continue;
            }
            let path = format!("{name}/{}", parts.join("/"));
            let is_link = fs::symlink_metadata(&child)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if child.is_dir() && !is_link {
                if child.join("CACHEDIR.TAG").is_file() {
                    continue;
                }
                entries.push((path + "/", None, false));
                walk(name, &child, &parts, entries)?;
            } else if child.is_file() {
                entries.push((path, Some(fs::read(&child)?), executable(&child)?));
            }
        }
        Ok(())
    }

    let mut entries = vec![(format!("{name}/"), None, false)];
    walk(name, src, &[], &mut entries)?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

#[cfg(unix)]
fn executable(p: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(p)?.permissions().mode() & 0o100 != 0)
}

#[cfg(not(unix))]
fn executable(_: &Path) -> io::Result<bool> {
    Ok(false)
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// The CRC-32 a zip records for each entry (`zlib.crc32`).
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// A stored (uncompressed) zip archive of `entries`, with UTF-8 names, Unix
/// permissions and fixed timestamps; an error past the plain (non-Zip64)
/// format's limits.
fn build_zip(entries: &[Entry]) -> Result<Vec<u8>, String> {
    let mut body: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (path, data, executable) in entries {
        let raw = path.as_bytes();
        let content: &[u8] = data.as_deref().unwrap_or(&[]);
        let crc = crc32(content);
        let attrs: u32 = match data {
            None => (0o040755 << 16) | 0x10,
            Some(_) if *executable => 0o100755 << 16,
            Some(_) => 0o100644 << 16,
        };
        let offset = body.len();
        if content.len() > ZIP_LIMIT || offset > ZIP_LIMIT {
            return Err("larger than 4 GiB".to_string());
        }
        let size = content.len() as u32;
        let name_len = raw.len() as u16;
        body.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        for v in [20u16, 0x0800, 0, ZIP_DOS_TIME, ZIP_DOS_DATE] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, size, size] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name_len, 0] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        body.extend_from_slice(raw);
        body.extend_from_slice(content);

        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        for v in [0x0314u16, 20, 0x0800, 0, ZIP_DOS_TIME, ZIP_DOS_DATE] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc, size, size] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name_len, 0, 0, 0, 0] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [attrs, offset as u32] {
            central.extend_from_slice(&v.to_le_bytes());
        }
        central.extend_from_slice(raw);
    }
    if entries.len() > 0xFFFF {
        return Err("more than 65535 files".to_string());
    }
    if body.len() > ZIP_LIMIT {
        return Err("larger than 4 GiB".to_string());
    }
    let count = entries.len() as u16;
    let mut out = body.clone();
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    for v in [0u16, 0, count, count] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [central.len() as u32, body.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// Skill names listed by an earlier export.json; empty if it is unreadable.
fn previous_names(index: &Path) -> BTreeSet<String> {
    let Ok(Json::Object(m)) = util::read_json(index) else {
        return BTreeSet::new();
    };
    let Some(skills) = util::arr(m.get("skills")) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    for s in skills {
        match util::obj(Some(s)).and_then(|s| util::string(s.get("name"))) {
            Some(n) => out.insert(n.to_string()),
            None => return BTreeSet::new(),
        };
    }
    out
}

/// Write DIR/<skill>.zip for every skill the manifest links, and
/// DIR/export.json classifying each against DIR/uploaded/<skill>.zip.
///
/// Returns (report, warnings, failures). A skill is `new` when nothing has
/// been uploaded for it, `changed` when its zip differs from the uploaded
/// one and `current` when they match. `dropped` lists uploaded skills the
/// manifest no longer has; it stays empty while anything failed, since a
/// missing checkout would otherwise read as every one of its skills dropped.
pub fn export_zips(
    ctx: &Ctx,
    manifest_name: &str,
    manifest: &JMap,
    skills_target: &Path,
    out_dir: &Path,
) -> (JMap, Vec<String>, Vec<String>) {
    let mut skills: Vec<(String, PathBuf)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for entry in util::arr_or_empty(manifest, "skills") {
        let entry = util::obj(Some(&entry))
            .unwrap_or_else(|| die("manifest 'skills' entries must be objects"));
        let name = util::string(entry.get("name"))
            .unwrap_or_else(|| die("manifest 'skills' entry without a 'name'"))
            .to_string();
        let dst = skills_target.join(&name);
        if !seen.insert(dst.clone()) {
            die(format!(
                "duplicate destination in manifest: {}",
                display(&dst)
            ));
        }
        skills.push((name, resolve_skill_src(ctx, entry)));
    }
    let repo = collect_repo_skills(ctx, manifest, skills_target, &seen);
    skills.extend(repo.entries.into_iter().map(|d| (d.label, d.src)));
    let mut fails: Vec<String> = repo
        .missing
        .iter()
        .map(|r| {
            format!(
                "skill repo {} not found — clone it, or drop it from the manifest's skill_repos",
                display(r)
            )
        })
        .collect();

    if let Err(e) = fs::create_dir_all(out_dir) {
        die(format!("{}: {e}", display(out_dir)));
    }
    let index = out_dir.join(EXPORT_INDEX);
    let previous = previous_names(&index);

    let mut rows: Vec<Json> = Vec::new();
    let mut exported: HashSet<String> = HashSet::new();
    let mut failed: HashSet<String> = HashSet::new();
    skills.sort();
    for (name, src) in skills {
        if !src.join("SKILL.md").is_file() {
            fails.push(format!("{name}: no SKILL.md in {}", display(&src)));
            failed.insert(name);
            continue;
        }
        let data = match skill_zip_entries(&name, &src)
            .map_err(|e| e.to_string())
            .and_then(|entries| build_zip(&entries))
        {
            Ok(d) => d,
            Err(e) => {
                fails.push(format!("{name}: {e}"));
                failed.insert(name);
                continue;
            }
        };
        let zip_name = format!("{name}.zip");
        let zip_path = out_dir.join(&zip_name);
        if fs::read(&zip_path).ok().as_deref() != Some(&data[..]) {
            if let Err(e) = util::write_bytes(&zip_path, &data) {
                fails.push(format!("{name}: {e}"));
                failed.insert(name);
                continue;
            }
        }
        let uploaded = out_dir.join(EXPORT_UPLOADED).join(&zip_name);
        let status = if !uploaded.is_file() {
            "new"
        } else if fs::read(&uploaded).ok().as_deref() == Some(&data[..]) {
            "current"
        } else {
            "changed"
        };
        rows.push(json!({"name": name, "zip": zip_name, "status": status}));
        exported.insert(name);
    }

    for name in &previous {
        if exported.contains(name) || failed.contains(name) {
            continue;
        }
        let stale = out_dir.join(format!("{name}.zip"));
        if stale.is_file() {
            let _ = fs::remove_file(&stale);
        }
    }
    let mut dropped: Vec<String> = Vec::new();
    let uploaded_dir = out_dir.join(EXPORT_UPLOADED);
    if fails.is_empty() && uploaded_dir.is_dir() {
        if let Ok(rd) = fs::read_dir(&uploaded_dir) {
            for e in rd.flatten() {
                let file_name = e.file_name().to_string_lossy().into_owned();
                let Some(stem) = file_name.strip_suffix(".zip") else {
                    continue;
                };
                if !stem.is_empty() && e.path().is_file() && !exported.contains(stem) {
                    dropped.push(stem.to_string());
                }
            }
        }
        dropped.sort();
    }
    let mut report = JMap::new();
    report.insert("manifest".to_string(), json!(manifest_name));
    report.insert("skills".to_string(), Json::Array(rows));
    report.insert("dropped".to_string(), json!(dropped));
    if let Err(e) = util::write_json(&index, &Json::Object(report.clone())) {
        die(format!("{}: {e}", display(&index)));
    }
    (report, repo.warnings, fails)
}

pub fn print_export(report: &JMap, out_dir: &Path) {
    let rows = util::arr_or_empty(report, "skills");
    let mut pending = 0;
    for row in &rows {
        let status = util::py_get_str(util::obj(Some(row)).unwrap(), "status");
        let name = util::py_get_str(util::obj(Some(row)).unwrap(), "name");
        println!("  {status:<8} {name}");
        if status != "current" {
            pending += 1;
        }
    }
    for name in util::arr_or_empty(report, "dropped") {
        println!(
            "  {:<8} {}  (not in the manifest; turn it off)",
            "dropped",
            util::py_str(&name)
        );
    }
    if pending > 0 {
        println!("\n{pending} to upload from {}", display(out_dir));
    } else {
        println!("\nnothing to upload from {}", display(out_dir));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn filter_drops_dotfiles_and_build_output() {
        let v = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(export_includes(&v(&["scripts", "check.py"])));
        assert!(!export_includes(&v(&[".git"])));
        assert!(!export_includes(&v(&["scripts", "__pycache__"])));
        assert!(!export_includes(&v(&["node_modules"])));
        assert!(!export_includes(&v(&["a", "b.pyc"])));
    }
}
