#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Run the qbranch test corpus.

Each case under tests/corpus/<name>/ holds:

  case.json      {"manifest": "<name>", "args": [...], "rc": 0,
                  "stderr_contains": "...", "git_dirs": ["home/src/x"]}
                 (all but manifest optional; git_dirs names fixture
                 directories the tool must see as git checkouts — an
                 empty .git is created in each, since git cannot track one)
  root/          the config root the tool is pointed at: manifests/,
                 skills/, claude-code/ fragments
  home/          the fake $HOME: .claude/, .agents/skills/, src/<repos>/
  claude.json    optional canned state for the fake `claude` CLI
  expected.json  the plan the tool must produce (--dry-run --json), or
                 {"rc": N, "stderr_contains": "..."} for a refusal

The case is copied to a temporary directory, the literal token <CASE> in
every .json file is replaced with that directory (so state files can hold
absolute paths), and the tool runs with HOME, CLAUDE_CONFIG_DIR,
QBRANCH_ROOT and PATH pointed inside it. Paths in the produced plan are
normalised back to <CASE> before comparison. Nothing outside the temporary
directory is touched; the real `claude` is kept off PATH so a case decides
for itself whether Claude Code "is installed".

  python3 tests/run_corpus.py            run every case
  python3 tests/run_corpus.py fresh-machine stale-links
  python3 tests/run_corpus.py --bless    rewrite expected.json from the current tool
  python3 tests/run_corpus.py --show X   print the plan for one case

  QBRANCH_BIN=target/release/qbranch python3 tests/run_corpus.py
      run the same cases against a compiled port instead of bin/qbranch

This corpus is the specification a port of the tool must satisfy.
"""
import argparse
import difflib
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOL = HERE.parent / "bin" / "qbranch"
CORPUS = HERE / "corpus"
FAKE_BIN = HERE / "fake-bin"


def tool_cmd() -> list[str]:
    """The tool under test: bin/qbranch via this interpreter, or $QBRANCH_BIN as-is."""
    override = os.environ.get("QBRANCH_BIN")
    if override:
        return [str(Path(override).expanduser().resolve())]
    return [sys.executable, str(TOOL)]


def substitute(root: Path, token: str, value: str) -> None:
    for p in root.rglob("*.json"):
        text = p.read_text(encoding="utf-8")
        if token in text:
            p.write_text(text.replace(token, value), encoding="utf-8")


def normalise(obj, case_dir: str):
    if isinstance(obj, str):
        return obj.replace(case_dir, "<CASE>")
    if isinstance(obj, list):
        return [normalise(x, case_dir) for x in obj]
    if isinstance(obj, dict):
        return {k: normalise(v, case_dir) for k, v in obj.items()}
    return obj


def run_case(name: str) -> tuple[dict, int, str]:
    """Copy the case to a temp dir, run the tool, return (plan, rc, stderr)."""
    src = CORPUS / name
    spec = json.loads((src / "case.json").read_text())
    with tempfile.TemporaryDirectory(prefix=f"corpus-{name}-") as tmp:
        case_dir = str(Path(tmp).resolve())
        shutil.copytree(src, case_dir, symlinks=True, dirs_exist_ok=True)
        substitute(Path(case_dir), "<CASE>", case_dir)
        for rel in spec.get("git_dirs", []):
            (Path(case_dir) / rel / ".git").mkdir(parents=True, exist_ok=True)
        home = Path(case_dir) / "home"
        home.mkdir(exist_ok=True)
        bin_dir = Path(case_dir) / "bin"
        bin_dir.mkdir(exist_ok=True)
        if (Path(case_dir) / "claude.json").is_file():
            shutil.copy(FAKE_BIN / "claude", bin_dir / "claude")
            (bin_dir / "claude").chmod(0o755)
        env = {
            "HOME": str(home),
            "CLAUDE_CONFIG_DIR": str(home / ".claude"),
            "QBRANCH_ROOT": str(Path(case_dir) / "root"),
            "QBRANCH_FAKE_CLAUDE_STATE": str(Path(case_dir) / "claude.json"),
            "QBRANCH_FAKE_CLAUDE_LOG": str(Path(case_dir) / "claude-calls.log"),
            "PATH": os.pathsep.join([str(bin_dir), "/usr/bin", "/bin"]),
            "LANG": "C.UTF-8",
        }
        cmd = [*tool_cmd(), "--dry-run", "--json",
               "--manifest", spec["manifest"],
               "--skills-target", str(home / ".agents" / "skills"),
               *spec.get("args", [])]
        r = subprocess.run(cmd, capture_output=True, text=True, env=env)
        plan = None
        if r.stdout.strip():
            try:
                plan = normalise(json.loads(r.stdout), case_dir)
            except json.JSONDecodeError:
                plan = {"stdout": r.stdout}
        return plan, r.returncode, r.stderr.replace(case_dir, "<CASE>")


def main() -> int:
    ap = argparse.ArgumentParser(description="Run the qbranch test corpus.")
    ap.add_argument("cases", nargs="*", help="case names (default: all)")
    ap.add_argument("--bless", action="store_true",
                    help="rewrite each case's expected.json from the current tool")
    ap.add_argument("--show", metavar="CASE", help="print one case's plan and exit")
    args = ap.parse_args()

    if args.show:
        plan, rc, err = run_case(args.show)
        print(json.dumps(plan, indent=2))
        print(f"rc={rc}", file=sys.stderr)
        if err:
            print(err, file=sys.stderr)
        return 0

    names = args.cases or sorted(p.name for p in CORPUS.iterdir()
                                 if (p / "case.json").is_file())
    failed = 0
    for name in names:
        spec = json.loads((CORPUS / name / "case.json").read_text())
        plan, rc, err = run_case(name)
        want_rc = spec.get("rc", 0)
        if plan is None or spec.get("stderr_contains"):
            got = {"rc": rc, "stderr_contains": spec.get("stderr_contains", "")}
            ok = rc == want_rc and spec.get("stderr_contains", "") in err
            expected_path = CORPUS / name / "expected.json"
            if args.bless:
                expected_path.write_text(json.dumps(got, indent=2) + "\n")
            status = "ok" if ok else "FAIL"
            if not ok:
                failed += 1
                print(f"{status:<5} {name}  rc={rc} (want {want_rc})\n{err}")
            else:
                print(f"{status:<5} {name}")
            continue

        expected_path = CORPUS / name / "expected.json"
        if args.bless:
            expected_path.write_text(json.dumps(plan, indent=2) + "\n")
            print(f"bless {name}")
            continue
        if not expected_path.is_file():
            failed += 1
            print(f"FAIL  {name}  no expected.json (run --bless after reviewing --show)")
            continue
        expected = json.loads(expected_path.read_text())
        ok = plan == expected and rc == want_rc
        if ok:
            print(f"ok    {name}")
            continue
        failed += 1
        print(f"FAIL  {name}  rc={rc} (want {want_rc})")
        a = json.dumps(expected, indent=2, sort_keys=True).splitlines()
        b = json.dumps(plan, indent=2, sort_keys=True).splitlines()
        for line in difflib.unified_diff(a, b, "expected", "got", lineterm="", n=2):
            print("  " + line)
        if err.strip():
            print("  stderr: " + err.strip().replace("\n", "\n          "))
    print(f"\n{len(names) - failed}/{len(names)} cases pass")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
