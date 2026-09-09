// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Why a skill source went missing, and what to do about it.
//!
//! A manifest entry whose source path has vanished has four possible causes,
//! and they want opposite fixes: the directory was renamed (rewrite the
//! entry), it was deleted (remove the entry), it lives on a branch this
//! checkout does not have (change nothing), or it never existed (a typo, or
//! an uncloned repo). Telling them apart is what stops a sync failing
//! forever with an error no one can act on.
//!
//! Git records no directory renames, only per-file ones, so the
//! directory-level answer is aggregated back out of the `R` lines. Note that
//! the log must be limited to the *parent* of the missing path: limiting it
//! to the old path itself returns nothing, because history simplification
//! prunes a pathspec that no longer resolves in HEAD.

// Nothing calls this yet: the MISS path in sync.rs and --fix-renames are the
// consumers, and this allow comes off when they land.
#![allow(dead_code)]

use crate::proc;
use crate::util::display;
use std::path::{Path, PathBuf};

/// How many commits of history to search before giving up. The log is not
/// path-limited (see `measure`), so this bounds the whole scan.
const LOG_DEPTH: &str = "500";

/// Renames are followed hop by hop; this bounds a pathological chain.
const MAX_HOPS: usize = 8;

/// One directory that files from the missing path moved to.
#[derive(Clone, Debug)]
pub struct Destination {
    /// Repo-relative path, e.g. `plugins/driver-porting/skills/cleanroom-spec`.
    pub rel: String,
    /// Files that landed here from the directory at this hop.
    pub files: usize,
    /// Lowest git similarity score among those files, 0-100.
    pub min_score: u32,
    /// Whether this directory is present in the working tree now.
    pub exists_now: bool,
}

/// Everything measured about a missing path, before any judgement is applied.
#[derive(Clone, Debug)]
pub struct Signals {
    /// Where the trail ends: the destinations found at the final hop, most
    /// files first. One entry is the ordinary case.
    pub destinations: Vec<Destination>,
    /// Renames followed to get there. More than one means the directory moved
    /// repeatedly, as it does when a repo is restructured under it.
    pub hops: usize,
    /// A hop offered several destinations at once, so the trail was cut short
    /// rather than guessed at.
    pub ambiguous: bool,
    /// Files under the path removed outright at the end of the trail.
    pub deleted: usize,
    /// Files the original path held before anything happened to it.
    pub held: usize,
    /// Short commit of the *first* rename: the one that broke the manifest.
    pub commit: String,
    /// Refs that still carry the path, e.g. `(feature)`. Empty when none.
    pub other_refs: String,
}

/// What to tell the user about a missing source.
#[derive(Clone, Debug)]
pub enum Verdict {
    /// Moved. `new_src` is absolute; `new_name` is its basename.
    Renamed {
        new_src: PathBuf,
        new_name: String,
        commit: String,
    },
    /// Gone from this branch, and not moved anywhere.
    Deleted { commit: String },
    /// Not on this branch, but present on another ref.
    Elsewhere { refs: String },
    /// No evidence either way.
    Unknown,
}

// ---------------------------------------------------------------------------
// Judgement
// ---------------------------------------------------------------------------

