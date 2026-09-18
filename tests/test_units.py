#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Curtis Galloway
# SPDX-License-Identifier: Apache-2.0
"""Fault-injection unit tests for the Python reference's review fixes."""

import contextlib
import io
import json
import os
import runpy
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import run_corpus

TOOL = Path(__file__).resolve().parents[1] / "bin" / "qbranch"


class Units(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.ns = runpy.run_path(str(TOOL), run_name="qbranch_unit")
        # Functions retain their own globals dictionary after runpy returns.
        self.globals = self.ns["write_json"].__globals__
        self.globals["REPO"] = self.root
        self.globals["SKILL_REPOS_CACHE"] = self.root / "cache"

    def test_atomic_failures_preserve_bytes_and_clean_staging(self):
        target = self.root / "settings.json"
        for operation in ("os.fsync", "os.replace"):
            with self.subTest(operation=operation):
                target.write_bytes(b"original\n")
                with mock.patch(operation, side_effect=OSError("injected")):
                    with self.assertRaises(OSError):
                        self.ns["write_json"](target, {"new": True})
                self.assertEqual(target.read_bytes(), b"original\n")
                self.assertEqual(list(self.root.iterdir()), [target])

    def test_serialisation_failure_never_touches_destination(self):
        target = self.root / "settings.json"
        target.write_bytes(b"original\n")
        with self.assertRaises(TypeError):
            self.ns["write_json"](target, {"bad": object()})
        self.assertEqual(target.read_bytes(), b"original\n")
        self.assertEqual(list(self.root.iterdir()), [target])

    def test_partial_write_failure_preserves_original_and_cleans_staging(self):
        target = self.root / "settings.json"
        target.write_bytes(b"original\n")
        original_open = Path.open

        @contextlib.contextmanager
        def failing_open(path, *args, **kwargs):
            with original_open(path, *args, **kwargs) as stream:
                proxy = mock.Mock(wraps=stream)

                def partial_write(text):
                    stream.write(text[:3])
                    stream.flush()
                    raise OSError("disk full after partial write")

                proxy.write.side_effect = partial_write
                yield proxy

        with mock.patch.object(Path, "open", failing_open):
            with self.assertRaisesRegex(OSError, "disk full"):
                self.ns["write_json"](target, {"new": True})
        self.assertEqual(target.read_bytes(), b"original\n")
        self.assertEqual(list(self.root.iterdir()), [target])

    @unittest.skipIf(os.name == "nt", "Unix permissions")
    def test_new_private_and_preserved_permissions(self):
        target = self.root / "settings.json"
        self.ns["write_json"](target, {})
        self.assertEqual(target.stat().st_mode & 0o777, 0o600)
        target.chmod(0o640)
        self.ns["write_json"](target, {"policy": True})
        self.assertEqual(target.stat().st_mode & 0o777, 0o640)
        self.ns["write_json"](target, {}, private=True)
        self.assertEqual(target.stat().st_mode & 0o777, 0o600)

    @unittest.skipIf(os.name == "nt", "Unix symlink setup")
    def test_regular_write_follows_link_private_write_replaces_it(self):
        target = self.root / "target.json"
        link = self.root / "link.json"
        target.write_text("{}\n")
        link.symlink_to(target)
        self.ns["write_json"](link, {"ordinary": True})
        self.assertTrue(link.is_symlink())
        self.assertEqual(json.loads(target.read_text()), {"ordinary": True})
        self.ns["write_json"](link, {"private": True}, private=True)
        self.assertFalse(link.is_symlink())
        self.assertEqual(json.loads(target.read_text()), {"ordinary": True})

    def test_copy_publish_failure_restores_old_destination(self):
        source, dest = self.root / "source", self.root / "dest"
        source.write_text("new")
        dest.write_text("old")
        original = Path.rename

        def rename(path, target):
            if path.name == "new":
                raise OSError("publish failed")
            return original(path, target)

        with mock.patch.object(Path, "rename", rename):
            with self.assertRaisesRegex(OSError, "publish failed"):
                self.ns["copy_path"](source, dest)
        self.assertEqual(dest.read_text(), "old")
        self.assertEqual(set(self.root.iterdir()), {source, dest})

    def test_copy_failed_rollback_preserves_recovery_backup(self):
        source, dest = self.root / "source", self.root / "dest"
        source.write_text("new")
        dest.write_text("old")
        original = Path.rename

        def rename(path, target):
            if path.name in {"new", "old"}:
                raise OSError("injected")
            return original(path, target)

        with mock.patch.object(Path, "rename", rename):
            with self.assertRaisesRegex(OSError, "previous copy retained at") as error:
                self.ns["copy_path"](source, dest)
        backups = list(self.root.glob(".qbranch-copy-*/old"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), "old")
        self.assertIn(str(backups[0]), str(error.exception))

    def test_copy_failure_before_backup_keeps_original(self):
        dest = self.root / "dest"
        dest.write_text("old")
        with self.assertRaises(OSError):
            self.ns["copy_path"](self.root / "absent", dest)
        self.assertEqual(dest.read_text(), "old")
        self.assertEqual(list(self.root.iterdir()), [dest])

    def test_directory_copy_ignores_state_and_detects_changes(self):
        source, dest = self.root / "source", self.root / "dest"
        source.mkdir()
        (source / "skill").write_text("content")
        for name in self.ns["COPY_IGNORE"]:
            (source / name).write_text("private state")
        self.ns["copy_path"](source, dest)
        self.assertEqual([p.name for p in dest.iterdir()], ["skill"])
        self.assertTrue(self.ns["copy_up_to_date"](source, dest))
        (source / "skill").write_text("changed")
        self.assertFalse(self.ns["copy_up_to_date"](source, dest))
        self.assertFalse(self.ns["copy_up_to_date"](self.root / "absent", dest))

    def test_read_only_resolution_never_invokes_git(self):
        with mock.patch.object(Path, "home", return_value=self.root):
            with mock.patch("subprocess.run") as run:
                for cached in (False, True):
                    if cached:
                        (self.root / "cache" / "demo" / ".git").mkdir(parents=True)
                    actual = self.ns["resolve_repo_local"](
                        "https://example.invalid/demo.git"
                    )
                    self.assertEqual(actual, self.root / "cache" / "demo")
                run.assert_not_called()

    def test_local_checkout_takes_precedence_even_during_update(self):
        local = self.root / "src" / "demo"
        (local / ".git").mkdir(parents=True)
        with mock.patch.object(Path, "home", return_value=self.root):
            with mock.patch("subprocess.run") as run:
                self.assertEqual(
                    self.ns["resolve_repo_local"](
                        "https://example.invalid/demo.git", update=True
                    ),
                    local,
                )
                run.assert_not_called()

    def test_preparation_fetches_each_repository_once(self):
        resolver = mock.Mock()
        with mock.patch.dict(self.globals, resolve_repo_local=resolver):
            self.ns["prepare_skill_repos"](
                {
                    "skills": [
                        {"repo": "one"},
                        {"repo": "one"},
                        {"path": "local"},
                        {"repo": "two"},
                    ]
                }
            )
        self.assertEqual(
            resolver.call_args_list,
            [mock.call("one", update=True), mock.call("two", update=True)],
        )

    def test_failed_removal_retains_ownership_and_retries(self):
        sb = run_corpus.Sandbox("copy-mode")
        self.addCleanup(sb.close)
        with mock.patch.dict(os.environ, sb.env, clear=True):
            ns = runpy.run_path(str(TOOL), run_name="qbranch_sync_test")
            globals_ = ns["main"].__globals__
            with mock.patch.object(sys, "argv", sb.argv([str(TOOL)], [])):
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(ns["main"](), 0)
                    manifest = Path(sb.dir) / "root/manifests/copy.json"
                    value = json.loads(manifest.read_text())
                    value["skills"] = [
                        e for e in value["skills"] if e["name"] != "alpha"
                    ]
                    manifest.write_text(json.dumps(value))
                    dest = sb.home / ".agents/skills/alpha"
                    remove = globals_["remove_path"]

                    def fail_remove(path):
                        if path == dest:
                            raise OSError("injected removal failure")
                        return remove(path)

                    with mock.patch.dict(
                        globals_, remove_path=fail_remove, offer_claude=lambda _: None
                    ):
                        with contextlib.redirect_stderr(io.StringIO()):
                            self.assertEqual(ns["main"](), 1)
                    state = json.loads(
                        (dest.parent / ".qbranch-state.json").read_text()
                    )
                    self.assertIn(str(dest), state["links"])
                    self.assertIn(str(dest), state["copies"])
                    self.assertTrue(dest.is_dir())
                    self.assertEqual(ns["main"](), 0)
                    self.assertFalse(dest.exists())

    def test_future_schema_edits_refuse_before_writing(self):
        directory = self.root / "manifests"
        directory.mkdir()
        path = directory / "future.json"
        raw = '{"schema":999,"skills":[{"name":"alpha","path":"missing"}]}\n'
        calls = [
            ("add_skill_to_manifest", ("future", "beta")),
            ("remove_skill_from_manifest", ("future", "alpha")),
            ("fix_renames_in_manifest", ("future",)),
            ("manage_plugin", ("future", "demo@example", "host", True)),
        ]
        for name, args in calls:
            with self.subTest(command=name):
                path.write_text(raw)
                with self.assertRaisesRegex(SystemExit, "understands up to schema"):
                    self.ns[name](*args)
                self.assertEqual(path.read_text(), raw)
                self.assertEqual(list(self.root.iterdir()), [directory])

    def test_supported_schema_edit_does_not_implicitly_upgrade(self):
        directory = self.root / "manifests"
        directory.mkdir()
        path = directory / "old.json"
        path.write_text('{"schema":1,"skills":[],"custom":"keep"}')
        self.assertTrue(self.ns["add_skill_to_manifest"]("old", "alpha"))
        value = json.loads(path.read_text())
        self.assertEqual(value["schema"], 1)
        self.assertEqual(value["custom"], "keep")
        self.assertNotIn("skill_repos", value)


if __name__ == "__main__":
    unittest.main(verbosity=2)
