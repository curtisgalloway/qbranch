// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! The sync itself: plan the link, settings and plugin actions from the
//! manifest and the state file, print the plan (as text or JSON), and
//! apply it unless this is a dry run.

use crate::ctx::{Ctx, LEGACY_STATE_FILE_NAME, MANIFEST_SCHEMA, STATE_FILE_NAME, VERSION};
use crate::manifest::{load_manifest, manifest_skill_srcs};
use crate::paths;
use crate::plugins::{plan_claude_plugins, run_plugin_action, PluginAction};
use crate::proc;
use crate::settings::sync_settings;
use crate::skills::{collect_desired, collect_repo_skills, unlinked_repo_skill_dirs};
use crate::state::{adopt_remembered_root, load_state, previous_links, save_state};
use crate::util::{self, die, display, py_repr_str, JMap};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

pub struct SyncArgs {
    pub manifest: Option<String>,
    pub skills_target: PathBuf,
    pub dry_run: bool,
    pub json: bool,
    pub root_given: bool,
}

/// Messages recorded for the plan; printed unless building JSON.
struct Say {
    json: bool,
    messages: Vec<Json>,
}

impl Say {
    fn say(&mut self, level: &str, text: String) {
        self.messages.push(json!({"level": level, "text": text}));
        if self.json {
            return;
        }
        match level {
            "error" => eprintln!("ERROR: {text}"),
            "info" => println!("  {text}"),
            _ => println!("{level}: {text}"),
        }
    }
}

struct Action {
    op: &'static str,
    label: String,
    src: Option<PathBuf>,
    dst: PathBuf,
    note: String,
}

struct AgentSpec {
    name: &'static str,
    manifest_key: &'static str,
    target: PathBuf,
    state_key: &'static str,
    installed: bool,
}

/// Convert a real per-agent skills directory into a symlink to src.
///
/// Stale symlinks inside have already been removed by the cleanup pass.
/// Returns an error message, or None on success.
fn migrate_skills_dir(dst: &Path, src: &Path) -> Option<String> {
    for junk in [STATE_FILE_NAME, LEGACY_STATE_FILE_NAME, ".DS_Store"] {
        let p = dst.join(junk);
        if p.is_file() {
            let _ = fs::remove_file(&p);
        }
    }
    if fs::remove_dir(dst).is_err() {
        let mut leftover: Vec<String> = fs::read_dir(dst)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        leftover.sort();
        return Some(format!(
            "{} is a directory and still contains: {} — move these aside and re-run",
            display(dst),
            leftover.join(", ")
        ));
    }
    match paths::symlink(src, dst) {
        Ok(()) => None,
        Err(e) => Some(format!("{}: {e}", display(dst))),
    }
}

/// If claude is on PATH and stdin is a terminal, offer to run it with the
/// failure context.
fn offer_claude(failures: &[String]) {
    if paths::which("claude").is_none() || !io::stdin().is_terminal() {
        return;
    }
    let lines: Vec<String> = failures.iter().map(|f| format!("  - {f}")).collect();
    let prompt = format!(
        "The qbranch script failed with the following problems:\n{}\nPlease help me fix them.",
        lines.join("\n")
    );
    print!("\nRun claude to fix this? [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    if io::stdin().lock().read_line(&mut answer).is_err() {
        println!();
        return;
    }
    if answer.trim().to_lowercase() == "y" {
        proc::exec(&["claude".to_string(), prompt]);
    }
}

fn link(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    paths::symlink(src, dst)
}

