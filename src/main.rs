// SPDX-FileCopyrightText: 2026 Curtis Galloway
// SPDX-License-Identifier: Apache-2.0
//! qbranch: outfit a machine's coding agents from a per-machine manifest.
//!
//! This is the Rust port of the reference Python script at `bin/qbranch`;
//! the corpus under `tests/corpus/` is the contract both must satisfy. The
//! script's module docstring documents the manifest and every behaviour;
//! this crate mirrors it function for function.

mod audit;
mod copy;
mod ctx;
mod manifest;
mod paths;
mod plugins;
mod proc;
mod settings;
mod skills;
mod state;
mod sync;
mod util;

use clap::{CommandFactory, FromArgMatches, Parser};
use ctx::{Ctx, BUNDLED_SKILLS, MANIFEST_SCHEMA, STATE_FILE_NAME, VERSION};
use serde_json::{json, Value as Json};
use util::{die, display, JMap};

#[derive(Parser, Debug)]
#[command(
    name = "qbranch",
    about = "Sync agent state from a manifest into the local filesystem.",
    infer_long_args = true,
    disable_version_flag = true
)]
struct Cli {
    /// Manifest name under manifests/ (without .json). Default: the last
    /// synced one, else this host's name, else 'default'.
    #[arg(short = 'm', long)]
    manifest: Option<String>,

    /// Where 'skills' entries get linked (default: ~/.agents/skills). The
    /// state file also lives here.
    #[arg(short = 't', long, value_name = "DIR")]
    skills_target: Option<String>,

    /// Print plan without changing the filesystem. With --json, print the
    /// plan as JSON instead.
    #[arg(short = 'n', long)]
    dry_run: bool,

    /// Config root holding manifests/, skills/ and claude-code/. Default:
    /// $QBRANCH_ROOT, else the root remembered by the last sync, else the
    /// current directory.
    #[arg(long, value_name = "DIR")]
    root: Option<String>,

    /// How entries are materialised: symlink (the default where links
    /// work), copy (copied and refreshed on every sync, for filesystems or
    /// Windows setups without symlinks), auto (symlink, falling back to
    /// copy on Windows without Developer Mode). Remembered like --root;
    /// auto forgets.
    #[arg(long, value_name = "MODE", value_parser = sync::LINK_MODES)]
    link_mode: Option<String>,

    /// List available manifests and exit.
    #[arg(long)]
    list: bool,

    /// Add SKILL to the target manifest(s) and exit. Path defaults to
    /// ${QBRANCH_ROOT}/skills/SKILL.
    #[arg(short = 'a', long, value_name = "SKILL")]
    add_skill: Option<String>,

    /// Remove SKILL from the target manifest(s) and exit.
    #[arg(short = 'r', long, value_name = "SKILL")]
    remove_skill: Option<String>,

    /// Git repo URL for the skill (used with --add-skill). On sync,
    /// ~/src/<repo-name> is checked first; otherwise the repo is cloned to
    /// ~/.agents/skill-repos/.
    #[arg(long, value_name = "URL")]
    repo: Option<String>,

    /// Path within the repo (used with --add-skill --repo). Defaults to
    /// skills/<skill-name>.
    #[arg(long, value_name = "PATH")]
    skill_path: Option<String>,

    /// With --add-skill/--remove-skill: apply to every manifest in
    /// manifests/. Prints a reminder to sync each machine afterwards.
    #[arg(long = "all")]
    all_manifests: bool,

    /// Print version and exit.
    #[arg(short = 'V', long)]
    version: bool,

    /// Print bundled skill NAME (its SKILL.md, verbatim) and exit; without
    /// NAME, list the bundled skills. These are the tool's own skills,
    /// review-plugins and agent-audit, so an agent that finds qbranch on
    /// PATH can read how to drive it.
    #[arg(long, value_name = "NAME", num_args = 0..=1, default_missing_value = "")]
    skill: Option<String>,

    /// Rewrite every manifest under manifests/ at the current schema and
    /// exit. Older manifests work without this (they are upgraded in
    /// memory); a manifest newer than this tool is refused.
    #[arg(long)]
    upgrade_manifests: bool,

