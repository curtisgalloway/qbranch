// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! The state file: what the last sync linked, which config root and
//! manifest it used, and the settings policy it applied.

use crate::ctx::{Ctx, LEGACY_STATE_FILE_NAME, STATE_FILE_NAME, STATE_SCHEMA};
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
    m
}

/// Load sync state, falling back to the legacy ~/.claude/skills location.
pub fn load_state(ctx: &Ctx, state_path: &Path) -> JMap {
    let mut path = state_path.to_path_buf();
    if !path.exists() {
        let mut candidates = vec![paths::parent(state_path).join(LEGACY_STATE_FILE_NAME)];
        if !paths::is_symlink(&ctx.claude_skills_link) {
            candidates.push(ctx.claude_skills_link.join(STATE_FILE_NAME));
            candidates.push(ctx.claude_skills_link.join(LEGACY_STATE_FILE_NAME));
        }
        for legacy in candidates {
            if legacy.is_file() {
                path = legacy;
                break;
            }
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
    if data.contains_key("names") && !data.contains_key("links") {
        let skills_target = paths::parent(&path);
        let links: Vec<Json> = util::arr_or_empty(&data, "names")
            .iter()
            .map(|n| Json::String(display(&skills_target.join(util::py_str(n)))))
            .collect();
        data.insert("links".to_string(), Json::Array(links));
    }
    if !data.contains_key("links") {
        data.insert("links".to_string(), json!([]));
    }
    data
}

/// Use the config root the last sync recorded, unless one was given.
///
/// The root is passed once (--root or QBRANCH_ROOT) and remembered in the
/// state file, so a plain `qbranch` afterwards knows where the manifests
/// are even though the tool no longer lives inside the config repo.
pub fn adopt_remembered_root(ctx: &mut Ctx, explicit: bool, state: &JMap) {
    if explicit
        || env::var("QBRANCH_ROOT")
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    {
        return;
    }
    if let Some(remembered) = util::string(state.get("root")).filter(|s| !s.is_empty()) {
        let root = paths::clean(remembered);
        if root.join("manifests").is_dir() {
            ctx.repo = root;
        }
    }
}

/// The links recorded by the last sync, as paths.
pub fn previous_links(state: &JMap) -> Vec<PathBuf> {
    util::arr_or_empty(state, "links")
        .iter()
        .map(|p| paths::clean(&util::py_str(p)))
        .collect()
}

/// Write the state file. `settings_applied` maps state key -> policy.
pub fn save_state(
    ctx: &Ctx,
    state_path: &Path,
    manifest_name: &str,
    links: &[PathBuf],
    settings_applied: &BTreeMap<String, Option<JMap>>,
) {
    let mut link_strs: Vec<String> = links.iter().map(|p| display(p)).collect();
    link_strs.sort();
    link_strs.dedup();
    let mut out = JMap::new();
    out.insert("schema".to_string(), json!(STATE_SCHEMA));
    out.insert("manifest".to_string(), json!(manifest_name));
    out.insert("root".to_string(), json!(display(&ctx.repo)));
    out.insert("linked_at".to_string(), json!(util::utc_now_iso()));
    out.insert("links".to_string(), json!(link_strs));
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
