# APK packages and ownership

**Language:** English · [Русский](../ru/packaging.md)

> Scope: current source tree · Updated: 2026-11-22

Alcard uses Alpine APK for both image construction and installed software. The repository contains 15 recipes: six hardware/BSP/profile recipes and nine user/session recipes. An image selects one of the two board profiles. This count excludes the ordinary Alpine dependencies resolved by APK.

## Current source versions

The [APKBUILD files](../../packaging/apk/) are the authority for `pkgver`, `pkgrel`, dependencies and file ownership. [package_metadata.py](../../tools/package_metadata.py) reads declarations without executing the recipes; builders and verifiers use this shared reader. The package inventory below describes source responsibilities. A source recipe version alone does not establish an accepted binary release.

Read current values directly from the recipes:

```sh
python3 tools/package_metadata.py manifest
python3 tools/package_metadata.py filename --group user
python3 tools/package_metadata.py kernel-version
python3 tools/package_metadata.py kernel-abi
```

| Package | Responsibility |
| --- | --- |
| `alcard-kmods-rpi` | External M5Stack modules for the exact Raspberry Pi kernel ABI |
| `alcard-firmware-zero` | ST7789 MIPI DBI initialization and BMI270 data firmware |
| `alcard-boot-zero` | Four shared v5 DTBO files |
| `alcard-bsp-zero` | Dependency-only hardware bundle |
| `alcard-board-zero-v5` | Standard v5 active boot configuration |
| `alcard-board-zero-v5-lite` | Alternative v5 Lite boot configuration |
| `alcard-keyboard-zero` | US/RU/ES/DE XKB and console layout data |
| `alcard-fonts-console` | Regular/Bold PSF2 console fonts |
| `alcard-fonts-gui` | Regular/Bold TTF fonts for applications such as Foot |
| `alcard-desktop-minimal` | labwc/Pixman and Foot fallback session |
| `alcard-hello` | Ratatui Settings/Hello, setup backend, firstboot/login entry points, shared radio service configuration |
| `alcard-shell` | Shell and restricted radio helper |
| `alcard-polkit-agent` | Graphical authentication agent |
| `alcard-lock` | Independent session locker and its PAM profile |
| `alcard-desktop-shell` | Main graphical session, supervision and PipeWire integration |

There is no current `alcard-repository`, `alcard-base`, `alcard-board-zero`, v3 profile or separate GUI Settings APK. Those names must not appear as required build inputs. `alcard-settings` is the executable shipped by `alcard-hello`.

## Hardware package boundary

The fixed kernel base is `linux-rpi=6.18.60-r0`, with ABI `6.18.60-0-rpi`. The kmods package revision `r3` is unrelated to the kernel package revision `r0`. Matching `linux-rpi-dev` is a build dependency and is not installed on the target merely to support modules.

| Owner | Installed material |
| --- | --- |
| Alpine `linux-rpi` | Kernel image and standard kernel modules |
| Alpine `raspberrypi-bootloader` | Raspberry Pi boot firmware |
| Alpine firmware packages | Standard firmware, including Broadcom resources required by the selected platform |
| `alcard-kmods-rpi` | `/usr/lib/modules/<ABI>/extra/alcard/*.ko` and `/usr/share/alcard/kmods-rpi-selection.txt` |
| `alcard-firmware-zero` | `/usr/lib/firmware/cardputerzero,st7789v.bin`, `/usr/lib/firmware/bmi270-init-data.fw`, source description and Bosch notice |
| `alcard-boot-zero` | Shared overlays under `/boot/overlays/`, without active `alcard.cfg` |
| One board-profile APK | `/boot/alcard.cfg` |
| `alcard-bsp-zero` | Exact dependencies only; no duplicated payload |

Both board profiles depend on the common BSP and conflict with one another. The common BSP does not select a profile. The standard profile enables the BMI270/BMM150 overlay; Lite omits the absent sensors. Camera support is described by the hardware sources, but the IMX219 line in the standard boot configuration remains commented until a matching physical camera is verified. See [hardware](hardware.md).

The kmods build chooses stock or vendor providers from the exact kernel configuration and exported symbols. It must fail on unresolved duplicate providers or ABI mismatches. `powerfail_suo` is not packaged. Firmware, DT and layout data have separate owners; no factory `.ko` from another kernel is copied into the system. Alpine's kmod integration supplies dependency indexes; isolated verification also invokes `depmod` explicitly against the target `/usr/lib/modules` tree.

## User and session package boundary

