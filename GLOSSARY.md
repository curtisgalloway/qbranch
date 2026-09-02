# Glossary

Plain-language definitions of every term qbranch borrows from the agent harnesses it manages
and every term it coins. Documents in this repo gloss a term on first use; this file is the
reference.

**Agent.** A coding assistant that runs in a terminal or editor, such as Claude Code or Google
Antigravity. Also called a harness when the point is the program rather than the model.

**Agent-neutral skills directory.** `~/.agents/skills/`, the one place qbranch links skills into.
Each harness's own skills directory is pointed at it, so every harness sees the same set.

**Antigravity.** Google's coding agent (the `agy` CLI and IDE). It keeps its state under
`~/.gemini/`.

**Capability fragment.** A settings fragment tied to an optional feature rather than to one
machine, listed only by the manifests of machines that have the feature. Example: the fragment
that declares the CAD plugins, listed by the two Macs.

**Claude Code.** Anthropic's coding agent. It keeps its state under `~/.claude/` (or the
directory named by `CLAUDE_CONFIG_DIR`).

**Config root.** The directory holding `manifests/`, settings fragments, and any skills or
instruction files that qbranch links from. Set with `--root` or `QBRANCH_ROOT`; remembered after
the first sync. The manifest variable `${QBRANCH_ROOT}` expands to it.

**Corpus.** The test cases under `tests/corpus/`, each a fixture config root and home directory
with the plan the tool must produce. It is the specification a port of the tool must satisfy.

**Double load.** The same skills loading twice because a plugin is installed and enabled while
its skills are also linked from a checkout. qbranch warns about it.

**Dry run.** `--dry-run`: compute and print the plan without changing anything. `--json` prints
that plan as JSON.

**Fragment.** See settings fragment.

**Host fragment.** A settings fragment for one machine, under `settings/hosts/<name>.json`,
listed last in that machine's manifest so its values win.

**Link.** A symbolic link qbranch creates from a destination the harness reads (a skills
directory, `~/.claude/CLAUDE.md`, a hook file) to a source in a checkout. Links are the only
things qbranch creates for files; it never copies them.

**Managed plugin.** A plugin that some settings fragment declares, as `true` (install and
enable) or `false` (keep disabled). Only managed plugins follow the user between machines.

**Manifest.** A JSON file under `manifests/` in the config root that says what should be active
on the machines that use it: skills, skill repos, links, settings fragments. One per machine or
per role. Carries a `schema` number.

**Marketplace.** Claude Code's catalog format for plugins: a repo with
`.claude-plugin/marketplace.json` listing plugins and where each one's files are. qbranch reads
marketplaces both to install plugins and to discover skills in a checkout.

**Plan.** What a sync would do: the link actions, the plugin actions, the settings changes and
the notes. `--dry-run` prints it; the corpus compares it.

**Plugin.** Claude Code's unit of installable extension: skills, hooks, MCP servers, LSP servers
and agents packaged together, installed from a marketplace. Identified as
`<plugin>@<marketplace>`.

**Policy.** The merged result of a manifest's settings fragments: the keys qbranch asserts into
the app-owned settings file. Everything else in that file is app state and left alone.

**Retraction.** Removing from the live settings file what an earlier policy asserted and the
current one no longer does, unless the app changed that value in the meantime. The state file
records the last applied policy to make this possible.

**Schema.** The version number a manifest carries. Older manifests are upgraded in memory on
every load; a manifest newer than the tool is refused. Forward-only.

**Settings fragment.** A JSON file holding the part of a harness's settings that should follow
the user between machines: permission rules, hooks, marketplaces, plugins. Fragments are merged
in manifest order and imposed on the harness's own settings file, which stays app-owned.

**Skill.** A directory with a `SKILL.md` that an agent loads on demand. The file's frontmatter
`description` is read into every session; the body only when the skill is invoked.

**Skill repo.** A checkout listed under `skill_repos` in a manifest, whose skills are linked
without per-skill entries. A marketplace-shaped repo is read plugin by plugin, so a manifest can
pick themes; a plain repo contributes `skills/*/SKILL.md`.

**Skills target.** The directory `skills` entries are linked into: the agent-neutral skills
directory by default. The state file lives there.

**State file.** `.qbranch-state.json` in the skills target: the manifest name, the config root,
every link the last sync created, and the settings policy it applied. Older syncs wrote
`.agent-skills-state.json`, which is read and replaced.

**Theme.** A plugin in a marketplace-shaped skills repo that groups related skills, such as
`hardware-lab` in public-skills.

**Unmanaged plugin.** A plugin installed on a machine that no settings fragment declares. It
stays local; qbranch reports it and the review-plugins skill triages it.
