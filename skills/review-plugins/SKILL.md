---
name: review-plugins
description: Triage the Claude Code plugins installed on this machine that no settings fragment in the config repo manages, deciding each one's fate with structured questions — manage everywhere, manage on this host only, pin it off here, or uninstall. Use when the user asks to review, triage, or clean up plugins, when `qbranch` reports unmanaged or no-longer-managed plugins, or after a plugin installed from `/plugin` should start following the user to other machines.
---

# Review unmanaged plugins

The sync tool `qbranch` (on PATH, or `~/src/qbranch/bin/qbranch`; `qbranch --skill qbranch`
explains the setup this skill assumes) makes Claude Code plugins follow the user between machines when a
settings fragment declares them. A plugin installed here but declared nowhere is
**unmanaged**: it stays local, and the sync only reports it. This skill turns that report
into decisions, one structured question per plugin, and applies them through the tool's
own flags so the fragments are never edited by hand.

`qbranch` remembers the config root and the manifest from the last sync, so no path needs
resolving; the config repo is wherever that root is.

## Procedure

1. **Get the report.**

   ```bash
   qbranch --plugin-status --json
   ```

   The JSON has `unmanaged` and `dropped` lists (each entry: `id`, `version`, `enabled`,
   `marketplace`, `marketplace_declared`), plus `base_fragment`, `host_fragment` and
   `manifest`. `dropped` holds plugins a fragment declared at the last sync and no longer
   does; they are disabled by the retraction but still on disk. If `errors` is non-empty,
   show it and stop. If both lists are empty, say so and stop.

2. **Ask, one structured question per plugin.** Use the harness's structured-question
   tool (`AskUserQuestion` in Claude Code), batching up to four plugins per call.

   - Header: the plugin's short name, cut to 12 characters.
   - Question: `<id> v<version> is installed here (<enabled|disabled>) and unmanaged.
     What should happen to it?` For a `dropped` entry say instead `...was dropped from
     the fragments and is now disabled here. What should happen to it?`
   - Say in the question text that answering "Other" with "leave" keeps it as is.
   - Options, in this order, with these descriptions:
     1. **Manage everywhere** — declared `true` in the base fragment
        (`<base_fragment>`); every machine on that fragment installs and enables it at
        its next sync.
     2. **Manage on this host only** — declared `true` in the host fragment
        (`<host_fragment>`, created and wired into the manifest if there is none).
     3. **Pin off on this host** — declared `false` in the host fragment; stays
        installed, disabled here, and every future sync keeps it that way.
     4. **Uninstall** — removed from this machine.

   If an entry has `marketplace_declared: false`, mention that options 1 and 2 will also
   declare its marketplace so other machines can install it.

3. **Apply each answer.**

   | Answer | Command |
   |---|---|
   | Manage everywhere | `qbranch --manage-plugin <id> --in base --value true` |
   | Manage on this host only | `qbranch --manage-plugin <id> --in host --value true` |
   | Pin off on this host | `qbranch --manage-plugin <id> --in host --value false` |
   | Uninstall | see below |
   | Leave | nothing |

   Uninstalling deletes `~/.claude/plugins/data/<id>/`, so first ask one more structured
   question: **Uninstall and delete its data** / **Uninstall but keep its data** /
   **Cancel**. Then run `claude plugin uninstall <id> --scope user`, adding `--keep-data`
   for the second answer.

   Relay every line the commands print. A line starting with `ERROR:` (most likely a
   marketplace that is not registered here) means the plugin was declared but its
   marketplace was not, so other machines cannot install it until the marketplace is
   added to that fragment by hand; say so rather than working around it.

4. **Apply and show.** Run `qbranch` and relay its plugin-related lines: the
   newly managed plugins are now asserted into `settings.json`, pinned-off ones show as
   disabled, and nothing should install here because the plugins already are.

5. **Close.** Summarize what changed, per fragment. The fragment and manifest edits reach
   other machines at their next sync once committed: commit them locally with a message that
   records the decisions; push only if the user asks, following the repo's push rules.

## Rules

- `--manage-plugin` is the only writer of the fragments; never edit them or
  `settings.json` by hand during this skill.
- Never uninstall without the second confirmation.
- The report covers user-scope plugins only. Project-scope plugins belong to a repo's own
  `.claude/settings.json` and are out of scope here.
- Do not decide for the user. A plugin that looks obviously host-specific still gets its
  question; the description can say why one option seems apt.
