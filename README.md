# qbranch

Q Branch outfits agents with their gear. This tool outfits the coding agents on a machine, Claude
Code and Google Antigravity today, with the skills, instruction files, hooks, settings and
plugins a per-machine manifest says they should have, and keeps every machine you use in step.

**Status: early.** The tool is in daily use on the maintainer's machines (macOS and Linux;
Windows is built and tested in CI but has had less real use). The released form is a single
Rust binary. A Python script (`bin/qbranch`) is kept as the reference implementation the test
corpus is written against; the two are held byte-for-byte identical in behaviour by CI.

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
   directory at it. Symbolic links by default; copies where links are unavailable (see the
   Windows note below).
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

## Installing

Download the binary for your platform from the
[releases page](https://github.com/curtisgalloway/qbranch/releases) and put it on your `PATH`,
or build it with a Rust toolchain:

```bash
cargo install --git https://github.com/curtisgalloway/qbranch
```

Then point it at your config root once:

```bash
qbranch --root ~/src/my-agent-config --manifest laptop
```

The reference script needs nothing but Python 3.9+ and runs straight from a checkout:

```bash
git clone https://github.com/curtisgalloway/qbranch.git ~/src/qbranch
~/src/qbranch/bin/qbranch --root ~/src/my-agent-config --manifest laptop
```

Both the config root and the manifest name are remembered, so afterwards a plain `qbranch`
re-syncs. Without either, qbranch looks for `manifests/` in the current directory and for a
manifest named after the machine, so `cd my-agent-config && qbranch` works on a machine whose
manifest carries its hostname.

**Windows.** qbranch makes symbolic links, which Windows allows once Developer Mode is on.
Without it, qbranch falls back to copying each entry and refreshing the copies on every sync;
`--link-mode copy` asks for that anywhere (a filesystem without symlinks, say) and
`--link-mode symlink` insists on links. The choice is remembered.

Everyday commands:

```bash
qbranch                       # sync with the remembered root and manifest
qbranch --dry-run             # show the plan, change nothing
qbranch --dry-run --json      # the same plan as JSON
qbranch --link-mode copy      # copies instead of links, from now on; --link-mode auto forgets
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
python3 tests/run_corpus.py                                      # the Python reference
cargo build --release && QBRANCH_BIN=target/release/qbranch python3 tests/run_corpus.py
python3 tests/run_parity.py    # apply every case with both and diff what they leave behind
python3 tests/run_corpus.py --apply           # apply for real; the next dry run must be a no-op
QBRANCH_LINK_MODE=copy python3 tests/run_corpus.py --apply   # the same in copy mode
```

Runs every case under `tests/corpus/` in a temporary copy with `HOME`, `CLAUDE_CONFIG_DIR`,
`QBRANCH_ROOT` and `PATH` pointed inside it and a fake `claude` answering the plugin queries, so
nothing on the machine is read or touched. The corpus is the specification both implementations
must satisfy; see `AGENTS.md` for how to extend it.

## License

Apache 2.0 — see [LICENSE](LICENSE).
