# Project overview and current status

**Language:** English · [Русский](../ru/overview.md)

> Scope: current source tree · Updated: 2026-11-22

Alcard is a compact Alpine Linux distribution and desktop environment for the **M5Stack Cardputer Zero**. It combines board integration, signed APK packages, persistent microSD images, first-run setup, a terminal settings application and a keyboard-oriented Wayland shell. The initial target is the aarch64 CM0/RP3A0 platform with 512 MiB RAM and the built-in 320×170 display.

**Hardware status: bring-up.** The repository contains implemented system components and verification tools. A successful source build or virtual-machine run is not acceptance of a physical Cardputer. Use the [hardware guide](hardware.md) for board-specific assumptions and the [current status](status.md) for readiness and verification boundaries.

## Supported installation modes

Hello offers three modes from the same installation image. The saved session value is an implementation detail with a defined meaning:

| Hello choice | Saved value | Session after setup |
| --- | --- | --- |
| Graphics | `alcard-shell` | labwc/Pixman, Alcard Shell, graphical polkit-agent and the supervised user session |
| Graphics Minimal | `labwc` | labwc/Pixman and Foot as the diagnostic graphical session |
| Console | `console` | Regular TTY login; first-run finalization removes the fixed graphical/login package set |

Both graphical choices retain the Shell and fallback packages. Console retains terminal settings, networking, keyboard support and the board base. A fallback Foot session remains available as a distinct installed profile; an automatic change from a broken Shell session to Foot is not implemented. See [installation and recovery](installation.md).

## Current implementation

| Area | Implemented state | Acceptance boundary |
| --- | --- | --- |
| Board integration | Pinned Raspberry Pi kernel, vendor modules, display firmware generation and v5/v5-lite profiles | Package/ABI/isolated-install checks are automated; physical board operation is still open |
| Setup | `alcard-settings --hello`, account/admin policy, language, keyboard, time, networking and session selection; root-only `alcard-setupd` | Hello/login/reboot scenarios are included in the manual acceptance path |
| Everyday settings | Eight Ratatui sections in ordinary `alcard-settings`; one system-language setting; shared settings core | Source checks are automated; focused installed-system acceptance is pending |
| Networking | NetworkManager owns Wi-Fi, Ethernet and saved profiles in Hello and Settings; Bluetooth management uses BlueZ | Actual adapters, RF operations and persistence on Cardputer need device acceptance |
| Desktop | Application menu, Recent/windows, notifications, themes, application search, quick settings and system actions | Native/compositor and installed-package scenarios are included in manual acceptance |
| Authentication | Separate graphical polkit-agent and PAM-backed native session locker; greetd/PAM/elogind login | Real PAM/polkit/compositor and VM recovery need their corresponding acceptance stages |
| Display, audio and power | Idle lock/output power, brightness controls, PipeWire/WirePlumber volume/mute, sleep coordination and battery policy | Real isolated audio/compositor tests are distinguished from simulated sysfs/login1 and physical power operations |
| Images and updates | Persistent ext4 ROOT and three FAT boot partitions; signed local APK output | Public Alcard APK channel, automatic BOOT activation/rollback and physical image acceptance are unfinished |
| GUI Settings | Planned separate graphical application using the same operations as TUI | Not implemented; Shell quick settings are a different existing interface |

The settings sections are System, Language and keyboard, Date and time, Network, Bluetooth, Display and power, Sound, and Appearance. The menu entry currently opens the Ratatui application in a terminal. See [Settings and Hello](settings.md), [desktop behavior](desktop.md) and [power and audio](power-audio.md).

## Verification scope