    /// Report this machine's Claude Code plugins as managed / unmanaged /
    /// no longer managed against the target manifest, and exit.
    #[arg(long)]
    plugin_status: bool,

    /// Inventory skills, plugins and MCP servers on this machine against
    /// the target manifest and report collisions, double loads,
    /// duplicates, dead weight and the context budget; then exit.
    #[arg(long)]
    audit: bool,

    /// With --plugin-status or --audit: machine-readable output.
    #[arg(long)]
    json: bool,

    /// Declare plugin ID (name@marketplace) in a fragment of the target
    /// manifest and exit. Needs --in; see --value.
    #[arg(long, value_name = "ID")]
    manage_plugin: Option<String>,

    /// With --manage-plugin: 'base' is the shared first fragment (every
    /// machine using it); 'host' is this manifest's hosts/ fragment,
    /// created and wired into the manifest if missing.
    #[arg(long = "in", value_name = "base|host", value_parser = ["base", "host"])]
    fragment_kind: Option<String>,

    /// With --manage-plugin: true installs and enables the plugin wherever
    /// the fragment applies; false pins it disabled there.
    #[arg(long, default_value = "true", value_parser = ["true", "false"])]
    value: String,
}

fn print_errors(fails: &[String]) {
    for f in fails {
        eprintln!("ERROR: {f}");
    }
}

/// --skill: list the bundled skills, or print one's SKILL.md verbatim.
fn print_skill(name: &str) -> i32 {
    if name.is_empty() {
        for (n, _) in BUNDLED_SKILLS {
            println!("{n}");
        }
        return 0;
    }
    match BUNDLED_SKILLS.iter().find(|(n, _)| *n == name) {
        Some((_, text)) => {
            print!("{text}");
            0
        }
        None => {
            let names: Vec<&str> = BUNDLED_SKILLS.iter().map(|(n, _)| *n).collect();
            die(format!(
                "no bundled skill '{name}'; available: {}",
                names.join(", ")
            ))
        }
    }
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let matches = Cli::command().get_matches();
    let args = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if args.version {
        println!("qbranch {VERSION} (manifest schema {MANIFEST_SCHEMA})");
        return 0;
    }
    if let Some(name) = &args.skill {
        return print_skill(name);
    }

    let mut ctx = Ctx::from_env();
    let skills_target = match &args.skills_target {
        Some(t) => paths::expanduser(&paths::clean(t), &ctx.home),
        None => ctx.default_skills_target.clone(),
    };
    let state_path = skills_target.join(STATE_FILE_NAME);

    if args.list {
        state::resolve_root(
            &mut ctx,
            args.root.as_deref(),
            &state::load_state(&state_path),
        );
        for n in manifest::list_manifests(&ctx) {
            println!("{n}");
        }
        return 0;
    }

    if args.upgrade_manifests {
        state::resolve_root(
            &mut ctx,
            args.root.as_deref(),
            &state::load_state(&state_path),
        );
        return manifest::upgrade_manifests(&ctx);
    }

    if args.plugin_status || args.manage_plugin.is_some() || args.audit {
        let state = state::load_state(&state_path);
        state::resolve_root(&mut ctx, args.root.as_deref(), &state);
        let manifest_name = state::choose_manifest(&ctx, args.manifest.as_deref(), &state);
        if args.audit {
            let (manifest, _) = manifest::load_manifest(&ctx, &manifest_name);
            let (report, fails) =
                audit::audit(&ctx, &manifest_name, &manifest, &state, &skills_target);
            if args.json {
                let mut out = report;
                out.insert("errors".to_string(), json!(fails));
                println!("{}", util::pretty(&Json::Object(out)));
            } else {
                audit::print_audit(&report);
                print_errors(&fails);
            }
            return if fails.is_empty() { 0 } else { 1 };
        }
        if let Some(pid) = &args.manage_plugin {
            let Some(kind) = &args.fragment_kind else {
                die("--manage-plugin needs --in base|host");
            };
            let (notes, fails) =
                plugins::manage_plugin(&ctx, &manifest_name, pid, kind, args.value == "true");
            for n in notes {
                println!("{n}");
            }
            print_errors(&fails);
            if fails.is_empty() {
                println!("Run qbranch to apply.");
            }
            return if fails.is_empty() { 0 } else { 1 };
        }
        let (manifest, _) = manifest::load_manifest(&ctx, &manifest_name);
        let (report, fails) = plugins::plugin_status(&ctx, &manifest_name, &manifest, &state);
        if args.json {
            let mut out = report;
            out.insert("errors".to_string(), json!(fails));
            println!("{}", util::pretty(&Json::Object(out)));
        } else {
            plugins::print_plugin_status(&report);
            print_errors(&fails);
        }
        return if fails.is_empty() { 0 } else { 1 };
    }

    if args.add_skill.is_some() && args.remove_skill.is_some() {
        die("--add-skill and --remove-skill are mutually exclusive");
    }

    if args.add_skill.is_some() || args.remove_skill.is_some() {
        let state = state::load_state(&state_path);
        state::resolve_root(&mut ctx, args.root.as_deref(), &state);
        return edit_skill(&ctx, &args, &state);
    }

    if args.json && !args.dry_run {
        die("--json on a sync needs --dry-run (it prints the plan); --plugin-status and --audit have their own JSON reports");
    }
    sync::run(
        &mut ctx,
        &sync::SyncArgs {
            manifest: args.manifest.clone(),
            skills_target,
            dry_run: args.dry_run,
            json: args.json,
            root: args.root.clone(),
            link_mode: args.link_mode.clone(),
        },
    )
}

