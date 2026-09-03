---
name: qbranch
description: Operate qbranch, the tool that outfits a machine's coding agents (Claude Code, Google Antigravity) from a per-machine manifest in a config repo — skills, instruction files, hooks, settings fragments and plugins. Use when setting up a machine from a config root, writing or editing a manifest, adding a skill or plugin to one, checking what a sync would do, or whenever `qbranch` is on PATH and you need to know how to drive it. Read it with `qbranch --skill qbranch`; no checkout is needed.
---

# Operate qbranch

**Terms.** A *manifest* is a JSON file naming what should be active on a machine. The
*config root* is the directory holding `manifests/`, the settings fragments and any skills or
instruction files they point at, usually a private git repo. A *settings fragment* is the part
of a harness's settings that follows the user between machines. A *skill repo* is a checkout
whose skills are linked in bulk. The full vocabulary is `GLOSSARY.md` in the qbranch repo.

## What a sync does, and what it never touches

1. Links every skill the manifest names, plus every skill each listed skill repo offers, into
   `~/.agents/skills/`, and points each harness's own skills directory at it.
2. Links instruction files and hooks wherever the manifest says, skipping destinations for a
   harness that is not installed.
3. Merges the settings fragments into each harness's own settings file: policy keys are
   asserted, keys the fragments never mention are left to the app, and anything an earlier
   sync asserted that the fragments no longer carry is retracted.
4. Registers the marketplaces and installs the plugins the merged fragments declare, through
   the `claude plugin` CLI, and reports plugins installed here but declared nowhere.
5. Records what it did in `~/.agents/skills/.qbranch-state.json`, so the next sync removes
   links that are no longer wanted and retracts settings that are no longer declared.

It writes no file of its own into a home directory besides that state file; everything else is
a symbolic link into a checkout, or in copy mode a tracked copy of one. It never symlinks the
app-owned settings files, never rewrites keys in them the fragments do not mention, and never
touches plugin install state except through the `claude plugin` CLI. Do not hand-edit those
either: the next sync treats a hand edit as app state and leaves it, or retracts it.

## The config root

```
my-agent-config/
├── manifests/<name>.json        one per machine or role
├── claude-code/
│   ├── CLAUDE.md                linked to ~/.claude/CLAUDE.md by a manifest entry
│   ├── hooks/                   hook scripts, linked likewise
│   ├── settings.json            the shared settings fragment
│   └── settings/hosts/<h>.json  per-host fragments; capability fragments alongside
├── agy/                         the same for Antigravity
├── prompts/<use>/AGENTS.md      directory-scoped instruction files, linked where they apply
└── skills/<name>/SKILL.md       skills kept in the config repo itself (optional)
```

One repo is enough: list the config root itself under `skill_repos` and its `skills/`
directory is linked. Separate skill repos are for sharing skills across people or publishing
them.

## Manifest keys

```json
{
  "schema": 2,
  "name": "laptop",
  "skill_repos": [ { "path": "${HOME}/src/public-skills", "plugins": ["dev-tools"] } ],
  "skills": [
    { "name": "learn", "path": "${QBRANCH_ROOT}/skills/learn" },
    { "name": "push", "repo": "git@github.com:me/private-skills.git", "path": "skills/push" }
  ],
  "links": [
    { "src": "${QBRANCH_ROOT}/claude-code/CLAUDE.md", "dst": "${HOME}/.claude/CLAUDE.md" }
  ],
  "claude_settings": [
    "${QBRANCH_ROOT}/claude-code/settings.json",
    "${QBRANCH_ROOT}/claude-code/settings/hosts/laptop.json"
  ],
  "agy_settings": [ "${QBRANCH_ROOT}/agy/settings.json" ]
}
```

- `schema`: the manifest format version, 2 today. An older manifest is upgraded in memory on
  every load; one newer than the tool is refused with a pointer to update the tool.
- `skill_repos`: checkouts whose skills link without per-skill entries. A repo with a Claude
  Code marketplace (`.claude-plugin/marketplace.json`) is read plugin by plugin and `plugins`
  picks themes; a plain repo contributes `skills/*/SKILL.md`. The checkout must already exist
  at that path; a missing one is a note in the plan, not an error.
- `skills`: `{name, path}` for a local path, or `{name, repo, path}` for a path inside a git
  repo. For the repo form `~/src/<repo-name>` is used when it is a checkout; otherwise the
  repo is cloned to `~/.agents/skill-repos/` and pulled on every sync. On a name collision a
  manifest skill wins, then earlier repos over later ones.
- `links`: `{src, dst}`, anything linked anywhere. Entries with a destination under
  `~/.claude/` are skipped where Claude Code is absent, under `~/.gemini/` where Antigravity
  is, so one manifest serves a machine with either, both or neither.
- `claude_settings`, `agy_settings`: ordered fragment paths, the shared base first and the
  host fragment last so its values win. Dicts merge recursively, lists union, later scalars
  win. `extraKnownMarketplaces` and `enabledPlugins` in the merged result are what drive the
  plugin installs.

Paths accept `${QBRANCH_ROOT}` (the config root), any `${VAR}`, and a leading `~`.

## First sync on a machine

1. Get git access to the config root and clone it. Clone any `skill_repos` checkouts at the
   paths the manifest expects; repo-based `skills` entries clone themselves.
2. `qbranch --root <config-root> --manifest <name> --dry-run` and read the plan. It names
   every link, settings key and plugin action. Without `--manifest` the tool takes the last
   synced one, else the manifest named after this host, else `default`.
3. Run it again without `--dry-run`. The root, the manifest name and the link mode are
   remembered in the state file, so from then on a plain `qbranch` re-syncs.
4. On Windows, symbolic links need Developer Mode. Without it the default `auto` mode copies
   each entry, refreshes the copies on every sync, and says so. `--link-mode copy` asks for
   that anywhere; the choice is remembered.

The config root is otherwise found from `$QBRANCH_ROOT`, then the remembered root, then the
current directory when it holds `manifests/`, so `cd my-agent-config && qbranch` also works.

## Everyday commands

```
qbranch                          sync with the remembered root and manifest
qbranch --dry-run [--json]       the plan, changing nothing; JSON for machine reading
qbranch --list                   manifests in the config root
qbranch --add-skill NAME         add a skill to the remembered manifest; --all for every manifest
qbranch --add-skill git://REPO/skills/NAME   the same as a repo entry, from a local checkout
qbranch --remove-skill NAME
qbranch --plugin-status [--json] managed / unmanaged / no-longer-managed plugins here
qbranch --manage-plugin ID --in base|host [--value false]   declare a plugin in a fragment
qbranch --audit [--json]         collisions, double loads, duplicate MCP servers, context budget
qbranch --upgrade-manifests      rewrite older manifests at the current schema
qbranch --skill [NAME]           this skill and its siblings, review-plugins and agent-audit
```

Every editing command changes the config root only. Run `qbranch` afterwards to apply, and
commit the config root so other machines pick the change up on their next sync.

## Rules of thumb

- Dry-run first whenever the manifest or a fragment changed, and read the plan.
- Change plugins and settings through the fragments and the flags above, never in the app's
  own files.
- A plugin dropped from the fragments is disabled, never uninstalled, and the sync reports it.
  Uninstalling is a deliberate `claude plugin uninstall`, since it deletes the plugin's data.
- The state file is per machine and is never committed.

## The other bundled skills

`qbranch --skill review-plugins` decides the fate of plugins installed here but declared
nowhere, one structured question each. `qbranch --skill agent-audit` walks an audit's
findings the same way. Both assume the setup this skill describes.
