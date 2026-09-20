# Release train profile: qbranch

Derived from commit 2f7054c on 2026-09-19. Executed by the `release-train`
skill; kept honest by `profile_check.py` (see `## Sources`). Lines marked
`UNVERIFIED` were inferred by the agent that wrote this file and have not been
confirmed by a maintainer or by a passing arm.

## Terms

*Channel*, *arm*, *smoke contract*, *release train* and *regression archaeology* are defined in
`GLOSSARY.md`, along with the qbranch vocabulary this file leans on: manifest, config root,
skill, skills target, state file, link mode, corpus. This file is the release train's *profile*
for qbranch: everything the generic skill needs to know about this project.

## Project

- cli: `qbranch`
- version source: the tag. `resolve version` in the release workflow takes the version from
  `v<X.Y.Z>` and refuses a tag that disagrees with any of the three strings that must match it:
  `version` in `Cargo.toml`, `VERSION` in `bin/qbranch`, and `VERSION` in `src/ctx.rs`. Only
  the last reaches `qbranch --version` from a built binary, so it is the one that can lie.
- tag format: `v<X.Y.Z>`; annotated, subject `qbranch <X.Y.Z>` (both existing tags)
- main branch: `main`; protected: yes — PR required, 0 approvals, strict (the branch must be
  current),
  required checks `check (ubuntu-latest)`, `check (macos-latest)`, `check (windows-latest)`.
  A ruleset named "Protect release tags" is active.
- release workflow: `.github/workflows/release.yml` (on a `v*` tag; `workflow_dispatch` runs the
  same pipeline as artifacts only, no release and no fan-out)
- ci workflow: `.github/workflows/ci.yml`; required checks: the three `check (<os>)` jobs above
- bump rules: this repo does not use conventional commits — subjects are sentences
  ("Clone a public skill repo over https, no account needed"). Classify by what changed, and
  mark every such call `heuristic` in the report: UNVERIFIED — a behaviour change in either
  implementation, a new manifest key, a new CLI flag or a manifest schema bump is **minor**
  while the project is pre-1.0; corpus, test, packaging and documentation-only commits are
  **patch**; a commit that only re-blesses `expected.json` for a version bump is **none**.
- releaser identity: the tag author matches the repo's existing commits, which all use the
  maintainer's GitHub noreply address; take it from `git log -1 --format=%ae` on `main` rather
  than from any configured global identity. This machine has no `user.email` set, so git
  commands need it passed explicitly — see `RELEASE-TRAIN.local.md`.

## Hosts

Roles only. The machines behind them, how to reach them, and any credentials
live in `RELEASE-TRAIN.local.md` (gitignored), one section per role.

| role | needed by | what it must have |
|---|---|---|
| local | source, archive, deb (degraded) | rust toolchain, python3, git, tar, dpkg-deb, gh — all present; the smoke contract runs here |
| linux-container | deb | a container runtime (docker or podman) and a Debian-family image, to `apt install` the package without touching the developer's machine |
| macos-bench | homebrew | macOS, Homebrew, the ability to `brew install` a local formula into a throwaway prefix |
| windows-bench | msi, zip | Windows, `msiexec`, PowerShell reachable non-interactively |

UNVERIFIED: the machine this profile was written on fills `local` only. It has no container
runtime (`docker` and `podman` are both absent), no macOS and no Windows, and no `rustup`
(the distro toolchain has no musl std installed). Until `RELEASE-TRAIN.local.md` names hosts
for the other three roles, `homebrew`, `msi` and `zip` SKIP and `deb` runs degraded.

## Smoke contract

Run against the installed copy, with `HOME` (or the platform equivalent)
pointed at a fresh temp dir and the working directory outside the checkout.

Steps S3 to S5 need a config root. Build the smallest one that exercises a sync, in the temp
`HOME`, before S3, with `MANIFEST=smoke`. qbranch takes no subcommands — every row below is
flags only, and the manifest name is a variable so that a checker reading these rows cannot
mistake a flag's value for a verb:

```
$ROOT/manifests/$MANIFEST.json
    {"schema": 2, "name": "smoke", "links": [],
     "skills": [{"name": "demo", "path": "${QBRANCH_ROOT}/skills/demo"}],
     "claude_settings": [], "agy_settings": []}

$ROOT/skills/demo/SKILL.md
    any content
```

