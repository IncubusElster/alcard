# Building Alcard

**Language:** English · [Русский](../ru/building.md)

> Scope: current source tree · Updated: 2026-11-22

This is the complete path from a checkout to signed packages and an offline Cardputer image. It uses the existing build scripts and a persistent **Alpine 3.24 aarch64** Distrobox. A successful build is not physical Cardputer acceptance; see [testing](testing.md).

## Build environment

The host needs Git, Bash, Python 3, rootless Podman and Distrobox. The static audit runs Python directly on the host; use Python 3.11+ to cover the repository's Python tools as well. On an x86-64 host, ARM64 QEMU user-mode/binfmt must already work. No build script installs host packages, creates the Distrobox or writes a physical SD card.

Clone the repository using your normal GitHub access and work from its root:

```sh
git clone https://github.com/IncubusElster/alcard.git
cd alcard
bash build.sh help
```

`build.sh` is executable; explicit `bash` is also supported. Paths are resolved relative to the checkout, not to a maintainer's home directory. Build inputs, caches, signing-related local configuration and artifacts belong under ignored local paths, not in source commits.

On the host, verify ARM execution:

```sh
podman run --rm --platform linux/arm64 docker.io/library/alpine:3.24 uname -m
```

The result must be `aarch64`. An `exec format error` means host emulation is not ready. Resolve that using the host distribution's supported binfmt/QEMU setup before continuing.

Create the container once, if it does not already exist:

```sh
distrobox create --name alcard-arm64 \
  --image docker.io/library/alpine:3.24 \
  --additional-flags "--platform=linux/arm64"
distrobox enter alcard-arm64
apk --print-arch
exit
```

Keep a working `alcard-arm64`; ordinary builds reuse it. `ALCARD_BUILD_BOX` overrides its name. Distrobox must expose the checkout at the same absolute path used by the host commands because workers are invoked through that path.

## Install build dependencies once

Run the following **on the host**. Root is used only inside the existing rootless container:

```sh
ALCARD_KERNEL_VERSION="$(python3 tools/package_metadata.py kernel-version)"
podman exec --user 0 alcard-arm64 apk update
podman exec --user 0 alcard-arm64 apk add \
  alpine-sdk bash git python3 dtc kmod \
  coreutils findutils grep sed tar gzip mawk \
  bc perl bison flex openssl-dev elfutils-dev \
  "linux-rpi=$ALCARD_KERNEL_VERSION" "linux-rpi-dev=$ALCARD_KERNEL_VERSION" \
  cargo rust rustfmt rust-clippy pkgconf \
  libxkbcommon-dev wayland-dev glib-dev polkit-elogind-dev linux-pam-dev \
  py3-fonttools py3-pillow kbd \
  dosfstools e2fsprogs e2fsprogs-extra mtools mkinitfs cpio
```

Use the matching `v3.24/main` and `v3.24/community` repositories. Do not mix `edge` into this build environment. If the pinned kernel packages have disappeared from a mirror, restore the matching trusted package snapshot or prepare a coordinated BSP update; changing only the header version is not a repair.

| Group | Why it is needed |
| --- | --- |
| `alpine-sdk`, archive tools, Git | `abuild`, compiler toolchain, source/archive staging and signatures |
| Exact kernel/dev packages, Kbuild tools, `dtc`, `kmod` | External modules, ABI/symbol checks and DTBO |
| Native Alpine Rust and development libraries | Console and independent graphical programs |
| Python 3.11+, fontTools, kbd | TOML firmware inputs, font generation and binary console maps |
| Filesystem tools, `mkinitfs`, `cpio` | Offline FAT/ext4 image construction and inspection |

The native source preflight requires Rust **1.96.1 or newer**; the manifest minimum is **1.96**. Builders deliberately use `/usr/bin/cargo`, `/usr/bin/rustc` and `/usr/bin/rustdoc`, with separate aarch64/musl caches. A host rustup shim in the shared Distrobox home is not a substitute. The device image does not need the compiler, kernel headers, fontTools or image-inspection tools merely because the builder uses them.

