// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Settings fragments: merge them into a policy, assert the policy into the
//! app-owned settings file, and retract what an earlier policy asserted
//! that the current one no longer does.

use crate::ctx::Ctx;
use crate::paths;
use crate::util::{self, die, display, JMap};
use serde_json::Value as Json;
use std::fs;
use std::path::{Path, PathBuf};

/// Merge a settings fragment onto base policy.
///
/// Dicts merge recursively, lists union (base order first), scalars from
/// the overlay win.
pub fn merge_fragments(base: &JMap, overlay: &JMap) -> JMap {
    let mut out = base.clone();
    for (k, v) in overlay {
        let merged = match (out.get(k), v) {
            (Some(Json::Object(cur)), Json::Object(ov)) => Json::Object(merge_fragments(cur, ov)),
            (Some(Json::Array(cur)), Json::Array(ov)) => {
                let mut items = cur.clone();
                for i in ov {
                    if !items.contains(i) {
                        items.push(i.clone());
                    }
                }
                Json::Array(items)
            }
            _ => v.clone(),
        };
        out.insert(k.clone(), merged);
    }
    out
}

/// Impose policy onto live settings in place.
///
/// Policy scalars overwrite, policy list items are appended if missing
/// (app-added items are kept), dicts recurse. Keys the policy doesn't
/// mention are left alone: they belong to the app.
pub fn assert_settings(live: &mut JMap, policy: &JMap) {
    for (k, v) in policy {
        match (live.get_mut(k), v) {
            (Some(Json::Object(cur)), Json::Object(pv)) => assert_settings(cur, pv),
            (Some(Json::Array(cur)), Json::Array(pv)) => {
                for item in pv {
                    if !cur.contains(item) {
                        cur.push(item.clone());
                    }
                }
            }
            _ => {
                live.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Remove what the old policy asserted but the new one no longer does.
///
/// List items are removed exactly; scalars only if the live value still
/// equals what old policy set (a locally changed value is app state now);
/// dicts recurse and are pruned when emptied.
pub fn retract_settings(live: &mut JMap, old: &JMap, new: &JMap) {
    let empty_map = JMap::new();
    let empty_list: Vec<Json> = Vec::new();
    for (k, old_v) in old {
        let new_v = new.get(k);
        let mut remove = false;
        match live.get_mut(k) {
            None => continue,
            Some(live_v) => match (old_v, live_v) {
                (Json::Object(ov), Json::Object(lv)) => {
                    let nv = match new_v {
                        Some(Json::Object(n)) => n,
                        _ => &empty_map,
                    };
                    retract_settings(lv, ov, nv);
                    if lv.is_empty() && !matches!(new_v, Some(Json::Object(_))) {
                        remove = true;
                    }
                }
                (Json::Array(ov), Json::Array(lv)) => {
                    let new_items = match new_v {
                        Some(Json::Array(n)) => n,
                        _ => &empty_list,
                    };
                    let gone: Vec<&Json> = ov.iter().filter(|i| !new_items.contains(i)).collect();
                    lv.retain(|i| !gone.contains(&i));
                    if lv.is_empty() && new_items.is_empty() {
                        remove = true;
                    }
                }
                (_, live_v) => {
                    if !new.contains_key(k) && *live_v == *old_v {
                        remove = true;
                    }
                }
            },
        }
        if remove {
            live.remove(k);
        }
    }
}

/// Read settings fragments in manifest order. Returns (loaded, failures).
pub fn load_fragments(ctx: &Ctx, raw_paths: &[Json]) -> (Vec<(PathBuf, JMap)>, Vec<String>) {
    let mut loaded = Vec::new();
    let mut failures = Vec::new();
    for raw in raw_paths {
        let p = ctx.expand(&util::py_str(raw));
        if !p.is_file() {
            failures.push(format!("settings fragment missing: {}", display(&p)));
            continue;
        }
        match util::read_json_object(&p) {
            Ok(m) => loaded.push((p, m)),
            Err(e) => failures.push(format!(
                "settings fragment unparseable: {}: {e}",
                display(&p)
            )),
        }
    }
    (loaded, failures)
}

pub struct SettingsOutcome {
    pub notes: Vec<String>,
    /// The applied policy for the state file; None when the merge failed.
    pub applied: Option<JMap>,
    pub failures: Vec<String>,
}

/// Merge `manifest_key` fragments into the app-owned settings file `target`.
///
/// The live file is never symlinked: the app owns it and freely rewrites
/// it (theme, effortLevel, plugins, ...). This asserts the policy keys
/// from the fragments into it and retracts whatever the previous sync's
/// policy asserted that the current one no longer does. The applied
/// policy snapshot is returned for the state file under `state_key`.
#[allow(clippy::too_many_arguments)]
pub fn sync_settings(
    ctx: &Ctx,
    agent: &str,
    manifest_key: &str,
    target: &Path,
    state_key: &str,
    manifest: &JMap,
    state: &JMap,
    dry_run: bool,
) -> SettingsOutcome {
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    let fragments = util::arr_or_empty(manifest, manifest_key);
    let old_applied = util::obj_or_empty(state, state_key);

    let (loaded, load_failures) = load_fragments(ctx, &fragments);
    if !load_failures.is_empty() {
        failures.extend(load_failures);
        return SettingsOutcome {
            notes,
            applied: None,
            failures,
        };
    }
    let mut policy = JMap::new();
    for (_, frag) in &loaded {
        policy = merge_fragments(&policy, frag);
    }

    if fragments.is_empty() && old_applied.is_empty() {
        return SettingsOutcome {
            notes,
            applied: Some(JMap::new()),
            failures,
        };
    }

    let mut live: JMap;
    if paths::is_symlink(target) {
        // Legacy layout: the live file was a symlink into the repo. Capture
        // its content into a real, app-owned file before merging into it.
        let content = if target.exists() {
            fs::read_to_string(paths::resolve(target))
                .unwrap_or_else(|e| die(format!("{}: {e}", display(target))))
        } else {
            "{}\n".to_string()
        };
        notes.push(format!(
            "{agent} settings: converting symlink -> real file  {}",
            display(target)
        ));
        if !dry_run {
            if let Err(e) = paths::unlink(target).and_then(|_| fs::write(target, &content)) {
                die(format!("{}: {e}", display(target)));
            }
        }
        live = match serde_json::from_str(&content) {
            Ok(Json::Object(m)) => m,
            Ok(other) => die(format!(
                "{}: expected a JSON object, got {}",
                display(target),
                util::kind(&other)
            )),
            Err(e) => die(format!("{}: {e}", display(target))),
        };
    } else if target.is_file() {
        match util::read_json_object(target) {
            Ok(m) => live = m,
            Err(e) => {
                failures.push(format!(
                    "live settings unparseable, leaving alone: {}: {e}",
                    display(target)
                ));
                return SettingsOutcome {
                    notes,
                    applied: None,
                    failures,
                };
            }
        }
    } else {
        live = JMap::new();
    }

    let before = live.clone();
    retract_settings(&mut live, &old_applied, &policy);
    assert_settings(&mut live, &policy);
    if live != before || !target.exists() {
        let verb = if dry_run { "would update" } else { "updated" };
        notes.push(format!(
            "{agent} settings: {verb} {} from {} fragment(s)",
            display(target),
            fragments.len()
        ));
        if !dry_run {
            if let Some(parent) = target.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    die(format!("{}: {e}", display(parent)));
                }
            }
            if let Err(e) = util::write_json(target, &Json::Object(live)) {
                die(format!("{}: {e}", display(target)));
            }
        }
    } else {
        notes.push(format!(
            "{agent} settings: {} up to date ({} fragment(s))",
            display(target),
            fragments.len()
        ));
    }
    SettingsOutcome {
        notes,
        applied: Some(policy),
        failures,
    }
}