pub fn run(ctx: &mut Ctx, args: &SyncArgs) -> i32 {
    if args.json && !args.dry_run {
        die("--json on a sync needs --dry-run (it prints the plan); --plugin-status and --audit have their own JSON reports");
    }
    let mut say = Say {
        json: args.json,
        messages: Vec::new(),
    };

    let skills_target = args.skills_target.clone();
    let fresh_target = paths::is_symlink(&skills_target);
    if fresh_target {
        // e.g. an old ~/.agents/skills -> ~/.claude/skills link; the generic
        // dir is the primary now, so it must be a real directory.
        let target = paths::read_link(&skills_target)
            .map(|p| display(&p))
            .unwrap_or_default();
        say.say(
            "note",
            format!(
                "{} was a symlink -> {target} — replacing with a real directory",
                display(&skills_target)
            ),
        );
        if !args.dry_run {
            if let Err(e) = paths::unlink(&skills_target) {
                die(format!("{}: {e}", display(&skills_target)));
            }
        }
    }
    if !args.dry_run {
        if let Err(e) = fs::create_dir_all(&skills_target) {
            die(format!("{}: {e}", display(&skills_target)));
        }
    }
    let state_path = skills_target.join(STATE_FILE_NAME);

    let state = load_state(ctx, &state_path);
    adopt_remembered_root(ctx, args.root_given, &state);
    let manifest_name = args
        .manifest
        .clone()
        .or_else(|| {
            util::string(state.get("manifest"))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "default".to_string());
    let (manifest, upgrade_notes) = load_manifest(ctx, &manifest_name);
    for n in upgrade_notes {
        say.say(
            "note",
            format!(
                "manifest {}: {n} (in memory; run --upgrade-manifests to rewrite manifests/)",
                py_repr_str(&manifest_name)
            ),
        );
    }
    let claude_ok = ctx.claude_installed();
    let agy_ok = ctx.agy_installed();

    let agents = [
        AgentSpec {
            name: "Claude Code",
            manifest_key: "claude_settings",
            target: ctx.claude_settings_file.clone(),
            state_key: "claude_settings_applied",
            installed: claude_ok,
        },
        AgentSpec {
            name: "Antigravity",
            manifest_key: "agy_settings",
            target: ctx.agy_settings_file.clone(),
            state_key: "agy_settings_applied",
            installed: agy_ok,
        },
    ];

    // Settings run before link planning: converting a legacy settings.json
    // symlink into a real file must happen before the stale-link remove pass
    // evaluates what is and isn't a symlink.
    let mut settings_notes: Vec<String> = Vec::new();
    let mut settings_failures: Vec<String> = Vec::new();
    let mut applied_policy: BTreeMap<String, Option<JMap>> = BTreeMap::new();
    for a in &agents {
        if a.installed {
            let o = sync_settings(
                ctx,
                a.name,
                a.manifest_key,
                &a.target,
                a.state_key,
                &manifest,
                &state,
                args.dry_run,
            );
            settings_notes.extend(o.notes);
            settings_failures.extend(o.failures);
            applied_policy.insert(a.state_key.to_string(), o.applied);
        } else if util::truthy(manifest.get(a.manifest_key)) {
            settings_notes.push(format!("{} settings: skipped (not installed)", a.name));
        }
    }

    let (mut desired, skipped) = collect_desired(ctx, &manifest, &skills_target, claude_ok, agy_ok);
    let taken: HashSet<PathBuf> = desired.iter().map(|d| d.dst.clone()).collect();
    let repo = collect_repo_skills(ctx, &manifest, &skills_target, &taken);
    desired.extend(repo.entries.iter().cloned());

    // Plugins follow the merged Claude policy: the settings pass has already
    // asserted the keys, this plans the CLI calls that make them true.
    let mut plugin_actions: Vec<PluginAction> = Vec::new();
    if claude_ok {
        if let Some(Some(policy)) = applied_policy.get("claude_settings_applied") {
            let old = util::obj_or_empty(&state, "claude_settings_applied");
            let (a, n, f) = plan_claude_plugins(policy, &old, &repo.discovered);
            plugin_actions = a;
            settings_notes.extend(n);
            settings_failures.extend(f);
        }
    }

    for r in &repo.missing {
        say.say(
            "note",
            format!(
                "skill repo {} not found — clone it, or drop it from the manifest's skill_repos",
                display(r)
            ),
        );
    }
    for w in &repo.warnings {
        say.say("warning", w.clone());
    }
    for s in &skipped {
        say.say("skipped", s.clone());
    }
    for n in &settings_notes {
        say.say("info", n.clone());
    }
    for f in &settings_failures {
        say.say("error", f.clone());
    }

    let desired_dsts: HashSet<PathBuf> = desired.iter().map(|d| d.dst.clone()).collect();
    let previous_dsts: BTreeSet<PathBuf> = previous_links(&state).into_iter().collect();

    // The live settings.json is managed by the settings merge, not as a
    // link: keep the remove pass away from it even if an old state file
    // still lists it from the symlink era.
    let managed: HashSet<PathBuf> = agents
        .iter()
        .filter(|a| a.installed && util::truthy(manifest.get(a.manifest_key)))
        .map(|a| a.target.clone())
        .collect();

    let mut actions: Vec<Action> = Vec::new();
    for dst in &previous_dsts {
        if desired_dsts.contains(dst) || managed.contains(dst) {
            continue;
        }
        if paths::is_symlink(dst) {
            let target = paths::read_link(dst)
                .map(|p| display(&p))
                .unwrap_or_default();
            actions.push(Action {
                op: "remove",
                label: paths::name(dst),
                src: None,
                dst: dst.clone(),
                note: format!("-> {target} (not in manifest)"),
            });
        }
    }

    for d in &desired {
        if !d.src.exists() && !(args.dry_run && d.src == skills_target) {
            // A MISS never reaches `final`, so its dst never lands in the
            // state file; the stale-removal pass above (which walks
            // previous_dsts) can therefore never reclaim a symlink left
            // here, and it would dangle forever. Clear it as part of the
            // failure instead. Only a symlink whose target is gone is
            // touched; a working link is left for the WARN/relink paths.
            let note = if paths::is_symlink(&d.dst) && !d.dst.exists() {
                "source missing (will remove stale link)"
            } else {
                "source missing"
            };
            actions.push(Action {
                op: "MISS",
                label: d.label.clone(),
                src: Some(d.src.clone()),
                dst: d.dst.clone(),
                note: note.to_string(),
            });
            continue;
        }
        let action = |op: &'static str, note: String| Action {
            op,
            label: d.label.clone(),
            src: Some(d.src.clone()),
            dst: d.dst.clone(),
            note,
        };
        if fresh_target && d.dst.starts_with(&skills_target) && d.dst != skills_target {
            // skills_target was just (or, on dry-run, would be) replaced with
            // an empty directory: nothing under it can pre-exist.
            actions.push(action("link", String::new()));
            continue;
        }
        if paths::is_symlink(&d.dst) {
            let cur_raw = paths::read_link(&d.dst).unwrap_or_default();
            let cur = if cur_raw.is_absolute() {
                paths::resolve(&cur_raw)
            } else {
                paths::resolve(&paths::parent(&d.dst).join(&cur_raw))
            };
            if cur == paths::resolve(&d.src) {
                actions.push(action("ok", String::new()));
            } else {
                actions.push(action("relink", format!("was: {}", display(&cur_raw))));
            }
        } else if d.dst.is_dir()
            && (d.dst == ctx.claude_skills_link || d.dst == ctx.agy_skills_link)
        {
            actions.push(action(
                "migrate",
                "dir -> symlink (after stale links removed)".to_string(),
            ));
        } else if d.dst.exists() {
            actions.push(action(
                "WARN",
                "non-symlink at destination — leaving alone".to_string(),
            ));
        } else {
            actions.push(action("link", String::new()));
        }
    }

    if !args.json {
        let label_w = actions
            .iter()
            .map(|a| a.label.chars().count())
            .chain(plugin_actions.iter().map(|a| a.label.chars().count()))
            .max()
            .unwrap_or(8);
        for a in &actions {
            let srcs = match &a.src {
                Some(src) => format!("{} -> {}", display(&a.dst), display(src)),
                None => display(&a.dst),
            };
            let line = format!(
                "  {:<7} {:<w$}  {}  {srcs}",
                a.op,
                a.label,
                a.note,
                w = label_w
            );
            println!("{}", line.trim_end());
        }
        for a in &plugin_actions {
            println!(
                "  {:<7} {:<w$}  {}",
                a.op,
                a.label,
                a.argv.join(" "),
                w = label_w
            );
        }
        if actions.is_empty() && plugin_actions.is_empty() {
            println!("  (nothing to do)");
        }
    }

    let unsynced = unlinked_repo_skill_dirs(&ctx.repo, &manifest_skill_srcs(ctx, &manifest), false);
    if !unsynced.is_empty() {
        say.say("note", format!("not in manifest: {}", unsynced.join(", ")));
    }

    if args.json {
        let plan = json!({
            "tool": VERSION,
            "schema": MANIFEST_SCHEMA,
            "manifest": manifest_name,
            "dry_run": true,
            "skills_target": display(&skills_target),
            "claude": claude_ok,
            "agy": agy_ok,
            "desired": desired_dsts.len(),
            "actions": actions.iter().map(|a| json!({
                "op": a.op, "label": a.label,
                "src": a.src.as_ref().map(|p| display(p)),
                "dst": display(&a.dst), "note": a.note,
            })).collect::<Vec<_>>(),
            "plugin_actions": plugin_actions.iter().map(|a| json!({
                "op": a.op, "label": a.label, "argv": a.argv,
            })).collect::<Vec<_>>(),
            "unlinked_repo_skills": unsynced,
            "messages": say.messages,
            "failures": settings_failures,
        });
        println!("{}", util::pretty(&plan));
        return if settings_failures.is_empty() { 0 } else { 1 };
    }

    println!();
    println!(
        "manifest={manifest_name}  desired={}  skills_target={}  claude={}  agy={}",
        desired_dsts.len(),
        display(&skills_target),
        if claude_ok { "yes" } else { "no" },
        if agy_ok { "yes" } else { "no" }
    );

    if args.dry_run {
        println!("(dry run — no changes made)");
        return if settings_failures.is_empty() { 0 } else { 1 };
    }

    let mut final_links: Vec<PathBuf> = Vec::new();
    let mut had_error = !settings_failures.is_empty();
    let mut failures: Vec<String> = settings_failures.clone();
    let fail = |msg: String, failures: &mut Vec<String>, had_error: &mut bool| {
        eprintln!("ERROR: {msg}");
        failures.push(msg);
        *had_error = true;
    };
    for a in &actions {
        let src = a.src.clone().unwrap_or_default();
        let result: io::Result<()> = match a.op {
            "remove" => paths::unlink(&a.dst),
            "relink" => paths::unlink(&a.dst)
                .and_then(|_| link(&src, &a.dst))
                .map(|_| {
                    final_links.push(a.dst.clone());
                }),
            "link" => link(&src, &a.dst).map(|_| {
                final_links.push(a.dst.clone());
            }),
            "ok" => {
                final_links.push(a.dst.clone());
                Ok(())
            }
            "migrate" => {
                match migrate_skills_dir(&a.dst, &src) {
                    Some(msg) => fail(format!("{}: {msg}", a.label), &mut failures, &mut had_error),
                    None => final_links.push(a.dst.clone()),
                }
                Ok(())
            }
            "WARN" => {
                fail(
                    format!(
                        "{}: non-symlink at {} — refusing to overwrite",
                        a.label,
                        display(&a.dst)
                    ),
                    &mut failures,
                    &mut had_error,
                );
                Ok(())
            }
            "MISS" => {
                let mut msg = format!("{}: source missing at {}", a.label, display(&src));
                if paths::is_symlink(&a.dst) && !a.dst.exists() {
                    if let Err(e) = paths::unlink(&a.dst) {
                        fail(
                            format!("{} ({}): {e}", a.label, display(&a.dst)),
                            &mut failures,
                            &mut had_error,
                        );
                        continue;
                    }
                    msg.push_str(&format!(" (removed stale link at {})", display(&a.dst)));
                }
                fail(msg, &mut failures, &mut had_error);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            fail(
                format!("{} ({}): {e}", a.label, display(&a.dst)),
                &mut failures,
                &mut had_error,
            );
        }
    }

    // Marketplaces come before the plugins that need them (planned in that
    // order). Each call may clone a repo, so show progress per item.
    for a in &plugin_actions {
        print!("  {:<7} {} ...", a.op, a.label);
        let _ = io::stdout().flush();
        match run_plugin_action(&a.argv) {
            Some(err) => {
                println!(" FAILED");
                fail(
                    format!("{}: {}: {err}", a.label, a.argv.join(" ")),
                    &mut failures,
                    &mut had_error,
                );
            }
            None => println!(" done"),
        }
    }

    for a in &agents {
        let entry = applied_policy
            .entry(a.state_key.to_string())
            .or_insert(None);
        if entry.is_none() {
            // Settings failed or the agent is absent: keep the previous
            // snapshot so a future sync can still retract what an older
            // policy asserted.
            *entry = Some(util::obj_or_empty(&state, a.state_key));
        }
    }
    save_state(
        ctx,
        &state_path,
        &manifest_name,
        &final_links,
        &applied_policy,
    );

    if !failures.is_empty() {
        offer_claude(&failures);
    }
    if had_error {
        1
    } else {
        0
    }
}
