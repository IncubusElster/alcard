#!/usr/bin/env python3
"""Verify a full acceptance artifact before reusing its APKs for Hello only.

prepare resolves provenance through GitHub's API. verify-local performs the
same archive/source/receipt checks with already obtained metadata and ZIP; it
does not independently authenticate that local metadata with GitHub.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile


MAX_MEMBERS = 4096
MAX_MEMBER_BYTES = 128 * 1024 * 1024
MAX_TOTAL_BYTES = 512 * 1024 * 1024
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
SHA256 = re.compile(r"[0-9a-f]{64}")
SHA1 = re.compile(r"[0-9a-f]{40}")
WORKFLOW = ".github/workflows/ci.yml"
STAGES = (
    "01-native-source-apk-runtime", "02-stage-same-signed-apks",
    "03-build-login-guest", "04-sequential-login-modes",
    "05-preserve-login-build", "06-build-hello-guest",
    "07-interactive-hello-settings",
)
SOURCE_CHECKS = (
    "static-audit",
    *(f"{index}-{workspace}-{check}"
      for index, workspace in enumerate(
          ("rust", "alcard-ui", "alcard-shell", "alcard-polkit-agent", "alcard-lock"), 1)
      for check in ("fmt", "test", "clippy", "build")),
    "architecture-audit", "test-session-client", "test-audio-session", "test-tty1-dispatch",
)
SERVICES = {
    "networkmanager": "network::activation::integration::isolated_network_activation_lifecycle",
    "bluez": "pairing::integration::isolated_bluez_pairing_lifecycle",
}
RUNTIME_CASES = ("notifications", "idle", "audio", "output-power", "battery-sleep-backlight")
POWER_CASES = ("failed", "reacquire", "hotplug", "retired-control")
BATTERY_CASES = (
    "default-warnings", "automatic-resume-restart", "stale-measurement",
    "charging-cancel-rearm", "unknown-cancel-no-retry", "failed-attempt-no-retry",
    "blocking-inhibitor-refusal", "authorization-challenge-refusal",
    "manual-suspend", "external-suspend", "brightness-controls",
)
LOGIN_MODES = ("graphics", "graphics_minimal", "console")
SHELL_FILES = (
    "/usr/bin/alcard-shell", "/usr/libexec/alcard/alcard-radio-helper",
    "/usr/share/polkit-1/actions/org.alcard.radio.policy",
    "/etc/dbus-1/system-services/org.bluez.service", "/etc/alcard/bluetooth.conf",
)
ALLOWED_FILES = frozenset((
    WORKFLOW, "scripts/ci/run.sh", "scripts/ci/select-scope.py", "scripts/ci/test-select.py",
    "scripts/ci/hello-baseline.py", "scripts/ci/test-hello-baseline.py",
))


class ValidationError(ValueError):
    """The baseline does not prove the prerequisites for a Hello continuation."""


def require(condition, message):
    if not condition:
        raise ValidationError(message)


def integer(value, name, minimum=1):
    require(type(value) is int and value >= minimum, f"Invalid {name}")
    return value


def no_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def parse_json(data):
    return json.loads(data, object_pairs_hook=no_duplicate_keys)


def read_json(path):
    return parse_json(Path(path).read_text(encoding="utf-8"))


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sha256_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def valid_repository(repository):
    require(isinstance(repository, str) and re.fullmatch(
        r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository), "Invalid GITHUB_REPOSITORY")


def validate_run_api(run, repository, run_id):
    valid_repository(repository)
    require(re.fullmatch(r"[1-9][0-9]*", str(run_id)), "baseline_run must be a positive numeric run ID")
    require(run.get("id") == int(run_id), "GitHub returned a different run")
    for key in ("repository", "head_repository"):
        require((run.get(key) or {}).get("full_name") == repository, f"Wrong run {key}")
    require(run.get("event") == "workflow_dispatch", "Baseline must be a manual full acceptance run")
    require(run.get("head_branch") == "main", "Baseline must belong to main")
    require(run.get("path") == WORKFLOW, "Baseline uses a different workflow")
    require(run.get("status") == "completed", "Baseline run is not completed")
    require(run.get("conclusion") in ("failure", "success"), "Baseline has no usable final conclusion")
    require(isinstance(run.get("head_sha"), str) and SHA1.fullmatch(run["head_sha"]),
            "Invalid baseline source SHA")
    return {
        "schema": 1, "repository": repository, "run_id": int(run_id),
        "run_attempt": integer(run.get("run_attempt"), "run attempt"),
        "source_sha": run["head_sha"], "conclusion": run["conclusion"],
        "event": run["event"], "head_branch": run["head_branch"],
        "workflow_path": run["path"], "status": run["status"],
    }


def select_artifact(metadata, payload):
    artifacts = payload.get("artifacts")
    require(isinstance(artifacts, list) and payload.get("total_count") == len(artifacts),
            "Incomplete artifact listing; refusing ambiguous provenance")
    expected = f"alcard-CI-ARM64-{metadata['run_id']}-{metadata['run_attempt']}"
    matches = [item for item in artifacts if item.get("name") == expected]
    require(len(matches) == 1, "Expected exactly one artifact for the baseline run attempt")
    artifact = matches[0]
    require(artifact.get("expired") is False, "Baseline artifact has expired")
    provenance = artifact.get("workflow_run") or {}
    require(provenance.get("id") == metadata["run_id"] and
            provenance.get("head_sha") == metadata["source_sha"] and
            provenance.get("head_branch") == "main", "Artifact belongs to a different run/source")
    result = dict(metadata, artifact={
        "id": artifact.get("id"), "name": artifact["name"],
        "digest": artifact.get("digest"), "size": artifact.get("size_in_bytes"),
    })
    validate_metadata(result, metadata["repository"])
    return result


def validate_metadata(metadata, repository):
    valid_repository(repository)
    require(metadata.get("schema") == 1 and metadata.get("repository") == repository,
            "Metadata repository/schema mismatch")
    for key in ("run_id", "run_attempt"):
        integer(metadata.get(key), key)
    require(isinstance(metadata.get("source_sha"), str) and SHA1.fullmatch(metadata["source_sha"]),
            "Invalid metadata source SHA")
    require(metadata.get("event") == "workflow_dispatch" and metadata.get("head_branch") == "main"
            and metadata.get("workflow_path") == WORKFLOW and metadata.get("status") == "completed",
            "Metadata is not a completed manual main CI run")
    require(metadata.get("conclusion") in ("failure", "success"), "Invalid metadata conclusion")
    artifact = metadata.get("artifact") or {}
    integer(artifact.get("id"), "artifact ID")
    size = integer(artifact.get("size"), "artifact size")
    require(size <= MAX_ARCHIVE_BYTES, "Artifact exceeds download size limit")
    require(artifact.get("name") ==
            f"alcard-CI-ARM64-{metadata['run_id']}-{metadata['run_attempt']}", "Artifact name mismatch")
    require(isinstance(artifact.get("digest"), str) and
            re.fullmatch(r"sha256:[0-9a-f]{64}", artifact["digest"]), "Missing SHA256 artifact digest")


def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, timeout=30)


def allowed_change(path):
    parts = PurePosixPath(path).parts
    if not parts or any(part in (".", "..") for part in path.split("/")) or "\\" in path:
        return False
    return (path in ALLOWED_FILES or path.startswith(("scripts/ci/hello/", "scripts/ci/settings/"))
            or (len(parts) >= 2 and parts[0] == "docs" and path.endswith(".md")))


def git_fingerprint(repo, revision, check_working_tree=False):
    result = {}
    for entry in git(repo, "ls-tree", "-r", "-z", revision).split(b"\0"):
        if not entry:
            continue
        identity, raw_path = entry.split(b"\t", 1)
        mode, kind, blob = identity.decode().split()
        path = raw_path.decode("utf-8")
        require(mode in ("100644", "100755") and kind == "blob",
                f"Unsupported tracked file type: {path}")
        result[path] = hashlib.sha256(git(repo, "cat-file", "blob", blob)).hexdigest()
        if check_working_tree:
            current = repo / path
            current_mode = current.lstat().st_mode
            require(stat.S_ISREG(current_mode) and sha256_file(current) == result[path]
                    and bool(current_mode & 0o111) == (mode == "100755"),
                    f"Current tracked file differs from Git: {path}")
    require(result, "Empty baseline source tree")
    return result


def validate_source(repo, metadata, before, after):
    source = metadata["source_sha"]
    require(before == after and before.get("head") == source and before.get("tracked_status") == "",
            "Baseline input fingerprints changed, were dirty, or identify another source")
    require(before.get("files") == git_fingerprint(repo, source),
            "Baseline fingerprint is incomplete or differs from its Git tree")
    require(git(repo, "status", "--porcelain", "--untracked-files=all") == b"",
            "Current checkout must be clean, including non-ignored untracked files")
    ancestor = subprocess.run(["git", "merge-base", "--is-ancestor", source, "HEAD"],
                              cwd=repo, timeout=30, check=False)
    require(ancestor.returncode == 0, "Baseline source is not an ancestor of current HEAD")
    current = git(repo, "rev-parse", "HEAD").decode().strip()
    changed = [name.decode("utf-8") for name in
               git(repo, "diff", "--no-renames", "--name-only", "-z", source, current).split(b"\0") if name]
    forbidden = [name for name in changed if not allowed_change(name)]
    require(not forbidden, "Changes invalidate package/runtime reuse: " + ", ".join(forbidden))
    # Also reject a new symlink/gitlink at an otherwise permitted harness path.
    git_fingerprint(repo, current, check_working_tree=True)
    return {
        "current_source_sha": current,
        "current_source_tree": git(repo, "rev-parse", f"{current}^{{tree}}").decode().strip(),
        "baseline_source_tree": git(repo, "rev-parse", f"{source}^{{tree}}").decode().strip(),
        "allowed_changed_paths": changed,
    }


def safe_extract(archive, destination, expected_digest, expected_size):
    archive, destination = Path(archive), Path(destination)
    require(not destination.exists() and not destination.is_symlink(), "Archive destination already exists")
    require(isinstance(expected_digest, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", expected_digest),
            "Invalid expected archive digest")
    require(type(expected_size) is int and 0 < expected_size <= MAX_ARCHIVE_BYTES
            and archive.stat().st_size == expected_size, "Archive size mismatch or limit exceeded")
    require("sha256:" + sha256_file(archive) == expected_digest, "Archive SHA256 digest mismatch")
    with zipfile.ZipFile(archive) as source:
        members = source.infolist()
        require(0 < len(members) <= MAX_MEMBERS, "Archive member count exceeds limit")
        names, files, total = set(), set(), 0
        for member in members:
            name = member.filename
            clean = name[:-1] if name.endswith("/") else name
            parts = clean.split("/")
            require(clean and not name.startswith("/") and "\\" not in name
                    and all(part not in ("", ".", "..") for part in parts)
                    and not any(ord(char) < 32 for char in name)
                    and ":" not in parts[0], f"Unsafe archive path: {name!r}")
            require(clean not in names, f"Duplicate archive path: {clean}")
            mode = (member.external_attr >> 16) & 0xFFFF
            kind = stat.S_IFMT(mode)
            require(kind in (0, stat.S_IFREG, stat.S_IFDIR) and not (member.flag_bits & 1),
                    f"Unsupported archive entry type/encryption: {clean}")
            require((kind != stat.S_IFDIR or member.is_dir())
                    and (kind != stat.S_IFREG or not member.is_dir()), "Inconsistent archive entry type")
            require(0 <= member.file_size <= MAX_MEMBER_BYTES, "Archive member exceeds size limit")
            require(not member.is_dir() or member.file_size == 0, "Nonempty archive directory")
            total += member.file_size
            require(total <= MAX_TOTAL_BYTES, "Archive expanded size exceeds limit")
            names.add(clean)
            if not member.is_dir():
                files.add(clean)
        require(all(not any(parent.as_posix() in files for parent in PurePosixPath(name).parents)
                    for name in names), "Archive file/directory collision")
        destination.mkdir(parents=True)
        for member in members:
            target = destination / member.filename
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.open(member) as incoming, target.open("xb") as outgoing:
                copied = 0
                while block := incoming.read(1024 * 1024):
                    copied += len(block)
                    require(copied <= member.file_size, "Expanded archive entry exceeds declared size")
                    outgoing.write(block)
            require(copied == member.file_size, "Truncated archive entry")
            target.chmod(0o644)


def exact_names(items, names, description):
    require(isinstance(items, list) and [item.get("name") for item in items] == list(names),
            f"Missing, reordered or duplicate {description}")


def passed_test_log(path, test):
    log = path.read_text(encoding="utf-8")
    require(f"test {test} ..." in log and
            "test result: ok. 1 passed; 0 failed; 0 ignored;" in log,
            f"Missing successful native test receipt: {test}")
    return log


def validate_packages(root, expected_manifest):
    for location in ("ci/static/package-recipes.json", "ci/acceptance/native/package-recipes.json"):
        require(read_json(root / location) == expected_manifest, "Package recipe manifest mismatch")
    names = expected_manifest["groups"]["user"]
    require(len(names) == len(set(names)) == 9, "Expected exactly nine user package recipes")
    filenames = {expected_manifest["packages"][name]["filename"] for name in names}
    require(len(filenames) == 9 and all(re.fullmatch(r"[A-Za-z0-9+_.~-]+\.apk", name)
                                      for name in filenames), "Invalid APK filenames")
    packages = root / "ci-packages"
    require((packages / "status.txt").read_text().strip() == "exit_status=0", "APK gate did not pass")
    archive_dir = packages / "packages/aarch64"
    require({item.name for item in archive_dir.iterdir()} == filenames | {"APKINDEX.tar.gz"},
            "APK directory does not contain exactly the same nine packages and index")
    hashes = {}
    for line in (packages / "SHA256SUMS").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  (packages/aarch64/[A-Za-z0-9+_.~-]+\.apk)", line)
        require(match is not None and match[2] not in hashes, "Malformed or duplicate APK checksum")
        hashes[match[2]] = match[1]
    require(set(hashes) == {f"packages/aarch64/{name}" for name in filenames}, "APK checksum set mismatch")
    for path, digest in hashes.items():
        require(sha256_file(packages / path) == digest, f"APK bytes differ: {path}")
    keys = sorted(packages.glob("*.rsa.pub"))
    require(len(keys) == 1 and re.fullmatch(r"[A-Za-z0-9_.-]+\.rsa\.pub", keys[0].name),
            "Expected one public APK signing key")
    require(keys[0].read_text().startswith("-----BEGIN PUBLIC KEY-----\n"), "Invalid public signing key")
    index = archive_dir / "APKINDEX.tar.gz"
    require(index.stat().st_size > 0, "Missing package index")
    expected_ok = sorted(f"{path}: OK" for path in hashes)
    for build in ("login-build", "hello-build"):
        receipt = root / f"ci-login/evidence/{build}/artifact-hashes.log"
        require(sorted(receipt.read_text().splitlines()) == expected_ok,
                f"{build} did not verify the same nine APKs")
    installed = read_json(packages / "shell-install-hashes.json")
    require(installed.get("status") == "PASS" and set(installed.get("files", {})) == set(SHELL_FILES),
            "Shell installation hashes are missing or incomplete")
    shell_hashes = {}
    for path, item in installed["files"].items():
        values = item.get("sha256", {})
        require(set(values) == {"built", "staged", "clean_rootfs", "installed_container"}
                and len(set(values.values())) == 1
                and isinstance(values["built"], str) and SHA256.fullmatch(values["built"]),
                f"Shell installation hash mismatch: {path}")
        shell_hashes[path] = values["built"]
    return {
        "apk_sha256": dict(sorted(hashes.items())), "shell_install_sha256": shell_hashes,
        "public_keys_sha256": {key.name: sha256_file(key) for key in keys},
        "apk_index_sha256": sha256_file(index),
    }


def current_catalog(repo):
    return parse_json(subprocess.check_output(
        [sys.executable, str(repo / "tools/package_metadata.py"), "--root", str(repo), "manifest"],
        cwd=repo, timeout=20))


def validate_receipts(root, metadata, repo):
    root, repo = Path(root), Path(repo)
    acceptance = root / "ci/acceptance"
    before = read_json(acceptance / "inputs-before.json")
    after = read_json(acceptance / "inputs-after.json")
    current = validate_source(repo, metadata, before, after)
    selection = read_json(root / "ci/selection.json")
    require(selection.get("scope") == "acceptance" and selection.get("event") == "workflow_dispatch"
            and selection.get("source_sha") == metadata["source_sha"]
            and selection.get("source_tree") == current["baseline_source_tree"],
            "Baseline was not full acceptance for the recorded source tree")
    original = read_json(acceptance / "result.json")
    stages = original.get("stages")
    exact_names(stages, STAGES, "acceptance stages")
    expected_final = 0 if metadata["conclusion"] == "success" else 1
    require(original.get("exit_code") == expected_final and original.get("inputs_unchanged") is True
            and original.get("last_stage") == ("complete" if expected_final == 0 else STAGES[-1]),
            "Original acceptance result/conclusion mismatch")
    for index, stage in enumerate(stages):
        require(type(stage.get("exit_code")) is int and
                stage["exit_code"] == (expected_final if index == 6 else 0),
                "Baseline failed before Hello, or Hello did not exit with the expected result")
        require(read_json(acceptance / f"stages/{stage['name']}.json") == stage,
                f"Inconsistent stage receipt: {stage['name']}")
    source_dir = acceptance / "native/source"
    source = read_json(source_dir / "result.json")
    require(source.get("exit_code") == 0 and source.get("inputs_unchanged") is True
            and source.get("inputs_before") == source.get("inputs_after")
            and isinstance(source.get("inputs_before"), dict) and source["inputs_before"]
            and all(before["files"].get(path) == value for path, value in source["inputs_before"].items()),
            "Original native source gate did not pass with unchanged inputs")
    exact_names(source.get("checks"), SOURCE_CHECKS, "source checks")
    for check in source["checks"]:
        require(check.get("exit_code") == 0 and check.get("log") == check["name"] + ".log"
                and sha256_file(source_dir / check["log"]) == check.get("log_sha256"),
                f"Missing or changed source check receipt: {check['name']}")
    service_dir = acceptance / "native/settings-services"
    services = read_json(service_dir / "result.json")
    require(services.get("exit_code") == 0, "Native Settings service gate failed")
    exact_names(services.get("cases"), SERVICES, "Settings service cases")
    for case in services["cases"]:
        name = case["name"]
        require(case.get("exit_code") == 0 and case.get("test") == SERVICES[name],
                f"Missing successful Settings service case: {name}")
        passed_test_log(service_dir / f"{name}.log", SERVICES[name])
    packages = validate_packages(root, current_catalog(repo))
    runtime_dir = acceptance / "runtime"
    runtime = read_json(runtime_dir / "result.json")
    require(runtime.get("status") == "PASS" and runtime.get("exit_code") == 0,
            "Installed runtime gate failed")
    exact_names(runtime.get("stages"), RUNTIME_CASES, "runtime cases")
    require(all(case.get("exit_code") == 0 for case in runtime["stages"]), "A runtime case failed")
    passed_test_log(runtime_dir / "notifications.log", "notifications::tests::isolated_dbus_notification_lifecycle")
    passed_test_log(runtime_dir / "idle/test.log", "integration::real_idle_lock_and_output_power")
    audio = read_json(runtime_dir / "audio/checks.json")
    require(audio.get("status") == "PASS" and isinstance(audio.get("verified"), list)
            and len(audio["verified"]) == len(set(audio["verified"])) == 7, "Audio receipts are incomplete")
    for directory, names, status in (("output-power", POWER_CASES, "PASS"),
                                     ("battery-sleep-backlight", BATTERY_CASES, "passed")):
        result = read_json(runtime_dir / directory / "result.json")
        exact_names(result.get("cases"), names, f"{directory} cases")
        require(result.get("status") == status and all(case.get("status") == status for case in result["cases"])
                and (result.get("shell") or {}).get("sha256") == packages["shell_install_sha256"][SHELL_FILES[0]],
                f"{directory} status or installed Shell identity mismatch")
    for mode in LOGIN_MODES:
        events = read_json(root / f"ci-login/evidence/{mode}/events.json")
        require(events.get("success") is True and isinstance(events.get("events"), list) and events["events"],
                f"Missing successful real login receipt: {mode}")
    return {
        "schema": 1, "status": "PASS", "scope": "Verified prerequisites reused for Hello continuation",
        "baseline": metadata, **current, **packages,
        "original_result": original,
        "reused_successful_scopes": {
            "source": {"status": "reused_not_rerun", "checks": list(SOURCE_CHECKS)},
            "native_services": {"status": "reused_not_rerun", "cases": list(SERVICES)},
            "signed_apks": {"status": "reused_not_rerun", "count": 9},
            "installed_runtime": {"status": "reused_not_rerun", "cases": list(RUNTIME_CASES)},
            "login": {"status": "reused_not_rerun", "modes": list(LOGIN_MODES)},
        },
        "not_performed": ["new source/native/APK/runtime/login execution", "new Hello/Settings execution",
                          "physical hardware acceptance"],
    }


def output_path(path, repo):
    path, local = Path(path).absolute(), repo / ".alcard-local"
    require(".." not in path.parts, "Parent traversal is not allowed in output paths")
    require(path.is_relative_to(local) and path != local, "Output must be inside .alcard-local")
    for component in (path, *path.parents):
        require(not component.is_symlink(), f"Refusing symlink in output path: {component}")
        if component == repo:
            break
    require(not path.exists(), f"Refusing existing output: {path}")
    return path


def prepare_local(metadata, archive, baseline_dir, input_dir, repo):
    repo = Path(repo).resolve()
    validate_metadata(metadata, os.environ.get("GITHUB_REPOSITORY", metadata["repository"]))
    baseline_dir, input_dir = (output_path(path, repo) for path in (baseline_dir, input_dir))
    require(not baseline_dir.is_relative_to(input_dir) and not input_dir.is_relative_to(baseline_dir),
            "Baseline and login input paths must be separate")
    baseline_dir.parent.mkdir(parents=True, exist_ok=True)
    input_dir.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hello-baseline-", dir=baseline_dir.parent) as staging:
        staged = Path(staging) / "baseline"
        staged.mkdir()
        artifact = staged / "artifact"
        safe_extract(archive, artifact, metadata["artifact"]["digest"], metadata["artifact"]["size"])
        summary = validate_receipts(artifact, metadata, repo)
        write_json(staged / "metadata.json", metadata)
        write_json(staged / "verification.json", summary)
        prepared = Path(staging) / "login-input"
        package_root = artifact / "ci-packages"
        shutil.copytree(package_root / "packages", prepared / "packages")
        for name in ("SHA256SUMS", "status.txt", *summary["public_keys_sha256"]):
            shutil.copyfile(package_root / name, prepared / name)
        # A failed validation never publishes a usable input directory.
        require(not baseline_dir.exists() and not input_dir.exists(), "Output appeared during verification")
        staged.rename(baseline_dir)
        prepared.rename(input_dir)
    return summary


def gh_api(endpoint, output=None):
    require(bool(os.environ.get("GH_TOKEN")), "GH_TOKEN is required to read baseline provenance")
    command = ["gh", "api", "--hostname", "github.com", "--method", "GET",
               "-H", "X-GitHub-Api-Version: 2022-11-28", endpoint]
    if output is None:
        return parse_json(subprocess.check_output(command, timeout=30))
    with Path(output).open("xb") as target:
        subprocess.run(command, stdout=target, check=True, timeout=75)


def resolve(run_id, repository, repo=None):
    valid_repository(repository)
    require(re.fullmatch(r"[1-9][0-9]*", run_id), "baseline_run must be a positive numeric run ID")
    run = gh_api(f"repos/{repository}/actions/runs/{run_id}")
    metadata = validate_run_api(run, repository, run_id)
    ancestor = subprocess.run(["git", "merge-base", "--is-ancestor", metadata["source_sha"], "HEAD"],
                              cwd=repo, timeout=30, check=False)
    require(ancestor.returncode == 0, "Baseline source is not an ancestor of current HEAD")
    artifacts = gh_api(f"repos/{repository}/actions/runs/{run_id}/artifacts?per_page=100")
    return select_artifact(metadata, artifacts)


def github_output(**values):
    target = os.environ.get("GITHUB_OUTPUT")
    if target:
        with open(target, "a", encoding="utf-8") as output:
            for key, value in values.items():
                output.write(f"{key}={value}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    resolving = commands.add_parser("resolve", help="Resolve trusted run and artifact metadata through GitHub")
    resolving.add_argument("run_id")
    resolving.add_argument("metadata", type=Path)
    preparing = commands.add_parser("prepare", help="Resolve, download, validate and stage the same APKs")
    preparing.add_argument("run_id")
    local = commands.add_parser("verify-local", help="Validate an existing metadata/ZIP pair without network")
    local.add_argument("metadata", type=Path)
    local.add_argument("archive", type=Path)
    for command in (preparing, local):
        command.add_argument("baseline_dir", type=Path)
        command.add_argument("input_dir", type=Path)
    args = parser.parse_args()
    try:
        repo = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip()).resolve()
        if args.command == "verify-local":
            metadata = read_json(args.metadata)
        else:
            metadata = resolve(args.run_id, os.environ.get("GITHUB_REPOSITORY", ""), repo)
        if args.command == "resolve":
            target = output_path(args.metadata, repo)
            target.parent.mkdir(parents=True, exist_ok=True)
            write_json(target, metadata)
            github_output(artifact_id=metadata["artifact"]["id"])
            print(f"Resolved baseline run {metadata['run_id']} attempt {metadata['run_attempt']}")
            return
        if args.command == "prepare":
            parent = output_path(args.baseline_dir, repo).parent
            parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix=".hello-download-", dir=parent) as temporary:
                archive = Path(temporary) / "artifact.zip"
                gh_api(f"repos/{metadata['repository']}/actions/artifacts/{metadata['artifact']['id']}/zip", archive)
                summary = prepare_local(metadata, archive, args.baseline_dir, args.input_dir, repo)
        else:
            summary = prepare_local(metadata, args.archive, args.baseline_dir, args.input_dir, repo)
        github_output(artifact_id=metadata["artifact"]["id"], baseline_source_sha=metadata["source_sha"])
        print(f"PASS: baseline {metadata['run_id']} preserves original exit "
              f"{summary['original_result']['exit_code']}; verified nine APKs for Hello only")
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
        parser.exit(1, f"ERROR: {error}\n")


if __name__ == "__main__":
    main()
