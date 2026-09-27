---
name: claude-apps-sync
description: Sync a manifest's skills to the Claude apps (claude.ai and the desktop app), which take skills by upload rather than from a directory — export them with `qbranch --export-zips`, upload the new and changed ones through the claude.ai Settings page in Chrome, and turn off the ones the manifest dropped. Use when the user says "sync my chat skills", "update my claude.ai skills", "upload my skills to Claude", or wants the skills qbranch manages to follow them into the apps.
---

# Sync skills to the Claude apps

The Claude apps install skills from uploaded zip files in Settings; nothing on disk can be
linked into them. `qbranch --export-zips DIR` writes one zip per skill and says which are
new, changed or current; this skill uploads the ones that are not current through the
claude.ai Settings page in Chrome, and records each successful upload so the next export
knows about it. `qbranch --skill qbranch` explains the tool and the manifest this assumes.

**Terms.** *Claude apps*: claude.ai in a browser and the desktop app, which share one
account's skills. *Export directory*: the folder `--export-zips` writes, by default
`~/.agents/claude-apps/`. *Apps manifest*: the manifest that lists the skills meant for
the apps. Full definitions are in qbranch's `GLOSSARY.md`.

## Before the first sync

- **A qbranch with `--export-zips`.** Releases up to 0.4.0 do not have it. Check with
  `qbranch --help`; if the flag is missing, build qbranch from source (`cargo build
  --release` in a checkout) and run that binary until a release carries it.
- **Browser tools.** The session needs Claude in Chrome, with its file-upload tool, and
  Chrome signed in to claude.ai. This skill never signs in or types a password; if the
  page asks for a login, stop and ask the user to sign in themselves.
- **Choose the manifest.** A skill uploaded to the apps also loads in Claude Code sessions
  on the same account. On a machine where qbranch already links skills for Claude Code,
  uploading the same ones makes each load twice, so keep a separate apps manifest,
  `manifests/claude-apps.json` in the config root, listing only what belongs in the apps.
  Someone who does not use Claude Code can export their machine's own manifest instead.

  To create the apps manifest, list the skills this machine links (`ls
  ~/.agents/skills/`), ask the user which should go to the apps (structured questions,
  several skills per question), and write:

  ```json
  {
    "schema": 2,
    "name": "claude-apps",
    "description": "Skills uploaded to the Claude apps by the claude-apps-sync skill",
    "skill_repos": [],
    "skills": [
      { "name": "<skill>", "path": "<its source directory>" }
    ],
    "links": []
  }
  ```

  A skill's source directory is where its link in `~/.agents/skills/` points (`readlink`).
  Write it with `${HOME}` or `${QBRANCH_ROOT}` in place of the literal prefix, the way the
  other manifests do, and commit the file to the config root.

## Procedure

1. **Export.**

   ```bash
   qbranch --export-zips ~/.agents/claude-apps --manifest claude-apps --json
   ```

   Leave out `--manifest claude-apps` when exporting the machine's own manifest. The JSON
   lists `skills` (each with `name`, `zip` and `status`: `new`, `changed` or `current`),
   `dropped`, `warnings` and `errors`. If `errors` is non-empty, show it and stop. If every
   skill is `current` and `dropped` is empty, say the apps are up to date and stop.

2. **Confirm.** Uploading changes the user's account, so show what will happen, as three
   short lists: to upload (`new`), to replace (`changed`), to turn off (`dropped`). Ask one
   structured question: **Sync all** / **Choose skills** (then one question per batch of up
   to four) / **Cancel**. Only confirmed skills go further.

3. **Open the Skills page.** Load the browser tools, get the tab context, open a new tab
   and navigate to `https://claude.ai/new#customize/skills/yours`. A fresh page load opens
   Settings on the Skills list; take a screenshot to confirm. A login page means stop (see
   above).

4. **Upload each skill, one at a time.** For each confirmed `new` or `changed` skill:

   1. Open the upload form: **Add** (top right of the Skills list), then **Upload skill**.
      Changing the URL to `#customize/skills/new/upload` does not work once Settings is
      already open; use the menu.
   2. Find the file input with the find tool: it is labelled "Skill file" and accepts
      `.zip,.skill,.md`. **Never click it**, since that opens the operating system's file
      picker, which the browser tools cannot see. Put the zip into it with the file-upload
      tool, using an absolute path. The tool uploads only files the session is allowed to
      share, and the export directory is usually not one of them (the refusal says "only
      files this session is allowed to read can be uploaded"). Copy the zip into the
      session's own temporary or scratch directory first and upload that copy; the bytes
      are the same, so step 7 still records the original from the export directory.
   3. Check the **Preview** box: the name must be the skill's name. The apps name a skill
      from its `SKILL.md` frontmatter, not the folder, so a different name means the
      manifest entry and the frontmatter disagree; skip that skill and report it.
   4. Click **Upload**. Nothing is sent until then; the security scan runs at this point.
   5. A dialog **Replace "<skill>" skill?** means the account already has a skill of that
      name. For a `changed` skill that is expected: click **Upload and replace** (the
      earlier version stays in the skill's version history). For a `new` skill it is not:
      the account has a same-named skill this sync did not upload, perhaps made by hand or
      turned off earlier. Click **Cancel** and ask the user whether to replace it.
   6. Confirm success: a notice "Uploaded <skill>" or "Replaced <skill>" and the skill's
      page. The page does not refresh itself after a replace and may still show the old
      version; that is expected. A security-scan rejection or any error message means the
      upload failed: report the message and go on to the next skill.
   7. **Only after a confirmed success**, record it:

      ```bash
      mkdir -p ~/.agents/claude-apps/uploaded
      cp ~/.agents/claude-apps/<skill>.zip ~/.agents/claude-apps/uploaded/<skill>.zip
      ```

      A skill that failed is not copied, so the next export offers it again.
   8. Return to the list with **Your skills** (top left of the skill page).

5. **Turn off each dropped skill.** In the Skills list, find the skill's row and use
   **Turn off**, from its **⋮** menu or the button that appears on the row. **Never click
   Remove**: it permanently deletes the skill and its version history. If the user wants
   the skill gone, tell them to remove it themselves. Once it is off, delete
   `~/.agents/claude-apps/uploaded/<skill>.zip`, which only records the upload, so the next
   export stops listing the skill as dropped.

6. **Verify.** Reload the page (a full reload, not a URL change) and check that every
   uploaded skill is listed and switched on; a replaced skill should show the new version
   in its version picker. Then export again: every synced skill should now be `current` and
   `dropped` empty. Close the tab you opened.

7. **Report**, briefly: uploaded, replaced, turned off, and anything that failed with its
   message. If the apps manifest was created or changed, commit it to the config root.

## Rules

- One skill per upload, even though the form accepts several files: a replace dialog, a
  scan result or an error then belongs to a single known skill.
- Record an upload in `uploaded/` only after the page confirms it. The copies are the only
  record of what the apps hold; a copy for a failed upload hides the skill from every later
  sync.
- Never click Remove, never enter credentials, never click the file input.
- The Settings page is a web page that changes without notice. If a label or button this
  skill names is missing, stop and describe what the page shows instead of guessing; the
  same after two failed attempts at any one step.
