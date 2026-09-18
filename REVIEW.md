# Code review and fixes

**Terms:** the corpus is the fixture-based behaviour contract; a manifest selects skills
and settings; the state file records what the last sync owns. See [GLOSSARY.md](GLOSSARY.md).

Reviewed the whole tracked codebase at `fe7d5463f7059eca2fb2fdd0c76061724b84d8fc`
(296 tracked files; clean working tree; base and head are the same because this was a
whole-codebase review). Coverage included both implementations, tests, packaging,
workflows and documentation. Four independent review passes covered security,
correctness, data preservation/compatibility and documentation. An independent referee
confirmed ten findings, lowering two documentation findings to low severity.

All ten findings below were fixed. No accepted finding was deferred. The locations in
the original findings table refer to the reviewed commit, before the fixes.

## Changes and regression coverage

| Finding | Change | Verification |
| --- | --- | --- |
| F1 | Create private state and new settings with Unix mode 0600; atomically replace the state path without following a symlink. | Permission, existing-state tightening and symlink-victim preservation regressions; Rust permission tests. |
| F2 | Refresh harness copies when earlier planned actions change the shared skills directory. | Copy and symlink corpus cases; one-apply update/add/remove checks for both harnesses. |
| F3 | Keep subprocess deadlines active until both process exit and output collection complete; terminate the Unix process group on timeout. | Rust tests for an exited parent with an inherited pipe and a running child with closed pipes. |
| F4 | Stage copies before replacing destinations; restore the previous destination on replacement failure and retain a recovery backup if restoration fails. | Failed nested-copy regression verifies the complete old installation survives and later recovers. |
| F5 | Retain ownership records for existing destinations left behind after failed actions. | Missing-source/recovery/removal regressions and corpus cases. |
| F6 | Write settings, state, manifests and fragments through complete staged files and atomic replacement; validate legacy settings before replacing their symlink. | Injected Python replacement failure, Rust failed-write test, mode/symlink checks and full apply parity. |
| F7 | Validate supported manifest schema before add/remove/rename/plugin edits without implicitly rewriting supported schemas. | Newer-schema corpus refusals and byte-preservation checks for all four edit commands. |
| F8 | Separate repository fetching from source resolution; only an actual sync fetches, once per repository URL. Document uncached dry-run sources as missing. | Absent/cached repository corpus cases and local-git regressions checking no clone/pull during dry runs or audits, followed by a real fetch on sync. |
| F9 | Correct the bundled skill count and document archive, installer and Cargo layouts. | Compared prose with packaging files and embedded skill declarations. |
| F10 | Correct release validation claims to distinguish native corpus runs, the cross-built Intel macOS binary and .deb/MSI install checks. | Compared release prose with workflow conditions and matrix entries. |

The copy replacement is staged with rollback, not a filesystem-wide transaction. Native
Windows and Linux execution remains covered by CI; local validation runs on macOS.

## Validation

- Python and Rust: 31/31 corpus plans pass.
- Apply/report parity: 279/279 runs identical.
- Each implementation: 27/27 applicable cases converge in both default and copy modes.
- Integration regressions: ten pass for Python; nine pass for Rust with the
  Python-only fault-injection test skipped.
- Python unit tests: 15 pass, including deterministic fault injection.
- Rust unit tests: 19 pass; the opt-in private-checkout probe remains ignored.
- Rust formatting and Clippy pass, including Windows-target Clippy for all targets.
- Changed Python code formatted with Pyink at 88 columns; whitespace checks pass.

## Test coverage follow-up

The unit suites now exercise copy publication failures, rollback failures with recovery
backups, atomic-write partial-write, publication and flush failures, private/new/preserved permissions,
symlink handling, successful and timed-out subprocesses, schema guards and read-only
repository resolution. Fault injection is local to each test; production has no global
failure switches. Portable Rust tests also run on Windows, with Unix-only permissions
and symlink checks gated individually.

F2 is verified by independent update, add and remove steps against both harnesses, each
followed by a convergence check. F5 additionally has an injected failed-removal/retry
test that checks both ownership lists. F6 checks valid legacy settings conversion without
changing source bytes and invalid legacy settings without removing their symlink.
F7 checks both plugin fragment targets and all skill edit commands; F8 checks audits,
dry runs and rename fixes before and after a cache exists. These command-level contracts
remain integration tests so both implementations are exercised through the real CLI.
F9 and F10 are documentation corrections verified against package layouts and workflow
conditions, not runtime unit-test claims.

## Original verified findings

## Review swarm: 10 finding(s)

All 4 arms delivered.

