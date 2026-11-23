#!/usr/bin/env python3
"""Exercise Hello baseline trust and source reuse without network or native fixtures."""

import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile


SPEC = importlib.util.spec_from_file_location(
    "hello_baseline", Path(__file__).with_name("hello-baseline.py"))
BASELINE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BASELINE
SPEC.loader.exec_module(BASELINE)

REPOSITORY = "example/alcard"
RUN_ID = 12345
SOURCE_SHA = "1" * 40


def run_api(sha=SOURCE_SHA):
    return {
        "id": RUN_ID, "run_attempt": 1, "name": "Alcard CI",
        "event": "workflow_dispatch", "path": ".github/workflows/ci.yml",
        "head_branch": "main", "head_sha": sha,
        "status": "completed", "conclusion": "failure",
        "repository": {"id": 123, "full_name": REPOSITORY},
        "head_repository": {"id": 123, "full_name": REPOSITORY},
    }


def artifact_api(sha=SOURCE_SHA):
    return {
        "id": 456, "name": f"alcard-CI-ARM64-{RUN_ID}-1",
        "digest": "sha256:" + "2" * 64, "size_in_bytes": 1024,
        "expired": False,
        "workflow_run": {
            "id": RUN_ID, "head_sha": sha, "head_branch": "main",
            "repository_id": 123, "head_repository_id": 123,
        },
    }


def metadata(sha=SOURCE_SHA):
    base = BASELINE.validate_run_api(run_api(sha), REPOSITORY, str(RUN_ID))
    return BASELINE.select_artifact(
        base, {"total_count": 1, "artifacts": [artifact_api(sha)]})


class ApiTrustTests(unittest.TestCase):
    def test_completed_same_repository_main_dispatch_is_accepted(self):
        for conclusion in ("failure", "success"):
            with self.subTest(conclusion=conclusion):
                run = run_api()
                run["conclusion"] = conclusion
                result = BASELINE.validate_run_api(run, REPOSITORY, str(RUN_ID))
                self.assertEqual(result["repository"], REPOSITORY)
                self.assertEqual(result["run_id"], RUN_ID)
                self.assertEqual(result["source_sha"], SOURCE_SHA)
                self.assertEqual(result["conclusion"], conclusion)
        BASELINE.validate_metadata(metadata(), REPOSITORY)

    def test_wrong_origin_trigger_state_and_identity_are_rejected(self):
        changes = (
            {"repository": {"id": 999, "full_name": "someone/alcard"}},
            {"head_repository": {"id": 999, "full_name": "someone/alcard"}},
            {"head_branch": "feature"},
            {"event": "pull_request"},
            {"path": ".github/workflows/unrelated.yml"},
            {"status": "in_progress"},
            {"conclusion": "cancelled"},
            {"id": RUN_ID + 1},
            {"head_sha": "not-a-commit"},
        )
        for change in changes:
            with self.subTest(change=change), self.assertRaises(BASELINE.ValidationError):
                run = run_api()
                run.update(change)
                BASELINE.validate_run_api(run, REPOSITORY, str(RUN_ID))

    def test_artifact_must_belong_to_exact_run_attempt_and_commit(self):
        base = BASELINE.validate_run_api(run_api(), REPOSITORY, str(RUN_ID))
        changes = (
            {"name": f"alcard-CI-ARM64-{RUN_ID}-2"},
            {"expired": True},
            {"digest": "2" * 64},
            {"size_in_bytes": 0},
            {"workflow_run": {"id": RUN_ID + 1, "head_sha": SOURCE_SHA,
                              "head_branch": "main"}},
            {"workflow_run": {"id": RUN_ID, "head_sha": "3" * 40,
                              "head_branch": "main"}},
        )
        for change in changes:
            with self.subTest(change=change), self.assertRaises(BASELINE.ValidationError):
                artifact = artifact_api()
                artifact.update(change)
                BASELINE.select_artifact(base, {"total_count": 1, "artifacts": [artifact]})
        for artifacts in ([], [artifact_api(), artifact_api()]):
            with self.subTest(count=len(artifacts)), self.assertRaises(BASELINE.ValidationError):
                BASELINE.select_artifact(base, {"total_count": len(artifacts),
                                                "artifacts": artifacts})

    def test_saved_metadata_does_not_relax_repository_or_digest_checks(self):
        for change in ({"repository": "someone/alcard"}, {"source_sha": "bad"}):
            with self.subTest(change=change), self.assertRaises(BASELINE.ValidationError):
                value = metadata()
                value.update(change)
                BASELINE.validate_metadata(value, REPOSITORY)
        for change in ({"digest": "sha256:" + "z" * 64}, {"size": -1}):
            with self.subTest(change=change), self.assertRaises(BASELINE.ValidationError):
                value = metadata()
                value["artifact"].update(change)
                BASELINE.validate_metadata(value, REPOSITORY)


