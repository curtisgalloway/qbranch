// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Copy mode: materialise an entry as a copy of its source instead of a
//! link, tell whether an existing copy is still faithful, and the probe
//! that decides whether this process may create symbolic links at all.

use crate::ctx::COPY_IGNORE;
use crate::paths;
use crate::util::PrivateStagingDir;
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
    let staging = PrivateStagingDir::beside(dst)?;
    let replacement = staging.path().join("new");
    if src.is_dir() {
        copy_tree(src, &replacement)?;
    } else {
        fs::copy(src, &replacement)?;
    }

    let backup = staging.path().join("backup");
    let had_destination = fs::symlink_metadata(dst).is_ok();
    if had_destination {
        fs::rename(dst, &backup)?;
    }
    if let Err(replace_error) = fs::rename(&replacement, dst) {
        if had_destination {
            if let Err(restore_error) = fs::rename(&backup, dst) {
                let recovery = staging.preserve();
                return Err(io::Error::new(
                    replace_error.kind(),
                    format!(
                        "{replace_error}; restoring the previous copy failed: {restore_error}; recover it from {}",
                        recovery.display()
                    ),
                ));
            }
        }
        return Err(replace_error);
    }
    if had_destination {
        remove_path(&backup)?;
    }
    Ok(())
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
    match (fs::read(src), fs::read(dst)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn names(d: &Path) -> io::Result<Vec<std::ffi::OsString>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(d)? {
        let entry = entry?;
        if !ignored(&entry.file_name()) {
            out.push(entry.file_name());
        }
    }
    out.sort();
    Ok(out)
}

pub fn trees_equal(a: &Path, b: &Path) -> bool {
    let (Ok(names_a), Ok(names_b)) = (names(a), names(b)) else {
        return false;
    };
    if names_a != names_b {
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
        } else if !pb.is_file() {
            return false;
        } else {
            match (fs::read(&pa), fs::read(&pb)) {
                (Ok(a), Ok(b)) if a == b => {}
                _ => return false,
            }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[cfg(unix)]
    #[test]
    fn failed_tree_copy_preserves_destination() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!(
            "qbranch-copy-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("working.txt"), "keep me").unwrap();
        symlink("missing", src.join("nested/dangling")).unwrap();

        assert!(copy_path(&src, &dst).is_err());
        assert_eq!(
            fs::read_to_string(dst.join("working.txt")).unwrap(),
            "keep me"
        );
        assert!(!dst.join("nested").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
