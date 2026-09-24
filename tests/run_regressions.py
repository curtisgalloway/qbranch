#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Integration regressions for behaviours that need more than one corpus plan."""

from __future__ import annotations

import json
import os
import runpy
import stat
import subprocess
import unittest
from pathlib import Path
from unittest import mock

import run_corpus

TOOL = run_corpus.tool_cmd()
WINDOWS = os.name == "nt"


class RegressionTest(unittest.TestCase):

    def setUp(self) -> None:
        self.sb = run_corpus.Sandbox("fresh-machine")
        self.root = Path(self.sb.dir) / "root"
        self.home = self.sb.home
        self.manifest = self.root / "manifests" / "fresh.json"

    def tearDown(self) -> None:
        self.sb.close()

    def write_manifest(self, *, skills=None, repo=None) -> None:
        entries = (
            skills
            if skills is not None
            else [{"name": "alpha", "path": "${QBRANCH_ROOT}/skills/alpha"}]
        )
        if repo:
            entries.append({"name": "remote", "repo": repo, "path": "skills/remote"})
        value = {
            "schema": 2,
            "name": "fresh",
            "skill_repos": [],
            "skills": entries,
            "links": [],
            "claude_settings": ["${QBRANCH_ROOT}/claude-code/settings.json"],
            "agy_settings": ["${QBRANCH_ROOT}/agy/settings.json"],
        }
        self.manifest.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")

    def set_copy_mode(self) -> None:
        self.sb.spec["args"] = ["--link-mode", "copy"]
        (self.home / ".claude").mkdir(exist_ok=True)
        (self.home / ".gemini" / "antigravity-cli").mkdir(parents=True, exist_ok=True)
        (self.root / "agy").mkdir(exist_ok=True)
        (self.root / "agy" / "settings.json").write_text("{}\n", encoding="utf-8")

    def run_tool(self, *args: str) -> subprocess.CompletedProcess:
        return self.sb.run(TOOL, list(args))

    def assert_ok(self, result: subprocess.CompletedProcess) -> None:
        self.assertEqual(result.returncode, 0, result.stderr)

    def assert_converged(self) -> None:
        plan, rc, err = self.sb.plan(TOOL)
        self.assertEqual(rc, 0, err)
        self.assertFalse(plan.get("failures"), plan)
        self.assertFalse([a for a in plan["actions"] if a["op"] != "ok"], plan)

    def test_copy_mode_skill_changes_reach_both_harnesses_in_one_apply(self) -> None:
        self.set_copy_mode()
        self.write_manifest()
        self.assert_ok(self.run_tool())

        alpha = self.root / "skills" / "alpha" / "SKILL.md"
        alpha.write_text("alpha changed\n", encoding="utf-8")
        self.assert_ok(self.run_tool())
        for relative in (
            Path(".claude/skills"),
            Path(".gemini/antigravity-cli/skills"),
        ):
            self.assertEqual(
                (self.home / relative / "alpha" / "SKILL.md").read_text(),
                "alpha changed\n",
            )
        self.assert_converged()
        beta = self.root / "skills" / "beta"
        beta.mkdir(exist_ok=True)
        (beta / "SKILL.md").write_text("beta added\n", encoding="utf-8")
        self.write_manifest(
            skills=[
                {"name": "alpha", "path": "${QBRANCH_ROOT}/skills/alpha"},
                {"name": "beta", "path": "${QBRANCH_ROOT}/skills/beta"},
            ]
        )
        self.assert_ok(self.run_tool())
        harnesses = (
            Path(".claude/skills"),
            Path(".gemini/antigravity-cli/skills"),
        )
        for relative in harnesses:
            skills = self.home / relative
            self.assertEqual(
                (skills / "alpha" / "SKILL.md").read_text(), "alpha changed\n"
            )
            self.assertEqual((skills / "beta" / "SKILL.md").read_text(), "beta added\n")
        self.assert_converged()

        self.write_manifest(
            skills=[{"name": "beta", "path": "${QBRANCH_ROOT}/skills/beta"}]
        )
        self.assert_ok(self.run_tool())
        for relative in harnesses:
            self.assertFalse((self.home / relative / "alpha").exists())
        self.assert_converged()

    @unittest.skipIf(WINDOWS, "symlink mode requires symlink support")
    def test_symlink_mode_harnesses_follow_changed_skill(self) -> None:
        self.sb.spec["args"] = ["--link-mode", "symlink"]
        self.write_manifest()
        (self.home / ".gemini" / "antigravity-cli").mkdir(parents=True, exist_ok=True)
        (self.root / "agy").mkdir(exist_ok=True)
        (self.root / "agy" / "settings.json").write_text("{}\n", encoding="utf-8")
        self.assert_ok(self.run_tool())
        alpha = self.root / "skills" / "alpha" / "SKILL.md"
        alpha.write_text("alpha changed through link\n", encoding="utf-8")
        harnesses = (
            Path(".claude/skills"),
            Path(".gemini/antigravity-cli/skills"),
        )
        for relative in harnesses:
            installed = self.home / relative / "alpha" / "SKILL.md"
            self.assertEqual(installed.read_text(), "alpha changed through link\n")
        self.assert_converged()

    def test_missing_source_preserves_copy_ownership_until_recovery(self) -> None:
        self.set_copy_mode()
        self.write_manifest()
        self.assert_ok(self.run_tool())
        source = self.root / "skills" / "alpha"
        hidden = source.with_name("alpha-away")
        source.rename(hidden)

        failed = self.run_tool()
        self.assertNotEqual(failed.returncode, 0)
        destinations = [
            self.home / ".agents" / "skills" / "alpha",
            self.home / ".claude" / "skills",
            self.home / ".gemini" / "antigravity-cli" / "skills",
        ]
        state_path = self.home / ".agents" / "skills" / ".qbranch-state.json"
        state = json.loads(state_path.read_text())
        self.assertIn(str(destinations[0]), state["copies"])
        self.assertTrue(all(p.exists() for p in destinations))

        (hidden / "SKILL.md").write_text("recovered\n", encoding="utf-8")
        hidden.rename(source)
        self.assert_ok(self.run_tool())
        harnesses = (
            Path(".claude/skills"),
            Path(".gemini/antigravity-cli/skills"),
        )
        for relative in harnesses:
            installed = self.home / relative / "alpha" / "SKILL.md"
            self.assertEqual(installed.read_text(), "recovered\n")
        self.assert_converged()

        source.rename(hidden)
        self.assertNotEqual(self.run_tool().returncode, 0)
        self.write_manifest(skills=[])
        self.assert_ok(self.run_tool())
        self.assertFalse(destinations[0].exists())
        state = json.loads(
            (self.home / ".agents" / "skills" / ".qbranch-state.json").read_text()
        )
        self.assertNotIn(str(destinations[0]), state["copies"])

    @unittest.skipIf(WINDOWS, "dangling symlink setup is POSIX-specific")
    def test_failed_nested_copy_keeps_complete_install_and_ownership(self) -> None:
        self.set_copy_mode()
        self.write_manifest()
        self.assert_ok(self.run_tool())
        source = self.root / "skills" / "alpha"
        installed = self.home / ".agents" / "skills" / "alpha"
        before = (installed / "SKILL.md").read_bytes()
        (source / "nested").mkdir()
        (source / "nested" / "broken").symlink_to("missing")

        failed = self.run_tool()
        self.assertNotEqual(failed.returncode, 0)
        self.assertEqual((installed / "SKILL.md").read_bytes(), before)
        self.assertFalse((installed / "nested").exists())
        state_path = self.home / ".agents" / "skills" / ".qbranch-state.json"
        state = json.loads(state_path.read_text())
        self.assertIn(str(installed), state["copies"])

        (source / "nested" / "broken").unlink()
        (source / "nested" / "good").write_text("complete\n", encoding="utf-8")
        self.assert_ok(self.run_tool())
        self.assertEqual((installed / "nested" / "good").read_text(), "complete\n")
        self.assert_converged()

    @unittest.skipIf(WINDOWS, "Unix permission bits and symlink replacement")
    def test_private_state_and_new_settings(self) -> None:
        self.write_manifest()
        fragment = self.root / "claude-code" / "settings.json"
        fragment.write_text('{"env":{"EXAMPLE_TOKEN":"secret"}}\n', encoding="utf-8")
        fragment.chmod(0o600)
        settings = self.home / ".claude" / "settings.json"
        settings.unlink()
        state_path = self.home / ".agents" / "skills" / ".qbranch-state.json"
        state_path.parent.mkdir(parents=True, exist_ok=True)
        victim = self.home / "victim.json"
        victim.write_text('{"untouched":true}\n', encoding="utf-8")
        state_path.symlink_to(victim)

        old_umask = os.umask(0o022)
        try:
            self.assert_ok(self.run_tool())
        finally:
            os.umask(old_umask)
        self.assertFalse(state_path.is_symlink())
        self.assertEqual(victim.read_text(), '{"untouched":true}\n')
        self.assertEqual(stat.S_IMODE(state_path.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(settings.stat().st_mode), 0o600)

        state_path.chmod(0o644)
        self.assert_ok(self.run_tool())
        self.assertEqual(stat.S_IMODE(state_path.stat().st_mode), 0o600)

    def test_newer_schema_edit_commands_leave_inputs_unchanged(self) -> None:
        fragment = self.root / "claude-code" / "settings.json"
        commands = [
            ["--add-skill", "alpha"],
            ["--remove-skill", "alpha"],
            ["--fix-renames"],
            ["--manage-plugin", "demo@example", "--in", "base"],
            ["--manage-plugin", "demo@example", "--in", "host"],
        ]
        for command in commands:
            with self.subTest(command=command):
                self.manifest.write_text(
                    '{\n  "schema": 999,\n  "name": "fresh",\n'
                    '  "skills": [{"name":"alpha",'
                    '"path":"${QBRANCH_ROOT}/skills/alpha"}],\n'
                    '  "claude_settings": '
                    '["${QBRANCH_ROOT}/claude-code/settings.json"]\n}\n',
                    encoding="utf-8",
                )
                manifest_before = self.manifest.read_bytes()
                fragment_before = fragment.read_bytes()
                result = self.run_tool(*command)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("understands up to schema", result.stderr)
                self.assertEqual(self.manifest.read_bytes(), manifest_before)
                self.assertEqual(fragment.read_bytes(), fragment_before)

    @unittest.skipIf(WINDOWS, "Unix symlink setup")
    def test_legacy_settings_conversion_preserves_source_and_raw_bytes(self) -> None:
        self.set_copy_mode()
        self.write_manifest()
        (self.root / "claude-code/settings.json").write_text("{}\n")
        source = self.root / "legacy-settings.json"
        original = b'{ "appTheme" : "dark" }\n'
        source.write_bytes(original)
        target = self.home / ".claude/settings.json"
        target.unlink()
        target.symlink_to(source)
        self.assert_ok(self.run_tool())
        self.assertFalse(target.is_symlink())
        self.assertEqual(target.read_bytes(), original)
        self.assertEqual(source.read_bytes(), original)
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)

    @unittest.skipIf(WINDOWS, "Unix symlink setup")
    def test_invalid_legacy_settings_are_left_untouched(self) -> None:
        self.set_copy_mode()
        self.write_manifest()
        source = self.root / "legacy-settings.json"
        source.write_bytes(b"invalid JSON\n")
        target = self.home / ".claude/settings.json"
        target.unlink()
        target.symlink_to(source)
        self.assertNotEqual(self.run_tool().returncode, 0)
        self.assertTrue(target.is_symlink())
        self.assertEqual(source.read_bytes(), b"invalid JSON\n")

    def git(self, cwd: Path, *args: str) -> subprocess.CompletedProcess:
        env = {**self.sb.env, **run_corpus.GIT_SETUP_ENV}
        result = subprocess.run(
            ["git", *args], cwd=cwd, env=env, text=True, capture_output=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result

    def test_reports_do_not_clone_or_pull_repo_cache_but_sync_does(self) -> None:
        upstream = Path(self.sb.dir) / "upstream"
        (upstream / "skills" / "remote").mkdir(parents=True)
        (upstream / "skills" / "remote" / "SKILL.md").write_text("one\n")
        self.git(upstream, "init", "-q")
        self.git(upstream, "add", ".")
        self.git(upstream, "commit", "-qm", "one")
        repo_url = upstream.as_uri()
        self.write_manifest(repo=repo_url)
        cache_root = self.home / ".agents" / "skill-repos"

        reports = (
            ("--dry-run", "--json"),
            ("--dry-run",),
            ("--audit", "--json"),
            ("--audit",),
            ("--fix-renames",),
        )
        for args in reports:
            result = self.run_tool(*args)
            self.assert_ok(result)
            self.assertFalse(cache_root.exists(), (args, result.stdout, result.stderr))
        self.assert_ok(self.run_tool())
        caches = [p for p in cache_root.iterdir() if p.is_dir()]
        self.assertEqual(len(caches), 1)
        cache = caches[0]
        old_head = self.git(cache, "rev-parse", "HEAD").stdout.strip()

        (upstream / "skills" / "remote" / "SKILL.md").write_text("two\n")
        self.git(upstream, "add", ".")
        self.git(upstream, "commit", "-qm", "two")
        for args in reports:
            self.assert_ok(self.run_tool(*args))
            head = self.git(cache, "rev-parse", "HEAD").stdout.strip()
            self.assertEqual(head, old_head)
        self.assert_ok(self.run_tool())
        new_head = self.git(cache, "rev-parse", "HEAD").stdout.strip()
        self.assertNotEqual(new_head, old_head)

    def make_upstream_and_checkout(self, name: str) -> tuple[Path, Path]:
        """An upstream repo with one skill, and a clone of it under ~/src."""
        upstream = Path(self.sb.dir) / "upstream" / name
        (upstream / "skills" / name).mkdir(parents=True)
        (upstream / "skills" / name / "SKILL.md").write_text("one\n")
        self.git(upstream, "init", "-q", "-b", "main")
        self.git(upstream, "add", ".")
        self.git(upstream, "commit", "-qm", "one")
        checkout = self.home / "src" / name
        checkout.parent.mkdir(parents=True, exist_ok=True)
        self.git(checkout.parent, "clone", "-q", upstream.as_uri(), name)
        return upstream, checkout

    def advance(self, upstream: Path, name: str, text: str) -> None:
        (upstream / "skills" / name / "SKILL.md").write_text(text)
        self.git(upstream, "commit", "-qam", text.strip())

    def test_update_fast_forwards_only_safe_checkouts(self) -> None:
        names = ("behind", "dirty", "diverged", "detached")
        pairs = {n: self.make_upstream_and_checkout(n) for n in names}
        for n, (upstream, _) in pairs.items():
            self.advance(upstream, n, "two\n")
        (pairs["dirty"][1] / "skills" / "dirty" / "SKILL.md").write_text("mine\n")
        diverged = pairs["diverged"][1]
        (diverged / "local.txt").write_text("local\n")
        self.git(diverged, "add", ".")
        self.git(diverged, "commit", "-qm", "local")
        self.git(pairs["detached"][1], "checkout", "-q", "--detach")
        heads = {n: self.git(c, "rev-parse", "HEAD").stdout for n, (_, c) in pairs.items()}

        manifest = json.loads(self.manifest.read_text())
        manifest["skill_repos"] = [{"path": f"${{HOME}}/src/{n}"} for n in names]
        self.manifest.write_text(json.dumps(manifest, indent=2) + "\n")

        result = self.run_tool("--update")
        self.assert_ok(result)
        out = result.stdout
        self.assertRegex(out, r"update: behind: \w+\.\.\w+ \(1 commit\)")
        self.assertIn("skipped: dirty: uncommitted changes, not updated", out)
        self.assertIn(
            "skipped: diverged: diverged from origin/main (1 local, 1 upstream), "
            "not updated",
            out,
        )
        self.assertIn("skipped: detached: no upstream branch, not updated", out)
        linked = self.home / ".agents" / "skills" / "behind" / "SKILL.md"
        self.assertEqual(linked.read_text(), "two\n")
        for n in ("dirty", "diverged", "detached"):
            head = self.git(pairs[n][1], "rev-parse", "HEAD").stdout
            self.assertEqual(head, heads[n], n)
        self.assertEqual(
            (pairs["dirty"][1] / "skills" / "dirty" / "SKILL.md").read_text(), "mine\n"
        )

        again = self.run_tool("--update")
        self.assert_ok(again)
        self.assertIn("update: behind: up to date", again.stdout)

    def test_update_refuses_dry_run(self) -> None:
        result = self.run_tool("--update", "--dry-run")
        self.assertEqual(result.returncode, 1)
        self.assertIn("cannot be combined with --dry-run", result.stderr)


@unittest.skipIf(bool(os.environ.get("QBRANCH_BIN")), "Python implementation only")
class PythonAtomicWriteTest(unittest.TestCase):

    def test_replace_failure_preserves_original_bytes(self) -> None:
        namespace = runpy.run_path(str(run_corpus.TOOL), run_name="qbranch_test")
        write_json = namespace["write_json"]
        with run_corpus.tempfile.TemporaryDirectory() as td:
            target = Path(td) / "value.json"
            original = b'{"original": true}\n'
            target.write_bytes(original)
            error = OSError("injected replace failure")
            with mock.patch("os.replace", side_effect=error):
                with self.assertRaises(OSError):
                    write_json(target, {"replacement": True})
            self.assertEqual(target.read_bytes(), original)


if __name__ == "__main__":
    unittest.main(verbosity=2)
