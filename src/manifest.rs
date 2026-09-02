// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Manifests: loading with the forward-only schema upgrade, listing,
//! rewriting at the current schema, and the --add-skill / --remove-skill
//! edits.

use crate::ctx::{Ctx, MANIFEST_SCHEMA, VERSION};
use crate::paths;
use crate::proc;
use crate::util::{self, die, display, py_str, JMap};
use serde_json::{json, Value as Json};
use std::fs;
use std::path::PathBuf;

/// Upgrade a manifest to MANIFEST_SCHEMA in memory.
///
/// Returns (manifest, notes) where notes describe each step taken. Exits
/// on a manifest newer than this tool understands: the upgrade path is
/// forward-only, so the fix is a newer tool, never a hand-edited manifest.
pub fn migrate_manifest(name: &str, mut m: JMap) -> (JMap, Vec<String>) {
    let v = m.get("schema").cloned().unwrap_or_else(|| json!(1));
    let schema = match &v {
        Json::Number(n) => n.as_i64(),
        _ => None,
    };
    let schema = match schema {
        Some(n) if n <= MANIFEST_SCHEMA => n,
        _ => die(format!(
            "manifest '{name}' is schema {}; qbranch {VERSION} understands up to schema {MANIFEST_SCHEMA} — update the tool",
            py_str(&v)
        )),
    };
    let mut notes = Vec::new();
    if schema < 2 {
        if !m.contains_key("skill_repos") {
            m.insert("skill_repos".to_string(), json!([]));
        }
        notes.push(
            "schema 1 -> 2: skill_repos added (empty); list the checkouts whose skills should be linked in bulk"
                .to_string(),
        );
    }
    if !notes.is_empty() {
        let mut out = JMap::new();
        out.insert("schema".to_string(), json!(MANIFEST_SCHEMA));
        for (k, v) in m {
            if k != "schema" {
                out.insert(k, v);
            }
        }
        m = out;
    }
    (m, notes)
}

/// Load manifests/<name>.json upgraded to the current schema.
///
/// Returns (manifest, upgrade_notes); notes are empty for a current one.
pub fn load_manifest(ctx: &Ctx, name: &str) -> (JMap, Vec<String>) {
    let p = ctx.manifest_path(name);
    if !p.exists() {
        let avail = list_manifests(ctx);
        let avail = if avail.is_empty() {
            "(none)".to_string()
        } else {
            avail.join(", ")
        };
        die(format!(
            "manifest not found: {}\navailable: {}",
            display(&p),
            avail
        ));
    }
    let m = util::read_json_object(&p).unwrap_or_else(|e| die(format!("{}: {}", display(&p), e)));
    migrate_manifest(name, m)
}

/// Rewrite every manifest at MANIFEST_SCHEMA. Never downgrades.
pub fn upgrade_manifests(ctx: &Ctx) -> i32 {
    for name in list_manifests(ctx) {
        let p = ctx.manifest_path(&name);
        let raw =
            util::read_json_object(&p).unwrap_or_else(|e| die(format!("{}: {}", display(&p), e)));
        let before = raw.clone();
        let (upgraded, notes) = migrate_manifest(&name, raw);
        if upgraded == before {
            let v = upgraded
                .get("schema")
                .map(py_str)
                .unwrap_or_else(|| "1".to_string());
            println!("  up to date  {name} (schema {v})");
            continue;
        }
        if let Err(e) = util::write_json(&p, &Json::Object(upgraded)) {
            die(format!("{}: {}", display(&p), e));
        }
        println!("  upgraded    {name}: {}", notes.join("; "));
    }
    println!(
        "\nCommit the rewritten manifests. A qbranch older than the schema they now carry refuses them."
    );
    0
}

pub fn list_manifests(ctx: &Ctx) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(ctx.manifests_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "json").unwrap_or(false) {
                if let Some(stem) = p.file_stem() {
                    out.push(stem.to_string_lossy().into_owned());
                }
            }
        }
    }
    out.sort();
    out
}

/// Extract 'paniolo' from 'git@github.com:user/paniolo.git'.
pub fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    name.strip_suffix(".git").unwrap_or(name).to_string()
}

fn read_manifest_raw(ctx: &Ctx, manifest_name: &str) -> (PathBuf, JMap) {
    let p = ctx.manifest_path(manifest_name);
    let m = util::read_json_object(&p).unwrap_or_else(|e| die(format!("{}: {}", display(&p), e)));
    (p, m)
}

fn skills_of(m: &JMap) -> Vec<Json> {
    util::arr_or_empty(m, "skills")
}

fn entry_name(e: &Json) -> Option<&str> {
    util::obj(Some(e)).and_then(|m| util::string(m.get("name")))
}

