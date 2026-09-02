#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Run the qbranch test corpus.

Each case under tests/corpus/<name>/ holds:

  case.json      the case's shape; every key is optional:
                   "manifest": "<name>"      passed as --manifest (omit it to
                                             exercise the default choice)
                   "args": [...]             extra arguments
                   "env": {"VAR": "v"|null}  set (or, for null, unset) in the
                                             tool's environment; <CASE> allowed
                   "cwd": "root"             working directory, relative to
                                             the case (default: the case dir)
                   "mkdirs": ["home/x/.git"] directories to create, for what
                                             git cannot track (empty dirs,
                                             .git markers)
                   "hostname_manifest": "h"  rename root/manifests/h.json to
                                             this machine's short hostname
                                             (its name prints as <HOST>)
                   "rc": 0                   expected exit code of the dry run
                   "stderr_contains": "..."  for a refusal, the text to expect
                   "apply_rc": 0             expected exit code under --apply
                                             (a case that fails on purpose)
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
normalised back to <CASE> (and, on Windows, to forward slashes) before
comparison. Nothing outside the temporary directory is touched; the real
`claude` is kept off PATH so a case decides for itself whether Claude Code
"is installed".

  python3 tests/run_corpus.py            run every case
  python3 tests/run_corpus.py fresh-machine stale-links
  python3 tests/run_corpus.py --bless    rewrite expected.json from the current tool
  python3 tests/run_corpus.py --show X   print the plan for one case
  python3 tests/run_corpus.py --apply    apply each case for real, then require
                                         the next dry run to find nothing to do

  QBRANCH_BIN=target/release/qbranch python3 tests/run_corpus.py
      run the same cases against a compiled port instead of bin/qbranch
  QBRANCH_LINK_MODE=copy python3 tests/run_corpus.py --apply
      the convergence check in copy mode (plans are not compared there)

This corpus is the specification every implementation must satisfy;
run_parity.py builds on the same sandbox to compare two of them.
"""
from __future__ import annotations

import argparse
import difflib
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOL = HERE.parent / "bin" / "qbranch"
CORPUS = HERE / "corpus"
FAKE_BIN = HERE / "fake-bin"
WINDOWS = os.name == "nt"
# After an apply, the next plan may only confirm what is there. Plugin
# actions are exempt: the fake `claude` records installs without changing
# what it later reports.
CONVERGED_OPS = {"ok"}


def tool_cmd() -> list[str]:
    """The tool under test: bin/qbranch via this interpreter, or $QBRANCH_BIN as-is."""
    override = os.environ.get("QBRANCH_BIN")
    if override:
        return [str(Path(override).expanduser().resolve())]
    return [sys.executable, str(TOOL)]


def short_hostname() -> str:
    """The same rule the tool uses for its per-host manifest default."""
    return socket.gethostname().split(".")[0].lower()


def substitute(root: Path, token: str, value: str) -> None:
    """Replace token in every .json file, JSON-escaped (Windows backslashes)."""
    escaped = json.dumps(value)[1:-1]
    for p in root.rglob("*.json"):
        text = p.read_text(encoding="utf-8")
        if token in text:
            p.write_text(text.replace(token, escaped), encoding="utf-8")


def recreate_symlinks(root: Path) -> None:
    """Give each fixture symlink the right kind on Windows.

    shutil.copytree recreates every symlink as a file symlink there, and a
    file symlink that points at a directory cannot be read through, so a
    link to a skill directory has to be remade as a directory symlink.
    """
    if not WINDOWS:
        return
    links = [p for p in root.rglob("*") if p.is_symlink()]
    for link in links:
        target = os.readlink(link)
        resolved = Path(target) if os.path.isabs(target) else link.parent / target
        try:
            link.unlink()
        except OSError:
            os.rmdir(link)
        os.symlink(target, link, target_is_directory=resolved.is_dir())


def install_fake_claude(bin_dir: Path) -> None:
    """Put the fake `claude` on the case's PATH, as a .cmd shim on Windows."""
    shutil.copy(FAKE_BIN / "claude", bin_dir / "claude")
    (bin_dir / "claude").chmod(0o755)
    if WINDOWS:
        (bin_dir / "claude.cmd").write_text(
            f'@"{sys.executable}" "%~dp0claude" %*\r\n', encoding="utf-8")


