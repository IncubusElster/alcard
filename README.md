# Alcard

**English** · [Русский](README.ru.md) · [Documentation](docs/README.md)

Alcard is a compact **Alpine Linux distribution for M5Stack Cardputer Zero**, built around OpenRC, APK, a keyboard-operated interface and a software-rendered Wayland desktop.

**Development status: experimental, hardware bring-up.** The current source has automated checks; complete installed-system and physical Cardputer Zero acceptance remain separate requirements. This repository does not yet provide a supported public image or an operational Alcard APK update channel.

## What is implemented

- A shared image with **Graphics**, **Graphics Minimal** and **Console** first-run choices.
- **Hello / Settings** in Rust and Ratatui. `alcard-settings --hello` is the first-run wizard; ordinary `alcard-settings` opens eight settings sections.
- **NetworkManager** for network ownership and saved connection profiles, including the Hello path.
- **Alcard Shell** on SCTK, tiny-skia and Wayland shared-memory buffers, with an application menu, recent windows, notifications, themes and quick settings.
- Separate **graphical polkit agent** and **PAM-backed session locker**.
- **PipeWire / WirePlumber** audio in the main Graphics session, brightness and volume controls, idle/display handling, suspend coordination and battery policy.
- Cardputer Zero board profiles, keyboard data, Alcard Pixel fonts and signed APK/image build tooling.

The user interface currently supports English, Russian and Spanish. This documentation has an English primary edition and a maintained Russian translation. The standalone graphical Settings application remains planned; the desktop Settings entry currently opens the same TUI in a terminal.

## Installation modes

| Hello choice | Session | Package policy |
| --- | --- | --- |
| Graphics | labwc/Pixman with Alcard Shell and its supervised agents | Keeps Shell and the Foot fallback profile |
| Graphics Minimal | labwc/Pixman with Foot | Keeps both graphical package sets; runs the minimal profile |
| Console | Normal console login | Removes the defined graphical/login packages; retains TUI settings, networking and the hardware base |

Details: [installation and first boot](docs/en/installation.md), [desktop controls](docs/en/desktop.md), [settings](docs/en/settings.md), [authentication and recovery](docs/en/security.md).

## Start here

| Goal | Guide |
| --- | --- |
| Understand the design and current boundaries | [Overview](docs/en/overview.md) |
| Prepare a clean build environment and build packages/images | [Building Alcard](docs/en/building.md) |
| Use Hello and ordinary system settings | [Installation](docs/en/installation.md) and [Settings](docs/en/settings.md) |
| Work on the Cardputer Zero BSP | [Hardware](docs/en/hardware.md) and [boot/update design](docs/en/boot-updates.md) |
| Change code or documentation | [Contributing](CONTRIBUTING.md) and [testing](docs/en/testing.md) |
| Find unfinished work | [Roadmap](docs/en/roadmap.md) |
| Understand what was actually verified | [Current status](docs/en/status.md) |

The complete build route is in one [build guide](docs/en/building.md). Run this cheap source preflight from the repository root before preparing a build environment:

```sh
bash build.sh audit
python3 tools/check-docs.py
```

These checks do not build APKs or boot a VM. Builds use a prepared Alpine aarch64 environment; the normal workers do not install their prerequisites or generate signing keys automatically. Generated files, caches and local signing inputs belong in ignored `.alcard-local/` storage.

## Current readiness

[Alcard CI](https://github.com/IncubusElster/alcard/actions/workflows/ci.yml) is the single verification entry point. It uses one runner, runs static/documentation preflight on changes, and adds Alpine source checks when code or build inputs change. Full package, VM and Hello acceptance is an explicit manual mode with sequential stages.

Read the run result for the exact commit being assessed. Source checks do not establish physical display/input, radio, audio, suspend or energy behavior. Current NetworkManager/Settings service integration and Cardputer hardware acceptance remain open.

The [current status](docs/en/status.md) separates implemented features, automated checks and release limitations. Follow the [roadmap](docs/en/roadmap.md) for remaining work and [packaging](docs/en/packaging.md) for current package inputs.

## License

Original Alcard work is licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE) and [NOTICE](NOTICE). M5Stack-derived hardware material, Bosch firmware and UNSCII font data retain their separately identified licenses; see [third-party notices](THIRD_PARTY_NOTICES.md) and the [licensing guide](docs/en/licensing.md).
