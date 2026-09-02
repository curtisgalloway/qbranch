# qbranch

The sync tool that outfits a machine's coding agents from a per-machine manifest. `README.md`
is the user-facing description; this file is for an agent working on the tool itself.

## Terms

See `GLOSSARY.md` for every term used here (manifest, config root, settings fragment, skill
repo, managed plugin, plan, corpus, schema).

## Layout

```
bin/qbranch            the reference implementation: one Python 3.9+ script, stdlib only
src/                   the Rust port, function for function; Cargo.toml at the root
  main.rs              CLI (clap) and dispatch        sync.rs      plan + apply
  ctx.rs               constants, harness paths       skills.rs    desired links, skill repos
  manifest.rs          load, migrate, edit            settings.rs  merge, assert, retract
  state.rs             the state file                 plugins.rs   reconcile, status, manage
  audit.rs             --audit                        paths.rs     pathlib/os.path parity
  proc.rs              subprocesses with timeouts     util.rs      JSON I/O, Python repr()
tests/run_corpus.py    runs the corpus; --show CASE, --bless, --apply; QBRANCH_BIN=<binary>
                       for the port
tests/run_parity.py    applies every case for real with both implementations and diffs the
                       results, plus the --plugin-status / --audit / --list output
tests/fake-bin/claude  stand-in for the claude CLI, driven by a case's claude.json
tests/corpus/<case>/   fixture root + home + expected plan; see run_corpus.py's docstring
skills/                the tool's own skills: review-plugins, agent-audit
GLOSSARY.md            the vocabulary; add a term before using it in a document
HANDOFF.md             session handoff, untracked; read it first when it exists
```

## Rules of the road

- **The corpus is the contract.** Every behaviour change gets a case, and a diff in
  `expected.json` is read before it is blessed. Both implementations must produce the same plan
  on every case. Before every commit run `python3 tests/run_corpus.py` and, after
  `cargo build --release`, `QBRANCH_BIN=target/release/qbranch python3 tests/run_corpus.py`.
- **Two implementations, one behaviour.** Until the first release the Python script is the
  reference and the Rust port mirrors it function for function, message for message. A
  behaviour change lands in both in the same commit; when the reference does something odd,
  fix the reference, do not port the oddity. The corpus only compares dry-run plans, so
  `python3 tests/run_parity.py` (after `cargo build --release`) applies every case for real
  with each implementation and diffs what they leave behind; run it too before a commit
  that touches the apply path, the state file or a report.
- **Link modes are always tested.** Every change to how entries are materialised gets a
  case in both modes (`copy-mode`, `copy-to-symlink` and the `link` cases are the pattern).
  `python3 tests/run_corpus.py --apply` applies every case for real and requires the next
  dry run to find nothing left to do; CI runs it on Linux, macOS and Windows in both
  modes (`QBRANCH_LINK_MODE=copy` for the second), so links and copies are actually made
  on every platform. The automatic Windows fallback is a single probe on top of the same
  code.
- **Links, never copies** above means qbranch never writes a file *of its own* into a home
  directory besides the state file. A copy made in copy mode is a copy of the source,
  recorded in the state file so it can be refreshed and removed.
- **Known, deliberate differences.** The port sends its two stray warnings (a failed
  `git pull`, an unparseable marketplace.json) to stderr, where the script prints them to
  stdout, so a `--dry-run --json` plan on stdout stays parseable. Its `--help` is clap's
  layout, not argparse's. Everything else, including JSON escaping and the text output of
  every mode, is byte for byte the same; `tests/run_parity.py` is the check.
- **Manifest compatibility is forward-only.** Bumping the schema means adding a migration in
  `migrate_manifest` that materializes the old implicit behaviour, so an upgraded manifest does
  exactly what it did before. A manifest newer than the tool is refused. Never write a
  downgrade.
- **The app owns its settings file.** qbranch asserts policy keys into it and retracts what it
  asserted; it never symlinks it, never rewrites keys the fragments don't mention, and never
  hand-edits plugin install state. Plugins are read and changed only through the `claude
  plugin` CLI.
- **Links, never copies.** The only file qbranch writes in a home directory is the state file;
  everything else is a symbolic link into a checkout, or in copy mode a tracked copy of one.
  On Windows links need Developer Mode; without it the default `auto` mode falls back to
  copies and says so.
- **No private content.** This repo becomes public. Hostnames, vault items, personal paths and
  the like belong in the config root, never in the tool, its docs or its fixtures. That also
  rules out upgrade shims for layouts only the maintainer's machines ever had: the tool
  migrates from its own released layouts, not from its prehistory.
- **Gloss borrowed vocabulary.** Every document opens with a Terms block for the handful of
  terms it leans on and points at `GLOSSARY.md`; a new term goes into the glossary first.
- **Headers.** Executable files carry the two-line SPDX header; Markdown carries none. `LICENSE`
  is Apache 2.0.

## Plan of record

1. Settle behaviour and the manifest schema in Python, in daily use across the maintainer's
   machines, growing the corpus as cases arise.
2. Port to Rust against the corpus: one static binary per platform (macOS and Linux on both
   architectures, Windows), release builds in CI, `cargo install` and GitHub releases. The
   port passes the corpus on macOS (2026-09-01) and type-checks for Windows and Linux. CI
   (`.github/workflows/ci.yml`) runs the corpus on all three; `release.yml` builds the five
   binaries on a version tag. Neither has run yet: both wait for the repo to have a remote.
3. First public push only once the Rust binary exists, so the Python version never ships.
   Until then the remote stays unset.

## Working here

- Python: stdlib only, Google style at 88 columns, `pyink` formatting. No third-party
  dependencies; the tool must run on a fresh machine with nothing but Python.
- Rust: edition 2021, `cargo fmt`, `cargo clippy` clean. Dependencies are `clap` and
  `serde_json` (with `preserve_order`, so settings files keep their key order); add another
  only with a reason a stdlib solution cannot meet. No async. JSON is handled as
  `serde_json::Value` throughout, because the settings merge operates on arbitrary app-owned
  documents and the Python reference does the same with dicts.
- Commit locally; push only when the maintainer says so. The first push is gated on step 3.
- The maintainer's own config root is a private repo; test against the corpus, not against it,
  unless asked.
