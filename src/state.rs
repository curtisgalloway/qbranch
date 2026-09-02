// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! The state file: what the last sync linked or copied, which config root,
//! manifest and link mode it used, and the settings policy it applied. Also
//! the run-level choices that read it: the config root and the manifest.

use crate::ctx::{Ctx, LEGACY_STATE_FILE_NAME, STATE_SCHEMA};
use crate::manifest::list_manifests;
use crate::paths;
use crate::util::{self, die, display, JMap};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

fn empty_state() -> JMap {
    let mut m = JMap::new();
    m.insert("manifest".to_string(), Json::Null);
    m.insert("links".to_string(), json!([]));
    m.insert("copies".to_string(), json!([]));
    m
}

/// Load sync state; an older sync's .agent-skills-state.json beside it counts.
pub fn load_state(state_path: &Path) -> JMap {
    let mut path = state_path.to_path_buf();
    if !path.exists() {
        let legacy = paths::parent(state_path).join(LEGACY_STATE_FILE_NAME);
        if legacy.is_file() {
            path = legacy;
        }
    }
    if !path.exists() {
        return empty_state();
    }
    let mut data = match util::read_json_object(&path) {
        Ok(m) => m,
        Err(_) => return empty_state(),
    };
    let schema = data.get("schema").and_then(Json::as_i64).unwrap_or(1);
    if schema > STATE_SCHEMA {
        die(format!(
            "{} was written by a newer qbranch (state schema {}, this tool: {}) — update the tool",
            display(&path),
            schema,
            STATE_SCHEMA
        ));
    }
    for key in ["links", "copies"] {
        if !data.contains_key(key) {
            data.insert(key.to_string(), json!([]));
        }
    }
    data
}

/// Pick the config root for this run and store it in the context.
///
/// --root wins, then $QBRANCH_ROOT, then the root the last sync remembered
/// in the state file, then the current directory when it holds manifests/.
/// A root given explicitly must hold manifests/; a remembered one that no
/// longer does is skipped.
pub fn resolve_root(ctx: &mut Ctx, explicit: Option<&str>, state: &JMap) {
    let mut candidates: Vec<(&str, PathBuf)> = Vec::new();
    if let Some(root) = explicit.filter(|s| !s.is_empty()) {
        candidates.push(("--root", paths::clean(root)));
    }
    if let Some(root) = env::var("QBRANCH_ROOT").ok().filter(|s| !s.is_empty()) {
        candidates.push(("QBRANCH_ROOT", paths::clean(&root)));
    }
    if let Some(root) = util::string(state.get("root")).filter(|s| !s.is_empty()) {
        candidates.push(("remembered", paths::clean(root)));
    }
    candidates.push((
        "cwd",
        env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    ));
    for (how, raw) in candidates {
        let p = paths::resolve(&paths::expanduser(&raw, &ctx.home));
        if p.join("manifests").is_dir() {
            ctx.repo = p;
            return;
        }
        if how == "--root" || how == "QBRANCH_ROOT" {
            die(format!("no manifests/ under {} ({how})", display(&p)));
        }
    }
    die("no config root: pass --root <dir> (remembered afterwards), set QBRANCH_ROOT, or run from a directory that holds manifests/");
}

/// --manifest, else the last synced one, else this host's, else `default`.
pub fn choose_manifest(ctx: &Ctx, explicit: Option<&str>, state: &JMap) -> String {
    if let Some(name) = explicit.filter(|s| !s.is_empty()) {
        return name.to_string();
    }
    if let Some(name) = util::string(state.get("manifest")).filter(|s| !s.is_empty()) {
        return name.to_string();
    }
    let available = list_manifests(ctx);
    for candidate in [paths::short_hostname(), "default".to_string()] {
        if available.contains(&candidate) {
            return candidate;
        }
    }
    die(format!(
        "no manifest chosen: pass --manifest <name> (remembered afterwards); available: {}",
        if available.is_empty() {
            "(none)".to_string()
        } else {
            available.join(", ")
        }
    ));
}

fn path_list(state: &JMap, key: &str) -> Vec<PathBuf> {
    util::arr_or_empty(state, key)
        .iter()
        .map(|p| paths::clean(&util::py_str(p)))
        .collect()
}

/// The links (and copies) recorded by the last sync, as paths.
pub fn previous_links(state: &JMap) -> Vec<PathBuf> {
    path_list(state, "links")
}

/// The destinations the last sync materialised as copies.
pub fn previous_copies(state: &JMap) -> Vec<PathBuf> {
    path_list(state, "copies")
}

fn sorted_unique(paths: &[PathBuf]) -> Vec<String> {
    let mut out: Vec<String> = paths.iter().map(|p| display(p)).collect();
    out.sort();
    out.dedup();
    out
}

/// Write the state file. `settings_applied` maps state key -> policy.
pub fn save_state(
    ctx: &Ctx,
    state_path: &Path,
    manifest_name: &str,
    links: &[PathBuf],
    copies: &[PathBuf],
    link_mode: Option<&str>,
    settings_applied: &BTreeMap<String, Option<JMap>>,
) {
    let mut out = JMap::new();
    out.insert("schema".to_string(), json!(STATE_SCHEMA));
    out.insert("manifest".to_string(), json!(manifest_name));
    out.insert("root".to_string(), json!(display(&ctx.repo)));
    out.insert("link_mode".to_string(), json!(link_mode));
    out.insert("linked_at".to_string(), json!(util::utc_now_iso()));
    out.insert("links".to_string(), json!(sorted_unique(links)));
    out.insert("copies".to_string(), json!(sorted_unique(copies)));
    for (k, v) in settings_applied {
        out.insert(k.clone(), Json::Object(v.clone().unwrap_or_default()));
    }
    if let Err(e) = util::write_json(state_path, &Json::Object(out)) {
        die(format!("{}: {}", display(state_path), e));
    }
    let legacy = paths::parent(state_path).join(LEGACY_STATE_FILE_NAME);
    if legacy.is_file() {
        let _ = std::fs::remove_file(&legacy);
    }
}
