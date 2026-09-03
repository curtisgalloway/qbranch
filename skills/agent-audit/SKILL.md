---
name: agent-audit
description: Run an overall health review of this machine's agent setup — linked skills, Claude Code plugins and MCP servers — against the active manifest, and walk the findings with structured questions: skill-name collisions, plugins whose skills are also linked from a checkout, MCP servers a plugin duplicates, unmanaged or disabled plugins, repo skills the manifest does not link, and the always-loaded context budget. Use when the user asks to audit, review, tidy, or slim down their skills or plugins, wonders whether they have too many or conflicting skills, or after a batch of installs.
---

# Audit the agent setup

The sync tool `qbranch` (on PATH, or `~/src/qbranch/bin/qbranch`; `qbranch --skill qbranch`
explains the setup this skill assumes) can inventory everything it knows about on this machine and flag what needs a human decision.
This skill runs that inventory and turns each finding into a structured question, then applies
the answers through the tool's own flags and the `claude` CLI. Nothing is decided for the user.

## Procedure

1. **Run the audit.**

   ```bash
   qbranch --audit --json
   ```

   The JSON has `budget`, `skills`, `plugins`, `mcp`, and a `findings` list of
   `{kind, severity, message, items}`. Show `errors` and stop if it is non-empty.

2. **Lead with the numbers.** Report the budget in one short table: skills linked, always-loaded
   description tokens, enabled plugins and their always-on tokens, user-scope MCP servers. Then
   the list of finding kinds with counts. Use the harness's structured-question tool
   (`AskUserQuestion` in Claude Code) to ask which findings to work through, multi-select, in
   the order below. Skip kinds with no items.

3. **Walk each chosen kind.** One structured question per item, or per small batch of the same
   kind (at most four per call). Options and what applying them means:

   | Kind | Options | Apply |
   | --- | --- | --- |
   | `mcp-duplicate` | Drop the user-scope server (recommended) / Keep both / Leave | `claude mcp remove <name> -s user` |
   | `double-load` | Uninstall the plugin / Narrow the checkout to other themes / Leave | `claude plugin uninstall <id> --scope user`; or edit the manifest's `skill_repos` entry to list only the plugins wanted from that repo (jq on `manifests/<name>.json` in the config root), then sync |
   | `skill-collision` | Keep the first source (no change) / Prefer the other source / Leave | To prefer the repo copy over the manifest's: `qbranch --remove-skill <name>`; to prefer the manifest's, nothing (it already wins) |
   | `plugin-skill-collision` | Disable the plugin / Drop the linked skill from the manifest / Leave | `claude plugin disable <id>`; or `qbranch --remove-skill <name>` |
   | `unmanaged-plugins` | Review them now / Later | Invoke the `review-plugins` skill, which owns that flow |
   | `dropped-plugins` | Uninstall / Re-manage on this host / Leave | `claude plugin uninstall <id> --scope user` (confirm first); or `qbranch --manage-plugin <id> --in host --value true` |
   | `disabled-plugins` | Uninstall / Leave | `claude plugin uninstall <id> --scope user`, after a second confirmation because it deletes the plugin's data directory (`--keep-data` keeps it) |
   | `unlinked-repo-skills` | Link into this manifest / Link into every manifest / Leave | `qbranch --add-skill <name>` or `... --add-skill <name> --all` |
   | `missing-skill-repos` | Clone it / Drop it from the manifest / Leave | `git clone` to the listed path; or edit `skill_repos` in the manifest |
   | `context-budget` | informational | Show the largest descriptions and remind that a description is 1 to 3 sentences of triggers; trimming is a skill-authoring change, not something to do here |

   Say in each question text that "Other" with "leave" is the same as the Leave option.

4. **Apply, then re-run.** After the answers are applied run `qbranch` (so manifest
   changes land) and `qbranch --audit` again, and show what changed: findings that went
   away and the new budget numbers.

5. **Close.** Summarize per kind what was done. Manifest and fragment edits reach other machines
   at their next sync once committed: commit them locally with a message that records the
   decisions; push only if the user asks, following the repo's push rules.

## Rules

- The tool's flags and the `claude` CLI are the only writers. Never hand-edit `settings.json`,
  the settings fragments, or `~/.claude.json`; the one manifest edit allowed here is the
  `skill_repos` entry, via jq, when the user chose to narrow a repo.
- Uninstalls and MCP removals need their own confirmation question before running.
- "Too many skills" is a judgement the user makes with the numbers in front of them. Present
  the budget and the largest contributors; do not prune on your own initiative.
- Project-scope plugins and project-scope MCP servers are out of scope; the audit reads user
  scope only.