class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="alcard-hello-zip-")
        self.root = Path(self.directory.name)
        self.addCleanup(self.directory.cleanup)
        self.archive = self.root / "baseline.zip"
        self.destination = self.root / "extracted"
        self.extract_count = 0

    def archive_bytes(self, entries):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)  # Deliberate duplicate ZIP entries.
            with zipfile.ZipFile(self.archive, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                for name, content, mode in entries:
                    member = zipfile.ZipInfo(name)
                    member.create_system = 3
                    member.external_attr = mode << 16
                    if stat.S_ISDIR(mode):
                        member.external_attr |= 0x10
                    archive.writestr(member, content)
        return self.archive.read_bytes()

    def extract(self, entries, *, digest=None, size=None):
        self.extract_count += 1
        self.destination = self.root / f"extracted-{self.extract_count}"
        data = self.archive_bytes(entries)
        BASELINE.safe_extract(
            self.archive, self.destination,
            digest if digest is not None else "sha256:" + hashlib.sha256(data).hexdigest(),
            len(data) if size is None else size)

    def test_verified_regular_files_and_directories_extract(self):
        self.extract([
            ("ci/", b"", stat.S_IFDIR | 0o755),
            ("ci/selection.json", b'{"scope":"acceptance"}\n', stat.S_IFREG | 0o644),
            ("ci-packages/status.txt", b"exit_status=0\n", stat.S_IFREG | 0o644),
        ])
        self.assertEqual((self.destination / "ci-packages/status.txt").read_bytes(),
                         b"exit_status=0\n")

    def test_digest_and_api_size_are_checked_before_use(self):
        entries = [("ci/result.json", b"{}", stat.S_IFREG | 0o644)]
        for kwargs in ({"digest": "sha256:" + "0" * 64}, {"size": 1}):
            with self.subTest(kwargs=kwargs), self.assertRaises(BASELINE.ValidationError):
                self.extract(entries, **kwargs)
        self.assertFalse((self.destination / "ci/result.json").exists())

    def test_path_traversal_absolute_paths_and_symlinks_are_rejected(self):
        for name in ("../escaped", "/escaped", "ci/../../escaped", "ci\\..\\escaped"):
            with self.subTest(name=name), self.assertRaises(BASELINE.ValidationError):
                self.extract([(name, b"unsafe", stat.S_IFREG | 0o644)])
        with self.assertRaises(BASELINE.ValidationError):
            self.extract([("ci/link", b"../../escaped", stat.S_IFLNK | 0o777)])
        self.assertFalse((self.root / "escaped").exists())

    def test_duplicate_names_and_file_directory_conflicts_are_rejected(self):
        variants = (
            [("ci/result.json", b"first", stat.S_IFREG | 0o644),
             ("ci/result.json", b"second", stat.S_IFREG | 0o644)],
            [("ci/item", b"file", stat.S_IFREG | 0o644),
             ("ci/item/child", b"child", stat.S_IFREG | 0o644)],
        )
        for entries in variants:
            with self.subTest(entries=entries), self.assertRaises(BASELINE.ValidationError):
                self.extract(entries)

    def test_archive_member_count_and_expanded_size_limits_are_enforced(self):
        entries = [("ci/one", b"1234", stat.S_IFREG | 0o644),
                   ("ci/two", b"5678", stat.S_IFREG | 0o644)]
        for constant, limit in (("MAX_ARCHIVE_BYTES", 1), ("MAX_MEMBERS", 1),
                                ("MAX_MEMBER_BYTES", 3), ("MAX_TOTAL_BYTES", 7)):
            with self.subTest(constant=constant), patch.object(BASELINE, constant, limit):
                with self.assertRaises(BASELINE.ValidationError):
                    self.extract(entries)


class OutputPathTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="alcard-hello-output-")
        self.root = Path(self.directory.name).resolve()
        self.addCleanup(self.directory.cleanup)
        self.local = self.root / ".alcard-local"
        self.local.mkdir()

    def test_new_child_of_local_output_directory_is_accepted(self):
        path = self.local / "acceptance" / "baseline"
        self.assertEqual(BASELINE.output_path(path, self.root), path)
        self.assertFalse(path.exists())

    def test_parent_traversal_and_outside_destinations_are_rejected(self):
        paths = (self.local / ".." / ".." / "outside",
                 self.root / "outside", self.local)
        for path in paths:
            with self.subTest(path=path), self.assertRaises(BASELINE.ValidationError):
                BASELINE.output_path(path, self.root)

    def test_symlink_output_ancestor_is_rejected(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.local / "redirect").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(BASELINE.ValidationError):
            BASELINE.output_path(self.local / "redirect" / "baseline", self.root)
        self.assertFalse((outside / "baseline").exists())

    def test_existing_output_is_rejected(self):
        path = self.local / "baseline"
        path.mkdir()
        with self.assertRaises(BASELINE.ValidationError):
            BASELINE.output_path(path, self.root)


class SourceReuseTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="alcard-hello-source-")
        self.root = Path(self.directory.name)
        self.addCleanup(self.directory.cleanup)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Hello baseline fixture")
        self.git("config", "user.email", "ci-fixture@localhost")
        self.git("config", "core.filemode", "true")
        self.write(".gitignore", ".alcard-local/\n")
        self.write("rust/app.rs", "fn main() {}\n")
        self.write("packaging/apk/demo/APKBUILD", "pkgname=demo\npkgver=0.1.0\npkgrel=0\n")
        self.write("tools/package_metadata.py", "# Fixed package metadata reader\n")
        self.write("scripts/ci/hello/driver.py", "# Baseline Hello driver\n")
        self.write("docs/en/testing.md", "# Testing\n")
        self.commit()
        self.base = self.git("rev-parse", "HEAD").strip()
        paths = self.git("ls-files", "-z").split("\0")
        self.before = {"head": self.base, "tracked_status": "", "files": {
            name: hashlib.sha256((self.root / name).read_bytes()).hexdigest()
            for name in paths if name}}
        self.after = copy.deepcopy(self.before)
        self.metadata = metadata(self.base)

    def git(self, *arguments):
        return subprocess.check_output(["git", *arguments], cwd=self.root,
                                       text=True, stderr=subprocess.DEVNULL)

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def commit(self):
        self.git("add", "--all")
        self.git("commit", "-q", "-m", "Fixture input")

    def validate(self):
        return BASELINE.validate_source(self.root, self.metadata, self.before, self.after)

    def test_exact_baseline_and_ignored_local_evidence_are_accepted(self):
        self.write(".alcard-local/evidence.json", "{}\n")
        result = self.validate()
        self.assertEqual(result["allowed_changed_paths"], [])

    def test_committed_harness_and_documentation_updates_are_accepted(self):
        changes = {"scripts/ci/hello/qemu_platform.py", "scripts/ci/settings/observer.py",
                   "docs/en/testing.md"}
        for name in changes:
            self.write(name, "# Updated fixture or documentation\n")
        self.commit()
        self.assertEqual(set(self.validate()["allowed_changed_paths"]), changes)

    def test_fingerprints_must_match_complete_baseline_git_blobs(self):
        original = copy.deepcopy(self.before)
        variants = []
        missing = copy.deepcopy(original)
        del missing["files"]["rust/app.rs"]
        variants.append(missing)
        forged = copy.deepcopy(original)
        forged["files"]["rust/app.rs"] = "0" * 64
        variants.append(forged)
        extra = copy.deepcopy(original)
        extra["files"]["untracked.txt"] = "0" * 64
        variants.append(extra)
        dirty = copy.deepcopy(original)
        dirty["tracked_status"] = " M rust/app.rs\n"
        variants.append(dirty)
        for before in variants:
            with self.subTest(before=before), self.assertRaises(BASELINE.ValidationError):
                BASELINE.validate_source(self.root, self.metadata, before, copy.deepcopy(before))
        self.after["files"]["rust/app.rs"] = "0" * 64
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_uncommitted_allowed_harness_edit_is_rejected(self):
        self.write("scripts/ci/hello/driver.py", "# Uncommitted change\n")
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_staged_allowed_harness_edit_is_rejected(self):
        self.write("scripts/ci/hello/driver.py", "# Staged change\n")
        self.git("add", "scripts/ci/hello/driver.py")
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_assume_unchanged_cannot_hide_modified_production_bytes(self):
        self.git("update-index", "--assume-unchanged", "rust/app.rs")
        self.write("rust/app.rs", 'fn main() { println!("Hidden change"); }\n')
        self.assertEqual(self.git("status", "--porcelain", "--untracked-files=all"), "")
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_untracked_source_input_is_rejected(self):
        self.write("scripts/ci/hello/extra.py", "# Not part of the checked-out commit\n")
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_production_code_change_is_rejected(self):
        self.write("rust/app.rs", 'fn main() { println!("Changed"); }\n')
        self.commit()
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_production_rename_into_allowed_docs_is_rejected(self):
        self.git("config", "diff.renames", "true")
        self.git("mv", "rust/app.rs", "docs/en/moved-code.md")
        self.commit()
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_production_mode_change_is_rejected(self):
        os.chmod(self.root / "rust/app.rs", 0o755)
        self.commit()
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_identical_tree_without_baseline_ancestry_is_rejected(self):
        original_tree = self.git("rev-parse", "HEAD^{tree}")
        self.git("checkout", "--orphan", "unrelated")
        self.git("commit", "-q", "-m", "Unrelated root with the same files")
        self.assertEqual(self.git("rev-parse", "HEAD^{tree}"), original_tree)
        with self.assertRaises(BASELINE.ValidationError):
            self.validate()

    def test_allowlist_is_scoped_to_exact_files_and_directories(self):
        allowed = (
            ".github/workflows/ci.yml", "scripts/ci/run.sh", "scripts/ci/select-scope.py",
            "scripts/ci/test-select.py", "scripts/ci/hello-baseline.py",
            "scripts/ci/test-hello-baseline.py", "scripts/ci/hello/driver.py",
            "scripts/ci/settings/observer.py", "docs/en/testing.md", "docs/ru/testing.md", "docs/README.md",
        )
        forbidden = (
            "scripts/ci/container.sh", "rust/app.rs", "packaging/apk/demo/APKBUILD",
            "tools/package_metadata.py", "README.md", ".github/workflows/publish.yml",
            "docs/en/tool.py", "scripts/ci/hello-extra/driver.py", "scripts/ci/settings-extra.py",
        )
        for name in allowed:
            with self.subTest(allowed=name):
                self.assertTrue(BASELINE.allowed_change(name))
        for name in forbidden:
            with self.subTest(forbidden=name):
                self.assertFalse(BASELINE.allowed_change(name))


