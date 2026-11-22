#!/usr/bin/env bash
# Alpine 3.24 source acceptance, x86_64 or aarch64. No package installation,
# VM creation, live PAM/login session, audio mutation or power operation.
#
# Usage:
#   bash scripts/check-alpine-source.sh preflight
#   ALCARD_CARGO_HOME=/path/to/populated/native/cache \
#     bash scripts/check-alpine-source.sh source [new-evidence-directory]
#
# Both modes use the distribution's /usr/bin Rust tools. Source mode is
# --offline/--locked and requires the five committed manifests' dependencies
# to have been fetched already, for example by the existing Alpine builders.
# Its PASS covers source/build checks only; installed APK, Wayland/PAM,
# PipeWire, suspend/resume and Cardputer acceptance remain separate gates.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
exec python3 - "$ROOT" "$@" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1])
args = sys.argv[2:]
mode = args[0] if args else "preflight"
if mode in {"-h", "--help", "help"}:
    print("Usage: check-alpine-source.sh preflight | source [new-evidence-directory]")
    raise SystemExit(0)
if mode not in {"preflight", "source"} or len(args) > (2 if mode == "source" else 1):
    raise SystemExit("Usage: check-alpine-source.sh preflight | source [new-evidence-directory]")

projects = ["rust", "rust/crates/alcard-ui", "rust/apps/alcard-shell",
            "rust/apps/alcard-polkit-agent", "rust/apps/alcard-lock"]
env = dict(os.environ)
for name in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET",
             "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS", "PKG_CONFIG_PATH",
             "PKG_CONFIG_LIBDIR", "PKG_CONFIG_SYSROOT_DIR", "LIBRARY_PATH",
             "LD_LIBRARY_PATH"):
    env.pop(name, None)
env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
env["CARGO_NET_OFFLINE"] = "true"
env["RUSTC"] = "/usr/bin/rustc"
env["RUSTDOC"] = "/usr/bin/rustdoc"
env["RUSTFMT"] = "/usr/bin/rustfmt"
env["ALCARD_CARGO"] = "/usr/bin/cargo"

def capture(command):
    try:
        result = subprocess.run(command, env=env, cwd=root, capture_output=True,
                                text=True, timeout=15, check=False)
        return {"command": command, "exit_code": result.returncode,
                "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": command, "exit_code": None, "error": str(error)}

blockers = []
release_file = Path("/etc/alpine-release")
release = release_file.read_text().strip() if release_file.is_file() else None
if release is None or not release.startswith("3.24."):
    blockers.append("Alpine 3.24 is required; this command does not create or emulate it.")
architecture = capture(["apk", "--print-arch"])
arch = architecture.get("stdout", "")
if architecture["exit_code"] != 0 or arch not in {"x86_64", "aarch64"}:
    blockers.append("A native Alpine x86_64 or aarch64 apk environment is required.")
tools = {name: str(Path("/usr/bin") / name) if os.access(Path("/usr/bin") / name, os.X_OK)
         else None for name in ("cargo", "rustc", "rustdoc", "rustfmt", "cargo-fmt", "cargo-clippy")}
for name, path in tools.items():
    if path is None:
        blockers.append(f"Missing distribution /usr/bin/{name}.")
for name in ("cc", "ar", "pkg-config", "bash", "python3", "setpriv", "flock"):
    if shutil.which(name, path=env["PATH"]) is None:
        blockers.append(f"Missing source-check prerequisite: {name}.")
if not os.access("/bin/ash", os.X_OK):
    blockers.append("Alpine /bin/ash is required for POSIX process fixtures.")
rust = capture(["/usr/bin/rustc", "-vV"])
rust_version = re.search(r"^release: (\d+)\.(\d+)\.(\d+)$", rust.get("stdout", ""), re.M)
if rust_version is None or tuple(map(int, rust_version.groups())) < (1, 96, 1):
    blockers.append("Native stable Rust 1.96.1 or newer is required.")
if not re.search(r"^host: " + re.escape(arch) + r"-(unknown|alpine)-linux-musl$",
                 rust.get("stdout", ""), re.M):
    blockers.append("Rust host must match apk architecture and use musl.")
libraries = capture(["pkg-config", "--modversion", "xkbcommon", "wayland-client",
                     "glib-2.0", "gobject-2.0", "gio-2.0", "polkit-gobject-1", "polkit-agent-1"])
if libraries["exit_code"] != 0:
    blockers.append("Native xkbcommon/Wayland/GLib/GIO/polkit development metadata is required.")
pam = capture(["apk", "info", "-e", "linux-pam-dev"])
if pam["exit_code"] != 0:
    blockers.append("Native linux-pam-dev is required for the locker build.")
cargo_home = Path(env.get("ALCARD_CARGO_HOME", str(root / ".alcard-local/cargo-home" /
                                                ("ci-" + (arch or "unknown") + "-musl")))).resolve()
if not (cargo_home / "registry").is_dir():
    blockers.append("Offline Cargo registry cache is missing; set ALCARD_CARGO_HOME to a populated native build cache.")
if (cargo_home / "bin").exists():
    blockers.append("Cargo cache must not contain a bin directory with host toolchain shims.")
