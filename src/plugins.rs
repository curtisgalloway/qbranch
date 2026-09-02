// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! Claude Code plugins, read and changed only through the `claude plugin`
//! CLI: the reconcile plan, the managed / unmanaged / dropped status
//! report, and --manage-plugin.

use crate::ctx::{official_marketplace_source, Ctx, OFFICIAL_MARKETPLACE};
use crate::paths;
use crate::proc;
use crate::settings::load_fragments;
use crate::util::{self, die, display, py_dumps, JMap};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct PluginAction {
    pub op: &'static str,
    pub label: String,
    pub argv: Vec<String>,
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Run `claude <args> --json` and parse its output as a list.
pub fn claude_cli_json(args: &[&str]) -> Result<Vec<Json>, String> {
    let mut cmd = argv(&["claude"]);
    cmd.extend(args.iter().map(|s| s.to_string()));
    cmd.push("--json".to_string());
    let shown = cmd.join(" ");
    let out = proc::run_capture(&cmd, Some(Duration::from_secs(120)))
        .map_err(|e| format!("{shown}: {e}"))?;
    if !out.ok() {
        let detail = if out.stderr.trim().is_empty() {
            out.stdout.trim()
        } else {
            out.stderr.trim()
        };
        return Err(format!("{shown}: exit {}: {detail}", out.code_str()));
    }
    match serde_json::from_str::<Json>(&out.stdout) {
        Ok(Json::Array(a)) => Ok(a),
        Ok(_) => Ok(Vec::new()),
        Err(e) => Err(format!("{shown}: unparseable output: {e}")),
    }
}

/// Turn an extraKnownMarketplaces entry into a `marketplace add` argument.
pub fn marketplace_add_spec(entry: &Json) -> Option<String> {
    let src = util::obj(Some(entry))
        .and_then(|e| util::obj(e.get("source")).cloned())
        .unwrap_or_default();
    if util::string(src.get("source")) == Some("github") && util::truthy(src.get("repo")) {
        return Some(util::py_str(&src["repo"]));
    }
    for k in ["url", "path"] {
        if util::truthy(src.get(k)) {
            return Some(util::py_str(&src[k]));
        }
    }
    None
}

pub fn plugin_marketplace_of(pid: &str) -> String {
    match pid.rfind('@') {
        Some(i) => pid[i + 1..].to_string(),
        None => String::new(),
    }
}

fn user_rows(installed: &[Json]) -> Vec<&JMap> {
    installed
        .iter()
        .filter_map(|p| util::obj(Some(p)))
        .filter(|p| util::string(p.get("scope")) == Some("user"))
        .collect()
}

fn id_of(row: &JMap) -> Option<String> {
    util::string(row.get("id")).map(str::to_string)
}

fn true_ids(enabled: &JMap) -> BTreeSet<String> {
    enabled
        .iter()
        .filter(|(_, on)| **on == Json::Bool(true))
        .map(|(pid, _)| pid.clone())
        .collect()
}

/// Plan the `claude plugin` calls that bring local plugins up to policy.
///
/// `extraKnownMarketplaces` in the merged fragments names marketplaces to
/// register; `enabledPlugins` entries set to true name plugins to install
/// at user scope. Only additions are acted on. A plugin dropped from
/// policy has already had its enabledPlugins entry retracted by the
/// settings merge, and Claude Code treats an installed plugin with no
/// entry as disabled, so it is reported rather than uninstalled:
/// uninstalling also deletes the plugin's data directory, which is the
/// user's call.
pub fn plan_claude_plugins(
    policy: &JMap,
    old_policy: &JMap,
    discovered: &BTreeSet<(String, String)>,
) -> (Vec<PluginAction>, Vec<String>, Vec<String>) {
    let mut actions = Vec::new();
    let mut notes = Vec::new();
    let mut failures = Vec::new();

    let mut want_mkts = util::obj_or_empty(policy, "extraKnownMarketplaces");
    let enabled = util::obj_or_empty(policy, "enabledPlugins");
    let want = true_ids(&enabled);
    let old_want = true_ids(&util::obj_or_empty(old_policy, "enabledPlugins"));
    if want_mkts.is_empty() && want.is_empty() && old_want.is_empty() {
        return (actions, notes, failures);
    }
    if paths::which("claude").is_none() {
        notes.push("Claude Code plugins: skipped (claude not on PATH)".to_string());
        return (actions, notes, failures);
    }

    let known = match claude_cli_json(&["plugin", "marketplace", "list"]) {
        Ok(k) => k,
        Err(e) => {
            failures.push(format!("plugins: {e}"));
            return (actions, notes, failures);
        }
    };
    let installed = match claude_cli_json(&["plugin", "list"]) {
        Ok(i) => i,
        Err(e) => {
            failures.push(format!("plugins: {e}"));
            return (actions, notes, failures);
        }
    };
    let known_names: HashSet<String> = known
        .iter()
        .filter_map(|m| {
            util::obj(Some(m))
                .and_then(|m| util::string(m.get("name")))
                .map(str::to_string)
        })
        .collect();
    let rows = user_rows(&installed);
    let installed_ids: BTreeSet<String> = rows.iter().filter_map(|p| id_of(p)).collect();
    let enabled_ids: BTreeSet<String> = rows
        .iter()
        .filter(|p| util::truthy(p.get("enabled")))
        .filter_map(|p| id_of(p))
        .collect();

    // A plugin whose skills are already linked from a checkout would load
    // them a second time if it is also installed and enabled.
    let twice: BTreeSet<String> = discovered
        .iter()
        .map(|(mkt, plugin)| format!("{plugin}@{mkt}"))
        .filter(|pid| enabled_ids.contains(pid))
        .collect();
    if !twice.is_empty() {
        notes.push(format!(
            "double load: installed and enabled here while their skills are also linked from a skill_repos checkout — uninstall the plugin or narrow the repo's 'plugins' list: {}",
            twice.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }

    // The official marketplace is implied by any plugin that lives in it.
    let official_suffix = format!("@{OFFICIAL_MARKETPLACE}");
    if want.iter().any(|pid| pid.ends_with(&official_suffix))
        && !want_mkts.contains_key(OFFICIAL_MARKETPLACE)
    {
        want_mkts.insert(
            OFFICIAL_MARKETPLACE.to_string(),
            official_marketplace_source(),
        );
    }

    for (name, entry) in &want_mkts {
        if known_names.contains(name) {
            continue;
        }
        match marketplace_add_spec(entry) {
            Some(spec) => actions.push(PluginAction {
                op: "add-mkt",
                label: name.clone(),
                argv: argv(&["claude", "plugin", "marketplace", "add", &spec]),
            }),
            None => failures.push(format!(
                "plugins: marketplace {name}: unsupported source {}",
                py_dumps(entry)
            )),
        }
    }

    for pid in want.difference(&installed_ids) {
        let mkt = plugin_marketplace_of(pid);
        if !mkt.is_empty() && !known_names.contains(&mkt) && !want_mkts.contains_key(&mkt) {
            failures.push(format!(
                "plugins: {pid}: marketplace '{mkt}' is not registered — add it to extraKnownMarketplaces in a fragment"
            ));
            continue;
        }
        actions.push(PluginAction {
            op: "install",
            label: pid.clone(),
            argv: argv(&["claude", "plugin", "install", pid, "--scope", "user"]),
        });
    }

    let retracted: Vec<String> = old_want
        .difference(&want)
        .filter(|pid| installed_ids.contains(*pid))
        .cloned()
        .collect();
    if !retracted.is_empty() {
        notes.push(format!(
            "plugins no longer managed (dropped from the fragments; now disabled, still installed; `claude plugin uninstall <id>` to remove): {}",
            retracted.join(", ")
        ));
    }
    let unmanaged: Vec<String> = installed_ids
        .iter()
        .filter(|pid| !enabled.contains_key(*pid) && !retracted.contains(pid))
        .cloned()
        .collect();
    if !unmanaged.is_empty() {
        notes.push(format!(
            "unmanaged plugins (installed here, declared in no fragment): {}",
            unmanaged.join(", ")
        ));
    }
    (actions, notes, failures)
}

/// The manifest's per-host fragment entry (raw, unexpanded), if any.
pub fn host_fragment_of(fragments: &[Json]) -> Option<String> {
    fragments
        .iter()
        .map(util::py_str)
        .find(|r| r.contains("/settings/hosts/"))
}

/// Rebuild an extraKnownMarketplaces entry from a `marketplace list` row.
pub fn marketplace_entry_from_cli(m: Option<&JMap>) -> Option<Json> {
    let m = m?;
    if !util::truthy(m.get("source")) {
        return None;
    }
    let mut src = JMap::new();
    src.insert("source".to_string(), m["source"].clone());
    for k in ["repo", "url", "path"] {
        if util::truthy(m.get(k)) {
            src.insert(k.to_string(), m[k].clone());
        }
    }
    Some(json!({"source": src}))
}

/// Classify this machine's user-scope plugins against the manifest.
///
/// managed:   declared (true or false) by some fragment of the manifest
/// unmanaged: installed here, declared nowhere
/// dropped:   declared at the last sync, not any more, still installed
///
/// Returns (report, failures). The report is what --plugin-status prints
/// and what the review-plugins skill consumes.
pub fn plugin_status(
    ctx: &Ctx,
    manifest_name: &str,
    manifest: &JMap,
    state: &JMap,
) -> (JMap, Vec<String>) {
    let fragments = util::arr_or_empty(manifest, "claude_settings");
    let mut report = JMap::new();
    report.insert("manifest".to_string(), json!(manifest_name));
    report.insert(
        "base_fragment".to_string(),
        match fragments.first() {
            Some(f) => json!(display(&ctx.expand(&util::py_str(f)))),
            None => Json::Null,
        },
    );
    report.insert(
        "host_fragment".to_string(),
        match host_fragment_of(&fragments) {
            Some(raw) => json!(display(&ctx.expand(&raw))),
            None => Json::Null,
        },
    );
    report.insert("managed".to_string(), json!({}));
    report.insert("unmanaged".to_string(), json!([]));
    report.insert("dropped".to_string(), json!([]));

    let (loaded, failures) = load_fragments(ctx, &fragments);
    if !failures.is_empty() {
        return (report, failures);
    }

    let mut declared: BTreeMap<String, (Json, String)> = BTreeMap::new();
    let mut declared_order: Vec<String> = Vec::new();
    let mut declared_mkts: HashSet<String> = HashSet::from([OFFICIAL_MARKETPLACE.to_string()]);
    for (p, frag) in &loaded {
        for (pid, val) in util::obj_or_empty(frag, "enabledPlugins") {
            if !declared.contains_key(&pid) {
                declared_order.push(pid.clone());
            }
            declared.insert(pid, (val, display(p)));
        }
        declared_mkts.extend(
            util::obj_or_empty(frag, "extraKnownMarketplaces")
                .keys()
                .cloned(),
        );
    }

    if paths::which("claude").is_none() {
        return (
            report,
            vec!["claude not on PATH — cannot read installed plugins".to_string()],
        );
    }
    let known = match claude_cli_json(&["plugin", "marketplace", "list"]) {
        Ok(k) => k,
        Err(e) => return (report, vec![e]),
    };
    let installed = match claude_cli_json(&["plugin", "list"]) {
        Ok(i) => i,
        Err(e) => return (report, vec![e]),
    };
    let mkt_by_name: BTreeMap<String, &JMap> = known
        .iter()
        .filter_map(|m| util::obj(Some(m)))
        .filter_map(|m| util::string(m.get("name")).map(|n| (n.to_string(), m)))
        .collect();
    let live: BTreeMap<String, &JMap> = user_rows(&installed)
        .into_iter()
        .filter_map(|p| id_of(p).map(|id| (id, p)))
        .collect();
    let old_policy = util::obj_or_empty(state, "claude_settings_applied");
    let old_want = true_ids(&util::obj_or_empty(&old_policy, "enabledPlugins"));

    let mut managed = JMap::new();
    for pid in &declared_order {
        let (value, fragment) = &declared[pid];
        let row = live.get(pid);
        managed.insert(
            pid.clone(),
            json!({
                "value": value,
                "fragment": fragment,
                "installed": row.is_some(),
                "enabled": row.map(|r| util::truthy(r.get("enabled"))).unwrap_or(false),
            }),
        );
    }
    report.insert("managed".to_string(), Json::Object(managed));
    let mut unmanaged = Vec::new();
    let mut dropped = Vec::new();
    for (pid, row) in &live {
        if declared.contains_key(pid) {
            continue;
        }
        let mkt = plugin_marketplace_of(pid);
        let entry = json!({
            "id": pid,
            "version": row.get("version").cloned().unwrap_or(Json::Null),
            "enabled": util::truthy(row.get("enabled")),
            "marketplace": mkt,
            "marketplace_declared": declared_mkts.contains(&mkt),
            "marketplace_source": marketplace_entry_from_cli(mkt_by_name.get(&mkt).copied()),
        });
        if old_want.contains(pid) {
            dropped.push(entry);
        } else {
            unmanaged.push(entry);
        }
    }
    report.insert("unmanaged".to_string(), Json::Array(unmanaged));
    report.insert("dropped".to_string(), Json::Array(dropped));
    (report, Vec::new())
}

pub fn print_plugin_status(report: &JMap) {
    println!("manifest: {}", util::py_get_str(report, "manifest"));
    println!(
        "base fragment: {}",
        util::py_get_str(report, "base_fragment")
    );
    let host = report.get("host_fragment");
    println!(
        "host fragment: {}",
        if util::truthy(host) {
            util::py_str(host.unwrap())
        } else {
            "(none — --manage-plugin --in host creates one)".to_string()
        }
    );
    let managed = util::obj_or_empty(report, "managed");
    println!("\nmanaged ({}):", managed.len());
    let mut pids: Vec<&String> = managed.keys().collect();
    pids.sort();
    for pid in pids {
        let info = util::obj(managed.get(pid)).cloned().unwrap_or_default();
        let state = if util::truthy(info.get("installed")) {
            "installed"
        } else {
            "NOT INSTALLED"
        };
        let on = if util::truthy(info.get("enabled")) {
            "enabled"
        } else {
            "disabled"
        };
        let value = info
            .get("value")
            .map(py_dumps)
            .unwrap_or_else(|| "null".to_string());
        let fragment = paths::name(Path::new(&util::py_get_str(&info, "fragment")));
        println!("  {pid:<48} {value:<6} {state}, {on}  [{fragment}]");
    }
    for (key, title) in [
        (
            "unmanaged",
            "unmanaged (installed here, declared in no fragment)",
        ),
        (
            "dropped",
            "no longer managed (dropped from the fragments, still installed)",
        ),
    ] {
        let rows = util::arr_or_empty(report, key);
        println!("\n{title} ({}):", rows.len());
        for e in rows {
            let e = util::obj(Some(&e)).cloned().unwrap_or_default();
            let on = if util::truthy(e.get("enabled")) {
                "enabled"
            } else {
                "disabled"
            };
            let mut mkt = util::py_get_str(&e, "marketplace");
            if !util::truthy(e.get("marketplace_declared")) {
                mkt.push_str(" (marketplace undeclared)");
            }
            println!(
                "  {:<48} v{}  {on}  {mkt}",
                util::py_get_str(&e, "id"),
                util::py_get_str(&e, "version")
            );
        }
    }
}

/// Declare pid in the manifest's base or host fragment.
///
/// `where_` is "base" (first fragment: every machine using it) or "host"
/// (the manifest's hosts/ fragment, created and wired into the manifest if
/// missing). A true value for a plugin from a non-official marketplace
/// also declares that marketplace, so other machines can install it.
/// Returns (notes, failures).
pub fn manage_plugin(
    ctx: &Ctx,
    manifest_name: &str,
    pid: &str,
    where_: &str,
    value: bool,
) -> (Vec<String>, Vec<String>) {
    let mut notes = Vec::new();
    let mut failures = Vec::new();
    let mpath = ctx.manifest_path(manifest_name);
    if !mpath.is_file() {
        return (
            notes,
            vec![format!("manifest not found: {}", display(&mpath))],
        );
    }
    let mut manifest = match util::read_json_object(&mpath) {
        Ok(m) => m,
        Err(e) => return (notes, vec![format!("{}: {e}", display(&mpath))]),
    };
    let mut fragments = util::arr_or_empty(&manifest, "claude_settings");
    if fragments.is_empty() {
        return (
            notes,
            vec![format!(
                "manifest {manifest_name} lists no claude_settings fragments"
            )],
        );
    }

    let raw = if where_ == "base" {
        util::py_str(&fragments[0])
    } else {
        match host_fragment_of(&fragments) {
            Some(r) => r,
            None => {
                let r =
                    format!("${{QBRANCH_ROOT}}/claude-code/settings/hosts/{manifest_name}.json");
                fragments.push(json!(r));
                manifest.insert(
                    "claude_settings".to_string(),
                    Json::Array(fragments.clone()),
                );
                if let Err(e) = util::write_json(&mpath, &Json::Object(manifest.clone())) {
                    die(format!("{}: {e}", display(&mpath)));
                }
                notes.push(format!("added {r} to manifest {manifest_name}"));
                r
            }
        }
    };
    let fpath = ctx.expand(&raw);
    let mut frag = if fpath.is_file() {
        util::read_json_object(&fpath).unwrap_or_else(|e| die(format!("{}: {e}", display(&fpath))))
    } else {
        JMap::new()
    };
    let other_raws: Vec<Json> = fragments
        .iter()
        .filter(|r| util::py_str(r) != raw)
        .cloned()
        .collect();
    let (others, load_failures) = load_fragments(ctx, &other_raws);
    failures.extend(load_failures);

    for (p, other) in &others {
        let ep = util::obj_or_empty(other, "enabledPlugins");
        if let Some(v) = ep.get(pid) {
            notes.push(format!(
                "note: {pid} is also declared in {} as {}; later fragments win",
                display(p),
                py_dumps(v)
            ));
        }
    }

    {
        let ep = frag
            .entry("enabledPlugins".to_string())
            .or_insert_with(|| json!({}));
        if !ep.is_object() {
            *ep = json!({});
        }
        ep.as_object_mut()
            .unwrap()
            .insert(pid.to_string(), Json::Bool(value));
    }
    let mkt = plugin_marketplace_of(pid);
    if value && !mkt.is_empty() && mkt != OFFICIAL_MARKETPLACE {
        let declared = util::obj_or_empty(&frag, "extraKnownMarketplaces").contains_key(&mkt)
            || others
                .iter()
                .any(|(_, o)| util::obj_or_empty(o, "extraKnownMarketplaces").contains_key(&mkt));
        if !declared {
            let (known, err) = if paths::which("claude").is_some() {
                match claude_cli_json(&["plugin", "marketplace", "list"]) {
                    Ok(k) => (k, String::new()),
                    Err(e) => (Vec::new(), e),
                }
            } else {
                (Vec::new(), "claude not on PATH".to_string())
            };
            let row = known
                .iter()
                .filter_map(|m| util::obj(Some(m)))
                .find(|m| util::string(m.get("name")) == Some(mkt.as_str()));
            match marketplace_entry_from_cli(row) {
                Some(entry) => {
                    let mkts = frag
                        .entry("extraKnownMarketplaces".to_string())
                        .or_insert_with(|| json!({}));
                    if !mkts.is_object() {
                        *mkts = json!({});
                    }
                    mkts.as_object_mut().unwrap().insert(mkt.clone(), entry);
                    notes.push(format!(
                        "declared marketplace {mkt} in {}",
                        paths::name(&fpath)
                    ));
                }
                None => failures.push(format!(
                    "marketplace {mkt} is not registered here{}; add it to extraKnownMarketplaces in {} by hand",
                    if err.is_empty() {
                        String::new()
                    } else {
                        format!(" ({err})")
                    },
                    display(&fpath)
                )),
            }
        }
    }

    if let Some(parent) = fpath.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            die(format!("{}: {e}", display(parent)));
        }
    }
    if let Err(e) = util::write_json(&fpath, &Json::Object(frag)) {
        die(format!("{}: {e}", display(&fpath)));
    }
    notes.push(format!(
        "{pid} = {} in {}",
        py_dumps(&Json::Bool(value)),
        display(&fpath)
    ));
    (notes, failures)
}

/// Parse `claude plugin details` for its always-on token estimate.
pub fn plugin_always_on_tokens(pid: &str) -> Option<i64> {
    let out = proc::run_capture(
        &argv(&["claude", "plugin", "details", pid]),
        Some(Duration::from_secs(60)),
    )
    .ok()?;
    parse_always_on(&out.stdout)
}

/// `Always-on:\s*~?([\d.,]+)\s*(k?)\s*tok`
fn parse_always_on(text: &str) -> Option<i64> {
    let mut search = text;
    while let Some(i) = search.find("Always-on:") {
        let rest = &search[i + "Always-on:".len()..];
        let mut s = rest.trim_start();
        s = s.strip_prefix('~').unwrap_or(s);
        let digits: String = s
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
            .collect();
        if !digits.is_empty() {
            let after = s[digits.len()..].trim_start();
            let (k, after) = match after.strip_prefix('k') {
                Some(a) => (true, a),
                None => (false, after),
            };
            if after.trim_start().starts_with("tok") {
                let n: f64 = digits.replace(',', "").parse().ok()?;
                return Some(if k { (n * 1000.0) as i64 } else { n as i64 });
            }
        }
        search = rest;
    }
    None
}

/// Run one planned `claude plugin` call. Returns an error message or None.
pub fn run_plugin_action(cmd: &[String]) -> Option<String> {
    match proc::run_capture(cmd, Some(Duration::from_secs(600))) {
        Err(e) => Some(e),
        Ok(out) if out.ok() => None,
        Ok(out) => {
            let text = if out.stderr.trim().is_empty() {
                out.stdout.trim()
            } else {
                out.stderr.trim()
            };
            let lines: Vec<&str> = text.lines().collect();
            let tail = lines[lines.len().saturating_sub(2)..].join(" ");
            Some(if tail.is_empty() {
                format!("exit {}", out.code_str())
            } else {
                tail
            })
        }
    }
}
