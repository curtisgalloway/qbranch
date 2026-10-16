#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Exercise the release smoke contract against an installed binary or archive."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import zipfile

SKILLS = {"agent-audit", "claude-apps-sync", "qbranch", "review-plugins"}


def smoke(binary: Path, version: str, skills: Path | None) -> None:
    """Run S1-S6 outside the checkout, without access to installed harnesses."""
    with tempfile.TemporaryDirectory(prefix="qbranch-smoke-") as directory:
        scratch = Path(directory).resolve()
        home = scratch / "home"
        home.mkdir()
        empty_bin = scratch / "empty-bin"
        empty_bin.mkdir()
        env = {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "CLAUDE_CONFIG_DIR": str(home / ".claude"),
            "PATH": str(empty_bin),
        }
        for key in ("SystemRoot", "COMSPEC", "PATHEXT", "TEMP", "TMP"):
            if key in os.environ:
                env[key] = os.environ[key]

        def run(*args: str, code: int = 0) -> str:
            result = subprocess.run(
                [str(binary), *args],
                cwd=scratch,
                env=env,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=30,
                check=False,
            )
            if result.returncode != code:
                raise AssertionError(
                    f"{args}: exit {result.returncode}, expected {code}\n"
                    f"{result.stdout}\n{result.stderr}"
                )
            assert "panicked" not in result.stderr and "backtrace" not in result.stderr
            return result.stdout

        help_text = run("--help")
        assert run("--version").split()[1] == version
        for flag in (
            "--dry-run",
            "--list",
            "--add-skill",
            "--audit",
            "--skill",
            "--update",
            "--export-zips",
        ):
            assert flag in help_text, flag
        print("S1 PASS: version and help", flush=True)

        assert set(run("--skill").splitlines()) == SKILLS
        for name in sorted(SKILLS):
            text = run("--skill", name)
            assert f"name: {name}" in text
            if skills is not None:
                assert text == (skills / name / "SKILL.md").read_text(encoding="utf-8")
        if skills is not None:
            assert {p.parent.name for p in skills.glob("*/SKILL.md")} == SKILLS
        print("S2 PASS: all four embedded and packaged skills", flush=True)

        root = home / "config"
        (root / "manifests").mkdir(parents=True)
        source = root / "skills" / "demo"
        source.mkdir(parents=True)
        (source / "SKILL.md").write_text("demo skill\n", encoding="utf-8")
        manifest = {
            "schema": 2,
            "name": "smoke",
            "links": [],
            "skills": [{"name": "demo", "path": "${QBRANCH_ROOT}/skills/demo"}],
            "claude_settings": [],
            "agy_settings": [],
        }
        (root / "manifests" / "smoke.json").write_text(
            json.dumps(manifest), encoding="utf-8"
        )
        target = home / ".agents" / "skills"
        args = (
            "--root",
            str(root),
            "--manifest",
            "smoke",
            "--skills-target",
            str(target),
        )
        applied = run(*args)
        installed = target / "demo"
        assert (installed / "SKILL.md").read_text(encoding="utf-8") == "demo skill\n"
        if installed.is_symlink():
            assert installed.resolve() == source.resolve()
        else:
            assert os.name == "nt" and "copying instead" in applied
        print("S3 PASS: installed binary syncs a skill", flush=True)

        plan = json.loads(run(*args, "--dry-run", "--json"))
        assert plan["actions"] and all(a["op"] == "ok" for a in plan["actions"])
        state = json.loads((target / ".qbranch-state.json").read_text(encoding="utf-8"))
        assert state["manifest"] == "smoke" and Path(state["root"]).resolve() == root
        print("S4 PASS: converged plan and state round trip", flush=True)

        json.loads(run("--audit", "--json"))
        report = json.loads(run("--plugin-status", "--json", code=1))
        assert report["errors"] == [
            "claude not on PATH — cannot read installed plugins"
        ]
        print("S5 PASS: diagnostics without a harness", flush=True)

        exports = home / "apps"
        export_args = (
            "--root",
            str(root),
            "--manifest",
            "smoke",
            "--export-zips",
            str(exports),
            "--json",
        )
        first = json.loads(run(*export_args))
        assert first["skills"][0]["name"] == "demo"
        assert first["skills"][0]["status"] == "new"
        with zipfile.ZipFile(exports / "demo.zip") as archive:
            assert archive.read("demo/SKILL.md") == b"demo skill\n"
        (exports / "uploaded").mkdir()
        shutil.copyfile(exports / "demo.zip", exports / "uploaded" / "demo.zip")
        assert json.loads(run(*export_args))["skills"][0]["status"] == "current"
        print("S6 PASS: export and upload-record round trip", flush=True)


def main() -> None:
    """Accept a binary already installed, or extract a workflow-built archive."""
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--binary", type=Path)
    group.add_argument("--archive", type=Path)
    parser.add_argument("--skills", type=Path)
    parser.add_argument("--version")
    args = parser.parse_args()
    version = args.version
    if version is None:
        manifest = Path(__file__).resolve().parents[1] / "Cargo.toml"
        version = re.search(r'^version = "([^"]+)"', manifest.read_text(), re.M)[1]
    if args.binary:
        smoke(
            args.binary.resolve(),
            version,
            args.skills.resolve() if args.skills else None,
        )
        return
    with tempfile.TemporaryDirectory(prefix="qbranch-archive-") as directory:
        shutil.unpack_archive(str(args.archive.resolve()), directory)
        entries = list(Path(directory).iterdir())
        assert (
            len(entries) == 1 and entries[0].is_dir()
        ), "expected one top-level directory"
        payload = entries[0]
        assert (payload / "LICENSE").is_file() and (payload / "README.md").is_file()
        binary = payload / ("qbranch.exe" if os.name == "nt" else "qbranch")
        smoke(binary.resolve(), version, payload / "skills")


if __name__ == "__main__":
    main()
