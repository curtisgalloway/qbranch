# qbranch

The sync tool that outfits a machine's coding agents from a per-machine manifest. `README.md`
is the user-facing description; this file is for an agent working on the tool itself.

## Terms

See `GLOSSARY.md` for every term used here (manifest, config root, settings fragment, skill
repo, managed plugin, plan, corpus, schema).

## Layout

```
bin/qbranch            the tool: one Python 3.9+ script, stdlib only
tests/run_corpus.py    runs the corpus; --show CASE, --bless
tests/fake-bin/claude  stand-in for the claude CLI, driven by a case's claude.json
tests/corpus/<case>/   fixture root + home + expected plan; see run_corpus.py's docstring
skills/                the tool's own skills: review-plugins, agent-audit
GLOSSARY.md            the vocabulary; add a term before using it in a document
HANDOFF.md             session handoff, untracked; read it first when it exists
```

## Rules of the road

- **The corpus is the contract.** Every behaviour change gets a case, and a diff in
  `expected.json` is read before it is blessed. A port passes when every case produces the same
  plan. Run `python3 tests/run_corpus.py` before every commit.
- **Manifest compatibility is forward-only.** Bumping the schema means adding a migration in
  `migrate_manifest` that materializes the old implicit behaviour, so an upgraded manifest does
  exactly what it did before. A manifest newer than the tool is refused. Never write a
  downgrade.
- **The app owns its settings file.** qbranch asserts policy keys into it and retracts what it
  asserted; it never symlinks it, never rewrites keys the fragments don't mention, and never
  hand-edits plugin install state. Plugins are read and changed only through the `claude
  plugin` CLI.
- **Links, never copies.** The only file qbranch writes in a home directory is the state file;
  everything else is a symbolic link into a checkout. On Windows this needs Developer Mode, and
  the Rust port will carry a copy-mode fallback for it.
- **No private content.** This repo becomes public. Hostnames, vault items, personal paths and
  the like belong in the config root, never in the tool, its docs or its fixtures.
- **Gloss borrowed vocabulary.** Every document opens with a Terms block for the handful of
  terms it leans on and points at `GLOSSARY.md`; a new term goes into the glossary first.
- **Headers.** Executable files carry the two-line SPDX header; Markdown carries none. `LICENSE`
  is Apache 2.0.

## Plan of record

1. Settle behaviour and the manifest schema in Python, in daily use across the maintainer's
   machines, growing the corpus as cases arise.
2. Port to Rust against the corpus: one static binary per platform (macOS and Linux on both
   architectures, Windows), release builds in CI, `cargo install` and GitHub releases.
3. First public push only once the Rust binary exists, so the Python version never ships.
   Until then the remote stays unset.

## Working here

- Python: stdlib only, Google style at 88 columns, `pyink` formatting. No third-party
  dependencies; the tool must run on a fresh machine with nothing but Python.
- Commit locally; push only when the maintainer says so. The first push is gated on step 3.
- The maintainer's own config root is a private repo; test against the corpus, not against it,
  unless asked.
