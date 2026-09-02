#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Check that a compiled port and the Python reference behave identically.

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
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_corpus import CORPUS, TOOL, Sandbox  # noqa: E402

DEFAULT_BIN = Path(__file__).resolve().parent.parent / "target" / "release" / "qbranch"
REPORT_MODES = (
    ["--plugin-status", "--json"],
    ["--audit", "--json"],
    ["--plugin-status"],
    ["--audit"],
    ["--list"],
)


def tree(sb: Sandbox) -> dict:
    """Everything under the case dir, as comparable strings."""
    out = {}
    for p in sorted(Path(sb.dir).rglob("*")):
        rel = p.relative_to(sb.dir)
        if rel.parts[0] == "bin" or "Library" in rel.parts:
            continue
        key = sb.norm(str(rel))
        if p.is_symlink():
            out[key] = "-> " + sb.norm(os.readlink(p))
        elif p.is_dir():
            out[key] = "dir"
        elif p.suffix == ".json":
            try:
                d = json.loads(p.read_text())
            except json.JSONDecodeError:
                out[key] = "file " + sb.norm(p.read_text(errors="replace"))
                continue
            if isinstance(d, dict) and "linked_at" in d:
                d["linked_at"] = "<TIME>"
            out[key] = "json " + sb.norm(json.dumps(d, sort_keys=True))
        else:
            out[key] = "file " + sb.norm(p.read_text(errors="replace"))
    return out


def snapshot(name: str, tool: list[str], extra: list[str], with_tree: bool) -> dict:
    sb = Sandbox(name)
    try:
        r = sb.run(tool, extra)
        snap = {"rc": r.returncode, "stdout": sb.norm(r.stdout),
                "stderr": sb.norm(r.stderr)}
        if with_tree:
            snap["tree"] = tree(sb)
        return snap
    finally:
        sb.close()


def compare(label: str, py: dict, port: dict) -> bool:
    a = json.dumps(py, indent=1, sort_keys=True).splitlines()
    b = json.dumps(port, indent=1, sort_keys=True).splitlines()
    if a == b:
        print(f"same  {label}  rc={py['rc']}")
        return True
    print(f"DIFF  {label}  python rc={py['rc']} port rc={port['rc']}")
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
