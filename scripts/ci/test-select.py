#!/usr/bin/env python3
"""Exercise CI selection, fresh Git histories and Python entry-point imports."""

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SPEC = importlib.util.spec_from_file_location("ci_select", Path(__file__).with_name("select-scope.py"))
SELECT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SELECT)


class EntrypointImportTests(unittest.TestCase):
    def test_ci_script_names_do_not_shadow_standard_library(self):
        # Built-in modules and preload order differ between Python builds.
        # Check names independently of this interpreter's import resolution.
        directory = Path(__file__).resolve().parent
        conflicts = sorted(str(path.relative_to(directory))
                           for path in directory.rglob("*.py")
                           if path.stem in sys.stdlib_module_names)
        self.assertEqual(conflicts, [], "CI scripts must not shadow Python's standard library")

    def test_settings_protocol_runner_imports_before_refusing_host_execution(self):
        # No arguments: only imports and the refusal guard can run. Skip site
        # initialization without hiding the script directory from Python.
        result = subprocess.run(
            [sys.executable, "-S", str(Path(__file__).with_name("settings-services.py"))],
            capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr.strip(),
                         "Requires the ordinary build user in the native Alpine acceptance container")


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