/// Add skill_name to manifest. Returns true if the manifest was modified.
pub fn add_skill_to_manifest(
    ctx: &Ctx,
    manifest_name: &str,
    skill_name: &str,
    repo: Option<&str>,
    skill_path: Option<&str>,
) -> bool {
    let (p, mut manifest) = read_manifest_raw(ctx, manifest_name);
    let mut skills = skills_of(&manifest);
    if skills.iter().any(|e| entry_name(e) == Some(skill_name)) {
        return false;
    }
    let entry = match repo {
        Some(url) => json!({
            "name": skill_name,
            "repo": url,
            "path": skill_path.map(str::to_string).unwrap_or_else(|| format!("skills/{skill_name}")),
        }),
        None => json!({
            "name": skill_name,
            "path": format!("${{QBRANCH_ROOT}}/skills/{skill_name}"),
        }),
    };
    skills.push(entry);
    manifest.insert("skills".to_string(), Json::Array(skills));
    if let Err(e) = util::write_json(&p, &Json::Object(manifest)) {
        die(format!("{}: {}", display(&p), e));
    }
    true
}

/// Remove skill_name from manifest. Returns true if the manifest was modified.
pub fn remove_skill_from_manifest(ctx: &Ctx, manifest_name: &str, skill_name: &str) -> bool {
    let (p, mut manifest) = read_manifest_raw(ctx, manifest_name);
    let before = skills_of(&manifest);
    let after: Vec<Json> = before
        .iter()
        .filter(|e| entry_name(e) != Some(skill_name))
        .cloned()
        .collect();
    if after.len() == before.len() {
        return false;
    }
    manifest.insert("skills".to_string(), Json::Array(after));
    if let Err(e) = util::write_json(&p, &Json::Object(manifest)) {
        die(format!("{}: {}", display(&p), e));
    }
    true
}

/// Parse a git:// shorthand into (skill_name, repo_url, skill_path).
///
/// Short form `git://<local-name>[/<path>]` (first segment has no dot) looks
/// up ~/src/<name> for the remote URL. Full form
/// `git://<host>/<owner>/<repo>[/<path>]` constructs git@host:owner/repo.git.
pub fn parse_git_skill_url(ctx: &Ctx, arg: &str) -> (String, String, String) {
    let rest = &arg["git://".len()..];
    let parts: Vec<&str> = rest.split('/').collect();
    if !parts[0].contains('.') {
        let repo_name = parts[0];
        let path_parts = &parts[1..];
        let local = ctx.home.join("src").join(repo_name);
        if !local.is_dir() || !local.join(".git").exists() {
            die(format!(
                "git:// short form: ~/src/{repo_name} not found or not a git repo\nuse full form: git://<host>/<owner>/<repo>[/<path>]"
            ));
        }
        let argv: Vec<String> = ["git", "-C", &display(&local), "remote", "get-url", "origin"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = proc::run_capture(&argv, None).unwrap_or_else(|e| {
            die(format!(
                "could not get remote URL for ~/src/{repo_name}: {e}"
            ))
        });
        if !out.ok() {
            die(format!(
                "could not get remote URL for ~/src/{repo_name}: {}",
                out.stderr.trim()
            ));
        }
        let repo_url = out.stdout.trim().to_string();
        let skill_path = if path_parts.is_empty() {
            format!("skills/{repo_name}")
        } else {
            path_parts.join("/")
        };
        let skill_name = path_parts
            .last()
            .map(|s| s.to_string())
            .unwrap_or_else(|| repo_name.to_string());
        return (skill_name, repo_url, skill_path);
    }
    if parts.len() < 3 {
        die(format!(
            "invalid git:// skill URL: '{arg}'\nexpected: git://<host>/<owner>/<repo>[/<path/to/skill>]"
        ));
    }
    let (host, owner, repo_part) = (parts[0], parts[1], parts[2]);
    let path_parts = &parts[3..];
    let repo_name = repo_part.strip_suffix(".git").unwrap_or(repo_part);
    let repo_url = format!("git@{host}:{owner}/{repo_name}.git");
    let skill_path = if path_parts.is_empty() {
        format!("skills/{repo_name}")
    } else {
        path_parts.join("/")
    };
    let skill_name = path_parts
        .last()
        .map(|s| s.to_string())
        .unwrap_or_else(|| repo_name.to_string());
    (skill_name, repo_url, skill_path)
}

/// `~/src/<name>` for a repo URL, the conventional local checkout.
pub fn local_checkout(ctx: &Ctx, repo_url: &str) -> PathBuf {
    ctx.home.join("src").join(repo_name_from_url(repo_url))
}

/// The `skills` entries that are path-based, each source resolved.
pub fn manifest_skill_srcs(ctx: &Ctx, manifest: &JMap) -> Vec<PathBuf> {
    skills_of(manifest)
        .iter()
        .filter_map(|e| util::obj(Some(e)))
        .filter(|e| !e.contains_key("repo"))
        .map(|e| paths::resolve(&ctx.expand(&util::py_get_str(e, "path"))))
        .collect()
}