| Owner | Main files or behaviour |
| --- | --- |
| `alcard-hello` | `/usr/bin/alcard-settings`, `/usr/sbin/alcard-setupd`, Settings desktop entry, tty1 dispatcher, firstboot and session startup helpers, greetd/PAM configuration, BlueZ activation/configuration |
| `alcard-keyboard-zero` | `/usr/share/X11/xkb/symbols/alcard`, `/usr/share/alcard/keyboard/console/*.bmap.gz`, source maps and layout catalog |
| `alcard-fonts-console` | `/usr/share/kbd/consolefonts/AlcardPixel-*.psf` |
| `alcard-fonts-gui` | `/usr/share/fonts/alcard/AlcardPixel-*.ttf` |
| `alcard-desktop-minimal` | `/usr/libexec/alcard/wayland-session`, fallback labwc assets and Foot configuration |
| `alcard-shell` | `/usr/bin/alcard-shell`, `/usr/libexec/alcard/alcard-radio-helper`, radio polkit policy |
| `alcard-polkit-agent` | `/usr/bin/alcard-polkit-agent` |
| `alcard-lock` | `/usr/bin/alcard-lock`, `/etc/pam.d/alcard-lock` |
| `alcard-desktop-shell` | Main Shell session/client, audio supervisor, primary labwc configuration and the corresponding runtime dependency closure |

`alcard-hello` owns NetworkManager/BlueZ integration needed after a Console installation. It does not depend on removable greetd/labwc/Foot packages. Hello removes the defined graphical/login package set only when applying Console; both graphical choices retain Shell and the fallback. See [installation](installation.md) and [security](security.md).

The main session depends on the three native graphical applications and the minimal profile. Its PipeWire/WirePlumber/Pulse/ALSA compatibility stack is session-owned. The minimal profile does not acquire that stack solely for Shell controls. Runtime clients such as `wpctl`, `pw-dump`, `nmcli`, BlueZ and polkit are target dependencies; their presence in a developer's host PATH is not proof of the target dependency closure.

## How builders produce signed APKs

The host [build entry point](../../build.sh) dispatches to dedicated scripts in a persistent Alpine aarch64 environment. Working files, caches, archives, indexes and APKs are under the ignored `.alcard-local/` directory. Builders use `abuild -d`: prepare dependencies and keys explicitly before building.

Rust builders use native Alpine tools, committed lockfiles and `cargo build --release --locked --bins`. They stage the resulting ELF binaries and selected runtime assets into deterministic tar/gzip archives. The Rust APKBUILD recipes package those prebuilt artifacts; the recipe alone is not the complete Rust compilation procedure. Publish the source commit, locks and builder provenance alongside a future binary release.

Tracked APKBUILD files are recipe templates. Their empty `sha512sums` are filled by `abuild checksum` in the disposable recipe copy, then `abuild` creates signed packages and indexes. This does not disable signature verification. Upstream M5Stack input is fixed to commit `4f7aeee9fc103ebb03d22cac476bb191ecfb321e`; patches apply to extracted build sources, not to the cached upstream checkout.

[Production verification](../../tools/verify-production-build.py) rejects test, benchmark and example artifacts, validates expected release binary paths, and compares installed binaries with the release build. Its installed-package check uses an exact file allowlist. Separate test sources remain in Git and are not installed as runtime payload.

License declarations apply to their actual content. The project license does not replace GPL-2.0 BSP provenance, the LCD/Bosch firmware terms or CC0 font terms. See [licensing](licensing.md).

## Dependency and update policy

Kernel changes form a coordinated release unit:

1. Obtain matching `linux-rpi` and `linux-rpi-dev` for the chosen Alpine stable branch.
2. Recheck Kconfig, `Module.symvers`, providers, module ABI and `MODPOST`.
3. Rebuild modules and affected firmware/DT/profile packages.
4. Verify both mutually exclusive profiles in clean root filesystems.
5. Validate the actual boot payload and hardware before promoting a supported release.

An APK dependency pin protects dependency resolution; it does not synchronize FAT or preserve an old kernel's modules during an A/B trial. The current image also pins the kernel in `/etc/apk/world`. A future new-kernel release needs an intentional world-pin transition, preserving the old ABI tree while it is needed. Force options and manual package-database changes are not the normal update procedure. See [boot and updates](boot-updates.md).

## Repository publication is still pending

The current builders use a signed local repository at `.alcard-local/packages/alcard/aarch64/`. The generated image lists Alpine `v3.24/main` and `v3.24/community`; it does not yet configure a public Alcard update channel. Some Alcard user/session versions are pinned in `world`, so their eventual update policy must be defined as well.

A future public channel requires a trusted public key/fingerprint, signed index, versioned architecture/Alpine branch, complete compatible package sets, retention of exact upstream APK inputs and a documented key-rotation procedure. Private signing keys stay outside Git and ordinary release artifacts. Testing and stable promotion must describe the actual acceptance level. There is no implemented unattended BOOT updater.

## Validation boundaries

- `verify-bsp*` checks the signed hardware packages, ABI, firmware and combined Device Tree/Kconfig inputs.
- `install-verify*` uses an isolated dependency root with `--usermode --no-scripts`; it is not a bootable image.
- `shell-install-verify` checks signed user/session packages installed into a disposable Alpine rootfs.
- Image builders run real package scripts/triggers and `mkinitfs`; offline image verification inspects the finished filesystems.
- VM acceptance exercises userland on `linux-virt`; physical Cardputer behaviour and Raspberry Pi boot selection remain separate.

Use [building](building.md) for the complete sequence, [testing](testing.md) for test scope and [current status](status.md) for readiness boundaries.