Runtime dependencies are resolved while constructing the disposable target rootfs. Installing labwc or NetworkManager on the build host does not establish that they are correctly declared in the target packages.

## Set up APK signing once

Generate a key as the normal build user inside Distrobox:

```sh
distrobox enter alcard-arm64
abuild-keygen -a -n
printf '%s\n' "$HOME"
ls -l ~/.abuild/*.rsa.pub
exit
```

`-a` records the private-key location in the user's abuild configuration. `-n` makes generation noninteractive. The `-i` option is unnecessary here: it attempts privileged installation of the public key and can fail in a rootless Distrobox setup.

Use the printed build-user home path, which may be under `/home` or `/var/home`. Open a root shell inside the same container:

```sh
podman exec -it --user 0 alcard-arm64 /bin/sh
```

In that container shell, set the actual path and install **only public keys**:

```sh
ALCARD_SIGNING_HOME=/actual/build-user/home
install -d -m 755 /etc/apk/keys
install -m 644 "$ALCARD_SIGNING_HOME"/.abuild/*.rsa.pub /etc/apk/keys/
exit
```

Replace `/actual/build-user/home` before running the command. Keep private `.rsa` keys outside Git and ordinary artifacts. Keys generated for local testing are not automatically the future public release key. Package builders run as the ordinary user; they reject Cargo/abuild execution as root. `abuild -d` expects prepared dependencies rather than installing them during a build.

## Run the static preflight

From the repository root on the host:

```sh
bash build.sh audit
```

This checks source syntax, assets, package constraints and localization catalogs. It does not build Rust or prove that an Alpine mirror can resolve every package. The kernel-specific `doctor` below is not a universal check for all user/session and image prerequisites.

## Build the hardware packages

For a complete Cardputer image, build the common BSP and both profile packages. Only one profile is installed into each resulting image.

```sh
bash build.sh doctor
bash build.sh kmods
bash build.sh firmware-check
bash build.sh firmware
bash build.sh boot-check
bash build.sh boot
bash build.sh bsp-check
bash build.sh bsp
bash build.sh board-v5-check
bash build.sh board-v5
bash build.sh board-lite-check
bash build.sh board-v5-lite
bash build.sh verify-bsp
bash build.sh verify-bsp-lite
bash build.sh install-check
bash build.sh install-verify
bash build.sh install-verify-lite
```

The module builder selects providers from the exact kernel `.config` and `Module.symvers`. A BQ27xxx/provider conflict is an intentional stop; do not force a duplicate module into the package. See [hardware](hardware.md) for provider rules.

`verify-bsp*` checks signatures, module ABI, firmware hashes, merged DT and relevant Kconfig. `install-verify*` uses `.alcard-local/verify/install-rootfs/` with `apk --root --usermode --no-scripts`, then checks module indexes. It exercises package resolution without modifying the host's `/boot`; it does not perform full package scripts or create a bootable image.

For a QEMU-only userland exercise, the next section is sufficient; the Cardputer BSP is not the kernel used by the virtual machine.

## Build all nine user/session packages

Run these commands from the repository root on the host, as the normal user:

```sh
bash build.sh keyboard-check
bash build.sh keyboard
bash build.sh fonts-check
bash build.sh fonts
bash build.sh fonts-gui-check
bash build.sh fonts-gui
bash build.sh desktop-check
bash build.sh desktop
bash build.sh hello-check
bash build.sh hello
bash build.sh shell-apk-check
bash build.sh shell-apks
bash build.sh shell-install-verify
```

The first five build commands produce keyboard, console fonts, GUI fonts, minimal desktop and Hello. `shell-apks` adds Shell, polkit-agent, locker and the primary desktop session. These nine packages are all required by the current full image builders; choosing Console is a firstboot action, not a separate image-build toggle.