The [current status](status.md) separates implemented functionality, automated checks and pending release/device acceptance. [Alcard CI](https://github.com/IncubusElster/alcard/actions/workflows/ci.yml) records the exact source, mode, architecture and result of each current run. A successful source job does not certify an unrun installed-package or VM stage. The [testing guide](testing.md) defines the required levels and the [roadmap](roadmap.md) records the remaining work.

## Architecture and ownership

| Layer | Owner and responsibility |
| --- | --- |
| Linux and Alpine | Standard SoC, storage, USB/SPI/I²C/GPIO, network, graphics and audio subsystems; `linux-rpi`, OpenRC and APK |
| Pinned M5Stack sources | Board-specific external modules and source material for the hardware configuration; compiled for the exact target kernel ABI |
| Alcard board integration | Device Tree, standard `panel-mipi-dbi` integration, adapted LCD initialization data, service configuration and package relationships |
| Shared Rust crates | Input, localization, setup protocol/state, lock state, settings/preferences/themes and graphical presentation |
| User applications | Ratatui Settings/Hello, Shell, polkit-agent and locker, with separate process and dependency boundaries |
| Packaging and acceptance | Native builders, deterministic package input archives, signed APKs, isolated installation, VM and device checks |

The [console workspace](../../rust/Cargo.toml) and the four independent graphical workspaces deliberately have separate lockfiles. Console does not inherit Wayland, SCTK, GLib/GIO or graphical rendering dependencies. PAM and privileged operations remain outside the Shell's ordinary rendering process. The [security guide](security.md) explains these boundaries.

Graphics uses **labwc/wlroots with Pixman**, with Alcard clients built on **SCTK, tiny-skia and `wl_shm`**. PipeWire/WirePlumber belong to the Graphics user session in an OpenRC system; systemd user units are not the session mechanism. These choices are established architecture, not a list of alternative GUI frameworks.

### Rust implementation map

| Area | Internal responsibilities |
| --- | --- |
| [Hello state](../../rust/crates/alcard-setup-core/src/lib.rs) | `input.rs` owns editing/transitions, `view.rs` owns rows/softkeys, and `request.rs` validates and serializes the separate credential request |
| [Shared settings](../../rust/crates/alcard-settings-core/src/settings.rs) | Public row/action types; `settings/inventory.rs` reads state, `settings/operation.rs` validates and runs operations, and `settings/services.rs` contains bounded service adapters |
| [Setup backend](../../rust/apps/alcard-setupd/src/main.rs) | `apply.rs` owns firstboot provisioning, `settings.rs` owns the installed-settings allowlist, and `system_config.rs` provides shared configuration writes |
| [Shell model](../../rust/apps/alcard-shell/src/model.rs) | Shared state/navigation with application, notification and quick-settings behavior in private `model/` modules |
| [Shell application](../../rust/apps/alcard-shell/src/app.rs) | Event coordination with power and window handling in private `app/` modules |
| [Shell rendering](../../rust/apps/alcard-shell/src/render.rs) | Screen composition with status indicators and quick-settings drawing in private `render/` modules |
| [Sleep client](../../rust/apps/alcard-shell/src/sleep.rs) | Public client/state contract with login1 transport and monitoring in private `sleep/` modules |
| [Runtime language](../../rust/crates/alcard-l10n/src/locale.rs) | One `alcard_l10n::Locale` and common automatic detection for Settings, Shell, locker and polkit-agent |

Feature-specific translation text stays with its application; Settings/Hello retain their JSON catalogs. Configuration `Language::Auto` and the serialized setup-protocol language remain distinct from the resolved runtime locale. Tests live in separate test-only modules alongside the corresponding implementation.

## Repository map

| Path | Purpose |
| --- | --- |
| [`board/cardputer-zero`](../../board/cardputer-zero) | Board Device Tree and display initialization inputs |
| [`patches`](../../patches) | Narrow vendor-source integration patches |
| [`keyboard-zero`](../../keyboard-zero) | Keyboard layouts and their verification |
| [`fonts/alcard-pixel`](../../fonts/alcard-pixel) | Bitmap font sources used to generate console and GUI fonts |
| [`rust/apps`](../../rust/apps) | User applications and the setup backend |
| [`rust/crates`](../../rust/crates) | Shared application contracts and presentation code |
| [`packaging/apk`](../../packaging/apk) | Fifteen APK recipes; current package versions are authoritative here |
| [`scripts`](../../scripts), [`tools`](../../tools) | Build, configuration, verification and isolated acceptance tools |
| [Documentation index](../README.md) | Primary English guides and matching Russian translations |
| [CI workflow](../../.github/workflows/ci.yml) | One-runner static, source and explicit manual acceptance orchestration |

Local inputs, caches, signing material and generated packages/images belong under the ignored `.alcard-local/` directory. They are not source deliverables. Package build input archives and Cargo lockfiles are part of reproducible build inputs; see [building](building.md) and [packaging](packaging.md).

## Established constraints

- Keep one persistent ext4 ROOT containing the system, `/home`, configuration and APK database. The future Raspberry Pi `tryboot` design applies to BOOT slots; it does not roll back ROOT package changes.
- Prefer standard Linux drivers and interfaces. Alcard does not introduce a new package manager, bootloader or DRM driver for functionality already provided upstream.
- Build external modules against the matching kernel ABI. Do not repair compatibility by changing `vermagic` or installing modules from another kernel.
- Give each hardware resource one driver/owner; avoid competing Device Tree nodes, GPIO users or network managers.
- Preserve offline first-run setup, ordinary console login and the distinction between the three installation modes.
- Keep one system-language choice, mandatory original US keyboard layout and the additional supported layouts described in [fonts and keyboard](fonts-keyboard.md).
- Keep tests in source control and outside production APK payloads. Release verification checks actual binaries and installed files, not only successful unit-test counts.
- Mark device-only claims as awaiting hardware acceptance until the exact board, kernel, packages and observations are recorded.

Home encryption, factory reset, switching an installed system between Console and Graphics, a user-application distribution mechanism and additional base applications are future work with explicit scope decisions still required. They are not hidden features of the current release.

## How to use this documentation

Start with [building](building.md) for a developer environment, [installation](installation.md) for the boot/setup path, or [Settings](settings.md) for everyday controls. Contributors should read the [contribution guide](../../CONTRIBUTING.md), [testing](testing.md) and [roadmap](roadmap.md).

English is the primary maintained documentation; Russian pages are complete translations of the same contract. Keep current behavior in topical guides and readiness in the status page. The project's licence and third-party exceptions are defined in [licensing](licensing.md).
