#!/usr/bin/env bash
# Static source preflight; never invokes sudo, abuild, cargo or network.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"
fail() { echo "ERROR: $*" >&2; exit 1; }
echo "==> Checking Bash build scripts"
shopt -s globstar nullglob
for script in build.sh scripts/**/*.sh; do
  bash -n "$script" || fail "Bash syntax: $script"
done
echo "==> Checking POSIX firstboot and Wayland scripts"
for script in scripts/hello/tty1-dispatch scripts/hello/firstboot-init \
  scripts/hello/start-session scripts/wayland/autostart \
  scripts/wayland/wayland-session scripts/wayland/shell-session \
  scripts/wayland/session-client scripts/wayland/audio-session scripts/test-session-apk-native.sh; do
  sh -n "$script" || fail "POSIX syntax: $script"
done
echo "==> Checking APKBUILD syntax"
for recipe in packaging/apk/*/APKBUILD; do
  sh -n "$recipe" || fail "APKBUILD syntax: $recipe"
done
echo "==> Checking Python source and Alpine package contract"
python3 - "$ROOT" <<'PY'
import ast, json, pathlib, re, sys
import xml.etree.ElementTree as ET
root = pathlib.Path(sys.argv[1])
sys.path.insert(0, str(root / "tools"))
from package_metadata import Catalog, GROUPS
catalog = Catalog(root)
for path in (list(root.glob("tools/**/*.py")) + list(root.glob("scripts/**/*.py"))
             + list(root.glob("keyboard-zero/*.py"))):
    ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
main = (root/"build.sh").read_text()
for name in ("keyboard", "keyboard-check", "hello", "fonts", "fonts-gui", "desktop", "qemu-hello",
             "shell-apk-check", "shell-apks", "shell-install-verify",
             "image-v5", "image-v5-lite", "image-verify-v5", "image-verify-lite"):
    if f"{name})" not in main:
        raise SystemExit(f"Missing build command: {name}")
required = [
    "tools/build-alcard-fonts.py", "tools/ensure-greetd-user.py",
    "tools/package_metadata.py", "scripts/package-metadata.sh",
    "scripts/hello/tty1-dispatch", "scripts/hello/firstboot-init",
    "scripts/hello/start-session", "scripts/hello/alcard-greetd.toml",
    "scripts/wayland/foot.ini", "scripts/wayland/wayland-session",
    "scripts/wayland/rc.xml",
    "scripts/wayland/shell-session", "scripts/wayland/session-client", "scripts/wayland/audio-session",
    "scripts/build-shell-apk.sh", "scripts/verify-session-install.sh",
    "scripts/test-session-apk-native.sh", "tools/stage-shell-apk.py",
    "tools/verify-session-install.py",
    "tools/test-session-client.py", "tools/test-audio-session.py",
    "packaging/apk/alcard-shell/APKBUILD", "packaging/apk/alcard-polkit-agent/APKBUILD",
    "packaging/apk/alcard-lock/APKBUILD", "packaging/apk/alcard-desktop-shell/APKBUILD",
    "rust/crates/alcard-setup-protocol/src/session_runtime.rs",
    "scripts/build-keyboard-zero.sh", "keyboard-zero/verify-keyboard.py",
    "keyboard-zero/layouts.json", "keyboard-zero/symbols/alcard",
    "scripts/build-hello.sh", "scripts/build-wayland-apk.sh",
    "scripts/build-fonts-console.sh", "scripts/build-image-zero.sh",
    "fonts/alcard-pixel/source/AlcardPixel-Regular.hex",
    "fonts/alcard-pixel/source/AlcardPixel-Bold.hex",
]
for filename in required:
    if not (root/filename).is_file():
        raise SystemExit(f"Missing source: {filename}")
# Installed primary profile belongs to the separate alcard-desktop-shell APK.
# Hide the pointer before Home even when Shell is already mapped.
home_xml = ET.parse(root/"scripts/wayland/rc.xml").getroot()
home_actions = home_xml.findall("./keyboard/keybind[@key='Super_L'][@onRelease='yes']/action")
if [action.attrib for action in home_actions] != [
        {"name": "HideCursor"},
        {"name": "Execute", "command": "alcard-shell --home"}]:
    raise SystemExit("labwc profile must hide the cursor before lone Super release invokes Home")
rule = home_xml.find("./windowRules/windowRule[@identifier='alcard-shell']")
if rule is None or rule.get("serverDecoration") != "no" or rule.get("event") != "onFirstMap":
    raise SystemExit("Installed Shell must have an undecorated first-map rule")
if [action.get("name") for action in rule.findall("action")] != ["ToggleFullscreen", "HideCursor"]:
    raise SystemExit("Installed Shell must enter fullscreen and hide cursor on first map")
