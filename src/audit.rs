// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! --audit: inventory the whole setup and flag what needs a human decision.

use crate::ctx::{Ctx, VERSION};
use crate::manifest::manifest_skill_srcs;
use crate::paths;
use crate::plugins::{claude_cli_json, plugin_always_on_tokens, plugin_status};
use crate::skills::{
    collect_desired, collect_repo_skills, frontmatter_description, scan_skill_dirs,
    unlinked_repo_skill_dirs,
};
use crate::util::{self, display, JMap};
use serde_json::{json, Value as Json};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

struct SkillRow {
    name: String,
    path: String,
    origin: String,
    description_chars: usize,
}

fn finding(kind: &str, severity: &str, message: &str, items: Vec<String>) -> Json {
    json!({"kind": kind, "severity": severity, "message": message, "items": items})
}

fn endpoint(cfg: &JMap) -> String {
    if util::truthy(cfg.get("url")) {
        return util::py_str(&cfg["url"]).trim_end_matches('/').to_string();
    }
    let mut parts = vec![util::string(cfg.get("command")).unwrap_or("").to_string()];
    parts.extend(util::arr_or_empty(cfg, "args").iter().map(util::py_str));
    parts.join(" ").trim().to_string()
}

/// Findings (each {kind, severity, message, items}):
///   skill-collision          a name claimed by two skill sources
///   plugin-skill-collision   a linked skill and an enabled plugin's skill share a name
///   double-load              a plugin installed here whose skills are also linked
///   mcp-duplicate            a user-scope MCP server an enabled plugin also provides
///   unmanaged-plugins / dropped-plugins / disabled-plugins
///   unlinked-repo-skills     skills in this repo the manifest does not link
///   missing-skill-repos      skill_repos checkouts that are absent
///   context-budget           counts and always-loaded token estimates
/// Returns (report, failures).
pub fn audit(
    ctx: &Ctx,
    manifest_name: &str,
    manifest: &JMap,
    state: &JMap,
    skills_target: &Path,
) -> (JMap, Vec<String>) {
    let mut failures: Vec<String> = Vec::new();
    let mut findings: Vec<Json> = Vec::new();
    let (claude_ok, agy_ok) = (ctx.claude_installed(), ctx.agy_installed());

    let (desired, _) = collect_desired(ctx, manifest, skills_target, claude_ok, agy_ok);
    let taken: HashSet<PathBuf> = desired.iter().map(|d| d.dst.clone()).collect();
    let repo = collect_repo_skills(ctx, manifest, skills_target, &taken);
    let mut skills: Vec<SkillRow> = Vec::new();
    for d in desired.iter().chain(repo.entries.iter()) {
        if paths::parent(&d.dst) != skills_target || !d.src.join("SKILL.md").is_file() {
            continue;
        }
        let desc = frontmatter_description(&d.src);
        skills.push(SkillRow {
            name: paths::name(&d.dst),
            path: display(&d.src),
            origin: repo
                .origins
                .get(&d.dst)
                .cloned()
                .unwrap_or_else(|| "manifest".to_string()),
            description_chars: desc.chars().count(),
        });
    }
    if !repo.warnings.is_empty() {
        findings.push(finding(
            "skill-collision",
            "warn",
            "a skill name claimed by more than one source; the first listed source wins",
            repo.warnings.clone(),
        ));
    }
    let linked_srcs = manifest_skill_srcs(ctx, manifest);
    let unlinked = unlinked_repo_skill_dirs(&ctx.repo, &linked_srcs, true);
    if !unlinked.is_empty() {
        findings.push(finding(
            "unlinked-repo-skills",
            "info",
            "skills in this repo that the manifest does not link (--add-skill to link)",
            unlinked,
        ));
    }
    if !repo.missing.is_empty() {
        findings.push(finding(
            "missing-skill-repos",
            "warn",
            "skill_repos checkouts not present here",
            repo.missing.iter().map(|p| display(p)).collect(),
        ));
    }

    let mut plugins = Json::Object(JMap::new());
    let mut user_mcp = JMap::new();
    let mut plugin_mcp: Vec<(String, JMap)> = Vec::new();
    let mut plugin_costs: Vec<(String, Option<i64>)> = Vec::new();
    if claude_ok && paths::which("claude").is_some() {
        let (pstatus, pfails) = plugin_status(ctx, manifest_name, manifest, state);
        failures.extend(pfails);
        let installed = match claude_cli_json(&["plugin", "list"]) {
            Ok(i) => i,
            Err(e) => {
                failures.push(e);
                Vec::new()
            }
        };
        let user_rows: Vec<&JMap> = installed
            .iter()
            .filter_map(|p| util::obj(Some(p)))
            .filter(|p| util::string(p.get("scope")) == Some("user"))
            .collect();
        let enabled_rows: Vec<&JMap> = user_rows
            .iter()
            .copied()
            .filter(|p| util::truthy(p.get("enabled")))
            .collect();
        let mut plugin_skills: Vec<(String, Vec<String>)> = Vec::new();
        for p in &enabled_rows {
            let Some(pid) = util::string(p.get("id")).map(str::to_string) else {
                continue;
            };
            plugin_costs.push((pid.clone(), plugin_always_on_tokens(&pid)));
            let install_path = match p.get("installPath") {
                Some(v) if util::truthy(Some(v)) => paths::clean(&util::py_str(v)),
                _ => PathBuf::from("/nonexistent"),
            };
            let names: Vec<String> = scan_skill_dirs(&install_path.join("skills"))
                .iter()
                .map(|d| paths::name(d))
                .collect();
            plugin_skills.push((pid.clone(), names));
            if let Some(servers) = util::obj(p.get("mcpServers")).filter(|m| !m.is_empty()) {
                plugin_mcp.push((pid, servers.clone()));
            }
        }
        let linked_names: HashSet<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        let overlap: Vec<String> = plugin_skills
            .iter()
            .flat_map(|(pid, names)| {
                names
                    .iter()
                    .filter(|n| linked_names.contains(n.as_str()))
                    .map(move |n| format!("{n} (linked) vs {n} in {pid}"))
            })
            .collect();
        if !overlap.is_empty() {
            findings.push(finding(
                "plugin-skill-collision",
                "warn",
                "a linked skill and an enabled plugin ship a skill of the same name",
                overlap,
            ));
        }
        let enabled_ids: BTreeSet<String> = enabled_rows
            .iter()
            .filter_map(|p| util::string(p.get("id")).map(str::to_string))
            .collect();
        let twice: Vec<String> = repo
            .discovered
            .iter()
            .map(|(m, pl)| format!("{pl}@{m}"))
            .filter(|pid| enabled_ids.contains(pid))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !twice.is_empty() {
            findings.push(finding(
                "double-load",
                "warn",
                "enabled plugins whose skills are also linked from a skill_repos checkout",
                twice,
            ));
        }
        for (key, sev, msg) in [
            (
                "unmanaged",
                "info",
                "installed here, declared in no fragment — the review-plugins skill triages these",
            ),
            (
                "dropped",
                "info",
                "dropped from the fragments, still installed",
            ),
        ] {
            let rows = util::arr_or_empty(&pstatus, key);
            if !rows.is_empty() {
                let items = rows
                    .iter()
                    .filter_map(|e| util::obj(Some(e)))
                    .map(|e| util::py_get_str(e, "id"))
                    .collect();
                findings.push(finding(&format!("{key}-plugins"), sev, msg, items));
            }
        }
        let disabled: Vec<String> = user_rows
            .iter()
            .filter(|p| !util::truthy(p.get("enabled")))
            .filter_map(|p| util::string(p.get("id")).map(str::to_string))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !disabled.is_empty() {
            findings.push(finding(
                "disabled-plugins",
                "info",
                "installed but disabled here; uninstall candidates if that is true everywhere",
                disabled.clone(),
            ));
        }
        if ctx.claude_json_file.is_file() {
            if let Ok(Json::Object(m)) = util::read_json(&ctx.claude_json_file) {
                user_mcp = util::obj_or_empty(&m, "mcpServers");
            }
        }
        let mut dup: Vec<String> = Vec::new();
        for (name, cfg) in &user_mcp {
            let Some(cfg) = util::obj(Some(cfg)) else {
                continue;
            };
            let ep = endpoint(cfg);
            if ep.is_empty() {
                continue;
            }
            for (pid, servers) in &plugin_mcp {
                for (sname, scfg) in servers {
                    let Some(scfg) = util::obj(Some(scfg)) else {
                        continue;
                    };
                    if ep == endpoint(scfg) {
                        dup.push(format!("{name} (user scope) = {sname} in {pid}"));
                    }
                }
            }
        }
        if !dup.is_empty() {
            findings.push(finding(
                "mcp-duplicate",
                "warn",
                "user-scope MCP servers that an enabled plugin also provides; `claude mcp remove <name> -s user` drops the duplicate",
                dup,
            ));
        }
        let mut managed: Vec<String> = util::obj_or_empty(&pstatus, "managed")
            .keys()
            .cloned()
            .collect();
        managed.sort();
        let skills_obj: JMap = plugin_skills
            .iter()
            .map(|(pid, names)| (pid.clone(), json!(names)))
            .collect();
        let costs_obj: JMap = plugin_costs
            .iter()
            .map(|(pid, c)| (pid.clone(), c.map(Json::from).unwrap_or(Json::Null)))
            .collect();
        plugins = json!({
            "managed": managed,
            "enabled": enabled_ids.iter().cloned().collect::<Vec<_>>(),
            "disabled": disabled,
            "skills": skills_obj,
            "always_on_tokens": costs_obj,
        });
    }

    let desc_chars: usize = skills.iter().map(|s| s.description_chars).sum();
    let plugin_tokens: i64 = plugin_costs
        .iter()
        .filter_map(|(_, c)| *c)
        .filter(|c| *c != 0)
        .sum();
    let mut largest: Vec<&SkillRow> = skills.iter().collect();
    largest.sort_by_key(|s| std::cmp::Reverse(s.description_chars));
    largest.truncate(8);
    let budget = json!({
        "skills": skills.len(),
        "description_chars": desc_chars,
        "description_tokens_est": desc_chars / 4,
        "plugin_always_on_tokens": plugin_tokens,
        "largest_descriptions": largest.iter().map(|s| json!({"name": s.name, "chars": s.description_chars})).collect::<Vec<_>>(),
    });
    findings.push(finding(
        "context-budget",
        "info",
        &format!(
            "{} skills linked, ~{} tokens of always-loaded descriptions, ~{} always-on tokens from {} enabled plugins",
            skills.len(),
            desc_chars / 4,
            plugin_tokens,
            plugin_costs.len()
        ),
        largest.iter().map(|s| format!("{}: {} chars", s.name, s.description_chars)).collect(),
    ));
    let mut user_servers: Vec<String> = user_mcp.keys().cloned().collect();
    user_servers.sort();
    let plugin_servers: JMap = plugin_mcp
        .iter()
        .map(|(pid, s)| {
            let mut names: Vec<String> = s.keys().cloned().collect();
            names.sort();
            (pid.clone(), json!(names))
        })
        .collect();
    let report = json!({
        "tool": VERSION,
        "manifest": manifest_name,
        "skills": skills.iter().map(|s| json!({
            "name": s.name, "path": s.path, "origin": s.origin,
            "description_chars": s.description_chars,
        })).collect::<Vec<_>>(),
        "plugins": plugins,
        "mcp": {"user_servers": user_servers, "plugin_servers": plugin_servers},
        "budget": budget,
        "findings": findings,
    });
    match report {
        Json::Object(m) => (m, failures),
        _ => unreachable!(),
    }
}

