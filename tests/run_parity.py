#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Check that the Rust port and the Python reference behave identically.

run_corpus.py compares dry-run plans. This covers what that cannot: each
corpus case is applied for real, in a scratch copy, by both implementations,
and everything they leave behind is diffed (exit code, stdout, stderr, every
symlink target, every file, every JSON document with the state file's
timestamp masked). The report modes that have no corpus expectation are
diffed the same way: --plugin-status and --audit in text and --json form,
and --list.

  python3 tests/run_parity.py                      after cargo build --release
  python3 tests/run_parity.py --bin path/to/qbranch
  python3 tests/run_parity.py fresh-machine        one case
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
DEFAULT_BIN = HERE.parent / "target" / "release" / "qbranch"
REPORT_MODES = (
    ["--plugin-status", "--json"],
    ["--audit", "--json"],
    ["--plugin-status"],
    ["--audit"],
    ["--list"],
)


def substitute(root: Path, token: str, value: str) -> None:
    for p in root.rglob("*.json"):
        text = p.read_text(encoding="utf-8")
        if token in text:
            p.write_text(text.replace(token, value), encoding="utf-8")


class Case:
    """A scratch copy of one corpus case, wired the way run_corpus.py wires it."""

    def __init__(self, name: str):
        src = CORPUS / name
        self.spec = json.loads((src / "case.json").read_text())
        self.tmp = tempfile.mkdtemp(prefix=f"parity-{name}-")
        self.dir = str(Path(self.tmp).resolve())
        shutil.copytree(src, self.dir, symlinks=True, dirs_exist_ok=True)
        substitute(Path(self.dir), "<CASE>", self.dir)
        for rel in self.spec.get("git_dirs", []):
            (Path(self.dir) / rel / ".git").mkdir(parents=True, exist_ok=True)
        self.home = Path(self.dir) / "home"
        self.home.mkdir(exist_ok=True)
        bin_dir = Path(self.dir) / "bin"
        bin_dir.mkdir(exist_ok=True)
        if (Path(self.dir) / "claude.json").is_file():
            shutil.copy(FAKE_BIN / "claude", bin_dir / "claude")
            (bin_dir / "claude").chmod(0o755)
        self.env = {
            "HOME": str(self.home),
            "CLAUDE_CONFIG_DIR": str(self.home / ".claude"),
            "QBRANCH_ROOT": str(Path(self.dir) / "root"),
            "QBRANCH_FAKE_CLAUDE_STATE": str(Path(self.dir) / "claude.json"),
            "QBRANCH_FAKE_CLAUDE_LOG": str(Path(self.dir) / "claude-calls.log"),
            "PATH": os.pathsep.join([str(bin_dir), "/usr/bin", "/bin"]),
            "LANG": "C.UTF-8",
            "PYTHONDONTWRITEBYTECODE": "1",
        }

    def run(self, tool: list[str], extra: list[str]) -> dict:
        cmd = [*tool, "--manifest", self.spec["manifest"],
               "--skills-target", str(self.home / ".agents" / "skills"),
               *self.spec.get("args", []), *extra]
        r = subprocess.run(cmd, capture_output=True, text=True, env=self.env,
                           stdin=subprocess.DEVNULL)
        return {"rc": r.returncode, "stdout": self.norm(r.stdout),
                "stderr": self.norm(r.stderr)}

    def norm(self, s: str) -> str:
        return s.replace(self.dir, "<CASE>")

    def tree(self) -> dict:
        """Everything under the case dir, as comparable strings."""
        out = {}
        skip = {"bin"}
        for p in sorted(Path(self.dir).rglob("*")):
            rel = p.relative_to(self.dir)
            if rel.parts[0] in skip or "Library" in rel.parts:
                continue
            key = str(rel)
            if p.is_symlink():
                out[key] = "-> " + self.norm(os.readlink(p))
            elif p.is_dir():
                out[key] = "dir"
            elif p.suffix == ".json":
                try:
                    d = json.loads(p.read_text())
                except json.JSONDecodeError:
                    out[key] = "file " + self.norm(p.read_text(errors="replace"))
                    continue
                if isinstance(d, dict) and "linked_at" in d:
                    d["linked_at"] = "<TIME>"
                out[key] = "json " + self.norm(json.dumps(d, sort_keys=True))
            else:
                out[key] = "file " + self.norm(p.read_text(errors="replace"))
        return out

    def close(self) -> None:
        shutil.rmtree(self.dir, ignore_errors=True)


def snapshot(name: str, tool: list[str], extra: list[str], with_tree: bool) -> dict:
    case = Case(name)
    try:
        snap = case.run(tool, extra)
        if with_tree:
            snap["tree"] = case.tree()
        return snap
    finally:
        case.close()


def compare(label: str, py: dict, rs: dict) -> bool:
    a = json.dumps(py, indent=1, sort_keys=True).splitlines()
    b = json.dumps(rs, indent=1, sort_keys=True).splitlines()
    if a == b:
        print(f"same  {label}  rc={py['rc']}")
        return True
    print(f"DIFF  {label}  python rc={py['rc']} port rc={rs['rc']}")
    for line in difflib.unified_diff(a, b, "python", "port", lineterm="", n=1):
        print("   " + line)
    return False


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("cases", nargs="*", help="case names (default: all)")
    ap.add_argument("--bin", default=str(DEFAULT_BIN),
                    help=f"the port under test (default: {DEFAULT_BIN})")
    args = ap.parse_args()
    port = Path(args.bin).expanduser().resolve()
    if not port.is_file():
        print(f"no binary at {port}; run cargo build --release or pass --bin",
              file=sys.stderr)
        return 2
    python = [sys.executable, str(TOOL)]
    names = args.cases or sorted(p.name for p in CORPUS.iterdir()
                                 if (p / "case.json").is_file())
    failed = 0
    for name in names:
        ok = compare(f"{name} (apply)",
                     snapshot(name, python, [], True),
                     snapshot(name, [str(port)], [], True))
        failed += not ok
        for extra in REPORT_MODES:
            ok = compare(f"{name} {' '.join(extra)}",
                         snapshot(name, python, extra, False),
                         snapshot(name, [str(port)], extra, False))
            failed += not ok
    total = len(names) * (1 + len(REPORT_MODES))
    print(f"\n{total - failed}/{total} runs identical")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
