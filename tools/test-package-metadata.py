#!/usr/bin/env python3
"""Exercise package identity changes, stale pins and non-executable metadata."""

from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

from package_metadata import Catalog, GROUPS, ROOT


def rejected(root, message):
    try:
        Catalog(root)
    except ValueError as error:
        assert message in str(error), error
    else:
        raise AssertionError("Invalid package metadata was accepted")


def main():
    current = Catalog()
    assert len(GROUPS["user"]) == len(set(GROUPS["user"])) == 9
    with tempfile.TemporaryDirectory(prefix="alcard-package-metadata-") as directory:
        root = Path(directory)
        shutil.copytree(ROOT / "packaging/apk", root / "packaging/apk")
        recipes = root / "packaging/apk"

        # A valid version bump must change both the chosen filename and the
        # deterministic archive prefix, without a second version declaration.
        recipe = recipes / "alcard-desktop-shell/APKBUILD"
        original = recipe.read_text()
        version = current["alcard-desktop-shell"].pkgver + "_p1"
        recipe.write_text(original.replace("pkgver=" + current["alcard-desktop-shell"].pkgver,
                                           "pkgver=" + version, 1))
        metadata = Catalog(root)["alcard-desktop-shell"]
        assert metadata.filename == f"alcard-desktop-shell-{version}-r{metadata.pkgrel}.apk"
        shutil.copytree(ROOT / "scripts/wayland", root / "scripts/wayland")
        archive = root / "session.tar.gz"
        subprocess.run([sys.executable, str(ROOT / "tools/stage-shell-apk.py"),
                        "--root", str(root), "--profile", "session", "--target", str(root),
                        "--output", str(archive)], check=True, stdout=subprocess.PIPE, text=True)
        with tarfile.open(archive) as stream:
            members = stream.getmembers()
            assert len(members) == 6
            assert all(member.name.startswith(f"alcard-desktop-shell-{version}/")
                       for member in members)
            assert all(member.uid == member.gid == member.mtime == 0 for member in members)
        recipe.write_text(original)

        # Changing a dependency's own revision must invalidate a stale exact
        # consumer pin instead of building/verifying against the older package.
        recipe = recipes / "alcard-lock/APKBUILD"
        original = recipe.read_text()
        revision = current["alcard-lock"].pkgrel
        recipe.write_text(original.replace("pkgrel=" + revision,
                                           "pkgrel=" + str(int(revision) + 1), 1))
        rejected(root, "stale dependency alcard-lock=")
        recipe.write_text(original)

        # Shell syntax is data here. Neither substitutions nor a declaration
        # after an unexpected duplicate can select a different package identity.
        marker = root / "executed"
        recipe.write_text(original.replace("pkgver=" + current["alcard-lock"].pkgver,
                                           f"pkgver=$(touch {marker})", 1))
        rejected(root, "Shell expressions are not allowed")
        assert not marker.exists()
        recipe.write_text(original + "\npkgrel=99\n")
        rejected(root, "Expected one pkgrel assignment")
        recipe.write_text(original)

        # A copied checkout is self-contained: it reads its own recipes rather
        # than the caller's checkout. No package declarations are imported from
        # the running host's APK database.
        assert Catalog(root).manifest() == current.manifest()
        output = subprocess.check_output(
            [sys.executable, str(ROOT / "tools/package_metadata.py"),
             "--root", str(root), "pin", "--group", "user"], text=True)
        assert output.splitlines() == [current[name].pin for name in GROUPS["user"]]

        # The real installed-system verifier must still reject an old package,
        # even when its name and the complete base package set are present.
        database = root / "rootfs/usr/lib/apk/db/installed"
        database.parent.mkdir(parents=True)
        database.write_text("\n\n".join(
            f"P:{name}\nV:{'0-stale' if name == 'alcard-hello' else current[name].version}"
            for name in GROUPS["base"]) + "\n")
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/verify-session-install.py"),
             "--root", str(root / "rootfs"), "--source", str(root), "--mode", "console"],
            text=True, capture_output=True)
        assert result.returncode != 0
        assert f"Expected {current['alcard-hello'].pin}, got 0-stale" in result.stderr, result.stderr
    print("PASS: version changes reach archive inputs; stale pins, shell expressions and duplicates fail; copied-checkout metadata is self-contained")


if __name__ == "__main__":
    main()
