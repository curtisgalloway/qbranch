# qbranch

Q Branch outfits agents with their gear. This tool outfits the coding agents on a machine, Claude
Code and Google Antigravity today, with the skills, instruction files, hooks, settings and
plugins a per-machine manifest says they should have, and keeps every machine you use in step.

**Status: pre-release.** The tool works and is in daily use across four machines, but it is a
Python script that will be ported to Rust, with a test corpus as the contract, before its first
public release. Until then it is used from a checkout.

## Terms

- **Manifest**: a JSON file, one per machine or role, that says what should be active there.
- **Config root**: the directory holding the manifests, the settings fragments and any skills
  or instruction files the manifests link from. Usually a private git repo.
- **Settings fragment**: the part of a harness's settings that should follow you between
  machines, merged into the harness's own settings file without taking that file over.
- **Skill repo**: a checkout whose skills are linked in bulk, optionally by theme.
- **Managed plugin**: a Claude Code plugin some fragment declares; only those follow you.

The full vocabulary is in [GLOSSARY.md](GLOSSARY.md).

## What a sync does

1. Links every skill the manifest names, plus every skill each listed skill repo offers, into
   the agent-neutral skills directory `~/.agents/skills/`, and points each harness's own skills
   directory at it. Links only; nothing is copied.
2. Links instruction files and hooks wherever the manifest says (`~/.claude/CLAUDE.md`,
   `~/.gemini/AGENTS.md`, hook scripts), skipping destinations for a harness that isn't
   installed.
3. Merges the manifest's settings fragments into each harness's own settings file: policy keys
   are asserted, keys the fragments never mention are left to the app, and anything an earlier
   sync asserted that the fragments no longer carry is retracted.
4. Registers the Claude Code marketplaces and installs the plugins the merged fragments
   declare, through the `claude plugin` CLI, and reports plugins that are installed but
   declared nowhere.
5. Records what it did in a state file, so the next sync removes links that are no longer
   wanted and retracts settings that are no longer declared.

Every step is planned first; `--dry-run` shows the plan and changes nothing.

## Using it from a checkout

```bash
git clone git@github.com:curtisgalloway/qbranch.git ~/src/qbranch
~/src/qbranch/bin/qbranch --root ~/src/my-agent-config --manifest laptop
```

Both the config root and the manifest name are remembered, so afterwards a plain `qbranch`
re-syncs. Put `~/src/qbranch/bin` on your `PATH` or symlink `bin/qbranch` into `~/.local/bin`.

Everyday commands:

```bash
qbranch                       # sync with the remembered root and manifest
qbranch --dry-run             # show the plan, change nothing
qbranch --dry-run --json      # the same plan as JSON
qbranch --list                # manifests in the config root
qbranch --plugin-status       # managed / unmanaged plugins on this machine
qbranch --manage-plugin <id> --in base|host [--value false]
qbranch --audit               # collisions, double loads, duplicate MCP servers, context budget
qbranch --add-skill <name> | git://<repo>/<path>    # add a skill to the remembered manifest
qbranch --upgrade-manifests   # rewrite older manifests at the current schema
qbranch --version
```

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

A manifest:

```json
{
  "schema": 2,
  "name": "laptop",
  "skill_repos": [
    { "path": "${HOME}/src/public-skills", "plugins": ["dev-tools", "agent-workflow"] },
    { "path": "${HOME}/src/private-skills" }
  ],
  "skills": [
    { "name": "push", "repo": "git@github.com:me/private-skills.git", "path": "skills/push" }
  ],
  "links": [
    { "src": "${QBRANCH_ROOT}/claude-code/CLAUDE.md", "dst": "${HOME}/.claude/CLAUDE.md" }
  ],
  "claude_settings": [
    "${QBRANCH_ROOT}/claude-code/settings.json",
    "${QBRANCH_ROOT}/claude-code/settings/hosts/laptop.json"
  ]
}
```

Manifests carry a schema number. An older manifest always works with a newer tool, upgraded in
memory and rewritten by `--upgrade-manifests`; a manifest newer than the tool is refused with a
pointer to update the tool. The tool's own `--help` and the docstring at the top of `bin/qbranch`
document every key.

## Skills that ship with it

`skills/review-plugins` walks the plugins a machine has installed but no fragment manages,
deciding each one's fate with structured questions. `skills/agent-audit` runs the overall audit
and walks its findings the same way. List this checkout as a skill repo in a manifest to link
them.

## Tests

```bash
python3 tests/run_corpus.py
```

Runs every case under `tests/corpus/` in a temporary copy with `HOME`, `CLAUDE_CONFIG_DIR`,
`QBRANCH_ROOT` and `PATH` pointed inside it and a fake `claude` answering the plugin queries, so
nothing on the machine is read or touched. The corpus is the specification the Rust port must
satisfy; see `AGENTS.md` for how to extend it.

## License

Apache 2.0 — see [LICENSE](LICENSE).