/// Choose the destination to suggest, or `None` to fall through to the
/// deleted / elsewhere / unknown branches.
///
/// This is the confidence rule: given what git recorded, is the evidence
/// strong enough to tell someone their skill moved, and to let
/// `--fix-renames` rewrite their manifest on the strength of it?
///
/// `s.destinations` is sorted with the most files first, so `.first()` is the
/// leading candidate. The signals worth weighing:
///
///   - `d.exists_now` — the trail is followed until it reaches a directory
///     that is really there, but it can run out first (a chain that ends in a
///     deletion, or history older than the scan). A suggestion pointing at a
///     path that is not on disk is worse than no suggestion.
///   - `d.files` vs `s.held` — did the whole directory move, or did one file
///     get pulled out of it into somewhere unrelated?
///   - `s.ambiguous` / `s.destinations.len()` — the directory was split across
///     several places, and no single rename describes it.
///   - `d.min_score` — git's similarity score, 0-100. A low score means the
///     file was substantially rewritten as it moved, so the rename detection
///     is a guess rather than a near-identical match.
///   - `s.hops` — a longer trail is still a real answer, but each hop is one
///     more inference away from what the manifest actually recorded.
///
/// Two real cases, for calibration:
///
///   oxbox        ox-review -> oxbox-review
///                held 4, files 4, min_score 75, hops 1, exists_now true
///   public-skills peripheral-spec -> plugins/driver-porting/skills/cleanroom-spec
///                held 3, files 3, min_score 98, hops 2, exists_now true
pub fn pick_destination(s: &Signals) -> Option<&Destination> {
    // A directory that was split across several places has no single rename
    // that describes it, so there is nothing honest to suggest.
    if s.ambiguous || s.destinations.len() != 1 {
        return None;
    }
    let d = s.destinations.first()?;
    // The trail is followed until it reaches something on disk, but it can run
    // out first: a chain ending in a deletion, or history older than the scan.
    // Naming a path nobody can open is worse than saying nothing.
    if !d.exists_now {
        return None;
    }
    // Most of what the directory held has to have made the trip. This lets a
    // move that dropped a file or two through, while rejecting a single file
    // lifted out of a directory that stayed where it was. `held` is 0 when the
    // pre-rename tree could not be read, and then it does not gate anything.
    if s.held > 0 && d.files * 2 < s.held {
        return None;
    }
    // Deliberately no similarity floor. Git's own rename threshold is 50%, so
    // every R line already clears that bar, and the real cases sit at 75 and
    // 98 -- a floor tight enough to matter would reject the oxbox rename that
    // prompted this code.
    Some(d)
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

/// The enclosing git work tree of `p`, which need not exist itself.
pub fn git_root(p: &Path) -> Option<PathBuf> {
    let mut cur = Some(p);
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

fn git(root: &Path, rest: &[&str]) -> Option<String> {
    let mut argv: Vec<String> = vec!["git".into(), "-C".into(), display(root)];
    argv.extend(rest.iter().map(|s| s.to_string()));
    match proc::run_capture(&argv, None) {
        Ok(out) if out.ok() => Some(out.stdout),
        _ => None,
    }
}

/// A `R<score>\t<old>\t<new>` or `D\t<path>` line, with its commit.
struct Change {
    commit: String,
    score: u32,
    old: String,
    new: String,
}

/// Parse `--name-status --format=%h` output into changes, newest first.
fn parse_log(out: &str) -> Vec<Change> {
    let mut commit = String::new();
    let mut changes = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        if !line.contains('\t') {
            commit = line.trim().to_string();
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        let status = cols[0];
        if status.starts_with('R') && cols.len() >= 3 {
            changes.push(Change {
                commit: commit.clone(),
                score: status[1..].parse().unwrap_or(0),
                old: cols[1].to_string(),
                new: cols[2].to_string(),
            });
        } else if status.starts_with('D') && cols.len() >= 2 {
            changes.push(Change {
                commit: commit.clone(),
                score: 0,
                old: cols[1].to_string(),
                new: String::new(),
            });
        }
    }
    changes
}

/// `child` is `dir` itself or below it.
fn under(child: &str, dir: &str) -> bool {
    child == dir || child.starts_with(&format!("{dir}/"))
}

/// The directory a moved file landed in, mapped back to the old directory.
///
/// `skills/a/scripts/x.py -> skills/b/scripts/x.py` under old `skills/a`
/// yields `skills/b`: the new path with the file's position inside the old
/// directory trimmed off its tail. Trimming by depth rather than by prefix is
/// what lets a skill move out of its parent entirely, as it does when a repo
/// is restructured into plugins.
fn destination_dir(old: &str, new: &str, old_dir: &str) -> Option<String> {
    let tail = old.strip_prefix(old_dir)?.trim_start_matches('/');
    let depth = tail.split('/').count();
    let parts: Vec<&str> = new.split('/').collect();
    if parts.len() <= depth {
        return None;
    }
    Some(parts[..parts.len() - depth].join("/"))
}

/// Destinations for one hop: where files under `dir` moved, and the commit.
fn hop(changes: &[Change], dir: &str, root: &Path) -> (Vec<Destination>, String) {
    let mut dests: Vec<Destination> = Vec::new();
    let mut commit = String::new();
    for c in changes
        .iter()
        .filter(|c| !c.new.is_empty() && under(&c.old, dir))
    {
        let Some(d) = destination_dir(&c.old, &c.new, dir) else {
            continue;
        };
        if d == dir {
            continue;
        }
        if commit.is_empty() {
            commit = c.commit.clone();
        }
        match dests.iter_mut().find(|x| x.rel == d) {
            Some(x) => {
                x.files += 1;
                x.min_score = x.min_score.min(c.score);
            }
            None => dests.push(Destination {
                exists_now: root.join(&d).is_dir(),
                rel: d,
                files: 1,
                min_score: c.score,
            }),
        }
    }
    dests.sort_by(|a, b| b.files.cmp(&a.files).then(a.rel.cmp(&b.rel)));
    (dests, commit)
}

/// Measure what git knows about a missing path. `None` when it is not in a
/// git work tree at all.
///
/// The log is deliberately *not* path-limited: `-M` only detects a rename when
/// the pathspec covers both sides of it, so limiting to the missing path's
/// parent hides every move that leaves that parent -- which is exactly what a
/// repo restructure does.
pub fn measure(missing: &Path) -> Option<Signals> {
    let root = git_root(missing)?;
    let rel = missing
        .strip_prefix(&root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");

    let log = git(
        &root,
        &[
            "log",
            "--diff-filter=RD",
            "-M",
            "--name-status",
            "--format=%h",
            "-n",
            LOG_DEPTH,
        ],
    )?;
    let changes = parse_log(&log);

    // Follow the trail until it reaches something that is really on disk.
    let mut current = rel.clone();
    let mut hops = 0usize;
    let mut ambiguous = false;
    let mut commit = String::new();
    // The weakest similarity anywhere along the trail: a confident final hop
    // says nothing about a shaky first one.
    let mut chain_min = 100u32;
    let mut dests;
    loop {
        let (d, c) = hop(&changes, &current, &root);
        dests = d;
        if dests.is_empty() {
            break;
        }
        if dests.len() > 1 {
            ambiguous = true;
            break;
        }
        if commit.is_empty() {
            commit = c;
        }
        chain_min = chain_min.min(dests[0].min_score);
        dests[0].min_score = chain_min;
        current = dests[0].rel.clone();
        hops += 1;
        if dests[0].exists_now || hops >= MAX_HOPS {
            break;
        }
    }

    let deleted = changes
        .iter()
        .filter(|c| c.new.is_empty() && under(&c.old, &current))
        .count();
    if commit.is_empty() {
        commit = changes
            .iter()
            .find(|c| c.new.is_empty() && under(&c.old, &current))
            .map(|c| c.commit.clone())
            .unwrap_or_default();
    }

    let held = if commit.is_empty() {
        0
    } else {
        git(
            &root,
            &[
                "ls-tree",
                "-r",
                "--name-only",
                &format!("{commit}^"),
                "--",
                &rel,
            ],
        )
        .map(|o| o.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
    };

    let other_refs = if dests.is_empty() && deleted == 0 {
        git(
            &root,
            &["log", "--all", "--format=%d", "-n", "1", "--", &rel],
        )
        .map(|o| o.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default()
    } else {
        String::new()
    };

    Some(Signals {
        destinations: dests,
        hops,
        ambiguous,
        deleted,
        held,
        commit,
        other_refs,
    })
}

/// Diagnose a missing source path.
pub fn diagnose(missing: &Path) -> Verdict {
    let Some(s) = measure(missing) else {
        return Verdict::Unknown;
    };
    let root = match git_root(missing) {
        Some(r) => r,
        None => return Verdict::Unknown,
    };
    if let Some(d) = pick_destination(&s) {
        let new_src = root.join(&d.rel);
        let new_name = d.rel.rsplit('/').next().unwrap_or(&d.rel).to_string();
        return Verdict::Renamed {
            new_src,
            new_name,
            commit: s.commit.clone(),
        };
    }
    if s.deleted > 0 {
        return Verdict::Deleted { commit: s.commit };
    }
    if !s.other_refs.is_empty() {
        return Verdict::Elsewhere { refs: s.other_refs };
    }
    Verdict::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// A repo exercising each way a skill directory can go missing. `tag`
    /// keeps parallel tests out of each other's tree.
    fn repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qbranch-rename-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(&dir, &["config", "user.email", "t@example.invalid"]);
        git(&dir, &["config", "user.name", "t"]);
        for n in ["alpha", "beta", "gamma", "stays"] {
            write(
                &dir,
                &format!("skills/{n}/SKILL.md"),
                "line one\nline two\nline three\n",
            );
            write(
                &dir,
                &format!("skills/{n}/scripts/go.py"),
                "print(1)\nprint(2)\n",
            );
        }
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "base"]);

        // A plain rename.
        git(&dir, &["mv", "skills/alpha", "skills/alpha-renamed"]);
        git(&dir, &["commit", "-qm", "rename alpha"]);

        // A deletion.
        git(&dir, &["rm", "-rq", "skills/beta"]);
        git(&dir, &["commit", "-qm", "delete beta"]);

        // Two hops: renamed, then the tree restructured under it. This is the
        // shape public-skills had, and it is what single-hop detection misses.
        git(&dir, &["mv", "skills/gamma", "skills/gamma-renamed"]);
        git(&dir, &["commit", "-qm", "rename gamma"]);
        fs::create_dir_all(dir.join("plugins/pack/skills")).unwrap();
        git(
            &dir,
            &[
                "mv",
                "skills/gamma-renamed",
                "plugins/pack/skills/gamma-renamed",
            ],
        );
        git(&dir, &["commit", "-qm", "restructure into plugins"]);

        // Present only on another branch.
        git(&dir, &["checkout", "-q", "-b", "feature"]);
        write(&dir, "skills/onbranch/SKILL.md", "body\n");
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-qm", "add onbranch"]);
        git(&dir, &["checkout", "-q", "-"]);
        dir
    }

    #[test]
    fn follows_a_plain_rename() {
        let d = repo("plain");
        match diagnose(&d.join("skills/alpha")) {
            Verdict::Renamed {
                new_name, new_src, ..
            } => {
                assert_eq!(new_name, "alpha-renamed");
                assert_eq!(new_src, d.join("skills/alpha-renamed"));
            }
            v => panic!("expected Renamed, got {v:?}"),
        }
    }

    #[test]
    fn follows_a_rename_through_a_restructure() {
        let d = repo("chain");
        let s = measure(&d.join("skills/gamma")).expect("signals");
        assert_eq!(s.hops, 2, "should follow both hops");
        match diagnose(&d.join("skills/gamma")) {
            Verdict::Renamed { new_src, .. } => {
                assert_eq!(new_src, d.join("plugins/pack/skills/gamma-renamed"));
            }
            v => panic!("expected Renamed, got {v:?}"),
        }
    }

    #[test]
    fn reports_a_deletion_as_deleted() {
        let d = repo("deleted");
        assert!(matches!(
            diagnose(&d.join("skills/beta")),
            Verdict::Deleted { .. }
        ));
    }

    #[test]
    fn reports_a_path_held_only_on_another_branch() {
        let d = repo("branch");
        assert!(matches!(
            diagnose(&d.join("skills/onbranch")),
            Verdict::Elsewhere { .. }
        ));
    }

    #[test]
    fn says_nothing_about_a_path_that_never_existed() {
        let d = repo("never");
        assert!(matches!(
            diagnose(&d.join("skills/never")),
            Verdict::Unknown
        ));
    }

    /// Opt-in check against a real checkout, for calibrating the rule:
    ///   QBRANCH_PROBE=/path/to/missing/skill cargo test probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_real_checkout() {
        let Ok(target) = std::env::var("QBRANCH_PROBE") else {
            println!("set QBRANCH_PROBE to a missing skill path");
            return;
        };
        let path = PathBuf::from(&target);
        if let Some(s) = measure(&path) {
            println!(
                "held={} deleted={} hops={} ambiguous={} commit={}",
                s.held, s.deleted, s.hops, s.ambiguous, s.commit
            );
            for d in &s.destinations {
                println!(
                    "  dest={} files={} min_score={} exists_now={}",
                    d.rel, d.files, d.min_score, d.exists_now
                );
            }
        }
        println!("verdict: {:?}", diagnose(&path));
    }

    #[test]
    fn not_a_git_tree_is_unknown() {
        assert!(matches!(diagnose(Path::new("/")), Verdict::Unknown));
    }
}