class ReceiptTests(unittest.TestCase):
    """Use the real receipt shapes and catalog reader, with synthetic bytes only."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="alcard-hello-receipts-")
        self.addCleanup(self.directory.cleanup)
        self.repo = Path(self.directory.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Hello receipt fixture")
        self.git("config", "user.email", "ci-fixture@localhost")
        self.write(self.repo / ".gitignore", ".alcard-local/\n")
        self.write(self.repo / "rust/app.rs", "fn main() {}\n")
        tool = Path(__file__).resolve().parents[2] / "tools/package_metadata.py"
        self.write(self.repo / "tools/package_metadata.py", tool.read_text())
        users = ("alcard-hello", "alcard-keyboard-zero", "alcard-fonts-console", "alcard-fonts-gui",
                 "alcard-desktop-minimal", "alcard-shell", "alcard-polkit-agent", "alcard-lock",
                 "alcard-desktop-shell")
        for name in (*users, "alcard-kmods-rpi", *(f"extra-recipe-{index}" for index in range(5))):
            recipe = f"pkgname={name}\npkgver=0.1.0\npkgrel=0\ndepends=\nmakedepends=\n"
            if name == "alcard-kmods-rpi":
                recipe += "_kernrel=0\n"
            self.write(self.repo / f"packaging/apk/{name}/APKBUILD", recipe)
        self.git("add", "--all")
        self.git("commit", "-q", "-m", "Synthetic receipt inputs")
        sha = self.git("rev-parse", "HEAD").strip()
        self.meta = metadata(sha)
        self.artifact = self.repo / ".alcard-local/original"
        fingerprint = {"head": sha, "tracked_status": "", "files": {
            path: hashlib.sha256((self.repo / path).read_bytes()).hexdigest()
            for path in self.git("ls-files", "-z").split("\0") if path}}
        for phase in ("before", "after"):
            self.record(f"ci/acceptance/inputs-{phase}.json", fingerprint)
        self.record("ci/selection.json", {
            "scope": "acceptance", "event": "workflow_dispatch", "source_sha": sha,
            "source_tree": self.git("rev-parse", "HEAD^{tree}").strip()})
        stages = [{"name": name, "exit_code": int(index == 6), "seconds": 1,
                   "command": ["synthetic-fixture"]} for index, name in enumerate(BASELINE.STAGES)]
        self.record("ci/acceptance/result.json", {
            "schema": 1, "exit_code": 1, "last_stage": BASELINE.STAGES[-1],
            "inputs_unchanged": True, "stages": stages})
        for stage in stages:
            self.record(f"ci/acceptance/stages/{stage['name']}.json", stage)
        source_checks = []
        for name in BASELINE.SOURCE_CHECKS:
            data = f"Synthetic source check {name}\n"
            self.write(self.artifact / f"ci/acceptance/native/source/{name}.log", data)
            source_checks.append({"name": name, "exit_code": 0, "log": f"{name}.log",
                                  "log_sha256": hashlib.sha256(data.encode()).hexdigest()})
        self.record("ci/acceptance/native/source/result.json", {
            "exit_code": 0, "inputs_unchanged": True, "inputs_before": fingerprint["files"],
            "inputs_after": fingerprint["files"], "checks": source_checks})
        services = []
        for name, test in BASELINE.SERVICES.items():
            services.append({"name": name, "test": test, "exit_code": 0})
            self.write(self.artifact / f"ci/acceptance/native/settings-services/{name}.log",
                       self.native_log(test))
        self.record("ci/acceptance/native/settings-services/result.json", {"exit_code": 0, "cases": services})
        self.catalog = BASELINE.current_catalog(self.repo)
        for path in ("ci/static/package-recipes.json", "ci/acceptance/native/package-recipes.json"):
            self.record(path, self.catalog)
        self.apk_hashes = {}
        for name in self.catalog["groups"]["user"]:
            relative = "packages/aarch64/" + self.catalog["packages"][name]["filename"]
            data = f"Synthetic APK bytes: {name}\n"
            self.write(self.artifact / "ci-packages" / relative, data)
            self.apk_hashes[relative] = hashlib.sha256(data.encode()).hexdigest()
        self.write(self.artifact / "ci-packages/status.txt", "exit_status=0\n")
        self.write(self.artifact / "ci-packages/packages/aarch64/APKINDEX.tar.gz", "Synthetic package index\n")
        self.write(self.artifact / "ci-packages/fixture.rsa.pub", "-----BEGIN PUBLIC KEY-----\nfixture\n")
        self.write(self.artifact / "ci-packages/SHA256SUMS",
                   "".join(f"{digest}  {path}\n" for path, digest in self.apk_hashes.items()))
        self.shell_hash = "a" * 64
        self.record("ci-packages/shell-install-hashes.json", {"status": "PASS", "files": {
            path: {"sha256": {stage: self.shell_hash for stage in
                              ("built", "staged", "clean_rootfs", "installed_container")}}
            for path in BASELINE.SHELL_FILES}})
        for build in ("login-build", "hello-build"):
            self.write(self.artifact / f"ci-login/evidence/{build}/artifact-hashes.log",
                       "".join(f"{path}: OK\n" for path in self.apk_hashes))
        self.record("ci/acceptance/runtime/result.json", {
            "status": "PASS", "exit_code": 0,
            "stages": [{"name": name, "exit_code": 0} for name in BASELINE.RUNTIME_CASES]})
        self.write(self.artifact / "ci/acceptance/runtime/notifications.log",
                   self.native_log("notifications::tests::isolated_dbus_notification_lifecycle"))
        self.write(self.artifact / "ci/acceptance/runtime/idle/test.log",
                   self.native_log("integration::real_idle_lock_and_output_power"))
        self.record("ci/acceptance/runtime/audio/checks.json", {
            "status": "PASS", "verified": [f"Audio receipt {index}" for index in range(7)]})
        for directory, names, status in (("output-power", BASELINE.POWER_CASES, "PASS"),
                                         ("battery-sleep-backlight", BASELINE.BATTERY_CASES, "passed")):
            self.record(f"ci/acceptance/runtime/{directory}/result.json", {
                "status": status, "shell": {"sha256": self.shell_hash},
                "cases": [{"name": name, "status": status} for name in names]})
        for mode in BASELINE.LOGIN_MODES:
            self.record(f"ci-login/evidence/{mode}/events.json", {"success": True, "events": ["observed"]})
        environment = patch.dict(os.environ, {"GITHUB_REPOSITORY": REPOSITORY})
        environment.start()
        self.addCleanup(environment.stop)

    def git(self, *arguments):
        return subprocess.check_output(["git", *arguments], cwd=self.repo, text=True,
                                       stderr=subprocess.DEVNULL)

    @staticmethod
    def write(path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def record(self, name, value):
        self.write(self.artifact / name, json.dumps(value) + "\n")

    @staticmethod
    def native_log(name):
        return f"test {name} ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n"

    def validate(self):
        return BASELINE.validate_receipts(self.artifact, self.meta, self.repo)

    def test_prepare_preserves_failed_full_result_and_stages_exact_apk_input(self):
        self.assertEqual(len(self.catalog["packages"]), 15)
        archive = self.repo / ".alcard-local/baseline.zip"
        with zipfile.ZipFile(archive, "w") as output:
            for path in self.artifact.rglob("*"):
                if path.is_file():
                    output.write(path, path.relative_to(self.artifact).as_posix())
        self.meta["artifact"].update(size=archive.stat().st_size,
                                     digest="sha256:" + BASELINE.sha256_file(archive))
        destination, inputs = (self.repo / ".alcard-local" / name for name in ("verified", "login-input"))
        summary = BASELINE.prepare_local(self.meta, archive, destination, inputs, self.repo)
        self.assertEqual(summary["status"], "PASS")
        self.assertEqual(summary["original_result"]["exit_code"], 1)
        self.assertEqual(summary["apk_sha256"], self.apk_hashes)
        self.assertEqual(BASELINE.read_json(destination / "verification.json"), summary)
        self.assertEqual((destination / "artifact/ci/acceptance/result.json").read_bytes(),
                         (self.artifact / "ci/acceptance/result.json").read_bytes())
        expected = set(self.apk_hashes) | {"packages/aarch64/APKINDEX.tar.gz", "fixture.rsa.pub",
                                         "SHA256SUMS", "status.txt"}
        self.assertEqual({path.relative_to(inputs).as_posix() for path in inputs.rglob("*") if path.is_file()},
                         expected)
        for name in expected:
            self.assertEqual((inputs / name).read_bytes(), (self.artifact / "ci-packages" / name).read_bytes())

    def test_successful_full_run_can_be_explicitly_repeated(self):
        self.meta["conclusion"] = "success"
        path = "ci/acceptance/result.json"
        original = BASELINE.read_json(self.artifact / path)
        original.update(exit_code=0, last_stage="complete")
        original["stages"][-1]["exit_code"] = 0
        self.record(path, original)
        self.record(f"ci/acceptance/stages/{BASELINE.STAGES[-1]}.json", original["stages"][-1])
        self.assertEqual(self.validate()["original_result"]["exit_code"], 0)

    def test_missing_altered_or_failed_gate_receipts_are_rejected(self):
        cases = (
            ("ci/selection.json", lambda value: value.update(scope="hello")),
            ("ci/acceptance/result.json", lambda value: value.update(exit_code=0)),
            ("ci/acceptance/result.json", lambda value: value["stages"][0].update(exit_code=1)),
            (f"ci/acceptance/stages/{BASELINE.STAGES[2]}.json", lambda value: value.update(seconds=20)),
            ("ci/acceptance/native/source/result.json", lambda value: value["checks"].pop()),
            ("ci/acceptance/native/source/result.json", lambda value: value.update(inputs_unchanged=False)),
            ("ci/acceptance/native/settings-services/result.json", lambda value: value["cases"].pop()),
            ("ci/acceptance/runtime/result.json", lambda value: value["stages"][1].update(exit_code=1)),
            ("ci/acceptance/runtime/audio/checks.json", lambda value: value["verified"].pop()),
            ("ci/acceptance/runtime/output-power/result.json", lambda value: value["cases"].pop()),
            ("ci/acceptance/runtime/battery-sleep-backlight/result.json",
             lambda value: value["cases"][0].update(status="failed")),
            ("ci-login/evidence/console/events.json", lambda value: value.update(success=False)),
            ("ci-login/evidence/graphics/events.json", lambda value: value.update(events=[])),
            ("ci-packages/shell-install-hashes.json",
             lambda value: value["files"][BASELINE.SHELL_FILES[0]]["sha256"].update(installed_container="b" * 64)),
        )
        for name, change in cases:
            path = self.artifact / name
            original = path.read_bytes()
            with self.subTest(receipt=name):
                value = BASELINE.read_json(path)
                change(value)
                self.record(name, value)
                try:
                    with self.assertRaises(BASELINE.ValidationError):
                        self.validate()
                finally:
                    path.write_bytes(original)

    def test_package_bytes_index_key_and_recipe_set_are_required(self):
        for name in (next(iter(self.apk_hashes)), "packages/aarch64/APKINDEX.tar.gz", "fixture.rsa.pub"):
            path = self.artifact / "ci-packages" / name
            original = path.read_bytes()
            with self.subTest(missing=name):
                path.unlink()
                try:
                    with self.assertRaises(BASELINE.ValidationError):
                        BASELINE.validate_packages(self.artifact, self.catalog)
                finally:
                    path.write_bytes(original)
        apk = self.artifact / "ci-packages" / next(iter(self.apk_hashes))
        apk.write_bytes(b"Changed APK bytes")
        with self.assertRaises(BASELINE.ValidationError):
            BASELINE.validate_packages(self.artifact, self.catalog)

    def test_source_and_login_hash_logs_cannot_be_replaced(self):
        for name in ("ci/acceptance/native/source/static-audit.log",
                     "ci-login/evidence/login-build/artifact-hashes.log",
                     "ci/acceptance/native/settings-services/bluez.log"):
            path = self.artifact / name
            original = path.read_bytes()
            with self.subTest(log=name):
                path.write_text("Replacement log\n")
                try:
                    with self.assertRaises(BASELINE.ValidationError):
                        self.validate()
                finally:
                    path.write_bytes(original)


if __name__ == "__main__":
    unittest.main()