| id | check | pass condition |
|---|---|---|
| S1 | `qbranch --help` and `qbranch --version` | exit 0; `--version` prints exactly the version being released; help lists `--dry-run`, `--list`, `--add-skill`, `--audit`, `--skill` |
| S2 | `qbranch --skill` then `qbranch --skill qbranch` | lists exactly `agent-audit`, `qbranch`, `review-plugins`; printing one emits its `SKILL.md` verbatim. Where the channel also installs the skills as files, `qbranch --skill review-plugins` must `diff` clean against the installed copy — this is what catches a package that forgot a skill |
| S3 | `qbranch --root $ROOT --manifest $MANIFEST --skills-target $HOME/.agents/skills` | exit 0; `$HOME/.agents/skills/demo` resolves to `$ROOT/skills/demo` (a symlink, or a copy under `--link-mode copy`) |
| S4 | the same command again with `--dry-run` | exit 0; every action is `ok` — nothing left to do. Then `$HOME/.agents/skills/.qbranch-state.json` parses and records `manifest: smoke` (the value of `$MANIFEST`) and the root, which is the config/state round trip |
| S5 | `qbranch --audit --json` and `qbranch --plugin-status --json` in that same `HOME` | exit 0 on the audit; both emit parseable JSON on stdout with no harness installed; neither panics nor prints a Rust backtrace. Both exit 0 with no harness installed (checked on `local`, 0.4.0); a panic is a FAIL whatever the exit code |

S5 is the hardware-free diagnostic: both report modes shell out to the `claude` CLI, which is
absent on a throwaway `HOME`, and the point is that they degrade rather than crash.

Verified: this contract was executed by hand against a 0.4.0 release build on the `local` host
on 2026-09-19 — S1 to S5 all green, both report modes exiting 0 with parseable JSON and no
panic. So the contract is known to be runnable; what stays UNVERIFIED is whether each channel's
*installed* copy passes it.

## Channels

One `### <arm>` per channel the project actually ships. Bullets `kind`,
`artifact`, `build`, `install like a user`, `smoke`, `cleanup` are required
(`profile_check.py` refuses an arm without them); `workflow job` is checked
against the release workflow when present.

### source

- kind: source
- artifact: `target/release/qbranch` installed into a throwaway `CARGO_INSTALL_ROOT`
- workflow job: none — this path is documented in `README.md`, not built by the workflow
- host: local
- build: `cargo build --release --locked`
- install like a user: `CARGO_INSTALL_ROOT=$(mktemp -d) cargo install --locked --path .`, which
  is what `cargo install qbranch` does once the crate is published. `CARGO_INSTALL_ROOT` is
  required: without it cargo writes to `~/.cargo/bin` and replaces the developer's own copy.
- smoke: S1..S5. A cargo install ships no skill files, so S2 checks only that `--skill` prints
  the three embedded ones.
- cleanup: remove the install root.
- caveats: `README.md` says `cargo install qbranch`, but the crate is not published — the
  `crates` job is gated behind the `CRATES_PUBLISH` repository variable and the first publish
  must be manual. UNVERIFIED: an unauthenticated crates.io lookup answered 403 rather than a
  clear yes or no. Until it is published, this arm proves the from-source path, not the
  registry one, and the README line is wrong.

### archive

- kind: zip
- artifact: `qbranch-v<X.Y.Z>-<target>.tar.gz` (`.zip` on Windows), one top-level directory
  holding `qbranch`, `LICENSE`, `README.md` and `skills/`
- workflow job: build
- host: local
- build: stage the release binary, `LICENSE`, `README.md` and the `skills/` tree into
  `dist/qbranch-v<X.Y.Z>-<target>/` and `tar -czf` it, exactly as the "Package the tarball"
  step does.
- install like a user: extract into a temp dir and put that directory on `PATH`; nothing else.
- smoke: S1..S5, plus: the archive expands to exactly one top-level directory; `skills/` beside
  the executable holds all three `SKILL.md` files; S2's `diff` runs against `skills/` in the
  extracted tree.
- cleanup: remove the staging and extraction directories.
- caveats: the shipped Linux archives are built for `*-unknown-linux-musl` and are fully static.
  UNVERIFIED: this host has no musl std installed and no `rustup` to add one, so the arm builds
  the native `gnu` binary instead. What it tests — the payload shape and that the extracted
  binary finds its skills — is target-independent; that the shipped binary is static is not
  tested here, and `ldd` on the real artifact is the check that belongs on a host that can
  build it.

### deb

- kind: deb
- artifact: `qbranch_<X.Y.Z>_<amd64|arm64>.deb`
- workflow job: build
- host: linux-container, degrading to local
- build: install `nfpm` (the workflow downloads the `.deb` from its GitHub release), stage the
  binary with `install -m 0755 "$BIN" packaging/stage/qbranch` as the workflow does, then
  `ARCH=<arch> VERSION=<X.Y.Z> nfpm package -f packaging/nfpm.yaml -p deb -t <out>`.