| # | ID | Severity | Arms | Location | Claim | Suggested fix |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | F8 | high | docs | `README.md:42` | The promised read-only dry run creates and updates repository caches for repo-based skills. src/sync.rs:312 calls collect_desired before the dry-run return at :570, and src/skills.rs:91 invokes resolve_skill_src, which reaches resolve_repo_local (:68). That function executes ["git", "-C", &display(&cache), "pull", "--ff-only"] (:38) or fs::create_dir_all(&ctx.skill_repos_cache) (:48) and ["git", "clone", repo_url, &display(&cache)] (:51). Python bin/qbranch:253-284 has the same behaviour. Thus a --dry-run can change already linked skill contents and create a cache, contrary to the user-facing guarantee. | Keep source resolution read-only during planning and reports; move cloning/pulling into an explicit apply preparation phase in both implementations. Add a loca… |
| 2 | F1 | high | security | `bin/qbranch:586-595` | Settings fragments may contain credentials, such as env API tokens. save_state copies the complete applied policy into a newly created state file using default permissions; with umask 022 it is 0644, even when the source fragment and existing live settings are 0600. On a machine with a traversable home/skills directory, other local users can read the credential from this extra copy. Reproduced in isolated homes with both Python and the release binary: a 0600 fragment containing env.EXAMPLE_API_TOKEN yielded a 0644 .qbranch-state.json containing the value. Rust has the same sink at src/state.rs:157-160 and src/util.rs:111-112. Newly created live settings have the same permission issue at bin/qbranch:1302 and src/settings.rs:246. | Write state files with owner-only permissions from creation on Unix, tighten existing state files safely, and create newly materialised settings files with equ… |
| 3 | F4 | high | compat | `src/copy.rs:33-38` | copy_path deletes the complete existing copy before it has successfully read the replacement source. A source tree containing a dangling nested symlink is accepted by planning, then copy fails after deleting the working installation. Verified with both binaries: Python leaves replacement partial content, Rust left no SKILL.md; both returned 1. bin/qbranch:546-553 has the same ordering. | Stage the complete copy in a sibling temporary path before touching the destination; replace only after copying succeeds, restoring the old destination if repl… |
| 4 | F2 | high | correctness | `src/sync.rs:465-466` | Harness copies are compared before the generic skills directory is updated, so a successful copy-mode sync leaves Claude/Antigravity using stale skill contents. Reproduced in both Python and Rust: converge copy-mode fixture, append CHANGED to root/skills/alpha/SKILL.md, then sync once; rc=0 but .claude/skills/alpha/SKILL.md has no CHANGED and next plan schedules claude-skills copy. Python parallel is bin/qbranch:2268-2276. Applies also to adding/removing skills after convergence. | When a harness source is the generic skills directory, schedule its copy if any earlier action changes that directory, even if its pre-apply content matches. A… |
| 5 | F5 | high | compat | `src/sync.rs:627-628` | A temporarily missing source permanently forgets ownership of an existing tracked copy: MISS leaves the copy in place but never adds it to final_links/copies, and save_state overwrites the old inventory. Verified full sync sequence in both binaries: initial copy succeeds, source temporarily renamed makes sync fail and writes copies=[], restore source then dry run yields WARN rather than refresh. Failed removals similarly lose ownership. Python bin/qbranch:2398-2412 and 2425-2432 match. | Retain previous ownership records for destinations still present after failed or missing-source actions; remove records only after successful removal/replaceme… |
| 6 | F6 | high | compat | `src/util.rs:111-112` | The shared JSON writer opens and truncates existing files in place. Disk-full, interrupted writes or an I/O error can destroy app-owned settings (including unrelated app keys), manifests and the sole ownership state. Settings calls this at src/settings.rs:246 and state at src/state.rs:160; Python also writes these files directly with Path.write_text (bin/qbranch:586,1302 and manifest edit sites). | Use a shared atomic JSON writer in both implementations: serialize before opening, write a same-directory temporary file, preserve necessary permissions, then… |
| 7 | F7 | medium | compat | `src/manifest.rs:128-132` | Manifest-edit commands bypass the forward-schema refusal: read_manifest_raw reads JSON without migrate_manifest validation, and add/remove/fix callers modify it. Verified both Python and Rust accept --remove-skill against schema 999, return 0 and replace skills with []. This violates the forward-only compatibility boundary and may corrupt layouts this version does not understand. Python bin/qbranch:1083-1084,1105-1106,1123-1124 likewise read raw JSON. | Validate supported schema before every manifest mutation, including add/remove/fix and manage-plugin. Keep existing supported-schema representation unless an e… |
| 8 | F3 | medium | correctness | `src/proc.rs:88-89` | The timeout stops being checked as soon as the direct child exits. Joining stdout/stderr reader threads then waits indefinitely when a descendant inherits the pipe handles. Reproduced with run_capture(["/bin/sh","-c","sleep 2 & exit 0"],100ms): returns success after 2.01 seconds, rather than timeout. CLI subprocesses can therefore hang beyond their advertised timeout. | Keep the deadline active while waiting for stream readers; use channel completion notifications with deadline-bound receives, and arrange cleanup for readers/d… |
| 9 | F10 | low | docs | `.github/workflows/release.yml:440-441` | The generated release notes promise validation that this workflow explicitly skips. The build matrix at :86 sets `{ os: macos-latest, target: x86_64-apple-darwin, test: false }`, and the corpus step is guarded by `if: matrix.test`. Archives are packaged and uploaded without installation or an archive smoke run; only .deb and MSI receive installation exercises. Consequently the published Intel macOS binary has not passed the corpus in this workflow and not every package was installed. | Make the release prose and AGENTS.md release step accurately name tested native binaries and the .deb/MSI installation checks, or add the missing native Intel… |
| 10 | F9 | low | docs | `README.md:74-75` | The documented installed skill path is wrong for release archives and cargo installs. .github/workflows/release.yml packages archives with `cp -R skills "dist/$name/skills"`, directly beside the executable, and its zip step similarly uses `Copy-Item -Recurse skills "$stage\skills"`; neither makes share/qbranch/skills or bin. Cargo.toml has only a [[bin]] install target, with the three skills embedded through src/ctx.rs BUNDLED_SKILLS, rather than installed as separate shared files. Users following this promise cannot find the skills at the stated location. | Document the three skills and channel-specific locations: share/qbranch/skills for system installers, skills/ beside the executable in archives, and --skill fo… |

Dropped before this table: checker dropped 0; referee dropped 0; merged 0.