env["CARGO_HOME"] = str(cargo_home)
env["CARGO_TARGET_DIR"] = str(root / ".alcard-local/cargo-target" / ("ci-" + (arch or "unknown") + "-musl"))
unix_socket = {"available": False}
try:
    with tempfile.TemporaryDirectory(prefix="alcard-ci-") as directory:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.bind(str(Path(directory) / "probe"))
    unix_socket["available"] = True
except OSError as error:
    unix_socket["error"] = str(error)
    blockers.append("AF_UNIX bind is unavailable; the normal unit suite cannot be accepted here.")
for project in projects:
    for name in ("Cargo.toml", "Cargo.lock"):
        if not (root / project / name).is_file():
            blockers.append(f"Missing {project}/{name}.")
report = {"schema": 1, "scope": "Alpine source gate, not installed or hardware acceptance",
          "ready": not blockers, "alpine_release": release, "architecture": architecture,
          "kernel_architecture": platform.machine(), "uid": os.getuid(), "rust": rust,
          "native_tools": tools, "development_libraries": libraries, "pam": pam,
          "cargo_home": str(cargo_home), "unix_socket": unix_socket,
          "runtime_tools_observed_only": {name: shutil.which(name, path=env["PATH"])
              for name in ("labwc", "loginctl", "pipewire", "wireplumber", "wpctl", "pw-dump")},
          "blockers": blockers}
if mode == "preflight":
    print(json.dumps(report, indent=2))
    raise SystemExit(0 if report["ready"] else 2)

if len(args) == 2:
    evidence = Path(args[1]).resolve()
    evidence.mkdir(parents=True, exist_ok=False)
else:
    (root / ".alcard-local").mkdir(exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="ci-source-" + (arch or "unknown") + "-",
                                     dir=root / ".alcard-local"))
(evidence / "preflight.json").write_text(json.dumps(report, indent=2) + "\n")
if not report["ready"]:
    print(json.dumps(report, indent=2))
    print(f"BLOCKED: source checks were not run. Evidence: {evidence}", file=sys.stderr)
    raise SystemExit(2)

def source_hashes():
    files = [root / "build.sh"]
    for relative in ("rust", "scripts", "tools", "packaging", "keyboard-zero", "fonts/alcard-pixel/source"):
        for directory, subdirs, names in os.walk(root / relative):
            subdirs[:] = [x for x in subdirs if x not in {"target", ".git", ".alcard-local", "__pycache__"}]
            files.extend(Path(directory) / name for name in names if not name.endswith(".pyc"))
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted(files) if path.is_file()}

before = source_hashes()
checks = []
status = 0
def run(name, command):
    log_path = evidence / (name + ".log")
    print(f"Checking {name}", flush=True)
    started = time.monotonic()
    with log_path.open("w") as log:
        result = subprocess.run(command, env=env, cwd=root, stdout=log,
                                stderr=subprocess.STDOUT, check=False)
    checks.append({"name": name, "command": command, "exit_code": result.returncode,
                   "seconds": round(time.monotonic() - started, 3), "log": log_path.name,
                   "log_sha256": hashlib.sha256(log_path.read_bytes()).hexdigest()})
    if result.returncode:
        print(log_path.read_text(errors="replace")[-10000:], file=sys.stderr)
        raise subprocess.CalledProcessError(result.returncode, command)

try:
    run("static-audit", ["bash", "scripts/audit-build.sh"])
    for index, project in enumerate(projects, 1):
        manifest = str(root / project / "Cargo.toml")
        native_target = re.search(r"^host: (.+)$", rust["stdout"], re.M).group(1)
        prefix = str(index) + "-" + Path(project).name
        run(prefix + "-fmt", ["/usr/bin/cargo", "fmt", "--manifest-path", manifest, "--all", "--check"])
        for operation in ("test", "clippy", "build"):
            command = ["/usr/bin/cargo", operation, "--offline", "--locked", "--target", native_target,
                       "--manifest-path", manifest]
            if operation != "build":
                command += ["--all-targets"]
            if operation == "clippy":
                command += ["--", "-D", "warnings"]
            run(prefix + "-" + operation, command)
    run("architecture-audit", ["python3", "tools/audit-shell.py"])
    for fixture in ("test-session-client", "test-audio-session", "test-tty1-dispatch"):
        run(fixture, ["python3", "tools/" + fixture + ".py", "--shell", "/bin/ash"])
except (OSError, subprocess.CalledProcessError) as error:
    status = 1
    print(f"FAILED: {error}", file=sys.stderr)
except KeyboardInterrupt:
    status = 130
    print("INTERRUPTED: source acceptance is incomplete.", file=sys.stderr)
finally:
    after = source_hashes()
    if after != before:
        status = 1
        print("FAILED: source inputs changed during the check.", file=sys.stderr)
    receipt = {"schema": 1, "scope": report["scope"], "exit_code": status,
               "checks": checks, "inputs_before": before, "inputs_after": after,
               "inputs_unchanged": before == after,
               "not_performed": ["APK build/install", "real Wayland/PAM login", "real PipeWire audio",
                                 "kernel suspend/resume", "physical battery", "Cardputer", "RAM/CPU benchmark"]}
    (evidence / "result.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(f"{'PASS' if status == 0 else 'FAIL'}: Alpine source checks only. Evidence: {evidence}")
raise SystemExit(status)
PY