- install like a user: `apt install ./qbranch_<X.Y.Z>_<arch>.deb` inside a throwaway container.
  Never on the developer's machine. Where no container runtime is reachable, extract with
  `dpkg-deb -x <deb> <tmpdir>` and run from there, and report the arm PARTIAL: extraction
  exercises the payload but not the package metadata, the dependency resolution or the
  maintainer scripts.
- smoke: S1..S5 against `/usr/bin/qbranch`, plus: `/usr/share/qbranch/skills/<name>/SKILL.md`
  exists for all three, `/usr/share/doc/qbranch/LICENSE` exists, and `dpkg-deb -I` reports no
  `Depends` — the binary is static and the package must stay dependency-free.
- cleanup: remove the container, the staging directory and the built package.
- caveats: the workflow builds `amd64` and `arm64` from the matching musl binaries. A local run
  on one architecture says nothing about the other.

### homebrew

- kind: homebrew
- artifact: the macOS tarballs, consumed by `Formula/qbranch.rb` in `curtisgalloway/homebrew-tap`
- workflow job: tap
- host: macos-bench
- build: none of its own — the formula points at the tarballs the `build` job produced, one URL
  and sha256 per architecture. The tap's own `bump-qbranch-formula.yml` rewrites the version
  line and both pairs on a `qbranch-release` dispatch.
- install like a user: `brew install curtisgalloway/tap/qbranch`. Against an unpublished
  candidate, install the formula from a local file with the URLs pointed at the staged tarballs,
  into a throwaway `HOMEBREW_PREFIX` or a temporary tap.
- smoke: S1..S5, plus: `brew list qbranch` shows the three skills under the keg's own
  `share/qbranch/skills`.
- cleanup: `brew uninstall qbranch` and remove the temporary tap or prefix.
- caveats: UNVERIFIED — no macOS host is reachable from the machine this profile was written on,
  so this arm has never run. The dispatch needs `HOMEBREW_TAP_DISPATCH_TOKEN` in this repo;
  without it the release warns and the tap's workflow is run by hand, so a green release does
  not by itself mean the tap moved.

### msi

- kind: msi
- artifact: `qbranch-v<X.Y.Z>-x86_64-pc-windows-msvc.msi`
- workflow job: windows
- host: windows-bench
- build: `packaging/windows/build.ps1`, which stages the payload and **refuses to build when
  `skills/` holds a skill `Package.wxs` does not list**. That refusal is the check that a new
  skill cannot be left out of the MSI.
- install like a user: `msiexec /i <msi> /qn` — per-user, no elevation, under
  `%LOCALAPPDATA%\Programs\qbranch`. Use a throwaway Windows user profile or a VM snapshot; the
  install puts `bin`
  on the user `PATH`.
- smoke: S1..S5, plus: the executable lands under `%LOCALAPPDATA%\Programs\qbranch\bin`, the
  three skills under `share\qbranch\skills`, and `PATH` picks it up in a fresh shell. Symbolic
  links need Developer Mode on Windows: with it off, S3 must fall back to copies and say so
  rather than fail, which is the platform behaviour worth proving here.
- cleanup: `msiexec /x` the product, then remove the profile or restore the snapshot.
- caveats: UNVERIFIED — no Windows host is reachable, so this arm has never run. Signing is
  Azure Artifact Signing over OIDC and only happens when the `AZURE_SIGNING_ACCOUNT` variable is
  set; until then the MSI ships unsigned and the workflow's verify step is what keeps that
  visible. The `UpgradeCode` and component GUIDs in `Package.wxs` are permanent identity and
  must never be regenerated — an arm that rebuilds the MSI must not touch them.

### zip

- kind: zip
- artifact: `qbranch-v<X.Y.Z>-x86_64-pc-windows-msvc.zip`
- workflow job: windows
- host: windows-bench
- build: the same staged payload as the MSI, zipped: `qbranch.exe`, `LICENSE`, `README.md` and
  `skills/` under one top-level directory.
- install like a user: expand the zip and put the directory on `PATH`.
- smoke: S1..S5, plus the archive-shape checks from the `archive` arm.
- cleanup: remove the expansion directory.
- caveats: UNVERIFIED — no Windows host is reachable. This is the arm the release notes describe
  as "packaged from those builds without a separate installation check", so it is the channel
  with the least coverage today and the one most worth wiring to a real host first.

## Archaeology

- issue source: `gh issue list --state closed --limit 200`. **The repo has no issues at all,
  open or closed** — every fix so far arrived as a pull request, so the PR list is the tracker:
  `gh pr list --state merged --limit 200`. Join a fix to its tests through the PR body and the
  merge commit, not through `#N` in prose.