pub fn print_audit(report: &JMap) {
    let b = util::obj_or_empty(report, "budget");
    println!(
        "manifest: {}   qbranch {}",
        util::py_get_str(report, "manifest"),
        util::py_get_str(report, "tool")
    );
    println!(
        "skills linked: {}   description text: {} chars (~{} tok, always loaded)",
        util::py_get_str(&b, "skills"),
        util::py_get_str(&b, "description_chars"),
        util::py_get_str(&b, "description_tokens_est")
    );
    let p = util::obj_or_empty(report, "plugins");
    if !p.is_empty() {
        println!(
            "plugins: {} enabled, {} disabled, {} managed; always-on ~{} tok",
            util::arr_or_empty(&p, "enabled").len(),
            util::arr_or_empty(&p, "disabled").len(),
            util::arr_or_empty(&p, "managed").len(),
            util::py_get_str(&b, "plugin_always_on_tokens")
        );
    }
    let m = util::obj_or_empty(report, "mcp");
    let from_plugins: usize = util::obj_or_empty(&m, "plugin_servers")
        .values()
        .map(|v| util::arr(Some(v)).map(Vec::len).unwrap_or(0))
        .sum();
    println!(
        "mcp: {} user-scope servers, {} from plugins",
        util::arr_or_empty(&m, "user_servers").len(),
        from_plugins
    );
    for f in util::arr_or_empty(report, "findings") {
        let Some(f) = util::obj(Some(&f)) else {
            continue;
        };
        println!(
            "\n[{}] {}: {}",
            util::py_get_str(f, "severity"),
            util::py_get_str(f, "kind"),
            util::py_get_str(f, "message")
        );
        for item in util::arr_or_empty(f, "items") {
            println!("  - {}", util::py_str(&item));
        }
    }
}