# Both images must install the same explicit session roots, including the
# graphical profiles. The package reader validates current exact recipe pins.
if set(GROUPS["session-roots"]) != {
        "alcard-hello", "alcard-keyboard-zero", "alcard-fonts-console",
        "alcard-desktop-minimal", "alcard-desktop-shell"}:
    raise SystemExit("Full-image package group changed")
for name in ("build-image-zero.sh", "build-qemu-hello-aarch64.sh"):
    text = (root/"scripts"/name).read_text()
    for contract in ('alcard_package_array SESSION_PINS pin --group session-roots',
                     '"${SESSION_PINS[@]}"', 'greetd-agreety'):
        if contract not in text:
            raise SystemExit(f"Missing full-image metadata contract {contract}: {name}")
    if "ALCARD_WITH_WAYLAND" in text:
        raise SystemExit(f"Obsolete image toggle: {name}")
hello = (root/"packaging/apk/alcard-hello/APKBUILD").read_text()
match = re.search(r'^depends="([^"]+)"', hello, re.M)
if not match:
    raise SystemExit("Hello APK runtime dependencies not declared")
if set(match.group(1).split()) & {"greetd", "greetd-agreety", "labwc", "foot"}:
    raise SystemExit("Hello cannot depend on removable login/Wayland packages")
# Keyboard package and the Hello protocol agree on the four supported layouts.
keyboards = json.loads((root/"keyboard-zero/layouts.json").read_text())
if set(keyboards["layouts"]) != {"us", "ru", "es", "de"}:
    raise SystemExit("Keyboard layout catalog mismatch")
if catalog["alcard-keyboard-zero"].pin not in catalog["alcard-hello"].dependencies:
    raise SystemExit("Hello must depend on Cardputer keyboard APK")
# Shell owns its control clients; Graphics owns the user audio stack. Minimal
# and Console must not acquire this stack solely for Shell controls.
shell_recipe = (root/"packaging/apk/alcard-shell/APKBUILD").read_text()
shell_dependencies = re.search(r'^depends="([^"]+)"', shell_recipe, re.M)
if not shell_dependencies or not {"wireplumber", "pipewire-tools"} <= set(shell_dependencies.group(1).split()):
    raise SystemExit("Shell must declare wireplumber and pipewire-tools for wpctl/pw-dump")
if "alsa-utils" in shell_dependencies.group(1).split():
    raise SystemExit("Shell must not retain its replaced direct ALSA helper dependency")
audio_packages = {"pipewire", "wireplumber", "pipewire-pulse", "pipewire-alsa", "flock"}
session_recipe = (root/"packaging/apk/alcard-desktop-shell/APKBUILD").read_text()
session_dependencies = re.search(r'^depends="([^"]+)"', session_recipe, re.M)
if not session_dependencies or not audio_packages <= set(session_dependencies.group(1).split()):
    raise SystemExit("Graphics must declare its session-owned PipeWire stack and lock helper")
minimal_recipe = (root/"packaging/apk/alcard-desktop-minimal/APKBUILD").read_text()
minimal_dependencies = re.search(r'^depends="([^"]+)"', minimal_recipe, re.M)
if not minimal_dependencies or set(minimal_dependencies.group(1).split()) & (audio_packages | {"pipewire-tools", "alsa-utils"}):
    raise SystemExit("Shell audio stack must not become a Minimal dependency")
for layout in ("us", "ru", "es", "de"):
    if not (root/f"keyboard-zero/console/{layout}.map").is_file():
        raise SystemExit(f"Missing console layout {layout}")
for lang in ("en", "ru", "es"):
    path = root/f"rust/crates/alcard-l10n/locales/{lang}.json"
    if not path.is_file():
        raise SystemExit(f"Missing catalog: {lang}")
values = [json.loads((root/f"rust/crates/alcard-l10n/locales/{lang}.json").read_text())
          for lang in ("en","ru","es")]
keys = set(values[0])
if any(set(v) != keys or not all(v.values()) for v in values):
    raise SystemExit("EN/RU/ES catalogs differ or contain empty values")
lock = (root/"rust/Cargo.lock").read_text() if (root/"rust/Cargo.lock").exists() else ""
if 'name = "zeroize"' not in lock:
    raise SystemExit("Cargo.lock missing zeroize; regenerate and commit the console lockfile")
if (root/"rust/apps/alcard-setupd/src/ipc.rs").exists():
    raise SystemExit("Removed developer socket unexpectedly exists")
print(f"PASS: {len(list(root.glob('packaging/apk/*/APKBUILD')))} APKBUILDs, "
      f"{len(keys)} localization keys, current package pins and full-image dependency contract")
PY
echo "==> PASS static sources (Cargo build and actual APK resolution still require execution)"