- fix commits: `git log --format='%H %s%n%b'` piped through
  `grep -iE 'fix|refuse|regression|leak|clobber|wrong'`, over subjects and bodies. The repo
  writes sentence subjects, so there is no `fix:` prefix to match; several commits fix something
  without saying "fix" ("Pair the two plan lines a
  skill_repos rename produces"). Read the body, which states the defect.
- test locations:
  - `tests/corpus/<case>/` — one directory per behaviour, `expected.json` is the assertion. This
    is the primary convention: **a behaviour change without a case is the bug to find.**
  - `tests/run_regressions.py` — integration regressions needing more than one plan.
  - `tests/test_units.py` — fault-injection units for the Python reference.
  - `#[cfg(test)] mod tests` in `src/*.rs` — units for the port (`rename.rs`, `proc.rs`,
    `manifest.rs`, `util.rs`).
- how to run:
  - `python3 tests/run_corpus.py` and again with `QBRANCH_BIN=target/release/qbranch`
  - `python3 tests/run_corpus.py --apply`, and again with `QBRANCH_LINK_MODE=copy`
  - `python3 tests/run_regressions.py` and `python3 tests/test_units.py`
  - `python3 tests/run_parity.py` (after `cargo build --release`)
  - `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
- prove-it-bites: `git checkout <fix>^ -- <fixed files>` then the test must fail; restore; it
  must pass. For a corpus case the revert must change `expected.json`'s plan, not merely the
  exit code. Note that a bump commit re-blesses the `"tool"` line in all 30 expected plans; that
  is not a behaviour change and must not be mistaken for one.
- both implementations: a regression test belongs wherever the defect was. A behaviour defect
  needs a corpus case, which runs against **both** the reference and the port; a defect in only
  one implementation still needs the case, because the other one passing is the proof.

## Publish

- gate: ask before tag or push; quote `date`. The maintainer's standing rule is that this repo
  is committed to locally and pushed only on request, so the train always stops here.
- steps:
  1. push the release branch, open a PR, wait for the three required `check (<os>)` jobs
  2. merge (the repo uses merge commits, not squash), then update local `main`
  3. bump all three version strings and `Cargo.lock` (`cargo build`) in that same PR, or a
     release PR of its own — `resolve version` refuses a tag that disagrees with any of them
  4. re-bless the corpus for the new `"tool"` line and read the diff: it must be that line and
     nothing else
  5. tag the merged head, annotated, `v<X.Y.Z>` with subject `qbranch <X.Y.Z>`, and push the tag
  6. watch `release.yml`: `resolve version`, five `build` jobs, `publish the GitHub release`,
     `bump the Homebrew tap`, `publish to crates.io` (skipped unless `CRATES_PUBLISH` is `true`).
     The Windows job runs in the `release` environment, which carries a reviewer rule.
- re-verify, each from the public URL a user would use:
  - deb: download `qbranch_<X.Y.Z>_amd64.deb` from the release, check it against `SHA256SUMS`,
    `apt install` it in a container, S1..S3
  - archive: download the musl tarball, check it against `SHA256SUMS`, extract, S1..S3
  - homebrew: `brew install curtisgalloway/tap/qbranch` on macos-bench, S1..S3
  - msi: download and `msiexec /i` on windows-bench, S1..S3
  - source: `cargo install --locked qbranch` into a throwaway root once the crate is published
- a re-verify failure does not roll back a public tag: open an issue and report
  `PUBLISHED, unverified on <channel>`.

## Sources

Files this profile was derived from. `profile_check.py` recomputes each blob
id; a CHANGED row means the section it feeds needs re-reading, `--update`
rewrites the ids once that is done.

| path | blob | feeds |
|---|---|---|
| `.github/workflows/release.yml` | 99a505b5552d | Project, Channels, Publish |
| `.github/workflows/ci.yml` | 15fb5d69b6db | Project, Archaeology |
| `packaging/nfpm.yaml` | b3cc2d76b54c | Channels: deb |
| `packaging/windows/Package.wxs` | 7b578647fcca | Channels: msi |
| `packaging/windows/build.ps1` | 6b95dacbd9fb | Channels: msi, zip |
| `Cargo.toml` | d06391eeea74 | Project |
| `src/ctx.rs` | ae1ee0e116b0 | Project: version source |
| `bin/qbranch` | a598c1144672 | Project: version source, Smoke contract |
| `README.md` | d2eb47a41d23 | Channels: install like a user |
| `AGENTS.md` | cb4acb03c626 | Project: bump rules, Publish |
