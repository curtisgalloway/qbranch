// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! The sync itself: plan the link, settings and plugin actions from the
//! manifest and the state file, print the plan (as text or JSON), and
//! apply it unless this is a dry run.

use crate::copy::{copy_path, copy_up_to_date, remove_path, symlinks_available};
use crate::ctx::{Ctx, LEGACY_STATE_FILE_NAME, MANIFEST_SCHEMA, STATE_FILE_NAME, VERSION};
use crate::manifest::{load_manifest, manifest_skill_srcs};
use crate::paths;
use crate::plugins::{plan_claude_plugins, run_plugin_action, PluginAction};
use crate::proc;
use crate::settings::sync_settings;
use crate::skills::{collect_desired, collect_repo_skills, unlinked_repo_skill_dirs};
use crate::state::{
    choose_manifest, load_state, previous_copies, previous_links, resolve_root, save_state,
};
use crate::util::{self, die, display, JMap};
use serde_json::{json, Value as Json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

pub const LINK_MODES: [&str; 3] = ["auto", "symlink", "copy"];

pub struct SyncArgs {
    pub manifest: Option<String>,
    pub skills_target: PathBuf,
    pub dry_run: bool,
    pub json: bool,
    pub root: Option<String>,
    pub link_mode: Option<String>,
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

/// The link mode for this run, and the choice to remember.
///
/// --link-mode (or $QBRANCH_LINK_MODE) wins and is remembered; `auto`
/// forgets a remembered choice. Otherwise the remembered choice applies,
/// else auto: symlinks, falling back to copies on Windows when the process
/// may not create them.
fn decide_link_mode(
    requested: Option<&str>,
    state: &JMap,
    notes: &mut Vec<String>,
) -> (String, Option<String>) {
    let env_mode = env::var("QBRANCH_LINK_MODE").ok().filter(|s| !s.is_empty());
    let chosen = requested.map(str::to_string).or(env_mode);
    if let Some(c) = &chosen {
        if !LINK_MODES.contains(&c.as_str()) {
            die(format!(
                "--link-mode must be one of {}, not '{c}'",
                LINK_MODES.join(", ")
            ));
        }
    }
    let mut remembered = util::string(state.get("link_mode"))
        .filter(|m| *m == "symlink" || *m == "copy")
        .map(str::to_string);
    let chosen = match chosen {
        None => remembered.clone().unwrap_or_else(|| "auto".to_string()),
        Some(c) if c == "auto" => {
            remembered = None;
            c
        }
        Some(c) => {
            remembered = Some(c.clone());
            c
        }
    };
    if chosen != "auto" {
        return (chosen, remembered);
    }
    if cfg!(windows) && !symlinks_available() {
        notes.push(
            "symbolic links are unavailable here (on Windows, enable Developer Mode to use them) — copying instead; --link-mode symlink insists on links"
                .to_string(),
        );
        return ("copy".to_string(), remembered);
    }
    ("symlink".to_string(), remembered)
}

/// Replace a real per-agent skills directory with a link to (or copy of) src.
///
/// Stale symlinks inside have already been removed by the cleanup pass; the
/// directory must be empty apart from a state file. Returns an error
/// message, or None on success.
fn migrate_skills_dir(dst: &Path, src: &Path, copying: bool) -> Option<String> {
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
    let made = if copying {
        copy_path(src, dst)
    } else {
        paths::symlink(src, dst)
    };
    match made {
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
    let mut say = Say {
        json: args.json,
        messages: Vec::new(),
    };

    let skills_target = args.skills_target.clone();
    if !args.dry_run {
        if let Err(e) = fs::create_dir_all(&skills_target) {
            die(format!("{}: {e}", display(&skills_target)));
        }
    }
    let state_path = skills_target.join(STATE_FILE_NAME);

    let state = load_state(&state_path);
    resolve_root(ctx, args.root.as_deref(), &state);
    let manifest_name = choose_manifest(ctx, args.manifest.as_deref(), &state);
    let (manifest, upgrade_notes) = load_manifest(ctx, &manifest_name);
    for n in upgrade_notes {
        say.say(
            "note",
            format!(
                "manifest '{manifest_name}': {n} (in memory; run --upgrade-manifests to rewrite manifests/)"
            ),
        );
    }
    let mut mode_notes = Vec::new();
    let (link_mode, remembered_mode) =
        decide_link_mode(args.link_mode.as_deref(), &state, &mut mode_notes);
    for n in mode_notes {
        say.say("note", n);
    }
    let copying = link_mode == "copy";
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
    // A copy of the skills directory can only be made once everything in it
    // is there: the per-harness skills entries go last.
    let (harness, rest): (Vec<_>, Vec<_>) =
        desired.into_iter().partition(|d| d.src == skills_target);
    let desired: Vec<_> = rest.into_iter().chain(harness).collect();

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
    let prev_copies: HashSet<PathBuf> = previous_copies(&state).into_iter().collect();

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
        } else if prev_copies.contains(dst) && dst.exists() {
            actions.push(Action {
                op: "remove",
                label: paths::name(dst),
                src: None,
                dst: dst.clone(),
                note: "copy (not in manifest)".to_string(),
            });
        }
    }

    let create: &'static str = if copying { "copy" } else { "link" };
    for d in &desired {
        // A dry run has not created the skills target yet, so a harness link
        // pointing at it is not a missing source.
        let source_missing = !d.src.exists();
        let target_only_planned = args.dry_run && d.src == skills_target;
        if source_missing && !target_only_planned {
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
        if paths::is_symlink(&d.dst) {
            let cur_raw = paths::read_link(&d.dst).unwrap_or_default();
            let cur = if cur_raw.is_absolute() {
                paths::resolve(&cur_raw)
            } else {
                paths::resolve(&paths::parent(&d.dst).join(&cur_raw))
            };
            if cur == paths::resolve(&d.src) {
                actions.push(action("ok", String::new()));
            } else if copying {
                actions.push(action(
                    "copy",
                    format!("was: link -> {}", display(&cur_raw)),
                ));
            } else {
                actions.push(action("relink", format!("was: {}", display(&cur_raw))));
            }
        } else if prev_copies.contains(&d.dst) && d.dst.exists() {
            // A copy an earlier sync made: refresh it, or turn it back into
            // a link if the mode changed.
            if !copying {
                actions.push(action("relink", "was: copy".to_string()));
            } else if copy_up_to_date(&d.src, &d.dst) {
                actions.push(action("ok", "copy".to_string()));
            } else {
                actions.push(action("copy", "refresh".to_string()));
            }
        } else if d.dst.is_dir()
            && (d.dst == ctx.claude_skills_link || d.dst == ctx.agy_skills_link)
        {
            actions.push(action(
                "migrate",
                format!(
                    "dir -> {} (after stale links removed)",
                    if copying { "copy" } else { "symlink" }
                ),
            ));
        } else if d.dst.exists() {
            actions.push(action(
                "WARN",
                "non-symlink at destination — leaving alone".to_string(),
            ));
        } else {
            actions.push(action(create, String::new()));
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
            "link_mode": link_mode,
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
        "manifest={manifest_name}  desired={}  skills_target={}  link_mode={link_mode}  claude={}  agy={}",
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
    let mut copies: Vec<PathBuf> = Vec::new();
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
            "remove" => remove_path(&a.dst),
            "relink" => remove_path(&a.dst)
                .and_then(|_| link(&src, &a.dst))
                .map(|_| final_links.push(a.dst.clone())),
            "link" => link(&src, &a.dst).map(|_| final_links.push(a.dst.clone())),
            "copy" => copy_path(&src, &a.dst).map(|_| {
                final_links.push(a.dst.clone());
                copies.push(a.dst.clone());
            }),
            "ok" => {
                final_links.push(a.dst.clone());
                if prev_copies.contains(&a.dst) && !paths::is_symlink(&a.dst) {
                    copies.push(a.dst.clone());
                }
                Ok(())
            }
            "migrate" => {
                match migrate_skills_dir(&a.dst, &src, copying) {
                    Some(msg) => fail(format!("{}: {msg}", a.label), &mut failures, &mut had_error),
                    None => {
                        final_links.push(a.dst.clone());
                        if copying {
                            copies.push(a.dst.clone());
                        }
                    }
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
        &copies,
        remembered_mode.as_deref(),
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
