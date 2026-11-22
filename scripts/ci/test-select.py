#!/usr/bin/env python3
"""Exercise CI selection against real Git histories, including a fresh root."""

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("ci_select", Path(__file__).with_name("select.py"))
SELECT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SELECT)


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="alcard-ci-select-")
        self.root = Path(self.directory.name)
        self.git("init", "-q")
        self.git("config", "user.name", "CI selection fixture")
        self.git("config", "user.email", "ci-fixture@localhost")
        self.commit("README.md", "Initial source\n")
        self.base = self.git("rev-parse", "HEAD").strip()

    def tearDown(self):
        self.directory.cleanup()

    def git(self, *arguments):
        return subprocess.check_output(["git", *arguments], cwd=self.root, text=True,
                                       stderr=subprocess.DEVNULL)

    def commit(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        self.git("add", "--all")
        self.git("commit", "-q", "-m", "Fixture input")

    def scope(self, event="push", before=None, mode="source"):
        base = self.base if before is None else before
        payload = {"before": base, "pull_request": {"base": {"sha": base}}}
        return SELECT.choose_scope(event, payload, mode, self.git)[0]

    def test_documentation_and_legal_only_stay_static(self):
        for name in ("docs/en/status.md", "README.ru.md", "LICENSE", "licenses/example.txt"):
            self.commit(name, "Documentation fixture\n")
        self.assertEqual(self.scope(), "static")
        self.assertEqual(self.scope(event="pull_request"), "static")

    def test_code_and_build_paths_cannot_be_skipped(self):
        for name in ("rust/app.rs", "board/device.dts", "patches/driver.patch", "keyboard-zero/map",
                     "packaging/apk/demo/APKBUILD", "scripts/build.sh", "tools/check.py",
                     ".github/workflows/ci.yml", "new-component/input"):
            with self.subTest(path=name):
                self.commit(name, "Changed input\n")
                self.assertEqual(self.scope(), "source")
                self.base = self.git("rev-parse", "HEAD").strip()

    def test_missing_history_never_skips_source(self):
        self.assertEqual(self.scope(before="0" * 40), "source")
        self.assertEqual(self.scope(before="f" * 40), "source")
        self.assertEqual(self.scope(before="--unsafe-option"), "source")
        self.assertEqual(self.scope(), "source")  # Empty diff still validates.

    def test_deleted_code_remains_relevant(self):
        self.commit("board/device.dts", "Fixture\n")
        self.base = self.git("rev-parse", "HEAD").strip()
        (self.root / "board/device.dts").unlink()
        self.git("add", "--all")
        self.git("commit", "-q", "-m", "Remove fixture")
        self.assertEqual(self.scope(), "source")

    def test_only_explicit_dispatch_selects_acceptance(self):
        self.assertEqual(self.scope(event="workflow_dispatch", mode="source"), "source")
        self.assertEqual(self.scope(event="workflow_dispatch", mode="acceptance"), "acceptance")
        self.commit("README.md", "Changed documentation\n")
        self.assertEqual(self.scope(mode="acceptance"), "static")
        with self.assertRaises(ValueError):
            self.scope(event="workflow_dispatch", mode="invalid")


if __name__ == "__main__":
    unittest.main()