/// --add-skill / --remove-skill against one manifest or all of them.
fn edit_skill(ctx: &Ctx, args: &Cli, state: &JMap) -> i32 {
    let adding = args.add_skill.is_some();
    let mut skill_name = args
        .add_skill
        .clone()
        .or_else(|| args.remove_skill.clone())
        .unwrap_or_default();
    let mut repo_url = args.repo.clone();
    let mut skill_path_arg = args.skill_path.clone();

    if adding && skill_name.starts_with("git://") {
        let (n, u, p) = manifest::parse_git_skill_url(ctx, &skill_name);
        skill_name = n;
        repo_url = Some(u);
        skill_path_arg = Some(p);
    }

    if adding {
        match &repo_url {
            Some(url) => {
                let local_root = manifest::local_checkout(ctx, url);
                if !local_root.is_dir() {
                    println!(
                        "note: {} not found — will clone on sync",
                        display(&local_root)
                    );
                }
            }
            None => {
                let skill_dir = ctx.repo.join("skills").join(&skill_name);
                if !skill_dir.is_dir() {
                    eprintln!("error: {} not found", display(&skill_dir));
                    return 2;
                }
            }
        }
    }

    let apply = |mname: &str, sname: &str| -> bool {
        if adding {
            manifest::add_skill_to_manifest(
                ctx,
                mname,
                sname,
                repo_url.as_deref(),
                skill_path_arg.as_deref(),
            )
        } else {
            manifest::remove_skill_from_manifest(ctx, mname, sname)
        }
    };

    if args.all_manifests {
        let targets = manifest::list_manifests(ctx);
        if targets.is_empty() {
            die("no manifests found");
        }
        for mname in targets {
            let changed = apply(&mname, &skill_name);
            let status = match (adding, changed) {
                (true, true) => "added",
                (true, false) => "already present",
                (false, true) => "removed",
                (false, false) => "not present",
            };
            println!("  {status:<15}  {mname}");
        }
        println!("\nRemember to run qbranch on each machine to apply.");
        return 0;
    }

    let manifest_name = state::choose_manifest(ctx, args.manifest.as_deref(), state);
    let changed = apply(&manifest_name, &skill_name);
    let (verb, already) = if adding {
        ("added to", "already in")
    } else {
        ("removed from", "not in")
    };
    if changed {
        println!("'{skill_name}' {verb} manifest '{manifest_name}'");
        println!("Run qbranch to apply.");
    } else {
        println!("'{skill_name}' {already} manifest '{manifest_name}'");
    }
    0
}
