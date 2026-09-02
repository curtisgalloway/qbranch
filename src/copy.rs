// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Copy mode: materialise an entry as a copy of its source instead of a
//! link, tell whether an existing copy is still faithful, and the probe
//! that decides whether this process may create symbolic links at all.

use crate::ctx::COPY_IGNORE;
use crate::paths;
use std::fs;
use std::io;
use std::path::Path;

fn ignored(name: &std::ffi::OsStr) -> bool {
    COPY_IGNORE.iter().any(|n| name == *n)
}

/// Remove a link, a file or a whole directory tree at p; nothing there is fine.
pub fn remove_path(p: &Path) -> io::Result<()> {
    if paths::is_symlink(p) || p.is_file() {
        paths::unlink(p)
    } else if p.is_dir() {
        fs::remove_dir_all(p)
    } else {
        Ok(())
    }
}

/// Copy src to dst (a directory tree or one file), replacing what is there.
///
/// Symlinks inside src are followed, so the copy stands on its own; the
/// state file is left out of a copied skills directory.
pub fn copy_path(src: &Path, dst: &Path) -> io::Result<()> {
    remove_path(dst)?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if src.is_dir() {
        copy_tree(src, dst)
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if ignored(&name) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Whether dst is a faithful copy of src: same tree, same bytes.
pub fn copy_up_to_date(src: &Path, dst: &Path) -> bool {
    if paths::is_symlink(dst) {
        return false;
    }
    if src.is_dir() {
        return dst.is_dir() && trees_equal(src, dst);
    }
    dst.is_file() && fs::read(src).ok() == fs::read(dst).ok()
}

fn names(d: &Path) -> Vec<std::ffi::OsString> {
    let mut out: Vec<_> = fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name())
                .filter(|n| !ignored(n))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

pub fn trees_equal(a: &Path, b: &Path) -> bool {
    let names_a = names(a);
    if names_a != names(b) {
        return false;
    }
    for n in names_a {
        let (pa, pb) = (a.join(&n), b.join(&n));
        if paths::is_symlink(&pb) {
            return false;
        }
        if pa.is_dir() {
            if !pb.is_dir() || !trees_equal(&pa, &pb) {
                return false;
            }
        } else if !pb.is_file() || fs::read(&pa).ok() != fs::read(&pb).ok() {
            return false;
        }
    }
    true
}

/// Whether this process may create symbolic links (a Windows privilege).
pub fn symlinks_available() -> bool {
    let probe = std::env::temp_dir().join(format!(".qbranch-probe-{}", std::process::id()));
    if paths::symlink(Path::new("qbranch-probe-target"), &probe).is_err() {
        return false;
    }
    let _ = paths::unlink(&probe);
    true
}