Output is `.alcard-local/packages/alcard/aarch64/`, including architecture-independent packages stored in the aarch64 repository. The metadata commands in [packaging](packaging.md) show expected revisions and filenames directly from APKBUILD. `shell-install-verify` verifies signatures and performs a real isolated installation into `.alcard-local/session-install/rootfs/`, checking package ownership, permissions, AArch64/musl ELF, libraries and authentication helpers. It uses container root, not host root.

Release builds use committed Cargo locks and `--release --locked --bins`. The production verifier rejects test/example/benchmark artifacts and compares installed programs with the release outputs. Native test compilation is a separate activity; use [testing](testing.md) rather than changing production build flags.

## Build and verify Cardputer images

After both the hardware packages and the nine user/session packages are available:

```sh
bash build.sh image-check-v5
bash build.sh image-v5
bash build.sh image-verify-v5
bash build.sh image-check-lite
bash build.sh image-v5-lite
bash build.sh image-verify-lite
```

The outputs are:

| Profile | Image | Sidecars |
| --- | --- | --- |
| Standard v5 | `.alcard-local/images/alcard-zero-v5-0.1.0.img` | Matching `.json` manifest and `.img.sha256` |
| v5 Lite | `.alcard-local/images/alcard-zero-v5-lite-0.1.0.img` | Matching `.json` manifest and `.img.sha256` |

The worker recreates `.alcard-local/image-stage/`, installs Alpine with package scripts/triggers, runs `mkinitfs`, and builds three FAT partitions plus one persistent ext4 ROOT. It uses regular files rather than loop devices or physical disks. A later build of the same profile replaces that profile's output. Default partition sizes and known release limitations are in [boot and updates](boot-updates.md).

Offline verification checks SHA-256, MBR/partitions, FAT content, ext4 with `e2fsck -fn`, UUID/PARTUUID, package database/world, profiles, module indexes, firmware, service configuration and gzip/cpio initramfs. It does not prove physical boot, LCD orientation, keyboard input or A/B rollback.

## QEMU alternative

The user/session packages can also produce a disposable Alpine aarch64 `virt` image:

```sh
bash build.sh qemu-hello-check
bash build.sh qemu-hello
```

The builder writes `.alcard-local/qemu-aarch64/`, uses `linux-virt` and generates the corresponding launch helper. This serial Hello path is distinct from the real-VT login VM acceptance. Consult [testing](testing.md) for the available runners and their assumptions. Do not substitute the QEMU image for a Cardputer SD image.

## Troubleshooting and release records

| Symptom | Check |
| --- | --- |
| `exec format error` before the container runs | Host ARM64 binfmt/QEMU and requested platform |
| Cargo path exists but cannot execute | Native Alpine `/usr/bin` toolchain; shared host rustup shims |
| Missing signed APK or index | Complete the preceding build group; check trusted public key and the actual recipe revision |
| kmods provider/undefined-symbol failure | Exact kernel/dev pair, `.config`, `Module.symvers`, selected vendor family and patch |
| Image lacks `debugfs` or `cpio` verification tools | Install `e2fsprogs-extra` and `cpio` in the builder container |
| Image rootfs exceeds its capacity allowance | Increase `ALCARD_ROOT_MIB` consciously; the worker limits staged usage to 75% of the requested partition |

Do not bypass a signature, ABI or package-conflict check to obtain an image. Keep the source commit, Cargo locks, toolchain versions, resolved APK inventory, image manifest and output hashes together for acceptance. The current `alpine:3.24` tag and branch repositories can change, and disk IDs/UUIDs are generated afresh; the builder does not promise bit-for-bit identical full images.

Normal images leave root locked for Hello. The explicit developer-root path is documented in [boot and updates](boot-updates.md); it currently shares the normal output filename and is not suitable for indistinguishable public release artifacts. No SD flashing command is part of this build sequence.