def system_path() -> list[str]:
    """The minimum PATH the tool needs besides the case's own bin/."""
    if WINDOWS:
        return [os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32")]
    return ["/usr/bin", "/bin"]


class Sandbox:
    """A scratch copy of one corpus case, with the environment the tool sees."""

    def __init__(self, name: str):
        self.name = name
        src = CORPUS / name
        self.spec = json.loads((src / "case.json").read_text())
        self._tmp = tempfile.TemporaryDirectory(prefix=f"corpus-{name}-")
        self.dir = str(Path(self._tmp.name).resolve())
        shutil.copytree(src, self.dir, symlinks=True, dirs_exist_ok=True)
        recreate_symlinks(Path(self.dir))
        substitute(Path(self.dir), "<CASE>", self.dir)
        for rel in self.spec.get("mkdirs", []):
            (Path(self.dir) / rel).mkdir(parents=True, exist_ok=True)
        self.home = Path(self.dir) / "home"
        self.home.mkdir(exist_ok=True)
        bin_dir = Path(self.dir) / "bin"
        bin_dir.mkdir(exist_ok=True)
        if (Path(self.dir) / "claude.json").is_file():
            install_fake_claude(bin_dir)
        self.host = None
        renamed = self.spec.get("hostname_manifest")
        if renamed:
            self.host = short_hostname()
            manifests = Path(self.dir) / "root" / "manifests"
            (manifests / f"{renamed}.json").rename(manifests / f"{self.host}.json")
        self.env = {
            "HOME": str(self.home),
            "USERPROFILE": str(self.home),
            "CLAUDE_CONFIG_DIR": str(self.home / ".claude"),
            "QBRANCH_ROOT": str(Path(self.dir) / "root"),
            "QBRANCH_FAKE_CLAUDE_STATE": str(Path(self.dir) / "claude.json"),
            "QBRANCH_FAKE_CLAUDE_LOG": str(Path(self.dir) / "claude-calls.log"),
            "PATH": os.pathsep.join([str(bin_dir), *system_path()]),
            "LANG": "C.UTF-8",
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        if WINDOWS:
            for k in ("SystemRoot", "TEMP", "TMP", "PATHEXT", "COMSPEC", "COMPUTERNAME"):
                if k in os.environ:
                    self.env[k] = os.environ[k]
        if "QBRANCH_LINK_MODE" in os.environ:
            self.env["QBRANCH_LINK_MODE"] = os.environ["QBRANCH_LINK_MODE"]
        for k, v in self.spec.get("env", {}).items():
            if v is None:
                self.env.pop(k, None)
            else:
                self.env[k] = v.replace("<CASE>", self.dir)
        self.cwd = str(Path(self.dir) / self.spec.get("cwd", "."))

    def argv(self, tool: list[str], extra: list[str]) -> list[str]:
        cmd = list(tool)
        if "manifest" in self.spec:
            cmd += ["--manifest", self.spec["manifest"]]
        cmd += ["--skills-target", str(self.home / ".agents" / "skills")]
        cmd += self.spec.get("args", [])
        return cmd + extra

    def run(self, tool: list[str], extra: list[str]) -> subprocess.CompletedProcess:
        return subprocess.run(self.argv(tool, extra), capture_output=True,
                              text=True, env=self.env, cwd=self.cwd,
                              stdin=subprocess.DEVNULL)

    def plan(self, tool: list[str]) -> tuple[dict | None, int, str]:
        """The dry-run plan, normalised; (None, rc, stderr) when there is none."""
        r = self.run(tool, ["--dry-run", "--json"])
        plan = None
        if r.stdout.strip():
            try:
                plan = self.normalise(json.loads(r.stdout))
            except json.JSONDecodeError:
                plan = {"stdout": r.stdout}
        return plan, r.returncode, self.norm(r.stderr)

    def norm(self, s: str) -> str:
        """Replace the case directory (and this host's name) with tokens."""
        s = s.replace(self.dir, "<CASE>")
        if WINDOWS:
            s = s.replace(self.dir.replace("\\", "/"), "<CASE>").replace("\\", "/")
        if self.host:
            s = s.replace(self.host, "<HOST>")
        return s

    def normalise(self, obj):
        if isinstance(obj, str):
            return self.norm(obj)
        if isinstance(obj, list):
            return [self.normalise(x) for x in obj]
        if isinstance(obj, dict):
            return {k: self.normalise(v) for k, v in obj.items()}
        return obj

    def close(self) -> None:
        self._tmp.cleanup()


def run_case(name: str) -> tuple[dict, int, str]:
    """Run the tool's dry-run plan for one case; return (plan, rc, stderr)."""
    sb = Sandbox(name)
    try:
        return sb.plan(tool_cmd())
    finally:
        sb.close()


def apply_case(name: str) -> list[str] | None:
    """Apply a case for real, then dry-run again; return the problems found.

    None means the case is a refusal and was not applied.
    """
    sb = Sandbox(name)
    try:
        if sb.spec.get("rc", 0) != 0:
            return None
        r = sb.run(tool_cmd(), [])
        want = sb.spec.get("apply_rc", 0)
        problems = []
        if r.returncode != want:
            problems.append(f"apply rc={r.returncode} (want {want})\n"
                            + sb.norm(r.stderr).strip())
        if want != 0:
            return problems
        plan, rc, err = sb.plan(tool_cmd())
        if not plan or "actions" not in plan:
            return problems + [f"no plan after apply (rc={rc})\n{err.strip()}"]
        left = [a for a in plan["actions"] if a["op"] not in CONVERGED_OPS]
        if left:
            problems.append("not converged: " + ", ".join(
                f"{a['op']} {a['label']} {a['note']}".rstrip() for a in left))
        if plan.get("failures"):
            problems.append("failures after apply: " + "; ".join(plan["failures"]))
        return problems
    finally:
        sb.close()


def all_case_names() -> list[str]:
    return sorted(p.name for p in CORPUS.iterdir() if (p / "case.json").is_file())


def main() -> int:
    ap = argparse.ArgumentParser(description="Run the qbranch test corpus.")
    ap.add_argument("cases", nargs="*", help="case names (default: all)")
    ap.add_argument("--bless", action="store_true",
                    help="rewrite each case's expected.json from the current tool")
    ap.add_argument("--show", metavar="CASE", help="print one case's plan and exit")
    ap.add_argument("--apply", action="store_true",
                    help="apply each case for real in its sandbox, then require "
                         "the next dry run to find nothing left to do")
    args = ap.parse_args()

    if args.show:
        plan, rc, err = run_case(args.show)
        print(json.dumps(plan, indent=2))
        print(f"rc={rc}", file=sys.stderr)
        if err:
            print(err, file=sys.stderr)
        return 0

    names = args.cases or all_case_names()

    if args.apply:
        failed = applied = 0
        for name in names:
            problems = apply_case(name)
            if problems is None:
                print(f"skip  {name}  (a refusal; nothing to apply)")
                continue
            applied += 1
            if problems:
                failed += 1
                print(f"FAIL  {name}")
                for p in problems:
                    print("  " + p.replace("\n", "\n  "))
            else:
                print(f"ok    {name}")
        print(f"\n{applied - failed}/{applied} applied cases converge")
        return 1 if failed else 0

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
