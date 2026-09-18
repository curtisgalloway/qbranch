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
    copy_path_with_rename(src, dst, |from, to| fs::rename(from, to))
}

fn copy_path_with_rename(
    src: &Path,
    dst: &Path,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
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
        rename(dst, &backup)?;
    }
    if let Err(replace_error) = rename(&replacement, dst) {
        if had_destination {
            if let Err(restore_error) = rename(&backup, dst) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qbranch-copy-{}-{tag}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn copies_files_and_trees_while_ignoring_state() {
        let dir = test_dir("success");
        let source_file = dir.join("source.txt");
        let copied_file = dir.join("copied.txt");
        fs::write(&source_file, "file contents").unwrap();
        copy_path(&source_file, &copied_file).unwrap();
        assert_eq!(fs::read_to_string(copied_file).unwrap(), "file contents");

        let source_tree = dir.join("source-tree");
        let copied_tree = dir.join("copied-tree");
        fs::create_dir_all(source_tree.join("nested")).unwrap();
        fs::write(source_tree.join("nested/kept"), "kept").unwrap();
        for state in COPY_IGNORE {
            fs::write(source_tree.join(state), "ignored").unwrap();
        }
        copy_path(&source_tree, &copied_tree).unwrap();
        assert_eq!(
            fs::read_to_string(copied_tree.join("nested/kept")).unwrap(),
            "kept"
        );
        for state in COPY_IGNORE {
            assert!(!copied_tree.join(state).exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_source_preserves_destination() {
        let dir = test_dir("missing-source");
        let dst = dir.join("dst");
        fs::write(&dst, "old").unwrap();
        assert!(copy_path(&dir.join("missing"), &dst).is_err());
        assert_eq!(fs::read_to_string(dst).unwrap(), "old");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replacement_failure_restores_destination() {
        let dir = test_dir("rollback");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::write(&src, "new").unwrap();
        fs::write(&dst, "old").unwrap();
        let mut calls = 0;
        let error = copy_path_with_rename(&src, &dst, |from, to| {
            calls += 1;
            if calls == 2 {
                Err(io::Error::other("injected replacement failure"))
            } else {
                fs::rename(from, to)
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected replacement failure"));
        assert_eq!(fs::read_to_string(&dst).unwrap(), "old");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rollback_failure_retains_recoverable_backup() {
        let dir = test_dir("failed-rollback");
        let src = dir.join("src");
        let dst = dir.join("dst");
        fs::write(&src, "new").unwrap();
        fs::write(&dst, "old").unwrap();
        let mut calls = 0;
        let error = copy_path_with_rename(&src, &dst, |from, to| {
            calls += 1;
            if calls >= 2 {
                Err(io::Error::other(format!("injected failure {calls}")))
            } else {
                fs::rename(from, to)
            }
        })
        .unwrap_err();
        let message = error.to_string();
        let recovery = message.split("recover it from ").nth(1).unwrap();
        assert_eq!(
            fs::read_to_string(Path::new(recovery).join("backup")).unwrap(),
            "old"
        );
        fs::remove_dir_all(recovery).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_tree_copy_preserves_destination() {
        use std::os::unix::fs::symlink;
        let dir = test_dir("failed-tree");
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

    #[cfg(unix)]
    #[test]
    fn replacing_destination_symlink_does_not_touch_target() {
        use std::os::unix::fs::symlink;
        let dir = test_dir("destination-symlink");
        let src = dir.join("src");
        let target = dir.join("target");
        let dst = dir.join("dst");
        fs::write(&src, "new").unwrap();
        fs::write(&target, "target bytes").unwrap();
        symlink(&target, &dst).unwrap();
        copy_path(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "new");
        assert_eq!(fs::read_to_string(&target).unwrap(), "target bytes");
        assert!(!fs::symlink_metadata(&dst).unwrap().file_type().is_symlink());
        fs::remove_dir_all(dir).unwrap();
    }
}
