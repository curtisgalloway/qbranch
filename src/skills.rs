// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! What the manifest wants linked: `skills` and `links` entries, the
//! per-harness skills-directory links, and the skills discovered in each
//! `skill_repos` checkout (marketplace-aware).

use crate::ctx::Ctx;
use crate::manifest::{local_checkout, repo_name_from_url};
use crate::paths;
use crate::proc;
use crate::util::{self, die, display, py_repr, JMap};
use serde_json::Value as Json;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One thing the manifest wants linked: `dst -> src`.
#[derive(Clone, Debug)]
pub struct Desired {
    pub label: String,
    pub src: PathBuf,
    pub dst: PathBuf,
}

/// Return the local root of repo_url, cloning if necessary.
///
/// Checks ~/src/<name> first (convention); if that's a git checkout it is
/// used as-is. Otherwise clones into ~/.agents/skill-repos/<name>/ and pulls
/// on every subsequent sync. A pre-existing clone in the legacy
/// ~/.claude/skill-repos/<name>/ location is honored.
pub fn resolve_repo_local(ctx: &Ctx, repo_url: &str) -> PathBuf {
    let name = repo_name_from_url(repo_url);
    let local = local_checkout(ctx, repo_url);
    if local.is_dir() && local.join(".git").exists() {
        return local;
    }
    let is_checkout = |p: &Path| p.is_dir() && p.join(".git").exists();
    let mut cache = ctx.skill_repos_cache.join(&name);
    let legacy_cache = ctx.legacy_skill_repos_cache.join(&name);
    if !is_checkout(&cache) && is_checkout(&legacy_cache) {
        cache = legacy_cache;
    }
    if is_checkout(&cache) {
        let argv: Vec<String> = ["git", "-C", &display(&cache), "pull", "--ff-only"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        match proc::run_capture(&argv, None) {
            Ok(out) if out.ok() => {}
            Ok(out) => eprintln!("warning: git pull failed for {name}: {}", out.stderr.trim()),
            Err(e) => eprintln!("warning: git pull failed for {name}: {e}"),
        }
    } else {
        if let Err(e) = fs::create_dir_all(&ctx.skill_repos_cache) {
            die(format!("{}: {}", display(&ctx.skill_repos_cache), e));
        }
        let argv: Vec<String> = ["git", "clone", repo_url, &display(&cache)]
            .iter()
            .map(|s| s.to_string())
            .collect();
        match proc::run_inherit(&argv) {
            Ok(true) => {}
            Ok(false) => die(format!("git clone {repo_url} failed")),
            Err(e) => die(format!("git clone {repo_url}: {e}")),
        }
    }
    cache
}

/// Return the source path for a skills entry (path-based or repo-based).
pub fn resolve_skill_src(ctx: &Ctx, entry: &JMap) -> PathBuf {
    if let Some(repo) = entry.get("repo") {
        let rel = paths::clean(&util::py_get_str(entry, "path"));
        return resolve_repo_local(ctx, &util::py_str(repo)).join(rel);
    }
    ctx.expand(&util::py_get_str(entry, "path"))
}

/// (desired entries, skipped-entry notes) from the manifest.
pub fn collect_desired(
    ctx: &Ctx,
    manifest: &JMap,
    skills_target: &Path,
    claude_ok: bool,
    agy_ok: bool,
) -> (Vec<Desired>, Vec<String>) {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for entry in util::arr_or_empty(manifest, "skills") {
        let entry = util::obj(Some(&entry))
            .unwrap_or_else(|| die("manifest 'skills' entries must be objects"));
        let name = util::string(entry.get("name"))
            .unwrap_or_else(|| die("manifest 'skills' entry without a 'name'"))
            .to_string();
        let src = resolve_skill_src(ctx, entry);
        let dst = skills_target.join(&name);
        if !seen.insert(dst.clone()) {
            die(format!(
                "duplicate destination in manifest: {}",
                display(&dst)
            ));
        }
        out.push(Desired {
            label: name,
            src,
            dst,
        });
    }

    for entry in util::arr_or_empty(manifest, "links") {
        let entry = util::obj(Some(&entry))
            .unwrap_or_else(|| die("manifest 'links' entries must be objects"));
        let src = ctx.expand(&util::py_get_str(entry, "src"));
        let dst = ctx.expand(&util::py_get_str(entry, "dst"));
        let label = match entry.get("label") {
            Some(l) if util::truthy(Some(l)) => util::py_str(l),
            _ => paths::name(&dst),
        };
        if !claude_ok && paths::is_under(&dst, &ctx.claude_dir) {
            skipped.push(format!(
                "{label}: {} (Claude Code not installed)",
                display(&dst)
            ));
            continue;
        }
        if !agy_ok && paths::is_under(&dst, &ctx.agy_dir) {
            skipped.push(format!(
                "{label}: {} (Antigravity not installed)",
                display(&dst)
            ));
            continue;
        }
        if !seen.insert(dst.clone()) {
            die(format!(
                "duplicate destination in manifest: {}",
                display(&dst)
            ));
        }
        out.push(Desired { label, src, dst });
    }

    // Claude reads skills from ~/.claude/skills: point it at the generic dir.
    if claude_ok && ctx.claude_skills_link != skills_target {
        out.push(Desired {
            label: "claude-skills".to_string(),
            src: skills_target.to_path_buf(),
            dst: ctx.claude_skills_link.clone(),
        });
    }
    // Same for agy, which reads ~/.gemini/antigravity-cli/skills.
    if agy_ok && ctx.agy_skills_link != skills_target {
        out.push(Desired {
            label: "agy-skills".to_string(),
            src: skills_target.to_path_buf(),
            dst: ctx.agy_skills_link.clone(),
        });
    }
    (out, skipped)
}

/// Immediate subdirectories of d that hold a SKILL.md, sorted.
pub fn scan_skill_dirs(d: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(d) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    children.sort();
    children
        .into_iter()
        .filter(|c| c.is_dir() && c.join("SKILL.md").is_file())
        .collect()
}

pub fn read_marketplace(repo: &Path) -> Option<JMap> {
    let p = repo.join(".claude-plugin").join("marketplace.json");
    if !p.is_file() {
        return None;
    }
    match util::read_json(&p) {
        Ok(Json::Object(m)) => Some(m),
        Ok(_) => Some(JMap::new()),
        Err(e) => {
            eprintln!(
                "warning: {}: {e} — treating {} as a plain skill repo",
                display(&p),
                paths::name(repo)
            );
            None
        }
    }
}

/// The skill directories a checkout offers, as (dir, plugin_name) pairs.
///
/// A repo with .claude-plugin/marketplace.json is read the way Claude Code
/// reads it: each plugin entry with a relative source contributes the
/// skills paths it lists (a listed directory of skills is scanned, a listed
/// skill is taken as-is) or, listing none, its whole skills/ folder.
/// `only_plugins` narrows that to named entries. A repo without a
/// marketplace contributes skills/*/SKILL.md. Also returns the marketplace
/// name ("" for a plain repo) and warnings.
pub fn repo_skill_dirs(
    repo: &Path,
    only_plugins: Option<&Vec<Json>>,
) -> (Vec<(PathBuf, String)>, String, Vec<String>) {
    let mut warnings = Vec::new();
    let repo_name = paths::name(repo);
    let Some(mkt) = read_marketplace(repo) else {
        if let Some(only) = only_plugins {
            warnings.push(format!(
                "{repo_name}: no marketplace.json, so the 'plugins' filter {} is ignored",
                py_repr(&Json::Array(only.clone()))
            ));
        }
        let dirs = scan_skill_dirs(&repo.join("skills"))
            .into_iter()
            .map(|d| (d, String::new()))
            .collect();
        return (dirs, String::new(), warnings);
    };

    let mut entries: Vec<&JMap> = util::arr(mkt.get("plugins"))
        .map(|a| a.iter().filter_map(|e| util::obj(Some(e))).collect())
        .unwrap_or_default();
    if let Some(only) = only_plugins {
        let names: Vec<Json> = entries
            .iter()
            .map(|e| e.get("name").cloned().unwrap_or(Json::Null))
            .collect();
        for want in only {
            if !names.contains(want) {
                warnings.push(format!(
                    "{repo_name}: marketplace has no plugin {}",
                    py_repr(want)
                ));
            }
        }
        entries.retain(|e| only.contains(e.get("name").unwrap_or(&Json::Null)));
    }

    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for e in entries {
        let Some(src) = util::string(e.get("source")) else {
            continue;
        };
        if !src.starts_with("./") {
            continue; // a remote plugin source is not part of this checkout
        }
        let root = paths::resolve(&repo.join(paths::clean(src)));
        let plugin_name = match e.get("name") {
            Some(n) if util::truthy(Some(n)) => util::py_str(n),
            _ => String::new(),
        };
        let mut candidates: Vec<PathBuf> = Vec::new();
        match util::arr(e.get("skills")).filter(|a| !a.is_empty()) {
            Some(listed) => {
                for s in listed {
                    let d = paths::resolve(&root.join(paths::clean(&util::py_str(s))));
                    if d.join("SKILL.md").is_file() {
                        candidates.push(d);
                    } else if d.is_dir() {
                        candidates.extend(scan_skill_dirs(&d));
                    } else {
                        warnings.push(format!(
                            "{repo_name}/{}: listed skill path missing: {}",
                            util::py_get_str(e, "name"),
                            display(&d)
                        ));
                    }
                }
            }
            None => candidates = scan_skill_dirs(&root.join("skills")),
        }
        for d in candidates {
            if !seen.insert(d.clone()) {
                continue;
            }
            out.push((d, plugin_name.clone()));
        }
    }
    let mkt_name = match mkt.get("name") {
        Some(n) if util::truthy(Some(n)) => util::py_str(n),
        _ => repo_name,
    };
    (out, mkt_name, warnings)
}

/// Implicit skill entries from every `skill_repos` checkout.
pub struct RepoSkills {
    pub entries: Vec<Desired>,
    pub warnings: Vec<String>,
    pub missing: Vec<PathBuf>,
    /// (marketplace, plugin) pairs whose skills were taken from a checkout;
    /// the plugin-install path would load those twice.
    pub discovered: BTreeSet<(String, String)>,
    /// dst -> "<repo>" or "<repo>:<plugin>", for audits.
    pub origins: HashMap<PathBuf, String>,
}

/// Manifest entries win on a name collision, then an earlier repo over a
/// later one; the loser is skipped and surfaced in `warnings`.
pub fn collect_repo_skills(
    ctx: &Ctx,
    manifest: &JMap,
    skills_target: &Path,
    taken_dsts: &HashSet<PathBuf>,
) -> RepoSkills {
    let mut r = RepoSkills {
        entries: Vec::new(),
        warnings: Vec::new(),
        missing: Vec::new(),
        discovered: BTreeSet::new(),
        origins: HashMap::new(),
    };
    let mut claimed = taken_dsts.clone();
    for spec in util::arr_or_empty(manifest, "skill_repos") {
        let spec: JMap = match spec {
            Json::String(s) => {
                let mut m = JMap::new();
                m.insert("path".to_string(), Json::String(s));
                m
            }
            Json::Object(m) => m,
            _ => die("manifest 'skill_repos' entries must be objects or strings"),
        };
        let repo = ctx.expand(&util::py_get_str(&spec, "path"));
        if !repo.is_dir() {
            r.missing.push(repo);
            continue;
        }
        let only = util::arr(spec.get("plugins")).filter(|a| !a.is_empty());
        let (dirs, mkt_name, w) = repo_skill_dirs(&repo, only);
        r.warnings.extend(w);
        let repo_name = paths::name(&repo);
        for (d, plugin) in dirs {
            if !mkt_name.is_empty() && !plugin.is_empty() {
                r.discovered.insert((mkt_name.clone(), plugin.clone()));
            }
            let dname = paths::name(&d);
            let dst = skills_target.join(&dname);
            if claimed.contains(&dst) {
                r.warnings.push(format!(
                    "{repo_name}/{dname}: already defined at {}, skipped",
                    display(&dst)
                ));
                continue;
            }
            claimed.insert(dst.clone());
            let origin = if plugin.is_empty() {
                repo_name.clone()
            } else {
                format!("{repo_name}:{plugin}")
            };
            r.origins.insert(dst.clone(), origin);
            r.entries.push(Desired {
                label: dname,
                src: d,
                dst,
            });
        }
    }
    r
}

/// The `description:` of a SKILL.md, folded scalars flattened; "" if none.
pub fn frontmatter_description(skill_dir: &Path) -> String {
    let Ok(bytes) = fs::read(skill_dir.join("SKILL.md")) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    if !text.starts_with("---") {
        return String::new();
    }
    let fm = match text[3..].find("\n---") {
        Some(end) => &text[3..3 + end],
        None => &text[3..],
    };
    let mut out: Vec<String> = Vec::new();
    let mut grab = false;
    for ln in fm.lines() {
        if grab {
            if ln.starts_with(' ') || ln.starts_with('\t') {
                out.push(ln.trim().to_string());
                continue;
            }
            break;
        }
        if let Some(rest) = ln.strip_prefix("description:") {
            let val = rest.trim();
            if matches!(val, ">" | ">-" | ">+" | "|" | "|-" | "|+") {
                grab = true;
            } else {
                out.push(val.trim_matches(|c| c == '"' || c == '\'').to_string());
                break;
            }
        }
    }
    out.join(" ")
}

/// Directory names under `<root>/skills` (any directory), sorted, whose
/// resolved path is not in `linked`.
pub fn unlinked_repo_skill_dirs(root: &Path, linked: &[PathBuf], skills_only: bool) -> Vec<String> {
    let repo_skills = root.join("skills");
    let children: Vec<PathBuf> = if skills_only {
        scan_skill_dirs(&repo_skills)
    } else {
        match fs::read_dir(&repo_skills) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.path())
                .filter(|c| c.is_dir())
                .collect(),
            Err(_) => Vec::new(),
        }
    };
    let mut out: Vec<String> = children
        .iter()
        .filter(|c| !linked.contains(&paths::resolve(c)))
        .map(|c| paths::name(c))
        .collect();
    out.sort();
    out
}
